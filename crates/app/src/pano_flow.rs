//! Photo → Photo merge → Panorama… (M10.17b) in the app: the dialog's state, the merge queued with the
//! HDR merges (`merge_queue`), and what happens when one lands.
//!
//! The decisions are the lib's (`rustroom_app::pano_merge`): which files, the name, the worker, LaMa. This
//! is the glue — the selection in, the preview's texture out, the download offered when Fill edges needs
//! LaMa, and on landing `land_merged` (the HDR merge's order).

use crate::modals::Modal;
use crate::RustroomApp;
use edit_model::Edits;
use eframe::egui;
use photo_merge::projection::Projection;
use photo_merge::rig::Rig;
use rustroom_app::hdr_merge::MergeKind;
use rustroom_app::pano_merge::worker::{FillFactory, PanoEvent, PanoWorker};
use rustroom_app::pano_merge::{self, lama, PanoPlan, PanoSettings};
use rustroom_app::photo_id::PhotoId;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The dialog while it is open.
pub(crate) struct PanoDialog {
    pub(crate) plan: PanoPlan,
    /// The options as set in the dialog; written to `Settings` only on Merge.
    pub(crate) draft: PanoSettings,
    /// What the preview worker was last given.
    sent: PanoSettings,
    worker: PanoWorker,
    pub(crate) preview: Option<egui::TextureHandle>,
    /// Auto crop's rectangle on the preview, and the projection it was stitched with (Auto's pick).
    pub(crate) crop: Option<edit_model::CropRect>,
    pub(crate) projection: Projection,
    /// A lens LensFun does not know.
    pub(crate) no_profile: Option<String>,
    /// The preview is for options other than the draft's: drawn dimmed until the next lands.
    pub(crate) stale: bool,
    /// The rig the merge starts from (the preview's).
    rig: Option<Rig>,
    pub(crate) failed: Option<String>,
    /// Frames that share nothing with the others.
    pub(crate) unconnected: Option<Vec<String>>,
    /// The projection asked for cannot hold the sweep.
    pub(crate) too_wide: bool,
    /// Where LaMa is looked for; `None` when there is no data folder at all.
    pub(crate) models: Option<PathBuf>,
    pub(crate) zoom: f32,
    pub(crate) pan: egui::Vec2,
    base: Edits,
}

impl PanoDialog {
    /// Merge is offered: the frames connect and the projection holds them.
    pub(crate) fn can_merge(&self) -> bool {
        self.unconnected.is_none() && !self.too_wide
    }
}

/// `draft` asks for Fill edges and LaMa is not in `models` — the dialog's line and download offer.
pub(crate) fn needs_lama(draft: &PanoSettings, models: Option<&Path>) -> bool {
    draft.fill_edges && !models.is_some_and(lama::present)
}

/// A panorama merge waiting its turn in [`crate::merge_queue`].
pub(crate) struct PanoJob {
    plan: PanoPlan,
    settings: PanoSettings,
    base: Edits,
    rig: Option<Rig>,
    models: Option<PathBuf>,
}

impl PanoJob {
    /// A job for `plan` with the defaults, no rig, no LaMa — for the queue's tests.
    #[cfg(test)]
    pub(crate) fn for_test(plan: PanoPlan) -> Self {
        Self {
            plan,
            settings: PanoSettings::default(),
            base: Edits::default(),
            rig: None,
            models: None,
        }
    }

    /// Its worker, started.
    pub(crate) fn start(&self) -> PanoWorker {
        PanoWorker::merge(
            self.plan.clone(),
            self.settings,
            self.base.clone(),
            self.rig.clone(),
            filler(&self.models),
        )
    }

    /// Fill edges was asked for and could not be had.
    fn fill_missing(&self) -> bool {
        self.settings.fill_edges && !self.models.as_deref().is_some_and(lama::present)
    }
}

/// LaMa as the edge filler, when it is downloaded — loaded on the worker's thread, on first need.
fn filler(models: &Option<PathBuf>) -> Option<FillFactory> {
    let models = models.clone().filter(|m| lama::present(m))?;
    Some(Box::new(move |meta| {
        let net = rustroom_app::removal::lama(&models)?;
        Ok(Box::new(lama::LamaFill::new(net, meta)) as Box<_>)
    }))
}

/// The panorama dialog's state; the merges themselves queue in [`crate::merge_queue`].
#[derive(Default)]
pub(crate) struct PanoState {
    pub(crate) dialog: Option<PanoDialog>,
}

/// The options the preview stitches from — the stack changes nothing it draws.
fn stitches_differently(a: &PanoSettings, b: &PanoSettings) -> bool {
    (a.auto_projection, a.projection, a.auto_crop, a.fill_edges)
        != (b.auto_projection, b.projection, b.auto_crop, b.fill_edges)
}

fn texture(ctx: &egui::Context, img: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        "pano-preview",
        egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw()),
        egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        },
    )
}

impl RustroomApp {
    /// Photo → Photo merge → Panorama… on the selection: the dialog, or with `last` the merge itself with
    /// the options last used. A selection that cannot be merged says why in the status row.
    pub(crate) fn open_pano_merge(&mut self, last: bool) {
        let ids = self.selection_ids();
        self.open_pano_merge_for(&ids, last);
    }

    /// [`Self::open_pano_merge`] on these photos — the right-click's targets.
    pub(crate) fn open_pano_merge_for(&mut self, ids: &[PhotoId], last: bool) {
        let Some(lib) = &self.library else {
            return;
        };
        let row = |id: &PhotoId| {
            lib.entries
                .iter()
                .position(|e| e.id == *id)
                .unwrap_or(usize::MAX)
        };
        let mut ids = ids.to_vec();
        ids.sort_by_key(row);
        let photos = rustroom_app::batch_rename::photos_of(&self.files_of(&ids));
        let root = self.immich_cache_root();
        let cat = self.catalog.as_ref();
        let plan = pano_merge::plan(
            &photos,
            |p| {
                root.as_deref()
                    .is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, p))
            },
            // ⚠ Only a DNG can be a merged photo, and sniffing reads the whole file — never a RAF's.
            |p| {
                p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng"))
                    && raw_decode::is_float_dng(p)
            },
            |p| crate::move_flow::path_taken(cat, p),
            Path::exists,
        );
        let mut plan = match plan {
            Ok(p) => p,
            Err(r) => {
                self.status = r.sentence_for(MergeKind::Panorama).to_string();
                return;
            }
        };
        let first = plan.frames[0].clone();
        if !self.place_immich_merge(&mut plan.output, &first, "Pano") {
            return;
        }
        let base = self.settings.initial_edits(None);
        let settings = self.settings.panorama;
        let models = rustroom_app::ai_cache::models_dir();
        if last {
            self.queue_pano(PanoJob {
                plan,
                settings,
                base,
                rig: None,
                models,
            });
            return;
        }
        self.pano.dialog = Some(PanoDialog {
            worker: PanoWorker::preview(&plan, settings, base.clone(), filler(&models)),
            plan,
            draft: settings,
            sent: settings,
            preview: None,
            crop: None,
            projection: settings.projection,
            no_profile: None,
            stale: false,
            rig: None,
            failed: None,
            unconnected: None,
            too_wide: false,
            models,
            zoom: rustroom_app::view_region::MIN_ZOOM,
            pan: egui::Vec2::ZERO,
            base,
        });
        self.modal = Modal::PanoMerge;
    }

    /// The dialog's Merge: the options remembered, the merge queued, the dialog closed.
    pub(crate) fn start_pano_merge(&mut self) {
        let Some(d) = self.pano.dialog.take() else {
            return;
        };
        self.modal = Modal::None;
        // ⚠ Remembered here and only here — `start_import`'s rule, so Cancel leaves them alone.
        if self.settings.panorama != d.draft {
            self.settings.panorama = d.draft;
            self.save_settings();
        }
        self.queue_pano(PanoJob {
            plan: d.plan,
            settings: d.draft,
            base: d.base,
            rig: d.rig,
            models: d.models,
        });
    }

    /// The dialog's Cancel: the preview worker stops when it is dropped.
    pub(crate) fn close_pano_merge(&mut self) {
        self.pano.dialog = None;
        self.modal = Modal::None;
    }

    /// Fill edges needs LaMa: the dialog closes and the download Remove offers opens (M10.16c1).
    pub(crate) fn offer_lama_download(&mut self) {
        self.pano.dialog = None;
        self.ai.proposed = vec![rustroom_app::removal::MODEL];
        self.modal = Modal::AiDownload;
    }

    fn queue_pano(&mut self, job: PanoJob) {
        let name = job
            .plan
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = pano_merge::progress_line(
            &photo_merge::pano::Progress::Decoding {
                done: 0,
                total: job.plan.frames.len(),
            },
            &name,
        );
        self.queue_merge(crate::merge_queue::Job::Panorama(job), line);
    }

    /// Once per frame: the dialog's preview.
    pub(crate) fn pump_pano(&mut self, ctx: &egui::Context) {
        let Some(d) = self.pano.dialog.as_mut() else {
            return;
        };
        if stitches_differently(&d.draft, &d.sent) {
            d.worker.set_options(d.draft);
            d.sent = d.draft;
            d.stale = d.preview.is_some();
        }
        while let Ok(e) = d.worker.events().try_recv() {
            match e {
                PanoEvent::Preview {
                    image,
                    crop,
                    projection,
                    no_profile,
                    rig,
                } => {
                    d.preview = Some(texture(ctx, &image));
                    (d.crop, d.projection, d.no_profile, d.rig) =
                        (crop, projection, no_profile, Some(*rig));
                    (d.stale, d.failed, d.too_wide) = (false, None, false);
                }
                PanoEvent::Unconnected(names) => d.unconnected = Some(names),
                PanoEvent::TooWide => {
                    (d.too_wide, d.stale) = (true, false);
                }
                PanoEvent::Failed(e) => d.failed = Some(e),
                _ => {}
            }
        }
        if d.preview.is_none() || d.stale {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The running panorama merge ended with `e` (Done, Failed or Cancelled).
    pub(crate) fn land_pano(&mut self, job: &PanoJob, e: PanoEvent) {
        let name = job
            .plan
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match e {
            PanoEvent::Done { path, edits } => {
                self.land_merged(&job.plan.frames, job.settings.create_stack, &path, &edits);
                if job.fill_missing() {
                    self.status =
                        format!("Merged {name}, cropped: Fill edges needs the LaMa model");
                }
            }
            PanoEvent::Failed(why) => self.status = format!("Could not merge {name}: {why}"),
            _ => self.status = format!("Merging {name} was cancelled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::RustroomApp;
    use eframe::egui;
    use photo_merge::synth_pano;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// ⚠ A ceiling against a hang, never a budget (AGENTS.md).
    const LONG: Duration = Duration::from_secs(120);

    /// A constructed row of M9.10 DNGs (`synth_pano`) in its own folder.
    fn row(tag: &str, scenes: &[u64]) -> (PathBuf, Vec<PathBuf>) {
        let dir = test_dir::path(&format!("pano-flow-{tag}"));
        let mut r = synth_pano::row(scenes.len(), 0.35, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        let files = synth_pano::write_dng_row(&dir, &r, scenes);
        (dir, files)
    }

    fn open_all(app: &mut RustroomApp, ctx: &egui::Context, dir: PathBuf) {
        app.open_folder_at(ctx, dir);
        let lib = app.library.as_mut().expect("a library");
        let n = lib.entries.len();
        lib.selection.select_indices(&(0..n).collect::<Vec<_>>());
    }

    fn run_to_end(app: &mut RustroomApp, ctx: &egui::Context) {
        let start = Instant::now();
        while app.merges.busy() {
            app.pump_merges(ctx);
            assert!(start.elapsed() < LONG, "the merge never landed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Pumps the dialog until `done` holds.
    fn pump_until(
        app: &mut RustroomApp,
        ctx: &egui::Context,
        done: impl Fn(&crate::pano_flow::PanoDialog) -> bool,
    ) {
        let start = Instant::now();
        while !app.pano.dialog.as_ref().is_some_and(&done) {
            app.pump_pano(ctx);
            assert!(start.elapsed() < LONG, "the dialog never got there");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// ⚠⚠ The whole wiring, headless: a selection → the merge (last settings, no dialog) → the file
    /// catalogued, its sidecar seeded with Auto crop's rectangle and the lens off, stacked, selected.
    #[test]
    fn merging_a_dng_row_catalogues_seeds_the_crop_stacks_and_selects() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, files) = row("e2e", &[1, 1, 1]);
        open_all(&mut app, &ctx, dir.clone());
        app.settings.panorama.create_stack = true;
        app.open_pano_merge(true);
        assert!(
            matches!(app.modal, crate::modals::Modal::None),
            "last settings opens no dialog"
        );
        run_to_end(&mut app, &ctx);

        let out = dir.join("frame0-Pano.dng");
        assert!(
            out.exists() && raw_decode::is_float_dng(&out),
            "{}",
            app.status
        );
        assert!(app.status.contains("frame0-Pano.dng"), "{}", app.status);
        assert!(
            app.catalog.as_ref().unwrap().knows(&out).unwrap(),
            "catalogued"
        );
        let edits = rustroom_app::sidecar::load(&out).expect("a sidecar");
        assert_ne!(
            edits.geometry.crop,
            edit_model::CropRect::default(),
            "Auto crop"
        );
        assert_eq!(
            edits.lens_correction,
            Some(edit_model::LensCorrection::default()),
            "lens off"
        );
        let lib = app.library.as_ref().unwrap();
        assert!(
            rustroom_app::stack::is_top(&lib.hand_stacks, &out),
            "on top of its stack"
        );
        for f in &files {
            assert!(lib.hand_stacks.contains_key(f), "{} stacked", f.display());
        }
        let active = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.path.clone());
        assert_eq!(active.as_ref(), Some(&out), "the result is selected");
    }

    /// Review Focus 5: a cancelled panorama leaves nothing, and a merge queued after it still runs.
    #[test]
    fn a_cancelled_panorama_leaves_no_file_and_the_next_merge_runs() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("cancel", &[1, 1, 1]);
        open_all(&mut app, &ctx, dir.clone());
        app.open_pano_merge(true);
        app.pump_merges(&ctx);
        app.cancel_merges();
        run_to_end(&mut app, &ctx);
        assert!(!dir.join("frame0-Pano.dng").exists());
        let partial = std::fs::read_dir(&dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".partial")
        });
        assert!(!partial, "a partial file was left");
        assert!(
            app.status.to_lowercase().contains("cancel"),
            "{}",
            app.status
        );
        app.open_hdr_merge(true);
        run_to_end(&mut app, &ctx);
        assert!(
            dir.join("frame0-HDR.dng").exists(),
            "the next merge ran: {}",
            app.status
        );
    }

    /// Review Focus 5's other half (the review's #4): an HDR merge queued while a panorama runs waits, then
    /// runs — both land, the panorama first.
    #[test]
    fn an_hdr_queued_behind_a_running_panorama_runs_after_it() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("queue", &[1, 1, 1]);
        open_all(&mut app, &ctx, dir.clone());
        app.open_pano_merge(true);
        app.pump_merges(&ctx);
        app.open_hdr_merge(true);
        let mut landed: Vec<String> = Vec::new();
        let start = Instant::now();
        while app.merges.busy() {
            app.pump_merges(&ctx);
            if app.status.starts_with("Merged") && landed.last() != Some(&app.status) {
                landed.push(app.status.clone());
            }
            assert!(start.elapsed() < LONG, "the merges never landed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            dir.join("frame0-Pano.dng").exists() && dir.join("frame0-HDR.dng").exists(),
            "{}",
            app.status
        );
        assert_eq!(
            landed,
            vec![
                "Merged frame0-Pano.dng".to_string(),
                "Merged frame0-HDR.dng".to_string()
            ]
        );
    }

    /// Review Focus 2: Perspective on a sweep too wide for it — said, and Merge is not offered.
    #[test]
    fn a_too_wide_perspective_disables_merge() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("toowide", &[1, 1, 1, 1]);
        open_all(&mut app, &ctx, dir);
        app.settings.panorama.auto_projection = false;
        app.settings.panorama.projection = photo_merge::projection::Projection::Perspective;
        app.open_pano_merge(false);
        pump_until(&mut app, &ctx, |d| d.too_wide || d.failed.is_some());
        assert!(!app.pano.dialog.as_ref().unwrap().can_merge());
    }

    #[test]
    fn unconnected_frames_are_named_and_merge_is_disabled() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("unconnected", &[1, 1, 2]);
        open_all(&mut app, &ctx, dir);
        app.open_pano_merge(false);
        pump_until(&mut app, &ctx, |d| {
            d.unconnected.is_some() || d.failed.is_some()
        });
        let d = app.pano.dialog.as_ref().unwrap();
        assert_eq!(
            d.unconnected.as_deref(),
            Some(&["frame2.dng".to_string()][..])
        );
        assert!(!d.can_merge());
    }

    /// Review Focus 3, the app's half: Fill edges without LaMa offers Remove's download.
    #[test]
    fn fill_without_the_model_offers_the_download() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("nolama", &[1, 1]);
        open_all(&mut app, &ctx, dir);
        app.open_pano_merge(false);
        let empty = test_dir::path("pano-flow-no-models");
        std::fs::create_dir_all(&empty).unwrap();
        let d = app.pano.dialog.as_mut().unwrap();
        d.models = Some(empty);
        assert!(
            !super::needs_lama(&d.draft, d.models.as_deref()),
            "nothing asked yet"
        );
        d.draft.fill_edges = true;
        assert!(super::needs_lama(&d.draft, d.models.as_deref()));
        app.offer_lama_download();
        assert!(app.modal == crate::modals::Modal::AiDownload, "the offer");
        assert_eq!(app.ai.proposed, vec![rustroom_app::removal::MODEL]);
    }

    /// The dialog: its preview arrives, and Merge queues the merge with the options and the preview's rig.
    #[test]
    fn the_dialog_previews_and_merge_starts_the_job() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = row("dialog", &[1, 1, 1]);
        open_all(&mut app, &ctx, dir.clone());
        app.open_pano_merge(false);
        assert!(matches!(app.modal, crate::modals::Modal::PanoMerge));
        pump_until(&mut app, &ctx, |d| {
            d.preview.is_some() || d.failed.is_some()
        });
        assert!(app.pano.dialog.as_ref().unwrap().can_merge());
        app.pano.dialog.as_mut().unwrap().draft.auto_crop = false;
        app.start_pano_merge();
        assert!(app.pano.dialog.is_none() && matches!(app.modal, crate::modals::Modal::None));
        assert!(!app.settings.panorama.auto_crop, "remembered on Merge");
        run_to_end(&mut app, &ctx);
        let out = dir.join("frame0-Pano.dng");
        assert!(out.exists(), "{}", app.status);
        let edits = rustroom_app::sidecar::load(&out).expect("a sidecar");
        assert_eq!(
            edits.geometry.crop,
            edit_model::CropRect::default(),
            "Auto crop off: no crop"
        );
    }
}

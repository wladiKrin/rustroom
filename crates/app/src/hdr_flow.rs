//! Photo → Photo merge → HDR… (M10.17a) in the app: the dialog's state, the queue of merges, and
//! what happens when one lands.
//!
//! The decisions are the lib's (`rustroom_app::hdr_merge`): which files, the name, the worker. This
//! is the glue — the selection in, the preview's texture out, and on landing the order the import
//! taught: the sidecar written BEFORE the folder is scanned (no thumbnail renders from a photo without
//! its edits), then the catalog, the Library row, the stack and the selection.

use crate::modals::Modal;
use crate::RustroomApp;
use edit_model::Edits;
use eframe::egui;
use photo_merge::homography::Homography;
use rustroom_app::hdr_merge::worker::{HdrEvent, HdrWorker};
use rustroom_app::hdr_merge::{self, HdrPlan, HdrSettings};
use rustroom_app::photo_id::PhotoId;
use std::path::Path;
use std::time::Duration;

/// The dialog while it is open.
pub(crate) struct HdrDialog {
    pub(crate) plan: HdrPlan,
    /// The options as set in the dialog; written to `Settings` only on Merge.
    pub(crate) draft: HdrSettings,
    /// What the preview worker was last given — a change to anything it renders sends the draft.
    sent: HdrSettings,
    worker: HdrWorker,
    /// The preview, and the same tinted where deghosting took one frame alone.
    pub(crate) preview: Option<(egui::TextureHandle, Option<egui::TextureHandle>)>,
    /// The preview is for options other than the draft's: drawn dimmed until the next lands.
    pub(crate) stale: bool,
    /// The half-size alignment the merge starts from.
    transforms: Option<Vec<Homography>>,
    pub(crate) failed: Option<String>,
    /// The preview's zoom — 1 is fit, as on the canvas — and its pan in screen points. Kept across
    /// option changes, so a new merge lands under the same view.
    pub(crate) zoom: f32,
    pub(crate) pan: egui::Vec2,
    /// What a new photo starts with — the Auto settings go on top.
    base: Edits,
}

/// An HDR merge waiting its turn in [`crate::merge_queue`].
pub(crate) struct HdrJob {
    plan: HdrPlan,
    settings: HdrSettings,
    base: Edits,
    init: Option<Vec<Homography>>,
}

impl HdrJob {
    /// Its worker, started.
    pub(crate) fn start(&self) -> HdrWorker {
        HdrWorker::merge(
            self.plan.clone(),
            self.settings,
            self.base.clone(),
            self.init.clone(),
        )
    }
}

/// The HDR dialog's state; the merges themselves queue in [`crate::merge_queue`].
#[derive(Default)]
pub(crate) struct HdrState {
    pub(crate) dialog: Option<HdrDialog>,
}

/// The options the preview renders from — the overlay and the stack change nothing it draws.
fn renders_differently(a: &HdrSettings, b: &HdrSettings) -> bool {
    (a.auto_align, a.auto_settings, a.deghost) != (b.auto_align, b.auto_settings, b.deghost)
}

/// ⚠ Mipmapped: the preview is the half-size merge at its own size (~3100 px), drawn far smaller at
/// fit, and plain linear minification of that shimmers.
fn texture(ctx: &egui::Context, name: &str, img: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw()),
        egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        },
    )
}

impl RustroomApp {
    /// Photo → Photo merge → HDR… on the selection: the dialog, or with `last` the merge itself
    /// with the options last used. A selection that cannot be merged says why in the status row.
    pub(crate) fn open_hdr_merge(&mut self, last: bool) {
        let ids = self.selection_ids();
        self.open_hdr_merge_for(&ids, last);
    }

    /// [`Self::open_hdr_merge`] on these photos — the right-click's targets.
    pub(crate) fn open_hdr_merge_for(&mut self, ids: &[PhotoId], last: bool) {
        let Some(lib) = &self.library else {
            return;
        };
        // Grid order, the active photo remembered: it wins a tie for the reference.
        let active = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.path.clone());
        let row = |id: &PhotoId| {
            lib.entries
                .iter()
                .position(|e| e.id == *id)
                .unwrap_or(usize::MAX)
        };
        let mut ids = ids.to_vec();
        ids.sort_by_key(row);
        let photos = rustroom_app::batch_rename::photos_of(&self.files_of(&ids));
        let active = active.and_then(|a| photos.iter().position(|files| files.contains(&a)));
        let root = self.immich_cache_root();
        let cat = self.catalog.as_ref();
        let plan = hdr_merge::plan(
            &photos,
            active,
            |p| {
                root.as_deref()
                    .is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, p))
            },
            // ⚠ Only a DNG can be a merged HDR, and sniffing reads the whole file — never a RAF's.
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
                self.status = r.sentence().to_string();
                return;
            }
        };
        let first = plan.frames[0].clone();
        if !self.place_immich_merge(&mut plan.output, &first, "HDR") {
            return;
        }
        let base = self.settings.initial_edits(None);
        let settings = self.settings.hdr;
        if last {
            self.queue_hdr(HdrJob {
                plan,
                settings,
                base,
                init: None,
            });
            return;
        }
        self.hdr.dialog = Some(HdrDialog {
            worker: HdrWorker::preview(&plan, settings, base.clone()),
            plan,
            draft: settings,
            sent: settings,
            preview: None,
            stale: false,
            transforms: None,
            failed: None,
            zoom: rustroom_app::view_region::MIN_ZOOM,
            pan: egui::Vec2::ZERO,
            base,
        });
        self.modal = Modal::HdrMerge;
    }

    /// The dialog's Merge: the options remembered, the merge queued, the dialog closed.
    pub(crate) fn start_hdr_merge(&mut self) {
        let Some(d) = self.hdr.dialog.take() else {
            return;
        };
        self.modal = Modal::None;
        // ⚠ Remembered here and only here — `start_import`'s rule, so Cancel leaves them alone.
        if self.settings.hdr != d.draft {
            self.settings.hdr = d.draft;
            self.save_settings();
        }
        self.queue_hdr(HdrJob {
            plan: d.plan,
            settings: d.draft,
            base: d.base,
            // ⚠ The preview's alignment only when it was made with Auto align as the merge has it.
            init: d
                .transforms
                .filter(|_| d.sent.auto_align == d.draft.auto_align),
        });
    }

    /// The dialog's Cancel: the preview worker stops when it is dropped.
    pub(crate) fn close_hdr_merge(&mut self) {
        self.hdr.dialog = None;
        self.modal = Modal::None;
    }

    fn queue_hdr(&mut self, job: HdrJob) {
        let name = job
            .plan
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = hdr_merge::progress_line(
            &photo_merge::hdr::Progress::Decoding {
                done: 0,
                total: job.plan.frames.len(),
            },
            &name,
        );
        self.queue_merge(crate::merge_queue::Job::Hdr(job), line);
    }

    /// Once per frame, beside the other jobs: the dialog's preview.
    pub(crate) fn pump_hdr(&mut self, ctx: &egui::Context) {
        if let Some(d) = self.hdr.dialog.as_mut() {
            if renders_differently(&d.draft, &d.sent) {
                d.worker.set_options(d.draft);
                d.sent = d.draft;
                d.stale = d.preview.is_some();
            }
            while let Ok(e) = d.worker.events().try_recv() {
                match e {
                    HdrEvent::Preview {
                        image,
                        tinted,
                        transforms,
                    } => {
                        d.preview = Some((
                            texture(ctx, "hdr-preview", &image),
                            tinted.map(|t| texture(ctx, "hdr-preview-tinted", &t)),
                        ));
                        d.transforms = Some(transforms);
                        d.stale = false;
                        d.failed = None;
                    }
                    HdrEvent::Failed(e) => d.failed = Some(e),
                    _ => {}
                }
            }
            if d.preview.is_none() || d.stale {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// The running HDR merge ended with `e` (Done, Failed or Cancelled).
    pub(crate) fn land_hdr(&mut self, job: &HdrJob, e: HdrEvent) {
        let name = job
            .plan
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match e {
            HdrEvent::Done { path, edits } => self.finish_hdr(job, &path, &edits),
            HdrEvent::Failed(why) => self.status = format!("Could not merge {name}: {why}"),
            _ => self.status = format!("Merging {name} was cancelled"),
        }
    }

    /// A merge landed: its edits, the catalog, the Library row, the stack, the selection.
    fn finish_hdr(&mut self, job: &HdrJob, path: &Path, edits: &Edits) {
        self.land_merged(&job.plan.frames, job.settings.create_stack, path, edits);
    }
}

#[cfg(test)]
mod tests {
    use crate::RustroomApp;
    use eframe::egui;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// ⚠ A ceiling against a hang, never a budget (CLAUDE.md).
    const LONG: Duration = Duration::from_secs(120);

    /// A bracket of M9.10 DNGs — libraw reads them as raws — at −1 / 0 / +1, in its own folder.
    /// ⚠ Constructed. 300 px wide: M9.10's DNG narrower than one tile reads back scrambled.
    fn bracket(tag: &str) -> (PathBuf, Vec<PathBuf>) {
        let dir = test_dir::path(&format!("hdr-flow-{tag}"));
        let (w, h) = (300, 200);
        let files = [-1.0f32, 0.0, 1.0]
            .iter()
            .enumerate()
            .map(|(i, ev)| {
                let mut img = core_image::ImageBuf::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (u, v) = (x as f32, y as f32);
                        let l = 0.02
                            * 2f32.powf(7.0 * u / w as f32)
                            * (1.0 + 0.4 * (u * 0.3).sin() * (v * 0.23).sin());
                        img.set_pixel(x, y, [l * 0.9, l, l * 1.1].map(|c| c * 2f32.powf(*ev)));
                    }
                }
                let p = dir.join(format!("B{i}.dng"));
                export::dng::save(&img, &p).unwrap();
                p
            })
            .collect();
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

    /// ⚠⚠ The whole wiring, headless: a selection → the merge (last settings, no dialog) → the file
    /// catalogued, its sidecar seeded with Auto, stacked on top of its sources, selected.
    #[test]
    fn merging_three_dngs_catalogues_seeds_stacks_and_selects_the_result() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, files) = bracket("e2e");
        open_all(&mut app, &ctx, dir.clone());
        app.settings.hdr.create_stack = true;
        app.open_hdr_merge(true);
        assert!(
            matches!(app.modal, crate::modals::Modal::None),
            "last settings opens no dialog"
        );
        run_to_end(&mut app, &ctx);

        let out = dir.join("B0-HDR.dng");
        assert!(
            out.exists() && raw_decode::is_float_dng(&out),
            "{}",
            app.status
        );
        assert!(app.status.contains("B0-HDR.dng"), "{}", app.status);
        let cat = app.catalog.as_ref().unwrap();
        assert!(cat.knows(&out).unwrap(), "catalogued");
        let edits = rustroom_app::sidecar::load(&out).expect("a sidecar");
        assert_ne!(edits.exposure, 0.0, "Auto settings");
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

    /// In Develop the open photo keeps the filmstrip's highlight: a merge landing in the background
    /// must not move the selection off the photo being edited (the Library selects the result).
    #[test]
    fn a_merge_landing_in_develop_keeps_the_selection() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, files) = bracket("develop");
        open_all(&mut app, &ctx, dir.clone());
        app.open_hdr_merge(true);
        app.library.as_mut().unwrap().selection.select_indices(&[1]);
        app.view = crate::AppView::Develop;
        run_to_end(&mut app, &ctx);
        assert!(dir.join("B0-HDR.dng").exists(), "{}", app.status);
        let lib = app.library.as_ref().unwrap();
        let active = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.path.clone());
        assert_eq!(
            active.as_ref(),
            Some(&files[1]),
            "the selection moved under Develop"
        );
        assert!(
            lib.entries
                .iter()
                .any(|e| e.id.path == dir.join("B0-HDR.dng")),
            "the row is there"
        );
    }

    #[test]
    fn a_cancelled_merge_leaves_no_file() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = bracket("cancel");
        open_all(&mut app, &ctx, dir.clone());
        app.open_hdr_merge(true);
        app.pump_merges(&ctx);
        app.cancel_merges();
        run_to_end(&mut app, &ctx);
        assert!(!dir.join("B0-HDR.dng").exists());
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
    }

    #[test]
    fn a_refused_selection_says_why_in_the_status_row() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = bracket("refused");
        app.open_folder_at(&ctx, dir);
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        app.open_hdr_merge(false);
        assert_eq!(
            app.status,
            rustroom_app::hdr_merge::Refusal::TooFew.sentence()
        );
        assert!(matches!(app.modal, crate::modals::Modal::None));
        assert!(!app.merges.busy());
    }

    /// The dialog: its preview arrives as a texture, and Merge queues the merge with the options.
    #[test]
    fn the_dialog_previews_and_merge_starts_the_job() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (dir, _) = bracket("dialog");
        open_all(&mut app, &ctx, dir.clone());
        app.open_hdr_merge(false);
        assert!(matches!(app.modal, crate::modals::Modal::HdrMerge));
        let start = Instant::now();
        while app.hdr.dialog.as_ref().is_some_and(|d| d.preview.is_none()) {
            app.pump_hdr(&ctx);
            assert!(start.elapsed() < LONG, "no preview");
            std::thread::sleep(Duration::from_millis(10));
        }
        app.hdr.dialog.as_mut().unwrap().draft.deghost = photo_merge::deghost::Deghost::Low;
        app.start_hdr_merge();
        assert!(app.hdr.dialog.is_none() && matches!(app.modal, crate::modals::Modal::None));
        assert_eq!(
            app.settings.hdr.deghost,
            photo_merge::deghost::Deghost::Low,
            "remembered on Merge"
        );
        run_to_end(&mut app, &ctx);
        assert!(dir.join("B0-HDR.dng").exists(), "{}", app.status);
    }
}

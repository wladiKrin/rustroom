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
use std::collections::VecDeque;
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
    /// What a new photo starts with — the Auto settings go on top.
    base: Edits,
}

/// A merge waiting its turn.
struct Job {
    plan: HdrPlan,
    settings: HdrSettings,
    base: Edits,
    init: Option<Vec<Homography>>,
}

/// The HDR merge's whole state: the dialog, the queue, the running merge.
#[derive(Default)]
pub(crate) struct HdrState {
    pub(crate) dialog: Option<HdrDialog>,
    queue: VecDeque<Job>,
    running: Option<(HdrWorker, Job)>,
    /// The status row's line while a merge runs.
    pub(crate) progress: String,
}

impl HdrState {
    /// A merge is running or waiting.
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }
}

/// The options the preview renders from — the overlay and the stack change nothing it draws.
fn renders_differently(a: &HdrSettings, b: &HdrSettings) -> bool {
    (a.auto_align, a.auto_settings, a.deghost) != (b.auto_align, b.auto_settings, b.deghost)
}

fn texture(ctx: &egui::Context, name: &str, img: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw()),
        egui::TextureOptions::LINEAR,
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
        let plan = match plan {
            Ok(p) => p,
            Err(r) => {
                self.status = r.sentence().to_string();
                return;
            }
        };
        let base = self.settings.initial_edits(None);
        let settings = self.settings.hdr;
        if last {
            self.queue_hdr(Job {
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
        self.queue_hdr(Job {
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

    fn queue_hdr(&mut self, job: Job) {
        let name = job
            .plan
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.hdr.progress = hdr_merge::progress_line(
            &photo_merge::hdr::Progress::Decoding {
                done: 0,
                total: job.plan.frames.len(),
            },
            &name,
        );
        self.hdr.queue.push_back(job);
    }

    /// The status row's Cancel: the running merge and every waiting one.
    pub(crate) fn cancel_hdr(&mut self) {
        self.hdr.queue.clear();
        if let Some((w, _)) = &self.hdr.running {
            w.cancel();
        } else {
            self.hdr.progress.clear();
            self.status = "HDR merge cancelled".to_string();
        }
    }

    /// Once per frame, beside the other jobs: the dialog's preview, the next merge, the running one.
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
        if self.hdr.running.is_none() {
            if let Some(job) = self.hdr.queue.pop_front() {
                let w = HdrWorker::merge(
                    job.plan.clone(),
                    job.settings,
                    job.base.clone(),
                    job.init.clone(),
                );
                self.hdr.running = Some((w, job));
            }
        }
        let mut landed = None;
        if let Some((w, _)) = &self.hdr.running {
            while let Ok(e) = w.events().try_recv() {
                match e {
                    HdrEvent::Progress(line) => self.hdr.progress = line,
                    HdrEvent::Preview { .. } => {}
                    other => landed = Some(other),
                }
            }
        }
        if let Some(e) = landed {
            let (_, job) = self.hdr.running.take().expect("running");
            let name = job
                .plan
                .output
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match e {
                HdrEvent::Done { path, edits } => self.finish_hdr(&job, &path, &edits),
                HdrEvent::Failed(why) => self.status = format!("Could not merge {name}: {why}"),
                _ => self.status = format!("Merging {name} was cancelled"),
            }
            if self.hdr.queue.is_empty() {
                self.hdr.progress.clear();
            }
        }
        if self.hdr.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// A merge landed: its edits, the catalog, the Library row, the stack, the selection.
    fn finish_hdr(&mut self, job: &Job, path: &Path, edits: &Edits) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(e) = rustroom_app::sidecar::persist(path, edits) {
            eprintln!("hdr: the sidecar: {e}");
        }
        let Some(dir) = path.parent() else { return };
        let scanned = self.scan_and_relink(dir);
        let id = PhotoId::original(path.to_path_buf());
        // ⚠ Only a Library showing the bracket gains the row — a catalog-backed view follows the
        // catalog by itself (`source_refresh`).
        let shows = self
            .library
            .as_ref()
            .is_some_and(|l| l.scan.iter().any(|e| job.plan.frames.contains(&e.path)));
        if shows {
            let entry = scanned.and_then(|(scan, _)| scan.into_iter().find(|e| e.path == path));
            if let (Some(lib), Some(entry)) = (self.library.as_mut(), entry) {
                if !lib.scan.iter().any(|e| e.path == path) {
                    lib.scan.push(entry);
                }
            }
            // ⚠ The Library selects the result (Lightroom's way); Develop keeps the selection, or the
            // filmstrip would highlight a photo the canvas is not showing.
            let select = (self.view == crate::AppView::Library).then_some(&id);
            self.rebuild_entries(select);
            if job.settings.create_stack {
                let mut ids = vec![id.clone()];
                ids.extend(job.plan.frames.iter().cloned().map(PhotoId::original));
                self.stack_ids(&ids);
            }
        }
        self.status = format!("Merged {name}");
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
        while app.hdr.busy() {
            app.pump_hdr(ctx);
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
        app.pump_hdr(&ctx);
        app.cancel_hdr();
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
        assert!(!app.hdr.busy());
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

//! The background merges (M10.17a, M10.17b): HDR and panorama merges queue here and run one at a time,
//! in order, the running one's line and Cancel in the status row.
//!
//! Each kind's decisions stay in its own flow (`hdr_flow`): what a job is, and what happens when it lands.

use crate::hdr_flow::HdrJob;
use crate::pano_flow::PanoJob;
use crate::RustroomApp;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::hdr_merge::worker::{HdrEvent, HdrWorker};
use rustroom_app::pano_merge::worker::{PanoEvent, PanoWorker};
use rustroom_app::photo_id::PhotoId;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

/// A merge waiting its turn.
pub(crate) enum Job {
    Hdr(HdrJob),
    Panorama(PanoJob),
}

/// The merge running now, with its worker.
enum Running {
    Hdr(HdrWorker, HdrJob),
    Panorama(PanoWorker, PanoJob),
}

/// What a merge whose thread died says.
const DIED: &str = "the merge stopped unexpectedly";

/// How the running merge ended.
enum Landed {
    Hdr(HdrEvent),
    Panorama(PanoEvent),
}

/// The queue, the running merge, and the status row's line.
#[derive(Default)]
pub(crate) struct MergeQueue {
    queue: VecDeque<Job>,
    running: Option<Running>,
    /// The status row's line while a merge runs or waits.
    pub(crate) progress: String,
}

impl MergeQueue {
    /// A merge is running or waiting.
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }
}

impl RustroomApp {
    /// A merge whose first frame is a downloaded Immich photo: `output` moved to the folder remembered
    /// for such merges, asked for once (`hdr_merge::out_folder`). `false` when the picker was cancelled
    /// or pointed back into the Immich cache — nothing merges then.
    pub(crate) fn place_immich_merge(
        &mut self,
        output: &mut PathBuf,
        first: &Path,
        suffix: &str,
    ) -> bool {
        use rustroom_app::hdr_merge::{out_folder, output_in, OutFolder};
        use rustroom_app::immich_source::is_immich_path;
        let root = self.immich_cache_root();
        let in_cache = |p: &Path| root.as_deref().is_some_and(|r| is_immich_path(r, p));
        let folder = match out_folder(
            in_cache(first),
            self.settings.immich_merge_folder.as_deref(),
            Path::is_dir,
        ) {
            OutFolder::Beside => return true,
            OutFolder::In(f) => f,
            OutFolder::Ask => {
                let Some(f) = rfd::FileDialog::new()
                    .set_title("Choose a folder for photos merged from Immich")
                    .pick_folder()
                else {
                    self.status = "Merge cancelled".to_string();
                    return false;
                };
                if in_cache(&f) {
                    self.status = "Choose a folder outside the Immich downloads".to_string();
                    return false;
                }
                self.settings.immich_merge_folder = Some(f.clone());
                self.save_settings();
                f
            }
        };
        // The photo's own name — the cache's file is named by its asset id.
        let shown = self
            .library
            .as_ref()
            .and_then(|l| l.scan.iter().find(|e| e.path == first))
            .map(|e| e.file_name.clone())
            .or_else(|| first.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let cat = self.catalog.as_ref();
        *output = output_in(&folder, &shown, suffix, |p| {
            crate::move_flow::path_taken(cat, p)
        });
        true
    }

    /// A merged photo landed at `path`, made from `sources`: its sidecar written BEFORE the folder is
    /// scanned (no thumbnail renders from a photo without its edits — the import's order), then the
    /// catalog, the Library row, the stack when asked, the selection, the status line.
    pub(crate) fn land_merged(
        &mut self,
        sources: &[PathBuf],
        stack: bool,
        path: &Path,
        edits: &Edits,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(e) = rustroom_app::sidecar::persist(path, edits) {
            eprintln!("merge: the sidecar: {e}");
        }
        let Some(dir) = path.parent() else { return };
        let scanned = self.scan_and_relink(dir);
        let id = PhotoId::original(path.to_path_buf());
        // ⚠ Only a Library showing the sources gains the row — a catalog-backed view follows the
        // catalog by itself (`source_refresh`).
        // ⚠ A merge from Immich lands in another folder (`place_immich_merge`) — not a row of this view.
        let beside = sources.first().and_then(|s| s.parent()) == Some(dir);
        let shows = beside
            && self
                .library
                .as_ref()
                .is_some_and(|l| l.scan.iter().any(|e| sources.contains(&e.path)));
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
            if stack {
                let mut ids = vec![id.clone()];
                ids.extend(sources.iter().cloned().map(PhotoId::original));
                self.stack_ids(&ids);
            }
        }
        self.status = if beside {
            format!("Merged {name}")
        } else {
            format!("Merged {name} into {}", dir.display())
        };
    }

    /// `job` to the back of the queue; `line` is the status row's until it starts.
    pub(crate) fn queue_merge(&mut self, job: Job, line: String) {
        self.merges.progress = line;
        self.merges.queue.push_back(job);
    }

    /// The status row's Cancel: the running merge and every waiting one.
    pub(crate) fn cancel_merges(&mut self) {
        self.merges.queue.clear();
        match &self.merges.running {
            Some(Running::Hdr(w, _)) => w.cancel(),
            Some(Running::Panorama(w, _)) => w.cancel(),
            None => {
                self.merges.progress.clear();
                self.status = "Merge cancelled".to_string();
            }
        }
    }

    /// Once per frame, beside the other jobs: the next merge started, the running one's events read.
    pub(crate) fn pump_merges(&mut self, ctx: &egui::Context) {
        if self.merges.running.is_none() {
            if let Some(job) = self.merges.queue.pop_front() {
                self.merges.running = Some(match job {
                    Job::Hdr(j) => Running::Hdr(j.start(), j),
                    Job::Panorama(j) => Running::Panorama(j.start(), j),
                });
            }
        }
        let landed = match &self.merges.running {
            Some(Running::Hdr(w, _)) => {
                let mut landed = None;
                loop {
                    match w.events().try_recv() {
                        Ok(HdrEvent::Progress(line)) => self.merges.progress = line,
                        Ok(HdrEvent::Preview { .. }) => {}
                        Ok(other) => landed = Some(Landed::Hdr(other)),
                        // ⚠ The thread is gone without a last word (a panic): a failure, or the queue
                        // would wait behind it for the rest of the session.
                        Err(TryRecvError::Disconnected) => {
                            landed = landed.or(Some(Landed::Hdr(HdrEvent::Failed(DIED.into()))));
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                landed
            }
            Some(Running::Panorama(w, _)) => {
                let mut landed = None;
                loop {
                    match w.events().try_recv() {
                        Ok(PanoEvent::Progress(line)) => self.merges.progress = line,
                        Ok(
                            e @ (PanoEvent::Done { .. }
                            | PanoEvent::Failed(_)
                            | PanoEvent::Cancelled),
                        ) => landed = Some(Landed::Panorama(e)),
                        Ok(_) => {}
                        Err(TryRecvError::Disconnected) => {
                            landed =
                                landed.or(Some(Landed::Panorama(PanoEvent::Failed(DIED.into()))));
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                landed
            }
            None => None,
        };
        if let Some(e) = landed {
            match (self.merges.running.take(), e) {
                (Some(Running::Hdr(_, job)), Landed::Hdr(e)) => self.land_hdr(&job, e),
                (Some(Running::Panorama(_, job)), Landed::Panorama(e)) => self.land_pano(&job, e),
                _ => unreachable!("a landing from another merge"),
            }
            if self.merges.queue.is_empty() {
                self.merges.progress.clear();
            }
        }
        if self.merges.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::pano_flow::PanoJob;
    use eframe::egui;
    use rustroom_app::pano_merge::worker::PanoWorker;
    use rustroom_app::pano_merge::PanoPlan;

    /// The review's #3: a merge thread that dies without a last word (a panic) must not hold the queue for
    /// the rest of the session — it lands as a failure, and the next merge can start.
    #[test]
    fn a_merge_thread_that_died_frees_the_queue() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let plan = PanoPlan {
            frames: vec![],
            output: std::path::PathBuf::from("/nowhere/x-Pano.dng"),
        };
        app.merges.running = Some(super::Running::Panorama(
            PanoWorker::ended(),
            PanoJob::for_test(plan),
        ));
        app.merges.progress = "Merging x-Pano.dng · stitching".into();
        app.pump_merges(&ctx);
        assert!(!app.merges.busy(), "the queue is still held");
        assert!(app.merges.progress.is_empty());
        assert!(
            app.status.contains("stopped unexpectedly"),
            "{}",
            app.status
        );
    }
}

#[cfg(test)]
mod immich_tests {
    use std::path::PathBuf;

    /// Asked for in use (2026-10-28): a merge of downloaded Immich photos lands in the remembered folder,
    /// named after the photo, never in the cache Clear… empties; a local merge stays beside its frames.
    #[test]
    fn a_merge_from_immich_goes_to_the_remembered_folder_under_the_photos_name() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        let root = app.immich_cache_root().unwrap();
        let dir = test_dir::TempDir::new("immich-merge-folder");
        app.settings.immich_merge_folder = Some(dir.path().to_path_buf());
        let first = root.join("a846b6b0-33cb-45e9-b85a-2dab3bc23483.RAF");
        let mut lib = crate::test_app::library_of_names(&["DSCF1234.RAF"]);
        lib.scan[0].path = first.clone();
        app.library = Some(lib);

        let mut out = root.join("a846b6b0-33cb-45e9-b85a-2dab3bc23483-Pano.dng");
        assert!(app.place_immich_merge(&mut out, &first, "Pano"));
        assert_eq!(out, dir.join("DSCF1234-Pano.dng"));

        let local = PathBuf::from("/photos/DSCF1.RAF");
        let mut out = PathBuf::from("/photos/DSCF1-Pano.dng");
        assert!(app.place_immich_merge(&mut out, &local, "Pano"));
        assert_eq!(
            out,
            PathBuf::from("/photos/DSCF1-Pano.dng"),
            "beside its frames"
        );
    }

    #[test]
    fn a_merge_landing_in_another_folder_says_where() {
        let mut app = crate::test_app::for_test();
        let dir = test_dir::TempDir::new("immich-merge-land");
        let out = dir.join("DSCF1234-Pano.dng");
        std::fs::write(&out, b"").unwrap();
        app.land_merged(
            &[PathBuf::from("/cache/a.RAF"), PathBuf::from("/cache/b.RAF")],
            true,
            &out,
            &edit_model::Edits::default(),
        );
        assert_eq!(
            app.status,
            format!("Merged DSCF1234-Pano.dng into {}", dir.path().display())
        );
    }
}

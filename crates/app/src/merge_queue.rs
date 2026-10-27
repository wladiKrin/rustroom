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
        let shows = self
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
        self.status = format!("Merged {name}");
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
                while let Ok(e) = w.events().try_recv() {
                    match e {
                        HdrEvent::Progress(line) => self.merges.progress = line,
                        HdrEvent::Preview { .. } => {}
                        other => landed = Some(Landed::Hdr(other)),
                    }
                }
                landed
            }
            Some(Running::Panorama(w, _)) => {
                let mut landed = None;
                while let Ok(e) = w.events().try_recv() {
                    match e {
                        PanoEvent::Progress(line) => self.merges.progress = line,
                        PanoEvent::Done { .. } | PanoEvent::Failed(_) | PanoEvent::Cancelled => {
                            landed = Some(Landed::Panorama(e))
                        }
                        _ => {}
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

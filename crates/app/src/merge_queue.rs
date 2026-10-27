//! The background merges (M10.17a, M10.17b): HDR and panorama merges queue here and run one at a time,
//! in order, the running one's line and Cancel in the status row.
//!
//! Each kind's decisions stay in its own flow (`hdr_flow`): what a job is, and what happens when it lands.

use crate::hdr_flow::HdrJob;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::hdr_merge::worker::{HdrEvent, HdrWorker};
use std::collections::VecDeque;
use std::time::Duration;

/// A merge waiting its turn.
pub(crate) enum Job {
    Hdr(HdrJob),
}

/// The merge running now, with its worker.
enum Running {
    Hdr(HdrWorker, HdrJob),
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
            None => {
                self.merges.progress.clear();
                self.status = "HDR merge cancelled".to_string();
            }
        }
    }

    /// Once per frame, beside the other jobs: the next merge started, the running one's events read.
    pub(crate) fn pump_merges(&mut self, ctx: &egui::Context) {
        if self.merges.running.is_none() {
            if let Some(job) = self.merges.queue.pop_front() {
                self.merges.running = Some(match job {
                    Job::Hdr(j) => Running::Hdr(j.start(), j),
                });
            }
        }
        let mut landed = None;
        if let Some(Running::Hdr(w, _)) = &self.merges.running {
            while let Ok(e) = w.events().try_recv() {
                match e {
                    HdrEvent::Progress(line) => self.merges.progress = line,
                    HdrEvent::Preview { .. } => {}
                    other => landed = Some(other),
                }
            }
        }
        if let Some(e) = landed {
            let Some(Running::Hdr(_, job)) = self.merges.running.take() else {
                unreachable!("a landing without a running merge")
            };
            self.land_hdr(&job, e);
            if self.merges.queue.is_empty() {
                self.merges.progress.clear();
            }
        }
        if self.merges.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

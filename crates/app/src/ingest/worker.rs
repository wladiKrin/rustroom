//! The import's background thread: surveying the card (capture times and thumbnails) and copying.
//!
//! ⚠ `batch_export::worker`'s shape — a `std::thread`, an mpsc channel of events, an `AtomicBool`
//! cancel checked between files. Two separate spawns rather than one worker with two jobs: the
//! survey is dropped whenever the dialog closes or the source changes, the copy runs to the end.
//!
//! ⚠ This only touches the filesystem. Every destination is planned on the UI thread before the
//! spawn (`plan::destination`), so the worker never decides a name; the catalog, XMP and edits are
//! applied there too (`entry`), where the app's catalog lives.

use core_image::{Capture, Thumbnail};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What the worker reports, in order.
#[derive(Debug)]
pub enum ImportEvent {
    /// One source file's capture facts and embedded thumbnail; either may be missing.
    Surveyed {
        index: usize,
        capture: Option<Capture>,
        thumb: Option<Thumbnail>,
    },
    SurveyDone,
    /// About to copy item `done + 1` of `total`.
    Progress {
        done: usize,
        total: usize,
        name: String,
    },
    /// Copied to `dest`. A backup failure is reported here and does not stop the import.
    Copied {
        index: usize,
        dest: PathBuf,
        capture: Capture,
        backup_error: Option<String>,
    },
    Failed {
        index: usize,
        error: String,
    },
    Finished {
        copied: usize,
        failed: usize,
        cancelled: bool,
    },
}

/// One file to copy, its destination planned already.
#[derive(Debug, Clone)]
pub struct CopyItem {
    /// The caller's index, echoed in the events.
    pub index: usize,
    pub src: PathBuf,
    pub dest: PathBuf,
    pub backup: Option<PathBuf>,
    /// From the survey when it got there; read from the copied file otherwise.
    pub capture: Option<Capture>,
}

pub struct ImportWorker {
    events: Receiver<ImportEvent>,
    cancel: Arc<AtomicBool>,
}

impl ImportWorker {
    /// Read each file's capture and thumbnail, in order.
    pub fn survey(files: Vec<PathBuf>) -> Self {
        Self::spawn(move |tx, cancel| {
            for (index, path) in files.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let (capture, thumb) = survey_one(&path);
                if tx
                    .send(ImportEvent::Surveyed {
                        index,
                        capture,
                        thumb,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = tx.send(ImportEvent::SurveyDone);
        })
    }

    /// Copy every item to its destination, and to its backup when it has one.
    pub fn import(items: Vec<CopyItem>) -> Self {
        Self::start_import(items, false)
    }

    /// `import` with the cancel flag's starting value — `true` only from the tests, which is the
    /// one way to reach the cancelled path without racing the thread.
    fn start_import(items: Vec<CopyItem>, cancelled_from_start: bool) -> Self {
        Self::spawn_with(cancelled_from_start, move |tx, cancel| {
            let total = items.len();
            let (mut copied, mut failed, mut cancelled) = (0, 0, false);
            for (done, item) in items.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                let name = item
                    .src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let _ = tx.send(ImportEvent::Progress { done, total, name });
                match super::copy::copy_verified(&item.src, &item.dest) {
                    Ok(()) => {
                        let backup_error = item.backup.as_ref().and_then(|b| {
                            super::copy::copy_verified(&item.src, b)
                                .err()
                                .map(|e| e.to_string())
                        });
                        // ⚠ Read from the COPY when the survey did not get there: it is on the
                        // local disk now, not the card.
                        let capture = item
                            .capture
                            .unwrap_or_else(|| read_capture(&item.dest).unwrap_or_default());
                        copied += 1;
                        let _ = tx.send(ImportEvent::Copied {
                            index: item.index,
                            dest: item.dest,
                            capture,
                            backup_error,
                        });
                    }
                    Err(e) => {
                        failed += 1;
                        let _ = tx.send(ImportEvent::Failed {
                            index: item.index,
                            error: e.to_string(),
                        });
                    }
                }
            }
            let _ = tx.send(ImportEvent::Finished {
                copied,
                failed,
                cancelled,
            });
        })
    }

    fn spawn(work: impl FnOnce(Sender<ImportEvent>, Arc<AtomicBool>) + Send + 'static) -> Self {
        Self::spawn_with(false, work)
    }

    fn spawn_with(
        cancelled: bool,
        work: impl FnOnce(Sender<ImportEvent>, Arc<AtomicBool>) + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(cancelled));
        let flag = cancel.clone();
        std::thread::spawn(move || work(tx, flag));
        Self { events: rx, cancel }
    }

    pub fn events(&self) -> &Receiver<ImportEvent> {
        &self.events
    }

    /// Stop after the file in hand. A copy in progress completes — or fails without a partial.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for ImportWorker {
    /// A dropped worker — the dialog closed, the source changed — stops at the next file rather
    /// than reading the rest of a card nobody is looking at.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// A file's capture and thumbnail for the grid.
///
/// ⚠⚠ **The EXIF block first, the whole file only when that fails.** The first version read every
/// RAW whole through libraw — "really slow" over hundreds of files, the author's report. Measured on
/// 200 X-T4 RAFs and 200 JPEGs: the fast path answered **every one**, its capture time agreed with
/// the full read **200/200**, and it cost **0.5 ms and ≤70 KB** a file against **113 ms / 31 MB**
/// (RAF) and **299 ms / 12 MB** (JPEG) — `examples/import_survey_probe.rs`. A format it does not
/// read (TIFF-based raws, PNG) falls through to the old path, so nothing is ever lost.
fn survey_one(path: &std::path::Path) -> (Option<Capture>, Option<Thumbnail>) {
    if let Ok(Some(q)) = image_decode::quick_look(path) {
        if q.thumb.is_some() {
            return (Some(q.capture), q.thumb);
        }
    }
    if catalog::is_raw(path) {
        raw_decode::survey(path).unwrap_or((None, None))
    } else {
        (
            image_decode::read_capture(path).ok(),
            image_decode::thumbnail(path).ok(),
        )
    }
}

fn read_capture(path: &std::path::Path) -> Option<Capture> {
    if let Ok(Some(q)) = image_decode::quick_look(path) {
        return Some(q.capture);
    }
    if catalog::is_raw(path) {
        raw_decode::read_capture(path).ok()
    } else {
        image_decode::read_capture(path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::temp_dir;
    use std::time::Duration;

    /// Every event until `Finished` (or `SurveyDone`), with a timeout so a hung worker fails.
    fn drain(w: &ImportWorker) -> Vec<ImportEvent> {
        let mut out = Vec::new();
        loop {
            let e = w
                .events()
                .recv_timeout(Duration::from_secs(10))
                .expect("the worker went quiet");
            let end = matches!(e, ImportEvent::Finished { .. } | ImportEvent::SurveyDone);
            out.push(e);
            if end {
                return out;
            }
        }
    }

    fn card(d: &std::path::Path) -> (PathBuf, PathBuf) {
        let a = d.join("card/DCIM/100/A.JPG");
        let b = d.join("card/DCIM/100/B.JPG");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, b"aaaa").unwrap();
        std::fs::write(&b, b"bb").unwrap();
        (a, b)
    }

    fn item(
        index: usize,
        src: &std::path::Path,
        dest: PathBuf,
        backup: Option<PathBuf>,
    ) -> CopyItem {
        CopyItem {
            index,
            src: src.to_path_buf(),
            dest,
            backup,
            capture: Some(Capture::default()),
        }
    }

    #[test]
    fn two_files_land_in_the_destination_and_the_backup_in_order() {
        let d = temp_dir("worker");
        let (a, b) = card(&d);
        let w = ImportWorker::import(vec![
            item(0, &a, d.join("home/A.JPG"), Some(d.join("bak/A.JPG"))),
            item(1, &b, d.join("home/B.JPG"), Some(d.join("bak/B.JPG"))),
        ]);
        let ev = drain(&w);
        let kinds: Vec<&str> = ev
            .iter()
            .map(|e| match e {
                ImportEvent::Progress { .. } => "progress",
                ImportEvent::Copied { .. } => "copied",
                ImportEvent::Finished { .. } => "finished",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["progress", "copied", "progress", "copied", "finished"]
        );
        assert!(matches!(
            ev.last(),
            Some(ImportEvent::Finished {
                copied: 2,
                failed: 0,
                cancelled: false
            })
        ));
        for n in ["A.JPG", "B.JPG"] {
            assert!(
                d.join("home").join(n).exists() && d.join("bak").join(n).exists(),
                "{n}"
            );
        }
        assert_eq!(std::fs::read(d.join("bak/A.JPG")).unwrap(), b"aaaa");
    }

    #[test]
    fn a_failed_backup_is_reported_and_does_not_stop_the_import() {
        let d = temp_dir("bakfail");
        let (a, b) = card(&d);
        std::fs::write(d.join("bak"), b"a file, not a folder").unwrap();
        let w = ImportWorker::import(vec![
            item(0, &a, d.join("home/A.JPG"), Some(d.join("bak/A.JPG"))),
            item(1, &b, d.join("home/B.JPG"), Some(d.join("bak/B.JPG"))),
        ]);
        let ev = drain(&w);
        let backup_errors = ev
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ImportEvent::Copied {
                        backup_error: Some(_),
                        ..
                    }
                )
            })
            .count();
        assert_eq!(backup_errors, 2);
        assert!(matches!(
            ev.last(),
            Some(ImportEvent::Finished {
                copied: 2,
                failed: 0,
                ..
            })
        ));
        assert!(d.join("home/B.JPG").exists());
    }

    #[test]
    fn a_missing_source_fails_alone() {
        let d = temp_dir("missing");
        let (a, _) = card(&d);
        let w = ImportWorker::import(vec![
            item(0, &d.join("card/gone.JPG"), d.join("home/gone.JPG"), None),
            item(1, &a, d.join("home/A.JPG"), None),
        ]);
        let ev = drain(&w);
        assert!(matches!(ev[1], ImportEvent::Failed { index: 0, .. }));
        assert!(matches!(
            ev.last(),
            Some(ImportEvent::Finished {
                copied: 1,
                failed: 1,
                ..
            })
        ));
    }

    #[test]
    fn a_cancelled_import_copies_nothing() {
        let d = temp_dir("cancel");
        let (a, _) = card(&d);
        let w = ImportWorker::start_import(vec![item(0, &a, d.join("home/A.JPG"), None)], true);
        let ev = drain(&w);
        assert!(matches!(
            ev.as_slice(),
            [ImportEvent::Finished {
                copied: 0,
                cancelled: true,
                ..
            }]
        ));
        assert!(!d.join("home/A.JPG").exists());
    }

    #[test]
    fn a_survey_reports_every_file_even_one_it_cannot_read() {
        let d = temp_dir("survey");
        let (a, b) = card(&d);
        let w = ImportWorker::survey(vec![a, b]);
        let ev = drain(&w);
        // Not real JPEGs, so capture and thumbnail are absent — but each file is still reported.
        let indices: Vec<usize> = ev
            .iter()
            .filter_map(|e| match e {
                ImportEvent::Surveyed {
                    index, thumb: None, ..
                } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, [0, 1]);
        assert!(matches!(ev.last(), Some(ImportEvent::SurveyDone)));
    }
}

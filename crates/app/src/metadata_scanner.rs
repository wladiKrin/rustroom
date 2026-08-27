//! A background pass that reads every photo's capture metadata into the catalog.
//!
//! ⚠⚠ **Its own pass, NOT piggybacked on `ThumbLoader`** — even though that already reads every
//! file — because thumbnails are **windowed** since M4.3a-i. Metadata attached to them would exist
//! only for photos that had been scrolled past, and *you cannot filter by a date that was never
//! read*. Accepted cost: a file is read twice on a first visit.
//!
//! ⚠ Measured: ~11–15 ms per RAW, almost all of it *reading the file* — libraw's parse is 0.2 ms.
//! About 10 s for a folder of 830, once, in the background. A truncated prefix read was measured
//! and rejected: libraw opens nothing at 1 MB and only half the files at 4 MB, so it would
//! silently miss part of a library.

use core_image::Capture;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// One file's capture metadata, read.
pub struct MetadataResult {
    pub path: PathBuf,
    pub capture: Capture,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<PathBuf>,
    /// Everything ever queued, so re-queuing a folder mid-pass reads nothing twice.
    queued: HashSet<PathBuf>,
    stop: bool,
}

/// Reads capture metadata off the UI thread.
pub struct MetadataScanner {
    rx: Receiver<MetadataResult>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    _handles: Vec<JoinHandle<()>>,
}

/// Read one file's capture metadata, dispatching on its type.
///
/// ⚠ A failure yields an **empty** `Capture` rather than being dropped: an unreadable file must
/// still be marked as read, or it is retried on every folder open forever.
fn read_capture(path: &Path) -> Capture {
    if catalog::is_raw(path) {
        raw_decode::read_capture(path).unwrap_or_default()
    } else {
        image_decode::read_capture(path).unwrap_or_default()
    }
}

impl MetadataScanner {
    /// `on_progress` is called after each file (e.g. `ctx.request_repaint`).
    pub fn new(on_progress: impl Fn() + Send + Sync + 'static) -> Self {
        Self::with_reader(read_capture, rustroom_app_worker_count(), on_progress)
    }

    /// The injectable form: `read` and the thread count are parameters so the whole loop is
    /// unit-testable with no image fixtures, the way `ThumbLoader` takes its loader fn.
    pub fn with_reader(
        read: fn(&Path) -> Capture,
        threads: usize,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel::<MetadataResult>();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let on_progress = Arc::new(on_progress);

        let handles = (0..threads.max(1))
            .map(|_| {
                let queue = queue.clone();
                let tx = tx.clone();
                let on_progress = on_progress.clone();
                thread::spawn(move || {
                    let (lock, cv) = &*queue;
                    loop {
                        let path = loop {
                            let mut q = lock.lock().unwrap();
                            if q.stop {
                                return;
                            }
                            if let Some(p) = q.pending.pop_front() {
                                break p;
                            }
                            drop(cv.wait(q).unwrap());
                        };
                        let capture = read(&path);
                        if tx.send(MetadataResult { path, capture }).is_err() {
                            return;
                        }
                        on_progress();
                    }
                })
            })
            .collect();
        drop(tx);

        Self {
            rx,
            queue,
            _handles: handles,
        }
    }

    /// Queue these paths for reading.
    ///
    /// ⚠⚠ **Additive, where `ThumbLoader::want` *replaces*, and that difference is the point.**
    /// The thumbnail queue is "what is on screen now", so replacing it *is* the cancellation. This
    /// queue is "what still needs reading", and dropping it on a source switch would leave folders
    /// permanently half-scanned.
    ///
    /// ⚠ De-duplicated against everything ever queued, so re-opening a folder mid-pass costs
    /// nothing.
    pub fn scan(&self, paths: &[PathBuf]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        for p in paths {
            if q.queued.insert(p.clone()) {
                q.pending.push_back(p.clone());
            }
        }
        drop(q);
        cv.notify_all();
    }

    /// Take everything read since the last call. ⚠ Results arrive out of order.
    pub fn drain(&self) -> Vec<MetadataResult> {
        self.rx.try_iter().collect()
    }
}

impl Drop for MetadataScanner {
    /// ⚠ Required because workers park. A worker asleep on the condvar never reaches `send`, so
    /// dropping the receiver alone would leave it sleeping for the life of the process.
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().stop = true;
        cv.notify_all();
    }
}

/// The same headroom rule the thumbnail loader follows — this pass runs while photos are being
/// edited, so it must not saturate the machine.
fn rustroom_app_worker_count() -> usize {
    crate::thumb_loader::worker_count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/nonexistent/meta_{i:04}.raf")))
            .collect()
    }

    /// Collect until `want` results have arrived or the deadline passes.
    fn collect(s: &MetadataScanner, want: usize) -> Vec<MetadataResult> {
        let mut out = Vec::new();
        for _ in 0..400 {
            out.extend(s.drain());
            if out.len() >= want {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    fn iso(_p: &Path) -> Capture {
        Capture {
            iso: Some(100),
            ..Default::default()
        }
    }

    #[test]
    fn every_queued_path_is_read_exactly_once() {
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Capture {
            READS.fetch_add(1, Ordering::SeqCst);
            Capture::default()
        }
        READS.store(0, Ordering::SeqCst);
        let s = MetadataScanner::with_reader(counted, 4, || {});
        s.scan(&paths(50));
        let got = collect(&s, 50);
        assert_eq!(got.len(), 50);
        assert_eq!(READS.load(Ordering::SeqCst), 50, "a file was read twice");
        let names: Vec<PathBuf> = got.iter().map(|r| r.path.clone()).collect();
        assert_eq!(names, paths(50));
    }

    #[test]
    fn a_second_scan_adds_to_the_queue_rather_than_replacing_it() {
        // ⚠⚠ The difference from `ThumbLoader::want`, which replaces. Replacing here would leave a
        // folder permanently half-scanned whenever the source was switched mid-pass.
        let s = MetadataScanner::with_reader(iso, 1, || {});
        s.scan(&paths(3));
        s.scan(&[PathBuf::from("/nonexistent/extra.raf")]);
        let got = collect(&s, 4);
        assert_eq!(got.len(), 4, "the first batch was dropped");
        assert!(got.iter().any(|r| r.path.ends_with("extra.raf")));
    }

    #[test]
    fn re_queuing_the_same_path_reads_it_once() {
        // ⚠ Re-opening a folder mid-pass must not double the work.
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Capture {
            READS.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(5));
            Capture::default()
        }
        READS.store(0, Ordering::SeqCst);
        let s = MetadataScanner::with_reader(counted, 1, || {});
        let p = paths(4);
        s.scan(&p);
        s.scan(&p);
        assert_eq!(collect(&s, 4).len(), 4);
        thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(READS.load(Ordering::SeqCst), 4, "paths were read twice");
    }

    #[test]
    fn a_file_with_nothing_readable_still_produces_a_result() {
        // ⚠ An unreadable file must be marked as read, or it is retried on every folder open —
        // 11–15 ms of file reading each time, forever, for nothing.
        let s = MetadataScanner::with_reader(|_| Capture::default(), 2, || {});
        s.scan(&paths(2));
        let got = collect(&s, 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|r| r.capture.is_empty()));
    }

    #[test]
    fn dropping_the_scanner_wakes_parked_workers() {
        // ⚠⚠ The same trap `ThumbLoader` hit — a test counting *reads started* cannot see a
        // missing `Drop`, because a worker parked on the condvar starts none either way. Each
        // worker holds a clone of the queue `Arc`, so the strong count **is** the live-worker
        // count rather than a proxy for it.
        let s = MetadataScanner::with_reader(|_| Capture::default(), 3, || {});
        let queue = s.queue.clone();
        assert_eq!(
            Arc::strong_count(&queue),
            5,
            "scanner + this clone + 3 workers"
        );
        thread::sleep(std::time::Duration::from_millis(20));
        drop(s);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while Arc::strong_count(&queue) > 1 && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            Arc::strong_count(&queue),
            1,
            "parked workers never woke and are leaked for the life of the process"
        );
    }
}

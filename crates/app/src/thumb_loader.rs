//! Background workers that extract embedded thumbnails off the UI thread and stream them
//! back keyed by their index in the folder listing.

use core_image::Thumbnail;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// One finished thumbnail (or `None` if that file had no usable preview).
pub struct ThumbResult {
    pub index: usize,
    pub thumb: Option<Thumbnail>,
}

/// How many threads share a folder.
///
/// ⚠ Deliberately **below** the core count. The develop pipeline's stages are rayon-parallel
/// over the *global* pool, and a decode can be running too, so a loader that saturated every
/// core would make editing sluggish for as long as a folder takes to load. Same reasoning
/// that keeps `batch_export` sequential and `DecodeWorker` at 2 — with the difference that
/// thumbnails are short and numerous, so some parallelism is clearly worth it: a folder of
/// 822 entries at ~90 ms each is ~74 s on one thread, which is how long a photo at the end
/// of a folder used to sit grey.
pub fn worker_count() -> usize {
    thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).clamp(1, 6))
        .unwrap_or(1)
}

/// Extract one thumbnail, dispatching on the file type.
fn load_thumbnail(path: &Path) -> Option<Thumbnail> {
    if catalog::is_raw(path) {
        raw_decode::extract_thumbnail(path).ok()
    } else {
        image_decode::thumbnail(path).ok()
    }
}

/// Owns the worker threads that load a fixed list of paths.
pub struct ThumbLoader {
    rx: Receiver<ThumbResult>,
    _handles: Vec<JoinHandle<()>>,
}

impl ThumbLoader {
    /// `on_progress` is called after each thumbnail (e.g. `ctx.request_repaint`).
    pub fn new(paths: Vec<PathBuf>, on_progress: impl Fn() + Send + Sync + 'static) -> Self {
        Self::with_loader(paths, load_thumbnail, worker_count(), on_progress)
    }

    /// The injectable form: `load` and the thread count are parameters so the whole loop is
    /// unit-testable with no image fixtures, the way `DecodeWorker` takes its decode fn.
    ///
    /// ⚠ Work is handed out through a **shared cursor**, not split into per-thread chunks.
    /// A folder mixes RAW and JPEG, whose extraction costs differ by a lot, so static
    /// chunking would leave threads idle while one ground through the expensive half.
    pub fn with_loader(
        paths: Vec<PathBuf>,
        load: fn(&Path) -> Option<Thumbnail>,
        threads: usize,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel::<ThumbResult>();
        let paths = Arc::new(paths);
        let next = Arc::new(AtomicUsize::new(0));
        let on_progress = Arc::new(on_progress);

        let n = threads.clamp(1, paths.len().max(1));
        let handles = (0..n)
            .map(|_| {
                let paths = paths.clone();
                let next = next.clone();
                let tx = tx.clone();
                let on_progress = on_progress.clone();
                thread::spawn(move || loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else { return };
                    let thumb = load(path);
                    // An error means the UI dropped the loader — stop rather than finish a
                    // folder nobody is looking at any more.
                    if tx.send(ThumbResult { index, thumb }).is_err() {
                        return;
                    }
                    on_progress();
                })
            })
            .collect();
        // The original sender must go, or `rx` would never see the workers finish.
        drop(tx);

        Self {
            rx,
            _handles: handles,
        }
    }

    /// Take all thumbnails finished since the last call.
    ///
    /// ⚠ Results arrive **out of order** now that several threads share the folder. Every
    /// one carries its `index`, and the caller stores by that index, so ordering never
    /// mattered — but do not reintroduce an assumption that it is sorted.
    pub fn drain(&self) -> Vec<ThumbResult> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect every result, giving the workers time to finish.
    fn collect(loader: &ThumbLoader, want: usize) -> Vec<ThumbResult> {
        let mut results = Vec::new();
        for _ in 0..400 {
            results.extend(loader.drain());
            if results.len() >= want {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        results.sort_by_key(|r| r.index);
        results
    }

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/nonexistent/rustroom_{i}.raf")))
            .collect()
    }

    #[test]
    fn streams_results_keyed_by_index_even_on_failure() {
        // Non-existent files fail extraction -> thumb is None, but a result still streams.
        let loader = ThumbLoader::new(paths(2), || {});
        let results = collect(&loader, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].index, 0);
        assert_eq!(results[1].index, 1);
        assert!(results[0].thumb.is_none() && results[1].thumb.is_none());
    }

    #[test]
    fn every_path_is_loaded_exactly_once() {
        // The shared cursor must hand out each index to exactly one worker — an off-by-one
        // in `fetch_add` would either duplicate or skip, and both would be invisible in the
        // UI until a thumbnail was silently missing.
        let loader = ThumbLoader::with_loader(paths(50), |_| None, 4, || {});
        let results = collect(&loader, 50);
        assert_eq!(results.len(), 50, "wrong number of results");
        let indices: Vec<usize> = results.iter().map(|r| r.index).collect();
        assert_eq!(indices, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn the_folder_is_loaded_in_parallel() {
        // ⚠ The point of the whole change. Measured by how many loads are in flight at once,
        // not by wall-clock time, so it does not go flaky on a loaded machine.
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        fn slow(_p: &Path) -> Option<Thumbnail> {
            let live = LIVE.fetch_add(1, Ordering::SeqCst) + 1;
            PEAK.fetch_max(live, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(20));
            LIVE.fetch_sub(1, Ordering::SeqCst);
            None
        }
        // The thread count is injected rather than taken from `worker_count()`, so this is
        // deterministic on any machine, including a single-core CI box.
        let loader = ThumbLoader::with_loader(paths(16), slow, 4, || {});
        assert_eq!(collect(&loader, 16).len(), 16);
        assert_eq!(
            PEAK.load(Ordering::SeqCst),
            4,
            "loads never overlapped — the folder is still being read one file at a time"
        );
    }

    #[test]
    fn a_dropped_loader_stops_its_workers() {
        // Switching folders must not leave the old one grinding through hundreds of files.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(10));
            None
        }
        let loader = ThumbLoader::with_loader(paths(500), counted, 2, || {});
        thread::sleep(std::time::Duration::from_millis(30));
        drop(loader);
        let at_drop = STARTED.load(Ordering::SeqCst);
        thread::sleep(std::time::Duration::from_millis(120));
        let after = STARTED.load(Ordering::SeqCst);
        // Each worker may finish the file already in its hands, so allow one more apiece.
        assert!(
            after <= at_drop + 2,
            "workers kept going after the loader was dropped: {at_drop} -> {after}"
        );
    }

    #[test]
    fn more_threads_than_files_is_harmless() {
        let loader = ThumbLoader::with_loader(paths(2), |_| None, 32, || {});
        assert_eq!(collect(&loader, 2).len(), 2);
        // And an empty folder must not spawn a thread that indexes past the end.
        let empty = ThumbLoader::with_loader(Vec::new(), |_| None, 4, || {});
        assert!(collect(&empty, 0).is_empty());
    }

    #[test]
    fn the_worker_count_leaves_headroom_for_rendering() {
        let n = worker_count();
        assert!(n >= 1, "must always make progress");
        assert!(n <= 6, "must not saturate the machine: {n}");
        if let Ok(cores) = thread::available_parallelism() {
            assert!(
                n < cores.get().max(2),
                "left no core for the render pipeline"
            );
        }
    }
}

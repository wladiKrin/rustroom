//! Background workers that extract embedded thumbnails off the UI thread, loading whatever the
//! UI currently wants and streaming the results back keyed by path.

use core_image::Thumbnail;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// One finished thumbnail (or `None` if that file had no usable preview).
///
/// ⚠ Keyed by **path**, not by an index. Indices shift under a re-sort or a copy being created;
/// a path does not, and the cache this feeds is path-keyed for the same reason.
pub struct ThumbResult {
    pub path: PathBuf,
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

/// Convert a thumbnail's pixels into the display's primaries, leaving alpha alone.
///
/// ⚠ 8-bit → 8-bit here, unlike the canvas, which converts in float before quantizing. A
/// thumbnail is a camera JPEG with no float behind it, so there is no precision to preserve.
fn convert_thumb(thumb: &mut Thumbnail, space: color::OutputSpace) {
    if space == color::OutputSpace::Srgb {
        return;
    }
    for px in thumb.rgba.as_chunks_mut::<4>().0 {
        let out = space.convert_srgb8([px[0], px[1], px[2]]);
        px[..3].copy_from_slice(&out);
    }
}

#[derive(Default)]
struct Queue {
    wanted: VecDeque<PathBuf>,
    in_flight: HashSet<PathBuf>,
    stop: bool,
}

/// Worker threads that load whatever the UI currently wants.
pub struct ThumbLoader {
    rx: Receiver<ThumbResult>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    _handles: Vec<JoinHandle<()>>,
}

impl ThumbLoader {
    /// `on_progress` is called after each thumbnail (e.g. `ctx.request_repaint`).
    pub fn new(
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_loader(load_thumbnail, worker_count(), display_space, on_progress)
    }

    /// The injectable form: `load` and the thread count are parameters so the whole loop is
    /// unit-testable with no image fixtures, the way `DecodeWorker` takes its decode fn.
    ///
    /// ⚠ Work is handed out through a **shared queue the UI replaces wholesale**, not a cursor
    /// over a fixed list. A folder mixes RAW and JPEG, whose extraction costs differ by a lot, so
    /// static chunking would leave threads idle while one ground through the expensive half.
    pub fn with_loader(
        load: fn(&Path) -> Option<Thumbnail>,
        threads: usize,
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel::<ThumbResult>();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let on_progress = Arc::new(on_progress);

        let n = threads.max(1);
        let handles = (0..n)
            .map(|_| {
                let queue = queue.clone();
                let tx = tx.clone();
                let on_progress = on_progress.clone();
                thread::spawn(move || {
                    let (lock, cv) = &*queue;
                    loop {
                        // Park until there is something to do, or the loader is dropped.
                        let path = loop {
                            let mut q = lock.lock().unwrap();
                            if q.stop {
                                return;
                            }
                            if let Some(p) = q.wanted.pop_front() {
                                q.in_flight.insert(p.clone());
                                break p;
                            }
                            drop(cv.wait(q).unwrap());
                        };
                        let mut thumb = load(&path);
                        // ⚠ Converted here rather than at upload, so a thumbnail is stored the
                        // way it will be shown. Not optional polish: the filmstrip sits directly
                        // beside the canvas showing the same photo, so correcting only the canvas
                        // would leave a visibly more saturated strip next to a corrected image.
                        if let Some(t) = thumb.as_mut() {
                            convert_thumb(t, display_space);
                        }
                        lock.lock().unwrap().in_flight.remove(&path);
                        // An error means the UI dropped the loader — stop rather than finish a
                        // folder nobody is looking at any more.
                        if tx.send(ThumbResult { path, thumb }).is_err() {
                            return;
                        }
                        on_progress();
                    }
                })
            })
            .collect();
        // The original sender must go, or `rx` would never see the workers finish.
        drop(tx);

        Self {
            rx,
            queue,
            _handles: handles,
        }
    }

    /// Ask for exactly these paths, in this order, discarding whatever was wanted before.
    ///
    /// ⚠⚠ **Replacing the list is the cancellation, and no cancellation mechanism is needed.** A
    /// scrolled-past request is simply never popped — "abandon, don't abort", the same trade
    /// `DecodeWorker` and `batch_export` make, because the work has no safe interruption point.
    ///
    /// ⚠ Paths already in flight are skipped. Consecutive want-lists overlap almost entirely as
    /// the window moves, so without this one slow thumbnail is fetched once per window move.
    ///
    /// ⚠ The caller filters out what it already holds — the UI owns the cache, so the UI is the
    /// only thing that knows what is loaded. A loader keeping its own shadow copy of that would
    /// be two facts that can disagree, the failure `render_max_dim` and `Session::geometry_mode`
    /// were each fixed by deriving rather than storing alongside.
    pub fn want(&self, paths: &[PathBuf]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.wanted.clear();
        for p in paths {
            if !q.in_flight.contains(p) {
                q.wanted.push_back(p.clone());
            }
        }
        drop(q);
        cv.notify_all();
    }

    /// Take all thumbnails finished since the last call.
    ///
    /// ⚠ Results arrive **out of order**, since several threads share the queue. Every one
    /// carries its `path`, and the caller stores by that — but do not reintroduce an assumption
    /// that it is sorted.
    pub fn drain(&self) -> Vec<ThumbResult> {
        self.rx.try_iter().collect()
    }
}

impl Drop for ThumbLoader {
    /// ⚠ Required now that workers park. The old fixed-list loader stopped because `tx.send`
    /// failed once `rx` was dropped; a worker asleep on the condvar never reaches `send`, so it
    /// has to be told and woken.
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().stop = true;
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Collect results until `want` of them have arrived or the deadline passes.
    fn collect(loader: &ThumbLoader, want: usize) -> Vec<ThumbResult> {
        let mut results = Vec::new();
        for _ in 0..400 {
            results.extend(loader.drain());
            if results.len() >= want {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        results.sort_by(|a, b| a.path.cmp(&b.path));
        results
    }

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/nonexistent/rustroom_{i:04}.raf")))
            .collect()
    }

    #[test]
    fn streams_results_keyed_by_path_even_on_failure() {
        // Non-existent files fail extraction -> thumb is None, but a result still streams.
        let loader = ThumbLoader::new(color::OutputSpace::Srgb, || {});
        loader.want(&paths(2));
        let results = collect(&loader, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, paths(2)[0]);
        assert_eq!(results[1].path, paths(2)[1]);
        assert!(results[0].thumb.is_none() && results[1].thumb.is_none());
    }

    #[test]
    fn every_wanted_path_is_loaded_exactly_once() {
        let loader = ThumbLoader::with_loader(|_| None, 4, color::OutputSpace::Srgb, || {});
        loader.want(&paths(50));
        let results = collect(&loader, 50);
        assert_eq!(results.len(), 50, "wrong number of results");
        let got: Vec<PathBuf> = results.iter().map(|r| r.path.clone()).collect();
        assert_eq!(got, paths(50));
    }

    #[test]
    fn a_worker_wakes_after_an_empty_queue() {
        // ⚠ The structural change from the old fixed-list loader: workers must park on an empty
        // queue and wake when the window moves, rather than returning when the list runs out.
        // Without the condvar notify they sleep forever and the grid stays grey.
        let loader = ThumbLoader::with_loader(|_| None, 2, color::OutputSpace::Srgb, || {});
        thread::sleep(std::time::Duration::from_millis(30));
        assert!(loader.drain().is_empty(), "nothing was wanted yet");
        loader.want(&paths(3));
        assert_eq!(collect(&loader, 3).len(), 3);
    }

    #[test]
    fn a_replaced_want_list_abandons_what_is_no_longer_wanted() {
        // ⚠ Replacing the list *is* the cancellation — "abandon, don't abort", the trade
        // `DecodeWorker` and `batch_export` both already make. A scrolled-past request is simply
        // never popped.
        //
        // Two workers, so at most two of the abandoned batch can already be in hand.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        fn slow(_p: &Path) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(20));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader = ThumbLoader::with_loader(slow, 2, color::OutputSpace::Srgb, || {});
        loader.want(&paths(200));
        thread::sleep(std::time::Duration::from_millis(10));
        let keep: Vec<PathBuf> = vec![PathBuf::from("/nonexistent/kept_a.raf")];
        loader.want(&keep);
        // ⚠ Wait for *this* path, not merely for the first result to arrive. The paths already in
        // flight when the list was replaced finish first, so `collect(&loader, 1)` returns one of
        // those and the assertion below would be false about perfectly correct code.
        let mut results = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            results.extend(loader.drain());
            if results.iter().any(|r| r.path == keep[0]) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            results.iter().any(|r| r.path == keep[0]),
            "the newly wanted path never loaded"
        );
        // The abandoned batch must not grind on: only what was already in hand may finish.
        thread::sleep(std::time::Duration::from_millis(80));
        let started = STARTED.load(Ordering::SeqCst);
        assert!(
            started <= 5,
            "abandoned work kept going: {started} loads started"
        );
    }

    #[test]
    fn an_in_flight_path_is_not_requested_twice() {
        // ⚠ The want-list is rewritten every time the window moves, and consecutive lists overlap
        // almost entirely. Without an in-flight set, one slow thumbnail is fetched once per
        // window move.
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        static RELEASE: AtomicUsize = AtomicUsize::new(0);
        fn gated(_p: &Path) -> Option<Thumbnail> {
            STARTS.fetch_add(1, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while RELEASE.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            None
        }
        STARTS.store(0, Ordering::SeqCst);
        RELEASE.store(0, Ordering::SeqCst);
        let one = paths(1);
        let loader = ThumbLoader::with_loader(gated, 4, color::OutputSpace::Srgb, || {});
        for _ in 0..20 {
            loader.want(&one);
            thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            STARTS.load(Ordering::SeqCst),
            1,
            "the same path was fetched more than once while in flight"
        );
        RELEASE.store(1, Ordering::SeqCst);
        assert_eq!(collect(&loader, 1).len(), 1);
    }

    /// Workers in this test, and paths — one each, so every worker must participate.
    const PAR: usize = 4;

    #[test]
    fn the_folder_is_loaded_in_parallel() {
        // ⚠ Each load waits for its peers rather than sleeping a fixed time, so the peak is
        // determined by the loader's structure and not by how the OS happened to schedule four
        // threads. `batch_export`'s cancel test failed on Linux CI for the same reason.
        //
        // ⚠ `ARRIVED` only ever counts up, and that is what makes this a barrier. Releasing on
        // `LIVE` does not work: the last worker to arrive satisfies the condition, leaves, and
        // decrements it below the threshold again — the test still passes, just two seconds
        // slower, which is exactly how a broken barrier hides.
        static ARRIVED: AtomicUsize = AtomicUsize::new(0);
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        fn gated(_p: &Path) -> Option<Thumbnail> {
            ARRIVED.fetch_add(1, Ordering::SeqCst);
            let live = LIVE.fetch_add(1, Ordering::SeqCst) + 1;
            PEAK.fetch_max(live, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while ARRIVED.load(Ordering::SeqCst) < PAR && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            LIVE.fetch_sub(1, Ordering::SeqCst);
            None
        }
        let loader = ThumbLoader::with_loader(gated, PAR, color::OutputSpace::Srgb, || {});
        loader.want(&paths(PAR));
        assert_eq!(collect(&loader, PAR).len(), PAR);
        assert_eq!(
            PEAK.load(Ordering::SeqCst),
            PAR,
            "loads never overlapped — the folder is still being read one file at a time"
        );
    }

    #[test]
    fn a_dropped_loader_stops_its_workers() {
        // Switching folders must not leave the old one grinding through hundreds of files.
        //
        // ⚠ The old loader stopped because `tx.send` failed once `rx` was dropped. A parked
        // worker never reaches `send`, so `Drop` has to set the stop flag and wake them.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(10));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader = ThumbLoader::with_loader(counted, 2, color::OutputSpace::Srgb, || {});
        loader.want(&paths(500));
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
    fn dropping_the_loader_wakes_parked_workers() {
        // ⚠⚠ `a_dropped_loader_stops_its_workers` cannot see this, and finding that out is what
        // this test exists for. That one counts *loads started*; a worker parked on the condvar
        // starts none either way, so deleting the whole `Drop` impl leaves it green.
        //
        // ⚠ The leak is real: without the stop flag a parked worker is never notified, so it
        // never reaches the `tx.send` whose failure used to be the only exit. Every folder open
        // and every display-space change builds a new loader, so the threads accumulate for the
        // life of the process.
        //
        // Each worker holds a clone of the queue `Arc`, so the strong count *is* the number of
        // live workers — which is the thing being asserted, rather than a proxy for it.
        let loader = ThumbLoader::with_loader(|_| None, 3, color::OutputSpace::Srgb, || {});
        // Nothing is wanted, so every worker parks immediately.
        let queue = loader.queue.clone();
        assert_eq!(
            Arc::strong_count(&queue),
            5,
            "expected the loader, this clone and three workers"
        );
        thread::sleep(std::time::Duration::from_millis(20));
        drop(loader);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while Arc::strong_count(&queue) > 1 && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            Arc::strong_count(&queue),
            1,
            "parked workers never woke up and are leaked for the life of the process"
        );
    }

    #[test]
    fn more_threads_than_paths_is_harmless() {
        let loader = ThumbLoader::with_loader(|_| None, 32, color::OutputSpace::Srgb, || {});
        loader.want(&paths(2));
        assert_eq!(collect(&loader, 2).len(), 2);
        // And an empty want-list must not spawn work or panic.
        loader.want(&[]);
        thread::sleep(std::time::Duration::from_millis(20));
        assert!(loader.drain().is_empty());
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

    #[test]
    fn a_thumbnail_is_converted_into_the_display_space() {
        // ⚠ The direction matters, not merely that it changed: an inverted transform would also
        // change the pixels, and would oversaturate exactly as the uncorrected preview does.
        let mut t = Thumbnail {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        convert_thumb(&mut t, color::OutputSpace::DisplayP3);
        assert!(t.rgba[0] < 255, "P3 red should be below 255: {}", t.rgba[0]);
        assert!(
            t.rgba[1] > 0 && t.rgba[2] > 0,
            "sRGB red is less saturated than P3 red, so it needs some green and blue: {:?}",
            &t.rgba[..3]
        );
        assert_eq!(t.rgba[3], 255, "alpha must be left alone");
    }

    #[test]
    fn srgb_leaves_a_thumbnail_untouched() {
        // The default must cost nothing — including not walking the buffer at all.
        let original = vec![255, 0, 0, 255, 12, 200, 90, 128];
        let mut t = Thumbnail {
            width: 2,
            height: 1,
            rgba: original.clone(),
        };
        convert_thumb(&mut t, color::OutputSpace::Srgb);
        assert_eq!(t.rgba, original);
    }
}

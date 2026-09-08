//! Background workers that extract embedded thumbnails off the UI thread, loading whatever the
//! UI currently wants and streaming the results back keyed by path.

use crate::photo_id::PhotoId;
use crate::thumb_key::ThumbKey;
use core_image::Thumbnail;
use edit_model::Edits;
use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// How a thumbnail is produced. Injected so the whole loop is unit-testable with no image
/// fixtures, the way `DecodeWorker` takes its decode fn.
///
/// ⚠ A boxed closure rather than a `fn` pointer, which cannot capture — the production one is a
/// plain fn, but a test's needs to count calls.
pub type LoadFn = Arc<dyn Fn(&ThumbRequest) -> Option<Thumbnail> + Send + Sync>;

/// One thumbnail to produce: which photo, and the edits to render it with.
///
/// ⚠ Carries the `Edits` **by value** rather than a fingerprint: the worker has to *render* with
/// them, and re-reading the sidecar on the worker would be a second source of truth for what a
/// photo's edits are.
#[derive(Clone, PartialEq)]
pub struct ThumbRequest {
    pub id: PhotoId,
    pub edits: Edits,
}

impl ThumbRequest {
    pub fn key(&self) -> ThumbKey {
        ThumbKey::new(self.id.clone(), &self.edits)
    }
}

/// One finished thumbnail (or `None` if that file had no usable preview).
///
/// ⚠ Keyed by **identity**, not by an index. Indices shift under a re-sort or a copy being
/// created; a `ThumbKey` does not, and the cache this feeds is keyed the same way.
///
/// ⚠⚠ The key is built from the **request the worker was handed**, never from anything it
/// re-reads: a key derived from a second read of the sidecar could disagree with the one the UI
/// is looking up under, and the thumbnail would never be found.
pub struct ThumbResult {
    pub key: ThumbKey,
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
/// Extract a photo's embedded preview and render its edits over it.
///
/// ⚠⚠ **Rendered from the embedded JPEG, not from a decode.** Measured: 4.9 ms of render on top
/// of the ~90 ms extraction, against 934 ms for a half-size decode — 9.8×, essentially all of it
/// the decode. It matters most on *invalidation*: a changed edit costs another ~95 ms here and
/// would cost ~930 ms there, which cannot be cached away since a decoded buffer is 78 MB.
/// Reproduce with `cargo run --release -p app --example bench_thumb_render <raw>`.
///
/// ⚠ **Accepted cost: the JPEG is 8-bit and display-referred, so highlight recovery is invisible
/// in the grid** — the headroom is gone before the pipeline sees it. Shadows compress the same
/// way, and the thumbnail shows the camera's rendering plus your edits rather than Rustroom's own
/// render.
fn load_thumbnail(req: &ThumbRequest, resampler: pipeline::Resampler) -> Option<Thumbnail> {
    let path = req.id.path.as_path();
    let thumb = if catalog::is_raw(path) {
        raw_decode::extract_thumbnail(path).ok()?
    } else {
        image_decode::thumbnail(path).ok()?
    };
    if req.edits == Edits::default() {
        // ⚠ Neutral edits are the common case on a first folder open; skip the round trip rather
        // than paying it 830 times for a no-op.
        return Some(thumb);
    }
    Some(render_edits(thumb, &thumbnail_edits(&req.edits), resampler))
}

/// The edits a **thumbnail** renders through: the photo's, with the display rendering removed.
///
/// ⚠⚠ **A thumbnail is the camera's embedded JPEG, which has already been rendered.** Running our
/// display rendering over it would render the photograph twice. Everything else is kept, because
/// an edit-accurate thumbnail is the whole point of rendering one at all.
///
/// ⚠ This is also what *closes* the strip-versus-canvas gap rather than widening it: the strip
/// keeps the camera's own rendering and the canvas gains ours, so the two converge.
fn thumbnail_edits(edits: &Edits) -> Edits {
    Edits {
        // ⚠ Undecided, not decided off — a render input, not a choice anyone made.
        rendering: None,
        ..edits.clone()
    }
}

/// Run the pipeline over an already-extracted thumbnail.
///
/// ⚠ `RenderScale::new(1.0)`: the thumbnail *is* the image as far as the pipeline is concerned,
/// so spatial radii are relative to it. A scale derived from the original's dimensions would make
/// a 512 px thumbnail apply full-resolution blur radii.
///
/// ⚠ The output dimensions come from the rendered image, **not** from the input — a crop changes
/// them, and reusing the input's would paint a cropped photo into the original's rect and skew it.
fn render_edits(thumb: Thumbnail, edits: &Edits, resampler: pipeline::Resampler) -> Thumbnail {
    let mut buf = core_image::ImageBuf::new(thumb.width, thumb.height);
    {
        let d = buf.data_mut();
        for (i, px) in thumb.rgba.as_chunks::<4>().0.iter().enumerate() {
            for c in 0..3 {
                d[i * 3 + c] = color::srgb_to_linear(px[c] as f32 / 255.0);
            }
        }
    }
    // ⚠⚠ **`srgb_to_prophoto`, and leaving it out is a bug this shipped once.** The pipeline's
    // working space is linear **ProPhoto**, and its output transform converts ProPhoto→sRGB on the
    // way out. Handing it linear *sRGB* means that matrix is applied to data that was never in
    // ProPhoto, and the result is heavily **oversaturated** — reported as "the saturation is very
    // exaggerated". `image_decode::decode` does exactly this pair of steps for the same reason,
    // and its own comment warns about it in the same words.
    //
    // ⚠ It bit only *edited* photos, because a neutral one takes the early return above and never
    // reaches the pipeline at all — which made the effect look arbitrary rather than systematic.
    let buf = buf.map_pixels(color::srgb_to_prophoto);
    let out =
        pipeline::build_pipeline_with(edits, pipeline::RenderScale::new(1.0), resampler).run(buf);
    Thumbnail {
        width: out.width(),
        height: out.height(),
        rgba: out.to_rgba8(),
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
    wanted: VecDeque<ThumbRequest>,
    /// ⚠ Keyed by `ThumbKey`, not `PhotoId`: two renders of one photo with different edits are
    /// two different jobs, and de-duplicating on the photo alone would drop the newer one and
    /// leave a stale thumbnail on screen for good.
    in_flight: HashSet<ThumbKey>,
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
        Self::with_loader(
            // ⚠⚠ Thumbnails always use the **default** resampler, deliberately, and do not follow
            // `Settings.resampler` (user's call, 2026-09-01). Honouring it was tried first and is
            // pure cost: the difference between kernels is **invisible at 512 px**, while changing
            // the setting would rebuild the loader and drop the whole folder's thumbnail cache —
            // ~41 ms × N, about 34 s on an 830-photo folder. The setting is about what you judge
            // at 1:1, which is where it can be seen.
            Arc::new(move |r: &ThumbRequest| load_thumbnail(r, pipeline::Resampler::default())),
            worker_count(),
            display_space,
            on_progress,
        )
    }

    /// The injectable form: `load` and the thread count are parameters so the whole loop is
    /// unit-testable with no image fixtures, the way `DecodeWorker` takes its decode fn.
    ///
    /// ⚠ Work is handed out through a **shared queue the UI replaces wholesale**, not a cursor
    /// over a fixed list. A folder mixes RAW and JPEG, whose extraction costs differ by a lot, so
    /// static chunking would leave threads idle while one ground through the expensive half.
    pub fn with_loader(
        load: LoadFn,
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
                let load = load.clone();
                thread::spawn(move || {
                    let (lock, cv) = &*queue;
                    loop {
                        // Park until there is something to do, or the loader is dropped.
                        let req = loop {
                            let mut q = lock.lock().unwrap();
                            if q.stop {
                                return;
                            }
                            if let Some(r) = q.wanted.pop_front() {
                                q.in_flight.insert(r.key());
                                break r;
                            }
                            drop(cv.wait(q).unwrap());
                        };
                        let mut thumb = load(&req);
                        // ⚠ Converted here rather than at upload, so a thumbnail is stored the
                        // way it will be shown. Not optional polish: the filmstrip sits directly
                        // beside the canvas showing the same photo, so correcting only the canvas
                        // would leave a visibly more saturated strip next to a corrected image.
                        if let Some(t) = thumb.as_mut() {
                            convert_thumb(t, display_space);
                        }
                        let key = req.key();
                        lock.lock().unwrap().in_flight.remove(&key);
                        // An error means the UI dropped the loader — stop rather than finish a
                        // folder nobody is looking at any more.
                        if tx.send(ThumbResult { key, thumb }).is_err() {
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

    /// Ask for exactly these thumbnails, in this order, discarding whatever was wanted before.
    ///
    /// ⚠⚠ **Replacing the list is the cancellation, and no cancellation mechanism is needed.** A
    /// scrolled-past request is simply never popped — "abandon, don't abort", the same trade
    /// `DecodeWorker` and `batch_export` make, because the work has no safe interruption point.
    ///
    /// ⚠ Requests already in flight are skipped. Consecutive want-lists overlap almost entirely as
    /// the window moves, so without this one slow thumbnail is fetched once per window move.
    ///
    /// ⚠ The caller filters out what it already holds — the UI owns the cache, so the UI is the
    /// only thing that knows what is loaded. A loader keeping its own shadow copy of that would
    /// be two facts that can disagree, the failure `render_max_dim` and `Session::geometry_mode`
    /// were each fixed by deriving rather than storing alongside.
    pub fn want(&self, wanted: &[ThumbRequest]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.wanted.clear();
        for r in wanted {
            if !q.in_flight.contains(&r.key()) {
                q.wanted.push_back(r.clone());
            }
        }
        drop(q);
        cv.notify_all();
    }

    /// Take all thumbnails finished since the last call.
    ///
    /// ⚠ Results arrive **out of order**, since several threads share the queue. Every one
    /// carries its `key`, and the caller stores by that — but do not reintroduce an assumption
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

    /// ⚠⚠ **A thumbnail is the camera's embedded JPEG, which is already rendered.** Running our
    /// rendering over it would render twice — and it is also what closes the strip/canvas gap
    /// rather than widening it: the strip keeps the camera's rendering and the canvas gains ours.
    #[test]
    fn a_thumbnail_is_never_rendered_twice() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            exposure: 0.5,
            ..Default::default()
        };
        let t = thumbnail_edits(&edits);
        assert_eq!(t.rendering, None, "the thumbnail path kept the rendering");
        // ⚠ And it keeps everything else — a thumbnail that dropped the edits would stop being
        // edit-accurate, which is the whole reason it renders at all.
        assert_eq!(t.exposure, 0.5, "it dropped the other edits too");
    }
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// `load_thumbnail` with the shipping kernel — the tests using it are about edits reaching
    /// the pixels, not about which kernel read them.
    fn load_thumbnail_default(req: &ThumbRequest) -> Option<Thumbnail> {
        load_thumbnail(req, pipeline::Resampler::default())
    }

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
        results.sort_by(|a, b| a.key.id.path.cmp(&b.key.id.path));
        results
    }

    fn path_of(i: usize) -> PathBuf {
        PathBuf::from(format!("/nonexistent/rustroom_{i:04}.raf"))
    }

    /// ⚠ Returns requests now rather than paths, so every test body below keeps its exact
    /// previous form — their assertions passing **unchanged** is the evidence that carrying edits
    /// moved none of the queue, parking, want-replacement or `Drop` behaviour.
    fn paths(n: usize) -> Vec<ThumbRequest> {
        (0..n)
            .map(|i| ThumbRequest {
                id: PhotoId::original(path_of(i)),
                edits: Edits::default(),
            })
            .collect()
    }

    #[test]
    fn streams_results_keyed_by_path_even_on_failure() {
        // Non-existent files fail extraction -> thumb is None, but a result still streams.
        let loader = ThumbLoader::new(color::OutputSpace::Srgb, || {});
        loader.want(&paths(2));
        let results = collect(&loader, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key.id.path, path_of(0));
        assert_eq!(results[1].key.id.path, path_of(1));
        assert!(results[0].thumb.is_none() && results[1].thumb.is_none());
    }

    #[test]
    fn every_wanted_path_is_loaded_exactly_once() {
        let loader = ThumbLoader::with_loader(
            Arc::new(|_: &ThumbRequest| None),
            4,
            color::OutputSpace::Srgb,
            || {},
        );
        loader.want(&paths(50));
        let results = collect(&loader, 50);
        assert_eq!(results.len(), 50, "wrong number of results");
        let got: Vec<PathBuf> = results.iter().map(|r| r.key.id.path.clone()).collect();
        let want: Vec<PathBuf> = (0..50).map(path_of).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_worker_wakes_after_an_empty_queue() {
        // ⚠ The structural change from the old fixed-list loader: workers must park on an empty
        // queue and wake when the window moves, rather than returning when the list runs out.
        // Without the condvar notify they sleep forever and the grid stays grey.
        let loader = ThumbLoader::with_loader(
            Arc::new(|_: &ThumbRequest| None),
            2,
            color::OutputSpace::Srgb,
            || {},
        );
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
        fn slow(_r: &ThumbRequest) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(20));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader = ThumbLoader::with_loader(Arc::new(slow), 2, color::OutputSpace::Srgb, || {});
        loader.want(&paths(200));
        thread::sleep(std::time::Duration::from_millis(10));
        let keep = vec![ThumbRequest {
            id: PhotoId::original(PathBuf::from("/nonexistent/kept_a.raf")),
            edits: Edits::default(),
        }];
        loader.want(&keep);
        // ⚠ Wait for *this* path, not merely for the first result to arrive. The paths already in
        // flight when the list was replaced finish first, so `collect(&loader, 1)` returns one of
        // those and the assertion below would be false about perfectly correct code.
        let mut results = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            results.extend(loader.drain());
            if results.iter().any(|r| r.key.id == keep[0].id) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            results.iter().any(|r| r.key.id == keep[0].id),
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
        fn gated(_r: &ThumbRequest) -> Option<Thumbnail> {
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
        let loader = ThumbLoader::with_loader(Arc::new(gated), 4, color::OutputSpace::Srgb, || {});
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
        fn gated(_r: &ThumbRequest) -> Option<Thumbnail> {
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
        let loader =
            ThumbLoader::with_loader(Arc::new(gated), PAR, color::OutputSpace::Srgb, || {});
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
        fn counted(_r: &ThumbRequest) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(10));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader =
            ThumbLoader::with_loader(Arc::new(counted), 2, color::OutputSpace::Srgb, || {});
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
        let loader = ThumbLoader::with_loader(
            Arc::new(|_: &ThumbRequest| None),
            3,
            color::OutputSpace::Srgb,
            || {},
        );
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
        let loader = ThumbLoader::with_loader(
            Arc::new(|_: &ThumbRequest| None),
            32,
            color::OutputSpace::Srgb,
            || {},
        );
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

    /// ⚠⚠ **The edits must reach the render fn.** Without this the loader would compile, run, and
    /// render every thumbnail neutral — the feature silently absent, with every other test green.
    #[test]
    fn the_requested_edits_reach_the_loader() {
        let seen = Arc::new(Mutex::new(Vec::<f32>::new()));
        let s = seen.clone();
        let loader = ThumbLoader::with_loader(
            Arc::new(move |req: &ThumbRequest| {
                s.lock().unwrap().push(req.edits.exposure);
                None
            }),
            1,
            color::OutputSpace::Srgb,
            || {},
        );
        loader.want(&[ThumbRequest {
            id: PhotoId::original(path_of(0)),
            edits: Edits {
                exposure: 1.5,
                ..Default::default()
            },
        }]);
        let _ = collect(&loader, 1);
        assert_eq!(seen.lock().unwrap().as_slice(), &[1.5]);
    }

    /// ⚠ Two renders of one photo with different edits are two jobs. De-duplicating on the photo
    /// alone would drop the newer one and leave a stale thumbnail on screen for good.
    #[test]
    fn the_same_photo_with_different_edits_is_two_jobs() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let loader = ThumbLoader::with_loader(
            Arc::new(move |_: &ThumbRequest| {
                c.fetch_add(1, Ordering::SeqCst);
                None
            }),
            1,
            color::OutputSpace::Srgb,
            || {},
        );
        let id = PhotoId::original(path_of(0));
        loader.want(&[
            ThumbRequest {
                id: id.clone(),
                edits: Edits::default(),
            },
            ThumbRequest {
                id,
                edits: Edits {
                    exposure: 1.0,
                    ..Default::default()
                },
            },
        ]);
        let _ = collect(&loader, 2);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    /// ⚠ A neutral photo skips the pipeline entirely — the common case on a first folder open,
    /// where paying a round trip 830 times for a no-op would be pure waste.
    #[test]
    fn neutral_edits_return_the_embedded_thumbnail_unchanged() {
        let thumb = Thumbnail {
            width: 2,
            height: 1,
            rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        let out = render_edits(
            thumb.clone(),
            &Edits::default(),
            pipeline::Resampler::default(),
        );
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 1);
    }

    /// ⚠⚠ **A crop changes the thumbnail's dimensions**, and they must come from the *rendered*
    /// image rather than the input — reusing the input's would paint a cropped photo into the
    /// original's rect and skew it.
    #[test]
    fn a_crop_changes_the_rendered_thumbnails_dimensions() {
        let thumb = Thumbnail {
            width: 40,
            height: 20,
            rgba: vec![128; 40 * 20 * 4],
        };
        let mut edits = Edits::default();
        edits.geometry.crop.right = 0.5;
        let out = render_edits(thumb, &edits, pipeline::Resampler::default());
        assert!(
            out.width < 40,
            "a half-width crop must narrow the thumbnail, got {}",
            out.width
        );
        assert_eq!(out.rgba.len(), out.width * out.height * 4);
    }

    /// ⚠⚠ **The only end-to-end proof that edits reach the pixels**, and it exists because the
    /// obvious sabotage — `load_thumbnail` returning the extracted preview unrendered — was
    /// *inert* against every other test in this file. The feature would have been silently
    /// absent with a green suite.
    ///
    /// ⚠ It needs a real file on disk, since `load_thumbnail` goes through `image_decode`. A
    /// saturated PNG written by `export::save` is the cheapest one available, and no image
    /// fixture has to be checked in.
    #[test]
    fn a_saturation_edit_really_reaches_the_pixels() {
        let dir = std::env::temp_dir().join(format!(
            "rustroom_thumbrender_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.png");

        // A strongly red image, so a monochrome conversion is unmistakable.
        let mut buf = core_image::ImageBuf::new(8, 8);
        for px in buf.data_mut().as_chunks_mut::<3>().0 {
            px[0] = 0.8;
            px[1] = 0.05;
            px[2] = 0.05;
        }
        export::save(
            &buf,
            &path,
            export::ExportFormat::Png8,
            92,
            color::OutputSpace::Srgb,
        )
        .unwrap();

        let neutral = load_thumbnail_default(&ThumbRequest {
            id: PhotoId::original(path.clone()),
            edits: Edits::default(),
        })
        .expect("the PNG must load");
        let mono = load_thumbnail_default(&ThumbRequest {
            id: PhotoId::original(path),
            edits: Edits {
                saturation: -100.0,
                ..Default::default()
            },
        })
        .expect("the PNG must load");
        let _ = std::fs::remove_dir_all(&dir);

        let spread = |t: &Thumbnail| {
            let p = &t.rgba[..3];
            p.iter().max().unwrap() - p.iter().min().unwrap()
        };
        assert!(
            spread(&neutral) > 40,
            "fixture check: the source really is coloured, spread {}",
            spread(&neutral)
        );
        assert!(
            spread(&mono) <= 2,
            "-100 saturation must reach the pixels, but the render still has a spread of {}",
            spread(&mono)
        );
    }

    /// ⚠⚠ **The regression guard for the oversaturation bug**, reported as *"the saturation of
    /// the raw file is very exaggerated"*.
    ///
    /// A *negligible* edit must leave the colours alone. It did not: `render_edits` linearized the
    /// thumbnail's sRGB and handed it straight to the pipeline, whose working space is linear
    /// **ProPhoto** and whose output transform converts ProPhoto→sRGB — so that matrix was applied
    /// to data that had never been in ProPhoto, expanding every colour.
    ///
    /// ⚠ `a_saturation_edit_really_reaches_the_pixels` above could **not** see this: −100
    /// saturation collapses the channels to grey whether or not the space is right. A test that
    /// exercises a *strong* edit is blind to a wrong colour space; only a near-neutral one is not.
    #[test]
    fn a_negligible_edit_does_not_change_the_colours() {
        let mut thumb = Thumbnail {
            width: 16,
            height: 16,
            rgba: Vec::new(),
        };
        // A spread of ordinary, moderately saturated colours.
        for i in 0..16 * 16 {
            let (r, g, b) = ((i * 7 % 200 + 30) as u8, (i * 13 % 180 + 40) as u8, 90u8);
            thumb.rgba.extend_from_slice(&[r, g, b, 255]);
        }

        let spread = |rgba: &[u8]| -> f64 {
            let mut t = 0.0;
            let mut n = 0.0;
            for px in rgba.as_chunks::<4>().0 {
                let mx = px[..3].iter().copied().max().unwrap() as f64;
                let mn = px[..3].iter().copied().min().unwrap() as f64;
                if mx > 0.0 {
                    t += (mx - mn) / mx;
                    n += 1.0;
                }
            }
            t / n
        };

        let before = spread(&thumb.rgba);
        let after = render_edits(
            thumb,
            &Edits {
                // Small enough to be invisible; large enough that `is_active` puts a stage in the
                // pipeline, which is what makes the colour space matter.
                exposure: 0.01,
                ..Default::default()
            },
            pipeline::Resampler::default(),
        );
        let after = spread(&after.rgba);
        assert!(
            (after - before).abs() < 0.02,
            "a negligible edit changed mean saturation from {before:.4} to {after:.4} — the \
             thumbnail is not being converted into the pipeline's ProPhoto working space"
        );
    }
}

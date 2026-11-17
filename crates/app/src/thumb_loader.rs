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
/// Every distinct `.cube` this folder's thumbnails have asked for.
///
/// ⚠⚠ **SHARED across threads, because thumbnails render on `worker_count()` of them.**
/// `RenderEngine`'s worker caches one look in a plain local, which is right for a path that renders
/// one photograph at a time and wrong here.
///
/// ⚠⚠ **The value is an `Option`, so a FAILED load is cached as a miss.** Without that, a preset
/// naming a `.cube` this machine does not have would re-read the looks directory once per
/// thumbnail — 830 times on an 830-photo folder, for a file that is not there.
pub(crate) type LookCache =
    Mutex<std::collections::HashMap<String, Option<Arc<pipeline::lut::Cube>>>>;

/// The look a thumbnail should render through, loading it at most once per name.
fn look_for(
    edits: &Edits,
    store: Option<&crate::looks::LookStore>,
    cache: Option<&LookCache>,
) -> Option<Arc<pipeline::lut::Cube>> {
    // ⚠ `is_active()` for `build_pipeline_split_in_region_with_brush`'s reason: a look at zero
    // strength must not suppress anything, and a name with no strength is not a look.
    if !edits.look.is_active() {
        return None;
    }
    let file = edits.look.file.as_deref()?;
    let (store, cache) = (store?, cache?);
    let mut g = cache.lock().ok()?;
    g.entry(file.to_string())
        .or_insert_with(|| store.load(file).ok().map(Arc::new))
        .clone()
}

fn load_thumbnail(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    store: Option<&crate::thumb_store::ThumbStore>,
    looks: Option<(&crate::looks::LookStore, &LookCache)>,
) -> Option<Thumbnail> {
    load_any(
        req,
        resampler,
        store,
        looks,
        &extract_preview,
        &decode_preview,
    )
}

/// [`load_thumbnail`] with both sources injected — so a test can see which one a request takes.
fn load_any(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    store: Option<&crate::thumb_store::ThumbStore>,
    looks: Option<(&crate::looks::LookStore, &LookCache)>,
    extract: &dyn Fn(&std::path::Path) -> Option<Thumbnail>,
    decode: &dyn Fn(&std::path::Path) -> Option<core_image::ImageBuf>,
) -> Option<Thumbnail> {
    if let Some(t) = negative_thumbnail(req, resampler, looks, decode) {
        return Some(t);
    }
    load_with(req, resampler, store, looks, extract)
        .or_else(|| unembedded_thumbnail(req, resampler, looks, decode))
}

/// A RAW with no embedded preview — an early compact's (a Canon PowerShot Pro70, a Minolta Dimage Z2, a Leica
/// Digilux 2: libraw answers `NoThumbnail`) — thumbnailed from its half-size decode, as a negative is (v0.1).
/// `None` for anything else. ⚠ Rendered through the photo's own rendering, or the camera's when it has none
/// yet: unlike an embedded JPEG this source has not been rendered. Not cached on disk, as for a negative.
fn unembedded_thumbnail(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    looks: Option<(&crate::looks::LookStore, &LookCache)>,
    decode: &dyn Fn(&std::path::Path) -> Option<core_image::ImageBuf>,
) -> Option<Thumbnail> {
    if !catalog::is_raw(&req.id.path) {
        return None;
    }
    let buf = decode(&req.id.path)?.downscale_to(NEGATIVE_EDGE);
    let mut edits = req.edits.clone();
    edits.rendering = edits.rendering.or(Some(edit_model::Rendering::Camera));
    let look = look_for(&edits, looks.map(|l| l.0), looks.map(|l| l.1));
    Some(render_buf(buf, &edits, resampler, look.as_ref()))
}

/// The long edge a RAW negative's thumbnail is rendered at.
const NEGATIVE_EDGE: usize = 512;

/// A RAW with an active film negative, thumbnailed from its half-size raw decode (M10.11a) — `None`
/// for anything else, which takes the embedded preview as before.
///
/// ⚠⚠ **Why not the embedded JPEG:** it has been through the camera's own tone curve, and the film
/// base and densities were measured on the raw data; inverting the JPEG with them gives a badly
/// balanced positive in the grid. ⚠ The cost, accepted for negatives only: a half-size decode, about
/// a second, and **no disk cache** — the store holds 8-bit previews, and an 8-bit negative bands in
/// exactly the dense areas the inversion turns into highlights. Rendered from the float decode with
/// the photo's **own** rendering, since this source has not been rendered.
fn negative_thumbnail(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    looks: Option<(&crate::looks::LookStore, &LookCache)>,
    decode: &dyn Fn(&std::path::Path) -> Option<core_image::ImageBuf>,
) -> Option<Thumbnail> {
    if !(catalog::is_raw(&req.id.path) && req.edits.negative.is_active()) {
        return None;
    }
    let buf = decode(&req.id.path)?.downscale_to(NEGATIVE_EDGE);
    let look = look_for(&req.edits, looks.map(|l| l.0), looks.map(|l| l.1));
    Some(render_buf(buf, &req.edits, resampler, look.as_ref()))
}

/// A half-size raw decode — the linear working-space buffer the negative was measured on.
fn decode_preview(path: &std::path::Path) -> Option<core_image::ImageBuf> {
    raw_decode::decode_raw(path, &raw_decode::DecodeOptions::preview())
        .ok()
        .map(|d| d.image)
}

/// Pull the source's own preview out of it, with no cache in sight.
///
/// ⚠ Split out so the carrier test can count how many times it runs. That count is the only
/// observable that distinguishes "the cache works" from "the cache is never consulted".
fn extract_preview(path: &std::path::Path) -> Option<Thumbnail> {
    if catalog::is_raw(path) {
        raw_decode::extract_thumbnail(path).ok()
    } else {
        image_decode::thumbnail(path).ok()
    }
}

/// `load_thumbnail` with the extraction injected.
///
/// ⚠⚠ **The store caches the EXTRACTED PREVIEW, before the render — never the finished
/// thumbnail.** Extraction is ~191 ms and the render is ~5 ms, so a store keyed on edits would be
/// useless for the case that matters most while editing (a changed edit) and would leave a file
/// behind for every edit state a photo has ever had. `ThumbCache` in memory is the level that is
/// keyed on edits; this one is keyed on the file.
///
/// ⚠ `Identity::of` stats the file. A source with no readable metadata is simply not cached,
/// which degrades to today's behaviour rather than to an error.
fn load_with(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    store: Option<&crate::thumb_store::ThumbStore>,
    looks: Option<(&crate::looks::LookStore, &LookCache)>,
    extract: &dyn Fn(&std::path::Path) -> Option<Thumbnail>,
) -> Option<Thumbnail> {
    let path = req.id.path.as_path();
    let id = store.and_then(|_| crate::thumb_store::Identity::of(path));
    let cached = match (store, &id) {
        (Some(s), Some(i)) => s.get(i),
        _ => None,
    };
    let thumb = match cached {
        Some(t) => t,
        None => {
            let t = extract(path)?;
            if let (Some(s), Some(i)) = (store, &id) {
                s.put(i, &t);
            }
            t
        }
    };
    if req.edits == Edits::default() {
        // ⚠ Neutral edits are the common case on a first folder open; skip the round trip rather
        // than paying it 830 times for a no-op.
        return Some(thumb);
    }
    let look = look_for(&req.edits, looks.map(|l| l.0), looks.map(|l| l.1));
    Some(render_edits(
        thumb,
        &thumbnail_edits(&req.edits),
        resampler,
        look.as_ref(),
    ))
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
/// A thumbnail's sRGB 8-bit pixels as a **linear ProPhoto** buffer — the pipeline's working
/// space, which is what every edit expects to receive.
///
/// ⚠⚠ **`srgb_to_prophoto`, and leaving it out is a bug this shipped once.** The pipeline's
/// output transform converts ProPhoto→sRGB on the way out; handing it linear *sRGB* means that
/// matrix is applied to data that was never in ProPhoto, and the result is heavily
/// **oversaturated** — reported as "the saturation is very exaggerated". `image_decode::decode`
/// does exactly this pair of steps for the same reason. Shared by the thumbnails and the ghost
/// session (`ghost.rs`).
pub fn linear_prophoto_from_srgb8(thumb: &Thumbnail) -> core_image::ImageBuf {
    let mut buf = core_image::ImageBuf::new(thumb.width, thumb.height);
    {
        let d = buf.data_mut();
        for (i, px) in thumb.rgba.as_chunks::<4>().0.iter().enumerate() {
            for c in 0..3 {
                d[i * 3 + c] = color::srgb_to_linear(px[c] as f32 / 255.0);
            }
        }
    }
    buf.map_pixels(color::srgb_to_prophoto)
}

fn render_edits(
    thumb: Thumbnail,
    edits: &Edits,
    resampler: pipeline::Resampler,
    look: Option<&Arc<pipeline::lut::Cube>>,
) -> Thumbnail {
    render_buf(linear_prophoto_from_srgb8(&thumb), edits, resampler, look)
}

/// Run the pipeline over a linear working-space buffer — the thumbnail's renderer, whichever source
/// it came from.
fn render_buf(
    buf: core_image::ImageBuf,
    edits: &Edits,
    resampler: pipeline::Resampler,
    look: Option<&Arc<pipeline::lut::Cube>>,
) -> Thumbnail {
    // ⚠⚠ **The look, and its absence was reported by use**: *"the look is not reflected or shown in
    // any way in the thumbnail picture"*. `build_pipeline_with` takes no `TailRendering` at all,
    // so every thumbnail rendered every edit **except** the user's `.cube` — the third path to lose
    // it this way, after the preview kept it and the export lost it.
    //
    // ⚠ `curve` and `baseline_ev` stay `None` deliberately: `thumbnail_edits` has already stripped
    // the rendering, because the source here is the camera's embedded JPEG and has been rendered
    // once already. Supplying them would be inert at best and a double render at worst.
    let out = pipeline::build_pipeline_rendered(
        edits,
        pipeline::RenderScale::new(1.0),
        // ⚠ The DEFAULT rendering intent, never the canvas's (M6.4c): thumbnails are cached on
        // disk, so following a view toggle would leave a library of mixed thumbnails.
        pipeline::RenderSpec {
            resampler,
            ..pipeline::RenderSpec::default()
        },
        pipeline::TailRendering {
            curve: None,
            baseline_ev: None,
            look,
            // ⚠ M10.13d: none — the source is the camera's own JPEG, which already holds its film simulation.
            camera_look: None,
        },
    )
    .run(buf);
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
pub fn convert_thumb(thumb: &mut Thumbnail, space: color::Display) {
    if space.is_srgb() {
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
        display_space: color::Display,
        thumb_budget_bytes: u64,
        immich_budget_bytes: u64,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        // ⚠ The budget is a parameter rather than read from `Settings` here: the store is a lib
        // module and must not depend on the settings loader. ⚠ It is read once, so a change takes
        // effect at the next folder open — which is what rebuilds this loader.
        let store = crate::thumb_store::ThumbStore::user_default(thumb_budget_bytes);
        // ⚠ Captured here for the ThumbStore's own stated reason — it keeps `ThumbLoader::new`'s
        // signature, and therefore its four call sites, untouched.
        let looks = crate::looks::LookStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        );
        let look_cache: LookCache = Mutex::new(std::collections::HashMap::new());
        // ⚠⚠ Captured here for the same reason the store and the looks are, and with the same
        // consequence: `ThumbLoader::new`'s signature, and therefore its four call sites, stay
        // untouched.
        //
        // ⚠ The shared **cell**, not a snapshot of the connection: pasting a key in Settings must
        // not rebuild the loader, because a rebuild drops the open folder's whole thumbnail cache.
        let immich = crate::immich_source::shared();
        // ⚠ Captured here for the same reason as the local store, and with its own budget: the two
        // caches live in separate directories so neither sweep can reach the other's entries.
        let immich_versions = crate::immich_source::shared_versions();
        let immich_store = crate::thumb_store::ThumbStore::immich_default(immich_budget_bytes);
        Self::with_loader(
            // ⚠⚠ Thumbnails always use the **default** resampler, deliberately, and do not follow
            // `Settings.resampler` (user's call, 2026-09-01). Honouring it was tried first and is
            // pure cost: the difference between kernels is **invisible at 512 px**, while changing
            // the setting would rebuild the loader and drop the whole folder's thumbnail cache —
            // ~41 ms × N, about 34 s on an 830-photo folder. The setting is about what you judge
            // at 1:1, which is where it can be seen.
            // ⚠ The store is captured here rather than passed to `ThumbLoader::new`, which keeps
            // that signature — and therefore its four call sites — untouched.
            Arc::new(move |r: &ThumbRequest| {
                // ⚠⚠ **Only while the original is absent.** Once a photo has been downloaded this
                // declines and the ordinary local path — on-disk store included — takes over with
                // no special case at all. For every non-Immich photo it is one `is_file` and one
                // mutex read.
                if let Some(t) = crate::immich_source::remote_thumbnail(
                    &immich,
                    &immich_versions,
                    &immich_store,
                    r.id.file(),
                ) {
                    return Some(t);
                }
                load_thumbnail(
                    r,
                    pipeline::Resampler::default(),
                    Some(&store),
                    Some((&looks, &look_cache)),
                )
            }),
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
        display_space: color::Display,
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

    /// Add to the wanted list without replacing it — for a caller that is not the grid, such as
    /// the Presets tool asking for one thumbnail per starter look. ⚠ `want` clears the list,
    /// because the grid's window is the list; a second caller must not take it over.
    pub fn also_want(&self, wanted: &[ThumbRequest]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        for r in wanted {
            let key = r.key();
            if !q.in_flight.contains(&key) && !q.wanted.iter().any(|w| w.key() == key) {
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

    /// M10.11a: the loader itself sends a RAW negative to the decode, and anything else to the
    /// embedded preview — the dispatch, not just the helper.
    #[test]
    fn the_loader_sends_a_raw_negative_to_the_decode() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static EXTRACTS: AtomicUsize = AtomicUsize::new(0);
        static DECODES: AtomicUsize = AtomicUsize::new(0);
        fn extract(_p: &std::path::Path) -> Option<Thumbnail> {
            EXTRACTS.fetch_add(1, Ordering::SeqCst);
            Some(Thumbnail {
                width: 2,
                height: 1,
                rgba: vec![128; 8],
            })
        }
        fn decode(_p: &std::path::Path) -> Option<core_image::ImageBuf> {
            DECODES.fetch_add(1, Ordering::SeqCst);
            Some(core_image::ImageBuf::new(4, 2))
        }
        let req = |n: edit_model::Negative| ThumbRequest {
            id: crate::photo_id::PhotoId::original(std::path::PathBuf::from("/scan/a.RAF")),
            edits: Edits {
                negative: n,
                ..Edits::default()
            },
        };
        let on = edit_model::Negative {
            on: true,
            base: [0.85, 0.55, 0.3],
            ..edit_model::Negative::default()
        };
        let r = pipeline::Resampler::default();
        EXTRACTS.store(0, Ordering::SeqCst);
        DECODES.store(0, Ordering::SeqCst);
        load_any(&req(on), r, None, None, &extract, &decode).expect("a thumbnail");
        assert_eq!(
            (
                EXTRACTS.load(Ordering::SeqCst),
                DECODES.load(Ordering::SeqCst)
            ),
            (0, 1)
        );
        load_any(
            &req(edit_model::Negative::default()),
            r,
            None,
            None,
            &extract,
            &decode,
        )
        .expect("a thumbnail");
        assert_eq!(
            (
                EXTRACTS.load(Ordering::SeqCst),
                DECODES.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
    }

    /// M10.11a: a RAW whose negative is active is thumbnailed from its raw decode, rendered with its
    /// own rendering — the base was measured on the raw data, not on the camera's JPEG.
    /// v0.1: a RAW with no embedded preview — measured on a Canon PowerShot Pro70 CRW, a Minolta Dimage Z2
    /// MRW and a Leica Digilux 2 RAW, where libraw answers `NoThumbnail` — is thumbnailed from its decode. The
    /// grid used to stay blank for them. A JPEG is never decoded as a RAW.
    #[test]
    fn a_raw_without_an_embedded_preview_is_thumbnailed_from_its_decode() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DECODES: AtomicUsize = AtomicUsize::new(0);
        fn decode(_p: &std::path::Path) -> Option<core_image::ImageBuf> {
            DECODES.fetch_add(1, Ordering::SeqCst);
            let mut img = core_image::ImageBuf::new(64, 32);
            for y in 0..32 {
                for x in 0..64 {
                    img.set_pixel(x, y, [x as f32 / 63.0; 3]);
                }
            }
            Some(img)
        }
        fn no_preview(_p: &std::path::Path) -> Option<Thumbnail> {
            None
        }
        let req = |file: &str| ThumbRequest {
            id: crate::photo_id::PhotoId::original(std::path::PathBuf::from(file)),
            edits: Edits::default(),
        };
        DECODES.store(0, Ordering::SeqCst);
        let r = pipeline::Resampler::default();
        let got = load_any(&req("/old/a.CRW"), r, None, None, &no_preview, &decode)
            .expect("a RAW without a preview is thumbnailed from its decode");
        assert_eq!(DECODES.load(Ordering::SeqCst), 1);
        assert!(got.width > 0 && got.height > 0);
        assert!(
            load_any(&req("/old/b.jpg"), r, None, None, &no_preview, &decode).is_none(),
            "a JPEG without a thumbnail is not decoded as a RAW"
        );
        assert_eq!(DECODES.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_raw_negative_is_thumbnailed_from_its_decode() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DECODES: AtomicUsize = AtomicUsize::new(0);
        fn decode(_p: &std::path::Path) -> Option<core_image::ImageBuf> {
            DECODES.fetch_add(1, Ordering::SeqCst);
            let mut img = core_image::ImageBuf::new(64, 32);
            for y in 0..32 {
                for x in 0..64 {
                    let d = 0.2 + 1.4 * x as f32 / 63.0;
                    img.set_pixel(x, y, [0.85, 0.55, 0.3].map(|b: f32| b * 10f32.powf(-d)));
                }
            }
            Some(img)
        }
        let negative = edit_model::Negative {
            on: true,
            base: [0.85, 0.55, 0.3],
            low: [0.2; 3],
            high: [1.6; 3],
            ..edit_model::Negative::default()
        };
        let req = |file: &str, n: edit_model::Negative| ThumbRequest {
            id: crate::photo_id::PhotoId::original(std::path::PathBuf::from(file)),
            edits: Edits {
                negative: n,
                rendering: Some(edit_model::Rendering::Camera),
                ..Edits::default()
            },
        };
        DECODES.store(0, Ordering::SeqCst);
        let r = pipeline::Resampler::default();
        let got = negative_thumbnail(&req("/scan/a.RAF", negative), r, None, &decode)
            .expect("a RAW negative takes the decode path");
        assert_eq!(DECODES.load(Ordering::SeqCst), 1);
        let edits = req("/scan/a.RAF", negative).edits;
        let rendered = render_buf(decode(std::path::Path::new("")).unwrap(), &edits, r, None);
        let stripped = render_buf(
            decode(std::path::Path::new("")).unwrap(),
            &thumbnail_edits(&edits),
            r,
            None,
        );
        assert_ne!(
            rendered.rgba, stripped.rgba,
            "the two must differ for this to mean anything"
        );
        assert_eq!(
            got.rgba, rendered.rgba,
            "rendered with the photo's own rendering"
        );
        DECODES.store(0, Ordering::SeqCst);
        assert!(negative_thumbnail(
            &req("/scan/a.RAF", edit_model::Negative::default()),
            r,
            None,
            &decode
        )
        .is_none());
        assert!(
            negative_thumbnail(&req("/scan/a.tif", negative), r, None, &decode).is_none(),
            "a scan's own pixels"
        );
        assert_eq!(DECODES.load(Ordering::SeqCst), 0);
    }
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A flat mid-grey thumbnail.
    fn grey_thumb(w: usize, h: usize) -> Thumbnail {
        Thumbnail {
            width: w,
            height: h,
            rgba: [128, 128, 128, 255].repeat(w * h),
        }
    }

    /// A 2³ cube that **darkens everything to 40%**.
    ///
    /// ⚠⚠ **It has to move a NEUTRAL image, and the obvious "loud" cube does not.** A cube that
    /// permutes channels looks dramatic and is a perfect no-op on grey — which is what every
    /// fixture here is. That exact mistake cost a debugging round on the export's look test the
    /// same day, so it is written down rather than rediscovered.
    fn darkening_cube() -> Arc<pipeline::lut::Cube> {
        let mut t = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    let f = |v: usize| v as f32 * 0.4;
                    t.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        Arc::new(pipeline::lut::Cube::parse(&t).expect("a parseable cube"))
    }

    fn with_look(file: &str) -> Edits {
        Edits {
            look: edit_model::Look {
                file: Some(file.into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Write `names` as `.cube` files into a fresh directory, and return it with a store.
    fn look_store(tag: &str, names: &[&str]) -> (PathBuf, crate::looks::LookStore) {
        let d = test_dir::path(&format!("thumb-look-{tag}"));
        for n in names {
            let mut t = String::from("LUT_3D_SIZE 2\n");
            for _ in 0..8 {
                t.push_str("0 0 0\n");
            }
            std::fs::write(d.join(n), t).unwrap();
        }
        (d.clone(), crate::looks::LookStore::new(d))
    }

    /// ⚠⚠ **THE test of this fix, reported by use:** *"the look is not reflected or shown in any
    /// way in the thumbnail picture"*. `render_edits` built its pipeline with
    /// `build_pipeline_with`, which takes no `TailRendering`, so every thumbnail rendered every
    /// edit except the user's `.cube`.
    ///
    /// ⚠ Sabotage: pass `None` for the look in `render_edits`.
    #[test]
    fn a_look_changes_a_rendered_thumbnail() {
        let e = with_look("x.cube");
        let without = render_edits(grey_thumb(8, 8), &e, pipeline::Resampler::default(), None);
        let cube = darkening_cube();
        let with = render_edits(
            grey_thumb(8, 8),
            &e,
            pipeline::Resampler::default(),
            Some(&cube),
        );
        assert_ne!(
            without.rgba, with.rgba,
            "a look must change the thumbnail's pixels"
        );
    }

    /// ⚠⚠ **A failed load is cached as a MISS, and without that a missing `.cube` is re-read once
    /// per thumbnail** — 830 times on an 830-photo folder, for a file that is not there.
    ///
    /// ⚠ Sabotage: drop the negative caching (`.or_insert_with` storing `None`), and the count
    /// below becomes one per call.
    #[test]
    fn a_look_that_will_not_load_is_attempted_once() {
        let (dir, store) = look_store("missing", &[]);
        let cache: LookCache = Mutex::new(std::collections::HashMap::new());
        let e = with_look("gone.cube");
        for _ in 0..5 {
            assert!(
                look_for(&e, Some(&store), Some(&cache)).is_none(),
                "a look this machine does not have resolves to nothing"
            );
        }
        assert_eq!(
            cache.lock().unwrap().len(),
            1,
            "the failure must be remembered, not retried per thumbnail"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ A look at zero strength is not a look: it must not be loaded at all.
    #[test]
    fn an_inactive_look_resolves_to_nothing() {
        let (dir, store) = look_store("inactive", &["x.cube"]);
        let cache: LookCache = Mutex::new(std::collections::HashMap::new());
        let mut e = with_look("x.cube");
        e.look.strength = 0.0;
        assert!(look_for(&e, Some(&store), Some(&cache)).is_none());
        assert!(
            cache.lock().unwrap().is_empty(),
            "an inactive look must not even be looked up"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::sync::Mutex;

    /// `load_thumbnail` with the shipping kernel — the tests using it are about edits reaching
    /// the pixels, not about which kernel read them.
    fn load_thumbnail_default(req: &ThumbRequest) -> Option<Thumbnail> {
        load_thumbnail(req, pipeline::Resampler::default(), None, None)
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
        let loader = ThumbLoader::new(
            color::Display::Space(color::OutputSpace::Srgb),
            u64::MAX,
            u64::MAX,
            || {},
        );
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
            color::Display::Space(color::OutputSpace::Srgb),
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
            color::Display::Space(color::OutputSpace::Srgb),
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
        let loader = ThumbLoader::with_loader(
            Arc::new(slow),
            2,
            color::Display::Space(color::OutputSpace::Srgb),
            || {},
        );
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
        let loader = ThumbLoader::with_loader(
            Arc::new(gated),
            4,
            color::Display::Space(color::OutputSpace::Srgb),
            || {},
        );
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
        let loader = ThumbLoader::with_loader(
            Arc::new(gated),
            PAR,
            color::Display::Space(color::OutputSpace::Srgb),
            || {},
        );
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
        let loader = ThumbLoader::with_loader(
            Arc::new(counted),
            2,
            color::Display::Space(color::OutputSpace::Srgb),
            || {},
        );
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
            color::Display::Space(color::OutputSpace::Srgb),
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
            color::Display::Space(color::OutputSpace::Srgb),
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
        convert_thumb(&mut t, color::Display::Space(color::OutputSpace::DisplayP3));
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
        convert_thumb(&mut t, color::Display::Space(color::OutputSpace::Srgb));
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
            color::Display::Space(color::OutputSpace::Srgb),
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
            color::Display::Space(color::OutputSpace::Srgb),
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
            None,
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
        let out = render_edits(thumb, &edits, pipeline::Resampler::default(), None);
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
        let dir = test_dir::path("thumbrender");
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
            None,
        );
        let after = spread(&after.rgba);
        assert!(
            (after - before).abs() < 0.02,
            "a negligible edit changed mean saturation from {before:.4} to {after:.4} — the \
             thumbnail is not being converted into the pipeline's ProPhoto working space"
        );
    }

    /// ⚠⚠ THE CARRIER. Everything in `thumb_store`'s own tests can pass while `load_thumbnail`
    /// never calls it — which is exactly how creative LUTs shipped with four passing pipeline
    /// tests and were 100% non-functional. This counts the **extractions**, the only observable
    /// that tells "the cache works" apart from "the cache is never asked".
    ///
    /// ⚠ It needs a real file on disk, because `Identity::of` stats it. A synthetic path has no
    /// mtime, so it would be uncacheable and the test would pass for the wrong reason.
    #[test]
    fn a_second_load_reads_the_store_instead_of_extracting_again() {
        let dir = carrier_dir("carrier");
        let photo = dir.join("a.png");
        image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8 * 8, y as u8 * 8, 90]))
            .save(&photo)
            .unwrap();

        let store = crate::thumb_store::ThumbStore::new(dir.join("thumbs"), u64::MAX);
        let n = AtomicUsize::new(0);
        let extract = |p: &std::path::Path| {
            n.fetch_add(1, Ordering::Relaxed);
            image_decode::thumbnail(p).ok()
        };
        let req = ThumbRequest {
            id: PhotoId::original(photo),
            edits: Edits::default(),
        };

        let first = load_with(
            &req,
            pipeline::Resampler::default(),
            Some(&store),
            None,
            &extract,
        );
        assert!(first.is_some(), "the first load must produce a thumbnail");
        assert_eq!(n.load(Ordering::Relaxed), 1, "the first load extracts");

        let second = load_with(
            &req,
            pipeline::Resampler::default(),
            Some(&store),
            None,
            &extract,
        );
        assert!(second.is_some(), "the second load must produce a thumbnail");
        assert_eq!(
            n.load(Ordering::Relaxed),
            1,
            "the SECOND load must come from the store, not from a second extraction"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ The other half: with no store the behaviour is exactly what it was before this feature.
    /// Without this, a `load_with` that ignored its `None` and always cached would pass above.
    #[test]
    fn with_no_store_every_load_extracts() {
        let dir = carrier_dir("nostore");
        let photo = dir.join("a.png");
        image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8, y as u8, 7]))
            .save(&photo)
            .unwrap();

        let n = AtomicUsize::new(0);
        let extract = |p: &std::path::Path| {
            n.fetch_add(1, Ordering::Relaxed);
            image_decode::thumbnail(p).ok()
        };
        let req = ThumbRequest {
            id: PhotoId::original(photo),
            edits: Edits::default(),
        };
        let _ = load_with(&req, pipeline::Resampler::default(), None, None, &extract);
        let _ = load_with(&req, pipeline::Resampler::default(), None, None, &extract);
        assert_eq!(n.load(Ordering::Relaxed), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠⚠ And the half that makes the cache SAFE rather than merely fast: a file edited outside
    /// Rustroom must re-extract. Without it the grid would show yesterday's picture forever, and
    /// no test above could see that — they all reuse one unchanged file.
    #[test]
    fn a_file_changed_on_disk_is_extracted_again() {
        let dir = carrier_dir("changed");
        let photo = dir.join("a.png");
        let store = crate::thumb_store::ThumbStore::new(dir.join("thumbs"), u64::MAX);
        let n = AtomicUsize::new(0);
        let extract = |p: &std::path::Path| {
            n.fetch_add(1, Ordering::Relaxed);
            image_decode::thumbnail(p).ok()
        };
        let req = ThumbRequest {
            id: PhotoId::original(photo.clone()),
            edits: Edits::default(),
        };

        image::RgbImage::from_fn(8, 6, |_, _| image::Rgb([10, 20, 30]))
            .save(&photo)
            .unwrap();
        let _ = load_with(
            &req,
            pipeline::Resampler::default(),
            Some(&store),
            None,
            &extract,
        );
        assert_eq!(n.load(Ordering::Relaxed), 1);

        // ⚠ A different SIZE as well as a different time, so the test cannot pass merely because
        // this machine's filesystem happens to have fine mtime resolution.
        image::RgbImage::from_fn(24, 18, |x, y| image::Rgb([x as u8, y as u8, 200]))
            .save(&photo)
            .unwrap();
        let got = load_with(
            &req,
            pipeline::Resampler::default(),
            Some(&store),
            None,
            &extract,
        )
        .expect("the changed file must still produce a thumbnail");
        assert_eq!(
            n.load(Ordering::Relaxed),
            2,
            "a file changed on disk must be extracted again, not served stale"
        );
        // ⚠ On the CONTENT, not the dimensions. `image_decode::thumbnail` normalises every
        // source to a 512 px long edge, so both pictures come back 512x384 and a size assertion
        // is wrong about correct code — it passed against a stale thumbnail. The old image is
        // flat blue 30, the new one flat blue 200, which nothing but a fresh extraction gives.
        let mean_blue = got
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[2] as u32)
            .sum::<u32>()
            / (got.rgba.len() / 4) as u32;
        assert!(
            mean_blue > 150,
            "expected the NEW picture (blue ~200), got mean blue {mean_blue}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scratch directory for the carrier tests.
    fn carrier_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("carrier-{tag}"))
    }
}

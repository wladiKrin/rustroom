//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

use crate::prefix_cache::{PrefixCache, Tier};
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{build_prefix_steps, PrefixKey, Region, RenderScale};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// A finished preview frame (display sRGB, RGBA8).
pub struct RenderedFrame {
    pub width: usize,
    pub height: usize,
    /// The frame in the **display's** primaries — what this monitor must be driven with.
    ///
    /// ⚠ An `Arc` so the sRGB case costs nothing: when the display space *is* sRGB, `rgba` and
    /// `srgb` are two handles on one allocation. `ColorImage::from_rgba_unmultiplied` takes a
    /// slice, so the texture path never needs ownership.
    pub rgba: Arc<Vec<u8>>,
    /// The same frame quantized to **sRGB**, before the display transform and before the
    /// Visualize Spots view mode.
    ///
    /// ⚠⚠ **This is the instrument channel, and `rgba` is not.** `hist` is already bucketed from
    /// these bytes for the reason `pack` records: "the histogram describes the image; the texture
    /// describes what this monitor must be driven with. Deriving one from the other is the bug
    /// this ordering exists to prevent." The colour readout is a second instrument of exactly
    /// that kind — reading `rgba` would give numbers that change when you change monitors.
    ///
    /// ⚠ Taken **before** Visualize Spots replaces the image, so the readout reports the
    /// photograph rather than the grey edge view.
    pub srgb: Arc<Vec<u8>>,
    /// Where this frame sits in the post-geometry image; `None` = the whole image. The canvas
    /// needs it to place the texture.
    pub region: Option<Region>,
    /// The image's histogram, computed **before** the display transform.
    ///
    /// ⚠⚠ Carried on the frame rather than derived from `rgba` by the caller, and that is the
    /// whole point: `rgba` is in the *display's* primaries, so bucketing it would produce a
    /// histogram that changes when you change monitors — useless as a clipping instrument,
    /// which is the one job it has. This was shipped wrong once and caught by asking what
    /// `shell.rs` actually reads.
    pub hist: [[u32; 256]; 3],
    /// The `max_dim` this frame was rendered at.
    ///
    /// ⚠ Carried rather than inferred. The canvas caches the post-geometry size from
    /// whole-image frames, and twice now a bug has come from inferring "whole image, therefore
    /// a fit render" — an inference that a cropped photo can make false, since a native rung
    /// times a small crop factor lands below the preview cap. This makes it a fact.
    pub max_dim: usize,
    /// The post-geometry image size **at the fit cap**, in exact floats — what the canvas places
    /// every overlay against.
    ///
    /// ⚠⚠ **Carried rather than derived by the caller, for the reason `max_dim` above is.** It was
    /// computed in `shell.rs` as `frame_dims x fit_cap / max_dim`, and a frame's dimensions come
    /// from `downscale_to`, which rounds — so the rounding was amplified by `rung / fit_cap` and
    /// the placement depended on which rung last landed. Measured: 1068.000 at rungs 1600 and
    /// 3200 against a true 1068.204 at 6246.
    ///
    /// ⚠ Deriving it in `shell.rs` from `Session::edits` instead would also be exact and would be
    /// **worse**: it would describe a crop the frame on screen does not have until the next one
    /// lands, so the texture would be misplaced for a frame or two after every crop change.
    /// Travelling with the pixels it describes is what makes that unrepresentable.
    pub fit_size: (f32, f32),
    /// The grade range view's plane — one byte per pixel, the selected range's weight — or
    /// `None` when the view is off.
    ///
    /// ⚠ Taken at the grading stage's **input** (`TailHalves::before_grade`'s output), the only
    /// place the ranges are exact; see `grade_view`. `width × height` bytes, the frame's own
    /// dimensions, by construction: no stage after the tap changes a buffer's size.
    pub grade_view: Option<Arc<Vec<u8>>>,
}

/// A requested crop of the rendered image, in post-geometry pixels.
///
/// ⚠⚠ **This was `RegionRect`, a struct identical to `pipeline::Rect` in everything but its name.**
/// Two types that cannot disagree because they are the same type twice is still two places to
/// change, and M8.7 would otherwise have added a third. The alias is kept so the ~40 call sites in
/// this crate keep reading as "a region", which is what they are about.
pub use pipeline::Rect as RegionRect;

/// A soft proof: the output space the canvas is being shown through, and whether the pixels that
/// space cannot hold are marked.
///
/// ⚠ The two travel together because the warning is meaningless without a space to be out of.
/// Carrying `gamut_warning` as a bare `bool` beside an `Option<OutputSpace>` would make
/// "warn, against nothing" representable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Proof {
    /// The space being proofed into. Never `ProPhoto`: the menu is built from
    /// [`color::OutputSpace::DISPLAYS`], and proofing into the working space would show nothing.
    pub space: color::OutputSpace,
    /// Whether out-of-gamut pixels are marked.
    ///
    /// ⚠ A separate toggle from the proof itself. Proofing *without* the warning is the common
    /// case — you want to see the picture — and a warning you cannot switch off is chrome.
    pub warn: bool,
}

/// What frame to produce: the working resolution, the visible sub-rect, the display's
/// primaries, and the resampling kernel.
///
/// Grouped because they travel together the whole way from a request to `pack`, and threading
/// them individually pushed `render_cached` past clippy's argument limit — which was a fair
/// complaint rather than a lint to silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSpec {
    pub max_dim: usize,
    pub region: Option<RegionRect>,
    /// ⚠ Applied in `pack`, *after* the pipeline — never as a pipeline stage, or the histogram
    /// becomes display-referred. See the spec.
    pub display_space: color::OutputSpace,
    /// ⚠⚠ Per **request**, not captured at `RenderEngine::new` the way `lens` is — and the
    /// difference is not stylistic. A lens profile cannot vary within one engine's life,
    /// because the engine is rebuilt per photo; the resampler is a global setting the user can
    /// change with a photo open. Passing it per request is what lets the change take effect on
    /// the next render with no engine rebuild — and because the three resampling
    /// `PrefixStepKey`s carry it, a stale checkpoint is an ordinary cache miss.
    pub resampler: pipeline::Resampler,
    /// Run the **global** HSL stage on the GPU.
    ///
    /// ⚠ Per **request**, for `resampler`'s reason directly above: a global setting the user can
    /// change with a photo open, so a toggle takes effect on the next render with no engine
    /// rebuild.
    ///
    /// ⚠⚠ Unlike the resampler it is in **no** `PrefixStepKey`, and must not be. HSL is a *tail*
    /// stage, so the prefix computes the same thing either way and every cached checkpoint stays
    /// valid — putting it in the key would invalidate the prefix for a change that cannot affect
    /// it.
    pub gpu_tail: bool,
    /// The Visualize Spots threshold, or `None` when the view is off.
    ///
    /// ⚠⚠ Applied in `pack`, *after* the pipeline — **never as a pipeline stage**, and the reason
    /// is recorded rather than reasoned: the display transform went into `pack` because a stage
    /// would make the **histogram** display-referred, and `pack` already takes the histogram from
    /// its sRGB output *before* the post-step. This is the same shape and inherits that ordering.
    /// As a stage it would also want a place in the cache key for something that is not an edit.
    ///
    /// ⚠ A view-only property of the *request*, like `display_space` — it never reaches `Edits`,
    /// the sidecar, an export or the CLI, and toggling it invalidates no checkpoint, because
    /// `CacheKey` is `(PrefixKey, max_dim)`.
    pub visualize: Option<f32>,
    /// The soft proof, or `None` when the canvas shows the ordinary render.
    ///
    /// ⚠⚠ **While this is `Some` the tail runs TWICE**, and that is the design rather than an
    /// accident of implementation — see `render_cached`. `None` runs it exactly once, on the path
    /// that existed before this field, so the ordinary render is bit-exactly unchanged.
    ///
    /// ⚠ A view-only property of the *request*, like `display_space` and `visualize`: it never
    /// reaches `Edits`, the sidecar, an export or the CLI, and toggling it invalidates no
    /// checkpoint, because `CacheKey` is `(PrefixKey, max_dim)` and the whole proof lives in the
    /// tail.
    pub soft_proof: Option<Proof>,
    /// The grade range view, or `None` when it is off.
    ///
    /// ⚠ A view-only property of the *request*, like `visualize` and `soft_proof`: never
    /// `Edits`, never the sidecar, an export or the CLI, and it invalidates no checkpoint,
    /// because `CacheKey` is `(PrefixKey, max_dim)` and the whole tap lives in the tail.
    ///
    /// ⚠ Unlike `visualize` it is **not** a post-step in `pack`: `pack` sees the output
    /// transform's result, and the ranges are selected on lightness before the display
    /// rendering — see `render_cached`.
    pub grade_view: Option<crate::grade_view::GradeRangeView>,
}

impl FrameSpec {
    /// A whole-frame render in sRGB.
    ///
    /// ⚠⚠ **`#[cfg(test)]` on purpose — do not make this available to production.** It bakes in
    /// `Srgb`, and a production call site that reached for it is exactly how the display setting
    /// came to do nothing: the render issued when a photo opened used this, so every photo
    /// opened uncorrected and stayed that way until some unrelated edit forced a re-render.
    /// Gating it makes that bug a compile error rather than something to remember.
    #[cfg(test)]
    pub fn fit(max_dim: usize) -> Self {
        Self {
            max_dim,
            region: None,
            display_space: color::OutputSpace::Srgb,
            resampler: pipeline::Resampler::default(),
            // ⚠ The CPU path is the reference, so a test fixture takes it unless it says
            // otherwise. A GPU-by-default fixture would make every unrelated test depend on an
            // adapter being present.
            gpu_tail: false,
            visualize: None,
            soft_proof: None,
            grade_view: None,
        }
    }
}

struct Request {
    edits: Edits,
    spec: FrameSpec,
}

/// Owns a worker thread that renders against a fixed editing buffer.
pub struct RenderEngine {
    req_tx: Sender<Request>,
    res_rx: Receiver<RenderedFrame>,
    _handle: JoinHandle<()>,
    /// How many renders have been asked for over this engine's life.
    ///
    /// ⚠⚠ **Unconditional, not `#[cfg(test)]`, and that is forced rather than sloppy.** This type
    /// lives in the **lib**; its only test-time consumer (`develop/canvas.rs`) lives in the
    /// **bin**, and when the bin's test target builds, the lib is a dependency compiled *without*
    /// `cfg(test)` — so a gated field would simply not exist where it is needed.
    ///
    /// It earns two `usize` of production surface because the alternative is untestable: "a drag
    /// mutated `Edits` but never asked for a render" leaves no trace anywhere else, and that bug
    /// has shipped — the mask overlay kept showing its previous position until some unrelated
    /// edit happened to request a frame.
    requests: Arc<std::sync::atomic::AtomicUsize>,
    /// Where this engine's worker looks for `.cube` files, kept so the **full-resolution
    /// handover** can carry it onto the replacement engine.
    ///
    /// ⚠⚠ The decode cannot supply it — `decode.rs` sets `looks_dir: None` on purpose — so the
    /// handover rebuilding from `decoded.profile` silently dropped it, and every creative look
    /// stopped loading ~4 s after a RAW opened. Retained here rather than re-derived at the
    /// handover, because "where looks live" existing in two places is how the two come to
    /// disagree.
    looks_dir: Option<std::path::PathBuf>,
}

impl RenderEngine {
    /// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
    /// same file (0.5 for a half-size RAW decode, 1.0 for JPEG/PNG/TIFF).
    /// `on_done` is called after each render completes (e.g. `ctx.request_repaint`).
    /// `lens` is the resolved profile for this photo, or `None` when the file records no lens or
    /// none matched.
    ///
    /// ⚠ **Captured here rather than passed per request.** The engine is rebuilt per photo, so a
    /// profile cannot vary within one engine's life — the same shape `DecodeWorker` uses for the
    /// camera profile, and for the same reason: a per-request parameter would impose a choice on
    /// callers that have no business making it.
    pub fn new(
        buffer: Arc<ImageBuf>,
        source_scale: f32,
        lens: Option<lens_profile::LensShot>,
        wb_baseline: color::wb::Illuminant,
        // ⚠ Captured, not per request: neither can change without a re-decode. The profile is a
        // `Settings` choice that rebuilds the decode worker, and whether a buffer is
        // scene-referred is a property of the file.
        profile: crate::decode_worker::ProfileParts,
        scene_referred: bool,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let looks_dir = profile.looks_dir.clone();
        let kept_looks_dir = looks_dir.clone();
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<RenderedFrame>();
        let handle = thread::spawn(move || {
            let mut cache = PrefixCache::default();
            // ⚠⚠ **Resolved HERE rather than on the UI thread**, and cached by name: a 64³ cube
            // is 786 KB and this loop runs on every slider frame. One slot is enough — a photo
            // has one look, and switching looks is a deliberate act, not a drag.
            let look_store = looks_dir.map(crate::looks::LookStore::new);
            let mut look_cache: Option<(String, Arc<pipeline::lut::Cube>)> = None;
            // Lives beside the prefix cache and for the same reason: this thread is the sole
            // renderer, so neither needs locking.
            let mut brush_cache = BrushCache::default();
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                // The stage count is for the tests and the bench; the worker only wants pixels.
                // ⚠⚠ **A source that is already rendered never renders again.** A JPEG, a PNG
                // or a TIFF has been through whatever produced it; applying ours on top would
                // render the photograph twice. Stripped here rather than at every call site, so
                // no caller can forget.
                let edits = if scene_referred {
                    req.edits
                } else {
                    edit_model::Edits {
                        // ⚠ Undecided, not decided off: a stripped copy is a render
                        // input, not a choice anyone made.
                        rendering: None,
                        ..req.edits
                    }
                };
                // ⚠ A look that will not load leaves `look` as `None`, which renders the photo
                // *without the look* and keeps the camera rendering — never an error that stops
                // the photo opening. The panel is where the user is told; see `LooksState`.
                if let (Some(store), Some(file)) = (look_store.as_ref(), edits.look.file.as_deref())
                {
                    if edits.look.is_active() && look_cache.as_ref().is_none_or(|(n, _)| n != file)
                    {
                        look_cache = store
                            .load(file)
                            .ok()
                            .map(|c| (file.to_owned(), Arc::new(c)));
                    }
                }
                let look = look_cache
                    .as_ref()
                    .filter(|(n, _)| {
                        edits.look.is_active() && edits.look.file.as_deref() == Some(n.as_str())
                    })
                    .map(|(_, c)| c);

                // ⚠ Env-gated, like `RUSTROOM_DEBUG_DISPLAY` below and `RUSTROOM_DEBUG_BRUSH`
                // before it, and added for the same reason both of those were: what a drag frame
                // costs depends on the **rung** the viewport put it on, and no benchmark can
                // guess that — `bench_masked` measures 1.7 Mpx at rung 1600 and 6.5 Mpx at the
                // native rung, a 4x spread, with only the running window able to say which one a
                // user is actually waiting for.
                let debug_render = std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some();
                let t0 = std::time::Instant::now();
                let (frame, ran) = render_cached(
                    &buffer,
                    &edits,
                    req.spec,
                    source_scale,
                    lens.clone(),
                    wb_baseline,
                    pipeline::ProfileRendering {
                        curve: profile.curve.as_deref(),
                        baseline_ev: profile.baseline_ev,
                        look,
                    },
                    &mut cache,
                    &mut brush_cache,
                );
                if debug_render {
                    eprintln!(
                        "render: {:6.1} ms  rung {:5}  frame {}x{} ({:.1} Mpx)  region {}  masks {}  prefix_ran {}  scoped {}  bytes {}",
                        t0.elapsed().as_secs_f64() * 1000.0,
                        req.spec.max_dim,
                        frame.width,
                        frame.height,
                        (frame.width * frame.height) as f64 / 1e6,
                        if frame.region.is_some() { "yes" } else { "no " },
                        edits.masks.len(),
                        ran.prefix_stages,
                        // ⚠ M8.6's observable in the running app. The two paths produce the same
                        // picture by design, so without this a gate that never fires looks
                        // exactly like one that does.
                        if ran.region_scoped { "yes" } else { "no " },
                        if ran.bytes { "yes" } else { "no " },
                    );
                }
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();
            }
        });
        Self {
            req_tx,
            res_rx,
            _handle: handle,
            requests: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            looks_dir: kept_looks_dir,
        }
    }

    /// Queue a render at the given working resolution, optionally cropped to `region`.
    pub fn request(&self, edits: Edits, spec: FrameSpec) {
        // ⚠ Bumped before the send, and unconditionally: a send to a dropped receiver returns
        // `Err`, but a request that could not be delivered is still one that was *asked for*,
        // which is what the tests assert about.
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.req_tx.send(Request { edits, spec });
    }

    /// How many renders have been asked for. See the field's doc comment for why this is not
    /// `#[cfg(test)]`.
    /// Where this engine loads `.cube` files from, so the full-resolution handover can carry it.
    ///
    /// ⚠ Unconditional rather than `#[cfg(test)]`, for `requests_issued`'s reason: `session_flow`
    /// is in the **bin** and this is the **lib**, which the bin's test target compiles without
    /// `cfg(test)`, so a gated accessor would not exist where it is needed.
    pub fn looks_dir(&self) -> Option<&std::path::Path> {
        self.looks_dir.as_deref()
    }

    pub fn requests_issued(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Return the newest finished frame, if any (drops older ones).
    pub fn poll_latest(&self) -> Option<RenderedFrame> {
        let mut latest = None;
        while let Ok(f) = self.res_rx.try_recv() {
            latest = Some(f);
        }
        latest
    }
}

/// The long edge `downscale_to(max_dim)` will produce, computed without doing the downscale.
///
/// The preview cache needs the render scale on a cache *hit*, when there is no downscaled
/// image to measure. `downscale_to` never upscales and caps the long edge, so this is exact
/// rather than an estimate — asserted against it in the tests.
fn rendered_long_edge(buffer_long: usize, max_dim: usize) -> usize {
    if max_dim == 0 {
        buffer_long
    } else {
        buffer_long.min(max_dim)
    }
}

/// How far a render is from the full-resolution original: the source buffer may already
/// be a half-size decode (`source_scale`), and `downscale_to` shrinks it further. The
/// long edge is the reference because that is what `downscale_to` caps.
fn render_scale(buffer_long: usize, rendered_long: usize, source_scale: f32) -> RenderScale {
    if buffer_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(source_scale * rendered_long as f32 / buffer_long as f32)
}

/// The [`RenderScale`] for a render at `max_dim`, expressed from the **full-resolution** long edge.
///
/// ⚠⚠ Algebraically identical to [`render_scale`], and phrased this way on purpose. At the
/// full-resolution handover `buffer_long` doubles and `source_scale` halves, while `full_long`
/// does **not move** — that invariance is the property it was created for. A caller deriving from
/// it therefore cannot be silently wrong by 2x for having failed to update a `source_scale` at the
/// handover: that bug is unrepresentable rather than merely tested.
/// `scale_from_full_agrees_with_the_engines_own_scale` pins the equality on both sides of it.
///
/// ⚠ `pub`, not `pub(crate)`, and that is **forced** rather than lax — the same reason
/// `requests_issued` is unconditional. This module is in the **lib**; `canvas.rs` and `Session`
/// are in the **bin**, which is a separate crate, so `pub(crate)` would not reach them.
///
/// It is needed because the **canvas** sizes its region request from
/// `pipeline::tail_kernel_radius`, which is measured in rendered pixels, so it needs exactly the
/// scale the worker will use. Re-deriving it there would be two facts that can disagree, with a
/// silent failure mode — a subtly wrong band at the edge of the visible area, and no error.
pub fn scale_from_full(full_long: usize, buffer_long: usize, max_dim: usize) -> RenderScale {
    if full_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(rendered_long_edge(buffer_long, max_dim) as f32 / full_long as f32)
}

/// Pack a rendered sRGB image into an RGBA8 frame.
///
/// The quantization lives on `ImageBuf`, where the buffer and rayon already are — which is also
/// why `app` needs no `rayon` dependency of its own.
/// Quantize a rendered frame for the GPU, converting into the display's primaries on the way.
///
/// ⚠ The conversion happens **here, in float, before quantizing** — not on the packed bytes. An
/// 8-bit → 8-bit conversion compresses the range and costs roughly 0.8 bits, which shows as
/// banding on a smooth sky. It also happens *after* the pipeline rather than inside it, so the
/// histogram stays image-referred; see the spec for why that is the whole design.
///
/// ⚠ `proofed` is the **second** tail run — the same frame rendered into ProPhoto — paired with the
/// proof it was rendered for. It is `None` on the ordinary path, which is then bit-exactly what it
/// was before soft proofing existed.
/// What a finished render hands to [`pack`].
///
/// ⚠⚠ **The `Rgba8` arm carries no `ImageBuf` at all**, and that is what keeps it small: the
/// histogram already comes from the quantized bytes, and `srgb` and `rgba` become one shared
/// allocation — precisely the existing `display_space == Srgb` branch.
///
/// ⚠ It is constructible **only where M8.3g's gate held**. Three things downstream need the float
/// — a non-sRGB display space, Visualize Spots and soft proofing — and on this arm they are
/// unrepresentable rather than guarded against.
pub enum Packed {
    Float(ImageBuf),
    Rgba8 { bytes: Vec<u8>, w: usize, h: usize },
}

/// The float image, on the paths that need it.
///
/// ⚠ Every caller is on a path [`Packed::Rgba8`] cannot reach, because the gate that builds it
/// excludes exactly those three. The message is the documentation of that invariant — if it ever
/// fires, the gate and this function have come to disagree, which is the one thing the enum is
/// shaped to prevent.
fn float(out: &Option<ImageBuf>) -> &ImageBuf {
    out.as_ref()
        .expect("this path needs the float image, and the gate keeps it for exactly these")
}

fn pack(
    packed: Packed,
    proofed: Option<(ImageBuf, Proof)>,
    region: Option<Region>,
    max_dim: usize,
    display_space: color::OutputSpace,
    visualize: Option<f32>,
) -> RenderedFrame {
    let (out, width, height, pre) = match packed {
        Packed::Float(out) => {
            let (w, h) = (out.width(), out.height());
            (Some(out), w, h, None)
        }
        Packed::Rgba8 { bytes, w, h } => (None, w, h, Some(bytes)),
    };
    // ⚠ Quantized to sRGB *first*, and the histogram taken from that. The histogram describes
    // the image; the texture describes what this monitor must be driven with. Deriving one from
    // the other is the bug this ordering exists to prevent.
    //
    // ⚠⚠ On the byte path the GPU has **already done this quantization**, with the same rounding
    // rule — so the histogram is unchanged by construction rather than by a second implementation.
    // That is what makes M8.3g's `Rgba8` arm small: it needs no `ImageBuf` at all.
    let srgb8 = match pre {
        Some(b) => b,
        None => float(&out).to_rgba8(),
    };
    let hist = crate::histogram::histogram_rgba(&srgb8);
    // ⚠⚠ Wrapped **here**, above the Visualize Spots block, so `srgb` carries the photograph's
    // bytes rather than the edge view's — the same ordering `hist` on the line above depends on,
    // and what makes the colour readout report the photograph under any view mode.
    //
    // ⚠ Moved into the `Arc` rather than cloned, so the ordinary path copies nothing and the
    // sRGB branch at the bottom shares this one allocation with `rgba` instead of duplicating
    // ~7 MB (the fit rung) or ~27 MB (a native one) on every rendered frame.
    let srgb = Arc::new(srgb8);

    // ⚠⚠ Visualize Spots goes here: AFTER the histogram, BEFORE the display transform. The
    // histogram must describe the photograph and not the view mode — the rule `display_space`
    // established above — and the display transform must still run, or the grey view would be
    // driven into the wrong primaries like any other pixels. Sabotage-verified by moving the
    // histogram below this block.
    //
    // ⚠ `shown` is what the *texture* shows; `srgb` above stays the photograph. In the ordinary
    // case they are one allocation.
    let (out, shown) = match visualize {
        Some(threshold) => {
            let out = edge_image(float(&out), threshold);
            let shown = Arc::new(out.to_rgba8());
            (Some(out), shown)
        }
        None => (out, Arc::clone(&srgb)),
    };

    // Env-gated, like `RUSTROOM_DEBUG_BRUSH`: instrumentation beat reasoning twice on the brush
    // canvas, and it did again here — the setting reaching `pack` is not something the code
    // reads wrong, it is something a call site can quietly bypass.
    if std::env::var_os("RUSTROOM_DEBUG_DISPLAY").is_some() {
        eprintln!(
            "pack: {}x{} space={:?} region={} proof={:?}",
            width,
            height,
            display_space,
            region.is_some(),
            proofed.as_ref().map(|(_, p)| p),
        );
    }

    let rgba = match proofed {
        // ⚠⚠ The proof **replaces** the display transform for the texture rather than composing
        // with it. That is the original architecture of this feature, kept through the amendment
        // that changed where the data comes from: soft proofing points the space `pack` converts
        // into at the export target instead of at the monitor. The consequence, recorded in the
        // spec rather than discovered later: while proofing, the display-profile setting has no
        // effect — which is also why the spec says the thing to look at is the *difference*
        // between proof and no proof, not the absolute colour.
        Some((proof_out, proof)) => Arc::new(proof_texture(&proof_out, proof)),
        None if display_space == color::OutputSpace::Srgb => shown,
        None => {
            // ⚠ Converted from the **float** image, not from `shown`. An 8-bit → 8-bit conversion
            // compresses the range and costs roughly 0.8 bits, which shows as banding on a smooth
            // sky. The extra quantization pass is paid only when a display space is actually set.
            Arc::new(
                float(&out)
                    .map_pixels(move |c| {
                        let lin = c.map(color::srgb_to_linear);
                        let conv = display_space.from_srgb(lin);
                        [
                            display_space.encode(conv[0]),
                            display_space.encode(conv[1]),
                            display_space.encode(conv[2]),
                        ]
                    })
                    .to_rgba8(),
            )
        }
    };

    RenderedFrame {
        width,
        height,
        rgba,
        srgb,
        hist,
        region,
        max_dim,
        // ⚠ A placeholder. `pack` knows neither the buffer nor the geometry; both callers
        // overwrite this immediately. It is the frame's own size, so a caller that forgot would
        // degrade to the pre-`fit_size` behaviour at the fit rung rather than to a zero.
        fit_size: (width as f32, height as f32),
        grade_view: None,
    }
}

/// The colour an out-of-gamut pixel is marked with, as **encoded output values** so it lands in
/// the texture exactly as written rather than being converted into the proof space on the way.
///
/// ⚠⚠ **Fixed, never the theme accent.** The accent is user-switchable and includes a `Neutral`
/// setting whose whole point is to have no colour — which would defeat the warning precisely when
/// somebody had chosen it. The crop overlay records the same reasoning for the same red
/// `(255, 60, 60)`, and reusing it keeps one rule rather than adding a second.
const GAMUT_WARNING: [f32; 3] = [1.0, 60.0 / 255.0, 60.0 / 255.0];

/// The texture for a soft proof: the ProPhoto-run frame taken back to the working space, marked
/// where the proof space cannot hold it, and encoded into that space.
///
/// ⚠⚠ **The decode is what makes the warning able to fire at all.** `OutputSpace::encode` clamps
/// to `[0, 1]` in every space, so the ordinary sRGB tail hands `pack` data that is already gamut
/// clipped — against which an exact out-of-gamut test is identically false for every target,
/// because sRGB fits inside all of them. The tail is run into ProPhoto — the documented bit-exact
/// identity, which converts nothing — precisely so this round trip recovers the working-space
/// colour instead.
///
/// ⚠ "Exact" therefore means exact *within ProPhoto*: a colour outside it was clipped by that
/// encode and cannot be recovered here. ProPhoto holds essentially every real colour and the
/// display rendering bounds the data before it, so the residue is negligible — stated so it is not
/// later found as a bug.
fn proof_texture(out: &ImageBuf, proof: Proof) -> Vec<u8> {
    out.map_pixels(move |c| {
        let working = c.map(|v| color::OutputSpace::ProPhoto.decode(v));
        if proof.warn && color::is_out_of_gamut(working, proof.space) {
            return GAMUT_WARNING;
        }
        let conv = proof.space.from_working(working);
        [
            proof.space.encode(conv[0]),
            proof.space.encode(conv[1]),
            proof.space.encode(conv[2]),
        ]
    })
    .to_rgba8()
}

/// Render one request, resuming from the deepest cached prefix checkpoint.
///
/// The prefix — geometry, noise reduction, dehaze — is by far the expensive half, and a slider
/// move usually changes only one of its stages, or none. So each stage's output is cached
/// separately and the walk restarts at the deepest checkpoint still valid, running only what is
/// downstream of the change. See
/// `docs/superpowers/specs/2026-08-12-second-prefix-checkpoint-design.md`.
///
/// Returns the frame and **how many prefix stages actually ran**. The worker ignores the count;
/// it exists because the correctness tests would all still pass with the cache deleted, so
/// something has to assert that work is genuinely being skipped.
/// Where a coverage map is being built: the frame it spans, the slice of it wanted, and the rung.
///
/// ⚠ A params struct because `key`, `remember` and `get_or_build` all pass the same four values,
/// and `remember` reached clippy's argument limit. That is a fair complaint and this project's
/// standing answer to it is a struct, not an allowance — `FrameSpec`, `NameField`, `AspectLock`
/// and `GradeUi` are the precedents.
#[derive(Clone, Copy, PartialEq)]
struct CoverageSpec {
    fw: usize,
    fh: usize,
    region: Option<(usize, usize, usize, usize)>,
    max_dim: usize,
}

/// What one mask's cached coverage was built from.
#[derive(PartialEq)]
struct MaskCoverageKey {
    /// This component's brush. `None` for an analytic shape, which is never cached here.
    brush: Option<edit_model::Brush>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    /// The rendered region as `(x0, y0, w, h)` in frame pixels, or `None` for the whole frame.
    ///
    /// ⚠⚠ **In the key on purpose, and this is the OPPOSITE of [`PrefixCache`]'s rule.** That rule
    /// — region out of the key, so panning never invalidates — is right for the prefix, which runs
    /// whole-buffer and whose output *is* the frame. Copied here it costs a full-frame
    /// rasterization on every paint frame: measured at 21.62 ms against 12.32 clipped for a plain
    /// brush, and **80.81 against 42.55** for a gating one.
    ///
    /// ⚠⚠ And for a **gating** brush a whole-frame map cannot be built during a region render at
    /// all: the gate reads base pixels the region render never computes. Not merely uncached —
    /// unknowable. **Do not "unify" the two caches.**
    ///
    /// ⚠ It also makes a clip mismatch unrepresentable: a map clipped to one region handed to a
    /// `MaskStage` rendering another would sample `outside` across the difference — a mask that
    /// fades out partway across the image rather than an error.
    region: Option<(usize, usize, usize, usize)>,
    /// What produced the pixels this mask's gate compared against, or `None` for a brush that does
    /// not gate.
    ///
    /// ⚠⚠ **`Edits` truncated to the masks BEFORE this one, plus the rung.** Sound because
    /// everything upstream of this mask's base is in it: the prefix stages all live in `Edits`, so
    /// does the entire global tail, and so does every earlier mask's shape *and* adjustments.
    ///
    /// ⚠⚠ **And the truncation is what makes the cache worth having** — dropping this mask drops
    /// its own adjustments with it, so moving its exposure leaves the identity unchanged. That is
    /// the case a mask exists for and the only thing this buys: a **global** slider genuinely
    /// changes the base, and no cache can help it.
    ///
    /// ⚠ It fails toward a **miss**, [`PrefixKey`]'s rule. The dangerous direction is a stale hit:
    /// coverage rendered positionally while the canvas tint shows it stopping at an edge, which is
    /// the silent preview/export divergence this project has recorded more than any other.
    base: Option<(Edits, usize)>,
}

/// Rasterized brush coverage, **per mask**, so an unrelated slider drag does not re-rasterize.
///
/// ⚠⚠ **Per mask rather than per render, and the win depends on it.** One entry holding every
/// mask's map would be invalidated by a change to any one of them — including mask 0's own
/// adjustment invalidating mask 0's own coverage, which is exactly the case this exists to make
/// free.
///
/// ⚠ It misses on every frame *while painting*, and that is correct — the strokes really did
/// change, so there is nothing to reuse.
#[derive(Default)]
struct BrushCache {
    entries: Vec<(MaskCoverageKey, Arc<pipeline::brush::CoverageMap>)>,
}

impl BrushCache {
    /// Two entries per **component**, and the reason 2 is unchanged from
    /// [`PrefixCache::FINALS_CAP`]: two rungs are live at once, so one entry per component would
    /// be evicted on every zoom change and rebuilt coming back.
    ///
    /// ⚠ Per component rather than per mask — a composite of three brushes needs three rows kept,
    /// and counting masks would make a composite's own siblings evict each other on every render.
    ///
    /// ⚠ A `CoverageMap` is 8-bit and **bbox-bounded**, so a stroke over a quarter of a 1.5 Mpx
    /// region is ~390 KB. Four masks at two rungs is a few megabytes against the prefix cache's
    /// 512 MB budget.
    fn cap(edits: &Edits) -> usize {
        let components: usize = edits.masks.iter().map(|m| 1 + m.refine.len()).sum();
        2 * components.max(1)
    }

    /// The base identity for mask `i`, or `None` when this brush does not gate on the image.
    ///
    /// ⚠⚠ **No component index here, and that is not an omission.** Every component of mask `i`
    /// reads the *same* base — the buffer the stage was handed — because components change the
    /// composite's coverage, never its pixels. A component index in the base identity would be
    /// wrong rather than merely redundant.
    fn base_id(
        edits: &Edits,
        i: usize,
        brush: &edit_model::Brush,
        max_dim: usize,
    ) -> Option<(Edits, usize)> {
        if !brush.needs_base() {
            return None;
        }
        let mut upstream = edits.clone();
        upstream.masks.truncate(i);
        Some((upstream, max_dim))
    }

    /// The shape at component `j` of mask `i` — `0` is the base, `j > 0` the `j-1`th refinement.
    ///
    /// ⚠ One resolver, used by `key` and by `get_or_build`. Two derivations of "which shape is
    /// this component" is how the key and the thing it keys would come to disagree.
    fn mask_at(edits: &Edits, i: usize, j: usize) -> Option<&edit_model::Mask> {
        let m = edits.masks.get(i)?;
        match j.checked_sub(1) {
            None => Some(&m.mask),
            Some(k) => m.refine.get(k).map(|r| &r.mask),
        }
    }

    /// The key for component `j` of mask `i`.
    ///
    /// ⚠⚠ **There is deliberately NO component index in the key, and the sabotage step is what
    /// established that.** The plan added one, on the reasoning that a composite's base and its
    /// refinements are different coverage under one mask index — and forcing it to `(i, 0)` left
    /// every test passing, including the one written to catch exactly that. The reason nothing
    /// caught it is that there is nothing to catch: the key already holds **the brush itself**,
    /// plus the geometry, the frame size, the region and the base identity, which is the complete
    /// list of things a component's coverage is a function of. Two components with identical
    /// brushes therefore have identical coverage and *should* share one entry; an index would
    /// only prevent a legitimate hit.
    ///
    /// ⚠ `i` and `j` are still parameters because they say **which shape** this is — see
    /// [`Self::mask_at`] — and `i` alone decides the base identity.
    fn key(edits: &Edits, i: usize, j: usize, spec: CoverageSpec) -> MaskCoverageKey {
        let brush = match Self::mask_at(edits, i, j) {
            Some(edit_model::Mask::Brush(b)) => Some(b.clone()),
            _ => None,
        };
        let base = brush
            .as_ref()
            .and_then(|b| Self::base_id(edits, i, b, spec.max_dim));
        MaskCoverageKey {
            brush,
            geometry: edits.geometry,
            fw: spec.fw,
            fh: spec.fh,
            region: spec.region,
            base,
        }
    }

    /// Store coverage a stage reported for component `j` of mask `i`.
    fn remember(
        &mut self,
        edits: &Edits,
        spec: CoverageSpec,
        i: usize,
        j: usize,
        map: Arc<pipeline::brush::CoverageMap>,
    ) {
        let key = Self::key(edits, i, j, spec);
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, map));
        self.entries.truncate(Self::cap(edits));
    }

    fn get_or_build(&mut self, edits: &Edits, spec: CoverageSpec) -> Arc<pipeline::BrushCoverage> {
        // ⚠ One row per mask, one slot per component within it — index 0 being the mask's base
        // shape. The row exists even for a mask with no brush at all, so the stage's `(i, j)`
        // always addresses the slot the cache computed a key for.
        let mut maps: Vec<Vec<Option<Arc<pipeline::brush::CoverageMap>>>> =
            Vec::with_capacity(edits.masks.len());
        for (i, m) in edits.masks.iter().enumerate() {
            let mut row = Vec::with_capacity(1 + m.refine.len());
            for j in 0..=m.refine.len() {
                let key = Self::key(edits, i, j, spec);
                if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
                    // Promote the hit, so the least recently used entry is the one evicted.
                    let hit = self.entries.remove(pos);
                    row.push(Some(Arc::clone(&hit.1)));
                    self.entries.insert(0, hit);
                    continue;
                }
                // ⚠⚠ **A gating brush is not built here, and must not be.** Its coverage depends
                // on pixel values and this function has no image; building one positionally is
                // what would render it as a plain stroke while the canvas tint showed it stopping
                // at an edge. `MaskStage::apply` builds it where the buffer is and reports it back
                // through `BrushCoverage::deposit`; `render_cached` stores it with `remember`.
                let built = match Self::mask_at(edits, i, j) {
                    Some(edit_model::Mask::Brush(b)) if !b.needs_base() => {
                        Some(Arc::new(pipeline::brush::rasterize_in(
                            b,
                            &edits.geometry,
                            spec.fw,
                            spec.fh,
                            spec.region,
                            None,
                        )))
                    }
                    _ => None,
                };
                if let Some(built) = &built {
                    self.entries.insert(0, (key, Arc::clone(built)));
                    self.entries.truncate(Self::cap(edits));
                }
                row.push(built);
            }
            maps.push(row);
        }
        let built = maps
            .iter()
            .map(|row| row.iter().map(|_| std::sync::Mutex::new(None)).collect())
            .collect();
        Arc::new(pipeline::BrushCoverage { maps, built })
    }
}

/// What a render actually did — for the tests that cannot see it any other way.
///
/// ⚠⚠ Returned and **discarded in production**. `prefix_stages` already worked this way: the
/// correctness tests would all pass with the prefix cache deleted, so a count is the only thing
/// that can see it working. `bytes` is the same shape for M8.3g — the byte path and the float path
/// produce **identical** frames by design, so nothing in the output can distinguish them and only
/// an explicit observable can.
///
/// ⚠ Per render rather than a process-wide static: `cargo test` runs in parallel, and a global
/// counter reading someone else's renders is a contaminated observable this project has already
/// paid for once, in `GpuChain`'s transfer counts.
pub struct Ran {
    pub prefix_stages: usize,
    pub bytes: bool,
    /// How many prefix steps ran on the GPU.
    ///
    /// ⚠⚠ **An observable, because the two paths produce the same picture by design** — every
    /// equivalence test in the workspace passes against a seam that never fires, which is exactly
    /// how creative LUTs (W5) shipped fully tested and 100% dead. `prefix_stages` and `bytes`
    /// exist for the same reason. ⚠ Per render rather than a process-wide static: `cargo test`
    /// runs in parallel, and a global counter reading someone else's renders is a contaminated
    /// observable this project has already paid for once, in `GpuChain`'s transfer counts.
    pub gpu_prefix_steps: usize,
    /// Whether the prefix ran region-scoped from its resume point, rather than whole-buffer.
    ///
    /// ⚠⚠ **An observable, because the two paths produce the same picture BY DESIGN.** A
    /// region-scoped render and a whole-buffer render cropped are bit-identical, so every
    /// equivalence test in the workspace passes against a gate that never fires — which is exactly
    /// how creative LUTs (W5) shipped fully tested and 100% dead. `prefix_stages`, `bytes` and
    /// `gpu_prefix_steps` all exist for this reason.
    pub region_scoped: bool,
}

/// The largest share of the frame a margined region may occupy and still be worth scoping, in
/// percent.
///
/// ⚠⚠ **Swept, not chosen.** Two constants in this crate were reasoned about instead and both were
/// wrong: `MIN_GPU_BOX_PX` started low by ~8x, and `MAX_TILE_DABS` was sized by a *limit* rather
/// than by the data and measured 5x slower than the CPU it replaced.
///
/// ⚠ Above this share the scoped path costs roughly what the whole-buffer path costs **and its
/// output is deliberately not cached**, so the settle render repeats the work — strictly worse than
/// not scoping at all.
const SCOPE_MAX_AREA_PCT: u64 = 50;

#[allow(clippy::too_many_arguments)]
fn render_cached(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    spec: FrameSpec,
    source_scale: f32,
    lens: Option<lens_profile::LensShot>,
    wb_baseline: color::wb::Illuminant,
    profile: pipeline::ProfileRendering<'_>,
    cache: &mut PrefixCache,
    brush_cache: &mut BrushCache,
) -> (RenderedFrame, Ran) {
    // Destructured so the body below reads exactly as it did before these three were grouped.
    let FrameSpec {
        max_dim,
        region,
        display_space,
        resampler,
        gpu_tail,
        visualize,
        soft_proof,
        grade_view,
    } = spec;
    let buffer_long = buffer.width().max(buffer.height());
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );

    // The prefix never depends on the region, so the whole chain is built and cached without
    // one — which is why panning re-runs only the tail and never invalidates a checkpoint.
    // A missing profile — an unlisted lens, or a JPEG — skips the stage silently, which is the
    // ordinary case and needs no special-casing anywhere else.
    // ⚠⚠ The frame-global airlight, replayed from the SOURCE buffer at a fixed 256 px. It is a
    // deterministic function of the edits, so it is identical on the whole-buffer path and on a
    // region-scoped one — which is what lets the two agree by construction rather than
    // approximately, and what removes any pop when a gesture settles.
    //
    // ⚠ Computed unconditionally rather than only when dehaze is active: `prefix_airlight` runs
    // the prefix on a 65k-pixel copy, and a branch here would be a second place that has to agree
    // with `Dehaze::is_active`.
    let airlight = pipeline::prefix_airlight(edits, buffer, resampler, lens.clone());
    let steps = build_prefix_steps(edits, scale, lens, resampler, Some(airlight));
    // ⚠ Per-step decomposition, env-gated and kept: `RUSTROOM_DEBUG_MASK`'s precedent. A whole-
    // render figure says a drag is slow; only this says WHICH stage is slow, and reasoning about
    // that from a sidecar has already been wrong once here.
    let debug_steps = std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some();
    let key_at = |n: usize| (PrefixKey::upto(&steps, n, scale), max_dim);
    let tier_at = |n: usize| {
        if n == steps.len() {
            Tier::Final
        } else {
            Tier::Mid
        }
    };

    // Resume from the deepest checkpoint still held. `n == 0` is the bare downscale.
    let mut gpu_prefix_steps = 0usize;
    let mut resumed = None;
    for n in (0..=steps.len()).rev() {
        if let Some(img) = cache.get(&key_at(n)) {
            resumed = Some((n, img));
            break;
        }
    }

    let (start, mut img) = match resumed {
        Some(hit) => hit,
        None => {
            // ⚠ When nothing needs shrinking, `downscale_to` is a clone of a buffer the engine
            // already owns behind an `Arc`. Sharing it makes checkpoint 0 free in both time and
            // memory; caching it would cost a whole buffer (78 MB on an X-T4) to save 2.2 ms.
            let seed_is_free = max_dim == 0 || buffer_long <= max_dim;
            let seed = if seed_is_free {
                Arc::clone(buffer)
            } else {
                let s = Arc::new(buffer.downscale_to(max_dim));
                cache.insert(key_at(0), s.clone(), tier_at(0));
                s
            };
            (0, seed)
        }
    };

    // ⚠⚠ **THE REGION-SCOPED PREFIX, and the gate is DERIVED rather than a flag.** Engage when a
    // region is wanted and *every* step from the resume point onward accepts one. There is no drag
    // signal to plumb and therefore nothing that can disagree with anything else — the rule
    // `Session::geometry_mode` was deleted for. It follows that this speeds a cold entry into 1:1
    // as well as a drag: the gate asks about the work, not about the gesture.
    //
    // ⚠⚠ M8.6's version engaged only when *exactly the last* step needed re-running, on the
    // argument that step n-1's scoped output cannot feed step n because step n is a gather. That
    // is true of two gathers in a row and this prefix is never that — its gathers (lens
    // correction, perspective, geometry) all precede its locals (noise reduction, dehaze), so a
    // local needs only a margin and the first scoped step is a gather reading a whole cached
    // buffer. `region_plan` is what establishes that per chain rather than by assertion.
    //
    // ⚠ At the fitted rung `region` is `None`, so this never engages there — it engages exactly
    // at the native rungs, which is where the cost is.
    let mut scoped = None;
    // ⚠ A plain flag rather than something derived from the output's size afterwards: a derived
    // fact that can be wrong is what `Session::geometry_mode` was deleted for.
    let mut was_scoped = false;
    if let Some(want) = region {
        let visible = pipeline::Rect {
            x: want.x,
            y: want.y,
            w: want.w,
            h: want.h,
        };
        // ⚠ `region_plan` clamps `visible` against the frame itself, so no clamping is done here —
        // two clamps would be two rules about the same rect.
        if let Some(plan) = pipeline::region_plan(&steps, start, visible, img.width(), img.height())
        {
            // ⚠⚠ **The area gate.** A margined region close to the whole frame costs roughly what
            // the whole-buffer path costs *and is not cached*, so the settle render repeats the
            // work — strictly worse than not scoping. `out_rects[0]` is the earliest scoped output
            // and therefore the largest of them, which is what makes it the right proxy.
            let margined: u64 = plan
                .out_rects
                .first()
                .map_or(0, |r| r.w as u64 * r.h as u64);
            let whole = plan.frame.0 as u64 * plan.frame.1 as u64;
            if whole > 0 && margined * 100 < whole * SCOPE_MAX_AREA_PCT {
                if let Some(out) = pipeline::run_region_plan(&steps, start, &img, &plan) {
                    let last = *plan.out_rects.last().expect("a plan has at least one rect");
                    if debug_steps {
                        eprintln!(
                            "  prefix: {:?}  resuming at {start} of {}  SCOPED {}x{} -> {}x{} of {}x{}",
                            steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>(),
                            steps.len(),
                            plan.out_rects[0].w,
                            plan.out_rects[0].h,
                            last.w,
                            last.h,
                            plan.frame.0,
                            plan.frame.1,
                        );
                    }
                    // The step count rides in the tuple rather than in a separate local: a count
                    // stored beside the thing it counts is a second fact that can go stale.
                    scoped = Some((out, last, plan.frame, plan.out_rects.len()));
                    was_scoped = true;
                }
            }
        }
    }

    let (input, rect, pw, ph, prefix_stages) = if let Some((out, last, frame, ran)) = scoped {
        // ⚠⚠ **DELIBERATELY NOT CACHED.** A region-scoped checkpoint served for a *different*
        // region is the one failure here that shows as a plausible photograph rather than an
        // error, and leaving the insert out is what makes it unrepresentable rather than guarded.
        // The cost is that a later whole-buffer render rebuilds the final checkpoint — against
        // re-running every step whole-buffer on every frame of the drag, which is what this
        // replaces.
        //
        // ⚠⚠ `frame` comes from the **plan**, composed through `output_dims` and read before any
        // step ran. Taking it from `out` would be the region's own size, which rasterises every
        // brush mask at the wrong size and thrashes the coverage cache on every pan — M8.6's
        // sharpest hazard, and it is sharper here because the chain is longer.
        //
        // ⚠ `gpu_prefix_steps` stays 0 here. Nothing is lost that the seam could have supplied:
        // `accelerate_prefix_stage`'s twins run whole-buffer, so a scoped chain is the CPU's.
        (
            out,
            Some(RegionRect {
                x: last.x,
                y: last.y,
                w: last.w,
                h: last.h,
            }),
            frame.0,
            frame.1,
            ran,
        )
    } else {
        // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU seam
        // clean — so each step that runs clones the checkpoint it starts from. Measured at 0.5 ms
        // (fit) and 2.1 ms (native) against the 13–117 ms each avoided stage costs.
        if debug_steps {
            eprintln!(
                "  prefix: {:?}  resuming at {start} of {}",
                steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>(),
                steps.len()
            );
        }
        for (i, step) in steps.iter().enumerate().skip(start) {
            let t_step = std::time::Instant::now();
            // ⚠⚠ **The prefix's GPU seam, and it must be here rather than where `steps` is built.**
            // `dehaze::airlight` is estimated from the image the stage is HANDED — the checkpoint
            // after geometry and noise reduction — so substituting up front would estimate it from
            // the raw buffer, before the crop. A slightly different airlight is a different
            // photograph, not an error.
            //
            // ⚠ The step's `key` is never touched: `PrefixStepKey` is derived from the edits, so a
            // checkpoint written by either path is a legitimate hit for the other and `PrefixCache`
            // needs no change at all.
            let accelerated = if gpu_tail {
                gpu_pipeline::shared().and_then(|ctx| {
                    gpu_pipeline::accelerate_prefix_stage(
                        step.stage.name(),
                        edits,
                        scale,
                        &img,
                        ctx,
                    )
                })
            } else {
                None
            };
            img = Arc::new(match accelerated {
                Some(g) => {
                    gpu_prefix_steps += 1;
                    g.apply((*img).clone())
                }
                None => step.stage.apply((*img).clone()),
            });
            if debug_steps {
                eprintln!(
                    "    [{i}] {:>16} {:8.1} ms   -> {}x{} ({:.1} Mpx)",
                    step.stage.name(),
                    t_step.elapsed().as_secs_f64() * 1000.0,
                    img.width(),
                    img.height(),
                    (img.width() * img.height()) as f64 / 1e6,
                );
            }
            cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
        }
        let ran = steps.len() - start;

        // ⚠ The complete prefix output must always live in the protected tier, even when we
        // resumed at it without running anything. A prefix that *shrinks* — its last stage going
        // neutral — turns a former intermediate into the final, and leaving it in the budgeted
        // tier would let a large render evict the very thing the zoom guarantee rests on.
        // `steps.is_empty()` is excluded because there the final *is* checkpoint 0, which is
        // either free (and must not be stored) or was already inserted as a final above.
        if ran == 0 && !steps.is_empty() {
            cache.insert(key_at(steps.len()), img.clone(), Tier::Final);
        }

        // The region can only be resolved once the prefix output's dimensions are known, which is
        // why the tail is built here rather than alongside the steps. Building is a few Box
        // allocations.
        let (pw, ph) = (img.width(), img.height());
        let rect = region.and_then(|r| r.clamped(pw, ph));
        let input = match rect {
            Some(r) => img.crop(r.x, r.y, r.w, r.h),
            None => (*img).clone(),
        };
        (input, rect, pw, ph, ran)
    };
    let region_scoped = was_scoped;

    // ⚠⚠ **The frame dimensions are `(pw, ph)` — the prefix output's — and on the region-scoped
    // path they come from `output_dims` rather than from the buffer.** That buffer *is* the region
    // there, so taking its size would rasterise every brush mask at the region's size and thrash
    // the coverage cache on every pan.
    let region = rect.map(|r| Region {
        full_w: pw,
        full_h: ph,
        x0: r.x,
        y0: r.y,
    });

    // ⚠ The frame dimensions are the PREFIX OUTPUT's — `(pw, ph)` — not the region's. That image
    // *is* the frame. Passing the region's would rasterize at the wrong size and, because the dims
    // are in the key, would also thrash the cache on every pan.
    // ⚠ The clip comes from `rect`, which carries the region's own width and height. `Region`
    // holds only the origin plus the full frame's size, so deriving the size as "frame minus
    // origin" would clip to the bottom-right corner of the frame rather than to the viewport —
    // larger than intended, and silently so, because the output would still be correct and merely
    // slow.
    let clip = rect.map(|r| (r.x, r.y, r.w, r.h));
    let cov_spec = CoverageSpec {
        fw: pw,
        fh: ph,
        region: clip,
        max_dim,
    };
    let coverage = brush_cache.get_or_build(edits, cov_spec);
    // ⚠⚠ **M8.3g's gate, derived ONCE and passed down rather than re-derived.** Three things
    // downstream need the float image, and each is a view mode or a setting:
    //
    //   1. a **non-sRGB display space** — `pack` converts from the float deliberately, because an
    //      8-bit → 8-bit conversion costs roughly 0.8 bits and bands a smooth sky. ⚠ Opt-in and
    //      Linux/Windows-only: on macOS `tag_as_srgb` succeeds, so `effective_display_space`
    //      returns `Srgb` whatever the dropdown says.
    //   2. **Visualize Spots** — `edge_image` is computed from the float.
    //   3. **soft proofing** — the ProPhoto run is decoded back to the working space to test the
    //      gamut, which an 8-bit round trip cannot support.
    //
    // ⚠ The **grade range view** is a fourth and needs nothing: it bypasses `accelerate` entirely,
    // building its own halves through `build_pipeline_split_tapped`.
    //
    // ⚠ A four-part condition spanning two crates is the shape this project's record says goes
    // wrong silently, so the other half — "is this chain last, and does it end in
    // `output_transform`" — is answered by construction inside `accelerate` rather than here.
    let float_needed =
        display_space != color::OutputSpace::Srgb || visualize.is_some() || soft_proof.is_some();
    let build_tail = |space| {
        // ⚠ Hoisted into a local because `accelerate` needs it too: `output_transform` is built
        // from the render's output space rather than from `Edits`, and two constructions of one
        // spec is how the pipeline and its accelerated twin would come to disagree about which
        // space they are rendering into.
        let render_spec = pipeline::RenderSpec {
            space,
            // ⚠ The tail resamples nothing — every resampling stage is in the prefix — so this
            // is carried for consistency rather than because it changes a pixel here.
            resampler,
            // ⚠ Likewise: dehaze is a PREFIX stage, so the tail never builds one. Carried so the
            // two specs cannot drift if a stage ever moves.
            airlight: Some(airlight),
            baseline: wb_baseline,
        };
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            scale,
            region,
            Some(&coverage),
            render_spec,
            // Lens correction lives in the *prefix*, which this call discards — only the tail is
            // wanted here. `None` is therefore correct rather than provisional.
            None,
            // ⚠ The display rendering, by contrast, lives in the **tail** — so this must be
            // carried, or every render would silently take the built-in curve and the built-in
            // baseline while a profile that declares its own sat unused.
            profile,
        );
        // ⚠⚠ The tail only. Every accelerated stage is a tail stage, and accelerating the prefix
        // would make `PrefixKey` describe two different computations under one key.
        //
        // ⚠ `shared()` returning `None` silently keeps the CPU path: a machine with no adapter
        // must not fail to render.
        //
        // ⚠⚠ **`if` rather than `match (gpu_tail, shared())`, and this was a real bug rather than
        // style.** A tuple scrutinee evaluates **both** elements before matching, so `shared()`
        // ran even with the setting OFF — acquiring an adapter on the render worker, which costs
        // ~42 s in a debug build. Three render tests began reporting "no frame produced" after a
        // one-second wait. With the setting off nothing GPU-related may be touched at all.
        if gpu_tail {
            match gpu_pipeline::shared() {
                Some(ctx) => {
                    gpu_pipeline::accelerate(tail, edits, scale, region, profile, render_spec, ctx)
                }
                None => (tail, None),
            }
        } else {
            (tail, None)
        }
    };

    // ⚠⚠ **While proofing the tail runs TWICE, and that was MEASURED into existence rather than
    // argued.** A single ProPhoto run was the first design; run over 12 real X-T4 frames, 12 of 12
    // histograms moved — worst **five 8-bit levels** with the camera rendering off, because
    // ProPhoto's `encode` ceilings genuine scene headroom at 1.0 *before* the sRGB matrix mixes the
    // channels, where the direct path clamps only afterwards. So the histogram comes from an
    // unmodified sRGB run and is bit-identical **by construction**, not by luck, and the texture
    // and the warning come from a ProPhoto run that reaches `pack` near-unclipped.
    //
    // ⚠ Cost: one extra tail run per proofed frame — ~7 ms at fit, ~13 ms at a native rung —
    // paid only while the proof is switched on. That is accepted; it is not something to optimise
    // away by collapsing the two runs back into one.
    //
    // ⚠ Suppressed while Visualize Spots is on. That view replaces the photograph with an
    // achromatic edge view, so proofing it would mark nothing and show nothing; suppressing it
    // here also means the extra run is never paid for a frame that could not use it. The two are
    // both view modes applied at this point and the diagnostic one wins.
    let proof = soft_proof.filter(|_| visualize.is_none());
    // ⚠ `input` is cloned **only** when proofing. With the proof off there is one tail, one run,
    // and no clone — bit-exactly the path that existed before this feature.
    let proofed = proof.map(|p| {
        // ⚠ Rejoined at once: the proof is one of the three things that needs the float, so its
        // tail is never a candidate for the byte path.
        let (pipe, chain) = build_tail(color::OutputSpace::ProPhoto);
        (gpu_pipeline::rejoin(pipe, chain).run(input.clone()), p)
    });
    // ⚠ The **pipeline** is always sRGB here, whatever the proof does. The display transform is a
    // post-step in `pack`, deliberately outside the pipeline so the histogram stays image-referred.
    //
    // ⚠⚠ The grade range view taps the tail at the grading stage's INPUT, not `pack`'s output:
    // the ranges are selected on Oklab L before the grade's own luminance offsets, the masks,
    // the display rendering S-curve and the creative LUT move it. `None` is the path that
    // existed before this — one tail, one run, no clone. `Some` runs the same stages in the same
    // order with one read between two halves, so the frame's pixels are identical too
    // (`the_grade_view_does_not_touch_the_photograph`).
    let (out, plane) = match grade_view {
        Some(range) => {
            let (_, halves) = pipeline::build_pipeline_split_tapped(
                edits,
                scale,
                region,
                Some(&coverage),
                pipeline::RenderSpec {
                    space: color::OutputSpace::Srgb,
                    resampler,
                    baseline: wb_baseline,
                    airlight: Some(airlight),
                },
                None,
                profile,
            );
            let mid = halves.before_grade.run(input);
            // ⚠ Blending and Balance are read from the edits whether or not the grade is
            // active: the ranges exist before a wheel is turned.
            let g = &edits.color_grading;
            let plane = crate::grade_view::coverage(&mid, range, g.blending, g.balance);
            (
                Packed::Float(halves.from_grade.run(mid)),
                Some(Arc::new(plane)),
            )
        }
        None => {
            let (pipe, chain) = build_tail(color::OutputSpace::Srgb);
            // ⚠⚠ **M8.3g: the tail's last chain quantizes on the GPU and downloads 27 MB instead
            // of 82.** Measured at rung 3200: 6.33 ms of copy+map plus a 6.87 ms `to_vec` become
            // 2.59 + 2.19, and `pack`'s own `to_rgba8` disappears rather than moving.
            match chain {
                Some(c) if !float_needed => {
                    let mid = pipe.run(input);
                    let (w, h) = (mid.width(), mid.height());
                    match c.run_rgba8(&mid) {
                        Some(b) => (Packed::Rgba8 { bytes: b, w, h }, None),
                        // ⚠ A GPU failure falls back through `apply`, which retries once and then
                        // takes the CPU path. One wasted attempt on a rare path beats a second
                        // fallback implementation here that could drift from the first.
                        // ⚠ Fully qualified rather than importing `PipelineStage` into this
                        // module: this is the only place that needs it, and the import would sit
                        // in scope for a whole file that deliberately talks in pipelines.
                        None => (Packed::Float(pipeline::PipelineStage::apply(&c, mid)), None),
                    }
                }
                other => (
                    Packed::Float(gpu_pipeline::rejoin(pipe, other).run(input)),
                    None,
                ),
            }
        }
    };
    // ⚠⚠ **Harvest what the stages built, after every tail has run.** A gating brush's coverage
    // can only be rasterized where the image is, inside `MaskStage::apply`, so this is the only
    // moment the cache can see it. One point covers all three paths — the proofed tail above, the
    // grade-view split and the plain one — because they have all run by here.
    //
    // ⚠ `take_built` clears each slot, so a second harvest would find nothing: a path that
    // deposits twice cannot store a map twice, and a path that is missed loses the win silently
    // rather than caching something wrong. That is the safe direction of the two.
    for (i, m) in edits.masks.iter().enumerate() {
        for j in 0..=m.refine.len() {
            if let Some(map) = coverage.take_built(i, j) {
                brush_cache.remember(edits, cov_spec, i, j, map);
            }
        }
    }
    // ⚠ **Derived** from what was actually produced rather than tracked alongside it. Two facts
    // that can disagree is the shape `render_max_dim` and `Session::geometry_mode` were each
    // fixed for; here there is only one.
    let bytes = matches!(out, Packed::Rgba8 { .. });
    let mut frame = pack(out, proofed, region, max_dim, display_space, visualize);
    frame.grade_view = plane;
    // ⚠ `edits` here is already the **view** edits — `Session::view_edits` has run
    // `preview_edits`, which is what resets the crop in geometry mode — so this is the geometry
    // the render was actually asked for, which is what `fit_size` requires.
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(buffer_long, crate::view_region::PREVIEW_MAX_DIM),
        &edits.geometry,
    );
    (
        frame,
        Ran {
            prefix_stages,
            gpu_prefix_steps,
            region_scoped,
            bytes,
        },
    )
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    display_space: color::OutputSpace,
    resampler: pipeline::Resampler,
    lens: Option<lens_profile::LensShot>,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    // ⚠⚠ **The same airlight definition `render_cached` uses, from the same source buffer.** This
    // function is the reference the cached path is tested against, so a second definition here
    // would make `every_cached_frame_matches_a_cold_render` fail — which is exactly how this was
    // caught, at one 8-bit level scattered across the frame.
    //
    // ⚠ `lens` is a parameter rather than `None` for that reason: "exact whenever the engine
    // happens to be lens-less" is a coincidence, not a guarantee, and this is a reference.
    let spec = pipeline::RenderSpec {
        space: color::OutputSpace::Srgb,
        resampler,
        baseline: color::wb::Illuminant::default(),
        airlight: Some(pipeline::prefix_airlight(edits, buffer, resampler, lens)),
    };
    let mut frame = pack(
        Packed::Float(pipeline::build_pipeline_spec(edits, scale, spec).run(preview)),
        // ⚠ Never proofed. This is the before/after baseline and the reference the cached path is
        // tested against; a proof is a property of the *view* and would make the reference move
        // with a menu setting.
        None,
        None,
        max_dim,
        display_space,
        None,
    );
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(
            buffer.width().max(buffer.height()),
            crate::view_region::PREVIEW_MAX_DIM,
        ),
        &edits.geometry,
    );
    frame
}

/// The Visualize Spots view of a rendered frame: the edge view of its luminance, written back as
/// grey.
///
/// ⚠ Taken on the **encoded** output, not linear light. This is a view aid, not a measurement,
/// and an edge in encoded values is closer to what the eye reads as an edge — which is what
/// makes dust findable. `color::luminance` is only a weighted sum here.
fn edge_image(out: &ImageBuf, threshold: f32) -> ImageBuf {
    let (w, h) = (out.width(), out.height());
    let luma: Vec<f32> = out
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| color::luminance(*p))
        .collect();
    let edges = crate::visualize::edge_view(&luma, w, h, threshold);
    let mut data = Vec::with_capacity(edges.len() * 3);
    for e in edges {
        data.extend_from_slice(&[e, e, e]);
    }
    ImageBuf::from_vec(w, h, data)
}

#[cfg(test)]
mod tests {
    // ⚠ Only the tests name this type — the lib drives the cache through its methods — so the
    // import is scoped here rather than at file level, where it would be dead under -D warnings.
    use crate::prefix_cache::CacheKey;

    /// `scale_from_full` is the engine's own `render_scale`, phrased from the invariant edge.
    ///
    /// ⚠⚠ The two must agree **on both sides of the full-resolution handover**, which is the
    /// whole reason the second phrasing exists: `buffer_long` doubles and `source_scale` halves
    /// at the upgrade, while `full_long` does not move. A caller that derives from `full_long`
    /// therefore cannot be wrong by 2x for having missed the handover — that bug is
    /// unrepresentable rather than merely tested.
    #[test]
    fn scale_from_full_agrees_with_the_engines_own_scale() {
        // (buffer_long, source_scale) before and after a RAW's handover, same photo.
        for (buffer_long, source_scale) in [(3123usize, 0.5f32), (6246, 1.0)] {
            let full_long = (buffer_long as f32 / source_scale).round() as usize;
            for max_dim in [1600usize, 3200, 6246] {
                let rendered = rendered_long_edge(buffer_long, max_dim);
                let engine = render_scale(buffer_long, rendered, source_scale);
                let derived = scale_from_full(full_long, buffer_long, max_dim);
                assert!(
                    (engine.get() - derived.get()).abs() < 1e-6,
                    "disagreed at buffer={buffer_long} max_dim={max_dim}: \
                     engine {} vs derived {}",
                    engine.get(),
                    derived.get()
                );
            }
        }
    }
    use super::*;

    fn tiny() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Arc::new(img)
    }

    /// ⚠⚠ **That `FrameSpec.resampler` actually reaches `build_prefix_steps`.** Every other
    /// test in this file passes the same kernel on both sides of whatever it compares — a
    /// cached frame against a cold one, a region against the whole — so `render_cached`
    /// substituting a constant for `spec.resampler` leaves all of them green *and* leaves the
    /// setting inert. Requests being issued proves nothing about which kernel ran.
    #[test]
    fn the_frame_spec_resampler_reaches_the_render() {
        // ⚠ A hard edge, not flat grey: `tiny()` is uniform, and every kernel agrees exactly on
        // a uniform image. That fixture cannot see this bug.
        let mut img = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = if (x / 4) % 2 == 0 { 0.05 } else { 0.9 };
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(img);
        let edits = Edits {
            geometry: edit_model::Geometry {
                angle: 5.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let render = |k| {
            render_cached(
                &buf,
                &edits,
                FrameSpec {
                    max_dim: 32,
                    region: None,
                    display_space: color::OutputSpace::Srgb,
                    resampler: k,
                    gpu_tail: false,
                    visualize: None,
                    soft_proof: None,
                    grade_view: None,
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                // ⚠ A fresh cache each time, or the second call would legitimately hit the
                // first's checkpoint and the test would be asserting about caching instead.
                pipeline::ProfileRendering::default(),
                &mut PrefixCache::default(),
                &mut BrushCache::default(),
            )
            .0
            .rgba
        };
        assert_ne!(
            render(pipeline::Resampler::Bilinear),
            render(pipeline::Resampler::Lanczos3),
            "the kernel in the spec never reached the pipeline"
        );
    }

    fn wait_for_frame(engine: &RenderEngine) -> RenderedFrame {
        for _ in 0..200 {
            if let Some(f) = engine.poll_latest() {
                return f;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no frame produced");
    }

    /// A buffer with varied content, so a stale cache actually shows up as different bytes.
    /// (The flat gray `tiny()` would hide almost any staleness.)
    ///
    /// ⚠ Rectangular, because a square fixture cannot see a width/height swap — the class of bug
    /// `every_prefix_stages_output_dims_match_what_it_produces` caught in `pipeline` with
    /// "`geometry` predicted (37, 23) and produced (23, 37)".
    fn varied_rect(w: usize, h: usize) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(w, h);
        let n = w.max(h).max(1);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / n as f32,
                        y as f32 / n as f32,
                        ((x + y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        Arc::new(img)
    }

    fn varied_sized(n: usize) -> Arc<ImageBuf> {
        varied_rect(n, n)
    }

    fn varied() -> Arc<ImageBuf> {
        varied_sized(16)
    }

    /// A deliberately non-square fixture: 43 rows do not survive a halving cleanly, so
    /// `downscale_to`'s rounding is observable between rungs. A square one could not see it.
    fn oblong() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(64, 43);
        for (i, s) in img.data_mut().iter_mut().enumerate() {
            *s = (i % 251) as f32 / 251.0;
        }
        Arc::new(img)
    }

    /// ⚠⚠ **The property the shipped code fails.** `fit_image_size` is what every overlay is
    /// placed against, and it must not depend on which rung last landed. Rendering the same photo
    /// at two rungs must report the same fit size.
    ///
    /// ⚠ Non-square on purpose. At rung 16 the 43 rows round to 11, and scaling that back up by
    /// `fit_cap / max_dim` gives 44 against the true 43 — which is the bug, one rung down, in
    /// miniature.
    #[test]
    fn the_fit_size_is_the_same_at_every_rung() {
        let buf = oblong();
        let at = |max_dim: usize| {
            render_once(
                &buf,
                &Edits::default(),
                max_dim,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .fit_size
        };
        let lo = at(16);
        let hi = at(64);
        assert!(
            (lo.0 - hi.0).abs() < 1e-3 && (lo.1 - hi.1).abs() < 1e-3,
            "the fit size was {lo:?} at one rung and {hi:?} at another — it is being derived \
             from the rendered frame's rounded dimensions"
        );
    }

    // ─────────────── M8.6: the region-scoped prefix ───────────────

    /// A straighten plus a vignette.
    ///
    /// ⚠⚠ **The vignette is what makes this fixture able to see the §5 hazard at all.** It is the
    /// one tail stage whose result depends on WHERE a pixel is, so a `Region` carrying the
    /// region's own size instead of the frame's re-centres the falloff and the bytes differ.
    /// Without it the frame dimensions could be taken off the region-scoped buffer and every byte
    /// would still match — the test would name the hazard and be unable to detect it.
    fn scoped_edits() -> Edits {
        Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    fn scoped_spec() -> FrameSpec {
        FrameSpec {
            region: Some(RegionRect {
                x: 3,
                y: 2,
                w: 9,
                h: 7,
            }),
            ..FrameSpec::fit(24)
        }
    }

    fn render_with(
        buf: &Arc<ImageBuf>,
        e: &Edits,
        spec: FrameSpec,
        cache: &mut PrefixCache,
        bc: &mut BrushCache,
    ) -> (RenderedFrame, Ran) {
        render_cached(
            buf,
            e,
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            cache,
            bc,
        )
    }

    /// ⚠⚠ **The acceptance for the whole milestone, end to end.** The region-scoped prefix and the
    /// whole-buffer-then-crop path must produce the same frame **byte for byte** — they are the
    /// same gather in the same order, so a tolerance would hide exactly what this exists to catch.
    ///
    /// ⚠ The two paths are forced apart by the CACHE, not by a flag: a warm final checkpoint means
    /// nothing is left to run, so the gate cannot fire and the region is taken by `img.crop`.
    #[test]
    fn a_region_scoped_render_matches_the_whole_buffer_path_byte_for_byte() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(24), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache with one step left must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        assert_eq!(
            via_crop.rgba, via_region.rgba,
            "the two paths must agree byte for byte"
        );
    }

    /// ⚠⚠ The gate itself. Every pixel test passes against a gate that never fires, because the
    /// two paths are bit-identical by design — this is what says it fired.
    #[test]
    fn a_drag_of_the_last_prefix_step_renders_only_the_region() {
        let buf = varied_sized(24);
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let spec = scoped_spec();

        let (_, cold) = render_with(&buf, &scoped_edits(), spec, &mut cache, &mut bc);
        assert_eq!(cold.prefix_stages, 1, "fixture check: one prefix step");

        // A Straighten drag: the same prefix, a different angle. Exactly the last step re-runs.
        let dragged = Edits {
            geometry: edit_model::Geometry {
                angle: 3.4,
                ..Default::default()
            },
            ..scoped_edits()
        };
        let (frame, ran) = render_with(&buf, &dragged, spec, &mut cache, &mut bc);
        assert!(ran.region_scoped, "the drag must render only the region");
        assert_eq!(ran.prefix_stages, 1);
        assert_eq!((frame.width, frame.height), (9, 7));
    }

    /// ⚠⚠ A region-scoped checkpoint served for a *different* region is the one failure that shows
    /// as a plausible photograph. Not inserting it is what makes that unrepresentable — and the way
    /// to see it is that a second identical render must scope **again** rather than find a final.
    #[test]
    fn a_region_scoped_render_leaves_no_final_checkpoint() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, first) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            first.region_scoped,
            "fixture check: the gate must have fired"
        );
        let (_, second) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            second.region_scoped,
            "no final checkpoint may have been stored, so the next render scopes again"
        );
        assert_eq!(second.prefix_stages, 1);
    }

    // ───────────────── M8.7: the whole prefix scoped, from its resume point ─────────────────

    /// A **two-step** prefix: a gather then a local, which is exactly the shape M8.6 declared
    /// impossible to chain.
    ///
    /// ⚠⚠ `smoothness: 0.0`, not the default 50, and it is load-bearing. The chroma radius runs
    /// 8..32 full-resolution pixels and its reach is **two passes** of it, so a defaulted
    /// smoothness margins a 24x18 region out to roughly the whole of a 128x96 fixture — past
    /// `SCOPE_MAX_AREA_PCT`, where the gate **correctly** refuses. The test would then be green
    /// and blind to everything it is named for.
    fn chained_edits() -> Edits {
        Edits {
            geometry: edit_model::Geometry {
                angle: 4.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 40.0,
                detail: 50.0,
                smoothness: 0.0,
            },
            ..Edits::default()
        }
    }

    fn chained_spec() -> FrameSpec {
        FrameSpec {
            region: Some(RegionRect {
                x: 50,
                y: 36,
                w: 24,
                h: 18,
            }),
            ..FrameSpec::fit(128)
        }
    }

    /// ⚠⚠ **THE CARRIER FOR M8.7, and without it the milestone ships dead.** A scoped render and a
    /// whole-buffer one produce the same picture **by design**, so every correctness test in the
    /// workspace — all 63 goldens included — passes with the gate deleted. That is precisely how
    /// creative LUTs (W5) shipped fully tested and 100% non-functional.
    ///
    /// ⚠ `prefix_stages` is asserted at **2**, not merely "scoped": M8.6's gate also sets
    /// `region_scoped`, so a test asking only that would pass against the single-step version this
    /// milestone replaces — which is the version that did nothing on the reporter's own chain.
    #[test]
    fn a_chain_of_prefix_steps_renders_only_the_region() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut cache = PrefixCache::default();
        let (frame, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            ran.region_scoped,
            "a region render whose whole chain accepts one must scope"
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "the scoped chain must cover BOTH steps, not just the last"
        );
        assert_eq!((frame.width, frame.height), (24, 18));

        // ⚠ And it must decline with no region at all — the fitted rung, where scoping would be
        // meaningless and where a gate that fired anyway would be producing a crop nobody asked
        // for.
        let mut cold = PrefixCache::default();
        let (whole, ran) = render_with(&buf, &e, FrameSpec::fit(128), &mut cold, &mut bc);
        assert!(!ran.region_scoped, "no region means no scoping");
        assert_eq!((whole.width, whole.height), (128, 96));
    }

    /// ⚠⚠ The engine-level acceptance for a **chain**: the scoped path and the whole-buffer path
    /// cropped must agree. The existing byte-for-byte test covers a one-step chain; only this one
    /// can see a margin that is right for a single step and wrong once composed — which is a wrong
    /// **band at one edge of the region**, a plausible photograph rather than an error.
    ///
    /// ⚠ **Within one 8-bit level, not bit-identical, and that is a property of the chain rather
    /// than a tolerance chosen for comfort.** Noise reduction's box blur keeps a running sum, so
    /// each position is reached by a different history of adds and subtracts and the last bits
    /// differ — `a_chain_with_a_local_agrees_to_within_a_rounding` measures that at one ulp. A
    /// chain of gathers alone *is* byte-identical, and `a_chain_of_gathers_is_byte_for_byte` is
    /// where that standard is held.
    ///
    /// ⚠ The two paths are forced apart by the CACHE rather than by a flag: a warm final
    /// checkpoint leaves nothing to run, so the gate cannot fire and the region is taken by
    /// `img.crop`.
    #[test]
    fn a_chained_region_render_agrees_with_the_whole_buffer_path() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(128), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        let worst = via_crop
            .rgba
            .iter()
            .zip(via_region.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "the two paths must agree to within a rounding, worst {worst} levels"
        );
    }

    /// ⚠⚠ **The caveat as a behaviour rather than as a sentence in a document.** A masked capture
    /// correction runs **last** in the prefix and declines a region by construction — it inherits
    /// `PipelineStage::input_rect_for`'s `None`. One decline abandons the whole plan, because a
    /// step that must run whole-buffer needs a whole-buffer input and therefore so does every step
    /// above it. So a photograph carrying a masked dehaze or a masked noise reduction gets no
    /// scoping at all, and that is recorded here so a later reader meets it as a decision.
    #[test]
    fn a_masked_capture_switches_scoping_off_for_that_photo() {
        let buf = varied_rect(128, 96);
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut plain = PrefixCache::default();
        let (_, before) = render_with(&buf, &chained_edits(), spec, &mut plain, &mut bc);
        assert!(
            before.region_scoped,
            "fixture check: without the mask this photo does scope"
        );

        let mut e = chained_edits();
        e.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 30.0 },
                ..Default::default()
            },
        });

        let mut cache = PrefixCache::default();
        let (_, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            !ran.region_scoped,
            "a masked capture correction declines a region, which abandons the whole plan"
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "fixture check: the mask really added a prefix step"
        );
    }

    /// Every prefix stage active, so a resume point is observable at each depth.
    fn full_prefix() -> Edits {
        Edits {
            exposure: 0.5,
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 40.0 },
            ..Edits::default()
        }
    }

    // ─────────────────────────── M8.3g: the 8-bit readback ───────────────────────────

    /// A tail worth accelerating: `hsl` alone clears the cost gate, so the tail becomes one chain
    /// ending in `output_transform` — the shape the byte path needs.
    fn hsl_edits() -> Edits {
        let mut e = Edits::default();
        e.hsl.bands[0] = edit_model::HslBand {
            hue: 20.0,
            saturation: 10.0,
            luminance: 5.0,
        };
        e
    }

    fn byte_spec() -> FrameSpec {
        FrameSpec {
            gpu_tail: true,
            ..FrameSpec::fit(8)
        }
    }

    fn rendered(spec: FrameSpec) -> (RenderedFrame, Ran) {
        render_cached(
            &varied(),
            &hsl_edits(),
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
    }

    /// A hazy buffer large enough for dehaze's guided reach, which is **twice** its radius.
    fn hazy_buffer() -> Arc<ImageBuf> {
        let (w, h) = (200usize, 160);
        let a = [0.85f32, 0.87, 0.9];
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let dark = x % 40 < 6 && y % 40 < 6;
                let clear = if dark {
                    [0.01, 0.01, 0.01]
                } else {
                    [0.05 + 0.7 * u, 0.05 + 0.6 * v, 0.05 + 0.5 * (1.0 - u)]
                };
                let t = 0.95 - 0.9 * u;
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = clear[c] * t + a[c] * (1.0 - t);
                }
                img.set_pixel(x, y, px);
            }
        }
        Arc::new(img)
    }

    fn dehaze_edits() -> Edits {
        Edits {
            dehaze: edit_model::Dehaze { amount: 70.0 },
            ..Edits::default()
        }
    }

    fn rendered_hazy(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &dehaze_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE CARRIER, and without it every test in `gpu-pipeline` passes against a seam
    /// `render_cached` never calls.** The prefix seam changes no pixel a test can read — the two
    /// paths render the same picture by design — so only an explicit count can see it. That
    /// asymmetry is exactly how creative LUTs (W5) shipped fully tested and 100% dead.
    #[test]
    fn a_dehaze_prefix_step_runs_on_the_gpu() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered_hazy(true, &mut PrefixCache::default());
        assert_eq!(
            ran.prefix_stages, 1,
            "fixture check: there must be exactly one prefix step to accelerate"
        );
        assert_eq!(ran.gpu_prefix_steps, 1, "dehaze must have run on the GPU");
    }

    /// ⚠ The other direction, and it is not symmetry: without it a seam that fired
    /// **unconditionally** — ignoring the setting entirely — would pass the carrier above.
    #[test]
    fn with_the_setting_off_no_prefix_step_runs_on_the_gpu() {
        let (_, ran) = rendered_hazy(false, &mut PrefixCache::default());
        assert_eq!(ran.prefix_stages, 1, "fixture check: one prefix step");
        assert_eq!(ran.gpu_prefix_steps, 0);
    }

    /// ⚠⚠ **A checkpoint written by either path is a legitimate hit for the other**, which is what
    /// `PrefixStepKey` being derived from the **edits** buys — and it is a claim until something
    /// asserts it. Render once on the GPU, then once on the CPU **through the same cache**: the
    /// second must run no prefix step at all.
    #[test]
    fn a_checkpoint_written_by_either_path_is_a_hit_for_the_other() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_hazy(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_hazy(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    fn denoised_edits() -> Edits {
        Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        }
    }

    fn rendered_denoised(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &denoised_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **The same property for the SECOND stage across the seam, and it is a claim again.**
    /// `PrefixStepKey` is derived from the edits, never from the stage — so a checkpoint written
    /// by one path must be a legitimate hit for the other. The dangerous direction is a stale
    /// hit, which renders the previous frame's pixels: a plausible photograph, not an error.
    #[test]
    fn a_noise_reduction_checkpoint_is_shared_between_the_paths() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_denoised(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_denoised(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    /// ⚠⚠ **Without this, the three "keeps the float" tests below all pass against an
    /// implementation that never takes the byte path at all** — which is the failure mode that let
    /// creative LUTs ship fully tested and 100% dead.
    #[test]
    fn with_none_of_the_float_consumers_active_the_bytes_are_used() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered(byte_spec());
        assert!(ran.bytes, "the tail's last chain must download RGBA8");
    }

    /// ⚠ `pack` converts to a non-sRGB display space from the **float**, because an 8-bit → 8-bit
    /// conversion costs roughly 0.8 bits and bands a smooth sky.
    #[test]
    fn a_non_srgb_display_space_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            display_space: color::OutputSpace::DisplayP3,
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The edge view is computed from the float.
    #[test]
    fn visualize_spots_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            visualize: Some(12.0),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The ProPhoto run is decoded back to the working space to test the gamut, which an 8-bit
    /// round trip cannot support.
    #[test]
    fn a_soft_proof_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            soft_proof: Some(Proof {
                space: color::OutputSpace::Srgb,
                warn: false,
            }),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠⚠ **THE CARRIER.** Every test above can pass while the bytes never reach the texture. The
    /// two paths are designed to be indistinguishable in the output, so this asserts exactly that
    /// — and it is what would fail if the shader's quantization and `ImageBuf::quantize` ever
    /// diverged on a real frame rather than on the synthetic fixture in `gpu-pipeline`.
    #[test]
    fn the_byte_path_and_the_float_path_produce_the_same_frame() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (with_bytes, ran) = rendered(byte_spec());
        assert!(ran.bytes, "fixture check: this must be the byte path");
        let (with_float, ran) = rendered(FrameSpec {
            gpu_tail: false,
            ..byte_spec()
        });
        assert!(!ran.bytes, "fixture check: this must be the float path");
        assert_eq!(
            (with_bytes.width, with_bytes.height),
            (with_float.width, with_float.height)
        );
        // ⚠ The GPU's transcendentals diverge from the CPU's by well under an 8-bit level, so the
        // comparison is in levels rather than bit-exact — the bound `gpu-pipeline`'s own
        // equivalence tests establish. What it catches is a *wrong* quantization, which moves
        // whole levels.
        let worst = with_bytes
            .rgba
            .iter()
            .zip(with_float.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "texture differs by {worst} levels");
        assert!(
            Arc::ptr_eq(&with_bytes.rgba, &with_bytes.srgb),
            "on the byte path the texture and the photograph are one allocation"
        );
    }

    #[test]
    fn a_change_reruns_only_what_is_downstream_of_it() {
        // The feature itself. `every_cached_frame_matches_a_cold_render` proves the output is
        // right but would still pass with the cache deleted; this proves work is actually
        // being skipped, which is the entire point of the change.
        let buf = varied();
        let base = full_prefix();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, cold) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            cold.prefix_stages, 3,
            "a cold render runs every prefix stage"
        );

        let (_, hit) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(hit.prefix_stages, 0, "an unchanged prefix runs nothing");

        // A tail-only change still runs nothing: it resumes at the final checkpoint.
        let tail_only = Edits {
            exposure: -1.5,
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &tail_only,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "a tail change must not touch the prefix"
        );

        // Dehaze is last in the prefix, so only dehaze re-runs. This is the 86.8 ms case.
        let dehaze = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &dehaze,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 1,
            "a dehaze change must resume after noise reduction"
        );

        // Noise reduction is second, so NR and dehaze re-run but geometry does not.
        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 15.0,
                color: 15.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &nr,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "an NR change must resume after geometry"
        );

        // Geometry is first, so everything re-runs — but the downscale is still reused.
        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &geo,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "a geometry change invalidates the whole prefix"
        );
    }

    #[test]
    fn the_native_seed_shares_the_buffer_rather_than_copying_it() {
        // When nothing is downscaled, `downscale_to` is a clone of a buffer the engine already
        // holds behind an `Arc`. Caching that would cost 78 MB on an X-T4 to save 2.2 ms, so
        // the chain seeds from the Arc instead and stores no checkpoint 0.
        let buf = varied(); // 16 x 16
        let base = full_prefix();
        let mut bc = BrushCache::default();

        let mut native = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut native,
            &mut bc,
        );
        assert_eq!(
            native.mid_len(),
            2,
            "native should store only the post-geometry and post-NR checkpoints"
        );

        let mut fit = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut fit,
            &mut bc,
        );
        assert_eq!(
            fit.mid_len(),
            3,
            "fit should additionally store the downscale"
        );
    }

    #[test]
    fn a_shrinking_prefix_does_not_serve_the_wrong_checkpoint() {
        // Turning dehaze off makes the post-NR checkpoint the *final* one. Its key is the same
        // value it had as a mid, so the tiers must not end up holding it twice with the two
        // copies free to diverge.
        let buf = varied();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let with = full_prefix();
        render_cached(
            &buf,
            &with,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );

        let without = Edits {
            dehaze: edit_model::Dehaze::default(),
            ..with.clone()
        };
        let (got, ran) = render_cached(
            &buf,
            &without,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "the post-NR checkpoint is now the final one"
        );
        assert_eq!(
            got.rgba,
            render_once(
                &buf,
                &without,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .rgba
        );

        // ...and it must have been *promoted* into the protected tier, not left in the
        // budgeted one where a big render could evict the thing zooming back relies on.
        assert_eq!(cache.final_len(), 2);
        assert_eq!(cache.mid_len(), 2);
    }

    /// A distinct dummy image of `n` x `n`, for cache tests that never render.
    fn dummy(n: usize, v: f32) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(n, n);
        for y in 0..n {
            for x in 0..n {
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        Arc::new(img)
    }

    /// A cache key that differs only by `tag`, so tests can fill tiers without rendering.
    fn fake_key(tag: usize) -> CacheKey {
        let e = Edits {
            dehaze: edit_model::Dehaze {
                amount: 1.0 + tag as f32,
            },
            ..Edits::default()
        };
        (
            PrefixKey::new(&e, RenderScale::FULL, pipeline::Resampler::Bilinear),
            tag,
        )
    }

    #[test]
    fn the_mid_budget_evicts_the_least_recently_used() {
        // Each dummy is 4*4*3*4 = 192 bytes; a budget of 400 holds two.
        let mut cache = PrefixCache::with_mid_budget(400);
        cache.insert(fake_key(0), dummy(4, 0.1), Tier::Mid);
        cache.insert(fake_key(1), dummy(4, 0.2), Tier::Mid);
        cache.insert(fake_key(2), dummy(4, 0.3), Tier::Mid);

        assert_eq!(cache.mid_len(), 2, "the budget must bound the mids tier");
        // ⚠⚠ This tests the **tie-break**, not depth, and its comment used to claim otherwise:
        // *"insertion order within one render is shallow -> deep, so the most recent insert is the
        // deepest checkpoint; evicting the LRU is therefore evicting the shallowest."* `fake_key`
        // varies only the `max_dim` tag, so all three keys have the SAME depth — the fixture could
        // never exercise the story the comment told. Eviction is deepest-first now, and equal
        // depths still resolve to least-recently-used, which is what these three are.
        assert!(cache.get(&fake_key(2)).is_some(), "the newest survives");
        assert!(cache.get(&fake_key(1)).is_some());
        assert!(
            cache.get(&fake_key(0)).is_none(),
            "at equal depth the least recently used goes first"
        );
    }

    /// ⚠⚠ **The bug, as a unit test.** A drag resumes at the checkpoint *before* the stage it
    /// moves, and plain LRU evicted exactly that one: the render reads it, which promotes it to
    /// the front, and then its own later inserts push it to the back and out. At full resolution
    /// the budget holds a single 298 MiB checkpoint, so this happened on every frame.
    #[test]
    fn a_full_mid_tier_evicts_the_deepest_checkpoint_not_the_shallowest() {
        let e = full_prefix();
        let steps = pipeline::build_prefix_steps(
            &e,
            RenderScale::FULL,
            None,
            pipeline::Resampler::Bilinear,
            None,
        );
        assert!(
            steps.len() >= 3,
            "fixture check: a multi-step prefix is the whole point, got {}",
            steps.len()
        );
        let key = |n| (PrefixKey::upto(&steps, n, RenderScale::FULL), 0usize);
        // Each dummy is 4*4*3*4 = 192 bytes; a budget of 200 holds exactly one, which is the
        // full-resolution situation in miniature.
        let mut cache = PrefixCache::with_mid_budget(200);
        cache.insert(key(1), dummy(4, 0.1), Tier::Mid);
        cache.insert(key(2), dummy(4, 0.2), Tier::Mid);
        cache.insert(key(3), dummy(4, 0.3), Tier::Mid);

        assert_eq!(
            cache.mid_len(),
            1,
            "fixture check: the budget must hold exactly one"
        );
        assert!(
            cache.get(&key(1)).is_some(),
            "the shallowest must survive — it is the one a drag can still resume from"
        );
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_none());
    }

    /// ⚠⚠ **The symptom it was found as**: at a native rung every frame of a prefix drag restarted
    /// from zero, re-running lens correction each time — measured in the running app at
    /// **1730–2223 ms a frame** with `prefix_ran 5` on every one.
    #[test]
    fn a_prefix_drag_resumes_when_the_budget_holds_one_checkpoint() {
        let buf = varied_sized(16);
        // 16*16*3*4 = 3072 bytes a checkpoint, so 4000 holds exactly one — 298 MiB against a
        // 512 MB budget, in miniature.
        let mut cache = PrefixCache::with_mid_budget(4000);
        let mut bc = BrushCache::default();
        let base = full_prefix();
        let (_, cold) = render_with(&buf, &base, FrameSpec::fit(16), &mut cache, &mut bc);
        assert_eq!(
            cold.prefix_stages, 3,
            "fixture check: a cold render runs the whole prefix"
        );

        // ⚠ Drag NOISE REDUCTION rather than geometry: geometry is the FIRST prefix step in this
        // fixture, so moving it invalidates every checkpoint and there is nothing to resume from
        // whatever the policy. The bug is about a drag that HAS valid upstream work — which is
        // every real one, since lens correction is on by default and runs first.
        let dragged = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 61.0,
                color: 60.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_with(&buf, &dragged, FrameSpec::fit(16), &mut cache, &mut bc);
        assert!(
            ran.prefix_stages < 3,
            "the drag must resume at the checkpoint before noise reduction rather than restart, ran {}",
            ran.prefix_stages
        );
    }

    #[test]
    fn the_finals_tier_is_untouched_by_mid_pressure() {
        // The two tiers cannot evict each other: that is what preserves the zoom guarantee.
        // With a zero budget no mid survives, and both finals must still be there.
        let mut cache = PrefixCache::with_mid_budget(0);
        cache.insert(fake_key(10), dummy(4, 0.4), Tier::Final);
        cache.insert(fake_key(11), dummy(4, 0.5), Tier::Final);
        for tag in 0..5 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), 0);
        assert_eq!(cache.final_len(), 2);
        assert!(cache.get(&fake_key(10)).is_some());
        assert!(cache.get(&fake_key(11)).is_some());
    }

    #[test]
    fn the_finals_tier_holds_one_per_render_mode() {
        let mut cache = PrefixCache::default();
        for tag in 0..4 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Final);
        }
        assert_eq!(cache.final_len(), PrefixCache::FINALS_CAP);
        assert!(cache.get(&fake_key(3)).is_some());
        assert!(cache.get(&fake_key(2)).is_some());
        assert!(cache.get(&fake_key(0)).is_none());
    }

    #[test]
    fn the_mids_cap_bounds_stale_checkpoints() {
        // ⚠ Dragging Straighten mints a *new* post-geometry key every frame, so the mids tier
        // fills with checkpoints nobody will ever return to — bounded only by the byte budget,
        // which means a bigger budget retains more garbage. A render's useful chain is at most
        // 3 mids, so 6 keeps the current chain plus one previous state.
        let mut cache = PrefixCache::default();
        for tag in 0..20 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), PrefixCache::MIDS_CAP);
        assert!(
            cache.get(&fake_key(19)).is_some(),
            "the newest must survive"
        );
        assert!(cache.get(&fake_key(0)).is_none(), "the oldest must not");
    }

    #[test]
    fn the_budget_still_evicts_before_the_cap() {
        // Bytes bind first when the entries are large — which is the full-resolution case,
        // where one checkpoint is 313 MB and the cap is nowhere near reached.
        let mut cache = PrefixCache::with_mid_budget(400); // holds two 192-byte dummies
        for tag in 0..5 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), 2);
        assert!(cache.mid_len() < PrefixCache::MIDS_CAP);
    }

    #[test]
    fn the_default_budget_is_the_real_one() {
        // `#[derive(Default)]` would silently give a budget of 0 and disable every mid, which
        // would leave the whole feature inert with all correctness tests still passing.
        // (That the const itself is non-zero is a compile-time assertion on `PrefixCache`.)
        assert_eq!(
            PrefixCache::default().mid_budget,
            PrefixCache::MID_BUDGET_BYTES
        );
    }

    /// The RGBA sub-rect of a frame, for comparing a region render against a whole one.
    fn crop_rgba(f: &RenderedFrame, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * f.width + x) * 4;
            out.extend_from_slice(&f.rgba[start..start + w * 4]);
        }
        out
    }

    #[test]
    fn the_brush_cache_never_builds_a_brush_that_needs_the_image() {
        // ⚠⚠ **What makes the dangerous state unrepresentable rather than documented.** A map
        // built here is rasterized without pixels, so an auto-masked brush served from one would
        // render positionally while the canvas tint showed it stopping at an edge — a silent
        // divergence between preview and export.
        //
        // ⚠⚠ **This test was INVERTED rather than deleted, and it used to assert something
        // stronger: that the cache *declines* a gating brush outright.** That was right while
        // nothing could identify the base. The cache now stores one, keyed on what produced that
        // base — but it still never **builds** one, because it has no image, and that is the half
        // that keeps the dangerous state unrepresentable. An inversion that changes only the
        // assertions leaves a name and a comment that lie, which is what happened once when the
        // spot-removal render rule was inverted.
        let gating = |auto: bool| {
            let mut e = Edits::default();
            e.masks.push(edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.3, 0.3], [0.7, 0.7]],
                        radius: 0.2,
                        auto_mask: auto,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: Default::default(),
            });
            e
        };
        let mut cache = BrushCache::default();
        let built = cache.get_or_build(
            &gating(true),
            CoverageSpec {
                fw: 64,
                fh: 64,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            built.maps[0][0].is_none(),
            "an auto-masked brush must not be prebuilt"
        );

        // ⚠ The discriminating half: a plain brush must STILL be cached, or this would be a
        // performance regression wearing a correctness fix.
        let built = cache.get_or_build(
            &gating(false),
            CoverageSpec {
                fw: 64,
                fh: 64,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            built.maps[0][0].is_some(),
            "a plain brush must still be cached"
        );
    }

    #[test]
    fn a_region_render_matches_the_whole_render_cropped() {
        // The master invariant. It validates the region maths, the vignette correction and
        // the cache at once — if any of the three is wrong, this fails.
        //
        // ⚠ Sharpening and local contrast *weaken* it, and that has to be said out loud
        // rather than left to an edit set that happens to omit them. A spatial stage in the
        // tail reads neighbours the cropped region genuinely does not have, so equality can
        // only hold in the interior. The excluded band is `tail_kernel_radius` wide —
        // precisely what `view_region::REGION_MARGIN` keeps off screen — and both the inset
        // and the buffer size are derived from it, so they track any future change to a
        // radius or a sigma.
        let edits = Edits {
            exposure: 0.4,
            vignette: edit_model::Vignette {
                amount: -80.0,
                roundness: 50.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                color: 40.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 70.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            // ⚠ A mask joins the master invariant because that test validates the region
            // maths, the vignette correction *and* the prefix cache at once — so this is what
            // proves masks survive the cached render path, not just a bare stage call.
            masks: vec![
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                        from: [0.1, 0.1],
                        to: [0.9, 0.8],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.6,
                        // ⚠⚠ **Masked spatial belongs in this edit set, and its absence would be
                        // silent.** This test has already been insufficient in exactly that way
                        // once: when sharpening landed it kept passing because its edits carried
                        // none, so the weakened invariant it exists to cover went unexercised. A
                        // masked clarity reaches 120 px on its own, *on top of* the global reach,
                        // because masks chain — so if `tail_kernel_radius` ever stopped counting
                        // them the inset below would be too small and this comparison would fail
                        // at the edges, which is precisely the bug it should catch.
                        local_contrast: edit_model::LocalContrast {
                            texture: 0.0,
                            clarity: 70.0,
                        },
                        // ⚠⚠ **The capture corrections belong in this edit set, and their absence
                        // would be silent.** They run in the *prefix*, whole-buffer, and that is the
                        // entire reason for their placement: dehaze estimates its airlight from the
                        // brightest 0.1% of the buffer it is handed, so if it ever ran on the
                        // visible region instead, the same pixel would render differently depending
                        // on where the viewport happened to be — and this comparison is what catches
                        // it. There is no other test that can.
                        //
                        // ⚠ They contribute **nothing** to `inset` below, because neither is in the
                        // tail. If this ever fails at the edges, the placement is wrong — do not
                        // widen the inset.
                        noise_reduction: edit_model::NoiseReduction {
                            luminance: 40.0,
                            color: 30.0,
                            ..Default::default()
                        },
                        dehaze: edit_model::Dehaze { amount: 45.0 },
                        ..Default::default()
                    },
                },
                // ⚠⚠ **AN AUTO-MASKED BRUSH BELONGS IN THIS EDIT SET, AND THIS IS THE GUARD ON THE
                // WHOLE AUTO-MASK DESIGN.** Its coverage depends on PIXEL VALUES as well as position,
                // which is what made a render-time reference untenable: a dab whose centre has been
                // panned off-region would have no reference to read, so its coverage — and therefore
                // the photograph — would depend on where the viewport happens to be. That is
                // `dehaze::airlight`'s failure arriving in a new place, and dehaze's escape (move to
                // the prefix) is not available, because brush coverage is resolved in the tail.
                //
                // A reference **baked into the stroke** travels in `Edits`, so it is available under
                // any region, in the CLI and in export alike. This comparison is what asserts that.
                // ⚠ Sabotage-verified: reading the reference from the base at the dab's own centre
                // fails this test.
                //
                // ⚠ It contributes **nothing** to `inset` below — a brush reads no neighbours, so it
                // adds no kernel reach. If this ever fails at the edges, something about the
                // reference's availability has changed; do not widen the inset.
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Brush(edit_model::Brush {
                        strokes: vec![edit_model::BrushStroke {
                            points: vec![[0.2, 0.3], [0.8, 0.7]],
                            radius: 0.2,
                            hardness: 60.0,
                            flow: 100.0,
                            erase: false,
                            auto_mask: true,
                            tolerance: 45.0,
                        }],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                },
            ],
            ..Edits::default()
        };

        let inset = pipeline::tail_kernel_radius(&edits, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 2 * inset + 40, 2 * inset + 36);
        let n = 2 * inset + 80;
        let buf = varied_sized(n);

        // max_dim = n is native for this n x n buffer, i.e. no downscale, so the render
        // scale is exactly 1.0 and the inset above is the full-resolution kernel.
        let (whole, _) = render_cached(
            &buf,
            &edits,
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        // Guard against the test passing trivially: the spatial tail stages must actually
        // move pixels here, or the "weakened invariant" it exists to cover is not exercised.
        let (unsharpened, _) = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                local_contrast: edit_model::LocalContrast::default(),
                ..edits.clone()
            },
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_ne!(
            whole.rgba, unsharpened.rgba,
            "the spatial tail stages changed nothing"
        );

        assert!(
            x + w <= whole.width && y + h <= whole.height,
            "region must fit"
        );
        assert!(
            w > 2 * inset && h > 2 * inset,
            "region must be wider than twice the kernel ({inset})"
        );

        let (got, _) = render_cached(
            &buf,
            &edits,
            FrameSpec {
                max_dim: n,
                region: Some(RegionRect { x, y, w, h }),
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        let (iw, ih) = (w - 2 * inset, h - 2 * inset);
        assert_eq!(
            crop_rgba(&got, inset, inset, iw, ih),
            crop_rgba(&whole, x + inset, y + inset, iw, ih),
            "the region's interior must match the whole render bit for bit"
        );
        assert_eq!(
            got.region,
            Some(pipeline::Region {
                full_w: whole.width,
                full_h: whole.height,
                x0: x,
                y0: y
            })
        );
    }

    #[test]
    fn the_cache_keeps_one_entry_per_render_mode() {
        // Fit and native are different max_dims. With a single entry, every zoom in or out
        // would recompute the prefix (~123 ms with NR on).
        let buf = varied();
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        render_cached(
            &buf,
            &e,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        ); // "fit"
        render_cached(
            &buf,
            &e,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        ); // "native"
        assert_eq!(cache.final_len(), 2, "both modes should stay cached");

        for max_dim in [8usize, 16] {
            let scale = render_scale(16, rendered_long_edge(16, max_dim), 1.0);
            assert!(
                cache
                    .get(&(
                        PrefixKey::new(&e, scale, pipeline::Resampler::Bilinear),
                        max_dim
                    ))
                    .is_some(),
                "max_dim {max_dim} was evicted"
            );
        }

        // A third distinct key evicts the least recently used.
        let e2 = Edits {
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..e.clone()
        };
        render_cached(
            &buf,
            &e2,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(cache.final_len(), 2);
    }

    #[test]
    fn a_region_bigger_than_the_image_is_clamped() {
        let buf = varied();
        let e = Edits::default();
        let (got, _) = render_cached(
            &buf,
            &e,
            FrameSpec {
                max_dim: 16,
                region: Some(RegionRect {
                    x: 10,
                    y: 10,
                    w: 999,
                    h: 999,
                }),
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!((got.width, got.height), (6, 6));
    }

    /// An auto-masked brush mask, plus a global exposure, so the two can be moved independently.
    fn auto_masked_edits(exposure: f32, mask_exposure: f32) -> Edits {
        let mut m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![edit_model::BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.3,
                    hardness: 100.0,
                    flow: 100.0,
                    auto_mask: true,
                    tolerance: 35.0,
                    ..Default::default()
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: Default::default(),
        };
        m.adjustments.exposure = mask_exposure;
        Edits {
            exposure,
            masks: vec![m],
            ..Edits::default()
        }
    }

    /// A coverage map distinguishable from a default one, so a hit can be told from a miss.
    fn a_coverage_map() -> Arc<pipeline::brush::CoverageMap> {
        Arc::new(pipeline::brush::CoverageMap {
            outside: 42,
            ..Default::default()
        })
    }

    /// ⚠⚠ **The load-bearing pair, and neither half alone means anything.** A test of the miss
    /// alone passes against a cache that never hits; a test of the hit alone passes against one
    /// that never misses.
    #[test]
    fn the_gating_coverage_survives_the_masks_own_slider_and_not_a_global_one() {
        let mut cache = BrushCache::default();
        let base = auto_masked_edits(0.0, 0.0);
        assert!(
            cache
                .get_or_build(
                    &base,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_none(),
            "fixture check: a cold cache must prebuild nothing for a gating brush, or the \
             assertions below are about the wrong thing"
        );
        cache.remember(
            &base,
            CoverageSpec {
                fw: 64,
                fh: 48,
                region: None,
                max_dim: 1600,
            },
            0,
            0,
            a_coverage_map(),
        );

        // ⚠ The mask's OWN adjustment moves. Its base is the output after the masks BEFORE it, so
        // this cannot change it — and that is the whole reason the key truncates.
        let own = auto_masked_edits(0.0, 1.5);
        assert!(
            cache
                .get_or_build(
                    &own,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_some(),
            "moving the mask's own slider must not invalidate its coverage — that is the case a \
             mask exists for, and the only thing this cache buys"
        );

        // ⚠ A GLOBAL adjustment moves. The masks loop runs after the entire global tail, so the
        // base really did change.
        let global = auto_masked_edits(1.5, 0.0);
        assert!(
            cache
                .get_or_build(
                    &global,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_none(),
            "a global slider changes the base, so serving the old coverage would render the mask \
             against pixels it was never gated on"
        );
    }

    /// ⚠ The rung is in the base identity: the base is a different image at a different resolution.
    #[test]
    fn the_gating_coverage_does_not_cross_rungs() {
        let mut cache = BrushCache::default();
        let e = auto_masked_edits(0.0, 0.0);
        cache.remember(
            &e,
            CoverageSpec {
                fw: 64,
                fh: 48,
                region: None,
                max_dim: 1600,
            },
            0,
            0,
            a_coverage_map(),
        );
        assert!(
            cache
                .get_or_build(
                    &e,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 3200
                    }
                )
                .get(0, 0)
                .is_none(),
            "coverage built at one rung must not be served at another"
        );
    }

    #[test]
    fn every_cached_frame_matches_a_cold_render() {
        // The master test: caching must be invisible in the output. The sequence walks a pure
        // hit, a tail-only change (resumes at the final), a dehaze change (resumes after NR),
        // an NR change (resumes after geometry), a geometry change (resumes at the seed), and
        // returns to earlier states so a stale checkpoint would show up.
        let buf = varied();
        // ⚠⚠ **An auto-masked brush is in the sequence, and it is what makes a stale base
        // visible.** Every other test of the coverage key asserts on *presence*; a key that admits
        // a stale base produces a map that is present and **wrong**, and only comparing pixels
        // against a cold render can see that. `b` below moves a **global** slider, which really
        // does change this mask's base — so the return leg is where a stale hit would show.
        let a = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.5, 0.5]],
                        radius: 0.35,
                        hardness: 100.0,
                        flow: 100.0,
                        auto_mask: true,
                        tolerance: 35.0,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..full_prefix()
        };
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // tail only — and it changes the mask's base
        let c = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..a.clone()
        }; // deepest prefix stage
        let d = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 10.0,
                color: 10.0,
                ..Default::default()
            },
            ..a.clone()
        }; // middle prefix stage
        let e = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..a.clone()
        }; // first prefix stage

        // Guard against the whole test passing trivially: these must actually differ.
        let cold_a = render_once(
            &buf,
            &a,
            8,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        for (name, other) in [("b", &b), ("c", &c), ("d", &d), ("e", &e)] {
            assert_ne!(
                cold_a.rgba,
                render_once(
                    &buf,
                    other,
                    8,
                    1.0,
                    color::OutputSpace::Srgb,
                    pipeline::Resampler::default(),
                    None,
                )
                .rgba,
                "edit set {name} renders identically to a — the test proves nothing"
            );
        }

        let engine = RenderEngine::new(
            buf.clone(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        for (i, ed) in [&a, &a, &b, &c, &a, &d, &e, &a, &c].iter().enumerate() {
            engine.request((*ed).clone(), FrameSpec::fit(8));
            let got = wait_for_frame(&engine);
            let cold = render_once(
                &buf,
                ed,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            );
            assert_eq!(got.rgba, cold.rgba, "frame {i} differs from a cold render");
        }
    }

    #[test]
    fn the_predicted_long_edge_matches_downscale_to() {
        // The cache keys off this prediction, so it must agree with what downscale_to
        // actually produces — including the case where the image is already small enough
        // and downscale_to just clones.
        for (w, h, max_dim) in [(8, 8, 4), (10, 5, 4), (5, 10, 4), (4, 4, 16), (7, 3, 7)] {
            let img = ImageBuf::new(w, h);
            let actual = img.downscale_to(max_dim);
            let actual_long = actual.width().max(actual.height());
            assert_eq!(
                rendered_long_edge(w.max(h), max_dim),
                actual_long,
                "{w}x{h} -> max_dim {max_dim}"
            );
        }
    }

    #[test]
    fn render_scale_combines_the_decode_and_the_downscale() {
        // A half-size RAW decode downscaled again by 2 is a quarter of full resolution;
        // the spatial stages must be told that, or a preview lies about the export.
        assert_eq!(render_scale(8, 4, 0.5).get(), 0.25);
        // A full-size source rendered at full size is exactly 1.0.
        assert_eq!(render_scale(8, 8, 1.0).get(), 1.0);
        // Non-square: the long edge is the reference, matching downscale_to.
        assert_eq!(render_scale(10, 5, 1.0).get(), 0.5);
    }

    #[test]
    fn produces_rgba_frame_of_expected_size() {
        let engine = RenderEngine::new(
            tiny(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        engine.request(Edits::default(), FrameSpec::fit(4)); // downscale 8 -> 4
        let f = wait_for_frame(&engine);
        assert_eq!((f.width, f.height), (4, 4));
        assert_eq!(f.rgba.len(), 4 * 4 * 4);
        // mid-gray linear 0.5 -> sRGB ~188; alpha 255
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
        assert_eq!(f.rgba[3], 255);
    }

    #[test]
    fn exposure_brightens() {
        let engine = RenderEngine::new(
            tiny(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        engine.request(
            Edits {
                exposure: 2.0,
                ..Edits::default()
            },
            FrameSpec::fit(4),
        );
        let f = wait_for_frame(&engine);
        assert!(f.rgba[0] > 240); // much brighter
    }

    #[test]
    fn render_once_matches_expected_gray() {
        let img = tiny();
        let f = render_once(
            &img,
            &Edits::default(),
            4,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        assert_eq!((f.width, f.height), (4, 4));
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
    }

    fn brush_edits() -> Edits {
        Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.3, 0.5], [0.7, 0.5]],
                        radius: 0.1,
                        hardness: 50.0,
                        flow: 100.0,
                        erase: false,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        }
    }

    /// Whether two coverage sets share mask 0's **map**, which is what "did not re-rasterize"
    /// means.
    ///
    /// ⚠⚠ **The observable moved and the old one silently stopped working.** These tests compared
    /// the outer `Arc<BrushCoverage>`, which was one cached object per render. Entries are per
    /// mask now and the set is assembled fresh each call, so outer identity is **never** equal —
    /// the three `ptr_eq` assertions began failing and, worse, the two `!ptr_eq` ones began
    /// passing **vacuously**. Only running them showed it.
    fn same_map(a: &Arc<pipeline::BrushCoverage>, b: &Arc<pipeline::BrushCoverage>) -> bool {
        same_map_at(a, b, 0, 0)
    }

    /// The same question for one component of one mask.
    ///
    /// ⚠ A composite's base and its refinements are different coverage under one mask index, so
    /// "did this shape re-rasterize" is only answerable per component.
    fn same_map_at(
        a: &Arc<pipeline::BrushCoverage>,
        b: &Arc<pipeline::BrushCoverage>,
        i: usize,
        j: usize,
    ) -> bool {
        match (a.get(i, j), b.get(i, j)) {
            (Some(x), Some(y)) => Arc::ptr_eq(&x, &y),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn the_brush_cache_hits_and_keeps_both_zoom_levels() {
        // ⚠ Every other test in this file would pass with the brush cache deleted, because it is
        // a caching change and nothing else. This one asserts identity of the Arc, which is the
        // only observable difference between a hit and a rebuild.
        let mut cache = BrushCache::default();
        let edits = brush_edits();
        let a = cache.get_or_build(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let b = cache.get_or_build(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &b), "the second call must hit");

        let c = cache.get_or_build(
            &edits,
            CoverageSpec {
                fw: 400,
                fh: 200,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "different frame dims must miss");

        // ⚠ The assertion that pins CAP = 2 rather than 1. With a single entry the 400x200 build
        // would have evicted the 200x100 one, and this would rebuild — which is exactly the zoom
        // thrash the capacity exists to prevent.
        let d = cache.get_or_build(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &d), "CAP=2 must keep both zoom levels warm");
    }

    #[test]
    fn changing_a_stroke_misses_the_brush_cache() {
        // The other half: a hit must depend on the strokes, not merely on the dimensions.
        let mut cache = BrushCache::default();
        let a = cache.get_or_build(
            &brush_edits(),
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let mut edited = brush_edits();
        if let edit_model::Mask::Brush(b) = &mut edited.masks[0].mask {
            b.strokes[0].points.push([0.8, 0.6]);
        }
        let c = cache.get_or_build(
            &edited,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "an extra point must miss");
    }

    #[test]
    fn the_coverage_index_follows_the_mask_position_not_the_active_count() {
        // ⚠ An INACTIVE mask still owns a slot. A counter over active masks would misalign every
        // map after the first inactive one — the same sparse-versus-dense index bug this codebase
        // has hit three times before.
        let mut edits = brush_edits();
        let inactive = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments::default(),
        };
        edits.masks.insert(0, inactive);
        let mut cache = BrushCache::default();
        let cov = cache.get_or_build(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(cov.get(0, 0).is_none(), "slot 0 is the analytic mask");
        assert!(cov.get(1, 0).is_some(), "slot 1 is the brush");
    }

    /// A composite whose base **and** refinement are both brushes, with different strokes so
    /// their maps are distinguishable.
    fn composite_of_two_brushes() -> Edits {
        let stroke = |x: f32| edit_model::BrushStroke {
            points: vec![[x, 0.5]],
            radius: 0.1,
            hardness: 50.0,
            flow: 100.0,
            erase: false,
            ..Default::default()
        };
        let brush = |x: f32| {
            edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![stroke(x)],
                invert: false,
            })
        };
        let mut e = brush_edits();
        e.masks[0].mask = brush(0.3);
        e.masks[0].refine = vec![edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: brush(0.7),
        }];
        e
    }

    fn whole_frame() -> CoverageSpec {
        CoverageSpec {
            fw: 200,
            fh: 100,
            region: None,
            max_dim: 1600,
        }
    }

    #[test]
    fn two_components_of_one_mask_do_not_evict_each_other() {
        // ⚠ The cap counts **components**, not masks: a composite of two brushes must keep both
        // maps at both live rungs, or every zoom change would re-rasterize the pair.
        let mut cache = BrushCache::default();
        let e = composite_of_two_brushes();
        let a = cache.get_or_build(&e, whole_frame());
        assert!(a.get(0, 0).is_some(), "the base's map must be built");
        assert!(a.get(0, 1).is_some(), "the refinement's map must be built");
        let b = cache.get_or_build(&e, whole_frame());
        assert!(same_map_at(&a, &b, 0, 0), "the base must hit");
        assert!(same_map_at(&a, &b, 0, 1), "the refinement must hit");
    }

    #[test]
    fn changing_one_component_misses_only_that_component() {
        // ⚠⚠ The discriminating test. Without the component in the key both shapes collide on one
        // entry, so touching either would invalidate the other — and the symptom is not an error
        // but a shape rendering positionally while its tint shows the gate working.
        let mut cache = BrushCache::default();
        let e = composite_of_two_brushes();
        let a = cache.get_or_build(&e, whole_frame());
        let mut moved = e.clone();
        if let Some(edit_model::Mask::Brush(b)) =
            moved.masks[0].refine.get_mut(0).map(|r| &mut r.mask)
        {
            b.strokes[0].points.push([0.75, 0.55]);
        }
        let b = cache.get_or_build(&moved, whole_frame());
        assert!(
            same_map_at(&a, &b, 0, 0),
            "the untouched base must still hit"
        );
        assert!(
            !same_map_at(&a, &b, 0, 1),
            "the changed refinement must miss"
        );
    }

    #[test]
    fn the_brush_cache_misses_when_the_region_moves() {
        // ⚠⚠ This REVERSES the rule `PrefixCache` follows, deliberately. Keeping the region out
        // of the key buys a cache that survives a pan — invisible, because panning re-runs the
        // whole tail anyway — and costs a full-frame rasterization on every paint frame:
        // 23.76 ms at 6246x4170, measured, and reported in use as "very sluggish at 1:1".
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let a = cache.get_or_build(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        let b = cache.get_or_build(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &b), "the same region must hit");
        let c = cache.get_or_build(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((20, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "a moved region must miss");
    }

    #[test]
    fn the_brush_cache_still_hits_when_only_a_slider_moves() {
        // The cache's remaining job, and the reason it exists at all: dragging an unrelated
        // slider with a brush mask present must not re-rasterize on every frame.
        let mut cache = BrushCache::default();
        let a = cache.get_or_build(
            &brush_edits(),
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let mut louder = brush_edits();
        louder.exposure = 1.5;
        let b = cache.get_or_build(
            &louder,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            same_map(&a, &b),
            "a global slider does not touch the strokes and must hit"
        );
    }

    #[test]
    fn a_clipped_cache_entry_covers_only_its_region() {
        // ⚠ The clip really is applied, not merely keyed on. Without it the two entries would be
        // identical maps and this would pass for the wrong reason, so it asserts a frame pixel
        // *outside* the clip reads nothing.
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let cov = cache.get_or_build(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 40, 100)),
                max_dim: 1600,
            },
        );
        let map = cov.get(0, 0).expect("the brush mask has a map");
        assert_eq!(map.at(120, 50), 0, "beyond the clip there is no coverage");
    }

    #[test]
    fn the_display_space_changes_the_packed_pixels_and_srgb_does_not() {
        // ⚠ Two claims. The sRGB half is the regression guard — it is the default, and if it
        // moved every preview would shift for people who never touched the setting. The P3 half
        // proves the parameter is actually reaching `pack`; without it the setting would be a
        // dropdown that does nothing, which is the failure this whole task risks.
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.9, 0.1, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));

        let srgb = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::DisplayP3,
            pipeline::Resampler::default(),
            None,
        );

        assert_ne!(srgb.rgba, p3.rgba, "the display space did not reach `pack`");
        // ⚠ The corrected image must be *less* saturated in red, not more — an inverted
        // transform would also differ, and would reproduce the very bug this fixes while
        // passing an `assert_ne!`.
        assert!(
            p3.rgba[0] < srgb.rgba[0],
            "P3 red {} should be below sRGB red {}",
            p3.rgba[0],
            srgb.rgba[0]
        );
    }

    #[test]
    fn the_histogram_is_unaffected_by_the_display_space() {
        // ⚠⚠ The property this whole design exists to protect, and it was **shipped broken
        // once**: the transform went into `pack`, and `shell.rs` derived the histogram from
        // `pack`'s output — so the histogram silently became display-referred. A histogram that
        // moves when you change monitors cannot tell you what is clipping.
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.95, 0.15, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));

        let srgb = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::DisplayP3,
            pipeline::Resampler::default(),
            None,
        );

        assert_ne!(
            srgb.rgba, p3.rgba,
            "the display space did not reach the texture"
        );
        assert_eq!(
            srgb.hist, p3.hist,
            "the histogram must describe the image, not the monitor"
        );
    }

    /// The fixture `the_histogram_is_unaffected_by_the_display_space` uses, reused rather than
    /// re-invented: a two-pixel buffer with a saturated red and a mid green, which is what makes
    /// the display transform actually move something.
    fn render_for_test(space: color::OutputSpace) -> RenderedFrame {
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.95, 0.15, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));
        render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            space,
            pipeline::Resampler::default(),
            None,
        )
    }

    /// ⚠⚠ **`srgb` must be sRGB whatever the display space, and `rgba` must not.** Nothing else
    /// can see this: a colour readout built on `rgba` looks entirely plausible and is simply
    /// wrong on a wide-gamut monitor. This is the sibling of
    /// `the_histogram_is_unaffected_by_the_display_space`, one layer over — and the histogram is
    /// asserted here too, so the two instrument channels cannot drift apart.
    #[test]
    fn the_srgb_channel_is_unaffected_by_the_display_space() {
        let a = render_for_test(color::OutputSpace::Srgb);
        let b = render_for_test(color::OutputSpace::DisplayP3);
        assert_eq!(
            *a.srgb, *b.srgb,
            "the sRGB channel moved with the display space"
        );
        assert_ne!(
            *a.rgba, *b.rgba,
            "fixture check: the display transform did nothing, so this test proves nothing"
        );
        assert_eq!(a.hist, b.hist);
    }

    /// ⚠ In the sRGB case the two must be **one** allocation, or every rendered frame pays a
    /// multi-megabyte clone — ~7 MB at the fit rung, ~27 MB at a native one. Asserted
    /// structurally rather than by measuring.
    #[test]
    fn the_srgb_case_shares_one_allocation() {
        let f = render_for_test(color::OutputSpace::Srgb);
        assert!(
            Arc::ptr_eq(&f.rgba, &f.srgb),
            "sRGB rendered two buffers where one would do"
        );
    }

    /// ⚠⚠ `srgb` is taken **before** Visualize Spots replaces the image, so the readout reports
    /// the photograph and not the grey edge view — the ordering `hist` already depends on, and
    /// the reason `pack` wraps the `Arc` above that block rather than below it. Nothing in the
    /// canvas can see this: an edge-view readout is a plausible-looking number.
    /// ⚠⚠ The two arms of [`Packed`] must produce the **same frame**, because the GPU's
    /// quantization and `to_rgba8` are the same rounding of the same number — M8.3g's whole
    /// premise. If this ever fails, the shader and `ImageBuf::quantize` have diverged.
    ///
    /// ⚠ And the histogram must come from the bytes rather than being recomputed: on the byte path
    /// there is no float left to recompute it from, which is what makes that arm small.
    #[test]
    fn packing_bytes_gives_the_same_frame_as_packing_the_float() {
        let buf = ImageBuf::from_vec(2, 1, vec![0.0, 0.5, 1.0, -0.2, 0.25, 1.7]);
        let want = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            None,
        );
        let got = pack(
            Packed::Rgba8 {
                bytes: buf.to_rgba8(),
                w: 2,
                h: 1,
            },
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            None,
        );
        assert_eq!(*got.rgba, *want.rgba, "texture");
        assert_eq!(*got.srgb, *want.srgb, "the photograph's own bytes");
        assert_eq!(got.hist, want.hist, "histogram");
        assert_eq!((got.width, got.height), (2, 1));
        assert!(
            Arc::ptr_eq(&got.rgba, &got.srgb),
            "the two must be one allocation, as on the float sRGB path"
        );
    }

    #[test]
    fn the_srgb_channel_is_unaffected_by_visualize_spots() {
        // A hard edge, so the edge view is far from the photograph and the textures must differ.
        let mut buf = ImageBuf::new(16, 4);
        for y in 0..4 {
            for x in 0..16 {
                let v = if x < 8 { 0.1 } else { 0.8 };
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let plain = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            None,
        );
        let vis = pack(
            Packed::Float(buf),
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            Some(12.0),
        );
        assert_ne!(
            *plain.rgba, *vis.rgba,
            "fixture check: the view mode did not reach the pixels"
        );
        assert_eq!(
            *plain.srgb, *vis.srgb,
            "the view mode reached the readout's bytes"
        );
    }

    /// ⚠⚠ The histogram must be **unaffected** by Visualize Spots, for the reason the display
    /// transform records: `pack` takes the histogram from its sRGB output *before* the post-step,
    /// and a view mode that poisoned it would make the histogram lie about the photograph. This
    /// is the test that the ordering was inherited rather than reinvented.
    #[test]
    fn the_histogram_is_unaffected_by_visualize_spots() {
        // A hard edge, so the edge view is far from black and the two textures must differ.
        let mut buf = ImageBuf::new(16, 4);
        for y in 0..4 {
            for x in 0..16 {
                let v = if x < 8 { 0.1 } else { 0.8 };
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let plain = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            None,
        );
        let vis = pack(
            Packed::Float(buf),
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            Some(12.0),
        );
        assert_ne!(
            plain.rgba, vis.rgba,
            "the view mode did not reach the pixels"
        );
        assert_eq!(plain.hist, vis.hist, "the view mode reached the histogram");
    }

    // ---- soft proofing -------------------------------------------------------------------

    /// A neutral grey: in gamut in every space, so it must never be marked.
    const IN_GAMUT: [f32; 3] = [0.5, 0.5, 0.5];

    /// Display P3's green primary, pulled a tenth of the way toward mid grey **in P3
    /// coordinates**, expressed in the working space.
    ///
    /// ⚠⚠ **This is the discriminating fixture, and it is off the boundary on purpose.** P3's
    /// primary itself lands at a P3 coordinate of exactly 1.0, where a rounding change either way
    /// decides whether it is "out of gamut" — a fixture that flips with the last f32 bit proves
    /// nothing. Blended, it sits at P3 ≈ (0.05, 0.95, 0.05): comfortably inside P3, and still far
    /// outside sRGB, where its red component is around −0.4. `the_fixture_is_in_one_gamut_and_out
    /// _of_another` asserts that premise rather than trusting this comment.
    const P3_GREEN: [f32; 3] = [0.242_51, 0.847_31, 0.095_72];

    /// ProPhoto's own green: outside sRGB *and* outside P3.
    const WIDE_GREEN: [f32; 3] = [0.0, 1.0, 0.0];

    /// Three working-space colours the proof must actually move, one pixel each.
    fn gamut_fixture() -> Arc<ImageBuf> {
        let mut buf = ImageBuf::new(3, 1);
        buf.set_pixel(0, 0, IN_GAMUT);
        buf.set_pixel(1, 0, P3_GREEN);
        buf.set_pixel(2, 0, WIDE_GREEN);
        Arc::new(buf)
    }

    /// Render `buf` through the cached path at its own resolution, with the given proof.
    fn proofed(buf: &Arc<ImageBuf>, soft_proof: Option<Proof>) -> RenderedFrame {
        proofed_with(buf, soft_proof, None)
    }

    fn proofed_with(
        buf: &Arc<ImageBuf>,
        soft_proof: Option<Proof>,
        visualize: Option<f32>,
    ) -> RenderedFrame {
        render_cached(
            buf,
            &Edits::default(),
            FrameSpec {
                // The fixture's own long edge, so nothing is downscaled and each pixel stays one
                // pixel — the texture is read back by index below.
                max_dim: 3,
                region: None,
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize,
                soft_proof,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    /// The RGBA of pixel `i` of a 3×1 frame.
    fn px(f: &RenderedFrame, i: usize) -> [u8; 4] {
        [
            f.rgba[i * 4],
            f.rgba[i * 4 + 1],
            f.rgba[i * 4 + 2],
            f.rgba[i * 4 + 3],
        ]
    }

    fn proof(space: color::OutputSpace, warn: bool) -> Option<Proof> {
        Some(Proof { space, warn })
    }

    #[test]
    fn the_fixture_is_in_one_gamut_and_out_of_another() {
        // ⚠⚠ The fixture's own premise, asserted before anything is built on it. A colour flagged
        // in *every* space cannot tell a working proof from one that ignores the space argument
        // entirely — which is the shape of bug the whole feature is exposed to. If this fails, the
        // tests below are measuring something other than what they name.
        assert!(
            color::is_out_of_gamut(P3_GREEN, color::OutputSpace::Srgb),
            "fixture check: the green must be outside sRGB"
        );
        assert!(
            !color::is_out_of_gamut(P3_GREEN, color::OutputSpace::DisplayP3),
            "fixture check: the green must be inside Display P3"
        );
        assert!(!color::is_out_of_gamut(IN_GAMUT, color::OutputSpace::Srgb));
        assert!(color::is_out_of_gamut(WIDE_GREEN, color::OutputSpace::Srgb));
    }

    /// ⚠⚠ **The histogram describes the PHOTOGRAPH, never the view mode.** It is derived from
    /// `pack`'s sRGB quantisation, so a proof applied before that point silently makes it
    /// display-referred — the bug the display-profile work shipped and caught by re-reading what
    /// consumes the bytes.
    ///
    /// ⚠⚠ It holds **byte for byte and by construction**, not by luck: while proofing the tail
    /// runs twice, and the histogram comes from an unmodified sRGB run. A single ProPhoto run was
    /// tried first and measured to move 12 of 12 real frames' histograms, worst five levels.
    #[test]
    fn the_histogram_is_unaffected_by_the_soft_proof() {
        let buf = gamut_fixture();
        let plain = proofed(&buf, None);
        let mut moved = false;
        for space in color::OutputSpace::DISPLAYS {
            for warn in [false, true] {
                let p = proofed(&buf, proof(space, warn));
                assert_eq!(
                    plain.hist, p.hist,
                    "the proof reached the histogram ({space:?}, warn={warn})"
                );
                // ⚠ The sRGB instrument channel too. A colour readout built on the *texture*
                // would report the proof's numbers, which look entirely plausible and are wrong.
                assert_eq!(
                    *plain.srgb, *p.srgb,
                    "the proof reached the readout's bytes ({space:?}, warn={warn})"
                );
                moved |= *plain.rgba != *p.rgba;
            }
        }
        assert!(
            moved,
            "fixture check: no proof changed the texture, so this test proves nothing"
        );
    }

    /// ⚠ With the proof off, `pack` is bit-exactly what it was — so nothing about the ordinary
    /// path moved. `render_once` is the reference precisely because it has no proof concept at
    /// all: it always passes `None`, so it cannot drift with this feature.
    #[test]
    fn the_proof_off_path_is_unchanged() {
        let buf = gamut_fixture();
        let cold = render_once(
            &buf,
            &Edits::default(),
            3,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        let off = proofed(&buf, None);
        assert_eq!(
            *cold.rgba, *off.rgba,
            "the texture moved with the proof off"
        );
        assert_eq!(*cold.srgb, *off.srgb, "the sRGB channel moved");
        assert_eq!(cold.hist, off.hist, "the histogram moved");
        // ⚠ And the fixture is one a proof genuinely changes, or "unchanged" would be trivially
        // true of an image nothing could move.
        let on = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            *cold.rgba, *on.rgba,
            "fixture check: the proof changed nothing, so this test proves nothing"
        );
    }

    /// ⚠⚠ **Both halves, and the second is the one that matters.** A warning that flags every
    /// pixel and one that flags none each pass a one-sided assertion.
    #[test]
    fn a_colour_the_proof_space_cannot_hold_is_marked_and_one_it_can_is_not() {
        let buf = gamut_fixture();
        let f = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            px(&f, 0),
            marked(),
            "a neutral grey is in gamut everywhere and must not be marked"
        );
        assert_eq!(px(&f, 1), marked(), "sRGB cannot hold this green");
        assert_eq!(px(&f, 2), marked(), "sRGB cannot hold ProPhoto green");
    }

    /// The warning as it must appear in the texture.
    fn marked() -> [u8; 4] {
        [255, 60, 60, 255]
    }

    /// ⚠ The colour is **fixed**, asserted, so it cannot drift onto the theme accent — which is
    /// user-switchable and includes a `Neutral` setting whose whole point is to have no colour,
    /// defeating the warning exactly when someone had chosen it.
    #[test]
    fn the_warning_colour_is_fixed() {
        let buf = gamut_fixture();
        let f = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_eq!(
            px(&f, 2),
            marked(),
            "the warning must be the crop overlay's fixed red (255, 60, 60)"
        );
    }

    /// ⚠ The warning is a **separate toggle** from the proof: proofing without it is the common
    /// case. Without this, a version that marked unconditionally would pass every other test here.
    #[test]
    fn the_warning_only_fires_when_it_is_switched_on() {
        let buf = gamut_fixture();
        let quiet = proofed(&buf, proof(color::OutputSpace::Srgb, false));
        let loud = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            px(&quiet, 2),
            marked(),
            "the warning fired with the flag off"
        );
        assert_eq!(px(&loud, 2), marked(), "fixture check: nothing was marked");
        // The proof itself still applies with the warning off — only the marking is suppressed.
        //
        // ⚠⚠ Measured against **Display P3**, not sRGB, and the reason is a finding rather than a
        // convenience: an sRGB proof with the warning off came back *bit-identical* to no proof on
        // this fixture, which is correct. The ordinary render already clips into sRGB, so proofing
        // into sRGB re-does what the pipeline just did. **An sRGB proof is only informative with
        // the warning on** — the marking is the whole of what it adds — and a test asserting
        // otherwise would have been asserting something false about correct code.
        let plain = proofed(&buf, None);
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, false));
        assert_ne!(
            *plain.rgba, *p3.rgba,
            "the proof did nothing with the warning off"
        );
    }

    /// ⚠⚠ **The test that catches a warning ignoring its space argument.** The green is inside
    /// Display P3 and outside sRGB, so proofing the same frame into the two spaces must mark it in
    /// one and not the other. A fixture flagged everywhere cannot see this, and neither can a test
    /// that only ever proofs into one space.
    #[test]
    fn the_proof_space_decides_what_is_marked() {
        let buf = gamut_fixture();
        let srgb = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, true));
        assert_eq!(px(&srgb, 1), marked(), "sRGB cannot hold this green");
        assert_ne!(px(&p3, 1), marked(), "Display P3 can hold this green");
        // ⚠ And a colour outside *both* is marked either way, so "P3 marks nothing" is excluded.
        assert_eq!(px(&p3, 2), marked(), "P3 cannot hold ProPhoto green");
    }

    /// ⚠ The proof reaches the texture at all, and reaches it *differently* per space — the
    /// failure this whole task risks is a menu that changes a field nothing reads.
    #[test]
    fn the_proof_reaches_the_texture() {
        let buf = gamut_fixture();
        let plain = proofed(&buf, None);
        let srgb = proofed(&buf, proof(color::OutputSpace::Srgb, false));
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, false));
        assert_ne!(*plain.rgba, *p3.rgba, "the proof never reached `pack`");
        assert_ne!(
            *srgb.rgba, *p3.rgba,
            "the two proof spaces produced the same texture"
        );
        // ⚠ The direction, not merely a difference: P3's primaries are wider, so an in-gamut
        // colour needs a *smaller* green coordinate there than in sRGB. An inverted conversion
        // would also differ and would be wrong in exactly the way this feature exists to avoid.
        assert!(
            px(&p3, 1)[1] < px(&srgb, 1)[1],
            "P3 green {:?} should sit below sRGB green {:?}",
            px(&p3, 1),
            px(&srgb, 1)
        );
    }

    /// ⚠ Visualize Spots and the proof are both view modes applied at the same point in `pack`,
    /// and the diagnostic one wins: its edge view is achromatic, so proofing it would mark nothing
    /// and show nothing while still paying for a second tail run. Asserted so the suppression is a
    /// decision rather than something a later reader removes as redundant.
    #[test]
    fn visualize_spots_suppresses_the_proof() {
        let buf = gamut_fixture();
        let vis = proofed_with(&buf, None, Some(12.0));
        let both = proofed_with(&buf, proof(color::OutputSpace::Srgb, true), Some(12.0));
        assert_eq!(
            *vis.rgba, *both.rgba,
            "the proof reached the edge view's texture"
        );
    }

    #[test]
    fn requests_are_counted() {
        // ⚠⚠ This counter exists because "a drag mutated `Edits` but never asked for a render"
        // is invisible from outside — the request goes into a channel and vanishes. That bug
        // shipped: the mask overlay kept showing its previous position until some *unrelated*
        // edit happened to request a frame, which is why moving a slider appeared to fix it.
        //
        // ⚠ The alternative was asserting on `Session::requested_region`, which is a *proxy*
        // that misses exactly the whole-image fit case where the bug actually occurred.
        let spec = || FrameSpec {
            max_dim: 64,
            region: None,
            display_space: color::OutputSpace::Srgb,
            resampler: pipeline::Resampler::default(),
            gpu_tail: false,
            visualize: None,
            soft_proof: None,
            grade_view: None,
        };
        let engine = RenderEngine::new(
            Arc::new(ImageBuf::new(4, 2)),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        assert_eq!(engine.requests_issued(), 0, "nothing asked for yet");

        engine.request(Edits::default(), spec());
        assert_eq!(engine.requests_issued(), 1);

        engine.request(Edits::default(), spec());
        assert_eq!(engine.requests_issued(), 2, "each request counts");
    }

    // ---- the grade range view ------------------------------------------------------------

    use crate::grade_view::GradeRangeView;

    /// Six greys spanning black to above white, a saturated blue and a red — 8×1.
    fn ramp_fixture() -> Arc<ImageBuf> {
        let px = [
            [0.005f32, 0.005, 0.005],
            [0.05, 0.05, 0.05],
            [0.18, 0.18, 0.18],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [1.4, 1.4, 1.4],
            [0.01, 0.025, 0.45],
            [0.5, 0.1, 0.1],
        ];
        let mut buf = ImageBuf::new(px.len(), 1);
        for (i, p) in px.iter().enumerate() {
            buf.set_pixel(i, 0, *p);
        }
        Arc::new(buf)
    }

    /// A grade that is active (saturation on all three wheels), with `luminance` on every wheel
    /// set to `lum` — the offset that moves L *after* the tap.
    fn graded(lum: f32, rendering: Option<edit_model::Rendering>) -> Edits {
        let wheel = |hue: f32| edit_model::GradeWheel {
            hue,
            saturation: 70.0,
            luminance: lum,
        };
        Edits {
            color_grading: edit_model::ColorGrading {
                shadows: wheel(240.0),
                midtones: wheel(120.0),
                highlights: wheel(45.0),
                ..edit_model::ColorGrading::default()
            },
            rendering,
            ..Default::default()
        }
    }

    /// Render through the cached path at the fixture's own resolution, with the view.
    fn viewed(
        buf: &Arc<ImageBuf>,
        edits: &Edits,
        grade_view: Option<GradeRangeView>,
        region: Option<RegionRect>,
    ) -> RenderedFrame {
        render_cached(
            buf,
            edits,
            FrameSpec {
                max_dim: buf.width().max(buf.height()),
                region,
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    fn plane(f: &RenderedFrame) -> Vec<u8> {
        f.grade_view
            .as_ref()
            .map(|p| p.as_ref().clone())
            .expect("the frame carries no plane")
    }

    /// ⚠⚠ The view is an overlay, never a change to the photograph: pixels, sRGB channel and
    /// histogram are byte-identical with the view on and off. Sabotage: painting the tint into
    /// `out` fails it. And `None` is the old path — no plane at all.
    #[test]
    fn the_grade_view_does_not_touch_the_photograph() {
        let buf = ramp_fixture();
        let edits = graded(0.0, None);
        let off = viewed(&buf, &edits, None, None);
        let on = viewed(&buf, &edits, Some(GradeRangeView::Shadows), None);
        assert!(
            off.grade_view.is_none(),
            "the view-off frame carries a plane"
        );
        assert_eq!(on.rgba, off.rgba, "the view changed the texture");
        assert_eq!(on.srgb, off.srgb, "the view changed the instrument channel");
        assert_eq!(on.hist, off.hist, "the view changed the histogram");
        let p = plane(&on);
        assert_eq!(p.len(), on.width * on.height);
        // The ramp's ends: the darkest grey is mostly shadow, the brightest barely — at the
        // fixture's default Blending of 50 the ranges overlap widely, so "mostly" is the honest
        // reading (143 of 255 at L = 0.17), not the near-255 a Blending of 0 would give.
        assert!(
            p[0] > 128 && p[5] < 16,
            "the plane is not the shadow weight: {p:?}"
        );
    }

    /// ⚠⚠ The plane is taken BEFORE the grade. A luminance wheel at +100 shifts L by 0.15 on
    /// every pixel it weights; tapped after the stage, the plane would move. Sabotage: run
    /// `from_grade` before taking the plane.
    #[test]
    fn the_plane_is_taken_before_the_grade() {
        let buf = ramp_fixture();
        for range in GradeRangeView::ALL {
            let flat = plane(&viewed(&buf, &graded(0.0, None), Some(range), None));
            let lifted = plane(&viewed(&buf, &graded(100.0, None), Some(range), None));
            assert_eq!(
                flat, lifted,
                "{range:?}: the grade's own luminance moved the plane"
            );
        }
        // The premise: the lift really changes the photograph, so the equality above is not
        // vacuous.
        let a = viewed(&buf, &graded(0.0, None), None, None);
        let b = viewed(&buf, &graded(100.0, None), None, None);
        assert_ne!(a.srgb, b.srgb, "premise: +100 luminance changed nothing");
    }

    /// ⚠⚠ The plane is taken BEFORE the display rendering — the test that encodes spec §1 and
    /// would have failed the pack-side design outright. Sabotage: tap after
    /// `DisplayRenderingStage` (i.e. from `pack`'s output).
    #[test]
    fn the_plane_is_taken_before_the_display_rendering() {
        let buf = ramp_fixture();
        let colorimetric = graded(0.0, None);
        let rendered = graded(0.0, Some(edit_model::Rendering::Camera));
        for range in GradeRangeView::ALL {
            let a = plane(&viewed(&buf, &colorimetric, Some(range), None));
            let b = plane(&viewed(&buf, &rendered, Some(range), None));
            assert_eq!(a, b, "{range:?}: the S-curve moved the plane");
        }
        let a = viewed(&buf, &colorimetric, None, None);
        let b = viewed(&buf, &rendered, None, None);
        assert_ne!(a.srgb, b.srgb, "premise: the rendering changed nothing");
    }

    /// On a region render the plane covers the rendered slice — `w * h` of the frame, not of
    /// the whole image. `ColorImage::from_rgba_unmultiplied` panics on a mismatch rather than
    /// reporting one, so this is pinned here.
    #[test]
    fn the_plane_has_the_frames_dimensions() {
        let mut buf = ImageBuf::new(8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let v = (x + y * 8) as f32 / 48.0;
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(buf);
        let region = Some(RegionRect {
            x: 2,
            y: 1,
            w: 4,
            h: 3,
        });
        let f = viewed(
            &buf,
            &graded(0.0, None),
            Some(GradeRangeView::Midtones),
            region,
        );
        assert_eq!((f.width, f.height), (4, 3));
        assert_eq!(plane(&f).len(), 12);
    }
}

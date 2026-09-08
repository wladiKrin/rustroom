//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

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
    /// The grade range view's plane — one byte per pixel, the selected range's weight — or
    /// `None` when the view is off.
    ///
    /// ⚠ Taken at the grading stage's **input** (`TailHalves::before_grade`'s output), the only
    /// place the ranges are exact; see `grade_view`. `width × height` bytes, the frame's own
    /// dimensions, by construction: no stage after the tap changes a buffer's size.
    pub grade_view: Option<Arc<Vec<u8>>>,
}

/// A requested crop of the rendered image, in post-geometry pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl RegionRect {
    /// Clamp to an image of `w` x `h`, returning `None` if nothing is left.
    fn clamped(self, w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let x = self.x.min(w - 1);
        let y = self.y.min(h - 1);
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self { x, y, w: cw, h: ch })
    }
}

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
                let (frame, _stages_run) = render_cached(
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
                        "render: {:6.1} ms  rung {:5}  frame {}x{} ({:.1} Mpx)  region {}  masks {}  prefix_ran {_stages_run}",
                        t0.elapsed().as_secs_f64() * 1000.0,
                        req.spec.max_dim,
                        frame.width,
                        frame.height,
                        (frame.width * frame.height) as f64 / 1e6,
                        if frame.region.is_some() { "yes" } else { "no " },
                        edits.masks.len(),
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
fn pack(
    out: ImageBuf,
    proofed: Option<(ImageBuf, Proof)>,
    region: Option<Region>,
    max_dim: usize,
    display_space: color::OutputSpace,
    visualize: Option<f32>,
) -> RenderedFrame {
    let (width, height) = (out.width(), out.height());
    // ⚠ Quantized to sRGB *first*, and the histogram taken from that. The histogram describes
    // the image; the texture describes what this monitor must be driven with. Deriving one from
    // the other is the bug this ordering exists to prevent.
    let srgb8 = out.to_rgba8();
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
            let out = edge_image(&out, threshold);
            let shown = Arc::new(out.to_rgba8());
            (out, shown)
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
                out.map_pixels(move |c| {
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

/// A cached checkpoint is valid for one cumulative set of prefix parameters at one working
/// resolution.
type CacheKey = (PrefixKey, usize);

/// Which tier a checkpoint belongs to. The complete prefix output is a `Final`; every
/// intermediate is a `Mid`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tier {
    Final,
    Mid,
}

/// Cached prefix checkpoints, in two tiers that cannot evict each other.
///
/// `finals` holds complete prefix outputs, capped at two entries — one per render mode, so
/// zooming in and out never recomputes a prefix. That is byte for byte the policy this cache
/// had before intermediates existed, and it is preserved unconditionally: a native render must
/// never be able to evict the fit final to make room for its own intermediates.
///
/// `mids` holds the intermediates under a byte budget. Retention is effectively deepest-first,
/// and falls out of plain LRU rather than needing its own rule: within one render checkpoints
/// are inserted shallow to deep, so the deepest is the most recently used and the shallowest is
/// evicted first. That is the right order — the deeper checkpoint saves more upstream work.
///
/// Both tiers are most-recently-used first.
struct PrefixCache {
    finals: Vec<(CacheKey, Arc<ImageBuf>)>,
    mids: Vec<(CacheKey, Arc<ImageBuf>)>,
    mid_budget: usize,
}

/// ⚠ A zero budget would evict every intermediate on insert, leaving the whole checkpoint chain
/// inert while every correctness test still passed. Checked at compile time so it cannot regress.
///
/// It has to be a free-standing `const _` item: an *associated* const is only evaluated where it
/// is used, so writing this inside `impl PrefixCache` compiles happily with a budget of 0 — a
/// guard that does not guard, which is worse than none. Verified by setting the budget to 0 and
/// watching this fail.
const _: () = assert!(PrefixCache::MID_BUDGET_BYTES > 0);

/// The sizing decision behind [`PrefixCache::MID_BUDGET_BYTES`], asserted rather than left in a
/// comment where it could quietly stop being true.
///
/// A 26 Mpx (X-T4) full-resolution checkpoint is ~313 MB: the budget must clear one and must
/// *not* clear two, since a second would double the footprint to save ~200 ms on a rare
/// interaction. It must also clear a 40 Mpx body's (X-T5, X-H2) — which is the whole reason
/// this is 512 MiB rather than 320.
///
/// Compile-time for the same reason as the guard above: an ordinary `assert!` on constants is
/// both a clippy warning and a check that only runs if someone runs the tests.
const _: () = {
    const X_T4: usize = 6246 * 4170 * 3 * 4;
    const X_T5: usize = 7728 * 5152 * 3 * 4;
    assert!(PrefixCache::MID_BUDGET_BYTES > X_T4);
    assert!(PrefixCache::MID_BUDGET_BYTES < 2 * X_T4);
    assert!(PrefixCache::MID_BUDGET_BYTES > X_T5);
};

impl Default for PrefixCache {
    /// ⚠ Hand-written rather than derived: a derived `Default` would give a budget of **0**,
    /// silently disabling every intermediate while leaving every correctness test passing.
    fn default() -> Self {
        Self::with_mid_budget(Self::MID_BUDGET_BYTES)
    }
}

impl PrefixCache {
    /// One complete prefix output per render mode (fit and native).
    const FINALS_CAP: usize = 2;

    /// Ceiling on the bytes held as intermediates.
    ///
    /// 512 MiB clears one full-resolution checkpoint (~313 MB on a 26 Mpx X-T4) but not two,
    /// which is the intent: the post-NR checkpoint makes a Dehaze drag at 1:1 reuse noise
    /// reduction, and a second checkpoint would double the footprint to save ~200 ms on a
    /// rarer interaction. ⚠ 512 rather than 320 MiB because 320 covers only 28.0 Mpx, and
    /// Fujifilm's current 40.2 Mpx bodies would get *zero* native checkpoints; 512 covers
    /// 44.7 Mpx. On a larger sensor than that, no intermediate survives and behaviour degrades
    /// to re-running the whole prefix — the ceiling working, not failing.
    ///
    /// At preview resolution it is never the binding constraint: entries are ~21 MB and
    /// [`PrefixCache::MIDS_CAP`] bites first.
    const MID_BUDGET_BYTES: usize = 512 << 20;

    /// Ceiling on the *number* of intermediates, independent of their size.
    ///
    /// ⚠ This is what makes a large byte budget safe. Dragging Straighten mints a new
    /// post-geometry key every frame, so without a count cap the tier fills with stale
    /// checkpoints bounded only by bytes — and a bigger budget would simply retain more
    /// garbage. A render's useful chain is at most 3 mids, so 6 keeps the current chain plus
    /// one previous state and discards the rest.
    const MIDS_CAP: usize = 6;

    fn with_mid_budget(mid_budget: usize) -> Self {
        Self {
            finals: Vec::new(),
            mids: Vec::new(),
            mid_budget,
        }
    }

    /// Only the tests ask how full a tier is — production code never does.
    #[cfg(test)]
    fn final_len(&self) -> usize {
        self.finals.len()
    }

    #[cfg(test)]
    fn mid_len(&self) -> usize {
        self.mids.len()
    }

    fn bytes(img: &ImageBuf) -> usize {
        std::mem::size_of_val(img.data())
    }

    /// Look in both tiers, promoting a hit to the front of its own tier.
    fn get(&mut self, key: &CacheKey) -> Option<Arc<ImageBuf>> {
        for tier in [&mut self.finals, &mut self.mids] {
            if let Some(i) = tier.iter().position(|(k, _)| k == key) {
                let hit = tier.remove(i);
                let img = hit.1.clone();
                tier.insert(0, hit);
                return Some(img);
            }
        }
        None
    }

    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>, tier: Tier) {
        // A key lives in exactly one tier, but drop it from both: an edit set whose prefix
        // shrinks (its last stage going neutral) turns a former mid into a final.
        self.finals.retain(|(k, _)| k != &key);
        self.mids.retain(|(k, _)| k != &key);

        match tier {
            Tier::Final => {
                self.finals.insert(0, (key, img));
                self.finals.truncate(Self::FINALS_CAP);
            }
            Tier::Mid => {
                self.mids.insert(0, (key, img));
                // Count first: it is O(1) and usually the binding constraint at preview
                // resolution, where entries are ~21 MB and the byte budget is nowhere near.
                self.mids.truncate(Self::MIDS_CAP);
                let mut total: usize = self.mids.iter().map(|(_, i)| Self::bytes(i)).sum();
                while total > self.mid_budget {
                    match self.mids.pop() {
                        Some((_, evicted)) => total -= Self::bytes(&evicted),
                        None => break,
                    }
                }
            }
        }
    }
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
/// What a cached set of brush coverage maps was built from.
#[derive(PartialEq)]
struct BrushKey {
    /// Every mask's brush **by position** — `None` for the analytic shapes.
    ///
    /// ⚠ All masks, not just the brushes, so the positional index into `BrushCoverage` cannot
    /// drift when a mask is added, removed or changed kind.
    brushes: Vec<Option<edit_model::Brush>>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    /// The rendered region as `(x0, y0, w, h)` in frame pixels, or `None` for the whole frame.
    ///
    /// ⚠⚠ **In the key on purpose, and this is the OPPOSITE of [`PrefixCache`]'s rule.** That
    /// rule — region out of the key, so panning never invalidates — is right for the prefix,
    /// which runs whole-buffer and whose output *is* the frame. Copied to the brush it buys a
    /// cache that survives a pan (invisible, since panning re-runs the whole tail regardless) and
    /// costs a full-frame rasterization on every paint frame: 23.76 ms at 6246x4170, measured,
    /// and reported in use as "very sluggish at 1:1". **Do not "unify" the two caches.**
    ///
    /// ⚠ It also makes a clip mismatch unrepresentable: a map clipped to one region handed to a
    /// `MaskStage` rendering another would sample `outside` across the difference — a mask that
    /// fades out partway across the image rather than an error.
    region: Option<(usize, usize, usize, usize)>,
}

/// Rasterized brush coverage, so an unrelated slider drag does not re-rasterize.
///
/// ⚠ **The region is not in the key**, exactly as [`PrefixCache`]'s is not: rasterize whole-frame
/// and sample the sub-rect, so panning never invalidates.
///
/// ⚠ It misses on every frame *while painting*, and that is correct — the strokes really did
/// change, so there is nothing to reuse. Its job is that dragging Exposure with a brush mask
/// present does not re-rasterize the strokes on every frame of that drag.
#[derive(Default)]
struct BrushCache {
    entries: Vec<(BrushKey, Arc<pipeline::BrushCoverage>)>,
}

impl BrushCache {
    /// ⚠ Two, not one, and for the reason [`PrefixCache::FINALS_CAP`] is two: the frame dimensions
    /// are in the key, so a single entry would be evicted on every zoom change and re-rasterized
    /// coming back — reintroducing at the brush layer the cost that decision removed at the prefix
    /// layer.
    const CAP: usize = 2;

    fn key(
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
    ) -> BrushKey {
        BrushKey {
            brushes: edits
                .masks
                .iter()
                .map(|m| match &m.mask {
                    edit_model::Mask::Brush(b) => Some(b.clone()),
                    _ => None,
                })
                .collect(),
            geometry: edits.geometry,
            fw,
            fh,
            region,
        }
    }

    fn get_or_build(
        &mut self,
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
    ) -> Arc<pipeline::BrushCoverage> {
        let key = Self::key(edits, fw, fh, region);
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            // Promote the hit, so the least recently used entry is the one evicted.
            let hit = self.entries.remove(i);
            let out = Arc::clone(&hit.1);
            self.entries.insert(0, hit);
            return out;
        }
        let maps = edits
            .masks
            .iter()
            .map(|m| match &m.mask {
                edit_model::Mask::Brush(b) => Some(Arc::new(pipeline::brush::rasterize_in(
                    b,
                    &edits.geometry,
                    fw,
                    fh,
                    region,
                ))),
                _ => None,
            })
            .collect();
        let built = Arc::new(pipeline::BrushCoverage { maps });
        self.entries.insert(0, (key, Arc::clone(&built)));
        self.entries.truncate(Self::CAP);
        built
    }
}

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
) -> (RenderedFrame, usize) {
    // Destructured so the body below reads exactly as it did before these three were grouped.
    let FrameSpec {
        max_dim,
        region,
        display_space,
        resampler,
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
    let steps = build_prefix_steps(edits, scale, lens, resampler);
    let key_at = |n: usize| (PrefixKey::upto(&steps, n, scale), max_dim);
    let tier_at = |n: usize| {
        if n == steps.len() {
            Tier::Final
        } else {
            Tier::Mid
        }
    };

    // Resume from the deepest checkpoint still held. `n == 0` is the bare downscale.
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

    // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU seam
    // clean — so each step that runs clones the checkpoint it starts from. Measured at 0.5 ms
    // (fit) and 2.1 ms (native) against the 13–117 ms each avoided stage costs.
    for (i, step) in steps.iter().enumerate().skip(start) {
        img = Arc::new(step.stage.apply((*img).clone()));
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
    let (input, region) = match rect {
        Some(r) => (
            img.crop(r.x, r.y, r.w, r.h),
            Some(Region {
                full_w: pw,
                full_h: ph,
                x0: r.x,
                y0: r.y,
            }),
        ),
        None => ((*img).clone(), None),
    };

    // ⚠ The frame dimensions are the PREFIX OUTPUT's — `(pw, ph)` — not the region's. That image
    // *is* the frame. Passing the region's would rasterize at the wrong size and, because the dims
    // are in the key, would also thrash the cache on every pan.
    // ⚠ The clip comes from `rect`, which carries the region's own width and height. `Region`
    // holds only the origin plus the full frame's size, so deriving the size as "frame minus
    // origin" would clip to the bottom-right corner of the frame rather than to the viewport —
    // larger than intended, and silently so, because the output would still be correct and merely
    // slow.
    let clip = rect.map(|r| (r.x, r.y, r.w, r.h));
    let coverage = brush_cache.get_or_build(edits, pw, ph, clip);
    let build_tail = |space| {
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            scale,
            region,
            Some(&coverage),
            pipeline::RenderSpec {
                space,
                // ⚠ The tail resamples nothing — every resampling stage is in the prefix — so this
                // is carried for consistency rather than because it changes a pixel here.
                resampler,
                baseline: wb_baseline,
            },
            // Lens correction lives in the *prefix*, which this call discards — only the tail is
            // wanted here. `None` is therefore correct rather than provisional.
            None,
            // ⚠ The display rendering, by contrast, lives in the **tail** — so this must be
            // carried, or every render would silently take the built-in curve and the built-in
            // baseline while a profile that declares its own sat unused.
            profile,
        );
        tail
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
        (
            build_tail(color::OutputSpace::ProPhoto).run(input.clone()),
            p,
        )
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
                },
                None,
                profile,
            );
            let mid = halves.before_grade.run(input);
            // ⚠ Blending and Balance are read from the edits whether or not the grade is
            // active: the ranges exist before a wheel is turned.
            let g = &edits.color_grading;
            let plane = crate::grade_view::coverage(&mid, range, g.blending, g.balance);
            (halves.from_grade.run(mid), Some(Arc::new(plane)))
        }
        None => (build_tail(color::OutputSpace::Srgb).run(input), None),
    };
    let mut frame = pack(out, proofed, region, max_dim, display_space, visualize);
    frame.grade_view = plane;
    (frame, ran)
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
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    pack(
        pipeline::build_pipeline_with(edits, scale, resampler).run(preview),
        // ⚠ Never proofed. This is the before/after baseline and the reference the cached path is
        // tested against; a proof is a property of the *view* and would make the reference move
        // with a menu setting.
        None,
        None,
        max_dim,
        display_space,
        None,
    )
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
    fn varied_sized(n: usize) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(n, n);
        for y in 0..n {
            for x in 0..n {
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

    fn varied() -> Arc<ImageBuf> {
        varied_sized(16)
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
        assert_eq!(cold, 3, "a cold render runs every prefix stage");

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
        assert_eq!(hit, 0, "an unchanged prefix runs nothing");

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
        assert_eq!(ran, 0, "a tail change must not touch the prefix");

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
        assert_eq!(ran, 1, "a dehaze change must resume after noise reduction");

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
        assert_eq!(ran, 2, "an NR change must resume after geometry");

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
        assert_eq!(ran, 3, "a geometry change invalidates the whole prefix");
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
        assert_eq!(ran, 0, "the post-NR checkpoint is now the final one");
        assert_eq!(
            got.rgba,
            render_once(
                &buf,
                &without,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default()
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
        // Insertion order within one render is shallow -> deep, so the most recent insert is
        // the deepest checkpoint. Evicting the LRU is therefore evicting the shallowest.
        assert!(
            cache.get(&fake_key(2)).is_some(),
            "the deepest must survive"
        );
        assert!(cache.get(&fake_key(1)).is_some());
        assert!(
            cache.get(&fake_key(0)).is_none(),
            "the shallowest goes first"
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
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.1, 0.1],
                    to: [0.9, 0.8],
                    invert: false,
                }),
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
            }],
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

    #[test]
    fn every_cached_frame_matches_a_cold_render() {
        // The master test: caching must be invisible in the output. The sequence walks a pure
        // hit, a tail-only change (resumes at the final), a dehaze change (resumes after NR),
        // an NR change (resumes after geometry), a geometry change (resumes at the seed), and
        // returns to earlier states so a stale checkpoint would show up.
        let buf = varied();
        let a = full_prefix();
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // tail only
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
                    pipeline::Resampler::default()
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
                    }],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        }
    }

    #[test]
    fn the_brush_cache_hits_and_keeps_both_zoom_levels() {
        // ⚠ Every other test in this file would pass with the brush cache deleted, because it is
        // a caching change and nothing else. This one asserts identity of the Arc, which is the
        // only observable difference between a hit and a rebuild.
        let mut cache = BrushCache::default();
        let edits = brush_edits();
        let a = cache.get_or_build(&edits, 200, 100, None);
        let b = cache.get_or_build(&edits, 200, 100, None);
        assert!(Arc::ptr_eq(&a, &b), "the second call must hit");

        let c = cache.get_or_build(&edits, 400, 200, None);
        assert!(!Arc::ptr_eq(&a, &c), "different frame dims must miss");

        // ⚠ The assertion that pins CAP = 2 rather than 1. With a single entry the 400x200 build
        // would have evicted the 200x100 one, and this would rebuild — which is exactly the zoom
        // thrash the capacity exists to prevent.
        let d = cache.get_or_build(&edits, 200, 100, None);
        assert!(Arc::ptr_eq(&a, &d), "CAP=2 must keep both zoom levels warm");
    }

    #[test]
    fn changing_a_stroke_misses_the_brush_cache() {
        // The other half: a hit must depend on the strokes, not merely on the dimensions.
        let mut cache = BrushCache::default();
        let a = cache.get_or_build(&brush_edits(), 200, 100, None);
        let mut edited = brush_edits();
        if let edit_model::Mask::Brush(b) = &mut edited.masks[0].mask {
            b.strokes[0].points.push([0.8, 0.6]);
        }
        let c = cache.get_or_build(&edited, 200, 100, None);
        assert!(!Arc::ptr_eq(&a, &c), "an extra point must miss");
    }

    #[test]
    fn the_coverage_index_follows_the_mask_position_not_the_active_count() {
        // ⚠ An INACTIVE mask still owns a slot. A counter over active masks would misalign every
        // map after the first inactive one — the same sparse-versus-dense index bug this codebase
        // has hit three times before.
        let mut edits = brush_edits();
        let inactive = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            adjustments: edit_model::LocalAdjustments::default(),
        };
        edits.masks.insert(0, inactive);
        let mut cache = BrushCache::default();
        let cov = cache.get_or_build(&edits, 200, 100, None);
        assert!(cov.get(0).is_none(), "slot 0 is the analytic mask");
        assert!(cov.get(1).is_some(), "slot 1 is the brush");
    }

    #[test]
    fn the_brush_cache_misses_when_the_region_moves() {
        // ⚠⚠ This REVERSES the rule `PrefixCache` follows, deliberately. Keeping the region out
        // of the key buys a cache that survives a pan — invisible, because panning re-runs the
        // whole tail anyway — and costs a full-frame rasterization on every paint frame:
        // 23.76 ms at 6246x4170, measured, and reported in use as "very sluggish at 1:1".
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let a = cache.get_or_build(&e, 200, 100, Some((0, 0, 50, 50)));
        let b = cache.get_or_build(&e, 200, 100, Some((0, 0, 50, 50)));
        assert!(Arc::ptr_eq(&a, &b), "the same region must hit");
        let c = cache.get_or_build(&e, 200, 100, Some((20, 0, 50, 50)));
        assert!(!Arc::ptr_eq(&a, &c), "a moved region must miss");
    }

    #[test]
    fn the_brush_cache_still_hits_when_only_a_slider_moves() {
        // The cache's remaining job, and the reason it exists at all: dragging an unrelated
        // slider with a brush mask present must not re-rasterize on every frame.
        let mut cache = BrushCache::default();
        let a = cache.get_or_build(&brush_edits(), 200, 100, None);
        let mut louder = brush_edits();
        louder.exposure = 1.5;
        let b = cache.get_or_build(&louder, 200, 100, None);
        assert!(
            Arc::ptr_eq(&a, &b),
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
        let cov = cache.get_or_build(&e, 200, 100, Some((0, 0, 40, 100)));
        let map = cov.get(0).expect("the brush mask has a map");
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
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::DisplayP3,
            pipeline::Resampler::default(),
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
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::DisplayP3,
            pipeline::Resampler::default(),
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
        let plain = pack(buf.clone(), None, None, 16, color::OutputSpace::Srgb, None);
        let vis = pack(buf, None, None, 16, color::OutputSpace::Srgb, Some(12.0));
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
        let plain = pack(buf.clone(), None, None, 16, color::OutputSpace::Srgb, None);
        let vis = pack(buf, None, None, 16, color::OutputSpace::Srgb, Some(12.0));
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

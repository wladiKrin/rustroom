//! What a render is asked for and what it hands back: the frame, its spec, the soft proof, the counters.

use super::RegionRect;
use pipeline::Region;
use std::sync::Arc;

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
    /// How colour the proof space cannot hold is brought inside it — the canvas's intent.
    pub intent: color::RenderingIntent,
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
    pub display_space: color::Display,
    /// The canvas's rendering intent (M6.4c). ⚠ Per request, for `resampler`'s reason below; a
    /// tail-only property, so in no `PrefixStepKey`.
    pub intent: color::RenderingIntent,
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
            display_space: color::Display::SRGB,
            intent: color::RenderingIntent::default(),
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
    /// How many uploads the GPU prefix steps paid, the resident input's included.
    ///
    /// ⚠ An observable for the resident input (2026-09-26): a drag frame after the first must
    /// read 0, and nothing else in the frame can tell a resident hit from an upload.
    pub gpu_prefix_uploads: usize,
    /// How many prefix steps kept their output on the GPU for the next one, with no download (M10.14f). ⚠ An
    /// observable: a carried and a downloaded step render the same picture by design.
    pub gpu_prefix_carried: usize,
    /// Whether the last prefix step handed its output to the tail on the GPU, with no download
    /// and therefore no final checkpoint. See [`render_cached_with`].
    pub handoff: bool,
    /// How many uploads the TAIL's chain paid. ⚠ The resident final's observable (2026-09-26): a
    /// tail-slider frame after the first must read 0.
    pub tail_uploads: usize,
    /// Whether the tail ran from the resident final rather than from a CPU image.
    pub resident_final_hit: bool,
}

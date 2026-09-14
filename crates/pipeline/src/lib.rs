//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

pub mod brush;

/// The two pixel measurements a mask's coverage is taken on, for a caller that must agree with it.
///
/// ⚠⚠ **Re-exported by name rather than by opening `stages`**, the rule the hue LUT already
/// follows: `pub mod stages` would make every stage public API for the sake of two functions.
///
/// ⚠ The canvas tint needs exactly these. A second lightness or hue convention there would be
/// invisible per widget and disagree with the render everywhere — the failure `color::luminance`
/// exists to prevent for the pipeline itself.
pub use stages::mask_coverage::{hue_chroma, lightness};

/// Pre-rasterized coverage for a photo's brush masks, indexed by **`[mask][component]`** —
/// position in `Edits::masks`, then position within that mask's composite, component 0 being its
/// base shape.
///
/// ⚠ Optional and additive, following the `Region` precedent: [`build_pipeline`] and
/// [`build_pipeline_split`] delegate with `None`, so the CLI, export and every golden compile
/// untouched.
#[derive(Default)]
pub struct BrushCoverage {
    /// ⚠ Nested because a composite can hold a brush in **any** component. Flattening with a
    /// prefix sum was the alternative and was rejected: the sum would have to be recomputed
    /// identically by the stage and by the cache, and two derivations of one index is precisely
    /// how a sparse-versus-dense bug enters — this codebase has had four.
    pub maps: Vec<Vec<Option<std::sync::Arc<brush::CoverageMap>>>>,
    /// Where a stage reports coverage it had to build itself.
    ///
    /// ⚠⚠ **The read side is what a caller supplies; this is what a stage gives back.** A brush
    /// that gates on the image can only be rasterized where the image is — inside
    /// `MaskStage::apply` — so `render_engine`, which owns the cache key, would otherwise never
    /// see the map it wants to keep. See the coverage-cache spec's §4.0.
    ///
    /// ⚠ `Mutex` rather than `RefCell`: this crosses into stages that must stay `Sync`. It is
    /// uncontended — `apply` runs sequentially, once per mask per tail.
    pub built: Vec<Vec<std::sync::Mutex<Option<std::sync::Arc<brush::CoverageMap>>>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i)?.get(j).and_then(|m| m.clone())
    }

    /// This mask's whole component row, or an empty slice when it has none.
    pub fn row(&self, i: usize) -> &[Option<std::sync::Arc<brush::CoverageMap>>] {
        self.maps.get(i).map_or(&[], |r| r.as_slice())
    }

    /// A coverage set with one empty deposit slot per component. `counts[i]` is mask `i`'s
    /// component count — `1 + refine.len()`.
    pub fn with_slots(counts: &[usize]) -> Self {
        Self {
            maps: counts.iter().map(|&n| vec![None; n]).collect(),
            built: counts
                .iter()
                .map(|&n| (0..n).map(|_| std::sync::Mutex::new(None)).collect())
                .collect(),
        }
    }

    /// Report coverage built at component `j` of mask `i`. Out of range is ignored.
    ///
    /// ⚠ Ignored rather than panicking: the mask list can change between this set being made and a
    /// stage running, and a render is not worth aborting over a slot that no longer exists.
    pub fn deposit(&self, i: usize, j: usize, map: std::sync::Arc<brush::CoverageMap>) {
        if let Some(slot) = self.built.get(i).and_then(|r| r.get(j)) {
            if let Ok(mut g) = slot.lock() {
                *g = Some(map);
            }
        }
    }

    /// Take what was reported at component `j` of mask `i`, clearing the slot.
    ///
    /// ⚠ Clearing is what stops a map surviving into a render it does not describe — the caller
    /// stores it under a key computed for *this* render.
    pub fn take_built(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.built
            .get(i)?
            .get(j)
            .and_then(|s| s.lock().ok()?.take())
    }
}
pub mod auto_tone;
/// ⚠ A **`.cube` parser and sampler, with no image and no stage** — deliberately separate from
/// `stages::creative_look`, which owns the colour-space shaping. Keeping the table pure means the
/// index-order and interpolation guards can be written against a string literal, with no fixture
/// on disk and no working-space assumptions to get wrong.
pub mod lut;
pub mod perspective;
/// ⚠ Its own module rather than a section of [`spatial`]: that one holds *separable blur*
/// primitives (box, Gaussian, guided) and is already 889 lines. A resampling kernel reads at a
/// fractional coordinate, which is a different operation with a different API.
pub mod resample;
pub mod spatial;
pub(crate) mod spline;
mod stages;
/// ⚠ The one stage-internal module that is public, re-exported rather than opening `stages`
/// wholesale — which would make every stage public API for the sake of one benchmark. `bench_hsl`
/// needs the table and the small-angle pair to sweep table sizes against real pixels.
pub use stages::hue_table;

/// The dehaze airlight of an image, as [`stages::dehaze`] estimates it.
///
/// ⚠ Re-exported by name rather than opening `stages`, the rule `hue_table` already follows: a
/// `pub mod stages` would make every stage public API for the sake of one probe.
pub use stages::dehaze::airlight as dehaze_airlight;

pub use resample::Resampler;
pub use stages::color_grading::ColorGradingStage;
pub use stages::contrast::{ContrastStage, PIVOT as CONTRAST_PIVOT};
pub use stages::creative_look::CreativeLookStage;
pub use stages::dehaze::DehazeStage;
// ⚠⚠ Re-exported so `gpu-pipeline` substitutes these into WGSL rather than transcribing them.
// ⚠ `T_MIN` and `A_MIN` are ALIASED on the way out: they are dehaze's own floors and a bare name
// would give a reader no way to tell which stage they belong to.
pub use stages::dehaze::{
    airlight, scaled_radius, AIRLIGHT_TOLERANCE, A_MIN as DEHAZE_A_MIN, OMEGA, PATCH_RADIUS_FULL,
    REFINE_EPS, REFINE_RADIUS_FULL, T_MIN as DEHAZE_T_MIN,
};
pub use stages::display_rendering::DisplayRenderingStage;
/// ⚠ Re-exported by name rather than opening `stages`, the rule `hue_table` set: a GPU shader
/// must upload these rather than re-type them, and two copies of a tuned constant is how the two
/// come to disagree about a number nobody re-derives.
pub use stages::display_rendering::{CHROMA_BOOST, SHADOW_KNEE};
pub use stages::exposure::ExposureStage;
pub use stages::geometry::GeometryStage;
pub use stages::hsl::HslStage;
pub use stages::lens_correction::LensCorrectionStage;
pub use stages::local_contrast::LocalContrastStage;
// ⚠⚠ Re-exported so `gpu-pipeline` substitutes these into WGSL rather than transcribing them —
// `color`'s own doc records six copies of a Rec.709 triple that all went wrong on one day.
// ⚠ `LUMA_EPS` and `GAIN_MAX` are ALIASED on the way out: `stages::sharpening` has constants of
// both names with different values, and a reader hitting a bare `pipeline::LUMA_EPS` would have
// to guess which stage it belongs to.
pub use stages::local_contrast::{
    texture_guided_radius, CLARITY_HALO_FLOOR, CLARITY_HALO_KNEE, CLARITY_SIGMA,
    GAIN_MAX as LOCAL_CONTRAST_GAIN_MAX, LUMA_EPS as LOCAL_CONTRAST_LUMA_EPS, MID_GRAY,
    TEXTURE_GUIDED_EPS, TEXTURE_SIGMA,
};
pub use stages::mask::MaskStage;
pub use stages::noise_reduction::NoiseReductionStage;
// ⚠⚠ Re-exported so `gpu-pipeline` SUBSTITUTES these into WGSL rather than transcribing them.
// A substituted constant cannot disagree; a transcribed one is merely checked for agreeing —
// and `color`'s own doc records six copies of a Rec.709 luma triple that all went wrong on one
// day. The same rule the `dehaze` block above follows.
// ⚠ `CHROMA_PASSES` is ALIASED on the way out: a bare name gives a reader no way to tell which
// stage's passes it counts, and `display_rendering` already exports a `CHROMA_BOOST` beside it.
pub use stages::noise_reduction::{
    chroma_radius_full, eps_for_detail, CHROMA_PASSES as NR_CHROMA_PASSES, COL_RADIUS_FULL,
    LUM_RADIUS_FULL,
};
pub use stages::output_transform::OutputTransform;
pub use stages::saturation::{SaturationStage, EPS as SATURATION_EPS};
pub use stages::sharpening::{max_kernel_radius, SharpenStage, RADIUS_RANGE};
pub use stages::spot_removal::SpotRemovalStage;
pub use stages::tone_curve::ToneCurveStage;
pub use stages::tone_regions::ToneRegionsStage;
pub use stages::vignette::{vignette_params, VignetteParams, VignetteStage};
pub use stages::white_balance::WhiteBalanceStage;

/// Ratio of the image being rendered to the full-resolution original: `1.0` at full res,
/// ≈0.27 for the 1600 px preview of a 6000 px file.
///
/// Spatial stages multiply their pixel radii by this, so a preview approximates the
/// export instead of contradicting it. Per-pixel stages ignore it entirely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderScale(f32);

impl RenderScale {
    /// Rendering at the original resolution.
    pub const FULL: RenderScale = RenderScale(1.0);

    /// Clamped to `(0, 1]`. Anything outside that (or non-finite) is a caller bug, but a
    /// clamped radius beats panicking on the render worker thread.
    pub fn new(scale: f32) -> Self {
        if scale.is_finite() {
            RenderScale(scale.clamp(1e-3, 1.0))
        } else {
            RenderScale::FULL
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// One stage of the develop pipeline. Pure: no shared state, independently testable.
/// (A future GPU stage implements the same trait — the seam for the Phase 5 port.)
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, image: ImageBuf) -> ImageBuf;

    /// Offer this stage's inner sub-stages for acceleration. Default: this stage has none.
    ///
    /// ⚠⚠ **`&mut self`, not `self: Box<Self>`.** `gpu_pipeline::accelerate` already owns each
    /// box from [`Pipeline::into_stages`], so the stage can `mem::take` its list, hand it over
    /// and put the result back — no cloning, no reconstruction. A `Box<Self>` receiver is
    /// object-safe but **cannot carry a default body**: returning `self` as
    /// `Box<dyn PipelineStage>` needs an unsize coercion, hence `Self: Sized`, which is exactly
    /// the bound that makes a method undispatchable on `dyn`.
    ///
    /// ⚠ The accelerator is handed the [`edit_model::LocalAdjustments`] the sub-stages were built
    /// from, because a GPU stage is built from *parameters* and a `dyn PipelineStage` cannot be
    /// asked for its own. Reading the **global** `Edits` instead is the failure this signature
    /// exists to prevent, and it would render a plausible photograph rather than an error.
    fn accelerate_inner(&mut self, _f: &SubStageAccelerator<'_>) {}

    /// The names of this stage's inner sub-stages, in run order. Default: none.
    ///
    /// ⚠⚠ **The only way to see what [`PipelineStage::accelerate_inner`] did without comparing
    /// pixels** — and pixels cannot see it, because an accelerated mask and a CPU one render the
    /// same picture by design. Every equivalence test would therefore pass against a hook that
    /// never fired, which is precisely how creative LUTs (W5) shipped fully tested and 100%
    /// non-functional.
    ///
    /// ⚠ M8.3g rejected a trait method for the byte-path question on the grounds that it leaks an
    /// **output-encoding** concern into a seam deliberately free of them. This one reports
    /// **composition**, which the trait already exposes a mutator for one method above; a trait
    /// you can change and cannot observe is worse than one carrying two views of one fact.
    fn sub_stage_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Offer this stage a coverage rasteriser. Default: this stage builds no coverage.
    ///
    /// ⚠ A second defaulted hook rather than a parameter on [`PipelineStage::accelerate_inner`]:
    /// that one transforms a **stage list** and this hands over a **collaborator**, so folding them
    /// together would give one method two unrelated jobs.
    fn set_coverage_rasterizer(&mut self, _r: &std::sync::Arc<dyn CoverageRasterizer>) {}

    /// Whether this stage holds a coverage rasteriser.
    ///
    /// ⚠⚠ **The observable, and it has to exist for the same reason [`Self::sub_stage_names`]
    /// does.** A coverage rasteriser changes no pixel a test can read — the GPU and CPU maps agree
    /// to within one level by design — so without this the offer could be deleted and every test
    /// in the workspace would stay green. That is the W5 failure mode exactly.
    fn has_coverage_rasterizer(&self) -> bool {
        false
    }

    /// This stage's output dimensions for an input of `w` x `h`. Default: unchanged.
    ///
    /// ⚠⚠ **The caller needs this BEFORE the stage runs.** A region-scoped render never produces
    /// the whole frame, and the frame's dimensions are what every brush mask's coverage is
    /// rasterised at. Taking them off a region-scoped buffer would rasterise every mask at the
    /// region's size — a wrong mask, and a coverage cache that thrashes on every pan.
    ///
    /// ⚠ The identity default is correct for every stage in the tree today: lens correction,
    /// perspective, spot removal, noise reduction, dehaze and the masked capture corrections all
    /// preserve dimensions, and only `GeometryStage` does not. ⚠⚠ It is silently wrong for a
    /// future dim-changing stage, which is what
    /// `every_prefix_stages_output_dims_match_what_it_produces` exists to catch.
    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        (w, h)
    }

    /// Produce only `rect` of this stage's output. Default: this stage cannot.
    ///
    /// ⚠⚠ **`None` is what makes every other stage decline BY CONSTRUCTION.** Dehaze's airlight is
    /// estimated from the whole frame and noise reduction reads a neighbourhood, so neither can be
    /// region-scoped without changing the photograph as you pan. Inheriting the default is what
    /// stops a caller name-checking its way into that. The rule `GpuStage::cpu()` and
    /// `accelerate_prefix_stage` already follow: **fail toward the whole-buffer path**, where a
    /// wrong answer costs milliseconds and never pixels.
    ///
    /// ⚠ It **borrows** where [`Self::apply`] consumes, and that is load-bearing rather than tidy:
    /// a gather only reads its source, and the consuming signature would force a clone of the whole
    /// checkpoint — measured at 8.4 ms on a 26 Mpx buffer, most of the frame this exists to shrink.
    ///
    /// ⚠ `rect` is in the stage's **output** coordinates, so a caller must resolve it against
    /// [`Self::output_dims`] rather than against the input.
    fn apply_region(&self, _src: &RegionSrc<'_>, _rect: Rect) -> Option<ImageBuf> {
        None
    }

    /// The input rect this stage needs in order to produce `out` of its output.
    ///
    /// ⚠⚠ **`None` is the default, so every stage declines BY CONSTRUCTION.** It is the mirror of
    /// [`Self::apply_region`] and carries the same rule: **fail toward the whole-buffer path**,
    /// where a wrong answer costs milliseconds and never pixels. Lens correction, spot removal and
    /// the masked captures all decline without the caller name-checking any of them.
    ///
    /// ⚠⚠ **A stage implementing one of the pair and not the other is a bug.** `region_plan` would
    /// promise an input rect for an output `apply_region` then refuses to produce — which is why
    /// `GeometryStage` declines *both* on a non-identity orientation rather than only one.
    ///
    /// ⚠ `out` is in this stage's **output** coordinates and the result is in its **input**
    /// coordinates, which for a dimension-changing stage are different spaces. Resolve `out`
    /// against [`Self::output_dims`] first.
    ///
    /// ⚠ Implementations clip the result into `in_w` x `in_h` themselves: a margin larger than the
    /// image is ordinary at low render scales, and [`Rect::grown`] does it.
    fn input_rect_for(&self, _out: Rect, _in_w: usize, _in_h: usize) -> Option<Rect> {
        None
    }
}

/// Builds a brush's coverage from an already-planned dab list.
///
/// ⚠⚠ **This exists because `MaskStage` cannot call `gpu-pipeline`.** That crate depends on
/// `pipeline` and its own header records that nothing may depend on it — so a GPU rasteriser is
/// **injected** rather than referenced, exactly as [`SubStageAccelerator`] is and for the same
/// reason. A `pipeline` that reached upward would put wgpu into the CLI, `export` and all 63
/// goldens.
///
/// ⚠ `Send + Sync` because [`PipelineStage`] is, and a mask's coverage is built inside a stage
/// that then blends through `map_pixels_xy`'s rayon rows.
pub trait CoverageRasterizer: Send + Sync {
    /// `None` declines, and the caller falls back to the CPU rasteriser.
    ///
    /// ⚠⚠ **Declining must always be safe, and every implementation should prefer it when
    /// unsure.** An adapter that cannot allocate, a box too small to repay a round trip, or a plan
    /// this implementation does not handle all return `None` and cost only the attempt. **Fail
    /// toward the CPU** — `GpuStage::cpu_cost`'s rule and `PrefixKey`'s, because a wrong answer
    /// here is wrong pixels rather than lost milliseconds.
    /// Called once at the top of every mask's `apply`, before any coverage is built.
    ///
    /// ⚠⚠ **This exists so an implementation can drop state scoped to ONE apply, and without it a
    /// shared GPU plane would be taken by the wrong frame.** A frame whose coverage comes from
    /// `BrushCache`'s prebuilt map rasterises nothing and so deposits nothing — and a sub-stage
    /// chain that then found a *previous* apply's plane still resident would render those pixels.
    /// The failure is a plausible photograph, not an error.
    ///
    /// ⚠ Deliberately **not** folded into [`Self::rasterize`]: a mask calls this even when it goes
    /// on to rasterise nothing at all, which is exactly the case that makes it necessary.
    ///
    /// ⚠ Defaulted to nothing, so an implementation holding no per-apply state ignores it.
    fn begin_mask(&self) {}

    fn rasterize(
        &self,
        plan: &brush::DabPlan,
        invert: bool,
        base: Option<brush::BaseRef<'_>>,
    ) -> Option<brush::CoverageMap>;
}

/// Transform a stage's inner sub-stage list, given the adjustments it was built from.
///
/// ⚠ The alias carries a lifetime because the closure `gpu-pipeline` supplies **borrows**: it
/// holds the outer `ProfileRendering<'_>` and a `&Arc<GpuContext>`. A `'static` alias would force
/// those to be owned, and `ProfileRendering` is a borrow by construction.
pub type SubStageAccelerator<'a> = dyn Fn(
        Vec<Box<dyn PipelineStage>>,
        &edit_model::LocalAdjustments,
        RenderScale,
    ) -> Vec<Box<dyn PipelineStage>>
    + 'a;

/// An ordered pipeline of stages.
pub struct Pipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl Pipeline {
    pub fn new(stages: Vec<Box<dyn PipelineStage>>) -> Self {
        Self { stages }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Stage names in run order. Useful when debugging a render, and it lets tests assert
    /// pipeline *order* rather than just its length.
    pub fn stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// The sub-stage names of the first stage with this name, if there is one.
    ///
    /// ⚠ [`Pipeline::stage_names`] reports the top level only, and a `Box<dyn PipelineStage>`
    /// cannot be downcast — so without this a test can see *that* a pipeline contains a mask and
    /// nothing at all about what the mask contains, which is exactly where M8.3j's work happens.
    /// `None` means no stage of that name; an empty `Vec` means one carrying no sub-stages.
    /// Whether the first `mask` stage holds a coverage rasteriser.
    ///
    /// ⚠ The observable for M8.3k's wiring — see [`PipelineStage::has_coverage_rasterizer`] for
    /// why a pixel comparison cannot serve.
    pub fn mask_has_coverage_rasterizer(&self) -> bool {
        self.stages
            .iter()
            .any(|s| s.name() == "mask" && s.has_coverage_rasterizer())
    }

    pub fn sub_stage_names_of(&self, name: &str) -> Option<Vec<&'static str>> {
        self.stages
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.sub_stage_names())
    }

    /// Consume the pipeline, yielding its stages — lets `build_pipeline` concatenate the two
    /// halves of [`build_pipeline_split`].
    pub fn into_stages(self) -> Vec<Box<dyn PipelineStage>> {
        self.stages
    }

    /// Run all stages in order.
    pub fn run(&self, mut image: ImageBuf) -> ImageBuf {
        for stage in &self.stages {
            image = stage.apply(image);
        }
        image
    }
}

/// Where a cropped render sits inside the full frame.
///
/// `None` at a call site means "this buffer *is* the full frame" — what every existing caller
/// implicitly passes, and what position-dependent stages already assume. Only stages whose
/// output depends on absolute position need it; today that is the vignette alone, but any
/// future graduated or radial filter will want the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub full_w: usize,
    pub full_h: usize,
    pub x0: usize,
    pub y0: usize,
}

/// A pixel rectangle, in whichever coordinate space the caller names.
///
/// ⚠ Deliberately **not** [`Region`]: that type carries an origin plus the *frame's* size and takes
/// its own width and height from the buffer it describes. A stage producing a sub-rect cannot do
/// that, because the buffer does not exist yet — the same trap `render_cached`'s `clip` comment
/// already records for deriving a size as "frame minus origin".
///
/// ⚠⚠ **Deliberately not called `Rect` either, which is what it was until M8.7.**
/// [`PipelineStage::apply_region`] takes an *output* rect and [`PipelineStage::input_rect_for`]
/// returns an *input* one; a type whose name asserts one of those reads as a bug in the other
/// position. It also replaced `app::RegionRect`, which was this same struct with different field
/// names — two types that could not disagree because they were the same type twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    /// This rect clipped into a `w` x `h` image, or `None` if nothing survives.
    ///
    /// ⚠ Moved here from `app::RegionRect` so the one clipping rule lives beside the type rather
    /// than in whichever crate happened to need it first.
    pub fn clamped(self, w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let x = self.x.min(w - 1);
        let y = self.y.min(h - 1);
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self { x, y, w: cw, h: ch })
    }

    /// This rect grown by `m` pixels on every side, clipped into `w` x `h`.
    ///
    /// ⚠ Saturating on the low side and clipping on the high side, so a margin larger than the
    /// image yields the whole image rather than an empty rect or a panic. A margin larger than the
    /// image is **ordinary** at low render scales, not a pathological case.
    pub fn grown(self, m: usize, w: usize, h: usize) -> Self {
        let x = self.x.saturating_sub(m);
        let y = self.y.saturating_sub(m);
        let x1 = (self.x + self.w + m).min(w);
        let y1 = (self.y + self.h + m).min(h);
        Self {
            x,
            y,
            w: x1.saturating_sub(x),
            h: y1.saturating_sub(y),
        }
    }
}

/// A stage's input when only part of it exists.
///
/// ⚠⚠ **`buf` is a sub-rect of a larger logical image, and both other fields are needed.**
/// `origin` says where `buf` sits in that image and `full` says how big the image is — a stage
/// whose result depends on *where* a pixel is (the rotation's centre, the homography's
/// normalisation) is wrong without them, and wrong as a plausible photograph of the wrong part of
/// the frame rather than as an error. That is M8.6's centre sabotage, which all 63 goldens were
/// blind to.
///
/// ⚠ The whole-buffer case is `origin == (0, 0)` with `full` equal to `buf`'s dimensions, which is
/// what makes M8.6's two implementations generalise rather than be rewritten.
#[derive(Debug, Clone, Copy)]
pub struct RegionSrc<'a> {
    pub buf: &'a ImageBuf,
    pub origin: (usize, usize),
    pub full: (usize, usize),
}

impl<'a> RegionSrc<'a> {
    /// The whole-buffer case: this buffer *is* the whole image.
    pub fn whole(buf: &'a ImageBuf) -> Self {
        Self {
            buf,
            origin: (0, 0),
            full: (buf.width(), buf.height()),
        }
    }

    /// The sample at a point in **full-image** coordinates, or `None` if `buf` does not hold it.
    ///
    /// ⚠ Returning `None` rather than clamping is deliberate: a stage reaching outside what
    /// [`region_plan`] promised is a bug in some `input_rect_for`, and clamping would hide it
    /// behind a plausible edge pixel.
    pub fn at(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        let lx = x.checked_sub(self.origin.0)?;
        let ly = y.checked_sub(self.origin.1)?;
        (lx < self.buf.width() && ly < self.buf.height()).then(|| self.buf.pixel(lx, ly))
    }
}

/// What each scoped step must produce, and the frame the whole path would have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionPlan {
    /// One output rect per step in `steps[start..]`, in that order.
    pub out_rects: Vec<Rect>,
    /// The **full-image** dimensions each step's input has, `dims[i]` for `steps[start + i]`.
    ///
    /// ⚠⚠ The executor needs these to set each chained source's `full`, and they are *logical*
    /// dimensions rather than the size of the slice in hand. Deriving them from the buffers as the
    /// chain runs is precisely the bug `RegionSrc` exists to prevent.
    pub dims: Vec<(usize, usize)>,
    /// The dimensions the **whole-buffer** path would end with.
    ///
    /// ⚠⚠ Composed through [`PipelineStage::output_dims`] and read **before** any step runs.
    /// These feed `CoverageSpec { fw, fh }` and the tail's [`Region`], so taking them off a
    /// region-scoped buffer would rasterise every brush mask at the region's size and thrash the
    /// coverage cache on every pan. M8.6's sharpest hazard, generalised to a chain.
    pub frame: (usize, usize),
}

/// Plan a region-scoped run of `steps[start..]` that ends by producing `visible`.
///
/// Walks **backwards** from `visible`, asking each step what input it needs
/// ([`PipelineStage::input_rect_for`]) and making that the previous step's output.
///
/// ⚠⚠ **Any decline abandons the whole plan**, and that is structural rather than conservative: a
/// step that must run whole-buffer needs a whole-buffer input, so every step above it must produce
/// the full frame too. One `None` anywhere at or after `start` therefore kills everything. Two
/// consequences on real chains: spots block a Perspective drag from scoping above `geometry`, and
/// a masked capture — which is **last** — switches the feature off for that photo entirely.
///
/// ⚠ An empty range is `None` as well. Nothing to scope is **no plan**, not an empty one, so the
/// caller takes the whole-buffer path rather than believing it scoped something.
///
/// ⚠ Pure, and takes plain rects on purpose: margins, back-projection, composition and the abandon
/// rule are then testable with no render engine and no window.
pub fn region_plan(
    steps: &[PrefixStep],
    start: usize,
    visible: Rect,
    in_w: usize,
    in_h: usize,
) -> Option<RegionPlan> {
    if start >= steps.len() || visible.w == 0 || visible.h == 0 {
        return None;
    }
    // Forward first: the dimensions each step sees, and the frame the path ends at.
    let mut dims = Vec::with_capacity(steps.len() - start + 1);
    let (mut w, mut h) = (in_w, in_h);
    dims.push((w, h));
    for s in &steps[start..] {
        let (ow, oh) = s.stage.output_dims(w, h);
        dims.push((ow, oh));
        w = ow;
        h = oh;
    }
    let frame = (w, h);

    // Backwards: each step's required output, from the last to `start`.
    let n = steps.len() - start;
    let mut out_rects = vec![visible.clamped(frame.0, frame.1)?; n];
    for i in (0..n).rev() {
        let (iw, ih) = dims[i];
        let need = steps[start + i]
            .stage
            .input_rect_for(out_rects[i], iw, ih)?;
        if i > 0 {
            out_rects[i - 1] = need;
        }
    }
    Some(RegionPlan {
        out_rects,
        dims,
        frame,
    })
}

/// Execute a [`RegionPlan`]: run `steps[start..]` region-scoped, producing only the visible rect.
///
/// ⚠⚠ **The counterpart to [`region_plan`], and it belongs here rather than in the render engine.**
/// Chain execution is pure — only the caching and the gate need an engine — and putting it here is
/// what lets the whole mechanism be tested against the whole-buffer path **byte for byte with no
/// window**. Without that, `region_plan` could return plausible rects that no stage ever honours,
/// and every test would still pass: the W5 failure mode.
///
/// ⚠ Each chained source carries the **logical** full dimensions from the plan, never the size of
/// the slice in hand. That is the line M8.6's centre sabotage lives on.
///
/// Returns `None` if any step declines, which should not happen for a plan [`region_plan`]
/// produced — the two ask the same questions — but declining is always safe.
pub fn run_region_plan(
    steps: &[PrefixStep],
    start: usize,
    src: &ImageBuf,
    plan: &RegionPlan,
) -> Option<ImageBuf> {
    let n = plan.out_rects.len();
    if n == 0 || start + n != steps.len() {
        return None;
    }
    let mut carried: Option<ImageBuf> = None;
    for i in 0..n {
        let region_src = match &carried {
            None => RegionSrc::whole(src),
            Some(buf) => RegionSrc {
                buf,
                origin: (plan.out_rects[i - 1].x, plan.out_rects[i - 1].y),
                full: plan.dims[i],
            },
        };
        carried = Some(
            steps[start + i]
                .stage
                .apply_region(&region_src, plan.out_rects[i])?,
        );
    }
    carried
}

/// The dehaze airlight for `edits`, from a fixed 256 px replay of the prefix up to dehaze.
///
/// ⚠⚠ **It replays from the START of the prefix, never from a resume point.** Replaying from
/// wherever a render happens to resume would make the airlight depend on *which slider was
/// dragged* — two facts that can disagree, the shape `render_max_dim`, `fit_image_size` and
/// `Session::geometry_mode` were each deleted for. Starting from the source buffer makes it a
/// deterministic function of the edits alone: independent of the region, of the resume point, and
/// of the render scale.
///
/// ⚠⚠ **This is what lets dehaze be region-scoped at all.** Estimated from the stage's own input
/// under a scoped run, the airlight would come from whatever slice is on screen and the photograph
/// would change as you pan — the failure `PipelineStage::apply_region`'s `None` default exists to
/// prevent.
///
/// ⚠ 256 px because that is what the estimator uses anyway: `AIRLIGHT_SAMPLE`, chosen at M8.3o
/// precisely so the estimate cannot vary with the render scale. The replay is a 65k-pixel run.
///
/// ⚠ `None` is passed for the airlight of the steps it builds, so the replay cannot recurse:
/// dehaze is never constructed here, only the steps above it.
pub fn prefix_airlight(
    edits: &Edits,
    src: &ImageBuf,
    resampler: Resampler,
    lens: Option<lens_profile::LensShot>,
) -> [f32; 3] {
    const SAMPLE: usize = 256;
    let small = src.downscale_to(SAMPLE);
    let long = small.width().max(small.height()).max(1);
    let full_long = src.width().max(src.height()).max(1);
    let scale = RenderScale::new(long as f32 / full_long as f32);
    // ⚠ `None`, so the replay cannot recurse: dehaze is never constructed here.
    let steps = build_prefix_steps(edits, scale, lens, resampler, None);
    let upto = steps
        .iter()
        .position(|s| s.stage.name() == "dehaze")
        .unwrap_or(steps.len());
    let mut cur = small;
    for s in &steps[..upto] {
        cur = s.stage.apply(cur);
    }
    stages::dehaze::airlight(&cur)
}

/// One prefix stage's parameters, identifying the image that stage produces.
///
/// ⚠ **Adding a prefix stage means adding a variant here.** [`build_prefix_steps`] returns
/// `(key, stage)` pairs, so a prefix stage with no key does not compile. That is deliberate:
/// a prefix stage missing from a cache key means the preview cache serves stale pixels with
/// nothing failing, and this is the guard that makes forgetting impossible rather than merely
/// caught by a test.
///
/// ⚠⚠ The three **resampling** stages carry the [`Resampler`] as well as their edits, and it is
/// not decoration: the kernel is a `Settings` choice rather than an `Edits` field, so nothing
/// else in the key can see it change. Without it, switching the setting mid-session would serve
/// checkpoints rendered with the old kernel and the control would look inert — the failure
/// `apply_camera_profile` had to clear `BufferCache` for, solved here structurally instead.
///
/// ⚠ Deliberately **not** a field of [`PrefixKey`] beside `scale`: on a photo with no geometry,
/// no perspective and no lens profile nothing resamples, so changing the kernel must invalidate
/// nothing. Putting it on the steps that actually use it is what gives that for free.
///
/// ⚠ **No longer `Copy`**, because a spot list is a `Vec`. `PrefixKey::upto` clones instead —
/// a handful of five-field structs per render, against re-running lens correction.
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixStepKey {
    LensCorrection(edit_model::LensCorrection, Resampler),
    Perspective(edit_model::Perspective, Resampler),
    SpotRemoval(Vec<edit_model::Spot>),
    Geometry(edit_model::Geometry, Resampler),
    NoiseReduction(edit_model::NoiseReduction),
    Dehaze(edit_model::Dehaze),
    /// One mask's capture corrections.
    ///
    /// ⚠⚠ Holds the **projection**, never the whole `MaskedAdjustment` — see
    /// [`capture_projection`]. Keyed on the whole thing, a masked-exposure drag would re-run a
    /// whole-buffer denoise.
    ///
    /// ⚠ **Boxed**, and clippy is why: at 296 bytes against the next-largest variant's 25, an
    /// inline `MaskedAdjustment` would make **every** entry in a `PrefixKey`'s `Vec` that size —
    /// including the zero-sized ones. The same `large_enum_variant` answer `ActionKind::Sync`
    /// already gives.
    MaskedCapture(Box<edit_model::MaskedAdjustment>),
}

/// One prefix stage together with the key identifying its output, so the renderer can cache
/// each stage's result independently.
pub struct PrefixStep {
    pub key: PrefixStepKey,
    pub stage: Box<dyn PipelineStage>,
}

/// The prefix as an ordered list of `(key, stage)` pairs.
///
/// An inactive stage produces no step — exactly as it produces no stage in the pipeline today.
/// That is what makes the checkpoint after an inactive stage *be* the checkpoint before it:
/// the cumulative keys are equal, so nothing extra is stored.
pub fn build_prefix_steps(
    edits: &Edits,
    scale: RenderScale,
    lens: Option<lens_profile::LensShot>,
    resampler: Resampler,
    airlight: Option<[f32; 3]>,
) -> Vec<PrefixStep> {
    let mut steps: Vec<PrefixStep> = Vec::new();

    // ⚠ First, ahead of geometry: the calibration is defined in the uncropped, unrotated sensor
    // frame, so it must precede both the crop and the straighten. A missing corrector — an
    // unlisted lens, or a JPEG — skips the step silently, which is the ordinary case.
    let lens_correction = edits.applied_lens_correction();
    if lens_correction.is_active() {
        if let Some(c) = lens {
            steps.push(PrefixStep {
                key: PrefixStepKey::LensCorrection(lens_correction, resampler),
                stage: Box::new(LensCorrectionStage::new(
                    c,
                    lens_profile::Corrections {
                        distortion: lens_correction.distortion,
                        tca: lens_correction.tca,
                        vignetting: lens_correction.vignetting,
                        scale: lens_correction.scale,
                    },
                    resampler,
                )),
            });
        }
    }

    // ⚠ Between lens correction and geometry: after the optics, before the crop. See the
    // stage's own module docs for why that order is forced rather than chosen.
    if edits.perspective.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Perspective(edits.perspective, resampler),
            stage: Box::new(stages::perspective::PerspectiveStage::new(
                edits.perspective,
                resampler,
            )),
        });
    }

    // ⚠ Between perspective and geometry, i.e. on `GeometryStage`'s **input** — the space brush
    // strokes are anchored in. A spot is content-referred, so anchoring it here is what makes
    // "the patch survives a crop" free. See the stage's module docs.
    if !edits.spots.is_empty() {
        steps.push(PrefixStep {
            key: PrefixStepKey::SpotRemoval(edits.spots.clone()),
            stage: Box::new(SpotRemovalStage::new(&edits.spots)),
        });
    }

    // Geometry runs next, so all adjustments operate on the final cropped pixels.
    if edits.geometry != edit_model::Geometry::default() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Geometry(edits.geometry, resampler),
            stage: Box::new(GeometryStage::new(&edits.geometry, resampler)),
        });
    }

    // Noise reduction comes before every adjustment: denoising the source is far easier than
    // denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are modifiers and
    // must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::NoiseReduction(edits.noise_reduction),
            stage: Box::new(NoiseReductionStage::new(&edits.noise_reduction, scale)),
        });
    }

    // Dehaze is a correction of the capture, not a grade, so it belongs upstream with noise
    // reduction — and after it, because dehaze is a contrast stretch that amplifies noise.
    //
    // ⚠ Being in the prefix is what makes a *frame-global* airlight possible at all: the
    // prefix always runs whole-buffer, so no region plumbing and no margin growth is needed.
    if edits.dehaze.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Dehaze(edits.dehaze),
            // ⚠⚠ A supplied airlight is what makes this step region-scopable. Without one the
            // stage estimates from the image it is handed, which under a scoped run is a slice —
            // and the photograph would then change as you pan.
            stage: match airlight {
                Some(a) => Box::new(DehazeStage::with_airlight(&edits.dehaze, scale, a)),
                None => Box::new(DehazeStage::new(&edits.dehaze, scale)),
            },
        });
    }

    // ⚠ After the global pair, mirroring the tail's own rule that a mask acts on the global look
    // rather than being re-mapped by it.
    //
    // ⚠ One step per mask that actually carries a capture correction; a mask with only tail
    // adjustments costs nothing here.
    for m in &edits.masks {
        let p = capture_projection(m);
        if !p.adjustments.noise_reduction.is_active() && !p.adjustments.dehaze.is_active() {
            continue;
        }
        steps.push(PrefixStep {
            // ⚠⚠ The key holds **the projection the stage was built from**, so the key and the
            // stage cannot drift — the property this function exists to guarantee.
            key: PrefixStepKey::MaskedCapture(Box::new(p.clone())),
            // ⚠⚠ **`None` for the prebuilt coverage, and it must stay that way.** The render
            // engine's `BrushCache` is sized from the *prefix output's* dimensions, which do not
            // exist until the prefix has run — so a prebuilt map is unavailable here even in
            // principle. `MaskStage::apply` rasterizes its own when handed `None`, and with
            // `region: None` that is the whole frame at exactly the right size. **It costs
            // nothing per frame because this whole step is cached**: the rasterization happens
            // when the mask's shape or its capture sliders change, not on every render.
            stage: Box::new(MaskStage::new_capture(&p, scale, &edits.geometry, &[])),
        });
    }

    steps
}

/// Everything a cached prefix checkpoint depends on: the parameters of the prefix stages that
/// have run so far, plus the render scale.
///
/// The key is **derived from** [`build_prefix_steps`] rather than written alongside it, so a
/// prefix stage cannot be missing from it — see [`PrefixStepKey`]. The tests in this module
/// additionally assert that every *downstream* edit field leaves this key unchanged.
///
/// Compared with `==` rather than hashed, so there is no collision risk. The `f32` is safe in
/// a `PartialEq`: the only pathological value is `NaN`, which compares unequal to itself and
/// so fails toward a cache *miss*.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixKey {
    /// The keys of the prefix stages that have run, in order.
    steps: Vec<PrefixStepKey>,
    /// Included because it sets the noise-reduction and dehaze radii.
    scale: f32,
}

impl PrefixKey {
    /// The key for the output of the *whole* prefix.
    pub fn new(edits: &Edits, scale: RenderScale, resampler: Resampler) -> Self {
        // ⚠ `None` is correct and must not be "fixed": the key is derived from the *edits*, and
        // `PrefixStepKey::LensCorrection` carries them. A corrector is needed to build the stage,
        // never to key it. Consequence, stated rather than discovered: with no corrector the step
        // is skipped, so this key can be one entry longer than the real prefix — harmless,
        // because whether a corrector exists is constant for a photo and the engine is per photo.
        // ⚠ `None` for `brush` for the same reason as `lens`: a coverage map is needed to RUN
        // the stage, never to KEY it — the key is derived from the edits, and
        // ⚠ `None` for the airlight: it is derived from `edits`, which this key already covers,
        // so carrying it here would be a second copy of a fact the key already holds.
        let steps = build_prefix_steps(edits, scale, None, resampler, None);
        Self::upto(&steps, steps.len(), scale)
    }

    /// How many prefix steps this key covers — checkpoint 0 is the bare downscale, so a key of
    /// depth `n` is the output of the first `n` steps.
    ///
    /// ⚠⚠ **Derived from the key rather than carried beside it.** A cache that stored a depth
    /// alongside a key would be two facts that can disagree, which is the shape
    /// `render_max_dim` and `Session::geometry_mode` were each deleted for.
    pub fn depth(&self) -> usize {
        self.steps.len()
    }

    /// The key for the checkpoint after the first `n` prefix stages have run.
    ///
    /// `n == 0` is the checkpoint straight after the downscale, before any stage. `n` beyond
    /// the end saturates, so a caller cannot key a checkpoint that does not exist.
    pub fn upto(steps: &[PrefixStep], n: usize, scale: RenderScale) -> Self {
        Self {
            steps: steps[..n.min(steps.len())]
                .iter()
                .map(|s| s.key.clone())
                .collect(),
            scale: scale.get(),
        }
    }
}

/// The per-render parameters that are not part of [`Edits`].
///
/// ⚠ A struct because the builder reached eight arguments and clippy's `too_many_arguments`
/// objected — the third time this project has answered that with a params struct, after
/// `FrameSpec` and `NameField`. Both times the note was that clippy was right, not a lint to
/// silence. The body destructures it, so it reads as before.
///
/// ⚠⚠ `baseline` is **the illuminant the source is already balanced to**, and it is the one field
/// here that is a fact about the file rather than a choice: the as-shot temperature for a RAW,
/// D65 for a JPEG the camera has already balanced. Its default is D65, which is not a fallback —
/// it is the right answer for every non-RAW file, and for a synthetic buffer with no camera at
/// all, which is what keeps the goldens untouched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSpec {
    pub space: color::OutputSpace,
    pub resampler: Resampler,
    pub baseline: color::wb::Illuminant,
    /// The frame-global dehaze airlight, from [`prefix_airlight`].
    ///
    /// ⚠⚠ **`None` means "let dehaze estimate from the image it is handed"**, which is what
    /// shipped before M8.7 and keeps every existing caller — the CLI, the goldens' default path,
    /// every test — on exactly the behaviour they had. A region-scoped render REQUIRES `Some`:
    /// `DehazeStage::apply_region` declines without one, because estimating from a slice changes
    /// the photograph as you pan.
    ///
    /// ⚠ It must be **derived from `edits`** via [`prefix_airlight`], never invented. `PrefixKey`
    /// does not carry it, and is sound only because the key already covers every edit the replay
    /// reads; an airlight unrelated to the edits would be a value the cache cannot see.
    pub airlight: Option<[f32; 3]>,
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            space: color::OutputSpace::Srgb,
            resampler: Resampler::default(),
            baseline: color::wb::Illuminant::default(),
            airlight: None,
        }
    }
}

/// Build the stages for one set of adjustments.
///
/// ⚠ **One list, two consumers**: the global tail calls it with the values from `Edits`, and
/// every mask calls it with its own `LocalAdjustments`. A stage added here reaches both — the
/// same guard [`build_prefix_steps`] provides for the prefix, and the reason `LocalAdjustments`
/// repeating field *names* from `Edits` is harmless: the duplication that would actually hurt
/// is this logic, and it exists once.
///
/// ⚠⚠ **FIVE of these are passed NEUTRAL by the global caller** — `tone_curve`,
/// `local_contrast`, `hsl`, `color_grading` and `sharpening` — because the global chain places
/// each itself, at a position each has earned. Passing the real values would apply them twice
/// and move every golden. This is the same rule `saturation` has followed here since masking
/// landed; see the caller for all of them.
///
/// ⚠ `scale` reaches the spatial stages only. The per-pixel ones ignore it, which
/// `per_pixel_stages_ignore_the_render_scale` still asserts bit for bit — and that test now
/// doubles as the guard that the **global** path did not accidentally gain a spatial stage.
/// The gains [`WhiteBalanceStage`] is built from, for a white balance and a source baseline.
///
/// ⚠⚠ **One expression, two consumers** — [`adjustment_stages`] and `gpu-pipeline`'s builder. The
/// second was about to recompute it, which is how the CPU and GPU stages would come to disagree
/// about what "as shot" means: a wrong picture rather than an error, and invisible on any neutral
/// fixture because both sides land near `[1, 1, 1]`.
///
/// ⚠ `kelvin: None` is "as shot", which resolves to the **baseline's** temperature so the
/// arithmetic lands on `[1, 1, 1]` — tint alone still needs a temperature to sit at, and the
/// baseline is the only one that leaves the temperature axis untouched.
pub fn white_balance_gains(
    wb: &edit_model::WhiteBalance,
    baseline: color::wb::Illuminant,
) -> [f32; 3] {
    let declared = color::wb::Illuminant::new(wb.kelvin.unwrap_or(baseline.kelvin), wb.tint);
    color::wb::adaptation_gains(declared, baseline, &color::spaces::PROPHOTO_TO_XYZ)
}

fn adjustment_stages(
    a: &edit_model::LocalAdjustments,
    baseline: color::wb::Illuminant,
    scale: RenderScale,
) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.white_balance.is_active() {
        let wb = &a.white_balance;
        // ⚠ `None` means the source's own baseline, so declaring it changes nothing — and the
        // cheapest way to be *exactly* neutral is to insert no stage rather than to trust the
        // arithmetic to land on [1, 1, 1]. Tint alone still needs a temperature to sit at, and
        // the baseline is the only one that leaves the temperature axis untouched.
        out.push(Box::new(WhiteBalanceStage::new(white_balance_gains(
            wb, baseline,
        ))));
    }
    if a.exposure != 0.0 {
        out.push(Box::new(ExposureStage::new(a.exposure)));
    }
    if a.contrast != 0.0 {
        out.push(Box::new(ContrastStage::new(a.contrast)));
    }
    if a.highlights != 0.0 || a.shadows != 0.0 || a.whites != 0.0 || a.blacks != 0.0 {
        out.push(Box::new(ToneRegionsStage::new(
            a.highlights,
            a.shadows,
            a.whites,
            a.blacks,
        )));
    }
    // After the tone regions and before local contrast, mirroring the global chain: a local
    // contrast measured before the curve would then be re-mapped by it.
    //
    // ⚠ Empty points insert nothing — the global rule, and what keeps a mask with a default
    // curve from paying for a 1024-entry identity LUT on every render.
    if !a.tone_curve.points.is_empty() {
        out.push(Box::new(ToneCurveStage::new(&a.tone_curve.points)));
    }
    // Before saturation, mirroring the global chain: a local contrast measured after the tonal
    // work is the one that describes what you actually see.
    if a.local_contrast.is_active() {
        out.push(Box::new(LocalContrastStage::new(&a.local_contrast, scale)));
    }
    if a.saturation != 0.0 || a.vibrance != 0.0 {
        out.push(Box::new(SaturationStage::new(a.saturation, a.vibrance)));
    }
    // After saturation for the reason it is there globally: HSL corrects colours that are *in*
    // the picture, so it must see final tonality.
    if a.hsl != edit_model::Hsl::default() {
        out.push(Box::new(HslStage::new(&a.hsl)));
    }
    // After HSL, mirroring the global chain: HSL corrects colours that are *in* the picture
    // while a grade is laid on top, so reversed an HSL band tweak would act on the tint just
    // added.
    //
    // ⚠ `is_active` is wheels-only, so blending and balance alone insert nothing — they shape
    // ranges that tint nothing until a wheel is turned up.
    //
    // ⚠ A third consumer of `ColorGradingStage::new`, which builds a 256-entry table per
    // construction. Per render rather than per pixel, and measured rather than assumed — see
    // `cargo run --release -p app --example bench_masked`.
    if a.color_grading.is_active() {
        out.push(Box::new(ColorGradingStage::new(&a.color_grading)));
    }
    // ⚠ Last, for the reason it is last globally: sharpening must not amplify anything a later
    // stage would then stretch.
    if a.sharpening.is_active() {
        out.push(Box::new(SharpenStage::new(&a.sharpening, scale)));
    }
    out
}

/// The **prefix** stages for one set of adjustments: the corrections of the capture.
///
/// ⚠⚠ **Its own builder rather than a branch inside [`adjustment_stages`], because these run in
/// a different half of the pipeline.** Noise reduction and dehaze are placed where the *global*
/// stages of the same name run — after geometry, before every tonal adjustment — so a masked
/// Luminance 50 means what a global Luminance 50 means.
///
/// ⚠⚠ **Dehaze is why the placement is forced rather than preferred.** Its airlight is estimated
/// from the brightest 0.1% of the buffer it is handed. The tail may run on the visible *region*,
/// so a tail-side masked dehaze would re-estimate the airlight as the user panned and the
/// photograph would change under them. The prefix always runs whole-buffer.
///
/// ⚠ Noise reduction before dehaze, mirroring the global prefix for its stated reason: dehaze is
/// a contrast stretch that amplifies noise.
///
/// ⚠ This and [`adjustment_stages`] **partition** a mask's adjustments — a stage built by both
/// would run twice. `the_two_mask_builders_do_not_overlap` is the guard.
fn capture_stages(
    a: &edit_model::LocalAdjustments,
    scale: RenderScale,
) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.noise_reduction.is_active() {
        out.push(Box::new(NoiseReductionStage::new(
            &a.noise_reduction,
            scale,
        )));
    }
    if a.dehaze.is_active() {
        out.push(Box::new(DehazeStage::new(&a.dehaze, scale)));
    }
    out
}

/// A mask reduced to just its capture corrections — what the prefix stage is built from **and**
/// what its cache key holds, so the two cannot drift.
///
/// ⚠⚠ **The projection is what stops the prefix cache thrashing.** Keyed on the whole
/// [`edit_model::MaskedAdjustment`], every masked-*exposure* drag would invalidate the checkpoint
/// and re-run a whole-buffer noise reduction — 117 ms at native, for a slider that has nothing to
/// do with it. `the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest` pins
/// both directions.
fn capture_projection(m: &edit_model::MaskedAdjustment) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: m.mask.clone(),
        // ⚠⚠ **The refinements are part of the SHAPE, so they belong in the key.** Left out, a
        // subtracted or intersected component would change what the masked noise reduction and
        // dehaze cover while the cached prefix checkpoint stayed valid — a stale picture at
        // native zoom with nothing on screen to announce it.
        //
        // ⚠ The compiler forced this decision rather than leaving it to be remembered: this is a
        // struct literal with no `..`, so the new field was an `E0063` right here.
        refine: m.refine.clone(),
        adjustments: edit_model::LocalAdjustments {
            noise_reduction: m.adjustments.noise_reduction,
            dehaze: m.adjustments.dehaze,
            ..Default::default()
        },
    }
}

/// The develop pipeline in two halves, split where the preview cache checkpoints.
///
/// The **prefix** — geometry, then noise reduction — depends only on `edits.geometry` and
/// `edits.noise_reduction`, which is exactly what [`PrefixKey`] captures. Its output can
/// therefore be reused across every adjustment downstream, which is what makes editing with
/// noise reduction enabled affordable.
///
/// The **tail** is everything else and always ends in the output transform.
///
/// [`build_pipeline`] is defined as `prefix ++ tail`, so the two constructions cannot drift
/// apart. Add a stage here, not there.
pub fn build_pipeline_split_in_region(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        region,
        None,
        RenderSpec::default(),
        None,
        ProfileRendering::default(),
    )
}

/// What the **tail's rendering** needs that [`Edits`] cannot carry.
///
/// ⚠ A struct rather than more parameters: the builder already sits at seven, and an eighth trips
/// `too_many_arguments` — the same answer `FrameSpec`, `NameField`, `SourceInfo`, `SourceFacts`
/// and `RenderSpec` each gave. `Copy` despite holding references, because a shared reference is
/// `Copy`, which is what lets it sit beside `RenderSpec` rather than inside it.
///
/// ⚠ **The name is now narrower than the contents**, and that is a deliberate wave-scoping
/// choice rather than a judgement: `look` is not a camera-profile field, but renaming the type
/// would churn ~25 `ProfileRendering::default()` call sites in `render_engine.rs`, which another
/// track is editing concurrently. Worth renaming to `TailRendering` once the wave merges.
///
/// ⚠⚠ **Everything here is loaded by the CALLER**, because `pipeline` does no I/O. A caller that
/// supplies nothing gets a render without it — which is exactly what a missing `.cube` must do.
#[derive(Clone, Copy, Default)]
pub struct ProfileRendering<'a> {
    /// The profile's `ProfileToneCurve`, when it has a usable one; else the built-in curve.
    pub curve: Option<&'a [[f32; 2]]>,
    /// The profile's `BaselineExposure` in stops; `None` falls back to
    /// [`stages::display_rendering::BASELINE_EV`].
    ///
    /// ⚠ `None` is not `Some(0.0)`: a profile that declares no shift has made a statement, and
    /// one that says nothing has not.
    pub baseline_ev: Option<f32>,
    /// The creative look's parsed table, when the app has resolved `edits.look.file` to a file
    /// and read it.
    ///
    /// ⚠ `&Arc` rather than `Arc`, so this struct stays `Copy` and the builder clones a refcount
    /// rather than up to 786 KB of table on every render.
    ///
    /// ⚠⚠ **`None` means the look does not run AND does not suppress the camera rendering.** A
    /// preset naming a `.cube` that is not on this machine must render *without the look*, not
    /// without a rendering — see `a_look_with_no_table_leaves_the_camera_rendering_alone`.
    pub look: Option<&'a std::sync::Arc<lut::Cube>>,
}

/// Whether a profile's tone curve is the identity, and therefore no rendering at all.
///
/// ⚠ Checked because the X-T4's `ProfileToneCurve` is exactly `[[0,0],[1,1]]`. Applying it would
/// leave the photograph colorimetric while the setting claimed to be rendering it — the feature
/// silently doing nothing on real hardware.
fn is_identity(points: &[[f32; 2]]) -> bool {
    points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
}

/// The profile curve a [`DisplayRenderingStage`] should actually be built with.
///
/// ⚠⚠ **One rule, one place.** The tail applies [`is_identity`] before constructing the stage, and
/// anything that rebuilds that stage — `gpu-pipeline`'s `accelerate` does — must apply exactly the
/// same filter. Two callers each remembering to would be two chances to forget, and the symptom is
/// a GPU render that differs from the CPU one only on cameras whose profile declares an identity
/// curve, which is most of them.
pub fn effective_profile_curve(curve: Option<&[[f32; 2]]>) -> Option<&[[f32; 2]]> {
    curve.filter(|p| !is_identity(p))
}

/// The tail in two halves, split exactly where the grading stage goes.
///
/// ⚠ The split exists whether or not `ColorGradingStage` is active: Blending and Balance define
/// the ranges regardless, so "before the grade" is a structural position — after HSL, before the
/// masks loop — not a search for a stage name. The grade range view taps the buffer here.
pub struct TailHalves {
    /// Everything up to but excluding colour grading.
    pub before_grade: Pipeline,
    /// Colour grading (when active) and everything after it, ending with the output transform.
    pub from_grade: Pipeline,
}

/// As [`build_pipeline_split_in_region_with_brush`], with the tail returned in two halves.
///
/// ⚠ The untapped builder is a wrapper over this one that concatenates the halves, so every
/// existing caller — and every golden — is byte-identical by construction.
pub fn build_pipeline_split_tapped(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    // ⚠ An `&Arc`, not an `&BrushCoverage`: the masks loop hands each stage a clone so it can
    // **report** the coverage it had to build. `get` still works through the deref, so no caller
    // changes. See the coverage-cache spec's §4.0.
    brush: Option<&std::sync::Arc<BrushCoverage>>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    // What the *tail's* rendering needs that `Edits` cannot carry.
    profile: ProfileRendering<'_>,
) -> (Pipeline, TailHalves) {
    let ProfileRendering {
        curve: profile_curve,
        baseline_ev,
        look: look_table,
    } = profile;
    let RenderSpec {
        space,
        resampler,
        baseline,
        airlight,
    } = spec;
    // Defined through `build_prefix_steps` so the pipeline and the cache keys cannot drift:
    // one list, two consumers.
    let prefix: Vec<Box<dyn PipelineStage>> =
        build_prefix_steps(edits, scale, lens, resampler, airlight)
            .into_iter()
            .map(|s| s.stage)
            .collect();

    // ⚠ `saturation`/`vibrance` are passed as **zero** here, deliberately. In the global chain
    // saturation runs *after* the tone curve and local contrast, not with the basic
    // adjustments — so it stays where it is, further down this tail. Passing the real values
    // would silently move it earlier and change every golden. Inside a mask there is no tone
    // curve, so ordering within `per_pixel_stages` is the whole story and saturation belongs
    // in it.
    let mut tail: Vec<Box<dyn PipelineStage>> = adjustment_stages(
        &edit_model::LocalAdjustments {
            // ⚠⚠ **Neutral because these two do not run here at all** — unlike the five below,
            // which this tail places itself further down. `noise_reduction` and `dehaze` are
            // built by `capture_stages` and run in the **prefix**, where the global pair run.
            // Passing them here would apply a capture correction after the tone curve, on a
            // buffer that may be only the visible region — which for dehaze means an airlight
            // that changes as you pan.
            noise_reduction: edit_model::NoiseReduction::default(),
            dehaze: edit_model::Dehaze::default(),
            white_balance: edits.white_balance,
            exposure: edits.exposure,
            contrast: edits.contrast,
            highlights: edits.highlights,
            shadows: edits.shadows,
            whites: edits.whites,
            blacks: edits.blacks,
            saturation: 0.0,
            vibrance: 0.0,
            // ⚠⚠ Neutral for the identical reason `saturation` above is zero: **this tail places
            // each of these itself**, further down, at a position each has earned. Passing the
            // real values would apply every one of them **twice** and move every golden.
            //
            // ⚠ Inside a mask there is no such second placement, so a mask passes its real
            // values and the ordering within the shared builder is the whole story.
            //
            // ⚠ `ToneCurve::default()` is an empty point list, which inserts no stage at all —
            // the same "be exactly neutral by inserting nothing rather than trusting the
            // arithmetic to land on the identity" rule `white_balance` follows above.
            tone_curve: edit_model::ToneCurve::default(),
            local_contrast: edit_model::LocalContrast::default(),
            hsl: edit_model::Hsl::default(),
            color_grading: edit_model::ColorGrading::default(),
            sharpening: edit_model::Sharpening::default(),
        },
        baseline,
        scale,
    );
    if !edits.tone_curve.points.is_empty() {
        tail.push(Box::new(ToneCurveStage::new(&edits.tone_curve.points)));
    }

    // Local contrast, in two bands. After the tone curve is mandatory: a local contrast
    // measured before it would then be re-mapped by it. Before the colour stages so they
    // see final tonality — numerically near-irrelevant, since the gain is scalar and
    // preserves r:g:b ratios, but the order should say what we mean.
    if edits.local_contrast.is_active() {
        tail.push(Box::new(LocalContrastStage::new(
            &edits.local_contrast,
            scale,
        )));
    }

    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        tail.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }

    if edits.hsl != edit_model::Hsl::default() {
        tail.push(Box::new(HslStage::new(&edits.hsl)));
    }

    // A look laid over the corrected photograph. After HSL because HSL corrects colours that
    // are *in* the picture while a grade is applied on top — reversed, an HSL band tweak would
    // act on the tint just added. After the tone curve and local contrast is forced rather than
    // chosen: the ranges are selected on lightness, so anything remapping lightness later would
    // slide pixels between ranges. Before the masks, which act on the global look.
    //
    // Per-pixel and position-independent, so it needs no `Region`, no `RenderScale` and no
    // kernel margin — and being in the tail, no `PrefixStepKey`.
    //
    // ⚠ The tap point. Everything pushed so far is the first half.
    let before_grade = std::mem::take(&mut tail);
    if edits.color_grading.is_active() {
        tail.push(Box::new(ColorGradingStage::new(&edits.color_grading)));
    }

    // ⚠ After the global per-pixel block because a mask is the same kind of operation,
    // localised — it should act on the global look rather than be re-mapped by it. Before the
    // vignette and sharpening because both have earned their end-of-chain positions: the
    // vignette's "+100 = exactly white" endpoint depends on running after the tone curve, and
    // sharpening must not amplify anything a later stage would then stretch.
    // ⚠ The index is the position in `edits.masks`, **not** a counter over *active* masks. An
    // inactive mask is skipped here but still owns a slot in `BrushCoverage`, so a separate
    // counter would misalign every map after the first inactive one — the same sparse-versus-dense
    // index bug this codebase has already hit three times (the crop feedback loop,
    // `render_max_dim`, the Library grid).
    for (i, m) in edits.masks.iter().enumerate() {
        if m.adjustments.is_active() {
            tail.push(Box::new(MaskStage::new_in_region(
                m,
                scale,
                region,
                &edits.geometry,
                // ⚠ The mask's whole component row, index 0 being its base. `i` is still the
                // position in `edits.masks`, never a counter over *active* masks.
                brush.map_or(&[][..], |b| b.row(i)),
                baseline,
                // ⚠ `i` is the position in `edits.masks`, the same index the comment above
                // insists on — so a deposited map lands under the key `render_engine` computed
                // for *this* mask. A counter over active masks would file one mask's coverage
                // against another's key: a stale hit, which renders positionally while the tint
                // shows the gate working.
                brush.map(|b| (std::sync::Arc::clone(b), i)),
            )));
        }
    }

    // A creative effect over the finished grade, so it runs last. It is post-crop for free:
    // `GeometryStage` ran first, so the frame it sees is already cropped. It takes no
    // `RenderScale` — normalized coordinates make it resolution-independent.
    if edits.vignette.is_active() {
        tail.push(Box::new(VignetteStage::new_in_region(
            &edits.vignette,
            region,
        )));
    }

    // The last spatial stage, and the only one in the tail. It therefore sets the margin a
    // region render must keep outside the visible rect — see `max_kernel_radius`. Placed
    // after the vignette so a darkened corner sharpens proportionally less in absolute
    // terms, which is what a ratio-based model should do.
    if edits.sharpening.is_active() {
        tail.push(Box::new(SharpenStage::new(&edits.sharpening, scale)));
    }

    // ⚠⚠ **Last before the output transform.** Every capture stage upstream keeps its
    // scene-linear tuning — `local_contrast`'s `MID_GRAY`, `sharpening`'s `LUMA_EPS`,
    // `dehaze`'s airlight percentile — and this is what bounds the data to [0, 1], which is the
    // seam creative LUTs and `ProfileLookTable` were blocked on.
    //
    // ⚠ It sits *after* sharpening, and "sharpening is last so nothing stretches it" survives
    // that: `OutputTransform` was always after sharpening too, and an sRGB encode is a ~1/2.2
    // power — far more aggressive than any tone curve.
    // ⚠⚠ **The look's table, and it gates everything below.** `edits.look` says what the user
    // asked for; `look_table` says whether the `.cube` is actually here. A preset naming a file
    // this machine does not have must render *without the look* — never silently as an error, and
    // never as a photo that lost its rendering too.
    let look = look_table.filter(|_| edits.look.is_active());

    // ⚠⚠ **A LINEAR LUT REPLACES THE CAMERA RENDERING; A DISPLAY-REFERRED ONE FOLLOWS IT.** This
    // is the core of the design (spec §2), and it is a placement decision rather than a shaping
    // one. A film emulation expects log-or-linear input and produces the final look — stacked on
    // top of `DisplayRenderingStage` it double-renders, our S-curve then the LUT's, giving
    // crushed shadows and a clipped roll-off. That is the single most common way LUT support is
    // reported as "looks wrong". A grade, by contrast, expects an already-rendered image.
    //
    // ⚠ Conditioned on `look.is_some()`, not on the declared space alone: see above.
    let look_is_the_rendering = look.is_some() && edits.look.space.is_the_rendering();

    match edits.applied_rendering() {
        edit_model::Rendering::None => {}
        _ if look_is_the_rendering => {
            // The linear LUT *is* the rendering. Suppressed deliberately — the Rendering panel
            // section must say so, because two renderings apparently both switched on with one
            // silently ignored is the invisible-state failure this project keeps recording.
        }
        edit_model::Rendering::Camera => {
            // ⚠⚠ An **identity** profile curve is ignored. The X-T4's `ProfileToneCurve` is
            // literally [[0,0],[1,1]], so honouring it would produce no rendering at all —
            // indistinguishable from `None`, and the setting would silently do nothing on the
            // author's own camera.
            let curve = effective_profile_curve(profile_curve);
            tail.push(Box::new(DisplayRenderingStage::new(curve, baseline_ev)));
        }
    }

    // ⚠ Either way, **before** the output transform: a look is a display-referred decision on
    // working-space data, not something applied to encoded output.
    if let Some(cube) = look {
        tail.push(Box::new(CreativeLookStage::new(
            // ⚠ Cloning the `Arc`, not the table — a refcount rather than up to 786 KB, on every
            // rebuild of the pipeline.
            std::sync::Arc::clone(cube),
            edits.look.space,
            edits.look.strength,
        )));
    }

    tail.push(Box::new(OutputTransform::new(space)));

    (
        Pipeline::new(prefix),
        TailHalves {
            before_grade: Pipeline::new(before_grade),
            from_grade: Pipeline::new(tail),
        },
    )
}

/// As [`build_pipeline_split_in_region`], with brush coverage the caller has already rasterized.
///
/// ⚠ The extra parameter is additive with a delegating wrapper — the `Region` precedent, which
/// gave that work zero call-site churn. The CLI, export and every golden go through the wrapper.
///
/// ⚠ Itself a wrapper over [`build_pipeline_split_tapped`], concatenating the two halves — the
/// same stages in the same order, so nothing here can move a pixel.
pub fn build_pipeline_split_in_region_with_brush(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    // ⚠ An `&Arc`, not an `&BrushCoverage`: the masks loop hands each stage a clone so it can
    // **report** the coverage it had to build. `get` still works through the deref, so no caller
    // changes. See the coverage-cache spec's §4.0.
    brush: Option<&std::sync::Arc<BrushCoverage>>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    profile: ProfileRendering<'_>,
) -> (Pipeline, Pipeline) {
    let (prefix, halves) =
        build_pipeline_split_tapped(edits, scale, region, brush, spec, lens, profile);
    let mut tail = halves.before_grade.into_stages();
    tail.extend(halves.from_grade.into_stages());
    (prefix, Pipeline::new(tail))
}

/// The develop pipeline in two halves, for a render covering the whole frame.
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region(edits, scale, None)
}

/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
///
/// ⚠ Resamples with [`Resampler::default`]. Every **production** render goes through
/// [`build_pipeline_with`] instead, so the user's choice reaches it; this wrapper exists so the
/// goldens, the examples and the debug tools compile untouched — the `Region` precedent.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline {
    build_pipeline_with(edits, scale, Resampler::default())
}

/// As [`build_pipeline`], resampling with the caller's chosen kernel.
pub fn build_pipeline_with(edits: &Edits, scale: RenderScale, resampler: Resampler) -> Pipeline {
    build_pipeline_to(edits, scale, color::OutputSpace::Srgb, resampler)
}

/// As [`build_pipeline`], writing to `space` rather than sRGB.
///
/// ⚠ Additive with a delegating wrapper — the `Region` precedent, which gave that work zero
/// call-site churn. Only export and `rustroom render` call this; the preview goes through
/// [`build_pipeline_with`] and stays sRGB, because the display is assumed sRGB until a display
/// profile exists.
pub fn build_pipeline_to(
    edits: &Edits,
    scale: RenderScale,
    space: color::OutputSpace,
    resampler: Resampler,
) -> Pipeline {
    build_pipeline_spec(
        edits,
        scale,
        RenderSpec {
            space,
            resampler,
            ..RenderSpec::default()
        },
    )
}

/// As [`build_pipeline_to`], with the whole [`RenderSpec`] — which is what an export of a RAW
/// needs, since only it knows the file's white-balance baseline.
pub fn build_pipeline_spec(edits: &Edits, scale: RenderScale, spec: RenderSpec) -> Pipeline {
    let (prefix, tail) = build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        None,
        None,
        spec,
        None,
        ProfileRendering::default(),
    );
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    Pipeline::new(stages)
}

/// The widest neighbourhood any **tail** stage reads, in rendered pixels, for these edits.
///
/// The tail is the half that runs on a cropped region during a 1:1 render, so at the
/// region's edge a spatial tail stage reads neighbours that do not exist. This is the width
/// of that wrong band, and therefore the margin such a render must keep outside what it
/// displays. Prefix stages are exempt: the prefix always runs whole-buffer, which is why
/// noise reduction can never produce a seam however you pan.
///
/// An inactive stage contributes 0.
///
/// ⚠ **The reaches add, they do not `max`.** Sharpening runs *after* local contrast, so a
/// wrong value at the region's edge propagates 120 px inward through clarity and then a
/// further 15 px through sharpening — 135, not 120. This was a `max` until masking exposed
/// it: `a_region_render_matches_the_whole_render_cropped` had been passing with an inset
/// ~15 px too small, because the residual differences happened to round the same way. Adding
/// a masked +0.6 EV — up to a 1.5× gain — pushed them across a quantisation boundary and the
/// test failed. The slack was luck, not margin.
///
/// A per-pixel stage between them contributes nothing: it cannot move information sideways.
/// That is why masks, the vignette and every colour stage are absent here.
pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize {
    let sharp = if edits.sharpening.is_active() {
        stages::sharpening::max_kernel_radius(edits.sharpening.radius, scale)
    } else {
        0
    };
    let local = stages::local_contrast::kernel_reach(&edits.local_contrast, scale);

    // ⚠⚠ **Summed over the masks, not maximised.** Mask N runs on mask N-1's output, so a pixel
    // at the region edge reads through every mask in turn — exactly the reason the global
    // `sharp` and `local` above are added rather than maxed. Taking a max here is invisible with
    // one mask and wrong with two; see `two_masked_clarities_reach_further_than_one`.
    //
    // ⚠ This is why no fixed region margin can be correct: the stack is unbounded, so the reach
    // is a function of the edits and the caller must size its request from it.
    let masked: usize = edits
        .masks
        .iter()
        .map(|m| {
            let a = &m.adjustments;
            let s = if a.sharpening.is_active() {
                stages::sharpening::max_kernel_radius(a.sharpening.radius, scale)
            } else {
                0
            };
            s + stages::local_contrast::kernel_reach(&a.local_contrast, scale)
        })
        .sum();

    sharp + local + masked
}

// ⚠⚠ **There is deliberately no `max_tail_kernel_radius` any more.** It maximised
// `tail_kernel_radius` over every representable setting, and a fixed region margin was checked
// against it. Masks now carry spatial adjustments and **chain**, so N masked clarities reach
// N x 120 px and no constant bounds the result. The caller derives its margin from
// `tail_kernel_radius` per render instead — see `Session::render_margin` in the app crate.

#[cfg(test)]
mod tests {

    use super::{PipelineStage, Rect, RegionSrc, RenderScale};

    #[test]
    fn a_stage_with_no_region_support_declines_to_name_an_input_rect() {
        // ⚠ The default is what makes lens correction, spot removal and the masked captures
        // decline without the caller name-checking any of them.
        let s = crate::stages::exposure::ExposureStage::new(1.0);
        assert_eq!(
            s.input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 4
                },
                64,
                64
            ),
            None
        );
    }

    #[test]
    fn noise_reduction_grows_the_rect_by_its_reach_and_clips_to_the_image() {
        let p = edit_model::NoiseReduction {
            luminance: 0.0,
            color: 50.0,
            detail: 50.0,
            smoothness: 100.0,
        };
        let s = crate::stages::noise_reduction::NoiseReductionStage::new(&p, RenderScale::FULL);
        // Reach 64 (CHROMA_PASSES 2 x radius 32), interior rect: grows on every side.
        let got = s
            .input_rect_for(
                Rect {
                    x: 100,
                    y: 100,
                    w: 10,
                    h: 10,
                },
                1000,
                1000,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 36,
                y: 36,
                w: 138,
                h: 138
            }
        );
        // ⚠ At the image's own corner it must CLIP rather than underflow or overrun. A margin
        // larger than the image is ordinary at low render scales.
        let got = s
            .input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                },
                20,
                20,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 20
            },
            "clipped to the whole image"
        );
    }

    #[test]
    fn dehaze_grows_the_rect_by_its_reach() {
        let s = crate::stages::dehaze::DehazeStage::new(
            &edit_model::Dehaze { amount: 60.0 },
            RenderScale::FULL,
        );
        let got = s
            .input_rect_for(
                Rect {
                    x: 500,
                    y: 500,
                    w: 10,
                    h: 10,
                },
                2000,
                2000,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 360,
                y: 360,
                w: 290,
                h: 290
            },
            "140 on every side"
        );
    }

    #[test]
    fn the_back_projections_are_reachable_through_the_trait_object() {
        // ⚠⚠ **A test on the concrete type cannot tell a trait method from an inherent one.**
        // `input_rect_for` was written into `impl PerspectiveStage` once by mistake; both its own
        // tests passed, because `s.input_rect_for(..)` resolved to the inherent method — while the
        // TRAIT method still returned the default `None`. `region_plan` calls through
        // `dyn PipelineStage`, so it would have seen the feature as simply absent. Only clippy's
        // dead-code pass caught it, which is luck rather than a guard.
        //
        // ⚠ This asks the way `region_plan` asks. A stage that answers here answers there.
        let g = edit_model::Geometry {
            angle: 5.0,
            ..Default::default()
        };
        let geo: Box<dyn PipelineStage> = Box::new(crate::stages::geometry::GeometryStage::new(
            &g,
            crate::resample::Resampler::CatmullRom,
        ));
        assert!(
            geo.input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some(),
            "geometry must answer through the trait, not only on the concrete type"
        );

        let p = edit_model::Perspective {
            vertical: 40.0,
            ..Default::default()
        };
        let per: Box<dyn PipelineStage> =
            Box::new(crate::stages::perspective::PerspectiveStage::new(
                p,
                crate::resample::Resampler::CatmullRom,
            ));
        assert!(
            per.input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some(),
            "perspective must answer through the trait, not only on the concrete type"
        );

        let nr = edit_model::NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            detail: 50.0,
            smoothness: 50.0,
        };
        let n: Box<dyn PipelineStage> = Box::new(
            crate::stages::noise_reduction::NoiseReductionStage::new(&nr, RenderScale::FULL),
        );
        assert!(n
            .input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some());

        let d: Box<dyn PipelineStage> = Box::new(crate::stages::dehaze::DehazeStage::new(
            &edit_model::Dehaze { amount: 60.0 },
            RenderScale::FULL,
        ));
        assert!(d
            .input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some());
    }

    #[test]
    fn a_region_src_reads_in_full_image_coordinates() {
        // ⚠⚠ Full-image coordinates, not the buffer's. A reader taking buffer coordinates would
        // shift every read by the origin — M8.6's centre sabotage, which the goldens cannot see.
        let mut buf = core_image::ImageBuf::new(4, 4);
        buf.set_pixel(1, 1, [0.5, 0.25, 0.125]);
        let src = RegionSrc {
            buf: &buf,
            origin: (10, 20),
            full: (100, 100),
        };
        assert_eq!(src.at(11, 21), Some([0.5, 0.25, 0.125]));
        assert_eq!(src.at(9, 21), None, "left of the region");
        assert_eq!(src.at(11, 19), None, "above the region");
        assert_eq!(src.at(14, 21), None, "right of the region");
        assert_eq!(src.at(11, 24), None, "below the region");
    }

    #[test]
    fn the_whole_buffer_case_is_the_identity() {
        // ⚠ This is what makes M8.6's two implementations generalise rather than be rewritten.
        let buf = core_image::ImageBuf::new(8, 6);
        let src = RegionSrc::whole(&buf);
        assert_eq!(src.origin, (0, 0));
        assert_eq!(src.full, (8, 6));
        assert_eq!(src.at(7, 5), Some(buf.pixel(7, 5)));
    }

    fn steps_for(e: &Edits) -> Vec<super::PrefixStep> {
        super::build_prefix_steps(
            e,
            RenderScale::FULL,
            None,
            crate::resample::Resampler::CatmullRom,
            None,
        )
    }

    #[test]
    fn the_plan_walks_backwards_growing_the_rect_at_each_local() {
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 50.0,
                detail: 50.0,
                smoothness: 100.0,
            },
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let steps = steps_for(&e);
        assert_eq!(
            steps.len(),
            2,
            "fixture check: NR then dehaze, and nothing else"
        );
        let visible = Rect {
            x: 1000,
            y: 1000,
            w: 200,
            h: 200,
        };
        let plan = super::region_plan(&steps, 0, visible, 4000, 3000)
            .expect("every step here supports a region");

        // The LAST step produces exactly the visible rect.
        assert_eq!(*plan.out_rects.last().unwrap(), visible);
        // Dehaze reads 140 beyond it, so NR must produce that.
        assert_eq!(plan.out_rects[0], visible.grown(140, 4000, 3000));
    }

    #[test]
    fn a_declining_step_abandons_the_whole_plan() {
        // ⚠⚠ THE structural rule: a whole-buffer step needs a whole-buffer input, so one decline
        // anywhere at or after `start` kills everything above it too.
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            spots: vec![edit_model::Spot {
                centre: [0.5, 0.5],
                source: [0.3, 0.3],
                radius: 0.02,
                feather: 50.0,
                mode: Default::default(),
            }],
            ..Default::default()
        };
        let steps = steps_for(&e);
        let spot_at = steps
            .iter()
            .position(|s| s.stage.name() == "spot_removal")
            .expect("fixture check: the spot step must be in the prefix");
        let visible = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert!(
            super::region_plan(&steps, spot_at, visible, 400, 300).is_none(),
            "spot removal declines, so nothing from there on can be scoped"
        );
        assert!(
            super::region_plan(&steps, spot_at + 1, visible, 400, 300).is_some(),
            "starting below it is fine — that is the contained case"
        );
    }

    #[test]
    fn a_masked_capture_blocks_everything_because_it_is_last() {
        // ⚠⚠ The caveat as a test. Masked captures sit at the END of the prefix, so one declining
        // step there switches the feature off for the whole photo — there is nothing after it to
        // scope. A test covering only spots would miss this, and spots are the *contained* case.
        let m = edit_model::MaskedAdjustment {
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 40.0 },
                ..Default::default()
            },
            ..Default::default()
        };
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            masks: vec![m],
            ..Default::default()
        };
        let steps = steps_for(&e);
        assert!(
            steps.iter().any(|s| s.stage.name() == "mask"),
            "fixture check: the masked capture step must actually be in the prefix"
        );
        let visible = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert!(
            super::region_plan(&steps, 0, visible, 400, 300).is_none(),
            "a masked capture is last, so nothing above it can be scoped either"
        );
    }

    #[test]
    fn the_plan_reports_the_frame_dimensions_the_whole_path_would_produce() {
        // ⚠⚠ M8.6's sharpest hazard, generalised. These feed CoverageSpec { fw, fh } and the
        // tail's Region { full_w, full_h }; taking them off a region-scoped buffer rasterises
        // every brush mask at the region's size and thrashes the coverage cache on every pan.
        let mut e = Edits::default();
        // ⚠ A field OF a field, which clippy's field_reassign_with_default allows.
        e.geometry.crop = edit_model::CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 0.5,
        };
        let steps = steps_for(&e);
        let plan = super::region_plan(
            &steps,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            400,
            300,
        )
        .unwrap();
        assert_eq!(
            plan.frame,
            (200, 150),
            "the CROPPED frame, not the region and not the input"
        );
    }

    #[test]
    fn an_empty_step_list_has_no_plan() {
        // Nothing to scope is not a plan of length zero — it is no plan, so the caller takes the
        // whole-buffer path rather than believing it scoped something.
        assert!(super::region_plan(
            &[],
            0,
            Rect {
                x: 0,
                y: 0,
                w: 4,
                h: 4
            },
            64,
            64
        )
        .is_none());
    }

    /// A chart with structure everywhere, so a wrongly-centred rotation cannot coincide.
    fn structured(w: usize, h: usize) -> core_image::ImageBuf {
        let mut b = core_image::ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                b.set_pixel(x, y, [fx, fy, (((x / 7) + (y / 5)) % 2) as f32 * 0.8 + 0.1]);
            }
        }
        b
    }

    /// Run a chain both ways and report `(differing samples, worst absolute difference)`.
    fn chain_vs_whole(e: &Edits, src: &core_image::ImageBuf, visible: Rect) -> (usize, f32) {
        let steps = steps_for(e);
        assert!(
            steps.len() >= 2,
            "fixture check: need a real chain, got {}",
            steps.len()
        );
        let plan = super::region_plan(&steps, 0, visible, src.width(), src.height())
            .expect("fixture check: every step must support a region");
        assert!(
            plan.out_rects[0].x > 0 || plan.out_rects[0].y > 0,
            "fixture check: the chain must carry a NON-ZERO origin, else it cannot see the centre \
             — got {:?}",
            plan.out_rects[0]
        );
        let scoped = super::run_region_plan(&steps, 0, src, &plan).expect("the chain runs");
        let mut whole = src.clone();
        for st in &steps {
            whole = st.stage.apply(whole);
        }
        let want = whole.crop(visible.x, visible.y, visible.w, visible.h);
        assert_eq!(
            (scoped.width(), scoped.height()),
            (want.width(), want.height())
        );
        let (mut n, mut worst) = (0usize, 0.0f32);
        for y in 0..want.height() {
            for x in 0..want.width() {
                let (a, b) = (scoped.pixel(x, y), want.pixel(x, y));
                for c in 0..3 {
                    worst = worst.max((a[c] - b[c]).abs());
                }
                if a != b {
                    n += 1;
                }
            }
        }
        (n, worst)
    }

    #[test]
    fn a_chain_of_gathers_is_byte_for_byte() {
        // ⚠⚠ **THE test the centre sabotage needs, and it needs no render engine.** Chain
        // execution is pure; only the caching and the gate need one. Without this, `region_plan`
        // could return plausible rects that no stage honours and every other test would pass —
        // the W5 failure mode, which is why this was pulled forward out of plan 2.
        //
        // ⚠ Gathers only, so bit-identity is achievable: an output pixel reads its source and
        // writes only itself, with no accumulation whose order could differ. That is M8.6's own
        // standard, and `a_chain_with_a_local_agrees_to_within_a_rounding` records why a chain
        // carrying noise reduction cannot meet it.
        let mut e = Edits {
            perspective: edit_model::Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            ..Default::default()
        };
        e.geometry.angle = 6.0;
        e.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let src = structured(220, 180);
        let (differing, worst) = chain_vs_whole(
            &e,
            &src,
            Rect {
                x: 40,
                y: 30,
                w: 50,
                h: 40,
            },
        );
        assert_eq!(
            differing, 0,
            "a chain of gathers must be byte for byte; worst {worst:e}"
        );
    }

    #[test]
    fn a_chain_with_a_local_agrees_to_within_a_rounding() {
        // ⚠⚠ **Byte-for-byte is NOT achievable here, and that is a property of the operator rather
        // than a defect.** `box_blur` keeps a running sum, so it reaches each position by a
        // different history of adds and subtracts depending on where the row starts — the third
        // CPU divergence class this project records, which shows up between two CPU runs too.
        //
        // ⚠ Measured at **3.6e-7 absolute, about ONE ULP and 0.00009 of an 8-bit level**. The
        // bound below is ~28x that, which is loose enough to survive a re-tune of the radii and far
        // tighter than anything visible. ⚠ A margin bug reads as a *band at one edge* and would
        // blow this by orders of magnitude, so the bound still catches the failure it matters for.
        let mut e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 40.0,
                detail: 50.0,
                smoothness: 0.0,
            },
            ..Default::default()
        };
        e.geometry.angle = 6.0;
        e.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let src = structured(220, 180);
        let (_, worst) = chain_vs_whole(
            &e,
            &src,
            Rect {
                x: 40,
                y: 30,
                w: 50,
                h: 40,
            },
        );
        assert!(
            worst < 1e-5,
            "a running sum may wobble in the last bits; a margin bug may not. worst {worst:e}"
        );
    }

    #[test]
    fn a_supplied_airlight_is_used_verbatim_rather_than_re_estimated() {
        // ⚠⚠ The only test that can see the stage falling back to its own estimate. Two renders of
        // the SAME image with different supplied airlights must differ; if the stage re-estimates,
        // they are identical and this fails.
        let img = structured(64, 48);
        let p = edit_model::Dehaze { amount: 60.0 };
        let a = crate::stages::dehaze::DehazeStage::with_airlight(&p, RenderScale::FULL, [0.9; 3])
            .apply(img.clone());
        let b = crate::stages::dehaze::DehazeStage::with_airlight(&p, RenderScale::FULL, [0.5; 3])
            .apply(img.clone());
        assert_ne!(
            a.data(),
            b.data(),
            "the supplied airlight must reach the pixels"
        );
        // And the fallback still works when none is supplied.
        let c = crate::stages::dehaze::DehazeStage::new(&p, RenderScale::FULL).apply(img);
        assert_ne!(
            c.data(),
            a.data(),
            "None must estimate rather than reuse a supplied value"
        );
    }

    #[test]
    fn the_replay_does_not_depend_on_where_a_render_resumes() {
        // ⚠⚠ THE property that made this design replay from the START of the prefix. An airlight
        // that depended on the resume point would change with WHICH SLIDER YOU DRAGGED — two facts
        // that can disagree, the shape render_max_dim and Session::geometry_mode were deleted for.
        let img = structured(220, 180);
        let mut e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                detail: 50.0,
                smoothness: 50.0,
            },
            ..Default::default()
        };
        e.geometry.angle = 4.0;
        let one = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        let two = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        assert_eq!(one, two, "deterministic");

        // ⚠ And it is a function of the EDITS, so a downstream-only edit cannot move it. That is
        // what makes `PrefixKey` sound without carrying the airlight: the key already covers
        // every edit the replay reads.
        let mut f = e.clone();
        f.exposure = 1.5;
        f.vignette = edit_model::Vignette {
            amount: -60.0,
            ..Default::default()
        };
        assert_eq!(
            super::prefix_airlight(&f, &img, crate::resample::Resampler::CatmullRom, None),
            one,
            "a tail edit must not move the airlight"
        );
    }

    #[test]
    fn the_replay_moves_when_an_upstream_edit_moves() {
        // The discriminating other half: a constant would pass the test above.
        let img = structured(220, 180);
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let base = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        let mut f = e.clone();
        f.geometry.crop = edit_model::CropRect {
            left: 0.5,
            top: 0.5,
            right: 1.0,
            bottom: 1.0,
        };
        assert_ne!(
            super::prefix_airlight(&f, &img, crate::resample::Resampler::CatmullRom, None),
            base,
            "cropping to a different quarter must change what the brightest pixels are"
        );
    }

    use super::BrushCoverage;

    fn a_map() -> std::sync::Arc<crate::brush::CoverageMap> {
        std::sync::Arc::new(crate::brush::CoverageMap {
            outside: 42,
            ..Default::default()
        })
    }

    /// ⚠⚠ The write side exists because `MaskStage` is the only thing holding the image, so it is
    /// the only thing that can build a gating brush's coverage — and `render_engine`, which owns
    /// the key, has to be able to see what it built. See the coverage-cache spec's §4.0.
    #[test]
    fn a_deposited_map_can_be_taken_back_out() {
        let c = BrushCoverage::with_slots(&[1, 3]);
        assert!(c.take_built(1, 2).is_none(), "nothing deposited yet");
        c.deposit(1, 2, a_map());
        assert!(c.take_built(1, 2).is_some(), "the deposit must be readable");
        assert!(
            c.take_built(1, 2).is_none(),
            "taking it must clear the slot, so a stale map cannot be read twice"
        );
    }

    /// ⚠⚠ A composite's components must not see each other's coverage. Filing a refinement's map
    /// under its base's index is a **stale hit** — the shape renders positionally while the tint
    /// shows the gate working — and that is the one direction this cache must never fail in.
    #[test]
    fn a_deposit_lands_under_its_own_component() {
        let c = BrushCoverage::with_slots(&[1, 3]);
        c.deposit(1, 2, a_map());
        assert!(
            c.take_built(1, 1).is_none(),
            "a sibling component must not see it"
        );
        assert!(c.take_built(0, 0).is_none(), "another mask must not see it");
        assert!(
            c.take_built(1, 2).is_some(),
            "and it is still where it was put"
        );
    }

    /// ⚠ An index past the end is ignored rather than panicking: the mask list can change between
    /// this set being made and a stage running, exactly as `suppressed_spot`'s index can.
    #[test]
    fn depositing_out_of_range_is_ignored() {
        let c = BrushCoverage::with_slots(&[1]);
        c.deposit(7, 0, a_map());
        c.deposit(0, 9, a_map());
        assert!(c.take_built(0, 0).is_none());
    }

    /// ⚠ The read side reads by `(mask, component)` now, and `row` is what the masks loop hands
    /// a stage — one lookup rather than a per-component fetch it would have to index correctly.
    #[test]
    fn the_prebuilt_side_reads_by_mask_and_component() {
        let c = BrushCoverage {
            maps: vec![vec![None], vec![None, Some(a_map())]],
            built: Vec::new(),
        };
        assert!(c.get(0, 0).is_none());
        assert!(c.get(1, 1).is_some());
        assert!(
            c.get(1, 0).is_none(),
            "the base of a composite is its own slot"
        );
        assert!(c.get(9, 0).is_none(), "past the end is None, never a panic");
        assert_eq!(c.row(1).len(), 2);
        assert!(
            c.row(9).is_empty(),
            "a mask with no row hands back an empty slice"
        );
    }

    /// ⚠⚠ `Rendering::None` must insert **no stage at all**, not a curve that happens to be the
    /// identity. Bit-exactness is the rule the entire golden harness rests on.
    #[test]
    fn the_default_rendering_inserts_no_stage() {
        let names = build_pipeline(&Edits::default(), RenderScale::new(1.0)).stage_names();
        assert!(
            !names.contains(&"display_rendering"),
            "a neutral photo got a rendering stage: {names:?}"
        );
    }

    /// ⚠ And it lands immediately before the output transform — after every capture stage, so
    /// their scene-linear tuning survives, and before the transform so the histogram (taken from
    /// `pack`'s quantization of this output) describes the rendered image.
    #[test]
    fn the_rendering_runs_last_before_the_output_transform() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let names = build_pipeline(&edits, RenderScale::new(1.0)).stage_names();
        let r = names
            .iter()
            .position(|n| *n == "display_rendering")
            .expect("no rendering stage");
        let o = names
            .iter()
            .position(|n| *n == "output_transform")
            .expect("no output transform");
        assert_eq!(
            o,
            r + 1,
            "something got between the rendering and the transform: {names:?}"
        );
    }

    /// The tail's stage names for `edits`, with `cube` supplied as the look's table.
    ///
    /// ⚠ The **only** order helper these look tests use. Six tests already name stage order and a
    /// second helper is how two come to disagree, so this one delegates to the same builder every
    /// other order assertion goes through rather than reconstructing the list.
    #[cfg(test)]
    fn look_stage_names(
        edits: &Edits,
        cube: Option<&std::sync::Arc<lut::Cube>>,
    ) -> Vec<&'static str> {
        let (_, tail) = build_pipeline_split_in_region_with_brush(
            edits,
            RenderScale::FULL,
            None,
            None,
            RenderSpec::default(),
            None,
            ProfileRendering {
                look: cube,
                ..ProfileRendering::default()
            },
        );
        tail.stage_names()
    }

    fn a_cube() -> std::sync::Arc<lut::Cube> {
        std::sync::Arc::new(
            lut::Cube::parse(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
            )
            .expect("parses"),
        )
    }

    /// ⚠⚠ **Assert on the STAGE LIST, not on pixels.** The two placements are indistinguishable
    /// from output whenever a LUT happens to be gentle, so a pixel test would pass against the
    /// wrong one — and double-rendering a film emulation (our S-curve, then the LUT's, giving
    /// crushed shadows and a clipped roll-off) is precisely the bug this prevents.
    #[test]
    fn a_linear_lut_replaces_the_camera_rendering_and_a_display_lut_follows_it() {
        let cube = a_cube();
        let with = |space| {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            look_stage_names(&edits, Some(&cube))
        };

        let display = with(edit_model::LutSpace::SrgbDisplay);
        assert!(
            display
                .windows(2)
                .any(|w| w[0] == "display_rendering" && w[1] == "creative_look"),
            "a display-referred look must FOLLOW the rendering: {display:?}"
        );
        // Rec.709 is display-referred too, and must place identically.
        assert_eq!(with(edit_model::LutSpace::Rec709), display);

        let linear = with(edit_model::LutSpace::Linear);
        assert!(
            !linear.contains(&"display_rendering"),
            "a linear look IS the rendering and must replace it: {linear:?}"
        );
        assert!(linear.contains(&"creative_look"), "{linear:?}");
    }

    /// ⚠ Either way the look runs **before** the output transform: it is a display-referred
    /// decision on working-space data, not something applied to encoded output.
    #[test]
    fn a_look_runs_before_the_output_transform() {
        let cube = a_cube();
        for space in edit_model::LutSpace::ALL {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            let names = look_stage_names(&edits, Some(&cube));
            let l = names.iter().position(|n| *n == "creative_look").unwrap();
            let o = names.iter().position(|n| *n == "output_transform").unwrap();
            assert_eq!(o, l + 1, "{space:?}: {names:?}");
        }
    }

    /// ⚠⚠ **A look whose file is missing renders WITHOUT the look — and the camera rendering
    /// stays.** Suppressing the rendering on the strength of a `Linear` declaration alone would
    /// leave a photo with a missing `.cube` rendering colorimetrically flat, which is worse than
    /// the missing look. The suppression is therefore conditioned on the table actually being
    /// there, not on what `Edits` claims.
    #[test]
    fn a_look_with_no_table_leaves_the_camera_rendering_alone() {
        for space in edit_model::LutSpace::ALL {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("gone.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            let names = look_stage_names(&edits, None);
            assert!(!names.contains(&"creative_look"), "{space:?}: {names:?}");
            assert!(
                names.contains(&"display_rendering"),
                "{space:?} lost the camera rendering to a look that is not there: {names:?}"
            );
        }
    }

    /// ⚠ An inactive look inserts no stage even when a table is supplied — a whole-image pass
    /// that does nothing is not free.
    #[test]
    fn an_inactive_look_inserts_no_stage() {
        let cube = a_cube();
        let no_file = Edits::default();
        assert!(!look_stage_names(&no_file, Some(&cube)).contains(&"creative_look"));

        let zero_strength = Edits {
            look: edit_model::Look {
                file: Some("x.cube".into()),
                strength: 0.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!look_stage_names(&zero_strength, Some(&cube)).contains(&"creative_look"));
    }

    /// ⚠⚠ An identity profile curve is **ignored** — the X-T4's is exactly that, so honouring it
    /// would leave the setting silently doing nothing on real hardware. Both directions, because
    /// a test of one alone passes against a version that always uses the built-in.
    #[test]
    fn an_identity_profile_curve_is_ignored_and_a_real_one_is_not() {
        assert!(
            is_identity(&[[0.0, 0.0], [1.0, 1.0]]),
            "the X-T4's curve reads as real"
        );
        assert!(is_identity(&[[0.5, 0.5]]), "a single point is not a curve");
        assert!(!is_identity(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]));
    }

    /// ⚠⚠ **And the pipeline actually consults it** — a test of `is_identity` alone went
    /// **uncaught** when the `.filter()` was removed, because nothing observed which curve the
    /// builder picked. Rendered output is the only thing that can see it.
    #[test]
    fn the_builder_falls_back_to_the_built_in_for_an_identity_curve() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let render = |curve: Option<&[[f32; 2]]>| {
            let (prefix, tail) = build_pipeline_split_in_region_with_brush(
                &edits,
                RenderScale::new(1.0),
                None,
                None,
                RenderSpec::default(),
                None,
                ProfileRendering {
                    curve,
                    // The transfer is what this test is about; a baseline shift would move the
                    // probe along it and blur what the assertion is comparing.
                    baseline_ev: Some(0.0),
                    ..ProfileRendering::default()
                },
            );
            let mut img = ImageBuf::new(1, 1);
            img.set_pixel(0, 0, [0.05, 0.05, 0.05]);
            tail.run(prefix.run(img)).pixel(0, 0)[0]
        };

        let identity: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];
        let real: [[f32; 3 - 1]; 3] = [[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]];
        let built_in = render(None);

        assert!(
            (render(Some(&identity)) - built_in).abs() < 1e-6,
            "an identity profile curve was honoured instead of ignored"
        );
        assert!(
            (render(Some(&real)) - built_in).abs() > 0.01,
            "a real profile curve was ignored"
        );
    }
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

    /// An `Edits` with every field non-neutral — so a stage that has drifted into the wrong
    /// half of the split shows up.
    ///
    /// Every field is listed explicitly rather than using `..Default::default()`, on purpose:
    /// adding a field to `Edits` then breaks this function's compilation, forcing whoever
    /// adds it to decide which half of the split it belongs to.
    /// ⚠⚠ Total over the **whole prefix** rather than over `GeometryStage` alone, because the
    /// identity default is what every other stage inherits — and a default that is silently wrong
    /// for a future dim-changing stage is exactly the trap this guards. A region-scoped render
    /// takes the frame's dimensions from this prediction, so a stage that lies here rasterises
    /// every brush mask at the wrong size.
    #[test]
    fn every_prefix_stages_output_dims_match_what_it_produces() {
        let mut src = ImageBuf::new(37, 23);
        for y in 0..23 {
            for x in 0..37 {
                src.set_pixel(x, y, [x as f32 / 37.0, y as f32 / 23.0, 0.5]);
            }
        }
        let steps = build_prefix_steps(
            &all_edits(),
            RenderScale::new(1.0),
            None,
            Resampler::default(),
            None,
        );
        assert!(
            steps.len() >= 4,
            "fixture check: the fixture must build a real prefix, got {}",
            steps.len()
        );
        let mut img = src;
        for step in &steps {
            let predicted = step.stage.output_dims(img.width(), img.height());
            let out = step.stage.apply(img.clone());
            assert_eq!(
                predicted,
                (out.width(), out.height()),
                "`{}` predicted {predicted:?} and produced {:?}",
                step.stage.name(),
                (out.width(), out.height())
            );
            img = out;
        }
    }

    fn all_edits() -> Edits {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4000.0),
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: -15.0,
                },
                highlights: edit_model::GradeWheel {
                    hue: 45.0,
                    saturation: 35.0,
                    luminance: 10.0,
                },
                blending: 60.0,
                balance: 15.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
            dehaze: edit_model::Dehaze { amount: -30.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -30.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 60.0,
                masking: 40.0,
                ..Default::default()
            },
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                tca: false,
                vignetting: true,
                scale: 80.0,
            }),
            perspective: edit_model::Perspective {
                vertical: 30.0,
                horizontal: -15.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 2.0,
                orientation: edit_model::Orientation {
                    rotate: edit_model::Quarter::Cw90,
                    mirror: true,
                },
                ..Default::default()
            },
            version: 0,
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                // ⚠⚠ Non-empty **on purpose**, for exactly the reason the comment below gives
                // about `LocalAdjustments`: a fixture that names exhaustiveness must actually
                // exercise the field. An empty list here would let a composite travel untested
                // through every consumer this fixture exists to check.
                refine: vec![edit_model::Refinement {
                    op: edit_model::MaskOp::Subtract,
                    mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
                }],
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.75,
                    // ⚠⚠ Set by hand, because this fixture **does not fail to compile** when
                    // `LocalAdjustments` grows — it fills the rest with `..Default::default()`.
                    // Left alone, three fields whose entire purpose is to be non-neutral would
                    // land untested by the very construct that exists to force a decision about
                    // them, and nothing would say so.
                    local_contrast: edit_model::LocalContrast {
                        texture: 35.0,
                        clarity: -25.0,
                    },
                    sharpening: edit_model::Sharpening {
                        amount: 60.0,
                        ..Default::default()
                    },
                    hsl: edit_model::Hsl {
                        bands: [edit_model::HslBand {
                            hue: 8.0,
                            saturation: -15.0,
                            luminance: 5.0,
                        }; 8],
                    },
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 35.0,
                        color: 25.0,
                        ..Default::default()
                    },
                    dehaze: edit_model::Dehaze { amount: 40.0 },
                    tone_curve: edit_model::ToneCurve {
                        points: vec![[0.0, 0.0], [0.45, 0.6], [1.0, 1.0]],
                    },
                    color_grading: edit_model::ColorGrading {
                        shadows: edit_model::GradeWheel {
                            hue: 210.0,
                            saturation: 30.0,
                            luminance: -8.0,
                        },
                        blending: 60.0,
                        ..edit_model::ColorGrading::default()
                    },
                    ..Default::default()
                },
            }],
            spots: vec![edit_model::Spot {
                centre: [0.3, 0.4],
                source: [0.5, 0.4],
                radius: 0.05,
                feather: 30.0,
                mode: edit_model::SpotMode::Heal,
            }],
            rendering: Some(edit_model::Rendering::Camera),
            look: edit_model::Look {
                file: Some("velvia.cube".into()),
                space: edit_model::LutSpace::SrgbDisplay,
                strength: 65.0,
            },
        }
    }

    #[test]
    fn prefix_key_ignores_every_downstream_field() {
        // Proves every edit field is either in the key or downstream of the checkpoint.
        // If a field is added to the prefix without being added to PrefixKey, the cache
        // would silently serve stale pixels; this is what stands in the way.
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL, Resampler::default());

        let mut hsl = edit_model::Hsl::default();
        hsl.bands[3].hue = 40.0;
        let downstream = [
            Edits {
                white_balance: edit_model::WhiteBalance {
                    kelvin: Some(3500.0),
                    tint: -20.0,
                },
                ..base.clone()
            },
            Edits {
                exposure: 2.0,
                ..base.clone()
            },
            Edits {
                contrast: 50.0,
                ..base.clone()
            },
            Edits {
                highlights: -50.0,
                ..base.clone()
            },
            Edits {
                shadows: 50.0,
                ..base.clone()
            },
            Edits {
                whites: 30.0,
                ..base.clone()
            },
            Edits {
                blacks: -30.0,
                ..base.clone()
            },
            Edits {
                tone_curve: edit_model::ToneCurve {
                    points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
                },
                ..base.clone()
            },
            Edits {
                saturation: 40.0,
                ..base.clone()
            },
            Edits {
                vibrance: 40.0,
                ..base.clone()
            },
            Edits {
                hsl,
                ..base.clone()
            },
            Edits {
                color_grading: edit_model::ColorGrading {
                    highlights: edit_model::GradeWheel {
                        hue: 30.0,
                        saturation: 50.0,
                        luminance: 20.0,
                    },
                    blending: 10.0,
                    balance: 60.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 60.0,
                    clarity: -40.0,
                },
                ..base.clone()
            },
            Edits {
                vignette: edit_model::Vignette {
                    amount: -70.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 80.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            // ⚠ A look is tail-only, so it must NOT disturb the prefix key — otherwise every
            // strength drag would throw away the cached prefix and re-run demosaic and geometry.
            Edits {
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space: edit_model::LutSpace::Linear,
                    strength: 40.0,
                },
                ..base.clone()
            },
        ];
        for e in downstream {
            assert_eq!(
                PrefixKey::new(&e, RenderScale::FULL, Resampler::default()),
                k,
                "a downstream field changed the prefix key"
            );
        }
    }

    #[test]
    fn prefix_key_changes_with_prefix_fields_and_scale() {
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL, Resampler::default());

        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&geo, RenderScale::FULL, Resampler::default()),
            k
        );

        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&nr, RenderScale::FULL, Resampler::default()),
            k
        );

        let dh = Edits {
            dehaze: edit_model::Dehaze { amount: 40.0 },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&dh, RenderScale::FULL, Resampler::default()),
            k
        );

        // Scale changes the noise-reduction radii, so it belongs in the key.
        assert_ne!(
            PrefixKey::new(&base, RenderScale::new(0.25), Resampler::default()),
            k
        );
    }

    /// ⚠⚠ **That the wrapper entry points do not quietly substitute the default.** Every
    /// production render outside the engine goes through `build_pipeline_with` or
    /// `build_pipeline_to` — the canvas's `render_once`, every thumbnail, every export — and
    /// hardcoding `Resampler::default()` in either leaves all 49 goldens green, because the
    /// goldens ask for the default anyway. Nothing else in the suite can see it.
    #[test]
    fn the_pipeline_builders_honour_the_kernel_they_are_handed() {
        // ⚠ A hard edge, not a ramp: every kernel reproduces a linear ramp almost exactly.
        let mut src = ImageBuf::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let v = if (x / 6) % 2 == 0 { 0.05 } else { 0.9 };
                src.set_pixel(x, y, [v, v, v]);
            }
        }
        let edits = Edits {
            geometry: edit_model::Geometry {
                angle: 4.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let with = |k| build_pipeline_with(&edits, RenderScale::FULL, k).run(src.clone());
        let to = |k| {
            build_pipeline_to(&edits, RenderScale::FULL, color::OutputSpace::Srgb, k)
                .run(src.clone())
        };
        assert_ne!(
            with(Resampler::Bilinear).data(),
            with(Resampler::Lanczos3).data(),
            "build_pipeline_with ignored its kernel"
        );
        assert_ne!(
            to(Resampler::Bilinear).data(),
            to(Resampler::Lanczos3).data(),
            "build_pipeline_to ignored its kernel"
        );
    }

    /// ⚠⚠ **What makes the resampler a live setting rather than one needing a restart.** The
    /// kernel lives in `Settings`, not in `Edits`, so nothing else in the key can notice it
    /// change — and a served checkpoint rendered with the old kernel is exactly how a control
    /// comes to look inert. `apply_camera_profile` had to clear `BufferCache` by hand for the
    /// same hazard; keying it makes the invalidation structural instead.
    ///
    /// ⚠ And the *other half*, which is the reason it is on the steps rather than beside
    /// `scale`: on a photo that resamples nothing, changing the kernel must invalidate nothing.
    #[test]
    fn the_resampler_changes_the_key_only_when_something_resamples() {
        let straightened = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_ne!(
            PrefixKey::new(&straightened, RenderScale::FULL, Resampler::Bilinear),
            PrefixKey::new(&straightened, RenderScale::FULL, Resampler::Lanczos3),
            "a straighten resamples, so its checkpoint depends on the kernel"
        );

        // ⚠ Not `Edits::default()`: a *fully neutral* photo has an empty prefix, so the keys
        // would agree whatever the rule. This one has a live prefix stage that resamples
        // nothing, which is the case the assertion is actually about.
        let denoised = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !build_prefix_steps(
                &denoised,
                RenderScale::FULL,
                None,
                Resampler::Bilinear,
                None
            )
            .is_empty(),
            "fixture check: this photo must have a prefix stage to key at all"
        );
        assert_eq!(
            PrefixKey::new(&denoised, RenderScale::FULL, Resampler::Bilinear),
            PrefixKey::new(&denoised, RenderScale::FULL, Resampler::Lanczos3),
            "nothing resamples here, so the kernel must not invalidate the checkpoint"
        );
    }

    #[test]
    fn a_truncated_prefix_key_ignores_the_stages_below_it() {
        // The whole feature rests on this: two edit sets that agree on the first n prefix
        // stages must share the checkpoint after stage n, however much they differ deeper.
        let a = all_edits();
        let b = Edits {
            dehaze: edit_model::Dehaze { amount: 95.0 },
            ..a.clone()
        };
        let steps_a = build_prefix_steps(&a, RenderScale::FULL, None, Resampler::default(), None);
        let steps_b = build_prefix_steps(&b, RenderScale::FULL, None, Resampler::default(), None);
        assert_eq!(steps_a.len(), 6);

        // Same up to and including noise reduction...
        for n in 0..=4 {
            assert_eq!(
                PrefixKey::upto(&steps_a, n, RenderScale::FULL),
                PrefixKey::upto(&steps_b, n, RenderScale::FULL),
                "checkpoint {n} should be shared"
            );
        }
        // ...and different once dehaze has run.
        assert_ne!(
            PrefixKey::upto(&steps_a, 5, RenderScale::FULL),
            PrefixKey::upto(&steps_b, 5, RenderScale::FULL)
        );

        // The full key is the deepest truncation, so `new` and `upto` cannot disagree.
        assert_eq!(
            PrefixKey::new(&a, RenderScale::FULL, Resampler::default()),
            PrefixKey::upto(&steps_a, steps_a.len(), RenderScale::FULL)
        );

        // Scale is part of every checkpoint, including the bare seed: it sets the radii of the
        // stages that follow.
        assert_ne!(
            PrefixKey::upto(&steps_a, 0, RenderScale::FULL),
            PrefixKey::upto(&steps_a, 0, RenderScale::new(0.25))
        );
    }

    #[test]
    fn a_none_region_split_is_identical_to_the_plain_split() {
        for edits in [Edits::default(), all_edits()] {
            let (p1, t1) = build_pipeline_split(&edits, RenderScale::FULL);
            let (p2, t2) = build_pipeline_split_in_region(&edits, RenderScale::FULL, None);
            assert_eq!(p1.stage_names(), p2.stage_names());
            assert_eq!(t1.stage_names(), t2.stage_names());
        }
    }

    #[test]
    fn build_pipeline_is_exactly_prefix_then_tail() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
            let mut expected = prefix.stage_names();
            expected.extend(tail.stage_names());
            assert_eq!(
                build_pipeline(&edits, RenderScale::FULL).stage_names(),
                expected
            );
        }
    }

    /// ⚠⚠ The tapped tail is the tail, cut in two at the grading stage — for a grade that is
    /// active AND for one that is not, because Blending and Balance define the ranges whether or
    /// not a wheel is turned, and the split must be structural rather than "find the stage
    /// named color_grading". Sabotage: cutting one stage late fails the second assertion.
    #[test]
    fn the_tapped_tail_concatenates_to_the_tail_and_splits_at_the_grade() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
            let (tapped_prefix, halves) = build_pipeline_split_tapped(
                &edits,
                RenderScale::FULL,
                None,
                None,
                RenderSpec::default(),
                None,
                ProfileRendering::default(),
            );
            assert_eq!(tapped_prefix.stage_names(), prefix.stage_names());
            let mut joined = halves.before_grade.stage_names();
            joined.extend(halves.from_grade.stage_names());
            assert_eq!(joined, tail.stage_names(), "the halves are not the tail");
            assert!(
                !halves.before_grade.stage_names().contains(&"color_grading"),
                "the grade is in the first half"
            );
            if edits.color_grading.is_active() {
                assert_eq!(
                    halves.from_grade.stage_names().first(),
                    Some(&"color_grading"),
                    "an active grade must open the second half"
                );
            }
        }
        // The fixture must exercise both branches, or the `if` above is never taken.
        assert!(all_edits().color_grading.is_active());
        assert!(!Edits::default().color_grading.is_active());
    }

    #[test]
    fn the_prefix_holds_the_capture_stages_and_the_capture_masks() {
        // The tripwire: if a stage is later inserted into the prefix region, this fails and
        // forces PrefixKey to be updated with its parameters.
        //
        // ⚠ No lens profile is supplied here, so `lens_correction` is absent by construction —
        // the prefix starts at perspective.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert_eq!(
            prefix.stage_names(),
            vec![
                "perspective",
                "spot_removal",
                "geometry",
                "noise_reduction",
                "dehaze",
                // ⚠ The fixture's mask carries capture corrections, so it contributes a
                // whole-buffer `MaskStage` here — after the global pair, mirroring the tail's
                // rule that a mask acts on the global look.
                "mask"
            ]
        );
        for name in tail.stage_names() {
            assert!(
                name != "geometry" && name != "noise_reduction",
                "prefix stage {name} leaked into the tail"
            );
        }
        assert_eq!(tail.stage_names().last(), Some(&"output_transform"));
    }

    #[test]
    fn the_prefix_steps_are_the_prefix_pipeline() {
        // build_pipeline_split_in_region is defined through build_prefix_steps, so the two
        // cannot drift: a stage added to one appears in the other.
        for edits in [Edits::default(), all_edits()] {
            let steps =
                build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None);
            let (prefix, _) = build_pipeline_split(&edits, RenderScale::FULL);
            let from_steps: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
            assert_eq!(from_steps, prefix.stage_names());
        }
    }

    /// ⚠⚠ **Both directions, and the second is the one that matters.** The prefix key must
    /// change when a mask's noise reduction changes — and must NOT change when its *exposure*
    /// does, or every masked-exposure drag would re-run a whole-buffer denoise (117 ms at
    /// native) for a slider that has nothing to do with it. That is what `capture_projection`
    /// buys, and the "does not change" half is the only thing that can see it.
    #[test]
    fn the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest() {
        let base = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::default(),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 30.0,
                        ..Default::default()
                    },
                    exposure: 0.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        };
        let key = |e: &Edits| PrefixKey::new(e, RenderScale::FULL, Resampler::default());

        let mut louder = base.clone();
        louder.masks[0].adjustments.noise_reduction.luminance = 80.0;
        assert_ne!(
            key(&base),
            key(&louder),
            "a masked noise-reduction change must invalidate the prefix"
        );

        let mut brighter = base.clone();
        brighter.masks[0].adjustments.exposure = 2.0;
        assert_eq!(
            key(&base),
            key(&brighter),
            "a masked EXPOSURE change must not re-run a whole-buffer denoise"
        );

        let mut hazier = base.clone();
        hazier.masks[0].adjustments.dehaze.amount = 40.0;
        assert_ne!(
            key(&base),
            key(&hazier),
            "masked dehaze must invalidate too"
        );

        let mut moved = base.clone();
        moved.masks[0].mask = edit_model::Mask::Radial(edit_model::RadialGradient::default());
        assert_ne!(
            key(&base),
            key(&moved),
            "the mask's SHAPE decides which pixels are denoised, so it belongs in the key"
        );

        // ⚠⚠ A refinement is part of the shape too, and it is the direction the other four
        // cannot see: it is neither a capture correction nor a tail adjustment, so a
        // `capture_projection` that dropped it would leave every assertion above passing while a
        // subtracted component silently kept a stale masked denoise on screen.
        let mut refined = base.clone();
        refined.masks[0].refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
        });
        assert_ne!(
            key(&base),
            key(&refined),
            "a refinement changes which pixels are denoised, so it belongs in the key"
        );
    }

    /// ⚠ The fixtures do not fail to compile when `LocalAdjustments` grows, so this asserts what
    /// the type system cannot. Part A's equivalent caught exactly this omission.
    #[test]
    fn the_exhaustive_fixture_carries_masked_capture_corrections() {
        let a = &all_edits().masks[0].adjustments;
        assert!(
            a.noise_reduction.is_active(),
            "all_edits' mask must carry a real noise reduction"
        );
        assert!(a.dehaze.is_active(), "and a real dehaze");
    }

    #[test]
    fn every_prefix_step_has_its_own_key() {
        // Each step's key must move with that step's parameters and with nothing else. If two
        // stages shared a key, a checkpoint would be reused across a change that altered it.
        let base = all_edits();
        let keys = |e: &Edits| -> Vec<PrefixStepKey> {
            build_prefix_steps(e, RenderScale::FULL, None, Resampler::default(), None)
                .iter()
                .map(|s| s.key.clone())
                .collect()
        };
        let base_keys = keys(&base);
        assert_eq!(
            base_keys.len(),
            6,
            "all six prefix stages should be active — the five global ones plus the fixture's \
             masked capture correction"
        );

        let variants = [
            (
                0,
                Edits {
                    perspective: edit_model::Perspective {
                        vertical: 70.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                1,
                Edits {
                    spots: vec![edit_model::Spot {
                        centre: [0.7, 0.7],
                        source: [0.2, 0.2],
                        radius: 0.09,
                        feather: 10.0,
                        mode: edit_model::SpotMode::Clone,
                    }],
                    ..base.clone()
                },
            ),
            (
                2,
                Edits {
                    geometry: edit_model::Geometry {
                        angle: 7.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                3,
                Edits {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 80.0,
                        color: 80.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                4,
                Edits {
                    dehaze: edit_model::Dehaze { amount: 90.0 },
                    ..base.clone()
                },
            ),
            // ⚠ The masked capture correction is a prefix step like any other, and its key must
            // move with its own parameters and nothing else — the same property the five above
            // assert, extended to the step this milestone added.
            (5, {
                let mut e = base.clone();
                e.masks[0].adjustments.noise_reduction.luminance = 90.0;
                e
            }),
        ];
        for (moved, e) in variants {
            let ks = keys(&e);
            for i in 0..6 {
                if i == moved {
                    assert_ne!(ks[i], base_keys[i], "step {i} should have changed");
                } else {
                    assert_eq!(ks[i], base_keys[i], "step {i} should not have changed");
                }
            }
        }
    }

    #[test]
    fn running_prefix_then_tail_matches_the_whole_pipeline() {
        // The numerical counterpart to the name-based test above: bit-identical, not close.
        let edits = all_edits();
        let mut img = ImageBuf::new(16, 12);
        for y in 0..12 {
            for x in 0..16 {
                img.set_pixel(x, y, [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
            }
        }
        let whole = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
        let split = tail.run(prefix.run(img));
        assert_eq!(whole.data(), split.data());
    }

    #[test]
    fn render_scale_full_is_one() {
        assert_eq!(RenderScale::FULL.get(), 1.0);
    }

    #[test]
    fn render_scale_clamps_nonsense_input() {
        // A scale above 1 or at/below 0 is a caller bug, but the render worker must not
        // panic over it — a clamped radius is a recoverable wrong answer.
        assert_eq!(RenderScale::new(4.0).get(), 1.0);
        assert_eq!(RenderScale::new(0.0).get(), RenderScale::new(-1.0).get());
        assert!(RenderScale::new(0.0).get() > 0.0);
        assert_eq!(RenderScale::new(f32::NAN).get(), 1.0);
        assert_eq!(RenderScale::new(0.25).get(), 0.25);
    }

    #[test]
    fn per_pixel_stages_ignore_the_render_scale() {
        // Everything that existed before noise reduction is per-pixel, so scale must be
        // a no-op for it — bit for bit, not just visually.
        let edits = Edits {
            exposure: 0.7,
            contrast: 25.0,
            saturation: 30.0,
            ..Edits::default()
        };
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.1 * x as f32, 0.2, 0.05 * y as f32]);
            }
        }
        let full = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let quarter = build_pipeline(&edits, RenderScale::new(0.25)).run(img);
        assert_eq!(full.data(), quarter.data());
    }

    /// The global tail must not apply the mask-only stages twice.
    ///
    /// ⚠⚠ The trap this exists for. `adjustment_stages` has two consumers — the global tail and
    /// every mask — and the global tail places the tone curve, local contrast, HSL, colour
    /// grading and sharpening **itself**, further down. So it passes those five **neutral** into
    /// the shared builder. Passing the real values applies each twice: visibly over-sharpened,
    /// and every golden moves.
    ///
    /// ⚠⚠ **IT NAMED THREE OF FIVE AND WAS THEREFORE BLIND TO A WHOLE MILESTONE.** When the tone
    /// curve and colour grading joined the shared builder, the double-application sabotage left
    /// this test **green** and was caught only by six moving goldens. A guard that enumerates a
    /// list has to be extended with the list, or it silently stops guarding the newest entries —
    /// which are exactly the ones nobody has habits about yet. Add every field the caller passes
    /// neutral, and add it here in the same commit.
    ///
    /// ⚠ This was written *before* the stages joined the shared builder, so it passed from the
    /// start — a tripwire, not a red test. What makes it a real guard is the sabotage recorded
    /// with it: hand `edits.local_contrast` to the builder and watch this fail.
    /// Two masked clarities reach twice as far as one.
    ///
    /// ⚠⚠ **A ONE-MASK TEST CANNOT SEE SUM-VERSUS-MAX** — with a single mask the two are
    /// identical, so the wrong arithmetic passes. Two is the smallest fixture that discriminates,
    /// the same shape as the same-month-in-two-years fixture.
    ///
    /// ⚠ The reaches compose because mask N runs on mask N-1's output, so a pixel at the region
    /// edge reads through every mask in turn. Getting this wrong makes the region margin too
    /// small, and the failure is silent: a subtly wrong band at the edge of the visible area.
    #[test]
    fn two_masked_clarities_reach_further_than_one() {
        let masked = |clarity: f32| edit_model::MaskedAdjustment {
            mask: edit_model::Mask::default(),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                local_contrast: edit_model::LocalContrast {
                    texture: 0.0,
                    clarity,
                },
                ..Default::default()
            },
        };
        let one = Edits {
            masks: vec![masked(100.0)],
            ..Edits::default()
        };
        let two = Edits {
            masks: vec![masked(100.0), masked(100.0)],
            ..Edits::default()
        };

        let r1 = tail_kernel_radius(&one, RenderScale::FULL);
        let r2 = tail_kernel_radius(&two, RenderScale::FULL);
        assert!(r1 > 0, "a masked clarity must contribute reach at all");
        assert_eq!(
            r2,
            2 * r1,
            "the reaches compose: two masks reach twice as far, not the same"
        );
    }

    #[test]
    fn the_global_tail_does_not_apply_the_mask_only_stages_twice() {
        let edits = Edits {
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 40.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 70.0,
                ..Default::default()
            },
            hsl: edit_model::Hsl {
                bands: [edit_model::HslBand {
                    hue: 20.0,
                    saturation: 0.0,
                    luminance: 0.0,
                }; 8],
            },
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.45, 0.6], [1.0, 1.0]],
            },
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 210.0,
                    saturation: 30.0,
                    luminance: -8.0,
                },
                ..edit_model::ColorGrading::default()
            },
            ..Edits::default()
        };
        let names = build_pipeline(&edits, RenderScale::FULL).stage_names();
        for stage in [
            "local_contrast",
            "sharpening",
            "hsl",
            "tone_curve",
            "color_grading",
        ] {
            assert_eq!(
                names.iter().filter(|n| **n == stage).count(),
                1,
                "{stage} ran twice: {names:?}"
            );
        }
    }

    /// A mask's tone curve runs after its tone regions and before its local contrast.
    ///
    /// ⚠⚠ **Order, not merely presence.** A masked curve that ran in the wrong place still
    /// produces a plausible photograph, so a test asserting only that the stage exists would
    /// pass against the wrong chain. The rule it pins is the global one, mirrored: a local
    /// contrast measured before the curve would then be re-mapped by it.
    ///
    /// ⚠ Asserted on the **mask's own sub-pipeline**, which `stage_names()` cannot see through
    /// `MaskStage`, so it calls the shared builder directly.
    #[test]
    fn a_masked_tone_curve_runs_after_the_tone_regions_and_before_local_contrast() {
        let a = edit_model::LocalAdjustments {
            shadows: 20.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]],
            },
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 30.0,
            },
            ..Default::default()
        };
        let names: Vec<&'static str> =
            adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
                .iter()
                .map(|s| s.name())
                .collect();
        let at = |n: &str| names.iter().position(|x| *x == n);
        assert_eq!(
            names.iter().filter(|n| **n == "tone_curve").count(),
            1,
            "exactly one curve stage: {names:?}"
        );
        assert!(
            at("tone_regions") < at("tone_curve"),
            "the curve must follow the tone regions: {names:?}"
        );
        assert!(
            at("tone_curve") < at("local_contrast"),
            "the curve must precede local contrast: {names:?}"
        );
    }

    /// An empty curve inserts no stage at all.
    ///
    /// ⚠ Exactly the global rule: empty is what keeps `ToneCurveStage` out of the pipeline, so a
    /// mask with a default curve pays nothing rather than running a 1024-entry identity LUT.
    #[test]
    fn a_masked_curve_with_no_points_inserts_no_stage() {
        let a = edit_model::LocalAdjustments {
            exposure: 0.5,
            ..Default::default()
        };
        let names: Vec<&'static str> =
            adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
                .iter()
                .map(|s| s.name())
                .collect();
        assert!(!names.contains(&"tone_curve"), "{names:?}");
    }

    /// A mask's colour grade runs after its HSL and before its sharpening.
    ///
    /// ⚠⚠ **After HSL is the load-bearing half.** HSL corrects colours that are *in* the picture
    /// while a grade is laid on top; reversed, an HSL band tweak would act on the tint just
    /// added. Before sharpening for the reason sharpening is last anywhere: it must not amplify
    /// what a later stage would then stretch.
    #[test]
    fn a_masked_colour_grade_runs_after_hsl_and_before_sharpening() {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        let a = edit_model::LocalAdjustments {
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 220.0,
                    saturation: 45.0,
                    luminance: -12.0,
                },
                ..edit_model::ColorGrading::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let names: Vec<&'static str> =
            adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
                .iter()
                .map(|s| s.name())
                .collect();
        let at = |n: &str| names.iter().position(|x| *x == n);
        assert_eq!(
            names.iter().filter(|n| **n == "color_grading").count(),
            1,
            "exactly one grade stage: {names:?}"
        );
        assert!(
            at("hsl") < at("color_grading"),
            "the grade must follow HSL: {names:?}"
        );
        assert!(
            at("color_grading") < at("sharpening"),
            "the grade must precede sharpening: {names:?}"
        );
    }

    /// Blending and balance alone insert no stage.
    ///
    /// ⚠ The rule `NoiseReduction::is_active` and `Vignette::is_active` already follow, reused
    /// here rather than reinvented: they shape ranges that tint nothing until a wheel is turned
    /// up, so dragging one alone must not insert a stage that would cost an Oklab round trip for
    /// no change at all.
    #[test]
    fn masked_grading_modifiers_alone_insert_no_stage() {
        let a = edit_model::LocalAdjustments {
            exposure: 0.5,
            color_grading: edit_model::ColorGrading {
                blending: 90.0,
                balance: -40.0,
                ..edit_model::ColorGrading::default()
            },
            ..Default::default()
        };
        let names: Vec<&'static str> =
            adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
                .iter()
                .map(|s| s.name())
                .collect();
        assert!(!names.contains(&"color_grading"), "{names:?}");
    }

    /// A mask's capture corrections run in the **prefix**, after the global pair, in mask order.
    ///
    /// ⚠⚠ This is the whole placement decision as a test. In the tail a masked dehaze would
    /// re-estimate its airlight from the visible region and change as the user panned.
    #[test]
    fn masked_capture_corrections_run_in_the_prefix_after_the_global_pair() {
        let masked = |amount: f32| edit_model::MaskedAdjustment {
            mask: edit_model::Mask::default(),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount },
                ..Default::default()
            },
        };
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 20.0 },
            masks: vec![masked(50.0), masked(70.0)],
            ..Edits::default()
        };
        let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
        assert_eq!(
            prefix.stage_names(),
            vec!["noise_reduction", "dehaze", "mask", "mask"],
            "the capture masks follow the global pair, one per active mask"
        );
        assert!(
            tail.stage_names()
                .iter()
                .all(|n| *n != "noise_reduction" && *n != "dehaze"),
            "no capture stage may leak into the tail: {:?}",
            tail.stage_names()
        );
    }

    /// A mask with no capture correction costs nothing in the prefix.
    #[test]
    fn a_mask_without_capture_corrections_adds_no_prefix_stage() {
        let edits = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::default(),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.8,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        };
        let (prefix, _) = build_pipeline_split(&edits, RenderScale::FULL);
        assert!(
            prefix.stage_names().is_empty(),
            "{:?}",
            prefix.stage_names()
        );
    }

    /// Inside one mask, noise reduction precedes dehaze — the global prefix's own order, and for
    /// its reason: dehaze is a contrast stretch that amplifies noise.
    #[test]
    fn a_masks_capture_stages_denoise_before_dehazing() {
        let a = edit_model::LocalAdjustments {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 50.0 },
            ..Default::default()
        };
        let names: Vec<&'static str> = capture_stages(&a, RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
        assert_eq!(names, vec!["noise_reduction", "dehaze"]);
    }

    /// ⚠ `capture_stages` and `adjustment_stages` **partition** a mask's adjustments — a stage
    /// built by both would run twice.
    #[test]
    fn the_two_mask_builders_do_not_overlap() {
        let a = all_edits().masks[0].adjustments.clone();
        let cap: Vec<&'static str> = capture_stages(&a, RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
        let tail: Vec<&'static str> =
            adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
                .iter()
                .map(|s| s.name())
                .collect();
        for n in &cap {
            assert!(!tail.contains(n), "{n} is built by both builders");
        }
        assert!(
            !cap.is_empty(),
            "fixture check: all_edits' mask must carry capture corrections"
        );
    }

    #[test]
    fn stage_names_are_reported_in_order() {
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["exposure", "saturation", "output_transform"]
        );
    }

    #[test]
    fn output_transform_encodes_linear_to_srgb() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.5, 0.5, 0.5]);
        let out = build_pipeline(&Edits::default(), RenderScale::FULL).run(img);
        let [r, g, b] = out.pixel(0, 0);
        let expected = color::linear_to_srgb(0.5);
        assert!((r - expected).abs() < 1e-4);
        assert!((g - expected).abs() < 1e-4);
        assert!((b - expected).abs() < 1e-4);
    }

    #[test]
    fn default_pipeline_has_output_transform_last() {
        let p = build_pipeline(&Edits::default(), RenderScale::FULL);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn nonneutral_edits_add_stages_before_output_transform() {
        let edits = Edits {
            exposure: 1.0,
            contrast: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 3);
    }

    #[test]
    fn hsl_adds_one_stage_when_non_neutral() {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[2].saturation = 30.0;
        let edits = Edits {
            hsl,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn neutral_hsl_adds_no_stage() {
        let edits = Edits {
            hsl: edit_model::Hsl::default(),
            saturation: 10.0,
            ..Edits::default()
        };
        // saturation stage + output transform only
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn saturation_adds_one_stage() {
        let edits = Edits {
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn vibrance_alone_adds_one_stage() {
        let edits = Edits {
            vibrance: -30.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn neutral_color_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn noise_reduction_runs_before_every_adjustment() {
        // Early placement is the whole point: denoise the source rather than fight noise
        // that a Shadows lift has already multiplied up. Geometry stays first so NR runs
        // on the cropped pixels.
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                color: 40.0,
                ..Default::default()
            },
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4500.0),
                ..Default::default()
            },
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "geometry",
                "noise_reduction",
                "white_balance",
                "exposure",
                "output_transform"
            ]
        );
    }

    #[test]
    fn noise_reduction_modifiers_alone_add_no_stage() {
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 0.0,
                detail: 100.0,
                smoothness: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn vignette_runs_last_before_the_output_transform() {
        // A creative effect laid over the finished grade.
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["exposure", "saturation", "vignette", "output_transform"]
        );
    }

    #[test]
    fn vignette_modifiers_alone_add_no_stage() {
        let edits = Edits {
            vignette: edit_model::Vignette {
                amount: 0.0,
                midpoint: 10.0,
                roundness: -100.0,
                feather: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn sharpening_runs_after_the_vignette_and_before_the_output_transform() {
        // Sharpening a vignetted frame is correct for a ratio-based model: darkened
        // corners sharpen proportionally less in absolute terms.
        let edits = Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["exposure", "vignette", "sharpening", "output_transform"]
        );
    }

    #[test]
    fn sharpening_modifiers_alone_add_no_stage() {
        let edits = Edits {
            sharpening: edit_model::Sharpening {
                amount: 0.0,
                radius: 3.0,
                detail: 0.0,
                masking: 90.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn local_contrast_runs_after_the_tone_curve_and_before_saturation() {
        // After the tone curve is mandatory: a local contrast computed before it would then
        // be re-mapped by it — the same argument that kept the vignette out of the prefix.
        // Before the colour stages so they see final tonality.
        let edits = Edits {
            exposure: 1.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: 20.0,
            },
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "tone_curve",
                "local_contrast",
                "saturation",
                "output_transform"
            ]
        );
    }

    #[test]
    fn neutral_local_contrast_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            local_contrast: edit_model::LocalContrast::default(),
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn local_contrast_is_in_the_tail() {
        // It must not join the prefix: it has to be downstream of the tone curve, and
        // putting it upstream would also make every Texture drag invalidate the noise
        // reduction cache.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert!(!prefix.stage_names().contains(&"local_contrast"));
        assert!(tail.stage_names().contains(&"local_contrast"));
    }

    #[test]
    fn tail_kernel_radius_covers_every_spatial_tail_stage() {
        let neutral = Edits::default();
        assert_eq!(tail_kernel_radius(&neutral, RenderScale::FULL), 0);

        // Sharpening alone at its default radius: ceil(3*1.0) + ceil(3*2.0) = 9.
        let sharp = Edits {
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&sharp, RenderScale::FULL), 9);

        // Clarity alone: wide_blur_reach(40.0) = 120, far wider than sharpening ever gets.
        let clarity = Edits {
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 50.0,
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&clarity, RenderScale::FULL), 120);

        // ⚠ Both: the **sum**, not the max — and this assertion used to say 120, with a
        // comment claiming each stage "reads its own neighbourhood of that stage's input".
        // That reasoning is wrong, and masking disproved it empirically: sharpening runs on
        // local contrast's *output*, so a wrong value at the region edge travels 120 px
        // through clarity and a further 9 px through sharpening. The old inset was ~9 px too
        // small and the region-equality test was passing only because the residual
        // differences happened to round the same way; a masked +0.6 EV pushed them across a
        // quantisation boundary and it failed.
        let both = Edits {
            sharpening: sharp.sharpening,
            local_contrast: clarity.local_contrast,
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&both, RenderScale::FULL), 129);

        // ⚠⚠ There used to be a worst-case bound here — `max_tail_kernel_radius`, asserted to
        // cover every representable edit set. **That premise is now false and the function is
        // gone.** Masks carry spatial adjustments and chain, so the reach is unbounded in the
        // number of masks; no constant bounds it. The replacement is that the *caller* derives
        // its margin from this function per render, which is what `Session::render_margin` does.
        //
        // What survives is the half that still means something: the reach must shrink with the
        // render scale, or a preview would over-reserve.
        assert!(
            tail_kernel_radius(&both, RenderScale::new(0.25))
                < tail_kernel_radius(&both, RenderScale::FULL),
            "the reach must shrink with the render scale"
        );
    }

    #[test]
    fn geometry_is_prepended_before_other_stages() {
        let edits = Edits {
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 3);
    }

    #[test]
    fn colour_grading_runs_after_hsl_and_before_the_masks() {
        // After HSL because HSL *corrects* colours that are in the photograph while a grade is
        // a look laid over the top — reversed, an HSL band tweak would act on the tint just
        // added. Before the masks because a mask "should act on the global look rather than be
        // re-mapped by it", and a grade is global look.
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        let edits = Edits {
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: 0.0,
                },
                ..Default::default()
            },
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.75,
                    ..Default::default()
                },
            }],
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "hsl",
                "color_grading",
                "mask",
                "vignette",
                "output_transform"
            ]
        );
    }

    #[test]
    fn colour_grading_modifiers_alone_add_no_stage() {
        let edits = Edits {
            color_grading: edit_model::ColorGrading {
                blending: 100.0,
                balance: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    /// A corrector built from a synthetic profile — no RAW file, no bundled database.
    fn test_corrector() -> Option<lens_profile::LensShot> {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        Some(lens_profile::LensShot {
            profile: p,
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        })
    }

    #[test]
    fn lens_correction_runs_first_in_the_prefix() {
        // Before geometry, because the calibration is defined in the uncropped sensor frame.
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let steps = build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default(),
            None,
        );
        let names: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
        assert_eq!(names, vec!["lens_correction", "geometry"]);
    }

    #[test]
    fn lens_correction_is_skipped_when_no_profile_matched() {
        // ⚠ The ordinary case for an unlisted lens, and it must be silent rather than an error.
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Edits::default()
        };
        assert!(
            build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None)
                .is_empty()
        );
    }

    #[test]
    fn the_lens_scale_alone_inserts_no_stage() {
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                scale: 0.0,
                ..Default::default()
            }),
            ..Edits::default()
        };
        assert!(build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default(),
            None
        )
        .is_empty());
    }

    /// ⚠ Neutral must be **bit-exact, not trusted** — the `kelvin: None` rule, and the reason
    /// every existing golden stays byte-identical through this feature.
    #[test]
    fn no_spots_means_no_stage() {
        let edits = Edits::default();
        let steps = build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None);
        assert!(steps.is_empty());
        assert!(!build_pipeline(&edits, RenderScale::FULL)
            .stage_names()
            .contains(&"spot_removal"));

        // And an empty list on an otherwise busy edit set inserts nothing either.
        let busy = Edits {
            exposure: 0.5,
            spots: Vec::new(),
            ..Edits::default()
        };
        assert!(!build_pipeline(&busy, RenderScale::FULL)
            .stage_names()
            .contains(&"spot_removal"));
    }

    /// ⚠⚠ The property the placement exists for, in the shape of the brush's real-`.RAF`
    /// validation: a spot is anchored in `GeometryStage`'s **input**, so cropping afterwards
    /// must land the patch on the same source content. Rendering the crop's window out of the
    /// uncropped render must equal the cropped render, bit for bit.
    ///
    /// ⚠ It fails if the stage is moved after geometry: the normalised centre would then refer
    /// to the cropped frame and the patch would slide.
    #[test]
    fn a_spot_survives_a_crop() {
        const W: usize = 64;
        const H: usize = 64;
        let mut img = ImageBuf::new(W, H);
        for y in 0..H {
            for x in 0..W {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / W as f32,
                        y as f32 / H as f32,
                        ((x * y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        let spot = edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.25, 0.25],
            radius: 0.125,
            feather: 0.0,
            mode: edit_model::SpotMode::Clone,
        };
        // A quarter-frame crop whose window still contains the whole patch.
        let crop = edit_model::CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let uncropped = Edits {
            spots: vec![spot],
            ..Edits::default()
        };
        let cropped = Edits {
            geometry: edit_model::Geometry {
                crop,
                ..Default::default()
            },
            ..uncropped.clone()
        };

        let whole = build_pipeline(&uncropped, RenderScale::FULL).run(img.clone());
        let part = build_pipeline(&cropped, RenderScale::FULL).run(img.clone());
        let (x0, y0) = (W / 4, H / 4);
        let window = whole.crop(x0, y0, W / 2, H / 2);
        assert_eq!(window.width(), part.width());
        assert_eq!(window.height(), part.height());

        // Guard against passing trivially: the patch must genuinely change these pixels.
        let neutral = build_pipeline(&Edits::default(), RenderScale::FULL).run(img);
        assert_ne!(
            neutral.crop(x0, y0, W / 2, H / 2).data(),
            part.data(),
            "the spot must actually alter the cropped window"
        );
        assert_eq!(window.data(), part.data());
    }
}

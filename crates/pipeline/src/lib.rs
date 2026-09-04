//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

pub mod brush;

/// Pre-rasterized coverage for a photo's brush masks, indexed by **position in `Edits::masks`**.
///
/// ⚠ Optional and additive, following the `Region` precedent: [`build_pipeline`] and
/// [`build_pipeline_split`] delegate with `None`, so the CLI, export and every golden compile
/// untouched.
#[derive(Default)]
pub struct BrushCoverage {
    pub maps: Vec<Option<std::sync::Arc<brush::CoverageMap>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i).and_then(|m| m.clone())
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

pub use resample::Resampler;
pub use stages::color_grading::ColorGradingStage;
pub use stages::contrast::ContrastStage;
pub use stages::creative_look::CreativeLookStage;
pub use stages::dehaze::DehazeStage;
pub use stages::display_rendering::DisplayRenderingStage;
pub use stages::exposure::ExposureStage;
pub use stages::geometry::GeometryStage;
pub use stages::hsl::HslStage;
pub use stages::lens_correction::LensCorrectionStage;
pub use stages::local_contrast::LocalContrastStage;
pub use stages::mask::MaskStage;
pub use stages::noise_reduction::NoiseReductionStage;
pub use stages::output_transform::OutputTransform;
pub use stages::saturation::SaturationStage;
pub use stages::sharpening::{max_kernel_radius, SharpenStage, RADIUS_RANGE};
pub use stages::spot_removal::SpotRemovalStage;
pub use stages::tone_curve::ToneCurveStage;
pub use stages::tone_regions::ToneRegionsStage;
pub use stages::vignette::VignetteStage;
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
}

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
) -> Vec<PrefixStep> {
    let mut steps: Vec<PrefixStep> = Vec::new();

    // ⚠ First, ahead of geometry: the calibration is defined in the uncropped, unrotated sensor
    // frame, so it must precede both the crop and the straighten. A missing corrector — an
    // unlisted lens, or a JPEG — skips the step silently, which is the ordinary case.
    if edits.lens_correction.is_active() {
        if let Some(c) = lens {
            steps.push(PrefixStep {
                key: PrefixStepKey::LensCorrection(edits.lens_correction, resampler),
                stage: Box::new(LensCorrectionStage::new(
                    c,
                    lens_profile::Corrections {
                        distortion: edits.lens_correction.distortion,
                        tca: edits.lens_correction.tca,
                        vignetting: edits.lens_correction.vignetting,
                        scale: edits.lens_correction.scale,
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
            stage: Box::new(DehazeStage::new(&edits.dehaze, scale)),
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
        let steps = build_prefix_steps(edits, scale, None, resampler);
        Self::upto(&steps, steps.len(), scale)
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
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            space: color::OutputSpace::Srgb,
            resampler: Resampler::default(),
            baseline: color::wb::Illuminant::default(),
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
/// ⚠⚠ **Three of these are passed NEUTRAL by the global caller** — `local_contrast`, `hsl` and
/// `sharpening` — because the global chain places each itself, at a position each has earned.
/// Passing the real values would apply them twice and move every golden. This is the same rule
/// `saturation` has followed here since masking landed; see the caller for both.
///
/// ⚠ `scale` reaches the spatial stages only. The per-pixel ones ignore it, which
/// `per_pixel_stages_ignore_the_render_scale` still asserts bit for bit — and that test now
/// doubles as the guard that the **global** path did not accidentally gain a spatial stage.
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
        let declared = color::wb::Illuminant::new(wb.kelvin.unwrap_or(baseline.kelvin), wb.tint);
        out.push(Box::new(WhiteBalanceStage::new(
            color::wb::adaptation_gains(declared, baseline, &color::spaces::PROPHOTO_TO_XYZ),
        )));
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
    // ⚠ Last, for the reason it is last globally: sharpening must not amplify anything a later
    // stage would then stretch.
    if a.sharpening.is_active() {
        out.push(Box::new(SharpenStage::new(&a.sharpening, scale)));
    }
    out
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

/// As [`build_pipeline_split_in_region`], with brush coverage the caller has already rasterized.
///
/// ⚠ The extra parameter is additive with a delegating wrapper — the `Region` precedent, which
/// gave that work zero call-site churn. The CLI, export and every golden go through the wrapper.
pub fn build_pipeline_split_in_region_with_brush(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    brush: Option<&BrushCoverage>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    // What the *tail's* rendering needs that `Edits` cannot carry.
    profile: ProfileRendering<'_>,
) -> (Pipeline, Pipeline) {
    let ProfileRendering {
        curve: profile_curve,
        baseline_ev,
        look: look_table,
    } = profile;
    let RenderSpec {
        space,
        resampler,
        baseline,
    } = spec;
    // Defined through `build_prefix_steps` so the pipeline and the cache keys cannot drift:
    // one list, two consumers.
    let prefix: Vec<Box<dyn PipelineStage>> = build_prefix_steps(edits, scale, lens, resampler)
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
            local_contrast: edit_model::LocalContrast::default(),
            hsl: edit_model::Hsl::default(),
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
                brush.and_then(|b| b.get(i)),
                baseline,
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

    match edits.rendering {
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
            let curve = profile_curve.filter(|p| !is_identity(p));
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

    (Pipeline::new(prefix), Pipeline::new(tail))
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
    sharp + local
}

/// [`tail_kernel_radius`] maximised over every representable setting — what a fixed render
/// margin has to exceed.
///
/// Defined *through* `tail_kernel_radius` rather than duplicating the maxima, so the two
/// cannot drift apart when a stage is added.
pub fn max_tail_kernel_radius(scale: RenderScale) -> usize {
    let worst = Edits {
        sharpening: edit_model::Sharpening {
            amount: 100.0,
            radius: RADIUS_RANGE.1,
            ..Default::default()
        },
        local_contrast: edit_model::LocalContrast {
            texture: 100.0,
            clarity: 100.0,
        },
        ..Edits::default()
    };
    tail_kernel_radius(&worst, scale)
}

#[cfg(test)]
mod tests {

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
            rendering: edit_model::Rendering::Camera,
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
                rendering: edit_model::Rendering::Camera,
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
                rendering: edit_model::Rendering::Camera,
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
                rendering: edit_model::Rendering::Camera,
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
            rendering: edit_model::Rendering::Camera,
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
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                tca: false,
                vignetting: true,
                scale: 80.0,
            },
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
            rendering: edit_model::Rendering::Camera,
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
            !build_prefix_steps(&denoised, RenderScale::FULL, None, Resampler::Bilinear).is_empty(),
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
        let steps_a = build_prefix_steps(&a, RenderScale::FULL, None, Resampler::default());
        let steps_b = build_prefix_steps(&b, RenderScale::FULL, None, Resampler::default());
        assert_eq!(steps_a.len(), 5);

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

    #[test]
    fn the_prefix_holds_perspective_spots_geometry_noise_reduction_and_dehaze() {
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
                "dehaze"
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
            let steps = build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default());
            let (prefix, _) = build_pipeline_split(&edits, RenderScale::FULL);
            let from_steps: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
            assert_eq!(from_steps, prefix.stage_names());
        }
    }

    #[test]
    fn every_prefix_step_has_its_own_key() {
        // Each step's key must move with that step's parameters and with nothing else. If two
        // stages shared a key, a checkpoint would be reused across a change that altered it.
        let base = all_edits();
        let keys = |e: &Edits| -> Vec<PrefixStepKey> {
            build_prefix_steps(e, RenderScale::FULL, None, Resampler::default())
                .iter()
                .map(|s| s.key.clone())
                .collect()
        };
        let base_keys = keys(&base);
        assert_eq!(
            base_keys.len(),
            5,
            "all five prefix stages should be active"
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
        ];
        for (moved, e) in variants {
            let ks = keys(&e);
            for i in 0..5 {
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
    /// every mask — and the global tail places local contrast, HSL and sharpening **itself**,
    /// further down. So it passes those three **neutral** into the shared builder. Passing the
    /// real values applies each twice: visibly over-sharpened, and every golden moves.
    ///
    /// ⚠ This was written *before* the stages joined the shared builder, so it passed from the
    /// start — a tripwire, not a red test. What makes it a real guard is the sabotage recorded
    /// with it: hand `edits.local_contrast` to the builder and watch this fail.
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
            ..Edits::default()
        };
        let names = build_pipeline(&edits, RenderScale::FULL).stage_names();
        for stage in ["local_contrast", "sharpening", "hsl"] {
            assert_eq!(
                names.iter().filter(|n| **n == stage).count(),
                1,
                "{stage} ran twice: {names:?}"
            );
        }
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

        // The worst case must bound every representable setting.
        let worst = max_tail_kernel_radius(RenderScale::FULL);
        for e in [&neutral, &sharp, &clarity, &both, &all_edits()] {
            assert!(
                tail_kernel_radius(e, RenderScale::FULL) <= worst,
                "an edit set exceeded max_tail_kernel_radius {worst}"
            );
        }
        // And it must be tighter at a smaller render scale, or a preview would over-reserve.
        assert!(max_tail_kernel_radius(RenderScale::new(0.25)) < worst);
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
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            },
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
        );
        let names: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
        assert_eq!(names, vec!["lens_correction", "geometry"]);
    }

    #[test]
    fn lens_correction_is_skipped_when_no_profile_matched() {
        // ⚠ The ordinary case for an unlisted lens, and it must be silent rather than an error.
        let edits = Edits {
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert!(
            build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default()).is_empty()
        );
    }

    #[test]
    fn the_lens_scale_alone_inserts_no_stage() {
        let edits = Edits {
            lens_correction: edit_model::LensCorrection {
                scale: 0.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert!(build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default()
        )
        .is_empty());
    }

    /// ⚠ Neutral must be **bit-exact, not trusted** — the `kelvin: None` rule, and the reason
    /// every existing golden stays byte-identical through this feature.
    #[test]
    fn no_spots_means_no_stage() {
        let edits = Edits::default();
        let steps = build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default());
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

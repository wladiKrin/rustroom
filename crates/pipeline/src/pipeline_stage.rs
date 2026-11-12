//! The stage API: `PipelineStage`, the `Pipeline` that runs a list of them, and the render scale.

use crate::{brush, per_pixel, CoverageRasterizer, Rect, Region, RegionSrc};
use core_image::ImageBuf;

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

    /// This stage's mask, geometry and region, when it **is** a mask. Default: it is not one.
    ///
    /// ⚠⚠ **This and [`Self::prebuilt_coverage`] exist because a `Box<dyn PipelineStage>` reports
    /// only `name` and `apply`.** That wall has now been hit three times — M8.3j needed a mask's
    /// adjustments, M8.9 needed a `LensShot`, and a GPU twin for the whole mask needs its *shape*.
    /// Downcasting every stage in the workspace was the only alternative.
    ///
    /// ⚠ It reports **composition**, exactly as [`Self::sub_stage_names`] does, which is the
    /// distinction M8.3g drew when it rejected a trait method for the byte-path question: that one
    /// would have leaked an *output-encoding* concern into a seam deliberately free of them.
    ///
    /// ⚠ Whole rather than piecemeal: a builder assembling this from three accessors is three
    /// chances to forget one, and the failure is a plausible photograph masked by the wrong shape.
    fn mask_description(
        &self,
    ) -> Option<(
        &edit_model::MaskedAdjustment,
        &edit_model::Geometry,
        Option<Region>,
    )> {
        None
    }

    /// This stage as a [`crate::SceneOffsetsStage`], for the GPU twin that needs its parts
    /// (M10.12b). `None` for every other stage.
    fn scene_offsets(&self) -> Option<&crate::SceneOffsetsStage> {
        None
    }

    /// Whether this stage blends by a coverage plane `SceneOffsetsStage` hands it from the reference
    /// photo (M10.12b) — a mask a GPU twin must not replace, since a twin would read its own base.
    fn takes_reference_coverage(&self) -> bool {
        false
    }

    /// The slot [`Self::takes_reference_coverage`] reads — what pairs a mask's GPU twin with the offsets stage's
    /// twin that fills it (M10.14h), by identity.
    fn reference_slot(&self) -> Option<std::sync::Arc<crate::CoverageSlot>> {
        None
    }

    /// Coverage maps this stage was handed already rasterized, one per component.
    ///
    /// ⚠⚠ **This is what makes a GATING brush accelerable.** Such a brush reads the photograph to
    /// decide its own coverage, so a GPU twin — which is built before any pixel exists — can only
    /// take a map somebody else already made. On a cache hit `render_engine` supplies one; on a
    /// miss the twin declines and the CPU stage rasterizes and deposits, warming the cache.
    ///
    /// ⚠ Default empty: the CLI, every export and all 63 goldens rasterize on the fly.
    fn prebuilt_coverage(&self) -> &[Option<std::sync::Arc<brush::CoverageMap>>] {
        &[]
    }

    /// This stage's output dimensions for an input of `w` x `h`. Default: unchanged.
    ///
    /// ⚠⚠ **The caller needs this BEFORE the stage runs.** A region-scoped render never produces
    /// the whole frame, and the frame's dimensions are what every brush mask's coverage is
    /// rasterised at. Taking them off a region-scoped buffer would rasterise every mask at the
    /// region's size — a wrong mask, and a coverage cache that thrashes on every pan.
    ///
    /// ⚠ The identity default is correct for every stage in the tree today: lens correction,
    /// spot removal, noise reduction, dehaze and the masked capture corrections all
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

    /// This stage as a per-pixel function for a `w` x `h` buffer, or `None` when it is not one.
    ///
    /// ⚠⚠ **`None` is the default, so a stage that reads neighbours declines BY CONSTRUCTION** —
    /// local contrast, sharpening, every prefix stage and a mask inherit it, and the runner then
    /// applies them on their own. The twelve pixel-in/pixel-out stages return `Some(self)`; the
    /// vignette builds a value for these dimensions, because its frame is the buffer's when there
    /// is no region.
    ///
    /// ⚠ Boxed because the vignette's answer is built per call; the other eleven box a borrow,
    /// which is one small allocation per stage per walk.
    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn per_pixel::PerPixel + '_>> {
        None
    }
}

/// Transform a stage's inner sub-stage list, given the adjustments it was built from.
///
/// ⚠ The alias carries a lifetime because the closure `gpu-pipeline` supplies **borrows**: it
/// holds the outer `TailRendering<'_>` and a `&Arc<GpuContext>`. A `'static` alias would force
/// those to be owned, and `TailRendering` is a borrow by construction.
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

    /// Whether the first `mask` stage holds a coverage rasteriser.
    ///
    /// ⚠ The observable for M8.3k's wiring — see [`PipelineStage::has_coverage_rasterizer`] for
    /// why a pixel comparison cannot serve.
    pub fn mask_has_coverage_rasterizer(&self) -> bool {
        self.stages
            .iter()
            .any(|s| s.name() == "mask" && s.has_coverage_rasterizer())
    }

    /// The sub-stage names of the first stage with this name, if there is one.
    ///
    /// ⚠ [`Pipeline::stage_names`] reports the top level only, and a `Box<dyn PipelineStage>`
    /// cannot be downcast — so without this a test can see *that* a pipeline contains a mask and
    /// nothing at all about what the mask contains, which is exactly where M8.3j's work happens.
    /// `None` means no stage of that name; an empty `Vec` means one carrying no sub-stages.
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
    ///
    /// ⚠ Through [`per_pixel::run_stages`], so a run of per-pixel stages is one walk. The list
    /// itself is untouched; see that module.
    pub fn run(&self, image: ImageBuf) -> ImageBuf {
        per_pixel::run_stages(self.stages.iter().map(|s| s.as_ref()), image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

#[cfg(test)]
mod per_pixel_tests {
    use super::*;
    use crate::{
        ColorGradingStage, ContrastStage, CreativeLookStage, DisplayRenderingStage, ExposureStage,
        HslStage, OutputTransform, SaturationStage, ToneCurveStage, ToneRegionsStage,
        VignetteStage, WhiteBalanceStage,
    };

    fn field() -> ImageBuf {
        let mut img = ImageBuf::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                let t = (x * 7 + y * 3) as f32 / 97.0;
                img.set_pixel(x, y, [t * 0.9, (1.0 - t) * 0.7 + 0.05, (t * t) * 1.3]);
            }
        }
        img
    }

    /// ⚠⚠ Every per-pixel stage at once, fused against sequential, sample for sample. A stage
    /// whose `px` drifts from the closure it replaced fails here even if no golden carries it.
    #[test]
    fn a_fused_run_equals_the_sequential_application() {
        use edit_model::{ColorGrading, GradeWheel, Hsl, HslBand, Vignette};
        let cube = std::sync::Arc::new(
            crate::lut::Cube::parse(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0.1\n0 1 0\n1 1 0.2\n0 0.1 1\n1 0 1\n0 1 1\n0.9 1 1\n",
            )
            .expect("a 2^3 cube"),
        );
        let mut hsl = Hsl::default();
        hsl.bands[0] = HslBand {
            hue: 20.0,
            saturation: 30.0,
            luminance: -10.0,
        };
        let grading = ColorGrading {
            shadows: GradeWheel {
                hue: 200.0,
                saturation: 40.0,
                ..Default::default()
            },
            highlights: GradeWheel {
                hue: 40.0,
                saturation: 30.0,
                ..Default::default()
            },
            ..ColorGrading::default()
        };
        let stages: Vec<Box<dyn PipelineStage>> = vec![
            Box::new(WhiteBalanceStage::new([1.1, 0.95, 1.05])),
            Box::new(ExposureStage::new(0.7)),
            Box::new(ContrastStage::new(25.0)),
            Box::new(ToneRegionsStage::new(-20.0, 30.0, 10.0, -15.0)),
            Box::new(
                ToneCurveStage::new(&[[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]]).with_channels(
                    &[[0.0, 0.0], [0.5, 0.45], [1.0, 1.0]],
                    &[],
                    &[[0.0, 0.05], [1.0, 1.0]],
                ),
            ),
            Box::new(SaturationStage::new(20.0, 15.0)),
            Box::new(HslStage::new(&hsl)),
            Box::new(ColorGradingStage::new(&grading)),
            Box::new(VignetteStage::new(&Vignette {
                amount: -40.0,
                ..Vignette::default()
            })),
            Box::new(DisplayRenderingStage::new(None, None)),
            Box::new(CreativeLookStage::new(
                cube,
                edit_model::LutSpace::SrgbDisplay,
                60.0,
            )),
            Box::new(OutputTransform::new(
                color::OutputSpace::Srgb,
                color::RenderingIntent::default(),
            )),
        ];
        for s in &stages {
            assert!(
                s.per_pixel(8, 8).is_some(),
                "{} must be per-pixel for this test to mean anything",
                s.name()
            );
        }
        let img = field();
        let want = stages.iter().fold(img.clone(), |i, s| s.apply(i));
        assert_ne!(
            want.data(),
            img.data(),
            "fixture check: the stages move pixels"
        );
        let got = per_pixel::run_stages_with(stages.iter().map(|b| b.as_ref()), img, true);
        let bad = got
            .data()
            .iter()
            .zip(want.data())
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(bad, None, "the fused run diverged at sample {bad:?}");
    }

    /// Exposure then a curve is not a curve then exposure — asserted first — and the fused walk
    /// of `[exposure, curve]` is the list order, sample for sample.
    #[test]
    fn a_fused_run_of_real_stages_applies_them_in_list_order() {
        let curve = || ToneCurveStage::new(&[[0.0, 0.0], [0.3, 0.9], [1.0, 1.0]]);
        let ab: Vec<Box<dyn PipelineStage>> =
            vec![Box::new(ExposureStage::new(2.0)), Box::new(curve())];
        let ba: Vec<Box<dyn PipelineStage>> =
            vec![Box::new(curve()), Box::new(ExposureStage::new(2.0))];
        let img = field();
        let seq_ab = ab.iter().fold(img.clone(), |i, s| s.apply(i));
        let seq_ba = ba.iter().fold(img.clone(), |i, s| s.apply(i));
        let worst = seq_ab
            .data()
            .iter()
            .zip(seq_ba.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst > 0.05,
            "fixture check: the two orders must differ, worst {worst}"
        );
        let fused = per_pixel::run_stages_with(ab.iter().map(|b| b.as_ref()), img, true);
        assert_eq!(
            fused.data(),
            seq_ab.data(),
            "the fused walk is the list order"
        );
    }

    /// `Pipeline::run` takes one walk for a run of three per-pixel stages.
    #[test]
    fn a_pipeline_of_per_pixel_stages_takes_one_fused_walk() {
        if !per_pixel::fusion_enabled() {
            eprintln!("SKIPPED: RUSTROOM_NO_FUSION is set");
            return;
        }
        let p = Pipeline::new(vec![
            Box::new(ExposureStage::new(0.5)),
            Box::new(ContrastStage::new(10.0)),
            Box::new(SaturationStage::new(10.0, 0.0)),
        ]);
        let before = per_pixel::fused_walks();
        let _ = p.run(ImageBuf::new(8, 8));
        assert_eq!(per_pixel::fused_walks() - before, 1);
    }
}

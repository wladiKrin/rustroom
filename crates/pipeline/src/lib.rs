//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

pub mod brush;
/// A run of per-pixel stages as one pass. See the module.
pub mod per_pixel;
pub use per_pixel::{run_stages, PerPixel};

/// The two pixel measurements a mask's coverage is taken on, for a caller that must agree with it.
///
/// ⚠⚠ **Re-exported by name rather than by opening `stages`**, the rule the hue LUT already
/// follows: `pub mod stages` would make every stage public API for the sake of two functions.
///
/// ⚠ The canvas tint needs exactly these. A second lightness or hue convention there would be
/// invisible per widget and disagree with the render everywhere — the failure `color::luminance`
/// exists to prevent for the pipeline itself.
pub use stages::mask_coverage::{hue_chroma, lightness};

pub mod auto_tone;
pub mod lens_grid;
/// ⚠ A **`.cube` parser and sampler, with no image and no stage** — deliberately separate from
/// `stages::creative_look`, which owns the colour-space shaping. Keeping the table pure means the
/// index-order and interpolation guards can be written against a string literal, with no fixture
/// on disk and no working-space assumptions to get wrong.
pub mod lut;
pub mod parametric;
pub mod perspective;
/// ⚠ Its own module rather than a section of [`spatial`]: that one holds *separable blur*
/// primitives (box, Gaussian, guided) and is already 889 lines. A resampling kernel reads at a
/// fractional coordinate, which is a different operation with a different API.
pub mod resample;
pub mod spatial;
pub(crate) mod spline;
mod stages;
/// M10.11b: grain's parameters, noise and stage — the GPU twin reads the same ones.
pub use stages::grain::{
    grain_params, GrainParams, GrainStage, HASH as GRAIN_HASH, OFFSET as GRAIN_OFFSET,
    SEED as GRAIN_SEED, SEED2 as GRAIN_SEED2,
};
/// ⚠ The one stage-internal module that is public, re-exported rather than opening `stages`
/// wholesale — which would make every stage public API for the sake of one benchmark. `bench_hsl`
/// needs the table and the small-angle pair to sweep table sizes against real pixels.
pub use stages::hue_table;
/// M10.11a: the film negative's per-pixel inversion, for every reader of the undeveloped decode.
pub use stages::negative;

/// The dehaze airlight of an image, as [`stages::dehaze`] estimates it.
///
/// ⚠ Re-exported by name rather than opening `stages`, the rule `hue_table` already follows: a
/// `pub mod stages` would make every stage public API for the sake of one probe.
pub use stages::dehaze::airlight as dehaze_airlight;

pub use resample::Resampler;
pub use stages::color_grading::ColorGradingStage;
pub use stages::contrast::{ContrastStage, PIVOT as CONTRAST_PIVOT};
pub use stages::creative_look::CreativeLookStage;
pub use stages::defringe::DefringeStage;
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
pub use stages::geometry::{GeometryStage, MapParams};
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
pub use stages::scene_offsets::{reads_reference, SceneOffsetsStage};
/// The tone regions' strengths, for a GPU shader to SUBSTITUTE rather than transcribe (M10.12b).
pub use stages::tone_regions::{
    END_STRENGTH as TONE_REGIONS_END_STRENGTH, STRENGTH as TONE_REGIONS_STRENGTH,
};
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
pub use stages::red_eye::{redness, RedEyeStage};
pub use stages::saturation::{SaturationStage, EPS as SATURATION_EPS};
pub use stages::sharpening::{max_kernel_radius, SharpenStage, RADIUS_RANGE};
pub use stages::spot_removal::SpotRemovalStage;
pub use stages::tone_curve::ToneCurveStage;
pub use stages::tone_regions::ToneRegionsStage;
pub use stages::vignette::{vignette_params, VignetteParams, VignetteStage};
pub use stages::white_balance::WhiteBalanceStage;

mod adjustments;
mod build;
mod coverage;
mod pipeline_stage;
mod prefix;
mod prefix_airlight;
mod region;
mod tail_rendering;
#[cfg(test)]
mod test_fixtures;
pub(crate) use adjustments::{adjustment_stages, capture_stages};
pub use adjustments::{
    global_wb_gains, look_only, mask_wb_shift, scene_only, shifted_white_balance,
    white_balance_gains, MIRED_PER_STEP,
};
pub use build::{
    build_pipeline, build_pipeline_rendered, build_pipeline_spec, build_pipeline_split,
    build_pipeline_split_in_region, build_pipeline_split_in_region_with_brush,
    build_pipeline_split_tapped, build_pipeline_to, build_pipeline_with, tail_kernel_radius,
    RenderSpec, TailHalves,
};
pub use coverage::{BrushCoverage, CoverageRasterizer};
pub use pipeline_stage::{Pipeline, PipelineStage, RenderScale, SubStageAccelerator};
pub use prefix::{build_prefix_steps, prefix_output_dims, PrefixKey, PrefixStep, PrefixStepKey};
pub use prefix_airlight::{prefix_airlight, prefix_airlight_key, AirlightKey};
pub use region::{region_plan, run_region_plan, Rect, Region, RegionPlan, RegionSrc};
pub use tail_rendering::{effective_profile_curve, TailRendering};

#[cfg(test)]
mod per_pixel_mask_tests {
    use super::*;

    /// The free part of fusion inside a mask: three per-pixel sub-stages are one walk over the
    /// copy.
    #[test]
    fn a_masks_per_pixel_sub_stages_are_one_fused_walk() {
        if !per_pixel::fusion_enabled() {
            eprintln!("SKIPPED: RUSTROOM_NO_FUSION is set");
            return;
        }
        let masked = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                exposure: 0.6,
                contrast: 20.0,
                saturation: 25.0,
                ..Default::default()
            },
            disabled: false,
            invert: false,
        };
        let stage = MaskStage::new(&masked, RenderScale::FULL, &edit_model::Geometry::default());
        let mut img = core_image::ImageBuf::new(32, 24);
        for y in 0..24 {
            for x in 0..32 {
                let t = (x * 5 + y * 3) as f32 / 61.0;
                img.set_pixel(x, y, [t, 0.5 * t + 0.1, 1.0 - t]);
            }
        }
        let before = per_pixel::fused_walks();
        let out = stage.apply(img.clone());
        assert_ne!(
            out.data(),
            img.data(),
            "fixture check: the mask moves pixels"
        );
        assert_eq!(
            per_pixel::fused_walks() - before,
            1,
            "three per-pixel sub-stages are one walk over the copy"
        );
    }
}

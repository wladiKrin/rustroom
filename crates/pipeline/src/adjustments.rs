//! The adjustment stages: one list for the global tail and every mask, and the capture corrections.

use crate::{
    ColorGradingStage, ContrastStage, DehazeStage, ExposureStage, HslStage, LocalContrastStage,
    NoiseReductionStage, PipelineStage, RenderScale, SaturationStage, SharpenStage, ToneCurveStage,
    ToneRegionsStage, WhiteBalanceStage,
};

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
pub(crate) fn adjustment_stages(
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
    // ⚠ A mask's curve stays **composite only, deliberately**: masks have no per-channel curves by
    // decision (Lightroom's have no curve at all), and a mask that wants a colour shift has its own
    // grading wheel. The asymmetry with the global curve above is a choice, not an oversight.
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
/// a different half of the pipeline.** Noise reduction and dehaze run in the prefix, before every
/// tonal adjustment, as the global stages of the same name do.
///
/// ⚠⚠ **A recorded asymmetry since M7.3b, not an oversight.** The global pair moved AHEAD of the
/// geometry resample (for drag speed; see `build_prefix_steps`). These masked twins stay AFTER it,
/// because a mask's coverage is rasterised in frame space (`MaskStage::new_capture` takes
/// `&edits.geometry`). Moving them would mean mapping every mask kind back through the homography
/// and the rotation. So on a straightened photo a masked Luminance 50 acts on noise the rotation
/// has resampled while a global one does not. M7.3b measured the global version of that
/// difference at 1-2% of residual grain.
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
pub(crate) fn capture_stages(
    a: &edit_model::LocalAdjustments,
    scale: RenderScale,
    airlight: Option<[f32; 3]>,
) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.noise_reduction.is_active() {
        out.push(Box::new(NoiseReductionStage::new(
            &a.noise_reduction,
            scale,
        )));
    }
    if a.dehaze.is_active() {
        // ⚠⚠ **The same frame-global value the global dehaze gets.** Without one this called
        // `DehazeStage::new`, which estimates from the stage's own input on every frame — measured
        // at 9.19-11.13 levels across a Straighten drag, WORSE than the global path beside it.
        out.push(match airlight {
            Some(air) => Box::new(DehazeStage::with_airlight(&a.dehaze, scale, air)),
            None => Box::new(DehazeStage::new(&a.dehaze, scale)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::all_edits;
    use crate::{build_pipeline, build_pipeline_split, TailRendering};
    use core_image::ImageBuf;
    use edit_model::Edits;

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
        let full = build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
            .run(img.clone());
        let quarter = build_pipeline(
            &edits,
            RenderScale::new(0.25),
            TailRendering::colorimetric(),
        )
        .run(img);
        assert_eq!(full.data(), quarter.data());
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
        let names =
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names();
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
            disabled: false,
            invert: false,
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
        let (prefix, tail) =
            build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
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
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        };
        let (prefix, _) =
            build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
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
        let names: Vec<&'static str> = capture_stages(&a, RenderScale::FULL, None)
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
        let cap: Vec<&'static str> = capture_stages(&a, RenderScale::FULL, None)
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
}

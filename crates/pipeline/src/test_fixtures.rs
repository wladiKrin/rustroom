//! Fixtures the pipeline tests share across modules.

use edit_model::Edits;

/// An `Edits` with every field non-neutral — so a stage that has drifted into the wrong
/// half of the split shows up.
///
/// Every field is listed explicitly rather than using `..Default::default()`, on purpose:
/// adding a field to `Edits` then breaks this function's compilation, forcing whoever
/// adds it to decide which half of the split it belongs to.
pub(crate) fn all_edits() -> Edits {
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
        // ⚠⚠ **Three DIFFERENT curves.** Three copies of one would let every "this group
        // transfers" test pass against code that ignored the channel index — the fixture trap paid
        // for on 2026-09-23.
        tone_curve_red: edit_model::ToneCurve {
            points: vec![[0.0, 0.15], [1.0, 1.0]],
        },
        tone_curve_green: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
        },
        tone_curve_blue: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [1.0, 0.85]],
        },
        tone_curve_parametric: edit_model::ParametricCurve {
            shadows: 20.0,
            darks: -10.0,
            lights: 15.0,
            highlights: -25.0,
            splits: [0.2, 0.45, 0.8],
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
        defringe: edit_model::Defringe {
            purple_amount: 30.0,
            ..Default::default()
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
            disabled: false,
            invert: false,
        }],
        spots: vec![edit_model::Spot {
            centre: [0.3, 0.4],
            source: [0.5, 0.4],
            radius: 0.05,
            feather: 30.0,
            mode: edit_model::SpotMode::Heal,
        }],
        eyes: vec![edit_model::Eye {
            centre: [0.6, 0.35],
            radius: 0.01,
            kind: edit_model::EyeKind::Pet,
            pupil_size: 60.0,
            darken: 40.0,
            catchlight: Some([-0.35, -0.35]),
        }],
        rendering: Some(edit_model::Rendering::Camera),
        look: edit_model::Look {
            file: Some("velvia.cube".into()),
            space: edit_model::LutSpace::SrgbDisplay,
            strength: 65.0,
        },
        grain: edit_model::Grain {
            amount: 30.0,
            size: 40.0,
            roughness: 70.0,
        },
        negative: edit_model::Negative {
            on: true,
            base: [0.7, 0.45, 0.25],
            gamma: 0.7,
            low: [0.05, 0.06, 0.07],
            high: [1.8, 1.9, 2.1],
            mono: false,
        },
    }
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

/// A chart with structure everywhere, so a wrongly-centred rotation cannot coincide.
pub(crate) fn structured(w: usize, h: usize) -> core_image::ImageBuf {
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

//! Golden-image regression tests. Set `UPDATE_GOLDEN=1` to (re)write references.
//!
//! A fixed synthetic "chart" is rendered through `build_pipeline(&edits)`; the RGB8
//! output must match the committed PNG within `TOL` per channel.

use core_image::ImageBuf;
use edit_model::{
    Dehaze, Edits, LocalContrast, NoiseReduction, Sharpening, ToneCurve, Vignette, WhiteBalance,
};
use pipeline::{build_pipeline, RenderScale};
use std::path::PathBuf;

const W: usize = 64;
const H: usize = 16;
const TOL: i32 = 2; // per-channel tolerance (float pow differs slightly across platforms)

/// Deterministic linear test chart: 4 horizontal bands (gray, R, G, B), each a 0..1 ramp.
fn chart() -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for y in 0..H {
        let band = y * 4 / H; // 0..3
        for x in 0..W {
            let v = x as f32 / (W - 1) as f32;
            let px = match band {
                0 => [v, v, v],
                1 => [v, 0.0, 0.0],
                2 => [0.0, v, 0.0],
                _ => [0.0, 0.0, v],
            };
            img.set_pixel(x, y, px);
        }
    }
    img
}

/// Deterministic linear chart of *muted* colors: 4 bands, each a 0..1 ramp with a weak
/// color cast (chroma/max = 0.2), so vibrance actually does something. The main `chart()`
/// is all pure primaries (chroma 1) and neutral gray (chroma 0), where vibrance is a no-op.
fn muted_chart() -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for y in 0..H {
        let band = y * 4 / H; // 0..3
        let tint = match band {
            0 => [1.0, 0.9, 0.8],  // warm
            1 => [0.8, 1.0, 0.9],  // green
            2 => [0.8, 0.9, 1.0],  // cool
            _ => [1.0, 0.8, 0.95], // magenta
        };
        for x in 0..W {
            let v = x as f32 / (W - 1) as f32;
            img.set_pixel(x, y, [v * tint[0], v * tint[1], v * tint[2]]);
        }
    }
    img
}

/// Minimal HSV → sRGB (all components 0..1), used to build the hue-sweep chart.
fn hsv_to_srgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// Deterministic linear chart sweeping the full hue circle across x at fixed saturation and
/// value — every band gets exercised, which the primaries-and-gray charts cannot do.
fn hue_sweep_chart() -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let hue = x as f32 / W as f32 * 360.0;
            let srgb = hsv_to_srgb(hue, 0.9, 0.8);
            img.set_pixel(
                x,
                y,
                [
                    color::srgb_to_linear(srgb[0]),
                    color::srgb_to_linear(srgb[1]),
                    color::srgb_to_linear(srgb[2]),
                ],
            );
        }
    }
    img
}

/// Deterministic noisy chart for the noise-reduction cases: three stacked regions — flat
/// gray, flat color, and a hard black/white vertical edge — all under per-channel
/// xorshift noise. One reference image therefore covers smoothing, chroma handling and
/// edge preservation at once.
///
/// Much larger than the other charts, and deliberately so. Each region is 64 px tall
/// because the chroma blur's window reaches 41 px across at Smoothness 50 (65 px at 100):
/// on shorter bands the reference would mostly record chroma bleeding between regions
/// rather than noise being removed inside one. Bleed across a strong color edge is real
/// and documented, but it is not what these three cases exist to pin down.
fn noise_chart() -> ImageBuf {
    const NW: usize = 128;
    const NH: usize = 192;
    let mut state: u32 = 0x9E37_79B9;
    let mut noise = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        ((state as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.03
    };
    let mut img = ImageBuf::new(NW, NH);
    for y in 0..NH {
        for x in 0..NW {
            let base = if y < NH / 3 {
                [0.35, 0.35, 0.35]
            } else if y < 2 * NH / 3 {
                [0.35, 0.18, 0.12]
            } else if x < NW / 2 {
                [0.05, 0.05, 0.05]
            } else {
                [0.75, 0.75, 0.75]
            };
            img.set_pixel(
                x,
                y,
                [
                    (base[0] + noise()).max(0.0),
                    (base[1] + noise()).max(0.0),
                    (base[2] + noise()).max(0.0),
                ],
            );
        }
    }
    img
}

/// Deterministic chart for the local-contrast cases: a coarse horizontal ramp (what
/// Clarity's 40 px radius sees), a fine 6 px-period sine on top of it (what Texture's 2 px
/// radius sees), and a hard horizontal edge three quarters of the way down (where a halo
/// would appear). Tinted rather than neutral, so the "hue ratios are preserved" property is
/// visible in the reference rather than only in a unit test.
///
/// 256x256, far larger than the other charts and deliberately so: Clarity's kernel reaches
/// 120 px, so on a 64- or 128-px chart the reference would record the global mean rather
/// than local contrast.
fn local_contrast_chart() -> ImageBuf {
    const S: usize = 256;
    let mut img = ImageBuf::new(S, S);
    for y in 0..S {
        for x in 0..S {
            let coarse = 0.15 + 0.5 * (x as f32 / S as f32);
            let fine = 0.03 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
            let step = if y >= 3 * S / 4 { 0.25 } else { 0.0 };
            let v = (coarse + fine + step).clamp(0.0, 1.0);
            img.set_pixel(x, y, [v, v * 0.85, v * 0.7]);
        }
    }
    img
}

/// A hazy chart, built by **applying** the haze model to a synthetic scene rather than by
/// hand-painting something that merely looks foggy — so the reference encodes a known correct
/// answer. Depth increases to the right, so transmission falls from 0.95 to 0.05: the far
/// edge is essentially pure airlight, as a real horizon is.
///
/// Periodic dark cells on a 40 px pitch, because the dark-channel prior assumes every patch
/// contains a near-black sample. 256x192 is large enough for the 20 px patch and the 60 px
/// refinement radius to behave as they will on a real image.
fn hazy_chart() -> ImageBuf {
    const S: usize = 256;
    const T: usize = 192;
    let a = [0.80f32, 0.82, 0.88];
    let mut img = ImageBuf::new(S, T);
    for y in 0..T {
        for x in 0..S {
            let u = x as f32 / S as f32;
            let v = y as f32 / T as f32;
            let clear = if x % 40 < 6 && y % 40 < 6 {
                [0.01, 0.01, 0.01]
            } else {
                [
                    0.05 + 0.70 * u * v,
                    0.05 + 0.60 * v,
                    0.05 + 0.50 * (1.0 - u),
                ]
            };
            let t = 0.95 - 0.9 * u;
            img.set_pixel(
                x,
                y,
                [
                    clear[0] * t + a[0] * (1.0 - t),
                    clear[1] * t + a[1] * (1.0 - t),
                    clear[2] * t + a[2] * (1.0 - t),
                ],
            );
        }
    }
    img
}

fn to_rgb8(img: &ImageBuf) -> Vec<u8> {
    img.data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

fn check_golden(name: &str, edits: &Edits) {
    check_golden_with(name, edits, chart());
}

fn check_golden_with(name: &str, edits: &Edits, input: ImageBuf) {
    let rendered = build_pipeline(edits, RenderScale::FULL).run(input);
    let (rw, rh) = (rendered.width() as u32, rendered.height() as u32);
    let bytes = to_rgb8(&rendered);
    let path = golden_path(name);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &bytes, rw, rh, image::ColorType::Rgb8).unwrap();
        return;
    }

    let golden = image::open(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with UPDATE_GOLDEN=1"))
        .to_rgb8();
    assert_eq!(golden.dimensions(), (rw, rh), "{name}: size");
    let gbytes = golden.into_raw();
    let max_diff = bytes
        .iter()
        .zip(&gbytes)
        .map(|(&a, &b)| (a as i32 - b as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        max_diff <= TOL,
        "{name}: max per-channel diff {max_diff} > {TOL}"
    );
}

#[test]
fn golden_neutral() {
    check_golden("neutral", &Edits::default());
}

#[test]
fn golden_exposure_plus1() {
    check_golden(
        "exposure_plus1",
        &Edits {
            exposure: 1.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_warm_wb() {
    check_golden(
        "warm_wb",
        &Edits {
            white_balance: WhiteBalance {
                temp: 60.0,
                tint: 0.0,
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_contrast_plus50() {
    check_golden(
        "contrast_plus50",
        &Edits {
            contrast: 50.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_tone_regions() {
    check_golden(
        "tone_regions",
        &Edits {
            shadows: 60.0,
            highlights: -40.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_scurve() {
    check_golden(
        "scurve",
        &Edits {
            tone_curve: ToneCurve {
                points: vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]],
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_combined() {
    check_golden(
        "combined",
        &Edits {
            white_balance: WhiteBalance {
                temp: 30.0,
                tint: -10.0,
            },
            exposure: 0.5,
            contrast: 25.0,
            shadows: 40.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_crop_center() {
    use edit_model::{CropRect, Geometry};
    check_golden(
        "crop_center",
        &Edits {
            geometry: Geometry {
                orientation: Default::default(),
                angle: 0.0,
                crop: CropRect {
                    left: 0.25,
                    top: 0.0,
                    right: 0.75,
                    bottom: 1.0,
                },
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_orient_cw90() {
    use edit_model::{Geometry, Orientation, Quarter};
    check_golden(
        "orient_cw90",
        &Edits {
            geometry: Geometry {
                orientation: Orientation {
                    rotate: Quarter::Cw90,
                    mirror: false,
                },
                ..Default::default()
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_orient_mirror() {
    use edit_model::{Geometry, Orientation, Quarter};
    check_golden(
        "orient_mirror",
        &Edits {
            geometry: Geometry {
                orientation: Orientation {
                    rotate: Quarter::Cw180,
                    mirror: true,
                },
                ..Default::default()
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_saturation_plus50() {
    check_golden(
        "saturation_plus50",
        &Edits {
            saturation: 50.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_vibrance_plus50() {
    check_golden_with(
        "vibrance_plus50",
        &Edits {
            vibrance: 50.0,
            ..Edits::default()
        },
        muted_chart(),
    )
}

#[test]
fn golden_straighten_5deg() {
    use edit_model::Geometry;
    check_golden(
        "straighten_5deg",
        &Edits {
            geometry: Geometry {
                angle: 5.0,
                ..Default::default()
            },
            ..Edits::default()
        },
    )
}

#[test]
fn golden_hsl_red_desaturated() {
    let mut hsl = edit_model::Hsl::default();
    hsl.bands[0].saturation = -100.0; // Red
    check_golden_with(
        "hsl_red_desaturated",
        &Edits {
            hsl,
            ..Edits::default()
        },
        hue_sweep_chart(),
    )
}

#[test]
fn golden_hsl_blue_hue_shift() {
    let mut hsl = edit_model::Hsl::default();
    hsl.bands[5].hue = 100.0; // Blue, +30°
    check_golden_with(
        "hsl_blue_hue_shift",
        &Edits {
            hsl,
            ..Edits::default()
        },
        hue_sweep_chart(),
    )
}

#[test]
fn golden_hsl_green_luminance() {
    let mut hsl = edit_model::Hsl::default();
    hsl.bands[3].luminance = 60.0; // Green
    check_golden_with(
        "hsl_green_luminance",
        &Edits {
            hsl,
            ..Edits::default()
        },
        hue_sweep_chart(),
    )
}

#[test]
fn golden_nr_luminance() {
    check_golden_with(
        "nr_luminance",
        &Edits {
            noise_reduction: NoiseReduction {
                luminance: 80.0,
                detail: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_nr_color() {
    check_golden_with(
        "nr_color",
        &Edits {
            noise_reduction: NoiseReduction {
                color: 80.0,
                smoothness: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_nr_both() {
    check_golden_with(
        "nr_both",
        &Edits {
            noise_reduction: NoiseReduction {
                luminance: 60.0,
                detail: 40.0,
                color: 80.0,
                smoothness: 60.0,
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

/// Amount is the same in all three cases so the references differ only by shape — which is
/// the thing worth regression-testing here.
fn vignette(roundness: f32) -> Edits {
    Edits {
        vignette: Vignette {
            amount: -60.0,
            roundness,
            ..Default::default()
        },
        ..Edits::default()
    }
}

#[test]
fn golden_vignette_darken() {
    check_golden("vignette_darken", &vignette(0.0));
}

#[test]
fn golden_vignette_circle() {
    check_golden("vignette_circle", &vignette(100.0));
}

#[test]
fn golden_vignette_square() {
    check_golden("vignette_square", &vignette(-100.0));
}

/// Sharpening cases run on `noise_chart()`, whose hard edge *and* noise are exactly what
/// sharpening interacts with — one chart covers halos, texture and the masking gate.
/// Amount and Radius are identical in all three, so the references differ only by the
/// control under test.
fn sharpen(detail: f32, masking: f32) -> Edits {
    Edits {
        sharpening: Sharpening {
            amount: 70.0,
            radius: 1.0,
            detail,
            masking,
        },
        ..Edits::default()
    }
}

#[test]
fn golden_sharpen_basic() {
    check_golden_with("sharpen_basic", &sharpen(50.0, 0.0), noise_chart());
}

#[test]
fn golden_sharpen_masked() {
    // Masking 80 should visibly spare the flat noisy bands while the edge still sharpens.
    check_golden_with("sharpen_masked", &sharpen(50.0, 80.0), noise_chart());
}

#[test]
fn golden_sharpen_detail_low() {
    check_golden_with("sharpen_detail_low", &sharpen(0.0, 0.0), noise_chart());
}

#[test]
fn golden_texture_plus() {
    check_golden_with(
        "texture_plus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 80.0,
                clarity: 0.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_texture_minus() {
    check_golden_with(
        "texture_minus",
        &Edits {
            local_contrast: LocalContrast {
                texture: -80.0,
                clarity: 0.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_clarity_plus() {
    check_golden_with(
        "clarity_plus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 0.0,
                clarity: 60.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_clarity_minus() {
    check_golden_with(
        "clarity_minus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 0.0,
                clarity: -60.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_dehaze_plus() {
    check_golden_with(
        "dehaze_plus",
        &Edits {
            dehaze: Dehaze { amount: 80.0 },
            ..Edits::default()
        },
        hazy_chart(),
    )
}

#[test]
fn golden_dehaze_minus() {
    check_golden_with(
        "dehaze_minus",
        &Edits {
            dehaze: Dehaze { amount: -80.0 },
            ..Edits::default()
        },
        hazy_chart(),
    )
}

#[test]
fn golden_mask_linear_exposure() {
    // A graduated exposure lift across the chart: the left edge untouched, the right at +1 EV.
    check_golden(
        "mask_linear_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_linear_inverted_saturation() {
    // A different payload, inverted and diagonal, so the reference covers more than one stage
    // inside the mask and the `invert` flag reaches a real image.
    check_golden(
        "mask_linear_inverted_saturation",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.2, 0.0],
                    to: [0.8, 1.0],
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_radial_exposure() {
    // An elliptical exposure lift in the middle of the 64x16 chart: full strength at the
    // centre, falling to nothing at the boundary. The radii are fractions of their own axis,
    // so on this 4:1 chart the ellipse is 45 px wide and 14 px tall — wide enough to sample
    // the ramp across the bands' 0..1 gradient.
    //
    // ⚠ **+0.55 EV, not +1.0, and the reason is discriminating power.** At +1.0 EV six of
    // twenty samples across the green band pinned at 255, so the reference could not tell the
    // ellipse's peak coverage from any larger value — the same weakness 3.1a recorded for its
    // linear exposure golden, whose right quarter clips. At +0.55 nothing clips, so every
    // sample inside the ellipse encodes the smoothstep profile and a change to the feather or
    // the radii moves it. Verified by measurement, not by eye.
    check_golden(
        "mask_radial_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.5, 0.5],
                    radius: [0.35, 0.45],
                    feather: 50.0,
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_radial_inverted_saturation() {
    // The subject vignette: everything *except* the ellipse is desaturated and darkened. This
    // is the case that `invert` exists for, and the one that had no UI until M3.1b.
    //
    // Off-centre and with a tighter feather, so the reference covers a different part of the
    // parameter space from the case above.
    check_golden(
        "mask_radial_inverted_saturation",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.3, 0.5],
                    radius: [0.25, 0.6],
                    feather: 25.0,
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_brush_exposure() {
    // A soft horizontal stroke over the LEFT half of the 64x16 chart.
    //
    // ⚠ **The left half, and that is measured rather than aesthetic.** Each band of the chart is
    // a 0..1 ramp, so the right end already sits near 253 and *any* positive lift clips there. A
    // stroke across the full width put 15 of 64 samples at 255, which is the weakness M3.1b's
    // radial golden had to be regenerated for: a reference that pins cannot discriminate the
    // stroke's peak coverage from any greater value. Confined to the darker half there is
    // headroom for the whole ramp.
    check_golden(
        "mask_brush_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.1, 0.5], [0.45, 0.5]],
                        radius: 0.1,
                        hardness: 40.0,
                        erase: false,
                    }],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_brush_erase() {
    // A broad stroke with a narrower eraser through it: the reference shows a treated band with a
    // clear channel down its middle, which is the ordering rule made visible.
    check_golden(
        "mask_brush_erase",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![
                        edit_model::BrushStroke {
                            points: vec![[0.1, 0.5], [0.9, 0.5]],
                            radius: 0.2,
                            hardness: 60.0,
                            erase: false,
                        },
                        edit_model::BrushStroke {
                            points: vec![[0.3, 0.5], [0.7, 0.5]],
                            radius: 0.08,
                            hardness: 100.0,
                            erase: true,
                        },
                    ],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

//! Golden-image regression tests. Set `UPDATE_GOLDEN=1` to (re)write references.
//!
//! A fixed synthetic "chart" is rendered through `build_pipeline(&edits, TailRendering::colorimetric())`; the RGB8
//! output must match the committed PNG within `TOL` per channel.

use core_image::ImageBuf;
use edit_model::{
    ColorGrading, Dehaze, Edits, GradeWheel, LocalContrast, NoiseReduction, Sharpening, ToneCurve,
    Vignette, WhiteBalance,
};
use pipeline::TailRendering;
use pipeline::{build_pipeline, RenderScale, Resampler};

/// What every golden renders with — the shipping kernel.
///
/// ⚠ One constant rather than `Resampler::default()` at each site, so the day the shipping
/// kernel changes again the references move by editing **one line** — and the diff says which
/// commit did it. That is exactly how they moved when Catmull-Rom replaced bilinear: only the
/// six references that actually resample changed, which is what proved the change was scoped.
const RESAMPLER: Resampler = Resampler::CatmullRom;
use std::path::PathBuf;

const W: usize = 64;
const H: usize = 16;
const TOL: i32 = 2; // per-channel tolerance (float pow differs slightly across platforms)

/// Write an **sRGB** colour into a working-space chart.
///
/// ⚠ The charts are authored in sRGB and converted here, rather than having their literals
/// reinterpreted as ProPhoto. Two reasons, and the second is the important one.
///
/// The literals mean something: `[1.0, 0.0, 0.0]` is written to say "pure red", and read as
/// ProPhoto it silently becomes a far more saturated colour than any camera produces — the chart
/// would stop resembling the thing it was built to exercise.
///
/// ⚠ And it restores the check this change otherwise destroys. All 38 references move at once, so
/// the usual proof — *"the others stayed byte-identical, therefore the change was scoped"* — is
/// unavailable. Authoring in sRGB means **the `neutral` golden must come back within `TOL`**,
/// because sRGB → ProPhoto → sRGB is a round trip. That single property is the strongest evidence
/// available that the colour maths is right.
fn set_srgb(img: &mut ImageBuf, x: usize, y: usize, srgb: [f32; 3]) {
    img.set_pixel(x, y, color::srgb_to_prophoto(srgb));
}

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
            set_srgb(&mut img, x, y, px);
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
            set_srgb(&mut img, x, y, [v * tint[0], v * tint[1], v * tint[2]]);
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
            set_srgb(
                &mut img,
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
            set_srgb(
                &mut img,
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
            set_srgb(&mut img, x, y, [v, v * 0.85, v * 0.7]);
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
            set_srgb(
                &mut img,
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

/// A lightness ramp for the colour-grading goldens: a neutral ramp above a muted warm one, so a
/// grade can be seen acting on both neutral and chromatic pixels.
///
/// ⚠ The ramp runs **0.02..0.68, not 0..1**, and the top was chosen by *measuring*, not by
/// eye. Adding chroma near white clips, and a clipped golden cannot tell one tint from another
/// — the failure the exposure golden recorded, where 15 of 64 samples pinned at 255. At a top
/// of 0.85 `grade_split_tone` pinned 144 samples; at 0.74 `grade_balance_high` still pinned 16.
/// At 0.68 the brightest sample across all four cases is 243, leaving real headroom.
/// `the_grading_goldens_do_not_clip` is the guard — if it fails, lower this, do not regenerate
/// the references.
fn grade_ramp_chart() -> ImageBuf {
    const GW: usize = 64;
    const GH: usize = 16;
    let mut img = ImageBuf::new(GW, GH);
    for y in 0..GH {
        for x in 0..GW {
            let v = 0.02 + (x as f32 / (GW - 1) as f32) * 0.66;
            let px = if y < GH / 2 {
                [v, v, v]
            } else {
                [v, v * 0.82, v * 0.66]
            };
            set_srgb(&mut img, x, y, px);
        }
    }
    img
}

/// A blue-shadow / warm-highlight split tone, the canonical grade.
fn split_tone() -> ColorGrading {
    ColorGrading {
        shadows: GradeWheel {
            hue: 240.0,
            saturation: 40.0,
            luminance: 0.0,
        },
        highlights: GradeWheel {
            hue: 45.0,
            saturation: 40.0,
            luminance: 0.0,
        },
        ..ColorGrading::default()
    }
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
    // ⚠⚠ **The goldens must render through the airlight definition that SHIPS**, which since M8.7
    // is a 256 px replay of the prefix rather than an estimate from dehaze's own input. A harness
    // left on `RenderSpec::default()` would pin a definition nothing uses and go quietly blind to
    // the one that does — the W5 failure mode, in the one place this project relies on most.
    //
    // ⚠ It costs a 65k-pixel run per golden, and it is what makes `dehaze_plus`, `dehaze_minus`
    // and `mask_dehaze` the only three references that move.
    let spec = pipeline::RenderSpec {
        airlight: Some(pipeline::prefix_airlight(
            edits,
            &input,
            pipeline::Resampler::default(),
            None,
        )),
        ..Default::default()
    };
    compare_or_update(
        name,
        pipeline::build_pipeline_spec(
            edits,
            RenderScale::FULL,
            spec,
            pipeline::TailRendering::colorimetric(),
        )
        .run(input),
    );
}

/// As [`check_golden`], rendering to a non-sRGB output space.
///
/// ⚠ The non-default output path has no reference otherwise: every other golden goes through
/// sRGB, so a mistake in the P3 matrix or in the space parameter's plumbing would be invisible.
fn check_golden_in_space(name: &str, edits: &Edits, space: color::OutputSpace) {
    compare_or_update(
        name,
        pipeline::build_pipeline_to(
            edits,
            RenderScale::FULL,
            space,
            RESAMPLER,
            pipeline::TailRendering::colorimetric(),
        )
        .run(chart()),
    );
}

fn compare_or_update(name: &str, rendered: ImageBuf) {
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

/// ⚠⚠ **Re-authored, not regenerated, when white balance became Kelvin.** Its `temp: 60` had no
/// Kelvin equivalent — the old model's coefficients were hand-chosen and traced a different
/// surface in gain space — so regenerating would have baked whatever the new code happens to do
/// into the reference.
///
/// ⚠ 5200 K against the 6504 K baseline, and the value was **measured rather than eyeballed**: it
/// newly pins exactly 116 of 3072 samples, the same as the reference it replaces, while moving
/// 2412 samples where the old one moved 2008. A warmer choice reads more obviously as warm and
/// buys it by clipping — 3200 K pins 360 — and a clipped sample cannot tell this effect from a
/// stronger one, which is the whole discriminating power of a golden.
#[test]
fn golden_warm_wb() {
    check_golden(
        "warm_wb",
        &Edits {
            white_balance: WhiteBalance {
                kelvin: Some(5200.0),
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

/// ⚠⚠ **Pixel evidence for the per-channel curves**, which unit tests and CPU/GPU parity cannot give:
/// those prove the stage composes and that the two backends agree, not that the *render* is what was
/// intended.
///
/// ⚠ **Three different curves, one per channel**, and the composite left neutral — so this golden
/// isolates the channel curves from the composite entirely. Three copies of one curve would have
/// produced a reference a channel-blind implementation also passes.
///
/// ⚠⚠ **Measured against `neutral`, so the reference is known not to be accidentally grey:** the grey
/// band reads `(159, 116, 100)` at the dark end where `neutral` is a flat `(111, 111, 111)` — a
/// 59-level warm spread — narrowing to `(243, 241, 218)`, a 25-level spread, at the bright end. Peak
/// difference from `neutral` is **135 levels**. That narrowing *is* the crossover, and it is what the
/// grading wheels cannot place this precisely.
#[test]
fn golden_rgb_channel_curves() {
    check_golden(
        "rgb_channel_curves",
        &Edits {
            tone_curve_red: ToneCurve {
                points: vec![[0.0, 0.12], [1.0, 1.0]],
            },
            tone_curve_green: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.56], [1.0, 1.0]],
            },
            tone_curve_blue: ToneCurve {
                points: vec![[0.0, 0.0], [1.0, 0.86]],
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
            // ⚠ Re-authored with `warm_wb`, and swept the same way: at 3600 K this newly pins
            // 696 samples against the previous reference's 620, which is as close as the sweep
            // gets — the clipping here is driven by the exposure and contrast beside it rather
            // than by the white balance, and barely moves across the whole Kelvin range.
            white_balance: WhiteBalance {
                kelvin: Some(3600.0),
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

/// M6.4c: the PERCEPTUAL intent on the same saturated chart as `saturation_plus50`, which renders
/// through the default, `Clip`. Every other golden goes through the default intent, so
/// without this one a regression in perceptual's knee would be invisible. ⚠ The test asserts the
/// two references differ, so the pair cannot silently collapse into one intent.
#[test]
fn golden_gamut_perceptual() {
    let edits = Edits {
        saturation: 50.0,
        ..Edits::default()
    };
    let render = |intent| {
        pipeline::build_pipeline_spec(
            &edits,
            RenderScale::FULL,
            pipeline::RenderSpec {
                intent,
                ..Default::default()
            },
            pipeline::TailRendering::colorimetric(),
        )
        .run(chart())
    };
    let per = render(color::RenderingIntent::Perceptual);
    assert_ne!(
        to_rgb8(&per),
        to_rgb8(&render(color::RenderingIntent::RelativeColorimetric)),
        "the fixture must tell the intents apart"
    );
    compare_or_update("gamut_perceptual", per);
}

/// M9.6: a purple and a green fringe beside hard edges, defringed. Every other golden has no
/// defringe, so a regression in the edge map, the hue ranges or the chroma scale would otherwise be
/// invisible. ⚠ Asserted to differ from the undefringed chart first.
#[test]
fn golden_defringe() {
    let mut img = ImageBuf::new(W, H);
    let purple = color::srgb_to_prophoto([0.35, 0.12, 0.55]);
    let green = color::srgb_to_prophoto([0.15, 0.5, 0.2]);
    for y in 0..H {
        for x in 0..W {
            let band = x * 6 / W;
            let c = match band {
                0 | 3 => [0.02; 3],
                1 => purple,
                4 => green,
                _ => [0.6; 3],
            };
            // Hard edges with the fringe only a few pixels wide beside the dark band.
            let near = x % (W / 6) < 3;
            let c = if band == 1 || band == 4 {
                if near {
                    c
                } else {
                    [0.6; 3]
                }
            } else {
                c
            };
            img.set_pixel(x, y, c);
        }
    }
    let edits = Edits {
        defringe: edit_model::Defringe {
            purple_amount: 80.0,
            green_amount: 80.0,
            ..Default::default()
        },
        ..Edits::default()
    };
    let render = |e: &Edits| {
        pipeline::build_pipeline_spec(
            e,
            RenderScale::FULL,
            pipeline::RenderSpec::default(),
            pipeline::TailRendering::colorimetric(),
        )
        .run(img.clone())
    };
    let out = render(&edits);
    assert_ne!(
        to_rgb8(&out),
        to_rgb8(&render(&Edits::default())),
        "the fixture's fringes must be touched"
    );
    compare_or_update("defringe", out);
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
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

/// One composite: a left-to-right gradient carrying a measured exposure lift, with a radial
/// composited onto it by `op`.
///
/// ⚠⚠ **+0.45 EV, SWEPT rather than chosen**, and the sweep is the point: a clipped reference
/// cannot tell full coverage from any larger value, and clipping is invisible when you look at the
/// PNG. Measured over the three ops, with the chart's own 24 inherently-clipped samples as the
/// floor:
///
/// | EV | Add | Subtract | Intersect |
/// |---|---|---|---|
/// | 0.55 | **64** | **48** | 24 |
/// | **0.45** | **24** | **24** | **24** |
/// | 0.35 | 24 | 24 | 24 |
///
/// 0.45 is the largest lift that adds **no** clipping of its own, so it is the strongest effect
/// still fully discriminating. ⚠ `Add` is the binding case, because it *widens* coverage.
///
/// ⚠ The gradient runs right-to-left so full coverage lands on the ramp's **dark** end. Left to
/// right put the strongest lift where the chart is already near 1.0 and clipped 434 samples.
fn composite_case(op: edit_model::MaskOp) -> Edits {
    Edits {
        masks: vec![edit_model::MaskedAdjustment {
            // ⚠ Right-to-left, so full coverage lands on the ramp's **dark** end. Left-to-right
            // puts the strongest lift exactly where the chart is already near 1.0, and `Add`
            // widens coverage — measured, that clipped 434 samples at +0.55 EV before a single
            // reference was written.
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [1.0, 0.0],
                to: [0.0, 0.0],
                invert: false,
            }),
            refine: vec![edit_model::Refinement {
                op,
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.5, 0.5],
                    radius: [0.3, 0.5],
                    feather: 40.0,
                    invert: false,
                }),
            }],
            adjustments: edit_model::LocalAdjustments {
                exposure: 0.45,
                ..Default::default()
            },
            disabled: false,
            invert: false,
        }],
        ..Edits::default()
    }
}

/// ⚠ A radial **cut out of** a gradient — `docs/BACKLOG.md`'s one-line statement of the whole gap.
#[test]
fn golden_mask_subtract() {
    check_golden(
        "mask_subtract",
        &composite_case(edit_model::MaskOp::Subtract),
    );
}

/// ⚠ The gradient kept only where the radial also reaches.
#[test]
fn golden_mask_intersect() {
    check_golden(
        "mask_intersect",
        &composite_case(edit_model::MaskOp::Intersect),
    );
}

/// ⚠ The union, which is the op two separate masks cannot express: where they overlap the
/// adjustment applies **once**, not twice.
#[test]
fn golden_mask_add() {
    check_golden("mask_add", &composite_case(edit_model::MaskOp::Add));
}

/// The subtract composite, inverted as a whole (M10.12a): everything the gradient-minus-radial
/// left out — the radial's centre and the gradient's uncovered end.
///
/// ⚠ **Measured, and the gradient runs LEFT-to-right here — the opposite of `composite_case`.**
/// Inverting moves full coverage to the gradient's other end, so `composite_case`'s right-to-left
/// gradient put the +0.45 EV lift on the ramp's bright end: **361** clipped samples against the
/// chart's own 24. Left-to-right lands it on the dark end again: 24, the floor exactly.
fn inverted_case() -> Edits {
    let mut e = composite_case(edit_model::MaskOp::Subtract);
    e.masks[0].invert = true;
    e.masks[0].mask = edit_model::Mask::Linear(edit_model::LinearGradient {
        from: [0.0, 0.0],
        to: [1.0, 0.0],
        invert: false,
    });
    e
}

#[test]
fn golden_mask_subtract_inverted() {
    check_golden("mask_subtract_inverted", &inverted_case());
}

/// ⚠⚠ The inverted reference must differ from the uninverted one, or a flag ignored everywhere
/// renders the subtract golden twice and passes forever. And it must add no clipping of its own —
/// inverting moves full coverage to the other end of the chart's ramp, which is exactly where
/// `composite_case` warns a lift clips.
#[test]
fn the_inverted_golden_discriminates() {
    let render = |e: &Edits| {
        to_rgb8(&build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).run(chart()))
    };
    let inv = render(&inverted_case());
    // ⚠ Against the SAME case with the flag off — `composite_case` runs its gradient the other
    // way, so comparing with it would pass even if the flag did nothing.
    let mut plain = inverted_case();
    plain.masks[0].invert = false;
    assert_ne!(inv, render(&plain), "the flag changed nothing");
    let clipped = |px: &[u8]| px.iter().filter(|&&v| v == 255).count();
    let floor = clipped(&render(&Edits::default()));
    assert_eq!(
        clipped(&inv),
        floor,
        "the inverted composite clipped {} samples against the chart's own {floor}",
        clipped(&inv)
    );
    let distinct: std::collections::HashSet<u8> = inv.iter().copied().collect();
    assert!(
        distinct.len() >= 100,
        "only {} distinct values",
        distinct.len()
    );
}

/// ⚠⚠ **Three plausible PNGs prove nothing.** A composite that silently ignored its refinements
/// would render all three identically to a plain masked gradient, and every golden above would
/// pass against it forever. This is what makes the references mean something: each op must differ
/// from the other two *and* from the base mask alone.
#[test]
fn the_three_composite_goldens_differ_from_one_another_and_from_the_base() {
    let render = |e: &Edits| {
        to_rgb8(&build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).run(chart()))
    };
    let sub = render(&composite_case(edit_model::MaskOp::Subtract));
    let int = render(&composite_case(edit_model::MaskOp::Intersect));
    let add = render(&composite_case(edit_model::MaskOp::Add));

    let mut base = composite_case(edit_model::MaskOp::Add);
    base.masks[0].refine.clear();
    let plain = render(&base);

    assert_ne!(sub, int, "subtract and intersect must differ");
    assert_ne!(sub, add, "subtract and add must differ");
    assert_ne!(int, add, "intersect and add must differ");
    assert_ne!(sub, plain, "a subtracted radial must change the picture");
    assert_ne!(int, plain, "an intersected radial must change the picture");
    assert_ne!(add, plain, "an added radial must change the picture");
}

/// ⚠⚠ **Measured, not eyeballed.** A clipped reference cannot tell full coverage from any larger
/// value, which is exactly how M3.1b's exposure golden lost its discriminating power — and
/// clipping is invisible when you look at the PNG.
#[test]
fn the_composite_goldens_discriminate() {
    // ⚠⚠ The floor is the **neutral chart's own** clipping, not zero: `chart()`'s ramp reaches
    // 1.0, so 24 samples are pinned before any mask exists. Asserting zero would be asserting
    // something false about correct code — the trap this project has hit four times — so what is
    // asserted is that the composite adds none of its own.
    let clipped = |px: &[u8]| px.iter().filter(|&&v| v == 255).count();
    let neutral = to_rgb8(
        &build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(chart()),
    );
    let floor = clipped(&neutral);

    for op in edit_model::MaskOp::ALL {
        let px = to_rgb8(
            &build_pipeline(
                &composite_case(op),
                RenderScale::FULL,
                TailRendering::colorimetric(),
            )
            .run(chart()),
        );
        let distinct: std::collections::HashSet<u8> = px.iter().copied().collect();
        assert_eq!(
            clipped(&px),
            floor,
            "{op:?} clipped {} samples against the chart's own {floor}",
            clipped(&px)
        );
        assert!(
            distinct.len() >= 100,
            "{op:?}: only {} distinct values, so the reference discriminates little",
            distinct.len()
        );
    }
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
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
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
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
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
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

/// A masked Clarity over the RIGHT half of the local-contrast chart.
///
/// ⚠ One helper for the golden and the confinement test below, so the two cannot describe
/// different edits while claiming to be about the same thing.
///
/// ⚠ The gradient runs 0.6 -> 0.9, so everything left of x = 0.6 is at coverage **exactly zero** —
/// which is what makes an assertion about pixel (1, 1) meaningful rather than approximate.
fn masked_clarity_edits() -> Edits {
    Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.6, 0.5],
                to: [0.9, 0.5],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                local_contrast: LocalContrast {
                    texture: 0.0,
                    clarity: 80.0,
                },
                ..Default::default()
            },
            disabled: false,
            invert: false,
        }],
        ..Edits::default()
    }
}

/// A masked Clarity leaves every uncovered pixel **bit-identical**.
///
/// ⚠⚠ "A masked clarity changes the pixels" passes with the mask ignored entirely — a global
/// clarity would satisfy it. **This is the discriminating half**: outside the mask the result must
/// be identical, not close. Blending at coverage 0 is bit-exactly the identity, so there is no
/// tolerance to argue about.
///
/// ⚠ The second assertion is the fixture check. Without it the test passes just as well against a
/// mask that changes nothing anywhere, which proves nothing at all.
#[test]
fn a_masked_clarity_leaves_uncovered_pixels_bit_identical() {
    let chart = local_contrast_chart();
    let plain = build_pipeline(
        &Edits::default(),
        RenderScale::FULL,
        TailRendering::colorimetric(),
    )
    .run(chart.clone());
    let masked = build_pipeline(
        &masked_clarity_edits(),
        RenderScale::FULL,
        TailRendering::colorimetric(),
    )
    .run(chart);

    assert_eq!(
        plain.pixel(1, 1),
        masked.pixel(1, 1),
        "the masked clarity leaked outside its mask"
    );
    assert_ne!(
        plain.data(),
        masked.data(),
        "fixture check: the mask changed nothing at all, so this test proves nothing"
    );
}

/// The masked-Clarity reference.
///
/// ⚠ On `local_contrast_chart` rather than the main chart: the main one is primaries and grey,
/// where a local-contrast band has almost nothing to act on.
#[test]
fn golden_mask_clarity() {
    check_golden_with(
        "mask_clarity",
        &masked_clarity_edits(),
        local_contrast_chart(),
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
                        flow: 100.0,
                        erase: false,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
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
                            flow: 100.0,
                            erase: false,
                            ..Default::default()
                        },
                        edit_model::BrushStroke {
                            points: vec![[0.3, 0.5], [0.7, 0.5]],
                            radius: 0.08,
                            hardness: 100.0,
                            flow: 100.0,
                            erase: true,
                            ..Default::default()
                        },
                    ],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

/// One auto-masked dab over the hue sweep, lifting exposure.
///
/// ⚠ The hue sweep is the right chart for this and the primaries chart is not: the gate carves a
/// band around the hue under the dab's own centre, so a *gradient* produces a smooth falloff with
/// many distinct values, where four flat bands would give a step and pin far fewer samples.
fn auto_mask_edits() -> Edits {
    Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![edit_model::BrushStroke {
                    // A single dab at the centre, wide enough that its disc spans hues the gate
                    // must refuse as well as the ones it must accept.
                    points: vec![[0.5, 0.5]],
                    radius: 0.45,
                    hardness: 40.0,
                    flow: 100.0,
                    erase: false,
                    auto_mask: true,
                    tolerance: 30.0,
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                // ⚠ 0.55 EV for `mask_brush_exposure`'s measured reason: the sweep is authored at
                // value 0.8, so a larger lift would pin samples at 255 and the reference could no
                // longer discriminate the gate's peak from any greater coverage.
                exposure: 0.55,
                ..Default::default()
            },
            disabled: false,
            invert: false,
        }],
        ..Edits::default()
    }
}

#[test]
fn the_auto_masked_lift_is_confined_to_a_band_around_the_centres_hue() {
    // ⚠⚠ The shape assertion, and what `golden_mask_brush_auto` cannot give on its own: a
    // reference records *an* image, and by eye this chart's own hue sweep dominates it, so
    // "the lift landed in the right place" is not something looking at the PNG can settle.
    //
    // The dab spans nearly the whole width (radius 0.45), so a gate that never fired would lift
    // essentially every column. Confinement to a band is the property, and it is measured here
    // rather than trusted.
    let gated = auto_mask_edits();
    let neutral = Edits::default();
    let a = to_rgb8(
        &build_pipeline(&gated, RenderScale::FULL, TailRendering::colorimetric())
            .run(hue_sweep_chart()),
    );
    let b = to_rgb8(
        &build_pipeline(&neutral, RenderScale::FULL, TailRendering::colorimetric())
            .run(hue_sweep_chart()),
    );

    let row = 8usize;
    let lifted: Vec<usize> = (0..W)
        .filter(|x| {
            let i = (row * W + x) * 3;
            (0..3).any(|c| a[i + c] != b[i + c])
        })
        .collect();

    assert!(!lifted.is_empty(), "nothing was lifted at all");
    let (lo, hi) = (lifted[0], *lifted.last().unwrap());
    assert!(
        hi - lo + 1 < W,
        "the lift spans the whole width ({lo}..={hi}) - the gate is not confining it"
    );
    // The dab is centred at x = W/2, so the band must contain it.
    assert!(
        lo <= W / 2 && hi >= W / 2,
        "the band {lo}..={hi} does not contain the dab's own centre"
    );
    // ⚠ And it must be genuinely narrower than the disc, not merely one pixel short of it.
    assert!(
        hi - lo + 1 < W * 3 / 4,
        "the band {lo}..={hi} is barely narrower than the disc"
    );
}

#[test]
fn golden_mask_brush_auto() {
    check_golden_with("mask_brush_auto", &auto_mask_edits(), hue_sweep_chart());
}

#[test]
fn an_auto_masked_dab_really_refuses_part_of_its_own_disc() {
    // ⚠⚠ **A golden alone cannot prove the gate fires**, and this is the W5 creative-LUT lesson
    // applied: a gate that never ran would produce a reference identical to the ungated render,
    // and `golden_mask_brush_auto` would then pass against it forever. Rendering the same dab
    // both ways is what discriminates.
    let gated = auto_mask_edits();
    let mut plain = auto_mask_edits();
    if let edit_model::Mask::Brush(b) = &mut plain.masks[0].mask {
        b.strokes[0].auto_mask = false;
    }
    let a = to_rgb8(
        &build_pipeline(&gated, RenderScale::FULL, TailRendering::colorimetric())
            .run(hue_sweep_chart()),
    );
    let b = to_rgb8(
        &build_pipeline(&plain, RenderScale::FULL, TailRendering::colorimetric())
            .run(hue_sweep_chart()),
    );
    let moved = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
    assert!(
        moved > a.len() / 20,
        "auto-mask changed only {moved} of {} samples - the gate is not firing",
        a.len()
    );
}

#[test]
fn golden_mask_brush_flow_buildup() {
    // ⚠ **This case exists because the other two could not witness the flow change.** When dabs
    // moved from a flat `max` to per-stroke max plus compositing, all 37 goldens stayed
    // byte-identical — `mask_brush_exposure` has one stroke (where the two rules agree exactly)
    // and `mask_brush_erase` uses a hard-edged eraser (where they also agree). A model change no
    // reference can see is a model change with no regression guard.
    //
    // Three overlapping passes at flow 35: each adds a fraction of what is still uncovered, so
    // the overlap region is visibly stronger than the single-pass tails.
    let pass = |from: f32, to: f32| edit_model::BrushStroke {
        points: vec![[from, 0.5], [to, 0.5]],
        radius: 0.15,
        hardness: 30.0,
        flow: 35.0,
        erase: false,
        ..Default::default()
    };
    check_golden(
        "mask_brush_flow_buildup",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![pass(0.1, 0.5), pass(0.2, 0.6), pass(0.3, 0.5)],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

/// Four tinted ramps whose **luminances differ strongly while their positions do not**.
///
/// ⚠⚠ This is the whole point of the chart, and no existing one has the property. A luminance
/// mask on a chart of uniform lightness is invisible; on a single ramp it is indistinguishable
/// from a horizontal gradient mask. Here the same `x` sits at a different lightness in every band
/// — measured across the ramp: gray spans about 8..76, green 6..68, red 3..50, blue 2..44 — so a
/// band of `[20, 50]` selects a *different portion of each row*, which a positional mask cannot do
/// and which is plainly visible in the reference.
///
/// ⚠ Tinted rather than pure primaries, so **no channel is ever 0**. `chart()`'s primaries pin two
/// channels at 0 in three of its four bands, and a reference already pinned cannot discriminate
/// what a mask does there — the weakness M3.1b's radial golden and 3.1a's linear one each had to
/// be regenerated for.
///
/// ⚠⚠ **The ramp's top is 0.53, not 1.0, and the number was arrived at by measurement.** The
/// hazard is specific to a *luminance* mask and is the mirror of the usual one: the blue band has
/// a **high channel value at a low lightness**, so a band chosen to select shadows selects the
/// blue band's brightest samples too, and +0.55 EV pinned 68 of them at 255. A ramp reaching 1.0
/// is safe for a positional mask and is not safe here.
///
/// ⚠ `set_srgb` takes **linear** sRGB, as `chart()` does and `hue_sweep_chart` makes explicit by
/// calling `srgb_to_linear` first. Reading it as display-referred is what produced the clipping
/// above: the intended top of 0.88 encoded to 241, not to 224.
fn luminance_chart() -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for y in 0..H {
        let band = y * 4 / H; // 0..3
        for x in 0..W {
            let v = 0.006 + (x as f32 / (W - 1) as f32) * 0.526;
            let t = v * 0.25;
            let px = match band {
                0 => [v, v, v],
                1 => [v, t, t],
                2 => [t, v, t],
                _ => [t, t, v],
            };
            set_srgb(&mut img, x, y, px);
        }
    }
    img
}

/// The two luminance-mask goldens, as one table.
///
/// ⚠ One list with two consumers — the golden tests and the discrimination guard below — for the
/// reason [`grading_cases`] gives: two copies of an edit set let the reference and the guard drift
/// apart, and the guard would then be measuring an image no reference contains.
fn luminance_mask_cases() -> Vec<(&'static str, Edits)> {
    let masked = |r: edit_model::LuminanceRange, a: edit_model::LocalAdjustments| Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Luminance(r),
            refine: Vec::new(),
            adjustments: a,
            disabled: false,
            invert: false,
        }],
        ..Edits::default()
    };
    vec![
        (
            // "Lift the shadows and lower midtones" — full coverage over lightness 20..50 with a
            // 15-point smoothstep shoulder either side, so the reference records the *ramp* and
            // not merely a step.
            //
            // ⚠ +0.55 EV, the value M3.1b settled on for the same reason: at +1.0 the brightest
            // covered samples pin at 255 and the reference can no longer tell full coverage from
            // any greater value.
            "mask_luminance_shadows",
            masked(
                edit_model::LuminanceRange {
                    low: 30.0,
                    high: 55.0,
                    feather: 30.0,
                    invert: false,
                },
                edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
            ),
        ),
        (
            // The inverted case with a different payload, so the pair covers `invert` and a
            // second stage inside the mask — the split `mask_radial_inverted_saturation` makes.
            "mask_luminance_inverted_highlights",
            masked(
                edit_model::LuminanceRange {
                    low: 30.0,
                    high: 55.0,
                    feather: 30.0,
                    invert: true,
                },
                edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
            ),
        ),
    ]
}

fn luminance_mask_case(name: &str) -> Edits {
    luminance_mask_cases()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no luminance-mask case named {name}"))
        .1
}

#[test]
fn golden_mask_luminance_shadows() {
    check_golden_with(
        "mask_luminance_shadows",
        &luminance_mask_case("mask_luminance_shadows"),
        luminance_chart(),
    );
}

#[test]
fn golden_mask_luminance_inverted_highlights() {
    check_golden_with(
        "mask_luminance_inverted_highlights",
        &luminance_mask_case("mask_luminance_inverted_highlights"),
        luminance_chart(),
    );
}

/// ⚠⚠ **Measured, not eyeballed** — clipping is invisible to the eye and fatal to a reference's
/// discriminating power, and this project has regenerated two mask goldens for exactly that.
///
/// Three properties, and the third is the one specific to a *range* mask:
/// 1. nothing pins at 0 or 255, so every sample encodes what the mask actually did;
/// 2. a substantial share of samples **differ** from the unmasked render, so the reference would
///    move if the mask stopped working;
/// 3. a substantial share **do not**, which is what makes it a selection rather than a global
///    adjustment — a mask covering everything would satisfy (2) alone and be indistinguishable
///    from moving the Exposure slider.
#[test]
fn the_luminance_mask_goldens_discriminate() {
    let neutral = to_rgb8(
        &build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(luminance_chart()),
    );
    for (name, edits) in luminance_mask_cases() {
        let bytes = to_rgb8(
            &build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .run(luminance_chart()),
        );
        let hi = bytes.iter().filter(|&&c| c == 255).count();
        let lo = bytes.iter().filter(|&&c| c == 0).count();
        assert!(
            hi == 0 && lo == 0,
            "{name}: {hi} samples clipped high, {lo} clipped low — the golden cannot \
             discriminate where it is pinned"
        );
        let moved = bytes.iter().zip(&neutral).filter(|(a, b)| a != b).count();
        let total = bytes.len();
        assert!(
            moved * 5 >= total,
            "{name}: only {moved}/{total} samples differ from the unmasked render — too few to \
             witness a change"
        );
        // ⚠ Asserted as "enough samples are UNTOUCHED" rather than as an upper bound on `moved`,
        // because `moved` counts a one-level partial change the same as a full one — with a
        // smoothstep shoulder spanning 15 lightness points, most of the chart is touched *a
        // little*. What distinguishes a selection from a global adjustment is that a real region
        // comes back byte-identical, which is what this measures. Measured: 428 and 1680 of 3072.
        let untouched = total - moved;
        assert!(
            untouched * 20 >= total,
            "{name}: only {untouched}/{total} samples are untouched — the mask is covering \
             everything, which a global adjustment would do too"
        );
    }
}

#[test]
fn golden_output_p3() {
    check_golden_in_space(
        "output_p3",
        &Edits::default(),
        color::OutputSpace::DisplayP3,
    );
}

/// The edits for one grading golden.
///
/// ⚠ Looked up from [`grading_cases`] rather than written out again here. Two copies of the
/// same edit set would let the goldens and `the_grading_goldens_do_not_clip` drift apart, and
/// the clipping guard would then be measuring an image no reference contains.
fn grading_case(name: &str) -> Edits {
    grading_cases()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no grading case named {name}"))
        .1
}

#[test]
fn golden_grade_split_tone() {
    check_golden_with(
        "grade_split_tone",
        &grading_case("grade_split_tone"),
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_luminance() {
    check_golden_with(
        "grade_luminance",
        &grading_case("grade_luminance"),
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_blending_wide() {
    check_golden_with(
        "grade_blending_wide",
        &grading_case("grade_blending_wide"),
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_balance_high() {
    check_golden_with(
        "grade_balance_high",
        &grading_case("grade_balance_high"),
        grade_ramp_chart(),
    );
}

/// The four grading edit sets, paired with their golden names.
fn grading_cases() -> Vec<(&'static str, Edits)> {
    vec![
        (
            "grade_split_tone",
            Edits {
                color_grading: split_tone(),
                ..Edits::default()
            },
        ),
        (
            "grade_luminance",
            Edits {
                color_grading: ColorGrading {
                    shadows: GradeWheel {
                        hue: 0.0,
                        saturation: 0.0,
                        luminance: -70.0,
                    },
                    highlights: GradeWheel {
                        hue: 0.0,
                        saturation: 0.0,
                        luminance: 70.0,
                    },
                    ..ColorGrading::default()
                },
                ..Edits::default()
            },
        ),
        (
            "grade_blending_wide",
            Edits {
                color_grading: ColorGrading {
                    blending: 100.0,
                    ..split_tone()
                },
                ..Edits::default()
            },
        ),
        (
            "grade_balance_high",
            Edits {
                color_grading: ColorGrading {
                    balance: 80.0,
                    ..split_tone()
                },
                ..Edits::default()
            },
        ),
    ]
}

/// ⚠ A golden that clips cannot discriminate one grade from another, and clipping is invisible
/// to the eye — the exposure golden pinned 15 of 64 samples at 255 and nobody noticed until it
/// failed to detect a change. This is why `grade_ramp_chart` stops at 0.85 rather than 1.0.
/// If this fails, lower the ramp's top; do not regenerate the references.
#[test]
fn the_grading_goldens_do_not_clip() {
    for (name, edits) in grading_cases() {
        let bytes = to_rgb8(
            &build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .run(grade_ramp_chart()),
        );
        let hi = bytes.iter().filter(|&&c| c == 255).count();
        let lo = bytes.iter().filter(|&&c| c == 0).count();
        let max = bytes.iter().max().copied().unwrap();
        assert!(
            hi == 0 && lo == 0,
            "{name}: {hi} samples clipped high, {lo} clipped low (max {max}) — the golden \
             cannot discriminate where it is pinned"
        );
    }
}

/// A grade whose parameters did nothing would still produce four valid-looking PNGs. Each case
/// varies exactly one axis, so any two rendering alike means that axis is inert.
#[test]
fn the_four_grading_goldens_differ_from_one_another() {
    let rendered: Vec<(&str, Vec<u8>)> = grading_cases()
        .into_iter()
        .map(|(n, e)| {
            (
                n,
                to_rgb8(
                    &build_pipeline(&e, RenderScale::FULL, TailRendering::colorimetric())
                        .run(grade_ramp_chart()),
                ),
            )
        })
        .collect();
    for i in 0..rendered.len() {
        for j in (i + 1)..rendered.len() {
            assert_ne!(
                rendered[i].1, rendered[j].1,
                "{} and {} render identically",
                rendered[i].0, rendered[j].0
            );
        }
    }
}

/// A grid of straight lines — the only chart on which distortion is legible at all.
///
/// ⚠ A ramp or a colour chart cannot show a geometric correction; straight lines bowing is the
/// whole visible effect. Lines are two pixels wide so a sub-pixel resample cannot erase them.
fn grid_chart() -> ImageBuf {
    const GW: usize = 96;
    const GH: usize = 64;
    let mut img = ImageBuf::new(GW, GH);
    for y in 0..GH {
        for x in 0..GW {
            let on_line = x % 16 < 2 || y % 16 < 2;
            let v = if on_line { 0.85 } else { 0.12 };
            set_srgb(&mut img, x, y, [v, v, v]);
        }
    }
    img
}

/// A synthetic lens with coefficients chosen here, not read from the bundled database.
///
/// ⚠ This is what makes lens-correction goldens possible at all: a real profile needs a real
/// camera and lens, which a synthetic chart does not have, and a fixture-gated test is no
/// evidence in CI.
///
/// ⚠ The distortion is **pincushion** (positive `b`), deliberately: it is the direction that
/// empties the frame, so it is the one whose scale slider has anything to do.
fn lens_shot() -> lens_profile::LensShot {
    let xml = r#"<lensdatabase version="1">
        <camera><maker>TestMaker</maker><model>TestCam</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
        <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor>
            <calibration>
                <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                <vignetting model="pa" focal="50" aperture="2.8" distance="1000"
                            k1="-0.5" k2="0.0" k3="0.0"/>
            </calibration></lens>
    </lensdatabase>"#;
    let db = lens_profile::Catalogue::from_xml(xml).unwrap();
    let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
    lens_profile::LensShot {
        profile: p,
        shot: lens_profile::Shot {
            focal: 50.0,
            aperture: 2.8,
        },
    }
}

fn check_lens_golden(name: &str, c: lens_profile::Corrections) {
    let chart = grid_chart();
    let edits = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: c.distortion,
            tca: c.tca,
            vignetting: c.vignetting,
            scale: c.scale,
        }),
        ..Edits::default()
    };
    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        &edits,
        RenderScale::FULL,
        None,
        None,
        pipeline::RenderSpec {
            resampler: RESAMPLER,
            ..pipeline::RenderSpec::default()
        },
        Some(lens_shot()),
        pipeline::TailRendering::default(),
    );
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    compare_or_update(name, pipeline::Pipeline::new(stages).run(chart));
}

#[test]
fn golden_lens_distortion() {
    check_lens_golden(
        "lens_distortion",
        lens_profile::Corrections {
            distortion: true,
            tca: false,
            vignetting: false,
            scale: 0.0,
        },
    );
}

#[test]
fn golden_lens_distortion_scaled() {
    check_lens_golden(
        "lens_distortion_scaled",
        lens_profile::Corrections {
            distortion: true,
            tca: false,
            vignetting: false,
            scale: 100.0,
        },
    );
}

#[test]
fn golden_lens_tca() {
    check_lens_golden(
        "lens_tca",
        lens_profile::Corrections {
            distortion: false,
            tca: true,
            vignetting: false,
            scale: 0.0,
        },
    );
}

#[test]
fn golden_lens_vignetting() {
    check_lens_golden(
        "lens_vignetting",
        lens_profile::Corrections {
            distortion: false,
            tca: false,
            vignetting: true,
            scale: 0.0,
        },
    );
}

/// ⚠ Reuses the **existing** `grid_chart` the lens-correction goldens already use, rather than
/// adding a second grid. One chart for every geometric stage means a keystone golden and a
/// distortion golden are directly comparable, and the chart's dimness (0.85 on 0.12) has already
/// been shown not to clip.
#[test]
fn golden_perspective_vertical() {
    check_golden_with(
        "perspective_vertical",
        &Edits {
            // ⚠ `scale: 0` — the **raw** transform, so the reference is mostly image rather than
            // mostly black. At the default (−100, fit) this chart comes back 66% black, which
            // leaves only a third of the pixels able to witness a change in the transform. The
            // fit path is covered by `perspective_aspect` below and by three unit tests.
            perspective: edit_model::Perspective {
                vertical: 55.0,
                scale: 0.0,
                ..Default::default()
            },
            ..Default::default()
        },
        grid_chart(),
    );
}

#[test]
fn golden_perspective_aspect() {
    check_golden_with(
        "perspective_aspect",
        &Edits {
            // ⚠ Left at the default `scale: -100`, so this one exercises the **fit** path — the
            // pair covers both, which one setting could not.
            perspective: edit_model::Perspective {
                horizontal: -40.0,
                aspect: 60.0,
                ..Default::default()
            },
            ..Default::default()
        },
        grid_chart(),
    );
}

/// A smooth vertical gradient with a dark 6 px "dust spot" — the one chart on which a clone and
/// a heal are legible and *distinguishable*.
///
/// ⚠ The source sits **diagonally** from the destination, not to the side, so clone lands a
/// visibly lighter disc where heal lands one that continues the gradient.
///
/// ⚠⚠ **AND THE GRADIENT ITSELF IS DIAGONAL, WHICH THE SABOTAGE STEP IS WHAT ESTABLISHED.** A
/// purely vertical ramp makes a *horizontal* offset error completely invisible: forcing `dx = 0`
/// left both references byte-identical and the guard green, because two points on the same row
/// have the same brightness. The spec predicted exactly this ("a clone test on a uniform source
/// cannot see a wrong offset") and a one-axis gradient is uniform along the other axis. With both
/// axes contributing, every component of the offset moves pixels.
///
/// ⚠ The ramp stops short of both ends (0.15..0.75) for `grade_ramp_chart`'s reason: a clipped
/// golden cannot tell one effect from a stronger one. `the_spot_goldens_do_not_clip_and_differ`
/// is the guard.
const SPOT_W: usize = 64;
const SPOT_H: usize = 48;

/// The retouch radius in pixels, shared by the cases and the guard so the two cannot disagree
/// about where the patch is. Comfortably larger than the 6 px dust spot — see `spot_cases`.
const SPOT_R: f32 = 9.0;

/// Where the dust spot and the retouch are centred.
const SPOT_C: (f32, f32) = (24.0, 20.0);

fn spot_chart() -> ImageBuf {
    let (sw, sh) = (SPOT_W, SPOT_H);
    let mut img = ImageBuf::new(sw, sh);
    for y in 0..sh {
        for x in 0..sw {
            let u = x as f32 / (sw - 1) as f32;
            let w = y as f32 / (sh - 1) as f32;
            let v = 0.15 + 0.60 * (0.5 * u + 0.5 * w);
            // A warm tint, so a per-channel heal shift is visible rather than only a grey level.
            let px = [v, v * 0.88, v * 0.74];
            set_srgb(&mut img, x, y, px);
        }
    }
    // The dust spot: a dark disc the retouch is there to remove.
    let (cx, cy) = SPOT_C;
    for y in 0..sh {
        for x in 0..sw {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d <= 6.0 {
                set_srgb(&mut img, x, y, [0.05, 0.05, 0.05]);
            }
        }
    }
    img
}

/// The two spot cases, looked up from one table so the goldens and their guard cannot describe
/// different renders — `grading_cases`' rule.
fn spot_cases() -> Vec<(&'static str, Edits)> {
    let mk = |mode| Edits {
        spots: vec![edit_model::Spot {
            centre: [24.0 / 63.0, 20.0 / 47.0],
            // Down and to the right: far enough that the gradient has visibly moved.
            source: [44.0 / 63.0, 34.0 / 47.0],
            // ⚠⚠ **9 px against the dust spot's 6, and the margin is load-bearing.** Heal matches
            // the annulus `r < d ≤ 1.5r`, so a spot no larger than the blemish measures a ring
            // that still lies *on* the blemish and dutifully matches the thing being removed —
            // which is exactly what the first version of this case did (patch 146 against a
            // surround of 171). A real retouch covers the blemish with room to spare, and that
            // is not merely tidiness: it is what makes the ring describe the surroundings.
            radius: SPOT_R / 64.0,
            feather: 40.0,
            mode,
        }],
        ..Edits::default()
    };
    vec![
        ("spot_clone", mk(edit_model::SpotMode::Clone)),
        ("spot_heal", mk(edit_model::SpotMode::Heal)),
    ]
}

fn spot_case(name: &str) -> Edits {
    spot_cases()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, e)| e)
        .unwrap()
}

#[test]
fn golden_spot_clone() {
    check_golden_with("spot_clone", &spot_case("spot_clone"), spot_chart());
}

#[test]
fn golden_spot_heal() {
    check_golden_with("spot_heal", &spot_case("spot_heal"), spot_chart());
}

/// ⚠ Two valid-looking PNGs prove nothing on their own. This asserts three things a picture
/// cannot: neither clips (so each can still witness a change), the two genuinely differ, and
/// **heal matches the surrounding gradient where clone does not** — which is the whole
/// difference between the modes, measured rather than eyeballed.
#[test]
fn the_spot_goldens_do_not_clip_and_differ() {
    let mut rendered: Vec<(&str, Vec<u8>)> = Vec::new();
    for (name, edits) in spot_cases() {
        let bytes = to_rgb8(
            &build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .run(spot_chart()),
        );
        let hi = bytes.iter().filter(|&&c| c == 255).count();
        let lo = bytes.iter().filter(|&&c| c == 0).count();
        assert!(
            hi == 0 && lo == 0,
            "{name}: {hi} samples clipped high, {lo} low — the golden cannot discriminate \
             where it is pinned"
        );
        rendered.push((name, bytes));
    }
    let differing = rendered[0]
        .1
        .iter()
        .zip(&rendered[1].1)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing >= 20,
        "clone and heal differ in only {differing} samples — the pair has no discriminating power"
    );

    // The measured half: the mean over the patch against the mean of the ring around it.
    // Heal exists to make those agree; clone brings the source's own brightness with it.
    //
    // ⚠ The ring is measured **on the neutral render, in place**, rather than sampled from some
    // fixed row or column. The gradient runs diagonally, so any fixed sample describes a
    // different brightness from the patch's own neighbourhood and the comparison would be
    // between two unrelated numbers.
    let (cx, cy) = SPOT_C;
    let region_mean = |bytes: &[u8], lo: f32, hi: f32| -> f32 {
        let (mut sum, mut n) = (0.0f32, 0usize);
        for y in 0..SPOT_H {
            for x in 0..SPOT_W {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d > lo && d <= hi {
                    sum += bytes[(y * SPOT_W + x) * 3] as f32;
                    n += 1;
                }
            }
        }
        sum / n as f32
    };
    // Well inside the patch, so the feathered rim cannot dilute the reading.
    let patch_mean = |bytes: &[u8]| region_mean(bytes, -1.0, SPOT_R * 0.6);
    let neutral = to_rgb8(
        &build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(spot_chart()),
    );
    let surround = region_mean(&neutral, SPOT_R, SPOT_R * 1.5);
    let heal = patch_mean(&rendered[1].1);
    let clone = patch_mean(&rendered[0].1);
    assert!(
        (heal - surround).abs() < 6.0,
        "heal should continue the gradient: patch {heal:.1} vs surround {surround:.1}"
    );
    assert!(
        (clone - surround).abs() > 12.0,
        "clone should carry the source's brightness: patch {clone:.1} vs surround {surround:.1}"
    );
}

/// M9.11: pale skin, and two eyes — iris rings of radius 10 around a red pupil on the left and a green eyeshine on the right, both radius 5.
fn red_eye_chart() -> ImageBuf {
    let (sw, sh) = (SPOT_W, SPOT_H);
    let mut img = ImageBuf::new(sw, sh);
    for y in 0..sh {
        for x in 0..sw {
            let d = |cx: f32| ((x as f32 - cx).powi(2) + (y as f32 - 24.0).powi(2)).sqrt();
            // ⚠ `set_srgb` takes LINEAR sRGB. A flash pupil is ~(0.60, 0.10, 0.10) *encoded*, which
            // is (0.32, 0.01, 0.01) linear; written as linear (0.60, 0.10, 0.10) it is a pale pink
            // the stage rightly corrects only partly — the first draft of this chart did that.
            let px = if d(20.0) <= 5.0 {
                [0.32, 0.01, 0.01]
            } else if d(44.0) <= 5.0 {
                [0.55, 0.95, 0.45]
            } else if d(20.0) <= 10.0 || d(44.0) <= 10.0 {
                // ⚠ An iris ring, as in a real eye: the red stage's fringe pass corrects any
                // faintly red pixel near the pupil, so skin placed directly against it (this
                // chart's first version) was greyed into a pale ring — pinned by the golden.
                [0.10, 0.10, 0.11]
            } else {
                [0.78, 0.57, 0.44]
            };
            set_srgb(&mut img, x, y, px);
        }
    }
    img
}

/// One red and one pet eye (with its catchlight), circles 7 px against the pupils' 5 — a real
/// click pads the detected pupil, so skin inside the circle is part of what this pins.
fn red_eye_case() -> Edits {
    let eye = |x: f32, kind, catchlight| edit_model::Eye {
        centre: [x / 63.0, 24.0 / 47.0],
        radius: 7.0 / 64.0,
        kind,
        pupil_size: 50.0,
        darken: 50.0,
        catchlight,
    };
    Edits {
        eyes: vec![
            eye(20.0, edit_model::EyeKind::Red, None),
            eye(44.0, edit_model::EyeKind::Pet, Some([-0.35, -0.35])),
        ],
        ..Edits::default()
    }
}

#[test]
fn golden_red_eye() {
    check_golden_with("red_eye", &red_eye_case(), red_eye_chart());
}

/// ⚠ A golden of an unchanged picture would pin nothing: prove both pupils actually moved, and
/// the skin between them did not.
#[test]
fn the_red_eye_golden_corrects_both_pupils_and_not_the_skin() {
    let render = |e: &Edits| {
        to_rgb8(
            &build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric())
                .run(red_eye_chart()),
        )
    };
    let (fixed, neutral) = (render(&red_eye_case()), render(&Edits::default()));
    let at = |b: &[u8], x: usize, y: usize| -> [i32; 3] {
        let i = (y * SPOT_W + x) * 3;
        [b[i] as i32, b[i + 1] as i32, b[i + 2] as i32]
    };
    let red_before = at(&neutral, 20, 24);
    let red_after = at(&fixed, 20, 24);
    assert!(
        red_before[0] - red_before[1] > 60,
        "fixture check: the left pupil is red on the neutral render: {red_before:?}"
    );
    assert!(
        (red_after[0] - red_after[1]).abs() < 25,
        "the red pupil is still red: {red_after:?}"
    );
    let pet_before = at(&neutral, 45, 25);
    let pet_after = at(&fixed, 45, 25);
    // ⚠ Measured, not guessed: a linear factor of ~0.09 (Darken 50) encodes to about a third of
    // the sRGB value — [196, 249, 179] became [77, 77, 77] — so "less than half, and grey" is the
    // property, where a first bar of "a third" sat exactly on the measurement.
    assert!(
        pet_after.iter().sum::<i32>() * 2 < pet_before.iter().sum::<i32>(),
        "the eyeshine was not darkened: {pet_before:?} → {pet_after:?}"
    );
    let spread = pet_after.iter().max().unwrap() - pet_after.iter().min().unwrap();
    assert!(spread <= 3, "the pet pupil kept its colour: {pet_after:?}");
    assert_eq!(
        at(&fixed, 32, 24),
        at(&neutral, 32, 24),
        "the skin between the eyes moved"
    );
}

// ── M7.1b, colour range masks ────────────────────────────────────────────────────────────────
//
// ⚠ **Appended, never inserted** — PARALLEL-WORK.md rule 3, and another track adds goldens in this
// same wave. A new case goes at the end of the file and its reference is a new PNG.

/// The colour-mask golden, as a one-entry table.
///
/// ⚠ One list with two consumers — the golden test and the discrimination guard below — for the
/// reason [`luminance_mask_cases`] and [`grading_cases`] both give: two copies of an edit set let
/// the reference and the guard drift apart, and the guard would then be measuring an image no
/// reference contains.
fn colour_mask_cases() -> Vec<(&'static str, Edits)> {
    vec![(
        // ⚠⚠ **The band CROSSES 0°, which is the property no other golden here can witness.**
        // 300 → 60 selects the magentas, reds and oranges either side of zero; read as a sorted
        // 60..300 it would select everything *except* them, and the reference would record the
        // exact complement of the intended mask while still looking like a plausible picture.
        //
        // ⚠ +0.55 EV, the value M3.1b settled on and the luminance goldens reuse: at +1.0 the
        // brightest covered samples pin at 255 and the reference can no longer tell full coverage
        // from any greater value.
        "mask_colour_reds_across_zero",
        Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Colour(edit_model::ColourRange {
                    low: 300.0,
                    high: 60.0,
                    feather: 10.0,
                    min_chroma: 0.02,
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    )]
}

fn colour_mask_case(name: &str) -> Edits {
    colour_mask_cases()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no colour-mask case named {name}"))
        .1
}

#[test]
fn golden_mask_colour_reds_across_zero() {
    // ⚠ `hue_sweep_chart` rather than a new chart: it is built to exercise hue, HSL's goldens
    // already use it, and a colour mask and a hue rotation measured on one reference stay
    // directly comparable.
    check_golden_with(
        "mask_colour_reds_across_zero",
        &colour_mask_case("mask_colour_reds_across_zero"),
        hue_sweep_chart(),
    );
}

/// ⚠⚠ **Measured, not eyeballed** — clipping is invisible to the eye and fatal to a reference's
/// discriminating power, and this project has regenerated two mask goldens for exactly that. The
/// first three properties are [`the_luminance_mask_goldens_discriminate`]'s; the fourth is
/// specific to a band that wraps.
///
/// Measured on the committed reference: **0 samples pinned at either end**, 1232 of 3072 moved
/// and 1840 untouched — so the mask is unambiguously a selection, and every sample in it still
/// encodes what the mask did rather than what the encoder ran out of room for.
#[test]
fn the_colour_mask_golden_discriminates() {
    let neutral = to_rgb8(
        &build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(hue_sweep_chart()),
    );
    for (name, edits) in colour_mask_cases() {
        let bytes = to_rgb8(
            &build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .run(hue_sweep_chart()),
        );
        let hi = bytes.iter().filter(|&&c| c == 255).count();
        let lo = bytes.iter().filter(|&&c| c == 0).count();
        assert!(
            hi == 0 && lo == 0,
            "{name}: {hi} samples clipped high, {lo} clipped low — the golden cannot \
             discriminate where it is pinned"
        );
        let moved = bytes.iter().zip(&neutral).filter(|(a, b)| a != b).count();
        let total = bytes.len();
        assert!(
            moved * 8 >= total,
            "{name}: only {moved}/{total} samples differ from the unmasked render — too few to \
             witness a change"
        );
        // ⚠ "Enough is UNTOUCHED", not an upper bound on `moved`, for the reason the luminance
        // guard records: `moved` counts a one-level partial change the same as a full one, and
        // what distinguishes a selection from a global adjustment is that a real region comes
        // back byte-identical.
        let untouched = total - moved;
        assert!(
            untouched * 4 >= total,
            "{name}: only {untouched}/{total} samples are untouched — the mask is covering \
             everything, which a global adjustment would do too"
        );

        // ⚠⚠ **The wrap, asserted against the rendered reference and not only in a unit test.**
        // The chart sweeps hue left to right, so a band crossing 0° must move BOTH ends of it and
        // leave the middle alone. A sorted 60..300 band produces exactly the complement — the
        // middle moves and the ends do not — which every assertion above would still accept.
        let col_moved = |x: usize| {
            (0..H).any(|y| {
                let i = (y * W + x) * 3;
                bytes[i..i + 3] != neutral[i..i + 3]
            })
        };
        assert!(
            col_moved(0) && col_moved(W - 1),
            "{name}: a band across 0° must reach both ends of the hue sweep"
        );
        assert!(
            !col_moved(W / 2),
            "{name}: and must leave the far side of the wheel alone"
        );
        // ⚠ "Differs" is not "was lifted": a mask that darkened the reds would satisfy every
        // assertion above. The payload is +0.55 EV, so the direction is part of the property.
        let col_mean =
            |b: &[u8], x: usize| (0..H).map(|y| b[(y * W + x) * 3] as f32).sum::<f32>() / H as f32;
        for x in [0, W - 1] {
            assert!(
                col_mean(&bytes, x) > col_mean(&neutral, x) + 4.0,
                "{name}: column {x} must be LIFTED, not merely changed: {:.1} -> {:.1}",
                col_mean(&neutral, x),
                col_mean(&bytes, x)
            );
        }
    }
}

#[test]
fn golden_mask_curve() {
    // A lifted-midtone curve confined to the left half of the chart, so the reference carries
    // both the curve's shape and the coverage ramp across it.
    //
    // ⚠ The curve lifts rather than crushes, and stops at 0.62 rather than higher, so nothing
    // pins at 255: a clipped sample cannot tell this curve from a stronger one, which is the
    // weakness the radial exposure golden had to be re-authored to remove.
    check_golden(
        "mask_curve",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    tone_curve: edit_model::ToneCurve {
                        points: vec![[0.0, 0.0], [0.4, 0.62], [1.0, 1.0]],
                    },
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_grade() {
    // A split tone confined to a diagonal gradient: cool shadows, warm highlights, with
    // blending and balance both off their neutral so the reference covers the range
    // decomposition rather than only the wheels.
    check_golden(
        "mask_grade",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.1, 0.0],
                    to: [0.9, 1.0],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    color_grading: edit_model::ColorGrading {
                        shadows: edit_model::GradeWheel {
                            hue: 240.0,
                            saturation: 45.0,
                            luminance: -10.0,
                        },
                        highlights: edit_model::GradeWheel {
                            hue: 50.0,
                            saturation: 40.0,
                            luminance: 8.0,
                        },
                        blending: 65.0,
                        balance: 20.0,
                        ..edit_model::ColorGrading::default()
                    },
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_nr() {
    // Luminance and colour noise reduction confined to the left half, so one reference carries
    // the denoised and the untouched halves of the same noisy chart side by side.
    //
    // ⚠ `noise_chart`, not the plain chart: with no noise there is nothing to remove and the
    // reference could not discriminate this edit from a stronger one.
    check_golden_with(
        "mask_nr",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 60.0,
                        color: 60.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_mask_dehaze() {
    // Haze cleared from the left half only.
    //
    // ⚠⚠ The airlight is estimated from the **whole** chart — the untouched half still
    // contributes to it — which is the placement's whole point and is baked into this reference.
    check_golden_with(
        "mask_dehaze",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    dehaze: edit_model::Dehaze { amount: 70.0 },
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        },
        hazy_chart(),
    );
}

/// ⚠⚠ **Proof the masked capture stages actually reach the pixels**, which the two goldens above
/// cannot give on their own: a stage that never ran would produce a reference identical to a
/// neutral render of the same chart, and the golden test would then pass against it forever.
///
/// Compared against the *same chart with the mask removed*, so nothing but the mask can explain
/// the difference.
#[test]
fn a_masked_capture_correction_really_changes_the_chart() {
    let cases: [(&str, Edits, ImageBuf); 2] = [
        (
            "noise reduction",
            Edits {
                masks: vec![edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                        from: [0.0, 0.0],
                        to: [1.0, 0.0],
                        invert: true,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        noise_reduction: edit_model::NoiseReduction {
                            luminance: 60.0,
                            color: 60.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    disabled: false,
                    invert: false,
                }],
                ..Edits::default()
            },
            noise_chart(),
        ),
        (
            "dehaze",
            Edits {
                masks: vec![edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                        from: [0.0, 0.0],
                        to: [1.0, 0.0],
                        invert: true,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        dehaze: edit_model::Dehaze { amount: 70.0 },
                        ..Default::default()
                    },
                    disabled: false,
                    invert: false,
                }],
                ..Edits::default()
            },
            hazy_chart(),
        ),
    ];

    for (what, edits, chart) in cases {
        let masked = build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
            .run(chart.clone());
        let neutral = build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(chart);
        let a = to_rgb8(&masked);
        let b = to_rgb8(&neutral);
        let moved = a.iter().zip(&b).filter(|(x, y)| x != y).count();
        assert!(
            moved > a.len() / 20,
            "masked {what} moved only {moved} of {} samples — it may not be running at all",
            a.len()
        );
    }
}

// ─────────────────────────── M7.3b: one geometric resample, a reordered prefix ───────────────────────────

/// ⚠⚠ **The fused path proper**: keystone, angle and crop together, which before M7.3b was two
/// resamples. `scale: 0` for the same reason as `perspective_vertical`: mostly image, not black.
#[test]
fn golden_perspective_straighten() {
    check_golden_with(
        "perspective_straighten",
        &Edits {
            perspective: edit_model::Perspective {
                vertical: 45.0,
                scale: 0.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 5.0,
                crop: edit_model::CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
                ..Default::default()
            },
            ..Default::default()
        },
        grid_chart(),
    );
}

/// ⚠⚠ **The reorder**: noise reduction with a straighten. Since M7.3b NR runs on the source and
/// the rotation resamples the result; before, NR was handed rotated (interpolated) noise.
#[test]
fn golden_nr_straighten() {
    check_golden_with(
        "nr_straighten",
        &Edits {
            noise_reduction: NoiseReduction {
                luminance: 60.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 5.0,
                ..Default::default()
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

/// ⚠⚠ **The new anchor**: a spot under an active keystone. Since M7.3b the spot is placed in
/// pre-perspective space, so the keystone carries the healed patch with the content around it.
#[test]
fn golden_spot_perspective() {
    let mut e = spot_case("spot_heal");
    e.perspective = edit_model::Perspective {
        vertical: 40.0,
        scale: 0.0,
        ..Default::default()
    };
    check_golden_with("spot_perspective", &e, spot_chart());
}

/// ⚠ M5.1c: the parametric curve alone, shadows lifted and highlights pulled down — the shape its
/// sliders are most often used for. Every other golden has an inactive parametric curve, which is
/// what `an_inactive_parametric_curve_changes_no_table` pins at the table.
#[test]
fn golden_parametric_curve() {
    check_golden(
        "parametric_curve",
        &Edits {
            tone_curve_parametric: edit_model::ParametricCurve {
                shadows: 40.0,
                highlights: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        },
    );
}

/// ⚠ M10.11a: a film negative of the chart — its linear values as exposure 1..1000 through a film of
/// contrast 0.6 on an orange base — inverted back. The stage's own tests prove the inversion; this
/// pins the whole render, so a change anywhere between the step and the encode is seen.
#[test]
fn golden_film_negative() {
    const BASE: [f32; 3] = [0.85, 0.55, 0.30];
    let src = chart();
    let mut neg = ImageBuf::new(src.width(), src.height());
    for y in 0..src.height() {
        for x in 0..src.width() {
            let p = src.pixel(x, y);
            neg.set_pixel(
                x,
                y,
                std::array::from_fn(|c| {
                    let exposure = 1.0 + 999.0 * p[c].clamp(0.0, 1.0);
                    BASE[c] * 10f32.powf(-0.6 * exposure.log10())
                }),
            );
        }
    }
    check_golden_with(
        "film_negative",
        &Edits {
            negative: edit_model::Negative {
                on: true,
                base: BASE,
                gamma: 0.6,
                low: [0.0; 3],
                high: [1.8; 3],
                mono: false,
            },
            ..Edits::default()
        },
        neg,
    );
}

/// ⚠ M10.11b: grain at Amount 50 on the chart — pins the noise lattice, the tone weight and the
/// placement after sharpening; a change in the hash or the cell would move every grain.
#[test]
fn golden_grain() {
    check_golden(
        "grain",
        &Edits {
            grain: edit_model::Grain {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        },
    );
}

/// M10.16a: a depth map's band as coverage, built the way the app builds it — `DepthRange::coverage`
/// over the map's positions — handed to the tail through the brush side channel. A horizontal ramp
/// stands in for the network: the left edge the nearest point, the right the farthest.
fn depth_ramp_coverage(range: edit_model::DepthRange) -> std::sync::Arc<pipeline::BrushCoverage> {
    let map = pipeline::brush::CoverageMap {
        x0: 0,
        y0: 0,
        w: W,
        h: H,
        data: (0..W * H)
            .map(|i| {
                let position = (i % W) as f32 / (W - 1) as f32;
                (range.coverage(position) * 255.0).round() as u8
            })
            .collect(),
        outside: if range.invert { 255 } else { 0 },
    };
    std::sync::Arc::new(pipeline::BrushCoverage {
        maps: vec![vec![Some(std::sync::Arc::new(map))]],
        built: vec![vec![std::sync::Mutex::new(None)]],
    })
}

/// The chart through the whole pipeline with `coverage` in the side channel.
fn render_with_coverage(
    edits: &Edits,
    coverage: Option<&std::sync::Arc<pipeline::BrushCoverage>>,
) -> ImageBuf {
    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        RenderScale::FULL,
        None,
        coverage,
        pipeline::RenderSpec {
            resampler: RESAMPLER,
            ..Default::default()
        },
        None,
        TailRendering::colorimetric(),
    );
    tail.run(prefix.run(chart()))
}

#[test]
fn golden_depth_band_exposure() {
    // The far part of the scene, from 40 back, lifted a stop — the near left edge untouched.
    let range = edit_model::DepthRange {
        near: 40.0,
        far: 100.0,
        feather: 30.0,
        invert: false,
    };
    let edits = Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Depth(range),
            adjustments: edit_model::LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Edits::default()
    };
    let rendered = render_with_coverage(&edits, Some(&depth_ramp_coverage(range)));
    // ⚠ Shape (c): the reference must differ from the neutral render — on the far right — or a
    // band that reached no pixel would pin a neutral chart and pass forever. And the nearest edge,
    // outside the band, must be untouched.
    let neutral = render_with_coverage(&Edits::default(), None);
    let worst = |x0: usize, x1: usize| {
        (0..H)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| {
                (0..3)
                    .map(|c| (rendered.pixel(x, y)[c] - neutral.pixel(x, y)[c]).abs())
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max)
    };
    assert!(worst(W - 8, W) * 255.0 > 3.0, "the far band was not lifted");
    assert!(worst(0, 4) * 255.0 < 0.5, "the near edge moved");
    compare_or_update("depth_band_exposure", rendered);
}

/// M10.16a: an AI component in a mask's Dehaze — the prefix fix. The map's source is handed to
/// `build_prefix_steps` as the app hands its maps; here it covers the left half.
#[test]
fn golden_ai_masked_dehaze() {
    struct LeftHalf;
    impl pipeline::RasterSource for LeftHalf {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            pipeline::brush::CoverageMap {
                x0: 0,
                y0: 0,
                w,
                h,
                data: (0..w * h)
                    .map(|i| if i % w < w / 2 { 255 } else { 0 })
                    .collect(),
                outside: 0,
            }
        }
    }
    struct Maps;
    impl pipeline::AiRasters for Maps {
        fn source(
            &self,
            _: &edit_model::Mask,
        ) -> Option<(std::sync::Arc<dyn pipeline::RasterSource>, u64)> {
            Some((std::sync::Arc::new(LeftHalf), 1))
        }
    }
    let render = |edits: &Edits| {
        let input = hazy_chart();
        // The airlight definition that ships — `check_golden_with`'s reason.
        let airlight = pipeline::prefix_airlight(edits, &input, RESAMPLER, None);
        let img = pipeline::build_prefix_steps(
            edits,
            RenderScale::FULL,
            None,
            RESAMPLER,
            Some(airlight),
            Some(&Maps),
        )
        .iter()
        .fold(input, |img, s| s.stage.apply(img));
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            RenderScale::FULL,
            None,
            None,
            pipeline::RenderSpec {
                resampler: RESAMPLER,
                airlight: Some(airlight),
                ..Default::default()
            },
            None,
            TailRendering::colorimetric(),
        );
        tail.run(img)
    };
    let edits = Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 70.0 },
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Edits::default()
    };
    let rendered = render(&edits);
    // ⚠ Shape (c): against the same chart with the mask removed — the left half must change, the
    // right must not.
    let plain = render(&Edits::default());
    // ⚠ The hazy chart's own size (256 × 192), never `W × H` — this test's first run measured a
    // 64-pixel strip inside the covered half and called it "the uncovered half".
    let (cw, ch) = (rendered.width(), rendered.height());
    let worst = |x0: usize, x1: usize| {
        (0..ch)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| {
                (0..3)
                    .map(|c| (rendered.pixel(x, y)[c] - plain.pixel(x, y)[c]).abs())
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max)
    };
    assert!(
        worst(0, cw / 2 - 2) * 255.0 > 3.0,
        "the covered half was not dehazed"
    );
    assert!(
        worst(cw / 2 + 2, cw) * 255.0 < 0.5,
        "the uncovered half changed"
    );
    compare_or_update("ai_masked_dehaze", rendered);
}

/// M10.16d: Lens Blur's scene, 256 × 192 — a textured background whose depth ramps from 0.4 (left) to
/// 1.0 (right), a subject disc at depth 0.3, three point lights far behind it, and a near strip along
/// the bottom at depth 0.05 — with the planes the app would hand the step.
mod lens_blur_scene {
    use super::*;

    pub const W: usize = 256;
    pub const H: usize = 192;
    const SUBJECT: (f32, f32, f32) = (100.0, 88.0, 34.0);
    const LIGHTS: [(usize, usize); 3] = [(196, 36), (222, 70), (178, 104)];
    const NEAR_FROM: usize = 168;

    fn in_subject(x: usize, y: usize) -> bool {
        let (cx, cy, r) = SUBJECT;
        (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) <= r
    }

    pub fn scene() -> ImageBuf {
        let mut img = ImageBuf::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let px = if in_subject(x, y) {
                    [0.55, 0.18, 0.08]
                } else if y >= NEAR_FROM {
                    if (x / 6 + y / 6) % 2 == 0 {
                        [0.30, 0.32, 0.12]
                    } else {
                        [0.08, 0.10, 0.04]
                    }
                } else if LIGHTS
                    .iter()
                    .any(|&(lx, ly)| x.abs_diff(lx) <= 1 && y.abs_diff(ly) <= 1)
                {
                    [4.0, 3.6, 2.8]
                } else {
                    let stripe = if (x / 4) % 2 == 0 { 0.06 } else { 0.0 };
                    let v = 0.10 + 0.10 * y as f32 / H as f32 + stripe;
                    [v * 0.8, v * 0.9, v * 1.1]
                };
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn plane(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> pipeline::brush::CoverageMap {
        pipeline::brush::CoverageMap {
            x0: 0,
            y0: 0,
            w,
            h,
            data: (0..w * h)
                .map(|i| (f(i % w, i / w).clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect(),
            outside: 0,
        }
    }

    struct Depth;
    impl pipeline::RasterSource for Depth {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            plane(w, h, |x, y| {
                if in_subject(x, y) {
                    0.3
                } else if y >= NEAR_FROM {
                    0.05
                } else {
                    0.4 + 0.6 * x as f32 / (w - 1) as f32
                }
            })
        }
    }

    struct Subject;
    impl pipeline::RasterSource for Subject {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            plane(w, h, |x, y| if in_subject(x, y) { 1.0 } else { 0.0 })
        }
    }

    /// Both planes, and the focus the app would have measured.
    pub struct Maps(pub f32);
    impl pipeline::AiRasters for Maps {
        fn source(
            &self,
            _: &edit_model::Mask,
        ) -> Option<(std::sync::Arc<dyn pipeline::RasterSource>, u64)> {
            None
        }
        fn plane(
            &self,
            which: pipeline::MapPlane,
        ) -> Option<(std::sync::Arc<dyn pipeline::RasterSource>, u64)> {
            Some(match which {
                pipeline::MapPlane::Depth => (std::sync::Arc::new(Depth), 1),
                pipeline::MapPlane::Subject => (std::sync::Arc::new(Subject), 2),
            })
        }
        fn lens_focus(&self, _: &edit_model::LensBlur) -> Option<f32> {
            Some(self.0)
        }
    }

    /// The whole chain: the prefix with the planes, then the colorimetric tail.
    pub fn render(lb: edit_model::LensBlur, focus: f32) -> ImageBuf {
        let edits = Edits {
            lens_blur: lb,
            ..Edits::default()
        };
        let img = pipeline::build_prefix_steps(
            &edits,
            RenderScale::FULL,
            None,
            RESAMPLER,
            None,
            Some(&Maps(focus)),
        )
        .iter()
        .fold(scene(), |img, s| s.stage.apply(img));
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            &edits,
            RenderScale::FULL,
            None,
            None,
            pipeline::RenderSpec {
                resampler: RESAMPLER,
                ..Default::default()
            },
            None,
            TailRendering::colorimetric(),
        );
        tail.run(img)
    }

    /// Shape (c): the blurred render must differ from the unblurred one before it is pinned.
    pub fn check(name: &str, lb: edit_model::LensBlur, focus: f32) {
        let rendered = render(lb, focus);
        let plain = render(edit_model::LensBlur { on: false, ..lb }, focus);
        let worst = rendered
            .data()
            .iter()
            .zip(plain.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst * 255.0 > 20.0,
            "{name}: the blur changed nothing ({worst})"
        );
        compare_or_update(name, rendered);
    }
}

fn lens_blur(bokeh: edit_model::Bokeh, focus: edit_model::LensFocus) -> edit_model::LensBlur {
    edit_model::LensBlur {
        on: true,
        amount: 100.0,
        focus,
        focal_range: 10.0,
        bokeh,
        boost: 60.0,
    }
}

/// M10.16d: Subject focus, round discs — the subject crisp, the lights round discs behind it, the
/// near strip blurred in front. ⚠ The strip does not reach the disc: a foreground spreading over the
/// subject is the gather's own test (`a_foreground_spreads_over_the_subject`).
#[test]
fn golden_lens_blur_subject_circle() {
    lens_blur_scene::check(
        "lens_blur_subject_circle",
        lens_blur(edit_model::Bokeh::Circle, edit_model::LensFocus::Subject),
        0.3,
    );
}

/// M10.16d: Cat-eye — the lights' discs narrowed radially, more toward the frame's right edge.
#[test]
fn golden_lens_blur_subject_cateye() {
    lens_blur_scene::check(
        "lens_blur_subject_cateye",
        lens_blur(edit_model::Bokeh::CatEye, edit_model::LensFocus::Subject),
        0.3,
    );
}

/// M10.16d: Point focus mid-background (depth 0.7) — no subject layer: the disc and the near strip
/// blur, the background around 0.7 stays sharp.
#[test]
fn golden_lens_blur_point() {
    lens_blur_scene::check(
        "lens_blur_point",
        lens_blur(
            edit_model::Bokeh::Circle,
            edit_model::LensFocus::Point { point: [0.7, 0.4] },
        ),
        0.7,
    );
}

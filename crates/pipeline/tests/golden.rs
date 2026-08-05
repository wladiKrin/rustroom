//! Golden-image regression tests. Set `UPDATE_GOLDEN=1` to (re)write references.
//!
//! A fixed synthetic "chart" is rendered through `build_pipeline(&edits)`; the RGB8
//! output must match the committed PNG within `TOL` per channel.

use core_image::ImageBuf;
use edit_model::{Edits, ToneCurve, WhiteBalance};
use pipeline::build_pipeline;
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
    let rendered = build_pipeline(edits).run(input);
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

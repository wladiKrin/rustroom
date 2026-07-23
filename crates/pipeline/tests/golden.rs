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
    let rendered = build_pipeline(edits).run(chart());
    let bytes = to_rgb8(&rendered);
    let path = golden_path(name);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &bytes, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
        return;
    }

    let golden = image::open(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with UPDATE_GOLDEN=1"))
        .to_rgb8();
    assert_eq!(golden.dimensions(), (W as u32, H as u32), "{name}: size");
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

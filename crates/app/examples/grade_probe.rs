//! Measure what colour grading actually does to a real photograph, bucketed by lightness.
//!
//! ⚠ A whole-image mean is the wrong statistic here and understates the effect badly — the
//! mistake the HueSatMap work nearly drew a wrong conclusion from. A split tone pushes shadows
//! one way and highlights the other, so the two largely cancel in a global average. Bucketing
//! by lightness is what makes the effect legible, and it is also what checks the *direction*:
//! shadows must go blue, highlights warm, midtones barely move.
//!
//! `cargo run --release -p app --example grade_probe <raw>`
use rustroom_app::render_engine::render_once;
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: grade_probe <raw>"));
    let decoded =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let buf = decoded.image;

    let neutral = edit_model::Edits::default();
    let graded = edit_model::Edits {
        color_grading: edit_model::ColorGrading {
            shadows: edit_model::GradeWheel {
                hue: 240.0,
                saturation: 70.0,
                luminance: 0.0,
            },
            highlights: edit_model::GradeWheel {
                hue: 45.0,
                saturation: 70.0,
                luminance: 0.0,
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let a = render_once(
        &buf,
        &neutral,
        1600,
        0.5,
        color::OutputSpace::Srgb,
        pipeline::Resampler::default(),
    );
    let b = render_once(
        &buf,
        &graded,
        1600,
        0.5,
        color::OutputSpace::Srgb,
        pipeline::Resampler::default(),
    );

    // (name, lower bound) over the 8-bit mean of the *neutral* render.
    let bands = [("shadow", 0u32), ("mid", 70), ("high", 170)];
    let mut sum = [0i64; 3];
    let mut n = [0usize; 3];

    for (p, q) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
    {
        let l = (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3;
        let k = bands.iter().rposition(|&(_, lo)| l >= lo).unwrap();
        // (R - B) shift: positive = warmer, negative = cooler.
        let warm = (q[0] as i32 - q[2] as i32) - (p[0] as i32 - p[2] as i32);
        sum[k] += warm as i64;
        n[k] += 1;
    }

    for k in 0..3 {
        if n[k] == 0 {
            println!("{:>6}: empty", bands[k].0);
            continue;
        }
        println!(
            "{:>6}: n={:<8} mean_warm_shift={:+.2}",
            bands[k].0,
            n[k],
            sum[k] as f64 / n[k] as f64
        );
    }
}

//! With the airlight now constant, does the dehaze STRENGTH still move as you straighten?
//!
//! ⚠ Reported in use 2026-09-17, after the airlight was made invariant: *"it does not breathe for
//! the masked one, but it does slightly for the global one, especially for straighten values above
//! 25 or below -25."* The airlight itself measures **0.00 of 255 levels across ±40°**, so whatever
//! moves is not the airlight.
//!
//! `transmission_with` is entirely local — a per-pixel normalise, a `min_filter`, and a guided
//! filter of radius 60 — so in principle rotation should only change the result near the empty
//! corners a rotation introduces. **This measures whether that is true.**
//!
//! At each angle it renders the prefix twice, with dehaze at 60 and at 0, and reports the mean
//! absolute difference over a **centred disc** that stays inside the frame at every angle. That
//! isolates the dehaze's own contribution from the rotation of the pixels underneath it.
//!
//! ```text
//! cargo run --release -p app --example probe_dehaze_stability <raw>
//! ```

use edit_model::{Dehaze, Edits};
use pipeline::{RenderScale, Resampler};

const ANGLES: [f32; 13] = [
    0.0, 5.0, 10.0, 15.0, 20.0, 23.0, 25.0, 27.0, 30.0, 33.0, 35.0, 38.0, 40.0,
];

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_dehaze_stability <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let buffer = d.image;
    let scale = RenderScale::new(1.0);
    eprintln!(
        "buffer {}x{} ({:.1} Mpx)",
        buffer.width(),
        buffer.height(),
        (buffer.width() * buffer.height()) as f64 / 1e6
    );

    // ⚠⚠ **The FULL pipeline, not just the prefix.** The first version of this probe ran only
    // `build_prefix_steps` and found the interior strength flat — but the reporter sees the whole
    // render, and the tail (exposure, the tone curve, the output transform) sits downstream of
    // dehaze and can amplify a small prefix change into a visible one. A probe that stops before
    // the tail is measuring a different picture from the one on screen.
    let as_shot = d.metadata.as_shot.unwrap_or_default();
    let run = |angle: f32, amount: f32| {
        let mut e = Edits::default();
        e.geometry.angle = angle;
        e.dehaze = Dehaze { amount };
        let air = pipeline::prefix_airlight(&e, &buffer, Resampler::default(), None);
        let spec = pipeline::RenderSpec {
            space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: Resampler::default(),
            baseline: as_shot,
            airlight: Some(air),
        };
        pipeline::build_pipeline_spec(&e, scale, spec, pipeline::TailRendering::colorimetric())
            .run(buffer.clone())
    };

    // ⚠ A disc of radius 0.30 * min(w,h) about the centre. A rotation of 40 degrees keeps a disc of
    // radius 0.5 * min(w,h) inside the frame, so 0.30 is comfortably clear of the empty corners —
    // which is the point: if the strength moves HERE, it is not an edge effect.
    let (w, h) = (buffer.width(), buffer.height());
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);

    // ⚠⚠ THREE ZONES, because the first version of this probe measured only the centre and found
    // the strength flat — which ruled out an interior effect and said nothing about the edges. A
    // rotation introduces EMPTY CORNERS, and dehaze's reach is PATCH + 2*REFINE = 140 px at full
    // scale, so the corners can only touch pixels within that distance of them. That is an EDGE
    // effect by construction, and it grows with the angle.
    let zones: [(&str, f32, f32); 3] = [
        ("centre  0.00-0.30", 0.00, 0.30),
        ("mid     0.30-0.42", 0.30, 0.42),
        ("outer   0.42-0.50", 0.42, 0.50),
    ];
    let m = w.min(h) as f32;

    println!("\nmean |dehazed - plain| in 255ths, by angle and zone:");
    println!("  angle |  centre   mid     outer");
    let mut base = [0.0f64; 3];
    for (i, angle) in ANGLES.iter().enumerate() {
        let hazy = run(*angle, 0.0);
        let dehazed = run(*angle, 60.0);
        let mut sum = [0.0f64; 3];
        let mut n = [0u64; 3];
        for y in 0..h {
            for x in 0..w {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let dist = (dx * dx + dy * dy).sqrt() / m;
                let Some(z) = zones
                    .iter()
                    .position(|(_, lo, hi)| dist >= *lo && dist < *hi)
                else {
                    continue;
                };
                let a = hazy.pixel(x, y);
                let b = dehazed.pixel(x, y);
                for c in 0..3 {
                    sum[z] += (a[c] - b[c]).abs() as f64;
                }
                n[z] += 3;
            }
        }
        let mean: Vec<f64> = (0..3).map(|z| sum[z] / n[z] as f64 * 255.0).collect();
        if i == 0 {
            base.copy_from_slice(&mean[..3]);
        }
        println!(
            "  {angle:5.1} | {:7.3} {:7.3} {:7.3}   (vs 0 deg: {:+6.3} {:+6.3} {:+6.3})",
            mean[0],
            mean[1],
            mean[2],
            mean[0] - base[0],
            mean[1] - base[1],
            mean[2] - base[2]
        );
    }
    println!(
        "\n⚠⚠ MEASURED ON DSCF6291 AND IT REPRODUCES NOTHING. Through the full pipeline every zone\n\
         is flat to within 0.14 of a level across 0-40 deg, and there is no knee at 25 where the\n\
         reporter put one. The airlight itself is EXACTLY invariant over the same sweep.\n\
         ⚠ So either the photograph matters — the airlight spreads ranged 0.05 to 6.8 levels across\n\
         three frames, so scene dependence here is likely — or the effect is in something this does\n\
         not run: the interactive RUNG (scaled_radius makes dehaze's radii rung-dependent, so a\n\
         dragging frame and a settled frame compute different transmission), or the GPU twin rather\n\
         than this CPU path. **Run it against the frame that actually shows it before believing any\n\
         of those.**"
    );
}

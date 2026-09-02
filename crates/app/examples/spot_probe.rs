//! Measure a spot on a real RAW frame.
//!
//! ```text
//! cargo run --release -p app --example spot_probe <raw> [cx cy sx sy radius]
//! ```
//!
//! Coordinates are normalised over the **pre-geometry** frame, exactly as `Spot` stores them.
//! Prints the mean RGB over the destination disc, the ring around it and the source disc, for
//! the neutral render and for each mode — the evidence a screenshot cannot give.
//!
//! ⚠ It runs the prefix only, and deliberately: the output transform would encode the numbers
//! into sRGB and a linear-light offset would stop being readable as one.

use core_image::ImageBuf;
use edit_model::{Edits, Spot, SpotMode};
use pipeline::{build_prefix_steps, RenderScale, Resampler};

fn mean(img: &ImageBuf, cx: f32, cy: f32, lo: f32, hi: f32) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    let mut n = 0usize;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d > lo && d <= hi {
                let p = img.pixel(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as f64;
                }
                n += 1;
            }
        }
    }
    if n == 0 {
        return [f32::NAN; 3];
    }
    [
        (sum[0] / n as f64) as f32,
        (sum[1] / n as f64) as f32,
        (sum[2] / n as f64) as f32,
    ]
}

fn run(edits: &Edits, src: &ImageBuf) -> ImageBuf {
    let mut img = src.clone();
    for step in build_prefix_steps(edits, RenderScale::FULL, None, Resampler::default()) {
        img = step.stage.apply(img);
    }
    img
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: spot_probe <raw> [cx cy sx sy radius]  (normalised, 0..1)");
        std::process::exit(2);
    };
    let num = |i: usize, d: f32| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (cx_n, cy_n) = (num(2, 0.5), num(3, 0.2));
    let (sx_n, sy_n) = (num(4, 0.6), num(5, 0.25));
    let radius = num(6, 0.01);

    let decoded = raw_decode::decode_raw(
        std::path::Path::new(path),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
    let img = decoded.image;
    println!("decoded {}x{}", img.width(), img.height());

    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (cx_n * (w - 1.0), cy_n * (h - 1.0));
    let (sx, sy) = (sx_n * (w - 1.0), sy_n * (h - 1.0));
    let r = radius * w.max(h);
    println!("destination ({cx:.0},{cy:.0})  source ({sx:.0},{sy:.0})  radius {r:.1} px");

    let spot = |mode| Edits {
        spots: vec![Spot {
            centre: [cx_n, cy_n],
            source: [sx_n, sy_n],
            radius,
            feather: 40.0,
            mode,
        }],
        ..Edits::default()
    };

    let neutral = run(&Edits::default(), &img);
    let report = |label: &str, out: &ImageBuf| {
        let disc = mean(out, cx, cy, -1.0, r * 0.6);
        let ring = mean(out, cx, cy, r, r * 1.5);
        let src = mean(out, sx, sy, -1.0, r * 0.6);
        println!(
            "{label:>8}  disc [{:.4} {:.4} {:.4}]  ring [{:.4} {:.4} {:.4}]  source [{:.4} {:.4} {:.4}]",
            disc[0], disc[1], disc[2], ring[0], ring[1], ring[2], src[0], src[1], src[2]
        );
    };
    report("neutral", &neutral);
    report("clone", &run(&spot(SpotMode::Clone), &img));
    report("heal", &run(&spot(SpotMode::Heal), &img));
}

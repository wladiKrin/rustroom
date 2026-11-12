//! What do HSL and Color Grading cost together — M10.14f's "two Oklab round trips", re-priced?
//!
//! `cargo run --release -p app --example bench_oklab_pair <raw>`
//!
//! Both stages convert to Oklab and back; fused they would convert once. This times a tail-only drag (a slider that
//! re-runs only the tail) with HSL alone, grading alone, and both, GPU tail on and off — the mean of 10 frames after
//! 2 warm ones, `RenderEngine` on the full decode at the 3200 rung.

use edit_model::Edits;
use rustroom_app::render_engine::{FrameSpec, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

fn frame_ms(engine: &RenderEngine, edits: &Edits, spec: FrameSpec) -> f64 {
    let t = Instant::now();
    engine.request(edits.clone(), spec);
    loop {
        if engine.poll_latest().is_some() {
            return t.elapsed().as_secs_f64() * 1000.0;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_oklab_pair <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let engine = RenderEngine::new(
        Arc::new(d.image),
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        Default::default(),
        true,
        || {},
    );
    let mut hsl = Edits::default();
    for (i, b) in hsl.hsl.bands.iter_mut().enumerate() {
        b.hue = (i as f32 * 7.0) % 30.0 - 15.0;
        b.saturation = 20.0;
        b.luminance = -10.0;
    }
    let mut grade = Edits::default();
    grade.color_grading.shadows.hue = 220.0;
    grade.color_grading.shadows.saturation = 30.0;
    grade.color_grading.highlights.hue = 40.0;
    grade.color_grading.highlights.saturation = 25.0;
    let both = Edits {
        color_grading: grade.color_grading,
        ..hsl.clone()
    };
    for gpu in [false, true] {
        let spec = FrameSpec {
            max_dim: 3200,
            region: None,
            display_space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: pipeline::Resampler::default(),
            gpu_tail: gpu,
            grade_view: None,
            soft_proof: None,
            visualize: None,
        };
        for (label, e) in [("HSL", &hsl), ("grading", &grade), ("both", &both)] {
            let mut sum = 0.0;
            for i in 0..12 {
                let mut f = e.clone();
                f.exposure = 0.01 * i as f32;
                let ms = frame_ms(&engine, &f, spec);
                if i >= 2 {
                    sum += ms;
                }
            }
            println!(
                "{:<4} {label:<8} {:7.2} ms a tail frame",
                if gpu { "GPU" } else { "CPU" },
                sum / 10.0
            );
        }
    }
}

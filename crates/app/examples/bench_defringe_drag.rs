//! What does a straighten drag cost with Defringe on (M10.14f)? Geometry precedes the defringe step in the prefix,
//! so every drag frame re-runs it — on the CPU before the GPU twin, which also ended the resident handoff.
//!
//! `cargo run --release -p app --example bench_defringe_drag <raw>`
//!
//! ⚠ `bench_mask_reference`'s method: `RenderEngine` itself on the preview decode, the GPU tail on; each frame a new
//! angle, as a drag makes; the mean of 10 frames after 2 warm ones.

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
        .expect("usage: bench_defringe_drag <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    println!("buffer {}x{}", d.image.width(), d.image.height());
    match gpu_pipeline::shared() {
        Some(c) => println!("adapter: {} ({})\n", c.adapter_name(), c.backend()),
        None => println!("no GPU adapter\n"),
    }
    let engine = RenderEngine::new(
        Arc::new(d.image),
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        Default::default(),
        true,
        || {},
    );
    let base = Edits {
        exposure: 0.3,
        rendering: Some(edit_model::Rendering::Camera),
        ..Edits::default()
    };
    let fringe = Edits {
        defringe: edit_model::Defringe {
            purple_amount: 60.0,
            green_amount: 40.0,
            ..Default::default()
        },
        ..base.clone()
    };
    for &max_dim in &[1600usize, 3200] {
        let spec = FrameSpec {
            max_dim,
            region: None,
            display_space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: pipeline::Resampler::default(),
            gpu_tail: true,
            grade_view: None,
            soft_proof: None,
            visualize: None,
        };
        println!("── rung {max_dim}");
        for (label, e) in [("defringe off", &base), ("defringe on", &fringe)] {
            let mut sum = 0.0;
            for i in 0..12 {
                let mut f = e.clone();
                f.geometry.angle = 0.5 + i as f32 * 0.17;
                let ms = frame_ms(&engine, &f, spec);
                if i >= 2 {
                    sum += ms;
                }
            }
            println!("  {label:<14} {:8.2} ms a drag frame", sum / 10.0);
        }
    }
}

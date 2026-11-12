//! What does a paint frame cost now — M10.14f's "incremental splatting" question, re-priced?
//!
//! `cargo run --release -p app --example bench_brush_paint <raw>`
//!
//! A brush mask with a look slider (Saturation — a scene slider would be an offset, no mask stage), its stroke growing
//! by one point a frame as a drag does, rendered through `RenderEngine` at native resolution over a 1:1 viewport
//! (`region`), GPU tail on and off. The incremental-splatting entry recorded ~16.6 ms a frame at 1:1 on the CPU in
//! 2026-09 (9.0 ms of it rasterisation). Constructed stroke; the decode is the author's photo.

use edit_model::{Brush, BrushStroke, Edits, LocalAdjustments, Mask, MaskedAdjustment};
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
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
        .expect("usage: bench_brush_paint <raw>");
    let opts = raw_decode::DecodeOptions::default();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    println!("buffer {bw}x{bh}");
    if let Some(c) = gpu_pipeline::shared() {
        println!("adapter: {} ({})", c.adapter_name(), c.backend());
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
    // A 1:1 viewport in the middle of the frame, as the canvas asks at native zoom.
    let (vw, vh) = (1600usize.min(bw), 1000usize.min(bh));
    let region = RegionRect {
        x: (bw - vw) / 2,
        y: (bh - vh) / 2,
        w: vw,
        h: vh,
    };
    for (label, auto_mask) in [("plain brush", false), ("auto-mask brush", true)] {
        for gpu in [false, true] {
            let spec = FrameSpec {
                max_dim: bw.max(bh),
                region: Some(region),
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: gpu,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            let mut stroke = BrushStroke {
                points: vec![[0.42, 0.5]],
                radius: 0.02,
                hardness: 40.0,
                auto_mask,
                decode: false,
                ..Default::default()
            };
            let (mut sum, mut n, mut worst) = (0.0, 0, 0f64);
            let total: usize = std::env::var("POINTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40);
            for i in 0..total {
                let t = i as f32 / total as f32;
                stroke
                    .points
                    .push([0.42 + t * 0.16, 0.5 + (i as f32 * 0.3).sin() * 0.02]);
                let e = Edits {
                    masks: vec![MaskedAdjustment {
                        mask: Mask::Brush(Brush {
                            strokes: vec![stroke.clone()],
                            invert: false,
                        }),
                        adjustments: LocalAdjustments {
                            saturation: 40.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                    ..Edits::default()
                };
                let ms = frame_ms(&engine, &e, spec);
                // The last ten frames: what a long stroke's frame costs.
                if i + 10 >= total {
                    sum += ms;
                    n += 1;
                    worst = worst.max(ms);
                }
            }
            println!(
                "{label:<16} {:<4}  {:6.2} ms a paint frame (worst {worst:.2}), the last 10 frames",
                if gpu { "GPU" } else { "CPU" },
                sum / n as f64
            );
        }
    }
}

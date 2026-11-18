//! What does M10.12b2's **reference photo** cost on a real frame, through the app's own render path?
//!
//! A mask that reads the photograph (a luminance or colour range, a gating brush) and carries a scene
//! offset selects on the globally edited photo: `SceneOffsetsStage` runs the whole global look a
//! second time, on a copy, and both it and that mask's look stage stay on the CPU (the GPU does not
//! compute the reference). These rows price that against the cases that pay nothing extra.
//!
//! `cargo run --release -p app --example bench_mask_reference <raw>`
//!
//! ⚠ `bench_gpu_tail`'s method: `RenderEngine` itself — request, poll, the real prefix cache — on the
//! preview decode, best of 5 warm frames, CPU tail and GPU tail alternated.

use edit_model::{Edits, LocalAdjustments, Mask, MaskedAdjustment};
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

/// Best of `n` warm rounds — round 0 compiles shaders and fills the prefix cache.
fn best(engine: &RenderEngine, edits: &Edits, spec: FrameSpec, n: usize) -> f64 {
    let mut b = f64::MAX;
    for r in 0..n {
        let ms = frame_ms(engine, edits, spec);
        if r > 0 {
            b = b.min(ms);
        }
    }
    b
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_mask_reference <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    println!("buffer {bw}x{bh}");
    match gpu_pipeline::shared() {
        Some(c) => println!("adapter: {} ({})\n", c.adapter_name(), c.backend()),
        None => println!("no GPU adapter — the gpu column is the CPU again\n"),
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

    // ⚠ `bench_gpu_tail`'s "typical" — the author's own sidecars' commonest sliders and the camera
    // rendering — plus a curve and saturation, so the reference runs a realistic global look.
    let global = Edits {
        exposure: 0.45,
        whites: 25.0,
        blacks: -20.0,
        saturation: 15.0,
        tone_curve: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [0.25, 0.2], [0.75, 0.82], [1.0, 1.0]],
        },
        dehaze: edit_model::Dehaze { amount: 35.0 },
        rendering: Some(edit_model::Rendering::Camera),
        ..Edits::default()
    };
    let with = |mask: Mask, a: LocalAdjustments| Edits {
        masks: vec![MaskedAdjustment {
            mask,
            adjustments: a,
            ..Default::default()
        }],
        ..global.clone()
    };
    let range = || Mask::Luminance(edit_model::LuminanceRange::default());
    let linear = || Mask::Linear(Default::default());
    let sat = LocalAdjustments {
        saturation: 40.0,
        ..Default::default()
    };
    let ev = LocalAdjustments {
        exposure: 0.5,
        ..Default::default()
    };
    let ev_sat = LocalAdjustments {
        exposure: 0.5,
        saturation: 40.0,
        ..Default::default()
    };
    let cases: Vec<(&str, Edits)> = vec![
        ("global only", global.clone()),
        ("+ range, Saturation (no offset)", with(range(), sat)),
        (
            "+ linear, Exposure (offsets only)",
            with(linear(), ev.clone()),
        ),
        ("+ range, Exposure (reference)", with(range(), ev)),
        ("+ range, Exposure + Saturation", with(range(), ev_sat)),
    ];

    for &max_dim in &[1600usize, 3200] {
        println!("── rung {max_dim}");
        println!("  case                                  cpu ms     gpu ms");
        for (label, edits) in &cases {
            let off = FrameSpec {
                max_dim,
                region: None,
                display_space: color::Display::SRGB,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            let on = FrameSpec {
                gpu_tail: true,
                ..off
            };
            let c = best(&engine, edits, off, 5);
            let g = best(&engine, edits, on, 5);
            println!("  {label:<36} {c:8.2}   {g:8.2}");
        }
        println!();
    }
}

//! What does `gpu_tail` save on a **real frame**, through the app's own render path?
//!
//! ⚠⚠ **Written because `gpu_run_bench` and the app disagree.** That bench times a bare tail on a
//! 1600×1068 synthetic ramp and reports 2.05×; a user with the toggle on could not see a
//! difference. The lesson this repo already records is *a synthetic bench prices a stage, not a
//! session* — the interactive-rung work found `bench_masked` understating the app by **2.6×**,
//! because a real frame carries the prefix, the pack, the upload and a whole tail rather than
//! three stages.
//!
//! So this drives `RenderEngine` itself — request, poll, the real `render_cached`, the real prefix
//! cache, the real `pack` — on a real RAW, at the rungs the app actually uses.
//!
//! `cargo run --release -p app --example bench_gpu_tail <raw>`

use edit_model::{ColorGrading, Edits, GradeWheel, Sharpening};
use rustroom_app::render_engine::{FrameSpec, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

fn graded() -> ColorGrading {
    ColorGrading {
        shadows: GradeWheel {
            hue: 250.0,
            saturation: 60.0,
            luminance: -20.0,
        },
        highlights: GradeWheel {
            hue: 150.0,
            saturation: 45.0,
            luminance: 15.0,
        },
        blending: 35.0,
        balance: -25.0,
        ..Default::default()
    }
}

fn hsl() -> edit_model::Hsl {
    let mut h = edit_model::Hsl::default();
    for (i, b) in h.bands.iter_mut().enumerate() {
        b.saturation = (i as f32 * 21.0) % 90.0 - 45.0;
        b.hue = (i as f32 * 13.0) % 70.0 - 35.0;
    }
    h
}

fn sharp() -> Sharpening {
    Sharpening {
        amount: 70.0,
        radius: 1.5,
        detail: 50.0,
        masking: 40.0,
    }
}

/// One render through the engine, waited for. Returns the wall time of the whole frame.
///
/// ⚠ The engine coalesces, so a request must be **polled to completion** before the next is
/// issued or the measurement times a frame that was skipped.
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

/// Best of `n` **warm** rounds — round 0 is discarded because the first encode compiles shaders.
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
        .expect("usage: bench_gpu_tail <raw>");
    let path = std::path::Path::new(&path);

    // ⚠ `preview()` — the half-size decode the app opens with, not a full one. Timing the full
    // decode's buffer would measure a rung the app only reaches after the handover.
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    let buffer = Arc::new(d.image);
    println!("buffer {bw}x{bh}, source_scale {}", opts.source_scale());
    match gpu_pipeline::shared() {
        Some(c) => println!("adapter: {} ({})\n", c.adapter_name(), c.backend()),
        None => {
            println!("no GPU adapter — nothing to compare");
            return;
        }
    }

    let engine = RenderEngine::new(
        buffer,
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        Default::default(),
        true,
        || {},
    );

    let mut three = Edits {
        hsl: hsl(),
        sharpening: sharp(),
        ..Edits::default()
    };
    three.color_grading = graded();
    let hsl_only = Edits {
        hsl: hsl(),
        ..Edits::default()
    };

    // ⚠⚠ **The case that actually matters, taken from the user's own 40 sidecars rather than
    // invented.** Dehaze is active on 100% of them, exposure on 57%, blacks 55%, whites 50% —
    // and the three accelerated stages on 5%, 2.5% and 2.5%. A bench that only measures the
    // stages we ported measures a photograph nobody here edits.
    let typical = Edits {
        exposure: 0.45,
        whites: 25.0,
        blacks: -20.0,
        dehaze: edit_model::Dehaze { amount: 35.0 },
        ..Edits::default()
    };

    let cases: [(&str, &Edits); 4] = [
        ("TYPICAL of this catalogue", &typical),
        ("neutral (no GPU stage at all)", &Edits::default()),
        ("hsl only", &hsl_only),
        ("hsl + grade + sharpening", &three),
    ];

    // ⚠⚠ Two rungs, because they are different questions. **1600 is the fit texture**; **3200 is
    // what a fitted photo actually renders at on a large HiDPI window** — the interactive-rung
    // work measured exactly that, 6.8 Mpx against the fit rung's 1.7, and it is the rung this
    // user sees with `drag_quality: Full`.
    for &max_dim in &[1600usize, 3200] {
        let mpx = {
            let long = bw.max(bh) as f64;
            let k = (max_dim as f64 / long).min(1.0);
            (bw as f64 * k) * (bh as f64 * k) / 1.0e6
        };
        println!("── rung {max_dim} — whole image, ~{mpx:.1} Mpx");
        println!("  case                              cpu        gpu     speed-up   saved");
        for (label, edits) in cases {
            let off = FrameSpec {
                max_dim,
                region: None,
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                // ⚠ The three view modes off: each adds work of its own (the grade range view is
                // ~9 ms at fit) and would blur what is being measured.
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            let on = FrameSpec {
                gpu_tail: true,
                ..off
            };
            // ⚠ Warm each configuration's prefix cache *and* its shaders before timing, then
            // alternate — a cold prefix would swamp a tail difference entirely.
            let c = best(&engine, edits, off, 5);
            let g = best(&engine, edits, on, 5);
            println!(
                "  {label:<32} {c:7.2} ms {g:7.2} ms   {:5.2}x  {:+6.2} ms",
                c / g,
                c - g
            );
        }
        println!();
    }

    println!(
        "⚠ These are WHOLE FRAMES through the real engine — prefix (cached), tail, pack.\n\
         ⚠ `gpu_run_bench` times the bare tail on a synthetic ramp and will read higher."
    );
}

//! Why panning is slower while AI denoise runs — the author on M10.16b5 (2026-10-16): *"panning and
//! zooming is noticeably slower"* during a job. Pans a 1:1 view through the real `RenderEngine` under
//! five conditions and prints the frames' times:
//!
//! - **A** Amount 0 — no denoise at all;
//! - **B** a result installed and left alone — the finished state;
//! - **C** B with a new result installed every second, as the job's partials are, and no job running;
//! - **D** the real job running, its partials NOT installed — the job's GPU and CPU work alone;
//! - **E** the real job with its partials installed as the app does — what the author saw.
//!
//! Then the fitted view, where a new result costs a fresh downscale of the full photo.
//!
//! Run: `cargo run --release --example bench_denoise_pan -- <models dir> <raw> [--ffdnet]` — the directory
//! holds `drunet_color.onnx`, or `ffdnet_color.pth` with `--ffdnet` (M10.16b6). `RUSTROOM_DEBUG_RENDER=1` adds each render's own line.
use core_image::ImageBuf;
use edit_model::{AiDenoise, Edits};
use rustroom_app::ai_denoise::TileNet;
use rustroom_app::denoise_worker::{DenoiseRequest, DenoiseResult, DenoiseWorker};
use rustroom_app::denoised::{self, Denoised};
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frames a pan, and how far the view moves between two.
const FRAMES: usize = 40;
const STEP: usize = 48;
/// The job's pans: long enough for several partial results.
const LONG: usize = 160;
/// A 1:1 view on the author's 2560 × 1440 display.
const VIEW: (usize, usize) = (2400, 1400);

fn frame_ms(engine: &RenderEngine, edits: &Edits, spec: FrameSpec) -> f64 {
    let t = Instant::now();
    engine.request(edits.clone(), spec);
    loop {
        if engine.poll_latest().is_some() {
            return t.elapsed().as_secs_f64() * 1000.0;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn spec(max_dim: usize, region: Option<RegionRect>) -> FrameSpec {
    FrameSpec {
        max_dim,
        region,
        display_space: color::OutputSpace::Srgb,
        intent: color::RenderingIntent::default(),
        resampler: pipeline::Resampler::default(),
        gpu_tail: true,
        grade_view: None,
        soft_proof: None,
        visualize: None,
    }
}

/// One pan at the native rung, `between` run before each frame.
fn pan(
    frames: usize,
    engine: &RenderEngine,
    edits: &Edits,
    size: (usize, usize),
    mut between: impl FnMut(),
) -> Vec<f64> {
    let (bw, bh) = size;
    let (w, h) = (VIEW.0.min(bw), VIEW.1.min(bh));
    let y = (bh - h) / 2;
    (0..frames)
        .map(|k| {
            between();
            let x = (bw / 8 + k * STEP).min(bw - w);
            frame_ms(
                engine,
                edits,
                spec(bw.max(bh), Some(RegionRect { x, y, w, h })),
            )
        })
        .collect()
}

fn report(label: &str, v: &[f64]) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "{label:<52} min {:7.1}  median {:7.1}  p90 {:7.1}  max {:7.1} ms",
        s[0],
        s[s.len() / 2],
        s[s.len() * 9 / 10],
        s[s.len() - 1]
    );
}

fn install(engine: &RenderEngine, id: u64, image: &Arc<ImageBuf>) {
    engine
        .ai_maps()
        .write()
        .expect("ai maps")
        .set_denoised(Some((Arc::new(Denoised::new(Arc::clone(image))), id)));
}

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: bench_denoise_pan <models dir> <raw> [--ffdnet]";
    let models = PathBuf::from(args.next().expect(usage));
    let path = PathBuf::from(args.next().expect(usage));
    let ffdnet = args.next().as_deref() == Some("--ffdnet");
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(&path, &opts).expect("decode");
    let buffer = Arc::new(d.image);
    let size = (buffer.width(), buffer.height());
    println!(
        "buffer {}x{}, view {}x{}, {FRAMES} frames {STEP} px apart",
        size.0, size.1, VIEW.0, VIEW.1
    );
    let engine = RenderEngine::new(
        Arc::clone(&buffer),
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        Default::default(),
        true,
        || {},
    );
    let on = Edits {
        ai_denoise: AiDenoise { amount: 60.0 },
        exposure: 0.35,
        contrast: 20.0,
        ..Edits::default()
    };
    let off = Edits {
        ai_denoise: AiDenoise::default(),
        ..on.clone()
    };

    // The decode stands in as the result: its pixels do not change what a frame costs.
    report("A  Amount 0", &pan(FRAMES, &engine, &off, size, || {}));
    install(&engine, 1, &buffer);
    report(
        "B  a result, left alone",
        &pan(FRAMES, &engine, &on, size, || {}),
    );
    let mut id = 2;
    let mut next = Instant::now();
    let c = pan(LONG, &engine, &on, size, || {
        if Instant::now() >= next {
            install(&engine, id, &buffer);
            id += 1;
            next = Instant::now() + Duration::from_secs(1);
        }
    });
    report(
        &format!("C  a new result every second ({} installed)", id - 2),
        &c,
    );

    let which = if ffdnet {
        rustroom_app::settings::DenoiseNetwork::Ffdnet
    } else {
        rustroom_app::settings::DenoiseNetwork::Drunet
    };
    // D2: the network's tiles on the GPU in a loop — the job's GPU work without its CPU work.
    {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let st = Arc::clone(&stop);
        let dir = models.clone();
        let gpu = std::thread::spawn(move || {
            let mut net = denoised::network(&dir, true, which).expect("network");
            let (tile, sigma) = (vec![0.5; 512 * 512 * 3], vec![15.0; 512 * 512]);
            while !st.load(std::sync::atomic::Ordering::Relaxed) {
                net.run(&tile, 512, 512, &sigma).expect("tile");
            }
        });
        std::thread::sleep(Duration::from_secs(3));
        install(&engine, 1, &buffer);
        report(
            "D2 the network's tiles on the GPU alone, no job",
            &pan(LONG, &engine, &on, size, || {}),
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        gpu.join().expect("gpu thread");
    }

    let dir = models.clone();
    let worker = DenoiseWorker::spawn(
        Box::new(move || denoised::network(&dir, true, which)),
        None,
        || {},
    );
    install(&engine, 1, &buffer);
    worker.request(DenoiseRequest {
        key: "probe-d".into(),
        image: Some(Arc::clone(&buffer)),
    });
    // The network loaded and the noise curve read: tiles are running.
    while worker.progress("probe-d").is_none_or(|(done, _)| done == 0) {
        std::thread::sleep(Duration::from_millis(50));
    }
    report(
        "D  the job running, partials not installed",
        &pan(LONG, &engine, &on, size, || {
            let _ = worker.poll();
        }),
    );

    worker.request(DenoiseRequest {
        key: "probe-e".into(),
        image: Some(Arc::clone(&buffer)),
    });
    let mut installed = 0;
    let e = pan(LONG, &engine, &on, size, || {
        for r in worker.poll() {
            if let DenoiseResult::Partial { image, done, .. } = r {
                install(&engine, 1000 + done as u64, &image);
                installed += 1;
            }
        }
    });
    report(
        &format!("E  the job, its partials installed ({installed})"),
        &e,
    );

    // The fitted view: an exposure step re-runs the tail only; a new result re-runs the prefix and
    // downscales the full photo again.
    let fit = spec(1600, None);
    install(&engine, 1, &buffer);
    frame_ms(&engine, &on, fit);
    let tail: Vec<f64> = (0..6)
        .map(|k| {
            let e = Edits {
                exposure: 0.35 + 0.01 * k as f32,
                ..on.clone()
            };
            frame_ms(&engine, &e, fit)
        })
        .collect();
    report("fit 1600: an exposure step", &tail);
    let fresh: Vec<f64> = (0..6)
        .map(|k| {
            install(&engine, 5000 + k, &buffer);
            frame_ms(&engine, &on, fit)
        })
        .collect();
    report("fit 1600: a new result", &fresh);
}

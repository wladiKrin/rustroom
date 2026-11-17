//! Replays a Straighten drag on a real photo through the app's own `RenderEngine`, so every part
//! of a drag frame can be timed with `RUSTROOM_DEBUG_RENDER=1`.
//!
//! ⚠ Uses the photo's **own sidecar edits** (copy 0) and its matched lens profile, and renders at
//! the rung the user's window actually used, because a drag frame's cost is a property of the
//! photo and the rung, not of a fixture. The camera profile's tone curve is **not** resolved here
//! (that lives in the binary's `decode.rs`), so the rendering stage runs with its built-in curve —
//! the same stage, the same cost.
//!
//! Run: `RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app --example probe_straighten_drag <raw> [rung]`
//! `PROBE_ANGLE_STEP=1.1` sweeps ~0.5°→43°, the range a real drag reaches; the default 0.05 stays
//! under 2.45°, the cheapest corner (M10.14d).
//! With `RUSTROOM_DEBUG_GPU_TIME=1` as well, every chain prints a `GPUTIME` block: the GPU time of
//! each stage's pass and the quantise, the GPU span, and the wall's remainder as `copies+latency`.
//! ⚠ A timed run opens one pass per stage, so quote its wall only beside the untimed one.

use rustroom_app::render_engine::{FrameSpec, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe_straighten_drag <raw> [rung]");
    let rung: usize = args.next().map_or(3200, |r| r.parse().expect("rung"));
    let path = std::path::Path::new(&path);
    // ⚠ The FULL decode: the app hands over to it after opening, and a rung of 3200 is then a
    // downscale of the 6246 px buffer rather than an upscale of the preview.
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let lens = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    let base = rustroom_app::sidecar::load(path).unwrap_or_default();
    eprintln!(
        "buffer {}x{}, lens {}, sidecar edits {}",
        d.image.width(),
        d.image.height(),
        if lens.is_some() { "matched" } else { "none" },
        if base == edit_model::Edits::default() {
            "none"
        } else {
            "loaded"
        }
    );
    let engine = RenderEngine::new(
        Arc::new(d.image),
        opts.source_scale(),
        lens,
        d.metadata.as_shot.unwrap_or_default(),
        rustroom_app::decode_worker::ProfileParts::default(),
        true,
        || {},
    );
    let frame = |angle: f32| -> f64 {
        let mut e = base.clone();
        e.geometry.angle = angle;
        let t = Instant::now();
        engine.request(
            e,
            FrameSpec {
                max_dim: rung,
                region: None,
                display_space: color::Display::SRGB,
                intent: color::RenderingIntent::default(),
                // ⚠ `PROBE_BILINEAR=1` isolates the gather's arithmetic: 2x2 taps against 4x4.
                resampler: if std::env::var_os("PROBE_BILINEAR").is_some() {
                    pipeline::Resampler::Bilinear
                } else {
                    pipeline::Resampler::default()
                },
                gpu_tail: true,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            },
        );
        loop {
            if engine.poll_latest().is_some() {
                return t.elapsed().as_secs_f64() * 1000.0;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    };
    // ⚠⚠ `PROBE_UI_LOAD=1` stands in for the WINDOW (2026-09-26): a second, independent wgpu device
    // — as eframe's is — writing a fresh texture of the frame's size and submitting at 60 Hz, which
    // is roughly what egui does on every drag frame. The in-app tail measured ~7-10 ms more GPU
    // submit+wait than this probe with identical CPU copies; this says whether contention is why.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let load = std::env::var_os("PROBE_UI_LOAD").is_some().then(|| {
        let stop = stop.clone();
        std::thread::spawn(move || ui_load(rung, &stop))
    });
    // Warm: the first frames compile shaders and fill the lens checkpoint.
    for a in [0.0, 0.1, 0.2] {
        frame(a);
    }
    eprintln!("---- drag ----");
    // ⚠ Median of 40, not a mean of 12: single runs varied 30-41 ms on an idle machine, and a
    // mean is dragged by the occasional frame a warm-behind render collided with.
    let n = 40;
    // ⚠⚠ `PROBE_ANGLE_STEP` (M10.14d): the default 0.05° sweeps only 0.5°–2.45°, the CHEAPEST corner
    // of a Straighten drag — the gather's cost rose 3.4× by 43° until it was tiled. `1.1` sweeps to
    // ~43°, the range a real drag reaches; compare like with like before calling anything a gap.
    let step = std::env::var("PROBE_ANGLE_STEP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.05);
    let mut all: Vec<f64> = (0..n).map(|i| frame(0.5 + step * i as f32)).collect();
    all.sort_by(|a, b| a.total_cmp(b));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(h) = load {
        let _ = h.join();
    }
    eprintln!(
        "median wall per drag frame: {:.1} ms  (p10 {:.1}, p90 {:.1}, n {n})",
        all[n / 2],
        all[n / 10],
        all[n * 9 / 10]
    );
    // ⚠ Release and settle (2026-09-26). The worker settles after 150 ms of quiet — with
    // `RUSTROOM_DEBUG_RENDER=1` its own line says how — and the first tail-slider frames after
    // the release are what the settle exists for: a tail with the prefix cached.
    std::thread::sleep(std::time::Duration::from_millis(400));
    let last_angle = 0.5 + 0.05 * (n - 1) as f32;
    let tail_frame = |exposure: f32| -> f64 {
        let mut e = base.clone();
        e.geometry.angle = last_angle;
        e.exposure = exposure;
        let t = Instant::now();
        engine.request(
            e,
            FrameSpec {
                max_dim: rung,
                region: None,
                display_space: color::Display::SRGB,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: true,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            },
        );
        loop {
            if engine.poll_latest().is_some() {
                return t.elapsed().as_secs_f64() * 1000.0;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    };
    let after: Vec<f64> = (1..=6).map(|i| tail_frame(0.05 * i as f32)).collect();
    eprintln!(
        "tail frames after release: {}",
        after
            .iter()
            .map(|ms| format!("{ms:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

/// A window-like GPU load on its OWN device: one texture of the frame's size rewritten and
/// submitted every 16 ms, until `stop`.
fn ui_load(rung: usize, stop: &std::sync::atomic::AtomicBool) {
    use eframe::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("an adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("a device");
    let (w, h) = (rung as u32, (rung as u32 * 2) / 3);
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let bytes = vec![128u8; (w * h * 4) as usize];
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let t = std::time::Instant::now();
        // ⚠ A NEW texture each time, as `ctx.load_texture` makes one per frame.
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-load"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            size,
        );
        queue.submit(None);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        if let Some(rest) = std::time::Duration::from_millis(16).checked_sub(t.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

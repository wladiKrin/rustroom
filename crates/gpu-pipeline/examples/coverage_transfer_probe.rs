//! What do M8.11's two coverage transfers ACTUALLY cost?
//!
//! ⚠⚠ **A throwaway probe, not a milestone.** `docs/BACKLOG.md`'s "A GPU-side mask coverage
//! cache" prices both transfers at "~2 ms at rung 3200", a *ceiling borrowed from M8.3g's
//! decomposition of a 6.5 MB u8 plane* — not a measurement of this. Composing component
//! measurements has missed in this crate five times. This measures the two transfers directly.
//!
//! Three things, at the rungs the ladder actually walks (1600 / 3200 / 6246 for an X-T4 buffer):
//!
//! - **(a) upload** — what a cache HIT pays: `GpuMaskStage::pack` (u8 → packed u32 words) plus the
//!   `write_buffer` of that plane. Measured two ways: a direct A/B on a bare device, and an
//!   in-situ differential through `GpuChain::apply` on a real `GpuMaskStage` with a `Prebuilt`
//!   component — full-frame box against a 16x16 box, everything else identical.
//! - **(b) download** — what a cache MISS pays to deposit: `GpuBrushRaster` reads its coverage
//!   back as **one u32 per sample** (`cov_bytes = w*h*4`), copies it to a `Vec<u32>` and then
//!   narrows to `Vec<u8>`. So the plane on the wire is 4x the u8 figure the backlog quotes.
//!   A/B'd against the identical submit with no copy and no map.
//! - **(c) the GPU rasterise the cache avoids** — `GpuBrushRaster::rasterize` on a synthetic brush
//!   of comparable weight, gating and not. M8.3k measured 31.8–43.5 ms for the gating case.
//!
//! ⚠ The frames are synthetic: nothing here depends on pixel content, only on size, and this crate
//! cannot decode a RAW (it has no `raw-decode`, deliberately).
//!
//! `cargo run --release -p gpu-pipeline --example coverage_transfer_probe`

use core_image::ImageBuf;
use edit_model::{Brush, BrushStroke, Geometry, LocalAdjustments, Mask, MaskedAdjustment};
use pipeline::brush::{BasePixels, BaseRef, CoverageMap};
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;
use std::time::Instant;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// stats
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// min / median / max in ms, from at least three runs. ⚠ A band, never a figure.
fn band(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

fn show(label: &str, v: Vec<f64>) {
    let (lo, med, hi) = band(v);
    println!("    {label:<44} {lo:7.2} .. {hi:7.2} ms   (median {med:.2})");
}

/// The difference of two paired run vectors, element by element — an A/B, not two bands
/// subtracted. ⚠ Subtracting bands is precisely the composition this probe exists to avoid.
fn paired_delta(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x - y).collect()
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// a bare device, because GpuContext's `device`/`queue` are pub(crate)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A device requested exactly as `GpuContext::new` does — same instance flags, same
/// `adapter.limits()`. ⚠ Same physical GPU; this exists only because an example is an external
/// crate and cannot reach `GpuContext`'s private device.
fn bare_device() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
        flags: wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL.with_env(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: Default::default(),
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let info = adapter.get_info();
    let limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;
    Some((device, queue, format!("{} ({:?})", info.name, info.backend)))
}

fn wait(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// (a) upload
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `GpuMaskStage::words_for`, verbatim.
fn words_for(m: &CoverageMap) -> usize {
    (m.w * m.h).div_ceil(4)
}

/// `GpuMaskStage::pack`, verbatim for a single component — the u8 → u32 bit-twiddle a hit pays
/// before a single byte reaches the queue.
fn pack(m: &CoverageMap) -> Vec<u32> {
    let mut packed: Vec<u32> = vec![0; words_for(m)];
    for (k, v) in m.data.iter().enumerate() {
        packed[k >> 2] |= (*v as u32) << ((k & 3) * 8);
    }
    packed
}

fn a_map(w: usize, h: usize) -> CoverageMap {
    CoverageMap {
        x0: 0,
        y0: 0,
        w,
        h,
        // ⚠ Non-constant, so nothing can fold it away.
        data: (0..w * h).map(|i| (i % 251) as u8).collect(),
        outside: 0,
    }
}

/// Direct A/B of the upload on a bare device: `create_buffer` + `write_buffer` + submit + wait,
/// against the same buffer, submit and wait with **no write**.
fn upload_ab(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &CoverageMap,
    runs: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let words = pack(map);
    let bytes = (words.len() * 4) as u64;
    let mut with = Vec::new();
    let mut without = Vec::new();
    let mut packing = Vec::new();

    for _ in 0..runs {
        // the pack loop alone — pure CPU, and it is not obviously free
        let t = Instant::now();
        let w2 = pack(map);
        packing.push(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&w2);

        // A: buffer + write_buffer + submit + wait
        let t = Instant::now();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-cov-a"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytemuck::cast_slice(&words));
        queue.submit(Some(device.create_command_encoder(&Default::default()).finish()));
        wait(device);
        with.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(buf);

        // B: the same, minus the write
        let t = Instant::now();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-cov-b"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.submit(Some(device.create_command_encoder(&Default::default()).finish()));
        wait(device);
        without.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(buf);
    }
    (with, without, packing)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// (b) download
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `GpuBrushRaster::run`'s readback, replicated: a `w*h*4` storage buffer copied to a MAP_READ
/// buffer, mapped, `to_vec`'d as `Vec<u32>` and narrowed to `Vec<u8>`.
///
/// ⚠ `samples * 4` bytes on the wire, because the shader writes **one u32 per sample** — narrowing
/// happens on the CPU after the map, and that is the shape the deposit actually has.
fn download_ab(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    samples: usize,
    bytes_per_sample: u64,
    runs: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    // ⚠ `COPY_BUFFER_ALIGNMENT` is 4; an odd-sized u8 plane must be rounded up.
    let bytes = (samples as u64 * bytes_per_sample).div_ceil(4) * 4;
    let src = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe-src"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut with = Vec::new();
    let mut without = Vec::new();
    let mut cpu_half = Vec::new();

    for _ in 0..runs {
        // A: copy + map + to_vec + narrow
        let t = Instant::now();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&src, 0, &readback, 0, bytes);
        queue.submit(Some(enc.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        wait(device);
        rx.recv().unwrap().unwrap();
        let t_cpu = Instant::now();
        if bytes_per_sample == 4 {
            let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
            let data: Vec<u8> = words.iter().map(|&v| v.min(255) as u8).collect();
            std::hint::black_box(&data);
        } else {
            let data: Vec<u8> = slice.get_mapped_range().to_vec();
            std::hint::black_box(&data);
        }
        cpu_half.push(t_cpu.elapsed().as_secs_f64() * 1000.0);
        readback.unmap();
        with.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(readback);

        // B: the same allocation and submit, no copy, no map
        let t = Instant::now();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-readback-b"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        queue.submit(Some(device.create_command_encoder(&Default::default()).finish()));
        wait(device);
        without.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(readback);
    }
    (with, without, cpu_half)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// (a) in-situ: the real GpuMaskStage, full-frame box against a 16x16 one
// ─────────────────────────────────────────────────────────────────────────────────────────────

fn masked() -> MaskedAdjustment {
    MaskedAdjustment {
        mask: Mask::Brush(Brush {
            strokes: vec![BrushStroke {
                points: (0..24).map(|i| [0.15 + i as f32 * 0.03, 0.5]).collect(),
                radius: 0.12,
                hardness: 40.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                ..Default::default()
            }],
            invert: false,
        }),
        adjustments: LocalAdjustments {
            exposure: 1.5,
            ..Default::default()
        },
        refine: Vec::new(),
    }
}

fn scene(w: usize, h: usize) -> ImageBuf {
    let mut v = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        for stride in [37usize, 61, 89] {
            v.push(((i * stride) % 313) as f32 / 313.0 * 1.4 - 0.1);
        }
    }
    ImageBuf::from_vec(w, h, v)
}

/// One `GpuChain::apply` over a mask whose only component is a `Prebuilt` map of the given box.
fn insitu_run(
    ctx: &Arc<gpu_pipeline::GpuContext>,
    img: &ImageBuf,
    map: Arc<CoverageMap>,
) -> Option<f64> {
    let m = masked();
    let subs: Vec<Box<dyn gpu_pipeline::GpuStage>> = vec![Box::new(
        gpu_pipeline::GpuExposureStage::new(m.adjustments.exposure, ctx.clone()),
    )];
    let stage = gpu_pipeline::GpuMaskStage::new(
        &m,
        &Geometry::default(),
        None,
        RenderScale::FULL,
        color::wb::Illuminant::default(),
        &[Some(map)],
        subs,
        ctx,
    )?;
    let chain = gpu_pipeline::GpuChain::new(ctx.clone(), vec![Box::new(stage)]);
    let t = Instant::now();
    let out = chain.apply(img.clone());
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    std::hint::black_box(&out);
    Some(ms)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// (c) the rasterise the cache avoids
// ─────────────────────────────────────────────────────────────────────────────────────────────

struct ImgBase<'a>(&'a ImageBuf);
impl BasePixels for ImgBase<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.0.pixel(x, y)
    }
}

/// A brush of roughly the weight the catalogue's real one has — 36 strokes, ~3,600 dabs over a
/// large box. ⚠ Synthetic; `auto_mask` is the caller's choice so the gating and non-gating paths
/// are both reachable.
fn heavy_brush(auto_mask: bool) -> Brush {
    let strokes = (0..36)
        .map(|s| {
            let y = 0.08 + (s as f32) * 0.024;
            BrushStroke {
                points: (0..40).map(|i| [0.06 + i as f32 * 0.022, y]).collect(),
                radius: 0.03,
                hardness: 40.0,
                flow: 100.0,
                auto_mask,
                tolerance: 25.0,
                ..Default::default()
            }
        })
        .collect();
    Brush {
        strokes,
        invert: false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);

    let Some((device, queue, name)) = bare_device() else {
        println!("no usable GPU adapter");
        return;
    };
    println!("bare device: {name}");
    let ctx = gpu_pipeline::shared().cloned();
    if let Some(c) = &ctx {
        println!("shared ctx : {} ({})", c.adapter_name(), c.backend());
    }
    println!("runs per configuration: {runs}\n");

    // The ladder an X-T4 buffer (6246x4170) walks, plus a realistic partial box.
    let rungs: [(&str, usize, usize); 4] = [
        ("rung 1600  (1600x1068, whole frame)", 1600, 1068),
        ("rung 3200  (3200x2137, whole frame)", 3200, 2137),
        ("rung 6246  (6246x4170, whole frame)", 6246, 4170),
        ("rung 3200  (1510x2019, a real brush box)", 1510, 2019),
    ];

    for (label, w, h) in rungs {
        let samples = w * h;
        println!(
            "── {label}  —  {:.2} Mpx, {:.2} MB as u8, {:.2} MB as u32",
            samples as f64 / 1.0e6,
            samples as f64 / 1.0e6,
            samples as f64 * 4.0 / 1.0e6
        );

        // (a) upload, direct
        let map = a_map(w, h);
        let (with, without, packing) = upload_ab(&device, &queue, &map, runs);
        show("(a) pack() alone, CPU", packing.clone());
        show("(a) alloc+write_buffer+submit+wait", with.clone());
        show("(a)   same, no write (control)", without.clone());
        show("(a) UPLOAD = paired delta", paired_delta(&with, &without));

        // (b) download, direct — the real u32 shape and, for contrast, a u8 one
        let (dw, dwo, cpu) = download_ab(&device, &queue, samples, 4, runs);
        show("(b) copy+map+to_vec+narrow (u32 wire)", dw.clone());
        show("(b)   same, no copy/map (control)", dwo.clone());
        show("(b)   of which CPU to_vec+narrow", cpu);
        show("(b) DOWNLOAD = paired delta", paired_delta(&dw, &dwo));
        let (u8w, u8wo, _) = download_ab(&device, &queue, samples, 1, runs);
        show(
            "(b') hypothetical u8 wire = paired delta",
            paired_delta(&u8w, &u8wo),
        );
        println!();
    }

    // (a) in-situ — the real GpuMaskStage through GpuChain, full box against 16x16.
    if let Some(ctx) = &ctx {
        println!("── (a) in-situ: GpuChain::apply on a Prebuilt mask, full box vs 16x16 box");
        for (label, w, h) in [("1600x1068", 1600usize, 1068usize), ("3200x2137", 3200, 2137)] {
            let img = scene(w, h);
            let full = Arc::new(a_map(w, h));
            let tiny = Arc::new(a_map(16, 16));
            // warm
            let _ = insitu_run(ctx, &img, full.clone());
            let _ = insitu_run(ctx, &img, tiny.clone());
            let mut a = Vec::new();
            let mut b = Vec::new();
            for _ in 0..runs {
                a.push(insitu_run(ctx, &img, full.clone()).expect("prebuilt mask must build"));
                b.push(insitu_run(ctx, &img, tiny.clone()).expect("prebuilt mask must build"));
            }
            println!("  {label}");
            show("chain.apply, full-frame coverage box", a.clone());
            show("chain.apply, 16x16 coverage box", b.clone());
            show("IN-SITU UPLOAD = paired delta", paired_delta(&a, &b));
        }
        println!();

        // (c) the rasterise the cache avoids.
        println!("── (c) GpuBrushRaster::rasterize — the work a cache hit avoids");
        let raster = gpu_pipeline::GpuBrushRaster::new(ctx);
        for (label, w, h) in [("1600x1068", 1600usize, 1068usize), ("3200x2137", 3200, 2137)] {
            let img = scene(w, h);
            let base = ImgBase(&img);
            for gating in [false, true] {
                let brush = heavy_brush(gating);
                let Some(plan) = pipeline::brush::plan_dabs(
                    &brush,
                    &Geometry::default(),
                    w,
                    h,
                    None,
                    if gating { Some(&base as &dyn BasePixels) } else { None },
                ) else {
                    println!("    no plan for {label} gating={gating}");
                    continue;
                };
                let dabs: usize = plan.strokes.iter().map(|s| s.dabs.len()).sum();
                let bref = BaseRef {
                    plane: img.data(),
                    w,
                    h,
                    x0: 0,
                    y0: 0,
                };
                let mut v = Vec::new();
                for _ in 0..runs {
                    let t = Instant::now();
                    let got = raster.rasterize(&plan, false, if gating { Some(bref) } else { None });
                    v.push(t.elapsed().as_secs_f64() * 1000.0);
                    assert!(got.is_some(), "the rasteriser declined");
                }
                println!(
                    "  {label} gating={gating}  box {}x{}  {dabs} dabs",
                    plan.w, plan.h
                );
                show("rasterize() wall", v);
            }
        }
    }
}

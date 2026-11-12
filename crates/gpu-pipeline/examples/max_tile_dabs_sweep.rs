//! Where is `MAX_TILE_DABS`'s real knee?
//!
//! ⚠⚠ **A MEASUREMENT HARNESS, NOT PRODUCTION CODE, AND IT DELIBERATELY DOES NOT TOUCH ANY
//! EXISTING FILE.** `GpuBrushRaster::with_capacity` is `#[cfg(test)]` and `brush_wgsl` is a private
//! module, so the shipped rasteriser cannot be asked to run at any capacity but
//! [`gpu_pipeline::max_tile_dabs`]. This example therefore builds its **own** `wgpu` device and its
//! own pipeline — but it does **not** copy the shader. The WGSL is `include_str!`'d out of
//! `src/brush_wgsl.rs` and `src/oklab_wgsl.rs` at COMPILE TIME and sliced out of the raw string
//! literal, so there is no frozen copy that can drift from the code under measurement.
//!
//! ⚠ And the harness is **validated before it is believed**: at the shipped capacity its coverage
//! map is compared byte for byte against the real `GpuBrushRaster`. A harness that disagrees with
//! the thing it stands in for is measuring something else.
//!
//! Two curves, because `MAX_TILE_DABS` sits between two different failure modes:
//!
//! 1. **The workload curve** — cost as the dabs *actually* in the busiest tile rises, at the
//!    shipped capacity. Its knee is the **overflow cliff**: past the capacity the kernel re-scans
//!    the whole dab list per pixel.
//! 2. **The capacity curve** — cost as the `var<workgroup>` array grows, at a fixed workload. Its
//!    knee is the **occupancy cliff**: workgroup storage throttles how many workgroups are resident
//!    per core. This is the curve the constant was originally sized against a *limit* instead of.
//!
//! `cargo run --release -p gpu-pipeline --example max_tile_dabs_sweep`

use core_image::ImageBuf;
use edit_model::{Brush, BrushStroke, Geometry};
use pipeline::brush::{plan_dabs, BasePixels, BaseRef, CoverageMap, DabPlan};
use std::time::Instant;

// ── the shader, extracted from the crate's own source ────────────────────────────────────────
//
// ⚠ `include_str!` of the .rs file, then a slice between `r#"` and the closing `"#;`. Both files
// contain exactly one raw string literal, which is asserted below rather than assumed.
const BRUSH_RS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/brush_wgsl.rs"));
const OKLAB_RS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/oklab_wgsl.rs"));

/// The one raw-string body in `src`, or a panic naming the file.
fn raw_string_body(src: &'static str, what: &str) -> &'static str {
    assert_eq!(
        src.matches("r#\"").count(),
        1,
        "{what}: expected exactly one raw string literal"
    );
    assert_eq!(
        src.matches("\"#;").count(),
        1,
        "{what}: expected exactly one raw string close"
    );
    let start = src.find("r#\"").unwrap() + 3;
    let end = src.find("\"#;").unwrap();
    &src[start..end]
}

/// The whole shader, with the two constants substituted exactly as `brush_wgsl::source` does.
fn shader_source(max_tile_dabs: u32) -> String {
    let body = raw_string_body(BRUSH_RS, "brush_wgsl.rs");
    let oklab = raw_string_body(OKLAB_RS, "oklab_wgsl.rs");
    format!(
        "{}\n{}",
        oklab,
        body.replace("{MAX_TILE_DABS}", &max_tile_dabs.to_string())
            .replace("{TILE}", "16")
            .replace("{THREADS}", "256")
            .replace("{COUNT_TESTS}", "false")
    )
}

// ── the buffer layouts, which must match the WGSL ────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuDab {
    pos: [f32; 4],
    gate: [f32; 4],
    stroke: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuStroke {
    flow: f32,
    erase: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    img_w: u32,
    img_h: u32,
    dab_count: u32,
    invert: u32,
    has_base: u32,
    tiles_x: u32,
    base_x0: u32,
    base_y0: u32,
    _pad: [u32; 2],
}

/// The harness: one device, one resident base plane, one pipeline per capacity.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    backend: String,
    max_workgroup_storage: u32,
}

/// Everything about one measured dispatch.
struct Shot {
    /// Submit → `poll(Wait)` → mapped, with the base plane ALREADY resident. The compute itself.
    dispatch_ms: f64,
    cov: Vec<u8>,
    occupancy: u32,
}

impl Harness {
    fn new() -> Option<Self> {
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
        let max_workgroup_storage = limits.max_compute_workgroup_storage_size;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("max-tile-dabs-sweep"),
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        Some(Self {
            device,
            queue,
            adapter: info.name,
            backend: format!("{:?}", info.backend),
            max_workgroup_storage,
        })
    }

    /// Flatten a plan exactly as `GpuBrushRaster::run` does.
    fn flatten(plan: &DabPlan) -> (Vec<GpuDab>, Vec<GpuStroke>) {
        let mut dabs: Vec<GpuDab> = Vec::new();
        let mut strokes: Vec<GpuStroke> = Vec::with_capacity(plan.strokes.len());
        for (si, s) in plan.strokes.iter().enumerate() {
            strokes.push(GpuStroke {
                flow: s.flow,
                erase: u32::from(s.erase),
                _pad: [0; 2],
            });
            if s.dabs.is_empty() || s.flow <= 0.0 {
                continue;
            }
            for d in &s.dabs {
                let (gate, ceiling) = match d.gate {
                    Some((r, c)) => (r, c),
                    None => ([0.0; 3], -1.0),
                };
                dabs.push(GpuDab {
                    pos: [d.x, d.y, d.r, d.hard],
                    gate: [gate[0], gate[1], gate[2], ceiling],
                    stroke: si as u32,
                    _pad: [0; 3],
                });
            }
        }
        (dabs, strokes)
    }

    /// One dispatch at `capacity`. `base_buf` is already on the device, so the time returned is the
    /// compute and the coverage readback — not the 82 MB upload, which is the same at every
    /// capacity and would only dilute the curve.
    #[allow(clippy::too_many_arguments)]
    fn shot(
        &self,
        plan: &DabPlan,
        capacity: u32,
        base_buf: &wgpu::Buffer,
        base_w: u32,
        base_h: u32,
        has_base: bool,
    ) -> Shot {
        let device = &self.device;
        let (dabs, strokes) = Self::flatten(plan);
        let (w, h) = (plan.w as u32, plan.h as u32);
        let tiles_x = w.div_ceil(16);
        let tiles_y = h.div_ceil(16);
        let tiles = tiles_x * tiles_y;

        let storage = |label: &'static str, bytes: &[u8]| -> wgpu::Buffer {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (bytes.len() as u64).max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !bytes.is_empty() {
                self.queue.write_buffer(&b, 0, bytes);
            }
            b
        };
        let dab_buf = storage("dabs", bytemuck::cast_slice(&dabs));
        let stroke_buf = storage("strokes", bytemuck::cast_slice(&strokes));

        let cov_bytes = u64::from(w) * u64::from(h) * 4;
        let cov_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cov"),
            size: cov_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let counters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("counters"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&counters, 0, &[0u8; 16]);

        let params = Params {
            x0: plan.x0 as u32,
            y0: plan.y0 as u32,
            w,
            h,
            img_w: base_w,
            img_h: base_h,
            dab_count: dabs.len() as u32,
            invert: 0,
            has_base: u32::from(has_base),
            tiles_x,
            base_x0: 0,
            base_y0: 0,
            _pad: [0; 2],
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform, 0, bytemuck::bytes_of(&params));

        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
                entry(4, wgpu::BufferBindingType::Uniform),
                entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush"),
            source: wgpu::ShaderSource::Wgsl(shader_source(capacity).into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("brush"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: dab_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: stroke_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: base_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: cov_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: counters.as_entire_binding(),
                },
            ],
        });

        let gx = tiles.clamp(1, 65535);
        let gy = tiles.div_ceil(gx);

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: cov_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let counter_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("counter-readback"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let t = Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        enc.copy_buffer_to_buffer(&cov_buf, 0, &readback, 0, cov_bytes);
        enc.copy_buffer_to_buffer(&counters, 0, &counter_read, 0, 16);
        self.queue.submit(Some(enc.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let cslice = counter_read.slice(..);
        let (ctx2, crx) = std::sync::mpsc::channel();
        cslice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = ctx2.send(r);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().unwrap().unwrap();
        crx.recv().unwrap().unwrap();
        let dispatch_ms = t.elapsed().as_secs_f64() * 1e3;

        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        let counts: Vec<u32> = bytemuck::cast_slice(&cslice.get_mapped_range()).to_vec();
        readback.unmap();
        counter_read.unmap();

        Shot {
            dispatch_ms,
            cov: words.iter().map(|&v| v.min(255) as u8).collect(),
            occupancy: counts[2],
        }
    }
}

// ── the fixture ──────────────────────────────────────────────────────────────────────────────

/// The rung every masked figure in the journal is quoted at.
const FRAME_W: usize = 3200;
const FRAME_H: usize = 2136;

/// A synthetic base plane with real structure, so the auto-mask gate is not degenerate.
///
/// ⚠⚠ **SYNTHETIC BECAUSE THE CATALOGUE NO LONGER CONTAINS A BRUSH MASK AT ALL.** M8.3k's figures
/// came from `DSCF6274.RAF.json`'s 36-stroke brush; scanned on 2026-09-16, all 40 sidecars in
/// `~/Pictures/fuji` carry exactly **one** mask between them and it is a `Luminance`. The brush
/// that justified `MAX_TILE_DABS` is gone — the journal's own rule that *a caveat measured against
/// a catalogue is invalidated by editing that catalogue*, arriving for the constant this time.
fn base_plane() -> ImageBuf {
    let mut img = ImageBuf::new(FRAME_W, FRAME_H);
    let d = img.data_mut();
    for y in 0..FRAME_H {
        for x in 0..FRAME_W {
            let i = (y * FRAME_W + x) * 3;
            let u = x as f32 / FRAME_W as f32;
            let v = y as f32 / FRAME_H as f32;
            // Two overlapping gradients plus a ridge, so neighbouring pixels differ and the gate
            // both passes and cuts somewhere.
            let ridge = if (x / 37 + y / 53) % 2 == 0 {
                0.18
            } else {
                0.0
            };
            d[i] = 0.22 + 0.5 * u + ridge;
            d[i + 1] = 0.30 + 0.4 * v;
            d[i + 2] = 0.26 + 0.3 * (u * v) + 0.5 * ridge;
        }
    }
    img
}

struct ImgBase<'a>(&'a ImageBuf);
impl BasePixels for ImgBase<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.0.pixel(x, y)
    }
}

/// `k` near-parallel vertical strokes over a fixed band, so the BOX stays constant while the dabs
/// in the busiest tile scale with `k`.
///
/// ⚠ `radius` is a fraction of the long edge. A large radius is what buys occupancy cheaply: a
/// tile sees every stroke passing within `r`, while dabs along a stroke are laid `0.25 r` apart —
/// so occupancy rises with `k * r` and the total dab count *falls* with `r`. That is how the real
/// brush reached 229 dabs in a tile with only 3,645 dabs in total, and reproducing it any other
/// way would measure the two cooperative scans instead of the shared list.
fn band_brush(k: usize, radius: f32, auto_mask_share: f32) -> Brush {
    let n_pts = 9;
    let strokes = (0..k)
        .map(|i| {
            let t = if k == 1 {
                0.5
            } else {
                i as f32 / (k - 1) as f32
            };
            // The band in normalised x, and a gentle S so the strokes are not one straight line.
            let cx = 0.26 + 0.44 * t;
            BrushStroke {
                decode: false,
                points: (0..n_pts)
                    .map(|j| {
                        let s = j as f32 / (n_pts - 1) as f32;
                        [cx + 0.02 * (s * 6.0).sin(), 0.06 + 0.88 * s]
                    })
                    .collect(),
                radius,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: (i as f32 / k.max(1) as f32) < auto_mask_share,
                tolerance: 25.0,
                erase: i % 11 == 10,
            }
        })
        .collect();
    Brush {
        strokes,
        invert: false,
    }
}

// ── reporting ────────────────────────────────────────────────────────────────────────────────

/// Best of `rounds` warm dispatches, round 0 discarded — the project's own bench rule.
fn best_of(rounds: usize, mut f: impl FnMut() -> Shot) -> (f64, Shot) {
    let mut best = f64::MAX;
    let mut last = f();
    for _ in 1..rounds {
        last = f();
        best = best.min(last.dispatch_ms);
    }
    (best, last)
}

fn band(v: &[f64]) -> String {
    let lo = v.iter().cloned().fold(f64::MAX, f64::min);
    let hi = v.iter().cloned().fold(f64::MIN, f64::max);
    format!("{lo:8.2}–{hi:<8.2}")
}

fn main() {
    let harness = match Harness::new() {
        Some(h) => h,
        None => {
            println!("no GPU adapter — nothing to measure");
            return;
        }
    };
    println!(
        "adapter: {} ({}), max_compute_workgroup_storage_size = {} bytes",
        harness.adapter, harness.backend, harness.max_workgroup_storage
    );
    println!(
        "shipped MAX_TILE_DABS = {}  ({} bytes of workgroup storage, + 1024 for the 256-entry \
         ordering array)",
        gpu_pipeline::max_tile_dabs(),
        gpu_pipeline::max_tile_dabs() * 4
    );
    println!("frame {FRAME_W}x{FRAME_H}\n");

    let img = base_plane();
    let b = ImgBase(&img);
    let geometry = Geometry::default();

    // The base plane, uploaded ONCE and left resident for every dispatch below.
    let base_bytes = std::mem::size_of_val(img.data()) as u64;
    let base_buf = harness.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("base"),
        size: base_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    harness
        .queue
        .write_buffer(&base_buf, 0, bytemuck::cast_slice(img.data()));
    println!("base plane resident: {:.1} MB\n", base_bytes as f64 / 1e6);

    let shot = |plan: &DabPlan, cap: u32| {
        harness.shot(plan, cap, &base_buf, FRAME_W as u32, FRAME_H as u32, true)
    };

    // ── 0. the harness is validated against the shipped rasteriser ──────────────────────────
    //
    // ⚠ A harness that disagrees with the code it stands in for is measuring something else.
    let reference_k = 100;
    let radius = 0.07;
    let ref_brush = band_brush(reference_k, radius, 0.6);
    let ref_plan =
        plan_dabs(&ref_brush, &geometry, FRAME_W, FRAME_H, None, Some(&b)).expect("no dabs");
    println!(
        "reference workload: {} strokes, r={:.3} of the long edge, box {}x{} ({} px, {} tiles), \
         {} dabs",
        reference_k,
        radius,
        ref_plan.w,
        ref_plan.h,
        ref_plan.w * ref_plan.h,
        (ref_plan.w.div_ceil(16)) * (ref_plan.h.div_ceil(16)),
        ref_plan.strokes.iter().map(|s| s.dabs.len()).sum::<usize>(),
    );

    match gpu_pipeline::shared() {
        Some(ctx) => {
            let real = gpu_pipeline::GpuBrushRaster::new(ctx);
            let mut real_best = f64::MAX;
            let mut real_map: Option<CoverageMap> = None;
            let mut real_occ = 0;
            for r in 0..5 {
                let t = Instant::now();
                let (m, occ) = real
                    .rasterize_with_occupancy(
                        &ref_plan,
                        false,
                        Some(BaseRef {
                            plane: img.data(),
                            w: FRAME_W,
                            h: FRAME_H,
                            x0: 0,
                            y0: 0,
                        }),
                    )
                    .expect("the shipped rasteriser failed");
                let ms = t.elapsed().as_secs_f64() * 1e3;
                if r > 0 {
                    real_best = real_best.min(ms);
                }
                real_occ = occ;
                real_map = Some(m);
            }
            let mine = shot(&ref_plan, gpu_pipeline::max_tile_dabs());
            let rm = real_map.unwrap();
            let differing = rm
                .data
                .iter()
                .zip(mine.cov.iter())
                .filter(|(a, b)| a != b)
                .count();
            println!(
                "harness check @ capacity {}: {} of {} samples differ from the shipped rasteriser; \
                 occupancy {} (shipped) vs {} (harness)",
                gpu_pipeline::max_tile_dabs(),
                differing,
                rm.data.len(),
                real_occ,
                mine.occupancy,
            );
            println!(
                "  shipped rasteriser, whole call incl. the {:.0} MB base upload: {real_best:.2} ms\n\
                 \x20 harness, compute + coverage readback only (base already resident): {:.2} ms\n",
                base_bytes as f64 / 1e6,
                mine.dispatch_ms,
            );
            assert_eq!(
                differing, 0,
                "the harness disagrees with the shipped rasteriser — do not trust the numbers below"
            );
        }
        None => println!("no shared GpuContext — skipping the validation against the shipped code"),
    }

    // ── 1. the workload curve, at the SHIPPED capacity ──────────────────────────────────────
    println!(
        "── 1. cost as the dabs in the busiest tile rise, at the shipped capacity {} ──",
        gpu_pipeline::max_tile_dabs()
    );
    println!("  strokes   dabs   box px  tiles   occupancy   over?        dispatch ms (3 passes)");
    let ks = [12usize, 25, 50, 100, 200, 400, 800, 1600];
    for k in ks {
        let brush = band_brush(k, radius, 0.6);
        let Some(plan) = plan_dabs(&brush, &geometry, FRAME_W, FRAME_H, None, Some(&b)) else {
            continue;
        };
        let dabs: usize = plan.strokes.iter().map(|s| s.dabs.len()).sum();
        let tiles = plan.w.div_ceil(16) * plan.h.div_ceil(16);
        let mut times = Vec::new();
        let mut occ = 0;
        for _ in 0..3 {
            let (best, last) = best_of(5, || shot(&plan, gpu_pipeline::max_tile_dabs()));
            times.push(best);
            occ = last.occupancy;
        }
        let over = if occ > gpu_pipeline::max_tile_dabs() {
            "OVERFLOW"
        } else {
            "        "
        };
        println!(
            "  {k:>7} {dabs:>6} {:>8} {tiles:>6}   {occ:>9}   {over}   {}",
            plan.w * plan.h,
            band(&times),
        );
    }

    // ── 2. the capacity curve, at a FIXED workload ──────────────────────────────────────────
    println!(
        "\n── 2. cost as the workgroup array grows, fixed workload (occupancy measured below) ──"
    );
    println!("  capacity   wg storage   dispatch ms (3 passes)          vs the shipped capacity");
    let caps: Vec<u32> = vec![64, 128, 256, 512, 1024, 1536, 2048, 2560, 3072, 3584];
    let mut at_shipped = f64::NAN;
    let mut rows: Vec<(u32, Vec<f64>, u32)> = Vec::new();
    for cap in &caps {
        let bytes = cap * 4 + 256 * 4 + 16;
        if bytes > harness.max_workgroup_storage {
            println!("  {cap:>8}   {bytes:>6} B    — above this device's limit, not measured");
            continue;
        }
        let mut times = Vec::new();
        let mut occ = 0;
        for _ in 0..3 {
            let (best, last) = best_of(5, || shot(&ref_plan, *cap));
            times.push(best);
            occ = last.occupancy;
        }
        if *cap == gpu_pipeline::max_tile_dabs() {
            at_shipped = times.iter().cloned().fold(f64::MAX, f64::min);
        }
        rows.push((*cap, times, occ));
    }
    for (cap, times, occ) in &rows {
        let lo = times.iter().cloned().fold(f64::MAX, f64::min);
        let note = if *occ > *cap { "  (OVERFLOW path)" } else { "" };
        println!(
            "  {cap:>8}   {:>6} B   {}   {:>6.2}x{note}",
            cap * 4 + 256 * 4 + 16,
            band(times),
            lo / at_shipped,
        );
    }

    // ── 3. the capacity curve again, at a HEAVIER workload ──────────────────────────────────
    //
    // ⚠ Because the plateau's width may itself depend on how much per-pixel work each resident
    // workgroup has to hide the latency with — one workload cannot say that.
    let heavy = band_brush(400, radius, 0.6);
    let heavy_plan =
        plan_dabs(&heavy, &geometry, FRAME_W, FRAME_H, None, Some(&b)).expect("no dabs");
    println!(
        "\n── 3. the same capacity curve at a 4x heavier workload ({} dabs) ──",
        heavy_plan
            .strokes
            .iter()
            .map(|s| s.dabs.len())
            .sum::<usize>()
    );
    println!("  capacity   occupancy   dispatch ms (3 passes)");
    for cap in &caps {
        let bytes = cap * 4 + 256 * 4 + 16;
        if bytes > harness.max_workgroup_storage {
            continue;
        }
        let mut times = Vec::new();
        let mut occ = 0;
        for _ in 0..3 {
            let (best, last) = best_of(5, || shot(&heavy_plan, *cap));
            times.push(best);
            occ = last.occupancy;
        }
        let note = if occ > *cap { "  (OVERFLOW path)" } else { "" };
        println!("  {cap:>8}   {occ:>9}   {}{note}", band(&times));
    }

    println!(
        "\n⚠ Quote a BAND over the three passes, never one figure. ⚠ Rows marked OVERFLOW are the\n\
         ⚠ workload exceeding the capacity, where the kernel re-scans the whole dab list per pixel\n\
         ⚠ — a correctness path, not a tuning point."
    );
}

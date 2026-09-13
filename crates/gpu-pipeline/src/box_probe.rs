//! ⚠⚠ **The load-bearing risk probe for M8.3n, run BEFORE the spec was written** — the M8.3f
//! precedent, where measuring the design's central assumption first is what stopped a wrong
//! design being specced.
//!
//! The assumption: that `box_blur` ports to the GPU the way `gaussian_blur` did. It may not.
//! **The CPU's box blur is O(1) in its radius** — it keeps a running sum, measured at 4.68 ms at
//! r = 40 against 4.63 at r = 2, i.e. flat. A GPU thread has no running sum to keep, so the
//! obvious port is **O(r)**: 81 taps at Clarity's radius against 5 at Texture's. Three iterated
//! passes, separable, is **486 taps per pixel** where the CPU pays a constant handful.
//!
//! So this times the naive port at both radii and asks whether it beats the CPU it replaces.
//!
//! Run: `cargo test --release -p gpu-pipeline -- --ignored --nocapture probe_box`

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// ⚠⚠ **The window CLAMPS rather than replicating, which is `box_blur`'s rule and NOT
/// `gaussian_blur`'s.** Near a border the CPU's window shrinks and the sum is divided by the
/// count actually inside it; `sharpen.rs`'s `clampi` instead repeats the edge sample. The two
/// agree on a constant plane and disagree on every real one, so this cannot reuse `blur_h`.
const SHADER: &str = r#"
struct Params {
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    r: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> a: array<f32>;
@group(0) @binding(3) var<storage, read_write> b: array<f32>;

fn index(id: vec3<u32>) -> u32 {
    return id.y * (params.row_groups * 64u) + id.x;
}

@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    a[i] = img[i * 3u];
}

// a -> b, horizontal, window clamped at the borders.
@compute @workgroup_size(64)
fn box_h(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    let lo = select(0u, x - params.r, x >= params.r);
    let hi = min(x + params.r + 1u, params.w);
    var acc = 0.0;
    for (var t = lo; t < hi; t = t + 1u) {
        acc = acc + a[y * params.w + t];
    }
    b[i] = acc / f32(hi - lo);
}

// b -> a, vertical.
@compute @workgroup_size(64)
fn box_v(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    let lo = select(0u, y - params.r, y >= params.r);
    let hi = min(y + params.r + 1u, params.h);
    var acc = 0.0;
    for (var t = lo; t < hi; t = t + 1u) {
        acc = acc + b[t * params.w + x];
    }
    a[i] = acc / f32(hi - lo);
}

// ⚠⚠ **The CPU's algorithm, transcribed: one thread per ROW, carrying a running sum.** O(1) per
// pixel in the radius instead of O(r) — and *more* faithful than the naive sum, because it
// accumulates in the same order the CPU does rather than re-summing the window per pixel.
// ⚠ The cost is coalescing: adjacent threads walk adjacent ROWS, so their reads sit `w` floats
// apart. Whether that outweighs doing 81x less work is exactly what this probe asks.
@compute @workgroup_size(64)
fn box_h_run(@builtin(global_invocation_id) id: vec3<u32>) {
    let y = id.y * (params.row_groups * 64u) + id.x;
    if (y >= params.h) { return; }
    let row = y * params.w;
    var hi = min(params.r + 1u, params.w);
    var lo = 0u;
    var sum = 0.0;
    for (var t = 0u; t < hi; t = t + 1u) { sum = sum + a[row + t]; }
    for (var x = 0u; x < params.w; x = x + 1u) {
        b[row + x] = sum / f32(hi - lo);
        if (x + params.r + 1u < params.w) { sum = sum + a[row + x + params.r + 1u]; hi = hi + 1u; }
        if (x >= params.r) { sum = sum - a[row + x - params.r]; lo = lo + 1u; }
    }
}

// ⚠ The vertical twin, and the coalescing here is **perfect**: adjacent threads are adjacent
// columns, so a step reads one contiguous row — which is exactly why the CPU's `blur_cols`
// advances every column together rather than walking one at a time.
@compute @workgroup_size(64)
fn box_v_run(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.y * (params.row_groups * 64u) + id.x;
    if (x >= params.w) { return; }
    var hi = min(params.r + 1u, params.h);
    var lo = 0u;
    var sum = 0.0;
    for (var t = 0u; t < hi; t = t + 1u) { sum = sum + b[t * params.w + x]; }
    for (var y = 0u; y < params.h; y = y + 1u) {
        a[y * params.w + x] = sum / f32(hi - lo);
        if (y + params.r + 1u < params.h) { sum = sum + b[(y + params.r + 1u) * params.w + x]; hi = hi + 1u; }
        if (y >= params.r) { sum = sum - b[(y - params.r) * params.w + x]; lo = lo + 1u; }
    }
}

@compute @workgroup_size(64)
fn emit(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    img[i * 3u] = a[i];
}
"#;

/// `passes` iterations of a separable box blur at radius `r`, over a scratch pair.
pub(crate) struct BoxProbe {
    pub r: u32,
    pub passes: usize,
    pub running: bool,
    cpu: pipeline::ExposureStage,
}

impl BoxProbe {
    pub fn new(r: u32, passes: usize, running: bool) -> Self {
        Self {
            r,
            passes,
            running,
            cpu: pipeline::ExposureStage::new(0.0),
        }
    }
}

impl GpuStage for BoxProbe {
    fn name(&self) -> &'static str {
        "box_probe"
    }
    fn cpu_cost(&self) -> f32 {
        1000.0
    }
    fn scratch_planes(&self) -> usize {
        2
    }
    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        let total = img.pixels().div_ceil(WORKGROUP);
        let row_groups = total.min(MAX_GROUPS_PER_DIM);
        let col_groups = total.div_ceil(row_groups);

        // ⚠ Each pass carries its OWN thread count: a running-sum pass wants one thread per line,
        // not one per pixel, and dispatching w*h threads for 2136 rows of work would price the
        // scheduler rather than the algorithm.
        let per_pixel = (row_groups, col_groups);
        let per_row = (img.height() as u32).div_ceil(WORKGROUP).min(MAX_GROUPS_PER_DIM);
        let per_col = (img.width() as u32).div_ceil(WORKGROUP).min(MAX_GROUPS_PER_DIM);
        let mut names: Vec<&str> = vec!["seed"];
        let mut groups: Vec<(u32, u32)> = vec![per_pixel];
        for _ in 0..self.passes {
            if self.running {
                names.push("box_h_run");
                groups.push((per_row, 1));
                names.push("box_v_run");
                groups.push((per_col, 1));
            } else {
                names.push("box_h");
                groups.push(per_pixel);
                names.push("box_v");
                groups.push(per_pixel);
            }
        }
        names.push("emit");
        groups.push(per_pixel);

        let rw = wgpu::BufferBindingType::Storage { read_only: false };
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
        let label: &'static str = Box::leak(format!("box_probe_{}", self.r).into_boxed_str());
        let (layout, pipelines) = ctx.cache.program(
            device,
            label,
            SHADER,
            &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, rw),
                entry(3, rw),
            ],
            &names,
        );

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("box-probe-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut p = [0u8; 32];
        p[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        p[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        p[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        p[12..16].copy_from_slice(&row_groups.to_le_bytes());
        p[16..20].copy_from_slice(&self.r.to_le_bytes());
        ctx.queue.write_buffer(&uniform, 0, &p);

        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
        ];
        let entries: Vec<wgpu::BindGroupEntry<'_>> = bound
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("box-probe-bind"),
            layout: &layout,
            entries: &entries,
        });
        for (compute, (gx, gy)) in pipelines.iter().zip(&groups) {
            pass.set_pipeline(compute);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(*gx, *gy, 1);
        }
    }
    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use std::time::Instant;

    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut px = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            let t = (i % 4096) as f32 / 4096.0 * 1.2;
            px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
        }
        ImageBuf::from_vec(w, h, px)
    }

    #[test]
    #[ignore]
    fn probe_box() {
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no adapter");
            return;
        };
        let ctx: Arc<GpuContext> = ctx.clone();
        let (w, h) = (3200usize, 2136);
        let img = scene(w, h);
        println!("{w}x{h} ({:.1} Mpx) on {}", (w * h) as f64 / 1e6, ctx.adapter_name());

        let run = |r: u32, passes: usize, running: bool| -> f64 {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let chain = crate::GpuChain::new(
                    ctx.clone(),
                    vec![Box::new(BoxProbe::new(r, passes, running)) as Box<dyn GpuStage>],
                );
                let c = img.clone();
                let t = Instant::now();
                let out = chain.apply(c);
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(&out);
            }
            best
        };

        // ⚠ Zero passes is the same chain with the same transfer and no blur at all, so the
        // subtraction isolates the blur rather than pricing the round trip again.
        // ⚠⚠ **The crossover is SWEPT, not guessed** — `MIN_GPU_BOX_PX` and `MAX_TILE_DABS` were
        // each swept and each turned out to be wrong by 8x and 10x respectively when reasoned
        // about instead. Three passes, the shape both consumers actually use.
        let sweep_base = run(0, 0, false);
        println!("  crossover sweep, 3 passes:");
        for r in [1u32, 2, 3, 4, 6, 8, 10, 16, 24, 40] {
            let naive = run(r, 3, false) - sweep_base;
            let running = run(r, 3, true) - sweep_base;
            println!(
                "    r={r:<3} naive {naive:6.2}   running {running:6.2}   {}",
                if naive < running { "naive" } else { "RUNNING" }
            );
        }
        println!();

        let base = run(0, 0, false);
        println!("  transfer only (0 passes)          {base:6.2} ms");
        println!();
        println!("  {:<30} {:>8} {:>8}   {:>6}", "", "naive", "running", "CPU");
        for (label, r, passes, cpu) in [
            ("clarity  wide_blur r=40 x3", 40u32, 3usize, 13.5f64),
            ("texture  guided    r=2  x4", 2, 4, 28.0),
            ("(scaling) r=40 x1", 40, 1, 4.7),
            ("(scaling) r=10 x3", 10, 3, 13.5),
        ] {
            let naive = run(r, passes, false) - base;
            let running = run(r, passes, true) - base;
            println!(
                "  {label:<30} {naive:7.2}  {running:7.2}   {cpu:5.1}   naive {:.2}x  running {:.2}x",
                cpu / naive,
                cpu / running
            );
        }
    }
}

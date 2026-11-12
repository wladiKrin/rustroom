//! Defringe on the GPU (M10.14f) — `pipeline`'s `DefringeStage`, global amounts only: a cheap lightness, its Sobel
//! magnitude, a separable max filter, then the chroma scaled in Oklab beside strong edges.
//!
//! ⚠⚠ Why it exists: defringe is a prefix step AFTER geometry, so with it on a straighten or crop drag paid its
//! ~62 ms (3200 px) on the CPU — and a CPU step there ended the resident GPU handoff from geometry to the tail.
//!
//! ⚠ A mask carrying a LOCAL defringe declines: its coverage would need the mask chain here, and the CPU step
//! folds it in. Global-only is what a drag usually holds.
//!
//! Three scratch planes, their roles changing between the passes: `p0` lightness, then the edge map; `p1` the
//! Sobel magnitude; `p2` the max filter's row pass.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

/// The CPU stage's cost per 6.8 Mpx, measured (2026-09-26, DSCF6263 at 3200 px: 62 ms).
const CPU_COST: f32 = 62.0;

pub struct GpuDefringeStage {
    cpu: pipeline::DefringeStage,
    global: edit_model::Defringe,
    radius: u32,
}

impl GpuDefringeStage {
    /// The twin for `edits`' defringe at `scale`, or `None` — no defringe, or a mask carrying a local one (the CPU
    /// step's `locals`, by the same predicate as `build_prefix_steps`).
    pub fn new(
        edits: &edit_model::Edits,
        scale: RenderScale,
        _ctx: &Arc<GpuContext>,
    ) -> Option<Self> {
        if !edits.defringe.is_active()
            || edits
                .masks
                .iter()
                .any(|m| !m.disabled && m.adjustments.defringe != 0.0)
        {
            return None;
        }
        Some(Self {
            cpu: pipeline::DefringeStage::new(edits.defringe, Vec::new(), scale),
            global: edits.defringe,
            // ⚠ The CPU stage's own rule, so the two search the same neighbourhood at every rung.
            radius: (pipeline::DEFRINGE_RADIUS_FULL * scale.get())
                .ceil()
                .max(1.0) as u32,
        })
    }

    fn shader(&self) -> String {
        format!(
            "{}{}\nconst EDGE_LO: f32 = {:?};\nconst EDGE_HI: f32 = {:?};\nconst FEATHER: f32 = {:?};\n{}",
            crate::oklab_wgsl::OKLAB_WGSL,
            crate::luma_wgsl::luma_wgsl(),
            pipeline::DEFRINGE_EDGE_LO,
            pipeline::DEFRINGE_EDGE_HI,
            pipeline::DEFRINGE_FEATHER_DEG,
            KERNELS
        )
    }
}

const KERNELS: &str = r#"
struct Params {
    w: u32, h: u32, pixels: u32, row_groups: u32,
    r: u32, _p0: u32, _p1: u32, _p2: u32,
    // purple amount, green amount, purple lo, hi, green lo, hi
    amt: vec4<f32>,
    hues: vec4<f32>,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;

fn index(id: vec3<u32>) -> u32 { return id.y * (params.row_groups * 64u) + id.x; }

fn smoothstep_df(lo: f32, hi: f32, x: f32) -> f32 {
    let t = clamp((x - lo) / (hi - lo), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// `defringe::membership`: 1 inside the range, feathered to 0 by circular distance.
fn membership(lo_in: f32, hi_in: f32, h: f32) -> f32 {
    let lo = min(lo_in, hi_in);
    let hi = max(lo_in, hi_in);
    let centre = 0.5 * (lo + hi);
    let half = 0.5 * (hi - lo);
    let raw = h - centre;
    let d0 = raw - 360.0 * floor(raw / 360.0);
    let d = min(d0, 360.0 - d0);
    return 1.0 - smoothstep_df(half, half + FEATHER, d);
}

fn at(x: i32, y: i32) -> f32 {
    let xx = u32(clamp(x, 0, i32(params.w) - 1));
    let yy = u32(clamp(y, 0, i32(params.h) - 1));
    return p0[yy * params.w + xx];
}

@compute @workgroup_size(64)
fn light(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let b = i * 3u;
    p0[i] = cbrt_signed(max(prophoto_luminance(img[b], img[b + 1u], img[b + 2u]), 0.0));
}

@compute @workgroup_size(64)
fn sobel(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let x = i32(i % params.w);
    let y = i32(i / params.w);
    let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
        - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
    let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
        - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
    p1[i] = sqrt(gx * gx + gy * gy) / 8.0;
}

@compute @workgroup_size(64)
fn max_rows(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let x = i32(i % params.w);
    let row = (i / params.w) * params.w;
    let r = i32(params.r);
    var m = 0.0;
    for (var k = max(x - r, 0); k <= min(x + r, i32(params.w) - 1); k = k + 1) {
        m = max(m, p1[row + u32(k)]);
    }
    p2[i] = m;
}

@compute @workgroup_size(64)
fn max_cols(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let x = i % params.w;
    let y = i32(i / params.w);
    let r = i32(params.r);
    var m = 0.0;
    for (var k = max(y - r, 0); k <= min(y + r, i32(params.h) - 1); k = k + 1) {
        m = max(m, p2[u32(k) * params.w + x]);
    }
    p0[i] = m;
}

@compute @workgroup_size(64)
fn apply_defringe(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let e = smoothstep_df(EDGE_LO, EDGE_HI, p0[i]);
    if (e <= 0.0) { return; }
    let b = i * 3u;
    let lab = prophoto_to_oklab(img[b], img[b + 1u], img[b + 2u]);
    let hue = atan2(lab.z, lab.y) * 57.29577951308232;
    let purple = clamp(params.amt.x, 0.0, 100.0) * membership(params.amt.z, params.amt.w, hue);
    let green = clamp(params.amt.y, 0.0, 100.0) * membership(params.hues.x, params.hues.y, hue);
    let wgt = clamp(e * max(purple, green) / 100.0, 0.0, 1.0);
    if (wgt <= 0.0) { return; }
    let k = 1.0 - wgt;
    let out = oklab_to_prophoto(lab.x, lab.y * k, lab.z * k);
    img[b] = out.x;
    img[b + 1u] = out.y;
    img[b + 2u] = out.z;
}
"#;

impl GpuStage for GpuDefringeStage {
    fn name(&self) -> &'static str {
        "defringe"
    }

    fn cpu_cost(&self) -> f32 {
        CPU_COST
    }

    fn scratch_planes(&self) -> usize {
        3
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        let (row_groups, col_groups) = crate::dispatch::Dispatch::grid(img.pixels());
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
        let rw = wgpu::BufferBindingType::Storage { read_only: false };
        let names = ["light", "sobel", "max_rows", "max_cols", "apply_defringe"];
        let (layout, pipelines) = ctx.cache.program(
            device,
            "defringe",
            &self.shader(),
            &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, rw),
                entry(3, rw),
                entry(4, rw),
            ],
            &names,
        );
        let g = &self.global;
        let mut p = [0u8; 64];
        let mut put = |off: usize, bytes: [u8; 4]| p[off..off + 4].copy_from_slice(&bytes);
        put(0, (img.width() as u32).to_le_bytes());
        put(4, (img.height() as u32).to_le_bytes());
        put(8, img.pixels().to_le_bytes());
        put(12, row_groups.to_le_bytes());
        put(16, self.radius.to_le_bytes());
        for (k, v) in [
            g.purple_amount,
            g.green_amount,
            g.purple_hue[0],
            g.purple_hue[1],
            g.green_hue[0],
            g.green_hue[1],
        ]
        .into_iter()
        .enumerate()
        {
            put(32 + 4 * k, v.to_le_bytes());
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("defringe-params"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &p);
        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
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
            label: Some("defringe-bind"),
            layout: &layout,
            entries: &entries,
        });
        // ⚠ In order, in one pass — each reads the plane the one before wrote (the in-pass barrier `GpuChain`
        // measures).
        for pipe in &pipelines {
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(row_groups, col_groups, 1);
        }
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

#[cfg(test)]
#[path = "defringe_tests.rs"]
mod tests;

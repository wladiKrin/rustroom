//! Orientation, keystone, straighten and crop as ONE compute gather: the GPU twin of
//! `pipeline::GeometryStage` (2026-09-26).
//!
//! ⚠⚠ **Why now, after it was declined.** Geometry was left on the CPU because region scoping
//! made 1:1 cheap. But the author's *fitted* view is already rung 3200 (6.8 Mpx), where the CPU
//! resample was **37.4 ms of a ~63 ms Straighten frame**. And the prize is not the resample, it is
//! what keeping the pixels on the GPU makes possible: a resident input and a handoff to the tail
//! with no float download. See `docs/superpowers/specs/2026-09-26-gpu-geometry-design.md`.
//!
//! ⚠⚠ **The map's constants arrive from the CPU as [`pipeline::MapParams`]** — crop origin,
//! rotation, orientation, homography — computed by the very `FusedMap` the CPU renders with. Only
//! the arithmetic that APPLIES them is written twice (here and in `FusedMap::source_of`), in the
//! same order, and the equivalence tests are what hold the two together.
//!
//! ⚠ **The first size-changing GPU stage.** A crop shrinks the image and a quarter turn swaps its
//! axes, so `output_dims` delegates to the CPU twin and the chain relabels after `encode`
//! (`GpuImage::relabel`). The output is written into the same buffer: `seed` copies the input into
//! three scratch planes first (a gather cannot be in place), then `gather` writes the first
//! `ow * oh` pixels.
//!
//! ⚠ It always maps coordinates, where the CPU's no-keystone path permutes indices exactly. At
//! angle 0 every sample lands on a whole pixel, where every kernel's phase-0 weight is 1 at the
//! centre, so the two agree there too; the tests pin it.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::resample::Weights;
use pipeline::{GeometryStage, MapParams, PipelineStage, Resampler};

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

pub struct GpuGeometryStage {
    cpu: GeometryStage,
    params: MapParams,
    weights: Vec<f32>,
    taps: usize,
    phases: usize,
    radius: i32,
    w: usize,
    h: usize,
    out: (usize, usize),
}

impl GpuGeometryStage {
    /// `None` for a frame too small to resample — the CPU passes it through untouched, and the
    /// seam then leaves the CPU stage in place.
    pub fn new(
        geometry: &edit_model::Geometry,
        perspective: edit_model::Perspective,
        resampler: Resampler,
        image: &core_image::ImageBuf,
    ) -> Option<Self> {
        let (w, h) = (image.width(), image.height());
        let cpu = GeometryStage::new(geometry, resampler).with_perspective(perspective);
        let params = cpu.map_params(w, h)?;
        let out = cpu.output_dims(w, h);
        let weights = Weights::new(resampler);
        Some(Self {
            cpu,
            params,
            // ⚠⚠ Substituted, never transcribed — the lens twin's rule.
            taps: weights.taps(),
            phases: weights.phases(),
            radius: resampler.radius() as i32,
            weights: weights.lut().to_vec(),
            w,
            h,
            out,
        })
    }

    fn shader(&self) -> String {
        let taps = self.taps;
        let radius = self.radius;
        // ⚠⚠ Bilinear is computed exactly, not from a table — the lens twin's note applies.
        let axis_body = if self.weights.is_empty() {
            "    wgt[0] = 1.0 - f;\n    wgt[1] = f;".to_string()
        } else {
            format!(
                "    let p = min(u32(f * {phases}.0), {last}u);\n    \
                 for (var i = 0u; i < {taps}u; i = i + 1u) {{ wgt[i] = lut[p * {taps}u + i]; }}",
                phases = self.phases,
                last = self.phases - 1,
            )
        };
        format!(
            r#"
struct Params {{
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    ow: u32,
    out_pixels: u32,
    quarter: u32,
    flags: u32,
    crop_x: f32,
    crop_y: f32,
    rcx: f32,
    rcy: f32,
    sin_a: f32,
    cos_a: f32,
    w1: f32,
    h1: f32,
    scx: f32,
    scy: f32,
    pad0: f32,
    pad1: f32,
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read> lut: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    p0[i] = img[q];
    p1[i] = img[q + 1u];
    p2[i] = img[q + 2u];
}}

fn axis(f: f32) -> array<f32, {taps}u> {{
    var wgt: array<f32, {taps}u>;
{axis_body}
    return wgt;
}}

// `FusedMap::source_of`, step for step and in the same association.
fn source_of(x: f32, y: f32) -> vec2<f32> {{
    var px = params.crop_x + x;
    var py = params.crop_y + y;
    if ((params.flags & 1u) != 0u) {{
        let dx = px - params.rcx;
        let dy = py - params.rcy;
        px = params.rcx + dx * params.cos_a + dy * params.sin_a;
        py = params.rcy - dx * params.sin_a + dy * params.cos_a;
    }}
    var sx = px;
    var sy = py;
    if (params.quarter == 1u) {{ sx = py; sy = params.h1 - px; }}
    else if (params.quarter == 2u) {{ sx = params.w1 - px; sy = params.h1 - py; }}
    else if (params.quarter == 3u) {{ sx = params.w1 - py; sy = px; }}
    if ((params.flags & 2u) != 0u) {{ sx = params.w1 - sx; }}
    if ((params.flags & 4u) != 0u) {{
        let nx = (sx - params.scx) / params.scy;
        let ny = (sy - params.scy) / params.scy;
        let hx = params.m0.x * nx + params.m0.y * ny + params.m0.z;
        let hy = params.m1.x * nx + params.m1.y * ny + params.m1.z;
        let hw = params.m2.x * nx + params.m2.y * ny + params.m2.z;
        var s = vec2<f32>(hx * 1e6, hy * 1e6);
        if (abs(hw) >= 1.1920929e-7) {{ s = vec2<f32>(hx / hw, hy / hw); }}
        sx = params.scx + s.x * params.scy;
        sy = params.scy + s.y * params.scy;
    }}
    return vec2<f32>(sx, sy);
}}

// ⚠ A tap outside the frame contributes ZERO, unrenormalised — `resample::sample`'s rule.
fn sample(sx: f32, sy: f32) -> vec3<f32> {{
    let bx = floor(sx);
    let by = floor(sy);
    let x0 = i32(bx) - {radius} + 1;
    let y0 = i32(by) - {radius} + 1;
    let wx = axis(sx - bx);
    let wy = axis(sy - by);
    var acc = vec3<f32>(0.0, 0.0, 0.0);
    for (var j = 0u; j < {taps}u; j = j + 1u) {{
        let yi = y0 + i32(j);
        if (yi < 0 || yi >= i32(params.h)) {{ continue; }}
        var row = vec3<f32>(0.0, 0.0, 0.0);
        for (var i = 0u; i < {taps}u; i = i + 1u) {{
            let xi = x0 + i32(i);
            if (xi < 0 || xi >= i32(params.w)) {{ continue; }}
            let o = u32(yi) * params.w + u32(xi);
            row = row + wx[i] * vec3<f32>(p0[o], p1[o], p2[o]);
        }}
        acc = acc + wy[j] * row;
    }}
    return acc;
}}

@compute @workgroup_size(64)
fn gather(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.out_pixels) {{ return; }}
    let x = f32(i % params.ow);
    let y = f32(i / params.ow);
    let s = source_of(x, y);
    let v = sample(s.x, s.y);
    let q = i * 3u;
    img[q] = v.x;
    img[q + 1u] = v.y;
    img[q + 2u] = v.z;
}}
"#,
        )
    }

    /// The uniform block, laid out as the shader's `Params` (std140: the three `vec4` rows start
    /// at byte 80, a multiple of 16).
    fn uniform_bytes(&self, row_groups: u32) -> [u8; 128] {
        let p = &self.params;
        let mut b = [0u8; 128];
        let mut put_u = |i: usize, v: u32| b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        put_u(0, self.w as u32);
        put_u(1, self.h as u32);
        put_u(2, (self.w * self.h) as u32);
        put_u(3, row_groups);
        put_u(4, self.out.0 as u32);
        put_u(5, (self.out.0 * self.out.1) as u32);
        put_u(6, p.quarter);
        let flags = u32::from(p.rotation.is_some())
            | (u32::from(p.mirror) << 1)
            | (u32::from(p.inverse.is_some()) << 2);
        put_u(7, flags);
        let rot = p.rotation.unwrap_or([0.0; 4]);
        let inv = p.inverse.unwrap_or([[0.0; 3]; 3]);
        let floats = [
            p.crop_origin[0],
            p.crop_origin[1],
            rot[0],
            rot[1],
            rot[2],
            rot[3],
            p.last[0],
            p.last[1],
            p.centre[0],
            p.centre[1],
            0.0,
            0.0,
            inv[0][0],
            inv[0][1],
            inv[0][2],
            0.0,
            inv[1][0],
            inv[1][1],
            inv[1][2],
            0.0,
            inv[2][0],
            inv[2][1],
            inv[2][2],
            0.0,
        ];
        for (k, f) in floats.iter().enumerate() {
            let i = 8 + k;
            b[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
        b
    }
}

impl GpuStage for GpuGeometryStage {
    fn name(&self) -> &'static str {
        "geometry"
    }

    /// The CPU resample measured 37.4 ms at 6.8 Mpx in a real Straighten drag (`TIMING`, rung
    /// 3200). ⚠ It is the one stage whose chain is worth more than its own compute, because of the
    /// resident input and the handoff it enables.
    fn cpu_cost(&self) -> f32 {
        37.0
    }

    fn scratch_planes(&self) -> usize {
        3
    }

    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        self.cpu.output_dims(w, h)
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        // ⚠ One grid over the INPUT pixels serves both kernels: the output never has more pixels
        // than the input, and `gather` returns early past `out_pixels`.
        let total = img.pixels().div_ceil(WORKGROUP);
        let row_groups = total.min(MAX_GROUPS_PER_DIM);
        let col_groups = total.div_ceil(row_groups);

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
        let ro = wgpu::BufferBindingType::Storage { read_only: true };
        let layout_entries = vec![
            entry(0, rw),
            entry(1, wgpu::BufferBindingType::Uniform),
            entry(2, rw),
            entry(3, rw),
            entry(4, rw),
            entry(5, ro),
        ];
        let (layout, pipelines) = ctx.cache.program(
            device,
            "geometry",
            &self.shader(),
            &layout_entries,
            &["seed", "gather"],
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("geometry-params"),
            size: 128,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&uniform, 0, &self.uniform_bytes(row_groups));
        // ⚠ `.max(4)`: wgpu refuses a zero-sized buffer and bilinear's table is empty.
        let lut = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("geometry-lut"),
            size: (self.weights.len() * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !self.weights.is_empty() {
            ctx.queue
                .write_buffer(&lut, 0, bytemuck::cast_slice(&self.weights));
        }
        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
            lut.as_entire_binding(),
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
            label: Some("geometry-bind"),
            layout: &layout,
            entries: &entries,
        });
        for p in pipelines.iter().take(2) {
            pass.set_pipeline(p);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(row_groups, col_groups, 1);
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
    use edit_model::{CropRect, Geometry, Orientation, Perspective, Quarter};
    use std::sync::Arc;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// Structure everywhere and an odd size, so a wrong centre, a transposed axis or an
    /// off-by-one cannot coincide with the right answer.
    fn chart(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let grid = if x % 9 < 2 || y % 7 < 2 { 0.85 } else { 0.1 };
                img.set_pixel(
                    x,
                    y,
                    [grid, x as f32 / w as f32, 0.2 + 0.6 * y as f32 / h as f32],
                );
            }
        }
        img
    }

    fn cases() -> Vec<(&'static str, Geometry, Perspective)> {
        let crop = CropRect {
            left: 0.1,
            top: 0.05,
            right: 0.85,
            bottom: 0.9,
        };
        let key = Perspective {
            vertical: 35.0,
            horizontal: -20.0,
            ..Default::default()
        };
        let o = |rotate, mirror| Orientation { rotate, mirror };
        let g = |orientation, angle, crop| Geometry {
            orientation,
            angle,
            crop,
        };
        vec![
            (
                "crop only",
                g(o(Quarter::Zero, false), 0.0, crop),
                Perspective::default(),
            ),
            (
                "cw90 mirror",
                g(o(Quarter::Cw90, true), 0.0, CropRect::default()),
                Perspective::default(),
            ),
            (
                "cw270",
                g(o(Quarter::Cw270, false), 0.0, crop),
                Perspective::default(),
            ),
            (
                "cw180",
                g(o(Quarter::Cw180, false), 0.0, CropRect::default()),
                Perspective::default(),
            ),
            (
                "angle +3",
                g(o(Quarter::Zero, false), 3.0, CropRect::default()),
                Perspective::default(),
            ),
            (
                "angle -3 crop",
                g(o(Quarter::Zero, false), -3.0, crop),
                Perspective::default(),
            ),
            ("keystone", Geometry::default(), key),
            ("everything", g(o(Quarter::Cw90, true), 2.5, crop), key),
        ]
    }

    /// ⚠⚠ **The twin against the CPU stage it replaces, over every term of the map.** First
    /// proves each case actually moves pixels (against a plain crop of the input at the output
    /// size), or agreement would prove nothing.
    #[test]
    fn every_case_renders_the_same_picture_as_the_cpu() {
        let Some(ctx) = ctx() else { return };
        let src = chart(97, 61);
        for r in [
            Resampler::Bilinear,
            Resampler::CatmullRom,
            Resampler::Lanczos3,
        ] {
            for (name, g, p) in cases() {
                let cpu = GeometryStage::new(&g, r).with_perspective(p);
                let want = cpu.apply(src.clone());
                let (ow, oh) = (want.width(), want.height());
                if name != "crop only" {
                    let naive = src.crop(0, 0, ow.min(97), oh.min(61));
                    assert!(
                        (ow, oh) != (naive.width(), naive.height()) || naive.data() != want.data(),
                        "{name}: fixture check, the case moved nothing"
                    );
                }
                let stage = GpuGeometryStage::new(&g, p, r, &src).expect("a resamplable frame");
                let chain = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]);
                let got = chain.apply(src.clone());
                assert_eq!(
                    chain.transfer_counts(),
                    (1, 1),
                    "{name}: the GPU path must have run"
                );
                assert_eq!((got.width(), got.height()), (ow, oh), "{name} {r:?}: size");
                let d = crate::compare(&want, &got);
                eprintln!(
                    "MEASURE {name:14} {r:?}: worst {} levels over {:.3}%",
                    d.worst_levels,
                    d.differing_percent()
                );
                // ⚠⚠ **Two bounds, from MEASUREMENT (2026-09-26), not borrowed.** The lens twin's
                // 0.01 failed here, and lowering the bar blind is the mistake this project records.
                // Measured on this fixture: orientation/crop cases **0** (Lanczos 1.5e-5: its
                // phase-0 weights are not exactly [0, 1, 0]); bilinear rotations and keystones
                // **<= 0.0114**; the TABLE kernels **<= 0.198**. The cause of the last: the GPU
                // compiler fuses multiply-adds in `source_of`, which moves a coordinate by an ulp,
                // and near a boundary of the 1024-phase table that selects the neighbouring phase.
                // The lens twin escapes it because its displacements are precomputed on the CPU.
                // 0.2 of an 8-bit level is invisible; 0.25 leaves 25% headroom. ⚠ Do not widen
                // it to make a failure pass — find out what moved.
                let resampled = g.angle != 0.0 || p.is_active();
                let bound = if resampled { 0.25 } else { 1e-4 };
                assert!(
                    d.worst_levels <= bound,
                    "{name} {r:?}: worst {} levels over {:.3}% of samples",
                    d.worst_levels,
                    d.differing_percent()
                );
            }
        }
    }

    #[test]
    fn a_frame_too_small_to_resample_is_declined() {
        let tiny = ImageBuf::new(1, 1);
        assert!(GpuGeometryStage::new(
            &Geometry {
                angle: 3.0,
                ..Default::default()
            },
            Perspective::default(),
            Resampler::default(),
            &tiny
        )
        .is_none());
    }
}

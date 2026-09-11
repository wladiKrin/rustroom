//! Sharpening as compute shaders — the first stage that reads its neighbours.
//!
//! ⚠⚠ **Eight dispatches (five with masking off) inside ONE compute pass**, which M8.3b measured
//! as carrying a memory barrier between dispatches. The separable blurs go
//! `luma → temp → blurred`, and every plane is borrowed from the chain's shared scratch — so the
//! image buffer itself is still read and written in place and the chain's contract is unchanged.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Sharpening;
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// ⚠ Mirrors `stages::sharpening`'s private constants, the same trade `hsl.rs` made: `pipeline`
/// keeps them private and widening its API for one consumer is what this project declined when it
/// refused `pub mod stages`. **The equivalence tests are what would catch them drifting.**
const MAX_AMOUNT: f32 = 1.5;
const HALO_KNEE: f32 = 0.05;
const MASK_SIGMA: f32 = 2.0;
const MASK_MAX: f32 = 0.08;
const LUMA_EPS: f32 = 1e-3;
const GAIN_MAX: f32 = 4.0;

/// ⚠⚠ **`x + j` first, THEN subtract with a floor.** WGSL indices are **unsigned**, so the CPU's
/// `(x + i).saturating_sub(r).min(w - 1)` cannot be transliterated: `x + j - r` wraps to a huge
/// number whenever `x < r`, which then clamps to `w - 1` instead of `0`. That is a fringe down the
/// **left and top edges only** — every interior pixel is correct, so it hides from any test that
/// looks at the middle. Both edges have their own test for exactly that reason.
const SHADER: &str = r#"
struct Params {
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    r: u32,
    mr: u32,
    use_mask: u32,
    pad0: u32,
    k: f32,
    s: f32,
    hi: f32,
    pad1: f32,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> kern: array<f32>;
@group(0) @binding(3) var<storage, read> mkern: array<f32>;
@group(0) @binding(4) var<storage, read_write> pl: array<f32>;
@group(0) @binding(5) var<storage, read_write> pb: array<f32>;
@group(0) @binding(6) var<storage, read_write> pd: array<f32>;
@group(0) @binding(7) var<storage, read_write> pm: array<f32>;

fn index(id: vec3<u32>) -> u32 {
    return id.y * (params.row_groups * 64u) + id.x;
}

// ⚠ The unsigned-index rule, written once so no pass can get it wrong on its own.
fn clampi(pos: u32, j: u32, r: u32, hi: u32) -> u32 {
    let t = pos + j;
    return min(select(0u, t - r, t >= r), hi - 1u);
}

@compute @workgroup_size(64)
fn luma_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let p = i * 3u;
    // ⚠ The WORKING SPACE's luma row, uploaded from `color::PROPHOTO_LUMA` rather than written
    // here — a Rec.709 triple would be the seventh copy of the constant that became wrong the day
    // the working space changed.
    pl[i] = LUMA_R * img[p] + LUMA_G * img[p + 1u] + LUMA_B * img[p + 2u];
}

@compute @workgroup_size(64)
fn blur_h(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * params.r; j = j + 1u) {
        acc = acc + kern[j] * pl[y * params.w + clampi(x, j, params.r, params.w)];
    }
    pd[i] = acc;
}

@compute @workgroup_size(64)
fn blur_v(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * params.r; j = j + 1u) {
        acc = acc + kern[j] * pd[clampi(y, j, params.r, params.h) * params.w + x];
    }
    pb[i] = acc;
}

@compute @workgroup_size(64)
fn difference(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    pd[i] = pl[i] - pb[i];
}

@compute @workgroup_size(64)
fn magnitude(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    pm[i] = abs(pd[i]);
}

@compute @workgroup_size(64)
fn mask_h(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * params.mr; j = j + 1u) {
        acc = acc + mkern[j] * pm[y * params.w + clampi(x, j, params.mr, params.w)];
    }
    pb[i] = acc;
}

@compute @workgroup_size(64)
fn mask_v(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * params.mr; j = j + 1u) {
        acc = acc + mkern[j] * pb[clampi(y, j, params.mr, params.h) * params.w + x];
    }
    pm[i] = smoothstep(0.0, params.hi, acc);
}

@compute @workgroup_size(64)
fn apply_gain(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let dv = pd[i];
    let q = abs(dv) / HALO_KNEE;
    let supp = params.s + (1.0 - params.s) / (1.0 + q * q);
    var m = 1.0;
    if (params.use_mask == 1u) { m = pm[i]; }
    let delta = params.k * dv * supp * m;
    // ⚠ The near-black guard is load-bearing, not defensive: without it a dark pixel beside a
    // bright edge divides by ~0 and the gain explodes.
    let gain = clamp(1.0 + delta / max(pl[i], LUMA_EPS), 0.0, GAIN_MAX);
    let p = i * 3u;
    img[p] = img[p] * gain;
    img[p + 1u] = img[p + 1u] * gain;
    img[p + 2u] = img[p + 2u] * gain;
}
"#;

/// The pass list, in dispatch order. Masking adds the middle three.
const PASSES: [&str; 5] = ["luma_pass", "blur_h", "blur_v", "difference", "apply_gain"];
const MASK_PASSES: [&str; 3] = ["magnitude", "mask_h", "mask_v"];

/// Sharpening, on the GPU.
pub struct GpuSharpenStage {
    /// ⚠⚠ The CPU's OWN weights, from `pipeline::spatial::gaussian_kernel`, uploaded rather than
    /// recomputed in WGSL — the rule `GpuHslStage` already follows for the hue table.
    kernel: Vec<f32>,
    mask_kernel: Vec<f32>,
    k: f32,
    s: f32,
    hi: f32,
    masking: bool,
    cpu: pipeline::SharpenStage,
}

impl GpuSharpenStage {
    pub fn new(params: &Sharpening, scale: RenderScale, _ctx: &Arc<GpuContext>) -> Self {
        let sigma = params
            .radius
            .clamp(pipeline::RADIUS_RANGE.0, pipeline::RADIUS_RANGE.1)
            * scale.get();
        let masking = params.masking.clamp(0.0, 100.0);
        Self {
            kernel: pipeline::spatial::gaussian_kernel(sigma),
            mask_kernel: pipeline::spatial::gaussian_kernel(MASK_SIGMA * scale.get()),
            k: (params.amount / 100.0).clamp(0.0, 1.0) * MAX_AMOUNT,
            s: (params.detail / 100.0).clamp(0.0, 1.0),
            hi: MASK_MAX * masking / 100.0,
            // ⚠ Masking 0 skips the three mask dispatches entirely, matching the CPU's
            // `Option<Vec<f32>>` — both the cheap path and what avoids `smoothstep(0, 0, ..)`
            // being degenerate.
            masking: masking > 0.0,
            cpu: pipeline::SharpenStage::new(params, scale),
        }
    }

    fn params_bytes(&self, img: &GpuImage, row_groups: u32) -> [u8; 48] {
        let r = ((self.kernel.len() - 1) / 2) as u32;
        let mr = ((self.mask_kernel.len() - 1) / 2) as u32;
        let mut b = [0u8; 48];
        b[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[16..20].copy_from_slice(&r.to_le_bytes());
        b[20..24].copy_from_slice(&mr.to_le_bytes());
        b[24..28].copy_from_slice(&u32::from(self.masking).to_le_bytes());
        b[32..36].copy_from_slice(&self.k.to_le_bytes());
        b[36..40].copy_from_slice(&self.s.to_le_bytes());
        b[40..44].copy_from_slice(&self.hi.to_le_bytes());
        b
    }
}

impl GpuStage for GpuSharpenStage {
    /// ⚠ Derived from the bench's 1600x1068 delta scaled by 3.9, not measured at 3200 — see
    /// `GpuHslStage::cpu_cost`. ⚠ Spatial, so unlike the per-pixel stages its cost is only
    /// roughly linear in pixel count; the margin over the bar is large enough not to care.
    fn cpu_cost(&self) -> f32 {
        54.0
    }

    fn name(&self) -> &'static str {
        "sharpening"
    }

    /// Luma, blurred, difference, mask.
    ///
    /// ⚠ Four whatever the masking setting, so the chain's allocation does not depend on a slider
    /// — a stage whose memory footprint changed with its parameters would make the chain's
    /// `scratch_needed` a function of the edits rather than of its stages.
    fn scratch_planes(&self) -> usize {
        4
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        let total_groups = img.pixels().div_ceil(WORKGROUP);
        let row_groups = total_groups.min(MAX_GROUPS_PER_DIM);
        let col_groups = total_groups.div_ceil(row_groups);

        // ⚠ The luma weights are substituted into the source rather than passed as uniforms, so
        // the shader carries no numeric literal of its own for them.
        let src = SHADER
            .replace("LUMA_R", &format!("{:?}", color::PROPHOTO_LUMA[0]))
            .replace("LUMA_G", &format!("{:?}", color::PROPHOTO_LUMA[1]))
            .replace("LUMA_B", &format!("{:?}", color::PROPHOTO_LUMA[2]))
            .replace("HALO_KNEE", &format!("{HALO_KNEE:?}"))
            .replace("LUMA_EPS", &format!("{LUMA_EPS:?}"))
            .replace("GAIN_MAX", &format!("{GAIN_MAX:?}"));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sharpen"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sharpen-params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&uniform, 0, &self.params_bytes(img, row_groups));

        let upload = |data: &[f32], label: &'static str| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (data.len() * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(data));
            b
        };
        let kern = upload(&self.kernel, "sharpen-kernel");
        let mkern = upload(&self.mask_kernel, "sharpen-mask-kernel");

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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sharpen-layout"),
            entries: &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, ro),
                entry(3, ro),
                entry(4, rw),
                entry(5, rw),
                entry(6, rw),
                entry(7, rw),
            ],
        });
        // ⚠ Written out rather than built by a helper closure: a closure returning a
        // `BindGroupEntry` cannot express that its borrow outlives its own call, and the compiler
        // says so.
        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            kern.as_entire_binding(),
            mkern.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
            scratch[3].as_entire_binding(),
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
            label: Some("sharpen-bind"),
            layout: &layout,
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sharpen-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        // ⚠ The mask passes sit between `difference` and `apply_gain` because `apply_gain` reads
        // the mask and `magnitude` reads the difference — the order is the data dependency, not a
        // convention.
        let names: Vec<&str> = if self.masking {
            PASSES[..4]
                .iter()
                .chain(MASK_PASSES.iter())
                .chain(PASSES[4..].iter())
                .copied()
                .collect()
        } else {
            PASSES.to_vec()
        };
        for name in names {
            let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("sharpen-pipeline"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            });
            pass.set_pipeline(&compute);
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

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// ⚠⚠ **A hard edge, not a ramp.** Sharpening is *about* edges, and a smooth ramp would let a
    /// wrong kernel or wrong edge replication pass — the mistake the M8.4 demosaic probe made by
    /// landing on road-sign lettering instead of foliage.
    ///
    /// ⚠ The edge is at `x = w/3` rather than the middle, so the left band is **narrower than the
    /// kernel reach is wide** at the sizes used here: an implementation that replicated the left
    /// edge wrongly has somewhere for that to show.
    fn edged(w: usize, h: usize) -> ImageBuf {
        let mut d = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 3 { 0.15 } else { 0.75 };
                let v = if y < h / 4 { v * 0.6 } else { v };
                // A little colour, so a luma-weighting error is visible.
                d.extend_from_slice(&[v, v * 0.9, v * 1.1]);
            }
        }
        ImageBuf::from_vec(w, h, d)
    }

    fn params(masking: f32) -> Sharpening {
        Sharpening {
            amount: 70.0,
            radius: 1.5,
            detail: 50.0,
            masking,
        }
    }

    fn gpu(ctx: &Arc<GpuContext>, p: &Sharpening, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuSharpenStage::new(p, RenderScale::FULL, ctx))],
        )
        .apply(img)
    }

    fn worst(p: &Sharpening, ctx: &Arc<GpuContext>, w: usize, h: usize) -> f32 {
        let img = edged(w, h);
        let cpu = pipeline::SharpenStage::new(p, RenderScale::FULL).apply(img.clone());
        let out = gpu(ctx, p, img);
        crate::compare(&cpu, &out).worst_levels
    }

    #[test]
    fn the_gpu_matches_the_cpu_with_masking_off() {
        let Some(ctx) = ctx() else { return };
        let d = worst(&params(0.0), &ctx, 96, 72);
        assert!(d < 0.01, "worst {d} levels");
    }

    #[test]
    fn the_gpu_matches_the_cpu_with_masking_on() {
        // ⚠ A different code path: a second Gaussian and a smoothstep. One test without the other
        // covers half the stage.
        let Some(ctx) = ctx() else { return };
        let d = worst(&params(60.0), &ctx, 96, 72);
        assert!(d < 0.01, "worst {d} levels");
    }

    #[test]
    fn the_left_and_top_edges_match_the_cpus_replication() {
        // ⚠⚠ **The unsigned-underflow guard.** `x + j - r` wraps to a huge number when `x < r`,
        // clamping to `w - 1` instead of `0` — a fringe down the **left and top edges only**,
        // every interior pixel correct.
        //
        // ⚠ Honestly: the whole-image equivalence tests DO catch this too, because their fixture
        // includes its own edges — verified by sabotage, all three fail together. What this one
        // adds is the **diagnosis**: it fails naming a coordinate, so the next person sees "left
        // edge at (0,17)" rather than a worst-case level over the whole frame. Kept for that, and
        // because an equivalence fixture could later be inset without anyone noticing this went
        // uncovered.
        let Some(ctx) = ctx() else { return };
        let p = params(0.0);
        let (w, h) = (64usize, 48usize);
        let img = edged(w, h);
        let cpu = pipeline::SharpenStage::new(&p, RenderScale::FULL).apply(img.clone());
        let out = gpu(&ctx, &p, img);
        let at = |x: usize, y: usize| {
            let i = (y * w + x) * 3;
            crate::compare(
                &ImageBuf::from_vec(1, 1, cpu.data()[i..i + 3].to_vec()),
                &ImageBuf::from_vec(1, 1, out.data()[i..i + 3].to_vec()),
            )
            .worst_levels
        };
        for y in 0..h {
            for x in 0..5usize {
                assert!(
                    at(x, y) < 0.01,
                    "left edge at ({x},{y}): {} levels",
                    at(x, y)
                );
            }
        }
        for x in 0..w {
            for y in 0..5usize {
                assert!(
                    at(x, y) < 0.01,
                    "top edge at ({x},{y}): {} levels",
                    at(x, y)
                );
            }
        }
    }

    #[test]
    fn it_asks_for_four_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuSharpenStage::new(&params(60.0), RenderScale::FULL, &ctx).scratch_planes(),
            4
        );
    }

    #[test]
    fn the_stage_really_sharpens_rather_than_copying() {
        // ⚠⚠ The equivalence tests above would ALL PASS against a stage that did nothing, because
        // they compare the GPU against the CPU and a sabotage deleting both would agree perfectly.
        // This is what says the fixture is one the stage actually moves.
        let Some(ctx) = ctx() else { return };
        let img = edged(96, 72);
        let out = gpu(&ctx, &params(0.0), img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 5.0,
            "the fixture must be one sharpening changes; moved only {} levels",
            d.worst_levels
        );
    }
}

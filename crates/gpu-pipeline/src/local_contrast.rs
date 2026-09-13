//! Texture and Clarity as compute shaders — the last unported mask sub-stage.
//!
//! ⚠⚠ **Up to 17 dispatches inside ONE compute pass over FIVE shared scratch planes**, the
//! `sharpen.rs` shape. M8.3b measured WebGPU as carrying a memory barrier between dispatches
//! within a pass, which is what lets a dependent sequence this long run without submitting.
//!
//! ⚠⚠ **Both bands read the luma plane and NEVER each other's output.** That is what makes the
//! two sliders exactly additive rather than sequential, and a shader that chained them would be a
//! different stage wearing the same name — a plausible photograph rather than an error.

use crate::box_wgsl::{box_pass, use_running_sum};
use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::LocalContrast;
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// Which thread count a pass wants.
///
/// ⚠⚠ **A running-sum pass wants one thread per LINE, not per pixel.** Dispatching `w*h` threads
/// for 2136 rows of work prices the scheduler rather than the algorithm, and every thread beyond
/// the line count returns immediately having done nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grid {
    Pixel,
    Rows,
    Cols,
}

/// The preamble: bindings, the pixel index, the edge rule, and the two shaped helpers.
///
/// ⚠⚠ Every constant is **substituted** from `pipeline`'s own definition, never transcribed. A
/// substituted constant cannot disagree; a transcribed one is merely checked for agreeing — the
/// distinction `luma_wgsl`'s doc records, and the reason `color`'s doc records six copies of a
/// Rec.709 triple that all went wrong on one day.
fn preamble(gauss_r: usize) -> String {
    format!(
        r#"
struct Params {{
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    pad3: u32,
    k_t: f32,
    k_c: f32,
    halo_floor: f32,
    pad4: f32,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> kern: array<f32>;
@group(0) @binding(3) var<storage, read_write> p0: array<f32>;
@group(0) @binding(4) var<storage, read_write> p1: array<f32>;
@group(0) @binding(5) var<storage, read_write> p2: array<f32>;
@group(0) @binding(6) var<storage, read_write> p3: array<f32>;
@group(0) @binding(7) var<storage, read_write> p4: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// ⚠ The unsigned-index rule, written once. WGSL indices are unsigned, so the CPU's
// `(x + i).saturating_sub(r).min(w - 1)` cannot be transliterated: `x + j - r` wraps to a huge
// number whenever `x < r` and then clamps to `w - 1` instead of `0`.
//
// ⚠⚠ This is `gaussian_blur`'s rule — **edge replication** — and it is deliberately NOT the box
// blur's, which shrinks its window instead. The two give different answers at a border and only
// the box's keeps a constant plane exactly constant.
fn clampi(pos: u32, j: u32, r: u32, hi: u32) -> u32 {{
    let t = pos + j;
    return min(select(0u, t - r, t >= r), hi - 1u);
}}

// ⚠ `t = l / (l + MID_GRAY)` maps mid-gray to exactly 0.5 and needs no `pow`, which is why the
// CPU prefers it to an `l^(1/2.2)` proxy.
fn midtone_weight(l: f32) -> f32 {{
    let t = l / (l + {mid_gray}f);
    return 4.0 * t * (1.0 - t);
}}

// ⚠ `powi(2)` on the CPU, so `q * q` here rather than `pow(q, 2.0)` — which would compute an
// exponential and a logarithm for a squaring, and need not round the same way.
fn halo_suppression(d: f32, floor: f32) -> f32 {{
    let q = abs(d) / {halo_knee}f;
    return floor + (1.0 - floor) / (1.0 + q * q);
}}

@compute @workgroup_size(64)
fn luma_pass(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let p = i * 3u;
    p0[i] = prophoto_luminance(img[p], img[p + 1u], img[p + 2u]);
    // ⚠⚠ Scratch planes are NOT zeroed by the chain, and the delta accumulates. Folded in here
    // rather than given a dispatch of its own.
    p1[i] = 0.0;
}}

@compute @workgroup_size(64)
fn copy_l_to_work(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p2[i] = p0[i];
}}

@compute @workgroup_size(64)
fn copy_l_to_aux(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p3[i] = p0[i];
}}

@compute @workgroup_size(64)
fn square(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let v = p0[i];
    p2[i] = v * v;
}}

@compute @workgroup_size(64)
fn gauss_h(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * {gr}u; j = j + 1u) {{
        acc = acc + kern[j] * p0[y * params.w + clampi(x, j, {gr}u, params.w)];
    }}
    p4[i] = acc;
}}

@compute @workgroup_size(64)
fn gauss_v(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let y = i / params.w;
    let x = i - y * params.w;
    var acc = 0.0;
    for (var j = 0u; j <= 2u * {gr}u; j = j + 1u) {{
        acc = acc + kern[j] * p4[clampi(y, j, {gr}u, params.h) * params.w + x];
    }}
    p2[i] = acc;
}}

// ⚠⚠ Reads BOTH planes before writing EITHER — it overwrites its own inputs in place.
@compute @workgroup_size(64)
fn slope_offset(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let mgsq = p2[i];
    let mg = p3[i];
    // ⚠⚠ `cov` and `var` are the SAME expression in the self-guided case, and only `var` is
    // clamped — `guided_filter`'s own comment says why: unlike a variance, a covariance may
    // legitimately be negative when `p` and `guide` are anti-correlated.
    //
    // ⚠⚠ **A SABOTAGE COLLAPSING THIS TO `var / (var + eps)` IS RECORDED AS NEAR-INERT RATHER
    // THAN COUNTED, because in the self-guided case the two are equal in exact arithmetic.** The
    // only way `cov` goes negative here is float cancellation on nearly-flat areas, so the
    // difference is rounding noise: measured at **4.7e-6**, which is *below* the GPU/CPU baseline
    // divergence of **1.05e-5** in the same regime (values of magnitude 8, where FMA contraction
    // dominates). No test in this crate can separate the two, and one that appeared to would be
    // measuring the baseline.
    //
    // ⚠ Kept anyway, and deliberately: it costs nothing, it mirrors the CPU line for line, and
    // the distinction becomes load-bearing the moment anything drives this shader with `p` and
    // `guide` genuinely different — which is what dehaze already does on the CPU side.
    let cov = mgsq - mg * mg;
    let vr = max(cov, 0.0);
    let s = cov / (vr + {guided_eps}f);
    p2[i] = s;
    p3[i] = mg - s * mg;
}}

@compute @workgroup_size(64)
fn acc_texture(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p1[i] = p1[i] + params.k_t * (p0[i] - p2[i]);
}}

@compute @workgroup_size(64)
fn acc_texture_guided(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let lv = p0[i];
    // The guided base, `mean_slope * l + mean_offset`, never materialised as a plane of its own.
    let base = p2[i] * lv + p3[i];
    p1[i] = p1[i] + params.k_t * (lv - base);
}}

@compute @workgroup_size(64)
fn acc_clarity(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let lv = p0[i];
    let d = lv - p2[i];
    p1[i] = p1[i] + params.k_c * d * midtone_weight(lv) * halo_suppression(d, params.halo_floor);
}}

@compute @workgroup_size(64)
fn apply_gain(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    // ⚠ The near-black guard is load-bearing, exactly as in sharpening: without it a dark pixel
    // beside a bright edge divides by ~0 and the gain explodes.
    // ⚠⚠ Clamped at 0 below, so the stage can never emit a negative sample and needs no separate
    // clamp — the structural property `VignetteStage` also has, and the one `compare` is blind to.
    let gain = clamp(1.0 + p1[i] / max(p0[i], {luma_eps}f), 0.0, {gain_max}f);
    let p = i * 3u;
    img[p] = img[p] * gain;
    img[p + 1u] = img[p + 1u] * gain;
    img[p + 2u] = img[p + 2u] * gain;
}}
"#,
        mid_gray = pipeline::MID_GRAY,
        halo_knee = pipeline::CLARITY_HALO_KNEE,
        guided_eps = pipeline::TEXTURE_GUIDED_EPS,
        luma_eps = pipeline::LOCAL_CONTRAST_LUMA_EPS,
        gain_max = pipeline::LOCAL_CONTRAST_GAIN_MAX,
        gr = gauss_r,
    )
}

/// Local contrast — Texture and Clarity — on the GPU.
pub struct GpuLocalContrastStage {
    k_t: f32,
    k_c: f32,
    /// ⚠ The CPU's OWN weights, from `spatial::gaussian_kernel`, uploaded rather than recomputed.
    kernel: Vec<f32>,
    /// Texture's guided-filter radius; Clarity's box radius.
    tex_r: usize,
    cla_r: usize,
    cpu: pipeline::LocalContrastStage,
}

impl GpuLocalContrastStage {
    pub fn new(params: &LocalContrast, scale: RenderScale, _ctx: &Arc<GpuContext>) -> Self {
        let s = scale.get();
        Self {
            k_t: (params.texture / 100.0).clamp(-1.0, 1.0),
            k_c: (params.clarity / 100.0).clamp(-1.0, 1.0),
            kernel: pipeline::spatial::gaussian_kernel(pipeline::TEXTURE_SIGMA * s),
            tex_r: pipeline::texture_guided_radius(scale),
            cla_r: pipeline::spatial::wide_blur_radius(pipeline::CLARITY_SIGMA * s),
            cpu: pipeline::LocalContrastStage::new(params, scale),
        }
    }

    /// The dispatch sequence, in order, each pass with the grid it wants.
    ///
    /// ⚠ Texture finishes into `p1` **before** Clarity starts, which is what lets the two bands
    /// share `p2`..`p4` and keeps the stage at five planes rather than seven.
    fn passes(&self) -> Vec<(&'static str, Grid)> {
        let mut v = vec![("luma_pass", Grid::Pixel)];
        let (tb, cb) = (box_grids(self.tex_r), box_grids(self.cla_r));
        if self.k_t < 0.0 {
            // The self-guided filter: two statistic blurs, one slope/offset kernel, two more.
            v.push(("square", Grid::Pixel));
            v.extend([("tbox_h2", tb.0), ("tbox_v2", tb.1)]);
            v.push(("copy_l_to_aux", Grid::Pixel));
            v.extend([("tbox_h3", tb.0), ("tbox_v3", tb.1)]);
            v.push(("slope_offset", Grid::Pixel));
            v.extend([("tbox_h2", tb.0), ("tbox_v2", tb.1)]);
            v.extend([("tbox_h3", tb.0), ("tbox_v3", tb.1)]);
            v.push(("acc_texture_guided", Grid::Pixel));
        } else if self.k_t > 0.0 {
            v.extend([
                ("gauss_h", Grid::Pixel),
                ("gauss_v", Grid::Pixel),
                ("acc_texture", Grid::Pixel),
            ]);
        }
        if self.k_c != 0.0 {
            v.push(("copy_l_to_work", Grid::Pixel));
            for _ in 0..pipeline::spatial::WIDE_PASSES {
                v.extend([("cbox_h", cb.0), ("cbox_v", cb.1)]);
            }
            v.push(("acc_clarity", Grid::Pixel));
        }
        v.push(("apply_gain", Grid::Pixel));
        v
    }

    fn shader(&self) -> String {
        let gauss_r = (self.kernel.len() - 1) / 2;
        let mut src = crate::luma_wgsl::luma_wgsl();
        src.push_str(&preamble(gauss_r));
        // ⚠ Emitted whatever the sliders say, so one shader text serves every path. The program
        // cache keys on that text, so a rung with different radii compiles its own — correct, and
        // a handful of programs rather than one per frame.
        src.push_str(&box_pass(
            "tbox_h2", "tbox_v2", "p2", "p2", "p4", self.tex_r,
        ));
        src.push_str(&box_pass(
            "tbox_h3", "tbox_v3", "p3", "p3", "p4", self.tex_r,
        ));
        src.push_str(&box_pass("cbox_h", "cbox_v", "p2", "p2", "p4", self.cla_r));
        src
    }
}

/// The `(h, v)` grids a box blur at this radius wants.
fn box_grids(r: usize) -> (Grid, Grid) {
    if use_running_sum(r) {
        (Grid::Rows, Grid::Cols)
    } else {
        (Grid::Pixel, Grid::Pixel)
    }
}

impl GpuStage for GpuLocalContrastStage {
    fn name(&self) -> &'static str {
        "local_contrast"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`, three runs, **after** the self-guided
    /// filter was slimmed — setting this against the old code would be the M8.3h staleness
    /// failure exactly: `texture_minus` read 50.4-66.8 ms before that landed and 38.8-39.2 after.
    ///
    /// | band | measured | ships |
    /// |---|---|---|
    /// | `texture_plus` | 17.93-18.55 | **17.9** |
    /// | `texture_minus` | 38.78-39.19 | **38.7** |
    /// | `clarity` | 21.93-22.33 | **21.9** |
    ///
    /// ⚠ Each at the **bottom** of its band — the conservative direction, since a lower number
    /// makes a run less likely to chain and the gate must fail toward the CPU.
    ///
    /// ⚠⚠ The **max** of the two bands, never their sum. Both together measured 32.77-33.66,
    /// which is *more* than either alone but *less* than both added (the luma extraction and the
    /// gain pass are shared) — so a max is always an underestimate, which is the safe side.
    fn cpu_cost(&self) -> f32 {
        let t: f32 = if self.k_t < 0.0 {
            38.7
        } else if self.k_t > 0.0 {
            17.9
        } else {
            0.0
        };
        let c = if self.k_c != 0.0 { 21.9 } else { 0.0 };
        t.max(c)
    }

    /// Luma, delta, work, aux, tmp.
    ///
    /// ⚠ **Five whatever the sliders say**, so the chain's allocation does not depend on an edit —
    /// the rule `GpuSharpenStage::scratch_planes` states. 136 MB at 6.8 Mpx against sharpening's
    /// 109, and it is allocated per **chain** rather than per stage, so a chain containing this
    /// stage pays it whether or not its neighbours want it.
    fn scratch_planes(&self) -> usize {
        5
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
        let rows = (img.height() as u32).div_ceil(WORKGROUP);
        let cols = (img.width() as u32).div_ceil(WORKGROUP);

        let src = self.shader();
        let seq = self.passes();
        // ⚠ De-duplicated, because `tbox_h2` is dispatched twice and a program's entry list is a
        // set. The dispatch loop looks the pipeline up by name rather than by position.
        let mut names: Vec<&'static str> = Vec::new();
        for (n, _) in &seq {
            if !names.contains(n) {
                names.push(n);
            }
        }

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

        let (layout, pipelines) = ctx.cache.program(
            device,
            "local_contrast",
            &src,
            &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, ro),
                entry(3, rw),
                entry(4, rw),
                entry(5, rw),
                entry(6, rw),
                entry(7, rw),
            ],
            &names,
        );

        let mut b = [0u8; 48];
        b[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[32..36].copy_from_slice(&self.k_t.to_le_bytes());
        b[36..40].copy_from_slice(&self.k_c.to_le_bytes());
        b[40..44].copy_from_slice(&pipeline::CLARITY_HALO_FLOOR.to_le_bytes());

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("local-contrast-params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &b);

        let kern = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("local-contrast-kernel"),
            size: (self.kernel.len() * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&kern, 0, bytemuck::cast_slice(&self.kernel));

        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            kern.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
            scratch[3].as_entire_binding(),
            scratch[4].as_entire_binding(),
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
            label: Some("local-contrast-bind"),
            layout: &layout,
            entries: &entries,
        });

        for (name, grid) in &seq {
            let idx = names.iter().position(|n| n == name).expect("pass compiled");
            let (gx, gy) = match grid {
                Grid::Pixel => (row_groups, col_groups),
                Grid::Rows => (rows, 1),
                Grid::Cols => (cols, 1),
            };
            pass.set_pipeline(&pipelines[idx]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
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

    /// Native scale, so Texture takes the **naive** box shader at r=2 and Clarity the
    /// **running-sum** one at r=40 — the two production radii, either side of the threshold, in
    /// one fixture.
    fn scale() -> RenderScale {
        RenderScale::new(1.0)
    }
    const W: usize = 128;
    const H: usize = 96;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// ⚠⚠ **Structure at BOTH scales, or the stage is inert and every shader passes.** A flat
    /// plane has `l - base == 0` everywhere, so it cannot distinguish a correct blur from a
    /// missing one. This carries:
    ///
    /// - a **coarse ramp** across the frame, which is what Clarity's 40 px band can see;
    /// - a **fine 6 px checker**, which only Texture's band can;
    /// - a **hard edge**, which is where the halo suppressor bites;
    /// - a **near-black spot in a bright field**, which drives the gain strongly negative;
    /// - values **below 0 and above 1**, because the working space is scene-referred and a shader
    ///   that clamped would be correct on a 0..1 chart and silently destroy every highlight.
    fn scene() -> ImageBuf {
        let mut px = Vec::with_capacity(W * H * 3);
        for i in 0..W * H {
            let (x, y) = (i % W, i / W);
            let ramp = x as f32 / W as f32 * 1.4 - 0.1;
            let fine = if (x / 3 + y / 3) % 2 == 0 {
                0.05
            } else {
                -0.05
            };
            let edge = if y > H / 2 { 0.35 } else { 0.0 };
            let mut v = ramp + fine + edge;
            // The dark spot: `l` near zero beside bright neighbours, so `l - base` is large and
            // negative and the gain goes below zero.
            if (40..48).contains(&x) && (40..48).contains(&y) {
                v = 0.0004;
            }
            px.extend_from_slice(&[v, v * 0.92, v * 1.06]);
        }
        ImageBuf::from_vec(W, H, px)
    }

    fn params(texture: f32, clarity: f32) -> LocalContrast {
        LocalContrast { texture, clarity }
    }

    fn chained(ctx: &Arc<GpuContext>, p: &LocalContrast, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuLocalContrastStage::new(p, scale(), ctx))],
        )
        .apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous.** A stage that did nothing would match
    /// the CPU perfectly on a fixture the CPU also leaves alone — which is how creative LUTs (W5)
    /// shipped fully tested and 100% dead.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        for (t, c) in [(70.0, 0.0), (-70.0, 0.0), (0.0, 70.0)] {
            let img = scene();
            let out = pipeline::LocalContrastStage::new(&params(t, c), scale()).apply(img.clone());
            let moved = img
                .data()
                .iter()
                .zip(out.data())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                moved > img.data().len() / 2,
                "fixture check: texture {t} clarity {c} must move most of the fixture, \
                 moved {moved} of {}",
                img.data().len()
            );
        }
    }

    /// ⚠ Bounded in 8-bit levels, not bit-identical: this stage is full of multiply-adds and the
    /// GPU fuses them, rounding once where the CPU rounds twice. See `contrast`'s
    /// `the_only_divergence_is_fused_multiply_add` for the class.
    ///
    /// ⚠⚠ **Both signs of Texture, because they take genuinely different code paths** — positive
    /// measures against a Gaussian and negative against a guided filter. A test covering one sign
    /// is blind to half the stage, which is how `GpuExposureStage`'s first test missed the only
    /// real divergence this crate had found at the time.
    #[test]
    fn every_band_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (t, c) in [
            (70.0f32, 0.0f32),
            (-70.0, 0.0),
            (0.0, 70.0),
            (0.0, -70.0),
            // ⚠ The only case that exercises the shared luma plane and the summed delta at once —
            // and the only one that could catch a shader chaining the bands instead of adding them.
            (60.0, 50.0),
            (-60.0, 50.0),
        ] {
            let p = params(t, c);
            let cpu = pipeline::LocalContrastStage::new(&p, scale()).apply(scene());
            let gpu = chained(&ctx, &p, scene());
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.5,
                "texture {t} clarity {c}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠⚠ **The two sliders are additive, and only this can see a shader that chained them.**
    /// Both bands read the luma plane and never each other's output — a chained implementation
    /// renders a plausible photograph that is simply not the sum of the two sliders, and every
    /// single-band test passes against it.
    ///
    /// ⚠⚠ **Compared against the CPU's OWN residual, not against zero — the first version of this
    /// test asserted zero and was wrong about correct code.** `out(t,c) == out(t,0) + out(0,c) -
    /// src` holds exactly while the gain is unclamped, and this fixture deliberately drives it
    /// past `GAIN_MAX` and below 0, because that is what `no_gain_flips_a_samples_sign` needs.
    /// Where a clamp bites, additivity legitimately breaks — **for the CPU too**. Measuring both
    /// residuals and comparing them tests the shader rather than the model, and still fails
    /// loudly against a chained implementation.
    #[test]
    fn the_two_bands_are_additive() {
        let Some(ctx) = ctx() else { return };
        let (t, c) = (60.0, 50.0);
        let src = scene();
        let residual = |both: &ImageBuf, x: &ImageBuf, y: &ImageBuf| -> Vec<f32> {
            both.data()
                .iter()
                .zip(x.data())
                .zip(y.data())
                .zip(src.data())
                .map(|(((&b, &p), &q), &s)| b - (p + q - s))
                .collect()
        };
        let cpu_of =
            |p: LocalContrast| pipeline::LocalContrastStage::new(&p, scale()).apply(scene());
        let cpu = residual(
            &cpu_of(params(t, c)),
            &cpu_of(params(t, 0.0)),
            &cpu_of(params(0.0, c)),
        );
        let gpu = residual(
            &chained(&ctx, &params(t, c), scene()),
            &chained(&ctx, &params(t, 0.0), scene()),
            &chained(&ctx, &params(0.0, c), scene()),
        );
        // ⚠ The fixture check: additivity must be something this fixture can actually observe. If
        // the CPU's residual were large everywhere, matching it would say nothing at all.
        let exact = cpu.iter().filter(|v| v.abs() < 1e-5).count();
        assert!(
            exact > cpu.len() / 2,
            "fixture check: the bands must be exactly additive over most of the fixture, {exact} \
             of {} samples",
            cpu.len()
        );
        let worst = cpu
            .iter()
            .zip(&gpu)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1e-3,
            "the GPU must be additive wherever the CPU is; worst divergence {worst}"
        );
    }

    /// ⚠⚠ **`compare` quantizes to 8-bit levels, where a negative sample and 0 are THE SAME
    /// VALUE** — so it is structurally blind to a missing clamp, and every test above stays green
    /// with the floor deleted. That blind spot is M8.3e's finding about the output transform's
    /// clamp, and it appeared three more times in M8.3m.
    ///
    /// ⚠⚠ **What the clamp guarantees is about the GAIN, not the sample — and asserting the sample
    /// was wrong about correct code.** The gain is a scalar multiply, so a scene-referred
    /// **negative input** stays negative however the gain is clamped: this fixture's first pixel
    /// is -0.05 and comes back -0.2 under a gain of 4, correctly, on both paths. What
    /// `clamp(.., 0.0, ..)` prevents is a **negative gain**, whose signature is a sample whose
    /// **sign flips** — invisible to a level comparison for exactly the same reason.
    #[test]
    fn no_gain_flips_a_samples_sign() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Texture is un-weighted — no midtone weight, no halo suppressor — so it drives the gain
        // hardest. The fixture's near-black spot sits in a bright field, so `l - base` is large
        // and negative there and `1 + delta / max(l, LUMA_EPS)` goes far below zero.
        let p = params(100.0, 0.0);
        let src = scene();
        let cpu = pipeline::LocalContrastStage::new(&p, scale()).apply(scene());
        assert!(
            cpu.data()
                .iter()
                .zip(src.data())
                .any(|(o, i)| *o == 0.0 && *i != 0.0),
            "fixture check: the CPU must actually reach the clamp here"
        );
        let gpu = chained(&ctx, &p, scene());
        for (i, (&out, &inp)) in gpu.data().iter().zip(src.data()).enumerate() {
            assert!(
                out == 0.0 || out.signum() == inp.signum(),
                "sample {i}: input {inp} came back {out} — the gain went negative, which a level \
                 comparison cannot see"
            );
        }
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what six order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        let p = params(10.0, 10.0);
        assert_eq!(
            GpuLocalContrastStage::new(&p, scale(), &ctx).name(),
            pipeline::LocalContrastStage::new(&p, scale()).name()
        );
    }

    /// ⚠⚠ **The radii this stage really asks for, either side of the swept threshold.** Texture
    /// takes the naive shader and Clarity the running-sum one at every scale — pinned here so a
    /// change to `TEXTURE_SIGMA`, `CLARITY_SIGMA` or `RUNNING_SUM_MIN_RADIUS` cannot silently move
    /// a band onto the shader that loses for it.
    #[test]
    fn each_band_gets_the_shader_the_sweep_chose() {
        let Some(ctx) = ctx() else { return };
        for scale in [1.0f32, 0.5, 0.25] {
            let s = RenderScale::new(scale);
            let st = GpuLocalContrastStage::new(&params(50.0, 50.0), s, &ctx);
            assert!(
                !use_running_sum(st.tex_r),
                "Texture at scale {scale} has r={} and must take the naive shader",
                st.tex_r
            );
            assert!(
                use_running_sum(st.cla_r),
                "Clarity at scale {scale} has r={} and must take the running-sum shader",
                st.cla_r
            );
        }
    }
}

//! Dehaze as compute shaders — the first **prefix** stage to move.
//!
//! ⚠⚠ **The airlight stays on the CPU and arrives as three uniforms.** It is already estimated on
//! a **fixed 256 px downscale** rather than at the render resolution — a deliberate decision, so
//! the estimate is render-scale-independent and the image does not shift on zoom — which makes it
//! negligible to compute *and* makes both paths identical **by construction** rather than by
//! comparison. Two estimates of "what colour is the haze" would be a different photograph, not an
//! error.

use crate::box_wgsl::box_pass;
use crate::min_wgsl::min_pass;
use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Dehaze;
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// Which thread count a pass wants — the box blur's running body is per **line**, not per pixel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grid {
    Pixel,
    Rows,
    Cols,
}

/// Bindings, the pixel index, and the two shaped helpers.
///
/// Plane roles: `p0` guide (luma), `p1` dark channel, `p2` transmission (coarse then refined),
/// `p3` aux statistic, `p4` tmp, `p5` second aux, `p6` third aux.
///
/// ⚠⚠ Seven planes rather than five, and the reason is that dehaze's guided filter is
/// **cross**-guided: `p` is the transmission map and `guide` is luma, so M8.3n's self-guided
/// shortcut does not apply and all **six** statistic blurs are genuinely needed.
fn preamble() -> String {
    format!(
        r#"
struct Params {{
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    remove: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    ar: f32,
    ag: f32,
    ab: f32,
    mag: f32,
    tolerance: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read_write> p3: array<f32>;
@group(0) @binding(6) var<storage, read_write> p4: array<f32>;
@group(0) @binding(7) var<storage, read_write> p5: array<f32>;
@group(0) @binding(8) var<storage, read_write> p6: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// ⚠ `dist.max(1e-6)` on the CPU, so the same floor here — without it a pixel exactly at the
// airlight divides by zero.
fn tolerant(t: f32, dist: f32) -> f32 {{
    if (params.tolerance <= 0.0) {{ return t; }}
    return min(t * max(params.tolerance / max(dist, 1e-6), 1.0), 1.0);
}}

@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    let r = img[q]; let g = img[q + 1u]; let b = img[q + 2u];
    p0[i] = prophoto_luminance(r, g, b);
    // min over channels of I_c / A_c — the dark channel before the patch minimum.
    p1[i] = min(r / params.ar, min(g / params.ag, b / params.ab));
}}

// ⚠ `dist` is the **largest per-channel** distance from the airlight: a pixel is "the airlight's
// colour" only when EVERY channel is within the tolerance. Recomputed from `img` rather than
// stored, which costs three reads and saves a plane.
@compute @workgroup_size(64)
fn coarse(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    let dist = max(
        abs(img[q] - params.ar),
        max(abs(img[q + 1u] - params.ag), abs(img[q + 2u] - params.ab))
    );
    p2[i] = tolerant(1.0 - {omega}f * p1[i], dist);
}}

// The guided filter's statistics. `p` is p2 (the coarse transmission), `guide` is p0 (luma).
@compute @workgroup_size(64)
fn stats(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let g = p0[i];
    p3[i] = g;          // -> mean_guide
    p5[i] = g * g;      // -> mean_guide_sq
    p6[i] = g * p2[i];  // -> mean_cross
    p1[i] = p2[i];      // -> mean_p  (p1's dark channel is spent by now)
}}

// ⚠⚠ The covariance is NOT clamped and the variance IS — `guided_filter`'s own comment says why:
// unlike a variance, a covariance may legitimately be negative when `p` and `guide` are
// anti-correlated. Here they genuinely can be, because this is CROSS-guided.
@compute @workgroup_size(64)
fn slope_offset(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let mg = p3[i];
    let mp = p1[i];
    let mgsq = p5[i];
    let mc = p6[i];
    let vr = max(mgsq - mg * mg, 0.0);
    let cov = mc - mg * mp;
    let s = cov / (vr + {eps}f);
    p3[i] = s;              // -> mean_slope
    p1[i] = mp - s * mg;    // -> mean_offset
}}

@compute @workgroup_size(64)
fn combine(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p2[i] = p3[i] * p0[i] + p1[i];
}}

@compute @workgroup_size(64)
fn apply_dehaze(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let tk = 1.0 - params.mag * (1.0 - p2[i]);
    let q = i * 3u;
    let a = array<f32, 3>(params.ar, params.ag, params.ab);
    for (var c = 0u; c < 3u; c = c + 1u) {{
        var v: f32;
        if (params.remove == 1u) {{
            // ⚠⚠ **A sabotage lowering `T_MIN` to 0.0001 is recorded as NEAR-INERT rather
            // than counted, and the reason is a feature rather than a weak fixture.** The floor
            // bites only where the refined transmission falls below 0.1 — and M7.6's airlight
            // tolerance RAISES `t` toward 1 for exactly those pixels, because a transmission that
            // low means the pixel is nearly the airlight's own colour, which is what the tolerance
            // is for. So the two guards overlap by construction and no photographic fixture
            // reaches this floor. Kept because it mirrors the CPU line for line and because the
            // tolerance is a slider-free constant somebody may one day turn down.
            v = (img[q + c] - a[c]) / max(tk, {t_min}f) + a[c];
        }} else {{
            v = img[q + c] * tk + a[c] * (1.0 - tk);
        }}
        // ⚠ A pixel darker than the airlight can be driven negative by the division; clamped at
        // 0, the convention every other stage uses — and one `compare` is structurally blind to.
        img[q + c] = max(v, 0.0);
    }}
}}
"#,
        omega = pipeline::OMEGA,
        eps = pipeline::REFINE_EPS,
        t_min = pipeline::DEHAZE_T_MIN,
    )
}

/// Dehaze, on the GPU.
pub struct GpuDehazeStage {
    /// ⚠ The frame-global replay airlight, supplied by the caller — never estimated here.
    airlight: [f32; 3],
    mag: f32,
    remove: bool,
    patch_r: usize,
    refine_r: usize,
    cpu: pipeline::DehazeStage,
}

impl GpuDehazeStage {
    /// ⚠⚠ **Takes the airlight rather than the image, and that is the fix for a defect found by
    /// use on 2026-09-17.** This used to call `pipeline::airlight(image)` — estimating from the
    /// frame it was handed, which under a Straighten drag is the ROTATED frame, so the angle fed
    /// straight into the estimate. The reporter saw the dehazed tone breathe past ±25°, and only
    /// on the global dehaze: a mask's capture stages run in the prefix, `accelerate` is only ever
    /// called on the tail, so the masked one stayed on the CPU and was already invariant.
    ///
    /// ⚠ The module header's claim that the two paths are *"identical by construction"* is true
    /// for SCALE — the estimate is taken on a fixed 256 px downscale — and was false for GEOMETRY.
    /// Supplying the value is what makes it true for both.
    ///
    /// ⚠⚠ **Required rather than `Option`.** A `None` arm would be a silent fallback to
    /// estimating, which is exactly how this survived. Unrepresentable beats documented.
    pub fn new(
        params: &Dehaze,
        scale: RenderScale,
        airlight: [f32; 3],
        _ctx: &Arc<GpuContext>,
    ) -> Self {
        let k = (params.amount / 100.0).clamp(-1.0, 1.0);
        Self {
            airlight,
            mag: k.abs(),
            remove: k > 0.0,
            patch_r: pipeline::scaled_radius(pipeline::PATCH_RADIUS_FULL, scale),
            refine_r: pipeline::scaled_radius(pipeline::REFINE_RADIUS_FULL, scale),
            // ⚠ The twin takes the SAME supplied value, so the fallback cannot disagree with
            // the shader about what colour the haze is.
            cpu: pipeline::DehazeStage::with_airlight(params, scale, airlight),
        }
    }

    fn passes(&self) -> Vec<(&'static str, Grid)> {
        let rb = if crate::box_wgsl::use_running_sum(self.refine_r) {
            (Grid::Rows, Grid::Cols)
        } else {
            (Grid::Pixel, Grid::Pixel)
        };
        let mut v = vec![("seed", Grid::Pixel)];
        // The dark channel: a separable minimum, p1 <-> p4.
        v.extend([("min_h", Grid::Pixel), ("min_v", Grid::Pixel)]);
        v.push(("coarse", Grid::Pixel));
        v.push(("stats", Grid::Pixel));
        // Four statistic blurs — all six of the general form, because this is cross-guided.
        for (h, vv) in [
            ("gb_h3", "gb_v3"),
            ("gb_h5", "gb_v5"),
            ("gb_h6", "gb_v6"),
            ("gb_h1", "gb_v1"),
        ] {
            v.extend([(h, rb.0), (vv, rb.1)]);
        }
        v.push(("slope_offset", Grid::Pixel));
        // Two more: mean_slope and mean_offset.
        v.extend([("gb_h3", rb.0), ("gb_v3", rb.1)]);
        v.extend([("gb_h1", rb.0), ("gb_v1", rb.1)]);
        v.push(("combine", Grid::Pixel));
        v.push(("apply_dehaze", Grid::Pixel));
        v
    }

    fn shader(&self) -> String {
        let mut src = crate::luma_wgsl::luma_wgsl();
        src.push_str(&preamble());
        src.push_str(&min_pass("min_h", "min_v", "p1", "p1", "p4", self.patch_r));
        for (h, v, plane) in [
            ("gb_h1", "gb_v1", "p1"),
            ("gb_h3", "gb_v3", "p3"),
            ("gb_h5", "gb_v5", "p5"),
            ("gb_h6", "gb_v6", "p6"),
        ] {
            src.push_str(&box_pass(h, v, plane, plane, "p4", self.refine_r));
        }
        src
    }
}

impl GpuStage for GpuDehazeStage {
    fn name(&self) -> &'static str {
        "dehaze"
    }

    /// Measured whole-buffer at 3123x2085 (6.5 Mpx) by `bench_preview`'s native block: **100.9 ms**.
    ///
    /// ⚠ Ships at the bottom of that reading, the conservative direction — a lower number makes a
    /// step less likely to chain, and the gate must fail toward the CPU.
    fn cpu_cost(&self) -> f32 {
        100.0
    }

    /// ⚠ Seven, because the cross-guided filter needs four statistics where the self-guided form
    /// needs two. 190 MB at 6.5 Mpx — the largest allocation any stage in this crate asks for.
    fn scratch_planes(&self) -> usize {
        7
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
        let rows = (img.height() as u32).div_ceil(WORKGROUP);
        let cols = (img.width() as u32).div_ceil(WORKGROUP);

        let src = self.shader();
        let seq = self.passes();
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
        let mut layout_entries = vec![entry(0, rw), entry(1, wgpu::BufferBindingType::Uniform)];
        for b in 2..9 {
            layout_entries.push(entry(b, rw));
        }
        let (layout, pipelines) =
            ctx.cache
                .program(device, "dehaze", &src, &layout_entries, &names);

        let mut b = [0u8; 64];
        b[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[16..20].copy_from_slice(&u32::from(self.remove).to_le_bytes());
        b[32..36].copy_from_slice(&self.airlight[0].to_le_bytes());
        b[36..40].copy_from_slice(&self.airlight[1].to_le_bytes());
        b[40..44].copy_from_slice(&self.airlight[2].to_le_bytes());
        b[44..48].copy_from_slice(&self.mag.to_le_bytes());
        b[48..52].copy_from_slice(&pipeline::AIRLIGHT_TOLERANCE.to_le_bytes());

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dehaze-params"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &b);

        let mut bound = vec![img.buffer.as_entire_binding(), uniform.as_entire_binding()];
        for s in scratch.iter().take(7) {
            bound.push(s.as_entire_binding());
        }
        let entries: Vec<wgpu::BindGroupEntry<'_>> = bound
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dehaze-bind"),
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

    /// ⚠ Scale 1.0, so the radii are the unscaled ones: `PATCH_RADIUS_FULL = 20` and
    /// `REFINE_RADIUS_FULL = 60`. That puts the guided filter's r=60 **above** M8.3n's crossover
    /// so it takes the running body, and the dark channel's r=20 below it — both production
    /// shapes in one fixture.
    fn scale() -> RenderScale {
        RenderScale::new(1.0)
    }

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// ⚠⚠ **The CPU stage's own fixture, and its three properties are load-bearing** — its tests
    /// record that three earlier versions were wrong and why. (1) **Periodic dark cells on a 40 px
    /// pitch**, so the dark-channel prior the model assumes actually holds; a smooth gradient
    /// measures how the model degrades rather than whether we implemented it. (2) **Transmission
    /// reaching 0.05**, because the airlight can only be *observed* where transmission is near
    /// zero — a fixture topping out at 0.3 gives the estimator nothing to find. (3) **Large
    /// enough that the guided reach — TWICE its radius — fits**, which is 120 px at r=60.
    fn hazy(w: usize, h: usize) -> ImageBuf {
        let a = [0.85f32, 0.87, 0.9];
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let dark = x % 40 < 6 && y % 40 < 6;
                let clear = if dark {
                    [0.01, 0.01, 0.01]
                } else {
                    [
                        0.05 + 0.7 * u,
                        0.05 + 0.6 * v,
                        0.05 + 0.5 * (1.0 - u) * (1.0 - v),
                    ]
                };
                let t = 0.95 - 0.9 * u;
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = clear[c] * t + a[c] * (1.0 - t);
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn params(amount: f32) -> Dehaze {
        Dehaze { amount }
    }

    fn chained(ctx: &Arc<GpuContext>, p: &Dehaze, img: ImageBuf) -> ImageBuf {
        let stage = GpuDehazeStage::new(p, scale(), pipeline::airlight(&img), ctx);
        crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous** — a stage that did nothing would match
    /// the CPU perfectly on a fixture the CPU also leaves alone. The W5 failure mode.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = hazy(200, 160);
        for amount in [70.0f32, -70.0] {
            let out = pipeline::DehazeStage::new(&params(amount), scale()).apply(img.clone());
            let moved = img
                .data()
                .iter()
                .zip(out.data())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                moved > img.data().len() / 2,
                "fixture check: dehaze {amount} must move most of the fixture, moved {moved}"
            );
        }
    }

    /// ⚠⚠ **The airlight must be the SUPPLIED one, and this is the only test that can see a second
    /// estimate.** Two readings of "what colour is the haze" would render a different photograph
    /// rather than fail.
    ///
    /// ⚠⚠ **Re-expressed 2026-09-17, because its old form pinned a defect.** It asserted the stage
    /// carried `pipeline::airlight(&img)` — the frame's own estimate — which is exactly what made
    /// the dehazed tone breathe under a Straighten drag: the frame handed over is the ROTATED one.
    /// Its intent was always right; only its expression named the wrong source.
    ///
    /// ⚠ The value below is deliberately **not** the frame's own estimate. Feeding the stage
    /// `airlight(&img)` and then asserting it holds `airlight(&img)` is a tautology that passes
    /// whether the stage stores the argument or re-derives it.
    #[test]
    fn the_airlight_is_the_supplied_one() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let supplied = [0.11, 0.22, 0.33];
        assert_ne!(
            supplied,
            pipeline::airlight(&img),
            "fixture check: the supplied value must differ from the frame's own estimate, or this \
             test cannot tell them apart"
        );
        let stage = GpuDehazeStage::new(&params(60.0), scale(), supplied, &ctx);
        assert_eq!(
            stage.airlight, supplied,
            "the GPU stage must carry the airlight it was given, never one of its own"
        );
    }

    /// ⚠⚠ The property the report was about: **the angle must not move the GPU stage's airlight.**
    ///
    /// Reported in use 2026-09-17 with the GPU on — the global dehaze breathed past ±25° while the
    /// masked one did not, because a mask's capture stages run in the prefix and `accelerate` is
    /// only ever called on the tail.
    #[test]
    fn the_angle_does_not_move_the_gpu_airlight() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let mut want: Option<[f32; 3]> = None;
        for angle in [0.0f32, 10.0, 25.0, 30.0, 40.0] {
            let mut e = edit_model::Edits {
                dehaze: params(60.0),
                ..Default::default()
            };
            e.geometry.angle = angle;
            let air = pipeline::prefix_airlight(&e, &img, pipeline::Resampler::default(), None);
            let stage = GpuDehazeStage::new(&e.dehaze, scale(), air, &ctx);
            match want {
                None => want = Some(stage.airlight),
                Some(w) => assert_eq!(
                    stage.airlight, w,
                    "{angle} deg moved the GPU stage's airlight"
                ),
            }
        }
    }

    /// ⚠ Bounded in 8-bit levels, not bit-identical: dehaze is full of divides and multiply-adds,
    /// so it inherits both FMA contraction (M8.3m) and the division class M8.3n found.
    ///
    /// ⚠ **Both signs**, because they are different expressions — positive inverts the model and
    /// negative applies it — and one direction only is how `GpuExposureStage`'s first test missed
    /// the one real divergence this crate had at the time.
    #[test]
    fn both_signs_match_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for amount in [70.0f32, -70.0, 100.0] {
            let img = hazy(200, 160);
            let cpu = pipeline::DehazeStage::new(&params(amount), scale()).apply(img.clone());
            let gpu = chained(&ctx, &params(amount), img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "dehaze {amount}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠⚠ **`compare` quantizes to 8-bit levels, where a negative sample and 0 are THE SAME
    /// VALUE** — so it is structurally blind to a missing clamp, and every test above stays green
    /// with the floor deleted. M8.3e's finding about the output transform, met for the sixth time.
    ///
    /// ⚠ Dehaze reaches it readily: a pixel darker than the airlight is driven negative by the
    /// division `(I - A) / t_k`, which is exactly what the fixture's dark cells are.
    #[test]
    fn no_sample_comes_back_negative() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let cpu = pipeline::DehazeStage::new(&params(100.0), scale()).apply(img.clone());
        assert!(
            cpu.data().contains(&0.0),
            "fixture check: the CPU must actually reach the clamp here"
        );
        let gpu = chained(&ctx, &params(100.0), img);
        for (i, v) in gpu.data().iter().enumerate() {
            assert!(
                *v >= 0.0,
                "sample {i} came back {v} — a level comparison cannot see this"
            );
        }
    }

    /// ⚠ The name is what `PrefixStepKey`'s neighbours and six order-asserting tests match on.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(64, 64);
        assert_eq!(
            GpuDehazeStage::new(&params(50.0), scale(), pipeline::airlight(&img), &ctx).name(),
            pipeline::DehazeStage::new(&params(50.0), scale()).name()
        );
    }
}

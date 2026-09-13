//! Noise reduction as compute shaders — the second **prefix** stage to move, and the cheaper
//! case of the guided filter M8.3o shipped.
//!
//! ⚠⚠ **Self-guided, where dehaze is cross-guided.** `guided_filter(&lum, &lum, ..)` passes the
//! same slice twice, which `spatial::guided_filter`'s pointer-identity shortcut collapses to
//! **two** statistic blurs — dehaze's transmission map is guided by luma and needs all six. So
//! this is `local_contrast`'s negative-Texture shape, not `dehaze`'s.
//!
//! ⚠⚠ **FOUR scratch planes, against dehaze's seven, and that is the milestone's one real design
//! choice.** The prefix always runs whole-buffer, so this is the SECOND stage ever to allocate at
//! full resolution — 99.4 MiB per plane on an X-T4 — and the first one killed the render worker.
//! The two phases are sequenced so the chroma half re-uses planes the luminance half has finished
//! with, and `apply_nr` **recomputes** `prophoto_to_oklab` rather than storing three original
//! planes. That recompute is three `cbrt` and two matrices against 398 MiB, and it is
//! bit-identical by construction: same bytes, same function, and `img` is not written until
//! `apply_nr` itself.

use crate::box_wgsl::{box_pass, use_running_sum};
use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::NoiseReduction;
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

/// The `(h, v)` grids a box blur at this radius wants.
fn box_grids(r: usize) -> (Grid, Grid) {
    if use_running_sum(r) {
        (Grid::Rows, Grid::Cols)
    } else {
        (Grid::Pixel, Grid::Pixel)
    }
}

/// Bindings, the pixel index, and the seeding and applying kernels.
///
/// Plane roles, and they change between the two phases **deliberately**:
/// - luminance: `p0` the guide L, `p1`/`p2` the statistics then slope/offset then the guided
///   result in `p1`, `p3` the blur tmp.
/// - chroma: `p0` the a channel, `p2` the b channel, `p3` the blur tmp. `p1` holds the guided
///   luminance across this phase and must not be touched.
///
/// ⚠⚠ **`eps` is a UNIFORM, where `box_pass`'s radius is a literal — and the asymmetry is
/// deliberate.** A literal puts the value in the shader text, which is the program cache's key:
/// `eps` moves with every step of the Detail slider, so a literal would compile a fresh program on
/// every frame of a drag. The radii move only with Smoothness and the render scale, a handful of
/// values, and `box_wgsl`'s own doc says a literal radius is what lets the compiler see a constant
/// trip count.
fn preamble() -> String {
    r#"
struct Params {
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    s_lum: f32,
    s_col: f32,
    eps: f32,
    pad0: f32,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read_write> p3: array<f32>;

fn index(id: vec3<u32>) -> u32 {
    return id.y * (params.row_groups * 64u) + id.x;
}

// The guide: Oklab L, which the guided filter both filters and is guided by.
@compute @workgroup_size(64)
fn seed_l(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let q = i * 3u;
    p0[i] = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]).x;
}

@compute @workgroup_size(64)
fn square(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let v = p0[i];
    p1[i] = v * v;
}

@compute @workgroup_size(64)
fn copy_l(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    p2[i] = p0[i];
}

// ⚠⚠ `cov` and `var` are the SAME expression in the self-guided case, and only `var` is clamped —
// `guided_filter`'s own comment says why: unlike a variance, a covariance may legitimately be
// negative when `p` and `guide` are anti-correlated, which cannot happen when they are the same
// plane. Kept as two names because it mirrors the CPU line for line.
//
// ⚠⚠ **A SABOTAGE DROPPING THIS CLAMP IS RECORDED AS NEAR-INERT RATHER THAN COUNTED, and it is
// M8.3n's finding arriving in a second stage.** Because the two are equal in exact arithmetic, the
// only way `cov` goes negative here is float cancellation on nearly-flat areas — so the difference
// is rounding noise, measured there at 4.7e-6, which is *below* the GPU/CPU baseline divergence of
// 1.05e-5 in the same regime. No test in this crate can separate the two, and one that appeared to
// would be measuring the baseline. Verified: removing it left all 10 tests green.
//
// ⚠ Kept anyway, and deliberately: it costs nothing, it mirrors the CPU line for line, and the
// distinction becomes load-bearing the moment anything drives this shader with `p` and `guide`
// genuinely different — which is what dehaze already does.
//
// ⚠ Reads BOTH planes before writing EITHER — it overwrites its own inputs in place.
@compute @workgroup_size(64)
fn slope_offset(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let mgsq = p1[i];
    let mg = p2[i];
    let cov = mgsq - mg * mg;
    let vr = max(cov, 0.0);
    let s = cov / (vr + params.eps);
    p1[i] = s;
    p2[i] = mg - s * mg;
}

// `mean_slope * guide + mean_offset`, landing in p1 so the chroma phase may take p0 and p2.
@compute @workgroup_size(64)
fn combine_lum(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    p1[i] = p1[i] * p0[i] + p2[i];
}

// ⚠ Seeds the chroma channels into planes the luminance phase has finished with. `p1` is the
// guided luminance and is NOT touched.
@compute @workgroup_size(64)
fn seed_c(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let q = i * 3u;
    let lab = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]);
    p0[i] = lab.y;
    p2[i] = lab.z;
}

// ⚠⚠ Recomputes the Oklab transform rather than reading three stored originals — the four-plane
// decision, and bit-identical because `img` has not been written until this pass.
//
// ⚠ Branches on each strength rather than trusting a plane to hold something harmless. Buffers are
// zero-initialised today, but `0.0 * NaN` is `NaN` and a lerp against an uninitialised plane is
// correct until a driver changes. The CPU stage branches on exactly these two conditions.
@compute @workgroup_size(64)
fn apply_nr(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = index(id);
    if (i >= params.pixels) { return; }
    let q = i * 3u;
    let lab = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]);
    var l = lab.x;
    var a = lab.y;
    var b = lab.z;
    if (params.s_lum > 0.0) { l = l + (p1[i] - l) * params.s_lum; }
    if (params.s_col > 0.0) {
        a = a + (p0[i] - a) * params.s_col;
        b = b + (p2[i] - b) * params.s_col;
    }
    let rgb = oklab_to_prophoto(l, a, b);
    // ⚠ Strong smoothing can push a pixel out of gamut; clamped at 0, the convention every other
    // stage uses — and one `compare` is structurally blind to, since a negative sample and 0
    // quantize to the same 8-bit level.
    img[q] = max(rgb.x, 0.0);
    img[q + 1u] = max(rgb.y, 0.0);
    img[q + 2u] = max(rgb.z, 0.0);
}
"#
    .to_string()
}

/// Noise reduction, on the GPU.
pub struct GpuNoiseReductionStage {
    s_lum: f32,
    s_col: f32,
    eps: f32,
    lum_r: usize,
    col_r: usize,
    cpu: pipeline::NoiseReductionStage,
}

impl GpuNoiseReductionStage {
    /// ⚠ Unlike `GpuDehazeStage` this needs **no image** — noise reduction estimates nothing from
    /// the photograph, so every parameter comes from the `Edits` and the render scale. The seam
    /// hands the buffer over regardless, because dehaze's airlight needs it.
    pub fn new(params: &NoiseReduction, scale: RenderScale, _ctx: &Arc<GpuContext>) -> Self {
        Self {
            s_lum: (params.luminance / 100.0).clamp(0.0, 1.0),
            s_col: (params.color / 100.0).clamp(0.0, 1.0),
            eps: pipeline::eps_for_detail(params.detail),
            lum_r: pipeline::scaled_radius(pipeline::LUM_RADIUS_FULL, scale),
            col_r: pipeline::scaled_radius(pipeline::chroma_radius_full(params.smoothness), scale),
            cpu: pipeline::NoiseReductionStage::new(params, scale),
        }
    }

    /// The dispatch sequence, in order, each pass with the grid it wants.
    ///
    /// ⚠ The luminance phase finishes into `p1` **before** the chroma phase starts, which is what
    /// lets the two share `p0` and `p2` and keeps the stage at four planes rather than six.
    fn passes(&self) -> Vec<(&'static str, Grid)> {
        let (lb, cb) = (box_grids(self.lum_r), box_grids(self.col_r));
        let mut v: Vec<(&'static str, Grid)> = Vec::new();
        if self.s_lum > 0.0 {
            v.push(("seed_l", Grid::Pixel));
            // The self-guided filter: two statistic blurs, one slope/offset kernel, two more.
            v.push(("square", Grid::Pixel));
            v.push(("copy_l", Grid::Pixel));
            v.extend([("lb_h1", lb.0), ("lb_v1", lb.1)]);
            v.extend([("lb_h2", lb.0), ("lb_v2", lb.1)]);
            v.push(("slope_offset", Grid::Pixel));
            v.extend([("lb_h1", lb.0), ("lb_v1", lb.1)]);
            v.extend([("lb_h2", lb.0), ("lb_v2", lb.1)]);
            v.push(("combine_lum", Grid::Pixel));
        }
        if self.s_col > 0.0 {
            v.push(("seed_c", Grid::Pixel));
            for _ in 0..pipeline::NR_CHROMA_PASSES {
                v.extend([("cb_h0", cb.0), ("cb_v0", cb.1)]);
            }
            for _ in 0..pipeline::NR_CHROMA_PASSES {
                v.extend([("cb_h2", cb.0), ("cb_v2", cb.1)]);
            }
        }
        v.push(("apply_nr", Grid::Pixel));
        v
    }

    /// ⚠ Emitted whatever the sliders say, so one shader text serves every path. The program cache
    /// keys on that text, so a rung with different radii compiles its own — correct, and a handful
    /// of programs rather than one per frame.
    fn shader(&self) -> String {
        let mut src = String::from(crate::oklab_wgsl::OKLAB_WGSL);
        src.push_str(&preamble());
        src.push_str(&box_pass("lb_h1", "lb_v1", "p1", "p1", "p3", self.lum_r));
        src.push_str(&box_pass("lb_h2", "lb_v2", "p2", "p2", "p3", self.lum_r));
        src.push_str(&box_pass("cb_h0", "cb_v0", "p0", "p0", "p3", self.col_r));
        src.push_str(&box_pass("cb_h2", "cb_v2", "p2", "p2", "p3", self.col_r));
        src
    }
}

impl GpuStage for GpuNoiseReductionStage {
    fn name(&self) -> &'static str {
        "noise_reduction"
    }

    /// Measured whole-buffer at 3123x2085 (6.5 Mpx) by `bench_preview`'s native block, three runs:
    /// **81.9-82.8 ms**.
    ///
    /// ⚠ Ships at the bottom of that band, the conservative direction — a lower number makes a
    /// step less likely to chain, and the gate must fail toward the CPU.
    ///
    /// ⚠⚠ **A single earlier reading said 86.1 and the band says 81.9-82.8 — quote a band, not a
    /// figure.** That reading was taken at load 5.43 against this band's 4.24-5.27, and a busier
    /// machine biases the CPU column upward. M8.3h had to retract a headline for exactly this
    /// reason: a point measurement compared against a point prediction says less than it looks.
    fn cpu_cost(&self) -> f32 {
        81.0
    }

    /// ⚠ Four, against dehaze's seven: the self-guided filter needs two statistics where the
    /// general form needs four, and the two phases are sequenced so they share planes. 104 MB at
    /// 6.5 Mpx, 398 MiB at full resolution.
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
        for b in 2..6 {
            layout_entries.push(entry(b, rw));
        }
        let (layout, pipelines) =
            ctx.cache
                .program(device, "noise_reduction", &src, &layout_entries, &names);

        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[16..20].copy_from_slice(&self.s_lum.to_le_bytes());
        b[20..24].copy_from_slice(&self.s_col.to_le_bytes());
        b[24..28].copy_from_slice(&self.eps.to_le_bytes());

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise-reduction-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &b);

        let mut bound = vec![img.buffer.as_entire_binding(), uniform.as_entire_binding()];
        for s in scratch.iter().take(4) {
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
            label: Some("noise-reduction-bind"),
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

    /// ⚠ Scale 1.0, so the radii are the unscaled ones: `LUM_RADIUS_FULL = 6` (below
    /// `RUNNING_SUM_MIN_RADIUS`, so the guided filter takes the **naive** body) and a chroma
    /// radius of 8..32 (above it, so the chroma blur takes the **running** body). Both production
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

    /// A noisy scene with real structure, and **all four** properties are load-bearing.
    ///
    /// (1) **Noise on every channel**, decorrelated, or there is nothing for either half to
    /// remove. (2) **Hard edges**, because the guided filter's whole job is to keep one — a
    /// fixture without one cannot tell the filter from a plain blur. (3) **Saturated colour**, or
    /// the chroma half acts on `a`/`b` that are near zero and a wrong blur is invisible.
    ///
    /// ⚠⚠ (4) **A near-black band BETWEEN two saturated ones, which is the only reason the zero
    /// clamp is reachable at all.** A first version used two mid-tone colours and never drove a
    /// single sample negative — so `no_sample_comes_back_negative` would have passed against a
    /// shader with no clamp in it. The chroma blur carries the neighbours' large `a`/`b` into a
    /// region whose `L` is tiny, and high chroma at low lightness is outside ProPhoto's gamut.
    /// Measured: 0 clamped samples with the old fixture, **5000** with this one.
    ///
    /// ⚠ 200x160 so the chroma reach fits: two passes at r=32 reach 64 px, comfortably inside the
    /// short edge.
    fn noisy(w: usize, h: usize) -> ImageBuf {
        const BANDS: [[f32; 3]; 3] = [[0.70, 0.01, 0.01], [0.02, 0.02, 0.02], [0.01, 0.01, 0.70]];
        let mut img = ImageBuf::new(w, h);
        let mut s: u32 = 0x1234_5678;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) - 0.5
        };
        for y in 0..h {
            for x in 0..w {
                let base = BANDS[x * BANDS.len() / w];
                let mut px = [0.0f32; 3];
                for (c, v) in px.iter_mut().enumerate() {
                    *v = (base[c] + 0.06 * rnd()).max(0.0);
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn params(luminance: f32, color: f32) -> NoiseReduction {
        NoiseReduction {
            luminance,
            color,
            ..NoiseReduction::default()
        }
    }

    fn chained(ctx: &Arc<GpuContext>, p: &NoiseReduction, img: ImageBuf) -> ImageBuf {
        let stage = GpuNoiseReductionStage::new(p, scale(), ctx);
        crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous** — a stage that did nothing would match
    /// the CPU perfectly on a fixture the CPU also leaves alone. The W5 failure mode.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = noisy(200, 160);
        for (l, c) in [(60.0f32, 0.0f32), (0.0, 60.0), (60.0, 60.0)] {
            let out = pipeline::NoiseReductionStage::new(&params(l, c), scale()).apply(img.clone());
            let moved = img
                .data()
                .iter()
                .zip(out.data())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                moved > img.data().len() / 2,
                "fixture check: NR {l}/{c} must move most of the fixture, moved {moved}"
            );
        }
    }

    /// ⚠⚠ **All three configurations, because `apply_nr` BRANCHES on each strength.** A test of
    /// "both on" cannot see a luminance-only path that reads an uninitialised chroma plane, and
    /// the reverse. This is the only place the two branches are exercised apart.
    ///
    /// ⚠ Bounded in 8-bit levels, not bit-identical: NR inherits all three recorded divergence
    /// classes — subnormal flush-to-zero (M8.3a), FMA contraction in the lerp and in
    /// `combine_lum` (M8.3m), and the box blur's division (M8.3n).
    #[test]
    fn every_configuration_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (l, c) in [(60.0f32, 0.0f32), (0.0, 60.0), (60.0, 60.0), (100.0, 100.0)] {
            let img = noisy(200, 160);
            let p = params(l, c);
            let cpu = pipeline::NoiseReductionStage::new(&p, scale()).apply(img.clone());
            let gpu = chained(&ctx, &p, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "NR {l}/{c}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠ The Detail slider sets `eps`, which is a **uniform** rather than a shader literal — so a
    /// stage that never wrote it, or wrote it at the wrong offset, would render every Detail value
    /// alike and the test above could not see it.
    #[test]
    fn the_detail_slider_reaches_the_shader() {
        let Some(ctx) = ctx() else { return };
        let img = noisy(200, 160);
        let mut out = Vec::new();
        for detail in [0.0f32, 100.0] {
            let p = NoiseReduction {
                luminance: 80.0,
                detail,
                ..NoiseReduction::default()
            };
            out.push(chained(&ctx, &p, img.clone()));
        }
        assert!(
            !crate::compare(&out[0], &out[1]).is_exact(),
            "Detail 0 and Detail 100 must not render alike"
        );
    }

    /// ⚠⚠ **`compare` quantizes to 8-bit levels, where a negative sample and 0 are THE SAME
    /// VALUE** — so it is structurally blind to a missing clamp, and every test above stays green
    /// with the floor deleted. M8.3e's finding about the output transform, met for the seventh
    /// time. Strong smoothing pushes a saturated pixel out of gamut, which is what the fixture's
    /// two saturated halves are for.
    #[test]
    fn no_sample_comes_back_negative() {
        let Some(ctx) = ctx() else { return };
        let img = noisy(200, 160);
        let p = params(100.0, 100.0);
        let cpu = pipeline::NoiseReductionStage::new(&p, scale()).apply(img.clone());
        assert!(
            cpu.data().contains(&0.0),
            "fixture check: the CPU must actually reach the clamp here"
        );
        let gpu = chained(&ctx, &p, img);
        for (i, v) in gpu.data().iter().enumerate() {
            assert!(
                *v >= 0.0,
                "sample {i} came back {v} — a level comparison cannot see this"
            );
        }
    }

    /// ⚠ The name is what `PrefixStepKey`'s neighbours and the seam's lookup match on.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx).name(),
            pipeline::NoiseReductionStage::new(&params(50.0, 50.0), scale()).name()
        );
    }

    /// ⚠⚠ **The plane budget as a decision rather than a comment.** Four is what the two-phase
    /// sequencing plus the recompute in `apply_nr` buy; a change that stored the originals would
    /// need six and would double the full-resolution allocation on the one stage where that is the
    /// known hazard.
    #[test]
    fn the_stage_asks_for_four_planes() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx).scratch_planes(),
            4
        );
    }

    /// ⚠ Both radii take the body the sweep chose, and neither is near the crossover — so this
    /// pins which side each lands on rather than the constant's exact value.
    #[test]
    fn each_radius_takes_the_body_the_sweep_chose() {
        let Some(ctx) = ctx() else { return };
        let s = GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx);
        assert!(
            !use_running_sum(s.lum_r),
            "the guided filter's r={} takes the naive body",
            s.lum_r
        );
        assert!(
            use_running_sum(s.col_r),
            "the chroma blur's r={} takes the running body",
            s.col_r
        );
    }

    /// ⚠⚠ **At scale 1.0 the radius scaling is the identity, so every test above is blind to it
    /// being dropped entirely.** A fit rung is the only place the two disagree — and it is the
    /// rung the app spends most of its time at.
    #[test]
    fn the_radii_follow_the_render_scale() {
        let Some(ctx) = ctx() else { return };
        let fit = RenderScale::new(0.25);
        let p = params(50.0, 50.0);
        let native = GpuNoiseReductionStage::new(&p, scale(), &ctx);
        let small = GpuNoiseReductionStage::new(&p, fit, &ctx);
        assert_eq!(
            small.lum_r,
            pipeline::scaled_radius(pipeline::LUM_RADIUS_FULL, fit),
            "the guided radius must follow the render scale"
        );
        assert!(
            small.lum_r < native.lum_r && small.col_r < native.col_r,
            "a fit rung must use smaller radii than a native one"
        );
    }
}

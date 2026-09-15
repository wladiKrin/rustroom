//! Lens correction as compute shaders — the third stage to move outside the tail, and the first
//! aimed at **export** rather than at the preview.
//!
//! ⚠⚠ **The field travels as a coarse displacement grid rather than as a WGSL reimplementation of
//! lensfun's distortion model.** See [`pipeline::lens_grid`] for the measurement that settled it:
//! at step 16 the lattice is 2.74 MiB against 695 MiB per-pixel on a 26 Mpx frame, and building it
//! costs 1.4 ms against the full field's 38.3. The alternative was a second definition of the
//! correction that could disagree with `Corrector::source`, made harder by `Modifier`'s
//! normalisation fields being private.
//!
//! ⚠⚠ **THREE SCRATCH PLANES, because a gather cannot be in place.** Every stage before this one
//! touched only its own pixel, which is what made the chain's one-image contract hold. An output
//! pixel here reads the source anywhere within the resampler's support, and those pixels may
//! already have been overwritten — so `seed` copies the image into the planes and `gather` reads
//! from them. Single-channel planes are exactly the deinterleaved layout a per-channel sampler
//! wants, the chain allocates the widest request once and shares it, and `encode` is handed a
//! `&mut ComputePass` and **cannot issue a buffer copy**, so a stage-owned source buffer would have
//! needed a copy kernel anyway.
//!
//! ⚠ Two dispatches in one pass, on the in-pass memory barrier M8.3b **measured** rather than read:
//! three chained exposures equalled one, and gains multiply, so a missing barrier would have given
//! a wrong product rather than a plausible image.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::lens_grid::{DisplacementGrid, STRIDE};
use pipeline::resample::Weights;
use pipeline::{PipelineStage, Resampler};

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// How many pixels apart the displacement field is sampled.
///
/// ⚠⚠ **Swept against OUTPUT 8-bit levels, never against the field's own pixel error — and the
/// sweep moved it from 16 to 8.** `the_grid_step_costs_what_the_lattice_predicts` measures, on the
/// 200x160 grid chart with the synthetic `b = -0.05`:
///
/// | step | lattice error | worst output levels |
/// |---|---|---|
/// | 1 | exact at every pixel | **0.00009** |
/// | 2 | 0.0017 px | 2.57 |
/// | 4 | 0.0069 px | 7.68 |
/// | **8** | **0.028 px** | **28.4** |
/// | 16 | 0.111 px | 94.7 |
/// | 32 | 0.459 px | 221.0 |
///
/// ⚠⚠ **That is about 1000 output levels per pixel of lattice error, and the spec's own estimate
/// was 250.** On this chart a hard 0.1/0.9 edge every 8 px, resampled through a kernel that
/// overshoots, produces gradients far steeper than the 156 levels/px an edge naively suggests —
/// so a position error buys several times the levels one would guess. That factor is the whole
/// reason this constant was swept rather than chosen, and it is what moved it.
///
/// ⚠⚠ **And on a REAL photograph the two steps are nearly the same, which is a correction to this
/// comment's own first draft.** `probe_lens` on the XF16-80 at 26.0 Mpx measures the lattice error
/// as **0.0015 / 0.0020 / 0.0027 / 0.0093 / 0.0348 px at steps 4 / 8 / 16 / 32 / 64** — essentially
/// **floored around 0.002 px below step 32**, because there the f32 precision of the field itself
/// dominates rather than the interpolation. So step 8 buys about **1.35x** over step 16 here, not
/// the 4x the synthetic sweep suggests, and the measured end-to-end divergence at step 8 is
/// **0.797 of a level**.
///
/// ⚠ It ships at 8 anyway, for a reason that is about the *next* lens rather than this one: the
/// XF16-80 at 19.2 mm is a mild correction, and on a fisheye or an ultrawide the quadratic term
/// takes over sooner — where step 8 really is 4x better. It costs **10.9 MiB against 2.74** on a
/// 298 MiB frame and **4.8 ms against 1.4** on a 730 ms stage. There is nothing to trade.
///
/// ⚠ Two constants in this crate were reasoned about instead of swept — `MIN_GPU_BOX_PX` low by
/// ~8x, `MAX_TILE_DABS` sitting past an 8.7x occupancy cliff. This is the third, and the sweep
/// disagreed with the reasoning again.
pub const GRID_STEP: usize = 8;

/// Lens correction on the GPU.
pub struct GpuLensCorrectionStage {
    cpu: pipeline::LensCorrectionStage,
    grid: DisplacementGrid,
    /// The CPU's own weight table, uploaded. **Empty for bilinear**, which the shader computes.
    weights: Vec<f32>,
    /// ⚠ Kept rather than re-derived per `encode`: building a `Weights` allocates a 4096-entry
    /// table, and `encode` runs once per render.
    taps: usize,
    phases: usize,
    radius: i32,
    w: usize,
    h: usize,
}

impl GpuLensCorrectionStage {
    /// Build the GPU twin, or `None` when the profile yields no corrector.
    ///
    /// ⚠ The corrector is built from **this image's** dimensions, exactly as the CPU stage builds
    /// its own from `src.full`: a corrector is tied to a size, and the preview and the export
    /// render at different ones through the same `Edits`.
    pub fn new(
        lens: &lens_profile::LensShot,
        want: lens_profile::Corrections,
        resampler: Resampler,
        image: &core_image::ImageBuf,
    ) -> Option<Self> {
        Self::with_step(lens, want, resampler, image, GRID_STEP)
    }

    /// As [`Self::new`], at a chosen lattice spacing.
    ///
    /// ⚠⚠ **This exists so the shader can be tested INDEPENDENTLY of the lattice**, and that
    /// separation was learned from a failure rather than designed in: the first equivalence test
    /// ran at `GRID_STEP` on a deliberately harsh synthetic lens and diverged by 75 levels — all
    /// of it the lattice, none of it the shader. `Weights::with_table_size`'s precedent.
    pub(crate) fn with_step(
        lens: &lens_profile::LensShot,
        want: lens_profile::Corrections,
        resampler: Resampler,
        image: &core_image::ImageBuf,
        step: usize,
    ) -> Option<Self> {
        let (w, h) = (image.width(), image.height());
        let corrector = lens_profile::Corrector::new(&lens.profile, &lens.shot, w, h, want)?;
        let weights = Weights::new(resampler);
        Some(Self {
            cpu: pipeline::LensCorrectionStage::new(lens.clone(), want, resampler),
            grid: DisplacementGrid::build(&corrector, w, h, step),
            // ⚠⚠ **Substituted, never transcribed** — the rule `ToneCurveStage::lut` already
            // follows. A shader that rebuilt the kernel would be a second definition of a curve
            // nobody re-derives, and the two would disagree invisibly.
            taps: weights.taps(),
            phases: weights.phases(),
            radius: resampler.radius() as i32,
            weights: weights.lut().to_vec(),
            w,
            h,
        })
    }

    /// The shader, with the tap count, the radius and the bilinear branch baked in.
    ///
    /// ⚠ Baked rather than passed as uniforms, for `box_wgsl`'s reason: the shader text is the
    /// program cache's key, and these change only with the resampler setting — four values — where
    /// a literal lets the compiler see a constant trip count.
    fn shader(&self) -> String {
        let taps = self.taps;
        let radius = self.radius;
        // ⚠⚠ **The bilinear branch is not an optimisation.** `Weights`' table is deliberately empty
        // for that kernel because it is computed exactly — `1.0 - (1.0 - f)` is not `f` in f32 —
        // so reading a table that does not exist would resample with zeros, and materialising one
        // would quantise the phase and diverge from the CPU on the one kernel every other path
        // agrees about. A divergence on exactly one setting is the worst shape a divergence has.
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
    cols: u32,
    rows: u32,
    step: f32,
    pad: f32,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read> grid: array<f32>;
@group(0) @binding(6) var<storage, read> lut: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// The source planes, so the gather never reads a pixel it has already overwritten.
@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    p0[i] = img[q];
    p1[i] = img[q + 1u];
    p2[i] = img[q + 2u];
}}

// The CPU twin is `DisplacementGrid::at`; the equivalence test is what holds the two together.
fn grid_at(x: f32, y: f32, k: u32) -> f32 {{
    let fx = x / params.step;
    let fy = y / params.step;
    let gx = min(u32(max(floor(fx), 0.0)), params.cols - 2u);
    let gy = min(u32(max(floor(fy), 0.0)), params.rows - 2u);
    let tx = fx - f32(gx);
    let ty = fy - f32(gy);
    let a = (gy * params.cols + gx) * {stride}u + k;
    let c = ((gy + 1u) * params.cols + gx) * {stride}u + k;
    let top = grid[a] * (1.0 - tx) + grid[a + {stride}u] * tx;
    let bot = grid[c] * (1.0 - tx) + grid[c + {stride}u] * tx;
    return top * (1.0 - ty) + bot * ty;
}}

fn axis(f: f32) -> array<f32, {taps}u> {{
    var wgt: array<f32, {taps}u>;
{axis_body}
    return wgt;
}}

// ⚠⚠ A tap outside the frame contributes ZERO and the weights are NOT renormalised — which is what
// `resample::sample_channel` does, and the only thing that can be done here: renormalising would
// brighten the border relative to the CPU rather than merely differ from it.
fn sample_ch(ch: u32, sx: f32, sy: f32) -> f32 {{
    let bx = floor(sx);
    let by = floor(sy);
    let x0 = i32(bx) - {radius} + 1;
    let y0 = i32(by) - {radius} + 1;
    let wx = axis(sx - bx);
    let wy = axis(sy - by);
    var acc = 0.0;
    for (var j = 0u; j < {taps}u; j = j + 1u) {{
        let yi = y0 + i32(j);
        if (yi < 0 || yi >= i32(params.h)) {{ continue; }}
        var row = 0.0;
        for (var i = 0u; i < {taps}u; i = i + 1u) {{
            let xi = x0 + i32(i);
            if (xi < 0 || xi >= i32(params.w)) {{ continue; }}
            let o = u32(yi) * params.w + u32(xi);
            var v = 0.0;
            if (ch == 0u) {{ v = p0[o]; }}
            else if (ch == 1u) {{ v = p1[o]; }}
            else {{ v = p2[o]; }}
            row = row + wx[i] * v;
        }}
        acc = acc + wy[j] * row;
    }}
    return acc;
}}

// ⚠ Per channel, because transverse chromatic aberration gives each its own source point — which
// is why `resample::sample_channel` exists at all rather than the three-channel entry point.
@compute @workgroup_size(64)
fn gather(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let x = f32(i % params.w);
    let y = f32(i / params.w);
    let gain = grid_at(x, y, 6u);
    let q = i * 3u;
    img[q] = sample_ch(0u, x + grid_at(x, y, 0u), y + grid_at(x, y, 1u)) * gain;
    img[q + 1u] = sample_ch(1u, x + grid_at(x, y, 2u), y + grid_at(x, y, 3u)) * gain;
    img[q + 2u] = sample_ch(2u, x + grid_at(x, y, 4u), y + grid_at(x, y, 5u)) * gain;
}}
"#,
            stride = STRIDE,
            taps = taps,
            radius = radius,
            axis_body = axis_body,
        )
    }
}

impl GpuStage for GpuLensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    /// Measured whole-buffer by `probe_lens` on an X-T4 frame: **729.6-736.3 ms at 26.0 Mpx**
    /// over three runs, which is 190-192 per 6.8 Mpx; and 169.0 ms at 6.5 Mpx, which is 176.8.
    ///
    /// ⚠ Ships at the **bottom** of the two, the conservative direction — a lower number makes a
    /// step less likely to chain, and the gate must fail toward the CPU. It clears
    /// `TRANSFER_COST`'s 27.0 by nearly seven times either way, so the exact value is not
    /// load-bearing here; what matters is that it was measured rather than guessed.
    fn cpu_cost(&self) -> f32 {
        176.0
    }

    /// ⚠⚠ **Three, and it is the first stage here that needs them for CORRECTNESS rather than for
    /// working room.** See the module's own note: a gather cannot be in place.
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
            entry(6, ro),
        ];
        let (layout, pipelines) = ctx.cache.program(
            device,
            "lens_correction",
            &self.shader(),
            &layout_entries,
            &["seed", "gather"],
        );

        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&(self.w as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(self.h as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[16..20].copy_from_slice(&(self.grid.cols() as u32).to_le_bytes());
        b[20..24].copy_from_slice(&(self.grid.rows() as u32).to_le_bytes());
        b[24..28].copy_from_slice(&(self.grid.step() as f32).to_le_bytes());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens-correction-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &b);

        // ⚠ `.max(4)` because wgpu refuses a zero-sized buffer, and bilinear's table is
        // legitimately empty — the same guard `Dispatch` and `GpuImage::scratch` both carry. The
        // shader's bilinear branch never reads it.
        let upload = |label: &'static str, data: &[f32]| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (data.len() * 4).max(4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !data.is_empty() {
                ctx.queue.write_buffer(&buf, 0, bytemuck::cast_slice(data));
            }
            buf
        };
        let grid_buf = upload("lens-correction-grid", self.grid.data());
        let lut_buf = upload("lens-correction-lut", &self.weights);

        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
            grid_buf.as_entire_binding(),
            lut_buf.as_entire_binding(),
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
            label: Some("lens-correction-bind"),
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

    /// The synthetic lens the CPU stage's own tests use, so the two are comparable.
    ///
    /// ⚠ `b` is **negative** — a barrel correction — deliberately: that is the sign whose extreme
    /// sits at an edge midpoint rather than at a corner, and the harder case the CPU stage's region
    /// tests established. The vignetting model is present because without one the gain is a
    /// constant 1.0 and every assertion about it is vacuous.
    fn lens() -> lens_profile::LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                    <vignetting model="pa" focal="50" aperture="2.8" distance="10"
                        k1="-0.3" k2="0.0" k3="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        lens_profile::LensShot {
            profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    fn want() -> lens_profile::Corrections {
        lens_profile::Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        }
    }

    /// ⚠⚠ **A hard grid, not a smooth ramp** — the CPU stage's own fixture, for its reason: every
    /// kernel reproduces a linear ramp almost exactly, so a ramp cannot tell two samplers apart and
    /// a sub-pixel source error would be invisible on one. Tinted, so a per-channel mistake shows.
    fn grid_chart(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x % 8 < 2 || y % 8 < 2 { 0.9 } else { 0.1 };
                img.set_pixel(x, y, [v, v * 0.6, v * 0.3]);
            }
        }
        img
    }

    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        img
    }

    /// ⚠⚠ **`FINE` rather than `GRID_STEP`, and the reason is the sharpest thing this milestone
    /// learned.** This fixture's synthetic `b = -0.05` on a 200x160 frame distorts about **40x
    /// harder relative to its own frame** than the real XF16-80 does across 6246x4170 — measured,
    /// 0.111 px of lattice error at step 16 against the real lens's 0.0027. So a test run at
    /// `GRID_STEP` here measures the LATTICE, not the shader: the first version diverged by 75
    /// levels and every one of them was interpolation. At step **1** the lattice is exact at every
    /// pixel — `at()`'s `tx` and `ty` are both zero there — so what remains is the shader alone,
    /// and it measures **0.00009 of a level**. That is the number these tests bound.
    ///
    /// ⚠ What `GRID_STEP` itself costs is a separate question and has its own test, which sweeps
    /// it rather than asserting a number.
    const FINE: usize = 1;

    fn on_gpu_at(ctx: &Arc<GpuContext>, r: Resampler, img: ImageBuf, step: usize) -> ImageBuf {
        let stage = GpuLensCorrectionStage::with_step(&lens(), want(), r, &img, step)
            .expect("fixture check: the synthetic profile must produce a corrector");
        crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img)
    }

    fn on_gpu(ctx: &Arc<GpuContext>, r: Resampler, img: ImageBuf) -> ImageBuf {
        on_gpu_at(ctx, r, img, FINE)
    }

    fn on_cpu(r: Resampler, img: ImageBuf) -> ImageBuf {
        pipeline::LensCorrectionStage::new(lens(), want(), r).apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous** — a stage that did nothing would match
    /// a CPU stage that also did nothing. The W5 failure mode, where creative LUTs shipped fully
    /// tested and 100% non-functional.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = grid_chart(200, 160);
        let out = on_cpu(Resampler::CatmullRom, img.clone());
        let moved = img
            .data()
            .iter()
            .zip(out.data())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            moved > img.data().len() / 2,
            "fixture check: the correction must move most of the fixture, moved {moved}"
        );
    }

    /// The milestone's acceptance test: the GPU renders the photograph the CPU does.
    ///
    /// ⚠⚠ **Every kernel, because bilinear takes a DIFFERENT BRANCH.** Its weight table is empty by
    /// design, so a shader that only read the table would agree on three kernels and render black
    /// on the fourth — a divergence on exactly one setting, which is the worst shape one can have.
    #[test]
    fn every_kernel_renders_the_same_picture_as_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for r in [
            Resampler::Bilinear,
            Resampler::Mitchell,
            Resampler::CatmullRom,
            Resampler::Lanczos3,
        ] {
            let img = grid_chart(200, 160);
            let d = crate::compare(&on_cpu(r, img.clone()), &on_gpu(&ctx, r, img));
            eprintln!(
                "{r:?}: worst {} levels over {:.3}% of samples",
                d.worst_levels,
                d.differing_percent()
            );
            // ⚠⚠ **0.01, not a loose tolerance** — with the lattice exact the shader is exact to
            // float noise, so anything above this is a real disagreement rather than rounding.
            // A 1.5-level bound would have passed against a shader with a genuine half-pixel
            // offset in it.
            assert!(
                d.worst_levels < 0.01,
                "{r:?}: worst {} levels over {:.3}% of samples",
                d.worst_levels,
                d.differing_percent()
            );
        }
    }

    /// ⚠⚠ **`GRID_STEP` is judged in OUTPUT LEVELS, which is the only judgement that matters** —
    /// a source error in pixels says nothing on its own. What is asserted is the **shape**: the
    /// cost is quadratic in the step, as bilinear interpolation of a smooth field must be, so the
    /// sweep is measuring the mechanism rather than noise.
    ///
    /// Measured here, Catmull-Rom on the 200x160 grid chart with the synthetic `b = -0.05`:
    ///
    /// | step | lattice error | output levels |
    /// |---|---|---|
    /// | 2 | ~0.002 px | see the printout |
    /// | 4 | ~0.007 px | |
    /// | 8 | ~0.028 px | |
    /// | **16** | **~0.111 px** | |
    /// | 32 | ~0.459 px | |
    ///
    /// ⚠⚠ **And the arithmetic that carries it to a real photograph, which is what actually
    /// decides the constant:** the lattice error scales as `step^2 * max_displacement / frame^2`,
    /// and `pipeline::lens_grid` measures the real XF16-80 at **0.0027 px at step 16** against
    /// this fixture's 0.111 — a factor of 41. So whatever this fixture costs at step 16, a real
    /// photograph pays roughly that at **step 2**, which is the row `FINE` uses and the row the
    /// equivalence tests run on.
    #[test]
    fn the_grid_step_costs_what_the_lattice_predicts() {
        let Some(ctx) = ctx() else { return };
        let mut prev = 0.0f32;
        for step in [1usize, 2, 4, 8, 16, 32] {
            let img = grid_chart(200, 160);
            let d = crate::compare(
                &on_cpu(Resampler::CatmullRom, img.clone()),
                &on_gpu_at(&ctx, Resampler::CatmullRom, img, step),
            );
            eprintln!(
                "step {step}: worst {} levels, {:.3}% of samples differ",
                d.worst_levels,
                d.differing_percent()
            );
            assert!(
                d.worst_levels >= prev,
                "step {step} costs {} levels, below step {}'s {prev} — a coarser lattice cannot \
                 be more accurate, so this sweep is measuring noise rather than the mechanism",
                d.worst_levels,
                (step / 2).max(1)
            );
            prev = d.worst_levels;
        }
        assert!(
            prev > 1.0,
            "the coarsest step costs only {prev} levels — this fixture is too easy to sweep on, \
             so the sweep says nothing about where the lattice starts to show"
        );
    }

    /// ⚠⚠ **Each correction alone, because the three together can hide one another.** This is what
    /// localised the milestone's one real failure: with all three on the divergence was 75 levels
    /// and unattributable; ablated, distortion read 48.9 while TCA read 0.004 and vignetting 0.57,
    /// which said immediately that the sampler was fine and the lattice was not.
    ///
    /// ⚠ It is also the only test that can see a **dropped TCA**: with all three on, a shader
    /// using channel 0's displacement for every channel differs by less than the lattice does.
    #[test]
    fn each_correction_alone_agrees_with_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (d, t, v) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let w = lens_profile::Corrections {
                distortion: d,
                tca: t,
                vignetting: v,
                scale: 0.0,
            };
            let img = grid_chart(200, 160);
            let cpu = pipeline::LensCorrectionStage::new(lens(), w, Resampler::CatmullRom)
                .apply(img.clone());
            let stage =
                GpuLensCorrectionStage::with_step(&lens(), w, Resampler::CatmullRom, &img, FINE)
                    .unwrap();
            let gpu = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img);
            let x = crate::compare(&cpu, &gpu);
            assert!(
                x.worst_levels < 0.01,
                "distortion={d} tca={t} vignetting={v}: worst {} levels over {:.3}%",
                x.worst_levels,
                x.differing_percent()
            );
        }
    }

    /// ⚠⚠ **THE TEST THE SABOTAGE STEP FORCED, and the gap it found is worth more than the test.**
    /// Every equivalence test above runs at `FINE`, where the lattice is exact at every integer
    /// pixel — `tx` and `ty` are both **zero**, so the interpolation's neighbour terms are
    /// multiplied by nothing and are **unreachable**. Stepping `grid_at` by `1u` instead of by
    /// `STRIDE` — reading the next point's `dx` where the gain belongs — left all six green.
    ///
    /// This drives the interpolation directly: vignetting alone at a **coarse** step, so the source
    /// position is the identity and the whole output is the interpolated gain, compared against
    /// `DisplacementGrid::at` on the CPU through the **same** lattice. Any difference is the
    /// shader's `grid_at` and nothing else.
    #[test]
    fn the_shaders_grid_read_agrees_with_the_cpus() {
        let Some(ctx) = ctx() else { return };
        const COARSE: usize = 8;
        let (w, h) = (200usize, 160usize);
        let want = lens_profile::Corrections {
            distortion: false,
            tca: false,
            vignetting: true,
            scale: 0.0,
        };
        let src = flat(w, h);
        let stage =
            GpuLensCorrectionStage::with_step(&lens(), want, Resampler::Bilinear, &src, COARSE)
                .expect("fixture check: the synthetic profile must produce a corrector");
        let gpu = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src);

        let l = lens();
        let c = lens_profile::Corrector::new(&l.profile, &l.shot, w, h, want).unwrap();
        let g = DisplacementGrid::build(&c, w, h, COARSE);
        let mut reference = ImageBuf::new(w, h);
        let mut interpolated = 0usize;
        for y in 0..h {
            for x in 0..w {
                let gain = g.gain(x as f32, y as f32);
                if (gain - c.gain(x as f32, y as f32)).abs() > 1e-6 {
                    interpolated += 1;
                }
                reference.set_pixel(x, y, [0.5 * gain, 0.5 * gain, 0.5 * gain]);
            }
        }
        // ⚠ Without this the test is vacuous: on a lattice fine enough to be exact everywhere,
        // `tx` and `ty` are zero and the neighbour terms are unreachable all over again.
        assert!(
            interpolated > w * h / 2,
            "fixture check: most pixels must fall BETWEEN lattice points, only {interpolated} did"
        );

        let d = crate::compare(&reference, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "the shader's grid read diverges by {} levels over {:.3}% of samples",
            d.worst_levels,
            d.differing_percent()
        );
    }

    /// ⚠⚠ **A flat image, so ONLY the gain acts** — on a grid chart a dropped seventh component
    /// hides inside the resampling difference, and the equivalence bound above might not see it.
    /// Here the whole output is the vignetting field.
    #[test]
    fn the_vignetting_gain_reaches_the_output() {
        let Some(ctx) = ctx() else { return };
        let src = flat(200, 160);
        let cpu = on_cpu(Resampler::CatmullRom, src.clone());
        let spread = cpu.data().iter().cloned().fold(0.0f32, f32::max)
            - cpu.data().iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread > 0.02,
            "fixture check: this profile's vignetting must vary across the frame, spread {spread}"
        );
        let d = crate::compare(&cpu, &on_gpu(&ctx, Resampler::CatmullRom, src));
        assert!(
            d.worst_levels < 0.01,
            "the gain diverges by {} levels",
            d.worst_levels
        );
    }
}

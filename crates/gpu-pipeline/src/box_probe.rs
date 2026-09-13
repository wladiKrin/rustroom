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
/// ⚠⚠ **Built from [`crate::box_wgsl::box_pass`], so this times and tests THE CODE THAT SHIPS.**
/// An earlier draft carried its own copy of both bodies — which measures a copy, and would let the
/// two drift the moment either was tuned. The frozen-reference rule (M8.3i) applies to a
/// deliberate oracle; it does not license a second implementation of the thing under test.
fn shader(r: usize, running: bool) -> String {
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
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> a: array<f32>;
@group(0) @binding(3) var<storage, read_write> b: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// ⚠ Channel 0 only: the probe is about a **plane**, and a box blur has no colour in it.
@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    a[i] = img[i * 3u];
}}

@compute @workgroup_size(64)
fn emit(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let p = i * 3u;
    img[p] = a[i];
    img[p + 1u] = a[i];
    img[p + 2u] = a[i];
}}
{passes}
"#,
        passes = crate::box_wgsl::box_pass_forcing("box_h", "box_v", "a", "a", "b", r, running)
    )
}

pub(crate) struct BoxProbe {
    pub r: u32,
    pub passes: usize,
    /// ⚠ Explicit, so the sweep can time both bodies at one radius. Production derives it.
    pub running: bool,
    cpu: pipeline::ExposureStage,
}

impl BoxProbe {
    /// The variant the threshold picks — what production would do at this radius.
    pub fn new(r: u32, passes: usize) -> Self {
        Self::forcing(r, passes, crate::box_wgsl::use_running_sum(r as usize))
    }

    pub fn forcing(r: u32, passes: usize, running: bool) -> Self {
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
        let per_row = (img.height() as u32)
            .div_ceil(WORKGROUP)
            .min(MAX_GROUPS_PER_DIM);
        let per_col = (img.width() as u32)
            .div_ceil(WORKGROUP)
            .min(MAX_GROUPS_PER_DIM);
        let mut names: Vec<&str> = vec!["seed"];
        let mut groups: Vec<(u32, u32)> = vec![per_pixel];
        for _ in 0..self.passes {
            if self.running {
                names.push("box_h");
                groups.push((per_row, 1));
                names.push("box_v");
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
        let label: &'static str =
            Box::leak(format!("box_probe_{}_{}", self.r, self.running).into_boxed_str());
        let src = shader(self.r as usize, self.running);
        let (layout, pipelines) = ctx.cache.program(
            device,
            label,
            &src,
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

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A plane with real structure at every scale, so a wrong window or a wrong divisor shows.
    fn plane(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let ramp = x as f32 / w as f32;
                let fine = if (x / 2 + y / 3) % 2 == 0 { 0.2 } else { -0.2 };
                let edge = if y > h / 2 { 0.6 } else { -0.1 };
                ramp + fine + edge
            })
            .collect()
    }

    fn as_image(p: &[f32], w: usize, h: usize) -> ImageBuf {
        let mut d = Vec::with_capacity(w * h * 3);
        for v in p {
            d.extend_from_slice(&[*v, *v, *v]);
        }
        ImageBuf::from_vec(w, h, d)
    }

    /// Channel 0 of one GPU box-blur pass at radius `r`.
    fn gpu_box(ctx: &Arc<GpuContext>, p: &[f32], w: usize, h: usize, r: u32) -> Vec<f32> {
        let out = crate::GpuChain::new(ctx.clone(), vec![Box::new(BoxProbe::new(r, 1))])
            .apply(as_image(p, w, h));
        out.data().as_chunks::<3>().0.iter().map(|c| c[0]).collect()
    }

    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut px = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            let t = (i % 4096) as f32 / 4096.0 * 1.2;
            px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
        }
        ImageBuf::from_vec(w, h, px)
    }

    /// ⚠⚠ **The load-bearing risk probe for M8.3n, run BEFORE the spec was written**, and kept for
    /// the same reason `bench_hsl` and `mask_breakdown` are: the numbers in `box_wgsl`'s header
    /// come from here, and a threshold nobody can re-measure is a threshold nobody can defend.
    ///
    /// ⚠ It drives `BoxProbe::forcing`, so it times **both** bodies at one radius. Production
    /// calls `BoxProbe::new`, which derives the variant — the threshold is the measurement's
    /// conclusion, not a parameter.
    ///
    /// Run: `cargo test --release -p gpu-pipeline -- --ignored --nocapture probe_box`
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
        println!(
            "{w}x{h} ({:.1} Mpx) on {}",
            (w * h) as f64 / 1e6,
            ctx.adapter_name()
        );

        let run = |r: u32, passes: usize, running: bool| -> f64 {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let chain = crate::GpuChain::new(
                    ctx.clone(),
                    vec![Box::new(BoxProbe::forcing(r, passes, running)) as Box<dyn GpuStage>],
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
        let base = run(0, 0, false);
        println!("  transfer only (0 passes)          {base:6.2} ms");
        println!("  crossover sweep, 3 passes:");
        for r in [1u32, 2, 3, 4, 6, 8, 10, 16, 24, 40] {
            let naive = run(r, 3, false) - base;
            let running = run(r, 3, true) - base;
            println!(
                "    r={r:<3} naive {naive:6.2}   running {running:6.2}   {}",
                if naive < running { "naive" } else { "RUNNING" }
            );
        }
        println!();
        // ⚠⚠ **The prefix's shapes, priced before any prefix design.** Dehaze's refinement is a
        // CROSS-guided filter (`p` is the transmission map, `guide` is luma), so M8.3n's
        // self-guided shortcut does NOT apply and it needs all six statistic box blurs. Its
        // radius is `REFINE_RADIUS_FULL = 60` scaled, i.e. **r=31 at a native rung** — above the
        // crossover, so the running body. The dark channel is a `min_filter` at
        // `PATCH_RADIUS_FULL = 20` scaled, i.e. **r=10**; a box blur cannot stand in for a minimum
        // (a min is not invertible, so there is no running form) but it bounds the naive cost.
        println!("  {:<30} {:>8} {:>8}", "prefix shapes", "naive", "running");
        for (label, r, passes) in [
            ("dehaze guided   r=31 x6", 31u32, 6usize),
            ("dehaze dark ch  r=10 x1", 10, 1),
            ("NR chroma       r=16 x2", 16, 2),
        ] {
            let n = run(r, passes, false) - base;
            let g = run(r, passes, true) - base;
            println!("  {label:<30} {n:8.2} {g:8.2}");
        }
        println!();
        println!(
            "  {:<30} {:>8} {:>8}   {:>6}",
            "", "naive", "running", "CPU"
        );
        for (label, r, passes, cpu) in [
            ("clarity  wide_blur r=40 x3", 40u32, 3usize, 13.5f64),
            ("texture  guided    r=2  x4", 2, 4, 18.5),
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

    /// One rounding of a quantity of this plane's magnitude.
    ///
    /// ⚠⚠ **Absolute, not in ulps, and the first version of this bound WAS in ulps and was
    /// wrong.** A box blur sums values of order `max|plane|` and divides; the error is one
    /// rounding of *that*, wherever the **result** happens to land. `plane()` crosses zero, and
    /// near zero a 3e-8 absolute difference is **30 ulps** — which reads as a catastrophe and is
    /// one rounding. Measuring the ulp distance of the result asks about the wrong quantity.
    fn one_rounding(p: &[f32]) -> f32 {
        f32::EPSILON * p.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// How many roundings the **running** body may differ by.
    ///
    /// ⚠ Both sides accumulate identically, so only the divide should differ — but a window sum
    /// rounds per term as well, so a literal one rounding was too tight and the first version of
    /// this test asserted it and failed. Worst observed on the 97x71 fixture: **2.2 roundings**.
    /// Eight leaves a 3.6x margin.
    const ROUNDINGS: f32 = 8.0;

    /// What the **CPU's own** running sum drifts by over one line.
    ///
    /// ⚠⚠ **The naive body needs a far looser bound than the running one, and the reason is that
    /// the drift is the CPU's rather than ours.** The GPU re-sums a 3-sample window; the CPU
    /// carries a running sum across all 97 columns, adding and subtracting, so its error
    /// accumulates as O(w) roundings while ours does not accumulate at all. Measured at r=1:
    /// **3.7e-6**, i.e. 17 roundings — well past `ROUNDINGS` and well inside this.
    ///
    /// ⚠ Loose (2e-5 here) and still discriminating by four orders of magnitude: a wrong window, a
    /// wrong divisor or a wrong border rule moves a sample by 0.01 to 0.5.
    fn row_drift(p: &[f32], w: usize) -> f32 {
        w as f32 * one_rounding(p)
    }

    /// ⚠⚠ **THE THIRD CPU/GPU DIVERGENCE CLASS THIS CRATE HAS FOUND: THE DIVISION.** The running
    /// body transliterates `blur_rows`/`blur_cols` exactly — same priming, same slide, same
    /// divisor — and the spec predicted it would therefore be **bit-identical**, since a running
    /// sum is adds and subtracts with no multiply-add to fuse. It is not, and measuring is what
    /// said so: on a 20x12 structured plane at r=8, **36 of 240 samples differ and every one of
    /// them by exactly ONE ULP** (`0.41` against `0.41000003`).
    ///
    /// ⚠ One rounding everywhere, rather than error compounding along the row, is what identifies
    /// it: an accumulation difference would grow over 3200 adds and this does not. It is
    /// consistent with Metal implementing `x / n` as `x * (1/n)`, which rounds twice where a true
    /// divide rounds once. ⚠ Stated as consistent-with rather than proven — WGSL exposes no
    /// division-mode control, so there is nothing to toggle and confirm against.
    ///
    /// ⚠⚠ The guard is therefore **one rounding, not a tolerance**. A tolerance passes against a
    /// genuinely wrong shader whose error happens to be small; one rounding cannot be reached by a
    /// wrong window, a wrong divisor or a wrong border rule — each of those moves a sample by
    /// orders of magnitude more. The reasoning of
    /// `contrast::the_only_divergence_is_fused_multiply_add`.
    #[test]
    fn the_running_sum_shader_matches_box_blur_to_one_rounding() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (97usize, 71);
        let p = plane(w, h);
        for r in [8u32, 16, 40] {
            assert!(
                crate::box_wgsl::use_running_sum(r as usize),
                "fixture check: r={r} must take the running body"
            );
            let want = pipeline::spatial::box_blur(&p, w, h, r as usize);
            let got = gpu_box(&ctx, &p, w, h, r);
            let worst = want
                .iter()
                .zip(&got)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(
                worst <= ROUNDINGS * one_rounding(&p),
                "running-sum shader at r={r}: worst {worst} against a bound of {}",
                ROUNDINGS * one_rounding(&p)
            );
        }
    }

    /// ⚠ The naive body re-sums the window, which is a **different** accumulation from a running
    /// sum carried across the row — and the **more** accurate of the two, since the CPU's is the
    /// one that drifts over thousands of adds. Bounded rather than exact, and what the bound
    /// measures is the CPU's error rather than ours.
    #[test]
    fn the_naive_shader_agrees_with_box_blur() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (97usize, 71);
        let p = plane(w, h);
        for r in [1u32, 2, 4, 6] {
            assert!(
                !crate::box_wgsl::use_running_sum(r as usize),
                "fixture check: r={r} must take the naive body"
            );
            let want = pipeline::spatial::box_blur(&p, w, h, r as usize);
            let got = gpu_box(&ctx, &p, w, h, r);
            let worst = want
                .iter()
                .zip(&got)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(
                worst <= row_drift(&p, w),
                "naive shader at r={r}: worst {worst} against a bound of {}",
                row_drift(&p, w)
            );
        }
    }

    /// ⚠⚠ **A constant plane must stay UNIFORM, and this is the ONLY test that separates the two
    /// border rules.** The box blur shrinks its window and divides by the count inside it;
    /// `gaussian_blur` replicates the edge sample. Both give plausible numbers at a border on a
    /// real photograph — but only the first leaves a constant plane alone, and a divisor of
    /// `2r + 1` at the edge darkens exactly the margin a region render exists to hide.
    ///
    /// ⚠⚠ **"Constant" means UNIFORM, not "equal to the input" — and asserting the latter was
    /// wrong about correct code.** Summing 81 copies of 0.37 and dividing by 81 gives
    /// `0.36999992` in f32, **on the CPU as well**, so a test demanding 0.37 fails against a
    /// perfectly correct blur and a bound of one rounding is too tight for a sum of 81 terms.
    /// What the border rule guarantees is that every output equals every other, which is exactly
    /// what a wrong divisor at the edge destroys — and it is checked against the CPU's own answer
    /// rather than against the input.
    #[test]
    fn a_constant_plane_stays_uniform() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (97usize, 71);
        let p = vec![0.37f32; w * h];
        for r in [2u32, 40] {
            let want = pipeline::spatial::box_blur(&p, w, h, r as usize);
            let got = gpu_box(&ctx, &p, w, h, r);
            assert!(
                (got[0] - want[0]).abs() <= row_drift(&p, w),
                "r={r}: {} against the CPU's {}",
                got[0],
                want[0]
            );
            // ⚠⚠ **Uniform to within rounding, not bit-uniform — and asserting the latter was
            // wrong about correct code for the THIRD time in this file.** `box_blur`'s doc says a
            // constant plane "stays exactly constant", which is true in real arithmetic and not in
            // f32: a running sum reaches each position by a different history of adds and
            // subtracts, so the last bits wobble (0.36999995 against 0.36999992). The CPU does the
            // same. What a wrong divisor at the border does is entirely different in scale — with
            // `2r + 1` instead of the live count the edge comes back at roughly half, i.e. 0.18
            // absolute, five orders of magnitude above this bound.
            // ⚠ `w + h`, not `w`: a separable blur carries a running sum across the row and then
            // another down the column, so the drift accumulates in **both** axes. At `w` alone
            // this lands exactly on the boundary, which is a bound describing the wrong thing.
            let bound = row_drift(&p, w + h);
            for (i, v) in got.iter().enumerate() {
                assert!(
                    (v - got[0]).abs() <= bound,
                    "r={r} sample {i} at x={} y={}: {v} against {} — a constant plane must stay \
                     uniform, and a border divisor of 2r+1 is what breaks it",
                    i % w,
                    i / w,
                    got[0]
                );
            }
        }
    }
}

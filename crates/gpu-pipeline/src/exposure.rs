//! Exposure as a compute shader — the first stage across the seam.

use crate::{GpuContext, GpuImage};
use core_image::ImageBuf;
use pipeline::PipelineStage;
use std::sync::Arc;

/// One multiply per channel. The gain arrives as a uniform.
///
/// ⚠ **No `exp2` in the shader**, deliberately: `2^ev` is computed CPU-side, so the GPU does
/// arithmetic that is exactly representable and the result is bit-identical to the CPU stage.
/// That is what makes a divergence here mean "plumbing bug" rather than "float drift".
pub(crate) const SHADER: &str = r#"
struct Params {
    gain: f32,
    pixels: u32,
    row_groups: u32,
    pad: u32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    data[p] = data[p] * params.gain;
    data[p + 1u] = data[p + 1u] * params.gain;
    data[p + 2u] = data[p + 2u] * params.gain;
}
"#;

/// Exposure, on the GPU.
///
/// ⚠ Holds a CPU stage of its own rather than recomputing the gain, so the fallback path is the
/// real thing and cannot drift from it.
pub struct GpuExposureStage {
    factor: f32,
    cpu: pipeline::ExposureStage,
    ctx: Arc<GpuContext>,
}

impl GpuExposureStage {
    pub fn new(ev: f32, ctx: Arc<GpuContext>) -> Self {
        Self {
            factor: 2.0f32.powf(ev),
            cpu: pipeline::ExposureStage::new(ev),
            ctx,
        }
    }

    /// A fresh instance with the same parameters.
    ///
    /// ⚠ A `Box<dyn GpuStage>` needs ownership, and this stage is cheap to rebuild — an `f32`, a
    /// CPU stage and an `Arc`.
    fn clone_stage(&self) -> Self {
        Self {
            factor: self.factor,
            cpu: pipeline::ExposureStage::new(self.factor.log2()),
            ctx: self.ctx.clone(),
        }
    }
}

impl crate::GpuStage for GpuExposureStage {
    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: +1.29 ms. ⚠⚠ Far under the transfer,
    /// which is the point — exposure alone must stay on the CPU.
    fn cpu_cost(&self) -> f32 {
        1.3
    }

    fn name(&self) -> &'static str {
        "exposure"
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        // ⚠ In-place and per-pixel, so it needs none — the default `scratch_planes`
        // of 0 already says so and the chain lends it an empty slice.
        _scratch: &[wgpu::Buffer],
    ) {
        // ⚠⚠ A `Dispatch` client only since M8.3f. It was written at M8.3a, **before** `Dispatch`
        // existed, and M8.3e made it replaceable without folding it in — so it carried its own
        // ~90-line copy of the layout/bind/pipeline dance and was the one per-pixel stage the
        // shared path did not cover. Measured by subtraction at 3200x2136, that copy was ~0.6 ms
        // of the 1.17 ms per-render encode cost: half of it, for one stage of four.
        let (row_groups, _) = crate::dispatch::Dispatch::grid(img.pixels());
        let params: [u32; 4] = [self.factor.to_bits(), img.pixels(), row_groups, 0];
        crate::dispatch::Dispatch {
            label: "exposure",
            shader: SHADER.to_string(),
            params: bytemuck::cast_slice(&params).to_vec(),
            storages: &[],
        }
        .encode(ctx, pass, img);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

impl PipelineStage for GpuExposureStage {
    /// ⚠ The **same** name as the CPU stage. `Pipeline::stage_names` is what six order-asserting
    /// tests match on, so a GPU substitution must be invisible to them.
    fn name(&self) -> &'static str {
        "exposure"
    }

    /// ⚠⚠ A **one-stage chain**, so there is exactly one upload/download implementation rather
    /// than two that can drift. The fallback and the transfer both live in `GpuChain`.
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(self.ctx.clone(), vec![Box::new(self.clone_stage())]).apply(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared context, or `None` when this machine has no usable adapter.
    ///
    /// ⚠⚠ **A test that cannot get a context SKIPS rather than fails** — the `RAW_FIXTURE`
    /// precedent. CI is Linux and may have no GPU at all, and a suite that goes red there for an
    /// absent device is one people learn to ignore.
    ///
    /// ⚠ And it prints **loudly**, because this project's record is blunt about the cost:
    /// *"a green run does not mean they ran."*
    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// ⚠⚠ **Negative samples and samples above 1.0 are the point of this fixture.** The working
    /// space is scene-referred, so both occur in every real photograph — and a shader that clamped
    /// would be correct on a 0..1 chart while silently destroying every highlight. The same trap
    /// `ProfileHueSatMap`'s `v` and the display rendering's LUT index each documented.
    fn awkward(w: usize, h: usize) -> ImageBuf {
        let n = w * h * 3;
        let data: Vec<f32> = (0..n)
            .map(|i| match i % 7 {
                0 => -0.25,
                1 => 0.0,
                2 => 0.5,
                3 => 1.0,
                4 => 4.0,
                // ⚠⚠ Deliberately **not** `f32::MIN_POSITIVE`. A subnormal input times a gain
                // below 1 produces a subnormal result, which **the GPU flushes to zero and the
                // CPU does not** — see `a_subnormal_result_is_flushed_to_zero_on_the_gpu`, which
                // documents that rather than letting it contaminate every other assertion. This
                // is small but *normal*, and far below anything a photograph contains anyway.
                5 => 1.0e-20,
                _ => (i % 997) as f32 / 997.0,
            })
            .collect();
        ImageBuf::from_vec(w, h, data)
    }

    /// Compare two buffers and report **where** they differ, not what they contain.
    ///
    /// ⚠⚠ A bare `assert_eq!` on two 12-million-element slices printed **254 MB** of numbers,
    /// which is not a diagnosis — it is a denial of service on your own terminal. Any assertion
    /// over a full-frame buffer needs to summarise.
    fn assert_same(gpu: &ImageBuf, cpu: &ImageBuf, what: &str) {
        assert_eq!(
            (gpu.width(), gpu.height()),
            (cpu.width(), cpu.height()),
            "{what}: a wrong stride or truncated readback shows up as the wrong size"
        );
        let (g, c) = (gpu.data(), cpu.data());
        let mut first = None;
        let mut n = 0usize;
        for (i, (a, b)) in g.iter().zip(c).enumerate() {
            if a != b {
                n += 1;
                if first.is_none() {
                    first = Some((i, *a, *b));
                }
            }
        }
        if let Some((i, a, b)) = first {
            panic!(
                "{what}: {n} of {} samples differ; first at index {i} (pixel {}, channel {}): \
                 gpu {a} vs cpu {b}",
                g.len(),
                i / 3,
                i % 3
            );
        }
    }

    #[test]
    fn the_gpu_matches_the_cpu_bit_for_bit() {
        // ⚠ **Exact, not within a tolerance.** The gain is computed CPU-side and passed as a
        // uniform, so the shader does one multiply and no `exp2` — the probe measured worst
        // difference 0. A tolerance here would hide the plumbing bug this test exists to catch.
        let Some(ctx) = ctx() else { return };
        // ⚠ **Both directions.** The first version tested only +1.3 EV, a gain above 1 — and a
        // gain above 1 cannot produce a subnormal, so it was blind to the one real divergence
        // this crate has found. Any future stage's equivalence test wants the same treatment:
        // exercise the parameter on both sides of its neutral value.
        for ev in [1.3f32, -0.5] {
            let img = awkward(64, 48);
            let cpu = pipeline::ExposureStage::new(ev).apply(img.clone());
            let gpu = GpuExposureStage::new(ev, ctx.clone()).apply(img);
            assert_same(
                &gpu,
                &cpu,
                &format!("exposure at {ev} EV is exactly representable on both"),
            );
        }
    }

    #[test]
    fn a_subnormal_result_is_flushed_to_zero_on_the_gpu() {
        // ⚠⚠ **THE FIRST REAL CPU/GPU DIVERGENCE THIS CRATE HAS FOUND, and it is not a bug.**
        // Most GPUs — Metal included — flush subnormal floats to zero; Rust on the CPU preserves
        // them. So "bit-exact" is **not** achievable in general even for a multiply, and every
        // future stage's equivalence test has to know that.
        //
        // ⚠ It does not matter for photographs: a subnormal is below ~1.2e-38, while a
        // scene-referred sample is roughly 1e-4 to 10. Anything that small is already far beneath
        // the precision of every stage downstream. **Recorded, not worked around** — pretending
        // it does not happen is how it would resurface later as an inexplicable tolerance.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(1, 1, vec![f32::MIN_POSITIVE, f32::MIN_POSITIVE, 1.0]);
        let cpu = pipeline::ExposureStage::new(-0.5).apply(img.clone());
        let gpu = GpuExposureStage::new(-0.5, ctx).apply(img);
        assert!(
            cpu.data()[0] > 0.0 && cpu.data()[0] < f32::MIN_POSITIVE,
            "fixture check: the CPU must actually produce a subnormal here, got {}",
            cpu.data()[0]
        );
        assert_eq!(gpu.data()[0], 0.0, "the GPU flushes it to zero");
        assert_eq!(
            gpu.data()[2],
            cpu.data()[2],
            "and a NORMAL sample in the same buffer is still exact"
        );
    }

    #[test]
    fn a_dispatch_larger_than_65535_workgroups_still_works() {
        // ⚠⚠ 65535 is the per-dimension workgroup limit. At 64 threads per group that is
        // 4,194,240 pixels, so this image is deliberately BIGGER. A 1D dispatch passes every small
        // fixture and fails on the first real photograph — the bug already hit by hand while
        // probing, which is why it is a test rather than a memory.
        let Some(ctx) = ctx() else { return };
        let (w, h) = (2500usize, 1700usize);
        assert!(
            w * h > 65535 * 64,
            "fixture check: the limit must be exceeded"
        );
        let img = awkward(w, h);
        let cpu = pipeline::ExposureStage::new(-0.5).apply(img.clone());
        let gpu = GpuExposureStage::new(-0.5, ctx).apply(img);
        assert_same(&gpu, &cpu, "a large frame must dispatch correctly");
    }

    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        // The name is what `Pipeline::stage_names` reports and what order-asserting tests match
        // on, so a GPU substitution must not change it.
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuExposureStage::new(0.0, ctx).name(),
            pipeline::ExposureStage::new(0.0).name()
        );
    }
}

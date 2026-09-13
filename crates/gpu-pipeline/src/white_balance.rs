//! White balance as a compute shader: three multiplies per pixel.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **The gains arrive as uniforms, computed CPU-side by [`pipeline::white_balance_gains`].**
/// The shader does three multiplies and no colour maths at all — no illuminant, no adaptation
/// matrix, no Kelvin. That is what makes the result bit-identical and a divergence here mean
/// *plumbing* rather than float drift, which is `GpuExposureStage`'s reasoning exactly.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
    gr: f32,
    gg: f32,
    gb: f32,
    pad2: f32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    data[p] = data[p] * params.gr;
    data[p + 1u] = data[p + 1u] * params.gg;
    data[p + 2u] = data[p + 2u] * params.gb;
}
"#;

/// White balance, on the GPU.
pub struct GpuWhiteBalanceStage {
    gains: [f32; 3],
    cpu: pipeline::WhiteBalanceStage,
}

impl GpuWhiteBalanceStage {
    pub fn new(gains: [f32; 3], _ctx: &Arc<GpuContext>) -> Self {
        Self {
            gains,
            cpu: pipeline::WhiteBalanceStage::new(gains),
        }
    }
}

impl GpuStage for GpuWhiteBalanceStage {
    fn name(&self) -> &'static str {
        "white_balance"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: **3.77-4.33 ms** over three runs.
    ///
    /// ⚠⚠ The **bottom** of the band, which is the conservative direction for a `cpu_cost`: a
    /// lower number makes a run *less* likely to chain, and the gate must fail toward the CPU.
    /// `TRANSFER_COST` is rounded up from the top of its own band for the mirror reason.
    fn cpu_cost(&self) -> f32 {
        4.0
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        // ⚠ Per-pixel and in-place-safe, so the default `scratch_planes` of 0 covers it.
        _scratch: &[wgpu::Buffer],
    ) {
        let pixels = img.pixels();
        let (row_groups, _) = crate::dispatch::Dispatch::grid(pixels);

        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&pixels.to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[16..20].copy_from_slice(&self.gains[0].to_le_bytes());
        params[20..24].copy_from_slice(&self.gains[1].to_le_bytes());
        params[24..28].copy_from_slice(&self.gains[2].to_le_bytes());

        crate::dispatch::Dispatch {
            label: "white_balance",
            shader: SHADER.to_string(),
            params: params.to_vec(),
            storages: &[],
        }
        .encode(ctx, pass, img);
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

    /// ⚠⚠ Negatives, values above 1.0 and a neutral — the working space is scene-referred, so a
    /// shader that clamped would be correct on a 0..1 chart and silently destroy every highlight.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, gains: [f32; 3], img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuWhiteBalanceStage::new(gains, ctx))],
        )
        .apply(img)
    }

    #[test]
    fn the_gpu_matches_the_cpu_bit_for_bit() {
        let Some(ctx) = ctx() else { return };
        // ⚠⚠ **Asymmetric gains, and both sides of neutral.** Equal gains would pass against a
        // shader that used `gr` for every channel — precisely the plumbing bug this exists to
        // catch — and one direction only is how `GpuExposureStage`'s first test missed the only
        // real divergence this crate has found.
        for gains in [[1.3f32, 1.0, 0.8], [0.7, 1.0, 1.4]] {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::WhiteBalanceStage::new(gains).apply(img.clone());
            let gpu = chained(&ctx, gains, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.is_exact(),
                "white balance at {gains:?} must be bit-identical: {d:?}"
            );
        }
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what six order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuWhiteBalanceStage::new([1.0; 3], &ctx).name(),
            pipeline::WhiteBalanceStage::new([1.0; 3]).name()
        );
    }
}

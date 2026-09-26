//! White balance as a compute shader: three multiplies per pixel.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **The gains arrive as uniforms, computed CPU-side by [`pipeline::white_balance_gains`].**
/// The shader does three multiplies and no colour maths at all — no illuminant, no adaptation
/// matrix, no Kelvin. That is what makes the result bit-identical and a divergence here mean
/// *plumbing* rather than float drift, which is `GpuExposureStage`'s reasoning exactly.
const WGSL: &str = r#"
struct Params_white_balance {
    gr: f32,
    gg: f32,
    gb: f32,
    pad0: f32,
};
fn apply_white_balance(p: vec3<f32>, x: u32, y: u32, q: Params_white_balance, base: u32) -> vec3<f32> {
    return vec3<f32>(p.x * q.gr, p.y * q.gg, p.z * q.gb);
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
        crate::per_pixel::encode_single(self, ctx, pass, img);
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&self.gains[0].to_le_bytes());
        params[4..8].copy_from_slice(&self.gains[1].to_le_bytes());
        params[8..12].copy_from_slice(&self.gains[2].to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "white_balance",
            wgsl: WGSL.to_string(),
            params: params.to_vec(),
            tables: Vec::new(),
            preludes: crate::per_pixel::Preludes::default(),
            needs_position: false,
        })
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

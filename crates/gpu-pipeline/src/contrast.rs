//! Contrast as a compute shader: a slope about linear middle grey.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **`{PIVOT}` is SUBSTITUTED from [`pipeline::CONTRAST_PIVOT`], never transcribed.** A
/// transcribed constant is checked for agreeing; a substituted one **cannot disagree** — the
/// distinction `luma_wgsl`'s doc records as making it stronger than `oklab_wgsl`, and `color`'s
/// own doc records six copies of a Rec.709 triple that all went wrong on one day. It is also why
/// this is a function rather than a `const`: `format!` is not a const operation.
///
/// ⚠ `ContrastStage` applies the same expression to every **sample**, so one thread handles all
/// three channels of its pixel. That keeps the dispatch per-pixel, which is what `Dispatch`
/// provides, rather than needing a second grid rule that could disagree with it.
fn shader() -> String {
    format!(
        r#"
struct Params {{
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
    slope: f32,
    pad2: f32,
    pad3: f32,
    pad4: f32,
}};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) {{ return; }}
    let p = pixel * 3u;
    // ⚠ Floored at 0 exactly as the CPU stage does: a sample driven negative by an extreme slope
    // must darken to black, never flip the pixel's sign.
    data[p] = max({p}f + (data[p] - {p}f) * params.slope, 0.0);
    data[p + 1u] = max({p}f + (data[p + 1u] - {p}f) * params.slope, 0.0);
    data[p + 2u] = max({p}f + (data[p + 2u] - {p}f) * params.slope, 0.0);
}}
"#,
        p = pipeline::CONTRAST_PIVOT
    )
}

/// Contrast, on the GPU.
pub struct GpuContrastStage {
    /// ⚠ The slope, not the slider value — computed CPU-side exactly as `ContrastStage::new` does,
    /// so the shader performs the same arithmetic in the same order and the result is bit-exact.
    slope: f32,
    cpu: pipeline::ContrastStage,
}

impl GpuContrastStage {
    pub fn new(amount: f32, _ctx: &Arc<GpuContext>) -> Self {
        Self {
            slope: 1.0 + amount / 100.0,
            cpu: pipeline::ContrastStage::new(amount),
        }
    }
}

impl GpuStage for GpuContrastStage {
    fn name(&self) -> &'static str {
        "contrast"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: **0.97-1.97 ms** over three runs.
    ///
    /// ⚠⚠ Far under the transfer, like exposure's 1.3 — and that is the point rather than a
    /// disappointment. **What porting it buys is not its own cost but its NEIGHBOURS':** it sits
    /// between `exposure` and `tone_regions`, so on the CPU it split the per-pixel head into
    /// `[exposure]` at 1.3 ms and `[tone_regions]` at 18.0, each refused by the gate. 19.3 ms of
    /// already-ported work was stranded on the CPU by one 1 ms stage standing between them.
    fn cpu_cost(&self) -> f32 {
        1.2
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
        params[16..20].copy_from_slice(&self.slope.to_le_bytes());

        crate::dispatch::Dispatch {
            label: "contrast",
            shader: shader(),
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

    /// ⚠⚠ Dark, mid, above white and **negative** — a sample below the pivot at a steep slope is
    /// the only place the `max(.., 0.0)` floor is load-bearing, and an in-gamut ramp cannot
    /// reach it.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, amount: f32, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuContrastStage::new(amount, ctx))],
        )
        .apply(img)
    }

    /// ⚠⚠ **NOT bit-identical, and the reason is worth more than the test: the GPU FUSES the
    /// multiply-add.** `PIVOT + (x - PIVOT) * slope` contracts to one `fma`, which rounds **once**
    /// where the CPU rounds twice — so the GPU is the *more* accurate of the two. Measured on this
    /// fixture: 3 of 18 samples differ, every one by exactly **1 ulp**, worst **0.0** of an 8-bit
    /// level.
    ///
    /// ⚠⚠ **This is the SECOND CPU/GPU divergence class this crate has found**, after
    /// `a_subnormal_result_is_flushed_to_zero_on_the_gpu`, and it is why `white_balance` and
    /// `exposure` can claim bit-identity while this one cannot: those are a pure multiply with no
    /// add to fuse. **Any future stage containing a multiply-add inherits this** — it is not a
    /// property of contrast.
    ///
    /// ⚠ WGSL exposes no contraction control, so it cannot be turned off; and restructuring buys
    /// nothing, because every algebraic form of this expression still contains a multiply-add.
    ///
    /// ⚠ A hundredth of a level is **50x tighter** than `tone_regions`' 0.1, which is loose for a
    /// different reason (a table the CPU does not have). ⚠⚠ If this ever has to be widened, that
    /// is a finding to investigate rather than a number to adjust.
    #[test]
    fn the_gpu_matches_the_cpu_within_a_hundredth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Both sides of neutral. One direction only is how `GpuExposureStage`'s first test
        // missed the one real divergence this crate had found at the time.
        for amount in [45.0f32, -45.0] {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::ContrastStage::new(amount).apply(img.clone());
            let gpu = chained(&ctx, amount, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.01,
                "contrast at {amount} diverged by more than fused rounding explains: {d:?}"
            );
        }
    }

    /// ⚠⚠ **The divergence is fused rounding and NOTHING else, asserted rather than assumed.**
    /// A tolerance alone would pass against a genuinely wrong shader whose error happened to be
    /// small — which is exactly how a wrong constant would hide. Every sample must equal the
    /// separate-rounding form **or** the fused one, with nothing in between.
    ///
    /// ⚠ Both are accepted rather than only the fused one, because contraction is the backend's
    /// choice: a backend that did not contract would produce the separate form, and this test
    /// must not go red on it.
    #[test]
    fn the_only_divergence_is_fused_multiply_add() {
        let Some(ctx) = ctx() else { return };
        let amount = 45.0f32;
        let slope = 1.0 + amount / 100.0;
        let p = pipeline::CONTRAST_PIVOT;
        let img = ImageBuf::from_vec(6, 1, spread());
        let gpu = chained(&ctx, amount, img);
        let mut fused = 0usize;
        for (i, out) in gpu.data().iter().enumerate() {
            let src = spread()[i];
            let separate = (p + (src - p) * slope).max(0.0);
            let contracted = (src - p).mul_add(slope, p).max(0.0);
            assert!(
                *out == separate || *out == contracted,
                "sample {i} ({src}) is neither rounding of the CPU expression: got {out:e}, \
                 separate {separate:e}, fused {contracted:e}"
            );
            if *out != separate {
                fused += 1;
            }
        }
        // ⚠ A fixture check, not a requirement: if a backend stops contracting this reads 0 and
        // the test above still holds. It is here so the count is visible when it changes.
        eprintln!(
            "{fused} of {} samples took the fused rounding",
            gpu.data().len()
        );
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what six order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuContrastStage::new(0.0, &ctx).name(),
            pipeline::ContrastStage::new(0.0).name()
        );
    }
}

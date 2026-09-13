//! Saturation and vibrance as a compute shader: chroma scaled about the pixel's own luminance.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **`{EPS}` is SUBSTITUTED from [`pipeline::SATURATION_EPS`], and the luminance comes from
/// [`crate::luma_wgsl`]** — so this stage cannot invent a second working-space luma, which is the
/// failure `color`'s own doc records six copies of. A substituted constant cannot disagree; a
/// transcribed one is merely checked for agreeing.
///
/// ⚠ Vibrance weights by `1 - chroma`, so it moves muted pixels most and leaves a fully saturated
/// one alone. A neutral pixel has chroma 0 and is untouched at **any** setting — which is why a
/// grey fixture cannot see this stage at all.
fn shader() -> String {
    format!(
        r#"
struct Params {{
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
    f_sat: f32,
    vib: f32,
    pad2: f32,
    pad3: f32,
}};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) {{ return; }}
    let p = pixel * 3u;
    let r = data[p]; let g = data[p + 1u]; let b = data[p + 2u];
    // ⚠ The pivot chroma is scaled around: real luminance in the working space.
    let luma = prophoto_luminance(r, g, b);
    let mx = max(r, max(g, b));
    let mn = min(r, min(g, b));
    let chroma = clamp((mx - mn) / max(mx, {eps}f), 0.0, 1.0);
    let f = params.f_sat * (1.0 + params.vib * (1.0 - chroma));
    // ⚠ Floored at 0 exactly as the CPU stage does: an extreme setting must darken to black,
    // never flip a channel's sign.
    data[p] = max(luma + (r - luma) * f, 0.0);
    data[p + 1u] = max(luma + (g - luma) * f, 0.0);
    data[p + 2u] = max(luma + (b - luma) * f, 0.0);
}}
"#,
        eps = pipeline::SATURATION_EPS
    )
}

/// Saturation and vibrance, on the GPU.
pub struct GpuSaturationStage {
    /// ⚠ Derived CPU-side exactly as `SaturationStage::new` does, so the shader performs the same
    /// arithmetic in the same order.
    f_sat: f32,
    vib: f32,
    cpu: pipeline::SaturationStage,
}

impl GpuSaturationStage {
    pub fn new(saturation: f32, vibrance: f32, _ctx: &Arc<GpuContext>) -> Self {
        Self {
            f_sat: 1.0 + saturation / 100.0,
            vib: vibrance / 100.0,
            cpu: pipeline::SaturationStage::new(saturation, vibrance),
        }
    }
}

impl GpuStage for GpuSaturationStage {
    fn name(&self) -> &'static str {
        "saturation"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: **7.19-9.39 ms** over three runs.
    ///
    /// ⚠ The bottom of the band — the conservative direction for a `cpu_cost`, since a lower
    /// number makes a run less likely to chain and the gate must fail toward the CPU.
    fn cpu_cost(&self) -> f32 {
        7.2
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
        params[16..20].copy_from_slice(&self.f_sat.to_le_bytes());
        params[20..24].copy_from_slice(&self.vib.to_le_bytes());

        crate::dispatch::Dispatch {
            label: "saturation",
            shader: format!("{}{}", crate::luma_wgsl::luma_wgsl(), shader()),
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

    /// ⚠⚠ **COLOURED, and that is not a detail.** A neutral pixel has chroma 0, so this stage
    /// leaves it exactly alone at **any** setting — a grey fixture would pass against a shader
    /// that did nothing at all. Spans muted and saturated (so vibrance's `1 - chroma` weighting is
    /// exercised at both ends), plus above white and negative.
    fn spread() -> Vec<f32> {
        vec![
            0.40, 0.38, 0.36, // barely coloured — vibrance's strongest case
            0.80, 0.12, 0.10, // strongly coloured — vibrance's weakest
            0.18, 0.18, 0.18, // neutral: untouched at any setting
            1.60, 0.90, 0.30, // above white
            -0.50, 0.20, 0.05, // negative
            0.05, 0.35, 0.62, // dark and coloured
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, sat: f32, vib: f32, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuSaturationStage::new(sat, vib, ctx))],
        )
        .apply(img)
    }

    /// ⚠ A hundredth of a level rather than bit-identity: `luma + (c - luma) * f` is a
    /// multiply-add and the GPU fuses it, rounding once where the CPU rounds twice. See
    /// `contrast`'s `the_only_divergence_is_fused_multiply_add` for the class.
    #[test]
    fn the_gpu_matches_the_cpu_within_a_hundredth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        // ⚠ **Both parameters, both directions.** Saturation and vibrance are different controls
        // with different weightings, and one direction only is how `GpuExposureStage`'s first
        // test missed the one real divergence this crate had found at the time.
        for (sat, vib) in [(60.0f32, 0.0f32), (-60.0, 0.0), (0.0, 60.0), (0.0, -60.0)] {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::SaturationStage::new(sat, vib).apply(img.clone());
            let gpu = chained(&ctx, sat, vib, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.01,
                "saturation ({sat}, {vib}) diverged by more than fused rounding explains: {d:?}"
            );
        }
    }

    /// ⚠⚠ **The fixture check, and without it every assertion above is vacuous.** A stage that did
    /// nothing would match the CPU perfectly on a grey fixture, so this asserts the CPU stage
    /// really moves these pixels before anything is compared against it.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = pipeline::SaturationStage::new(60.0, 0.0).apply(img.clone());
        let moved = img
            .data()
            .iter()
            .zip(out.data())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            moved >= 12,
            "fixture check: saturation must move most of this fixture, moved {moved} of 18"
        );
    }

    /// ⚠ A neutral pixel is untouched at any setting — chroma 0 means the scale multiplies zero.
    /// If a grey ever moves, the luminance is wrong rather than the scale.
    #[test]
    fn a_neutral_pixel_is_untouched() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(1, 1, vec![0.42, 0.42, 0.42]);
        let out = chained(&ctx, 80.0, -40.0, img);
        for v in out.data() {
            assert!(
                (v - 0.42).abs() < 1e-6,
                "a neutral pixel must survive any setting, got {v}"
            );
        }
    }

    /// ⚠⚠ **The `max(.., 0.0)` floor, and `compare` structurally cannot see it.** An extreme
    /// setting drives a channel below zero; the CPU floors it at black and a shader without the
    /// floor returns the negative value — but `compare` quantizes to 8-bit levels, where a
    /// negative sample and 0 are **the same value**. Sabotaging the floor left every other test in
    /// this module green.
    ///
    /// ⚠⚠ That is the blind spot M8.3e recorded for the output transform's clamp, and it has now
    /// appeared **four times in one day** — here, in `contrast`, in `tone_curve` and in M8.3e
    /// itself. **Any clamp at zero needs a raw-float assertion; a level-based comparison is
    /// structurally blind to it.**
    #[test]
    fn a_channel_driven_negative_is_floored_at_black() {
        let Some(ctx) = ctx() else { return };
        // ⚠ A strongly coloured pixel at a big positive saturation: the channel furthest below
        // the luminance is pushed further still, through zero.
        let img = ImageBuf::from_vec(1, 1, vec![0.9, 0.05, 0.02]);
        let out = chained(&ctx, 300.0, 0.0, img.clone());
        let cpu = pipeline::SaturationStage::new(300.0, 0.0).apply(img);
        assert!(
            cpu.data().contains(&0.0),
            "fixture check: the CPU must actually floor a channel here, got {:?}",
            cpu.data()
        );
        for (i, v) in out.data().iter().enumerate() {
            assert!(
                *v >= 0.0,
                "channel {i} must be floored at black, got {v} — a level comparison cannot see this"
            );
        }
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what six order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuSaturationStage::new(0.0, 0.0, &ctx).name(),
            pipeline::SaturationStage::new(0.0, 0.0).name()
        );
    }
}

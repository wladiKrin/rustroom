//! The camera rendering as a compute shader — the stage active on every photograph.
//!
//! ⚠⚠ **Per channel, not on luminance with a scalar gain.** Every other tonal stage in this
//! pipeline preserves hue deliberately; this one must not. Curving the three channels
//! independently is where a camera rendering's shadow saturation and its roll toward white come
//! from, and it is what the DNG spec does with `ProfileToneCurve`. A shader that "fixed" that
//! would be a different stage wearing the same name — and a neutral fixture could not see it.
//!
//! ⚠ The table is the CPU stage's own 4096 entries, uploaded rather than rebuilt: the size is
//! measured (1024 drifts 0.61 of an 8-bit level from the spline) and `lut_max` moves with the
//! baseline exposure, so a shader that resampled either would be smoothly wrong.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

const WGSL: &str = r#"
struct Params_display_rendering {
    lut_len: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    lut_max: f32,
    chroma_boost: f32,
    shadow_knee: f32,
    pad3: f32,
};
// ⚠ Mirrors `DisplayRenderingStage::map` exactly, including both of its total-function edges:
// a non-positive or non-finite input reads the FIRST entry, and anything past the table's top
// takes the last, which the curve has already flattened to 1.
fn curve_display_rendering(x: f32, q: Params_display_rendering, base: u32) -> f32 {
    let n = q.lut_len;
    if (!(x > 0.0)) { return tables[base]; }          // ⚠ catches NaN too, unlike `x <= 0.0`
    let pos = min(x / q.lut_max, 1.0) * f32(n - 1u);
    let i = u32(floor(pos));
    if (i >= n - 1u) { return tables[base + n - 1u]; }
    return tables[base + i] + (pos - f32(i)) * (tables[base + i + 1u] - tables[base + i]);
}
fn apply_display_rendering(p: vec3<f32>, x: u32, y: u32, q: Params_display_rendering, base: u32) -> vec3<f32> {
    let o = vec3<f32>(
        curve_display_rendering(p.x, q, base),
        curve_display_rendering(p.y, q, base),
        curve_display_rendering(p.z, q, base));
    let yl = prophoto_luminance(o.x, o.y, o.z);

    // ⚠ `1 - y^4` is flat through the midtones and falls away only as white approaches; the
    // shadow knee is there because what reads as saturation near black is chroma NOISE, which a
    // boost would make worse on any high-ISO file.
    let yc = clamp(yl, 0.0, 1.0);
    let shadow = min(yc / q.shadow_knee, 1.0);
    let s = 1.0 + (q.chroma_boost - 1.0) * shadow * (1.0 - yc * yc * yc * yc);

    // ⚠ Clamped: a bright saturated pixel scaled up can leave [0,1], and letting one channel clip
    // while its neighbours do not would shift the hue — the one thing this stage's per-channel
    // curve is careful to do only where the *curve* says so.
    return vec3<f32>(
        clamp(yl + (o.x - yl) * s, 0.0, 1.0),
        clamp(yl + (o.y - yl) * s, 0.0, 1.0),
        clamp(yl + (o.z - yl) * s, 0.0, 1.0));
}
"#;

/// The camera rendering, on the GPU.
pub struct GpuDisplayRenderingStage {
    /// ⚠ The CPU stage's **own** table. Never rebuilt here.
    table: Vec<f32>,
    lut_max: f32,
    cpu: pipeline::DisplayRenderingStage,
}

impl GpuDisplayRenderingStage {
    /// ⚠⚠ `curve` must already have been through [`pipeline::effective_profile_curve`], which is
    /// what the tail applies before building the CPU stage. `accelerate`'s table row does that;
    /// a caller that skipped it would render a photograph whose profile declares an identity
    /// curve differently from the CPU — which is most cameras, the X-T4 included.
    pub fn new(
        curve: Option<&[[f32; 2]]>,
        baseline_ev: Option<f32>,
        _ctx: &Arc<GpuContext>,
    ) -> Self {
        let cpu = pipeline::DisplayRenderingStage::new(curve, baseline_ev);
        Self {
            table: cpu.lut().to_vec(),
            lut_max: cpu.lut_max(),
            cpu,
        }
    }
}

impl GpuStage for GpuDisplayRenderingStage {
    fn name(&self) -> &'static str {
        "display_rendering"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: +25.73 ms — the largest delta of any
    /// optional stage in the tail, and active on 100% of this catalogue.
    fn cpu_cost(&self) -> f32 {
        25.7
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        _scratch: &[wgpu::Buffer],
    ) {
        crate::per_pixel::encode_single(self, ctx, pass, img);
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&(self.table.len() as u32).to_le_bytes());
        params[16..20].copy_from_slice(&self.lut_max.to_le_bytes());
        // ⚠ Uploaded from the CPU stage's own constants rather than re-typed into WGSL.
        params[20..24].copy_from_slice(&pipeline::CHROMA_BOOST.to_le_bytes());
        params[24..28].copy_from_slice(&pipeline::SHADOW_KNEE.to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "display_rendering",
            wgsl: WGSL.to_string(),
            params: params.to_vec(),
            tables: vec![&self.table],
            preludes: crate::per_pixel::Preludes {
                luma: true,
                ..Default::default()
            },
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

    /// ⚠⚠ Spans dark, mid, **above white** and **negative**, plus a neutral.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn cpu_of() -> pipeline::DisplayRenderingStage {
        pipeline::DisplayRenderingStage::new(None, None)
    }

    fn chained(ctx: &Arc<GpuContext>, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuDisplayRenderingStage::new(None, None, ctx))],
        )
        .apply(img)
    }

    #[test]
    fn the_gpu_rendering_matches_the_cpu_within_a_tenth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = cpu_of().apply(img.clone());
        let gpu = chained(&ctx, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ The equivalence test above passes against a GPU **and** CPU stage that both do nothing.
    #[test]
    fn the_stage_really_renders_rather_than_copying() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = chained(&ctx, img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 20.0,
            "the rendering must move the fixture; moved only {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ **The one tonal stage that deliberately does NOT preserve hue.** A shader that curved
    /// luminance and scaled RGB instead would be a different stage wearing the same name — ⚠ and
    /// every other test here would still pass, because a neutral or near-neutral sample cannot
    /// see the difference.
    #[test]
    fn a_saturated_pixel_shifts_hue_exactly_as_the_cpu_does() {
        let Some(ctx) = ctx() else { return };
        // A deep saturated red in shadow: where a per-channel curve and a hue-preserving one
        // disagree most.
        let img = ImageBuf::from_vec(1, 1, vec![0.22, 0.02, 0.015]);
        let cpu = cpu_of().apply(img.clone());
        let gpu = chained(&ctx, img.clone());
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
        // ⚠ The fixture check: the two must have something to disagree about. A channel ratio
        // preserved exactly would mean the CPU stage is hue-preserving after all, and this test
        // would be asserting nothing.
        let (i, o) = (img.data(), gpu.data());
        let before = i[0] / i[1];
        let after = o[0] / o[1];
        assert!(
            (before / after - 1.0).abs() > 0.1,
            "the channel ratio must move: {before} -> {after}"
        );
    }

    /// ⚠ A negative sample must read the table's first entry, not index out of it or return NaN.
    /// `spread()` carries one, but this fails with a coordinate rather than a worst-case level.
    #[test]
    fn a_negative_sample_reads_the_bottom_of_the_curve() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(1, 1, vec![-0.5, -0.01, 0.0]);
        let gpu = chained(&ctx, img.clone());
        assert!(
            gpu.data().iter().all(|v| v.is_finite()),
            "a negative input must stay finite, got {:?}",
            gpu.data()
        );
        let cpu = cpu_of().apply(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
    }

    #[test]
    fn it_asks_for_no_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        let chain = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuDisplayRenderingStage::new(None, None, &ctx))],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }
}

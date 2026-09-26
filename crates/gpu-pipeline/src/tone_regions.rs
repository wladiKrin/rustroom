//! The four tone-region sliders as a compute shader.
//!
//! ⚠⚠ **The whole four-slider model is one gain that depends on luminance alone**, so it is a 1-D
//! table and this shader contains no weight shaping at all. That matters beyond brevity: the
//! weights are computed on **display-encoded** luminance — the fix for "the Whites slider did
//! nothing on most photographs" — and re-deriving that in WGSL is how the two would drift on
//! something nobody re-checks.
//!
//! ⚠ Unlike `tone_curve` and `display_rendering`, this table does **not** already exist on the
//! CPU, so building it introduces a quantization the CPU stage does not have. It is bounded by
//! `ToneRegionsStage::LUT_SIZE`'s own sweep at 5.1x margin, and it is the deliberate price of
//! there being exactly one `gain_for_luma`.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Edits;
use pipeline::PipelineStage;
use std::sync::Arc;

const WGSL: &str = r#"
struct Params_tone_regions {
    lut_len: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    top: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
};
fn apply_tone_regions(p: vec3<f32>, x: u32, y: u32, q: Params_tone_regions, base: u32) -> vec3<f32> {
    let l = prophoto_luminance(p.x, p.y, p.z);
    // ⚠⚠ Clamp the INDEX, never l itself. A scene-referred highlight above the table's top takes
    // the last entry, which is the EXACT answer rather than an approximation -- gain_for_luma
    // clamps its own input, so the gain really is flat up there.
    let t = clamp(l / q.top, 0.0, 1.0) * f32(q.lut_len - 1u);
    let i = min(u32(floor(t)), q.lut_len - 1u);
    let j = min(i + 1u, q.lut_len - 1u);
    // ⚠ Floored at 0 exactly as the CPU stage does: a gain driven negative by extreme sliders
    // must darken to black, never flip the pixel's sign.
    let gain = max(mix(tables[base + i], tables[base + j], t - f32(i)), 0.0);
    return vec3<f32>(p.x * gain, p.y * gain, p.z * gain);
}
"#;

/// Highlights, shadows, whites and blacks, on the GPU.
pub struct GpuToneRegionsStage {
    /// ⚠ The CPU stage's **own** table. Never rebuilt here.
    table: Vec<f32>,
    cpu: pipeline::ToneRegionsStage,
}

impl GpuToneRegionsStage {
    pub fn new(e: &Edits, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::ToneRegionsStage::new(e.highlights, e.shadows, e.whites, e.blacks);
        let table = cpu.gain_lut();
        Self { table, cpu }
    }
}

impl GpuStage for GpuToneRegionsStage {
    fn name(&self) -> &'static str {
        "tone_regions"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: +18.10 ms.
    fn cpu_cost(&self) -> f32 {
        18.0
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
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&(self.table.len() as u32).to_le_bytes());
        params[16..20].copy_from_slice(&pipeline::ToneRegionsStage::LUT_TOP.to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "tone_regions",
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

    /// All four sliders doing something, so no region is inert.
    fn edits() -> Edits {
        Edits {
            highlights: -60.0,
            shadows: 45.0,
            whites: 70.0,
            blacks: -35.0,
            ..Edits::default()
        }
    }

    /// ⚠⚠ Spans dark, mid, **above white** and **negative**, plus a neutral — the regions where a
    /// wrong clamp hides and an in-gamut ramp cannot see it.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, e: &Edits, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneRegionsStage::new(e, ctx))],
        )
        .apply(img)
    }

    fn cpu_of(e: &Edits) -> pipeline::ToneRegionsStage {
        pipeline::ToneRegionsStage::new(e.highlights, e.shadows, e.whites, e.blacks)
    }

    /// ⚠ The band is wider than the transcendental stages' 0.01 because this stage's table does
    /// not exist on the CPU, so its quantization is real — bounded by
    /// `the_gain_table_resolves_the_curve` at a tenth of a level, with 5.1x margin.
    /// ⚠⚠ If it ever has to be widened **further**, that is a finding to investigate rather than
    /// a number to adjust.
    #[test]
    fn the_gpu_tone_regions_match_the_cpu_within_a_tenth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        let e = edits();
        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = cpu_of(&e).apply(img.clone());
        let gpu = chained(&ctx, &e, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ Every equivalence test above passes against a GPU **and** CPU stage that both do
    /// nothing, because they compare the two against each other. This says the fixture is one the
    /// stage actually moves — and the goldens cannot see this crate at all.
    #[test]
    fn the_stage_really_moves_the_tones_rather_than_copying() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = chained(&ctx, &edits(), img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 5.0,
            "the fixture must be one the tone regions change; moved only {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ A luminance above the table's top must take the last entry, not be dragged back — the
    /// **index** is clamped, never the value. Scene-referred highlights are routine, and this is
    /// the identical trap `ProfileHueSatMap`'s `v` and the grade's `lab.x` are each documented
    /// for. ⚠ On an in-gamut fixture it is invisible, and the result is still a plausible picture.
    #[test]
    fn a_luminance_above_the_tables_top_keeps_its_headroom() {
        let Some(ctx) = ctx() else { return };
        let e = edits();
        let img = ImageBuf::from_vec(2, 1, vec![6.0, 5.5, 5.0, 0.4, 0.4, 0.4]);
        let cpu = cpu_of(&e).apply(img.clone());
        let gpu = chained(&ctx, &e, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
        // ⚠ The fixture check: the bright pixel must still BE bright, or this would pass against
        // two implementations that both crushed it.
        assert!(
            gpu.data()[0] > 3.0,
            "the highlight was crushed to {}",
            gpu.data()[0]
        );
    }

    /// ⚠ So adding it to a chain still allocates nothing.
    #[test]
    fn it_asks_for_no_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        let chain = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneRegionsStage::new(&edits(), &ctx))],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }
}

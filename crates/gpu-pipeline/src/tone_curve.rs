//! The point tone curve as a compute shader: a 1024-entry LUT with linear interpolation.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **The shader reproduces [`pipeline::ToneCurveStage::sample`] exactly, CLAMP INCLUDED.**
/// `sample` clamps its input to `[0, 1]` *before* indexing, so a scene-referred highlight above
/// 1.0 is pulled down to the curve's top entry. That is the CPU's behaviour, so parity requires
/// it — and it is the **opposite** of the rule `ProfileHueSatMap`'s `v`, `ColorGradingStage`'s
/// index and `tone_regions`' `l` each follow, where the *index* clamps and the *value* keeps its
/// headroom. A shader that "fixed" it would be a different stage wearing the same name, and only
/// a fixture carrying samples above 1.0 can see the difference.
///
/// ⚠ `ToneCurveStage` applies the curve to every **sample**, so one thread handles all three
/// channels of its pixel — keeping the dispatch per-pixel, which is what `Dispatch` provides.
///
/// ⚠⚠ **Four tables in one buffer**, composite then red, green, blue, indexed by `stride`. The shader
/// is **unconditional**, so a channel with no curve still has a table — an identity one, built by
/// `ToneCurveStage::luts`. A branch per channel would cost more than the identity lookup it saves.
const WGSL: &str = r#"
struct Params_tone_curve {
    last: u32,
    // ⚠ Entries per table. The four tables — composite, red, green, blue — are one buffer, so a
    // channel is reached by adding `stride * (channel + 1)` to the base.
    stride: u32,
    pad0: u32,
    pad1: u32,
};
fn curve_tone_curve(v_in: f32, at: u32, last: u32) -> f32 {
    let v = clamp(v_in, 0.0, 1.0);
    let pos = v * f32(last);
    let lo = u32(floor(pos));
    if (lo >= last) { return tables[at + last]; }
    let frac = pos - f32(lo);
    return tables[at + lo] + frac * (tables[at + lo + 1u] - tables[at + lo]);
}
fn apply_tone_curve(p: vec3<f32>, x: u32, y: u32, q: Params_tone_curve, base: u32) -> vec3<f32> {
    // ⚠⚠ **Composite first, then the channel** — the order `ToneCurveStage::with_channels` documents.
    // Changing it means changing both, and only a fixture where the two orders differ can see it.
    return vec3<f32>(
        curve_tone_curve(curve_tone_curve(p.x, base, q.last), base + q.stride, q.last),
        curve_tone_curve(curve_tone_curve(p.y, base, q.last), base + q.stride * 2u, q.last),
        curve_tone_curve(curve_tone_curve(p.z, base, q.last), base + q.stride * 3u, q.last));
}
"#;

/// The tone curve, on the GPU.
pub struct GpuToneCurveStage {
    /// ⚠⚠ The CPU stage's **own** compiled curves, never rebuilt here — all four, concatenated.
    /// Two constructions of one spline is how a preview stops being evidence about the render, the
    /// rule `ColorGradingStage::lut` and `HueTable::entries` already follow.
    luts: Vec<f32>,
    cpu: pipeline::ToneCurveStage,
}

impl GpuToneCurveStage {
    pub fn new(points: &[[f32; 2]], _ctx: &Arc<GpuContext>) -> Self {
        Self::from_cpu(pipeline::ToneCurveStage::new(points))
    }

    /// The composite plus three per-channel curves.
    ///
    /// ⚠⚠ **`accelerate` must call this and not `new`.** The GPU tail is on by default, so a
    /// construction site that passed only the composite would render a preview that **disagrees with
    /// the export** — the worst failure shape this project has, and the one it has already paid for
    /// three times in one milestone.
    pub fn with_channels(
        points: &[[f32; 2]],
        red: &[[f32; 2]],
        green: &[[f32; 2]],
        blue: &[[f32; 2]],
        _ctx: &Arc<GpuContext>,
    ) -> Self {
        Self::from_cpu(pipeline::ToneCurveStage::new(points).with_channels(red, green, blue))
    }

    /// Every curve an [`edit_model::Edits`] carries — the one a render builds.
    ///
    /// ⚠⚠ `accelerate` calls this, mirroring `pipeline::ToneCurveStage::from_edits`, so the GPU
    /// table and the CPU table come from one constructor and a new curve field cannot reach one
    /// and not the other.
    pub fn from_edits(e: &edit_model::Edits, _ctx: &Arc<GpuContext>) -> Self {
        Self::from_cpu(pipeline::ToneCurveStage::from_edits(e))
    }

    /// ⚠ One place builds the buffer, so `new` and `with_channels` cannot lay it out differently —
    /// and `cpu` carries the channels too, which is what makes the **decline path** correct.
    fn from_cpu(cpu: pipeline::ToneCurveStage) -> Self {
        Self {
            luts: cpu.luts().concat(),
            cpu,
        }
    }
}

impl GpuStage for GpuToneCurveStage {
    fn name(&self) -> &'static str {
        "tone_curve"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: **8.07-9.27 ms** over three runs.
    ///
    /// ⚠ The bottom of the band — the conservative direction for a `cpu_cost`, since a lower
    /// number makes a run less likely to chain and the gate must fail toward the CPU.
    fn cpu_cost(&self) -> f32 {
        8.0
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
        // ⚠ Passed rather than written as a literal in the shader, so the table's length and the
        // index bound cannot disagree.
        let stride = pipeline::ToneCurveStage::LUT_LEN as u32;
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&(stride - 1).to_le_bytes());
        params[4..8].copy_from_slice(&stride.to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "tone_curve",
            wgsl: WGSL.to_string(),
            params: params.to_vec(),
            tables: vec![&self.luts],
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

    /// A curve with real curvature in both directions — a straight line would pass against a
    /// shader that ignored the table entirely.
    fn curvy() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [0.25, 0.38], [0.75, 0.62], [1.0, 1.0]]
    }

    /// ⚠⚠ Dark, mid, **above white** and **negative** — the clamp is only visible above 1.0, and
    /// the bottom of the curve only below 0.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, points: &[[f32; 2]], img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneCurveStage::new(points, ctx))],
        )
        .apply(img)
    }

    /// ⚠ A hundredth of a level rather than bit-identity: the interpolation is a multiply-add and
    /// the GPU fuses it, which rounds once where the CPU rounds twice. See `contrast`'s
    /// `the_only_divergence_is_fused_multiply_add` for the class.
    #[test]
    fn the_gpu_matches_the_cpu_within_a_hundredth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        // ⚠ A curved case **and** a two-point identity, which is the neutral this stage ships at.
        for points in [curvy(), vec![[0.0, 0.0], [1.0, 1.0]]] {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::ToneCurveStage::new(&points).apply(img.clone());
            let gpu = chained(&ctx, &points, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.01,
                "tone curve {points:?} diverged by more than fused rounding explains: {d:?}"
            );
        }
    }

    /// ⚠⚠ **The parity that matters most for this feature.** The GPU tail is on by default, so if the
    /// shader composed the curves in the other order — or read the wrong table — the **preview would
    /// disagree with the export** and nothing else would notice.
    ///
    /// ⚠ **Three DIFFERENT channel curves**, so a shader that read one table for all three, or offset
    /// the base wrongly, cannot pass. Three copies of one would.
    #[test]
    fn the_gpu_matches_the_cpu_with_three_different_channel_curves() {
        let Some(ctx) = ctx() else { return };
        let composite = curvy();
        let red = vec![[0.0, 0.18], [1.0, 1.0]];
        let green = vec![[0.0, 0.0], [0.5, 0.58], [1.0, 1.0]];
        let blue = vec![[0.0, 0.0], [1.0, 0.82]];

        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = pipeline::ToneCurveStage::new(&composite)
            .with_channels(&red, &green, &blue)
            .apply(img.clone());
        let gpu = crate::chain::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneCurveStage::with_channels(
                &composite, &red, &green, &blue, &ctx,
            ))],
        )
        .apply(img.clone());

        // ⚠ The fixture must actually separate the channels, or this would pass on a grey image.
        let a = [cpu.data()[3], cpu.data()[4], cpu.data()[5]];
        assert!(
            (a[0] - a[1]).abs() > 0.02 && (a[1] - a[2]).abs() > 0.02,
            "the three channels must land apart for this to prove anything: {a:?}"
        );

        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "per-channel curves diverged by more than fused rounding explains: {d:?}"
        );
    }

    /// ⚠⚠ **The clamp, and it needs a fixture above 1.0 or it is untested.** `sample` clamps its
    /// input before indexing, so every scene-referred highlight takes the curve's **top entry** —
    /// the opposite of the index-clamping rule every other table in this crate follows. Asserted
    /// against the table directly rather than against the CPU stage, so it cannot pass by both
    /// sides making the same mistake.
    #[test]
    fn a_sample_above_one_takes_the_curves_top_entry() {
        let Some(ctx) = ctx() else { return };
        let points = curvy();
        let top = *pipeline::ToneCurveStage::new(&points).lut().last().unwrap();
        let img = ImageBuf::from_vec(1, 1, vec![1.6, 4.0, 1.0]);
        let out = chained(&ctx, &points, img);
        for (i, v) in out.data().iter().enumerate() {
            assert!(
                (v - top).abs() < 1e-6,
                "sample {i} above or at white must take the top entry {top}, got {v}"
            );
        }
    }

    /// ⚠⚠ **The clamp's OTHER half, and the half that is load-bearing.** Above 1.0 the clamp is
    /// redundant — `lo >= last` catches it anyway — so sabotaging the clamp leaves
    /// `a_sample_above_one_takes_the_curves_top_entry` green. Below 0 it is the only thing
    /// standing between the shader and a hugely negative `frac`: `u32(floor(-511.5))` saturates
    /// to 0 on Metal, so the interpolation runs with `frac = -511.5` and extrapolates far off the
    /// bottom of the curve.
    ///
    /// ⚠⚠ **And `compare` structurally cannot see it**, because it quantizes to 8-bit levels
    /// where a negative sample and 0 are the same value — the blind spot M8.3e recorded for the
    /// output transform's clamp, met for the third time today. So this asserts on the **raw
    /// float**, against the table directly.
    #[test]
    fn a_negative_sample_takes_the_curves_bottom_entry() {
        let Some(ctx) = ctx() else { return };
        let points = curvy();
        let bottom = pipeline::ToneCurveStage::new(&points).lut()[0];
        let img = ImageBuf::from_vec(1, 1, vec![-0.5, -4.0, 0.0]);
        let out = chained(&ctx, &points, img);
        for (i, v) in out.data().iter().enumerate() {
            assert!(
                (v - bottom).abs() < 1e-6,
                "sample {i} at or below black must take the bottom entry {bottom}, got {v}"
            );
        }
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what six order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuToneCurveStage::new(&curvy(), &ctx).name(),
            pipeline::ToneCurveStage::new(&curvy()).name()
        );
    }
}

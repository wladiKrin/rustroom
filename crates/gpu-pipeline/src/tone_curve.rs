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
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    last: u32,
    pad0: u32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> lut: array<f32>;

fn curve(v_in: f32) -> f32 {
    let v = clamp(v_in, 0.0, 1.0);
    let pos = v * f32(params.last);
    let lo = u32(floor(pos));
    if (lo >= params.last) { return lut[params.last]; }
    let frac = pos - f32(lo);
    return lut[lo] + frac * (lut[lo + 1u] - lut[lo]);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    data[p] = curve(data[p]);
    data[p + 1u] = curve(data[p + 1u]);
    data[p + 2u] = curve(data[p + 2u]);
}
"#;

/// The tone curve, on the GPU.
pub struct GpuToneCurveStage {
    /// ⚠⚠ The CPU stage's **own** compiled curve, never rebuilt here. Two constructions of one
    /// spline is how a preview stops being evidence about the render — the rule
    /// `ColorGradingStage::lut` and `HueTable::entries` already follow.
    lut: Vec<f32>,
    cpu: pipeline::ToneCurveStage,
}

impl GpuToneCurveStage {
    pub fn new(points: &[[f32; 2]], _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::ToneCurveStage::new(points);
        Self {
            lut: cpu.lut().to_vec(),
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
        let pixels = img.pixels();
        let (row_groups, _) = crate::dispatch::Dispatch::grid(pixels);

        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&pixels.to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        // ⚠ Passed rather than written as a literal in the shader, so the table's length and the
        // index bound cannot disagree.
        params[8..12].copy_from_slice(&((self.lut.len() - 1) as u32).to_le_bytes());

        crate::dispatch::Dispatch {
            label: "tone_curve",
            shader: SHADER.to_string(),
            params: params.to_vec(),
            storages: &[&self.lut],
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

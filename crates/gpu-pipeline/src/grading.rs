//! Colour grading as a compute shader — the stage that was splitting the merged run.
//!
//! ⚠ Simpler than `hsl`: no `atan2`, no `cos`/`sin`, no band weighting, **no branch**. Per pixel
//! it is an Oklab round trip, one 256-entry table lookup with a lerp, and three adds. The whole
//! three-wheel grade collapsing to a 1-D table on lightness is `ColorGradingStage`'s own founding
//! decision; this inherits it rather than re-deriving it.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::ColorGrading;
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠⚠ **The INDEX is clamped, never the pixel's own lightness.** A scene-referred value above
/// white has `L > 1`, and clamping the *value* would drag every highlight back toward white —
/// destroying exactly the headroom the wide working space exists to preserve. On a photograph that
/// is invisible, because the result is still a plausible picture. The identical trap
/// `ProfileHueSatMap`'s `v` is documented for, and `a_lightness_above_one_keeps_its_headroom` is
/// the only test that can see it.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    lut_len: u32,
    pad0: u32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> lut: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let lab = prophoto_to_oklab(data[p], data[p + 1u], data[p + 2u]);

    // ⚠⚠ clamp the INDEX, not lab.x -- see the constant's doc comment.
    let t = clamp(lab.x, 0.0, 1.0) * f32(params.lut_len - 1u);
    let i = min(u32(floor(t)), params.lut_len - 1u);
    let j = min(i + 1u, params.lut_len - 1u);
    let f = t - f32(i);
    let da = mix(lut[i * 3u],      lut[j * 3u],      f);
    let db = mix(lut[i * 3u + 1u], lut[j * 3u + 1u], f);
    let dl = mix(lut[i * 3u + 2u], lut[j * 3u + 2u], f);

    let out = oklab_to_prophoto(lab.x + dl, lab.y + da, lab.z + db);
    // ⚠ Clamped at 0 only, never at 1: the output transform is the only place clipping belongs.
    data[p]      = max(out.x, 0.0);
    data[p + 1u] = max(out.y, 0.0);
    data[p + 2u] = max(out.z, 0.0);
}
"#;

/// Three-way colour grading, on the GPU.
pub struct GpuGradingStage {
    /// ⚠ The CPU stage's **own** table, flattened — never rebuilt here.
    table: Vec<f32>,
    cpu: pipeline::ColorGradingStage,
}

impl GpuGradingStage {
    pub fn new(g: &ColorGrading, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::ColorGradingStage::new(g);
        let table = cpu.lut().iter().flatten().copied().collect();
        Self { table, cpu }
    }
}

impl GpuStage for GpuGradingStage {
    /// ⚠ Derived from the bench's 1600x1068 delta scaled by 3.9, not measured at 3200 — see
    /// `GpuHslStage::cpu_cost`.
    fn cpu_cost(&self) -> f32 {
        48.0
    }

    fn name(&self) -> &'static str {
        "color_grading"
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        // ⚠ Per-pixel and in-place-safe, so the default `scratch_planes` of 0 already covers it
        // and the chain lends an empty slice. A grade adds nothing to sharpening's four planes.
        _scratch: &[wgpu::Buffer],
    ) {
        let pixels = img.pixels();
        let (row_groups, _) = crate::dispatch::Dispatch::grid(pixels);

        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&pixels.to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[8..12].copy_from_slice(&((self.table.len() / 3) as u32).to_le_bytes());

        crate::dispatch::Dispatch {
            label: "grading",
            shader: format!("{}{}", crate::oklab_wgsl::OKLAB_WGSL, SHADER),
            params: params.to_vec(),
            storages: &[&self.table],
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
    use edit_model::GradeWheel;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A grade with all four wheels doing something, so no range is inert.
    fn graded() -> ColorGrading {
        ColorGrading {
            shadows: GradeWheel {
                hue: 250.0,
                saturation: 60.0,
                luminance: -20.0,
            },
            midtones: GradeWheel {
                hue: 40.0,
                saturation: 35.0,
                luminance: 10.0,
            },
            highlights: GradeWheel {
                hue: 150.0,
                saturation: 45.0,
                luminance: 15.0,
            },
            global: GradeWheel {
                hue: 20.0,
                saturation: 12.0,
                luminance: 5.0,
            },
            blending: 35.0,
            balance: -25.0,
        }
    }

    fn chained(ctx: &Arc<GpuContext>, g: &ColorGrading, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(ctx.clone(), vec![Box::new(GpuGradingStage::new(g, ctx))]).apply(img)
    }

    /// ⚠⚠ Spans dark, mid, **above white** and **negative**, plus a neutral — the regions where a
    /// wrong clamp or a wrong `cbrt_signed` hide and an in-gamut ramp cannot see either.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    #[test]
    fn the_gpu_grade_matches_the_cpu_within_a_hundredth_of_a_level() {
        // ⚠ A tolerance rather than exactness: this stage is transcendental (a cbrt round trip).
        // HSL measured 0.00104 levels and sharpening 0.00905 on the same harness, so 0.01 is the
        // established band. ⚠⚠ If this ever has to be WIDENED that is a finding to investigate,
        // never a number to adjust.
        let Some(ctx) = ctx() else { return };
        let g = graded();
        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = pipeline::ColorGradingStage::new(&g).apply(img.clone());
        let gpu = chained(&ctx, &g, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ **The prelude's cross-check, and it needs NO extra machinery.** A neutral grade has an
    /// all-zero LUT, so the shader reduces to exactly `oklab_to_prophoto(prophoto_to_oklab(rgb))`
    /// — a pure round trip through both shared matrices. Comparing that against `color::oklab`'s
    /// own round trip tests the WGSL against the Rust it must agree with, **through the real
    /// stage**, rather than parsing digits out of a shader string.
    ///
    /// ⚠ Compared against the CPU **round trip**, never against the input: `cbrt` then cube is not
    /// the identity in f32, so asserting `out == in` would be asserting something false about
    /// correct code — the trap this project records five times.
    #[test]
    fn the_wgsl_prelude_agrees_with_color_oklab() {
        let Some(ctx) = ctx() else { return };
        let src = spread();
        let gpu = chained(
            &ctx,
            &ColorGrading::default(),
            ImageBuf::from_vec(6, 1, src.clone()),
        );
        let want: Vec<f32> = src
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| {
                let lab = color::oklab::prophoto_to_oklab([p[0], p[1], p[2]]);
                let o = color::oklab::oklab_to_prophoto(lab);
                [o[0].max(0.0), o[1].max(0.0), o[2].max(0.0)]
            })
            .collect();
        let d = crate::compare(&ImageBuf::from_vec(6, 1, want), &gpu);
        assert!(
            d.worst_levels < 0.01,
            "the WGSL prelude disagrees with color::oklab by {} levels",
            d.worst_levels
        );
    }

    #[test]
    fn a_lightness_above_one_keeps_its_headroom() {
        // ⚠⚠ The index-vs-value clamp guard. `clamp(lab.x, 0, 1)` instead of clamping the index
        // drags every highlight back toward white — invisible on an in-gamut fixture, and still a
        // plausible photograph when it happens.
        //
        // ⚠ Honestly: the equivalence tests catch this too, because `spread()` deliberately
        // carries an above-white sample — verified by sabotage, four tests fail together. What
        // this one adds is the **diagnosis**, failing on a fixture that is nothing but the
        // highlight case, plus a floor under `spread()` ever being narrowed.
        let Some(ctx) = ctx() else { return };
        let g = graded();
        let img = ImageBuf::from_vec(2, 1, vec![2.4, 2.1, 1.8, 0.4, 0.4, 0.4]);
        let cpu = pipeline::ColorGradingStage::new(&g).apply(img.clone());
        let gpu = chained(&ctx, &g, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
        // ⚠ And the fixture check: the bright pixel must still BE bright, or this would pass
        // against two implementations that both crushed it.
        assert!(
            gpu.data()[0] > 1.5,
            "the highlight was crushed to {}",
            gpu.data()[0]
        );
    }

    #[test]
    fn a_negative_sample_does_not_become_nan() {
        // ⚠⚠ The `cbrt` trap. ⚠ `[-0.5, 0.02, 0.03]` is COMPUTED, not guessed: through the
        // ProPhoto→LMS matrix it gives l = -0.353, so `pow` genuinely sees a negative base.
        // M8.3b's first attempt used `[-0.05, 0.3, 0.2]`, whose LMS components are all POSITIVE,
        // and its sabotage passed.
        let Some(ctx) = ctx() else { return };
        let g = graded();
        let img = ImageBuf::from_vec(1, 1, vec![-0.5, 0.02, 0.03]);
        let gpu = chained(&ctx, &g, img.clone());
        assert!(
            gpu.data().iter().all(|v| v.is_finite()),
            "a negative input must stay finite, got {:?}",
            gpu.data()
        );
        // ⚠ Finite alone would pass against a shader that clamped the input to zero.
        let cpu = pipeline::ColorGradingStage::new(&g).apply(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "and must match the CPU, which uses cbrt; moved {} levels",
            d.worst_levels
        );
    }

    #[test]
    fn the_stage_really_grades_rather_than_copying() {
        // ⚠⚠ Every equivalence test above passes against a GPU **and** CPU stage that both do
        // nothing, because they compare the two against each other. This says the fixture is one
        // the grade actually moves.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = chained(&ctx, &graded(), img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 5.0,
            "the fixture must be one the grade changes; moved only {} levels",
            d.worst_levels
        );
    }

    #[test]
    fn it_asks_for_no_scratch_planes() {
        // ⚠ So adding a grade to an HSL chain still allocates zero — the grade is per-pixel and
        // must not widen what the chain reserves.
        let Some(ctx) = ctx() else { return };
        let chain = crate::GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(crate::GpuHslStage::new(&edit_model::Hsl::default(), &ctx)),
                Box::new(GpuGradingStage::new(&graded(), &ctx)),
            ],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }
}

//! HSL as a compute shader — the first expensive stage across the seam.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::hue_table::MAX_HUE_SHIFT_DEG;
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠ Mirrors `stages::hsl`'s own constants. Duplicated rather than re-exported because `pipeline`
/// keeps them private, and widening its API for one consumer is the trade this project declined
/// for `pub mod stages`. **A test asserts the shader agrees with the CPU stage**, which is what
/// would catch them drifting.
const CHROMA_EPS: f32 = 1e-6;
const CHROMA_FADE: f32 = 0.02;

/// ⚠ The Oklab conversions come from `oklab_wgsl::OKLAB_WGSL`, shared with the grading shader —
/// including the `cbrt_signed` trap, which is documented there.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    table_len: u32,
    max_shift_deg: f32,
    chroma_eps: f32,
    chroma_fade: f32,
    pad0: f32,
    pad1: f32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> table: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let r = data[p]; let g = data[p + 1u]; let bl = data[p + 2u];

    // ⚠ The shared prelude, so this shader and the grading one cannot disagree about the
    // matrices. See `oklab_wgsl::OKLAB_WGSL`.
    let lab = prophoto_to_oklab(r, g, bl);
    let bigL = lab.x; let A = lab.y; let B = lab.z;

    let chroma = sqrt(A*A + B*B);
    // ⚠ The early-out is a BRANCH, matching the CPU's `return rgb`. Computing a near-zero
    // adjustment instead would give neutral pixels hue noise, which is what the chroma fade
    // exists to prevent.
    if (chroma < params.chroma_eps) { return; }

    var hue = degrees(atan2(B, A));
    if (hue < 0.0) { hue = hue + 360.0; }

    let x = hue * (f32(params.table_len) / 360.0);
    let i0 = u32(floor(x)) % params.table_len;
    let i1 = (i0 + 1u) % params.table_len;
    let t = x - floor(x);
    var dh  = table[i0*3u]      + (table[i1*3u]      - table[i0*3u])      * t;
    var sat = table[i0*3u + 1u] + (table[i1*3u + 1u] - table[i0*3u + 1u]) * t;
    var lum = table[i0*3u + 2u] + (table[i1*3u + 2u] - table[i0*3u + 2u]) * t;

    let ct = min(chroma / params.chroma_fade, 1.0);
    let strength = ct * ct * (3.0 - 2.0 * ct);
    dh = dh * strength; sat = sat * strength; lum = lum * strength;

    // ⚠ The same identity the CPU uses: cos(hue) and sin(hue) are already in hand as A/chroma and
    // B/chroma, so the chroma cancels and the rotation costs two small-angle polynomials. A naive
    // atan2-then-cos round trip would be a DIFFERENT stage wearing the same name.
    let d = radians(dh * params.max_shift_deg / 100.0);
    let d2 = d * d;
    let sd = d * (1.0 - d2 * (1.0/6.0 - d2 * (1.0/120.0)));
    let cd = 1.0 - d2 * (0.5 - d2 * (1.0/24.0 - d2 * (1.0/720.0)));
    let k = max(1.0 + sat / 100.0, 0.0);
    let L2 = max(bigL * (1.0 + lum / 200.0), 0.0);
    let A2 = k * (A * cd - B * sd);
    let B2 = k * (A * sd + B * cd);

    let out = oklab_to_prophoto(L2, A2, B2);
    data[p]      = max(out.x, 0.0);
    data[p + 1u] = max(out.y, 0.0);
    data[p + 2u] = max(out.z, 0.0);
}
"#;

/// HSL, on the GPU.
pub struct GpuHslStage {
    table: Vec<f32>,
    cpu: pipeline::HslStage,
}

impl GpuHslStage {
    pub fn new(hsl: &edit_model::Hsl, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::HslStage::new(hsl);
        // ⚠ Built from the CPU stage's OWN table rather than rebuilt here, so the two cannot
        // disagree about band blending — the failure a second construction path would invite.
        let table = cpu.table().entries().iter().flatten().copied().collect();
        Self { table, cpu }
    }
}

impl GpuStage for GpuHslStage {
    /// ⚠ Derived, not measured directly: the bench's delta at 1600x1068 scaled by 3.9, because
    /// `probe_stages` cannot reach an already-ported stage at 3200. Comfortably over the bar
    /// either way.
    fn cpu_cost(&self) -> f32 {
        60.0
    }

    fn name(&self) -> &'static str {
        "hsl"
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
        let pixels = img.pixels();
        let (row_groups, _) = crate::dispatch::Dispatch::grid(pixels);
        let table_len = (self.table.len() / 3) as u32;

        // ⚠ Two u32 fields then two f32s then two more f32s: laid out to match the WGSL struct
        // exactly, with explicit padding to 32 bytes. A mismatch here reads garbage rather than
        // failing, which is why the equivalence test is the guard.
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&pixels.to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[8..12].copy_from_slice(&table_len.to_le_bytes());
        params[12..16].copy_from_slice(&MAX_HUE_SHIFT_DEG.to_le_bytes());
        params[16..20].copy_from_slice(&CHROMA_EPS.to_le_bytes());
        params[20..24].copy_from_slice(&CHROMA_FADE.to_le_bytes());

        crate::dispatch::Dispatch {
            label: "hsl",
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
    use edit_model::{Hsl, HslBand};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn bands() -> Hsl {
        let mut h = Hsl::default();
        for (i, b) in h.bands.iter_mut().enumerate() {
            *b = HslBand {
                hue: (i as f32 * 13.0) % 70.0 - 35.0,
                saturation: (i as f32 * 21.0) % 90.0 - 45.0,
                luminance: (i as f32 * 9.0) % 40.0 - 20.0,
            };
        }
        h
    }

    /// ⚠⚠ Spans dark to above-white **and includes near-neutral pixels**, which is where the
    /// chroma early-out and the fade live and where a naive port goes wrong invisibly.
    fn colours(w: usize, h: usize) -> ImageBuf {
        let n = w * h;
        let mut d = Vec::with_capacity(n * 3);
        for i in 0..n {
            let t = (i % 719) as f32 / 719.0;
            match i % 5 {
                0 => d.extend_from_slice(&[t * 1.3, t * 0.4, t * 0.8]),
                1 => d.extend_from_slice(&[t * 0.2, t * 1.1, t * 0.5]),
                2 => d.extend_from_slice(&[t * 0.6, t * 0.6, t * 1.2]),
                3 => d.extend_from_slice(&[t, t + 1e-7, t - 1e-7]),
                _ => d.extend_from_slice(&[t, t, t]),
            }
        }
        ImageBuf::from_vec(w, h, d)
    }

    fn chained(ctx: &Arc<GpuContext>, hsl: &Hsl, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(ctx.clone(), vec![Box::new(GpuHslStage::new(hsl, ctx))]).apply(img)
    }

    #[test]
    fn the_gpu_hsl_matches_the_cpu_within_a_hundredth_of_a_level() {
        // ⚠ A tolerance rather than exactness, unlike exposure: this stage is transcendental. The
        // probe measured this exact maths at **0.0006 levels**, so 0.01 is ~16x headroom and
        // still ~100x tighter than anything visible.
        //
        // ⚠⚠ If this ever has to be WIDENED, that is a finding to investigate rather than a
        // number to adjust — a loose tolerance defeats the harness's whole purpose.
        let Some(ctx) = ctx() else { return };
        let hsl = bands();
        let img = colours(96, 72);
        let cpu = pipeline::HslStage::new(&hsl).apply(img.clone());
        let gpu = chained(&ctx, &hsl, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn black_takes_the_early_out_exactly_and_grey_stays_grey() {
        // ⚠⚠ **The first draft of this test asserted that any grey comes back bit-identical, and
        // that is FALSE of correct code.** A grey in ProPhoto has non-zero Oklab chroma — the
        // matrix rows do not sum equally — so `[0.4, 0.4, 0.4]` sits right at the `EPS = 1e-6`
        // boundary: the CPU took the early-out and the GPU's slightly different chroma did not,
        // giving `0.3999944`. **Exactness across that branch is not achievable**, the same family
        // as the subnormal finding in `exposure.rs`.
        //
        // ⚠ Which also says something about the stage: **only pure black has chroma exactly
        // zero**, so `EPS` almost never fires and the real protection for neutrals is the
        // `CHROMA_FADE` smoothstep. So this asserts the two things that ARE true.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(2, 1, vec![0.0, 0.0, 0.0, 0.4, 0.4, 0.4]);
        let out = chained(&ctx, &bands(), img.clone());

        assert_eq!(
            &out.data()[0..3],
            &[0.0, 0.0, 0.0],
            "black has chroma exactly zero, so the early-out fires on both paths"
        );

        let grey = &out.data()[3..6];
        let d = crate::compare(
            &ImageBuf::from_vec(1, 1, img.data()[3..6].to_vec()),
            &ImageBuf::from_vec(1, 1, grey.to_vec()),
        );
        assert!(
            d.worst_levels < 0.01,
            "a grey must be visually unchanged, moved {} levels",
            d.worst_levels
        );
        // ⚠ And it must stay **neutral**: a hue shift applied to a near-neutral pixel would pull
        // the channels apart, which is exactly what the fade exists to prevent and what a wrong
        // fade would show as a colour cast on grey.
        let spread = grey.iter().cloned().fold(f32::MIN, f32::max)
            - grey.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread < 1e-5,
            "a grey must stay grey; channels spread by {spread}"
        );
    }

    #[test]
    fn a_negative_sample_does_not_become_nan() {
        // ⚠⚠ The `cbrt` trap. WGSL's `pow` returns NaN for a negative base, and a wide working
        // space produces scene-referred negatives routinely. A naive port turns them into NaN,
        // which then propagates through the output matrix and paints black holes.
        // ⚠⚠ **The first fixture here was `[-0.05, 0.3, 0.2]` and it could NOT detect the bug** —
        // through the ProPhoto→LMS matrix that gives l=0.056, m=0.198, s=0.191, **all positive**,
        // so `pow` never sees a negative base and a sabotage replacing `cbrt_signed` with a bare
        // `pow` passed every test. Caught by the sabotage step, not by review.
        //
        // ⚠ This input gives l=-0.353, m=-0.122, s=-0.030 — computed, not guessed — so the branch
        // is genuinely reached. A negative red with small green and blue is what a saturated
        // colour becomes after white balance in a wide working space.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(1, 1, vec![-0.5, 0.02, 0.03]);
        let out = chained(&ctx, &bands(), img);
        assert!(
            out.data().iter().all(|v| v.is_finite()),
            "a negative input must stay finite, got {:?}",
            out.data()
        );
        // ⚠ And it must agree with the CPU, which uses `cbrt` for exactly this reason — finite
        // alone would pass against a shader that clamped the input to zero.
        let cpu = pipeline::HslStage::new(&bands()).apply(ImageBuf::from_vec(
            1,
            1,
            vec![-0.5, 0.02, 0.03],
        ));
        let d = crate::compare(&cpu, &out);
        assert!(
            d.worst_levels < 0.01,
            "and must match the CPU, which uses cbrt; moved {} levels",
            d.worst_levels
        );
    }
}

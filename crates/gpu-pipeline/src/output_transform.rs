//! The output transform as a compute shader — the largest per-pixel stage in the tail.
//!
//! ⚠⚠ **28.63 ms at 3200x2136, second only to the display rendering, and present on EVERY
//! photograph** including a completely neutral one. It is three `powf` per pixel — twenty million
//! transcendentals at a native rung. Leaving it on the CPU caps an ordinary photograph's win at
//! 1.2x instead of 2.2x, and putting it in is what forced `accelerate`'s cost gate: without that,
//! a neutral frame would build a one-stage chain and pay a 34 ms toll for 28.6 ms of work.

use crate::{GpuContext, GpuImage, GpuStage};
use color::{OutputSpace, RenderingIntent};
use pipeline::PipelineStage;
use std::sync::Arc;

/// Which transfer curve, as the shader's uniform sees it.
const MODE_SRGB: u32 = 0;
const MODE_ADOBE: u32 = 1;
const MODE_PROPHOTO: u32 = 2;

const WGSL: &str = r#"
struct Params_output_transform {
    mode: u32,
    // 0 relative colorimetric, 1 perceptual — `color::RenderingIntent`.
    intent: u32,
    // 1 when the target is the working space itself: the gamut map is the identity.
    identity: u32,
    pad2: u32,
    // ⚠ Three vec4 rows rather than a mat3x3: a mat3x3 in a uniform is padded to three vec4s
    // anyway, and writing the rows explicitly means the Rust side cannot disagree with WGSL's
    // alignment rules about where each number lives.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    adobe_gamma: f32,
    prophoto_toe_end: f32,
    pad3: f32,
    pad4: f32,
};

// ⚠⚠ **Clamp FIRST.** Out-of-gamut colour reaches this stage with negative components BY DESIGN
// -- that is the point of a wide working space, and `OutputSpace::from_working`'s own doc says so.
// The CPU's `encode` clamps before its `powf` for exactly this reason, so the clamp is
// load-bearing rather than tidy. ⚠ An in-gamut fixture cannot see a missing one: after the matrix
// every component is already positive.
//
// ⚠⚠ **What an unclamped `pow` actually does was MEASURED, 2026-09-15, and this comment used to
// be wrong about it.** It said WGSL's `pow` "returns NaN for a negative base". The spec says the
// result is **indeterminate**, and on this adapter (Apple M2, Metal) it returns a finite, wrong
// number -- `-6.46` for an input of `-0.5`. That is strictly worse than NaN, which would at least
// propagate visibly to black. See `srgb_wgsl`'s `a_negative_sample_encodes_to_exactly_zero`.
//
// ⚠ The sRGB branch is SUBSTITUTED from `srgb_wgsl` rather than written here, so this stage and a
// luminance-range mask cannot come to disagree about the transfer curve -- the failure `color`'s
// own doc records six copies of. Note it clamps again internally, which is harmless and is what
// lets the rule live in exactly one place.
fn encode_channel_output_transform(v: f32, q: Params_output_transform) -> f32 {
    // ⚠ sRGB and Display P3 share this curve. Using gamma 2.2 for P3 is a common error and an
    // invisible one -- it is wrong by a few percent in the shadows, which reads as a contrast
    // preference rather than a bug.
    if (q.mode == 0u) { return linear_to_srgb(v); }
    let c = clamp(v, 0.0, 1.0);
    if (q.mode == 1u) { return pow(c, 1.0 / q.adobe_gamma); }
    if (c < q.prophoto_toe_end) { return c * 16.0; }
    return pow(c, 1.0 / 1.8);
}

// ⚠⚠ The gamut map is `color::gamut::GamutMap::map`, line for line: an exact per-pixel bisection
// of the boundary, never a table (a bilinear table desaturated a pure sRGB red by 49 levels at the
// gamut's corners — see `color::gamut`). The constants are substituted from that module.
fn inside_output_transform(v: vec3<f32>) -> bool {
    return all(v >= vec3<f32>(-GAMUT_EPS)) && all(v <= vec3<f32>(1.0 + GAMUT_EPS));
}

fn fits_output_transform(l: f32, c: f32, dir: vec2<f32>, q: Params_output_transform, target_space: bool) -> bool {
    let p = oklab_to_prophoto(l, c * dir.x, c * dir.y);
    if (!target_space) { return inside_output_transform(p); }
    return inside_output_transform(vec3<f32>(dot(q.r0.xyz, p), dot(q.r1.xyz, p), dot(q.r2.xyz, p)));
}

fn bisect_output_transform(l: f32, dir: vec2<f32>, lo_in: f32, hi_in: f32, q: Params_output_transform, target_space: bool) -> f32 {
    var lo = lo_in;
    var hi = hi_in;
    for (var i = 0u; i < GAMUT_STEPS; i = i + 1u) {
        let mid = 0.5 * (lo + hi);
        if (fits_output_transform(l, mid, dir, q, target_space)) { lo = mid; } else { hi = mid; }
    }
    return lo;
}

fn gamut_map_output_transform(p: vec3<f32>, q: Params_output_transform) -> vec3<f32> {
    if (q.identity == 1u) { return p; }
    if (q.intent == 0u) {
        let lin = vec3<f32>(dot(q.r0.xyz, p), dot(q.r1.xyz, p), dot(q.r2.xyz, p));
        if (inside_output_transform(lin)) { return p; }
    }
    let lab = prophoto_to_oklab(p.x, p.y, p.z);
    let l = lab.x;
    if (l <= 0.0) { return vec3<f32>(0.0); }
    if (l >= 1.0) { return vec3<f32>(1.0); }
    let c = length(lab.yz);
    if (c <= 0.0) { return p; }
    let dir = lab.yz / c;
    var target_c: f32;
    if (q.intent == 0u) {
        target_c = bisect_output_transform(l, dir, 0.0, c, q, true);
    } else {
        let reach = c / GAMUT_KNEE;
        if (fits_output_transform(l, reach, dir, q, true)) { return p; }
        let bound = bisect_output_transform(l, dir, 0.0, reach, q, true);
        let src = max(bisect_output_transform(l, dir, bound, max(GAMUT_C_MAX, bound), q, false), bound);
        let t = GAMUT_KNEE * bound;
        let eps = 1.1920929e-7;
        if (src - t <= eps || bound - t <= eps) {
            target_c = min(c, bound);
        } else {
            let k = (src - t) / (bound - t);
            let xx = (c - t) / (src - t);
            target_c = min(t + (bound - t) * k * xx / (1.0 + (k - 1.0) * xx), bound);
        }
    }
    if (target_c >= c) { return p; }
    let s = target_c / c;
    return oklab_to_prophoto(l, lab.y * s, lab.z * s);
}

fn apply_output_transform(p_in: vec3<f32>, x: u32, y: u32, q: Params_output_transform, base: u32) -> vec3<f32> {
    let p = gamut_map_output_transform(p_in, q);
    // ⚠ Primaries first, in linear light, then the transfer. Encoding before converting would
    // apply a matrix to gamma-encoded values, which is meaningless and does not fail loudly.
    let lin = vec3<f32>(
        dot(q.r0.xyz, p),
        dot(q.r1.xyz, p),
        dot(q.r2.xyz, p),
    );
    return vec3<f32>(
        encode_channel_output_transform(lin.x, q),
        encode_channel_output_transform(lin.y, q),
        encode_channel_output_transform(lin.z, q));
}
"#;

/// The working space onto an output space, on the GPU.
pub struct GpuOutputTransformStage {
    /// Row-major, extracted from the CPU's own transform.
    rows: [[f32; 3]; 3],
    mode: u32,
    space: OutputSpace,
    intent: RenderingIntent,
    cpu: pipeline::OutputTransform,
}

impl GpuOutputTransformStage {
    pub fn new(space: OutputSpace, intent: RenderingIntent, _ctx: &Arc<GpuContext>) -> Self {
        // ⚠⚠ **The matrix is EXTRACTED from `OutputSpace::from_working`, never copied.** It is a
        // linear map, so its columns are just the images of the basis vectors — which means there
        // is no second copy of any matrix to go stale, ProPhoto's identity included. This repo has
        // already shipped a row-swapped P3 matrix that passed every test using a neutral fixture.
        let col = |e: [f32; 3]| space.from_working(e);
        let (x, y, z) = (
            col([1.0, 0.0, 0.0]),
            col([0.0, 1.0, 0.0]),
            col([0.0, 0.0, 1.0]),
        );
        let rows = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
        let mode = match space {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => MODE_SRGB,
            OutputSpace::AdobeRgb => MODE_ADOBE,
            OutputSpace::ProPhoto => MODE_PROPHOTO,
        };
        Self {
            rows,
            mode,
            space,
            intent,
            cpu: pipeline::OutputTransform::new(space, intent),
        }
    }
}

impl GpuStage for GpuOutputTransformStage {
    fn name(&self) -> &'static str {
        "output_transform"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: **28.63 ms**, the tail's baseline —
    /// it is on every photograph, so it is the whole of a neutral frame's tail.
    fn cpu_cost(&self) -> f32 {
        28.6
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
        let mut params = [0u8; 80];
        params[0..4].copy_from_slice(&self.mode.to_le_bytes());
        let intent: u32 = match self.intent {
            RenderingIntent::RelativeColorimetric => 0,
            RenderingIntent::Perceptual => 1,
        };
        params[4..8].copy_from_slice(&intent.to_le_bytes());
        let identity = u32::from(self.space == OutputSpace::ProPhoto);
        params[8..12].copy_from_slice(&identity.to_le_bytes());
        for (r, row) in self.rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let at = 16 + r * 16 + c * 4;
                params[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        params[64..68].copy_from_slice(&color::ADOBE_RGB_GAMMA.to_le_bytes());
        params[68..72].copy_from_slice(&color::PROPHOTO_TOE_END.to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "output_transform",
            // ⚠ The grid and the knee are substituted from `color::gamut`, never re-typed.
            wgsl: format!(
                "const GAMUT_KNEE: f32 = {:?};\nconst GAMUT_C_MAX: f32 = {:?};\nconst GAMUT_STEPS: u32 = {}u;\nconst GAMUT_EPS: f32 = {:?};\n{WGSL}",
                color::gamut::KNEE,
                color::gamut::C_MAX,
                color::gamut::BISECTION_STEPS,
                color::gamut::IN_GAMUT_EPS,
            ),
            params: params.to_vec(),
            tables: Vec::new(),
            // ⚠ The sRGB prelude, so `encode_channel_output_transform`'s mode-0 branch calls a
            // function this crate defines exactly once; Oklab for the gamut map.
            preludes: crate::per_pixel::Preludes {
                srgb: true,
                oklab: true,
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

    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(
        ctx: &Arc<GpuContext>,
        space: OutputSpace,
        intent: RenderingIntent,
        img: ImageBuf,
    ) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuOutputTransformStage::new(space, intent, ctx))],
        )
        .apply(img)
    }

    const SPACES: [OutputSpace; 4] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
        OutputSpace::ProPhoto,
    ];

    /// ⚠ All four spaces: three transfer curves and four matrices are seven chances to wire the
    /// wrong one, and ProPhoto's matrix must be exactly the identity.
    ///
    /// ⚠⚠ The fixture is **coloured**, not a grey ramp. A neutral survives every one of the four
    /// matrices nearly unchanged, so a test using one would pass with any two of them swapped —
    /// the blindness that let a row-swapped P3 matrix through in the display-profile work.
    #[test]
    fn every_output_space_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for space in SPACES {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::OutputTransform::new(space, RenderingIntent::default())
                .apply(img.clone());
            let gpu = chained(&ctx, space, RenderingIntent::default(), img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.1,
                "{space:?}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠ And the spaces must not all agree, or the loop above would pass with one matrix wired
    /// four times.
    #[test]
    fn the_four_spaces_differ_from_one_another() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let srgb = chained(
            &ctx,
            OutputSpace::Srgb,
            RenderingIntent::default(),
            img.clone(),
        );
        for other in [
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
            OutputSpace::ProPhoto,
        ] {
            let d = crate::compare(
                &srgb,
                &chained(&ctx, other, RenderingIntent::default(), img.clone()),
            );
            assert!(
                d.worst_levels > 1.0,
                "{other:?} renders the same as sRGB; moved {} levels",
                d.worst_levels
            );
        }
    }

    /// ⚠⚠ **WGSL's `pow` returns NaN for a negative base, and out-of-gamut colour reaches this
    /// stage with negative components BY DESIGN.** The CPU's `encode` clamps to [0,1] first, so
    /// the shader must too. ⚠ An in-gamut fixture cannot see this, because after the matrix every
    /// component is already positive — which is exactly how it would ship.
    #[test]
    fn an_out_of_gamut_colour_does_not_become_nan() {
        let Some(ctx) = ctx() else { return };
        // A saturated ProPhoto green: outside sRGB, so the matrix gives it a negative red.
        let px = [0.05, 0.9, 0.08];
        // ⚠ The fixture check first — which colours fall outside sRGB is not obvious by eye, and
        // an in-gamut one would make this test prove nothing.
        let lin = OutputSpace::Srgb.from_working(px);
        assert!(
            lin.iter().any(|c| *c < 0.0),
            "fixture must be out of gamut, got {lin:?}"
        );
        let img = ImageBuf::from_vec(1, 1, px.to_vec());
        let gpu = chained(
            &ctx,
            OutputSpace::Srgb,
            RenderingIntent::default(),
            img.clone(),
        );
        assert!(
            gpu.data().iter().all(|v| v.is_finite()),
            "out-of-gamut colour must stay finite, got {:?}",
            gpu.data()
        );
        // ⚠ Finite alone would pass against a shader that zeroed everything.
        let cpu = pipeline::OutputTransform::new(OutputSpace::Srgb, RenderingIntent::default())
            .apply(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
    }

    /// ⚠⚠ **THE TEST ABOVE COULD NOT SEE A MISSING CLAMP, AND FINDING THAT OUT IS WORTH MORE THAN
    /// THE STAGE.** Deleting `clamp(v, 0, 1)` from `encode_channel` left all five tests green.
    /// Two blindnesses compounded:
    ///
    /// 1. **sRGB's toe branch catches a negative before `pow` ever sees it** — `c <= 0.0031308`
    ///    is true of every negative — so the NaN the clamp is documented to prevent never
    ///    happens *in the one space that test used*. It happens in Adobe RGB, which reaches
    ///    `pow` directly.
    /// 2. The output is then **negative rather than 0**, and `compare` converts to **8-bit
    ///    levels**, where a negative sample and 0 quantize identically. Invisible.
    ///
    /// ⚠ So the guard is the property this stage actually has, asserted on raw floats across
    /// **every** space: its output is display-referred, and `encode` clamps, so nothing may leave
    /// `[0, 1]`. That catches the negative leak *and* the NaN, in every mode, rather than naming
    /// one trap and probing where it is inert.
    #[test]
    fn no_space_can_emit_a_sample_outside_zero_to_one() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Out of gamut in every direction, plus scene-referred values above white.
        let px = vec![
            0.05, 0.9, 0.08, 0.95, 0.03, 0.9, -0.5, 0.02, 0.03, 2.4, 2.1, 1.8,
        ];
        for space in SPACES {
            let gpu = chained(
                &ctx,
                space,
                RenderingIntent::default(),
                ImageBuf::from_vec(4, 1, px.clone()),
            );
            for (i, v) in gpu.data().iter().enumerate() {
                assert!(
                    v.is_finite() && (0.0..=1.0).contains(v),
                    "{space:?} emitted {v} at sample {i}, outside [0, 1]"
                );
            }
        }
    }

    /// ⚠ ProPhoto is the identity and the only lossless option — it *is* the working space, so it
    /// converts nothing. A matrix extracted wrongly would show up here first.
    #[test]
    fn prophoto_applies_no_matrix() {
        let Some(ctx) = ctx() else { return };
        let s = GpuOutputTransformStage::new(
            OutputSpace::ProPhoto,
            RenderingIntent::default(),
            crate::shared().expect("checked above"),
        );
        assert_eq!(s.rows, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let _ = ctx;
    }

    #[test]
    fn it_asks_for_no_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        let chain = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuOutputTransformStage::new(
                OutputSpace::Srgb,
                RenderingIntent::default(),
                &ctx,
            ))],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }

    /// ⚠⚠ Every space × intent on colours OUTSIDE every target — an in-gamut fixture would pass
    /// with the whole gamut map missing from the shader. The fixture is asserted to move first.
    #[test]
    fn the_gamut_map_matches_the_cpu_for_every_space_and_intent() {
        let Some(ctx) = ctx() else { return };
        let mut px = Vec::new();
        for r in [0.0f32, 0.1, 0.5, 0.9] {
            for g in [0.0f32, 0.3, 0.8] {
                for b in [0.0f32, 0.2, 0.7, 1.0] {
                    px.extend_from_slice(&[r, g, b]);
                }
            }
        }
        let n = px.len() / 3;
        let img = ImageBuf::from_vec(n, 1, px);
        let perceptual =
            pipeline::OutputTransform::new(OutputSpace::Srgb, RenderingIntent::Perceptual)
                .apply(img.clone());
        let relative = pipeline::OutputTransform::new(
            OutputSpace::Srgb,
            RenderingIntent::RelativeColorimetric,
        )
        .apply(img.clone());
        assert!(
            crate::compare(&perceptual, &relative).worst_levels > 5.0,
            "the fixture must tell the intents apart"
        );
        for space in SPACES {
            for intent in RenderingIntent::ALL {
                let cpu = pipeline::OutputTransform::new(space, intent).apply(img.clone());
                let gpu = chained(&ctx, space, intent, img.clone());
                let d = crate::compare(&cpu, &gpu);
                assert!(
                    d.worst_levels < 0.5,
                    "{space:?} {intent:?}: worst {} levels over {} of {} samples",
                    d.worst_levels,
                    d.differing,
                    d.total
                );
            }
        }
    }
}

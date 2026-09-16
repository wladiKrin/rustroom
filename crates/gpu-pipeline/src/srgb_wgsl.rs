//! The sRGB transfer function as WGSL, written once.
//!
//! ⚠⚠ **Extracted rather than copied a second time.** `output_transform` carried this inline, and
//! a luminance-range mask needs the identical curve — `LuminanceRange::coverage` selects on
//! `linear_to_srgb(luminance(rgb)) * 100`. A second spelling of a transfer function is invisible
//! per stage and wrong everywhere, which is the failure `color`'s own doc records against a
//! Rec.709 triple that went wrong in six places on one day.
//!
//! ⚠⚠ **The clamp is load-bearing twice over.** WGSL's `pow` is **indeterminate** for a negative
//! base — ⚠ *not* NaN, which is what this crate assumed until it was measured; see
//! `a_negative_sample_encodes_to_exactly_zero` below — and a wide working space produces negatives
//! routinely, because that is the point of it. And for a range
//! mask the same clamp is the **index** clamp: `LuminanceRange::coverage`'s own doc records that
//! the selector is clamped while the pixel keeps its headroom, so a specular highlight selects at
//! the top of the scale and still carries every bit of its latitude into the adjustment. Dropping
//! it would be the `ProfileHueSatMap` trap, which this project has now met three times.
//!
//! ⚠ The constants are the sRGB standard's own and are literals in [`color::linear_to_srgb`] too,
//! so there is nothing named to substitute *from*. The guard is therefore **behavioural** — the
//! test below runs the shader and compares against `color`'s function — which is the same posture
//! `oklab_wgsl` takes, and for the reason its doc gives: parsing constants out of shader source
//! compares digits to digits and breaks on any reformatting.

/// `linear_to_srgb`, matching [`color::linear_to_srgb`] expression for expression.
pub(crate) const SRGB_WGSL: &str = r#"
fn linear_to_srgb(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if (c <= 0.0031308) { return c * 12.92; }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GpuContext, GpuImage, GpuStage};
    use core_image::ImageBuf;
    use pipeline::PipelineStage;
    use std::sync::Arc;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A stage that does nothing but run [`SRGB_WGSL`] over every channel.
    ///
    /// ⚠ It carries a `noop` only because `cpu()` must return something — a `GpuStage` cannot be
    /// written without a twin, which is the whole point of that method. The `chain.rs` test
    /// module's `NeedsScratch` does the same.
    struct SrgbProbe {
        noop: pipeline::ExposureStage,
    }

    impl SrgbProbe {
        fn new() -> Self {
            Self {
                noop: pipeline::ExposureStage::new(0.0),
            }
        }
    }

    impl GpuStage for SrgbProbe {
        fn name(&self) -> &'static str {
            "srgb_probe"
        }

        fn cpu_cost(&self) -> f32 {
            99.0
        }

        fn encode(
            &self,
            ctx: &GpuContext,
            pass: &mut wgpu::ComputePass,
            img: &GpuImage,
            _scratch: &[wgpu::Buffer],
        ) {
            let pixels = img.pixels();
            let (row_groups, _) = crate::dispatch::Dispatch::grid(pixels);
            let mut params = [0u8; 16];
            params[0..4].copy_from_slice(&pixels.to_le_bytes());
            params[4..8].copy_from_slice(&row_groups.to_le_bytes());

            let body = r#"
struct Params { pixels: u32, row_groups: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    data[p] = linear_to_srgb(data[p]);
    data[p + 1u] = linear_to_srgb(data[p + 1u]);
    data[p + 2u] = linear_to_srgb(data[p + 2u]);
}
"#;
            crate::dispatch::Dispatch {
                label: "srgb_probe",
                shader: format!("{SRGB_WGSL}{body}"),
                params: params.to_vec(),
                storages: &[],
            }
            .encode(ctx, pass, img);
        }

        fn cpu(&self) -> &dyn PipelineStage {
            &self.noop
        }
    }

    /// ⚠⚠ **Behavioural, not textual.** This is the only thing standing between the extraction and
    /// a shader that silently spells the curve differently from `color::linear_to_srgb`.
    ///
    /// ⚠ The probes are chosen to cross every branch: below the knee, exactly on it, above it, and
    /// **outside `[0, 1]` in both directions** — the last two being the only ones that can see the
    /// clamp, and a fixture confined to in-gamut values could not.
    #[test]
    fn the_wgsl_encode_agrees_with_color_linear_to_srgb() {
        let Some(ctx) = ctx() else { return };
        let probes: Vec<f32> = vec![
            -0.5, // negative: `pow` is indeterminate here without the clamp
            0.0, 0.001, 0.0031308, // the knee itself
            0.01, 0.18, 0.5, 0.9, 1.0, 2.0, // above white: the index clamp's own case
        ];
        let img = ImageBuf::from_vec(
            probes.len(),
            1,
            probes.iter().flat_map(|p| [*p, *p, *p]).collect(),
        );
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(SrgbProbe::new())]).apply(img);

        for (i, p) in probes.iter().enumerate() {
            let want = color::linear_to_srgb(*p);
            let g = got.data()[i * 3];
            assert!(
                (g - want).abs() < 1e-5,
                "linear_to_srgb({p}): gpu {g} vs cpu {want}"
            );
        }
    }

    /// ⚠ A guard on the *reason* the clamp exists, stated as its own property so that removing it
    /// fails with a diagnosis rather than as one sample among ten.
    ///
    /// ⚠⚠ **MEASURED 2026-09-15, and it corrects an assumption this crate already carries.**
    /// `output_transform`'s own comment says *"WGSL's `pow` returns NaN for a negative base"*.
    /// Sabotaged by deleting the clamp, this adapter (Apple M2, Metal) returned **−6.46**, not
    /// NaN — a finite, plausible, wrong number. The WGSL spec says only that the result is
    /// **indeterminate**, which is the accurate statement and is strictly worse than NaN for us:
    /// a NaN propagates visibly to black, where a finite wrong value is just a wrong photograph.
    ///
    /// ⚠ So an `is_finite` assertion is **not** the guard, and a test that only checked for NaN
    /// would have passed against the sabotage. The guard is the exact value.
    #[test]
    fn a_negative_sample_encodes_to_exactly_zero() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(2, 1, vec![-0.5, -1.0, -2.0, -0.25, -0.75, -3.0]);
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(SrgbProbe::new())]).apply(img);
        for (i, v) in got.data().iter().enumerate() {
            assert_eq!(
                *v, 0.0,
                "sample {i} came back {v}, so the clamp is not doing its job"
            );
        }
    }
}

//! The post-crop vignette as a compute shader: a position-dependent gain in linear light.
//!
//! ⚠⚠ **The only stage in this crate whose result depends on WHERE a pixel is**, which is why it
//! is the only one that needs the [`Region`]. Every other port is a pure function of the sample.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Vignette;
use pipeline::{PipelineStage, Region};
use std::sync::Arc;

/// ⚠⚠ **The smoothstep is written out rather than using WGSL's built-in `smoothstep`.** The
/// formula is the same, but writing it matches the CPU's **expression shape** — so any divergence
/// is the known FMA-contraction class (M8.3m) rather than a third thing to explain.
///
/// ⚠ `pow` is safe here where it was not in `oklab_wgsl`: the base is `abs(du)` and never
/// negative, so the `sign(x) * pow(abs(x), e)` guard M8.3b needed does not apply.
///
/// ⚠⚠ **`max(1.0 - c, 0.0)` in the lift is load-bearing and `compare` cannot see it.** It leaves a
/// scene-referred sample already above 1.0 alone rather than dragging it down toward white; drop
/// it and a highlight at 1.4 is pulled *down*, which quantises to 255 either way. M8.3e's
/// output-transform clamp in mirror form — at the top rather than at zero, and the same blindness.
fn shader() -> &'static str {
    r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    width: u32,
    x0: u32,
    y0: u32,
    elliptical: u32,
    pad0: u32,
    pad1: u32,
    hw: f32,
    hh: f32,
    wu: f32,
    wv: f32,
    expo: f32,
    e0: f32,
    e1: f32,
    k: f32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    // ⚠ x and y are recovered from the LINEAR index: the dispatch grid is a 2-D split of a flat
    // pixel range, never a split of the image's own rows, so `id.y` is not the image row.
    let x = pixel % params.width;
    let y = pixel / params.width;

    let u = (f32(x + params.x0) + 0.5) / params.hw - 1.0;
    let v = (f32(y + params.y0) + 0.5) / params.hh - 1.0;
    let du = u * params.wu;
    let dv = v * params.wv;
    var d: f32;
    if (params.elliptical != 0u) {
        d = sqrt(du * du + dv * dv);
    } else {
        d = pow(pow(abs(du), params.expo) + pow(abs(dv), params.expo), 1.0 / params.expo);
    }
    let t = clamp((d - params.e0) / (params.e1 - params.e0), 0.0, 1.0);
    let f = params.k * (t * t * (3.0 - 2.0 * t));

    let p = pixel * 3u;
    if (f <= 0.0) {
        // Squared about its own endpoint: exactly 0 at f = -1, and f = -0.5 is -2 EV.
        let gain = (1.0 + f) * (1.0 + f);
        data[p] = data[p] * gain;
        data[p + 1u] = data[p + 1u] * gain;
        data[p + 2u] = data[p + 2u] * gain;
    } else {
        let s = 1.0 - (1.0 - f) * (1.0 - f);
        data[p] = data[p] + max(1.0 - data[p], 0.0) * s;
        data[p + 1u] = data[p + 1u] + max(1.0 - data[p + 1u], 0.0) * s;
        data[p + 2u] = data[p + 2u] + max(1.0 - data[p + 2u], 0.0) * s;
    }
}
"#
}

/// The post-crop vignette, on the GPU.
///
/// ⚠⚠ **Holds the edit and the region rather than derived parameters**, because the frame the
/// falloff is measured against is only known at `encode` time when there is no region — it is then
/// the image's own dimensions. Deriving in the constructor would need a size nobody has yet.
pub struct GpuVignetteStage {
    params: Vignette,
    region: Option<Region>,
    cpu: pipeline::VignetteStage,
}

impl GpuVignetteStage {
    pub fn new(params: &Vignette, region: Option<Region>, _ctx: &Arc<GpuContext>) -> Self {
        Self {
            params: *params,
            region,
            cpu: pipeline::VignetteStage::new_in_region(params, region),
        }
    }
}

impl GpuStage for GpuVignetteStage {
    fn name(&self) -> &'static str {
        "vignette"
    }

    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`, three runs.
    ///
    /// ⚠⚠ **The two shapes cost wildly different amounts, and one number cannot describe both:**
    /// the elliptical branch reads **8.57-9.05 ms** and the superellipse **40.44-45.51 ms**, a
    /// factor of five. The `sqrt` fast path against two `powf` plus a reciprocal exponent, per
    /// pixel — which is exactly why `Shape` takes that fast path at all.
    ///
    /// ⚠ Ships at the bottom of the **elliptical** band, so a negative-roundness vignette is
    /// under-reported by ~4.7x. That is the conservative direction and the gate's own rule: a
    /// lower number makes a run less likely to chain, and the gate must fail toward the CPU,
    /// where a wrong number costs milliseconds and never pixels. Branching here would put the
    /// `roundness < 0.0` rule in a second place, and `Shape::new` is where it belongs.
    ///
    /// ⚠⚠ Well under `TRANSFER_COST`, so **on its own this will never chain, and that is
    /// correct.** Its value is entirely in joining a run that already pays — the same reason
    /// `contrast` was worth porting at 1.2 ms.
    fn cpu_cost(&self) -> f32 {
        8.5
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

        // ⚠⚠ The frame the falloff is measured against, and where this buffer sits in it. Without
        // a region the buffer **is** the frame — the whole-image case, exactly as the CPU stage
        // resolves it.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (img.width(), img.height(), 0, 0),
        };

        // ⚠⚠ Through `pipeline::vignette_params`, never re-derived here. The two paths would
        // disagree **invisibly** the first time `R_MID`, `FEATHER` or the amount curve moved,
        // because both answers are plausible photographs.
        let vp = pipeline::vignette_params(&self.params, fw, fh);
        let (hw, hh) = (fw as f32 * 0.5, fh as f32 * 0.5);

        let mut params = [0u8; 64];
        params[0..4].copy_from_slice(&pixels.to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[8..12].copy_from_slice(&(img.width() as u32).to_le_bytes());
        params[12..16].copy_from_slice(&(x0 as u32).to_le_bytes());
        params[16..20].copy_from_slice(&(y0 as u32).to_le_bytes());
        params[20..24].copy_from_slice(&u32::from(vp.elliptical).to_le_bytes());
        params[32..36].copy_from_slice(&hw.to_le_bytes());
        params[36..40].copy_from_slice(&hh.to_le_bytes());
        params[40..44].copy_from_slice(&vp.wu.to_le_bytes());
        params[44..48].copy_from_slice(&vp.wv.to_le_bytes());
        params[48..52].copy_from_slice(&vp.expo.to_le_bytes());
        params[52..56].copy_from_slice(&vp.e0.to_le_bytes());
        params[56..60].copy_from_slice(&vp.e1.to_le_bytes());
        params[60..64].copy_from_slice(&vp.k.to_le_bytes());

        crate::dispatch::Dispatch {
            label: "vignette",
            shader: shader().to_string(),
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

    /// ⚠⚠ **Non-uniform, and that is not a detail.** A vignette is the one stage here whose result
    /// depends on WHERE a pixel is, so a flat fixture would render identically under a gain
    /// applied at the wrong coordinate. The ramp varies in both axes.
    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut px = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let t = (x as f32 / w as f32) * 0.8 + (y as f32 / h as f32) * 0.4;
                px.extend_from_slice(&[t, t * 0.85, t * 1.1]);
            }
        }
        ImageBuf::from_vec(w, h, px)
    }

    fn vignette(amount: f32, roundness: f32) -> Vignette {
        Vignette {
            amount,
            roundness,
            ..Vignette::default()
        }
    }

    fn chained(
        ctx: &Arc<GpuContext>,
        v: &Vignette,
        region: Option<Region>,
        img: ImageBuf,
    ) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuVignetteStage::new(v, region, ctx))],
        )
        .apply(img)
    }

    /// ⚠⚠ **The fixture check, and without it every equivalence assertion below is vacuous.** The
    /// GPU is compared against the CPU, so a shader that did nothing would agree perfectly with a
    /// CPU stage that also did nothing. This asserts the CPU stage really moves these pixels.
    #[test]
    fn the_stage_really_vignettes_rather_than_copying() {
        let img = ramp(40, 30);
        let out = pipeline::VignetteStage::new(&vignette(-70.0, 0.0)).apply(img.clone());
        let moved = img
            .data()
            .iter()
            .zip(out.data())
            .filter(|(a, b)| (*a - *b).abs() > 1e-4)
            .count();
        assert!(
            moved > img.data().len() / 2,
            "fixture check: the vignette must move most of this fixture, moved {moved} of {}",
            img.data().len()
        );
    }

    /// ⚠⚠ **Every shape, INCLUDING a negative roundness — or the `pow` branch is never reached.**
    /// An elliptical-only fixture takes the `sqrt` fast path, so it says nothing about the branch
    /// most likely to diverge: two `pow` calls and a reciprocal exponent.
    ///
    /// ⚠ Within one 8-bit level rather than bit-identical: the smoothstep is a divide plus
    /// multiply-adds, which the GPU fuses (M8.3m's contraction class).
    #[test]
    fn every_shape_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (amount, roundness) in [
            (-70.0f32, 0.0f32), // elliptical, darkening
            (70.0, 0.0),        // elliptical, brightening
            (-70.0, 100.0),     // stretched toward a circle
            (-70.0, -100.0),    // superellipse: the `pow` branch
            (60.0, -60.0),      // superellipse, brightening
        ] {
            let v = vignette(amount, roundness);
            // ⚠ Deliberately not square: a wrong axis in the stretch is invisible on a square
            // frame, where `wu` and `wv` are interchangeable.
            let img = ramp(48, 30);
            let cpu = pipeline::VignetteStage::new(&v).apply(img.clone());
            let gpu = chained(&ctx, &v, None, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "vignette (amount {amount}, roundness {roundness}) diverged: {d:?}"
            );
        }
    }

    /// ⚠⚠ **The lift's `max(1.0 - c, 0.0)`, and `compare` is STRUCTURALLY BLIND to it.** That max
    /// is what leaves a scene-referred sample already above 1.0 alone rather than dragging it down
    /// toward white — and a level comparison quantises everything above 1.0 to 255, so the bug is
    /// invisible there. M8.3e's output-transform clamp in mirror form, at the top rather than at
    /// zero, and the same blindness. This reads **raw floats**.
    #[test]
    fn a_scene_referred_highlight_is_not_dragged_down() {
        let Some(ctx) = ctx() else { return };
        // A bright corner, where a positive vignette lifts hardest.
        let img = ImageBuf::from_vec(2, 2, vec![1.4; 12]);
        let out = chained(&ctx, &vignette(80.0, 0.0), None, img);
        for (i, c) in out.data().iter().enumerate() {
            assert!(
                *c >= 1.4 - 1e-4,
                "sample {i} was dragged down from 1.4 to {c} — a level comparison cannot see this"
            );
        }
    }

    /// ⚠⚠ **A region must be shaded exactly as that part of the whole would have been**, which is
    /// the whole reason this stage takes a [`Region`] at all. Without the origin the falloff is
    /// measured from the region's own centre and the vignette slides across the frame — a
    /// plausible photograph, not an error.
    #[test]
    fn a_region_is_shaded_as_that_part_of_the_whole() {
        let Some(ctx) = ctx() else { return };
        let (fw, fh) = (64usize, 48usize);
        let (x0, y0, rw, rh) = (20usize, 12usize, 24usize, 18usize);
        let v = vignette(-70.0, -40.0);

        // The whole frame on the CPU, then the same rectangle cut out of it.
        let whole = pipeline::VignetteStage::new(&v).apply(ramp(fw, fh));
        let mut want = ImageBuf::new(rw, rh);
        for y in 0..rh {
            for x in 0..rw {
                want.set_pixel(x, y, whole.pixel(x + x0, y + y0));
            }
        }

        // The same rectangle rendered on the GPU as a region of that frame.
        let src = ramp(fw, fh);
        let mut part = ImageBuf::new(rw, rh);
        for y in 0..rh {
            for x in 0..rw {
                part.set_pixel(x, y, src.pixel(x + x0, y + y0));
            }
        }
        let region = Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let got = chained(&ctx, &v, Some(region), part);
        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "a region must be shaded as that part of the whole: {d:?}"
        );
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what the order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        let v = Vignette::default();
        assert_eq!(
            GpuVignetteStage::new(&v, None, &ctx).name(),
            pipeline::VignetteStage::new(&v).name()
        );
    }
}

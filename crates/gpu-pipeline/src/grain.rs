//! Grain as a compute shader (M10.11b): `pipeline::GrainStage`, expression for expression.
//!
//! ⚠⚠ **Integer hashing, so the lattice is the CPU's exactly.** The hash's multipliers, the seeds and
//! the second octave's offset are **substituted** from `pipeline` rather than transcribed — the
//! `luma_wgsl` rule — so the two cannot drift; only the interpolation and `exp2` are float, the
//! FMA-contraction class every per-pixel twin already lives with.
//!
//! ⚠ Region-aware like the vignette: the lattice is addressed in the frame's pixels, `x0`/`y0` placing
//! the buffer in it.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Grain;
use pipeline::{PipelineStage, Region, RenderScale};
use std::sync::Arc;

fn wgsl() -> String {
    let h = pipeline::GRAIN_HASH;
    format!(
        r#"
struct Params_grain {{
    x0: u32,
    y0: u32,
    pad0: u32,
    pad1: u32,
    cell: f32,
    amp: f32,
    rough: f32,
    pad2: f32,
}};
fn hash_grain(x: i32, y: i32, seed: u32) -> u32 {{
    var h = (bitcast<u32>(x) * {h0}u) ^ (bitcast<u32>(y) * {h1}u) ^ seed;
    h = h ^ (h >> 16u);
    h = h * {h2}u;
    h = h ^ (h >> 15u);
    h = h * {h3}u;
    h = h ^ (h >> 16u);
    return h;
}}
fn lattice_grain(x: i32, y: i32, seed: u32) -> f32 {{
    return f32(hash_grain(x, y, seed) >> 8u) / 8388608.0 - 1.0;
}}
fn smooth_grain(t: f32) -> f32 {{
    return t * t * (3.0 - 2.0 * t);
}}
fn value_noise_grain(x: f32, y: f32, seed: u32) -> f32 {{
    let xf = floor(x);
    let yf = floor(y);
    let ix = i32(xf);
    let iy = i32(yf);
    let tx = smooth_grain(x - xf);
    let ty = smooth_grain(y - yf);
    let a = lattice_grain(ix, iy, seed);
    let b = lattice_grain(ix + 1, iy, seed);
    let c = lattice_grain(ix, iy + 1, seed);
    let d = lattice_grain(ix + 1, iy + 1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}}
fn octave_grain(fx: f32, fy: f32, cell: f32, seed: u32, ox: f32, oy: f32) -> f32 {{
    if (cell >= 1.0) {{
        return value_noise_grain(fx / cell + ox, fy / cell + oy, seed);
    }}
    return value_noise_grain(fx + ox, fy + oy, seed) * cell;
}}
fn apply_grain(p: vec3<f32>, x: u32, y: u32, q: Params_grain, base: u32) -> vec3<f32> {{
    let fx = f32(x + q.x0);
    let fy = f32(y + q.y0);
    let n1 = octave_grain(fx, fy, q.cell, {seed}u, 0.0, 0.0);
    var n = n1;
    if (q.rough > 0.0) {{
        let n2 = octave_grain(fx, fy, q.cell * 0.5, {seed2}u, {ox:?}, {oy:?});
        let r = q.rough;
        n = ((1.0 - r) * n1 + r * n2) / sqrt((1.0 - r) * (1.0 - r) + r * r);
    }}
    let l = linear_to_srgb(clamp(prophoto_luminance(p.x, p.y, p.z), 0.0, 1.0));
    let w = 4.0 * l * (1.0 - l);
    let gain = exp2(q.amp * n * w);
    return vec3<f32>(p.x * gain, p.y * gain, p.z * gain);
}}
"#,
        h0 = h[0],
        h1 = h[1],
        h2 = h[2],
        h3 = h[3],
        seed = pipeline::GRAIN_SEED,
        seed2 = pipeline::GRAIN_SEED2,
        ox = pipeline::GRAIN_OFFSET.0,
        oy = pipeline::GRAIN_OFFSET.1,
    )
}

/// The grain stage on the GPU.
pub struct GpuGrainStage {
    params: pipeline::GrainParams,
    region: Option<Region>,
    cpu: pipeline::GrainStage,
}

impl GpuGrainStage {
    pub fn new(
        g: &Grain,
        scale: RenderScale,
        region: Option<Region>,
        _ctx: &Arc<GpuContext>,
    ) -> Self {
        Self {
            params: pipeline::grain_params(g, scale),
            region,
            cpu: pipeline::GrainStage::new_in_region(g, scale, region),
        }
    }
}

impl GpuStage for GpuGrainStage {
    fn name(&self) -> &'static str {
        "grain"
    }

    fn cpu_cost(&self) -> f32 {
        12.0
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

    fn per_pixel(&self, w: usize, h: usize) -> Option<crate::per_pixel::Part<'_>> {
        if w == 0 || h == 0 || self.params.amp <= 0.0 {
            return None;
        }
        let (x0, y0) = self.region.map_or((0, 0), |r| (r.x0, r.y0));
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&(x0 as u32).to_le_bytes());
        params[4..8].copy_from_slice(&(y0 as u32).to_le_bytes());
        params[16..20].copy_from_slice(&self.params.cell.to_le_bytes());
        params[20..24].copy_from_slice(&self.params.amp.to_le_bytes());
        params[24..28].copy_from_slice(&self.params.rough.to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "grain",
            wgsl: wgsl(),
            params: params.to_vec(),
            tables: Vec::new(),
            preludes: crate::per_pixel::Preludes {
                srgb: true,
                luma: true,
                ..Default::default()
            },
            needs_position: true,
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

    fn chained(
        ctx: &Arc<GpuContext>,
        g: &Grain,
        scale: RenderScale,
        region: Option<Region>,
        img: ImageBuf,
    ) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuGrainStage::new(g, scale, region, ctx))],
        )
        .apply(img)
    }

    #[test]
    fn every_setting_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (amount, size, roughness, scale) in [
            (50.0f32, 25.0f32, 50.0f32, 1.0f32),
            (100.0, 0.0, 0.0, 1.0),
            (80.0, 100.0, 100.0, 1.0),
            (60.0, 40.0, 70.0, 0.3), // the second octave below a pixel
            (90.0, 0.0, 50.0, 0.25), // both octaves below a pixel
        ] {
            let g = Grain {
                amount,
                size,
                roughness,
            };
            let s = RenderScale::new(scale);
            let img = ramp(48, 30);
            let cpu = pipeline::GrainStage::new_in_region(&g, s, None).apply(img.clone());
            let gpu = chained(&ctx, &g, s, None, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "grain {amount}/{size}/{roughness} at {scale} diverged: {d:?}"
            );
        }
    }

    #[test]
    fn a_region_matches_that_part_of_the_cpu_whole() {
        let Some(ctx) = ctx() else { return };
        let g = Grain {
            amount: 80.0,
            size: 30.0,
            roughness: 60.0,
        };
        let whole =
            pipeline::GrainStage::new_in_region(&g, RenderScale::FULL, None).apply(ramp(64, 48));
        let region = Region {
            full_w: 64,
            full_h: 48,
            x0: 20,
            y0: 12,
        };
        let part = chained(
            &ctx,
            &g,
            RenderScale::FULL,
            Some(region),
            ramp(64, 48).crop(20, 12, 24, 18),
        );
        let d = crate::compare(&whole.crop(20, 12, 24, 18), &part);
        assert!(d.worst_levels < 1.0, "{d:?}");
    }
}

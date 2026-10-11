//! Film grain (M10.11b) — Lightroom's Effects → Grain.
//!
//! ⚠⚠ **IN STOPS, BRIGHTNESS ONLY, STRONGEST IN THE MIDTONES.** `out = p · 2^(A · n · w(L))`: a tiny
//! exposure change per point, so it shows evenly across tones in the way the eye judges them; the same
//! gain on all three channels, so it adds no colour noise; and `w(L) = 4L(1 − L)` on the encoded
//! luminance, so it fades in deep shadows and highlights, as film grain does.
//!
//! ⚠⚠ **SIZED IN THE FRAME'S OWN PIXELS.** The noise lattice is addressed at `frame pixel / cell`, the
//! cell being the grain size in full-resolution pixels times the render scale — so pixel `x` of a
//! quarter-scale preview reads exactly what pixel `4x` of the export does. Below one pixel per cell (a fit
//! preview) the lattice is sampled per pixel and the amplitude scaled by the cell — the spread an average
//! of that many cells has — rather than drawing harsh single-pixel noise for fine grain.
//!
//! ⚠ **Integer hashing only**, so the GPU twin computes the same lattice; one fixed seed, so the preview,
//! the export and the GPU agree on every photo. The last scene-linear stage: after sharpening, which must
//! not sharpen it.

use crate::{PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::Grain;

/// Amount 100's strength, in stops at the peak of the noise. ⚠ The author's call, tuned on crops.
pub const MAX_EV: f32 = 0.5;

/// The lattice's seed, and the second octave's.
pub const SEED: u32 = 0x9E37_79B9;
pub const SEED2: u32 = 0x5BD1_E995;

/// Where the second octave's lattice sits against the first's, in its own cells — so the two never
/// share lattice points.
pub const OFFSET: (f32, f32) = (17.3, 41.9);

/// What one render's grain needs, from the setting and the render scale. ⚠ The GPU twin reads this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainParams {
    /// The cell, in this buffer's pixels.
    pub cell: f32,
    /// Stops at the noise's peak, before the tone weight.
    pub amp: f32,
    /// The second octave's share, 0..1.
    pub rough: f32,
}

pub fn grain_params(g: &Grain, scale: RenderScale) -> GrainParams {
    let cell_full = 1.0 + 4.0 * (g.size / 100.0).clamp(0.0, 1.0);
    GrainParams {
        cell: cell_full * scale.get(),
        amp: (g.amount / 100.0).clamp(0.0, 1.0) * MAX_EV,
        rough: (g.roughness / 100.0).clamp(0.0, 1.0),
    }
}

/// An integer hash of a lattice point — the same `u32` arithmetic in the WGSL twin.
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

/// A lattice point's value, uniform in [−1, 1).
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / 8_388_608.0 - 1.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise at lattice coordinates `(x, y)`: the four corners, smoothstep-interpolated.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (xf as i32, yf as i32);
    let (tx, ty) = (smooth(x - xf), smooth(y - yf));
    let a = lattice(ix, iy, seed);
    let b = lattice(ix + 1, iy, seed);
    let c = lattice(ix, iy + 1, seed);
    let d = lattice(ix + 1, iy + 1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

/// One octave at a cell of `cell` buffer pixels, at frame pixel `(fx, fy)`.
fn octave(fx: f32, fy: f32, cell: f32, seed: u32, off: (f32, f32)) -> f32 {
    if cell >= 1.0 {
        value_noise(fx / cell + off.0, fy / cell + off.1, seed)
    } else {
        // ⚠ Smaller than a pixel: one lattice point per pixel, and the spread of `1 / cell²` cells
        // averaged together.
        value_noise(fx + off.0, fy + off.1, seed) * cell
    }
}

/// The grain at frame pixel `(fx, fy)` — zero-mean, the strength of one octave.
pub fn grain_noise(fx: f32, fy: f32, p: &GrainParams) -> f32 {
    let n1 = octave(fx, fy, p.cell, SEED, (0.0, 0.0));
    if p.rough <= 0.0 {
        return n1;
    }
    let n2 = octave(fx, fy, p.cell * 0.5, SEED2, OFFSET);
    let r = p.rough;
    ((1.0 - r) * n1 + r * n2) / ((1.0 - r) * (1.0 - r) + r * r).sqrt()
}

/// One pixel with its grain, at frame pixel `(fx, fy)`.
pub fn grain_px(rgb: [f32; 3], fx: f32, fy: f32, p: &GrainParams) -> [f32; 3] {
    if p.amp <= 0.0 {
        return rgb;
    }
    let l = color::linear_to_srgb(color::luminance(rgb).clamp(0.0, 1.0));
    let w = 4.0 * l * (1.0 - l);
    let gain = (p.amp * grain_noise(fx, fy, p) * w).exp2();
    [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
}

/// The grain stage.
pub struct GrainStage {
    params: GrainParams,
    region: Option<Region>,
}

impl GrainStage {
    pub fn new_in_region(g: &Grain, scale: RenderScale, region: Option<Region>) -> Self {
        Self {
            params: grain_params(g, scale),
            region,
        }
    }
}

struct GrainPx {
    params: GrainParams,
    x0: usize,
    y0: usize,
}

impl crate::per_pixel::PerPixel for GrainPx {
    fn px(&self, x: usize, y: usize, rgb: [f32; 3]) -> [f32; 3] {
        grain_px(
            rgb,
            (x + self.x0) as f32,
            (y + self.y0) as f32,
            &self.params,
        )
    }
}

impl GrainStage {
    fn prepare(&self) -> Option<GrainPx> {
        if self.params.amp <= 0.0 {
            return None;
        }
        // ⚠ Where this buffer sits in the frame — the vignette's rule, so a region is the same crop.
        let (x0, y0) = self.region.map_or((0, 0), |r| (r.x0, r.y0));
        Some(GrainPx {
            params: self.params,
            x0,
            y0,
        })
    }
}

impl PipelineStage for GrainStage {
    fn name(&self) -> &'static str {
        "grain"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        match self.prepare() {
            Some(px) => crate::per_pixel::apply_one(&px, image),
            None => image,
        }
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        self.prepare()
            .map(|px| Box::new(px) as Box<dyn crate::per_pixel::PerPixel + '_>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grain(amount: f32, size: f32, roughness: f32) -> Grain {
        Grain {
            amount,
            size,
            roughness,
        }
    }

    fn flat(w: usize, h: usize, v: f32) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// The per-pixel gain in stops a render applied to a flat frame of `v`.
    fn stops(g: &Grain, v: f32, scale: f32) -> Vec<f32> {
        let out =
            GrainStage::new_in_region(g, RenderScale::new(scale), None).apply(flat(128, 128, v));
        out.data()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| (p[0] / v).log2())
            .collect()
    }

    fn spread(v: &[f32]) -> f32 {
        let m = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn off_changes_nothing() {
        let img = flat(16, 16, 0.3);
        let out = GrainStage::new_in_region(&Grain::default(), RenderScale::FULL, None)
            .apply(img.clone());
        assert_eq!(out.data(), img.data());
    }

    #[test]
    fn grain_leaves_the_average_where_it_was_and_scales_with_amount() {
        let out = GrainStage::new_in_region(&grain(50.0, 25.0, 50.0), RenderScale::FULL, None)
            .apply(flat(256, 256, 0.18));
        let mean = out.data().iter().sum::<f32>() / out.data().len() as f32;
        assert!((mean / 0.18 - 1.0).abs() < 0.02, "the mean moved: {mean}");
        let (half, full) = (
            spread(&stops(&grain(50.0, 25.0, 50.0), 0.18, 1.0)),
            spread(&stops(&grain(100.0, 25.0, 50.0), 0.18, 1.0)),
        );
        assert!(half > 0.01, "no grain: {half}");
        assert!((full / half - 2.0).abs() < 0.01, "{half} → {full}");
    }

    #[test]
    fn the_midtones_carry_the_most_grain() {
        let g = grain(80.0, 25.0, 50.0);
        let (mid, shadow, high) = (
            spread(&stops(&g, 0.18, 1.0)),
            spread(&stops(&g, 0.004, 1.0)),
            spread(&stops(&g, 4.0, 1.0)),
        );
        assert!(
            mid > 2.0 * shadow && shadow > 0.0,
            "mid {mid}, shadow {shadow}"
        );
        // ⚠ None to within rounding: `linear_to_srgb(1.0)` lands a hair under 1, a weight of ~2e-7.
        assert!(high < 1e-6, "a highlight past white takes none: {high}");
        let black = GrainStage::new_in_region(&g, RenderScale::FULL, None).apply(flat(8, 8, 0.0));
        assert!(black.data().iter().all(|&v| v == 0.0), "black stays black");
    }

    #[test]
    fn the_grain_sits_on_the_photo_at_any_scale() {
        // ⚠ Pixel x at a quarter scale reads exactly what pixel 4x does at full — the cells are sized
        // in the frame's own pixels, so a preview and the export show the same grain.
        let g = grain(50.0, 100.0, 0.0);
        let (full, quarter) = (
            grain_params(&g, RenderScale::FULL),
            grain_params(&g, RenderScale::new(0.25)),
        );
        for (x, y) in [(0, 0), (3, 7), (10, 2), (25, 31)] {
            let a = grain_noise(x as f32, y as f32, &quarter);
            let b = grain_noise((4 * x) as f32, (4 * y) as f32, &full);
            assert!((a - b).abs() < 1e-5, "({x},{y}): {a} against {b}");
        }
    }

    #[test]
    fn a_cell_smaller_than_a_pixel_averages_down_rather_than_aliasing() {
        // Size 0 is a one-pixel cell at full scale; at a quarter it is a quarter pixel, and four
        // by four cells average into each pixel — a quarter of the spread.
        let g = grain(80.0, 0.0, 0.0);
        let (full, quarter) = (
            spread(&stops(&g, 0.18, 1.0)),
            spread(&stops(&g, 0.18, 0.25)),
        );
        assert!((quarter / full - 0.25).abs() < 0.04, "{full} → {quarter}");
    }

    #[test]
    fn roughness_adds_finer_grain() {
        let fine = |r: f32| {
            let p = grain_params(&grain(50.0, 60.0, r), RenderScale::FULL);
            let n: Vec<f32> = (0..4096)
                .map(|i| grain_noise((i % 64) as f32, (i / 64) as f32, &p))
                .collect();
            let diff = (0..4095)
                .filter(|i| i % 64 != 63)
                .map(|i| (n[i + 1] - n[i]).powi(2))
                .sum::<f32>();
            diff / n.iter().map(|v| v * v).sum::<f32>()
        };
        assert!(
            fine(100.0) > 1.5 * fine(0.0),
            "{} against {}",
            fine(100.0),
            fine(0.0)
        );
    }

    #[test]
    fn a_region_is_the_same_crop_of_the_whole_frame() {
        let mut img = ImageBuf::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                img.set_pixel(x, y, [0.1 + x as f32 / 128.0, 0.2, 0.3 + y as f32 / 200.0]);
            }
        }
        let g = grain(70.0, 30.0, 60.0);
        let whole = GrainStage::new_in_region(&g, RenderScale::FULL, None).apply(img.clone());
        let region = Region {
            full_w: 64,
            full_h: 48,
            x0: 10,
            y0: 7,
        };
        let part = GrainStage::new_in_region(&g, RenderScale::FULL, Some(region))
            .apply(img.crop(10, 7, 20, 15));
        assert_eq!(part.data(), whole.crop(10, 7, 20, 15).data());
        let again = GrainStage::new_in_region(&g, RenderScale::FULL, None).apply(img);
        assert_eq!(again.data(), whole.data(), "the same every time");
    }
}

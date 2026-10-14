//! A photo's depth map, finished (M10.16a): the network's depth upsampled to the stored size and its
//! edges snapped to the photo's own.
//!
//! ⚠⚠ **The snap is Dehaze's trick, reused.** The network answers at ~770 px and its depth edges are
//! soft; a mask across a person's outline would fade over a dozen pixels, and Lens Blur (M10.16d) would
//! halo. `pipeline::spatial::guided_filter`, guided by the photo's luminance, pulls a soft map onto the
//! photo's real edges — exactly what Dehaze does to its patch-blocky transmission map.
//!
//! ⚠ Here, not in `ai-mask`: that crate keeps no `pipeline` dependency.

use ai_mask::depth::{DepthScale, Plane};
use ai_mask::ProbMap;
use image::RgbImage;

/// How a depth map is made — every number that changes the stored map. ⚠ A change here is a new
/// recipe: bump `ai_coverage::DEPTH_QUERY`, or every cached map keeps the old look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recipe {
    /// The network's input, long side (a multiple of 14 is chosen from it).
    pub infer_long: u32,
    /// The stored map's long side — and the guide's.
    pub store_long: u32,
    /// The guided filter's radius at the stored size, pixels; 0 = no snap.
    pub radius: usize,
    /// The guided filter's regulariser, in squared luminance.
    pub eps: f32,
    pub scale: DepthScale,
}

/// ⚠⚠ The author's choices from the spike's sheets (2026-10-14, `examples/depth_probe.rs sheet`):
/// **equalised** — linear disparity crushed the trees, the far shore and the sky into one black;
/// **snap radius 8** — hair strands where the photo has contrast, slightly softer where it has none
/// (r16 blurred everywhere, none left a blocky staircase); **stored at 1536** — no visible difference
/// from 2048 at the network's resolution. Inference at **770** by the rule: the largest size under
/// ~3 s (1.65 s on an M2; 1036 took 3.9 s).
pub const RECIPE: Recipe = Recipe {
    infer_long: 770,
    store_long: 1536,
    radius: 8,
    eps: 1e-3,
    scale: DepthScale::Equalised,
};

/// Rec.709 luma of the 8-bit sRGB `guide`, 0..1 — on the encoded values, as the eye sees edges.
pub fn luma(guide: &RgbImage) -> Vec<f32> {
    guide
        .pixels()
        .map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0)
        .collect()
}

/// `p` resampled to `w × h`, bilinear, pixel centres aligned, clamped at the edges.
fn upsample(p: &Plane, w: u32, h: u32) -> Vec<f32> {
    let (pw, ph) = (p.w.max(1), p.h.max(1));
    let at = |x: u32, y: u32| p.data[(y.min(ph - 1) * pw + x.min(pw - 1)) as usize];
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) * ph as f32 / h as f32 - 0.5).clamp(0.0, (ph - 1) as f32);
        let (y0, fy) = (sy.floor() as u32, sy.fract());
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * pw as f32 / w as f32 - 0.5).clamp(0.0, (pw - 1) as f32);
            let (x0, fx) = (sx.floor() as u32, sx.fract());
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            out.push(top * (1.0 - fy) + bot * fy);
        }
    }
    out
}

/// `position` (0 nearest … 1 farthest) at `guide`'s size, snapped to its edges by `r`, as a map.
pub fn finish(position: &Plane, guide: &RgbImage, r: &Recipe) -> ProbMap {
    let (w, h) = (guide.width(), guide.height());
    let p = upsample(position, w, h);
    let p = if r.radius > 0 {
        pipeline::spatial::guided_filter(&p, &luma(guide), w as usize, h as usize, r.radius, r.eps)
    } else {
        p
    };
    ProbMap::from_fn(w, h, |x, y| p[(y * w + x) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The share of row `y`'s whole rise that happens in the ONE step across the photo's edge, between
    /// x = 99 and 100 — where a snapped depth edge belongs.
    ///
    /// ⚠⚠ Not a 10 %→90 % width — measured, and that metric was the wrong one: a guided filter makes a
    /// hard step at the guide's edge (0.23 → 0.72 in one pixel) but leaves gentle tails within its
    /// radius either side, and the width read those tails and called the snap a widening (17 vs 13).
    /// `scratchpad/gf.py`'s 1-D simulation of the same algebra gave the numbers this test asserts.
    fn share_at_the_edge(m: &ProbMap, y: u32) -> f32 {
        let at = |x: u32| m.data[(y * m.w + x) as usize] as f32 / 255.0;
        let rise = at(m.w - 1) - at(0);
        (at(100) - at(99)) / rise.max(1e-6)
    }

    /// A guide with a hard step at x = 100, and a soft depth ramp across x 92..108 — the network's
    /// blurred version of that same edge.
    ///
    /// ⚠ 16 px, not wider: a guided filter acts only within its radius of the guide's edge, and the
    /// network's real soft edge at the stored size is ~3 of its pixels × the 2× upsample ≈ 6 px. The
    /// first fixture ramped over 40 px, put its 10 % and 90 % points 16 px from the edge — outside an
    /// 8-px radius — and measured the snap as making it WIDER (35 vs 32): a fixture no network makes.
    fn soft_edge() -> (Plane, RgbImage) {
        let (w, h) = (200u32, 20u32);
        let guide = RgbImage::from_fn(w, h, |x, _| {
            let v = if x < 100 { 51 } else { 204 };
            image::Rgb([v, v, v])
        });
        let data = (0..h)
            .flat_map(|_| (0..w).map(|x| ((x as f32 - 92.0) / 16.0).clamp(0.0, 1.0)))
            .collect();
        (Plane { w, h, data }, guide)
    }

    #[test]
    fn the_snap_pulls_a_soft_edge_onto_the_photos_edge() {
        let (position, guide) = soft_edge();
        let snap = Recipe {
            radius: 8,
            eps: 1e-3,
            ..RECIPE
        };
        let none = Recipe { radius: 0, ..snap };
        let (soft, snapped) = (
            finish(&position, &guide, &none),
            finish(&position, &guide, &snap),
        );
        // The control: unsnapped, the ramp rises 1/16 per pixel, the edge's step no different.
        let (soft, snapped) = (
            share_at_the_edge(&soft, 10),
            share_at_the_edge(&snapped, 10),
        );
        assert!(soft < 0.1, "{soft}");
        // Simulated 0.49 at radius 8, eps 1e-3: half the depth change lands on the photo's edge.
        assert!(
            snapped > 0.4 && snapped > 4.0 * soft,
            "snapped {snapped} vs soft {soft}"
        );
    }

    #[test]
    fn finish_has_the_guides_size_and_stays_in_range() {
        let position = Plane {
            w: 7,
            h: 5,
            data: (0..35).map(|i| i as f32 / 34.0).collect(),
        };
        let guide = RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 9]));
        let m = finish(&position, &guide, &RECIPE);
        assert_eq!((m.w, m.h), (64, 48));
        // Upsampled, not shuffled: the far corner stays far, the near corner near.
        assert!(
            m.data[0] < 40 && m.data[64 * 48 - 1] > 215,
            "{} {}",
            m.data[0],
            m.data[64 * 48 - 1]
        );
    }

    #[test]
    fn luma_reads_the_encoded_value() {
        let g = RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 255, 0])
            }
        });
        let l = luma(&g);
        assert!((l[0] - 1.0).abs() < 1e-6);
        assert!((l[1] - 0.7152).abs() < 1e-3);
    }
}

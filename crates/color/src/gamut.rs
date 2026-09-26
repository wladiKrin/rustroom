//! Rendering intents: how colour the target space cannot hold is brought inside it (M6.4c).
//!
//! ⚠⚠ **Why this is ours and not LittleCMS's.** All four output profiles are *matrix-shaper*
//! profiles, which carry no perceptual tables, so lcms's "perceptual" on them falls back to
//! colorimetric — the same per-channel clip this module replaces. That clip shifts hue: an orange
//! whose blue goes negative is pulled toward yellow, a violet toward blue.
//!
//! Both intents here work in **Oklab at constant lightness and hue**, moving chroma only:
//! - **Relative colorimetric** leaves every in-gamut colour bit-identical and cuts chroma at the
//!   target's boundary.
//! - **Perceptual** leaves chroma below [`KNEE`] of the boundary alone and compresses above it, so
//!   the working space's edge lands exactly on the target's and saturated gradations survive.
//!
//! The boundary is a table per space over (lightness, hue), found once by bisection and read
//! bilinearly — the GPU twin uploads the same tables, so there is one definition of the boundary.

use crate::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use crate::OutputSpace;
use std::sync::OnceLock;

/// Lightness samples in the boundary tables, `L = 0 ..= 1`.
pub const NL: usize = 65;
/// Hue samples in the boundary tables, `h = 0 .. 2π`, wrapping.
pub const NH: usize = 256;
/// Perceptual: the fraction of the boundary below which chroma is left alone.
pub const KNEE: f32 = 0.8;
/// The largest chroma the bisection considers. ProPhoto's imaginary primaries reach far past any
/// real target; this bounds its table rather than the targets'.
const C_MAX: f32 = 2.0;
const BISECTION_STEPS: usize = 40;

/// How out-of-gamut colour is brought into the target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum RenderingIntent {
    /// In-gamut colour untouched; outside, chroma cut to the boundary at constant hue.
    #[default]
    RelativeColorimetric,
    /// Saturated colour compressed smoothly toward the boundary, keeping gradation.
    Perceptual,
}

impl RenderingIntent {
    pub const ALL: [RenderingIntent; 2] = [
        RenderingIntent::Perceptual,
        RenderingIntent::RelativeColorimetric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RenderingIntent::Perceptual => "Perceptual",
            RenderingIntent::RelativeColorimetric => "Relative colorimetric",
        }
    }
}

/// One output space's gamut boundary, and the working space's, over (lightness, hue).
pub struct GamutMap {
    space: OutputSpace,
    /// The largest chroma `space` holds, `[l * NH + h]`.
    target: Vec<f32>,
    /// The largest chroma ProPhoto holds, on the same grid: where the source ends.
    source: Vec<f32>,
}

fn fits(space: OutputSpace, lab: [f32; 3]) -> bool {
    space
        .from_working(oklab_to_prophoto(lab))
        .iter()
        .all(|v| (0.0..=1.0).contains(v))
}

/// The largest chroma at `(l, h)` that `space` holds: the last in-gamut point of a bisection.
fn bisect(space: OutputSpace, l: f32, h: f32) -> f32 {
    let (ca, sa) = (h.cos(), h.sin());
    let (mut lo, mut hi) = (0.0f32, C_MAX);
    for _ in 0..BISECTION_STEPS {
        let mid = 0.5 * (lo + hi);
        if fits(space, [l, mid * ca, mid * sa]) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn table(space: OutputSpace) -> Vec<f32> {
    let mut t = vec![0.0; NL * NH];
    for li in 0..NL {
        let l = li as f32 / (NL - 1) as f32;
        for hi in 0..NH {
            let h = hi as f32 / NH as f32 * std::f32::consts::TAU;
            t[li * NH + hi] = bisect(space, l, h);
        }
    }
    t
}

/// Bilinear read of a boundary table at lightness `l ∈ [0, 1]` and hue `h` in radians.
fn lookup(t: &[f32], l: f32, h: f32) -> f32 {
    let v = l.clamp(0.0, 1.0) * (NL - 1) as f32;
    let l0 = (v.floor() as usize).min(NL - 2);
    let fl = v - l0 as f32;
    let u = h.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * NH as f32;
    let h0 = (u.floor() as usize) % NH;
    let h1 = (h0 + 1) % NH;
    let fh = u - u.floor();
    let at = |li: usize, hi: usize| t[li * NH + hi];
    let row = |li| at(li, h0) + fh * (at(li, h1) - at(li, h0));
    row(l0) + fl * (row(l0 + 1) - row(l0))
}

impl GamutMap {
    /// The map into `space`, built on first use and shared thereafter.
    pub fn for_space(space: OutputSpace) -> &'static GamutMap {
        static MAPS: [OnceLock<GamutMap>; 4] = [
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
        ];
        let k = OutputSpace::ALL
            .iter()
            .position(|&s| s == space)
            .expect("ALL lists every space");
        MAPS[k].get_or_init(|| {
            static SOURCE: OnceLock<Vec<f32>> = OnceLock::new();
            GamutMap {
                space,
                target: table(space),
                source: SOURCE.get_or_init(|| table(OutputSpace::ProPhoto)).clone(),
            }
        })
    }

    /// The target boundary table, `NL × NH`, row-major by lightness — for the GPU twin.
    pub fn target_table(&self) -> &[f32] {
        &self.target
    }

    /// The working space's boundary table on the same grid — for the GPU twin.
    pub fn source_table(&self) -> &[f32] {
        &self.source
    }

    /// The largest chroma the target holds at Oklab lightness `l` and hue `h` (radians).
    pub fn boundary(&self, l: f32, h: f32) -> f32 {
        lookup(&self.target, l, h)
    }

    /// Bring a **linear ProPhoto** colour inside the target under `intent`; the result is still
    /// linear ProPhoto, ready for `OutputSpace::from_working`.
    ///
    /// ⚠ Returns its argument untouched whenever nothing is to be done — relative colorimetric
    /// on an in-gamut colour, perceptual below the knee, ProPhoto as the target — so those
    /// pixels are bit-identical to the transform this replaced, not merely close.
    pub fn map(&self, intent: RenderingIntent, rgb: [f32; 3]) -> [f32; 3] {
        if self.space == OutputSpace::ProPhoto {
            return rgb;
        }
        if intent == RenderingIntent::RelativeColorimetric
            && self
                .space
                .from_working(rgb)
                .iter()
                .all(|v| (0.0..=1.0).contains(v))
        {
            return rgb;
        }
        let [l, a, b] = prophoto_to_oklab(rgb);
        if l <= 0.0 {
            return [0.0; 3];
        }
        if l >= 1.0 {
            return [1.0; 3];
        }
        let c = a.hypot(b);
        let h = b.atan2(a);
        let bound = self.boundary(l, h);
        let target = match intent {
            RenderingIntent::RelativeColorimetric => c.min(bound),
            RenderingIntent::Perceptual => {
                let t = KNEE * bound;
                if c <= t {
                    return rgb;
                }
                perceptual_chroma(c, t, bound, lookup(&self.source, l, h))
            }
        };
        if target >= c {
            return rgb;
        }
        let s = if c > 0.0 { target / c } else { 0.0 };
        oklab_to_prophoto([l, a * s, b * s])
    }
}

/// Perceptual's knee: identity at `t`, with slope 1 there, mapping `source` exactly onto `bound`
/// and never past it. `k·x / (1 + (k−1)·x)` is the rational curve with `f(0) = 0`, `f'(0) = k`,
/// `f(1) = 1`.
fn perceptual_chroma(c: f32, t: f32, bound: f32, source: f32) -> f32 {
    let s = source.max(bound);
    if s - t <= f32::EPSILON || bound - t <= f32::EPSILON {
        return c.min(bound);
    }
    let k = (s - t) / (bound - t);
    let x = (c - t) / (s - t);
    (t + (bound - t) * k * x / (1.0 + (k - 1.0) * x)).min(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGETS: [OutputSpace; 3] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
    ];

    fn lch(rgb: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = prophoto_to_oklab(rgb);
        [l, a.hypot(b), b.atan2(a)]
    }

    fn hue_diff_deg(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d).to_degrees()
    }

    /// The per-channel clip this module replaces: matrix, clamp, back to working.
    fn old_clip(space: OutputSpace, rgb: [f32; 3]) -> [f32; 3] {
        let lin = space.from_working(rgb).map(|v| v.clamp(0.0, 1.0));
        // Invert the matrix numerically through its columns.
        let m = |e: [f32; 3]| space.from_working(e);
        let (x, y, z) = (m([1.0, 0.0, 0.0]), m([0.0, 1.0, 0.0]), m([0.0, 0.0, 1.0]));
        let cols = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
        solve3(cols, lin)
    }

    fn solve3(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        let det = |m: [[f32; 3]; 3]| {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let d = det(m);
        let mut out = [0.0; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let mut mi = m;
            for r in 0..3 {
                mi[r][i] = v[r];
            }
            *o = det(mi) / d;
        }
        out
    }

    /// A grid over the working space, well outside every target.
    fn spread() -> Vec<[f32; 3]> {
        let steps = [0.0, 0.05, 0.2, 0.45, 0.7, 1.0];
        let mut v = Vec::new();
        for r in steps {
            for g in steps {
                for b in steps {
                    v.push([r, g, b]);
                }
            }
        }
        v
    }

    /// ⚠⚠ Assert the sample first: a grid that happened to sit inside sRGB would pass everything
    /// below vacuously.
    #[test]
    fn the_spread_is_mostly_outside_srgb() {
        let out = spread()
            .into_iter()
            .filter(|&c| crate::is_out_of_gamut(c, OutputSpace::Srgb))
            .count();
        assert!(out > 100, "only {out} of 216 are outside sRGB");
    }

    #[test]
    fn every_stored_boundary_is_inside_and_a_little_more_is_outside() {
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            let mut checked = 0;
            for li in 1..NL - 1 {
                let l = li as f32 / (NL - 1) as f32;
                for hi in (0..NH).step_by(7) {
                    let h = hi as f32 / NH as f32 * std::f32::consts::TAU;
                    let c = m.target[li * NH + hi];
                    assert!(fits(space, [l, c * h.cos(), c * h.sin()]));
                    let more = c * 1.01 + 1e-4;
                    assert!(
                        !fits(space, [l, more * h.cos(), more * h.sin()]),
                        "{space:?} L={l} h={h}: {c} is not the edge"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 1000);
        }
    }

    /// ⚠ To 1%, not exactly: next to black and white both boundaries shrink to a few thousandths
    /// and P3's measured 0.5% past ProPhoto's at chroma 0.007 — bisection noise, not a gamut.
    /// `map` takes the larger of the two for exactly that reason.
    #[test]
    fn the_working_space_boundary_contains_every_target() {
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for (t, s) in m.target.iter().zip(&m.source) {
                assert!(
                    s * 1.01 + 1e-6 >= *t,
                    "{space:?}: target {t} beyond source {s}"
                );
            }
        }
    }

    /// ⚠ The bound is MEASURED: the bilinear boundary overshoots the true one slightly where it is
    /// convex, and `encode` clamps that residue. What is asserted is that the residue stays small.
    #[test]
    fn every_mapped_colour_lands_inside_the_target() {
        let mut worst = 0.0f32;
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for intent in RenderingIntent::ALL {
                for c in spread() {
                    let lin = space.from_working(m.map(intent, c));
                    for v in lin {
                        worst = worst.max(-v).max(v - 1.0);
                    }
                }
            }
        }
        // Measured 2026-09-26: 7.3e-4 of full scale.
        assert!(worst < 1e-3, "worst excursion {worst}");
    }

    #[test]
    fn lightness_and_hue_are_kept() {
        let mut worst_l = 0.0f32;
        let mut worst_h = 0.0f32;
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for intent in RenderingIntent::ALL {
                for c in spread() {
                    let before = lch(c);
                    if before[0] <= 0.0 || before[0] >= 1.0 || before[1] < 1e-3 {
                        continue;
                    }
                    let after = lch(m.map(intent, c));
                    worst_l = worst_l.max((after[0] - before[0]).abs());
                    if after[1] > 1e-3 {
                        worst_h = worst_h.max(hue_diff_deg(after[2], before[2]));
                    }
                }
            }
        }
        // Measured 2026-09-26: lightness 1.2e-7 (float), hue 0.0014°.
        assert!(worst_l < 1e-6, "lightness moved by {worst_l}");
        assert!(worst_h < 0.01, "hue moved by {worst_h}°");
    }

    #[test]
    fn relative_leaves_every_in_gamut_colour_bit_identical() {
        let mut seen = 0;
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for c in spread() {
                if !crate::is_out_of_gamut(c, space) {
                    assert_eq!(m.map(RenderingIntent::RelativeColorimetric, c), c);
                    seen += 1;
                }
            }
        }
        assert!(seen > 20, "only {seen} in-gamut samples");
    }

    /// ⚠ The discriminating fixture: inside P3, outside sRGB. A map that ignored its space would
    /// treat both the same.
    #[test]
    fn a_colour_inside_p3_and_outside_srgb_moves_only_for_srgb() {
        let c = crate::srgb_to_prophoto([1.15, 0.2, 0.05]);
        assert!(crate::is_out_of_gamut(c, OutputSpace::Srgb));
        assert!(!crate::is_out_of_gamut(c, OutputSpace::DisplayP3));
        let r = RenderingIntent::RelativeColorimetric;
        assert_eq!(GamutMap::for_space(OutputSpace::DisplayP3).map(r, c), c);
        assert_ne!(GamutMap::for_space(OutputSpace::Srgb).map(r, c), c);
    }

    #[test]
    fn prophoto_is_the_identity_for_both_intents() {
        let m = GamutMap::for_space(OutputSpace::ProPhoto);
        for intent in RenderingIntent::ALL {
            for c in spread() {
                assert_eq!(m.map(intent, c), c);
            }
        }
    }

    /// Along one ray of constant lightness and hue: perceptual is monotone, continuous at the
    /// knee, never past the boundary, and lower than relative only where it should be.
    #[test]
    fn perceptual_is_monotone_and_continuous_along_a_ray() {
        let m = GamutMap::for_space(OutputSpace::Srgb);
        let (l, h) = (0.6f32, 0.5f32);
        let bound = m.boundary(l, h);
        let mut prev = 0.0f32;
        let mut steps = 0;
        let mut c = 0.0f32;
        while c < 0.5 {
            let rgb = oklab_to_prophoto([l, c * h.cos(), c * h.sin()]);
            let out = lch(m.map(RenderingIntent::Perceptual, rgb))[1];
            assert!(out + 1e-5 >= prev, "not monotone at c={c}: {out} < {prev}");
            assert!(out <= bound + 1e-4, "past the boundary at c={c}");
            // Continuity: no step larger than the input step (slope ≤ 1 everywhere).
            assert!(out - prev <= 0.0021, "a jump at c={c}: {prev} → {out}");
            prev = out;
            c += 0.002;
            steps += 1;
        }
        assert!(
            prev > 0.95 * bound,
            "the far end should approach the boundary"
        );
        assert!(steps > 200);
    }

    /// ⚠⚠ **The plan's done-when.** A saturated red subject exported to sRGB keeps its hue — and
    /// the old clip is asserted to shift it FIRST, so the fixture can fail.
    #[test]
    fn a_saturated_red_keeps_its_hue_where_the_old_clip_shifted_it() {
        let red = [0.85f32, 0.08, 0.02];
        let space = OutputSpace::Srgb;
        assert!(crate::is_out_of_gamut(red, space));
        let h0 = lch(red)[2];
        let clipped = hue_diff_deg(lch(old_clip(space, red))[2], h0);
        assert!(clipped > 3.0, "the old clip shifted hue by only {clipped}°");
        let m = GamutMap::for_space(space);
        for intent in RenderingIntent::ALL {
            let d = hue_diff_deg(lch(m.map(intent, red))[2], h0);
            assert!(d < 0.05, "{intent:?} shifted hue by {d}°");
        }
    }
}

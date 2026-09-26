//! Rendering intents: how colour the target space cannot hold is brought inside it (M6.4c).
//!
//! ⚠⚠ **Why this is ours and not LittleCMS's.** All four output profiles are *matrix-shaper*
//! profiles, which carry no perceptual tables, so lcms's "perceptual" on them falls back to
//! colorimetric — a per-channel clip. That clip shifts hue: an orange whose blue goes negative is
//! pulled toward yellow, a violet toward blue.
//!
//! ⚠⚠ **[`RenderingIntent::Clip`] IS that clip, it is the DEFAULT, and that is the author's call
//! from his own photographs (2026-09-26).** `probe_gamut_crops` rendered his six most out-of-gamut
//! photos (23–46% of the frame outside sRGB) all ways: the clip was the most vivid in every crop,
//! and the hue-preserving intents read duller — reds brown-orange, foliage teal. The camera
//! profiles were tuned under Adobe's pipeline, whose export does this same clip. So the clip is
//! kept as the default and the two below are the alternatives, not replacements.
//!
//! The two hue-preserving intents work in **Oklab at constant lightness and hue**, moving chroma
//! only:
//! - **Relative colorimetric** leaves every in-gamut colour bit-identical and cuts chroma at the
//!   target's boundary.
//! - **Perceptual** leaves chroma below [`KNEE`] of the boundary alone and compresses above it, so
//!   the working space's edge lands exactly on the target's and saturated gradations survive.
//!
//! ⚠⚠ **The boundary is found EXACTLY, per pixel, by bisection — not read from a table.** A
//! bilinear (lightness × hue) table was built first and failed on the golden chart: pure sRGB
//! primaries sit exactly ON the gamut's corners (the cusps), where linear interpolation of the
//! boundary underestimates it most, and a pure red came back **49 levels** desaturated. Saturated
//! colour lives precisely at those corners. The bisection's step is Oklab → linear — cubes and a
//! matrix, no cube root — and it runs only for pixels that need it: out-of-gamut ones under
//! relative, those past the knee under perceptual (the knee itself is one in-gamut test).

use crate::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use crate::OutputSpace;

/// Perceptual: the fraction of the boundary below which chroma is left alone.
pub const KNEE: f32 = 0.8;
/// The largest chroma the working space's bisection considers. ProPhoto's imaginary primaries
/// reach far past any real target.
pub const C_MAX: f32 = 2.0;
/// Bisection steps: 2⁻²⁴ of the interval, at float precision.
pub const BISECTION_STEPS: u32 = 24;
/// How far outside `[0, 1]` a channel may sit and still count as in gamut. ⚠ Float noise, not a
/// tolerance on colour: a pure sRGB primary taken through ProPhoto and back reads ~1e-7 outside,
/// and without this it would be "out of gamut" by rounding alone.
pub const IN_GAMUT_EPS: f32 = 1e-6;

/// How out-of-gamut colour is brought into the target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum RenderingIntent {
    /// Each channel clamped on its own — what every export did before M6.4c, and what Adobe's
    /// relative colorimetric does on a matrix profile. Vivid; shifts hue. **The default** (see
    /// the module doc for why).
    #[default]
    Clip,
    /// In-gamut colour untouched; outside, chroma cut to the boundary at constant hue.
    RelativeColorimetric,
    /// Saturated colour compressed smoothly toward the boundary, keeping gradation.
    Perceptual,
}

impl RenderingIntent {
    pub const ALL: [RenderingIntent; 3] = [
        RenderingIntent::Clip,
        RenderingIntent::Perceptual,
        RenderingIntent::RelativeColorimetric,
    ];

    /// The two that keep hue — what every property test of the gamut map is about.
    pub const HUE_PRESERVING: [RenderingIntent; 2] = [
        RenderingIntent::Perceptual,
        RenderingIntent::RelativeColorimetric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RenderingIntent::Clip => "Clip",
            RenderingIntent::Perceptual => "Perceptual",
            RenderingIntent::RelativeColorimetric => "Relative colorimetric",
        }
    }
}

/// Whether linear `rgb` in `space` is inside `[0, 1]³`, to float noise.
fn inside(rgb: [f32; 3]) -> bool {
    rgb.iter()
        .all(|v| (-IN_GAMUT_EPS..=1.0 + IN_GAMUT_EPS).contains(v))
}

/// Whether the Oklab colour `[l, c·cos h, c·sin h]` fits `space`.
fn fits(space: OutputSpace, l: f32, c: f32, (ca, sa): (f32, f32)) -> bool {
    inside(space.from_working(oklab_to_prophoto([l, c * ca, c * sa])))
}

/// The largest chroma in `[lo, hi]` that `space` holds at lightness `l` along direction
/// `(cos h, sin h)`: the last in-gamut point of a bisection. ⚠ `lo` must fit.
fn bisect(space: OutputSpace, l: f32, dir: (f32, f32), mut lo: f32, mut hi: f32) -> f32 {
    for _ in 0..BISECTION_STEPS {
        let mid = 0.5 * (lo + hi);
        if fits(space, l, mid, dir) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The gamut map into one output space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamutMap {
    space: OutputSpace,
}

impl GamutMap {
    pub fn for_space(space: OutputSpace) -> GamutMap {
        GamutMap { space }
    }

    /// The largest chroma the target holds at Oklab lightness `l` and hue `h` (radians).
    pub fn boundary(&self, l: f32, h: f32) -> f32 {
        bisect(self.space, l, (h.cos(), h.sin()), 0.0, C_MAX)
    }

    /// Bring a **linear ProPhoto** colour inside the target under `intent`; the result is still
    /// linear ProPhoto, ready for `OutputSpace::from_working`.
    ///
    /// ⚠ Returns its argument untouched whenever nothing is to be done — relative colorimetric
    /// on an in-gamut colour, perceptual below the knee, ProPhoto as the target — so those
    /// pixels are bit-identical to the transform this replaced, not merely close.
    pub fn map(&self, intent: RenderingIntent, rgb: [f32; 3]) -> [f32; 3] {
        // ⚠ Clip maps nothing: `OutputSpace::encode` clamps each channel, which IS the clip, so
        // the default render is bit-identical to the transform before M6.4c.
        if self.space == OutputSpace::ProPhoto || intent == RenderingIntent::Clip {
            return rgb;
        }
        if intent == RenderingIntent::RelativeColorimetric && inside(self.space.from_working(rgb)) {
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
        if c <= 0.0 {
            // A grey outside the target can only be a lightness overshoot, handled above; this
            // is float noise on the axis.
            return rgb;
        }
        let dir = (a / c, b / c);
        let target = match intent {
            // ⚠ `c` itself is outside, so the boundary lies in `[0, c]`.
            RenderingIntent::Clip => unreachable!("returned above"),
            RenderingIntent::RelativeColorimetric => bisect(self.space, l, dir, 0.0, c),
            RenderingIntent::Perceptual => {
                // ⚠ The knee test is ONE in-gamut check: `c ≤ KNEE·bound` exactly when
                // `c / KNEE` still fits. Most pixels stop here.
                let reach = c / KNEE;
                if fits(self.space, l, reach, dir) {
                    return rgb;
                }
                let bound = bisect(self.space, l, dir, 0.0, reach);
                let source = bisect(OutputSpace::ProPhoto, l, dir, bound, C_MAX.max(bound));
                perceptual_chroma(c, KNEE * bound, bound, source)
            }
        };
        if target >= c {
            return rgb;
        }
        let s = target / c;
        oklab_to_prophoto([l, a * s, b * s])
    }
}

/// Perceptual's knee: identity at `t`, with slope 1 there, mapping `source` exactly onto `bound`
/// and never past it. `k·x / (1 + (k−1)·x)` is the rational curve with `f(0) = 0`, `f'(0) = k`,
/// `f(1) = 1`.
pub fn perceptual_chroma(c: f32, t: f32, bound: f32, source: f32) -> f32 {
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
    fn every_boundary_is_inside_and_a_little_more_is_outside() {
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            let mut checked = 0;
            for li in 1..64 {
                let l = li as f32 / 64.0;
                for hi in 0..40 {
                    let h = hi as f32 / 40.0 * std::f32::consts::TAU;
                    let dir = (h.cos(), h.sin());
                    let c = m.boundary(l, h);
                    assert!(fits(space, l, c, dir));
                    let more = c * 1.001 + 1e-5;
                    assert!(
                        !fits(space, l, more, dir),
                        "{space:?} L={l} h={h}: {c} is not the edge"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 1000);
        }
    }

    /// ⚠⚠ **To 1%, not exactly — and the gap is REAL, not rounding.** Measured 2026-09-26 with the
    /// exact boundary: at Oklab hue ~30° (red) Display P3 reaches up to **0.65%** more chroma than
    /// ProPhoto, at every lightness. After white-point adaptation ProPhoto does not quite hold that
    /// sliver of P3's red. `map` takes the larger of the two for exactly this reason.
    #[test]
    fn the_working_space_boundary_contains_every_target() {
        let pro = GamutMap::for_space(OutputSpace::ProPhoto);
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for li in 1..32 {
                let l = li as f32 / 32.0;
                for hi in 0..36 {
                    let h = hi as f32 / 36.0 * std::f32::consts::TAU;
                    let (t, s) = (m.boundary(l, h), pro.boundary(l, h));
                    assert!(
                        t <= s * 1.01,
                        "{space:?} L={l} h={h}: target {t} beyond source {s}"
                    );
                }
            }
        }
    }

    /// ⚠ The bound is MEASURED: what is left for `encode`'s clamp is float residue.
    #[test]
    fn every_mapped_colour_lands_inside_the_target() {
        let mut worst = 0.0f32;
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for intent in RenderingIntent::HUE_PRESERVING {
                for c in spread() {
                    let lin = space.from_working(m.map(intent, c));
                    for v in lin {
                        worst = worst.max(-v).max(v - 1.0);
                    }
                }
            }
        }
        // Measured 2026-09-26: 4.2e-4 of full scale.
        assert!(worst < 1e-3, "worst excursion {worst}");
    }

    #[test]
    fn lightness_and_hue_are_kept() {
        let mut worst_l = 0.0f32;
        let mut worst_h = 0.0f32;
        for space in TARGETS {
            let m = GamutMap::for_space(space);
            for intent in RenderingIntent::HUE_PRESERVING {
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
        // Measured 2026-09-26: lightness 1.8e-7 (float), hue 0.0009°.
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
    fn prophoto_is_the_identity_for_every_intent() {
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
        for intent in RenderingIntent::HUE_PRESERVING {
            let d = hue_diff_deg(lch(m.map(intent, red))[2], h0);
            assert!(d < 0.05, "{intent:?} shifted hue by {d}°");
        }
    }

    /// ⚠⚠ Clip is the transform before M6.4c, bit for bit: the map returns its argument and the
    /// encode's per-channel clamp does the rest. The fixture is out of gamut, or "returns its
    /// argument" would hold for every intent.
    #[test]
    fn clip_maps_nothing() {
        let m = GamutMap::for_space(OutputSpace::Srgb);
        let mut seen = 0;
        for c in spread() {
            if crate::is_out_of_gamut(c, OutputSpace::Srgb) {
                assert_eq!(m.map(RenderingIntent::Clip, c), c);
                seen += 1;
            }
        }
        assert!(seen > 100);
        assert_eq!(RenderingIntent::default(), RenderingIntent::Clip);
    }
}

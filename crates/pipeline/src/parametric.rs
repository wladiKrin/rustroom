//! The parametric tone curve (M5.1c): four region amounts over three splits, Lightroom's model.
//!
//! ⚠⚠ **Display-referred.** `u` is the curve widget's x (`linear_to_srgb` of the working value), so
//! "shadows" is the widget's left quarter. `ToneCurveStage` converts at its boundary.
//!
//! ⚠⚠ **Each region is a raised-cosine bump, and its amplitude is CAPPED so it cannot fold the
//! curve.** A bump of amplitude `A` over radius `r` has slope up to `A·π/(2r)`; on its falling side a
//! positive bump (or on its rising side a negative one) subtracts that from the identity's slope of 1.
//! Capping `|A|` at `FOLD_MARGIN · 2r/π` keeps every single region strictly increasing. The edge
//! regions are pinned at black and white (their support ends there), which halves their radius and
//! therefore their reach: with default splits, Shadows and Highlights reach ±0.072, Darks and Lights
//! ±0.143 of display value. ⚠ Two NEIGHBOURS at opposite extremes can still add slopes; the stage
//! makes its table monotone as the final guard.

use edit_model::ParametricCurve;

/// The largest lift any region may apply at ±100, in display units.
pub const MAX_LIFT: f32 = 0.15;

/// How much of the no-fold slope budget one bump may use. Below 1 so a single region at ±100 keeps
/// the curve strictly increasing, not merely flat.
const FOLD_MARGIN: f32 = 0.9;

/// One region's bump: centre, left and right radius, amplitude.
struct Bump {
    c: f32,
    rl: f32,
    rr: f32,
    a: f32,
}

impl Bump {
    fn at(&self, u: f32) -> f32 {
        let d = u - self.c;
        let r = if d < 0.0 { self.rl } else { self.rr };
        if r <= 0.0 || d.abs() >= r {
            return 0.0;
        }
        self.a * 0.5 * (1.0 + (std::f32::consts::PI * d / r).cos())
    }
}

/// The four bumps for `p`. Region `k` spans `[b_k, b_k+1]`; its bump peaks at the region's centre
/// and reaches half-way into each neighbour, or exactly to 0 and 1 at the ends.
fn bumps(p: &ParametricCurve) -> [Bump; 4] {
    let s = sanitised_splits(p.splits);
    let b = [0.0, s[0], s[1], s[2], 1.0];
    let amounts = [p.shadows, p.darks, p.lights, p.highlights];
    std::array::from_fn(|k| {
        let (lo, hi) = (b[k], b[k + 1]);
        let c = 0.5 * (lo + hi);
        let left = if k == 0 {
            0.0
        } else {
            lo - 0.5 * (lo - b[k - 1])
        };
        let right = if k == 3 {
            1.0
        } else {
            hi + 0.5 * (b[k + 2] - hi)
        };
        let (rl, rr) = (c - left, right - c);
        let cap = (FOLD_MARGIN * 2.0 * rl.min(rr) / std::f32::consts::PI).min(MAX_LIFT);
        Bump {
            c,
            rl,
            rr,
            a: amounts[k].clamp(-100.0, 100.0) / 100.0 * cap,
        }
    })
}

/// Splits clamped into (0, 1) and forced strictly increasing, so a hand-edited sidecar cannot
/// produce an empty or inverted region.
pub fn sanitised_splits(s: [f32; 3]) -> [f32; 3] {
    const GAP: f32 = 0.01;
    let a = s[0].clamp(GAP, 1.0 - 3.0 * GAP);
    let b = s[1].clamp(a + GAP, 1.0 - 2.0 * GAP);
    let c = s[2].clamp(b + GAP, 1.0 - GAP);
    [a, b, c]
}

/// The curve at display-referred `u`, clamped to [0, 1]. The identity when `p` is inactive —
/// exactly, not approximately: callers skip it then, but it must not move a value if called.
pub fn eval(p: &ParametricCurve, u: f32) -> f32 {
    if !p.is_active() {
        return u;
    }
    let lift: f32 = bumps(p).iter().map(|b| b.at(u)).sum();
    (u + lift).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(k: usize, v: f32) -> ParametricCurve {
        let mut p = ParametricCurve::default();
        match k {
            0 => p.shadows = v,
            1 => p.darks = v,
            2 => p.lights = v,
            _ => p.highlights = v,
        }
        p
    }

    #[test]
    fn an_inactive_curve_is_the_identity_exactly() {
        let p = ParametricCurve {
            splits: [0.1, 0.3, 0.9],
            ..Default::default()
        };
        for i in 0..=100 {
            let u = i as f32 / 100.0;
            assert_eq!(eval(&p, u), u);
        }
    }

    /// Each region moves its own centre most and leaves the far regions alone.
    #[test]
    fn each_amount_moves_its_own_region() {
        let centres = [0.125, 0.375, 0.625, 0.875];
        for k in 0..4 {
            let p = with(k, 100.0);
            let lifts: Vec<f32> = centres.iter().map(|&u| eval(&p, u) - u).collect();
            let own = lifts[k];
            assert!(own > 0.05, "region {k} lifted only {own}");
            for (j, &l) in lifts.iter().enumerate() {
                if j.abs_diff(k) >= 2 {
                    assert!(
                        l.abs() < 0.01 * own.max(0.01),
                        "region {k} moved far region {j}: {l}"
                    );
                }
            }
        }
    }

    /// ⚠⚠ No single region can fold the curve, at either extreme; strictly increasing.
    #[test]
    fn a_single_region_never_folds_the_curve() {
        for k in 0..4 {
            for v in [-100.0, 100.0] {
                let p = with(k, v);
                let mut prev = eval(&p, 0.0);
                for i in 1..=2000 {
                    let y = eval(&p, i as f32 / 2000.0);
                    assert!(y > prev, "region {k} at {v} folds at {}", i as f32 / 2000.0);
                    prev = y;
                }
            }
        }
    }

    #[test]
    fn the_end_points_are_pinned() {
        for k in 0..4 {
            for v in [-100.0, 100.0] {
                let p = with(k, v);
                assert_eq!(eval(&p, 0.0), 0.0);
                assert!((eval(&p, 1.0) - 1.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn moving_a_split_moves_the_region() {
        let mut p = with(1, 100.0);
        let peak = |p: &ParametricCurve| {
            (0..=1000)
                .map(|i| i as f32 / 1000.0)
                .max_by(|a, b| (eval(p, *a) - a).total_cmp(&(eval(p, *b) - b)))
                .unwrap()
        };
        let before = peak(&p);
        p.splits = [0.4, 0.6, 0.8];
        let after = peak(&p);
        assert!(
            after > before + 0.1,
            "the darks peak moved {before} -> {after}"
        );
    }

    #[test]
    fn bad_splits_are_made_usable() {
        let s = sanitised_splits([0.9, 0.2, f32::NAN.max(0.5)]);
        assert!(
            s[0] < s[1] && s[1] < s[2] && s[2] < 1.0 && s[0] > 0.0,
            "{s:?}"
        );
    }
}

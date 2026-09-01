use crate::PipelineStage;
use core_image::ImageBuf;

const LUT_SIZE: usize = 1024;

/// Two control points closer together in `x` than this are one point.
///
/// ⚠ A zero-width span makes the secant slope infinite and every tangent derived from it
/// `NaN`, which then propagates through the whole LUT — one duplicated x would flatten the
/// entire curve to garbage rather than misbehaving locally.
const X_EPS: f32 = 1e-6;

/// A secant this shallow counts as flat. Below it the tangent clipping divides by ~0.
const FLAT_EPS: f32 = 1e-12;

/// Point tone curve compiled to a LUT. Points are (x, y) in [0, 1].
pub struct ToneCurveStage {
    lut: Vec<f32>,
}

/// A **monotone** cubic interpolant (Fritsch–Carlson) through the control points.
///
/// ⚠⚠ **Monotone specifically, not Catmull-Rom, and the difference is visible rather than
/// academic.** An unconstrained cubic through monotone data overshoots near a sharp knee — it
/// dips below the lower control point before rising to it. In a *tone* curve that is a
/// **contrast reversal**: a brighter input mapping to a darker output. It does not read as a
/// wobble, it reads as banding, and it appears exactly where someone has just pulled a
/// deliberate S. Fritsch–Carlson clips each tangent into the region where the cubic provably
/// cannot leave the interval its two control points bound, so overshoot is impossible by
/// construction rather than unlikely in practice.
///
/// ⚠ With exactly two points both tangents are the single secant, and Hermite then reduces
/// **exactly** to the straight line the previous piecewise-linear implementation drew. So a
/// two-point curve — including the identity — is bit-for-bit unchanged by this.
struct Spline {
    pts: Vec<[f32; 2]>,
    /// Tangent at each control point, already clipped for monotonicity.
    m: Vec<f32>,
}

impl Spline {
    fn new(points: &[[f32; 2]]) -> Self {
        // ⚠ Non-finite points are dropped rather than sorted. `partial_cmp` returns `None` for
        // a `NaN`, so the old `.unwrap()` here was a panic reachable from a hand-edited
        // sidecar — and `f32::clamp` does not remove a `NaN`, it passes it through.
        let mut pts: Vec<[f32; 2]> = points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        // Keeps the first of each run of equal `x`. Arbitrary, but total.
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < X_EPS);
        let m = tangents(&pts);
        Self { pts, m }
    }

    /// Evaluate at `x`; identity if fewer than 2 points, flat outside the point range.
    fn eval(&self, x: f32) -> f32 {
        let pts = &self.pts;
        if pts.len() < 2 {
            return x;
        }
        let last = pts.len() - 1;
        if x <= pts[0][0] {
            return pts[0][1];
        }
        if x >= pts[last][0] {
            return pts[last][1];
        }
        // The segment containing `x`. A linear scan: a curve has a handful of points, and the
        // LUT is built once per render rather than per pixel.
        let i = (0..last)
            .find(|&i| x >= pts[i][0] && x <= pts[i + 1][0])
            .unwrap_or(0);
        let h = pts[i + 1][0] - pts[i][0];
        let t = (x - pts[i][0]) / h;
        let (t2, t3) = (t * t, t * t * t);
        // Cubic Hermite basis.
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * pts[i][1] + h10 * h * self.m[i] + h01 * pts[i + 1][1] + h11 * h * self.m[i + 1]
    }
}

/// The Fritsch–Butland tangent at an end of the curve.
///
/// `h_near`/`d_near` describe the interval touching the endpoint, `h_far`/`d_far` the one beyond
/// it. The unclamped value is the slope of the parabola through the three points; the two guards
/// are what keep it inside the monotone region, and without them this formula can overshoot where
/// the naive one-sided slope could not.
fn end_tangent(h_near: f32, h_far: f32, d_near: f32, d_far: f32) -> f32 {
    let denom = h_near + h_far;
    if denom.abs() < X_EPS {
        return d_near;
    }
    let m = ((2.0 * h_near + h_far) * d_near - h_near * d_far) / denom;
    // ⚠ Pointing against its own secant would put an extremum in the first segment — the
    // contrast reversal the whole interpolant exists to prevent.
    if m * d_near <= 0.0 {
        return 0.0;
    }
    // ⚠ At a turning point the parabola can be far steeper than the data supports; 3× is the
    // standard bound, the same factor the interior clipping loop below enforces.
    if d_near * d_far <= 0.0 && m.abs() > 3.0 * d_near.abs() {
        return 3.0 * d_near;
    }
    m
}

/// Fritsch–Carlson tangents for `pts` (sorted, x-distinct).
fn tangents(pts: &[[f32; 2]]) -> Vec<f32> {
    let n = pts.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let d: Vec<f32> = (0..n - 1)
        .map(|i| (pts[i + 1][1] - pts[i][1]) / (pts[i + 1][0] - pts[i][0]))
        .collect();

    // Start from the average of the two adjacent secants.
    let mut m = vec![0.0f32; n];
    for i in 1..n - 1 {
        m[i] = 0.5 * (d[i - 1] + d[i]);
    }

    // ⚠⚠ **The endpoints use the three-point formula, not the one secant they have.** The naive
    // `m[0] = d[0]` is what `pchip` calls the one-sided slope, and on a tone curve it feels
    // wrong in a specific way, reported in use: with three points the curve then leaves (0,0)
    // along the *straight chord* to the dragged point, so the whole first segment goes flat and
    // every bit of curvature piles up at the point. Dragging one control moves the far end's
    // shape as much as its own neighbourhood — "wonky" was the word.
    //
    // This is the Fritsch–Butland endpoint, the one SciPy's `pchip` and MATLAB use: quadratic
    // through the first three points, so the slope accounts for the *second* interval and the
    // curve enters and leaves with the shape the points actually imply.
    //
    // ⚠ It needs three points. With exactly two, both tangents stay the single secant — which is
    // what keeps a two-point curve bit-for-bit the straight line the old piecewise-linear
    // implementation drew, including the identity.
    if n == 2 {
        m[0] = d[0];
        m[1] = d[0];
    } else {
        m[0] = end_tangent(pts[1][0] - pts[0][0], pts[2][0] - pts[1][0], d[0], d[1]);
        m[n - 1] = end_tangent(
            pts[n - 1][0] - pts[n - 2][0],
            pts[n - 2][0] - pts[n - 3][0],
            d[n - 2],
            d[n - 3],
        );
    }

    // ⚠ The clipping loop is the whole of the monotonicity guarantee — delete it and this is
    // an ordinary cubic that overshoots.
    for i in 0..n - 1 {
        if d[i].abs() < FLAT_EPS {
            // A flat segment. Both of its ends must be flat too, or the cubic bulges off the
            // plateau and the curve is no longer monotone across it.
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        // A tangent pointing *against* its secant puts a local extremum inside the segment —
        // the contrast reversal this interpolant exists to prevent.
        if m[i] / d[i] < 0.0 {
            m[i] = 0.0;
        }
        if m[i + 1] / d[i] < 0.0 {
            m[i + 1] = 0.0;
        }
        // And the monotonicity region itself: (α, β) must lie inside the circle of radius 3.
        let a = m[i] / d[i];
        let b = m[i + 1] / d[i];
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[i] = tau * a * d[i];
            m[i + 1] = tau * b * d[i];
        }
    }
    m
}

impl ToneCurveStage {
    /// Build from control points. Fewer than 2 points => identity.
    pub fn new(points: &[[f32; 2]]) -> Self {
        let spline = Spline::new(points);
        let lut = (0..LUT_SIZE)
            .map(|i| spline.eval(i as f32 / (LUT_SIZE - 1) as f32))
            .collect();
        Self { lut }
    }

    /// The curve's output for one input sample, straight out of the LUT.
    ///
    /// ⚠ Public so the panel's curve widget can *plot the function this stage applies* rather
    /// than re-deriving it — two implementations of one curve is how a preview stops being
    /// evidence about the render.
    pub fn sample(&self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        let pos = v * (LUT_SIZE - 1) as f32;
        let i = pos.floor() as usize;
        if i >= LUT_SIZE - 1 {
            return self.lut[LUT_SIZE - 1];
        }
        let frac = pos - i as f32;
        self.lut[i] + frac * (self.lut[i + 1] - self.lut[i])
    }
}

impl PipelineStage for ToneCurveStage {
    fn name(&self) -> &'static str {
        "tone_curve"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_samples(|s| self.sample(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_curve_passes_through() {
        let s = ToneCurveStage::new(&[[0.0, 0.0], [1.0, 1.0]]);
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((s.sample(v) - v).abs() < 2e-3);
        }
    }

    #[test]
    fn empty_points_is_identity() {
        let s = ToneCurveStage::new(&[]);
        assert!((s.sample(0.37) - 0.37).abs() < 2e-3);
    }

    #[test]
    fn a_single_point_is_still_the_identity() {
        // ⚠ A curve is *built* one point at a time, so "one point" is a state the widget can
        // legitimately be in. It must pass the image through untouched rather than flatten it
        // to that point's value.
        let s = ToneCurveStage::new(&[[0.5, 0.9]]);
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((s.sample(v) - v).abs() < 2e-3, "v={v} -> {}", s.sample(v));
        }
    }

    #[test]
    fn duplicate_x_values_collapse_rather_than_poisoning_the_curve() {
        // Two points at the same x give an infinite secant, and one `NaN` tangent spreads
        // through the whole LUT. The curve must stay usable.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.3], [0.5, 0.8], [1.0, 1.0]]);
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            assert!(s.sample(v).is_finite(), "v={v} -> {}", s.sample(v));
        }
    }

    #[test]
    fn a_non_finite_point_does_not_panic() {
        // Reachable from a hand-edited sidecar: the points are plain JSON.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [f32::NAN, 0.5], [1.0, 1.0]]);
        assert!((s.sample(0.5) - 0.5).abs() < 2e-3);
    }

    #[test]
    fn lift_midpoint_brightens_mids() {
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!((s.sample(0.5) - 0.7).abs() < 2e-3);
        assert!(s.sample(0.25) > 0.25);
    }

    #[test]
    fn two_points_reduce_to_the_straight_line() {
        // ⚠ The regression guard on the *previous* implementation: with two points the
        // Hermite tangents are both the single secant, so the cubic is exactly the line. Any
        // two-point curve anyone has already saved renders unchanged.
        let s = ToneCurveStage::new(&[[0.2, 0.1], [0.8, 0.9]]);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let x = 0.2 + t * 0.6;
            assert!((s.sample(x) - (0.1 + t * 0.8)).abs() < 2e-3, "x={x}");
        }
    }

    /// A knee sharp enough that an unconstrained cubic visibly overshoots it: nearly flat up
    /// to 0.4, then a near-vertical rise, then nearly flat again.
    fn knee() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [0.4, 0.05], [0.5, 0.9], [1.0, 1.0]]
    }

    #[test]
    fn a_sharp_knee_never_reverses_contrast() {
        // ⚠⚠ **The test the whole interpolant exists for.** A Catmull-Rom through `knee()`
        // dips *below* 0.05 approaching x=0.4 and rises *above* 0.9 after x=0.5 — brighter in,
        // darker out. Sampling densely is what catches it: at the control points themselves
        // every interpolant agrees, and the overshoot lives strictly between them.
        let s = ToneCurveStage::new(&knee());
        let mut prev = s.sample(0.0);
        for i in 1..=2000 {
            let x = i as f32 / 2000.0;
            let y = s.sample(x);
            assert!(
                y >= prev - 1e-4,
                "contrast reversal at x={x}: {prev} -> {y}",
            );
            prev = y;
        }
    }

    #[test]
    fn a_sharp_knee_stays_inside_its_control_points() {
        // The other half of monotonicity, and the one a "non-decreasing" check alone cannot
        // see: a curve can rise everywhere and still bulge past the value it is heading for.
        let pts = knee();
        let s = ToneCurveStage::new(&pts);
        for i in 0..=2000 {
            let x = i as f32 / 2000.0;
            // Bracketing control points for this x.
            let lo = pts.iter().rev().find(|p| p[0] <= x).unwrap();
            let hi = pts.iter().find(|p| p[0] >= x).unwrap();
            let (a, b) = (lo[1].min(hi[1]), lo[1].max(hi[1]));
            let y = s.sample(x);
            assert!(
                y >= a - 2e-3 && y <= b + 2e-3,
                "overshoot at x={x}: {y} outside [{a}, {b}]",
            );
        }
    }

    #[test]
    fn a_curve_is_smooth_where_the_old_one_had_a_facet() {
        // ⚠ The reason for the change, stated as a property: at a control point the two
        // segments must agree on their slope. The piecewise-linear predecessor did not — that
        // corner is the facet visible in a sky.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]]);
        let d = 4e-3;
        let at = |x: f32| (s.sample(x + d) - s.sample(x - d)) / (2.0 * d);
        for &x in &[0.25f32, 0.75] {
            let (before, after) = (at(x - 2.0 * d), at(x + 2.0 * d));
            assert!(
                (before - after).abs() < 0.25,
                "slope jumps at x={x}: {before} -> {after}",
            );
        }
    }

    #[test]
    fn a_flat_segment_stays_flat() {
        // A plateau between two equal control points must not bulge: the tangents at both of
        // its ends are forced to zero for exactly this.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.3, 0.6], [0.7, 0.6], [1.0, 1.0]]);
        for i in 0..=20 {
            let x = 0.3 + (i as f32 / 20.0) * 0.4;
            assert!((s.sample(x) - 0.6).abs() < 2e-3, "x={x} -> {}", s.sample(x));
        }
    }

    /// ⚠⚠ The fix for "the endpoints feel wonky", reported in use 2026-09-01.
    ///
    /// With the naive one-sided slope `m[0] = d[0]`, a three-point curve leaves (0,0) along the
    /// **straight chord** to the dragged point — so the first segment is dead straight and all
    /// the curvature piles up at the point. This asserts the endpoint tangent is *not* that
    /// chord, and that the curve genuinely bends in the first segment.
    #[test]
    fn the_endpoints_do_not_leave_along_the_bare_chord() {
        let pts = [[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]];
        let m = tangents(&pts);
        let chord = 0.75 / 0.5;
        assert!(
            (m[0] - chord).abs() > 0.1,
            "endpoint tangent {} is still the chord {chord}",
            m[0]
        );

        // The visible consequence: the first segment must bow away from its chord.
        let sp = Spline::new(&pts);
        let x = 0.25;
        let on_chord = chord * x;
        assert!(
            (sp.eval(x) - on_chord).abs() > 0.01,
            "the first segment is still straight: {} vs chord {on_chord}",
            sp.eval(x)
        );
    }

    /// ⚠ The three-point endpoint can overshoot where the one-sided slope could not, so its two
    /// guards are load-bearing. A curve that turns sharply at the end is where they bite.
    #[test]
    fn the_endpoint_formula_still_cannot_overshoot() {
        for pts in [
            vec![[0.0, 0.0], [0.1, 0.9], [1.0, 1.0]],
            vec![[0.0, 0.0], [0.9, 0.1], [1.0, 1.0]],
            vec![[0.0, 0.0], [0.05, 0.02], [0.95, 0.99], [1.0, 1.0]],
        ] {
            let sp = Spline::new(&pts);
            for i in 0..=1000 {
                let x = i as f32 / 1000.0;
                let y = sp.eval(x);
                assert!(
                    (-1e-5..=1.0 + 1e-5).contains(&y),
                    "overshoot at x={x}: y={y} for {pts:?}"
                );
            }
            // And monotone: a tone curve that reverses is a contrast reversal.
            let mut prev = sp.eval(0.0);
            for i in 1..=1000 {
                let y = sp.eval(i as f32 / 1000.0);
                assert!(y >= prev - 1e-5, "reversal at {i} for {pts:?}");
                prev = y;
            }
        }
    }

    /// ⚠ The property that keeps every existing two-point sidecar byte-identical: with two points
    /// Hermite must reduce **exactly** to the straight line, so the endpoint formula is skipped.
    #[test]
    fn two_points_still_reduce_to_the_exact_straight_line() {
        let sp = Spline::new(&[[0.0, 0.2], [1.0, 0.8]]);
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((sp.eval(x) - (0.2 + 0.6 * x)).abs() < 1e-6, "at x={x}");
        }
    }
}

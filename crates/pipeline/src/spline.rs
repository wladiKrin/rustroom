//! A **monotone** cubic interpolant, shared by every curve in the pipeline.
//!
//! ⚠ Extracted from `stages/tone_curve.rs` when the display rendering needed the same
//! interpolant. `tone_curve`'s own tests passed unchanged through the move, which is the
//! evidence it moved no behaviour.
//!
//! ⚠⚠ **Control points are clamped to `[0, 1]` in both axes.** A caller working in another
//! domain — log-exposure, say — must normalise into `[0, 1]` first, or every point collapses
//! onto the edges and the curve flattens to garbage. `DisplayRenderingStage` does exactly that.

/// Two control points closer together in `x` than this are one point.
///
/// ⚠ A zero-width span makes the secant slope infinite and every tangent derived from it
/// `NaN`, which then propagates through the whole LUT — one duplicated x would flatten the
/// entire curve to garbage rather than misbehaving locally.
const X_EPS: f32 = 1e-6;

/// A secant this shallow counts as flat. Below it the tangent clipping divides by ~0.
const FLAT_EPS: f32 = 1e-12;

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
pub(crate) struct Spline {
    pts: Vec<[f32; 2]>,
    /// Tangent at each control point, already clipped for monotonicity.
    m: Vec<f32>,
}

impl Spline {
    pub(crate) fn new(points: &[[f32; 2]]) -> Self {
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
    pub(crate) fn eval(&self, x: f32) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

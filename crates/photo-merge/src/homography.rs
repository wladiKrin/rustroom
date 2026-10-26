//! A plane-to-plane projective map in pixel coordinates — how a handheld frame sits on the
//! reference: a shift, a turn, a little perspective.
//!
//! Pixel centres are integers. ⚠ [`Homography::rescaled`] keeps that convention across sizes: a
//! half-size frame's pixel `i` covers full-size pixels `2i` and `2i + 1`, centred at `2i + 0.5`.

/// Row-major 3 × 3, mapping `(x, y, 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    }

    pub fn translation(dx: f64, dy: f64) -> Self {
        Self([1.0, 0.0, dx, 0.0, 1.0, dy, 0.0, 0.0, 1.0])
    }

    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }

    /// `self ∘ other`: `other` first, then `self`.
    pub fn compose(&self, other: &Self) -> Self {
        let (a, b) = (&self.0, &other.0);
        let mut m = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                m[3 * r + c] = (0..3).map(|k| a[3 * r + k] * b[3 * k + c]).sum();
            }
        }
        Self(m)
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if !det.is_finite() || det.abs() < 1e-15 {
            return None;
        }
        let adj = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        Some(Self(adj.map(|v| v / det)))
    }

    /// The same motion on images `s` times larger: `T · H · T⁻¹` with `T(x) = s·x + (s − 1)/2`.
    pub fn rescaled(&self, s: f64) -> Self {
        let o = (s - 1.0) / 2.0;
        let t = Self([s, 0.0, o, 0.0, s, o, 0.0, 0.0, 1.0]);
        let ti = Self([1.0 / s, 0.0, -o / s, 0.0, 1.0 / s, -o / s, 0.0, 0.0, 1.0]);
        t.compose(self).compose(&ti)
    }

    /// The largest distance between where `self` and `other` send the corners of a `w × h` frame.
    pub fn corner_shift(&self, other: &Self, w: usize, h: usize) -> f64 {
        let (w, h) = ((w as f64 - 1.0).max(0.0), (h as f64 - 1.0).max(0.0));
        [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .iter()
            .map(|&(x, y)| {
                let (a, b) = (self.apply(x, y), other.apply(x, y));
                ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
            })
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(deg: f64, dx: f64, dy: f64) -> Homography {
        let (s, c) = deg.to_radians().sin_cos();
        Homography([c, -s, dx, s, c, dy, 0.0, 0.0, 1.0])
    }

    #[test]
    fn compose_then_inverse_is_identity() {
        let a = Homography([1.01, 0.02, 3.0, -0.01, 0.99, -2.0, 1e-5, -2e-5, 1.0]);
        let id = a.compose(&a.inverse().unwrap());
        for (x, y) in [(0.0, 0.0), (1000.0, 20.0), (-50.0, 800.0)] {
            let (u, v) = id.apply(x, y);
            assert!((u - x).abs() < 1e-9 && (v - y).abs() < 1e-9, "({u}, {v})");
        }
    }

    /// `a.compose(b)` applies `b` first.
    #[test]
    fn compose_applies_the_right_hand_map_first() {
        let (a, b) = (turn(90.0, 0.0, 0.0), Homography::translation(10.0, 0.0));
        let (x, y) = a.compose(&b).apply(1.0, 0.0);
        assert!(
            (x - 0.0).abs() < 1e-9 && (y - 11.0).abs() < 1e-9,
            "({x}, {y})"
        );
    }

    /// The same motion at twice the size: a shift twice as far.
    #[test]
    fn rescaled_moves_a_point_twice_as_far_at_twice_the_size() {
        let h = Homography::translation(3.0, -1.5).rescaled(2.0);
        let (x, y) = h.apply(100.0, 100.0);
        assert!(
            (x - 106.0).abs() < 1e-9 && (y - 97.0).abs() < 1e-9,
            "({x}, {y})"
        );
        // And a turn about a point keeps its angle.
        let r = turn(1.0, 0.0, 0.0).rescaled(2.0);
        let (x0, y0) = r.apply(0.5, 0.5);
        let (x1, y1) = r.apply(1000.5, 0.5);
        let angle = ((y1 - y0) / (x1 - x0)).atan().to_degrees();
        assert!((angle - 1.0).abs() < 1e-6, "{angle}");
    }

    #[test]
    fn the_corner_shift_is_the_largest_corner_distance() {
        let a = Homography::identity();
        let b = Homography::translation(3.0, 4.0);
        assert!((a.corner_shift(&b, 100, 50) - 5.0).abs() < 1e-9);
        assert_eq!(a.corner_shift(&a, 100, 50), 0.0);
    }
}

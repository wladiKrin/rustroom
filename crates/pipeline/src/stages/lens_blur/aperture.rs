//! The aperture (M10.16d): the four bokeh shapes the gather tests each disc against.
//!
//! Every shape is defined in **unit-disc units**: `o` is an offset divided by the disc's radius, so
//! `|o| ≤ 1` is the disc a circular aperture would make.

use edit_model::Bokeh;

/// The aperture's weight at `o` (unit-disc units) for a disc centred at frame-normalised `pos`.
///
/// - **Circle** — `|o| ≤ 1`.
/// - **Blades** — a six-bladed aperture's hexagon, vertices on the x-axis, inscribed in the disc
///   (apothem cos 30°).
/// - **Ring** — a mirror lens's donut: the disc weighted `0.35` at its centre rising to `1` at its rim.
/// - **Cat-eye** — a fast lens's optical vignetting: the intersection of two unit discs offset by
///   `±shift / 2`, `shift = 0.6 · (2·pos − 1)` — round at the frame's centre, narrowing radially toward
///   its edges. ⚠ Symmetric about `o = 0` on purpose, so each disc stays centred on its own pixel.
pub fn weight(kind: Bokeh, o: [f32; 2], pos: [f32; 2]) -> f32 {
    let r = o[0].hypot(o[1]);
    match kind {
        Bokeh::Circle => inside(r <= 1.0),
        Bokeh::Blades => inside(
            blade_edges()
                .iter()
                .all(|&(s, c)| (o[0] * c + o[1] * s).abs() <= apothem()),
        ),
        Bokeh::Ring => {
            if r > 1.0 {
                0.0
            } else {
                let t = ((r - 0.55) / 0.4).clamp(0.0, 1.0);
                0.35 + 0.65 * t * t * (3.0 - 2.0 * t)
            }
        }
        Bokeh::CatEye => {
            let half = cat_eye_shift(pos).map(|v| v / 2.0);
            let a = (o[0] - half[0]).hypot(o[1] - half[1]);
            let b = (o[0] + half[0]).hypot(o[1] + half[1]);
            inside(a <= 1.0 && b <= 1.0)
        }
    }
}

/// The hexagon's three edge normals as `(sin, cos)` of 30°, 90° and 150° — exactly as computed here,
/// so the GPU twin's literals are the same bits (M10.16d2).
pub fn blade_edges() -> [(f32, f32); 3] {
    [30f32, 90.0, 150.0].map(|deg| deg.to_radians().sin_cos())
}

/// The hexagon's apothem, `cos 30°`.
pub fn apothem() -> f32 {
    (std::f32::consts::PI / 6.0).cos()
}

fn inside(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// The cat-eye's offset between its two discs: `0.6 ×` the point's offset from the frame's centre, the
/// frame's half-extent being 1.
fn cat_eye_shift(pos: [f32; 2]) -> [f32; 2] {
    pos.map(|v| 0.6 * (2.0 * v - 1.0))
}

/// The shape's integral over the plane in unit-disc units — π for the circle — at `pos`: what a
/// highlight's energy is spread over.
pub fn area(kind: Bokeh, pos: [f32; 2]) -> f32 {
    match kind {
        Bokeh::Circle => std::f32::consts::PI,
        // A regular hexagon of circumradius 1.
        Bokeh::Blades => 1.5 * 3f32.sqrt(),
        // ⚠ `2π ∫₀¹ w(r) r dr` for the ring's weight, integrated once — no closed form worth keeping
        // in step with the smoothstep by hand.
        Bokeh::Ring => {
            static RING: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
            *RING.get_or_init(|| {
                let n = 20_000;
                let dr = 1.0 / n as f64;
                let sum: f64 = (0..n)
                    .map(|i| {
                        let r = (i as f64 + 0.5) * dr;
                        weight(Bokeh::Ring, [r as f32, 0.0], [0.5, 0.5]) as f64 * r * dr
                    })
                    .sum();
                (2.0 * std::f64::consts::PI * sum) as f32
            })
        }
        // The lens where two unit discs `d` apart overlap.
        Bokeh::CatEye => {
            let s = cat_eye_shift(pos);
            let d = s[0].hypot(s[1]).min(2.0);
            2.0 * (d / 2.0).acos() - (d / 2.0) * (4.0 - d * d).sqrt()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Bokeh; 4] = [Bokeh::Circle, Bokeh::Blades, Bokeh::Ring, Bokeh::CatEye];

    #[test]
    fn a_circle_covers_the_unit_disc_only() {
        let c = |o| weight(Bokeh::Circle, o, [0.5, 0.5]);
        assert_eq!(c([0.0, 0.0]), 1.0);
        assert_eq!(c([0.99, 0.0]), 1.0);
        assert_eq!(c([0.0, -1.01]), 0.0);
        assert_eq!(c([0.72, 0.72]), 0.0, "|o| = 1.018");
    }

    /// A hexagon with its vertices on the x-axis: circumradius 1, apothem cos 30° ≈ 0.866.
    #[test]
    fn blades_make_a_hexagon() {
        let b = |o| weight(Bokeh::Blades, o, [0.5, 0.5]);
        assert_eq!(b([0.9, 0.0]), 1.0, "toward a vertex");
        let (s, c) = 30f32.to_radians().sin_cos();
        assert_eq!(b([0.9 * c, 0.9 * s]), 0.0, "toward an edge's middle");
        assert_eq!(b([0.8 * c, 0.8 * s]), 1.0);
    }

    #[test]
    fn a_ring_is_brighter_at_its_rim() {
        let r = |o| weight(Bokeh::Ring, o, [0.5, 0.5]);
        assert!(r([0.0, 0.0]) < 0.4);
        assert!(r([0.95, 0.0]) > 0.95);
        assert_eq!(r([1.05, 0.0]), 0.0);
    }

    /// Round at the frame's centre; clipped along the radial axis toward an edge, not across it.
    #[test]
    fn a_cat_eye_is_round_at_the_centre_and_clipped_toward_the_edges() {
        for o in [[0.95, 0.0], [0.0, 0.95], [-0.6, 0.6]] {
            assert_eq!(
                weight(Bokeh::CatEye, o, [0.5, 0.5]),
                weight(Bokeh::Circle, o, [0.5, 0.5])
            );
        }
        let edge = [0.95, 0.5];
        assert_eq!(
            weight(Bokeh::CatEye, [0.9, 0.0], edge),
            0.0,
            "radially clipped"
        );
        assert_eq!(
            weight(Bokeh::CatEye, [-0.9, 0.0], edge),
            0.0,
            "on both sides"
        );
        assert_eq!(weight(Bokeh::CatEye, [0.0, 0.9], edge), 1.0, "not across");
        assert!(area(Bokeh::CatEye, edge) < area(Bokeh::CatEye, [0.5, 0.5]));
    }

    /// `area` is the shape's integral — checked against a 400 × 400 numeric one, at the centre and at
    /// a corner, for every shape.
    #[test]
    fn area_matches_the_numeric_integral() {
        for kind in ALL {
            for pos in [[0.5, 0.5], [0.05, 0.92]] {
                let n = 400;
                let step = 2.4 / n as f32;
                let mut sum = 0.0f64;
                for j in 0..n {
                    for i in 0..n {
                        let o = [
                            -1.2 + (i as f32 + 0.5) * step,
                            -1.2 + (j as f32 + 0.5) * step,
                        ];
                        sum += weight(kind, o, pos) as f64;
                    }
                }
                let numeric = sum * (step as f64).powi(2);
                let a = area(kind, pos) as f64;
                assert!(
                    (a - numeric).abs() / numeric < 0.01,
                    "{kind:?} at {pos:?}: {a} vs {numeric}"
                );
            }
        }
    }
}

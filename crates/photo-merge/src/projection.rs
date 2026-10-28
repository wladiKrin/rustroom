//! Projections and the canvas (M10.17b): how the sphere of directions a panorama covers is laid flat.
//!
//! All three are expressed in one basis, centred on the reference frame: `Z` its optical axis (made
//! perpendicular to the sweep's axis), `R` to the right, `D = Z × R` down — for a horizontal sweep `D` is the
//! world's down (the rig is levelled), for a vertical one `R` is the world's horizontal sweep axis. A
//! direction's coordinates `(a, b, c) = (d·R, d·D, d·Z)`:
//! - **Cylindrical** — the cylinder's axis along the sweep's: verticals stay vertical, the horizon straight.
//! - **Spherical** — angles along and across: very wide or tall sweeps; bends straight lines more.
//! - **Perspective** — the plane `c = 1`: straight lines stay straight, the edges stretch; narrow sweeps only.

use crate::rig::{Rig, Sweep};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Projection {
    Spherical,
    Cylindrical,
    Perspective,
}

impl Projection {
    pub fn label(self) -> &'static str {
        match self {
            Projection::Spherical => "Spherical",
            Projection::Cylindrical => "Cylindrical",
            Projection::Perspective => "Perspective",
        }
    }
}

/// Past this total angle along the sweep Perspective is refused: its edges would stretch without bound.
pub const PERSPECTIVE_LIMIT_DEG: f64 = 120.0;
/// Auto select picks Perspective under this angle along the sweep…
pub const AUTO_PERSPECTIVE_DEG: f64 = 70.0;
/// …and Spherical when the angle across it exceeds this; otherwise Cylindrical. Both the author's to tune.
pub const AUTO_SPHERICAL_CROSS_DEG: f64 = 70.0;
/// The panorama's long edge at most (the author's cap, 2026-10-27).
pub const MAX_LONG_EDGE: usize = 16_000;

/// The projection asked for cannot hold this sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooWide;

/// The sweep's total angle along it and across it, degrees, from every frame's outline.
pub fn extent(rig: &Rig) -> (f64, f64) {
    let basis = basis_of(rig);
    let (mut along, mut across) = ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN));
    for d in outline(rig) {
        let (u, v) = angles(&basis, rig.sweep, d);
        along = (along.0.min(u), along.1.max(u));
        across = (across.0.min(v), across.1.max(v));
    }
    (
        (along.1 - along.0).to_degrees(),
        (across.1 - across.0).to_degrees(),
    )
}

/// The angle along the sweep and the angle across it, radians, of world direction `d`.
fn angles(basis: &[[f64; 3]; 3], sweep: Sweep, d: [f64; 3]) -> (f64, f64) {
    let [a, b, c] = coords(basis, d);
    match sweep {
        Sweep::Horizontal => (a.atan2(c), b.atan2(a.hypot(c))),
        Sweep::Vertical => (b.atan2(c), a.atan2(b.hypot(c))),
    }
}

/// `d` in the basis: `(d·R, d·D, d·Z)`.
fn coords(basis: &[[f64; 3]; 3], d: [f64; 3]) -> [f64; 3] {
    basis.map(|e| e[0] * d[0] + e[1] * d[1] + e[2] * d[2])
}

fn normalised(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / n)
}

/// `[R, D, Z]`, centred on the reference (the module's note).
fn basis_of(rig: &Rig) -> [[f64; 3]; 3] {
    let z = rig.rotations[rig.reference].row(2);
    match rig.sweep {
        Sweep::Horizontal => {
            let d = [0.0, 1.0, 0.0];
            let z = normalised([z[0], 0.0, z[2]]);
            [crate::rotation::cross(d, z), d, z]
        }
        Sweep::Vertical => {
            let r = [1.0, 0.0, 0.0];
            let z = normalised([0.0, z[1], z[2]]);
            [r, crate::rotation::cross(z, r), z]
        }
    }
}

/// World directions along every frame's four edges.
fn outline(rig: &Rig) -> Vec<[f64; 3]> {
    let (w, h) = (rig.size.0 as f64 - 1.0, rig.size.1 as f64 - 1.0);
    let n = 64;
    let mut out = Vec::new();
    for f in 0..rig.rotations.len() {
        for k in 0..=n {
            let t = k as f64 / n as f64;
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                out.push(rig.ray(f, x, y));
            }
        }
    }
    out
}

/// Auto select's pick for `rig` (the module's constants).
pub fn auto(rig: &Rig) -> Projection {
    let (along, across) = extent(rig);
    if along < AUTO_PERSPECTIVE_DEG && across < PERSPECTIVE_LIMIT_DEG {
        Projection::Perspective
    } else if across > AUTO_SPHERICAL_CROSS_DEG {
        Projection::Spherical
    } else {
        Projection::Cylindrical
    }
}

/// A projection laid on pixels: canvas pixel `(u, v)` ↔ projection coordinates
/// `(origin.0 + u / px_per_rad, origin.1 + v / px_per_rad)` (radians, or tangents for Perspective).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canvas {
    pub projection: Projection,
    pub width: usize,
    pub height: usize,
    pub px_per_rad: f64,
    pub origin: (f64, f64),
    pub axis: Sweep,
    basis: [[f64; 3]; 3],
}

impl Canvas {
    /// The canvas holding every frame of `rig` under `p`, at the rig's own scale (one pixel per
    /// `1 / focal_px` radians) or lower so its long edge is at most `max_long_edge`.
    pub fn plan(rig: &Rig, p: Projection, max_long_edge: usize) -> Result<Canvas, TooWide> {
        if p == Projection::Perspective && extent(rig).0 > PERSPECTIVE_LIMIT_DEG {
            return Err(TooWide);
        }
        let mut c = Canvas {
            projection: p,
            width: 1,
            height: 1,
            px_per_rad: 1.0,
            origin: (0.0, 0.0),
            axis: rig.sweep,
            basis: basis_of(rig),
        };
        let (mut x, mut y) = ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN));
        for d in outline(rig) {
            let (u, v) = c.project(d).ok_or(TooWide)?;
            x = (x.0.min(u), x.1.max(u));
            y = (y.0.min(v), y.1.max(v));
        }
        let (rx, ry) = (x.1 - x.0, y.1 - y.0);
        let mut s = rig.focal_px;
        let long = rx.max(ry) * s;
        if long + 1.0 > max_long_edge as f64 {
            s *= (max_long_edge as f64 - 1.0) / long;
        }
        c.px_per_rad = s;
        c.origin = (x.0, y.0);
        c.width = ((rx * s).ceil() as usize + 1).min(max_long_edge);
        c.height = ((ry * s).ceil() as usize + 1).min(max_long_edge);
        Ok(c)
    }

    /// Projection coordinates of world direction `d`, before the canvas's scale and origin.
    fn project(&self, d: [f64; 3]) -> Option<(f64, f64)> {
        let [a, b, c] = coords(&self.basis, d);
        Some(match (self.projection, self.axis) {
            (Projection::Perspective, _) => {
                if c <= 1e-9 {
                    return None;
                }
                (a / c, b / c)
            }
            (Projection::Cylindrical, Sweep::Horizontal) => (a.atan2(c), b / a.hypot(c)),
            (Projection::Spherical, Sweep::Horizontal) => (a.atan2(c), b.atan2(a.hypot(c))),
            (Projection::Cylindrical, Sweep::Vertical) => (a / b.hypot(c), b.atan2(c)),
            (Projection::Spherical, Sweep::Vertical) => (a.atan2(b.hypot(c)), b.atan2(c)),
        })
    }

    /// The world direction (unit) of canvas pixel `(u, v)`; `None` off the projection's domain.
    pub fn ray(&self, u: f64, v: f64) -> Option<[f64; 3]> {
        let (x, y) = (
            self.origin.0 + u / self.px_per_rad,
            self.origin.1 + v / self.px_per_rad,
        );
        let [r, d, z] = self.basis;
        let (a, b, c) = match (self.projection, self.axis) {
            (Projection::Perspective, _) => (x, y, 1.0),
            (Projection::Cylindrical, Sweep::Horizontal) => (x.sin(), y, x.cos()),
            (Projection::Spherical, Sweep::Horizontal) => {
                (y.cos() * x.sin(), y.sin(), y.cos() * x.cos())
            }
            (Projection::Cylindrical, Sweep::Vertical) => (x, y.sin(), y.cos()),
            (Projection::Spherical, Sweep::Vertical) => {
                (x.sin(), x.cos() * y.sin(), x.cos() * y.cos())
            }
        };
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            return None;
        }
        Some(normalised(
            [0, 1, 2].map(|i| a * r[i] + b * d[i] + c * z[i]),
        ))
    }

    /// Where world direction `ray` lands on the canvas (possibly off it); `None` where the projection
    /// cannot show it (behind a Perspective plane).
    pub fn point(&self, ray: [f64; 3]) -> Option<(f64, f64)> {
        let (x, y) = self.project(ray)?;
        Some((
            (x - self.origin.0) * self.px_per_rad,
            (y - self.origin.1) * self.px_per_rad,
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rotation::Rot;
    use crate::synth_pano::{self, Shot};

    /// A rig straight from synthetic shots — no fitting, the truth itself.
    pub(crate) fn rig_from(shots: &[Shot], size: (usize, usize), focal: f64, sweep: Sweep) -> Rig {
        let rotations = shots
            .iter()
            .map(|s| {
                let m = s.rotation();
                Rot([
                    m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
                ])
            })
            .collect();
        Rig {
            focal_px: focal,
            size,
            rotations,
            order: (0..shots.len()).collect(),
            reference: (shots.len() - 1) / 2,
            sweep,
        }
    }

    fn level_row(n: usize, overlap: f32, portrait: bool) -> synth_pano::Row {
        let mut r = synth_pano::row(n, overlap, portrait);
        for s in r.shots.iter_mut() {
            (s.pitch, s.roll) = (0.0, 0.0);
        }
        r
    }

    #[test]
    fn ray_and_point_are_inverse() {
        let r = level_row(3, 0.33, false);
        for sweep in [Sweep::Horizontal, Sweep::Vertical] {
            let mut shots = r.shots.clone();
            if sweep == Sweep::Vertical {
                for (i, s) in shots.iter_mut().enumerate() {
                    (s.yaw, s.pitch) = (0.0, (i as f64 - 1.0) * 0.5);
                }
            }
            // A long lens (900 px, ~30° a frame), so Perspective holds the sweep too.
            let rig = rig_from(&shots, (r.w, r.h), 900.0, sweep);
            for p in [
                Projection::Spherical,
                Projection::Cylindrical,
                Projection::Perspective,
            ] {
                let c = Canvas::plan(&rig, p, MAX_LONG_EDGE).expect("fits");
                for (u, v) in [
                    (10.5, 20.25),
                    (c.width as f64 / 2.0, c.height as f64 / 3.0),
                    (c.width as f64 - 3.0, 5.0),
                ] {
                    let d = c.ray(u, v).expect("on the domain");
                    let (pu, pv) = c.point(d).expect("shown");
                    assert!(
                        (pu - u).abs() < 1e-6 && (pv - v).abs() < 1e-6,
                        "{p:?} {sweep:?}: ({u}, {v}) → ({pu}, {pv})"
                    );
                }
            }
        }
    }

    /// Every frame's centre lands inside the canvas, and the canvas is at the rig's own scale.
    #[test]
    fn the_canvas_holds_every_frame() {
        let r = level_row(4, 0.33, false);
        let rig = rig_from(&r.shots, (r.w, r.h), r.focal, Sweep::Horizontal);
        let c = Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).unwrap();
        assert!((c.px_per_rad - r.focal).abs() < 1e-9);
        for i in 0..4 {
            let (u, v) = c
                .point(rig.ray(i, (r.w as f64 - 1.0) / 2.0, (r.h as f64 - 1.0) / 2.0))
                .unwrap();
            assert!(
                (0.0..c.width as f64).contains(&u) && (0.0..c.height as f64).contains(&v),
                "frame {i}: ({u}, {v})"
            );
        }
        // Four frames each ~1.04 rad wide, a third shared: ~3.1 rad of canvas at 420 px a radian.
        assert!((1250..1400).contains(&c.width), "{}", c.width);
    }

    /// A pole of the synthetic scene — a vertical of the world — through the rig the frames themselves
    /// gave (fitted and levelled), onto the cylinder: its x does not drift down its height.
    #[test]
    fn cylindrical_keeps_a_vertical_vertical() {
        let mut r = synth_pano::row(5, 0.33, false);
        for s in r.shots.iter_mut() {
            s.roll = 0.0;
        }
        r.shots[2].pitch = (-5f64).to_radians();
        let f = synth_pano::render(&synth_pano::Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let rig = crate::rig::tests::rig_of(&f, r.focal, (r.w, r.h));
        let c = Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).unwrap();
        // The truth's world → the rig's, through the reference camera.
        let m = r.shots[rig.reference].rotation();
        let truth = Rot([
            m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
        ]);
        let to_rig = rig.rotations[rig.reference].transpose().compose(&truth);
        let xs: Vec<f64> = (0..20)
            .map(|k| {
                let lat = -0.3 + 0.012 * k as f64;
                let lon: f64 = 0.35;
                let d = [lat.cos() * lon.sin(), -lat.sin(), lat.cos() * lon.cos()];
                c.point(to_rig.apply(d)).unwrap().0
            })
            .collect();
        let spread = xs.iter().cloned().fold(f64::MIN, f64::max)
            - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 0.5, "the pole drifts {spread} px");
    }

    #[test]
    fn perspective_keeps_a_line_straight() {
        let r = level_row(2, 0.5, false);
        let rig = rig_from(&r.shots, (r.w, r.h), r.focal, Sweep::Horizontal);
        let c = Canvas::plan(&rig, Projection::Perspective, MAX_LONG_EDGE).unwrap();
        // A straight line in space, seen from the lens: directions toward P0 + t·V.
        let pts: Vec<(f64, f64)> = (0..15)
            .map(|k| {
                let t = -1.0 + k as f64 / 7.0;
                c.point([0.2 + 0.9 * t, 0.3 - 0.2 * t, 2.0]).unwrap()
            })
            .collect();
        let (a, b) = (pts[0], pts[14]);
        let worst = pts
            .iter()
            .map(|p| {
                ((b.0 - a.0) * (a.1 - p.1) - (a.0 - p.0) * (b.1 - a.1)).abs()
                    / (b.0 - a.0).hypot(b.1 - a.1)
            })
            .fold(0f64, f64::max);
        assert!(worst < 1.0, "{worst} px off the line");
    }

    #[test]
    fn auto_picks_by_the_angles() {
        // Two frames of a long lens (900 px: ~30° each), half shared: ~45° in all.
        let narrow = level_row(2, 0.5, false);
        let shots: Vec<Shot> = narrow
            .shots
            .iter()
            .map(|s| Shot {
                yaw: s.yaw * narrow.focal / 900.0,
                ..*s
            })
            .collect();
        let rig = rig_from(&shots, (narrow.w, narrow.h), 900.0, Sweep::Horizontal);
        assert!(extent(&rig).0 < AUTO_PERSPECTIVE_DEG, "{:?}", extent(&rig));
        assert_eq!(auto(&rig), Projection::Perspective);
        let wide = level_row(5, 0.33, false);
        let rig = rig_from(&wide.shots, (wide.w, wide.h), wide.focal, Sweep::Horizontal);
        assert_eq!(auto(&rig), Projection::Cylindrical);
        // Portrait frames at a short focal length: 2·atan(240 / 250) ≈ 88° across the sweep.
        let tall = level_row(4, 0.33, true);
        let rig = rig_from(&tall.shots, (tall.w, tall.h), 250.0, Sweep::Horizontal);
        assert!(
            extent(&rig).1 > AUTO_SPHERICAL_CROSS_DEG,
            "{:?}",
            extent(&rig)
        );
        assert_eq!(auto(&rig), Projection::Spherical);
    }

    #[test]
    fn the_canvas_is_capped_at_sixteen_thousand() {
        // Six portrait X-T4 frames at 23 mm (6107 px), 30 % shared: ~18,000 px wide at their own scale.
        let r = level_row(6, 0.3, true);
        let (w, h, f) = (4160usize, 6240usize, 6107.0);
        let hfov = 2.0 * ((w as f64 / 2.0) / f).atan();
        let shots: Vec<Shot> = r
            .shots
            .iter()
            .enumerate()
            .map(|(i, s)| Shot {
                yaw: (i as f64 - 2.5) * hfov * 0.7,
                ..*s
            })
            .collect();
        let rig = rig_from(&shots, (w, h), f, Sweep::Horizontal);
        let natural = Canvas::plan(&rig, Projection::Cylindrical, usize::MAX).unwrap();
        assert!(
            natural.width > 16_000,
            "the fixture must exceed the cap ({})",
            natural.width
        );
        let c = Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).unwrap();
        assert!(
            c.width <= MAX_LONG_EDGE && c.height <= MAX_LONG_EDGE,
            "{} × {}",
            c.width,
            c.height
        );
        let aspect = |c: &Canvas| c.width as f64 / c.height as f64;
        assert!((aspect(&c) / aspect(&natural) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn perspective_past_its_limit_is_refused() {
        let r = level_row(6, 0.33, false);
        let rig = rig_from(&r.shots, (r.w, r.h), r.focal, Sweep::Horizontal);
        assert!(extent(&rig).0 > PERSPECTIVE_LIMIT_DEG);
        assert_eq!(
            Canvas::plan(&rig, Projection::Perspective, MAX_LONG_EDGE),
            Err(TooWide)
        );
        assert!(Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).is_ok());
    }
}

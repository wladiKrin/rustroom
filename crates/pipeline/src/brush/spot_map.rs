//! The forward map from **spot** coordinates into frame pixels — `FrameMap` with the keystone in
//! front of it.
//!
//! ⚠⚠ **Why spots need their own map (M7.3b).** Brushes are anchored in post-perspective,
//! pre-geometry space, and `FrameMap` maps that. Spots moved one step earlier, to post-lens,
//! **pre-perspective** space, when perspective was fused into `GeometryStage`: the buffer spot
//! removal used to run on no longer exists. So a spot needs the homography's forward map before
//! `FrameMap`'s, and a canvas click needs its inverse after.
//!
//! ⚠ With the perspective inactive this is `FrameMap` exactly — the same calls, no extra
//! arithmetic — so a photo without a keystone places and draws spots as it always did.

use super::FrameMap;
use crate::perspective::Homography;
use edit_model::{Geometry, Perspective};

/// Below this a side is treated as degenerate; it only guards a division. `FrameMap`'s value.
const EPS: f32 = 1e-6;

/// Maps a spot's normalised coordinates to **frame** pixels, and back.
pub struct SpotMap {
    frame: FrameMap,
    /// `None` for an inactive perspective, so that case is `FrameMap` bit for bit.
    hom: Option<Homography>,
    /// Width over height of the spot space: the pre-orientation, pre-crop frame.
    aspect: f32,
    /// One long-edge unit, as a step in `u` and in `v` — how far a spot radius reaches along each
    /// normalised axis.
    long_u: f32,
    long_v: f32,
    /// `FrameMap::radius_scale`: frame pixels per long-edge unit, **ignoring** the keystone. The
    /// grab threshold uses it; drawing uses [`Self::radius_scale_at`].
    pub radius_scale: f32,
}

impl SpotMap {
    pub fn new(g: &Geometry, p: Perspective, frame_w: usize, frame_h: usize) -> Self {
        let frame = FrameMap::new(g, frame_w, frame_h);
        // The spot space is the pre-orientation, pre-crop frame: undo the crop, then the axis
        // swap a quarter turn makes. ⚠ Derived exactly as `FrameMap::new` derives its own.
        let cw = (g.crop.right - g.crop.left).max(EPS);
        let ch = (g.crop.bottom - g.crop.top).max(EPS);
        let (ow, oh) = (frame_w as f32 / cw, frame_h as f32 / ch);
        let (sw, sh) = if g.orientation.rotate.swaps_axes() {
            (oh, ow)
        } else {
            (ow, oh)
        };
        let long = sw.max(sh);
        let aspect = sw / sh.max(EPS);
        Self {
            radius_scale: frame.radius_scale,
            frame,
            // ⚠⚠ The same `from_edits` at the same aspect as `GeometryStage`'s fused path, so the
            // autoscale (fit/fill) is the one the render used.
            hom: p.is_active().then(|| Homography::from_edits(&p, aspect)),
            aspect,
            long_u: long / sw.max(EPS),
            long_v: long / sh.max(EPS),
        }
    }

    /// `[0, 1]` normalised → the homography's space: centred, half-height = 1.
    fn hom_of(&self, q: [f32; 2]) -> (f32, f32) {
        ((q[0] - 0.5) * 2.0 * self.aspect, (q[1] - 0.5) * 2.0)
    }

    /// The inverse of [`Self::hom_of`].
    fn unit_of(&self, n: (f32, f32)) -> [f32; 2] {
        [n.0 / (2.0 * self.aspect) + 0.5, n.1 / 2.0 + 0.5]
    }

    /// `(x, y)` in frame pixels. May fall outside the frame — the caller clips.
    pub fn point(&self, p: [f32; 2]) -> (f32, f32) {
        let q = match &self.hom {
            Some(h) => self.unit_of(h.output_of(self.hom_of(p))),
            None => p,
        };
        self.frame.point(q)
    }

    /// The inverse of [`Self::point`]: frame pixels back to spot coordinates.
    pub fn inverse_point(&self, x: f32, y: f32) -> [f32; 2] {
        let q = self.frame.inverse_point(x, y);
        match &self.hom {
            Some(h) => self.unit_of(h.source_of(self.hom_of(q))),
            None => q,
        }
    }

    /// Frame pixels per long-edge unit **at `p`** — the local scale of the whole map.
    ///
    /// ⚠ A keystone magnifies one end of the frame and shrinks the other, so a spot's drawn circle
    /// must be sized where it sits. The square root of the Jacobian's determinant is the scale of
    /// an area; the circle is drawn round, which is honest to within the keystone's anisotropy
    /// across one spot. With no perspective this is exactly [`Self::radius_scale`].
    pub fn radius_scale_at(&self, p: [f32; 2]) -> f32 {
        if self.hom.is_none() {
            return self.radius_scale;
        }
        const E: f32 = 1e-3;
        let a = self.point(p);
        let b = self.point([p[0] + E * self.long_u, p[1]]);
        let c = self.point([p[0], p[1] + E * self.long_v]);
        let det = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        det.abs().sqrt() / E
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::geometry::GeometryStage;
    use crate::PipelineStage;
    use core_image::ImageBuf;
    use edit_model::{CropRect, Orientation, Quarter};

    fn persp(v: f32) -> Perspective {
        Perspective {
            vertical: v,
            scale: 0.0,
            ..Default::default()
        }
    }

    fn geo(orientation: Orientation, angle: f32, crop: CropRect) -> Geometry {
        Geometry {
            orientation,
            angle,
            crop,
        }
    }

    fn cropped() -> CropRect {
        CropRect {
            left: 0.1,
            top: 0.05,
            right: 0.95,
            bottom: 0.9,
        }
    }

    #[test]
    fn without_perspective_it_is_the_frame_map() {
        let g = geo(Orientation::default(), 3.0, cropped());
        let s = SpotMap::new(&g, Perspective::default(), 170, 110);
        let f = FrameMap::new(&g, 170, 110);
        for p in [[0.2, 0.3], [0.5, 0.5], [0.8, 0.7]] {
            assert_eq!(s.point(p), f.point(p));
            let (x, y) = f.point(p);
            assert_eq!(s.inverse_point(x, y), f.inverse_point(x, y));
            assert_eq!(s.radius_scale_at(p), f.radius_scale);
        }
    }

    #[test]
    fn it_round_trips() {
        for o in [
            Orientation::default(),
            Orientation {
                rotate: Quarter::Cw90,
                mirror: true,
            },
        ] {
            let s = SpotMap::new(&geo(o, 3.0, cropped()), persp(40.0), 170, 110);
            for i in 0..5 {
                for j in 0..5 {
                    let p = [0.1 + 0.2 * i as f32, 0.1 + 0.2 * j as f32];
                    let (x, y) = s.point(p);
                    let q = s.inverse_point(x, y);
                    assert!(
                        (q[0] - p[0]).abs() < 1e-4 && (q[1] - p[1]).abs() < 1e-4,
                        "{o:?}: {p:?} came back as {q:?}"
                    );
                }
            }
        }
    }

    /// ⚠⚠ **Checked against the real stage, not a re-derivation** — `FrameMap`'s own rule. A
    /// forward and an inverse wrong in mirrored ways round-trip perfectly, so the round trip above
    /// cannot stand alone.
    #[test]
    fn a_dot_lands_where_the_map_says() {
        let (w, h) = (160usize, 120usize);
        for (o, (sx, sy)) in [
            (Orientation::default(), (60usize, 42usize)),
            (
                Orientation {
                    rotate: Quarter::Cw90,
                    mirror: false,
                },
                (100, 75),
            ),
        ] {
            let g = geo(o, 4.0, cropped());
            let p = persp(45.0);
            let mut img = ImageBuf::new(w, h);
            img.set_pixel(sx, sy, [1.0, 1.0, 1.0]);
            // ⚠ Bilinear: the brightest-pixel search needs a kernel without negative lobes.
            let out = GeometryStage::new(&g, crate::Resampler::Bilinear)
                .with_perspective(p)
                .apply(img);
            let (mut best, mut bx, mut by) = (0.0f32, 0usize, 0usize);
            for y in 0..out.height() {
                for x in 0..out.width() {
                    let v = out.pixel(x, y)[0];
                    if v > best {
                        best = v;
                        bx = x;
                        by = y;
                    }
                }
            }
            assert!(best > 0.05, "{o:?}: the marked pixel was lost");
            let at = [(sx as f32 + 0.5) / w as f32, (sy as f32 + 0.5) / h as f32];
            let map = SpotMap::new(&g, p, out.width(), out.height());
            let (mx, my) = map.point(at);
            // ⚠ The fixture must discriminate: without the keystone the map would be elsewhere.
            let (fx, fy) = FrameMap::new(&g, out.width(), out.height()).point(at);
            assert!(
                (fx - mx).abs() + (fy - my).abs() > 3.0,
                "{o:?}: fixture check: the keystone must move the dot"
            );
            assert!(
                (mx - bx as f32).abs() < 1.5 && (my - by as f32).abs() < 1.5,
                "{o:?}: map says ({mx:.2}, {my:.2}), the stage put it at ({bx}, {by})"
            );
        }
    }

    #[test]
    fn the_local_scale_follows_the_keystone() {
        let s = SpotMap::new(
            &Geometry::default(),
            Perspective {
                vertical: 60.0,
                scale: 0.0,
                ..Default::default()
            },
            200,
            150,
        );
        let top = s.radius_scale_at([0.5, 0.1]);
        let bottom = s.radius_scale_at([0.5, 0.9]);
        assert!(
            (top / bottom - 1.0).abs() > 0.05,
            "a strong keystone must scale the two ends differently: {top} vs {bottom}"
        );
    }
}

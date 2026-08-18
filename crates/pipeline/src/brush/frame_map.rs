//! The forward map from pre-geometry source coordinates into frame pixels.
//!
//! ⚠ `GeometryStage` is written entirely as **inverse** maps — every output pixel asks where it
//! came from — because that is what resampling needs. Splatting a brush dab needs the opposite
//! direction, so all three steps are inverted here. The tests check this against `GeometryStage`
//! itself rather than against a second derivation of the same maths.

use edit_model::{CropRect, Geometry, Orientation, Quarter};

/// Maps a point in pre-geometry **source** normalised coordinates to **frame** pixels.
pub struct FrameMap {
    orientation: Orientation,
    sin: f32,
    cos: f32,
    crop: CropRect,
    /// Width over height of the oriented, pre-crop image.
    aspect_o: f32,
    fw: f32,
    fh: f32,
    /// Multiplies a radius given as a fraction of the source long edge into frame pixels.
    pub radius_scale: f32,
}

/// Below this a crop side is treated as degenerate; it only guards a division.
const CROP_EPS: f32 = 1e-6;

impl FrameMap {
    pub fn new(g: &Geometry, frame_w: usize, frame_h: usize) -> Self {
        let cw = (g.crop.right - g.crop.left).max(CROP_EPS);
        let ch = (g.crop.bottom - g.crop.top).max(CROP_EPS);
        // The oriented, pre-crop dimensions at this render scale. ⚠ Derived rather than passed
        // in: the pipeline is built before the image flows through it, so nothing upstream knows
        // them at construction time.
        let ow = frame_w as f32 / cw;
        let oh = frame_h as f32 / ch;
        let (sin, cos) = g.angle.to_radians().sin_cos();
        Self {
            orientation: g.orientation,
            sin,
            cos,
            crop: g.crop,
            aspect_o: ow / oh.max(CROP_EPS),
            fw: frame_w as f32,
            fh: frame_h as f32,
            // Orientation only permutes pixels, so the source long edge is the oriented one.
            radius_scale: ow.max(oh),
        }
    }

    /// `(x, y)` in frame pixels. May fall outside the frame — the caller clips.
    pub fn point(&self, p: [f32; 2]) -> (f32, f32) {
        // 1. Orientation. Mirror first, then turn — the forward of `orient`, which un-turns and
        //    then un-mirrors in source space. Dimensionless.
        let u = if self.orientation.mirror {
            1.0 - p[0]
        } else {
            p[0]
        };
        let v = p[1];
        let (ou, ov) = match self.orientation.rotate {
            Quarter::Zero => (u, v),
            Quarter::Cw90 => (1.0 - v, u),
            Quarter::Cw180 => (1.0 - u, 1.0 - v),
            Quarter::Cw270 => (v, 1.0 - u),
        };

        // 2. Straighten. ⚠ Rotation mixes the axes, so it needs an isotropic space: working in
        //    units of the oriented image's *height* gives one while needing only the aspect,
        //    never absolute dimensions. `GeometryStage`'s inverse is R(-θ), so this is R(θ).
        let (cx, cy) = (self.aspect_o * 0.5, 0.5);
        let (dx, dy) = (ou * self.aspect_o - cx, ov - cy);
        let rx = cx + dx * self.cos - dy * self.sin;
        let ry = cy + dx * self.sin + dy * self.cos;
        let (ru, rv) = (rx / self.aspect_o.max(CROP_EPS), ry);

        // 3. Crop, and 4. into frame pixels.
        let cw = (self.crop.right - self.crop.left).max(CROP_EPS);
        let ch = (self.crop.bottom - self.crop.top).max(CROP_EPS);
        (
            (ru - self.crop.left) / cw * self.fw,
            (rv - self.crop.top) / ch * self.fh,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::geometry::GeometryStage;
    use crate::PipelineStage;
    use core_image::ImageBuf;

    fn geo(orientation: Orientation, angle: f32, crop: CropRect) -> Geometry {
        Geometry {
            orientation,
            angle,
            crop,
        }
    }

    /// Run `GeometryStage` on a source with one marked pixel and report the frame, plus where the
    /// mark landed.
    fn run(g: &Geometry, w: usize, h: usize, sx: usize, sy: usize) -> (usize, usize, f32, f32) {
        let mut img = ImageBuf::new(w, h);
        img.set_pixel(sx, sy, [1.0, 1.0, 1.0]);
        let out = GeometryStage::new(g).apply(img);
        let (mut best, mut bx, mut by) = (0.0f32, 0usize, 0usize);
        for y in 0..out.height() {
            for x in 0..out.width() {
                let val = out.pixel(x, y)[0];
                if val > best {
                    best = val;
                    bx = x;
                    by = y;
                }
            }
        }
        assert!(best > 0.05, "the marked pixel was cropped away or lost");
        (out.width(), out.height(), bx as f32, by as f32)
    }

    /// ⚠⚠ **The whole point of this suite: the map is checked against the real transform, not
    /// against a re-derivation of it.** `FrameMap` was built by inverting three separate inverse
    /// maps, and re-deriving that in a test would prove only that the same mistake was made
    /// twice.
    fn check(g: Geometry, w: usize, h: usize, sx: usize, sy: usize) {
        let (fw, fh, gx, gy) = run(&g, w, h, sx, sy);
        let map = FrameMap::new(&g, fw, fh);
        let (mx, my) = map.point([(sx as f32 + 0.5) / w as f32, (sy as f32 + 0.5) / h as f32]);
        // ⚠ ~1.5 px: `rotate_bilinear` centres on `(w-1)/2` while the map centres on the
        // normalised midpoint, and the brightest-pixel search quantises to whole pixels.
        assert!(
            (mx - gx).abs() < 1.5 && (my - gy).abs() < 1.5,
            "map says ({mx:.2}, {my:.2}), geometry says ({gx}, {gy})"
        );
    }

    #[test]
    fn the_identity_geometry_is_the_identity_map() {
        let map = FrameMap::new(&Geometry::default(), 80, 40);
        let (x, y) = map.point([0.25, 0.75]);
        assert!((x - 20.0).abs() < 1e-3, "{x}");
        assert!((y - 30.0).abs() < 1e-3, "{y}");
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_crop() {
        check(
            geo(
                Orientation::default(),
                0.0,
                CropRect {
                    left: 0.2,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.8,
                },
            ),
            80,
            40,
            30,
            22,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_quarter_turn() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw90,
                    mirror: false,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            30,
            12,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_mirror() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Zero,
                    mirror: true,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            30,
            12,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_mirrored_turn() {
        // ⚠ The discriminating case for the non-abelian composition the orientation work
        // recorded: mirror-then-turn is not turn-then-mirror, and only a combined case tells them
        // apart.
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw270,
                    mirror: true,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            22,
            9,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_straighten_angle() {
        // ⚠⚠ **The probe needs LEVERAGE, and the obvious one does not have it.** A rotation sign
        // error displaces a point in proportion to its distance from the centre, so a probe near
        // the middle barely moves. An earlier version used 7° at (26, 14) — a mere 0.34 of the
        // half-width out — where flipping the sign moves the point **1.34 px**, just inside this
        // suite's 1.5 px tolerance. It passed with the sign flipped.
        //
        // 12° at (68, 33) is 0.71 of the half-width out and moves **5.61 px**, which the
        // tolerance catches comfortably. Found by running the sabotage, not by review.
        check(
            geo(Orientation::default(), 12.0, CropRect::default()),
            80,
            40,
            68,
            33,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_everything_at_once() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw90,
                    mirror: true,
                },
                5.0,
                CropRect {
                    left: 0.1,
                    top: 0.15,
                    right: 0.85,
                    bottom: 0.9,
                },
            ),
            80,
            40,
            34,
            18,
        );
    }

    #[test]
    fn the_radius_scale_is_the_source_long_edge_in_frame_pixels() {
        // A half-crop doubles the pre-crop size, so a radius of a given fraction of the long edge
        // covers twice as many frame pixels as it would uncropped.
        let full = FrameMap::new(&Geometry::default(), 80, 40);
        assert!(
            (full.radius_scale - 80.0).abs() < 1e-3,
            "{}",
            full.radius_scale
        );
        let half = FrameMap::new(
            &geo(
                Orientation::default(),
                0.0,
                CropRect {
                    left: 0.25,
                    top: 0.25,
                    right: 0.75,
                    bottom: 0.75,
                },
            ),
            40,
            20,
        );
        assert!(
            (half.radius_scale - 80.0).abs() < 1e-3,
            "{}",
            half.radius_scale
        );
    }
}

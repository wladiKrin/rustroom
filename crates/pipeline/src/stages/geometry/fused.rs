//! The fused inverse map: orientation, perspective, straighten and crop in **one** resample
//! (M7.3b).
//!
//! ⚠⚠ **Only taken when the perspective is active.** Without one, `GeometryStage` runs its original
//! code byte for byte (an exact orientation permutation, `rotate_region`, a crop slice), so no
//! photo without perspective renders one bit differently because this exists.
//!
//! ⚠⚠ **The brush anchor space survives as COORDINATES.** Brushes are anchored in "post-perspective,
//! pre-geometry" space. Before M7.3b that was a buffer (`PerspectiveStage`'s output), and here it is
//! the point between un-orienting and the homography in [`FusedMap::source_of`]. It is never stored,
//! but it has the same meaning, which is why `FrameMap` needed no change.
//!
//! ⚠ Why fusing is worth it: two resamples cost ~20% of acutance against one (the backlog's
//! `persp_sharpness` measurement), and Guide Mode sets both a keystone and an angle, so for the
//! people who use perspective at all, the double pass was the normal case.

use crate::perspective::Homography;
use crate::resample::{self, Weights};
use crate::{Rect, RegionSrc};
use core_image::ImageBuf;
use edit_model::{CropRect, Orientation, Perspective, Quarter};
use rayon::prelude::*;

/// Output pixel (in the cropped frame) → source pixel (pre-orientation, pre-perspective).
pub(super) struct FusedMap {
    /// The crop's origin in the oriented, rotated canvas.
    x0: f32,
    y0: f32,
    /// `(cx, cy, sin, cos)` about the **oriented** canvas centre. ⚠ `None` at angle 0: skipped
    /// rather than computed with `sin = 0`, so a perspective with no angle does
    /// `PerspectiveStage`'s arithmetic exactly and its goldens stay byte-identical.
    rot: Option<(f32, f32, f32, f32)>,
    orientation: Orientation,
    /// The source's `w - 1` and `h - 1`: `orient`'s index table, made continuous.
    w1: f32,
    h1: f32,
    /// The source frame's centre. ⚠⚠ The homography is defined about the FULL pre-orientation
    /// frame's centre in half-height units, which is `PerspectiveStage::render`'s normalisation,
    /// copied rather than re-derived.
    scx: f32,
    scy: f32,
    hom: Homography,
}

impl FusedMap {
    /// `(w, h)` are the **source** frame's, before orientation. ⚠⚠ The FULL frame's, never a
    /// region slice's: every constant here is a property of the frame.
    pub(super) fn new(
        w: usize,
        h: usize,
        orientation: Orientation,
        angle_rad: f32,
        crop: &CropRect,
        p: &Perspective,
    ) -> Self {
        let (ow, oh) = if orientation.rotate.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        };
        let (x0, y0, _, _) = super::crop_bounds(ow, oh, crop);
        let rot = (angle_rad != 0.0).then(|| {
            let (sin, cos) = angle_rad.sin_cos();
            ((ow as f32 - 1.0) / 2.0, (oh as f32 - 1.0) / 2.0, sin, cos)
        });
        let scx = (w as f32 - 1.0) / 2.0;
        let scy = (h as f32 - 1.0) / 2.0;
        Self {
            x0: x0 as f32,
            y0: y0 as f32,
            rot,
            orientation,
            w1: w as f32 - 1.0,
            h1: h as f32 - 1.0,
            scx,
            scy,
            hom: Homography::from_edits(p, scx / scy),
        }
    }

    /// Where output pixel `(x, y)` of the cropped frame reads from, in source pixels.
    pub(super) fn source_of(&self, x: f32, y: f32) -> (f32, f32) {
        // 1. Un-crop: into the oriented, rotated canvas.
        let (mut px, mut py) = (self.x0 + x, self.y0 + y);
        // 2. Un-rotate — `rotate_region`'s inverse map, associated the same way.
        if let Some((cx, cy, sin, cos)) = self.rot {
            let dx = px - cx;
            let dy = py - cy;
            px = cx + dx * cos + dy * sin;
            py = cy - dx * sin + dy * cos;
        }
        // 3. Un-orient: `orient`'s index table, made continuous. Un-rotate first, then un-mirror
        //    in source space, exactly as `orient` does.
        let (mut sx, sy) = match self.orientation.rotate {
            Quarter::Zero => (px, py),
            Quarter::Cw90 => (py, self.h1 - px),
            Quarter::Cw180 => (self.w1 - px, self.h1 - py),
            Quarter::Cw270 => (self.w1 - py, px),
        };
        if self.orientation.mirror {
            sx = self.w1 - sx;
        }
        // ← This point is the brush anchor space: post-perspective, pre-geometry.
        // 4. The homography's inverse, into the pre-perspective source.
        let n = ((sx - self.scx) / self.scy, (sy - self.scy) / self.scy);
        let s = self.hom.source_of(n);
        (self.scx + s.0 * self.scy, self.scy + s.1 * self.scy)
    }
}

/// [`FusedMap`]'s constants, exported so a GPU twin receives them rather than re-deriving them.
///
/// ⚠⚠ **One definition of the map's constants**: the crop origin, the rotation, the orientation
/// and the homography are computed here, by the code the CPU renders with, and handed over.
/// `inverse` is `None` when the keystone is inactive; the CPU's original path does no homography
/// then, and neither must the twin (a neutral homography is the identity only to within f32).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapParams {
    /// The crop's origin in the oriented, rotated canvas.
    pub crop_origin: [f32; 2],
    /// `[cx, cy, sin, cos]` about the oriented canvas centre; `None` at angle 0.
    pub rotation: Option<[f32; 4]>,
    /// Quarter turns clockwise, 0..=3.
    pub quarter: u32,
    pub mirror: bool,
    /// The source's `w - 1` and `h - 1`.
    pub last: [f32; 2],
    /// The source frame's centre, in pixels; the homography's unit is `centre[1]`.
    pub centre: [f32; 2],
    pub inverse: Option<[[f32; 3]; 3]>,
}

impl FusedMap {
    pub(super) fn params(&self, keystone: bool) -> MapParams {
        MapParams {
            crop_origin: [self.x0, self.y0],
            rotation: self.rot.map(|(cx, cy, s, c)| [cx, cy, s, c]),
            quarter: match self.orientation.rotate {
                Quarter::Zero => 0,
                Quarter::Cw90 => 1,
                Quarter::Cw180 => 2,
                Quarter::Cw270 => 3,
            },
            mirror: self.orientation.mirror,
            last: [self.w1, self.h1],
            centre: [self.scx, self.scy],
            inverse: keystone.then(|| self.hom.inverse_matrix()),
        }
    }
}

/// The gather, producing only `rect` of the cropped output.
///
/// ⚠⚠ **`GeometryStage::apply` on the fused path is this at full extent**, so the whole-buffer and
/// region paths are bit-identical by construction. Parallel over output rows: a gather writes only
/// its own pixel, so it is bit-identical to the sequential form.
pub(super) fn render(
    src: &RegionSrc<'_>,
    map: &FusedMap,
    weights: &Weights,
    rect: Rect,
) -> ImageBuf {
    const CH: usize = ImageBuf::CHANNELS;
    let stride = rect.w * CH;
    let mut data = vec![0.0f32; rect.w * rect.h * CH];
    // ⚠ `par_chunks_exact_mut(0)` panics.
    if stride == 0 {
        return ImageBuf::from_vec(rect.w, rect.h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(ry, orow)| {
            let y = (rect.y + ry) as f32;
            for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let (sx, sy) = map.source_of((rect.x + rx) as f32, y);
                *out = resample::sample_region(src, sx, sy, weights);
            }
        });
    ImageBuf::from_vec(rect.w, rect.h, data)
}

/// The source rect `render` reads for output rect `out`, plus the kernel's tap radius.
///
/// ⚠ Exact rather than conservative: un-crop, rotation, the orientation permutation and the
/// homography all send lines to lines, so the image of a rectangle is a quadrilateral and the
/// bounding box of its four mapped corners contains it. ⚠⚠ **Except across the horizon**, where the
/// homography's denominator vanishes: a non-finite corner **declines**, failing toward the
/// whole-buffer path. Unreachable for any representable `Perspective` (`MAX_ANGLE_DEG` is 30) —
/// `PerspectiveStage`'s guard, carried over rather than dropped.
pub(super) fn input_rect(
    map: &FusedMap,
    out: Rect,
    in_w: usize,
    in_h: usize,
    pad: usize,
) -> Option<Rect> {
    let (mut lo_x, mut lo_y) = (f32::INFINITY, f32::INFINITY);
    let (mut hi_x, mut hi_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let xs = [out.x as f32, (out.x + out.w - 1) as f32];
    let ys = [out.y as f32, (out.y + out.h - 1) as f32];
    for &y in &ys {
        for &x in &xs {
            let (sx, sy) = map.source_of(x, y);
            if !sx.is_finite() || !sy.is_finite() {
                return None;
            }
            lo_x = lo_x.min(sx);
            hi_x = hi_x.max(sx);
            lo_y = lo_y.min(sy);
            hi_y = hi_y.max(sy);
        }
    }
    let x = (lo_x.floor().max(0.0) as usize).saturating_sub(pad);
    let y = (lo_y.floor().max(0.0) as usize).saturating_sub(pad);
    let bx = ((hi_x.ceil().max(0.0) as usize) + pad + 1).min(in_w);
    let by = ((hi_y.ceil().max(0.0) as usize) + pad + 1).min(in_h);
    Some(Rect {
        x,
        y,
        w: bx.saturating_sub(x),
        h: by.saturating_sub(y),
    })
}

#[cfg(test)]
mod tests {
    //! ⚠ The first half is `PerspectiveStage`'s own suite, re-pointed at the fused path when that
    //! stage was deleted: a perspective with no angle, no crop and no orientation must be exactly
    //! what that stage was. The second half is what fusion adds.
    use crate::perspective::Homography;
    use crate::stages::geometry::GeometryStage;
    use crate::{PipelineStage, Rect, Resampler};
    use core_image::ImageBuf;
    use edit_model::{CropRect, Geometry, Orientation, Perspective, Quarter};

    fn persp(v: f32) -> Perspective {
        Perspective {
            vertical: v,
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

    /// Perspective alone, through the fused path — what `PerspectiveStage::new(p, k)` was.
    fn keystone(p: Perspective, k: Resampler) -> GeometryStage {
        GeometryStage::new(&Geometry::default(), k).with_perspective(p)
    }

    fn fused(g: &Geometry, p: Perspective) -> GeometryStage {
        GeometryStage::new(g, Resampler::default()).with_perspective(p)
    }

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    /// A field with an edge in it, so a wrong map shows as moved structure, not a shifted ramp.
    fn field(w: usize, h: usize) -> ImageBuf {
        let mut src = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let e = if x * 3 < w { 0.0 } else { 1.0 };
                src.set_pixel(x, y, [x as f32 / w as f32, y as f32 / h as f32, e]);
            }
        }
        src
    }

    /// Smooth enough that one extra Catmull-Rom pass changes it by far less than the keystone does.
    fn smooth(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let a = (u * 6.0).sin() * 0.25 + 0.5;
                let b = (v * 5.0).cos() * 0.25 + 0.5;
                img.set_pixel(x, y, [a, b, (a + b) * 0.5]);
            }
        }
        img
    }

    fn max_diff_interior(a: &ImageBuf, b: &ImageBuf, margin: usize) -> f32 {
        let mut m = 0.0f32;
        for y in margin..a.height() - margin {
            for x in margin..a.width() - margin {
                let (p, q) = (a.pixel(x, y), b.pixel(x, y));
                for c in 0..3 {
                    m = m.max((p[c] - q[c]).abs());
                }
            }
        }
        m
    }

    // ---- PerspectiveStage's suite, re-pointed ------------------------------------------------

    #[test]
    fn perspective_back_projects_every_sample_it_will_read() {
        let p = persp(40.0);
        let s = keystone(p, Resampler::CatmullRom);
        let (w, h) = (400usize, 300usize);
        let out = Rect {
            x: 100,
            y: 80,
            w: 50,
            h: 40,
        };
        let got = s.input_rect_for(out, w, h).unwrap();
        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        let hom = Homography::from_edits(&p, cx / cy);
        let pad = Resampler::CatmullRom.radius() as f32;
        for oy in out.y..out.y + out.h {
            for ox in out.x..out.x + out.w {
                let n = ((ox as f32 - cx) / cy, (oy as f32 - cy) / cy);
                let sp = hom.source_of(n);
                let (sx, sy) = (cx + sp.0 * cy, cy + sp.1 * cy);
                for (tx, ty) in [(sx - pad, sy - pad), (sx + pad, sy + pad)] {
                    let tx = tx.clamp(0.0, w as f32 - 1.0);
                    let ty = ty.clamp(0.0, h as f32 - 1.0);
                    assert!(
                        tx >= got.x as f32 - 1.0
                            && tx <= (got.x + got.w) as f32
                            && ty >= got.y as f32 - 1.0
                            && ty <= (got.y + got.h) as f32,
                        "sample ({tx}, {ty}) for output ({ox}, {oy}) falls outside {got:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_perspective_region_outside_the_output_is_refused() {
        let src = ramp(40, 30);
        let stage = keystone(persp(20.0), Resampler::default());
        for bad in [
            Rect {
                x: 0,
                y: 0,
                w: 41,
                h: 30,
            },
            Rect {
                x: 39,
                y: 0,
                w: 2,
                h: 30,
            },
            Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 30,
            },
        ] {
            assert!(
                stage
                    .apply_region(&crate::RegionSrc::whole(&src), bad)
                    .is_none(),
                "{bad:?}"
            );
            assert!(stage.input_rect_for(bad, 40, 30).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_keystone_really_moves_the_pixels() {
        let src = ramp(64, 48);
        let out = keystone(persp(50.0), Resampler::default()).apply(src.clone());
        assert_ne!(out, src);
        assert_eq!((out.width(), out.height()), (src.width(), src.height()));
    }

    #[test]
    fn the_canvas_size_is_unchanged_by_a_strong_correction() {
        for (w, h) in [(64, 48), (48, 64), (2, 2)] {
            let out = keystone(
                Perspective {
                    vertical: 100.0,
                    horizontal: -100.0,
                    aspect: 80.0,
                    scale: 0.0,
                },
                Resampler::default(),
            )
            .apply(ramp(w, h));
            assert_eq!((out.width(), out.height()), (w, h));
        }
    }

    #[test]
    fn a_one_pixel_image_passes_through() {
        let src = ImageBuf::new(1, 1);
        let out = keystone(persp(50.0), Resampler::default()).apply(src.clone());
        assert_eq!(out, src);
    }

    #[test]
    fn the_keystone_honours_the_chosen_resampler() {
        let mut src = ImageBuf::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let v = if (x / 4) % 2 == 0 { 0.0 } else { 1.0 };
                src.set_pixel(x, y, [v, v, v]);
            }
        }
        let run = |k| keystone(persp(40.0), k).apply(src.clone());
        let bilinear = run(Resampler::Bilinear);
        for k in Resampler::ALL {
            if k == Resampler::Bilinear {
                continue;
            }
            assert_ne!(
                run(k).data(),
                bilinear.data(),
                "{k:?} rendered identically to bilinear"
            );
        }
    }

    // ---- what fusion adds -------------------------------------------------------------------

    /// ⚠⚠ Region == whole **under the full map**, orientation included, fed exactly the slice
    /// `input_rect_for` asks for. That makes it a check on the bound as well: a bound too small
    /// reads zeros at the slice edge and differs.
    #[test]
    fn a_fused_region_matches_the_same_rect_of_the_whole() {
        let src = field(61, 43);
        for o in [
            Orientation::default(),
            Orientation {
                rotate: Quarter::Cw90,
                mirror: true,
            },
            Orientation {
                rotate: Quarter::Cw270,
                mirror: false,
            },
        ] {
            let g = geo(
                o,
                4.0,
                CropRect {
                    left: 0.1,
                    top: 0.05,
                    right: 0.9,
                    bottom: 0.95,
                },
            );
            let s = fused(&g, persp(30.0));
            let whole = s.apply(src.clone());
            let rect = Rect {
                x: 5,
                y: 3,
                w: 17,
                h: 13,
            };
            let need = s
                .input_rect_for(rect, src.width(), src.height())
                .expect("the fused path scopes under any orientation");
            let slice = src.crop(need.x, need.y, need.w, need.h);
            let rs = crate::RegionSrc {
                buf: &slice,
                origin: (need.x, need.y),
                full: (src.width(), src.height()),
            };
            let got = s.apply_region(&rs, rect).expect("accepts");
            assert_eq!(got, whole.crop(rect.x, rect.y, rect.w, rect.h), "{o:?}");
        }
    }

    /// ⚠⚠ The fused map against the two-pass chain it replaces. The chain is built from the stage
    /// itself (perspective alone, then geometry alone), which is what shipped before M7.3b.
    #[test]
    fn the_fused_stage_agrees_with_perspective_then_geometry() {
        let src = smooth(160, 120);
        for o in [
            Orientation::default(),
            Orientation {
                rotate: Quarter::Cw90,
                mirror: true,
            },
        ] {
            let g = geo(
                o,
                5.0,
                CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
            );
            let one = fused(&g, persp(40.0)).apply(src.clone());
            let two = GeometryStage::new(&g, Resampler::default())
                .apply(keystone(persp(40.0), Resampler::default()).apply(src.clone()));
            assert_eq!((one.width(), one.height()), (two.width(), two.height()));
            // ⚠ First prove the keystone moved something, or agreement proves nothing.
            let geo_only = GeometryStage::new(&g, Resampler::default()).apply(src.clone());
            assert!(
                max_diff_interior(&one, &geo_only, 8) > 0.02,
                "fixture: the keystone must move pixels"
            );
            // ⚠⚠ Compared only where BOTH passes of the chain read well inside their frames. The
            // default (fit) keystone leaves empty regions, and one pass and two smear that black
            // edge differently. The first run of this test found exactly that: 0.36 at a pixel
            // whose source sat 0.015 px from the bottom edge. The intermediate point is the fused
            // map with a neutral homography, since that is the chain's first output.
            let (w, h) = (src.width(), src.height());
            let angle = g.angle.to_radians();
            let full = super::FusedMap::new(w, h, o, angle, &g.crop, &persp(40.0));
            let mid = super::FusedMap::new(w, h, o, angle, &g.crop, &Perspective::default());
            let inside = |(x, y): (f32, f32)| {
                x >= 4.0 && y >= 4.0 && x <= w as f32 - 5.0 && y <= h as f32 - 5.0
            };
            let (mut d, mut n) = (0.0f32, 0usize);
            for y in 0..one.height() {
                for x in 0..one.width() {
                    let (fx, fy) = (x as f32, y as f32);
                    if !inside(full.source_of(fx, fy)) || !inside(mid.source_of(fx, fy)) {
                        continue;
                    }
                    n += 1;
                    let (p, q) = (one.pixel(x, y), two.pixel(x, y));
                    for c in 0..3 {
                        d = d.max((p[c] - q[c]).abs());
                    }
                }
            }
            // ⚠ Assert the sample first: an empty comparison passes silently.
            assert!(
                n > one.width() * one.height() / 2,
                "fixture: only {n} pixels compared"
            );
            assert!(d < 0.01, "{o:?}: fused and chained differ by {d}");
        }
    }

    /// ⚠ An inactive perspective runs the original code, bit for bit, whatever else is set.
    #[test]
    fn an_inactive_perspective_is_the_original_stage_exactly() {
        let src = field(61, 43);
        let g = geo(
            Orientation {
                rotate: Quarter::Cw90,
                mirror: false,
            },
            3.0,
            CropRect {
                left: 0.1,
                top: 0.0,
                right: 1.0,
                bottom: 0.8,
            },
        );
        assert_eq!(
            fused(&g, Perspective::default()).apply(src.clone()),
            GeometryStage::new(&g, Resampler::default()).apply(src)
        );
    }

    /// The output size is the crop's, fused or not.
    #[test]
    fn the_fused_output_has_the_cropped_size() {
        let src = field(61, 43);
        let g = geo(
            Orientation {
                rotate: Quarter::Cw90,
                mirror: false,
            },
            2.0,
            CropRect {
                left: 0.2,
                top: 0.1,
                right: 0.9,
                bottom: 0.7,
            },
        );
        let s = fused(&g, persp(25.0));
        let out = s.apply(src.clone());
        assert_eq!(
            (out.width(), out.height()),
            s.output_dims(src.width(), src.height())
        );
    }
}

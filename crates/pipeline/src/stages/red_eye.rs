//! Red- and pet-eye correction: each stored [`Eye`] circle recolours the pupil pixels inside it.
//!
//! ⚠⚠ **No detection here** (M9.11, approach A). The circle was found once, at the click, by
//! `app::eye_detect`; this stage only corrects inside it, per pixel, in proportion to how red
//! (pet: how bright) each pixel is — so skin and iris caught inside a slightly generous circle
//! survive, and the result cannot move when an upstream edit changes the pixels.
//!
//! ⚠ A **prefix** step right after spot removal, in the same post-lens, pre-perspective space, so
//! `SpotMap` draws it and a drag re-runs only this and the geometry. Like spot removal it is a
//! gather over a small bounding box — a plain loop, no rayon, no GPU twin.

use crate::stages::spot_removal::coverage;
use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::{Eye, EyeKind};

/// Feather of the circle's own edge, on `Spot::feather`'s 0..=100 scale.
const EDGE_FEATHER: f32 = 30.0;
/// Width of the redness / brightness ramp above its threshold.
const RAMP: f32 = 0.12;
/// Catchlight disc radius, in eye radii.
const CATCHLIGHT_R: f32 = 0.18;

/// How red a linear ProPhoto pixel is: `(R − max(G,B)) / R`, in `[0, 1]`.
///
/// ⚠⚠ **One definition, used by detection too** (`app::eye_detect`), so the click and the render
/// cannot disagree about what "red" is. Chosen by measurement over `(R − (G+B)/2) / R`, which put
/// skin at 0.27–0.47 and so overlapped flash pupils; this one puts skin at ≤ 0.37 and pupils at
/// ≥ 0.56 — `redness_separates_skin_from_flash_pupils` guards the gap.
pub fn redness(p: [f32; 3]) -> f32 {
    ((p[0] - p[1].max(p[2])) / p[0].max(1e-4)).clamp(0.0, 1.0)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Correct every eye, in order, on the working buffer.
pub struct RedEyeStage {
    eyes: Vec<Eye>,
}

impl RedEyeStage {
    pub fn new(eyes: &[Eye]) -> Self {
        Self {
            eyes: eyes.to_vec(),
        }
    }
}

impl PipelineStage for RedEyeStage {
    fn name(&self) -> &'static str {
        "red_eye"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let mut image = image;
        for e in &self.eyes {
            apply_eye(&mut image, e);
        }
        image
    }
}

fn apply_eye(image: &mut ImageBuf, e: &Eye) {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return;
    }
    // ⚠ The long edge, never the width — `Spot::radius`'s rule, or every circle is an ellipse.
    let r = e.radius * w.max(h) as f32;
    if !r.is_finite() || r <= 0.0 {
        return;
    }
    let cx = e.centre[0] * (w - 1) as f32;
    let cy = e.centre[1] * (h - 1) as f32;
    if !cx.is_finite() || !cy.is_finite() || cx + r < 0.0 || cy + r < 0.0 {
        return;
    }
    let x0 = (cx - r).floor().max(0.0) as usize;
    let y0 = (cy - r).floor().max(0.0) as usize;
    let x1 = ((cx + r).ceil() as usize).min(w - 1);
    let y1 = ((cy + r).ceil() as usize).min(h - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let size = (e.pupil_size / 100.0).clamp(0.0, 1.0);
    let dark = (e.darken / 100.0).clamp(0.0, 1.0);
    let dist = |x: usize, y: usize| (x as f32 - cx).hypot(y as f32 - cy);

    match e.kind {
        EyeKind::Red => {
            // ⚠ Pupil Size is SENSITIVITY: it lowers the redness threshold, reaching the pinkish
            // rim of a pupil; the circle still bounds it. Even at 100, `t0` stays above flushed
            // skin's 0.37.
            let t0 = lerp(0.60, 0.40, size);
            let k = lerp(1.0, 0.25, dark);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = coverage(dist(x, y), r, EDGE_FEATHER);
                    if c <= 0.0 {
                        continue;
                    }
                    let p = image.pixel(x, y);
                    let wgt = c * smoothstep(t0, t0 + RAMP, redness(p));
                    if wgt <= 0.0 {
                        continue;
                    }
                    // A neutral grey at the mean of green and blue — which carry the pupil's texture
                    // through — then darkened.
                    // ⚠ Neutral, not `[mean, G, B]`: the first draft kept G and B and replaced only
                    // red, and in ProPhoto that leaves a BROWN pupil once encoded to sRGB (seen on
                    // the golden), because a flash pupil's G and B are not equal in this space.
                    let v = 0.5 * (p[1] + p[2]) * k;
                    let fixed = [v; 3];
                    image.set_pixel(x, y, std::array::from_fn(|i| lerp(p[i], fixed[i], wgt)));
                }
            }
        }
        EyeKind::Pet => {
            // The 95th-percentile luminance inside the circle is the eyeshine's own level, so the
            // threshold follows the exposure rather than an absolute number.
            let mut ys: Vec<f32> = Vec::new();
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if dist(x, y) <= r {
                        ys.push(color::luminance(image.pixel(x, y)));
                    }
                }
            }
            if ys.is_empty() {
                return;
            }
            ys.sort_by(|a, b| a.total_cmp(b));
            let y_ref = ys[((ys.len() - 1) as f32 * 0.95) as usize].max(1e-6);
            // ⚠ Measured (sRGB samples through ProPhoto luminance): mid skin sits at 0.44–0.48 of
            // a green, yellow or white eyeshine's level, so the default `t0` of 0.60 leaves it
            // alone; the first draft's 0.45 darkened it. ⚠⚠ Luminance cannot separate a PALE
            // surround from a DIM eyeshine (light skin reaches 1.28× a dim green) — there only
            // the circle bounds the correction, which is why detection pads it by just 1.2.
            let t0 = lerp(0.80, 0.40, size);
            let k = lerp(0.15, 0.03, dark);
            let light = e.catchlight.map(|o| (cx + o[0] * r, cy + o[1] * r));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = coverage(dist(x, y), r, EDGE_FEATHER);
                    if c <= 0.0 {
                        continue;
                    }
                    let p = image.pixel(x, y);
                    let lum = color::luminance(p);
                    let wgt = c * smoothstep(t0, t0 + RAMP, lum / y_ref);
                    if wgt <= 0.0 {
                        continue;
                    }
                    // A neutral dark pupil that keeps the eyeshine's texture as luminance.
                    let mut fixed = [lum * k; 3];
                    // ⚠ The catchlight is part of the corrected pupil, so it lands only where the
                    // eyeshine weight is non-zero — never painted onto iris or fur.
                    if let Some((lx, ly)) = light {
                        let a =
                            coverage((x as f32 - lx).hypot(y as f32 - ly), CATCHLIGHT_R * r, 60.0);
                        fixed = fixed.map(|v| lerp(v, 0.9 * y_ref, a));
                    }
                    image.set_pixel(x, y, std::array::from_fn(|i| lerp(p[i], fixed[i], wgt)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(srgb: [f32; 3]) -> [f32; 3] {
        color::srgb_to_prophoto(srgb.map(color::srgb_to_linear))
    }

    /// 41×41 mid skin with a pupil of radius 6 at the centre and one white catchlight pixel.
    fn face(pupil: [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(41, 41);
        for y in 0..41 {
            for x in 0..41 {
                let d = (x as f32 - 20.0).hypot(y as f32 - 20.0);
                let px = if d <= 6.0 {
                    pp(pupil)
                } else {
                    pp([0.78, 0.57, 0.44])
                };
                img.set_pixel(x, y, px);
            }
        }
        img.set_pixel(18, 18, pp([1.0, 1.0, 1.0]));
        img
    }

    /// A circle of radius 9 px — deliberately larger than the pupil's 6, so skin lies inside it.
    fn eye(kind: EyeKind) -> Eye {
        Eye {
            centre: [0.5, 0.5],
            radius: 9.0 / 41.0,
            kind,
            pupil_size: 50.0,
            darken: 50.0,
            catchlight: None,
        }
    }

    /// ⚠ The measurement the thresholds were chosen from. Guarding it is what stops a later
    /// "improvement" of the metric silently re-opening the skin/pupil overlap.
    #[test]
    fn redness_separates_skin_from_flash_pupils() {
        for skin in [
            [0.87, 0.72, 0.62],
            [0.78, 0.57, 0.44],
            [0.45, 0.30, 0.22],
            [0.85, 0.55, 0.50],
        ] {
            let r = redness(pp(skin));
            assert!(r < 0.40, "skin {skin:?} scored {r}");
        }
        for pupil in [[0.60, 0.10, 0.10], [0.75, 0.35, 0.40], [0.45, 0.05, 0.05]] {
            let r = redness(pp(pupil));
            assert!(r > 0.55, "pupil {pupil:?} scored {r}");
        }
    }

    #[test]
    fn a_red_pupil_is_neutralised_and_darkened_while_skin_and_catchlight_are_kept() {
        let img = face([0.60, 0.10, 0.10]);
        let out = RedEyeStage::new(&[eye(EyeKind::Red)]).apply(img.clone());
        let p = out.pixel(22, 21);
        assert!(
            redness(img.pixel(22, 21)) > 0.7,
            "fixture check: the pupil is red"
        );
        assert!(redness(p) < 0.15, "the pupil is still red: {p:?}");
        assert!(color::luminance(p) < color::luminance(img.pixel(22, 21)));
        // Skin INSIDE the circle (d = 8 < r = 9) is untouched: its redness is below t0.
        assert_eq!(
            out.pixel(28, 20),
            img.pixel(28, 20),
            "skin inside the circle changed"
        );
        assert_eq!(
            out.pixel(18, 18),
            img.pixel(18, 18),
            "the catchlight changed"
        );
        assert_eq!(
            out.pixel(2, 2),
            img.pixel(2, 2),
            "outside the circle changed"
        );
    }

    #[test]
    fn pupil_size_reaches_a_pinker_pupil() {
        let img = face([0.75, 0.35, 0.40]);
        let small = RedEyeStage::new(&[Eye {
            pupil_size: 0.0,
            ..eye(EyeKind::Red)
        }])
        .apply(img.clone());
        let big = RedEyeStage::new(&[Eye {
            pupil_size: 100.0,
            ..eye(EyeKind::Red)
        }])
        .apply(img);
        let (s, b) = (redness(small.pixel(22, 21)), redness(big.pixel(22, 21)));
        assert!(
            b + 0.1 < s,
            "Pupil Size did not widen what is corrected: {s} vs {b}"
        );
    }

    #[test]
    fn a_pet_pupil_goes_dark_and_neutral_and_the_catchlight_lands_at_its_offset() {
        let img = face([0.55, 0.95, 0.45]); // green eyeshine
        let lit = Eye {
            catchlight: Some([-0.4, -0.4]),
            ..eye(EyeKind::Pet)
        };
        let out = RedEyeStage::new(&[lit]).apply(img.clone());
        let p = out.pixel(23, 22);
        assert!(color::luminance(p) < 0.25 * color::luminance(img.pixel(23, 22)));
        let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
        assert!(
            mx - mn < 0.02 * mx.max(1e-6) + 1e-4,
            "the pupil kept colour: {p:?}"
        );
        // offset −0.4 r ≈ −3.6 px from (20, 20)
        let cl = out.pixel(16, 16);
        assert!(
            color::luminance(cl) > 4.0 * color::luminance(p),
            "no catchlight at its offset: {cl:?} vs {p:?}"
        );
        let none = RedEyeStage::new(&[eye(EyeKind::Pet)]).apply(img);
        assert!(color::luminance(none.pixel(16, 16)) < color::luminance(cl));
        // Skin is darker than the eyeshine's threshold, so it survives inside the circle.
        assert_eq!(out.pixel(28, 20), face([0.55, 0.95, 0.45]).pixel(28, 20));
    }

    #[test]
    fn a_degenerate_radius_changes_nothing() {
        let img = face([0.60, 0.10, 0.10]);
        for r in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let out = RedEyeStage::new(&[Eye {
                radius: r,
                ..eye(EyeKind::Red)
            }])
            .apply(img.clone());
            assert_eq!(
                out.pixel(20, 20),
                img.pixel(20, 20),
                "radius {r} changed pixels"
            );
        }
    }
}

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
///
/// ⚠ Narrow on purpose: the circle BOUNDS the correction, the per-pixel redness already makes it
/// gradual. A feather of 30 faded it from 0.7 r and left the pupil's rim — where the halo is —
/// with 10–60% of its correction on the author's photo.
const EDGE_FEATHER: f32 = 10.0;
/// Width of the redness / brightness ramp above its threshold.
const RAMP: f32 = 0.12;
/// Catchlight disc radius, in eye radii.
const CATCHLIGHT_R: f32 = 0.18;
/// How far the fringe pass reaches beyond the red core, in eye radii (at least 1.5 px).
const FRINGE_GROW: f32 = 0.3;
/// The fringe's redness ramp: an iris reads ≈0.01, a pupil rim 0.17–0.44 (measured).
const FRINGE_LO: f32 = 0.05;
const FRINGE_HI: f32 = 0.20;

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

/// A square max filter of half-width `g` over a `w`×`h` map, as two 1-D passes.
fn dilate(m: &[f32], w: usize, h: usize, g: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; m.len()];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(g), (x + g).min(w - 1));
            rows[y * w + x] = m[y * w + a..=y * w + b].iter().copied().fold(0.0, f32::max);
        }
    }
    let mut out = vec![0.0f32; m.len()];
    for x in 0..w {
        for y in 0..h {
            let (a, b) = (y.saturating_sub(g), (y + g).min(h - 1));
            out[y * w + x] = (a..=b).map(|yy| rows[yy * w + x]).fold(0.0, f32::max);
        }
    }
    out
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
            // ⚠⚠ Two passes, because a threshold alone leaves a HALO (the author's report,
            // 2026-09-27, on a flash JPEG). A pupil's rim is part red, part iris: measured at
            // redness 0.17–0.44 there, the range skin also occupies, so no threshold that spares
            // skin can reach it — 57–64 pixels per 1-px ring stayed red at 10–12 px out.
            //
            // 1. The CORE: pixels confidently red. ⚠ Pupil Size is SENSITIVITY — it lowers this
            //    threshold; the circle still bounds it. ⚠ Calibrated on that photo: its core read
            //    0.44–0.57, not the 0.56–0.78 of synthetic pure reds the first draft assumed, and
            //    a threshold of 0.50 left half of it red at the default.
            let t0 = lerp(0.45, 0.30, size);
            let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
            let mut core = vec![0.0f32; bw * bh];
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if dist(x, y) <= r {
                        core[(y - y0) * bw + (x - x0)] =
                            smoothstep(t0, t0 + RAMP, redness(image.pixel(x, y)));
                    }
                }
            }
            // 2. The FRINGE: within `grow` px of the core, any red excess at all — the pupil's rim
            //    and a catchlight's pink edge. The iris (≈0.01) and skin further out are spared by
            //    distance, not by colour. A separable square max filter: O(n · grow).
            let grow = (FRINGE_GROW * r).max(1.5).ceil() as usize;
            let near = dilate(&core, bw, bh, grow);
            let k = lerp(1.0, 0.25, dark);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = coverage(dist(x, y), r, EDGE_FEATHER);
                    if c <= 0.0 {
                        continue;
                    }
                    let p = image.pixel(x, y);
                    let at = (y - y0) * bw + (x - x0);
                    let fringe = near[at] * smoothstep(FRINGE_LO, FRINGE_HI, redness(p));
                    let wgt = c * core[at].max(fringe);
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

    /// 41×41: a pupil of radius 6 (sRGB `pupil`), a grey iris out to 12, mid skin beyond, and one
    /// white catchlight pixel.
    ///
    /// ⚠ The iris ring is anatomy, not decoration: the fringe pass corrects any faintly red pixel
    /// within 0.3 r of the red core, so skin placed directly against the pupil — the first
    /// version of this fixture — would be corrected by design. A real pupil is ringed by iris.
    fn face(pupil: [f32; 3]) -> ImageBuf {
        face_linear(pp(pupil))
    }

    fn face_linear(pupil: [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(41, 41);
        for y in 0..41 {
            for x in 0..41 {
                let d = (x as f32 - 20.0).hypot(y as f32 - 20.0);
                let px = if d <= 6.0 {
                    pupil
                } else if d <= 12.0 {
                    [0.10, 0.10, 0.11]
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
            decode: false,
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
        // The iris INSIDE the circle (d = 8 < r = 9), within the fringe band, is untouched: it
        // carries no red excess.
        assert_eq!(
            out.pixel(28, 20),
            img.pixel(28, 20),
            "the iris inside the circle changed"
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

    /// ⚠ Redness 0.40, linear ProPhoto: below the core threshold at Pupil Size 0 (0.45), inside it
    /// at 100 (0.30). The pink of the first version (0.565) is corrected at every size since the
    /// recalibration on a real photo.
    #[test]
    fn pupil_size_reaches_a_pinker_pupil() {
        let img = face_linear([0.30, 0.18, 0.17]);
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

    /// A REAL pupil, as measured on a flash JPEG (2026-09-27, the author's report of a red halo):
    /// a core at redness ~0.5 — not the 0.8 of a synthetic pure red — fading over 3 px into a grey
    /// iris, with a white catchlight whose edge is pink. Linear ProPhoto, built directly.
    fn soft_eye() -> ImageBuf {
        let core = [0.30f32, 0.13, 0.12]; // redness ≈ 0.57
        let iris = [0.10f32, 0.10, 0.11];
        let skin = pp([0.78, 0.57, 0.44]);
        let mut img = ImageBuf::new(61, 61);
        for y in 0..61 {
            for x in 0..61 {
                let d = (x as f32 - 30.0).hypot(y as f32 - 30.0);
                let px = if d <= 16.0 {
                    // 0..=8 core, 8..=11 a linear blend into the iris, iris beyond.
                    let t = ((d - 8.0) / 3.0).clamp(0.0, 1.0);
                    std::array::from_fn(|i| core[i] * (1.0 - t) + iris[i] * t)
                } else {
                    skin
                };
                img.set_pixel(x, y, px);
            }
        }
        // A catchlight of radius 2 at (26, 26) with a one-pixel pink blend around it.
        for y in 22..31 {
            for x in 22..31 {
                let d = (x as f32 - 26.0).hypot(y as f32 - 26.0);
                let t = (3.0 - d).clamp(0.0, 1.0);
                if t > 0.0 {
                    let p = img.pixel(x, y);
                    img.set_pixel(x, y, std::array::from_fn(|i| p[i] * (1.0 - t) + 0.9 * t));
                }
            }
        }
        img
    }

    /// ⚠⚠ The reported halo. A redness threshold alone cannot reach a pupil's soft rim: those
    /// pixels are part red, part iris, and score 0.17–0.44 — the range skin also occupies. On the
    /// real photo 57–64 pixels per 1-px ring stayed red at distances 10–12 px.
    #[test]
    fn a_soft_pupil_rim_and_a_pink_catchlight_edge_leave_no_red_behind() {
        let img = soft_eye();
        let e = Eye {
            centre: [0.5, 0.5],
            radius: 13.0 / 61.0,
            ..eye(EyeKind::Red)
        };
        let out = RedEyeStage::new(&[e]).apply(img.clone());
        let mut left = Vec::new();
        for y in 17..44 {
            for x in 17..44 {
                if (x as f32 - 30.0).hypot(y as f32 - 30.0) <= 12.0 {
                    let r = redness(out.pixel(x, y));
                    if r > 0.10 {
                        left.push((x, y, r, redness(img.pixel(x, y))));
                    }
                }
            }
        }
        assert!(
            redness(img.pixel(30, 20)) > 0.15,
            "fixture check: the rim is reddish before"
        );
        assert!(
            left.is_empty(),
            "red left behind (x, y, after, before): {left:?}"
        );
        // The iris beyond the rim is not red and must not be touched.
        assert_eq!(out.pixel(30, 17), img.pixel(30, 17), "the iris changed");
        // The catchlight's white centre survives.
        assert!(
            color::luminance(out.pixel(26, 26)) > 0.5,
            "the catchlight was darkened"
        );
    }

    /// And at the DEFAULT Pupil Size, a real-strength core (redness ~0.5) is fully corrected —
    /// the first calibration's threshold of 0.50 left half of it red.
    #[test]
    fn a_real_strength_core_is_corrected_at_the_default_pupil_size() {
        let img = soft_eye();
        let e = Eye {
            centre: [0.5, 0.5],
            radius: 13.0 / 61.0,
            ..eye(EyeKind::Red)
        };
        let out = RedEyeStage::new(&[e]).apply(img);
        assert!(redness(out.pixel(33, 31)) < 0.05, "{:?}", out.pixel(33, 31));
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

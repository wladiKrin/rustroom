use crate::{PipelineStage, Region};
use core_image::ImageBuf;
use edit_model::Vignette;

/// Radius at which the falloff is centred, at Midpoint 0 and 100. `d` is 1 on the
/// frame-inscribed ellipse and ~1.41 at its corners, so this spans "well inside the frame"
/// to "past the corners", i.e. effectively off.
const R_MID: (f32, f32) = (0.2, 1.4);
/// Width of the transition band at Feather 0 and 100. Starts at 0 so Feather 0 is a
/// genuinely hard edge.
const FEATHER: (f32, f32) = (0.0, 1.0);
/// ...which makes this the only thing standing between Feather 0 and a division by zero in
/// `smoothstep`. Load-bearing, not belt-and-braces.
const WIDTH_EPS: f32 = 1e-4;
/// Superellipse exponent at Roundness −100. Higher is more rectangular.
const P_MAX: f32 = 6.0;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Clamped Hermite smoothstep. Same form as the inline one in `stages/hsl.rs`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The vignette's iso-contour shape, from Roundness and the frame's aspect.
///
/// The two halves of the slider move different variables and never both at once: positive
/// stretches an axis (ellipse → circle), negative raises the exponent (ellipse → rounded
/// rectangle).
struct Shape {
    wu: f32,
    wv: f32,
    /// Superellipse exponent. 2 is an ellipse and takes the `sqrt` fast path.
    expo: f32,
    elliptical: bool,
}

impl Shape {
    /// `w`/`h` are the *cropped* frame's dimensions, so this cannot be built in
    /// `VignetteStage::new` — it depends on the image the stage is handed.
    fn new(roundness: f32, w: usize, h: usize) -> Self {
        let (mut wu, mut wv, mut expo, mut elliptical) = (1.0, 1.0, 2.0, true);
        if roundness > 0.0 {
            // Stretch the long axis so d = 1 traces a circle of radius min(w,h)/2 in pixels.
            let t = (roundness / 100.0).min(1.0);
            let aspect = w.max(h) as f32 / w.min(h).max(1) as f32;
            if w >= h {
                wu = lerp(1.0, aspect, t);
            } else {
                wv = lerp(1.0, aspect, t);
            }
        } else if roundness < 0.0 {
            expo = lerp(2.0, P_MAX, (-roundness / 100.0).min(1.0));
            elliptical = false;
        }
        Self {
            wu,
            wv,
            expo,
            elliptical,
        }
    }

    /// Distance in this shape's metric, for normalized frame coordinates in [-1, 1].
    fn distance(&self, u: f32, v: f32) -> f32 {
        let (u, v) = (u * self.wu, v * self.wv);
        if self.elliptical {
            (u * u + v * v).sqrt()
        } else {
            (u.abs().powf(self.expo) + v.abs().powf(self.expo)).powf(1.0 / self.expo)
        }
    }
}

/// Post-crop vignette: a position-dependent gain applied in linear light.
///
/// Needs no `RenderScale` — it works in normalized frame coordinates, so the preview and the
/// export agree exactly. And "post-crop" is structural rather than a mode: `GeometryStage`
/// runs first, so the image handed here is already cropped and its aspect is the crop's.
///
/// Amount interpolates toward an endpoint rather than applying an exposure-style gain,
/// because a multiplicative gain is strictly positive and so can *never* reach black:
/// darkening scales by `(1 + f)²` (reaching exactly 0 at −100), brightening lifts toward
/// white by `(1 − v)·(1 − (1 − f)²)` (reaching exactly 1.0 at +100).
///
/// Both curves are **squared about their own endpoint**, so the mid-range carries real
/// weight — `f = −0.5` is two stops down — and the two halves are mirror images: equal
/// |amount| moves a pixel equally far toward black and toward white.
///
/// The response is not symmetric in log space, though: past −50 each further step is worth
/// progressively more, which is inherent to reaching true black at all.
///
/// Neither branch can produce a negative sample (`1 + f ≥ 0`, and the lift only ever raises
/// a value), so no clamp is needed, unlike `HslStage` and `NoiseReductionStage`. Darkening
/// remains a scalar multiply and so preserves `r:g:b` ratios exactly; brightening
/// desaturates as it approaches white, which is what "toward white" means.
pub struct VignetteStage {
    params: Vignette,
    region: Option<Region>,
}

impl VignetteStage {
    /// For a render covering the whole frame.
    pub fn new(params: &Vignette) -> Self {
        Self::new_in_region(params, None)
    }

    /// For a render covering only part of the frame — the falloff is still measured against
    /// the full frame, so a cropped region is shaded exactly as that part of the whole would
    /// have been.
    pub fn new_in_region(params: &Vignette, region: Option<Region>) -> Self {
        Self {
            params: *params,
            region,
        }
    }
}

impl PipelineStage for VignetteStage {
    fn name(&self) -> &'static str {
        "vignette"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let p = self.params;

        // The frame the falloff is measured against, and where this buffer sits in it.
        // Without a region the buffer *is* the frame — the existing behaviour, exactly.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };

        let shape = Shape::new(p.roundness, fw, fh);
        let r_mid = lerp(R_MID.0, R_MID.1, (p.midpoint / 100.0).clamp(0.0, 1.0));
        let width = lerp(FEATHER.0, FEATHER.1, (p.feather / 100.0).clamp(0.0, 1.0)).max(WIDTH_EPS);
        let (e0, e1) = (r_mid - width * 0.5, r_mid + width * 0.5);
        let k = (p.amount / 100.0).clamp(-1.0, 1.0);
        let (hw, hh) = (fw as f32 * 0.5, fh as f32 * 0.5);

        image.map_pixels_xy(|x, y, rgb| {
            let u = ((x + x0) as f32 + 0.5) / hw - 1.0;
            let v = ((y + y0) as f32 + 0.5) / hh - 1.0;
            let f = k * smoothstep(e0, e1, shape.distance(u, v)); // -1 … 1
            if f <= 0.0 {
                // Squared: 1 at f = 0, still exactly 0 at f = -1, but about twice the stops
                // at every point in between (f = -0.5 is -2 EV, not -1).
                let gain = (1.0 + f) * (1.0 + f);
                [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
            } else {
                // Toward white, squared about *its* endpoint so the two halves are mirror
                // images: equal |amount| moves a pixel equally far toward black and toward
                // white. `max(0.0)` leaves scene-referred samples already above 1.0 alone
                // rather than dragging them back down.
                let s = 1.0 - (1.0 - f) * (1.0 - f);
                let lift = |v: f32| v + (1.0 - v).max(0.0) * s;
                [lift(rgb[0]), lift(rgb[1]), lift(rgb[2])]
            }
        })
    }
}

#[cfg(test)]
mod tests {
    // Brings in VignetteStage, Shape, ImageBuf, Vignette, PipelineStage (for `.apply`)
    // and the constants.
    use super::*;

    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        img
    }

    /// The gain applied at one pixel, recovered from a flat 0.5 image.
    fn gain_at(params: &Vignette, w: usize, h: usize, x: usize, y: usize) -> f32 {
        VignetteStage::new(params).apply(flat(w, h)).pixel(x, y)[0] / 0.5
    }

    fn darken(amount: f32) -> Vignette {
        Vignette {
            amount,
            ..Default::default()
        }
    }

    /// A frame with varied content, so a mis-placed vignette shows up as different bytes.
    fn varied(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        0.2 + x as f32 / (w as f32 * 2.0),
                        0.5,
                        0.8 - y as f32 / (h as f32 * 2.0),
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn a_region_render_matches_the_whole_frame_cropped() {
        // The whole point of Region: given a crop, the falloff must still be measured
        // against the full frame. Without it the vignette would re-centre on the crop.
        let full = varied(40, 24);
        let params = Vignette {
            amount: -70.0,
            midpoint: 30.0,
            roundness: 60.0,
            feather: 40.0,
        };
        let expect = VignetteStage::new(&params)
            .apply(full.clone())
            .crop(11, 7, 13, 9);

        let region = Region {
            full_w: 40,
            full_h: 24,
            x0: 11,
            y0: 7,
        };
        let got =
            VignetteStage::new_in_region(&params, Some(region)).apply(full.crop(11, 7, 13, 9));

        assert_eq!(got.data(), expect.data());
    }

    #[test]
    fn a_region_covering_the_whole_frame_is_the_same_as_no_region() {
        let full = varied(20, 12);
        let params = Vignette {
            amount: -50.0,
            roundness: -40.0,
            ..Default::default()
        };
        let none = VignetteStage::new(&params).apply(full.clone());
        let whole = VignetteStage::new_in_region(
            &params,
            Some(Region {
                full_w: 20,
                full_h: 12,
                x0: 0,
                y0: 0,
            }),
        )
        .apply(full);
        assert_eq!(none.data(), whole.data());
    }

    #[test]
    fn the_region_sets_the_aspect_not_the_crop() {
        // A tall crop out of a wide frame must still use the *frame's* aspect for
        // roundness — this is the part that silently looks fine but is wrong.
        let full = varied(64, 16);
        let params = Vignette {
            amount: -80.0,
            roundness: 100.0,
            ..Default::default()
        };
        let expect = VignetteStage::new(&params)
            .apply(full.clone())
            .crop(20, 2, 8, 12);
        let got = VignetteStage::new_in_region(
            &params,
            Some(Region {
                full_w: 64,
                full_h: 16,
                x0: 20,
                y0: 2,
            }),
        )
        .apply(full.crop(20, 2, 8, 12));
        assert_eq!(got.data(), expect.data());
    }

    #[test]
    fn amount_zero_is_an_exact_identity() {
        let params = Vignette {
            amount: 0.0,
            midpoint: 0.0,
            roundness: -100.0,
            feather: 0.0,
        };
        let src = flat(16, 8);
        assert_eq!(VignetteStage::new(&params).apply(src.clone()), src);
    }

    #[test]
    fn the_centre_is_untouched_at_default_shape() {
        // d ~ 0 at the centre, well inside e0 for the default midpoint/feather.
        let g = gain_at(&darken(-100.0), 64, 64, 32, 32);
        assert!((g - 1.0).abs() < 1e-5, "centre gain {g}");
    }

    #[test]
    fn corners_darken_and_brighten_with_the_sign_of_amount() {
        assert!(gain_at(&darken(-60.0), 64, 64, 0, 0) < 0.99);
        assert!(gain_at(&darken(60.0), 64, 64, 0, 0) > 1.01);
    }

    #[test]
    fn the_extremes_reach_pure_black_and_pure_white() {
        // Midpoint and feather low, so a corner is unambiguously past the transition and
        // t == 1. This is the whole reason the model is a lerp toward the endpoints rather
        // than a multiplicative gain: exp2 is strictly positive and can never reach black.
        let params = Vignette {
            amount: -100.0,
            midpoint: 0.0,
            feather: 0.0,
            roundness: 0.0,
        };
        let out = VignetteStage::new(&params).apply(flat(64, 64));
        assert_eq!(
            out.pixel(0, 0),
            [0.0, 0.0, 0.0],
            "corner should be pure black"
        );

        let up = Vignette {
            amount: 100.0,
            ..params
        };
        let out = VignetteStage::new(&up).apply(flat(64, 64));
        let px = out.pixel(0, 0);
        for c in px {
            assert!(
                (c - 1.0).abs() < 1e-6,
                "corner should be pure white: {px:?}"
            );
        }
    }

    #[test]
    fn darkening_and_brightening_are_mirror_images() {
        // Both curves are squared about their own endpoint, so equal |amount| moves a pixel
        // equally far toward black and toward white. From a flat 0.5: -50 lands at 0.125,
        // which is 75% of the way to black; +50 must land at 0.875, 75% of the way to white.
        let at = |amount: f32| {
            let p = Vignette {
                amount,
                midpoint: 0.0,
                feather: 0.0,
                roundness: 0.0,
            };
            VignetteStage::new(&p).apply(flat(64, 64)).pixel(0, 0)[0]
        };
        for a in [25.0, 50.0, 75.0] {
            let toward_black = 1.0 - at(-a) / 0.5;
            let toward_white = (at(a) - 0.5) / 0.5;
            assert!(
                (toward_black - toward_white).abs() < 1e-5,
                "amount ±{a}: {toward_black} toward black vs {toward_white} toward white"
            );
        }
    }

    #[test]
    fn the_middle_of_the_slider_is_two_stops_down() {
        // The darkening curve is squared, so amount -50 is (1 - 0.5)^2 = 0.25, i.e. two
        // stops — roughly double the punch of the linear curve it replaced, at every point.
        // Asserted so a future retune cannot silently shift the range people build a feel
        // for.
        let params = Vignette {
            amount: -50.0,
            midpoint: 0.0,
            feather: 0.0,
            roundness: 0.0,
        };
        let g = gain_at(&params, 64, 64, 0, 0);
        assert!(
            (g - 0.25).abs() < 1e-5,
            "corner gain {g}, want 0.25 (= -2 EV)"
        );
    }

    #[test]
    fn gain_is_four_fold_mirror_symmetric() {
        let p = darken(-60.0);
        let g = gain_at(&p, 64, 64, 10, 20);
        for (x, y) in [(53, 20), (10, 43), (53, 43)] {
            let m = gain_at(&p, 64, 64, x, y);
            assert!((g - m).abs() < 1e-5, "({x},{y}): {m} vs {g}");
        }
    }

    #[test]
    fn roundness_plus_100_is_circular_in_pixel_space() {
        // 128x64: u = 0.5 is 32 px horizontally, v = 1.0 is 32 px vertically. A true
        // circle must treat them identically.
        let s = Shape::new(100.0, 128, 64);
        assert!((s.distance(0.5, 0.0) - s.distance(0.0, 1.0)).abs() < 1e-6);
    }

    #[test]
    fn roundness_zero_follows_the_frame_not_the_pixels() {
        // The counterpart to the test above: at roundness 0 equal *fractional* distances
        // match and equal *pixel* distances do not. Either test alone would pass for the
        // wrong reason on a square frame.
        let s = Shape::new(0.0, 128, 64);
        assert!((s.distance(0.5, 0.0) - s.distance(0.0, 0.5)).abs() < 1e-6);
        assert!((s.distance(0.5, 0.0) - s.distance(0.0, 1.0)).abs() > 0.4);
    }

    #[test]
    fn roundness_minus_100_darkens_corners_less() {
        // Raising the superellipse exponent LOWERS d off the axes, so the corner is less
        // affected and the untouched interior becomes a rounded rectangle. Getting this
        // direction backwards is the easy mistake.
        let corner_round = Shape::new(-100.0, 64, 64).distance(1.0, 1.0);
        let corner_ellipse = Shape::new(0.0, 64, 64).distance(1.0, 1.0);
        assert!(
            corner_round < corner_ellipse,
            "superellipse corner {corner_round} should be below ellipse {corner_ellipse}"
        );
        assert!((corner_ellipse - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((corner_round - 2.0f32.powf(1.0 / P_MAX)).abs() < 1e-6);
    }

    #[test]
    fn hue_is_preserved_when_darkening() {
        // Darkening is still a scalar multiply, so r:g:b ratios survive exactly. (The
        // brightening half lerps toward white and therefore desaturates by construction —
        // that is what "toward white" means, not a defect.)
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.4, 0.2, 0.1]);
            }
        }
        let out = VignetteStage::new(&darken(-70.0)).apply(img);
        let px = out.pixel(0, 0);
        assert!((px[0] / px[1] - 2.0).abs() < 1e-4, "{px:?}");
        assert!((px[1] / px[2] - 2.0).abs() < 1e-4, "{px:?}");
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        // gain = exp2(..) is strictly positive, so this holds structurally — assert it so
        // a future refactor cannot quietly break it.
        for params in [
            Vignette {
                amount: -100.0,
                midpoint: 0.0,
                roundness: -100.0,
                feather: 0.0,
            },
            Vignette {
                amount: 100.0,
                midpoint: 100.0,
                roundness: 100.0,
                feather: 100.0,
            },
        ] {
            let out = VignetteStage::new(&params).apply(flat(32, 16));
            assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
        }
    }

    #[test]
    fn feather_zero_is_a_hard_edge_not_a_divide_by_zero() {
        let params = Vignette {
            amount: -100.0,
            midpoint: 50.0,
            feather: 0.0,
            roundness: 0.0,
        };
        let out = VignetteStage::new(&params).apply(flat(64, 64));
        assert!(out.data().iter().all(|v| v.is_finite()));
        // Centre untouched, corner fully affected — no intermediate band worth speaking of.
        assert!((out.pixel(32, 32)[0] / 0.5 - 1.0).abs() < 1e-5);
        assert_eq!(out.pixel(0, 0)[0], 0.0);
    }

    #[test]
    fn the_gain_is_resolution_independent() {
        // The property RenderScale exists for, obtained free by working in normalized
        // coordinates. Asserted in two parts, because pixel *sample centres* do not land on
        // identical normalized positions at different sizes — at 64 px a half-pixel is
        // 1/128 of the half-width, which is a real offset, and conflating that with scale
        // dependence would make this test either wrong or meaningless.
        let p = darken(-60.0);

        // Exact: the corner is past the falloff at both sizes, so t == 1 and the gain is
        // 1 + s regardless of resolution. No quantization to muddy it.
        let small = gain_at(&p, 64, 64, 0, 0);
        let large = gain_at(&p, 256, 256, 0, 0);
        assert!((small - large).abs() < 1e-6, "corner: {small} vs {large}");
        assert!(
            (small - 0.16).abs() < 1e-5,
            "corner gain {small}, want (1 - 0.6)^2"
        );

        // Mid-falloff: agrees to within the half-pixel offset between the two sample grids,
        // which is far tighter than the ~0.36 spread across the falloff itself.
        let small = gain_at(&p, 64, 64, 16, 16);
        let large = gain_at(&p, 256, 256, 64, 64);
        assert!((small - large).abs() < 0.05, "mid: {small} vs {large}");
    }
}

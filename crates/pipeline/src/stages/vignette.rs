use crate::PipelineStage;
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
/// Amount ±100 is this many stops at full falloff. Symmetric in log space, so −50 and +50
/// are exactly equal and opposite.
const MAX_EV: f32 = 2.0;
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
/// The gain is `exp2(..)`, hence strictly positive: this stage cannot emit negative samples
/// and needs no clamp, unlike `HslStage` and `NoiseReductionStage`. Being a scalar multiply,
/// it also leaves `r:g:b` ratios — and so hue and saturation — exactly alone.
pub struct VignetteStage {
    params: Vignette,
}

impl VignetteStage {
    pub fn new(params: &Vignette) -> Self {
        Self { params: *params }
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

        let shape = Shape::new(p.roundness, w, h);
        let r_mid = lerp(R_MID.0, R_MID.1, (p.midpoint / 100.0).clamp(0.0, 1.0));
        let width = lerp(FEATHER.0, FEATHER.1, (p.feather / 100.0).clamp(0.0, 1.0)).max(WIDTH_EPS);
        let (e0, e1) = (r_mid - width * 0.5, r_mid + width * 0.5);
        let k = p.amount / 100.0 * MAX_EV;
        let (hw, hh) = (w as f32 * 0.5, h as f32 * 0.5);

        image.map_pixels_xy(|x, y, rgb| {
            let u = (x as f32 + 0.5) / hw - 1.0;
            let v = (y as f32 + 0.5) / hh - 1.0;
            let gain = (k * smoothstep(e0, e1, shape.distance(u, v))).exp2();
            [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
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
    fn amount_minus_100_is_exactly_two_stops_at_full_falloff() {
        // Midpoint and feather low, so a corner is unambiguously past the transition and
        // t == 1. This pins MAX_EV.
        let params = Vignette {
            amount: -100.0,
            midpoint: 0.0,
            feather: 0.0,
            roundness: 0.0,
        };
        let g = gain_at(&params, 64, 64, 0, 0);
        assert!((g - 0.25).abs() < 1e-4, "corner gain {g}, want 0.25");

        let up = Vignette {
            amount: 100.0,
            ..params
        };
        let g = gain_at(&up, 64, 64, 0, 0);
        assert!((g - 4.0).abs() < 1e-3, "corner gain {g}, want 4.0");
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
    fn hue_is_preserved() {
        // A scalar gain leaves r:g:b ratios alone — no Oklab round trip needed to be safe.
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
        assert!((out.pixel(0, 0)[0] / 0.5 - 0.25).abs() < 1e-4);
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
        // exp2(k) regardless of resolution. No quantization to muddy it.
        let small = gain_at(&p, 64, 64, 0, 0);
        let large = gain_at(&p, 256, 256, 0, 0);
        assert!((small - large).abs() < 1e-6, "corner: {small} vs {large}");
        assert!(
            (small - (-1.2f32).exp2()).abs() < 1e-5,
            "corner gain {small}"
        );

        // Mid-falloff: agrees to within the half-pixel offset between the two sample grids,
        // which is far tighter than the ~0.36 spread across the falloff itself.
        let small = gain_at(&p, 64, 64, 16, 16);
        let large = gain_at(&p, 256, 256, 64, 64);
        assert!((small - large).abs() < 0.05, "mid: {small} vs {large}");
    }
}

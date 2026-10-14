//! How strongly each pixel is blurred: its depth's distance from the focal plane, beyond the sharp zone.

use edit_model::{Bokeh, LensBlur};

/// Everything the blur needs besides the planes — resolved for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// The focal plane's depth, 0 (nearest) ..= 1 (farthest).
    pub focus: f32,
    /// Half the sharp zone's depth, 0 ..= 0.5 — `focal_range / 200`.
    pub half_range: f32,
    /// The strongest blur's radius in this frame's pixels — `amount / 100 × 3 %` of its long side.
    pub radius_px: f32,
    pub bokeh: Bokeh,
    /// 0 ..= 100.
    pub boost: f32,
}

/// The strongest blur's radius, as a fraction of the frame's long side, at Amount 100.
pub const MAX_RADIUS: f32 = 0.03;

/// How far beyond the sharp zone, in depth, the blur reaches its full radius.
pub const RAMP: f32 = 0.5;

impl Params {
    pub fn new(lb: &LensBlur, focus: f32, long_side_px: usize) -> Params {
        Params {
            focus,
            half_range: lb.focal_range.clamp(0.0, 100.0) / 200.0,
            radius_px: lb.amount.clamp(0.0, 100.0) / 100.0 * MAX_RADIUS * long_side_px as f32,
            bokeh: lb.bokeh,
            boost: lb.boost.clamp(0.0, 100.0),
        }
    }
}

/// The blur radius in pixels at depth `d` (0 near … 1 far), before the subject's alpha: 0 inside the
/// sharp zone `|d − focus| ≤ half_range`, rising linearly to `radius_px` [`RAMP`] beyond it. A
/// non-finite depth is in focus.
pub fn radius(p: &Params, d: f32) -> f32 {
    if !d.is_finite() {
        return 0.0;
    }
    p.radius_px * (((d - p.focus).abs() - p.half_range) / RAMP).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params::new(
            &LensBlur {
                on: true,
                amount: 100.0,
                focal_range: 20.0,
                ..LensBlur::default()
            },
            0.4,
            1000,
        )
    }

    #[test]
    fn params_follow_the_settings() {
        let p = params();
        assert_eq!(p.focus, 0.4);
        assert!((p.half_range - 0.1).abs() < 1e-6);
        assert!(
            (p.radius_px - 30.0).abs() < 1e-4,
            "3 % of 1000 at Amount 100"
        );
        let half = Params::new(
            &LensBlur {
                amount: 50.0,
                ..LensBlur::default()
            },
            0.4,
            1000,
        );
        assert!((half.radius_px - 15.0).abs() < 1e-4);
    }

    #[test]
    fn radius_is_zero_in_the_sharp_zone_and_grows_outside() {
        let p = params();
        for d in [0.3, 0.35, 0.4, 0.45, 0.5] {
            assert_eq!(radius(&p, d), 0.0, "{d} is in the sharp zone");
        }
        let mut last = 0.0;
        for i in 0..=100 {
            let r = radius(&p, 0.5 + 0.5 * i as f32 / 100.0);
            assert!(r >= last, "not monotone at {i}");
            last = r;
        }
        assert!(
            (radius(&p, 1.0) - 30.0).abs() < 1e-4,
            "full at half_range + RAMP"
        );
        assert!(radius(&p, 0.0) > 0.0, "the foreground blurs too");
        assert_eq!(radius(&p, f32::NAN), 0.0);
    }
}

//! Lens Blur (M10.16d): a synthetic shallow depth of field on the photo's estimated depth — what the
//! user set; the blur itself is `pipeline::stages::lens_blur`.

use serde::{Deserialize, Serialize};

/// The Lens Blur section's settings.
///
/// ⚠ **Off is never written** (`Edits::lens_blur`'s `skip_serializing_if`): a default block would change
/// every rendered preview's key on disk — `negative`'s and `grain`'s reason.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensBlur {
    pub on: bool,
    /// 0..=100 — the strongest blur: at 100 the farthest background's discs reach 3 % of the long side.
    pub amount: f32,
    pub focus: LensFocus,
    /// 0..=100 — how deep the sharp zone around the focus is; 0 = only the focal plane.
    pub focal_range: f32,
    pub bokeh: Bokeh,
    /// 0..=100 — how strongly bright out-of-focus highlights bloom into discs.
    pub boost: f32,
}

/// What is in focus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum LensFocus {
    /// The detected subject — its outline from the Subject map, its depth the focal plane.
    #[default]
    Subject,
    /// The depth under a click, the point normalised over the **decoded** photo's own pixel grid (an
    /// AI click's space), so it stays on its object through every crop, rotation and lens change.
    Point { point: [f32; 2] },
}

/// The aperture's shape — what an out-of-focus highlight becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Bokeh {
    #[default]
    Circle,
    /// A six-bladed aperture's hexagon.
    Blades,
    /// A mirror lens's donut.
    Ring,
    /// Discs clipped toward the frame's edges — a fast lens's optical vignetting.
    CatEye,
}

/// ⚠ **Hand-written, not derived**: the container-level `#[serde(default)]` fills a missing field from
/// the struct's `Default`, so a derived one would give a partial block Amount 0 and Boost 0.
impl Default for LensBlur {
    fn default() -> Self {
        Self {
            on: false,
            amount: 50.0,
            focus: LensFocus::Subject,
            focal_range: 10.0,
            bokeh: Bokeh::Circle,
            boost: 25.0,
        }
    }
}

impl LensBlur {
    /// Whether this is exactly the default — what decides it is not written.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether it changes the picture: switched on with some blur.
    pub fn is_active(&self) -> bool {
        self.on && self.amount > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_block_loads_the_hand_written_default() {
        let lb: LensBlur = serde_json::from_str("{}").unwrap();
        assert_eq!(lb, LensBlur::default());
        assert_eq!(
            (lb.on, lb.amount, lb.focal_range, lb.boost),
            (false, 50.0, 10.0, 25.0)
        );
        assert_eq!((lb.focus, lb.bokeh), (LensFocus::Subject, Bokeh::Circle));
    }

    #[test]
    fn a_lens_blur_round_trips() {
        let lb = LensBlur {
            on: true,
            amount: 80.0,
            focus: LensFocus::Point { point: [0.3, 0.7] },
            focal_range: 25.0,
            bokeh: Bokeh::CatEye,
            boost: 60.0,
        };
        let back: LensBlur = serde_json::from_str(&serde_json::to_string(&lb).unwrap()).unwrap();
        assert_eq!(back, lb);
    }

    #[test]
    fn only_the_default_is_default_and_only_on_with_blur_is_active() {
        let d = LensBlur::default();
        assert!(d.is_default() && !d.is_active());
        let on = LensBlur { on: true, ..d };
        assert!(!on.is_default() && on.is_active());
        assert!(
            !LensBlur { amount: 0.0, ..on }.is_active(),
            "on with no blur changes nothing"
        );
        assert!(!LensBlur { boost: 30.0, ..d }.is_default());
    }
}

//! White balance, the tone curves and the parametric curve.

use serde::{Deserialize, Serialize};

/// White balance as an absolute instrument: the illuminant declared for the scene.
///
/// ⚠⚠ `kelvin: None` is **the file's own baseline** — the as-shot temperature for a RAW, D65 for a
/// JPEG the camera has already balanced. That is what keeps `Edits::default()` neutral without
/// this type knowing which kind of file it describes, which is the rule the whole golden harness
/// rests on.
///
/// ⚠ It replaces a relative `temp`/`tint` pair whose coefficients were chosen by hand, and which
/// therefore described a *shift* rather than a *target* — so a white balance copied or synced
/// between two photos meant nothing. A legacy `temp` key is ignored by serde; every one of the 49
/// real copies stored 0.0, so nothing was converted because nothing had ever been set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WhiteBalance {
    /// Kelvin. `None` = the file's own baseline.
    pub kelvin: Option<f32>,
    /// Green–magenta, perpendicular to the locus. Positive = magenta, negative = green.
    pub tint: f32,
}

impl WhiteBalance {
    /// True when this changes pixels — the rule `NoiseReduction::is_active` established.
    pub fn is_active(&self) -> bool {
        self.kelvin.is_some() || self.tint != 0.0
    }
}

/// A point tone curve. Empty (or absent) = identity. Points are (x, y) in [0, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToneCurve {
    pub points: Vec<[f32; 2]>,
}

/// Lightroom's parametric tone curve: four region amounts over three movable splits (M5.1c).
///
/// ⚠⚠ **Its own representation, beside the point curve, by the author's decision (2026-09-26).**
/// A region slider cannot be read back out of hand-placed points, so folding it into
/// [`ToneCurve`] would leave the sliders unable to show an honest value. The pipeline folds both
/// into one table: this curve first, then the point curve.
///
/// ⚠ Defined on the **display-referred** axis (the curve widget's), so "shadows" is the widget's
/// left quarter. The point curve's `x` is linear light; the stage converts between the two.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParametricCurve {
    /// -100..=100 each.
    pub shadows: f32,
    pub darks: f32,
    pub lights: f32,
    pub highlights: f32,
    /// The region boundaries in display-referred `x`, strictly increasing.
    pub splits: [f32; 3],
}

impl Default for ParametricCurve {
    fn default() -> Self {
        Self {
            shadows: 0.0,
            darks: 0.0,
            lights: 0.0,
            highlights: 0.0,
            splits: [0.25, 0.5, 0.75],
        }
    }
}

impl ParametricCurve {
    /// Whether the curve changes anything. ⚠ The splits alone are modifiers, like Detail and
    /// Smoothness beside noise reduction's amounts: they insert nothing.
    pub fn is_active(&self) -> bool {
        self.shadows != 0.0 || self.darks != 0.0 || self.lights != 0.0 || self.highlights != 0.0
    }
}

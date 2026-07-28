//! The non-destructive edit description (the "op-list" / sidecar model).
//! Serialized to JSON alongside the original; the pipeline is built from it.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Creative white balance layered on the camera WB. `temp`/`tint` in ~[-100, 100], 0 = neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WhiteBalance {
    /// Positive = warmer (more red, less blue).
    pub temp: f32,
    /// Positive = magenta, negative = green.
    pub tint: f32,
}

/// A point tone curve. Empty (or absent) = identity. Points are (x, y) in [0, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToneCurve {
    pub points: Vec<[f32; 2]>,
}

/// Normalized crop rectangle in [0, 1]; default is the full frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

/// Geometry: straighten angle (degrees, + = clockwise) then crop. Neutral by default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Geometry {
    /// Straighten angle in degrees. 0 = none.
    pub angle: f32,
    pub crop: CropRect,
}

/// All edits applied to one image. Every field is neutral by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Edits {
    /// Schema version for future migrations.
    pub version: u32,
    pub white_balance: WhiteBalance,
    /// Exposure in stops (EV). 0 = no change.
    pub exposure: f32,
    /// Contrast, ~[-100, 100]. 0 = no change.
    pub contrast: f32,
    /// Highlights, ~[-100, 100]. Positive brightens highlights.
    pub highlights: f32,
    /// Shadows, ~[-100, 100]. Positive brightens shadows.
    pub shadows: f32,
    /// Whites endpoint, ~[-100, 100]. Positive lifts the top end.
    pub whites: f32,
    /// Blacks endpoint, ~[-100, 100]. Positive lifts the bottom end.
    pub blacks: f32,
    pub tone_curve: ToneCurve,
    pub geometry: Geometry,
}

impl Edits {
    /// Load edits from a JSON sidecar file.
    pub fn from_json_file(path: &Path) -> Result<Self, EditLoadError> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Write edits to a JSON sidecar file (pretty-printed).
    pub fn to_json_file(&self, path: &Path) -> Result<(), EditLoadError> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EditLoadError {
    #[error("reading edits file: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing edits JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_default_roundtrips_json() {
        let edits = Edits::default();
        let json = serde_json::to_string(&edits).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(edits, back);
    }

    #[test]
    fn empty_object_deserializes_to_default() {
        let back: Edits = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Edits::default());
    }

    #[test]
    fn neutral_is_default_and_all_zero() {
        let e = Edits::default();
        assert_eq!(e.exposure, 0.0);
        assert_eq!(e.white_balance, WhiteBalance::default());
        assert!(e.tone_curve.points.is_empty());
    }

    #[test]
    fn partial_json_fills_neutral_defaults() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 1.5}"#).unwrap();
        assert_eq!(e.exposure, 1.5);
        assert_eq!(e.contrast, 0.0);
    }

    #[test]
    fn geometry_default_is_neutral() {
        let g = Geometry::default();
        assert_eq!(g.angle, 0.0);
        assert_eq!(g.crop, CropRect::default());
        assert_eq!(
            g.crop,
            CropRect {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0
            }
        );
    }

    #[test]
    fn partial_geometry_json_defaults_full_crop() {
        let e: Edits = serde_json::from_str(r#"{"geometry": {"angle": 2.5}}"#).unwrap();
        assert_eq!(e.geometry.angle, 2.5);
        assert_eq!(e.geometry.crop, CropRect::default());
    }

    #[test]
    fn json_file_roundtrip() {
        let e = Edits {
            exposure: 1.25,
            geometry: Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let path = std::env::temp_dir().join("rustroom_edits_roundtrip.json");
        e.to_json_file(&path).unwrap();
        let back = Edits::from_json_file(&path).unwrap();
        assert_eq!(e, back);
        let _ = std::fs::remove_file(&path);
    }
}

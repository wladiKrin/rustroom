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

/// One hue band's adjustments. All ~[-100, 100]; 0 = neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HslBand {
    /// Hue rotation for this band; ±100 maps to ±30°.
    pub hue: f32,
    /// Chroma scale; ±100 maps to ×0 … ×2.
    pub saturation: f32,
    /// Lightness scale; ±100 maps to ×0.5 … ×1.5.
    pub luminance: f32,
}

/// Per-hue adjustments for the 8 bands, in [`HSL_BAND_NAMES`] order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Hsl {
    pub bands: [HslBand; 8],
}

/// Band names in `Hsl::bands` order — the one source of truth shared by the pipeline
/// (band centers) and the GUI (slider labels). The sidecar stores bands positionally, so
/// this order is part of the file format.
pub const HSL_BAND_NAMES: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// Noise reduction. The two strengths are 0..100 and default to 0 (off); `detail` and
/// `smoothness` are quality modifiers that default to 50 and do nothing on their own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    /// Luminance NR strength, 0..100. 0 = off.
    pub luminance: f32,
    /// How hard luminance detail is protected, 0..100. Higher keeps more texture — and
    /// more noise with it. Inert while `luminance` is 0.
    pub detail: f32,
    /// Color (chroma) NR strength, 0..100. 0 = off.
    pub color: f32,
    /// Chroma blur radius, 0..100. Higher removes larger color blotches. Inert while
    /// `color` is 0.
    pub smoothness: f32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            detail: 50.0,
            color: 0.0,
            smoothness: 50.0,
        }
    }
}

impl NoiseReduction {
    /// True when these settings actually change pixels.
    ///
    /// Deliberately *not* `self != &Self::default()`: `detail` and `smoothness` only
    /// modify how the two strengths behave, so moving one on its own must not insert a
    /// stage into the pipeline.
    pub fn is_active(&self) -> bool {
        self.luminance > 0.0 || self.color > 0.0
    }
}

/// Post-crop vignette: darken or brighten the frame's edges. `amount` 0 = off; the other
/// three only shape the falloff.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// -100 darkens the edges by 2 EV, +100 brightens by 2 EV. 0 = off.
    pub amount: f32,
    /// Where the falloff sits, 0..100. Higher pushes it outward.
    pub midpoint: f32,
    /// Shape: -100 rounded rectangle … 0 ellipse following the frame … +100 true circle.
    pub roundness: f32,
    /// Transition width, 0..100. 0 is a hard edge.
    pub feather: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
        }
    }
}

impl Vignette {
    /// True when this actually changes pixels.
    ///
    /// Amount-only on purpose, the same rule [`NoiseReduction::is_active`] follows: midpoint,
    /// roundness and feather shape an effect that is off until `amount` is non-zero, so
    /// dragging one alone must not insert a stage.
    pub fn is_active(&self) -> bool {
        self.amount != 0.0
    }
}

/// Unsharp-mask sharpening. `amount` 0 = off; the other three only shape the effect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpening {
    /// Strength, 0..100. 0 = off.
    pub amount: f32,
    /// Blur radius (sigma) in **full-resolution pixels**, ~0.5..3.0. Scaled to whatever
    /// resolution is actually being rendered, so a preview approximates the export.
    pub radius: f32,
    /// Halo suppression, 0..100. 100 is a pure unsharp mask; lower values hold back
    /// sharpening where the local contrast is already high. Inert while `amount` is 0.
    pub detail: f32,
    /// Edge gate, 0..100. Higher confines sharpening to edges, sparing flat noisy areas.
    /// 0 = no gate. Inert while `amount` is 0.
    pub masking: f32,
}

impl Default for Sharpening {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
            detail: 50.0,
            masking: 0.0,
        }
    }
}

impl Sharpening {
    /// True when these settings actually change pixels. Amount-only, the same rule
    /// [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow.
    pub fn is_active(&self) -> bool {
        self.amount > 0.0
    }
}

/// Group names in [`EditGroups`] field order — the one source of truth shared by the merge
/// logic, the checkbox UI and the tests. Mirrors [`HSL_BAND_NAMES`].
pub const EDIT_GROUP_NAMES: [&str; 6] = [
    "White Balance",
    "Tone",
    "Color",
    "Detail",
    "Effects",
    "Geometry",
];

/// Which groups of settings a paste or a preset carries.
///
/// [`Default`] is everything **except** geometry: a crop framed for one composition reframes
/// another, so carrying it across photos is usually wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditGroups {
    pub white_balance: bool,
    /// Exposure, contrast, highlights, shadows, whites, blacks, tone curve.
    pub tone: bool,
    /// Saturation, vibrance, HSL.
    pub color: bool,
    /// Noise reduction and sharpening.
    pub detail: bool,
    /// Vignette.
    pub effects: bool,
    /// Crop and straighten.
    pub geometry: bool,
}

impl Default for EditGroups {
    fn default() -> Self {
        Self {
            white_balance: true,
            tone: true,
            color: true,
            detail: true,
            effects: true,
            geometry: false,
        }
    }
}

impl EditGroups {
    pub const ALL: Self = Self {
        white_balance: true,
        tone: true,
        color: true,
        detail: true,
        effects: true,
        geometry: true,
    };

    pub const NONE: Self = Self {
        white_balance: false,
        tone: false,
        color: false,
        detail: false,
        effects: false,
        geometry: false,
    };

    /// Only group `i`, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn only(i: usize) -> Self {
        let mut g = Self::NONE;
        *g.get_mut(i) = true;
        g
    }

    /// Whether group `i` is selected, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get(&self, i: usize) -> bool {
        match i {
            0 => self.white_balance,
            1 => self.tone,
            2 => self.color,
            3 => self.detail,
            4 => self.effects,
            5 => self.geometry,
            _ => panic!("edit group index {i} out of range"),
        }
    }

    /// Mutable access to group `i`, for the checkbox UI. Indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get_mut(&mut self, i: usize) -> &mut bool {
        match i {
            0 => &mut self.white_balance,
            1 => &mut self.tone,
            2 => &mut self.color,
            3 => &mut self.detail,
            4 => &mut self.effects,
            5 => &mut self.geometry,
            _ => panic!("edit group index {i} out of range"),
        }
    }
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
    /// Saturation, ~[-100, 100]. -100 = monochrome, 0 = no change, +100 = 2x chroma.
    pub saturation: f32,
    /// Vibrance, ~[-100, 100]. Like saturation, but weighted toward already-muted colors.
    pub vibrance: f32,
    pub hsl: Hsl,
    pub noise_reduction: NoiseReduction,
    pub vignette: Vignette,
    pub sharpening: Sharpening,
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

    /// Overwrite this photo's settings with `src`'s, for the selected groups only.
    ///
    /// `version` is a schema marker rather than an edit, so it never transfers — the target
    /// keeps its own.
    ///
    /// ⚠ The destructuring below is **exhaustive on purpose**. Adding a field to `Edits`
    /// breaks this function until whoever adds it decides which group owns it. Without that,
    /// a new field would silently never transfer through a paste or a preset, and nothing
    /// would fail — the same class of bug `PrefixKey` guards against in the pipeline.
    pub fn merge_from(&mut self, src: &Edits, groups: EditGroups) {
        let Edits {
            version: _,
            white_balance,
            exposure,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            tone_curve,
            saturation,
            vibrance,
            hsl,
            noise_reduction,
            sharpening,
            vignette,
            geometry,
        } = src;

        if groups.white_balance {
            self.white_balance = *white_balance;
        }
        if groups.tone {
            self.exposure = *exposure;
            self.contrast = *contrast;
            self.highlights = *highlights;
            self.shadows = *shadows;
            self.whites = *whites;
            self.blacks = *blacks;
            self.tone_curve = tone_curve.clone();
        }
        if groups.color {
            self.saturation = *saturation;
            self.vibrance = *vibrance;
            self.hsl = *hsl;
        }
        if groups.detail {
            self.noise_reduction = *noise_reduction;
            self.sharpening = *sharpening;
        }
        if groups.effects {
            self.vignette = *vignette;
        }
        if groups.geometry {
            self.geometry = *geometry;
        }
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
    fn hsl_defaults_to_eight_neutral_bands() {
        let e = Edits::default();
        assert_eq!(e.hsl.bands.len(), 8);
        assert!(e.hsl.bands.iter().all(|b| *b == HslBand::default()));
        assert_eq!(HSL_BAND_NAMES.len(), 8);
    }

    #[test]
    fn old_sidecar_without_hsl_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.hsl, Hsl::default());
    }

    #[test]
    fn hsl_roundtrips_json() {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = -40.0;
        hsl.bands[5].hue = 25.0;
        hsl.bands[3].luminance = 60.0;
        let e = Edits {
            hsl,
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn color_fields_default_to_neutral() {
        let e = Edits::default();
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn old_sidecar_without_color_fields_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5, "contrast": 10.0}"#).unwrap();
        assert_eq!(e.exposure, 0.5);
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn color_fields_roundtrip_json() {
        let e = Edits {
            saturation: 25.0,
            vibrance: -40.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn noise_reduction_defaults_to_off_with_midpoint_modifiers() {
        let nr = NoiseReduction::default();
        assert_eq!(nr.luminance, 0.0);
        assert_eq!(nr.color, 0.0);
        // The two modifiers sit at their midpoint, not zero: they describe *how* to
        // denoise, and zero would mean "smooth everything" / "smallest radius".
        assert_eq!(nr.detail, 50.0);
        assert_eq!(nr.smoothness, 50.0);
        assert_eq!(Edits::default().noise_reduction, nr);
    }

    #[test]
    fn noise_reduction_is_active_only_when_a_strength_is_up() {
        assert!(!NoiseReduction::default().is_active());
        // Modifiers alone must NOT activate the stage — otherwise dragging Detail would
        // insert a stage that costs an Oklab round trip and changes nothing.
        assert!(!NoiseReduction {
            detail: 100.0,
            smoothness: 0.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            luminance: 1.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            color: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_noise_reduction_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.noise_reduction, NoiseReduction::default());
    }

    #[test]
    fn partial_noise_reduction_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"noise_reduction": {"color": 40.0}}"#).unwrap();
        assert_eq!(e.noise_reduction.color, 40.0);
        assert_eq!(e.noise_reduction.luminance, 0.0);
        assert_eq!(e.noise_reduction.detail, 50.0);
        assert_eq!(e.noise_reduction.smoothness, 50.0);
    }

    #[test]
    fn noise_reduction_roundtrips_json() {
        let e = Edits {
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                detail: 70.0,
                color: 55.0,
                smoothness: 20.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn vignette_defaults_to_off() {
        let v = Vignette::default();
        assert_eq!(v.amount, 0.0);
        // Midpoint and feather sit at the middle of their 0..100 range; roundness's
        // neutral is 0, which is also the middle of its -100..100 range.
        assert_eq!(v.midpoint, 50.0);
        assert_eq!(v.feather, 50.0);
        assert_eq!(v.roundness, 0.0);
        assert_eq!(Edits::default().vignette, v);
    }

    #[test]
    fn vignette_is_active_only_on_amount() {
        assert!(!Vignette::default().is_active());
        // The other three only shape an effect that is off until amount is non-zero.
        assert!(!Vignette {
            midpoint: 10.0,
            roundness: -100.0,
            feather: 0.0,
            ..Default::default()
        }
        .is_active());
        assert!(Vignette {
            amount: -1.0,
            ..Default::default()
        }
        .is_active());
        assert!(Vignette {
            amount: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_vignette_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.vignette, Vignette::default());
    }

    #[test]
    fn partial_vignette_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"vignette": {"amount": -40.0}}"#).unwrap();
        assert_eq!(e.vignette.amount, -40.0);
        assert_eq!(e.vignette.midpoint, 50.0);
        assert_eq!(e.vignette.feather, 50.0);
        assert_eq!(e.vignette.roundness, 0.0);
    }

    #[test]
    fn vignette_roundtrips_json() {
        let e = Edits {
            vignette: Vignette {
                amount: -55.0,
                midpoint: 30.0,
                roundness: 80.0,
                feather: 20.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn sharpening_defaults_to_off() {
        let s = Sharpening::default();
        assert_eq!(s.amount, 0.0);
        assert_eq!(s.masking, 0.0);
        // Radius is a physical pixel quantity, not a 0..100 slider; 1.0 px is the usual
        // starting point. Detail sits at its midpoint like every other modifier.
        assert_eq!(s.radius, 1.0);
        assert_eq!(s.detail, 50.0);
        assert_eq!(Edits::default().sharpening, s);
    }

    #[test]
    fn sharpening_is_active_only_on_amount() {
        assert!(!Sharpening::default().is_active());
        // Same rule as NoiseReduction and Vignette: the modifiers shape an effect that is
        // off until amount is non-zero, so dragging one alone must not insert a stage.
        assert!(!Sharpening {
            radius: 3.0,
            detail: 0.0,
            masking: 90.0,
            ..Default::default()
        }
        .is_active());
        assert!(Sharpening {
            amount: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_sharpening_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.sharpening, Sharpening::default());
    }

    #[test]
    fn partial_sharpening_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"sharpening": {"amount": 60.0}}"#).unwrap();
        assert_eq!(e.sharpening.amount, 60.0);
        assert_eq!(e.sharpening.radius, 1.0);
        assert_eq!(e.sharpening.detail, 50.0);
        assert_eq!(e.sharpening.masking, 0.0);
    }

    #[test]
    fn sharpening_roundtrips_json() {
        let e = Edits {
            sharpening: Sharpening {
                amount: 70.0,
                radius: 2.5,
                detail: 30.0,
                masking: 80.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
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

    /// An `Edits` with every field non-neutral, so a field that fails to transfer shows up.
    ///
    /// Deliberately does NOT use `..Default::default()`: adding a field to `Edits` should
    /// break this function, forcing whoever adds it to give it a group.
    fn all_nonneutral() -> Edits {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            version: 7,
            white_balance: WhiteBalance {
                temp: 10.0,
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            sharpening: Sharpening {
                amount: 60.0,
                ..Default::default()
            },
            vignette: Vignette {
                amount: -30.0,
                ..Default::default()
            },
            geometry: Geometry {
                angle: 2.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
            },
        }
    }

    #[test]
    fn merging_all_groups_reproduces_the_source() {
        // ⚠ The coverage half of the partition proof. A field belonging to NO group would
        // silently never transfer — and this is what catches it, because the result would
        // keep the target's default for that field.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got, Edits { version: 0, ..src });
    }

    #[test]
    fn version_never_transfers() {
        // `version` is a schema marker, not an edit. The target keeps its own.
        let src = all_nonneutral();
        assert_ne!(src.version, 0);
        let mut got = Edits {
            version: 3,
            ..Edits::default()
        };
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got.version, 3);
    }

    #[test]
    fn merging_no_groups_changes_nothing() {
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::NONE);
        assert_eq!(got, Edits::default());
    }

    #[test]
    fn every_group_transfers_something() {
        // A group that moves no fields is dead weight, and would make the disjointness
        // test below pass vacuously for that group.
        let src = all_nonneutral();
        for (i, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut got = Edits::default();
            got.merge_from(&src, EditGroups::only(i));
            assert_ne!(got, Edits::default(), "group {name} transferred nothing");
        }
    }

    #[test]
    fn the_groups_are_disjoint() {
        // ⚠ The other half of the partition proof, and it needs no field list of its own.
        //
        // Clear group `g` from a fully non-neutral source. If some field belonged to both
        // `g` and `h`, clearing `g` would change what `h` transfers — so comparing group
        // `h`'s transfer before and after catches any overlap generically.
        let src = all_nonneutral();
        for (g, g_name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut cleared = src.clone();
            cleared.merge_from(&Edits::default(), EditGroups::only(g));
            for (h, h_name) in EDIT_GROUP_NAMES.iter().enumerate() {
                if h == g {
                    continue;
                }
                let mut from_src = Edits::default();
                from_src.merge_from(&src, EditGroups::only(h));
                let mut from_cleared = Edits::default();
                from_cleared.merge_from(&cleared, EditGroups::only(h));
                assert_eq!(
                    from_src, from_cleared,
                    "clearing {g_name} changed what {h_name} transfers — the groups overlap"
                );
            }
        }
    }

    #[test]
    fn clearing_then_restoring_a_group_is_a_round_trip() {
        // Proves group `g` covers *exactly* the fields that `cleared` lost — not fewer.
        let src = all_nonneutral();
        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut round = src.clone();
            round.merge_from(&Edits::default(), EditGroups::only(g));
            assert_ne!(round, src, "group {name} did nothing");
            round.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                round, src,
                "group {name} did not restore everything it cleared"
            );
        }
    }

    #[test]
    fn the_default_selection_excludes_geometry() {
        // A crop framed for one composition reframes another, and a preset built on a 3:2
        // crop would reshape every photo it touches.
        let g = EditGroups::default();
        assert!(!g.geometry);
        assert!(g.white_balance && g.tone && g.color && g.detail && g.effects);
    }

    #[test]
    fn edit_groups_roundtrip_json_and_default_when_absent() {
        let g = EditGroups {
            geometry: true,
            tone: false,
            ..EditGroups::default()
        };
        let back: EditGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(g, back);
        // An older file missing the field entirely still loads.
        let partial: EditGroups = serde_json::from_str(r#"{"geometry": true}"#).unwrap();
        assert!(partial.geometry);
        assert_eq!(partial.tone, EditGroups::default().tone);
    }

    #[test]
    fn group_accessors_match_the_name_table() {
        let mut g = EditGroups::NONE;
        for i in 0..EDIT_GROUP_NAMES.len() {
            assert!(!g.get(i));
            *g.get_mut(i) = true;
            assert!(g.get(i));
        }
        assert_eq!(g, EditGroups::ALL);
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

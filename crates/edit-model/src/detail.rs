//! Detail and presence: noise reduction, sharpening, local contrast, dehaze, defringe, vignette.

use serde::{Deserialize, Serialize};

/// AI denoise (M10.16b): how much of the network's result replaces the photo — 0 off, 100 all of it.
/// The strength (σ) is measured from the photo, never set here; the result itself is cached, not
/// stored in the edits.
///
/// ⚠ **Off is never written** (`Edits::ai_denoise`'s `skip_serializing_if`) — `negative`'s reason: a
/// default block would change every rendered preview's key on disk.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiDenoise {
    /// 0..=100.
    pub amount: f32,
}

impl AiDenoise {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether it changes the picture — once its result exists.
    pub fn is_active(&self) -> bool {
        self.amount > 0.0
    }
}

/// Manual defringe (M9.6): removes the purple and green halos longitudinal chromatic aberration
/// leaves beside high-contrast edges, which a lens profile's lateral (TCA) correction cannot touch.
///
/// Amounts are 0..100 and default to 0 (off). The hue ranges are **Oklab hue in degrees**,
/// `[lo, hi]` with `lo < hi`, and are modifiers: moving one on its own inserts nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defringe {
    pub purple_amount: f32,
    pub purple_hue: [f32; 2],
    pub green_amount: f32,
    pub green_hue: [f32; 2],
}

impl Defringe {
    pub const DEFAULT_PURPLE_HUE: [f32; 2] = [280.0, 340.0];
    pub const DEFAULT_GREEN_HUE: [f32; 2] = [110.0, 170.0];

    /// True when an amount is non-zero. ⚠ Not `!= default()`: the ranges alone do nothing.
    pub fn is_active(&self) -> bool {
        self.purple_amount > 0.0 || self.green_amount > 0.0
    }
}

impl Default for Defringe {
    fn default() -> Self {
        Self {
            purple_amount: 0.0,
            purple_hue: Self::DEFAULT_PURPLE_HUE,
            green_amount: 0.0,
            green_hue: Self::DEFAULT_GREEN_HUE,
        }
    }
}

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

/// Film grain (M10.11b) — Lightroom's Effects → Grain: brightness only, strongest in the midtones.
///
/// ⚠ In the vignette's group, which the panel and Copy/Paste/Sync show as **Effects** — Lightroom's
/// arrangement. The group's *field* is still `vignette`, so every stored selection reads as before.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    /// 0..100. 0 = off.
    pub amount: f32,
    /// How large each grain is, 0..100.
    pub size: f32,
    /// How uneven the grain is, 0..100.
    pub roughness: f32,
}

impl Default for Grain {
    /// ⚠ Hand-written: Lightroom's 25 and 50, which a derived `Default` would make 0 for every file
    /// written before grain existed.
    fn default() -> Self {
        Self {
            amount: 0.0,
            size: 25.0,
            roughness: 50.0,
        }
    }
}

impl Grain {
    /// Grain only when there is an amount — Size and Roughness only shape it.
    pub fn is_active(&self) -> bool {
        self.amount > 0.0
    }

    /// Exactly the default — what `Edits` leaves out of its serialization.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
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

/// Texture and Clarity: local contrast in two frequency bands. Both 0 = off.
///
/// Same operation as [`Sharpening`] at a different radius, which is why they share a stage
/// shape but not a struct: these are *look* controls that belong with tone, while sharpening
/// is capture- and output-specific.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalContrast {
    /// Fine-band local contrast, ~[-100, 100]. Positive brings out surface detail, negative
    /// smooths it. 0 = off.
    pub texture: f32,
    /// Coarse-band local contrast, ~[-100, 100]. Positive is punchy midtone contrast,
    /// negative is a glow. 0 = off.
    pub clarity: f32,
}

impl LocalContrast {
    /// True when these settings actually change pixels.
    ///
    /// There are no modifier fields here, so this could have been `!= default()`. Written
    /// the same way [`NoiseReduction::is_active`], [`Vignette::is_active`] and
    /// [`Sharpening::is_active`] are, so the four read alike and so adding a modifier later
    /// does not change the rule.
    pub fn is_active(&self) -> bool {
        self.texture != 0.0 || self.clarity != 0.0
    }
}

/// Haze removal, via the dark-channel prior. 0 = off.
///
/// Runs in the pipeline **prefix**, beside noise reduction, because it is a correction of the
/// capture rather than a grade — and because a frame-global airlight can only be estimated
/// where the whole frame is present.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Dehaze {
    /// ~[-100, 100]. Positive removes the estimated haze, negative lays it back on —
    /// depth-aware in both directions, since both use the same transmission map. 0 = off.
    pub amount: f32,
}

impl Dehaze {
    /// True when these settings actually change pixels. Same rule as every other stage.
    pub fn is_active(&self) -> bool {
        self.amount != 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M10.11b: grain is off by default with Lightroom's Size and Roughness — a derived `Default`
    /// would give both 0 to every file written before it.
    #[test]
    fn grain_is_off_by_default_with_lightrooms_size_and_roughness() {
        let g = Grain::default();
        assert!(!g.is_active() && g.is_default());
        assert_eq!((g.amount, g.size, g.roughness), (0.0, 25.0, 50.0));
        assert!(Grain { amount: 10.0, ..g }.is_active());
        assert!(
            !Grain { size: 80.0, ..g }.is_active(),
            "size alone draws no grain"
        );
    }
    use crate::{EditGroups, Edits, EDIT_GROUP_NAMES};

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
    fn local_contrast_defaults_to_off() {
        let lc = LocalContrast::default();
        assert_eq!(lc.texture, 0.0);
        assert_eq!(lc.clarity, 0.0);
        assert_eq!(Edits::default().local_contrast, lc);
    }

    #[test]
    fn local_contrast_is_active_when_either_slider_moves() {
        assert!(!LocalContrast::default().is_active());
        assert!(LocalContrast {
            texture: 1.0,
            clarity: 0.0
        }
        .is_active());
        assert!(LocalContrast {
            texture: 0.0,
            clarity: -1.0
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_local_contrast_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.local_contrast, LocalContrast::default());
    }

    #[test]
    fn local_contrast_roundtrips_json() {
        let e = Edits {
            local_contrast: LocalContrast {
                texture: 35.0,
                clarity: -60.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn dehaze_defaults_to_off() {
        assert_eq!(Dehaze::default().amount, 0.0);
        assert_eq!(Edits::default().dehaze, Dehaze::default());
    }

    #[test]
    fn dehaze_is_active_in_both_directions() {
        assert!(!Dehaze::default().is_active());
        assert!(Dehaze { amount: 1.0 }.is_active());
        assert!(Dehaze { amount: -1.0 }.is_active());
    }

    #[test]
    fn old_sidecar_without_dehaze_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.dehaze, Dehaze::default());
    }

    #[test]
    fn dehaze_roundtrips_json() {
        let e = Edits {
            dehaze: Dehaze { amount: -45.0 },
            ..Default::default()
        };
        let back: Edits = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }

    /// M9.6: a sidecar from before defringe loads it off, the hue ranges alone insert nothing, and
    /// it rides the Lens group — ⚠ both directions, so a group that always or never carried it fails.
    #[test]
    fn defringe_loads_neutral_is_active_on_amounts_and_rides_the_lens_group() {
        let e: Edits = serde_json::from_str("{}").unwrap();
        assert_eq!(e.defringe, Defringe::default());
        assert!(!e.defringe.is_active());
        let ranged = Defringe {
            purple_hue: [250.0, 300.0],
            ..Default::default()
        };
        assert!(!ranged.is_active(), "a hue range alone must insert nothing");
        assert!(Defringe {
            green_amount: 1.0,
            ..Default::default()
        }
        .is_active());
        let src = Edits {
            defringe: Defringe {
                purple_amount: 40.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lens_group = EDIT_GROUP_NAMES.iter().position(|n| *n == "Lens").unwrap();
        let mut lens = Edits::default();
        lens.merge_from(&src, EditGroups::only(lens_group));
        assert_eq!(lens.defringe, src.defringe);
        let mut rest = Edits::default();
        let mut all_but = EditGroups::ALL;
        all_but.lens = false;
        rest.merge_from(&src, all_but);
        assert_eq!(rest.defringe, Defringe::default());
    }
}

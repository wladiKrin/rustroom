//! Colour: HSL, colour grading, the camera rendering and the creative look.

use serde::{Deserialize, Serialize};

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

/// One colour-grading wheel: a hue/saturation tint direction plus a lightness offset,
/// applied to one tonal range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GradeWheel {
    /// Oklab hue angle in degrees, 0..360, measured from the +a axis counterclockwise —
    /// i.e. exactly `atan2(b, a)`, with no offset to get wrong.
    ///
    /// ⚠ This is the **same angle** `BAND_HUES` uses in the HSL stage, so "red" means the
    /// same thing in both halves of the Color section. It would not if one were an sRGB hue.
    pub hue: f32,
    /// Tint strength, 0..100. 0 = no tint, whatever the hue.
    pub saturation: f32,
    /// Lightness offset for this range, -100..100.
    pub luminance: f32,
}

/// Three-way colour grading: a tint per tonal range, plus one applied everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
    /// Applied uniformly at every lightness, on top of the weighted three.
    pub global: GradeWheel,
    /// How far the three ranges overlap, 0..100. At 0 the nearest range wins outright; at
    /// 100 all three weigh 1/3 everywhere and the grade becomes a uniform tint.
    pub blending: f32,
    /// Slides all three range centres, -100..100. Positive lets the highlight tint reach
    /// further down.
    pub balance: f32,
}

impl Default for ColorGrading {
    /// ⚠ Hand-written, **not** derived: `blending` is 50 at neutral. The container-level
    /// `#[serde(default)]` fills a missing field from this, so a derived `Default` would give
    /// every partial sidecar `blending: 0` — hard-edged ranges — with nothing to notice.
    fn default() -> Self {
        Self {
            shadows: GradeWheel::default(),
            midtones: GradeWheel::default(),
            highlights: GradeWheel::default(),
            global: GradeWheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    /// True when this actually changes pixels.
    ///
    /// Wheels only, the rule [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow:
    /// `blending` and `balance` shape ranges that tint nothing until a wheel is turned up, and
    /// a `hue` with zero `saturation` is a direction with no magnitude.
    pub fn is_active(&self) -> bool {
        [self.shadows, self.midtones, self.highlights, self.global]
            .iter()
            .any(|w| w.saturation != 0.0 || w.luminance != 0.0)
    }
}

/// How a scene-referred photograph is mapped onto a display.
///
/// ⚠⚠ **A rendering is not an adjustment.** Every slider in this pipeline moves a photograph that
/// is already in a defined state; a rendering *establishes* that state — it turns scene-referred
/// light into something a screen can show. That is why it curves each channel independently
/// rather than preserving hue like every other tonal stage, and why "Reset adjustments" does not
/// touch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Rendering {
    /// Colorimetric only — the primaries and the transfer curve, nothing else. What Rustroom did
    /// before this existed, and what every golden and the CLI still get.
    #[default]
    None,
    /// The camera's rendering: the profile's tone curve when it supplies a usable one, else the
    /// built-in filmic curve.
    Camera,
}

/// The colour space a `.cube` was authored against.
///
/// ⚠⚠ **This decides WHERE the look runs**, not merely how it is shaped — see the spec
/// `2026-09-04-creative-luts-design.md` §2. A display-referred LUT *modifies* the camera
/// rendering; a linear one runs **in place of** it, because it **is** the rendering. Stacking a
/// film emulation on our S-curve double-renders — our tone curve, then the LUT's — giving crushed
/// shadows and a clipped roll-off, which is the single most common way LUT support is reported as
/// "looks wrong".
///
/// ⚠ Log encodings (LogC, S-Log3, ACEScct) are deliberately absent: each is a specific transfer
/// function needing implementation *and* validation, and there is no footage here to validate
/// against. Add a variant when there is something to check it with — never before. That is the
/// position colour management C2 was in without its oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LutSpace {
    /// sRGB primaries and the sRGB transfer curve. What most downloadable looks expect.
    #[default]
    SrgbDisplay,
    /// Rec.709 primaries with **its own** transfer curve — not sRGB's, though the two are
    /// routinely confused. Kept distinct rather than aliased because a LUT author chose one.
    Rec709,
    /// Scene-linear. A LUT declaring this **is** the display rendering.
    Linear,
}

impl LutSpace {
    /// Every variant, for the panel's dropdown. ⚠ Append only — the order is what the UI shows.
    pub const ALL: [LutSpace; 3] = [LutSpace::SrgbDisplay, LutSpace::Rec709, LutSpace::Linear];

    pub fn label(self) -> &'static str {
        match self {
            LutSpace::SrgbDisplay => "sRGB display",
            LutSpace::Rec709 => "Rec.709",
            LutSpace::Linear => "Linear",
        }
    }

    /// Whether a LUT in this space **replaces** the camera rendering rather than following it.
    ///
    /// ⚠⚠ The one place this rule is written down. The pipeline's placement and the panel's
    /// warning both read it here, so the render and the label it shows cannot disagree — the
    /// invisible-state failure this project has recorded repeatedly.
    pub fn is_the_rendering(self) -> bool {
        matches!(self, LutSpace::Linear)
    }
}

/// A creative look: a 3-D LUT applied as a display-referred grade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The `.cube`'s filename, resolved against the looks directory.
    ///
    /// ⚠ A NAME, not a path: a preset carrying a look must be shareable, and an absolute path is
    /// machine-specific. Camera profiles already behave this way. The accepted cost is that a
    /// preset is portable only where the `.cube` is present — a missing file renders **without**
    /// the look and says so, never silently and never as an error that stops the photo opening.
    pub file: Option<String>,
    pub space: LutSpace,
    /// 0..=100. 100 is the LUT exactly as authored.
    ///
    /// ⚠ Default **100** so that applying a look does what its name says. A lower default means a
    /// LUT never looks like what its author shipped until you find the slider.
    pub strength: f32,
}

impl Default for Look {
    /// ⚠⚠ **Hand-written, not derived.** `strength` is 100 at neutral, and the container-level
    /// `#[serde(default)]` fills a *missing field* from this — so a derived `Default` would give
    /// every partial sidecar `strength: 0`, i.e. a named look that silently does nothing.
    /// `RadialGradient::default` records this trap twice.
    fn default() -> Self {
        Self {
            file: None,
            space: LutSpace::default(),
            strength: 100.0,
        }
    }
}

impl Look {
    /// True when this look actually changes pixels. Same rule as every other stage.
    pub fn is_active(&self) -> bool {
        self.file.is_some() && self.strength > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditGroups, Edits, LensCorrection, RENDERING_GROUP};

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

    /// ⚠⚠ `None` by default is what keeps `Edits::default()` strictly neutral, and therefore
    /// what keeps all 53 goldens and the CLI untouched. A default of `Camera` would move every
    /// one of them.
    #[test]
    fn the_default_rendering_is_undecided_and_applies_as_colorimetric() {
        assert_eq!(Edits::default().rendering, None);
        assert_eq!(Edits::default().applied_rendering(), Rendering::None);
        assert_eq!(Edits::default().lens_correction, None);
        assert!(!Edits::default().applied_lens_correction().is_active());
    }

    /// ⚠⚠ THE DISTINCTION THIS CHANGE EXISTS FOR: decided off is not the default. Before it, a
    /// photo whose rendering was switched off read as neutral, its sidecar was deleted, and the
    /// preference switched it back on at the next open.
    #[test]
    fn a_decided_off_is_not_the_default() {
        let off = Edits {
            rendering: Some(Rendering::None),
            ..Default::default()
        };
        assert_ne!(
            off,
            Edits::default(),
            "decided-off rendering read as neutral"
        );
        assert_eq!(
            off.applied_rendering(),
            Rendering::None,
            "but still applies as off"
        );
        let lens_off = Edits {
            lens_correction: Some(LensCorrection::default()),
            ..Default::default()
        };
        assert_ne!(
            lens_off,
            Edits::default(),
            "decided-off lens read as neutral"
        );
        assert!(!lens_off.applied_lens_correction().is_active());
    }

    /// ⚠ Serde ignores what it does not know, so a sidecar from before either field reads as
    /// *undecided* — which applies as off, exactly what those photos were rendered with.
    #[test]
    fn a_sidecar_written_before_the_rendering_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure":0.5}"#).expect("parses");
        assert_eq!(e.rendering, None);
        assert_eq!(e.lens_correction, None);
        assert_eq!(e.exposure, 0.5);
    }

    /// ⚠ A sidecar that already says `"None"` was written by whoever switched the rendering off,
    /// so *decided off* is the reading they meant — and nothing existing changes meaning.
    #[test]
    fn an_existing_off_in_a_sidecar_reads_as_decided_off() {
        let e: Edits = serde_json::from_str(r#"{"rendering":"None"}"#).unwrap();
        assert_eq!(e.rendering, Some(Rendering::None));
        let e: Edits = serde_json::from_str(r#"{"rendering":"Camera"}"#).unwrap();
        assert_eq!(e.rendering, Some(Rendering::Camera));
        let e: Edits = serde_json::from_str(r#"{"lens_correction":{"distortion":false}}"#).unwrap();
        assert_eq!(e.lens_correction, Some(LensCorrection::default()));
    }

    /// ⚠ Undecided is **absent** from the file, never `null`: a photo that was never asked
    /// writes the same bytes it always did.
    #[test]
    fn an_undecided_field_is_absent_from_the_sidecar() {
        let text = serde_json::to_string(&Edits::default()).unwrap();
        assert!(!text.contains("\"rendering\""), "{text}");
        assert!(!text.contains("\"lens_correction\""), "{text}");
        let decided = Edits {
            rendering: Some(Rendering::None),
            lens_correction: Some(LensCorrection::default()),
            ..Default::default()
        };
        let text = serde_json::to_string(&decided).unwrap();
        assert!(text.contains("\"rendering\":\"None\""), "{text}");
        assert!(text.contains("\"lens_correction\""), "{text}");
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, decided);
    }

    /// ⚠ Included in `EditGroups::default()` because a rendering is a *look* — it should follow
    /// a paste, a preset and a sync, unlike Geometry, Masks and Retouch, which are placements.
    #[test]
    fn the_rendering_travels_on_its_own_group_and_by_default() {
        assert!(EditGroups::default().get(RENDERING_GROUP));

        let src = Edits {
            rendering: Some(Rendering::Camera),
            exposure: 1.0,
            ..Default::default()
        };
        let mut dst = Edits::default();
        dst.merge_from(&src, EditGroups::only(RENDERING_GROUP));
        assert_eq!(
            dst.rendering,
            Some(Rendering::Camera),
            "the rendering did not transfer"
        );
        assert_eq!(dst.exposure, 0.0, "its group carried something else too");
    }

    #[test]
    fn a_sidecar_without_blending_still_gets_fifty() {
        // ⚠ `blending` defaults to 50, not 0, so `ColorGrading::default` must be hand-written.
        // A derived `Default` would give every partial sidecar hard-edged ranges, silently —
        // the same trap `RadialGradient::default` records.
        let e: Edits =
            serde_json::from_str(r#"{"color_grading":{"shadows":{"saturation":40.0}}}"#).unwrap();
        assert_eq!(e.color_grading.blending, 50.0);
        assert_eq!(e.color_grading.shadows.saturation, 40.0);
    }

    #[test]
    fn a_hue_with_no_saturation_is_inert() {
        // A hue is a direction, not a strength. Rotating the wheel with the saturation at zero
        // must not insert a stage.
        let g = ColorGrading {
            shadows: GradeWheel {
                hue: 210.0,
                saturation: 0.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        assert!(!g.is_active());
    }

    #[test]
    fn blending_and_balance_alone_are_inert() {
        // The rule `NoiseReduction::is_active` and `Vignette::is_active` already follow: a
        // modifier shapes an effect that is off until a wheel is turned up.
        let g = ColorGrading {
            blending: 100.0,
            balance: -80.0,
            ..ColorGrading::default()
        };
        assert!(!g.is_active());
    }

    #[test]
    fn any_wheel_with_saturation_or_luminance_is_active() {
        for g in [
            ColorGrading {
                shadows: GradeWheel {
                    saturation: 30.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                midtones: GradeWheel {
                    luminance: -20.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                highlights: GradeWheel {
                    saturation: 5.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                global: GradeWheel {
                    luminance: 1.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
        ] {
            assert!(g.is_active(), "{g:?} should be active");
        }
    }

    /// ⚠⚠ Hand-written `Default`, not derived: `strength` is 100 at neutral, and the
    /// container-level `#[serde(default)]` fills a missing field from the struct's `Default` — so
    /// a derived one gives every partial sidecar `strength: 0`, a look silently doing nothing.
    /// `RadialGradient::default` and `ColorGrading::default` record this trap.
    #[test]
    fn a_looks_default_strength_is_full() {
        assert_eq!(Look::default().strength, 100.0);
        assert_eq!(Look::default().file, None);
    }

    /// ⚠ A look with no file is inert, so `Edits::default()` stays strictly neutral and every
    /// golden is untouched.
    #[test]
    fn a_default_look_is_not_active() {
        assert!(!Look::default().is_active());
    }

    /// ⚠ Strength 0 is not active either — it is the identity by definition, and inserting a
    /// stage that does nothing costs a whole-image pass.
    #[test]
    fn a_look_at_zero_strength_is_not_active() {
        let l = Look {
            file: Some("x.cube".into()),
            strength: 0.0,
            ..Look::default()
        };
        assert!(!l.is_active());
    }

    /// ⚠⚠ The trap made concrete: a sidecar that names a look but omits `strength` must load at
    /// **100**, not 0. This is the test a derived `Default` fails, and the reason it is written
    /// against JSON rather than against `Look::default()` alone is that the failure only appears
    /// through serde's field-level fill-in.
    #[test]
    fn a_sidecar_naming_a_look_without_a_strength_loads_at_full() {
        let e: Edits = serde_json::from_str(r#"{"look":{"file":"portra.cube"}}"#).unwrap();
        assert_eq!(e.look.file.as_deref(), Some("portra.cube"));
        assert_eq!(e.look.strength, 100.0);
        assert!(e.look.is_active(), "a named look must actually be applied");
    }

    /// ⚠ An older sidecar has no `look` key at all and must load inert — the property that keeps
    /// every existing golden byte-identical.
    #[test]
    fn a_sidecar_without_a_look_loads_inert() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.look, Look::default());
        assert!(!e.look.is_active());
    }
}

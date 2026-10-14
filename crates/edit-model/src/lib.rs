//! The non-destructive edit description (the "op-list" / sidecar model).
//! Serialized to JSON alongside the original; the pipeline is built from it.

use serde::{Deserialize, Serialize};
use std::path::Path;

mod colour;
mod detail;
mod geo_group;
mod geometry;
mod groups;
mod lens_blur;
mod local_group;
mod mask_ops;
mod mask_ranges;
mod mask_shapes;
mod masks;
mod negative;
mod retouch;
mod tone;
pub use colour::{
    ColorGrading, GradeWheel, Hsl, HslBand, Look, LutSpace, Rendering, HSL_BAND_NAMES,
};
pub use detail::{Defringe, Dehaze, Grain, LocalContrast, NoiseReduction, Sharpening, Vignette};
pub use geo_group::GeoGroup;
pub use geometry::{CropRect, Geometry, LensCorrection, Orientation, Perspective, Quarter};
pub use groups::{
    EditGroups, EDIT_GROUP_NAMES, GEOMETRY_GROUP, LENS_BLUR_GROUP, MASKS_GROUP, NEGATIVE_GROUP,
    RENDERING_GROUP, RETOUCH_GROUP,
};
pub use lens_blur::{Bokeh, LensBlur, LensFocus};
pub use local_group::LocalGroup;
pub use mask_ops::{MaskOp, Refinement};
pub use mask_ranges::{ColourRange, DepthRange, LuminanceRange};
pub use mask_shapes::{Brush, BrushStroke, LinearGradient, RadialGradient};
pub use masks::{AiMask, AiPrompt, LocalAdjustments, Mask, MaskedAdjustment};
pub use negative::Negative;
pub use retouch::{Eye, EyeKind, Spot, SpotMode};
pub use tone::{ParametricCurve, ToneCurve, WhiteBalance};

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
    /// The red channel's own curve, composed **after** [`Self::tone_curve`].
    ///
    /// ⚠⚠ **The classic grading tool, not a fourth colour control.** Per-channel curves are in
    /// Photoshop, Lightroom, Camera Raw, Capture One, darktable, RawTherapee, Affinity and GIMP; the
    /// shadow/midtone/highlight wheels are the friendlier interface built *over* them. What they add
    /// that [`ColorGrading`] cannot is an **exact crossover point and shape** — lifting only the
    /// deepest shadows in one primary, with a precise knee.
    ///
    /// ⚠ Empty points mean identity, the rule [`Self::tone_curve`] already follows, and it is what
    /// keeps `ToneCurveStage` out of the pipeline. ⚠⚠ The pipeline's guard must therefore test **all
    /// four** curves; naming only `tone_curve` would silently render a channel-only edit unchanged.
    ///
    /// ⚠ In the **`Tone`** group with the composite, so Sync and "Reset adjustments" treat the curve as
    /// one instrument. ⚠ No `version` bump: the container's `#[serde(default)]` loads an older sidecar
    /// with three identity curves, and a test rather than a number is the guard.
    pub tone_curve_red: ToneCurve,
    /// The green channel's own curve. See [`Self::tone_curve_red`].
    pub tone_curve_green: ToneCurve,
    /// The blue channel's own curve. See [`Self::tone_curve_red`].
    pub tone_curve_blue: ToneCurve,
    /// The parametric curve (M5.1c), applied before the point curves. In the **`Tone`** group with
    /// them: one instrument as far as Sync and "Reset adjustments" are concerned.
    pub tone_curve_parametric: ParametricCurve,
    /// Saturation, ~[-100, 100]. -100 = monochrome, 0 = no change, +100 = 2x chroma.
    pub saturation: f32,
    /// Vibrance, ~[-100, 100]. Like saturation, but weighted toward already-muted colors.
    pub vibrance: f32,
    pub hsl: Hsl,
    pub color_grading: ColorGrading,
    pub local_contrast: LocalContrast,
    pub dehaze: Dehaze,
    pub noise_reduction: NoiseReduction,
    pub vignette: Vignette,
    pub sharpening: Sharpening,
    /// The lens corrections. `None` = **never decided**, which applies as none;
    /// `Some(LensCorrection::default())` = **decided off**, a different fact that survives a
    /// photo switch. Read through [`Edits::applied_lens_correction`].
    ///
    /// ⚠ `skip_serializing_if`: undecided is *absent* from the sidecar, never `null`, so a photo
    /// that was never asked writes the same bytes it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_correction: Option<LensCorrection>,
    /// Manual defringe (M9.6). In the **Lens** group, beside the profile corrections.
    pub defringe: Defringe,
    pub perspective: Perspective,
    pub geometry: Geometry,
    /// Local adjustments, each confined to a mask. Applied in order, after the global
    /// per-pixel adjustments and before the vignette.
    pub masks: Vec<MaskedAdjustment>,
    /// How the scene is mapped onto a display. See [`Rendering`].
    ///
    /// ⚠ `None` = **never decided**, which applies as `Rendering::None` — what keeps
    /// `Edits::default()` strictly neutral, and therefore all 59 goldens and the CLI untouched.
    /// `Some(Rendering::None)` = **decided off**: not the default, so a sidecar holding it is
    /// kept, which is the whole fix. The GUI seeds `Some(Settings::default_rendering)` on a
    /// photo with no sidecar, the rule `initial_lens_correction` follows.
    /// Read through [`Edits::applied_rendering`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendering: Option<Rendering>,
    /// Spot removal patches, applied in order on the pre-geometry frame. Empty = no stage.
    pub spots: Vec<Spot>,
    /// A creative look: a `.cube` laid over the finished photograph.
    ///
    /// ⚠ Rides the **`Rendering`** group, not `Color` — a look is a rendering decision. And its
    /// declared [`LutSpace`] decides *where* it runs: a linear LUT replaces the camera rendering
    /// rather than following it.
    ///
    /// ⚠ Inert by default (no file), which is what keeps `Edits::default()` strictly neutral and
    /// all 53 goldens byte-identical.
    #[serde(default)]
    pub look: Look,
    /// Red- and pet-eye corrections (M9.11), applied right after spot removal on the
    /// pre-geometry frame. Empty = no stage. Rides the Retouch group with `spots`.
    pub eyes: Vec<Eye>,
    /// A scanned film negative's inversion (M10.11a), applied right after lens correction. Its own
    /// group, [`NEGATIVE_GROUP`], off by default.
    ///
    /// ⚠⚠ **Not written at all while it is the default.** Every sidecar and every rendered preview's
    /// key is a serialization of `Edits`; writing a default block would change the key of every
    /// preview on disk — `the_fingerprint_is_stable_across_calls_and_pinned` caught it — and grow
    /// every digital photo's sidecar with a film setting it has nothing to do with.
    #[serde(default, skip_serializing_if = "Negative::is_default")]
    pub negative: Negative,
    /// Film grain (M10.11b), the last scene-linear stage. Rides the vignette's group (Effects).
    /// ⚠ Not written while default — `negative`'s reason: a default block would change every preview's
    /// key on disk.
    #[serde(default, skip_serializing_if = "Grain::is_default")]
    pub grain: Grain,
    /// Lens Blur (M10.16d) — a synthetic shallow depth of field on the estimated depth map, its own
    /// group. ⚠ Not written while default — `negative`'s reason.
    #[serde(default, skip_serializing_if = "LensBlur::is_default")]
    pub lens_blur: LensBlur,
}

impl Edits {
    /// What the pipeline applies: the decided rendering, else none.
    ///
    /// ⚠ **The one read rule** — every consumer goes through here, so "undecided renders as
    /// off" is written once.
    pub fn applied_rendering(&self) -> Rendering {
        self.rendering.unwrap_or_default()
    }

    /// What the pipeline applies: the decided lens corrections, else none. Same rule.
    pub fn applied_lens_correction(&self) -> LensCorrection {
        self.lens_correction.unwrap_or_default()
    }
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
            tone_curve_red,
            tone_curve_green,
            tone_curve_blue,
            tone_curve_parametric,
            saturation,
            vibrance,
            hsl,
            color_grading,
            local_contrast,
            dehaze,
            noise_reduction,
            sharpening,
            lens_correction,
            defringe,
            perspective,
            vignette,
            geometry,
            masks,
            spots,
            rendering,
            look,
            eyes,
            negative,
            grain,
            lens_blur,
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
            // ⚠ With the composite, because the curve is one instrument as far as Sync and
            // "Reset adjustments" are concerned — the panel presents it as one widget.
            self.tone_curve_red = tone_curve_red.clone();
            self.tone_curve_green = tone_curve_green.clone();
            self.tone_curve_blue = tone_curve_blue.clone();
            self.tone_curve_parametric = *tone_curve_parametric;
            self.local_contrast = *local_contrast;
            self.dehaze = *dehaze;
        }
        if groups.color {
            self.saturation = *saturation;
            self.vibrance = *vibrance;
            self.hsl = *hsl;
            self.color_grading = *color_grading;
        }
        if groups.detail {
            self.noise_reduction = *noise_reduction;
            self.sharpening = *sharpening;
        }
        if groups.lens {
            self.lens_correction = *lens_correction;
            self.defringe = *defringe;
        }
        if groups.vignette {
            self.vignette = *vignette;
            // ⚠ M10.11b: grain with the vignette — the group shown as Effects, Lightroom's.
            self.grain = *grain;
        }
        if groups.geometry {
            self.geometry = *geometry;
            // ⚠ Perspective is compositional, exactly as the crop is: a keystone framed for one
            // photograph is meaningless on another. So it rides the Geometry group, which
            // defaults **off** for presets, paste and sync.
            self.perspective = *perspective;
        }
        if groups.masks {
            self.masks = masks.clone();
        }
        if groups.retouch {
            self.spots = spots.clone();
            self.eyes = eyes.clone();
        }
        if groups.rendering {
            self.rendering = *rendering;
            // ⚠ A look rides `Rendering` rather than `Color`: it is a rendering decision, and a
            // `Linear` one literally *is* the rendering. Splitting the two across groups would
            // let a paste carry a linear look while leaving `rendering` behind, which is the
            // half-applied state this partition exists to prevent.
            self.look = look.clone();
        }
        if groups.negative {
            self.negative = *negative;
        }
        if groups.lens_blur {
            self.lens_blur = *lens_blur;
        }
    }

    /// Whether `self` and `other` differ inside edit group `group`.
    ///
    /// ⚠ Defined *through* [`Self::merge_from`] rather than as a second field list, so it
    /// cannot disagree with the partition: merge `self`'s group into a clone of `other` and
    /// see whether anything moved. Costs one clone per call — the Develop panel asks six
    /// times a frame, which is a few kilobytes; if a profile ever shows it, cache the six
    /// answers on a change rather than inventing a second partition.
    pub fn group_differs(&self, other: &Edits, group: usize) -> bool {
        let mut probe = other.clone();
        probe.merge_from(self, EditGroups::only(group));
        probe != *other
    }

    /// How many individual edit parameters differ between `self` and `other`.
    ///
    /// ⚠⚠ **Derived from serialisation, deliberately, and not from a field list.** `Edits` has 25
    /// top-level fields and no `#[serde(skip)]` anywhere, so every parameter — including one added
    /// tomorrow — is counted without anybody remembering to add it. A hand-written list would report
    /// **zero** changes for a new field, which is the exact failure mode this repo's hardest-won
    /// lesson is about: a guard that enumerates stops guarding the moment the list grows.
    ///
    /// ⚠ Why it exists: the snapshot strip used to count *history steps* back to the reference state,
    /// so setting a slider, resetting it and setting it again read as three changes when the present
    /// differed from the reference in one parameter. Reported in use twice.
    ///
    /// Granularity, chosen and not accidental:
    /// - nested objects are **recursed into**, so three HSL sliders are three changes, not one;
    /// - a list counts **one per differing element** and is not recursed into, so adding a mask is one
    ///   change rather than the twenty fields that mask contains.
    pub fn changed_parameters(&self, other: &Edits) -> usize {
        // ⚠ A fallback that still answers, rather than a panic or a silent zero: `serde_json` refuses
        // non-finite floats, and a NaN reaching an edit field must not take the panel down. "They
        // differ, by at least one thing" is true and useless, which is the right shape for a case
        // that should not arise.
        let (Ok(a), Ok(b)) = (serde_json::to_value(self), serde_json::to_value(other)) else {
            return usize::from(self != other);
        };
        count_value_differences(&a, &b)
    }
}

/// The recursion behind [`Edits::changed_parameters`].
fn count_value_differences(a: &serde_json::Value, b: &serde_json::Value) -> usize {
    use serde_json::Value;
    if a == b {
        return 0;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            // ⚠ The **union** of keys: a field carrying `skip_serializing_if` is absent on one side
            // and present on the other, and that is a difference rather than something to skip.
            let mut keys: Vec<&str> = x.keys().chain(y.keys()).map(|k| k.as_str()).collect();
            keys.sort_unstable();
            keys.dedup();
            keys.iter()
                .map(|k| match (x.get(*k), y.get(*k)) {
                    (Some(av), Some(bv)) => count_value_differences(av, bv),
                    _ => 1,
                })
                .sum()
        }
        (Value::Array(x), Value::Array(y)) => {
            let shared = x.len().min(y.len());
            let differing = (0..shared).filter(|&i| x[i] != y[i]).count();
            differing + x.len().abs_diff(y.len())
        }
        // ⚠ Two different shapes, or two different scalars: one change, not a recursion.
        _ => 1,
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

    /// ⚠⚠ The three curves must travel with **`Tone`** and with nothing else — the composite's group,
    /// because the panel presents the curve as one instrument.
    #[test]
    fn the_channel_curves_transfer_with_tone_only() {
        let src = all_nonneutral();
        assert!(
            !src.tone_curve_red.points.is_empty()
                && src.tone_curve_red != src.tone_curve_green
                && src.tone_curve_green != src.tone_curve_blue,
            "the fixture must set three DIFFERENT curves, or this proves nothing"
        );

        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.tone_curve_red, src.tone_curve_red);
        assert_eq!(got.tone_curve_green, src.tone_curve_green);
        assert_eq!(got.tone_curve_blue, src.tone_curve_blue);

        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            if g == 1 {
                continue;
            }
            let mut other = Edits::default();
            other.merge_from(&src, EditGroups::only(g));
            assert!(
                other.tone_curve_red.points.is_empty(),
                "group {g} ({name}) carried a channel curve it should not"
            );
        }
    }

    /// ⚠⚠ A sidecar written before these fields existed must load as **identity** — the container's
    /// `#[serde(default)]` doing its job, with no `version` bump involved. A test rather than a number
    /// is the guard. ⚠ (This used to end "which is why `version` is still 7". It never was: M7.3b
    /// found `version` is never written, so every sidecar on disk carries 0.)
    /// ⚠ M5.1c. An older sidecar has no `tone_curve_parametric` and must load neutral; the splits
    /// alone are modifiers and do not activate the curve; and the Tone group carries it.
    #[test]
    fn the_parametric_curve_defaults_neutral_and_travels_with_tone() {
        let e: Edits = serde_json::from_str(r#"{"exposure":0.5}"#).expect("must parse");
        assert_eq!(e.tone_curve_parametric, ParametricCurve::default());
        assert!(!e.tone_curve_parametric.is_active());
        assert_eq!(e.tone_curve_parametric.splits, [0.25, 0.5, 0.75]);
        let moved = ParametricCurve {
            splits: [0.1, 0.5, 0.9],
            ..Default::default()
        };
        assert!(
            !moved.is_active(),
            "splits alone must not activate the curve"
        );
        let lifted = ParametricCurve {
            shadows: 10.0,
            ..Default::default()
        };
        assert!(lifted.is_active());

        let src = Edits {
            tone_curve_parametric: lifted,
            ..Default::default()
        };
        let mut dst = Edits::default();
        dst.merge_from(
            &src,
            EditGroups {
                tone: true,
                ..EditGroups::NONE
            },
        );
        assert_eq!(
            dst.tone_curve_parametric, lifted,
            "the Tone group must carry it"
        );
        let mut untouched = Edits::default();
        untouched.merge_from(
            &src,
            EditGroups {
                tone: false,
                ..EditGroups::ALL
            },
        );
        assert_eq!(untouched.tone_curve_parametric, ParametricCurve::default());
    }

    #[test]
    fn a_sidecar_without_the_channel_curves_loads_as_identity() {
        let e: Edits = serde_json::from_str(r#"{"version":7,"exposure":0.5}"#).expect("must parse");
        assert_eq!(e.exposure, 0.5);
        assert!(e.tone_curve_red.points.is_empty());
        assert!(e.tone_curve_green.points.is_empty());
        assert!(e.tone_curve_blue.points.is_empty());
        assert!(e.tone_curve.points.is_empty());
    }

    /// ⚠⚠ **Reported in use twice, 2026-09-23.** The snapshot strip counted *history steps* back to the
    /// reference state, so setting exposure, resetting it and setting it again read as **3 changes**
    /// when the present differed from the snapshot in exactly one parameter. This counts what differs.
    #[test]
    fn one_changed_slider_is_one_changed_parameter() {
        let a = Edits::default();
        let b = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        assert_eq!(a.changed_parameters(&b), 1);
        assert_eq!(b.changed_parameters(&a), 1, "and it is symmetric");
    }

    #[test]
    fn identical_edits_have_no_changed_parameters() {
        let a = Edits {
            exposure: 0.5,
            contrast: 10.0,
            ..Default::default()
        };
        assert_eq!(a.changed_parameters(&a.clone()), 0);
    }

    /// ⚠ The distinction the user asked for: two sliders is two, not one. A count of edit *groups*
    /// would have said one, because exposure and contrast share the Tone group.
    #[test]
    fn two_changed_sliders_are_two_changed_parameters() {
        let a = Edits::default();
        let b = Edits {
            exposure: 0.5,
            contrast: 10.0,
            ..Default::default()
        };
        assert_eq!(a.changed_parameters(&b), 2);
    }

    /// ⚠⚠ **The staleness guard, and the reason this is derived from serialisation rather than a field
    /// list.** `Edits` has 25 top-level fields and no `#[serde(skip)]`, so every parameter — including
    /// one added tomorrow — is visible to the count. Any two `Edits` that differ must report at least
    /// one change; a hand-written list would silently report zero for a new field.
    #[test]
    fn any_difference_at_all_counts_as_at_least_one() {
        let base = Edits::default();
        // Sweep every edit group, which by construction covers every field of `Edits`.
        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut other = base.clone();
            other.merge_from(&all_nonneutral(), EditGroups::only(g));
            if other == base {
                continue;
            }
            assert!(
                base.changed_parameters(&other) > 0,
                "group {g} ({name}) differs but counted zero changes"
            );
        }
    }

    /// ⚠ Nested objects are recursed into, so three HSL sliders read as three changes rather than one.
    #[test]
    fn nested_parameters_are_counted_individually() {
        let a = Edits::default();
        let mut b = a.clone();
        b.hsl.bands[0].hue = 10.0;
        b.hsl.bands[1].saturation = 20.0;
        assert_eq!(a.changed_parameters(&b), 2);
    }

    fn spot(at: f32) -> Spot {
        Spot {
            centre: [at, at],
            source: [at + 0.1, at],
            radius: 0.05,
            feather: 50.0,
            mode: SpotMode::default(),
        }
    }

    /// ⚠ A list counts **one per differing element**, deliberately not recursed into: adding a mask is
    /// one change, not the twenty fields that mask happens to contain.
    /// M10.11a: a negative travels only with its own group, which is off by default — pasting a
    /// digital photo's settings must never switch a negative's inversion off.
    /// M10.11b: grain rides the vignette's group, shown as Effects; a default is not written, so no
    /// sidecar or preview key changes for a photo without grain.
    /// M10.16d: Lens Blur travels only with its own group (off by default), and off is never written.
    #[test]
    fn lens_blur_rides_its_own_group_and_off_is_not_written() {
        let src = Edits {
            lens_blur: LensBlur {
                on: true,
                ..LensBlur::default()
            },
            ..Edits::default()
        };
        assert!(
            !EditGroups::default().lens_blur,
            "off for Paste and Sync by default"
        );
        let mut a = Edits::default();
        a.merge_from(&src, EditGroups::default());
        assert_eq!(
            a.lens_blur,
            LensBlur::default(),
            "a default paste carried it"
        );
        a.merge_from(&src, EditGroups::only(LENS_BLUR_GROUP));
        assert_eq!(a.lens_blur, src.lens_blur);
        let json = serde_json::to_string(&Edits::default()).unwrap();
        assert!(!json.contains("lens_blur"), "{json}");
        assert!(serde_json::to_string(&src).unwrap().contains("lens_blur"));
    }

    #[test]
    fn grain_rides_the_effects_group_and_a_default_is_not_written() {
        let src = Edits {
            grain: Grain {
                amount: 40.0,
                ..Grain::default()
            },
            ..Edits::default()
        };
        let mut a = Edits::default();
        a.merge_from(&src, EditGroups::only(5));
        assert_eq!(a.grain, src.grain);
        let mut b = Edits::default();
        b.merge_from(&src, EditGroups::only(1));
        assert_eq!(b.grain, Grain::default(), "not with Tone");
        assert_eq!(EDIT_GROUP_NAMES[5], "Effects");
        let plain = serde_json::to_string(&Edits::default()).unwrap();
        assert!(!plain.contains("grain"), "{plain}");
        let back: Edits = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back.grain, src.grain);
        let old: Edits = serde_json::from_str("{}").unwrap();
        assert_eq!(old.grain, Grain::default());
    }

    #[test]
    fn a_negative_is_carried_only_with_its_group() {
        let src = Edits {
            negative: Negative {
                on: true,
                base: [0.8, 0.5, 0.3],
                ..Negative::default()
            },
            ..Edits::default()
        };
        let mut a = Edits::default();
        a.merge_from(&src, EditGroups::default());
        assert_eq!(a.negative, Negative::default(), "not by default");
        let mut b = Edits::default();
        b.merge_from(&src, EditGroups::only(NEGATIVE_GROUP));
        assert_eq!(b.negative, src.negative);
    }

    #[test]
    fn adding_one_list_element_is_one_change() {
        let a = Edits::default();
        let mut b = a.clone();
        b.spots.push(spot(0.5));
        assert_eq!(a.changed_parameters(&b), 1);
        let mut c = b.clone();
        c.spots.push(spot(0.25));
        assert_eq!(
            a.changed_parameters(&c),
            2,
            "two added spots are two changes"
        );
    }

    use super::*;

    /// ⚠⚠ The fixtures do not fail to compile when `LocalAdjustments` grows, so this asserts what
    /// the type system cannot: that `all_nonneutral`'s mask really carries every field
    /// non-neutral. Without it a new field lands untested by the construct that exists to force
    /// a decision about it, silently, with nothing to say so.
    #[test]
    fn the_exhaustive_fixture_carries_a_non_neutral_curve_and_grade() {
        let a = &all_nonneutral().masks[0].adjustments;
        assert!(
            !a.tone_curve.points.is_empty(),
            "all_nonneutral's mask must carry a real curve"
        );
        assert!(
            a.color_grading.is_active(),
            "all_nonneutral's mask must carry a real grade"
        );
        assert!(
            a.noise_reduction.is_active(),
            "all_nonneutral's mask must carry a real noise reduction"
        );
        assert!(a.dehaze.is_active(), "and a real dehaze");
    }

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
    fn local_contrast_travels_with_the_tone_group() {
        // Texture and Clarity are *look*, which is what a preset carries; Detail is
        // capture-specific (noise, lens, output size). The generic partition tests above
        // prove disjointness and coverage — this pins which group it landed in.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.local_contrast, src.local_contrast);
        let mut not_detail = Edits::default();
        not_detail.merge_from(&src, EditGroups::only(3)); // 3 == "Detail"
        assert_eq!(not_detail.local_contrast, LocalContrast::default());
    }

    #[test]
    fn dehaze_travels_with_the_tone_group() {
        // ⚠ Tone, not Detail — even though dehaze sits beside noise reduction in the
        // *pipeline*. Pipeline order is about mathematical correctness; groups are about what
        // a person wants to copy between photos, and dehaze travels with a look.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.dehaze, src.dehaze);
        let mut not_detail = Edits::default();
        not_detail.merge_from(&src, EditGroups::only(3)); // 3 == "Detail"
        assert_eq!(not_detail.dehaze, Dehaze::default());
    }

    #[test]
    fn group_differs_names_exactly_the_group_that_moved() {
        // ⚠ Both halves: "always true" fails the identity row, "always false" fails every
        // moved row. Swept over every group via the exhaustive fixture, so a group whose
        // fields never differ from neutral would surface here as a fixture gap.
        let neutral = Edits::default();
        let full = all_nonneutral();
        for (g, moved) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut one = neutral.clone();
            one.merge_from(&full, EditGroups::only(g));
            for (h, asked) in EDIT_GROUP_NAMES.iter().enumerate() {
                assert_eq!(
                    one.group_differs(&neutral, h),
                    g == h,
                    "moved group {g} ({moved}), asked about {h} ({asked})"
                );
            }
        }
        for h in 0..EDIT_GROUP_NAMES.len() {
            assert!(
                !neutral.group_differs(&neutral, h),
                "identity differs in {h}"
            );
        }
    }

    fn all_nonneutral() -> Edits {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            version: 7,
            white_balance: WhiteBalance {
                kelvin: Some(4000.0),
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
            // ⚠⚠ **Three DIFFERENT curves.** Three copies of one would let every "this group
            // transfers" test in this file pass against code that ignored the channel index — the
            // fixture trap paid for on 2026-09-23.
            tone_curve_red: ToneCurve {
                points: vec![[0.0, 0.15], [1.0, 1.0]],
            },
            tone_curve_green: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
            },
            tone_curve_blue: ToneCurve {
                points: vec![[0.0, 0.0], [1.0, 0.85]],
            },
            tone_curve_parametric: ParametricCurve {
                shadows: 20.0,
                darks: -10.0,
                lights: 15.0,
                highlights: -25.0,
                splits: [0.2, 0.45, 0.8],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            color_grading: ColorGrading {
                shadows: GradeWheel {
                    hue: 220.0,
                    saturation: 35.0,
                    luminance: -10.0,
                },
                highlights: GradeWheel {
                    hue: 40.0,
                    saturation: 30.0,
                    luminance: 8.0,
                },
                blending: 70.0,
                balance: 20.0,
                ..ColorGrading::default()
            },
            local_contrast: LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
            dehaze: Dehaze { amount: -30.0 },
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            sharpening: Sharpening {
                amount: 60.0,
                ..Default::default()
            },
            lens_correction: Some(LensCorrection {
                distortion: true,
                tca: true,
                vignetting: false,
                scale: 60.0,
            }),
            defringe: Defringe {
                purple_amount: 30.0,
                purple_hue: [270.0, 330.0],
                green_amount: 10.0,
                green_hue: [120.0, 160.0],
            },
            perspective: Perspective {
                vertical: 40.0,
                horizontal: -20.0,
                aspect: 15.0,
                scale: 80.0,
            },
            vignette: Vignette {
                amount: -30.0,
                ..Default::default()
            },
            geometry: Geometry {
                orientation: Orientation {
                    rotate: Quarter::Cw90,
                    mirror: true,
                },
                angle: 2.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
            },
            masks: vec![MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                // ⚠⚠ Non-empty **on purpose**, and for the reason the comment below already
                // gives about `LocalAdjustments`: a fixture that names exhaustiveness must
                // actually exercise the field, or a composite would travel untested through
                // every consumer this fixture exists to check.
                refine: vec![
                    Refinement {
                        op: MaskOp::Subtract,
                        mask: Mask::Radial(RadialGradient::default()),
                    },
                    // M9.13: an AI component travels with the composite like any other.
                    Refinement {
                        op: MaskOp::Add,
                        mask: Mask::Ai(AiMask {
                            prompt: AiPrompt::Click { point: [0.4, 0.2] },
                            threshold: 20.0,
                            invert: true,
                        }),
                    },
                    // M10.16a: a depth range, every field off its default.
                    Refinement {
                        op: MaskOp::Intersect,
                        mask: Mask::Depth(DepthRange {
                            near: 30.0,
                            far: 70.0,
                            feather: 20.0,
                            invert: true,
                        }),
                    },
                ],
                adjustments: LocalAdjustments {
                    exposure: 0.75,
                    // ⚠ Set by hand, exercised (M10.12b): `..Default::default()` below would
                    // otherwise carry them untested through every consumer.
                    temp: 12.0,
                    tint: -6.0,
                    // ⚠⚠ Set by hand, because this fixture **does not fail to compile** when
                    // `LocalAdjustments` grows: it fills the rest with `..Default::default()`.
                    // A construct that names exhaustiveness and does not deliver it through a
                    // nested struct is exactly how a new field lands untested by the very thing
                    // that exists to force a decision about it.
                    local_contrast: LocalContrast {
                        texture: 35.0,
                        clarity: -25.0,
                    },
                    sharpening: Sharpening {
                        amount: 60.0,
                        ..Sharpening::default()
                    },
                    hsl: Hsl {
                        bands: [HslBand {
                            hue: 8.0,
                            saturation: -15.0,
                            luminance: 5.0,
                        }; 8],
                    },
                    noise_reduction: NoiseReduction {
                        luminance: 35.0,
                        color: 25.0,
                        ..Default::default()
                    },
                    dehaze: Dehaze { amount: 40.0 },
                    tone_curve: ToneCurve {
                        points: vec![[0.0, 0.0], [0.45, 0.6], [1.0, 1.0]],
                    },
                    color_grading: ColorGrading {
                        shadows: GradeWheel {
                            hue: 210.0,
                            saturation: 30.0,
                            luminance: -8.0,
                        },
                        highlights: GradeWheel {
                            hue: 45.0,
                            saturation: 25.0,
                            luminance: 6.0,
                        },
                        blending: 60.0,
                        balance: 15.0,
                        ..ColorGrading::default()
                    },
                    ..Default::default()
                },
                disabled: false,
                // ⚠ On, for the reason `refine` is non-empty above: an exhaustive fixture must
                // exercise the field or it travels untested through every consumer (M10.12a).
                invert: true,
            }],
            spots: vec![Spot {
                centre: [0.3, 0.4],
                source: [0.5, 0.4],
                radius: 0.05,
                feather: 30.0,
                mode: SpotMode::Clone,
            }],
            eyes: vec![Eye {
                centre: [0.6, 0.35],
                radius: 0.01,
                kind: EyeKind::Pet,
                pupil_size: 60.0,
                darken: 40.0,
                catchlight: Some([-0.35, -0.35]),
            }],
            rendering: Some(Rendering::Camera),
            look: Look {
                file: Some("velvia.cube".into()),
                space: LutSpace::Rec709,
                strength: 65.0,
            },
            grain: Grain {
                amount: 30.0,
                size: 40.0,
                roughness: 70.0,
            },
            // M10.16d: every field off its default, so the Lens Blur group's sweep sees it.
            lens_blur: LensBlur {
                on: true,
                amount: 70.0,
                focus: LensFocus::Point { point: [0.4, 0.6] },
                focal_range: 20.0,
                bokeh: Bokeh::CatEye,
                boost: 40.0,
            },
            negative: Negative {
                on: true,
                base: [0.7, 0.45, 0.25],
                gamma: 0.7,
                low: [0.05, 0.06, 0.07],
                high: [1.8, 1.9, 2.1],
                mono: true,
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
    fn json_file_roundtrip() {
        let e = Edits {
            exposure: 1.25,
            geometry: Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let path = test_dir::path("edits-roundtrip").join("roundtrip.json");
        e.to_json_file(&path).unwrap();
        let back = Edits::from_json_file(&path).unwrap();
        assert_eq!(e, back);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_masks_group_transfers_masks_and_nothing_else() {
        let src = all_nonneutral();
        let mut dst = Edits::default();
        // ⚠ The named constant, not a literal — this test hardcoded `6` and broke the moment a
        // group was inserted before Masks, which is exactly what `MASKS_GROUP` exists to prevent.
        dst.merge_from(&src, EditGroups::only(MASKS_GROUP));
        assert_eq!(dst.masks, src.masks);
        assert_eq!(dst.exposure, 0.0, "only the masks group was selected");
    }

    #[test]
    fn lens_corrections_travel_on_their_own_group_not_with_detail() {
        // ⚠ The reason `lens` was split out of `detail`. They were bundled at first, which meant
        // syncing noise reduction to another photo also switched its lens corrections on or off —
        // two unrelated things travelling together. This pins both halves of the separation.
        let src = all_nonneutral();
        let lens_group = EDIT_GROUP_NAMES.iter().position(|n| *n == "Lens").unwrap();

        let mut only_detail = Edits::default();
        only_detail.merge_from(&src, EditGroups::only(3));
        assert_eq!(
            only_detail.lens_correction, None,
            "the Detail group must not carry lens corrections"
        );
        assert_eq!(only_detail.noise_reduction, src.noise_reduction);

        let mut only_lens = Edits::default();
        only_lens.merge_from(&src, EditGroups::only(lens_group));
        assert_eq!(only_lens.lens_correction, src.lens_correction);
        assert_eq!(
            only_lens.noise_reduction,
            NoiseReduction::default(),
            "the Lens group must not carry anything else"
        );
    }

    /// ⚠ Both directions. That it *travels* with Geometry proves the arm exists; that it does
    /// **not** travel with any other group proves it was not put in two.
    #[test]
    fn perspective_travels_on_the_geometry_group_and_no_other() {
        let src = all_nonneutral();
        let mut with = Edits::default();
        with.merge_from(&src, EditGroups::only(GEOMETRY_GROUP));
        assert_eq!(with.perspective, src.perspective);

        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            if g == GEOMETRY_GROUP {
                continue;
            }
            let mut without = Edits::default();
            without.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                without.perspective,
                Perspective::default(),
                "group {g} ({name}) must not carry perspective"
            );
        }
    }

    /// ⚠ Both directions, `perspective_travels_on_the_geometry_group_and_no_other`'s shape: that
    /// spots *travel* with Retouch proves the arm exists; that they travel with **no other**
    /// group proves they were not put in two.
    #[test]
    fn spots_travel_on_the_retouch_group_and_no_other() {
        let src = all_nonneutral();
        assert!(
            !src.spots.is_empty(),
            "fixture check: the source carries a spot"
        );
        let mut with = Edits::default();
        with.merge_from(&src, EditGroups::only(RETOUCH_GROUP));
        assert_eq!(with.spots, src.spots);
        assert_eq!(with.exposure, 0.0, "only the retouch group was selected");

        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            if g == RETOUCH_GROUP {
                continue;
            }
            let mut without = Edits::default();
            without.merge_from(&src, EditGroups::only(g));
            assert!(
                without.spots.is_empty(),
                "group {g} ({name}) must not carry spots"
            );
        }
    }

    /// ⚠ Both directions, `spots_travel_on_the_retouch_group_and_no_other`'s shape.
    #[test]
    fn eyes_travel_on_the_retouch_group_and_no_other() {
        let src = all_nonneutral();
        assert!(
            !src.eyes.is_empty(),
            "fixture check: the source carries an eye"
        );
        let mut with = Edits::default();
        with.merge_from(&src, EditGroups::only(RETOUCH_GROUP));
        assert_eq!(with.eyes, src.eyes);
        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            if g == RETOUCH_GROUP {
                continue;
            }
            let mut without = Edits::default();
            without.merge_from(&src, EditGroups::only(g));
            assert!(
                without.eyes.is_empty(),
                "group {g} ({name}) must not carry eyes"
            );
        }
    }
}

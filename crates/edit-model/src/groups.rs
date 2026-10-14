//! The edit groups: what copy, paste, sync and presets transfer, one checkbox each.

use serde::{Deserialize, Serialize};

/// Group names in [`EditGroups`] field order — the one source of truth shared by the merge
/// logic, the checkbox UI and the tests. Mirrors [`HSL_BAND_NAMES`].
pub const EDIT_GROUP_NAMES: [&str; 12] = [
    "White Balance",
    "Tone",
    "Color",
    "Detail",
    "Lens",
    "Effects",
    "Geometry",
    "Masks",
    "Retouch",
    "Rendering",
    "Negative",
    "Lens Blur",
];

/// The `Geometry` group's index into [`EDIT_GROUP_NAMES`].
///
/// ⚠ Named rather than written as `5` at call sites: the Crop tool's own reset targets exactly
/// this group, and a magic index would silently reset the wrong one if the table ever changed
/// order. `the_named_group_indices_match_the_table` is the guard.
pub const GEOMETRY_GROUP: usize = 6;

/// The `Masks` group's index into [`EDIT_GROUP_NAMES`]. See [`GEOMETRY_GROUP`].
pub const MASKS_GROUP: usize = 7;

/// The `Retouch` group's index into [`EDIT_GROUP_NAMES`] — spot removal. See
/// [`GEOMETRY_GROUP`].
pub const RETOUCH_GROUP: usize = 8;

/// The `Rendering` group's index into [`EDIT_GROUP_NAMES`].
///
/// ⚠⚠ **There is deliberately no panel *section* named "Rendering".** "Reset adjustments" derives
/// its group set from the panel's `SECTIONS`, so a group with no matching section is never reset —
/// which is what stops that button flattening the photograph back to a colorimetric render. The
/// control lives in the panel's Profile block above the sections, whose header offers no reset.
pub const RENDERING_GROUP: usize = 9;

/// The `Negative` group's index into [`EDIT_GROUP_NAMES`] (M10.11a) — the film negative's inversion.
///
/// ⚠ **Appended**, so every index before it stays. ⚠ Not a panel section either: the Negative block
/// sits above them, like Profile, so Reset adjustments leaves the inversion alone — resetting a
/// scanned negative should start over from the positive, not hand back the negative.
pub const NEGATIVE_GROUP: usize = 10;

/// The `Lens Blur` group's index into [`EDIT_GROUP_NAMES`] (M10.16d). ⚠ **Appended**, so every index
/// before it stays. Off by default for Paste, Sync and presets — a focus point belongs to one
/// photograph, Masks' and Geometry's reason.
pub const LENS_BLUR_GROUP: usize = 11;

/// Which groups of settings a paste or a preset carries.
///
/// [`Default`] is everything **except** geometry: a crop framed for one composition reframes
/// another, so carrying it across photos is usually wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditGroups {
    pub white_balance: bool,
    /// Exposure, contrast, highlights, shadows, whites, blacks, tone curve, texture,
    /// clarity, dehaze.
    pub tone: bool,
    /// Saturation, vibrance, HSL.
    pub color: bool,
    /// Noise reduction and sharpening.
    pub detail: bool,
    /// Optical corrections read from a lens profile.
    ///
    /// ⚠ Its own group rather than part of `detail`. They were bundled at first, which meant
    /// syncing noise reduction also synced lens corrections — two unrelated things travelling
    /// together for no reason. Separating them is what lets "copy this photo's lens
    /// corrections" be its own choice in Paste, Sync and Save-preset.
    pub lens: bool,
    /// The creative, post-crop vignette.
    ///
    /// ⚠ `#[serde(alias = "effects")]` because this field was called `effects` and the group
    /// was displayed as "Effects". `EditGroups` carries a container-level `#[serde(default)]`,
    /// so without the alias an existing preset storing `"effects": false` would fall back to
    /// this field's default of **true** — silently starting to carry a vignette the preset was
    /// deliberately built to exclude. Renaming a serialized field is never just a rename.
    #[serde(alias = "effects")]
    pub vignette: bool,
    /// Crop and straighten.
    pub geometry: bool,
    /// Masked local adjustments.
    ///
    /// ⚠ Off by default, like `geometry` and for the same reason: a gradient positioned for
    /// one composition is meaningless on another.
    pub masks: bool,
    /// Spot removal — clone and heal patches — and red/pet-eye corrections (M9.11).
    ///
    /// ⚠ Off by default, like `geometry` and `masks`: the values are content-referred, and
    /// Lightroom's Copy Settings excludes Spot Removal for the same reason it excludes the crop.
    /// Syncing a burst with the box *ticked* is still the real use — the same sensor dust sits
    /// at the same coordinates on every frame, which pre-geometry anchoring is what makes right.
    pub retouch: bool,
    /// The display rendering.
    ///
    /// ⚠ **On** by default, unlike `geometry`, `masks` and `retouch`: a rendering is a *look*,
    /// so it should follow a paste, a preset and a sync, where those three are placements tied
    /// to one photograph's content.
    pub rendering: bool,
    /// The film negative's inversion (M10.11a).
    ///
    /// ⚠⚠ **Off by default, like `geometry`** — the base and the densities are measured from one
    /// roll's film, and a paste of a digital photo's settings or a colour preset must never switch a
    /// negative's inversion off. Tick it to Sync a balanced frame across its roll.
    pub negative: bool,
    /// Lens Blur (M10.16d). ⚠ Off by default, like `geometry` and `masks` — a Point focus is one
    /// photograph's, and even a Subject focus is a decision about this picture rather than a look.
    pub lens_blur: bool,
}

impl Default for EditGroups {
    fn default() -> Self {
        Self {
            white_balance: true,
            tone: true,
            color: true,
            detail: true,
            lens: true,
            vignette: true,
            geometry: false,
            masks: false,
            retouch: false,
            rendering: true,
            negative: false,
            lens_blur: false,
        }
    }
}

impl EditGroups {
    pub const ALL: Self = Self {
        white_balance: true,
        tone: true,
        color: true,
        detail: true,
        lens: true,
        vignette: true,
        geometry: true,
        masks: true,
        retouch: true,
        rendering: true,
        negative: true,
        lens_blur: true,
    };

    pub const NONE: Self = Self {
        white_balance: false,
        tone: false,
        color: false,
        detail: false,
        lens: false,
        vignette: false,
        geometry: false,
        masks: false,
        retouch: false,
        rendering: false,
        negative: false,
        lens_blur: false,
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
            4 => self.lens,
            5 => self.vignette,
            6 => self.geometry,
            7 => self.masks,
            8 => self.retouch,
            9 => self.rendering,
            10 => self.negative,
            11 => self.lens_blur,
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
            4 => &mut self.lens,
            5 => &mut self.vignette,
            6 => &mut self.geometry,
            7 => &mut self.masks,
            8 => &mut self.retouch,
            9 => &mut self.rendering,
            10 => &mut self.negative,
            11 => &mut self.lens_blur,
            _ => panic!("edit group index {i} out of range"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_named_group_indices_match_the_table() {
        // ⚠ A tripwire, not a discovery. The two constants exist so a per-tool reset does not
        // write a magic `5` or `6`; this is what stops them going stale if the table is
        // reordered, which nothing else would catch — a wrong index still compiles and still
        // resets *something*.
        assert_eq!(EDIT_GROUP_NAMES[GEOMETRY_GROUP], "Geometry");
        assert_eq!(EDIT_GROUP_NAMES[MASKS_GROUP], "Masks");
        assert_eq!(EDIT_GROUP_NAMES[RETOUCH_GROUP], "Retouch");
        assert_eq!(EDIT_GROUP_NAMES[NEGATIVE_GROUP], "Negative");
        assert_eq!(EDIT_GROUP_NAMES[LENS_BLUR_GROUP], "Lens Blur");
    }

    /// M10.11a: the Negative group is off by default, like Geometry, and an older selection that
    /// never heard of it reads it off.
    #[test]
    fn the_negative_group_is_off_unless_asked() {
        assert!(!EditGroups::default().negative);
        assert!(EditGroups::ALL.get(NEGATIVE_GROUP) && !EditGroups::NONE.get(NEGATIVE_GROUP));
        assert!(EditGroups::only(NEGATIVE_GROUP).get(NEGATIVE_GROUP));
        let old: EditGroups = serde_json::from_str(r#"{"tone":true}"#).unwrap();
        assert!(!old.negative);
    }

    #[test]
    fn the_default_selection_excludes_geometry() {
        // A crop framed for one composition reframes another, and a preset built on a 3:2
        // crop would reshape every photo it touches.
        let g = EditGroups::default();
        assert!(!g.geometry);
        assert!(g.white_balance && g.tone && g.color && g.detail && g.vignette);
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
    fn masks_do_not_transfer_by_default() {
        // ⚠ The same treatment geometry gets, for the same reason: a gradient positioned for
        // one composition is meaningless on another, so carrying it across photos by default
        // would be wrong more often than right.
        assert!(!EditGroups::default().masks);
        // ⚠ Via `get(6)` rather than the field: that is the path the dialog and the generic
        // partition tests index through, so this also checks the new arm exists — and a bare
        // `EditGroups::ALL.masks` is a compile-time constant, which clippy rejects.
        assert!(
            EditGroups::ALL.get(6),
            "but it must be reachable when asked for"
        );
    }

    #[test]
    fn a_preset_that_excluded_effects_still_excludes_the_vignette() {
        // ⚠ The whole reason `vignette` carries `#[serde(alias = "effects")]`. `EditGroups` has a
        // container-level `#[serde(default)]`, so without the alias this `false` would be dropped
        // and the field would fall back to its default of **true** — a preset deliberately built
        // to leave the vignette alone would silently start applying one. A renamed serialized
        // field is a migration, not a rename.
        let g: EditGroups = serde_json::from_str(r#"{"effects":false}"#).unwrap();
        assert!(!g.vignette, "the old key must still be honoured");

        // And the new name works, so the alias has not replaced the real field.
        let g: EditGroups = serde_json::from_str(r#"{"vignette":false}"#).unwrap();
        assert!(!g.vignette);
    }

    #[test]
    fn the_vignette_group_is_named_for_what_it_carries() {
        // ⚠ M10.11b: it holds the vignette AND grain now, so it is "Effects" again — Lightroom's name.
        // It was renamed "Vignette" when it held exactly one thing; the name follows the contents.
        assert_eq!(EDIT_GROUP_NAMES[5], "Effects");
    }

    #[test]
    fn retouch_is_off_by_default_for_paste_and_presets() {
        // ⚠ Like geometry and masks, and for the same reason: a spot's coordinates are
        // content-referred. `ALL` must still reach it, or a ticked box could never sync it.
        assert!(!EditGroups::default().retouch);
        assert!(!EditGroups::default().get(RETOUCH_GROUP));
        assert!(EditGroups::ALL.get(RETOUCH_GROUP));
        assert!(EditGroups::only(RETOUCH_GROUP).retouch);
    }
}

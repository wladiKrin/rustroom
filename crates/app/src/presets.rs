//! Named `Edits`: the preset itself, what its row says, and the presets Rustroom ships.
//!
//! ⚠ The store — files, folders, the trash — is `preset_store` (split out in M10.9a, when folders
//! would have taken this file past 600 lines); its names are re-exported here so callers did not
//! move.

use edit_model::{EditGroups, Edits};
use serde::{Deserialize, Serialize};

pub use crate::preset_store::{PresetError, PresetStore, TRASH_CAP};

/// A named set of edits plus the groups it carries.
///
/// The preset stores its **own** group mask, chosen when it is created, so a "punchy tone"
/// preset never disturbs the photo's white balance. Copy/paste picks groups at paste time
/// instead; the shared `EditGroups` type is what makes the two one feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Display name. Stored inside the file as well as in the filename, because the
    /// filename is sanitized and the display name should survive that.
    pub name: String,
    pub groups: EditGroups,
    pub edits: Edits,
    /// The folder it lives in, `None` for none (M10.9a). ⚠ **Not in the file** — the folder is where
    /// the file is, so an exported preset is exactly the stored one; `PresetStore::list` fills it.
    #[serde(skip)]
    pub folder: Option<String>,
}

impl Preset {
    /// What names this preset across folders: its name, or `folder/name` — what Import's preset
    /// choice stores. ⚠ A name stored before folders existed is a top-level preset's key unchanged.
    pub fn key(&self) -> String {
        match &self.folder {
            Some(f) => format!("{f}/{}", self.name),
            None => self.name.clone(),
        }
    }

    /// Whether the open photo already has this preset: merging it into `edits` would change
    /// nothing. What lights a row in the panel.
    ///
    /// ⚠ Decided through `merge_from` itself, never a field-by-field comparison: the merge is
    /// what applying does, so the two cannot disagree about which fields a group carries.
    pub fn matches(&self, edits: &Edits) -> bool {
        let mut probe = edits.clone();
        probe.merge_from(&self.edits, self.groups);
        probe == *edits
    }
}

/// What a preset carries, for its row: "all settings", or the on groups' names joined with
/// " · " ("Tone · Color").
pub fn groups_summary(groups: EditGroups) -> String {
    let on: Vec<&str> = edit_model::EDIT_GROUP_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| groups.get(*i))
        .map(|(_, n)| *n)
        .collect();
    if on.len() == edit_model::EDIT_GROUP_NAMES.len() {
        "all settings".to_owned()
    } else if on.is_empty() {
        "nothing".to_owned()
    } else {
        on.join(" · ")
    }
}

/// The groups a starter preset carries: tone and colour, never anything tied to one photo.
///
/// ⚠ A preset *replaces* the groups it carries, so a starter that sets contrast also sets
/// exposure back to 0 — the same rule every user preset follows. Worth knowing before adding a
/// starter that touches only one slider.
fn starter_groups() -> EditGroups {
    EditGroups {
        white_balance: false,
        tone: true,
        color: true,
        detail: false,
        lens: false,
        vignette: false,
        geometry: false,
        masks: false,
        retouch: false,
        rendering: false,
        negative: false,
        lens_blur: false,
    }
}

/// The presets Rustroom ships, so the Presets tool is never empty on first run — the one thing
/// both first-use reviews called a blocker for an amateur (A-23, P-22).
///
/// ⚠ Modest on purpose: a starter is a nudge a beginner builds on, not a filter. The author
/// retunes the values; the test pins only that each one changes something and that no two are
/// alike.
pub fn builtin() -> Vec<Preset> {
    use edit_model::{ColorGrading, GradeWheel, LocalContrast, ParametricCurve};
    let wheel = |hue: f32, saturation: f32| GradeWheel {
        hue,
        saturation,
        luminance: 0.0,
    };
    let starter = |name: &str, edits: Edits| Preset {
        name: name.to_string(),
        groups: starter_groups(),
        edits,
        folder: None,
    };
    vec![
        starter(
            "Natural",
            Edits {
                contrast: 8.0,
                vibrance: 12.0,
                local_contrast: LocalContrast {
                    clarity: 5.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Punchy",
            Edits {
                contrast: 20.0,
                whites: 10.0,
                blacks: -10.0,
                vibrance: 25.0,
                local_contrast: LocalContrast {
                    clarity: 15.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Soft",
            Edits {
                contrast: -15.0,
                highlights: -15.0,
                shadows: 15.0,
                local_contrast: LocalContrast {
                    clarity: -20.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Warm",
            Edits {
                vibrance: 5.0,
                color_grading: ColorGrading {
                    shadows: wheel(40.0, 12.0),
                    highlights: wheel(45.0, 8.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Cool",
            Edits {
                color_grading: ColorGrading {
                    shadows: wheel(220.0, 12.0),
                    highlights: wheel(210.0, 6.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Black & white",
            Edits {
                saturation: -100.0,
                contrast: 10.0,
                local_contrast: LocalContrast {
                    clarity: 10.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "Matte",
            Edits {
                contrast: -5.0,
                saturation: -10.0,
                tone_curve_parametric: ParametricCurve {
                    shadows: 25.0,
                    highlights: -10.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        starter(
            "High key",
            Edits {
                exposure: 0.7,
                contrast: -10.0,
                highlights: -20.0,
                whites: 15.0,
                blacks: 15.0,
                ..Default::default()
            },
        ),
        starter(
            "Faded",
            Edits {
                contrast: -20.0,
                saturation: -25.0,
                tone_curve_parametric: ParametricCurve {
                    shadows: 20.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
    ]
}

#[cfg(test)]
mod panel_helpers_tests {
    use super::*;

    #[test]
    fn a_summary_names_the_groups_or_says_all() {
        assert_eq!(groups_summary(EditGroups::ALL), "all settings");
        assert_eq!(groups_summary(EditGroups::NONE), "nothing");
        assert_eq!(
            groups_summary(EditGroups::only(1)),
            edit_model::EDIT_GROUP_NAMES[1]
        );
        let mut two = EditGroups::only(1);
        *two.get_mut(2) = true;
        assert_eq!(
            groups_summary(two),
            format!(
                "{} · {}",
                edit_model::EDIT_GROUP_NAMES[1],
                edit_model::EDIT_GROUP_NAMES[2]
            )
        );
    }

    #[test]
    fn a_preset_matches_when_applying_it_would_change_nothing() {
        // ⚠ Both halves: a `matches` that always said yes would light every row.
        let edits = Edits {
            exposure: 1.0,
            ..Default::default()
        };
        let same = Preset {
            name: "Same".into(),
            groups: EditGroups::ALL,
            edits: edits.clone(),
            folder: None,
        };
        assert!(same.matches(&edits));
        let differs = Preset {
            name: "Other".into(),
            groups: EditGroups::ALL,
            edits: Edits {
                exposure: 2.0,
                ..Default::default()
            },
            folder: None,
        };
        assert!(!differs.matches(&edits));
        // ⚠ And the groups scope it: a preset carrying only the Color group matches a photo
        // whose exposure differs from the preset's, because exposure is not what it applies.
        let scoped = Preset {
            name: "Colour".into(),
            groups: EditGroups::only(2),
            edits: Edits {
                exposure: 5.0,
                ..Default::default()
            },
            folder: None,
        };
        assert!(
            scoped.matches(&edits),
            "a group it does not carry must not count"
        );
    }

    /// Every starter changes the default photo, no two starters are alike, and none carries a
    /// group tied to one photo (crop, masks, retouch) — the property, not the values, which are
    /// the author's to tune.
    #[test]
    fn the_starter_presets_each_do_something_different() {
        let set = builtin();
        assert!(set.len() >= 8, "sample: {}", set.len());
        let mut results: Vec<Edits> = Vec::new();
        for p in &set {
            assert!(
                !p.groups.geometry && !p.groups.masks && !p.groups.retouch,
                "{}",
                p.name
            );
            let mut e = Edits::default();
            e.merge_from(&p.edits, p.groups);
            assert_ne!(e, Edits::default(), "{} changes nothing", p.name);
            assert!(
                !results.contains(&e),
                "{} duplicates another starter",
                p.name
            );
            results.push(e);
        }
        let mut names: Vec<&str> = set.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), set.len(), "two starters share a name");
    }
}

//! The Profile block's rendering controls: the camera rendering and the look.
//!
//! ⚠⚠ **A rendering is not an adjustment, and it sits above every section to say so.** Every
//! slider below moves a photograph already in a defined state; these controls *establish* that
//! state, turning scene-referred light into something a screen can show. They shared the Tone
//! section with the sliders until 2026-09-19, where the checkbox sat on top of Exposure and read
//! as one of them (user's report). With the camera profile, which `panel/mod.rs` draws just
//! above, they are the three things that decide how the raw data becomes a picture.
//!
//! ⚠ Not a `Section`: "Reset adjustments" derives its group set from `SECTIONS`, and a reset here
//! would flatten the photo to a colorimetric render — see `edit_model::RENDERING_GROUP`. The
//! header is drawn by `panel/mod.rs` with `resettable: false`.

use crate::develop::panel::Edited;
use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::looks::LooksState;

/// The camera-rendering checkbox and, directly under it, the look controls.
pub fn rendering(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    looks: &LooksState,
    // What Camera matching applies to this photo (M10.13d); `None` when the file names no film simulation.
    camera_look: Option<&rustroom_app::camera_look::Resolved>,
    ed: &mut Edited,
) {
    let mut rendered = edits.applied_rendering().is_camera();
    if ui
        .checkbox(&mut rendered, "Camera rendering")
        .hover(
            "Renders the photo with the camera's own tone and colour, instead of a flat colorimetric look",
        )
        .changed()
    {
        // ⚠ Always `Some`: a click is a decision, and *decided off* is what survives a photo
        // switch — `None` would read as never-asked and the preference would switch it back on.
        edits.rendering = Some(if rendered {
            edit_model::Rendering::Camera
        } else {
            edit_model::Rendering::None
        });
        // ⚠ `mark_changed`, not a slider's `changed()`: a click is a complete act, so it is one
        // history entry immediately rather than waiting for a `drag_stopped` that never comes.
        ed.mark_changed();
    }
    // M10.13d: matching the camera, for a photo that names its film simulation.
    if let (Some(cl), true) = (camera_look, edits.applied_rendering().is_camera()) {
        let mut matched = edits.applied_rendering() == edit_model::Rendering::CameraMatch;
        if ui
            .checkbox(&mut matched, "Match the camera's colour")
            .hover("Applies the look learned from the camera's own JPEGs of this film simulation")
            .changed()
        {
            edits.rendering = Some(if matched {
                edit_model::Rendering::CameraMatch
            } else {
                edit_model::Rendering::Camera
            });
            ed.mark_changed();
        }
        if matched {
            let line = if cl.cube.is_some() {
                cl.label.clone()
            } else {
                format!("{} — not learned yet", cl.label)
            };
            crate::theme::hint(ui, &line);
        }
    }
    // ⚠ Directly under the camera-rendering checkbox, because a `Linear` look *replaces* it:
    // the two controls decide one thing between them and must be read together.
    crate::develop::panel::look::section(ui, edits, looks, ed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **Both directions, because a checkbox stuck on would pass a one-way test.** The
    /// control writes an enum from a `bool`, so the mapping is the thing that can be wrong, and
    /// only clicking it twice can see it.
    #[test]
    fn the_rendering_checkbox_switches_both_ways_and_marks_the_edit() {
        let ctx = crate::test_ui::ctx();
        let mut edits = edit_model::Edits::default();
        let mut ed = Edited::default();

        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            rendering(ui, &mut edits, &Default::default(), None, &mut ed);
        });
        assert_eq!(
            edits.rendering,
            Some(edit_model::Rendering::Camera),
            "ticking it did not turn the rendering on"
        );
        assert!(ed.changed && ed.settled, "one click, one history entry");

        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            rendering(ui, &mut edits, &Default::default(), None, &mut ed);
        });
        assert_eq!(
            edits.rendering,
            Some(edit_model::Rendering::None),
            "unticking it did not turn the rendering off"
        );
    }

    /// M10.13d: under the camera rendering, a photo naming its film simulation can be matched to the camera —
    /// both ways — and the line says which look applies, or that none is learned yet.
    #[test]
    fn matching_the_camera_switches_both_ways_and_names_the_look() {
        let ctx = crate::test_ui::ctx();
        let mut edits = edit_model::Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let unlearned = rustroom_app::camera_look::Resolved {
            label: "Classic Chrome (X-T4)".into(),
            cube: None,
        };
        let mut ed = Edited::default();
        let label = "Match the camera's colour";
        crate::test_ui::click(&ctx, label, |ui| {
            rendering(
                ui,
                &mut edits,
                &Default::default(),
                Some(&unlearned),
                &mut ed,
            );
        });
        assert_eq!(edits.rendering, Some(edit_model::Rendering::CameraMatch));
        assert!(ed.changed);
        let shown: Vec<String> = crate::test_ui::nodes(&ctx, |ui| {
            rendering(
                ui,
                &mut edits,
                &Default::default(),
                Some(&unlearned),
                &mut Edited::default(),
            );
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        assert!(
            shown
                .iter()
                .any(|l| l == "Classic Chrome (X-T4) — not learned yet"),
            "{shown:?}"
        );
        crate::test_ui::click(&ctx, label, |ui| {
            rendering(
                ui,
                &mut edits,
                &Default::default(),
                Some(&unlearned),
                &mut Edited::default(),
            );
        });
        assert_eq!(edits.rendering, Some(edit_model::Rendering::Camera));
        // A file naming no film simulation offers no matching at all.
        let none: Vec<String> = crate::test_ui::nodes(&ctx, |ui| {
            rendering(
                ui,
                &mut edits,
                &Default::default(),
                None,
                &mut Edited::default(),
            );
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        assert!(
            none.iter().any(|l| l == "Camera rendering"),
            "the sample: {none:?}"
        );
        assert!(!none.iter().any(|l| l == label), "{none:?}");
    }

    /// ⚠ The checkbox writes `Some` in both directions. `None` on untick would read as
    /// never-asked, and the preference would switch the rendering back on at the next open —
    /// the exact bug the `Option` exists to end.
    #[test]
    fn unticking_the_rendering_decides_off_rather_than_forgetting() {
        let ctx = crate::test_ui::ctx();
        let mut edits = edit_model::Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            rendering(ui, &mut edits, &Default::default(), None, &mut ed);
        });
        assert_eq!(edits.rendering, Some(edit_model::Rendering::None));
        assert_ne!(
            edits,
            edit_model::Edits::default(),
            "decided-off must not read as the untouched default"
        );
    }

    /// A `LooksState` with two installed looks, built without touching disk.
    fn two_looks() -> LooksState {
        LooksState::from_entries(vec![
            rustroom_app::looks::LookEntry {
                file: "portra.cube".into(),
                name: "portra".into(),
                learned: false,
            },
            rustroom_app::looks::LookEntry {
                file: "teal.cube".into(),
                name: "teal".into(),
                learned: false,
            },
        ])
    }

    /// M10.13d: a learned film picked from the Look menu is an sRGB display table — its space is set, and
    /// there is no space to choose.
    #[test]
    fn a_learned_look_sets_its_own_space_and_offers_none() {
        let learned = rustroom_app::looks::LookEntry {
            file: "camera/Classic Chrome _X-T4_.cube".into(),
            name: "Classic Chrome (X-T4)".into(),
            learned: true,
        };
        let looks = LooksState::from_entries(vec![learned.clone()]);
        let mut edits = edit_model::Edits {
            look: edit_model::Look {
                space: edit_model::LutSpace::Linear,
                ..Default::default()
            },
            ..Default::default()
        };
        crate::develop::panel::look::choose(&mut edits, &learned);
        assert_eq!(edits.look.file.as_deref(), Some(learned.file.as_str()));
        assert_eq!(edits.look.space, edit_model::LutSpace::SrgbDisplay);
        let labels = look_labels(&mut edits, &looks);
        assert!(
            labels.iter().any(|l| l == "Strength"),
            "the sample: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l == "sRGB display"), "{labels:?}");
        // A look the user added keeps its space control.
        let mut mine = edit_model::Edits::default();
        let own = &two_looks().entries[0].clone();
        crate::develop::panel::look::choose(&mut mine, own);
        assert!(look_labels(&mut mine, &two_looks())
            .iter()
            .any(|l| l == "sRGB display"));
    }

    fn look_labels(edits: &mut edit_model::Edits, looks: &LooksState) -> Vec<String> {
        let ctx = crate::test_ui::ctx();
        let mut ed = Edited::default();
        crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            ed = Edited::default();
            rendering(ui, edits, looks, None, &mut ed);
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    /// ⚠ The controls appear only once there is something to choose, and the Strength slider
    /// only once a look is chosen — a dropdown whose sole entry is "None" can only disappoint.
    ///
    /// ⚠⚠ **Asserted on Strength, not on the dropdown or the warnings.** The harness reads the
    /// accessibility tree, which carries interactive widgets and not plain labels or combo box
    /// headings — so what the panel *says* is guarded in `looks::notes`, where it is a pure
    /// function, and only what the panel *offers* is guarded here.
    #[test]
    fn the_strength_slider_appears_only_when_a_look_is_chosen() {
        let looks = two_looks();
        let mut none = edit_model::Edits::default();
        assert!(
            !look_labels(&mut none, &looks)
                .iter()
                .any(|l| l == "Strength"),
            "Strength was offered with no look chosen"
        );

        let mut chosen = edit_model::Edits {
            look: edit_model::Look {
                file: Some("portra.cube".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            look_labels(&mut chosen, &looks)
                .iter()
                .any(|l| l == "Strength"),
            "a chosen look offered no Strength"
        );

        // ⚠ And nothing at all when none are installed and none is named.
        let empty = LooksState::default();
        let mut e = edit_model::Edits::default();
        assert!(
            !look_labels(&mut e, &empty).iter().any(|l| l == "Strength"),
            "the look controls appeared with no looks installed"
        );
    }
}

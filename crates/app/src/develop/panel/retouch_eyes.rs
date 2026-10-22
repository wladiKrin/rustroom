//! The Retouch panel's eye settings (M9.11; split by M10.19d): the eye brush's Pupil Size, Darken and Catchlight
//! for the NEXT eye, and a selected eye's own — its kind, Pupil Size, Darken and Catchlight — under the list.
//!
//! ⚠ The brush never edits a selected eye (M10.19d): the two are separate calls, each owning one target.

use super::Edited;
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use edit_model::EyeKind;
use eframe::egui;
use rustroom_app::eye_tool::{EyeTool, FALLBACK_CATCHLIGHT};
use rustroom_app::slider_help::Scope;

pub(super) const RED_EYE_HINT: &str = "Removes the red from a pupil you click";
pub(super) const PET_EYE_HINT: &str = "Darkens an animal's glowing pupil you click";
const CATCHLIGHT_HOVER: &str = "Adds a small highlight to the pupil so it looks alive";

/// Red eye · Pet eye, the compact switch — the brush's or a selected eye's.
pub(super) fn kind_switch(ui: &mut egui::Ui, kind: &mut EyeKind) {
    theme::segmented(
        ui,
        kind,
        &[
            (EyeKind::Red, "Red eye", RED_EYE_HINT),
            (EyeKind::Pet, "Pet eye", PET_EYE_HINT),
        ],
    );
}

/// The eye brush's Pupil Size, Darken and (pet) Catchlight — what the next eye takes. Tool values: no photo edit.
pub(super) fn brush_settings(ui: &mut egui::Ui, tool: &mut EyeTool, ed: &mut Edited) {
    let d = EyeTool::default();
    let mut local = Edited {
        help_scope: Scope::Eye,
        ..Edited::default()
    };
    local.slider(
        ui,
        &mut tool.pupil_size,
        0.0..=100.0,
        "Pupil Size",
        d.pupil_size,
    );
    local.slider(ui, &mut tool.darken, 0.0..=100.0, "Darken", d.darken);
    ed.missing_help.append(&mut local.missing_help);
    if tool.active == Some(EyeKind::Pet) {
        ui.checkbox(&mut tool.catchlight, "Catchlight")
            .hover(CATCHLIGHT_HOVER);
    }
}

/// Eye `i`'s kind, Pupil Size, Darken and (pet) Catchlight. A kind or Catchlight change is one undo step; a slider
/// settles one on release.
pub(super) fn selected_settings(
    ui: &mut egui::Ui,
    s: &mut Session,
    i: usize,
    tool: &EyeTool,
    ed: &mut Edited,
) {
    let mut kind = s.edits.eyes[i].kind;
    let was = kind;
    kind_switch(ui, &mut kind);
    if kind != was {
        let e = &mut s.edits.eyes[i];
        e.kind = kind;
        e.catchlight = match kind {
            EyeKind::Red => None,
            EyeKind::Pet => tool.catchlight.then_some(FALLBACK_CATCHLIGHT),
        };
        ed.discrete();
    }
    let d = EyeTool::default();
    let (mut size, mut dark) = (s.edits.eyes[i].pupil_size, s.edits.eyes[i].darken);
    let mut local = Edited {
        help_scope: Scope::Eye,
        ..Edited::default()
    };
    local.slider(ui, &mut size, 0.0..=100.0, "Pupil Size", d.pupil_size);
    local.slider(ui, &mut dark, 0.0..=100.0, "Darken", d.darken);
    ed.missing_help.append(&mut local.missing_help);
    s.edits.eyes[i].pupil_size = size;
    s.edits.eyes[i].darken = dark;
    ed.changed |= local.changed;
    ed.settled |= local.settled;
    if s.edits.eyes[i].kind == EyeKind::Pet {
        let mut on = s.edits.eyes[i].catchlight.is_some();
        if ui
            .checkbox(&mut on, "Catchlight")
            .hover(CATCHLIGHT_HOVER)
            .changed()
        {
            // ⚠ Re-ticking uses a fixed up-left in the stage's pre-orientation frame: only a canvas click knows
            // which way is up on screen.
            s.edits.eyes[i].catchlight = on.then_some(FALLBACK_CATCHLIGHT);
            ed.discrete();
        }
    }
}

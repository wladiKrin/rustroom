//! The Retouch panel's eye half (M9.11): the four-way mode, the Eyes list, and Pupil Size /
//! Darken / Catchlight for the lit eye.
//!
//! ⚠ Its own file because `retouch.rs` was already ~310 lines of code; the patch half stays there.

use super::Edited;
use crate::develop::panel::{mask_components, section};
use crate::theme::Hover as _;
use crate::Session;
use edit_model::{EyeKind, SpotMode};
use eframe::egui;
use rustroom_app::eye_tool::{EyeTool, FALLBACK_CATCHLIGHT};
use rustroom_app::{mask_tool, spot_tool};

/// The Retouch tool's four modes. Derived, never stored: `SpotTool::mode` and `EyeTool::active`
/// are the two facts, and this is their product.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    Heal,
    Clone,
    RedEye,
    PetEye,
}

impl Mode {
    pub(super) fn current(spot: SpotMode, eye: Option<EyeKind>) -> Self {
        match (eye, spot) {
            (Some(EyeKind::Red), _) => Self::RedEye,
            (Some(EyeKind::Pet), _) => Self::PetEye,
            (None, SpotMode::Heal) => Self::Heal,
            (None, SpotMode::Clone) => Self::Clone,
        }
    }

    fn eye(self) -> Option<EyeKind> {
        match self {
            Self::RedEye => Some(EyeKind::Red),
            Self::PetEye => Some(EyeKind::Pet),
            Self::Heal | Self::Clone => None,
        }
    }
}

pub(super) const RED_EYE_HINT: &str = "Removes the red from a pupil you click";
pub(super) const PET_EYE_HINT: &str = "Darkens an animal's glowing pupil you click";

/// Apply a mode chosen in the selector to the eye side. Returns the spot mode to apply when the
/// new mode is a patch mode (the caller owns the patch side), `None` otherwise.
///
/// ⚠ To an eye mode: the patch selection is dropped, and a selected eye takes the new kind — one
/// undo step. To a patch mode: the eye selection is dropped and the tool leaves eye mode.
pub(super) fn apply_mode(
    s: &mut Session,
    eyes: &mut EyeTool,
    to: Mode,
    ed: &mut Edited,
) -> Option<SpotMode> {
    match to.eye() {
        Some(kind) => {
            s.selected_spot = None;
            eyes.active = Some(kind);
            if let Some(e) = s
                .selected_eye
                .and_then(|i| s.edits.eyes.get_mut(i))
                .filter(|e| e.kind != kind)
            {
                e.kind = kind;
                e.catchlight = match kind {
                    EyeKind::Red => None,
                    EyeKind::Pet => eyes.catchlight.then_some(FALLBACK_CATCHLIGHT),
                };
                ed.discrete();
            }
            None
        }
        None => {
            eyes.active = None;
            s.selected_eye = None;
            Some(if to == Mode::Clone {
                SpotMode::Clone
            } else {
                SpotMode::Heal
            })
        }
    }
}

/// The Eyes list: one row per eye, click selects, `×` deletes.
pub(super) fn list(ui: &mut egui::Ui, s: &mut Session, eyes: &mut EyeTool, ed: &mut Edited) {
    let n = s.edits.eyes.len();
    let count = match n {
        0 => "no eyes".to_owned(),
        1 => "1 eye".to_owned(),
        n => format!("{n} eyes"),
    };
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Eyes")),
        true,
        section::Header::new("Eyes")
            .modified(n > 0)
            .resettable(true)
            .trailing(&count),
    );
    if out.reset {
        s.edits.eyes.clear();
        s.selected_eye = None;
        s.suppressed_eye = None;
        ed.discrete();
    }
    out.show_body(ui, |ui| {
        let long = s.full_long;
        let selected = s.selected_eye.filter(|&i| i < s.edits.eyes.len());
        let (mut pick, mut remove) = (None, None);
        for (i, e) in s.edits.eyes.iter().enumerate() {
            let name = format!("Eye {}", i + 1);
            let kind = match e.kind {
                EyeKind::Red => "Red",
                EyeKind::Pet => "Pet",
            };
            let meta = format!("{kind} · {:.0} px", spot_tool::radius_px(e.radius, long));
            let row = mask_components::row(
                ui,
                mask_components::RowSpec {
                    glyph: mask_components::RowGlyph::Eye(e.kind),
                    op: None,
                    name: &name,
                    meta: &meta,
                    accessible: &name,
                    selected: selected == Some(i),
                    current: false,
                    deletable: true,
                    indent: 0.0,
                    eye: None,
                    hold: None,
                    reorder: None,
                },
            );
            if row.delete {
                remove = Some(i);
            } else if row.clicked {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            s.selected_eye = Some(i);
            s.selected_spot = None;
            eyes.active = Some(s.edits.eyes[i].kind);
        }
        if let Some(i) = remove {
            let before = s.edits.eyes.len();
            s.edits.eyes.remove(i);
            s.selected_eye = mask_tool::selection_after_delete(s.selected_eye, i, before);
            if s.suppressed_eye == Some(i) {
                s.suppressed_eye = None;
            }
            ed.discrete();
        }
    });
}

/// Pupil Size, Darken and (pet) Catchlight for the selected eye — or, with none selected, the
/// tool's values for the next one. A new eye inherits the last values used.
pub(super) fn settings(ui: &mut egui::Ui, s: &mut Session, tool: &mut EyeTool, ed: &mut Edited) {
    let sel = s.selected_eye.filter(|&i| i < s.edits.eyes.len());
    let d = EyeTool::default();
    let (mut size, mut dark) = match sel {
        Some(i) => (s.edits.eyes[i].pupil_size, s.edits.eyes[i].darken),
        None => (tool.pupil_size, tool.darken),
    };
    // ⚠ A tool default is not a photo edit: with nothing selected the sliders must neither render
    // nor push history — `retouch::settings`' rule for its Next patch row.
    let mut local = Edited {
        help_scope: rustroom_app::slider_help::Scope::Eye,
        ..Edited::default()
    };
    local.slider(ui, &mut size, 0.0..=100.0, "Pupil Size", d.pupil_size);
    local.slider(ui, &mut dark, 0.0..=100.0, "Darken", d.darken);
    ed.missing_help.append(&mut local.missing_help);
    tool.pupil_size = size;
    tool.darken = dark;
    if let Some(i) = sel {
        s.edits.eyes[i].pupil_size = size;
        s.edits.eyes[i].darken = dark;
        ed.changed |= local.changed;
        ed.settled |= local.settled;
    }
    let pet = sel.map_or(tool.active == Some(EyeKind::Pet), |i| {
        s.edits.eyes[i].kind == EyeKind::Pet
    });
    if pet {
        let mut on = sel.map_or(tool.catchlight, |i| s.edits.eyes[i].catchlight.is_some());
        if ui
            .checkbox(&mut on, "Catchlight")
            .hover("Adds a small highlight to the pupil so it looks alive")
            .changed()
        {
            tool.catchlight = on;
            if let Some(i) = sel {
                // ⚠ Re-ticking uses a fixed up-left in the stage's pre-orientation frame: only a
                // canvas click knows which way is up on screen.
                s.edits.eyes[i].catchlight = on.then_some(FALLBACK_CATCHLIGHT);
                ed.discrete();
            }
        }
    }
}

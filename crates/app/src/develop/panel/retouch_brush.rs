//! Retouch's brush (M10.19d): **Remove · Heal · Clone · Eye** and the settings every NEW retouch takes. A selected
//! retouch's own settings sit under the list (`retouch_items`) — the brush never edits one.

use super::{retouch_eyes, section, Edited};
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use edit_model::{EyeKind, SpotMode};
use eframe::egui;
use rustroom_app::eye_tool::EyeTool;
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::retouch_list::Brush;
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::{self, SpotTool, MAX_RADIUS, MIN_RADIUS};

pub(super) const REMOVE_HINT: &str =
    "Paints over something to remove, filled from its surroundings";
pub(super) const HEAL_HINT: &str =
    "Removes a spot by blending in nearby texture that matches its surroundings";
pub(super) const CLONE_HINT: &str = "Removes a spot by copying another area exactly";
const EYE_HINT: &str = "Fixes a red or glowing pupil you click";

/// The brush the tools say: Remove's `active`, else the eye tool's, else the patch tool's mode.
pub(super) fn current(tool: &SpotTool, eyes: &EyeTool, remove: &RemoveTool) -> Brush {
    if remove.active {
        Brush::Remove
    } else if eyes.active.is_some() {
        Brush::Eye
    } else if tool.mode == SpotMode::Clone {
        Brush::Clone
    } else {
        Brush::Heal
    }
}

/// Switch to brush `b`. The selection a brush's photo view does not draw is dropped — Remove draws removals,
/// Heal and Clone patches, Eye eyes — so a selected retouch is always one the photo shows.
pub(super) fn set(
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    remove: &mut RemoveTool,
    b: Brush,
) {
    remove.active = b == Brush::Remove;
    eyes.active = match b {
        Brush::Eye => Some(eyes.active.unwrap_or(EyeKind::Red)),
        _ => None,
    };
    match b {
        Brush::Heal => tool.mode = SpotMode::Heal,
        Brush::Clone => tool.mode = SpotMode::Clone,
        Brush::Remove | Brush::Eye => {}
    }
    if b != Brush::Remove {
        s.removal.selected = None;
    }
    if !matches!(b, Brush::Heal | Brush::Clone) {
        s.selected_spot = None;
    }
    if b != Brush::Eye {
        s.selected_eye = None;
    }
}

/// The "Brush" card.
pub(super) fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    remove: &mut RemoveTool,
    ed: &mut Edited,
) {
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Brush")),
        true,
        section::Header::new("Brush"),
    );
    out.show_body(ui, |ui| {
        let was = current(tool, eyes, remove);
        let mut b = was;
        let font = egui::TextStyle::Body.resolve(ui.style());
        theme::segmented_sized(
            ui,
            &mut b,
            &[
                (Brush::Remove, "Remove", REMOVE_HINT),
                (Brush::Heal, "Heal", HEAL_HINT),
                (Brush::Clone, "Clone", CLONE_HINT),
                (Brush::Eye, "Eye", EYE_HINT),
            ],
            28.0,
            font,
        );
        if b != was {
            set(s, tool, eyes, remove, b);
        }
        let long = s.full_long;
        match b {
            Brush::Remove => {
                size(ui, &mut remove.radius, long, Scope::Remove);
                ui.checkbox(&mut remove.detect, "Whole object")
                    .hover("Grows each new stroke to the whole object under it");
            }
            Brush::Heal | Brush::Clone => {
                size(ui, &mut tool.radius, long, Scope::Patch);
                ui.add(
                    super::slider::Slider::new(&mut tool.feather, 0.0..=100.0)
                        .text("Feather")
                        .help(slider_help::text(Scope::Patch, "Feather"))
                        .reset_to(SpotTool::default().feather),
                );
            }
            Brush::Eye => {
                let mut kind = eyes.active.unwrap_or(EyeKind::Red);
                retouch_eyes::kind_switch(ui, &mut kind);
                eyes.active = Some(kind);
                retouch_eyes::brush_settings(ui, eyes, ed);
            }
        }
    });
}

/// A brush's Size, in decode pixels on screen, stored as a share of the long edge.
fn size(ui: &mut egui::Ui, radius: &mut f32, long: usize, scope: Scope) {
    let mut px = spot_tool::radius_px(*radius, long);
    let r = ui.add(
        super::slider::Slider::new(
            &mut px,
            spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
        )
        .logarithmic(true)
        .text("Size")
        .help(slider_help::text(scope, "Size"))
        .custom_formatter(|v| format!("{v:.0} px")),
    );
    if r.changed() {
        *radius = spot_tool::radius_from_px(px, long);
    }
}

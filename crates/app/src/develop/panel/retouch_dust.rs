//! The Retouch panel's Remove dust (M10.16c2): the button and its Sensitivity, beside the Find dust view.
//!
//! ⚠ Its own file, `retouch_eyes`'s reason: `retouch.rs` sits at the line ceiling.

use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::SpotTool;

/// The line under the button while it looks.
const LOOKING_HOVER: &str =
    "Each spot of sensor dust it finds becomes a Heal patch you can move or delete";

/// Remove dust, what it is doing or what it did, and its Sensitivity (the author, 2026-10-20: a press
/// "often" seemed to do nothing). The search and its patches are `dust_flow`'s.
pub(super) fn show(ui: &mut egui::Ui, s: &mut Session, tool: &mut SpotTool) {
    ui.add_space(6.0);
    let looking = s.dust.search.is_some();
    if ui
        .add_enabled(!looking, egui::Button::new("Remove dust"))
        .hover("Finds sensor dust in the photo and heals each spot with a patch")
        .clicked()
    {
        s.press_find_dust(tool.dust);
    }
    if let Some(search) = &s.dust.search {
        super::progress_line::show(ui, "Looking for dust", search.progress(), LOOKING_HOVER);
    } else if s.dust.queued.is_some() {
        theme::hint(ui, "Waits for the photo to load");
    } else if let Some(said) = &s.dust.said {
        theme::hint(ui, said);
    }
    ui.add(
        super::slider::Slider::new(&mut tool.dust, 0.0..=100.0)
            .text("Sensitivity")
            .help(slider_help::text(Scope::Patch, "Sensitivity"))
            .reset_to(rustroom_app::dust_tool::DEFAULT_SENSITIVITY),
    );
}

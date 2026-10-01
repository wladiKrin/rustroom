//! Settings → Appearance → where the panels sit (M10.8b): the Library rail, the edit panel and the
//! filmstrip, each on its own.
//!
//! ⚠ Its own module: `settings.rs` is over 800 lines of code. These are rows of the Appearance grid —
//! the caller has opened it and ends nothing here but these rows.
//!
//! ⚠ They were View menu items for one build; the author moved them here (2026-10-01): where a panel
//! sits is a setting, not something toggled while working.

use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::layout::{Layout, Side, StripPlace};

/// The three rows, each a segmented control.
pub(super) fn rows(ui: &mut egui::Ui, layout: &mut Layout) {
    ui.label("Library rail")
        .hover("Sets which side of the Library the folders and collections rail sits on");
    crate::segmented::segmented(
        ui,
        &mut layout.library_rail,
        &[
            (Side::Left, "Left", "Puts the Library rail on the left"),
            (Side::Right, "Right", "Puts the Library rail on the right"),
        ],
    );
    ui.end_row();

    ui.label("Edit panel")
        .hover("Sets which side of Develop the sliders and the tool rail sit on");
    crate::segmented::segmented(
        ui,
        &mut layout.edit_panel,
        &[
            (Side::Left, "Left", "Puts the edit panel on the left"),
            (Side::Right, "Right", "Puts the edit panel on the right"),
        ],
    );
    ui.end_row();

    ui.label("Filmstrip")
        .hover("Sets where Develop's filmstrip sits");
    crate::segmented::segmented(
        ui,
        &mut layout.filmstrip,
        &[
            (
                StripPlace::Bottom,
                "Bottom",
                "Puts the filmstrip along the bottom",
            ),
            (
                StripPlace::Left,
                "Left",
                "Puts the filmstrip in a column on the left",
            ),
            (
                StripPlace::Right,
                "Right",
                "Puts the filmstrip in a column on the right",
            ),
        ],
    );
    ui.end_row();
}

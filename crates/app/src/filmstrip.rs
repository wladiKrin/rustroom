//! The Develop filmstrip: the loaded folder's thumbnails along the bottom.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores
//! its *content's* rendered rect as its size each frame, so content smaller than the panel
//! makes it shrink back every frame. Applies to any future resizable panel whose content does
//! not fill the resize axis.

use crate::{apply_click, thumb, RustroomApp};
use eframe::egui;

/// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
/// The panel is resizable (drag its top border); thumbnails scale with its height.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut open_index: Option<usize> = None;
    // Decided before the panel borrows `app.library`, so the closure only needs two
    // `Copy` locals. Opening from the Library grid and arrow-navigating both land here.
    let active = app.library.as_ref().and_then(|l| l.selection.active());
    let want_scroll = active.is_some() && app.filmstrip_scrolled_to != active;
    egui::Panel::bottom("filmstrip")
        .resizable(true)
        .default_size(96.0)
        .show(ui, |ui| {
            let Some(lib) = &mut app.library else { return };
            // Fill the panel's height so egui stores the panel size (not the smaller
            // content size) — otherwise a resizable bottom panel shrinks back each frame.
            let avail_h = ui.available_height();
            ui.set_min_height(avail_h);
            let cell_h = (avail_h - 8.0).clamp(48.0, 400.0);
            let box_size = egui::vec2(cell_h * 1.5, cell_h);
            let visible = lib.visible.clone();
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for &i in &visible {
                        // The filmstrip's cell *is* its click target, so it opts in here.
                        let resp = thumb::cell(
                            ui,
                            box_size,
                            lib.thumbs[i].as_ref(),
                            lib.entries[i].id.copy,
                            lib.marks
                                .get(&lib.entries[i].id)
                                .copied()
                                .unwrap_or_default(),
                        )
                        .interact(egui::Sense::click());
                        thumb::selection_border(
                            ui,
                            resp.rect,
                            lib.selection.active() == Some(i),
                            lib.selection.contains(i),
                        );
                        if want_scroll && Some(i) == active {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                        if resp.clicked() {
                            let mods = ui.input(|inp| inp.modifiers);
                            // Only a plain click moves the active photo, so only a
                            // plain click opens — a Cmd- or Shift-click builds the
                            // target set without triggering a decode.
                            if apply_click(&mut lib.selection, i, mods) {
                                open_index = Some(i);
                            }
                        }
                    }
                });
            });
        });
    if want_scroll {
        app.filmstrip_scrolled_to = active;
    }
    if let Some(i) = open_index {
        app.open_entry(ctx, i);
    }
}

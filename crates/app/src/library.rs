//! The Library grid: a page of thumbnails, filtered by format, with selection.

use crate::{apply_click, name_edit, thumb, RustroomApp};
use eframe::egui;
use rustroom_app::format_filter;
use rustroom_app::photo_id::PhotoId;

/// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut open_index: Option<usize> = None;
    // ⚠ Collected here and applied after the panel closes: committing needs `&mut RustroomApp`
    // while the closure below still holds `&mut app.library`. Same shape `open_index` uses.
    let mut rename: Option<(PhotoId, name_edit::Rename)> = None;
    // Pinch on a trackpad, or ctrl/cmd+scroll, resizes the grid — the same gesture the develop
    // canvas uses to zoom the photo.
    //
    // ⚠ `zoom_delta` folds trackpad pinch *and* ctrl+scroll into one **multiplicative factor
    // that is 1.0 when idle**, which is why plain scrolling still scrolls the grid rather than
    // resizing it.
    let zoom = ui.ctx().input(|i| i.zoom_delta());
    if zoom != 1.0 {
        app.settings.library_cell = app.settings.zoomed_cell(zoom);
        app.cell_zoom_pending = true;
    } else if app.cell_zoom_pending {
        // ⚠ Saved when the gesture *settles*, not per frame: a pinch runs at 60 Hz and writing
        // the settings file that often is the `apply_view_demosaic` mistake in a new costume.
        app.cell_zoom_pending = false;
        app.save_settings();
    }

    // ⚠ Borrowed out of `app` up front, because the grid closure already holds `&mut app.library`
    // and the two are disjoint fields — which the borrow checker only accepts when it can see so.
    let rename_draft = &mut app.rename_draft;
    // ⚠ From the settings, not a constant. Everything below is expressed as a fraction of it so
    // the thumbnail, the cell and the name all grow together — a fixed caption beside a scaling
    // thumbnail is what makes a size control look broken at the extremes.
    let cell = app.settings.clamped_cell();
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = &mut app.library else {
            ui.centered_and_justified(|ui| {
                ui.label("Open a folder to browse (Open folder… button).");
            });
            return;
        };
        // A folder can be reloaded with fewer photos than the selection remembers. This
        // guards a *different* failure from filtering, so it stays.
        lib.selection.clamp(lib.entries.len());
        if lib.visible.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(if lib.entries.is_empty() {
                    "No supported images in this folder."
                } else {
                    "Every format is hidden — re-enable one above."
                });
            });
            return;
        }

        // Keyboard: arrows move the selection, Enter opens it.
        //
        // ⚠⚠ **Suppressed while anything has keyboard focus.** With a name being edited, arrow
        // keys must move the cursor inside the field and Enter must commit — not step to the next
        // photo and open it. This is the same gate the filmstrip's arrow navigation already uses,
        // and it was simply missing here.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        ui.input(|i| {
            if typing {
                return;
            }
            // Steps through the *visible* list, so arrows skip a hidden format rather
            // than landing on gaps.
            let cur = lib.selection.active().unwrap_or(0);
            if i.key_pressed(egui::Key::ArrowRight) {
                if let Some(n) = format_filter::step(&lib.visible, cur, true) {
                    lib.selection.click(n);
                }
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                if let Some(n) = format_filter::step(&lib.visible, cur, false) {
                    lib.selection.click(n);
                }
            }
            if i.key_pressed(egui::Key::Enter) {
                open_index = lib.selection.active();
            }
            if i.key_pressed(egui::Key::A) && i.modifiers.command {
                // Visible only: selecting hidden photos is the very hazard the prune
                // exists to prevent.
                lib.selection.select_indices(&lib.visible);
            }
            if i.key_pressed(egui::Key::Escape) {
                lib.selection.clear();
            }
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            let cols = grid_columns(ui.available_width(), cell, ui.spacing().item_spacing.x);
            // Cloned so the loop body can mutate `lib.selection`. A few hundred `usize`
            // per frame is nothing against the thumbnails being drawn.
            let visible = lib.visible.clone();
            // ⚠ The caption scales *relative to the theme's own* Small size rather than from a
            // literal, so a later change to the type scale carries through here for free.
            let name_size = ui.style().text_styles[&egui::TextStyle::Small].size * cell
                / rustroom_app::settings::DEFAULT_CELL as f32;
            egui::Grid::new("thumb_grid").show(ui, |ui| {
                // ⚠ `pos` is the cell's place in the grid; `i` indexes `entries`. The row
                // break must use `pos`: once a filter is on, `i` is *sparse*, so counting
                // rows by it breaks after the wrong cells — gaps where a break was skipped
                // and overflowing rows where several were.
                for (pos, &i) in visible.iter().enumerate() {
                    let box_size = egui::vec2(cell - 12.0, (cell - 12.0) * 0.66);
                    let resp = ui
                        .allocate_ui(egui::vec2(cell, cell), |ui| {
                            // Pin the cell to its full size. Without this `allocate_ui`
                            // reports only the space the content used, so cells varied
                            // with their thumbnail — the filmstrip's reflow bug in its
                            // grid form, where `egui::Grid`'s column alignment hid it.
                            ui.set_min_size(egui::vec2(cell, cell));
                            ui.vertical_centered(|ui| {
                                thumb::cell(
                                    ui,
                                    box_size,
                                    lib.thumbs[i].as_ref(),
                                    lib.entries[i].id.copy,
                                );
                                let e = &lib.entries[i];
                                // ⚠⚠ `label()`, not `file_name`. Every copy of a file has the
                                // same `file_name`, so the grid showed three identical captions
                                // and a copy's own name could never appear in it at all.
                                //
                                // ⚠ Only the **active** row is editable. A text field in every
                                // cell would fight the click that selects a photo, and would put
                                // hundreds of them on screen at once; requiring the photo to be
                                // selected first is the gesture people already make.
                                if let Some(r) = name_edit::caption_or_field(
                                    ui,
                                    &e.id,
                                    &e.file_name,
                                    e.name.as_deref(),
                                    rename_draft,
                                    name_size,
                                ) {
                                    rename = Some((e.id.clone(), r));
                                }
                            });
                        })
                        .response
                        .interact(egui::Sense::click());

                    thumb::selection_border(
                        ui,
                        resp.rect,
                        lib.selection.active() == Some(i),
                        lib.selection.contains(i),
                    );
                    if resp.clicked() {
                        let mods = ui.input(|inp| inp.modifiers);
                        apply_click(&mut lib.selection, i, mods);
                    }
                    if resp.double_clicked() {
                        open_index = Some(i);
                    }
                    if (pos + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        });
    });

    if let Some((id, r)) = rename {
        name_edit::commit(app, ctx, &id, r);
    }
    if let Some(i) = open_index {
        app.open_entry(ctx, i);
    }
}

/// Outline a selected cell: bright for the active photo, dim for the rest of the selection.
/// Being able to tell the source from the targets is what makes a sync predictable.
/// How many `cell`-wide columns fit in `avail`, given egui's inter-column `spacing`.
///
/// ⚠ `n` cells need `n*cell + (n-1)*spacing`, **not** `n*cell`. Ignoring the gaps overflows
/// whenever the leftover width is smaller than the gaps the columns need — and that gets *more*
/// likely as the window widens, because the gap total grows with the column count while the
/// leftover stays uniform in `[0, cell)`. Which is exactly why it showed on a large monitor and
/// not a small one: at 14 columns the gaps want 104 px of slack, at 7 only 48.
fn grid_columns(avail: f32, cell: f32, spacing: f32) -> usize {
    if cell <= 0.0 {
        return 1;
    }
    (((avail + spacing) / (cell + spacing)).floor() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out `visible` cells in a grid of `cols` columns, breaking rows the way the Library
    /// grid does, and return how many distinct rows the cells landed on.
    ///
    /// `by_index` reproduces the bug: counting rows by the *entry* index rather than the
    /// cell's position in the grid.
    fn grid_rows(visible: &[usize], cols: usize, by_index: bool) -> usize {
        let ctx = egui::Context::default();
        let mut tops: Vec<i32> = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::Grid::new("t").show(ui, |ui| {
                for (pos, &i) in visible.iter().enumerate() {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    tops.push(rect.top() as i32);
                    let counter = if by_index { i } else { pos };
                    if (counter + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        });
        tops.sort_unstable();
        tops.dedup();
        tops.len()
    }

    #[test]
    fn grid_columns_leaves_room_for_the_gaps() {
        // ⚠ The property, over a sweep of window widths: the chosen columns must *fit* with
        // their gaps, and one more must not — so it is neither overflowing nor wasteful.
        const CELL: f32 = 168.0;
        const GAP: f32 = 8.0;
        let width_of = |n: f32| n * CELL + (n - 1.0) * GAP;
        for avail in [300.0f32, 700.0, 1200.0, 1850.0, 2400.0, 3000.0] {
            let n = grid_columns(avail, CELL, GAP) as f32;
            assert!(
                width_of(n) <= avail,
                "{n} columns need {} > {avail}",
                width_of(n)
            );
            assert!(
                width_of(n + 1.0) > avail,
                "{} columns would also have fit in {avail}",
                n + 1.0
            );
        }

        // The naive `avail / cell` overflows precisely where this does not — the bug, pinned.
        let naive = (2400.0f32 / CELL).floor();
        assert!(
            width_of(naive) > 2400.0,
            "the naive formula should overflow"
        );
        assert!((grid_columns(2400.0, CELL, GAP) as f32) < naive);

        // Degenerate inputs still give a usable answer rather than 0 columns or a panic.
        assert_eq!(grid_columns(0.0, CELL, GAP), 1);
        assert_eq!(grid_columns(-50.0, CELL, GAP), 1);
        assert_eq!(grid_columns(500.0, 0.0, GAP), 1);
    }

    #[test]
    fn the_grid_breaks_rows_by_position_not_by_entry_index() {
        // ⚠ Once a format is filtered out the visible indices are *sparse*, so counting rows
        // by the entry index breaks after the wrong cells — gaps where a break was skipped,
        // overflowing rows where several were. Six cells in three columns is two rows.
        let sparse = vec![0usize, 2, 4, 6, 8, 10];
        assert_eq!(grid_rows(&sparse, 3, false), 2);

        // The bug, reproduced: every index here is even, so `(i + 1) % 3 == 0` fires only on
        // i = 2 and i = 8 — three rows of wildly uneven length instead of two.
        assert_ne!(
            grid_rows(&sparse, 3, true),
            2,
            "the index-counted version should not lay out correctly"
        );

        // Dense indices are the case that always worked, and must keep working.
        let dense: Vec<usize> = (0..6).collect();
        assert_eq!(grid_rows(&dense, 3, false), 2);
        assert_eq!(grid_rows(&dense, 3, true), 2);
    }
}

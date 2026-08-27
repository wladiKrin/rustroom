//! The Library grid: a page of thumbnails, filtered by format, with selection.

use crate::{apply_click, name_edit, thumb, RustroomApp};
use eframe::egui;
use rustroom_app::format_filter;
use rustroom_app::photo_id::PhotoId;
use std::path::PathBuf;

/// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut open_index: Option<usize> = None;
    // ⚠ Collected here and applied after the panel closes: committing needs `&mut RustroomApp`
    // while the closure below still holds `&mut app.library`. Same shape `open_index` uses.
    let mut rename: Option<(PhotoId, name_edit::Rename)> = None;
    // ⚠ Declared out here, like `open_index` and `rename`: applying it needs `&mut RustroomApp`
    // while the closure below still holds `&mut app.library`.
    let mut mark: Option<rustroom_app::marks::MarkAction> = None;
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
            // Culling. ⚠⚠ Behind the **same** `typing` gate as the arrows above, and that is not
            // optional: the Library has an editable filename field and a new-collection field,
            // and a bare `5` typed into either must not rate a photo. This project has recorded
            // that hazard twice.
            for (c, key) in [
                ('0', egui::Key::Num0),
                ('1', egui::Key::Num1),
                ('2', egui::Key::Num2),
                ('3', egui::Key::Num3),
                ('4', egui::Key::Num4),
                ('5', egui::Key::Num5),
                ('6', egui::Key::Num6),
                ('7', egui::Key::Num7),
                ('8', egui::Key::Num8),
                ('9', egui::Key::Num9),
                ('p', egui::Key::P),
                ('x', egui::Key::X),
                ('u', egui::Key::U),
            ] {
                // ⚠ No modifier: ⌘X is cut and ⌘A is select-all. A culling key is bare or it is
                // not a culling key.
                if i.key_pressed(key) && !i.modifiers.any() {
                    mark = rustroom_app::marks::action_for(c);
                }
            }
        });

        // ⚠ How many rows of prefetch to keep resident beyond the drawn ones. Latency, not
        // correctness: at 0 the grid is still right, it just shows grey while you scroll. No test
        // covers this number for that reason.
        const MARGIN_ROWS: usize = 2;

        let cols = grid_columns(ui.available_width(), cell, ui.spacing().item_spacing.x);
        // Cloned so the loop body can mutate `lib.selection`. A few hundred `usize`
        // per frame is nothing against the thumbnails being drawn.
        let visible = lib.visible.clone();
        let rows = visible.len().div_ceil(cols);
        // ⚠ The caption scales *relative to the theme's own* Small size rather than from a
        // literal, so a later change to the type scale carries through here for free.
        let name_size = ui.style().text_styles[&egui::TextStyle::Small].size * cell
            / rustroom_app::settings::DEFAULT_CELL as f32;

        // ⚠ `show_rows` adds `item_spacing.y` itself, so the height passed is the bare cell.
        let drawn = egui::ScrollArea::vertical()
            .show_rows(ui, cell, rows, |ui, row_range| {
                for row in row_range.clone() {
                    ui.horizontal(|ui| {
                        for pos in cells_in_row(row, cols, visible.len()) {
                            let i = visible[pos];
                            // ⚠⚠ A stable id per photo, not egui's auto-id counter. Under
                            // `show_rows` that counter shifts with the scroll position — it skips
                            // `min_row` ids while each row holds `cols` widgets — which silently
                            // kills the rename field's focus: the stale id keeps focus, so
                            // `autofocus`'s `focused().is_none()` guard never fires and the box
                            // goes dead. Keyed by `PhotoId`, so it is also stable across sorting
                            // and filtering.
                            ui.push_id(&lib.entries[i].id, |ui| {
                                let box_size = egui::vec2(cell - 12.0, (cell - 12.0) * 0.66);
                                let resp = ui
                                    .allocate_ui(egui::vec2(cell, cell), |ui| {
                                        // Pin the cell to its full size. Without this
                                        // `allocate_ui` reports only the space the content used,
                                        // so cells varied with their thumbnail — the filmstrip's
                                        // reflow bug in its grid form.
                                        ui.set_min_size(egui::vec2(cell, cell));
                                        ui.vertical_centered(|ui| {
                                            thumb::cell(
                                                ui,
                                                box_size,
                                                lib.cache.get(&lib.entries[i].id.path),
                                                lib.entries[i].id.copy,
                                                lib.marks
                                                    .get(&lib.entries[i].id)
                                                    .copied()
                                                    .unwrap_or_default(),
                                            );
                                            let e = &lib.entries[i];
                                            // ⚠⚠ `label()`, not `file_name`. Every copy of a file
                                            // has the same `file_name`, so the grid showed
                                            // identical captions and a copy's own name could
                                            // never appear in it at all.
                                            //
                                            // ⚠ Only the **active** row is editable. A text field
                                            // in every cell would fight the click that selects a
                                            // photo, and would put hundreds of them on screen.
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
                            });
                        }
                    });
                }
                row_range
            })
            .inner;

        // ⚠ The window is the drawn rows widened by the prefetch margin, mapped through
        // positions to entry indices to paths. Built after drawing so it reflects what egui
        // actually laid out this frame.
        let first = drawn.start.saturating_sub(MARGIN_ROWS);
        let last = drawn.end.saturating_add(MARGIN_ROWS).min(rows);
        let mut window: Vec<PathBuf> = Vec::new();
        for row in first..last {
            for pos in cells_in_row(row, cols, visible.len()) {
                let path = &lib.entries[visible[pos]].id.path;
                // ⚠ De-duplicated: a file's copies are separate rows sharing one path, and the
                // cache holds one entry per path.
                if !window.iter().any(|p| p == path) {
                    window.push(path.clone());
                }
            }
        }
        lib.sync_thumb_window(&window);
    });

    if let Some(a) = mark {
        app.apply_mark(a);
    }
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
/// The positions in `visible` that belong to grid row `row`.
///
/// ⚠ Positions, never entry indices. Once a format or star filter is on, `visible` is *sparse*;
/// the grid's geometry is a function of the *position* in that list, and conflating the two is
/// the bug this project has now made three times.
fn cells_in_row(row: usize, cols: usize, n_visible: usize) -> std::ops::Range<usize> {
    let start = row.saturating_mul(cols).min(n_visible);
    let end = start.saturating_add(cols).min(n_visible);
    start..end
}

fn grid_columns(avail: f32, cell: f32, spacing: f32) -> usize {
    if cell <= 0.0 {
        return 1;
    }
    (((avail + spacing) / (cell + spacing)).floor() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rows_consume_the_visible_positions_densely_and_in_order() {
        // ⚠ Replaces `the_grid_breaks_rows_by_position_not_by_entry_index`. That bug — counting
        // rows by a *sparse* entry index — is now structurally impossible, because a row is
        // computed from the position directly. What still has to hold is that the rows together
        // cover every position exactly once, in order, with only the last one partial.
        const COLS: usize = 4;
        for n in [0usize, 1, 3, 4, 5, 11, 12] {
            let rows = n.div_ceil(COLS);
            let mut seen = Vec::new();
            for row in 0..rows {
                let span = cells_in_row(row, COLS, n);
                assert!(
                    span.len() <= COLS,
                    "row {row} of {n} holds {} cells",
                    span.len()
                );
                seen.extend(span);
            }
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "n = {n}");
        }
    }

    #[test]
    fn a_row_past_the_end_is_empty() {
        // A stale scroll offset can name a row that no longer exists after a filter change.
        assert!(cells_in_row(99, 4, 10).is_empty());
    }
}

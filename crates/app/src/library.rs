//! The Library grid: a page of thumbnails, filtered by format, with selection.

use crate::{apply_click, name_edit, thumb, RustroomApp};
use eframe::egui;
use rustroom_app::format_filter;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::thumb_loader::ThumbRequest;
use std::path::PathBuf;

/// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
/// The entry indices whose cells the rubber band touches.
///
/// ⚠ Pure, so the geometry is testable without a window — the `thumb_window` precedent. The rects
/// come from `Response::rect` and are therefore screen-space, which is also what the band is.
///
/// ⚠ **Intersects, not contains.** A band must catch a photo it merely clips, or selecting a run
/// would need the rectangle drawn precisely around every thumbnail.
/// ⚠ Takes the drag's two **corners**, not a rectangle, so the normalisation lives here rather
/// than at the call site. It was in the caller first, which made the "backwards drag" test
/// exercise egui's `from_two_pos` instead of our own use of it — a test that named a bug it could
/// not detect, caught by the sabotage step.
fn banded(cells: &[(usize, egui::Rect)], start: egui::Pos2, now: egui::Pos2) -> Vec<usize> {
    let band = egui::Rect::from_two_pos(start, now);
    cells
        .iter()
        .filter(|(_, r)| r.intersects(band))
        .map(|(i, _)| *i)
        .collect()
}

/// What a photo's right-click menu can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellAction {
    /// Open it in Develop.
    Edit,
    /// Put its caption into edit mode.
    Rename,
    /// Show everything known about it.
    Info,
    /// Add every target to a collection.
    ///
    /// ⚠ Acts on the **selection**, not on the right-clicked index — which is why the menu
    /// selects the photo first when it is not already selected.
    AddToCollection(i64),
    /// Assign a keyword across the targets, or remove it from all of them.
    SetKeyword { id: i64, on: bool },
}

/// The photos a right-click acts on.
///
/// ⚠⚠ **The clicked photo alone when it is not selected**, because `menu_select` is applied
/// *after* the frame — so while the menu is drawn `lib.selection` still holds the set that is
/// about to be replaced. Reading the selection unconditionally would build the menu for photos
/// the click has already discarded.
pub fn menu_targets(lib: &crate::Library, clicked: usize) -> Vec<PhotoId> {
    if lib.selection.contains(clicked) {
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i))
            .map(|e| e.id.clone())
            .collect()
    } else {
        lib.entries
            .get(clicked)
            .map(|e| vec![e.id.clone()])
            .into_iter()
            .flatten()
            .collect()
    }
}

/// The distinct **files** among some targets.
///
/// ⚠ Keywords are per file and collections per copy, so the two lists differ deliberately: two
/// copies of one photo are one file to tag and two rows to collect.
pub fn distinct_files(targets: &[PhotoId]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for t in targets {
        if !files.contains(&t.path) {
            files.push(t.path.clone());
        }
    }
    files
}

/// The keyword menu's three states.
///
/// ⚠⚠ **Probed, never assumed.** `✔` and `✓` are in **no** bundled font and would have shipped
/// as tofu boxes — the same finding the rail's four glyphs and the mark badges each produced.
/// `every_keyword_mark_is_renderable` is the standing guard.
const MARK_ALL: &str = "■ ";
const MARK_SOME: &str = "– ";
const MARK_NONE: &str = "☐ ";

/// The photo context menu, shared by the grid and the filmstrip so the two cannot drift.
///
/// ⚠ **The catalog queries live in here**, which egui runs only while the menu is open — reading
/// them beside the grid would run three queries at 60 Hz, the `apply_view_demosaic` mistake in
/// another costume.
pub fn cell_menu(
    ui: &mut egui::Ui,
    catalog: Option<&catalog::db::Catalog>,
    targets: &[PhotoId],
) -> Option<CellAction> {
    let mut action = None;
    let n = targets.len();
    // ⚠ Rename and Information are single-photo by nature, so they are **disabled** above one
    // rather than hidden: a menu that changes shape is harder to learn than one that greys out.
    let single = n == 1;
    if ui.button("Edit").clicked() {
        action = Some(CellAction::Edit);
        ui.close();
    }
    if ui
        .add_enabled(single, egui::Button::new("Rename"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::Rename);
        ui.close();
    }
    if ui
        .add_enabled(single, egui::Button::new("Information…"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::Info);
        ui.close();
    }
    let Some(cat) = catalog else {
        return action;
    };
    ui.separator();

    let label = if single {
        "Add to collection".to_string()
    } else {
        format!("Add {n} to collection")
    };
    ui.menu_button(label, |ui| {
        let rows = cat.collections().unwrap_or_default();
        if rows.is_empty() {
            crate::theme::hint(ui, "No collections yet.");
        }
        for c in &rows {
            if ui.button(&c.name).clicked() {
                action = Some(CellAction::AddToCollection(c.id));
                ui.close();
            }
        }
    });

    ui.menu_button("Keywords", |ui| {
        let kws = cat.keywords().unwrap_or_default();
        if kws.is_empty() {
            crate::theme::hint(ui, "No keywords yet.");
        }
        let files = distinct_files(targets);
        let counts = cat.keyword_counts(&files).unwrap_or_default();
        for k in &kws {
            let have = counts.get(&k.id).copied().unwrap_or(0);
            // ⚠ Three states, not two: filled when every file carries it, a dash when only some
            // do. A two-state tick on a mixed selection claims something false about half of it.
            let mark = if have == files.len() && have > 0 {
                MARK_ALL
            } else if have > 0 {
                MARK_SOME
            } else {
                MARK_NONE
            };
            let text = format!("{mark}{}{}", "  ".repeat(k.depth), k.name);
            if ui.button(text).clicked() {
                // Clicking makes the selection uniform: on unless everything already has it.
                let on = !(have == files.len() && have > 0);
                action = Some(CellAction::SetKeyword { id: k.id, on });
                ui.close();
            }
        }
    });
    action
}

/// Apply a context-menu choice. Shared by the grid and the filmstrip so the two cannot drift.
pub fn apply_cell_action(
    app: &mut RustroomApp,
    ctx: &egui::Context,
    index: usize,
    action: CellAction,
) {
    let Some(id) = app
        .library
        .as_ref()
        .and_then(|l| l.entries.get(index).map(|e| e.id.clone()))
    else {
        return;
    };
    match action {
        CellAction::Edit => app.open_entry(ctx, index),
        // ⚠ Reuses `name_edit`'s draft rather than adding a second path. Two editable-name
        // mechanisms drifting is the bug that made copy names invisible in the grid.
        CellAction::Rename => {
            let shown = app
                .library
                .as_ref()
                .and_then(|l| l.entries.get(index))
                .map(|e| e.label())
                .unwrap_or_default();
            app.rename_draft = Some((id, shown));
        }
        CellAction::Info => {
            app.modal = crate::modals::Modal::PhotoInfo { id, dims: None };
        }
        // ⚠ These two act on the targets, not on `index` — see `menu_targets`.
        CellAction::AddToCollection(cid) => app.add_targets_to_collection(cid, index),
        CellAction::SetKeyword { id: kid, on } => app.set_keyword_on_targets(kid, on, index),
    }
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut open_index: Option<usize> = None;
    // ⚠ Collected and applied after the closure, the shape every action in this file uses: the
    // closure holds `lib`, and all three of these need `&mut app`.
    let mut menu: Option<(usize, CellAction)> = None;
    let mut menu_select: Option<usize> = None;
    // ⚠ Only the cells actually drawn this frame. The grid is virtualised, so a band can reach
    // only what is on screen — which is also all it can cover, since it cannot leave the viewport.
    let mut cell_rects: Vec<(usize, egui::Rect)> = Vec::new();
    let mut band_started: Option<egui::Pos2> = None;
    let mut bg_clicked = false;
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

    // ⚠ Hoisted out of `app` before the closure borrows `app.library`: the open photo's edits
    // live in `Session` and the sidecar is stale while it is being edited, so the thumbnail must
    // use these rather than the row's stored copy.
    //
    // ⚠ **Before any `&mut app.*` binding below**: `live_edits` takes `&self`, so it borrows the
    // whole of `app` and cannot follow a mutable borrow of one of its fields.
    let live = app.live_edits();
    // ⚠ Borrowed out of `app` up front, because the grid closure already holds `&mut app.library`
    // and the two are disjoint fields — which the borrow checker only accepts when it can see so.
    let rename_draft = &mut app.rename_draft;
    // ⚠ Same reason: the context menu reads the catalog while the closure holds `&mut
    // app.library`, and the borrow checker only accepts that when the two fields are visibly
    // disjoint.
    let catalog = app.catalog.as_ref();
    // ⚠ From the settings, not a constant. Everything below is expressed as a fraction of it so
    // the thumbnail, the cell and the name all grow together — a fixed caption beside a scaling
    // thumbnail is what makes a size control look broken at the extremes.
    let cell = app.settings.clamped_cell();
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = &mut app.library else {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Open a folder to browse (Open folder… button).");
            });
            return;
        };
        // A folder can be reloaded with fewer photos than the selection remembers. This
        // guards a *different* failure from filtering, so it stays.
        lib.selection.clamp(lib.entries.len());
        if lib.visible.is_empty() {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(
                    ui,
                    if lib.entries.is_empty() {
                        "No supported images in this folder."
                    } else {
                        "Every format is hidden — re-enable one above."
                    },
                );
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

        // ⚠⚠ **A drag-sensing background, registered BEFORE the cells so the cells win the
        // hit-test.** egui breaks a tie by registration order — `hit_test.rs`: "In case of a tie,
        // take the last one = the one on top" — so this must come first, or it would swallow every
        // click meant for a thumbnail. It is what lets a rubber band start in the gaps between
        // cells and in the empty area below the last row.
        //
        // ⚠ The scrollbar is drawn by the `ScrollArea` afterwards and is therefore on top, so
        // dragging it still scrolls rather than banding.
        let bg_rect = ui.available_rect_before_wrap();
        let bg = ui.interact(
            bg_rect,
            ui.id().with("grid_background"),
            egui::Sense::click_and_drag(),
        );
        if bg.drag_started_by(egui::PointerButton::Primary) {
            band_started = bg.interact_pointer_pos();
        }
        // ⚠ A *click* on empty space clears the selection. egui fires `clicked()` only for a
        // press-and-release that did **not** become a drag, so this and the rubber band above
        // cannot both happen from one gesture.
        bg_clicked = bg.clicked();

        // ⚠ `show_rows` adds `item_spacing.y` itself, so the height passed is the bare cell.
        let drawn = egui::ScrollArea::vertical()
            // ⚠ The scrollbar belongs at the window's right edge, not hugging the rightmost
            // thumbnail. With `auto_shrink` egui puts the leftover width *outside* the scroll
            // area, so the bar tracked the grid and moved whenever the column count or cell size
            // changed. `false` puts that space inside instead, pinning the bar.
            .auto_shrink([false, false])
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
                            // ⚠ Cloned rather than borrowed: the closure reads `*lib` whole (the
                            // context menu needs the selection *and* the entries), so a live
                            // borrow of `lib.entries` in the argument would conflict with it.
                            let cell_id = lib.entries[i].id.clone();
                            ui.push_id(&cell_id, |ui| {
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
                                                // ⚠ The exact render if it is there, else this
                                                // photo's previous one — never grey while a
                                                // re-render is in flight.
                                                lib.cache
                                                    .get(&lib.thumb_request(i, live.as_ref()).key())
                                                    .or_else(|| {
                                                        lib.cache.get_stale(&lib.entries[i].id)
                                                    }),
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
                                    // ⚠ `click_and_drag`, so a drag starting on a thumbnail can
                                    // begin a rubber band. A plain click still reports
                                    // `clicked()`; egui distinguishes the two.
                                    .interact(egui::Sense::click_and_drag());
                                cell_rects.push((i, resp.rect));

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
                                // ⚠ `drag_started_by(Primary)`, never a bare `drag_started()`:
                                // egui reports a drag for **every** button, so the pan binding
                                // would otherwise rubber-band as it passed over the grid — the
                                // trap the canvas-navigation work recorded.
                                if resp.drag_started_by(egui::PointerButton::Primary) {
                                    band_started = resp.interact_pointer_pos();
                                }
                                // ⚠⚠ A secondary click does **not** fire `clicked()`, so the
                                // photo is selected here explicitly — otherwise Edit and Rename
                                // would act on whatever was selected before rather than on what
                                // was right-clicked.
                                resp.context_menu(|ui| {
                                    if !lib.selection.contains(i) {
                                        menu_select = Some(i);
                                    }
                                    let targets = menu_targets(lib, i);
                                    if let Some(a) = cell_menu(ui, catalog, &targets) {
                                        menu = Some((i, a));
                                    }
                                });
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
        let mut window: Vec<ThumbRequest> = Vec::new();
        for row in first..last {
            for pos in cells_in_row(row, cols, visible.len()) {
                // ⚠ **No longer de-duplicated by path.** A file's copies were separate rows
                // sharing one cache entry while a thumbnail was the camera's JPEG; now each copy
                // renders its own edits, so each is its own request.
                window.push(lib.thumb_request(visible[pos], live.as_ref()));
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
    // ── Rubber-band selection ────────────────────────────────────────────────────────────────
    //
    // ⚠ Handled after the grid closure, where `lib` is free again and the painter can draw over
    // the cells. The rects were collected inside it.
    {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let mods = ui.input(|i| i.modifiers);
        if let Some(lib) = app.library.as_mut() {
            // ⚠ **Only a plain click**, matching `apply_click`'s rule that only a plain click
            // moves the active photo: a ⌘- or Shift-click on empty space is part of building a
            // selection, not a request to throw it away.
            if bg_clicked && !mods.any() {
                lib.selection.clear();
            }
            if let Some(start) = band_started {
                lib.band = Some((start, start));
            }
            if let Some((start, _)) = lib.band {
                if let Some(now) = pointer {
                    lib.band = Some((start, now));
                    let rect = egui::Rect::from_two_pos(start, now);
                    // (the same normalisation `banded` performs, for the drawn outline)
                    // ⚠ Only once the drag is a real drag: a 1 px band on a plain click would
                    // replace the selection with the single cell under the cursor, which the click
                    // handler is already doing — and would fight it.
                    if rect.width() > 3.0 || rect.height() > 3.0 {
                        let hit = banded(&cell_rects, start, now);
                        // ⚠ Plain drag replaces; ⌘/Shift adds — the rule `apply_click` follows.
                        //
                        // ⚠ The additive case unions and re-selects rather than toggling: a
                        // toggle would *deselect* a photo the band passed over twice, so sweeping
                        // back and forth would flicker the selection.
                        if mods.command || mods.shift {
                            let mut all: Vec<usize> = lib.selection.iter().collect();
                            for i in hit {
                                if !all.contains(&i) {
                                    all.push(i);
                                }
                            }
                            all.sort_unstable();
                            lib.selection.select_indices(&all);
                        } else {
                            lib.selection.select_indices(&hit);
                        }
                        ui.painter().rect_stroke(
                            rect,
                            egui::CornerRadius::same(2),
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 160, 240)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().rect_filled(
                            rect,
                            egui::CornerRadius::same(2),
                            egui::Color32::from_rgba_unmultiplied(120, 160, 240, 24),
                        );
                    }
                }
            }
            if released {
                lib.band = None;
            }
        }
    }

    if let Some(i) = menu_select {
        if let Some(lib) = app.library.as_mut() {
            lib.selection.click(i);
        }
    }
    if let Some((i, action)) = menu {
        apply_cell_action(app, ctx, i, action);
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
    fn a_right_click_inside_the_selection_targets_the_whole_selection() {
        let mut lib = crate::test_app::library_of(5);
        lib.selection.select_indices(&[1, 2, 3]);

        let t = menu_targets(&lib, 2);

        assert_eq!(t.len(), 3, "all three, not just the one clicked");
    }

    #[test]
    fn a_right_click_outside_the_selection_targets_only_that_photo() {
        // ⚠⚠ The reason this function exists. `menu_select` is applied *after* the frame, so
        // while the menu is drawn the selection still holds the set the click is about to
        // replace — reading it would build the menu for photos already discarded.
        let mut lib = crate::test_app::library_of(5);
        lib.selection.select_indices(&[0, 1]);

        let t = menu_targets(&lib, 4);

        assert_eq!(t.len(), 1);
        assert_eq!(t[0], lib.entries[4].id);
    }

    #[test]
    fn two_copies_of_one_photo_are_one_file_to_tag() {
        // ⚠ Keywords are per file and collections per copy, so the two lists differ by design.
        let lib = crate::test_app::library_of(2);
        let mut targets = vec![lib.entries[0].id.clone(), lib.entries[1].id.clone()];
        let mut copy = lib.entries[0].id.clone();
        copy.copy = 1;
        targets.push(copy);

        assert_eq!(targets.len(), 3, "three rows");
        assert_eq!(distinct_files(&targets).len(), 2, "but two files");
    }

    #[test]
    fn every_keyword_mark_is_renderable() {
        // ⚠ A frame must run first — `Context::default()` has not built its atlas, and without
        // `install_fonts` the answer is about egui's defaults, which production no longer uses.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for m in [MARK_ALL, MARK_SOME, MARK_NONE] {
            for c in m.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                    "{c} (U+{:04X}) is in no bundled font and would render as a tofu box",
                    c as u32
                );
            }
        }
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

    #[test]
    fn only_the_visible_rows_are_asked_for() {
        // ⚠⚠ The point of the whole change, asserted where it is observable: with 2,000 entries
        // and a viewport a few rows tall, the want-list must be a few dozen paths, not 2,000.
        //
        // ⚠ Asserted on the *want-list*, not on the drawn cell count: drawing fewer cells is
        // egui's `show_rows` doing its job, while asking for fewer thumbnails is ours. The
        // memory and the 34-second fill are both consequences of the ask.
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(2000));

        let input = egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            show(&mut app, ui, &ctx);
        });

        let asked = app.library.as_ref().unwrap().last_want.len();
        assert!(asked > 0, "nothing was asked for at all");
        assert!(
            asked < 300,
            "asked for {asked} thumbnails — the grid is not windowed"
        );
    }

    #[test]
    fn a_filter_that_hides_a_selected_photo_prunes_the_selection() {
        // ⚠⚠ The hazard `retain_visible` exists for, and the reason `refilter` always does BOTH
        // halves. Select photos, then filter them away, and the selection would still hold them —
        // so a sync or a paste would write to photos that are not on screen. Silent, and
        // discovered as data loss rather than as a bug.
        let mut lib = crate::test_app::library_of(4);
        lib.selection.select_indices(&[0, 1, 2, 3]);
        assert_eq!(lib.selection.len(), 4);
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "nothing matches this".into(),
            ..Default::default()
        };
        lib.refilter(&filter);
        assert!(lib.visible.is_empty(), "the filter did not hide anything");
        assert_eq!(
            lib.selection.len(),
            0,
            "the selection still holds photos that are not on screen"
        );
    }

    #[test]
    fn the_search_text_narrows_without_touching_the_others() {
        // ⚠ Composition through the one `visible` list, asserted end to end on a real `Library`
        // rather than only on `LibraryFilter` in isolation.
        let mut lib = crate::test_app::library_of(20);
        assert_eq!(lib.visible.len(), 20);
        // ⚠ "0001" would match eleven rows — 00001 and 00010..00019 — which is *correct* and was
        // the first version of this assertion, wrong about right code. A substring that occurs in
        // exactly one name is what discriminates.
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "00007".into(),
            ..Default::default()
        };
        lib.refilter(&filter);
        assert_eq!(lib.visible.len(), 1, "expected exactly rustroom_00007.RAF");
    }

    /// A row of four 100×100 cells at y = 0, 10 px apart.
    fn strip() -> Vec<(usize, egui::Rect)> {
        (0..4)
            .map(|i| {
                let x = i as f32 * 110.0;
                (
                    i,
                    egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(100.0, 100.0)),
                )
            })
            .collect()
    }

    #[test]
    fn a_band_catches_every_cell_it_touches() {
        // ⚠ **Intersects, not contains.** A band must catch a photo it merely clips, or selecting
        // a run would mean drawing the rectangle precisely around every thumbnail.
        let cells = strip();
        // Clips the right edge of cell 0 and the left edge of cell 1.
        assert_eq!(
            banded(&cells, egui::pos2(90.0, 40.0), egui::pos2(120.0, 60.0)),
            vec![0, 1]
        );
    }

    #[test]
    fn a_band_over_empty_space_catches_nothing() {
        let cells = strip();
        assert!(banded(&cells, egui::pos2(0.0, 200.0), egui::pos2(400.0, 260.0)).is_empty());
    }

    #[test]
    fn a_band_drawn_upwards_and_leftwards_works_the_same() {
        // ⚠ `Rect::from_two_pos` normalises, so dragging up-left is the same rectangle as
        // down-right. Without it a backwards drag would select nothing at all.
        let cells = strip();
        let forward = banded(&cells, egui::pos2(50.0, 10.0), egui::pos2(230.0, 90.0));
        let backward = banded(&cells, egui::pos2(230.0, 90.0), egui::pos2(50.0, 10.0));
        assert_eq!(forward, backward);
        assert_eq!(forward, vec![0, 1, 2]);
    }

    #[test]
    fn a_band_covering_everything_selects_everything() {
        let cells = strip();
        assert_eq!(
            banded(&cells, egui::pos2(-10.0, -10.0), egui::pos2(500.0, 200.0)),
            vec![0, 1, 2, 3]
        );
    }
}

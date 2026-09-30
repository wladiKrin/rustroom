//! The Library grid: a page of thumbnails, filtered by format, with selection.

mod band;
mod cell_menu;
mod grid;

use crate::interaction::apply_click;
use crate::theme::Hover as _;
use crate::{name_edit, thumb, RustroomApp};
use band::banded;
pub use cell_menu::{apply_cell_action, cell_menu, distinct_files, menu_targets, CellAction};
use eframe::egui;
use grid::{cells_in_row, grid_columns};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::thumb_loader::ThumbRequest;

/// The stack count badge's size.
///
/// ⚠ Small enough not to crowd a 10 pt thumbnail and large enough to hit — the grid marks settled at
/// 10–20 pt for the same reason.
const STACK_BADGE: egui::Vec2 = egui::vec2(20.0, 16.0);

/// What an empty Library says. Also swept by `theme::every_menu_string_is_renderable`.
pub(crate) const EMPTY_LIBRARY_TEXT: &str =
    "You can also drop a folder or some photos onto this window. Nothing is copied or changed: \
     Rustroom reads your photos where they are and keeps its edits beside them.";

/// The empty Library: two real buttons and one sentence, where the first-use review found a
/// footnote in a void (A-08).
fn empty_library(ui: &mut egui::Ui) -> Option<rustroom_app::menu::Command> {
    let mut picked: Option<rustroom_app::menu::Command> = None;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.35).max(24.0));
        ui.label(egui::RichText::new("No photos yet").heading());
        ui.add_space(12.0);
        let big = |text: &str| {
            egui::Button::new(egui::RichText::new(text).size(crate::theme::tokens::TITLE))
                .min_size(egui::vec2(240.0, 36.0))
        };
        if ui
            .add(big("Open a folder…"))
            .hover("Opens a folder of photos (⇧⌘O)")
            .clicked()
        {
            picked = Some(rustroom_app::menu::Command::OpenFolder);
        }
        ui.add_space(6.0);
        if ui
            .add(big("Import from a card…"))
            .hover("Copies new photos from a memory card into dated folders (⇧⌘I)")
            .clicked()
        {
            picked = Some(rustroom_app::menu::Command::Import);
        }
        ui.add_space(14.0);
        ui.set_max_width(460.0);
        crate::theme::hint(ui, EMPTY_LIBRARY_TEXT);
    });
    picked
}

/// How much larger the grid's marks are drawn than its filename caption.
///
/// ⚠ 1.6 (user's call, 2026-09-23): the stars, the reject cross and the colour chip were legible but
/// small. They are what a culling pass actually reads, so they are not tied to the caption's size.
/// ⚠ `thumb::marks_row` clamps the result, so this cannot make them absurd on a huge cell.
const MARK_SCALE: f32 = 1.6;

/// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut open_index: Option<usize> = None;
    // ⚠ Declared out here like `open_index`: the empty state's buttons run a command that needs
    // `&mut RustroomApp`, which the closure cannot have while it borrows fields of it.
    let mut empty_pick: Option<rustroom_app::menu::Command> = None;
    // ⚠ Same shape: a star clicked on a thumbnail rates through `apply_mark_to`, which needs
    // `&mut RustroomApp` once the closure has let go of the library.
    let mut star_click: Option<(PhotoId, u8)> = None;
    // ⚠ Collected and applied after the closure, the shape every action in this file uses: the
    // closure holds `lib`, and all three of these need `&mut app`.
    let mut menu: Option<(usize, CellAction)> = None;
    let mut menu_select: Option<usize> = None;
    // ⚠ Only the cells actually drawn this frame. The grid is virtualised, so a band can reach
    // only what is on screen — which is also all it can cover, since it cannot leave the viewport.
    let mut cell_rects: Vec<(usize, egui::Rect)> = Vec::new();
    let mut band_started: Option<egui::Pos2> = None;
    // Where the grid was scrolled to, read out here so it survives the closure that borrows
    // `app.library` and can be written back after it.
    let scroll_was = app.grid_scroll;
    // ⚠ Hoisted before the closure borrows `app.library`, like `scroll_was`: reading
    // `app.settings` inside would be a second borrow of `app`.
    let auto_advance = app.settings.auto_advance;
    let mut scroll_now: Option<f32> = None;
    let mut bg_clicked = false;
    // ⚠ Collected here and applied after the panel closes: committing needs `&mut RustroomApp`
    // while the closure below still holds `&mut app.library`. Same shape `open_index` uses.
    let mut rename: Option<(PhotoId, name_edit::Rename)> = None;
    // ⚠ Collected inside the grid closure and applied after it: opening a draft writes
    // `app.label_draft`, which the closure cannot reach while it holds `&mut app.library`.
    let mut badge_clicked: Option<(PhotoId, String)> = None;
    // The stack whose expanded state a click asked to flip.
    //
    // ⚠ Collected rather than applied: `lib` is borrowed through the grid closure, and flipping it
    // must trigger a **rebuild** of `entries` rather than patch the list the closure is indexing.
    let mut stack_toggle: Option<rustroom_app::stack::StackKey> = None;
    // ⚠ Declared out here, like `open_index` and `rename`: applying it needs `&mut RustroomApp`
    // while the closure below still holds `&mut app.library`.
    // ⚠⚠ **The targets travel WITH the action, and that is the whole safety of auto-advance.**
    // The advance below moves the selection, and `apply_mark` used to read the targets out of the
    // live selection *after* the frame — so rating a run of five frames would have rated the wrong
    // photo, silently. Carrying the ids here makes "a mark without its targets" unrepresentable
    // rather than something a comment asks the next reader to remember.
    let mut mark: Option<(Vec<PhotoId>, rustroom_app::marks::MarkAction)> = None;
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
    let label_draft = &mut app.label_draft;
    // ⚠ Same reason: the context menu reads the catalog while the closure holds `&mut
    // app.library`, and the borrow checker only accepts that when the two fields are visibly
    // disjoint.
    let catalog = app.catalog.as_ref();
    // ⚠ From the settings, not a constant. Everything below is expressed as a fraction of it so
    // the thumbnail, the cell and the name all grow together — a fixed caption beside a scaling
    // thumbnail is what makes a size control look broken at the extremes.
    let cell = app.settings.clamped_cell();
    egui::CentralPanel::default().show(ui, |ui| {
        // ⚠ The grid's own gap, 12 each way (`docs/DESIGN.md` §5) — scoped to this panel, because
        // `item_spacing` is also every control row's air and 12 there would double the panel.
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
        // ⚠ Read before `lib` takes a `&mut` borrow of `app`, so the grid can ask which assets are on
        // disk without cloning the set into the draw closure.
        let immich_present = &app.immich_present;
        let Some(lib) = &mut app.library else {
            empty_pick = empty_library(ui);
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
        // ⚠ Hoisted above the keyboard block because a vertical arrow steps by a *row*, and a row
        // is `cols` wide. It was computed further down, next to the layout that also needs it —
        // one value, read twice, rather than two derivations that could disagree about the shape
        // of the grid the arrows are moving through.
        let cols = grid_columns(ui.available_width(), cell, ui.spacing().item_spacing.x);
        let mut centre_on: Option<usize> = None;
        ui.input(|i| {
            if typing {
                return;
            }
            // Steps through the *visible* list, so arrows skip a hidden format rather
            // than landing on gaps.
            let cur = lib.selection.active().unwrap_or(0);
            // ⚠⚠ **Up and down move by a whole row**, which the grid did not do at all: it
            // stepped left-to-right only, so reaching a photo four rows down meant holding `→`
            // through everything between. `grid_nav::step_by` is the same function Survey uses —
            // a grid *is* its visible list read in rows, and `cols` is the only difference.
            for (key, delta) in [
                (egui::Key::ArrowRight, 1i64),
                (egui::Key::ArrowLeft, -1),
                (egui::Key::ArrowDown, cols as i64),
                (egui::Key::ArrowUp, -(cols as i64)),
            ] {
                if i.key_pressed(key) {
                    if let Some(n) = rustroom_app::grid_nav::step_by(&lib.visible, cur, delta) {
                        lib.selection.click(n);
                        // ⚠ Only on the frame a key moved it: forcing the offset every frame
                        // would fight the wheel, dragging you back the moment you scrolled away.
                        centre_on = lib.visible.binary_search(&n).ok();
                    }
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
            //
            // ⚠ The key table lives in `interaction::culling_key`, shared with Compare and
            // Survey, so the three views cannot drift about what a digit means.
            if let Some(a) = crate::interaction::culling_key(i) {
                // ⚠⚠ **Targets first, then move.** `selected_ids` is read before the advance
                // below touches the selection; the other order rates the photo you moved *to*.
                mark = Some((lib.selected_ids(), a));
                // ⚠⚠ Auto-advance is the arrow key's own step, reached through the same two
                // lines — `step_by` then `centre_on` — so the grid cannot come to disagree with
                // itself about where "the next photo" is.
                //
                // ⚠ It happens **before** the mark is applied, which is what lets `centre_on` be
                // set on this frame rather than needing a field to carry it to the next one.
                // Safe because `apply_mark_to` refilters, and `retain_visible` only prunes what
                // the *new* marks hid — which is the run just rated, no longer selected. The
                // photo advanced to was not rated, so no filter change can reach it.
                if auto_advance && a.advances() {
                    let selected: Vec<usize> = lib.selection.iter().collect();
                    if let Some(n) = rustroom_app::grid_nav::advance_past(&lib.visible, &selected) {
                        lib.selection.click(n);
                        centre_on = lib.visible.binary_search(&n).ok();
                    }
                }
            }
        });

        // ⚠ How many rows of prefetch to keep resident beyond the drawn ones. Latency, not
        // correctness: at 0 the grid is still right, it just shows grey while you scroll. No test
        // covers this number for that reason.
        const MARGIN_ROWS: usize = 2;

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

        // The cell's geometry: a fixed 3:2 picture box and a height that is exactly the box,
        // the caption and the marks row — `cell_layout::grid_cell`'s rule.
        let layout = {
            let spacing_y = ui.spacing().item_spacing.y;
            let caption_h = ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(name_size)));
            rustroom_app::cell_layout::grid_cell(
                cell,
                caption_h,
                thumb::marks_row_height(name_size * MARK_SCALE),
                spacing_y,
            )
        };
        // ⚠ `show_rows` adds `item_spacing.y` itself, so the height passed is the bare cell —
        // and the *pitch* the centring works in is therefore the cell plus that gap.
        let pitch = layout.height + ui.spacing().item_spacing.y;
        let viewport_h = ui.available_height();
        let mut area = egui::ScrollArea::vertical()
            // ⚠ The scrollbar belongs at the window's right edge, not hugging the rightmost
            // thumbnail. With `auto_shrink` egui puts the leftover width *outside* the scroll
            // area, so the bar tracked the grid and moved whenever the column count or cell size
            // changed. `false` puts that space inside instead, pinning the bar.
            .auto_shrink([false, false]);
        // ⚠⚠ **Arithmetic, not `scroll_to_me`.** egui computes a `scroll_to_me` target offset
        // **once** and never recomputes it, which is the drift bug the filmstrip already paid
        // for: the centred photo slid away as thumbnails ahead of it loaded and reflowed the
        // content. A grid of fixed-height rows can be placed exactly instead.
        if let Some(offset) = centre_on.and_then(|pos| {
            rustroom_app::grid_nav::follow_offset(pos, cols, pitch, viewport_h, rows, scroll_was)
        }) {
            area = area.vertical_scroll_offset(offset);
        }
        // ⚠ Computed once for the whole grid, not per cell: `source` does not change mid-draw, and
        // `present` is the pre-read set rather than a stat. See `immich_catalog::shows_download_badge`.
        // ⚠⚠ **One bool per entry, decided before the closure — not a set cloned into it.** `lib` is a
        // `&mut` borrow of `app`, so the closure cannot also read `app.immich_present`; cloning the set
        // would work and would allocate a thousand short strings **every frame**, which is the
        // per-frame-work mistake this file has paid for twice. A `Vec<bool>` is one pass and one
        // kilobyte, and `shows_download_badge` stays the only place that decides.
        let immich_grid = matches!(
            lib.source,
            crate::LibrarySource::ImmichAlbum { .. } | crate::LibrarySource::ImmichBucket { .. }
        );
        let downloaded: Vec<bool> = if immich_grid {
            lib.entries
                .iter()
                .map(|e| {
                    rustroom_app::immich_catalog::shows_download_badge(
                        true,
                        &e.id.path,
                        immich_present,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let drawn = area.show_rows(ui, layout.height, rows, |ui, row_range| {
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
                        // ⚠ `caption()`, never `file_name` — the filmstrip's rule.
                        let caption = lib.entries[i].caption();
                        ui.push_id(&cell_id, |ui| {
                            // The picture gets the cell's height minus the caption and the
                            // marks row — `cell_layout::thumb_box`'s rule.
                            let box_size = egui::vec2(layout.box_w, layout.box_h);
                            let cell_size = egui::vec2(cell, layout.height);
                            let resp = ui
                                .allocate_ui(cell_size, |ui| {
                                    // Pin the cell to its full size. Without this
                                    // `allocate_ui` reports only the space the content used,
                                    // so cells varied with their thumbnail — the filmstrip's
                                    // reflow bug in its grid form.
                                    ui.set_min_size(cell_size);
                                    ui.vertical_centered(|ui| {
                                        let marks = lib
                                            .marks
                                            .get(&lib.entries[i].id)
                                            .copied()
                                            .unwrap_or_default();
                                        // ⚠ The marks go in a row under the picture, not over
                                        // it — see `thumb::marks_row`.
                                        let cell_resp = thumb::cell_with(
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
                                            lib.entries[i].badge().as_deref(),
                                            marks,
                                            thumb::CellStyle {
                                                overlay_marks: false,
                                                downloaded: downloaded
                                                    .get(i)
                                                    .copied()
                                                    .unwrap_or(false),
                                                sit_on_bottom: true,
                                            },
                                        );
                                        let e = &lib.entries[i];
                                        // ⚠⚠ Drawn **here** rather than inside `cell_with`, and for
                                        // two reasons: the count badge must be **clickable** while
                                        // `cell_with` allocates with `Sense::hover()`, and a click
                                        // in this app has to be *collected* and applied after the
                                        // closure releases `lib`. ⚠ Top-right, because the Immich
                                        // download badge already owns the top-left — and the
                                        // top-right **of the picture**, not of the cell, or a
                                        // portrait's badge floated in the empty corner beside it.
                                        if let Some(st) = e.stack {
                                            if let Some(k) = rustroom_app::stack::key_of(&e.id.path)
                                            {
                                                let full =
                                                    thumb::image_rect(ui).unwrap_or(cell_resp.rect);
                                                let pill = egui::Rect::from_min_size(
                                                    egui::pos2(
                                                        full.right() - STACK_BADGE.x - 4.0,
                                                        full.top() + 4.0,
                                                    ),
                                                    STACK_BADGE,
                                                );
                                                let r = ui.interact(
                                                    pill,
                                                    ui.id().with("stack_badge"),
                                                    egui::Sense::click(),
                                                );
                                                let v = ui.visuals();
                                                // ⚠ The thumbnail badge's colours (§7.10): white
                                                // on 55 % black at rest, the accent under the
                                                // pointer because it is a button.
                                                let hot = r.hovered();
                                                ui.painter().rect_filled(
                                                    pill,
                                                    4.0,
                                                    if hot {
                                                        v.selection.bg_fill
                                                    } else {
                                                        egui::Color32::from_black_alpha(140)
                                                    },
                                                );
                                                ui.painter().text(
                                                    pill.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    // ⚠ `count`, never "1": a lone file is
                                                    // `stack: None`, so this is always ≥ 2.
                                                    if st.expanded {
                                                        // ⚠ Not "×": on both children of an
                                                        // expanded stack it read as delete (A-11).
                                                        "▴".to_string()
                                                    } else {
                                                        st.count.to_string()
                                                    },
                                                    egui::FontId::proportional(
                                                        crate::theme::tokens::CAPTION,
                                                    ),
                                                    if hot {
                                                        crate::theme::on_accent(v.dark_mode)
                                                    } else {
                                                        egui::Color32::WHITE
                                                    },
                                                );
                                                let clicked = r.clicked();
                                                let _ = r.hover(if st.expanded {
                                                    "Collapses this stack to one photo"
                                                } else {
                                                    "Shows every file in this stack"
                                                });
                                                if clicked {
                                                    stack_toggle = Some(k);
                                                }
                                            }
                                        }
                                        // ⚠ The selected cell's name is drawn in the strong
                                        // text colour: the border says "selected" at the edge
                                        // of the eye, the name says it where you read.
                                        if lib.selection.contains(i) {
                                            let strong = ui.visuals().strong_text_color();
                                            ui.visuals_mut().override_text_color = Some(strong);
                                        }
                                        // ⚠⚠ Clicking the **badge** opens the label draft,
                                        // clicking the **caption** opens the file-rename
                                        // draft: each affordance edits the string drawn at
                                        // that spot. That is M6.0's rule, and it is what
                                        // replaced one field whose meaning depended on
                                        // whether the row happened to be a copy.
                                        if let Some(br) = thumb::badge_rect(ui) {
                                            let bresp = ui.interact(
                                                br,
                                                ui.id().with("badge_click"),
                                                egui::Sense::click(),
                                            );
                                            if bresp.clicked() {
                                                badge_clicked = Some((
                                                    e.id.clone(),
                                                    e.label.clone().unwrap_or_default(),
                                                ));
                                            }
                                            if bresp.hovered() {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                            }
                                        }
                                        // ⚠ The caption is the file's name.
                                        //
                                        // ⚠ Only the **active** row is editable. A text field
                                        // in every cell would fight the click that selects a
                                        // photo, and would put hundreds of them on screen.
                                        // ⚠⚠ One field, and which string it edits depends on
                                        // which affordance opened it — the badge or the
                                        // caption. Two fields in a cell would fight for the
                                        // same space and the same click.
                                        let editing_label =
                                            label_draft.as_ref().is_some_and(|(d, _)| *d == e.id);
                                        let r = if editing_label {
                                            name_edit::field(
                                                ui,
                                                name_edit::NameField {
                                                    id: &e.id,
                                                    file_name: &e.file_name,
                                                    label: e.label.as_deref(),
                                                    mode: name_edit::Editing::Label,
                                                },
                                                label_draft,
                                                egui::FontId::proportional(name_size),
                                                true,
                                            )
                                        } else {
                                            name_edit::caption_or_field(
                                                ui,
                                                &e.id,
                                                &e.file_name,
                                                e.label.as_deref(),
                                                rename_draft,
                                                name_size,
                                            )
                                        };
                                        if let Some(r) = r {
                                            rename = Some((e.id.clone(), r));
                                        }
                                        ui.visuals_mut().override_text_color = None;
                                        // ⚠ The marks get their **own** size, not the filename's:
                                        // they are what the grid is read for while culling, and the
                                        // caption is not. Scales with the cell exactly as
                                        // `name_size` does, then `marks_row` clamps it.
                                        thumb::marks_row(
                                            ui,
                                            box_size.x,
                                            marks,
                                            name_size * MARK_SCALE,
                                        );
                                        // ⚠ Stars under the pointer, on the thumbnail itself:
                                        // rating from the mouse where the eye already is (A-12).
                                        // Clicking the current rating clears it, like `0`.
                                        let thumb_rect = cell_resp.rect;
                                        if ui.rect_contains_pointer(thumb_rect) {
                                            let size = (name_size * MARK_SCALE).clamp(12.0, 18.0);
                                            let base = egui::pos2(
                                                thumb_rect.left() + 6.0,
                                                thumb_rect.bottom() - size * 0.75,
                                            );
                                            let star_hit = |k: u8| {
                                                egui::Rect::from_center_size(
                                                    egui::pos2(
                                                        base.x + (k as f32 - 0.5) * size,
                                                        base.y,
                                                    ),
                                                    egui::vec2(size, size),
                                                )
                                            };
                                            // ⚠ The pointer over star k previews a rating of k,
                                            // so stars 1..=k light together — a rating is a
                                            // count, not a position (reported 2026-09-28).
                                            let previewed = (1..=5u8)
                                                .find(|k| ui.rect_contains_pointer(star_hit(*k)));
                                            for k in 1..=5u8 {
                                                let hit = star_hit(k);
                                                let c = hit.center();
                                                let r = ui.interact(
                                                    hit,
                                                    ui.id().with(("star", i, k)),
                                                    egui::Sense::click(),
                                                );
                                                let lit = match previewed {
                                                    Some(h) => k <= h,
                                                    None => k <= marks.rating,
                                                };
                                                ui.painter().text(
                                                    c,
                                                    egui::Align2::CENTER_CENTER,
                                                    if lit { "★" } else { "☆" },
                                                    egui::FontId::proportional(size),
                                                    if lit {
                                                        egui::Color32::WHITE
                                                    } else {
                                                        egui::Color32::from_white_alpha(170)
                                                    },
                                                );
                                                if r.clicked() {
                                                    let stars =
                                                        if k == marks.rating { 0 } else { k };
                                                    star_click = Some((e.id.clone(), stars));
                                                }
                                            }
                                        }
                                    });
                                })
                                .response
                                // ⚠ `click_and_drag`, so a drag starting on a thumbnail can
                                // begin a rubber band. A plain click still reports
                                // `clicked()`; egui distinguishes the two.
                                .interact(egui::Sense::click_and_drag());
                            // ⚠ The name as the cell's accessible name and description, but **no
                            // tooltip**: the caption under the cell already says it (the author,
                            // 2026-09-30, after trying one). The filmstrip, which draws no
                            // caption, keeps its tooltip.
                            crate::theme::describe(&resp, &caption);
                            let lit = lib.selection.contains(i);
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    true,
                                    lit,
                                    &caption,
                                )
                            });
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
        });
        // ⚠ Remembered for the *next* frame's follow decision: `follow_offset` has to know where
        // the view is before the scroll area is built, and by then this frame's output is gone.
        scroll_now = Some(drawn.state.offset.y);
        let drawn = drawn.inner;

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

    if let Some((targets, a)) = mark {
        app.apply_mark_to(&targets, a);
    }
    // ⚠⚠ Applied by **rebuilding** the entry list, never by patching it: the grid, the filmstrip,
    // `selection` and `view_region` all index into `lib.entries`, and `entries`' own doc records
    // three sparse-index bugs on exactly that. A rebuild is measured at 4.6 ms for 830 files.
    if let Some(k) = stack_toggle {
        if let Some(lib) = app.library.as_mut() {
            if !lib.stack_expanded.remove(&k) {
                lib.stack_expanded.insert(k);
            }
        }
        app.rebuild_entries(None);
    }
    if let Some((id, text)) = badge_clicked {
        // ⚠ Opening the label draft closes any file-rename draft: two open fields in one cell
        // would both claim the same space, and the second would be invisible.
        app.rename_draft = None;
        app.label_draft = Some((id, text));
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
                        // ⚠ The accent, not a fixed blue: a band is a selection in progress.
                        let accent = ui.visuals().selection.bg_fill;
                        ui.painter().rect_stroke(
                            rect,
                            egui::CornerRadius::same(2),
                            egui::Stroke::new(1.0, accent),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().rect_filled(
                            rect,
                            egui::CornerRadius::same(2),
                            accent.gamma_multiply(0.1),
                        );
                    }
                }
            }
            if released {
                lib.band = None;
            }
        }
    }

    if let Some(y) = scroll_now {
        app.grid_scroll = y;
    }
    if let Some(i) = menu_select {
        if let Some(lib) = app.library.as_mut() {
            lib.selection.click(i);
        }
    }
    if let Some((i, action)) = menu {
        apply_cell_action(app, ctx, i, action);
    }
    if let Some(cmd) = empty_pick {
        crate::commands::execute(app, ctx, cmd);
    }
    if let Some((id, stars)) = star_click {
        app.apply_mark_to(&[id], rustroom_app::marks::MarkAction::Rating(stars));
    }
    if let Some(i) = open_index {
        app.open_entry(ctx, i);
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn grid_frame(app: &mut RustroomApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            show(app, ui, &ctx);
        });
    }

    fn grid_key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn grid_app(n: usize) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(n));
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and every rating
        // assertion below fails against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);
        app
    }

    /// ⚠⚠ **The carrier test, and both halves matter.** That the selection moved is the feature;
    /// that the photos rated are the ones that *were* selected is the bug the capture ordering
    /// exists to prevent — an implementation that advanced first would rate row 6 and leave the
    /// run untouched, and an assertion on the selection alone would pass against it.
    #[test]
    fn rating_advances_past_the_whole_selection() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[3, 4, 5]);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.selection.active(),
            Some(6),
            "it did not step past the run that was just rated"
        );
        for row in [3usize, 4, 5] {
            assert_eq!(
                lib.marks.get(&lib.entries[row].id).map(|m| m.rating),
                Some(3),
                "row {row} was selected and should have been rated"
            );
        }
        assert_eq!(
            lib.marks.get(&lib.entries[6].id).map(|m| m.rating),
            None,
            "the photo advanced TO was rated — the targets were captured after the advance"
        );
    }

    /// ⚠ A correction must not walk away from the photo you are fixing.
    #[test]
    fn a_correction_does_not_advance() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num0)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "clearing a rating moved on"
        );

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::U)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "un-flagging moved on"
        );
    }

    /// ⚠ The setting genuinely gates it — and the rating must still land, or "off" would mean
    /// "the key does nothing" rather than "the key does not move on".
    #[test]
    fn auto_advance_off_leaves_the_selection_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.settings.auto_advance = false;
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.active(), Some(4), "it advanced anyway");
        assert_eq!(
            lib.marks.get(&lib.entries[4].id).map(|m| m.rating),
            Some(3),
            "turning advancing off also stopped the rating"
        );
    }

    /// ⚠ Clamped, never wrapping: the end of the list is information, and being thrown back to
    /// the top of a folder mid-cull is disorienting.
    #[test]
    fn rating_the_last_photo_stays_there() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(5);
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "it wrapped to the top of the folder"
        );
    }

    /// ⚠ The author's call (2026-09-30, after trying it): the grid shows a photo's name in its
    /// caption, so resting on the photo shows **no** tooltip — while the name stays the cell's
    /// accessible name and description. The filmstrip's own test is the positive twin: the same
    /// rest there does show a tooltip, so this absence is not a helper that never fires.
    #[test]
    fn the_grid_names_a_photo_without_a_tooltip() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let lib = crate::test_app::library_of(5);
        let name = lib.entries[1].caption();
        app.library = Some(lib);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut body = |ui: &mut egui::Ui| {
            let c = ui.ctx().clone();
            super::show(&mut app, ui, &c);
        };
        let nodes = crate::test_ui::nodes_on(&ctx, screen, &mut body);
        // ⚠ The cell, not its caption text, which carries the same words as a Label.
        let cell = nodes
            .iter()
            .find(|n| n.label == name && n.role == egui::accesskit::Role::Button)
            .expect("the photo's cell is drawn and named");
        assert_eq!(cell.description.as_deref(), Some(name.as_str()));
        // Near the top of the cell: the picture, not the caption row under it.
        let on_picture = egui::pos2(cell.rect.center().x, cell.rect.top() + 20.0);
        assert!(
            !crate::test_ui::tooltip_after_rest(&ctx, screen, on_picture, &mut body),
            "a tooltip over a photo whose caption already names it"
        );
    }
}

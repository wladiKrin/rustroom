//! The Develop filmstrip: the loaded folder's thumbnails along the bottom.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores
//! its *content's* rendered rect as its size each frame, so content smaller than the panel
//! makes it shrink back every frame. Applies to any future resizable panel whose content does
//! not fill the resize axis.

use crate::interaction::apply_click;
use crate::{thumb, RustroomApp};
use eframe::egui;
use rustroom_app::thumb_loader::ThumbRequest;

/// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
/// The panel is resizable (drag its top border); thumbnails scale with its height.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Hoisted out of `app` before the closure borrows `app.library`: the open photo's edits
    // live in `Session` and the sidecar is stale while it is being edited, so the thumbnail must
    // use these rather than the row's stored copy.
    //
    // ⚠ **Before any `&mut app.*` binding below**: `live_edits` takes `&self`, so it borrows the
    // whole of `app` and cannot follow a mutable borrow of one of its fields.
    let live = app.live_edits();
    let mut open_index: Option<usize> = None;
    // ⚠ The same menu as the grid's, applied through the same shared function so the two cannot
    // drift — the rule `thumb::cell` and `name_edit` already follow.
    let mut menu: Option<(usize, crate::library::CellAction)> = None;
    let mut menu_select: Option<usize> = None;
    // Decided before the panel borrows `app.library`, so the closure only needs two
    // `Copy` locals. Opening from the Library grid and arrow-navigating both land here.
    let active = app.library.as_ref().and_then(|l| l.selection.active());
    let want_scroll = active.is_some() && app.filmstrip_scrolled_to != active;
    // ⚠ Borrowed out of `app` up front: the context menu reads the catalog while the closure
    // below holds `&mut app.library`, and the borrow checker only accepts that when the two
    // fields are visibly disjoint. Same shape the grid uses.
    let catalog = app.catalog.as_ref();
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
            // ⚠ Latency, not correctness — see the grid's MARGIN_ROWS.
            const MARGIN_CELLS: usize = 4;

            let step = box_size.x + ui.spacing().item_spacing.x;
            let content = visible.len() as f32 * step;
            let extent = ui.available_width();

            // ⚠ `scroll_to_me` cannot work on a virtualised strip: an off-window cell is never
            // drawn, so there is no response to scroll to. Computing the offset is also the
            // correct fix for the M2.2 drift bug — egui computes a `scroll_to_me` target once and
            // never recomputes it, so the centred photo slid left as thumbnails ahead of it
            // loaded and reflowed the content.
            let mut area = egui::ScrollArea::horizontal();
            if want_scroll {
                if let Some(pos) = active.and_then(|a| visible.iter().position(|&i| i == a)) {
                    area = area.horizontal_scroll_offset(
                        rustroom_app::thumb_window::centre_offset(pos, step, extent, content),
                    );
                }
            }

            let span = area
                .show_viewport(ui, |ui, viewport| {
                    let span = rustroom_app::thumb_window::visible_span(
                        viewport.min.x,
                        viewport.width(),
                        step,
                        visible.len(),
                        0,
                    );
                    ui.horizontal(|ui| {
                        // Reserve the space the skipped cells would have taken, so the scrollbar
                        // and the drawn cells agree about where they are.
                        ui.add_space(span.start as f32 * step);
                        for &i in &visible[span.clone()] {
                            // ⚠ A stable id per photo — see the grid's equivalent. The filmstrip
                            // has no text field, so the failure is milder, but a click resolving
                            // against a shifted id for one frame is the same hazard and
                            // `push_id` costs nothing.
                            // ⚠ Cloned rather than borrowed: the closure reads `*lib` whole (the
                            // context menu needs the selection *and* the entries), so a live
                            // borrow of `lib.entries` in the argument would conflict with it.
                            let cell_id = lib.entries[i].id.clone();
                            ui.push_id(&cell_id, |ui| {
                                // The filmstrip's cell *is* its click target, so it opts in here.
                                let resp = thumb::cell(
                                    ui,
                                    box_size,
                                    // ⚠ The exact render if it is there, else this photo's
                                    // previous one — never grey while a re-render is in flight.
                                    lib.cache
                                        .get(&lib.thumb_request(i, live.as_ref()).key())
                                        .or_else(|| lib.cache.get_stale(&lib.entries[i].id)),
                                    lib.entries[i].badge().as_deref(),
                                    lib.marks
                                        .get(&lib.entries[i].id)
                                        .copied()
                                        .unwrap_or_default(),
                                )
                                .interact(egui::Sense::click())
                                // ⚠ `label()`, never `file_name` — the one rule the Library
                                // caption, the rename field and the default export name share.
                                // The grid needs no hover: it draws the caption already.
                                .on_hover_text(lib.entries[i].caption());
                                thumb::selection_border(
                                    ui,
                                    resp.rect,
                                    lib.selection.active() == Some(i),
                                    lib.selection.contains(i),
                                );
                                if resp.clicked() {
                                    let mods = ui.input(|inp| inp.modifiers);
                                    // Only a plain click moves the active photo, so only a
                                    // plain click opens — a Cmd- or Shift-click builds the
                                    // target set without triggering a decode.
                                    if apply_click(&mut lib.selection, i, mods) {
                                        open_index = Some(i);
                                    }
                                }
                                // ⚠ A secondary click does not fire `clicked()`, so the photo is
                                // selected here explicitly.
                                resp.context_menu(|ui| {
                                    if !lib.selection.contains(i) {
                                        menu_select = Some(i);
                                    }
                                    let targets = crate::library::menu_targets(lib, i);
                                    if let Some(a) =
                                        crate::library::cell_menu(ui, catalog, &targets)
                                    {
                                        menu = Some((i, a));
                                    }
                                });
                            });
                        }
                        ui.add_space((visible.len() - span.end) as f32 * step);
                    });
                    span
                })
                .inner;

            let first = span.start.saturating_sub(MARGIN_CELLS);
            let last = span.end.saturating_add(MARGIN_CELLS).min(visible.len());
            // ⚠ One request per row, not per path: each copy renders its own edits now.
            let mut window: Vec<ThumbRequest> = Vec::new();
            for &i in &visible[first..last] {
                window.push(lib.thumb_request(i, live.as_ref()));
            }
            lib.sync_thumb_window(&window);
        });
    if want_scroll {
        app.filmstrip_scrolled_to = active;
    }
    if let Some(i) = menu_select {
        if let Some(lib) = app.library.as_mut() {
            lib.selection.click(i);
        }
    }
    if let Some((i, action)) = menu {
        crate::library::apply_cell_action(app, ctx, i, action);
    }
    if let Some(i) = open_index {
        app.open_entry(ctx, i);
    }
}

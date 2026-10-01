//! The Develop filmstrip: the loaded folder's thumbnails along the bottom — or, since M10.8b, in a
//! column on the left or the right (`settings.layout.filmstrip`).
//!
//! ⚠ One code path along either axis: the cell is sized from the strip's cross size, and the step,
//! the span, the centring and the resize rule all run along the main axis — `thumb_window` never knew
//! which way it was going.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` (or `set_min_width`, on a side) is load-bearing: a
//! resizable egui panel stores its *content's* rendered rect as its size each frame, so content
//! smaller than the panel makes it shrink back every frame. Applies to any future resizable panel
//! whose content does not fill the resize axis.

use crate::interaction::apply_click;
use crate::theme::Hover as _;
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
    let place = app.settings.layout.filmstrip;
    let vertical = place.side().is_some();
    let panel = match place.side() {
        None => egui::Panel::bottom("filmstrip")
            .resizable(true)
            .default_size(96.0),
        // ⚠ Its own id: a side strip's width and a bottom strip's height are different sizes, and
        // one remembered `PanelState` would hand each the other's.
        Some(side) => crate::side_panel::side(side, "filmstrip_side")
            .resizable(true)
            .default_size(150.0)
            .min_size(96.0),
    };
    panel.show(ui, |ui| {
        let Some(lib) = &mut app.library else { return };
        // Fill the panel across its resize axis so egui stores the panel size (not the
        // smaller content size) — otherwise a resizable panel shrinks back each frame.
        let cross = if vertical {
            let w = ui.available_width();
            ui.set_min_width(w);
            w
        } else {
            let h = ui.available_height();
            ui.set_min_height(h);
            h
        };
        // ⚠ 3:2 either way: across a bottom strip the cell is as tall as the strip, in a side
        // strip as wide as it.
        let cell = (cross - 8.0).clamp(48.0, 400.0);
        let box_size = if vertical {
            egui::vec2(cell, cell / 1.5)
        } else {
            egui::vec2(cell * 1.5, cell)
        };
        let visible = lib.visible.clone();
        // ⚠ Latency, not correctness — see the grid's MARGIN_ROWS.
        const MARGIN_CELLS: usize = 4;

        let spacing = ui.spacing().item_spacing;
        let step = if vertical {
            box_size.y + spacing.y
        } else {
            box_size.x + spacing.x
        };
        let content = visible.len() as f32 * step;
        let extent = if vertical {
            ui.available_height()
        } else {
            ui.available_width()
        };

        // ⚠ `scroll_to_me` cannot work on a virtualised strip: an off-window cell is never
        // drawn, so there is no response to scroll to. Computing the offset is also the
        // correct fix for the M2.2 drift bug — egui computes a `scroll_to_me` target once and
        // never recomputes it, so the centred photo slid left as thumbnails ahead of it
        // loaded and reflowed the content.
        //
        // ⚠⚠ **And again whenever the strip's geometry changes** (reported in use 2026-09-29:
        // "when I resize the film strip the images move around"). The offset is in points, so a
        // resize that grows every cell slid the photos sideways under a fixed offset; the
        // active one is re-centred on every frame the step or the width differs, so it stays
        // put and only grows or shrinks. `thumb_window::layout_changed`.
        let layout_id = egui::Id::new("filmstrip_layout");
        let before: Option<(f32, f32)> = ui.ctx().data(|d| d.get_temp(layout_id));
        ui.ctx()
            .data_mut(|d| d.insert_temp(layout_id, (step, extent)));
        let recentre =
            want_scroll || rustroom_app::thumb_window::layout_changed(before, (step, extent));
        let mut area = if vertical {
            egui::ScrollArea::vertical()
        } else {
            egui::ScrollArea::horizontal()
        };
        if recentre {
            if let Some(pos) = active.and_then(|a| visible.iter().position(|&i| i == a)) {
                let offset = rustroom_app::thumb_window::centre_offset(pos, step, extent, content);
                area = if vertical {
                    area.vertical_scroll_offset(offset)
                } else {
                    area.horizontal_scroll_offset(offset)
                };
            }
        }

        let span = area
            .show_viewport(ui, |ui, viewport| {
                let (start, length) = if vertical {
                    (viewport.min.y, viewport.height())
                } else {
                    (viewport.min.x, viewport.width())
                };
                let span =
                    rustroom_app::thumb_window::visible_span(start, length, step, visible.len(), 0);
                // ⚠ Centred across the strip, as `ui.horizontal` always did.
                let direction = if vertical {
                    egui::Layout::top_down(egui::Align::Center)
                } else {
                    egui::Layout::left_to_right(egui::Align::Center)
                };
                ui.with_layout(direction, |ui| {
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
                        // ⚠ `caption()`, never `file_name` — the one rule the Library
                        // caption, the rename field and the default export name share.
                        let caption = lib.entries[i].caption();
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
                            // The name on hover: the strip draws no caption. (The grid's cells
                            // carry it only as their description — their caption says it.)
                            .hover(&caption);
                            // The caption as the cell's accessible name too — a screen reader
                            // and a test find the photo by it.
                            let active_here = lib.selection.active() == Some(i);
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    true,
                                    active_here,
                                    &caption,
                                )
                            });
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
                                    crate::library::cell_menu(ui, catalog, &lib.source, &targets)
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

#[cfg(test)]
mod tests {
    use crate::RustroomApp;
    use eframe::egui;

    /// ⚠⚠ **Reported in use 2026-09-29:** "when I resize the film strip, the images move around,
    /// but it should always be centered around the image that's currently selected, so it doesn't
    /// move, just gets resized." The strip is resized by storing the panel's size, as a drag of its
    /// top edge does. ⚠ Sample first: the two heights must really give two cell sizes, or two equal
    /// positions would prove nothing.
    #[test]
    fn resizing_the_strip_keeps_the_selected_photo_where_it_was() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let mut lib = crate::test_app::library_of(40);
        lib.selection.click(20);
        let name = lib.entries[20].caption();
        app.library = Some(lib);
        let (w, h) = (1200.0, 800.0);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h));
        let at = |app: &mut RustroomApp, height: f32| -> egui::Rect {
            ctx.data_mut(|d| {
                d.insert_persisted(
                    egui::Id::new("filmstrip"),
                    egui::containers::panel::PanelState {
                        outer_rect: egui::Rect::from_min_max(
                            egui::pos2(0.0, h - height),
                            egui::pos2(w, h),
                        ),
                    },
                )
            });
            let mut found = Vec::new();
            // Two frames: a scroll offset set on one is laid out on the next.
            for _ in 0..2 {
                found = crate::test_ui::labelled_rects_on(&ctx, screen, |ui| {
                    let c = ui.ctx().clone();
                    super::show(app, ui, &c);
                });
            }
            found
                .into_iter()
                .find(|(l, _)| *l == name)
                .map(|(_, r)| r)
                .unwrap_or_else(|| panic!("the selected photo was not drawn at {height}"))
        };
        let small = at(&mut app, 96.0);
        let large = at(&mut app, 200.0);
        assert!(
            large.width() > small.width() * 1.5,
            "sample: the cells did not grow: {small:?} then {large:?}"
        );
        assert!(
            (large.center().x - small.center().x).abs() < 1.0,
            "the selected photo moved when the strip was resized: {} then {}",
            small.center().x,
            large.center().x
        );
        let back = at(&mut app, 120.0);
        assert!(
            (back.center().x - small.center().x).abs() < 1.0,
            "and when it shrank again: {}",
            back.center().x
        );
    }

    /// ⚠ The author's ask (2026-09-30): resting on a photo shows its name. Checked as SHOWN — a
    /// tooltip up after the pointer rests — and as the cell's description; the control, a rest on
    /// nothing, shows none, so the helper cannot pass by always saying yes.
    #[test]
    fn resting_on_a_photo_shows_its_name() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let lib = crate::test_app::library_of(5);
        let name = lib.entries[2].caption();
        app.library = Some(lib);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut body = |ui: &mut egui::Ui| {
            let c = ui.ctx().clone();
            super::show(&mut app, ui, &c);
        };
        let nodes = crate::test_ui::nodes_on(&ctx, screen, &mut body);
        let cell = nodes
            .iter()
            .find(|n| n.label == name)
            .expect("the photo is drawn and named");
        assert_eq!(cell.description.as_deref(), Some(name.as_str()));
        assert!(
            crate::test_ui::tooltip_after_rest(&ctx, screen, cell.rect.center(), &mut body),
            "no tooltip over the photo"
        );
        let ctx = crate::test_ui::ctx();
        assert!(
            !crate::test_ui::tooltip_after_rest(&ctx, screen, egui::pos2(5.0, 5.0), &mut body),
            "a tooltip over nothing"
        );
    }

    /// A strip of `n` photos on the left, the 21st selected.
    fn side_strip(n: usize) -> (RustroomApp, String) {
        let mut app = crate::test_app::for_test();
        app.settings.layout.filmstrip = rustroom_app::layout::StripPlace::Left;
        let mut lib = crate::test_app::library_of(n);
        lib.selection.click(20);
        let name = lib.entries[20].caption();
        app.library = Some(lib);
        (app, name)
    }

    fn drawn(ctx: &egui::Context, app: &mut RustroomApp) -> Vec<(String, egui::Rect)> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut found = Vec::new();
        for _ in 0..2 {
            found = crate::test_ui::labelled_rects_on(ctx, screen, |ui| {
                let c = ui.ctx().clone();
                super::show(app, ui, &c);
            });
        }
        found
    }

    /// M10.8b: on a side the strip is a column — cells stacked downwards in the left strip, only the
    /// window around the selection drawn, and a click selects.
    #[test]
    fn a_side_strip_is_a_column_that_draws_only_its_window() {
        let ctx = crate::test_ui::ctx();
        let (mut app, name) = side_strip(200);
        let mut cells: Vec<(String, egui::Rect)> = drawn(&ctx, &mut app)
            .into_iter()
            .filter(|(l, _)| l.starts_with("rustroom_"))
            .collect();
        // ⚠ AccessKit's node order is not the screen's.
        cells.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
        assert!(cells.len() > 3, "the sample: {} cells", cells.len());
        assert!(
            cells.len() < 40,
            "the strip drew {} of 200 cells",
            cells.len()
        );
        let r = cells
            .iter()
            .find(|(l, _)| *l == name)
            .expect("the selected one")
            .1;
        assert!(r.max.x < 300.0, "on the left: {r:?}");
        let (a, b) = (cells[0].1, cells[1].1);
        assert!(
            (a.center().x - b.center().x).abs() < 1.0 && b.min.y > a.min.y,
            "a column: {a:?} then {b:?}"
        );
        let other = cells[0].0.clone();
        crate::test_ui::click(&ctx, &other, |ui| {
            let c = ui.ctx().clone();
            super::show(&mut app, ui, &c);
        });
        let lib = app.library.as_ref().unwrap();
        let active = lib.selection.active().map(|i| lib.entries[i].caption());
        assert_eq!(active.as_deref(), Some(other.as_str()));
    }

    /// The bottom strip's resize rule, on a side: widening the column grows the cells and leaves the
    /// selected photo where it was.
    #[test]
    fn resizing_the_side_strip_keeps_the_selected_photo_where_it_was() {
        let ctx = crate::test_ui::ctx();
        let (mut app, name) = side_strip(40);
        let at = |app: &mut RustroomApp, width: f32| -> egui::Rect {
            ctx.data_mut(|d| {
                d.insert_persisted(
                    egui::Id::new("filmstrip_side"),
                    egui::containers::panel::PanelState {
                        outer_rect: egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(width, 800.0),
                        ),
                    },
                )
            });
            drawn(&ctx, app)
                .into_iter()
                .find(|(l, _)| *l == name)
                .map(|(_, r)| r)
                .unwrap_or_else(|| panic!("the selected photo was not drawn at {width}"))
        };
        let small = at(&mut app, 120.0);
        let large = at(&mut app, 240.0);
        assert!(
            large.height() > small.height() * 1.5,
            "sample: the cells did not grow: {small:?} then {large:?}"
        );
        assert!(
            (large.center().y - small.center().y).abs() < 1.0,
            "the selected photo moved when the strip was resized: {} then {}",
            small.center().y,
            large.center().y
        );
    }
}

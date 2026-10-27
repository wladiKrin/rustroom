//! Survey (N) — the whole selection at once, tiled to fill the view.
//!
//! ⚠⚠ **The selection is the *content* of this view, not a target set — and every rule here
//! follows from that.** Clicking a tile moves the focus with [`Selection::focus`] rather than
//! `click`, which would collapse the survey to one photo; the culling keys act on the focused
//! tile alone rather than on the whole selection, which would rate every frame you were
//! choosing between; and `×` on a tile removes it from the selection, which is how a survey is
//! narrowed down to the keeper.
//!
//! ⚠⚠ **A scrolling, virtualised grid, and it did not start that way.** It used to fill one
//! screenful, capped at 24 tiles, and say "showing 24 of 39" — which named the problem and gave
//! no way out: the remaining photographs were simply unreachable. Virtualised, the cap is gone
//! *and* the memory bound it existed for is stronger, because `retain_window` now pins only what
//! is actually on screen. The Library grid solved the identical problem the identical way.

use crate::multi;
use crate::theme::Hover as _;
use crate::{LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::preview_key::is_untouched;
use rustroom_app::preview_loader::PreviewRequest;
use rustroom_app::selection::Selection;
use rustroom_app::thumb_loader::ThumbRequest;
use rustroom_app::tile_layout;

/// A cell whose image area exceeds this on either side draws the preview rather than the
/// 512-px thumbnail (M9.14).
pub(crate) const PREVIEW_CELL_PX: f32 = 512.0;

/// What one frame of Survey decided, applied after the closure that borrows `app.library`.
#[derive(Default)]
struct Outcome {
    open: Option<usize>,
    mark: Option<(PhotoId, rustroom_app::marks::MarkAction)>,
    /// Make this row the focused tile, keeping the rest of the survey.
    focus: Option<usize>,
    /// Drop this row from the survey.
    drop: Option<usize>,
    /// Keep this *position in the survey* inside the follow band.
    centre_on: Option<usize>,
    /// Where the view ended up, remembered for the next frame's follow decision.
    scroll: Option<f32>,
    to_grid: bool,
}

/// The rows a survey shows: the selection, in `visible` order, capped.
///
/// ⚠⚠ **In `visible` order, and intersected with it.** `Selection` is a `BTreeSet` of entry
/// indices and `visible` is the sparse derived list — iterating the set directly would show a
/// photo the filter says is not there, and would order the survey by index rather than by what
/// the grid shows. `retain_visible` should have pruned it already, so this is defence in depth
/// against the mistake this project has made three times.
fn surveyed(visible: &[usize], selection: &Selection) -> Vec<usize> {
    visible
        .iter()
        .copied()
        .filter(|&i| selection.contains(i))
        .collect()
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Hoisted before any `&mut app.*` — `live_edits` borrows the whole of `app`.
    let live = app.live_edits();
    // Where the survey was scrolled to last frame: `follow_offset` needs it *before* the scroll
    // area is built, by which time this frame's own output does not exist yet.
    let scroll_was = app.survey_scroll;
    let mut out = Outcome::default();

    // ⚠⚠ **Pinch, or ctrl/cmd+scroll, resizes the tiles — the same gesture the Library grid
    // takes**, because a survey of three photos on a large monitor should be able to fill it.
    // `zoom_delta` folds trackpad pinch and ctrl+scroll into one **multiplicative factor that is
    // 1.0 when idle**, which is what leaves plain scrolling free to scroll.
    // ⚠ Not while a dialog is open — the grid's rule (reported 2026-10-27, the HDR preview's zoom).
    let zoom = if matches!(app.modal, crate::modals::Modal::None) {
        ui.ctx().input(|i| i.zoom_delta())
    } else {
        1.0
    };
    if zoom != 1.0 {
        app.settings.survey_cell = app.settings.zoomed_survey_cell(zoom);
        app.cell_zoom_pending = true;
    } else if app.cell_zoom_pending {
        // ⚠ Saved when the gesture *settles*, not per frame — a pinch runs at 60 Hz.
        app.cell_zoom_pending = false;
        app.save_settings();
    }
    let cell = app.settings.clamped_survey_cell();
    // M9.14: a cell larger than a thumbnail draws the preview at fit. Shaped here, where the
    // settings are reachable; the closure below holds the library.
    let fresh = app.settings.initial_edits(None);
    let tier = app.settings.preview_tier();
    let previews = &app.preview_textures;
    let mut preview_reqs: Vec<PreviewRequest> = Vec::new();
    let library = &mut app.library;

    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = library else {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Open a folder to survey photos.");
            });
            return;
        };
        let rows_list = surveyed(&lib.visible, &lib.selection);
        if rows_list.is_empty() {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(
                    ui,
                    "Select photos in the grid, then press N to survey them.",
                );
            });
            return;
        }

        // ── Keyboard ─────────────────────────────────────────────────────────────────────
        //
        // ⚠ The `memory().focused()` gate the Library's own keys use — the search field is on
        // screen here too.
        let focused = lib.selection.active();
        let typing = ui.ctx().memory(|m| m.focused().is_some());

        // ── Layout ───────────────────────────────────────────────────────────────────────
        let spacing = ui.spacing().item_spacing.x;
        let cols = grid_columns(ui.available_width(), cell, spacing);
        // ⚠ Aspect-shaped, not square: a survey is looked at rather than scanned, and a square
        // cell gives a 3:2 photograph two grey bars for nothing. The Library grid is square
        // because it is scanned.
        let tile_h = cell / tile_layout::NOMINAL_ASPECT + multi::CAPTION_H;

        ui.input(|i| {
            if typing {
                return;
            }
            // ⚠⚠ **Up and down move by a whole row, left and right by one.** With the survey a
            // scrolling grid rather than a single screenful, stepping only left-to-right makes a
            // long selection unreachable by keyboard — you would hold `→` through forty photos.
            let at = focused.and_then(|f| rows_list.iter().position(|&r| r == f));
            for (key, delta) in [
                (egui::Key::ArrowRight, 1i64),
                (egui::Key::ArrowLeft, -1),
                (egui::Key::ArrowDown, cols as i64),
                (egui::Key::ArrowUp, -(cols as i64)),
            ] {
                if i.key_pressed(key) {
                    let next = at.map_or(0, |p| {
                        (p as i64 + delta).clamp(0, rows_list.len() as i64 - 1) as usize
                    });
                    out.focus = Some(rows_list[next]);
                    // ⚠⚠ **Scroll only on the frame a key moved the selection.** Setting the
                    // offset every frame would fight the wheel: you would scroll away and be
                    // dragged straight back.
                    out.centre_on = Some(next);
                }
            }
            if i.key_pressed(egui::Key::Enter) {
                out.open = focused;
            }
            if i.key_pressed(egui::Key::Escape) {
                out.to_grid = true;
            }
            // ⚠ The focused tile alone — see the module doc.
            if let (Some(a), Some(f)) = (crate::interaction::culling_key(i), focused) {
                if let Some(e) = lib.entries.get(f) {
                    out.mark = Some((e.id.clone(), a));
                }
            }
            // ⚠⚠ **Rejecting takes the photo off the wall, and that is what Survey is for.**
            // Without it the view is a smaller Library grid. Culling is a subtractive act: the
            // wall shrinks as you reject, and what is left when you stop is the keeper. Nothing
            // is deleted; the photo is still in the folder, still rejected, and `X` again in the
            // grid un-rejects it.
            if let (Some(a), Some(f)) = (crate::interaction::culling_key(i), focused) {
                // ⚠⚠ **A flag is a decision; a star is not.** `P` and `X` both take the photo off
                // the wall, because once you have said keep-this or lose-this there is nothing
                // left to ask about it and leaving it up means judging it again on every pass.
                // A **rating** must not drop: a star is a judgement you go on to compare against
                // the others, and a survey that emptied itself as you rated would destroy the
                // very set you were choosing within.
                if matches!(a, rustroom_app::marks::MarkAction::Flag(f2)
                    if f2 == catalog::db::Flag::Pick || f2 == catalog::db::Flag::Reject)
                {
                    out.drop = Some(f);
                }
            }
        });

        // ── Draw ─────────────────────────────────────────────────────────────────────────

        let grid_rows = rows_list.len().div_ceil(cols);
        let mut window: Vec<ThumbRequest> = Vec::new();
        // ⚠ `show_rows` adds `item_spacing.y` between rows itself, so the pitch is the tile plus
        // that gap — using the bare tile height drifts further from the truth with every row and
        // centres the wrong photo near the bottom of a long grid.
        let pitch = tile_h + ui.spacing().item_spacing.y;
        let viewport_h = ui.available_height();
        // ⚠⚠ **A scrolling, virtualised grid rather than one screenful.** It used to cap at
        // `MAX_TILES` and say "showing 24 of 39" — which named the problem without offering a
        // way out, and the remaining photos were simply unreachable. Virtualised, the cap is
        // gone *and* the memory bound it existed for is stronger: `retain_window` now pins only
        // what is actually on screen, exactly as the Library grid does.
        let mut area = egui::ScrollArea::vertical()
            // ⚠ Pins the scrollbar to the panel edge rather than letting it track the widest
            // row — the fix the Library grid needed for the same reason.
            .auto_shrink([false, false]);
        if let Some(offset) = out.centre_on.and_then(|pos| {
            rustroom_app::grid_nav::follow_offset(
                pos, cols, pitch, viewport_h, grid_rows, scroll_was,
            )
        }) {
            area = area.vertical_scroll_offset(offset);
        }
        let drawn = area.show_rows(ui, tile_h, grid_rows, |ui, row_range| {
            for grid_row in row_range {
                ui.horizontal(|ui| {
                    let first = grid_row * cols;
                    let last = (first + cols).min(rows_list.len());
                    for &row in &rows_list[first..last] {
                        let (_, rect) = ui.allocate_space(egui::vec2(cell, tile_h));
                        let req = lib.thumb_request(row, live.as_ref());
                        let tex = lib
                            .cache
                            .get(&req.key())
                            // ⚠ This photo's previous render rather than grey while a
                            // re-render lands.
                            .or_else(|| lib.cache.get_stale(&lib.entries[row].id));
                        let marks = lib
                            .marks
                            .get(&lib.entries[row].id)
                            .copied()
                            .unwrap_or_default();
                        // ⚠⚠ A stable id per photo, not egui's auto-id counter: under
                        // `show_rows` that counter shifts with the scroll position, which
                        // silently detaches every widget's state from its photo. The
                        // Library grid records the same trap.
                        let base = ui.id().with(&lib.entries[row].id);
                        // ⚠ Only a cell the thumbnail cannot fill asks for a preview, so a wall
                        // of two dozen small cells costs nothing new.
                        let img_area = multi::image_area(rect);
                        let preview = if img_area.width().max(img_area.height()) > PREVIEW_CELL_PX {
                            let preq = PreviewRequest {
                                untouched: is_untouched(&req.edits, &fresh),
                                tier,
                                network: app.settings.denoise_network,
                                id: req.id.clone(),
                                edits: req.edits.clone(),
                            };
                            let found = previews.get(&preq.key()).map(|(t, _)| t);
                            preview_reqs.push(preq);
                            found
                        } else {
                            None
                        };
                        let image = match preview {
                            Some(t) => multi::TileImage::Pane {
                                tex: t,
                                scale: (img_area.width() / t.size_vec2().x.max(1.0))
                                    .min(img_area.height() / t.size_vec2().y.max(1.0)),
                                offset: egui::Vec2::ZERO,
                            },
                            None => multi::TileImage::Thumb(tex),
                        };
                        let resp = multi::tile(
                            ui,
                            rect,
                            base.with("tile"),
                            image,
                            &lib.entries[row].caption(),
                            marks,
                            focused == Some(row),
                        );
                        if resp.double_clicked() {
                            out.open = Some(row);
                        } else if resp.clicked() {
                            out.focus = Some(row);
                        }
                        match tile_buttons(ui, rect, base, focused == Some(row)) {
                            Some(TileAction::Pick) => {
                                if let Some(e) = lib.entries.get(row) {
                                    out.mark = Some((
                                        e.id.clone(),
                                        rustroom_app::marks::MarkAction::Flag(
                                            catalog::db::Flag::Pick,
                                        ),
                                    ));
                                }
                            }
                            Some(TileAction::Remove) => out.drop = Some(row),
                            None => {}
                        }
                        window.push(req);
                    }
                });
            }
        });
        // ⚠ Remembered for the *next* frame's follow decision — see `RustroomApp::grid_scroll`.
        out.scroll = Some(drawn.state.offset.y);
        // ⚠⚠ Everything drawn, or `retain_window` leaves these photos unpinned and evictable
        // while they are the only thing on screen.
        lib.sync_thumb_window(&window);
    });
    app.sync_preview_window(&preview_reqs);

    if let Some(y) = out.scroll {
        app.survey_scroll = y;
    }
    if let Some(row) = out.focus {
        if let Some(lib) = app.library.as_mut() {
            // ⚠ `focus`, never `click`: `click` replaces the selection, which *is* this view's
            // contents — clicking a tile would leave a survey of one.
            lib.selection.focus(row);
        }
    }
    if let Some((id, action)) = out.mark {
        app.apply_mark_to(&[id], action);
    }
    // ⚠ **After the mark, never before.** `drop_from_survey` moves the focus off the photo, and
    // the mark is addressed to the photo that *was* focused — reversed, a reject would land on
    // whichever tile the focus had just moved to.
    if let Some(row) = out.drop {
        drop_from_survey(app, row);
    }
    if out.to_grid {
        app.library_mode = LibraryMode::Grid;
    }
    // ⚠ `open_entry` switches to Develop itself.
    if let Some(row) = out.open {
        app.open_entry(ctx, row);
    }
}

/// What a tile's own buttons asked for.
enum TileAction {
    /// Flag it a pick — the keeper this survey is narrowing towards.
    Pick,
    /// Take it off the wall without judging it.
    Remove,
}

/// The two buttons in a tile's top-right corner.
///
/// ⚠⚠ **Registered AFTER the tile, so they win the hit test**: egui breaks a tie by registration
/// order — "in case of a tie, take the last one = the one on top" — and registered first the tile
/// would swallow every click meant for them.
///
/// ⚠ Shown on hover or on the focused tile, not always: two glyphs on every tile of a
/// forty-photo wall is more chrome than photograph.
///
/// ⚠ The glyphs are `⚑` and `×`, the two the mark badges already use — probed and known to
/// render. `✓`, `✔`, `✕` and `✗` are in **no** bundled font.
fn tile_buttons(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    base: egui::Id,
    focused: bool,
) -> Option<TileAction> {
    const SIZE: f32 = 20.0;
    let mut out = None;
    for (i, (glyph, action, colour, hint)) in [
        (
            "⚑",
            TileAction::Pick,
            egui::Color32::from_rgb(120, 200, 130),
            "Flags this photo as a pick",
        ),
        (
            "×",
            TileAction::Remove,
            egui::Color32::from_rgb(230, 110, 110),
            "Takes this photo out of the survey without rejecting it",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let at = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - SIZE * (2.0 - i as f32) - 4.0, rect.min.y + 4.0),
            egui::vec2(SIZE, SIZE),
        );
        if !rect.contains_rect(at) {
            continue;
        }
        let r = ui
            .interact(at, base.with(("tilebtn", i)), egui::Sense::click())
            .hover(hint);
        if r.hovered() || focused {
            ui.painter().text(
                at.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(crate::theme::tokens::ICON),
                colour,
            );
        }
        if r.clicked() {
            out = Some(action);
        }
    }
    out
}

/// How many tiles fit across `avail`, counting the gaps between them.
///
/// ⚠ `n` cells need `n*cell + (n-1)*spacing`, not `n*cell`. The Library grid shipped without the
/// gap term and overflowed on wide windows, where the gap total grows with the column count while
/// the leftover width stays uniform — which is why it showed on a large monitor and not a small
/// one.
fn grid_columns(avail: f32, cell: f32, spacing: f32) -> usize {
    if !avail.is_finite() || avail <= 0.0 || cell <= 0.0 {
        return 1;
    }
    (((avail + spacing) / (cell + spacing)).floor() as usize).max(1)
}

/// Take one photo out of the survey.
///
/// ⚠⚠ `Selection::toggle` refuses to deselect the **active** photo, which is what keeps its
/// invariant — so removing the focused tile has to move the focus first. Without that the `×`
/// on the focused tile silently does nothing, which reads as the button being broken.
fn drop_from_survey(app: &mut RustroomApp, row: usize) {
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    if lib.selection.active() == Some(row) {
        let rows = surveyed(&lib.visible, &lib.selection);
        // The next survivor in survey order, else the previous one.
        let next = rows
            .iter()
            .position(|&r| r == row)
            .and_then(|p| {
                rows.get(p + 1)
                    .or_else(|| p.checked_sub(1).and_then(|q| rows.get(q)))
            })
            .copied();
        match next {
            Some(n) => {
                lib.selection.focus(n);
            }
            // ⚠ The last photo in the survey: dropping it would leave a selection with no
            // active photo, which `Selection`'s invariant forbids. Leave it and go back to the
            // grid — an empty survey is not a state worth being in.
            None => {
                app.library_mode = LibraryMode::Grid;
                return;
            }
        }
    }
    lib.selection.toggle(row);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppView;

    fn frame(app: &mut RustroomApp, ctx: &egui::Context, events: Vec<egui::Event>) {
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

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn app_with(n: usize, selected: &[usize]) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = AppView::Library;
        app.library_mode = LibraryMode::Survey;
        let mut lib = crate::test_app::library_of(n);
        lib.selection.select_indices(selected);
        app.library = Some(lib);
        app
    }

    /// M9.14: only a cell the thumbnail cannot fill asks the preview loader for anything.
    #[test]
    fn a_large_survey_cell_asks_for_a_preview_and_a_small_one_does_not() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(2, &[0, 1]);
        app.settings.survey_cell = rustroom_app::settings::SURVEY_CELL_RANGE.0;
        frame(&mut app, &ctx, Vec::new());
        assert!(
            app.preview_want.is_empty(),
            "a {}-pt cell asked for a preview",
            rustroom_app::settings::SURVEY_CELL_RANGE.0
        );
        app.settings.survey_cell = rustroom_app::settings::SURVEY_CELL_RANGE.1;
        frame(&mut app, &ctx, Vec::new());
        assert!(
            !app.preview_want.is_empty(),
            "a {}-pt cell did not ask for a preview",
            rustroom_app::settings::SURVEY_CELL_RANGE.1
        );
    }

    /// ⚠⚠ **The act that makes Survey a culling view rather than a smaller grid.** Rejecting a
    /// photo takes it off the wall, so the survey shrinks toward the keeper as you work.
    #[test]
    fn rejecting_a_photo_takes_it_off_the_wall() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(50, &[2, 5, 9]);
        frame(&mut app, &ctx, Vec::new());
        {
            let lib = app.library.as_mut().unwrap();
            lib.selection.focus(5);
        }
        frame(&mut app, &ctx, vec![key(egui::Key::X)]);
        let lib = app.library.as_ref().unwrap();
        assert!(
            !lib.selection.contains(5),
            "the rejected photo is still on the wall"
        );
        assert!(
            lib.selection.contains(2) && lib.selection.contains(9),
            "rejecting one photo removed the others"
        );
    }

    /// ⚠⚠ **A pick is a decision too, so it also leaves the wall** (user's call, 2026-09-02).
    /// Once you have said keep-this there is nothing left to ask about it, and leaving it up
    /// means judging it again on every pass.
    #[test]
    fn picking_a_photo_also_takes_it_off_the_wall() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(50, &[2, 5, 9]);
        frame(&mut app, &ctx, Vec::new());
        {
            let lib = app.library.as_mut().unwrap();
            lib.selection.focus(5);
        }
        frame(&mut app, &ctx, vec![key(egui::Key::P)]);
        let lib = app.library.as_ref().unwrap();
        assert!(
            !lib.selection.contains(5),
            "the picked photo is still on the wall"
        );
        assert!(
            lib.selection.contains(2) && lib.selection.contains(9),
            "picking one photo removed the others"
        );
    }

    /// ⚠ The other half, and the one a "does X drop it?" test cannot see: **rating must not
    /// drop.** A star is a judgement you go on to compare against the others, so a survey that
    /// emptied itself as you rated would destroy the very set you were choosing within.
    #[test]
    fn rating_a_photo_leaves_it_on_the_wall() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(50, &[2, 5, 9]);
        frame(&mut app, &ctx, Vec::new());
        {
            let lib = app.library.as_mut().unwrap();
            lib.selection.focus(5);
        }
        // ⚠ `P` is **not** in this list any more: it became a decision alongside `X` and now
        // takes the photo off the wall. `U` stays, because un-flagging *undoes* a decision — a
        // photo you have just made undecided again belongs back on the wall.
        for k in [egui::Key::Num3, egui::Key::Num0, egui::Key::U] {
            frame(&mut app, &ctx, vec![key(k)]);
            let lib = app.library.as_ref().unwrap();
            assert!(
                lib.selection.contains(5),
                "{k:?} took the photo off the wall"
            );
        }
    }

    #[test]
    fn survey_asks_for_exactly_the_photos_it_is_showing() {
        // ⚠⚠ The pinning constraint, asserted where it is observable. A view drawing from the
        // cache without a want-list shows grey tiles *and* lets its own photos be evicted.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(50, &[2, 5, 9, 30]);

        frame(&mut app, &ctx, Vec::new());

        let lib = app.library.as_ref().unwrap();
        let asked: Vec<usize> = lib
            .last_want
            .iter()
            .filter_map(|r| lib.entries.iter().position(|e| e.id == r.id))
            .collect();
        assert_eq!(asked, vec![2, 5, 9, 30]);
    }

    /// ⚠⚠ **Up and down move by a whole row.** With the survey a scrolling grid rather than one
    /// screenful, left-to-right alone makes a long selection unreachable by keyboard — you would
    /// hold `→` through forty photographs.
    #[test]
    fn up_and_down_move_by_a_row() {
        let ctx = crate::test_ui::ctx();
        let sel: Vec<usize> = (0..12).collect();
        let mut app = app_with(40, &sel);
        // ⚠ The harness screen is 400x300 and the default tile is 320 wide, which gives **one**
        // column — where a row and a step are the same thing and this test could not see the
        // difference it names. The smallest tile fits two.
        app.settings.survey_cell = rustroom_app::settings::SURVEY_CELL_RANGE.0;
        frame(&mut app, &ctx, Vec::new());
        app.library.as_mut().unwrap().selection.focus(0);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowDown)]);
        let after_down = app.library.as_ref().unwrap().selection.active().unwrap();
        assert!(
            after_down > 1,
            "down moved by one photo ({after_down}) rather than by a row"
        );

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowUp)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(0),
            "up did not undo down"
        );
    }

    /// ⚠ Clamped, not wrapping — the rule the arrows already followed left-to-right. Wrapping
    /// from the last row to the first is disorienting in a grid you are scrolling.
    #[test]
    fn the_arrows_clamp_at_the_ends() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(40, &[2, 5, 9]);
        app.settings.survey_cell = rustroom_app::settings::SURVEY_CELL_RANGE.0;
        frame(&mut app, &ctx, Vec::new());
        app.library.as_mut().unwrap().selection.focus(2);
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowUp)]);
        assert_eq!(app.library.as_ref().unwrap().selection.active(), Some(2));
        app.library.as_mut().unwrap().selection.focus(9);
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(app.library.as_ref().unwrap().selection.active(), Some(9));
    }

    /// ⚠ Pinch resizes the tiles and **persists**, so a survey opened again is the size you left
    /// it. `zoom_delta` is a factor that is 1.0 when idle, not a delta — treating it as a delta
    /// collapses the size to nothing on every idle frame, which this project has paid for once
    /// already in the canvas navigation work.
    #[test]
    fn pinching_resizes_the_tiles() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4]);
        let before = app.settings.survey_cell;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                events: vec![egui::Event::Zoom(1.5)],
                ..Default::default()
            },
            |ui| {
                let c = ui.ctx().clone();
                show(&mut app, ui, &c);
            },
        );
        assert!(
            app.settings.survey_cell > before,
            "a pinch left the tile size at {before}"
        );
    }

    /// A pinch meant for a dialog — the HDR merge preview zooms with it — leaves the tiles alone
    /// (reported 2026-10-27: zooming the preview resized the Library behind it).
    #[test]
    fn a_pinch_with_a_dialog_open_leaves_the_tiles_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4]);
        app.modal = crate::modals::Modal::HdrMerge;
        let before = app.settings.survey_cell;
        frame(&mut app, &ctx, vec![egui::Event::Zoom(1.5)]);
        assert_eq!(app.settings.survey_cell, before);
    }

    /// ⚠ And an idle frame must not move it: the guard against reading `zoom_delta` as a delta.
    #[test]
    fn an_idle_frame_leaves_the_tile_size_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4]);
        let before = app.settings.survey_cell;
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(app.settings.survey_cell, before);
    }

    #[test]
    fn a_huge_selection_asks_only_for_what_is_on_screen() {
        // ⚠⚠ **This test used to assert the opposite** — that a huge selection was *capped* at
        // 24 tiles — and the cap was the bug: "showing 24 of 39" named the problem and offered no
        // way to reach the rest. Virtualising is the stronger answer to the memory bound the cap
        // existed for: `retain_window` pins everything in the window and never evicts it, so what
        // matters is that the window stays bounded by the viewport, not by a constant.
        //
        // ⚠ The assertion is on the **want-list**, not on the tile count: a view that drew fewer
        // tiles while still asking for all 300 would pass a "how many tiles?" check and blow the
        // cache anyway.
        let ctx = crate::test_ui::ctx();
        let all: Vec<usize> = (0..300).collect();
        let mut app = app_with(300, &all);

        frame(&mut app, &ctx, Vec::new());

        let asked = app.library.as_ref().unwrap().last_want.len();
        assert!(asked > 0, "the survey asked for nothing at all");
        assert!(
            asked < 300,
            "the survey asked for the whole selection ({asked}) rather than a screenful"
        );
    }

    /// ⚠ And the other half: every photo in the selection is *reachable*, which is what the cap
    /// took away. `surveyed` is what the grid is laid out from, so an unreachable photo is one
    /// missing here.
    #[test]
    fn every_selected_photo_is_in_the_survey() {
        let mut app = app_with(300, &(0..300).collect::<Vec<_>>());
        let lib = app.library.as_mut().unwrap();
        assert_eq!(surveyed(&lib.visible, &lib.selection).len(), 300);
    }

    #[test]
    fn clicking_a_tile_moves_the_focus_without_collapsing_the_survey() {
        // ⚠⚠ The reason this view calls `focus` rather than `click`. `click` replaces the whole
        // selection — which here *is* the survey — so one click would leave a survey of one.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4, 7]);
        // Lay out once to learn where the tiles are, then click the second one's centre.
        frame(&mut app, &ctx, Vec::new());
        let stage = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::test_ui::screen().size());
        let t = tile_layout::survey(3, stage.width(), stage.height(), 10.0);
        let centre = multi::tile_rect(stage, t[1]).center();

        // Three passes: move to the point, press while still there, release.
        for events in [
            vec![egui::Event::PointerMoved(centre)],
            vec![egui::Event::PointerButton {
                pos: centre,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerButton {
                pos: centre,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            frame(&mut app, &ctx, events);
        }

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.len(), 3, "the click collapsed the survey");
    }

    #[test]
    fn an_arrow_moves_the_focus_through_the_survey_only() {
        // ⚠ Through the *survey*, never through the folder: stepping onto a photo nobody
        // selected would silently be a different view.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4, 7]);
        app.library.as_mut().unwrap().selection.focus(4);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(app.library.as_ref().unwrap().selection.active(), Some(7));

        // And it clamps at the end rather than leaving the survey.
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(app.library.as_ref().unwrap().selection.active(), Some(7));
    }

    #[test]
    fn a_culling_key_rates_the_focused_tile_and_not_the_rest() {
        // ⚠ The asymmetry with the grid, and the half a "did anything get rated?" assertion
        // cannot see: rating the whole survey rates every frame you are choosing between.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4, 7]);
        app.library.as_mut().unwrap().selection.focus(4);
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and the test fails
        // against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);

        frame(&mut app, &ctx, vec![key(egui::Key::Num5)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.marks.get(&lib.entries[4].id).map(|m| m.rating), Some(5));
        assert_eq!(lib.marks.get(&lib.entries[1].id).map(|m| m.rating), None);
        assert_eq!(lib.marks.get(&lib.entries[7].id).map(|m| m.rating), None);
    }

    #[test]
    fn a_key_typed_into_a_text_field_does_not_rate_anything() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20, &[1, 4, 7]);
        crate::test_app::catalog_library(&mut app);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));

        frame(&mut app, &ctx, vec![key(egui::Key::Num5)]);

        assert!(app.library.as_ref().unwrap().marks.is_empty());
    }

    #[test]
    fn a_hidden_photo_never_reaches_the_survey() {
        // ⚠⚠ The sparse-versus-dense boundary, tested with a *filtered* library. Iterating the
        // selection directly rather than intersecting it with `visible` would show a photo the
        // filter says is not there — and would order the survey by index rather than by what
        // the grid shows.
        // ⚠⚠ **The two lists are set by hand, and the first version of this test went through
        // `refilter` instead — where it could not see the bug it names.** `refilter` calls
        // `retain_visible`, which prunes the selection to the visible rows, so afterwards the
        // two agree and iterating the selection directly gives the same answer: the sabotage was
        // inert and the test passed against a `surveyed` that ignored `visible` entirely.
        //
        // ⚠ Production really does keep them in step, so this state is defence in depth — but
        // *because* it is unreachable through `refilter`, constructing it by hand is the only
        // way to assert the intersection at all.
        let mut lib = crate::test_app::library_of(20);
        lib.visible = vec![4];
        lib.selection.select_indices(&[1, 4, 7]);

        assert_eq!(surveyed(&lib.visible, &lib.selection), vec![4]);
    }

    #[test]
    fn the_survey_is_ordered_the_way_the_grid_is() {
        // ⚠ `visible` order, not selection order. A `BTreeSet` happens to be ascending too, so
        // this is only discriminating on a *sparse* visible list — hence the gap in the middle.
        let mut lib = crate::test_app::library_of(20);
        lib.visible = vec![9, 3, 5];
        lib.selection.select_indices(&[3, 5, 9]);
        assert_eq!(surveyed(&lib.visible, &lib.selection), vec![9, 3, 5]);
    }

    #[test]
    fn removing_the_focused_photo_moves_the_focus_first() {
        // ⚠⚠ `Selection::toggle` refuses to deselect the active photo — that is what keeps its
        // invariant — so without moving the focus first the `×` on the focused tile silently
        // does nothing, which reads as the button being broken.
        let mut app = app_with(20, &[1, 4, 7]);
        app.library.as_mut().unwrap().selection.focus(4);

        drop_from_survey(&mut app, 4);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.len(), 2, "the focused photo was not removed");
        assert!(!lib.selection.contains(4));
        assert_eq!(lib.selection.active(), Some(7));
    }

    #[test]
    fn removing_an_unfocused_photo_leaves_the_focus_alone() {
        let mut app = app_with(20, &[1, 4, 7]);
        app.library.as_mut().unwrap().selection.focus(4);

        drop_from_survey(&mut app, 7);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.active(), Some(4));
        assert!(!lib.selection.contains(7));
    }

    #[test]
    fn removing_the_last_photo_goes_back_to_the_grid() {
        // ⚠ A `Selection` with no active photo is not a usable state, and a survey of nothing
        // is not a view worth being in — so the last removal leaves rather than emptying.
        let mut app = app_with(20, &[4]);
        app.library.as_mut().unwrap().selection.focus(4);

        drop_from_survey(&mut app, 4);

        assert_eq!(app.library_mode, LibraryMode::Grid);
    }
}

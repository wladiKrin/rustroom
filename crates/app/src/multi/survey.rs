//! Survey (N) — the whole selection at once, tiled to fill the view.
//!
//! ⚠⚠ **The selection is the *content* of this view, not a target set — and every rule here
//! follows from that.** Clicking a tile moves the focus with [`Selection::focus`] rather than
//! `click`, which would collapse the survey to one photo; the culling keys act on the focused
//! tile alone rather than on the whole selection, which would rate every frame you were
//! choosing between; and `×` on a tile removes it from the selection, which is how a survey is
//! narrowed down to the keeper.
//!
//! ⚠ Capped at [`tile_layout::MAX_TILES`], which is a memory bound before it is a legibility
//! one — see that constant.

use crate::multi;
use crate::{LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::selection::Selection;
use rustroom_app::thumb_loader::ThumbRequest;
use rustroom_app::tile_layout;

/// What one frame of Survey decided, applied after the closure that borrows `app.library`.
#[derive(Default)]
struct Outcome {
    open: Option<usize>,
    mark: Option<(PhotoId, rustroom_app::marks::MarkAction)>,
    /// Make this row the focused tile, keeping the rest of the survey.
    focus: Option<usize>,
    /// Drop this row from the survey.
    drop: Option<usize>,
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
        .take(tile_layout::MAX_TILES)
        .collect()
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Hoisted before any `&mut app.*` — `live_edits` borrows the whole of `app`.
    let live = app.live_edits();
    let mut out = Outcome::default();

    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = &mut app.library else {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Open a folder to survey photos.");
            });
            return;
        };
        let rows = surveyed(&lib.visible, &lib.selection);
        if rows.is_empty() {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(
                    ui,
                    "Select photos in the grid, then press N to survey them.",
                );
            });
            return;
        }
        let total = lib.selection.len();

        // ── Keyboard ─────────────────────────────────────────────────────────────────────
        //
        // ⚠ The `memory().focused()` gate the Library's own keys use — the search field is on
        // screen here too.
        let focused = lib.selection.active();
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        ui.input(|i| {
            if typing {
                return;
            }
            // ⚠ Arrows move through the *survey*, not through the folder: this view shows a
            // handful of photos and stepping off the end into ones nobody selected would be a
            // different view. Clamped rather than wrapping, matching the grid's arrows.
            let at = focused.and_then(|f| rows.iter().position(|&r| r == f));
            for (key, delta) in [(egui::Key::ArrowRight, 1i64), (egui::Key::ArrowLeft, -1)] {
                if i.key_pressed(key) {
                    let next = at.map_or(0, |p| {
                        (p as i64 + delta).clamp(0, rows.len() as i64 - 1) as usize
                    });
                    out.focus = Some(rows[next]);
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
                // ⚠⚠ **Rejecting takes the photo off the wall, and that is what Survey is for.**
                // Without it the view is a smaller Library grid — reported as "I don't get the
                // point of survey". Culling is a subtractive act: the wall shrinks as you
                // reject, and what is left when you stop is the keeper. Nothing is deleted; the
                // photo is still in the folder, still rejected, and `X` again in the grid
                // un-rejects it.
                if a == rustroom_app::marks::MarkAction::Flag(catalog::db::Flag::Reject) {
                    out.drop = Some(f);
                }
            }
        });

        // ── Draw ─────────────────────────────────────────────────────────────────────────
        let hint = if total > rows.len() {
            // ⚠ Said out loud rather than silently truncated. A survey that showed 24 of 57
            // without saying so reads as photos having gone missing.
            format!(
                "Showing {} of {total} selected · click a photo to focus it · X rejects and \
                 takes it off the wall · × removes it without rejecting",
                rows.len()
            )
        } else {
            "Click a photo to focus it · digits rate · P picks · X rejects and takes it off \
             the wall · × removes it without rejecting · Enter opens it"
                .to_string()
        };
        ui.label(egui::RichText::new(hint).small().weak());

        let stage = multi::stage(ui);
        let tiles = tile_layout::survey(rows.len(), stage.width(), stage.height(), 10.0);
        let mut window: Vec<ThumbRequest> = Vec::new();
        for (slot, &row) in rows.iter().enumerate() {
            let Some(t) = tiles.get(slot) else { continue };
            let rect = multi::tile_rect(stage, *t);
            let req = lib.thumb_request(row, live.as_ref());
            let tex = lib
                .cache
                .get(&req.key())
                // ⚠ This photo's previous render rather than grey while a re-render lands.
                .or_else(|| lib.cache.get_stale(&lib.entries[row].id));
            let marks = lib
                .marks
                .get(&lib.entries[row].id)
                .copied()
                .unwrap_or_default();
            let resp = multi::photo_tile(
                ui,
                rect,
                ui.id().with(("survey", slot)),
                tex,
                &lib.entries[row].caption(),
                marks,
                focused == Some(row),
            );
            if resp.double_clicked() {
                out.open = Some(row);
            } else if resp.clicked() {
                out.focus = Some(row);
            }

            // ⚠⚠ The remove button is registered **after** the tile, so it wins the hit test:
            // egui breaks a tie by registration order — "in case of a tie, take the last one =
            // the one on top" — and registered first the tile would swallow every click meant
            // for it. The mirror of the Library grid's drag background, which is registered
            // *before* its cells for the same reason.
            let close = egui::Rect::from_min_size(
                egui::pos2(rect.max.x - 22.0, rect.min.y + 4.0),
                egui::vec2(18.0, 18.0),
            );
            if rect.contains_rect(close) {
                let cr = ui.interact(
                    close,
                    ui.id().with(("survey_close", slot)),
                    egui::Sense::click(),
                );
                if cr.hovered() || focused == Some(row) {
                    ui.painter().text(
                        close.center(),
                        egui::Align2::CENTER_CENTER,
                        // ⚠ `×`, the glyph the reject badge already uses — probed and known to
                        // render. `✕` and `✗` are in no bundled font.
                        "×",
                        egui::FontId::proportional(15.0),
                        egui::Color32::from_rgb(230, 110, 110),
                    );
                }
                if cr.clicked() {
                    out.drop = Some(row);
                }
            }
            window.push(req);
        }
        // ⚠⚠ Everything drawn, or `retain_window` leaves these photos unpinned and evictable
        // while they are the only thing on screen.
        lib.sync_thumb_window(&window);
    });

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
        for k in [egui::Key::Num3, egui::Key::P, egui::Key::U] {
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

    #[test]
    fn a_huge_selection_is_capped_rather_than_pinning_the_whole_cache() {
        // ⚠⚠ Not a legibility nicety: `retain_window` pins everything in the window and never
        // evicts it, so an uncapped survey over hundreds of photos would pin far past the
        // 512 MB budget with nothing sheddable. A cap that only limited the *drawing* would
        // pass a "how many tiles?" assertion and still blow the cache.
        let ctx = crate::test_ui::ctx();
        let all: Vec<usize> = (0..300).collect();
        let mut app = app_with(300, &all);

        frame(&mut app, &ctx, Vec::new());

        assert_eq!(
            app.library.as_ref().unwrap().last_want.len(),
            tile_layout::MAX_TILES
        );
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

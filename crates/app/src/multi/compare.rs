//! Compare (X|Y) — two photos side by side, a **reference** and a **candidate**.
//!
//! The reference is the Library's active photo: the frame you are currently keeping. The
//! candidate is the one being judged against it, moved with the arrow keys and promoted to
//! reference with Enter when it wins.
//!
//! ⚠⚠ **The culling keys act on the candidate, never on both.** That is the whole shape of the
//! view: the reference is settled and the candidate is the question. A key that rated both
//! would answer nothing, and a key that rated the reference would undo a decision you had
//! already made.
//!
//! ⚠ Which photos the candidate moves through is [`rustroom_app::compare_pair::pool`]'s one
//! decision, and it is confined there.

use crate::multi;
use crate::{LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::compare_pair;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::thumb_loader::ThumbRequest;
use rustroom_app::tile_layout;

/// What one frame of Compare decided, applied after the closure that borrows `app.library`.
///
/// ⚠ The deferred-action shape every view in this crate uses: the draw closure holds
/// `&mut app.library`, and all four of these need `&mut RustroomApp`.
#[derive(Default)]
struct Outcome {
    /// Open this row in Develop.
    open: Option<usize>,
    /// Mark this photo.
    mark: Option<(PhotoId, rustroom_app::marks::MarkAction)>,
    /// Remember this candidate. `Some(None)` clears it; `None` leaves it alone.
    candidate: Option<Option<PhotoId>>,
    /// Make this row the reference, keeping the selection it belongs to.
    promote: Option<usize>,
    /// Leave for the grid.
    to_grid: bool,
}

/// The line under the two photos.
///
/// ⚠⚠ Named rather than inline **so the glyph probe can reach it**. Its arrows shipped as tofu
/// boxes because they were chosen without checking — the fifth time this project has done that,
/// after the rail's four glyphs, the mark badges, the keyword checkmarks and the perspective
/// arrows. A literal inside a `RichText::new(..)` call cannot be probed by a test.
///
/// ⚠ `<-` and `->` as ASCII, not `←`/`→`: neither arrow is in any bundled font.
const INSTRUCTION: &str =
    "<- -> change the candidate · Enter promotes it · digits, P, X and U rate it";

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Hoisted before any `&mut app.*`: `live_edits` takes `&self`, so it borrows the whole of
    // `app` and cannot follow a mutable borrow of one field. The grid records the same rule.
    let live = app.live_edits();
    // ⚠ Read out here for the same reason — the closure below holds `app.library`.
    let stored = app.compare_candidate.clone();
    let mut out = Outcome::default();

    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = &mut app.library else {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Open a folder to compare photos.");
            });
            return;
        };
        if lib.visible.is_empty() {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Nothing to compare — every photo is filtered out.");
            });
            return;
        }

        let selected: Vec<usize> = lib.selection.iter().collect();
        // ⚠ Falls back to the first visible row rather than refusing to draw. Arriving in
        // Compare with nothing selected is ordinary — pressing `C` is how you get here — and an
        // empty state you can only leave by going back to the grid is worse than a sensible
        // starting pair.
        let Some(reference) = lib
            .selection
            .active()
            .or_else(|| lib.visible.first().copied())
        else {
            return;
        };
        let pool = compare_pair::pool(&lib.visible, &selected);
        // ⚠ Resolved from the stored **id** every frame, so a candidate whose row moved under a
        // sort, was filtered away, or has just been promoted falls back rather than showing the
        // wrong photograph. See `RustroomApp::compare_candidate`.
        let stored_row = stored
            .as_ref()
            .and_then(|id| lib.entries.iter().position(|e| &e.id == id));
        let candidate = compare_pair::candidate(&pool, reference, stored_row);

        // ── Keyboard ─────────────────────────────────────────────────────────────────────
        //
        // ⚠⚠ Behind the **same** `memory().focused()` gate the grid's keys use, and that is not
        // optional: the Library's search field is on screen in this view too, and a `3` typed
        // into it must not rate a photograph.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        ui.input(|i| {
            if typing {
                return;
            }
            for (key, forward) in [(egui::Key::ArrowRight, true), (egui::Key::ArrowLeft, false)] {
                if i.key_pressed(key) {
                    if let Some(next) = compare_pair::step(&pool, reference, candidate, forward) {
                        out.candidate = Some(lib.entries.get(next).map(|e| e.id.clone()));
                    }
                }
            }
            if i.key_pressed(egui::Key::Enter) {
                out.promote = candidate;
            }
            // ⚠ Escape leaves for the grid, the same exit `G` gives. Without it the one key
            // everybody reaches for to back out of a mode would do nothing here.
            if i.key_pressed(egui::Key::Escape) {
                out.to_grid = true;
            }
            // ⚠ The candidate alone — see the module doc.
            if let (Some(a), Some(c)) = (crate::interaction::culling_key(i), candidate) {
                if let Some(e) = lib.entries.get(c) {
                    out.mark = Some((e.id.clone(), a));
                }
            }
        });

        // ── Draw ─────────────────────────────────────────────────────────────────────────
        ui.label(egui::RichText::new(INSTRUCTION).small().weak());

        let stage = multi::stage(ui);
        // ⚠ `pair`, never `survey(2, ..)`: Compare is side by side at every window shape,
        // because a left-to-right judgement is the entire point of the view.
        let tiles = tile_layout::pair(stage.width(), stage.height(), 10.0);
        if tiles.len() < 2 {
            return;
        }

        let Some(cand) = candidate else {
            // One photo cannot be compared with anything — say so rather than drawing a pane
            // with nothing in it.
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(
                    ui,
                    "Only one photo here — select two or more, or clear the filter.",
                );
            });
            return;
        };

        let mut window: Vec<ThumbRequest> = Vec::new();
        for (slot, (row, label)) in [(reference, "Reference"), (cand, "Candidate")]
            .into_iter()
            .enumerate()
        {
            let rect = multi::tile_rect(stage, tiles[slot]);
            let req = lib.thumb_request(row, live.as_ref());
            let tex = lib
                .cache
                .get(&req.key())
                // ⚠ This photo's previous render if the exact one is still in flight — never
                // grey while a re-render lands, the rule the grid and the filmstrip follow.
                .or_else(|| lib.cache.get_stale(&lib.entries[row].id));
            let caption = format!("{label} — {}", lib.entries[row].caption());
            let marks = lib
                .marks
                .get(&lib.entries[row].id)
                .copied()
                .unwrap_or_default();
            let resp = multi::photo_tile(
                ui,
                rect,
                ui.id().with(("compare", slot)),
                tex,
                &caption,
                marks,
                // The candidate carries the focus ring, because it is what the keys act on.
                slot == 1,
            );
            if resp.double_clicked() {
                out.open = Some(row);
            } else if resp.clicked() && slot == 1 {
                // ⚠ Clicking the candidate promotes it — the same act as Enter, reached with
                // the mouse. Clicking the *reference* deliberately does nothing: it is already
                // the reference, and inventing a second meaning for that click is how one
                // gesture comes to do two things.
                out.promote = Some(row);
            }
            window.push(req);
        }
        // ⚠ Prefetch the candidates either side, so stepping is instant rather than showing a
        // grey pane for a moment. Four requests in all — nothing against the byte budget.
        for forward in [true, false] {
            if let Some(next) = compare_pair::step(&pool, reference, candidate, forward) {
                window.push(lib.thumb_request(next, live.as_ref()));
            }
        }
        // ⚠⚠ The window is told about **everything drawn**, or `retain_window` would leave these
        // two photos unpinned and evictable while they are the only thing on screen.
        lib.sync_thumb_window(&window);
    });

    if let Some(c) = out.candidate {
        app.compare_candidate = c;
    }
    if let Some((id, action)) = out.mark {
        app.apply_mark_to(&[id], action);
    }
    if let Some(row) = out.promote {
        promote(app, row);
    }
    if out.to_grid {
        app.library_mode = LibraryMode::Grid;
    }
    // ⚠ `open_entry` switches to Develop itself, so this must not do it again — two facts that
    // could disagree, and the rule this file already follows for the candidate.
    if let Some(row) = out.open {
        app.open_entry(ctx, row);
    }
}

/// Make `row` the reference, and hand the old reference back as the candidate.
///
/// ⚠⚠ **`focus` when the row is already selected, `click` only when it is not** — and the
/// difference is what keeps the pool stable. `click` replaces the whole selection, so promoting
/// inside a two-photo comparison would collapse the selection to one and silently widen the
/// pool from "these two" to "the whole folder". `Selection::focus` moves the active photo
/// without touching the set, which is exactly a swap.
fn promote(app: &mut RustroomApp, row: usize) {
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let old = lib
        .selection
        .active()
        .and_then(|i| lib.entries.get(i))
        .map(|e| e.id.clone());
    if !lib.selection.focus(row) {
        lib.selection.click(row);
    }
    // ⚠ The old reference becomes the candidate, so Enter is a *swap* rather than a one-way
    // step — press it twice and you are back where you started, which is what makes it safe to
    // try.
    app.compare_candidate = old;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppView;

    /// Run one frame of Compare over a library of `n` synthetic photos.
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

    fn app_with(n: usize) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = AppView::Library;
        app.library_mode = LibraryMode::Compare;
        app.library = Some(crate::test_app::library_of(n));
        app
    }

    #[test]
    fn compare_asks_for_the_two_photos_it_is_showing() {
        // ⚠⚠ The constraint the whole feature hangs on: `retain_window` **pins** what is on
        // screen, so a view that drew from the cache without contributing a want-list would
        // show grey panes *and* let its own photos be evicted. Asserted on the want-list rather
        // than on the drawing, which is the only half this view owns.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(200);
        app.library.as_mut().unwrap().selection.click(0);

        frame(&mut app, &ctx, Vec::new());

        let want = &app.library.as_ref().unwrap().last_want;
        assert!(!want.is_empty(), "Compare asked for nothing at all");
        assert!(
            want.len() <= 4,
            "asked for {} thumbnails — Compare shows two",
            want.len()
        );
        // The reference is row 0, and the candidate is the next one along.
        let paths: Vec<_> = want.iter().map(|r| r.id.path.clone()).collect();
        let entries = &app.library.as_ref().unwrap().entries;
        assert!(
            paths.contains(&entries[0].id.path),
            "the reference is missing"
        );
        assert!(
            paths.contains(&entries[1].id.path),
            "the candidate is missing"
        );
    }

    #[test]
    fn an_arrow_moves_the_candidate_and_leaves_the_reference_alone() {
        // ⚠ Both halves, because moving the *reference* would decode a photo and would make the
        // comparison meaningless — and a test asserting only that the candidate moved cannot
        // see it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.active(), Some(2), "the reference moved");
        assert_eq!(
            app.compare_candidate.as_ref(),
            Some(&lib.entries[4].id),
            "the candidate did not step past the default (row 3)"
        );
    }

    #[test]
    fn enter_swaps_the_two_photos() {
        // ⚠ A *swap*, not a one-way step: press Enter twice and you are back where you started,
        // which is what makes it safe to try. A promote that dropped the old reference would
        // pass an "the active photo changed" assertion and lose the frame you were keeping.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);
        let before = app.library.as_ref().unwrap().entries[2].id.clone();

        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.selection.active(),
            Some(3),
            "the candidate was not promoted"
        );
        assert_eq!(
            app.compare_candidate.as_ref(),
            Some(&before),
            "the old reference was not handed back as the candidate"
        );
    }

    #[test]
    fn promoting_inside_a_two_photo_selection_keeps_both_selected() {
        // ⚠⚠ The reason `promote` calls `focus` rather than `click`. `click` replaces the whole
        // selection, so a swap would collapse "compare exactly these two" to one photo and
        // silently widen the pool to the entire folder — a change with nothing on screen to
        // announce it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[1, 4]);

        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.len(), 2, "the swap collapsed the selection");
        assert_eq!(lib.selection.active(), Some(4));
    }

    #[test]
    fn a_culling_key_rates_the_candidate_and_not_the_reference() {
        // ⚠ The module's central rule, and the half a "did anything get rated?" assertion
        // cannot see: rating the reference would undo a decision already made.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and the test fails
        // against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);

        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.marks.get(&lib.entries[1].id).map(|m| m.rating),
            Some(3),
            "the candidate was not rated"
        );
        assert_eq!(
            lib.marks.get(&lib.entries[0].id).map(|m| m.rating),
            None,
            "the reference was rated too"
        );
    }

    #[test]
    fn a_key_typed_into_a_text_field_does_not_rate_anything() {
        // ⚠ The gate the Library's own keys sit behind, re-asserted here because this view has
        // its own `ui.input` block and could have forgotten it. Simulated by giving egui's
        // memory a focused widget before the frame runs.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        crate::test_app::catalog_library(&mut app);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));

        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert!(
            lib.marks.is_empty(),
            "a digit typed into a field rated a photo: {:?}",
            lib.marks
        );
    }

    #[test]
    fn a_stale_candidate_falls_back_rather_than_showing_nothing() {
        // ⚠ The self-healing half of `compare_candidate`. A remembered id that no longer names
        // a visible row — filtered away, deleted, or from another folder — must not leave the
        // right-hand pane empty with no way to refill it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        app.compare_candidate = Some(PhotoId::new(
            std::path::PathBuf::from("/nonexistent/gone.RAF"),
            0,
        ));

        frame(&mut app, &ctx, Vec::new());

        let want = &app.library.as_ref().unwrap().last_want;
        let entries = &app.library.as_ref().unwrap().entries;
        assert!(
            want.iter().any(|r| r.id == entries[1].id),
            "the stale candidate did not fall back to a real one"
        );
    }

    #[test]
    fn a_filtered_library_compares_only_visible_photos() {
        // ⚠⚠ Tested with a *filtered* library rather than a dense one, because `visible` is a
        // sparse index list and this project has made the sparse-versus-dense mistake three
        // times. A pool taken from `entries` rather than `visible` would offer a photo the
        // filter says is not there.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20);
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "0000".into(),
            ..Default::default()
        };
        app.library.as_mut().unwrap().refilter(&filter);
        let visible = app.library.as_ref().unwrap().visible.clone();
        assert!(
            visible.len() > 1 && visible.len() < 20,
            "the fixture must actually filter something: {visible:?}"
        );
        app.library.as_mut().unwrap().selection.click(visible[0]);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);

        let lib = app.library.as_ref().unwrap();
        let shown: Vec<usize> = lib
            .last_want
            .iter()
            .filter_map(|r| lib.entries.iter().position(|e| e.id == r.id))
            .collect();
        for row in shown {
            assert!(
                lib.visible.contains(&row),
                "row {row} is hidden by the filter and Compare showed it anyway"
            );
        }
    }

    /// ⚠⚠ **Probe before choosing, never after.** This project has shipped tofu boxes four times
    /// — the rail's four glyphs, the mark badges, the keyword checkmarks and the perspective
    /// arrows — and this instruction line made it five: `←` and `→` were chosen without checking
    /// and rendered as empty squares. Reported in use.
    #[test]
    fn every_compare_glyph_is_renderable() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // ⚠ A frame must run first: a fresh `Context` has not built its atlas, so it answers
        // about egui's defaults rather than the stack production actually uses.
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for c in INSTRUCTION.chars().filter(|c| !c.is_ascii()) {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{c:?} (U+{:04X}) is in no bundled font and will draw as a tofu box",
                c as u32
            );
        }
    }
}

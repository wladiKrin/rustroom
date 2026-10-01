//! The grid's keyboard: arrows, Enter, ⌘A, Escape, and the culling keys with auto-advance.

use super::GridOutcome;
use eframe::egui;

/// One frame's keys, when nothing has keyboard focus. Returns the position to centre on, when the
/// selection moved.
pub(super) fn grid_keys(
    i: &egui::InputState,
    lib: &mut crate::Library,
    cols: usize,
    auto_advance: bool,
    out: &mut GridOutcome,
) -> Option<usize> {
    let mut centre_on: Option<usize> = None;
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
        out.open_index = lib.selection.active();
    }
    if i.key_pressed(egui::Key::A) && i.modifiers.command {
        // Visible only: selecting hidden photos is the very hazard the prune
        // exists to prevent.
        lib.selection.select_indices(&lib.visible);
    }
    if i.key_pressed(egui::Key::Escape) {
        lib.selection.clear();
    }
    // ⚠ Only asks: whether a collection is on screen is the app's to answer, after the grid.
    if i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace) {
        out.remove_from_collection = true;
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
        out.mark = Some((lib.selected_ids(), a));
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
    centre_on
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::show;
    use crate::RustroomApp;

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

    /// A collection "Trips" holding all `n` photos, on screen, the second selected.
    fn trips_on_screen(ctx: &egui::Context, n: usize) -> (RustroomApp, i64) {
        let mut app = grid_app(n);
        let ids: Vec<_> = app
            .library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let cat = app.catalog.as_ref().unwrap();
        let trips = cat.create_collection("Trips").unwrap();
        for id in &ids {
            cat.add_to_collection(trips, &id.path, id.copy).unwrap();
        }
        app.open_collection(ctx, trips, "Trips");
        app.library.as_mut().unwrap().selection.click(1);
        (app, trips)
    }

    fn count(app: &RustroomApp, c: i64) -> usize {
        app.catalog
            .as_ref()
            .unwrap()
            .collection_members(c)
            .unwrap()
            .len()
    }

    /// Delete and Backspace take the selection out of the collection on screen.
    #[test]
    fn delete_and_backspace_remove_from_the_collection_on_screen() {
        for key in [egui::Key::Delete, egui::Key::Backspace] {
            let ctx = crate::test_ui::ctx();
            let (mut app, trips) = trips_on_screen(&ctx, 3);
            assert_eq!(count(&app, trips), 3, "the sample");
            grid_frame(&mut app, &ctx, vec![grid_key(key)]);
            assert_eq!(count(&app, trips), 2, "{key:?}");
        }
    }

    /// Outside a collection the key does nothing — it never deletes a photo.
    #[test]
    fn delete_in_a_folder_does_nothing() {
        let ctx = crate::test_ui::ctx();
        let (mut app, trips) = trips_on_screen(&ctx, 3);
        // The same photos, as a folder.
        app.library = Some(crate::test_app::library_of(3));
        app.library.as_mut().unwrap().selection.click(1);
        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Delete)]);
        assert_eq!(count(&app, trips), 3);
        assert_eq!(app.library.as_ref().unwrap().entries.len(), 3);
    }
}

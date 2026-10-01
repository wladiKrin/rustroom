//! Putting photos into a collection and taking them out — every route, and their undo (2026-10-01).
//!
//! ⚠⚠ **One funnel, [`RustroomApp::change_collection`].** The rail's `+` and `−`, the menu's add and
//! remove and the Delete key all go through it, so none of them can be the one that forgets the undo
//! step or says something different — `keyword_ops::tag_files`' rule.
//!
//! ⚠ Per **copy**, not per file: a collection can hold the black-and-white copy and leave the colour
//! one out, which is the whole reason collections are not merely saved searches.

use crate::RustroomApp;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::status_text::photos;

impl RustroomApp {
    /// Put `ids` into `collection` (`add`) or take them out, and record it as **one** undo step.
    ///
    /// ⚠ Records only the copies whose membership changed, so undoing an add never takes out a photo
    /// that was already in. Nothing changed, nothing recorded. Returns how many changed.
    pub(crate) fn change_collection(
        &mut self,
        collection: i64,
        add: bool,
        ids: &[PhotoId],
    ) -> usize {
        let Some(cat) = &self.catalog else {
            return 0;
        };
        let name = cat
            .collections()
            .unwrap_or_default()
            .into_iter()
            .find(|c| c.id == collection)
            .map(|c| c.name)
            .unwrap_or_default();
        let changed: Vec<PhotoId> = ids
            .iter()
            .filter(|id| {
                let r = if add {
                    cat.add_to_collection(collection, &id.path, id.copy)
                } else {
                    cat.remove_from_collection(collection, &id.path, id.copy)
                };
                matches!(r, Ok(true))
            })
            .cloned()
            .collect();
        let n = changed.len();
        if n > 0 {
            let open = self.open_photo();
            self.actions.push(
                ActionKind::CollectionChange {
                    collection,
                    add,
                    members: changed,
                },
                open.as_ref(),
            );
        }
        self.status = status(add, n, &name);
        n
    }

    /// A right-click's targets into a collection — the clicked photo, or the selection it is in.
    pub(crate) fn add_targets_to_collection(&mut self, collection: i64, clicked: usize) {
        let targets = self.menu_targets_of(clicked);
        self.change_collection(collection, true, &targets);
    }

    /// A right-click's targets out of a collection.
    pub(crate) fn remove_targets_from_collection(&mut self, collection: i64, clicked: usize) {
        let targets = self.menu_targets_of(clicked);
        self.change_collection(collection, false, &targets);
    }

    /// The selection into or out of `collection` — the rail's `+` and `−`.
    pub(crate) fn change_selection_in_collection(&mut self, collection: i64, add: bool) {
        let ids = self
            .library
            .as_ref()
            .map(|l| l.selected_ids())
            .unwrap_or_default();
        if !ids.is_empty() {
            self.change_collection(collection, add, &ids);
        }
    }

    /// The selection out of the collection on screen — the Delete key. Nothing outside one.
    pub(crate) fn remove_selection_from_viewed_collection(&mut self) {
        if let Some(c) = self.selected_collection() {
            self.change_selection_in_collection(c, false);
        }
    }

    fn menu_targets_of(&self, clicked: usize) -> Vec<PhotoId> {
        self.library
            .as_ref()
            .map(|l| crate::library::menu_targets(l, clicked))
            .unwrap_or_default()
    }

    /// Reverse a [`ActionKind::CollectionChange`] for undo (`forward: false`) or replay it for redo.
    pub(crate) fn replay_collection_change(&mut self, kind: &ActionKind, forward: bool) {
        let ActionKind::CollectionChange {
            collection,
            add,
            members,
        } = kind
        else {
            return;
        };
        let Some(cat) = &self.catalog else { return };
        let put_in = *add == forward;
        let failed = members
            .iter()
            .map(|id| {
                if put_in {
                    cat.add_to_collection(*collection, &id.path, id.copy)
                } else {
                    cat.remove_from_collection(*collection, &id.path, id.copy)
                }
            })
            .find_map(Result::err);
        self.status = match failed {
            Some(e) => format!("Could not change the collection: {e}"),
            None if forward => "Redone".to_string(),
            None => "Undone".to_string(),
        };
    }
}

/// What a change says in the status row.
fn status(add: bool, n: usize, name: &str) -> String {
    match (add, n) {
        (true, 0) => format!("Already in “{name}”"),
        (false, 0) => format!("Not in “{name}”"),
        (true, n) => format!("Added {} to “{name}” — Ctrl+Z to undo", photos(n)),
        (false, n) => format!("Removed {} from “{name}” — Ctrl+Z to undo", photos(n)),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app;
    use rustroom_app::action_log::ActionKind;
    use rustroom_app::photo_id::PhotoId;

    /// Three photos in the catalog, a collection "Trips" holding the first, and the first two
    /// selected — the state that tells "changed" from "asked about".
    fn trips() -> (crate::RustroomApp, Vec<PhotoId>, i64) {
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(3));
        test_app::catalog_library(&mut app);
        let lib = app.library.as_mut().unwrap();
        let ids: Vec<PhotoId> = lib.entries.iter().map(|e| e.id.clone()).collect();
        lib.selection.click(0);
        lib.selection.toggle(1);
        let cat = app.catalog.as_ref().unwrap();
        let trips = cat.create_collection("Trips").unwrap();
        cat.add_to_collection(trips, &ids[0].path, 0).unwrap();
        (app, ids, trips)
    }

    fn members(app: &crate::RustroomApp, c: i64) -> Vec<PhotoId> {
        let mut m: Vec<PhotoId> = app
            .catalog
            .as_ref()
            .unwrap()
            .collection_members(c)
            .unwrap()
            .into_iter()
            .map(|(p, copy)| PhotoId::new(p, copy))
            .collect();
        m.sort_by(|a, b| a.path.cmp(&b.path));
        m
    }

    /// ⚠⚠ Adding the selection records only the photo that went in, so its undo leaves the one that
    /// was already there.
    #[test]
    fn an_add_records_only_what_changed_and_its_undo_keeps_what_was_in() {
        let (mut app, ids, trips) = trips();
        assert!(!app.actions.can_undo(None), "the fixture pushed nothing");
        app.change_selection_in_collection(trips, true);
        assert_eq!(members(&app, trips), vec![ids[0].clone(), ids[1].clone()]);
        assert_eq!(
            app.actions.peek_undo(None),
            Some(&ActionKind::CollectionChange {
                collection: trips,
                add: true,
                members: vec![ids[1].clone()],
            })
        );
        assert!(
            app.status.starts_with("Added 1 photo to “Trips”"),
            "{}",
            app.status
        );
        app.undo_action(&egui::Context::default());
        assert_eq!(
            members(&app, trips),
            vec![ids[0].clone()],
            "the first stays in"
        );
    }

    #[test]
    fn a_removal_undoes_and_redoes() {
        let (mut app, ids, trips) = trips();
        app.catalog
            .as_ref()
            .unwrap()
            .add_to_collection(trips, &ids[1].path, 0)
            .unwrap();
        app.change_selection_in_collection(trips, false);
        assert!(members(&app, trips).is_empty());
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert_eq!(members(&app, trips), vec![ids[0].clone(), ids[1].clone()]);
        app.redo_action(&ctx);
        assert!(members(&app, trips).is_empty());
    }

    /// A change that changes nothing is not an undo step — a Ctrl+Z that does nothing looks broken.
    #[test]
    fn nothing_changed_pushes_nothing() {
        let (mut app, ids, trips) = trips();
        app.change_collection(trips, false, std::slice::from_ref(&ids[2]));
        assert!(!app.actions.can_undo(None));
        assert_eq!(app.status, "Not in “Trips”");
    }

    /// The menu's add and remove act on the right-click's targets, each one step.
    #[test]
    fn the_menu_adds_and_removes_its_targets() {
        let (mut app, ids, trips) = trips();
        // A click outside the selection targets only that photo.
        app.add_targets_to_collection(trips, 2);
        assert_eq!(members(&app, trips), vec![ids[0].clone(), ids[2].clone()]);
        // A click inside it targets the whole selection.
        app.remove_targets_from_collection(trips, 0);
        assert_eq!(members(&app, trips), vec![ids[2].clone()]);
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert_eq!(members(&app, trips), vec![ids[0].clone(), ids[2].clone()]);
        app.undo_action(&ctx);
        assert_eq!(members(&app, trips), vec![ids[0].clone()]);
        assert!(!app.actions.can_undo(None), "two gestures, two steps");
    }

    /// The rail's `−` on a collection row takes the selection out, from any view — through the
    /// drawn panel.
    #[test]
    fn the_rails_minus_removes_the_selection() {
        let (mut app, ids, trips) = trips();
        app.catalog
            .as_ref()
            .unwrap()
            .add_to_collection(trips, &ids[1].path, 0)
            .unwrap();
        app.settings.panel_sections.collections = true;
        let ctx = crate::test_ui::ctx();
        crate::test_ui::click(&ctx, "Remove from Trips", |ui| {
            crate::collections_panel::show(&mut app, ui, &ctx)
        });
        assert!(members(&app, trips).is_empty());
        assert!(matches!(
            app.actions.peek_undo(None),
            Some(ActionKind::CollectionChange { add: false, .. })
        ));
    }
}

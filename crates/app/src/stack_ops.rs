//! Stacks made by hand (M10.8c): Stack (⌘G), Unstack (⇧⌘G), Make top of stack, Stack by capture time —
//! from the Photo menu and the right-click — and their undo.
//!
//! ⚠ The rules are the lib's `stack_by_hand`; this reads the view, writes the catalog, records **one**
//! undo step per command and rebuilds the rows.

use crate::RustroomApp;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::stack_by_hand::{self, Change};
use std::path::{Path, PathBuf};

impl RustroomApp {
    /// The files `ids` stand for: one per file — copies share it — and every file of a **collapsed**
    /// stack, since its one row is all of them to the eye. Shared with Move to folder.
    pub(crate) fn files_of(&self, ids: &[PhotoId]) -> Vec<PathBuf> {
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        let remote = &self.immich_stacks.of_asset;
        let mut out: Vec<PathBuf> = Vec::new();
        let mut add = |p: &Path| {
            if !out.iter().any(|q| q == p) {
                out.push(p.to_path_buf());
            }
        };
        for id in ids {
            add(&id.path);
            let collapsed = lib
                .entries
                .iter()
                .find(|e| &e.id == id)
                .and_then(|e| e.stack.as_ref())
                .is_some_and(|s| !s.expanded);
            if collapsed {
                let key = rustroom_app::stack::key_for(&id.path, remote, &lib.hand_stacks);
                for e in &lib.scan {
                    if key.is_some()
                        && rustroom_app::stack::key_for(&e.path, remote, &lib.hand_stacks) == key
                    {
                        add(&e.path);
                    }
                }
            }
        }
        out
    }

    /// The selection's ids, the active one first — the top of a new stack.
    fn selection_ids(&self) -> Vec<PhotoId> {
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        let active = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.clone());
        let mut ids: Vec<PhotoId> = active.iter().cloned().collect();
        ids.extend(
            lib.selected_ids()
                .into_iter()
                .filter(|id| Some(id) != active.as_ref()),
        );
        ids
    }

    /// Photo → Stack (⌘G): the selection, with the active photo on top.
    pub(crate) fn stack_selection(&mut self) {
        let ids = self.selection_ids();
        self.stack_ids(&ids);
    }

    /// Stack `ids` — two or more photos — with the first on top.
    pub(crate) fn stack_ids(&mut self, ids: &[PhotoId]) {
        if ids.len() < 2 {
            self.status = "Select two or more photos to stack".to_string();
            return;
        }
        let files = self.files_of(ids);
        let (Some(lib), Some(cat)) = (&self.library, &self.catalog) else {
            return;
        };
        let next = cat.next_stack_id().unwrap_or(1);
        let top = ids.first().map(|i| i.path.clone());
        let changes = stack_by_hand::stack(&files, top.as_deref(), &lib.hand_stacks, next);
        let n = changes.iter().filter(|(_, m)| m.is_some()).count();
        self.apply_stack_changes(changes, format!("Stacked {n} photos — Ctrl+Z to undo"));
    }

    /// Photo → Unstack (⇧⌘G): every hand stack the selection touches.
    pub(crate) fn unstack_selection(&mut self) {
        let ids = self.selection_ids();
        self.unstack_ids(&ids);
    }

    pub(crate) fn unstack_ids(&mut self, ids: &[PhotoId]) {
        let files = self.files_of(ids);
        let Some(lib) = &self.library else { return };
        let changes = stack_by_hand::unstack(&files, &lib.hand_stacks);
        self.apply_stack_changes(changes, "Unstacked — Ctrl+Z to undo".to_string());
    }

    /// Make `id`'s file the top of its hand stack.
    pub(crate) fn make_top_of_stack(&mut self, id: &PhotoId) {
        let Some(lib) = &self.library else { return };
        let changes = stack_by_hand::make_top(&id.path, &lib.hand_stacks);
        self.apply_stack_changes(
            changes,
            "Made the top of its stack — Ctrl+Z to undo".to_string(),
        );
    }

    /// Photo → Make top of stack: the active photo.
    pub(crate) fn make_active_top_of_stack(&mut self) {
        if let Some(id) = self.selection_ids().into_iter().next() {
            self.make_top_of_stack(&id);
        }
    }

    /// Photo → Stack by capture time: the selection, or the whole view with fewer than two selected.
    pub(crate) fn stack_by_capture_time(&mut self, secs: u32) {
        let ids = self.selection_ids();
        let files = if ids.len() >= 2 {
            self.files_of(&ids)
        } else {
            self.library
                .as_ref()
                .map(|l| l.scan.iter().map(|e| e.path.clone()).collect())
                .unwrap_or_default()
        };
        let (Some(lib), Some(cat)) = (&self.library, &self.catalog) else {
            return;
        };
        let timed: Vec<(PathBuf, Option<i64>)> = files
            .into_iter()
            .map(|p| {
                let t = lib.captures.get(&p).and_then(|c| c.taken);
                (p, t)
            })
            .collect();
        let next = cat.next_stack_id().unwrap_or(1);
        let changes = stack_by_hand::by_capture_time(&timed, secs as i64, &lib.hand_stacks, next);
        let stacks: std::collections::HashSet<i64> = changes
            .iter()
            .filter_map(|(_, m)| m.map(|(id, _)| id))
            .collect();
        let n = stacks.len();
        self.apply_stack_changes(
            changes,
            format!(
                "Made {n} {} — Ctrl+Z to undo",
                if n == 1 { "stack" } else { "stacks" }
            ),
        );
    }

    /// Write `changes`, record them as **one** undo step, and rebuild the rows.
    fn apply_stack_changes(&mut self, changes: Vec<Change>, said: String) {
        if changes.is_empty() {
            self.status = "Nothing to change".to_string();
            return;
        }
        let Some(lib) = &self.library else { return };
        let recorded: Vec<_> = changes
            .iter()
            .map(|(p, after)| (p.clone(), lib.hand_stacks.get(p).copied(), *after))
            .collect();
        self.write_stacks(changes.iter().map(|(p, m)| (p.as_path(), *m)));
        let open = self.open_photo();
        self.actions
            .push(ActionKind::StackChange { changes: recorded }, open.as_ref());
        self.status = said;
    }

    /// Undo (`forward: false`) or redo a [`ActionKind::StackChange`].
    pub(crate) fn replay_stack_change(&mut self, kind: &ActionKind, forward: bool) {
        let ActionKind::StackChange { changes } = kind else {
            return;
        };
        self.write_stacks(
            changes
                .iter()
                .map(|(p, before, after)| (p.as_path(), if forward { *after } else { *before })),
        );
        self.status = if forward { "Redone" } else { "Undone" }.to_string();
    }

    /// Write memberships to the catalog, then reload the view's map and rebuild its rows.
    fn write_stacks<'a>(
        &mut self,
        writes: impl Iterator<Item = (&'a Path, stack_by_hand::Membership)>,
    ) {
        if let Some(cat) = &self.catalog {
            for (p, m) in writes {
                if let Err(e) = cat.set_hand_stack(p, m) {
                    eprintln!("catalog: {e}");
                }
            }
        }
        let hand = self
            .library
            .as_ref()
            .map(|l| crate::library_state::hand_stacks_of(self.catalog.as_ref(), &l.scan))
            .unwrap_or_default();
        if let Some(lib) = self.library.as_mut() {
            lib.hand_stacks = hand;
        }
        self.rebuild_entries(None);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app;
    use rustroom_app::menu::Command;

    /// Four photos in the catalog, the first three selected, the first active.
    fn four() -> crate::RustroomApp {
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(4));
        test_app::catalog_library(&mut app);
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(0);
        lib.selection.toggle(1);
        lib.selection.toggle(2);
        app
    }

    fn run(app: &mut crate::RustroomApp, c: Command) {
        crate::commands::execute(app, &egui::Context::default(), c);
    }

    fn rows(app: &crate::RustroomApp) -> Vec<(String, Option<usize>)> {
        let lib = app.library.as_ref().unwrap();
        lib.entries
            .iter()
            .map(|e| (e.file_name.clone(), e.stack.map(|s| s.count)))
            .collect()
    }

    /// ⚠⚠ ⌘G stacks the selection — one row with the active photo on top and a count of three —
    /// as one undo step; Unstack takes it apart.
    #[test]
    fn stack_and_unstack_the_selection() {
        let mut app = four();
        assert_eq!(rows(&app).len(), 4, "the sample");
        run(&mut app, Command::Stack);
        let r = rows(&app);
        assert_eq!(r.len(), 2, "{r:?} — {}", app.status);
        assert_eq!(
            r[0],
            ("rustroom_00000.RAF".to_string(), Some(3)),
            "the active one on top"
        );
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert_eq!(rows(&app).len(), 4, "one undo");
        assert!(!app.actions.can_undo(None), "one step");
        app.redo_action(&ctx);
        assert_eq!(rows(&app).len(), 2, "redo");
        app.library.as_mut().unwrap().selection.click(0);
        run(&mut app, Command::Unstack);
        assert_eq!(rows(&app).len(), 4, "{}", app.status);
    }

    /// Make top of stack changes which photo the collapsed row shows.
    #[test]
    fn make_top_changes_the_shown_photo() {
        let mut app = four();
        run(&mut app, Command::Stack);
        let lib = app.library.as_ref().unwrap();
        let other = lib.scan[2].path.clone();
        app.make_top_of_stack(&rustroom_app::photo_id::PhotoId::original(other));
        assert_eq!(rows(&app)[0].0, "rustroom_00002.RAF");
    }

    /// Stack by capture time on the whole view: runs within the gap stack, the rest stand alone.
    #[test]
    fn stacking_by_capture_time_on_the_view() {
        let mut app = four();
        app.library.as_mut().unwrap().selection.clear();
        app.library.as_mut().unwrap().selection = Default::default();
        let paths: Vec<_> = app
            .library
            .as_ref()
            .unwrap()
            .scan
            .iter()
            .map(|e| e.path.clone())
            .collect();
        let cat = app.catalog.as_ref().unwrap();
        for (p, t) in paths.iter().zip([100, 101, 200, 202]) {
            let c = core_image::Capture {
                taken: Some(t),
                ..Default::default()
            };
            cat.set_capture(p, &c).unwrap();
        }
        app.reload_captures();
        run(&mut app, Command::StackByTime(2));
        let r = rows(&app);
        assert_eq!(r.len(), 2, "{r:?} — {}", app.status);
        assert!(r.iter().all(|(_, c)| *c == Some(2)));
        assert!(app.status.starts_with("Made 2 stacks"), "{}", app.status);
    }

    /// A hand stack is in the catalog: reopening the view shows it again.
    #[test]
    fn a_hand_stack_survives_reopening() {
        let mut app = four();
        run(&mut app, Command::Stack);
        let lib = app.library.as_ref().unwrap();
        let (source, scan) = (lib.source.clone(), lib.scan.clone());
        app.library = None;
        app.open_scan(&egui::Context::default(), source, scan);
        assert_eq!(rows(&app).len(), 2);
    }
}

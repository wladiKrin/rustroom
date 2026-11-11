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
        // ⚠ A hidden stack-mate is out of the rows (hidden before grouping) but still the photo's: it follows.
        let hidden = lib.effective(&self.filter).format;
        // ⚠ Only when something is hidden: otherwise a big selection would walk the folder once per photo.
        let mates_hidden = self.settings.stacking.enabled && hidden.is_active();
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
            if collapsed || mates_hidden {
                let key = rustroom_app::stack::key_for(&id.path, remote, &lib.hand_stacks);
                for e in &lib.scan {
                    if key.is_some()
                        && (collapsed || !hidden.is_visible(&e.path))
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
    pub(crate) fn selection_ids(&self) -> Vec<PhotoId> {
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

    /// Move `from` to `onto`'s place within their unfurled stack — the drag (M10.8c after use); the
    /// leftmost is the top. ⚠ A name or server stack reordered becomes a hand stack in that order, and
    /// stays unfurled under its new key.
    pub(crate) fn move_in_stack(&mut self, from: &PhotoId, onto: &PhotoId) {
        let (Some(lib), Some(cat)) = (&self.library, &self.catalog) else {
            return;
        };
        let remote = &self.immich_stacks.of_asset;
        let key = |p: &Path| rustroom_app::stack::key_for(p, remote, &lib.hand_stacks);
        let k = key(&from.path);
        if k.is_none() || k != key(&onto.path) {
            return;
        }
        let mut order: Vec<PathBuf> = lib
            .entries
            .iter()
            .filter(|e| e.stack.is_some_and(|s| s.expanded) && key(&e.id.path) == k)
            .map(|e| e.id.path.clone())
            .collect();
        let (Some(a), Some(b)) = (
            order.iter().position(|p| *p == from.path),
            order.iter().position(|p| *p == onto.path),
        ) else {
            return;
        };
        let moved = order.remove(a);
        order.insert(b, moved);
        let next = cat.next_stack_id().unwrap_or(1);
        let changes = stack_by_hand::set_order(&order, &lib.hand_stacks, next);
        let stack = changes
            .iter()
            .find(|(p, _)| *p == from.path)
            .and_then(|(_, m)| m.map(|m| m.stack));
        if let (Some(stack), Some(dir), Some(lib)) =
            (stack, from.path.parent(), self.library.as_mut())
        {
            lib.stack_expanded
                .insert((dir.to_path_buf(), format!("hand:{stack}")));
        }
        self.apply_stack_changes(changes, "Reordered the stack — Ctrl+Z to undo".to_string());
    }

    /// Photo → Make top of stack: the active photo.
    pub(crate) fn make_active_top_of_stack(&mut self) {
        if let Some(id) = self.selection_ids().into_iter().next() {
            self.make_top_of_stack(&id);
        }
    }

    /// Photo → Stack by capture time: the selection, or the whole view with fewer than two selected.
    ///
    /// `None` is the menu's *None*: the time stacks there come apart, and nothing new is made.
    pub(crate) fn stack_by_capture_time(&mut self, secs: Option<u32>) {
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
        let changes =
            stack_by_hand::by_capture_time(&timed, secs.map(i64::from), &lib.hand_stacks, next);
        let stacks: std::collections::HashSet<i64> = changes
            .iter()
            .filter_map(|(_, m)| m.map(|m| m.stack))
            .collect();
        let n = stacks.len();
        let said = match secs {
            None => "Took the time stacks apart — Ctrl+Z to undo".to_string(),
            Some(_) => format!(
                "Made {n} {} — Ctrl+Z to undo",
                if n == 1 { "stack" } else { "stacks" }
            ),
        };
        self.apply_stack_changes(changes, said);
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
        self.reload_hand_stacks();
        self.rebuild_entries(None);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app;
    use rustroom_app::menu::Command;

    /// A photo whose stack-mate is hidden still renames and moves WITH it — hiding the JPEG must not split a
    /// RAW+JPEG pair (2026-11-11, with hidden formats leaving their stacks before grouping).
    #[test]
    fn a_hidden_stack_mate_follows_its_photo() {
        let mut app = crate::test_app::for_test();
        app.settings.stacking.enabled = true;
        app.library = Some(crate::test_app::library_of_names(&[
            "a.JPG", "a.RAF", "b.RAF",
        ]));
        app.toggle_format("jpg");
        let a = rustroom_app::photo_id::PhotoId::original("/nonexistent/a.RAF".into());
        let files = app.files_of(&[a]);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["a.RAF", "a.JPG"]);
        // The control: with stacking off a hidden JPEG is not the RAW's, and stays put.
        app.settings.stacking.enabled = false;
        app.rebuild_entries(None);
        let a = rustroom_app::photo_id::PhotoId::original("/nonexistent/a.RAF".into());
        assert_eq!(app.files_of(&[a]).len(), 1);
    }

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
        run(&mut app, Command::StackByTime(Some(2)));
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

    /// ⚠⚠ After use: the count badge unfurls a hand stack, as it does a RAW+JPEG pair. It keyed on the
    /// name alone, so a click on a hand stack's badge did nothing.
    #[test]
    fn the_badge_unfurls_a_hand_stack() {
        let mut app = four();
        app.view = crate::AppView::Library;
        run(&mut app, Command::Stack);
        assert_eq!(rows(&app).len(), 2, "the sample: collapsed");
        let ctx = crate::test_ui::ctx();
        let draw = |app: &mut crate::RustroomApp, ui: &mut egui::Ui| {
            let c = ui.ctx().clone();
            crate::library::show(app, ui, &c);
        };
        // ⚠ By the badge's own name: the pile shrinks the picture, so the badge is not at the cell's
        // corner.
        crate::test_ui::click(&ctx, "Stack of 3", |ui| draw(&mut app, ui));
        let r = rows(&app);
        assert_eq!(r.len(), 4, "unfurled: {r:?}");
        assert_eq!(r.iter().filter(|(_, c)| *c == Some(3)).count(), 3);
    }

    /// After use: *None* takes apart what Stack by capture time made, and leaves a ⌘G stack alone.
    #[test]
    fn none_takes_apart_only_the_time_stacks() {
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(6));
        test_app::catalog_library(&mut app);
        let paths: Vec<_> = app
            .library
            .as_ref()
            .unwrap()
            .scan
            .iter()
            .map(|e| e.path.clone())
            .collect();
        let cat = app.catalog.as_ref().unwrap();
        // 0–1 close together, 2–3 close together, 4–5 far apart.
        for (p, t) in paths.iter().zip([100, 101, 200, 201, 300, 400]) {
            let c = core_image::Capture {
                taken: Some(t),
                ..Default::default()
            };
            cat.set_capture(p, &c).unwrap();
        }
        app.reload_captures();
        // A stack of the user's: 4 and 5.
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(4);
        lib.selection.toggle(5);
        run(&mut app, Command::Stack);
        app.library.as_mut().unwrap().selection = Default::default();
        run(&mut app, Command::StackByTime(Some(2)));
        assert_eq!(
            rows(&app).len(),
            3,
            "two time stacks and the user's: {:?}",
            rows(&app)
        );
        run(&mut app, Command::StackByTime(None));
        let r = rows(&app);
        assert_eq!(
            r.len(),
            5,
            "the time stacks came apart, the user's stayed: {r:?}"
        );
        assert_eq!(r.iter().filter(|(_, c)| *c == Some(2)).count(), 1);
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// ⚠⚠ After use: dragging a photo of an unfurled stack onto another takes its place — through the
    /// drawn grid. The stack of 0, 1, 2 (0 on top) becomes 2, 0, 1, so 2 is the top.
    #[test]
    fn dragging_a_photo_within_an_unfurled_stack_reorders_it() {
        let mut app = four();
        app.view = crate::AppView::Library;
        run(&mut app, Command::Stack);
        let lib = app.library.as_mut().unwrap();
        let first = lib.scan[0].path.clone();
        let key =
            rustroom_app::stack::key_for(&first, &Default::default(), &lib.hand_stacks).unwrap();
        lib.stack_expanded.insert(key);
        app.rebuild_entries(None);
        assert_eq!(rows(&app).len(), 4, "the sample: unfurled");
        let ctx = crate::test_ui::ctx();
        let mut t = 1.0;
        let mut frame = |app: &mut crate::RustroomApp, events: Vec<egui::Event>| {
            t += 0.02;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    time: Some(t),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let c = ui.ctx().clone();
                    crate::library::show(app, ui, &c);
                },
            );
        };
        frame(&mut app, vec![]);
        let found = crate::test_ui::labelled_rects_on(&ctx, crate::test_ui::screen(), |ui| {
            let c = ui.ctx().clone();
            crate::library::show(&mut app, ui, &c);
        });
        // ⚠ The largest rect of that name: the caption under a cell carries the name too.
        let at = |name: &str| {
            found
                .iter()
                .filter(|(l, _)| l == name)
                .map(|(_, r)| *r)
                .max_by(|a, b| a.area().total_cmp(&b.area()))
                .map(|r| r.center())
                .unwrap_or_else(|| panic!("no cell {name}"))
        };
        let (from, to) = (at("rustroom_00002.RAF"), at("rustroom_00000.RAF"));
        frame(&mut app, vec![egui::Event::PointerMoved(from)]);
        frame(&mut app, vec![press(from, true)]);
        for k in 1..=8 {
            let p = from + (to - from) * (k as f32 / 8.0);
            frame(&mut app, vec![egui::Event::PointerMoved(p)]);
        }
        frame(&mut app, vec![press(to, false)]);
        frame(&mut app, vec![]);
        let lib = app.library.as_ref().unwrap();
        let ord = |n: usize| lib.hand_stacks[&lib.scan[n].path].ord;
        assert!(
            ord(2) < ord(0) && ord(0) < ord(1),
            "2, 0, 1 — {}",
            app.status
        );
        assert!(app.status.starts_with("Reordered"), "{}", app.status);
        assert_eq!(rows(&app).len(), 4, "still unfurled");
    }

    /// Reordering a RAW+JPEG pair makes it a hand stack in that order, still unfurled.
    #[test]
    fn a_reordered_pair_becomes_a_hand_stack() {
        let mut app = test_app::for_test();
        app.settings.stacking.enabled = true;
        app.library = Some(test_app::library_of_names(&["a.RAF", "a.JPG", "b.RAF"]));
        test_app::catalog_library(&mut app);
        let lib = app.library.as_mut().unwrap();
        let first = lib.scan[0].path.clone();
        lib.stack_expanded
            .insert(rustroom_app::stack::key_of(&first).unwrap());
        app.rebuild_entries(None);
        assert_eq!(rows(&app).len(), 3, "the sample: the pair unfurled");
        // ⚠ By name: the rebuild sorted the scan, and a.JPG sorts before a.RAF.
        let lib = app.library.as_ref().unwrap();
        let by_name = |n: &str| {
            lib.scan
                .iter()
                .find(|e| e.file_name == n)
                .unwrap()
                .path
                .clone()
        };
        let (raw, jpg) = (by_name("a.RAF"), by_name("a.JPG"));
        app.move_in_stack(
            &rustroom_app::photo_id::PhotoId::original(jpg.clone()),
            &rustroom_app::photo_id::PhotoId::original(raw.clone()),
        );
        // Shown JPEG, RAW (by name); the JPEG dropped onto the RAW takes its place: RAW, JPEG.
        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.hand_stacks[&raw].ord, 0, "the RAW on top now");
        assert_eq!(lib.hand_stacks[&jpg].ord, 1);
        assert!(!lib.hand_stacks[&jpg].by_time, "the user's");
        assert_eq!(rows(&app).len(), 3, "still unfurled under its new key");
    }

    /// Peeking asks for the thumbnails of the photos below a collapsed stack; cards ask for none.
    #[test]
    fn peeking_asks_for_the_photos_below_and_cards_do_not() {
        let wanted = |look| {
            let mut app = four();
            app.view = crate::AppView::Library;
            app.settings.stacking.look = look;
            run(&mut app, Command::Stack);
            let ctx = crate::test_ui::ctx();
            let _ = crate::test_ui::nodes(&ctx, |ui| {
                let c = ui.ctx().clone();
                crate::library::show(&mut app, ui, &c);
            });
            app.library.as_ref().unwrap().last_want.len()
        };
        let peek = wanted(rustroom_app::stack::StackLook::Peek);
        let cards = wanted(rustroom_app::stack::StackLook::Cards);
        let none = wanted(rustroom_app::stack::StackLook::Off);
        assert_eq!(
            cards, 2,
            "two rows on screen: the stack and the fourth photo"
        );
        assert_eq!(peek, 4, "and the stack's two photos below");
        assert_eq!(none, 2, "None asks for no photo below either");
    }

    /// The drawn grid: cards outline one layer per photo below a collapsed stack; None draws no
    /// pile — the count badge alone.
    #[test]
    fn none_draws_no_pile() {
        let layers = |look| {
            let mut app = four();
            app.view = crate::AppView::Library;
            app.settings.stacking.look = look;
            run(&mut app, Command::Stack);
            let ctx = crate::test_ui::ctx();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let c = ui.ctx().clone();
                    crate::library::show(&mut app, ui, &c);
                },
            );
            // ⚠ A pile's layer is the one rect with the pile's 1.5-pt rim as its outline.
            let outlined = out
                .shapes
                .iter()
                .filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.stroke.width == 1.5))
                .count();
            let badge = crate::test_ui::nodes(&ctx, |ui| {
                let c = ui.ctx().clone();
                crate::library::show(&mut app, ui, &c);
            })
            .iter()
            .any(|n| n.label == "Stack of 3");
            (outlined, badge)
        };
        assert_eq!(
            layers(rustroom_app::stack::StackLook::Cards),
            (2, true),
            "the sample: two layers below the top photo, and the count"
        );
        assert_eq!(
            layers(rustroom_app::stack::StackLook::Off),
            (0, true),
            "no layer, the count still there"
        );
    }
}

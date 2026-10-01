//! Renaming, moving and merging keywords from the Keywords panel, and their undo (M9.8); tagging and
//! creating, and theirs (M10.8a).
//!
//! ⚠ Its own module rather than more of `catalog_ops`, which was already ~450 lines of code.
//!
//! ⚠⚠ **Every operation rewrites the XMP of the files it touched**, collected BEFORE the change —
//! a merged-away keyword has no files left to ask about afterwards — and through `write_xmp_for`,
//! the one funnel that honours the Settings toggle and the RAW-only rule.

use crate::modals::Modal;
use crate::RustroomApp;
use catalog::db::KeywordChange;
use rustroom_app::action_log::ActionKind;
use std::path::PathBuf;

impl RustroomApp {
    fn keyword_files(&self, id: i64) -> Vec<PathBuf> {
        self.catalog
            .as_ref()
            .and_then(|c| c.keyword_files(id).ok())
            .unwrap_or_default()
    }

    fn push_catalog_action(&mut self, kind: ActionKind) {
        let open = self.open_photo();
        self.actions.push(kind, open.as_ref());
    }

    /// Rename `id` to what was typed. A collision asks to merge instead.
    pub(crate) fn rename_keyword(&mut self, id: i64, typed: &str) {
        let name = match rustroom_app::keyword_tool::valid_name(typed) {
            Ok(n) => n,
            Err(e) => {
                self.status = e.message().to_string();
                return;
            }
        };
        let Some(cat) = &self.catalog else { return };
        let Ok(from) = cat.keyword_name(id) else {
            return;
        };
        if from == name {
            return;
        }
        let files = self.keyword_files(id);
        let Some(cat) = &self.catalog else { return };
        match cat.rename_keyword(id, &name) {
            Ok(KeywordChange::Done) => {
                self.push_catalog_action(ActionKind::KeywordRename {
                    id,
                    from: from.clone(),
                    to: name.clone(),
                });
                self.write_xmp_for(&files);
                self.status = format!("Renamed “{from}” to “{name}” — Ctrl+Z to undo");
            }
            Ok(KeywordChange::Collides(into)) => self.ask_to_merge(id, into),
            Ok(KeywordChange::Loop) => {}
            Err(e) => self.status = format!("Could not rename the keyword: {e}"),
        }
    }

    /// Move `id` under `parent` (`None`: the top level). A collision asks to merge instead.
    pub(crate) fn move_keyword(&mut self, id: i64, parent: Option<i64>) {
        let Some(cat) = &self.catalog else { return };
        let Ok(from) = cat.keyword_parent(id) else {
            return;
        };
        if from == parent {
            return;
        }
        let files = self.keyword_files(id);
        let Some(cat) = &self.catalog else { return };
        match cat.move_keyword(id, parent) {
            Ok(KeywordChange::Done) => {
                self.push_catalog_action(ActionKind::KeywordMove {
                    id,
                    from,
                    to: parent,
                });
                self.write_xmp_for(&files);
                self.status = "Moved keyword — Ctrl+Z to undo".to_string();
            }
            Ok(KeywordChange::Collides(into)) => self.ask_to_merge(id, into),
            Ok(KeywordChange::Loop) => {
                self.status = "A keyword cannot go inside itself".to_string();
            }
            Err(e) => self.status = format!("Could not move the keyword: {e}"),
        }
    }

    /// Open the merge confirmation for `from` into `into`.
    fn ask_to_merge(&mut self, from: i64, into: i64) {
        let Some(cat) = &self.catalog else { return };
        let (Ok(from_name), Ok(into_name)) = (cat.keyword_name(from), cat.keyword_name(into))
        else {
            return;
        };
        self.modal = Modal::ConfirmMergeKeyword {
            from,
            into,
            from_name,
            into_name,
            photos: self.keyword_files(from).len(),
        };
    }

    /// Merge `from` into `into` — the confirmation's Merge.
    pub(crate) fn merge_keyword_now(&mut self, from: i64, into: i64) {
        let files = self.keyword_files(from);
        let Some(cat) = &self.catalog else { return };
        match cat.merge_keyword(from, into) {
            Ok(rec) => {
                self.push_catalog_action(ActionKind::KeywordMerge(Box::new(rec)));
                self.write_xmp_for(&files);
                // ⚠ The merged-away keyword may be the one on screen.
                if self.selected_keyword() == Some(from) {
                    self.library = None;
                }
                self.status = "Merged keywords — Ctrl+Z to undo".to_string();
            }
            Err(e) => self.status = format!("Could not merge the keywords: {e}"),
        }
    }

    /// Reverse a keyword action for undo (`forward: false`) or replay it for redo.
    pub(crate) fn replay_keyword_action(&mut self, kind: &ActionKind, forward: bool) {
        let Some(cat) = &self.catalog else { return };
        let result = match kind {
            ActionKind::KeywordRename { id, from, to } => {
                let name = if forward { to } else { from };
                let files = cat.keyword_files(*id).unwrap_or_default();
                cat.rename_keyword(*id, name).map(|c| (c, files))
            }
            ActionKind::KeywordMove { id, from, to } => {
                let parent = if forward { *to } else { *from };
                let files = cat.keyword_files(*id).unwrap_or_default();
                cat.move_keyword(*id, parent).map(|c| (c, files))
            }
            ActionKind::KeywordMerge(rec) => {
                if forward {
                    let files = cat.keyword_files(rec.from).unwrap_or_default();
                    cat.merge_keyword(rec.from, rec.into)
                        .map(|_| (KeywordChange::Done, files))
                } else {
                    cat.unmerge_keyword(rec).map(|()| {
                        let files = cat.keyword_files(rec.from).unwrap_or_default();
                        (KeywordChange::Done, files)
                    })
                }
            }
            ActionKind::KeywordTag { keyword, on, files } => {
                let put_on = *on == forward;
                files
                    .iter()
                    .map(|f| {
                        if put_on {
                            cat.assign_keyword(*keyword, f)
                        } else {
                            cat.unassign_keyword(*keyword, f)
                        }
                    })
                    .collect::<Result<Vec<bool>, _>>()
                    .map(|_| (KeywordChange::Done, files.clone()))
            }
            ActionKind::KeywordCreate { id, name, parent } => {
                // ⚠ The files collected BEFORE the delete, the rule of every deletion here — a
                // keyword tagged since by a route the log cannot see still leaves the XMP.
                let files = cat.keyword_files(*id).unwrap_or_default();
                if forward {
                    cat.recreate_keyword(*id, name, *parent)
                } else {
                    cat.delete_keyword(*id).map(|_| ())
                }
                .map(|()| (KeywordChange::Done, files))
            }
            _ => return,
        };
        // ⚠ An undone creation may be the keyword on screen — `delete_keyword_now`'s rule.
        if let ActionKind::KeywordCreate { id, .. } = kind {
            if !forward && self.selected_keyword() == Some(*id) {
                self.library = None;
            }
        }
        match result {
            Ok((KeywordChange::Done, files)) => {
                self.write_xmp_for(&files);
                self.status = if forward { "Redone" } else { "Undone" }.to_string();
            }
            // ⚠ Something now occupies the name or the place — another keyword created since. The
            // step is refused rather than merged behind the user's back.
            Ok(_) => {
                self.status = "The keyword could not be put back — its place is taken".to_string()
            }
            Err(e) => self.status = format!("Could not change the keyword: {e}"),
        }
    }

    /// The selected photos' distinct **files** — keywords are per file, and two copies of one photo
    /// share it, so a count over copies would lie about how much was tagged.
    pub(crate) fn selected_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = Vec::new();
        if let Some(lib) = self.library.as_ref() {
            for i in lib.selection.iter() {
                if let Some(e) = lib.entries.get(i) {
                    if !files.contains(&e.id.path) {
                        files.push(e.id.path.clone());
                    }
                }
            }
        }
        files
    }

    /// Put `keyword` on (`on`) or take it off `files`, and record it as **one** undo step (M10.8a).
    ///
    /// ⚠⚠ Every tagging route goes through here — the rail's `+`/`−`, the menu's toggle, a
    /// suggestion, the field — so none of them can be the one that forgets the undo, and the XMP
    /// write is the same on all of them. Returns the files whose state changed.
    pub(crate) fn tag_files(&mut self, keyword: i64, on: bool, files: &[PathBuf]) -> Vec<PathBuf> {
        let changed = self.apply_tag(keyword, on, files);
        if !changed.is_empty() {
            self.push_catalog_action(ActionKind::KeywordTag {
                keyword,
                on,
                files: changed.clone(),
            });
        }
        self.status = tag_status(on, changed.len());
        changed
    }

    /// The catalog half of [`Self::tag_files`], unrecorded: the files whose state changed.
    ///
    /// ⚠ The XMP is written for every file asked about, not only the changed ones — the behaviour
    /// tagging always had, which also repairs a sidecar left stale while XMP writing was off.
    fn apply_tag(&mut self, keyword: i64, on: bool, files: &[PathBuf]) -> Vec<PathBuf> {
        let Some(cat) = &self.catalog else {
            return Vec::new();
        };
        let changed: Vec<PathBuf> = files
            .iter()
            .filter(|f| {
                let r = if on {
                    cat.assign_keyword(keyword, f)
                } else {
                    cat.unassign_keyword(keyword, f)
                };
                matches!(r, Ok(true))
            })
            .cloned()
            .collect();
        self.write_xmp_for(files);
        changed
    }

    /// Create `typed` under the last-clicked keyword and tag the selection with it — **one** undo
    /// step for both, a `Batch` undone in reverse (M10.8a). Nothing selected: it only creates.
    ///
    /// ⚠ A name already there is reused (`create_keyword`'s rule) and recorded as no creation, so
    /// undoing the tag can never delete a keyword the user had before.
    pub(crate) fn create_keyword_and_tag(&mut self, typed: &str) {
        let name = match rustroom_app::keyword_tool::valid_name(typed) {
            Ok(n) => n,
            Err(e) => {
                self.status = e.message().to_string();
                return;
            }
        };
        let parent = self.new_keyword_parent;
        let Some(cat) = &self.catalog else { return };
        let (id, created) = match cat.create_keyword_reporting(&name, parent) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not create the keyword: {e}");
                return;
            }
        };
        let mut steps = Vec::new();
        if created {
            steps.push(ActionKind::KeywordCreate {
                id,
                name: name.clone(),
                parent,
            });
        }
        let files = self.selected_files();
        let changed = self.apply_tag(id, true, &files);
        if !changed.is_empty() {
            steps.push(ActionKind::KeywordTag {
                keyword: id,
                on: true,
                files: changed.clone(),
            });
        }
        self.status = match (created, changed.len()) {
            (true, 0) => format!("Created “{name}”"),
            (true, n) => format!("Created “{name}” and tagged {} — Ctrl+Z to undo", photos(n)),
            (false, n) => tag_status(true, n),
        };
        match steps.len() {
            0 => {}
            1 => self.push_catalog_action(steps.remove(0)),
            _ => self.push_catalog_action(ActionKind::Batch(steps)),
        }
    }

    /// The files a keyword deletion snapshot touches, for its undo and redo.
    pub(crate) fn snapshot_files(&self, snap: &catalog::snapshot::CatalogSnapshot) -> Vec<PathBuf> {
        match (snap, &self.catalog) {
            (catalog::snapshot::CatalogSnapshot::Keyword { assignments, .. }, Some(cat)) => {
                cat.assignment_paths(assignments).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

/// "1 photo", "3 photos".
fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".to_string()
    } else {
        format!("{n} photos")
    }
}

/// What a tagging says in the status row.
fn tag_status(on: bool, n: usize) -> String {
    match (on, n) {
        (true, 0) => "Every selected photo already has this keyword".to_string(),
        (false, 0) => "No selected photo has this keyword".to_string(),
        (true, n) => format!("Tagged {} — Ctrl+Z to undo", photos(n)),
        (false, n) => format!("Removed the keyword from {} — Ctrl+Z to undo", photos(n)),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app::{for_test, TestDir};
    use rustroom_app::action_log::ActionKind;
    use std::path::{Path, PathBuf};

    /// An app on a real folder with one RAW tagged `Europe > Pariss`, XMP writing on, and the
    /// sidecar written — the state a rename has to rewrite.
    fn tagged(tag: &str) -> (TestDir, crate::RustroomApp, PathBuf, i64, i64) {
        let dir = TestDir::new(tag);
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();
        let mut app = for_test();
        app.settings.write_xmp = true;
        let cat = app.catalog.as_ref().unwrap();
        cat.sync_folder(
            &dir.0,
            &[catalog::CatalogEntry {
                path: photo.clone(),
                file_name: "a.RAF".to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        let eu = cat.create_keyword("Europe", None).unwrap();
        let paris = cat.create_keyword("Pariss", Some(eu)).unwrap();
        cat.assign_keyword(paris, &photo).unwrap();
        app.write_xmp_for(std::slice::from_ref(&photo));
        (dir, app, photo, eu, paris)
    }

    fn xmp(photo: &Path) -> String {
        std::fs::read_to_string(rustroom_app::xmp_sync::sidecar_path(photo)).unwrap_or_default()
    }

    /// ⚠⚠ The plan's done-when, through real files: a rename reaches the XMP of every file
    /// carrying it, lands in the action log, and undoes — sidecar included.
    #[test]
    fn a_rename_rewrites_the_xmp_and_undoes() {
        let (_d, mut app, photo, _eu, paris) = tagged("kw_rename");
        assert!(
            xmp(&photo).contains("Europe|Pariss"),
            "fixture: {}",
            xmp(&photo)
        );
        app.rename_keyword(paris, "  Paris ");
        assert!(
            xmp(&photo).contains("Europe|Paris<"),
            "after the rename: {}",
            xmp(&photo)
        );
        assert!(matches!(
            app.actions.peek_undo(None),
            Some(ActionKind::KeywordRename { .. })
        ));
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert!(
            xmp(&photo).contains("Europe|Pariss"),
            "after undo: {}",
            xmp(&photo)
        );
        app.redo_action(&ctx);
        assert!(
            xmp(&photo).contains("Europe|Paris<"),
            "after redo: {}",
            xmp(&photo)
        );
    }

    #[test]
    fn a_move_rewrites_the_path_in_the_xmp() {
        let (_d, mut app, photo, _eu, paris) = tagged("kw_move");
        app.move_keyword(paris, None);
        let text = xmp(&photo);
        assert!(!text.contains("Europe|Pariss"), "{text}");
        assert!(text.contains(">Pariss<"), "now a root: {text}");
    }

    /// A colliding rename asks, and the confirmation's Merge merges — the photo keeps the survivor
    /// and its XMP says so.
    #[test]
    fn a_colliding_rename_asks_and_merge_merges() {
        let (_d, mut app, photo, eu, paris) = tagged("kw_merge");
        let real = app
            .catalog
            .as_ref()
            .unwrap()
            .create_keyword("Paris", Some(eu))
            .unwrap();
        app.rename_keyword(paris, "Paris");
        let crate::modals::Modal::ConfirmMergeKeyword { from, into, .. } = app.modal else {
            panic!(
                "a collision must ask, got {:?}",
                std::mem::discriminant(&app.modal)
            );
        };
        assert_eq!((from, into), (paris, real));
        app.merge_keyword_now(from, into);
        let cat = app.catalog.as_ref().unwrap();
        assert!(cat.file_has_keyword(real, &photo).unwrap());
        assert!(xmp(&photo).contains("Europe|Paris<"), "{}", xmp(&photo));
    }

    /// ⚠⚠ The gap found while designing M9.8: a deleted keyword stayed in the XMP.
    #[test]
    fn deleting_a_keyword_takes_it_out_of_the_xmp_and_undo_puts_it_back() {
        let (_d, mut app, photo, _eu, paris) = tagged("kw_delete");
        app.delete_keyword_now(paris);
        assert!(!xmp(&photo).contains("Pariss"), "{}", xmp(&photo));
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert!(xmp(&photo).contains("Europe|Pariss"), "{}", xmp(&photo));
    }

    /// Two real RAWs, both selected, XMP writing on; `a` already carries `Europe > Pariss`, `b`
    /// nothing — the state that tells "the files that changed" from "the files asked about".
    fn selected(tag: &str) -> (TestDir, crate::RustroomApp, PathBuf, PathBuf, i64, i64) {
        let (dir, mut app, a, eu, paris) = tagged(tag);
        let b = dir.0.join("b.RAF");
        std::fs::write(&b, b"not a real raw").unwrap();
        let entry = |p: &Path, n: &str| catalog::CatalogEntry {
            path: p.to_path_buf(),
            file_name: n.to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        };
        let cat = app.catalog.as_ref().unwrap();
        cat.sync_folder(&dir.0, &[entry(&a, "a.RAF"), entry(&b, "b.RAF")])
            .unwrap();
        let mut lib = crate::test_app::library_of(2);
        lib.entries[0].id = rustroom_app::photo_id::PhotoId::original(a.clone());
        lib.entries[1].id = rustroom_app::photo_id::PhotoId::original(b.clone());
        lib.selection.click(0);
        lib.selection.toggle(1);
        app.library = Some(lib);
        (dir, app, a, b, eu, paris)
    }

    fn has(app: &crate::RustroomApp, kw: i64, photo: &Path) -> bool {
        app.catalog
            .as_ref()
            .unwrap()
            .file_has_keyword(kw, photo)
            .unwrap()
    }

    /// ⚠⚠ The spec's undo rule: `+` on a selection where `a` already had the keyword records only
    /// `b`, so undoing it takes the keyword off `b` and leaves `a`'s — catalog and XMP both.
    #[test]
    fn undoing_a_tag_leaves_a_tag_the_photo_already_had() {
        let (_d, mut app, a, b, _eu, paris) = selected("kw_tag_undo");
        app.assign_keyword_to_selection(paris);
        assert!(has(&app, paris, &b), "the tag landed");
        assert!(xmp(&b).contains("Europe|Pariss"), "{}", xmp(&b));
        assert_eq!(
            app.actions.peek_undo(None),
            Some(&ActionKind::KeywordTag {
                keyword: paris,
                on: true,
                files: vec![b.clone()],
            }),
            "only the file that changed is recorded"
        );
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert!(has(&app, paris, &a), "a had it before and keeps it");
        assert!(xmp(&a).contains("Europe|Pariss"), "{}", xmp(&a));
        assert!(!has(&app, paris, &b));
        assert!(
            !xmp(&b).contains("Pariss"),
            "undo rewrote b's XMP: {}",
            xmp(&b)
        );
        app.redo_action(&ctx);
        assert!(has(&app, paris, &b));
        assert!(xmp(&b).contains("Europe|Pariss"), "redo: {}", xmp(&b));
    }

    /// Each route is one step, and a tagging that changes nothing is none — a step that undoes
    /// nothing would make Ctrl+Z appear broken.
    #[test]
    fn each_route_pushes_one_step_and_a_no_op_pushes_none() {
        let (_d, mut app, a, b, _eu, paris) = selected("kw_tag_routes");
        assert!(!app.actions.can_undo(None), "the fixture pushed nothing");
        app.set_keyword_on_targets(paris, false, 0);
        assert!(
            !has(&app, paris, &a),
            "the menu's toggle reached the selection"
        );
        assert_eq!(
            app.actions.peek_undo(None),
            Some(&ActionKind::KeywordTag {
                keyword: paris,
                on: false,
                files: vec![a.clone()],
            })
        );
        app.unassign_keyword_from_selection(paris);
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert!(
            has(&app, paris, &a),
            "the − was a no-op, so one undo undid the toggle"
        );
        assert!(xmp(&a).contains("Europe|Pariss"), "{}", xmp(&a));
        assert!(!app.actions.can_undo(None), "exactly one step was pushed");
        assert!(!has(&app, paris, &b));
    }

    /// ⚠⚠ Create-and-tag is ONE step: one undo removes the keyword and every tag it made.
    #[test]
    fn one_undo_removes_a_created_and_tagged_keyword() {
        let (_d, mut app, a, b, _eu, _paris) = selected("kw_create_tag");
        app.create_keyword_and_tag("  Berlin ");
        let cat = app.catalog.as_ref().unwrap();
        let berlin = cat
            .keywords()
            .unwrap()
            .into_iter()
            .find(|k| k.name == "Berlin")
            .expect("created")
            .id;
        assert!(has(&app, berlin, &a) && has(&app, berlin, &b));
        assert!(xmp(&b).contains(">Berlin<"), "{}", xmp(&b));
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        let names: Vec<String> = app
            .catalog
            .as_ref()
            .unwrap()
            .keywords()
            .unwrap()
            .into_iter()
            .map(|k| k.name)
            .collect();
        assert!(
            names.contains(&"Pariss".to_string()),
            "the sample: {names:?}"
        );
        assert!(!names.contains(&"Berlin".to_string()), "{names:?}");
        assert!(!xmp(&a).contains("Berlin"), "{}", xmp(&a));
        assert!(!app.actions.can_undo(None), "one step, not two");
        app.redo_action(&ctx);
        assert!(
            has(&app, berlin, &a) && has(&app, berlin, &b),
            "redo: same id"
        );
        assert!(xmp(&a).contains(">Berlin<"), "{}", xmp(&a));
    }

    /// A name already there is reused — and undoing the tag must not delete it.
    #[test]
    fn creating_an_existing_name_tags_and_its_undo_keeps_the_keyword() {
        let (_d, mut app, a, b, eu, paris) = selected("kw_create_existing");
        app.new_keyword_parent = Some(eu);
        app.create_keyword_and_tag("Pariss");
        assert!(has(&app, paris, &b), "the existing keyword tagged b");
        assert!(matches!(
            app.actions.peek_undo(None),
            Some(ActionKind::KeywordTag { .. })
        ));
        app.undo_action(&egui::Context::default());
        assert!(has(&app, paris, &a), "Pariss and a's tag survive");
        assert!(!has(&app, paris, &b));
    }

    /// Nothing selected: the field only creates, and that is undoable too.
    #[test]
    fn a_bare_create_is_undoable() {
        let (_d, mut app, _a, _eu, _paris) = tagged("kw_create_bare");
        app.create_keyword_and_tag("Night");
        let count =
            |app: &crate::RustroomApp| app.catalog.as_ref().unwrap().keywords().unwrap().len();
        assert_eq!(count(&app), 3);
        assert!(matches!(
            app.actions.peek_undo(None),
            Some(ActionKind::KeywordCreate { .. })
        ));
        let ctx = egui::Context::default();
        app.undo_action(&ctx);
        assert_eq!(count(&app), 2);
        app.redo_action(&ctx);
        assert_eq!(count(&app), 3);
    }

    /// ⚠⚠ The whole path through the drawn rail: `c`, taken the same day as the selection, carries
    /// `Street`, so the rail suggests it; the Suggested row's `+` tags both selected photos and
    /// writes their XMP, as one undo step.
    #[test]
    fn the_rail_suggests_from_the_same_day_and_its_plus_writes_the_xmp() {
        let (d, mut app, a, b, _eu, _paris) = selected("kw_suggest_rail");
        let c = d.0.join("c.RAF");
        std::fs::write(&c, b"not a real raw").unwrap();
        let entry = |p: &Path| catalog::CatalogEntry {
            path: p.to_path_buf(),
            file_name: p.file_name().unwrap().to_string_lossy().into_owned(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        };
        let cat = app.catalog.as_ref().unwrap();
        cat.sync_folder(&d.0, &[entry(&a), entry(&b), entry(&c)])
            .unwrap();
        for p in [&a, &b, &c] {
            let capture = core_image::Capture {
                taken: Some(20_590 * 86_400 + 3_600),
                ..Default::default()
            };
            cat.set_capture(p, &capture).unwrap();
        }
        let street = cat.create_keyword("Street", None).unwrap();
        cat.assign_keyword(street, &c).unwrap();
        app.settings.panel_sections.keywords = true;

        let ctx = crate::test_ui::ctx();
        let tags: Vec<egui::Pos2> = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::collections_panel::show(&mut app, ui, &ctx)
        })
        .into_iter()
        .filter(|(l, _)| l == "Tag Street")
        .map(|(_, p)| p)
        .collect();
        // ⚠ The sample: the tree's row and the suggestion's — one would mean no suggestion.
        assert_eq!(tags.len(), 2, "{tags:?}");
        // The suggestions sit above the tree.
        let at = tags.into_iter().min_by(|p, q| p.y.total_cmp(&q.y)).unwrap();
        crate::test_ui::click_at(&ctx, at, false, |ui| {
            crate::collections_panel::show(&mut app, ui, &ctx)
        });
        assert!(has(&app, street, &a) && has(&app, street, &b));
        assert!(xmp(&a).contains(">Street<"), "{}", xmp(&a));
        assert!(xmp(&b).contains(">Street<"), "{}", xmp(&b));
        assert!(matches!(
            app.actions.peek_undo(None),
            Some(ActionKind::KeywordTag { on: true, .. })
        ));
    }
}

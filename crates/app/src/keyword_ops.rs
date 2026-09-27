//! Renaming, moving and merging keywords from the Keywords panel, and their undo (M9.8).
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
            _ => return,
        };
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
}

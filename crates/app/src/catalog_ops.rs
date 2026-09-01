//! Writing to the catalog: keywords, collection membership, marks, smart-collection rules,
//! and putting a cascading deletion back.
//!
//! Split out of `main.rs` unchanged. ⚠ Every call site degrades to doing nothing when the
//! catalog is absent — a database that will not open must never stop you editing
//! photographs, because edits live in the sidecars and do not need it.

use crate::*;

impl RustroomApp {
    /// Show every photo taken in a calendar period.
    ///
    /// ⚠ Browsed rather than filtered, like a keyword — which is what makes it need **no date
    /// picker**: the tree in the panel is a `GROUP BY` over what is actually there. The cost is
    /// that "in this collection, show me July" is not expressible; that is what M4.3c is for.
    /// Store an edited rule, and refresh the view if it is the one on screen.
    ///
    /// ⚠⚠ **Reopening matters**: without it the grid keeps showing the *old* rule's results with
    /// no sign that anything changed, which reads as Save not having worked.
    pub(crate) fn save_rule(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        name: &str,
        form: &rustroom_app::rule_form::Form,
    ) {
        let json = match serde_json::to_string(&form.to_expr()) {
            Ok(j) => j,
            Err(e) => {
                self.status = format!("Could not build the rule: {e}");
                return;
            }
        };
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.set_smart_rule(id, &json) {
                self.status = format!("Could not save: {e}");
                return;
            }
        }
        self.status = format!("Saved “{name}”");
        let showing = matches!(
            self.library.as_ref().map(|l| &l.source),
            Some(LibrarySource::Smart { id: open, .. }) if *open == id
        );
        if showing {
            self.open_smart(ctx, id, name);
        }
    }

    /// The rule that reproduces what is on screen right now.
    ///
    /// ⚠⚠ This is what makes smart collections usable **with no editor at all**: browse, filter,
    /// save. Its limit is equally structural — the UI can only be in *one* source at a time, so
    /// the rule it captures has at most one source criterion. Composing two ("Portfolio ∩ July")
    /// needs the editor, which is plan 2 and the actual point of the milestone.
    pub(crate) fn rule_from_current(&self) -> rustroom_app::rule::Expr {
        use rustroom_app::rule::{Cmp, Criterion, Expr};
        let mut parts: Vec<Expr> = Vec::new();

        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Folder(p)) => {
                parts.push(Expr::Is(Criterion::Folder(p.clone())));
            }
            Some(LibrarySource::Collection { id, .. }) => {
                parts.push(Expr::Is(Criterion::Collection(*id)));
            }
            Some(LibrarySource::Keyword { id, .. }) => {
                parts.push(Expr::Is(Criterion::Keyword(*id)));
            }
            // ⚠ A set of periods becomes `Any([Period, …])` — exactly the shape a multi-value
            // row takes in the editor, so the two agree by construction.
            Some(LibrarySource::Periods { periods, .. }) if !periods.is_empty() => {
                parts.push(Expr::Any(
                    periods
                        .iter()
                        .map(|&(year, month, day)| Expr::Is(Criterion::Period { year, month, day }))
                        .collect(),
                ));
            }
            // ⚠ All Photos contributes no source criterion — it *is* the universe.
            //
            // ⚠⚠ And neither does a smart collection: saving one's own view would nest a rule
            // inside itself **by id**, which a later delete would leave dangling. Saving from a
            // smart collection therefore captures only the filter, which is the honest answer.
            _ => {}
        }

        let f = &self.filter;
        if !f.text.is_empty() {
            parts.push(Expr::Is(Criterion::Text(f.text.clone())));
        }
        // ⚠ The mark filters are **sets**, so each becomes an `Any([...])` — which is precisely
        // why the rule needed `Any`, and why the checkbox model maps onto it cleanly.
        if !f.stars.is_empty() {
            parts.push(Expr::Any(
                f.stars
                    .ratings()
                    .into_iter()
                    .map(|v| {
                        Expr::Is(Criterion::Rating {
                            op: Cmp::Exactly,
                            value: v,
                        })
                    })
                    .collect(),
            ));
        }
        if !f.flags.is_empty() {
            parts.push(Expr::Any(
                f.flags
                    .flags()
                    .into_iter()
                    .map(|v| Expr::Is(Criterion::Flag(v)))
                    .collect(),
            ));
        }
        if !f.labels.is_empty() {
            parts.push(Expr::Any(
                f.labels
                    .labels()
                    .into_iter()
                    .map(|v| Expr::Is(Criterion::Label(v)))
                    .collect(),
            ));
        }
        if !f.cameras.is_empty() {
            parts.push(Expr::Any(
                f.cameras
                    .iter()
                    .map(|c| Expr::Is(Criterion::Camera(c.clone())))
                    .collect(),
            ));
        }
        if !f.lenses.is_empty() {
            parts.push(Expr::Any(
                f.lenses
                    .iter()
                    .map(|l| Expr::Is(Criterion::Lens(l.clone())))
                    .collect(),
            ));
        }
        Expr::All(parts)
    }

    /// Tag every selected photo's **file** with a keyword.
    ///
    /// ⚠ De-duplicated by path: two copies of one file are one file. Assigning twice is a no-op
    /// anyway, but the status count would otherwise lie about how much was tagged.
    pub(crate) fn assign_keyword_to_selection(&mut self, keyword: i64) {
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
        if files.is_empty() {
            return;
        }
        let mut n = 0;
        if let Some(cat) = &self.catalog {
            for f in &files {
                if cat.assign_keyword(keyword, f).is_ok() {
                    n += 1;
                }
            }
        }
        self.status = format!("Tagged {n} photos");
    }

    /// Remove a keyword from every selected photo — the panel's `−`, mirroring its `+`.
    pub(crate) fn unassign_keyword_from_selection(&mut self, keyword: i64) {
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
        if files.is_empty() {
            return;
        }
        let mut n = 0;
        if let Some(cat) = &self.catalog {
            for f in &files {
                if cat.unassign_keyword(keyword, f).is_ok() {
                    n += 1;
                }
            }
        }
        self.status = format!("Removed the keyword from {n} photos");
    }

    /// Add a right-click's targets to a collection.
    ///
    /// ⚠ Per **copy**, not per file — a collection can hold the black-and-white copy and leave
    /// the colour one out, which is the whole reason collections are not merely saved searches.
    pub(crate) fn add_targets_to_collection(&mut self, collection: i64, clicked: usize) {
        let targets = self
            .library
            .as_ref()
            .map(|l| crate::library::menu_targets(l, clicked))
            .unwrap_or_default();
        let mut n = 0;
        if let Some(cat) = &self.catalog {
            for pid in &targets {
                if cat
                    .add_to_collection(collection, &pid.path, pid.copy)
                    .is_ok()
                {
                    n += 1;
                }
            }
        }
        self.status = format!("Added {n} to the collection");
    }

    /// Assign or remove a keyword across a right-click's targets.
    ///
    /// ⚠ De-duplicated to **files**, the rule `assign_keyword_to_selection` already follows: two
    /// copies of one photo are one file to tag, and a count that said otherwise would lie.
    pub(crate) fn set_keyword_on_targets(&mut self, keyword: i64, on: bool, clicked: usize) {
        let files = self
            .library
            .as_ref()
            .map(|l| crate::library::distinct_files(&crate::library::menu_targets(l, clicked)))
            .unwrap_or_default();
        let mut n = 0;
        if let Some(cat) = &self.catalog {
            for f in &files {
                let ok = if on {
                    cat.assign_keyword(keyword, f).is_ok()
                } else {
                    cat.unassign_keyword(keyword, f).is_ok()
                };
                if ok {
                    n += 1;
                }
            }
        }
        self.status = if on {
            format!("Tagged {n} photos")
        } else {
            format!("Removed the keyword from {n} photos")
        };
    }

    /// Delete a keyword and record it, however the deletion was reached.
    ///
    /// ⚠⚠ **The one implementation both routes share.** Two copies of "delete, then push the undo
    /// entry, then tidy the view" is how one of them ends up not pushing — the rule
    /// `apply_cell_action` and `name_edit` already follow.
    pub(crate) fn delete_keyword_now(&mut self, id: i64) {
        let Some(cat) = &self.catalog else {
            return;
        };
        let snap = match cat.delete_keyword(id) {
            Ok(snap) => snap,
            Err(e) => {
                self.status = format!("Could not delete the keyword: {e}");
                return;
            }
        };
        let open = self.open_photo();
        self.actions.push(
            ActionKind::CatalogDelete {
                snapshot: Box::new(snap),
            },
            open.as_ref(),
        );
        // ⚠ If the deleted keyword was on screen there is nothing left to show — and its children
        // went with it, so a child cannot be showing either.
        if self.selected_keyword() == Some(id) {
            self.library = None;
        }
        self.status = "Deleted keyword — Ctrl+Z to restore".to_string();
    }

    /// Put the view back in step after a catalog row came back.
    ///
    /// ⚠ Only the *folder* case needs anything: restoring a collection or a keyword adds a panel
    /// row, and the panel reads the catalog as it draws. A restored folder, on the other hand,
    /// puts photos back into All Photos, which is materialised.
    pub(crate) fn after_catalog_restore(&mut self, ctx: &egui::Context, snap: &CatalogSnapshot) {
        if matches!(snap, CatalogSnapshot::Folder { .. })
            && matches!(
                self.library.as_ref().map(|l| &l.source),
                Some(LibrarySource::AllPhotos)
            )
        {
            self.open_all_photos(ctx);
        }
    }

    /// Delete again what an undo put back.
    ///
    /// ⚠⚠ **Looked up by NAME, never by the id in the snapshot.** The restore gave the row a
    /// *fresh* id, so the old one either names nothing or — worse — names something else that has
    /// since taken it. That is the same reason `PresetDelete` is keyed by name rather than by
    /// "whatever is newest in the trash".
    pub(crate) fn redo_catalog_delete(&mut self, ctx: &egui::Context, snap: &CatalogSnapshot) {
        let Some(cat) = &self.catalog else {
            return;
        };
        let found = match snap {
            CatalogSnapshot::Smart { name, .. } => cat
                .smart_collections()
                .ok()
                .and_then(|rows| rows.into_iter().find(|r| &r.name == name))
                .map(|r| cat.delete_smart(r.id)),
            CatalogSnapshot::Collection { name, .. } => cat
                .collections()
                .ok()
                .and_then(|rows| rows.into_iter().find(|r| &r.name == name))
                .map(|r| cat.delete_collection(r.id)),
            CatalogSnapshot::Keyword { tree, .. } => tree.first().and_then(|root| {
                cat.keywords()
                    .ok()
                    .and_then(|rows| {
                        rows.into_iter().find(|r| {
                            r.name == root.name && r.parent_id.is_none() == root.parent_id.is_none()
                        })
                    })
                    .map(|r| cat.delete_keyword(r.id))
            }),
            CatalogSnapshot::Folder { path, .. } => cat
                .folders()
                .ok()
                .and_then(|rows| rows.into_iter().find(|r| &r.path == path))
                .map(|r| cat.forget_folder(r.id)),
        };
        match found {
            // ⚠ The returned snapshot is discarded: the log already holds an equivalent one, and
            // the restore reproduced the same content under new ids.
            Some(Ok(_)) => self.status = "Deleted again".into(),
            Some(Err(e)) => self.status = format!("Could not delete: {e}"),
            // Renamed or removed by hand since the undo. Reported rather than silent.
            None => self.status = "It is no longer in the catalog".into(),
        }
        if matches!(snap, CatalogSnapshot::Folder { .. }) {
            match self.library.as_ref().map(|l| l.source.clone()) {
                Some(LibrarySource::AllPhotos) => self.open_all_photos(ctx),
                Some(LibrarySource::Folder(p)) if matches!(snap, CatalogSnapshot::Folder { path, .. } if *path == p) =>
                {
                    self.library = None;
                }
                _ => {}
            }
        }
    }

    /// Apply a culling action to every selected photo.
    ///
    /// ⚠ The **selection**, not the active photo — the rule a paste, a preset and a sync already
    /// follow, and culling is where it matters most: select a run of frames and press `X`.
    pub(crate) fn apply_mark(&mut self, action: rustroom_app::marks::MarkAction) {
        let chosen: Vec<PhotoId> = self
            .library
            .as_ref()
            .map(|l| {
                l.selection
                    .iter()
                    .filter_map(|i| l.entries.get(i))
                    .map(|e| e.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if chosen.is_empty() {
            return;
        }
        if let Some(cat) = &self.catalog {
            for id in &chosen {
                let now = cat.marks(&id.path, id.copy).unwrap_or_default();
                let next = rustroom_app::marks::apply(now, action);
                let _ = cat.set_marks(&id.path, id.copy, next);
            }
        }
        self.reload_marks();
    }
}

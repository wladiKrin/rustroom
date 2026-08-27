//! The Library's left panel: collections and keywords.
//!
//! ⚠ **One panel, two sections — not two panels.** Both are "a named set of photos from
//! anywhere", so they share an idiom: a list with counts, `+` to apply to the selection, click to
//! browse. Two files would invite two different idioms for one concept.
//!
//! ⚠ A **left** panel, in the Library view only — Lightroom's placement, and it competes with
//! nothing. The Presets panel was removed for having two homes; these have one.

use crate::RustroomApp;
use eframe::egui;

/// One collapsible section of the panel.
///
/// ⚠⚠ **`Settings` is what persists; egui owns the state within a session.** This app implements
/// no `eframe::App::save`, so egui's collapsing memory dies with the process — without the
/// stored flag a collapsed section would spring open on every launch. `default_open` seeds egui
/// from the flag on the first frame after a restart, and the header's click updates the flag, so
/// the two say the same thing at every moment they are both defined.
///
/// ⚠ `id_salt` on every one: egui's auto-ids shift with what was drawn before them, and this
/// panel's contents change with the catalog.
fn section<R>(
    ui: &mut egui::Ui,
    open: &mut bool,
    id: &str,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) {
    let r = egui::CollapsingHeader::new(egui::RichText::new(title).heading())
        .id_salt(id)
        .default_open(*open)
        .show(ui, body);
    if r.header_response.clicked() {
        *open = !*open;
    }
}

/// Whether a name field was just committed with Enter.
///
/// ⚠ **Enter, not a bare `lost_focus()`** — clicking away is abandoning, not committing, the rule
/// the copy-rename field already follows.
///
/// ⚠ Written once so the three name fields cannot disagree about what commits.
fn committed(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Draw the collections panel. Every action is applied here.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Collected and applied after the list closes, the shape every panel in this app uses:
    // egui borrows `app` through the closure, so a click cannot act immediately.
    enum Action {
        None,
        Open(i64, String),
        Delete(i64),
        AddSelection(i64),
        OpenKeyword(i64, String),
        DeleteKeyword(i64),
        TagSelection(i64),
        UntagSelection(i64),
        CreateKeyword,
        OpenAllPhotos,
        OpenFolder(std::path::PathBuf),
        ForgetFolder(i64, std::path::PathBuf, i64),
        OpenPeriod(rustroom_app::date_tree::Period, egui::Modifiers),
        OpenSmart(i64, String),
        DeleteSmart(i64),
        EditSmart(i64, String, String),
        SaveSmart,
    }
    let mut action = Action::None;
    // ⚠ Copied into a local and written back once, after the panel: the closure below already
    // borrows half of `app`, and writing the settings file per frame is the `apply_view_demosaic`
    // mistake in another costume.
    let mut sections = app.settings.panel_sections;
    // ⚠ Read once, out here, because Collections and Keywords are separate sections now and both
    // need it — it used to be a local of the Collections block that Keywords happened to see.
    let picked = app.library.as_ref().map(|l| l.selection.len()).unwrap_or(0);
    let selected = app.selected_collection();

    egui::Panel::left("collections")
        .default_size(190.0)
        .show(ui, |ui| {
            // ⚠⚠ **One scroll area for the whole panel, and the three inner ones were REMOVED
            // to get it.** Nested vertical scroll areas fight over the wheel — the inner one
            // under the pointer consumes it, so the panel could not be scrolled past whichever
            // list you happened to be over. With every section collapsible, an inner scrollbar
            // buys nothing that folding the section does not.
            //
            // ⚠ `auto_shrink([false, false])` pins the scrollbar to the panel's edge instead of
            // letting it track the widest row — the same fix the Library grid needed.
            egui::ScrollArea::vertical()
                .id_salt("panel")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // ⚠ Folders first: "where photos come from" is the outer concept, and All Photos —
                    // the whole catalog — is the natural top of it.
                    section(ui, &mut sections.folders, "sec_folders", "Folders", |ui| {
                        let all = app
                            .catalog
                            .as_ref()
                            .and_then(|c| c.photo_count().ok())
                            .unwrap_or(0);
                        let showing_all = matches!(
                            app.library.as_ref().map(|l| &l.source),
                            Some(crate::LibrarySource::AllPhotos)
                        );
                        if ui
                            .selectable_label(showing_all, format!("All Photos ({all})"))
                            .on_hover_text(
                                "Every photo in the catalog, from every folder ever opened",
                            )
                            .clicked()
                        {
                            action = Action::OpenAllPhotos;
                        }
                        let folders = app
                            .catalog
                            .as_ref()
                            .and_then(|c| c.folders().ok())
                            .unwrap_or_default();
                        for f in &folders {
                            ui.horizontal(|ui| {
                                if ui.button("×").on_hover_text("Forget this folder").clicked() {
                                    action = Action::ForgetFolder(f.id, f.path.clone(), f.count);
                                }
                                // ⚠ The basename is what fits and what identifies; the full path is the hover.
                                let shown = f
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| f.path.to_string_lossy().to_string());
                                let selected = matches!(
                                    app.library.as_ref().map(|l| &l.source),
                                    Some(crate::LibrarySource::Folder(p)) if p == &f.path
                                );
                                if ui
                                    .selectable_label(selected, format!("{shown} ({})", f.count))
                                    .on_hover_text(f.path.to_string_lossy())
                                    .clicked()
                                {
                                    action = Action::OpenFolder(f.path.clone());
                                }
                            });
                        }
                    });
                    ui.separator();
                    section(
                        ui,
                        &mut sections.collections,
                        "sec_collections",
                        "Collections",
                        |ui| {
                            ui.horizontal(|ui| {
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut app.new_collection)
                                        .hint_text("New collection")
                                        .desired_width(110.0),
                                );
                                // ⚠ Guarded on a non-empty name: the store keys on the name, and a nameless
                                // collection cannot be told from another nameless one.
                                //
                                // ⚠ The **same** guard serves Enter and `+`, so the two cannot disagree about
                                // what is a valid name — two guards for one rule is how they drift.
                                let named = !app.new_collection.trim().is_empty();
                                let plus = ui.add_enabled(named, egui::Button::new("+")).clicked();
                                if named && (plus || committed(ui, &field)) {
                                    if let Some(cat) = &app.catalog {
                                        let _ = cat.create_collection(app.new_collection.trim());
                                    }
                                    app.new_collection.clear();
                                }
                            });
                            ui.separator();

                            let rows = app
                                .catalog
                                .as_ref()
                                .and_then(|c| c.collections().ok())
                                .unwrap_or_default();
                            if rows.is_empty() {
                                crate::theme::hint(ui, "No collections yet.");
                            }
                            for c in &rows {
                                ui.horizontal(|ui| {
                                    if ui.button("×").on_hover_text("Delete").clicked() {
                                        action = Action::Delete(c.id);
                                    }
                                    // ⚠ Enabled only with a selection, and the hover names the count, so it
                                    // can never be ambiguous about what it is about to do.
                                    if ui
                                        .add_enabled(picked > 0, egui::Button::new("+"))
                                        .on_hover_text(format!("Add {picked} selected"))
                                        .clicked()
                                    {
                                        action = Action::AddSelection(c.id);
                                    }
                                    let label = format!("{} ({})", c.name, c.count);
                                    if ui.selectable_label(selected == Some(c.id), label).clicked()
                                    {
                                        action = Action::Open(c.id, c.name.clone());
                                    }
                                });
                            }
                        },
                    );
                    ui.separator();
                    section(
                        ui,
                        &mut sections.smart,
                        "sec_smart",
                        "Smart collections",
                        |ui| {
                            ui.horizontal(|ui| {
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut app.new_smart)
                                        .hint_text("Save this view")
                                        .desired_width(110.0),
                                );
                                // ⚠ Enabled only with a name *and* a Library open: the rule is built from what is
                                // on screen, so there has to be something on screen.
                                let ready =
                                    !app.new_smart.trim().is_empty() && app.library.is_some();
                                let plus = ui
                                    .add_enabled(ready, egui::Button::new("+"))
                                    .on_hover_text("Save the current source and filter as a rule")
                                    .clicked();
                                if ready && (plus || committed(ui, &field)) {
                                    action = Action::SaveSmart;
                                }
                            });
                            let smarts = app
                                .catalog
                                .as_ref()
                                .and_then(|c| c.smart_collections().ok())
                                .unwrap_or_default();
                            if smarts.is_empty() {
                                crate::theme::hint(ui, "No smart collections yet.");
                            }
                            for sc in &smarts {
                                ui.horizontal(|ui| {
                                    // ⚠ No confirmation: a smart collection is a **query** with nothing behind it,
                                    // so it follows `delete_collection`'s precedent rather than `forget_folder`'s,
                                    // which takes ratings and keywords with it.
                                    if ui.button("×").on_hover_text("Delete").clicked() {
                                        action = Action::DeleteSmart(sc.id);
                                    }
                                    if ui.button("⚙").on_hover_text("Edit the rule").clicked() {
                                        action = Action::EditSmart(
                                            sc.id,
                                            sc.name.clone(),
                                            sc.rule.clone(),
                                        );
                                    }
                                    let selected = matches!(
                                        app.library.as_ref().map(|l| &l.source),
                                        Some(crate::LibrarySource::Smart { id, .. }) if *id == sc.id
                                    );
                                    // ⚠⚠ **No count, unlike Collections and Keywords.** Computing one means
                                    // evaluating the rule over every photo, every frame. The row does not pretend
                                    // to know something it would be expensive to know.
                                    if ui.selectable_label(selected, &sc.name).clicked() {
                                        action = Action::OpenSmart(sc.id, sc.name.clone());
                                    }
                                });
                            }
                        },
                    );
                    ui.separator();
                    section(
                        ui,
                        &mut sections.keywords,
                        "sec_keywords",
                        "Keywords",
                        |ui| {
                            ui.horizontal(|ui| {
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut app.new_keyword)
                                        .hint_text("New keyword")
                                        .desired_width(110.0),
                                );
                                let named = !app.new_keyword.trim().is_empty();
                                let plus = ui.add_enabled(named, egui::Button::new("+")).clicked();
                                if named && (plus || committed(ui, &field)) {
                                    action = Action::CreateKeyword;
                                }
                            });
                            let kws = app
                                .catalog
                                .as_ref()
                                .and_then(|c| c.keywords().ok())
                                .unwrap_or_default();
                            // ⚠ The parent is chosen *before* creating, because there is no drag-and-drop and no
                            // re-parenting: this is the only moment a keyword's place in the tree is decided.
                            let parent_name = app
                                .new_keyword_parent
                                .and_then(|id| kws.iter().find(|k| k.id == id))
                                .map(|k| k.name.clone())
                                .unwrap_or_else(|| "(top level)".to_string());
                            egui::ComboBox::from_id_salt("kw_parent")
                                .selected_text(format!("under {parent_name}"))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut app.new_keyword_parent,
                                        None,
                                        "(top level)",
                                    );
                                    for k in &kws {
                                        ui.selectable_value(
                                            &mut app.new_keyword_parent,
                                            Some(k.id),
                                            format!("{}{}", "  ".repeat(k.depth), k.name),
                                        );
                                    }
                                });

                            if kws.is_empty() {
                                crate::theme::hint(ui, "No keywords yet.");
                            }
                            let picked_kw = app.selected_keyword();
                            for k in &kws {
                                ui.horizontal(|ui| {
                                    // ⚠ Indented by depth — the only thing that makes the tree readable,
                                    // since the rows are a flat depth-first list.
                                    ui.add_space(k.depth as f32 * 12.0);
                                    if ui
                                        .button("×")
                                        .on_hover_text("Delete, with everything under it")
                                        .clicked()
                                    {
                                        action = Action::DeleteKeyword(k.id);
                                    }
                                    if ui
                                        .add_enabled(picked > 0, egui::Button::new("+"))
                                        .on_hover_text(format!("Tag {picked} selected"))
                                        .clicked()
                                    {
                                        action = Action::TagSelection(k.id);
                                    }
                                    // ⚠ Completing the existing mechanism rather than adding a second
                                    // one: `unassign_keyword` already existed and was tested, and the
                                    // `+` was already here. Both go through one pair of functions the
                                    // photo context menu also calls.
                                    if ui
                                        .add_enabled(picked > 0, egui::Button::new("−"))
                                        .on_hover_text(format!("Remove from {picked} selected"))
                                        .clicked()
                                    {
                                        action = Action::UntagSelection(k.id);
                                    }
                                    let label = format!("{} ({})", k.name, k.count);
                                    if ui
                                        .selectable_label(picked_kw == Some(k.id), label)
                                        .clicked()
                                    {
                                        action = Action::OpenKeyword(k.id, k.name.clone());
                                    }
                                });
                            }
                        },
                    );
                    ui.separator();
                    section(ui, &mut sections.dates, "sec_dates", "Dates", |ui| {
                        // ⚠ A tree built from a `GROUP BY` over what is actually in the catalog — which is
                        // what makes this need **no date picker**, since egui has none. It shows the days you
                        // have rather than asking you to type a range.
                        let days = app
                            .catalog
                            .as_ref()
                            .and_then(|c| c.capture_days().ok())
                            .unwrap_or_default();
                        if days.is_empty() {
                            crate::theme::hint(ui, "No capture dates yet.");
                        }
                        // ⚠ Every selected period highlights, not just one — the point of multi-select.
                        let open_periods: Vec<rustroom_app::date_tree::Period> =
                            match app.library.as_ref().map(|l| &l.source) {
                                Some(crate::LibrarySource::Periods { periods, .. }) => {
                                    periods.clone()
                                }
                                _ => Vec::new(),
                            };
                        for y in rustroom_app::date_tree::group(&days) {
                            // ⚠ `id_salt` on every header: egui's auto-ids shift with what was drawn
                            // before them — the hazard the grid's `push_id` fixed — so two years with
                            // the same month number would otherwise collapse together.
                            egui::CollapsingHeader::new(format!("{} ({})", y.year, y.count))
                                .id_salt(("year", y.year))
                                .show(ui, |ui| {
                                    let sel = open_periods.contains(&(y.year, None, None));
                                    if ui
                                        .selectable_label(sel, format!("All of {}", y.year))
                                        .clicked()
                                    {
                                        action = Action::OpenPeriod(
                                            (y.year, None, None),
                                            ui.input(|i| i.modifiers),
                                        );
                                    }
                                    for m in &y.months {
                                        let name = rustroom_app::date_tree::month_name(m.month);
                                        egui::CollapsingHeader::new(format!(
                                            "{name} ({})",
                                            m.count
                                        ))
                                        .id_salt(("month", y.year, m.month))
                                        .show(ui, |ui| {
                                            let sel = open_periods.contains(&(
                                                y.year,
                                                Some(m.month),
                                                None,
                                            ));
                                            if ui
                                                .selectable_label(sel, format!("All of {name}"))
                                                .clicked()
                                            {
                                                action = Action::OpenPeriod(
                                                    (y.year, Some(m.month), None),
                                                    ui.input(|i| i.modifiers),
                                                );
                                            }
                                            for d in &m.days {
                                                let sel = open_periods.contains(&(
                                                    y.year,
                                                    Some(m.month),
                                                    Some(d.day),
                                                ));
                                                let label =
                                                    format!("{} {name} ({})", d.day, d.count);
                                                if ui.selectable_label(sel, label).clicked() {
                                                    action = Action::OpenPeriod(
                                                        (y.year, Some(m.month), Some(d.day)),
                                                        ui.input(|i| i.modifiers),
                                                    );
                                                }
                                            }
                                        });
                                    }
                                });
                        }
                    });
                });
        });

    if sections != app.settings.panel_sections {
        app.settings.panel_sections = sections;
        app.save_settings();
    }

    match action {
        Action::None => {}
        Action::Open(id, name) => app.open_collection(ctx, id, &name),
        Action::Delete(id) => {
            if let Some(cat) = &app.catalog {
                let _ = cat.delete_collection(id);
            }
            // ⚠ If the deleted collection was the one on screen, there is nothing left to show.
            if app.selected_collection() == Some(id) {
                app.library = None;
                app.status = "Collection deleted".into();
            }
        }
        Action::AddSelection(id) => {
            let chosen: Vec<_> = app
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
            let mut n = 0;
            if let Some(cat) = &app.catalog {
                for pid in &chosen {
                    if cat.add_to_collection(id, &pid.path, pid.copy).is_ok() {
                        n += 1;
                    }
                }
            }
            app.status = format!("Added {n} to the collection");
        }
        Action::CreateKeyword => {
            if let Some(cat) = &app.catalog {
                let _ = cat.create_keyword(app.new_keyword.trim(), app.new_keyword_parent);
            }
            app.new_keyword.clear();
        }
        Action::OpenAllPhotos => app.open_all_photos(ctx),
        Action::OpenSmart(id, name) => app.open_smart(ctx, id, &name),
        Action::EditSmart(id, name, rule) => {
            // ⚠ The form and the dropdown lists are read **once**, here, not per frame.
            let parsed: Option<rustroom_app::rule::Expr> = serde_json::from_str(&rule).ok();
            let form = parsed
                .as_ref()
                .and_then(rustroom_app::rule_form::Form::from_expr);
            // ⚠⚠ A rule the flat form cannot show is opened **read-only**, never as an empty
            // form: an empty form that replaced a working rule on Save is the data loss the
            // refusal exists to prevent.
            app.modal = crate::modals::Modal::EditRule {
                id,
                name,
                unsupported: form.is_none(),
                form: form.unwrap_or_default(),
                lists: crate::modals::rule_edit::Lists::read(app),
            };
        }
        Action::DeleteSmart(id) => {
            if let Some(cat) = &app.catalog {
                let _ = cat.delete_smart(id);
            }
            // ⚠ If the deleted rule was the one on screen there is nothing left to show — the
            // rule the collection and keyword deletions already follow.
            let showing = matches!(
                app.library.as_ref().map(|l| &l.source),
                Some(crate::LibrarySource::Smart { id: open, .. }) if *open == id
            );
            if showing {
                app.library = None;
            }
        }
        Action::SaveSmart => {
            let rule = app.rule_from_current();
            let name = app.new_smart.trim().to_string();
            match serde_json::to_string(&rule) {
                Ok(json) => {
                    if let Some(cat) = &app.catalog {
                        match cat.create_smart(&name, &json) {
                            Ok(_) => app.status = format!("Saved “{name}”"),
                            Err(e) => app.status = format!("Could not save: {e}"),
                        }
                    }
                    app.new_smart.clear();
                }
                Err(e) => app.status = format!("Could not build the rule: {e}"),
            }
        }
        Action::OpenPeriod(period, mods) => app.click_period(ctx, period, mods),
        Action::OpenFolder(path) => app.open_catalog_folder(ctx, &path),
        Action::ForgetFolder(id, path, count) => {
            app.modal = crate::modals::Modal::ConfirmForgetFolder { path, id, count };
        }
        Action::OpenKeyword(id, name) => app.open_keyword(ctx, id, &name),
        Action::DeleteKeyword(id) => {
            if let Some(cat) = &app.catalog {
                let _ = cat.delete_keyword(id);
            }
            // ⚠ If the deleted keyword was on screen there is nothing left to show — and its
            // children went with it, so a child cannot be showing either.
            if app.selected_keyword() == Some(id) {
                app.library = None;
                app.status = "Keyword deleted".into();
            }
        }
        Action::TagSelection(id) => app.assign_keyword_to_selection(id),
        Action::UntagSelection(id) => app.unassign_keyword_from_selection(id),
    }
}

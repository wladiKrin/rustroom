//! The Library's left panel: collections and keywords.
//!
//! ⚠ **One panel, two sections — not two panels.** Both are "a named set of photos from
//! anywhere", so they share an idiom: a list with counts, `+` to apply to the selection, click to
//! browse. Two files would invite two different idioms for one concept.
//!
//! ⚠ A **left** panel, in the Library view only — Lightroom's placement, and it competes with
//! nothing. The Presets panel was removed for having two homes; these have one.

use crate::develop::panel::section::{header, Header};
use crate::list_row::{self, RowAction, RowSpec};
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::icons;
use rustroom_app::settings::PanelSection;

/// One collapsible section of the panel: the Develop panel's header, draggable.
///
/// ⚠⚠ **`Settings` is what persists; egui owns the state within a session.** This app implements
/// no `eframe::App::save`, so egui's collapsing memory dies with the process — without the
/// stored flag a collapsed section would spring open on every launch. `default_open` seeds egui
/// from the flag on the first frame after a restart, and the header's click updates the flag, so
/// the two say the same thing at every moment they are both defined. The flag mirrors egui's
/// state rather than competing with it: an earlier version flipped `*open` directly and left the
/// panel with two click targets that disagreed.
///
/// ⚠ `make_persistent_id` on every one: egui's auto-ids shift with what was drawn before them,
/// and this panel's contents change with the catalog.
///
/// Returns the header row's `Response`, which senses the drag that reorders the sections —
/// `Header::draggable` makes it `Sense::click_and_drag`, and egui reports `clicked()` only for
/// a press-release that did **not** become a drag, so the two gestures exclude each other by
/// construction.
fn section<R>(
    ui: &mut egui::Ui,
    open: &mut bool,
    id: &str,
    title: &str,
    trailing: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::Response {
    let state_id = ui.make_persistent_id(id);
    let mut h = Header::new(title).draggable(true);
    if !trailing.is_empty() {
        h = h.trailing(trailing);
    }
    let mut out = header(ui, state_id, *open, h);
    out.state.show_body_unindented(ui, body);
    *open = out.state.is_open();
    out.response
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
        DeleteKeyword(i64, String),
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
    // ⚠ One coherent set: which section a grip started dragging this frame, whether a grip was
    // released this frame, and where the pointer says it would land. The move is applied only
    // when all three line up, after the panel closes — the closure already borrows `app`.
    let mut drag_started: Option<PanelSection> = None;
    let mut drag_released = false;
    let mut drop_to: Option<usize> = None;

    egui::Panel::left("collections")
        .default_size(210.0)
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
                    // ⚠⚠ **The order is a value now, and the `match` is the whole trick**: it lets five
                    // closures over five different sets of `app` fields keep their exact previous
                    // form while their order becomes data. The bodies below are unchanged.
                    let order = app.settings.panel_order();
                    // Each header's top edge this frame, for `panel_order::drop_index`.
                    let mut tops: Vec<f32> = Vec::with_capacity(order.len());
                    for sec in &order {
                        let top = ui.cursor().top();
                        let grip = match sec {
                            PanelSection::Folders => {
                                let all = app
                                    .catalog
                                    .as_ref()
                                    .and_then(|c| c.photo_count().ok())
                                    .unwrap_or(0);
                                let folders = app
                                    .catalog
                                    .as_ref()
                                    .and_then(|c| c.folders().ok())
                                    .unwrap_or_default();
                                let n = folders.len().to_string();
                                section(ui, &mut sections.folders, "sec_folders", "Folders", &n, |ui| {
                                    let showing_all = matches!(
                                        app.library.as_ref().map(|l| &l.source),
                                        Some(crate::LibrarySource::AllPhotos)
                                    );
                                    let r = list_row::row(
                                        ui,
                                        RowSpec {
                                            name: "All Photos",
                                            count: Some(all),
                                            selected: showing_all,
                                            indent: 0.0,
                                            hover: "Every photo in the catalog, from every folder ever opened",
                                            actions: &[],
                                        },
                                    );
                                    if r.clicked {
                                        action = Action::OpenAllPhotos;
                                    }
                                    for f in &folders {
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
                                        let path = f.path.to_string_lossy();
                                        let r = list_row::row(
                                            ui,
                                            RowSpec {
                                                name: &shown,
                                                count: Some(f.count),
                                                selected,
                                                indent: 0.0,
                                                hover: &path,
                                                actions: &[RowAction {
                                                    glyph: '×',
                                                    verb: "Forget",
                                                    hover: "Forget this folder",
                                                    enabled: true,
                                                }],
                                            },
                                        );
                                        if r.action == Some(0) {
                                            action = Action::ForgetFolder(f.id, f.path.clone(), f.count);
                                        } else if r.clicked {
                                            action = Action::OpenFolder(f.path.clone());
                                        }
                                    }
                                })
                            }
                            PanelSection::Collections => {
                                let rows = app
                                    .catalog
                                    .as_ref()
                                    .and_then(|c| c.collections().ok())
                                    .unwrap_or_default();
                                let n = rows.len().to_string();
                                section(
                                    ui,
                                    &mut sections.collections,
                                    "sec_collections",
                                    "Collections",
                                    &n,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            let field = ui.add(
                                                egui::TextEdit::singleline(&mut app.new_collection)
                                                    .hint_text("New collection")
                                                    .desired_width(ui.available_width() - 30.0),
                                            );
                                            let named = !app.new_collection.trim().is_empty();
                                            let plus =
                                                ui.add_enabled(named, egui::Button::new("+")).clicked();
                                            if named && (plus || theme::committed(ui, &field)) {
                                                if let Some(cat) = &app.catalog {
                                                    let _ = cat.create_collection(app.new_collection.trim());
                                                }
                                                app.new_collection.clear();
                                            }
                                        });
                                        if rows.is_empty() {
                                            crate::theme::hint(ui, "No collections yet.");
                                        }
                                        // ⚠ Enabled only with a selection, and the hover names the count, so it
                                        // can never be ambiguous about what it is about to do.
                                        let add_hover = format!("Add {picked} selected");
                                        for c in &rows {
                                            let r = list_row::row(
                                                ui,
                                                RowSpec {
                                                    name: &c.name,
                                                    count: Some(c.count),
                                                    selected: selected == Some(c.id),
                                                    indent: 0.0,
                                                    hover: "",
                                                    actions: &[
                                                        RowAction {
                                                            glyph: '×',
                                                            verb: "Delete",
                                                            hover: "Delete",
                                                            enabled: true,
                                                        },
                                                        RowAction {
                                                            glyph: '+',
                                                            verb: "Add to",
                                                            hover: &add_hover,
                                                            enabled: picked > 0,
                                                        },
                                                    ],
                                                },
                                            );
                                            match r.action {
                                                Some(0) => action = Action::Delete(c.id),
                                                Some(_) => action = Action::AddSelection(c.id),
                                                None if r.clicked => {
                                                    action = Action::Open(c.id, c.name.clone())
                                                }
                                                None => {}
                                            }
                                        }
                                    },
                                )
                            }
                            PanelSection::Smart => {
                                let smarts = app
                                    .catalog
                                    .as_ref()
                                    .and_then(|c| c.smart_collections().ok())
                                    .unwrap_or_default();
                                let n = smarts.len().to_string();
                                section(
                                    ui,
                                    &mut sections.smart,
                                    "sec_smart",
                                    "Smart collections",
                                    &n,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            let field = ui.add(
                                                egui::TextEdit::singleline(&mut app.new_smart)
                                                    .hint_text("Save this view")
                                                    .desired_width(ui.available_width() - 30.0),
                                            );
                                            let ready =
                                                !app.new_smart.trim().is_empty() && app.library.is_some();
                                            let plus = ui
                                                .add_enabled(ready, egui::Button::new("+"))
                                                .on_hover_text("Save the current source and filter as a rule")
                                                .clicked();
                                            if ready && (plus || theme::committed(ui, &field)) {
                                                action = Action::SaveSmart;
                                            }
                                        });
                                        if smarts.is_empty() {
                                            crate::theme::hint(ui, "No smart collections yet.");
                                        }
                                        for sc in &smarts {
                                            let selected = matches!(
                                                app.library.as_ref().map(|l| &l.source),
                                                Some(crate::LibrarySource::Smart { id, .. }) if *id == sc.id
                                            );
                                            // ⚠⚠ **No count, unlike Collections and Keywords.** Computing one means
                                            // evaluating the rule over every photo, every frame. The row does not
                                            // pretend to know something it would be expensive to know.
                                            //
                                            // ⚠ No confirmation on delete: a smart collection is a **query** with
                                            // nothing behind it, so it follows `delete_collection`'s precedent rather
                                            // than `forget_folder`'s, which takes ratings and keywords with it.
                                            let r = list_row::row(
                                                ui,
                                                RowSpec {
                                                    name: &sc.name,
                                                    count: None,
                                                    selected,
                                                    indent: 0.0,
                                                    hover: "",
                                                    actions: &[
                                                        RowAction {
                                                            glyph: '×',
                                                            verb: "Delete",
                                                            hover: "Delete",
                                                            enabled: true,
                                                        },
                                                        RowAction {
                                                            glyph: icons::GEAR,
                                                            verb: "Edit",
                                                            hover: "Edit the rule",
                                                            enabled: true,
                                                        },
                                                    ],
                                                },
                                            );
                                            match r.action {
                                                Some(0) => action = Action::DeleteSmart(sc.id),
                                                Some(_) => {
                                                    action = Action::EditSmart(
                                                        sc.id,
                                                        sc.name.clone(),
                                                        sc.rule.clone(),
                                                    )
                                                }
                                                None if r.clicked => {
                                                    action = Action::OpenSmart(sc.id, sc.name.clone())
                                                }
                                                None => {}
                                            }
                                        }
                                    },
                                )
                            }
                            PanelSection::Keywords => {
                                let kws = app
                                    .catalog
                                    .as_ref()
                                    .and_then(|c| c.keywords().ok())
                                    .unwrap_or_default();
                                let n = kws.len().to_string();
                                section(
                                    ui,
                                    &mut sections.keywords,
                                    "sec_keywords",
                                    "Keywords",
                                    &n,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            let field = ui.add(
                                                egui::TextEdit::singleline(&mut app.new_keyword)
                                                    .hint_text("New keyword")
                                                    .desired_width(ui.available_width() - 30.0),
                                            );
                                            let named = !app.new_keyword.trim().is_empty();
                                            let plus =
                                                ui.add_enabled(named, egui::Button::new("+")).clicked();
                                            if named && (plus || theme::committed(ui, &field)) {
                                                action = Action::CreateKeyword;
                                            }
                                        });
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
                                        let tag_hover = format!("Tag {picked} selected");
                                        let untag_hover = format!("Remove from {picked} selected");
                                        for k in &kws {
                                            // ⚠ Indented by depth — the only thing that makes the tree readable,
                                            // since the rows are a flat depth-first list.
                                            //
                                            // ⚠ `−` completes the existing mechanism rather than adding a second
                                            // one: `unassign_keyword` already existed and was tested. Both go
                                            // through one pair of functions the photo context menu also calls.
                                            let r = list_row::row(
                                                ui,
                                                RowSpec {
                                                    name: &k.name,
                                                    count: Some(k.count),
                                                    selected: picked_kw == Some(k.id),
                                                    indent: k.depth as f32 * 12.0,
                                                    hover: "",
                                                    actions: &[
                                                        RowAction {
                                                            glyph: '×',
                                                            verb: "Delete",
                                                            hover: "Delete, with everything under it",
                                                            enabled: true,
                                                        },
                                                        RowAction {
                                                            glyph: '+',
                                                            verb: "Tag",
                                                            hover: &tag_hover,
                                                            enabled: picked > 0,
                                                        },
                                                        RowAction {
                                                            glyph: '−',
                                                            verb: "Untag",
                                                            hover: &untag_hover,
                                                            enabled: picked > 0,
                                                        },
                                                    ],
                                                },
                                            );
                                            match r.action {
                                                Some(0) => {
                                                    action = Action::DeleteKeyword(k.id, k.name.clone())
                                                }
                                                Some(1) => action = Action::TagSelection(k.id),
                                                Some(_) => action = Action::UntagSelection(k.id),
                                                None if r.clicked => {
                                                    action = Action::OpenKeyword(k.id, k.name.clone())
                                                }
                                                None => {}
                                            }
                                        }
                                    },
                                )
                            }
                            PanelSection::Dates => {
                        section(ui, &mut sections.dates, "sec_dates", "Dates", "", |ui| {
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
                        })
                            }
                        };
                        tops.push(top);
                        if grip.drag_started() {
                            drag_started = Some(*sec);
                        }
                        if grip.drag_stopped() {
                            drag_released = true;
                        }
                    }

                    // ⚠ Painted after every section, so the line sits on top of them rather than
                    // under the next one.
                    if app.panel_drag.is_some() {
                        if let Some(y) = ui.ctx().pointer_interact_pos().map(|p| p.y) {
                            let at = rustroom_app::panel_order::drop_index(&tops, y);
                            drop_to = Some(at);
                            let line_y =
                                tops.get(at).copied().unwrap_or_else(|| ui.cursor().top());
                            ui.painter().hline(
                                ui.max_rect().x_range(),
                                line_y,
                                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                            );
                        }
                    }

                });
        });

    // ⚠ A drag begins on the frame a grip reports `drag_started`, and `app.panel_drag` is what
    // carries it across frames.
    if let Some(sec) = drag_started {
        app.panel_drag = Some(sec);
    }
    if drag_released {
        if let (Some(from), Some(to)) = (app.panel_drag, drop_to) {
            let order = app.settings.panel_order();
            if let Some(i) = order.iter().position(|s| *s == from) {
                let next = rustroom_app::panel_order::moved(&order, i, to);
                if next != order {
                    app.settings.panel_order = next;
                    // ⚠ Written when the drag **settles**, not per frame — the
                    // `apply_view_demosaic` rule, cited more than a dozen times in CLAUDE.md.
                    app.save_settings();
                }
            }
        }
        // ⚠ Cleared whatever happened, including a release with no drop position: a stuck
        // `panel_drag` would leave the insertion line painted forever.
        app.panel_drag = None;
    }

    if sections != app.settings.panel_sections {
        app.settings.panel_sections = sections;
        app.save_settings();
    }

    match action {
        Action::None => {}
        Action::Open(id, name) => app.open_collection(ctx, id, &name),
        Action::Delete(id) => {
            // ⚠ The snapshot is **pushed**, not discarded. `let _ =` here still compiles and
            // silently throws away the only record of what was deleted.
            if let Some(snap) = app
                .catalog
                .as_ref()
                .and_then(|c| c.delete_collection(id).ok())
            {
                let open = app.open_photo();
                app.actions.push(
                    rustroom_app::action_log::ActionKind::CatalogDelete {
                        snapshot: Box::new(snap),
                    },
                    open.as_ref(),
                );
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
            if let Some(snap) = app.catalog.as_ref().and_then(|c| c.delete_smart(id).ok()) {
                let open = app.open_photo();
                app.actions.push(
                    rustroom_app::action_log::ActionKind::CatalogDelete {
                        snapshot: Box::new(snap),
                    },
                    open.as_ref(),
                );
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
        Action::DeleteKeyword(id, name) => {
            let scope = app
                .catalog
                .as_ref()
                .and_then(|c| c.keyword_delete_scope(id).ok())
                .unwrap_or((0, 0));
            // ⚠⚠ Confirmed **only when the deletion removes more than the keyword itself.** A
            // leaf nobody has used is an unsurprising deletion, and a dialog for it is the false
            // alarm `banding_warning` already rejects: one that fires on ordinary practice trains
            // people to dismiss the one that matters.
            if scope.0 > 0 || scope.1 > 0 {
                app.modal = crate::modals::Modal::ConfirmDeleteKeyword {
                    id,
                    name,
                    descendants: scope.0,
                    assignments: scope.1,
                };
            } else {
                app.delete_keyword_now(id);
            }
        }
        Action::TagSelection(id) => app.assign_keyword_to_selection(id),
        Action::UntagSelection(id) => app.unassign_keyword_from_selection(id),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_ui;
    use eframe::egui;

    /// Run `section` for one frame with `events`, returning the `open` flag afterwards.
    fn click_title(events: Vec<egui::Event>, start_open: bool) -> bool {
        let ctx = test_ui::ctx();
        let mut open = start_open;
        // Two passes: the first lays the title out so the second can land a click on it.
        for pass in 0..2 {
            let ev = if pass == 1 {
                events.clone()
            } else {
                Vec::new()
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    events: ev,
                    ..Default::default()
                },
                |ui| {
                    super::section(ui, &mut open, "sec_test", "Folders", "", |ui| {
                        ui.label("body");
                    });
                },
            );
        }
        open
    }

    /// ⚠⚠ **The guard for a regression this shipped once.**
    /// `CollapsingState::show_header`'s own response comes from a plain `horizontal` scope, whose
    /// sense is `Sense::hover()` — so `clicked()` on it is *always false*, and the flag that
    /// persists the collapsed state silently stopped updating. Collapsing still looked correct
    /// because egui's arrow handles it in-session; only persistence was lost, which no screenshot
    /// would ever show.
    #[test]
    fn clicking_the_title_toggles_the_persisted_open_flag() {
        // The title sits near the top-left of the panel, right of the collapse arrow.
        let at = egui::pos2(60.0, 12.0);
        let events = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        assert!(
            !click_title(events, true),
            "clicking an open section's title must close it"
        );
    }

    /// ⚠ And nothing happens without a click — or the test above would pass on any frame at all.
    #[test]
    fn drawing_the_section_alone_does_not_toggle_it() {
        assert!(click_title(Vec::new(), true), "no click, no change");
    }

    /// ⚠⚠ **The arrow, which the title test above structurally cannot see — and which is the
    /// affordance a user actually reaches for.** It was broken and shipped: egui's own toggle
    /// flipped the visual state and never touched the flag, so a section collapsed by the arrow
    /// looked collapsed for the rest of the session and sprang open on the next launch, because
    /// nothing was ever persisted. Reported in use, 2026-09-01.
    ///
    /// ⚠ The two affordances now go through one path — the title toggles egui's state and the
    /// flag mirrors it — so this test and the title test are the two halves of one rule.
    #[test]
    fn clicking_the_arrow_also_persists_the_open_flag() {
        // The arrow is the leftmost thing in the header, left of the title.
        let at = egui::pos2(12.0, 12.0);
        let events = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        assert!(
            !click_title(events.clone(), true),
            "collapsing with the arrow must persist, not only change what is drawn"
        );
        assert!(
            click_title(events, false),
            "and expanding with it must persist too"
        );
    }
}

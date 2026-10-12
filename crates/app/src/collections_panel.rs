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
use crate::theme::Hover as _;
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
    let mut h = Header::new(title).draggable(true).rail(true);
    if !trailing.is_empty() {
        h = h.trailing(trailing);
    }
    let mut out = header(ui, state_id, *open, h);
    out.state.show_body_unindented(ui, body);
    *open = out.state.is_open();
    out.response
}

/// A name field with a "+" at its right edge — the rail's way to add a collection, a smart
/// collection or a keyword. Returns the field's response and whether "+" was clicked.
///
/// ⚠⚠ **Laid out at exactly the width it is given, the "+" first from the right and the field in
/// what is left.** The three rows used to size the field as "available width − 30" beside the
/// button — and when the design language widened button padding, the "+" outgrew those 30 points,
/// the row asked for more than the panel had, and egui widened the panel by the overflow **every
/// frame**: "the left sidecar keeps expanding indefinitely" (2026-09-29), the 2026-09-19 growth bug
/// again. A row that derives its parts from its own allocation cannot ask for more than it got.
///
/// ⚠ `extra` gates the "+" beyond a name being typed (Save this view needs an open source); the
/// caller still decides the commit, after the field has taken this frame's typing.
fn new_item_row(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    extra: bool,
    plus_hover: &str,
) -> (egui::Response, bool) {
    let named = !text.trim().is_empty();
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let plus = ui
                .add_enabled(named && extra, egui::Button::new("+"))
                .hover(plus_hover)
                .clicked();
            let field = ui.add_sized(
                egui::vec2(ui.available_width(), size.y),
                egui::TextEdit::singleline(text).hint_text(hint),
            );
            (field, plus)
        },
    )
    .inner
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
        RemoveSelection(i64),
        OpenKeyword(i64, String),
        DeleteKeyword(i64, String),
        TagSelection(i64),
        UntagSelection(i64),
        CreateKeyword(String),
        RenameKeyword(i64, String),
        MoveKeyword(i64, Option<i64>),
        OpenAllPhotos,
        OpenFolder(std::path::PathBuf),
        ForgetFolders(Vec<std::path::PathBuf>),
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
    // ⚠ Its own variable rather than another `Action` variant: the Immich section's outcomes are
    // enqueued jobs, not catalog operations, and folding them into the catalog `Action` enum would
    // put two unrelated concerns behind one match.
    let mut immich_action = crate::immich_panel::Action::None;
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

    // M10.8b: on the side the View menu put it.
    crate::side_panel::side(app.settings.layout.library_rail, "collections")
        .default_size(240.0)
        .min_size(200.0)
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
                                            meta: "",
                                            selected: showing_all,
                                            indent: 0.0,
                                            hover: "Shows every photo in the catalog, from every folder you have opened",
                                            actions: &[],
                                        },
                                    );
                                    if r.clicked {
                                        action = Action::OpenAllPhotos;
                                    }
                                    // ⚠ Once, not per row.
                                    let immich_root = app.immich_cache_root();
                                    let open = match app.library.as_ref().map(|l| &l.source) {
                                        Some(crate::LibrarySource::Folder(p)) => Some(p.clone()),
                                        _ => None,
                                    };
                                    // ⚠ M9.9: the tree — `folder_rows` draws it, `folder_tree`
                                    // builds it.
                                    match crate::folder_rows::rows(
                                        ui,
                                        crate::folder_rows::View {
                                            folders: &folders,
                                            immich_root: immich_root.as_deref(),
                                            immich_count: app.immich_present.len() as i64,
                                            open: open.as_deref(),
                                        },
                                        &mut app.folders_collapsed,
                                    ) {
                                        Some(crate::folder_rows::Out::Open(p)) => {
                                            action = Action::OpenFolder(p)
                                        }
                                        Some(crate::folder_rows::Out::Forget(paths)) => {
                                            action = Action::ForgetFolders(paths)
                                        }
                                        None => {}
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
                                        let (field, plus) = new_item_row(
                                            ui,
                                            &mut app.new_collection,
                                            "New collection",
                                            true,
                                            "Create a collection with this name",
                                        );
                                        let named = !app.new_collection.trim().is_empty();
                                        if named && (plus || theme::committed(ui, &field)) {
                                            if let Some(cat) = &app.catalog {
                                                let _ = cat.create_collection(app.new_collection.trim());
                                            }
                                            app.new_collection.clear();
                                        }
                                        if rows.is_empty() {
                                            crate::theme::hint(ui, "No collections yet.");
                                        }
                                        // ⚠ Enabled only with a selection, and the hover names the count, so it
                                        // can never be ambiguous about what it is about to do.
                                        let add_hover = format!("Add {picked} selected");
                                        let remove_hover = format!("Remove {picked} selected");
                                        for c in &rows {
                                            let r = list_row::row(
                                                ui,
                                                RowSpec {
                                                    name: &c.name,
                                                    count: Some(c.count),
                                                    meta: "",
                                                    selected: selected == Some(c.id),
                                                    indent: 0.0,
                                                    hover: "Shows the photos in this collection",
                                                    actions: &[
                                                        RowAction {
                                                            glyph: '×',
                                                            verb: "Delete",
                                                            hover: "Deletes this collection",
                                                            enabled: true,
                                                        },
                                                        RowAction {
                                                            glyph: '+',
                                                            verb: "Add to",
                                                            hover: &add_hover,
                                                            enabled: picked > 0,
                                                        },
                                                        RowAction {
                                                            glyph: '−',
                                                            verb: "Remove from",
                                                            hover: &remove_hover,
                                                            enabled: picked > 0,
                                                        },
                                                    ],
                                                },
                                            );
                                            match r.action {
                                                Some(0) => action = Action::Delete(c.id),
                                                Some(1) => action = Action::AddSelection(c.id),
                                                Some(_) => action = Action::RemoveSelection(c.id),
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
                                        let has_library = app.library.is_some();
                                        let (field, plus) = new_item_row(
                                            ui,
                                            &mut app.new_smart,
                                            "Save this view",
                                            has_library,
                                            "Save the current source and filter as a smart collection",
                                        );
                                        let ready = !app.new_smart.trim().is_empty() && has_library;
                                        if ready && (plus || theme::committed(ui, &field)) {
                                            action = Action::SaveSmart;
                                        }
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
                                                    meta: "",
                                                    selected,
                                                    indent: 0.0,
                                                    hover: "Shows the photos these rules find",
                                                    actions: &[
                                                        RowAction {
                                                            glyph: '×',
                                                            verb: "Delete",
                                                            hover: "Deletes this smart collection",
                                                            enabled: true,
                                                        },
                                                        RowAction {
                                                            glyph: icons::GEAR,
                                                            verb: "Edit",
                                                            hover: "Edits this smart collection's rule",
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
                                        // ⚠⚠ M10.8a: type to tag — `keyword_field`. The parent of a
                                        // new keyword is the one last CLICKED; a click anywhere else
                                        // clears it (below), and the hint says which.
                                        let parent = app
                                            .new_keyword_parent
                                            .and_then(|id| kws.iter().find(|k| k.id == id))
                                            .map(|k| k.name.clone());
                                        match crate::keyword_field::field(
                                            ui,
                                            &mut app.new_keyword,
                                            &mut app.keyword_field,
                                            &kws,
                                            picked,
                                            parent.as_deref(),
                                        ) {
                                            Some(crate::keyword_field::FieldOut::Tag(id)) => {
                                                action = Action::TagSelection(id)
                                            }
                                            Some(crate::keyword_field::FieldOut::Create(name)) => {
                                                action = Action::CreateKeyword(name)
                                            }
                                            Some(crate::keyword_field::FieldOut::Open(id, name)) => {
                                                app.new_keyword_parent = Some(id);
                                                action = Action::OpenKeyword(id, name)
                                            }
                                            None => {}
                                        }
                                        if kws.is_empty() {
                                            crate::theme::hint(ui, "No keywords yet.");
                                        }
                                        // M10.8a: the selection's suggestions, between the field
                                        // and the tree. ⚠ Cached — the rail redraws every frame.
                                        let files = app.selected_files();
                                        let suggested = match &app.catalog {
                                            Some(cat) if !files.is_empty() => {
                                                app.keyword_suggest.get(cat, &files).to_vec()
                                            }
                                            _ => Vec::new(),
                                        };
                                        match crate::keyword_rows::suggested(
                                            ui, &suggested, &kws, picked,
                                        ) {
                                            Some(crate::keyword_rows::Out::Tag(id)) => {
                                                action = Action::TagSelection(id)
                                            }
                                            Some(crate::keyword_rows::Out::Open(id, name)) => {
                                                app.new_keyword_parent = Some(id);
                                                action = Action::OpenKeyword(id, name)
                                            }
                                            _ => {}
                                        }
                                        let picked_kw = app.selected_keyword();
                                        // ⚠ M9.8: the rows, their rename field and their drag live in
                                        // `keyword_rows`; this maps what they asked for onto `Action`.
                                        match crate::keyword_rows::rows(
                                            ui,
                                            &kws,
                                            picked,
                                            picked_kw,
                                            &mut app.keyword_rename,
                                        ) {
                                            Some(crate::keyword_rows::Out::Open(id, name)) => {
                                                app.new_keyword_parent = Some(id);
                                                action = Action::OpenKeyword(id, name)
                                            }
                                            Some(crate::keyword_rows::Out::Delete(id, name)) => {
                                                action = Action::DeleteKeyword(id, name)
                                            }
                                            Some(crate::keyword_rows::Out::Tag(id)) => {
                                                action = Action::TagSelection(id)
                                            }
                                            Some(crate::keyword_rows::Out::Untag(id)) => {
                                                action = Action::UntagSelection(id)
                                            }
                                            Some(crate::keyword_rows::Out::Rename(id, name)) => {
                                                action = Action::RenameKeyword(id, name)
                                            }
                                            Some(crate::keyword_rows::Out::Move(id, parent)) => {
                                                action = Action::MoveKeyword(id, parent)
                                            }
                                            None => {}
                                        }
                                        // A click anywhere outside this list — the field, the rows
                                        // and the drop row — clears the parent. ⚠ Read from the
                                        // context's input, so a click on the grid or another panel
                                        // counts, not only one inside this section.
                                        let list = ui.min_rect();
                                        let outside = ui.input(|i| {
                                            i.pointer.primary_clicked()
                                                && i.pointer
                                                    .interact_pos()
                                                    .is_some_and(|p| !list.contains(p))
                                        });
                                        if outside {
                                            app.new_keyword_parent = None;
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
                            // ⚠ Rows with their counts in the rail's column, not collapsing headers — `date_rows`.
                            if let Some((period, mods)) = crate::date_rows::rows(ui, &days, &open_periods) {
                                action = Action::OpenPeriod(period, mods);
                            }
                        })
                            }
                            PanelSection::Immich => {
                                // ⚠⚠ **Not drawn at all when the integration is off** — the
                                // requirement is "no mention of immich at all", and a disabled or
                                // empty section is still a mention. ⚠ This also removes the empty
                                // "Immich" section a user who never configured it used to see: the
                                // gate is `connection()`, which is `None` when unconfigured too.
                                if rustroom_app::immich_source::connection(&app.settings).is_none() {
                                    // ⚠⚠ **Push the slot rather than just `continue`ing.** `tops` is
                                    // index-aligned with `order` and feeds `panel_order::drop_index`,
                                    // whose result indexes **`order`** when a drag settles. A bare
                                    // `continue` shortened `tops` and put every drop one slot off
                                    // past this section — caught while writing this, not by a test.
                                    // A zero-height slot is indistinguishable from dropping on the
                                    // next section, which is the right behaviour for an absent one.
                                    tops.push(top);
                                    continue;
                                }
                                // ⚠ No count on the header, as for Dates: the Timeline and Albums
                                // rows carry their own (M10.6a).
                                section(ui, &mut sections.immich, "sec_immich", "Immich", "", |ui| {
                                    immich_action = crate::immich_panel::body(app, ui);
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

                    // ⚠⚠ The invariant a hidden section can break: `drop_index` returns an index
                    // into `tops`, and `moved` applies it to `order`. If a section is skipped without
                    // pushing its slot, every drop past it lands one place off.
                    debug_assert_eq!(
                        tops.len(),
                        order.len(),
                        "tops must stay index-aligned with the section order"
                    );

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

    // ⚠ Applied here, after the panel's closures have released their borrow of `app` — the shape
    // every panel in this file uses.
    match immich_action {
        crate::immich_panel::Action::None => {}
        crate::immich_panel::Action::Refresh => app.request_immich_albums(),
        crate::immich_panel::Action::Open { id, name } => {
            // ⚠⚠ Already fetched this session: open it now rather than waiting a couple of seconds
            // for the same reply again. This is what makes switching back and forth feel instant.
            if let Some(assets) = app.immich_assets.get(&id).cloned() {
                app.open_immich_album(ctx, id, name, &assets);
            } else if let Some(assets) = app.immich_cached_album(&id) {
                // ⚠ On disk from an earlier run, and its validator still matches what the rail's
                // album list says — so the *first* visit after a launch is instant too.
                app.immich_assets.insert(id.clone(), assets.clone());
                app.open_immich_album(ctx, id, name, &assets);
            } else {
                // ⚠ Feedback **before** the request, or the click looks like it did nothing: the
                // rail's highlight follows `library.source`, which will not change for seconds.
                app.immich_opening = Some(id.clone());
                app.status = format!("Loading “{name}” from Immich…");
                app.immich
                    .request(rustroom_app::immich_worker::Job::Album { id, name });
            }
        }
        // ⚠⚠ The asset list is needed to know *which* files to act on, and the rail may not have it.
        // Reuse the click path's own three steps — session map, then the stored list, then a fetch —
        // rather than inventing a fourth way to get one. ⚠ If it has to be fetched, say so instead of
        // silently doing nothing; the user pressed a button.
        crate::immich_panel::Action::DownloadAlbum { id, name } => {
            match app.immich_album_assets(&id) {
                Some(assets) => {
                    let paths = app.immich_paths_for(&assets);
                    app.download_assets(&paths);
                }
                None => {
                    app.status =
                        format!("Opening “{name}” first — press Download again once it loads");
                    app.immich_opening = Some(id.clone());
                    app.immich
                        .request(rustroom_app::immich_worker::Job::Album { id, name });
                }
            }
        }
        crate::immich_panel::Action::RemoveAlbum { id, name } => {
            match app.immich_album_assets(&id) {
                Some(assets) => {
                    let paths = app.immich_paths_for(&assets);
                    app.remove_downloaded(&paths);
                }
                None => {
                    app.status = format!(
                        "Opening “{name}” first — press Remove downloads again once it loads"
                    );
                    app.immich_opening = Some(id.clone());
                    app.immich
                        .request(rustroom_app::immich_worker::Job::Album { id, name });
                }
            }
        }
        crate::immich_panel::Action::RefreshBuckets => {
            app.immich
                .request(rustroom_app::immich_worker::Job::Buckets);
        }
        crate::immich_panel::Action::OpenBucket { bucket, label } => {
            if let Some(assets) = app.immich_assets.get(&bucket).cloned() {
                app.open_immich_bucket(ctx, bucket, label, &assets);
            } else if let Some(assets) = app.immich_cached_bucket(&bucket) {
                app.immich_assets.insert(bucket.clone(), assets.clone());
                app.open_immich_bucket(ctx, bucket, label, &assets);
            } else {
                app.immich_opening = Some(bucket.clone());
                app.status = format!("Loading {label} from Immich…");
                app.immich
                    .request(rustroom_app::immich_worker::Job::Bucket { bucket, label });
            }
        }
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
        Action::AddSelection(id) => app.change_selection_in_collection(id, true),
        Action::RemoveSelection(id) => app.change_selection_in_collection(id, false),
        Action::CreateKeyword(name) => app.create_keyword_and_tag(&name),
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
        Action::ForgetFolders(paths) => app.ask_forget_folders(paths),
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
        Action::RenameKeyword(id, name) => app.rename_keyword(id, &name),
        Action::MoveKeyword(id, parent) => app.move_keyword(id, parent),
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

    /// ⚠⚠ **Reported in use 2026-09-29: "when moving the mouse in the Library the left sidecar keeps
    /// expanding indefinitely."** Every frame the pointer moves is a frame drawn, and a row that asks
    /// for more than the panel's width makes egui widen the panel by the overflow — then the row asks
    /// for that much more. Drawn with the **real theme** (the bare harness has egui's narrower button
    /// padding, under which the rows fitted), every section open, the pointer moving; the width must
    /// settle. ⚠ Sample first: the panel must actually have been drawn and measured.
    #[test]
    fn the_rail_keeps_its_width_while_the_pointer_moves() {
        let ctx = crate::test_ui::ctx();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let mut app = crate::test_app::for_test();
        let s = &mut app.settings.panel_sections;
        s.folders = true;
        s.collections = true;
        s.smart = true;
        s.keywords = true;
        s.dates = true;
        s.immich = true;
        // The Immich section too, with the integration on — it draws rows and actions of its own.
        app.settings.immich_enabled = true;
        app.settings.immich_url = "https://immich.example".to_string();
        app.settings.immich_key = "a-key".to_string();
        app.catalog
            .as_ref()
            .unwrap()
            .create_keyword("Europe", None)
            .unwrap();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 875.0));
        let mut widths = Vec::new();
        for frame in 0..30 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(egui::pos2(
                        120.0 + frame as f32,
                        300.0,
                    ))],
                    ..Default::default()
                },
                |ui| super::show(&mut app, ui, &ctx),
            );
            if let Some(st) =
                egui::containers::panel::PanelState::load(&ctx, egui::Id::new("collections"))
            {
                widths.push(st.size().x);
            }
        }
        assert!(
            widths.len() > 20,
            "the panel was never measured: {widths:?}"
        );
        let settled = widths[5];
        assert!(settled > 100.0, "sample: {settled}");
        assert!(
            widths.iter().skip(5).all(|w| (w - settled).abs() < 0.5),
            "the rail grew while the pointer moved: {widths:?}"
        );
    }

    /// The new keyword's parent is the keyword last clicked, and a click anywhere else clears it —
    /// the author's replacement for the parent dropdown. ⚠ Both halves, and the clear is asserted
    /// from a click OUTSIDE the panel's list, which is the case the rule is about.
    #[test]
    fn clicking_a_keyword_makes_it_the_parent_and_clicking_elsewhere_clears_it() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.settings.panel_sections.keywords = true;
        let europe = app
            .catalog
            .as_ref()
            .unwrap()
            .create_keyword("Europe", None)
            .unwrap();
        crate::test_ui::click(&ctx, "Europe", |ui| super::show(&mut app, ui, &ctx));
        assert_eq!(
            app.new_keyword_parent,
            Some(europe),
            "the click must set the parent"
        );
        let far = egui::pos2(390.0, 1390.0);
        crate::test_ui::click_at(&ctx, far, false, |ui| super::show(&mut app, ui, &ctx));
        assert_eq!(
            app.new_keyword_parent, None,
            "a click elsewhere must clear it"
        );
    }

    /// ⚠⚠ **Reported in use 2026-09-29:** "the numbers indicating the amount of photos are not in
    /// one column" — a folder's count stood left of its one action, an Immich album's left of its
    /// two, a header's at the edge, and the Dates' inside their labels. Every count the rail paints
    /// must end at one x. Found as painted text, which is what the eye sees.
    ///
    /// ⚠ The sample: counts from a header, a one-action row, a two-action row, a no-action row and
    /// a tree row, or "one x" could be the same kind of row five times.
    #[test]
    fn every_count_in_the_rail_ends_at_one_x() {
        let ctx = crate::test_ui::ctx();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(3));
        crate::test_app::catalog_library(&mut app);
        {
            let cat = app.catalog.as_ref().unwrap();
            let first = app.library.as_ref().unwrap().entries[0].id.path.clone();
            // 2026-05-17, as days since the epoch — the catalog's own fixture date.
            cat.set_capture(
                &first,
                &core_image::Capture {
                    taken: Some(20_590 * 86_400),
                    ..Default::default()
                },
            )
            .unwrap();
            cat.create_collection("Trips").unwrap();
            cat.create_keyword("Europe", None).unwrap();
        }
        let s = &mut app.settings.panel_sections;
        s.folders = true;
        s.collections = true;
        s.smart = true;
        s.keywords = true;
        s.dates = true;
        s.immich = true;
        app.settings.immich_enabled = true;
        app.settings.immich_url = "https://immich.example".to_string();
        app.settings.immich_key = "a-key".to_string();
        app.immich_albums = Some(Ok(vec![immich::AlbumSummary {
            id: "a1".to_string(),
            album_name: "Holiday".to_string(),
            asset_count: 1234,
            last_modified_asset_timestamp: None,
        }]));
        app.immich_buckets = Some(Ok(vec![immich::Bucket {
            time_bucket: "2025-03-01T00:00:00.000Z".to_string(),
            count: 57,
        }]));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 1600.0));
        let draw = |app: &mut crate::RustroomApp| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| super::show(app, ui, &ctx),
            )
        };
        // Open the Immich timeline, so its years are drawn too.
        let _ = draw(&mut app);
        crate::test_ui::click(&ctx, "Timeline", |ui| super::show(&mut app, ui, &ctx));
        let _ = draw(&mut app);
        let out = draw(&mut app);

        fn walk(shape: &egui::Shape, found: &mut Vec<(String, f32)>) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, found)),
                egui::Shape::Text(t) => {
                    let text = t.galley.text();
                    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
                        found.push((text.to_owned(), t.pos.x + t.galley.size().x));
                    }
                }
                _ => {}
            }
        }
        let mut counts = Vec::new();
        for c in &out.shapes {
            walk(&c.shape, &mut counts);
        }
        let texts: Vec<&str> = counts.iter().map(|(t, _)| t.as_str()).collect();
        // 1234: the album, two actions. 3: the folder, one. 57: the Immich year, a tree row.
        // 2026 and 2025 are the two years' NAMES, painted at the left, and are left out below —
        // asserted present first, so the filter is known to remove something real.
        for want in ["1234", "3", "57", "2026", "2025"] {
            assert!(
                texts.contains(&want),
                "sample: {want} was not painted: {texts:?}"
            );
        }
        let column: Vec<&(String, f32)> = counts
            .iter()
            .filter(|(t, _)| t != "2026" && t != "2025")
            .collect();
        assert!(column.len() >= 6, "sample: too few counts: {column:?}");
        let x = column[0].1;
        assert!(
            column.iter().all(|(_, r)| (r - x).abs() < 0.5),
            "the counts do not end at one x: {column:?}"
        );
    }

    /// ⚠⚠ **The requirement as a test rather than a list of places to remember** — with the
    /// integration off, the panel must not offer the Immich **section**.
    ///
    /// ⚠⚠ **One deliberate exception, and it is the user revising their own rule (2026-09-24):** the
    /// downloaded folder keeps the name **"immich (downloaded)"** whether or not the integration is
    /// on, because those are their photographs and they know where they came from. So this asserts
    /// the *section* is gone and that nothing else mentions it — not that the word is absent
    /// entirely, which would be a claim the design no longer makes.
    ///
    /// ⚠ Both halves: present when configured, absent when off. An absence alone would pass against
    /// a panel that drew nothing.
    #[test]
    fn the_immich_section_appears_only_when_the_integration_is_on() {
        let labels = |enabled: bool| -> Vec<String> {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            app.settings.immich_enabled = enabled;
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "a-key".to_string();
            crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                super::show(&mut app, ui, &ctx);
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect()
        };

        let on = labels(true);
        assert!(
            on.iter().any(|l| l == "Immich"),
            "the section must be there when configured and enabled: {on:?}"
        );

        let off = labels(false);
        // ⚠ The sample assertion: the panel really drew, so the absence below means something.
        assert!(
            off.iter().any(|l| l == "Folders"),
            "the panel did not draw at all: {off:?}"
        );
        assert!(
            !off.iter().any(|l| l == "Immich"),
            "the Immich section is still offered with it switched off: {off:?}"
        );
        // ⚠ Nothing *else* may mention it either — the downloaded folder's row is the one allowed
        // name, and it is not drawn by this fixture, so anything matching here is a leak.
        let leaked: Vec<&String> = off
            .iter()
            .filter(|l| l.to_lowercase().contains("immich"))
            .filter(|l| *l != "immich (downloaded)")
            .collect();
        assert!(
            leaked.is_empty(),
            "these labels still mention Immich with it switched off: {leaked:?}"
        );
    }

    /// ⚠⚠ **The bug the user reported as "the name still changes", and the rename was NOT the
    /// cause.** `immich_cache_root` went through `connection`, so switching Immich off made it
    /// `None` — and `folder_display_name(path, None)` falls through to the directory's basename, so
    /// the Folders row started showing the raw host slug `https___…` instead of
    /// "immich (downloaded)". A path is not a behaviour, so it is derived from the URL alone.
    #[test]
    fn the_downloaded_folder_keeps_its_name_with_immich_switched_off() {
        let name_for = |enabled: bool| -> String {
            let mut app = crate::test_app::for_test();
            app.settings.immich_enabled = enabled;
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "a-key".to_string();
            let root = app
                .immich_cache_root()
                .expect("the cache root is a path, and the URL is set");
            rustroom_app::immich_catalog::folder_display_name(&root, Some(root.as_path()))
        };
        assert_eq!(name_for(true), "immich (downloaded)");
        assert_eq!(
            name_for(false),
            "immich (downloaded)",
            "switching Immich off must not rename the folder"
        );
    }
}

//! The Library rail's Publish services section (M10.18d): one row a service — click to open its view, × to
//! delete, + and − to add or take out the selection (the Collections idiom) — a right-click to edit it, and
//! New service… at the top.
//!
//! ⚠ Draws from `RailData::publish`, read once per catalog change — never a query per frame.

use crate::list_row::{self, RowAction, RowSpec};
use crate::theme::Hover as _;
use crate::RustroomApp;
use eframe::egui;

/// What a click in the section asks for, applied after the panel closes.
pub(crate) enum Out {
    New,
    Open(i64, String),
    Edit(i64),
    Delete(i64, String),
    Add(i64),
    Remove(i64),
}

/// The section's body.
pub(crate) fn body(app: &RustroomApp, ui: &mut egui::Ui) -> Option<Out> {
    let mut out = None;
    if ui
        .button("New service…")
        .hover("Makes a service that keeps a folder or an Immich album in step with its photos")
        .clicked()
    {
        out = Some(Out::New);
    }
    let rows = std::sync::Arc::clone(&app.rail_data.publish);
    if rows.is_empty() {
        crate::theme::hint(ui, "No publish services yet.");
    }
    let picked = app.library.as_ref().map_or(0, |l| l.selection.len());
    let shown = app.shown_service();
    let add_hover = format!("Add {picked} selected");
    let remove_hover = format!("Take {picked} selected out");
    for s in rows.iter() {
        let r = list_row::row(
            ui,
            RowSpec {
                name: &s.name,
                count: Some(s.count),
                meta: "",
                selected: shown == Some(s.id),
                indent: 0.0,
                hover: "Shows this service's photos and what is waiting to be published",
                actions: &[
                    RowAction {
                        glyph: '×',
                        verb: "Delete",
                        hover: "Deletes this service, leaving what it published",
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
                        verb: "Take out of",
                        hover: &remove_hover,
                        enabled: picked > 0,
                    },
                ],
            },
        );
        r.response.context_menu(|ui| {
            if ui
                .button("Edit…")
                .hover("Changes the service's name, destination or export settings")
                .clicked()
            {
                out = Some(Out::Edit(s.id));
                ui.close();
            }
        });
        match r.action {
            Some(0) => out = Some(Out::Delete(s.id, s.name.clone())),
            Some(1) => out = Some(Out::Add(s.id)),
            Some(_) => out = Some(Out::Remove(s.id)),
            None if r.clicked => out = Some(Out::Open(s.id, s.name.clone())),
            None => {}
        }
    }
    out
}

/// Apply what the section asked for.
pub(crate) fn apply(app: &mut RustroomApp, ctx: &egui::Context, out: Out) {
    match out {
        Out::New => app.open_new_service(),
        Out::Open(id, name) => app.open_publish(ctx, id, &name),
        Out::Edit(id) => app.open_edit_service(id),
        Out::Delete(id, name) => {
            app.modal = crate::modals::Modal::ConfirmDeleteService { id, name };
        }
        Out::Add(id) => app.service_selection(id, true),
        Out::Remove(id) => app.service_selection(id, false),
    }
}

/// A cell's publish mark (M10.18d), over the picture's top-left corner: a filled dot for New, a ring for
/// Modified — the accent, which is the selection's colour — and Removed dimmed under the panel's colour.
pub(crate) fn cell_mark(ui: &egui::Ui, picture: egui::Rect, s: rustroom_app::publish_state::State) {
    use rustroom_app::publish_state::State;
    let accent = ui.visuals().selection.bg_fill;
    let at = picture.left_top() + egui::vec2(MARK_INSET, MARK_INSET);
    let p = ui.painter();
    match s {
        State::New => {
            p.circle_filled(at, MARK_RADIUS, accent);
        }
        State::Modified => {
            p.circle_stroke(at, MARK_RADIUS - 1.0, egui::Stroke::new(2.0, accent));
        }
        State::Removed => {
            p.rect_filled(
                picture,
                0.0,
                crate::theme::tokens::of(ui).panel.gamma_multiply(0.7),
            );
        }
        State::Published => {}
    }
}

/// The mark's radius and its inset from the picture's corner.
const MARK_RADIUS: f32 = 6.0;
const MARK_INSET: f32 = 10.0;

/// The status row's Publish controls in a service's view, drawn right to left: **Publish N**, then what is
/// waiting.
pub(crate) fn status_controls(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let Some(c) = app.publish.view.as_ref().map(|v| v.counts) else {
        return;
    };
    let running = app.publish_line().is_some();
    let label = match c.to_publish() {
        0 => "Publish".to_string(),
        n => format!("Publish {n}"),
    };
    if ui
        .add_enabled(c.has_work() && !running, egui::Button::new(label))
        .hover("Exports the new and modified photos and clears the removed ones")
        .clicked()
    {
        app.start_publish();
    }
    ui.label(egui::RichText::new(c.summary()).color(crate::theme::tokens::of(ui).text2))
        .hover("New photos have a dot, modified ones a ring, removed ones are dimmed");
}

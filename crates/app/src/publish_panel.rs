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
/// Modified — the accent, which is the selection's colour, each in a dark rim — and Removed dimmed under the panel's colour.
pub(crate) fn cell_mark(ui: &egui::Ui, picture: egui::Rect, s: rustroom_app::publish_state::State) {
    use rustroom_app::publish_state::State;
    let accent = ui.visuals().selection.bg_fill;
    let rim = crate::theme::mark_rim();
    let at = picture.left_top() + egui::vec2(MARK_INSET, MARK_INSET);
    let p = ui.painter();
    match s {
        State::New => {
            p.circle(at, MARK_RADIUS, accent, egui::Stroke::new(1.5, rim));
        }
        State::Modified => {
            // The rim is a wider stroke under the ring, so it shows a dark edge inside and out.
            p.circle_stroke(at, MARK_RADIUS - 1.0, egui::Stroke::new(4.0, rim));
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

/// A service view's band above the grid (M10.18d, the author's call over the status row): the service's
/// name and what is waiting at the left, **Publish N** at the right. Nothing outside a service's view.
pub(crate) fn band(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let Some(c) = app.publish.view.as_ref().map(|v| v.counts) else {
        return;
    };
    let name = match app.library.as_ref().map(|l| &l.source) {
        Some(crate::LibrarySource::Publish { name, .. }) => name.clone(),
        _ => return,
    };
    let running = app.publish_line().is_some();
    let label = match c.to_publish() {
        0 => "Publish".to_string(),
        n => format!("Publish {n}"),
    };
    egui::Panel::top("publish_band")
        .exact_size(crate::status_bar::HEIGHT)
        .resizable(false)
        .frame(crate::status_bar::frame(ui.style()))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(name).strong());
                ui.label(
                    egui::RichText::new(c.summary()).color(crate::theme::tokens::of(ui).text2),
                )
                .hover("New photos have a dot, modified ones a ring, removed ones are dimmed");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::theme::primary_button(ui, &label, c.has_work() && !running)
                        .hover("Exports the new and modified photos and clears the removed ones")
                        .clicked()
                    {
                        app.start_publish();
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use rustroom_app::publish_state::State;

    /// The circles `cell_mark` paints for `s`.
    fn circles(s: State) -> Vec<eframe::epaint::CircleShape> {
        let ctx = egui::Context::default();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 80.0));
            super::cell_mark(ui, r, s);
        });
        out.shapes
            .into_iter()
            .filter_map(|c| match c.shape {
                egui::Shape::Circle(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    use eframe::egui;

    /// The dot and the ring each carry a dark rim, so they read on a light thumbnail (the author, 2026-11-02).
    #[test]
    fn the_new_and_modified_marks_have_a_dark_rim() {
        for s in [State::New, State::Modified] {
            let c = circles(s);
            assert!(!c.is_empty(), "{s:?} drew no circle");
            assert!(
                c.iter()
                    .any(|c| c.stroke.color == crate::theme::mark_rim() && c.stroke.width > 0.0),
                "{s:?} has no dark rim: {c:?}"
            );
        }
        assert!(circles(State::Published).is_empty());
    }
}

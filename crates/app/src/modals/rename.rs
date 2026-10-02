//! Photo → Rename… (M10.8d): the template, the start number, and a preview of the first new names.

use super::{dialog, Footer, Modal};
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::batch_rename;
use rustroom_app::filename_template::{self, Template};
use rustroom_app::photo_move::Planned;
use rustroom_app::status_text::photos as count;

/// How many photos the preview names before "and N more".
const PREVIEW_ROWS: usize = 3;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::Rename {
        photos,
        left_out,
        template,
        start,
        planned,
    } = &mut app.modal
    else {
        return;
    };
    let photos = std::mem::take(photos);
    let left_out = *left_out;
    let mut template = std::mem::take(template);
    let mut start = *start;
    let mut planned = planned.take();

    // ⚠ Planned again only when the template or the start number changed — see `Modal::Rename`.
    let parsed = Template::parse_for_rename(&template);
    if let Ok(t) = &parsed {
        let current = planned.as_ref().map(|(s, n, _)| (s.as_str(), *n));
        if current != Some((template.as_str(), start)) {
            let cat = app.catalog.as_ref();
            let plan =
                batch_rename::plan(&photos, t, start, |p| crate::move_flow::path_taken(cat, p));
            planned = Some((template.clone(), start, plan));
        }
    }
    let plan: &[Planned] = match (&parsed, &planned) {
        (Ok(_), Some((_, _, p))) => p,
        _ => &[],
    };
    let ok = parsed.is_ok() && !plan.is_empty();
    let note = if left_out > 0 {
        format!(
            "{} left out — Immich photos keep the server's names",
            count(left_out)
        )
    } else {
        String::new()
    };
    let (lines, more) = batch_rename::preview(plan, PREVIEW_ROWS);

    let out = dialog(
        ctx,
        &mut app.settings,
        "rename_photos",
        "Rename photos",
        theme::DialogSize::Standard,
        Footer::confirm(
            "Rename",
            "Renames these photos' files on disk",
            ok,
            "Cancel",
        )
        .note(&note),
        |ui| {
            egui::Grid::new("rename_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Template");
                    ui.add(egui::TextEdit::singleline(&mut template).desired_width(f32::INFINITY))
                        .hover(filename_template::rename_token_hover());
                    ui.end_row();
                    ui.label("Start at");
                    ui.add(egui::DragValue::new(&mut start).range(1..=999_999))
                        .hover("The number the first photo's {seq} takes");
                    ui.end_row();
                });
            match &parsed {
                Err(e) => theme::error_hint(ui, &e.to_string()),
                Ok(_) if lines.is_empty() => {
                    theme::hint(ui, "Every photo already has the name this gives")
                }
                Ok(_) => {
                    for (from, to) in &lines {
                        ui.label(format!("{from} → {to}"));
                    }
                    if more > 0 {
                        theme::hint(ui, &format!("and {} more", count(more)));
                    }
                }
            }
        },
    );

    if out.primary && ok {
        app.modal = Modal::None;
        app.rename_photos(ctx, &photos, &template, start, left_out);
    } else if out.dismissed {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::Rename {
            photos,
            left_out,
            template,
            start,
            planned,
        };
    }
}

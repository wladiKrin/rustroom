//! Asking before downloading an AI model (M9.13): every file, its size and its licence, then the
//! progress. Nothing is fetched until *Download* is pressed.

use super::{dialog, Footer};
use crate::theme;
use crate::RustroomApp;
use eframe::egui;
use std::sync::atomic::Ordering;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let proposed = app.ai.proposed.clone();
    let downloading = app
        .ai
        .download
        .as_ref()
        .map(|d| (d.received.load(Ordering::Relaxed), d.total, d.error.clone()));
    let busy = downloading.as_ref().is_some_and(|(_, _, e)| e.is_none());
    let total: u64 = proposed.iter().map(|m| m.spec().bytes).sum();
    let primary = format!("Download {:.0} MB", total as f64 / 1e6);
    let out = dialog(
        ctx,
        &mut app.settings,
        "ai_download",
        "AI masks need a model",
        theme::DialogSize::Standard,
        Footer::confirm(&primary, !busy, if busy { "Cancel" } else { "Not now" }),
        |ui| {
            theme::hint(
                ui,
                "These files are downloaded once from GitHub, checked against a pinned hash, and \
                 kept for every photo. Everything runs on this computer — no photo leaves it.",
            );
            ui.add_space(6.0);
            egui::Grid::new("ai_models")
                .num_columns(3)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for m in &proposed {
                        let s = m.spec();
                        ui.label(s.file);
                        ui.label(format!("{:.0} MB", s.bytes as f64 / 1e6));
                        ui.hyperlink_to(s.licence, s.licence_url);
                        ui.end_row();
                    }
                });
            if let Some((got, of, error)) = &downloading {
                ui.add_space(8.0);
                match error {
                    Some(e) => theme::error_hint(ui, e),
                    None => {
                        ui.add(
                            egui::ProgressBar::new(*got as f32 / (*of).max(1) as f32).text(
                                format!("{:.0} of {:.0} MB", *got as f64 / 1e6, *of as f64 / 1e6),
                            ),
                        );
                    }
                }
            }
        },
    );
    if out.primary {
        app.start_ai_download();
    } else if out.dismissed {
        app.close_ai_download();
    }
}

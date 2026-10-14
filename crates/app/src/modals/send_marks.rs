//! Confirming a send of marks to Immich, and its undo (M10.15d).
//!
//! ⚠⚠ **These two buttons are the only way a send reaches the server.** The send's dialog lists every
//! photo that would change, one line each, with the lines that replace a mark Immich already had in
//! the warning colour; the summary counts them too, so the colour is never the only signal.

use super::{dialog, Footer, Modal};
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::immich_send;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::SendMarks { changes } = &app.modal else {
        return;
    };
    let changes = changes.clone();
    let summary = immich_send::summary(&changes);
    let out = dialog(
        ctx,
        &mut app.settings,
        "send_marks",
        "Send marks to Immich",
        theme::DialogSize::Standard,
        Footer::confirm(
            "Send",
            "Writes these marks to your Immich server",
            true,
            "Cancel",
        ),
        |ui| {
            ui.label(&summary);
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for c in &changes {
                        let text = immich_send::line(c);
                        if immich_send::replaces(c) {
                            let warn = ui.visuals().warn_fg_color;
                            ui.label(egui::RichText::new(text).color(warn));
                        } else {
                            ui.label(text);
                        }
                    }
                });
        },
    );
    if out.primary {
        app.modal = Modal::None;
        app.confirm_send(changes);
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}

/// **Undo last send**'s confirmation.
pub fn undo(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::UndoSend { count } = app.modal else {
        return;
    };
    let body = if count == 1 {
        "Put back what 1 photo had in Immich?".to_string()
    } else {
        format!("Put back what {count} photos had in Immich?")
    };
    let out = dialog(
        ctx,
        &mut app.settings,
        "undo_send",
        "Undo last send",
        theme::DialogSize::Compact,
        Footer::confirm(
            "Undo send",
            "Puts back what the last send replaced in Immich",
            true,
            "Cancel",
        ),
        |ui| {
            ui.label(&body);
        },
    );
    if out.primary {
        app.modal = Modal::None;
        app.confirm_undo();
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};
    use rustroom_app::immich_send::{Change, ServerMarks};
    use rustroom_app::immich_worker::{ImmichWorker, Job, Outcome};
    use std::sync::{Arc, Mutex};

    fn change(rating: Option<i32>, after: Option<i32>, fav: bool) -> Change {
        Change {
            asset_id: "a".into(),
            name: "IMG_1.jpg".into(),
            before: ServerMarks {
                rating,
                favourite: false,
            },
            after: ServerMarks {
                rating: after,
                favourite: fav,
            },
        }
    }

    /// ⚠⚠ A glyph missing from every bundled font draws as a grey box — `↓` and `✕` shipped that way
    /// once. Every character a line or the summary can hold must be in the fonts production installs.
    #[test]
    fn every_glyph_in_a_line_renders() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        let samples = [
            immich_send::line(&change(Some(3), Some(5), false)),
            immich_send::line(&change(None, Some(-1), true)),
            immich_send::summary(&[change(Some(3), Some(5), true)]),
        ];
        for s in &samples {
            for c in s.chars() {
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                    "{c:?} (in {s:?}) is in no bundled font"
                );
            }
        }
    }

    #[test]
    fn the_dialog_sends_only_on_its_button() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.immich_url = "https://example.invalid".into();
        app.settings.immich_key = "k".into();
        app.settings.immich_upload_key = "w".into();
        let rec: Arc<Mutex<Vec<Job>>> = Arc::new(Mutex::new(Vec::new()));
        let r = rec.clone();
        app.immich = ImmichWorker::new(
            Arc::new(move |job: Job| {
                r.lock().unwrap().push(job.clone());
                match job {
                    Job::SendMarks { groups, undo, .. } => Outcome::SendMarks {
                        landed: (0..groups.len()).collect(),
                        error: None,
                        undo,
                    },
                    other => panic!("unexpected job {other:?}"),
                }
            }),
            || {},
        );
        let open = |app: &mut RustroomApp| {
            app.modal = Modal::SendMarks {
                changes: vec![change(None, Some(5), false)],
            };
        };
        open(&mut app);
        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |_| show(&mut app, &ctx));
        test_ui::click(&ctx, "Cancel", |_| show(&mut app, &ctx));
        assert!(matches!(app.modal, Modal::None));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(rec.lock().unwrap().is_empty(), "Cancel sent");
        open(&mut app);
        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |_| show(&mut app, &ctx));
        test_ui::click(&ctx, "Send", |_| show(&mut app, &ctx));
        let t = std::time::Instant::now();
        while rec.lock().unwrap().is_empty() && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(rec.lock().unwrap().len(), 1, "Send did not send once");
    }
}

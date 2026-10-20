//! Detail's AI denoise block (M10.16b): the Amount, and the line that says while the job runs, waits for
//! the full decode or the model, or failed.
//!
//! ⚠ The bin half only: the line is `rustroom_app::denoise_tool::status`, the job `denoise_flow`'s.

use crate::develop::panel::Edited;
use crate::theme;
use eframe::egui;
use rustroom_app::denoise_tool::Line;
use rustroom_app::slider_help::Scope;

/// The quiet line's hover — why the view looks done while the photo is not.
const LINE_HOVER: &str =
    "Your view is denoised first and the rest of the photo follows in the background";

/// What the block needs besides the setting. `Default` draws no line — a test's "no session".
#[derive(Default)]
pub struct AiDenoiseUi {
    /// `denoise_tool::line` — the whole-photo job's progress, or why nothing shows yet.
    pub line: Option<Line>,
}

pub(super) fn body(
    ui: &mut egui::Ui,
    ad: &mut edit_model::AiDenoise,
    view: AiDenoiseUi,
    ed: &mut Edited,
) {
    theme::sub_label(ui, "AI denoise");
    ed.help_scope = Scope::AiDenoise;
    ed.slider(ui, &mut ad.amount, 0.0..=100.0, "Amount", 0.0);
    match &view.line {
        Some(Line::Text(text)) => theme::hint(ui, text),
        Some(Line::Progress(done)) => full_denoise(ui, *done),
        None => {}
    }
}

/// "Full denoise" over the quiet line (M10.16b6, the author's variant C) — `docs/DESIGN.md` §7.16.
fn full_denoise(ui: &mut egui::Ui, done: f32) {
    super::progress_line::show(ui, "Full denoise", done, LINE_HOVER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;
    use crate::theme::tokens;

    /// M10.16b6: while the whole-photo job runs, "Full denoise" over a quiet line — no percentage.
    #[test]
    fn the_block_draws_full_denoise_while_the_job_runs() {
        let ctx = test_ui::ctx();
        let mut ad = edit_model::AiDenoise { amount: 80.0 };
        let labels: Vec<String> = test_ui::nodes(&ctx, |ui| {
            let mut ed = Edited::default();
            body(
                ui,
                &mut ad,
                AiDenoiseUi {
                    line: Some(Line::Progress(0.4)),
                },
                &mut ed,
            );
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        assert!(labels.iter().any(|l| l == "Full denoise"), "{labels:?}");
        assert!(labels.iter().all(|l| !l.contains('%')), "{labels:?}");
    }

    /// The line is neutral — its 3 px track `line`, its done part `text3` — never the accent (DESIGN.md §4.2:
    /// the accent is for selection and set state). Amount's own slider uses the accent, so only the 3 px
    /// rectangles are read.
    #[test]
    fn the_full_denoise_line_is_never_the_accent() {
        let ctx = test_ui::ctx();
        let mut ad = edit_model::AiDenoise { amount: 80.0 };
        let mut want = None;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let t = tokens::of(ui);
                want = Some((t.line, t.text3, ui.visuals().selection.bg_fill));
                let mut ed = Edited::default();
                body(
                    ui,
                    &mut ad,
                    AiDenoiseUi {
                        line: Some(Line::Progress(0.5)),
                    },
                    &mut ed,
                );
            },
        );
        let (track, done, accent) = want.expect("drawn");
        let rects: Vec<egui::epaint::RectShape> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) => Some(r.clone()),
                _ => None,
            })
            .collect();
        // ⚠ The line's own row — the track found by its fill — so Amount's rail above, which IS the
        // accent by design, is not read.
        let row = rects
            .iter()
            .find(|r| r.fill == track && (r.rect.height() - tokens::PROGRESS_LINE).abs() < 0.01)
            .expect("the track")
            .rect
            .top();
        let fills: Vec<egui::Color32> = rects
            .iter()
            .filter(|r| (r.rect.top() - row).abs() < 0.01)
            .map(|r| r.fill)
            .collect();
        assert!(fills.contains(&done), "no done part in {fills:?}");
        assert!(!fills.contains(&accent), "the accent in {fills:?}");
    }

    /// The block draws its Amount, with its hover, and the line it is handed.
    #[test]
    fn the_block_draws_its_amount_and_its_line() {
        let ctx = test_ui::ctx();
        let mut ad = edit_model::AiDenoise::default();
        let mut missing = Vec::new();
        let labels: Vec<String> = test_ui::nodes(&ctx, |ui| {
            let mut ed = Edited::default();
            body(
                ui,
                &mut ad,
                AiDenoiseUi {
                    line: Some(Line::Text(
                        "Waiting for the full-resolution decode…".to_string(),
                    )),
                },
                &mut ed,
            );
            missing.extend(ed.missing_help);
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        for want in [
            "AI denoise",
            "Amount",
            "Waiting for the full-resolution decode…",
        ] {
            assert!(
                labels.iter().any(|l| l == want),
                "{want} missing from {labels:?}"
            );
        }
        assert!(missing.is_empty(), "{missing:?}");
    }
}

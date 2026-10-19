//! Detail's AI denoise block (M10.16b): the Amount, and the line that says while the job runs, waits for
//! the full decode or the model, or failed.
//!
//! ⚠ The bin half only: the line is `rustroom_app::denoise_tool::status`, the job `denoise_flow`'s.

use crate::develop::panel::Edited;
use crate::theme;
use eframe::egui;
use rustroom_app::slider_help::Scope;

/// What the block needs besides the setting. `Default` draws no line — a test's "no session".
#[derive(Default)]
pub struct AiDenoiseUi {
    /// `denoise_tool::status` — the job running, waiting or failed.
    pub status: Option<String>,
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
    if let Some(text) = &view.status {
        theme::hint(ui, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

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
                    status: Some("Denoising… 37 %".to_string()),
                },
                &mut ed,
            );
            missing.extend(ed.missing_help);
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        for want in ["AI denoise", "Amount", "Denoising… 37 %"] {
            assert!(
                labels.iter().any(|l| l == want),
                "{want} missing from {labels:?}"
            );
        }
        assert!(missing.is_empty(), "{missing:?}");
    }
}

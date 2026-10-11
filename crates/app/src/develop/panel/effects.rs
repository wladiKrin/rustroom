//! The Effects section's body (M10.11b): the post-crop vignette, then grain — Lightroom's panel.
//!
//! ⚠ Its own file because `adjustments.rs` held 795 lines of code when grain arrived.

use crate::develop::panel::Edited;
use crate::theme;
use eframe::egui;
use rustroom_app::slider_help::Scope;

/// The vignette's sliders, then grain's.
pub(super) fn body(ui: &mut egui::Ui, edits: &mut edit_model::Edits, ed: &mut Edited) {
    theme::sub_label(ui, "Post-crop vignette");
    let v = &mut edits.vignette;
    ed.help_scope = Scope::Vignette;
    ed.slider(ui, &mut v.amount, -100.0..=100.0, "Amount", 0.0);
    ed.slider(
        ui,
        &mut v.midpoint,
        0.0..=100.0,
        "Midpoint",
        edit_model::Vignette::default().midpoint,
    );
    ed.slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness", 0.0);
    ed.slider(
        ui,
        &mut v.feather,
        0.0..=100.0,
        "Feather",
        edit_model::Vignette::default().feather,
    );
    theme::sub_label(ui, "Grain");
    let g = &mut edits.grain;
    ed.help_scope = Scope::Grain;
    let d = edit_model::Grain::default();
    ed.slider(ui, &mut g.amount, 0.0..=100.0, "Amount", d.amount);
    ed.slider(ui, &mut g.size, 0.0..=100.0, "Size", d.size);
    ed.slider(ui, &mut g.roughness, 0.0..=100.0, "Roughness", d.roughness);
    ed.help_scope = Scope::Global;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn draw<'a>(e: &'a mut edit_model::Edits) -> impl FnMut(&mut egui::Ui) + 'a {
        move |ui| {
            let mut ed = Edited::default();
            body(ui, e, &mut ed);
        }
    }

    #[test]
    fn the_effects_body_holds_the_vignette_then_grain() {
        let ctx = test_ui::ctx();
        let mut e = edit_model::Edits::default();
        let sliders = test_ui::slider_labels(&ctx, draw(&mut e));
        for want in ["Midpoint", "Roundness", "Feather", "Size", "Roughness"] {
            assert!(sliders.iter().any(|l| l == want), "{want}: {sliders:?}");
        }
        assert_eq!(
            sliders.iter().filter(|l| *l == "Amount").count(),
            2,
            "{sliders:?}"
        );
        // ⚠ By position, not by the order AccessKit lists nodes in, which is not the drawing order.
        let nodes = test_ui::nodes(&ctx, draw(&mut e));
        let y = |s: &str| {
            nodes
                .iter()
                .find(|n| n.label == s)
                .map(|n| n.rect.min.y)
                .unwrap_or_else(|| panic!("{s} was not drawn"))
        };
        assert!(
            y("Post-crop vignette") < y("Grain"),
            "the vignette first, as Lightroom"
        );
        for l in ["Amount", "Size", "Roughness"] {
            assert!(
                rustroom_app::slider_help::text(rustroom_app::slider_help::Scope::Grain, l)
                    .is_some(),
                "Grain's {l} has no hover sentence"
            );
        }
    }

    #[test]
    fn the_grain_amount_slider_sets_the_grain() {
        let ctx = test_ui::ctx();
        let mut e = edit_model::Edits::default();
        // The lower "Amount" is Grain's — the vignette's comes first.
        let at = test_ui::slider_centres(&ctx, draw(&mut e))
            .into_iter()
            .filter(|(l, _)| l == "Amount")
            .map(|(_, p)| p)
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .expect("Grain's Amount");
        test_ui::click_at(&ctx, at + egui::vec2(40.0, 0.0), false, draw(&mut e));
        assert!(e.grain.amount > 0.0, "{:?}", e.grain);
        assert_eq!(
            e.vignette,
            edit_model::Vignette::default(),
            "the vignette untouched"
        );
    }
}

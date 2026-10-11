//! The Negative block (M10.11a): a scanned film negative turned into a positive.
//!
//! ⚠ Above the Profile block and, like it, **not a section** — Reset adjustments derives its groups
//! from the sections, and resetting a scanned negative should start over from the positive, not hand
//! back the negative (`edit_model::NEGATIVE_GROUP`). Collapsed by default: a digital photograph has
//! no use for it, and its header says whether it is on.

use crate::develop::panel::Edited;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::negative_tool;

/// The block's body. Arms the film-base eyedropper; the click itself is the canvas's.
pub(super) fn block(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    let picked = s.edits.negative.base.iter().all(|&b| b > 0.0);
    let mut on = s.edits.negative.on;
    // ⚠ Enabled once a base is picked: on without one would invert by nothing.
    if ui
        .add_enabled(picked, egui::Checkbox::new(&mut on, "Film negative"))
        .hover("Turns the negative into a positive")
        .changed()
    {
        s.edits.negative.on = on;
        ed.discrete();
    }
    ui.horizontal(|ui| {
        if picked {
            let b = pipeline::negative::positive(
                s.edits.negative.base,
                &edit_model::Negative::default(),
            );
            let c = b.map(|v| (color::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
        }
        let armed = s.picking_negative;
        if ui
            .selectable_label(armed, "Pick film base")
            .hover("Picks the unexposed film between frames to cancel its orange mask")
            .clicked()
        {
            // ⚠ One eyedropper at a time: a click must mean one thing.
            s.picking_negative = !armed;
            s.picking_wb = false;
            s.picking_fringe = false;
        }
        if ui
            .add_enabled(picked, egui::Button::new("Auto balance"))
            .hover("Measures this frame again for black and white")
            .clicked()
        {
            let buffer = std::sync::Arc::clone(&s.buffer);
            if negative_tool::rebalance(&mut s.edits, &buffer) {
                ed.discrete();
            }
        }
    });
    let mut mono = s.edits.negative.mono;
    if ui
        .checkbox(&mut mono, "Black & white")
        .hover("Inverts a black and white negative into one neutral channel")
        .changed()
    {
        s.edits.negative.mono = mono;
        ed.discrete();
    }
    let mut contrast = negative_tool::contrast_of(s.edits.negative.gamma);
    let r = ed.slider(ui, &mut contrast, -100.0..=100.0, "Film contrast", 0.0);
    if r.changed() {
        s.edits.negative.gamma = negative_tool::gamma_of(contrast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 40);
        app
    }

    fn draw(app: &mut crate::RustroomApp) -> impl FnMut(&mut egui::Ui) + '_ {
        move |ui| {
            let mut ed = Edited::default();
            block(ui, app.session.as_mut().expect("a session"), &mut ed);
        }
    }

    #[test]
    fn pick_film_base_arms_the_picker_and_a_second_click_disarms_it() {
        let ctx = crate::test_ui::ctx();
        let mut app = session(&ctx);
        crate::test_ui::click(&ctx, "Pick film base", draw(&mut app));
        assert!(app.session.as_ref().unwrap().picking_negative);
        crate::test_ui::click(&ctx, "Pick film base", draw(&mut app));
        assert!(!app.session.as_ref().unwrap().picking_negative);
    }

    #[test]
    fn nothing_turns_on_before_a_base_is_picked() {
        let ctx = crate::test_ui::ctx();
        let mut app = session(&ctx);
        crate::test_ui::click(&ctx, "Film negative", draw(&mut app));
        assert!(!app.session.as_ref().unwrap().edits.negative.on);
    }

    #[test]
    fn the_controls_are_drawn_and_film_contrast_explains_itself() {
        let ctx = crate::test_ui::ctx();
        let mut app = session(&ctx);
        let sliders = crate::test_ui::slider_labels(&ctx, draw(&mut app));
        assert!(sliders.iter().any(|l| l == "Film contrast"), "{sliders:?}");
        let help = rustroom_app::slider_help::text(
            rustroom_app::slider_help::Scope::Global,
            "Film contrast",
        );
        assert!(help.is_some(), "Film contrast has no hover sentence");
        let labels: Vec<String> = crate::test_ui::nodes(&ctx, draw(&mut app))
            .into_iter()
            .map(|n| n.label)
            .collect();
        for want in [
            "Film negative",
            "Pick film base",
            "Auto balance",
            "Black & white",
        ] {
            assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
        }
    }

    #[test]
    fn black_and_white_and_auto_balance_change_the_negative() {
        let ctx = crate::test_ui::ctx();
        let mut app = session(&ctx);
        {
            let s = app.session.as_mut().unwrap();
            s.edits.negative = edit_model::Negative {
                on: true,
                base: [0.85, 0.55, 0.3],
                ..edit_model::Negative::default()
            };
        }
        crate::test_ui::click(&ctx, "Black & white", draw(&mut app));
        assert!(app.session.as_ref().unwrap().edits.negative.mono);
        let before = app.session.as_ref().unwrap().edits.negative;
        crate::test_ui::click(&ctx, "Auto balance", draw(&mut app));
        let after = app.session.as_ref().unwrap().edits.negative;
        assert_ne!(
            (before.low, before.high),
            (after.low, after.high),
            "Auto balance measured nothing"
        );
    }
}

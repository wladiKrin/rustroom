//! The Retouch tool's controls: the patch's size and feather, its mode, and what to do with the
//! spots already placed.

use super::Edited;
use crate::Session;
use edit_model::SpotMode;
use eframe::egui;
use rustroom_app::spot_tool::{SpotTool, MAX_RADIUS, MIN_RADIUS};

/// Draw the Retouch controls.
///
/// ⚠ `tool` is `&mut` to a field of `RustroomApp` while `s` borrows another — **disjoint** fields,
/// which is what makes both legal where a closure capturing the app twice would be `E0499`. The
/// `masks::BrushTool` precedent, which takes five of them.
pub(crate) fn show(ui: &mut egui::Ui, s: &mut Session, tool: &mut SpotTool, ed: &mut Edited) {
    // ⚠⚠ **Not a second mechanism.** This slider shows and sets the *same* value the canvas
    // size-drag sets — one value, two affordances, the eyedropper-and-Kelvin idiom M5.2 shipped.
    // The drag writes back into `tool.radius`, so the two can never disagree.
    ui.add(
        egui::Slider::new(&mut tool.radius, MIN_RADIUS..=MAX_RADIUS)
            .text("Size")
            .custom_formatter(|v, _| format!("{:.1}%", v * 100.0)),
    )
    .on_hover_text(
        "The patch's radius, as a percentage of the long edge. Dragging out on the canvas sets \
         this too.",
    );
    ui.add(egui::Slider::new(&mut tool.feather, 0.0..=100.0).text("Feather"))
        .on_hover_text("0 is a hard edge; 100 ramps from the centre to the rim.");

    ui.horizontal(|ui| {
        // ⚠ A segmented pair, matching the HSL channel selector and the grading wheels: two
        // mutually exclusive modes read better as one control than as two checkboxes.
        ui.spacing_mut().item_spacing.x = 0.0;
        for (m, label, hint) in [
            (
                SpotMode::Heal,
                "Heal",
                "Matches the source patch to the ring around the destination — what makes a spot \
                 on a sky disappear rather than leaving a disc of slightly different sky.",
            ),
            (
                SpotMode::Clone,
                "Clone",
                "A straight copy, blended by the feather.",
            ),
        ] {
            if ui
                .selectable_label(tool.mode == m, format!("  {label}  "))
                .on_hover_text(hint)
                .clicked()
            {
                tool.mode = m;
            }
        }
    });

    ui.separator();

    // ⚠ A count, not a list. Forty dust spots make a useless list, and Lightroom has none either.
    ui.horizontal(|ui| {
        let n = s.edits.spots.len();
        crate::theme::hint(
            ui,
            &match n {
                0 => "No spots".to_string(),
                1 => "1 spot".to_string(),
                n => format!("{n} spots"),
            },
        );
        if ui
            .add_enabled(n > 0, egui::Button::new("Clear all"))
            .on_hover_text("Removes every retouch patch from this photo")
            .clicked()
        {
            s.edits.spots.clear();
            // ⚠ The selection must go with them, or the next Delete acts on an index into an
            // empty list.
            s.selected_spot = None;
            // ⚠ `discrete`, not a slider's `changed`: a click is a complete act, so it is one
            // history entry immediately rather than waiting for a `drag_stopped` that never comes.
            ed.discrete();
        }
    });

    crate::theme::hint(
        ui,
        "Click to place a patch; drag out to size it; ⌥-drag to choose the source yourself. \
         Delete removes the selected patch.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_spot() -> edit_model::Spot {
        edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.2, 0.2],
            radius: 0.02,
            feather: 50.0,
            mode: SpotMode::Heal,
        }
    }

    /// ⚠ One click, one history entry — and it must clear the selection too, or the next
    /// keystroke would act on an index into an empty list.
    #[test]
    fn clear_all_removes_every_spot_and_marks_one_edit() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot(), a_spot()];
            s.selected_spot = Some(1);
        }
        let mut seen = Edited::default();
        crate::test_ui::click(&ctx, "Clear all", |ui| {
            let mut ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut ed);
            if ed.changed {
                seen = ed;
            }
        });
        let s = app.session.as_ref().expect("session");
        assert!(s.edits.spots.is_empty(), "Clear all left spots behind");
        assert_eq!(s.selected_spot, None, "the selection outlived the spots");
        assert!(
            seen.changed && seen.settled,
            "one click must be one history entry"
        );
    }

    /// ⚠ The discriminating other half: merely drawing the panel must change nothing, or the
    /// button would fire sixty times a second — the `apply_view_demosaic` rule.
    #[test]
    fn merely_drawing_the_panel_changes_nothing() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.session.as_mut().unwrap().edits.spots = vec![a_spot()];
        for _ in 0..3 {
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    show(ui, s, &mut tool, &mut ed);
                    assert!(!ed.changed, "drawing the panel reported an edit");
                },
            );
        }
        assert_eq!(
            app.session.as_ref().unwrap().edits.spots.len(),
            1,
            "drawing the panel removed a spot"
        );
    }

    /// ⚠ The default radius is generous on purpose: A's Wave 3 finding is that the radius must
    /// **exceed** the blemish, or heal measures a ring that is still lying on it.
    #[test]
    fn the_default_tool_is_usable() {
        let t = SpotTool::default();
        assert!(t.radius >= MIN_RADIUS * 4.0, "got {}", t.radius);
        assert_eq!(t.mode, SpotMode::Heal, "Heal is the useful default");
    }
}

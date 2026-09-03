//! The Retouch tool's controls: the patch's size and feather, its mode, and what to do with the
//! spots already placed.

use super::Edited;
use crate::Session;
use edit_model::SpotMode;
use eframe::egui;
use rustroom_app::spot_tool::{self, SpotTool, MAX_RADIUS, MIN_RADIUS};

/// Draw the Retouch controls.
///
/// ⚠ `tool` is `&mut` to a field of `RustroomApp` while `s` borrows another — **disjoint** fields,
/// which is what makes both legal where a closure capturing the app twice would be `E0499`. The
/// `masks::BrushTool` precedent, which takes five of them.
pub(crate) fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    show_outlines: &mut bool,
    ed: &mut Edited,
) {
    // ⚠⚠ **When a patch is selected the controls edit THAT patch; otherwise they set what the
    // next one will be.** Reported from use: "when clicking on a patch, it should be possible to
    // adapt the size and feather, right now it's doing nothing" — because the sliders only ever
    // wrote the tool's defaults, which affect the *next* placement and nothing on screen.
    //
    // ⚠ Editing a selected patch also updates the defaults, so the next placement inherits what
    // you just dialled in. The reverse does not hold: changing the defaults with nothing selected
    // must not reach back and alter an existing patch.
    let selected = s.selected_spot.filter(|&i| i < s.edits.spots.len());
    crate::theme::hint(
        ui,
        match selected {
            Some(_) => "Editing the selected patch",
            None => "Settings for the next patch",
        },
    );

    // ⚠ Copied out, edited, then written back — `s.edits.spots[i]` cannot be borrowed mutably
    // across the whole block while `s.edits.spots.len()` is also read. `Spot` is `Copy`.
    let mut live = selected.map(|i| s.edits.spots[i]);
    let radius = live.as_mut().map_or(&mut tool.radius, |sp| &mut sp.radius);
    // ⚠⚠ **Not a second mechanism.** This slider shows and sets the *same* value the canvas
    // size-drag sets — one value, two affordances, the eyedropper-and-Kelvin idiom M5.2 shipped.
    // The drag writes `tool.radius` on every frame, so the slider tracks the gesture live.
    //
    // ⚠⚠ **Shown in PIXELS of the full-resolution image, not as a percentage** — reported as
    // "I don't like the % numbering, can we just do pixels". A radius is *stored* as a fraction of
    // the long edge, because that is what makes it survive a resize; nobody thinks about dust in
    // fractions of a frame.
    //
    // ⚠ Against `full_long`, never the buffer's long edge: a RAW opens at half size, so a pixel
    // count taken from the buffer would silently double a few seconds later when the
    // full-resolution decode landed.
    //
    // ⚠⚠ **Recorded honestly: no test can see which of the two is used.** The slider's range and
    // its write-back divide by the *same* figure, so a wrong `long` is self-consistent and yields
    // an identical radius — it changes only the digits on screen, and egui does not publish a
    // slider's formatted value to the accessibility tree. The arithmetic itself is covered by
    // `spot_tool::the_pixel_conversion_round_trips_and_clamps`; this one line is not.
    let long = s.full_long;
    let mut px = spot_tool::radius_px(*radius, long);
    let r = ui
        .add(
            // ⚠⚠ **Logarithmic**, because the range spans 50x and the useful zone sits at the
            // bottom of it. On a linear slider 0.1–0.5% — where this tool is actually used —
            // occupied the first 8% of the travel, which is the "cramped" half of the same report.
            egui::Slider::new(
                &mut px,
                spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
            )
            .logarithmic(true)
            .text("Size")
            .custom_formatter(|v, _| format!("{v:.0} px")),
        )
        .on_hover_text(
            "The patch's radius in pixels of the full-resolution image. Dragging out on the \
             canvas sets this too.",
        );
    if r.changed() {
        *radius = spot_tool::radius_from_px(px, long);
    }
    // ⚠⚠ **Accounted locally, and folded into `ed` only if a patch is selected.** Calling
    // `absorb` here unconditionally re-rendered the photo and pushed a history entry every time
    // the *defaults* were adjusted — an undo step for a change no pixel saw. Caught by the test
    // below, not by review.
    let mut touched = r.changed();
    let mut settled = r.drag_stopped();

    let feather = live
        .as_mut()
        .map_or(&mut tool.feather, |sp| &mut sp.feather);
    let r = ui
        .add(egui::Slider::new(feather, 0.0..=100.0).text("Feather"))
        .on_hover_text("0 is a hard edge; 100 ramps from the centre to the rim.");
    touched |= r.changed();
    settled |= r.drag_stopped();

    let mode = live.as_mut().map_or(&mut tool.mode, |sp| &mut sp.mode);
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
                .selectable_label(*mode == m, format!("  {label}  "))
                .on_hover_text(hint)
                .clicked()
            {
                *mode = m;
                // ⚠ Both flags: a click is a complete act with no drag to stop, so without
                // `settled` the edit would reach the pixels and never the undo stack.
                touched = true;
                settled = true;
            }
        }
    });

    // ⚠ Only a change to a *selected patch* is a photo edit. Adjusting the defaults changes no
    // pixels, so it must not re-render or push a history entry — the `apply_view_demosaic` rule
    // in another costume.
    if let (Some(i), Some(sp)) = (selected, live) {
        if touched {
            s.edits.spots[i] = sp;
            // ⚠ The defaults follow the patch you just tuned, so the next click inherits it.
            tool.radius = sp.radius;
            tool.feather = sp.feather;
            tool.mode = sp.mode;
        }
        ed.changed |= touched;
        ed.settled |= settled;
    }

    ui.separator();

    // ⚠⚠ **Turning the outlines off is how you see the photograph.** With thirty patches placed
    // the circles become the picture, and judging what still needs retouching is impossible.
    // Reported alongside wanting the unselected ones dimmer — the two answer one problem at
    // different strengths.
    //
    // ⚠ It suppresses only the drawing of *committed* patches: every gesture still works, and a
    // gesture in progress is still previewed. A toggle that disabled the tool would be a mode.
    ui.checkbox(show_outlines, "Show patches").on_hover_text(
        "Hides the outlines so you can see the photograph. Placing and editing still work.",
    );

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
            show(ui, s, &mut tool, &mut true, &mut ed);
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
                    show(ui, s, &mut tool, &mut true, &mut ed);
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

    /// Drive one panel frame with a synthetic drag on the Size slider.
    fn drag_size(app: &mut crate::RustroomApp, ctx: &egui::Context, to: f32) -> Edited {
        let mut out = Edited::default();
        // Find the slider, then drag its handle to the far right.
        let at = crate::test_ui::find(ctx, "Size", |ui| {
            let mut ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut true, &mut ed);
        })
        .expect("no Size slider was drawn");
        let target = egui::pos2(at.x + to, at.y);
        for pass in 0..4 {
            let pos = if pass <= 1 { at } else { target };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 || pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let mut tool = app.spot_tool;
            let mut ed = Edited::default();
            {
                let s = app.session.as_mut().expect("session");
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(crate::test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, s, &mut tool, &mut true, &mut ed),
                );
            }
            app.spot_tool = tool;
            if ed.changed {
                out.changed = true;
            }
            if ed.settled {
                out.settled = true;
            }
        }
        out
    }

    /// ⚠⚠ **The reported bug: selecting a patch and moving Size did nothing**, because the
    /// sliders only ever wrote the tool's defaults — which decide the *next* placement and change
    /// nothing on screen. With a patch selected they must edit that patch.
    #[test]
    fn the_sliders_edit_the_selected_patch() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot()];
            s.selected_spot = Some(0);
        }
        let before = app.session.as_ref().unwrap().edits.spots[0].radius;
        let ed = drag_size(&mut app, &ctx, 200.0);
        let after = app.session.as_ref().unwrap().edits.spots[0].radius;
        assert_ne!(after, before, "the slider did not reach the selected patch");
        assert!(ed.changed, "editing a patch must request a render");
        // ⚠ And the defaults follow it, so the next click inherits what was just dialled in.
        assert_eq!(
            app.spot_tool.radius, after,
            "the tool default did not follow the patch"
        );
    }

    /// ⚠⚠ The discriminating other half: with **nothing** selected the same slider must move only
    /// the default and leave every existing patch alone — and must not report a photo edit, since
    /// it changes no pixels. Without this, "the slider edits the patch" would pass against a
    /// version that edited a patch whether or not one was selected.
    #[test]
    fn with_nothing_selected_the_sliders_leave_the_patches_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot()];
            s.selected_spot = None;
        }
        let before = app.session.as_ref().unwrap().edits.spots[0];
        let ed = drag_size(&mut app, &ctx, 200.0);
        let after = app.session.as_ref().unwrap().edits.spots[0];
        assert_eq!(after, before, "an unselected patch was edited");
        assert_ne!(
            app.spot_tool.radius, before.radius,
            "the default did not move either, so the drag did nothing at all"
        );
        assert!(
            !ed.settled,
            "changing a default pushed a history entry for a photo that did not change"
        );
    }

    /// ⚠ The toggle is a plain view flag, and turning it off must not touch a single patch —
    /// hiding the outlines is for *looking*, not for editing.
    #[test]
    fn hiding_the_outlines_changes_no_patch() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.session.as_mut().unwrap().edits.spots = vec![a_spot()];
        let before = app.session.as_ref().unwrap().edits.spots.clone();
        let mut on = true;
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Show patches", |ui| {
            ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut on, &mut ed);
        });
        assert!(!on, "the checkbox did not toggle");
        assert_eq!(
            app.session.as_ref().unwrap().edits.spots,
            before,
            "hiding the outlines edited a patch"
        );
        assert!(
            !ed.settled,
            "a view toggle pushed a history entry for a photo that did not change"
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

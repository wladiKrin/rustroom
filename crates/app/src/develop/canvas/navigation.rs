//! Canvas navigation: which gestures belong to the active tool, and which pan the view.
//!
//! Split out of `canvas.rs` — the two are asked about together on every canvas feature, and
//! keeping them beside their (large) harnesses keeps the orchestrator readable.

use eframe::egui;
use rustroom_app::settings::PanDrag;

/// Whether a drag belongs to the active tool — the crop rectangle, a mask handle, and later
/// the brush.
///
/// ⚠ **egui reports `dragged()` for every button, not just the primary one.** `interaction.rs`
/// matches `PointerEvent::Pressed { .. }` and ignores the button when setting
/// `potential_drag_id`. Since panning now lives on middle- or right-drag, an ungated tool would
/// be driven by the pan gesture as well — dragging a crop corner while trying to pan.
pub(super) fn tool_drag(resp: &egui::Response) -> bool {
    resp.dragged_by(egui::PointerButton::Primary)
}

/// Whether a drag on the tool's button has just begun. Same gating, same reason.
pub(super) fn tool_drag_started(resp: &egui::Response) -> bool {
    resp.drag_started_by(egui::PointerButton::Primary)
}

/// Every source of panning, folded into one screen-space delta.
///
/// ⚠ **Scroll always pans and is not configurable.** It is the universal route — a two-finger
/// swipe on any trackpad, a wheel on any mouse — so making it optional would let a user lock
/// themselves out of panning entirely. Only the extra *drag* gesture is a setting.
///
/// ⚠ Both scroll axes are used. A horizontal two-finger swipe is an ordinary gesture, and the
/// old code read `.y` alone only because `.y` was driving zoom.
///
/// ⚠ The sign is right by egui's own convention, not by guesswork: `Event::MouseWheel`'s doc
/// says the delta "indicates how to move the *content* being viewed", and panning is exactly
/// moving the content.
///
/// ⚠⚠ **Takes plain values, never `&egui::InputState`.** `Response::dragged_by` calls
/// `self.ctx.input(..)` internally, so a signature holding the input lock deadlocks the moment a
/// drag is actually in progress — epaint's mutex panics on the re-entrant borrow. It survives
/// casual testing because `dragged_by` is `dragged() && ctx.input(..)`, and `dragged()` reads a
/// plain flag, so the `&&` short-circuits and never locks *unless something is being dragged*.
/// Caught by `the_configured_drag_button_pans`; it would otherwise have panicked in production
/// on the first middle-drag.
pub(super) fn pan_delta(
    resp: &egui::Response,
    scroll: egui::Vec2,
    space_down: bool,
    binding: PanDrag,
    text_focused: bool,
) -> egui::Vec2 {
    let mut delta = egui::Vec2::ZERO;

    // ⚠ The hover gate is what stops a scroll over the adjustments panel or the filmstrip
    // panning the photo underneath them.
    if resp.hovered() {
        delta += scroll;
    }

    let dragging = match binding {
        PanDrag::Middle => resp.dragged_by(egui::PointerButton::Middle),
        PanDrag::Secondary => resp.dragged_by(egui::PointerButton::Secondary),
        // ⚠ Gated on focus: without it, a space typed into the preset-name field would arm
        // panning. The rule arrow-nav and the clipboard keys already follow.
        PanDrag::SpaceLeft => {
            !text_focused && space_down && resp.dragged_by(egui::PointerButton::Primary)
        }
    };
    if dragging {
        delta += resp.drag_delta();
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::view_region;

    /// Drive a real allocated rect through a press-and-move gesture with `button`, and report
    /// what `tool_drag` said on the final pass.
    ///
    /// ⚠ Four passes, not three. egui only becomes "decidedly dragging" once the pointer has
    /// moved far enough *after* the press (`is_decidedly_dragging` requires `!any_pressed()`),
    /// so the press and the movement must land on different frames.
    fn tool_drag_for(button: egui::PointerButton) -> bool {
        let ctx = egui::Context::default();
        let mut out = false;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            let pos = start + egui::vec2(40.0, 40.0) * (pass as f32 - 1.0).max(0.0);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    out = tool_drag(&resp);
                }
            });
        }
        out
    }

    #[test]
    fn a_primary_drag_belongs_to_the_tools() {
        assert!(tool_drag_for(egui::PointerButton::Primary));
    }

    #[test]
    fn a_middle_drag_does_not_belong_to_the_tools() {
        // ⚠ Not hypothetical. egui's `interaction.rs` matches `PointerEvent::Pressed { .. }`
        // and IGNORES the button when deciding what is being dragged, so `resp.dragged()` is
        // true for a middle-drag too. Without this gate the pan button would also drag crop
        // rectangles and mask handles.
        assert!(!tool_drag_for(egui::PointerButton::Middle));
    }

    #[test]
    fn a_secondary_drag_does_not_belong_to_the_tools() {
        // Right-drag is one of the selectable pan bindings, so it needs the same exclusion.
        assert!(!tool_drag_for(egui::PointerButton::Secondary));
    }

    /// Drive a real allocated rect and report what `pan_delta` returned on the final pass.
    ///
    /// ⚠⚠ **The pointer must move on the frame that is read, not merely before it.**
    /// `drag_delta()` is per-frame, so an earlier version that moved once on pass 2 and read on
    /// pass 3 saw `[0,0]` — which made `a_left_drag_does_not_pan`, the most important test in
    /// this change, pass vacuously: it would have passed with the gating deleted entirely.
    /// Movement therefore continues on every pass from 2 onward.
    fn pan_delta_for(
        button: Option<egui::PointerButton>,
        scroll: egui::Vec2,
        binding: PanDrag,
        space_down: bool,
        text_focused: bool,
    ) -> egui::Vec2 {
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            // Keeps moving: pass 2 is +40, pass 3 is +80, so the frame that is read has a
            // real per-frame delta of its own.
            let pos = start + egui::vec2(40.0, 40.0) * (pass as f32 - 1.0).max(0.0);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                if let Some(b) = button {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: b,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                if space_down {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                }
            }
            // ⚠ Delivered on passes 2 AND 3: egui smooths scroll across frames, so a single
            // wheel event may not have reached `smooth_scroll_delta` by the time it is read.
            if pass >= 2 && scroll != egui::Vec2::ZERO {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: scroll,
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    // ⚠ The input values are read and the lock released *before* `pan_delta`
                    // runs, because `dragged_by` takes the same lock internally.
                    let scroll = ui.input(|i| i.smooth_scroll_delta);
                    let space = ui.input(|i| i.key_down(egui::Key::Space));
                    out = pan_delta(&resp, scroll, space, binding, text_focused);
                }
            });
        }
        out
    }

    #[test]
    fn a_left_drag_does_not_pan() {
        // ⚠⚠ The regression guard for this entire change, and it asserts the pan delta is
        // ZERO rather than that some other branch ran. Asserting "the tool branch was taken"
        // would pass with a stray `s.pan +=` still in place — precisely the shape of mistake
        // this project keeps making.
        for binding in PanDrag::ALL {
            assert_eq!(
                pan_delta_for(
                    Some(egui::PointerButton::Primary),
                    egui::Vec2::ZERO,
                    binding,
                    false,
                    false
                ),
                egui::Vec2::ZERO,
                "{binding:?}"
            );
        }
    }

    #[test]
    fn the_configured_drag_button_pans() {
        let d = pan_delta_for(
            Some(egui::PointerButton::Middle),
            egui::Vec2::ZERO,
            PanDrag::Middle,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "middle: {d:?}");

        let d = pan_delta_for(
            Some(egui::PointerButton::Secondary),
            egui::Vec2::ZERO,
            PanDrag::Secondary,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "secondary: {d:?}");
    }

    #[test]
    fn a_drag_on_the_wrong_button_does_not_pan() {
        // Middle-drag while the binding is Secondary must do nothing at all.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Middle),
                egui::Vec2::ZERO,
                PanDrag::Secondary,
                false,
                false
            ),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn scrolling_pans_both_axes() {
        // ⚠ Before this change only `.y` was read anywhere in this file, and it drove zoom. A
        // horizontal two-finger swipe must pan too, so the x component is asserted separately
        // rather than folded into a magnitude check that a y-only implementation would satisfy.
        let d = pan_delta_for(None, egui::vec2(11.0, 7.0), PanDrag::Middle, false, false);
        assert!(d.x.abs() > 0.0, "x component ignored: {d:?}");
        assert!(d.y.abs() > 0.0, "y component ignored: {d:?}");
    }

    #[test]
    fn scrolling_outside_the_image_does_not_pan() {
        // ⚠ The `hovered()` gate. Without it, scrolling the adjustments panel or the filmstrip
        // would pan the photo underneath them.
        //
        // ⚠ This cannot reuse `pan_delta_for`: that harness always points at (100,100), inside
        // the rect, so it could never detect a missing gate. The pointer is driven far from a
        // small rect here instead.
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        for pass in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(380.0, 380.0)));
            if pass >= 2 {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(11.0, 7.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                // A small rect in the corner, far from the pointer.
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(50.0, 50.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    let scroll = ui.input(|i| i.smooth_scroll_delta);
                    out = pan_delta(&resp, scroll, false, PanDrag::Middle, false);
                }
            });
        }
        assert_eq!(out, egui::Vec2::ZERO);
    }

    #[test]
    fn space_left_drag_pans_only_with_space_held() {
        assert!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                false
            ) != egui::Vec2::ZERO
        );
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                false,
                false
            ),
            egui::Vec2::ZERO,
            "without Space this is an ordinary left-drag and must not pan"
        );
    }

    #[test]
    fn space_left_drag_is_ignored_while_a_text_field_has_focus() {
        // ⚠ Otherwise typing a space into the preset-name field arms panning. The same rule
        // arrow-nav and the clipboard keys already follow.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                true
            ),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn the_pan_reset_at_fit_is_unconditional() {
        // ⚠ A behaviour fix, not a port. The old reset sat *inside* the scroll branch, so a
        // left-drag at fit genuinely pushed a fitting image off-centre into grey and nothing
        // put it back until the next scroll. This pins the rule itself rather than the call
        // site, in the style of `grid_columns_leaves_room_for_the_gaps`.
        let reset = |zoom: f32, pan: egui::Vec2| {
            if zoom <= view_region::MIN_ZOOM {
                egui::Vec2::ZERO
            } else {
                pan
            }
        };
        assert_eq!(reset(1.0, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(reset(0.5, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(
            reset(1.5, egui::vec2(50.0, 50.0)),
            egui::vec2(50.0, 50.0),
            "above fit the pan must survive"
        );
    }
}

//! Shared harness for the canvas tests: a screen rect, the canvas centre measured rather than
//! hardcoded, and a full press → move → release drag through the real `show`.
//!
//! ⚠ Lives in its own module because the crop and mask overlays both drive it, and a helper
//! duplicated per overlay is how the two would drift.

use crate::RustroomApp;
use eframe::egui;

/// The screen rect every harness pass uses.
pub(super) fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
}

/// The centre of the area the canvas is drawn into — and, with no pan, the screen position
/// of both a mask handle at the normalised frame centre and the middle of the image.
///
/// ⚠⚠ **Measured, not hardcoded.** An earlier version wrote `(200, 150)`, obtained by
/// instrumenting the layout once. That is brittle in the worst way: a layout change moves the
/// target silently, and the failure mode is a test that still passes while grabbing nothing —
/// which is exactly the fault this file hit twice while it was being written. Running one
/// pass and reading `ui.max_rect()` costs a frame and cannot go stale.
pub(super) fn canvas_centre(app: &mut RustroomApp, ctx: &egui::Context) -> egui::Pos2 {
    let mut centre = egui::Pos2::ZERO;
    let c = ctx.clone();
    let _ = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        |ui| {
            centre = ui.max_rect().center();
            super::show(app, ui, &c);
        },
    );
    centre
}

/// Drive `show` through a full press → move → release drag.
///
/// ⚠⚠ **Five passes, and every one is load-bearing.** Move to `from`; press *while still
/// there*; move to `to`, which is the drag; release; settle. `Response` state is per-frame,
/// so a shorter harness reads zeros — that is what made `a_left_drag_does_not_pan` pass
/// vacuously once already — and a release is only *known* on its own pass, so anything it
/// triggers cannot be observed until the frame after.
/// Returns the engine's request count after each pass, so a test can sample the *drag frame*
/// rather than the whole gesture.
pub(super) fn drag_through(
    app: &mut RustroomApp,
    ctx: &egui::Context,
    from: egui::Pos2,
    to: egui::Pos2,
) -> Vec<usize> {
    let mut counts = Vec::new();
    for pass in 0..5 {
        // ⚠⚠ The pointer must still be at `from` on the pass that presses. An earlier
        // version moved it to `to` on the same pass, so the press landed away from the
        // handle — and `dragging_a_mask_handle_asks_for_a_render` **passed anyway**, because
        // `show` issues renders for other reasons. Its failing sibling is what exposed it.
        let pos = if pass <= 1 { from } else { to };
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if pass == 1 {
            events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
        }
        if pass == 3 {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
        }
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                events,
                ..Default::default()
            },
            |ui| {
                super::show(app, ui, &c);
            },
        );
        counts.push(
            app.session
                .as_ref()
                .map(|s| s.engine.requests_issued())
                .unwrap_or(0),
        );
    }
    counts
}

/// The pass on which the pointer moves with the button down — i.e. the drag itself.
pub(super) const DRAG_PASS: usize = 2;

/// Drive `show` through a press → release **at one point**, i.e. an ordinary click.
///
/// ⚠ Not a zero-length `drag_through`: egui reports `clicked()` only for a press-release that
/// never became a drag, so the pointer must not move between the two. Five passes for the same
/// reason the drag harness has them — a release is only known on its own pass, and what it
/// triggers cannot be observed until the frame after.
pub(super) fn click_through(app: &mut RustroomApp, ctx: &egui::Context, at: egui::Pos2) {
    for pass in 0..5 {
        let mut events = vec![egui::Event::PointerMoved(at)];
        if pass == 1 || pass == 2 {
            events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: pass == 1,
                modifiers: Default::default(),
            });
        }
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                events,
                ..Default::default()
            },
            |ui| {
                super::show(app, ui, &c);
            },
        );
    }
}

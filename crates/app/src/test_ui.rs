//! Finding and clicking widgets by **label**, headlessly.
//!
//! ⚠⚠ **This exists so tests never hardcode screen coordinates.** The first canvas tests wrote
//! `(200, 150)`, obtained by instrumenting the layout once — brittle in the worst way, because a
//! layout change moves the target silently and the failure mode is a test that still passes while
//! grabbing nothing. That fault occurred twice while those tests were being written.
//!
//! ⚠ It uses **AccessKit**, which egui already ships (eframe turns the feature on), so this costs
//! no new dependency. `egui_kittest` is a wrapper over exactly this mechanism; if the widget
//! queries here ever grow beyond "find a button by its label", that crate is the thing to adopt
//! rather than to reimplement.

use eframe::egui;

/// The screen rect every headless harness uses.
///
/// ⚠⚠ **Tall enough that a whole panel fits, which 300 pt no longer was — nor did 700 pt once a
/// mask's section gained noise reduction (2026-09-06).** These harnesses draw
/// a panel body into a *root* `Ui` with no scroll area, so a widget laid out past the bottom of
/// this rect is simply outside the window: `find` still reports its bounds from AccessKit — so
/// nothing panics — but `click` then presses a point egui ignores, and the test reads as "the
/// button did nothing". That is exactly how it failed when the Geometry panel grew by two rows:
/// `resetting_the_geometry_clears_the_perspective_too` broke while the button it clicks was
/// untouched. In production that panel is inside an `egui::ScrollArea`, so its length is not a
/// constraint on anything — a taller harness screen is the accurate model of "the user scrolled
/// to it", not a workaround.
///
/// ⚠ Deliberately separate from `develop::canvas::test_support::screen`, which stays 400×300:
/// the canvas tests derive real image placement from it, so its size is part of what they test.
pub fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 1400.0))
}

/// A context with AccessKit on, which is what makes [`find`] work.
pub fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx
}

/// Run one frame of `body`, returning the centre of every widget whose label matches, by label.
///
/// ⚠ Returns *centres*, not rects: a click needs a point, and the centre is the one point that is
/// inside a widget however it is laid out.
pub fn labelled_centres(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    body: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Pos2)> {
    labelled_centres_on(ctx, screen(), events, body)
}

/// [`labelled_centres`] on a screen of the caller's size — the top bar wants a wide one, since
/// at the panels' 400 pt width its centred line has no gap to sit in.
pub fn labelled_centres_on(
    ctx: &egui::Context,
    screen_rect: egui::Rect,
    events: Vec<egui::Event>,
    body: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Pos2)> {
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            events,
            ..Default::default()
        },
        body,
    );
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            // ⚠ A `ComboBox` has no label at all — egui gives it only its selected text as the
            // node's *value* (`combo_box.rs`, `current_text_value`) — so it is reachable here by
            // that text, which is also what a screen reader says for it (2026-09-19).
            let label = node
                .label()
                .map(str::to_owned)
                .or_else(|| node.value().map(str::to_owned))?;
            let b = node.bounds()?;
            Some((
                label,
                egui::pos2((b.x0 + b.x1) as f32 / 2.0, (b.y0 + b.y1) as f32 / 2.0),
            ))
        })
        .collect()
}

/// The numeric value of every widget that reports one — a `Slider` or a `DragValue` — after
/// laying `body` out once.
///
/// ⚠ A `DragValue` has **no AccessKit label**: egui gives it `Role::SpinButton` with a numeric
/// value and nothing else (`response.rs`, `WidgetInfo::drag_value`), so [`labelled_centres`]
/// cannot see it at all — found by a test that looked for a field by its value and got the
/// whole panel back without it. This is the query that can.
pub fn numeric_values(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) -> Vec<f64> {
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        body,
    );
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter_map(|(_, node)| node.numeric_value())
        .collect()
}

/// The label of every widget that also reports a number — in practice every `Slider`.
///
/// ⚠⚠ **A role filter, not a list of expected names**, and that is the whole point: it answers "what
/// does this panel call its sliders" for whatever sliders exist, so one added tomorrow is included
/// without anybody remembering to add it. The enumerating guards this project keeps being bitten by
/// are the alternative.
///
/// ⚠ A `DragValue` has a numeric value and **no** label, so it is absent here — see
/// [`numeric_values`].
pub fn slider_labels(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        body,
    );
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter(|(_, n)| n.numeric_value().is_some())
        .filter_map(|(_, n)| n.label().map(str::to_owned))
        .collect()
}

/// Where the widget labelled `label` sits, after laying `body` out once.
pub fn find(
    ctx: &egui::Context,
    label: &str,
    body: impl FnMut(&mut egui::Ui),
) -> Option<egui::Pos2> {
    labelled_centres(ctx, Vec::new(), body)
        .into_iter()
        .find(|(l, _)| l == label)
        .map(|(_, p)| p)
}

/// Lay out, then click the widget labelled `label`. Panics if no such widget was drawn.
///
/// ⚠⚠ **Three passes, and the count is load-bearing.** `Response` state is per-frame, so a
/// single-pass harness reads zeros — the mistake that made `a_left_drag_does_not_pan` pass
/// vacuously once. Pass 0 lays out and locates; pass 1 presses; pass 2 releases.
///
/// ⚠ Panicking on a missing label is deliberate. A silent miss is how a test ends up asserting
/// that nothing happened when nothing was clicked.
pub fn click(ctx: &egui::Context, label: &str, mut body: impl FnMut(&mut egui::Ui)) {
    let at = find(ctx, label, &mut body)
        .unwrap_or_else(|| panic!("no widget labelled {label:?} was drawn"));
    for pass in 0..2 {
        let events = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: pass == 0,
                modifiers: Default::default(),
            },
        ];
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                events,
                ..Default::default()
            },
            &mut body,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_is_found_by_its_label() {
        let ctx = ctx();
        let at = find(&ctx, "Add brush", |ui| {
            let _ = ui.button("Add brush");
        });
        assert!(at.is_some(), "the button should be locatable by label");
    }

    #[test]
    fn a_label_that_was_never_drawn_is_not_found() {
        // ⚠ The discriminating half: `find` returning `Some` for anything would make every
        // click test pass while clicking the wrong thing.
        let ctx = ctx();
        let at = find(&ctx, "Add radial filter", |ui| {
            let _ = ui.button("Add brush");
        });
        assert!(at.is_none());
    }

    #[test]
    fn clicking_by_label_reaches_the_button() {
        let ctx = ctx();
        let mut clicks = 0;
        click(&ctx, "Add brush", |ui| {
            if ui.button("Add brush").clicked() {
                clicks += 1;
            }
        });
        assert_eq!(clicks, 1, "exactly one click should have landed");
    }

    #[test]
    fn clicking_one_button_does_not_press_its_neighbour() {
        // ⚠ Two buttons in a row is the case a coordinate-based harness gets wrong, and it is
        // the whole reason this module exists.
        let ctx = ctx();
        let (mut first, mut second) = (0, 0);
        click(&ctx, "Add radial filter", |ui| {
            ui.horizontal(|ui| {
                if ui.button("Add brush").clicked() {
                    first += 1;
                }
                if ui.button("Add radial filter").clicked() {
                    second += 1;
                }
            });
        });
        assert_eq!((first, second), (0, 1));
    }
}

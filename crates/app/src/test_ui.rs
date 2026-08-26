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
pub fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
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
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen()),
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
            let label = node.label()?;
            let b = node.bounds()?;
            Some((
                label.to_string(),
                egui::pos2((b.x0 + b.x1) as f32 / 2.0, (b.y0 + b.y1) as f32 / 2.0),
            ))
        })
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

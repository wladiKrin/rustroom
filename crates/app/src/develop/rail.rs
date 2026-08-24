//! The Develop tool rail: Edit, Crop, Masks, Presets.
//!
//! ⚠ **Outermost on the right.** egui allocates side panels in call order, so this must be
//! shown *before* the adjustments panel or it lands between the panel and the canvas. The
//! ordering rule is documented on `develop::body` and is load-bearing.

use eframe::egui;
use rustroom_app::tool::Tool;

/// Rail width in points. Wide enough for "Presets" at the small text size.
const RAIL_WIDTH: f32 = 64.0;

/// Draw the rail.
///
/// ⚠ Returns nothing on purpose. `body` detects a tool change by comparing against the
/// previous value, which covers the keyboard shortcuts too — a return value here would catch
/// only clicks, and the 1–4 keys would then switch tools without re-rendering.
pub fn show(ui: &mut egui::Ui, tool: &mut Tool) {
    egui::Panel::right("rail")
        .resizable(false)
        .default_size(RAIL_WIDTH)
        .show(ui, |ui| {
            ui.vertical_centered_justified(|ui| {
                for t in Tool::ALL {
                    let picked = *tool == t;
                    let text = format!("{}\n{}", t.glyph(), t.label());
                    if ui.selectable_label(picked, text).clicked() {
                        *tool = t;
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rail_glyph_is_renderable() {
        // ⚠⚠ The test that makes glyphs safe at all, and it is not hypothetical: the design
        // spec's own candidates — ▦ ⧉ ◐ ≡ — are ALL absent from egui's bundled fonts and
        // would have shipped as tofu boxes. egui bundles Hack, Ubuntu-Light, NotoEmoji and
        // emoji-icon-font; nothing guarantees an arbitrary Geometric-Shapes character is in
        // any of them, and a missing glyph is invisible to every other test in this suite.
        //
        // ⚠ A frame must run before the fonts exist — `Context::default()` has not built its
        // atlas yet, so asking a fresh context would answer about nothing.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for t in Tool::ALL {
            let c = t.glyph();
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{:?}'s glyph {c} (U+{:04X}) is not in any bundled font and would render as \
                 a tofu box",
                t,
                c as u32
            );
        }
    }

    /// Click the rail entry for `want` and report the selection afterwards.
    ///
    /// Three passes: lay out with the pointer in place, press, release — egui resolves a
    /// click against the widgets registered on the *previous* pass. A single-pass harness
    /// reads zeros and passes vacuously, which is exactly how `a_left_drag_does_not_pan` was
    /// once inert.
    ///
    /// ⚠ Re-declares the rail body rather than calling `show`, because the harness must
    /// observe the click on a specific pass. Keep the two in step: if `show`'s widget stops
    /// being a `selectable_label`, this must change with it or it stops describing what ships.
    fn click_rail(want: Tool) -> Tool {
        let ctx = egui::Context::default();
        let mut tool = Tool::Edit;
        let mut target = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(target));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                egui::Panel::right("rail").show(ui, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        for t in Tool::ALL {
                            let picked = tool == t;
                            let text = format!("{}\n{}", t.glyph(), t.label());
                            let r = ui.selectable_label(picked, text);
                            if t == want && pass == 0 {
                                target = r.rect.center();
                            }
                            if r.clicked() {
                                tool = t;
                            }
                        }
                    });
                });
            });
        }
        tool
    }

    #[test]
    fn clicking_a_rail_entry_selects_that_tool() {
        // ⚠ Canvas and panel wiring is the area this suite does not cover — six bugs in the
        // brush work, every one there, none caught by ~30 tests on the pure functions beside
        // it. Every non-default tool is exercised, because a rail stuck on its default would
        // pass a test that only ever clicked Edit.
        assert_eq!(click_rail(Tool::Crop), Tool::Crop);
        assert_eq!(click_rail(Tool::Masks), Tool::Masks);
        assert_eq!(click_rail(Tool::Presets), Tool::Presets);
    }
}

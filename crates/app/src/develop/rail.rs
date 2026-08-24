//! The Develop tool rail: Edit, Crop, Masks, Presets.
//!
//! ⚠ **Outermost on the right.** egui allocates side panels in call order, so this must be
//! shown *before* the adjustments panel or it lands between the panel and the canvas. The
//! ordering rule is documented on `develop::body` and is load-bearing.

use eframe::egui;
use rustroom_app::tool::Tool;

/// Rail width in points.
///
/// ⚠ Sized so **"Presets" fits on one line** at [`LABEL_SIZE`]. At 64.0 it wrapped across two
/// lines, which is what made the rail look broken rather than merely narrow.
const RAIL_WIDTH: f32 = 88.0;

/// The glyph's point size — deliberately much larger than the label, so the rail reads as an
/// icon strip rather than as two lines of text.
const GLYPH_SIZE: f32 = 22.0;

/// The label's point size. Larger than `theme`'s compact body text: the rail is a primary
/// navigation control, and the compact density exists for panels of sliders, not for this.
const LABEL_SIZE: f32 = 13.0;

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
                    let entry = egui::Button::selectable(picked, entry_text(ui, t, picked))
                        // ⚠ Extend, not Wrap: "Presets" wrapping onto a second line was the
                        // visible symptom. The rail is sized to fit instead.
                        .wrap_mode(egui::TextWrapMode::Extend);
                    if ui.add(entry).clicked() {
                        *tool = t;
                    }
                }
            });
        });
}

/// The two-line label for one rail entry: the glyph above the name.
///
/// ⚠ **A `LayoutJob` with `halign: Center`, not a `"{glyph}\n{label}"` string.** A plain
/// newline leaves the two rows left-aligned against each other, so a one-character glyph sits
/// hard left of a seven-character word — which is exactly how the first version looked wrong.
/// `halign` centres each row against the widest, putting the glyph over the middle of the name.
///
/// The surrounding `vertical_centered_justified` layout then centres the whole block in the
/// rail, because `AtomLayout` takes its default alignment from `ui.layout()`.
fn entry_text(ui: &egui::Ui, t: Tool, picked: bool) -> egui::WidgetText {
    // Selected entries sit on the accent fill, where the ordinary body colour is low contrast.
    let color = if picked {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let mut job = egui::text::LayoutJob {
        halign: egui::Align::Center,
        ..Default::default()
    };
    job.append(
        &t.glyph().to_string(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(GLYPH_SIZE),
            color,
            ..Default::default()
        },
    );
    job.append(
        &format!("\n{}", t.label()),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(LABEL_SIZE),
            color,
            ..Default::default()
        },
    );
    job.into()
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
    /// observe the click on a specific pass. It builds the entry through the **same
    /// `entry_text`** production uses, so the two cannot drift in the part that matters — the
    /// hit rect. If `show`'s widget changes, this must change with it or it stops describing
    /// what ships.
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
                            let r = ui.add(
                                egui::Button::selectable(picked, entry_text(ui, t, picked))
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
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

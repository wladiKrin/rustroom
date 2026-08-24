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

/// Vertical padding above the glyph and below the label.
const PAD: f32 = 6.0;

/// The gap between glyph and label.
const GAP: f32 = 2.0;

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
            for t in Tool::ALL {
                if entry(ui, t, *tool == t).clicked() {
                    *tool = t;
                }
            }
        });
}

/// One rail entry: the glyph centred over the label, in a full-width clickable frame.
///
/// ⚠ **Painted by hand rather than built as a `Button` with a two-line label**, and the two
/// failed attempts are worth knowing. A `"{glyph}\n{label}"` string leaves the rows
/// left-aligned against each other, so a one-character glyph sits hard left of a
/// seven-character word. Fixing that with a `LayoutJob { halign: Center }` centres the *rows*
/// but positions them about x = 0, so the galley's own rect starts negative and the whole
/// block then sits left inside the button frame. Painting each line at the frame's centre with
/// `Align2::CENTER_TOP` has neither problem and needs no assumptions about how egui aligns
/// widget content.
fn entry(ui: &mut egui::Ui, t: Tool, picked: bool) -> egui::Response {
    let height = PAD * 2.0 + GLYPH_SIZE + GAP + LABEL_SIZE;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );

    let visuals = ui.style().interact_selectable(&resp, picked);
    let fill = if picked {
        ui.visuals().selection.bg_fill
    } else if resp.hovered() {
        visuals.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, visuals.corner_radius, fill);
    }

    // Selected entries sit on the accent fill, where the ordinary body colour is low contrast.
    let color = if picked {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let cx = rect.center().x;
    let painter = ui.painter();
    painter.text(
        egui::pos2(cx, rect.top() + PAD),
        egui::Align2::CENTER_TOP,
        t.glyph(),
        egui::FontId::proportional(GLYPH_SIZE),
        color,
    );
    painter.text(
        egui::pos2(cx, rect.top() + PAD + GLYPH_SIZE + GAP),
        egui::Align2::CENTER_TOP,
        t.label(),
        egui::FontId::proportional(LABEL_SIZE),
        color,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rail_glyph_is_renderable() {
        // ⚠⚠ Still the guard that makes icons safe, now against the bundled font rather than
        // egui's defaults. It began life catching the design spec's own candidates — ▦ ⧉ ◐ ≡ —
        // which are all absent from egui's stack and would have shipped as tofu boxes. With a
        // real icon font the same test catches a font that failed to embed, or a codepoint that
        // moved in a version bump.
        //
        // ⚠ A frame must run before the fonts exist — `Context::default()` has not built its
        // atlas yet, so asking a fresh context would answer about nothing.
        let ctx = egui::Context::default();
        // ⚠ Without this the test asks egui's *default* stack, which production no longer uses —
        // it would fail on every icon while the app renders them perfectly. The test is only
        // meaningful against the fonts that ship.
        crate::theme::install_fonts(&ctx);
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
    /// ⚠ Re-declares the rail's *loop* rather than calling `show`, because the harness must
    /// observe the click on a specific pass — but it calls the **same `entry`** production
    /// does, so the widget under test is literally the shipped one. Only the panel wrapper
    /// differs.
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
                    for t in Tool::ALL {
                        let picked = tool == t;
                        let r = entry(ui, t, picked);
                        if t == want && pass == 0 {
                            target = r.rect.center();
                        }
                        if r.clicked() {
                            tool = t;
                        }
                    }
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

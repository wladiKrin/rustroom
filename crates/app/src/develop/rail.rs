//! The Develop tool rail: one icon per tool, its name on hover.
//!
//! ⚠ **Outermost on the right.** egui allocates side panels in call order, so this must be
//! shown *before* the adjustments panel or it lands between the panel and the canvas. The
//! ordering rule is documented on `develop::body` and is load-bearing.

use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::tool::Tool;

/// Rail width in points.
///
/// ⚠ Icon-only since 2026-09-19: the labels the first rail was sized for ("Presets fits on
/// one line" at 88 pt) are the tooltip now, and the width is what the glyph and its padding
/// need.
const RAIL_WIDTH: f32 = 48.0;

/// The glyph's point size.
///
/// ⚠ 26, up from 22 (2026-09-19, user's call): the icons are the rail's only content and
/// should read as its dominant element rather than as marks in a strip.
const GLYPH_SIZE: f32 = crate::theme::tokens::ICON_LARGE;

/// Padding around the glyph: a 32-pt button around a 24-pt glyph, `docs/DESIGN.md` §7.11.
const PAD: f32 = 4.0;

/// How much of the accent the selected entry's tint carries.
///
/// ⚠ A tint, not a solid block and not a bar: the accent should not shout beside the image —
/// the visual identity spec's "low chroma near the image". The bar was drafted and rejected.
const TINT: f32 = crate::theme::tokens::TINT;

/// Draw the rail.
///
/// ⚠ Returns nothing on purpose. `body` detects a tool change by comparing against the
/// previous value, which covers the keyboard shortcuts too — a return value here would catch
/// only clicks, and the 1–4 keys would then switch tools without re-rendering.
pub fn show(ui: &mut egui::Ui, tool: &mut Tool) {
    egui::Panel::right("rail")
        .resizable(false)
        .default_size(RAIL_WIDTH)
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(4))
        .show(ui, |ui| {
            for t in Tool::ALL {
                if entry(ui, t, *tool == t).clicked() {
                    *tool = t;
                }
            }
        });
}

/// One rail entry: the glyph centred in a full-width clickable frame, its name on hover.
///
/// ⚠ The name is set **twice**, and both matter: `on_hover_text` is what a sighted user gets,
/// `widget_info` is what a screen reader and `test_ui` get. Dropping either leaves one of them
/// with an unlabelled button.
///
/// ⚠ Painted by hand rather than built as a `Button`: a button's frame follows the widget
/// visuals, and the selected state here is a *tint* of the accent rather than any of them.
fn entry(ui: &mut egui::Ui, t: Tool, picked: bool) -> egui::Response {
    let height = PAD * 2.0 + GLYPH_SIZE;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let resp = resp.hover(t.label());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, t.label()));

    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    let (fill, colour) = if picked {
        (accent.gamma_multiply(TINT), accent)
    } else if resp.hovered() {
        (v.widgets.hovered.weak_bg_fill, v.text_color())
    } else {
        // ⚠ The secondary grey at rest, so the active tool's accent is the only strong glyph.
        (
            egui::Color32::TRANSPARENT,
            crate::theme::tokens::of(ui).text2,
        )
    };
    let painter = ui.painter();
    if fill != egui::Color32::TRANSPARENT {
        painter.rect_filled(rect.shrink(2.0), crate::theme::tokens::RADIUS, fill);
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        t.glyph(),
        egui::FontId::proportional(GLYPH_SIZE),
        colour,
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

    #[test]
    fn every_rail_entry_is_labelled_for_accessibility() {
        // ⚠ The rail paints no text any more; the tool's name reaches a sighted user as a
        // tooltip and everyone else — a screen reader, this harness — as the accessible name.
        // Sabotage: drop `widget_info` and every label vanishes.
        let ctx = crate::test_ui::ctx();
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            for t in Tool::ALL {
                let _ = entry(ui, t, false);
            }
        });
        for t in Tool::ALL {
            assert!(
                found.iter().any(|(l, _)| l == t.label()),
                "{} has no accessible name: {found:?}",
                t.label()
            );
        }
    }
}

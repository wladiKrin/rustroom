//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod canvas_geom;
pub mod panel;
pub mod rail;

use crate::commands;
use crate::{filmstrip, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::icons;
use rustroom_app::view_region;

/// The toolbar icons' point size.
///
/// ⚠ Larger than body text on purpose — see [`icon_text`]. Still smaller than the rail's 22 pt,
/// because a toolbar icon sits in a row of its own height while a rail entry has a label under
/// it and a whole 88 pt column to fill.
const ICON_SIZE: f32 = 17.0;
use rustroom_app::menu::Command;

/// The Develop editor body: preset panel, adjustments panel, filmstrip, canvas.
///
/// ⚠ `photo_edited` is collected from both halves and recorded **once**, after they have both
/// run. Each half sets it inside a closure that already holds `&mut app` through `app.session`,
/// so neither can touch the action log itself — which is why they return a `bool` instead.
///
/// ⚠ **Panel order is load-bearing.** egui allocates side panels in call order and the central
/// panel takes what is left, so rail (right, outermost) → adjustments (right) → filmstrip
/// (bottom) → canvas (central) must be preserved exactly. The rail coming *first* is what puts
/// it against the window edge rather than between the panel and the image.
/// Delete or Backspace removes the selected retouch spot. `true` when one was removed.
///
/// ⚠ **Gated on nothing having keyboard focus**, the rule every bare key in this app follows:
/// without it, Backspace in the Save-preset name field would delete a spot instead of a
/// character.
///
/// ⚠ And on the Retouch tool being active, so the key means nothing anywhere else — a Delete that
/// silently removed a patch while the user was moving a slider would be unattributable.
fn delete_selected_spot(app: &mut RustroomApp, ctx: &egui::Context) -> bool {
    if app.tool != rustroom_app::tool::Tool::Retouch || ctx.memory(|m| m.focused()).is_some() {
        return false;
    }
    let pressed =
        ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));
    if !pressed {
        return false;
    }
    let Some(s) = app.session.as_mut() else {
        return false;
    };
    if rustroom_app::spot_tool::delete_selected(&mut s.edits.spots, &mut s.selected_spot) {
        s.request(
            s.view_edits(rustroom_app::tool::Tool::Retouch),
            s.render_max_dim(),
            s.requested_region,
        );
        s.history.push(s.edits.clone());
        true
    } else {
        false
    }
}

pub fn body(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    // ⚠ The rail writes into a local, which is then applied through `select_tool` — the one
    // place that owns the re-render. The 1–4 keys and the Photo menu arrive through the same
    // method, so none of the three can switch tools without re-rendering.
    let mut picked = app.tool;
    if app.session.is_some() {
        rail::show(ui, &mut picked);
    }
    // ⚠⚠ **The one derivation site for mask isolation, and it runs BEFORE `select_tool`.**
    // Recomputed every frame from the same three facts that decide whether the tint is drawn, so
    // the suppression and the tint cannot disagree and no second owner can appear. See
    // `Session::suppressed_mask`.
    //
    // ⚠⚠ **Keyed on `picked`, not `app.tool`, and placed above `select_tool` — because
    // `select_tool` owns the re-render for a tool change and would otherwise request with the
    // suppression of the tool being LEFT.** Reported in use, both directions at once: leaving
    // Masks rendered without the mask's edits, so they appeared only when something else forced a
    // frame; arriving at Masks rendered *with* them, so they sat under the tint until the same.
    // One ordering, two symptoms.
    //
    // ⚠ Also before both panels: each requests renders through `view_edits`, and a value written
    // after them would be one frame stale exactly where it matters.
    // ⚠ Every mask whose eye is on, not the selected one: the overlay is per mask since
    // 2026-09-19, and the picture must hold out exactly the masks it tints.
    let suppressed: Vec<usize> = if picked == rustroom_app::tool::Tool::Masks {
        app.session
            .as_ref()
            .map(|s| s.shown_masks())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // ⚠⚠ **A change guard that ASKS FOR A RENDER, which the first version did not.** The
    // suppression decides what `view_edits` returns, so a change to it changes the picture — and
    // nothing else necessarily asks. `select_tool` covers a tool change; ticking "Show mask" and
    // selecting a different mask change it with no tool change at all, and those went unrendered
    // until some unrelated edit happened to request a frame.
    let tool_for_request = picked;
    if let Some(s) = app.session.as_mut() {
        if s.suppressed_masks != suppressed {
            s.suppressed_masks = suppressed;
            // ⚠⚠ **One rung lower first, so the switch is seen at once and sharpens after.** A
            // full-quality render of this is 95-300 ms on a large window — measured, with a
            // single mask roughly doubling it — and that is felt as a delay between the click and
            // the picture. Reported as switching modes "not being great".
            //
            // ⚠ It needs no settle timer and no new state: setting `render_rung` to the dropped
            // value makes the canvas's own `max_dim != s.render_rung` disjunction request full
            // quality on the very next frame. The same mechanism the interactive rung uses, and
            // the reason that one has no timer either.
            //
            // ⚠⚠ **This is the 2026-08-13 progressive-fit feature, deliberately, and the reason
            // it was removed does not apply.** That one was deleted because "the user could not
            // see it on or off" — it traded a 1.44x residual nobody noticed. Here the trade is a
            // 95-300 ms wait against a 2x-softer frame for one or two frames, and the wait is
            // what was reported. ⚠ It also cannot oscillate the way that one did: the gate is a
            // change in `suppressed_mask`, which no render can affect.
            let soft = view_region::rung_below(s.render_rung, s.fit_cap(), s.buffer_long, 1);
            s.render_rung = soft;
            // ⚠ The baton: without it the canvas, which runs after the panels, would see the rung
            // change on this very frame and ask for full quality straight away — superseding the
            // soft render before it could be shown. See `Session::soft_next`.
            s.soft_next = true;
            s.request(s.view_edits(tool_for_request), soft, s.requested_region);
        }
    }
    app.select_tool(picked);
    photo_edited |= panel::show(app, ui);
    if app.show_filmstrip && app.library.is_some() {
        filmstrip::show(app, ui, ctx);
    }
    photo_edited |= canvas::show(app, ui, ctx);
    photo_edited |= delete_selected_spot(app, ctx);

    // Recorded in the same order as everything else, so "delete a preset, move a slider,
    // Ctrl+Z twice" undoes the slider and then restores the preset.
    if photo_edited {
        if let Some(id) = app.open_photo() {
            app.actions
                .push(ActionKind::PhotoEdit { id: id.clone() }, Some(&id));
        }
    }
}

/// Develop-mode toolbar buttons (shown in the top panel when a photo is open).
///
/// ⚠ **Six controls.** Everything else moved into the menus; what is left is either
/// high-frequency (undo, zoom, before/after) or a mode switch. Each also appears in a menu, on
/// purpose: a menu is for reach and discovery, a button is for speed.
pub fn toolbar(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // Undo/Redo drive the unified `ActionLog`, so one control walks back through photo edits
    // and preset deletions in the order they actually happened. ⚠ The *tooltip* says which,
    // so an icon button never silently does the surprising one — that wording used to be the
    // button's label and must not be lost with it.
    let open = app.open_photo();
    let undo_hint = match app.actions.peek_undo(open.as_ref()) {
        Some(ActionKind::PresetDelete { name }) => format!("Undo delete “{name}”"),
        _ => "Undo".to_string(),
    };
    let undo_click = icon(
        ui,
        icons::UNDO,
        &undo_hint,
        app.actions.can_undo(open.as_ref()),
    );
    let redo_click = icon(ui, icons::REDO, "Redo", app.actions.can_redo(open.as_ref()));
    if undo_click {
        commands::execute(app, ctx, Command::Undo);
    }
    if redo_click {
        commands::execute(app, ctx, Command::Redo);
    }

    if app.session.is_some() {
        ui.separator();
        // ⚠ Real magnifying glasses, from the bundled font. These were `⊞`/`⊟` and a text
        // "1:1" until the icon font landed, because egui's own fonts contain no magnifying
        // glass at all. ⚠ `ZOOM_FIT` is a frame rather than a fourth glass — Phosphor has
        // exactly three glass variants, and a frame is the conventional fit symbol anyway.
        let zoom_out = icon(ui, icons::ZOOM_OUT, "Zoom out", true);
        let zoom_in = icon(ui, icons::ZOOM_IN, "Zoom in", true);
        let zoom_fit = icon(ui, icons::ZOOM_FIT, "Zoom to fit", true);
        let zoom_actual = icon(ui, icons::ZOOM_ACTUAL, "Zoom to 1:1", true);
        if zoom_out {
            commands::execute(app, ctx, Command::ZoomOut);
        }
        if zoom_in {
            commands::execute(app, ctx, Command::ZoomIn);
        }
        if zoom_fit {
            commands::execute(app, ctx, Command::FitView);
        }
        if zoom_actual {
            commands::execute(app, ctx, Command::ZoomActual);
        }

        ui.separator();
        // ⚠ Reads the live flag and dispatches on a change rather than writing it, so the
        // button and the View-menu item are one implementation.
        let mut before_after = app.session.as_ref().is_some_and(|s| s.show_before);
        if ui
            .toggle_value(&mut before_after, icon_text(icons::BEFORE_AFTER))
            .on_hover_text("Before / After")
            .clicked()
        {
            commands::execute(app, ctx, Command::ToggleBeforeAfter);
        }
    }
    // ⚠ The filename is **not** drawn here. `shell` owns the row's right-aligned cluster, so
    // that the status line and the filename cannot fight over the same remaining space — which
    // is exactly what happened when this function right-aligned it itself.
}

/// A toolbar icon button with a tooltip. Returns whether it was clicked.
///
/// ⚠ The tooltip is not decoration: it is the only thing naming the action once the label is a
/// glyph, so every caller must supply a real one.
fn icon(ui: &mut egui::Ui, glyph: char, hint: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(icon_text(glyph)))
        .on_hover_text(hint)
        .clicked()
}

/// One toolbar icon at [`ICON_SIZE`].
///
/// ⚠ Sized explicitly rather than inheriting the body text size. `theme::tune` deliberately
/// makes body text compact, because the panel stacks twenty-odd sliders — but an icon carries
/// the *whole* meaning of its button, where a word has letters to be recognised by, so the same
/// size reads as too small. One helper, so the toolbar cannot end up with two icon sizes.
fn icon_text(glyph: char) -> egui::RichText {
    egui::RichText::new(glyph).size(ICON_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the real `develop::body` and report what `suppressed_mask` came out as.
    fn suppression_under(
        tool: rustroom_app::tool::Tool,
        shown: bool,
        selected: Option<usize>,
    ) -> Vec<usize> {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = tool;
        {
            let s = app.session.as_mut().expect("session");
            s.edits.masks = vec![edit_model::MaskedAdjustment::default()];
            s.selected_mask = selected;
            s.mask_shown = vec![shown];
        }
        let c = ctx.clone();
        // ⚠ A 1400 pt screen, not 700. `test_ui::screen`'s own doc records that a widget laid out
        // past the bottom of the rect is simply outside the window and reads as "the control did
        // nothing"; the mask panel already grew past 700 once.
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1400.0),
                )),
                ..Default::default()
            },
            |ui| body(&mut app, ui, &c),
        );
        app.session
            .as_ref()
            .expect("session")
            .suppressed_masks
            .clone()
    }

    /// Run one real `body` frame and report `(suppressed_mask, renders issued)`.
    fn frame(app: &mut crate::RustroomApp, ctx: &egui::Context) -> (Vec<usize>, usize) {
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1400.0),
                )),
                ..Default::default()
            },
            |ui| body(app, ui, &c),
        );
        let s = app.session.as_ref().expect("session");
        (s.suppressed_masks.clone(), s.engine.requests_issued())
    }

    /// ⚠⚠ **Reported in use, both directions at once.** Leaving the Masks tool rendered *without*
    /// the mask's edits and arriving at it rendered *with* them, each correcting itself only when
    /// something unrelated forced a frame. Two symptoms, one cause: the suppression was stored and
    /// updated after `select_tool`, which owns the re-render for a tool change — so every tool
    /// change requested with the suppression of the tool being left. It is derived above
    /// `select_tool` now, from `picked`.
    ///
    /// ⚠ The second half is the change guard. Ticking "Show mask" or selecting a different mask
    /// changes what `view_edits` returns with **no tool change at all**, so nothing asked for a
    /// render and the screen kept the old answer.
    #[test]
    fn a_change_of_suppression_asks_for_a_render() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Masks;
        {
            let s = app.session.as_mut().expect("session");
            s.edits.masks = vec![edit_model::MaskedAdjustment::default()];
            s.selected_mask = Some(0);
            s.mask_shown = vec![true];
        }
        // Settle: the suppression is established and whatever renders it wants are issued.
        let _ = frame(&mut app, &ctx);
        let (before_mask, before_reqs) = frame(&mut app, &ctx);
        assert_eq!(
            before_mask,
            vec![0],
            "fixture check: the mask must be suppressed before the toggle, or this proves nothing"
        );

        // Untick "Show mask" — no tool change, so `select_tool` will not fire.
        app.session.as_mut().expect("session").mask_shown = vec![false];
        let (after_mask, after_reqs) = frame(&mut app, &ctx);
        assert!(
            after_mask.is_empty(),
            "the suppression must have been lifted"
        );
        assert!(
            after_reqs > before_reqs,
            "lifting the suppression changed the picture and asked for no render, so the mask's \
             edits stay invisible until something unrelated forces a frame"
        );
    }

    /// ⚠⚠ **The toggle renders one rung lower first, then full quality.** A full-quality render
    /// of this is 95-300 ms on a large window, felt as a delay between the click and the picture.
    ///
    /// ⚠ Both halves, and neither alone would do: asserting only the drop would pass against a
    /// version that never sharpened again, and asserting only the recovery would pass against one
    /// that never dropped and simply took the full 300 ms.
    #[test]
    fn a_suppression_change_renders_soft_first_and_sharpens_on_the_next_frame() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Masks;
        {
            let s = app.session.as_mut().expect("session");
            s.edits.masks = vec![edit_model::MaskedAdjustment::default()];
            s.selected_mask = Some(0);
            s.mask_shown = vec![true];
            // A ladder with somewhere to drop to: the fixture's own buffer gives one rung, where
            // `rung_below` returns the rung unchanged and this test could see nothing.
            s.buffer_long = 3200;
        }
        let _ = frame(&mut app, &ctx);
        let _ = frame(&mut app, &ctx);
        let settled = app.session.as_ref().expect("session").render_rung;
        assert_eq!(
            settled, 3200,
            "fixture check: the view must settle on a rung with one below it, or a drop is \
             unrepresentable and this test proves nothing"
        );

        app.session.as_mut().expect("session").mask_shown = vec![false];
        let (_, _) = frame(&mut app, &ctx);
        assert_eq!(
            app.session.as_ref().expect("session").render_rung,
            1600,
            "the toggle must render a rung lower first, so the switch is seen at once"
        );
        let (_, _) = frame(&mut app, &ctx);
        assert_eq!(
            app.session.as_ref().expect("session").render_rung,
            3200,
            "full quality must come back on the very next frame, with no settle timer"
        );
    }

    /// ⚠⚠ **The carrier.** The three facts reaching `Session::suppressed_mask` through the real
    /// `body`, which is the hop nothing else proves — every `view_edits` test would pass against
    /// a version where nobody ever writes the field. That is exactly how creative LUTs shipped
    /// fully tested and 100% non-functional.
    #[test]
    fn a_shown_mask_is_suppressed_under_the_masks_tool_whether_or_not_selected() {
        // ⚠ Selection no longer enters into it: the eye is per mask, and a mask you are not
        // editing can be shown too.
        assert_eq!(
            suppression_under(rustroom_app::tool::Tool::Masks, true, Some(0)),
            vec![0]
        );
        assert_eq!(
            suppression_under(rustroom_app::tool::Tool::Masks, true, None),
            vec![0]
        );
    }

    #[test]
    fn nothing_is_suppressed_under_another_tool_or_with_the_eye_off() {
        assert!(
            suppression_under(rustroom_app::tool::Tool::Edit, true, Some(0)).is_empty(),
            "the Edit panel draws no tint, so nothing may be hidden there"
        );
        assert!(
            suppression_under(rustroom_app::tool::Tool::Masks, false, Some(0)).is_empty(),
            "with the eye off the mask's effect is the only feedback there is"
        );
    }

    #[test]
    fn every_toolbar_glyph_is_renderable() {
        // ⚠ Sweeps `icons::ALL` rather than a table local to this file. A second list would
        // drift from the first, and the drift would be invisible: an icon dropped from the
        // local table is simply one that nothing checks.
        //
        // ⚠ Installs the real font stack, because production no longer uses egui's default one.
        // Without it this fails on every icon while the app renders them perfectly.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for c in icons::ALL {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "icon U+{:04X} is not in the bundled font",
                c as u32
            );
        }
    }
}

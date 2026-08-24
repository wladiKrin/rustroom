//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod panel;
pub mod rail;

use crate::commands;
use crate::{filmstrip, theme, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
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
    app.select_tool(picked);
    photo_edited |= panel::show(app, ui);
    if app.show_filmstrip && app.library.is_some() {
        filmstrip::show(app, ui, ctx);
    }
    photo_edited |= canvas::show(app, ui, ctx);

    // Recorded in the same order as everything else, so "delete a preset, move a slider,
    // Ctrl+Z twice" undoes the slider and then restores the preset.
    if photo_edited {
        if let Some(path) = app.open_photo() {
            // ⚠ `Some(path.as_path())`, not `Some(&path)`: the parameter is `Option<&Path>`,
            // and deref coercion does not reach inside `Some`.
            app.actions.push(
                ActionKind::PhotoEdit { path: path.clone() },
                Some(path.as_path()),
            );
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
    let undo_hint = match app.actions.peek_undo(open.as_deref()) {
        Some(ActionKind::PresetDelete { name }) => format!("Undo delete “{name}”"),
        _ => "Undo".to_string(),
    };
    let undo_click = icon(ui, "⟲", &undo_hint, app.actions.can_undo(open.as_deref()));
    let redo_click = icon(ui, "⟳", "Redo", app.actions.can_redo(open.as_deref()));
    if undo_click {
        commands::execute(app, ctx, Command::Undo);
    }
    if redo_click {
        commands::execute(app, ctx, Command::Redo);
    }

    if app.session.is_some() {
        ui.separator();
        // ⚠ No magnifying glass exists in egui's bundled fonts — 🔍, 🔎 and ⌕ are all absent
        // and would ship as tofu boxes, the same trap the rail's glyphs fell into. `⊞`/`⊟`
        // (plus/minus in a box) and `⛶` (frame corners) do render; 1:1 has no glyph at all in
        // those fonts, so it keeps its conventional text label. `every_toolbar_glyph_is_renderable`
        // is the guard.
        let zoom_out = icon(ui, "⊟", "Zoom out", true);
        let zoom_in = icon(ui, "⊞", "Zoom in", true);
        let zoom_fit = icon(ui, "⛶", "Zoom to fit", true);
        let zoom_actual = icon(ui, "1:1", "Zoom to 1:1", true);
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
            .toggle_value(&mut before_after, "◑")
            .on_hover_text("Before / After")
            .clicked()
        {
            commands::execute(app, ctx, Command::ToggleBeforeAfter);
        }

        // ⚠ The filename, right-aligned. The row that used to hold fourteen controls now
        // carries the one piece of context worth always seeing.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(s) = &app.session {
                if let Some(name) = s.path.file_name() {
                    theme::caption(ui, &name.to_string_lossy());
                }
            }
        });
    }
}

/// A toolbar icon button with a tooltip. Returns whether it was clicked.
///
/// ⚠ The tooltip is not decoration: it is the only thing naming the action once the label is a
/// glyph, so every caller must supply a real one.
fn icon(ui: &mut egui::Ui, glyph: &str, hint: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(glyph))
        .on_hover_text(hint)
        .clicked()
}

/// Every glyph the toolbar paints, for the coverage test.
#[cfg(test)]
const TOOLBAR_GLYPHS: [&str; 5] = ["⟲", "⟳", "⊟", "⊞", "⛶"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_toolbar_glyph_is_renderable() {
        // ⚠ The same guard the rail carries, for the same reason: the obvious icons for these
        // actions — 🔍 for zoom, ↩/↪ for undo, ◧ for a before/after split — are ALL absent
        // from egui's bundled fonts and would ship as tofu boxes. A missing glyph is invisible
        // to every other test here.
        //
        // ⚠ A frame must run first: a fresh `Context` has not built its font atlas.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for g in TOOLBAR_GLYPHS {
            for c in g.chars() {
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                    "toolbar glyph {c} (U+{:04X}) is not in any bundled font",
                    c as u32
                );
            }
        }
    }

    #[test]
    fn the_before_after_glyph_is_renderable() {
        // Separate from the button table because it lives on a `toggle_value`, not on `icon`.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        assert!(ctx.fonts_mut(|f| f.has_glyph(&id, '◑')));
    }
}

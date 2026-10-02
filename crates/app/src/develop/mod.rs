//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod ambient;
pub mod canvas;
pub mod canvas_geom;
pub mod panel;
pub mod rail;

use crate::commands;
use crate::{filmstrip, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::icons;

/// The toolbar icons' point size.
///
/// ⚠ Larger than body text on purpose — see [`icon_text`]. Still smaller than the rail's 22 pt,
/// because a toolbar icon sits in a row of its own height while a rail entry has a label under
/// it and a whole 88 pt column to fill.
const ICON_SIZE: f32 = crate::theme::tokens::ICON;
use crate::theme::Hover as _;
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
/// Delete or Backspace removes the selected retouch spot or eye (M9.11). `true` when one was
/// removed.
///
/// ⚠ **Gated on nothing having keyboard focus**, the rule every bare key in this app follows:
/// without it, Backspace in the Save-preset name field would delete a spot instead of a
/// character.
///
/// ⚠ And on the Retouch tool being active, so the key means nothing anywhere else — a Delete that
/// silently removed a patch while the user was moving a slider would be unattributable.
fn delete_selected_retouch(app: &mut RustroomApp, ctx: &egui::Context) -> bool {
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
    // ⚠ M9.11: an eye and a patch are never selected together, so Delete has one target.
    if let Some(i) = s.selected_eye.filter(|&i| i < s.edits.eyes.len()) {
        s.edits.eyes.remove(i);
        s.selected_eye = None;
        if s.suppressed_eye == Some(i) {
            s.suppressed_eye = None;
        }
        s.request(
            s.view_edits(rustroom_app::tool::Tool::Retouch),
            s.render_max_dim(),
            s.requested_region,
        );
        return s.history.push(s.edits.clone());
    }
    let removed = s.selected_spot;
    if rustroom_app::spot_tool::delete_selected(&mut s.edits.spots, &mut s.selected_spot) {
        // ⚠ The row flags go with the patch, or the next patch inherits its eye and hold-out.
        if let Some(i) = removed {
            if i < s.spot_shown.len() {
                s.spot_shown.remove(i);
            }
            if i < s.spot_held.len() {
                s.spot_held.remove(i);
            }
        }
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
    use rustroom_app::layout::Piece;
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    // ⚠ The rail writes into a local, which is then applied through `select_tool` — the one
    // place that owns the re-render. The 1–4 keys and the Photo menu arrive through the same
    // method, so none of the three can switch tools without re-rendering.
    let mut picked = app.tool;
    // ⚠⚠ M10.8b: in `develop_order` — outermost first, since egui gives a side panel the room the
    // ones before it left. A strip on the edit panel's side comes first and sits at the window edge;
    // a bottom strip comes after the side panels and spans only the photo column.
    let layout = app.settings.layout;
    for piece in rustroom_app::layout::develop_order(&layout) {
        match piece {
            Piece::ToolRail => {
                if app.session.is_some() && app.show_panels {
                    rail::show(ui, &mut picked, layout.edit_panel);
                }
                app.select_tool(picked);
            }
            Piece::EditPanel => {
                if app.show_panels {
                    photo_edited |= panel::show(app, ui);
                }
            }
            Piece::Filmstrip => {
                if app.show_filmstrip && app.show_panels && app.library.is_some() {
                    filmstrip::show(app, ui, ctx);
                }
            }
        }
    }
    // ⚠ After the filmstrip, so it sits above it: egui stacks bottom panels outward in call
    // order. It spans the canvas column, the rail and the panel having taken their sides.
    if app.session.is_some() && app.show_panels {
        crate::status_bar::develop(app, ui);
    }
    photo_edited |= canvas::show(app, ui, ctx);
    photo_edited |= delete_selected_retouch(app, ctx);

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
    // ⚠ It named the configured pan binding until 2026-09-29; a hover now says what the control
    // does in one sentence (`docs/DESIGN.md` §8), and panning is in the shortcuts dialog.
    let zoom_actual_hint = "Zooms to 100 %, one photo pixel per screen pixel (⌘1)";
    // Undo/Redo drive the unified `ActionLog`, so one control walks back through photo edits
    // and preset deletions in the order they actually happened. ⚠ The *tooltip* says which,
    // so an icon button never silently does the surprising one — that wording used to be the
    // button's label and must not be lost with it.
    let open = app.open_photo();
    let undo_hint = match app.actions.peek_undo(open.as_ref()) {
        Some(ActionKind::PresetDelete { name, .. }) => {
            format!("Brings back the deleted preset “{name}” (⌘Z)")
        }
        _ => "Undoes the last change (⌘Z)".to_string(),
    };
    let undo_click = icon(
        ui,
        icons::UNDO,
        &undo_hint,
        app.actions.can_undo(open.as_ref()),
    );
    let redo_click = icon(
        ui,
        icons::REDO,
        "Redoes the last undone change (⇧⌘Z)",
        app.actions.can_redo(open.as_ref()),
    );
    ui.separator();
    if undo_click {
        commands::execute(app, ctx, Command::Undo);
    }
    if redo_click {
        commands::execute(app, ctx, Command::Redo);
    }

    if app.session.is_some() {
        // ⚠ Real magnifying glasses, from the bundled font. These were `⊞`/`⊟` and a text
        // "1:1" until the icon font landed, because egui's own fonts contain no magnifying
        // glass at all. ⚠ `ZOOM_FIT` is a frame rather than a fourth glass — Phosphor has
        // exactly three glass variants, and a frame is the conventional fit symbol anyway.
        //
        // ⚠ The four abut in one dark well, in the order a zoom reads: out, fit, 1:1, in. They
        // are momentary actions, so none is ever lit — a `pill` would claim one of them is
        // the current state, and "fit" stops being true the moment you scroll.
        let (zoom_out, zoom_fit, zoom_actual, zoom_in) = well(ui, |ui| {
            (
                icon(ui, icons::ZOOM_OUT, "Zooms out (⌘−)", true),
                icon(
                    ui,
                    icons::ZOOM_FIT,
                    "Fits the whole photo in the view (⌘0)",
                    true,
                ),
                icon(ui, icons::ZOOM_ACTUAL, zoom_actual_hint, true),
                icon(ui, icons::ZOOM_IN, "Zooms in (⌘=)", true),
            )
        });
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
        //
        // ⚠ Labelled, where the zoom buttons are glyphs alone: a toggle whose state you have
        // to notice deserves the word, and it is the one control here that is a state.
        let before_after = app.session.as_ref().is_some_and(|s| s.show_before);
        let label = egui::RichText::new(format!("{} Before / After", icons::BEFORE_AFTER));
        if ui
            .add(
                egui::Button::selectable(before_after, label)
                    .min_size(egui::vec2(0.0, TOOL_HEIGHT)),
            )
            .hover("Shows the photo without your adjustments (\\)")
            .clicked()
        {
            commands::execute(app, ctx, Command::ToggleBeforeAfter);
        }
    }
    // ⚠ The filename is **not** drawn here. `shell` owns the row's right-aligned cluster, so
    // that the status line and the filename cannot fight over the same remaining space — which
    // is exactly what happened when this function right-aligned it itself.
}

/// How far [`toolbar`] advances the bar's cursor, measured rather than drawn — the seed the top bar
/// uses to centre the photo's name identically in both views before Develop has been drawn once
/// (`shell::shared_gap`).
///
/// ⚠ Mirrors `toolbar`'s sequence: undo, redo, a separator, the zoom well (spacing 2 inside), a
/// separator, Before / After. `shell`'s `the_centred_name_sits_at_the_same_x_in_both_views` fails
/// if this and the drawn toolbar drift apart.
pub fn toolbar_width(ui: &egui::Ui) -> f32 {
    let pad = ui.spacing().button_padding.x * 2.0;
    let gap = ui.spacing().item_spacing.x;
    let sep = 6.0;
    let glyph = |c: char| {
        let w = ui
            .painter()
            .layout_no_wrap(
                c.to_string(),
                egui::FontId::proportional(ICON_SIZE),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x;
        (w + pad).max(TOOL_WIDTH)
    };
    let body = egui::TextStyle::Button.resolve(ui.style());
    let before_after = ui
        .painter()
        .layout_no_wrap(
            format!("{} Before / After", icons::BEFORE_AFTER),
            body,
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x
        + pad;
    let well: f32 = [
        icons::ZOOM_OUT,
        icons::ZOOM_FIT,
        icons::ZOOM_ACTUAL,
        icons::ZOOM_IN,
    ]
    .iter()
    .map(|c| glyph(*c) + 2.0)
    .sum();
    glyph(icons::UNDO)
        + gap
        + glyph(icons::REDO)
        + gap
        + sep
        + gap
        + well
        + sep
        + gap
        + before_after
        + gap
}

/// A toolbar icon button with a tooltip. Returns whether it was clicked.
///
/// ⚠ The tooltip is not decoration: it is the only thing naming the action once the label is a
/// glyph, so every caller must supply a real one.
fn icon(ui: &mut egui::Ui, glyph: char, hint: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(icon_text(glyph)).min_size(egui::vec2(TOOL_WIDTH, TOOL_HEIGHT)),
    )
    .hover(hint)
    .clicked()
}

/// A toolbar control's size: wide enough for a glyph with air, as tall as the Library's
/// layout pill so the two bars share one rhythm.
const TOOL_WIDTH: f32 = 30.0;
const TOOL_HEIGHT: f32 = 28.0;

/// A dark well holding abutting buttons — the zoom group.
///
/// ⚠ The well is what turns four icon buttons into one control: `item_spacing` 2 between
/// them, the extreme background behind, one rounding around. The buttons keep their own
/// frames, so hover and press look as they do everywhere else.
///
/// ⚠⚠ **No child `Ui` of any kind, and painted behind.** The first version was an
/// `egui::Frame` with a 2 pt inner margin and came out 39 pt tall in a 36 pt row; a `scope`
/// around a `horizontal` still came out 37 — a nested ui is centred in the row and then
/// allocated back into it, and the rounding lands wherever it lands. So the bar grew in
/// Develop and shrank back in Library ("the height of the top bar changes when switching",
/// seen in use 2026-09-20). The buttons go straight into the row with a narrower spacing, the
/// well is a shape reserved first and set afterwards (Frame's own trick, without its
/// allocation), 2 pt beyond the buttons, `TOOL_HEIGHT` tall about the row's centre — it
/// allocates nothing, so the row stays the buttons' height.
/// `the_bar_is_the_same_height_in_both_views` measures it.
fn well<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // ⚠ The segmented well's colour and shape (`docs/DESIGN.md` §7.2): the canvas colour, a pill.
    let fill = crate::theme::tokens::of(ui).canvas;
    let bg = ui.painter().add(egui::Shape::Noop);
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 2.0;
    let left = ui.cursor().left();
    let inner = add(ui);
    let right = ui.cursor().left() - 2.0;
    ui.spacing_mut().item_spacing.x = spacing;
    let cy = ui.max_rect().center().y;
    let rect = egui::Rect::from_min_max(
        egui::pos2(left, cy - TOOL_HEIGHT / 2.0),
        egui::pos2(right, cy + TOOL_HEIGHT / 2.0),
    )
    .expand(2.0);
    ui.painter().set(
        bg,
        egui::epaint::RectShape::filled(rect, rect.height() / 2.0, fill),
    );
    inner
}

/// One toolbar icon at [`ICON_SIZE`].
///
/// ⚠ Sized explicitly at the toolbar icon size (`tokens::ICON`, 16) rather than inheriting the
/// 13-pt body text: an icon carries the *whole* meaning of its button, where a word has letters
/// to be recognised by, so the body size reads as too small. One helper, so the toolbar cannot
/// end up with two icon sizes.
fn icon_text(glyph: char) -> egui::RichText {
    egui::RichText::new(glyph).size(ICON_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the real `develop::body` and report what `suppressed_mask` came out as.
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

//! The Masks section: add, select, delete, shape properties, brush settings, adjustments.
//!
//! ⚠ Shown only while the **Masks** tool is selected on the rail, and it is then the *whole*
//! panel — the global adjustment sliders are not visible alongside it. A mask carries its own
//! Temp…Vibrance, so nothing here needs them.

use crate::develop::panel::Edited;
use crate::{theme, Session};
use eframe::egui;
use rustroom_app::mask_tool;

/// Brush tool settings, which live on the app rather than on the mask.
///
/// ⚠ They are the *tool's* state and apply to the next stroke, exactly as a brush size does
/// in any editor. Each stroke records its own copy at the moment it is drawn.
///
/// ⚠ Five `&mut` to *disjoint* fields of `RustroomApp`, which is what replaced the previous
/// copy-into-locals-and-write-back workaround: that existed only because a closure capturing
/// `app` while `s` also borrowed it is `E0499`. Explicit parameters have no such problem.
pub struct BrushTool<'a> {
    pub size: &'a mut f32,
    pub hardness: &'a mut f32,
    pub flow: &'a mut f32,
    pub erase: &'a mut bool,
    pub show_tint: &'a mut bool,
}

/// Draw the Masks tool's controls.
///
/// ⚠ No `CollapsingHeader` of its own — the panel supplies the heading. A header inside a
/// header is a nesting level this rework exists to remove.
pub fn show(ui: &mut egui::Ui, s: &mut Session, brush: BrushTool<'_>, ed: &mut Edited) {
    let mut added: Option<edit_model::Mask> = None;
    ui.horizontal(|ui| {
        if ui.button("Add graduated filter").clicked() {
            added = Some(edit_model::Mask::Linear(edit_model::LinearGradient {
                // A visible default: top to bottom across the middle
                // third, so the handles land on screen and the ramp is
                // immediately obvious.
                from: [0.5, 0.33],
                to: [0.5, 0.67],
                invert: false,
            }));
        }
        if ui.button("Add brush").clicked() {
            // ⚠ `Brush::default()` has no strokes, so the mask is inert until
            // painted — which is exactly why the tint defaults on.
            added = Some(edit_model::Mask::Brush(edit_model::Brush::default()));
        }
        if ui.button("Add radial filter").clicked() {
            // ⚠ The hand-written `Default`: centred, a quarter of each
            // axis, feather 50. Both radii being fractions of their own
            // axis makes the default ellipse *similar to the frame*, which
            // is usually the shape being reached for.
            added = Some(edit_model::Mask::Radial(
                edit_model::RadialGradient::default(),
            ));
        }
    });
    if let Some(mask) = added {
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask,
            adjustments: edit_model::LocalAdjustments::default(),
        });
        s.selected_mask = Some(s.edits.masks.len() - 1);
        ed.discrete();
    }

    let mut remove: Option<usize> = None;
    for i in 0..s.edits.masks.len() {
        ui.horizontal(|ui| {
            // ⚠ The same "× then name" row the Presets tool uses. Two delete idioms in one
            // app is how a design system loses the conventions it exists to hold.
            if ui.button("×").on_hover_text("Delete").clicked() {
                remove = Some(i);
            }
            let picked = s.selected_mask == Some(i);
            let name = format!("{} {}", s.edits.masks[i].mask.label(), i + 1);
            if ui.selectable_label(picked, name).clicked() {
                s.selected_mask = if picked { None } else { Some(i) };
            }
        });
    }
    if let Some(i) = remove {
        s.selected_mask =
            mask_tool::selection_after_delete(s.selected_mask, i, s.edits.masks.len());
        s.edits.masks.remove(i);
        // ⚠ Clear any in-flight drag handle too: the canvas overlay
        // returns early when the selection is gone, which would
        // otherwise leave a stale handle set and panning dead.
        s.active_mask_handle = None;
        ed.discrete();
    }

    if let Some(i) = s.selected_mask {
        if let Some(m) = s.edits.masks.get_mut(i) {
            ui.separator();
            // Shape properties, kept apart from the adjustments below:
            // these describe the mask, not what it carries.
            //
            // ⚠ `invert` had no UI at all until now. Survivable for a
            // linear gradient, which can be inverted by dragging its
            // endpoints past each other; not survivable for a radial,
            // where the subject vignette — darken everything except the
            // ellipse — depends on it and no drag substitutes.
            if ui.checkbox(m.mask.invert_mut(), "Invert").changed() {
                ed.discrete();
            }
            if let edit_model::Mask::Radial(r) = &mut m.mask {
                ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
            }
            if matches!(m.mask, edit_model::Mask::Brush(_)) {
                // ⚠ Brush settings live on the app, not on the mask: they are
                // the *tool's* state and apply to the next stroke, exactly as
                // a brush size does in any editor. Each stroke records its own
                // copy at the moment it is drawn.
                ed.slider(ui, &mut *brush.size, 4.0..=300.0, "Size");
                ed.slider(ui, &mut *brush.hardness, 0.0..=100.0, "Hardness");
                ed.slider(ui, &mut *brush.flow, 1.0..=100.0, "Flow");
                ui.checkbox(&mut *brush.erase, "Erase");
                ui.checkbox(&mut *brush.show_tint, "Show mask");
                theme::hint(
                    ui,
                    "Drag to paint. Hardness shapes the edge; Flow is \
                     strength, and repeated passes build up.",
                );
            }
            ui.separator();
            let a = &mut m.adjustments;
            ed.slider(ui, &mut a.white_balance.temp, -100.0..=100.0, "Temp");
            ed.slider(ui, &mut a.white_balance.tint, -100.0..=100.0, "Tint");
            ed.slider(ui, &mut a.exposure, -5.0..=5.0, "Exposure");
            ed.slider(ui, &mut a.contrast, -100.0..=100.0, "Contrast");
            ed.slider(ui, &mut a.highlights, -100.0..=100.0, "Highlights");
            ed.slider(ui, &mut a.shadows, -100.0..=100.0, "Shadows");
            ed.slider(ui, &mut a.whites, -100.0..=100.0, "Whites");
            ed.slider(ui, &mut a.blacks, -100.0..=100.0, "Blacks");
            // ⚠ The same order as the global panel: White Balance, then Tone, then
            // Color with Vibrance first. One layout learned once, and it keeps
            // holding as either list changes.
            ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
            ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");
        }
    }

    theme::hint(
        ui,
        "Select a mask to drag its handles on the image. A gradient's two \
         outer lines mark where the effect starts and reaches full \
         strength; a radial's ellipse marks where it falls to nothing.",
    );
}

//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;

use crate::{preview_edits, theme, RustroomApp};
use edit_model::Edits;
use eframe::egui;
use rustroom_app::{crop_tool, mask_tool};

/// Accumulates, over one frame of the panel, whether the photo changed and whether a drag
/// settled into a history entry.
///
/// ⚠ **A struct rather than a closure, and that is what makes the panel splittable.** The
/// previous shape was one `FnMut` capturing `changed` and `settled` for the whole function,
/// so any *second* closure touching either was `E0499` — two simultaneous mutable captures
/// of the same local. Two comments in this file used to describe working around that. A
/// `&mut Edited` passed to each section has neither problem, and it crosses a function
/// boundary, which a capture cannot.
#[derive(Default)]
pub struct Edited {
    /// Something changed this frame; the preview must be re-requested.
    pub changed: bool,
    /// A drag ended or a discrete control was used; push one history entry.
    pub settled: bool,
}

impl Edited {
    /// A slider bound to `value`, recording change and settle separately.
    ///
    /// ⚠ One history entry per *settled* drag, not per changed frame: `r.changed()` and
    /// `r.drag_stopped()` are recorded apart, so dragging Exposure across its range is one
    /// Ctrl+Z rather than two hundred.
    pub fn slider(
        &mut self,
        ui: &mut egui::Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        label: &str,
    ) {
        let r = ui.add(egui::Slider::new(value, range).text(label));
        self.changed |= r.changed();
        self.settled |= r.drag_stopped();
    }

    /// A discrete act — a button, a checkbox, a preset click.
    ///
    /// ⚠ Both flags, unlike a slider: a press is a complete act, so it is one history entry
    /// immediately rather than waiting for a `drag_stopped` that will never come.
    pub fn discrete(&mut self) {
        self.changed = true;
        self.settled = true;
    }
}

/// Draw the adjustments panel.
///
/// Returns whether a photo edit settled into a new history entry.
///
/// ⚠ One history entry per *settled* drag, not per changed frame: the `slider` closure records
/// `r.drag_stopped()` separately from `r.changed()`, so dragging Exposure across its range is
/// one Ctrl+Z rather than two hundred.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) -> bool {
    let mut photo_edited = false;
    egui::Panel::right("panel")
        .default_size(240.0)
        .show(ui, |ui| {
            if app.pending.is_some() {
                ui.disable();
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(s) = &mut app.session {
                    draw_histogram(ui, &s.hist);
                    ui.separator();

                    let mut ed = Edited::default();
                    adjustments::show(ui, &mut s.edits, &mut app.hsl_channel, &mut ed);
                    // ⚠ Copied into locals for the same reason `modals/settings.rs` does: the
                    // closure below already borrows `app` through `s`, and a second capture is
                    // E0499. Written back after it closes.
                    let mut brush_size = app.brush_size;
                    let mut brush_hardness = app.brush_hardness;
                    let mut brush_flow = app.brush_flow;
                    let mut brush_erase = app.brush_erase;
                    let mut show_mask_tint = app.show_mask_tint;
                    egui::CollapsingHeader::new("Masks").show(ui, |ui| {
                        let mut added: Option<edit_model::Mask> = None;
                        ui.horizontal(|ui| {
                            if ui.button("Add graduated filter").clicked() {
                                added =
                                    Some(edit_model::Mask::Linear(edit_model::LinearGradient {
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
                                let picked = s.selected_mask == Some(i);
                                let name = format!("{} {}", s.edits.masks[i].mask.label(), i + 1);
                                if ui.selectable_label(picked, name).clicked() {
                                    s.selected_mask = if picked { None } else { Some(i) };
                                }
                                if ui.button("Delete").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            s.selected_mask = mask_tool::selection_after_delete(
                                s.selected_mask,
                                i,
                                s.edits.masks.len(),
                            );
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
                                    ed.slider(ui, &mut brush_size, 4.0..=300.0, "Size");
                                    ed.slider(ui, &mut brush_hardness, 0.0..=100.0, "Hardness");
                                    ed.slider(ui, &mut brush_flow, 1.0..=100.0, "Flow");
                                    ui.checkbox(&mut brush_erase, "Erase");
                                    ui.checkbox(&mut show_mask_tint, "Show mask");
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
                                ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");
                                ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
                            }
                        }

                        theme::hint(
                            ui,
                            "Select a mask to drag its handles on the image. A gradient's two \
                             outer lines mark where the effect starts and reaches full \
                             strength; a radial's ellipse marks where it falls to nothing.",
                        );
                    });
                    // ⚠ Deliberately NOT folded into `changed`/`settled`: a tool setting does not
                    // alter the photograph, so it must not push a history entry or request a
                    // render.
                    app.brush_size = brush_size;
                    app.brush_hardness = brush_hardness;
                    app.brush_flow = brush_flow;
                    app.brush_erase = brush_erase;
                    app.show_mask_tint = show_mask_tint;

                    ui.separator();
                    if ui.button("Reset all").clicked() {
                        s.edits = Edits::default();
                        ed.discrete();
                    }

                    if s.geometry_mode {
                        ui.separator();
                        ui.label("Geometry");
                        // Quarter turns and mirroring. ⚠ Each button moves the crop by the
                        // *same* transform, or the crop lands on different content — see
                        // `CropRect::rotated_cw`. Keep the two in step.
                        ui.horizontal(|ui| {
                            let g = &mut s.edits.geometry;
                            let mut turned = false;
                            if ui.button("⟲").on_hover_text("Rotate left").clicked() {
                                g.orientation.rotate_ccw();
                                for _ in 0..3 {
                                    g.crop = g.crop.rotated_cw();
                                }
                                turned = true;
                            }
                            if ui.button("⟳").on_hover_text("Rotate right").clicked() {
                                g.orientation.rotate_cw();
                                g.crop = g.crop.rotated_cw();
                                turned = true;
                            }
                            if ui.button("⬌").on_hover_text("Flip horizontal").clicked() {
                                g.orientation.flip_h();
                                g.crop = g.crop.flipped_h();
                                turned = true;
                            }
                            if ui.button("⬍").on_hover_text("Flip vertical").clicked() {
                                g.orientation.flip_v();
                                g.crop = g.crop.flipped_h();
                                g.crop = g.crop.rotated_cw();
                                g.crop = g.crop.rotated_cw();
                                turned = true;
                            }
                            if turned {
                                // ⚠ `settled` too: a button press is a discrete act, so it
                                // is one history entry and one Ctrl+Z — unlike a slider,
                                // which pushes only on `drag_stopped`.
                                ed.discrete();
                            }
                        });
                        let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
                        let presets: [(&str, Option<f32>); 5] = [
                            ("Free", None),
                            ("1:1", Some(1.0)),
                            ("3:2", Some(1.5)),
                            ("4:3", Some(4.0 / 3.0)),
                            ("16:9", Some(16.0 / 9.0)),
                        ];
                        ui.horizontal(|ui| {
                            for (label, pa) in presets {
                                if ui.button(label).clicked() {
                                    s.edits.geometry.crop = match pa {
                                        Some(a) => crop_tool::snap_to_aspect(a / img_aspect),
                                        None => edit_model::CropRect::default(),
                                    };
                                    ed.discrete();
                                }
                            }
                        });
                        ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");
                    }

                    if ed.changed {
                        s.request(
                            preview_edits(&s.edits, s.geometry_mode),
                            s.render_max_dim(),
                            s.requested_region,
                        );
                    }
                    if ed.settled {
                        photo_edited |= s.history.push(s.edits.clone());
                    }

                    ui.separator();
                    // The filename first: which photo this is matters more than what
                    // shot it, and the panel scrolls, so it belongs above the metadata.
                    if let Some(name) = s.path.file_name() {
                        ui.label(egui::RichText::new(name.to_string_lossy()).strong());
                    }
                    theme::caption(ui, &s.make_model);
                } else {
                    ui.label("No image open.");
                }
            });
        });
    photo_edited
}

fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    let max = hist
        .iter()
        .flat_map(|c| c.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let colors = [
        egui::Color32::from_rgb(220, 80, 80),
        egui::Color32::from_rgb(80, 200, 80),
        egui::Color32::from_rgb(90, 120, 230),
    ];
    for (ci, chan) in hist.iter().enumerate() {
        for (i, &count) in chan.iter().enumerate() {
            let x = rect.left() + rect.width() * (i as f32 / 255.0);
            let h = rect.height() * (count as f32 / max);
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, colors[ci].linear_multiply(0.5)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discrete_act_is_both_changed_and_settled() {
        // ⚠ A button press is one history entry and one Ctrl+Z, unlike a slider, which
        // pushes only on `drag_stopped`. Conflating the two is how a single click became
        // either zero undo steps or two.
        let mut e = Edited::default();
        assert!(!e.changed && !e.settled);
        e.discrete();
        assert!(e.changed, "a discrete act must request a render");
        assert!(e.settled, "a discrete act must push one history entry");
    }

    #[test]
    fn an_untouched_frame_records_nothing() {
        // The overwhelmingly common case: the panel runs every frame and must not push a
        // history entry for merely existing.
        let e = Edited::default();
        assert!(!e.changed);
        assert!(!e.settled);
    }
}

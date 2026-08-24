//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;
pub mod masks;

use crate::{preview_edits, theme, RustroomApp};
use edit_model::Edits;
use eframe::egui;
use rustroom_app::crop_tool;

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
                    masks::show(
                        ui,
                        s,
                        masks::BrushTool {
                            size: &mut app.brush_size,
                            hardness: &mut app.brush_hardness,
                            flow: &mut app.brush_flow,
                            erase: &mut app.brush_erase,
                            show_tint: &mut app.show_mask_tint,
                        },
                        &mut ed,
                    );
                    adjustments::show(ui, &mut s.edits, &mut app.hsl_channel, &mut ed);

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

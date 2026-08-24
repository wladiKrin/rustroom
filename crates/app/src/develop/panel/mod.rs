//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;
pub mod geometry;
pub mod masks;
pub mod presets;

use crate::{preview_edits, theme, RustroomApp};
use edit_model::Edits;
use eframe::egui;
use rustroom_app::tool::Tool;

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
                if app.session.is_none() {
                    ui.label("No image open.");
                    return;
                }
                let tool = app.tool;
                // ⚠ A *shared* borrow that ends on this line, so `presets::show` below can take
                // `&mut app`. The histogram only reads.
                if let Some(s) = &app.session {
                    draw_histogram(ui, &s.hist);
                }
                ui.separator();

                // ⚠ Presets is drawn here, outside the mutable session borrow, because it is
                // the one drawer that edits neither `Session` nor `Edits` — it applies to the
                // whole selection. Inside the drawer `match` below it would need `&mut app`
                // while `s` already holds `&mut app.session`, which are not disjoint fields.
                if tool == Tool::Presets {
                    egui::CollapsingHeader::new("Presets")
                        .default_open(true)
                        .show(ui, |ui| presets::show(app, ui));
                    ui.separator();
                }

                if let Some(s) = &mut app.session {
                    let mut ed = Edited::default();
                    // The tool's drawer, above the permanent adjustment stack — the global
                    // sliders stay reachable while a mask is being painted. ⚠ `Tool::Edit`
                    // opens nothing, which is what makes the panel at rest exactly the five
                    // adjustment sections.
                    if let Some(title) = tool.drawer_label() {
                        egui::CollapsingHeader::new(title)
                            .default_open(true)
                            .show(ui, |ui| match tool {
                                Tool::Crop => geometry::show(ui, s, &mut ed),
                                Tool::Masks => masks::show(
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
                                ),
                                // Drawn above, outside the session borrow — see the comment
                                // there. ⚠ A match arm, not a wildcard: a fifth tool must
                                // force a decision here.
                                Tool::Presets => {}
                                Tool::Edit => {}
                            });
                        ui.separator();
                    }
                    adjustments::show(ui, &mut s.edits, &mut app.hsl_channel, &mut ed);

                    ui.separator();
                    if ui.button("Reset all").clicked() {
                        s.edits = Edits::default();
                        ed.discrete();
                    }

                    if ed.changed {
                        s.request(
                            preview_edits(&s.edits, tool),
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

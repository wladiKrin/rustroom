//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;
pub mod geometry;
pub mod masks;
pub mod presets;

use crate::{theme, RustroomApp};
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

                // ⚠ **The panel is exclusive: exactly one tool's controls at a time.** The
                // global sliders are reachable only under `Edit`. An earlier design kept them
                // permanently below a drawer so they stayed available while masking; that was
                // reversed in use — mixing a tool's controls with the global stack made it
                // ambiguous which of the two a slider belonged to.
                //
                // ⚠ A plain heading, not a `CollapsingHeader`. Now that the tool's controls
                // are the *only* thing in the panel, collapsing them would leave it empty.
                if let Some(title) = tool.panel_heading() {
                    ui.heading(title);
                }
                // ⚠ Presets is drawn outside the mutable session borrow, because it is the one
                // tool that edits neither `Session` nor `Edits` — it applies to the whole
                // selection. Inside the `match` below it would need `&mut app` while `s`
                // already holds `&mut app.session`, which are not disjoint fields.
                if tool == Tool::Presets {
                    presets::show(app, ui);
                }

                // ⚠ Drawn here, outside the session borrow below, for the same reason
                // `presets::show` is: it needs `&mut app` for the settings and the re-decode,
                // which is not disjoint from `&mut app.session`.
                if tool == Tool::Edit {
                    camera_profile_row(app, ui);
                }

                if let Some(s) = &mut app.session {
                    let mut ed = Edited::default();
                    // ⚠ An exhaustive match, not a wildcard: a fifth tool must force a
                    // decision about what the panel shows under it.
                    match tool {
                        Tool::Edit => {
                            adjustments::show(ui, &mut s.edits, &mut app.hsl_channel, &mut ed);
                            ui.separator();
                            // ⚠ **Scoped to the five sections above, not to everything.**
                            // Under an exclusive panel each tool owns its own domain, so this
                            // must not throw away a crop or a painted mask the user cannot
                            // even see from here. The group set is *derived* from `SECTIONS`,
                            // so it resets exactly what is on screen.
                            //
                            // ⚠ Consequence, accepted: geometry and masks have no reset of
                            // their own yet. Crop is undone by the "Free" aspect preset and a
                            // zero angle; a mask by deleting it.
                            if ui
                                .button("Reset adjustments")
                                .on_hover_text("Resets White Balance, Tone, Color, Detail and Effects. Leaves the crop and any masks alone.")
                                .clicked()
                            {
                                s.edits
                                    .merge_from(&Edits::default(), adjustments::edit_groups());
                                ed.discrete();
                            }
                        }
                        Tool::Crop => geometry::show(ui, s, &mut app.crop_aspect, &mut ed),
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
                        // Drawn above, outside the session borrow — see the comment there.
                        Tool::Presets => {}
                    }

                    if ed.changed {
                        s.request(
                            s.view_edits(tool),
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

/// The camera-profile dropdown.
///
/// ⚠ **RAW only.** A JPEG carries no camera profile, and offering one would imply it did.
///
/// ⚠ The profile list is read from `app.camera_profiles_cache`, populated when a photo opens —
/// **not** rescanned here. This function runs every frame, and scanning a directory sixty times a
/// second is the `apply_view_demosaic` mistake in a different costume.
fn camera_profile_row(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some((make_model, is_raw)) = app
        .session
        .as_ref()
        .map(|s| (s.make_model.clone(), catalog::is_raw(&s.path)))
    else {
        return;
    };
    if !is_raw {
        return;
    }
    let current = app.settings.camera_profiles.get(&make_model).cloned();
    let mut chosen = current.clone();
    let label = current
        .as_ref()
        .and_then(|p| {
            app.camera_profiles_cache
                .iter()
                .find(|e| &e.path == p)
                .map(|e| e.name.clone())
        })
        .unwrap_or_else(|| "Built-in (libraw)".to_string());

    ui.horizontal(|ui| {
        ui.label("Camera profile");
        egui::ComboBox::from_id_salt("camera_profile_pick")
            .selected_text(label)
            .show_ui(ui, |ui| {
                // ⚠ "Built-in" is a real choice, not a fallback label: it selects C1's libraw
                // matrix, the well-tested path and the oracle C2 and C3 are measured against.
                // Someone who dislikes a profile must be able to get back to it.
                ui.selectable_value(&mut chosen, None, "Built-in (libraw)");
                for e in &app.camera_profiles_cache {
                    ui.selectable_value(&mut chosen, Some(e.path.clone()), &e.name);
                }
            });
    });
    if app.camera_profiles_cache.is_empty() {
        theme::hint(
            ui,
            "No profiles installed. Rustroom ships none — drop .dcp files into the profiles \
             folder beside your settings to use them.",
        );
    }
    // ⚠ A change guard like its neighbours: this body runs every frame, and `apply_camera_profile`
    // rebuilds two decode workers and re-decodes the open photo.
    if chosen != current {
        app.apply_camera_profile(make_model, chosen, &ctx);
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

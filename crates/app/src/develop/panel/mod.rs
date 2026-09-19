//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;
pub mod color_wheel;
pub mod geometry;
pub mod hsl_bands;
pub mod look;
pub mod mask_adjust;
pub mod mask_components;
pub mod masks;
pub mod presets;
pub mod retouch;
pub mod snapshots;
pub mod tone_curve;

use crate::name_edit;
use crate::{theme, RustroomApp};
use edit_model::Edits;
use eframe::egui;
use rustroom_app::file_rename;
use rustroom_app::photo_id::PhotoId;
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
    ) -> egui::Response {
        let r = ui.add(egui::Slider::new(value, range).text(label));
        self.changed |= r.changed();
        self.settled |= r.drag_stopped();
        // ⚠ Returned so a caller can attach hover text. Every existing call site ignores it,
        // which is why this was additive rather than a new `slider_hint` beside it.
        r
    }

    /// Record a discrete control's use — a button, a preset — as one settled edit.
    ///
    /// ⚠ Both flags, not just `changed`: a click has no drag to stop, so without `settled` the
    /// edit would reach the pixels and never reach the undo stack.
    pub fn mark_changed(&mut self) {
        self.changed = true;
        self.settled = true;
    }

    /// Fold a `Response` from a widget this helper did not build, e.g. one with a custom
    /// formatter. Same accounting as [`Self::slider`].
    pub fn absorb(&mut self, r: &egui::Response) {
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
/// The photo's filename, editable in place.
///
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) -> bool {
    let mut photo_edited = false;
    let mut rename: Option<name_edit::Rename> = None;
    // ⚠ The Geometry panel's Auto button, applied after the panel closes: `auto_guides` needs
    // `&mut RustroomApp` while the closure below still holds `&mut app.session`, which is the
    // same reason `rename` is applied out here rather than at its call site.
    let mut auto = false;
    // ⚠ Captured so the settings file is written **once, on an actual change**, after the panel
    // closes. The Crop tool's dropdown edits `app.settings.composition_guide` in place — a
    // disjoint field, so the borrow is legal — but `save_settings` takes `&mut app` whole, and
    // this body runs every frame: writing unconditionally would rewrite the file sixty times a
    // second, the `apply_view_demosaic` rule.
    let guide_before = app.settings.composition_guide;
    let custom_before = app.settings.custom_aspect;
    egui::Panel::right("panel")
        .default_size(240.0)
        .frame(theme::panel_frame(ui.style()))
        .show(ui, |ui| {
            if app.pending.is_some() {
                ui.disable();
            }
            // ⚠⚠ **Before the scroll area, and that ordering is the whole feature.** egui
            // allocates in call order, so the strip takes its height from the panel's bottom
            // edge and the scroll area gets what is left — which is what pins the strip while
            // forty sliders scroll behind it. Moved after, it would scroll away with them.
            if app.pending.is_none() {
                let ctx = ui.ctx().clone();
                photo_edited |= snapshots::show(app, ui, &ctx);
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
                    presets::show(app, ui, &ui.ctx().clone());
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
                            adjustments::show(
                                ui,
                                &mut s.edits,
                                &mut app.hsl_channel,
                                adjustments::GradeUi {
                                    range: &mut app.grade_range,
                                    show_range: &mut s.show_grade_range,
                                    view: &mut s.grade_view,
                                },
                                adjustments::SourceInfo {
                                    lens: s.lens.name.as_deref(),
                                    lens_profile_found: s.lens.shot.is_some(),
                                    wb_baseline: s.wb_baseline,
                                    baseline_ev: s.baseline_ev,
                                    // ⚠ A shared borrow of a *different* field from the `&mut
                                    // s.edits` beside it — disjoint fields, so this is legal
                                    // where a closure capturing `s` twice would be `E0499`.
                                    buffer: &s.buffer,
                                    looks: &app.looks,
                                },
                                Some(&mut s.picking_wb),
                                &mut ed,
                            );
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
                                // ⚠ Derived from `SECTIONS`, not written out: this string said
                                // "White Balance, Tone, Color, Detail and Effects" and went stale
                                // the moment a section was added. The button's group set is
                                // already derived; its description now is too.
                                .on_hover_text(format!(
                                    "Resets {}. Leaves the crop and any masks alone.",
                                    adjustments::SECTIONS
                                        .iter()
                                        .map(|s| s.name())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ))
                                .clicked()
                            {
                                s.edits
                                    .merge_from(&Edits::default(), adjustments::edit_groups());
                                ed.discrete();
                            }
                        }
                        Tool::Crop => {
                            let found = s.lens.shot.is_some();
                            geometry::show(
                                ui,
                                s,
                                geometry::AspectLock {
                                    lock: &mut app.crop_aspect,
                                    custom: &mut app.settings.custom_aspect,
                                },
                                &mut app.settings.composition_guide,
                                found,
                                &mut ed,
                                &mut auto,
                            )
                        }
                        Tool::Masks => masks::show(
                            ui,
                            s,
                            masks::MaskUi {
                                hsl: &mut app.mask_hsl_channel,
                                grade: &mut app.mask_grade_range,
                                add_mode: &mut app.mask_add_mode,
                            },
                            masks::BrushTool {
                                size: &mut app.brush_size,
                                hardness: &mut app.brush_hardness,
                                flow: &mut app.brush_flow,
                                erase: &mut app.brush_erase,
                                show_tint: &mut app.show_mask_tint,
                                auto_mask: &mut app.brush_auto_mask,
                                tolerance: &mut app.brush_tolerance,
                            },
                            &mut ed,
                        ),
                        Tool::Retouch => {
                            retouch::show(ui, s, &mut app.spot_tool, &mut app.show_spots, &mut ed)
                        }
                        // Drawn above, outside the session borrow — see the comment there.
                        Tool::Presets => {}
                    }

                    if ed.changed {
                        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                    }
                    if ed.settled {
                        photo_edited |= s.history.push(s.edits.clone());
                    }

                    ui.separator();
                    // ⚠⚠ The **filename**, as a caption — not a field. Renaming the file is the
                    // Library caption's job; this panel edits the *label*, below. One control per
                    // string, which is the whole of M6.0.
                    theme::caption(ui, &file_rename::field_text(&s.path));
                    // ⚠ The label field is the discoverable home for a label. The badge in the
                    // grid is the other, and a badge is small — this is why there are two entry
                    // points for one string, which is acceptable precisely *because* it is one
                    // string. Contrast the Presets panel, removed for being two homes for one
                    // feature.
                    rename = name_edit::field(
                        ui,
                        name_edit::NameField {
                            id: &PhotoId::new(s.path.clone(), s.copy),
                            file_name: &file_rename::field_text(&s.path),
                            label: s.name.as_deref(),
                            mode: name_edit::Editing::Label,
                        },
                        &mut app.label_draft,
                        egui::TextStyle::Body.resolve(ui.style()),
                        // ⚠ Never autofocus: this field is on screen the whole time, so grabbing
                        // focus would hand it every keystroke the moment a photo opened.
                        false,
                    );
                    if s.copy != 0 {
                        theme::caption(ui, &format!("Copy {}", s.copy));
                    }
                    theme::caption(ui, &s.make_model);
                }
            });
        });

    // ⚠ Guarded on an actual change, and outside the closure — see `guide_before`. The custom
    // crop ratio is the second setting this panel edits in place.
    if app.settings.composition_guide != guide_before || app.settings.custom_aspect != custom_before
    {
        app.save_settings();
    }

    // ⚠ Run OUTSIDE the session borrow: `auto_guides` needs `&mut RustroomApp`, which
    // `&mut app.session` rules out — the same reason `presets::show` is drawn outside it.
    if auto {
        photo_edited |= app.auto_guides();
    }

    // ⚠ Applied *after* the panel closes, not inside it: the commit writes the sidecar and
    // rebuilds the Library, both of which need `&mut app` while the closure still holds
    // `&mut app.session`. The same shape `photo_edited` uses, and for the same reason.
    if let Some(r) = rename {
        let id = app
            .session
            .as_ref()
            .map(|s| PhotoId::new(s.path.clone(), s.copy));
        if let Some(id) = id {
            let ctx = ui.ctx().clone();
            name_edit::commit(app, &ctx, &id, r);
        }
    }
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

    /// ⚠⚠ **The session's baseline must reach `SourceInfo`, and only a whole-panel draw can see
    /// it.** The correction lives in `pipeline`, the forwarding in `adjustments::tone` — both
    /// tested — and a `baseline_ev: None` here would still leave the fix present and
    /// **unreachable from the running app**, with every other test green. Found by sabotage,
    /// which is the only reason this test exists.
    #[test]
    fn the_sessions_baseline_reaches_the_auto_solve() {
        let ctx = crate::test_ui::ctx();
        let mut exposures = Vec::new();
        // ⚠ `None` against a **negative** baseline. The pair was `None`/`Some(2.0)` until the
        // 2026-09-08 retune of `MEDIAN_TARGET` to 0.20: the fixture's median then solves to
        // −2.45 EV under the fallback baseline, and a +2 EV baseline aims below the −2.5 EV clamp,
        // so both values sat on the clamp 0.05 apart and the test failed for the wrong reason.
        // A −1 EV baseline raises the target instead, well inside the range.
        for baseline in [None, Some(-1.0)] {
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
            {
                let s = app.session.as_mut().expect("session");
                s.baseline_ev = baseline;
                // ⚠ The correction applies only under the camera rendering; with `None` the two
                // do not stack, the target is unchanged, and this test could see nothing.
                s.edits.rendering = Some(edit_model::Rendering::Camera);
            }
            crate::test_ui::click(&ctx, "Auto", |ui| {
                show(&mut app, ui);
            });
            exposures.push(app.session.as_ref().unwrap().edits.exposure);
        }
        assert!(
            (exposures[0] - exposures[1]).abs() > 0.1,
            "the session's baseline never reached the solve: {exposures:?}"
        );
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

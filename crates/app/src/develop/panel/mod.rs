//! The right-hand adjustments panel: histogram, then every Develop control.

pub mod adjustments;
pub mod color_wheel;
pub mod defringe;
pub mod geometry;
pub mod hsl_bands;
pub mod look;
pub mod mask_adjust;
pub mod mask_components;
pub mod mask_glyph;
pub mod masks;
pub mod parametric_curve;
pub mod presets;
pub mod profile;
pub mod retouch;
mod retouch_eyes;
pub mod section;
pub mod slider;
pub mod snapshots;
pub mod tone_curve;

use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::tool::Tool;
use crate::theme::Hover as _;

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
    /// One line for the status bar about what a button just did — Auto says which sliders it
    /// set and by how much, so two sliders pinned at +100 read as a result, not a fault (A-17).
    pub note: Option<String>,
    /// Which block the next sliders belong to, for their hover sentence — set before a block,
    /// reset after (`slider_help::Scope`).
    pub help_scope: rustroom_app::slider_help::Scope,
    /// Every slider drawn this frame whose scope and label found no sentence, as `Scope/label`.
    /// ⚠ The panel tests assert it stays empty — the pairs actually drawn, not a list of labels.
    pub missing_help: Vec<String>,
}

/// Whether `r`'s gesture is over, so its change is one undo step.
///
/// ⚠⚠ **`drag_stopped` alone missed a CLICK (reported 2026-09-27).** The rail sets the value on the
/// press, before any drag exists, so a click on the track reached the picture and never the undo
/// stack. Now also: a `clicked()` release, and a change made with the pointer NOT held on the
/// widget — the arrow keys. A press that has not been released yet settles nothing, so a real drag
/// is still ONE step, and a settle with no net change records nothing (`History::push` ignores an
/// equal value).
pub(crate) fn settles(r: &egui::Response) -> bool {
    r.drag_stopped() || r.clicked() || (r.changed() && !r.is_pointer_button_down_on())
}

impl Edited {
    /// A slider bound to `value`, recording change and settle separately; a double-click restores
    /// `default`.
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
        // ⚠ Required, never defaulted to 0: a double-click restores it, and a tool slider such
        // as Hardness has a non-zero default a silent 0 would get wrong. Forgetting it is a
        // compile error; getting it wrong fails `every_slider_resets_to_its_fields_default`.
        default: f32,
    ) -> egui::Response {
        // ⚠ Every slider explains itself on hover, from one table, in its block's scope. A pair the
        // table does not know draws no help and is recorded; the panel tests fail on it.
        let help = rustroom_app::slider_help::text(self.help_scope, label);
        if help.is_none() {
            self.missing_help
                .push(format!("{:?}/{label}", self.help_scope));
        }
        let r = ui.add(
            slider::Slider::new(value, range)
                .text(label)
                .reset_to(default)
                .help(help),
        );
        self.absorb(&r);
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
        self.settled |= settles(r);
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
    // The tone curve's open state is a setting, toggled deep inside the session borrow below;
    // saved once out here when it changed — `collections_panel`'s `panel_sections` shape.
    let curve_was_open = app.settings.tone_curve_open;
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
        // ⚠ 320, up from 270 (2026-09-23, user's call); 270 was up from 240 (2026-09-21), when the
        // snapshot strip's Now row and the longer slider names were cut off.
        //
        // ⚠⚠ egui's panel state is **not persisted** here, so this default is the width every launch
        // actually gets — not merely a first-run hint.
        .default_size(316.0)
        // ⚠ `default_size` is a *wrapping* width under egui 0.35: on a fresh profile the panel came
        // up at ~214 pt, which left every slider rail 60 pt long — a 40-pt drag took Exposure
        // from 0 to +5 (both first-use reviews). The floor is what keeps the rails usable.
        .min_size(300.0)
        .frame(theme::panel_frame(ui.style()))
        .show(ui, |ui| {
            // ⚠ A ghost session (`ghost.rs`) is the pending photo's own, and its sliders work
            // on the 512-px buffer until the decode lands. Only a pending photo with **no**
            // session yet leaves the previous session behind it, and then the panel is inert —
            // but **not faded** (the author's call, 2026-09-29): a greyed panel reads as
            // "nothing loaded yet".
            let ghost_covers = app
                .session
                .as_ref()
                .zip(app.pending.as_ref())
                .is_some_and(|(s, p)| s.path == p.id.path && s.copy == p.id.copy);
            if app.pending.is_some() && !ghost_covers {
                ui.disable();
                ui.set_opacity(1.0);
            }
            // ⚠⚠ **Before the scroll area, and that ordering is the whole feature.** egui
            // allocates in call order, so the strip takes its height from the panel's bottom
            // edge and the scroll area gets what is left — which is what pins the strip while
            // forty sliders scroll behind it. Moved after, it would scroll away with them.
            {
                // ⚠ Drawn while pending too, inert like the sliders — the strip vanishing and
                // returning was half of what made an open look slow.
                let ctx = ui.ctx().clone();
                photo_edited |= snapshots::show(app, ui, &ctx);
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                if app.session.is_none() {
                    ui.label("No image open.");
                    return;
                }
                let tool = app.tool;
                // What this photo would open as untouched — the adjustments' neutral and the
                // target of both resets (`adjustments::SourceInfo::fresh`).
                let fresh = app.settings.initial_edits(None);
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
                // ⚠ Hoisted above the reset row and the Profile block, which record clicks
                // into it before the session borrow below reads it.
                let mut ed = Edited::default();
                // ⚠ At the top of the stack, not under forty sliders (2026-09-19): every section
                // carries its own reset now, and the one for all of them must be reachable
                // without scrolling. Its own right-aligned row, because `Edit` has no heading
                // to share — `panel_heading` says why.
                //
                // ⚠ **Scoped to the sections, not to everything.** Under an exclusive panel each
                // tool owns its own domain, so this must not throw away a crop or a painted mask
                // the user cannot even see from here. The group set is *derived* from `SECTIONS`,
                // so it resets exactly what is on screen; the tooltip is derived the same way,
                // because a written-out list went stale the moment a section was added.
                //
                // ⚠ Consequence, accepted: geometry and masks have no reset of their own yet.
                // Crop is undone by the "Free" aspect preset and a zero angle; a mask by deleting
                // it.
                //
                // ⚠ Inside a `horizontal`, which bounds the row's height. A bare
                // `with_layout(right_to_left(Center))` in a scroll area centres over an unbounded
                // height and pushed the whole stack below it out of reach (caught by
                // `the_sessions_baseline_reaches_the_auto_solve`).
                if tool == Tool::Edit {
                    if let Some(s) = &mut app.session {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button("Reset adjustments")
                                    .hover(format!(
                                        "Resets {} to how a new photo opens, keeping the crop and masks",
                                        adjustments::SECTIONS
                                            .iter()
                                            .map(|s| s.name())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    ))
                                    .clicked()
                                {
                                    // ⚠ To how a new photo opens, never all-off: a ticked "Apply
                                    // lens correction to new photos" is the default, not an edit.
                                    s.edits.merge_from(&fresh, adjustments::edit_groups());
                                    ed.discrete();
                                }
                            },
                        );
                    }
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
                    // ⚠⚠ **The Profile block: the three controls that decide how the raw data
                    // becomes a picture**, gathered above every adjustment because they establish
                    // the starting picture and everything below adjusts it. Not a `Section`, and
                    // deliberately `resettable: false` — "Reset adjustments" derives its group set
                    // from `SECTIONS`, and a reset here would flatten the photo to a colorimetric
                    // render (`edit_model::RENDERING_GROUP`).
                    //
                    // ⚠ Drawn here rather than in `adjustments` because the camera profile row
                    // needs `&mut app` (settings, a re-decode) where the rest needs `&mut
                    // app.session`; the body borrows `app` first, then the session.
                    let mut out = section::header(
                        ui,
                        ui.make_persistent_id("profile-block"),
                        true,
                        section::Header::new("Profile"),
                    );
                    out.show_body(ui, |ui| {
                        camera_profile_row(app, ui);
                        if let Some(s) = &mut app.session {
                            profile::rendering(ui, &mut s.edits, &app.looks, &mut ed);
                        }
                    });
                }

                if let Some(s) = &mut app.session {
                    // ⚠ An exhaustive match, not a wildcard: a fifth tool must force a
                    // decision about what the panel shows under it.
                    match tool {
                        Tool::Edit => {
                            adjustments::show(
                                ui,
                                &mut s.edits,
                                adjustments::Channels {
                                    hsl: &mut app.hsl_channel,
                                    curve_mode: &mut app.curve_mode,
                                    curve: &mut app.curve_channel,
                                    curve_open: &mut app.settings.tone_curve_open,
                                    parametric_drag: app.settings.parametric_drag,
                                },
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
                                    hist: Some(&s.hist),
                                    fresh: &fresh,
                                },
                                adjustments::Pickers {
                                    wb: Some(&mut s.picking_wb),
                                    fringe: Some(&mut s.picking_fringe),
                                },
                                &mut ed,
                            );
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
                                curve_open: &mut app.settings.tone_curve_open,
                            },
                            masks::BrushTool {
                                size: &mut app.brush_size,
                                hardness: &mut app.brush_hardness,
                                flow: &mut app.brush_flow,
                                erase: &mut app.brush_erase,
                                auto_mask: &mut app.brush_auto_mask,
                                tolerance: &mut app.brush_tolerance,
                            },
                            &mut ed,
                        ),
                        Tool::Retouch => retouch::show_with(
                            ui,
                            s,
                            &mut app.spot_tool,
                            &mut app.eye_tool,
                            &mut ed,
                        ),
                        // Drawn above, outside the session borrow — see the comment there.
                        Tool::Presets => {}
                    }

                    if ed.changed {
                        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                    }
                    if ed.settled {
                        photo_edited |= s.history.push(s.edits.clone());
                    }
                    if let Some(note) = ed.note.take() {
                        app.status = note;
                    }

                    // ⚠ No footer (2026-09-20, user's call): the filename, the copy-label field
                    // and the camera used to sit here. The top bar names the photo now, and
                    // renames it; the copy label keeps its home on the grid's badge.
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

    if app.settings.tone_curve_open != curve_was_open {
        app.save_settings();
    }

    // ⚠ Applied *after* the panel closes, not inside it: the commit writes the sidecar and
    // rebuilds the Library, both of which need `&mut app` while the closure still holds
    // `&mut app.session`. The same shape `photo_edited` uses, and for the same reason.
    photo_edited
}

/// The histogram well's height.
const HIST_HEIGHT: f32 = 120.0;

/// The histogram's channel colours. ⚠ Hand-painted constants outside the accent system, as
/// everything painted by hand is (visual identity spec, caveat 7).
const CHANNEL_COLOURS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(220, 80, 80),
    egui::Color32::from_rgb(80, 200, 80),
    egui::Color32::from_rgb(90, 120, 230),
];

/// A filled area under a 256-bin curve, as one mesh of 255 quads. Shared with the tone
/// curve's ghost, so the two draw the same shape from the same bins.
pub(crate) fn area_mesh(rect: egui::Rect, bins: &[f32; 256], colour: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let x = |i: usize| rect.left() + rect.width() * (i as f32 / 255.0);
    let y = |v: f32| rect.bottom() - rect.height() * v.clamp(0.0, 1.0);
    for i in 0..255 {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x(i), rect.bottom()), colour);
        mesh.colored_vertex(egui::pos2(x(i), y(bins[i])), colour);
        mesh.colored_vertex(egui::pos2(x(i + 1), rect.bottom()), colour);
        mesh.colored_vertex(egui::pos2(x(i + 1), y(bins[i + 1])), colour);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    mesh
}

/// The histogram: three filled channels in a well, with a clipping mark at either top corner
/// when an edge bin holds more than `histogram::CLIP_FRACTION` of the frame.
fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    // ⚠ The scale lives in `histogram.rs`, shared with the tone curve's ghost — see `normalised`.
    // Both sites used to compute their own maximum, linearly, which is what let one flat area
    // squash the whole graph.
    let bins = rustroom_app::histogram::normalised(hist);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HIST_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, v.extreme_bg_color);
    let plot = rect.shrink2(egui::vec2(0.0, 4.0));
    for (ci, chan) in bins.iter().enumerate() {
        painter.add(egui::Shape::mesh(area_mesh(
            plot,
            chan,
            CHANNEL_COLOURS[ci].linear_multiply(0.55),
        )));
    }
    let (left, right) = rustroom_app::histogram::clipped_edges(hist);
    let mark = |corner: egui::Pos2, dir: f32| {
        painter.add(egui::Shape::convex_polygon(
            vec![
                corner,
                corner + egui::vec2(6.0 * dir, 0.0),
                corner + egui::vec2(0.0, 6.0),
            ],
            v.strong_text_color(),
            egui::Stroke::NONE,
        ));
    };
    if left {
        mark(rect.left_top() + egui::vec2(1.0, 1.0), 1.0);
    }
    if right {
        mark(rect.right_top() + egui::vec2(-1.0, 1.0), -1.0);
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

    // ⚠ The explanation is the combo's hover (`docs/DESIGN.md` §8), not three lines of 9-pt text
    // under it: it said the same thing on every photo, which is what the author could not read.
    const PROFILE_HELP: &str =
        "Chooses this camera's colour profile, including any .dcp file you put in the profiles folder";
    ui.horizontal(|ui| {
        ui.label("Camera profile").hover(PROFILE_HELP);
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
            })
            .response
            .hover(PROFILE_HELP);
    });
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

    /// ⚠ The button moved to the top of the stack on 2026-09-19; this pins that it is still on
    /// screen with a session open, still scoped (the crop survives), and still one history entry.
    #[test]
    fn reset_adjustments_sits_at_the_top_and_resets_the_sliders_only() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.exposure = 2.0;
            s.edits.geometry.crop.left = 0.25;
        }
        assert!(
            !app.session.as_ref().unwrap().history.can_undo(),
            "a fresh session has nothing to undo"
        );
        crate::test_ui::click(&ctx, "Reset adjustments", |ui| {
            show(&mut app, ui);
        });
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.edits.exposure, 0.0, "the sliders were not reset");
        assert_eq!(s.edits.geometry.crop.left, 0.25, "the crop must survive");
        assert!(s.history.can_undo(), "one click, one history entry");
    }

    /// ⚠⚠ **Reported in use 2026-09-29:** with "Apply lens correction to new photos" ticked, an
    /// untouched photo's Lens section showed a reset as though it had been edited, and both "Reset
    /// adjustments" paths turned the default correction off. The preference is the photo's starting
    /// point, not an edit. ⚠ Both halves: turning the correction off must still read as an edit
    /// (the harness can see a reset at all), and the button and the menu command both restore it.
    #[test]
    fn a_default_lens_correction_is_not_an_edit() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        app.settings.auto_lens_correction = true;
        let fresh = app.settings.initial_edits(None);
        assert!(
            fresh.lens_correction.is_some_and(|l| l.is_active()),
            "sample: the preference must seed a correction"
        );
        app.session.as_mut().unwrap().edits = fresh.clone();
        let resets = |app: &mut RustroomApp| {
            crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                show(app, ui);
            })
            .into_iter()
            .filter(|(l, _)| l == "reset")
            .count()
        };
        assert_eq!(resets(&mut app), 0, "an untouched photo offered a reset");

        app.session.as_mut().unwrap().edits.lens_correction = None;
        assert_eq!(
            resets(&mut app),
            1,
            "turning the default correction off must read as an edit"
        );
        crate::test_ui::click(&ctx, "Reset adjustments", |ui| {
            show(&mut app, ui);
        });
        assert_eq!(
            app.session.as_ref().unwrap().edits.lens_correction,
            fresh.lens_correction,
            "the panel's Reset adjustments must restore the default correction"
        );

        app.session.as_mut().unwrap().edits.lens_correction = None;
        app.reset_adjustments();
        assert_eq!(
            app.session.as_ref().unwrap().edits.lens_correction,
            fresh.lens_correction,
            "the menu's Reset adjustments must restore it too"
        );
    }

    /// Every labelled rect the whole panel draws for `tool`, at a real window's size, after the
    /// sections have settled — the panel's own width, not a harness's.
    fn panel_rects(
        app: &mut RustroomApp,
        ctx: &egui::Context,
        tool: Tool,
    ) -> Vec<(String, egui::Rect)> {
        app.tool = tool;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 3000.0));
        let mut found = Vec::new();
        for _ in 0..3 {
            found = crate::test_ui::labelled_rects_on(ctx, screen, |ui| {
                show(app, ui);
            });
        }
        found
    }

    fn rect_of(found: &[(String, egui::Rect)], label: &str) -> egui::Rect {
        found
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("{label} was not drawn"))
    }

    /// ⚠⚠ The author's pick, 2026-09-29 (option A on the canvas): the curve's two choices on two rows
    /// at the control row's height — its five labels on one compact row had shrunk to 9.35 pt in
    /// 20-pt cells. Measured at the panel's real width.
    #[test]
    fn the_curve_selectors_are_two_full_height_rows() {
        let ctx = crate::test_ui::ctx();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        let found = panel_rects(&mut app, &ctx, Tool::Edit);
        let (point, param) = (
            rect_of(&found, "Point curve"),
            rect_of(&found, "Parametric"),
        );
        let chans = ["RGB", "Red", "Green", "Blue"].map(|l| rect_of(&found, l));
        assert!(
            (point.center().y - param.center().y).abs() < 0.5,
            "the modes share a row"
        );
        assert!(
            chans
                .iter()
                .all(|r| (r.center().y - chans[0].center().y).abs() < 0.5),
            "the four channels share a row: {chans:?}"
        );
        assert!(
            chans[0].top() > point.bottom(),
            "the channels sit under the modes"
        );
        for r in chans.iter().chain([&point, &param]) {
            assert!(
                r.height() >= 24.0,
                "a segment is shorter than the control row's: {r:?}"
            );
        }
    }

    /// ⚠⚠ The author's pick, 2026-09-29 (option B): the four mask modes as a 2 × 2 block with an
    /// icon each, never three and one. Measured at the panel's real width.
    #[test]
    fn the_mask_modes_are_a_two_by_two_block() {
        let ctx = crate::test_ui::ctx();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        let found = panel_rects(&mut app, &ctx, Tool::Masks);
        let cells = ["New mask", "Add", "Subtract", "Intersect"].map(|l| rect_of(&found, l));
        let mut ys: Vec<i32> = cells.iter().map(|r| r.center().y.round() as i32).collect();
        let mut xs: Vec<i32> = cells.iter().map(|r| r.center().x.round() as i32).collect();
        ys.sort_unstable();
        ys.dedup();
        xs.sort_unstable();
        xs.dedup();
        assert_eq!(
            (ys.len(), xs.len()),
            (2, 2),
            "not two rows of two: {cells:?}"
        );
        assert!(
            cells.iter().all(|r| r.height() >= 30.0),
            "the cells are not the larger size: {cells:?}"
        );
    }

    /// ⚠ The curve's open state is a **setting**, saved when it changes — collapsed to keep it out
    /// of the way, it must stay collapsed across a restart. The file is read back, not the field.
    #[test]
    fn collapsing_the_tone_curve_is_saved() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        app.tool = Tool::Edit;
        assert!(
            app.settings.tone_curve_open,
            "sample: the curve starts open"
        );
        crate::test_ui::click(&ctx, "Tone curve", |ui| {
            show(&mut app, ui);
        });
        assert!(!app.settings.tone_curve_open, "the click did not close it");
        assert!(
            !app.settings_store.load().tone_curve_open,
            "closed, but not saved — it would reopen on the next launch"
        );
    }

    /// ⚠⚠ **The panel must not widen frame over frame.** A row whose content is wider than the
    /// space it was given makes egui grow the panel to fit, which gives the row more space, which
    /// it again exceeds by the same amount — "the sidecar keeps expanding whenever I move the
    /// mouse" (2026-09-19). A sub-label that had become a full-width row, nested inside the
    /// tone curve's header row, did exactly that. Measured on the Profile header's centre, which
    /// tracks the panel's left edge; a wide Temp value is set so the slider row is exercised too.
    #[test]
    fn the_panel_keeps_its_width_frame_over_frame() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
        // ⚠ A wide value on purpose — "12500 K" is the widest the Temp row can show.
        app.session.as_mut().unwrap().edits.white_balance.kelvin = Some(12500.0);
        let centre = |app: &mut RustroomApp, frame: usize| {
            crate::test_ui::labelled_centres(
                &ctx,
                vec![egui::Event::PointerMoved(egui::pos2(
                    10.0 + frame as f32,
                    10.0,
                ))],
                |ui| {
                    show(app, ui);
                },
            )
            .into_iter()
            .find(|(l, _)| l == "Profile")
            .map(|(_, p)| p.x)
            .expect("the Profile header was not drawn")
        };
        let first = centre(&mut app, 0);
        for frame in 1..6 {
            let x = centre(&mut app, frame);
            assert_eq!(x, first, "the panel widened by frame {frame}");
        }
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

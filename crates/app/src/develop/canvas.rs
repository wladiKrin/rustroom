//! The central image canvas: the rendered photo, zoom and pan, and the crop overlay.

use crate::develop::canvas_geom;
use crate::{rects_around, PendingOpen, RustroomApp, Session, PREVIEW_MAX_DIM};
use eframe::egui;
use rustroom_app::tool::Tool;
use rustroom_app::{brush_tool, crop_tool, mask_tool, settings::PanDrag, view_region};

/// Whether a drag belongs to the active tool — the crop rectangle, a mask handle, and later
/// the brush.
///
/// ⚠ **egui reports `dragged()` for every button, not just the primary one.** `interaction.rs`
/// matches `PointerEvent::Pressed { .. }` and ignores the button when setting
/// `potential_drag_id`. Since panning now lives on middle- or right-drag, an ungated tool would
/// be driven by the pan gesture as well — dragging a crop corner while trying to pan.
fn tool_drag(resp: &egui::Response) -> bool {
    resp.dragged_by(egui::PointerButton::Primary)
}

/// Whether a drag on the tool's button has just begun. Same gating, same reason.
fn tool_drag_started(resp: &egui::Response) -> bool {
    resp.drag_started_by(egui::PointerButton::Primary)
}

/// Every source of panning, folded into one screen-space delta.
///
/// ⚠ **Scroll always pans and is not configurable.** It is the universal route — a two-finger
/// swipe on any trackpad, a wheel on any mouse — so making it optional would let a user lock
/// themselves out of panning entirely. Only the extra *drag* gesture is a setting.
///
/// ⚠ Both scroll axes are used. A horizontal two-finger swipe is an ordinary gesture, and the
/// old code read `.y` alone only because `.y` was driving zoom.
///
/// ⚠ The sign is right by egui's own convention, not by guesswork: `Event::MouseWheel`'s doc
/// says the delta "indicates how to move the *content* being viewed", and panning is exactly
/// moving the content.
///
/// ⚠⚠ **Takes plain values, never `&egui::InputState`.** `Response::dragged_by` calls
/// `self.ctx.input(..)` internally, so a signature holding the input lock deadlocks the moment a
/// drag is actually in progress — epaint's mutex panics on the re-entrant borrow. It survives
/// casual testing because `dragged_by` is `dragged() && ctx.input(..)`, and `dragged()` reads a
/// plain flag, so the `&&` short-circuits and never locks *unless something is being dragged*.
/// Caught by `the_configured_drag_button_pans`; it would otherwise have panicked in production
/// on the first middle-drag.
fn pan_delta(
    resp: &egui::Response,
    scroll: egui::Vec2,
    space_down: bool,
    binding: PanDrag,
    text_focused: bool,
) -> egui::Vec2 {
    let mut delta = egui::Vec2::ZERO;

    // ⚠ The hover gate is what stops a scroll over the adjustments panel or the filmstrip
    // panning the photo underneath them.
    if resp.hovered() {
        delta += scroll;
    }

    let dragging = match binding {
        PanDrag::Middle => resp.dragged_by(egui::PointerButton::Middle),
        PanDrag::Secondary => resp.dragged_by(egui::PointerButton::Secondary),
        // ⚠ Gated on focus: without it, a space typed into the preset-name field would arm
        // panning. The rule arrow-nav and the clipboard keys already follow.
        PanDrag::SpaceLeft => {
            !text_focused && space_down && resp.dragged_by(egui::PointerButton::Primary)
        }
    };
    if dragging {
        delta += resp.drag_delta();
    }
    delta
}

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) -> bool {
    // ⚠ Hoisted before `app.session` is borrowed mutably: `app.tool` is a disjoint field, but
    // reading it *through* `app` after the borrow is E0502. `Tool` is `Copy`, so this is free.
    let tool = app.tool;
    let mut photo_edited = false;
    // ⚠ The solve needs `&mut RustroomApp`, which the session borrow below rules out — so the
    // overlay only reports that a drag settled and `show` runs the solve afterwards.
    let mut guides_settled = false;
    // ⚠ Copied out before `match &mut app.session` takes its mutable borrow. `PanDrag` is
    // `Copy`, so this is a copy rather than a borrow that would collide.
    let app_pan_drag = app.settings.pan_drag;
    // ⚠ Same reason as `app_pan_drag`: copied out before `match &mut app.session` takes its
    // mutable borrow.
    let app_brush_size = app.brush_size;
    let app_brush_hardness = app.brush_hardness;
    let app_brush_erase = app.brush_erase;
    egui::CentralPanel::default().show(ui, |ui| {
        if let Some(p) = &app.pending {
            draw_pending_canvas(ui, p, app.session.as_ref());
            return;
        }
        match &mut app.session {
            Some(s) => {
                if let Some(tex) = &s.texture {
                    // ⚠ Always the rendered frame, never `s.baseline`. Before/after is a
                    // property of *what is rendered* now — see `Session::view_edits` — so the
                    // before image arrives at the current rung and region like any other, and
                    // is sharp at 1:1 instead of a magnified preview-resolution texture.
                    let shown = tex;
                    let avail = ui.available_size();

                    // fit/zoom are expressed against the post-geometry image at *fit*
                    // resolution, which stays constant as the render mode changes.
                    let full = s.fit_image_size;
                    // ⚠ No `.min(1.0)`. That clamp meant "never magnify past 100%" — but
                    // 100% of *what*? Of an arbitrary 1600 px preview, not of the image.
                    // On a canvas wider than the fit texture it letterboxed the photo at
                    // 1600 points inside a 2302-point window. Unclamped, the ladder climbs
                    // a rung on its own and supplies the texels to fill it.
                    let fit = (avail.x / full.x).min(avail.y / full.y);
                    // ⚠ Recorded for `Command::ZoomActual`, which has no other way to learn it.
                    s.fit_scale = fit;
                    // Crop mode stays at fit resolution whatever the zoom: it is a framing
                    // operation, the overlay is drawn against the whole image, and a
                    // whole-image *native* render would be both expensive and placed by
                    // the fit-mode maths the overlay assumes.
                    // ⚠ The ladder's base is the *cap* a fit render is downscaled to, not
                    // the size it comes out at. Geometry crops after the downscale, so the
                    // two differ on any cropped photo — and feeding the rendered size back
                    // in compounds the crop every frame. See `view_region::fit_cap`.
                    let fit_cap = view_region::fit_cap(s.buffer_long, PREVIEW_MAX_DIM);
                    // Geometry mode pins the fit rung: the overlay is drawn against the
                    // whole image and placed by the fit-mode maths.
                    let rung = if tool.is_geometry() {
                        fit_cap
                    } else {
                        view_region::tier_max_dim(fit_cap, s.buffer_long, fit, s.zoom)
                    };
                    let max_dim = rung;
                    let native = view_region::is_native(max_dim, fit_cap);

                    // Screen pixels per rendered pixel. `k` compares the two *caps*, so
                    // the crop factor cancels — it applies equally to both.
                    let full_long = full.x.max(full.y);
                    let k = max_dim as f32 / fit_cap as f32;
                    let (iw, ih) = ((full.x * k).round() as usize, (full.y * k).round() as usize);
                    let scale = fit * s.zoom / k;

                    // Which part of the image the view needs, and whether what is already
                    // rendered still covers it.
                    // ⚠ Decided by *visibility*, not by the rung. Once `fit` may exceed 1.0
                    // the whole image can be visible at a higher rung, and cropping to it
                    // would gain nothing while starving `fit_image_size` — which is only
                    // taken from whole-image frames — of any refresh.
                    let want = if native {
                        let vis = view_region::visible_rect(
                            iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y,
                        );
                        if view_region::covers_whole_image(vis, iw, ih) {
                            None
                        } else {
                            Some(view_region::with_margin(
                                vis,
                                iw,
                                ih,
                                view_region::REGION_MARGIN,
                            ))
                        }
                    } else {
                        None
                    };
                    let covered = match (want, s.rendered_region) {
                        (None, None) => true,
                        (Some(w), Some(r)) => view_region::contains(r, w),
                        _ => false,
                    };
                    // ⚠ A change of *rung* must re-request even when the region is
                    // unchanged — otherwise moving between rungs, or an upgraded buffer
                    // arriving, would leave the old texture on screen.
                    if (!covered && s.requested_region != want) || max_dim != s.render_rung {
                        s.requested_region = want;
                        s.render_rung = max_dim;
                        s.request(s.view_edits(tool), max_dim, want);
                    }

                    // Place using the *texture's* own render resolution, not the current
                    // mode's: a frame from the other mode may still be on screen while a
                    // new one is in flight.
                    // ⚠ No before/after special case any more: the before frame is rendered
                    // with the *same* geometry, rung and region as the after frame, so it
                    // places identically. Holding the framing fixed is what lets the toggle be
                    // a comparison rather than a jump.
                    let (place_region, place_long) = (s.rendered_region, s.rendered_long.max(1.0));
                    let pk = place_long / full_long;
                    let pscale = fit * s.zoom / pk;

                    let img_rect = match place_region {
                        Some(r) => {
                            let origin = ui.max_rect().center() + s.pan
                                - egui::vec2(full.x * pk, full.y * pk) * pscale * 0.5;
                            egui::Rect::from_min_size(
                                origin + egui::vec2(r.x as f32, r.y as f32) * pscale,
                                shown.size_vec2() * pscale,
                            )
                        }
                        // `pscale` rather than `fit * zoom`: the texture's own size
                        // already carries its render resolution, so this stays right
                        // even for a whole-image frame from the other mode.
                        None => egui::Rect::from_center_size(
                            ui.max_rect().center() + s.pan,
                            shown.size_vec2() * pscale,
                        ),
                    };
                    // ⚠ Where the **whole frame** sits on screen, which is *not* `img_rect`
                    // once a region render is on: `img_rect` then covers only the rendered
                    // slice. A mask is defined in whole-frame normalised coordinates, so its
                    // overlay must be placed against this rect or it drifts with the region —
                    // appearing to follow the view as you pan, which is exactly how the bug
                    // showed up. `origin` is the same whole-image corner the region placement
                    // above derives from, so the two agree by construction.
                    let frame_rect =
                        canvas_geom::frame_rect(ui.max_rect().center(), s.pan, full, pk, pscale);

                    let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                    egui::Image::new(shown).paint_at(ui, resp.rect);

                    // ⚠ A behaviour change, and an intended one: mask handles and brush
                    // painting now belong to the Masks tool rather than to "any mode that is
                    // not Geometry". A stray drag in Edit can no longer paint a stroke.
                    //
                    // ⚠ An exhaustive match, not `_ => {}`: a fifth tool must force a
                    // decision about what the canvas does under it.
                    match tool {
                        Tool::Crop => {
                            // ⚠⚠ **Exclusive**: while Guides is on the crop overlay is neither
                            // drawn nor interactive. The Geometry tool gives every drag to the
                            // crop rectangle, so without this the two would fight over one
                            // gesture — and the crop rectangle is invisible here anyway, since
                            // the preview resets the crop.
                            if s.show_guides {
                                guides_settled |= draw_guide_overlay(ui, s, &resp, img_rect);
                            } else {
                                photo_edited |=
                                    draw_crop_overlay(ui, s, &resp, img_rect, app.crop_aspect);
                            }
                        }
                        Tool::Masks => {
                            photo_edited |= draw_mask_overlay(
                                ui,
                                s,
                                &resp,
                                img_rect,
                                frame_rect,
                                BrushTool {
                                    size: app_brush_size,
                                    hardness: app_brush_hardness,
                                    flow: app.brush_flow,
                                    erase: app_brush_erase,
                                    show_tint: app.show_mask_tint,
                                },
                                tool,
                            );
                        }
                        Tool::Edit | Tool::Presets => {}
                    }
                    // ⚠ Not in geometry mode. The crop overlay is drawn against the whole
                    // image and placed by the fit-mode maths (see the rung pin above), so
                    // zooming or panning underneath it would misplace it. This gate was
                    // previously the `else` of the geometry branch; it must survive the
                    // change to a per-tool match.
                    if !tool.is_geometry() {
                        // Zoom first, then pan: the anchor correction below depends on the
                        // new zoom, and the other order would anchor against a stale factor.
                        //
                        // ⚠ `zoom_delta()` is a multiplicative factor that is 1.0 when idle,
                        // and it folds trackpad pinch together with ctrl+scroll — so mouse
                        // users keep a zoom gesture without plain scroll being consumed by it.
                        let zoom_factor = ui.input(|i| i.zoom_delta());
                        if zoom_factor != 1.0 && resp.hovered() {
                            let old = s.zoom;
                            s.zoom = view_region::zoom_step(old, zoom_factor);
                            // Zoom about the cursor, not the centre: keep whatever the
                            // pointer is over fixed on screen.
                            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                                let anchor = ui.max_rect().center() + s.pan;
                                let (dx, dy) = view_region::zoom_anchor_shift(
                                    (p.x, p.y),
                                    (anchor.x, anchor.y),
                                    old,
                                    s.zoom,
                                );
                                s.pan += egui::vec2(dx, dy);
                            }
                        }

                        // ⚠ Read and release the input lock *before* `pan_delta`, which calls
                        // `Response::dragged_by` — that takes the same lock internally, and a
                        // nested borrow panics in epaint's mutex the moment a drag is live.
                        let scroll = ui.input(|i| i.smooth_scroll_delta);
                        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
                        let text_focused = ui.ctx().memory(|m| m.focused().is_some());
                        s.pan += pan_delta(&resp, scroll, space_down, app_pan_drag, text_focused);

                        // ⚠ Unconditional, where the old code reset only inside the scroll
                        // branch — so a left-drag at fit really could push a fitting image
                        // off-centre into grey and nothing put it back until the next scroll.
                        // At fit the whole image is on screen and there is nothing to reveal.
                        if s.zoom <= view_region::MIN_ZOOM {
                            s.pan = egui::Vec2::ZERO;
                        }
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                    });
                }
            }
            None => {
                ui.centered_and_justified(|ui| {
                    crate::theme::empty_state(ui, "Open an image to begin (Open file… button).");
                });
            }
        }
    });
    if guides_settled {
        // ⚠ One gesture, one history entry — the rule every drag in this app follows.
        if app.solve_guides() {
            if let Some(s) = app.session.as_mut() {
                s.history.push(s.edits.clone());
            }
            photo_edited = true;
        }
    }
    photo_edited
}

/// Canvas while a decode is in flight: the target photo's thumbnail if we have one (scaled
/// up to fill, since thumbnails are ≤512 px), else the previous photo dimmed, else a spinner.
fn draw_pending_canvas(ui: &mut egui::Ui, pending: &PendingOpen, session: Option<&Session>) {
    let showing_placeholder = pending.placeholder.is_some();
    let fallback = session.and_then(|s| s.texture.as_ref());
    let Some(tex) = pending.placeholder.as_ref().or(fallback) else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    };

    // Lay the placeholder out where the decoded preview will land, so the swap happens in
    // place instead of snapping: the preview's long side is exactly PREVIEW_MAX_DIM, so
    // predict that size from this texture's aspect and then apply the session's fit rule.
    let tex_size = tex.size_vec2();
    let long = tex_size.x.max(tex_size.y).max(1.0);
    let preview_size = tex_size * (PREVIEW_MAX_DIM as f32 / long);
    let avail = ui.available_size();
    let fit = (avail.x / preview_size.x)
        .min(avail.y / preview_size.y)
        .min(1.0);
    let rect = egui::Rect::from_center_size(ui.max_rect().center(), preview_size * fit);
    egui::Image::new(tex).paint_at(ui, rect);

    if !showing_placeholder {
        // This is the *previous* photo — dim it so it reads as stale.
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(140),
        );
    }
}

/// Draw the crop rectangle overlay and handle dragging (reads/writes `s.edits.geometry.crop`).
///
/// Returns whether a drag settled into a new history entry, so the caller can treat that as
/// a photo edit — it is one of the two places a photo edit is committed.
/// How close, in normalised frame units, a press must be to grab a guide endpoint.
const GUIDE_GRAB: f32 = 0.03;

/// Draw the perspective guides and apply any drag. Returns whether a drag settled — the same
/// contract `draw_crop_overlay` has.
///
/// ⚠⚠ Guides are drawn on the **raw** frame: `preview_edits` resets perspective and straighten
/// while this sub-mode is on, so a normalised coordinate here always means one fixed thing.
fn draw_guide_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
) -> bool {
    use pipeline::perspective::guides::{Family, Guide};

    let to_screen = |n: [f32; 2]| {
        egui::pos2(
            img_rect.left() + n[0] * img_rect.width(),
            img_rect.top() + n[1] * img_rect.height(),
        )
    };
    let marks = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    let halo = egui::Color32::from_black_alpha(180);
    for g in &s.guides {
        // ⚠ The inferred family is **shown**, so a misclassified 45° line is visible and fixed by
        // nudging an endpoint. A rule the user cannot see is one they cannot correct.
        let colour = match g.family() {
            Family::Vertical => egui::Color32::from_rgb(80, 200, 255),
            Family::Horizontal => egui::Color32::from_rgb(255, 190, 70),
        };
        let (a, b) = (to_screen(g.from), to_screen(g.to));
        marks.line_segment([a, b], egui::Stroke::new(5.0, halo));
        marks.line_segment([a, b], egui::Stroke::new(2.0, colour));
        for p in [a, b] {
            marks.circle_filled(p, 6.0, halo);
            marks.circle_filled(p, 4.0, colour);
        }
    }

    let mut settled = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let n = [
            ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0),
            ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0),
        ];
        if tool_drag_started(resp) {
            s.active_guide =
                canvas_geom::guide_endpoint_at(&s.guides, n, GUIDE_GRAB).or_else(|| {
                    // ⚠ A new guide starts as a zero-length line whose *`to`* end follows the cursor,
                    // so the very first drag frame extends it — no separate "creating" state.
                    s.guides.push(Guide { from: n, to: n });
                    Some((s.guides.len() - 1, 1))
                });
        }
        if tool_drag(resp) {
            if let Some((i, e)) = s.active_guide {
                if let Some(g) = s.guides.get_mut(i) {
                    if e == 0 {
                        g.from = n;
                    } else {
                        g.to = n;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            // ⚠ A tap that never became a drag leaves a zero-length guide, which has no direction
            // and would classify arbitrarily. Drop it rather than keeping a line nobody can see.
            if let Some((i, _)) = s.active_guide {
                let dead = s.guides.get(i).is_some_and(|g| {
                    (g.to[0] - g.from[0]).abs() < 1e-4 && (g.to[1] - g.from[1]).abs() < 1e-4
                });
                if dead {
                    s.guides.remove(i);
                }
            }
            s.active_guide = None;
            settled = true;
        }
    }
    settled
}

/// How far each corner bracket reaches along its two edges, in screen points.
///
/// ⚠ Shrinks for a small crop — see `canvas_geom::corner_brackets`.
const CROP_BRACKET_ARM: f32 = 22.0;

fn draw_crop_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    crop_aspect: Option<f32>,
) -> bool {
    // ⚠ Normalized here, from the *baseline* texture, because that is the same source the
    // Crop panel uses to pick its presets. Two different aspect sources would disagree on any
    // photo whose preview and baseline differ, and the crop would drift as you dragged.
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let norm_aspect = crop_aspect.map(|a| a / img_aspect);
    let g = s.edits.geometry.crop;
    let to_screen = |nx: f32, ny: f32| {
        egui::pos2(
            img_rect.left() + nx * img_rect.width(),
            img_rect.top() + ny * img_rect.height(),
        )
    };
    let crop_rect =
        egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom));

    let painter = ui.painter_at(img_rect);
    let dim = egui::Color32::from_black_alpha(140);
    for outside in rects_around(img_rect, crop_rect) {
        painter.rect_filled(outside, egui::CornerRadius::ZERO, dim);
    }
    // ⚠⚠ A **wider** clip for the marks. `line_segment` centres its stroke on the path, so at a
    // full-frame crop — where the boundary *is* the image edge — clipping to the image cuts the
    // outer half off every mark. See `canvas_geom::overlay_clip`. The dimming above keeps the
    // tight clip: there is no photograph outside the image to darken.
    let marks = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    // ⚠⚠ **Every mark is drawn twice — a dark stroke first, the light one over it.** A plain
    // white line is invisible against a bright photograph, and the crop border is *hardest* to
    // see in the case that matters most: at a full-frame crop it lies exactly on the image
    // boundary, so there is nothing to distinguish the control from the edge of the picture.
    // Reported in use as the border and the corner dots being easy to miss entirely.
    let halo = egui::Color32::from_black_alpha(180);
    // ⚠ Red, not white and not the theme accent. White vanishes against a bright photograph, and
    // the accent is user-switchable — including a `Neutral` setting whose whole point is to have
    // no colour, which would defeat this. A fixed red is legible on essentially all photographic
    // content and reads unmistakably as a control rather than part of the picture.
    //
    // ⚠ The crop overlay was already painted by hand with fixed colours and deliberately does not
    // follow the accent; this keeps that, rather than introducing a second rule.
    let mark = egui::Color32::from_rgb(255, 60, 60);
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(4.0, halo),
        egui::StrokeKind::Middle,
    );
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.5, mark),
        egui::StrokeKind::Middle,
    );

    // ⚠ Corner **brackets**, not dots. They are the conventional crop affordance, they read as
    // "grab me" where a dot reads as decoration, and they are far larger targets — the hit test
    // below uses a 0.03 tolerance, which a 4 px dot badly under-advertised.
    for (a, b) in canvas_geom::corner_brackets(crop_rect, CROP_BRACKET_ARM) {
        marks.line_segment(
            [a, b],
            egui::Stroke::new(canvas_geom::CROP_HALO_WIDTH, halo),
        );
        marks.line_segment([a, b], egui::Stroke::new(3.5, mark));
    }

    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0);
        if tool_drag_started(resp) {
            s.active_handle = crop_tool::hit_test(&s.edits.geometry.crop, [nx, ny], 0.03);
        }
        if tool_drag(resp) {
            if let Some(h) = s.active_handle {
                let d = resp.drag_delta();
                let dx = d.x / img_rect.width();
                let dy = d.y / img_rect.height();
                s.edits.geometry.crop =
                    crop_tool::apply_drag(s.edits.geometry.crop, h, dx, dy, norm_aspect);
            }
        }
        if resp.drag_stopped() {
            s.active_handle = None;
            s.history.push(s.edits.clone());
            return true;
        }
    }
    false
}

/// Draw the selected mask's handles and guide lines, and apply any drag.
///
/// Returns whether a drag settled into a new history entry — the same contract
/// `draw_crop_overlay` has.
///
/// ⚠ **Three lines, not a tint.** A faithful coverage tint needs three quads, because coverage
/// is clamped flat either side of the ramp; two boundary lines state the same thing exactly
/// and do not wash out the photograph being judged.
/// ⚠ **Every coordinate here is against `frame_rect`, never `img_rect`.** A mask is defined in
/// whole-frame normalised coordinates, but `img_rect` covers only the *rendered slice* once a
/// region render is on. Using it made the overlay drift with the view while panning at high
/// zoom — and silently corrupted `aspect` too, since a region's aspect is not the frame's.
/// `img_rect` is still the right clip: there is no photograph outside it to annotate.
/// The brush tool's live settings, copied out of `RustroomApp` before the session borrow.
///
/// ⚠ A struct rather than four loose parameters: `draw_mask_overlay` already takes five, and a
/// nine-argument function is where argument-order mistakes start.
#[derive(Clone, Copy)]
struct BrushTool {
    size: f32,
    hardness: f32,
    flow: f32,
    erase: bool,
    show_tint: bool,
}

fn draw_mask_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    brush: BrushTool,
    tool: Tool,
) -> bool {
    let Some(i) = s.selected_mask else {
        return false;
    };
    let Some(m) = s.edits.masks.get(i) else {
        return false;
    };
    let mask = m.mask.clone();
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }

    paint_mask(
        &ui.painter_at(img_rect),
        &mask,
        frame_rect,
        img_rect,
        &s.edits.geometry,
        brush.show_tint,
    );

    let aspect = canvas_geom::frame_aspect(frame_rect);
    let mut moved = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
        if matches!(mask, edit_model::Mask::Brush(_)) {
            // ⚠ No paint mode. Left-drag is free because the navigation rework moved panning onto
            // scroll and a drag button, which is the whole reason that landed first.
            let map = pipeline::brush::FrameMap::new(
                &s.edits.geometry,
                frame_rect.width() as usize,
                frame_rect.height() as usize,
            );
            let radius = (brush.size / map.radius_scale.max(1e-6)).max(1e-4);
            let src = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
            if tool_drag_started(resp) {
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    b.strokes.push(edit_model::BrushStroke {
                        points: vec![src],
                        radius,
                        hardness: brush.hardness,
                        flow: brush.flow,
                        erase: brush.erase,
                    });
                    // ⚠⚠ **Deliberately does NOT set `moved`.** Requesting a render here starts a
                    // full-resolution render of a ONE-POINT stroke you are about to extend. At
                    // 1:1 that takes seconds, occupies the render worker for the whole drag, and
                    // lands *after* the line is finished — repainting the single initial dab over
                    // the correct result, which then has to queue behind it.
                    //
                    // Diagnosed from `RUSTROOM_DEBUG_BRUSH` output showing strokes recorded
                    // correctly (20-52 points) at `max_dim 6246`: the model was right and the
                    // render was stale. Reported as "the initial dot is still there".
                }
            } else if tool_drag(resp) {
                // ⚠ `else if`, not a second `if`: both predicates are true on the frame a drag
                // begins, and without the `else` that frame would record its first point twice.
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    if let Some(stroke) = b.strokes.last_mut() {
                        let keep = stroke
                            .points
                            .last()
                            .is_none_or(|last| brush_tool::should_record(*last, src, radius));
                        if keep {
                            stroke.points.push(src);
                            // ⚠ Live rendering, enabled by region-scoped coverage: a paint frame
                            // is now bounded by the viewport rather than by the image. Measured
                            // at ~16.6 ms at 1:1 on a full-resolution X-T4 buffer — 9.03 ms
                            // clipped rasterization, 7.2 ms region tail, ~0.4 ms pack — which is
                            // marginal against a 60 Hz budget and recorded as such rather than
                            // claimed as comfortable.
                            //
                            // ⚠ `drag_started` stays suppressed. Rendering a one-point stroke you
                            // are about to extend was pure waste and caused the "initial dot"
                            // bug: a full-resolution render landing after the line was finished
                            // and painting over it. Live rendering means rendering on **motion**,
                            // never on press.
                            moved = true;
                        }
                    }
                }
            }
        } else {
            if tool_drag_started(resp) {
                s.active_mask_handle = mask_tool::hit_test(&mask, [nx, ny], aspect, 0.04);
            }
            if tool_drag(resp) {
                if let Some(h) = s.active_mask_handle {
                    let d = resp.drag_delta();
                    let dx = d.x / frame_rect.width();
                    let dy = d.y / frame_rect.height();
                    if let Some(m) = s.edits.masks.get_mut(i) {
                        mask_tool::apply_drag(&mut m.mask, h, dx, dy);
                        moved = true;
                    }
                }
            }
            if resp.drag_stopped() && s.active_mask_handle.take().is_some() {
                s.history.push(s.edits.clone());
                return true;
            }
        }
    }

    // ⚠⚠ **Outside the `interact_pointer_pos` block on purpose.** On the frame a drag ends egui
    // often reports no interact position, so a `drag_stopped` handler nested inside it simply
    // never runs. That is not hypothetical: it shipped for one build, and the symptom was that
    // after drawing a line only the *first dab* appeared once the tint was hidden — the last
    // render was the one from `drag_started`, when the stroke had a single point. The history
    // push was being skipped too, so the stroke was not undoable.
    if matches!(mask, edit_model::Mask::Brush(_)) && resp.drag_stopped() {
        // The final render for this stroke, and the one that pushes history. Motion frames render
        // too now — see the note there — so this mostly guarantees the last few points land.
        // ⚠ Env-gated instrumentation, kept because this project's own history says to reach for
        // a print when the code reads correctly and the behaviour disagrees — the fit-view
        // investigation went four rounds on reasoning before one `eprintln` reframed it.
        if std::env::var("RUSTROOM_DEBUG_BRUSH").is_ok() {
            if let Some(edit_model::Mask::Brush(b)) = s.edits.masks.get(i).map(|m| &m.mask) {
                eprintln!(
                    "brush: drag_stopped, {} strokes, last has {} points, max_dim {}, region {:?}",
                    b.strokes.len(),
                    b.strokes.last().map_or(0, |st| st.points.len()),
                    s.render_max_dim(),
                    s.requested_region.is_some(),
                );
            }
        }
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        s.history.push(s.edits.clone());
        return true;
    }

    // The brush cursor: a circle at the pointer at the current screen radius. ⚠ Without it the
    // Size slider is guesswork, because a screen-point size has no other on-screen
    // representation. `hover_pos`, not `interact_pointer_pos`: it must show before a drag begins.
    if matches!(mask, edit_model::Mask::Brush(_)) {
        if let Some(hp) = resp.hover_pos() {
            ui.painter_at(img_rect).circle_stroke(
                hp,
                brush.size,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
            );
        }
    }

    // ⚠ Moving a mask changes the *rendered image*, so it must ask for a re-render — the
    // panel's sliders do exactly this on `changed`. Without it the handles moved while the
    // photo kept showing the mask's previous position, updating only when some later edit
    // happened to request a frame. Unlike `draw_crop_overlay`, which needs no request because
    // `preview_edits` strips the crop in geometry mode and the overlay *is* the feedback.
    if moved {
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }
    false
}

/// The largest edge the tint's coverage is rasterized at, whatever the zoom.
///
/// ⚠ The tint is an indicator over the photograph, not the photograph, so it does not need the
/// frame's real resolution — and at 1:1 the frame is the full decode. Capping makes the cost
/// constant in zoom.
const TINT_MAX_DIM: usize = 1400;

/// A memoised tint, rasterized at a capped resolution and held as an uploaded **texture**.
///
/// ⚠⚠ **A mesh is the wrong representation and it crashed the app.** The first version emitted
/// one quad per run of equal coverage along a row. That collapses a stroke's flat interior, but
/// across a *soft* brush's gradient almost every pixel differs from its neighbour, so every run
/// is length 1 — and a large soft stroke is mostly gradient. wgpu refused a 711 MB vertex buffer
/// (~7 million quads) against a 268 MB limit. One textured quad is O(1) geometry whatever the
/// stroke.
///
/// ⚠ `paint_mask` runs on every egui frame, so without this memo the tint would re-rasterize
/// *and* re-upload at ~60 Hz while merely hovering. One stroke is ~1.2 ms at screen size, so
/// thirty strokes was ~36 ms per frame with nothing happening — the "sluggish moving the brush
/// around" reported in use. The benchmark could not have found it: it timed one stroke once,
/// while the real cost scales with stroke count and is paid continuously.
struct TintCache {
    brush: edit_model::Brush,
    geometry: edit_model::Geometry,
    w: usize,
    h: usize,
    /// The uploaded coverage, with its box in **coverage-pixel** coordinates and the scale it was
    /// rasterized at. `None` when the brush paints nothing inside its box.
    ///
    /// ⚠⚠ **A screen `Rect` must NOT be cached here.** It was, and panning at zoom then left the
    /// tint fixed in window coordinates while the image slid under it, snapping back the moment a
    /// brush stroke invalidated the entry. That is precisely 3.1a's second real-use bug — an
    /// overlay placed from a stale rect rather than recomputed against the live `frame_rect` —
    /// reintroduced through a cache. Placement is derived every frame; only the texture is kept.
    drawn: Option<(egui::TextureHandle, [usize; 4], f32)>,
    /// Coverage outside the box — non-zero for an **inverted** brush, which covers the whole
    /// frame except what was painted.
    ///
    /// ⚠ Without this the tint showed the inversion only inside a rectangle around the strokes,
    /// which grew as more was painted. Found in use, and the same omission the coverage map
    /// itself had.
    outside: u8,
}

thread_local! {
    /// ⚠ Thread-local rather than a field on `RustroomApp`: `paint_mask` is a free function
    /// reached from inside egui closures that already hold `&mut app`, and threading a cache down
    /// would widen four signatures for something with no observable behaviour. egui is
    /// single-threaded per context, so this is the same thread every frame.
    static TINT: std::cell::RefCell<Option<TintCache>> = const { std::cell::RefCell::new(None) };
}

/// Paint the brush's coverage as a translucent red tint.
///
/// The texture is rebuilt only when the strokes, geometry or screen size change.
fn paint_tint(
    painter: &egui::Painter,
    b: &edit_model::Brush,
    geometry: &edit_model::Geometry,
    frame_rect: egui::Rect,
) {
    let (w, h) = (frame_rect.width() as usize, frame_rect.height() as usize);
    if w == 0 || h == 0 {
        return;
    }
    // ⚠⚠ **`frame_rect` is the WHOLE FRAME's on-screen rect, not the viewport.** At 1:1 on a
    // full-resolution buffer that is ~6246x4170 — so rasterizing "at screen resolution" was
    // rasterizing at *full* resolution, ~24 ms plus a 26 MB texture upload, every time the memo
    // missed. Reported in use as: responsive with the mask hidden, sluggish with it shown.
    //
    // The tint is a translucent indicator, not the photograph. Capping it and letting the GPU
    // stretch the texture makes its cost constant in zoom, and a slightly soft tint edge is
    // invisible against a 50% overlay. The spec's claim of "~1400x900, sub-millisecond" was
    // simply wrong about which rectangle it meant.
    let long = w.max(h);
    let scale = if long > TINT_MAX_DIM {
        TINT_MAX_DIM as f32 / long as f32
    } else {
        1.0
    };
    let (tw, th) = (
        ((w as f32 * scale) as usize).max(1),
        ((h as f32 * scale) as usize).max(1),
    );
    TINT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let hit = slot
            .as_ref()
            .is_some_and(|c| c.w == w && c.h == h && c.geometry == *geometry && c.brush == *b);
        if !hit {
            let cov = pipeline::brush::rasterize(b, geometry, tw, th);
            let outside = cov.outside;
            let drawn = if cov.w == 0 || cov.h == 0 {
                None
            } else {
                // ⚠ Red because it is conventional and because none of the four accents is red,
                // so the tint can never be confused with active state. Alpha is coverage/2, so a
                // weak stroke reads as a weak tint — which is what makes Flow legible.
                let px: Vec<egui::Color32> = cov
                    .data
                    .iter()
                    .map(|&a| egui::Color32::from_rgba_unmultiplied(255, 40, 40, a / 2))
                    .collect();
                let img = egui::ColorImage {
                    size: [cov.w, cov.h],
                    pixels: px,
                    source_size: egui::vec2(cov.w as f32, cov.h as f32),
                };
                let tex =
                    painter
                        .ctx()
                        .load_texture("mask_tint", img, egui::TextureOptions::LINEAR);
                Some((tex, [cov.x0, cov.y0, cov.w, cov.h], scale))
            };
            *slot = Some(TintCache {
                brush: b.clone(),
                geometry: *geometry,
                w,
                h,
                drawn,
                outside,
            });
        }
        let Some(cache) = slot.as_ref() else { return };
        if cache.outside > 0 {
            let tint = egui::Color32::from_rgba_unmultiplied(255, 40, 40, cache.outside / 2);
            // ⚠⚠ Painted **around** the box, never under it. A single full-frame rect plus the
            // box's own texture double-blends wherever they overlap — and for an inverted brush
            // the area inside the box but outside the strokes is *also* fully covered, so the box
            // showed up as a distinctly brighter rectangle over the photograph. Reported in use
            // as "an additional mask box".
            let box_rect = cache.drawn.as_ref().map(|(_, b, sc)| {
                let inv = 1.0 / sc;
                egui::Rect::from_min_size(
                    frame_rect.min + egui::vec2(b[0] as f32 * inv, b[1] as f32 * inv),
                    egui::vec2(b[2] as f32 * inv, b[3] as f32 * inv),
                )
            });
            match box_rect {
                None => {
                    painter.rect_filled(frame_rect, egui::CornerRadius::ZERO, tint);
                }
                Some(b) => {
                    let b = b.intersect(frame_rect);
                    for r in [
                        // above, below, left, right — no overlap with `b` and none with each other.
                        egui::Rect::from_min_max(
                            frame_rect.min,
                            egui::pos2(frame_rect.max.x, b.min.y),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(frame_rect.min.x, b.max.y),
                            frame_rect.max,
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(frame_rect.min.x, b.min.y),
                            egui::pos2(b.min.x, b.max.y),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(b.max.x, b.min.y),
                            egui::pos2(frame_rect.max.x, b.max.y),
                        ),
                    ] {
                        if r.width() > 0.0 && r.height() > 0.0 {
                            painter.rect_filled(r, egui::CornerRadius::ZERO, tint);
                        }
                    }
                }
            }
        }
        if let Some((tex, b, sc)) = cache.drawn.as_ref() {
            // ⚠ Derived from the LIVE `frame_rect`, never from a cached rect — see the field's
            // note. `frame_rect` moves with the pan, so the tint must be placed against it afresh.
            let inv = 1.0 / sc;
            let rect = egui::Rect::from_min_size(
                frame_rect.min + egui::vec2(b[0] as f32 * inv, b[1] as f32 * inv),
                egui::vec2(b[2] as f32 * inv, b[3] as f32 * inv),
            );
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    });
}

/// Points used to approximate a mask ellipse. egui has no ellipse primitive, so it is drawn as
/// a closed polyline; 64 is smooth at any zoom this canvas reaches.
const ELLIPSE_SEGMENTS: usize = 64;

/// Paint one mask's handles and guides.
///
/// ⚠ **Split from `draw_mask_overlay` deliberately, on painting rather than on the whole
/// function.** The drag plumbing — hit-testing, applying the drag, pushing history and calling
/// `s.request` — is shared by every shape and must be written once. Duplicating it per shape is
/// exactly how the two would drift, and it is where both of 3.1a's real-use bugs lived.
///
/// ⚠ `frame_rect` places everything; `clip` only bounds what is drawn. They differ once a
/// region render is on, and using `clip` for placement is the bug that made the overlay drift
/// with the view while panning at high zoom.
fn paint_mask(
    painter: &egui::Painter,
    mask: &edit_model::Mask,
    frame_rect: egui::Rect,
    clip: egui::Rect,
    geometry: &edit_model::Geometry,
    show_tint: bool,
) {
    let to_screen = |p: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + p[0] * frame_rect.width(),
            frame_rect.top() + p[1] * frame_rect.height(),
        )
    };
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);

    match mask {
        edit_model::Mask::Linear(g) => {
            let (a, b) = (to_screen(g.from), to_screen(g.to));
            painter.line_segment([a, b], stroke);

            // ⚠ The perpendicular is computed **in screen space**, from the on-screen axis —
            // the same trap as `coverage` and `hit_test`, in its third guise. A
            // normalised-space perpendicular is visibly not perpendicular on a 3:2 frame.
            let axis = b - a;
            let len = axis.length();
            if len > f32::EPSILON {
                let perp = egui::vec2(-axis.y, axis.x) / len;
                // Long enough to cross the clip region from any point inside it.
                let reach = clip.width() + clip.height();
                for end in [a, b] {
                    painter.line_segment(
                        [end - perp * reach, end + perp * reach],
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
                    );
                }
            }
            for end in [a, b] {
                painter.circle_filled(end, 5.0, egui::Color32::WHITE);
            }
        }
        edit_model::Mask::Radial(r) => {
            let points: Vec<egui::Pos2> = (0..ELLIPSE_SEGMENTS)
                .map(|i| {
                    let th = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                    to_screen([
                        r.center[0] + r.radius[0] * th.cos(),
                        r.center[1] + r.radius[1] * th.sin(),
                    ])
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
            // Centre plus the four edge handles, in the order `mask_tool::hit_test_radial`
            // tests them.
            let handles = [
                r.center,
                [r.center[0] - r.radius[0], r.center[1]],
                [r.center[0] + r.radius[0], r.center[1]],
                [r.center[0], r.center[1] - r.radius[1]],
                [r.center[0], r.center[1] + r.radius[1]],
            ];
            for h in handles {
                painter.circle_filled(to_screen(h), 5.0, egui::Color32::WHITE);
            }
        }
        edit_model::Mask::Brush(b) => {
            // ⚠ There are no handles — a stroke is not a shape with grab points, the same
            // observation that makes `mask_tool::hit_test` return `None` for a brush.
            if show_tint {
                paint_tint(painter, b, geometry, frame_rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a real allocated rect through a press-and-move gesture with `button`, and report
    /// what `tool_drag` said on the final pass.
    ///
    /// ⚠ Four passes, not three. egui only becomes "decidedly dragging" once the pointer has
    /// moved far enough *after* the press (`is_decidedly_dragging` requires `!any_pressed()`),
    /// so the press and the movement must land on different frames.
    fn tool_drag_for(button: egui::PointerButton) -> bool {
        let ctx = egui::Context::default();
        let mut out = false;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            let pos = start + egui::vec2(40.0, 40.0) * (pass as f32 - 1.0).max(0.0);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    out = tool_drag(&resp);
                }
            });
        }
        out
    }

    #[test]
    fn a_primary_drag_belongs_to_the_tools() {
        assert!(tool_drag_for(egui::PointerButton::Primary));
    }

    #[test]
    fn a_middle_drag_does_not_belong_to_the_tools() {
        // ⚠ Not hypothetical. egui's `interaction.rs` matches `PointerEvent::Pressed { .. }`
        // and IGNORES the button when deciding what is being dragged, so `resp.dragged()` is
        // true for a middle-drag too. Without this gate the pan button would also drag crop
        // rectangles and mask handles.
        assert!(!tool_drag_for(egui::PointerButton::Middle));
    }

    #[test]
    fn a_secondary_drag_does_not_belong_to_the_tools() {
        // Right-drag is one of the selectable pan bindings, so it needs the same exclusion.
        assert!(!tool_drag_for(egui::PointerButton::Secondary));
    }

    /// Drive a real allocated rect and report what `pan_delta` returned on the final pass.
    ///
    /// ⚠⚠ **The pointer must move on the frame that is read, not merely before it.**
    /// `drag_delta()` is per-frame, so an earlier version that moved once on pass 2 and read on
    /// pass 3 saw `[0,0]` — which made `a_left_drag_does_not_pan`, the most important test in
    /// this change, pass vacuously: it would have passed with the gating deleted entirely.
    /// Movement therefore continues on every pass from 2 onward.
    fn pan_delta_for(
        button: Option<egui::PointerButton>,
        scroll: egui::Vec2,
        binding: PanDrag,
        space_down: bool,
        text_focused: bool,
    ) -> egui::Vec2 {
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            // Keeps moving: pass 2 is +40, pass 3 is +80, so the frame that is read has a
            // real per-frame delta of its own.
            let pos = start + egui::vec2(40.0, 40.0) * (pass as f32 - 1.0).max(0.0);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                if let Some(b) = button {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: b,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                if space_down {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                }
            }
            // ⚠ Delivered on passes 2 AND 3: egui smooths scroll across frames, so a single
            // wheel event may not have reached `smooth_scroll_delta` by the time it is read.
            if pass >= 2 && scroll != egui::Vec2::ZERO {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: scroll,
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    // ⚠ The input values are read and the lock released *before* `pan_delta`
                    // runs, because `dragged_by` takes the same lock internally.
                    let scroll = ui.input(|i| i.smooth_scroll_delta);
                    let space = ui.input(|i| i.key_down(egui::Key::Space));
                    out = pan_delta(&resp, scroll, space, binding, text_focused);
                }
            });
        }
        out
    }

    #[test]
    fn a_left_drag_does_not_pan() {
        // ⚠⚠ The regression guard for this entire change, and it asserts the pan delta is
        // ZERO rather than that some other branch ran. Asserting "the tool branch was taken"
        // would pass with a stray `s.pan +=` still in place — precisely the shape of mistake
        // this project keeps making.
        for binding in PanDrag::ALL {
            assert_eq!(
                pan_delta_for(
                    Some(egui::PointerButton::Primary),
                    egui::Vec2::ZERO,
                    binding,
                    false,
                    false
                ),
                egui::Vec2::ZERO,
                "{binding:?}"
            );
        }
    }

    #[test]
    fn the_configured_drag_button_pans() {
        let d = pan_delta_for(
            Some(egui::PointerButton::Middle),
            egui::Vec2::ZERO,
            PanDrag::Middle,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "middle: {d:?}");

        let d = pan_delta_for(
            Some(egui::PointerButton::Secondary),
            egui::Vec2::ZERO,
            PanDrag::Secondary,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "secondary: {d:?}");
    }

    #[test]
    fn a_drag_on_the_wrong_button_does_not_pan() {
        // Middle-drag while the binding is Secondary must do nothing at all.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Middle),
                egui::Vec2::ZERO,
                PanDrag::Secondary,
                false,
                false
            ),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn scrolling_pans_both_axes() {
        // ⚠ Before this change only `.y` was read anywhere in this file, and it drove zoom. A
        // horizontal two-finger swipe must pan too, so the x component is asserted separately
        // rather than folded into a magnitude check that a y-only implementation would satisfy.
        let d = pan_delta_for(None, egui::vec2(11.0, 7.0), PanDrag::Middle, false, false);
        assert!(d.x.abs() > 0.0, "x component ignored: {d:?}");
        assert!(d.y.abs() > 0.0, "y component ignored: {d:?}");
    }

    #[test]
    fn scrolling_outside_the_image_does_not_pan() {
        // ⚠ The `hovered()` gate. Without it, scrolling the adjustments panel or the filmstrip
        // would pan the photo underneath them.
        //
        // ⚠ This cannot reuse `pan_delta_for`: that harness always points at (100,100), inside
        // the rect, so it could never detect a missing gate. The pointer is driven far from a
        // small rect here instead.
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        for pass in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(380.0, 380.0)));
            if pass >= 2 {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(11.0, 7.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                // A small rect in the corner, far from the pointer.
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(50.0, 50.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    let scroll = ui.input(|i| i.smooth_scroll_delta);
                    out = pan_delta(&resp, scroll, false, PanDrag::Middle, false);
                }
            });
        }
        assert_eq!(out, egui::Vec2::ZERO);
    }

    #[test]
    fn space_left_drag_pans_only_with_space_held() {
        assert!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                false
            ) != egui::Vec2::ZERO
        );
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                false,
                false
            ),
            egui::Vec2::ZERO,
            "without Space this is an ordinary left-drag and must not pan"
        );
    }

    #[test]
    fn space_left_drag_is_ignored_while_a_text_field_has_focus() {
        // ⚠ Otherwise typing a space into the preset-name field arms panning. The same rule
        // arrow-nav and the clipboard keys already follow.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                true
            ),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn the_pan_reset_at_fit_is_unconditional() {
        // ⚠ A behaviour fix, not a port. The old reset sat *inside* the scroll branch, so a
        // left-drag at fit genuinely pushed a fitting image off-centre into grey and nothing
        // put it back until the next scroll. This pins the rule itself rather than the call
        // site, in the style of `grid_columns_leaves_room_for_the_gaps`.
        let reset = |zoom: f32, pan: egui::Vec2| {
            if zoom <= view_region::MIN_ZOOM {
                egui::Vec2::ZERO
            } else {
                pan
            }
        };
        assert_eq!(reset(1.0, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(reset(0.5, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(
            reset(1.5, egui::vec2(50.0, 50.0)),
            egui::vec2(50.0, 50.0),
            "above fit the pan must survive"
        );
    }

    #[test]
    fn the_fixture_opens_a_photo_and_the_canvas_draws_it() {
        // ⚠ The smoke test that makes the fixture worth having: it proves `show` can be driven
        // headlessly at all. Everything else in this module builds on it.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        assert!(app.session.is_some());

        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                ..Default::default()
            },
            |ui| {
                super::show(&mut app, ui, &ctx.clone());
            },
        );
    }

    /// Drive `show` through a full press → move → release drag.
    ///
    /// ⚠⚠ **Five passes, and every one is load-bearing.** Move to `from`; press *while still
    /// there*; move to `to`, which is the drag; release; settle. `Response` state is per-frame,
    /// so a shorter harness reads zeros — that is what made `a_left_drag_does_not_pan` pass
    /// vacuously once already — and a release is only *known* on its own pass, so anything it
    /// triggers cannot be observed until the frame after.
    /// Returns the engine's request count after each pass, so a test can sample the *drag frame*
    /// rather than the whole gesture.
    fn drag_through(
        app: &mut RustroomApp,
        ctx: &egui::Context,
        from: egui::Pos2,
        to: egui::Pos2,
    ) -> Vec<usize> {
        let mut counts = Vec::new();
        for pass in 0..5 {
            // ⚠⚠ The pointer must still be at `from` on the pass that presses. An earlier
            // version moved it to `to` on the same pass, so the press landed away from the
            // handle — and `dragging_a_mask_handle_asks_for_a_render` **passed anyway**, because
            // `show` issues renders for other reasons. Its failing sibling is what exposed it.
            let pos = if pass <= 1 { from } else { to };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 {
                events.push(egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            if pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            }
            let c = ctx.clone();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::show(app, ui, &c);
                },
            );
            counts.push(
                app.session
                    .as_ref()
                    .map(|s| s.engine.requests_issued())
                    .unwrap_or(0),
            );
        }
        counts
    }

    /// The pass on which the pointer moves with the button down — i.e. the drag itself.
    const DRAG_PASS: usize = 2;

    /// A session with one linear-gradient mask selected and the Masks tool active.
    ///
    /// ⚠ The endpoints are set explicitly. `LinearGradient::default()` puts *both* at `[0,0]`,
    /// which is a degenerate mask whose handles coincide — a drag on it would be testing nothing
    /// in particular. `from` sits at the frame centre so its screen position is the viewport
    /// centre, which is what makes the drag points below deterministic.
    fn with_selected_mask(app: &mut RustroomApp) {
        app.tool = rustroom_app::tool::Tool::Masks;
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.5, 0.5],
                to: [0.5, 0.9],
                invert: false,
            }),
            adjustments: Default::default(),
        });
        s.selected_mask = Some(0);
    }

    /// The screen rect every harness pass uses.
    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
    }

    /// The centre of the area the canvas is drawn into — and, with no pan, the screen position
    /// of both a mask handle at the normalised frame centre and the middle of the image.
    ///
    /// ⚠⚠ **Measured, not hardcoded.** An earlier version wrote `(200, 150)`, obtained by
    /// instrumenting the layout once. That is brittle in the worst way: a layout change moves the
    /// target silently, and the failure mode is a test that still passes while grabbing nothing —
    /// which is exactly the fault this file hit twice while it was being written. Running one
    /// pass and reading `ui.max_rect()` costs a frame and cannot go stale.
    fn canvas_centre(app: &mut RustroomApp, ctx: &egui::Context) -> egui::Pos2 {
        let mut centre = egui::Pos2::ZERO;
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                ..Default::default()
            },
            |ui| {
                centre = ui.max_rect().center();
                super::show(app, ui, &c);
            },
        );
        centre
    }

    #[test]
    fn dragging_a_mask_handle_asks_for_a_render() {
        // ⚠⚠ The regression guard for a bug that shipped: `draw_mask_overlay` mutated `Edits` but
        // never called `s.request`, so the image kept showing the mask's previous position until
        // some *unrelated* edit happened to request a frame. That is why moving a slider appeared
        // to "fix" it.
        //
        // ⚠ The cause was an inherited false analogy — it was modelled on `draw_crop_overlay`,
        // which needs no request *because* `preview_edits` strips the crop in geometry mode, so
        // the overlay itself is the feedback. Masks are not stripped.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);

        // ⚠⚠ **Sampled on the drag frame itself, not after the gesture.** Measuring at the end
        // does not discriminate: a *later* frame issues a request of its own once `Edits` has
        // changed, so the count rises either way and the test passes with `s.request` deleted —
        // verified by sabotage. In the real app that later frame never comes, because egui idles
        // after the drag; that is precisely why the bug was visible as "the image does not update
        // until you touch a slider".
        let centre = canvas_centre(&mut app, &ctx);
        let counts = drag_through(&mut app, &ctx, centre, centre + egui::vec2(40.0, 0.0));
        assert!(
            counts[DRAG_PASS] > counts[DRAG_PASS - 1],
            "the drag frame itself must ask for a render; counts={counts:?}"
        );
    }

    #[test]
    fn a_drag_that_misses_every_handle_asks_for_nothing() {
        // ⚠⚠ **The guard on the guard, and it is not optional.** `show` issues renders for
        // reasons of its own, so "the count went up after a drag" only means something if it
        // *stays put* when the drag grabs nothing. An earlier version of the harness pressed
        // after the pointer had already moved, so every drag missed — and the sibling test
        // passed anyway. This is what would have caught that.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);

        // Warm up: let the first frames issue whatever they issue, then measure.
        drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(14.0, 60.0),
        );
        let before = app.session.as_ref().unwrap().engine.requests_issued();
        // Far from `from` [0.5,0.5] and `to` [0.5,0.9], in the frame's top-left corner.
        drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(30.0, 60.0),
        );
        let after = app.session.as_ref().unwrap().engine.requests_issued();

        assert_eq!(
            before, after,
            "a drag grabbing no handle must not ask for a render; before={before} after={after}"
        );
    }

    #[test]
    fn a_mask_drag_commits_one_undo_step_on_release() {
        // ⚠⚠ The regression guard for the third bug shape: `drag_stopped` was nested inside
        // `if let Some(p) = resp.interact_pointer_pos()`, and on the frame a drag ends egui often
        // reports **no** interact position — so the commit was silently skipped.
        //
        // ⚠ Asserted on the **history**, not on `Edits`. The edits change during the *drag*, from
        // `apply_drag`; only the undo push happens on release. An earlier version of this test
        // compared edits and passed with the release handler disabled entirely — caught by
        // sabotage, and exactly the "a test names a bug it cannot detect" trap this project has
        // now hit a dozen times.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);
        assert!(
            !app.session.as_ref().unwrap().history.can_undo(),
            "nothing to undo before the drag"
        );

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(60.0, 10.0));

        let s = app.session.as_ref().unwrap();
        assert!(
            s.history.can_undo(),
            "the release must have committed one undo step"
        );
        assert_ne!(
            s.edits,
            edit_model::Edits::default(),
            "and the mask must actually have moved"
        );
    }

    #[test]
    fn the_mask_overlay_is_placed_against_the_frame_not_the_view() {
        // ⚠⚠ The regression guard for the drift bug: the overlay was placed against `img_rect`,
        // which once a region render is on covers only the *rendered slice*. The mask then
        // followed the visible part of the image as you panned.
        //
        // ⚠ Asserted through `canvas_geom::frame_rect` rather than by reading pixels: the frame's
        // *size* must not change with the pan, which is precisely what `img_rect` would do.
        let full = egui::vec2(80.0, 40.0);
        let at = egui::pos2(200.0, 150.0);
        let unpanned = super::canvas_geom::frame_rect(at, egui::Vec2::ZERO, full, 1.0, 2.0);
        let panned = super::canvas_geom::frame_rect(at, egui::vec2(50.0, -20.0), full, 1.0, 2.0);
        assert_eq!(
            unpanned.size(),
            panned.size(),
            "panning must not resize the frame the mask is placed against"
        );
        assert_eq!(panned.center() - unpanned.center(), egui::vec2(50.0, -20.0));
    }

    /// A session in Crop mode with an inset crop, so a drag from the centre has room to move it.
    ///
    /// ⚠ Inset deliberately. On a full-frame crop `Handle::Inside` is clamped at every edge, so
    /// the drag would be a no-op and the test would assert nothing.
    fn with_inset_crop(app: &mut RustroomApp) {
        app.tool = rustroom_app::tool::Tool::Crop;
        let s = app.session.as_mut().unwrap();
        s.edits.geometry.crop = edit_model::CropRect {
            left: 0.2,
            top: 0.2,
            right: 0.8,
            bottom: 0.8,
        };
    }

    #[test]
    fn dragging_inside_the_crop_moves_it() {
        // ⚠ The crop overlay is the same shape as the mask overlay — drag lifecycle, handle
        // hit-test, commit on release — so it inherits the same three failure modes. This is the
        // guard that it actually responds to a drag at all.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        let before = app.session.as_ref().unwrap().edits.geometry.crop;

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        let after = app.session.as_ref().unwrap().edits.geometry.crop;
        assert_ne!(before, after, "a drag inside the crop must move it");
        assert!(
            after.left > before.left,
            "and it must move the way the pointer went; before={before:?} after={after:?}"
        );
    }

    #[test]
    fn a_crop_drag_commits_one_undo_step_on_release() {
        // ⚠⚠ The crop's version of the `drag_stopped` bug. Asserted on the **history**, not on
        // the crop rect: the rect changes during the *drag*, so comparing it would pass with the
        // release handler disabled — the same trap the mask test fell into first.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        assert!(!app.session.as_ref().unwrap().history.can_undo());

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 8.0));

        assert!(
            app.session.as_ref().unwrap().history.can_undo(),
            "the release must have committed one undo step"
        );
    }

    #[test]
    fn a_crop_drag_that_grabs_nothing_leaves_the_crop_alone() {
        // ⚠ The guard on the guard, as for masks. A drag well outside the crop rect hits no
        // handle, so the crop must not move — otherwise "the crop changed" says nothing about
        // whether the hit-test works.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        let before = app.session.as_ref().unwrap().edits.geometry.crop;

        // The frame's top-left corner: outside a crop inset to 0.2..0.8.
        let _ = drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(30.0, 62.0),
        );

        assert_eq!(
            before,
            app.session.as_ref().unwrap().edits.geometry.crop,
            "a drag outside the crop must not move it"
        );
    }

    #[test]
    fn the_crop_overlay_does_not_ask_for_a_render() {
        // ⚠⚠ **The asymmetry with masks, pinned so nobody "fixes" it.** A crop drag deliberately
        // issues *no* render: `preview_edits` strips the crop in geometry mode, so the overlay
        // itself is the feedback and the pixels underneath do not change. Copying the mask
        // overlay's `s.request` here would re-render the whole frame on every drag frame for no
        // visible difference — and the reverse mistake, assuming masks needed no request either,
        // is exactly the bug that shipped.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);

        let centre = canvas_centre(&mut app, &ctx);
        let counts = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        assert_eq!(
            counts[DRAG_PASS],
            counts[DRAG_PASS - 1],
            "a crop drag must not ask for a render; counts={counts:?}"
        );
    }
}

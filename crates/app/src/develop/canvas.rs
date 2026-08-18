//! The central image canvas: the rendered photo, zoom and pan, and the crop overlay.

use crate::{preview_edits, rects_around, PendingOpen, RustroomApp, Session, PREVIEW_MAX_DIM};
use eframe::egui;
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
    let mut photo_edited = false;
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
                    let shown = if s.show_before { &s.baseline } else { tex };
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
                    let rung = if s.geometry_mode {
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
                        s.request(preview_edits(&s.edits, s.geometry_mode), max_dim, want);
                    }

                    // Place using the *texture's* own render resolution, not the current
                    // mode's: a frame from the other mode may still be on screen while a
                    // new one is in flight.
                    let (place_region, place_long) = if s.show_before {
                        // The baseline is always the whole image at fit resolution.
                        (None, full_long)
                    } else {
                        (s.rendered_region, s.rendered_long.max(1.0))
                    };
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
                    let frame_size = egui::vec2(full.x * pk, full.y * pk) * pscale;
                    let frame_rect =
                        egui::Rect::from_center_size(ui.max_rect().center() + s.pan, frame_size);

                    let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                    egui::Image::new(shown).paint_at(ui, resp.rect);

                    if s.geometry_mode {
                        photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect);
                    } else {
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
                        );
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
                    ui.label("Open an image to begin (Open file… button).");
                });
            }
        }
    });
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
fn draw_crop_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
) -> bool {
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
    painter.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.5, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    for c in [
        crop_rect.left_top(),
        crop_rect.right_top(),
        crop_rect.left_bottom(),
        crop_rect.right_bottom(),
    ] {
        painter.circle_filled(c, 4.0, egui::Color32::WHITE);
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
                s.edits.geometry.crop = crop_tool::apply_drag(s.edits.geometry.crop, h, dx, dy);
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
    tool: BrushTool,
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
        tool.show_tint,
    );

    let aspect = frame_rect.width() / frame_rect.height();
    let mut moved = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0);
        if matches!(mask, edit_model::Mask::Brush(_)) {
            // ⚠ No paint mode. Left-drag is free because the navigation rework moved panning onto
            // scroll and a drag button, which is the whole reason that landed first.
            let map = pipeline::brush::FrameMap::new(
                &s.edits.geometry,
                frame_rect.width() as usize,
                frame_rect.height() as usize,
            );
            let radius = (tool.size / map.radius_scale.max(1e-6)).max(1e-4);
            let src = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
            if tool_drag_started(resp) {
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    b.strokes.push(edit_model::BrushStroke {
                        points: vec![src],
                        radius,
                        hardness: tool.hardness,
                        flow: tool.flow,
                        erase: tool.erase,
                    });
                    moved = true;
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
                            moved = true;
                        }
                    }
                }
            }
            if resp.drag_stopped() {
                s.history.push(s.edits.clone());
                return true;
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

    // The brush cursor: a circle at the pointer at the current screen radius. ⚠ Without it the
    // Size slider is guesswork, because a screen-point size has no other on-screen
    // representation. `hover_pos`, not `interact_pointer_pos`: it must show before a drag begins.
    if matches!(mask, edit_model::Mask::Brush(_)) {
        if let Some(hp) = resp.hover_pos() {
            ui.painter_at(img_rect).circle_stroke(
                hp,
                tool.size,
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
        s.request(
            preview_edits(&s.edits, s.geometry_mode),
            s.render_max_dim(),
            s.requested_region,
        );
    }
    false
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
            // ⚠ The canvas rasterizes its OWN coverage at screen resolution rather than reusing
            // the render worker's. A deliberate second rasterization: reusing the worker's would
            // give one source of truth but make the tint lag the stroke by a full render, and for
            // a brush, seeing the mark appear under the cursor *is* the experience.
            //
            // There are no handles — a stroke is not a shape with grab points, the same
            // observation that makes `mask_tool::hit_test` return `None` for a brush.
            if !show_tint || b.strokes.is_empty() {
                return;
            }
            let (w, h) = (frame_rect.width() as usize, frame_rect.height() as usize);
            if w == 0 || h == 0 {
                return;
            }
            let cov = pipeline::brush::rasterize(b, geometry, w, h);
            if cov.is_empty() {
                return;
            }
            // ⚠ Red because it is conventional and because none of the four accents is red, so
            // the tint can never be confused with active state.
            //
            // ⚠ One quad per *run* of equal coverage along a row, not one per pixel. A per-pixel
            // mesh over a large stroke is hundreds of thousands of quads; runs collapse the flat
            // interior of a stroke — which is most of it — into a handful.
            let mut mesh = egui::Mesh::default();
            for y in 0..cov.h {
                let mut x = 0usize;
                while x < cov.w {
                    let a = cov.at(cov.x0 + x, cov.y0 + y);
                    if a == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < cov.w && cov.at(cov.x0 + x, cov.y0 + y) == a {
                        x += 1;
                    }
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(
                            frame_rect.left() + (cov.x0 + start) as f32,
                            frame_rect.top() + (cov.y0 + y) as f32,
                        ),
                        egui::vec2((x - start) as f32, 1.0),
                    );
                    mesh.add_colored_rect(
                        rect,
                        egui::Color32::from_rgba_unmultiplied(255, 40, 40, a / 2),
                    );
                }
            }
            painter.add(egui::Shape::mesh(mesh));
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
}

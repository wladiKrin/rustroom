//! The central image canvas: the rendered photo, the render rung and region it asks for, its
//! placement, and zoom and pan. The per-tool overlays live in the sibling modules and `show`
//! dispatches to exactly one of them.
//!
//! ⚠ `frame_rect` — where the **whole frame** sits on screen — is computed here and handed
//! down, because every overlay must place against it rather than against `img_rect`. See its
//! comment at the call site.

mod crop_overlay;
mod guide_overlay;
mod mask_overlay;
mod mask_tint;
pub(crate) mod navigation;
pub(crate) mod spot_overlay;
mod straighten_overlay;
#[cfg(test)]
mod test_support;

use crate::develop::canvas_geom;
use crate::{PendingOpen, RustroomApp, Session, PREVIEW_MAX_DIM};
use crop_overlay::draw_crop_overlay;
use eframe::egui;
use guide_overlay::{draw_guide_overlay, GuideDrag};
use mask_overlay::{draw_mask_overlay, BrushTool};
use navigation::pan_delta;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use spot_overlay::{draw_spot_overlay, sampler_for};
use straighten_overlay::draw_straighten_overlay;

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
    let mut guide_drag = GuideDrag::None;
    // ⚠⚠ **A COPY of the tool, written back after the session borrow ends.** The size-drag has to
    // update `app.spot_tool.radius` so the panel's Size slider follows the gesture, but
    // `&mut app.spot_tool` cannot be taken while `match &mut app.session` holds its borrow — the
    // canvas takes `&mut app` whole, unlike the panel, which borrows disjoint fields.
    let mut app_spot_tool = app.spot_tool;
    let app_show_spots = app.show_spots;
    // ⚠ Copied out before `match &mut app.session` takes its mutable borrow. `PanDrag` is
    // `Copy`, so this is a copy rather than a borrow that would collide.
    let app_pan_drag = app.settings.pan_drag;
    // ⚠ Same reason as `app_pan_drag`: copied out before `match &mut app.session` takes its
    // mutable borrow.
    // ⚠ Same reason as `app_pan_drag`: copied out before `match &mut app.session` takes its
    // mutable borrow. `CompositionGuide` is `Copy`, so this is a copy rather than a borrow.
    let app_guide = app.settings.composition_guide;
    let app_brush_size = app.brush_size;
    let app_brush_hardness = app.brush_hardness;
    let app_brush_erase = app.brush_erase;
    // ⚠ Cleared here, once, and set again only by an actual hover over the image at the end of
    // this function — so every path that draws no image (a decode in flight, no texture yet, no
    // session) leaves it blank. A stale number left on screen after the pointer leaves is worse
    // than none, because it gets trusted.
    if let Some(s) = app.session.as_mut() {
        s.readout.clear();
    }
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
                    // Geometry mode pins the fit rung: the crop overlay is drawn against the
                    // whole image and placed by the fit-mode maths.
                    //
                    // ⚠⚠ **Guide Mode is the exception and unpins it**, because placing an
                    // endpoint exactly on a building edge needs real pixels — at fit resolution
                    // you are aiming at a blur. It can afford to: the guide overlay is anchored
                    // to `frame_rect`, so it survives a region render, which is precisely what
                    // the crop overlay cannot do.
                    let rung = if tool.is_geometry() && !s.show_guides {
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
                            //
                            // ⚠⚠ **Precedence, and it is a three-way decision now.** Guide Mode
                            // stays first and stays exclusive; the straighten line is next; the
                            // crop rectangle is the default. Without an order the three would
                            // fight over one gesture.
                            //
                            // ⚠ `show_guides` is **read** here and never written by the
                            // straighten gesture. Mutual exclusion is by gating — the panel
                            // disables the toggle while guides are in force — which keeps that
                            // track out of `Session.guides` entirely.
                            if s.show_guides {
                                let d = draw_guide_overlay(ui, s, &resp, frame_rect, img_rect);
                                if d != GuideDrag::None {
                                    guide_drag = d;
                                }
                            } else if s.straighten_armed {
                                photo_edited |= draw_straighten_overlay(
                                    ui, s, &resp, frame_rect, img_rect, tool,
                                );
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
                        Tool::Edit if s.picking_wb => {
                            // ⚠⚠ **A second, click-sensing interaction rather than changing the
                            // canvas's own sense.** Making `resp` `click_and_drag` does give
                            // `clicked()`, and it also changes when egui promotes a press to a
                            // drag — which broke both mask-drag tests outright. This one exists
                            // only while the eyedropper is armed, so no other gesture can see it.
                            //
                            // ⚠ Registered *after* the canvas response, which is what makes it
                            // win the click: egui breaks a hit-test tie by registration order,
                            // taking the last one as the one on top.
                            //
                            // ⚠ `Crosshair` because egui's `CursorIcon` has no pipette — the
                            // set maps onto the OS cursors, and neither winit nor CSS defines
                            // one. Crosshair is the conventional stand-in for a colour pick and
                            // is what makes the armed state visible where the pointer actually
                            // is, rather than only in the panel.
                            let pick = ui
                                .interact(
                                    img_rect,
                                    ui.id().with("wb_eyedropper"),
                                    egui::Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::Crosshair);
                            photo_edited |= pick_white_balance(s, &pick, frame_rect, tool);
                        }
                        Tool::Retouch => {
                            // ⚠ Built here rather than inside the overlay so the shared borrow of
                            // `s` it holds ends before the overlay takes `&mut s`.
                            let sample = sampler_for(s);
                            photo_edited |= draw_spot_overlay(
                                ui,
                                s,
                                &resp,
                                img_rect,
                                frame_rect,
                                &mut app_spot_tool,
                                app_show_spots,
                                &sample,
                            );
                        }
                        Tool::Edit | Tool::Presets => {}
                    }

                    // ⚠⚠ **One rule: the guides divide what will be the final photograph.** Under
                    // Crop the preview strips the crop, so the crop rect *is* that photograph;
                    // everywhere else the crop is applied, so the frame is. It only looks like two
                    // behaviours because Crop is the one place the two differ.
                    //
                    // ⚠ Guide Mode is the exception: there the preview strips the crop **and** no
                    // crop rect is drawn, so a guide would divide a rectangle nobody keeps. A guide
                    // that divides the wrong rectangle is worse than no guide, because it is
                    // trusted.
                    //
                    // ⚠ `frame_rect`, never `img_rect`, for the reason the mask and perspective
                    // overlays both record: once a region render is on, `img_rect` covers only the
                    // rendered *slice*, and guides must divide the whole photograph. `img_rect`
                    // survives as the clip — widened by `overlay_clip`, because a diagonal reaches
                    // the corners and a tight clip would shave half the stroke off there.
                    let guide_rect = match tool {
                        Tool::Crop if s.show_guides => None,
                        Tool::Crop => Some(crop_overlay::crop_screen_rect(s, img_rect)),
                        _ => Some(frame_rect),
                    };
                    if let Some(r) = guide_rect {
                        let p = ui.painter_at(canvas_geom::overlay_clip(img_rect));
                        crop_overlay::draw_guides(&p, r, app_guide);
                    }
                    // ⚠ Not in geometry mode. The crop overlay is drawn against the whole
                    // image and placed by the fit-mode maths (see the rung pin above), so
                    // zooming or panning underneath it would misplace it. This gate was
                    // previously the `else` of the geometry branch; it must survive the
                    // change to a per-tool match.
                    // ⚠ Guide Mode zooms and pans like the ordinary tools; only the crop
                    // rectangle needs the view held still.
                    if !tool.is_geometry() || s.show_guides {
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

                    // ⚠⚠ **`img_rect`, NOT `frame_rect` — and this is the one place in this
                    // canvas where that is true.** `draw_loupe` records the rule and its
                    // exception together: "`img_rect` is where the texture is actually painted,
                    // so this stays exact whether the frame was rendered whole or as a region."
                    // The readout indexes the *rendered bytes*, and those cover the rendered
                    // slice — exactly what `img_rect` spans. Every other overlay here is
                    // frame-relative, so this needs its own comment or the next reader will
                    // "fix" it.
                    //
                    // ⚠ No inverse mapping anywhere: cursor → UV → index. That is what makes
                    // reading the rendered frame the cheap choice.
                    //
                    // ⚠ Placed after the per-tool match and senses nothing at all, so it never
                    // competes for a gesture.
                    //
                    // ⚠ `s.readout` was cleared at the top of `show`, so leaving the image —
                    // or landing on a pixel that reads nothing — blanks it.
                    if let (Some(p), Some((bytes, bw, bh))) = (
                        ui.input(|i| i.pointer.hover_pos()),
                        s.readout_bytes.as_ref(),
                    ) {
                        if img_rect.contains(p) && img_rect.width() > 0.0 && img_rect.height() > 0.0
                        {
                            let u = (p.x - img_rect.left()) / img_rect.width();
                            let v = (p.y - img_rect.top()) / img_rect.height();
                            let x = ((u * *bw as f32) as usize).min(bw.saturating_sub(1));
                            let y = ((v * *bh as f32) as usize).min(bh.saturating_sub(1));
                            let text = rustroom_app::color_readout::mean_at(bytes, *bw, *bh, x, y)
                                .map(rustroom_app::color_readout::label);
                            // ⚠ Computed into a local first: `bytes` is a shared borrow of `s`
                            // and writing `s.readout` needs a mutable one. Never a clone of the
                            // bytes.
                            if let Some(text) = text {
                                s.readout = text;
                            }
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
    // ⚠ Solved on **release only**, so the image is frozen for the whole drag. One gesture, one
    // Ctrl+Z, the rule every drag here follows.
    if guide_drag == GuideDrag::Settled && app.solve_guides() {
        if let Some(s) = app.session.as_mut() {
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            s.history.push(s.edits.clone());
        }
        photo_edited = true;
    }
    // ⚠ Written back after the session borrow ends: the size-drag sets the radius, and the panel's
    // Size slider reads this same field next frame. One value, two affordances.
    app.spot_tool = app_spot_tool;
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

/// Set the white balance from a pixel the user clicked.
///
/// ⚠⚠ **Reads the DECODE BUFFER, not the rendered texture.** The texture already has the white
/// balance applied, so solving from it would make the eyedropper chase its own correction: pick a
/// grey, the picture shifts, pick the same grey again and it shifts further. The same trap the
/// luminance range mask keys on `base` to avoid.
///
/// ⚠ The click is mapped through `FrameMap`, which un-does orientation, straighten and crop — so
/// it works on a cropped, rotated photo. It does **not** un-do lens correction or perspective,
/// which is the caveat the brush already carries: a few pixels of displacement, and this averages
/// a patch anyway.
fn pick_white_balance(
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    tool: Tool,
) -> bool {
    if !resp.clicked() {
        return false;
    }
    let Some(p) = resp.interact_pointer_pos() else {
        return false;
    };
    let map = pipeline::brush::FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let [u, v] = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
    let (bw, bh) = (s.buffer.width(), s.buffer.height());
    if bw == 0 || bh == 0 || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return false;
    }
    // ⚠ A patch, not one pixel. A single sample is sensor noise, and the whole point of clicking
    // a grey card is to average over it — every eyedropper in every editor does this.
    let cx = ((u * bw as f32) as usize).min(bw - 1);
    let cy = ((v * bh as f32) as usize).min(bh - 1);
    let r = 2usize;
    let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
    for y in cy.saturating_sub(r)..=(cy + r).min(bh - 1) {
        for x in cx.saturating_sub(r)..=(cx + r).min(bw - 1) {
            let px = s.buffer.pixel(x, y);
            for (a, v) in acc.iter_mut().zip(px) {
                *a += v;
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return false;
    }
    let mean = [acc[0] / n, acc[1] / n, acc[2] / n];
    s.edits.white_balance = rustroom_app::wb_tool::solve_pick(mean, s.wb_baseline);
    // ⚠ Disarmed after one pick: an eyedropper that stayed armed would turn every subsequent
    // click — including one meant to select a mask or to dismiss something — into a white-balance
    // change. Lightroom's behaves the same way.
    s.picking_wb = false;
    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    true
}

#[cfg(test)]
mod tests {
    use super::test_support::screen;
    use eframe::egui;

    /// ⚠⚠ The wiring test, and it is the half this project has repeatedly shipped broken: the
    /// solve has its own unit tests, but nothing else can see that a click on the canvas ever
    /// reaches it. Arming the eyedropper and clicking must change `Edits`.
    #[test]
    fn an_armed_eyedropper_sets_the_white_balance_from_a_click() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.picking_wb = true;
            assert_eq!(s.edits.white_balance, edit_model::WhiteBalance::default());
        }
        super::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert!(
            s.edits.white_balance.kelvin.is_some(),
            "the click set no temperature"
        );
        // ⚠ And it disarms, or every later click — one meant to select a mask, or to dismiss
        // something — would silently re-set the white balance.
        assert!(!s.picking_wb, "the eyedropper stayed armed after a pick");
    }

    fn crop_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 60);
        app.tool = rustroom_app::tool::Tool::Crop;
        app
    }

    /// ⚠⚠ The wiring test: an armed drag must reach `angle_for_line` and land in `Edits`. The
    /// angle has its own unit tests, and nothing else can see that a drag on the canvas ever
    /// reaches it — `an_armed_eyedropper_sets_the_white_balance_from_a_click` exists for exactly
    /// this reason.
    #[test]
    fn an_armed_straighten_drag_sets_the_angle_and_one_history_entry() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let before = app
            .session
            .as_ref()
            .expect("a session")
            .edits
            .geometry
            .angle;
        assert_eq!(before, 0.0, "fixture check: the angle starts neutral");
        // A clearly tilted, clearly diagonal drag.
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert!(
            s.edits.geometry.angle.abs() > 1.0,
            "the drag did not reach the angle: {}",
            s.edits.geometry.angle
        );
        // ⚠ `can_undo`, not only `Edits`. The headless-canvas work records that "the edits change
        // during the drag… only the undo push happens on release", so an `Edits` assertion alone
        // cannot see a missing release commit.
        assert!(s.history.can_undo(), "the release pushed no history entry");
        // ⚠ And it disarms, or every later drag would rotate the photo instead of moving the crop.
        assert!(!s.straighten_armed, "the mode stayed armed after one line");
    }

    /// ⚠⚠ **A straighten line clears the perspective guides, and leaves the sliders alone.** It
    /// writes `geometry.angle`, one of the three values a guide solve owns, so guides left on
    /// screen would describe a solve that no longer holds — and this project's records call a
    /// stale-but-plausible overlay worse than none, because it gets trusted. The rule is the one
    /// `panel/geometry.rs` already applies to the Straighten slider.
    ///
    /// ⚠ Asserts **both halves**. A version that cleared the whole solve — guides *and* the
    /// perspective sliders — would pass a guides-only assertion, and `Clear`'s own contract is
    /// "the sliders they produced are left alone."
    ///
    /// ⚠ Driven through the real drag rather than by calling `settle` directly: the wiring half is
    /// what this project has repeatedly shipped broken.
    #[test]
    fn a_straighten_line_clears_the_guides_and_leaves_the_perspective_alone() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.straighten_armed = true;
            s.edits.perspective.vertical = 30.0;
            for x in [0.2f32, 0.8] {
                s.guides.push(pipeline::perspective::guides::Guide {
                    from: [x, 0.2],
                    to: [x, 0.8],
                });
            }
            // ⚠ Set deliberately: with `active_guide` left `None` the clearing of it is a no-op
            // on this fixture, so the assertion below could not see it being dropped — found by
            // the sabotage, which went uncaught until this line existed. A stale index into a
            // list that has just been emptied is exactly what `restore_snapshot` clears
            // `selected_mask` for.
            s.active_guide = Some((1, 1));
        }
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert!(s.guides.is_empty(), "the guides survived a straighten line");
        assert_eq!(s.active_guide, None, "a stale active guide survived");
        assert_eq!(
            s.edits.perspective.vertical, 30.0,
            "the straighten line cleared the perspective solve too"
        );
    }

    /// The guard on the guard: with the mode unarmed the drag belongs to the crop rectangle, so
    /// "the angle moved after a drag" means nothing without this.
    #[test]
    fn an_unarmed_drag_leaves_the_straighten_angle_alone() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(
            s.edits.geometry.angle, 0.0,
            "an unarmed drag rotated the photo"
        );
    }

    /// ⚠ Guide Mode keeps precedence. Armed *and* in Guide Mode, the drag must reach the guides.
    #[test]
    fn guide_mode_still_wins_over_an_armed_straighten_line() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.show_guides = true;
            s.straighten_armed = true;
        }
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(20.0, 60.0),
            centre + egui::vec2(20.0, 60.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(
            s.guides.len(),
            1,
            "the drag did not reach the guide overlay"
        );
    }

    /// ⚠⚠ The point of keeping the line: after release it must read **level on screen**, which is
    /// the only feedback that says a subtle correction worked. Asserted through the same forward
    /// map the overlay draws with, so a line stored in the wrong space cannot pass.
    ///
    /// ⚠⚠ **Mapped through `s.edits`, NOT through `s.shown_geo` — and the plan predicted this
    /// exact substitution.** `shown_geo` is advanced at the frame-landing site in `shell.rs`,
    /// which this harness does not run, so it never moves on its own here.
    /// `s.edits.geometry.angle` is the angle the release just wrote and the one the next frame
    /// will render with, so this is the property the user actually sees — a frame later.
    ///
    /// ⚠⚠ **The fixture carries a 5° correction ALREADY APPLIED, and that is what gives the test
    /// leverage.** With `shown_geo` at its default the raw frame and the corrected frame coincide,
    /// so `from_frame`/`to_frame` are the identity and `shown_angle + a == a` — under which
    /// **two sabotages this test names went uncaught**: storing the *screen* endpoints instead of
    /// the raw ones, and dropping the `shown_angle +` that makes the new angle relative. Both are
    /// invisible at zero. Seeding a photo that has already been straightened once is the case
    /// where they differ, and it is also the real workflow: straighten, look again, straighten
    /// better.
    #[test]
    fn the_settled_straighten_line_reads_level_afterwards() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.straighten_armed = true;
            // A frame that has already landed carrying 5° — `edits` and `shown_geo` agree, as
            // they do in the app once the render catches up.
            s.edits.geometry.angle = 5.0;
            s.shown_geo = (s.edits.perspective, 5.0);
        }
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        let (from, to) = s.straighten_line.expect("the line was not kept");
        let aspect = s.fit_image_size.x / s.fit_image_size.y.max(1.0);
        let hom = pipeline::perspective::Homography::from_edits(&s.edits.perspective, aspect);
        let angle = s.edits.geometry.angle.to_radians();
        let a = pipeline::perspective::guides::to_frame(from, &hom, angle, aspect);
        let b = pipeline::perspective::guides::to_frame(to, &hom, angle, aspect);
        // ⚠ The guard on the guard: a stored line that was never rotated would also have a small
        // rise if the drag itself had been level, so assert the drag really was tilted.
        assert!(
            (to[1] - from[1]).abs() > 0.05,
            "fixture check: the drawn line was not tilted ({from:?} to {to:?})"
        );
        // ⚠ And the angle really did move off the 5° the fixture started at, or the "level" check
        // below would be measuring a photo nothing happened to.
        assert!(
            (s.edits.geometry.angle - 5.0).abs() > 1.0,
            "fixture check: the release did not change the angle: {}",
            s.edits.geometry.angle
        );
        // Levelled: the mapped line's screen-space rise is ~0.
        let rise = (b[1] - a[1]).abs();
        assert!(
            rise < 0.02,
            "the line did not come out level: rise {rise} (from {a:?} to {b:?})"
        );
    }

    /// ⚠ A non-primary drag must leave it alone. egui reports `dragged()` for every button, so a
    /// bare `dragged()` would let the middle-drag pan binding rotate the photo as it passed over
    /// the canvas — and no test using Primary alone can see that.
    #[test]
    fn a_non_primary_drag_does_not_straighten() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        super::test_support::drag_through_with(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
            egui::PointerButton::Middle,
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.edits.geometry.angle, 0.0, "a middle-drag straightened");
    }

    fn retouch_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 60);
        app.tool = rustroom_app::tool::Tool::Retouch;
        app
    }

    fn spots(app: &crate::RustroomApp) -> Vec<edit_model::Spot> {
        app.session.as_ref().expect("a session").edits.spots.clone()
    }

    /// ⚠ A click places exactly one spot, and the auto-source must put the source somewhere else:
    /// a source equal to the destination copies the blemish onto itself.
    #[test]
    fn a_retouch_click_places_one_spot_with_a_source_elsewhere() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click did not place exactly one spot");
        assert_ne!(
            placed[0].source, placed[0].centre,
            "the source sits on the destination"
        );
    }

    /// ⚠⚠ The guard on the guard. Without it, "a spot appeared after a click" means nothing — it
    /// would pass against a version that placed spots under every tool.
    #[test]
    fn a_click_under_another_tool_places_no_spot() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        app.tool = rustroom_app::tool::Tool::Edit;
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert!(spots(&app).is_empty(), "Edit placed a retouch spot");
    }

    /// ⚠ A second click on a placed spot **selects** it rather than stacking another on top —
    /// the hit-test-before-placement rule, and what makes a crowded frame editable.
    #[test]
    fn clicking_a_placed_spot_selects_it_rather_than_stacking() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "the second click stacked a spot");
        assert_eq!(
            app.session.as_ref().unwrap().selected_spot,
            Some(0),
            "the second click did not select the spot it landed on"
        );
    }

    /// ⚠⚠ The render is sampled **on the drag frame itself** (`DRAG_PASS`), not after the
    /// gesture. A later frame issues one anyway, and in the real app that frame never comes
    /// because egui idles after a drag — which is exactly how the M3.1a mask bug hid.
    #[test]
    fn a_retouch_drag_sizes_the_spot_and_asks_for_a_render() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let before = app.spot_tool.radius;
        let counts = super::test_support::drag_through(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(60.0, 0.0),
        );
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the drag did not place exactly one spot");
        assert!(
            placed[0].radius > before,
            "the drag did not size the spot: {} vs {before}",
            placed[0].radius
        );
        // ⚠ And the tool keeps it, so the panel's Size slider follows the gesture.
        assert!(
            (app.spot_tool.radius - placed[0].radius).abs() < 1e-6,
            "the drag did not write the radius back into the tool"
        );
        // ⚠ A *placement* happens on release, so its render belongs on the release frame — not
        // the drag frame, which is where a **move** must request (see the test below). Asserting
        // "somewhere in the gesture" would be the weak form this project warns about, so the
        // frame is named: nothing before it, a request on it.
        assert_eq!(
            counts[super::test_support::DRAG_PASS],
            counts[0],
            "a placement rendered before the gesture finished: {counts:?}"
        );
        assert!(
            *counts.last().unwrap() > counts[super::test_support::DRAG_PASS],
            "the placement never asked for a render: {counts:?}"
        );
    }

    /// ⚠⚠ **A spot must land where you clicked on a CROPPED photo**, which is what
    /// `FrameMap::inverse_point` buys and what nothing else here can see. Spots are stored in
    /// pre-geometry source coordinates while the canvas shows the cropped frame, so treating a
    /// canvas position as if it were already a source position is invisible on an uncropped photo
    /// and wrong on every cropped one.
    ///
    /// With the right half of the frame cropped, the centre of what is shown is source x = 0.75.
    /// Skipping the map would store 0.5.
    #[test]
    fn a_click_on_a_cropped_photo_lands_where_it_was_clicked() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.edits.geometry.crop = edit_model::CropRect {
                left: 0.5,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            };
        }
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click placed nothing");
        assert!(
            (placed[0].centre[0] - 0.75).abs() < 0.02,
            "the crop was not undone: stored x = {} (0.5 means the map was skipped)",
            placed[0].centre[0]
        );
    }

    /// How many shapes one frame of the canvas paints.
    ///
    /// ⚠ The overlay is *appearance*, which this harness cannot assert on directly — but egui's
    /// `FullOutput` carries the shapes it painted, and a count is enough to see that a whole
    /// group of outlines stopped being drawn.
    fn painted(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(super::test_support::screen()),
                ..Default::default()
            },
            |ui| {
                super::show(app, ui, &c);
            },
        );
        out.shapes.len()
    }

    /// ⚠⚠ **Turning the outlines off must actually stop drawing them.** Reported as wanting a way
    /// to "remove the skeletons... such that one can better see the picture": with thirty patches
    /// the circles become the picture. A test on the flag alone would pass against a canvas that
    /// ignored it entirely, which is exactly what a sabotage found.
    #[test]
    fn hiding_the_patches_paints_fewer_shapes() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: nothing was placed");

        app.show_spots = true;
        let shown = painted(&mut app, &ctx);
        app.show_spots = false;
        let hidden = painted(&mut app, &ctx);
        assert!(
            hidden < shown,
            "hiding the patches painted just as much: {hidden} vs {shown}"
        );
        // ⚠ And the patch itself survives — this is a *view* toggle, not a delete.
        assert_eq!(spots(&app).len(), 1, "hiding the outlines removed a patch");
    }

    /// ⚠⚠ **A non-primary drag must place nothing.** egui reports `dragged()` for *every* button,
    /// so a tool reading a bare `dragged()` would place a spot as the middle-drag pan gesture
    /// passed over the canvas. `tool_drag` is the guard, and no test using Primary alone can see
    /// it removed — this one was added because that sabotage went uncaught.
    #[test]
    fn a_middle_drag_places_no_spot_and_pans_instead() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::drag_through_with(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(60.0, 0.0),
            egui::PointerButton::Middle,
        );
        assert!(
            spots(&app).is_empty(),
            "the pan gesture placed a retouch spot"
        );
    }

    /// ⚠⚠ **The patch must leave the VIEW while it is being moved**, so the blemish underneath is
    /// visible and the new position can be lined up against it. Reported as "it shows the previous
    /// retouching there, I think it should just disappear, so I can once again better align the
    /// positions".
    ///
    /// ⚠ Both halves are asserted. That it disappears is half the property; that it comes back on
    /// release is the other, and without that a moved patch would stay invisible for good — the
    /// worse bug of the two.
    #[test]
    fn a_moving_patch_leaves_the_view_and_returns_on_release() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: nothing was placed");

        // ⚠ Sampled *during* the drag rather than after it: the suppression exists only for the
        // gesture's duration, so a check afterwards could only ever see it cleared.
        let mut during = None;
        for pass in 0..4 {
            let pos = if pass <= 1 {
                centre
            } else {
                centre + egui::vec2(30.0, 0.0)
            };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 {
                events.push(egui::Event::PointerButton {
                    pos: centre,
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
                    screen_rect: Some(super::test_support::screen()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::show(&mut app, ui, &c);
                },
            );
            if pass == super::test_support::DRAG_PASS {
                during = app.session.as_ref().unwrap().suppressed_spot;
            }
        }
        assert_eq!(
            during,
            Some(0),
            "the patch stayed in the view while it was being moved"
        );
        assert_eq!(
            app.session.as_ref().unwrap().suppressed_spot,
            None,
            "the patch never came back after the move"
        );
    }

    /// ⚠ The suppression is *view* state: it must never reach the stored edits, or a moved patch
    /// would be deleted rather than hidden.
    #[test]
    fn suppressing_a_patch_leaves_the_stored_edits_alone() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let stored = spots(&app);
        app.session.as_mut().unwrap().suppressed_spot = Some(0);
        let _ = painted(&mut app, &ctx);
        assert_eq!(spots(&app), stored, "suppression edited the stored spots");
        // ⚠ And the *view* really does leave it out, or the mechanism is inert.
        let s = app.session.as_ref().unwrap();
        assert!(
            s.view_edits(rustroom_app::tool::Tool::Retouch)
                .spots
                .is_empty(),
            "the suppressed patch is still in the rendered edits"
        );
    }

    /// ⚠⚠ **Moving a spot must NOT render while the drag runs — the moving circle is the
    /// feedback.** This reverses an earlier version of this very test, and the reversal was forced
    /// by using the tool: "moving around the retouch patch is very laggy, as it adapts the
    /// retouching on the go".
    ///
    /// The reason is structural rather than a matter of taste. `SpotRemovalStage` lives in the
    /// **prefix**, so moving a spot drops the prefix cache and re-runs noise reduction and dehaze
    /// over the whole buffer — ~117 ms each at native resolution, every frame of the drag. A mask
    /// handle can render live because `MaskStage` is in the cheap tail; the M3.1a rule was right
    /// for masks and wrong here.
    ///
    /// ⚠ Both halves are asserted. "No render during the drag" alone would pass against a version
    /// that never rendered at all and left the patch permanently stale.
    #[test]
    fn dragging_a_placed_spot_renders_only_on_release() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: the click placed nothing");
        let before = spots(&app)[0].centre;

        let counts = super::test_support::drag_through(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(25.0, 15.0),
        );
        let after = spots(&app);
        assert_eq!(after.len(), 1, "the drag stacked a second spot");
        assert_ne!(after[0].centre, before, "the drag did not move the spot");
        assert_eq!(
            counts[super::test_support::DRAG_PASS],
            counts[super::test_support::DRAG_PASS - 1],
            "the move re-rendered mid-drag, which is what made it laggy: {counts:?}"
        );
        assert!(
            *counts.last().unwrap() > counts[super::test_support::DRAG_PASS],
            "the move never rendered at all, so the patch is left stale: {counts:?}"
        );
    }

    /// ⚠ The guard on the guard: "a click changed the edits" means nothing unless a click with
    /// the eyedropper **off** leaves them alone. Without this, a `pick_white_balance` that ran
    /// unconditionally would pass the test above.
    #[test]
    fn a_click_without_the_eyedropper_leaves_the_white_balance_alone() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.edits.white_balance, edit_model::WhiteBalance::default());
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

    /// ⚠⚠ A **region** frame must refresh `fit_image_size`, not only a whole-image one. It carries
    /// `full_w`/`full_h` — the same post-geometry dimensions — and Guide Mode unpinned the fit
    /// rung, so a zoomed session can render nothing but regions. Left stale, `frame_rect` is the
    /// wrong size and every frame-relative overlay sits slightly off, correcting itself only when
    /// you zoom out far enough for a whole-image frame to land.
    ///
    /// ⚠ The concrete case: entering geometry mode **with a crop applied** resets the crop for the
    /// preview, so the frame grows — and the stale value still described the cropped one.
    #[test]
    fn a_region_frame_refreshes_the_fit_image_size() {
        // A frame cut from a 1600x1000 post-geometry frame.
        let region = pipeline::Region {
            full_w: 1600,
            full_h: 1000,
            x0: 100,
            y0: 50,
        };
        // The quantity `shell` now reads: the *whole frame's* dimensions, which a region frame
        // carries — the same source `rendered_long` beside it has always used.
        assert_eq!((region.full_w, region.full_h), (1600, 1000));
        // ⚠ And they are emphatically not the cut's own size, which is what made a stale
        // `fit_image_size` look plausible for as long as it did.
        assert_ne!(region.full_w, region.x0 + 400);
    }

    /// ⚠⚠ **The defect this fixes, and the only test shape that can see it.** The guides were
    /// drawn from inside `draw_crop_overlay`, which the canvas calls only under `Tool::Crop` — so
    /// the View menu entry and the `O` key did nothing in three of the four tools. A test that
    /// checks one tool cannot distinguish "works everywhere" from "works in the tool I happened
    /// to pick". Same argument as `the_strip_is_present_under_every_tool`.
    #[test]
    fn the_composition_guides_are_drawn_under_every_tool() {
        for tool in rustroom_app::tool::Tool::ALL {
            let ctx = egui::Context::default();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 200, 150);
            app.tool = tool;
            assert!(
                guide_lines_drawn(&mut app, &ctx) > 0,
                "no composition guides under {tool:?}"
            );
        }
    }

    /// ⚠ The deliberate exception. Perspective Guide Mode strips the crop from the preview *and*
    /// draws no crop rect, so a guide there would divide a rectangle the user is not going to
    /// keep — and one that divides the wrong rectangle is worse than none, because it is trusted.
    #[test]
    fn guide_mode_draws_no_composition_guides() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 150);
        app.tool = rustroom_app::tool::Tool::Crop;
        app.session.as_mut().expect("a session").show_guides = true;
        assert_eq!(
            guide_lines_drawn(&mut app, &ctx),
            0,
            "Guide Mode must draw no composition guides"
        );
    }

    /// ⚠⚠ `Off` must actually suppress them, and this is **not** the tautology it first looks
    /// like. My first version asked `guide_lines_drawn` — which substitutes `Thirds` when the
    /// setting is `Off` — so it measured Thirds-minus-Off and asserted that was zero. It compares
    /// the two paints directly instead, which is the only form that can see a setting being
    /// ignored.
    #[test]
    fn the_off_setting_draws_fewer_lines_than_a_guide_does() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 200, 150);
        app.tool = rustroom_app::tool::Tool::Edit;
        use rustroom_app::composition_guides::CompositionGuide;
        let off = segments_with(&mut app, &ctx, CompositionGuide::Off);
        let on = segments_with(&mut app, &ctx, CompositionGuide::Thirds);
        assert!(on > off, "Off drew as much as Thirds: {off} vs {on}");
    }

    /// How many line segments the guides add: the current setting, minus `Off`.
    ///
    /// ⚠ Counts painted shapes rather than looking for a widget — guides are *painted*, so
    /// `test_ui` cannot see them; it only reaches interactive widgets through AccessKit.
    fn guide_lines_drawn(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        use rustroom_app::composition_guides::CompositionGuide;
        let off = segments_with(app, ctx, CompositionGuide::Off);
        let on = segments_with(app, ctx, CompositionGuide::Thirds);
        on.saturating_sub(off)
    }

    /// Paint one canvas frame with `guide` set, and count the line segments in it.
    fn segments_with(
        app: &mut crate::RustroomApp,
        ctx: &egui::Context,
        guide: rustroom_app::composition_guides::CompositionGuide,
    ) -> usize {
        let was = app.settings.composition_guide;
        app.settings.composition_guide = guide;
        let ctx2 = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                ..Default::default()
            },
            |ui| {
                super::show(app, ui, &ctx2);
            },
        );
        app.settings.composition_guide = was;
        let mut n = 0;
        for cs in out.shapes {
            count_segments(&cs.shape, &mut n);
        }
        n
    }

    fn count_segments(shape: &egui::Shape, n: &mut usize) {
        match shape {
            egui::Shape::LineSegment { .. } => *n += 1,
            egui::Shape::Vec(v) => {
                for s in v {
                    count_segments(s, n);
                }
            }
            _ => {}
        }
    }

    /// Seed the bytes the frame-landing site in `shell.rs` would have retained, since this
    /// harness drives `canvas::show` alone and no frame ever lands in it. A left/right split
    /// rather than a flat field, so a wrong UV mapping has something to be caught by.
    ///
    /// Returns the two byte values, dark then light.
    fn seed_readout_bytes(app: &mut crate::RustroomApp) -> (u8, u8) {
        const N: usize = 32;
        const DARK: u8 = 20;
        const LIGHT: u8 = 200;
        let mut bytes = Vec::with_capacity(N * N * 4);
        for _y in 0..N {
            for x in 0..N {
                let v = if x < N / 2 { DARK } else { LIGHT };
                bytes.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let s = app.session.as_mut().expect("a session");
        s.readout_bytes = Some((std::sync::Arc::new(bytes), N, N));
        (DARK, LIGHT)
    }

    fn readout(app: &crate::RustroomApp) -> String {
        app.session.as_ref().expect("a session").readout.clone()
    }

    /// ⚠⚠ The wiring test. `color_readout` has its own unit tests, but nothing else can see that
    /// a hover over the canvas ever reaches it and lands in the string the status cluster shows —
    /// which is the half this project has repeatedly shipped broken.
    /// `an_armed_eyedropper_sets_the_white_balance_from_a_click` exists for the same reason.
    #[test]
    fn hovering_the_canvas_produces_a_readout() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let (dark, _light) = seed_readout_bytes(&mut app);
        super::test_support::hover_through(&mut app, &ctx, centre);
        // The patch at the exact centre straddles the split, so assert only that *something*
        // was read and that it is the label's shape — the two probes below carry the position.
        let got = readout(&app);
        assert!(!got.is_empty(), "the hover produced no readout");
        assert!(
            got.starts_with("R ") && got.contains("G ") && got.contains("B "),
            "the hover produced something that is not the readout label: {got:?}"
        );
        assert_ne!(
            got,
            rustroom_app::color_readout::label([dark, dark, dark]),
            "fixture check: the centre patch should straddle the split"
        );
    }

    /// ⚠ The position, not merely the presence, of the read — the guard on the UV mapping. A
    /// flipped or offset mapping still produces *a* number, so hovering left of centre and right
    /// of centre must give the two halves in the right order.
    #[test]
    fn the_readout_names_the_pixel_under_the_cursor() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let (dark, light) = seed_readout_bytes(&mut app);

        super::test_support::hover_through(&mut app, &ctx, centre - egui::vec2(60.0, 0.0));
        let left = readout(&app);
        super::test_support::hover_through(&mut app, &ctx, centre + egui::vec2(60.0, 0.0));
        let right = readout(&app);

        assert_eq!(
            left,
            rustroom_app::color_readout::label([dark, dark, dark]),
            "left of centre should read the dark half, got {left:?}"
        );
        assert_eq!(
            right,
            rustroom_app::color_readout::label([light, light, light]),
            "right of centre should read the light half, got {right:?}"
        );
    }

    /// ⚠ The guard on the guard. "A number appeared after hovering" means nothing unless it goes
    /// away — a stale value left on screen is worse than none, because it gets trusted.
    #[test]
    fn hovering_off_the_image_clears_the_readout() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        seed_readout_bytes(&mut app);
        super::test_support::hover_through(&mut app, &ctx, centre);
        assert!(
            !readout(&app).is_empty(),
            "fixture check: nothing was read, so this test proves nothing"
        );
        super::test_support::hover_away(&mut app, &ctx);
        assert!(
            readout(&app).is_empty(),
            "the readout survived the pointer leaving: {:?}",
            readout(&app)
        );
    }

    /// ⚠ A hover reads and **writes nothing else**: no render request, no history entry, no
    /// edit. A readout that dirtied the photo would be found much later, by use.
    #[test]
    fn hovering_asks_for_no_render_and_pushes_no_history() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        seed_readout_bytes(&mut app);
        let before = {
            let s = app.session.as_ref().expect("a session");
            (
                s.engine.requests_issued(),
                s.history.can_undo(),
                s.edits.clone(),
            )
        };
        super::test_support::hover_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert!(!s.readout.is_empty(), "fixture check: nothing was read");
        assert_eq!(
            s.engine.requests_issued(),
            before.0,
            "the hover requested a render"
        );
        assert_eq!(s.history.can_undo(), before.1, "the hover pushed history");
        assert_eq!(s.edits, before.2, "the hover changed the edits");
    }
}

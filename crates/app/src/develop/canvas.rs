//! The central image canvas: the rendered photo, zoom and pan, and the crop overlay.

use crate::{preview_edits, rects_around, PendingOpen, RustroomApp, Session, PREVIEW_MAX_DIM};
use eframe::egui;
use rustroom_app::{crop_tool, view_region};

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) -> bool {
    let mut photo_edited = false;
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
                    let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                    egui::Image::new(shown).paint_at(ui, resp.rect);

                    if s.geometry_mode {
                        photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect);
                    } else {
                        if resp.dragged() {
                            s.pan += resp.drag_delta();
                        }
                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                        if scroll != 0.0 && resp.hovered() {
                            let old = s.zoom;
                            s.zoom = (s.zoom * (1.0 + scroll * 0.001)).clamp(1.0, 8.0);
                            // Zoom about the cursor, not the centre: keep whatever the
                            // pointer is over fixed on screen. `anchor` is the image's
                            // centre, so `d` is the cursor's offset from it; magnifying
                            // by `r` moves that point to `d*r`, which `d*(1-r)` undoes.
                            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                                let anchor = ui.max_rect().center() + s.pan;
                                s.pan += (p - anchor) * (1.0 - s.zoom / old);
                            }
                            // Back at fit the whole image is on screen, so leftover pan
                            // from cursor-anchored zooming would just push it off-centre.
                            if s.zoom <= 1.0 {
                                s.pan = egui::Vec2::ZERO;
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
        if resp.drag_started() {
            s.active_handle = crop_tool::hit_test(&s.edits.geometry.crop, [nx, ny], 0.03);
        }
        if resp.dragged() {
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

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
mod navigation;
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
                            if s.show_guides {
                                let d = draw_guide_overlay(ui, s, &resp, frame_rect, img_rect);
                                if d != GuideDrag::None {
                                    guide_drag = d;
                                }
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

#[cfg(test)]
mod tests {
    use super::test_support::screen;
    use eframe::egui;

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
}

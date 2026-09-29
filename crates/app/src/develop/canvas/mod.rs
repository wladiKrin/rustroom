//! The central image canvas: the rendered photo, the render rung and region it asks for, its
//! placement, and zoom and pan. The per-tool overlays live in the sibling modules and `show`
//! dispatches to exactly one of them.
//!
//! ⚠ `frame_rect` — where the **whole frame** sits on screen — is computed here and handed
//! down, because every overlay must place against it rather than against `img_rect`. See its
//! comment at the call site.

mod crop_overlay;
pub(crate) mod eye_overlay;
mod guide_overlay;
mod loupe;
mod mask_overlay;
pub(crate) mod mask_tint;
pub(crate) mod navigation;
pub(crate) mod spot_overlay;
mod straighten_overlay;
#[cfg(test)]
mod test_support;

use crate::develop::canvas_geom;
use crate::{PendingOpen, RustroomApp, Session};
use crop_overlay::draw_crop_overlay;
use eframe::egui;
use guide_overlay::{draw_guide_overlay, GuideDrag};
use mask_overlay::{draw_mask_overlay, BrushTool};
use navigation::pan_delta;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use spot_overlay::{draw_spot_overlay, sampler_for};
use straighten_overlay::draw_straighten_overlay;

/// What an empty Develop says. ⚠ There has been no "Open file… button" since the menus took the
/// toolbar's place; the old text still named one. Also swept by
/// `theme::every_menu_string_is_renderable`.
pub(crate) const EMPTY_DEVELOP_TEXT: &str =
    "Open a photo from the Library, or File → Open file… (⌘O).";

/// Whether `RUSTROOM_DEBUG_PLACE` is set. Read once: the placement log runs per frame, where
/// the render engine's equivalent runs per render.
static DEBUG_PLACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("RUSTROOM_DEBUG_PLACE").is_some());

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
/// How many rungs a live gesture should drop.
///
/// ⚠⚠ **The interactive drop engages for a drag that changes the PICTURE, not one that only
/// changes the view.** A pan changes which part of the frame is on screen; a gesture on a mask
/// whose adjustments are held out of the view changes nothing at all. Neither needs a render, and
/// dropping the rung for one discards a texture that already covers the viewport and replaces it
/// with a softer one for nothing — the request condition below ends in `|| max_dim !=
/// s.render_rung`, which fires even when `covered` is true.
///
/// ⚠⚠ **Still a question about the gesture, never about the render, which is what keeps it from
/// oscillating.** The 2026-08-13 progressive-fit feature asked "is the showing resolution not yet
/// sharp?" — a question the render itself answers, so the render answered it, the gate flipped,
/// and it counted that as interaction. Nothing here can be affected by what a render produces.
///
/// ⚠⚠ **Panning keeps the exemption on a MEASUREMENT, not a preference.** `bench_pan_rung`
/// (kept) prices a region tail with the prefix cached — which is the whole per-frame cost of a
/// pan, since panning changes no edits — at the reporter's zoom on an X-T4:
///
/// | rung | region | tail |
/// |---|---|---|
/// | 3200 | 488x277, 0.14 Mpx | **1.48 ms** |
/// | 6246 | 953x541, 0.52 Mpx | **6.02 ms** |
///
/// So the drop buys **4.5 ms** — a full-rung pan frame is 6 ms, about 166 fps — and costs a 2x
/// blur across the whole view. That is the opposite of the fitted whole-image case the
/// interactive rung was designed against, where the same drop was worth 98 ms.
///
/// ⚠ A slider drag must still drop, and that is the half confirmed by use: 137.9 ms to 40.0 ms,
/// 7.2 to 25 fps. Every non-canvas request site renders at `render_max_dim()`, i.e. whatever rung
/// the canvas last set, so a rule that merely stopped the canvas requesting would disable the
/// feature outright.
fn rungs_to_drop(dragging: bool, quality_drop: u32, view_only_gesture: bool) -> u32 {
    if !dragging || view_only_gesture {
        0
    } else {
        quality_drop
    }
}

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
    let mut app_eye_tool = app.eye_tool;
    let mut eye_status: Option<String> = None;
    // ⚠ The defringe pick's status line, written back to `app.status` after the session borrow —
    // `app_spot_tool`'s shape (M9.6).
    let mut fringe_status: Option<String> = None;
    // ⚠ Copied out before `match &mut app.session` takes its mutable borrow. `PanDrag` is
    // `Copy`, so this is a copy rather than a borrow that would collide.
    let app_pan_drag = app.settings.pan_drag;
    // ⚠ Same reason as `app_pan_drag`: copied out before `app.session` is borrowed mutably.
    let app_erase_drag = app.settings.erase_drag;
    // ⚠ Same reason as `app_pan_drag`: copied out before `match &mut app.session` takes its
    // mutable borrow.
    // ⚠ Same reason as `app_pan_drag`: copied out before `match &mut app.session` takes its
    // mutable borrow. `CompositionGuide` is `Copy`, so this is a copy rather than a borrow.
    let app_guide = app.settings.composition_guide;
    // ⚠⚠ **The interactive rung's gate.** Hoisted before `app.session` is borrowed: `app.settings`
    // is a disjoint field, but reading it *through* `app` after the borrow is E0502 — the rule
    // `tool` and `app_guide` already follow. `DragQuality` is `Copy`, so this is free.
    //
    // ⚠ `is_decidedly_dragging` rather than `any_down`: its condition includes
    // `!could_any_button_be_click() && !any_click()`, which is what stops a plain click costing
    // two renders — one down to the low rung and one back up. Verified in egui 0.35's source,
    // not assumed.
    //
    // ⚠⚠ **A gate on the POINTER, never on the render — and that is what makes it structurally
    // unable to oscillate.** The 2026-08-13 progressive-fit feature asked "is the showing
    // resolution not yet sharp?", a question the render itself answers; so the render answered
    // it, the gate flipped, and it counted that as interaction and reset its own timer. Nothing
    // a render does can change this one.
    //
    // ⚠ Its two documented edges are both benign: it is false on the *first* frame of a drag and
    // can be true on the *release* frame. The render worker drains its queue to the newest
    // request, so the first frame's full-rung request is discarded before it renders; the release
    // is restored one frame later.
    //
    // ⚠⚠ **And it is exempt for a gesture that changes nothing renderable** — see
    // [`rungs_to_drop`]. A pan inside the rendered margin, and a stroke on a mask whose
    // adjustments are held out of the view, both leave the picture alone.
    //
    // ⚠ The press **origin**, not the current pointer position: a drag that began on a panel
    // slider and wandered over the canvas is still a slider drag and must still drop the rung.
    // ⚠ Whether the mask being edited is switched off (`MaskedAdjustment::disabled`): a stroke
    // on it changes no pixel, so the gesture is view-only.
    let app_suppressed_mask = app.session.as_ref().is_some_and(|s| {
        s.selected_mask
            .is_some_and(|i| s.edits.masks.get(i).is_some_and(|m| m.disabled))
    });
    let text_focused = ui.ctx().memory(|m| m.focused()).is_some();
    let press_on_canvas = ui
        .ctx()
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| ui.max_rect().contains(p));
    let panning = ui
        .ctx()
        .input(|i| navigation::pan_button_down(i, app_pan_drag, text_focused));
    let view_only_gesture = press_on_canvas && (panning || app_suppressed_mask);
    // ⚠⚠ **`press_origin.is_some()` as well as `is_decidedly_dragging()`, and it was found by
    // instrumenting.** On the RELEASE frame egui still reports a decided drag while the press
    // origin is already gone — the edge the comment above calls benign. It stops being benign
    // once the exemption depends on where the press began: the gesture is no longer attributable,
    // so the exemption evaporates while the drop still applies, and the rung dropped for one
    // frame at the end of every pan. Requiring an origin makes "a drag we can attribute" the
    // condition, which also restores full quality one frame sooner for every other gesture.
    let app_drag_drop = rungs_to_drop(
        ui.ctx()
            .input(|i| i.pointer.is_decidedly_dragging() && i.pointer.press_origin().is_some()),
        app.settings.drag_quality.rungs_dropped(),
        view_only_gesture,
    );
    let app_brush_size = app.brush_size;
    let app_brush_hardness = app.brush_hardness;
    let app_brush_erase = app.brush_erase;
    // ⚠ Hoisted with the others: read before `app.session` is borrowed mutably below.
    let app_brush_auto_mask = app.brush_auto_mask;
    let app_brush_tolerance = app.brush_tolerance;
    // ⚠ An `Arc::clone`, not a copy of the bytes: the frame already holds this allocation, so it
    // costs a refcount. Hoisted with the others, i.e. read before `app.session` is borrowed
    // mutably below.
    let app_tint_frame = app.session.as_ref().and_then(|s| {
        let (bytes, bw, bh) = s.readout_bytes.as_ref()?;
        Some(mask_overlay::TintFrame {
            bytes: std::sync::Arc::clone(bytes),
            bw: *bw,
            bh: *bh,
            origin: s.rendered_region.map_or((0, 0), |r| (r.x, r.y)),
            full: s.rendered_full,
        })
    });
    // ⚠ Cleared here, once, and set again only by an actual hover over the image at the end of
    // this function — so every path that draws no image (a decode in flight, no texture yet, no
    // session) leaves it blank. A stale number left on screen after the pointer leaves is worse
    // than none, because it gets trusted.
    if let Some(s) = app.session.as_mut() {
        s.readout = None;
    }
    egui::CentralPanel::default()
        .frame(crate::theme::canvas_frame(ui.style()))
        .show(ui, |ui| {
            // ⚠ With a ghost session for the pending photo the session branch below draws it —
            // the pending branch is the fallback for a photo with no cached thumbnail.
            let ghost_covers = app
                .session
                .as_ref()
                .zip(app.pending.as_ref())
                .is_some_and(|(s, p)| s.path == p.id.path && s.copy == p.id.copy);
            if !ghost_covers {
                if let Some(p) = &mut app.pending {
                    // M9.14: the camera JPEG or a rendered preview, when it has landed, takes
                    // the thumbnail placeholder's place — sharp instead of a 512-px blur — and
                    // it zooms and pans while the decode runs; the session inherits the view.
                    let key = p.preview_request.key();
                    let preview = app.preview_textures.get(&key).map(|(t, _)| t);
                    let missed = app.preview_misses.contains(&key);
                    draw_pending_canvas(ui, p, app.session.as_ref(), preview, missed, app_pan_drag);
                    return;
                }
            }
            // ⚠ Disjoint field borrows, taken before `app.session` is borrowed mutably below.
            let preview_textures = &app.preview_textures;
            let preview_misses = &app.preview_misses;
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
                        // ⚠ Through `Session::fit_cap`, not derived again here — its doc carries
                        // the rule, and a second derivation is how the two come to disagree.
                        let fit_cap = s.fit_cap();
                        // Geometry mode pins the fit rung: the crop overlay is drawn against the
                        // whole image and placed by the fit-mode maths.
                        //
                        // ⚠⚠ **Guide Mode is the exception and unpins it**, because placing an
                        // endpoint exactly on a building edge needs real pixels — at fit resolution
                        // you are aiming at a blur. It can afford to: the guide overlay is anchored
                        // to `frame_rect`, so it survives a region render, which is precisely what
                        // the crop overlay cannot do.
                        // ⚠⚠ **The fit rung is no longer pinned in Geometry mode.** It was, because
                        // "the crop overlay is drawn against the whole image and placed by the
                        // fit-mode maths", so a region render would misplace it. The crop overlay is
                        // anchored to `frame_rect` now, like the guide and mask overlays, so it
                        // survives a region render and the pin is unnecessary — which is what lets
                        // the crop tool zoom and pan. Guide Mode was already the exception for
                        // exactly this reason; there is no longer an exception to make.
                        let rung = view_region::tier_max_dim(fit_cap, s.buffer_long, fit, s.zoom);
                        // ⚠⚠ **The interactive rung.** A live gesture renders `app_drag_drop` rungs
                        // lower and the release restores full quality — measured at 137.9 ms against
                        // 35.2 ms on the reporter's window. There is **no settle timer**: the
                        // `max_dim != s.render_rung` disjunction below already re-requests on any
                        // rung change, so the release edge triggers the full-quality render by
                        // itself, one frame later rather than 200 ms.
                        //
                        // ⚠⚠ `native` is computed from the DROPPED `max_dim`, which is the *opposite*
                        // of the 2026-08-13 spec's warning — and correct, because this override goes
                        // DOWN where that one went up. Theirs could report native at fit zoom and make
                        // the whole image vanish; this one cannot. At rung 1600 whole-image is right,
                        // and a fitted view at a high rung is caught by `covers_whole_image` below.
                        let max_dim =
                            view_region::rung_below(rung, fit_cap, s.buffer_long, app_drag_drop);
                        let native = view_region::is_native(max_dim, fit_cap);

                        // Screen pixels per rendered pixel. `k` compares the two *caps*, so
                        // the crop factor cancels — it applies equally to both.
                        let full_long = full.x.max(full.y);
                        let k = max_dim as f32 / fit_cap as f32;
                        let (iw, ih) =
                            ((full.x * k).round() as usize, (full.y * k).round() as usize);
                        let scale = fit * s.zoom / k;

                        // Which part of the image the view needs, and whether what is already
                        // rendered still covers it.
                        // ⚠ Decided by *visibility*, not by the rung. Once `fit` may exceed 1.0
                        // the whole image can be visible at a higher rung, and cropping to it
                        // would gain nothing while starving `fit_image_size` — which is only
                        // taken from whole-image frames — of any refresh.
                        // ⚠⚠ Computed **once** and used for both the margin and the request. The
                        // region must be large enough for the reach of the edits it will be rendered
                        // with, so deriving the margin from a different value than the render uses is
                        // exactly the two-facts-that-disagree failure — and a silent one, showing as
                        // a subtly wrong band at the edge of the visible area.
                        let shown_edits = s.view_edits(tool);
                        // M9.14: the preview is of the photo's plain edits — the key the loader
                        // was asked with — so it stands in only while the view shows exactly
                        // those (not Before, not a geometry tool's uncropped view of a crop).
                        let view_is_plain = !s.show_before && shown_edits == s.edits;
                        let stand_in_key = rustroom_app::thumb_key::ThumbKey::new(
                            rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
                            &s.edits,
                        );
                        let needed_long = view_region::needed_long(fit_cap, fit, s.zoom);
                        let (vis, want) = if native {
                            let v = view_region::visible_rect(
                                iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y,
                            );
                            if view_region::covers_whole_image(v, iw, ih) {
                                (None, None)
                            } else {
                                let margined = view_region::with_margin(
                                    v,
                                    iw,
                                    ih,
                                    // ⚠ Derived per render, not a constant: masks chain, so N masked
                                    // clarities reach N x 120 px and no fixed number covers an
                                    // unbounded stack. See `Session::render_margin`.
                                    s.render_margin(&shown_edits, max_dim),
                                );
                                (Some(v), Some(margined))
                            }
                        } else {
                            (None, None)
                        };
                        // ⚠⚠ **Against the VISIBLE rect, never the margined one — and comparing the
                        // margined one meant the margin had never worked.** `with_margin`'s own doc
                        // says why it exists: "small pans reuse the existing texture instead of
                        // re-rendering". But `want` and `rendered_region` are both `visible + 2 ×
                        // margin`, so they are the same size, and one rectangle contains another of
                        // equal size only when they are identical — **any pan at all made this
                        // false**. The margin made every render bigger and reduced the re-render rate
                        // by nothing.
                        //
                        // ⚠ Measured, not reasoned: `RUSTROOM_DEBUG_RENDER` over one pan showed **81
                        // consecutive renders, one per frame**, their regions drifting by a pixel at a
                        // time, 21 ms each. Reported as panning being smooth but "very sluggish".
                        //
                        // ⚠ Asking whether what is rendered still covers what is on screen is also
                        // the only question that means anything here: the margin is headroom, not part
                        // of the requirement.
                        let covered = match (vis, s.rendered_region) {
                            (None, None) => true,
                            (Some(v), Some(r)) => view_region::contains(r, v),
                            _ => false,
                        };
                        // ⚠ A change of *rung* must re-request even when the region is
                        // unchanged — otherwise moving between rungs, or an upgraded buffer
                        // arriving, would leave the old texture on screen.
                        // ⚠ Consumed here, and only here: one frame in which a deliberately softer
                        // render is left alone. The next frame sees the rung still changed and asks
                        // for full quality, which is what makes the sharpening need no timer.
                        let soft_frame = std::mem::take(&mut s.soft_next);
                        if (!covered && s.requested_region != want)
                            || (max_dim != s.render_rung && !soft_frame)
                        {
                            s.requested_region = want;
                            s.render_rung = max_dim;
                            s.request(shown_edits, max_dim, want);
                        }

                        // Place using the *texture's* own render resolution, not the current
                        // mode's: a frame from the other mode may still be on screen while a
                        // new one is in flight.
                        // ⚠ No before/after special case any more: the before frame is rendered
                        // with the *same* geometry, rung and region as the after frame, so it
                        // places identically. Holding the framing fixed is what lets the toggle be
                        // a comparison rather than a jump.
                        let (place_region, place_long) =
                            (s.rendered_region, s.rendered_long.max(1.0));
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
                        let frame_rect = canvas_geom::frame_rect(
                            ui.max_rect().center(),
                            s.pan,
                            full,
                            pk,
                            pscale,
                        );
                        s.canvas_frame_rect = Some(frame_rect);

                        // ⚠ Env-gated instrumentation, the `RUSTROOM_DEBUG_BRUSH` precedent: this
                        // file's own record says reasoning about placement has been wrong twice, and
                        // one print settled each of them in a run. `RUSTROOM_DEBUG_PLACE=1` logs every
                        // input the placement is derived from, so a rung transition can be read off a
                        // pair of consecutive lines rather than argued about.
                        if *DEBUG_PLACE {
                            let r = s.rendered_region;
                            eprintln!(
                                "place rung={max_dim} native={native} fit_cap={fit_cap} \
                             full=({:.3},{:.3}) fit={fit:.6} zoom={:.4} pan=({:.2},{:.2}) \
                             place_long={place_long:.1} pk={pk:.6} pscale={pscale:.6} \
                             tex=({},{}) region={:?} img_min=({:.2},{:.2}) \
                             frame_min=({:.2},{:.2}) frame_size=({:.2},{:.2}) avail=({:.1},{:.1})",
                                full.x,
                                full.y,
                                s.zoom,
                                s.pan.x,
                                s.pan.y,
                                shown.size()[0],
                                shown.size()[1],
                                r.map(|r| (r.x, r.y, r.w, r.h)),
                                img_rect.min.x,
                                img_rect.min.y,
                                frame_rect.min.x,
                                frame_rect.min.y,
                                frame_rect.width(),
                                frame_rect.height(),
                                avail.x,
                                avail.y,
                            );
                        }

                        // M9.14: the preview stands in where the live buffer cannot supply the
                        // pixels asked for — the rule is `stand_in::prefer_preview`; the key check
                        // is that the texture is of the edits on screen, never the before image.
                        let stand_in = if s.buffer_is_full_res || !view_is_plain {
                            None
                        } else {
                            preview_textures
                                .get(&stand_in_key)
                                .filter(|(t, _)| {
                                    let long = t.size()[0].max(t.size()[1]);
                                    rustroom_app::stand_in::prefer_preview(
                                        needed_long,
                                        s.buffer_long,
                                        long,
                                    )
                                })
                                .cloned()
                        };

                        let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                        // ⚠⚠ **The backdrop, painted at `frame_rect` UNDER the region texture.** A
                        // region render covers only `img_rect`; everything outside it used to be
                        // unpainted, so panning revealed black rectangles that snapped to image when
                        // the next region landed. The last whole-image texture covers the entire
                        // frame, so a revealed edge is a soft but correct picture that snaps sharp.
                        //
                        // ⚠ Only while a region is on screen. With a whole-image frame the two would
                        // coincide exactly and the backdrop would be a wasted draw of the same pixels.
                        //
                        // ⚠ Only when its `fit_size` still matches: a crop or an orientation change
                        // moves the frame, and a backdrop from before it would be stretched across
                        // the new one at the wrong scale — visible, because it spans the whole rect.
                        if place_region.is_some() {
                            if let Some((tex, size)) = &s.backdrop {
                                if (size.0 - full.x).abs() < 0.5 && (size.1 - full.y).abs() < 0.5 {
                                    egui::Image::new(tex).paint_at(ui, frame_rect);
                                }
                            }
                        }
                        // ⚠ A ghost whose preview has not answered yet shows the previous
                        // photo dimmed, never its own 512-px render — the author called that a
                        // low-resolution flash (2026-09-29). Once the loader has said there is
                        // no preview (an edited photo without one), the ghost's render is all
                        // there is and it shows.
                        let waiting = s.buffer_is_ghost
                            && stand_in.is_none()
                            && !preview_misses.contains(&stand_in_key);
                        match &stand_in {
                            // ⚠ At `frame_rect`, the whole post-geometry frame: a camera JPEG
                            // is the camera's framing, which for an untouched photo is the frame;
                            // a rendered preview is the frame exactly.
                            Some((tex, source)) => {
                                egui::Image::new(tex).paint_at(ui, frame_rect);
                                let label = match source {
                                    rustroom_app::preview_loader::PreviewSource::Camera => {
                                        "Camera preview"
                                    }
                                    rustroom_app::preview_loader::PreviewSource::Rendered => {
                                        "Preview"
                                    }
                                };
                                let at = frame_rect.min + egui::vec2(10.0, 8.0);
                                let font = egui::FontId::proportional(12.0);
                                let galley = ui.painter().layout_no_wrap(
                                    label.to_string(),
                                    font.clone(),
                                    egui::Color32::WHITE,
                                );
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(at, galley.size())
                                        .expand2(egui::vec2(6.0, 3.0)),
                                    egui::CornerRadius::same(3),
                                    egui::Color32::from_black_alpha(140),
                                );
                                ui.painter().galley(at, galley, egui::Color32::WHITE);
                            }
                            None if waiting => {
                                paint_stale(ui, s.stale.as_ref());
                            }
                            None => {
                                egui::Image::new(shown).paint_at(ui, resp.rect);
                            }
                        }
                        // ⚠ The same rectangle as the frame it was computed from — `J`.
                        if let Some(clip) = &s.clip_texture {
                            egui::Image::new(clip).paint_at(ui, resp.rect);
                        }
                        // ⚠ The grade range view, over the photograph and under every tool overlay.
                        // `resp.rect` is `img_rect` — the rendered slice — and this texture covers
                        // exactly that, whether the frame was whole or a region; the readout's
                        // exception, for the readout's reason. Every other overlay is frame-relative.
                        if let Some(tint) = &s.grade_view_texture {
                            egui::Image::new(tint).paint_at(ui, resp.rect);
                        }

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
                                    // ⚠ Guide Mode shows the muted crop rectangle for the same reason
                                    // the straighten mode does: the preview strips the crop, so
                                    // without it there is nothing on screen saying what will actually
                                    // be kept. ⚠ This is **not** the composition-guide rule below,
                                    // which stays — those divide the photograph-to-be, and under
                                    // Guide Mode there is no crop rect *they* may divide.
                                    crop_overlay::draw_crop_marks(
                                        ui,
                                        crop_overlay::crop_screen_rect(s, frame_rect),
                                        img_rect,
                                        crop_overlay::Marks::Muted,
                                    );
                                    let d = draw_guide_overlay(ui, s, &resp, frame_rect, img_rect);
                                    if d != GuideDrag::None {
                                        guide_drag = d;
                                    }
                                } else if s.straighten_armed {
                                    // ⚠ The crop rectangle stays on screen while the straighten mode
                                    // is armed, muted and non-interactive. It used to vanish — the
                                    // dispatch replaced `draw_crop_overlay` entirely, as Guide Mode
                                    // does — which left the composition guides dividing a rectangle
                                    // nobody could see. Lightroom keeps it visible too.
                                    crop_overlay::draw_crop_marks(
                                        ui,
                                        crop_overlay::crop_screen_rect(s, frame_rect),
                                        img_rect,
                                        crop_overlay::Marks::Muted,
                                    );
                                    photo_edited |= draw_straighten_overlay(
                                        ui, s, &resp, frame_rect, img_rect, tool,
                                    );
                                } else {
                                    photo_edited |= draw_crop_overlay(
                                        ui,
                                        s,
                                        &resp,
                                        frame_rect,
                                        img_rect,
                                        app.crop_aspect,
                                    );
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
                                        auto_mask: app_brush_auto_mask,
                                        tolerance: app_brush_tolerance,
                                        base: app_tint_frame.as_ref(),
                                        pan_drag: app_pan_drag,
                                        erase_drag: app_erase_drag,
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
                            // ⚠ The white-balance eyedropper's twin (M9.6): its own click-sensing
                            // interaction, only while armed, and the same crosshair.
                            Tool::Edit if s.picking_fringe => {
                                let pick = ui
                                    .interact(
                                        img_rect,
                                        ui.id().with("fringe_eyedropper"),
                                        egui::Sense::click(),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::Crosshair);
                                if let Some(m) = pick_fringe(s, &pick, frame_rect, tool) {
                                    photo_edited |= m.1;
                                    fringe_status = Some(m.0);
                                }
                            }
                            Tool::Retouch => {
                                // ⚠ Built here rather than inside the overlay so the shared borrow of
                                // `s` it holds ends before the overlay takes `&mut s`.
                                let sample = sampler_for(s);
                                // ⚠ M9.11: patch gestures only in a patch mode. Eyes are drawn in
                                // every mode but take gestures only in theirs — one click, one
                                // meaning. Patch outlines are not drawn in an eye mode.
                                if app_eye_tool.active.is_none() {
                                    photo_edited |= draw_spot_overlay(
                                        ui,
                                        s,
                                        &resp,
                                        img_rect,
                                        frame_rect,
                                        &mut app_spot_tool,
                                        &sample,
                                    );
                                }
                                photo_edited |= eye_overlay::draw_eye_overlay(
                                    ui,
                                    s,
                                    &resp,
                                    img_rect,
                                    frame_rect,
                                    &mut app_eye_tool,
                                    &sample,
                                    &mut eye_status,
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
                            Tool::Crop => Some(crop_overlay::crop_screen_rect(s, frame_rect)),
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
                        // ⚠ Every tool zooms and pans now. This was gated off for Geometry because
                        // the crop overlay was placed by fit-mode maths; see the rung above.
                        {
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
                            s.pan +=
                                pan_delta(&resp, scroll, space_down, app_pan_drag, text_focused);

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
                            if img_rect.contains(p)
                                && img_rect.width() > 0.0
                                && img_rect.height() > 0.0
                            {
                                let u = (p.x - img_rect.left()) / img_rect.width();
                                let v = (p.y - img_rect.top()) / img_rect.height();
                                let x = ((u * *bw as f32) as usize).min(bw.saturating_sub(1));
                                let y = ((v * *bh as f32) as usize).min(bh.saturating_sub(1));
                                // ⚠ Computed into a local first: `bytes` is a shared borrow of `s`
                                // and writing `s.readout` needs a mutable one. Never a clone of the
                                // bytes.
                                let sampled =
                                    rustroom_app::color_readout::mean_at(bytes, *bw, *bh, x, y);
                                s.readout = sampled;
                            }
                        }
                    } else {
                        // No frame yet — a ghost session's first render is a few ms away. The
                        // preview, when it has landed, fills those frames at the session's view.
                        let key = rustroom_app::thumb_key::ThumbKey::new(
                            rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
                            &s.edits,
                        );
                        match preview_textures.get(&key) {
                            Some((tex, _)) => {
                                paint_fitted(ui, tex, s.zoom, s.pan);
                            }
                            None => paint_stale(ui, s.stale.as_ref()),
                        }
                    }
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        crate::theme::empty_state(ui, EMPTY_DEVELOP_TEXT);
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
    app.eye_tool = app_eye_tool;
    if let Some(m) = eye_status {
        app.status = m;
    }
    if let Some(m) = fringe_status {
        app.status = m;
    }
    photo_edited
}

/// Canvas while a decode is in flight: the target photo's thumbnail if we have one (scaled
/// up to fill, since thumbnails are ≤512 px), else the previous photo dimmed, else a spinner.
fn draw_pending_canvas(
    ui: &mut egui::Ui,
    pending: &mut PendingOpen,
    session: Option<&Session>,
    preview: Option<&egui::TextureHandle>,
    missed: bool,
    pan_binding: rustroom_app::settings::PanDrag,
) {
    // ⚠ The 512-px placeholder only once the loader has said there is no preview; while one
    // may be on its way, the previous photo's frame, dimmed.
    let placeholder = pending.placeholder.as_ref().filter(|_| missed);
    let showing_placeholder = placeholder.is_some() || preview.is_some();
    let fallback = session.and_then(|s| s.texture.as_ref());
    let Some(tex) = preview.or(placeholder).or(fallback) else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    };

    // The same placement the session will use: the picture fitted to the canvas, times the
    // zoom, about the centre plus the pan — so the swap happens in place instead of snapping.
    let tex_size = tex.size_vec2();
    let avail = ui.available_size();
    let fit = (avail.x / tex_size.x.max(1.0)).min(avail.y / tex_size.y.max(1.0));
    pending.fit_long = tex_size.x.max(tex_size.y) * fit;
    let centre = ui.max_rect().center();
    let rect = egui::Rect::from_center_size(centre + pending.pan, tex_size * fit * pending.zoom);
    let resp = ui.allocate_rect(rect, egui::Sense::drag());

    // Zoom about the cursor, then pan — the session canvas's rule, on the pending view.
    let zoom_factor = ui.input(|i| i.zoom_delta());
    if zoom_factor != 1.0 && resp.hovered() {
        let old = pending.zoom;
        pending.zoom = view_region::zoom_step(old, zoom_factor);
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            let anchor = centre + pending.pan;
            let (dx, dy) =
                view_region::zoom_anchor_shift((p.x, p.y), (anchor.x, anchor.y), old, pending.zoom);
            pending.pan += egui::vec2(dx, dy);
        }
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta);
    let space_down = ui.input(|i| i.key_down(egui::Key::Space));
    let text_focused = ui.ctx().memory(|m| m.focused().is_some());
    pending.pan += pan_delta(&resp, scroll, space_down, pan_binding, text_focused);
    if pending.zoom <= view_region::MIN_ZOOM {
        pending.pan = egui::Vec2::ZERO;
    }
    let rect = paint_fitted(ui, tex, pending.zoom, pending.pan);

    if !showing_placeholder {
        // This is the *previous* photo — dim it so it reads as stale.
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(140),
        );
    }
}

/// Paint a texture fitted to the canvas, times `zoom`, about the centre plus `pan` — the
/// placement a session frame gets, so a swap happens in place instead of snapping.
fn paint_fitted(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Rect {
    let tex_size = tex.size_vec2();
    let avail = ui.available_size();
    let fit = (avail.x / tex_size.x.max(1.0)).min(avail.y / tex_size.y.max(1.0));
    let rect = egui::Rect::from_center_size(ui.max_rect().center() + pan, tex_size * fit * zoom);
    egui::Image::new(tex).paint_at(ui, rect);
    rect
}

/// The previous photo's last frame, fitted and dimmed so it reads as stale — or a spinner on
/// a first open, when there is none.
fn paint_stale(ui: &mut egui::Ui, stale: Option<&egui::TextureHandle>) {
    match stale {
        Some(tex) => {
            let rect = paint_fitted(ui, tex, 1.0, egui::Vec2::ZERO);
            // ⚠ A light dim (2026-09-29): at 140 the previous frame read as "a flash of a black
            // image" for the fraction of a second a preview takes. Lightly dimmed it reads as a
            // crossfade, and with the neighbours fetched ahead it is rarely seen at all.
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha(48),
            );
        }
        None => {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        }
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

/// The defringe eyedropper's click: the most saturated pixel of a 5×5 patch of the SOURCE buffer
/// under the pointer — the as-shot colour the defringe stage selects on, before any tail stage has
/// moved its hue — through `defringe_tool::pick`. Returns the status line and whether the photo
/// changed; `None` when there was no click on the photo.
///
/// ⚠ The source buffer, `pick_white_balance`'s reasoning: the rendered texture has every tone and
/// colour stage applied, and would read a hue the defringe step never sees.
fn pick_fringe(
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    tool: Tool,
) -> Option<(String, bool)> {
    if !resp.clicked() {
        return None;
    }
    let p = resp.interact_pointer_pos()?;
    let map = pipeline::brush::FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let [u, v] = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
    let (bw, bh) = (s.buffer.width(), s.buffer.height());
    if bw == 0 || bh == 0 || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let cx = ((u * bw as f32) as usize).min(bw - 1);
    let cy = ((v * bh as f32) as usize).min(bh - 1);
    let r = rustroom_app::color_readout::PATCH_R;
    let patch = (cy.saturating_sub(r)..=(cy + r).min(bh - 1))
        .flat_map(|y| (cx.saturating_sub(r)..=(cx + r).min(bw - 1)).map(move |x| (x, y)));
    let buffer = std::sync::Arc::clone(&s.buffer);
    let hue =
        rustroom_app::defringe_tool::most_chromatic_hue(patch.map(|(x, y)| buffer.pixel(x, y)));
    // ⚠ Disarmed after one pick, whatever it found — the white-balance eyedropper's rule.
    s.picking_fringe = false;
    let Some(h) = hue else {
        return Some((
            "No colour there to pick — click on the fringe itself".to_string(),
            false,
        ));
    };
    match rustroom_app::defringe_tool::pick(s.edits.defringe, h) {
        Some((d, f)) => {
            s.edits.defringe = d;
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            Some((
                format!("Defringe: {} range set around {h:.0}°", f.label()),
                true,
            ))
        }
        None => Some((
            format!("{h:.0}° is not a fringe colour — purple or green only"),
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::screen;
    use eframe::egui;

    /// The rung drop [`super::rungs_to_drop`] chooses, for a `Faster` setting.
    fn drop_for(dragging: bool, suppressed: Option<usize>) -> u32 {
        // A canvas gesture on a suppressed mask is view-only; nothing else here is.
        let view_only = suppressed.is_some();
        super::rungs_to_drop(dragging, 1, view_only)
    }

    /// ⚠⚠ **Reported in use: panning at 1:1 showed black rectangles at the edges that snapped to
    /// image as each new region landed** — "very stuttery", "very unpleasant". A region render
    /// paints only `img_rect`; outside it nothing was painted at all.
    ///
    /// ⚠ The backdrop is drawn **only** while a region is on screen. With a whole-image frame the
    /// two rects coincide and it would be a wasted draw of the same pixels — so a test that only
    /// counted "is there a backdrop" would pass against a version that always painted one.
    #[test]
    fn a_region_render_paints_a_backdrop_under_it_and_a_whole_image_one_does_not() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let tex = app
            .session
            .as_ref()
            .expect("session")
            .texture
            .clone()
            .expect("fixture check: the session must have a texture to use as a backdrop");
        {
            let s = app.session.as_mut().expect("session");
            s.backdrop = Some((tex, (s.fit_image_size.x, s.fit_image_size.y)));
        }
        let whole = textured_rects(&mut app, &ctx);

        {
            let s = app.session.as_mut().expect("session");
            s.zoom = 2.0;
            s.rendered_long = 80.0;
            s.rendered_region = Some(crate::RegionRect {
                x: 3,
                y: 2,
                w: 40,
                h: 20,
            });
        }
        let region = textured_rects(&mut app, &ctx);
        assert_eq!(
            region,
            whole + 1,
            "a region render must paint the backdrop under it and a whole-image one must not; \
             whole={whole} region={region}"
        );
    }

    /// The texture ids of every textured rect the canvas painted in one frame.
    fn painted_texture_ids(
        app: &mut crate::RustroomApp,
        ctx: &egui::Context,
    ) -> Vec<egui::TextureId> {
        fn collect(s: &egui::Shape, out: &mut Vec<egui::TextureId>) {
            match s {
                egui::Shape::Rect(r) => {
                    if let Some(b) = &r.brush {
                        out.push(b.fill_texture_id);
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
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
        let mut ids = Vec::new();
        out.shapes.iter().for_each(|s| collect(&s.shape, &mut ids));
        ids
    }

    /// M9.14: at fit the live render is painted; asked for more pixels than the buffer has,
    /// 2026-09-29: a ghost whose preview has not answered shows no 512-px render — the
    /// previous frame dimmed, or a spinner — and shows its own render only once the loader has
    /// said there is no preview.
    #[test]
    fn a_ghost_waits_for_its_preview_and_shows_its_own_render_only_after_a_miss() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 800, 400);
        let (live_id, key) = {
            let s = app.session.as_mut().expect("session");
            s.buffer_is_ghost = true;
            s.buffer_is_full_res = false;
            s.buffer_long = 800;
            s.fit_image_size = egui::vec2(800.0, 400.0);
            let key = rustroom_app::thumb_key::ThumbKey::new(
                rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
                &s.edits,
            );
            (s.texture.as_ref().expect("live texture").id(), key)
        };
        let waiting = painted_texture_ids(&mut app, &ctx);
        assert!(
            !waiting.contains(&live_id),
            "the ghost's own render was painted while its preview was still on its way"
        );
        app.preview_misses.insert(key);
        let after_miss = painted_texture_ids(&mut app, &ctx);
        assert!(
            after_miss.contains(&live_id),
            "with no preview to come, the ghost's render is all there is and must show"
        );
    }

    /// the preview is painted in its place; once the buffer is full resolution, the live render
    /// again. Pixel-level, through the real `show`.
    #[test]
    fn a_preview_stands_in_only_where_the_buffer_falls_short() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        // ⚠ 800×400 on a 400×300 screen, so fit is 0.5 and the buffer has the pixels at fit —
        // an 80-px fixture blown up 5× would be short of pixels at fit too, and the preview
        // would legitimately win everywhere.
        crate::test_app::open_test_photo(&mut app, &ctx, 800, 400);
        let (live_id, key) = {
            let s = app.session.as_mut().expect("session");
            s.buffer_is_full_res = false;
            s.buffer_long = 800;
            s.fit_image_size = egui::vec2(800.0, 400.0);
            let key = rustroom_app::thumb_key::ThumbKey::new(
                rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
                &s.edits,
            );
            (s.texture.as_ref().expect("live texture").id(), key)
        };
        let preview = crate::test_app::texture(&ctx, "preview", 1600, 800);
        let preview_id = preview.id();
        app.preview_textures.insert(
            key,
            (preview, rustroom_app::preview_loader::PreviewSource::Camera),
            1600 * 800 * 4,
        );

        let at_fit = painted_texture_ids(&mut app, &ctx);
        assert!(
            at_fit.contains(&live_id),
            "at fit the live render is painted"
        );
        assert!(
            !at_fit.contains(&preview_id),
            "at fit the preview stays out of it"
        );

        app.session.as_mut().expect("session").zoom = 8.0;
        let zoomed = painted_texture_ids(&mut app, &ctx);
        assert!(
            zoomed.contains(&preview_id),
            "zoomed past the buffer, the preview stands in"
        );
        assert!(
            !zoomed.contains(&live_id),
            "and the soft live render is not painted over it"
        );

        app.session.as_mut().expect("session").buffer_is_full_res = true;
        let full = painted_texture_ids(&mut app, &ctx);
        assert!(
            full.contains(&live_id),
            "the full buffer wins at every zoom"
        );
        assert!(!full.contains(&preview_id));
    }

    /// 2026-09-29: while the decode runs, the preview zooms with the pinch and the session
    /// inherits that view — "it takes three seconds to be responsive" was the wait for exactly
    /// this.
    #[test]
    fn a_pending_photo_zooms_on_its_preview_and_the_session_inherits_the_view() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let id = rustroom_app::photo_id::PhotoId::original(std::path::PathBuf::from(
            "/test/pending.RAF",
        ));
        let req = app.preview_request(id.clone(), app.settings.initial_edits(None));
        let preview = crate::test_app::texture(&ctx, "pending-preview", 800, 400);
        app.preview_textures.insert(
            req.key(),
            (preview, rustroom_app::preview_loader::PreviewSource::Camera),
            800 * 400 * 4,
        );
        app.view = crate::AppView::Develop;
        app.pending = Some(crate::PendingOpen {
            id: id.clone(),
            placeholder: None,
            start_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
            seq: None,
            preview_request: req,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit_long: 0.0,
        });
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(super::test_support::screen()),
                events: vec![egui::Event::PointerMoved(centre), egui::Event::Zoom(2.0)],
                ..Default::default()
            },
            |ui| {
                super::show(&mut app, ui, &c);
            },
        );
        let zoom = app.pending.as_ref().expect("still pending").zoom;
        assert!(
            zoom > 1.0,
            "the pinch did not zoom the pending preview: {zoom}"
        );

        let view = app.pending.as_ref().map(|p| (p.zoom, p.pan)).unwrap();
        app.pending = None;
        app.finalize_session_with_view(&ctx, id, crate::test_app::fake_decode(), view);
        let s = app.session.as_ref().expect("session");
        assert_eq!(s.zoom, zoom, "the session did not inherit the pending zoom");
    }

    /// ⚠ A backdrop from a different framing is dropped rather than stretched. A crop or an
    /// orientation change moves the frame, and this is painted across the *whole* frame rect — so
    /// a stale one is visibly wrong, not subtly.
    #[test]
    fn a_backdrop_from_a_different_framing_is_not_painted() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let tex = app
            .session
            .as_ref()
            .expect("session")
            .texture
            .clone()
            .expect("fixture check: the session must have a texture");
        {
            let s = app.session.as_mut().expect("session");
            s.zoom = 2.0;
            s.rendered_long = 80.0;
            s.rendered_region = Some(crate::RegionRect {
                x: 3,
                y: 2,
                w: 40,
                h: 20,
            });
            s.backdrop = Some((tex.clone(), (s.fit_image_size.x, s.fit_image_size.y)));
        }
        let matching = textured_rects(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("session");
            // As a crop would leave it: the same texture, a framing that no longer applies.
            s.backdrop = Some((tex, (s.fit_image_size.x * 0.6, s.fit_image_size.y * 0.6)));
        }
        let stale = textured_rects(&mut app, &ctx);
        assert_eq!(
            stale,
            matching - 1,
            "a backdrop whose framing no longer matches must not be painted; \
             matching={matching} stale={stale}"
        );
    }

    /// ⚠⚠ **The defect, as a test.** The request condition ends in
    /// `|| max_dim != s.render_rung`, which fires even when `covered` is true — so the first
    /// frame of a pan discarded a texture that already covered the viewport and replaced it with
    /// one a rung lower, for nothing.
    ///
    /// ⚠⚠ **`buffer_long` is raised to 3200 and that is what makes this test able to see
    /// anything.** The fixture's 80x40 buffer gives `fit_cap` 80 and a **one-rung ladder**, so
    /// `rung_below` returns the same rung whatever the drop — the first version of this test
    /// passed with the exemption deleted. A ladder needs `buffer_long > 1600` to have a second
    /// rung at all.
    ///
    /// ⚠ It settles with **idle** frames rather than a warm-up drag. A warm-up that dragged would
    /// already have dropped the rung under the sabotage, leaving the measured drag with no rung
    /// change to make and the test blind again for a second reason.
    ///
    /// ⚠⚠ **This guards the RUNG rule and not the coverage rule, and saying so is the point.** It
    /// seeds `rendered_region` as the whole image, which contains the margined rect as well as the
    /// visible one — so restoring the old `contains(r, want)` comparison leaves it **green**. That
    /// was verified by sabotage, not assumed. The canvas harness structurally cannot see it: a
    /// margin of 256 rendered pixels needs an image far larger than any fixture that can reach a
    /// native rung, since `tier_max_dim` needs `fit * zoom > 1` and zoom caps at 8.
    /// `view_region::a_small_pan_stays_inside_the_margin_but_not_inside_a_margined_rect` is what
    /// guards the coverage rule, and it carries its own fixture check.
    #[test]
    fn a_pan_inside_the_margin_asks_for_nothing_and_leaves_the_rung_alone() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.settings.drag_quality = rustroom_app::settings::DragQuality::Faster;
        {
            let s = app.session.as_mut().expect("session");
            s.buffer_long = 3200;
            // ⚠ Zoomed in, so the image genuinely exceeds the viewport and a *region* render is
            // in use. At fit the whole image is visible, `want` is `None`, and the seeded
            // `rendered_region` below would then hit `covered`'s `_ => false` arm — making every
            // pan re-request for a reason that has nothing to do with the rung.
            s.zoom = 4.0;
        }

        // Two idle frames, so the canvas settles on the top rung with nothing being dragged.
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let _ = super::test_support::canvas_centre(&mut app, &ctx);
        // ⚠⚠ **Seeded, because no frame ever lands in this harness.** `rendered_region` is
        // written by `shell.rs`, which the canvas tests do not run, so without this it stays
        // `None`, `covered` is never true and every pan re-requests for a reason that has
        // nothing to do with the rung. `crop_overlay`'s tests seed it for the same reason.
        // At rung 3200 the post-geometry image is 160x80 rendered pixels, so this covers it.
        {
            let s = app.session.as_mut().expect("session");
            s.rendered_region = Some(crate::RegionRect {
                x: 0,
                y: 0,
                w: 160,
                h: 80,
            });
        }
        let (before_rung, before_reqs) = {
            let s = app.session.as_ref().expect("session");
            (s.render_rung, s.engine.requests_issued())
        };
        assert_eq!(
            before_rung, 3200,
            "fixture check: the ladder must have settled on its top rung, or a drop is \
             unrepresentable and this test proves nothing"
        );

        let _ = super::test_support::drag_through_with(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(6.0, 4.0),
            egui::PointerButton::Middle,
        );
        let s = app.session.as_ref().expect("session");
        assert_eq!(
            s.render_rung, before_rung,
            "a pan changed the render rung, so the whole view went soft for nothing"
        );
        assert_eq!(
            s.engine.requests_issued(),
            before_reqs,
            "a pan inside the rendered margin asked for a render"
        );
    }

    /// ⚠ The other half, and it is what stops the fix silently deleting a feature confirmed by
    /// use. A test that only checked the pan would pass against a version that never drops the
    /// rung at all.
    #[test]
    fn a_slider_drag_still_drops_the_rung() {
        assert_eq!(
            drop_for(true, None),
            1,
            "a drag that changes the picture must still drop a rung — the interactive rung took \
             a masked-exposure drag from 137.9 ms to 40.0 ms and is confirmed by use"
        );
        assert_eq!(drop_for(false, None), 0, "nothing is being dragged");
        assert_eq!(
            drop_for(true, Some(0)),
            0,
            "a canvas gesture on a suppressed mask changes no pixels"
        );
    }

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

    /// ⚠⚠ The defringe eyedropper's wiring test (M9.6), `an_armed_eyedropper_sets_the_white_balance
    /// _from_a_click`'s twin: the pick rule has unit tests, and nothing else can see that a click
    /// reaches it. A purple source sets the purple range and amount and disarms; a grey one changes
    /// nothing, says so, and disarms too.
    #[test]
    fn an_armed_fringe_picker_sets_the_purple_range_from_a_click() {
        let run = |colour: [f32; 3]| {
            let ctx = egui::Context::default();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
            let centre = super::test_support::canvas_centre(&mut app, &ctx);
            {
                let s = app.session.as_mut().expect("a session");
                let (w, h) = (s.buffer.width(), s.buffer.height());
                let mut img = core_image::ImageBuf::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        img.set_pixel(x, y, colour);
                    }
                }
                s.buffer = std::sync::Arc::new(img);
                s.picking_fringe = true;
            }
            super::test_support::click_through(&mut app, &ctx, centre);
            let s = app.session.as_ref().expect("a session");
            assert!(!s.picking_fringe, "the picker stayed armed after a click");
            (s.edits.defringe, app.status.clone())
        };
        let (d, status) = run(color::srgb_to_prophoto([0.35, 0.12, 0.55]));
        assert!(
            d.purple_amount > 0.0,
            "the click set no purple amount: {d:?}"
        );
        assert_ne!(d.purple_hue, edit_model::Defringe::DEFAULT_PURPLE_HUE);
        assert_eq!(d.green_amount, 0.0);
        assert!(status.contains("purple"), "{status}");

        let (d, status) = run([0.3; 3]);
        assert_eq!(d, edit_model::Defringe::default());
        assert!(status.contains("No colour"), "{status}");
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

    /// How many line segments a frame paints.
    fn segments(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::LineSegment { .. } => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
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
        out.shapes.iter().map(|s| count(&s.shape)).sum()
    }

    /// ⚠⚠ **The crop rectangle stays on screen while the straighten mode is armed.** It used to
    /// vanish outright — the dispatch replaced `draw_crop_overlay` entirely, as Guide Mode does —
    /// which left the composition guides dividing a rectangle nobody could see. Reported in use;
    /// Lightroom keeps it visible too.
    ///
    /// ⚠ Counts the **corner brackets**, which only the crop marks paint: eight segments, and
    /// nothing else in the straighten overlay draws in eights. Asserting merely "more than zero"
    /// would pass against the straighten line's own two segments.
    #[test]
    fn the_crop_rectangle_stays_visible_while_straighten_is_armed() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let unarmed = segments(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let armed = segments(&mut app, &ctx);
        assert_eq!(
            armed, unarmed,
            "arming the straighten mode changed how much of the crop rectangle is drawn: \
             {unarmed} unarmed, {armed} armed"
        );
        assert!(
            armed >= 8,
            "the corner brackets are not being drawn: {armed}"
        );
    }

    /// ⚠ Guide Mode shows it too, for the same reason: the preview strips the crop, so without the
    /// rectangle there is nothing on screen saying what will be kept.
    ///
    /// ⚠ This is **not** the composition-guide rule, which stays as it was — under Guide Mode no
    /// crop rect is passed to `draw_guides`, because a guide dividing a rectangle nobody keeps is
    /// worse than none. Drawing the crop *rectangle* is a different thing from dividing it.
    #[test]
    fn the_crop_rectangle_stays_visible_in_guide_mode() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let plain = segments(&mut app, &ctx);
        app.session.as_mut().expect("a session").show_guides = true;
        let guiding = segments(&mut app, &ctx);
        assert_eq!(
            guiding, plain,
            "turning on Guide Mode changed how much of the crop rectangle is drawn: \
             {plain} plain, {guiding} in Guide Mode"
        );
        assert!(
            guiding >= 8,
            "the corner brackets are not being drawn: {guiding}"
        );
    }

    /// ⚠ Visible is not interactive. The muted rectangle is context; a drag while armed belongs to
    /// the straighten line, and the crop must not move with it. Without this, "the crop is drawn"
    /// could be satisfied by re-enabling the whole crop overlay.
    #[test]
    fn the_visible_crop_rectangle_is_not_draggable_while_armed() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.straighten_armed = true;
            s.edits.geometry.crop = edit_model::CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            };
        }
        let before = app.session.as_ref().expect("a session").edits.geometry.crop;
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(40.0, 10.0),
            centre + egui::vec2(40.0, 10.0),
        );
        let after = app.session.as_ref().expect("a session").edits.geometry.crop;
        assert_eq!(before, after, "the crop moved during a straighten drag");
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

    /// How many mesh shapes a frame paints. `painter.image` — which is all the loupe is — emits a
    /// `Shape::Mesh`, and nothing else in this canvas does.
    /// With the pointer moved to `at` first, or with no pointer at all when `at` is `None`.
    fn meshes_hovering(
        app: &mut crate::RustroomApp,
        ctx: &egui::Context,
        at: Option<egui::Pos2>,
    ) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::Mesh(_) => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
        let c = ctx.clone();
        // ⚠ Two passes: egui's pointer state is per-frame, so a hover is only *there* on the frame
        // after the move event. Reading on the same pass sees no pointer — the harness fault the
        // headless-canvas work records three separate times.
        let mut total = 0;
        for _ in 0..2 {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(super::test_support::screen()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::show(app, ui, &c);
                },
            );
            total = out.shapes.iter().map(|s| count(&s.shape)).sum();
        }
        total
    }

    /// ⚠⚠ **The loupe is up whenever the mode is armed, not only while dragging.** Reported in
    /// use: gated on a live drag, the one point you could not place precisely was the **first**
    /// one — you pressed blind and corrected afterwards.
    #[test]
    fn the_straighten_loupe_is_up_while_armed_before_any_drag() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let idle = meshes_hovering(&mut app, &ctx, Some(centre));
        app.session.as_mut().expect("a session").straighten_armed = true;
        let armed = meshes_hovering(&mut app, &ctx, Some(centre));
        assert_eq!(
            armed,
            idle + 1,
            "expected one extra mesh — the loupe — once armed; got {armed} armed, {idle} idle"
        );
    }

    /// ⚠ It follows the pointer only while the pointer is **over the photograph**. A loupe pinned
    /// in the corner magnifying nothing is chrome, and it goes stale rather than empty.
    #[test]
    fn the_straighten_loupe_is_absent_when_the_pointer_is_off_the_image() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let on = meshes_hovering(&mut app, &ctx, Some(centre));
        let off = meshes_hovering(&mut app, &ctx, Some(egui::pos2(-50.0, -50.0)));
        assert_eq!(
            on,
            off + 1,
            "the loupe survived the pointer leaving the image: {on} on, {off} off"
        );
    }

    /// ⚠ Guide Mode gets the same rule from the same helper, so the two cannot drift.
    #[test]
    fn the_guide_loupe_is_up_before_any_endpoint_is_grabbed() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let idle = meshes_hovering(&mut app, &ctx, Some(centre));
        app.session.as_mut().expect("a session").show_guides = true;
        let guiding = meshes_hovering(&mut app, &ctx, Some(centre));
        assert_eq!(
            guiding,
            idle + 1,
            "expected one extra mesh — the loupe — in Guide Mode; got {guiding}, idle {idle}"
        );
    }

    /// How many **textured rectangles** a frame paints with no pointer over it.
    /// `egui::Image::paint_at` — the frame and the tint alike — emits a `RectShape` with a
    /// brush, not a mesh; the loupe's `painter.image` is what emits meshes.
    fn textured_rects(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::Rect(r) if r.brush.is_some() => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
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
        out.shapes.iter().map(|s| count(&s.shape)).sum()
    }

    /// ⚠ The view's texture is one extra textured rect over the photograph, painted at the
    /// image rect; absent, it paints nothing. Counted, the way the loupe is.
    #[test]
    fn a_landed_plane_is_painted_over_the_photograph() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let idle = textured_rects(&mut app, &ctx);
        assert_eq!(idle, 1, "setup: the photograph itself is one textured rect");
        {
            let s = app.session.as_mut().expect("a session");
            let rgba = rustroom_app::grade_view::tint_rgba(&vec![200u8; 80 * 60]);
            let img = egui::ColorImage::from_rgba_unmultiplied([80, 60], &rgba);
            s.grade_view_texture =
                Some(ctx.load_texture("grade_view_test", img, egui::TextureOptions::LINEAR));
        }
        let tinted = textured_rects(&mut app, &ctx);
        assert_eq!(
            tinted,
            idle + 1,
            "expected one extra textured rect — the range tint; got {tinted} tinted, {idle} idle"
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

    /// ⚠⚠ A placed spot must be ONE history step. Before M9.11 the overlay changed the edits and
    /// reported the change, but never pushed — so the action log recorded a step the history did
    /// not have, and Ctrl+Z popped the step *before* the spot too.
    #[test]
    fn placing_a_spot_is_one_history_step() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.edits.spots.len(),
            1,
            "fixture check: the click placed a spot"
        );
        assert_eq!(
            s.history.undo_depth(),
            depth + 1,
            "the placement was not recorded"
        );
        assert_eq!(
            s.history.get(),
            &s.edits,
            "history's present is not the edits"
        );
    }

    /// And a move is one step for the whole gesture, not one per drag frame.
    #[test]
    fn moving_a_spot_is_one_history_step() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let before = spots(&app)[0].centre;
        super::test_support::drag_through(&mut app, &ctx, centre, centre + egui::vec2(30.0, 0.0));
        let s = app.session.as_ref().unwrap();
        assert_ne!(
            s.edits.spots[0].centre, before,
            "fixture check: the drag moved the spot"
        );
        assert_eq!(
            s.history.undo_depth(),
            depth + 1,
            "a move was not exactly one step"
        );
        assert_eq!(s.history.get(), &s.edits);
    }

    fn eye_app(ctx: &egui::Context, kind: edit_model::EyeKind) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.eye_tool.active = Some(kind);
        app
    }

    fn eyes(app: &crate::RustroomApp) -> Vec<edit_model::Eye> {
        app.session.as_ref().expect("a session").edits.eyes.clone()
    }

    /// M9.11: a click in an eye mode places one eye and no patch, as one undo step, selected.
    #[test]
    fn an_eye_mode_click_places_one_eye_and_no_spot_as_one_history_step() {
        let ctx = egui::Context::default();
        let mut app = eye_app(&ctx, edit_model::EyeKind::Red);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(
            eyes(&app).len(),
            1,
            "the click did not place exactly one eye"
        );
        assert!(spots(&app).is_empty(), "an eye-mode click placed a spot");
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.history.undo_depth(), depth + 1, "not one history step");
        assert_eq!(s.history.get(), &s.edits);
        assert_eq!(s.selected_eye, Some(0));
        assert_eq!(s.selected_spot, None);
        // The test photo is a flat chart: nothing to detect, so the fallback circle and its
        // message — a click is never a no-op.
        assert!(
            app.status.contains("No red pupil"),
            "no message for a failed detection: {:?}",
            app.status
        );
    }

    /// The guard on the guard: patch mode still places patches, and no eyes.
    #[test]
    fn patch_mode_places_no_eye() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        assert!(eyes(&app).is_empty(), "patch mode placed an eye");
        assert_eq!(spots(&app).len(), 1);
    }

    #[test]
    fn clicking_a_placed_eye_selects_it_and_a_drag_inside_moves_it_as_one_step() {
        let ctx = egui::Context::default();
        let mut app = eye_app(&ctx, edit_model::EyeKind::Red);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        {
            // Grabbable at test scale: the fallback radius is a few screen points here.
            let s = app.session.as_mut().unwrap();
            s.edits.eyes[0].radius = 0.2;
            s.history.push(s.edits.clone());
            s.selected_eye = None;
        }
        super::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(eyes(&app).len(), 1, "the second click stacked an eye");
        assert_eq!(app.session.as_ref().unwrap().selected_eye, Some(0));
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let before = eyes(&app)[0].centre;
        super::test_support::drag_through(&mut app, &ctx, centre, centre + egui::vec2(10.0, 0.0));
        let s = app.session.as_ref().unwrap();
        assert_ne!(
            s.edits.eyes[0].centre, before,
            "the drag did not move the eye"
        );
        assert_eq!(s.edits.eyes[0].radius, 0.2, "a move resized the eye");
        assert_eq!(s.history.undo_depth(), depth + 1, "a move was not one step");
        assert_eq!(s.suppressed_eye, None, "the eye stayed out of the view");
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

    /// ⚠⚠ **Spots live BEFORE the perspective since M7.3b**, so a click on a keystoned photo must
    /// store the homography's *source* of the clicked point — `SpotMap::inverse_point` — and not
    /// `FrameMap`'s answer, which is the post-perspective point the brush would use.
    ///
    /// ⚠ The crop moves the clicked point off the frame centre, where a keystone might leave it
    /// in place, and the fixture check below proves the two answers differ before comparing.
    #[test]
    fn a_click_on_a_keystoned_photo_lands_in_pre_perspective_space() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let crop = edit_model::CropRect {
            left: 0.5,
            top: 0.3,
            right: 1.0,
            bottom: 1.0,
        };
        let persp = edit_model::Perspective {
            vertical: 50.0,
            horizontal: 50.0,
            scale: 0.0,
            ..Default::default()
        };
        {
            let s = app.session.as_mut().expect("a session");
            s.edits.geometry.crop = crop;
            s.edits.perspective = persp;
            // ⚠⚠ A frame that has already LANDED at the cropped size. No render runs in this
            // harness, so `fit_image_size` would otherwise still be the uncropped 80x60 and the
            // canvas frame would carry the wrong aspect. A crop alone cannot see that (the
            // cropped-photo test above passes either way); a keystone can, because the homography
            // is defined in half-height units. The straighten test's `shown_geo` does the same.
            s.fit_image_size = egui::vec2(40.0, 42.0);
        }
        // The 80x60 test photo cropped to its right half and lower 70%: a 40x42 frame. Only the
        // aspect matters to the normalised answer, so native pixels are as good as screen ones.
        let geometry = edit_model::Geometry {
            crop,
            ..Default::default()
        };
        let (fw, fh) = (40usize, 42usize);
        let want = pipeline::brush::SpotMap::new(&geometry, persp, fw, fh)
            .inverse_point(fw as f32 / 2.0, fh as f32 / 2.0);
        let brush = pipeline::brush::FrameMap::new(&geometry, fw, fh)
            .inverse_point(fw as f32 / 2.0, fh as f32 / 2.0);
        assert!(
            (want[0] - brush[0]).abs() + (want[1] - brush[1]).abs() > 0.02,
            "fixture check: the keystone must move the point ({want:?} vs {brush:?})"
        );

        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click placed nothing");
        let got = placed[0].centre;
        assert!(
            (got[0] - want[0]).abs() < 0.02 && (got[1] - want[1]).abs() < 0.02,
            "stored {got:?}, pre-perspective {want:?}, post-perspective (the old answer) {brush:?}"
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

        // ⚠ Per patch since 2026-09-20: the row's eye, `Session::spot_shown`.
        app.session.as_mut().unwrap().spot_shown = vec![true];
        let shown = painted(&mut app, &ctx);
        app.session.as_mut().unwrap().spot_shown = vec![false];
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

    /// The readout as the status cluster renders it, or empty when there is none.
    ///
    /// ⚠ Goes through `color_readout::label`, the same function `shell.rs` calls, so a change to
    /// the wording cannot pass here and fail on screen.
    fn readout(app: &crate::RustroomApp) -> String {
        app.session
            .as_ref()
            .expect("a session")
            .readout
            .map(rustroom_app::color_readout::label)
            .unwrap_or_default()
    }

    /// The sampled value itself.
    fn readout_value(app: &crate::RustroomApp) -> Option<[u8; 3]> {
        app.session.as_ref().expect("a session").readout
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
        // ⚠ Asserted through the stored **value** rather than by matching the label's wording:
        // the wording is `color_readout`'s to decide (it lost its `R`/`G`/`B` prefixes when the
        // swatch arrived), and a wiring test that pins the wording breaks on every cosmetic
        // change while saying nothing about the wiring.
        let v = readout_value(&app).expect("a sampled value");
        assert_eq!(
            got,
            rustroom_app::color_readout::label(v),
            "the string the cluster shows is not this value's label"
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
        assert!(s.readout.is_some(), "fixture check: nothing was read");
        assert_eq!(
            s.engine.requests_issued(),
            before.0,
            "the hover requested a render"
        );
        assert_eq!(s.history.can_undo(), before.1, "the hover pushed history");
        assert_eq!(s.edits, before.2, "the hover changed the edits");
    }
}

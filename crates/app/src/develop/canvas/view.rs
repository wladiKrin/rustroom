//! The view: the render rung and region the canvas asks for, and where the frame is placed.

use crate::develop::canvas_geom;
use crate::Session;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::render_engine::RegionRect;
use rustroom_app::thumb_key::ThumbKey;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;

/// What `plan` decided about this frame.
pub(super) struct ViewPlan {
    pub(super) avail: egui::Vec2,
    pub(super) full: egui::Vec2,
    pub(super) fit: f32,
    pub(super) fit_cap: usize,
    pub(super) max_dim: usize,
    pub(super) native: bool,
    pub(super) full_long: f32,
    pub(super) view_is_plain: bool,
    pub(super) needed_long: usize,
    pub(super) vis: Option<RegionRect>,
    pub(super) want: Option<RegionRect>,
}

/// Where the frame goes on screen.
pub(super) struct Placement {
    pub(super) place_region: Option<RegionRect>,
    pub(super) place_long: f32,
    pub(super) pk: f32,
    pub(super) pscale: f32,
    pub(super) img_rect: egui::Rect,
    pub(super) frame_rect: egui::Rect,
}

/// Whether `RUSTROOM_DEBUG_PLACE` is set. Read once: the placement log runs per frame, where
/// the render engine's equivalent runs per render.
static DEBUG_PLACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("RUSTROOM_DEBUG_PLACE").is_some());
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
pub(super) fn rungs_to_drop(dragging: bool, quality_drop: u32, view_only_gesture: bool) -> u32 {
    if !dragging || view_only_gesture {
        0
    } else {
        quality_drop
    }
}

/// The fit, the rung and the region this frame needs — read before anything is allocated, so
/// `available_size` is the canvas's whole area. Returns the edits the view shows and the preview key too.
pub(super) fn plan(
    ui: &egui::Ui,
    s: &mut Session,
    tool: Tool,
    app_drag_drop: u32,
) -> (ViewPlan, Edits, ThumbKey) {
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
    let max_dim = view_region::rung_below(rung, fit_cap, s.buffer_long, app_drag_drop);
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
        let v = view_region::visible_rect(iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y);
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
    (
        ViewPlan {
            avail,
            full,
            fit,
            fit_cap,
            max_dim,
            native,
            full_long,
            view_is_plain,
            needed_long,
            vis,
            want,
        },
        shown_edits,
        stand_in_key,
    )
}

/// Ask for a render when what is rendered no longer covers the view, or the rung changed.
pub(super) fn request_if_stale(s: &mut Session, plan: &ViewPlan, shown_edits: Edits) {
    let ViewPlan {
        vis, want, max_dim, ..
    } = *plan;
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
    if (!covered && s.requested_region != want) || (max_dim != s.render_rung && !soft_frame) {
        s.requested_region = want;
        s.render_rung = max_dim;
        s.request(shown_edits, max_dim, want);
    }
}

/// Where the frame and the whole post-geometry frame sit on screen, from the texture's own render
/// resolution.
pub(super) fn place(
    ui: &egui::Ui,
    s: &Session,
    shown: &egui::TextureHandle,
    plan: &ViewPlan,
) -> Placement {
    let ViewPlan {
        full,
        fit,
        full_long,
        ..
    } = *plan;
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
        None => {
            egui::Rect::from_center_size(ui.max_rect().center() + s.pan, shown.size_vec2() * pscale)
        }
    };
    // ⚠ Where the **whole frame** sits on screen, which is *not* `img_rect`
    // once a region render is on: `img_rect` then covers only the rendered
    // slice. A mask is defined in whole-frame normalised coordinates, so its
    // overlay must be placed against this rect or it drifts with the region —
    // appearing to follow the view as you pan, which is exactly how the bug
    // showed up. `origin` is the same whole-image corner the region placement
    // above derives from, so the two agree by construction.
    let frame_rect = canvas_geom::frame_rect(ui.max_rect().center(), s.pan, full, pk, pscale);
    Placement {
        place_region,
        place_long,
        pk,
        pscale,
        img_rect,
        frame_rect,
    }
}

/// `RUSTROOM_DEBUG_PLACE=1`: every input the placement is derived from, one line per frame.
pub(super) fn debug_place(
    s: &Session,
    shown: &egui::TextureHandle,
    plan: &ViewPlan,
    place: &Placement,
) {
    let ViewPlan {
        max_dim,
        native,
        fit_cap,
        full,
        fit,
        avail,
        ..
    } = *plan;
    let Placement {
        place_long,
        pk,
        pscale,
        img_rect,
        frame_rect,
        ..
    } = *place;
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
}

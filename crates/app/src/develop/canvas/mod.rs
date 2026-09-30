//! The central image canvas: the rendered photo, the render rung and region it asks for, its
//! placement, and zoom and pan. The per-tool overlays live in the sibling modules and `show`
//! dispatches to exactly one of them.
//!
//! ⚠ `frame_rect` — where the **whole frame** sits on screen — is computed here and handed
//! down, because every overlay must place against it rather than against `img_rect`. See its
//! comment at the call site.

mod composition;
mod crop_overlay;
pub(crate) mod eye_overlay;
mod eyedropper;
mod guide_overlay;
mod loupe;
mod mask_overlay;
pub(crate) mod mask_tint;
pub(crate) mod navigation;
mod paint;
mod readout;
pub(crate) mod spot_overlay;
mod straighten_overlay;
#[cfg(test)]
mod test_support;
mod tools;
mod view;

use crate::{RustroomApp, Session};
use eframe::egui;
use guide_overlay::GuideDrag;
use paint::draw_pending_canvas;
use rustroom_app::tool::Tool;
use view::rungs_to_drop;

/// What an empty Develop says. ⚠ There has been no "Open file… button" since the menus took the
/// toolbar's place; the old text still named one. Also swept by
/// `theme::every_menu_string_is_renderable`.
pub(crate) const EMPTY_DEVELOP_TEXT: &str =
    "Open a photo from the Library, or File → Open file… (⌘O).";

/// What drawing the canvas changed, written back to `app` once the session borrow ends.
struct CanvasOutcome {
    photo_edited: bool,
    /// ⚠ The solve needs `&mut RustroomApp`, which the session borrow below rules out — so the
    /// overlay only reports that a drag settled and `show` runs the solve afterwards.
    guide_drag: GuideDrag,
    /// ⚠⚠ **A COPY of the tool, written back after the session borrow ends.** The size-drag has to
    /// update `app.spot_tool.radius` so the panel's Size slider follows the gesture, but
    /// `&mut app.spot_tool` cannot be taken while `match &mut app.session` holds its borrow — the
    /// canvas takes `&mut app` whole, unlike the panel, which borrows disjoint fields.
    app_spot_tool: rustroom_app::spot_tool::SpotTool,
    app_eye_tool: rustroom_app::eye_tool::EyeTool,
    eye_status: Option<String>,
    /// ⚠ The defringe pick's status line, written back to `app.status` after the session borrow —
    /// `app_spot_tool`'s shape (M9.6).
    fringe_status: Option<String>,
}

/// What the canvas reads from `app`, copied out before `app.session` is borrowed mutably.
struct CanvasInputs {
    tool: Tool,
    app_pan_drag: rustroom_app::settings::PanDrag,
    app_erase_drag: rustroom_app::settings::EraseDrag,
    app_guide: rustroom_app::composition_guides::CompositionGuide,
    app_drag_drop: u32,
    app_brush_size: f32,
    app_brush_hardness: f32,
    app_brush_erase: bool,
    app_brush_auto_mask: bool,
    app_brush_tolerance: f32,
    app_tint_frame: Option<mask_overlay::TintFrame>,
    app_crop_aspect: Option<f32>,
    app_brush_flow: f32,
}

/// Copy out everything the canvas reads — with the **outer** `ui`, whose rect `press_on_canvas` asks about.
fn hoist(app: &RustroomApp, ui: &egui::Ui) -> CanvasInputs {
    // ⚠ Hoisted before `app.session` is borrowed mutably: `app.tool` is a disjoint field, but
    // reading it *through* `app` after the borrow is E0502. `Tool` is `Copy`, so this is free.
    let tool = app.tool;
    // ⚠ Copied out before `match &mut app.session` takes its mutable borrow. `PanDrag` is
    // `Copy`, so this is a copy rather than a borrow that would collide.
    let app_pan_drag = app.settings.pan_drag;
    // ⚠ Same reason as `app_pan_drag`: copied out before `app.session` is borrowed mutably.
    let app_erase_drag = app.settings.erase_drag;
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
    CanvasInputs {
        tool,
        app_pan_drag,
        app_erase_drag,
        app_guide,
        app_drag_drop,
        app_brush_size,
        app_brush_hardness,
        app_brush_erase,
        app_brush_auto_mask,
        app_brush_tolerance,
        app_tint_frame,
        app_crop_aspect: app.crop_aspect,
        app_brush_flow: app.brush_flow,
    }
}

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) -> bool {
    let inp = hoist(app, ui);
    let mut out = CanvasOutcome {
        photo_edited: false,
        guide_drag: GuideDrag::None,
        app_spot_tool: app.spot_tool,
        app_eye_tool: app.eye_tool,
        eye_status: None,
        fringe_status: None,
    };
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
                    draw_pending_canvas(
                        ui,
                        p,
                        app.session.as_ref(),
                        preview,
                        missed,
                        inp.app_pan_drag,
                    );
                    return;
                }
            }
            // ⚠ Disjoint field borrows, taken before `app.session` is borrowed mutably below.
            let preview_textures = &app.preview_textures;
            let preview_misses = &app.preview_misses;
            match &mut app.session {
                Some(s) => session_canvas(
                    ui,
                    s,
                    &inp,
                    paint::Previews {
                        textures: preview_textures,
                        misses: preview_misses,
                    },
                    &mut out,
                ),
                None => {
                    ui.centered_and_justified(|ui| {
                        crate::theme::empty_state(ui, EMPTY_DEVELOP_TEXT);
                    });
                }
            }
        });
    write_back(out, app, inp.tool)
}

/// The canvas for a session: its render planned and requested, the frame placed and painted, then
/// the tool's overlay, the composition guides, zoom and pan, and the readout — in that order.
fn session_canvas(
    ui: &mut egui::Ui,
    s: &mut Session,
    inp: &CanvasInputs,
    pv: paint::Previews,
    out: &mut CanvasOutcome,
) {
    if s.texture.is_none() {
        paint::no_frame_yet(ui, s, pv.textures);
        return;
    }
    // ⚠ Always the rendered frame, never `s.baseline`. Before/after is a
    // property of *what is rendered* now — see `Session::view_edits` — so the
    // before image arrives at the current rung and region like any other, and
    // is sharp at 1:1 instead of a magnified preview-resolution texture.
    let (plan, shown_edits, stand_in_key) = view::plan(ui, s, inp.tool, inp.app_drag_drop);
    view::request_if_stale(s, &plan, shown_edits);
    let shown = s.texture.as_ref().expect("the texture is checked above");
    let place = view::place(ui, s, shown, &plan);
    s.canvas_frame_rect = Some(place.frame_rect);
    view::debug_place(s, shown, &plan, &place);
    let resp = paint::frame(ui, s, shown, &plan, &place, pv, stand_in_key);
    tools::dispatch(ui, s, &resp, &place, inp, out);
    composition::draw(ui, s, inp.tool, &place, inp.app_guide);
    // ⚠ Every tool zooms and pans now. This was gated off for Geometry because
    // the crop overlay was placed by fit-mode maths; see the rung above.
    navigation::zoom_and_pan(ui, s, &resp, inp.app_pan_drag);
    readout::sample_hover(ui, s, place.img_rect);
}

/// The session borrow is over: apply what the canvas collected, in the order it always was.
fn write_back(mut out: CanvasOutcome, app: &mut RustroomApp, tool: Tool) -> bool {
    // ⚠ Solved on **release only**, so the image is frozen for the whole drag. One gesture, one
    // Ctrl+Z, the rule every drag here follows.
    if out.guide_drag == GuideDrag::Settled && app.solve_guides() {
        if let Some(s) = app.session.as_mut() {
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            s.history.push(s.edits.clone());
        }
        out.photo_edited = true;
    }
    // ⚠ Written back after the session borrow ends: the size-drag sets the radius, and the panel's
    // Size slider reads this same field next frame. One value, two affordances.
    app.spot_tool = out.app_spot_tool;
    app.eye_tool = out.app_eye_tool;
    if let Some(m) = out.eye_status {
        app.status = m;
    }
    if let Some(m) = out.fringe_status {
        app.status = m;
    }
    out.photo_edited
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
            s.rendered_region = Some(rustroom_app::render_engine::RegionRect {
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
            s.rendered_region = Some(rustroom_app::render_engine::RegionRect {
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
            s.rendered_region = Some(rustroom_app::render_engine::RegionRect {
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

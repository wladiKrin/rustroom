//! The central image canvas: the rendered photo, the render rung and region it asks for, its
//! placement, and zoom and pan. The per-tool overlays live in the sibling modules and `show`
//! dispatches to exactly one of them.
//!
//! ⚠ `frame_rect` — where the **whole frame** sits on screen — is computed here and handed
//! down, because every overlay must place against it rather than against `img_rect`. See its
//! comment at the call site.

mod composition;
mod crop_overlay;
mod depth_overlay;
pub(crate) mod eye_overlay;
mod eyedropper;
mod guide_overlay;
mod loupe;
mod mask_overlay;
pub(crate) mod mask_tint;
pub(crate) mod navigation;
mod paint;
mod readout;
pub(crate) mod remove_overlay;
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
///
/// ⚠ The chord comes from the user's bindings (M10.9b).
pub(crate) fn empty_develop_text(keys: &[rustroom_app::shortcuts::Rebinding]) -> String {
    format!(
        "Open a photo from the Library, or File → Open file…{}.",
        rustroom_app::shortcuts::hint(rustroom_app::menu::Command::OpenFile, keys)
    )
}

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
    /// Show depth map (M10.16a) — view state on the app.
    app_show_depth_map: bool,
    /// Lens Blur's Visualize depth (M10.16d) — view state on the app.
    app_show_lens_depth: bool,
    /// Retouch's Remove brush (M10.16c1) — the canvas reads it, only the panel changes it.
    app_remove_tool: rustroom_app::remove_tool::RemoveTool,
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
    // ⚠ M10.16c1: a Remove stroke changes no pixel until its release either — the view must stay sharp
    // under the brush.
    let app_remove_stroke =
        tool == Tool::Retouch && app.remove_tool.active && app.eye_tool.active.is_none();
    let view_only_gesture =
        press_on_canvas && (panning || app_suppressed_mask || app_remove_stroke);
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
        app_show_depth_map: app.show_depth_map,
        app_show_lens_depth: app.show_lens_depth,
        app_remove_tool: app.remove_tool,
    }
}

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) -> bool {
    let empty_text = empty_develop_text(&app.settings.shortcuts);
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
                        crate::theme::empty_state(ui, &empty_text);
                    });
                }
            }
        });
    write_back(out, app, inp.tool)
}

/// The view's centre `centre` on screen as a decode point, through the photo's frame `frame` — what AI
/// denoise's job is aimed at (M10.16b4); the eyedropper's own mapping.
pub(crate) fn view_focus(s: &Session, frame: egui::Rect, centre: egui::Pos2) -> [f32; 2] {
    eyedropper::decode_point(s, frame, centre)
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
    s.canvas_view_centre = Some(ui.clip_rect().center());
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
}

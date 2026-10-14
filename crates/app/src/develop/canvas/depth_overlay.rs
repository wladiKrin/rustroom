//! A Depth range on the canvas (M10.16a): the picker — a drag sets the band to the depths under the
//! dragged rectangle, live; a click, a narrow band around the depth under it.
//!
//! ⚠ The gesture only: the rule is `rustroom_app::depth_tool`, the sampling `ai_coverage::AiSource`.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::panel::mask_components;
use crate::theme::Hover;
use crate::Session;
use eframe::egui;
use rustroom_app::ai_coverage::{AiSource, MapQuery};
use rustroom_app::depth_tool;
use rustroom_app::tool::Tool;

/// Pick a band for component `j` of mask `i`, a depth range. `true` when this frame pushed history.
///
/// ⚠ The drag is the canvas's own (`resp`, primary button), as a brush's is — no Settings binding:
/// it is the component's primary action; Space+drag still pans, as it does over a brush.
#[allow(clippy::too_many_arguments)]
pub(super) fn pick(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    i: usize,
    j: usize,
    tool: Tool,
) -> bool {
    let click = ui
        .interact(img_rect, ui.id().with("depth_pick"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    // ⚠ A snapshot — a handful of `Arc`s — so no lock is held while the edits are written below.
    let maps = s.engine.ai_maps().read().expect("ai maps").clone();
    let Some(map) = maps.get(&MapQuery::Depth).cloned() else {
        click.hover("Waiting for the depth map");
        return false;
    };
    let to_frame = |p: egui::Pos2| [p.x - frame_rect.left(), p.y - frame_rect.top()];
    let dragging = tool_drag(resp) || tool_drag_started(resp);
    let band = {
        let src = AiSource {
            maps: &maps,
            edits: &s.edits,
            lens: s.lens.shot.as_ref(),
            decode: (s.buffer.width(), s.buffer.height()),
        };
        let lensed = src.lensed(&map);
        let dm = src.decode_map(
            &s.edits.geometry,
            frame_rect.width() as usize,
            frame_rect.height() as usize,
        );
        if dragging {
            let from = ui.input(|i| i.pointer.press_origin());
            match (from, resp.interact_pointer_pos()) {
                (Some(a), Some(b)) => {
                    let painter = ui.painter_at(img_rect);
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(a, b),
                        0.0,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                        egui::StrokeKind::Middle,
                    );
                    let ([x0, y0], [x1, y1]) = (to_frame(a), to_frame(b));
                    depth_tool::band(&mut depth_tool::samples(&lensed, &dm, [x0, y0, x1, y1]))
                }
                _ => None,
            }
        } else if click.clicked() {
            click.interact_pointer_pos().and_then(|p| {
                let [x, y] = to_frame(p);
                depth_tool::samples(&lensed, &dm, [x, y, x, y])
                    .first()
                    .map(|&v| depth_tool::band_at(v))
            })
        } else {
            None
        }
    };
    if let Some((near, far)) = band {
        if let Some(edit_model::Mask::Depth(r)) = s
            .edits
            .masks
            .get_mut(i)
            .and_then(|m| mask_components::component_mut(m, j))
        {
            r.near = near;
            r.far = far;
        }
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }
    // ⚠ One undo step per gesture — at its end, outside the pointer-position block (on the frame a
    // drag ends egui often reports no position; the brush's lesson in `mask_overlay`).
    if resp.drag_stopped() || (click.clicked() && band.is_some()) {
        s.history.push(s.edits.clone());
        return true;
    }
    false
}

/// The Show-depth-map view's grid, at most, on its long side — the maps themselves are at most
/// ~1536 px long, and a screen-sized grid is what is seen (`ai_coverage`'s `GRID_LONG` reason).
const VIEW_LONG: f32 = 1024.0;

/// What a cached view was drawn for.
#[derive(Clone, Copy, PartialEq)]
struct ViewKey {
    map: u64,
    geometry: edit_model::Geometry,
    perspective: edit_model::Perspective,
    lens: u64,
    w: usize,
    h: usize,
}

thread_local! {
    /// ⚠ Thread-local, the tint's own reason (`mask_tint::TINT`): reached from inside egui closures
    /// that hold `&mut app`; egui is single-threaded per context.
    static VIEW: std::cell::RefCell<Option<(ViewKey, egui::TextureHandle)>> =
        const { std::cell::RefCell::new(None) };
}

/// Show depth map (M10.16a): the selected depth range's map in grey over the frame, the nearest
/// point white — through the render's own decode mapping, so it lines up through crop and rotation.
/// Returns the texture it drew, `None` when there is nothing to show (no depth range selected, or
/// its map not yet computed).
///
/// ⚠ Drawn UNDER the tints by `draw_mask_overlay`, so a mask's eye shows its band against the depths.
pub(super) fn paint(
    painter: &egui::Painter,
    s: &Session,
    frame_rect: egui::Rect,
) -> Option<egui::TextureId> {
    let selected = s
        .selected_mask
        .and_then(|i| s.edits.masks.get(i))
        .and_then(|m| mask_components::component(m, s.component_index()));
    if !matches!(selected, Some(edit_model::Mask::Depth(_))) {
        return None;
    }
    let maps = s.engine.ai_maps().read().expect("ai maps").clone();
    let map = maps.get(&MapQuery::Depth)?;
    let k = (VIEW_LONG / frame_rect.width().max(frame_rect.height()).max(1.0)).min(1.0);
    let (w, h) = (
        (frame_rect.width() * k).round().max(1.0) as usize,
        (frame_rect.height() * k).round().max(1.0) as usize,
    );
    let decode = (s.buffer.width(), s.buffer.height());
    let key = ViewKey {
        map: maps.id(&MapQuery::Depth).unwrap_or(0),
        geometry: s.edits.geometry,
        perspective: s.edits.perspective,
        lens: rustroom_app::ai_space::lens_key(s.lens.shot.as_ref(), &s.edits, decode.0, decode.1),
        w,
        h,
    };
    let id = VIEW.with(|v| {
        let mut v = v.borrow_mut();
        if let Some((k, tex)) = v.as_ref() {
            if *k == key {
                return tex.id();
            }
        }
        let src = AiSource {
            maps: &maps,
            edits: &s.edits,
            lens: s.lens.shot.as_ref(),
            decode,
        };
        let rgba = rustroom_app::depth_view::image(
            &src.lensed(map),
            &src.decode_map(&s.edits.geometry, w, h),
            w,
            h,
        );
        let tex = painter.ctx().load_texture(
            "depth_view",
            egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba),
            egui::TextureOptions::LINEAR,
        );
        let id = tex.id();
        *v = Some((key, tex));
        id
    });
    painter.image(
        id,
        frame_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{canvas_centre, click_through, drag_through};
    use crate::RustroomApp;
    use eframe::egui;

    /// The Masks tool, a Depth range selected, and — with `map` — a depth map whose left half is at
    /// 0.2 and right half at 0.8.
    fn depth_app(ctx: &egui::Context, map: bool) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Masks;
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Depth(edit_model::DepthRange::default()),
            ..Default::default()
        });
        s.selected_mask = Some(0);
        if map {
            s.engine.ai_maps().write().unwrap().insert(
                rustroom_app::ai_coverage::MapQuery::Depth,
                std::sync::Arc::new(ai_mask::ProbMap::from_fn(80, 40, |x, _| {
                    if x < 40 {
                        0.2
                    } else {
                        0.8
                    }
                })),
            );
        }
        app
    }

    fn band(app: &RustroomApp) -> edit_model::DepthRange {
        match app.session.as_ref().unwrap().edits.masks[0].mask {
            edit_model::Mask::Depth(r) => r,
            ref other => panic!("expected a depth range, got {other:?}"),
        }
    }

    fn steps(app: &RustroomApp) -> usize {
        app.session.as_ref().unwrap().history.undo_depth()
    }

    #[test]
    fn a_drag_across_two_depths_sets_the_band_to_them_as_one_step() {
        let ctx = egui::Context::default();
        let mut app = depth_app(&ctx, true);
        let c = canvas_centre(&mut app, &ctx);
        let before = steps(&app);
        drag_through(
            &mut app,
            &ctx,
            c - egui::vec2(60.0, 8.0),
            c + egui::vec2(60.0, 8.0),
        );
        let r = band(&app);
        assert!(
            (r.near - 20.0).abs() < 1.5 && (r.far - 80.0).abs() < 1.5,
            "{r:?}"
        );
        assert_eq!(steps(&app), before + 1, "one undo step for the gesture");
    }

    #[test]
    fn a_click_sets_a_narrow_band_around_the_depth_under_it() {
        let ctx = egui::Context::default();
        let mut app = depth_app(&ctx, true);
        let c = canvas_centre(&mut app, &ctx);
        let before = steps(&app);
        click_through(&mut app, &ctx, c + egui::vec2(60.0, 0.0));
        let r = band(&app);
        assert!(
            (r.near - 75.0).abs() < 0.6 && (r.far - 85.0).abs() < 0.6,
            "{r:?}"
        );
        assert_eq!(steps(&app), before + 1);
    }

    /// Draw `paint` once against `app`'s session, in a 400 × 200 frame.
    fn painted(app: &RustroomApp, ctx: &egui::Context) -> Option<egui::TextureId> {
        let mut out = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 200.0));
            out = super::paint(ui.painter(), app.session.as_ref().unwrap(), rect);
        });
        out
    }

    #[test]
    fn the_depth_view_draws_only_a_selected_depth_ranges_map() {
        let ctx = egui::Context::default();
        let app = depth_app(&ctx, true);
        assert!(
            painted(&app, &ctx).is_some(),
            "selected, with its map: drawn"
        );
        let waiting = depth_app(&ctx, false);
        assert!(painted(&waiting, &ctx).is_none(), "no map yet: nothing");
        let mut other = depth_app(&ctx, true);
        other.session.as_mut().unwrap().selected_mask = None;
        assert!(
            painted(&other, &ctx).is_none(),
            "no depth range selected: nothing"
        );
    }

    /// ⚠ The toggle is what draws it: with Show depth map off the canvas never paints the view.
    ///
    /// ⚠ The texture is read from the cache AFTER the canvas's own passes: a `paint` at another size
    /// keys another texture, and an id taken from it matched nothing — this test's first version.
    /// Off and on get the same passes, so "on draws" is the positive control for "off does not".
    #[test]
    fn the_canvas_paints_the_view_only_with_the_toggle_on() {
        let ctx = egui::Context::default();
        let mut app = depth_app(&ctx, true);
        let cached = || super::VIEW.with(|v| v.borrow().as_ref().map(|(_, t)| t.id()));
        super::VIEW.with(|v| *v.borrow_mut() = None);
        let run = |app: &mut RustroomApp| {
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let c = ctx.clone();
                shapes = ctx
                    .run_ui(
                        egui::RawInput {
                            screen_rect: Some(super::super::test_support::screen()),
                            ..Default::default()
                        },
                        |ui| {
                            super::super::show(app, ui, &c);
                        },
                    )
                    .shapes;
            }
            shapes
        };
        let _ = run(&mut app);
        assert!(cached().is_none(), "off: the view was drawn");
        app.show_depth_map = true;
        let shapes = run(&mut app);
        let id = cached().expect("on: the view was not drawn");
        assert!(
            shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Mesh(m) if m.texture_id == id)),
            "on: its texture is not on the canvas"
        );
    }

    #[test]
    fn without_its_map_the_picker_changes_nothing() {
        let ctx = egui::Context::default();
        let mut app = depth_app(&ctx, false);
        let c = canvas_centre(&mut app, &ctx);
        let before = (band(&app), steps(&app));
        drag_through(
            &mut app,
            &ctx,
            c - egui::vec2(60.0, 8.0),
            c + egui::vec2(60.0, 8.0),
        );
        click_through(&mut app, &ctx, c);
        assert_eq!((band(&app), steps(&app)), before);
    }
}

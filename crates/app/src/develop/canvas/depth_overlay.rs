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

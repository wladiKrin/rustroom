//! The composition guides over the canvas.

use super::crop_overlay;
use super::view::Placement;
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use rustroom_app::tool::Tool;

/// The guides, dividing what will be the final photograph.
pub(super) fn draw(
    ui: &mut egui::Ui,
    s: &Session,
    tool: Tool,
    place: &Placement,
    app_guide: rustroom_app::composition_guides::CompositionGuide,
) {
    let Placement {
        img_rect,
        frame_rect,
        ..
    } = *place;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::canvas::test_support::screen;

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
                crate::develop::canvas::show(app, ui, &ctx2);
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
}

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

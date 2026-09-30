//! The loupe: a magnified crop of the rendered texture around the point being placed.
//!
//! ⚠⚠ **Shared by the guide and straighten overlays, and extracted rather than copied.**
//! `test_support.rs` states the rule for exactly this situation: "a helper duplicated per overlay
//! is how the two would drift." Two loupes would diverge in magnification, in placement and in
//! crosshair size, and nothing would fail.

use eframe::egui;

/// How wide the loupe is on screen, and how much it magnifies.
const LOUPE_SIZE: f32 = 168.0;
const LOUPE_MAG: f32 = 5.0;

/// A magnified crop of the rendered texture around `at`.
///
/// ⚠⚠ The sample window is taken in **`img_rect`** coordinates, not `frame_rect`: `img_rect` is
/// where the texture is actually painted, so this stays exact whether the frame was rendered
/// whole or as a region. That is the one place in this overlay where `img_rect` is the right
/// rect — everything else is frame-relative.
pub(super) fn draw_loupe(
    painter: &egui::Painter,
    tex: &egui::TextureHandle,
    at: egui::Pos2,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
) {
    if img_rect.width() <= f32::EPSILON || img_rect.height() <= f32::EPSILON {
        return;
    }
    // ⚠ Placed in the corner **furthest from the cursor**, so it never covers the thing being
    // traced. A loupe pinned beside the pointer hides exactly the edge you are aiming at.
    let pad = 12.0;
    let left = at.x > frame_rect.center().x;
    let top = at.y > frame_rect.center().y;
    let corner = egui::pos2(
        if left {
            img_rect.left() + pad
        } else {
            img_rect.right() - pad - LOUPE_SIZE
        },
        if top {
            img_rect.top() + pad
        } else {
            img_rect.bottom() - pad - LOUPE_SIZE
        },
    );
    let dest = egui::Rect::from_min_size(corner, egui::vec2(LOUPE_SIZE, LOUPE_SIZE));

    let uv_c = egui::pos2(
        (at.x - img_rect.left()) / img_rect.width(),
        (at.y - img_rect.top()) / img_rect.height(),
    );
    let half = egui::vec2(
        LOUPE_SIZE / (2.0 * LOUPE_MAG * img_rect.width()),
        LOUPE_SIZE / (2.0 * LOUPE_MAG * img_rect.height()),
    );
    let uv = egui::Rect::from_min_max(uv_c - half, uv_c + half);

    painter.rect_filled(dest, egui::CornerRadius::ZERO, egui::Color32::BLACK);
    painter.image(tex.id(), dest, uv, egui::Color32::WHITE);
    // ⚠ Crosshairs rather than a dot: a dot hides the very pixel being placed.
    let c = dest.center();
    let arm = 10.0;
    let mark = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 60, 60));
    painter.line_segment(
        [egui::pos2(c.x - arm, c.y), egui::pos2(c.x - 3.0, c.y)],
        mark,
    );
    painter.line_segment(
        [egui::pos2(c.x + 3.0, c.y), egui::pos2(c.x + arm, c.y)],
        mark,
    );
    painter.line_segment(
        [egui::pos2(c.x, c.y - arm), egui::pos2(c.x, c.y - 3.0)],
        mark,
    );
    painter.line_segment(
        [egui::pos2(c.x, c.y + 3.0), egui::pos2(c.x, c.y + arm)],
        mark,
    );
    painter.rect_stroke(
        dest,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(180)),
        egui::StrokeKind::Outside,
    );
}

/// Where the loupe should look: the point a live gesture is placing, else the pointer itself
/// while it is over the photograph.
///
/// ⚠⚠ **The `else` half is the whole reason this exists.** Both overlays originally showed the
/// loupe only while an endpoint was being *dragged*, which meant the one point you could not place
/// precisely was the **first** one — you pressed blind and corrected afterwards. Reported in use.
///
/// ⚠ `None` when the pointer is off the photograph: a loupe pinned in the corner magnifying
/// nothing is chrome, and worse, it goes stale rather than empty.
///
/// ⚠ `img_rect`, not `frame_rect`, for the reason [`draw_loupe`] itself records — it samples the
/// texture, and `img_rect` is where the texture is actually painted.
pub(super) fn loupe_at(
    ui: &egui::Ui,
    img_rect: egui::Rect,
    active: Option<egui::Pos2>,
) -> Option<egui::Pos2> {
    if active.is_some() {
        return active;
    }
    // ⚠ Read and release the input lock before anything else touches it. `Response::dragged_by`
    // takes the same lock internally, and a nested borrow panics in epaint's mutex the moment a
    // drag is live — the trap `pan_delta` already records.
    let p = ui.input(|i| i.pointer.hover_pos())?;
    img_rect.contains(p).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::canvas::test_support::crop_app;

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
                    screen_rect: Some(crate::develop::canvas::test_support::screen()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    crate::develop::canvas::show(app, ui, &c);
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        let idle = meshes_hovering(&mut app, &ctx, Some(centre));
        app.session.as_mut().expect("a session").show_guides = true;
        let guiding = meshes_hovering(&mut app, &ctx, Some(centre));
        assert_eq!(
            guiding,
            idle + 1,
            "expected one extra mesh — the loupe — in Guide Mode; got {guiding}, idle {idle}"
        );
    }
}

//! The pixel readout under the pointer.

use crate::Session;
use eframe::egui;

/// Sample the rendered frame under the pointer into `s.readout`.
pub(super) fn sample_hover(ui: &egui::Ui, s: &mut Session, img_rect: egui::Rect) {
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
        if img_rect.contains(p) && img_rect.width() > 0.0 && img_rect.height() > 0.0 {
            let u = (p.x - img_rect.left()) / img_rect.width();
            let v = (p.y - img_rect.top()) / img_rect.height();
            let x = ((u * *bw as f32) as usize).min(bw.saturating_sub(1));
            let y = ((v * *bh as f32) as usize).min(bh.saturating_sub(1));
            // ⚠ Computed into a local first: `bytes` is a shared borrow of `s`
            // and writing `s.readout` needs a mutable one. Never a clone of the
            // bytes.
            let sampled = rustroom_app::color_readout::mean_at(bytes, *bw, *bh, x, y);
            s.readout = sampled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        let (dark, _light) = seed_readout_bytes(&mut app);
        crate::develop::canvas::test_support::hover_through(&mut app, &ctx, centre);
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        let (dark, light) = seed_readout_bytes(&mut app);

        crate::develop::canvas::test_support::hover_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 0.0),
        );
        let left = readout(&app);
        crate::develop::canvas::test_support::hover_through(
            &mut app,
            &ctx,
            centre + egui::vec2(60.0, 0.0),
        );
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        seed_readout_bytes(&mut app);
        crate::develop::canvas::test_support::hover_through(&mut app, &ctx, centre);
        assert!(
            !readout(&app).is_empty(),
            "fixture check: nothing was read, so this test proves nothing"
        );
        crate::develop::canvas::test_support::hover_away(&mut app, &ctx);
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
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        seed_readout_bytes(&mut app);
        let before = {
            let s = app.session.as_ref().expect("a session");
            (
                s.engine.requests_issued(),
                s.history.can_undo(),
                s.edits.clone(),
            )
        };
        crate::develop::canvas::test_support::hover_through(&mut app, &ctx, centre);
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

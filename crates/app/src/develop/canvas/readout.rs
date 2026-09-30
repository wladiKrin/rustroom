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

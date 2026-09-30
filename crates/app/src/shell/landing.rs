//! What each frame lands: a rendered frame into the session, thumbnails into the grid, capture
//! metadata into the catalog.

use eframe::egui;

/// The clipping overlay for a displayed frame, or `None` when nothing clips.
pub(crate) fn clipping_texture(
    ctx: &egui::Context,
    rgba: &[u8],
    w: usize,
    h: usize,
) -> Option<egui::TextureHandle> {
    let overlay = rustroom_app::clipping::overlay(rgba, w, h, 4);
    if rustroom_app::clipping::is_empty(&overlay) {
        return None;
    }
    let img = egui::ColorImage::from_rgba_premultiplied([w, h], &overlay);
    Some(ctx.load_texture("clipping", img, egui::TextureOptions::NEAREST))
}

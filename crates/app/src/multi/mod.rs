//! The two multi-photo culling views: **Compare** (X|Y) and **Survey** (N).
//!
//! Choosing between two nearly identical frames is what a large shoot spends its time on, and
//! the grid can only do it by clicking back and forth and comparing from memory. These two views
//! put the candidates on screen together.
//!
//! ⚠⚠ **Both draw from the thumbnail cache, never from a decode.** A RAW decode is ~1 s and
//! 78 MB, and Survey can put two dozen photos on screen; thumbnails are already
//! *edit-accurate* — `ThumbKey` carries a fingerprint of the edits — so a black-and-white
//! virtual copy already shows in black and white. The consequence, stated so it is not later
//! found as a bug: **neither view can judge sharpness at 1:1**, which would need real decodes.
//!
//! ⚠⚠ **Both go through `Library::sync_thumb_window`, exactly as the grid and the filmstrip do.**
//! `retain_window` **pins** what is on screen and never evicts it, so a view that drew from the
//! cache without telling it what it is drawing would show grey cells for photos it is displaying
//! *and* let them be evicted underneath itself. That is why every draw ends with a want-list.
//!
//! ⚠ **One window at a time.** These views replace the grid and never appear beside it or beside
//! the filmstrip, which is what keeps a single overwriting window correct. If that ever changes
//! they must contribute to a *union* rather than each writing the whole window — the note
//! `thumb_window` already carries.
//!
//! The rect arithmetic is in the lib (`rustroom_app::tile_layout`), so the tiling can be tested
//! without a window; only the drawing and the egui wiring are here.

pub mod compare;
pub(crate) mod pane;
pub mod survey;

use crate::thumb;
use eframe::egui;

/// The gap between tiles, and the inset from the viewport's edge.
const GAP: f32 = 10.0;

/// How tall the caption strip under a photo is.
const CAPTION_H: f32 = 20.0;

/// Draw one photo into `rect`: the image fitted and centred, a caption strip under it, its
/// marks, and a border when it is the focused tile.
///
/// ⚠ Painted into the rect it is handed rather than allocated through the layout cursor —
/// `tile_layout` has already decided where every tile goes, and re-deriving that from egui's
/// cursor is how the two would come to disagree.
///
/// Returns the tile's response so the caller can add whatever interaction it wants; this
/// function senses `click` only, following `thumb::cell`'s rule that painting and interaction
/// are separate concerns.
pub(crate) fn photo_tile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    tex: Option<&egui::TextureHandle>,
    caption: &str,
    marks: catalog::db::Marks,
    focused: bool,
) -> egui::Response {
    tile(ui, rect, id, TileImage::Thumb(tex), caption, marks, focused)
}

/// The part of a tile the photo occupies: everything above the caption strip.
///
/// ⚠ Public because Compare needs it *before* drawing — a pane picks its render rung from the
/// area it will be drawn into, and re-deriving that from the tile rect in two places is how the
/// two would come to disagree about where the picture goes.
pub(crate) fn image_area(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.max.x, (rect.max.y - CAPTION_H).max(rect.min.y)),
    )
}

/// What goes in a tile's image area.
///
/// ⚠⚠ **One chrome, two pictures.** A Survey tile shows a thumbnail fitted to it; a Compare pane
/// shows a real render at whatever zoom and pan the two panes share. The caption, the marks and
/// the focus ring are identical, and duplicating them per view is how the two would drift.
pub(crate) enum TileImage<'a> {
    /// A thumbnail, fitted and centred — magnified up to [`MAX_UPSCALE`] to fill the tile.
    Thumb(Option<&'a egui::TextureHandle>),
    /// A rendered frame at an explicit scale and offset, clipped to the tile.
    ///
    /// ⚠ Clipped rather than fitted: at zoom the picture is *meant* to overflow, and that is what
    /// panning moves around inside.
    Pane {
        tex: &'a egui::TextureHandle,
        scale: f32,
        offset: egui::Vec2,
    },
}

/// How far a thumbnail may be magnified to fill its tile.
///
/// ⚠⚠ **Not 1.0, and that was the bug.** `photo_tile` originally refused to upscale at all —
/// `thumb::cell`'s rule, so a grid thumbnail is never magnified into softness. But a Survey of
/// three photos gives each a tile several hundred points across, and a 512 px thumbnail then sat
/// small in the middle of it with grey all round. Reported as "the images are quite small".
///
/// ⚠ A survey is judged for **composition and expression**, not for sharpness — that is what
/// Compare is for, and Compare draws from real decodes rather than from this. So filling the tile
/// is the right trade here and would be the wrong one in the grid. 2.0 rather than unbounded
/// because past it a 512 px thumbnail is more artifact than photograph.
const MAX_UPSCALE: f32 = 2.0;

pub(crate) fn tile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    image: TileImage<'_>,
    caption: &str,
    marks: catalog::db::Marks,
    focused: bool,
) -> egui::Response {
    let resp = ui.interact(rect, id, egui::Sense::click());
    let p = ui.painter_at(rect);

    // The photo gets everything above the caption strip.
    let img = image_area(rect);
    p.rect_filled(
        rect,
        egui::CornerRadius::same(3),
        egui::Color32::from_gray(24),
    );
    match image {
        TileImage::Thumb(Some(t)) => {
            // Fit, then centre. Magnified up to `MAX_UPSCALE` so a large tile is filled rather
            // than holding a small photo in a field of grey — see that constant for the trade.
            let size = t.size_vec2();
            if size.x > 0.0 && size.y > 0.0 && img.width() > 0.0 && img.height() > 0.0 {
                let scale = (img.width() / size.x)
                    .min(img.height() / size.y)
                    .min(MAX_UPSCALE);
                egui::Image::new(t)
                    .paint_at(ui, egui::Rect::from_center_size(img.center(), size * scale));
            }
        }
        TileImage::Pane { tex, scale, offset } => {
            let size = tex.size_vec2();
            if size.x > 0.0 && size.y > 0.0 && img.width() > 0.0 && img.height() > 0.0 {
                // ⚠⚠ **Clipped explicitly.** At zoom the picture is larger than the pane, and
                // `paint_at` honours the `Ui`'s clip rect rather than its layout rect — so
                // without `set_clip_rect` the zoomed photo spills across the divider and over
                // its neighbour, which is the one thing a comparison view must never do.
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(img));
                child.set_clip_rect(img);
                egui::Image::new(tex).paint_at(
                    &child,
                    egui::Rect::from_center_size(img.center() + offset, size * scale),
                );
            }
        }
        TileImage::Thumb(None) => {
            // ⚠ A placeholder, not nothing: a tile that vanished while its thumbnail loaded
            // would make the whole layout look broken rather than merely unfinished.
            p.rect_filled(
                img,
                egui::CornerRadius::same(3),
                egui::Color32::from_gray(48),
            );
        }
    }

    // ⚠ The badge size is derived from the *image* area rather than the whole tile, and clamped
    // by `paint_marks` — a Survey tile can be several hundred points tall and a star scaled to
    // match would be absurd.
    thumb::paint_marks(&p, img, marks, img.height() * 0.05);

    if !caption.is_empty() && rect.height() > CAPTION_H {
        p.text(
            egui::pos2(rect.center().x, rect.max.y - CAPTION_H * 0.5),
            egui::Align2::CENTER_CENTER,
            caption,
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
    }

    // ⚠ The focus ring is the only thing saying which tile a culling key will act on, so it is
    // drawn last, over the photo, rather than under it.
    if focused {
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(3),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 160, 240)),
            egui::StrokeKind::Inside,
        );
    }
    resp
}

/// The viewport a set of tiles is laid out in: the available area, inset by [`GAP`].
pub(crate) fn stage(ui: &egui::Ui) -> egui::Rect {
    ui.available_rect_before_wrap().shrink(GAP)
}

/// Turn a [`rustroom_app::tile_layout::Tile`] — viewport-relative — into a screen rect.
pub(crate) fn tile_rect(stage: egui::Rect, t: rustroom_app::tile_layout::Tile) -> egui::Rect {
    egui::Rect::from_min_size(
        stage.min + egui::vec2(t.x, t.y),
        egui::vec2(t.w.max(0.0), t.h.max(0.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_lands_where_the_layout_put_it() {
        // ⚠ The one thing that can go wrong at this seam: `tile_layout` works in
        // viewport-relative coordinates and egui in screen ones, so the origin has to be added
        // exactly once. Adding it twice or not at all is invisible on a stage that starts at
        // (0, 0), which is why this one deliberately does not.
        let stage = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(800.0, 600.0));
        let t = rustroom_app::tile_layout::Tile {
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0,
        };
        let r = tile_rect(stage, t);
        assert_eq!(r.min, egui::pos2(110.0, 60.0));
        assert_eq!(r.size(), egui::vec2(300.0, 200.0));
    }

    #[test]
    fn every_tile_stays_inside_the_stage() {
        // The property `tile_layout` guarantees, re-checked *after* the coordinate conversion —
        // that conversion is the only place it could be lost.
        let stage = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(800.0, 600.0));
        for n in 1..=rustroom_app::tile_layout::MAX_TILES {
            for t in rustroom_app::tile_layout::survey(n, 800.0, 600.0, GAP) {
                let r = tile_rect(stage, t);
                assert!(
                    stage.contains_rect(r.shrink(0.01)),
                    "n = {n}: {r:?} escapes {stage:?}"
                );
            }
        }
    }
}

//! Painting the canvas: the frame and its stand-ins, the backdrop, the clipping and grade views,
//! and the pending and frameless states.

use super::navigation::pan_delta;
use super::view::{Placement, ViewPlan};
use crate::{PendingOpen, Session};
use eframe::egui;
use rustroom_app::thumb_key::ThumbKey;
use rustroom_app::view_region;

/// The preview textures and the misses, borrowed from `app` beside the session.
#[derive(Clone, Copy)]
pub(super) struct Previews<'a> {
    pub(super) textures: &'a rustroom_app::thumb_cache::ThumbCache<(
        egui::TextureHandle,
        rustroom_app::preview_loader::PreviewSource,
    )>,
    pub(super) misses: &'a std::collections::HashSet<ThumbKey>,
}

/// Allocate the image and paint what stands for it: a preview, the stale frame, or the frame itself —
/// over the backdrop, under the clipping and grade views. Returns the image's response.
pub(super) fn frame(
    ui: &mut egui::Ui,
    s: &Session,
    shown: &egui::TextureHandle,
    plan: &ViewPlan,
    place: &Placement,
    pv: Previews,
    stand_in_key: ThumbKey,
) -> egui::Response {
    let ViewPlan {
        full,
        view_is_plain,
        needed_long,
        ..
    } = *plan;
    let Placement {
        place_region,
        img_rect,
        frame_rect,
        ..
    } = *place;
    let Previews {
        textures: preview_textures,
        misses: preview_misses,
    } = pv;
    // M9.14: the preview stands in where the live buffer cannot supply the
    // pixels asked for — the rule is `stand_in::prefer_preview`; the key check
    // is that the texture is of the edits on screen, never the before image.
    let stand_in = if s.buffer_is_full_res || !view_is_plain {
        None
    } else {
        preview_textures
            .get(&stand_in_key)
            .filter(|(t, _)| {
                let long = t.size()[0].max(t.size()[1]);
                rustroom_app::stand_in::prefer_preview(needed_long, s.buffer_long, long)
            })
            .cloned()
    };

    let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
    // ⚠⚠ **The backdrop, painted at `frame_rect` UNDER the region texture.** A
    // region render covers only `img_rect`; everything outside it used to be
    // unpainted, so panning revealed black rectangles that snapped to image when
    // the next region landed. The last whole-image texture covers the entire
    // frame, so a revealed edge is a soft but correct picture that snaps sharp.
    //
    // ⚠ Only while a region is on screen. With a whole-image frame the two would
    // coincide exactly and the backdrop would be a wasted draw of the same pixels.
    //
    // ⚠ Only when its `fit_size` still matches: a crop or an orientation change
    // moves the frame, and a backdrop from before it would be stretched across
    // the new one at the wrong scale — visible, because it spans the whole rect.
    if place_region.is_some() {
        if let Some((tex, size)) = &s.backdrop {
            if (size.0 - full.x).abs() < 0.5 && (size.1 - full.y).abs() < 0.5 {
                egui::Image::new(tex).paint_at(ui, frame_rect);
            }
        }
    }
    // ⚠ A ghost whose preview has not answered yet shows the previous
    // photo dimmed, never its own 512-px render — the author called that a
    // low-resolution flash (2026-09-29). Once the loader has said there is
    // no preview (an edited photo without one), the ghost's render is all
    // there is and it shows.
    let waiting =
        s.buffer_is_ghost && stand_in.is_none() && !preview_misses.contains(&stand_in_key);
    match &stand_in {
        // ⚠ At `frame_rect`, the whole post-geometry frame: a camera JPEG
        // is the camera's framing, which for an untouched photo is the frame;
        // a rendered preview is the frame exactly.
        Some((tex, source)) => {
            egui::Image::new(tex).paint_at(ui, frame_rect);
            let label = match source {
                rustroom_app::preview_loader::PreviewSource::Camera => "Camera preview",
                rustroom_app::preview_loader::PreviewSource::Rendered => "Preview",
            };
            let at = frame_rect.min + egui::vec2(10.0, 8.0);
            // ⚠ Caption, not a literal 12 — found once the font-size sweep
            // stopped at test items rather than at the file's first one.
            let font = egui::FontId::proportional(crate::theme::tokens::CAPTION);
            let galley =
                ui.painter()
                    .layout_no_wrap(label.to_string(), font.clone(), egui::Color32::WHITE);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0)),
                egui::CornerRadius::same(3),
                egui::Color32::from_black_alpha(140),
            );
            ui.painter().galley(at, galley, egui::Color32::WHITE);
        }
        None if waiting => {
            paint_stale(ui, s.stale.as_ref());
        }
        None => {
            egui::Image::new(shown).paint_at(ui, resp.rect);
        }
    }
    // ⚠ The same rectangle as the frame it was computed from — `J`.
    if let Some(clip) = &s.clip_texture {
        egui::Image::new(clip).paint_at(ui, resp.rect);
    }
    // ⚠ The grade range view, over the photograph and under every tool overlay.
    // `resp.rect` is `img_rect` — the rendered slice — and this texture covers
    // exactly that, whether the frame was whole or a region; the readout's
    // exception, for the readout's reason. Every other overlay is frame-relative.
    if let Some(tint) = &s.grade_view_texture {
        egui::Image::new(tint).paint_at(ui, resp.rect);
    }
    resp
}

/// No frame yet: the preview at the session's view when it has landed, else the stale frame.
pub(super) fn no_frame_yet(
    ui: &mut egui::Ui,
    s: &Session,
    preview_textures: &rustroom_app::thumb_cache::ThumbCache<(
        egui::TextureHandle,
        rustroom_app::preview_loader::PreviewSource,
    )>,
) {
    // No frame yet — a ghost session's first render is a few ms away. The
    // preview, when it has landed, fills those frames at the session's view.
    let key = rustroom_app::thumb_key::ThumbKey::new(
        rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
        &s.edits,
    );
    match preview_textures.get(&key) {
        Some((tex, _)) => {
            paint_fitted(ui, tex, s.zoom, s.pan);
        }
        None => paint_stale(ui, s.stale.as_ref()),
    }
}

/// Canvas while a decode is in flight: the target photo's thumbnail if we have one (scaled
/// up to fill, since thumbnails are ≤512 px), else the previous photo dimmed, else a spinner.
pub(super) fn draw_pending_canvas(
    ui: &mut egui::Ui,
    pending: &mut PendingOpen,
    session: Option<&Session>,
    preview: Option<&egui::TextureHandle>,
    missed: bool,
    pan_binding: rustroom_app::settings::PanDrag,
) {
    // ⚠ The 512-px placeholder only once the loader has said there is no preview; while one
    // may be on its way, the previous photo's frame, dimmed.
    let placeholder = pending.placeholder.as_ref().filter(|_| missed);
    let showing_placeholder = placeholder.is_some() || preview.is_some();
    let fallback = session.and_then(|s| s.texture.as_ref());
    let Some(tex) = preview.or(placeholder).or(fallback) else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    };

    // The same placement the session will use: the picture fitted to the canvas, times the
    // zoom, about the centre plus the pan — so the swap happens in place instead of snapping.
    let tex_size = tex.size_vec2();
    let avail = ui.available_size();
    let fit = (avail.x / tex_size.x.max(1.0)).min(avail.y / tex_size.y.max(1.0));
    pending.fit_long = tex_size.x.max(tex_size.y) * fit;
    let centre = ui.max_rect().center();
    let rect = egui::Rect::from_center_size(centre + pending.pan, tex_size * fit * pending.zoom);
    let resp = ui.allocate_rect(rect, egui::Sense::drag());

    // Zoom about the cursor, then pan — the session canvas's rule, on the pending view.
    let zoom_factor = ui.input(|i| i.zoom_delta());
    if zoom_factor != 1.0 && resp.hovered() {
        let old = pending.zoom;
        pending.zoom = view_region::zoom_step(old, zoom_factor);
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            let anchor = centre + pending.pan;
            let (dx, dy) =
                view_region::zoom_anchor_shift((p.x, p.y), (anchor.x, anchor.y), old, pending.zoom);
            pending.pan += egui::vec2(dx, dy);
        }
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta);
    let space_down = ui.input(|i| i.key_down(egui::Key::Space));
    let text_focused = ui.ctx().memory(|m| m.focused().is_some());
    pending.pan += pan_delta(&resp, scroll, space_down, pan_binding, text_focused);
    if pending.zoom <= view_region::MIN_ZOOM {
        pending.pan = egui::Vec2::ZERO;
    }
    let rect = paint_fitted(ui, tex, pending.zoom, pending.pan);

    if !showing_placeholder {
        // This is the *previous* photo — dim it so it reads as stale.
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(140),
        );
    }
}
/// Paint a texture fitted to the canvas, times `zoom`, about the centre plus `pan` — the
/// placement a session frame gets, so a swap happens in place instead of snapping.
fn paint_fitted(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Rect {
    let tex_size = tex.size_vec2();
    let avail = ui.available_size();
    let fit = (avail.x / tex_size.x.max(1.0)).min(avail.y / tex_size.y.max(1.0));
    let rect = egui::Rect::from_center_size(ui.max_rect().center() + pan, tex_size * fit * zoom);
    egui::Image::new(tex).paint_at(ui, rect);
    rect
}
/// The previous photo's last frame, fitted and dimmed so it reads as stale — or a spinner on
/// a first open, when there is none.
fn paint_stale(ui: &mut egui::Ui, stale: Option<&egui::TextureHandle>) {
    match stale {
        Some(tex) => {
            let rect = paint_fitted(ui, tex, 1.0, egui::Vec2::ZERO);
            // ⚠ A light dim (2026-09-29): at 140 the previous frame read as "a flash of a black
            // image" for the fraction of a second a preview takes. Lightly dimmed it reads as a
            // crossfade, and with the neighbours fetched ahead it is rarely seen at all.
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha(48),
            );
        }
        None => {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        }
    }
}

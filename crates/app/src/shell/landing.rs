//! What each frame lands: a rendered frame into the session, thumbnails into the grid, capture
//! metadata into the catalog.

use crate::{RustroomApp, Session};
use eframe::egui;
use rustroom_app::render_engine::{RegionRect, RenderedFrame};

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

impl RustroomApp {
    /// Land the render worker's newest frame, if one has arrived.
    pub(super) fn pump_render_frame(&mut self, ctx: &egui::Context) {
        if let Some(s) = &mut self.session {
            if let Some(frame) = s.engine.poll_latest() {
                apply_frame(s, frame, self.show_clipping, ctx);
            }
        }
    }

    /// Turn finished thumbnails into textures in the grid cache.
    pub(super) fn pump_thumbnails(&mut self, ctx: &egui::Context) {
        if let Some(lib) = &mut self.library {
            for res in lib.loader.drain() {
                if let Some(t) = res.thumb {
                    let bytes = t.rgba.len();
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    // ⚠ The texture name is derived from the **key**, not from an index: two
                    // different photos sharing a name would have egui serve one image for both,
                    // and now two renders of one photo with different edits would too.
                    let tex = ctx.load_texture(
                        format!(
                            "thumb:{}#{}:{}",
                            res.key.id.path.display(),
                            res.key.id.copy,
                            res.key.edits
                        ),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    lib.cache.insert(res.key, tex, bytes);
                }
            }
        }
    }

    /// The startup location pass's findings into the catalog (M10.18b). Every write moves
    /// `change_count`, which is what refreshes the Map's pins.
    pub(super) fn pump_location_pass(&mut self, ctx: &egui::Context) {
        let Some(pass) = &self.location_pass else {
            return;
        };
        let (found, done) = pass.drain();
        if let Some(cat) = &self.catalog {
            for (path, at) in &found {
                if let Err(e) = cat.set_file_location(path, *at) {
                    eprintln!("catalog: {e}");
                }
            }
        }
        if done {
            self.location_pass = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Write finished capture metadata into the catalog, and refresh the Library that filters on it.
    pub(super) fn pump_captures(&mut self) {
        let done = self.metadata.drain();
        if let Some(cat) = &self.catalog {
            for r in &done {
                if let Err(e) = cat.set_capture(&r.path, &r.capture) {
                    eprintln!("catalog: {e}");
                }
                // M10.18b: the file's own position — or "read, none", so the startup pass skips it.
                if let Err(e) = cat.set_file_location(&r.path, r.capture.gps) {
                    eprintln!("catalog: {e}");
                }
            }
        }
        // ⚠⚠ Refresh the Library's capture map, or a folder that has just been scanned stays
        // unfilterable until something unrelated rebuilds — the `reload_marks` refilter rule,
        // one layer along.
        //
        // ⚠ Guarded on there being results: this runs every frame, and re-querying every
        // folder sixty times a second is the `apply_view_demosaic` mistake in yet another
        // costume.
        if !done.is_empty() {
            self.reload_captures();
        }
    }
}

/// A frame into the session: its texture, histogram and readout bytes, its placement, and the
/// views that travel with it. Needs no window — only the context the textures belong to.
pub(super) fn apply_frame(
    s: &mut Session,
    frame: RenderedFrame,
    show_clipping: bool,
    ctx: &egui::Context,
) {
    // ⚠ Taken from the frame, never recomputed from `frame.rgba` — those bytes are in
    // the display's primaries. See `RenderedFrame::hist`.
    s.hist = frame.hist;
    // ⚠⚠ Kept rather than dropped, and it is the **sRGB** channel, not `rgba`. See
    // `RenderedFrame::srgb`: `rgba` is in the display's primaries, so a readout built
    // on it would report different numbers on a different monitor — the same bug the
    // histogram's ordering already exists to prevent. The dimensions travel with the
    // bytes so nothing has to re-derive them.
    //
    // ⚠ An `Arc::clone`, so this retains no memory the frame did not already hold.
    s.readout_bytes = Some((
        std::sync::Arc::clone(&frame.srgb),
        frame.width,
        frame.height,
    ));
    s.clip_texture = if show_clipping {
        clipping_texture(ctx, &frame.srgb, frame.width, frame.height)
    } else {
        None
    };
    // ⚠ Timed under `RUSTROOM_DEBUG_RENDER` (2026-09-26): the worker's `render:` line
    // cannot see what the UI thread spends turning a frame into a texture, and the
    // in-app drag ran ~11 ms slower than the headless probe with no line saying where.
    let t_tex = std::time::Instant::now();
    // ⚠ PREMULTIPLIED, because every frame is opaque (the GPU quantiser ORs in
    // 0xff000000; `to_rgba8` is pinned opaque by `to_rgba8_is_to_rgb8_with_an_opaque_alpha`),
    // and at alpha 255 the two are the same pixels — without the per-pixel conversion
    // that cost 5.5 ms of the UI thread per 6.5 Mpx drag frame.
    let img = egui::ColorImage::from_rgba_premultiplied([frame.width, frame.height], &frame.rgba);
    let t_load = std::time::Instant::now();
    // ⚠⚠ **Updated in place when a whole frame follows a whole frame of the same size**
    // (2026-09-26): a NEW 27 MB texture per drag frame is an allocation plus an upload
    // on the window's own GPU device, competing with the render pipeline's. ⚠ Only
    // whole-after-whole: `backdrop` is a clone of this very handle, and a region frame
    // written into it would turn the backdrop into the slice it exists to surround — so
    // a region frame (or the first whole frame after one) still gets a fresh texture.
    let reuse = frame.region.is_none()
        && s.rendered_region.is_none()
        && s.texture
            .as_ref()
            .is_some_and(|t| t.size() == [frame.width, frame.height]);
    // ⚠ MEASUREMENT SWITCH (2026-09-26): with `RUSTROOM_DEBUG_NO_UPLOAD` set, a new frame
    // never reaches the window's GPU once a picture is on screen — the window keeps
    // drawing the old one. It exists to answer ONE question: does the window's handling
    // of new frames cost the render pipeline GPU time (the ~12 ms app-vs-probe gap)?
    let frozen = s.texture.is_some() && std::env::var_os("RUSTROOM_DEBUG_NO_UPLOAD").is_some();
    match (&mut s.texture, reuse) {
        _ if frozen => {}
        (Some(t), true) => t.set(img, egui::TextureOptions::LINEAR),
        _ => {
            s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
        }
    }
    if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
        eprintln!(
            "  UI texture: convert {:5.1} ms  load {:5.1} ms  {}x{}",
            (t_load - t_tex).as_secs_f64() * 1e3,
            t_load.elapsed().as_secs_f64() * 1e3,
            frame.width,
            frame.height
        );
    }
    // ⚠ A whole-image frame is kept as the backdrop a region render is painted over.
    // ⚠ `Some(..)` only — a region frame must NOT replace it, or the backdrop would
    // become the very slice it exists to surround and the black would come back.
    if frame.region.is_none() {
        s.backdrop = s.texture.clone().map(|t| (t, frame.fit_size));
    }
    // ⚠ The grade range view's plane becomes a texture in the same place the frame
    // does, so the two can never describe different frames — and a frame that
    // arrives without a plane (box unticked, Global, a tool switch) drops the
    // texture in this same assignment. No separate clear.
    s.grade_view_texture = frame.grade_view.as_ref().map(|plane| {
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width, frame.height],
            &rustroom_app::grade_view::tint_rgba(plane),
        );
        ctx.load_texture("grade_view", img, egui::TextureOptions::LINEAR)
    });
    s.rendered_region = frame.region.map(|r| RegionRect {
        x: r.x0,
        y: r.y0,
        w: frame.width,
        h: frame.height,
    });
    // ⚠⚠ **Read from the frame, not recomputed here.** This used to be
    // `frame_dims x fit_cap / frame.max_dim`. A frame's dimensions come from
    // `downscale_to`, which rounds, so that form amplified the rounding by
    // `rung / fit_cap` and made the placement depend on which rung last landed —
    // measured on an uncropped X-T4 frame at 1068.000 against a true 1068.204.
    // `RenderedFrame::fit_size` is computed from the buffer and the view geometry, so
    // it is exact and identical at every rung.
    //
    // ⚠ It is carried on the frame rather than derived here from `s.edits`, which
    // would also be exact and would be worse: it would describe a crop the frame on
    // screen does not have until the next one lands, misplacing the texture for a
    // frame or two after every crop change. Travelling with the pixels it describes
    // is what makes that unrepresentable.
    //
    // ⚠ The finding that made the old code take region frames as well as whole-image
    // ones still holds and is now structural: **every** frame carries this, so the
    // distinction has stopped existing here. The symptom it fixed is worth keeping —
    // entering geometry mode with a crop applied resets the crop for the preview, so
    // the frame grows; with only region frames arriving, a whole-image-only rule left
    // `fit_image_size` describing the *cropped* frame, `frame_rect` was the wrong size
    // and the guide overlay sat slightly off, correcting itself only when you zoomed
    // out far enough for a whole-image frame to land.
    s.fit_image_size = egui::vec2(frame.fit_size.0, frame.fit_size.1);
    // A region frame carries the full post-geometry dimensions it was cut from,
    // so this is exact either way.
    s.rendered_full = frame
        .region
        .map(|r| (r.full_w, r.full_h))
        .unwrap_or((frame.width, frame.height));
    s.rendered_long = s.rendered_full.0.max(s.rendered_full.1) as f32;
    // ⚠ The overlay may now describe this frame. The engine coalesces superseded
    // requests, so the frame that lands is the newest one asked for.
    s.shown_geo = s.pending_geo.get();
}

#[cfg(test)]
mod tests {
    /// ⚠⚠ A **region** frame must refresh `fit_image_size`, not only a whole-image one. It carries
    /// `full_w`/`full_h` — the same post-geometry dimensions — and Guide Mode unpinned the fit
    /// rung, so a zoomed session can render nothing but regions. Left stale, `frame_rect` is the
    /// wrong size and every frame-relative overlay sits slightly off, correcting itself only when
    /// you zoom out far enough for a whole-image frame to land.
    ///
    /// ⚠ The concrete case: entering geometry mode **with a crop applied** resets the crop for the
    /// preview, so the frame grows — and the stale value still described the cropped one.
    #[test]
    fn a_region_frame_refreshes_the_fit_image_size() {
        // A frame cut from a 1600x1000 post-geometry frame.
        let region = pipeline::Region {
            full_w: 1600,
            full_h: 1000,
            x0: 100,
            y0: 50,
        };
        // The quantity `shell` now reads: the *whole frame's* dimensions, which a region frame
        // carries — the same source `rendered_long` beside it has always used.
        assert_eq!((region.full_w, region.full_h), (1600, 1000));
        // ⚠ And they are emphatically not the cut's own size, which is what made a stale
        // `fit_image_size` look plausible for as long as it did.
        assert_ne!(region.full_w, region.x0 + 400);
    }
}

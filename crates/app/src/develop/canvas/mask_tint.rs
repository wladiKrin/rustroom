//! The brush coverage tint: a memoised, resolution-capped rasterization uploaded as one
//! texture, plus the surround an inverted brush needs.
//!
//! ⚠ Its own module because the memo, its cache key and the box-versus-surround painting are a
//! self-contained concern — `mask_overlay` only ever calls `paint_tint`.

use eframe::egui;

/// The largest edge the tint's coverage is rasterized at, whatever the zoom.
///
/// ⚠ The tint is an indicator over the photograph, not the photograph, so it does not need the
/// frame's real resolution — and at 1:1 the frame is the full decode. Capping makes the cost
/// constant in zoom.
const TINT_MAX_DIM: usize = 1400;

/// A memoised tint, rasterized at a capped resolution and held as an uploaded **texture**.
///
/// ⚠⚠ **A mesh is the wrong representation and it crashed the app.** The first version emitted
/// one quad per run of equal coverage along a row. That collapses a stroke's flat interior, but
/// across a *soft* brush's gradient almost every pixel differs from its neighbour, so every run
/// is length 1 — and a large soft stroke is mostly gradient. wgpu refused a 711 MB vertex buffer
/// (~7 million quads) against a 268 MB limit. One textured quad is O(1) geometry whatever the
/// stroke.
///
/// ⚠ `paint_mask` runs on every egui frame, so without this memo the tint would re-rasterize
/// *and* re-upload at ~60 Hz while merely hovering. One stroke is ~1.2 ms at screen size, so
/// thirty strokes was ~36 ms per frame with nothing happening — the "sluggish moving the brush
/// around" reported in use. The benchmark could not have found it: it timed one stroke once,
/// while the real cost scales with stroke count and is paid continuously.
struct TintCache {
    brush: edit_model::Brush,
    geometry: edit_model::Geometry,
    w: usize,
    h: usize,
    /// The uploaded coverage, with its box in **coverage-pixel** coordinates and the scale it was
    /// rasterized at. `None` when the brush paints nothing inside its box.
    ///
    /// ⚠⚠ **A screen `Rect` must NOT be cached here.** It was, and panning at zoom then left the
    /// tint fixed in window coordinates while the image slid under it, snapping back the moment a
    /// brush stroke invalidated the entry. That is precisely 3.1a's second real-use bug — an
    /// overlay placed from a stale rect rather than recomputed against the live `frame_rect` —
    /// reintroduced through a cache. Placement is derived every frame; only the texture is kept.
    drawn: Option<(egui::TextureHandle, [usize; 4], f32)>,
    /// Which displayed frame the gate was computed against, as that buffer's own address.
    ///
    /// ⚠⚠ **An identity, never a screen rect.** M3.1c-B's sixth bug was a cached screen rect
    /// surviving a pan, itself a regression of 3.1a's overlay drift: *cache the expensive thing,
    /// never the placement.* A pointer changes when a new frame lands and does not change when the
    /// view is panned, which is exactly the distinction wanted.
    base: Option<*const u8>,
    /// Coverage outside the box — non-zero for an **inverted** brush, which covers the whole
    /// frame except what was painted.
    ///
    /// ⚠ Without this the tint showed the inversion only inside a rectangle around the strokes,
    /// which grew as more was painted. Found in use, and the same omission the coverage map
    /// itself had.
    outside: u8,
}

thread_local! {
    /// ⚠ Thread-local rather than a field on `RustroomApp`: `paint_mask` is a free function
    /// reached from inside egui closures that already hold `&mut app`, and threading a cache down
    /// would widen four signatures for something with no observable behaviour. egui is
    /// single-threaded per context, so this is the same thread every frame.
    static TINT: std::cell::RefCell<Option<TintCache>> = const { std::cell::RefCell::new(None) };
}

/// Paint the brush's coverage as a translucent red tint.
///
/// The texture is rebuilt only when the strokes, geometry or screen size change.
/// The displayed frame, as the tint's gate needs to read it.
///
/// `bytes` is RGBA8 of `bw` x `bh` **rendered** pixels whose top-left sits at `origin` in the full
/// rendered frame `full`.
pub(super) struct TintFrame {
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub bw: usize,
    pub bh: usize,
    pub origin: (usize, usize),
    pub full: (usize, usize),
}

/// Samples the displayed frame at **tint-grid** coordinates.
///
/// ⚠⚠ **Three scales are in play and conflating any two is a silent bug**: the tint's own capped
/// grid (`tw` x `th`), the full rendered frame, and the readout buffer, which at zoom holds only
/// the rendered *region* of it. The bridge is the frame fraction, which the tint grid and the
/// rendered frame share by construction because both are proportional to `frame_rect`.
///
/// ⚠ **This is the displayed frame — post-mask, 8-bit, display-encoded — where the render's gate
/// compares the pre-mask working-space buffer.** So the tint is an approximation of the render,
/// not a preview of it. It is self-consistent, because *both* ends of its comparison come from
/// this same buffer; the gap against the render is the accepted caveat, and it is why the tint is
/// feedback rather than the photograph.
struct TintBase<'a> {
    f: &'a TintFrame,
    tw: usize,
    th: usize,
}

impl pipeline::brush::BasePixels for TintBase<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let (fw, fh) = self.f.full;
        if self.tw == 0 || self.th == 0 || fw == 0 || fh == 0 || self.f.bw == 0 || self.f.bh == 0 {
            return [0.0; 3];
        }
        // Tint grid -> frame fraction -> rendered pixel -> this buffer's own pixel.
        let rx = (x as f32 + 0.5) / self.tw as f32 * fw as f32;
        let ry = (y as f32 + 0.5) / self.th as f32 * fh as f32;
        // ⚠ Clamped rather than answering black, for `BaseView`'s reason: a dab kept because its
        // disc overlaps can have its centre just outside, and the nearest real pixel is far closer
        // to the truth than black.
        let px = (rx as isize - self.f.origin.0 as isize).clamp(0, self.f.bw as isize - 1) as usize;
        let py = (ry as isize - self.f.origin.1 as isize).clamp(0, self.f.bh as isize - 1) as usize;
        let i = (py * self.f.bw + px) * 4;
        let Some(c) = self.f.bytes.get(i..i + 3) else {
            return [0.0; 3];
        };
        // ⚠ Decoded into the working space's own primaries, because `BasePixels` promises linear
        // ProPhoto — the rasterizer converts what it is given with `prophoto_to_oklab`.
        color::srgb_to_prophoto([
            color::srgb_to_linear(c[0] as f32 / 255.0),
            color::srgb_to_linear(c[1] as f32 / 255.0),
            color::srgb_to_linear(c[2] as f32 / 255.0),
        ])
    }
}

pub(super) fn paint_tint(
    painter: &egui::Painter,
    b: &edit_model::Brush,
    geometry: &edit_model::Geometry,
    frame_rect: egui::Rect,
    base: Option<&TintFrame>,
) {
    let (w, h) = (frame_rect.width() as usize, frame_rect.height() as usize);
    if w == 0 || h == 0 {
        return;
    }
    // ⚠⚠ **`frame_rect` is the WHOLE FRAME's on-screen rect, not the viewport.** At 1:1 on a
    // full-resolution buffer that is ~6246x4170 — so rasterizing "at screen resolution" was
    // rasterizing at *full* resolution, ~24 ms plus a 26 MB texture upload, every time the memo
    // missed. Reported in use as: responsive with the mask hidden, sluggish with it shown.
    //
    // The tint is a translucent indicator, not the photograph. Capping it and letting the GPU
    // stretch the texture makes its cost constant in zoom, and a slightly soft tint edge is
    // invisible against a 50% overlay. The spec's claim of "~1400x900, sub-millisecond" was
    // simply wrong about which rectangle it meant.
    let long = w.max(h);
    let scale = if long > TINT_MAX_DIM {
        TINT_MAX_DIM as f32 / long as f32
    } else {
        1.0
    };
    let (tw, th) = (
        ((w as f32 * scale) as usize).max(1),
        ((h as f32 * scale) as usize).max(1),
    );
    TINT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let base_id = base.map(|f| f.bytes.as_ptr());
        let hit = slot.as_ref().is_some_and(|c| {
            c.w == w && c.h == h && c.geometry == *geometry && c.brush == *b && c.base == base_id
        });
        if !hit {
            // ⚠ Gated only when this brush actually needs pixels *and* a frame has landed. With
            // no base an auto-masked stroke tints positionally, which is the documented
            // degradation and better than showing nothing while the first frame renders.
            let view = base
                .filter(|_| b.needs_base())
                .map(|f| TintBase { f, tw, th });
            let cov = pipeline::brush::rasterize_in(
                b,
                geometry,
                tw,
                th,
                None,
                view.as_ref().map(|v| v as &dyn pipeline::brush::BasePixels),
            );
            let outside = cov.outside;
            let drawn = if cov.w == 0 || cov.h == 0 {
                None
            } else {
                // ⚠ Red because it is conventional and because none of the four accents is red,
                // so the tint can never be confused with active state. Alpha is coverage/2, so a
                // weak stroke reads as a weak tint — which is what makes Flow legible.
                let px: Vec<egui::Color32> = cov
                    .data
                    .iter()
                    .map(|&a| egui::Color32::from_rgba_unmultiplied(255, 40, 40, a / 2))
                    .collect();
                let img = egui::ColorImage {
                    size: [cov.w, cov.h],
                    pixels: px,
                    source_size: egui::vec2(cov.w as f32, cov.h as f32),
                };
                let tex =
                    painter
                        .ctx()
                        .load_texture("mask_tint", img, egui::TextureOptions::LINEAR);
                Some((tex, [cov.x0, cov.y0, cov.w, cov.h], scale))
            };
            *slot = Some(TintCache {
                brush: b.clone(),
                geometry: *geometry,
                w,
                h,
                base: base_id,
                drawn,
                outside,
            });
        }
        let Some(cache) = slot.as_ref() else { return };
        if cache.outside > 0 {
            let tint = egui::Color32::from_rgba_unmultiplied(255, 40, 40, cache.outside / 2);
            // ⚠⚠ Painted **around** the box, never under it. A single full-frame rect plus the
            // box's own texture double-blends wherever they overlap — and for an inverted brush
            // the area inside the box but outside the strokes is *also* fully covered, so the box
            // showed up as a distinctly brighter rectangle over the photograph. Reported in use
            // as "an additional mask box".
            let box_rect = cache.drawn.as_ref().map(|(_, b, sc)| {
                let inv = 1.0 / sc;
                egui::Rect::from_min_size(
                    frame_rect.min + egui::vec2(b[0] as f32 * inv, b[1] as f32 * inv),
                    egui::vec2(b[2] as f32 * inv, b[3] as f32 * inv),
                )
            });
            match box_rect {
                None => {
                    painter.rect_filled(frame_rect, egui::CornerRadius::ZERO, tint);
                }
                Some(b) => {
                    let b = b.intersect(frame_rect);
                    for r in [
                        // above, below, left, right — no overlap with `b` and none with each other.
                        egui::Rect::from_min_max(
                            frame_rect.min,
                            egui::pos2(frame_rect.max.x, b.min.y),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(frame_rect.min.x, b.max.y),
                            frame_rect.max,
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(frame_rect.min.x, b.min.y),
                            egui::pos2(b.min.x, b.max.y),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(b.max.x, b.min.y),
                            egui::pos2(frame_rect.max.x, b.max.y),
                        ),
                    ] {
                        if r.width() > 0.0 && r.height() > 0.0 {
                            painter.rect_filled(r, egui::CornerRadius::ZERO, tint);
                        }
                    }
                }
            }
        }
        if let Some((tex, b, sc)) = cache.drawn.as_ref() {
            // ⚠ Derived from the LIVE `frame_rect`, never from a cached rect — see the field's
            // note. `frame_rect` moves with the pan, so the tint must be placed against it afresh.
            let inv = 1.0 / sc;
            let rect = egui::Rect::from_min_size(
                frame_rect.min + egui::vec2(b[0] as f32 * inv, b[1] as f32 * inv),
                egui::vec2(b[2] as f32 * inv, b[3] as f32 * inv),
            );
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::brush::BasePixels;

    /// A `bw` x 1 strip whose red channel is `i * 60`, so which pixel was read is unambiguous.
    fn strip(bw: usize, origin: (usize, usize), full: (usize, usize)) -> TintFrame {
        let mut bytes = Vec::new();
        for i in 0..bw {
            bytes.extend_from_slice(&[(i * 60) as u8, 0, 0, 255]);
        }
        TintFrame {
            bytes: std::sync::Arc::new(bytes),
            bw,
            bh: 1,
            origin,
            full,
        }
    }

    /// The red channel the sampler should have produced for buffer pixel `i`.
    fn expected(i: usize) -> f32 {
        color::srgb_to_prophoto([color::srgb_to_linear((i * 60) as f32 / 255.0), 0.0, 0.0])[0]
    }

    #[test]
    fn the_tint_grid_maps_onto_the_rendered_frame() {
        // ⚠⚠ The mapping is where a silent bug lives: three scales are in play — the tint's capped
        // grid, the full rendered frame, and the buffer, which at zoom holds only a region of it.
        // A wrong one still produces a plausible tint, just over the wrong pixels.
        let f = strip(4, (0, 0), (4, 1));
        let b = TintBase {
            f: &f,
            tw: 8,
            th: 2,
        };
        // Eight tint columns over four frame pixels: two columns per pixel.
        for (x, want) in [(0, 0), (1, 0), (2, 1), (3, 1), (6, 3), (7, 3)] {
            assert!(
                (b.at(x, 0)[0] - expected(want)).abs() < 1e-6,
                "tint column {x} should read buffer pixel {want}, got {}",
                b.at(x, 0)[0]
            );
        }
    }

    #[test]
    fn a_region_buffers_origin_is_subtracted() {
        // ⚠ The discriminating case. With an origin of (0, 0) a sampler that ignored it would pass
        // the test above unchanged, and a region render is the common case at 1:1.
        let f = strip(2, (2, 0), (4, 1));
        let b = TintBase {
            f: &f,
            tw: 8,
            th: 2,
        };
        // Tint column 4 sits at frame x 2.25, i.e. buffer pixel 0; column 6 at 3.25, pixel 1.
        assert!((b.at(4, 0)[0] - expected(0)).abs() < 1e-6);
        assert!((b.at(6, 0)[0] - expected(1)).abs() < 1e-6);
    }

    #[test]
    fn outside_the_rendered_region_clamps_rather_than_reading_black() {
        // ⚠ `BaseView`'s reason: a dab is kept when its disc overlaps, so its centre can sit just
        // outside the region. Black would give it an absurd reference and make it paint nothing;
        // the nearest real pixel keeps the error small and local.
        let f = strip(2, (2, 0), (4, 1));
        let b = TintBase {
            f: &f,
            tw: 8,
            th: 2,
        };
        assert!(
            (b.at(0, 0)[0] - expected(0)).abs() < 1e-6,
            "left of the region must clamp to its first pixel"
        );
    }

    #[test]
    fn a_degenerate_frame_is_survivable() {
        // The buffer arrives from another thread; a mismatch must degrade, never panic.
        let f = strip(0, (0, 0), (0, 0));
        let b = TintBase {
            f: &f,
            tw: 0,
            th: 0,
        };
        assert_eq!(b.at(3, 3), [0.0; 3]);
    }
}

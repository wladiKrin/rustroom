//! The brush coverage tint: a memoised, resolution-capped rasterization uploaded as one
//! texture, plus the surround an inverted brush needs.
//!
//! ⚠ Its own module because the memo, its cache key and the box-versus-surround painting are a
//! self-contained concern — `mask_overlay` only ever calls `paint_tint`.

use eframe::egui;
use rayon::prelude::*;

/// The largest edge the tint's coverage is rasterized at, whatever the zoom.
///
/// ⚠ The tint is an indicator over the photograph, not the photograph, so it does not need the
/// frame's real resolution — and at 1:1 the frame is the full decode. Capping makes the cost
/// constant in zoom.
/// How many masks' tints are kept: enough for every eye a panel is likely to have on.
const TINT_CACHE_ENTRIES: usize = 8;
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
    /// The composite's **shape** — its base and every refinement. ⚠ Not the adjustments: the tint
    /// shows where a mask reaches, and moving its exposure does not move that.
    mask: edit_model::Mask,
    refine: Vec<edit_model::Refinement>,
    /// The whole-mask Invert (M10.12a) — part of the shape the tint shows.
    invert: bool,
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
    /// The AI maps' generation this was drawn from (M9.13) — a map arriving is a miss.
    ai_gen: u64,
    /// Coverage outside the box — non-zero for an **inverted** brush, which covers the whole
    /// frame except what was painted.
    ///
    /// ⚠ Without this the tint showed the inversion only inside a rectangle around the strokes,
    /// which grew as more was painted. Found in use, and the same omission the coverage map
    /// itself had.
    outside: u8,
}

impl TintCache {
    /// Whether this entry was drawn for `m`'s **shape**.
    ///
    /// ⚠⚠ An exhaustive destructure, so a field added to `MaskedAdjustment` is a compile error
    /// here and whether it moves the tint is decided rather than forgotten — `invert` arrived and
    /// the old `mask == && refine ==` comparison went on serving the uninverted tint (M10.12a).
    fn same_shape(&self, m: &edit_model::MaskedAdjustment) -> bool {
        let edit_model::MaskedAdjustment {
            mask,
            refine,
            invert,
            adjustments: _,
            disabled: _,
        } = m;
        self.mask == *mask && self.refine == *refine && self.invert == *invert
    }
}

thread_local! {
    /// ⚠ Thread-local rather than a field on `RustroomApp`: `paint_mask` is a free function
    /// reached from inside egui closures that already hold `&mut app`, and threading a cache down
    /// would widen four signatures for something with no observable behaviour. egui is
    /// single-threaded per context, so this is the same thread every frame.
    static TINT: std::cell::RefCell<Vec<TintCache>> = const { std::cell::RefCell::new(Vec::new()) };
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

/// What the tint needs to draw an AI component (M9.13) — references only, so building one every
/// frame costs nothing; the decode mapping is made on a cache miss.
pub(super) struct AiTintSource<'a> {
    pub maps: &'a rustroom_app::ai_coverage::AiMaps,
    pub edits: &'a edit_model::Edits,
    pub lens: Option<&'a lens_profile::LensShot>,
    /// The decode buffer's size.
    pub decode: (usize, usize),
}

/// Whether the composite needs the displayed frame to be evaluated.
fn needs_base(m: &edit_model::MaskedAdjustment) -> bool {
    std::iter::once(&m.mask)
        .chain(m.refine.iter().map(|r| &r.mask))
        .any(|k| match k {
            edit_model::Mask::Brush(b) => b.needs_base(),
            // ⚠⚠ **A range component reads the photograph, so it needs the frame.** This is the
            // half `paint_mask`'s old comment refused to draw at all, and mask isolation is what
            // makes it honest: while the tint is shown, `suppressed_mask` holds THIS mask's
            // adjustments out of the view, so the displayed frame is this mask's own base rather
            // than its output. The mask can no longer be shown selecting the tones it just moved.
            //
            // ⚠ Two residual error terms remain and are not papered over: the frame is 8-bit sRGB
            // **after the display rendering and the creative LUT**, where the render reads linear
            // working-space pixels; and **later masks' adjustments are still in it**, where this
            // mask's true base is masks 0..i-1 only. Doing it exactly needs coverage against the
            // pre-mask buffer, which lives on the render worker — a real feature, and a different
            // one.
            edit_model::Mask::Luminance(_) | edit_model::Mask::Colour(_) => true,
            _ => false,
        })
}

/// The composite's coverage on the tint grid.
///
/// ⚠⚠ **A lone brush takes the path it always did, byte for byte** — one `rasterize_in`, keeping
/// its bbox, which is what makes the common case cost what it used to. Only a composite pays for
/// a full-grid plane, and only a composite needs one: an analytic component covers an arbitrary
/// area that no stroke bbox bounds.
fn coverage(
    m: &edit_model::MaskedAdjustment,
    geometry: &edit_model::Geometry,
    tw: usize,
    th: usize,
    view: Option<&TintBase<'_>>,
    ai: &AiTintSource<'_>,
) -> pipeline::brush::CoverageMap {
    // ⚠ A plain binding rather than a closure: a closure taking and returning a reference needs
    // the two lifetimes tied, which is more ceremony than the one call site is worth.
    let as_base: Option<&dyn pipeline::brush::BasePixels> =
        view.map(|v| v as &dyn pipeline::brush::BasePixels);
    // ⚠ The fast path is a **lone brush** specifically, not "no refinements": it is the one case
    // whose coverage is already bbox-bounded, which is what keeps the commonest tint costing what
    // it always did. Every other shape — including a lone gradient, radial or range — goes through
    // the plane below, because an analytic or pixel-dependent component covers an area no stroke
    // bbox bounds.
    if m.refine.is_empty() {
        if let edit_model::Mask::Brush(b) = &m.mask {
            // ⚠ Under the whole-mask Invert (M10.12a), the brush with its own Invert flipped: for
            // one shape the two are the same coverage, and this keeps the box and the `outside`
            // surround an inverted brush already uses.
            let flipped;
            let b = if m.invert {
                flipped = edit_model::Brush {
                    invert: !b.invert,
                    ..b.clone()
                };
                &flipped
            } else {
                b
            };
            return pipeline::brush::rasterize_in(b, geometry, tw, th, None, as_base);
        }
    }

    // One rasterization per painted component, indexed alongside the shapes.
    let shapes: Vec<&edit_model::Mask> = std::iter::once(&m.mask)
        .chain(m.refine.iter().map(|r| &r.mask))
        .collect();
    let maps: Vec<Option<pipeline::brush::CoverageMap>> = shapes
        .iter()
        .map(|k| match k {
            edit_model::Mask::Brush(b) => Some(pipeline::brush::rasterize_in(
                b, geometry, tw, th, None, as_base,
            )),
            // M9.13: the AI map on this grid, through the same decode mapping the render uses.
            // ⚠ The same once-per-map lens warp the render uses — a tint miss no longer builds a
            // lens corrector (43 ms) or looks one up per pixel.
            edit_model::Mask::Ai(a) => ai.maps.get(&a.prompt).map(|p| {
                let lensed = rustroom_app::ai_coverage::post_lens(
                    p,
                    rustroom_app::ai_space::lens_key(ai.lens, ai.edits, ai.decode.0, ai.decode.1),
                    ai.decode,
                    || {
                        ai.lens.and_then(|l| {
                            rustroom_app::ai_space::corrector_for(
                                l,
                                ai.edits,
                                ai.decode.0,
                                ai.decode.1,
                            )
                        })
                    },
                );
                let dm = rustroom_app::ai_space::DecodeMap::new(
                    geometry,
                    ai.edits.perspective,
                    tw,
                    th,
                    None,
                    ai.decode.0,
                    ai.decode.1,
                );
                rustroom_app::ai_coverage::to_frame(&lensed, a, &dm, None)
            }),
            _ => None,
        })
        .collect();
    // ⚠⚠ The render's rule (`Composite::waits_for_a_map`, M10.12a's review): an inverted
    // composite whose AI map has not arrived tints nothing — inverted, the missing map's 0 would
    // paint the whole frame.
    if m.invert
        && shapes
            .iter()
            .zip(&maps)
            .any(|(k, c)| matches!(k, edit_model::Mask::Ai(_)) && c.is_none())
    {
        return pipeline::brush::CoverageMap {
            x0: 0,
            y0: 0,
            w: tw,
            h: th,
            data: vec![0; tw * th],
            outside: 0,
        };
    }

    // ⚠ The frame's aspect, not the tint grid's — they agree by construction, since the grid is
    // proportional to `frame_rect`, and saying which one is meant is what stops the next reader
    // reaching for the wrong rectangle.
    // ⚠ The frame's aspect, not the tint grid's — they agree by construction, since the grid is
    // proportional to `frame_rect`, and saying which one is meant is what stops the next reader
    // reaching for the wrong rectangle.
    let aspect = tw as f32 / th.max(1) as f32;
    let mut data = vec![0u8; tw * th];
    // ⚠⚠ **Rayon over ROWS, and the sequential version was a DEFECT rather than a caveat.** The
    // memo does not save the case that hurts: dragging a component's slider changes the shape
    // every frame, so it misses every frame. Measured at 1400x930, best of 5:
    //
    // | composite | sequential | parallel |
    // |---|---|---|
    // | + 1 radial | 13.0 ms | **3.6 ms** |
    // | + 1 luminance | 28.1 ms | **7.8 ms** |
    // | + 1 colour | **62.1 ms** | **14.1 ms** |
    // | + 3 colour | **99.6 ms** | **20.9 ms** |
    //
    // 62 ms/frame is worse than the ~36 ms once reported in use as "responsive with the mask
    // hidden, sluggish with it shown". The pipeline's own equivalent has always been parallel
    // (`map_pixels_xy`); this was the only place doing the same arithmetic serially.
    // `grade_view::coverage` is the precedent in this crate, down to the `par_chunks_mut(w)`.
    //
    // ⚠ Three colour components cost 20.9 ms against one at 14.1 — about +3.4 ms each rather than
    // +14 — which is the per-pixel sharing below doing the same job it does in the render.
    //
    // ⚠ The projection this replaced was **5x optimistic**, derived from the render's own
    // per-component numbers without noticing they were measured on a parallel loop. Sixth cost
    // estimate in this area corrected by measuring.
    //
    // ⚠ A row carries no state between pixels, so the result is bit-identical to the sequential
    // form — which is what lets the tint tests stand as the check on this.
    data.par_chunks_mut(tw).enumerate().for_each(|(y, row)| {
        for (x, slot) in row.iter_mut().enumerate() {
            let (u, v) = ((x as f32 + 0.5) / tw as f32, (y as f32 + 0.5) / th as f32);
            // ⚠ Read once per pixel and shared by every range component, the rule
            // `mask_coverage::BaseFacts` follows in the render for the reason measured there:
            // three colour components must cost one Oklab conversion, not three.
            let mut light: Option<f32> = None;
            let mut hc: Option<(f32, f32)> = None;
            let sample = || view.map_or([0.0; 3], |b| pipeline::brush::BasePixels::at(b, x, y));
            let at = |k: &edit_model::Mask,
                      j: usize,
                      light: &mut Option<f32>,
                      hc: &mut Option<(f32, f32)>|
             -> f32 {
                match k {
                    edit_model::Mask::Linear(_) | edit_model::Mask::Radial(_) => {
                        k.analytic_coverage(u, v, aspect).unwrap_or(0.0)
                    }
                    // ⚠ M9.13: an AI mask's raster sits in the same per-component slot as a brush's,
                    // resampled to this grid by whoever filled `maps`.
                    edit_model::Mask::Brush(_) | edit_model::Mask::Ai(_) => {
                        maps[j].as_ref().map_or(0.0, |c| c.at(x, y) as f32 / 255.0)
                    }
                    edit_model::Mask::Luminance(r) => {
                        r.coverage(*light.get_or_insert_with(|| pipeline::lightness(sample())))
                    }
                    edit_model::Mask::Colour(r) => {
                        let (hue, chroma) =
                            *hc.get_or_insert_with(|| pipeline::hue_chroma(sample()));
                        r.coverage_at(hue, chroma)
                    }
                }
            };
            let mut a = at(shapes[0], 0, &mut light, &mut hc).clamp(0.0, 1.0);
            for (j, r) in m.refine.iter().enumerate() {
                let b = at(&r.mask, j + 1, &mut light, &mut hc).clamp(0.0, 1.0);
                a = r.op.combine(a, b);
            }
            // ⚠ The whole-mask Invert, after the last fold — `Composite::at`'s order.
            if m.invert {
                a = 1.0 - a.clamp(0.0, 1.0);
            }
            *slot = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
    pipeline::brush::CoverageMap {
        x0: 0,
        y0: 0,
        w: tw,
        h: th,
        data,
        // ⚠ Zero: the plane spans the whole grid, so there is no "outside" for an inversion to
        // leak into — every component's own `invert` is already folded into `data`.
        outside: 0,
    }
}

pub(super) fn paint_tint(
    painter: &egui::Painter,
    m: &edit_model::MaskedAdjustment,
    geometry: &edit_model::Geometry,
    frame_rect: egui::Rect,
    base: Option<&TintFrame>,
    ai: &AiTintSource<'_>,
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
        // ⚠ A handful of entries, not one: several masks can be shown at once (2026-09-19),
        // and a single slot would recompute every one of them every frame.
        let found = slot.iter().position(|c| {
            c.w == w
                && c.h == h
                && c.geometry == *geometry
                && c.same_shape(m)
                // ⚠ The displayed frame matters only to a mask that READS it (a gating brush, a
                // range). Keyed for every mask, each pan's new frame missed — and an AI component's
                // rebuild made that sluggish (M9.13 follow-up).
                && (c.base == base_id || !needs_base(m))
                && c.ai_gen == ai.maps.generation()
        });
        if found.is_none() {
            // ⚠ Gated only when this brush actually needs pixels *and* a frame has landed. With
            // no base an auto-masked stroke tints positionally, which is the documented
            // degradation and better than showing nothing while the first frame renders.
            let view = base
                .filter(|_| needs_base(m))
                .map(|f| TintBase { f, tw, th });
            let cov = coverage(m, geometry, tw, th, view.as_ref(), ai);
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
            if slot.len() >= TINT_CACHE_ENTRIES {
                slot.remove(0);
            }
            slot.push(TintCache {
                mask: m.mask.clone(),
                refine: m.refine.clone(),
                invert: m.invert,
                geometry: *geometry,
                w,
                h,
                base: base_id,
                ai_gen: ai.maps.generation(),
                drawn,
                outside,
            });
        }
        let Some(cache) = found.map(|i| &slot[i]).or_else(|| slot.last()) else {
            return;
        };
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

    /// A mask whose base covers the whole grid, so a refinement is the only thing that can change
    /// the tint.
    fn everything() -> edit_model::MaskedAdjustment {
        edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [-1.0, 0.0],
                to: [-0.5, 0.0],
                invert: false,
            }),
            ..Default::default()
        }
    }

    /// No AI maps — every tint test but the AI one's.
    fn no_ai<R>(f: impl FnOnce(&AiTintSource<'_>) -> R) -> R {
        let maps = rustroom_app::ai_coverage::AiMaps::default();
        let edits = edit_model::Edits::default();
        f(&AiTintSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (1, 1),
        })
    }

    fn plane(
        m: &edit_model::MaskedAdjustment,
        tw: usize,
        th: usize,
    ) -> pipeline::brush::CoverageMap {
        no_ai(|ai| coverage(m, &edit_model::Geometry::default(), tw, th, None, ai))
    }

    /// M9.13: an AI component tints where its map covers — on the tint grid, through the decode
    /// mapping — and a mask without its map tints nothing.
    #[test]
    fn an_ai_component_tints_where_its_map_covers() {
        let m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
            ..Default::default()
        };
        let mut maps = rustroom_app::ai_coverage::AiMaps::default();
        maps.insert(
            edit_model::AiPrompt::Subject,
            std::sync::Arc::new(ai_mask::ProbMap::from_fn(20, 10, |x, _| {
                if x < 10 {
                    1.0
                } else {
                    0.0
                }
            })),
        );
        let edits = edit_model::Edits::default();
        let ai = AiTintSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (40, 20),
        };
        let cov = coverage(&m, &edit_model::Geometry::default(), 40, 20, None, &ai);
        assert_eq!(cov.at(5, 10), 255);
        assert_eq!(cov.at(35, 10), 0);
        let empty = plane(&m, 40, 20);
        assert!(
            empty.data.iter().all(|&c| c == 0),
            "no map must tint nothing"
        );
    }

    /// As [`plane`], with a displayed frame for the pixel-dependent kinds to read.
    fn plane_with_base(
        m: &edit_model::MaskedAdjustment,
        tw: usize,
        th: usize,
    ) -> pipeline::brush::CoverageMap {
        let f = strip(8, (0, 0), (8, 1));
        let view = TintBase { f: &f, tw, th };
        no_ai(|ai| coverage(m, &edit_model::Geometry::default(), tw, th, Some(&view), ai))
    }

    /// ⚠⚠ **The tint covers every component, not just the base.** A version that tinted the base
    /// alone renders a plausible red wash and would pass any "is there a tint?" check — so this
    /// asserts the *subtracted* region specifically, against the same point with no refinement.
    #[test]
    fn a_subtracted_component_cuts_the_tint() {
        // ⚠ Self-contained: a lone gradient is not tintable, so there is no single-shape baseline
        // to compare against — the two probes are inside and outside the cut-out of one composite.
        let (tw, th) = (64, 64);
        let mut m = everything();
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                center: [0.5, 0.5],
                radius: [0.3, 0.3],
                feather: 0.0,
                invert: false,
            }),
        });
        let cov = plane(&m, tw, th);
        let (inside, outside) = (cov.at(tw / 2, th / 2), cov.at(1, 1));
        assert!(
            outside > 200,
            "fixture: the base must cover the corner, got {outside}"
        );
        assert!(
            inside < 40,
            "the subtracted radial must cut the tint: inside {inside}, outside {outside}"
        );
    }

    /// ⚠ The composite plane spans the whole grid, so `outside` must be zero — an inversion is
    /// already folded into the data, and a non-zero outside would paint a second full-frame wash
    /// over it. That surround is what the inverted brush needed and what a composite must not have.
    #[test]
    fn a_composite_plane_has_no_surround() {
        let mut m = everything();
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
        });
        let cov = plane(&m, 32, 32);
        assert_eq!(cov.outside, 0);
        assert_eq!((cov.x0, cov.y0, cov.w, cov.h), (0, 0, 32, 32));
    }

    /// ⚠⚠ **A lone brush must take the path it always did.** The composite plane is a full grid;
    /// the single-brush path keeps its bounding box, which is what makes the common case cost
    /// what it used to. Asserting the box is *smaller than the grid* is what distinguishes them.
    #[test]
    fn a_lone_brush_keeps_its_bounding_box() {
        let m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![edit_model::BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.05,
                    hardness: 100.0,
                    flow: 100.0,
                    ..Default::default()
                }],
                invert: false,
            }),
            ..Default::default()
        };
        let cov = plane(&m, 128, 128);
        assert!(
            cov.w < 128 && cov.h < 128,
            "a lone brush must stay bbox-bounded, got {}x{}",
            cov.w,
            cov.h
        );
    }

    /// ⚠⚠ **Every mask kind tints, and the rule has no exceptions any more.** It used to be
    /// "a composite, or a lone brush" — reported in use, because `develop::body` suppresses
    /// whatever mask is selected while the tint is on **whatever kind it is**, so a lone radial
    /// held its own adjustments out of the view with no reachable control to untick. A tint that
    /// is drawn for exactly the masks that are suppressed is one rule rather than two that can
    /// disagree.
    ///
    /// ⚠ A lone brush is still the one *fast path* — see `coverage` — but that is about the box,
    /// not about whether anything is drawn.
    #[test]
    fn every_mask_kind_produces_a_plane() {
        let kinds: Vec<(&str, edit_model::Mask)> = vec![
            (
                "linear",
                edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: false,
                }),
            ),
            (
                "radial",
                edit_model::Mask::Radial(edit_model::RadialGradient::default()),
            ),
            (
                "luminance",
                edit_model::Mask::Luminance(edit_model::LuminanceRange::default()),
            ),
            (
                "colour",
                edit_model::Mask::Colour(edit_model::ColourRange::default()),
            ),
        ];
        for (name, mask) in kinds {
            let m = edit_model::MaskedAdjustment {
                mask,
                ..Default::default()
            };
            // ⚠ With a base, because a range kind reads the photograph — see the test below
            // for what it does without one.
            let cov = plane_with_base(&m, 48, 48);
            assert_eq!(
                (cov.w, cov.h),
                (48, 48),
                "{name}: a lone shape spans the whole grid"
            );
            assert!(
                cov.data.iter().any(|&a| a > 0),
                "{name}: the plane covers nothing at all"
            );
        }
    }

    /// ⚠⚠ **A colour range with no displayed frame tints NOTHING, and that is correct rather than
    /// a gap.** Its coverage is a function of the photograph's own hue; with no frame the sample
    /// reads black, whose chroma is zero, and the chroma gate — which exists precisely so a
    /// near-neutral pixel is not selected on noise — zeroes it. Inventing a hue instead would be
    /// the silent-mask failure `analytic_coverage`'s `Option` exists to prevent.
    ///
    /// ⚠ The same documented degradation an auto-masked brush already has when it tints before
    /// the first frame lands, which is why this is pinned rather than "fixed".
    #[test]
    fn a_range_mask_with_no_frame_tints_nothing() {
        let m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Colour(edit_model::ColourRange::default()),
            ..Default::default()
        };
        let cov = plane(&m, 32, 32);
        assert!(cov.data.iter().all(|&a| a == 0));
    }

    /// ⚠ The render's rule (`Composite::waits_for_a_map`): an inverted composite whose AI map has
    /// not arrived tints nothing, rather than the whole frame (M10.12a's review).
    #[test]
    fn an_inverted_composite_waiting_for_its_ai_map_tints_nothing() {
        let mut m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
            invert: true,
            ..Default::default()
        };
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Add,
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
                invert: false,
            }),
        });
        let waiting = plane(&m, 32, 32);
        assert!(
            waiting.data.iter().all(|&a| a == 0) && waiting.outside == 0,
            "an inverted composite tinted while its map was missing"
        );
        // Positive control: with the map (here, the network found nothing) it tints.
        let mut maps = rustroom_app::ai_coverage::AiMaps::default();
        maps.insert(
            edit_model::AiPrompt::Subject,
            std::sync::Arc::new(ai_mask::ProbMap::from_fn(20, 10, |_, _| 0.0)),
        );
        let edits = edit_model::Edits::default();
        let ai = AiTintSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (32, 32),
        };
        let answered = coverage(&m, &edit_model::Geometry::default(), 32, 32, None, &ai);
        assert!(
            answered.data.iter().any(|&a| a > 0),
            "fixture: with its map the inverted composite tints"
        );
    }

    /// The whole-mask Invert flips the composite plane at every pixel (M10.12a).
    #[test]
    fn an_inverted_composite_tints_the_inverse() {
        let mut m = everything();
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                center: [0.5, 0.5],
                radius: [0.3, 0.3],
                feather: 0.0,
                invert: false,
            }),
        });
        let plain = plane(&m, 64, 64);
        m.invert = true;
        let inv = plane(&m, 64, 64);
        for (x, y) in [(32, 32), (1, 1), (20, 32)] {
            let (p, q) = (plain.at(x, y) as i32, inv.at(x, y) as i32);
            assert!((p + q - 255).abs() <= 1, "({x},{y}): {p} + {q} is not 255");
        }
    }

    /// ⚠ A lone brush under the whole-mask Invert keeps its fast path — the box — and covers its
    /// surround, exactly as the brush's own Invert does.
    #[test]
    fn a_lone_brush_inverted_as_a_mask_tints_its_surround_and_keeps_its_box() {
        let brush = edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: vec![[0.5, 0.5]],
                radius: 0.05,
                hardness: 100.0,
                flow: 100.0,
                ..Default::default()
            }],
            invert: false,
        };
        let m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(brush),
            invert: true,
            ..Default::default()
        };
        let cov = plane(&m, 128, 128);
        assert!(
            cov.w < 128 && cov.h < 128,
            "the box was lost: {}x{}",
            cov.w,
            cov.h
        );
        assert_eq!(cov.outside, 255, "the surround must be covered");
    }

    /// ⚠⚠ The cache trap: an entry drawn for a mask must not serve the same mask inverted, and must
    /// still serve it with only an adjustment moved — the memo's whole purpose.
    #[test]
    fn the_tint_cache_tells_an_inverted_shape_apart_and_ignores_the_adjustments() {
        let m = everything();
        let entry = TintCache {
            mask: m.mask.clone(),
            refine: m.refine.clone(),
            invert: m.invert,
            geometry: edit_model::Geometry::default(),
            w: 1,
            h: 1,
            drawn: None,
            base: None,
            ai_gen: 0,
            outside: 0,
        };
        assert!(entry.same_shape(&m));
        let mut brighter = m.clone();
        brighter.adjustments.exposure = 1.0;
        assert!(
            entry.same_shape(&brighter),
            "an adjustment does not move the tint"
        );
        let mut inverted = m.clone();
        inverted.invert = true;
        assert!(
            !entry.same_shape(&inverted),
            "an inverted mask must re-tint"
        );
    }
}

//! Fixtures the render engine tests share across modules.

use super::brush_cache::BrushCache;
use super::frame::{FrameSpec, Ran, RenderedFrame};
use super::render::render_cached;
use super::RegionRect;
use crate::prefix_cache::PrefixCache;
use core_image::ImageBuf;
use edit_model::Edits;
use std::sync::Arc;

pub(crate) fn tiny() -> Arc<ImageBuf> {
    let mut img = ImageBuf::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            img.set_pixel(x, y, [0.5, 0.5, 0.5]);
        }
    }
    Arc::new(img)
}

/// A buffer with varied content, so a stale cache actually shows up as different bytes.
/// (The flat gray `tiny()` would hide almost any staleness.)
///
/// ⚠ Rectangular, because a square fixture cannot see a width/height swap — the class of bug
/// `every_prefix_stages_output_dims_match_what_it_produces` caught in `pipeline` with
/// "`geometry` predicted (37, 23) and produced (23, 37)".
pub(crate) fn varied_rect(w: usize, h: usize) -> Arc<ImageBuf> {
    let mut img = ImageBuf::new(w, h);
    let n = w.max(h).max(1);
    for y in 0..h {
        for x in 0..w {
            img.set_pixel(
                x,
                y,
                [
                    x as f32 / n as f32,
                    y as f32 / n as f32,
                    ((x + y) % 5) as f32 / 5.0,
                ],
            );
        }
    }
    Arc::new(img)
}

pub(crate) fn varied_sized(n: usize) -> Arc<ImageBuf> {
    varied_rect(n, n)
}

pub(crate) fn varied() -> Arc<ImageBuf> {
    varied_sized(16)
}

// ───────────────── M8.7: the whole prefix scoped, from its resume point ─────────────────
/// A **two-step** prefix: a gather then a local, which is exactly the shape M8.6 declared
/// impossible to chain.
///
/// ⚠⚠ `smoothness: 0.0`, not the default 50, and it is load-bearing. The chroma radius runs
/// 8..32 full-resolution pixels and its reach is **two passes** of it, so a defaulted
/// smoothness margins a 24x18 region out to roughly the whole of a 128x96 fixture — past
/// `SCOPE_MAX_AREA_PCT`, where the gate **correctly** refuses. The test would then be green
/// and blind to everything it is named for.
pub(crate) fn chained_edits() -> Edits {
    Edits {
        geometry: edit_model::Geometry {
            angle: 4.0,
            ..Default::default()
        },
        noise_reduction: edit_model::NoiseReduction {
            luminance: 0.0,
            color: 40.0,
            detail: 50.0,
            smoothness: 0.0,
        },
        // ⚠⚠ **The vignette is the point of the fixture, not decoration.** It is the only tail
        // stage whose result depends on *where* a pixel sits in the frame, so it is the only
        // thing that can see `(pw, ph)` taken from the region-scoped buffer instead of composed
        // from the plan. M8.6's version of this hazard was caught by exactly this stage and by
        // nothing else — all 63 goldens were blind to it.
        vignette: edit_model::Vignette {
            amount: -60.0,
            ..Default::default()
        },
        ..Edits::default()
    }
}

pub(crate) fn chained_spec() -> FrameSpec {
    FrameSpec {
        region: Some(RegionRect {
            x: 50,
            y: 36,
            w: 24,
            h: 18,
        }),
        ..FrameSpec::fit(128)
    }
}

/// Every prefix stage active, so a resume point is observable at each depth.
pub(crate) fn full_prefix() -> Edits {
    Edits {
        exposure: 0.5,
        geometry: edit_model::Geometry {
            angle: 2.0,
            ..Default::default()
        },
        noise_reduction: edit_model::NoiseReduction {
            luminance: 60.0,
            color: 60.0,
            ..Default::default()
        },
        dehaze: edit_model::Dehaze { amount: 40.0 },
        ..Edits::default()
    }
}

// ─────────────────────────── M8.3g: the 8-bit readback ───────────────────────────
/// A tail worth accelerating: `hsl` alone clears the cost gate, so the tail becomes one chain
/// ending in `output_transform` — the shape the byte path needs.
pub(crate) fn hsl_edits() -> Edits {
    let mut e = Edits::default();
    e.hsl.bands[0] = edit_model::HslBand {
        hue: 20.0,
        saturation: 10.0,
        luminance: 5.0,
    };
    e
}

/// A hazy buffer large enough for dehaze's guided reach, which is **twice** its radius.
pub(crate) fn hazy_buffer() -> Arc<ImageBuf> {
    let (w, h) = (200usize, 160);
    let a = [0.85f32, 0.87, 0.9];
    let mut img = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let dark = x % 40 < 6 && y % 40 < 6;
            let clear = if dark {
                [0.01, 0.01, 0.01]
            } else {
                [0.05 + 0.7 * u, 0.05 + 0.6 * v, 0.05 + 0.5 * (1.0 - u)]
            };
            let t = 0.95 - 0.9 * u;
            let mut px = [0.0f32; 3];
            for c in 0..3 {
                px[c] = clear[c] * t + a[c] * (1.0 - t);
            }
            img.set_pixel(x, y, px);
        }
    }
    Arc::new(img)
}

pub(super) fn render_with(
    buf: &Arc<ImageBuf>,
    e: &Edits,
    spec: FrameSpec,
    cache: &mut PrefixCache,
    bc: &mut BrushCache,
) -> (RenderedFrame, Ran) {
    render_cached(
        buf,
        e,
        spec,
        1.0,
        None,
        color::wb::Illuminant::default(),
        pipeline::TailRendering::default(),
        cache,
        bc,
    )
}

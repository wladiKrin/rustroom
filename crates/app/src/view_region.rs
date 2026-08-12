//! Pure viewport maths for region rendering: which part of the image the canvas needs, and
//! whether what is already rendered still covers it.
//!
//! Deliberately free of `egui` types so it can be tested directly — this is where clamping
//! and off-by-one mistakes would otherwise only show up as a visibly wrong image.

pub use crate::render_engine::RegionRect;

/// Rendered beyond the visible rect so small pans reuse the existing texture — and, since
/// the spatial tail stages run on the cropped region, so their edge band falls outside what
/// is displayed. See `the_region_margin_exceeds_every_tail_kernel_radius`.
///
/// 192 rather than a tighter fit over Clarity's 120 px reach: 72 px of headroom costs about
/// 20% more pixels in the region tail (~1.5 ms) and buys room for the next spatial stage.
pub const REGION_MARGIN: usize = 192;

/// The working resolution to render at, chosen from a ladder of rungs that double from fit
/// resolution up to the buffer's own.
///
/// `fit` is the scale that makes the whole image fit the viewport and `zoom` the user's factor
/// on top, so `fit * zoom` is screen pixels per *fit-render* pixel. The chosen rung is the
/// smallest one at which the texture is not being magnified.
///
/// Rungs **double** so the on-screen scale can never fall below ~0.5 — rendering much more than
/// is displayed is what makes a region render expensive. And they are **quantized** rather than
/// fitted exactly, because a `max_dim` varying continuously with zoom would rebuild the prefix
/// cache on every scroll tick.
///
/// The ladder falls out of the buffer rather than being enumerated, so a half-size buffer
/// yields two rungs and a full-resolution one yields three, on any sensor, with nothing to
/// extend when the buffer is upgraded underneath it.
pub fn tier_max_dim(fit_long: usize, buffer_long: usize, fit: f32, zoom: f32) -> usize {
    let fit_long = fit_long.max(1);
    let needed = (fit * zoom * fit_long as f32).ceil().max(1.0) as usize;
    let mut rung = fit_long;
    while rung < needed && rung < buffer_long {
        rung = (rung * 2).min(buffer_long);
    }
    rung
}

/// Whether a chosen rung renders a *region* rather than the whole image.
pub fn is_native(max_dim: usize, fit_long: usize) -> bool {
    max_dim > fit_long
}

/// Whether a whole-image frame may be taken as the fit-resolution size.
///
/// ⚠ The canvas caches the post-geometry size from whole-image frames, on the reasoning that
/// "a whole-image frame is by definition a fit render". That holds only while a request's
/// `max_dim` and its region cannot disagree. When they briefly could, a whole-image render at
/// a *native* rung set the cached size to the native dimensions — which shrank `fit` by 2–4×,
/// and because the ladder starts from that cached size, kept producing native whole-image
/// frames. Self-perpetuating, and unrecoverable by zooming since zoom is clamped at 1.0.
///
/// A fit render's long edge can never exceed the preview cap, so this rejects the bad frame
/// even if the pairing is broken again.
pub fn is_fit_sized(long_edge: usize, fit_max: usize) -> bool {
    long_edge <= fit_max
}

/// The sub-rect of an `image_w` x `image_h` image visible in a viewport of `avail_w` x
/// `avail_h` screen pixels, drawn at `scale` screen pixels per image pixel and panned by
/// `pan_x`/`pan_y` screen pixels.
///
/// Always returns an in-bounds, non-empty rect: panning far past an edge clamps rather than
/// producing an empty or negative region.
pub fn visible_rect(
    image_w: usize,
    image_h: usize,
    avail_w: f32,
    avail_h: f32,
    scale: f32,
    pan_x: f32,
    pan_y: f32,
) -> RegionRect {
    let axis = |image: usize, avail: f32, pan: f32| -> (usize, usize) {
        let image_f = image as f32;
        // Screen-space span of the image, centred in the viewport then panned.
        let drawn = image_f * scale;
        let left = (avail - drawn) * 0.5 + pan;
        // Image-space coordinate at each viewport edge. The low edge floors and the high edge
        // ceils, so the rect always *covers* the viewport rather than falling short of it.
        let lo = (-left / scale).floor().max(0.0);
        let hi = ((avail - left) / scale).ceil().min(image_f);
        let lo = lo.min(image_f - 1.0) as usize;
        let hi = (hi as usize).max(lo + 1).min(image);
        (lo, hi - lo)
    };
    let (x, w) = axis(image_w, avail_w, pan_x);
    let (y, h) = axis(image_h, avail_h, pan_y);
    RegionRect { x, y, w, h }
}

/// Expand by `margin` on every side, clipped to the image. Rendering a little more than is
/// visible means small pans reuse the existing texture instead of re-rendering.
pub fn with_margin(r: RegionRect, image_w: usize, image_h: usize, margin: usize) -> RegionRect {
    let x = r.x.saturating_sub(margin);
    let y = r.y.saturating_sub(margin);
    let right = (r.x + r.w + margin).min(image_w);
    let bottom = (r.y + r.h + margin).min(image_h);
    RegionRect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    }
}

/// Whether `inner` lies entirely within `outer` — the test for "the rendered region still
/// covers what is on screen, so nothing needs re-rendering".
pub fn contains(outer: RegionRect, inner: RegionRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::RenderScale;

    #[test]
    fn the_region_margin_exceeds_every_tail_kernel_radius() {
        // ⚠ The invariant that keeps 1:1 panning seam-free.
        //
        // Noise reduction cannot produce a seam because it lives in the *prefix*, which
        // always runs whole-buffer. Sharpening and local contrast live in the **tail**,
        // which runs on the cropped region — so at the region's edge they read neighbours
        // that do not exist, producing a band of wrong output. `with_margin` renders that
        // much extra on every side, so the band is never displayed.
        //
        // Widening a slider range, or adding another spatial stage to the tail, must fail
        // here rather than produce seams that only show up while panning at 1:1.
        let worst = pipeline::max_tail_kernel_radius(RenderScale::FULL);
        assert!(
            REGION_MARGIN > worst,
            "REGION_MARGIN {REGION_MARGIN} must exceed the tail's kernel radius {worst}"
        );
    }

    #[test]
    fn the_ladder_doubles_from_fit_to_the_buffer() {
        // Pre-upgrade: a 3123 buffer gives two rungs — exactly the behaviour before the
        // full-resolution work existed.
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 0.5), 1600);
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 1.0), 1600); // scale exactly 1.0 still fits
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 1.01), 3123);
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 9.0), 3123); // capped: no more data

        // Post-upgrade: a 6246 buffer gives three, with no enum to extend.
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 1.0), 1600);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 1.01), 3200);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 2.0), 3200);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 2.01), 6246);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 9.0), 6246);
    }

    #[test]
    fn a_rung_below_the_buffer_never_magnifies() {
        // ⚠ The invariant the ladder exists for, in the precise form — the loose version
        // ("scale is always in (0.5, 1.0]") is false twice over and would fail here. At the
        // fit rung there is no lower bound (zoomed out, fit is arbitrarily small), and at the
        // top rung magnification is unavoidable because the file holds no more data.
        for buffer in [3123_usize, 6246] {
            let mut z = 0.01_f32;
            while z <= 12.0 {
                let m = tier_max_dim(1600, buffer, 1.0, z);
                let scale = z * 1600.0 / m as f32;
                if m > 1600 {
                    assert!(
                        scale > 0.5 - 1e-4,
                        "buffer {buffer} zoom {z}: renders {:.1}x the displayed pixels",
                        1.0 / scale
                    );
                }
                if m > 1600 && m < buffer {
                    assert!(
                        scale <= 1.0 + 1e-4,
                        "buffer {buffer} zoom {z}: magnifies its own texture (scale {scale})"
                    );
                }
                z += 0.01;
            }
        }
    }

    #[test]
    fn the_ladder_is_quantized_not_continuous() {
        // A `max_dim` varying continuously with zoom would rebuild the prefix cache on every
        // scroll tick, which is the whole reason for rungs rather than an exact fit.
        let rung = tier_max_dim(1600, 6246, 1.0, 1.2);
        for z in [1.05_f32, 1.3, 1.6, 1.9, 2.0] {
            assert_eq!(
                tier_max_dim(1600, 6246, 1.0, z),
                rung,
                "zoom {z} left the rung"
            );
        }
    }

    #[test]
    fn a_tiny_image_has_only_one_rung() {
        // An image smaller than the preview cap: `fit_long == buffer_long`, so there is
        // nothing to climb to.
        for z in [0.1_f32, 1.0, 5.0] {
            assert_eq!(tier_max_dim(800, 800, 1.0, z), 800);
        }
        // Degenerate inputs must neither divide by zero nor loop forever.
        assert!(tier_max_dim(0, 0, 1.0, 2.0) >= 1);
        assert!(tier_max_dim(1600, 0, 1.0, 4.0) >= 1);
    }

    #[test]
    fn a_native_sized_frame_is_not_accepted_as_the_fit_size() {
        // ⚠ The guard behind the "image opens zoomed absurdly far out" bug. A whole-image
        // frame at a native rung must never be mistaken for a fit render, because the cached
        // fit size feeds the ladder and the mistake then sustains itself.
        assert!(is_fit_sized(1600, 1600));
        assert!(is_fit_sized(1068, 1600));
        assert!(!is_fit_sized(3123, 1600));
        assert!(!is_fit_sized(6246, 1600));
    }

    #[test]
    fn is_native_is_anything_above_the_fit_rung() {
        assert!(!is_native(1600, 1600));
        assert!(is_native(3200, 1600));
        assert!(is_native(6246, 1600));
    }

    #[test]
    fn a_fully_visible_image_needs_all_of_it() {
        // Viewport larger than the image at 1:1 — the whole image is visible.
        let v = visible_rect(100, 60, 400.0, 300.0, 1.0, 0.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 0,
                y: 0,
                w: 100,
                h: 60
            }
        );
    }

    #[test]
    fn zooming_in_needs_only_the_centre() {
        // 100x60 image at 4x in a 200x120 viewport shows 50x30 image pixels, centred.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 0.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 25,
                y: 15,
                w: 50,
                h: 30
            }
        );
    }

    #[test]
    fn panning_moves_the_visible_rect_and_clamps_at_the_edges() {
        // Panning the image right by 100 screen px at 4x moves the view 25 image px left.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 100.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 0,
                y: 15,
                w: 50,
                h: 30
            }
        );
        // Far past the edge still yields a valid in-bounds rect.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, -10_000.0, 0.0);
        assert_eq!(v.x + v.w, 100);
    }

    #[test]
    fn the_margin_expands_and_clips_at_the_image_edge() {
        let r = with_margin(
            RegionRect {
                x: 40,
                y: 30,
                w: 20,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 32,
                y: 22,
                w: 36,
                h: 26
            }
        );
        // At a corner the margin is clipped, not wrapped or negative.
        let r = with_margin(
            RegionRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 0,
                y: 0,
                w: 18,
                h: 18
            }
        );
    }

    #[test]
    fn containment_decides_whether_to_re_render() {
        let rendered = RegionRect {
            x: 10,
            y: 10,
            w: 50,
            h: 50,
        };
        assert!(contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(contains(rendered, rendered));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 5,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 100,
                h: 10
            }
        ));
    }
}

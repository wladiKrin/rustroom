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

/// Which resolution the canvas renders at.
///
/// Quantized to three tiers on purpose: a `max_dim` varying continuously with zoom would
/// rebuild the prefix cache on every scroll tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTier {
    /// The whole image at preview resolution.
    Fit,
    /// The half-size editing buffer's own resolution.
    NativeHalf,
    /// A full-resolution decode — the only tier that shows real demosaiced detail.
    NativeFull,
}

impl RenderTier {
    /// Whether this tier renders a *region* rather than the whole image.
    pub fn is_native(self) -> bool {
        self != RenderTier::Fit
    }

    /// Fall back to [`RenderTier::NativeHalf`] when no full-resolution buffer exists yet, so
    /// the view keeps working at the previous fidelity while the decode runs — or for good, if
    /// it fails.
    ///
    /// Kept separate from [`tier_for`] deliberately: the upgrade decision needs to know which
    /// tier the zoom *wants*, and folding availability into `tier_for` would make that
    /// circular — the tier could never ask for a buffer that does not exist yet.
    pub fn clamped(self, full_available: bool) -> RenderTier {
        match self {
            RenderTier::NativeFull if !full_available => RenderTier::NativeHalf,
            other => other,
        }
    }
}

/// The tier the current zoom calls for, ignoring what has actually been decoded.
///
/// `fit` is the scale that makes the whole image fit the viewport and `zoom` the user's factor
/// on top, so `fit * zoom` is screen pixels per *fit-render* pixel. `half_ratio` is the
/// half-size buffer's long edge divided by the fit render's (~1.95 on an X-T4): the zoom at
/// which the NativeHalf texture is itself magnified past its own pixels — exactly the reasoning
/// that puts the Fit boundary at 1.0.
pub fn tier_for(fit: f32, zoom: f32, half_ratio: f32) -> RenderTier {
    let s = fit * zoom;
    if s <= 1.0 {
        RenderTier::Fit
    } else if s <= half_ratio {
        RenderTier::NativeHalf
    } else {
        RenderTier::NativeFull
    }
}

/// Whether the view is magnifying the fit-resolution texture past its own pixels, in which
/// case the render should switch to the buffer's native resolution.
///
/// `fit` is the scale that makes the whole image fit the viewport; `zoom` is the user's
/// factor on top of it. At exactly 1.0 the texture is pixel-for-pixel, which fit still covers.
pub fn needs_native(fit: f32, zoom: f32) -> bool {
    fit * zoom > 1.0
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
    fn the_tier_boundaries_follow_the_buffer_ratio() {
        // `r` is the half-size buffer's long edge over the fit render's — ~1.95 on an X-T4.
        let r = 1.952;
        // At and below 1.0 the fit texture still has the pixels.
        assert_eq!(tier_for(0.5, 1.0, r), RenderTier::Fit);
        assert_eq!(tier_for(1.0, 1.0, r), RenderTier::Fit);
        // Just past it, only the half-size buffer does.
        assert_eq!(tier_for(1.0, 1.001, r), RenderTier::NativeHalf);
        assert_eq!(tier_for(1.0, r, r), RenderTier::NativeHalf);
        // Past the half buffer's own resolution, only a full decode does.
        assert_eq!(tier_for(1.0, r + 0.01, r), RenderTier::NativeFull);
        assert_eq!(tier_for(1.0, 8.0, r), RenderTier::NativeFull);
    }

    #[test]
    fn without_a_full_buffer_the_top_tier_falls_back() {
        // The view must keep working at today's fidelity while the decode runs, and if it
        // fails. `tier_for` says what the zoom *wants*; `clamped` says what is available.
        assert_eq!(
            tier_for(1.0, 8.0, 1.952).clamped(false),
            RenderTier::NativeHalf
        );
        assert_eq!(
            tier_for(1.0, 8.0, 1.952).clamped(true),
            RenderTier::NativeFull
        );
        // Clamping never promotes, and never touches the lower tiers.
        assert_eq!(tier_for(0.5, 1.0, 1.952).clamped(false), RenderTier::Fit);
        assert_eq!(
            tier_for(1.0, 1.5, 1.952).clamped(false),
            RenderTier::NativeHalf
        );
    }

    #[test]
    fn a_native_tier_never_magnifies_its_own_texture() {
        // ⚠ The invariant the three tiers exist for, and the reason a third tier was needed
        // rather than swapping the second one's buffer. `k` is rendered pixels per fit pixel,
        // so on-screen scale is fit*zoom/k. Inside a native tier it must never exceed 1.0
        // (magnifying a texture that has more pixels available) nor fall to 0.5 (rendering
        // more than about twice what is displayed — which is what made the two-tier version
        // with a full-res buffer cost ~180 ms a frame instead of 13).
        let r = 1.952_f32; // half buffer / fit
        let full_r = 2.0 * r; // full buffer / fit
        let mut z = 1.001_f32;
        while z <= full_r {
            let tier = tier_for(1.0, z, r);
            let k = match tier {
                RenderTier::Fit => 1.0,
                RenderTier::NativeHalf => r,
                RenderTier::NativeFull => full_r,
            };
            let scale = z / k;
            assert!(
                scale <= 1.0 + 1e-6,
                "zoom {z} magnifies its own texture (scale {scale})"
            );
            assert!(
                scale > 0.5 - 1e-6,
                "zoom {z} renders {:.1}x the displayed pixels",
                1.0 / scale
            );
            z += 0.01;
        }
    }

    #[test]
    fn is_native_matches_the_old_two_way_split() {
        // `needs_native` is kept and still means "render a region rather than the whole
        // image"; the tiers must agree with it or the canvas and the cache disagree.
        let r = 1.952;
        for z in [0.4_f32, 0.99, 1.0, 1.01, 1.9, 2.5, 9.0] {
            assert_eq!(
                tier_for(1.0, z, r).is_native(),
                needs_native(1.0, z),
                "zoom {z}"
            );
        }
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

    #[test]
    fn native_mode_engages_once_the_texture_is_magnified() {
        assert!(!needs_native(0.5, 1.0));
        assert!(!needs_native(0.5, 2.0)); // fit*zoom == 1.0 exactly: still fit
        assert!(needs_native(0.5, 2.1));
        assert!(needs_native(1.0, 1.5));
    }
}

//! The render scale: how much smaller than the full image a frame is rendered.

use pipeline::RenderScale;

/// The long edge `downscale_to(max_dim)` will produce, computed without doing the downscale.
///
/// The preview cache needs the render scale on a cache *hit*, when there is no downscaled
/// image to measure. `downscale_to` never upscales and caps the long edge, so this is exact
/// rather than an estimate — asserted against it in the tests.
pub(super) fn rendered_long_edge(buffer_long: usize, max_dim: usize) -> usize {
    if max_dim == 0 {
        buffer_long
    } else {
        buffer_long.min(max_dim)
    }
}

/// How far a render is from the full-resolution original: the source buffer may already
/// be a half-size decode (`source_scale`), and `downscale_to` shrinks it further. The
/// long edge is the reference because that is what `downscale_to` caps.
pub(super) fn render_scale(
    buffer_long: usize,
    rendered_long: usize,
    source_scale: f32,
) -> RenderScale {
    if buffer_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(source_scale * rendered_long as f32 / buffer_long as f32)
}

/// The [`RenderScale`] for a render at `max_dim`, expressed from the **full-resolution** long edge.
///
/// ⚠⚠ Algebraically identical to [`render_scale`], and phrased this way on purpose. At the
/// full-resolution handover `buffer_long` doubles and `source_scale` halves, while `full_long`
/// does **not move** — that invariance is the property it was created for. A caller deriving from
/// it therefore cannot be silently wrong by 2x for having failed to update a `source_scale` at the
/// handover: that bug is unrepresentable rather than merely tested.
/// `scale_from_full_agrees_with_the_engines_own_scale` pins the equality on both sides of it.
///
/// ⚠ `pub`, not `pub(crate)`, and that is **forced** rather than lax — the same reason
/// `requests_issued` is unconditional. This module is in the **lib**; `canvas.rs` and `Session`
/// are in the **bin**, which is a separate crate, so `pub(crate)` would not reach them.
///
/// It is needed because the **canvas** sizes its region request from
/// `pipeline::tail_kernel_radius`, which is measured in rendered pixels, so it needs exactly the
/// scale the worker will use. Re-deriving it there would be two facts that can disagree, with a
/// silent failure mode — a subtly wrong band at the edge of the visible area, and no error.
pub fn scale_from_full(full_long: usize, buffer_long: usize, max_dim: usize) -> RenderScale {
    if full_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(rendered_long_edge(buffer_long, max_dim) as f32 / full_long as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    /// `scale_from_full` is the engine's own `render_scale`, phrased from the invariant edge.
    ///
    /// ⚠⚠ The two must agree **on both sides of the full-resolution handover**, which is the
    /// whole reason the second phrasing exists: `buffer_long` doubles and `source_scale` halves
    /// at the upgrade, while `full_long` does not move. A caller that derives from `full_long`
    /// therefore cannot be wrong by 2x for having missed the handover — that bug is
    /// unrepresentable rather than merely tested.
    #[test]
    fn scale_from_full_agrees_with_the_engines_own_scale() {
        // (buffer_long, source_scale) before and after a RAW's handover, same photo.
        for (buffer_long, source_scale) in [(3123usize, 0.5f32), (6246, 1.0)] {
            let full_long = (buffer_long as f32 / source_scale).round() as usize;
            for max_dim in [1600usize, 3200, 6246] {
                let rendered = rendered_long_edge(buffer_long, max_dim);
                let engine = render_scale(buffer_long, rendered, source_scale);
                let derived = scale_from_full(full_long, buffer_long, max_dim);
                assert!(
                    (engine.get() - derived.get()).abs() < 1e-6,
                    "disagreed at buffer={buffer_long} max_dim={max_dim}: \
                     engine {} vs derived {}",
                    engine.get(),
                    derived.get()
                );
            }
        }
    }

    #[test]
    fn the_predicted_long_edge_matches_downscale_to() {
        // The cache keys off this prediction, so it must agree with what downscale_to
        // actually produces — including the case where the image is already small enough
        // and downscale_to just clones.
        for (w, h, max_dim) in [(8, 8, 4), (10, 5, 4), (5, 10, 4), (4, 4, 16), (7, 3, 7)] {
            let img = ImageBuf::new(w, h);
            let actual = img.downscale_to(max_dim);
            let actual_long = actual.width().max(actual.height());
            assert_eq!(
                rendered_long_edge(w.max(h), max_dim),
                actual_long,
                "{w}x{h} -> max_dim {max_dim}"
            );
        }
    }

    #[test]
    fn render_scale_combines_the_decode_and_the_downscale() {
        // A half-size RAW decode downscaled again by 2 is a quarter of full resolution;
        // the spatial stages must be told that, or a preview lies about the export.
        assert_eq!(render_scale(8, 4, 0.5).get(), 0.25);
        // A full-size source rendered at full size is exactly 1.0.
        assert_eq!(render_scale(8, 8, 1.0).get(), 1.0);
        // Non-square: the long edge is the reference, matching downscale_to.
        assert_eq!(render_scale(10, 5, 1.0).get(), 0.5);
    }
}

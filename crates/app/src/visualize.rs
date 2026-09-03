//! The Visualize Spots view: a high-contrast edge render that makes sensor dust findable.
//!
//! ⚠ A **view mode, not an edit**. It never reaches `Edits`, the sidecar, an export or the CLI.
//! It is applied in `render_engine::pack`, after the histogram is taken, for the reason the
//! display transform lives there: a pipeline stage would make the histogram describe the view
//! rather than the photograph.

/// The blur radius the high-pass is taken against, in **rendered** pixels.
///
/// ⚠ Rendered, not scene, pixels — and deliberately **not** scaled by the render scale. The point
/// is to make dust visible on screen, and dust is a few pixels across wherever you are looking
/// from. Unlike the spots themselves, which are normalised, this is a screen-space effect.
pub const SIGMA: f32 = 6.0;

/// The default threshold gain.
pub const DEFAULT_THRESHOLD: f32 = 12.0;
/// The slider's range.
pub const MIN_THRESHOLD: f32 = 1.0;
pub const MAX_THRESHOLD: f32 = 50.0;

/// A high-contrast edge view of one luminance plane.
///
/// `|luma − blur(luma)|`, gain-mapped by `threshold` and clamped. Dust and sensor spots read as
/// bright rings against flat grey.
///
/// ⚠ The threshold is a **gain**, not a cutoff: a cutoff would show nothing at all over most of a
/// slider's travel, which makes it impossible to judge by eye.
pub fn edge_view(luma: &[f32], w: usize, h: usize, threshold: f32) -> Vec<f32> {
    if w == 0 || h == 0 || luma.len() != w * h {
        return Vec::new();
    }
    let gain = if threshold.is_finite() {
        threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD)
    } else {
        MAX_THRESHOLD
    };
    let base = pipeline::spatial::gaussian_blur(luma, w, h, SIGMA);
    luma.iter()
        .zip(&base)
        .map(|(v, b)| ((v - b).abs() * gain).clamp(0.0, 1.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat plane with one small dark disc — a dust spot.
    fn dusty(w: usize, h: usize) -> Vec<f32> {
        let mut p = vec![0.6f32; w * h];
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, 4.0);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d < r {
                    p[y * w + x] = 0.3;
                }
            }
        }
        p
    }

    /// ⚠⚠ The whole point: the spot must stand out from the flat field far more than it does in
    /// the original. A test that only checked "the output differs" would pass against any
    /// transform at all, including one that made dust *less* visible.
    #[test]
    fn a_dust_spot_stands_out_further_than_it_did() {
        let (w, h) = (64, 64);
        let src = dusty(w, h);
        let out = edge_view(&src, w, h, DEFAULT_THRESHOLD);
        let at = |p: &[f32], x: usize, y: usize| p[y * w + x];
        let before = (at(&src, 32, 32) - at(&src, 8, 8)).abs();
        let after = (at(&out, 32, 32) - at(&out, 8, 8)).abs();
        assert!(after > before * 1.5, "before={before} after={after}");
    }

    /// ⚠ A flat field must come back flat and near black, or every photograph would look like it
    /// were covered in dust.
    #[test]
    fn a_flat_field_is_flat_and_dark() {
        let out = edge_view(&vec![0.42; 32 * 32], 32, 32, DEFAULT_THRESHOLD);
        let max = out.iter().copied().fold(0.0f32, f32::max);
        assert!(max < 0.05, "a flat field lit up: {max}");
    }

    /// ⚠ The threshold is a **gain**, so raising it shows more. A slider mapped to a hard cutoff
    /// would show nothing over most of its travel.
    #[test]
    fn a_higher_threshold_shows_more() {
        let (w, h) = (64, 64);
        let src = dusty(w, h);
        let low: f32 = edge_view(&src, w, h, 4.0).iter().sum();
        let high: f32 = edge_view(&src, w, h, 40.0).iter().sum();
        assert!(high > low * 1.5, "low={low} high={high}");
    }

    /// ⚠ Total: the output is always in `[0, 1]`, whatever the gain, or `to_rgb8` would clip
    /// arbitrarily and the view would read as solid white.
    #[test]
    fn the_output_stays_in_range_at_any_threshold() {
        let src = dusty(32, 32);
        for t in [0.0, 1.0, 50.0, 1e6, f32::INFINITY] {
            let out = edge_view(&src, 32, 32, t);
            assert!(
                out.iter().all(|v| (0.0..=1.0).contains(v)),
                "threshold {t} left the range"
            );
        }
    }

    /// ⚠ Degenerate sizes must not panic — the canvas can ask for a zero-width region.
    #[test]
    fn an_empty_plane_is_handled() {
        assert!(edge_view(&[], 0, 0, DEFAULT_THRESHOLD).is_empty());
    }
}

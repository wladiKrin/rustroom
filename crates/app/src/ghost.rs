//! A session at the click (2026-09-29): the cached 512-px preview stands in as the editing
//! buffer until the half-size decode lands, so the panel, the histogram, the guides and the
//! zoom keys exist from the first frame and nothing jumps in later.
//!
//! ⚠ A ghost is **display-referred**: the camera's JPEG has been rendered once already, so the
//! engine strips the camera rendering for it exactly as it does for a JPEG file
//! (`render_engine`'s `scene_referred` rule). The half decode replaces it through
//! `RustroomApp::adopt_decoded`, which keeps the edits, the history and the view.

use crate::decode_worker::{Decoded, LensFacts, ProfileParts};
use crate::render_engine::render_once;
use core_image::Thumbnail;
use edit_model::Edits;
use std::sync::Arc;

/// The decoded photo a thumbnail stands for. `max_dim` bounds the baseline render, as the
/// decode worker's does.
pub fn ghost_decoded(thumb: &Thumbnail, max_dim: usize) -> Arc<Decoded> {
    let buffer = Arc::new(crate::thumb_loader::linear_prophoto_from_srgb8(thumb));
    let baseline = render_once(
        &buffer,
        &Edits::default(),
        max_dim,
        1.0,
        color::Display::SRGB,
        pipeline::Resampler::default(),
        None,
    );
    Arc::new(Decoded {
        buffer,
        label: String::new(),
        baseline,
        source_scale: 1.0,
        lens: LensFacts::default(),
        wb_baseline: color::wb::Illuminant::default(),
        profile: ProfileParts::default(),
        scene_referred: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ghost_is_the_thumbnail_in_linear_prophoto_and_display_referred() {
        let thumb = Thumbnail {
            width: 2,
            height: 1,
            rgba: vec![255, 255, 255, 255, 0, 0, 0, 255],
        };
        let g = ghost_decoded(&thumb, 64);
        assert_eq!((g.buffer.width(), g.buffer.height()), (2, 1));
        let white = g.buffer.pixel(0, 0);
        let black = g.buffer.pixel(1, 0);
        assert!(
            white.iter().all(|c| (c - 1.0).abs() < 1e-3),
            "white stays white: {white:?}"
        );
        assert!(
            black.iter().all(|c| c.abs() < 1e-6),
            "black stays black: {black:?}"
        );
        assert!(!g.scene_referred, "already rendered once by the camera");
        assert_eq!(g.source_scale, 1.0);
        assert_eq!((g.baseline.width, g.baseline.height), (2, 1));
    }
}

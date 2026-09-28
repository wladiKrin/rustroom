//! Where an AI map lives: the **decoded photo's own pixel grid**, normalised per axis (M9.13).
//!
//! ⚠⚠ Chosen so a cached map survives every edit that moves pixels about — crop, straighten,
//! orientation, perspective, lens correction. A frame pixel reaches the map through the same chain
//! spot healing's sampler uses: `SpotMap::inverse_point` (frame → post-lens, pre-perspective) and
//! then the lens corrector's `source` (post-lens → decode). Only a new decode of the file
//! invalidates a map.

use edit_model::{Geometry, Perspective};
use pipeline::brush::SpotMap;

/// The lens corrector a decode of `w × h` gets under `edits` — the one construction, shared with
/// spot healing's sampler, so the two cannot disagree about where a post-lens point came from.
pub fn corrector_for(
    shot: &lens_profile::LensShot,
    edits: &edit_model::Edits,
    w: usize,
    h: usize,
) -> Option<lens_profile::Corrector> {
    let c = edits.applied_lens_correction();
    lens_profile::Corrector::new(
        &shot.profile,
        &shot.shot,
        w,
        h,
        lens_profile::Corrections {
            distortion: c.distortion,
            tca: c.tca,
            vignetting: c.vignetting,
            scale: c.scale,
        },
    )
}

/// A stable identity for "this lens, these corrections, this decode size" — what a map warped by
/// [`crate::ai_coverage::post_lens`] was warped for. `0` means no correction applies.
pub fn lens_key(
    shot: Option<&lens_profile::LensShot>,
    edits: &edit_model::Edits,
    w: usize,
    h: usize,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(shot) = shot else { return 0 };
    let mut hsh = std::collections::hash_map::DefaultHasher::new();
    format!("{shot:?}|{:?}|{w}x{h}", edits.applied_lens_correction()).hash(&mut hsh);
    hsh.finish().max(1)
}

/// Frame pixels → decode-normalised coordinates.
pub struct DecodeMap {
    spot: SpotMap,
    frame: (f32, f32),
    corr: Option<lens_profile::Corrector>,
    /// The decode's size in pixels — the post-lens frame has the same size.
    decode: (f32, f32),
}

impl DecodeMap {
    /// `corr` is the lens corrector for a decode of `decode_w × decode_h`, or `None` when no
    /// correction applies (a JPEG, a lens without a profile, correction off).
    pub fn new(
        geometry: &Geometry,
        perspective: Perspective,
        frame_w: usize,
        frame_h: usize,
        corr: Option<lens_profile::Corrector>,
        decode_w: usize,
        decode_h: usize,
    ) -> Self {
        Self {
            spot: SpotMap::new(geometry, perspective, frame_w, frame_h),
            frame: (frame_w as f32, frame_h as f32),
            corr,
            decode: (decode_w.max(1) as f32, decode_h.max(1) as f32),
        }
    }

    /// Frame pixel `(fx, fy)` (its centre is `+0.5`) → decode-normalised `[u, v]`.
    pub fn frame_to_decode(&self, fx: f32, fy: f32) -> [f32; 2] {
        self.post_lens_to_decode(self.spot.inverse_point(fx, fy))
    }

    /// A post-lens normalised point → decode-normalised. ⚠ The green channel's position: TCA moves
    /// the three apart by a pixel or so, far below a mask edge's width.
    pub fn post_lens_to_decode(&self, p: [f32; 2]) -> [f32; 2] {
        match &self.corr {
            Some(c) => {
                let src = c.source(p[0] * self.decode.0, p[1] * self.decode.1)[1];
                [src[0] / self.decode.0, src[1] / self.decode.1]
            }
            None => p,
        }
    }

    pub fn frame_size(&self) -> (f32, f32) {
        self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_geometry_and_no_lens_the_frame_is_the_decode() {
        let m = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            200,
            100,
            None,
            400,
            200,
        );
        let [u, v] = m.frame_to_decode(50.0, 25.0);
        assert!(
            (u - 0.25).abs() < 0.01 && (v - 0.25).abs() < 0.01,
            "{u},{v}"
        );
    }

    /// ⚠ The property the space exists for: a quarter turn moves the frame, and the decode point
    /// under a frame corner moves with it — the map itself never needs recomputing.
    #[test]
    fn a_quarter_turn_maps_the_frames_top_left_to_another_decode_corner() {
        let mut g = Geometry::default();
        g.orientation.rotate_cw();
        // Rotated, the frame is 100 × 200.
        let m = DecodeMap::new(&g, Perspective::default(), 100, 200, None, 200, 100);
        let [u, v] = m.frame_to_decode(1.0, 1.0);
        let unrotated = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            200,
            100,
            None,
            200,
            100,
        )
        .frame_to_decode(1.0, 1.0);
        assert!(
            (u - unrotated[0]).abs() > 0.5 || (v - unrotated[1]).abs() > 0.5,
            "the rotation moved nothing: {u},{v} vs {unrotated:?}"
        );
    }
}

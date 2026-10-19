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

/// The lens corrector a decode gets, built once per lens, its corrections and the decode's size.
///
/// ⚠ A corrector samples its whole displacement field when it is built — 290–370 ms on an X-T4 decode —
/// and Retouch built one on EVERY frame, so with a lens profile a hover over the photo ran at a few frames
/// a second (the author's report on M10.16c1, 2026-10-19). Look it up here, never build it per frame.
#[derive(Default)]
pub struct CorrectorMemo {
    held: Option<(u64, Option<std::sync::Arc<lens_profile::Corrector>>)>,
}

impl CorrectorMemo {
    /// `corrector_for`'s answer for this lens, these edits' corrections and a `w × h` decode — the held one
    /// while [`lens_key`] says nothing changed.
    pub fn get(
        &mut self,
        shot: Option<&lens_profile::LensShot>,
        edits: &edit_model::Edits,
        w: usize,
        h: usize,
    ) -> Option<std::sync::Arc<lens_profile::Corrector>> {
        let key = lens_key(shot, edits, w, h);
        if let Some((k, c)) = &self.held {
            if *k == key {
                return c.clone();
            }
        }
        let c = shot
            .and_then(|s| corrector_for(s, edits, w, h))
            .map(std::sync::Arc::new);
        self.held = Some((key, c.clone()));
        c
    }

    /// The corrector held now — what a test reads to see one was not built again.
    pub fn held(&self) -> Option<&std::sync::Arc<lens_profile::Corrector>> {
        self.held.as_ref().and_then(|(_, c)| c.as_ref())
    }
}

/// Frame pixels → decode-normalised coordinates.
pub struct DecodeMap {
    spot: SpotMap,
    frame: (f32, f32),
    corr: Option<std::sync::Arc<lens_profile::Corrector>>,
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
        Self::with_shared(
            geometry,
            perspective,
            frame_w,
            frame_h,
            corr.map(std::sync::Arc::new),
            decode_w,
            decode_h,
        )
    }

    /// [`Self::new`] with a corrector held elsewhere — a [`CorrectorMemo`]'s.
    pub fn with_shared(
        geometry: &Geometry,
        perspective: Perspective,
        frame_w: usize,
        frame_h: usize,
        corr: Option<std::sync::Arc<lens_profile::Corrector>>,
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

    /// Decode-normalised `d` → frame pixel `(fx, fy)` — [`Self::frame_to_decode`]'s inverse, for drawing
    /// what lives in decode space (M10.16c1's removals). ⚠ The lens has no forward map (M10.12c), so its
    /// inverse is solved by fixed-point iteration — a distortion is a few percent, and eight steps land
    /// within a hundredth of a pixel.
    pub fn decode_to_frame(&self, d: [f32; 2]) -> (f32, f32) {
        let mut p = d;
        if self.corr.is_some() {
            for _ in 0..8 {
                let at = self.post_lens_to_decode(p);
                p = [p[0] + d[0] - at[0], p[1] + d[1] - at[1]];
            }
        }
        self.spot.point(p)
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

    /// M10.16c1: a decode point drawn on the frame lands where the frame point it came from was — through
    /// a rotation and crop, and through a lens's distortion.
    #[test]
    fn decode_to_frame_undoes_frame_to_decode() {
        let mut g = Geometry {
            angle: 4.0,
            ..Default::default()
        };
        g.orientation.rotate_cw();
        let xml = r#"<lensdatabase version="1">
            <camera><maker>M</maker><model>C</model><mount>X</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>M</maker><model>L 50mm</model><mount>X</mount><cropfactor>1.0</cropfactor>
                <calibration><distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/></calibration>
            </lens></lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let profile = db.find("M", "C", "L 50mm").unwrap();
        let corr = lens_profile::Corrector::new(
            &profile,
            &lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
            300,
            200,
            lens_profile::Corrections {
                distortion: true,
                tca: false,
                vignetting: false,
                scale: 1.0,
            },
        );
        assert!(corr.is_some(), "the fixture's lens corrects");
        for corr in [None, corr] {
            let lens = corr.is_some();
            let m = DecodeMap::new(&g, Perspective::default(), 200, 300, corr, 300, 200);
            for (fx, fy) in [(20.0, 30.0), (100.0, 150.0), (180.0, 270.0)] {
                let d = m.frame_to_decode(fx, fy);
                let (bx, by) = m.decode_to_frame(d);
                assert!(
                    (bx - fx).abs() < 0.05 && (by - fy).abs() < 0.05,
                    "({fx}, {fy}) → {d:?} → ({bx}, {by}), lens {lens}"
                );
            }
        }
    }

    /// The Retouch review (2026-10-19): a corrector samples its whole displacement field when built —
    /// 290–370 ms on an X-T4 — so the memo hands back the same one while the lens, its corrections and the
    /// decode's size stay, and a new one when any changes.
    #[test]
    fn a_corrector_is_built_once_per_lens_and_size() {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>M</maker><model>C</model><mount>X</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>M</maker><model>L 50mm</model><mount>X</mount><cropfactor>1.0</cropfactor>
                <calibration><distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/></calibration>
            </lens></lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let shot = lens_profile::LensShot {
            profile: db.find("M", "C", "L 50mm").unwrap(),
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        };
        let mut edits = edit_model::Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut memo = CorrectorMemo::default();
        let a = memo
            .get(Some(&shot), &edits, 300, 200)
            .expect("a corrector");
        let b = memo
            .get(Some(&shot), &edits, 300, 200)
            .expect("a corrector");
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "built twice for the same lens"
        );
        let c = memo
            .get(Some(&shot), &edits, 600, 400)
            .expect("a corrector");
        assert!(
            !std::sync::Arc::ptr_eq(&a, &c),
            "another size, the same corrector"
        );
        edits.lens_correction = Some(edit_model::LensCorrection::default());
        assert!(
            memo.get(Some(&shot), &edits, 600, 400).is_none(),
            "corrections off"
        );
        assert!(memo.get(None, &edits, 600, 400).is_none(), "no lens");
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

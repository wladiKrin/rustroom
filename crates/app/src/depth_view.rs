//! Show depth map (M10.16a): a photo's depth map drawn over its frame, the nearest point white.
//!
//! ⚠ No egui — the canvas paints the bytes (`develop::canvas::depth_overlay`). Kept apart from the
//! Depth range on purpose: M10.16d's Lens Blur shows the same map for its focus.

use crate::ai_space::DecodeMap;
use ai_mask::ProbMap;

/// The depth `map` over a `w × h` frame, as opaque RGBA8 — grey `255 − depth`, so near is white —
/// sampled at each pixel's centre through `dm`, which must map a `w × h` frame.
///
/// ⚠ `map` must be in the space `dm` maps into: post-lens (`AiSource::lensed`) for a corrector-free
/// `dm` — the same pair the render's coverage reads, which is what makes the view line up with the
/// mask (`the_view_lines_up_with_the_coverage`).
pub fn image(map: &ProbMap, dm: &DecodeMap, w: usize, h: usize) -> Vec<u8> {
    use rayon::prelude::*;
    let mut out = vec![0u8; w * h * 4];
    out.par_chunks_mut((w * 4).max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let [u, v] = dm.frame_to_decode(x as f32 + 0.5, y as f32 + 0.5);
                let grey = 255 - (map.at(u, v) * 255.0).round() as u8;
                *px = [grey, grey, grey, 255];
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Geometry, Orientation, Perspective, Quarter};

    #[test]
    fn near_is_white_and_far_is_black() {
        let map = ProbMap::from_fn(10, 5, |x, _| if x < 5 { 0.0 } else { 1.0 });
        let dm = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            100,
            50,
            None,
            10,
            5,
        );
        let rgba = image(&map, &dm, 100, 50);
        assert_eq!(rgba.len(), 100 * 50 * 4);
        let px = |x: usize, y: usize| &rgba[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
        assert_eq!(px(10, 25), &[255, 255, 255, 255], "the near half is white");
        assert_eq!(
            px(90, 25),
            &[0, 0, 0, 255],
            "the far half black, and opaque"
        );
    }

    /// Under a rotation, the view's white is exactly where a near-half depth range covers — the view
    /// and the mask read one map through one mapping.
    #[test]
    fn the_view_lines_up_with_the_coverage() {
        let map = ProbMap::from_fn(40, 20, |x, y| ((x + 2 * y) % 40) as f32 / 39.0);
        let geometry = Geometry {
            orientation: Orientation {
                rotate: Quarter::Cw90,
                mirror: false,
            },
            ..Default::default()
        };
        let (w, h) = (20, 40); // the rotated frame
        let edits = edit_model::Edits {
            geometry,
            ..Default::default()
        };
        let near_half = edit_model::Mask::Depth(edit_model::DepthRange {
            near: 0.0,
            far: 50.0,
            feather: 0.0,
            invert: false,
        });
        let mut maps = crate::ai_coverage::AiMaps::default();
        maps.insert(
            crate::ai_coverage::MapQuery::Depth,
            std::sync::Arc::new(map.clone()),
        );
        let src = crate::ai_coverage::AiSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (40, 20),
        };
        let dm = src.decode_map(&geometry, w, h);
        let cov = src.component(&near_half, &dm, None).expect("a map");
        let rgba = image(&map, &dm, w, h);
        let mut agree = 0;
        for y in 0..h {
            for x in 0..w {
                let grey = rgba[(y * w + x) * 4];
                // Away from the band's edge, white (near) ⇔ covered.
                if !(118..=137).contains(&grey) {
                    assert_eq!(grey > 128, cov.at(x, y) > 128, "({x},{y}) grey {grey}");
                    agree += 1;
                }
            }
        }
        assert!(agree > 600, "only {agree} pixels compared");
    }
}

use super::*;
use crate::brush::rasterize_in;

/// Constructed: a wavy stroke of `n` points across the frame.
fn stroke(n: usize, y: f32, flow: f32, erase: bool) -> BrushStroke {
    BrushStroke {
        points: (0..n)
            .map(|i| [0.15 + i as f32 * 0.012, y + (i as f32 * 0.7).sin() * 0.03])
            .collect(),
        radius: 0.035,
        hardness: 30.0,
        flow,
        erase,
        ..Default::default()
    }
}

/// Every pixel of the frame, through `CoverageMap::at` — the two maps' boxes differ (the clip's against the dabs'),
/// and what must agree is coverage everywhere.
fn same_everywhere(a: &CoverageMap, b: &CoverageMap, w: usize, h: usize) -> Result<(), String> {
    for y in 0..h {
        for x in 0..w {
            if a.at(x, y) != b.at(x, y) {
                return Err(format!("({x}, {y}): {} against {}", a.at(x, y), b.at(x, y)));
            }
        }
    }
    Ok(())
}

/// ⚠⚠ The design's whole claim: grown one point at a time, the incremental map is the full raster, bit for bit, at
/// every step — under earlier strokes, an erase stroke as the one painted, an inverted brush, a clip and a crop.
#[test]
fn an_incremental_stroke_is_the_full_raster_at_every_step() {
    let (w, h) = (240, 160);
    let cropped = Geometry {
        crop: edit_model::CropRect {
            left: 0.1,
            top: 0.05,
            right: 0.95,
            bottom: 0.9,
        },
        angle: 2.0,
        ..Default::default()
    };
    for (label, head, last, invert, clip, geometry) in [
        (
            "alone",
            vec![],
            stroke(30, 0.5, 100.0, false),
            false,
            None,
            Geometry::default(),
        ),
        (
            "over two strokes, partial flow",
            vec![stroke(20, 0.45, 60.0, false), stroke(25, 0.55, 40.0, false)],
            stroke(30, 0.5, 70.0, false),
            false,
            None,
            Geometry::default(),
        ),
        (
            "erasing",
            vec![stroke(30, 0.5, 100.0, false)],
            stroke(30, 0.52, 80.0, true),
            false,
            None,
            Geometry::default(),
        ),
        (
            "inverted, clipped, cropped",
            vec![stroke(20, 0.4, 50.0, false)],
            stroke(30, 0.5, 90.0, false),
            true,
            Some((60, 30, 120, 90)),
            cropped,
        ),
    ] {
        let mut state = None;
        let mut painted = 0;
        for n in 1..=last.points.len() {
            let brush = Brush {
                strokes: head
                    .iter()
                    .cloned()
                    .chain(std::iter::once(BrushStroke {
                        points: last.points[..n].to_vec(),
                        ..last.clone()
                    }))
                    .collect(),
                invert,
            };
            let got = rasterize(&mut state, &brush, &geometry, w, h, clip);
            let want = rasterize_in(&brush, &geometry, w, h, clip, None);
            if let Err(e) = same_everywhere(&got, &want, w, h) {
                panic!("{label}, {n} points: {e}");
            }
            painted += usize::from(want.data.iter().any(|&v| v != want.outside));
        }
        assert!(painted > 0, "fixture check: {label} must paint something");
    }
}

/// A stroke that does not GROW — a new stroke begun, an earlier one changed, a different flow — rebuilds rather than
/// splatting onto the old field, and still matches.
#[test]
fn a_change_that_is_not_growth_rebuilds_and_still_matches() {
    let (w, h) = (200, 140);
    let g = Geometry::default();
    let mut state = None;
    let a = Brush {
        strokes: vec![stroke(20, 0.5, 100.0, false)],
        invert: false,
    };
    let _ = rasterize(&mut state, &a, &g, w, h, None);
    for b in [
        // A second stroke begun.
        Brush {
            strokes: vec![stroke(20, 0.5, 100.0, false), stroke(3, 0.3, 100.0, false)],
            invert: false,
        },
        // The same stroke at another flow.
        Brush {
            strokes: vec![stroke(20, 0.5, 50.0, false)],
            invert: false,
        },
        // A shorter stroke (an undo).
        Brush {
            strokes: vec![stroke(10, 0.5, 50.0, false)],
            invert: false,
        },
    ] {
        let got = rasterize(&mut state, &b, &g, w, h, None);
        let want = rasterize_in(&b, &g, w, h, None, None);
        same_everywhere(&got, &want, w, h).unwrap();
    }
}

/// An EARLIER stroke changed while the last one grew — an undo of one stroke under another, say — must rebuild: the
/// kept head is the old one.
#[test]
fn a_changed_earlier_stroke_under_a_growing_one_rebuilds() {
    let (w, h) = (200, 140);
    let g = Geometry::default();
    let mut state = None;
    let a = Brush {
        strokes: vec![
            stroke(20, 0.45, 100.0, false),
            stroke(10, 0.5, 100.0, false),
        ],
        invert: false,
    };
    let _ = rasterize(&mut state, &a, &g, w, h, None);
    let b = Brush {
        strokes: vec![stroke(20, 0.6, 100.0, false), stroke(12, 0.5, 100.0, false)],
        invert: false,
    };
    let got = rasterize(&mut state, &b, &g, w, h, None);
    let want = rasterize_in(&b, &g, w, h, None, None);
    same_everywhere(&got, &want, w, h).unwrap();
}

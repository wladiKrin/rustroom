//! Finding a pupil around a click — **once**, at the click (M9.11, approach A).
//!
//! ⚠⚠ The result is stored as an [`edit_model::Eye`]; the stage never detects. An eye that
//! re-detected on every render would move whenever an upstream edit changed the pixels.
//!
//! ⚠ No `ImageBuf`: the search reads an injected sampler over the red-eye stage's input space —
//! `spot_source`'s design, and the same sampler (`spot_overlay::sampler_for`) feeds it, lens
//! inverse included. No egui: testable headless, like every lib module.
//!
//! ⚠ Redness is `pipeline::redness`, the stage's own definition, so the click and the render
//! cannot disagree about what "red" is.

use edit_model::EyeKind;

/// Grid cells per side of the search window.
pub const GRID: usize = 64;
/// The window's half-width, as a fraction of the long edge.
pub const WINDOW: f32 = 0.03;
/// A seed below this redness is not a red pupil (skin tops out ~0.37, flash pupils start ~0.56 —
/// measured, see `pipeline::redness`).
pub const RED_FLOOR: f32 = 0.50;
/// The fill threshold never drops below this, so the blob cannot leak into flushed skin.
pub const RED_FILL_MIN: f32 = 0.42;
/// A blob whose bounding box is longer than this against its width is not a pupil (a lip).
pub const MAX_ELONGATION: f32 = 2.5;
/// Padding from the blob's equal-area radius to the stored circle.
///
/// ⚠ The blob stops at redness ~0.42, INSIDE a real pupil's soft rim (measured 0.17–0.44 at 10–12
/// px on a 10 px pupil), so 1.2 left the rim at the circle's edge. Padding into the iris is
/// harmless for a red eye — it carries no red — and for a pet eye the circle is still what bounds
/// the correction, so this stays well short of the iris' own edge.
pub const PAD: f32 = 1.35;

/// What a click found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Detection {
    /// A pupil: its centre (normalised) and radius (fraction of the long edge).
    Found {
        centre: [f32; 2],
        radius: f32,
    },
    NotFound,
}

/// Search around `click` (normalised) in a frame of `dims` pixels.
///
/// Scores a `GRID²` window (redness, or luminance for a pet eye), seeds at the best cell of the
/// central quarter, flood-fills the blob, and fits a circle of the blob's area. A blob touching
/// the window's edge, or longer than [`MAX_ELONGATION`], is not a pupil.
pub fn detect(
    click: [f32; 2],
    kind: EyeKind,
    dims: (usize, usize),
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> Detection {
    let (w, h) = (dims.0.max(1) as f32, dims.1.max(1) as f32);
    let long = w.max(h);
    // ⚠ Square in PIXELS, so the window is not squashed on a non-square frame.
    let half = WINDOW * long;
    let step = 2.0 * half / GRID as f32;
    let at = |i: f32, j: f32| -> [f32; 2] {
        let x = click[0] * w - half + (i + 0.5) * step;
        let y = click[1] * h - half + (j + 0.5) * step;
        [x / w, y / h]
    };
    let score: Vec<f32> = (0..GRID * GRID)
        .map(|k| {
            let p = sample(at((k % GRID) as f32, (k / GRID) as f32));
            match kind {
                EyeKind::Red => pipeline::redness(p),
                EyeKind::Pet => color::luminance(p),
            }
        })
        .collect();

    // The seed must clear `floor`; the fill never goes below `fill_min`.
    let (floor, fill_min, factor) = match kind {
        EyeKind::Red => (RED_FLOOR, RED_FILL_MIN, 0.6),
        EyeKind::Pet => {
            // Eyeshine is bright against its window: twice the median, whatever the exposure.
            let mut sorted = score.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let twice_median = 2.0 * sorted[sorted.len() / 2].max(1e-4);
            (twice_median, twice_median, 0.5)
        }
    };

    let (lo, hi) = (GRID / 4, 3 * GRID / 4);
    let Some(seed) = (lo..hi)
        .flat_map(|j| (lo..hi).map(move |i| (i, j)))
        .max_by(|a, b| score[a.1 * GRID + a.0].total_cmp(&score[b.1 * GRID + b.0]))
    else {
        return Detection::NotFound;
    };
    let seed_score = score[seed.1 * GRID + seed.0];
    if seed_score.is_nan() || seed_score < floor {
        return Detection::NotFound;
    }
    let thr = fill_min.max(factor * seed_score);

    // 8-connected flood fill over the grid.
    let mut seen = vec![false; GRID * GRID];
    seen[seed.1 * GRID + seed.0] = true;
    let mut stack = vec![seed];
    let (mut n, mut sx, mut sy) = (0usize, 0.0f32, 0.0f32);
    let (mut minx, mut maxx, mut miny, mut maxy) = (GRID, 0, GRID, 0);
    while let Some((i, j)) = stack.pop() {
        // ⚠ Touching the edge means the blob is bigger than the window: a lip, a shirt, a lamp.
        // Returning here also keeps the neighbour arithmetic below from ever leaving the grid.
        if i == 0 || j == 0 || i == GRID - 1 || j == GRID - 1 {
            return Detection::NotFound;
        }
        n += 1;
        sx += i as f32;
        sy += j as f32;
        (minx, maxx) = (minx.min(i), maxx.max(i));
        (miny, maxy) = (miny.min(j), maxy.max(j));
        for nj in j - 1..=j + 1 {
            for ni in i - 1..=i + 1 {
                let k = nj * GRID + ni;
                if !seen[k] && score[k] >= thr {
                    seen[k] = true;
                    stack.push((ni, nj));
                }
            }
        }
    }
    let (bw, bh) = ((maxx - minx + 1) as f32, (maxy - miny + 1) as f32);
    if bw.max(bh) / bw.min(bh) > MAX_ELONGATION {
        return Detection::NotFound;
    }
    let centre = at(sx / n as f32, sy / n as f32);
    let r_px = PAD * (n as f32 * step * step / std::f32::consts::PI).sqrt();
    Detection::Found {
        centre,
        radius: (r_px / long).clamp(crate::spot_tool::MIN_RADIUS, crate::spot_tool::MAX_RADIUS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIMS: (usize, usize) = (3000, 2000);

    fn pp(srgb: [f32; 3]) -> [f32; 3] {
        color::srgb_to_prophoto(srgb.map(color::srgb_to_linear))
    }

    /// Mid skin with one disc of `pupil` colour, radius `r` px at `c` px. Constructed, not
    /// captured: this proves the search's geometry, not its behaviour on a real face.
    fn frame(c: (f32, f32), r: f32, pupil: [f32; 3]) -> impl Fn([f32; 2]) -> [f32; 3] {
        let (skin, pupil) = (pp([0.78, 0.57, 0.44]), pp(pupil));
        move |p: [f32; 2]| {
            let (x, y) = (p[0] * DIMS.0 as f32, p[1] * DIMS.1 as f32);
            if (x - c.0).hypot(y - c.1) <= r {
                pupil
            } else {
                skin
            }
        }
    }

    fn norm(x: f32, y: f32) -> [f32; 2] {
        [x / DIMS.0 as f32, y / DIMS.1 as f32]
    }

    #[test]
    fn a_red_pupil_is_found_from_an_off_centre_click() {
        let s = frame((1500.0, 1000.0), 25.0, [0.60, 0.10, 0.10]);
        let Detection::Found { centre, radius } =
            detect(norm(1512.0, 994.0), EyeKind::Red, DIMS, &s)
        else {
            panic!("not found")
        };
        let (cx, cy) = (centre[0] * DIMS.0 as f32, centre[1] * DIMS.1 as f32);
        assert!(
            (cx - 1500.0).abs() < 3.0 && (cy - 1000.0).abs() < 3.0,
            "centre {cx},{cy}"
        );
        let rpx = radius * 3000.0;
        assert!(
            (rpx - PAD * 25.0).abs() < 0.2 * PAD * 25.0,
            "radius {rpx}px"
        );
    }

    #[test]
    fn green_eyeshine_is_found_as_a_pet_eye_and_not_as_a_red_one() {
        let s = frame((1500.0, 1000.0), 20.0, [0.55, 0.95, 0.45]);
        assert!(matches!(
            detect(norm(1500.0, 1000.0), EyeKind::Pet, DIMS, &s),
            Detection::Found { .. }
        ));
        assert_eq!(
            detect(norm(1500.0, 1000.0), EyeKind::Red, DIMS, &s),
            Detection::NotFound
        );
    }

    #[test]
    fn plain_skin_finds_nothing() {
        let s = frame((0.0, 0.0), 0.0, [0.0, 0.0, 0.0]);
        assert_eq!(
            detect(norm(1500.0, 1000.0), EyeKind::Red, DIMS, &s),
            Detection::NotFound
        );
        assert_eq!(
            detect(norm(1500.0, 1000.0), EyeKind::Pet, DIMS, &s),
            Detection::NotFound
        );
    }

    /// A lip: red, but a long band crossing the window — rejected rather than circled.
    #[test]
    fn an_elongated_red_band_is_not_a_pupil() {
        let (skin, lip) = (pp([0.78, 0.57, 0.44]), pp([0.60, 0.10, 0.10]));
        let s = move |p: [f32; 2]| {
            if (p[1] * 2000.0 - 1000.0).abs() < 8.0 {
                lip
            } else {
                skin
            }
        };
        assert_eq!(
            detect(norm(1500.0, 1000.0), EyeKind::Red, DIMS, &s),
            Detection::NotFound
        );
    }
}

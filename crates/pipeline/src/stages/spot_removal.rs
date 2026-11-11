//! Spot removal — clone and heal patches, each a disc of pixels copied from elsewhere in the
//! frame at a whole-pixel offset.
//!
//! ⚠⚠ Runs in the **prefix**, after lens correction and dehaze and BEFORE the geometry — which
//! since M7.3b includes the perspective keystone. So a spot lives in **post-lens,
//! pre-perspective** space, NOT the brush's anchor space (post-perspective). It moved there
//! because fusing perspective into `GeometryStage` removed the buffer it used to run on, and a
//! pixel edit cannot live inside a coordinate map. `pipeline::brush::SpotMap` maps it to the
//! canvas. A spot is content-referred (a dust spot is on the sensor, a blemish on the subject), so
//! it must not slide when the crop, the angle **or the keystone** changes; normalising against this
//! frame gives all three for free. After noise reduction, so the patch copies denoised texture into
//! denoised surroundings and the grain matches. ⚠ It still inherits the brush's caveat for lens
//! correction: toggling that after placing a spot shifts it.
//!
//! ⚠ Being in the prefix it always runs whole-buffer — no `Region`, no kernel margin, no
//! `tail_kernel_radius` change. Being normalised it takes no `RenderScale`: preview and export
//! agree exactly, the creative vignette's property.
//!
//! ⚠ **The offset is rounded to whole pixels, deliberately.** A resampled patch is *softer* than
//! its surroundings, and that softness is precisely what makes a patch visible. A dust spot does
//! not need half-pixel placement.
//!
//! ⚠ **Gather first, then blend.** Each spot's source disc is read into a scratch before any of
//! the destination is written. That is what lets a later spot source from an earlier spot's healed
//! area (Lightroom's behaviour, and useful) and what stops a spot ever reading its own half-written
//! pixels when its source overlaps its destination.
//!
//! This is a gather over a disc's bounding box, not a per-pixel map, so `map_pixels_xy` is not
//! the shape. A plain loop; rayon is not worth it for a few thousand pixels per spot.

use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::{Spot, SpotMode};

/// Below this the feather ramp is treated as a hard edge.
///
/// ⚠⚠ **Load-bearing at exactly one input, and the sabotage step is what proves it.** At
/// `feather == 0` the ramp width `f·r` is zero, so `(r − d) / (f·r)` is `+inf` for every `d < r`
/// — which clamps to the *correct* `1.0` — and `0/0 = NaN` at `d == r` exactly, where the clamp
/// passes NaN straight through and the blend writes NaN into the buffer. The same shape as
/// `RadialGradient::FEATHER_EPS`, which is likewise genuinely load-bearing; contrast the brush's
/// `HARDNESS_EPS`, which is a fast path and not a guard.
pub const FEATHER_EPS: f32 = 1e-4;

/// Coverage at distance `d` (in pixels) from the centre of a spot of radius `r` pixels, with the
/// feather in `0..=100`. `1` for `d ≤ (1 − f)·r`, a smoothstep down to `0` at `d = r`, and `0`
/// beyond.
///
/// A non-positive radius covers nothing: a disc with no area is the honest answer, and it is
/// what keeps `f·r == 0` from dividing by zero for a zero-radius spot at a non-zero feather.
pub fn coverage(d: f32, r: f32, feather: f32) -> f32 {
    if r <= 0.0 || d > r {
        return 0.0;
    }
    let f = (feather / 100.0).clamp(0.0, 1.0);
    if f <= FEATHER_EPS {
        return 1.0;
    }
    // Smoothstep rather than a linear ramp: a linear ramp in radial distance leaves a visible
    // crease ring where the derivative jumps — the radial mask's and the vignette's choice.
    let t = ((r - d) / (f * r)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Clone and heal every spot, in order, on the working buffer.
pub struct SpotRemovalStage {
    spots: Vec<Spot>,
}

impl SpotRemovalStage {
    pub fn new(spots: &[Spot]) -> Self {
        Self {
            spots: spots.to_vec(),
        }
    }
}

impl PipelineStage for SpotRemovalStage {
    fn name(&self) -> &'static str {
        "spot_removal"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let mut image = image;
        for spot in &self.spots {
            apply_spot(&mut image, spot);
        }
        image
    }
}

/// The ring whose mean a heal matches: `r < d ≤ RING_OUTER · r`.
const RING_OUTER: f32 = 1.5;

/// One gathered destination pixel: where it is, what the source holds there, and how much of
/// the patch lands on it.
struct Gathered {
    x: usize,
    y: usize,
    src: [f32; 3],
    cov: f32,
}

fn apply_spot(image: &mut ImageBuf, spot: &Spot) {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return;
    }
    let cx = spot.centre[0] * (w - 1) as f32;
    let cy = spot.centre[1] * (h - 1) as f32;
    let sx = spot.source[0] * (w - 1) as f32;
    let sy = spot.source[1] * (h - 1) as f32;
    // ⚠ The long edge, never the width: `centre` is normalised per axis and so lives in an
    // anisotropic space. A radius against width compared against height would make every patch
    // an ellipse — `BrushStroke::radius`'s rule.
    let r = spot.radius * w.max(h) as f32;
    // ⚠ `is_finite` first, so a NaN **or** infinite radius is skipped rather than sizing a
    // bounding box from it. Written this way and not as `!(r > 0.0)` — which says the same thing
    // — because clippy rejects a negated comparison on a partially ordered type, and it is right
    // that the NaN case should be visible rather than implied.
    //
    // ⚠⚠ **This guard is defence in depth, and deleting it entirely leaves every test green** —
    // established by sabotage, and recorded rather than papered over. Three independent
    // mechanisms downstream already produce "a degenerate radius changes nothing": `coverage`
    // returns 0.0 for `r <= 0.0` so the blend is skipped; every comparison against a NaN `r` is
    // false, so no pixel joins the disc or the ring; and a negative `r` inverts the bounding box,
    // which `x0 > x1` rejects. `a_zero_radius_spot_changes_nothing` therefore names a real and
    // guaranteed property that it cannot attribute to *this* line. It stays because relying on
    // three coincidences is not a thing to build on — the `min_filter` clause's precedent.
    if !r.is_finite() || r <= 0.0 {
        return;
    }
    let dx = (sx - cx).round() as i64;
    let dy = (sy - cy).round() as i64;

    // The ring's bounding box contains the disc's, so one sweep classifies every pixel.
    let reach = RING_OUTER * r;
    let x0 = ((cx - reach).floor().max(0.0)) as usize;
    let y0 = ((cy - reach).floor().max(0.0)) as usize;
    let x1 = (((cx + reach).ceil()) as usize).min(w - 1);
    let y1 = (((cy + reach).ceil()) as usize).min(h - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }

    let mut gathered: Vec<Gathered> = Vec::new();
    let mut src_sum = [0.0f32; 3];
    let mut src_n = 0usize;
    let mut ring_sum = [0.0f32; 3];
    let mut ring_n = 0usize;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let ddx = x as f32 - cx;
            let ddy = y as f32 - cy;
            let d = (ddx * ddx + ddy * ddy).sqrt();
            if d <= r {
                // Source pixels outside the image clamp to the edge.
                let px = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                let py = (y as i64 + dy).clamp(0, h as i64 - 1) as usize;
                let src = image.pixel(px, py);
                for c in 0..3 {
                    src_sum[c] += src[c];
                }
                src_n += 1;
                gathered.push(Gathered {
                    x,
                    y,
                    src,
                    cov: coverage(d, r, spot.feather),
                });
            } else if d <= reach {
                // ⚠ The ring is read from the buffer *before* this spot writes anything — which
                // is also the buffer *after* every earlier spot, so a heal beside an earlier heal
                // matches the healed surroundings.
                let p = image.pixel(x, y);
                for c in 0..3 {
                    ring_sum[c] += p[c];
                }
                ring_n += 1;
            }
        }
    }

    // ⚠ Additive in linear light, not multiplicative: what a sky gradient carries across a spot
    // is an offset in exposure, and a ratio would over-correct dark sources. A ring or a source
    // disc with no pixel in it (a sub-pixel radius at a corner) falls back to no shift rather than
    // dividing by zero.
    let shift = match spot.mode {
        SpotMode::Clone => [0.0; 3],
        SpotMode::Heal if src_n > 0 && ring_n > 0 => {
            let mut s = [0.0f32; 3];
            for c in 0..3 {
                s[c] = ring_sum[c] / ring_n as f32 - src_sum[c] / src_n as f32;
            }
            s
        }
        SpotMode::Heal => [0.0; 3],
    };

    for g in gathered {
        if g.cov <= 0.0 {
            continue;
        }
        let dst = image.pixel(g.x, g.y);
        let mut out = [0.0f32; 3];
        for c in 0..3 {
            let patch = match spot.mode {
                SpotMode::Clone => g.src[c],
                // Clamped at 0 only: scene-referred, so there is no upper clamp.
                SpotMode::Heal => (g.src[c] + shift[c]).max(0.0),
            };
            // `dst·(1−cov) + patch·cov` rather than `dst + (patch − dst)·cov`: at full coverage
            // the former is *exactly* the patch, which is what lets a hard-edged clone be tested
            // bit for bit.
            out[c] = dst[c] * (1.0 - g.cov) + patch * g.cov;
        }
        image.set_pixel(g.x, g.y, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-pixel pattern in which every pixel differs from its neighbours in every direction,
    /// so an offset error of one pixel in either axis is visible.
    ///
    /// ⚠ A uniform source cannot see a wrong offset — the spec's own warning — so no clone
    /// test here uses one.
    fn patterned(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / w as f32,
                        y as f32 / h as f32,
                        ((x * y) % 7) as f32 / 7.0,
                    ],
                );
            }
        }
        img
    }

    fn norm(px: usize, n: usize) -> f32 {
        px as f32 / (n - 1) as f32
    }

    fn run(img: ImageBuf, spots: &[Spot]) -> ImageBuf {
        SpotRemovalStage::new(spots).apply(img)
    }

    /// A hard-edged clone on a `w × h` patterned frame, centred on the pixel `centre` with the
    /// source `delta` pixels away. Inside the disc every pixel must be the source pixel, bit for
    /// bit; outside it every pixel must be untouched, bit for bit.
    ///
    /// ⚠ Both halves. One alone passes against a stage that copies the whole image; the other
    /// alone passes against a stage that copies nothing.
    fn check_hard_clone(w: usize, h: usize, centre: (usize, usize), delta: (i64, i64)) {
        let (cxp, cyp) = centre;
        let (dx, dy) = delta;
        let source = ((cxp as i64 + dx) as usize, (cyp as i64 + dy) as usize);
        let spot = Spot {
            decode: false,
            centre: [norm(cxp, w), norm(cyp, h)],
            source: [norm(source.0, w), norm(source.1, h)],
            radius: 0.125,
            feather: 0.0,
            mode: SpotMode::Clone,
            dust: false,
        };
        // Fixture checks: the centre lands on a whole pixel and the radius on a whole number,
        // so `d == r` is reached exactly at the pixel `r` to the right of the centre — the one
        // input where the `FEATHER_EPS` guard is load-bearing. Without these two asserts the
        // rim probe below might silently be testing `d` slightly inside or outside `r`.
        let cx = spot.centre[0] * (w - 1) as f32;
        let cy = spot.centre[1] * (h - 1) as f32;
        let r = spot.radius * w.max(h) as f32;
        assert_eq!(
            cx, cxp as f32,
            "fixture check: the centre x is a whole pixel"
        );
        assert_eq!(
            cy, cyp as f32,
            "fixture check: the centre y is a whole pixel"
        );
        assert_eq!(r, 8.0, "fixture check: the radius is exactly eight pixels");

        let orig = patterned(w, h);
        let out = run(orig.clone(), &[spot]);
        let mut inside = 0;
        for y in 0..h {
            for x in 0..w {
                let ddx = x as f32 - cx;
                let ddy = y as f32 - cy;
                let d = (ddx * ddx + ddy * ddy).sqrt();
                if d <= r {
                    inside += 1;
                    let expect = orig.pixel((x as i64 + dx) as usize, (y as i64 + dy) as usize);
                    assert_eq!(out.pixel(x, y), expect, "({x},{y}) inside the disc, d={d}");
                } else {
                    assert_eq!(
                        out.pixel(x, y),
                        orig.pixel(x, y),
                        "({x},{y}) outside the disc"
                    );
                }
            }
        }
        // The rim pixel is inside by construction; a disc of radius 8 has ~200 pixels.
        assert!(inside > 150, "fixture check: {inside} pixels inside");
        let rim = out.pixel(cxp + 8, cyp);
        assert!(
            rim.iter().all(|v| v.is_finite()),
            "the pixel exactly on the rim must not be NaN: {rim:?}"
        );
    }

    #[test]
    fn a_hard_edged_clone_copies_the_disc_exactly_and_nothing_else() {
        // Diagonal offsets throughout: a zero component is a blind spot for a swapped axis.
        check_hard_clone(64, 64, (21, 21), (20, 9));
    }

    /// ⚠ Both orientations, because a radius scaled by the width is inert on a landscape frame
    /// and one scaled by the height is inert on a portrait one — the sabotage that catches
    /// `radius · w` needs the frame whose long edge is *not* the width.
    #[test]
    fn the_radius_is_a_fraction_of_the_long_edge_on_either_orientation() {
        check_hard_clone(64, 32, (21, 10), (20, 2));
        check_hard_clone(32, 64, (21, 21), (-12, 20));
    }

    /// Left half a fine checker (mean 0.10 per channel), right half a flat tinted colour. A spot on
    /// the right sourcing from the left must, healed, land with the ring's mean; cloned, with the
    /// checker's.
    fn split_frame() -> (ImageBuf, [f32; 3]) {
        const W: usize = 64;
        const H: usize = 64;
        let right = [0.8f32, 0.6, 0.4];
        let mut img = ImageBuf::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let px = if x < W / 2 {
                    let v = if (x / 2 + y / 2) % 2 == 0 { 0.05 } else { 0.15 };
                    [v, v, v]
                } else {
                    right
                };
                img.set_pixel(x, y, px);
            }
        }
        (img, right)
    }

    fn disc_pixels(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let ddx = x as f32 - cx;
                let ddy = y as f32 - cy;
                if (ddx * ddx + ddy * ddy).sqrt() <= r {
                    v.push((x, y));
                }
            }
        }
        v
    }

    fn mean_over(img: &ImageBuf, px: &[(usize, usize)]) -> [f32; 3] {
        let mut s = [0.0f32; 3];
        for &(x, y) in px {
            let p = img.pixel(x, y);
            for c in 0..3 {
                s[c] += p[c];
            }
        }
        s.map(|v| v / px.len() as f32)
    }

    #[test]
    fn heal_matches_the_ring_where_clone_does_not() {
        let (orig, right) = split_frame();
        let (w, h) = (orig.width(), orig.height());
        let centre = (48usize, 32usize);
        let source = (16usize, 32usize);
        let mk = |mode| Spot {
            decode: false,
            centre: [norm(centre.0, w), norm(centre.1, h)],
            source: [norm(source.0, w), norm(source.1, h)],
            radius: 6.0 / 64.0,
            feather: 0.0,
            mode,
            dust: false,
        };
        let cx = mk(SpotMode::Heal).centre[0] * (w - 1) as f32;
        let cy = mk(SpotMode::Heal).centre[1] * (h - 1) as f32;
        let r = 6.0;
        let dst_disc = disc_pixels(w, h, cx, cy, r);
        let src_disc: Vec<(usize, usize)> = dst_disc
            .iter()
            .map(|&(x, y)| (x - (centre.0 - source.0), y))
            .collect();
        let src_mean = mean_over(&orig, &src_disc);
        assert!(
            (src_mean[0] - 0.1).abs() < 0.03,
            "fixture check: the checker averages near 0.1, got {src_mean:?}"
        );

        let healed = run(orig.clone(), &[mk(SpotMode::Heal)]);
        let heal_mean = mean_over(&healed, &dst_disc);
        for c in 0..3 {
            assert!(
                (heal_mean[c] - right[c]).abs() < 1e-5,
                "healed disc mean {heal_mean:?} must equal the ring's {right:?}"
            );
        }
        // Per pixel: the source pixel plus one constant shift per channel, so the texture of the
        // source survives and only its level moves.
        for (&(x, y), &(sx, sy)) in dst_disc.iter().zip(&src_disc) {
            let got = healed.pixel(x, y);
            let src = orig.pixel(sx, sy);
            for c in 0..3 {
                let want = src[c] + (right[c] - src_mean[c]);
                assert!(
                    (got[c] - want).abs() < 1e-5,
                    "({x},{y}) channel {c}: got {} want {want}",
                    got[c]
                );
            }
        }

        let cloned = run(orig.clone(), &[mk(SpotMode::Clone)]);
        let clone_mean = mean_over(&cloned, &dst_disc);
        for c in 0..3 {
            assert!(
                (clone_mean[c] - src_mean[c]).abs() < 1e-5,
                "cloned disc mean {clone_mean:?} must equal the source's {src_mean:?}"
            );
            // ⚠ 0.2, not 0.3: the blue channel's true gap is 0.4 − 0.1 = **exactly** 0.30, so a
            // strict `> 0.3` asserted something false about correct code and failed on first run.
            // The threshold only has to be far above the 1e-5 the two equalities above use.
            assert!(
                (clone_mean[c] - right[c]).abs() > 0.2,
                "clone must be visibly different from heal, or the two modes are one"
            );
        }
    }

    #[test]
    fn a_later_spot_reads_an_earlier_spots_result() {
        const W: usize = 64;
        const H: usize = 64;
        let mut img = ImageBuf::new(W, H);
        for y in 0..H {
            for x in 0..W {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        let a = (16usize, 32usize);
        let b = (48usize, 32usize);
        for &(cx, cy) in &[a, b] {
            for y in cy - 3..=cy + 3 {
                for x in cx - 3..=cx + 3 {
                    if ((x as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)).sqrt()
                        <= 3.0
                    {
                        img.set_pixel(x, y, [0.0, 0.0, 0.0]);
                    }
                }
            }
        }
        let spot = |centre: (usize, usize), source: (usize, usize)| Spot {
            decode: false,
            centre: [norm(centre.0, W), norm(centre.1, H)],
            source: [norm(source.0, W), norm(source.1, H)],
            radius: 6.0 / 64.0,
            feather: 0.0,
            mode: SpotMode::Clone,
            dust: false,
        };
        // Spot 1 cleans A from a clean area; spot 2 cleans B *from A*.
        let first = spot(a, (16, 10));
        let second = spot(b, a);

        let out = run(img.clone(), &[first, second]);
        assert_eq!(
            out.pixel(b.0, b.1),
            [0.5, 0.5, 0.5],
            "B must be cleaned from A's already-healed pixels"
        );
        assert_eq!(out.pixel(a.0, a.1), [0.5, 0.5, 0.5]);

        // ⚠ The contrast that makes the order observable: run B's spot first and it copies A's
        // dust. Without this half a stage reading the *original* buffer for every spot would
        // fail the assertion above, but so would one applying spots in reverse — this pins that
        // "in order" means the sidecar's order.
        let reversed = run(img, &[second, first]);
        assert_eq!(reversed.pixel(b.0, b.1), [0.0, 0.0, 0.0]);
    }

    /// The source sits up and to the **left** of the destination, so an in-place loop in scan
    /// order would read pixels it had just written. Gathering first means every destination pixel
    /// is the *original* source pixel.
    #[test]
    fn an_overlapping_source_is_gathered_before_anything_is_written() {
        const W: usize = 64;
        const H: usize = 64;
        let orig = patterned(W, H);
        let centre = (32usize, 32usize);
        let (dx, dy) = (-4i64, -3i64);
        let spot = Spot {
            decode: false,
            centre: [norm(centre.0, W), norm(centre.1, H)],
            source: [
                norm((centre.0 as i64 + dx) as usize, W),
                norm((centre.1 as i64 + dy) as usize, H),
            ],
            radius: 0.125,
            feather: 0.0,
            mode: SpotMode::Clone,
            dust: false,
        };
        let cx = spot.centre[0] * (W - 1) as f32;
        let cy = spot.centre[1] * (H - 1) as f32;
        let out = run(orig.clone(), &[spot]);
        let disc = disc_pixels(W, H, cx, cy, 8.0);
        assert!(disc.len() > 150, "fixture check");
        for (x, y) in disc {
            let want = orig.pixel((x as i64 + dx) as usize, (y as i64 + dy) as usize);
            assert_eq!(out.pixel(x, y), want, "({x},{y})");
        }
    }

    #[test]
    fn a_source_off_the_image_clamps_to_the_edge() {
        const W: usize = 32;
        const H: usize = 32;
        let orig = patterned(W, H);
        let centre = (16usize, 16usize);
        let mk = |source: [f32; 2]| Spot {
            decode: false,
            centre: [norm(centre.0, W), norm(centre.1, H)],
            source,
            radius: 6.0 / 32.0,
            feather: 0.0,
            mode: SpotMode::Clone,
            dust: false,
        };
        let cx = 16.0;
        let disc = disc_pixels(W, H, cx, cx, 6.0);
        assert!(disc.len() > 100, "fixture check");

        // Far off the right edge, four rows down: every source column clamps to the last one.
        let out = run(orig.clone(), &[mk([1.6, norm(20, H)])]);
        for &(x, y) in &disc {
            assert_eq!(out.pixel(x, y), orig.pixel(W - 1, y + 4), "({x},{y}) right");
        }
        // Far above the top edge, three columns right: every source row clamps to the first.
        let out = run(orig.clone(), &[mk([norm(19, W), -0.9])]);
        for &(x, y) in &disc {
            assert_eq!(out.pixel(x, y), orig.pixel(x + 3, 0), "({x},{y}) top");
        }
    }

    #[test]
    fn the_feather_ramps_monotonically_from_the_inner_radius_to_the_rim() {
        let r = 8.0;
        for feather in [10.0, 40.0, 70.0, 100.0] {
            let f = feather / 100.0;
            let mut prev = f32::INFINITY;
            for i in 0..=240 {
                let d = 1.2 * r * i as f32 / 240.0;
                let c = coverage(d, r, feather);
                assert!(
                    c <= prev,
                    "coverage rose from {prev} to {c} at d={d}, feather {feather}"
                );
                assert!((0.0..=1.0).contains(&c));
                if d <= (1.0 - f) * r {
                    assert_eq!(
                        c, 1.0,
                        "full inside the inner radius, d={d} feather {feather}"
                    );
                }
                if d >= r {
                    assert_eq!(
                        c, 0.0,
                        "nothing at or beyond the rim, d={d} feather {feather}"
                    );
                }
                prev = c;
            }
        }
        // Smoothstep, not a linear ramp: three quarters of the way in is 0.84375, not 0.75.
        assert!((coverage(5.0, 8.0, 50.0) - 0.84375).abs() < 1e-6);
    }

    /// ⚠⚠ The one input the `FEATHER_EPS` guard exists for. Either side of the rim the
    /// unguarded expression is `±inf`, which clamps to the correct answer — IEEE infinity hides
    /// a missing guard exactly as `feather_zero_is_a_hard_edge` in `radial` records. Only
    /// `d == r` is `0/0`.
    #[test]
    fn feather_zero_is_a_hard_edge_even_exactly_on_the_rim() {
        assert_eq!(coverage(8.0, 8.0, 0.0), 1.0);
        assert_eq!(coverage(7.9, 8.0, 0.0), 1.0);
        assert_eq!(coverage(8.1, 8.0, 0.0), 0.0);
        // A zero radius covers nothing, and does so without a division by zero at any feather.
        assert_eq!(coverage(0.0, 0.0, 0.0), 0.0);
        assert_eq!(coverage(0.0, 0.0, 50.0), 0.0);
    }

    #[test]
    fn a_zero_radius_spot_changes_nothing() {
        let orig = patterned(32, 32);
        for radius in [0.0, -0.1, f32::NAN] {
            let out = run(
                orig.clone(),
                &[Spot {
                    decode: false,
                    centre: [0.5, 0.5],
                    source: [0.2, 0.2],
                    radius,
                    feather: 50.0,
                    mode: SpotMode::Heal,
                    dust: false,
                }],
            );
            assert_eq!(out.data(), orig.data(), "radius {radius}");
        }
    }
}

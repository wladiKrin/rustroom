//! Splatting strokes into a coverage map.

use super::FrameMap;
use edit_model::{Brush, Geometry};

/// A rasterized brush mask: 8-bit coverage over a bounding box in **frame** pixels.
///
/// ⚠ Bounded by what was actually painted, and that is the representation rather than an
/// optimisation bolted on. A full-frame map at full resolution is 6246x4170 = 26 MB per mask, and
/// several masks would run to hundreds. Bounding makes memory proportional to painted area *and*
/// gives `MaskStage` an early-out for every pixel outside the box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoverageMap {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// `w * h` samples.
    pub data: Vec<u8>,
}

impl CoverageMap {
    /// Coverage at a **frame** pixel. Everything outside the box reads 0.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return 0;
        }
        self.data[(y - self.y0) * self.w + (x - self.x0)]
    }

    /// Whether this map covers nothing at all.
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Dabs are laid this fraction of a radius apart along a stroke, so a fast drag leaves no gaps.
const SPACING: f32 = 0.25;
/// Below this a hardness is treated as a hard edge.
///
/// ⚠ **This is a fast path, NOT a NaN guard**, and the spec claimed otherwise. `0.0 / 0.0` in the
/// ramp needs exactly `q == 1 && hard == 1`, and the `q >= 1.0` early return above already
/// excludes `q == 1`. For any `q < 1` with `hard == 1` the division yields `+inf`, which clamps to
/// `1.0` — the correct answer. Verified by deleting it and watching every test still pass.
///
/// Contrast `RadialGradient::FEATHER_EPS`, which genuinely is load-bearing: that ramp has no
/// equivalent early return, so its boundary really does reach `0.0 / 0.0`.
const HARDNESS_EPS: f32 = 1e-4;
/// A radius below this in frame pixels cannot paint anything visible.
const MIN_RADIUS_PX: f32 = 0.5;

/// One splat: centre, radius and profile in frame pixels.
struct Dab {
    x: f32,
    y: f32,
    r: f32,
    hard: f32,
    erase: bool,
}

/// Rasterize a brush into a coverage map over the frame.
pub fn rasterize(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
) -> CoverageMap {
    if brush.strokes.is_empty() || frame_w == 0 || frame_h == 0 {
        return CoverageMap::default();
    }
    let map = FrameMap::new(geometry, frame_w, frame_h);

    // Every dab in frame pixels, so the bounding box is known before anything is allocated.
    let mut dabs: Vec<Dab> = Vec::new();
    for s in &brush.strokes {
        let r = (s.radius * map.radius_scale).max(MIN_RADIUS_PX);
        let hard = (s.hardness / 100.0).clamp(0.0, 1.0);
        let pts: Vec<(f32, f32)> = s.points.iter().map(|p| map.point(*p)).collect();
        let Some(first) = pts.first() else { continue };
        // ⚠ A single point still paints one dab: a click without a drag is one point, so the
        // interpolation below must not be the only thing that emits dabs.
        dabs.push(Dab {
            x: first.0,
            y: first.1,
            r,
            hard,
            erase: s.erase,
        });
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            let step = (r * SPACING).max(0.5);
            let n = ((len / step).ceil() as usize).max(1);
            for i in 1..=n {
                let t = i as f32 / n as f32;
                dabs.push(Dab {
                    x: a.0 + dx * t,
                    y: a.1 + dy * t,
                    r,
                    hard,
                    erase: s.erase,
                });
            }
        }
    }
    if dabs.is_empty() {
        return CoverageMap::default();
    }

    // Bounding box, clipped to the frame.
    let (mut lx, mut ly, mut hx, mut hy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for d in &dabs {
        lx = lx.min(d.x - d.r);
        ly = ly.min(d.y - d.r);
        hx = hx.max(d.x + d.r);
        hy = hy.max(d.y + d.r);
    }
    let x0 = lx.floor().clamp(0.0, frame_w as f32) as usize;
    let y0 = ly.floor().clamp(0.0, frame_h as f32) as usize;
    let x1 = ((hx.ceil().clamp(0.0, frame_w as f32) as usize) + 1).min(frame_w);
    let y1 = ((hy.ceil().clamp(0.0, frame_h as f32) as usize) + 1).min(frame_h);
    if x1 <= x0 || y1 <= y0 {
        return CoverageMap::default();
    }
    let (w, h) = (x1 - x0, y1 - y0);

    let mut cov = vec![0.0f32; w * h];
    for d in &dabs {
        // Only the rows and columns this dab can reach.
        let dx0 = (d.x - d.r).floor().clamp(x0 as f32, x1 as f32) as usize;
        let dy0 = (d.y - d.r).floor().clamp(y0 as f32, y1 as f32) as usize;
        let dx1 = (((d.x + d.r).ceil().clamp(0.0, x1 as f32) as usize) + 1).min(x1);
        let dy1 = (((d.y + d.r).ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1);
        for y in dy0..dy1 {
            for x in dx0..dx1 {
                let dist =
                    (((x as f32 + 0.5) - d.x).powi(2) + ((y as f32 + 0.5) - d.y).powi(2)).sqrt();
                let a = dab_alpha(dist, d.r, d.hard);
                if a <= 0.0 {
                    continue;
                }
                let slot = &mut cov[(y - y0) * w + (x - x0)];
                // ⚠ `max`, not accumulation. Accumulation would make a slow stroke darker than a
                // fast one over the same path, because consecutive dabs overlap — the
                // stroke-overlap bookkeeping that buys nothing until flow exists, and is why
                // flow is a later milestone.
                if d.erase {
                    *slot = slot.min(1.0 - a);
                } else {
                    *slot = slot.max(a);
                }
            }
        }
    }

    let invert = brush.invert;
    let data = cov
        .iter()
        .map(|c| {
            let c = if invert { 1.0 - *c } else { *c };
            // ⚠ NaN reaches 0 through the cast, not the clamp: `f32::clamp` propagates NaN while
            // Rust's float-to-int casts saturate. The same note `ImageBuf::quantize` carries.
            (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        })
        .collect();

    CoverageMap { x0, y0, w, h, data }
}

/// A dab's alpha at distance `d` from its centre.
///
/// Smoothstep from 1 at `d/r = hard` to 0 at `d/r = 1` — the mirror of `RadialGradient`'s feather,
/// with `hard = 1 - feather`.
fn dab_alpha(d: f32, r: f32, hard: f32) -> f32 {
    if r <= 0.0 {
        return 0.0;
    }
    let q = d / r;
    if q >= 1.0 {
        return 0.0;
    }
    // A fast path only — see `HARDNESS_EPS`. Removing it changes no output.
    if hard >= 1.0 - HARDNESS_EPS {
        return 1.0;
    }
    let t = ((1.0 - q) / (1.0 - hard)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{BrushStroke, CropRect};

    fn dot(x: f32, y: f32, radius: f32, hardness: f32, erase: bool) -> BrushStroke {
        BrushStroke {
            points: vec![[x, y]],
            radius,
            hardness,
            erase,
        }
    }

    fn brush(strokes: Vec<BrushStroke>) -> Brush {
        Brush {
            strokes,
            invert: false,
        }
    }

    fn flat(strokes: Vec<BrushStroke>) -> CoverageMap {
        rasterize(&brush(strokes), &Geometry::default(), 80, 40)
    }

    #[test]
    fn an_empty_brush_covers_nothing() {
        // A mask added but not yet painted must be inert.
        let m = rasterize(&brush(vec![]), &Geometry::default(), 80, 40);
        assert!(m.is_empty());
        assert_eq!(m.at(10, 10), 0);
    }

    #[test]
    fn a_single_point_stroke_paints_one_dab() {
        // ⚠ A click without a drag is one point. Interpolation must not be the only source of
        // dabs.
        let m = flat(vec![dot(0.5, 0.5, 0.1, 100.0, false)]);
        assert_eq!(m.at(40, 20), 255, "the centre must be fully covered");
        assert_eq!(m.at(2, 2), 0, "far away must be untouched");
    }

    #[test]
    fn a_dab_is_round_on_a_wide_frame() {
        // ⚠⚠ The aspect trap in its fourth guise. On an 80x40 frame a radius of 0.125 of the long
        // edge is 10 px in BOTH directions. Measured against width it would be 10 px across and
        // 10/40*80 = 20 px down — an ellipse.
        let m = flat(vec![dot(0.5, 0.5, 0.125, 100.0, false)]);
        assert_eq!(m.at(47, 20), 255, "7 px right must be covered");
        assert_eq!(m.at(40, 27), 255, "7 px down must be covered");
        assert_eq!(m.at(52, 20), 0, "12 px right must be outside");
        assert_eq!(m.at(40, 32), 0, "12 px down must be outside");

        // ⚠⚠ **The landscape case above cannot detect the bug it names**, and this one is why it
        // is not enough on its own. On an 80x40 frame the long edge IS the width, so scaling the
        // radius by `frame_w` instead of the long edge is a no-op — the sabotage passed. On a
        // PORTRAIT frame they differ: the long edge is 80 (the height) while the width is 40, so
        // scaling by width would halve every radius.
        let tall = rasterize(
            &brush(vec![dot(0.5, 0.5, 0.125, 100.0, false)]),
            &Geometry::default(),
            40,
            80,
        );
        // Radius is 0.125 * 80 = 10 px. Scaled by the width it would be 5 px, so 7 px out
        // discriminates: covered when correct, clear when wrong.
        assert_eq!(tall.at(27, 40), 255, "7 px right on a tall frame");
        assert_eq!(tall.at(20, 47), 255, "7 px down on a tall frame");
        assert_eq!(tall.at(20, 52), 0, "12 px down must still be outside");
    }

    #[test]
    fn a_soft_dab_ramps_from_the_centre_outward() {
        // ⚠ The centre is NOT asserted to be exactly 255, and that is geometry rather than
        // slack: normalised 0.5 of an 80 px frame is x = 40.0, which is the *boundary* between
        // pixels 39 and 40, while pixel centres sit at x + 0.5. So no pixel centre coincides with
        // the dab centre and the peak sample is 254. The real property is monotonic falloff.
        let m = flat(vec![dot(0.5, 0.5, 0.25, 0.0, false)]);
        let row: Vec<u8> = (40..60).map(|x| m.at(x, 20)).collect();
        assert!(row[0] >= 250, "the centre must be near-full: {}", row[0]);
        for w in row.windows(2) {
            assert!(
                w[1] <= w[0],
                "coverage must not rise going outward: {row:?}"
            );
        }
        assert_eq!(*row.last().unwrap(), 0, "the outer edge must reach zero");
        assert!(
            row.iter().any(|&v| v > 0 && v < 250),
            "there must be a genuine ramp, not a hard step: {row:?}"
        );
    }

    #[test]
    fn a_hard_brush_has_no_partial_coverage() {
        // ⚠⚠ The guard is only reachable at exactly `d == r`: on either side IEEE arithmetic
        // gives the right answer anyway (+inf and -inf both clamp correctly). Probing either side
        // of the boundary would pass with the guard deleted — the exact mistake M3.1b's
        // `feather_zero_is_a_hard_edge` made and had to be corrected for.
        //
        // A radius of 0.125 of an 80 px long edge is exactly 10 px, and pixel centres sit at
        // x + 0.5, so the sample at x = 50 is exactly 10.0 px from a centre at 40.5... which is
        // why the centre is placed to make the distance land on r exactly.
        let m = rasterize(
            &brush(vec![BrushStroke {
                points: vec![[0.50625, 0.5125]],
                radius: 0.125,
                hardness: 100.0,
                erase: false,
            }]),
            &Geometry::default(),
            80,
            40,
        );
        // Centre maps to (40.5, 20.5); the sample at (50, 20) has centre (50.5, 20.5), exactly
        // 10.0 px away — the boundary.
        let on_boundary = m.at(50, 20);
        assert!(
            on_boundary == 255 || on_boundary == 0,
            "the boundary produced {on_boundary}, which is neither covered nor clear — a NaN \
             reached the quantizer"
        );
    }

    #[test]
    fn erase_after_paint_removes_coverage() {
        let m = flat(vec![
            dot(0.5, 0.5, 0.2, 100.0, false),
            dot(0.5, 0.5, 0.1, 100.0, true),
        ]);
        assert_eq!(m.at(40, 20), 0, "the erased core must be clear");
        assert_eq!(m.at(52, 20), 255, "outside the eraser must survive");
    }

    #[test]
    fn paint_after_erase_restores_coverage() {
        // Order matters, so an eraser is not terminal.
        let m = flat(vec![
            dot(0.5, 0.5, 0.2, 100.0, false),
            dot(0.5, 0.5, 0.1, 100.0, true),
            dot(0.5, 0.5, 0.05, 100.0, false),
        ]);
        assert_eq!(m.at(40, 20), 255);
    }

    #[test]
    fn overlapping_dabs_in_one_stroke_do_not_double_up() {
        // ⚠ `max`, not accumulation. Without it a slow stroke would be darker than a fast one
        // over the same path, which is the stroke-overlap problem that made flow a later
        // milestone.
        let m = flat(vec![BrushStroke {
            points: vec![[0.3, 0.5], [0.7, 0.5]],
            radius: 0.1,
            hardness: 0.0,
            erase: false,
        }]);
        // ⚠⚠ The sample is compared against a SINGLE dab at the same place, not against a
        // range. An earlier version asserted only `> 0 && < 250` on a perpendicular sample —
        // but accumulation there sums to about 0.71, i.e. 181, which is inside that band. It
        // passed with `max` replaced by `+=`. Equality with one dab is what actually
        // discriminates: `max` reproduces the single-dab profile exactly, accumulation cannot.
        //
        // The probe is perpendicular to the stroke: on the axis every sample is 255 either way.
        // The stroke's dab spacing is 8 px * 0.25 = 2 px over x = 24..56, so one dab lands
        // exactly on (40, 20) — the same place the single dab below sits.
        let one = flat(vec![dot(0.5, 0.5, 0.1, 0.0, false)]);
        assert_eq!(
            m.at(40, 14),
            one.at(40, 14),
            "a long stroke must match a single dab off-axis; accumulation would exceed it"
        );
        assert!(
            one.at(40, 14) > 0,
            "the probe must actually be inside the dab, or this proves nothing"
        );
    }

    #[test]
    fn dabs_do_not_gap_on_a_fast_stroke() {
        // Two points far apart, interpolated. Every pixel along the segment must be covered.
        let m = flat(vec![BrushStroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            radius: 0.05,
            hardness: 100.0,
            erase: false,
        }]);
        for x in 12..68 {
            assert_eq!(m.at(x, 20), 255, "gap at x = {x}");
        }
    }

    #[test]
    fn the_bbox_bounds_the_painted_region() {
        // ⚠ The representation, not an optimisation: a full-frame map at full resolution is
        // 26 MB per mask.
        let m = flat(vec![dot(0.5, 0.5, 0.05, 100.0, false)]);
        assert!(m.w < 20 && m.h < 20, "bbox {}x{} is not bounded", m.w, m.h);
        assert!(
            m.x0 > 20 && m.y0 > 5,
            "bbox origin ({}, {}) is wrong",
            m.x0,
            m.y0
        );
    }

    #[test]
    fn coverage_outside_the_bbox_reads_zero() {
        let m = flat(vec![dot(0.5, 0.5, 0.05, 100.0, false)]);
        assert_eq!(m.at(0, 0), 0);
        assert_eq!(m.at(79, 39), 0);
        assert_eq!(m.at(1000, 1000), 0, "far out of range must not panic");
    }

    #[test]
    fn a_stroke_near_the_edge_is_clipped_not_wrapped() {
        let m = flat(vec![dot(0.02, 0.02, 0.1, 100.0, false)]);
        assert_eq!(m.at(1, 1), 255);
        assert_eq!(m.at(79, 39), 0, "the far corner must not be painted");
    }

    #[test]
    fn a_stroke_survives_a_crop() {
        // ⚠⚠ The load-bearing test of the whole spec, and the natural wrong version of it fails
        // against a CORRECT implementation. Asserting "the bbox is unchanged" would be wrong —
        // the frame moved, that is the point. Assert instead that the dab lands on the SAME IMAGE
        // CONTENT: source (0.5, 0.5) is the middle of the photo, so it must sit at the point of
        // the frame corresponding to source (0.5, 0.5) whatever the crop.
        let strokes = vec![dot(0.5, 0.5, 0.05, 100.0, false)];
        let g = Geometry {
            crop: CropRect {
                left: 0.25,
                top: 0.25,
                right: 0.75,
                bottom: 0.75,
            },
            ..Geometry::default()
        };
        // The cropped frame is half the size on each axis, and source (0.5, 0.5) is its centre.
        let cropped = rasterize(&brush(strokes.clone()), &g, 40, 20);
        assert_eq!(
            cropped.at(20, 10),
            255,
            "the stroke must still be on the same content"
        );
        // Uncropped, the same source point is the centre of the full frame.
        let full = rasterize(&brush(strokes), &Geometry::default(), 80, 40);
        assert_eq!(full.at(40, 20), 255);
    }

    #[test]
    fn an_eight_bit_edge_has_no_visible_banding() {
        // ⚠ Measured rather than assumed. The worst case is the softest possible edge across the
        // widest practical radius: consecutive samples must never jump by much more than one
        // step, which is below the visibility threshold for a smooth gradient.
        let radius_px: f32 = 0.4 * 80.0;
        let m = flat(vec![dot(0.5, 0.5, 0.4, 0.0, false)]);
        let mut worst = 0i32;
        for x in 40..71 {
            let a = m.at(x, 20) as i32;
            let b = m.at(x + 1, 20) as i32;
            worst = worst.max((a - b).abs());
        }
        // ⚠ The bound is the RAMP'S OWN analytic slope plus one quantization step, not a round
        // number. A smoothstep's steepest gradient is 1.5x its average, so over `radius_px` the
        // ramp inherently steps `255 * 1.5 / radius_px` per pixel — about 12 here. An earlier
        // version asserted `<= 8`, a threshold picked without computing it, and failed against a
        // correct implementation. What quantization may add on top is one step.
        let bound = (255.0 * 1.5 / radius_px).ceil() as i32 + 1;
        assert!(
            worst <= bound,
            "worst adjacent step was {worst}/255, above the ramp's own bound of {bound}"
        );
    }
}

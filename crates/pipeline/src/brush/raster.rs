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
    /// Coverage **everywhere outside the box**.
    ///
    /// ⚠⚠ Load-bearing, and its absence was a real bug found in use. An **inverted** brush covers
    /// the whole frame *except* what was painted — but the box only spans what was painted, so
    /// returning 0 outside it confined the inversion to a rectangle around the strokes, which
    /// grew as more was painted. Storing the outside value keeps the box's memory win (a
    /// full-frame map is 26 MB at full resolution) while making the map total.
    pub outside: u8,
}

impl CoverageMap {
    /// Coverage at a **frame** pixel. Everything outside the box reads 0.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return self.outside;
        }
        self.data[(y - self.y0) * self.w + (x - self.x0)]
    }

    /// Whether this map covers nothing at all — no box *and* nothing outside it.
    ///
    /// ⚠ Not merely "the box is empty": an inverted brush with no strokes has no box and covers
    /// **everything**, so a box-only test would call it inert and skip a stage that must run.
    pub fn is_inert(&self) -> bool {
        (self.w == 0 || self.h == 0) && self.outside == 0
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
}

/// One stroke's dabs, with the properties that apply to the stroke as a whole.
///
/// ⚠ Dabs are grouped **per stroke** rather than flattened, and that grouping is what makes flow
/// correct: within a stroke the dabs combine with `max`, so a slow drag is not darker than a fast
/// one over the same path; across strokes the results composite, so a second pass builds up.
/// Flattening would force one rule to serve both.
struct Stroke {
    dabs: Vec<Dab>,
    flow: f32,
    erase: bool,
}

/// Rasterize a brush into a coverage map over the whole frame.
///
/// The uncached path: the CLI, every export, and the fit view.
pub fn rasterize(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
) -> CoverageMap {
    rasterize_in(brush, geometry, frame_w, frame_h, None)
}

/// Rasterize only what falls inside `clip` — the rendered region, as `(x0, y0, w, h)` in **frame
/// pixels**.
///
/// ⚠ **`FrameMap` is untouched and dab positions are unchanged.** Only the output box is clipped,
/// so a clipped map and a whole-frame one agree exactly at every frame coordinate inside the clip.
/// That is the property that makes this safe, and `a_clipped_raster_matches_the_whole_frame_one`
/// is the guard.
///
/// ⚠ `None` rasterizes the whole frame and is byte-identical to the pre-clip behaviour — which is
/// why no golden moved when this landed.
pub fn rasterize_in(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap {
    // ⚠ An inverted brush with no strokes covers everything, not nothing.
    let outside = if brush.invert { 255 } else { 0 };
    if brush.strokes.is_empty() || frame_w == 0 || frame_h == 0 {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }
    // The clip in frame pixels, clamped to the frame. `None` is the whole frame.
    let (cx0, cy0, cx1, cy1) = match clip {
        Some((x, y, w, h)) => (
            x.min(frame_w),
            y.min(frame_h),
            (x + w).min(frame_w),
            (y + h).min(frame_h),
        ),
        None => (0, 0, frame_w, frame_h),
    };
    if cx1 <= cx0 || cy1 <= cy0 {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }
    let map = FrameMap::new(geometry, frame_w, frame_h);

    // Every dab in frame pixels, grouped by stroke, so the bounding box is known before anything
    // is allocated.
    let mut strokes: Vec<Stroke> = Vec::new();
    for s in &brush.strokes {
        let r = (s.radius * map.radius_scale).max(MIN_RADIUS_PX);
        let hard = (s.hardness / 100.0).clamp(0.0, 1.0);
        let pts: Vec<(f32, f32)> = s.points.iter().map(|p| map.point(*p)).collect();
        let Some(first) = pts.first() else { continue };
        // ⚠ A single point still paints one dab: a click without a drag is one point, so the
        // interpolation below must not be the only thing that emits dabs.
        // A dab entirely outside the clip cannot affect any pixel inside it, so it is dropped
        // before it can widen the bounding box or cost a buffer. This is where the saving comes
        // from on a long stroke that runs off-screen — the common case at 1:1.
        //
        // ⚠ **An optimisation, not a correctness guard** — verified by replacing this with
        // `|_, _| true` and watching every test still pass. The bounding box is clamped to the
        // clip regardless, so an out-of-clip dab would contribute nothing to the output anyway;
        // what the skip saves is the box being widened to the stroke's full extent and both f32
        // buffers being sized for it. `a_dab_entirely_outside_the_clip_contributes_nothing`
        // therefore guards the *output*, not this branch.
        let keep = |x: f32, y: f32| {
            !(x + r < cx0 as f32
                || x - r >= cx1 as f32
                || y + r < cy0 as f32
                || y - r >= cy1 as f32)
        };
        let mut dabs: Vec<Dab> = Vec::new();
        if keep(first.0, first.1) {
            dabs.push(Dab {
                x: first.0,
                y: first.1,
                r,
                hard,
            });
        }
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            let step = (r * SPACING).max(0.5);
            let n = ((len / step).ceil() as usize).max(1);
            for i in 1..=n {
                let t = i as f32 / n as f32;
                let (px, py) = (a.0 + dx * t, a.1 + dy * t);
                if keep(px, py) {
                    dabs.push(Dab {
                        x: px,
                        y: py,
                        r,
                        hard,
                    });
                }
            }
        }
        strokes.push(Stroke {
            dabs,
            flow: (s.flow / 100.0).clamp(0.0, 1.0),
            erase: s.erase,
        });
    }
    if strokes.iter().all(|s| s.dabs.is_empty()) {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }

    // Bounding box, clipped to the frame.
    let (mut lx, mut ly, mut hx, mut hy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for d in strokes.iter().flat_map(|s| s.dabs.iter()) {
        lx = lx.min(d.x - d.r);
        ly = ly.min(d.y - d.r);
        hx = hx.max(d.x + d.r);
        hy = hy.max(d.y + d.r);
    }
    // ⚠ Clamped to the CLIP, not the frame. That single change is most of this feature: the box,
    // and therefore both f32 buffers below, shrink from the stroke's full extent to its visible
    // part.
    let x0 = lx.floor().clamp(cx0 as f32, cx1 as f32) as usize;
    let y0 = ly.floor().clamp(cy0 as f32, cy1 as f32) as usize;
    let x1 = ((hx.ceil().clamp(cx0 as f32, cx1 as f32) as usize) + 1).min(cx1);
    let y1 = ((hy.ceil().clamp(cy0 as f32, cy1 as f32) as usize) + 1).min(cy1);
    if x1 <= x0 || y1 <= y0 {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }
    let (w, h) = (x1 - x0, y1 - y0);

    let mut cov = vec![0.0f32; w * h];
    // ⚠ One scratch buffer, reused per stroke. Within a stroke the dabs combine with `max` so a
    // slow drag is not darker than a fast one; the stroke's result is then composited into `cov`,
    // so a second pass over the same place *does* build up. That two-level rule is what flow
    // needs and what a single flattened `max` could never express.
    let mut layer = vec![0.0f32; w * h];
    for stroke in &strokes {
        if stroke.dabs.is_empty() || stroke.flow <= 0.0 {
            continue;
        }
        // Only the sub-rect this stroke touches has to be cleared and composited.
        let (mut slx, mut sly, mut shx, mut shy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for d in &stroke.dabs {
            slx = slx.min(d.x - d.r);
            sly = sly.min(d.y - d.r);
            shx = shx.max(d.x + d.r);
            shy = shy.max(d.y + d.r);
        }
        let sx0 = (slx.floor().clamp(x0 as f32, x1 as f32) as usize).max(x0);
        let sy0 = (sly.floor().clamp(y0 as f32, y1 as f32) as usize).max(y0);
        let sx1 = (((shx.ceil().clamp(0.0, x1 as f32) as usize) + 1).min(x1)).max(sx0);
        let sy1 = (((shy.ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1)).max(sy0);
        for y in sy0..sy1 {
            for x in sx0..sx1 {
                layer[(y - y0) * w + (x - x0)] = 0.0;
            }
        }
        for d in &stroke.dabs {
            let dx0 = (d.x - d.r).floor().clamp(sx0 as f32, sx1 as f32) as usize;
            let dy0 = (d.y - d.r).floor().clamp(sy0 as f32, sy1 as f32) as usize;
            let dx1 = (((d.x + d.r).ceil().clamp(0.0, sx1 as f32) as usize) + 1).min(sx1);
            let dy1 = (((d.y + d.r).ceil().clamp(0.0, sy1 as f32) as usize) + 1).min(sy1);
            for y in dy0..dy1 {
                for x in dx0..dx1 {
                    let dist = (((x as f32 + 0.5) - d.x).powi(2)
                        + ((y as f32 + 0.5) - d.y).powi(2))
                    .sqrt();
                    let a = dab_alpha(dist, d.r, d.hard);
                    if a <= 0.0 {
                        continue;
                    }
                    let slot = &mut layer[(y - y0) * w + (x - x0)];
                    *slot = slot.max(a);
                }
            }
        }
        for y in sy0..sy1 {
            for x in sx0..sx1 {
                let idx = (y - y0) * w + (x - x0);
                let a = layer[idx] * stroke.flow;
                if a <= 0.0 {
                    continue;
                }
                if stroke.erase {
                    cov[idx] *= 1.0 - a;
                } else {
                    // Standard "over": a second pass adds a fraction of what is still uncovered,
                    // so repeated strokes approach full coverage without ever exceeding it.
                    cov[idx] += (1.0 - cov[idx]) * a;
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

    CoverageMap {
        x0,
        y0,
        w,
        h,
        data,
        outside,
    }
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
            flow: 100.0,
            erase,
            ..Default::default()
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
        assert!(m.is_inert());
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
                flow: 100.0,
                erase: false,
                ..Default::default()
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
            flow: 100.0,
            erase: false,
            ..Default::default()
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
            flow: 100.0,
            erase: false,
            ..Default::default()
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

    #[test]
    fn a_weak_flow_paints_weakly() {
        // ⚠ The bug this field exists for, found in real use. `hardness` shapes a dab's EDGE, so
        // even at hardness 0 the centre reaches full coverage — the softest possible brush still
        // painted at full strength, which is not what anyone means by a soft brush.
        let strong = flat(vec![dot(0.5, 0.5, 0.1, 100.0, false)]);
        let weak = flat(vec![BrushStroke {
            flow: 20.0,
            ..dot(0.5, 0.5, 0.1, 100.0, false)
        }]);
        assert_eq!(strong.at(40, 20), 255);
        let w = weak.at(40, 20);
        assert!(w > 0 && w < 80, "flow 20 should be faint, got {w}");
    }

    #[test]
    fn flow_builds_up_across_strokes() {
        // ⚠ The other half of the same complaint: repeated brushing must get stronger. Under the
        // old flat `max` rule a second pass over the same place changed nothing at all.
        let one = flat(vec![BrushStroke {
            flow: 40.0,
            ..dot(0.5, 0.5, 0.1, 100.0, false)
        }]);
        let two = flat(vec![
            BrushStroke {
                flow: 40.0,
                ..dot(0.5, 0.5, 0.1, 100.0, false)
            },
            BrushStroke {
                flow: 40.0,
                ..dot(0.5, 0.5, 0.1, 100.0, false)
            },
        ]);
        assert!(
            two.at(40, 20) > one.at(40, 20) + 20,
            "two passes ({}) must be clearly stronger than one ({})",
            two.at(40, 20),
            one.at(40, 20)
        );
        assert!(
            two.at(40, 20) < 255,
            "two passes at flow 40 must not saturate"
        );
    }

    #[test]
    fn flow_does_not_build_up_within_one_stroke() {
        // ⚠⚠ The discriminating case for the two-level rule, and the reason dabs are grouped per
        // stroke rather than flattened. A long stroke lays dozens of overlapping dabs over the
        // same pixel; if those composited, a slow drag would be darker than a fast one over the
        // same path. Within a stroke they must `max`, and only the stroke's result composites.
        let single = flat(vec![BrushStroke {
            flow: 40.0,
            ..dot(0.5, 0.5, 0.1, 0.0, false)
        }]);
        let dragged = flat(vec![BrushStroke {
            points: vec![[0.3, 0.5], [0.7, 0.5]],
            radius: 0.1,
            hardness: 0.0,
            flow: 40.0,
            erase: false,
            ..Default::default()
        }]);
        assert_eq!(
            dragged.at(40, 20),
            single.at(40, 20),
            "a long stroke must match a single dab; overlapping dabs are compositing"
        );
    }

    #[test]
    fn flow_zero_paints_nothing() {
        let m = flat(vec![BrushStroke {
            flow: 0.0,
            ..dot(0.5, 0.5, 0.1, 100.0, false)
        }]);
        assert_eq!(m.at(40, 20), 0);
    }

    #[test]
    fn full_flow_reproduces_the_old_max_rule_for_one_stroke() {
        // ⚠ The compatibility guarantee, and why all 37 goldens stayed byte-identical through the
        // composition change: from zero coverage, `cov += (1 - cov) * a * 1.0` is exactly `a`.
        let m = flat(vec![dot(0.5, 0.5, 0.15, 50.0, false)]);
        for x in [40usize, 45, 48] {
            let v = m.at(x, 20);
            assert!(v > 0, "sample at {x} should be painted");
        }
        assert_eq!(m.at(40, 20), 255, "the centre is still full at flow 100");
    }

    #[test]
    fn an_inverted_brush_covers_the_whole_frame_outside_its_strokes() {
        // ⚠⚠ Found in real use, and the report diagnosed it exactly: after painting, inverting
        // showed the mask only inside "a cropped window" that *grew as more was painted*. That
        // window was the bounding box. `at()` returned 0 outside it, so the inversion stopped at
        // the edge of what had been painted.
        //
        // The far corners of the frame are nowhere near the stroke and must be fully covered.
        let m = rasterize(
            &Brush {
                strokes: vec![dot(0.5, 0.5, 0.05, 100.0, false)],
                invert: true,
            },
            &Geometry::default(),
            80,
            40,
        );
        assert_eq!(m.at(0, 0), 255, "the top-left corner must be covered");
        assert_eq!(m.at(79, 39), 255, "the bottom-right corner must be covered");
        assert_eq!(m.at(40, 20), 0, "the painted centre must be clear");
    }

    #[test]
    fn an_inverted_brush_with_no_strokes_covers_everything() {
        // ⚠ "Everything except nothing" is everything. A box-only emptiness test would call this
        // inert and skip the stage entirely.
        let m = rasterize(
            &Brush {
                strokes: vec![],
                invert: true,
            },
            &Geometry::default(),
            80,
            40,
        );
        assert!(!m.is_inert(), "an inverted empty brush is not inert");
        assert_eq!(m.at(40, 20), 255);
        assert_eq!(m.at(0, 0), 255);
    }

    #[test]
    fn a_plain_brush_with_no_strokes_is_inert() {
        let m = rasterize(&brush(vec![]), &Geometry::default(), 80, 40);
        assert!(m.is_inert());
        assert_eq!(m.at(40, 20), 0);
    }

    #[test]
    fn the_bbox_does_not_grow_just_because_a_brush_is_inverted() {
        // ⚠ The whole point of storing an outside value rather than falling back to a full-frame
        // map: a full-frame coverage map is 26 MB at full resolution, and an inverted mask would
        // otherwise force one.
        let m = rasterize(
            &Brush {
                strokes: vec![dot(0.5, 0.5, 0.05, 100.0, false)],
                invert: true,
            },
            &Geometry::default(),
            80,
            40,
        );
        assert!(
            m.w < 20 && m.h < 20,
            "box {}x{} should stay small",
            m.w,
            m.h
        );
    }

    /// A stroke crossing most of the frame, so a clip genuinely cuts it.
    fn wide_stroke() -> Brush {
        brush(vec![BrushStroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            radius: 0.06,
            hardness: 40.0,
            flow: 100.0,
            erase: false,
            ..Default::default()
        }])
    }

    #[test]
    fn a_clipped_raster_matches_the_whole_frame_one() {
        // ⚠⚠ The load-bearing test. It compares COVERAGE VALUES AT FRAME COORDINATES, never the
        // maps' fields: the boxes legitimately differ — that is the entire point of clipping — so
        // asserting on `x0` or `w` would fail against a correct implementation. The same trap
        // `a_stroke_survives_a_crop` had to avoid.
        let b = wide_stroke();
        let g = Geometry::default();
        let whole = rasterize(&b, &g, 80, 40);
        let clip = (30usize, 12usize, 20usize, 16usize);
        let part = rasterize_in(&b, &g, 80, 40, Some(clip));
        for y in clip.1..clip.1 + clip.3 {
            for x in clip.0..clip.0 + clip.2 {
                assert_eq!(part.at(x, y), whole.at(x, y), "at frame ({x}, {y})");
            }
        }
    }

    #[test]
    fn the_clip_does_not_shift_dab_positions() {
        // ⚠ The bug clipping invites: subtracting the clip origin somewhere it should not be,
        // which lands every stroke off by the region offset — visible only when zoomed, and easy
        // to mistake for a mask that "drifts".
        let b = brush(vec![dot(0.5, 0.5, 0.05, 100.0, false)]);
        let g = Geometry::default();
        let part = rasterize_in(&b, &g, 80, 40, Some((30, 12, 20, 16)));
        assert_eq!(part.at(40, 20), 255, "the dab centre is at frame (40, 20)");
        assert_eq!(part.at(31, 13), 0, "the clip's own corner is not painted");
    }

    #[test]
    fn a_dab_entirely_outside_the_clip_contributes_nothing() {
        // ⚠ Guards the output, not the skip in `keep`. Removing that skip leaves this passing —
        // checked — because the bounding box is clamped to the clip either way. The skip is an
        // optimisation and its comment says so.
        let far = brush(vec![dot(0.9, 0.9, 0.03, 100.0, false)]);
        let part = rasterize_in(&far, &Geometry::default(), 80, 40, Some((0, 0, 20, 10)));
        assert!(part.is_inert(), "nothing inside the clip should be painted");
    }

    #[test]
    fn an_inverted_mask_still_covers_the_clipped_area() {
        // ⚠ The bbox-versus-outside bug must not come back through the clip.
        let b = Brush {
            strokes: vec![dot(0.5, 0.5, 0.05, 100.0, false)],
            invert: true,
        };
        let part = rasterize_in(&b, &Geometry::default(), 80, 40, Some((0, 0, 20, 10)));
        assert_eq!(part.at(5, 5), 255, "inverted coverage inside the clip");
        assert_eq!(part.at(40, 20), 255, "and outside it, via `outside`");
    }

    #[test]
    fn no_clip_is_byte_identical_to_the_whole_frame() {
        let b = wide_stroke();
        let g = Geometry::default();
        assert_eq!(
            rasterize_in(&b, &g, 80, 40, None),
            rasterize(&b, &g, 80, 40)
        );
    }

    #[test]
    fn a_clip_larger_than_the_frame_is_harmless() {
        // The canvas clamps its region, but a clip must not index out of bounds if one slips
        // through.
        let b = wide_stroke();
        let g = Geometry::default();
        let part = rasterize_in(&b, &g, 80, 40, Some((0, 0, 9999, 9999)));
        assert_eq!(part, rasterize(&b, &g, 80, 40));
    }
}

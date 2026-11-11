//! Splatting strokes into a coverage map.

use super::FrameMap;
use edit_model::{Brush, Geometry};
use rayon::prelude::*;

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

/// A base plane as a GPU rasteriser needs it: contiguous samples plus where they sit in the frame.
///
/// ⚠⚠ **The origin is NOT optional, and that is the whole reason this is a struct rather than a
/// tuple.** [`BasePixels::at`] takes **frame** coordinates; a region render's buffer covers only
/// part of the frame, so a consumer indexing `(fy * w + fx)` reads pixels shifted by the origin.
/// That renders an auto-mask gating against colours from somewhere else — a plausible mask rather
/// than an error — and every whole-frame fixture is blind to it.
#[derive(Clone, Copy)]
pub struct BaseRef<'a> {
    pub plane: &'a [f32],
    pub w: usize,
    pub h: usize,
    /// The frame coordinate of the plane's `(0, 0)`.
    pub x0: usize,
    /// The frame coordinate of the plane's `(0, 0)`.
    pub y0: usize,
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

/// Linear working-space RGB at a **frame** pixel, for the auto-mask gate.
///
/// ⚠ **Frame coordinates, not region coordinates.** Dab positions are frame-space and a clip only
/// bounds the output box, so a sampler expecting region coordinates would read the wrong pixels —
/// silently, since the result would still be a plausible mask.
///
/// ⚠ Must be **total**. The rasterizer only ever asks inside its own clip, but an implementation
/// is still required to answer rather than panic.
/// Rows handed to one rayon task when a stroke is splatted.
///
/// ⚠⚠ **Swept, and the answer INVERTS the 8-bit pack's finding.** There, per-item rayon measured
/// *slower than not threading at all* and the fix was coarse blocks. Here the opposite holds —
/// median `dabs_ms` on the real 36-stroke brush at 1510x2019, three runs each:
///
/// | rows/task | 1 | 2 | **4** | 8 | 16 | 64 | 256 |
/// |---|---|---|---|---|---|---|---|
/// | ms | 55.6 | 55.5 | **56.4** | 56.4 | 57.8 | 70.4 | 121.3 |
///
/// The cause is **load imbalance, not dispatch**: rows differ enormously in how many dabs touch
/// them and how wide those dabs are, so a 256-row task can hold a large share of the stroke while
/// every other thread waits. A task here is one row of up to 1510 px against the pack's four
/// bytes, so dispatch is negligible and work-stealing is what matters.
///
/// ⚠ 1–8 is a plateau; 4 sits mid-plateau and amortises the per-task `scratch` allocation four
/// ways rather than allocating one per row.
const CHUNK_ROWS: usize = 4;

/// Below this many pixels a stroke is splatted sequentially.
///
/// ⚠ The canvas tint rasterises on every drag frame and every test uses a tiny fixture, where
/// rayon's dispatch would dominate. **Swept rather than chosen** — `pipeline`'s lib suite, which
/// is overwhelmingly small rasterisations, three runs each:
///
/// | floor | 0 | 16 Ki | **64 Ki** | 256 Ki | never parallel |
/// |---|---|---|---|---|---|
/// | s | 0.86–0.98 | 1.13–1.55 | **0.74–0.79** | 0.72–0.83 | 0.70–0.79 |
///
/// So a floor of 0 really does cost ~0.15 s of small work, and everything from 64 Ki upward is
/// indistinguishable from never parallelising at all. 64 Ki is the **smallest** value at that
/// plateau, which is what keeps the parallel path reachable for the largest possible share of
/// real strokes. ⚠ The 16 Ki row is noise — it should sit between 0 and 64 Ki and does not.
const SEQUENTIAL_FLOOR_PX: usize = 64 * 1024;

// How many strokes have taken the **parallel** splat, on this thread.
//
// ⚠⚠ An explicit event counter, because the two branches produce identical pixels by design and
// nothing in the output can distinguish them — the same reason `Ran { bytes }` and the Oklab
// conversion counter exist. **A counter, not a flag**: a flag describing the current state
// cannot see its own absence. **Thread-local**, because libtest runs tests in parallel and a
// shared counter would read someone else's rasterisation.
//
// ⚠ Incremented on the *calling* thread, outside the rayon closure — inside it, the increment
// would land on a worker's thread-local and the test would never see it.
#[cfg(test)]
thread_local! {
    pub(crate) static PARALLEL_STROKES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// ⚠ `Sync` is a supertrait because `rasterize_in` reads the base from several rayon threads at
/// once when it builds `base_lab`. Every implementor here already holds only shared references,
/// so this costs nothing; the compiler names any future one that does not.
pub trait BasePixels: Sync {
    fn at(&self, x: usize, y: usize) -> [f32; 3];
}

/// One splat: centre, radius and profile in frame pixels.
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub hard: f32,
    /// `(reference in Oklab, ceiling)` when this dab was painted with Auto Mask on.
    ///
    /// ⚠ The **ceiling** is precomputed per stroke rather than the raw tolerance, so the inner
    /// pixel loop does a distance and a smoothstep and nothing else.
    pub gate: Option<([f32; 3], f32)>,
}

/// One stroke's dabs, with the properties that apply to the stroke as a whole.
///
/// ⚠ Dabs are grouped **per stroke** rather than flattened, and that grouping is what makes flow
/// correct: within a stroke the dabs combine with `max`, so a slow drag is not darker than a fast
/// one over the same path; across strokes the results composite, so a second pass builds up.
/// Flattening would force one rule to serve both.
pub struct Stroke {
    pub dabs: Vec<Dab>,
    pub flow: f32,
    pub erase: bool,
}

/// Every dab a brush produces, grouped by stroke, with the box they fall in.
///
/// ⚠ `x0`/`y0`/`w`/`h` are the **clipped** box, already clamped exactly as the coverage map's is,
/// so a consumer never re-derives it and the two cannot disagree about which pixels exist.
pub struct DabPlan {
    pub strokes: Vec<Stroke>,
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// Coverage everywhere outside the box — 255 for an inverted brush.
    pub outside: u8,
    /// The row span a gating dab can read, if anything gates.
    pub gating_rows: Option<(usize, usize)>,
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
    rasterize_in(brush, geometry, frame_w, frame_h, None, None)
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
/// Plan every dab, or `None` when there is nothing to rasterise.
///
/// ⚠⚠ **One dab list, two consumers** — [`rasterize_planned`] below and `gpu-pipeline`'s
/// `GpuBrushRaster`. That is the rule `adjustment_stages` already follows for stages: the
/// duplication that would actually hurt is this arithmetic — the `SPACING` interpolation, the
/// per-dab gate reference, the clip rejection, the box — and it exists once.
///
/// ⚠ Returns `None` for each of the early-outs `rasterize_in` had, because in every one of them
/// the answer is the same: a map with no box and only `outside`. The caller builds that.
pub fn plan_dabs(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
    base: Option<&dyn BasePixels>,
) -> Option<DabPlan> {
    // ⚠ An inverted brush with no strokes covers everything, not nothing.
    let outside = if brush.invert { 255 } else { 0 };
    if brush.strokes.is_empty() || frame_w == 0 || frame_h == 0 {
        return None;
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
        return None;
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
        // ⚠⚠ **Each dab's reference is the pixel under its own centre, read here rather than
        // stored.** Both ends of the comparison then come from one buffer in one colour space,
        // which is what lets the gate use lightness as well as chroma — see `auto_mask_gate`.
        //
        // ⚠ Read once per dab, never per pixel: a dab covers hundreds of pixels and they all
        // compare against this one value.
        //
        // ⚠ With no base — the CLI, or the tint before a frame has landed — a dab carries no gate
        // and paints positionally. `Brush::needs_base` plus `BrushCache`'s refusal are what stop
        // the *render* ever taking that path.
        let ceiling = auto_mask_ceiling(s.tolerance);
        let gate_at = |x: f32, y: f32| -> Option<([f32; 3], f32)> {
            if !s.auto_mask {
                return None;
            }
            base.map(|b| {
                (
                    color::oklab::prophoto_to_oklab(b.at(x.max(0.0) as usize, y.max(0.0) as usize)),
                    ceiling,
                )
            })
        };
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
                gate: gate_at(first.0, first.1),
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
                        gate: gate_at(px, py),
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
        return None;
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
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    // ⚠ Only the rows a gating dab can read are converted. `base_lab` is still indexed over the
    // WHOLE box — rows outside the gating span are simply never written and never read — so the
    // `(y - y0) * w + (x - x0)` arithmetic in the dab loop is unchanged.
    //
    // ⚠ Rows only, not columns: a per-row `x` span would make the fill ragged while the *index*
    // stayed whole-box, which is more arithmetic for a smaller saving.
    let gating_rows: Option<(usize, usize)> = {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for d in strokes
            .iter()
            .filter(|s| s.dabs.iter().any(|d| d.gate.is_some()))
            .flat_map(|s| s.dabs.iter())
        {
            lo = lo.min(d.y - d.r);
            hi = hi.max(d.y + d.r);
        }
        if lo > hi {
            None
        } else {
            let a = (lo.floor().clamp(y0 as f32, y1 as f32) as usize).max(y0);
            let b = (((hi.ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1)).max(a);
            (b > a).then_some((a, b))
        }
    };
    Some(DabPlan {
        strokes,
        x0,
        y0,
        w,
        h,
        outside,
        gating_rows,
    })
}

pub fn rasterize_in(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
    base: Option<&dyn BasePixels>,
) -> CoverageMap {
    // ⚠ An inverted brush with no strokes covers everything, not nothing.
    let outside = if brush.invert { 255 } else { 0 };
    let Some(plan) = plan_dabs(brush, geometry, frame_w, frame_h, clip, base) else {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    };
    rasterize_planned(&plan, brush.invert, base)
}

/// The CPU kernel over an already-planned dab list.
///
/// ⚠ Split out at M8.3k so [`plan_dabs`] has one caller shape and the GPU rasteriser can
/// consume the same plan. **The arithmetic below is unmoved** — which is what the
/// byte-identical goldens and the unchanged test count establish, not a claim in this comment.
///
/// ⚠ `t_lab` now brackets the Oklab conversion **alone**, where it used to include the
/// `gating_rows` scan over the dab list. That scan moved into `plan_dabs`, so the `lab` figure
/// in `RUSTROOM_DEBUG_MASK` is a slightly tighter measurement of the same thing rather than a
/// different one.
fn rasterize_planned(plan: &DabPlan, invert: bool, base: Option<&dyn BasePixels>) -> CoverageMap {
    let DabPlan {
        strokes,
        x0,
        y0,
        w,
        h,
        outside,
        gating_rows,
    } = plan;
    let (x0, y0, w, h) = (*x0, *y0, *w, *h);
    let (outside, gating_rows) = (*outside, *gating_rows);
    let (x1, y1) = (x0 + w, y0 + h);

    // ⚠ One Oklab conversion per pixel of the box, eagerly, rather than one per pixel *per dab*.
    // Dabs are laid a quarter of a radius apart (`SPACING`) and overlap heavily, so a lazy
    // per-dab conversion would convert the same pixel a dozen times over. Built only when
    // something actually gates, so a plain brush pays nothing at all.
    let t_lab = std::time::Instant::now();
    let base_lab: Option<Vec<[f32; 3]>> = match (base, gating_rows) {
        (Some(b), Some((gy0, gy1))) => {
            // ⚠ Rayon over rows, the shape `map_pixels_xy` and the 8-bit pack already use. Each
            // element is a pure function of one base pixel and the order is unchanged, so this is
            // bit-identical by construction rather than by tolerance.
            let mut v = vec![[0.0f32; 3]; w * h];
            v[(gy0 - y0) * w..(gy1 - y0) * w]
                .par_chunks_exact_mut(w)
                .enumerate()
                .for_each(|(row, out)| {
                    let y = gy0 + row;
                    for (i, slot) in out.iter_mut().enumerate() {
                        *slot = color::oklab::prophoto_to_oklab(b.at(x0 + i, y));
                    }
                });
            Some(v)
        }
        _ => None,
    };

    let d_lab = t_lab.elapsed();
    let t_dabs = std::time::Instant::now();
    let mut cov = vec![0.0f32; w * h];
    for stroke in strokes {
        if stroke.dabs.is_empty() || stroke.flow <= 0.0 {
            continue;
        }
        // Only the sub-rect this stroke touches is visited.
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
        if sx1 <= sx0 || sy1 <= sy0 {
            continue;
        }
        let sw = sx1 - sx0;
        let op = if stroke.erase {
            edit_model::MaskOp::Subtract
        } else {
            edit_model::MaskOp::Add
        };
        // ⚠⚠ **One row of this stroke, as a GATHER rather than a scatter.** The shape this
        // replaced splatted each dab into a shared `layer`, which cannot be parallelised because
        // dabs overlap. Walking the dabs *per row* inverts that: the row owns its own scratch and
        // its own row of `cov`, so rows are independent and `layer` disappears entirely — three
        // passes per stroke (clear, splat, composite) become one.
        //
        // ⚠⚠ **The short-circuit before the gate is load-bearing and must not move.** The gate
        // can only ever reduce an alpha, so a dab whose positional alpha already loses the
        // running max cannot change the pixel however its gate resolves — and dabs are laid
        // `SPACING` (a quarter radius) apart and overlap heavily, so most of them lose. Gating
        // before the skip is *correct* and measurably slower.
        //
        // ⚠ Dabs are visited in stroke order. That is for the skip's effectiveness, **not** for
        // correctness: the within-stroke combine is a `max` and the skip only ever drops a dab
        // whose gated alpha is already below it, so the value is exactly `max_d gated_alpha(d)`
        // and is order-independent. Both facts were verified by sabotage rather than assumed.
        let row_body = |y: usize, cov_row: &mut [f32], scratch: &mut [f32]| {
            // ⚠ One scratch row, reused down a chunk and cleared per row. Within a stroke the
            // dabs combine with `max` so a slow drag is not darker than a fast one; the stroke's
            // result is then composited into `cov`, so a second pass over the same place *does*
            // build up. That two-level rule is what flow needs and what a single flattened `max`
            // could never express.
            //
            // ⚠ Clearing is per ROW rather than per stroke now. Miss it and one row's coverage
            // leaks into the next, which is the likeliest bug in this rewrite — sabotage-verified
            // against `the_rasterizer_is_bit_identical_to_its_reference`.
            for v in scratch.iter_mut() {
                *v = 0.0;
            }
            for d in &stroke.dabs {
                let dy0 = (d.y - d.r).floor().clamp(sy0 as f32, sy1 as f32) as usize;
                let dy1 = (((d.y + d.r).ceil().clamp(0.0, sy1 as f32) as usize) + 1).min(sy1);
                if y < dy0 || y >= dy1 {
                    continue;
                }
                let dx0 = (d.x - d.r).floor().clamp(sx0 as f32, sx1 as f32) as usize;
                let dx1 = (((d.x + d.r).ceil().clamp(0.0, sx1 as f32) as usize) + 1).min(sx1);
                for x in dx0..dx1 {
                    let dist = (((x as f32 + 0.5) - d.x).powi(2)
                        + ((y as f32 + 0.5) - d.y).powi(2))
                    .sqrt();
                    let a = dab_alpha(dist, d.r, d.hard);
                    let slot = &mut scratch[x - sx0];
                    if a <= *slot {
                        continue;
                    }
                    // ⚠⚠ The gate multiplies the dab's alpha **before** the per-stroke
                    // `max` below, and that is what per-dab references force: two dabs on
                    // opposite sides of a ridge carry different references, so a gate applied
                    // after the `max` could only ever honour one of them.
                    //
                    // ⚠ With no base — the CLI, or the tint outside the rendered region — an
                    // auto-masked dab paints positionally. Documented degradation, not an error;
                    // `Brush::needs_base` plus `BrushCache`'s refusal are what stop the *render*
                    // ever taking that path.
                    let a = match (&d.gate, &base_lab) {
                        (Some((r, ceil)), Some(lab)) => {
                            a * auto_mask_gate(lab[(y - y0) * w + (x - x0)], *r, *ceil)
                        }
                        _ => a,
                    };
                    *slot = slot.max(a);
                }
            }
            // ⚠⚠ **The same two rules `MaskOp` offers BETWEEN masks, called from here so the
            // two cannot drift.** `Subtract` is `c * (1 - a)` and `Add` is the standard "over" —
            // a second pass adds a fraction of what is still uncovered, so repeated strokes
            // approach full coverage without ever exceeding it.
            //
            // ⚠ Bit-identical to the two expressions it replaced (`c *= 1-a` and
            // `c += (1-c)*a` are the same f32 operations in the same order), which is what the
            // byte-identical goldens prove rather than merely "still green".
            //
            // ⚠ This is the whole argument for the product algebra between masks:
            // erase-within-a-brush and subtract-between-masks are ONE rule, not two that happen
            // to agree at the endpoints and diverge at every partial coverage.
            for x in sx0..sx1 {
                let a = scratch[x - sx0] * stroke.flow;
                if a <= 0.0 {
                    continue;
                }
                let idx = x - x0;
                cov_row[idx] = op.combine(cov_row[idx], a);
            }
        };
        let r0 = sy0 - y0;
        let r1 = sy1 - y0;
        let rows = &mut cov[r0 * w..r1 * w];
        if (sy1 - sy0) * sw < SEQUENTIAL_FLOOR_PX {
            let mut scratch = vec![0.0f32; sw];
            for (ri, cov_row) in rows.chunks_mut(w).enumerate() {
                row_body(sy0 + ri, cov_row, &mut scratch);
            }
        } else {
            #[cfg(test)]
            PARALLEL_STROKES.with(|c| c.set(c.get() + 1));
            rows.par_chunks_mut(w * CHUNK_ROWS)
                .enumerate()
                .for_each(|(ci, chunk)| {
                    let mut scratch = vec![0.0f32; sw];
                    for (ri, cov_row) in chunk.chunks_mut(w).enumerate() {
                        row_body(sy0 + ci * CHUNK_ROWS + ri, cov_row, &mut scratch);
                    }
                });
        }
    }

    let data = cov
        .iter()
        .map(|c| {
            let c = if invert { 1.0 - *c } else { *c };
            // ⚠ NaN reaches 0 through the cast, not the clamp: `f32::clamp` propagates NaN while
            // Rust's float-to-int casts saturate. The same note `ImageBuf::quantize` carries.
            (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        })
        .collect();

    let d_dabs = t_dabs.elapsed();
    // ⚠ Kept permanently. The split it reports — Oklab conversion against dab compositing — is
    // what decides whether a future change should attack the gate or the splat.
    if std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
        eprintln!(
            "RASTER box={}x{} strokes={} dabs={} gate_rows={} lab={:.2} dabs_ms={:.2}",
            w,
            h,
            strokes.len(),
            strokes.iter().map(|s| s.dabs.len()).sum::<usize>(),
            gating_rows.map_or(0, |(a, b)| b - a),
            d_lab.as_secs_f64() * 1e3,
            d_dabs.as_secs_f64() * 1e3,
        );
    }

    CoverageMap {
        x0,
        y0,
        w,
        h,
        data,
        outside,
    }
}

/// The smallest Oklab chroma distance the gate accepts, at tolerance 0.
///
/// ⚠ Not zero. A tolerance of 0 must still paint the pixel the reference was sampled from, and a
/// ceiling of zero would both refuse it and collapse the ramp below.
const AUTO_MASK_MIN_DELTA: f32 = 0.01;
/// The Oklab chroma distance accepted at tolerance 100.
///
/// ⚠ **A starting value, swept against real photographs before this milestone closes.** The
/// vignette needed three revisions before its feel was right and the colour range mask's constants
/// survived first contact; neither outcome is predictable from a spec.
///
/// ⚠ It has **no** space-mismatch floor to clear, because both ends of the comparison now come
/// from one buffer. The earlier stored-reference design did, and that is what forced it to ignore
/// lightness.
const AUTO_MASK_MAX_DELTA: f32 = 0.35;
/// The fraction of the ceiling at which the gate starts falling off.
///
/// ⚠ The gate is a smoothstep rather than a cut because a hard boundary aliases along a ridge,
/// which is the case this tool exists for. It is also why no `EPS` guard is needed here, unlike
/// [`RadialGradient::FEATHER_EPS`]: the ramp's width is a positive constant fraction of a ceiling
/// that is itself never zero.
const AUTO_MASK_SOFT: f32 = 0.5;

/// The Oklab chroma distance at which a pixel stops being painted, for a 0..=100 tolerance.
pub fn auto_mask_ceiling(tolerance: f32) -> f32 {
    let t = (tolerance / 100.0).clamp(0.0, 1.0);
    AUTO_MASK_MIN_DELTA + t * (AUTO_MASK_MAX_DELTA - AUTO_MASK_MIN_DELTA)
}

/// How much of a dab's coverage survives at a pixel, given that dab's reference colour.
///
/// Both arguments are **Oklab**, and both are read from the **same buffer** — the pixel at this
/// position and the pixel under the dab's own centre. That is what lets the full distance be used,
/// lightness included, so a purely tonal edge is detected as well as a chromatic one.
///
/// ⚠⚠ **An earlier design stored the reference in the stroke and had to drop the lightness term.**
/// A stored reference is sampled from the *displayed* frame while the pixels come from
/// `MaskStage`'s input, before the display rendering; `auto_mask_space_probe` measured that
/// lightness disagreement at a mean of **+0.07 to +0.11** across four frames, 7-10x the chroma
/// disagreement. Reading both ends from one buffer removes the disagreement rather than working
/// around it. ⚠ Do not reintroduce a stored reference without re-reading that measurement.
pub fn auto_mask_gate(pixel: [f32; 3], reference: [f32; 3], ceiling: f32) -> f32 {
    let d = ((pixel[0] - reference[0]).powi(2)
        + (pixel[1] - reference[1]).powi(2)
        + (pixel[2] - reference[2]).powi(2))
    .sqrt();
    let lo = ceiling * (1.0 - AUTO_MASK_SOFT);
    if d <= lo {
        return 1.0;
    }
    if d >= ceiling {
        return 0.0;
    }
    let q = (d - lo) / (ceiling - lo);
    1.0 - q * q * (3.0 - 2.0 * q)
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

    /// A base whose left half and right half differ in Oklab, so an auto-mask gate genuinely
    /// discriminates rather than passing everything.
    struct HalfBase;
    impl BasePixels for HalfBase {
        fn at(&self, x: usize, _y: usize) -> [f32; 3] {
            if x < 40 {
                [0.05, 0.20, 0.05]
            } else {
                [0.60, 0.15, 0.50]
            }
        }
    }

    /// The fixture the bit-identity test rasterises.
    ///
    /// ⚠⚠ Every property here is load-bearing and a weaker fixture cannot see the bug it guards:
    /// **overlapping dabs** (so the running max and its short-circuit matter), an **erase**
    /// stroke (so `MaskOp::Subtract` and stroke order matter), an **auto-masked** stroke crossing
    /// the base's split (so the gate is reached and differs per dab), a **one-row** stroke (the
    /// degenerate chunk), and flow below 100 (so the cross-stroke composite is not saturating).
    fn gather_fixture() -> Brush {
        let along = |y: f32, x0: f32, x1: f32, n: usize| -> Vec<[f32; 2]> {
            (0..n)
                .map(|i| [x0 + (x1 - x0) * i as f32 / (n - 1) as f32, y])
                .collect()
        };
        Brush {
            strokes: vec![
                BrushStroke {
                    points: along(0.35, 0.15, 0.85, 12),
                    radius: 0.09,
                    hardness: 40.0,
                    flow: 70.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.42, 0.20, 0.80, 9),
                    radius: 0.07,
                    hardness: 65.0,
                    flow: 55.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.38, 0.30, 0.70, 7),
                    radius: 0.05,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: true,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.60, 0.10, 0.90, 14),
                    radius: 0.08,
                    hardness: 50.0,
                    flow: 80.0,
                    auto_mask: true,
                    tolerance: 30.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: vec![[0.50, 0.90]],
                    radius: 0.006,
                    hardness: 100.0,
                    flow: 100.0,
                    ..Default::default()
                },
            ],
            invert: false,
        }
    }

    #[test]
    fn the_rasterizer_is_bit_identical_to_its_reference() {
        // ⚠⚠ **`rasterize_in_reference` is a VERBATIM COPY of today's `rasterize_in`, living in
        // the test module and deliberately NOT shared with production.** A "reference" that
        // delegated to the function under test would compare it against itself and pass forever —
        // the vacuous shape this project records more often than any other. It is the rule
        // `downscale_to`'s and the 8-bit pack's parallelisations each followed.
        //
        // ⚠⚠ **Both frame sizes are load-bearing and the small one alone is BLIND.** A stroke
        // below `SEQUENTIAL_FLOOR_PX` takes the sequential splat, so an 80x60 fixture never
        // reaches the parallel branch at all — which is why the counter is asserted rather than
        // assumed. Found by a sabotage that an unrelated test caught while this one stayed green.
        let b = gather_fixture();
        let g = Geometry::default();
        for (fw, fh, want_parallel) in [(80usize, 60usize, false), (900, 700, true)] {
            PARALLEL_STROKES.with(|c| c.set(0));
            let want = rasterize_in_reference(&b, &g, fw, fh, None, Some(&HalfBase));
            let got = rasterize_in(&b, &g, fw, fh, None, Some(&HalfBase));
            let took_parallel = PARALLEL_STROKES.with(|c| c.get()) > 0;
            assert_eq!(
                took_parallel, want_parallel,
                "fixture check: {fw}x{fh} must exercise the expected splat"
            );
            let read = |m: &CoverageMap| -> Vec<u8> {
                (0..fh)
                    .flat_map(|y| (0..fw).map(move |x| (x, y)))
                    .map(|(x, y)| m.at(x, y))
                    .collect()
            };
            let (want, got) = (read(&want), read(&got));
            assert!(
                got.iter().any(|&v| v > 0) && got.contains(&0),
                "fixture check: at {fw}x{fh} the brush must both cover and not cover"
            );
            assert!(
                got.iter().any(|&v| v > 0 && v < 255),
                "fixture check: at {fw}x{fh} some coverage must be partial"
            );
            assert_eq!(
                want, got,
                "the rasterizer must be bit-identical to its reference at {fw}x{fh}"
            );
        }
    }

    /// A verbatim copy of `rasterize_in` as it stood before M8.3i parallelised it: sequential
    /// Oklab conversion, and a per-stroke three-pass scatter into a reused `layer`.
    ///
    /// ⚠⚠ **Deliberately duplicated rather than shared.** Its whole value is being an
    /// independent witness: a reference that called `rasterize_in` could not see a change to
    /// `rasterize_in`. It is frozen — when the shipped function changes, this one must NOT be
    /// updated to match; if the two disagree, that is the finding.
    ///
    /// ⚠ It will drift out of resemblance over time, which is expected and is the cost of the
    /// guarantee. Delete it only when the parallel implementation has been shipped long enough
    /// that the goldens are the accepted oracle.
    #[allow(clippy::too_many_arguments)]
    fn rasterize_in_reference(
        brush: &Brush,
        geometry: &Geometry,
        frame_w: usize,
        frame_h: usize,
        clip: Option<(usize, usize, usize, usize)>,
        base: Option<&dyn BasePixels>,
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
            // ⚠⚠ **Each dab's reference is the pixel under its own centre, read here rather than
            // stored.** Both ends of the comparison then come from one buffer in one colour space,
            // which is what lets the gate use lightness as well as chroma — see `auto_mask_gate`.
            //
            // ⚠ Read once per dab, never per pixel: a dab covers hundreds of pixels and they all
            // compare against this one value.
            //
            // ⚠ With no base — the CLI, or the tint before a frame has landed — a dab carries no gate
            // and paints positionally. `Brush::needs_base` plus `BrushCache`'s refusal are what stop
            // the *render* ever taking that path.
            let ceiling = auto_mask_ceiling(s.tolerance);
            let gate_at = |x: f32, y: f32| -> Option<([f32; 3], f32)> {
                if !s.auto_mask {
                    return None;
                }
                base.map(|b| {
                    (
                        color::oklab::prophoto_to_oklab(
                            b.at(x.max(0.0) as usize, y.max(0.0) as usize),
                        ),
                        ceiling,
                    )
                })
            };
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
                    gate: gate_at(first.0, first.1),
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
                            gate: gate_at(px, py),
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

        // ⚠ One Oklab conversion per pixel of the box, eagerly, rather than one per pixel *per dab*.
        // Dabs are laid a quarter of a radius apart (`SPACING`) and overlap heavily, so a lazy
        // per-dab conversion would convert the same pixel a dozen times over. Built only when
        // something actually gates, so a plain brush pays nothing at all.
        let base_lab: Option<Vec<[f32; 3]>> = match base {
            Some(b)
                if strokes
                    .iter()
                    .flat_map(|s| s.dabs.iter())
                    .any(|d| d.gate.is_some()) =>
            {
                let mut v = Vec::with_capacity(w * h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        v.push(color::oklab::prophoto_to_oklab(b.at(x, y)));
                    }
                }
                Some(v)
            }
            _ => None,
        };

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
                        // ⚠⚠ The gate multiplies the dab's alpha **before** the per-stroke `max`
                        // below, and that is what per-dab references force: two dabs on opposite
                        // sides of a ridge carry different references, so a gate applied after the
                        // `max` could only ever honour one of them.
                        //
                        // ⚠ With no base — the CLI before this landed, or the tint outside the
                        // rendered region — an auto-masked dab paints positionally. Documented
                        // degradation, not an error; `Brush::needs_base` plus `BrushCache`'s refusal
                        // are what stop the *render* ever taking that path.
                        let slot = &mut layer[(y - y0) * w + (x - x0)];
                        // ⚠⚠ **Short-circuit before the gate, and it is sound rather than an
                        // approximation.** Dabs within a stroke combine with `max`, and the gate can
                        // only ever *reduce* an alpha — so a dab whose positional alpha already loses
                        // the `max` cannot change this pixel however its gate resolves. Worth having
                        // because dabs are laid `SPACING` (a quarter radius) apart and overlap
                        // heavily, so most of them lose.
                        if a <= *slot {
                            continue;
                        }
                        let a = match (&d.gate, &base_lab) {
                            (Some((r, ceil)), Some(lab)) => {
                                a * auto_mask_gate(lab[(y - y0) * w + (x - x0)], *r, *ceil)
                            }
                            _ => a,
                        };
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
                    // ⚠⚠ **The same two rules `MaskOp` offers BETWEEN masks, called from here so the
                    // two cannot drift.** `Subtract` is `c * (1 - a)` and `Add` is the standard
                    // "over" — a second pass adds a fraction of what is still uncovered, so repeated
                    // strokes approach full coverage without ever exceeding it.
                    //
                    // ⚠ Bit-identical to the two expressions this replaces (`c *= 1-a` and
                    // `c += (1-c)*a` are the same f32 operations in the same order), which is what
                    // the 60 byte-identical goldens prove rather than merely "still green".
                    //
                    // ⚠ This is the whole argument for the product algebra between masks:
                    // erase-within-a-brush and subtract-between-masks are now ONE rule, not two that
                    // happen to agree at the endpoints and diverge at every partial coverage.
                    let op = if stroke.erase {
                        edit_model::MaskOp::Subtract
                    } else {
                        edit_model::MaskOp::Add
                    };
                    cov[idx] = op.combine(cov[idx], a);
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

    /// A base image split down the middle, in **linear ProPhoto**, which is what `BasePixels`
    /// promises.
    struct SplitBase {
        split_x: usize,
        left: [f32; 3],
        right: [f32; 3],
    }
    impl BasePixels for SplitBase {
        fn at(&self, x: usize, _y: usize) -> [f32; 3] {
            if x < self.split_x {
                self.left
            } else {
                self.right
            }
        }
    }

    const LEFT: [f32; 3] = [0.05, 0.20, 0.05];
    const RIGHT: [f32; 3] = [0.60, 0.15, 0.50];

    fn split_at(x: usize) -> SplitBase {
        SplitBase {
            split_x: x,
            left: LEFT,
            right: RIGHT,
        }
    }

    /// One auto-masked dab, centred at `cx` in frame pixels of an 80-wide frame.
    fn auto_dot(cx: f32, radius: f32, tolerance: f32) -> Brush {
        brush(vec![BrushStroke {
            decode: false,
            points: vec![[cx / 80.0, 0.5]],
            radius,
            hardness: 100.0,
            flow: 100.0,
            erase: false,
            auto_mask: true,
            tolerance,
        }])
    }

    #[test]
    fn a_dab_paints_only_what_matches_its_own_centre() {
        // ⚠⚠ This is the shape of the feature, and it is not "a stroke stops at an edge". A dab
        // paints the pixels similar to the pixel under **its own centre**, so a wide dab sitting
        // on the sky does not paint the ridge inside its radius. Dragging a stroke *across* the
        // ridge legitimately paints both sides, because each dab re-references what is under it —
        // which is what Lightroom's Auto Mask does too.
        let base = split_at(40);
        // Centred on the left colour, radius wide enough to reach well past the split.
        let cov = rasterize_in(
            &auto_dot(20.0, 0.35, 30.0),
            &Geometry::default(),
            80,
            20,
            None,
            Some(&base),
        );
        assert!(
            cov.at(20, 10) > 200,
            "the centre's own colour must be painted: {}",
            cov.at(20, 10)
        );
        assert_eq!(
            cov.at(45, 10),
            0,
            "the far colour inside the same disc must not be"
        );
    }

    #[test]
    fn the_same_dab_without_auto_mask_covers_its_whole_disc() {
        // ⚠ The discriminating half. A test asserting only that the far side is empty would pass
        // against a rasterizer that painted nothing at all.
        let base = split_at(40);
        let mut b = auto_dot(20.0, 0.35, 30.0);
        b.strokes[0].auto_mask = false;
        let cov = rasterize_in(&b, &Geometry::default(), 80, 20, None, Some(&base));
        assert!(
            cov.at(45, 10) > 200,
            "a plain dab must cover its whole disc: {}",
            cov.at(45, 10)
        );
    }

    #[test]
    fn each_dab_references_its_own_centre_rather_than_the_strokes_first() {
        // ⚠⚠ The test that discriminates the per-dab design. A rasterizer that took one reference
        // for the whole stroke — the first dab's — would paint the left half and refuse the
        // right. Both halves must paint, because each dab references what is under itself.
        let base = split_at(40);
        let b = brush(vec![BrushStroke {
            decode: false,
            points: vec![[0.15, 0.5], [0.85, 0.5]],
            radius: 0.08,
            hardness: 100.0,
            flow: 100.0,
            erase: false,
            auto_mask: true,
            tolerance: 30.0,
        }]);
        let cov = rasterize_in(&b, &Geometry::default(), 80, 20, None, Some(&base));
        assert!(
            cov.at(15, 10) > 200,
            "the left half must paint: {}",
            cov.at(15, 10)
        );
        assert!(
            cov.at(65, 10) > 200,
            "the right half must paint too: {}",
            cov.at(65, 10)
        );
    }

    #[test]
    fn the_gate_sees_a_purely_tonal_edge() {
        // ⚠⚠ The capability the stored-reference design could not have had. Both ends of the
        // comparison come from one buffer, so lightness is usable and two neutrals of different
        // brightness are two regions — a white wall against a grey one.
        let base = SplitBase {
            split_x: 40,
            left: [0.05, 0.05, 0.05],
            right: [0.75, 0.75, 0.75],
        };
        let cov = rasterize_in(
            &auto_dot(20.0, 0.35, 30.0),
            &Geometry::default(),
            80,
            20,
            None,
            Some(&base),
        );
        assert!(cov.at(20, 10) > 200, "the dark side must paint");
        assert_eq!(cov.at(45, 10), 0, "the bright side must not");
    }

    #[test]
    fn an_auto_masked_stroke_with_no_base_paints_positionally() {
        // ⚠ Documented degradation, not an error. `MaskStage` always supplies a base for a brush
        // that needs one — `Brush::needs_base` plus `BrushCache`'s refusal make that so — and the
        // tint may legitimately have none before a frame has landed.
        let cov = rasterize_in(
            &auto_dot(20.0, 0.35, 30.0),
            &Geometry::default(),
            80,
            20,
            None,
            None,
        );
        assert!(cov.at(45, 10) > 200);
    }

    #[test]
    fn a_pixel_matching_its_reference_is_fully_painted() {
        let r = [0.5, 0.02, -0.03];
        assert_eq!(auto_mask_gate(r, r, auto_mask_ceiling(35.0)), 1.0);
    }

    #[test]
    fn a_pixel_far_from_its_reference_is_not_painted_at_all() {
        let r = [0.5, 0.02, -0.03];
        assert_eq!(
            auto_mask_gate([0.5, 0.30, 0.30], r, auto_mask_ceiling(35.0)),
            0.0
        );
    }

    #[test]
    fn the_gate_uses_lightness_as_well_as_chroma() {
        // ⚠⚠ The property the stored-reference design could not have had, and the one most likely
        // to be dropped by a later reader who remembers the earlier chroma-only version. Both ends
        // of the comparison come from one buffer, so two neutrals of different brightness are two
        // regions — which is what makes a white wall against a grey one detectable.
        let r = [0.2, 0.05, 0.05];
        let same_hue_much_brighter = [0.9, 0.05, 0.05];
        assert_eq!(
            auto_mask_gate(same_hue_much_brighter, r, auto_mask_ceiling(35.0)),
            0.0,
            "a large lightness difference must close the gate"
        );
    }

    #[test]
    fn the_gate_is_continuous_rather_than_a_hard_cut() {
        // ⚠ The discriminating property, and the reason the gate is a smoothstep: a hard cut
        // aliases along a ridge, which is exactly where this tool is used. Sweeping for the
        // largest step between neighbouring samples is what a probe at two chosen points could
        // not see.
        let r = [0.5, 0.0, 0.0];
        let ceil = auto_mask_ceiling(50.0);
        let mut prev = auto_mask_gate(r, r, ceil);
        let mut worst = 0.0f32;
        for i in 1..=400 {
            let d = ceil * 1.5 * i as f32 / 400.0;
            let g = auto_mask_gate([0.5, d, 0.0], r, ceil);
            worst = worst.max((g - prev).abs());
            prev = g;
        }
        assert!(worst < 0.05, "gate jumps by {worst}, which is a hard cut");
    }

    #[test]
    fn a_larger_tolerance_selects_more() {
        // ⚠ Asserts the DIRECTION, which a single-tolerance probe cannot. Both readings are kept
        // off the ends of the ramp so neither saturates and turns the difference into noise.
        let r = [0.5, 0.0, 0.0];
        let p = [0.5, 0.08, 0.0];
        let low = auto_mask_gate(p, r, auto_mask_ceiling(20.0));
        let high = auto_mask_gate(p, r, auto_mask_ceiling(80.0));
        assert!(
            high > low,
            "tolerance 80 gave {high}, tolerance 20 gave {low}"
        );
    }

    #[test]
    fn the_ceiling_is_never_zero_or_inverted() {
        // A tolerance of 0 must still paint the pixel its reference was sampled from, and must
        // never make the ramp divide by zero.
        for t in [-50.0, 0.0, 50.0, 100.0, 500.0] {
            let c = auto_mask_ceiling(t);
            assert!(c > 0.0, "tolerance {t} gave ceiling {c}");
            assert_eq!(auto_mask_gate([0.5, 0.0, 0.0], [0.5, 0.0, 0.0], c), 1.0);
        }
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
        let part = rasterize_in(&b, &g, 80, 40, Some(clip), None);
        for y in clip.1..clip.1 + clip.3 {
            for x in clip.0..clip.0 + clip.2 {
                assert_eq!(part.at(x, y), whole.at(x, y), "at frame ({x}, {y})");
            }
        }
    }

    #[test]
    fn an_auto_masked_clipped_raster_agrees_away_from_the_clip_edge() {
        // ⚠⚠ **This is the ACCEPTED COST of reading the reference at render time, pinned so its
        // extent is a number rather than a hope.** A dab is kept when its *disc* overlaps the
        // clip, so its centre may lie up to one radius outside it — and under a region render the
        // base has no pixels there. Those dabs therefore reference a clamped edge pixel instead of
        // the true one, and the clipped raster can differ from the whole-frame one **within one
        // radius of the clip boundary**. Beyond that band they must agree exactly.
        //
        // ⚠ In practice the band is far quieter than this bound suggests, because within a stroke
        // dabs combine with `max` and are spaced `SPACING` (a quarter radius) apart, so almost
        // every pixel in the band is also covered at full alpha by a dab whose centre *is* inside
        // the region. That is measured, not assumed: it is why three separate attempts to make
        // the master region invariant fail under this design all passed.
        let b = brush(vec![BrushStroke {
            decode: false,
            points: vec![[0.05, 0.5], [0.95, 0.5]],
            radius: 0.15,
            hardness: 60.0,
            flow: 100.0,
            erase: false,
            auto_mask: true,
            tolerance: 45.0,
        }]);
        let g = Geometry::default();
        let clip = (30usize, 12usize, 20usize, 16usize);
        let whole = rasterize_in(&b, &g, 80, 40, None, Some(&split_at(40)));
        let part = rasterize_in(&b, &g, 80, 40, Some(clip), Some(&split_at(40)));
        // One radius in frame pixels, the width of the band this cannot promise.
        let band = (0.15f32 * 80.0).ceil() as usize;
        for y in clip.1..clip.1 + clip.3 {
            for x in clip.0 + band..clip.0 + clip.2 - band {
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
        let part = rasterize_in(&b, &g, 80, 40, Some((30, 12, 20, 16)), None);
        assert_eq!(part.at(40, 20), 255, "the dab centre is at frame (40, 20)");
        assert_eq!(part.at(31, 13), 0, "the clip's own corner is not painted");
    }

    #[test]
    fn a_dab_entirely_outside_the_clip_contributes_nothing() {
        // ⚠ Guards the output, not the skip in `keep`. Removing that skip leaves this passing —
        // checked — because the bounding box is clamped to the clip either way. The skip is an
        // optimisation and its comment says so.
        let far = brush(vec![dot(0.9, 0.9, 0.03, 100.0, false)]);
        let part = rasterize_in(
            &far,
            &Geometry::default(),
            80,
            40,
            Some((0, 0, 20, 10)),
            None,
        );
        assert!(part.is_inert(), "nothing inside the clip should be painted");
    }

    #[test]
    fn an_inverted_mask_still_covers_the_clipped_area() {
        // ⚠ The bbox-versus-outside bug must not come back through the clip.
        let b = Brush {
            strokes: vec![dot(0.5, 0.5, 0.05, 100.0, false)],
            invert: true,
        };
        let part = rasterize_in(&b, &Geometry::default(), 80, 40, Some((0, 0, 20, 10)), None);
        assert_eq!(part.at(5, 5), 255, "inverted coverage inside the clip");
        assert_eq!(part.at(40, 20), 255, "and outside it, via `outside`");
    }

    #[test]
    fn no_clip_is_byte_identical_to_the_whole_frame() {
        let b = wide_stroke();
        let g = Geometry::default();
        assert_eq!(
            rasterize_in(&b, &g, 80, 40, None, None),
            rasterize(&b, &g, 80, 40)
        );
    }

    #[test]
    fn a_clip_larger_than_the_frame_is_harmless() {
        // The canvas clamps its region, but a clip must not index out of bounds if one slips
        // through.
        let b = wide_stroke();
        let g = Geometry::default();
        let part = rasterize_in(&b, &g, 80, 40, Some((0, 0, 9999, 9999)), None);
        assert_eq!(part, rasterize(&b, &g, 80, 40));
    }
}

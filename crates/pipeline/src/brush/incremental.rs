//! Painting a stroke without redrawing it (M10.14f): while a stroke grows, only its new dabs are splatted.
//!
//! ⚠⚠ **Exact, not approximate — and why.** A stroke's dabs combine with `max`, which is order-independent and
//! idempotent, so the last stroke's field is the old field `max` the new segment's dabs (its first dab repeats the
//! last one already splatted, harmlessly). Strokes then composite IN ORDER, so the result is the strokes before the
//! last, composited once and kept, with the last one's field laid over them — `raster::composite`'s arithmetic, in its
//! order. `an_incremental_stroke_is_the_full_raster_at_every_step` holds it to the bit.
//!
//! ⚠ Plain brushes only: a gating brush's dabs read the photograph, which this does not take (the render builds those
//! where the buffer is). Measured before this, at 1:1 on an X-T4 frame: a 400-point stroke's paint frame 18 ms on the
//! GPU tail, 27 ms on the CPU — every dab of the stroke, every frame (`bench_brush_paint`).

use super::raster::{composite, dab_alpha, CoverageMap};
use super::{plan_dabs, Dab, DabPlan};
use edit_model::{Brush, BrushStroke, Geometry};

/// The state a growing stroke is painted from.
pub struct Incremental {
    frame: (usize, usize),
    clip: Option<(usize, usize, usize, usize)>,
    geometry: Geometry,
    invert: bool,
    /// Every stroke before the last, and the last as splatted so far.
    head: Vec<BrushStroke>,
    last: BrushStroke,
    /// The clip box in frame pixels: `(x0, y0, w, h)`. Both planes cover it.
    boxed: (usize, usize, usize, usize),
    /// `head` composited.
    head_cov: Vec<f32>,
    /// The last stroke's `max` of dab alphas, before its flow.
    field: Vec<f32>,
    /// What anything has touched, in frame pixels `(x0, y0, x1, y1)` — the map returned covers only this, so a short
    /// stroke's frame does not composite the whole viewport (a first version did: 6.5 → 8.7 ms at 40 points).
    touched: Option<(usize, usize, usize, usize)>,
}

fn widen(t: &mut Option<(usize, usize, usize, usize)>, r: (usize, usize, usize, usize)) {
    if r.2 <= r.0 || r.3 <= r.1 {
        return;
    }
    *t = Some(match *t {
        None => r,
        Some(a) => (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3)),
    });
}

/// Whether `b` is `a` grown — the same stroke with points appended.
fn grows(a: &BrushStroke, b: &BrushStroke) -> bool {
    let same = BrushStroke {
        points: Vec::new(),
        ..a.clone()
    } == BrushStroke {
        points: Vec::new(),
        ..b.clone()
    };
    same && !a.points.is_empty() && b.points.starts_with(&a.points)
}

/// `brush`'s coverage over `clip` (or the whole frame), as [`super::rasterize_in`] would give it with no base —
/// splatting only what is new since `state` when the brush's last stroke grew, and rebuilding `state` otherwise.
pub fn rasterize(
    state: &mut Option<Incremental>,
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap {
    let outside = if brush.invert { 255 } else { 0 };
    let Some(last) = brush.strokes.last().filter(|_| !brush.needs_base()) else {
        *state = None;
        return super::rasterize_in(brush, geometry, frame_w, frame_h, clip, None);
    };
    let head = &brush.strokes[..brush.strokes.len() - 1];
    let reuse = state.as_ref().is_some_and(|s| {
        s.frame == (frame_w, frame_h)
            && s.clip == clip
            && s.geometry == *geometry
            && s.invert == brush.invert
            && s.head == head
            && grows(&s.last, last)
    });
    if !reuse {
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
            *state = None;
            return CoverageMap {
                outside,
                ..Default::default()
            };
        }
        let boxed = (cx0, cy0, cx1 - cx0, cy1 - cy0);
        let mut head_cov = vec![0.0f32; boxed.2 * boxed.3];
        let mut touched = None;
        let plain = |strokes: Vec<BrushStroke>| Brush {
            strokes,
            invert: false,
        };
        if let Some(p) = plan_dabs(
            &plain(head.to_vec()),
            geometry,
            frame_w,
            frame_h,
            clip,
            None,
        ) {
            place(&composite(&p, None), &p, &mut head_cov, boxed);
            widen(&mut touched, (p.x0, p.y0, p.x0 + p.w, p.y0 + p.h));
        }
        *state = Some(Incremental {
            frame: (frame_w, frame_h),
            clip,
            geometry: *geometry,
            invert: brush.invert,
            head: head.to_vec(),
            last: BrushStroke {
                points: Vec::new(),
                ..last.clone()
            },
            boxed,
            head_cov,
            field: vec![0.0f32; boxed.2 * boxed.3],
            touched,
        });
    }
    let s = state.as_mut().expect("built above");
    // The new segment: from the last point already splatted (its dab again, harmless under `max`) to the end.
    let from = s.last.points.len().saturating_sub(1);
    let segment = Brush {
        strokes: vec![BrushStroke {
            points: last.points[from..].to_vec(),
            ..last.clone()
        }],
        invert: false,
    };
    if let Some(p) = plan_dabs(&segment, geometry, frame_w, frame_h, clip, None) {
        splat(&mut s.field, s.boxed, &p.strokes[0].dabs);
        widen(&mut s.touched, (p.x0, p.y0, p.x0 + p.w, p.y0 + p.h));
    }
    s.last = last.clone();
    let flow = (last.flow / 100.0).clamp(0.0, 1.0);
    let op = if last.erase {
        edit_model::MaskOp::Subtract
    } else {
        edit_model::MaskOp::Add
    };
    let Some((tx0, ty0, tx1, ty1)) = s.touched else {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    };
    let (bx, by, bw, _) = s.boxed;
    let mut data = Vec::with_capacity((tx1 - tx0) * (ty1 - ty0));
    for y in ty0..ty1 {
        let row = (y - by) * bw;
        for x in tx0..tx1 {
            let (c, f) = (s.head_cov[row + x - bx], s.field[row + x - bx]);
            let a = f * flow;
            // ⚠ `composite`'s own skip: a zero alpha leaves the head's value untouched, bit for bit.
            let c = if a > 0.0 { op.combine(c, a) } else { c };
            let c = if brush.invert { 1.0 - c } else { c };
            data.push((c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
    }
    CoverageMap {
        x0: tx0,
        y0: ty0,
        w: tx1 - tx0,
        h: ty1 - ty0,
        data,
        outside,
    }
}

/// Copy `cov`, over `plan`'s box, into `into`, over `boxed` — the plan's box lies inside the clip box.
fn place(cov: &[f32], plan: &DabPlan, into: &mut [f32], boxed: (usize, usize, usize, usize)) {
    let (bx, by, bw, _) = boxed;
    for row in 0..plan.h {
        let src = &cov[row * plan.w..(row + 1) * plan.w];
        let at = (plan.y0 + row - by) * bw + (plan.x0 - bx);
        into[at..at + plan.w].copy_from_slice(src);
    }
}

/// `max` every dab's alpha into `field` — the within-stroke rule, over each dab's own rect (`composite`'s bounds).
fn splat(field: &mut [f32], boxed: (usize, usize, usize, usize), dabs: &[Dab]) {
    let (bx, by, bw, bh) = boxed;
    let (x1, y1) = (bx + bw, by + bh);
    for d in dabs {
        let dy0 = (d.y - d.r).floor().clamp(by as f32, y1 as f32) as usize;
        let dy1 = (((d.y + d.r).ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1);
        let dx0 = (d.x - d.r).floor().clamp(bx as f32, x1 as f32) as usize;
        let dx1 = (((d.x + d.r).ceil().clamp(0.0, x1 as f32) as usize) + 1).min(x1);
        for y in dy0..dy1 {
            let row = (y - by) * bw;
            for x in dx0..dx1 {
                let dist =
                    (((x as f32 + 0.5) - d.x).powi(2) + ((y as f32 + 0.5) - d.y).powi(2)).sqrt();
                let a = dab_alpha(dist, d.r, d.hard);
                let slot = &mut field[row + x - bx];
                if a > *slot {
                    *slot = a;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "incremental_tests.rs"]
mod tests;

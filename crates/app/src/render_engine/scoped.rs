//! The region-scoped prefix: when a region is wanted and every step from the resume point accepts
//! one, only the part of the frame the region needs is rendered.

use super::prefix::{PrefixOut, Walk};
use super::RegionRect;
use core_image::ImageBuf;
use std::sync::Arc;

/// The largest share of the frame a **margined** region may occupy and still be worth scoping, in
/// percent.
///
/// ⚠⚠ **Swept, not chosen** — `bench_preview`'s SCOPED DRAG SWEEP, on `DSCF6296.RAF` at a native
/// rung (3123x2085, 6.5 Mpx), chain `perspective -> geometry -> noise_reduction -> dehaze`,
/// dragging `geometry`, best of 3. ⚠ Load average 3.6, so **not a quiet machine**.
///
/// | region (each axis) | margined share | scoped | whole-buffer | |
/// |---|---|---|---|---|
/// | 10% | 2.9% | 8.7 ms | 200.4 ms | 22.9x |
/// | 30% | 13.8% | 34.5 | 204.8 | 5.94x |
/// | 50% | 32.7% | 78.6 | 222.4 | 2.83x |
/// | **60%** | **45.1%** | **109.4** | **213.1** | **1.95x** |
/// | 70% | 59.6% | 146.2 | 218.3 | 1.49x |
/// | 85% | 85.0% | 203.3 | 226.7 | 1.12x |
/// | 100% | 100.0% | 236.6 | 229.9 | **0.97x** |
///
/// ⚠⚠ **Parity on the frame alone is at ~100%, and that is the wrong number to set this from**,
/// because the scoped output is deliberately never cached: a drag of `n` frames costs
/// `n * scoped + whole` against the whole-buffer path's `n * whole + tail`, so the break-even
/// tightens as the gesture shortens. At 45.1% a two-frame gesture already wins (432 ms against
/// 441); at 59.6% it **loses** (511 against 452) and only pays off past about five frames.
///
/// 50 sits between those two rows, so the gate refuses exactly the band where a short gesture would
/// be worse off — the rule that it must fail toward the whole-buffer path, where a wrong answer
/// costs milliseconds and never pixels. The placeholder value turned out to be the right one; what
/// changed is that it is now defensible.
///
/// ⚠ It is the **margined** share, not the visible one. A local's reach is in full-resolution
/// pixels, so at a native rung a modest viewport margins out a long way — 50% of each axis becomes
/// a third of the frame — which is why this is a share rather than an absolute size.
const SCOPE_MAX_AREA_PCT: u64 = 50;

/// A scoped prefix output: the region's pixels, the last rect, the frame's size, and how many
/// steps ran.
pub(super) type Scoped = (ImageBuf, pipeline::Rect, (usize, usize), usize);

/// The region-scoped walk, when its gate engages, and whether it did.
pub(super) fn scope(
    walk: &Walk,
    region: Option<RegionRect>,
    start: usize,
    img: &ImageBuf,
) -> (Option<Scoped>, bool) {
    let Walk {
        ref steps,
        debug_steps,
        ..
    } = *walk;
    // ⚠⚠ **THE REGION-SCOPED PREFIX, and the gate is DERIVED rather than a flag.** Engage when a
    // region is wanted and *every* step from the resume point onward accepts one. There is no drag
    // signal to plumb and therefore nothing that can disagree with anything else — the rule
    // `Session::geometry_mode` was deleted for. It follows that this speeds a cold entry into 1:1
    // as well as a drag: the gate asks about the work, not about the gesture.
    //
    // ⚠⚠ M8.6's version engaged only when *exactly the last* step needed re-running, on the
    // argument that step n-1's scoped output cannot feed step n because step n is a gather. That
    // is true of two gathers in a row and this prefix is never that — its gathers (lens
    // correction, perspective, geometry) all precede its locals (noise reduction, dehaze), so a
    // local needs only a margin and the first scoped step is a gather reading a whole cached
    // buffer. `region_plan` is what establishes that per chain rather than by assertion.
    //
    // ⚠ At the fitted rung `region` is `None`, so this never engages there — it engages exactly
    // at the native rungs, which is where the cost is.
    let mut scoped = None;
    // ⚠ A plain flag rather than something derived from the output's size afterwards: a derived
    // fact that can be wrong is what `Session::geometry_mode` was deleted for.
    let mut was_scoped = false;
    if let Some(want) = region {
        let visible = pipeline::Rect {
            x: want.x,
            y: want.y,
            w: want.w,
            h: want.h,
        };
        // ⚠ `region_plan` clamps `visible` against the frame itself, so no clamping is done here —
        // two clamps would be two rules about the same rect.
        if let Some(plan) = pipeline::region_plan(steps, start, visible, img.width(), img.height())
        {
            // ⚠⚠ **The area gate.** A margined region close to the whole frame costs roughly what
            // the whole-buffer path costs *and is not cached*, so the settle render repeats the
            // work — strictly worse than not scoping. `out_rects[0]` is the earliest scoped output
            // and therefore the largest of them, which is what makes it the right proxy.
            let margined: u64 = plan
                .out_rects
                .first()
                .map_or(0, |r| r.w as u64 * r.h as u64);
            let whole = plan.frame.0 as u64 * plan.frame.1 as u64;
            if whole > 0 && margined * 100 < whole * SCOPE_MAX_AREA_PCT {
                if let Some(out) = pipeline::run_region_plan(steps, start, img, &plan) {
                    let last = *plan.out_rects.last().expect("a plan has at least one rect");
                    if debug_steps {
                        eprintln!(
                            "  prefix: {:?}  resuming at {start} of {}  SCOPED {}x{} -> {}x{} of {}x{}",
                            steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>(),
                            steps.len(),
                            plan.out_rects[0].w,
                            plan.out_rects[0].h,
                            last.w,
                            last.h,
                            plan.frame.0,
                            plan.frame.1,
                        );
                    }
                    // The step count rides in the tuple rather than in a separate local: a count
                    // stored beside the thing it counts is a second fact that can go stale.
                    scoped = Some((out, last, plan.frame, plan.out_rects.len()));
                    was_scoped = true;
                }
            }
        }
    }
    (scoped, was_scoped)
}

/// A scoped output as the tail takes it.
pub(super) fn output((out, last, frame, ran): Scoped, img: Arc<ImageBuf>) -> PrefixOut {
    // ⚠⚠ **DELIBERATELY NOT CACHED.** A region-scoped checkpoint served for a *different*
    // region is the one failure here that shows as a plausible photograph rather than an
    // error, and leaving the insert out is what makes it unrepresentable rather than guarded.
    // The cost is that a later whole-buffer render rebuilds the final checkpoint — against
    // re-running every step whole-buffer on every frame of the drag, which is what this
    // replaces.
    //
    // ⚠⚠ `frame` comes from the **plan**, composed through `output_dims` and read before any
    // step ran. Taking it from `out` would be the region's own size, which rasterises every
    // brush mask at the wrong size and thrashes the coverage cache on every pan — M8.6's
    // sharpest hazard, and it is sharper here because the chain is longer.
    //
    // ⚠ `gpu_prefix_steps` stays 0 here. Nothing is lost that the seam could have supplied:
    // `accelerate_prefix_stage`'s twins run whole-buffer, so a scoped chain is the CPU's.
    PrefixOut {
        input: Some(Arc::new(out)),
        rect: Some(RegionRect {
            x: last.x,
            y: last.y,
            w: last.w,
            h: last.h,
        }),
        pw: frame.0,
        ph: frame.1,
        prefix_stages: ran,
        // ⚠ Unread: the scoped path always hands the tail a CPU input, so no last-resort re-run starts from `img`.
        img_step: 0,
        gpu_prefix_carried: 0,
        img,
        gpu_prefix_steps: 0,
        gpu_prefix_uploads: 0,
        handed: None,
    }
}

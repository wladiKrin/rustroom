//! One render: resume the prefix from its deepest checkpoint, run the tail, pack the frame.

use super::brush_cache::{BrushCache, CoverageSpec};
use super::frame::{FrameSpec, Ran, RenderedFrame};
use super::pack::{pack, Packed};
use super::scale::{render_scale, rendered_long_edge};
use super::RegionRect;
use crate::prefix_cache::{PrefixCache, Tier};
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{build_prefix_steps, PipelineStage, PrefixKey, Region};
use std::sync::Arc;

/// An owned copy of a shared checkpoint: the buffer itself when nothing else holds it, a clone
/// otherwise. The tail's input travels as an `Arc` so the one-chain paths never clone it.
fn unshare(a: Arc<ImageBuf>) -> ImageBuf {
    Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())
}

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

#[cfg(test)]
thread_local! {
    /// Test-only: refuse the tail its resident final for one whole render, so a handoff frame
    /// takes the download fallback. Every tail stage has a GPU twin, so no real edit set produces
    /// the refusal.
    static REFUSE_THE_RESIDENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`render_cached_with`] with the handoff OFF — every checkpoint cached, as it always was. The
/// worker's settle pass and every test that predates the handoff render through this.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_cached(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    spec: FrameSpec,
    source_scale: f32,
    lens: Option<lens_profile::LensShot>,
    wb_baseline: color::wb::Illuminant,
    rendering: pipeline::TailRendering<'_>,
    cache: &mut PrefixCache,
    brush_cache: &mut BrushCache,
) -> (RenderedFrame, Ran) {
    render_cached_with(
        buffer,
        edits,
        spec,
        source_scale,
        lens,
        wb_baseline,
        rendering,
        cache,
        brush_cache,
        false,
    )
}

/// Render one request, resuming from the deepest cached prefix checkpoint.
///
/// The prefix — geometry, noise reduction, dehaze — is by far the expensive half, and a slider
/// move usually changes only one of its stages, or none. So each stage's output is cached
/// separately and the walk restarts at the deepest checkpoint still valid, running only what is
/// downstream of the change. See
/// `docs/superpowers/specs/2026-08-12-second-prefix-checkpoint-design.md`.
///
/// Returns the frame and **how many prefix stages actually ran**. The worker ignores the count;
/// it exists because the correctness tests would all still pass with the cache deleted, so
/// something has to assert that work is genuinely being skipped.
///
/// ⚠⚠ `handoff` lets the LAST prefix step hand its output to the tail on the GPU with no download
/// (2026-09-26). The frame then leaves no final checkpoint, so only the live render asks for it;
/// the worker's settle pass renders again without it, which is what fills the cache on release.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_cached_with(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    spec: FrameSpec,
    source_scale: f32,
    lens: Option<lens_profile::LensShot>,
    wb_baseline: color::wb::Illuminant,
    rendering: pipeline::TailRendering<'_>,
    cache: &mut PrefixCache,
    brush_cache: &mut BrushCache,
    handoff: bool,
) -> (RenderedFrame, Ran) {
    // Destructured so the body below reads exactly as it did before these three were grouped.
    let FrameSpec {
        max_dim,
        region,
        display_space,
        intent,
        resampler,
        gpu_tail,
        visualize,
        soft_proof,
        grade_view,
    } = spec;
    let buffer_long = buffer.width().max(buffer.height());
    // ⚠ Kept for the AI masks' decode mapping (M9.13): `lens` is moved into the prefix below.
    let ai_lens = lens.clone();
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );

    // The prefix never depends on the region, so the whole chain is built and cached without
    // one — which is why panning re-runs only the tail and never invalidates a checkpoint.
    // A missing profile — an unlisted lens, or a JPEG — skips the stage silently, which is the
    // ordinary case and needs no special-casing anywhere else.
    // ⚠⚠ The frame-global airlight, replayed from the SOURCE buffer at a fixed 256 px. It is a
    // deterministic function of the edits, so it is identical on the whole-buffer path and on a
    // region-scoped one — which is what lets the two agree by construction rather than
    // approximately, and what removes any pop when a gesture settles.
    //
    // ⚠ Computed unconditionally rather than only when dehaze is active: `prefix_airlight` runs
    // the prefix on a 65k-pixel copy, and a branch here would be a second place that has to agree
    // with `Dehaze::is_active`.
    // ⚠ The `TIMING` lines below split a frame into its parts under `RUSTROOM_DEBUG_RENDER`. They
    // were added 2026-09-26 when a Straighten drag's non-geometry half was ~54 ms on a photo with
    // an empty tail, and found 12.1 ms of airlight replay and 12.4 ms of single-threaded
    // histogram inside it. The total per-render line above cannot show that.
    let t_air = std::time::Instant::now();
    let airlight = cache.airlight(edits, buffer, resampler, lens.clone());
    if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
        eprintln!(
            "  TIMING airlight {:6.1} ms",
            t_air.elapsed().as_secs_f64() * 1000.0
        );
    }
    // ⚠ Cloned rather than moved: the GPU seam below needs the same profile per step, and a
    // `LensShot` is a handful of bytes plus a shared profile handle.
    let steps = build_prefix_steps(edits, scale, lens.clone(), resampler, Some(airlight));
    // ⚠ Per-step decomposition, env-gated and kept: `RUSTROOM_DEBUG_MASK`'s precedent. A whole-
    // render figure says a drag is slow; only this says WHICH stage is slow, and reasoning about
    // that from a sidecar has already been wrong once here.
    let debug_steps = std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some();
    let key_at = |n: usize| (PrefixKey::upto(&steps, n, scale), max_dim);
    let tier_at = |n: usize| {
        if n == steps.len() {
            Tier::Final
        } else {
            Tier::Mid
        }
    };

    // Resume from the deepest checkpoint still held. `n == 0` is the bare downscale.
    let mut gpu_prefix_steps = 0usize;
    let mut gpu_prefix_uploads = 0usize;
    // ⚠⚠ **When the last step may run WITHOUT a download.** A whole-frame render, sRGB display,
    // and none of the views that need float pixels (proof, visualize, grade range). Its output
    // then lives only as the resident final; the tail runs from it, and the settle downloads it.
    let handoff_ok = handoff
        && gpu_tail
        && region.is_none()
        && grade_view.is_none()
        && visualize.is_none()
        && soft_proof.is_none()
        && display_space == color::OutputSpace::Srgb;
    // The output size of a prefix whose last step handed off — there is no CPU image to ask.
    let mut handed: Option<(usize, usize)> = None;
    // ⚠ Resolved ONCE, and only when the setting is on. `shared()` acquires an adapter on first
    // call — ~42 s in a debug build — and the `None` arm of the loop below would otherwise ask for
    // it on every CPU step of every render, including renders with the GPU switched off.
    let prefix_ctx = if gpu_tail {
        gpu_pipeline::shared().cloned()
    } else {
        None
    };
    let mut resumed = None;
    for n in (0..=steps.len()).rev() {
        if let Some(img) = cache.get(&key_at(n)) {
            resumed = Some((n, img));
            break;
        }
    }

    let (start, mut img) = match resumed {
        Some(hit) => hit,
        None => {
            // ⚠ When nothing needs shrinking, `downscale_to` is a clone of a buffer the engine
            // already owns behind an `Arc`. Sharing it makes checkpoint 0 free in both time and
            // memory; caching it would cost a whole buffer (78 MB on an X-T4) to save 2.2 ms.
            let seed_is_free = max_dim == 0 || buffer_long <= max_dim;
            let seed = if seed_is_free {
                Arc::clone(buffer)
            } else {
                let s = Arc::new(buffer.downscale_to(max_dim));
                cache.insert(key_at(0), s.clone(), tier_at(0));
                s
            };
            (0, seed)
        }
    };

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
        if let Some(plan) = pipeline::region_plan(&steps, start, visible, img.width(), img.height())
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
                if let Some(out) = pipeline::run_region_plan(&steps, start, &img, &plan) {
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

    let (input, rect, pw, ph, prefix_stages) = if let Some((out, last, frame, ran)) = scoped {
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
        (
            Some(Arc::new(out)),
            Some(RegionRect {
                x: last.x,
                y: last.y,
                w: last.w,
                h: last.h,
            }),
            frame.0,
            frame.1,
            ran,
        )
    } else {
        // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU seam
        // clean — so each step that runs clones the checkpoint it starts from. Measured at 0.5 ms
        // (fit) and 2.1 ms (native) against the 13–117 ms each avoided stage costs.
        if debug_steps {
            eprintln!(
                "  prefix: {:?}  resuming at {start} of {}",
                steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>(),
                steps.len()
            );
        }
        // ⚠⚠ **A deposit must not outlive its render.** The tail may decline to take — it runs on
        // the CPU whenever the setting is off, and an export's tail always does — leaving the plane
        // parked. The next render is the same photograph at the same rung on the same worker
        // thread, so every check `SharedPlane::take` makes would pass and the *previous frame's*
        // pixels would be handed to the first prefix step. Clearing at the top bounds a deposit to
        // one render, which is `CoverageRasterizer::end_mask`'s rule one level up.
        if let Some(ctx) = &prefix_ctx {
            ctx.prefix_plane().clear();
        }
        for (i, step) in steps.iter().enumerate().skip(start) {
            let t_step = std::time::Instant::now();
            // ⚠⚠ **The prefix's GPU seam, and it must be here rather than where `steps` is built.**
            // `dehaze::airlight` is estimated from the image the stage is HANDED — the checkpoint
            // after lens correction and noise reduction since M7.3b — so substituting up front would
            // estimate it from the raw buffer. A slightly different airlight is a different
            // photograph, not an error.
            //
            // ⚠ The step's `key` is never touched: `PrefixStepKey` is derived from the edits, so a
            // checkpoint written by either path is a legitimate hit for the other and `PrefixCache`
            // needs no change at all.
            let mut accelerated = if gpu_tail {
                gpu_pipeline::shared().and_then(|ctx| {
                    gpu_pipeline::accelerate_prefix_stage(
                        step.stage.name(),
                        edits,
                        scale,
                        &img,
                        // ⚠ The facts `Edits` cannot carry — the matched profile, the kernel, and
                        // ⚠⚠ since 2026-09-17 the airlight. `GpuDehazeStage` used to estimate that
                        // from `img`, which under a Straighten drag is the ROTATED frame, so the
                        // dehazed tone breathed as the angle moved. Found by use.
                        gpu_pipeline::SeamFacts {
                            lens: lens.as_ref(),
                            resampler,
                            airlight,
                        },
                        ctx,
                    )
                })
            } else {
                None
            };
            let is_last = i + 1 == steps.len();
            if let Some(g) = accelerated.as_mut() {
                g.allow_shared_input(gpu_pipeline::Slot::Prefix);
                // ⚠ The last step keeps its output for the tail instead of offering it to a next
                // step there is not: one buffer, one owner.
                if !is_last {
                    g.offer_output(gpu_pipeline::Slot::Prefix);
                }
            }
            // ⚠⚠ **THE LAST STEP KEEPS ITS OUTPUT ON THE GPU — the resident final (2026-09-26).**
            // With a download it is also the CPU checkpoint, as always; without one (`handoff_ok`)
            // it is the tail's only input, and `img` is left as the checkpoint before this step.
            // Any `None` falls through to the ordinary path below.
            let kept_last = match (is_last, accelerated.as_ref()) {
                (true, Some(g)) => {
                    let ctx = gpu_pipeline::shared();
                    let from_resident = if i == start {
                        ctx.and_then(|ctx| cache.resident(&key_at(i), &img, ctx))
                    } else {
                        None
                    };
                    let run = match from_resident {
                        Some((r, uploaded)) => {
                            gpu_prefix_uploads += usize::from(uploaded);
                            g.run_keeping_output(gpu_pipeline::Input::Resident(r), !handoff_ok)
                        }
                        None => g.run_keeping_output(gpu_pipeline::Input::Cpu(&img), !handoff_ok),
                    };
                    gpu_prefix_uploads += g.transfer_counts().0;
                    run
                }
                _ => None,
            };
            if let Some((out, kept)) = kept_last {
                gpu_prefix_steps += 1;
                let dims = (kept.width(), kept.height());
                cache.keep_resident_final((key_at(i + 1), None), kept);
                match out {
                    Some(out) => {
                        img = Arc::new(out);
                        if debug_steps {
                            eprintln!(
                                "    [{i}] {:>16} {:8.1} ms   -> {}x{} ({:.1} Mpx) KEPT",
                                step.stage.name(),
                                t_step.elapsed().as_secs_f64() * 1000.0,
                                img.width(),
                                img.height(),
                                (img.width() * img.height()) as f64 / 1e6,
                            );
                        }
                        cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
                    }
                    None => {
                        handed = Some(dims);
                        if debug_steps {
                            eprintln!(
                                "    [{i}] {:>16} {:8.1} ms   -> {}x{} HANDED OFF (no download)",
                                step.stage.name(),
                                t_step.elapsed().as_secs_f64() * 1000.0,
                                dims.0,
                                dims.1,
                            );
                        }
                    }
                }
                break;
            }
            img = Arc::new(match accelerated {
                Some(g) => {
                    // ⚠⚠ **Take from the previous step and offer to the next — the whole of the
                    // resident prefix seam.** Consecutive accelerated steps are safe by
                    // construction: step *i+1*'s input **is** step *i*'s output, so the plane the
                    // previous step parked is exactly the one this step would otherwise upload.
                    // Measured at rung 6246: 17.9-23.5 ms a seam, all of it host memcpy and
                    // allocation, because `SharedPlane::take` refuses anything that is not this
                    // thread's deposit at these exact dimensions.
                    //
                    // ⚠ `Slot::Prefix`, never `Slot::Mask` — see `GpuContext::prefix_plane` for
                    // the guard that separation preserves.
                    gpu_prefix_steps += 1;
                    // ⚠⚠ **The resident input, for the FIRST step this render runs** — the one a
                    // drag resumes at on every frame. Later steps take the previous step's plane
                    // instead. Any `None` falls back to the ordinary upload.
                    let from_resident = if i == start {
                        let ctx = gpu_pipeline::shared();
                        ctx.and_then(|ctx| cache.resident(&key_at(i), &img, ctx))
                            .and_then(|(r, uploaded)| {
                                gpu_prefix_uploads += usize::from(uploaded);
                                g.apply_from(r)
                            })
                    } else {
                        None
                    };
                    let out = match from_resident {
                        Some(out) => out,
                        None => g.apply((*img).clone()),
                    };
                    gpu_prefix_uploads += g.transfer_counts().0;
                    out
                }
                None => {
                    // ⚠⚠ **THE CLEAR, AND WITHOUT IT THIS RENDERS A PLAUSIBLE PHOTOGRAPH.** A CPU
                    // step between two accelerated ones breaks the "its input is the previous
                    // output" invariant the take rests on: the parked plane is now two stages
                    // stale, at the *same* dimensions on the *same* thread, so `take` would
                    // happily hand it over. Nothing downstream can tell — it is a picture, not an
                    // error, which is the failure mode this project has recorded more than any
                    // other.
                    if let Some(ctx) = &prefix_ctx {
                        ctx.prefix_plane().clear();
                    }
                    step.stage.apply((*img).clone())
                }
            });
            if debug_steps {
                eprintln!(
                    "    [{i}] {:>16} {:8.1} ms   -> {}x{} ({:.1} Mpx)",
                    step.stage.name(),
                    t_step.elapsed().as_secs_f64() * 1000.0,
                    img.width(),
                    img.height(),
                    (img.width() * img.height()) as f64 / 1e6,
                );
            }
            cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
        }
        let ran = steps.len() - start;

        // ⚠ The complete prefix output must always live in the protected tier, even when we
        // resumed at it without running anything. A prefix that *shrinks* — its last stage going
        // neutral — turns a former intermediate into the final, and leaving it in the budgeted
        // tier would let a large render evict the very thing the zoom guarantee rests on.
        // `steps.is_empty()` is excluded because there the final *is* checkpoint 0, which is
        // either free (and must not be stored) or was already inserted as a final above.
        if ran == 0 && !steps.is_empty() {
            cache.insert(key_at(steps.len()), img.clone(), Tier::Final);
        }

        // The region can only be resolved once the prefix output's dimensions are known, which is
        // why the tail is built here rather than alongside the steps. Building is a few Box
        // allocations.
        let (pw, ph) = handed.unwrap_or((img.width(), img.height()));
        let rect = region.and_then(|r| r.clamped(pw, ph));
        let t_clone = std::time::Instant::now();
        // ⚠ No CPU input after a handoff: the tail's input is the resident final.
        // ⚠⚠ **Shared, not cloned (2026-09-26).** The whole-frame input IS the cached final, and
        // cloning it cost 2.5 ms at rung 3200 on every tail-slider frame — paid before the tail
        // could know it would run from the resident and never touch the CPU image. The Arc is
        // handed down; `unshare` clones only on the paths that must own a buffer.
        let input = match (handed, rect) {
            (Some(_), _) => None,
            (None, Some(r)) => Some(Arc::new(img.crop(r.x, r.y, r.w, r.h))),
            (None, None) => Some(Arc::clone(&img)),
        };
        if debug_steps {
            eprintln!(
                "  TIMING input clone {:6.1} ms",
                t_clone.elapsed().as_secs_f64() * 1000.0
            );
        }
        (input, rect, pw, ph, ran)
    };
    let region_scoped = was_scoped;

    // ⚠⚠ **The frame dimensions are `(pw, ph)` — the prefix output's — and on the region-scoped
    // path they come from `output_dims` rather than from the buffer.** That buffer *is* the region
    // there, so taking its size would rasterise every brush mask at the region's size and thrash
    // the coverage cache on every pan.
    let region = rect.map(|r| Region {
        full_w: pw,
        full_h: ph,
        x0: r.x,
        y0: r.y,
    });

    // ⚠ The frame dimensions are the PREFIX OUTPUT's — `(pw, ph)` — not the region's. That image
    // *is* the frame. Passing the region's would rasterize at the wrong size and, because the dims
    // are in the key, would also thrash the cache on every pan.
    // ⚠ The clip comes from `rect`, which carries the region's own width and height. `Region`
    // holds only the origin plus the full frame's size, so deriving the size as "frame minus
    // origin" would clip to the bottom-right corner of the frame rather than to the viewport —
    // larger than intended, and silently so, because the output would still be correct and merely
    // slow.
    let clip = rect.map(|r| (r.x, r.y, r.w, r.h));
    let cov_spec = CoverageSpec {
        fw: pw,
        fh: ph,
        region: clip,
        max_dim,
    };
    let t_cov = std::time::Instant::now();
    let coverage = brush_cache.get_or_build(
        edits,
        cov_spec,
        (buffer.width(), buffer.height()),
        ai_lens.as_ref(),
    );
    if debug_steps {
        eprintln!(
            "  TIMING coverage {:6.1} ms",
            t_cov.elapsed().as_secs_f64() * 1000.0
        );
    }
    // ⚠⚠ **M8.3g's gate, derived ONCE and passed down rather than re-derived.** Three things
    // downstream need the float image, and each is a view mode or a setting:
    //
    //   1. a **non-sRGB display space** — `pack` converts from the float deliberately, because an
    //      8-bit → 8-bit conversion costs roughly 0.8 bits and bands a smooth sky. ⚠ Opt-in and
    //      Linux/Windows-only: on macOS `tag_as_srgb` succeeds, so `effective_display_space`
    //      returns `Srgb` whatever the dropdown says.
    //   2. **Visualize Spots** — `edge_image` is computed from the float.
    //   3. **soft proofing** — the ProPhoto run is decoded back to the working space to test the
    //      gamut, which an 8-bit round trip cannot support.
    //
    // ⚠ The **grade range view** is a fourth and needs nothing: it bypasses `accelerate` entirely,
    // building its own halves through `build_pipeline_split_tapped`.
    //
    // ⚠ A four-part condition spanning two crates is the shape this project's record says goes
    // wrong silently, so the other half — "is this chain last, and does it end in
    // `output_transform`" — is answered by construction inside `accelerate` rather than here.
    let float_needed =
        display_space != color::OutputSpace::Srgb || visualize.is_some() || soft_proof.is_some();
    let build_tail = |space| {
        // ⚠ Hoisted into a local because `accelerate` needs it too: `output_transform` is built
        // from the render's output space rather than from `Edits`, and two constructions of one
        // spec is how the pipeline and its accelerated twin would come to disagree about which
        // space they are rendering into.
        let render_spec = pipeline::RenderSpec {
            space,
            intent,
            // ⚠ The tail resamples nothing — every resampling stage is in the prefix — so this
            // is carried for consistency rather than because it changes a pixel here.
            resampler,
            // ⚠ Likewise: dehaze is a PREFIX stage, so the tail never builds one. Carried so the
            // two specs cannot drift if a stage ever moves.
            airlight: Some(airlight),
            baseline: wb_baseline,
        };
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            scale,
            region,
            Some(&coverage),
            render_spec,
            // Lens correction lives in the *prefix*, which this call discards — only the tail is
            // wanted here. `None` is therefore correct rather than provisional.
            None,
            // ⚠ The display rendering, by contrast, lives in the **tail** — so this must be
            // carried, or every render would silently take the built-in curve and the built-in
            // baseline while a profile that declares its own sat unused.
            rendering,
        );
        // ⚠⚠ The tail only. Every accelerated stage is a tail stage, and accelerating the prefix
        // would make `PrefixKey` describe two different computations under one key.
        //
        // ⚠ `shared()` returning `None` silently keeps the CPU path: a machine with no adapter
        // must not fail to render.
        //
        // ⚠⚠ **`if` rather than `match (gpu_tail, shared())`, and this was a real bug rather than
        // style.** A tuple scrutinee evaluates **both** elements before matching, so `shared()`
        // ran even with the setting OFF — acquiring an adapter on the render worker, which costs
        // ~42 s in a debug build. Three render tests began reporting "no frame produced" after a
        // one-second wait. With the setting off nothing GPU-related may be touched at all.
        if gpu_tail {
            match gpu_pipeline::shared() {
                Some(ctx) => gpu_pipeline::accelerate(
                    tail,
                    edits,
                    scale,
                    gpu_pipeline::TailFacts {
                        region,
                        rendering,
                        spec: render_spec,
                    },
                    ctx,
                ),
                None => (tail, None),
            }
        } else {
            (tail, None)
        }
    };

    // ⚠⚠ **While proofing the tail runs TWICE, and that was MEASURED into existence rather than
    // argued.** A single ProPhoto run was the first design; run over 12 real X-T4 frames, 12 of 12
    // histograms moved — worst **five 8-bit levels** with the camera rendering off, because
    // ProPhoto's `encode` ceilings genuine scene headroom at 1.0 *before* the sRGB matrix mixes the
    // channels, where the direct path clamps only afterwards. So the histogram comes from an
    // unmodified sRGB run and is bit-identical **by construction**, not by luck, and the texture
    // and the warning come from a ProPhoto run that reaches `pack` near-unclipped.
    //
    // ⚠ Cost: one extra tail run per proofed frame — ~7 ms at fit, ~13 ms at a native rung —
    // paid only while the proof is switched on. That is accepted; it is not something to optimise
    // away by collapsing the two runs back into one.
    //
    // ⚠ Suppressed while Visualize Spots is on. That view replaces the photograph with an
    // achromatic edge view, so proofing it would mark nothing and show nothing; suppressing it
    // here also means the extra run is never paid for a frame that could not use it. The two are
    // both view modes applied at this point and the diagnostic one wins.
    // ⚠⚠ **The tail's input, and where it lives (2026-09-26).** `tail_key` names it: the final
    // checkpoint, at this rect. `PrefixCache::resident_final` holds it on the GPU under exactly
    // that key when a previous frame ran from it, or the last prefix step just kept it. A tail
    // that is one GPU chain runs FROM it; a tail that cannot (a CPU stage first, a GPU failure)
    // gets a CPU image — and on a handoff frame that means downloading the resident, which is
    // the settle's own path and caches the final as a side effect. Nothing else in this function
    // needs to know which happened.
    let tail_key = (key_at(steps.len()), rect);
    let mut tail_uploads = 0usize;
    let mut resident_final_hit = false;
    let refused = {
        #[cfg(test)]
        {
            REFUSE_THE_RESIDENT.with(|l| l.get())
        }
        #[cfg(not(test))]
        {
            false
        }
    };
    let mut from_resident: Option<Packed> = None;
    let input: Arc<ImageBuf> = match input {
        Some(i) => i,
        None => {
            // ⚠ A handoff frame: no CPU image exists. The resident final must serve the tail
            // directly; if it cannot, it is downloaded and cached as the final.
            let t_res = std::time::Instant::now();
            let ctx = gpu_pipeline::shared();
            let from_gpu = if refused {
                None
            } else {
                ctx.and_then(|_| {
                    let (pipe, chain) = build_tail(color::OutputSpace::Srgb);
                    let chain = chain.filter(|_| pipe.stage_names().is_empty())?;
                    let r = cache.resident_final(&tail_key)?;
                    let bytes = chain.run_rgba8_from(r)?;
                    tail_uploads += chain.transfer_counts().0;
                    Some(Packed::Rgba8 {
                        bytes,
                        w: pw,
                        h: ph,
                    })
                })
            };
            match from_gpu {
                Some(p) => {
                    resident_final_hit = true;
                    if debug_steps {
                        eprintln!(
                            "  TIMING tail from resident {:6.1} ms",
                            t_res.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    from_resident = Some(p);
                    Arc::new(ImageBuf::new(0, 0))
                }
                None => match ctx {
                    Some(ctx) if cache.settle_from_resident(ctx) => cache
                        .get(&key_at(steps.len()))
                        .expect("settle_from_resident inserted the final"),
                    // ⚠ The last resort — the GPU refused even a download. The step re-runs on
                    // the CPU from the checkpoint before it, which `img` still is.
                    _ => {
                        let last = steps.last().expect("a handoff implies a prefix step");
                        let out = Arc::new(last.stage.apply((*img).clone()));
                        cache.insert(key_at(steps.len()), out.clone(), Tier::Final);
                        out
                    }
                },
            }
        }
    };
    let proof = soft_proof.filter(|_| visualize.is_none());
    // ⚠ `input` is cloned **only** when proofing. With the proof off there is one tail, one run,
    // and no clone — bit-exactly the path that existed before this feature.
    let proofed = proof.map(|p| {
        // ⚠ Rejoined at once: the proof is one of the three things that needs the float, so its
        // tail is never a candidate for the byte path.
        let (pipe, chain) = build_tail(color::OutputSpace::ProPhoto);
        (gpu_pipeline::rejoin(pipe, chain).run((*input).clone()), p)
    });
    // ⚠ The **pipeline** is always sRGB here, whatever the proof does. The display transform is a
    // post-step in `pack`, deliberately outside the pipeline so the histogram stays image-referred.
    //
    // ⚠⚠ The grade range view taps the tail at the grading stage's INPUT, not `pack`'s output:
    // the ranges are selected on Oklab L before the grade's own luminance offsets, the masks,
    // the display rendering S-curve and the creative LUT move it. `None` is the path that
    // existed before this — one tail, one run, no clone. `Some` runs the same stages in the same
    // order with one read between two halves, so the frame's pixels are identical too
    // (`the_grade_view_does_not_touch_the_photograph`).
    let (out, plane) = match (from_resident, grade_view) {
        (Some(p), _) => (p, None),
        (None, grade_view) => match grade_view {
            Some(range) => {
                let (_, halves) = pipeline::build_pipeline_split_tapped(
                    edits,
                    scale,
                    region,
                    Some(&coverage),
                    pipeline::RenderSpec {
                        space: color::OutputSpace::Srgb,
                        intent,
                        resampler,
                        baseline: wb_baseline,
                        airlight: Some(airlight),
                    },
                    None,
                    rendering,
                );
                let mid = halves.before_grade.run(unshare(input));
                // ⚠ Blending and Balance are read from the edits whether or not the grade is
                // active: the ranges exist before a wheel is turned.
                let g = &edits.color_grading;
                let plane = crate::grade_view::coverage(&mid, range, g.blending, g.balance);
                (
                    Packed::Float(halves.from_grade.run(mid)),
                    Some(Arc::new(plane)),
                )
            }
            None => {
                let t_build = std::time::Instant::now();
                let (pipe, chain) = build_tail(color::OutputSpace::Srgb);
                if debug_steps {
                    eprintln!(
                        "  TIMING build_tail {:6.1} ms  cpu stages {:?}  gpu chain {}",
                        t_build.elapsed().as_secs_f64() * 1000.0,
                        pipe.stage_names(),
                        chain.is_some(),
                    );
                }
                // ⚠⚠ **M8.3g: the tail's last chain quantizes on the GPU and downloads 27 MB instead
                // of 82.** Measured at rung 3200: 6.33 ms of copy+map plus a 6.87 ms `to_vec` become
                // 2.59 + 2.19, and `pack`'s own `to_rgba8` disappears rather than moving.
                match chain {
                    Some(c) if !float_needed => {
                        let t_cpu = std::time::Instant::now();
                        // ⚠⚠ **One chain, no clone.** When the tail is one GPU chain its input IS
                        // the checkpoint: a resident hit borrows nothing, and a miss borrows the
                        // Arc for its upload. Only a CPU stage before the chain needs an owned
                        // buffer, and `unshare` clones it there — which is what every frame paid.
                        let whole = pipe.stage_names().is_empty();
                        let (src, mid) = if whole {
                            (Some(input), None)
                        } else {
                            (None, Some(pipe.run(unshare(input))))
                        };
                        let (w, h) = match (&src, &mid) {
                            (Some(s), _) => (s.width(), s.height()),
                            (None, Some(m)) => (m.width(), m.height()),
                            (None, None) => unreachable!("one of the two is the chain's input"),
                        };
                        let t_gpu = std::time::Instant::now();
                        // ⚠⚠ **From the resident final when the tail is one chain**, else a
                        // miss that keeps its upload for the next frame. A tail with a CPU stage
                        // first has an input that is not the checkpoint, and keeps nothing.
                        let got = match (&src, &mid) {
                            (Some(s), _) => {
                                match cache.resident_final(&tail_key).filter(|_| !refused) {
                                    Some(r) => {
                                        resident_final_hit = true;
                                        c.run_rgba8_from(r)
                                    }
                                    None => c.run_rgba8_keeping_input(s).map(|(b, kept)| {
                                        cache.keep_resident_final(tail_key.clone(), kept);
                                        b
                                    }),
                                }
                            }
                            (None, Some(m)) => c.run_rgba8(m),
                            (None, None) => None,
                        };
                        tail_uploads += c.transfer_counts().0;
                        if debug_steps {
                            eprintln!(
                                "  TIMING cpu tail {:6.1} ms  gpu run_rgba8 {:6.1} ms",
                                (t_gpu - t_cpu).as_secs_f64() * 1000.0,
                                t_gpu.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                        match got {
                            Some(b) => (Packed::Rgba8 { bytes: b, w, h }, None),
                            // ⚠ A GPU failure falls back through `apply`, which retries once and then
                            // takes the CPU path. One wasted attempt on a rare path beats a second
                            // fallback implementation here that could drift from the first.
                            // ⚠ Fully qualified rather than importing `PipelineStage` into this
                            // module: this is the only place that needs it, and the import would sit
                            // in scope for a whole file that deliberately talks in pipelines.
                            None => {
                                let owned = match (src, mid) {
                                    (Some(s), _) => unshare(s),
                                    (None, Some(m)) => m,
                                    (None, None) => unreachable!("checked above"),
                                };
                                (
                                    Packed::Float(pipeline::PipelineStage::apply(&c, owned)),
                                    None,
                                )
                            }
                        }
                    }
                    // ⚠ The float path keeps the tail's input too, when the tail is one chain: a
                    // non-sRGB display, Visualize Spots, or the proof's sRGB run.
                    Some(c) if pipe.stage_names().is_empty() => {
                        let hit = match cache.resident_final(&tail_key).filter(|_| !refused) {
                            Some(r) => {
                                resident_final_hit = true;
                                c.apply_from(r)
                            }
                            None => None,
                        };
                        let out = match hit {
                            Some(o) => o,
                            None => {
                                let (o, kept) = c.apply_keeping_input(unshare(input));
                                if let Some(k) = kept {
                                    cache.keep_resident_final(tail_key.clone(), k);
                                }
                                o
                            }
                        };
                        tail_uploads += c.transfer_counts().0;
                        (Packed::Float(out), None)
                    }
                    other => (
                        Packed::Float(gpu_pipeline::rejoin(pipe, other).run(unshare(input))),
                        None,
                    ),
                }
            }
        },
    };
    // ⚠⚠ **Harvest what the stages built, after every tail has run.** A gating brush's coverage
    // can only be rasterized where the image is, inside `MaskStage::apply`, so this is the only
    // moment the cache can see it. One point covers all three paths — the proofed tail above, the
    // grade-view split and the plain one — because they have all run by here.
    //
    // ⚠ `take_built` clears each slot, so a second harvest would find nothing: a path that
    // deposits twice cannot store a map twice, and a path that is missed loses the win silently
    // rather than caching something wrong. That is the safe direction of the two.
    for (i, m) in edits.masks.iter().enumerate() {
        for j in 0..=m.refine.len() {
            if let Some(map) = coverage.take_built(i, j) {
                brush_cache.remember(edits, cov_spec, i, j, map);
            }
        }
    }
    // ⚠ **Derived** from what was actually produced rather than tracked alongside it. Two facts
    // that can disagree is the shape `render_max_dim` and `Session::geometry_mode` were each
    // fixed for; here there is only one.
    let bytes = matches!(out, Packed::Rgba8 { .. });
    let t_pack = std::time::Instant::now();
    let mut frame = pack(out, proofed, region, max_dim, display_space, visualize);
    if debug_steps {
        eprintln!(
            "  TIMING pack {:6.1} ms",
            t_pack.elapsed().as_secs_f64() * 1000.0
        );
    }
    frame.grade_view = plane;
    // ⚠ `edits` here is already the **view** edits — `Session::view_edits` has run
    // `preview_edits`, which is what resets the crop in geometry mode — so this is the geometry
    // the render was actually asked for, which is what `fit_size` requires.
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(buffer_long, crate::view_region::PREVIEW_MAX_DIM),
        &edits.geometry,
    );
    (
        frame,
        Ran {
            prefix_stages,
            gpu_prefix_steps,
            gpu_prefix_uploads,
            handoff: handed.is_some(),
            region_scoped,
            bytes,
            tail_uploads,
            resident_final_hit,
        },
    )
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    display_space: color::OutputSpace,
    resampler: pipeline::Resampler,
    lens: Option<lens_profile::LensShot>,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    // ⚠⚠ **The same airlight definition `render_cached` uses, from the same source buffer.** This
    // function is the reference the cached path is tested against, so a second definition here
    // would make `every_cached_frame_matches_a_cold_render` fail — which is exactly how this was
    // caught, at one 8-bit level scattered across the frame.
    //
    // ⚠ `lens` is a parameter rather than `None` for that reason: "exact whenever the engine
    // happens to be lens-less" is a coincidence, not a guarantee, and this is a reference.
    let spec = pipeline::RenderSpec {
        space: color::OutputSpace::Srgb,
        // ⚠ The default, not the canvas's: this is a reference, and its one production output
        // (the decode baseline) is never drawn.
        intent: color::RenderingIntent::default(),
        resampler,
        baseline: color::wb::Illuminant::default(),
        airlight: Some(pipeline::prefix_airlight(edits, buffer, resampler, lens)),
    };
    let mut frame = pack(
        Packed::Float(
            // ⚠⚠ **`colorimetric()` on purpose, and it is now SAID rather than inherited.** This is
            // the before/after baseline and the reference `render_cached` is tested against, so it
            // must not move with a camera profile or a `.cube` — the same argument the `None` proof
            // below makes about the view. It used to get this by default, which meant nobody could
            // tell the choice from an omission; that ambiguity is what cost three defects on
            // 2026-09-18.
            pipeline::build_pipeline_spec(
                edits,
                scale,
                spec,
                pipeline::TailRendering::colorimetric(),
            )
            .run(preview),
        ),
        // ⚠ Never proofed. This is the before/after baseline and the reference the cached path is
        // tested against; a proof is a property of the *view* and would make the reference move
        // with a menu setting.
        None,
        None,
        max_dim,
        display_space,
        None,
    );
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(
            buffer.width().max(buffer.height()),
            crate::view_region::PREVIEW_MAX_DIM,
        ),
        &edits.geometry,
    );
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade_view::GradeRangeView;
    use crate::render_engine::fixtures::{
        chained_edits, chained_spec, full_prefix, hazy_buffer, hsl_edits, render_with, tiny,
        varied, varied_rect, varied_sized,
    };
    use crate::render_engine::frame::Proof;
    use pipeline::RenderScale;

    /// ⚠⚠ **That `FrameSpec.resampler` actually reaches `build_prefix_steps`.** Every other
    /// test in this file passes the same kernel on both sides of whatever it compares — a
    /// cached frame against a cold one, a region against the whole — so `render_cached`
    /// substituting a constant for `spec.resampler` leaves all of them green *and* leaves the
    /// setting inert. Requests being issued proves nothing about which kernel ran.
    #[test]
    fn the_frame_spec_resampler_reaches_the_render() {
        // ⚠ A hard edge, not flat grey: `tiny()` is uniform, and every kernel agrees exactly on
        // a uniform image. That fixture cannot see this bug.
        let mut img = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = if (x / 4) % 2 == 0 { 0.05 } else { 0.9 };
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(img);
        let edits = Edits {
            geometry: edit_model::Geometry {
                angle: 5.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let render = |k| {
            render_cached(
                &buf,
                &edits,
                FrameSpec {
                    max_dim: 32,
                    region: None,
                    display_space: color::OutputSpace::Srgb,
                    intent: color::RenderingIntent::default(),
                    resampler: k,
                    gpu_tail: false,
                    visualize: None,
                    soft_proof: None,
                    grade_view: None,
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                // ⚠ A fresh cache each time, or the second call would legitimately hit the
                // first's checkpoint and the test would be asserting about caching instead.
                pipeline::TailRendering::default(),
                &mut PrefixCache::default(),
                &mut BrushCache::default(),
            )
            .0
            .rgba
        };
        assert_ne!(
            render(pipeline::Resampler::Bilinear),
            render(pipeline::Resampler::Lanczos3),
            "the kernel in the spec never reached the pipeline"
        );
    }

    /// A deliberately non-square fixture: 43 rows do not survive a halving cleanly, so
    /// `downscale_to`'s rounding is observable between rungs. A square one could not see it.
    fn oblong() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(64, 43);
        for (i, s) in img.data_mut().iter_mut().enumerate() {
            *s = (i % 251) as f32 / 251.0;
        }
        Arc::new(img)
    }

    /// ⚠⚠ **The property the shipped code fails.** `fit_image_size` is what every overlay is
    /// placed against, and it must not depend on which rung last landed. Rendering the same photo
    /// at two rungs must report the same fit size.
    ///
    /// ⚠ Non-square on purpose. At rung 16 the 43 rows round to 11, and scaling that back up by
    /// `fit_cap / max_dim` gives 44 against the true 43 — which is the bug, one rung down, in
    /// miniature.
    #[test]
    fn the_fit_size_is_the_same_at_every_rung() {
        let buf = oblong();
        let at = |max_dim: usize| {
            render_once(
                &buf,
                &Edits::default(),
                max_dim,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .fit_size
        };
        let lo = at(16);
        let hi = at(64);
        assert!(
            (lo.0 - hi.0).abs() < 1e-3 && (lo.1 - hi.1).abs() < 1e-3,
            "the fit size was {lo:?} at one rung and {hi:?} at another — it is being derived \
             from the rendered frame's rounded dimensions"
        );
    }

    // ─────────────── M8.6: the region-scoped prefix ───────────────
    /// A straighten plus a vignette.
    ///
    /// ⚠⚠ **The vignette is what makes this fixture able to see the §5 hazard at all.** It is the
    /// one tail stage whose result depends on WHERE a pixel is, so a `Region` carrying the
    /// region's own size instead of the frame's re-centres the falloff and the bytes differ.
    /// Without it the frame dimensions could be taken off the region-scoped buffer and every byte
    /// would still match — the test would name the hazard and be unable to detect it.
    fn scoped_edits() -> Edits {
        Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    fn scoped_spec() -> FrameSpec {
        FrameSpec {
            region: Some(RegionRect {
                x: 3,
                y: 2,
                w: 9,
                h: 7,
            }),
            ..FrameSpec::fit(24)
        }
    }

    /// M9.13, end to end through the preview's render: an AI mask whose map is in the shared store
    /// darkens exactly the half the map covers — and before the map arrives it darkens nothing.
    #[test]
    fn an_ai_mask_renders_from_the_shared_map_and_not_before() {
        let mut b = ImageBuf::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                b.set_pixel(x, y, [0.18; 3]);
            }
        }
        let buf = Arc::new(b);
        let e = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: -2.0,
                    ..Default::default()
                },
                disabled: false,
            }],
            ..Edits::default()
        };
        let mut bc = BrushCache::default();
        let (before, _) = render_with(
            &buf,
            &e,
            FrameSpec::fit(64),
            &mut PrefixCache::default(),
            &mut bc,
        );
        let px = |f: &RenderedFrame, x: usize, y: usize| f.rgba[(y * f.width + x) * 4] as i32;
        assert_eq!(
            px(&before, 5, 16),
            px(&before, 58, 16),
            "no map yet: nothing is covered"
        );

        // The subject is the LEFT half of the decode.
        let map = ai_mask::ProbMap::from_fn(32, 16, |x, _| if x < 16 { 1.0 } else { 0.0 });
        bc.ai
            .write()
            .unwrap()
            .insert(edit_model::AiPrompt::Subject, Arc::new(map));
        let (after, _) = render_with(
            &buf,
            &e,
            FrameSpec::fit(64),
            &mut PrefixCache::default(),
            &mut bc,
        );
        assert!(
            px(&after, 5, 16) + 20 < px(&after, 58, 16),
            "the covered half is not darker: {} vs {}",
            px(&after, 5, 16),
            px(&after, 58, 16)
        );
        assert_eq!(
            px(&after, 58, 16),
            px(&before, 58, 16),
            "the uncovered half changed"
        );
    }

    /// ⚠⚠ **The acceptance for the whole milestone, end to end.** The region-scoped prefix and the
    /// whole-buffer-then-crop path must produce the same frame **byte for byte** — they are the
    /// same gather in the same order, so a tolerance would hide exactly what this exists to catch.
    ///
    /// ⚠ The two paths are forced apart by the CACHE, not by a flag: a warm final checkpoint means
    /// nothing is left to run, so the gate cannot fire and the region is taken by `img.crop`.
    #[test]
    fn a_region_scoped_render_matches_the_whole_buffer_path_byte_for_byte() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(24), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache with one step left must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        assert_eq!(
            via_crop.rgba, via_region.rgba,
            "the two paths must agree byte for byte"
        );
    }

    /// ⚠⚠ The gate itself. Every pixel test passes against a gate that never fires, because the
    /// two paths are bit-identical by design — this is what says it fired.
    #[test]
    fn a_drag_of_the_last_prefix_step_renders_only_the_region() {
        let buf = varied_sized(24);
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let spec = scoped_spec();

        let (_, cold) = render_with(&buf, &scoped_edits(), spec, &mut cache, &mut bc);
        assert_eq!(cold.prefix_stages, 1, "fixture check: one prefix step");

        // A Straighten drag: the same prefix, a different angle. Exactly the last step re-runs.
        let dragged = Edits {
            geometry: edit_model::Geometry {
                angle: 3.4,
                ..Default::default()
            },
            ..scoped_edits()
        };
        let (frame, ran) = render_with(&buf, &dragged, spec, &mut cache, &mut bc);
        assert!(ran.region_scoped, "the drag must render only the region");
        assert_eq!(ran.prefix_stages, 1);
        assert_eq!((frame.width, frame.height), (9, 7));
    }

    /// ⚠⚠ A region-scoped checkpoint served for a *different* region is the one failure that shows
    /// as a plausible photograph. Not inserting it is what makes that unrepresentable — and the way
    /// to see it is that a second identical render must scope **again** rather than find a final.
    #[test]
    fn a_region_scoped_render_leaves_no_final_checkpoint() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, first) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            first.region_scoped,
            "fixture check: the gate must have fired"
        );
        let (_, second) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            second.region_scoped,
            "no final checkpoint may have been stored, so the next render scopes again"
        );
        assert_eq!(second.prefix_stages, 1);
    }

    /// ⚠⚠ **THE CARRIER FOR M8.7, and without it the milestone ships dead.** A scoped render and a
    /// whole-buffer one produce the same picture **by design**, so every correctness test in the
    /// workspace — all 63 goldens included — passes with the gate deleted. That is precisely how
    /// creative LUTs (W5) shipped fully tested and 100% non-functional.
    ///
    /// ⚠ `prefix_stages` is asserted at **2**, not merely "scoped": M8.6's gate also sets
    /// `region_scoped`, so a test asking only that would pass against the single-step version this
    /// milestone replaces — which is the version that did nothing on the reporter's own chain.
    #[test]
    fn a_chain_of_prefix_steps_renders_only_the_region() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut cache = PrefixCache::default();
        let (frame, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            ran.region_scoped,
            "a region render whose whole chain accepts one must scope"
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "the scoped chain must cover BOTH steps, not just the last"
        );
        assert_eq!((frame.width, frame.height), (24, 18));

        // ⚠ And it must decline with no region at all — the fitted rung, where scoping would be
        // meaningless and where a gate that fired anyway would be producing a crop nobody asked
        // for.
        let mut cold = PrefixCache::default();
        let (whole, ran) = render_with(&buf, &e, FrameSpec::fit(128), &mut cold, &mut bc);
        assert!(!ran.region_scoped, "no region means no scoping");
        assert_eq!((whole.width, whole.height), (128, 96));
    }

    /// ⚠⚠ The engine-level acceptance for a **chain**: the scoped path and the whole-buffer path
    /// cropped must agree. The existing byte-for-byte test covers a one-step chain; only this one
    /// can see a margin that is right for a single step and wrong once composed — which is a wrong
    /// **band at one edge of the region**, a plausible photograph rather than an error.
    ///
    /// ⚠ **Within one 8-bit level, not bit-identical, and that is a property of the chain rather
    /// than a tolerance chosen for comfort.** Noise reduction's box blur keeps a running sum, so
    /// each position is reached by a different history of adds and subtracts and the last bits
    /// differ — `a_chain_with_a_local_agrees_to_within_a_rounding` measures that at one ulp. A
    /// chain of gathers alone *is* byte-identical, and `a_chain_of_gathers_is_byte_for_byte` is
    /// where that standard is held.
    ///
    /// ⚠ The two paths are forced apart by the CACHE rather than by a flag: a warm final
    /// checkpoint leaves nothing to run, so the gate cannot fire and the region is taken by
    /// `img.crop`.
    #[test]
    fn a_chained_region_render_agrees_with_the_whole_buffer_path() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(128), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        let worst = via_crop
            .rgba
            .iter()
            .zip(via_region.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "the two paths must agree to within a rounding, worst {worst} levels"
        );
    }

    /// ⚠⚠ **The caveat as a behaviour rather than as a sentence in a document.** A masked capture
    /// correction runs **last** in the prefix and declines a region by construction — it inherits
    /// `PipelineStage::input_rect_for`'s `None`. One decline abandons the whole plan, because a
    /// step that must run whole-buffer needs a whole-buffer input and therefore so does every step
    /// above it. So a photograph carrying a masked dehaze or a masked noise reduction gets no
    /// scoping at all, and that is recorded here so a later reader meets it as a decision.
    #[test]
    fn a_masked_capture_switches_scoping_off_for_that_photo() {
        let buf = varied_rect(128, 96);
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut plain = PrefixCache::default();
        let (_, before) = render_with(&buf, &chained_edits(), spec, &mut plain, &mut bc);
        assert!(
            before.region_scoped,
            "fixture check: without the mask this photo does scope"
        );

        let mut e = chained_edits();
        e.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 30.0 },
                ..Default::default()
            },
            disabled: false,
        });

        let mut cache = PrefixCache::default();
        let (_, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            !ran.region_scoped,
            "a masked capture correction declines a region, which abandons the whole plan"
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "fixture check: the mask really added a prefix step"
        );
    }

    fn byte_spec() -> FrameSpec {
        FrameSpec {
            gpu_tail: true,
            ..FrameSpec::fit(8)
        }
    }

    fn rendered(spec: FrameSpec) -> (RenderedFrame, Ran) {
        render_cached(
            &varied(),
            &hsl_edits(),
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
    }

    fn dehaze_edits() -> Edits {
        Edits {
            dehaze: edit_model::Dehaze { amount: 70.0 },
            ..Edits::default()
        }
    }

    fn rendered_hazy(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &dehaze_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE CARRIER, and without it every test in `gpu-pipeline` passes against a seam
    /// `render_cached` never calls.** The prefix seam changes no pixel a test can read — the two
    /// paths render the same picture by design — so only an explicit count can see it. That
    /// asymmetry is exactly how creative LUTs (W5) shipped fully tested and 100% dead.
    #[test]
    fn a_dehaze_prefix_step_runs_on_the_gpu() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered_hazy(true, &mut PrefixCache::default());
        assert_eq!(
            ran.prefix_stages, 1,
            "fixture check: there must be exactly one prefix step to accelerate"
        );
        assert_eq!(ran.gpu_prefix_steps, 1, "dehaze must have run on the GPU");
    }

    fn straightened(angle: f32, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        straightened_at(angle, 200, cache)
    }

    fn straightened_at(angle: f32, rung: usize, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        let mut e = Edits::default();
        e.geometry.angle = angle;
        render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(rung)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **The resident input (2026-09-26): a Straighten drag uploads once, not per frame.**
    /// Both halves: the second frame pays NO upload, and its picture is byte-identical to the
    /// same angle rendered through a fresh cache (a resident miss), so a hit cannot be serving a
    /// stale or wrong image.
    #[test]
    fn a_straighten_drag_uploads_its_input_once() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = straightened(2.0, &mut cache);
        assert_eq!(
            first.gpu_prefix_steps, 1,
            "fixture check: geometry ran on the GPU"
        );
        assert_eq!(first.gpu_prefix_uploads, 1, "the first frame uploads");
        let (second, ran) = straightened(3.0, &mut cache);
        assert_eq!(ran.gpu_prefix_steps, 1);
        assert_eq!(
            ran.gpu_prefix_uploads, 0,
            "a later frame of the drag must not upload"
        );
        let (fresh, _) = straightened(3.0, &mut PrefixCache::default());
        assert_eq!(
            second.rgba, fresh.rgba,
            "a resident hit must render what a miss renders"
        );
        // ⚠ A different rung is a different checkpoint: it must miss, never be served the 200 px
        // image the slot holds.
        let (other, ran) = straightened_at(3.0, 120, &mut cache);
        assert_eq!(
            ran.gpu_prefix_uploads, 1,
            "a rung change must upload its own input"
        );
        let (fresh, _) = straightened_at(3.0, 120, &mut PrefixCache::default());
        assert_eq!(other.rgba, fresh.rgba);
    }

    /// A Straighten frame whose tail is one GPU chain (`hsl_edits` clears the cost gate), which is
    /// what a handoff needs, rendered with the handoff on or off.
    fn handed(angle: f32, handoff: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        let mut e = hsl_edits();
        e.geometry.angle = angle;
        render_cached_with(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
            handoff,
        )
    }

    /// ⚠⚠ **The handoff (2026-09-26): the geometry's output reaches the tail without a download**,
    /// and changes no pixel. Byte-identical to the same frame rendered without it (both run the
    /// same GPU twin and the same tail chain on the same pixels), it reports the handoff, and it
    /// leaves NO final checkpoint — which is why the worker settles such frames.
    #[test]
    fn a_handed_off_frame_renders_the_same_bytes_and_caches_no_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut with = PrefixCache::default();
        let (a, ran) = handed(3.0, true, &mut with);
        assert!(
            ran.handoff,
            "fixture check: the handoff must have been taken"
        );
        assert_eq!(ran.gpu_prefix_steps, 1);
        assert_eq!(
            with.final_len(),
            0,
            "a handed-off frame caches no final checkpoint"
        );
        let mut without = PrefixCache::default();
        let (b, ran) = handed(3.0, false, &mut without);
        assert!(!ran.handoff);
        assert_eq!(
            without.final_len(),
            1,
            "fixture check: the ordinary path caches the final"
        );
        assert_eq!(a.rgba, b.rgba, "the handoff must not change a byte");
        assert_eq!(a.hist, b.hist);
    }

    /// ⚠ A handoff frame whose tail cannot run from the resident must fall back, not fail — and
    /// the fallback is the settle's own path: download the resident, cache it as the final, render
    /// the ordinary way. ⚠ Every tail stage has a GPU twin, so no real edit set refuses the
    /// resident; the test refuses it deliberately (`REFUSE_THE_RESIDENT`), which is the run-time
    /// failure being guarded.
    #[test]
    fn a_handoff_whose_tail_cannot_run_from_the_resident_downloads_it() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let mut e = hsl_edits();
        e.geometry.angle = 3.0;
        REFUSE_THE_RESIDENT.with(|l| l.set(true));
        let (frame, ran) = render_cached_with(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut BrushCache::default(),
            true,
        );
        REFUSE_THE_RESIDENT.with(|l| l.set(false));
        assert!(ran.handoff, "fixture check: the step handed off");
        assert!(!ran.resident_final_hit, "the tail was refused the resident");
        assert_eq!(
            cache.final_len(),
            1,
            "the fallback downloads the resident and caches it as the final"
        );
        // ⚠ Against the same edits through the ordinary path (a download, a cached final, the
        // tail uploading it) — the same GPU twins on the same pixels, so byte-identical. Not
        // against a CPU tail, which differs by the twins' one-level bound.
        let (want, _) = handed(3.0, false, &mut PrefixCache::default());
        assert_eq!(
            frame.rgba, want.rgba,
            "the fallback renders the ordinary picture"
        );
    }

    /// A tail-slider frame with a fixed prefix, at a rung, with `gpu_tail` on.
    fn tail_frame(
        exposure: f32,
        angle: f32,
        rung: usize,
        cache: &mut PrefixCache,
    ) -> (RenderedFrame, Ran) {
        let mut e = hsl_edits();
        e.exposure = exposure;
        e.geometry.angle = angle;
        render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(rung)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE claim (2026-09-26): a tail-slider frame after the first uploads NOTHING.** The first
    /// frame's tail uploads the final and keeps it; the second copies it GPU-side. Both halves:
    /// the counter, and a picture byte-identical to the same edits through a fresh cache.
    #[test]
    fn a_tail_slider_frame_uploads_once_and_then_hits_the_resident_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        // ⚠ No prefix step at all: the final IS checkpoint 0, the bare buffer.
        let (_, first) = tail_frame(0.3, 0.0, 200, &mut cache);
        assert_eq!(first.prefix_stages, 0, "fixture check: a bare prefix");
        assert_eq!(first.tail_uploads, 1, "the first tail frame uploads");
        assert!(!first.resident_final_hit);
        let (second, ran) = tail_frame(0.6, 0.0, 200, &mut cache);
        assert_eq!(ran.tail_uploads, 0, "the second must upload nothing");
        assert!(ran.resident_final_hit);
        let (fresh, _) = tail_frame(0.6, 0.0, 200, &mut PrefixCache::default());
        assert_eq!(second.rgba, fresh.rgba, "a hit renders what a miss renders");
        assert_eq!(second.hist, fresh.hist);
        // ⚠ A rung change is another checkpoint: it misses and uploads its own input.
        let (other, ran) = tail_frame(0.6, 0.0, 120, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a rung change uploads");
        let (fresh, _) = tail_frame(0.6, 0.0, 120, &mut PrefixCache::default());
        assert_eq!(other.rgba, fresh.rgba);
    }

    /// ⚠⚠ **A prefix edit between two tail frames re-fills the slot from the step's own output**,
    /// so the tail still uploads nothing — and the picture follows the edit. The sabotage that
    /// ignores the key serves the old geometry to the new tail and fails the byte comparison.
    #[test]
    fn a_prefix_edit_between_two_tail_frames_refills_the_resident_from_the_step() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, a) = tail_frame(0.3, 2.0, 200, &mut cache);
        assert_eq!(
            a.gpu_prefix_steps, 1,
            "fixture check: geometry ran on the GPU"
        );
        let (b_frame, b) = tail_frame(0.6, 2.0, 200, &mut cache);
        assert!(
            b.resident_final_hit,
            "fixture check: the tail hits after the step"
        );
        assert_eq!(b.tail_uploads, 0);
        let (c_frame, c) = tail_frame(0.6, 3.0, 200, &mut cache);
        assert_eq!(c.gpu_prefix_steps, 1, "the geometry re-ran");
        assert_eq!(
            c.tail_uploads, 0,
            "and its output fed the tail without an upload"
        );
        assert_ne!(
            c_frame.rgba, b_frame.rgba,
            "fixture check: the two angles render different pictures"
        );
        let (fresh, _) = tail_frame(0.6, 3.0, 200, &mut PrefixCache::default());
        assert_eq!(c_frame.rgba, fresh.rgba, "the picture follows the edit");
    }

    /// At 1:1 the tail's input is a crop, and the key carries the rect: two frames at one rect
    /// upload once; a pan uploads again, and renders the panned picture.
    #[test]
    fn at_one_to_one_the_resident_final_is_keyed_on_the_rect() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let at = |exposure: f32, rect: RegionRect, cache: &mut PrefixCache| {
            let mut e = hsl_edits();
            e.exposure = exposure;
            render_cached(
                &hazy_buffer(),
                &e,
                FrameSpec {
                    gpu_tail: true,
                    region: Some(rect),
                    ..FrameSpec::fit(200)
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                pipeline::TailRendering::default(),
                cache,
                &mut BrushCache::default(),
            )
        };
        let a = RegionRect {
            x: 10,
            y: 10,
            w: 60,
            h: 40,
        };
        let b = RegionRect {
            x: 70,
            y: 50,
            w: 60,
            h: 40,
        };
        let mut cache = PrefixCache::default();
        let (_, first) = at(0.3, a, &mut cache);
        assert_eq!(first.tail_uploads, 1);
        let (_, second) = at(0.6, a, &mut cache);
        assert_eq!(second.tail_uploads, 0, "the same rect hits");
        assert!(second.resident_final_hit);
        let (panned, ran) = at(0.6, b, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a pan is another crop and must upload");
        let (fresh, _) = at(0.6, b, &mut PrefixCache::default());
        assert_eq!(panned.rgba, fresh.rgba, "and renders the panned picture");
    }

    /// A handoff frame's resident, downloaded, IS the final checkpoint the ordinary path caches.
    #[test]
    fn a_handed_off_residents_download_equals_the_cached_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let ctx = gpu_pipeline::shared().expect("checked");
        let mut with = PrefixCache::default();
        let (_, ran) = handed(3.0, true, &mut with);
        assert!(ran.handoff, "fixture check");
        assert_eq!(
            with.final_len(),
            0,
            "fixture check: no CPU final after a handoff"
        );
        assert!(with.settle_from_resident(ctx), "the settle downloads it");
        let mut without = PrefixCache::default();
        let (_, ran) = handed(3.0, false, &mut without);
        assert!(!ran.handoff);
        let a = with.finals[0].1.clone();
        let b = without.finals[0].1.clone();
        assert_eq!(with.finals[0].0, without.finals[0].0, "the same key");
        assert_eq!(a.data(), b.data(), "the same checkpoint, sample for sample");
    }

    /// ⚠ The other direction, and it is not symmetry: without it a seam that fired
    /// **unconditionally** — ignoring the setting entirely — would pass the carrier above.
    #[test]
    fn with_the_setting_off_no_prefix_step_runs_on_the_gpu() {
        let (_, ran) = rendered_hazy(false, &mut PrefixCache::default());
        assert_eq!(ran.prefix_stages, 1, "fixture check: one prefix step");
        assert_eq!(ran.gpu_prefix_steps, 0);
    }

    /// ⚠⚠ **A checkpoint written by either path is a legitimate hit for the other**, which is what
    /// `PrefixStepKey` being derived from the **edits** buys — and it is a claim until something
    /// asserts it. Render once on the GPU, then once on the CPU **through the same cache**: the
    /// second must run no prefix step at all.
    #[test]
    fn a_checkpoint_written_by_either_path_is_a_hit_for_the_other() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_hazy(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_hazy(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    fn denoised_edits() -> Edits {
        Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        }
    }

    fn rendered_denoised(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &denoised_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **The same property for the SECOND stage across the seam, and it is a claim again.**
    /// `PrefixStepKey` is derived from the edits, never from the stage — so a checkpoint written
    /// by one path must be a legitimate hit for the other. The dangerous direction is a stale
    /// hit, which renders the previous frame's pixels: a plausible photograph, not an error.
    #[test]
    fn a_noise_reduction_checkpoint_is_shared_between_the_paths() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_denoised(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_denoised(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    /// ⚠⚠ **Without this, the three "keeps the float" tests below all pass against an
    /// implementation that never takes the byte path at all** — which is the failure mode that let
    /// creative LUTs ship fully tested and 100% dead.
    #[test]
    fn with_none_of_the_float_consumers_active_the_bytes_are_used() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered(byte_spec());
        assert!(ran.bytes, "the tail's last chain must download RGBA8");
    }

    /// ⚠ `pack` converts to a non-sRGB display space from the **float**, because an 8-bit → 8-bit
    /// conversion costs roughly 0.8 bits and bands a smooth sky.
    #[test]
    fn a_non_srgb_display_space_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            display_space: color::OutputSpace::DisplayP3,
            intent: color::RenderingIntent::default(),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The edge view is computed from the float.
    #[test]
    fn visualize_spots_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            visualize: Some(12.0),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The ProPhoto run is decoded back to the working space to test the gamut, which an 8-bit
    /// round trip cannot support.
    #[test]
    fn a_soft_proof_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            soft_proof: Some(Proof {
                space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                warn: false,
            }),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠⚠ **THE CARRIER.** Every test above can pass while the bytes never reach the texture. The
    /// two paths are designed to be indistinguishable in the output, so this asserts exactly that
    /// — and it is what would fail if the shader's quantization and `ImageBuf::quantize` ever
    /// diverged on a real frame rather than on the synthetic fixture in `gpu-pipeline`.
    #[test]
    fn the_byte_path_and_the_float_path_produce_the_same_frame() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (with_bytes, ran) = rendered(byte_spec());
        assert!(ran.bytes, "fixture check: this must be the byte path");
        let (with_float, ran) = rendered(FrameSpec {
            gpu_tail: false,
            ..byte_spec()
        });
        assert!(!ran.bytes, "fixture check: this must be the float path");
        assert_eq!(
            (with_bytes.width, with_bytes.height),
            (with_float.width, with_float.height)
        );
        // ⚠ The GPU's transcendentals diverge from the CPU's by well under an 8-bit level, so the
        // comparison is in levels rather than bit-exact — the bound `gpu-pipeline`'s own
        // equivalence tests establish. What it catches is a *wrong* quantization, which moves
        // whole levels.
        let worst = with_bytes
            .rgba
            .iter()
            .zip(with_float.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "texture differs by {worst} levels");
        assert!(
            Arc::ptr_eq(&with_bytes.rgba, &with_bytes.srgb),
            "on the byte path the texture and the photograph are one allocation"
        );
    }

    #[test]
    fn a_change_reruns_only_what_is_downstream_of_it() {
        // The feature itself. `every_cached_frame_matches_a_cold_render` proves the output is
        // right but would still pass with the cache deleted; this proves work is actually
        // being skipped, which is the entire point of the change.
        let buf = varied();
        let base = full_prefix();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, cold) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            cold.prefix_stages, 3,
            "a cold render runs every prefix stage"
        );

        let (_, hit) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(hit.prefix_stages, 0, "an unchanged prefix runs nothing");

        // A tail-only change still runs nothing: it resumes at the final checkpoint.
        let tail_only = Edits {
            exposure: -1.5,
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &tail_only,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "a tail change must not touch the prefix"
        );

        // ⚠ Since M7.3b the order is NR → dehaze → geometry. A dehaze change resumes after NR and
        // re-runs dehaze and the geometry resample behind it.
        let dehaze = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &dehaze,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "a dehaze change must resume after noise reduction"
        );

        // Noise reduction is first, so everything re-runs — but the downscale is still reused.
        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 15.0,
                color: 15.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &nr,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "noise reduction is the first prefix step since M7.3b"
        );

        // ⚠⚠ Geometry is LAST since M7.3b, so a straighten re-runs only the resample. This is the
        // point of the reorder: before it, this was 3 — NR and dehaze on every frame of a drag.
        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &geo,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 1,
            "a geometry change must resume after dehaze"
        );
    }

    #[test]
    fn the_native_seed_shares_the_buffer_rather_than_copying_it() {
        // When nothing is downscaled, `downscale_to` is a clone of a buffer the engine already
        // holds behind an `Arc`. Caching that would cost 78 MB on an X-T4 to save 2.2 ms, so
        // the chain seeds from the Arc instead and stores no checkpoint 0.
        let buf = varied(); // 16 x 16
        let base = full_prefix();
        let mut bc = BrushCache::default();

        let mut native = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut native,
            &mut bc,
        );
        assert_eq!(
            native.mid_len(),
            2,
            "native should store only the post-geometry and post-NR checkpoints"
        );

        let mut fit = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut fit,
            &mut bc,
        );
        assert_eq!(
            fit.mid_len(),
            3,
            "fit should additionally store the downscale"
        );
    }

    #[test]
    fn a_shrinking_prefix_does_not_serve_the_wrong_checkpoint() {
        // Turning the geometry off makes the post-dehaze checkpoint the *final* one. Its key is
        // the same value it had as a mid, so the tiers must not end up holding it twice with the
        // two copies free to diverge. ⚠ The geometry rather than dehaze since M7.3b, because the
        // geometry is now the prefix's last step.
        let buf = varied();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let with = full_prefix();
        render_cached(
            &buf,
            &with,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );

        let without = Edits {
            geometry: edit_model::Geometry::default(),
            ..with.clone()
        };
        let (got, ran) = render_cached(
            &buf,
            &without,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "the post-dehaze checkpoint is now the final one"
        );
        assert_eq!(
            got.rgba,
            render_once(
                &buf,
                &without,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .rgba
        );

        // ...and it must have been *promoted* into the protected tier, not left in the
        // budgeted one where a big render could evict the thing zooming back relies on.
        assert_eq!(cache.final_len(), 2);
        assert_eq!(cache.mid_len(), 2);
    }

    /// ⚠⚠ **The symptom it was found as**: at a native rung every frame of a prefix drag restarted
    /// from zero, re-running lens correction each time — measured in the running app at
    /// **1730–2223 ms a frame** with `prefix_ran 5` on every one.
    #[test]
    fn a_prefix_drag_resumes_when_the_budget_holds_one_checkpoint() {
        let buf = varied_sized(16);
        // 16*16*3*4 = 3072 bytes a checkpoint, so 4000 holds exactly one — 298 MiB against a
        // 512 MB budget, in miniature.
        let mut cache = PrefixCache::with_mid_budget(4000);
        let mut bc = BrushCache::default();
        let base = full_prefix();
        let (_, cold) = render_with(&buf, &base, FrameSpec::fit(16), &mut cache, &mut bc);
        assert_eq!(
            cold.prefix_stages, 3,
            "fixture check: a cold render runs the whole prefix"
        );

        // ⚠ Drag DEHAZE rather than noise reduction: NR is the FIRST prefix step since M7.3b, so
        // moving it invalidates every checkpoint and there is nothing to resume from whatever the
        // policy. The bug is about a drag that HAS valid upstream work.
        let dragged = Edits {
            dehaze: edit_model::Dehaze { amount: 41.0 },
            ..base.clone()
        };
        let (_, ran) = render_with(&buf, &dragged, FrameSpec::fit(16), &mut cache, &mut bc);
        assert!(
            ran.prefix_stages < 3,
            "the drag must resume at the checkpoint before dehaze rather than restart, ran {}",
            ran.prefix_stages
        );
    }

    /// The RGBA sub-rect of a frame, for comparing a region render against a whole one.
    fn crop_rgba(f: &RenderedFrame, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * f.width + x) * 4;
            out.extend_from_slice(&f.rgba[start..start + w * 4]);
        }
        out
    }

    #[test]
    fn a_region_render_matches_the_whole_render_cropped() {
        // The master invariant. It validates the region maths, the vignette correction and
        // the cache at once — if any of the three is wrong, this fails.
        //
        // ⚠ Sharpening and local contrast *weaken* it, and that has to be said out loud
        // rather than left to an edit set that happens to omit them. A spatial stage in the
        // tail reads neighbours the cropped region genuinely does not have, so equality can
        // only hold in the interior. The excluded band is `tail_kernel_radius` wide —
        // precisely what `view_region::REGION_MARGIN` keeps off screen — and both the inset
        // and the buffer size are derived from it, so they track any future change to a
        // radius or a sigma.
        let edits = Edits {
            exposure: 0.4,
            vignette: edit_model::Vignette {
                amount: -80.0,
                roundness: 50.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                color: 40.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 70.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            // ⚠ A mask joins the master invariant because that test validates the region
            // maths, the vignette correction *and* the prefix cache at once — so this is what
            // proves masks survive the cached render path, not just a bare stage call.
            masks: vec![
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                        from: [0.1, 0.1],
                        to: [0.9, 0.8],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.6,
                        // ⚠⚠ **Masked spatial belongs in this edit set, and its absence would be
                        // silent.** This test has already been insufficient in exactly that way
                        // once: when sharpening landed it kept passing because its edits carried
                        // none, so the weakened invariant it exists to cover went unexercised. A
                        // masked clarity reaches 120 px on its own, *on top of* the global reach,
                        // because masks chain — so if `tail_kernel_radius` ever stopped counting
                        // them the inset below would be too small and this comparison would fail
                        // at the edges, which is precisely the bug it should catch.
                        local_contrast: edit_model::LocalContrast {
                            texture: 0.0,
                            clarity: 70.0,
                        },
                        // ⚠⚠ **The capture corrections belong in this edit set, and their absence
                        // would be silent.** They run in the *prefix*, whole-buffer, and that is the
                        // entire reason for their placement: dehaze estimates its airlight from the
                        // brightest 0.1% of the buffer it is handed, so if it ever ran on the
                        // visible region instead, the same pixel would render differently depending
                        // on where the viewport happened to be — and this comparison is what catches
                        // it. There is no other test that can.
                        //
                        // ⚠ They contribute **nothing** to `inset` below, because neither is in the
                        // tail. If this ever fails at the edges, the placement is wrong — do not
                        // widen the inset.
                        noise_reduction: edit_model::NoiseReduction {
                            luminance: 40.0,
                            color: 30.0,
                            ..Default::default()
                        },
                        dehaze: edit_model::Dehaze { amount: 45.0 },
                        ..Default::default()
                    },
                    disabled: false,
                },
                // ⚠⚠ **AN AUTO-MASKED BRUSH BELONGS IN THIS EDIT SET, AND THIS IS THE GUARD ON THE
                // WHOLE AUTO-MASK DESIGN.** Its coverage depends on PIXEL VALUES as well as position,
                // which is what made a render-time reference untenable: a dab whose centre has been
                // panned off-region would have no reference to read, so its coverage — and therefore
                // the photograph — would depend on where the viewport happens to be. That is
                // `dehaze::airlight`'s failure arriving in a new place, and dehaze's escape (move to
                // the prefix) is not available, because brush coverage is resolved in the tail.
                //
                // A reference **baked into the stroke** travels in `Edits`, so it is available under
                // any region, in the CLI and in export alike. This comparison is what asserts that.
                // ⚠ Sabotage-verified: reading the reference from the base at the dab's own centre
                // fails this test.
                //
                // ⚠ It contributes **nothing** to `inset` below — a brush reads no neighbours, so it
                // adds no kernel reach. If this ever fails at the edges, something about the
                // reference's availability has changed; do not widen the inset.
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Brush(edit_model::Brush {
                        strokes: vec![edit_model::BrushStroke {
                            points: vec![[0.2, 0.3], [0.8, 0.7]],
                            radius: 0.2,
                            hardness: 60.0,
                            flow: 100.0,
                            erase: false,
                            auto_mask: true,
                            tolerance: 45.0,
                        }],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                    disabled: false,
                },
            ],
            ..Edits::default()
        };

        let inset = pipeline::tail_kernel_radius(&edits, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 2 * inset + 40, 2 * inset + 36);
        let n = 2 * inset + 80;
        let buf = varied_sized(n);

        // max_dim = n is native for this n x n buffer, i.e. no downscale, so the render
        // scale is exactly 1.0 and the inset above is the full-resolution kernel.
        let (whole, _) = render_cached(
            &buf,
            &edits,
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        // Guard against the test passing trivially: the spatial tail stages must actually
        // move pixels here, or the "weakened invariant" it exists to cover is not exercised.
        let (unsharpened, _) = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                local_contrast: edit_model::LocalContrast::default(),
                ..edits.clone()
            },
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_ne!(
            whole.rgba, unsharpened.rgba,
            "the spatial tail stages changed nothing"
        );

        assert!(
            x + w <= whole.width && y + h <= whole.height,
            "region must fit"
        );
        assert!(
            w > 2 * inset && h > 2 * inset,
            "region must be wider than twice the kernel ({inset})"
        );

        let (got, _) = render_cached(
            &buf,
            &edits,
            FrameSpec {
                max_dim: n,
                region: Some(RegionRect { x, y, w, h }),
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        let (iw, ih) = (w - 2 * inset, h - 2 * inset);
        assert_eq!(
            crop_rgba(&got, inset, inset, iw, ih),
            crop_rgba(&whole, x + inset, y + inset, iw, ih),
            "the region's interior must match the whole render bit for bit"
        );
        assert_eq!(
            got.region,
            Some(pipeline::Region {
                full_w: whole.width,
                full_h: whole.height,
                x0: x,
                y0: y
            })
        );
    }

    #[test]
    fn the_cache_keeps_one_entry_per_render_mode() {
        // Fit and native are different max_dims. With a single entry, every zoom in or out
        // would recompute the prefix (~123 ms with NR on).
        let buf = varied();
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        render_cached(
            &buf,
            &e,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        ); // "fit"
        render_cached(
            &buf,
            &e,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        ); // "native"
        assert_eq!(cache.final_len(), 2, "both modes should stay cached");

        for max_dim in [8usize, 16] {
            let scale = render_scale(16, rendered_long_edge(16, max_dim), 1.0);
            assert!(
                cache
                    .get(&(
                        PrefixKey::new(&e, scale, pipeline::Resampler::Bilinear),
                        max_dim
                    ))
                    .is_some(),
                "max_dim {max_dim} was evicted"
            );
        }

        // A third distinct key evicts the least recently used.
        let e2 = Edits {
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..e.clone()
        };
        render_cached(
            &buf,
            &e2,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(cache.final_len(), 2);
    }

    #[test]
    fn a_region_bigger_than_the_image_is_clamped() {
        let buf = varied();
        let e = Edits::default();
        let (got, _) = render_cached(
            &buf,
            &e,
            FrameSpec {
                max_dim: 16,
                region: Some(RegionRect {
                    x: 10,
                    y: 10,
                    w: 999,
                    h: 999,
                }),
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!((got.width, got.height), (6, 6));
    }

    #[test]
    fn render_once_matches_expected_gray() {
        let img = tiny();
        let f = render_once(
            &img,
            &Edits::default(),
            4,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        assert_eq!((f.width, f.height), (4, 4));
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
    }

    // ---- the grade range view ------------------------------------------------------------
    /// Six greys spanning black to above white, a saturated blue and a red — 8×1.
    fn ramp_fixture() -> Arc<ImageBuf> {
        let px = [
            [0.005f32, 0.005, 0.005],
            [0.05, 0.05, 0.05],
            [0.18, 0.18, 0.18],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [1.4, 1.4, 1.4],
            [0.01, 0.025, 0.45],
            [0.5, 0.1, 0.1],
        ];
        let mut buf = ImageBuf::new(px.len(), 1);
        for (i, p) in px.iter().enumerate() {
            buf.set_pixel(i, 0, *p);
        }
        Arc::new(buf)
    }

    /// A grade that is active (saturation on all three wheels), with `luminance` on every wheel
    /// set to `lum` — the offset that moves L *after* the tap.
    fn graded(lum: f32, rendering: Option<edit_model::Rendering>) -> Edits {
        let wheel = |hue: f32| edit_model::GradeWheel {
            hue,
            saturation: 70.0,
            luminance: lum,
        };
        Edits {
            color_grading: edit_model::ColorGrading {
                shadows: wheel(240.0),
                midtones: wheel(120.0),
                highlights: wheel(45.0),
                ..edit_model::ColorGrading::default()
            },
            rendering,
            ..Default::default()
        }
    }

    /// Render through the cached path at the fixture's own resolution, with the view.
    fn viewed(
        buf: &Arc<ImageBuf>,
        edits: &Edits,
        grade_view: Option<GradeRangeView>,
        region: Option<RegionRect>,
    ) -> RenderedFrame {
        render_cached(
            buf,
            edits,
            FrameSpec {
                max_dim: buf.width().max(buf.height()),
                region,
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    fn plane(f: &RenderedFrame) -> Vec<u8> {
        f.grade_view
            .as_ref()
            .map(|p| p.as_ref().clone())
            .expect("the frame carries no plane")
    }

    /// ⚠⚠ The view is an overlay, never a change to the photograph: pixels, sRGB channel and
    /// histogram are byte-identical with the view on and off. Sabotage: painting the tint into
    /// `out` fails it. And `None` is the old path — no plane at all.
    #[test]
    fn the_grade_view_does_not_touch_the_photograph() {
        let buf = ramp_fixture();
        let edits = graded(0.0, None);
        let off = viewed(&buf, &edits, None, None);
        let on = viewed(&buf, &edits, Some(GradeRangeView::Shadows), None);
        assert!(
            off.grade_view.is_none(),
            "the view-off frame carries a plane"
        );
        assert_eq!(on.rgba, off.rgba, "the view changed the texture");
        assert_eq!(on.srgb, off.srgb, "the view changed the instrument channel");
        assert_eq!(on.hist, off.hist, "the view changed the histogram");
        let p = plane(&on);
        assert_eq!(p.len(), on.width * on.height);
        // The ramp's ends: the darkest grey is mostly shadow, the brightest barely — at the
        // fixture's default Blending of 50 the ranges overlap widely, so "mostly" is the honest
        // reading (143 of 255 at L = 0.17), not the near-255 a Blending of 0 would give.
        assert!(
            p[0] > 128 && p[5] < 16,
            "the plane is not the shadow weight: {p:?}"
        );
    }

    /// ⚠⚠ The plane is taken BEFORE the grade. A luminance wheel at +100 shifts L by 0.15 on
    /// every pixel it weights; tapped after the stage, the plane would move. Sabotage: run
    /// `from_grade` before taking the plane.
    #[test]
    fn the_plane_is_taken_before_the_grade() {
        let buf = ramp_fixture();
        for range in GradeRangeView::ALL {
            let flat = plane(&viewed(&buf, &graded(0.0, None), Some(range), None));
            let lifted = plane(&viewed(&buf, &graded(100.0, None), Some(range), None));
            assert_eq!(
                flat, lifted,
                "{range:?}: the grade's own luminance moved the plane"
            );
        }
        // The premise: the lift really changes the photograph, so the equality above is not
        // vacuous.
        let a = viewed(&buf, &graded(0.0, None), None, None);
        let b = viewed(&buf, &graded(100.0, None), None, None);
        assert_ne!(a.srgb, b.srgb, "premise: +100 luminance changed nothing");
    }

    /// ⚠⚠ The plane is taken BEFORE the display rendering — the test that encodes spec §1 and
    /// would have failed the pack-side design outright. Sabotage: tap after
    /// `DisplayRenderingStage` (i.e. from `pack`'s output).
    #[test]
    fn the_plane_is_taken_before_the_display_rendering() {
        let buf = ramp_fixture();
        let colorimetric = graded(0.0, None);
        let rendered = graded(0.0, Some(edit_model::Rendering::Camera));
        for range in GradeRangeView::ALL {
            let a = plane(&viewed(&buf, &colorimetric, Some(range), None));
            let b = plane(&viewed(&buf, &rendered, Some(range), None));
            assert_eq!(a, b, "{range:?}: the S-curve moved the plane");
        }
        let a = viewed(&buf, &colorimetric, None, None);
        let b = viewed(&buf, &rendered, None, None);
        assert_ne!(a.srgb, b.srgb, "premise: the rendering changed nothing");
    }

    /// On a region render the plane covers the rendered slice — `w * h` of the frame, not of
    /// the whole image. `ColorImage::from_rgba_unmultiplied` panics on a mismatch rather than
    /// reporting one, so this is pinned here.
    #[test]
    fn the_plane_has_the_frames_dimensions() {
        let mut buf = ImageBuf::new(8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let v = (x + y * 8) as f32 / 48.0;
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(buf);
        let region = Some(RegionRect {
            x: 2,
            y: 1,
            w: 4,
            h: 3,
        });
        let f = viewed(
            &buf,
            &graded(0.0, None),
            Some(GradeRangeView::Midtones),
            region,
        );
        assert_eq!((f.width, f.height), (4, 3));
        assert_eq!(plane(&f).len(), 12);
    }
}

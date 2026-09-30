//! The prefix walk: plan the steps, resume from the deepest checkpoint still held, and run the
//! rest over the whole buffer, caching each checkpoint on the way.

use super::frame::FrameSpec;
use super::RegionRect;
use crate::prefix_cache::{CacheKey, PrefixCache, Tier};
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{build_prefix_steps, PipelineStage, PrefixKey, RenderScale};
use std::sync::Arc;

/// What every phase of one render's prefix walk reads: the steps, the rung they render at, and the
/// facts `Edits` cannot carry.
pub(super) struct Walk<'a> {
    pub(super) steps: Vec<pipeline::PrefixStep>,
    pub(super) scale: RenderScale,
    pub(super) max_dim: usize,
    pub(super) airlight: [f32; 3],
    pub(super) debug_steps: bool,
    pub(super) edits: &'a Edits,
    pub(super) lens: Option<lens_profile::LensShot>,
    pub(super) resampler: pipeline::Resampler,
    pub(super) gpu_tail: bool,
}

impl Walk<'_> {
    /// The cache key of checkpoint `n`: the output of the first `n` steps, at this rung.
    pub(super) fn key_at(&self, n: usize) -> CacheKey {
        (PrefixKey::upto(&self.steps, n, self.scale), self.max_dim)
    }

    /// The tier checkpoint `n` is kept in — the complete prefix output is the protected one.
    pub(super) fn tier_at(&self, n: usize) -> Tier {
        if n == self.steps.len() {
            Tier::Final
        } else {
            Tier::Mid
        }
    }
}

/// What the prefix hands the tail: its input (`None` after a handoff), the region it covers, the
/// frame's dimensions, and the counters the render reports.
pub(super) struct PrefixOut {
    pub(super) input: Option<Arc<ImageBuf>>,
    pub(super) rect: Option<RegionRect>,
    pub(super) pw: usize,
    pub(super) ph: usize,
    pub(super) prefix_stages: usize,
    /// The last whole-buffer checkpoint — a handoff's last-resort CPU re-run starts from it.
    pub(super) img: Arc<ImageBuf>,
    pub(super) gpu_prefix_steps: usize,
    pub(super) gpu_prefix_uploads: usize,
    pub(super) handed: Option<(usize, usize)>,
}

/// Plan one render's prefix: the frame-global airlight, then the steps.
pub(super) fn plan<'a>(
    edits: &'a Edits,
    buffer: &Arc<ImageBuf>,
    lens: Option<lens_profile::LensShot>,
    scale: RenderScale,
    spec: &FrameSpec,
    cache: &mut PrefixCache,
) -> Walk<'a> {
    let FrameSpec {
        max_dim,
        resampler,
        gpu_tail,
        ..
    } = *spec;
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
    Walk {
        steps,
        scale,
        max_dim,
        airlight,
        debug_steps,
        edits,
        lens,
        resampler,
        gpu_tail,
    }
}

/// The deepest checkpoint still held, and where the walk resumes from it.
pub(super) fn resume(
    walk: &Walk,
    buffer: &Arc<ImageBuf>,
    buffer_long: usize,
    cache: &mut PrefixCache,
) -> (usize, Arc<ImageBuf>) {
    let Walk {
        ref steps, max_dim, ..
    } = *walk;
    // Resume from the deepest checkpoint still held. `n == 0` is the bare downscale.
    let mut resumed = None;
    for n in (0..=steps.len()).rev() {
        if let Some(img) = cache.get(&walk.key_at(n)) {
            resumed = Some((n, img));
            break;
        }
    }

    let (start, img) = match resumed {
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
                cache.insert(walk.key_at(0), s.clone(), walk.tier_at(0));
                s
            };
            (0, seed)
        }
    };
    (start, img)
}

/// Run every step from `start` over the whole buffer, caching each checkpoint. The last step
/// may keep its output on the GPU instead — downloaded as well, or, when `handoff_ok`, not.
pub(super) fn walk_whole(
    walk: &Walk,
    handoff_ok: bool,
    region: Option<RegionRect>,
    start: usize,
    mut img: Arc<ImageBuf>,
    cache: &mut PrefixCache,
) -> PrefixOut {
    let Walk {
        ref steps,
        scale,
        airlight,
        debug_steps,
        edits,
        ref lens,
        resampler,
        gpu_tail,
        ..
    } = *walk;
    let mut gpu_prefix_steps = 0usize;
    let mut gpu_prefix_uploads = 0usize;
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
                    ctx.and_then(|ctx| cache.resident(&walk.key_at(i), &img, ctx))
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
            cache.keep_resident_final((walk.key_at(i + 1), None), kept);
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
                    cache.insert(walk.key_at(i + 1), img.clone(), walk.tier_at(i + 1));
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
                    ctx.and_then(|ctx| cache.resident(&walk.key_at(i), &img, ctx))
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
        cache.insert(walk.key_at(i + 1), img.clone(), walk.tier_at(i + 1));
    }
    let ran = steps.len() - start;

    // ⚠ The complete prefix output must always live in the protected tier, even when we
    // resumed at it without running anything. A prefix that *shrinks* — its last stage going
    // neutral — turns a former intermediate into the final, and leaving it in the budgeted
    // tier would let a large render evict the very thing the zoom guarantee rests on.
    // `steps.is_empty()` is excluded because there the final *is* checkpoint 0, which is
    // either free (and must not be stored) or was already inserted as a final above.
    if ran == 0 && !steps.is_empty() {
        cache.insert(walk.key_at(steps.len()), img.clone(), Tier::Final);
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
    PrefixOut {
        input,
        rect,
        pw,
        ph,
        prefix_stages: ran,
        img,
        gpu_prefix_steps,
        gpu_prefix_uploads,
        handed,
    }
}

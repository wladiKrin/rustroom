//! The tail: what it is built from, where its input lives — the CPU checkpoint or the GPU's
//! resident final — and the run that ends in packed bytes or a float frame.

use super::pack::Packed;
use super::prefix::Walk;
use crate::prefix_cache::{PrefixCache, TailKey, Tier};
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{Region, RenderScale};
use std::sync::Arc;

/// An owned copy of a shared checkpoint: the buffer itself when nothing else holds it, a clone
/// otherwise. The tail's input travels as an `Arc` so the one-chain paths never clone it.
fn unshare(a: Arc<ImageBuf>) -> ImageBuf {
    Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())
}

/// Everything the tail is built from, fixed for one render.
#[derive(Clone, Copy)]
pub(super) struct TailSpec<'a> {
    pub(super) edits: &'a Edits,
    pub(super) scale: RenderScale,
    pub(super) region: Option<Region>,
    pub(super) coverage: &'a Arc<pipeline::BrushCoverage>,
    pub(super) intent: color::RenderingIntent,
    pub(super) resampler: pipeline::Resampler,
    pub(super) airlight: [f32; 3],
    pub(super) wb_baseline: color::wb::Illuminant,
    pub(super) rendering: pipeline::TailRendering<'a>,
    pub(super) gpu_tail: bool,
}

impl TailSpec<'_> {
    /// The tail's pipeline, and its GPU twin when the setting is on and an adapter exists.
    pub(super) fn build(
        &self,
        space: color::OutputSpace,
    ) -> (pipeline::Pipeline, Option<gpu_pipeline::GpuChain>) {
        let TailSpec {
            edits,
            scale,
            region,
            coverage,
            intent,
            resampler,
            airlight,
            wb_baseline,
            rendering,
            gpu_tail,
        } = *self;
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
            Some(coverage),
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
    }
}

/// What a tail run produces: the frame and, in the grade range view, its coverage plane.
pub(super) type TailOut = (Packed, Option<Arc<Vec<u8>>>);

/// What the tail phases share for one render.
#[derive(Clone, Copy)]
pub(super) struct TailRun<'a> {
    pub(super) tail: TailSpec<'a>,
    pub(super) walk: &'a Walk<'a>,
    pub(super) tail_key: &'a TailKey,
    pub(super) refused: bool,
}

/// What the tail phases report.
pub(super) struct TailStats {
    pub(super) tail_uploads: usize,
    pub(super) resident_final_hit: bool,
}

/// The tail's input. A frame whose prefix handed off has no CPU image: the resident final serves
/// the tail directly — then the packed frame comes back too — or is downloaded and cached.
pub(super) fn resolve_input(
    t: TailRun,
    input: Option<Arc<ImageBuf>>,
    pw: usize,
    ph: usize,
    // The last whole-buffer checkpoint and the step it is the input of (M10.14f: outputs carried on the GPU leave it
    // behind the last step).
    (img, img_step): (&ImageBuf, usize),
    cache: &mut PrefixCache,
) -> (Arc<ImageBuf>, Option<Packed>, TailStats) {
    let TailRun {
        tail,
        walk,
        tail_key,
        refused,
    } = t;
    let Walk {
        ref steps,
        debug_steps,
        ..
    } = *walk;
    let mut tail_uploads = 0usize;
    let mut resident_final_hit = false;
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
                    let (pipe, chain) = tail.build(color::OutputSpace::Srgb);
                    let chain = chain.filter(|_| pipe.stage_names().is_empty())?;
                    let r = cache.resident_final(tail_key)?;
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
                        .get(&walk.key_at(steps.len()))
                        .expect("settle_from_resident inserted the final"),
                    // ⚠ The last resort — the GPU refused even a download. The steps re-run on
                    // the CPU from the checkpoint `img` is — the one before the last, or earlier
                    // when outputs were carried on the GPU (M10.14f).
                    _ => {
                        assert!(img_step < steps.len(), "a handoff implies a prefix step");
                        let out = Arc::new(super::prefix::catch_up(
                            steps,
                            img_step,
                            steps.len(),
                            img.clone(),
                        ));
                        cache.insert(walk.key_at(steps.len()), out.clone(), Tier::Final);
                        out
                    }
                },
            }
        }
    };
    (
        input,
        from_resident,
        TailStats {
            tail_uploads,
            resident_final_hit,
        },
    )
}

/// Run the tail: the resident final's packed frame when the input phase already has it, the grade
/// range view's split when it is on, else the tail built for sRGB.
pub(super) fn run(
    t: TailRun,
    from_resident: Option<Packed>,
    input: Arc<ImageBuf>,
    grade_view: Option<crate::grade_view::GradeRangeView>,
    float_needed: bool,
    cache: &mut PrefixCache,
) -> (Packed, Option<Arc<Vec<u8>>>, TailStats) {
    let TailRun {
        tail,
        walk,
        tail_key,
        refused,
    } = t;
    let Walk { debug_steps, .. } = *walk;
    let mut tail_uploads = 0usize;
    let mut resident_final_hit = false;
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
            Some(range) => grade_split(tail, range, input),
            None => {
                let t_build = std::time::Instant::now();
                let (pipe, chain) = tail.build(color::OutputSpace::Srgb);
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
                        let (packed, s) = byte_tail(t, c, pipe, input, cache);
                        tail_uploads += s.tail_uploads;
                        resident_final_hit |= s.resident_final_hit;
                        packed
                    }
                    // ⚠ The float path keeps the tail's input too, when the tail is one chain: a
                    // non-sRGB display, Visualize Spots, or the proof's sRGB run.
                    Some(c) if pipe.stage_names().is_empty() => {
                        let hit = match cache.resident_final(tail_key).filter(|_| !refused) {
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
    (
        out,
        plane,
        TailStats {
            tail_uploads,
            resident_final_hit,
        },
    )
}

/// The grade range view's tail: the same stages in the same order, read at the grading stage's
/// input between two halves.
fn grade_split(
    tail: TailSpec,
    range: crate::grade_view::GradeRangeView,
    input: Arc<ImageBuf>,
) -> TailOut {
    let TailSpec {
        edits,
        scale,
        region,
        coverage,
        intent,
        resampler,
        airlight,
        wb_baseline,
        rendering,
        ..
    } = tail;
    let (_, halves) = pipeline::build_pipeline_split_tapped(
        edits,
        scale,
        region,
        Some(coverage),
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

/// The byte path: the tail's last chain quantizes on the GPU and the frame downloads as RGBA8.
fn byte_tail(
    t: TailRun,
    c: gpu_pipeline::GpuChain,
    pipe: pipeline::Pipeline,
    input: Arc<ImageBuf>,
    cache: &mut PrefixCache,
) -> (TailOut, TailStats) {
    let TailRun {
        walk,
        tail_key,
        refused,
        ..
    } = t;
    let Walk { debug_steps, .. } = *walk;
    let mut tail_uploads = 0usize;
    let mut resident_final_hit = false;
    let packed = {
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
            (Some(s), _) => match cache.resident_final(tail_key).filter(|_| !refused) {
                Some(r) => {
                    resident_final_hit = true;
                    c.run_rgba8_from(r)
                }
                None => c.run_rgba8_keeping_input(s).map(|(b, kept)| {
                    cache.keep_resident_final(tail_key.clone(), kept);
                    b
                }),
            },
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
    };
    (
        packed,
        TailStats {
            tail_uploads,
            resident_final_hit,
        },
    )
}

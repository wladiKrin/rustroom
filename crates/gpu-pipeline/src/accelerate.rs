//! Swapping runs of CPU stages for GPU chains in a built pipeline.

use crate::{
    GpuChain, GpuContext, GpuContrastStage, GpuDisplayRenderingStage, GpuExposureStage,
    GpuGradingStage, GpuHslStage, GpuOutputTransformStage, GpuSaturationStage, GpuSharpenStage,
    GpuStage, GpuToneCurveStage, GpuToneRegionsStage, GpuWhiteBalanceStage,
};
use edit_model::Edits;
use pipeline::{Pipeline, PipelineStage, ProfileRendering, RenderScale, RenderSpec};
use std::sync::Arc;

/// Builds one GPU stage from what the tail was built from.
///
/// ⚠ It takes the [`RenderScale`] as well as the [`Edits`], because `SharpenStage`'s sigma is in
/// **full-resolution pixels** and only the scale says what that is at this rung. `GpuHslStage`
/// ignores it; a signature that omitted it would make the sharpen entry unwritable.
///
/// ⚠⚠ **And the [`ProfileRendering`], because `display_rendering` is NOT built from `Edits`.** Its
/// curve and baseline exposure come from the camera profile, which `pipeline` never loads — they
/// are handed to `build_pipeline_split_in_region_with_brush` by the caller. A builder without it
/// would silently give every photograph the built-in curve and the built-in baseline while a
/// profile declaring its own sat unused, which is a plausible picture rather than an error.
///
/// ⚠⚠ **And the [`RenderSpec`], because `output_transform` is not built from `Edits` either** —
/// its output space is a property of the *render*, chosen per call: sRGB for the preview,
/// ProPhoto for a soft proof. ⚠ The whole spec rather than just the space, because it also
/// carries the white-balance `baseline` that stage will need — one signature change instead of
/// two.
type Builder = fn(
    &Edits,
    RenderScale,
    ProfileRendering<'_>,
    RenderSpec,
    &Arc<GpuContext>,
) -> Box<dyn GpuStage>;

/// The stages this can replace, by the name `PipelineStage::name` reports.
///
/// ⚠⚠ **Still name-matching, and still stringly-typed.** M8.3b recorded that this does not scale
/// to a dozen stages and that the right answer then is a `StageAccelerator` hook `pipeline` calls
/// at construction. A table is the smaller step that stops an if-chain growing; it is **not** the
/// fix. ⚠ The hook still loses for the reason it did in M8.3b: `RenderSpec` is `Copy + PartialEq`
/// and an `Arc<dyn Trait>` is neither.
///
/// ⚠ Every name here must occur **at most once** in a tail, which is what makes the lookup sound.
/// A mask's own stages live inside `MaskStage`, which the tail sees as one opaque stage — asserted
/// by `the_replaceable_names_occur_at_most_once_in_a_tail`.
/// ⚠ Kept in **tail order**. The order does not affect the lookup, but a table that reads in
/// pipeline order is one a reader can check against `build_pipeline_split_in_region` by eye.
const REPLACEABLE: &[(&str, Builder)] = &[
    // ⚠⚠ First, as in the tail — and the only row whose parameters come from the `RenderSpec`
    // rather than from `Edits` alone. `white_balance_gains` is shared with `adjustment_stages`,
    // so the GPU and CPU stages cannot disagree about what "as shot" means.
    ("white_balance", |e, _, _, spec, ctx| {
        Box::new(GpuWhiteBalanceStage::new(
            pipeline::white_balance_gains(&e.white_balance, spec.baseline),
            ctx,
        ))
    }),
    ("exposure", |e, _, _, _, ctx| {
        Box::new(GpuExposureStage::new(e.exposure, ctx.clone()))
    }),
    // ⚠⚠ Between `exposure` and `tone_regions`, which is the whole reason 1.2 ms is worth
    // porting: on the CPU it split the per-pixel head into two runs the gate refused.
    ("contrast", |e, _, _, _, ctx| {
        Box::new(GpuContrastStage::new(e.contrast, ctx))
    }),
    ("tone_regions", |e, _, _, _, ctx| {
        Box::new(GpuToneRegionsStage::new(e, ctx))
    }),
    // ⚠ After `tone_regions`, as in the tail. Like `contrast`, it sits between ported stages and
    // split the per-pixel head before this milestone.
    ("tone_curve", |e, _, _, _, ctx| {
        Box::new(GpuToneCurveStage::new(&e.tone_curve.points, ctx))
    }),
    // ⚠ Before `hsl`, as in the tail — colour follows tone.
    ("saturation", |e, _, _, _, ctx| {
        Box::new(GpuSaturationStage::new(e.saturation, e.vibrance, ctx))
    }),
    ("hsl", |e, _, _, _, ctx| {
        Box::new(GpuHslStage::new(&e.hsl, ctx))
    }),
    ("color_grading", |e, _, _, _, ctx| {
        Box::new(GpuGradingStage::new(&e.color_grading, ctx))
    }),
    ("sharpening", |e, scale, _, _, ctx| {
        Box::new(GpuSharpenStage::new(&e.sharpening, scale, ctx))
    }),
    // ⚠⚠ The only row built from something other than `Edits`. ⚠ `effective_profile_curve` is
    // what the CPU tail applies before constructing this stage — skipping it would render every
    // camera whose profile declares an identity curve differently from the CPU, which is most of
    // them, the X-T4 included.
    ("display_rendering", |_, _, profile, _, ctx| {
        Box::new(GpuDisplayRenderingStage::new(
            pipeline::effective_profile_curve(profile.curve),
            profile.baseline_ev,
            ctx,
        ))
    }),
    // ⚠ Last, as in the tail — and the only stage here that is on **every** photograph, which is
    // what the cost gate exists to keep honest.
    ("output_transform", |_, _, _, spec, ctx| {
        Box::new(GpuOutputTransformStage::new(spec.space, ctx))
    }),
];

/// What one round trip costs, in the same units as [`GpuStage::cpu_cost`] — milliseconds per 6.8
/// megapixels, measured by `gpu-pipeline`'s `probe_rt` on an Apple M2.
///
/// ⚠⚠ **THE MEASUREMENT, AND WHY IT MOVED (M8.3h).** This was **34.0** from M8.3e until M8.3h,
/// and two later milestones made the round trip cheaper *without re-measuring it*: **M8.3f**'s
/// resource cache recovered 9.46 ms per render, and **M8.3g**'s 8-bit readback took the download
/// from 13.20 ms to 4.78. For two milestones the gate was refusing runs that would have paid,
/// using a number that had stopped describing reality.
///
/// ⚠⚠ **QUOTED AS A RANGE, BECAUSE ONE RUN IS NOT A MEASUREMENT HERE.** Seven runs of `probe_rt`
/// at 3200x2136 on a quiet machine give a float round trip of **23.0-26.8 ms** and a byte one of
/// **15.7-21.0**. The first reading taken was 26.80 — the *top* of its band — and quoting it
/// alone would have stated a number that does not reproduce. **27.0 sits at or just above the
/// top of the float band**, which is what keeps it conservative.
///
/// ⚠⚠ **THIS IS THE FLOAT ROUND TRIP, DELIBERATELY, THOUGH THE CASE IT UNLOCKS TAKES THE BYTE
/// PATH.** The byte figure is *not* used, because a lone `display_rendering` run (25.7 ms) can
/// occur — `CreativeLookStage` sits between it and `output_transform`, so a creative look makes
/// it a run of its own — and **that run is not trailing, so it can never take the byte path**.
/// ⚠ Stated honestly: 25.7 sits **inside** the float band, so accelerating it is not a clear
/// loss, merely not a clear win — and the gate's own property, *fail toward the CPU, because a
/// wrong number costs milliseconds and never pixels*, is what settles it. A byte-valued
/// threshold would accept it on the strength of a saving it cannot have. One conservative
/// constant refuses it by construction, with no second rule to keep in step. Revisit when a
/// ported stage's cost lands clearly between the two bands.
///
/// ⚠⚠ **STANDING RULE: anything that changes the upload, the dispatch count or the download must
/// re-run `probe_rt` and update this.** That is how it went stale, and it is recorded as a rule
/// rather than a test on purpose — a timing assertion cannot fail a build honestly, and this
/// project already records that a benchmark is not a test.
///
/// ⚠ **The comparison is scale-free, which is why this is one number and not one per rung.** Both
/// sides are linear in pixel count, and both paths measure it: at 1600x1068 the float trip is
/// ~7.2-7.3 ms and the byte one ~5.1-5.2, against a 4.0x pixel ratio.
const TRANSFER_COST: f32 = 27.0;

/// Project a mask's [`edit_model::LocalAdjustments`] onto the [`Edits`] the [`REPLACEABLE`]
/// builders read.
///
/// ⚠⚠ **Written as an exhaustive destructuring on purpose: adding a field to `LocalAdjustments`
/// is a COMPILE ERROR here.** The alternative — `a.exposure`, `a.hsl`, … field by field — leaves a
/// new adjustment silently rendering as its default inside every mask, which is a plausible
/// photograph rather than an error. That is `MaskStage::source_for`'s rule (*"exhaustive on
/// purpose — a catch-all here is a trap"*) applied to the projection.
///
/// ⚠ The eight carried fields are exactly the ones a `REPLACEABLE` builder reads. The eight
/// ignored ones have **no GPU stage**: white balance, contrast, the tone curve, local contrast and
/// saturation are M8.3e plan 2's; noise reduction and dehaze run in the **prefix** and are never in
/// a mask's tail sub-stage list at all.
///
/// ⚠ `display_rendering` and `output_transform` are the two builders not built from `Edits`, and
/// neither can occur in a mask's sub-stage list — `adjustment_stages` does not place them.
fn sub_edits(a: &edit_model::LocalAdjustments) -> Edits {
    let edit_model::LocalAdjustments {
        // carried — a GPU builder reads each of these
        exposure,
        highlights,
        shadows,
        whites,
        blacks,
        hsl,
        color_grading,
        sharpening,
        // deliberately not carried — no GPU stage exists for them
        noise_reduction: _,
        dehaze: _,
        white_balance: _,
        contrast: _,
        tone_curve: _,
        saturation: _,
        vibrance: _,
        local_contrast: _,
    } = a;
    Edits {
        exposure: *exposure,
        highlights: *highlights,
        shadows: *shadows,
        whites: *whites,
        blacks: *blacks,
        hsl: *hsl,
        color_grading: *color_grading,
        sharpening: *sharpening,
        ..Edits::default()
    }
}

/// A stage that could be replaced, beside the CPU stage it would replace.
///
/// ⚠ Both halves travel together because `flush` may decide against the chain, and
/// `GpuStage::cpu()` hands out a borrow that cannot be pushed into the output pipeline.
type Candidate = (Box<dyn GpuStage>, Box<dyn PipelineStage>);

/// Is this run worth a round trip?
///
/// ⚠⚠ **Work, not a stage count.** One expensive stage pays easily — `hsl` alone is a measured
/// 1.56x — while three cheap ones do not, so a minimum count would refuse the case M8.3b shipped
/// and accept the case this exists to refuse.
fn worth_a_chain(costs: &[f32]) -> bool {
    costs.iter().sum::<f32>() > TRANSFER_COST
}

/// Replace runs of GPU-capable stages in `tail` with chains.
///
/// ⚠⚠ **Consecutive stages share ONE chain, and therefore one round trip.** That is the point:
/// sharpening alone is break-even against the transfer, and only the run is worth having.
///
/// ⚠ A regrouping, never a reordering — a stage this cannot replace **ends the current run** and
/// keeps its position. An implementation that merged every replaceable stage regardless of
/// position would run the intervening CPU stage's work in the wrong order, which looks like a
/// plausible photograph rather than an error.
///
/// ⚠ Only the **tail**, never the prefix — accelerating the prefix would make `PrefixKey` describe
/// two different computations under one key.
///
/// ⚠⚠ **Since M8.3j it also recurses ONE level into a stage that carries sub-stages** — in
/// practice `MaskStage`, which the tail still sees as one opaque stage named `"mask"`. That is
/// what makes masked HSL reachable at all; before it, the case reported in use as slow was the one
/// thing the whole GPU tail could not touch.
pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> (Pipeline, Option<GpuChain>) {
    // ⚠⚠ **Depth one, and it is structural rather than a bolted-on limit.** The accelerator handed
    // to the outer walk recurses into a mask; the walk it starts is handed **no** accelerator. That
    // is sound because `adjustment_stages` is exhaustive over `LocalAdjustments`, which has no
    // `masks` field — a mask's sub-stages can never contain a mask. Pinned by
    // `a_masks_sub_stages_never_contain_a_mask` over in `pipeline`.
    let inner = |stages: Vec<Box<dyn PipelineStage>>,
                 a: &edit_model::LocalAdjustments,
                 sub_scale: RenderScale| {
        let e = sub_edits(a);
        // ⚠ `rejoin`, because a mask wants one plain list. The inner walk can never return a
        // trailing chain anyway: `output_transform` is not something `adjustment_stages` places,
        // so the byte path is unreachable here **by construction** and a mask always pays the
        // float round trip that `TRANSFER_COST` describes.
        let (p, c) = walk(
            Pipeline::new(stages),
            &e,
            sub_scale,
            profile,
            spec,
            ctx,
            None,
            // ⚠⚠ **True only here.** A mask's first sub-stage is the one place in the workspace
            // that receives the very plane a coverage rasteriser was handed — see `mark_shared`.
            true,
        );
        rejoin(p, c).into_stages()
    };
    // ⚠ The cast is explicit: an unsize coercion from `&{closure}` to `&dyn Fn` inside a `Some(..)`
    // is not guaranteed to be inferred through the wrapper.
    walk(
        tail,
        edits,
        scale,
        profile,
        spec,
        ctx,
        Some(&inner as &pipeline::SubStageAccelerator<'_>),
        // ⚠ Nothing in the outer tail stands in a mask's relationship to a coverage rasteriser, so
        // there is never a deposit for its chain to take.
        false,
    )
}

/// One pass of run detection over a stage list.
///
/// ⚠ `inner` is `Some` for the outer walk and **`None` for the recursive one** — see
/// [`accelerate`], where the depth-one argument is written out.
#[allow(clippy::too_many_arguments)]
fn walk(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
    inner: Option<&pipeline::SubStageAccelerator<'_>>,
    share_input: bool,
) -> (Pipeline, Option<GpuChain>) {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠⚠ **Both halves.** A run that turns out not to be worth the transfer has to be put back,
    // and `GpuStage::cpu()` hands out a *borrow* that cannot be pushed into `out` — so the
    // original travels beside the GPU stage and `flush` chooses between them.
    let mut run: Vec<Candidate> = Vec::new();
    // ⚠ Built once for the whole walk rather than per stage: it holds only an `Arc<GpuContext>`,
    // and a mask clones the `Arc` rather than the rasteriser.
    let coverage: Arc<dyn pipeline::CoverageRasterizer> = Arc::new(crate::GpuBrushRaster::new(ctx));
    for mut s in tail.into_stages() {
        // ⚠ Offered to **every** stage this walk sees, before the lookup. A replaceable stage has
        // no inner list and inherits the empty default, so one rule covers both arms rather than
        // one arm carrying an exception nobody would think to check.
        //
        // ⚠⚠ Both hooks are guarded by the same `inner.is_some()`, so the **recursive** call
        // offers neither. That is sound for the same reason the depth bound is: a mask's
        // sub-stages can never contain a mask, so they build no coverage and offering into them
        // would be machinery for a case that cannot arise.
        if let Some(f) = inner {
            s.accelerate_inner(f);
            s.set_coverage_rasterizer(&coverage);
        }
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, profile, spec, ctx), s)),
            None => {
                // ⚠ A chain followed by a CPU stage is **not** the tail's last stage, so it goes
                // straight back into the pipeline and can never be a candidate for the byte path.
                if let Some(mut c) = flush(&mut run, &mut out, ctx) {
                    mark_shared(&mut c, share_input, out.is_empty());
                    out.push(Box::new(c));
                }
                out.push(s);
            }
        }
    }
    // ⚠⚠ Whatever this produces **is** last, by construction — the loop is over, so there is
    // nothing left to push after it. That is what makes the question answerable here and nowhere
    // else: a caller holding the finished `Pipeline` could only recognise a chain by downcasting.
    match flush(&mut run, &mut out, ctx) {
        // ⚠ The name is **checked**, not assumed. `output_transform` ends every tail today, and
        // that is a fact about the current pipeline rather than a guarantee about the next one —
        // and it is the stage that clamps, so it is what makes the output display-referred.
        Some(c) if c.last_name() == Some("output_transform") => (Pipeline::new(out), Some(c)),
        Some(mut c) => {
            mark_shared(&mut c, share_input, out.is_empty());
            out.push(Box::new(c));
            (Pipeline::new(out), None)
        }
        None => (Pipeline::new(out), None),
    }
}

/// Put a chain back on the end of a pipeline.
///
/// ⚠ For the callers that want one plain `Pipeline` — the soft-proof tail, and every case the
/// M8.3g gate turns down. Written once so that "the pipeline plus its chain" cannot be assembled
/// two ways.
pub fn rejoin(p: Pipeline, chain: Option<GpuChain>) -> Pipeline {
    match chain {
        None => p,
        Some(c) => {
            let mut v = p.into_stages();
            v.push(Box::new(c));
            Pipeline::new(v)
        }
    }
}

/// Let a chain take a mask's deposited base plane, when it is that mask's **first** sub-stage.
///
/// ⚠⚠ **`out.is_empty()` is what "first" means, and it is checked AT THE PUSH rather than inferred
/// afterwards.** A finished `Vec<Box<dyn PipelineStage>>` cannot be asked which of its entries is
/// a chain without downcasting, so the position has to be recorded where it is still known.
///
/// ⚠ `share_input` is true only for the recursive walk — a mask's sub-stages — because the
/// property this rests on is `MaskStage::apply`'s: it clones the image and pipes it through its
/// sub-stages in order, so only the first receives the plane the gate was handed. A chain at index
/// 1 gets whatever index 0 produced, and taking the deposit there is a plausible wrong picture.
fn mark_shared(c: &mut GpuChain, share_input: bool, is_first: bool) {
    if share_input && is_first {
        c.allow_shared_input();
    }
}

/// Turn any pending run into a chain — or put it back on the CPU if it is not worth the transfer.
///
/// ⚠⚠ **Returns the chain rather than pushing it**, so the caller can see whether the last thing
/// it built was one. Pushing into a `Vec<Box<dyn PipelineStage>>` would make that question
/// answerable only by downcasting.
fn flush(
    run: &mut Vec<Candidate>,
    out: &mut Vec<Box<dyn PipelineStage>>,
    ctx: &Arc<GpuContext>,
) -> Option<GpuChain> {
    if run.is_empty() {
        return None;
    }
    let taken = std::mem::take(run);
    let costs: Vec<f32> = taken.iter().map(|(g, _)| g.cpu_cost()).collect();
    if worth_a_chain(&costs) {
        Some(GpuChain::new(
            ctx.clone(),
            taken.into_iter().map(|(g, _)| g).collect(),
        ))
    } else {
        // ⚠ In order, so a refused run keeps its position exactly as the CPU stages had it —
        // the same rule that makes this a regrouping and never a reordering.
        out.extend(taken.into_iter().map(|(_, c)| c));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{HslBand, Sharpening};

    const SCALE: RenderScale = RenderScale::FULL;

    /// ⚠⚠ The gate's whole reason for existing: `output_transform` is on **every** photograph, so
    /// without this a neutral frame would build a one-stage chain and pay a 34 ms toll for 28.6 ms
    /// of work — a regression where there is none today, felt exactly while browsing.
    #[test]
    fn a_run_cheaper_than_the_transfer_is_left_on_the_cpu() {
        assert!(
            !worth_a_chain(&[0.5, 1.2]),
            "two trivial stages must not be worth a round trip"
        );
    }

    /// ⚠ The other direction, and it is what rules out a stage *count* as the gate: one expensive
    /// stage pays easily — `hsl` alone is a measured 1.56x — while three cheap ones do not.
    #[test]
    fn one_expensive_stage_is_still_worth_a_chain() {
        assert!(
            worth_a_chain(&[60.0]),
            "hsl alone must still be accelerated"
        );
    }

    /// ⚠ Fails toward the CPU: an empty run is never a chain.
    #[test]
    fn an_empty_run_is_not_a_chain() {
        assert!(!worth_a_chain(&[]));
    }

    /// ⚠⚠ Exposure alone must **not** become a chain — 1.3 ms of work against a 34 ms transfer.
    ///
    /// ⚠ Written *before* the `REPLACEABLE` row, when it passes for the trivial reason that
    /// exposure is not yet replaceable at all. That is the point: it pins the behaviour first, so
    /// that once the row lands the test is passing because of the **gate** rather than because of
    /// an absence. A test written afterwards could not tell those two apart.
    #[test]
    fn exposure_alone_is_not_worth_a_chain() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            exposure: 0.45,
            ..Edits::default()
        };
        let (_, tail) = pipeline::build_pipeline_split_in_region(&e, SCALE, None);
        // ⚠⚠ **`output_transform` is filtered out, and M8.3h is what forced that.** Until then
        // this test read the whole tail `["exposure", "output_transform"]` — one run of 29.9 ms
        // — and passed because the *pair* was under a 34.0 ms bar. So it never measured exposure
        // **alone**, which is what it is named for, and it was passing for the wrong reason.
        // With the bar at 27.0 the pair chains and exposure is genuinely alone only when the
        // tail is hand-built. The name is true for the first time.
        let stages: Vec<Box<dyn PipelineStage>> = tail
            .into_stages()
            .into_iter()
            .filter(|s| s.name() != "output_transform")
            .collect();
        assert_eq!(
            stages.len(),
            1,
            "fixture check: exposure must really be alone, got {:?}",
            stages.iter().map(|s| s.name()).collect::<Vec<_>>()
        );
        let names: Vec<_> = accel(
            Pipeline::new(stages),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .into_stages()
        .iter()
        .map(|s| s.name())
        .collect();
        assert!(
            names.contains(&"exposure"),
            "expected the CPU exposure stage, got {names:?}"
        );
    }

    /// ⚠⚠ **Built from the stages' OWN `cpu_cost`, never from literals.** A test asserting
    /// `worth_a_chain(&[28.6])` writes the same number twice and passes against any pair that
    /// happens to agree — it guards nothing at all. Reading the cost off the real stage means
    /// that if a cost is ever re-measured, this still describes the truth rather than a stale
    /// pair of digits.
    ///
    /// ⚠⚠ The `display_rendering` half is not symmetry, it is the argument for a **single
    /// conservative constant**. `CreativeLookStage` sits between `display_rendering` and
    /// `output_transform`, so with a creative look active `display_rendering` is a run of its
    /// own — and that run is **not trailing**, so it can never take M8.3g's byte path. A
    /// byte-valued threshold (21.0) would accept it and then pay the float price of 26.8 for
    /// 25.7 ms of work. One constant refuses it by construction.
    #[test]
    fn a_lone_output_transform_chains_and_a_lone_display_rendering_does_not() {
        let Some(ctx) = ctx() else { return };
        let ot = GpuOutputTransformStage::new(RenderSpec::default().space, &ctx);
        let dr = GpuDisplayRenderingStage::new(None, None, &ctx);
        assert!(
            worth_a_chain(&[ot.cpu_cost()]),
            "a lone trailing output_transform ({} ms) must clear the gate — it is the \
             colorimetric frame and the trailing run of every masked photo",
            ot.cpu_cost()
        );
        assert!(
            !worth_a_chain(&[dr.cpu_cost()]),
            "a lone display_rendering ({} ms) must NOT clear the gate: it is never trailing, so \
             it cannot take the byte path, and accepting it would pay the float price for less \
             work",
            dr.cpu_cost()
        );
    }

    /// ⚠⚠ **The test that would have caught the staleness as a BEHAVIOUR rather than as a
    /// number.** A colorimetric frame's whole tail is `["output_transform"]`, and from M8.3e
    /// until M8.3h it built **zero chains** — the gate refusing 28.6 ms of work against a 34.0 ms
    /// threshold that M8.3f and M8.3g had already invalidated. Nothing in the suite could see
    /// that, because every other test asserts on what a chain *contains* rather than on whether
    /// one exists at all.
    #[test]
    fn a_colorimetric_tail_builds_a_chain() {
        let Some(ctx) = ctx() else { return };
        let e = Edits::default();
        let (_, tail) = pipeline::build_pipeline_split_in_region(&e, SCALE, None);
        assert_eq!(
            tail.stage_names(),
            vec!["output_transform"],
            "fixture check: a colorimetric tail is exactly one stage, and this test means \
             nothing if that ever stops being true"
        );
        let (rest, chain) = split(tail, &e, &ctx);
        assert!(
            chain.is_some(),
            "the lone output_transform must now be a chain"
        );
        assert!(
            rest.stage_names().is_empty(),
            "and nothing must be left on the CPU, got {:?}",
            rest.stage_names()
        );
    }

    /// ⚠⚠ **A colorimetric frame now takes a GPU path it has never taken, so it must still be
    /// the CPU's picture.** The pieces are covered — `output_transform`'s own equivalence test,
    /// and M8.3g's byte-against-float frame test — but nothing covered the *combination*, which
    /// is exactly what M8.3h newly creates. A chain that rendered a plausible-but-different
    /// neutral photograph would pass every other test in this file.
    ///
    /// ⚠ The bound is the **per-stage** one (0.01 of an 8-bit level), not the three-stage chain's
    /// 0.02: this is a one-stage chain, and the looser bound exists for accumulation across
    /// several transcendental stages that is not happening here.
    #[test]
    fn a_colorimetric_frame_renders_the_same_picture_on_the_gpu() {
        let Some(ctx) = ctx() else { return };
        let e = Edits::default();
        let cpu = tail_of(&e).run(scene_ramp());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(scene_ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ **The guard on the projection, and the only thing that can see a mis-mapped field.**
    /// Every one of the eight projected fields is non-default here, so a projection that forgot
    /// `whites` — or carried the *global* value instead of the mask's — renders a different
    /// photograph. A fixture setting only `hsl` would be blind to seven of the eight.
    ///
    /// ⚠ The fixture check matters as much as the assertion: an accelerated mask and a CPU one
    /// agree trivially if the mask covers nothing, so the masked render is first shown to differ
    /// from the unmasked one.
    #[test]
    fn a_mask_carrying_every_ported_stage_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let adjustments = edit_model::LocalAdjustments {
            exposure: 0.6,
            highlights: -30.0,
            shadows: 35.0,
            whites: 18.0,
            blacks: -14.0,
            hsl: with_hsl().hsl,
            color_grading: graded(Edits::default()).color_grading,
            sharpening: Sharpening {
                amount: 55.0,
                radius: 1.2,
                detail: 50.0,
                masking: 0.0,
            },
            ..Default::default()
        };
        let e = Edits {
            masks: vec![mask_carrying(adjustments)],
            ..Edits::default()
        };

        let unmasked = tail_of(&Edits::default()).run(scene_ramp());
        let cpu = tail_of(&e).run(scene_ramp());
        assert!(
            !crate::compare(&unmasked, &cpu).is_exact(),
            "fixture check: the mask must actually change the picture"
        );

        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(scene_ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.02,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// The whole accelerated tail as one pipeline.
    ///
    /// ⚠ Every test that predates M8.3g goes through here, so its assertions are **unchanged** —
    /// which is what makes their passing evidence that splitting the last chain out moved no
    /// behaviour, rather than merely evidence that they were edited until they passed.
    fn accel(
        tail: Pipeline,
        e: &Edits,
        scale: RenderScale,
        profile: ProfileRendering<'_>,
        spec: RenderSpec,
        ctx: &Arc<GpuContext>,
    ) -> Pipeline {
        let (p, c) = accelerate(tail, e, scale, profile, spec, ctx);
        rejoin(p, c)
    }

    fn split(tail: Pipeline, e: &Edits, ctx: &Arc<GpuContext>) -> (Pipeline, Option<GpuChain>) {
        accelerate(
            tail,
            e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            ctx,
        )
    }

    /// ⚠⚠ The condition M8.3g's whole gate rests on, and it is answered **by construction** here
    /// rather than by a caller trying to recognise a chain through a `Box<dyn PipelineStage>`.
    #[test]
    fn a_tail_ending_in_an_accelerated_output_transform_hands_the_chain_back() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let (rest, chain) = split(tail_of(&e), &e, &ctx);
        let chain = chain.expect("this tail ends in a chain");
        assert_eq!(
            chain.last_name(),
            Some("output_transform"),
            "only a chain ending in the output transform is display-referred"
        );
        assert!(
            !rest.stage_names().contains(&"gpu_chain"),
            "the returned pipeline must no longer hold that chain, got {:?}",
            rest.stage_names()
        );
    }

    /// ⚠⚠ **A CPU stage after the chain means the tail's output is NOT the chain's output**, so
    /// nothing may be handed back.
    ///
    /// ⚠ No `Edits` can produce this today — `output_transform` ends every tail and is itself
    /// replaceable — so the tail is built by hand. The plan predicted this case would be
    /// unreachable and recorded as inert; it is constructible after all, which is strictly better
    /// than a guard nothing exercises.
    #[test]
    fn a_cpu_stage_after_the_chain_means_no_chain_is_handed_back() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let mut stages = tail_of(&e).into_stages();
        stages.push(Box::new(pipeline::SaturationStage::new(20.0, 0.0)));
        let (rest, chain) = split(Pipeline::new(stages), &e, &ctx);
        assert!(
            chain.is_none(),
            "a trailing CPU stage must keep the chain inside the pipeline"
        );
        assert_eq!(
            chains(&rest.stage_names()),
            1,
            "and the chain must still be there: {:?}",
            rest.stage_names()
        );
    }

    /// ⚠⚠ **The name guard, and it needs its own fixture because nothing else reaches it.** The
    /// trailing-CPU-stage test above does not: there the final flush is empty, so `None` comes back
    /// whether or not the name is checked. Only a chain that genuinely *is* last while ending in
    /// something other than `output_transform` exercises it — which no `Edits` can produce today,
    /// since that stage ends every tail and is itself replaceable. Hand-built, so the guard is
    /// exercised rather than recorded as inert.
    ///
    /// ⚠ What it protects: `run_rgba8` quantizes to a byte, which is sound only for
    /// display-referred `[0, 1]` data. A chain ending in `hsl` hands back scene-referred values
    /// with headroom above white, and quantizing those would clip every highlight to 255.
    #[test]
    fn a_chain_not_ending_in_the_output_transform_is_not_handed_back() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let stages: Vec<Box<dyn PipelineStage>> = tail_of(&e)
            .into_stages()
            .into_iter()
            .filter(|s| s.name() != "output_transform")
            .collect();
        let (rest, chain) = split(Pipeline::new(stages), &e, &ctx);
        assert!(
            chain.is_none(),
            "a chain ending in {:?} is not display-referred and must stay in the pipeline",
            rest.stage_names()
        );
        assert_eq!(chains(&rest.stage_names()), 1, "and it must still be there");
    }

    /// ⚠ A run the cost gate refused is not a chain at all — the byte path must not appear out of
    /// a tail that was left entirely on the CPU.
    ///
    /// ⚠⚠ **The fixture is hand-built, and M8.3h is what forced that.** This used a colorimetric
    /// `Edits::default()`, whose tail is `["output_transform"]` — refused at 34.0 and **chained
    /// at 27.0**, because that stage alone is 28.6 ms. Since `output_transform` ends every tail
    /// and now always clears the bar on its own, **a trailing run can no longer be refused
    /// through any real `Edits`**, so the property is reachable only by construction. Same shape
    /// as `a_chain_not_ending_in_the_output_transform_is_not_handed_back` just above, and the
    /// same reason M8.3e had to rebuild four tests: a premise about one stage's cost was
    /// load-bearing without being stated.
    #[test]
    fn a_tail_the_cost_gate_refused_hands_nothing_back() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            exposure: 0.45,
            ..Edits::default()
        };
        let stages: Vec<Box<dyn PipelineStage>> = tail_of(&e)
            .into_stages()
            .into_iter()
            .filter(|s| s.name() != "output_transform")
            .collect();
        let cheap = GpuExposureStage::new(0.45, ctx.clone());
        assert!(
            !worth_a_chain(&[cheap.cpu_cost()]),
            "fixture check: this run ({} ms) must be under the bar, or the test proves nothing",
            cheap.cpu_cost()
        );
        let (rest, chain) = split(Pipeline::new(stages), &e, &ctx);
        assert!(chain.is_none());
        assert!(
            rest.stage_names().contains(&"exposure"),
            "the CPU stage must survive, got {:?}",
            rest.stage_names()
        );
    }

    fn with_hsl() -> Edits {
        let mut e = Edits::default();
        e.hsl.bands[0] = HslBand {
            hue: 20.0,
            saturation: 10.0,
            luminance: 5.0,
        };
        e
    }

    fn with_sharpening(mut e: Edits) -> Edits {
        e.sharpening = Sharpening {
            amount: 60.0,
            ..Default::default()
        };
        e
    }

    /// ⚠⚠ **The projection must carry every field a GPU builder reads, and forgetting one renders
    /// a plausible photograph rather than an error.** So this sets all eight to values nothing else
    /// in the suite uses and reads each back — a test naming "the projection" while checking only
    /// `exposure` would be blind to the other seven.
    #[test]
    fn the_projection_carries_every_field_a_gpu_builder_reads() {
        let a = edit_model::LocalAdjustments {
            exposure: -1.25,
            highlights: -33.0,
            shadows: 44.0,
            whites: 22.0,
            blacks: -11.0,
            hsl: with_hsl().hsl,
            color_grading: graded(Edits::default()).color_grading,
            sharpening: Sharpening {
                amount: 63.0,
                radius: 1.7,
                detail: 44.0,
                masking: 21.0,
            },
            ..Default::default()
        };
        let e = sub_edits(&a);
        assert_eq!(e.exposure, -1.25);
        assert_eq!(e.highlights, -33.0);
        assert_eq!(e.shadows, 44.0);
        assert_eq!(e.whites, 22.0);
        assert_eq!(e.blacks, -11.0);
        assert_eq!(e.hsl, a.hsl);
        assert_eq!(e.color_grading, a.color_grading);
        assert_eq!(e.sharpening, a.sharpening);
    }

    /// ⚠ The projection must carry **nothing else**. A field that leaked in would be a global
    /// value applied inside a mask — same class of bug, opposite direction.
    #[test]
    fn the_projection_carries_nothing_a_gpu_builder_does_not_read() {
        let a = edit_model::LocalAdjustments {
            contrast: 40.0,
            saturation: 35.0,
            vibrance: 30.0,
            ..Default::default()
        };
        let e = sub_edits(&a);
        let d = Edits::default();
        assert_eq!(e.contrast, d.contrast);
        assert_eq!(e.saturation, d.saturation);
        assert_eq!(e.vibrance, d.vibrance);
        assert_eq!(e.masks.len(), 0, "a projection must never carry masks");
    }

    fn tail_of(e: &Edits) -> Pipeline {
        pipeline::build_pipeline_split(e, SCALE).1
    }

    /// A mask carrying exactly what it is handed, under a full-frame linear gradient.
    fn mask_carrying(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: a,
        }
    }

    /// The sub-stage names of the first `mask` stage in a pipeline.
    fn mask_sub_stages(p: &Pipeline) -> Vec<&'static str> {
        p.sub_stage_names_of("mask")
            .expect("fixture check: the pipeline must contain a mask stage")
    }

    /// ⚠⚠ **The carrier: a mask's sub-stages really do become a chain.** Every equivalence test in
    /// this file compares an accelerated render against a CPU one, and a hook that never fired
    /// makes the two trivially identical — so something has to assert the chain was **built**. That
    /// is the W5 failure mode (creative LUTs shipped fully tested and 100% dead) as a test.
    /// A brush mask, because only a **painted** mask rasterises coverage at all — an analytic one
    /// answers from its own position, so offering it a rasteriser is a no-op and this test would
    /// be vacuous.
    fn brush_mask() -> edit_model::MaskedAdjustment {
        let mut m = mask_carrying(edit_model::LocalAdjustments {
            exposure: 0.5,
            ..Default::default()
        });
        m.mask = edit_model::Mask::Brush(edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: (0..10).map(|i| [0.3 + i as f32 * 0.03, 0.5]).collect(),
                radius: 0.05,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        });
        m
    }

    /// ⚠⚠ **The carrier: `accelerate` really offers the rasteriser.** Everything else in this file
    /// would pass with the offer deleted — the mask would build its coverage on the CPU and render
    /// the same picture, because the two rasterisers agree to within one level by design. Even
    /// `sub_stage_names_of` cannot see it, since a coverage rasteriser is not a stage. That is the
    /// W5 failure mode, and this is the only guard against it.
    #[test]
    fn accelerate_offers_a_coverage_rasterizer_to_a_mask() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![brush_mask()],
            ..Edits::default()
        };
        assert!(
            !tail_of(&e).mask_has_coverage_rasterizer(),
            "fixture check: an unaccelerated tail must hold none, or this proves nothing"
        );
        let accelerated = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert!(
            accelerated.mask_has_coverage_rasterizer(),
            "the mask must have been offered a coverage rasteriser"
        );
    }

    // ── M8.3l: the shared upload ──────────────────────────────────────────────────────
    //
    // ⚠⚠ **A shared upload changes no pixel any test can read**, because the same bytes reach the
    // same buffer by a different route. Every equivalence test in this file passes with the whole
    // mechanism deleted, so the deposit counters are the only observable — the W5 failure mode,
    // and the same reason `GpuChain::transfer_counts` exists.

    /// A **private** context, so the shared plane's counters describe this test and nothing else.
    ///
    /// ⚠⚠ **Never `shared()` here.** That one is process-wide and `cargo test` runs in parallel,
    /// so its deposit and take counts would be whatever the rest of the suite happened to do —
    /// exactly the contamination that made `GpuChain`'s transfer counters per chain rather than
    /// global. A second `GpuContext::new()` in a process that already has one is **259 us**
    /// (measured in M8.3a), so a private context costs nothing.
    fn private_ctx() -> Option<Arc<GpuContext>> {
        match GpuContext::new() {
            Some(c) => Some(Arc::new(c)),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A brush big enough for the GPU and gating hard enough to make it upload a base.
    ///
    /// ⚠⚠ **Both properties are load-bearing and neither is obvious.** `GpuBrushRaster` declines a
    /// box below `MIN_GPU_BOX_PX` (512 Ki px), so a small stroke never reaches the GPU at all; and
    /// the base plane is uploaded **only when a stroke is auto-masked**, since a positional brush
    /// needs no pixels to rasterise. A fixture missing either one deposits nothing and makes every
    /// assertion below vacuous — which is why each test asserts on `deposits()` first.
    fn gating_brush_mask(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        let mut m = mask_carrying(a);
        m.mask = edit_model::Mask::Brush(edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: (0..12).map(|i| [0.1 + i as f32 * 0.07, 0.5]).collect(),
                radius: 0.45,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        });
        m
    }

    /// A scene big enough that the brush's box clears `MIN_GPU_BOX_PX`.
    fn big_scene() -> core_image::ImageBuf {
        let (w, h) = (1024, 1024);
        core_image::ImageBuf::from_vec(
            w,
            h,
            (0..w * h * 3).map(|i| (i % 977) as f32 / 977.0).collect(),
        )
    }

    /// ⚠⚠ **THE test of this milestone.** The mask's gate uploads the base plane, and the mask's
    /// first sub-stage chain takes that very allocation instead of uploading the same bytes again.
    #[test]
    fn a_masked_chain_at_index_zero_takes_the_plane_its_gate_uploaded() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert_eq!(
            mask_sub_stages(&p),
            vec!["gpu_chain"],
            "fixture check: the mask's hsl must have become a chain, and it must be first"
        );
        let _ = p.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the gate must have uploaded a base plane, or there is nothing to share"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            1,
            "the mask's first sub-stage must take the plane instead of uploading it again"
        );
    }

    /// ⚠⚠ **The negative half, and it is the one that matters.** A chain behind a CPU stage
    /// receives **that stage's output**, not the plane the gate was handed. Taking the deposit
    /// there would render a picture that is plausible and wrong — the failure mode this project
    /// records more than any other.
    ///
    /// ⚠ `local_contrast` is the wedge because it is a mask sub-stage that is **not** in
    /// `REPLACEABLE`, so it stays on the CPU, and it runs *before* hsl in a mask's chain — so the
    /// chain lands at index 1.
    #[test]
    fn a_chain_behind_a_cpu_stage_does_not_take_the_plane() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                local_contrast: edit_model::LocalContrast {
                    texture: 0.0,
                    clarity: 40.0,
                },
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let subs = mask_sub_stages(&p);
        assert_eq!(
            subs.first(),
            Some(&"local_contrast"),
            "fixture check: a CPU stage must come first, got {subs:?}"
        );
        assert!(
            subs.contains(&"gpu_chain"),
            "fixture check: there must still be a chain behind it, got {subs:?}"
        );
        let _ = p.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the gate must still have uploaded a base plane"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            0,
            "a chain that is not the mask's first sub-stage must upload its own input"
        );
    }

    /// ⚠⚠ **The test that fails when `begin_mask` is deleted, and nothing else does.** Render
    /// twice through one pipeline: each render's gate deposits a fresh plane, so each take must be
    /// of that render's own. A slot that were never cleared would still hand out *a* plane — the
    /// previous render's — and the take count would look identical, so the assertion that
    /// discriminates is on the **pixels**.
    #[test]
    fn a_second_render_does_not_take_the_first_renders_plane() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let first = p.run(big_scene());
        let second = p.run(big_scene());
        let d = crate::compare(&first, &second);
        assert!(
            d.is_exact(),
            "two renders of one scene must agree exactly: {d:?}"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            2,
            "each render must share its own plane"
        );
    }

    /// ⚠⚠ **`share_input` is load-bearing, and finding that out took a sabotage rather than
    /// review.** The obvious reading is that restricting the offer to a mask's sub-stages is belt
    /// and braces — the outer tail has no coverage rasteriser, so what deposit could its chain
    /// possibly take? **The previous frame's.** A mask whose own chain sits behind a CPU stage
    /// leaves its deposit in the slot; the outer tail's chain runs *before* any mask, so on the
    /// next frame it would find that plane, at matching dimensions on the same thread, and render
    /// the previous frame's pixels into the current frame's tail.
    ///
    /// ⚠ The fixture therefore needs both halves at once: a **global** hsl, which puts a chain at
    /// index 0 of the outer walk, and a mask whose chain is behind `local_contrast`, which is what
    /// leaves a deposit unclaimed. Neither alone can see it.
    #[test]
    fn the_outer_tails_own_chain_never_takes_a_masks_leftover_plane() {
        let Some(ctx) = private_ctx() else { return };
        let mut e = with_hsl();
        e.masks = vec![gating_brush_mask(edit_model::LocalAdjustments {
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 40.0,
            },
            hsl: with_hsl().hsl,
            ..Default::default()
        })];
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert_eq!(
            p.stage_names().first(),
            Some(&"gpu_chain"),
            "fixture check: the global hsl must be a chain at the outer tail's index 0, got {:?}",
            p.stage_names()
        );
        assert_eq!(
            mask_sub_stages(&p).first(),
            Some(&"local_contrast"),
            "fixture check: the mask's chain must sit behind a CPU stage, so its deposit is left"
        );

        let first = p.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the mask's gate must have deposited a plane nobody took"
        );
        let second = p.run(big_scene());
        let d = crate::compare(&first, &second);
        assert!(
            d.is_exact(),
            "the outer tail must not render a plane left over from a mask: {d:?}"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            0,
            "nothing outside a mask may take a deposited plane"
        );
    }

    /// A brush the GPU will rasterise but that needs **no base plane**, so it deposits nothing.
    ///
    /// ⚠ `auto_mask: false` is the whole of it: without a gate `plan.gating_rows` is `None`, and
    /// the rasteriser uploads no base at all. That is also why the shared upload is worth nothing
    /// to a plain brush — a real limit of this milestone rather than an oversight.
    fn plain_brush_mask(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        let mut m = gating_brush_mask(a);
        if let edit_model::Mask::Brush(b) = &mut m.mask {
            b.strokes[0].auto_mask = false;
        }
        m
    }

    /// A second scene, plainly different from [`big_scene`] and the same size.
    fn other_scene() -> core_image::ImageBuf {
        let (w, h) = (1024, 1024);
        core_image::ImageBuf::from_vec(
            w,
            h,
            (0..w * h * 3)
                .map(|i| 1.0 - (i % 389) as f32 / 389.0)
                .collect(),
        )
    }

    /// ⚠⚠ **THE test `begin_mask` exists for, and the obvious one could not see it.** A test that
    /// renders the same pipeline twice cannot: its chain *takes* the deposit every time, so the
    /// slot is already empty at the next apply and clearing is a no-op. Deleting `begin_mask`'s
    /// body left that test green — found by running the sabotage, not by reading.
    ///
    /// ⚠ The discriminating case needs two different pipelines sharing one context: one that
    /// **leaves** a deposit (its chain sits behind a CPU stage) and one that **deposits nothing**
    /// (a plain brush needs no base) whose chain is at index 0 and would therefore take whatever
    /// it finds. Without the clear, the second renders the first's pixels — a plausible
    /// photograph, and of the wrong photograph.
    #[test]
    fn a_mask_that_deposits_nothing_never_takes_another_masks_plane() {
        let Some(ctx) = private_ctx() else { return };

        let leaver = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                local_contrast: edit_model::LocalContrast {
                    texture: 0.0,
                    clarity: 40.0,
                },
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let taker = Edits {
            masks: vec![plain_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };

        let build = |e: &Edits| {
            accel(
                tail_of(e),
                e,
                SCALE,
                ProfileRendering::default(),
                RenderSpec::default(),
                &ctx,
            )
        };
        let (pl, pt) = (build(&leaver), build(&taker));
        assert_eq!(
            mask_sub_stages(&pl).first(),
            Some(&"local_contrast"),
            "fixture check: the leaver's chain must sit behind a CPU stage"
        );
        assert_eq!(
            mask_sub_stages(&pt),
            vec!["gpu_chain"],
            "fixture check: the taker's chain must be at index 0, or it could never take one"
        );

        // ⚠ The reference first, on a context whose slot is empty.
        let clean = pt.run(other_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            0,
            "fixture check: a plain brush must deposit nothing, or this proves nothing"
        );

        let _ = pl.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the leaver must have deposited a plane nobody took"
        );

        let after = pt.run(other_scene());
        let d = crate::compare(&clean, &after);
        assert!(
            d.is_exact(),
            "a mask that deposited nothing must render its own input, not another's: {d:?}"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            0,
            "nothing may take a stale plane"
        );
    }

    #[test]
    fn a_masked_hsl_becomes_a_chain_inside_the_mask() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let before = mask_sub_stages(&tail_of(&e));
        assert_eq!(
            before,
            vec!["hsl"],
            "fixture check: the mask must carry exactly hsl before acceleration"
        );
        let after = mask_sub_stages(&accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        ));
        assert_eq!(
            after,
            vec!["gpu_chain"],
            "the mask's hsl must have become a chain"
        );
    }

    /// ⚠⚠ **The gate reaches the right verdict INSIDE a mask, and this is the case the catalogue
    /// actually has.** Its one masked photograph carries `exposure: -3.6` and nothing else — 1.3 ms
    /// of work against a 27.0 ms transfer — so it must be **refused**. A milestone that accelerated
    /// it would make that photograph slower.
    #[test]
    fn a_mask_carrying_only_exposure_is_not_worth_a_chain() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                exposure: -3.6,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let after = mask_sub_stages(&accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        ));
        assert_eq!(
            after,
            vec!["exposure"],
            "exposure alone must stay on the CPU inside a mask"
        );
    }

    /// ⚠⚠ **Accelerating the tail must not reach into the PREFIX**, or `PrefixKey` would describe
    /// two different computations under one key. The mask below carries both halves — noise
    /// reduction runs in the prefix, HSL in the tail — so one `Edits` exercises both, and the two
    /// assertions are the two directions of one property.
    #[test]
    fn accelerating_the_tail_leaves_the_prefixs_mask_alone() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                noise_reduction: edit_model::NoiseReduction {
                    luminance: 40.0,
                    color: 40.0,
                    ..Default::default()
                },
                ..Default::default()
            })],
            ..Edits::default()
        };
        let (prefix, tail) = pipeline::build_pipeline_split(&e, SCALE);
        assert!(
            prefix.sub_stage_names_of("mask").is_some(),
            "fixture check: the prefix must carry a capture mask"
        );
        let accelerated = accel(
            tail,
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert_eq!(
            mask_sub_stages(&accelerated),
            vec!["gpu_chain"],
            "the tail's mask must be accelerated"
        );
        assert!(
            !prefix
                .sub_stage_names_of("mask")
                .unwrap()
                .contains(&"gpu_chain"),
            "the prefix's mask must be untouched"
        );
    }

    fn chains(names: &[&str]) -> usize {
        names.iter().filter(|n| **n == "gpu_chain").count()
    }

    #[test]
    fn the_replaceable_names_occur_at_most_once_in_a_tail() {
        // ⚠⚠ This is what makes name-matching sound, and it is a real property rather than an
        // assumption: a mask's own HSL and sharpening live INSIDE MaskStage, which the tail sees
        // as a single stage named "mask". A future refactor that flattened masks into the tail
        // would break this silently, so it is asserted **with a masked HSL present**.
        //
        // ⚠ Over the whole table rather than over "hsl" alone: a guard that enumerates one entry
        // stops guarding the moment the table grows, which is the sharpest finding the masked-grade
        // milestone left behind.
        let mut e = with_sharpening(with_hsl());
        e.masks = vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                sharpening: Sharpening {
                    amount: 40.0,
                    ..Default::default()
                },
                ..Default::default()
            },
        }];
        let names = tail_of(&e).stage_names();
        for (n, _) in REPLACEABLE {
            assert!(
                names.iter().filter(|x| *x == n).count() <= 1,
                "{n} occurs more than once in {names:?}"
            );
        }
    }

    /// Every replaceable stage must be inside a chain, so none of their names survives.
    ///
    /// ⚠ Replaces the `after.len() == before.len() - N` arithmetic those tests used. That form
    /// encodes *how many* stages merged, which changes every time a stage is ported — it broke
    /// four tests at once when `output_transform` joined — where what they mean is that nothing
    /// replaceable was left behind.
    fn no_replaceable_survives(after: &[&str]) {
        for n in after {
            assert!(
                !REPLACEABLE.iter().any(|(r, _)| r == n),
                "{n} is replaceable but was left on the CPU, in {after:?}"
            );
        }
    }

    #[test]
    fn consecutive_gpu_stages_merge_into_one_chain() {
        // With colour grading neutral, `hsl` and `sharpening` are adjacent in the tail — which is
        // the whole reason this slice ported sharpening rather than colour grading.
        let Some(ctx) = ctx() else { return };
        let e = with_sharpening(with_hsl());
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            before.windows(2).any(|w| w == ["hsl", "sharpening"]),
            "fixture check: hsl and sharpening must be adjacent, got {before:?}"
        );
        let after = accel(
            tail,
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        assert_eq!(
            chains(&after),
            1,
            "two adjacent GPU stages must become ONE chain, got {after:?}"
        );
        no_replaceable_survives(&after);
    }

    #[test]
    fn a_cpu_stage_between_two_gpu_stages_produces_two_chains() {
        // ⚠⚠ **The half that makes the test above mean something.** An implementation that merged
        // every replaceable stage regardless of position would pass it — and would run the CPU
        // stage's work in the wrong order, producing a plausible photograph rather than an error.
        //
        // ⚠ **Re-aimed at the VIGNETTE at M8.3d.** It used to use the colour grade, which stopped
        // being a CPU stage when that was accelerated — so the fixture check below would have
        // failed loudly rather than the test going quietly vacuous, but the rule it guards is
        // unchanged and only its example moved. The vignette runs after sharpening and is not
        // replaceable, so it still splits a run.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        e.vignette.amount = -40.0;
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            before.contains(&"vignette"),
            "fixture check: an unaccelerated stage must sit between them, got {before:?}"
        );
        let after = accel(
            tail,
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        assert_eq!(
            chains(&after),
            2,
            "a CPU stage between them must split the run, got {after:?}"
        );
    }

    #[test]
    fn merging_preserves_order() {
        // ⚠ A regrouping, never a reordering. The non-replaceable stage must keep its position
        // relative to the chains around it. ⚠ Re-aimed at the vignette at M8.3d, with the grade.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        e.vignette.amount = -40.0;
        let after = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        let grade = after
            .iter()
            .position(|n| *n == "vignette")
            .expect("the vignette is kept");
        let at: Vec<usize> = after
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == "gpu_chain")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 2, "{after:?}");
        assert!(
            at[0] < grade && grade < at[1],
            "the grade must stay between the two chains, got {after:?}"
        );
    }

    fn graded(mut e: Edits) -> Edits {
        e.color_grading.shadows.saturation = 30.0;
        e.color_grading.shadows.hue = 250.0;
        e
    }

    #[test]
    fn three_consecutive_gpu_stages_merge_into_one_chain() {
        // ⚠⚠ **The whole point of M8.3d.** Before it, an active grade sat between hsl and
        // sharpening, split the run and took the win from 1.52x to 1.04x. Now the three are ONE
        // chain — the first three-stage run, and the best case run-merging has.
        let Some(ctx) = ctx() else { return };
        let e = graded(with_sharpening(with_hsl()));
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            before
                .windows(3)
                .any(|w| w == ["hsl", "color_grading", "sharpening"]),
            "fixture check: the three must be consecutive, got {before:?}"
        );
        let after = accel(
            tail,
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        assert_eq!(
            chains(&after),
            1,
            "three must become ONE chain, got {after:?}"
        );
        no_replaceable_survives(&after);
    }

    /// A ramp reaching **1.2x white**, unlike [`ramp`].
    ///
    /// ⚠⚠ **Added because the ordinary fixture could not see the regime the bench measures.**
    /// `ramp()` never exceeds 1.0, and on it the three-stage chain diverges by 0.00106 levels —
    /// while `gpu_run_bench`, whose ramp reaches 1.2, measured **0.01153** on the same chain. A
    /// carrier that only ever sees in-gamut values is not testing the composition at all.
    fn scene_ramp() -> core_image::ImageBuf {
        core_image::ImageBuf::from_vec(
            64,
            48,
            (0..64 * 48 * 3)
                .map(|i| (i % 719) as f32 / 719.0 * 1.2)
                .collect(),
        )
    }

    #[test]
    fn a_three_stage_run_renders_the_same_picture() {
        // ⚠ The carrier. The shape test above passes against a chain of three stages that compute
        // nonsense, or that run in the wrong order.
        //
        // ⚠⚠ **The bound is 0.02, not the 0.01 every single-stage test uses, and that is a
        // MEASURED property of composition rather than a loosened number.** Three transcendental
        // stages each under 0.01 do not compose to under 0.01: hsl alone measures 0.00104,
        // sharpening 0.00905, and the three together 0.01153 on a scene-referred ramp. The
        // per-stage bound is what catches a wrong stage; this one catches a wrong *chain*, and
        // asserting 0.01 here would mean a passing test only for as long as the fixture stayed
        // easy. ⚠ 0.02 is still ~50x below one 8-bit level — if it ever has to be widened
        // *again*, that is a finding to investigate rather than a number to adjust.
        let Some(ctx) = ctx() else { return };
        let e = graded(with_sharpening(with_hsl()));
        let cpu = tail_of(&e).run(scene_ramp());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(scene_ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.02,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn accelerating_replaces_only_the_replaceable_stages() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let tail = tail_of(&e);
        let before = tail.stage_names();
        let after = accel(
            tail,
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        no_replaceable_survives(&after);
        // ⚠ And every stage this cannot replace survives, in the same relative order. A
        // one-to-one zip would have said that more directly and is no longer available: a run
        // collapses several stages into one chain, so the two lists have different lengths.
        let kept: Vec<_> = before
            .iter()
            .filter(|b| !REPLACEABLE.iter().any(|(n, _)| n == *b))
            .collect();
        let survived: Vec<_> = after.iter().filter(|a| **a != "gpu_chain").collect();
        assert_eq!(kept, survived, "a non-replaceable stage was touched");
    }

    /// ⚠⚠ **This test's original premise no longer exists, and that is the point of M8.3e.** It
    /// was "a tail with nothing replaceable is returned unchanged", first with `exposure` as the
    /// example of an unreplaceable stage and then with the vignette — and its fixture check fired
    /// both times, loudly, rather than letting it go quietly vacuous. Now `output_transform` is
    /// replaceable and is in **every** tail, so no such tail can be built at all.
    ///
    /// ⚠ What survives is the property it existed for — a tail can come back untouched — and it
    /// is now the **cost gate** that delivers it rather than the absence of a table row, which
    /// makes this an end-to-end test of the gate rather than of a gap.
    #[test]
    fn a_tail_too_cheap_to_be_worth_a_chain_is_returned_unchanged() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Exposure (replaceable, 1.3 ms) beside the vignette (not replaceable), so there is a
        // real run for the gate to refuse rather than nothing to decide about.
        let e = Edits {
            exposure: 0.45,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        // ⚠⚠ **`output_transform` is filtered out, and the hardcoded `28.6` that used to stand
        // in for its cost is gone with it.** That literal is exactly how this fixture check went
        // stale: it asserted `!worth_a_chain(&[28.6])`, which was true at 34.0 and false at
        // 27.0, so the guard fired on M8.3h rather than the assertion — the guard working, and
        // also the reason a cost must be read off the stage rather than written twice.
        let stages: Vec<Box<dyn PipelineStage>> = tail_of(&e)
            .into_stages()
            .into_iter()
            .filter(|s| s.name() != "output_transform")
            .collect();
        let tail = Pipeline::new(stages);
        let before = tail.stage_names();
        // ⚠ The fixture check: there IS something replaceable here, and the run it forms is under
        // the bar. Without this the test could pass on a tail that happened to contain nothing
        // replaceable, which would make "returned unchanged" true for the wrong reason.
        let cheap = GpuExposureStage::new(0.45, ctx.clone());
        assert!(
            before.contains(&"exposure") && !worth_a_chain(&[cheap.cpu_cost()]),
            "fixture check: got {before:?} at {} ms",
            cheap.cpu_cost()
        );
        assert!(before.len() >= 2, "fixture check: got {before:?}");
        assert_eq!(
            accel(
                tail,
                &e,
                SCALE,
                ProfileRendering::default(),
                RenderSpec::default(),
                &ctx
            )
            .stage_names(),
            before,
            "the gate must leave a run cheaper than the transfer on the CPU"
        );
    }

    fn ramp() -> core_image::ImageBuf {
        core_image::ImageBuf::from_vec(
            64,
            48,
            (0..64 * 48 * 3).map(|i| (i % 601) as f32 / 601.0).collect(),
        )
    }

    #[test]
    fn a_merged_tail_renders_the_same_picture() {
        // ⚠⚠ The carrier: everything above can pass while the substituted stages compute
        // nonsense, or while merging silently reorders. This runs both tails over the same buffer.
        let Some(ctx) = ctx() else { return };
        let e = with_sharpening(with_hsl());
        let cpu = tail_of(&e).run(ramp());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn a_split_run_renders_the_same_picture_as_the_cpu() {
        // ⚠ The split path needs its own carrier: two chains with a CPU stage between them is a
        // different execution than one chain, and a reordering bug lives exactly there.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        e.vignette.amount = -40.0;
        let cpu = tail_of(&e).run(ramp());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }
}

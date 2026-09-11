//! Swapping runs of CPU stages for GPU chains in a built pipeline.

use crate::{
    GpuChain, GpuContext, GpuDisplayRenderingStage, GpuExposureStage, GpuGradingStage, GpuHslStage,
    GpuOutputTransformStage, GpuSharpenStage, GpuStage, GpuToneRegionsStage,
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
    ("exposure", |e, _, _, _, ctx| {
        Box::new(GpuExposureStage::new(e.exposure, ctx.clone()))
    }),
    ("tone_regions", |e, _, _, _, ctx| {
        Box::new(GpuToneRegionsStage::new(e, ctx))
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
/// ⚠ Only the **global** tail. A mask's own stages are inside `MaskStage`, so masked HSL is still
/// not accelerated — the case reported in use as slow, and M8.3d's.
pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> (Pipeline, Option<GpuChain>) {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠⚠ **Both halves.** A run that turns out not to be worth the transfer has to be put back,
    // and `GpuStage::cpu()` hands out a *borrow* that cannot be pushed into `out` — so the
    // original travels beside the GPU stage and `flush` chooses between them.
    let mut run: Vec<Candidate> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, profile, spec, ctx), s)),
            None => {
                // ⚠ A chain followed by a CPU stage is **not** the tail's last stage, so it goes
                // straight back into the pipeline and can never be a candidate for the byte path.
                if let Some(c) = flush(&mut run, &mut out, ctx) {
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
        Some(c) => {
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

    fn tail_of(e: &Edits) -> Pipeline {
        pipeline::build_pipeline_split(e, SCALE).1
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

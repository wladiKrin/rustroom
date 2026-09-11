//! Swapping runs of CPU stages for GPU chains in a built pipeline.

use crate::{
    GpuChain, GpuContext, GpuExposureStage, GpuGradingStage, GpuHslStage, GpuSharpenStage,
    GpuStage, GpuToneRegionsStage,
};
use edit_model::Edits;
use pipeline::{Pipeline, PipelineStage, RenderScale};
use std::sync::Arc;

/// Builds one GPU stage from the edits the tail was built from.
///
/// ⚠ It takes the [`RenderScale`] as well as the [`Edits`], because `SharpenStage`'s sigma is in
/// **full-resolution pixels** and only the scale says what that is at this rung. `GpuHslStage`
/// ignores it; a signature that omitted it would make the sharpen entry unwritable.
type Builder = fn(&Edits, RenderScale, &Arc<GpuContext>) -> Box<dyn GpuStage>;

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
    ("exposure", |e, _, ctx| {
        Box::new(GpuExposureStage::new(e.exposure, ctx.clone()))
    }),
    ("tone_regions", |e, _, ctx| {
        Box::new(GpuToneRegionsStage::new(e, ctx))
    }),
    ("hsl", |e, _, ctx| Box::new(GpuHslStage::new(&e.hsl, ctx))),
    ("color_grading", |e, _, ctx| {
        Box::new(GpuGradingStage::new(&e.color_grading, ctx))
    }),
    ("sharpening", |e, scale, ctx| {
        Box::new(GpuSharpenStage::new(&e.sharpening, scale, ctx))
    }),
];

/// What one round trip costs, in the same units as [`GpuStage::cpu_cost`] — milliseconds per 6.8
/// megapixels, measured by `gpu-pipeline`'s `probe_rt` on an Apple M2.
///
/// ⚠⚠ **The comparison is scale-free, which is why this is one number and not one per rung.**
/// Both sides are linear in pixel count — the transfer moves `w*h*12` bytes, a per-pixel stage
/// does `w*h` work — so the ratio does not depend on resolution. Measured rather than assumed:
/// 4x the pixels gave 3.9x the round trip, 8.71 -> 34.03 ms.
const TRANSFER_COST: f32 = 34.0;

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
    ctx: &Arc<GpuContext>,
) -> Pipeline {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠⚠ **Both halves.** A run that turns out not to be worth the transfer has to be put back,
    // and `GpuStage::cpu()` hands out a *borrow* that cannot be pushed into `out` — so the
    // original travels beside the GPU stage and `flush` chooses between them.
    let mut run: Vec<Candidate> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, ctx), s)),
            None => {
                flush(&mut run, &mut out, ctx);
                out.push(s);
            }
        }
    }
    flush(&mut run, &mut out, ctx);
    Pipeline::new(out)
}

/// Turn any pending run into a chain — or put it back on the CPU if it is not worth the transfer.
fn flush(run: &mut Vec<Candidate>, out: &mut Vec<Box<dyn PipelineStage>>, ctx: &Arc<GpuContext>) {
    if run.is_empty() {
        return;
    }
    let taken = std::mem::take(run);
    let costs: Vec<f32> = taken.iter().map(|(g, _)| g.cpu_cost()).collect();
    if worth_a_chain(&costs) {
        out.push(Box::new(GpuChain::new(
            ctx.clone(),
            taken.into_iter().map(|(g, _)| g).collect(),
        )));
    } else {
        // ⚠ In order, so a refused run keeps its position exactly as the CPU stages had it —
        // the same rule that makes this a regrouping and never a reordering.
        out.extend(taken.into_iter().map(|(_, c)| c));
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
        let names: Vec<_> = accelerate(tail, &e, SCALE, &ctx)
            .into_stages()
            .iter()
            .map(|s| s.name())
            .collect();
        assert!(
            names.contains(&"exposure"),
            "expected the CPU exposure stage, got {names:?}"
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
        let after = accelerate(tail, &e, SCALE, &ctx).stage_names();
        assert_eq!(
            chains(&after),
            1,
            "two adjacent GPU stages must become ONE chain, got {after:?}"
        );
        assert_eq!(
            after.len(),
            before.len() - 1,
            "the pair collapses to one stage, got {after:?}"
        );
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
        let after = accelerate(tail, &e, SCALE, &ctx).stage_names();
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
        let after = accelerate(tail_of(&e), &e, SCALE, &ctx).stage_names();
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
        let after = accelerate(tail, &e, SCALE, &ctx).stage_names();
        assert_eq!(
            chains(&after),
            1,
            "three must become ONE chain, got {after:?}"
        );
        assert_eq!(
            after.len(),
            before.len() - 2,
            "three collapse to one stage, got {after:?}"
        );
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
        let gpu = accelerate(tail_of(&e), &e, SCALE, &ctx).run(scene_ramp());
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
        let after = accelerate(tail, &e, SCALE, &ctx).stage_names();
        assert_eq!(
            before.len(),
            after.len(),
            "no stage may be added or dropped"
        );
        for (b, a) in before.iter().zip(&after) {
            if REPLACEABLE.iter().any(|(n, _)| n == b) {
                assert_eq!(*a, "gpu_chain", "{b} must become a chain");
            } else {
                assert_eq!(b, a, "every other stage must be untouched");
            }
        }
    }

    #[test]
    fn a_tail_with_nothing_replaceable_is_returned_unchanged() {
        // ⚠ The other half. Without it, a substitution that replaced the FIRST stage rather than a
        // named one would still pass the test above.
        let Some(ctx) = ctx() else { return };
        // ⚠⚠ The fixture was `exposure` until M8.3e made exposure replaceable, and the fixture
        // check below is what said so — loudly, rather than letting this test quietly stop
        // testing anything. ⚠ It has to be re-chosen again the moment the vignette or the
        // creative look is ported, and the check is what will say when.
        let e = Edits {
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            !before
                .iter()
                .any(|b| REPLACEABLE.iter().any(|(n, _)| n == b)),
            "fixture check: nothing here is replaceable, got {before:?}"
        );
        // ⚠ And it must not be empty, or "unchanged" would be vacuously true.
        assert!(before.len() >= 2, "fixture check: got {before:?}");
        assert_eq!(accelerate(tail, &e, SCALE, &ctx).stage_names(), before);
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
        let gpu = accelerate(tail_of(&e), &e, SCALE, &ctx).run(ramp());
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
        let gpu = accelerate(tail_of(&e), &e, SCALE, &ctx).run(ramp());
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

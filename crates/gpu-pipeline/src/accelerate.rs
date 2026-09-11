//! Swapping runs of CPU stages for GPU chains in a built pipeline.

use crate::{GpuChain, GpuContext, GpuGradingStage, GpuHslStage, GpuSharpenStage, GpuStage};
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
const REPLACEABLE: &[(&str, Builder)] = &[
    ("hsl", |e, _, ctx| Box::new(GpuHslStage::new(&e.hsl, ctx))),
    ("color_grading", |e, _, ctx| {
        Box::new(GpuGradingStage::new(&e.color_grading, ctx))
    }),
    ("sharpening", |e, scale, ctx| {
        Box::new(GpuSharpenStage::new(&e.sharpening, scale, ctx))
    }),
];

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
    let mut run: Vec<Box<dyn GpuStage>> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push(build(edits, scale, ctx)),
            None => {
                flush(&mut run, &mut out, ctx);
                out.push(s);
            }
        }
    }
    flush(&mut run, &mut out, ctx);
    Pipeline::new(out)
}

/// Turn any pending run into a chain.
fn flush(
    run: &mut Vec<Box<dyn GpuStage>>,
    out: &mut Vec<Box<dyn PipelineStage>>,
    ctx: &Arc<GpuContext>,
) {
    if !run.is_empty() {
        out.push(Box::new(GpuChain::new(ctx.clone(), std::mem::take(run))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{HslBand, Sharpening};

    const SCALE: RenderScale = RenderScale::FULL;

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

    #[test]
    fn a_three_stage_run_renders_the_same_picture() {
        // ⚠ The carrier. The shape test above passes against a chain of three stages that compute
        // nonsense, or that run in the wrong order.
        let Some(ctx) = ctx() else { return };
        let e = graded(with_sharpening(with_hsl()));
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
        let e = Edits {
            exposure: 0.5,
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

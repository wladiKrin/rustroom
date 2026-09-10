//! Swapping CPU stages for GPU ones in a built pipeline.

use crate::{GpuChain, GpuContext, GpuHslStage, GpuStage};
use edit_model::Edits;
use pipeline::Pipeline;
use std::sync::Arc;

/// The name of the one stage this can replace.
///
/// ⚠⚠ **Name-matching is stringly-typed and that is the cost of needing no change to
/// `pipeline`.** A `dyn PipelineStage` exposes only `name()` and `apply()`, so the parameters
/// needed to build a GPU equivalent are gone — but they do not have to be recovered from the
/// stage, because the caller still holds the `Edits`.
///
/// ⚠ It does **not** scale to a dozen stages. When several are accelerated, the right answer is a
/// `StageAccelerator` hook that `pipeline` calls at construction; it loses today only because
/// `RenderSpec` is `Copy + PartialEq` and an `Arc<dyn Trait>` is neither.
const HSL: &str = "hsl";

/// Replace GPU-capable stages in `tail` with their GPU twins.
///
/// ⚠ Only the **global** tail. A mask's own HSL lives inside `MaskStage`, which this sees as one
/// opaque stage — so masked HSL is not accelerated, and that is the case reported in use as slow.
pub fn accelerate(tail: Pipeline, edits: &Edits, ctx: &Arc<GpuContext>) -> Pipeline {
    Pipeline::new(
        tail.into_stages()
            .into_iter()
            .map(|s| {
                if s.name() == HSL {
                    let gpu: Box<dyn GpuStage> = Box::new(GpuHslStage::new(&edits.hsl, ctx));
                    Box::new(GpuChain::new(ctx.clone(), vec![gpu])) as _
                } else {
                    s
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::HslBand;

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

    #[test]
    fn the_tail_contains_at_most_one_stage_named_hsl() {
        // ⚠⚠ This is what makes name-matching sound, and it is a real property rather than an
        // assumption: a mask's own HSL lives INSIDE MaskStage, which the tail sees as a single
        // stage named "mask". A future refactor that flattened masks into the tail would break
        // this silently, so it is asserted **with a masked HSL present**.
        let mut e = with_hsl();
        e.masks = vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            },
        }];
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let n = tail.stage_names().iter().filter(|x| **x == HSL).count();
        assert_eq!(n, 1, "got {:?}", tail.stage_names());
    }

    #[test]
    fn accelerating_replaces_the_hsl_stage_and_nothing_else() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = tail.stage_names();
        let after = accelerate(tail, &e, &ctx).stage_names();
        assert_eq!(
            before.len(),
            after.len(),
            "no stage may be added or dropped"
        );
        for (b, a) in before.iter().zip(&after) {
            if *b == HSL {
                assert_eq!(*a, "gpu_chain", "the hsl stage must become a chain");
            } else {
                assert_eq!(b, a, "every other stage must be untouched");
            }
        }
    }

    #[test]
    fn a_tail_with_no_hsl_is_returned_unchanged() {
        // ⚠ The other half. Without it, a substitution that replaced the FIRST stage rather than
        // the named one would still pass the test above.
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            exposure: 0.5,
            ..Edits::default()
        };
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = tail.stage_names();
        assert!(
            !before.contains(&HSL),
            "fixture check: this tail has no hsl"
        );
        assert_eq!(accelerate(tail, &e, &ctx).stage_names(), before);
    }

    #[test]
    fn an_accelerated_tail_renders_the_same_picture() {
        // ⚠⚠ The carrier: everything above can pass while the substituted stage computes
        // nonsense. This runs both tails over the same buffer and compares the pixels.
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let scale = pipeline::RenderScale::new(1.0);
        let img = core_image::ImageBuf::from_vec(
            64,
            48,
            (0..64 * 48 * 3).map(|i| (i % 601) as f32 / 601.0).collect(),
        );
        let (_, cpu_tail) = pipeline::build_pipeline_split(&e, scale);
        let (_, gpu_tail) = pipeline::build_pipeline_split(&e, scale);
        let cpu = cpu_tail.run(img.clone());
        let gpu = accelerate(gpu_tail, &e, &ctx).run(img);
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

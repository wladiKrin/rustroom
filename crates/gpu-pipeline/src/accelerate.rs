//! Swapping runs of CPU stages for GPU chains in a built pipeline.

use crate::{
    GpuChain, GpuContext, GpuContrastStage, GpuDisplayRenderingStage, GpuExposureStage,
    GpuGradingStage, GpuHslStage, GpuLocalContrastStage, GpuOutputTransformStage,
    GpuSaturationStage, GpuSharpenStage, GpuStage, GpuToneCurveStage, GpuToneRegionsStage,
    GpuVignetteStage, GpuWhiteBalanceStage,
};
use edit_model::Edits;
use pipeline::{Pipeline, PipelineStage, Region, RenderScale, RenderSpec, TailRendering};
use std::sync::Arc;

/// Builds one GPU stage from what the tail was built from.
///
/// ⚠ It takes the [`RenderScale`] as well as the [`Edits`], because `SharpenStage`'s sigma is in
/// **full-resolution pixels** and only the scale says what that is at this rung. `GpuHslStage`
/// ignores it; a signature that omitted it would make the sharpen entry unwritable.
///
/// ⚠⚠ **And the [`TailRendering`], because `display_rendering` is NOT built from `Edits`.** Its
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
/// ⚠⚠ **`Option`, so a builder may DECLINE.** `creative_look` is built from the profile's parsed
/// `.cube`, which the tail also needed in order to have placed the stage at all — but "the tail and
/// this call were handed the same profile" is an invariant across two calls rather than something
/// the types say. Declining leaves the CPU stage in place, which is the gate's own rule: fail
/// toward the CPU, where a wrong answer costs milliseconds and never pixels. An `expect` here
/// would panic on a render worker instead.
type Builder = fn(
    &Edits,
    RenderScale,
    // ⚠⚠ The **frame** the render is a part of, when it is a part of one. Only `vignette` reads
    // it — its falloff is measured against the whole frame, so a region must be shaded exactly as
    // that part of the whole would have been. ⚠ Deliberately a parameter of its own rather than a
    // field of `RenderSpec`: `build_pipeline_split_in_region` takes the region *beside* the spec,
    // and giving it two homes in one call is the two-facts-that-can-disagree shape this project
    // has already paid for three times.
    Option<Region>,
    TailRendering<'_>,
    RenderSpec,
    &Arc<GpuContext>,
) -> Option<Box<dyn GpuStage>>;

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
    ("white_balance", |e, _, _, _, spec, ctx| {
        Some(Box::new(GpuWhiteBalanceStage::new(
            pipeline::white_balance_gains(&e.white_balance, spec.baseline),
            ctx,
        )))
    }),
    ("exposure", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuExposureStage::new(e.exposure, ctx.clone())))
    }),
    // ⚠⚠ Between `exposure` and `tone_regions`, which is the whole reason 1.2 ms is worth
    // porting: on the CPU it split the per-pixel head into two runs the gate refused.
    ("contrast", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuContrastStage::new(e.contrast, ctx)))
    }),
    ("tone_regions", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuToneRegionsStage::new(e, ctx)))
    }),
    // ⚠ After `tone_regions`, as in the tail. Like `contrast`, it sits between ported stages and
    // split the per-pixel head before this milestone.
    // ⚠⚠ `with_channels`, never `new`: the tail is on by default, so passing only the composite would
    // give a preview that disagrees with the export on any photo with a channel curve.
    // ⚠⚠ Since M5.1c through `from_edits`, the constructor the CPU pipeline uses too, so the
    // parametric curve (and any later curve field) reaches both or neither.
    ("tone_curve", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuToneCurveStage::from_edits(e, ctx)))
    }),
    // ⚠⚠ **The last stage that could split a mask's sub-stage chain**, and the last that could
    // take the LEADING position and deny a mask M8.3l's shared upload. Takes the render scale,
    // because both bands' radii are in full-resolution pixels.
    ("local_contrast", |e, scale, _, _, _, ctx| {
        Some(Box::new(GpuLocalContrastStage::new(
            &e.local_contrast,
            scale,
            ctx,
        )))
    }),
    // ⚠ Before `hsl`, as in the tail — colour follows tone.
    ("saturation", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuSaturationStage::new(
            e.saturation,
            e.vibrance,
            ctx,
        )))
    }),
    ("hsl", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuHslStage::new(&e.hsl, ctx)))
    }),
    ("color_grading", |e, _, _, _, _, ctx| {
        Some(Box::new(GpuGradingStage::new(&e.color_grading, ctx)))
    }),
    ("sharpening", |e, scale, _, _, _, ctx| {
        Some(Box::new(GpuSharpenStage::new(&e.sharpening, scale, ctx)))
    }),
    // ⚠ M10.11b: grain, after sharpening as in the tail, and reading the region the vignette's row
    // does — its lattice is addressed in the frame's pixels.
    ("grain", |e, scale, region, _, _, ctx| {
        Some(Box::new(crate::GpuGrainStage::new(
            &e.grain, scale, region, ctx,
        )))
    }),
    // ⚠⚠ **After `sharpening`, as in the tail — and the ONLY row that reads the region.** Its
    // falloff is measured against the whole frame, so a region render must be shaded exactly as
    // that part of the whole would have been; passing `None` here would re-centre the vignette on
    // whatever slice is on screen and slide it across the picture as you pan.
    ("vignette", |e, _, region, _, _, ctx| {
        Some(Box::new(GpuVignetteStage::new(&e.vignette, region, ctx)))
    }),
    // ⚠⚠ The only row built from something other than `Edits`. ⚠ `effective_profile_curve` is
    // what the CPU tail applies before constructing this stage — skipping it would render every
    // camera whose profile declares an identity curve differently from the CPU, which is most of
    // them, the X-T4 included.
    ("display_rendering", |_, _, _, rendering, _, ctx| {
        Some(Box::new(GpuDisplayRenderingStage::new(
            pipeline::effective_profile_curve(rendering.curve),
            rendering.baseline_ev,
            ctx,
        )))
    }),
    // ⚠ M10.13d: the look learned for this photo's film simulation, after the rendering and before the
    // user's look, as in the tail — the creative look's shader in sRGB display at full strength. Declines
    // without a cube, as the creative look's row does.
    ("camera_look", |_, _, _, rendering, _, ctx| {
        rendering.camera_look.map(|cube| {
            Box::new(crate::GpuCreativeLookStage::new(
                cube,
                edit_model::LutSpace::SrgbDisplay,
                100.0,
                ctx,
            )) as Box<dyn GpuStage>
        })
    }),
    // ⚠⚠ **Between `display_rendering` and `output_transform`, as in the tail — which is the
    // position that matters most in this table.** A creative look sitting on the CPU is what made
    // a lone `display_rendering` run possible at all, and that run is the one case
    // `TRANSFER_COST`'s doc records as sitting inside the float band and being refused.
    //
    // ⚠ The only row that can DECLINE: the cube comes from the tail's rendering, and the tail placed
    // this stage only because it had one. Declining leaves the CPU stage rather than panicking.
    ("creative_look", |e, _, _, rendering, _, ctx| {
        rendering.look.map(|cube| {
            Box::new(crate::GpuCreativeLookStage::new(
                cube,
                e.look.space,
                e.look.strength,
                ctx,
            )) as Box<dyn GpuStage>
        })
    }),
    // ⚠ Last, as in the tail — and the only stage here that is on **every** photograph, which is
    // what the cost gate exists to keep honest.
    ("output_transform", |_, _, _, _, spec, ctx| {
        Some(Box::new(GpuOutputTransformStage::new(
            spec.space,
            spec.intent,
            ctx,
        )))
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
/// ⚠ The carried fields are exactly the ones a `REPLACEABLE` builder reads. The **two** ignored
/// ones have no GPU stage and cannot have one here: noise reduction and dehaze run in the
/// **prefix** and are never in a mask's tail sub-stage list at all.
///
/// ⚠⚠ **THE EXHAUSTIVE DESTRUCTURE DOES NOT PROTECT AGAINST THE FAILURE THAT ACTUALLY HAPPENED.**
/// It catches a *new field*, because a new field has no arm. It cannot catch a field whose status
/// **changed** — and M8.3m shipped exactly that: it added `white_balance`, `contrast`,
/// `tone_curve` and `saturation` to [`REPLACEABLE`] and left all four sitting under
/// "deliberately not carried" here. The consequence inside a mask was silent and total: the
/// builder read `Edits::default()`, so a masked white balance solved gains of `[1, 1, 1]`, a
/// masked contrast a slope of 1, a masked tone curve an empty point list — **every one of them an
/// identity**, while the CPU stage they replaced did the work. A slider that does nothing, with
/// `gpu_tail` on, and no error anywhere.
///
/// ⚠ Nobody hit it: `gpu_tail` is off by default and 0 of 40 sidecars use those four. That is luck
/// rather than a guard. **Anything added to `REPLACEABLE` must be carried here**, and
/// `a_mask_carrying_every_ported_stage_renders_the_same_picture` is what now sees it — its fixture
/// is built from the same list rather than from a hand-written set that went stale.
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
        white_balance,
        contrast,
        tone_curve,
        saturation,
        vibrance,
        local_contrast,
        // deliberately not carried — these run in the PREFIX and cannot reach a mask's tail
        noise_reduction: _,
        dehaze: _,
        defringe: _,
        // deliberately not carried — a relative white-balance shift is a scene OFFSET, applied by
        // `SceneOffsetsStage` at the head of the tail, never one of a mask's sub-stages (M10.12b)
        temp: _,
        tint: _,
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
        white_balance: *white_balance,
        contrast: *contrast,
        tone_curve: tone_curve.clone(),
        saturation: *saturation,
        vibrance: *vibrance,
        local_contrast: *local_contrast,
        ..Edits::default()
    }
}

/// A GPU twin for one **prefix** step, or `None` to leave it on the CPU — the first acceleration
/// outside the tail.
///
/// ⚠⚠ **Per step and at APPLY time, not over the whole list up front, and the airlight is what
/// forces that.** `dehaze::airlight` is estimated from the image the stage is handed — which in
/// the prefix is the checkpoint *after* geometry and noise reduction, not the raw buffer. Geometry
/// crops, so an estimate taken from the buffer would key on pixels the crop has removed: a
/// slightly different airlight, which is a different photograph rather than an error. The plan for
/// this milestone said "substitute the list up front" and was wrong for exactly that reason.
///
/// ⚠⚠ **The caller keeps the step's `key` untouched, and that is the whole safety argument.**
/// `PrefixStepKey` is derived from the **edits**, never from the stage — so a checkpoint written
/// by the CPU path is a legitimate hit for the GPU path and the reverse, and `PrefixCache` needs
/// no change at all. ⚠ It also means `gpu_tail` must **not** enter any key: the rule the resampler
/// follows and HSL deliberately does not. Returning a *stage* rather than a *step* makes a mutated
/// key unrepresentable instead of merely tested.
///
/// ⚠⚠ **One stage per round trip, never chained.** Chaining the two prefix stages would pay the
/// transfer once — and destroy the post-NR checkpoint, which exists precisely so a Dehaze drag
/// resumes *after* noise reduction. Priced at 6.5 Mpx: per-step gives a Dehaze drag ~92 ms against
/// chaining's ~113, and a Dehaze drag is the case that matters, dehaze being used twice as often
/// as NR in this catalogue. ⚠ Chaining wins an **NR** drag (~113 against ~139), recorded rather
/// than hidden: it becomes the right answer if NR is ever ported *and* the checkpoint's value is
/// re-measured.
/// The facts a prefix stage needs that [`Edits`] cannot carry.
///
/// ⚠⚠ **Grouped when the third arrived**, because `too_many_arguments` fired at 8 and it was a fair
/// signal rather than a nuisance: three loose parameters threaded through six call sites is how one
/// of them comes to be passed wrongly, which is exactly the defect the `airlight` field fixes.
/// `pipeline::RenderSpec` is the same shape one layer up.
pub struct SeamFacts<'a> {
    /// ⚠ From the file's own metadata matched against the bundled database.
    pub lens: Option<&'a lens_profile::LensShot>,
    /// ⚠ A `RenderSpec` field — a property of the **render** rather than of the photograph, which
    /// is exactly why it is not an `ExportOptions` field either.
    pub resampler: pipeline::Resampler,
    /// The frame-global dehaze airlight, from `pipeline::prefix_airlight`.
    ///
    /// ⚠⚠ **Added 2026-09-17 for a defect found by use.** `GpuDehazeStage` estimated it from the
    /// image the seam hands over — which under a Straighten drag is the ROTATED frame — so the
    /// dehazed tone breathed as the angle moved. ⚠ Not an `Option`: a `None` arm would be a silent
    /// fallback to estimating, which is how it survived.
    pub airlight: [f32; 3],
    /// The photo's AI and depth maps (M10.16d2) — what Lens Blur's twin is built from, through the
    /// step's own lookup. `None` where a caller has none: the twin then declines, as the step
    /// itself is absent without its maps.
    pub ai: Option<&'a dyn pipeline::AiRasters>,
}

pub fn accelerate_prefix_stage(
    name: &str,
    edits: &Edits,
    scale: RenderScale,
    image: &core_image::ImageBuf,
    facts: SeamFacts<'_>,
    ctx: &Arc<GpuContext>,
) -> Option<GpuChain> {
    let SeamFacts {
        lens,
        resampler,
        airlight,
        ai,
    } = facts;
    // ⚠ Name-matched, exactly as `REPLACEABLE` is, and for the same reason: a `dyn PipelineStage`
    // exposes only `name()` and `apply()`, while the caller still holds the `Edits` it was built
    // from.
    let gpu: Box<dyn GpuStage> = match name {
        "dehaze" => Box::new(crate::GpuDehazeStage::new(
            &edits.dehaze,
            scale,
            airlight,
            ctx,
        )),
        // ⚠ Takes no image — noise reduction estimates nothing from the photograph, where dehaze's
        // airlight is a property of *this* frame. The buffer is handed over regardless, because
        // one signature serves both.
        "noise_reduction" => Box::new(crate::GpuNoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
            ctx,
        )),
        // ⚠⚠ **The only arm that can DECLINE, and both `?` are load-bearing.** A photo with no
        // matched profile, or a profile that yields no corrector, has nothing to accelerate — and
        // `None` here leaves the CPU stage in place, which is the seam's own rule: fail toward the
        // whole-buffer path, where a wrong answer costs milliseconds and never pixels.
        "lens_correction" => {
            let c = edits.applied_lens_correction();
            Box::new(crate::GpuLensCorrectionStage::new(
                lens?,
                lens_profile::Corrections {
                    distortion: c.distortion,
                    tca: c.tca,
                    vignetting: c.vignetting,
                    scale: c.scale,
                },
                resampler,
                image,
            )?)
        }
        // ⚠ Since 2026-09-26. Declines on a frame too small to resample, where the CPU stage
        // passes the image through untouched.
        "geometry" => Box::new(crate::GpuGeometryStage::new(
            &edits.geometry,
            edits.perspective,
            resampler,
            image,
        )?),
        // ⚠⚠ M10.16d2: built through `pipeline::lens_blur::inputs` — the lookup the CPU step was
        // keyed and built with — so the twin blurs with exactly its planes and focus.
        "lens_blur" => {
            let i = pipeline::lens_blur::inputs(&edits.lens_blur, ai?)?;
            Box::new(crate::GpuLensBlurStage::new(
                &edits.lens_blur,
                &i,
                image,
                ctx,
            )?)
        }
        _ => return None,
    };
    // ⚠ The same question the tail asks, and it fails toward the CPU for the same reason: a wrong
    // number costs milliseconds and never pixels.
    if !worth_a_chain(&[gpu.cpu_cost()]) {
        return None;
    }
    // ⚠⚠ **The concrete chain, not a `Box<dyn PipelineStage>`, and the difference is the point.**
    // `GpuChain` implements `PipelineStage`, so every caller's `.apply()` is unchanged — but a
    // boxed trait object hides `offer_output`/`allow_shared_input` from the ONE caller that knows
    // the step order, which is `render_cached`'s prefix loop. Hiding it there is what left the
    // prefix seams paying a full round trip per step.
    Some(GpuChain::new(ctx.clone(), vec![gpu]))
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
/// What the tail's accelerator needs besides the edits and the scale.
///
/// ⚠⚠ **Grouped rather than passed flat, because the eighth argument is where clippy draws the
/// line and it drew it on this function's sibling.** `accelerate_prefix_stage` reached 8 and
/// `too_many_arguments` fired, which was a fair signal; `SeamFacts` was the answer there and this
/// is the same answer here. Silencing the lint would have been the other option and a worse one.
pub struct TailFacts<'a> {
    /// The viewport region this render is scoped to, if any.
    pub region: Option<Region>,
    /// The camera rendering in force.
    pub rendering: TailRendering<'a>,
    /// What the render is producing.
    pub spec: RenderSpec,
}

pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    facts: TailFacts<'_>,
    ctx: &Arc<GpuContext>,
) -> (Pipeline, Option<GpuChain>) {
    let TailFacts {
        region,
        rendering,
        spec,
    } = facts;
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
            // ⚠ Carried for one rule rather than because anything reads it: `LocalAdjustments`
            // has no vignette, so no mask sub-stage can ask about the frame.
            region,
            rendering,
            spec,
            ctx,
            None,
            // ⚠⚠ **`Mask` only here.** A mask's first sub-stage is the one place in the workspace
            // that receives the very plane a coverage rasteriser was handed — see `mark_shared`.
            Some(crate::Slot::Mask),
        );
        rejoin(p, c).into_stages()
    };
    // ⚠ The cast is explicit: an unsize coercion from `&{closure}` to `&dyn Fn` inside a `Some(..)`
    // is not guaranteed to be inferred through the wrapper.
    walk(
        tail,
        edits,
        scale,
        region,
        rendering,
        spec,
        ctx,
        Some(&inner as &pipeline::SubStageAccelerator<'_>),
        // ⚠⚠ **`None`: the outer tail takes no plane (2026-09-26).** Its input is the resident
        // final, looked up by KEY in `PrefixCache`, so the anonymous slot is step→step only. The
        // one thing it must never take is a mask's leftover —
        // `the_outer_tails_own_chain_never_takes_a_masks_leftover_plane` pins that, and
        // `the_handed_back_chain_takes_no_plane` pins this arm.
        None,
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
    region: Option<Region>,
    rendering: TailRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
    inner: Option<&pipeline::SubStageAccelerator<'_>>,
    share_input: Option<crate::Slot>,
) -> (Pipeline, Option<GpuChain>) {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠⚠ **Both halves.** A run that turns out not to be worth the transfer has to be put back,
    // and `GpuStage::cpu()` hands out a *borrow* that cannot be pushed into `out` — so the
    // original travels beside the GPU stage and `flush` chooses between them.
    let mut run: Vec<Candidate> = Vec::new();
    // ⚠ Built once for the whole walk rather than per stage: it holds only an `Arc<GpuContext>`,
    // and a mask clones the `Arc` rather than the rasteriser.
    let coverage: Arc<dyn pipeline::CoverageRasterizer> = Arc::new(crate::GpuBrushRaster::new(ctx));
    let stages = tail.into_stages();
    // ⚠⚠ M10.14h: the reference photo on the GPU — decided for the whole tail before any stage is built, because it
    // is all or nothing (`reference_plan`).
    let plan = reference_plan(&stages, edits, scale, region, rendering, spec, ctx);
    for mut s in stages {
        // ⚠ Offered to **every** stage this walk sees, before the lookup. A replaceable stage has
        // no inner list and inherits the empty default, so one rule covers both arms rather than
        // one arm carrying an exception nobody would think to check.
        //
        // ⚠⚠ Both hooks are guarded by the same `inner.is_some()`, so the **recursive** call
        // offers neither. That is sound for the same reason the depth bound is: a mask's
        // sub-stages can never contain a mask, so they build no coverage and offering into them
        // would be machinery for a case that cannot arise.
        // ⚠⚠ **Captured BEFORE `accelerate_inner`, which rewrites the list into a `gpu_chain`.**
        // The GPU twin needs to know which sub-stages a mask carries, in order, and after the hook
        // has run `sub_stage_names` reports one name that says nothing about what is inside it.
        let mask_subs: Option<Vec<&'static str>> =
            (s.name() == "mask").then(|| s.sub_stage_names());
        if let Some(f) = inner {
            s.accelerate_inner(f);
            s.set_coverage_rasterizer(&coverage);
        }
        // ⚠⚠ **Deliberately AFTER the hooks, so a decline costs nothing.** If the twin cannot be
        // built, `s` goes on exactly as it did before this milestone — inner chain, coverage
        // rasteriser and all. Trying first and skipping the hooks would make every decline a
        // regression against M8.3j.
        //
        // ⚠ Not a `REPLACEABLE` row: that table's builders take an `&Edits`, a mask needs its own
        // `MaskedAdjustment`, and `the_replaceable_names_occur_at_most_once_in_a_tail` rests on
        // each name appearing once — while a tail legitimately holds several masks.
        if let Some(names) = mask_subs {
            let fed = plan.as_ref().map_or(&[][..], |p| &p.by_cpu[..]);
            if let Some(g) = gpu_mask(s.as_ref(), &names, scale, region, rendering, spec, ctx, fed)
            {
                run.push((g, s));
                continue;
            }
        }
        // ⚠ M10.12b: like a mask, not a `REPLACEABLE` row — its twin needs every contributing
        // mask's own parts (prebuilt maps included), which a builder taking `&Edits` cannot see. A
        // decline falls through to the CPU arm below, exactly as a mask's does.
        if let Some(so) = s.scene_offsets() {
            let twin = match &plan {
                Some(p) => reference_twins(so, edits, scale, region, rendering, spec, ctx)
                    .and_then(|t| {
                        crate::scene_offsets::GpuSceneOffsetsStage::with_reference(
                            so,
                            ctx,
                            t,
                            p.slots.clone(),
                        )
                    }),
                None => crate::scene_offsets::GpuSceneOffsetsStage::new(so, ctx),
            };
            if let Some(g) = twin {
                run.push((Box::new(g), s));
                continue;
            }
        }
        match REPLACEABLE
            .iter()
            .find(|(n, _)| *n == s.name())
            .and_then(|(_, build)| build(edits, scale, region, rendering, spec, ctx))
        {
            // ⚠ `and_then`, so a builder that declines falls through to the CPU arm below rather
            // than being treated as a replaceable stage with nothing behind it.
            Some(g) => run.push((g, s)),
            None => {
                // ⚠ A chain followed by a CPU stage is **not** the tail's last stage, so it goes
                // straight back into the pipeline and can never be a candidate for the byte path.
                if let Some(mut c) = flush(&mut run, &mut out, ctx) {
                    mark_shared(&mut c, share_input, out.is_empty());
                    // ⚠⚠ **Only before a MASK, and only a mask that declined.** A declining mask is
                    // a gating brush: it rasterises through `GpuBrushRaster`, whose gate needs the
                    // very bytes this chain just produced. Offering to any other CPU stage would
                    // deposit for a consumer that does not exist.
                    //
                    // ⚠ `s.name()` rather than `mask_subs`, which was consumed above.
                    if s.name() == "mask" {
                        c.offer_output(crate::Slot::Mask);
                    }
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
        //
        // ⚠ `mark_shared` here too, for the recursive walk's sake: a mask whose whole sub-list
        // is one chain lands in this arm.
        Some(mut c) if c.last_name() == Some("output_transform") => {
            mark_shared(&mut c, share_input, out.is_empty());
            (Pipeline::new(out), Some(c))
        }
        Some(mut c) => {
            mark_shared(&mut c, share_input, out.is_empty());
            out.push(Box::new(c));
            (Pipeline::new(out), None)
        }
        None => (Pipeline::new(out), None),
    }
}

/// Build a GPU twin for a mask, or decline.
///
/// ⚠⚠ **Every sub-stage must have a GPU twin, or the whole mask declines.** A `GpuMaskStage` runs
/// its sub-stages inside its own compute pass, so it has no way to drop back to the CPU for one of
/// them halfway through. That is not a limitation in practice: M8.3n ported the last of the ten,
/// so a mask's sub-stages are all replaceable today — and `a_mask_carrying_every_ported_stage...`
/// is what says so. If that ever stops being true, this declines and the CPU `MaskStage` runs,
/// which is the gate's own rule: fail toward the CPU.
///
/// ⚠ The sub-stage list is built from the **names the mask reported**, in order, through the same
/// `REPLACEABLE` table `walk` uses — so the GPU mask and the CPU mask cannot disagree about what a
/// mask contains or in what order.
#[allow(clippy::too_many_arguments)]
fn gpu_mask(
    s: &dyn PipelineStage,
    names: &[&'static str],
    scale: RenderScale,
    region: Option<Region>,
    rendering: TailRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
    fed: &[(usize, Arc<crate::scene_offsets::GpuCoverageSlot>)],
) -> Option<Box<dyn GpuStage>> {
    // ⚠⚠ M10.12b: a mask fed its coverage from the reference photo selects by the plane the offsets stage computed,
    // never its own base, which its own offsets have moved (the chase b2 removes). Since M10.14h its twin binds the
    // GPU plane — but only when the walk's plan pairs its slot with a GPU offsets stage; otherwise it declines and
    // its CPU stage reads the CPU slot.
    let feed = match s.reference_slot() {
        Some(cpu) => {
            let key = Arc::as_ptr(&cpu) as usize;
            let gpu = fed.iter().find(|(k, _)| *k == key)?.1.clone();
            Some((gpu, cpu))
        }
        None => None,
    };
    let (masked, geometry, mask_region) = s.mask_description()?;
    // ⚠ The mask's own adjustments projected onto an `Edits`, which is what a `REPLACEABLE`
    // builder reads. `sub_edits`' exhaustive destructure is what stops a field being forgotten.
    let e = sub_edits(&masked.adjustments);
    let mut subs: Vec<Box<dyn GpuStage>> = Vec::with_capacity(names.len());
    for n in names {
        let (_, build) = REPLACEABLE.iter().find(|(r, _)| r == n)?;
        subs.push(build(&e, scale, region, rendering, spec, ctx)?);
    }
    // ⚠ The whole per-component slice, base first — `BrushCoverage`'s own `[mask][component]`
    // shape, handed over as it stands rather than indexed here. Picking out index 0 is what this
    // did before the fold, and doing it here would have put the chain's ordering in two places.
    let twin = crate::GpuMaskStage::new(
        masked,
        geometry,
        mask_region,
        scale,
        spec.baseline,
        s.prebuilt_coverage(),
        subs,
        ctx,
    )?;
    Some(Box::new(match feed {
        Some((gpu, cpu)) => twin.fed_by(gpu, cpu),
        None => twin,
    }))
}

/// The reference photo on the GPU (M10.14h): the GPU slots to pair the offsets stage with the look stages of the
/// masks that read it.
struct ReferencePlan {
    /// One per contributing mask of the offsets stage, `Some` where it reads the reference.
    slots: Vec<Option<Arc<crate::scene_offsets::GpuCoverageSlot>>>,
    /// The same GPU slots keyed by their CPU slot's address — how a mask's look stage finds its own.
    by_cpu: Vec<(usize, Arc<crate::scene_offsets::GpuCoverageSlot>)>,
}

/// The plan, or `None` — then the offsets stage and those masks run on the CPU, exactly as before M10.14h.
///
/// ⚠⚠ **All or nothing, and that is the correctness of it, not caution.** The GPU offsets stage fills only GPU
/// slots; a mask left on the CPU reads its CPU slot, which would then be empty and select on its own base. So the
/// plan exists only when every stage from the offsets stage to the last mask reading it gets a twin — the whole
/// stretch is one run of GPU stages, so one chain and one submission, the slot filled before it is read.
#[allow(clippy::too_many_arguments)]
fn reference_plan(
    stages: &[Box<dyn PipelineStage>],
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    rendering: TailRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> Option<ReferencePlan> {
    let at = stages
        .iter()
        .position(|s| s.scene_offsets().is_some_and(|o| o.has_reference()))?;
    let so = stages[at].scene_offsets()?;
    let slots: Vec<Option<Arc<crate::scene_offsets::GpuCoverageSlot>>> = so
        .slots()
        .map(|c| c.map(|_| Arc::new(Default::default())))
        .collect();
    let by_cpu: Vec<(usize, Arc<crate::scene_offsets::GpuCoverageSlot>)> = so
        .slots()
        .zip(&slots)
        .filter_map(|(c, g)| Some((Arc::as_ptr(c?) as usize, g.clone()?)))
        .collect();
    // The offsets stage itself, with its reference.
    let twins = reference_twins(so, edits, scale, region, rendering, spec, ctx)?;
    crate::scene_offsets::GpuSceneOffsetsStage::with_reference(so, ctx, twins, slots.clone())?;
    let reads = |s: &dyn PipelineStage| {
        s.reference_slot()
            .is_some_and(|c| by_cpu.iter().any(|(k, _)| *k == Arc::as_ptr(&c) as usize))
    };
    let last = stages.iter().rposition(|s| reads(s.as_ref())).unwrap_or(at);
    for s in &stages[at + 1..=last] {
        let twinned = if s.name() == "mask" {
            let names = s.sub_stage_names();
            gpu_mask(
                s.as_ref(),
                &names,
                scale,
                region,
                rendering,
                spec,
                ctx,
                &by_cpu,
            )
            .is_some()
        } else {
            REPLACEABLE
                .iter()
                .find(|(n, _)| *n == s.name())
                .and_then(|(_, build)| build(edits, scale, region, rendering, spec, ctx))
                .is_some()
        };
        if !twinned {
            return None;
        }
    }
    Some(ReferencePlan { slots, by_cpu })
}

/// The GPU twins of the offsets stage's reference — the global look, by the names the CPU stage reports, through the
/// same `REPLACEABLE` builders the tail's own stages use. `None` if any has no twin.
#[allow(clippy::too_many_arguments)]
fn reference_twins(
    so: &pipeline::SceneOffsetsStage,
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    rendering: TailRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> Option<Vec<Box<dyn GpuStage>>> {
    so.reference_names()
        .iter()
        .map(|n| {
            REPLACEABLE
                .iter()
                .find(|(r, _)| r == n)
                .and_then(|(_, build)| build(edits, scale, region, rendering, spec, ctx))
        })
        .collect()
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
fn mark_shared(c: &mut GpuChain, share_input: Option<crate::Slot>, is_first: bool) {
    if let (Some(slot), true) = (share_input, is_first) {
        c.allow_shared_input(slot);
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
        let (_, tail) = pipeline::build_pipeline_split_in_region(
            &e,
            SCALE,
            None,
            pipeline::TailRendering::colorimetric(),
        );
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
            TailRendering::default(),
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
        let ot = GpuOutputTransformStage::new(
            RenderSpec::default().space,
            RenderSpec::default().intent,
            &ctx,
        );
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
        let (_, tail) = pipeline::build_pipeline_split_in_region(
            &e,
            SCALE,
            None,
            pipeline::TailRendering::colorimetric(),
        );
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
            TailRendering::default(),
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

    /// ⚠⚠ **The intent must reach the GPU's output transform (M6.4c).** `GPU_STAGES` builds it from
    /// `spec.intent`; a site passing the default instead compiles perfectly and would make the
    /// preview disagree with the export on every saturated colour. The fixture is saturated and
    /// shown first to render differently under the two intents on the CPU.
    #[test]
    fn the_rendering_intent_reaches_the_gpu_tail() {
        let Some(ctx) = ctx() else { return };
        let e = Edits::default();
        let mut px = Vec::new();
        for k in 0..16 {
            let t = k as f32 / 15.0;
            px.extend_from_slice(&[0.05 + 0.9 * t, 0.85 - 0.8 * t, 0.1]);
        }
        let img = core_image::ImageBuf::from_vec(16, 1, px);
        let tail = |intent| {
            let spec = RenderSpec {
                intent,
                ..RenderSpec::default()
            };
            let (_, t) = pipeline::build_pipeline_split_in_region_with_brush(
                &e,
                SCALE,
                None,
                None,
                spec,
                None,
                TailRendering::colorimetric(),
            );
            (t, spec)
        };
        let run = |intent, gpu: bool| {
            let (t, spec) = tail(intent);
            if gpu {
                accel(t, &e, SCALE, TailRendering::colorimetric(), spec, &ctx).run(img.clone())
            } else {
                t.run(img.clone())
            }
        };
        let (rel, per) = (
            color::RenderingIntent::RelativeColorimetric,
            color::RenderingIntent::Perceptual,
        );
        assert!(
            crate::compare(&run(rel, false), &run(per, false)).worst_levels > 2.0,
            "the fixture must tell the intents apart"
        );
        for intent in [rel, per] {
            let d = crate::compare(&run(intent, false), &run(intent, true));
            assert!(
                d.worst_levels < 0.5,
                "{intent:?}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠⚠ **The preview-versus-export guard for the per-channel curves, and it closes a gap I had
    /// written off as "only checkable by use".** `a_colorimetric_frame_renders_the_same_picture_on_the_gpu`
    /// above runs on `Edits::default()`, so it cannot see a construction site in `GPU_STAGES` that passed
    /// only the composite curve — and that site compiles perfectly. With the GPU tail on by default, the
    /// symptom would be a **preview that disagrees with the export**, which is this project's worst
    /// failure shape and one it has already paid for three times in a single milestone.
    ///
    /// ⚠ Three **different** channel curves, and the fixture is first shown to differ from neutral — an
    /// accelerated chain and a CPU one agree trivially on a render that does nothing.
    #[test]
    fn a_frame_with_channel_curves_renders_the_same_picture_on_the_gpu() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            tone_curve_red: edit_model::ToneCurve {
                points: vec![[0.0, 0.18], [1.0, 1.0]],
            },
            tone_curve_green: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.58], [1.0, 1.0]],
            },
            tone_curve_blue: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [1.0, 0.82]],
            },
            // ⚠ M5.1c: the parametric curve too, since it is folded into the same composite table
            // the GPU twin uploads. A construction site that skipped it would disagree here.
            tone_curve_parametric: edit_model::ParametricCurve {
                shadows: 40.0,
                highlights: -40.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let cpu = tail_of(&e).run(scene_ramp());

        // ⚠ The fixture must actually do something, or the comparison below is vacuous.
        let neutral = tail_of(&Edits::default()).run(scene_ramp());
        let moved = crate::compare(&cpu, &neutral);
        assert!(
            moved.worst_levels > 1.0,
            "the channel curves changed nothing, so this would guard nothing: {moved:?}"
        );

        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(scene_ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "the GPU tail and the CPU disagree on the channel curves — a preview that would not \
             match its export: worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ **The guard on the projection, and the only thing that can see a mis-mapped field.**
    /// Every projected field is non-default here, so a projection that forgot one — or carried the
    /// *global* value instead of the mask's — renders a different photograph. A fixture setting
    /// only `hsl` would be blind to every other.
    ///
    /// ⚠⚠ **THIS FIXTURE WENT STALE ONCE AND THE TEST KEPT PASSING.** Its doc used to read
    /// *"every one of the eight projected fields is non-default here"*, which was true when
    /// written — and M8.3m grew the projected set to twelve without touching the fixture, so four
    /// stages sat silently inert inside every mask and this test reported green. **A guard that
    /// enumerates a list stops guarding the moment the list grows**, which is the masked-grade
    /// milestone's sharpest finding arriving in a new place. ⚠ The mitigation is the assertion
    /// below: the fixture must leave nothing at its default but the two prefix stages, and that is
    /// checked rather than trusted.
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
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4200.0),
                tint: 8.0,
            },
            contrast: 28.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.4, 0.52], [1.0, 1.0]],
            },
            saturation: 22.0,
            vibrance: 16.0,
            local_contrast: edit_model::LocalContrast {
                texture: 45.0,
                clarity: 35.0,
            },
            // ⚠ The only two left at their defaults, and they must be: noise reduction and dehaze
            // run in the PREFIX and can never appear in a mask's tail sub-stage list.
            ..Default::default()
        };
        // ⚠⚠ **The check that stops this fixture going stale again.** Anything added to
        // `REPLACEABLE` must be carried by `sub_edits`, and `sub_edits` reads these fields — so a
        // field left at its default here is a field this test cannot see.
        let carried = sub_edits(&adjustments);
        assert_ne!(
            carried,
            Edits::default(),
            "fixture check: the projection must carry something"
        );
        for (name, same) in [
            ("exposure", carried.exposure == 0.0),
            ("whites", carried.whites == 0.0),
            ("white_balance", carried.white_balance.kelvin.is_none()),
            ("contrast", carried.contrast == 0.0),
            ("tone_curve", carried.tone_curve.points.is_empty()),
            ("saturation", carried.saturation == 0.0),
            ("vibrance", carried.vibrance == 0.0),
            ("local_contrast", !carried.local_contrast.is_active()),
            ("hsl", carried.hsl == Edits::default().hsl),
            (
                "color_grading",
                carried.color_grading == Edits::default().color_grading,
            ),
            ("sharpening", carried.sharpening.amount == 0.0),
        ] {
            assert!(
                !same,
                "fixture check: `{name}` reached the projection at its default, so this test is \
                 blind to it — either the fixture or `sub_edits` is stale"
            );
        }
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
            TailRendering::default(),
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

    /// A hazy fixture big enough for the guided reach (twice its radius).
    /// ⚠ Bilinear, so the seam's own tests exercise the branch whose weight table is **empty**.
    /// Every other kernel reads the uploaded LUT; this one is computed exactly, and a shader that
    /// only ever read the table would agree on three kernels and render black on the fourth.
    const RES: pipeline::Resampler = pipeline::Resampler::Bilinear;

    /// The synthetic lens the CPU stage's own tests use, so the two are comparable.
    ///
    /// ⚠ Copied rather than shared: it lives in `pipeline`'s test module, which is another crate,
    /// and exporting a fixture across a crate boundary is more machinery than eight lines is worth.
    fn lens_fixture() -> lens_profile::LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                    <vignetting model="pa" focal="50" aperture="2.8" distance="10"
                        k1="-0.3" k2="0.0" k3="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        lens_profile::LensShot {
            profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    fn lens_edits() -> Edits {
        Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                scale: 0.0,
            }),
            ..Edits::default()
        }
    }

    fn hazy(w: usize, h: usize) -> core_image::ImageBuf {
        let a = [0.85f32, 0.87, 0.9];
        let mut img = core_image::ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let dark = x % 40 < 6 && y % 40 < 6;
                let clear = if dark {
                    [0.01, 0.01, 0.01]
                } else {
                    [0.05 + 0.7 * u, 0.05 + 0.6 * v, 0.05 + 0.5 * (1.0 - u)]
                };
                let t = 0.95 - 0.9 * u;
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = clear[c] * t + a[c] * (1.0 - t);
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn hazed_edits(amount: f32) -> Edits {
        Edits {
            dehaze: edit_model::Dehaze { amount },
            ..Edits::default()
        }
    }

    /// ⚠ The seam exists and fires. Without this every assertion below could pass against a
    /// function nothing calls — the W5 failure mode.
    #[test]
    fn a_dehaze_prefix_step_becomes_a_chain() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let g = accelerate_prefix_stage(
            "dehaze",
            &hazed_edits(60.0),
            SCALE,
            &img,
            SeamFacts {
                lens: None,
                resampler: RES,
                airlight: pipeline::airlight(&img),
                ai: None,
            },
            &ctx,
        )
        .expect("dehaze must be replaceable");
        assert_eq!(g.name(), "gpu_chain");
    }

    /// ⚠⚠ **The guard for the SEAM, and it did not exist until 2026-09-17 — which is how the defect
    /// shipped.** `GpuDehazeStage` estimated its own airlight from the image the seam handed over,
    /// which under a Straighten drag is the ROTATED frame, so the dehazed tone breathed as the
    /// angle moved. Found by use, with the GPU on.
    ///
    /// ⚠ Every other test here builds `GpuDehazeStage` directly, so **re-introducing the defect at
    /// the seam failed none of them** — verified by sabotage. A stage-level guard cannot see a
    /// caller that passes the wrong argument.
    ///
    /// ⚠ Observed through the PICTURE rather than the field: the seam returns a
    /// `Box<dyn PipelineStage>`, which exposes `name()` and `apply()` and nothing else.
    #[test]
    fn the_seam_hands_dehaze_the_supplied_airlight() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let e = hazed_edits(60.0);
        let run = |air: [f32; 3]| {
            accelerate_prefix_stage(
                "dehaze",
                &e,
                SCALE,
                &img,
                SeamFacts {
                    lens: None,
                    resampler: RES,
                    airlight: air,
                    ai: None,
                },
                &ctx,
            )
            .expect("dehaze must be replaceable")
            .apply(img.clone())
        };
        let hi = run([0.9; 3]);
        let lo = run([0.5; 3]);
        assert_ne!(
            hi.data(),
            lo.data(),
            "the seam must pass the supplied airlight through to the shader"
        );
    }

    fn denoised_edits(luminance: f32, color: f32) -> Edits {
        Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance,
                color,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        }
    }

    /// ⚠ The seam's second arm exists and fires. Without this the whole milestone could be fully
    /// tested and never reached — the W5 failure mode, which this crate has now recorded twice.
    #[test]
    fn a_noise_reduction_prefix_step_becomes_a_chain() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let g = accelerate_prefix_stage(
            "noise_reduction",
            &denoised_edits(60.0, 60.0),
            SCALE,
            &img,
            SeamFacts {
                lens: None,
                resampler: RES,
                airlight: pipeline::airlight(&img),
                ai: None,
            },
            &ctx,
        )
        .expect("noise reduction must be replaceable");
        assert_eq!(g.name(), "gpu_chain");
    }

    /// ⚠ The picture must be the same however the step ran — that is what makes a checkpoint
    /// written by either path interchangeable rather than merely equal in its key.
    #[test]
    fn an_accelerated_noise_reduction_step_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let e = denoised_edits(70.0, 70.0);
        let img = hazy(200, 160);
        let steps = pipeline::build_prefix_steps(
            &e,
            SCALE,
            None,
            pipeline::Resampler::Bilinear,
            None,
            None,
        );
        let step = steps
            .iter()
            .find(|s| s.stage.name() == "noise_reduction")
            .expect("fixture check: there must be a noise_reduction step");
        let cpu = step.stage.apply(img.clone());
        let gpu = accelerate_prefix_stage(
            "noise_reduction",
            &e,
            SCALE,
            &img,
            SeamFacts {
                lens: None,
                resampler: RES,
                airlight: pipeline::airlight(&img),
                ai: None,
            },
            &ctx,
        )
        .expect("noise reduction must be replaceable")
        .apply(img.clone());
        assert!(
            !crate::compare(&img, &cpu).is_exact(),
            "fixture check: the step must actually change the picture"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ **A FORCING FUNCTION, not a coverage test.** Naming the prefix stages with no GPU twin
    /// is what makes adding one a deliberate act: this test breaks, and whoever breaks it has to
    /// say why. It broke for noise reduction, for lens correction, and (2026-09-26) for geometry.
    ///
    /// ⚠ Geometry was left out on the argument that eight cores had made it cheap (M8.5: ~160 ms
    /// to ~34). That was true of its COMPUTE and missed its TRANSFERS: on a fitted rung of 6.8 Mpx
    /// a Straighten frame was 63 ms, and the win came from keeping the pixels on the GPU — the
    /// resident input and the no-download handoff — which only a twin makes possible. See
    /// `docs/superpowers/specs/2026-09-26-gpu-geometry-design.md`.
    #[test]
    fn every_other_prefix_stage_is_left_alone() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(64, 64);
        // ⚠ Geometry has a twin since 2026-09-26. Spot removal is here because it was measured
        // and closed unported (`docs/BACKLOG.md`), and a name nothing implements must fall through
        // too.
        // Red eye (M9.11) has no twin either: a few thousand pixels per eye in the prefix.
        for name in ["spot_removal", "red_eye", "no_such_stage"] {
            assert!(
                accelerate_prefix_stage(
                    name,
                    &hazed_edits(60.0),
                    SCALE,
                    &img,
                    SeamFacts {
                        lens: None,
                        resampler: RES,
                        airlight: pipeline::airlight(&img),
                        ai: None,
                    },
                    &ctx
                )
                .is_none(),
                "`{name}` has no GPU twin and must be left alone"
            );
        }
    }

    /// M10.16d2: Lens Blur's maps as the seam is handed them — depth ramping left to right, an empty
    /// subject, the focus at 0.3.
    struct LensPlanes;
    impl pipeline::AiRasters for LensPlanes {
        fn source(
            &self,
            _: &edit_model::Mask,
        ) -> Option<(std::sync::Arc<dyn pipeline::RasterSource>, u64)> {
            None
        }
        fn plane(
            &self,
            which: pipeline::MapPlane,
        ) -> Option<(std::sync::Arc<dyn pipeline::RasterSource>, u64)> {
            Some(match which {
                pipeline::MapPlane::Depth => (std::sync::Arc::new(Ramp(true)), 1),
                pipeline::MapPlane::Subject => (std::sync::Arc::new(Ramp(false)), 2),
            })
        }
        fn lens_focus(&self, _: &edit_model::LensBlur) -> Option<f32> {
            Some(0.3)
        }
    }

    /// `true`: 0 at the left to 255 at the right; `false`: 0 everywhere.
    struct Ramp(bool);
    impl pipeline::RasterSource for Ramp {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            pipeline::brush::CoverageMap {
                x0: 0,
                y0: 0,
                w,
                h,
                data: (0..w * h)
                    .map(|i| {
                        if self.0 {
                            ((i % w) * 255 / (w - 1).max(1)) as u8
                        } else {
                            0
                        }
                    })
                    .collect(),
                outside: 0,
            }
        }
    }

    fn blurred_edits() -> Edits {
        Edits {
            lens_blur: edit_model::LensBlur {
                on: true,
                amount: 100.0,
                boost: 40.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    /// ⚠⚠ Both halves: without its maps the seam leaves the step alone; with them it fires.
    #[test]
    fn lens_blur_is_accelerated_only_with_its_maps() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let e = blurred_edits();
        let seam = |ai: Option<&dyn pipeline::AiRasters>| {
            accelerate_prefix_stage(
                "lens_blur",
                &e,
                SCALE,
                &img,
                SeamFacts {
                    lens: None,
                    resampler: RES,
                    airlight: pipeline::airlight(&img),
                    ai,
                },
                &ctx,
            )
        };
        assert!(
            seam(None).is_none(),
            "without its maps there is nothing to accelerate"
        );
        assert!(
            seam(Some(&LensPlanes)).is_some(),
            "with them the seam must fire"
        );
    }

    /// The seam's chain renders the prefix's own CPU step's picture.
    #[test]
    fn an_accelerated_lens_blur_step_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let e = blurred_edits();
        let img = hazy(200, 160);
        let steps = pipeline::build_prefix_steps(&e, SCALE, None, RES, None, Some(&LensPlanes));
        let step = steps
            .iter()
            .find(|s| s.stage.name() == "lens_blur")
            .expect("fixture check: there must be a lens blur step");
        let cpu = step.stage.apply(img.clone());
        let gpu = accelerate_prefix_stage(
            "lens_blur",
            &e,
            SCALE,
            &img,
            SeamFacts {
                lens: None,
                resampler: RES,
                airlight: pipeline::airlight(&img),
                ai: Some(&LensPlanes),
            },
            &ctx,
        )
        .expect("lens blur must be replaceable")
        .apply(img.clone());
        assert!(
            !crate::compare(&img, &cpu).is_exact(),
            "fixture check: the step must actually change the picture"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ **BOTH halves, because either alone passes against a bug.** An arm that never fires and
    /// one that fires unconditionally are equally wrong, and only the pair distinguishes them.
    #[test]
    fn lens_correction_is_accelerated_only_when_a_profile_is_in_hand() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let e = lens_edits();
        assert!(
            accelerate_prefix_stage(
                "lens_correction",
                &e,
                SCALE,
                &img,
                SeamFacts {
                    lens: None,
                    resampler: RES,
                    airlight: pipeline::airlight(&img),
                    ai: None,
                },
                &ctx
            )
            .is_none(),
            "with no matched profile there is nothing to accelerate"
        );
        let lens = lens_fixture();
        assert!(
            accelerate_prefix_stage(
                "lens_correction",
                &e,
                SCALE,
                &img,
                SeamFacts {
                    lens: Some(&lens),
                    resampler: RES,
                    airlight: pipeline::airlight(&img),
                    ai: None,
                },
                &ctx
            )
            .is_some(),
            "with a profile the seam must fire"
        );
    }

    /// ⚠⚠ **THE CACHE-COMPATIBILITY GUARD, and it is the reason this seam is safe at all.** The
    /// substitution returns a **stage**, never a step, so a mutated `PrefixStepKey` is
    /// unrepresentable rather than merely tested — and a checkpoint written by one path is a
    /// legitimate hit for the other. This asserts the premise that rests on: the key is a function
    /// of the **edits**, so building the steps twice for the same edits gives equal keys whatever
    /// is done to the stages.
    #[test]
    fn a_prefix_key_is_a_function_of_the_edits_alone() {
        let e = hazed_edits(60.0);
        let a = pipeline::build_prefix_steps(
            &e,
            SCALE,
            None,
            pipeline::Resampler::Bilinear,
            None,
            None,
        );
        let b = pipeline::build_prefix_steps(
            &e,
            SCALE,
            None,
            pipeline::Resampler::Bilinear,
            None,
            None,
        );
        assert!(!a.is_empty(), "fixture check: dehaze must produce a step");
        let ka: Vec<_> = (0..=a.len())
            .map(|n| pipeline::PrefixKey::upto(&a, n, SCALE))
            .collect();
        let kb: Vec<_> = (0..=b.len())
            .map(|n| pipeline::PrefixKey::upto(&b, n, SCALE))
            .collect();
        assert_eq!(
            ka, kb,
            "a prefix key must not depend on which stage is inside"
        );
    }

    /// ⚠ The picture must be the same however the step ran — that is what makes a checkpoint
    /// written by either path interchangeable rather than merely equal in its key.
    #[test]
    fn an_accelerated_prefix_step_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let e = hazed_edits(70.0);
        let img = hazy(200, 160);
        let steps = pipeline::build_prefix_steps(
            &e,
            SCALE,
            None,
            pipeline::Resampler::Bilinear,
            None,
            None,
        );
        let step = steps
            .iter()
            .find(|s| s.stage.name() == "dehaze")
            .expect("fixture check: there must be a dehaze step");
        let cpu = step.stage.apply(img.clone());
        let gpu = accelerate_prefix_stage(
            "dehaze",
            &e,
            SCALE,
            &img,
            SeamFacts {
                lens: None,
                resampler: RES,
                airlight: pipeline::airlight(&img),
                ai: None,
            },
            &ctx,
        )
        .expect("dehaze must be replaceable")
        .apply(img.clone());
        assert!(
            !crate::compare(&img, &cpu).is_exact(),
            "fixture check: the step must actually change the picture"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// A prefix of two GPU-able steps, as `render_cached`'s loop builds it.
    ///
    /// ⚠⚠ **Two, because one has no step-to-step seam at all.** A fixture with a single
    /// accelerated prefix step measures the floor, which is the error the probe for this milestone
    /// made first and had to be re-run for.
    fn two_step_prefix_edits() -> Edits {
        let mut e = Edits {
            dehaze: edit_model::Dehaze { amount: 45.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                detail: 50.0,
                color: 35.0,
                smoothness: 50.0,
            },
            ..Default::default()
        };
        e.exposure = 0.4;
        e.contrast = 25.0;
        e.saturation = 20.0;
        e
    }

    /// Run the prefix steps, optionally with the resident seam wired.
    ///
    /// Returns the prefix output and **how many uploads each accelerated step performed**.
    ///
    /// ⚠⚠ **The upload count, not `SharedPlane::takes()`, and finding that out took a sabotage.**
    /// A take that is then *discarded* still increments `takes()` — so a version of `run_with` that
    /// called `take` and threw the plane away read as `takes() == 1` while uploading anyway, and
    /// **every test here stayed green**: the picture is correct on that path, because uploading is
    /// what it fell back to. `transfer_counts().0 == 0` is the only observable that separates
    /// *took and used it* from *took and dropped it*. This is the W5 failure mode, caught by
    /// sabotage rather than by review.
    fn run_prefix(
        e: &Edits,
        ctx: &Arc<GpuContext>,
        resident: bool,
        img: core_image::ImageBuf,
    ) -> (core_image::ImageBuf, Vec<usize>) {
        let airlight = pipeline::prefix_airlight(e, &img, pipeline::Resampler::default(), None);
        let steps = pipeline::build_prefix_steps(
            e,
            SCALE,
            None,
            pipeline::Resampler::default(),
            Some(airlight),
            None,
        );
        let mut out = img;
        let mut uploads: Vec<usize> = Vec::new();
        for step in &steps {
            let g = accelerate_prefix_stage(
                step.stage.name(),
                e,
                SCALE,
                &out,
                SeamFacts {
                    lens: None,
                    resampler: pipeline::Resampler::default(),
                    airlight,
                    ai: None,
                },
                ctx,
            );
            out = match g {
                Some(mut g) => {
                    if resident {
                        g.allow_shared_input(crate::Slot::Prefix);
                        g.offer_output(crate::Slot::Prefix);
                    }
                    let done = g.apply(out);
                    uploads.push(g.transfer_counts().0);
                    done
                }
                None => {
                    ctx.prefix_plane().clear();
                    step.stage.apply(out)
                }
            };
        }
        assert_eq!(
            uploads.len(),
            2,
            "fixture check: both prefix steps must be accelerated, or this proves nothing"
        );
        (out, uploads)
    }

    /// ⚠⚠ **THE test of the resident prefix seam: the same picture, one fewer round trip.**
    /// Every other assertion about this hand-off is a counter, and a counter cannot tell a correct
    /// plane from a plausible one.
    #[test]
    fn a_resident_prefix_seam_renders_the_same_picture() {
        let Some(ctx) = private_ctx() else { return };
        let e = two_step_prefix_edits();
        let (plain, plain_uploads) = run_prefix(&e, &ctx, false, big_scene());
        assert_eq!(
            plain_uploads,
            vec![1, 1],
            "fixture check: unwired, every step uploads its own input"
        );
        let (resident, resident_uploads) = run_prefix(&e, &ctx, true, big_scene());
        // ⚠⚠ **This is the claim, and it is the one a counter cannot make.** The first step has
        // nothing to take and uploads; the second takes and uploads **nothing at all**. A `takes()`
        // assertion passes even when the plane is taken and thrown away — see `run_prefix`.
        assert_eq!(
            resident_uploads,
            vec![1, 0],
            "the second step must upload NOTHING — it was handed the first's buffer"
        );
        // ⚠ The band is measured at both ends rather than borrowed: this is a same-shader,
        // same-input comparison where only the buffer's provenance differs, so it is EXACT. M8.13's
        // 0.088 came from comparing a GPU chain against the CPU, which is a different question.
        let d = crate::compare(&plain, &resident);
        assert!(
            d.is_exact(),
            "a taken plane must be the bytes the upload would have carried: {d:?}"
        );
    }

    /// ⚠⚠ **The slots must not be cross-wired, and nothing else in the workspace would notice.**
    /// A chain allowed on one slot that read the other would still render *a* photograph — the
    /// wrong one — and in a fixture where only one slot is ever occupied it would simply upload
    /// and every equivalence test would stay green. This asserts the refusal directly.
    ///
    /// ⚠ Sabotage: make `GpuContext::plane` return `shared_plane` for both arms.
    #[test]
    fn a_chain_allowed_on_one_slot_never_takes_from_the_other() {
        let Some(ctx) = private_ctx() else { return };
        let scene = big_scene();
        let (w, h) = (scene.width(), scene.height());
        for (offered, allowed) in [
            (crate::Slot::Mask, crate::Slot::Prefix),
            (crate::Slot::Prefix, crate::Slot::Mask),
        ] {
            ctx.plane(crate::Slot::Mask).clear();
            ctx.plane(crate::Slot::Prefix).clear();
            let before = ctx.plane(allowed).takes();
            let plane = crate::GpuImage::new(&ctx, w, h).expect("a plane");
            ctx.plane(offered).deposit(w, h, plane);

            let mut c = GpuChain::new(
                ctx.clone(),
                vec![Box::new(GpuExposureStage::new(0.5, ctx.clone())) as Box<dyn GpuStage>],
            );
            c.allow_shared_input(allowed);
            let _ = c.apply(scene.clone());
            assert_eq!(
                ctx.plane(allowed).takes(),
                before,
                "a chain on {allowed:?} must not take a plane deposited in {offered:?}"
            );
            assert_eq!(
                c.transfer_counts().0,
                1,
                "and it must have uploaded instead: {offered:?} -> {allowed:?}"
            );
        }
    }

    /// ⚠⚠ **A cleared slot must be untakeable, and this pins the primitive the loop leans on.**
    /// `render_cached`'s prefix loop clears whenever a step runs on the CPU, because the parked
    /// plane is then no longer the next step's input — same dimensions, same thread, **a plausible
    /// photograph**.
    ///
    /// ⚠ The `[GPU, CPU, GPU]` arrangement that makes the mid-loop clear *live* needs a matched
    /// lens: the prefix order is lens correction, perspective, spot removal, geometry, noise
    /// reduction, dehaze, and the only twins are lens correction, NR and dehaze — so without a
    /// profile every CPU step falls *before* both GPU ones and nothing can sit between them. This
    /// test therefore pins the primitive and the reachability is reasoned, which is stated here
    /// rather than left for a reader to assume it was tested.
    #[test]
    fn a_cleared_prefix_slot_is_not_taken() {
        let Some(ctx) = private_ctx() else { return };
        let scene = big_scene();
        let (w, h) = (scene.width(), scene.height());
        let plane = crate::GpuImage::new(&ctx, w, h).expect("a plane");
        ctx.prefix_plane().deposit(w, h, plane);
        ctx.prefix_plane().clear();

        let mut c = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(0.5, ctx.clone())) as Box<dyn GpuStage>],
        );
        c.allow_shared_input(crate::Slot::Prefix);
        let _ = c.apply(scene);
        assert_eq!(
            ctx.prefix_plane().takes(),
            0,
            "a cleared slot has nothing to give"
        );
        assert_eq!(c.transfer_counts().0, 1, "so the chain must upload");
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
        rendering: TailRendering<'_>,
        spec: RenderSpec,
        ctx: &Arc<GpuContext>,
    ) -> Pipeline {
        let (p, c) = accelerate(
            tail,
            e,
            scale,
            TailFacts {
                region: None,
                rendering,
                spec,
            },
            ctx,
        );
        rejoin(p, c)
    }

    fn split(tail: Pipeline, e: &Edits, ctx: &Arc<GpuContext>) -> (Pipeline, Option<GpuChain>) {
        accelerate(
            tail,
            e,
            SCALE,
            TailFacts {
                region: None,
                rendering: TailRendering::default(),
                spec: RenderSpec::default(),
            },
            ctx,
        )
    }

    /// ⚠⚠ **The outer tail's chain takes NO plane — since the resident final (2026-09-26).** Its
    /// input comes from `PrefixCache::resident_final` under a key, which is what makes a stale
    /// hit unrepresentable; the anonymous prefix slot is step→step only. A tail offered
    /// `Slot::Prefix` would take a step's leftover at matching dimensions and render a plausible
    /// photograph — the sabotage this pins is `Some(crate::Slot::Prefix)` in `accelerate`'s
    /// outer walk.
    #[test]
    fn the_handed_back_chain_takes_no_plane() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let (_, chain) = accelerate(
            tail_of(&e),
            &e,
            SCALE,
            TailFacts {
                region: None,
                rendering: TailRendering::default(),
                spec: RenderSpec::default(),
            },
            &ctx,
        );
        let chain = chain.expect("fixture check: this tail ends in a chain");
        assert_eq!(chain.takes_from(), None);
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
    ///
    /// ⚠⚠ **THIS TEST ASSERTED THE M8.3m BUG WAS CORRECT BEHAVIOUR, AND THAT IS THE SHARPEST
    /// FINDING OF M8.3n.** Its fixture was `contrast`, `saturation` and `vibrance` — the exact
    /// fields whose `REPLACEABLE` rows M8.3m had just added — so it demanded that a projection
    /// *drop* three values a GPU builder now reads. It passed, loudly and wrongly, and its name
    /// said the opposite of what it checked.
    ///
    /// ⚠ Rebuilt against the property that is actually stable: the only things a projection must
    /// never carry are the two stages that **cannot reach a mask's tail at all** — noise reduction
    /// and dehaze run in the prefix — and `masks` themselves, since `LocalAdjustments` has no
    /// `masks` field and a mask can never contain a mask. Those cannot go stale as `REPLACEABLE`
    /// grows, where an enumerated list of ignored fields provably can.
    #[test]
    fn the_projection_carries_nothing_a_gpu_builder_does_not_read() {
        let a = edit_model::LocalAdjustments {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 55.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 45.0 },
            ..Default::default()
        };
        let e = sub_edits(&a);
        let d = Edits::default();
        assert_eq!(
            e.noise_reduction, d.noise_reduction,
            "noise reduction runs in the PREFIX and can never be a mask's tail sub-stage"
        );
        assert_eq!(
            e.dehaze, d.dehaze,
            "dehaze runs in the PREFIX and can never be a mask's tail sub-stage"
        );
        assert_eq!(e.masks.len(), 0, "a projection must never carry masks");
    }

    fn tail_of(e: &Edits) -> Pipeline {
        pipeline::build_pipeline_split(e, SCALE, pipeline::TailRendering::colorimetric()).1
    }

    fn tail_in_region(e: &Edits, region: Option<Region>) -> Pipeline {
        pipeline::build_pipeline_split_in_region(
            e,
            SCALE,
            region,
            pipeline::TailRendering::colorimetric(),
        )
        .1
    }

    /// ⚠⚠ Every per-pixel stage active at once — a channel curve, a parametric curve, the camera
    /// rendering and a region for the vignette — renders on the GPU within a bound of the CPU,
    /// and the whole tail is ONE fused dispatch. First asserted to move pixels.
    #[test]
    fn the_whole_per_pixel_tail_fuses_and_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        let mut e = graded(with_hsl());
        e.white_balance = edit_model::WhiteBalance {
            kelvin: Some(4800.0),
            tint: -6.0,
        };
        e.exposure = 0.5;
        e.contrast = 20.0;
        e.highlights = -25.0;
        e.shadows = 30.0;
        e.whites = 10.0;
        e.blacks = -10.0;
        e.tone_curve.points = vec![[0.0, 0.0], [0.45, 0.5], [1.0, 1.0]];
        e.tone_curve_red.points = vec![[0.0, 0.0], [0.5, 0.47], [1.0, 1.0]];
        e.tone_curve_parametric.highlights = 20.0;
        e.saturation = 15.0;
        e.vibrance = 10.0;
        e.vignette = edit_model::Vignette {
            amount: -35.0,
            ..Default::default()
        };
        e.rendering = Some(edit_model::Rendering::Camera);
        let region = Some(Region {
            full_w: 96,
            full_h: 64,
            x0: 8,
            y0: 4,
        });
        let want = tail_in_region(&e, region).run(scene_ramp());
        let neutral = tail_in_region(&Edits::default(), region).run(scene_ramp());
        assert!(
            crate::compare(&want, &neutral).worst_levels > 1.0,
            "fixture check: the edits move pixels"
        );
        let (pipe, chain) = accelerate(
            tail_in_region(&e, region),
            &e,
            SCALE,
            TailFacts {
                region,
                rendering: TailRendering::colorimetric(),
                spec: RenderSpec::default(),
            },
            &ctx,
        );
        assert!(
            pipe.stage_names().is_empty(),
            "every stage has a twin: {:?}",
            pipe.stage_names()
        );
        let chain = chain.expect("the tail is one chain");
        let got = chain.apply(scene_ramp());
        assert_eq!(
            chain.encode_units(),
            (1, 0),
            "one fused dispatch for the whole tail"
        );
        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels <= 1.0,
            "GPU differs from CPU by {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// The free part of fusion inside a mask: three per-pixel sub-stages are one fused dispatch
    /// within the mask's own pass, beside the tail's own fused run after it.
    #[test]
    fn a_masks_per_pixel_sub_stages_are_one_fused_dispatch() {
        let Some(ctx) = ctx() else { return };
        if !crate::per_pixel::fusion_enabled() {
            eprintln!("SKIPPED: RUSTROOM_NO_FUSION is set");
            return;
        }
        // ⚠ Two per-pixel LOOK sub-stages, so "fused into one" is a claim about more than one.
        // It was exposure, contrast and saturation until M10.12b made the first two offsets —
        // leaving one sub-stage, for which one dispatch is no evidence of fusion at all.
        let e = Edits {
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                tone_curve: edit_model::ToneCurve {
                    points: vec![[0.0, 0.0], [0.4, 0.55], [1.0, 1.0]],
                },
                saturation: 25.0,
                ..Default::default()
            })],
            ..Edits::default()
        };
        assert_eq!(
            tail_of(&e).sub_stage_names_of("mask").map(|v| v.len()),
            Some(2),
            "fixture check: the mask carries two sub-stages to fuse"
        );
        let cpu = tail_of(&e).run(scene_ramp());
        let unmasked = tail_of(&Edits::default()).run(scene_ramp());
        assert!(
            crate::compare(&cpu, &unmasked).worst_levels > 1.0,
            "fixture check: the mask moves pixels"
        );
        let (p, c) = accelerate(
            tail_of(&e),
            &e,
            SCALE,
            TailFacts {
                region: None,
                rendering: TailRendering::colorimetric(),
                spec: RenderSpec::default(),
            },
            &ctx,
        );
        let before = crate::per_pixel::fused_dispatches();
        let gpu = rejoin(p, c).run(scene_ramp());
        assert_eq!(
            crate::per_pixel::fused_dispatches() - before,
            2,
            "one fused dispatch inside the mask, one for the tail after it"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels <= 0.25,
            "GPU differs from CPU by {} levels",
            d.worst_levels
        );
    }

    /// A mask carrying exactly what it is handed, under a full-frame linear gradient.
    fn mask_carrying(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: a,
            disabled: false,
            invert: false,
        }
    }

    /// The sub-stage names of the first `mask` stage in a pipeline.
    fn mask_sub_stages(p: &Pipeline) -> Vec<&'static str> {
        p.sub_stage_names_of("mask")
            .expect("fixture check: the pipeline must contain a mask stage")
    }

    // ⚠⚠ **`brush_mask` was deleted at M8.11, and its going dead is itself the finding.** It
    // built a *plain* brush mask, and every such mask is now substituted for a GPU twin — so
    // nothing in this file could still use it. What is left as a wedge is `gating_brush_mask`;
    // see its doc for why that case is permanent.

    /// ⚠⚠ **The carrier: `accelerate` really offers the rasteriser.** Everything else in this file
    /// would pass with the offer deleted — the mask would build its coverage on the CPU and render
    /// the same picture, because the two rasterisers agree to within one level by design. Even
    /// `sub_stage_names_of` cannot see it, since a coverage rasteriser is not a stage. That is the
    /// W5 failure mode, and this is the only guard against it.
    #[test]
    fn accelerate_offers_a_coverage_rasterizer_to_a_mask() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            // ⚠ A look slider (M10.12b): a mask's exposure is an offset now, not a sub-stage.
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                saturation: 30.0,
                ..Default::default()
            })],
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
            TailRendering::default(),
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
    /// ⚠⚠ **A GATING BRUSH IS THE WEDGE NOW, AND THAT IS A CONSEQUENCE OF M8.11.** Substituting a
    /// mask for its GPU twin means the CPU `MaskStage` is no longer in the accelerated pipeline at
    /// all — so every test that inspects a mask's *inner* structure needs a mask the twin
    /// **declines**. A gating brush with no cached coverage map is that case, permanently: it
    /// decides its coverage from the photograph, which does not exist when the twin is built, and
    /// it is what every fresh render looks like before the cache warms.
    ///
    /// ⚠ This is the sixth time a property here became unconstructible by being fixed — after
    /// M8.3e's "a tail with nothing replaceable", M8.3h's "a trailing run refused", M8.3n's "a
    /// chain behind a CPU stage" and M8.3p's vignette. It used to be `mask_carrying(..)`.
    fn gating_brush_mask(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        let mut m = mask_carrying(a);
        m.mask = edit_model::Mask::Brush(edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                decode: false,
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
            TailRendering::default(),
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

    /// ⚠⚠ **The test that fails when `end_mask` is deleted, and nothing else does.** Render
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
            TailRendering::default(),
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

    /// ⚠⚠ **The negative half, and it is the one that matters.** A chain behind a CPU stage
    /// receives **that stage's output**, not the plane the gate was handed. Taking the deposit
    /// there would render a picture that is plausible and wrong — the failure mode this project
    /// records more than any other.
    ///
    /// ⚠⚠ **REBUILT AT M8.3n, AND THE REASON IS A RESULT RATHER THAN A GAP.** The fixture was a
    /// mask whose chain sat behind `local_contrast` — the last mask sub-stage with no
    /// `REPLACEABLE` row. Porting it means **all ten are ported**, so a mask's sub-stages are
    /// contiguous and its chain always lands at index 0: *"a chain behind a CPU stage"* is no
    /// longer constructible through any `Edits`. That is precisely the milestone's third claim
    /// delivered in full — **every** mask can now take M8.3l's shared upload, not merely more of
    /// them — and it is the third time this crate has watched a property become unreachable by
    /// being fixed (M8.3e's "a tail with nothing replaceable", M8.3h's "a trailing run refused").
    ///
    /// ⚠ So the rule is driven directly, over its whole truth table. Weaker than an end-to-end
    /// render in one way — it does not prove `walk` passes the right `is_first` — and stronger in
    /// another: it cannot go vacuous as the fixture drifts, which is exactly what happened to the
    /// test it replaces. ⚠ The carrier half is covered by
    /// `a_masks_first_sub_stage_takes_the_plane`, which is an end-to-end render and still builds.
    #[test]
    fn only_a_leading_chain_offered_the_plane_takes_it() {
        let Some(ctx) = private_ctx() else { return };
        let chain = || {
            GpuChain::new(
                ctx.clone(),
                vec![Box::new(GpuExposureStage::new(0.5, ctx.clone())) as Box<dyn GpuStage>],
            )
        };
        for (share_input, is_first, want) in [
            (Some(crate::Slot::Mask), true, true),
            // ⚠ The case the whole mechanism exists for: offered, but not at index 0.
            (Some(crate::Slot::Mask), false, false),
            // ⚠ And an unoffered walk, which is what the outer tail is when no accelerated prefix
            // step ran before it — any plane it found would be a *previous frame's* leftover.
            (None, true, false),
            (None, false, false),
            // ⚠⚠ **Since the resident prefix seam, `share_input` names WHICH slot, and that is the
            // guard rather than a detail.** The outer tail is offered `Prefix` and must never be
            // offered `Mask`; the two cannot be told apart by `shares_input()` alone, which is why
            // `takes_from()` exists and is asserted below.
            (Some(crate::Slot::Prefix), true, true),
        ] {
            let mut c = chain();
            assert!(
                !c.shares_input(),
                "fixture check: a fresh chain must not already share"
            );
            mark_shared(&mut c, share_input, is_first);
            assert_eq!(
                c.shares_input(),
                want,
                "share_input={share_input:?} is_first={is_first}"
            );
            // ⚠ The slot travels unchanged: `mark_shared` may refuse, but it must never SUBSTITUTE
            // one slot for the other. Cross-wiring is the sabotage the slot separation is written
            // against, and it moves no pixel in a fixture where only one slot is occupied.
            assert_eq!(
                c.takes_from(),
                if want { share_input } else { None },
                "the slot must be carried, not chosen: {share_input:?} is_first={is_first}"
            );
        }
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
    /// index 0 of the outer walk, and a mask that leaves a deposit unclaimed. Neither alone can
    /// see it.
    #[test]
    fn the_outer_tails_own_chain_never_takes_a_masks_leftover_plane() {
        let Some(ctx) = private_ctx() else { return };
        let mut e = with_hsl();
        // ⚠⚠ **The wedge changed at M8.3n and the property did not.** It used to be a mask whose
        // chain sat behind `local_contrast`; porting that stage means every mask sub-stage is now
        // GPU-able, so a mask leaves its deposit unclaimed for the *other* reason instead — its
        // sub-stages are too cheap to be worth a chain, so no chain exists to take the plane.
        // Saturation alone is 7.2 ms against a 27.0 ms transfer. ⚠ It was exposure (1.3 ms) until
        // M10.12b made a mask's exposure an offset rather than one of its sub-stages.
        e.masks = vec![gating_brush_mask(edit_model::LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        })];
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert_eq!(
            p.stage_names().first(),
            Some(&"gpu_chain"),
            "fixture check: the global hsl must be a chain at the outer tail's index 0, got {:?}",
            p.stage_names()
        );
        let subs = mask_sub_stages(&p);
        assert!(
            !subs.contains(&"gpu_chain"),
            "fixture check: the mask must build NO chain, so its deposit is left, got {subs:?}"
        );

        let first = p.run(big_scene());
        // ⚠⚠ **These counts changed at the resident base, and the CHANGE is the point.** Each
        // frame now makes two deposits and one take: the outer chain offers its output, the gate
        // takes it instead of uploading, and the gate then re-offers that same allocation to a
        // sub-stage chain — which this fixture deliberately does not have, so that second deposit
        // is the one left unclaimed.
        assert_eq!(
            (
                ctx.shared_plane().deposits(),
                ctx.shared_plane().takes(),
                format!("{:?}", ctx.shared_plane()).contains("occupied: false")
            ),
            (2, 1, true),
            "fixture check: chain offers + gate re-offers = 2, the gate takes 1, and `end_mask` \
             leaves the slot EMPTY — got {:?}",
            ctx.shared_plane()
        );
        let second = p.run(big_scene());
        let d = crate::compare(&first, &second);
        assert!(
            d.is_exact(),
            "the outer tail must not render a plane left over from a mask: {d:?}"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            2,
            "one take a frame and no more — a third would be somebody outside a gate taking one"
        );
    }

    /// ⚠⚠ **THE test of the resident base: the gate must render the CPU's picture while taking a
    /// buffer it never uploaded.** Every other assertion in this file about the hand-off is a
    /// *counter*, and a counter cannot tell a correct plane from a plausible one — taking the
    /// wrong allocation of the right size produces a photograph, not an error, which is the
    /// failure this project has recorded more than any other.
    ///
    /// ⚠ The fixture check is not decoration: without the global hsl there is no chain to offer
    /// anything, the gate uploads as it always did, and the comparison below passes while testing
    /// nothing at all. Measured before writing this — a light edit set of exposure and contrast
    /// builds **no chain**, because `worth_a_chain` refuses a run that cheap.
    #[test]
    fn a_gate_that_takes_the_chains_plane_still_renders_the_cpus_picture() {
        let Some(ctx) = private_ctx() else { return };
        let mut e = with_hsl();
        // ⚠ A look slider (M10.12b): a mask's exposure is an offset now, not a sub-stage.
        e.masks = vec![gating_brush_mask(edit_model::LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        })];
        let cpu = tail_of(&e).run(big_scene());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(big_scene());
        assert_eq!(
            ctx.shared_plane().takes(),
            1,
            "fixture check: the gate must actually have TAKEN a plane, else this proves nothing"
        );
        // ⚠⚠ **The bound was MEASURED at both ends, not guessed, and the measurement is the
        // finding: the take does not move this render by a single sample.** Forcing the take to
        // miss gives `worst_levels: 0.088246465, differing: 2207405` — the same figures to the
        // last digit as taking. The 0.088 is the accumulated hsl-chain-and-mask divergence this
        // file already lives with; a bound of 0.02 borrowed from a one-stage chain would have
        // failed here for a reason that has nothing to do with the hand-off.
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "a taken base must be the same pixels the upload carried: {d:?}"
        );
    }

    /// ⚠⚠ **THE test the end-placement of `end_mask` exists for, and it is the hazard the resident
    /// base newly creates.** A chain offers its output *for the mask that follows it*. When that
    /// mask does not take it — a gate under the box floor, or, as here, a brush with no gating
    /// rows at all — the plane is left in the slot. The *next* mask's gate would then find it at
    /// matching dimensions on the same thread and take it as its base. But its base is the first
    /// mask's **output**, and the leftover is the first mask's **input**: same size, same thread,
    /// different pixels. A plausible photograph.
    ///
    /// ⚠⚠ Sabotage to check this guards: empty the body of `GpuBrushRaster::end_mask`. The take
    /// count goes to 1 and the picture diverges. Clearing at the *top* of a mask instead — where
    /// the hook used to live — passes this test but silently kills the hand-off entirely, which is
    /// what `a_gate_that_takes_the_chains_plane_still_renders_the_cpus_picture` catches. **The two
    /// together pin the placement; neither alone does.**
    #[test]
    fn a_chains_offer_never_survives_into_the_next_mask() {
        let Some(ctx) = private_ctx() else { return };
        let mut e = with_hsl();
        // ⚠⚠ **Building the mask that leaves an offer unclaimed took two tries, and the first
        // one's failure is worth more than the fixture.** A brush with `auto_mask: false` has no
        // gating rows and so takes nothing — which looks like the wedge, and is not: M8.11
        // substitutes such a mask for its GPU twin, so it is folded INTO the preceding chain and
        // the offer is simply made to the *next* mask, whose input it correctly is. Measured
        // shape: `["gpu_chain", "mask", "gpu_chain"]`, one take, and the take was right.
        //
        // ⚠ So the first mask has to be one the twin **declines** — a gating brush, permanently —
        // whose box is nonetheless **under `MIN_GPU_BOX_PX`**, so `rasterize` returns at the floor
        // before it can take. Saturation alone keeps its sub-stages too cheap for a chain, which is
        // the second way it could have claimed the plane.
        //
        // ⚠⚠ Exposure until M10.12b, which made a mask's exposure an offset: both masks then had
        // NO stage at all, and the counter below read 0 because no gate existed — the test passed
        // vacuously until the fixture check after the masks was added.
        let mut small = gating_brush_mask(edit_model::LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        });
        if let edit_model::Mask::Brush(b) = &mut small.mask {
            b.strokes[0].points = vec![[0.5, 0.5]];
            b.strokes[0].radius = 0.05;
        }
        e.masks = vec![
            small,
            gating_brush_mask(edit_model::LocalAdjustments {
                saturation: -20.0,
                ..Default::default()
            }),
        ];
        let masks = tail_of(&e)
            .stage_names()
            .iter()
            .filter(|n| **n == "mask")
            .count();
        assert_eq!(
            masks, 2,
            "fixture check: both masks must be stages, or no gate exists"
        );
        let cpu = tail_of(&e).run(big_scene());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(big_scene());
        assert_eq!(
            ctx.shared_plane().takes(),
            0,
            "the second mask's gate must NOT have taken the offer made to the first"
        );
        // ⚠ **The counter above is the wedge; this is corroboration, and the band is chain noise
        // rather than a judgement.** Intact it measures 0.0549 levels — the same family as the
        // 0.088 the sibling test lives with. The leak it guards against is not a near miss: the
        // sibling `a_mask_that_deposits_nothing_never_takes_another_masks_plane` reports **243
        // levels** under the same sabotage, three orders of magnitude clear of this bound.
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "the second mask rendered a plane meant for the first: {d:?}"
        );
    }

    // ⚠⚠ **`plain_brush_mask` died at the composite fold, and its death is the finding — the
    // second helper in this file to go that way, after `brush_mask` at the substitution.** It
    // built a brush with `auto_mask: false`, which deposits nothing because without a gate
    // `plan.gating_rows` is `None` and the rasteriser uploads no base. That made it the taker in
    // `a_mask_that_deposits_nothing...`, but only while it ALSO declined — which it did solely
    // because it carried a refinement. Once a composite is what the fold accelerates, a plain
    // brush is substituted like any other and cannot be a wedge at all.
    //
    // ⚠ Clippy caught it while 209 tests were green, which is at least the twenty-fourth time in
    // this project. A dead fixture is invisible to a passing suite by construction.

    /// A gating brush whose box falls **below** `MIN_GPU_BOX_PX`.
    ///
    /// ⚠⚠ **Declines the twin and deposits nothing, which after M8.11 is a combination only this
    /// shape has.** The twin declines because the brush is gating with no cached map; the *gate*
    /// deposits nothing because `GpuBrushRaster` refuses a box under 512 Ki px, so the coverage
    /// never reaches the GPU and no base plane is uploaded. A full-size gating brush declines too
    /// and **deposits**, which is the one thing the taker must not do.
    ///
    /// ⚠ The scene stays 1024x1024 — only the *stroke* shrinks. `SharedPlane` refuses a deposit
    /// whose dimensions do not match, so a taker at a different image size could not take a stale
    /// plane even if the clear were deleted, and the test would go quietly vacuous.
    fn small_gating_brush_mask(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        let mut m = gating_brush_mask(a);
        if let edit_model::Mask::Brush(b) = &mut m.mask {
            b.strokes[0].points = vec![[0.1, 0.1], [0.14, 0.1]];
            b.strokes[0].radius = 0.03;
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

    /// ⚠⚠ **THE test `end_mask` exists for, and the obvious one could not see it.** A test that
    /// renders the same pipeline twice cannot: its chain *takes* the deposit every time, so the
    /// slot is already empty at the next apply and clearing is a no-op. Deleting `end_mask`'s
    /// body left that test green — found by running the sabotage, not by reading.
    ///
    /// ⚠ The discriminating case needs two different pipelines sharing one context: one that
    /// **leaves** a deposit (its sub-stages are too cheap to chain) and one that **deposits nothing**
    /// (a plain brush needs no base) whose chain is at index 0 and would therefore take whatever
    /// it finds. Without the clear, the second renders the first's pixels — a plausible
    /// photograph, and of the wrong photograph.
    #[test]
    fn a_mask_that_deposits_nothing_never_takes_another_masks_plane() {
        let Some(ctx) = private_ctx() else { return };

        // ⚠ The leaver's sub-stages are too cheap to be worth a chain (saturation alone is 7.2 ms
        // against a 27.0 ms transfer — exposure until M10.12b made it an offset), so its gate
        // deposits a plane nobody takes. ⚠⚠ It used to
        // be a mask whose chain sat behind `local_contrast`; porting that stage at M8.3n made
        // every mask sub-stage GPU-able, so that fixture no longer exists.
        let leaver = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                saturation: 30.0,
                ..Default::default()
            })],
            ..Edits::default()
        };
        // ⚠⚠ A gating brush too SMALL for the GPU rasteriser: it declines the twin, so the chain
        // this test is about still exists, and its gate never reaches the GPU, so it deposits
        // nothing. ⚠ It used to be a plain brush carrying a refinement — the twin declined any
        // composite — and the fold retired that wedge; see `small_gating_brush_mask`.
        let taker = Edits {
            masks: vec![small_gating_brush_mask(edit_model::LocalAdjustments {
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
                TailRendering::default(),
                RenderSpec::default(),
                &ctx,
            )
        };
        let (pl, pt) = (build(&leaver), build(&taker));
        assert!(
            !mask_sub_stages(&pl).contains(&"gpu_chain"),
            "fixture check: the leaver must build NO chain, so its deposit is left, got {:?}",
            mask_sub_stages(&pl)
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

    /// ⚠⚠ **What M8.3m actually buys, as a test — and it is NOT the four stages' own cost.**
    /// `contrast` and `tone_curve` sit **between** ported stages, so before this milestone a photo
    /// using them split the per-pixel head into pieces too small to pay: `[exposure]` at 1.3 ms
    /// and `[tone_regions]` at 18.0 were each refused by the gate, stranding **19.3 ms of
    /// already-ported work** on the CPU for no reason but a 1 ms neighbour standing between them.
    ///
    /// ⚠ M8.3h's lesson: state which decisions change and pin them, rather than believing the runs
    /// merged. This asserts the whole per-pixel head is absorbed — no member survives as a named
    /// CPU stage.
    #[test]
    fn the_per_pixel_head_becomes_one_run_once_every_member_is_ported() {
        let Some(ctx) = ctx() else { return };
        const HEAD: [&str; 6] = [
            "white_balance",
            "exposure",
            "contrast",
            "tone_regions",
            "tone_curve",
            "saturation",
        ];
        let mut e = with_hsl();
        e.white_balance.tint = 8.0;
        e.exposure = 0.4;
        e.contrast = 25.0;
        e.highlights = -30.0;
        e.tone_curve.points = vec![[0.0, 0.0], [0.3, 0.42], [1.0, 1.0]];
        e.saturation = 20.0;

        let before = tail_of(&e).stage_names();
        for s in HEAD {
            assert!(
                before.contains(&s),
                "fixture check: {s} must be in the CPU tail, got {before:?}"
            );
        }
        let after = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        let survivors: Vec<_> = after.iter().filter(|n| HEAD.contains(n)).collect();
        assert!(
            survivors.is_empty(),
            "every per-pixel head stage must have been absorbed into a chain, {survivors:?} \
             survived in {after:?}"
        );
    }

    /// ⚠⚠ **TEN of ten mask sub-stages, which is M8.3n's headline for masks.** Whatever a mask
    /// carries, its sub-stages are now contiguous and collapse to **one** chain — so a mask always
    /// pays one round trip rather than two, and its chain always sits at index 0, which is what
    /// makes M8.3l's shared upload reachable for **every** mask shape rather than merely more of
    /// them.
    ///
    /// ⚠⚠ **This test used to name `local_contrast` and its fixture did not contain one.** It
    /// asserted "with no local_contrast the whole set must become ONE chain" over a fixture that
    /// had none by construction, so it passed for the trivial reason and could never have seen the
    /// stage it was named for. The fixture now carries **every** sub-stage a mask can hold, and
    /// the fixture check below is what stops it drifting back.
    #[test]
    fn a_mask_carrying_the_whole_per_pixel_set_becomes_one_chain() {
        let Some(ctx) = ctx() else { return };
        let a = edit_model::LocalAdjustments {
            white_balance: edit_model::WhiteBalance {
                kelvin: None,
                tint: 8.0,
            },
            exposure: 0.4,
            contrast: 25.0,
            highlights: -30.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.3, 0.42], [1.0, 1.0]],
            },
            saturation: 20.0,
            hsl: with_hsl().hsl,
            color_grading: graded(Edits::default()).color_grading,
            sharpening: Sharpening {
                amount: 50.0,
                radius: 1.1,
                detail: 50.0,
                masking: 0.0,
            },
            // ⚠ The stage this milestone ports, and the one whose absence made this test vacuous.
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: 30.0,
            },
            ..Default::default()
        };
        let e = Edits {
            masks: vec![gating_brush_mask(a)],
            ..Edits::default()
        };
        let subs = mask_sub_stages(&tail_of(&e));
        // ⚠⚠ **All six, asserted by name.** `subs.len() > 1` was the old check and it is far too
        // weak: it holds for any two, so the one stage the test is about could go missing without
        // anything failing — which is exactly what happened.
        //
        // ⚠ Six, not ten, since M10.12b: the fixture still sets white balance, exposure, contrast
        // and highlights, and those are offsets in `SceneOffsetsStage` now — never sub-stages.
        for scene in ["white_balance", "exposure", "contrast", "tone_regions"] {
            assert!(
                !subs.contains(&scene),
                "a mask's {scene} is an offset, not a sub-stage: {subs:?}"
            );
        }
        for want in [
            "tone_curve",
            "local_contrast",
            "saturation",
            "hsl",
            "color_grading",
            "sharpening",
        ] {
            assert!(
                subs.contains(&want),
                "fixture check: the mask must carry `{want}` before acceleration, got {subs:?}"
            );
        }
        let after = mask_sub_stages(&accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        ));
        assert_eq!(
            after,
            vec!["gpu_chain"],
            "every sub-stage a mask can carry must collapse into ONE chain, got {after:?}"
        );
    }

    #[test]
    fn a_masked_hsl_becomes_a_chain_inside_the_mask() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
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
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        ));
        assert_eq!(
            after,
            vec!["gpu_chain"],
            "the mask's hsl must have become a chain"
        );
    }

    /// ⚠⚠ **The gate reaches the right verdict INSIDE a mask.** A mask carrying one cheap slider —
    /// saturation, 7.2 ms of work against a 27.0 ms transfer — must be **refused**: accelerating it
    /// would make the photograph slower.
    ///
    /// ⚠ This was `a_mask_carrying_only_exposure_is_not_worth_a_chain`, the catalogue's one masked
    /// photograph (`exposure: -3.6` and nothing else). Since M10.12b that mask inserts NO mask
    /// stage — its exposure is an offset in `SceneOffsetsStage` — which the first assertion pins.
    #[test]
    fn a_mask_carrying_one_cheap_slider_is_not_worth_a_chain() {
        let Some(ctx) = ctx() else { return };
        let exposure_only = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                exposure: -3.6,
                ..Default::default()
            })],
            ..Edits::default()
        };
        assert!(
            !tail_of(&exposure_only).stage_names().contains(&"mask"),
            "an exposure-only mask is an offset, with no mask stage"
        );
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                saturation: 30.0,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let after = mask_sub_stages(&accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        ));
        assert_eq!(
            after,
            vec!["saturation"],
            "one cheap slider must stay on the CPU inside a mask"
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
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
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
        let (prefix, tail) =
            pipeline::build_pipeline_split(&e, SCALE, pipeline::TailRendering::colorimetric());
        assert!(
            prefix.sub_stage_names_of("mask").is_some(),
            "fixture check: the prefix must carry a capture mask"
        );
        let accelerated = accel(
            tail,
            &e,
            SCALE,
            TailRendering::default(),
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

    /// A `.cube` for the look, so `creative_look` is actually in the tail.
    fn a_cube() -> std::sync::Arc<pipeline::lut::Cube> {
        std::sync::Arc::new(
            pipeline::lut::Cube::parse(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
            )
            .expect("the fixture cube parses"),
        )
    }

    /// An `Edits` that switches on every tail stage this table can replace.
    ///
    /// ⚠ Hand-built here rather than borrowed from `pipeline`'s own `all_edits`, which is private
    /// to that crate's tests. What stops it going stale is the completeness assertion in
    /// `every_tail_stage_is_replaceable_or_a_mask`, not care.
    fn every_tail_stage() -> Edits {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4000.0),
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: -15.0,
                },
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
            vignette: edit_model::Vignette {
                amount: -30.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 60.0,
                ..Default::default()
            },
            // ⚠ M10.11b: grain, so the tail's every-stage-replaceable and one-chain checks see it.
            grain: edit_model::Grain {
                amount: 40.0,
                ..Default::default()
            },
            // ⚠ M10.13d: Camera matching, so the tail carries `camera_look` (its cube from `full_tail`).
            rendering: Some(edit_model::Rendering::CameraMatch),
            look: edit_model::Look {
                file: Some("x.cube".into()),
                space: edit_model::LutSpace::SrgbDisplay,
                strength: 100.0,
            },
            ..Edits::default()
        }
    }

    /// The tail for [`every_tail_stage`], with the cube supplied.
    fn full_tail(e: &Edits, cube: Option<&std::sync::Arc<pipeline::lut::Cube>>) -> Pipeline {
        pipeline::build_pipeline_split_in_region_with_brush(
            e,
            SCALE,
            None,
            None,
            RenderSpec::default(),
            None,
            TailRendering {
                look: cube,
                camera_look: cube,
                ..TailRendering::default()
            },
        )
        .1
    }

    /// ⚠⚠ **THE MILESTONE'S HEADLINE AS A TEST: after M8.3p a tail contains nothing this table
    /// cannot replace except a mask.** The vignette and the creative look were the last two stages
    /// that could split it, and with both ported the property is worth asserting rather than
    /// describing — a stage added to the tail without a `REPLACEABLE` row silently reintroduces a
    /// split, and nothing else would notice.
    ///
    /// ⚠ `mask` is excepted because `accelerate` recurses *into* it rather than replacing it: it
    /// keeps its name and its position by design, which is what `merging_preserves_order` rests on.
    #[test]
    fn every_tail_stage_is_replaceable_or_a_mask() {
        let cube = a_cube();
        let e = every_tail_stage();
        let names = full_tail(&e, Some(&cube)).stage_names();
        // ⚠ M10.12b: the same tail with a mask carrying a scene offset, where `scene_offsets` takes
        // the four global scene stages' place — a stage with a twin, like a mask, and no table row.
        let mut offset = e.clone();
        offset
            .masks
            .push(mask_carrying(edit_model::LocalAdjustments {
                exposure: 0.5,
                ..Default::default()
            }));
        let offset_names = full_tail(&offset, Some(&cube)).stage_names();
        assert!(
            offset_names.contains(&"scene_offsets"),
            "fixture check: an offset puts `scene_offsets` in the tail, got {offset_names:?}"
        );
        // ⚠⚠ **The fixture check, and it is what stops this going stale.** The fixture is
        // hand-built, so a row added to `REPLACEABLE` with no field here would leave the property
        // asserted over a tail that never contained the new stage. Asserting the tail carries
        // **every** replaceable name makes that a failure rather than a silence.
        for (n, _) in REPLACEABLE {
            assert!(
                names.contains(n),
                "fixture check: `every_tail_stage` must switch on {n}, got {names:?}"
            );
        }
        for n in names.iter().chain(&offset_names) {
            assert!(
                *n == "mask" || *n == "scene_offsets" || REPLACEABLE.iter().any(|(r, _)| r == n),
                "{n} is in the tail and is neither replaceable nor a mask: {names:?}"
            );
        }
    }

    /// ⚠⚠ **The carrier for M10.12b's twin.** A CPU `scene_offsets` and a GPU one render the same
    /// picture by design, so every equivalence test passes with the substitution deleted — M8.11's
    /// lesson. This asserts the stage LEFT the CPU list and the picture still agrees.
    #[test]
    fn scene_offsets_run_on_the_gpu() {
        let Some(ctx) = ctx() else { return };
        let mut e = with_hsl();
        e.masks = vec![mask_carrying(edit_model::LocalAdjustments {
            exposure: 0.6,
            contrast: 20.0,
            ..Default::default()
        })];
        let before = tail_of(&e).stage_names();
        assert!(
            before.contains(&"scene_offsets"),
            "fixture check: the tail carries the offsets: {before:?}"
        );
        let cpu = tail_of(&e).run(scene_ramp());
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let after = p.stage_names();
        assert!(
            !after.contains(&"scene_offsets"),
            "the CPU stage must have been substituted: {after:?}"
        );
        let d = crate::compare(&cpu, &p.run(scene_ramp()));
        assert!(
            d.worst_levels < 1.0,
            "GPU differs by {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ M10.14h: a range mask with a scene offset selects on the reference photo — and since M10.14h the
    /// offsets stage computes it on the GPU and the mask's look twin blends by that plane, so NEITHER stays on the
    /// CPU, and the picture still equals the CPU's. ⚠ The fixture is the chase's: a twin selecting from its own base
    /// fails the picture check (it was M10.12b2's carrier when both stayed on the CPU).
    #[test]
    fn a_range_mask_with_an_offset_renders_on_the_gpu_and_agrees() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Twice: with the global look HSL only, and with a global exposure and curve too — without the second the
        // reference selects what the input does, and a twin that skipped the reference passed (a sabotage did).
        for global_ev in [0.0, 0.8] {
            let mut e = with_hsl();
            e.exposure = global_ev;
            if global_ev != 0.0 {
                e.tone_curve.points = vec![[0.0, 0.0], [0.3, 0.42], [1.0, 1.0]];
            }
            let base = e.clone();
            // ⚠ A narrow band and a big lift, so the chase is large: a wide default band selected the
            // same pixels from either base, and a twin reading its own base passed the picture check.
            e.masks = vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Luminance(edit_model::LuminanceRange {
                    low: 0.0,
                    high: 40.0,
                    feather: 5.0,
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 2.0,
                    saturation: 80.0,
                    ..Default::default()
                },
                ..Default::default()
            }];
            let cpu = tail_of(&e).run(scene_ramp());
            let p = accel(
                tail_of(&e),
                &e,
                SCALE,
                TailRendering::default(),
                RenderSpec::default(),
                &ctx,
            );
            let names = p.stage_names();
            assert!(
                !names.contains(&"scene_offsets") && !names.contains(&"mask"),
                "both run on the GPU: {names:?}"
            );
            let gpu = p.run(scene_ramp());
            assert!(
                gpu.data().iter().all(|v| v.is_finite()),
                "a non-finite GPU output"
            );
            let plain = tail_of(&base).run(scene_ramp());
            assert!(
                crate::compare(&cpu, &plain).worst_levels > 5.0,
                "fixture check: the mask must change the picture"
            );
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "global ev {global_ev}: the GPU path differs by {} levels",
                d.worst_levels
            );
        }
    }

    /// M10.14h with a colour range and a white-balance offset — the other kind that reads the photograph, and the
    /// offset whose `ln R` row the plane-fed path must still sum.
    #[test]
    fn a_colour_range_with_temp_renders_on_the_gpu_and_agrees() {
        let Some(ctx) = ctx() else { return };
        // Constructed: hues all round — `scene_ramp` is nearly grey, and a colour range picked nothing from it.
        let colourful = || {
            core_image::ImageBuf::from_vec(
                64,
                48,
                (0..64 * 48)
                    .flat_map(|i| {
                        let (x, y) = ((i % 64) as f32 / 64.0, (i / 64) as f32 / 48.0);
                        [x * 1.1, y, (1.0 - x) * (1.0 - y) * 0.9]
                    })
                    .collect(),
            )
        };
        let mut e = with_hsl();
        e.exposure = 0.6;
        let base = e.clone();
        e.masks = vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Colour(edit_model::ColourRange::default()),
            adjustments: edit_model::LocalAdjustments {
                temp: 60.0,
                highlights: -40.0,
                saturation: 30.0,
                ..Default::default()
            },
            ..Default::default()
        }];
        let cpu = tail_of(&e).run(colourful());
        assert!(
            crate::compare(&cpu, &tail_of(&base).run(colourful())).worst_levels > 5.0,
            "fixture check: the mask must change the picture"
        );
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let names = p.stage_names();
        assert!(
            !names.contains(&"scene_offsets") && !names.contains(&"mask"),
            "both run on the GPU: {names:?}"
        );
        let gpu = p.run(colourful());
        assert!(gpu.data().iter().all(|v| v.is_finite()));
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 1.0, "differs by {} levels", d.worst_levels);
    }

    /// M10.14h is all or nothing: a gating brush between the offsets stage and the mask reading the reference has no
    /// twin (it gates on the photograph, with no map yet), so the stretch cannot be one chain — and then the offsets
    /// stage and the reading mask BOTH stay on the CPU, and the picture still equals the CPU's. A GPU offsets stage
    /// beside a CPU mask would leave that mask's CPU slot empty.
    #[test]
    fn the_reference_falls_back_to_the_cpu_as_a_whole() {
        let Some(ctx) = ctx() else { return };
        let mut e = with_hsl();
        e.exposure = 0.8;
        let gating = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![edit_model::BrushStroke {
                    points: vec![[0.2, 0.5], [0.8, 0.5]],
                    radius: 0.2,
                    auto_mask: true,
                    ..Default::default()
                }],
                invert: false,
            }),
            adjustments: edit_model::LocalAdjustments {
                saturation: 40.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let range = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Luminance(edit_model::LuminanceRange {
                low: 0.0,
                high: 40.0,
                feather: 5.0,
                invert: false,
            }),
            adjustments: edit_model::LocalAdjustments {
                exposure: 2.0,
                saturation: 80.0,
                ..Default::default()
            },
            ..Default::default()
        };
        e.masks = vec![gating, range];
        let cpu = tail_of(&e).run(scene_ramp());
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let names = p.stage_names();
        assert!(
            names.contains(&"scene_offsets") && names.iter().filter(|n| **n == "mask").count() == 2,
            "the stretch stays on the CPU: {names:?}"
        );
        let d = crate::compare(&cpu, &p.run(scene_ramp()));
        assert!(d.worst_levels < 1.0, "differs by {} levels", d.worst_levels);
    }

    /// ⚠⚠⚠ **THE CARRIER FOR M8.11, AND IT IS WHAT STOPS THE MILESTONE SHIPPING GREEN AND DEAD.**
    ///
    /// An accelerated mask and a CPU mask produce the same picture **by design**, so every
    /// equivalence test in this crate — and all 63 goldens, which have no GPU context at all —
    /// passes with the whole substitution deleted. That is precisely how creative LUTs (W5) shipped
    /// fully tested and 100% non-functional, and how M8.7 plan 1 shipped inert.
    ///
    /// ⚠⚠ **One assertion carries both halves**: that the mask ran on the GPU *and* that it no
    /// longer ends a run. Before this milestone `every_tail_stage_is_replaceable_or_a_mask`
    /// recorded that a mask was the only thing left in the tail that could split it, so a tail with
    /// a mask in the middle produced **two** chains. One is the whole claim.
    #[test]
    fn a_tail_with_a_mask_in_the_middle_is_one_chain() {
        let Some(ctx) = ctx() else { return };
        // exposure -> mask -> output_transform: the mask sits between two replaceable stages, so
        // before M8.11 it split them into two runs.
        // ⚠ The mask carries a LOOK slider: since M10.12b a masked exposure is an offset at the
        // head of the tail and inserts no mask stage at all.
        let e = Edits {
            exposure: 0.5,
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                saturation: 30.0,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let before = tail_of(&e).stage_names();
        assert!(
            before.contains(&"mask"),
            "fixture check: the tail must actually contain a mask: {before:?}"
        );
        let after = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        assert!(
            !after.contains(&"mask"),
            "the CPU MaskStage must have been substituted: {after:?}"
        );
        assert_eq!(
            chains(&after),
            1,
            "a mask must no longer split the tail: {after:?}"
        );
    }

    /// ⚠ A mask the twin cannot build must leave the **fully accelerated** CPU stage in place —
    /// inner chain and coverage rasteriser and all — not a bare one. Trying the twin before the
    /// hooks run would make every decline a regression against M8.3j.
    #[test]
    fn a_declined_mask_keeps_its_inner_acceleration() {
        let Some(ctx) = ctx() else { return };
        // ⚠⚠ **This used to be a COMPOSITE, and that wedge died the day the fold landed.** The
        // twin declined any mask carrying refinements, so "a mask with a refinement" was a
        // decline; now a composite is exactly what the fold accelerates. A gating brush with no
        // cached map is the wedge that is permanent — see `gating_brush_mask`'s own doc.
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let after = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert!(
            after.stage_names().contains(&"mask"),
            "a declined mask must stay a CPU MaskStage: {:?}",
            after.stage_names()
        );
        assert_eq!(
            mask_sub_stages(&after),
            vec!["gpu_chain"],
            "and it must still carry M8.3j's inner chain"
        );
    }

    /// ⚠⚠ **And its consequence: an unmasked tail is now exactly ONE chain and one round trip.**
    /// Before this milestone a photograph carrying a vignette paid two, and one carrying a creative
    /// look paid two more — with a lone `display_rendering` run between them that the gate then
    /// refused outright, leaving 25.7 ms of ported work stranded on the CPU.
    #[test]
    fn an_unmasked_tail_is_exactly_one_chain() {
        let Some(ctx) = ctx() else { return };
        let cube = a_cube();
        let e = every_tail_stage();
        let after = accel(
            full_tail(&e, Some(&cube)),
            &e,
            SCALE,
            TailRendering {
                look: Some(&cube),
                camera_look: Some(&cube),
                ..TailRendering::default()
            },
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        assert_eq!(chains(&after), 1, "one chain, one round trip: {after:?}");
        assert_eq!(
            after.len(),
            1,
            "and nothing beside it — every tail stage joined it: {after:?}"
        );
    }

    /// ⚠⚠ **The carrier for the region, and nothing else can see it.** `GpuVignetteStage`'s own
    /// `a_region_is_shaded_as_that_part_of_the_whole` proves the *stage* honours a region; it says
    /// nothing about whether this table's row **passes** one. A row built with `None` would render
    /// a perfectly plausible photograph whose vignette is centred on whatever slice is on screen
    /// and slides across the picture as you pan — the M8.3k region-origin failure in a new place.
    #[test]
    fn the_vignette_row_carries_the_region() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            vignette: edit_model::Vignette {
                amount: -80.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        // A region well away from the frame's centre, so the falloff there is nothing like the
        // falloff a region-less stage would compute from the slice's own middle.
        let region = Region {
            full_w: 64,
            full_h: 48,
            x0: 40,
            y0: 30,
        };
        let img = || {
            let mut px = Vec::with_capacity(20 * 16 * 3);
            for i in 0..20 * 16 {
                let t = (i % 97) as f32 / 97.0;
                px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
            }
            core_image::ImageBuf::from_vec(20, 16, px)
        };
        let render = |r: Option<Region>| {
            let tail = pipeline::build_pipeline_split_in_region(
                &e,
                SCALE,
                r,
                pipeline::TailRendering::colorimetric(),
            )
            .1;
            let (p, c) = accelerate(
                tail,
                &e,
                SCALE,
                TailFacts {
                    region: r,
                    rendering: TailRendering::default(),
                    spec: RenderSpec::default(),
                },
                &ctx,
            );
            rejoin(p, c).run(img())
        };
        let want = pipeline::build_pipeline_split_in_region(
            &e,
            SCALE,
            Some(region),
            pipeline::TailRendering::colorimetric(),
        )
        .1
        .run(img());
        let with = render(Some(region));
        let without = render(None);
        // ⚠ The fixture check: the region must actually change the answer, or "it carries the
        // region" is asserted over a case where carrying it makes no difference.
        assert!(
            crate::compare(&want, &without).worst_levels > 4.0,
            "fixture check: this region must change the vignette"
        );
        let d = crate::compare(&want, &with);
        assert!(d.worst_levels < 1.0, "the region was not carried: {d:?}");
    }

    /// M10.11b: the grain row passes the region, as the vignette's does — grain is addressed in the
    /// frame's pixels, and a row built with `None` would re-seed it on whatever slice is on screen.
    #[test]
    fn the_grain_row_carries_the_region() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            grain: edit_model::Grain {
                amount: 80.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let region = Region {
            full_w: 64,
            full_h: 48,
            x0: 40,
            y0: 30,
        };
        let img = || {
            let mut px = Vec::with_capacity(20 * 16 * 3);
            for i in 0..20 * 16 {
                let t = 0.05 + (i % 97) as f32 / 160.0;
                px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
            }
            core_image::ImageBuf::from_vec(20, 16, px)
        };
        let render = |r: Option<Region>| {
            let tail = pipeline::build_pipeline_split_in_region(
                &e,
                SCALE,
                r,
                pipeline::TailRendering::colorimetric(),
            )
            .1;
            let (p, c) = accelerate(
                tail,
                &e,
                SCALE,
                TailFacts {
                    region: r,
                    rendering: TailRendering::default(),
                    spec: RenderSpec::default(),
                },
                &ctx,
            );
            rejoin(p, c).run(img())
        };
        let want = pipeline::build_pipeline_split_in_region(
            &e,
            SCALE,
            Some(region),
            pipeline::TailRendering::colorimetric(),
        )
        .1
        .run(img());
        let without = render(None);
        assert!(
            crate::compare(&want, &without).worst_levels > 1.0,
            "fixture check: this region must change the grain"
        );
        let d = crate::compare(&want, &render(Some(region)));
        assert!(d.worst_levels < 1.0, "the region was not carried: {d:?}");
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
            disabled: false,
            invert: false,
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
            TailRendering::default(),
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
        // ⚠⚠ **Re-aimed TWICE, and the second time is the interesting one.** It used the colour
        // grade until M8.3d and the **vignette** until M8.3p, and each stopped being a CPU stage
        // the moment it was ported. The wedge is a **mask** now, which is the last stage the outer
        // tail contains that `REPLACEABLE` cannot reach — `accelerate` recurses *into* it, so it
        // keeps its name and its position and still splits the run. Nothing else can play this
        // part any more, and that is the fourth time this crate has watched a property become
        // unconstructible by being fixed.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        // ⚠ A GATING brush: M8.11 substitutes every mask its twin can build, so an ordinary mask
        // is no longer a CPU stage at all. One that decides its coverage from the photograph
        // cannot be built before the photograph exists, so it declines and keeps its position.
        // ⚠ A look slider (M10.12b): a mask's exposure is an offset now, not a sub-stage.
        e.masks = vec![gating_brush_mask(edit_model::LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        })];
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            before.contains(&"mask"),
            "fixture check: an unaccelerated stage must sit between them, got {before:?}"
        );
        let after = accel(
            tail,
            &e,
            SCALE,
            TailRendering::default(),
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
        // relative to the chains around it. ⚠ Re-aimed at the vignette at M8.3d and at a **mask**
        // at M8.3p, for the reason written out on the test above.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        // ⚠ A GATING brush: M8.11 substitutes every mask its twin can build, so an ordinary mask
        // is no longer a CPU stage at all. One that decides its coverage from the photograph
        // cannot be built before the photograph exists, so it declines and keeps its position.
        // ⚠ A look slider (M10.12b): a mask's exposure is an offset now, not a sub-stage.
        e.masks = vec![gating_brush_mask(edit_model::LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        })];
        let after = accel(
            tail_of(&e),
            &e,
            SCALE,
            TailRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .stage_names();
        let wedge = after
            .iter()
            .position(|n| *n == "mask")
            .expect("the mask is kept");
        let at: Vec<usize> = after
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == "gpu_chain")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 2, "{after:?}");
        assert!(
            at[0] < wedge && wedge < at[1],
            "the mask must stay between the two chains, got {after:?}"
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
            TailRendering::default(),
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
            TailRendering::default(),
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
            TailRendering::default(),
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
                TailRendering::default(),
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
            TailRendering::default(),
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
            TailRendering::default(),
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

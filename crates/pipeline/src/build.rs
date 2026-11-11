//! The builders: every way to turn `Edits` into a pipeline, and how far the tail reaches.

use crate::{
    adjustment_stages, build_prefix_steps, effective_profile_curve, stages, BrushCoverage,
    ColorGradingStage, CreativeLookStage, DisplayRenderingStage, HslStage, LocalContrastStage,
    MaskStage, OutputTransform, Pipeline, PipelineStage, Region, RenderScale, Resampler,
    SaturationStage, SharpenStage, TailRendering, ToneCurveStage, VignetteStage,
};
use edit_model::Edits;

/// The per-render parameters that are not part of [`Edits`].
///
/// ⚠ A struct because the builder reached eight arguments and clippy's `too_many_arguments`
/// objected — the third time this project has answered that with a params struct, after
/// `FrameSpec` and `NameField`. Both times the note was that clippy was right, not a lint to
/// silence. The body destructures it, so it reads as before.
///
/// ⚠⚠ `baseline` is **the illuminant the source is already balanced to**, and it is the one field
/// here that is a fact about the file rather than a choice: the as-shot temperature for a RAW,
/// D65 for a JPEG the camera has already balanced. Its default is D65, which is not a fallback —
/// it is the right answer for every non-RAW file, and for a synthetic buffer with no camera at
/// all, which is what keeps the goldens untouched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSpec {
    pub space: color::OutputSpace,
    /// How colour `space` cannot hold is brought inside it (M6.4c). A tail-only property: no
    /// prefix stage reads it, so no `PrefixKey` carries it.
    pub intent: color::RenderingIntent,
    pub resampler: Resampler,
    pub baseline: color::wb::Illuminant,
    /// The frame-global dehaze airlight, from [`prefix_airlight`].
    ///
    /// ⚠⚠ **`None` means "let dehaze estimate from the image it is handed"**, which is what
    /// shipped before M8.7 and keeps every existing caller — the CLI, the goldens' default path,
    /// every test — on exactly the behaviour they had. A region-scoped render REQUIRES `Some`:
    /// `DehazeStage::apply_region` declines without one, because estimating from a slice changes
    /// the photograph as you pan.
    ///
    /// ⚠ It must be **derived from `edits`** via [`prefix_airlight`], never invented. `PrefixKey`
    /// does not carry it, and is sound only because the key already covers every edit the replay
    /// reads; an airlight unrelated to the edits would be a value the cache cannot see.
    pub airlight: Option<[f32; 3]>,
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: Resampler::default(),
            baseline: color::wb::Illuminant::default(),
            airlight: None,
        }
    }
}

/// The develop pipeline in two halves, split where the preview cache checkpoints.
///
/// The **prefix** — geometry, then noise reduction — depends only on `edits.geometry` and
/// `edits.noise_reduction`, which is exactly what [`PrefixKey`] captures. Its output can
/// therefore be reused across every adjustment downstream, which is what makes editing with
/// noise reduction enabled affordable.
///
/// The **tail** is everything else and always ends in the output transform.
///
/// [`build_pipeline`] is defined as `prefix ++ tail`, so the two constructions cannot drift
/// apart. Add a stage here, not there.
pub fn build_pipeline_split_in_region(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    rendering: TailRendering<'_>,
) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        region,
        None,
        RenderSpec::default(),
        None,
        rendering,
    )
}

/// The tail in two halves, split exactly where the grading stage goes.
///
/// ⚠ The split exists whether or not `ColorGradingStage` is active: Blending and Balance define
/// the ranges regardless, so "before the grade" is a structural position — after HSL, before the
/// masks loop — not a search for a stage name. The grade range view taps the buffer here.
pub struct TailHalves {
    /// Everything up to but excluding colour grading.
    pub before_grade: Pipeline,
    /// Colour grading (when active) and everything after it, ending with the output transform.
    pub from_grade: Pipeline,
}

/// As [`build_pipeline_split_in_region_with_brush`], with the tail returned in two halves.
///
/// ⚠ The untapped builder is a wrapper over this one that concatenates the halves, so every
/// existing caller — and every golden — is byte-identical by construction.
pub fn build_pipeline_split_tapped(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    // ⚠ An `&Arc`, not an `&BrushCoverage`: the masks loop hands each stage a clone so it can
    // **report** the coverage it had to build. `get` still works through the deref, so no caller
    // changes. See the coverage-cache spec's §4.0.
    brush: Option<&std::sync::Arc<BrushCoverage>>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    // What the *tail's* rendering needs that `Edits` cannot carry.
    rendering: TailRendering<'_>,
) -> (Pipeline, TailHalves) {
    let TailRendering {
        curve: profile_curve,
        baseline_ev,
        look: look_table,
        camera_look,
    } = rendering;
    let RenderSpec {
        space,
        intent,
        resampler,
        baseline,
        airlight,
    } = spec;
    // Defined through `build_prefix_steps` so the pipeline and the cache keys cannot drift:
    // one list, two consumers.
    let prefix: Vec<Box<dyn PipelineStage>> =
        build_prefix_steps(edits, scale, lens, resampler, airlight, None)
            .into_iter()
            .map(|s| s.stage)
            .collect();

    // ⚠⚠ M10.12b: when any active mask carries a scene slider, ONE stage owns the global white
    // balance, exposure, contrast and tone regions together with every mask's offsets to them —
    // and `global_look` builds those four neutral, or they would apply twice. With no such mask it is
    // `None` and the tail is built exactly as before, byte for byte.
    // ⚠ The reference photo (b2) is the global look itself, built at the global values — asked for
    // only when a mask reads it.
    let offsets = crate::SceneOffsetsStage::new(
        edits,
        scale,
        region,
        &edits.geometry,
        brush,
        baseline,
        || {
            let (mut stages, grade) = global_look(edits, scale, baseline, true);
            stages.extend(grade);
            stages
        },
    );
    // ⚠ Taken before the stage moves into the tail: each mask that reads the reference hands its
    // slot to its own look stage below.
    let slots: Vec<Option<std::sync::Arc<crate::CoverageSlot>>> = (0..edits.masks.len())
        .map(|i| offsets.as_ref().and_then(|o| o.slot_for(i)))
        .collect();
    // ⚠ The tap point: the global look is the first half, the grade begins the second.
    let (mut before_grade, grade) = global_look(edits, scale, baseline, offsets.is_none());
    if let Some(o) = offsets {
        before_grade.insert(0, Box::new(o));
    }
    let mut tail: Vec<Box<dyn PipelineStage>> = grade.into_iter().collect();

    // ⚠ After the global per-pixel block because a mask is the same kind of operation,
    // localised — it should act on the global look rather than be re-mapped by it. Before the
    // vignette and sharpening because both have earned their end-of-chain positions: the
    // vignette's "+100 = exactly white" endpoint depends on running after the tone curve, and
    // sharpening must not amplify anything a later stage would then stretch.
    // ⚠ The index is the position in `edits.masks`, **not** a counter over *active* masks. An
    // inactive mask is skipped here but still owns a slot in `BrushCoverage`, so a separate
    // counter would misalign every map after the first inactive one — the same sparse-versus-dense
    // index bug this codebase has already hit three times (the crop feedback loop,
    // `render_max_dim`, the Library grid).
    for (i, m) in edits.masks.iter().enumerate() {
        // ⚠ `disabled` is the panel's off switch (2026-09-20): the mask keeps its slot in the
        // coverage tables and its place in the list, and inserts no stage.
        // ⚠ M10.12b: only for its LOOK sliders — its scene sliders are offsets in
        // `SceneOffsetsStage`, so a mask carrying nothing else inserts no stage here.
        //
        // ⚠⚠ The stage is HANDED the look-only mask rather than stripping it itself: `MaskStage`
        // stays the general "adjustments under a mask" stage (its own tests, the GPU's parity
        // tests), and the GPU twin builds from the same look-only description it was built from.
        let look = edit_model::MaskedAdjustment {
            adjustments: crate::look_only(&m.adjustments),
            ..m.clone()
        };
        if look.adjustments.is_active() && !m.disabled {
            let stage = MaskStage::new_in_region(
                &look,
                scale,
                region,
                &edits.geometry,
                // ⚠ The mask's whole component row, index 0 being its base. `i` is still the
                // position in `edits.masks`, never a counter over *active* masks.
                brush.map_or(&[][..], |b| b.row(i)),
                baseline,
                // ⚠ `i` is the position in `edits.masks`, the same index the comment above
                // insists on — so a deposited map lands under the key `render_engine` computed
                // for *this* mask. A counter over active masks would file one mask's coverage
                // against another's key: a stale hit, which renders positionally while the tint
                // shows the gate working.
                brush.map(|b| (std::sync::Arc::clone(b), i)),
            );
            // ⚠⚠ M10.12b: a mask that reads the reference blends by the coverage its offsets used,
            // never by its own base — which those offsets have already moved.
            tail.push(Box::new(match slots[i].clone() {
                Some(slot) => stage.with_coverage_from(slot),
                None => stage,
            }));
        }
    }

    // A creative effect over the finished grade, so it runs last. It is post-crop for free:
    // `GeometryStage` ran first, so the frame it sees is already cropped. It takes no
    // `RenderScale` — normalized coordinates make it resolution-independent.
    if edits.vignette.is_active() {
        tail.push(Box::new(VignetteStage::new_in_region(
            &edits.vignette,
            region,
        )));
    }

    // The last spatial stage, and the only one in the tail. It therefore sets the margin a
    // region render must keep outside the visible rect — see `max_kernel_radius`. Placed
    // after the vignette so a darkened corner sharpens proportionally less in absolute
    // terms, which is what a ratio-based model should do.
    if edits.sharpening.is_active() {
        tail.push(Box::new(SharpenStage::new(&edits.sharpening, scale)));
    }

    // ⚠ M10.11b: grain, the last scene-linear stage — after sharpening, which must not sharpen it, and
    // before the rendering. Position-dependent like the vignette (frame coordinates, so a region is
    // the same crop) but reads no neighbour, so it needs no margin.
    if edits.grain.is_active() {
        tail.push(Box::new(crate::stages::grain::GrainStage::new_in_region(
            &edits.grain,
            scale,
            region,
        )));
    }

    // ⚠⚠ **Last before the output transform.** Every capture stage upstream keeps its
    // scene-linear tuning — `local_contrast`'s `MID_GRAY`, `sharpening`'s `LUMA_EPS`,
    // `dehaze`'s airlight percentile — and this is what bounds the data to [0, 1], which is the
    // seam creative LUTs and `ProfileLookTable` were blocked on.
    //
    // ⚠ It sits *after* sharpening, and "sharpening is last so nothing stretches it" survives
    // that: `OutputTransform` was always after sharpening too, and an sRGB encode is a ~1/2.2
    // power — far more aggressive than any tone curve.
    // ⚠⚠ **The look's table, and it gates everything below.** `edits.look` says what the user
    // asked for; `look_table` says whether the `.cube` is actually here. A preset naming a file
    // this machine does not have must render *without the look* — never silently as an error, and
    // never as a photo that lost its rendering too.
    let look = look_table.filter(|_| edits.look.is_active());

    // ⚠⚠ **A LINEAR LUT REPLACES THE CAMERA RENDERING; A DISPLAY-REFERRED ONE FOLLOWS IT.** This
    // is the core of the design (spec §2), and it is a placement decision rather than a shaping
    // one. A film emulation expects log-or-linear input and produces the final look — stacked on
    // top of `DisplayRenderingStage` it double-renders, our S-curve then the LUT's, giving
    // crushed shadows and a clipped roll-off. That is the single most common way LUT support is
    // reported as "looks wrong". A grade, by contrast, expects an already-rendered image.
    //
    // ⚠ Conditioned on `look.is_some()`, not on the declared space alone: see above.
    let look_is_the_rendering = look.is_some() && edits.look.space.is_the_rendering();

    match edits.applied_rendering() {
        edit_model::Rendering::None => {}
        _ if look_is_the_rendering => {
            // The linear LUT *is* the rendering. Suppressed deliberately — the Rendering panel
            // section must say so, because two renderings apparently both switched on with one
            // silently ignored is the invisible-state failure this project keeps recording.
        }
        r @ (edit_model::Rendering::Camera | edit_model::Rendering::CameraMatch) => {
            // ⚠⚠ An **identity** profile curve is ignored. The X-T4's `ProfileToneCurve` is
            // literally [[0,0],[1,1]], so honouring it would produce no rendering at all —
            // indistinguishable from `None`, and the setting would silently do nothing on the
            // author's own camera.
            let curve = effective_profile_curve(profile_curve);
            tail.push(Box::new(DisplayRenderingStage::new(curve, baseline_ev)));
            // ⚠ M10.13d: the learned look follows the rendering it was fitted on, and precedes the user's look.
            if let (edit_model::Rendering::CameraMatch, Some(cube)) = (r, camera_look) {
                tail.push(Box::new(CreativeLookStage::camera(std::sync::Arc::clone(
                    cube,
                ))));
            }
        }
    }

    // ⚠ Either way, **before** the output transform: a look is a display-referred decision on
    // working-space data, not something applied to encoded output.
    if let Some(cube) = look {
        tail.push(Box::new(CreativeLookStage::new(
            // ⚠ Cloning the `Arc`, not the table — a refcount rather than up to 786 KB, on every
            // rebuild of the pipeline.
            std::sync::Arc::clone(cube),
            edits.look.space,
            edits.look.strength,
        )));
    }

    tail.push(Box::new(OutputTransform::new(space, intent)));

    (
        Pipeline::new(prefix),
        TailHalves {
            before_grade: Pipeline::new(before_grade),
            from_grade: Pipeline::new(tail),
        },
    )
}

/// One boxed stage — `global_look`'s grade, apart from the rest.
type Stage = Box<dyn PipelineStage>;

/// The global look: the four scene stages, the tone curve, local contrast, saturation and HSL — and
/// the grade, returned apart because the tail is tapped between them (`TailHalves`).
///
/// ⚠⚠ `scene`: whether the global white balance, exposure, contrast and tone regions run here —
/// `false` when `SceneOffsetsStage` does them together with the masks' offsets (M10.12b), `true` for a
/// tail without offsets and for that stage's reference photo. ONE builder, so the reference IS the
/// global look rather than a second list that could drift from it.
fn global_look(
    edits: &Edits,
    scale: RenderScale,
    baseline: color::wb::Illuminant,
    scene: bool,
) -> (Vec<Box<dyn PipelineStage>>, Option<Stage>) {
    let (wb, ev, con, regions) = if scene {
        (
            edits.white_balance,
            edits.exposure,
            edits.contrast,
            [edits.highlights, edits.shadows, edits.whites, edits.blacks],
        )
    } else {
        (edit_model::WhiteBalance::default(), 0.0, 0.0, [0.0; 4])
    };
    // ⚠ `saturation`/`vibrance` are passed as **zero** here, deliberately. In the global chain
    // saturation runs *after* the tone curve and local contrast, not with the basic
    // adjustments — so it stays where it is, further down this tail. Passing the real values
    // would silently move it earlier and change every golden. Inside a mask there is no tone
    // curve, so ordering within `per_pixel_stages` is the whole story and saturation belongs
    // in it.
    let mut tail: Vec<Box<dyn PipelineStage>> = adjustment_stages(
        &edit_model::LocalAdjustments {
            // ⚠⚠ **Neutral because these two do not run here at all** — unlike the five below,
            // which this tail places itself further down. `noise_reduction` and `dehaze` are
            // built by `capture_stages` and run in the **prefix**, where the global pair run.
            // Passing them here would apply a capture correction after the tone curve, on a
            // buffer that may be only the visible region — which for dehaze means an airlight
            // that changes as you pan.
            noise_reduction: edit_model::NoiseReduction::default(),
            dehaze: edit_model::Dehaze::default(),
            // ⚠ Likewise a prefix step (M9.6), after geometry — never a tail stage.
            defringe: 0.0,
            white_balance: wb,
            // ⚠ A mask's relative shift (M10.12b); the global white balance above is absolute.
            temp: 0.0,
            tint: 0.0,
            exposure: ev,
            contrast: con,
            highlights: regions[0],
            shadows: regions[1],
            whites: regions[2],
            blacks: regions[3],
            saturation: 0.0,
            vibrance: 0.0,
            // ⚠⚠ Neutral for the identical reason `saturation` above is zero: **this tail places
            // each of these itself**, further down, at a position each has earned. Passing the
            // real values would apply every one of them **twice** and move every golden.
            //
            // ⚠ Inside a mask there is no such second placement, so a mask passes its real
            // values and the ordering within the shared builder is the whole story.
            //
            // ⚠ `ToneCurve::default()` is an empty point list, which inserts no stage at all —
            // the same "be exactly neutral by inserting nothing rather than trusting the
            // arithmetic to land on the identity" rule `white_balance` follows above.
            tone_curve: edit_model::ToneCurve::default(),
            local_contrast: edit_model::LocalContrast::default(),
            hsl: edit_model::Hsl::default(),
            color_grading: edit_model::ColorGrading::default(),
            sharpening: edit_model::Sharpening::default(),
        },
        baseline,
        scale,
    );
    // ⚠⚠ **All four curves, not just the composite.** An `is_empty` guard naming one field of four
    // would render a channel-only edit unchanged, with nothing failing — the regression this feature
    // invites, and it has its own test.
    // ⚠ Through `is_needed`/`from_edits`, never the fields directly: the parametric curve (M5.1c)
    // is the fifth field, and a guard naming four would render it unchanged.
    if ToneCurveStage::is_needed(edits) {
        tail.push(Box::new(ToneCurveStage::from_edits(edits)));
    }

    // Local contrast, in two bands. After the tone curve is mandatory: a local contrast
    // measured before it would then be re-mapped by it. Before the colour stages so they
    // see final tonality — numerically near-irrelevant, since the gain is scalar and
    // preserves r:g:b ratios, but the order should say what we mean.
    if edits.local_contrast.is_active() {
        tail.push(Box::new(LocalContrastStage::new(
            &edits.local_contrast,
            scale,
        )));
    }

    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        tail.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }

    if edits.hsl != edit_model::Hsl::default() {
        tail.push(Box::new(HslStage::new(&edits.hsl)));
    }

    // A look laid over the corrected photograph. After HSL because HSL corrects colours that
    // are *in* the picture while a grade is applied on top — reversed, an HSL band tweak would
    // act on the tint just added. After the tone curve and local contrast is forced rather than
    // chosen: the ranges are selected on lightness, so anything remapping lightness later would
    // slide pixels between ranges. Before the masks, which act on the global look.
    //
    // Per-pixel and position-independent, so it needs no `Region`, no `RenderScale` and no
    // kernel margin — and being in the tail, no `PrefixStepKey`.
    //
    // ⚠ The tap point: everything above is the first half, the grade begins the second.
    let grade = edits
        .color_grading
        .is_active()
        .then(|| Box::new(ColorGradingStage::new(&edits.color_grading)) as Box<dyn PipelineStage>);
    (tail, grade)
}

/// As [`build_pipeline_split_in_region`], with brush coverage the caller has already rasterized.
///
/// ⚠ The extra parameter is additive with a delegating wrapper — the `Region` precedent, which
/// gave that work zero call-site churn. The CLI, export and every golden go through the wrapper.
///
/// ⚠ Itself a wrapper over [`build_pipeline_split_tapped`], concatenating the two halves — the
/// same stages in the same order, so nothing here can move a pixel.
pub fn build_pipeline_split_in_region_with_brush(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    // ⚠ An `&Arc`, not an `&BrushCoverage`: the masks loop hands each stage a clone so it can
    // **report** the coverage it had to build. `get` still works through the deref, so no caller
    // changes. See the coverage-cache spec's §4.0.
    brush: Option<&std::sync::Arc<BrushCoverage>>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    rendering: TailRendering<'_>,
) -> (Pipeline, Pipeline) {
    let (prefix, halves) =
        build_pipeline_split_tapped(edits, scale, region, brush, spec, lens, rendering);
    let mut tail = halves.before_grade.into_stages();
    tail.extend(halves.from_grade.into_stages());
    (prefix, Pipeline::new(tail))
}

/// The develop pipeline in two halves, for a render covering the whole frame.
pub fn build_pipeline_split(
    edits: &Edits,
    scale: RenderScale,
    rendering: TailRendering<'_>,
) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region(edits, scale, None, rendering)
}

/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
///
/// ⚠ Resamples with [`Resampler::default`]. Every **production** render goes through
/// [`build_pipeline_with`] instead, so the user's choice reaches it; this wrapper exists so the
/// goldens, the examples and the debug tools compile untouched — the `Region` precedent.
pub fn build_pipeline(edits: &Edits, scale: RenderScale, rendering: TailRendering<'_>) -> Pipeline {
    build_pipeline_with(edits, scale, Resampler::default(), rendering)
}

/// As [`build_pipeline`], resampling with the caller's chosen kernel.
pub fn build_pipeline_with(
    edits: &Edits,
    scale: RenderScale,
    resampler: Resampler,
    rendering: TailRendering<'_>,
) -> Pipeline {
    build_pipeline_to(edits, scale, color::OutputSpace::Srgb, resampler, rendering)
}

/// As [`build_pipeline`], writing to `space` rather than sRGB.
///
/// ⚠ Additive with a delegating wrapper — the `Region` precedent, which gave that work zero
/// call-site churn. Only export and `rustroom render` call this; the preview goes through
/// [`build_pipeline_with`] and stays sRGB, because the display is assumed sRGB until a display
/// profile exists.
pub fn build_pipeline_to(
    edits: &Edits,
    scale: RenderScale,
    space: color::OutputSpace,
    resampler: Resampler,
    rendering: TailRendering<'_>,
) -> Pipeline {
    build_pipeline_spec(
        edits,
        scale,
        RenderSpec {
            space,
            intent: color::RenderingIntent::default(),
            resampler,
            ..RenderSpec::default()
        },
        rendering,
    )
}

/// As [`build_pipeline_to`], with the whole [`RenderSpec`] — which is what an export of a RAW
/// needs, since only it knows the file's white-balance baseline.
pub fn build_pipeline_spec(
    edits: &Edits,
    scale: RenderScale,
    spec: RenderSpec,
    rendering: TailRendering<'_>,
) -> Pipeline {
    build_pipeline_rendered(edits, scale, spec, rendering)
}

/// As [`build_pipeline_spec`], differing only in that it takes the whole [`RenderSpec`] **and** the
/// rendering — which, since every builder above now requires the rendering too, makes this the one
/// they all delegate to.
///
/// ⚠⚠ **It exists because defaulting the rendering SILENTLY cost three defects in one day.** Until
/// 2026-09-18 every whole-pipeline builder handed `TailRendering::default()` down without being
/// asked, so a call site that needed the profile's curve or the user's `.cube` got neither **and no
/// warning** — which is how an export rendered without the look, and how a thumbnail still did
/// after that was fixed. An absence looked exactly like a decision.
///
/// ⚠ Those builders now take the rendering as a required argument, so the hole this was added to
/// work around is closed at the source. See [`TailRendering::colorimetric`] for the name a
/// caller that wants none should use.
pub fn build_pipeline_rendered(
    edits: &Edits,
    scale: RenderScale,
    spec: RenderSpec,
    rendering: TailRendering<'_>,
) -> Pipeline {
    let (prefix, tail) =
        build_pipeline_split_in_region_with_brush(edits, scale, None, None, spec, None, rendering);
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    Pipeline::new(stages)
}

/// The widest neighbourhood any **tail** stage reads, in rendered pixels, for these edits.
///
/// The tail is the half that runs on a cropped region during a 1:1 render, so at the
/// region's edge a spatial tail stage reads neighbours that do not exist. This is the width
/// of that wrong band, and therefore the margin such a render must keep outside what it
/// displays. Prefix stages are exempt: the prefix always runs whole-buffer, which is why
/// noise reduction can never produce a seam however you pan.
///
/// An inactive stage contributes 0.
///
/// ⚠ **The reaches add, they do not `max`.** Sharpening runs *after* local contrast, so a
/// wrong value at the region's edge propagates 120 px inward through clarity and then a
/// further 15 px through sharpening — 135, not 120. This was a `max` until masking exposed
/// it: `a_region_render_matches_the_whole_render_cropped` had been passing with an inset
/// ~15 px too small, because the residual differences happened to round the same way. Adding
/// a masked +0.6 EV — up to a 1.5× gain — pushed them across a quantisation boundary and the
/// test failed. The slack was luck, not margin.
///
/// A per-pixel stage between them contributes nothing: it cannot move information sideways.
/// That is why masks, the vignette and every colour stage are absent here.
pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize {
    let sharp = if edits.sharpening.is_active() {
        stages::sharpening::max_kernel_radius(edits.sharpening.radius, scale)
    } else {
        0
    };
    let local = stages::local_contrast::kernel_reach(&edits.local_contrast, scale);

    // ⚠⚠ **Summed over the masks, not maximised.** Mask N runs on mask N-1's output, so a pixel
    // at the region edge reads through every mask in turn — exactly the reason the global
    // `sharp` and `local` above are added rather than maxed. Taking a max here is invisible with
    // one mask and wrong with two; see `two_masked_clarities_reach_further_than_one`.
    //
    // ⚠ This is why no fixed region margin can be correct: the stack is unbounded, so the reach
    // is a function of the edits and the caller must size its request from it.
    let masked: usize = edits
        .masks
        .iter()
        .filter(|m| !m.disabled)
        .map(|m| {
            let a = &m.adjustments;
            let s = if a.sharpening.is_active() {
                stages::sharpening::max_kernel_radius(a.sharpening.radius, scale)
            } else {
                0
            };
            s + stages::local_contrast::kernel_reach(&a.local_contrast, scale)
        })
        .sum();

    sharp + local + masked
}

// ⚠⚠ **There is deliberately no `max_tail_kernel_radius` any more.** It maximised
// `tail_kernel_radius` over every representable setting, and a fixed region margin was checked
// against it. Masks now carry spatial adjustments and **chain**, so N masked clarities reach
// N x 120 px and no constant bounds the result. The caller derives its margin from
// `tail_kernel_radius` per render instead — see `Session::render_margin` in the app crate.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::all_edits;
    use core_image::ImageBuf;

    /// ⚠⚠ **The regression this feature invites.** The guard keeping `ToneCurveStage` out of the
    /// pipeline tested `edits.tone_curve.points` alone; with three more curves, naming one field of four
    /// would render a channel-only edit **unchanged, with nothing failing**.
    #[test]
    fn a_channel_curve_alone_still_reaches_the_pipeline() {
        let names = |e: &Edits| {
            build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).stage_names()
        };
        let base = Edits::default();
        assert!(
            !names(&base).contains(&"tone_curve"),
            "with no curve at all the stage must stay out"
        );

        for which in 0..3 {
            let mut e = Edits::default();
            let c = edit_model::ToneCurve {
                points: vec![[0.0, 0.2], [1.0, 1.0]],
            };
            match which {
                0 => e.tone_curve_red = c,
                1 => e.tone_curve_green = c,
                _ => e.tone_curve_blue = c,
            }
            assert!(
                names(&e).contains(&"tone_curve"),
                "channel {which} alone did not bring the stage into the pipeline"
            );
        }

        let mut e = Edits::default();
        e.tone_curve.points = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        assert!(
            names(&e).contains(&"tone_curve"),
            "and the composite still does"
        );
    }

    #[test]
    fn a_switched_off_mask_inserts_no_stage_anywhere() {
        // ⚠ The export builds the same pipeline as the view, so "not in the export" is exactly
        // "no stage": neither the tail's mask stage nor a prefix step for its NR or dehaze.
        // Compared against no mask at all, so a stage that slipped through under another name
        // would show.
        let mut edits = Edits::default();
        edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                exposure: 1.0,
                dehaze: edit_model::Dehaze { amount: 30.0 },
                ..Default::default()
            },
            disabled: false,
            invert: false,
        });
        let names = |e: &Edits| {
            build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).stage_names()
        };
        let on = names(&edits);
        assert!(
            on.contains(&"mask"),
            "fixture: the enabled mask must insert a stage: {on:?}"
        );
        edits.masks[0].disabled = true;
        let off = names(&edits);
        assert!(
            !off.contains(&"mask"),
            "a switched-off mask inserted a stage: {off:?}"
        );
        assert_eq!(
            off,
            names(&Edits::default()),
            "a switched-off mask must build the same pipeline as no mask"
        );
    }

    /// ⚠ "Off" is skipping the stage, never zero coverage — which an Invert would turn into FULL.
    /// `a_switched_off_mask_inserts_no_stage_anywhere`'s shape, with its positive control.
    #[test]
    fn a_switched_off_inverted_mask_changes_nothing() {
        let mut edits = Edits::default();
        // ⚠ A scene slider AND a look slider (M10.12b): enabled, the mask inserts both stages —
        // `scene_offsets` for its exposure, `mask` for its saturation — and switched off, neither.
        edits.masks.push(edit_model::MaskedAdjustment {
            invert: true,
            adjustments: edit_model::LocalAdjustments {
                exposure: 1.0,
                saturation: 30.0,
                ..Default::default()
            },
            ..Default::default()
        });
        let names = |e: &Edits| {
            build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).stage_names()
        };
        let on = names(&edits);
        assert!(
            on.contains(&"mask") && on.contains(&"scene_offsets"),
            "fixture: an inverted mask inserts its stages: {on:?}"
        );
        edits.masks[0].disabled = true;
        assert_eq!(
            names(&edits),
            names(&Edits::default()),
            "a switched-off inverted mask must build the same pipeline as no mask"
        );
    }

    /// ⚠⚠ **That the wrapper entry points do not quietly substitute the default.** Every
    /// production render outside the engine goes through `build_pipeline_with` or
    /// `build_pipeline_to` — the canvas's `render_once`, every thumbnail, every export — and
    /// hardcoding `Resampler::default()` in either leaves all 49 goldens green, because the
    /// goldens ask for the default anyway. Nothing else in the suite can see it.
    #[test]
    fn the_pipeline_builders_honour_the_kernel_they_are_handed() {
        // ⚠ A hard edge, not a ramp: every kernel reproduces a linear ramp almost exactly.
        let mut src = ImageBuf::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let v = if (x / 6) % 2 == 0 { 0.05 } else { 0.9 };
                src.set_pixel(x, y, [v, v, v]);
            }
        }
        let edits = Edits {
            geometry: edit_model::Geometry {
                angle: 4.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let with = |k| {
            build_pipeline_with(&edits, RenderScale::FULL, k, TailRendering::colorimetric())
                .run(src.clone())
        };
        let to = |k| {
            build_pipeline_to(
                &edits,
                RenderScale::FULL,
                color::OutputSpace::Srgb,
                k,
                TailRendering::colorimetric(),
            )
            .run(src.clone())
        };
        assert_ne!(
            with(Resampler::Bilinear).data(),
            with(Resampler::Lanczos3).data(),
            "build_pipeline_with ignored its kernel"
        );
        assert_ne!(
            to(Resampler::Bilinear).data(),
            to(Resampler::Lanczos3).data(),
            "build_pipeline_to ignored its kernel"
        );
    }

    #[test]
    fn a_none_region_split_is_identical_to_the_plain_split() {
        for edits in [Edits::default(), all_edits()] {
            let (p1, t1) =
                build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
            let (p2, t2) = build_pipeline_split_in_region(
                &edits,
                RenderScale::FULL,
                None,
                TailRendering::colorimetric(),
            );
            assert_eq!(p1.stage_names(), p2.stage_names());
            assert_eq!(t1.stage_names(), t2.stage_names());
        }
    }

    #[test]
    fn build_pipeline_is_exactly_prefix_then_tail() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) =
                build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
            let mut expected = prefix.stage_names();
            expected.extend(tail.stage_names());
            assert_eq!(
                build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                    .stage_names(),
                expected
            );
        }
    }

    /// ⚠⚠ The tapped tail is the tail, cut in two at the grading stage — for a grade that is
    /// active AND for one that is not, because Blending and Balance define the ranges whether or
    /// not a wheel is turned, and the split must be structural rather than "find the stage
    /// named color_grading". Sabotage: cutting one stage late fails the second assertion.
    #[test]
    fn the_tapped_tail_concatenates_to_the_tail_and_splits_at_the_grade() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) =
                build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
            let (tapped_prefix, halves) = build_pipeline_split_tapped(
                &edits,
                RenderScale::FULL,
                None,
                None,
                RenderSpec::default(),
                None,
                TailRendering::default(),
            );
            assert_eq!(tapped_prefix.stage_names(), prefix.stage_names());
            let mut joined = halves.before_grade.stage_names();
            joined.extend(halves.from_grade.stage_names());
            assert_eq!(joined, tail.stage_names(), "the halves are not the tail");
            assert!(
                !halves.before_grade.stage_names().contains(&"color_grading"),
                "the grade is in the first half"
            );
            if edits.color_grading.is_active() {
                assert_eq!(
                    halves.from_grade.stage_names().first(),
                    Some(&"color_grading"),
                    "an active grade must open the second half"
                );
            }
        }
        // The fixture must exercise both branches, or the `if` above is never taken.
        assert!(all_edits().color_grading.is_active());
        assert!(!Edits::default().color_grading.is_active());
    }

    #[test]
    fn running_prefix_then_tail_matches_the_whole_pipeline() {
        // The numerical counterpart to the name-based test above: bit-identical, not close.
        let edits = all_edits();
        let mut img = ImageBuf::new(16, 12);
        for y in 0..12 {
            for x in 0..16 {
                img.set_pixel(x, y, [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
            }
        }
        let whole = build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
            .run(img.clone());
        let (prefix, tail) =
            build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
        let split = tail.run(prefix.run(img));
        assert_eq!(whole.data(), split.data());
    }

    /// The global tail must not apply the mask-only stages twice.
    ///
    /// ⚠⚠ The trap this exists for. `adjustment_stages` has two consumers — the global tail and
    /// every mask — and the global tail places the tone curve, local contrast, HSL, colour
    /// grading and sharpening **itself**, further down. So it passes those five **neutral** into
    /// the shared builder. Passing the real values applies each twice: visibly over-sharpened,
    /// and every golden moves.
    ///
    /// ⚠⚠ **IT NAMED THREE OF FIVE AND WAS THEREFORE BLIND TO A WHOLE MILESTONE.** When the tone
    /// curve and colour grading joined the shared builder, the double-application sabotage left
    /// this test **green** and was caught only by six moving goldens. A guard that enumerates a
    /// list has to be extended with the list, or it silently stops guarding the newest entries —
    /// which are exactly the ones nobody has habits about yet. Add every field the caller passes
    /// neutral, and add it here in the same commit.
    ///
    /// ⚠ This was written *before* the stages joined the shared builder, so it passed from the
    /// start — a tripwire, not a red test. What makes it a real guard is the sabotage recorded
    /// with it: hand `edits.local_contrast` to the builder and watch this fail.
    /// Two masked clarities reach twice as far as one.
    ///
    /// ⚠⚠ **A ONE-MASK TEST CANNOT SEE SUM-VERSUS-MAX** — with a single mask the two are
    /// identical, so the wrong arithmetic passes. Two is the smallest fixture that discriminates,
    /// the same shape as the same-month-in-two-years fixture.
    ///
    /// ⚠ The reaches compose because mask N runs on mask N-1's output, so a pixel at the region
    /// edge reads through every mask in turn. Getting this wrong makes the region margin too
    /// small, and the failure is silent: a subtly wrong band at the edge of the visible area.
    #[test]
    fn two_masked_clarities_reach_further_than_one() {
        let masked = |clarity: f32| edit_model::MaskedAdjustment {
            mask: edit_model::Mask::default(),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                local_contrast: edit_model::LocalContrast {
                    texture: 0.0,
                    clarity,
                },
                ..Default::default()
            },
            disabled: false,
            invert: false,
        };
        let one = Edits {
            masks: vec![masked(100.0)],
            ..Edits::default()
        };
        let two = Edits {
            masks: vec![masked(100.0), masked(100.0)],
            ..Edits::default()
        };

        let r1 = tail_kernel_radius(&one, RenderScale::FULL);
        let r2 = tail_kernel_radius(&two, RenderScale::FULL);
        assert!(r1 > 0, "a masked clarity must contribute reach at all");
        assert_eq!(
            r2,
            2 * r1,
            "the reaches compose: two masks reach twice as far, not the same"
        );
    }

    #[test]
    fn stage_names_are_reported_in_order() {
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec!["exposure", "saturation", "output_transform"]
        );
    }

    #[test]
    fn output_transform_encodes_linear_to_srgb() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.5, 0.5, 0.5]);
        let out = build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(img);
        let [r, g, b] = out.pixel(0, 0);
        let expected = color::linear_to_srgb(0.5);
        assert!((r - expected).abs() < 1e-4);
        assert!((g - expected).abs() < 1e-4);
        assert!((b - expected).abs() < 1e-4);
    }

    #[test]
    fn default_pipeline_has_output_transform_last() {
        let p = build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        );
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn nonneutral_edits_add_stages_before_output_transform() {
        let edits = Edits {
            exposure: 1.0,
            contrast: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            3
        );
    }

    #[test]
    fn hsl_adds_one_stage_when_non_neutral() {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[2].saturation = 30.0;
        let edits = Edits {
            hsl,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn neutral_hsl_adds_no_stage() {
        let edits = Edits {
            hsl: edit_model::Hsl::default(),
            saturation: 10.0,
            ..Edits::default()
        };
        // saturation stage + output transform only
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn saturation_adds_one_stage() {
        let edits = Edits {
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn vibrance_alone_adds_one_stage() {
        let edits = Edits {
            vibrance: -30.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn neutral_color_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn noise_reduction_runs_before_every_adjustment() {
        // Early placement is the whole point: denoise the source rather than fight noise
        // that a Shadows lift has already multiplied up. Geometry stays first so NR runs
        // on the cropped pixels.
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                color: 40.0,
                ..Default::default()
            },
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4500.0),
                ..Default::default()
            },
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec![
                "noise_reduction",
                "geometry",
                "white_balance",
                "exposure",
                "output_transform"
            ]
        );
    }

    #[test]
    fn noise_reduction_modifiers_alone_add_no_stage() {
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 0.0,
                detail: 100.0,
                smoothness: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            1
        );
    }

    #[test]
    fn vignette_runs_last_before_the_output_transform() {
        // A creative effect laid over the finished grade.
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec!["exposure", "saturation", "vignette", "output_transform"]
        );
    }

    #[test]
    fn vignette_modifiers_alone_add_no_stage() {
        let edits = Edits {
            vignette: edit_model::Vignette {
                amount: 0.0,
                midpoint: 10.0,
                roundness: -100.0,
                feather: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            1
        );
    }

    /// M10.11b: grain is the last scene-linear stage — after sharpening, which must not sharpen it.
    #[test]
    fn grain_runs_after_sharpening_and_before_the_output_transform() {
        let e = Edits {
            exposure: 0.5,
            sharpening: edit_model::Sharpening {
                amount: 40.0,
                ..Default::default()
            },
            grain: edit_model::Grain {
                amount: 30.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&e, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec!["exposure", "sharpening", "grain", "output_transform"]
        );
    }

    #[test]
    fn sharpening_runs_after_the_vignette_and_before_the_output_transform() {
        // Sharpening a vignetted frame is correct for a ratio-based model: darkened
        // corners sharpen proportionally less in absolute terms.
        let edits = Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec!["exposure", "vignette", "sharpening", "output_transform"]
        );
    }

    #[test]
    fn sharpening_modifiers_alone_add_no_stage() {
        let edits = Edits {
            sharpening: edit_model::Sharpening {
                amount: 0.0,
                radius: 3.0,
                detail: 0.0,
                masking: 90.0,
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            1
        );
    }

    #[test]
    fn local_contrast_runs_after_the_tone_curve_and_before_saturation() {
        // After the tone curve is mandatory: a local contrast computed before it would then
        // be re-mapped by it — the same argument that kept the vignette out of the prefix.
        // Before the colour stages so they see final tonality.
        let edits = Edits {
            exposure: 1.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: 20.0,
            },
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec![
                "exposure",
                "tone_curve",
                "local_contrast",
                "saturation",
                "output_transform"
            ]
        );
    }

    #[test]
    fn neutral_local_contrast_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            local_contrast: edit_model::LocalContrast::default(),
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            2
        );
    }

    #[test]
    fn local_contrast_is_in_the_tail() {
        // It must not join the prefix: it has to be downstream of the tone curve, and
        // putting it upstream would also make every Texture drag invalidate the noise
        // reduction cache.
        let (prefix, tail) = build_pipeline_split(
            &all_edits(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        );
        assert!(!prefix.stage_names().contains(&"local_contrast"));
        assert!(tail.stage_names().contains(&"local_contrast"));
    }

    #[test]
    fn tail_kernel_radius_covers_every_spatial_tail_stage() {
        let neutral = Edits::default();
        assert_eq!(tail_kernel_radius(&neutral, RenderScale::FULL), 0);

        // Sharpening alone at its default radius: ceil(3*1.0) + ceil(3*2.0) = 9.
        let sharp = Edits {
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&sharp, RenderScale::FULL), 9);

        // Clarity alone: wide_blur_reach(40.0) = 120, far wider than sharpening ever gets.
        let clarity = Edits {
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 50.0,
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&clarity, RenderScale::FULL), 120);

        // ⚠ Both: the **sum**, not the max — and this assertion used to say 120, with a
        // comment claiming each stage "reads its own neighbourhood of that stage's input".
        // That reasoning is wrong, and masking disproved it empirically: sharpening runs on
        // local contrast's *output*, so a wrong value at the region edge travels 120 px
        // through clarity and a further 9 px through sharpening. The old inset was ~9 px too
        // small and the region-equality test was passing only because the residual
        // differences happened to round the same way; a masked +0.6 EV pushed them across a
        // quantisation boundary and it failed.
        let both = Edits {
            sharpening: sharp.sharpening,
            local_contrast: clarity.local_contrast,
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&both, RenderScale::FULL), 129);

        // ⚠⚠ There used to be a worst-case bound here — `max_tail_kernel_radius`, asserted to
        // cover every representable edit set. **That premise is now false and the function is
        // gone.** Masks carry spatial adjustments and chain, so the reach is unbounded in the
        // number of masks; no constant bounds it. The replacement is that the *caller* derives
        // its margin from this function per render, which is what `Session::render_margin` does.
        //
        // What survives is the half that still means something: the reach must shrink with the
        // render scale, or a preview would over-reserve.
        assert!(
            tail_kernel_radius(&both, RenderScale::new(0.25))
                < tail_kernel_radius(&both, RenderScale::FULL),
            "the reach must shrink with the render scale"
        );
    }

    #[test]
    fn geometry_is_prepended_before_other_stages() {
        let edits = Edits {
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            3
        );
    }

    #[test]
    fn colour_grading_runs_after_hsl_and_before_the_masks() {
        // After HSL because HSL *corrects* colours that are in the photograph while a grade is
        // a look laid over the top — reversed, an HSL band tweak would act on the tint just
        // added. Before the masks because a mask "should act on the global look rather than be
        // re-mapped by it", and a grade is global look.
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        let edits = Edits {
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: 0.0,
                },
                ..Default::default()
            },
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                refine: Vec::new(),
                // ⚠ A LOOK slider: since M10.12b a mask's exposure is an offset at the head of the
                // tail, and what this test orders is the mask's own stage.
                adjustments: edit_model::LocalAdjustments {
                    saturation: 30.0,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).stage_names(),
            vec![
                "hsl",
                "color_grading",
                "mask",
                "vignette",
                "output_transform"
            ]
        );
    }

    #[test]
    fn colour_grading_modifiers_alone_add_no_stage() {
        let edits = Edits {
            color_grading: edit_model::ColorGrading {
                blending: 100.0,
                balance: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric()).len(),
            1
        );
    }
}

/// M10.12b — a mask's scene sliders as offsets, through the whole pipeline.
#[cfg(test)]
mod scene_offset_tail_tests {
    use crate::{build_pipeline, RenderScale, TailRendering};
    use core_image::ImageBuf;
    use edit_model::{
        Edits, LinearGradient, LocalAdjustments, Mask, MaskedAdjustment, RadialGradient,
    };

    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let s = y as f32 / (h - 1) as f32;
                img.set_pixel(x, y, [t * 1.2, 0.1 + 0.6 * s, 0.05 + 0.5 * (1.0 - t) * s]);
            }
        }
        img
    }

    fn render(e: &Edits) -> ImageBuf {
        build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).run(scene(48, 24))
    }

    fn names(e: &Edits) -> Vec<&'static str> {
        build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).stage_names()
    }

    /// The probe's global edit (`examples/probe_mask_offsets.rs`).
    fn probe_edit() -> Edits {
        Edits {
            contrast: 25.0,
            highlights: -50.0,
            shadows: 30.0,
            saturation: 15.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.25, 0.2], [0.75, 0.82], [1.0, 1.0]],
            },
            ..Default::default()
        }
    }

    fn everything(a: LocalAdjustments) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Linear(LinearGradient {
                from: [-1.0, 0.0],
                to: [-0.5, 0.0],
                invert: false,
            }),
            adjustments: a,
            ..Default::default()
        }
    }

    fn close(a: &ImageBuf, b: &ImageBuf, tol: f32) -> f32 {
        let worst = a
            .data()
            .iter()
            .zip(b.data())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(worst <= tol, "worst sample differs by {worst}");
        worst
    }

    /// ⚠⚠ **The probe as a test.** A mask covering everything with each scene slider renders like the
    /// global slider moved by the same amount — under the probe's global edit, and with none.
    #[test]
    fn a_full_mask_offset_is_the_global_slider() {
        let baseline = color::wb::Illuminant::default();
        for base in [probe_edit(), Edits::default()] {
            type Case = (
                &'static str,
                LocalAdjustments,
                fn(&mut Edits, color::wb::Illuminant),
            );
            let cases: Vec<Case> = vec![
                (
                    "exposure",
                    LocalAdjustments {
                        exposure: 1.0,
                        ..Default::default()
                    },
                    |e, _| e.exposure += 1.0,
                ),
                (
                    "contrast",
                    LocalAdjustments {
                        contrast: 30.0,
                        ..Default::default()
                    },
                    |e, _| e.contrast += 30.0,
                ),
                (
                    "highlights",
                    LocalAdjustments {
                        highlights: -40.0,
                        ..Default::default()
                    },
                    |e, _| e.highlights += -40.0,
                ),
                (
                    "shadows",
                    LocalAdjustments {
                        shadows: 35.0,
                        ..Default::default()
                    },
                    |e, _| e.shadows += 35.0,
                ),
                (
                    "whites",
                    LocalAdjustments {
                        whites: 20.0,
                        ..Default::default()
                    },
                    |e, _| e.whites += 20.0,
                ),
                (
                    "blacks",
                    LocalAdjustments {
                        blacks: -25.0,
                        ..Default::default()
                    },
                    |e, _| e.blacks += -25.0,
                ),
                (
                    "temp",
                    LocalAdjustments {
                        temp: 40.0,
                        ..Default::default()
                    },
                    |e, b| {
                        e.white_balance =
                            crate::shifted_white_balance(&e.white_balance, b, 40.0, 0.0)
                    },
                ),
                (
                    "tint",
                    LocalAdjustments {
                        tint: 20.0,
                        ..Default::default()
                    },
                    |e, b| {
                        e.white_balance =
                            crate::shifted_white_balance(&e.white_balance, b, 0.0, 20.0)
                    },
                ),
            ];
            for (name, offset, moved) in cases {
                let mut masked = base.clone();
                masked.masks.push(everything(offset));
                let mut global = base.clone();
                moved(&mut global, baseline);
                let (m, g) = (render(&masked), render(&global));
                assert!(
                    m.data()
                        .iter()
                        .zip(render(&base).data())
                        .any(|(a, b)| (a - b).abs() > 1e-3),
                    "fixture: {name} must move the picture"
                );
                close(&m, &g, 1e-5);
            }
        }
    }

    #[test]
    fn where_no_mask_covers_the_render_is_unchanged() {
        let mut e = probe_edit();
        e.masks.push(MaskedAdjustment {
            mask: Mask::Radial(RadialGradient {
                center: [0.15, 0.5],
                radius: [0.1, 0.2],
                feather: 0.0,
                invert: false,
            }),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
            ..Default::default()
        });
        let (with, without) = (render(&e), render(&probe_edit()));
        assert!(
            names(&e).contains(&"scene_offsets"),
            "fixture: the offset stage is in"
        );
        for y in 0..24 {
            for x in 24..48 {
                assert_eq!(with.pixel(x, y), without.pixel(x, y), "({x},{y})");
            }
        }
        assert_ne!(
            with.pixel(7, 12),
            without.pixel(7, 12),
            "fixture: inside it moves"
        );
    }

    #[test]
    fn no_scene_offset_keeps_the_four_stages() {
        let mut e = probe_edit();
        e.exposure = 0.5;
        let n = names(&e);
        assert!(
            n.contains(&"exposure") && !n.contains(&"scene_offsets"),
            "{n:?}"
        );
    }

    #[test]
    fn look_sliders_alone_keep_the_four_stages() {
        let mut e = probe_edit();
        e.exposure = 0.5;
        e.masks.push(everything(LocalAdjustments {
            saturation: 40.0,
            ..Default::default()
        }));
        let n = names(&e);
        assert!(n.contains(&"exposure") && n.contains(&"mask"), "{n:?}");
        assert!(!n.contains(&"scene_offsets"), "{n:?}");
    }

    /// Review Focus 1.
    #[test]
    fn a_switched_off_mask_adds_no_offset() {
        let mut e = probe_edit();
        let mut m = everything(LocalAdjustments {
            exposure: 1.0,
            ..Default::default()
        });
        m.disabled = true;
        e.masks.push(m);
        assert!(!names(&e).contains(&"scene_offsets"));
        assert_eq!(render(&e), render(&probe_edit()));
    }

    /// Review Focus 5: a mask's look sliders still apply beside its offsets — and its offsets no
    /// longer run again inside its `MaskStage`.
    #[test]
    fn a_mask_keeps_its_look_sliders() {
        let offset = everything(LocalAdjustments {
            exposure: 1.0,
            ..Default::default()
        });
        let both = everything(LocalAdjustments {
            exposure: 1.0,
            saturation: 50.0,
            ..Default::default()
        });
        let e1 = Edits {
            masks: vec![offset],
            ..probe_edit()
        };
        let e2 = Edits {
            masks: vec![both],
            ..probe_edit()
        };
        let n = names(&e2);
        assert!(n.contains(&"scene_offsets") && n.contains(&"mask"), "{n:?}");
        assert_ne!(render(&e1), render(&e2), "saturation must still apply");
        // The exposure applies ONCE: the look-only mask's saturation, on top of the global +1 EV.
        let mut global = probe_edit();
        global.exposure += 1.0;
        global.masks.push(everything(LocalAdjustments {
            saturation: 50.0,
            ..Default::default()
        }));
        close(&render(&e2), &render(&global), 1e-5);
    }

    /// Review Focus 4: under an as-shot global, a legacy mask Kelvin renders as that white balance.
    #[test]
    fn a_legacy_kelvin_renders_as_its_shift() {
        let wb = edit_model::WhiteBalance {
            kelvin: Some(6500.0),
            tint: 5.0,
        };
        let legacy = Edits {
            masks: vec![everything(LocalAdjustments {
                white_balance: wb,
                ..Default::default()
            })],
            ..Default::default()
        };
        let global = Edits {
            white_balance: wb,
            ..Default::default()
        };
        close(&render(&legacy), &render(&global), 1e-5);
    }

    // ---- b2: the reference photo for pixel-reading masks with a scene offset ----

    /// A dark band, lightness [0, 40] with a 2.5-wide falloff (feather 5; lightness is the
    /// sRGB-encoded luminance × 100).
    fn band(a: LocalAdjustments) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Luminance(edit_model::LuminanceRange {
                low: 0.0,
                high: 40.0,
                feather: 5.0,
                invert: false,
            }),
            adjustments: a,
            ..Default::default()
        }
    }

    fn uniform(rgb: [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set_pixel(x, y, rgb);
            }
        }
        img
    }

    fn render_on(e: &Edits, img: &ImageBuf) -> ImageBuf {
        build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).run(img.clone())
    }

    fn chroma(p: [f32; 3]) -> f32 {
        p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])
    }

    fn masks_of(masks: Vec<MaskedAdjustment>) -> Edits {
        Edits {
            masks,
            ..Default::default()
        }
    }

    /// ⚠⚠ **The chase made impossible.** +2 EV lifts the patch (lightness ≈ 25) out of the band that
    /// selected it (≈ 50). Selecting ONCE, on the reference photo, the mask's Saturation still lands
    /// on it; selecting from its own base after the offset, it would land on nothing.
    #[test]
    fn a_range_mask_selects_the_same_pixels_for_its_offset_and_its_look() {
        let patch = uniform([0.06, 0.05, 0.04]);
        let ev_only = render_on(
            &masks_of(vec![band(LocalAdjustments {
                exposure: 2.0,
                ..Default::default()
            })]),
            &patch,
        );
        let ev_sat = render_on(
            &masks_of(vec![band(LocalAdjustments {
                exposure: 2.0,
                saturation: 80.0,
                ..Default::default()
            })]),
            &patch,
        );
        let plain = render_on(&Edits::default(), &patch);
        assert!(
            ev_only.pixel(8, 8)[0] > plain.pixel(8, 8)[0] + 0.15,
            "fixture check: the band selected the patch and the offset lifted it"
        );
        let (a, b) = (chroma(ev_sat.pixel(8, 8)), chroma(ev_only.pixel(8, 8)));
        assert!(
            a > 1.3 * b,
            "the mask's Saturation lost the pixels its offset moved: {a} vs {b}"
        );
    }

    /// The reference is the GLOBALLY EDITED photo: a global −2 EV brings mid-grey (lightness ≈ 46,
    /// outside the band) into it, so the mask's +1 makes the global −1.
    #[test]
    fn a_range_mask_with_an_offset_selects_on_the_globally_edited_photo() {
        let grey = uniform([0.18; 3]);
        let masked = Edits {
            exposure: -2.0,
            masks: vec![band(LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            })],
            ..Default::default()
        };
        let global = Edits {
            exposure: -1.0,
            ..Default::default()
        };
        let unmasked = Edits {
            exposure: -2.0,
            ..Default::default()
        };
        assert!(
            (render_on(&global, &grey).pixel(8, 8)[0] - render_on(&unmasked, &grey).pixel(8, 8)[0])
                .abs()
                > 0.05,
            "fixture check: one stop apart"
        );
        close(&render_on(&masked, &grey), &render_on(&global, &grey), 1e-5);
    }

    /// What each mask's stages say about the reference: `(has_reference, slot per mask)` from the
    /// offsets stage, and `takes_reference_coverage` per mask stage, in order.
    fn reference_facts(e: &Edits) -> (Option<(bool, Vec<bool>)>, Vec<bool>) {
        let stages =
            build_pipeline(e, RenderScale::FULL, TailRendering::colorimetric()).into_stages();
        let offsets = stages.iter().find_map(|s| {
            s.scene_offsets().map(|o| {
                (
                    o.has_reference(),
                    (0..e.masks.len())
                        .map(|i| o.slot_for(i).is_some())
                        .collect(),
                )
            })
        });
        let masks = stages
            .iter()
            .filter(|s| s.name() == "mask")
            .map(|s| s.takes_reference_coverage())
            .collect();
        (offsets, masks)
    }

    /// The cost is paid only where it is needed: a positional mask's offsets build no reference, a
    /// range mask without an offset takes no slot, and a range mask with one takes ITS slot.
    #[test]
    fn the_reference_is_built_only_for_a_pixel_reading_mask_with_an_offset() {
        let ev = LocalAdjustments {
            exposure: 1.0,
            ..Default::default()
        };
        let sat = LocalAdjustments {
            saturation: 30.0,
            ..Default::default()
        };
        let both = LocalAdjustments {
            exposure: 1.0,
            saturation: 30.0,
            ..Default::default()
        };
        assert_eq!(
            reference_facts(&masks_of(vec![everything(ev.clone())])),
            (Some((false, vec![false])), vec![])
        );
        assert_eq!(
            reference_facts(&masks_of(vec![everything(ev), band(sat)])),
            (Some((false, vec![false, false])), vec![false])
        );
        assert_eq!(
            reference_facts(&masks_of(vec![band(both.clone())])),
            (Some((true, vec![true])), vec![true])
        );
        assert_eq!(
            reference_facts(&masks_of(vec![everything(both.clone()), band(both)])),
            (Some((true, vec![false, true])), vec![false, true])
        );
    }

    #[test]
    fn a_disabled_range_mask_with_an_offset_builds_no_reference_and_changes_nothing() {
        let off = MaskedAdjustment {
            disabled: true,
            ..band(LocalAdjustments {
                exposure: 2.0,
                saturation: 80.0,
                ..Default::default()
            })
        };
        let lin = everything(LocalAdjustments {
            exposure: 0.5,
            ..Default::default()
        });
        assert_eq!(
            reference_facts(&masks_of(vec![lin.clone(), off.clone()])),
            (Some((false, vec![false, false])), vec![])
        );
        let patch = uniform([0.06, 0.05, 0.04]);
        close(
            &render_on(&masks_of(vec![lin.clone(), off]), &patch),
            &render_on(&masks_of(vec![lin]), &patch),
            0.0,
        );
    }

    /// ⚠ The band intersected with a ramp across the middle, so the coverage depends on POSITION too:
    /// a plane indexed at the wrong pixel would show.
    fn reference_edit() -> Edits {
        let mut m = band(LocalAdjustments {
            exposure: 1.5,
            saturation: 60.0,
            ..Default::default()
        });
        m.refine = vec![edit_model::Refinement {
            op: edit_model::MaskOp::Intersect,
            mask: Mask::Linear(LinearGradient {
                from: [0.3, 0.0],
                to: [0.7, 0.0],
                invert: false,
            }),
        }];
        Edits {
            exposure: -1.0,
            masks: vec![m],
            ..Default::default()
        }
    }

    /// A region render — zoomed in, panning — selects exactly as the whole frame does there: the
    /// slot's plane and the reference are both region-sized.
    #[test]
    fn a_region_render_of_a_range_mask_with_an_offset_is_the_whole_frames_crop() {
        let e = reference_edit();
        let whole = scene(48, 24);
        let (_, tail) = crate::build_pipeline_split_in_region(
            &e,
            RenderScale::FULL,
            None,
            TailRendering::colorimetric(),
        );
        let want = tail.run(whole.clone()).crop(10, 6, 20, 12);
        let region = crate::Region {
            full_w: 48,
            full_h: 24,
            x0: 10,
            y0: 6,
        };
        let (_, tail) = crate::build_pipeline_split_in_region(
            &e,
            RenderScale::FULL,
            Some(region),
            TailRendering::colorimetric(),
        );
        let got = tail.run(whole.crop(10, 6, 20, 12));
        assert_ne!(
            render(&e).data(),
            render(&Edits::default()).data(),
            "fixture check: the mask does something"
        );
        close(&got, &want, 1e-6);
    }

    /// The grade range view runs the tail as two halves: the offsets stage in the first, the
    /// slot-fed mask in the second — they must share ONE slot.
    #[test]
    fn the_tapped_halves_share_the_slot() {
        let e = reference_edit();
        let (_, halves) = crate::build_pipeline_split_tapped(
            &e,
            RenderScale::FULL,
            None,
            None,
            crate::RenderSpec::default(),
            None,
            TailRendering::colorimetric(),
        );
        let (_, tail) = crate::build_pipeline_split_in_region(
            &e,
            RenderScale::FULL,
            None,
            TailRendering::colorimetric(),
        );
        let img = scene(48, 24);
        close(
            &halves.from_grade.run(halves.before_grade.run(img.clone())),
            &tail.run(img),
            0.0,
        );
    }

    /// A gating brush with an offset selects on the reference, so the offsets stage is where its map
    /// is built — and it reports it, so the next frame is served from the cache.
    #[test]
    fn a_gating_brush_with_an_offset_deposits_its_map_from_the_reference() {
        let brush = edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: vec![[0.5, 0.5]],
                radius: 0.3,
                hardness: 100.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 35.0,
                ..Default::default()
            }],
            invert: false,
        };
        let e = masks_of(vec![MaskedAdjustment {
            mask: Mask::Brush(brush),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
            ..Default::default()
        }]);
        let cov = std::sync::Arc::new(crate::BrushCoverage::with_slots(&[1]));
        let (_, tail) = crate::build_pipeline_split_in_region_with_brush(
            &e,
            RenderScale::FULL,
            None,
            Some(&cov),
            crate::RenderSpec::default(),
            None,
            TailRendering::colorimetric(),
        );
        assert_eq!(
            tail.stage_names().iter().filter(|n| **n == "mask").count(),
            0,
            "fixture check: offsets only"
        );
        let _ = tail.run(scene(48, 24));
        assert!(
            cov.take_built(0, 0).is_some(),
            "the reference's map was built and not reported"
        );
    }
}

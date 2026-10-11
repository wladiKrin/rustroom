//! The prefix: its steps, their cache keys, and the whole-prefix key.

use crate::{
    stages, DehazeStage, GeometryStage, LensCorrectionStage, MaskStage, NoiseReductionStage,
    PipelineStage, RedEyeStage, RenderScale, Resampler, SpotRemovalStage,
};
use edit_model::Edits;

/// One prefix stage's parameters, identifying the image that stage produces.
///
/// ⚠ **Adding a prefix stage means adding a variant here.** [`build_prefix_steps`] returns
/// `(key, stage)` pairs, so a prefix stage with no key does not compile. That is deliberate:
/// a prefix stage missing from a cache key means the preview cache serves stale pixels with
/// nothing failing, and this is the guard that makes forgetting impossible rather than merely
/// caught by a test.
///
/// ⚠⚠ The two **resampling** stages carry the [`Resampler`] as well as their edits, and it is
/// not decoration: the kernel is a `Settings` choice rather than an `Edits` field, so nothing
/// else in the key can see it change. Without it, switching the setting mid-session would serve
/// checkpoints rendered with the old kernel and the control would look inert — the failure
/// `apply_camera_profile` had to clear `BufferCache` for, solved here structurally instead.
///
/// ⚠ Deliberately **not** a field of [`PrefixKey`] beside `scale`: on a photo with no geometry,
/// no perspective and no lens profile nothing resamples, so changing the kernel must invalidate
/// nothing. Putting it on the steps that actually use it is what gives that for free.
///
/// ⚠ **No longer `Copy`**, because a spot list is a `Vec`. `PrefixKey::upto` clones instead —
/// a handful of five-field structs per render, against re-running lens correction.
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixStepKey {
    LensCorrection(edit_model::LensCorrection, Resampler),
    /// M10.11a — the film negative's inversion: its whole setting, so a pick, a re-measure or a
    /// contrast change each miss the cache.
    Negative(edit_model::Negative),
    SpotRemoval(Vec<edit_model::Spot>),
    /// M9.11 — the whole eye list, so any change to any eye misses the cache (`SpotRemoval`'s rule).
    RedEye(Vec<edit_model::Eye>),
    /// ⚠ Carries the perspective since M7.3b: the keystone is part of this stage's one resample.
    Geometry(edit_model::Geometry, edit_model::Perspective, Resampler),
    NoiseReduction(edit_model::NoiseReduction),
    Dehaze(edit_model::Dehaze),
    /// One mask's capture corrections.
    ///
    /// ⚠⚠ Holds the **projection**, never the whole `MaskedAdjustment` — see
    /// [`capture_projection`]. Keyed on the whole thing, a masked-exposure drag would re-run a
    /// whole-buffer denoise.
    ///
    /// ⚠ **Boxed**, and clippy is why: at 296 bytes against the next-largest variant's 25, an
    /// inline `MaskedAdjustment` would make **every** entry in a `PrefixKey`'s `Vec` that size —
    /// including the zero-sized ones. The same `large_enum_variant` answer `ActionKind::Sync`
    /// already gives.
    MaskedCapture(Box<edit_model::MaskedAdjustment>),
    /// The one defringe step (M9.6): the global settings and the **projection** of every enabled
    /// mask carrying a local defringe — its shape, its refinements and its slider, nothing else,
    /// so a masked-exposure drag does not re-run it. ⚠ Geometry is not here and need not be: the
    /// key is cumulative and the geometry step precedes this one.
    Defringe(edit_model::Defringe, Vec<edit_model::MaskedAdjustment>),
}

/// One prefix stage together with the key identifying its output, so the renderer can cache
/// each stage's result independently.
pub struct PrefixStep {
    pub key: PrefixStepKey,
    pub stage: Box<dyn PipelineStage>,
}

/// The size the prefix leaves a `w` × `h` decode at — the final picture's, since geometry is the
/// prefix's one resize — without touching a pixel (M10.10b).
///
/// ⚠ Folded over the steps' own [`PipelineStage::output_dims`] rather than re-deriving the crop, so
/// a future stage that changes size is predicted too; `every_prefix_stages_output_dims_match_what_
/// it_produces` keeps each answer true. Built without the lens, which keeps the size.
pub fn prefix_output_dims(edits: &Edits, w: usize, h: usize) -> (usize, usize) {
    build_prefix_steps(edits, RenderScale::FULL, None, Resampler::default(), None)
        .iter()
        .fold((w, h), |(w, h), step| step.stage.output_dims(w, h))
}

/// The prefix as an ordered list of `(key, stage)` pairs.
///
/// An inactive stage produces no step — exactly as it produces no stage in the pipeline today.
/// That is what makes the checkpoint after an inactive stage *be* the checkpoint before it:
/// the cumulative keys are equal, so nothing extra is stored.
pub fn build_prefix_steps(
    edits: &Edits,
    scale: RenderScale,
    lens: Option<lens_profile::LensShot>,
    resampler: Resampler,
    airlight: Option<[f32; 3]>,
) -> Vec<PrefixStep> {
    let mut steps: Vec<PrefixStep> = Vec::new();

    // ⚠⚠ **THE ORDER, SINCE M7.3b: lens → NR → dehaze → spots → geometry → masked capture.** It
    // used to be lens → perspective → spots → geometry → NR → dehaze, and a straighten, crop or
    // keystone drag then re-ran NR, dehaze and the masked capture stages on every frame, because
    // they sat downstream of geometry in the prefix cache. With the one geometric resample last,
    // only a geometry edit invalidates it. Measured on DSCF6291 at the fit rung (GPU path): a
    // straighten drag **137 → 35 ms**, a keystone drag 170 → 36.
    //
    // ⚠⚠ **It is a SPEED trade, not an image-quality win — the IQ argument was measured and
    // RETRACTED.** The brainstorm claimed NR ahead of the resamplers would see cleaner, less
    // correlated noise. On DSCF6708 (ISO 3200) the residual grain's lag-1 correlation was identical
    // in all four orders tried, and NR *first* left 4-8% MORE grain at the same slider (the
    // resample's own softening had been helping). The price is on the other drags: NR 102 → 161 ms
    // and dehaze 54 → 106, which now re-run the resample behind them. Lens stays FIRST because NR
    // ahead of it cost a further 92 ms per NR drag for no measurable gain. Numbers and the
    // alternatives are in the M7.3b journal entry.

    // Lens correction first. ⚠ Ahead of geometry: the calibration is defined in the uncropped,
    // unrotated sensor frame. A missing corrector — an unlisted lens, or a JPEG — skips the step
    // silently, which is the ordinary case.
    let lens_correction = edits.applied_lens_correction();
    if lens_correction.is_active() {
        if let Some(c) = lens {
            steps.push(PrefixStep {
                key: PrefixStepKey::LensCorrection(lens_correction, resampler),
                stage: Box::new(LensCorrectionStage::new(
                    c,
                    lens_profile::Corrections {
                        distortion: lens_correction.distortion,
                        tca: lens_correction.tca,
                        vignetting: lens_correction.vignetting,
                        scale: lens_correction.scale,
                    },
                    resampler,
                )),
            });
        }
    }

    // ⚠⚠ M10.11a: a film negative becomes a positive here — right AFTER lens correction, whose
    // vignetting multiplies the negative and must be divided out first, and BEFORE everything that
    // reads tone: noise reduction then sees the positive's grain, and dehaze's airlight replay (which
    // includes this step) measures the positive's haze. A per-pixel step on the CPU; it has no GPU
    // twin, and in the prefix it is computed once and cached, so no tail drag re-runs it.
    if edits.negative.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Negative(edits.negative),
            stage: Box::new(crate::stages::negative::NegativeStage::new(&edits.negative)),
        });
    }

    // Noise reduction before every tonal stage: denoising the source is far easier than
    // denoising after Exposure, Shadows and the tone curve have amplified the grain. `is_active`
    // rather than a `!= default()` check — Detail and Smoothness are modifiers and must not insert
    // a stage on their own.
    if edits.noise_reduction.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::NoiseReduction(edits.noise_reduction),
            stage: Box::new(NoiseReductionStage::new(&edits.noise_reduction, scale)),
        });
    }

    // Dehaze: a correction of the capture, so upstream of every tonal stage. ⚠ AFTER lens
    // correction, because it must see the vignetting correction: a darkened corner reads as less
    // haze. After NR, because dehaze is a contrast stretch that amplifies noise.
    //
    // ⚠ Being in the prefix is what makes a *frame-global* airlight possible at all: the prefix
    // always runs whole-buffer, so no region plumbing and no margin growth is needed.
    if edits.dehaze.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Dehaze(edits.dehaze),
            // ⚠⚠ A supplied airlight is what makes this step region-scopable. Without one the
            // stage estimates from the image it is handed, which under a scoped run is a slice —
            // and the photograph would then change as you pan.
            stage: match airlight {
                Some(a) => Box::new(DehazeStage::with_airlight(&edits.dehaze, scale, a)),
                None => Box::new(DehazeStage::new(&edits.dehaze, scale)),
            },
        });
    }

    // ⚠⚠ Spots in post-lens, **pre-perspective** space since M7.3b. Before it they sat between
    // perspective and geometry, and fusing the two resamples deletes that buffer — a pixel edit
    // cannot live inside a coordinate map. Anchored here, a spot also stays on its blemish when
    // the keystone changes, which it did not before. After dehaze so a spot drag re-runs only
    // this and the geometry step.
    if !edits.spots.is_empty() {
        steps.push(PrefixStep {
            key: PrefixStepKey::SpotRemoval(edits.spots.clone()),
            stage: Box::new(SpotRemovalStage::new(&edits.spots)),
        });
    }

    // ⚠ M9.11: red eye right after spots, in the same pre-perspective space — so `SpotMap` draws
    // both, and a heal patch beside a pupil samples the uncorrected neighbour it was placed against.
    if !edits.eyes.is_empty() {
        steps.push(PrefixStep {
            key: PrefixStepKey::RedEye(edits.eyes.clone()),
            stage: Box::new(RedEyeStage::new(&edits.eyes)),
        });
    }

    // Geometry LAST — orientation, perspective, straighten and crop, in ONE resample (M7.3b) —
    // so all adjustments downstream operate on the final cropped pixels. ⚠ Inserted for a
    // perspective alone, too: the keystone lives in this stage now.
    if edits.geometry != edit_model::Geometry::default() || edits.perspective.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Geometry(edits.geometry, edits.perspective, resampler),
            stage: Box::new(
                GeometryStage::new(&edits.geometry, resampler).with_perspective(edits.perspective),
            ),
        });
    }

    // ⚠⚠ Defringe (M9.6): ONE step for the global settings and every mask's local slider, AFTER
    // geometry — in the frame the masks live in, which is what lets a mask at −100 protect its
    // area from the global amount inside one computation — and so a defringe drag re-runs only
    // this and the masked captures, never the geometry resample. Inserted for a local slider alone,
    // since a positive local defringes with the global amount at 0.
    let locals: Vec<edit_model::MaskedAdjustment> = edits
        .masks
        .iter()
        .filter(|m| !m.disabled && m.adjustments.defringe != 0.0)
        .map(defringe_projection)
        .collect();
    if edits.defringe.is_active() || locals.iter().any(|m| m.adjustments.defringe > 0.0) {
        let stages = locals
            .iter()
            .map(|p| {
                (
                    MaskStage::new_capture(p, scale, &edits.geometry, &[], None),
                    p.adjustments.defringe,
                )
            })
            .collect();
        steps.push(PrefixStep {
            key: PrefixStepKey::Defringe(edits.defringe, locals),
            stage: Box::new(stages::defringe::DefringeStage::new(
                edits.defringe,
                stages,
                scale,
            )),
        });
    }

    // ⚠ After the global pair, mirroring the tail's own rule that a mask acts on the global look
    // rather than being re-mapped by it.
    //
    // ⚠ One step per mask that actually carries a capture correction; a mask with only tail
    // adjustments costs nothing here.
    for m in &edits.masks {
        // ⚠ A switched-off mask contributes nothing anywhere — view, export, prefix cache key.
        if m.disabled {
            continue;
        }
        let p = capture_projection(m);
        if !p.adjustments.noise_reduction.is_active() && !p.adjustments.dehaze.is_active() {
            continue;
        }
        steps.push(PrefixStep {
            // ⚠⚠ The key holds **the projection the stage was built from**, so the key and the
            // stage cannot drift — the property this function exists to guarantee.
            key: PrefixStepKey::MaskedCapture(Box::new(p.clone())),
            // ⚠⚠ **`None` for the prebuilt coverage, and it must stay that way.** The render
            // engine's `BrushCache` is sized from the *prefix output's* dimensions, which do not
            // exist until the prefix has run — so a prebuilt map is unavailable here even in
            // principle. `MaskStage::apply` rasterizes its own when handed `None`, and with
            // `region: None` that is the whole frame at exactly the right size. **It costs
            // nothing per frame because this whole step is cached**: the rasterization happens
            // when the mask's shape or its capture sliders change, not on every render.
            stage: Box::new(MaskStage::new_capture(
                &p,
                scale,
                &edits.geometry,
                &[],
                airlight,
            )),
        });
    }

    steps
}

/// Everything a cached prefix checkpoint depends on: the parameters of the prefix stages that
/// have run so far, plus the render scale.
///
/// The key is **derived from** [`build_prefix_steps`] rather than written alongside it, so a
/// prefix stage cannot be missing from it — see [`PrefixStepKey`]. The tests in this module
/// additionally assert that every *downstream* edit field leaves this key unchanged.
///
/// Compared with `==` rather than hashed, so there is no collision risk. The `f32` is safe in
/// a `PartialEq`: the only pathological value is `NaN`, which compares unequal to itself and
/// so fails toward a cache *miss*.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixKey {
    /// The keys of the prefix stages that have run, in order.
    steps: Vec<PrefixStepKey>,
    /// Included because it sets the noise-reduction and dehaze radii.
    scale: f32,
}

impl PrefixKey {
    /// The key for the output of the *whole* prefix.
    pub fn new(edits: &Edits, scale: RenderScale, resampler: Resampler) -> Self {
        // ⚠ `None` is correct and must not be "fixed": the key is derived from the *edits*, and
        // `PrefixStepKey::LensCorrection` carries them. A corrector is needed to build the stage,
        // never to key it. Consequence, stated rather than discovered: with no corrector the step
        // is skipped, so this key can be one entry longer than the real prefix — harmless,
        // because whether a corrector exists is constant for a photo and the engine is per photo.
        // ⚠ `None` for the airlight: it is derived from `edits`, which this key already covers,
        // so carrying it here would be a second copy of a fact the key already holds.
        let steps = build_prefix_steps(edits, scale, None, resampler, None);
        Self::upto(&steps, steps.len(), scale)
    }

    /// How many prefix steps this key covers — checkpoint 0 is the bare downscale, so a key of
    /// depth `n` is the output of the first `n` steps.
    ///
    /// ⚠⚠ **Derived from the key rather than carried beside it.** A cache that stored a depth
    /// alongside a key would be two facts that can disagree, which is the shape
    /// `render_max_dim` and `Session::geometry_mode` were each deleted for.
    pub fn depth(&self) -> usize {
        self.steps.len()
    }

    /// The key for the checkpoint after the first `n` prefix stages have run.
    ///
    /// `n == 0` is the checkpoint straight after the downscale, before any stage. `n` beyond
    /// the end saturates, so a caller cannot key a checkpoint that does not exist.
    pub fn upto(steps: &[PrefixStep], n: usize, scale: RenderScale) -> Self {
        Self {
            steps: steps[..n.min(steps.len())]
                .iter()
                .map(|s| s.key.clone())
                .collect(),
            scale: scale.get(),
        }
    }
}

/// A mask as the defringe step reads it: its shape, its refinements and its local defringe.
fn defringe_projection(m: &edit_model::MaskedAdjustment) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: m.mask.clone(),
        refine: m.refine.clone(),
        adjustments: edit_model::LocalAdjustments {
            defringe: m.adjustments.defringe,
            ..Default::default()
        },
        disabled: false,
    }
}

/// A mask reduced to just its capture corrections — what the prefix stage is built from **and**
/// what its cache key holds, so the two cannot drift.
///
/// ⚠⚠ **The projection is what stops the prefix cache thrashing.** Keyed on the whole
/// [`edit_model::MaskedAdjustment`], every masked-*exposure* drag would invalidate the checkpoint
/// and re-run a whole-buffer noise reduction — 117 ms at native, for a slider that has nothing to
/// do with it. `the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest` pins
/// both directions.
fn capture_projection(m: &edit_model::MaskedAdjustment) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: m.mask.clone(),
        // ⚠⚠ **The refinements are part of the SHAPE, so they belong in the key.** Left out, a
        // subtracted or intersected component would change what the masked noise reduction and
        // dehaze cover while the cached prefix checkpoint stayed valid — a stale picture at
        // native zoom with nothing on screen to announce it.
        //
        // ⚠ The compiler forced this decision rather than leaving it to be remembered: this is a
        // struct literal with no `..`, so the new field was an `E0063` right here.
        refine: m.refine.clone(),
        adjustments: edit_model::LocalAdjustments {
            noise_reduction: m.adjustments.noise_reduction,
            dehaze: m.adjustments.dehaze,
            ..Default::default()
        },
        disabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::all_edits;
    use crate::{build_pipeline, build_pipeline_split, TailRendering};
    use core_image::ImageBuf;

    /// ⚠⚠ Total over the **whole prefix** rather than over `GeometryStage` alone, because the
    /// identity default is what every other stage inherits — and a default that is silently wrong
    /// for a future dim-changing stage is exactly the trap this guards. A region-scoped render
    /// takes the frame's dimensions from this prediction, so a stage that lies here rasterises
    /// every brush mask at the wrong size.
    #[test]
    fn every_prefix_stages_output_dims_match_what_it_produces() {
        let mut src = ImageBuf::new(37, 23);
        for y in 0..23 {
            for x in 0..37 {
                src.set_pixel(x, y, [x as f32 / 37.0, y as f32 / 23.0, 0.5]);
            }
        }
        let steps = build_prefix_steps(
            &all_edits(),
            RenderScale::new(1.0),
            None,
            Resampler::default(),
            None,
        );
        assert!(
            steps.len() >= 4,
            "fixture check: the fixture must build a real prefix, got {}",
            steps.len()
        );
        let mut img = src;
        for step in &steps {
            let predicted = step.stage.output_dims(img.width(), img.height());
            let out = step.stage.apply(img.clone());
            assert_eq!(
                predicted,
                (out.width(), out.height()),
                "`{}` predicted {predicted:?} and produced {:?}",
                step.stage.name(),
                (out.width(), out.height())
            );
            img = out;
        }
    }

    #[test]
    fn prefix_key_ignores_every_downstream_field() {
        // Proves every edit field is either in the key or downstream of the checkpoint.
        // If a field is added to the prefix without being added to PrefixKey, the cache
        // would silently serve stale pixels; this is what stands in the way.
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL, Resampler::default());

        let mut hsl = edit_model::Hsl::default();
        hsl.bands[3].hue = 40.0;
        let downstream = [
            Edits {
                white_balance: edit_model::WhiteBalance {
                    kelvin: Some(3500.0),
                    tint: -20.0,
                },
                ..base.clone()
            },
            Edits {
                exposure: 2.0,
                ..base.clone()
            },
            Edits {
                contrast: 50.0,
                ..base.clone()
            },
            Edits {
                highlights: -50.0,
                ..base.clone()
            },
            Edits {
                shadows: 50.0,
                ..base.clone()
            },
            Edits {
                whites: 30.0,
                ..base.clone()
            },
            Edits {
                blacks: -30.0,
                ..base.clone()
            },
            Edits {
                tone_curve: edit_model::ToneCurve {
                    points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
                },
                ..base.clone()
            },
            Edits {
                saturation: 40.0,
                ..base.clone()
            },
            Edits {
                vibrance: 40.0,
                ..base.clone()
            },
            Edits {
                hsl,
                ..base.clone()
            },
            Edits {
                color_grading: edit_model::ColorGrading {
                    highlights: edit_model::GradeWheel {
                        hue: 30.0,
                        saturation: 50.0,
                        luminance: 20.0,
                    },
                    blending: 10.0,
                    balance: 60.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 60.0,
                    clarity: -40.0,
                },
                ..base.clone()
            },
            Edits {
                vignette: edit_model::Vignette {
                    amount: -70.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 80.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            // ⚠ A look is tail-only, so it must NOT disturb the prefix key — otherwise every
            // strength drag would throw away the cached prefix and re-run demosaic and geometry.
            Edits {
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space: edit_model::LutSpace::Linear,
                    strength: 40.0,
                },
                ..base.clone()
            },
        ];
        for e in downstream {
            assert_eq!(
                PrefixKey::new(&e, RenderScale::FULL, Resampler::default()),
                k,
                "a downstream field changed the prefix key"
            );
        }
    }

    #[test]
    fn prefix_key_changes_with_prefix_fields_and_scale() {
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL, Resampler::default());

        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&geo, RenderScale::FULL, Resampler::default()),
            k
        );

        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&nr, RenderScale::FULL, Resampler::default()),
            k
        );

        let dh = Edits {
            dehaze: edit_model::Dehaze { amount: 40.0 },
            ..base.clone()
        };
        assert_ne!(
            PrefixKey::new(&dh, RenderScale::FULL, Resampler::default()),
            k
        );

        // Scale changes the noise-reduction radii, so it belongs in the key.
        assert_ne!(
            PrefixKey::new(&base, RenderScale::new(0.25), Resampler::default()),
            k
        );
    }

    /// ⚠⚠ **What makes the resampler a live setting rather than one needing a restart.** The
    /// kernel lives in `Settings`, not in `Edits`, so nothing else in the key can notice it
    /// change — and a served checkpoint rendered with the old kernel is exactly how a control
    /// comes to look inert. `apply_camera_profile` had to clear `BufferCache` by hand for the
    /// same hazard; keying it makes the invalidation structural instead.
    ///
    /// ⚠ And the *other half*, which is the reason it is on the steps rather than beside
    /// `scale`: on a photo that resamples nothing, changing the kernel must invalidate nothing.
    #[test]
    fn the_resampler_changes_the_key_only_when_something_resamples() {
        let straightened = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_ne!(
            PrefixKey::new(&straightened, RenderScale::FULL, Resampler::Bilinear),
            PrefixKey::new(&straightened, RenderScale::FULL, Resampler::Lanczos3),
            "a straighten resamples, so its checkpoint depends on the kernel"
        );

        // ⚠ Not `Edits::default()`: a *fully neutral* photo has an empty prefix, so the keys
        // would agree whatever the rule. This one has a live prefix stage that resamples
        // nothing, which is the case the assertion is actually about.
        let denoised = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !build_prefix_steps(
                &denoised,
                RenderScale::FULL,
                None,
                Resampler::Bilinear,
                None
            )
            .is_empty(),
            "fixture check: this photo must have a prefix stage to key at all"
        );
        assert_eq!(
            PrefixKey::new(&denoised, RenderScale::FULL, Resampler::Bilinear),
            PrefixKey::new(&denoised, RenderScale::FULL, Resampler::Lanczos3),
            "nothing resamples here, so the kernel must not invalidate the checkpoint"
        );
    }

    #[test]
    fn a_truncated_prefix_key_ignores_the_stages_below_it() {
        // The whole feature rests on this: two edit sets that agree on the first n prefix
        // stages must share the checkpoint after stage n, however much they differ deeper.
        //
        // ⚠ Varies the LAST step (the masked capture correction) so that as many checkpoints as
        // possible are shared. Since M7.3b dehaze is second, and varying it would leave this
        // checking only two.
        let a = all_edits();
        let mut b = a.clone();
        b.masks[0].adjustments.noise_reduction.luminance = 95.0;
        let steps_a = build_prefix_steps(&a, RenderScale::FULL, None, Resampler::default(), None);
        let steps_b = build_prefix_steps(&b, RenderScale::FULL, None, Resampler::default(), None);
        assert_eq!(steps_a.len(), 8, "the negative (M10.11a) first");

        // Same up to and including the geometry and the defringe (M9.6)...
        for n in 0..=7 {
            assert_eq!(
                PrefixKey::upto(&steps_a, n, RenderScale::FULL),
                PrefixKey::upto(&steps_b, n, RenderScale::FULL),
                "checkpoint {n} should be shared"
            );
        }
        // ...and different once the masked capture correction has run.
        assert_ne!(
            PrefixKey::upto(&steps_a, 8, RenderScale::FULL),
            PrefixKey::upto(&steps_b, 8, RenderScale::FULL)
        );

        // The full key is the deepest truncation, so `new` and `upto` cannot disagree.
        assert_eq!(
            PrefixKey::new(&a, RenderScale::FULL, Resampler::default()),
            PrefixKey::upto(&steps_a, steps_a.len(), RenderScale::FULL)
        );

        // Scale is part of every checkpoint, including the bare seed: it sets the radii of the
        // stages that follow.
        assert_ne!(
            PrefixKey::upto(&steps_a, 0, RenderScale::FULL),
            PrefixKey::upto(&steps_a, 0, RenderScale::new(0.25))
        );
    }

    #[test]
    fn the_prefix_holds_the_capture_stages_and_the_capture_masks() {
        // The tripwire: if a stage is later inserted into the prefix region, this fails and
        // forces PrefixKey to be updated with its parameters.
        //
        // ⚠ No lens profile is supplied here, so `lens_correction` is absent by construction.
        // ⚠ There is no `perspective` step since M7.3b: the fixture's keystone is inside
        // `geometry`, which now comes LAST among the global steps.
        let (prefix, tail) = build_pipeline_split(
            &all_edits(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        );
        assert_eq!(
            prefix.stage_names(),
            vec![
                // ⚠ M10.11a: right after lens, which this fixture has no profile for.
                "negative",
                "noise_reduction",
                "dehaze",
                "spot_removal",
                // ⚠ M9.11: right after spots, in their space.
                "red_eye",
                "geometry",
                // ⚠ M9.6: after geometry, in the frame the masks live in.
                "defringe",
                // ⚠ The fixture's mask carries capture corrections, so it contributes a
                // whole-buffer `MaskStage` here — after the global pair, mirroring the tail's
                // rule that a mask acts on the global look.
                "mask"
            ]
        );
        for name in tail.stage_names() {
            assert!(
                name != "geometry" && name != "noise_reduction",
                "prefix stage {name} leaked into the tail"
            );
        }
        assert_eq!(tail.stage_names().last(), Some(&"output_transform"));
    }

    #[test]
    fn the_prefix_steps_are_the_prefix_pipeline() {
        // build_pipeline_split_in_region is defined through build_prefix_steps, so the two
        // cannot drift: a stage added to one appears in the other.
        for edits in [Edits::default(), all_edits()] {
            let steps =
                build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None);
            let (prefix, _) =
                build_pipeline_split(&edits, RenderScale::FULL, TailRendering::colorimetric());
            let from_steps: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
            assert_eq!(from_steps, prefix.stage_names());
        }
    }

    /// ⚠⚠ **Both directions, and the second is the one that matters.** The prefix key must
    /// change when a mask's noise reduction changes — and must NOT change when its *exposure*
    /// does, or every masked-exposure drag would re-run a whole-buffer denoise (117 ms at
    /// native) for a slider that has nothing to do with it. That is what `capture_projection`
    /// buys, and the "does not change" half is the only thing that can see it.
    #[test]
    fn the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest() {
        let base = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::default(),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 30.0,
                        ..Default::default()
                    },
                    exposure: 0.5,
                    ..Default::default()
                },
                disabled: false,
            }],
            ..Edits::default()
        };
        let key = |e: &Edits| PrefixKey::new(e, RenderScale::FULL, Resampler::default());

        let mut louder = base.clone();
        louder.masks[0].adjustments.noise_reduction.luminance = 80.0;
        assert_ne!(
            key(&base),
            key(&louder),
            "a masked noise-reduction change must invalidate the prefix"
        );

        let mut brighter = base.clone();
        brighter.masks[0].adjustments.exposure = 2.0;
        assert_eq!(
            key(&base),
            key(&brighter),
            "a masked EXPOSURE change must not re-run a whole-buffer denoise"
        );

        let mut hazier = base.clone();
        hazier.masks[0].adjustments.dehaze.amount = 40.0;
        assert_ne!(
            key(&base),
            key(&hazier),
            "masked dehaze must invalidate too"
        );

        let mut moved = base.clone();
        moved.masks[0].mask = edit_model::Mask::Radial(edit_model::RadialGradient::default());
        assert_ne!(
            key(&base),
            key(&moved),
            "the mask's SHAPE decides which pixels are denoised, so it belongs in the key"
        );

        // ⚠⚠ A refinement is part of the shape too, and it is the direction the other four
        // cannot see: it is neither a capture correction nor a tail adjustment, so a
        // `capture_projection` that dropped it would leave every assertion above passing while a
        // subtracted component silently kept a stale masked denoise on screen.
        let mut refined = base.clone();
        refined.masks[0].refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
        });
        assert_ne!(
            key(&base),
            key(&refined),
            "a refinement changes which pixels are denoised, so it belongs in the key"
        );
    }

    /// M9.6: when the defringe step exists. ⚠ Both directions for each case: a hue range alone and
    /// a negative local alone (nothing to protect against) insert nothing; a global amount, or a
    /// positive local with the global at 0, insert it; a switched-off mask contributes nothing.
    #[test]
    fn the_defringe_step_appears_exactly_when_it_can_change_a_pixel() {
        let has = |e: &Edits| {
            build_prefix_steps(e, RenderScale::FULL, None, Resampler::default(), None)
                .iter()
                .any(|s| matches!(s.key, PrefixStepKey::Defringe(..)))
        };
        let with_local = |amount: f32, disabled: bool| Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    defringe: amount,
                    ..Default::default()
                },
                disabled,
            }],
            ..Edits::default()
        };
        assert!(!has(&Edits::default()));
        assert!(!has(&Edits {
            defringe: edit_model::Defringe {
                purple_hue: [250.0, 300.0],
                ..Default::default()
            },
            ..Edits::default()
        }));
        assert!(has(&Edits {
            defringe: edit_model::Defringe {
                green_amount: 5.0,
                ..Default::default()
            },
            ..Edits::default()
        }));
        assert!(has(&with_local(50.0, false)));
        assert!(
            !has(&with_local(-50.0, false)),
            "nothing to protect against"
        );
        assert!(!has(&with_local(50.0, true)), "a switched-off mask");
    }

    #[test]
    fn every_prefix_step_has_its_own_key() {
        // Each step's key must move with that step's parameters and with nothing else. If two
        // stages shared a key, a checkpoint would be reused across a change that altered it.
        let base = all_edits();
        let keys = |e: &Edits| -> Vec<PrefixStepKey> {
            build_prefix_steps(e, RenderScale::FULL, None, Resampler::default(), None)
                .iter()
                .map(|s| s.key.clone())
                .collect()
        };
        let base_keys = keys(&base);
        assert_eq!(
            base_keys.len(),
            8,
            "all eight prefix steps should be active — the negative (M10.11a), NR, dehaze, spots, red eye (M9.11), the geometry (with the \
             keystone fused in since M7.3b), the defringe (M9.6) and the fixture's masked capture \
             correction"
        );

        let variants = [
            // ⚠ M10.11a: the negative step moves with its own setting and nothing else.
            (
                0,
                Edits {
                    negative: edit_model::Negative {
                        gamma: 0.9,
                        ..base.negative
                    },
                    ..base.clone()
                },
            ),
            // ⚠ The keystone moves the GEOMETRY key since M7.3b — it is part of that step.
            (
                5,
                Edits {
                    perspective: edit_model::Perspective {
                        vertical: 70.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                3,
                Edits {
                    spots: vec![edit_model::Spot {
                        centre: [0.7, 0.7],
                        source: [0.2, 0.2],
                        radius: 0.09,
                        feather: 10.0,
                        mode: edit_model::SpotMode::Clone,
                    }],
                    ..base.clone()
                },
            ),
            (
                5,
                Edits {
                    geometry: edit_model::Geometry {
                        angle: 7.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                1,
                Edits {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 80.0,
                        color: 80.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                2,
                Edits {
                    dehaze: edit_model::Dehaze { amount: 90.0 },
                    ..base.clone()
                },
            ),
            // ⚠ The masked capture correction is a prefix step like any other, and its key must
            // move with its own parameters and nothing else — the same property the five above
            // assert, extended to the step this milestone added.
            (7, {
                let mut e = base.clone();
                e.masks[0].adjustments.noise_reduction.luminance = 90.0;
                e
            }),
            // ⚠ M9.6: the defringe step moves with the global settings AND with a mask's local
            // slider — and a mask's local slider moves nothing else, not its capture step.
            (
                6,
                Edits {
                    defringe: edit_model::Defringe {
                        purple_amount: 70.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (6, {
                let mut e = base.clone();
                e.masks[0].adjustments.defringe = -40.0;
                e
            }),
            // ⚠ M9.11: the red-eye step moves with its own eye list and nothing else.
            (4, {
                let mut e = base.clone();
                e.eyes[0].darken = 95.0;
                e
            }),
        ];
        for (moved, e) in variants {
            let ks = keys(&e);
            for i in 0..8 {
                if i == moved {
                    assert_ne!(ks[i], base_keys[i], "step {i} should have changed");
                } else {
                    assert_eq!(ks[i], base_keys[i], "step {i} should not have changed");
                }
            }
        }
    }

    /// A corrector built from a synthetic profile — no RAW file, no bundled database.
    fn test_corrector() -> Option<lens_profile::LensShot> {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        Some(lens_profile::LensShot {
            profile: p,
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        })
    }

    #[test]
    fn lens_correction_runs_first_in_the_prefix() {
        // Before geometry, because the calibration is defined in the uncropped sensor frame.
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let steps = build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default(),
            None,
        );
        let names: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
        assert_eq!(names, vec!["lens_correction", "geometry"]);
    }

    /// ⚠⚠ **M7.3b's whole order, with every global prefix stage present, lens included.** Lens
    /// precedes dehaze so dehaze sees the vignetting correction; spots sit pre-perspective; the one
    /// geometric resample is last, so a geometry drag re-runs nothing else.
    #[test]
    fn the_prefix_runs_lens_nr_dehaze_spots_red_eye_then_geometry() {
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 40.0 },
            perspective: edit_model::Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            spots: vec![edit_model::Spot {
                centre: [0.5, 0.5],
                source: [0.3, 0.3],
                radius: 0.02,
                feather: 50.0,
                mode: Default::default(),
            }],
            eyes: vec![edit_model::Eye {
                centre: [0.4, 0.4],
                radius: 0.01,
                kind: Default::default(),
                pupil_size: 50.0,
                darken: 50.0,
                catchlight: None,
            }],
            ..Edits::default()
        };
        let names: Vec<&'static str> = build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default(),
            None,
        )
        .iter()
        .map(|s| s.stage.name())
        .collect();
        assert_eq!(
            names,
            vec![
                "lens_correction",
                "noise_reduction",
                "dehaze",
                "spot_removal",
                "red_eye",
                "geometry"
            ]
        );
    }

    /// ⚠ A keystone with neutral geometry still needs the geometry step: it lives there now.
    #[test]
    fn perspective_alone_inserts_the_geometry_step() {
        let edits = Edits {
            perspective: edit_model::Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let names: Vec<&'static str> =
            build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None)
                .iter()
                .map(|s| s.stage.name())
                .collect();
        assert_eq!(names, vec!["geometry"]);
    }

    #[test]
    fn lens_correction_is_skipped_when_no_profile_matched() {
        // ⚠ The ordinary case for an unlisted lens, and it must be silent rather than an error.
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Edits::default()
        };
        assert!(
            build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None)
                .is_empty()
        );
    }

    #[test]
    fn the_lens_scale_alone_inserts_no_stage() {
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                scale: 0.0,
                ..Default::default()
            }),
            ..Edits::default()
        };
        assert!(build_prefix_steps(
            &edits,
            RenderScale::FULL,
            test_corrector(),
            Resampler::default(),
            None
        )
        .is_empty());
    }

    /// ⚠ Neutral must be **bit-exact, not trusted** — the `kelvin: None` rule, and the reason
    /// every existing golden stays byte-identical through this feature.
    #[test]
    fn no_spots_means_no_stage() {
        let edits = Edits::default();
        let steps = build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::default(), None);
        assert!(steps.is_empty());
        assert!(
            !build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .stage_names()
                .contains(&"spot_removal")
        );
        assert!(
            !build_pipeline(&edits, RenderScale::FULL, TailRendering::colorimetric())
                .stage_names()
                .contains(&"red_eye")
        );

        // And an empty list on an otherwise busy edit set inserts nothing either.
        let busy = Edits {
            exposure: 0.5,
            spots: Vec::new(),
            ..Edits::default()
        };
        assert!(
            !build_pipeline(&busy, RenderScale::FULL, TailRendering::colorimetric())
                .stage_names()
                .contains(&"spot_removal")
        );
    }

    /// ⚠⚠ The property the placement exists for, in the shape of the brush's real-`.RAF`
    /// validation: a spot is anchored in `GeometryStage`'s **input**, so cropping afterwards
    /// must land the patch on the same source content. Rendering the crop's window out of the
    /// uncropped render must equal the cropped render, bit for bit.
    ///
    /// ⚠ It fails if the stage is moved after geometry: the normalised centre would then refer
    /// to the cropped frame and the patch would slide.
    #[test]
    fn a_spot_survives_a_crop() {
        const W: usize = 64;
        const H: usize = 64;
        let mut img = ImageBuf::new(W, H);
        for y in 0..H {
            for x in 0..W {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / W as f32,
                        y as f32 / H as f32,
                        ((x * y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        let spot = edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.25, 0.25],
            radius: 0.125,
            feather: 0.0,
            mode: edit_model::SpotMode::Clone,
        };
        // A quarter-frame crop whose window still contains the whole patch.
        let crop = edit_model::CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let uncropped = Edits {
            spots: vec![spot],
            ..Edits::default()
        };
        let cropped = Edits {
            geometry: edit_model::Geometry {
                crop,
                ..Default::default()
            },
            ..uncropped.clone()
        };

        let whole = build_pipeline(&uncropped, RenderScale::FULL, TailRendering::colorimetric())
            .run(img.clone());
        let part = build_pipeline(&cropped, RenderScale::FULL, TailRendering::colorimetric())
            .run(img.clone());
        let (x0, y0) = (W / 4, H / 4);
        let window = whole.crop(x0, y0, W / 2, H / 2);
        assert_eq!(window.width(), part.width());
        assert_eq!(window.height(), part.height());

        // Guard against passing trivially: the patch must genuinely change these pixels.
        let neutral = build_pipeline(
            &Edits::default(),
            RenderScale::FULL,
            TailRendering::colorimetric(),
        )
        .run(img);
        assert_ne!(
            neutral.crop(x0, y0, W / 2, H / 2).data(),
            part.data(),
            "the spot must actually alter the cropped window"
        );
        assert_eq!(window.data(), part.data());
    }
}

#[cfg(test)]
mod output_dims_tests {
    use super::*;

    fn with_negative() -> Edits {
        Edits {
            negative: edit_model::Negative {
                on: true,
                base: [0.85, 0.55, 0.3],
                ..edit_model::Negative::default()
            },
            ..Edits::default()
        }
    }

    /// M10.11a: the negative step comes before noise reduction and dehaze — everything after it
    /// works on the positive — and the airlight replay includes it, so dehaze measures the positive.
    #[test]
    fn the_negative_step_comes_before_everything_that_reads_tone() {
        let mut e = with_negative();
        e.noise_reduction.luminance = 30.0;
        e.dehaze.amount = 20.0;
        let names: Vec<&str> =
            build_prefix_steps(&e, RenderScale::FULL, None, Resampler::default(), None)
                .iter()
                .map(|s| s.stage.name())
                .collect();
        assert_eq!(
            names,
            ["negative", "noise_reduction", "dehaze"],
            "{names:?}"
        );
        let mut plain = e.clone();
        plain.negative = edit_model::Negative::default();
        assert_ne!(
            crate::prefix_airlight_key(&e, Resampler::default(), None),
            crate::prefix_airlight_key(&plain, Resampler::default(), None),
            "the airlight is measured on the positive"
        );
        assert_eq!(prefix_output_dims(&with_negative(), 40, 20), (40, 20));
    }

    #[test]
    fn the_predicted_size_follows_the_crop_and_the_turn() {
        assert_eq!(prefix_output_dims(&Edits::default(), 400, 200), (400, 200));
        let mut half = Edits::default();
        half.geometry.crop.right = 0.5;
        assert_eq!(prefix_output_dims(&half, 400, 200), (200, 200));
        let mut turned = Edits::default();
        turned.geometry.orientation.rotate = edit_model::Quarter::Cw90;
        assert_eq!(prefix_output_dims(&turned, 400, 200), (200, 400));
    }
}

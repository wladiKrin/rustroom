//! Dehaze's frame-global airlight, replayed from the source so every render path agrees on it.

use crate::{build_prefix_steps, stages, PrefixStep, PrefixStepKey, RenderScale, Resampler};
use core_image::ImageBuf;
use edit_model::Edits;

/// The dehaze airlight for `edits`, from a fixed 256 px replay of the prefix up to dehaze.
///
/// ⚠⚠ **It replays from the START of the prefix, never from a resume point.** Replaying from
/// wherever a render happens to resume would make the airlight depend on *which slider was
/// dragged* — two facts that can disagree, the shape `render_max_dim`, `fit_image_size` and
/// `Session::geometry_mode` were each deleted for. Starting from the source buffer makes it a
/// deterministic function of the edits alone: independent of the region, of the resume point, and
/// of the render scale.
///
/// ⚠⚠ **This is what lets dehaze be region-scoped at all.** Estimated from the stage's own input
/// under a scoped run, the airlight would come from whatever slice is on screen and the photograph
/// would change as you pan — the failure `PipelineStage::apply_region`'s `None` default exists to
/// prevent.
///
/// ⚠ 256 px because that is what the estimator uses anyway: `AIRLIGHT_SAMPLE`, chosen at M8.3o
/// precisely so the estimate cannot vary with the render scale. The replay is a 65k-pixel run.
///
/// ⚠ `None` is passed for the airlight of the steps it builds, so the replay cannot recurse:
/// dehaze is never constructed here, only the steps above it.
pub fn prefix_airlight(
    edits: &Edits,
    src: &ImageBuf,
    resampler: Resampler,
    lens: Option<lens_profile::LensShot>,
) -> [f32; 3] {
    const SAMPLE: usize = 256;
    let small = src.downscale_to(SAMPLE);
    let long = small.width().max(small.height()).max(1);
    let full_long = src.width().max(src.height()).max(1);
    let scale = RenderScale::new(long as f32 / full_long as f32);
    let mut cur = small;
    for s in airlight_replay(edits, scale, lens, resampler) {
        cur = s.stage.apply(cur);
    }
    stages::dehaze::airlight(&cur)
}

/// The steps [`prefix_airlight`] replays: the prefix up to dehaze, minus the resampling family.
///
/// ⚠⚠ **The one list both [`prefix_airlight`] and [`prefix_airlight_key`] are built from**, so
/// the computation and the key a cache stores it under cannot drift apart.
fn airlight_replay(
    edits: &Edits,
    scale: RenderScale,
    lens: Option<lens_profile::LensShot>,
    resampler: Resampler,
) -> Vec<PrefixStep> {
    // ⚠ `None`, so the replay cannot recurse: dehaze is never constructed here.
    let steps = build_prefix_steps(edits, scale, lens, resampler, None, None);
    let upto = steps
        .iter()
        .position(|s| s.stage.name() == "dehaze")
        .unwrap_or(steps.len());
    steps
        .into_iter()
        .take(upto)
        // ⚠⚠ **The resampling family is SKIPPED, and that is the whole point.** The airlight is an
        // order statistic over the frame, so replaying geometry made it a function of the ANGLE —
        // measured at 6.06-6.81 levels across an ordinary Straighten drag on DSCF6291, and
        // 1.47-1.71 on DSCF6296. `docs/BACKLOG.md` said M8.7's replay "does not move during a drag
        // at all"; that conflates DETERMINISTIC with INVARIANT, because a drag changes an edit on
        // every frame.
        //
        // ⚠ Spot removal and noise reduction stay: neither reframes, and both are part of what
        // dehaze actually sees.
        //
        // ⚠⚠ The consequence, stated rather than discovered: **a crop no longer re-estimates.** A
        // crop into a hazy sky and one into a clear foreground share one value. That IS what
        // invariance means — and a crop drag is continuous too, so re-estimating there would
        // reintroduce the defect in the gesture most likely to expose it.
        //
        // ⚠ Defringe (M9.6) is skipped too — not because it reframes but because a defringe drag
        // must not re-estimate the haze under a masked dehaze; its chroma-only change is
        // immaterial to an order statistic of the brightest dark channel.
        .filter(|s| {
            !matches!(
                s.key,
                PrefixStepKey::LensCorrection(..)
                    | PrefixStepKey::Geometry(..)
                    | PrefixStepKey::Defringe(..)
            )
        })
        .collect()
}

/// Everything [`prefix_airlight`]'s answer depends on, for one source buffer: equal keys mean an
/// equal airlight, so a caller may keep the last value and skip the replay.
///
/// ⚠⚠ **Why this exists (2026-09-26): the replay ran on EVERY render**, including renders with no
/// dehaze anywhere, and it starts with a 256 px downscale of the whole SOURCE buffer. Measured at
/// **12.1 ms of an 84.5 ms Straighten frame** on a 26 Mpx X-T4 decode — a whole frame spent on a
/// number nothing used, and one a straighten drag can never change.
///
/// ⚠⚠ **The geometry is in the key exactly when a masked capture step is replayed.** Those steps
/// build their coverage from `edits.geometry` (see [`build_prefix_steps`]), which their own
/// `PrefixStepKey` does not carry, so with global dehaze off the replay CAN depend on the angle.
/// Leaving it out would serve a stale airlight during a Straighten drag on such a photo.
/// `the_airlight_key_decides_the_airlight` pins "equal keys, equal airlight" on that case too.
///
/// ⚠ The source buffer is NOT in the key: the caller owns one buffer per cache (the render
/// worker is rebuilt with its buffer). Nor is the render scale: no `PrefixStepKey` variant
/// carries it, and the replay's own scale is a function of the buffer alone.
#[derive(Debug, Clone, PartialEq)]
pub struct AirlightKey {
    steps: Vec<PrefixStepKey>,
    geometry: Option<edit_model::Geometry>,
}

pub fn prefix_airlight_key(
    edits: &Edits,
    resampler: Resampler,
    lens: Option<lens_profile::LensShot>,
) -> AirlightKey {
    let steps: Vec<PrefixStepKey> = airlight_replay(edits, RenderScale::FULL, lens, resampler)
        .into_iter()
        .map(|s| s.key)
        .collect();
    let geometry = steps
        .iter()
        .any(|k| matches!(k, PrefixStepKey::MaskedCapture(..)))
        .then_some(edits.geometry);
    AirlightKey { steps, geometry }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::structured;
    use crate::{capture_stages, PipelineStage};

    /// ⚠⚠ The guard for the MASKED half, and it is the only thing that can see it regress.
    ///
    /// Before 2026-09-17 `capture_stages` called `DehazeStage::new`, so a mask's dehaze estimated
    /// from its own input on every frame — 9.39-12.09 levels of movement across a Straighten drag,
    /// worse than the global path beside it. ⚠ A probe cannot show this any more: measuring what an
    /// *unsupplied* stage would estimate is a counterfactual once the stage is supplied. Two
    /// different supplied airlights producing two different pictures is the observable.
    #[test]
    fn a_masks_capture_dehaze_uses_the_supplied_airlight() {
        let img = structured(64, 48);
        let a = edit_model::LocalAdjustments {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let run = |air: Option<[f32; 3]>| {
            let mut out = img.clone();
            for st in capture_stages(&a, RenderScale::FULL, air) {
                out = st.apply(out);
            }
            out
        };
        let hi = run(Some([0.9; 3]));
        let lo = run(Some([0.5; 3]));
        assert_ne!(
            hi.data(),
            lo.data(),
            "the supplied airlight must reach a mask's pixels"
        );
        // ⚠ And `None` must still estimate, which is what the test call sites rely on.
        assert_ne!(
            run(None).data(),
            hi.data(),
            "None must estimate rather than reuse a supplied value"
        );
    }

    #[test]
    fn a_supplied_airlight_is_used_verbatim_rather_than_re_estimated() {
        // ⚠⚠ The only test that can see the stage falling back to its own estimate. Two renders of
        // the SAME image with different supplied airlights must differ; if the stage re-estimates,
        // they are identical and this fails.
        let img = structured(64, 48);
        let p = edit_model::Dehaze { amount: 60.0 };
        let a = crate::stages::dehaze::DehazeStage::with_airlight(&p, RenderScale::FULL, [0.9; 3])
            .apply(img.clone());
        let b = crate::stages::dehaze::DehazeStage::with_airlight(&p, RenderScale::FULL, [0.5; 3])
            .apply(img.clone());
        assert_ne!(
            a.data(),
            b.data(),
            "the supplied airlight must reach the pixels"
        );
        // And the fallback still works when none is supplied.
        let c = crate::stages::dehaze::DehazeStage::new(&p, RenderScale::FULL).apply(img);
        assert_ne!(
            c.data(),
            a.data(),
            "None must estimate rather than reuse a supplied value"
        );
    }

    #[test]
    fn the_replay_does_not_depend_on_where_a_render_resumes() {
        // ⚠⚠ THE property that made this design replay from the START of the prefix. An airlight
        // that depended on the resume point would change with WHICH SLIDER YOU DRAGGED — two facts
        // that can disagree, the shape render_max_dim and Session::geometry_mode were deleted for.
        let img = structured(220, 180);
        let mut e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                detail: 50.0,
                smoothness: 50.0,
            },
            ..Default::default()
        };
        e.geometry.angle = 4.0;
        let one = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        let two = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        assert_eq!(one, two, "deterministic");

        // ⚠ And it is a function of the EDITS, so a downstream-only edit cannot move it. That is
        // what makes `PrefixKey` sound without carrying the airlight: the key already covers
        // every edit the replay reads.
        let mut f = e.clone();
        f.exposure = 1.5;
        f.vignette = edit_model::Vignette {
            amount: -60.0,
            ..Default::default()
        };
        assert_eq!(
            super::prefix_airlight(&f, &img, crate::resample::Resampler::CatmullRom, None),
            one,
            "a tail edit must not move the airlight"
        );
    }

    /// ⚠⚠ The property this milestone exists for: **reframing must not move the airlight.**
    ///
    /// Before 2026-09-17 the replay ran geometry, so the airlight was a function of the ANGLE —
    /// measured at 6.06-6.81 levels across an ordinary Straighten drag on `DSCF6291`.
    #[test]
    fn the_airlight_does_not_move_when_the_frame_is_reframed() {
        let img = structured(220, 180);
        let base = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let want =
            super::prefix_airlight(&base, &img, crate::resample::Resampler::CatmullRom, None);

        let mut angled = base.clone();
        angled.geometry.angle = 6.0;
        let mut cropped = base.clone();
        cropped.geometry.crop = edit_model::CropRect {
            left: 0.5,
            top: 0.5,
            right: 1.0,
            bottom: 1.0,
        };
        let mut both = angled.clone();
        both.geometry.crop = cropped.geometry.crop;

        for (name, e) in [("angle", &angled), ("crop", &cropped), ("both", &both)] {
            let got = super::prefix_airlight(e, &img, crate::resample::Resampler::CatmullRom, None);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-6,
                    "{name} moved the airlight on ch{c}: {} vs {}",
                    got[c],
                    want[c]
                );
            }
        }
    }

    /// ⚠⚠ **The property a memo of the airlight relies on: equal keys, equal airlight.** Checked
    /// over every pair of a set of variants chosen to hit each thing the replay reads: noise
    /// reduction, spots, dehaze on and off (which changes where the replay stops), a mask's
    /// capture correction, and reframes on either side of it.
    ///
    /// ⚠ Both halves are asserted to occur, or the implication would hold vacuously: some pairs
    /// must share a key (a Straighten drag on a plain photo is the case the memo is for) and some
    /// must not.
    #[test]
    fn the_airlight_key_decides_the_airlight() {
        let img = structured(220, 180);
        let r = crate::resample::Resampler::CatmullRom;
        let dehazed = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let masked_nr = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 70.0,
                        color: 70.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Default::default()
        };
        let with = |base: &Edits, f: &dyn Fn(&mut Edits)| {
            let mut e = base.clone();
            f(&mut e);
            e
        };
        let angle = |e: &mut Edits| e.geometry.angle = 6.0;
        let nr = |e: &mut Edits| {
            e.noise_reduction = edit_model::NoiseReduction {
                luminance: 80.0,
                color: 80.0,
                ..Default::default()
            }
        };
        let spot = |e: &mut Edits| {
            e.spots = vec![edit_model::Spot {
                centre: [0.3, 0.3],
                source: [0.7, 0.7],
                radius: 0.08,
                feather: 20.0,
                mode: edit_model::SpotMode::Clone,
                dust: false,
            }]
        };
        let variants = [
            Edits::default(),
            with(&Edits::default(), &angle),
            dehazed.clone(),
            with(&dehazed, &angle),
            with(&dehazed, &nr),
            with(&with(&dehazed, &nr), &angle),
            with(&dehazed, &spot),
            with(&Edits::default(), &spot),
            masked_nr.clone(),
            with(&masked_nr, &angle),
        ];
        let keys: Vec<_> = variants
            .iter()
            .map(|e| super::prefix_airlight_key(e, r, None))
            .collect();
        let airs: Vec<_> = variants
            .iter()
            .map(|e| super::prefix_airlight(e, &img, r, None))
            .collect();
        let (mut shared, mut distinct) = (0, 0);
        for i in 0..variants.len() {
            for j in i + 1..variants.len() {
                if keys[i] == keys[j] {
                    shared += 1;
                    assert_eq!(
                        airs[i], airs[j],
                        "variants {i} and {j} share a key but not an airlight"
                    );
                } else {
                    distinct += 1;
                }
            }
        }
        assert!(
            shared > 0 && distinct > 0,
            "fixture: {shared} shared, {distinct} distinct"
        );
        // ⚠⚠ The masked-capture case specifically: its step key carries no geometry, so without
        // the geometry term these two would share a key.
        assert_ne!(
            keys[8], keys[9],
            "a masked capture step must key on the geometry"
        );
        // And the case the memo is for: a Straighten drag on a dehazed photo shares one key.
        assert_eq!(keys[2], keys[3]);
    }

    #[test]
    fn the_replay_moves_when_an_upstream_edit_moves() {
        // The discriminating other half: a constant would pass the test above.
        //
        // ⚠⚠ **This used to crop to a different quarter, and that stopped discriminating on
        // 2026-09-17** — `prefix_airlight` now skips the resampling family, so a crop is
        // deliberately unable to move the airlight. Noise reduction is the replacement because it
        // is upstream of dehaze and is *not* a reframe. ⚠ If this ever fails again, check whether
        // the skip list has grown rather than assuming the estimator broke.
        let img = structured(220, 180);
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let base = super::prefix_airlight(&e, &img, crate::resample::Resampler::CatmullRom, None);
        let mut f = e.clone();
        f.noise_reduction = edit_model::NoiseReduction {
            luminance: 80.0,
            detail: 50.0,
            color: 80.0,
            smoothness: 80.0,
        };
        assert_ne!(
            super::prefix_airlight(&f, &img, crate::resample::Resampler::CatmullRom, None),
            base,
            "an upstream edit that is not a reframe must still move the airlight"
        );
    }
}

//! Decoding a file into the pipeline's linear working space, and the workers that do it
//! off the UI thread.
//!
//! Split out of `main.rs` unchanged. Everything here is a free function: nothing in this
//! module touches `RustroomApp`, which is what lets the decode workers capture it.

use crate::MAX_CONCURRENT_FULL_DECODES;
use eframe::egui;
use rustroom_app::decode_worker::{DecodeWorker, LensFacts, ProfileParts, SourceFacts};

/// What the camera profile at `path` contributes to the display rendering: its tone curve and
/// its baseline exposure, either of which may be absent.
///
/// ⚠ Parsed here rather than handed back by `raw_decode`, which loads the same file for its
/// matrices and its HueSatMap. A second parse of a few-hundred-KB file once per decode is
/// cheaper than widening that crate's return type, and it keeps the profile's *use* — the
/// pipeline ignores an identity curve — out of the decoder entirely.
///
/// ⚠ **One parse for both**, so the curve and the baseline cannot come from different reads of
/// the same file.
fn profile_rendering(profile: &Option<std::path::PathBuf>) -> (Option<Vec<[f32; 2]>>, Option<f32>) {
    let Some(p) = profile
        .as_ref()
        .and_then(|p| camera_profile::CameraProfile::from_file(p).ok())
    else {
        return (None, None);
    };
    (p.tone_curve, p.baseline_exposure)
}

/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf`, a display label, and the buffer's size relative to a full-resolution
/// decode. Dispatches by file kind.
pub(crate) fn decode_for_edit(
    path: &std::path::Path,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, SourceFacts), String> {
    if catalog::is_raw(path) {
        let opts = raw_decode::DecodeOptions {
            camera_profile: profile,
            ..raw_decode::DecodeOptions::preview()
        };
        let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
        let label = format!("{} {}", d.metadata.make, d.metadata.model);
        let lens = resolve_lens(&d.metadata);
        let (profile_curve, baseline_ev) = profile_rendering(&opts.camera_profile);
        Ok((
            d.image,
            SourceFacts {
                label,
                source_scale: opts.source_scale(),
                lens,
                // ⚠ The camera's own as-shot white, which is where the Kelvin slider's zero point
                // sits for this photo. A file that records no usable multipliers falls back to
                // D65, the same answer a JPEG gets — a right answer, not a degraded one.
                wb_baseline: d.metadata.as_shot.unwrap_or_default(),
                profile: ProfileParts {
                    curve: profile_curve,
                    baseline_ev,
                    // ⚠ Filled in at `RenderEngine::new`, not here: the decode has no business
                    // knowing where looks live, and the directory is a session fact rather than
                    // a fact about this file.
                    looks_dir: None,
                },
                scene_referred: true,
            },
        ))
    } else {
        let d = image_decode::decode(path).map_err(|e| e.to_string())?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        // Non-RAW formats are decoded at full size, and carry no lens metadata.
        Ok((
            d.image,
            SourceFacts {
                label: format!("{ext} {}×{}", d.width, d.height),
                source_scale: 1.0,
                lens: LensFacts::default(),
                // A file the camera has already balanced: a neutral object in it reads as equal
                // RGB, so its baseline *is* sRGB's white point.
                //
                // ⚠⚠ And `scene_referred` falls out of `Default` as **false**, which is
                // load-bearing rather than incidental: a JPEG/PNG/TIFF has already been rendered
                // by whatever produced it, so the display rendering must be skipped or the
                // photograph is rendered twice. Stated here because a defaulted field carrying a
                // decision is exactly the kind of thing a later reader "tidies up".
                ..SourceFacts::default()
            },
        ))
    }
}

/// Match the decoded file's lens against the bundled LensFun database.
///
/// ⚠ Called on the **decode thread**. `Catalogue::shared()` parses the database once per process,
/// but that first parse decompresses 56 XML files and has no business on the UI thread — and a
/// database load in a panel body would be the `apply_view_demosaic` mistake in a new costume.
pub(crate) fn resolve_lens(m: &raw_decode::RawMetadata) -> LensFacts {
    let Some(l) = m.lens.as_ref() else {
        return LensFacts::default();
    };
    let shot = lens_profile::Catalogue::shared()
        .and_then(|db| db.find(&m.make, &m.model, &l.name))
        .map(|profile| lens_profile::LensShot {
            profile,
            shot: lens_profile::Shot {
                focal: l.focal,
                aperture: l.aperture,
            },
        });
    LensFacts {
        name: Some(l.name.clone()),
        shot,
    }
}

/// What choosing `tier` for the 1:1 view costs, relative to the default.
///
/// ⚠ A **multiplier, not seconds.** The export dialog says "roughly 1.2 s to 11 s per photo",
/// which are X-T4 numbers written into prose and wrong on any other body. Ratios are the
/// stable part, and they matter more here because this cost is *recurring*: the
/// full-resolution decode is eager, so a slower tier is paid on every photo you stop on,
/// not once per export.
///
/// Derived from the measured end-to-end decodes — ~1.4 / 2.9 / 7.5 / 10.9 s on an X-T4,
/// recorded in `docs/superpowers/specs/2026-09-10-demosaic-decision.md` (and consistent with
/// the original 1.25 / 2.89 / 6.9 / 11.42 in the M8.4 predecessor spec).
///
/// ⚠ The ratios are against **the default**, not against a named tier. They were written
/// against `Fast` when `Fast` was the default, so moving the default at M8.4 would have left
/// every one of them quietly describing the wrong baseline.
///
/// Lives here rather than on `raw_decode::Demosaic` because a cost measured on one camera is
/// presentation, not a property of the algorithm; `raw-decode` exposes the neutral facts.
pub(crate) fn view_quality_hint(tier: raw_decode::Demosaic) -> &'static str {
    match tier {
        raw_decode::Demosaic::Fastest => "about 0.2× the decode time of the default",
        raw_decode::Demosaic::Fast => {
            "about 0.4× the decode time of the default — \
             but it smears fine foliage; see M8.4"
        }
        raw_decode::Demosaic::Better => "the default",
        raw_decode::Demosaic::Best => {
            "about 1.45× the decode time of the default, \
             and indistinguishable from it at 4×"
        }
    }
}

/// Decode a RAW at **full** resolution, for the true-1:1 view, at `tier`.
///
/// The tier comes from `Settings::view_demosaic`, not from any export preset — see that
/// field's doc comment for why mirroring an export preset was rejected.
pub(crate) fn decode_full_res(
    path: &std::path::Path,
    tier: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, SourceFacts), String> {
    let opts = raw_decode::DecodeOptions {
        demosaic: tier,
        camera_profile: profile,
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
    let label = format!("{} {}", d.metadata.make, d.metadata.model);
    let lens = resolve_lens(&d.metadata);
    let (profile_curve, baseline_ev) = profile_rendering(&opts.camera_profile);
    Ok((
        d.image,
        SourceFacts {
            label,
            source_scale: opts.source_scale(),
            lens,
            wb_baseline: d.metadata.as_shot.unwrap_or_default(),
            profile: ProfileParts {
                curve: profile_curve,
                baseline_ev,
                looks_dir: None,
            },
            scene_referred: true,
        },
    ))
}

/// Build the full-resolution decode worker for `tier`.
///
/// ⚠ The tier is **captured** here rather than passed per request. `DecodeWorker` is shared
/// with the preview decode, where demosaic is inert — libraw skips it entirely for a
/// half-size decode — so putting a tier on `DecodeWorker::request` would impose a parameter
/// on a caller that cannot act on it. Changing the setting therefore rebuilds this worker;
/// dropping the old one disconnects its channel, so a decode in flight finishes and discards
/// its own result, the same abandon-don't-abort trade the full-resolution handover makes.
/// ⚠ The tier and the profile are **captured**, not passed per request: `DecodeWorker`'s callers
/// have no business knowing about either, and rebuilding the worker is what makes a change take
/// effect. Dropping the old one disconnects its channel, so an in-flight decode discards its own
/// result — the abandon-don't-abort trade used at handover.
pub(crate) fn full_res_worker(
    tier: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
    ctx: &egui::Context,
) -> DecodeWorker {
    let ctx = ctx.clone();
    DecodeWorker::new(
        move |p| decode_full_res(p, tier, profile.clone()),
        MAX_CONCURRENT_FULL_DECODES,
        move || ctx.request_repaint(),
    )
}

/// Full-resolution decode for export. Dispatches by file kind.
///
/// ⚠⚠ Returns the white-balance baseline alongside the pixels, because a Kelvin in the sidecar is
/// meaningless without the illuminant it was set against — and a batch export decodes each file
/// itself rather than borrowing the open session's. Drop it and every exported RAW gets D65's
/// gains where the preview used the camera's own, so **the file would not match what was on
/// screen**.
pub(crate) fn decode_for_export(
    path: &std::path::Path,
    demosaic: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, color::wb::Illuminant), String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                demosaic,
                camera_profile: profile,
                ..raw_decode::DecodeOptions::linear_scene()
            },
        )
        .map(|d| (d.image, d.metadata.as_shot.unwrap_or_default()))
        .map_err(|e| e.to_string())
    } else {
        image_decode::decode(path)
            .map(|d| (d.image, color::wb::Illuminant::default()))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demosaic_tier_has_a_cost_hint() {
        // ⚠ A table test rather than four assertions: a fifth tier added to `Demosaic::ALL`
        // must not ship with an empty hint in the Settings dialog, and nothing else would
        // catch that.
        for d in raw_decode::Demosaic::ALL {
            let hint = view_quality_hint(d);
            assert!(!hint.is_empty(), "{d:?} has no cost hint");
        }
    }

    #[test]
    fn the_default_tier_is_described_as_the_default() {
        // The hint is the only place the dialog says which one costs nothing extra.
        assert!(view_quality_hint(raw_decode::Demosaic::default()).contains("default"));
    }

    #[test]
    fn a_full_size_decode_is_twice_the_half_size_one() {
        // ⚠ Without this, a typo in `decode_full_res` would leave the whole feature inert but
        // entirely plausible-looking: a second engine, a second cache, a status line, and the
        // same binned pixels. Gated like the other real-decode tests; run with --release.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path.raw to run this test");
            return;
        };
        let p = std::path::Path::new(&path);
        let (half, half_facts) = decode_for_edit(p, None).expect("half decode");
        let half_scale = half_facts.source_scale;
        let (full, full_facts) =
            decode_full_res(p, raw_decode::Demosaic::default(), None).expect("full decode");
        let full_scale = full_facts.source_scale;
        assert_eq!(half_scale, 0.5);
        assert_eq!(full_scale, 1.0);
        // libraw's half-size path halves each dimension, rounding down.
        assert!(
            full.width() >= half.width() * 2 - 2 && full.width() <= half.width() * 2 + 2,
            "full {} vs half {}",
            full.width(),
            half.width()
        );
    }
}

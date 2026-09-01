//! Decoding a file into the pipeline's linear working space, and the workers that do it
//! off the UI thread.
//!
//! Split out of `main.rs` unchanged. Everything here is a free function: nothing in this
//! module touches `RustroomApp`, which is what lets the decode workers capture it.

use crate::MAX_CONCURRENT_FULL_DECODES;
use eframe::egui;
use rustroom_app::decode_worker::{DecodeWorker, LensFacts};

/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf`, a display label, and the buffer's size relative to a full-resolution
/// decode. Dispatches by file kind.
pub(crate) fn decode_for_edit(
    path: &std::path::Path,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, String, f32, LensFacts), String> {
    if catalog::is_raw(path) {
        let opts = raw_decode::DecodeOptions {
            camera_profile: profile,
            ..raw_decode::DecodeOptions::preview()
        };
        let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
        let label = format!("{} {}", d.metadata.make, d.metadata.model);
        let lens = resolve_lens(&d.metadata);
        Ok((d.image, label, opts.source_scale(), lens))
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
            format!("{ext} {}×{}", d.width, d.height),
            1.0,
            LensFacts::default(),
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
/// Derived from the measured end-to-end decodes — 1.25 / 2.89 / 6.9 / 11.42 s — recorded in
/// `docs/superpowers/specs/2026-08-11-demosaic-backend-design.md`.
///
/// Lives here rather than on `raw_decode::Demosaic` because a cost measured on one camera is
/// presentation, not a property of the algorithm; `raw-decode` exposes the neutral facts.
pub(crate) fn view_quality_hint(tier: raw_decode::Demosaic) -> &'static str {
    match tier {
        raw_decode::Demosaic::Fastest => "about 0.4× the decode time of Fast",
        raw_decode::Demosaic::Fast => "the default",
        raw_decode::Demosaic::Better => "about 2.4× the decode time of Fast",
        raw_decode::Demosaic::Best => "about 4× the decode time of Fast",
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
) -> Result<(core_image::ImageBuf, String, f32, LensFacts), String> {
    let opts = raw_decode::DecodeOptions {
        demosaic: tier,
        camera_profile: profile,
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
    let label = format!("{} {}", d.metadata.make, d.metadata.model);
    let lens = resolve_lens(&d.metadata);
    Ok((d.image, label, opts.source_scale(), lens))
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
pub(crate) fn decode_for_export(
    path: &std::path::Path,
    demosaic: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
) -> Result<core_image::ImageBuf, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                demosaic,
                camera_profile: profile,
                ..raw_decode::DecodeOptions::linear_scene()
            },
        )
        .map(|d| d.image)
        .map_err(|e| e.to_string())
    } else {
        image_decode::decode(path)
            .map(|d| d.image)
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
        let (half, _, half_scale, _) = decode_for_edit(p, None).expect("half decode");
        let (full, _, full_scale, _) =
            decode_full_res(p, raw_decode::Demosaic::default(), None).expect("full decode");
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

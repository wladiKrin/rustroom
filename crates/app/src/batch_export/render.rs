//! Rendering and writing **one** exported photo.
//!
//! ⚠ Split out of `batch_export.rs` on 2026-09-18 as a **pure move** — the `prefix_cache.rs`
//! precedent, with the test count unchanged and no golden moved. Every `⚠` below is the record of
//! a decision or a bug; **do not reword one.**

use super::*;
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::PipelineStage;
use std::path::Path;

/// The render scale a `max_edge` cap implies for a source whose long edge is `source_long`.
///
/// Mirrors `render_engine::render_scale`, with `source_scale` fixed at 1.0 because an export
/// always decodes at full resolution — never through `DecodeOptions::preview()`.
pub fn scale_for(source_long: usize, max_edge: Option<usize>) -> pipeline::RenderScale {
    let Some(max_edge) = max_edge else {
        return pipeline::RenderScale::FULL;
    };
    if source_long == 0 || max_edge >= source_long {
        // `downscale_to` never upscales, so neither does the scale.
        return pipeline::RenderScale::FULL;
    }
    pipeline::RenderScale::new(max_edge as f32 / source_long as f32)
}

/// Render one decoded image through `edits` and write it as a JPEG.
///
/// ⚠ The downscale happens **before** the pipeline, not after. Rendering at full resolution
/// and shrinking the result would resample **sRGB-encoded** pixels, which is gamma-incorrect;
/// `downscale_to` here operates on the linear decode, where averaging is meaningful. It is
/// also the path the preview already takes, and it is faster, since every stage then runs on
/// fewer pixels.
/// The unresolved payload a job carries, in the export crate's terms.
///
/// ⚠ This is the one place a catalog `Description`, `Marks` and keyword paths become the plain
/// `FileMetadata` the packet is written from. `FileMetadata::resolve` then lays the preset's
/// defaults under it and applies the scope.
pub fn file_metadata(job: &ExportJob) -> export::FileMetadata {
    export::FileMetadata {
        title: job.description.title.clone(),
        caption: job.description.caption.clone(),
        copyright: job.description.copyright.clone(),
        creator: job.description.creator.clone(),
        rating: job.marks.rating,
        label: crate::xmp_sync::label_to_xmp(job.marks.label).map(str::to_string),
        keywords: job.keywords.clone(),
        capture: job.capture.clone(),
    }
}

// ⚠⚠ **A COUNTER, and it exists because an accelerated export and a CPU export produce the SAME
// photograph by design** — so every correctness test in the workspace passes with the seam
// deleted. That is the W5 failure mode, where creative LUTs shipped fully tested and 100%
// non-functional. `RenderEngine::requests_issued` and `Ran { prefix_stages }` exist for the same
// reason.
//
// ⚠ A **counter** rather than a flag: an observable describing the current state cannot see its
// own absence. ⚠ And **thread-local**, because `cargo test` runs tests in parallel and a shared
// count would report whatever the rest of the suite happened to be doing — the contamination
// `GpuChain`'s transfer counters were made per chain to avoid.
//
// ⚠ Plain `//`, not `///`: a doc comment on a `thread_local!` invocation is `unused_doc_comments`,
// which `-D warnings` rejects. This file's own history records that trap; it fired again here.
#[cfg(test)]
thread_local! {
    static ACCELERATED_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn accelerated_steps_bump() {
    ACCELERATED_STEPS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(test))]
fn accelerated_steps_bump() {}

/// Reset the counter and return what the closure's exports crossed the seam with.
#[cfg(test)]
fn accelerated_steps<T>(f: impl FnOnce() -> T) -> usize {
    ACCELERATED_STEPS.with(|c| c.set(0));
    f();
    ACCELERATED_STEPS.with(|c| c.get())
}

/// Render one photo and write it out.
///
/// ⚠⚠ **The prefix runs as STEPS rather than as one pipeline, and that is what lets each of them
/// be offered to the GPU seam** — the shape `render_cached` already uses, and for its reason:
/// `accelerate_prefix_stage` substitutes a stage by name and has to see them one at a time.
///
/// ⚠⚠ **And it is what makes lens correction reach an export at all.** This used to build through
/// `pipeline::build_pipeline_spec`, which hardcodes `lens: None` — so for the life of the export
/// dialog every RAW was written out with no lens correction while the preview and the CLI both
/// applied it. Measured on an X-T4 frame at a mild 19.2 mm f/8: **worst 187 of 255 levels, mean
/// 6.95, 33.5% of samples differing by more than 4.** `a_lens_profile_reaches_the_exported_pixels`
/// is the carrier; nothing in `pipeline` could ever have seen it, because the stage itself was
/// never broken.
///
/// ⚠ The tail comes from `build_pipeline_split_in_region_with_brush` with its prefix **discarded**,
/// deliberately: both halves then come from one builder, so they cannot drift about what belongs
/// in which.
pub fn export_one(
    source: ExportSource,
    edits: &Edits,
    dest: &Path,
    opts: &ExportOptions,
    render: ExportRender,
    meta: &export::FileMetadata,
    // ⚠⚠ **Passed in rather than resolved here, and that is a per-batch/per-photo distinction.**
    // A 64³ cube is 786 KB and one batch shares one look, so loading it here would be per-photo
    // work for a per-batch fact — the same reasoning `RenderEngine`'s worker gives for caching it
    // in one slot. `export_one` does no file I/O of its own.
    look: Option<&std::sync::Arc<pipeline::lut::Cube>>,
) -> Result<(), String> {
    // ⚠⚠ **The same strip `render_cached`'s worker does, and for the same reason**: a JPEG, a PNG
    // or a TIFF has been through whatever produced it, so applying our rendering on top renders
    // the photograph twice. Done here rather than at the call sites so no caller can forget —
    // which is exactly the sentence `render_cached` uses, and exactly what went wrong when only
    // one of the two paths had it.
    //
    // ⚠ `rendering: None` is **undecided**, not decided off: a stripped copy is a render input,
    // not a choice anyone made, so it must never be written back to a sidecar.
    let stripped;
    let edits = if source.scene_referred {
        edits
    } else {
        stripped = Edits {
            rendering: None,
            ..edits.clone()
        };
        &stripped
    };
    let source_long = source.image.width().max(source.image.height());
    let scaled = match opts.max_edge {
        Some(max) => source.image.downscale_to(max),
        None => source.image,
    };
    let scale = scale_for(source_long, opts.max_edge);
    // The buffer the prefix's lens stage corrects — what an AI map's decode mapping must match.
    let (decode_w, decode_h) = (scaled.width(), scaled.height());
    // ⚠⚠ The export must use the SAME airlight definition as the preview, or a dehazed photo
    // would render one way on screen and another in the file — by up to 5.6 of an 8-bit level,
    // measured. `prefix_airlight` replays the prefix at a fixed 256 px, so it is a deterministic
    // function of the edits and costs a 65k-pixel run.
    //
    // ⚠ And it is handed the **lens**, for the same reason: the replay walks the prefix, so a
    // replay without the correction would estimate the airlight from a differently-shaped
    // photograph than the one being exported.
    let airlight = pipeline::prefix_airlight(edits, &scaled, render.resampler, source.lens.clone());
    let spec = pipeline::RenderSpec {
        // ⚠ DNG renders into its fixed space (linear ProPhoto) whatever the dialog says.
        space: opts.format.fixed_space().unwrap_or(opts.space),
        intent: opts.intent,
        resampler: render.resampler,
        baseline: source.baseline,
        airlight: Some(airlight),
    };
    let steps = pipeline::build_prefix_steps(
        edits,
        scale,
        source.lens.clone(),
        render.resampler,
        Some(airlight),
    );
    // ⚠⚠ **This was `TailRendering::default()` until 2026-09-18, and that was the bug**: an
    // export rendered with the *built-in* curve and `BASELINE_EV` while the preview consulted the
    // camera profile, and the creative look never ran at all. Measured before the fix — on the
    // catalogue's own X-T4 the difference is **0.00 levels**, because that profile's
    // `ProfileToneCurve` is exactly `[[0,0],[1,1]]` (which `effective_profile_curve` rejects) and
    // it declares no `BaselineExposure`. A profile carrying a real curve diverges by up to
    // **~157 of 255 levels**, which is what the fixture check in
    // `examples/probe_export_rendering.rs` measures.
    //
    // ⚠ `curve` and `baseline_ev` travel on `ExportSource` because this struct **borrows** them.

    // ⚠ Resolved once and only when the setting is on: `shared()` acquires an adapter on first
    // call, and the CPU arm below would otherwise ask for it per step.
    let prefix_ctx = if render.gpu_tail {
        gpu_pipeline::shared().cloned()
    } else {
        None
    };
    // ⚠⚠ **A deposit must not outlive its render, and in a BATCH EXPORT that is not theoretical.**
    // The last prefix step parks its output for a consumer that may not exist — this export path
    // runs its tail on the CPU — so the plane survives the photograph. The next photo in the batch
    // is from the same camera, so it has the *same dimensions*, and it is rendered on the *same
    // thread*: `SharedPlane::take` would hand the previous photograph's pixels to the first step of
    // the next one. **A plausible photograph, in an exported file.** Clearing at the top of the
    // loop bounds a deposit to one render, which is `end_mask`'s rule one level up.
    if let Some(ctx) = &prefix_ctx {
        ctx.prefix_plane().clear();
    }

    let mut img = scaled;
    for step in &steps {
        // ⚠⚠ **Per step and at apply time, exactly as `render_cached` does it** — never over the
        // whole list up front. `dehaze::airlight` is estimated from the image the stage is HANDED,
        // and geometry crops, so substituting up front would estimate it from the wrong pixels: a
        // slightly different airlight is a different photograph, not an error.
        //
        // ⚠⚠ **`if` rather than `match (gpu_tail, shared())`** — a tuple scrutinee evaluates both
        // elements, so with the setting off `shared()` would still acquire an adapter. That was a
        // real bug when `gpu_tail` first landed and it cost ~42 s per debug test run.
        //
        // ⚠ An export decode is never cached, so there is no `PrefixStepKey` question here at all:
        // the seam returns a **stage**, never a step, and nothing stores the result.
        let accelerated = if render.gpu_tail {
            gpu_pipeline::shared().and_then(|ctx| {
                gpu_pipeline::accelerate_prefix_stage(
                    step.stage.name(),
                    edits,
                    scale,
                    &img,
                    gpu_pipeline::SeamFacts {
                        lens: source.lens.as_ref(),
                        resampler: render.resampler,
                        // ⚠ The same frame-global value the CPU prefix was built with, so the twin
                        // cannot disagree with the shader about what colour the haze is.
                        airlight,
                    },
                    ctx,
                )
            })
        } else {
            None
        };
        img = match accelerated {
            Some(mut gpu) => {
                // ⚠ The resident prefix seam — see `render_cached`'s loop, which carries the
                // reasoning. Consecutive accelerated steps hand the buffer over instead of a
                // download-and-re-upload of the whole frame.
                gpu.allow_shared_input(gpu_pipeline::Slot::Prefix);
                gpu.offer_output(gpu_pipeline::Slot::Prefix);
                accelerated_steps_bump();
                gpu.apply(img)
            }
            None => {
                // ⚠⚠ A CPU step makes the parked plane two stages stale while its dimensions and
                // thread still match, so `take` would accept it. Clearing is what makes that
                // unrepresentable rather than merely unlikely.
                if let Some(ctx) = &prefix_ctx {
                    ctx.prefix_plane().clear();
                }
                step.stage.apply(img)
            }
        };
    }
    // ⚠ Built AFTER the prefix since M9.13: an AI mask's coverage is resampled to the final frame,
    // whose size only the geometry step decides.
    let coverage = ai_coverage_for(
        edits,
        &source.ai,
        &img,
        (decode_w, decode_h),
        source.lens.as_ref(),
    );
    let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        None,
        coverage.as_ref(),
        spec,
        source.lens.clone(),
        pipeline::TailRendering {
            curve: source.curve.as_deref(),
            baseline_ev: source.baseline_ev,
            look,
        },
    );
    let mut rendered = tail.run(img);

    // ⚠⚠ **THE ORDER OF THESE THREE IS THE FEATURE, NOT AN ARRANGEMENT.**
    //
    // Output sharpening goes **first**, because it exists to put back the edge the resize took
    // and must therefore measure the photograph — a watermark laid down first would be a hard
    // white edge for it to ring around. Both act on the display-encoded buffer, which is what
    // the pipeline has just handed back; see each module for why that is right for it.
    //
    // The metadata goes **last**, after the encode, because it is surgery on the container
    // rather than on the pixels. It cannot change what the photograph looks like.
    export::output_sharpen::apply(&mut rendered, &opts.output_sharpening);
    export::watermark::apply(&mut rendered, &opts.watermark);
    export::save(&rendered, dest, opts.format, opts.quality, opts.space)
        .map_err(|e| e.to_string())?;
    export::metadata::embed(dest, opts.format, meta).map_err(|e| e.to_string())
}

/// The brush side channel carrying `edits`' AI masks for a frame the size of `frame`, or `None`
/// when there are no AI masks — every other mask builds its own coverage, as it always did.
fn ai_coverage_for(
    edits: &Edits,
    maps: &crate::ai_coverage::AiMaps,
    frame: &core_image::ImageBuf,
    decode: (usize, usize),
    lens: Option<&lens_profile::LensShot>,
) -> Option<std::sync::Arc<pipeline::BrushCoverage>> {
    if crate::ai_coverage::prompts(edits).is_empty() {
        return None;
    }
    let dm = crate::ai_space::DecodeMap::new(
        &edits.geometry,
        edits.perspective,
        frame.width(),
        frame.height(),
        None,
        decode.0,
        decode.1,
    );
    let key = crate::ai_space::lens_key(lens, edits, decode.0, decode.1);
    let maps: Vec<Vec<Option<std::sync::Arc<pipeline::brush::CoverageMap>>>> = edits
        .masks
        .iter()
        .map(|m| {
            std::iter::once(&m.mask)
                .chain(m.refine.iter().map(|r| &r.mask))
                .map(|mask| match mask {
                    edit_model::Mask::Ai(a) => maps.get(&a.prompt).map(|p| {
                        let lensed = crate::ai_coverage::post_lens(p, key, decode, || {
                            lens.and_then(|l| {
                                crate::ai_space::corrector_for(l, edits, decode.0, decode.1)
                            })
                        });
                        std::sync::Arc::new(crate::ai_coverage::to_frame(&lensed, a, &dm, None))
                    }),
                    // ⚠ Every other component builds its own coverage exactly as before.
                    _ => None,
                })
                .collect()
        })
        .collect();
    let built = maps
        .iter()
        .map(|row| row.iter().map(|_| std::sync::Mutex::new(None)).collect())
        .collect();
    Some(std::sync::Arc::new(pipeline::BrushCoverage { maps, built }))
}

/// What a decode hands the exporter.
///
/// ⚠⚠ **A struct rather than a growing tuple, and the third field is why it exists.** Until M8.9
/// this was `(ImageBuf, Illuminant)` and the lens shot was simply **discarded** — which is how the
/// GUI's export came to render every RAW with **no lens correction at all** while the preview and
/// `rustroom render` both applied it. Measured on an X-T4 frame at a mild 19.2 mm f/8: worst 187
/// of 255 levels, mean 6.95, and 33.5% of samples differing by more than 4. A named field is
/// harder to drop than a tuple element.
pub struct ExportSource {
    pub image: ImageBuf,
    /// The white-balance baseline the decode left the picture at. Drop it and every exported RAW
    /// gets D65's gains where the preview used the camera's own.
    pub baseline: color::wb::Illuminant,
    /// The matched lens profile; `None` for a JPEG, or for a lens the database does not list.
    pub lens: Option<lens_profile::LensShot>,
    /// The camera profile's `ProfileToneCurve`, when it has one.
    ///
    /// ⚠⚠ **Owned, because [`pipeline::TailRendering`] BORROWS** — the borrow has to outlive the
    /// whole render, and this struct is what lives that long. The same shape `lens` above already
    /// takes.
    ///
    /// ⚠ Derived by the *same* `decode::profile_rendering` helper the preview's decode uses. Two
    /// derivations of "what this profile renders with" is how a preview and an export come to
    /// disagree, which is the bug this field exists to close.
    pub curve: Option<Vec<[f32; 2]>>,
    /// The camera profile's `BaselineExposure` in stops.
    ///
    /// ⚠ `None` is not `Some(0.0)`: a profile that declares no shift has made a statement and one
    /// that says nothing has not — `TailRendering::baseline_ev`'s own rule.
    pub baseline_ev: Option<f32>,
    /// Whether this buffer holds **scene-referred** light — true for a RAW, false for a JPEG, PNG
    /// or TIFF that has already been rendered by whatever produced it.
    ///
    /// ⚠⚠ **Without this an already-rendered photograph is rendered TWICE at export, and it was.**
    /// `render_cached`'s worker strips `Edits.rendering` for a non-scene-referred buffer — *"a
    /// source that is already rendered never renders again"* — and the export path had no way to
    /// know, so the preview stripped it and the export did not. **Measured at 119.32 levels with
    /// 100% of samples differing**, which is what a preview/export divergence reported by use
    /// looked like. `examples/probe_jpeg_rendering_gap.rs`.
    pub scene_referred: bool,
    /// The photo's cached AI mask maps (M9.13) — `ai_coverage::maps_for`, the preview's own
    /// lookup. A prompt with no map renders as nothing and is reported.
    pub ai: crate::ai_coverage::AiMaps,
}

/// What the *render* needs that [`ExportOptions`] deliberately does not carry.
///
/// ⚠⚠ **Neither field may become an `ExportOptions` field, and the reason is already written down
/// in [`ExportWorker::spawn`]'s own doc comment**: in `ExportOptions` they would be saved into
/// every export preset, so a preset could disagree with the preview about how the photograph is
/// rendered. Two facts that can disagree.
///
/// ⚠ `gpu_tail` is named for the *tail* and now governs three things — the tail, the preview's
/// prefix seam, and this. That naming hazard was flagged and accepted (user's call, 2026-09-15);
/// the Settings hint is what carries the correction, by describing the rule rather than
/// enumerating what it reaches.
#[derive(Clone, Copy)]
pub struct ExportRender {
    pub resampler: pipeline::Resampler,
    pub gpu_tail: bool,
    /// Which network's cached map a *Subject* mask reads (M9.13) — the Settings' choice, the same
    /// one the preview used.
    pub ai_subject: ai_mask::SubjectModel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::fixtures::*;
    use export::ExportFormat;

    /// A **non-identity** tone curve, for the camera-rendering tests.
    ///
    /// ⚠⚠ **Non-identity is the whole point, and an identity one is how this bug survived.** The
    /// catalogue's own X-T4 profile carries exactly `[[0,0],[1,1]]`, which
    /// `pipeline::effective_profile_curve` rejects in favour of the built-in curve — so a test
    /// written against that profile passes with the fix **reverted**, and measures nothing. A fix
    /// that changes every exported RAW cannot be guarded by a fixture that cannot see it.
    fn real_curve() -> Vec<[f32; 2]> {
        (0..=16)
            .map(|i| {
                let x = i as f32 / 16.0;
                [x, x.powf(0.72)]
            })
            .collect()
    }

    /// M9.13: an export applies a photo's cached AI map exactly where it covers — and without the
    /// map the mask covers nothing, so the file equals one exported with no mask at all.
    #[test]
    fn an_exported_ai_mask_applies_its_map_and_only_there() {
        let export = |tag: &str, masks: bool, map: bool| -> image::RgbImage {
            let d = TempDir::new(tag);
            let mut o = opts(&d, None);
            o.format = ExportFormat::Tiff8;
            let dest = d.0.join("out.tif");
            let e = Edits {
                masks: if masks {
                    vec![edit_model::MaskedAdjustment {
                        mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
                        refine: Vec::new(),
                        adjustments: edit_model::LocalAdjustments {
                            exposure: -2.0,
                            ..Default::default()
                        },
                        disabled: false,
                    }]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let mut src = source(flat(80, 40));
            if map {
                // The subject is the LEFT half of the decode.
                src.ai.insert(
                    edit_model::AiPrompt::Subject,
                    std::sync::Arc::new(ai_mask::ProbMap::from_fn(40, 20, |x, _| {
                        if x < 20 {
                            1.0
                        } else {
                            0.0
                        }
                    })),
                );
            }
            export_one(
                src,
                &e,
                &dest,
                &o,
                CPU,
                &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
                None,
            )
            .unwrap();
            image::open(&dest).unwrap().to_rgb8()
        };
        let with = export("ai_on", true, true);
        let (l, r) = (with.get_pixel(10, 20)[0], with.get_pixel(70, 20)[0]);
        assert!(l + 20 < r, "the covered half is not darker: {l} vs {r}");
        let plain = export("ai_none", false, false);
        assert_eq!(
            with.get_pixel(70, 20),
            plain.get_pixel(70, 20),
            "the uncovered half changed"
        );
        let no_map = export("ai_nomap", true, false);
        assert_eq!(
            no_map.as_raw(),
            plain.as_raw(),
            "a mask without its map changed the file"
        );
    }

    /// Export one photo with a given rendering and return the file's bytes.
    fn rendered(tag: &str, curve: Option<Vec<[f32; 2]>>, baseline_ev: Option<f32>) -> Vec<u8> {
        let d = TempDir::new(tag);
        let mut o = opts(&d, None);
        // ⚠ TIFF for `exported_with`'s reason: a lossy encoder would quantize the difference.
        o.format = ExportFormat::Tiff8;
        let dest = d.0.join("out.tif");
        // ⚠⚠ The rendering stage is gated on `edits.applied_rendering()`, which defaults to
        // `Rendering::None` — so without this the pipeline builds NO stage and every variant below
        // is byte-identical. That is exactly how the first run of `probe_export_rendering`
        // measured 0.00 for everything and read like "no effect".
        let e = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        export_one(
            ExportSource {
                ai: Default::default(),
                image: edged(160, 120),
                baseline: color::wb::Illuminant::default(),
                lens: None,
                curve,
                baseline_ev,
                scene_referred: true,
            },
            &e,
            &dest,
            &o,
            CPU,
            &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
            None,
        )
        .unwrap();
        std::fs::read(&dest).unwrap()
    }

    /// ⚠⚠ **The test of this fix: the camera profile's tone curve reaches an exported file.**
    /// Until 2026-09-18 `export_one` passed `TailRendering::default()`, so an export rendered
    /// with the built-in curve while the preview consulted the profile.
    ///
    /// ⚠ Sabotage: put `curve: None` back into `export_one`'s `TailRendering`.
    #[test]
    fn the_profiles_tone_curve_reaches_an_exported_file() {
        let without = rendered("render_curve_off", None, None);
        let with = rendered("render_curve_on", Some(real_curve()), None);
        assert_ne!(
            without, with,
            "a profile's tone curve must change the exported pixels"
        );
    }

    /// ⚠⚠ **The other half, and it is a SEPARATE test on purpose.** Curve and baseline exposure
    /// travel together through one struct, so one test asserting "the rendering reaches an export"
    /// would pass with either half still withheld — the guard-that-enumerates failure this repo
    /// records.
    ///
    /// ⚠ Sabotage: put `baseline_ev: None` back.
    #[test]
    fn the_profiles_baseline_exposure_reaches_an_exported_file() {
        let without = rendered("render_base_off", Some(real_curve()), None);
        let with = rendered("render_base_on", Some(real_curve()), Some(0.5));
        assert_ne!(
            without, with,
            "a profile's baseline exposure must change the exported pixels"
        );
    }

    /// ⚠⚠ **An already-rendered source must not be rendered again at export — FOUND BY USE.**
    /// `render_cached`'s worker strips `Edits.rendering` for a non-scene-referred buffer; the
    /// export path could not, because `ExportSource` carried no such flag. So a JPEG with the
    /// camera rendering switched on looked one way on screen and another in the file — **measured
    /// at 119.32 levels, 100% of samples**, which is what the divergence reported by use looked
    /// like.
    ///
    /// ⚠ Sabotage: make `export_one` use `edits` unconditionally instead of the stripped copy.
    #[test]
    fn an_already_rendered_source_is_not_rendered_again_at_export() {
        let e = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let write = |tag: &str, scene_referred: bool| {
            let d = TempDir::new(tag);
            let mut o = opts(&d, None);
            o.format = ExportFormat::Tiff8;
            let dest = d.0.join("out.tif");
            export_one(
                ExportSource {
                    ai: Default::default(),
                    image: edged(160, 120),
                    baseline: color::wb::Illuminant::default(),
                    lens: None,
                    curve: None,
                    baseline_ev: None,
                    scene_referred,
                },
                &e,
                &dest,
                &o,
                CPU,
                &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
                None,
            )
            .unwrap();
            std::fs::read(&dest).unwrap()
        };
        // ⚠ The fixture check: with a scene-referred source the rendering DOES run, so the two
        // differ. Without it, "already-rendered exports unrendered" would be vacuously true for a
        // build that never renders at all.
        let raw_like = write("strip_raw", true);
        let already = write("strip_jpeg", false);
        assert_ne!(
            raw_like, already,
            "fixture check: the camera rendering must actually do something here"
        );

        // And what an already-rendered source exports must equal what it exports with the
        // rendering simply switched off — which is what the preview shows.
        let d = TempDir::new("strip_off");
        let mut o = opts(&d, None);
        o.format = ExportFormat::Tiff8;
        let dest = d.0.join("off.tif");
        export_one(
            ExportSource {
                ai: Default::default(),
                image: edged(160, 120),
                baseline: color::wb::Illuminant::default(),
                lens: None,
                curve: None,
                baseline_ev: None,
                scene_referred: false,
            },
            &Edits::default(),
            &dest,
            &o,
            CPU,
            &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
            None,
        )
        .unwrap();
        assert_eq!(
            already,
            std::fs::read(&dest).unwrap(),
            "an already-rendered source must export exactly as the preview renders it"
        );
    }

    /// A 2³ cube that **darkens everything to 40%**.
    ///
    /// ⚠⚠ **It must change a NEUTRAL image, and the first version did not.** A cube that swapped
    /// green into red looks obviously "loud" and is a perfect no-op on grey — and `edged`, like
    /// every fixture here, is grey (`[0.20; 3]` and `[0.55; 3]`). The test failed for the fixture's
    /// reason rather than the wiring's, which is the same trap `real_curve` above documents: a
    /// fixture that cannot see the thing under test proves nothing about it.
    ///
    /// ⚠ A uniform scale of the corners is exactly a uniform scale after trilinear interpolation,
    /// so this is `out = 0.4 * in` everywhere rather than only at the corners.
    fn loud_cube() -> std::sync::Arc<pipeline::lut::Cube> {
        let mut t = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    let f = |v: usize| v as f32 * 0.4;
                    t.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        std::sync::Arc::new(pipeline::lut::Cube::parse(&t).expect("a parseable cube"))
    }

    /// ⚠⚠ **The creative look reaches an exported file — and this half was NOT in the backlog
    /// entry.** It was found while scoping the tone-curve fix: `TailRendering` carries three
    /// things and `export_one` was defaulting all three, so a look never ran at export either.
    ///
    /// ⚠ Sabotage: pass `look: None` in `export_one`'s `TailRendering`.
    #[test]
    fn the_creative_look_reaches_an_exported_file() {
        let d = TempDir::new("render_look");
        let mut o = opts(&d, None);
        o.format = ExportFormat::Tiff8;
        let e = Edits {
            look: edit_model::Look {
                file: Some("loud.cube".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let write = |name: &str, look: Option<&std::sync::Arc<pipeline::lut::Cube>>| {
            let dest = d.0.join(name);
            export_one(
                ExportSource {
                    ai: Default::default(),
                    image: edged(160, 120),
                    baseline: color::wb::Illuminant::default(),
                    lens: None,
                    curve: None,
                    baseline_ev: None,
                    scene_referred: true,
                },
                &e,
                &dest,
                &o,
                CPU,
                &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
                look,
            )
            .unwrap();
            std::fs::read(&dest).unwrap()
        };
        let cube = loud_cube();
        assert_ne!(
            write("without.tif", None),
            write("with.tif", Some(&cube)),
            "a look handed to the export must change the exported pixels"
        );
    }

    /// Read back a written JPEG's dimensions.
    fn jpeg_size(path: &Path) -> (u32, u32) {
        let img = image::open(path).expect("a readable jpeg");
        (img.width(), img.height())
    }

    #[test]
    fn export_one_writes_a_jpeg_at_full_size_without_a_cap() {
        let d = TempDir::new("one_full");
        let dest = d.0.join("a.jpg");
        export_one(
            source(flat(40, 20)),
            &Edits::default(),
            &dest,
            &opts(&d, None),
            CPU,
            &export::FileMetadata::default(),
            None,
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn the_long_edge_cap_is_honoured() {
        let d = TempDir::new("one_cap");
        let dest = d.0.join("a.jpg");
        export_one(
            source(flat(40, 20)),
            &Edits::default(),
            &dest,
            &opts(&d, Some(10)),
            CPU,
            &export::FileMetadata::default(),
            None,
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (10, 5));
    }

    #[test]
    fn a_cap_larger_than_the_image_does_not_upscale() {
        // `downscale_to` never upscales; the export must not either.
        let d = TempDir::new("one_big_cap");
        let dest = d.0.join("a.jpg");
        export_one(
            source(flat(40, 20)),
            &Edits::default(),
            &dest,
            &opts(&d, Some(999)),
            CPU,
            &export::FileMetadata::default(),
            None,
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    /// A hard vertical edge on a mid-grey field, big enough to hold a watermark.
    fn edged(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, if x < w / 2 { [0.20; 3] } else { [0.55; 3] });
            }
        }
        img
    }

    /// A synthetic lens with coefficients chosen here rather than read from the bundled database.
    ///
    /// ⚠ Copied from `pipeline`'s and `gpu-pipeline`'s own fixtures rather than shared: they live
    /// in other crates' test modules, and exporting a fixture across a crate boundary is more
    /// machinery than eight lines is worth.
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

    fn lens_on() -> Edits {
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

    /// ⚠⚠ **THE CARRIER FOR THE M8.9 FIX, and the bug it guards had shipped for the life of the
    /// export dialog.** `export_one` used to build through `pipeline::build_pipeline_spec`, which
    /// hardcodes `lens: None` — so every RAW was written out with **no lens correction** while the
    /// preview and `rustroom render` both applied it. Measured through the shipped CLI on an X-T4
    /// frame at a mild 19.2 mm f/8: **worst 187 of 255 levels, mean 6.95, and 33.5% of samples
    /// differing by more than 4.**
    ///
    /// ⚠ Nothing in `pipeline` could ever have seen it, because the stage itself was never broken
    /// — which is why this test has to go through `export_one` rather than through the pipeline.
    /// The W5 failure mode: fully tested, and never reached.
    #[test]
    fn a_lens_profile_reaches_the_exported_pixels() {
        let d = TempDir::new("one_lens");
        let dest = d.0.join("a.png");
        let o = ExportOptions {
            format: ExportFormat::Png8,
            ..opts(&d, None)
        };
        let write = |lens: Option<lens_profile::LensShot>| {
            export_one(
                ExportSource {
                    ai: Default::default(),
                    image: edged(160, 120),
                    baseline: color::wb::Illuminant::default(),
                    lens,
                    curve: None,
                    baseline_ev: None,
                    scene_referred: true,
                },
                &lens_on(),
                &dest,
                &o,
                CPU,
                &export::FileMetadata::default(),
                None,
            )
            .unwrap();
            std::fs::read(&dest).unwrap()
        };
        let without = write(None);
        let with = write(Some(lens_fixture()));
        assert_ne!(
            without, with,
            "a matched lens profile must change the exported file"
        );
    }

    /// ⚠⚠ **THE CARRIER FOR THE SEAM, and a picture test cannot be one.** An accelerated export and
    /// a CPU export produce the same photograph **by design**, so every correctness test here
    /// passes with the whole offer deleted. The observable is the count of steps that crossed.
    ///
    /// ⚠ The fixture drives **dehaze**, not lens correction, deliberately: dehaze is already
    /// ported and needs no profile, so this says something about the **wiring** independently of
    /// whether the lens shader exists at all. That is what makes this half of the milestone worth
    /// landing on its own merits.
    #[test]
    fn an_accelerated_export_really_uses_the_seam() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let d = TempDir::new("one_seam");
        let dest = d.0.join("a.jpg");
        let edits = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Edits::default()
        };
        let run = |gpu_tail: bool| {
            accelerated_steps(|| {
                export_one(
                    source(edged(160, 120)),
                    &edits,
                    &dest,
                    &opts(&d, None),
                    ExportRender {
                        ai_subject: ai_mask::SubjectModel::IsNet,
                        resampler: pipeline::Resampler::CatmullRom,
                        gpu_tail,
                    },
                    &export::FileMetadata::default(),
                    None,
                )
                .unwrap();
            })
        };
        // ⚠ Both halves: an offer that never fires and one that fires unconditionally are equally
        // wrong, and only the pair distinguishes them.
        assert_eq!(run(false), 0, "with the setting off nothing may cross");
        assert!(
            run(true) >= 1,
            "with it on, dehaze — which is ported — must cross"
        );
    }

    /// Export `edged` twice — once neutral, once with `tweak` applied to the options — and
    /// report the written bytes of each.
    ///
    /// ⚠⚠ **THESE THREE TESTS EXIST BECAUSE THREE SABOTAGES WENT UNCAUGHT.** Deleting each of
    /// `export_one`'s three new calls left the entire suite green: `export::metadata`,
    /// `export::watermark` and `export::output_sharpen` are all thoroughly tested *in their own
    /// crate*, and nothing anywhere asserted that the export path actually reaches them. That is
    /// the "feature silently absent" failure this project has already shipped once, in
    /// edit-accurate thumbnails.
    fn exported_with(tag: &str, tweak: impl FnOnce(&mut ExportOptions)) -> (Vec<u8>, Vec<u8>) {
        // ⚠ TIFF, not JPEG: a lossy encoder quantizes a faint watermark or a one-pixel sharpen
        // halo into the same blocks, so "the bytes differ" would be measuring the encoder.
        let d = TempDir::new(tag);
        let mut base = opts(&d, None);
        base.format = ExportFormat::Tiff8;
        let plain = d.0.join("plain.tif");
        export_one(
            source(edged(160, 120)),
            &Edits::default(),
            &plain,
            &base,
            CPU,
            &export::FileMetadata::resolve(&base.metadata, export::FileMetadata::default()),
            None,
        )
        .unwrap();
        let mut changed = base.clone();
        tweak(&mut changed);
        let other = d.0.join("other.tif");
        export_one(
            source(edged(160, 120)),
            &Edits::default(),
            &other,
            &changed,
            CPU,
            &export::FileMetadata::resolve(&changed.metadata, export::FileMetadata::default()),
            None,
        )
        .unwrap();
        (
            std::fs::read(&plain).unwrap(),
            std::fs::read(&other).unwrap(),
        )
    }

    /// The decoded pixels of a written file.
    fn pixels(bytes: &[u8]) -> Vec<u8> {
        image::load_from_memory(bytes).unwrap().to_rgb8().into_raw()
    }

    #[test]
    fn an_export_carries_its_metadata() {
        let (plain, tagged) = exported_with("e2e_meta", |o| {
            o.metadata.copyright = "(c) 2026 Rustroom".into()
        });
        let needle = b"(c) 2026 Rustroom";
        assert!(
            !plain.windows(needle.len()).any(|w| w == needle),
            "fixture check: the neutral export already carried it"
        );
        assert!(
            tagged.windows(needle.len()).any(|w| w == needle),
            "the copyright never reached the file"
        );
        // ⚠ And only the container moved: metadata is written after the encode, so it cannot
        // change what the photograph looks like.
        assert_eq!(
            pixels(&plain),
            pixels(&tagged),
            "the metadata moved a pixel"
        );
    }

    #[test]
    fn a_photos_own_caption_reaches_the_file_and_beats_the_presets() {
        // ⚠⚠ The carrier test W5 taught: the packet writer is thoroughly tested in its own
        // crate, and nothing there proves the worker builds the payload from the job. Deleting
        // the `file_metadata` call leaves `export` green and this red.
        let d = TempDir::new("e2e_desc");
        let mut opts = opts(&d, None);
        opts.format = ExportFormat::Tiff8;
        opts.metadata.caption = "preset caption".into();
        opts.metadata.copyright = "(c) preset".into();
        let mut job = jobs(&d, &["one.RAF"]).remove(0);
        job.description = catalog::db::Description {
            title: "Harbour at dusk".into(),
            caption: "Boats returning".into(),
            ..Default::default()
        };
        job.marks = catalog::db::Marks {
            rating: 4,
            label: 2,
            ..Default::default()
        };
        job.keywords = vec![vec!["Europe".into(), "UK".into()]];
        let (report, _) = run_export(vec![job], opts, Vec::new(), ok_decode());
        assert_eq!(report.exported, 1, "{:?}", report.failed);
        let bytes = std::fs::read(d.0.join("one.tif")).unwrap();
        let has = |s: &str| bytes.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has("Harbour at dusk"), "the title never reached the file");
        assert!(
            has("Boats returning"),
            "the photo's caption never reached the file"
        );
        assert!(
            !has("preset caption"),
            "the preset's caption overrode the photo's"
        );
        assert!(
            has("(c) preset"),
            "the preset's copyright did not fill the blank"
        );
        assert!(has("<xmp:Rating>4</xmp:Rating>"), "the rating is missing");
        assert!(has("<xmp:Label>Blue</xmp:Label>"), "label 2 is Blue");
        assert!(has("Europe|UK"), "the keyword path is missing");
    }

    #[test]
    fn copyright_only_strips_the_photos_own_words_at_export() {
        let d = TempDir::new("e2e_scope");
        let mut opts = opts(&d, None);
        opts.format = ExportFormat::Tiff8;
        opts.metadata.copyright = "(c) preset".into();
        opts.metadata.scope = export::MetadataScope::CopyrightOnly;
        let mut job = jobs(&d, &["one.RAF"]).remove(0);
        job.description.caption = "Boats returning".into();
        job.marks.rating = 5;
        let (report, _) = run_export(vec![job], opts, Vec::new(), ok_decode());
        assert_eq!(report.exported, 1);
        let bytes = std::fs::read(d.0.join("one.tif")).unwrap();
        let has = |s: &str| bytes.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has("(c) preset"));
        assert!(!has("Boats returning"), "the scope was not applied");
        assert!(
            !has("xmp:Rating"),
            "the scope was not applied to the rating"
        );
    }

    #[test]
    fn an_export_carries_its_watermark() {
        let (plain, marked) = exported_with("e2e_mark", |o| o.watermark.text = "Rustroom".into());
        assert_ne!(
            pixels(&plain),
            pixels(&marked),
            "the watermark never reached the pixels"
        );
    }

    #[test]
    fn an_export_carries_its_output_sharpening() {
        let (plain, sharp) = exported_with("e2e_sharp", |o| o.output_sharpening.amount = 100);
        let (a, b) = (pixels(&plain), pixels(&sharp));
        assert_ne!(a, b, "output sharpening never reached the pixels");
        // ⚠ And it is *sharpening*, not a global lift: the flat halves must be untouched while
        // the edge moves. Without this the test would pass for any change at all — including a
        // watermark, which is the other thing this call site could have been wired to.
        let at = |v: &[u8], x: usize| v[(60 * 160 + x) * 3] as i32;
        assert_eq!(at(&a, 5), at(&b, 5), "the flat left half moved");
        assert_eq!(at(&a, 155), at(&b, 155), "the flat right half moved");
        assert!(
            at(&b, 80) > at(&a, 80) && at(&b, 79) < at(&a, 79),
            "no overshoot/undershoot at the edge: {:?} vs {:?}",
            (at(&a, 79), at(&a, 80)),
            (at(&b, 79), at(&b, 80))
        );
    }

    /// ⚠⚠ M6.4c: the preset's intent reaches the file. `export_one` building its `RenderSpec`
    /// with the default instead compiles perfectly. The source is SATURATED — `edged` is grey,
    /// which no intent moves, so it would pass with the field ignored.
    ///
    /// ⚠ Sabotage: `intent: color::RenderingIntent::default()` in `export_one`'s spec.
    #[test]
    fn the_rendering_intent_reaches_an_exported_file() {
        let d = TempDir::new("render_intent");
        let mut img = ImageBuf::new(64, 8);
        for y in 0..8 {
            for x in 0..64 {
                let t = x as f32 / 63.0;
                img.set_pixel(x, y, [0.04, 0.25 + 0.6 * t, 0.05]);
            }
        }
        let write = |name: &str, intent| {
            let mut o = opts(&d, None);
            o.format = ExportFormat::Tiff8;
            o.intent = intent;
            let dest = d.0.join(name);
            export_one(
                source(img.clone()),
                &Edits::default(),
                &dest,
                &o,
                CPU,
                &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
                None,
            )
            .unwrap();
            pixels(&std::fs::read(&dest).unwrap())
        };
        assert_ne!(
            write("rel.tif", color::RenderingIntent::RelativeColorimetric),
            write("per.tif", color::RenderingIntent::Perceptual),
            "the preset's intent never reached the exported pixels"
        );
    }

    /// ⚠⚠ M9.10 through the real export: a DNG with metadata opens in libraw at the photo's size,
    /// and the XMP post-pass — which rewrites IFD0 — has left the image's SubIFD readable.
    #[test]
    fn a_dng_export_opens_in_libraw_and_carries_its_metadata() {
        let d = TempDir::new("render_dng");
        let mut o = opts(&d, None);
        o.format = ExportFormat::Dng;
        o.metadata.copyright = "(c) 2026 Rustroom".into();
        let dest = d.0.join("x.dng");
        export_one(
            source(edged(160, 120)),
            &Edits::default(),
            &dest,
            &o,
            CPU,
            &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
            None,
        )
        .unwrap();
        let bytes = std::fs::read(&dest).unwrap();
        assert!(
            bytes.windows(17).any(|w| w == b"(c) 2026 Rustroom"),
            "the XMP was not embedded"
        );
        let decoded = raw_decode::decode_raw(&dest, &raw_decode::DecodeOptions::linear_scene())
            .expect("libraw must open the exported DNG");
        assert_eq!((decoded.image.width(), decoded.image.height()), (160, 120));
    }

    #[test]
    fn an_unwritable_destination_is_an_error_not_a_panic() {
        let d = TempDir::new("one_bad");
        let dest = d.0.join("no_such_dir").join("a.jpg");
        assert!(export_one(
            source(flat(4, 4)),
            &Edits::default(),
            &dest,
            &opts(&d, None),
            CPU,
            &export::FileMetadata::default(),
            None,
        )
        .is_err());
    }

    #[test]
    fn the_render_scale_follows_the_downscale() {
        // ⚠ The reason the resize happens *before* the pipeline: the spatial stages have to
        // be told the output resolution, or a resized export would apply full-resolution
        // radii to a small image.
        assert_eq!(scale_for(40, None).get(), 1.0);
        assert_eq!(scale_for(40, Some(10)).get(), 0.25);
        // No upscaling, so a cap above the source is still 1.0.
        assert_eq!(scale_for(40, Some(999)).get(), 1.0);
    }
}

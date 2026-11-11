//! One render: resume the prefix from its deepest checkpoint, run the tail, pack the frame.

use super::brush_cache::{BrushCache, CoverageSpec};
use super::frame::{FrameSpec, Ran, RenderedFrame};
use super::pack::{pack, Packed};
use super::scale::{render_scale, rendered_long_edge};
use crate::prefix_cache::PrefixCache;
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::Region;
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    /// Test-only: refuse the tail its resident final for one whole render, so a handoff frame
    /// takes the download fallback. Every tail stage has a GPU twin, so no real edit set produces
    /// the refusal.
    static REFUSE_THE_RESIDENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`render_cached_with`] with the handoff OFF — every checkpoint cached, as it always was. The
/// worker's settle pass and every test that predates the handoff render through this.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_cached(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    spec: FrameSpec,
    source_scale: f32,
    lens: Option<lens_profile::LensShot>,
    wb_baseline: color::wb::Illuminant,
    rendering: pipeline::TailRendering<'_>,
    cache: &mut PrefixCache,
    brush_cache: &mut BrushCache,
) -> (RenderedFrame, Ran) {
    render_cached_with(
        buffer,
        edits,
        spec,
        source_scale,
        lens,
        wb_baseline,
        rendering,
        cache,
        brush_cache,
        false,
    )
}

/// Render one request, resuming from the deepest cached prefix checkpoint.
///
/// The prefix — geometry, noise reduction, dehaze — is by far the expensive half, and a slider
/// move usually changes only one of its stages, or none. So each stage's output is cached
/// separately and the walk restarts at the deepest checkpoint still valid, running only what is
/// downstream of the change. See
/// `docs/superpowers/specs/2026-08-12-second-prefix-checkpoint-design.md`.
///
/// Returns the frame and **how many prefix stages actually ran**. The worker ignores the count;
/// it exists because the correctness tests would all still pass with the cache deleted, so
/// something has to assert that work is genuinely being skipped.
///
/// ⚠⚠ `handoff` lets the LAST prefix step hand its output to the tail on the GPU with no download
/// (2026-09-26). The frame then leaves no final checkpoint, so only the live render asks for it;
/// the worker's settle pass renders again without it, which is what fills the cache on release.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_cached_with(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    spec: FrameSpec,
    source_scale: f32,
    lens: Option<lens_profile::LensShot>,
    wb_baseline: color::wb::Illuminant,
    rendering: pipeline::TailRendering<'_>,
    cache: &mut PrefixCache,
    brush_cache: &mut BrushCache,
    handoff: bool,
) -> (RenderedFrame, Ran) {
    // Destructured so the body below reads exactly as it did before these three were grouped.
    let FrameSpec {
        max_dim,
        region,
        display_space,
        intent,
        resampler,
        gpu_tail,
        visualize,
        soft_proof,
        grade_view,
    } = spec;
    let buffer_long = buffer.width().max(buffer.height());
    // ⚠ Kept for the AI masks' decode mapping (M9.13): `lens` is moved into the prefix below.
    let ai_lens = lens.clone();
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );

    // ⚠ A snapshot of the maps — a handful of `Arc`s — so the read guard is not held across the render.
    let ai_maps = brush_cache.ai.read().expect("ai maps").clone();
    let walk = super::prefix::plan(edits, buffer, lens, scale, &spec, cache, &ai_maps);
    let debug_steps = walk.debug_steps;
    // ⚠⚠ **When the last step may run WITHOUT a download.** A whole-frame render, sRGB display,
    // and none of the views that need float pixels (proof, visualize, grade range). Its output
    // then lives only as the resident final; the tail runs from it, and the settle downloads it.
    let handoff_ok = handoff
        && gpu_tail
        && region.is_none()
        && grade_view.is_none()
        && visualize.is_none()
        && soft_proof.is_none()
        && display_space == color::OutputSpace::Srgb;
    let (start, img) = super::prefix::resume(&walk, buffer, buffer_long, cache);
    let (scoped, was_scoped) = super::scoped::scope(&walk, region, start, &img);
    let super::prefix::PrefixOut {
        input,
        rect,
        pw,
        ph,
        prefix_stages,
        img,
        gpu_prefix_steps,
        gpu_prefix_uploads,
        handed,
    } = match scoped {
        Some(s) => super::scoped::output(s, img),
        None => super::prefix::walk_whole(&walk, handoff_ok, region, start, img, cache),
    };
    let region_scoped = was_scoped;

    // ⚠⚠ **The frame dimensions are `(pw, ph)` — the prefix output's — and on the region-scoped
    // path they come from `output_dims` rather than from the buffer.** That buffer *is* the region
    // there, so taking its size would rasterise every brush mask at the region's size and thrash
    // the coverage cache on every pan.
    let region = rect.map(|r| Region {
        full_w: pw,
        full_h: ph,
        x0: r.x,
        y0: r.y,
    });

    // ⚠ The frame dimensions are the PREFIX OUTPUT's — `(pw, ph)` — not the region's. That image
    // *is* the frame. Passing the region's would rasterize at the wrong size and, because the dims
    // are in the key, would also thrash the cache on every pan.
    // ⚠ The clip comes from `rect`, which carries the region's own width and height. `Region`
    // holds only the origin plus the full frame's size, so deriving the size as "frame minus
    // origin" would clip to the bottom-right corner of the frame rather than to the viewport —
    // larger than intended, and silently so, because the output would still be correct and merely
    // slow.
    let clip = rect.map(|r| (r.x, r.y, r.w, r.h));
    let cov_spec = CoverageSpec {
        fw: pw,
        fh: ph,
        region: clip,
        max_dim,
    };
    let t_cov = std::time::Instant::now();
    let coverage = brush_cache.get_or_build(
        edits,
        cov_spec,
        (buffer.width(), buffer.height()),
        ai_lens.as_ref(),
    );
    if debug_steps {
        eprintln!(
            "  TIMING coverage {:6.1} ms",
            t_cov.elapsed().as_secs_f64() * 1000.0
        );
    }
    // ⚠⚠ **M8.3g's gate, derived ONCE and passed down rather than re-derived.** Three things
    // downstream need the float image, and each is a view mode or a setting:
    //
    //   1. a **non-sRGB display space** — `pack` converts from the float deliberately, because an
    //      8-bit → 8-bit conversion costs roughly 0.8 bits and bands a smooth sky. ⚠ Opt-in and
    //      Linux/Windows-only: on macOS `tag_as_srgb` succeeds, so `effective_display_space`
    //      returns `Srgb` whatever the dropdown says.
    //   2. **Visualize Spots** — `edge_image` is computed from the float.
    //   3. **soft proofing** — the ProPhoto run is decoded back to the working space to test the
    //      gamut, which an 8-bit round trip cannot support.
    //
    // ⚠ The **grade range view** is a fourth and needs nothing: it bypasses `accelerate` entirely,
    // building its own halves through `build_pipeline_split_tapped`.
    //
    // ⚠ A four-part condition spanning two crates is the shape this project's record says goes
    // wrong silently, so the other half — "is this chain last, and does it end in
    // `output_transform`" — is answered by construction inside `accelerate` rather than here.
    let float_needed =
        display_space != color::OutputSpace::Srgb || visualize.is_some() || soft_proof.is_some();
    let tail = super::tail::TailSpec {
        edits,
        scale,
        region,
        coverage: &coverage,
        intent,
        resampler,
        airlight: walk.airlight,
        wb_baseline,
        rendering,
        gpu_tail,
    };

    // ⚠⚠ **While proofing the tail runs TWICE, and that was MEASURED into existence rather than
    // argued.** A single ProPhoto run was the first design; run over 12 real X-T4 frames, 12 of 12
    // histograms moved — worst **five 8-bit levels** with the camera rendering off, because
    // ProPhoto's `encode` ceilings genuine scene headroom at 1.0 *before* the sRGB matrix mixes the
    // channels, where the direct path clamps only afterwards. So the histogram comes from an
    // unmodified sRGB run and is bit-identical **by construction**, not by luck, and the texture
    // and the warning come from a ProPhoto run that reaches `pack` near-unclipped.
    //
    // ⚠ Cost: one extra tail run per proofed frame — ~7 ms at fit, ~13 ms at a native rung —
    // paid only while the proof is switched on. That is accepted; it is not something to optimise
    // away by collapsing the two runs back into one.
    //
    // ⚠ Suppressed while Visualize Spots is on. That view replaces the photograph with an
    // achromatic edge view, so proofing it would mark nothing and show nothing; suppressing it
    // here also means the extra run is never paid for a frame that could not use it. The two are
    // both view modes applied at this point and the diagnostic one wins.
    // ⚠⚠ **The tail's input, and where it lives (2026-09-26).** `tail_key` names it: the final
    // checkpoint, at this rect. `PrefixCache::resident_final` holds it on the GPU under exactly
    // that key when a previous frame ran from it, or the last prefix step just kept it. A tail
    // that is one GPU chain runs FROM it; a tail that cannot (a CPU stage first, a GPU failure)
    // gets a CPU image — and on a handoff frame that means downloading the resident, which is
    // the settle's own path and caches the final as a side effect. Nothing else in this function
    // needs to know which happened.
    let tail_key = (walk.key_at(walk.steps.len()), rect);
    let refused = {
        #[cfg(test)]
        {
            REFUSE_THE_RESIDENT.with(|l| l.get())
        }
        #[cfg(not(test))]
        {
            false
        }
    };
    let run = super::tail::TailRun {
        tail,
        walk: &walk,
        tail_key: &tail_key,
        refused,
    };
    let (input, from_resident, k) = super::tail::resolve_input(run, input, pw, ph, &img, cache);
    let proof = soft_proof.filter(|_| visualize.is_none());
    // ⚠ `input` is cloned **only** when proofing. With the proof off there is one tail, one run,
    // and no clone — bit-exactly the path that existed before this feature.
    let proofed = proof.map(|p| {
        // ⚠ Rejoined at once: the proof is one of the three things that needs the float, so its
        // tail is never a candidate for the byte path.
        let (pipe, chain) = tail.build(color::OutputSpace::ProPhoto);
        (gpu_pipeline::rejoin(pipe, chain).run((*input).clone()), p)
    });
    // ⚠ The **pipeline** is always sRGB here, whatever the proof does. The display transform is a
    // post-step in `pack`, deliberately outside the pipeline so the histogram stays image-referred.
    //
    // ⚠⚠ The grade range view taps the tail at the grading stage's INPUT, not `pack`'s output:
    // the ranges are selected on Oklab L before the grade's own luminance offsets, the masks,
    // the display rendering S-curve and the creative LUT move it. `None` is the path that
    // existed before this — one tail, one run, no clone. `Some` runs the same stages in the same
    // order with one read between two halves, so the frame's pixels are identical too
    // (`the_grade_view_does_not_touch_the_photograph`).
    let (out, plane, m) =
        super::tail::run(run, from_resident, input, grade_view, float_needed, cache);
    let tail_uploads = k.tail_uploads + m.tail_uploads;
    let resident_final_hit = k.resident_final_hit || m.resident_final_hit;
    // ⚠⚠ **Harvest what the stages built, after every tail has run.** A gating brush's coverage
    // can only be rasterized where the image is, inside `MaskStage::apply`, so this is the only
    // moment the cache can see it. One point covers all three paths — the proofed tail above, the
    // grade-view split and the plain one — because they have all run by here.
    //
    // ⚠ `take_built` clears each slot, so a second harvest would find nothing: a path that
    // deposits twice cannot store a map twice, and a path that is missed loses the win silently
    // rather than caching something wrong. That is the safe direction of the two.
    for (i, m) in edits.masks.iter().enumerate() {
        for j in 0..=m.refine.len() {
            if let Some(map) = coverage.take_built(i, j) {
                brush_cache.remember(edits, cov_spec, i, j, map);
            }
        }
    }
    // ⚠ **Derived** from what was actually produced rather than tracked alongside it. Two facts
    // that can disagree is the shape `render_max_dim` and `Session::geometry_mode` were each
    // fixed for; here there is only one.
    let bytes = matches!(out, Packed::Rgba8 { .. });
    let t_pack = std::time::Instant::now();
    let mut frame = pack(out, proofed, region, max_dim, display_space, visualize);
    if debug_steps {
        eprintln!(
            "  TIMING pack {:6.1} ms",
            t_pack.elapsed().as_secs_f64() * 1000.0
        );
    }
    frame.grade_view = plane;
    // ⚠ `edits` here is already the **view** edits — `Session::view_edits` has run
    // `preview_edits`, which is what resets the crop in geometry mode — so this is the geometry
    // the render was actually asked for, which is what `fit_size` requires.
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(buffer_long, crate::view_region::PREVIEW_MAX_DIM),
        &edits.geometry,
    );
    (
        frame,
        Ran {
            prefix_stages,
            gpu_prefix_steps,
            gpu_prefix_uploads,
            handoff: handed.is_some(),
            region_scoped,
            bytes,
            tail_uploads,
            resident_final_hit,
        },
    )
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    display_space: color::OutputSpace,
    resampler: pipeline::Resampler,
    lens: Option<lens_profile::LensShot>,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    // ⚠⚠ **The same airlight definition `render_cached` uses, from the same source buffer.** This
    // function is the reference the cached path is tested against, so a second definition here
    // would make `every_cached_frame_matches_a_cold_render` fail — which is exactly how this was
    // caught, at one 8-bit level scattered across the frame.
    //
    // ⚠ `lens` is a parameter rather than `None` for that reason: "exact whenever the engine
    // happens to be lens-less" is a coincidence, not a guarantee, and this is a reference.
    let spec = pipeline::RenderSpec {
        space: color::OutputSpace::Srgb,
        // ⚠ The default, not the canvas's: this is a reference, and its one production output
        // (the decode baseline) is never drawn.
        intent: color::RenderingIntent::default(),
        resampler,
        baseline: color::wb::Illuminant::default(),
        airlight: Some(pipeline::prefix_airlight(edits, buffer, resampler, lens)),
    };
    let mut frame = pack(
        Packed::Float(
            // ⚠⚠ **`colorimetric()` on purpose, and it is now SAID rather than inherited.** This is
            // the before/after baseline and the reference `render_cached` is tested against, so it
            // must not move with a camera profile or a `.cube` — the same argument the `None` proof
            // below makes about the view. It used to get this by default, which meant nobody could
            // tell the choice from an omission; that ambiguity is what cost three defects on
            // 2026-09-18.
            pipeline::build_pipeline_spec(
                edits,
                scale,
                spec,
                pipeline::TailRendering::colorimetric(),
            )
            .run(preview),
        ),
        // ⚠ Never proofed. This is the before/after baseline and the reference the cached path is
        // tested against; a proof is a property of the *view* and would make the reference move
        // with a menu setting.
        None,
        None,
        max_dim,
        display_space,
        None,
    );
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(
            buffer.width().max(buffer.height()),
            crate::view_region::PREVIEW_MAX_DIM,
        ),
        &edits.geometry,
    );
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade_view::GradeRangeView;
    use crate::render_engine::fixtures::{
        chained_edits, chained_spec, full_prefix, hazy_buffer, hsl_edits, render_with, tiny,
        varied, varied_rect, varied_sized,
    };
    use crate::render_engine::frame::Proof;
    use crate::render_engine::RegionRect;
    use pipeline::PrefixKey;
    use pipeline::RenderScale;

    /// ⚠⚠ **That `FrameSpec.resampler` actually reaches `build_prefix_steps`.** Every other
    /// test in this file passes the same kernel on both sides of whatever it compares — a
    /// cached frame against a cold one, a region against the whole — so `render_cached`
    /// substituting a constant for `spec.resampler` leaves all of them green *and* leaves the
    /// setting inert. Requests being issued proves nothing about which kernel ran.
    #[test]
    fn the_frame_spec_resampler_reaches_the_render() {
        // ⚠ A hard edge, not flat grey: `tiny()` is uniform, and every kernel agrees exactly on
        // a uniform image. That fixture cannot see this bug.
        let mut img = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = if (x / 4) % 2 == 0 { 0.05 } else { 0.9 };
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(img);
        let edits = Edits {
            geometry: edit_model::Geometry {
                angle: 5.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let render = |k| {
            render_cached(
                &buf,
                &edits,
                FrameSpec {
                    max_dim: 32,
                    region: None,
                    display_space: color::OutputSpace::Srgb,
                    intent: color::RenderingIntent::default(),
                    resampler: k,
                    gpu_tail: false,
                    visualize: None,
                    soft_proof: None,
                    grade_view: None,
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                // ⚠ A fresh cache each time, or the second call would legitimately hit the
                // first's checkpoint and the test would be asserting about caching instead.
                pipeline::TailRendering::default(),
                &mut PrefixCache::default(),
                &mut BrushCache::default(),
            )
            .0
            .rgba
        };
        assert_ne!(
            render(pipeline::Resampler::Bilinear),
            render(pipeline::Resampler::Lanczos3),
            "the kernel in the spec never reached the pipeline"
        );
    }

    /// A deliberately non-square fixture: 43 rows do not survive a halving cleanly, so
    /// `downscale_to`'s rounding is observable between rungs. A square one could not see it.
    fn oblong() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(64, 43);
        for (i, s) in img.data_mut().iter_mut().enumerate() {
            *s = (i % 251) as f32 / 251.0;
        }
        Arc::new(img)
    }

    /// ⚠⚠ **The property the shipped code fails.** `fit_image_size` is what every overlay is
    /// placed against, and it must not depend on which rung last landed. Rendering the same photo
    /// at two rungs must report the same fit size.
    ///
    /// ⚠ Non-square on purpose. At rung 16 the 43 rows round to 11, and scaling that back up by
    /// `fit_cap / max_dim` gives 44 against the true 43 — which is the bug, one rung down, in
    /// miniature.
    #[test]
    fn the_fit_size_is_the_same_at_every_rung() {
        let buf = oblong();
        let at = |max_dim: usize| {
            render_once(
                &buf,
                &Edits::default(),
                max_dim,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .fit_size
        };
        let lo = at(16);
        let hi = at(64);
        assert!(
            (lo.0 - hi.0).abs() < 1e-3 && (lo.1 - hi.1).abs() < 1e-3,
            "the fit size was {lo:?} at one rung and {hi:?} at another — it is being derived \
             from the rendered frame's rounded dimensions"
        );
    }

    // ─────────────── M8.6: the region-scoped prefix ───────────────
    /// A straighten plus a vignette.
    ///
    /// ⚠⚠ **The vignette is what makes this fixture able to see the §5 hazard at all.** It is the
    /// one tail stage whose result depends on WHERE a pixel is, so a `Region` carrying the
    /// region's own size instead of the frame's re-centres the falloff and the bytes differ.
    /// Without it the frame dimensions could be taken off the region-scoped buffer and every byte
    /// would still match — the test would name the hazard and be unable to detect it.
    fn scoped_edits() -> Edits {
        Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    fn scoped_spec() -> FrameSpec {
        FrameSpec {
            region: Some(RegionRect {
                x: 3,
                y: 2,
                w: 9,
                h: 7,
            }),
            ..FrameSpec::fit(24)
        }
    }

    /// M9.13, end to end through the preview's render: an AI mask whose map is in the shared store
    /// darkens exactly the half the map covers — and before the map arrives it darkens nothing.
    #[test]
    fn an_ai_mask_renders_from_the_shared_map_and_not_before() {
        let mut b = ImageBuf::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                b.set_pixel(x, y, [0.18; 3]);
            }
        }
        let buf = Arc::new(b);
        let e = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: -2.0,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        };
        let mut bc = BrushCache::default();
        let (before, _) = render_with(
            &buf,
            &e,
            FrameSpec::fit(64),
            &mut PrefixCache::default(),
            &mut bc,
        );
        let px = |f: &RenderedFrame, x: usize, y: usize| f.rgba[(y * f.width + x) * 4] as i32;
        assert_eq!(
            px(&before, 5, 16),
            px(&before, 58, 16),
            "no map yet: nothing is covered"
        );

        // The subject is the LEFT half of the decode.
        let map = ai_mask::ProbMap::from_fn(32, 16, |x, _| if x < 16 { 1.0 } else { 0.0 });
        bc.ai.write().unwrap().insert(
            crate::ai_coverage::MapQuery::Prompt(edit_model::AiPrompt::Subject),
            Arc::new(map),
        );
        let (after, _) = render_with(
            &buf,
            &e,
            FrameSpec::fit(64),
            &mut PrefixCache::default(),
            &mut bc,
        );
        assert!(
            px(&after, 5, 16) + 20 < px(&after, 58, 16),
            "the covered half is not darker: {} vs {}",
            px(&after, 5, 16),
            px(&after, 58, 16)
        );
        assert_eq!(
            px(&after, 58, 16),
            px(&before, 58, 16),
            "the uncovered half changed"
        );
    }

    /// ⚠⚠ **The acceptance for the whole milestone, end to end.** The region-scoped prefix and the
    /// whole-buffer-then-crop path must produce the same frame **byte for byte** — they are the
    /// same gather in the same order, so a tolerance would hide exactly what this exists to catch.
    ///
    /// ⚠ The two paths are forced apart by the CACHE, not by a flag: a warm final checkpoint means
    /// nothing is left to run, so the gate cannot fire and the region is taken by `img.crop`.
    #[test]
    fn a_region_scoped_render_matches_the_whole_buffer_path_byte_for_byte() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(24), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache with one step left must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        assert_eq!(
            via_crop.rgba, via_region.rgba,
            "the two paths must agree byte for byte"
        );
    }

    /// ⚠⚠ The gate itself. Every pixel test passes against a gate that never fires, because the
    /// two paths are bit-identical by design — this is what says it fired.
    #[test]
    fn a_drag_of_the_last_prefix_step_renders_only_the_region() {
        let buf = varied_sized(24);
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let spec = scoped_spec();

        let (_, cold) = render_with(&buf, &scoped_edits(), spec, &mut cache, &mut bc);
        assert_eq!(cold.prefix_stages, 1, "fixture check: one prefix step");

        // A Straighten drag: the same prefix, a different angle. Exactly the last step re-runs.
        let dragged = Edits {
            geometry: edit_model::Geometry {
                angle: 3.4,
                ..Default::default()
            },
            ..scoped_edits()
        };
        let (frame, ran) = render_with(&buf, &dragged, spec, &mut cache, &mut bc);
        assert!(ran.region_scoped, "the drag must render only the region");
        assert_eq!(ran.prefix_stages, 1);
        assert_eq!((frame.width, frame.height), (9, 7));
    }

    /// ⚠⚠ A region-scoped checkpoint served for a *different* region is the one failure that shows
    /// as a plausible photograph. Not inserting it is what makes that unrepresentable — and the way
    /// to see it is that a second identical render must scope **again** rather than find a final.
    #[test]
    fn a_region_scoped_render_leaves_no_final_checkpoint() {
        let buf = varied_sized(24);
        let e = scoped_edits();
        let spec = scoped_spec();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, first) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            first.region_scoped,
            "fixture check: the gate must have fired"
        );
        let (_, second) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            second.region_scoped,
            "no final checkpoint may have been stored, so the next render scopes again"
        );
        assert_eq!(second.prefix_stages, 1);
    }

    /// ⚠⚠ **THE CARRIER FOR M8.7, and without it the milestone ships dead.** A scoped render and a
    /// whole-buffer one produce the same picture **by design**, so every correctness test in the
    /// workspace — all 63 goldens included — passes with the gate deleted. That is precisely how
    /// creative LUTs (W5) shipped fully tested and 100% non-functional.
    ///
    /// ⚠ `prefix_stages` is asserted at **2**, not merely "scoped": M8.6's gate also sets
    /// `region_scoped`, so a test asking only that would pass against the single-step version this
    /// milestone replaces — which is the version that did nothing on the reporter's own chain.
    #[test]
    fn a_chain_of_prefix_steps_renders_only_the_region() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut cache = PrefixCache::default();
        let (frame, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            ran.region_scoped,
            "a region render whose whole chain accepts one must scope"
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "the scoped chain must cover BOTH steps, not just the last"
        );
        assert_eq!((frame.width, frame.height), (24, 18));

        // ⚠ And it must decline with no region at all — the fitted rung, where scoping would be
        // meaningless and where a gate that fired anyway would be producing a crop nobody asked
        // for.
        let mut cold = PrefixCache::default();
        let (whole, ran) = render_with(&buf, &e, FrameSpec::fit(128), &mut cold, &mut bc);
        assert!(!ran.region_scoped, "no region means no scoping");
        assert_eq!((whole.width, whole.height), (128, 96));
    }

    /// ⚠⚠ The engine-level acceptance for a **chain**: the scoped path and the whole-buffer path
    /// cropped must agree. The existing byte-for-byte test covers a one-step chain; only this one
    /// can see a margin that is right for a single step and wrong once composed — which is a wrong
    /// **band at one edge of the region**, a plausible photograph rather than an error.
    ///
    /// ⚠ **Within one 8-bit level, not bit-identical, and that is a property of the chain rather
    /// than a tolerance chosen for comfort.** Noise reduction's box blur keeps a running sum, so
    /// each position is reached by a different history of adds and subtracts and the last bits
    /// differ — `a_chain_with_a_local_agrees_to_within_a_rounding` measures that at one ulp. A
    /// chain of gathers alone *is* byte-identical, and `a_chain_of_gathers_is_byte_for_byte` is
    /// where that standard is held.
    ///
    /// ⚠ The two paths are forced apart by the CACHE rather than by a flag: a warm final
    /// checkpoint leaves nothing to run, so the gate cannot fire and the region is taken by
    /// `img.crop`.
    #[test]
    fn a_chained_region_render_agrees_with_the_whole_buffer_path() {
        let buf = varied_rect(128, 96);
        let e = chained_edits();
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut warm = PrefixCache::default();
        let _ = render_with(&buf, &e, FrameSpec::fit(128), &mut warm, &mut bc);
        let (via_crop, a) = render_with(&buf, &e, spec, &mut warm, &mut bc);
        assert!(
            !a.region_scoped,
            "fixture check: a warm final checkpoint must NOT take the scoped path"
        );

        let mut cold = PrefixCache::default();
        let (via_region, b) = render_with(&buf, &e, spec, &mut cold, &mut bc);
        assert!(
            b.region_scoped,
            "fixture check: a cold cache must take the scoped path"
        );

        assert_eq!(
            (via_crop.width, via_crop.height),
            (via_region.width, via_region.height)
        );
        let worst = via_crop
            .rgba
            .iter()
            .zip(via_region.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "the two paths must agree to within a rounding, worst {worst} levels"
        );
    }

    /// ⚠⚠ **The caveat as a behaviour rather than as a sentence in a document.** A masked capture
    /// correction runs **last** in the prefix and declines a region by construction — it inherits
    /// `PipelineStage::input_rect_for`'s `None`. One decline abandons the whole plan, because a
    /// step that must run whole-buffer needs a whole-buffer input and therefore so does every step
    /// above it. So a photograph carrying a masked dehaze or a masked noise reduction gets no
    /// scoping at all, and that is recorded here so a later reader meets it as a decision.
    #[test]
    fn a_masked_capture_switches_scoping_off_for_that_photo() {
        let buf = varied_rect(128, 96);
        let spec = chained_spec();
        let mut bc = BrushCache::default();

        let mut plain = PrefixCache::default();
        let (_, before) = render_with(&buf, &chained_edits(), spec, &mut plain, &mut bc);
        assert!(
            before.region_scoped,
            "fixture check: without the mask this photo does scope"
        );

        let mut e = chained_edits();
        e.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 30.0 },
                ..Default::default()
            },
            disabled: false,
            invert: false,
        });

        let mut cache = PrefixCache::default();
        let (_, ran) = render_with(&buf, &e, spec, &mut cache, &mut bc);
        assert!(
            !ran.region_scoped,
            "a masked capture correction declines a region, which abandons the whole plan"
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "fixture check: the mask really added a prefix step"
        );
    }

    fn byte_spec() -> FrameSpec {
        FrameSpec {
            gpu_tail: true,
            ..FrameSpec::fit(8)
        }
    }

    fn rendered(spec: FrameSpec) -> (RenderedFrame, Ran) {
        render_cached(
            &varied(),
            &hsl_edits(),
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
    }

    fn dehaze_edits() -> Edits {
        Edits {
            dehaze: edit_model::Dehaze { amount: 70.0 },
            ..Edits::default()
        }
    }

    fn rendered_hazy(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &dehaze_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE CARRIER, and without it every test in `gpu-pipeline` passes against a seam
    /// `render_cached` never calls.** The prefix seam changes no pixel a test can read — the two
    /// paths render the same picture by design — so only an explicit count can see it. That
    /// asymmetry is exactly how creative LUTs (W5) shipped fully tested and 100% dead.
    #[test]
    fn a_dehaze_prefix_step_runs_on_the_gpu() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered_hazy(true, &mut PrefixCache::default());
        assert_eq!(
            ran.prefix_stages, 1,
            "fixture check: there must be exactly one prefix step to accelerate"
        );
        assert_eq!(ran.gpu_prefix_steps, 1, "dehaze must have run on the GPU");
    }

    /// ⚠⚠ M10.16d2's carrier, `a_dehaze_prefix_step_runs_on_the_gpu`'s reason: the seam changes no
    /// pixel a test can read, so only the count can see that the walk hands it the maps.
    #[test]
    fn a_lens_blur_prefix_step_runs_on_the_gpu() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let edits = edit_model::Edits {
            lens_blur: edit_model::LensBlur {
                on: true,
                amount: 100.0,
                focus: edit_model::LensFocus::Point { point: [0.5, 0.5] },
                ..Default::default()
            },
            ..Default::default()
        };
        let mut brush = BrushCache::default();
        brush.ai.write().expect("ai maps").insert(
            crate::ai_coverage::MapQuery::Depth,
            std::sync::Arc::new(ai_mask::ProbMap::from_fn(64, 48, |x, _| x as f32 / 63.0)),
        );
        let (_, ran) = render_cached(
            &hazy_buffer(),
            &edits,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut brush,
        );
        assert_eq!(
            ran.prefix_stages, 1,
            "fixture check: the lens blur step must be built"
        );
        assert_eq!(
            ran.gpu_prefix_steps, 1,
            "lens blur must have run on the GPU"
        );
    }

    fn straightened(angle: f32, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        straightened_at(angle, 200, cache)
    }

    fn straightened_at(angle: f32, rung: usize, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        let mut e = Edits::default();
        e.geometry.angle = angle;
        render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(rung)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **The resident input (2026-09-26): a Straighten drag uploads once, not per frame.**
    /// Both halves: the second frame pays NO upload, and its picture is byte-identical to the
    /// same angle rendered through a fresh cache (a resident miss), so a hit cannot be serving a
    /// stale or wrong image.
    #[test]
    fn a_straighten_drag_uploads_its_input_once() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = straightened(2.0, &mut cache);
        assert_eq!(
            first.gpu_prefix_steps, 1,
            "fixture check: geometry ran on the GPU"
        );
        assert_eq!(first.gpu_prefix_uploads, 1, "the first frame uploads");
        let (second, ran) = straightened(3.0, &mut cache);
        assert_eq!(ran.gpu_prefix_steps, 1);
        assert_eq!(
            ran.gpu_prefix_uploads, 0,
            "a later frame of the drag must not upload"
        );
        let (fresh, _) = straightened(3.0, &mut PrefixCache::default());
        assert_eq!(
            second.rgba, fresh.rgba,
            "a resident hit must render what a miss renders"
        );
        // ⚠ A different rung is a different checkpoint: it must miss, never be served the 200 px
        // image the slot holds.
        let (other, ran) = straightened_at(3.0, 120, &mut cache);
        assert_eq!(
            ran.gpu_prefix_uploads, 1,
            "a rung change must upload its own input"
        );
        let (fresh, _) = straightened_at(3.0, 120, &mut PrefixCache::default());
        assert_eq!(other.rgba, fresh.rgba);
    }

    /// A Straighten frame whose tail is one GPU chain (`hsl_edits` clears the cost gate), which is
    /// what a handoff needs, rendered with the handoff on or off.
    fn handed(angle: f32, handoff: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        let mut e = hsl_edits();
        e.geometry.angle = angle;
        render_cached_with(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
            handoff,
        )
    }

    /// ⚠⚠ **The handoff (2026-09-26): the geometry's output reaches the tail without a download**,
    /// and changes no pixel. Byte-identical to the same frame rendered without it (both run the
    /// same GPU twin and the same tail chain on the same pixels), it reports the handoff, and it
    /// leaves NO final checkpoint — which is why the worker settles such frames.
    #[test]
    fn a_handed_off_frame_renders_the_same_bytes_and_caches_no_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut with = PrefixCache::default();
        let (a, ran) = handed(3.0, true, &mut with);
        assert!(
            ran.handoff,
            "fixture check: the handoff must have been taken"
        );
        assert_eq!(ran.gpu_prefix_steps, 1);
        assert_eq!(
            with.final_len(),
            0,
            "a handed-off frame caches no final checkpoint"
        );
        let mut without = PrefixCache::default();
        let (b, ran) = handed(3.0, false, &mut without);
        assert!(!ran.handoff);
        assert_eq!(
            without.final_len(),
            1,
            "fixture check: the ordinary path caches the final"
        );
        assert_eq!(a.rgba, b.rgba, "the handoff must not change a byte");
        assert_eq!(a.hist, b.hist);
    }

    /// ⚠ A handoff frame whose tail cannot run from the resident must fall back, not fail — and
    /// the fallback is the settle's own path: download the resident, cache it as the final, render
    /// the ordinary way. ⚠ Every tail stage has a GPU twin, so no real edit set refuses the
    /// resident; the test refuses it deliberately (`REFUSE_THE_RESIDENT`), which is the run-time
    /// failure being guarded.
    #[test]
    fn a_handoff_whose_tail_cannot_run_from_the_resident_downloads_it() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let mut e = hsl_edits();
        e.geometry.angle = 3.0;
        REFUSE_THE_RESIDENT.with(|l| l.set(true));
        let (frame, ran) = render_cached_with(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut BrushCache::default(),
            true,
        );
        REFUSE_THE_RESIDENT.with(|l| l.set(false));
        assert!(ran.handoff, "fixture check: the step handed off");
        assert!(!ran.resident_final_hit, "the tail was refused the resident");
        assert_eq!(
            cache.final_len(),
            1,
            "the fallback downloads the resident and caches it as the final"
        );
        // ⚠ Against the same edits through the ordinary path (a download, a cached final, the
        // tail uploading it) — the same GPU twins on the same pixels, so byte-identical. Not
        // against a CPU tail, which differs by the twins' one-level bound.
        let (want, _) = handed(3.0, false, &mut PrefixCache::default());
        assert_eq!(
            frame.rgba, want.rgba,
            "the fallback renders the ordinary picture"
        );
    }

    /// A tail-slider frame with a fixed prefix, at a rung, with `gpu_tail` on.
    fn tail_frame(
        exposure: f32,
        angle: f32,
        rung: usize,
        cache: &mut PrefixCache,
    ) -> (RenderedFrame, Ran) {
        let mut e = hsl_edits();
        e.exposure = exposure;
        e.geometry.angle = angle;
        render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(rung)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE claim (2026-09-26): a tail-slider frame after the first uploads NOTHING.** The first
    /// frame's tail uploads the final and keeps it; the second copies it GPU-side. Both halves:
    /// the counter, and a picture byte-identical to the same edits through a fresh cache.
    #[test]
    fn a_tail_slider_frame_uploads_once_and_then_hits_the_resident_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        // ⚠ No prefix step at all: the final IS checkpoint 0, the bare buffer.
        let (_, first) = tail_frame(0.3, 0.0, 200, &mut cache);
        assert_eq!(first.prefix_stages, 0, "fixture check: a bare prefix");
        assert_eq!(first.tail_uploads, 1, "the first tail frame uploads");
        assert!(!first.resident_final_hit);
        let (second, ran) = tail_frame(0.6, 0.0, 200, &mut cache);
        assert_eq!(ran.tail_uploads, 0, "the second must upload nothing");
        assert!(ran.resident_final_hit);
        let (fresh, _) = tail_frame(0.6, 0.0, 200, &mut PrefixCache::default());
        assert_eq!(second.rgba, fresh.rgba, "a hit renders what a miss renders");
        assert_eq!(second.hist, fresh.hist);
        // ⚠ A rung change is another checkpoint: it misses and uploads its own input.
        let (other, ran) = tail_frame(0.6, 0.0, 120, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a rung change uploads");
        let (fresh, _) = tail_frame(0.6, 0.0, 120, &mut PrefixCache::default());
        assert_eq!(other.rgba, fresh.rgba);
    }

    /// ⚠⚠ **A prefix edit between two tail frames re-fills the slot from the step's own output**,
    /// so the tail still uploads nothing — and the picture follows the edit. The sabotage that
    /// ignores the key serves the old geometry to the new tail and fails the byte comparison.
    #[test]
    fn a_prefix_edit_between_two_tail_frames_refills_the_resident_from_the_step() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, a) = tail_frame(0.3, 2.0, 200, &mut cache);
        assert_eq!(
            a.gpu_prefix_steps, 1,
            "fixture check: geometry ran on the GPU"
        );
        let (b_frame, b) = tail_frame(0.6, 2.0, 200, &mut cache);
        assert!(
            b.resident_final_hit,
            "fixture check: the tail hits after the step"
        );
        assert_eq!(b.tail_uploads, 0);
        let (c_frame, c) = tail_frame(0.6, 3.0, 200, &mut cache);
        assert_eq!(c.gpu_prefix_steps, 1, "the geometry re-ran");
        assert_eq!(
            c.tail_uploads, 0,
            "and its output fed the tail without an upload"
        );
        assert_ne!(
            c_frame.rgba, b_frame.rgba,
            "fixture check: the two angles render different pictures"
        );
        let (fresh, _) = tail_frame(0.6, 3.0, 200, &mut PrefixCache::default());
        assert_eq!(c_frame.rgba, fresh.rgba, "the picture follows the edit");
    }

    /// At 1:1 the tail's input is a crop, and the key carries the rect: two frames at one rect
    /// upload once; a pan uploads again, and renders the panned picture.
    #[test]
    fn at_one_to_one_the_resident_final_is_keyed_on_the_rect() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let at = |exposure: f32, rect: RegionRect, cache: &mut PrefixCache| {
            let mut e = hsl_edits();
            e.exposure = exposure;
            render_cached(
                &hazy_buffer(),
                &e,
                FrameSpec {
                    gpu_tail: true,
                    region: Some(rect),
                    ..FrameSpec::fit(200)
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                pipeline::TailRendering::default(),
                cache,
                &mut BrushCache::default(),
            )
        };
        let a = RegionRect {
            x: 10,
            y: 10,
            w: 60,
            h: 40,
        };
        let b = RegionRect {
            x: 70,
            y: 50,
            w: 60,
            h: 40,
        };
        let mut cache = PrefixCache::default();
        let (_, first) = at(0.3, a, &mut cache);
        assert_eq!(first.tail_uploads, 1);
        let (_, second) = at(0.6, a, &mut cache);
        assert_eq!(second.tail_uploads, 0, "the same rect hits");
        assert!(second.resident_final_hit);
        let (panned, ran) = at(0.6, b, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a pan is another crop and must upload");
        let (fresh, _) = at(0.6, b, &mut PrefixCache::default());
        assert_eq!(panned.rgba, fresh.rgba, "and renders the panned picture");
    }

    /// A handoff frame's resident, downloaded, IS the final checkpoint the ordinary path caches.
    #[test]
    fn a_handed_off_residents_download_equals_the_cached_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let ctx = gpu_pipeline::shared().expect("checked");
        let mut with = PrefixCache::default();
        let (_, ran) = handed(3.0, true, &mut with);
        assert!(ran.handoff, "fixture check");
        assert_eq!(
            with.final_len(),
            0,
            "fixture check: no CPU final after a handoff"
        );
        assert!(with.settle_from_resident(ctx), "the settle downloads it");
        let mut without = PrefixCache::default();
        let (_, ran) = handed(3.0, false, &mut without);
        assert!(!ran.handoff);
        let a = with.finals[0].1.clone();
        let b = without.finals[0].1.clone();
        assert_eq!(with.finals[0].0, without.finals[0].0, "the same key");
        assert_eq!(a.data(), b.data(), "the same checkpoint, sample for sample");
    }

    /// ⚠ The other direction, and it is not symmetry: without it a seam that fired
    /// **unconditionally** — ignoring the setting entirely — would pass the carrier above.
    #[test]
    fn with_the_setting_off_no_prefix_step_runs_on_the_gpu() {
        let (_, ran) = rendered_hazy(false, &mut PrefixCache::default());
        assert_eq!(ran.prefix_stages, 1, "fixture check: one prefix step");
        assert_eq!(ran.gpu_prefix_steps, 0);
    }

    /// ⚠⚠ **A checkpoint written by either path is a legitimate hit for the other**, which is what
    /// `PrefixStepKey` being derived from the **edits** buys — and it is a claim until something
    /// asserts it. Render once on the GPU, then once on the CPU **through the same cache**: the
    /// second must run no prefix step at all.
    #[test]
    fn a_checkpoint_written_by_either_path_is_a_hit_for_the_other() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_hazy(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_hazy(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    fn denoised_edits() -> Edits {
        Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        }
    }

    fn rendered_denoised(gpu: bool, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        render_cached(
            &hazy_buffer(),
            &denoised_edits(),
            FrameSpec {
                gpu_tail: gpu,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **The same property for the SECOND stage across the seam, and it is a claim again.**
    /// `PrefixStepKey` is derived from the edits, never from the stage — so a checkpoint written
    /// by one path must be a legitimate hit for the other. The dangerous direction is a stale
    /// hit, which renders the previous frame's pixels: a plausible photograph, not an error.
    #[test]
    fn a_noise_reduction_checkpoint_is_shared_between_the_paths() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_denoised(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_denoised(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }

    /// ⚠⚠ **Without this, the three "keeps the float" tests below all pass against an
    /// implementation that never takes the byte path at all** — which is the failure mode that let
    /// creative LUTs ship fully tested and 100% dead.
    #[test]
    fn with_none_of_the_float_consumers_active_the_bytes_are_used() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (_, ran) = rendered(byte_spec());
        assert!(ran.bytes, "the tail's last chain must download RGBA8");
    }

    /// ⚠ `pack` converts to a non-sRGB display space from the **float**, because an 8-bit → 8-bit
    /// conversion costs roughly 0.8 bits and bands a smooth sky.
    #[test]
    fn a_non_srgb_display_space_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            display_space: color::OutputSpace::DisplayP3,
            intent: color::RenderingIntent::default(),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The edge view is computed from the float.
    #[test]
    fn visualize_spots_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            visualize: Some(12.0),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠ The ProPhoto run is decoded back to the working space to test the gamut, which an 8-bit
    /// round trip cannot support.
    #[test]
    fn a_soft_proof_keeps_the_float_path() {
        if gpu_pipeline::shared().is_none() {
            return;
        }
        let (_, ran) = rendered(FrameSpec {
            soft_proof: Some(Proof {
                space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                warn: false,
            }),
            ..byte_spec()
        });
        assert!(!ran.bytes);
    }

    /// ⚠⚠ **THE CARRIER.** Every test above can pass while the bytes never reach the texture. The
    /// two paths are designed to be indistinguishable in the output, so this asserts exactly that
    /// — and it is what would fail if the shader's quantization and `ImageBuf::quantize` ever
    /// diverged on a real frame rather than on the synthetic fixture in `gpu-pipeline`.
    #[test]
    fn the_byte_path_and_the_float_path_produce_the_same_frame() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let (with_bytes, ran) = rendered(byte_spec());
        assert!(ran.bytes, "fixture check: this must be the byte path");
        let (with_float, ran) = rendered(FrameSpec {
            gpu_tail: false,
            ..byte_spec()
        });
        assert!(!ran.bytes, "fixture check: this must be the float path");
        assert_eq!(
            (with_bytes.width, with_bytes.height),
            (with_float.width, with_float.height)
        );
        // ⚠ The GPU's transcendentals diverge from the CPU's by well under an 8-bit level, so the
        // comparison is in levels rather than bit-exact — the bound `gpu-pipeline`'s own
        // equivalence tests establish. What it catches is a *wrong* quantization, which moves
        // whole levels.
        let worst = with_bytes
            .rgba
            .iter()
            .zip(with_float.rgba.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "texture differs by {worst} levels");
        assert!(
            Arc::ptr_eq(&with_bytes.rgba, &with_bytes.srgb),
            "on the byte path the texture and the photograph are one allocation"
        );
    }

    #[test]
    fn a_change_reruns_only_what_is_downstream_of_it() {
        // The feature itself. `every_cached_frame_matches_a_cold_render` proves the output is
        // right but would still pass with the cache deleted; this proves work is actually
        // being skipped, which is the entire point of the change.
        let buf = varied();
        let base = full_prefix();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        let (_, cold) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            cold.prefix_stages, 3,
            "a cold render runs every prefix stage"
        );

        let (_, hit) = render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(hit.prefix_stages, 0, "an unchanged prefix runs nothing");

        // A tail-only change still runs nothing: it resumes at the final checkpoint.
        let tail_only = Edits {
            exposure: -1.5,
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &tail_only,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "a tail change must not touch the prefix"
        );

        // ⚠ Since M7.3b the order is NR → dehaze → geometry. A dehaze change resumes after NR and
        // re-runs dehaze and the geometry resample behind it.
        let dehaze = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &dehaze,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 2,
            "a dehaze change must resume after noise reduction"
        );

        // Noise reduction is first, so everything re-runs — but the downscale is still reused.
        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 15.0,
                color: 15.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &nr,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 3,
            "noise reduction is the first prefix step since M7.3b"
        );

        // ⚠⚠ Geometry is LAST since M7.3b, so a straighten re-runs only the resample. This is the
        // point of the reorder: before it, this was 3 — NR and dehaze on every frame of a drag.
        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(
            &buf,
            &geo,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 1,
            "a geometry change must resume after dehaze"
        );
    }

    #[test]
    fn the_native_seed_shares_the_buffer_rather_than_copying_it() {
        // When nothing is downscaled, `downscale_to` is a clone of a buffer the engine already
        // holds behind an `Arc`. Caching that would cost 78 MB on an X-T4 to save 2.2 ms, so
        // the chain seeds from the Arc instead and stores no checkpoint 0.
        let buf = varied(); // 16 x 16
        let base = full_prefix();
        let mut bc = BrushCache::default();

        let mut native = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut native,
            &mut bc,
        );
        assert_eq!(
            native.mid_len(),
            2,
            "native should store only the post-geometry and post-NR checkpoints"
        );

        let mut fit = PrefixCache::default();
        render_cached(
            &buf,
            &base,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut fit,
            &mut bc,
        );
        assert_eq!(
            fit.mid_len(),
            3,
            "fit should additionally store the downscale"
        );
    }

    #[test]
    fn a_shrinking_prefix_does_not_serve_the_wrong_checkpoint() {
        // Turning the geometry off makes the post-dehaze checkpoint the *final* one. Its key is
        // the same value it had as a mid, so the tiers must not end up holding it twice with the
        // two copies free to diverge. ⚠ The geometry rather than dehaze since M7.3b, because the
        // geometry is now the prefix's last step.
        let buf = varied();
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let with = full_prefix();
        render_cached(
            &buf,
            &with,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );

        let without = Edits {
            geometry: edit_model::Geometry::default(),
            ..with.clone()
        };
        let (got, ran) = render_cached(
            &buf,
            &without,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(
            ran.prefix_stages, 0,
            "the post-dehaze checkpoint is now the final one"
        );
        assert_eq!(
            got.rgba,
            render_once(
                &buf,
                &without,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
            .rgba
        );

        // ...and it must have been *promoted* into the protected tier, not left in the
        // budgeted one where a big render could evict the thing zooming back relies on.
        assert_eq!(cache.final_len(), 2);
        assert_eq!(cache.mid_len(), 2);
    }

    /// ⚠⚠ **The symptom it was found as**: at a native rung every frame of a prefix drag restarted
    /// from zero, re-running lens correction each time — measured in the running app at
    /// **1730–2223 ms a frame** with `prefix_ran 5` on every one.
    #[test]
    fn a_prefix_drag_resumes_when_the_budget_holds_one_checkpoint() {
        let buf = varied_sized(16);
        // 16*16*3*4 = 3072 bytes a checkpoint, so 4000 holds exactly one — 298 MiB against a
        // 512 MB budget, in miniature.
        let mut cache = PrefixCache::with_mid_budget(4000);
        let mut bc = BrushCache::default();
        let base = full_prefix();
        let (_, cold) = render_with(&buf, &base, FrameSpec::fit(16), &mut cache, &mut bc);
        assert_eq!(
            cold.prefix_stages, 3,
            "fixture check: a cold render runs the whole prefix"
        );

        // ⚠ Drag DEHAZE rather than noise reduction: NR is the FIRST prefix step since M7.3b, so
        // moving it invalidates every checkpoint and there is nothing to resume from whatever the
        // policy. The bug is about a drag that HAS valid upstream work.
        let dragged = Edits {
            dehaze: edit_model::Dehaze { amount: 41.0 },
            ..base.clone()
        };
        let (_, ran) = render_with(&buf, &dragged, FrameSpec::fit(16), &mut cache, &mut bc);
        assert!(
            ran.prefix_stages < 3,
            "the drag must resume at the checkpoint before dehaze rather than restart, ran {}",
            ran.prefix_stages
        );
    }

    /// The RGBA sub-rect of a frame, for comparing a region render against a whole one.
    /// ⚠⚠ **The picture half of the eviction fix (M10.14b, owed since 2026-09-14).** The test above
    /// proves a drag *resumes* when the budget holds one checkpoint — it counts stages, not pixels. A
    /// resume is only an optimisation: its frame must be EXACTLY a cold render of the same edits. A
    /// checkpoint served past the step that moved would show the old dehaze with that test green.
    ///
    /// ⚠ On `hazy_buffer` at its own size, not the 16-pixel buffer: dehaze barely acts there, and the
    /// fixture check below failed on it — equality would have proved nothing.
    #[test]
    fn a_resumed_drag_frame_is_exactly_a_cold_render() {
        let buf = hazy_buffer();
        let spec = FrameSpec::fit(200);
        // 200*160*3*4 = 384,000 bytes a checkpoint, so 400,000 holds exactly one — the native rung's
        // situation, as in the test above.
        let mut cache = PrefixCache::with_mid_budget(400_000);
        let mut bc = BrushCache::default();
        let base = full_prefix();
        let (first, cold) = render_with(&buf, &base, spec, &mut cache, &mut bc);
        assert_eq!(
            cold.prefix_stages, 3,
            "fixture check: a cold render runs the whole prefix"
        );

        let dragged = Edits {
            dehaze: edit_model::Dehaze { amount: 80.0 },
            ..base.clone()
        };
        let (resumed, ran) = render_with(&buf, &dragged, spec, &mut cache, &mut bc);
        assert!(
            ran.prefix_stages < 3,
            "fixture check: the drag resumed rather than restarted, ran {}",
            ran.prefix_stages
        );

        let (fresh, _) = render_with(
            &buf,
            &dragged,
            spec,
            &mut PrefixCache::with_mid_budget(400_000),
            &mut BrushCache::default(),
        );
        assert_ne!(
            first.rgba, fresh.rgba,
            "fixture check: the drag changes the picture, or the equality below proves nothing"
        );
        assert!(
            resumed.rgba == fresh.rgba,
            "the resumed drag frame differs from a cold render of the same edits"
        );
    }

    fn crop_rgba(f: &RenderedFrame, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * f.width + x) * 4;
            out.extend_from_slice(&f.rgba[start..start + w * 4]);
        }
        out
    }

    #[test]
    fn a_region_render_matches_the_whole_render_cropped() {
        // The master invariant. It validates the region maths, the vignette correction and
        // the cache at once — if any of the three is wrong, this fails.
        //
        // ⚠ Sharpening and local contrast *weaken* it, and that has to be said out loud
        // rather than left to an edit set that happens to omit them. A spatial stage in the
        // tail reads neighbours the cropped region genuinely does not have, so equality can
        // only hold in the interior. The excluded band is `tail_kernel_radius` wide —
        // precisely what `view_region::REGION_MARGIN` keeps off screen — and both the inset
        // and the buffer size are derived from it, so they track any future change to a
        // radius or a sigma.
        let edits = Edits {
            exposure: 0.4,
            vignette: edit_model::Vignette {
                amount: -80.0,
                roundness: 50.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                color: 40.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 70.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            // ⚠ A mask joins the master invariant because that test validates the region
            // maths, the vignette correction *and* the prefix cache at once — so this is what
            // proves masks survive the cached render path, not just a bare stage call.
            masks: vec![
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                        from: [0.1, 0.1],
                        to: [0.9, 0.8],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.6,
                        // ⚠⚠ **Masked spatial belongs in this edit set, and its absence would be
                        // silent.** This test has already been insufficient in exactly that way
                        // once: when sharpening landed it kept passing because its edits carried
                        // none, so the weakened invariant it exists to cover went unexercised. A
                        // masked clarity reaches 120 px on its own, *on top of* the global reach,
                        // because masks chain — so if `tail_kernel_radius` ever stopped counting
                        // them the inset below would be too small and this comparison would fail
                        // at the edges, which is precisely the bug it should catch.
                        local_contrast: edit_model::LocalContrast {
                            texture: 0.0,
                            clarity: 70.0,
                        },
                        // ⚠⚠ **The capture corrections belong in this edit set, and their absence
                        // would be silent.** They run in the *prefix*, whole-buffer, and that is the
                        // entire reason for their placement: dehaze estimates its airlight from the
                        // brightest 0.1% of the buffer it is handed, so if it ever ran on the
                        // visible region instead, the same pixel would render differently depending
                        // on where the viewport happened to be — and this comparison is what catches
                        // it. There is no other test that can.
                        //
                        // ⚠ They contribute **nothing** to `inset` below, because neither is in the
                        // tail. If this ever fails at the edges, the placement is wrong — do not
                        // widen the inset.
                        noise_reduction: edit_model::NoiseReduction {
                            luminance: 40.0,
                            color: 30.0,
                            ..Default::default()
                        },
                        dehaze: edit_model::Dehaze { amount: 45.0 },
                        ..Default::default()
                    },
                    disabled: false,
                    invert: false,
                },
                // ⚠⚠ **AN AUTO-MASKED BRUSH BELONGS IN THIS EDIT SET, AND THIS IS THE GUARD ON THE
                // WHOLE AUTO-MASK DESIGN.** Its coverage depends on PIXEL VALUES as well as position,
                // which is what made a render-time reference untenable: a dab whose centre has been
                // panned off-region would have no reference to read, so its coverage — and therefore
                // the photograph — would depend on where the viewport happens to be. That is
                // `dehaze::airlight`'s failure arriving in a new place, and dehaze's escape (move to
                // the prefix) is not available, because brush coverage is resolved in the tail.
                //
                // A reference **baked into the stroke** travels in `Edits`, so it is available under
                // any region, in the CLI and in export alike. This comparison is what asserts that.
                // ⚠ Sabotage-verified: reading the reference from the base at the dab's own centre
                // fails this test.
                //
                // ⚠ It contributes **nothing** to `inset` below — a brush reads no neighbours, so it
                // adds no kernel reach. If this ever fails at the edges, something about the
                // reference's availability has changed; do not widen the inset.
                edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Brush(edit_model::Brush {
                        strokes: vec![edit_model::BrushStroke {
                            decode: false,
                            points: vec![[0.2, 0.3], [0.8, 0.7]],
                            radius: 0.2,
                            hardness: 60.0,
                            flow: 100.0,
                            erase: false,
                            auto_mask: true,
                            tolerance: 45.0,
                        }],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                    disabled: false,
                    invert: false,
                },
            ],
            ..Edits::default()
        };

        let inset = pipeline::tail_kernel_radius(&edits, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 2 * inset + 40, 2 * inset + 36);
        let n = 2 * inset + 80;
        let buf = varied_sized(n);

        // max_dim = n is native for this n x n buffer, i.e. no downscale, so the render
        // scale is exactly 1.0 and the inset above is the full-resolution kernel.
        let (whole, _) = render_cached(
            &buf,
            &edits,
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        // Guard against the test passing trivially: the spatial tail stages must actually
        // move pixels here, or the "weakened invariant" it exists to cover is not exercised.
        let (unsharpened, _) = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                local_contrast: edit_model::LocalContrast::default(),
                ..edits.clone()
            },
            FrameSpec::fit(n),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_ne!(
            whole.rgba, unsharpened.rgba,
            "the spatial tail stages changed nothing"
        );

        assert!(
            x + w <= whole.width && y + h <= whole.height,
            "region must fit"
        );
        assert!(
            w > 2 * inset && h > 2 * inset,
            "region must be wider than twice the kernel ({inset})"
        );

        let (got, _) = render_cached(
            &buf,
            &edits,
            FrameSpec {
                max_dim: n,
                region: Some(RegionRect { x, y, w, h }),
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        let (iw, ih) = (w - 2 * inset, h - 2 * inset);
        assert_eq!(
            crop_rgba(&got, inset, inset, iw, ih),
            crop_rgba(&whole, x + inset, y + inset, iw, ih),
            "the region's interior must match the whole render bit for bit"
        );
        assert_eq!(
            got.region,
            Some(pipeline::Region {
                full_w: whole.width,
                full_h: whole.height,
                x0: x,
                y0: y
            })
        );
    }

    #[test]
    fn the_cache_keeps_one_entry_per_render_mode() {
        // Fit and native are different max_dims. With a single entry, every zoom in or out
        // would recompute the prefix (~123 ms with NR on).
        let buf = varied();
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();

        render_cached(
            &buf,
            &e,
            FrameSpec::fit(8),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        ); // "fit"
        render_cached(
            &buf,
            &e,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        ); // "native"
        assert_eq!(cache.final_len(), 2, "both modes should stay cached");

        for max_dim in [8usize, 16] {
            let scale = render_scale(16, rendered_long_edge(16, max_dim), 1.0);
            assert!(
                cache
                    .get(&(
                        PrefixKey::new(&e, scale, pipeline::Resampler::Bilinear),
                        max_dim
                    ))
                    .is_some(),
                "max_dim {max_dim} was evicted"
            );
        }

        // A third distinct key evicts the least recently used.
        let e2 = Edits {
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..e.clone()
        };
        render_cached(
            &buf,
            &e2,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert_eq!(cache.final_len(), 2);
    }

    #[test]
    fn a_region_bigger_than_the_image_is_clamped() {
        let buf = varied();
        let e = Edits::default();
        let (got, _) = render_cached(
            &buf,
            &e,
            FrameSpec {
                max_dim: 16,
                region: Some(RegionRect {
                    x: 10,
                    y: 10,
                    w: 999,
                    h: 999,
                }),
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!((got.width, got.height), (6, 6));
    }

    #[test]
    fn render_once_matches_expected_gray() {
        let img = tiny();
        let f = render_once(
            &img,
            &Edits::default(),
            4,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        assert_eq!((f.width, f.height), (4, 4));
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
    }

    // ---- the grade range view ------------------------------------------------------------
    /// Six greys spanning black to above white, a saturated blue and a red — 8×1.
    fn ramp_fixture() -> Arc<ImageBuf> {
        let px = [
            [0.005f32, 0.005, 0.005],
            [0.05, 0.05, 0.05],
            [0.18, 0.18, 0.18],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [1.4, 1.4, 1.4],
            [0.01, 0.025, 0.45],
            [0.5, 0.1, 0.1],
        ];
        let mut buf = ImageBuf::new(px.len(), 1);
        for (i, p) in px.iter().enumerate() {
            buf.set_pixel(i, 0, *p);
        }
        Arc::new(buf)
    }

    /// A grade that is active (saturation on all three wheels), with `luminance` on every wheel
    /// set to `lum` — the offset that moves L *after* the tap.
    fn graded(lum: f32, rendering: Option<edit_model::Rendering>) -> Edits {
        let wheel = |hue: f32| edit_model::GradeWheel {
            hue,
            saturation: 70.0,
            luminance: lum,
        };
        Edits {
            color_grading: edit_model::ColorGrading {
                shadows: wheel(240.0),
                midtones: wheel(120.0),
                highlights: wheel(45.0),
                ..edit_model::ColorGrading::default()
            },
            rendering,
            ..Default::default()
        }
    }

    /// Render through the cached path at the fixture's own resolution, with the view.
    fn viewed(
        buf: &Arc<ImageBuf>,
        edits: &Edits,
        grade_view: Option<GradeRangeView>,
        region: Option<RegionRect>,
    ) -> RenderedFrame {
        render_cached(
            buf,
            edits,
            FrameSpec {
                max_dim: buf.width().max(buf.height()),
                region,
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize: None,
                soft_proof: None,
                grade_view,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    fn plane(f: &RenderedFrame) -> Vec<u8> {
        f.grade_view
            .as_ref()
            .map(|p| p.as_ref().clone())
            .expect("the frame carries no plane")
    }

    /// ⚠⚠ The view is an overlay, never a change to the photograph: pixels, sRGB channel and
    /// histogram are byte-identical with the view on and off. Sabotage: painting the tint into
    /// `out` fails it. And `None` is the old path — no plane at all.
    #[test]
    fn the_grade_view_does_not_touch_the_photograph() {
        let buf = ramp_fixture();
        let edits = graded(0.0, None);
        let off = viewed(&buf, &edits, None, None);
        let on = viewed(&buf, &edits, Some(GradeRangeView::Shadows), None);
        assert!(
            off.grade_view.is_none(),
            "the view-off frame carries a plane"
        );
        assert_eq!(on.rgba, off.rgba, "the view changed the texture");
        assert_eq!(on.srgb, off.srgb, "the view changed the instrument channel");
        assert_eq!(on.hist, off.hist, "the view changed the histogram");
        let p = plane(&on);
        assert_eq!(p.len(), on.width * on.height);
        // The ramp's ends: the darkest grey is mostly shadow, the brightest barely — at the
        // fixture's default Blending of 50 the ranges overlap widely, so "mostly" is the honest
        // reading (143 of 255 at L = 0.17), not the near-255 a Blending of 0 would give.
        assert!(
            p[0] > 128 && p[5] < 16,
            "the plane is not the shadow weight: {p:?}"
        );
    }

    /// ⚠⚠ The plane is taken BEFORE the grade. A luminance wheel at +100 shifts L by 0.15 on
    /// every pixel it weights; tapped after the stage, the plane would move. Sabotage: run
    /// `from_grade` before taking the plane.
    #[test]
    fn the_plane_is_taken_before_the_grade() {
        let buf = ramp_fixture();
        for range in GradeRangeView::ALL {
            let flat = plane(&viewed(&buf, &graded(0.0, None), Some(range), None));
            let lifted = plane(&viewed(&buf, &graded(100.0, None), Some(range), None));
            assert_eq!(
                flat, lifted,
                "{range:?}: the grade's own luminance moved the plane"
            );
        }
        // The premise: the lift really changes the photograph, so the equality above is not
        // vacuous.
        let a = viewed(&buf, &graded(0.0, None), None, None);
        let b = viewed(&buf, &graded(100.0, None), None, None);
        assert_ne!(a.srgb, b.srgb, "premise: +100 luminance changed nothing");
    }

    /// ⚠⚠ The plane is taken BEFORE the display rendering — the test that encodes spec §1 and
    /// would have failed the pack-side design outright. Sabotage: tap after
    /// `DisplayRenderingStage` (i.e. from `pack`'s output).
    #[test]
    fn the_plane_is_taken_before_the_display_rendering() {
        let buf = ramp_fixture();
        let colorimetric = graded(0.0, None);
        let rendered = graded(0.0, Some(edit_model::Rendering::Camera));
        for range in GradeRangeView::ALL {
            let a = plane(&viewed(&buf, &colorimetric, Some(range), None));
            let b = plane(&viewed(&buf, &rendered, Some(range), None));
            assert_eq!(a, b, "{range:?}: the S-curve moved the plane");
        }
        let a = viewed(&buf, &colorimetric, None, None);
        let b = viewed(&buf, &rendered, None, None);
        assert_ne!(a.srgb, b.srgb, "premise: the rendering changed nothing");
    }

    /// On a region render the plane covers the rendered slice — `w * h` of the frame, not of
    /// the whole image. `ColorImage::from_rgba_unmultiplied` panics on a mismatch rather than
    /// reporting one, so this is pinned here.
    #[test]
    fn the_plane_has_the_frames_dimensions() {
        let mut buf = ImageBuf::new(8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let v = (x + y * 8) as f32 / 48.0;
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(buf);
        let region = Some(RegionRect {
            x: 2,
            y: 1,
            w: 4,
            h: 3,
        });
        let f = viewed(
            &buf,
            &graded(0.0, None),
            Some(GradeRangeView::Midtones),
            region,
        );
        assert_eq!((f.width, f.height), (4, 3));
        assert_eq!(plane(&f).len(), 12);
    }
}

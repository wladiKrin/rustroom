//! A masked group of per-pixel adjustments.
//!
//! Runs ordinary stages on a copy of the image and blends the result back by the mask's
//! coverage. ⚠ **Every existing stage is reused completely unchanged** — nothing else in the
//! pipeline learns that masking exists, which is what keeps the `PipelineStage` seam opaque
//! for the eventual GPU port.

use crate::{adjustment_stages, PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::{Geometry, MaskedAdjustment};

use super::mask_coverage::{BaseFacts, Composite, CoverageSource, PixelCtx};

pub struct MaskStage {
    // ⚠ The `MaskedAdjustment` is still not retained whole: the coverage source and the sub-stage
    // list are extracted at construction, and only the `LocalAdjustments` the sub-stages were
    // built from is kept beside them — see `adjustments` below, and why.
    /// The mask's coverage: its base shape plus every refinement, resolved once at construction.
    composite: Composite,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
    /// Where to report coverage this stage had to build, and at which position.
    ///
    /// ⚠⚠ **Reported, never looked up.** This stage holds no cache and consults none: what
    /// identifies a base lives beside `PrefixKey`, in `render_engine`. A gating brush can only be
    /// rasterized where the image is, which is here — so without this the app could never keep the
    /// map it wants to cache. See the coverage-cache spec's §4.0.
    ///
    /// ⚠ `None` for every caller that hands a prebuilt map or does not cache — the CLI, the export
    /// path and all 60 goldens. Nothing about the rendered pixels depends on it.
    ///
    /// ⚠ An `Arc`, not a borrow: the stage outlives the builder's scope, so a lifetime here would
    /// infect the whole stage list.
    deposit: Option<(std::sync::Arc<crate::BrushCoverage>, usize)>,
    /// The adjustments [`Self::stages`] was built from, kept so
    /// [`PipelineStage::accelerate_inner`] can hand them to an accelerator.
    ///
    /// ⚠⚠ **This field was deliberately dropped once and is deliberately back.** The old comment
    /// said everything needed was extracted at construction — true until a GPU builder needed the
    /// *parameters* a CPU stage does not expose. A `Box<dyn PipelineStage>` reports only `name`
    /// and `apply`, so the only alternative was downcasting every stage in the workspace.
    adjustments: edit_model::LocalAdjustments,
    /// A GPU coverage rasteriser, when the app has offered one.
    ///
    /// ⚠⚠ **`None` for the CLI, every export and all 63 goldens** — none of which has a GPU
    /// context — so those paths are byte-for-byte unchanged, which is exactly what keeps the
    /// goldens a valid guard on this milestone.
    coverage_gpu: Option<std::sync::Arc<dyn crate::CoverageRasterizer>>,
    /// The rung these sub-stages were built for.
    ///
    /// ⚠ `GpuSharpenStage`'s sigma is in **full-resolution** pixels and only the scale says what
    /// that is here, so an accelerator without it cannot build the sharpen row at all.
    scale: RenderScale,
}

impl MaskStage {
    /// For a render covering the whole frame, against the default white-balance baseline.
    ///
    /// ⚠ D65 is the right baseline for a buffer with no camera behind it — a synthetic chart, a
    /// JPEG — which is every caller of this convenience. Production goes through
    /// [`Self::new_in_region`] and passes the source's own.
    pub fn new(masked: &MaskedAdjustment, scale: RenderScale, geometry: &Geometry) -> Self {
        Self::new_in_region(
            masked,
            scale,
            None,
            geometry,
            &[],
            color::wb::Illuminant::default(),
            // ⚠ No deposit: this convenience is for synthetic buffers and tests, which cache
            // nothing. Production goes through `new_in_region`.
            None,
        )
    }

    /// For a render covering only part of the frame — coverage is still measured against the
    /// full frame, so a cropped region is masked exactly as that part of the whole would be.
    ///
    /// ⚠ `geometry` is needed only by a brush, whose strokes are in **pre-geometry source**
    /// coordinates and must be mapped forward into frame pixels. The analytic shapes ignore it.
    pub fn new_in_region(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        region: Option<Region>,
        geometry: &Geometry,
        prebuilt: &[Option<std::sync::Arc<crate::brush::CoverageMap>>],
        baseline: color::wb::Illuminant,
        deposit: Option<(std::sync::Arc<crate::BrushCoverage>, usize)>,
    ) -> Self {
        Self {
            composite: Self::composite(masked, geometry, prebuilt),
            stages: adjustment_stages(&masked.adjustments, baseline, scale),
            region,
            deposit,
            adjustments: masked.adjustments.clone(),
            scale,
            coverage_gpu: None,
        }
    }

    /// For the **prefix** half of a mask: its corrections of the capture.
    ///
    /// ⚠⚠ **There is no `region` parameter, and that is the point rather than an omission.** The
    /// prefix always runs whole-buffer, which is what makes dehaze's frame-global airlight
    /// correct *and* stable as the user pans. A `Region` here would reintroduce precisely the bug
    /// the placement exists to avoid — the same pixel rendering differently depending on where
    /// the viewport was.
    ///
    /// ⚠ Identical to [`Self::new_in_region`] but for the sub-stage builder, and both go through
    /// one [`Self::coverage_source`], so a new `Mask` variant stays a compile error in one place.
    pub fn new_capture(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        geometry: &Geometry,
        prebuilt: &[Option<std::sync::Arc<crate::brush::CoverageMap>>],
    ) -> Self {
        Self {
            composite: Self::composite(masked, geometry, prebuilt),
            stages: crate::capture_stages(&masked.adjustments, scale),
            region: None,
            // ⚠ The capture half runs in the **prefix**, which always renders whole-buffer and is
            // cached by `PrefixCache` at a coarser grain. There is nothing for a coverage cache to
            // add here, and a deposit would report a whole-frame map under a region's key.
            deposit: None,
            // ⚠⚠ **Filled here too, and the capture half is not an oversight.** `accelerate` is
            // only ever called on the *tail*, so this stage's hook is never invoked — and if it
            // ever were, `capture_stages` builds noise reduction and dehaze, neither of which is
            // in `REPLACEABLE`, so the list would come back unchanged. Safe by construction in
            // both directions, which beats safe by one caller remembering.
            adjustments: masked.adjustments.clone(),
            scale,
            coverage_gpu: None,
        }
    }

    /// How this mask's coverage is obtained, shared by both constructors.
    ///
    /// ⚠⚠ **Exhaustive on purpose — a catch-all here is a trap.** It used to read
    /// `other => Analytic(other.clone())`, so a new variant whose `analytic_coverage` returns
    /// `None` would fall through to `unwrap_or(0.0)` and produce a mask covering **nothing**,
    /// silently. That is precisely the failure `analytic_coverage`'s `Option` exists to make
    /// impossible, defeated one layer up. Adding a variant is a compile error here.
    /// The whole composite: the base shape, then each refinement with the op that folds it on.
    ///
    /// ⚠ `prebuilt` is indexed **per component**, 0 being the base, so a refinement's brush is
    /// served from the cache exactly as the base's is. A short or empty slice simply means
    /// nothing was prebuilt, which fails toward rasterizing here — `PrefixKey`'s rule, and the
    /// safe direction of the two.
    fn composite(
        masked: &MaskedAdjustment,
        geometry: &Geometry,
        prebuilt: &[Option<std::sync::Arc<crate::brush::CoverageMap>>],
    ) -> Composite {
        let at = |j: usize| prebuilt.get(j).cloned().flatten();
        Composite {
            base: Self::source_for(&masked.mask, geometry, at(0)),
            refine: masked
                .refine
                .iter()
                .enumerate()
                .map(|(k, r)| (r.op, Self::source_for(&r.mask, geometry, at(k + 1))))
                .collect(),
        }
    }

    /// How one shape's coverage is obtained.
    fn source_for(
        mask: &edit_model::Mask,
        geometry: &Geometry,
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    ) -> CoverageSource {
        match mask {
            edit_model::Mask::Brush(b) => CoverageSource::Painted {
                brush: b.clone(),
                geometry: *geometry,
                prebuilt,
            },
            edit_model::Mask::Luminance(r) => CoverageSource::Range(*r),
            edit_model::Mask::Colour(r) => CoverageSource::Colour(*r),
            m @ (edit_model::Mask::Linear(_) | edit_model::Mask::Radial(_)) => {
                CoverageSource::Analytic(m.clone())
            }
        }
    }
}

/// A view of the buffer [`MaskStage::apply`] was handed, addressed in **frame** coordinates.
///
/// ⚠ Under a region render the buffer *is* the region, so a frame coordinate is offset by the
/// region's origin.
///
/// ⚠⚠ **Out of range CLAMPS to the nearest edge pixel, and that is not merely a totality
/// formality.** The rasterizer asks for the pixel under each dab's centre, and a dab is kept when
/// its *disc* overlaps the clip — so its centre can lie up to one radius outside the region, where
/// this view has no pixels. Returning black there would give such a dab an absurd reference and
/// make it paint nothing; the nearest edge pixel is usually close to the truth, and it keeps the
/// error small and local. The residual is pinned by
/// `an_auto_masked_clipped_raster_agrees_away_from_the_clip_edge`.
struct BaseView<'a> {
    image: &'a ImageBuf,
    x0: usize,
    y0: usize,
}

impl crate::brush::BasePixels for BaseView<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        if self.image.width() == 0 || self.image.height() == 0 {
            return [0.0; 3];
        }
        let lx = x.saturating_sub(self.x0).min(self.image.width() - 1);
        let ly = y.saturating_sub(self.y0).min(self.image.height() - 1);
        self.image.pixel(lx, ly)
    }
}

impl PipelineStage for MaskStage {
    fn name(&self) -> &'static str {
        "mask"
    }

    fn accelerate_inner(&mut self, f: &crate::SubStageAccelerator<'_>) {
        let taken = std::mem::take(&mut self.stages);
        self.stages = f(taken, &self.adjustments, self.scale);
    }

    fn sub_stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    fn set_coverage_rasterizer(&mut self, r: &std::sync::Arc<dyn crate::CoverageRasterizer>) {
        self.coverage_gpu = Some(r.clone());
    }

    fn has_coverage_rasterizer(&self) -> bool {
        self.coverage_gpu.is_some()
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || self.stages.is_empty() {
            return image;
        }

        // ⚠ Without a region the buffer *is* the frame — byte for byte the whole-frame path.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };
        if fw == 0 || fh == 0 {
            return image;
        }
        let aspect = fw as f32 / fh as f32;

        // ⚠ Whole-frame, never region-sized. Coverage is defined over the frame, so a
        // region-sized raster would move under panning — the same class of bug the mask overlay
        // hit in 3.1a.
        // ⚠ One entry per component, index 0 being the base. An analytic or range component
        // rasterizes nothing and answers from inside the loop; only a painted one has a map.
        let build_map = |src: &CoverageSource, j: usize| match src {
            // ⚠ Neither of these rasterizes anything: one answers from the position, the other
            // from the pixel, both inside the loop below.
            CoverageSource::Analytic(_) | CoverageSource::Range(_) | CoverageSource::Colour(_) => {
                None
            }
            CoverageSource::Painted {
                prebuilt: Some(c), ..
            } => Some(std::sync::Arc::clone(c)),
            CoverageSource::Painted {
                brush, geometry, ..
            } => {
                // ⚠ Clip to this render's region. `w`/`h` are the incoming image's dimensions,
                // which *are* the region's size, and `x0`/`y0` its frame-space origin. With no
                // region the clip is `None` and the result is the whole frame, byte for byte.
                //
                // ⚠ This must be the same rectangle `render_engine`'s `BrushCache` clips to, or a
                // prebuilt map would disagree with this fallback. It is, by construction: both
                // derive from the same `RegionRect`. A mismatch would not error — it would read
                // `outside` across the difference, fading the mask out partway across the image.
                let clip = self.region.map(|r| (r.x0, r.y0, w, h));
                // ⚠⚠ **The buffer is in hand here and has not been cloned yet**, so the sampler
                // costs nothing at all. This is also the *only* path an auto-masked brush can
                // take: `render_engine`'s `BrushCache` declines to prebuild a map for a brush
                // whose `needs_base()` is true, which is what stops a gated brush ever rendering
                // positionally while the canvas tint shows it stopping at an edge.
                let view = BaseView {
                    image: &image,
                    x0,
                    y0,
                };
                // ⚠⚠ **Planned ONCE and offered to both.** `plan_dabs` does the `SPACING`
                // interpolation, the per-dab gate reference, the clip rejection and the box; the
                // GPU and the CPU then consume the same plan, so they cannot disagree about which
                // dabs exist. A GPU path planning its own would be a second copy of that
                // arithmetic — the duplication `adjustment_stages` exists to avoid for stages.
                //
                // ⚠ Fails toward the CPU at every step: no rasteriser offered, a plan that yields
                // nothing, or a rasteriser that declines (no adapter, a box below its floor, an
                // allocation refused) all fall through to `rasterize_in`, costing only the attempt.
                let built = self
                    .coverage_gpu
                    .as_ref()
                    .and_then(|r| {
                        let plan =
                            crate::brush::plan_dabs(brush, geometry, fw, fh, clip, Some(&view))?;
                        r.rasterize(
                            &plan,
                            brush.invert,
                            Some(crate::brush::BaseRef {
                                plane: image.data(),
                                w,
                                h,
                                x0,
                                y0,
                            }),
                        )
                    })
                    .map(std::sync::Arc::new)
                    .unwrap_or_else(|| {
                        std::sync::Arc::new(crate::brush::rasterize_in(
                            brush,
                            geometry,
                            fw,
                            fh,
                            clip,
                            Some(&view),
                        ))
                    });
                // ⚠ Reported, not cached here. This stage holds no cache and does no lookup: what
                // identifies a base lives beside `PrefixKey`, in `render_engine`. See the
                // coverage-cache spec's §4.0.
                // ⚠ Reported, not cached here. This stage holds no cache and does no lookup: what
                // identifies a base lives beside `PrefixKey`, in `render_engine`. See the
                // coverage-cache spec's §4.0.
                //
                // ⚠ `(i, j)` — the mask's position in `edits.masks` and the component's position
                // within it — so a deposited map lands under the key `render_engine` computed for
                // *that shape*. Filing a refinement's coverage under the base's index would be a
                // stale hit, which renders positionally while the tint shows the gate working.
                if let Some((sink, i)) = &self.deposit {
                    sink.deposit(*i, j, std::sync::Arc::clone(&built));
                }
                Some(built)
            }
        };
        let t_cov = std::time::Instant::now();
        let mut maps: Vec<Option<std::sync::Arc<crate::brush::CoverageMap>>> =
            Vec::with_capacity(1 + self.composite.refine.len());
        maps.push(build_map(&self.composite.base, 0));
        for (j, (_, src)) in self.composite.refine.iter().enumerate() {
            maps.push(build_map(src, j + 1));
        }
        let d_cov = t_cov.elapsed();
        // ⚠ An **optimisation, not a correctness guard** — verified by deleting it and watching
        // every test still pass. An empty coverage map reads 0 everywhere, and blending at
        // coverage 0 is bit-exactly the identity, so the output is already right without this.
        // What it saves is a full-image clone plus a full run of every sub-stage, which for an
        // unpainted brush is the entire cost of the stage.
        //
        // ⚠⚠ **And it stops being merely an optimisation once an `Add` refinement exists.** An
        // inert base stays inert under `Subtract` and `Intersect` and does NOT under `Add`, where
        // the added component is the whole mask. `an_added_component_survives_an_inert_base` is
        // what fails when the `Add` clause is dropped, and nothing else in the suite can see it —
        // the render would simply be the unmasked photograph.
        if self.composite.base_inert_is_enough()
            && maps
                .first()
                .and_then(|m| m.as_ref())
                .is_some_and(|c| c.is_inert())
        {
            return image;
        }

        // Run the sub-stages on a copy. The clone is the price of reusing every stage
        // unchanged; a bounding-box optimisation is deferred to 3.1b/c, where masks are small
        // enough for it to pay.
        let t_clone = std::time::Instant::now();
        let mut adjusted = image.clone();
        let d_clone = t_clone.elapsed();
        let t_sub = std::time::Instant::now();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }
        let d_sub = t_sub.elapsed();
        let t_blend = std::time::Instant::now();

        // ⚠ `map_pixels_xy` rather than a hand-written loop: it is rayon-parallel over rows
        // and is the idiom `VignetteStage` already uses for position-dependent work. The
        // closure captures `&adjusted`, which is `Sync` because `ImageBuf` is a `Vec<f32>`.
        let out = image.map_pixels_xy(|x, y, base| {
            // Pixel position in the *frame's* normalised coordinates.
            let u = (x0 + x) as f32 / fw as f32;
            let v = (y0 + y) as f32 / fh as f32;
            // ⚠ `base`, never `adjusted`, for every component. A mask keyed on its own output
            // chases its effect: lifting the shadows moves them out of the band that selected
            // them, and a saturation or white-balance move shifts the very hue a colour range
            // measured. Every component reading the *same* base is what leaves no second rule to
            // keep in step.
            //
            // ⚠ `BaseFacts` is per pixel and dropped here — two `Option`s on the stack. It is
            // what makes three colour components cost one Oklab round trip rather than three.
            let mut facts = BaseFacts::default();
            let cx = PixelCtx {
                u,
                v,
                aspect,
                fx: x0 + x,
                fy: y0 + y,
                maps: &maps,
            };
            let c = self.composite.at(&cx, base, &mut facts);
            let adj = adjusted.pixel(x, y);
            [
                base[0] + (adj[0] - base[0]) * c,
                base[1] + (adj[1] - base[1]) * c,
                base[2] + (adj[2] - base[2]) * c,
            ]
        });
        let d_blend = t_blend.elapsed();
        // ⚠⚠ Kept permanently, the `RUSTROOM_DEBUG_BRUSH` / `RUSTROOM_DEBUG_RENDER` precedent.
        // This split is what reframed M8.3i: it showed that a mask has NO dominant phase — HSL
        // is 84% sub-stages, a colour range 91% blend, an auto-masked brush 97% coverage — so
        // "port the mask loop" is not one thing. Rebuilding it by hand is the alternative.
        if std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
            eprintln!(
                "MASK {}x{} stages={} cov={:.2} clone={:.2} sub={:.2} blend={:.2} total={:.2}",
                w,
                h,
                self.stages.len(),
                d_cov.as_secs_f64() * 1e3,
                d_clone.as_secs_f64() * 1e3,
                d_sub.as_secs_f64() * 1e3,
                d_blend.as_secs_f64() * 1e3,
                (d_cov + d_clone + d_sub + d_blend).as_secs_f64() * 1e3,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // ⚠ In the test module, not at the top: production reads these through `Composite`
    // now, so a module-level import would be unused in a non-test build.
    use super::super::mask_coverage::{hue_chroma, lightness};
    use edit_model::{LinearGradient, LocalAdjustments, Mask};

    #[test]
    fn a_masked_brush_gates_against_the_buffer_it_is_handed() {
        // ⚠⚠ The wiring test. The gate's arithmetic is proven in `brush::raster`, and nothing
        // there says `MaskStage` ever hands it a base at all. Sabotaging the `Some(&BaseView…)`
        // back to `None` leaves every raster test green and fails only this one.
        use edit_model::{Brush, BrushStroke};

        // A colour split. ⚠ Under the shipped design a *tonal* split would work here too, since
        // both ends of the comparison come from this same buffer — see
        // `the_gate_sees_a_purely_tonal_edge` in `brush::raster`.
        const LEFT: [f32; 3] = [0.05, 0.20, 0.05];
        const RIGHT: [f32; 3] = [0.60, 0.15, 0.50];
        let mut img = ImageBuf::new(80, 20);
        for y in 0..20 {
            for x in 0..80 {
                let c = if x < 40 { LEFT } else { RIGHT };
                let i = (y * 80 + x) * 3;
                img.data_mut()[i..i + 3].copy_from_slice(&c);
            }
        }
        let masked = MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    // ⚠ A single dab centred on the LEFT colour, wide enough to reach past the
                    // split. A *stroke* across the split would legitimately paint both sides,
                    // since each dab references what is under itself.
                    points: vec![[0.25, 0.5]],
                    radius: 0.4,
                    hardness: 100.0,
                    flow: 100.0,
                    auto_mask: true,
                    tolerance: 25.0,
                    ..Default::default()
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.5,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&masked, RenderScale::new(1.0), &Geometry::default()).apply(img);
        assert!(
            out.pixel(10, 10)[0] > LEFT[0] * 1.5,
            "the matching side must be lifted: {}",
            out.pixel(10, 10)[0]
        );
        assert!(
            (out.pixel(70, 10)[0] - RIGHT[0]).abs() < 1e-6,
            "the far colour must be untouched: {}",
            out.pixel(70, 10)[0]
        );
    }

    /// ⚠⚠ **The carrier.** `BrushCoverage`'s own test proves the channel; nothing proves the stage
    /// uses it. That is the W5 shape — creative LUTs shipped with the maths fully tested and
    /// nothing carrying the data to it.
    ///
    /// ⚠ An **auto-masked** brush specifically: a plain one is prebuilt by `render_engine` and
    /// never reaches this path, so a fixture using one would pass against a stage that deposits
    /// nothing.
    #[test]
    fn an_auto_masked_brush_reports_the_coverage_it_builds() {
        use edit_model::{Brush, BrushStroke};

        let masked = MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.3,
                    hardness: 100.0,
                    flow: 100.0,
                    auto_mask: true,
                    tolerance: 35.0,
                    ..Default::default()
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        };
        let coverage = std::sync::Arc::new(crate::BrushCoverage::with_slots(&[1]));
        let stage = MaskStage::new_in_region(
            &masked,
            RenderScale::new(1.0),
            None,
            &Geometry::default(),
            &[],
            color::wb::Illuminant::default(),
            Some((std::sync::Arc::clone(&coverage), 0)),
        );
        let _ = stage.apply(flat(32, 24));
        assert!(
            coverage.take_built(0, 0).is_some(),
            "the stage built a coverage map and did not report it, so nothing can cache it"
        );
    }

    /// A flat mid-grey image.
    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for s in img.data_mut() {
            *s = 0.5;
        }
        img
    }

    fn masked(from: [f32; 2], to: [f32; 2]) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Linear(LinearGradient {
                from,
                to,
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn coverage_zero_leaves_the_pixel_untouched_and_one_applies_fully() {
        // The two endpoints of the blend. +1 EV doubles a linear sample, so mid-grey 0.5
        // becomes 1.0 where coverage is full and stays 0.5 where it is zero.
        let stage = MaskStage::new(
            &masked([0.0, 0.0], [1.0, 0.0]),
            RenderScale::FULL,
            &Geometry::default(),
        );
        let img = stage.apply(flat(64, 16));
        let left = img.pixel(0, 8)[0];
        let right = img.pixel(63, 8)[0];
        assert!((left - 0.5).abs() < 1e-4, "left edge moved: {left}");
        assert!(
            (right - 1.0).abs() < 2e-2,
            "right edge should be near +1 EV: {right}"
        );
    }

    #[test]
    fn a_subtracted_radial_is_cut_out_of_a_linear() {
        // A +1 EV lift under a left-to-right gradient, with a radial subtracted from the right
        // half. At the right edge the gradient is at full coverage, so the cut-out is the only
        // thing that can leave the pixel alone.
        let mut m = masked([0.0, 0.0], [1.0, 0.0]);
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: Mask::Radial(edit_model::RadialGradient {
                center: [0.95, 0.5],
                radius: [0.3, 1.0],
                feather: 0.0,
                invert: false,
            }),
        });
        let img = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(64, 16));
        let inside = img.pixel(63, 8)[0];
        let outside = img.pixel(40, 8)[0];
        assert!(
            (inside - 0.5).abs() < 1e-4,
            "inside the subtracted radial the pixel must be untouched: {inside}"
        );
        assert!(
            outside > 0.55,
            "outside it the gradient must still lift: {outside}"
        );
    }

    #[test]
    fn an_added_component_survives_an_inert_base() {
        // ⚠⚠ **The `is_inert` hazard, as a test.** The base is an unpainted brush — inert, so the
        // old fast path returns the image untouched — and the refinement is an added gradient.
        // Short-circuiting on the base alone makes the added component vanish **silently**: the
        // result is simply the unmasked photograph, which looks like nothing rather than an error.
        let mut m = MaskedAdjustment {
            mask: Mask::Brush(edit_model::Brush::default()),
            ..Default::default()
        };
        m.adjustments.exposure = 1.0;
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Add,
            mask: Mask::Linear(LinearGradient {
                from: [-1.0, 0.0],
                to: [-0.5, 0.0],
                invert: false,
            }),
        });
        let img = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(16, 8));
        let got = img.pixel(8, 4)[0];
        assert!(
            got > 0.9,
            "an added component must render over an inert base: {got}"
        );
    }

    #[test]
    fn a_subtracted_component_over_an_inert_base_still_short_circuits() {
        // The other half, and what keeps the fast path honest: with no `Add`, an inert base
        // really is inert, so the output must be byte-identical.
        let mut m = MaskedAdjustment {
            mask: Mask::Brush(edit_model::Brush::default()),
            ..Default::default()
        };
        m.adjustments.exposure = 1.0;
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: Mask::Linear(LinearGradient {
                from: [-1.0, 0.0],
                to: [-0.5, 0.0],
                invert: false,
            }),
        });
        let before = flat(16, 8);
        let after =
            MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(before.clone());
        assert_eq!(after, before);
    }

    #[test]
    fn an_inactive_adjustment_set_is_a_no_op() {
        // No stages to run means no clone and no blend — and byte-identical output, which is
        // what keeps the existing goldens unchanged when `masks` is empty.
        let stage = MaskStage::new(
            &MaskedAdjustment::default(),
            RenderScale::FULL,
            &Geometry::default(),
        );
        let before = flat(8, 8);
        let after = stage.apply(before.clone());
        assert_eq!(after, before);
    }

    #[test]
    fn a_region_render_matches_the_whole_frame_cropped() {
        // ⚠ The load-bearing test, and a direct descendant of the vignette's. A mask is
        // defined in *frame* coordinates while the 1:1 view renders a crop, so the region's
        // own dimensions must never be mistaken for the frame's. Get this wrong and masks
        // slide as you zoom.
        let (fw, fh) = (64usize, 32usize);
        let (x0, y0, rw, rh) = (10usize, 6usize, 20usize, 12usize);
        let m = masked([0.0, 0.0], [1.0, 1.0]);

        let whole = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(fw, fh));

        let region = Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let part = MaskStage::new_in_region(
            &m,
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            &[],
            color::wb::Illuminant::default(),
            None,
        )
        .apply(flat(rw, rh));

        for y in 0..rh {
            for x in 0..rw {
                assert_eq!(
                    part.pixel(x, y),
                    whole.pixel(x0 + x, y0 + y),
                    "region pixel ({x},{y}) differs from the whole frame's ({},{})",
                    x0 + x,
                    y0 + y
                );
            }
        }
    }

    use edit_model::{Brush, BrushStroke};

    fn painted() -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.2,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: false,
                    ..Default::default()
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_brush_mask_brightens_only_where_it_was_painted() {
        let stage = MaskStage::new(&painted(), RenderScale::FULL, &Geometry::default());
        let out = stage.apply(flat(80, 40));
        // `flat` is mid-grey 0.5, so +1 EV lifts the painted core to 1.0.
        assert!(
            out.pixel(40, 20)[0] > 0.9,
            "the painted centre must be lifted"
        );
        assert!(
            (out.pixel(2, 2)[0] - 0.5).abs() < 1e-6,
            "the unpainted corner must be untouched"
        );
    }

    #[test]
    fn an_unpainted_brush_mask_changes_nothing() {
        // ⚠ A mask added but not yet painted must be exactly inert — not merely close. This
        // pins *inertness*, not the early return above: that return is an optimisation, and
        // removing it leaves this test passing (checked).
        let m = MaskedAdjustment {
            mask: Mask::Brush(Brush::default()),
            ..painted()
        };
        let stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let src = flat(80, 40);
        let out = stage.apply(src.clone());
        assert_eq!(out, src);
    }

    #[test]
    fn a_brush_mask_in_a_region_matches_the_same_part_of_the_whole() {
        // The region path must agree with the whole-frame path pixel for pixel, which is what
        // makes zooming safe.
        //
        // ⚠ This test predates region-scoped coverage and now guards it too: the region path
        // rasterizes a *clipped* map while the whole-frame path does not, so any clip that shifts
        // or truncates coverage shows up here. Watched failing with the clip origin dropped. The same property `a_region_render_matches_the_whole_render_cropped`
        // pins for the pipeline at large.
        let whole =
            MaskStage::new(&painted(), RenderScale::FULL, &Geometry::default()).apply(flat(80, 40));
        let region = Region {
            full_w: 80,
            full_h: 40,
            x0: 30,
            y0: 10,
        };
        let got = MaskStage::new_in_region(
            &painted(),
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            &[],
            color::wb::Illuminant::default(),
            None,
        )
        .apply(flat(20, 20));
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(
                    got.pixel(x, y),
                    whole.pixel(30 + x, 10 + y),
                    "at ({x}, {y})"
                );
            }
        }
    }

    /// A vertical lightness ramp: black at the top, white at the bottom, in the working space.
    ///
    /// ⚠⚠ **A uniform buffer cannot see a luminance mask at all** — every pixel selects
    /// identically, so a wrong band, a wrong feather and a deleted stage are indistinguishable.
    /// `flat()` is exactly the fixture where this feature's bugs are inert, which is why every
    /// test below uses this one instead.
    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            // Authored as a *display-referred* value and decoded, so a row's index maps directly
            // onto the lightness the mask is specified in.
            let l = y as f32 / (h - 1) as f32;
            let v = color::srgb_to_prophoto([
                color::srgb_to_linear(l),
                color::srgb_to_linear(l),
                color::srgb_to_linear(l),
            ]);
            for x in 0..w {
                img.set_pixel(x, y, v);
            }
        }
        img
    }

    fn ranged(r: edit_model::LuminanceRange) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Luminance(r),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    /// The lightness the stage itself would compute for a rendered row — the same rule, so the
    /// tests below cannot drift from the implementation's idea of where a band sits.
    fn row_lightness(img: &ImageBuf, y: usize) -> f32 {
        lightness(img.pixel(0, y))
    }

    #[test]
    fn a_luminance_range_lifts_only_the_tones_inside_its_band() {
        // 64 rows of a black-to-white ramp; the band selects the darker third with a hard edge.
        let src = ramp(4, 64);
        let m = ranged(edit_model::LuminanceRange {
            low: 0.0,
            high: 33.0,
            feather: 0.0,
            invert: false,
        });
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());

        let mut lifted = 0;
        let mut untouched = 0;
        for y in 0..64 {
            let l = row_lightness(&src, y);
            let before = src.pixel(0, y)[1];
            let after = out.pixel(0, y)[1];
            if l <= 32.0 {
                // +1 EV doubles a linear sample. Row 0 is exactly black and cannot double, so it
                // is excluded from the count rather than from the assertion.
                assert!(
                    after >= before * 1.999 - 1e-6,
                    "row {y} (l={l:.1}) is inside the band and must be lifted: {before} -> {after}"
                );
                if before > 1e-5 {
                    lifted += 1;
                }
            } else if l >= 34.0 {
                assert_eq!(
                    after, before,
                    "row {y} (l={l:.1}) is outside the band and must be untouched"
                );
                untouched += 1;
            }
        }
        // ⚠ Both counts asserted: a stage that lifted *everything* and one that lifted nothing
        // are equally wrong, and only the pair distinguishes them.
        assert!(
            lifted >= 10,
            "the band must actually contain tones: {lifted}"
        );
        assert!(
            untouched >= 30,
            "and plenty must sit outside it: {untouched}"
        );
    }

    #[test]
    fn the_band_is_selected_on_the_pixel_before_the_adjustment() {
        // ⚠⚠ The load-bearing test of the whole feature. Keyed on the *adjusted* pixel, a +1 EV
        // lift moves a shadow out of the band that selected it, so its own correction fades away
        // — a mask chasing its own effect. Concretely: the strongest lift keys on `base`, so a
        // row well inside the band comes back at exactly 2x. Keyed on `adjusted` it would come
        // back at something between 1x and 2x, and the closer to the band's top edge the worse.
        let src = ramp(4, 64);
        let m = ranged(edit_model::LuminanceRange {
            low: 0.0,
            high: 40.0,
            feather: 0.0,
            invert: false,
        });
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());

        // The brightest row still comfortably inside the band — where the two rules diverge most,
        // because +1 EV is exactly what pushes it out.
        let y = (0..64)
            .rfind(|&y| row_lightness(&src, y) <= 39.0)
            .expect("the fixture must contain rows inside the band");
        let before = src.pixel(0, y)[1];
        let after = out.pixel(0, y)[1];
        assert!(
            (after / before - 2.0).abs() < 1e-3,
            "row {y} must get the FULL lift, not a faded one: {before} -> {after}"
        );
    }

    #[test]
    fn feather_produces_a_partial_lift_between_the_extremes() {
        // A soft band must actually blend, not merely widen. Asserted as a strict ordering across
        // three rows rather than against a magic number, so it survives any retune of the ramp's
        // shape while still failing if the shoulder collapses to a step.
        let src = ramp(4, 64);
        let m = ranged(edit_model::LuminanceRange {
            low: 0.0,
            high: 40.0,
            feather: 40.0, // ramp half-width = 20 lightness points, so 40..60 is the shoulder
            invert: false,
        });
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());

        let ratio = |y: usize| out.pixel(0, y)[1] / src.pixel(0, y)[1];
        let inside = (0..64).find(|&y| row_lightness(&src, y) >= 20.0).unwrap();
        let middle = (0..64).find(|&y| row_lightness(&src, y) >= 50.0).unwrap();
        let outside = (0..64).find(|&y| row_lightness(&src, y) >= 65.0).unwrap();

        assert!(
            (ratio(inside) - 2.0).abs() < 1e-3,
            "inside the band: {}",
            ratio(inside)
        );
        assert!(
            (ratio(outside) - 1.0).abs() < 1e-3,
            "past the shoulder: {}",
            ratio(outside)
        );
        let mid = ratio(middle);
        assert!(
            mid > 1.2 && mid < 1.8,
            "mid-shoulder must be partial, not either extreme: {mid}"
        );
    }

    #[test]
    fn a_luminance_range_in_a_region_matches_the_same_part_of_the_whole() {
        // Coverage is a function of the pixel, so it is region-independent by construction — but
        // that is exactly the sort of claim that stops being true when someone reaches for `u`/`v`
        // inside the arm. Same guard the analytic and brush masks each carry.
        let m = ranged(edit_model::LuminanceRange {
            low: 10.0,
            high: 70.0,
            feather: 30.0,
            invert: false,
        });
        let whole = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(ramp(32, 64));
        let region = Region {
            full_w: 32,
            full_h: 64,
            x0: 8,
            y0: 20,
        };
        let part = MaskStage::new_in_region(
            &m,
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            &[],
            color::wb::Illuminant::default(),
            None,
        )
        .apply(ramp(32, 64).crop(8, 20, 16, 24));
        for y in 0..24 {
            for x in 0..16 {
                assert_eq!(
                    part.pixel(x, y),
                    whole.pixel(8 + x, 20 + y),
                    "at ({x}, {y})"
                );
            }
        }
    }

    /// ⚠⚠ **A range mask selects on PRE-TONAL pixels in its capture half — accepted (2026-09-06),
    /// and pinned here so it is a documented property rather than something found later and
    /// filed as a bug.**
    ///
    /// `MaskStage` computes coverage from the buffer it is applied to. The capture half runs in
    /// the **prefix**, before every tonal adjustment, so a luminance range selects the tones of
    /// the *scene* rather than of the rendered photograph — while the same mask's tail half
    /// selects post-tonal. One mask, two selections.
    ///
    /// For a correction of the *capture* that is arguably the more correct answer rather than a
    /// compromise, and it is what the global stages already do: global noise reduction is applied
    /// to pre-tonal data too.
    #[test]
    fn a_luminance_range_selects_pre_tonal_tones_in_the_capture_half() {
        let src = ramp(16, 32);
        let masked = MaskedAdjustment {
            mask: Mask::Luminance(edit_model::LuminanceRange {
                low: 0.0,
                high: 30.0,
                feather: 0.0,
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 90.0 },
                ..Default::default()
            },
        };
        let out = MaskStage::new_capture(
            &masked,
            RenderScale::FULL,
            &edit_model::Geometry::default(),
            &[],
        )
        .apply(src.clone());

        let moved = |y: usize| (out.pixel(8, y)[0] - src.pixel(8, y)[0]).abs();
        // ⚠ Rows are lightness: row 0 is black, row 31 white. The band is the dark end.
        assert!(
            moved(2) > 1e-4,
            "a dark row is inside the band and must be corrected, moved {}",
            moved(2)
        );
        assert!(
            moved(30) < 1e-4,
            "a bright row is outside it and must not be, moved {}",
            moved(30)
        );
    }

    #[test]
    fn an_inverted_luminance_range_lifts_everything_outside_the_band() {
        let src = ramp(4, 64);
        let m = ranged(edit_model::LuminanceRange {
            low: 0.0,
            high: 40.0,
            feather: 0.0,
            invert: true,
        });
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());
        let dark = (0..64).find(|&y| row_lightness(&src, y) >= 20.0).unwrap();
        let light = (0..64).find(|&y| row_lightness(&src, y) >= 60.0).unwrap();
        assert_eq!(
            out.pixel(0, dark),
            src.pixel(0, dark),
            "the band itself is now excluded"
        );
        assert!(
            out.pixel(0, light)[1] / src.pixel(0, light)[1] > 1.9,
            "and everything outside it is lifted"
        );
    }

    /// Three vertical patches, authored in **linear sRGB** and decoded into the working space —
    /// the rule `luminance_chart` follows, and for its reason: a literal read as ProPhoto is a far
    /// more saturated colour than any camera produces.
    fn patches(cols: [[f32; 3]; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(30, 4);
        for y in 0..4 {
            for x in 0..30 {
                img.set_pixel(x, y, color::srgb_to_prophoto(cols[x / 10]));
            }
        }
        img
    }

    #[test]
    fn a_colour_range_mask_really_reaches_the_pixels() {
        // ⚠⚠ **The wiring test.** `ColourRange` has its own unit tests in `edit-model`, but
        // nothing else can see that a colour mask reaches `MaskStage` and moves pixels — the half
        // this project has repeatedly shipped broken. It is also the only thing that can see the
        // catch-all trap: routing `Mask::Colour` into `CoverageSource::Analytic` compiles, runs,
        // and produces a mask covering **nothing**.
        let red = [0.5, 0.05, 0.05];
        let green = [0.05, 0.5, 0.05];
        // ⚠ A NOISY near-neutral, not a perfect grey. A perfect grey has an undefined hue and
        // would be rejected by a broken gate too, which is the fixture fault the spec names.
        let grey = [0.30, 0.297, 0.303];
        let src = patches([red, green, grey]);

        // ⚠ The fixture asserts its own premise before testing anything. Without this the test
        // could pass on patches that never sat where it believes they sit — and the hue angles
        // are not guessable by eye, they are a property of Oklab.
        let (hr, cr) = hue_chroma(src.pixel(5, 0));
        let (hg, cg) = hue_chroma(src.pixel(15, 0));
        let (_, cn) = hue_chroma(src.pixel(25, 0));
        assert!(
            (0.0..=60.0).contains(&hr),
            "fixture check: the red patch must sit inside a 0..60 band, got {hr}"
        );
        assert!(
            !(0.0..=60.0).contains(&hg),
            "fixture check: the green patch must sit outside it, got {hg}"
        );
        assert!(
            cr > 0.05 && cg > 0.05,
            "fixture check: both must be real colours, got {cr} and {cg}"
        );
        assert!(
            cn < 0.02,
            "fixture check: the near-neutral must be under the gate, got {cn}"
        );

        let m = MaskedAdjustment {
            mask: Mask::Colour(edit_model::ColourRange {
                low: 0.0,
                high: 60.0,
                feather: 0.0,
                min_chroma: 0.02,
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());

        // +1 EV doubles a linear sample.
        let ratio = |x: usize| out.pixel(x, 0)[1] / src.pixel(x, 0)[1];
        assert!(
            (ratio(5) - 2.0).abs() < 1e-3,
            "the red patch must get the full lift, got {}",
            ratio(5)
        );
        // ⚠ All three asserted. A stage that lifted everything and one that lifted nothing are
        // equally wrong, and the near-neutral is what proves the chroma gate is wired rather than
        // merely present in `edit-model`.
        assert_eq!(
            out.pixel(15, 0),
            src.pixel(15, 0),
            "green is outside the band and must be untouched"
        );
        assert_eq!(
            out.pixel(25, 0),
            src.pixel(25, 0),
            "and a near-neutral is under the chroma gate however well its hue matches"
        );
    }

    #[test]
    fn the_hue_band_is_selected_on_the_pixel_before_the_adjustment() {
        // ⚠⚠ **Added because the sabotage step showed the wiring test could not see this.** Keying
        // the band on `adjusted` instead of `base` left `a_colour_range_mask_really_reaches_the_
        // pixels` green, because its payload is an **exposure lift** — a scalar gain, which
        // preserves `r:g:b` and therefore preserves hue exactly. The rule is invisible to any
        // adjustment that does not move the pixel out of its own selection.
        //
        // A saturation collapse does. Keyed on `base` the patch is selected and goes grey; keyed
        // on `adjusted` the stage measures a pixel it has just made neutral, the chroma gate
        // rejects it, coverage falls to zero and **the mask undoes itself** — the "mask chasing
        // its own effect" failure, in its sharpest form.
        let red = [0.5, 0.05, 0.05];
        let src = patches([red, red, red]);
        let m = MaskedAdjustment {
            mask: Mask::Colour(edit_model::ColourRange {
                low: 0.0,
                high: 60.0,
                feather: 0.0,
                min_chroma: 0.02,
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                saturation: -100.0,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(src.clone());

        let spread = |p: [f32; 3]| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        let before = spread(src.pixel(5, 0));
        let after = spread(out.pixel(5, 0));
        assert!(
            before > 0.05,
            "fixture check: the patch must start with real chroma, got {before}"
        );
        assert!(
            after < before * 0.05,
            "the selected colour must go fully grey, not fade back to itself: {before} -> {after}"
        );
    }

    #[test]
    fn the_analytic_masks_are_unaffected() {
        // The refactor must not disturb the two existing shapes.
        let m = MaskedAdjustment {
            mask: Mask::Radial(edit_model::RadialGradient::default()),
            refine: Vec::new(),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(80, 40));
        assert!(
            out.pixel(40, 20)[0] > 0.9,
            "the ellipse centre must be lifted"
        );
        assert!(
            (out.pixel(1, 1)[0] - 0.5).abs() < 1e-6,
            "the corner is outside"
        );
    }

    /// ⚠⚠ **The recursion-depth assumption, as a property rather than a comment.** `accelerate`
    /// offers a mask's sub-stages to an accelerator and passes a **no-op** one to the inner call,
    /// which is sound only because a mask's sub-stages can never themselves contain a mask.
    ///
    /// That is structural rather than a convention: [`crate::adjustment_stages`] is exhaustive
    /// over `LocalAdjustments`, which has **no `masks` field**, so there is nothing for it to
    /// build one from. Asserted here so a future `LocalAdjustments::masks` is caught by a test
    /// rather than by an infinite recursion at render time.
    /// A rasteriser that records what it was asked and returns a map of its own.
    ///
    /// ⚠⚠ **A stub rather than the real GPU one, and a COUNTER rather than a pixel comparison.**
    /// The two rasterisers agree to within one level by design, so nothing in the output can say
    /// which ran — the same reason `PARALLEL_STROKES` and `Ran { bytes }` exist. Using a stub also
    /// lets the wiring be tested here in `pipeline`, which has no GPU at all and must not.
    struct StubRaster {
        calls: std::sync::atomic::AtomicUsize,
        origin: std::sync::Mutex<Option<(usize, usize)>>,
    }
    impl crate::CoverageRasterizer for StubRaster {
        fn rasterize(
            &self,
            plan: &crate::brush::DabPlan,
            invert: bool,
            base: Option<crate::brush::BaseRef<'_>>,
        ) -> Option<crate::brush::CoverageMap> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *self.origin.lock().unwrap() = base.map(|b| (b.x0, b.y0));
            // ⚠ Deliberately a DIFFERENT map from the CPU's — full coverage inside the box — so
            // the test can prove the stage used what it was handed rather than merely that it ran.
            Some(crate::brush::CoverageMap {
                x0: plan.x0,
                y0: plan.y0,
                w: plan.w,
                h: plan.h,
                data: vec![255; plan.w * plan.h],
                outside: u8::from(invert) * 255,
            })
        }
    }

    fn stub() -> std::sync::Arc<StubRaster> {
        std::sync::Arc::new(StubRaster {
            calls: std::sync::atomic::AtomicUsize::new(0),
            origin: std::sync::Mutex::new(None),
        })
    }

    #[test]
    fn an_offered_rasterizer_builds_the_coverage_and_the_stage_uses_it() {
        let m = painted();
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let s = stub();
        let as_trait: std::sync::Arc<dyn crate::CoverageRasterizer> = s.clone();
        stage.set_coverage_rasterizer(&as_trait);
        assert!(stage.has_coverage_rasterizer());

        let plain = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let with_gpu = stage.apply(flat(80, 60));
        let without = plain.apply(flat(80, 60));

        assert_eq!(
            s.calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the stage must ask the rasteriser exactly once"
        );
        assert_ne!(
            with_gpu.data(),
            without.data(),
            "the stage must USE the map it was handed — a full-coverage map cannot render the \
             same picture as the real one"
        );
        assert_eq!(
            *s.origin.lock().unwrap(),
            Some((0, 0)),
            "a whole-frame render hands the rasteriser the frame's own origin"
        );
    }

    /// ⚠ Declining must fall back, not fail. A rasteriser returning `None` must leave the render
    /// **byte-identical** to having offered nothing at all — that is what makes offering one
    /// always safe, and it is the property the CLI and every export rely on by never offering.
    #[test]
    fn a_rasterizer_that_declines_falls_back_to_the_cpu() {
        struct Decline;
        impl crate::CoverageRasterizer for Decline {
            fn rasterize(
                &self,
                _: &crate::brush::DabPlan,
                _: bool,
                _: Option<crate::brush::BaseRef<'_>>,
            ) -> Option<crate::brush::CoverageMap> {
                None
            }
        }
        let m = painted();
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let r: std::sync::Arc<dyn crate::CoverageRasterizer> = std::sync::Arc::new(Decline);
        stage.set_coverage_rasterizer(&r);
        let plain = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        assert_eq!(
            stage.apply(flat(80, 60)).data(),
            plain.apply(flat(80, 60)).data(),
            "declining must be byte-identical to offering nothing"
        );
    }

    #[test]
    fn a_masks_sub_stages_never_contain_a_mask() {
        let mut m = masked([0.0, 0.0], [1.0, 1.0]);
        m.adjustments.hsl.bands[0].saturation = 40.0;
        m.adjustments.sharpening = edit_model::Sharpening {
            amount: 60.0,
            ..Default::default()
        };
        let stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let names = stage.sub_stage_names();
        assert!(
            !names.is_empty(),
            "fixture check: the mask must carry sub-stages, got {names:?}"
        );
        assert!(
            !names.contains(&"mask"),
            "a mask's sub-stages must never contain a mask, got {names:?}"
        );
    }

    /// ⚠ The hook must actually replace the list, or every equivalence test in `gpu-pipeline`
    /// passes against a no-op. This drives it with a transformer that **empties** the list, which
    /// no real accelerator would do — the point is that what it returns is what the stage keeps.
    #[test]
    fn accelerate_inner_replaces_the_sub_stage_list() {
        let m = masked([0.0, 0.0], [1.0, 1.0]);
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        assert_eq!(
            stage.sub_stage_names(),
            vec!["exposure"],
            "fixture check: one sub-stage before the hook"
        );
        // ⚠ The return type is annotated: a bare `Vec::new()` would leave the element type to be
        // inferred from the later unsize coercion to `&dyn Fn`, which does not flow backwards.
        let drop_everything = |_: Vec<Box<dyn PipelineStage>>,
                               _: &LocalAdjustments,
                               _: RenderScale|
         -> Vec<Box<dyn PipelineStage>> { Vec::new() };
        stage.accelerate_inner(&drop_everything);
        assert!(
            stage.sub_stage_names().is_empty(),
            "the hook must keep what the accelerator returned, got {:?}",
            stage.sub_stage_names()
        );
    }

    /// ⚠ The adjustments handed to the accelerator must be the mask's own, never the global
    /// `Edits` — that is the whole "plausible photograph" failure this seam exists to avoid.
    /// Captured through the hook so the test sees exactly what `gpu-pipeline` will.
    #[test]
    fn accelerate_inner_hands_out_the_masks_own_adjustments() {
        let mut m = masked([0.0, 0.0], [1.0, 1.0]);
        m.adjustments.exposure = -1.25;
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let seen = std::cell::Cell::new(f32::NAN);
        let capture = |s: Vec<Box<dyn PipelineStage>>, a: &LocalAdjustments, _: RenderScale| {
            seen.set(a.exposure);
            s
        };
        stage.accelerate_inner(&capture);
        assert_eq!(seen.get(), -1.25);
    }
}

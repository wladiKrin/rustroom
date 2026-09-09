//! A masked group of per-pixel adjustments.
//!
//! Runs ordinary stages on a copy of the image and blends the result back by the mask's
//! coverage. ⚠ **Every existing stage is reused completely unchanged** — nothing else in the
//! pipeline learns that masking exists, which is what keeps the `PipelineStage` seam opaque
//! for the eventual GPU port.

use crate::{adjustment_stages, PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::{Geometry, MaskedAdjustment};

/// Where a mask's coverage comes from.
///
/// ⚠ Resolved **once at construction**, so `apply` matches on it outside the pixel loop. This is
/// the shape a brush forces: `Mask::analytic_coverage` is a pure function of position and a brush
/// is sampled data, so the two cannot share one code path without one of them lying.
enum CoverageSource {
    /// Linear and radial — a pure function of position.
    Analytic(edit_model::Mask),
    /// Brush.
    ///
    /// ⚠ Kept **unrasterized** when no prebuilt map is supplied: the frame's dimensions are not
    /// known until `apply`, because with no region the frame *is* the image and the pipeline is
    /// built before the image flows through it. That path is correct and slow, and it is the one
    /// the CLI and export walk — they render once and have nothing to reuse.
    Painted {
        brush: edit_model::Brush,
        geometry: Geometry,
        /// A map the caller already rasterized, from `render_engine`'s cache.
        ///
        /// ⚠ It **must** have been built at the same `fw`/`fh`. The cache key includes them, so
        /// this holds by construction — a mismatch would silently sample the wrong pixels rather
        /// than erroring, which is why the key must never lose them.
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    },
    /// A luminance range — a pure function of the **pixel's value**.
    ///
    /// ⚠⚠ The third kind, and the reason this enum earns its keep a second time. Analytic masks
    /// answer from `(u, v)` and a brush answers from a rasterized buffer; a range mask needs the
    /// sample, which `apply`'s `map_pixels_xy` closure already has in hand. Widening
    /// `Mask::analytic_coverage` to take a pixel would churn every variant for one that wants it.
    ///
    /// ⚠ **Keyed on the `base` pixel — the value *before* this mask's own adjustment.** Reading
    /// the adjusted one would make the mask chase its own effect: lifting the shadows moves them
    /// out of the band that selected them, so the correction would fade as it applied.
    ///
    /// ⚠ A colour range is the same *kind* and joins below as a sibling variant.
    Range(edit_model::LuminanceRange),
    /// A colour range — a pure function of the **pixel's hue**, gated on its chroma.
    ///
    /// ⚠ The sibling [`CoverageSource::Range`] anticipated, and it inherits every one of that
    /// variant's notes: it is keyed on the `base` pixel, never the adjusted one, or the mask
    /// chases its own effect — a saturation lift moves a colour out of the band that selected it,
    /// so the correction would fade as it applied.
    Colour(edit_model::ColourRange),
}

/// Display-referred lightness of a working-space pixel, in 0..=100.
///
/// ⚠ **`color::luminance` is the one luminance rule in this codebase** — it returns ProPhoto's Y,
/// which is real physical luminance and therefore agrees with Rec.709 for real colours. A
/// hand-rolled weight triple here would be invisible per stage and wrong everywhere.
///
/// ⚠ `linear_to_srgb` clamps its input to `[0, 1]`, which is exactly the *index* clamp a range
/// mask wants and never touches the pixel itself: a specular highlight still selects at the top of
/// the scale while keeping every bit of its headroom for the adjustment that follows.
fn lightness(rgb: [f32; 3]) -> f32 {
    color::linear_to_srgb(color::luminance(rgb)) * 100.0
}

/// Oklab hue in degrees and Oklab chroma of a working-space pixel.
///
/// ⚠ **The same measurement `HslStage` makes, from the same function** — `prophoto_to_oklab`,
/// `b.atan2(a)`, `a.hypot(b)` — so "red" means the same angle to a colour range mask as it does to
/// the Red band of HSL. A second hue convention here would be invisible per stage and disagree
/// everywhere; `BAND_HUES` is the table both are measured against.
///
/// ⚠ `atan2` returns `-180..=180`. Nothing normalises it here on purpose:
/// `ColourRange::coverage_at` takes both its span and its offset with `rem_euclid(360)`, so a
/// negative angle is already handled by the one wrap rule rather than by a second one.
///
/// ⚠ Chroma is **not** clamped and must not be: it is the gate's own input, and a wide-gamut
/// colour legitimately reads above anything sRGB can express.
fn hue_chroma(rgb: [f32; 3]) -> (f32, f32) {
    let [_, a, b] = color::oklab::prophoto_to_oklab(rgb);
    (b.atan2(a).to_degrees(), a.hypot(b))
}

pub struct MaskStage {
    // ⚠ The `MaskedAdjustment` itself is deliberately not retained. Everything needed is
    // extracted at construction — the coverage source and the sub-stage list — and keeping a copy
    // as well was dead weight that `-D warnings` caught the moment the coverage lookup stopped
    // going through it.
    source: CoverageSource,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
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
            None,
            color::wb::Illuminant::default(),
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
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
        baseline: color::wb::Illuminant,
    ) -> Self {
        Self {
            source: Self::coverage_source(masked, geometry, prebuilt),
            stages: adjustment_stages(&masked.adjustments, baseline, scale),
            region,
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
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    ) -> Self {
        Self {
            source: Self::coverage_source(masked, geometry, prebuilt),
            stages: crate::capture_stages(&masked.adjustments, scale),
            region: None,
        }
    }

    /// How this mask's coverage is obtained, shared by both constructors.
    ///
    /// ⚠⚠ **Exhaustive on purpose — a catch-all here is a trap.** It used to read
    /// `other => Analytic(other.clone())`, so a new variant whose `analytic_coverage` returns
    /// `None` would fall through to `unwrap_or(0.0)` and produce a mask covering **nothing**,
    /// silently. That is precisely the failure `analytic_coverage`'s `Option` exists to make
    /// impossible, defeated one layer up. Adding a variant is a compile error here.
    fn coverage_source(
        masked: &MaskedAdjustment,
        geometry: &Geometry,
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    ) -> CoverageSource {
        match &masked.mask {
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
        let coverage = match &self.source {
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
                Some(std::sync::Arc::new(crate::brush::rasterize_in(
                    brush,
                    geometry,
                    fw,
                    fh,
                    clip,
                    Some(&view),
                )))
            }
        };
        // ⚠ An **optimisation, not a correctness guard** — verified by deleting it and watching
        // every test still pass. An empty coverage map reads 0 everywhere, and blending at
        // coverage 0 is bit-exactly the identity, so the output is already right without this.
        // What it saves is a full-image clone plus a full run of every sub-stage, which for an
        // unpainted brush is the entire cost of the stage.
        if coverage.as_ref().is_some_and(|c| c.is_inert()) {
            return image;
        }

        // Run the sub-stages on a copy. The clone is the price of reusing every stage
        // unchanged; a bounding-box optimisation is deferred to 3.1b/c, where masks are small
        // enough for it to pay.
        let mut adjusted = image.clone();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }

        // ⚠ `map_pixels_xy` rather than a hand-written loop: it is rayon-parallel over rows
        // and is the idiom `VignetteStage` already uses for position-dependent work. The
        // closure captures `&adjusted`, which is `Sync` because `ImageBuf` is a `Vec<f32>`.
        image.map_pixels_xy(|x, y, base| {
            // Pixel position in the *frame's* normalised coordinates.
            let u = (x0 + x) as f32 / fw as f32;
            let v = (y0 + y) as f32 / fh as f32;
            let c = match (&self.source, &coverage) {
                (CoverageSource::Analytic(m), _) => {
                    m.analytic_coverage(u, v, aspect).unwrap_or(0.0)
                }
                // ⚠ `base`, never `adjusted`. A mask keyed on its own output chases its effect:
                // lifting the shadows moves them out of the band that selected them.
                (CoverageSource::Range(r), _) => r.coverage(lightness(base)),
                // ⚠ `base` here too, and the failure it avoids is the sharper of the two: a
                // saturation or white-balance move inside the mask shifts the pixel's own hue, so
                // a colour range keyed on its own output would slide the selection out from under
                // the correction as it applied.
                (CoverageSource::Colour(r), _) => {
                    let (hue, chroma) = hue_chroma(base);
                    r.coverage_at(hue, chroma)
                }
                // ⚠ Indexed by FRAME pixel, not by the region-relative `(x, y)`.
                (CoverageSource::Painted { .. }, Some(cov)) => {
                    cov.at(x0 + x, y0 + y) as f32 / 255.0
                }
                (CoverageSource::Painted { .. }, None) => 0.0,
            }
            .clamp(0.0, 1.0);
            let adj = adjusted.pixel(x, y);
            [
                base[0] + (adj[0] - base[0]) * c,
                base[1] + (adj[1] - base[1]) * c,
                base[2] + (adj[2] - base[2]) * c,
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            None,
            color::wb::Illuminant::default(),
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
            None,
            color::wb::Illuminant::default(),
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
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    /// The lightness the stage itself would compute for a rendered row — the same rule, so the
    /// tests below cannot drift from the implementation's idea of where a band sits.
    fn row_lightness(img: &ImageBuf, y: usize) -> f32 {
        super::lightness(img.pixel(0, y))
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
            None,
            color::wb::Illuminant::default(),
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
            adjustments: LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 90.0 },
                ..Default::default()
            },
        };
        let out = MaskStage::new_capture(
            &masked,
            RenderScale::FULL,
            &edit_model::Geometry::default(),
            None,
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
}

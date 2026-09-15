//! Optical corrections from a lens profile: distortion, TCA and vignetting.
//!
//! ⚠ Runs **first in the prefix, before `GeometryStage`**, because the calibration is defined in
//! the uncropped, unrotated sensor frame. Being in the prefix also means it always runs
//! whole-buffer, so it needs no `Region` and no kernel margin despite being a spatial resample.

use crate::resample::{self, Resampler, Weights};
use crate::{PipelineStage, Rect, RegionSrc};
use core_image::ImageBuf;
use lens_profile::{Corrections, Corrector, LensShot, RowMap};
use rayon::prelude::*;

pub struct LensCorrectionStage {
    lens: LensShot,
    want: Corrections,
    /// ⚠ Built once here rather than per pixel — and this stage samples **three times** per
    /// pixel, once per channel, so rebuilding it inline would be three times as wasteful again.
    weights: Weights,
}

impl LensCorrectionStage {
    pub fn new(lens: LensShot, want: Corrections, resampler: Resampler) -> Self {
        Self {
            lens,
            want,
            weights: Weights::new(resampler),
        }
    }

    /// The correction for an image of these dimensions.
    ///
    /// ⚠ Built here rather than handed in, because a [`Corrector`] is tied to a specific image
    /// size and this is the only place that knows it — the preview and the 1:1 view render at
    /// different sizes through the same `Edits`. Handing one in would make a size mismatch
    /// *representable*, and it would be silently wrong rather than an error. Building it is
    /// cheap: the database is already parsed, so this is a `Modifier::new` and three `enable_*`.
    fn corrector(&self, w: usize, h: usize) -> Option<Corrector> {
        Corrector::new(&self.lens.profile, &self.lens.shot, w, h, self.want)
    }
}

impl PipelineStage for LensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        // Nothing enabled, or nothing this profile can supply: pass the image through untouched.
        let Some(corrector) = self.corrector(w, h) else {
            return image;
        };
        let src = RegionSrc::whole(&image);
        self.render(&src, &corrector, Rect { x: 0, y: 0, w, h })
    }
    /// ⚠⚠ **The margin is frame-global, never a bound on this rect's four mapped corners.** A
    /// homography sends lines to lines, so [`crate::stages::perspective::PerspectiveStage`] can
    /// map four corners and be exact; **a radial map bows every edge**, so the image of a
    /// rectangle is a curved quadrilateral whose extreme can sit at an edge midpoint. A corner
    /// bound would under-estimate, and the failure is a dark band at one edge of the region —
    /// a plausible photograph rather than an error. [`Corrector::margin`] is the one definition,
    /// and `no_pixel_displaces_further_than_the_margin` is its guarantee.
    ///
    /// ⚠ The tap radius comes from `self.weights.kernel()` rather than a stored [`Resampler`], so
    /// the weights and the radius cannot disagree. Geometry's and perspective's rule.
    ///
    /// ⚠ Dimension-preserving, so `out` is already in the input's coordinate space and
    /// [`PipelineStage::output_dims`] keeps the trait's identity default.
    fn input_rect_for(&self, out: Rect, in_w: usize, in_h: usize) -> Option<Rect> {
        if out.w == 0 || out.h == 0 || in_w == 0 || in_h == 0 {
            return None;
        }
        // ⚠ Refuse a rect this stage could not produce, rather than clamping it: a caller that
        // asked for the wrong thing must fall back to the whole-buffer path, not silently get a
        // different rectangle back without being told. Geometry's rule.
        if out.x + out.w > in_w || out.y + out.h > in_h {
            return None;
        }
        let Some(corrector) = self.corrector(in_w, in_h) else {
            // Nothing enabled: an exact identity, so the rect is its own pre-image — the same
            // branch `apply` takes when it passes the image through untouched.
            return Some(out);
        };
        Some(out.grown(corrector.margin(self.weights.kernel().radius()), in_w, in_h))
    }

    /// ⚠⚠ **`src.full` is the FULL image's dimensions, never the slice in hand.** The corrector is
    /// built against the frame — its centre, its normalisation and its autoscale are all
    /// properties of the frame — so building it from `src.buf` would correct a different lens.
    /// That is M8.6's centre sabotage in this stage's costume: a plausible photograph of the wrong
    /// part of the frame, invisible to all 63 goldens.
    fn apply_region(&self, src: &RegionSrc<'_>, rect: Rect) -> Option<ImageBuf> {
        let (w, h) = src.full;
        if rect.w == 0 || rect.h == 0 || rect.x + rect.w > w || rect.y + rect.h > h {
            return None;
        }
        let Some(corrector) = self.corrector(w, h) else {
            // An exact identity, so a sub-rect of the output is a sub-rect of the source — the
            // same branch `input_rect_for` takes.
            // ⚠ Local coordinates for the buffer copy: `rect` is in full-image space.
            let lx = rect.x.checked_sub(src.origin.0)?;
            let ly = rect.y.checked_sub(src.origin.1)?;
            return Some(src.buf.crop(lx, ly, rect.w, rect.h));
        };
        Some(self.render(src, &corrector, rect))
    }
}

impl LensCorrectionStage {
    /// Produce `rect` of the correction's output.
    ///
    /// ⚠⚠ **[`Self::apply`] is this function at full extent, which is what makes the whole-buffer
    /// and region paths bit-identical BY CONSTRUCTION rather than by comparison.** Two
    /// implementations that must agree is how they come to disagree; there is only one here.
    /// `geometry::rotate` already follows exactly this rule, and for the same reason.
    ///
    /// ⚠⚠ **The field is always filled from x = 0, never from the rect.** [`Corrector::fill_row`]
    /// reaches lensfun's `apply_geometry_distortion`, which computes each `x` as
    /// `(xu + ns * i as f64) as f32` — **from scratch per `i`, not incrementally**. Starting the
    /// row at `rect.x` therefore re-associates an f64 addition, and f64 addition is not
    /// associative: the discrepancy is ~1e-16 against an f32 spacing of ~1.2e-7, so about one
    /// pixel in 1e9 could round to a different f32. A faint band that appears one pixel in a
    /// billion is precisely the class of thing this project has paid for. Filling from zero costs
    /// ~2% of a region render — the field is 5% of the stage, and only the prefix of each row is
    /// wasted — and it is bit-identical.
    fn render(&self, src: &RegionSrc<'_>, corrector: &Corrector, rect: Rect) -> ImageBuf {
        const CH: usize = ImageBuf::CHANNELS;
        // ⚠⚠ **From x = 0 up to the rect's right edge, and BOTH halves matter.** Indexing is in
        // frame coordinates, so the row cannot start at the rect (see this function's second
        // note); but it need not run past it either, because `apply_geometry_distortion` computes
        // each `x` as `(xu + ns * i as f64) as f32` from the row's own start — so truncating the
        // row changes *how many* `i` are computed and not *what any of them is*. Bit-identical,
        // and it is what `a_region_matches_the_same_part_of_the_whole` establishes rather than
        // assumes. On a cold 1:1 entry the rect's right edge sits at ~4228 of 6246, so this is a
        // third of the field's rows returned for nothing.
        let field_w = rect.x + rect.w;
        let stride = rect.w * CH;
        let mut data = vec![0.0f32; rect.w * rect.h * CH];
        if stride == 0 {
            // `par_chunks_exact_mut(0)` panics.
            return ImageBuf::from_vec(rect.w, rect.h, data);
        }
        // ⚠ Parallel over **output** rows, which is sound because this is a gather: an output
        // pixel reads the whole source and writes only itself. Bit-identical to the sequential
        // form, which is what lets the goldens stand as the check on it.
        //
        // ⚠ `for_each_init` rather than `for_each`: the initialiser runs once per rayon job, so
        // one `RowMap` is allocated per worker and reused down its rows. Allocating per row
        // would mean zeroing the scratch millions of times over a full-resolution render.
        data.par_chunks_exact_mut(stride).enumerate().for_each_init(
            || RowMap::new(field_w),
            |map, (ry, orow)| {
                let y = rect.y + ry;
                corrector.fill_row(y, map);
                for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                    let x = rect.x + rx;
                    let s = map.source(x);
                    let gain = map.gain(x);
                    for (ch, p) in out.iter_mut().enumerate() {
                        // ⚠ Per channel, because TCA gives each channel its own source point —
                        // which is why `resample` has a second entry point at all.
                        *p = resample::sample_channel_region(
                            src,
                            s[ch][0],
                            s[ch][1],
                            ch,
                            &self.weights,
                        ) * gain;
                    }
                }
            },
        );
        ImageBuf::from_vec(rect.w, rect.h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic lens with coefficients chosen here rather than read from the bundled
    /// database — the seam that makes lens correction testable in CI at all, since a real
    /// profile needs a real camera and lens. Mirrors the golden harness's fixture.
    fn lens_shot() -> LensShot {
        lens_shot_b("0.05")
    }

    /// ⚠⚠ **The sign of `b` decides whether a four-corner bound is sound, and that is the whole
    /// reason this is a parameter.** ptlens is `r_d = r * (b * r^2 + (1 - b))`, so the radial
    /// factor is **increasing** in `r` for a pincushion correction (`b > 0`) and **decreasing**
    /// for a barrel one (`b < 0`). Increasing puts every extreme of a rect's image at a corner,
    /// so the corner box contains it; decreasing puts the extreme at an **edge midpoint** whenever
    /// the rect's span brackets the optical centre, and the corner box misses the bulge.
    /// `every_source_a_region_reads_lies_inside_the_rect_it_asked_for` needs the barrel case, and
    /// on the pincushion fixture that test cannot witness a corner bound at all.
    fn lens_shot_b(b: &str) -> LensShot {
        let xml = format!(
            r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="{b}" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                </calibration></lens>
        </lensdatabase>"#
        );
        let db = lens_profile::Catalogue::from_xml(&xml).unwrap();
        let profile = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        LensShot {
            profile,
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    /// A hard grid, not a smooth ramp: every kernel reproduces a linear ramp almost exactly, so
    /// a ramp cannot tell two kernels apart.
    fn grid(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x % 8 < 2 || y % 8 < 2 { 0.9 } else { 0.1 };
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        img
    }

    /// ⚠⚠ **THE SECOND SAMPLER.** This stage does not go through `GeometryStage`'s sampler — it
    /// has its own per-channel entry point — so a resampler setting honoured by geometry alone
    /// would leave the pass every RAW actually pays still on bilinear, lens correction being on
    /// by default. That is the half of this feature most easily shipped missing.
    #[test]
    fn the_lens_correction_honours_the_chosen_resampler() {
        let src = grid(64, 64);
        let want = Corrections {
            distortion: true,
            tca: true,
            vignetting: false,
            scale: 0.0,
        };
        let corrected = |k| LensCorrectionStage::new(lens_shot(), want, k).apply(src.clone());
        let bilinear = corrected(Resampler::Bilinear);
        assert_ne!(
            bilinear.data(),
            src.data(),
            "fixture check: the correction must actually move pixels"
        );
        for k in Resampler::ALL {
            if k == Resampler::Bilinear {
                continue;
            }
            assert_ne!(
                corrected(k).data(),
                bilinear.data(),
                "{k:?} rendered identically to bilinear"
            );
        }
    }

    /// ⚠⚠ **Byte for byte, and that is achievable here where a chain carrying a local is bounded
    /// to one ulp.** Lens correction is a pure gather, and `apply` **is** `apply_region` at full
    /// extent — so anything but exact equality means the field was computed differently for the
    /// two, which is the one way this can go wrong.
    ///
    /// ⚠ One rect in the interior and one against the frame's corner, where the margin clips and
    /// the taps run out — an implementation right in the middle and wrong at the edge is exactly
    /// the band this stage's margin exists to prevent.
    #[test]
    fn a_region_matches_the_same_part_of_the_whole() {
        let src = grid(160, 120);
        let want = Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        };
        let stage = LensCorrectionStage::new(lens_shot(), want, Resampler::CatmullRom);
        let whole = stage.apply(src.clone());
        // Fixture check: a stage that moved nothing would make every comparison below vacuous.
        assert_ne!(whole.data(), src.data(), "the correction must move pixels");

        for rect in [
            Rect {
                x: 40,
                y: 30,
                w: 48,
                h: 36,
            },
            Rect {
                x: 0,
                y: 0,
                w: 32,
                h: 24,
            },
        ] {
            let need = stage
                .input_rect_for(rect, 160, 120)
                .expect("lens correction accepts a region");
            let slice = src.crop(need.x, need.y, need.w, need.h);
            let region_src = RegionSrc {
                buf: &slice,
                origin: (need.x, need.y),
                full: (160, 120),
            };
            let got = stage
                .apply_region(&region_src, rect)
                .expect("apply_region agrees with input_rect_for");
            assert_eq!((got.width(), got.height()), (rect.w, rect.h));
            for ry in 0..rect.h {
                for rx in 0..rect.w {
                    assert_eq!(
                        got.pixel(rx, ry),
                        whole.pixel(rect.x + rx, rect.y + ry),
                        "rect {rect:?} at ({rx}, {ry})"
                    );
                }
            }
        }
    }

    /// ⚠⚠ **THE test for the margin, and the byte-for-byte equivalence test CANNOT stand in for
    /// it.** Every source point a rect's pixels read, plus the resampler's taps, must lie inside
    /// the rect [`PipelineStage::input_rect_for`] asked for — otherwise `sample_channel_region`
    /// reads black outside the buffer it was given, which is the dark band at the region's edge.
    ///
    /// ⚠⚠ **Transplanting `PerspectiveStage`'s four-corner bound here passes the equivalence test
    /// and fails this one — but only on the BARREL fixture, and working out why sharpened the
    /// design's own argument.** "A radial map bends lines, so a corner bound is unsound" is too
    /// loose: ptlens is `r_d = r * (b * r^2 + (1 - b))`, so for a **pincushion** correction
    /// (`b > 0`) the radial factor increases with `r`, every extreme of a rect's image sits at a
    /// corner, and the corner box genuinely contains it. For a **barrel** correction (`b < 0`) the
    /// factor decreases, so the extreme sits at an **edge midpoint** whenever the rect's span
    /// brackets the optical centre — and the corner box misses the bulge outright. Measured on
    /// this fixture: **9.2 px** of shortfall, against a Lanczos3 tap radius of 3.
    ///
    /// ⚠ The rects therefore **bracket the centre in both axes**, which is the condition, and the
    /// coefficient is strong enough that the bulge exceeds any tap radius. A rect in one quadrant
    /// cannot witness this however large the displacement.
    #[test]
    fn every_source_a_region_reads_lies_inside_the_rect_it_asked_for() {
        let want = Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        };
        let (w, h) = (400usize, 300usize);
        let pad = Resampler::Lanczos3.radius() as f32;
        for b in ["0.05", "-0.05", "0.3", "-0.3"] {
            let stage = LensCorrectionStage::new(lens_shot_b(b), want, Resampler::Lanczos3);
            let c = stage
                .corrector(w, h)
                .expect("the profile supplies a correction");
            for rect in [
                // Bracketing the centre in both axes: the case a barrel correction bulges in.
                Rect {
                    x: 40,
                    y: 40,
                    w: 320,
                    h: 220,
                },
                Rect {
                    x: 120,
                    y: 90,
                    w: 160,
                    h: 120,
                },
                // And one in a corner, where the margin clips against the frame.
                Rect {
                    x: 0,
                    y: 0,
                    w: 120,
                    h: 90,
                },
            ] {
                let need = stage
                    .input_rect_for(rect, w, h)
                    .expect("lens correction accepts a region");
                let mut map = RowMap::new(w);
                for ry in 0..rect.h {
                    let y = rect.y + ry;
                    c.fill_row(y, &mut map);
                    for rx in 0..rect.w {
                        let x = rect.x + rx;
                        for (ch, sc) in map.source(x).iter().enumerate() {
                            // The taps reach `pad` either side, and a source outside the frame is
                            // fine — the whole-buffer path reads black there too. Only what falls
                            // *inside* the frame has to be inside `need`.
                            let lo_x = (sc[0] - pad).max(0.0);
                            let hi_x = (sc[0] + pad).min(w as f32 - 1.0);
                            let lo_y = (sc[1] - pad).max(0.0);
                            let hi_y = (sc[1] + pad).min(h as f32 - 1.0);
                            assert!(
                                lo_x >= need.x as f32
                                    && hi_x <= (need.x + need.w) as f32 - 1.0
                                    && lo_y >= need.y as f32
                                    && hi_y <= (need.y + need.h) as f32 - 1.0,
                                "b={b} {rect:?} channel {ch} at ({x}, {y}) reads \
                                 [{lo_x}, {hi_x}] x [{lo_y}, {hi_y}], outside {need:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// ⚠⚠ **The pair must agree about what is possible**, or `region_plan` promises an input rect
    /// for an output `apply_region` then refuses to produce. `GeometryStage` declines *both* on a
    /// non-identity orientation for exactly this reason.
    #[test]
    fn the_pair_agrees_about_what_it_refuses() {
        let want = Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        };
        let stage = LensCorrectionStage::new(lens_shot(), want, Resampler::CatmullRom);
        let src = grid(64, 48);
        for rect in [
            // Past the right edge, past the bottom edge, and empty.
            Rect {
                x: 40,
                y: 0,
                w: 32,
                h: 8,
            },
            Rect {
                x: 0,
                y: 40,
                w: 8,
                h: 32,
            },
            Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 8,
            },
        ] {
            assert!(
                stage.input_rect_for(rect, 64, 48).is_none(),
                "input_rect_for accepted {rect:?}"
            );
            assert!(
                stage.apply_region(&RegionSrc::whole(&src), rect).is_none(),
                "apply_region accepted {rect:?}"
            );
        }
    }
}

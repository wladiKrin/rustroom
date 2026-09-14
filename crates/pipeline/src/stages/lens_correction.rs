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
        // ⚠ The FULL image's width, never the slice in hand: `fill_row` is indexed in frame
        // coordinates and the corrector was built against the frame.
        let field_w = src.full.0;
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
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
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
}

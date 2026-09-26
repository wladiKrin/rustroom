//! A run of per-pixel stages as ONE pass over the pixels.
//!
//! Twelve tail stages are pixel-in, pixel-out; only local contrast and sharpening read
//! neighbours. Applied one after another, each walks the whole image and writes a fresh copy —
//! nine walks and nine allocations for what is one function per pixel. [`run_stages`] folds a run
//! of consecutive [`PerPixel`] stages into one in-place walk: rows in parallel, pixels in order,
//! each pixel through every member's function before the next pixel.
//!
//! ⚠⚠ **Bit-identical to the sequential form by construction.** The same operations on the same
//! values in the same order per pixel, and no intermediate buffer ever changed a value. The
//! goldens are the check, and `a_fused_run_equals_the_sequential_application` pins it over every
//! per-pixel stage at once.
//!
//! ⚠⚠ **Grouping happens HERE, at run time, never at build time.** The stage list the builders
//! produce is untouched, so the 84 uses of `stage_names()`, `accelerate`'s name matching, the
//! region planner, the cost gate and `PrefixKey` all see exactly the list they saw before. Fusion
//! is a property of how a list is executed; `RUSTROOM_NO_FUSION=1` switches it off.
//!
//! ⚠ The walk counter is the observable: fused and unfused produce identical pixels, so nothing in
//! the output can see whether the grouping fired — the `brush::raster` precedent.

use crate::PipelineStage;
use core_image::ImageBuf;
use rayon::prelude::*;
use std::cell::Cell;

/// A stage whose output pixel is a function of its input pixel and position alone.
///
/// ⚠ Position is there for the vignette, which adds its region origin itself; the other eleven
/// ignore it. `(x, y)` are the buffer's own coordinates, never the frame's.
pub trait PerPixel: Send + Sync {
    fn px(&self, x: usize, y: usize, p: [f32; 3]) -> [f32; 3];
}

/// A borrowed stage is one too, which is what lets `per_pixel` hand back `Box::new(self)`.
impl<T: PerPixel + ?Sized> PerPixel for &T {
    fn px(&self, x: usize, y: usize, p: [f32; 3]) -> [f32; 3] {
        (**self).px(x, y, p)
    }
}

thread_local! {
    // How many fused walks this thread has taken. ⚠ Thread-local, because libtest runs tests in
    // parallel; a counter, not a flag, because a flag cannot see its own absence.
    static FUSED_WALKS: Cell<usize> = const { Cell::new(0) };
}

/// Fused walks taken on this thread so far.
pub fn fused_walks() -> usize {
    FUSED_WALKS.with(|c| c.get())
}

/// Whether the environment switched fusion off. Read once.
pub fn fusion_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTROOM_NO_FUSION").is_none())
}

/// One in-place walk: every pixel through every member of `run`, in order.
///
/// ⚠ Rows in parallel through rayon, pixels in order within a row, `x` counted rather than
/// divided. Bit-identical to `map_pixels` applied per stage: the per-pixel values are the same
/// however the pixels are split between threads.
fn walk(run: &[Box<dyn PerPixel + '_>], mut image: ImageBuf) -> ImageBuf {
    let w = image.width();
    let row = w * ImageBuf::CHANNELS;
    if row == 0 {
        return image;
    }
    image
        .data_mut()
        .par_chunks_exact_mut(row)
        .enumerate()
        .for_each(|(y, orow)| {
            for (x, px) in orow
                .as_chunks_mut::<{ ImageBuf::CHANNELS }>()
                .0
                .iter_mut()
                .enumerate()
            {
                let mut p = *px;
                for s in run {
                    p = s.px(x, y, p);
                }
                *px = p;
            }
        });
    image
}

/// The one walk every per-pixel stage's `apply` is: `stage.px` over the whole image, in place.
///
/// ⚠ `apply` and the fused walk then share one definition of what the stage does, which is what
/// makes a fused run unable to disagree with the sequential one.
pub fn apply_one(stage: &dyn PerPixel, image: ImageBuf) -> ImageBuf {
    let run: [Box<dyn PerPixel + '_>; 1] = [Box::new(stage)];
    walk(&run, image)
}

/// Run `stages` in order, folding each run of consecutive per-pixel stages into one walk.
///
/// ⚠ Takes the switch as a parameter so a test can exercise both paths without touching the
/// process environment, which every other test shares. Production goes through [`run_stages`].
pub fn run_stages_with<'a>(
    stages: impl IntoIterator<Item = &'a dyn PipelineStage>,
    mut image: ImageBuf,
    fuse: bool,
) -> ImageBuf {
    let stages: Vec<&'a dyn PipelineStage> = stages.into_iter().collect();
    let mut i = 0;
    while i < stages.len() {
        let (w, h) = (image.width(), image.height());
        if fuse {
            let mut run: Vec<Box<dyn PerPixel + 'a>> = Vec::new();
            while i < stages.len() {
                match stages[i].per_pixel(w, h) {
                    Some(p) => {
                        run.push(p);
                        i += 1;
                    }
                    None => break,
                }
            }
            if !run.is_empty() {
                FUSED_WALKS.with(|c| c.set(c.get() + 1));
                image = walk(&run, image);
                continue;
            }
        }
        image = stages[i].apply(image);
        i += 1;
    }
    image
}

/// [`run_stages_with`] under the environment's switch — what the pipeline, a mask and a GPU
/// chain's fallback all call.
pub fn run_stages<'a>(
    stages: impl IntoIterator<Item = &'a dyn PipelineStage>,
    image: ImageBuf,
) -> ImageBuf {
    run_stages_with(stages, image, fusion_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy per-pixel stage: add a constant to every sample.
    struct Plus(f32);
    impl PerPixel for Plus {
        fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
            [p[0] + self.0, p[1] + self.0, p[2] + self.0]
        }
    }
    impl PipelineStage for Plus {
        fn name(&self) -> &'static str {
            "plus"
        }
        fn apply(&self, image: ImageBuf) -> ImageBuf {
            apply_one(self, image)
        }
        fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn PerPixel + '_>> {
            Some(Box::new(self))
        }
    }

    /// A toy per-pixel stage whose result depends on position: scale by `x`.
    struct TimesX;
    impl PerPixel for TimesX {
        fn px(&self, x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
            let k = x as f32;
            [p[0] * k, p[1] * k, p[2] * k]
        }
    }
    impl PipelineStage for TimesX {
        fn name(&self) -> &'static str {
            "times_x"
        }
        fn apply(&self, image: ImageBuf) -> ImageBuf {
            apply_one(self, image)
        }
        fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn PerPixel + '_>> {
            Some(Box::new(self))
        }
    }

    /// A toy NON-per-pixel stage: flip the image vertically. It must end a run.
    struct Flip;
    impl PipelineStage for Flip {
        fn name(&self) -> &'static str {
            "flip"
        }
        fn apply(&self, image: ImageBuf) -> ImageBuf {
            let (w, h) = (image.width(), image.height());
            let mut out = ImageBuf::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    out.set_pixel(x, y, image.pixel(x, h - 1 - y));
                }
            }
            out
        }
    }

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let n = w * h * 3;
        ImageBuf::from_vec(w, h, (0..n).map(|i| (i % 97) as f32 / 97.0).collect())
    }

    fn sequential(stages: &[Box<dyn PipelineStage>], img: ImageBuf) -> ImageBuf {
        stages.iter().fold(img, |img, s| s.apply(img))
    }

    /// Fused and sequential agree sample for sample, a non-per-pixel stage splits the runs, and
    /// the walk counter says how many walks were taken.
    #[test]
    fn a_fused_run_matches_the_sequential_one_and_counts_its_walks() {
        let stages: Vec<Box<dyn PipelineStage>> = vec![
            Box::new(Plus(0.25)),
            Box::new(TimesX),
            Box::new(Flip),
            Box::new(Plus(-0.5)),
        ];
        let img = ramp(7, 5);
        let want = sequential(&stages, img.clone());
        assert_ne!(
            want.data(),
            img.data(),
            "fixture check: the stages move pixels"
        );
        let before = fused_walks();
        let got = run_stages_with(stages.iter().map(|b| b.as_ref()), img, true);
        assert_eq!(
            got.data(),
            want.data(),
            "fused must equal sequential, sample for sample"
        );
        assert_eq!(
            fused_walks() - before,
            2,
            "[plus, times_x] then [plus]: two walks"
        );
    }

    /// With the switch off nothing fuses, and the pixels are the same.
    #[test]
    fn the_switch_off_takes_no_fused_walk_and_changes_no_pixel() {
        let stages: Vec<Box<dyn PipelineStage>> = vec![Box::new(Plus(0.1)), Box::new(TimesX)];
        let img = ramp(6, 4);
        let want = sequential(&stages, img.clone());
        let before = fused_walks();
        let got = run_stages_with(stages.iter().map(|b| b.as_ref()), img, false);
        assert_eq!(got.data(), want.data());
        assert_eq!(fused_walks() - before, 0);
    }

    /// Order inside a run is list order: `Plus` then `TimesX` is not `TimesX` then `Plus`.
    #[test]
    fn a_fused_run_applies_its_members_in_list_order() {
        let ab: Vec<Box<dyn PipelineStage>> = vec![Box::new(Plus(1.0)), Box::new(TimesX)];
        let ba: Vec<Box<dyn PipelineStage>> = vec![Box::new(TimesX), Box::new(Plus(1.0))];
        let img = ramp(5, 3);
        let seq_ab = sequential(&ab, img.clone());
        let seq_ba = sequential(&ba, img.clone());
        assert_ne!(
            seq_ab.data(),
            seq_ba.data(),
            "fixture check: the two orders differ"
        );
        let fused_ab = run_stages_with(ab.iter().map(|b| b.as_ref()), img, true);
        assert_eq!(fused_ab.data(), seq_ab.data());
    }

    /// An empty image and an empty list are both fine.
    #[test]
    fn degenerate_inputs_do_not_panic() {
        let stages: Vec<Box<dyn PipelineStage>> = vec![Box::new(Plus(1.0))];
        let out = run_stages_with(stages.iter().map(|b| b.as_ref()), ImageBuf::new(0, 4), true);
        assert_eq!(out.width(), 0);
        let none: Vec<Box<dyn PipelineStage>> = Vec::new();
        let out = run_stages_with(none.iter().map(|b| b.as_ref()), ramp(3, 3), true);
        assert_eq!(out.width(), 3);
    }
}

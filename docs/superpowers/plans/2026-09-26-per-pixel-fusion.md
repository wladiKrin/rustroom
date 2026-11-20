# Per-Pixel Fusion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A run of consecutive per-pixel stages executes as one pass over the pixels, on the CPU (bit-identical) and on the GPU (one dispatch), without changing the stage list.

**Architecture:** `pipeline::per_pixel` adds a `PerPixel` trait the twelve pixel-in/pixel-out stages implement, and a runner that folds a run of them into one in-place walk; `Pipeline::run`, `MaskStage` and `GpuChain::cpu_fallback` call it. `gpu_pipeline::per_pixel` adds a `GpuPerPixel` description (a WGSL function, its parameters, its tables) each twin provides, an emitter that concatenates a run into one shader with three bindings, and `next_unit`, which `GpuChain::run_with` and `GpuMaskStage::encode` walk. Grouping is decided at run time; the builders, keys and every `stage_names()` test are untouched. `RUSTROOM_NO_FUSION=1` makes every run a run of one on both backends.

**Tech Stack:** Rust, rayon, wgpu 29 / WGSL. Spec: `docs/superpowers/specs/2026-09-26-per-pixel-fusion-design.md`. Measured basis: a full-frame per-pixel pass is ~2.2 ms at 6.8 Mpx (`grep -n 'GPU timestamps' docs/JOURNAL.md`), above the spec's 1 ms stop rule.

**Conventions that apply:** no `Co-Authored-By` trailer; never push; goldens must not move on the CPU tasks (`cargo test --workspace golden`); GPU iteration with `cargo test --release -p gpu-pipeline <filter>`; the full debug gate once at the end; every GPU test skips with `SKIPPED:` when `shared()` is `None`.

---

### Task 1: `pipeline::per_pixel` — the trait, the walk, the runner, the switch, the counter

**Files:**
- Create: `crates/pipeline/src/per_pixel.rs`
- Modify: `crates/pipeline/src/lib.rs` (a `pub mod per_pixel;` beside `pub mod brush;`; a `pub use per_pixel::{run_stages, PerPixel};`; a defaulted method on `PipelineStage`)

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/per_pixel.rs`:

```rust
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
        assert_ne!(want.data(), img.data(), "fixture check: the stages move pixels");
        let before = fused_walks();
        let got = run_stages_with(stages.iter().map(|b| b.as_ref()), img, true);
        assert_eq!(got.data(), want.data(), "fused must equal sequential, sample for sample");
        assert_eq!(fused_walks() - before, 2, "[plus, times_x] then [plus]: two walks");
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
        assert_ne!(seq_ab.data(), seq_ba.data(), "fixture check: the two orders differ");
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
```

In `lib.rs`, after `pub mod brush;` add:

```rust
/// A run of per-pixel stages as one pass. See the module.
pub mod per_pixel;
pub use per_pixel::{run_stages, PerPixel};
```

And on `PipelineStage`, after `input_rect_for`, add the defaulted method:

```rust
    /// This stage as a per-pixel function for a `w` x `h` buffer, or `None` when it is not one.
    ///
    /// ⚠⚠ **`None` is the default, so a stage that reads neighbours declines BY CONSTRUCTION** —
    /// local contrast, sharpening, every prefix stage and a mask inherit it, and the runner then
    /// applies them on their own. The twelve pixel-in/pixel-out stages return `Some(self)`; the
    /// vignette builds a value for these dimensions, because its frame is the buffer's when there
    /// is no region.
    ///
    /// ⚠ Boxed because the vignette's answer is built per call; the other eleven box a borrow,
    /// which is one small allocation per stage per walk.
    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn per_pixel::PerPixel + '_>> {
        None
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test --workspace per_pixel::tests`
Expected: compile errors until the module exists; then, since the module carries its own toys, `4 passed`. (The failing state is the missing module; the point of the step is that the tests exist before the runner is wired anywhere.)

- [ ] **Step 3: Run them to see them pass**

Run: `cargo test --workspace per_pixel::tests`
Expected: `4 passed`.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/src/per_pixel.rs crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): per_pixel — the PerPixel trait, one in-place walk, and a runner that folds a run"
```

---

### Task 2: The twelve CPU stages implement `PerPixel`, and `apply` becomes the one walk

**Files:**
- Modify: `crates/pipeline/src/stages/{white_balance,exposure,contrast,tone_regions,tone_curve,saturation,hsl,color_grading,vignette,display_rendering,creative_look,output_transform}.rs`

Each change is the same shape: move the closure body into `fn px`, make `apply` call `crate::per_pixel::apply_one(self, image)`, and add `per_pixel` returning `Some(Box::new(self))`. The arithmetic is copied, not rewritten; where a stage used `map_samples`, `px` applies the same per-sample expression to the three channels in order.

- [ ] **Step 1: white_balance.rs**

Replace the `impl PipelineStage for WhiteBalanceStage` block with:

```rust
impl crate::per_pixel::PerPixel for WhiteBalanceStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let [gr, gg, gb] = self.gains;
        [r * gr, g * gg, b * gb]
    }
}

impl PipelineStage for WhiteBalanceStage {
    fn name(&self) -> &'static str {
        "white_balance"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 2: exposure.rs**

```rust
impl crate::per_pixel::PerPixel for ExposureStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        // ⚠ The same per-sample expression `map_samples` applied, to the three channels in order.
        [p[0] * self.factor, p[1] * self.factor, p[2] * self.factor]
    }
}

impl PipelineStage for ExposureStage {
    fn name(&self) -> &'static str {
        "exposure"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 3: contrast.rs**

```rust
impl crate::per_pixel::PerPixel for ContrastStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        let slope = self.slope;
        let f = |s: f32| (PIVOT + (s - PIVOT) * slope).max(0.0);
        [f(p[0]), f(p[1]), f(p[2])]
    }
}

impl PipelineStage for ContrastStage {
    fn name(&self) -> &'static str {
        "contrast"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 4: tone_regions.rs**

```rust
impl crate::per_pixel::PerPixel for ToneRegionsStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let l = color::luminance([r, g, b]);
        let gain = self.gain_for_luma(l).max(0.0);
        [r * gain, g * gain, b * gain]
    }
}

impl PipelineStage for ToneRegionsStage {
    fn name(&self) -> &'static str {
        "tone_regions"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 5: tone_curve.rs**

```rust
impl crate::per_pixel::PerPixel for ToneCurveStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        match &self.channels {
            // ⚠ The common path: one table over every sample, exactly `map_samples`' expression.
            None => [self.sample(p[0]), self.sample(p[1]), self.sample(p[2])],
            Some(ch) => std::array::from_fn(|c| sample_table(&ch[c], self.sample(p[c]))),
        }
    }
}

impl PipelineStage for ToneCurveStage {
    fn name(&self) -> &'static str {
        "tone_curve"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 6: saturation.rs**

```rust
impl crate::per_pixel::PerPixel for SaturationStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let (f_sat, vib) = (self.f_sat, self.vib);
        // ⚠ The pivot chroma is scaled around: real luminance in the working space.
        let luma = color::luminance([r, g, b]);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        // Current chroma in [0, 1]; 0 for neutrals, 1 for a fully saturated color.
        let chroma = ((max - min) / max.max(EPS)).clamp(0.0, 1.0);
        let f = f_sat * (1.0 + vib * (1.0 - chroma));
        [
            (luma + (r - luma) * f).max(0.0),
            (luma + (g - luma) * f).max(0.0),
            (luma + (b - luma) * f).max(0.0),
        ]
    }
}

impl PipelineStage for SaturationStage {
    fn name(&self) -> &'static str {
        "saturation"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 7: hsl.rs**

Replace `impl PipelineStage for HslStage` with:

```rust
impl crate::per_pixel::PerPixel for HslStage {
    fn px(&self, _x: usize, _y: usize, rgb: [f32; 3]) -> [f32; 3] {
        let table = &self.table;
        let [l, a, b] = prophoto_to_oklab(rgb);
        let chroma = (a * a + b * b).sqrt();
        if chroma < EPS {
            return rgb; // neutral — nothing to band-match
        }
        let hue = {
            let d = b.atan2(a).to_degrees();
            if d < 0.0 {
                d + 360.0
            } else {
                d
            }
        };

        let [mut dh, mut sat, mut lum] = table.sample(hue);

        // Fade the whole adjustment out as chroma approaches zero, where hue is noise.
        let t = (chroma / CHROMA_FADE).min(1.0);
        let strength = t * t * (3.0 - 2.0 * t); // smoothstep
        dh *= strength;
        sat *= strength;
        lum *= strength;

        // ⚠⚠ `cos(hue)` and `sin(hue)` are already in hand — they are `a / chroma` and
        // `b / chroma`. Expanding `cos(hue + δ)` and multiplying by the new chroma, the
        // chroma CANCELS, so the rotation costs two small-angle polynomials and four
        // multiplies instead of two libm calls, and it never performs the division the
        // naive form of this identity would need. It also drops the `atan2 -> cos` round
        // trip, so `hue` survives only as an index into the band model.
        let delta = (dh * MAX_HUE_SHIFT_DEG / 100.0).to_radians();
        let (cd, sd) = (cos_small(delta), sin_small(delta));
        // ⚠ Unreachable given the ±100 slider range, which puts `k` in [0, 2]: defence in
        // depth, and an unreachable guard is an untested one.
        let k = (1.0 + sat / 100.0).max(0.0);
        let l2 = (l * (1.0 + lum / 200.0)).max(0.0);

        let out = oklab_to_prophoto([l2, k * (a * cd - b * sd), k * (a * sd + b * cd)]);
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    }
}

impl PipelineStage for HslStage {
    fn name(&self) -> &'static str {
        "hsl"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 8: color_grading.rs**

```rust
impl crate::per_pixel::PerPixel for ColorGradingStage {
    fn px(&self, _x: usize, _y: usize, rgb: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let [da, db, dl] = self.grade_at(l);
        let out = oklab_to_prophoto([l + dl, a + da, b + db]);
        // Clamped at 0 only, matching `SaturationStage` and `HslStage`. Never at 1: the
        // output transform is the only place clipping belongs.
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    }
}

impl PipelineStage for ColorGradingStage {
    fn name(&self) -> &'static str {
        "color_grading"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 9: vignette.rs**

The vignette's per-pixel function needs the frame, which without a region is the buffer's own size — so `per_pixel` builds a value for `(w, h)`. Add above `impl PipelineStage for VignetteStage`:

```rust
/// The vignette's per-pixel function, resolved against one frame.
///
/// ⚠ Built per walk by [`VignetteStage::per_pixel`] because the frame is the buffer's own size
/// when there is no region, and the buffer is only known when the walk starts — the same reason
/// `Shape` is built in `apply` rather than in `new`.
struct VignettePx {
    shape: Shape,
    e0: f32,
    e1: f32,
    k: f32,
    hw: f32,
    hh: f32,
    x0: usize,
    y0: usize,
}

impl VignetteStage {
    /// The function for a `w` x `h` buffer, or `None` when the stage would change nothing.
    fn prepare(&self, w: usize, h: usize) -> Option<VignettePx> {
        if w == 0 || h == 0 || !self.params.is_active() {
            return None;
        }
        // The frame the falloff is measured against, and where this buffer sits in it.
        // Without a region the buffer *is* the frame — the existing behaviour, exactly.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };
        // ⚠ Through `vignette_params` rather than inline, so this path IS the reference the GPU
        // stage reads rather than merely agreeing with it.
        let vp = vignette_params(&self.params, fw, fh);
        Some(VignettePx {
            shape: Shape {
                wu: vp.wu,
                wv: vp.wv,
                expo: vp.expo,
                elliptical: vp.elliptical,
            },
            e0: vp.e0,
            e1: vp.e1,
            k: vp.k,
            hw: fw as f32 * 0.5,
            hh: fh as f32 * 0.5,
            x0,
            y0,
        })
    }
}

impl crate::per_pixel::PerPixel for VignettePx {
    fn px(&self, x: usize, y: usize, rgb: [f32; 3]) -> [f32; 3] {
        let u = ((x + self.x0) as f32 + 0.5) / self.hw - 1.0;
        let v = ((y + self.y0) as f32 + 0.5) / self.hh - 1.0;
        let f = self.k * smoothstep(self.e0, self.e1, self.shape.distance(u, v)); // -1 … 1
        if f <= 0.0 {
            // Squared: 1 at f = 0, still exactly 0 at f = -1, but about twice the stops
            // at every point in between (f = -0.5 is -2 EV, not -1).
            let gain = (1.0 + f) * (1.0 + f);
            [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
        } else {
            // Toward white, squared about *its* endpoint so the two halves are mirror
            // images: equal |amount| moves a pixel equally far toward black and toward
            // white. `max(0.0)` leaves scene-referred samples already above 1.0 alone
            // rather than dragging them back down.
            let s = 1.0 - (1.0 - f) * (1.0 - f);
            let lift = |v: f32| v + (1.0 - v).max(0.0) * s;
            [lift(rgb[0]), lift(rgb[1]), lift(rgb[2])]
        }
    }
}
```

and replace the `impl PipelineStage for VignetteStage` block with:

```rust
impl PipelineStage for VignetteStage {
    fn name(&self) -> &'static str {
        "vignette"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        match self.prepare(image.width(), image.height()) {
            Some(px) => crate::per_pixel::apply_one(&px, image),
            None => image,
        }
    }

    fn per_pixel(&self, w: usize, h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        self.prepare(w, h)
            .map(|px| Box::new(px) as Box<dyn crate::per_pixel::PerPixel + '_>)
    }
}
```

(`Shape` needs `#[derive(Clone, Copy)]` if it is not already; add it.)

- [ ] **Step 10: display_rendering.rs**

```rust
impl crate::per_pixel::PerPixel for DisplayRenderingStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        // ⚠⚠ **Per channel, not on luminance with a scalar gain.** Every other tonal stage here
        // preserves hue deliberately; this one must not. Curving the channels independently is
        // where a camera rendering's shadow saturation and its roll toward white come from, and
        // it is what the DNG spec does with `ProfileToneCurve`. A neutral grey still stays
        // exactly neutral, because all three channels take the same curve.
        let p = [self.map(r), self.map(g), self.map(b)];
        // ⚠ The curve alone lands 0.945× the camera's saturation; [`CHROMA_BOOST`] is the
        // measured remainder. Scaled about the pixel's own luminance, so the *tone* the curve
        // just decided is left exactly alone and only the colour opens up.
        let y = color::luminance(p);
        let s = Self::chroma_scale(y);
        // ⚠ Clamped: a bright saturated pixel scaled up can leave [0, 1], and letting one
        // channel clip while its neighbours do not would shift the hue — the one thing this
        // stage's per-channel curve is careful to do only where the *curve* says so.
        p.map(|c| (y + (c - y) * s).clamp(0.0, 1.0))
    }
}

impl PipelineStage for DisplayRenderingStage {
    fn name(&self) -> &'static str {
        "display_rendering"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 11: creative_look.rs**

```rust
impl crate::per_pixel::PerPixel for CreativeLookStage {
    fn px(&self, _x: usize, _y: usize, px: [f32; 3]) -> [f32; 3] {
        let t = self.blend;
        // ⚠⚠ **Short-circuit at 0, and it is a correctness guard rather than a speed one.**
        // The shaper clamps out-of-gamut values, so running it at strength 0 and then
        // lerping back would leave a residue on saturated colours — a look dialled to zero
        // that is not quite no look. `strength_zero_is_bit_exactly_the_input` is the guard.
        if t == 0.0 {
            return px;
        }
        let out = self.look(px);
        // ⚠ `a + (b - a) * t`, so `t = 1` returns `b` exactly and `t = 0` returns `a`
        // exactly. `a*(1-t) + b*t` does neither in f32.
        [
            px[0] + (out[0] - px[0]) * t,
            px[1] + (out[1] - px[1]) * t,
            px[2] + (out[2] - px[2]) * t,
        ]
    }
}

impl PipelineStage for CreativeLookStage {
    fn name(&self) -> &'static str {
        "creative_look"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 12: output_transform.rs**

```rust
impl crate::per_pixel::PerPixel for OutputTransform {
    fn px(&self, _x: usize, _y: usize, c: [f32; 3]) -> [f32; 3] {
        // ⚠ Two steps, and the order is not negotiable: primaries first, in linear light, then
        // the transfer curve. Encoding before converting would apply a matrix to gamma-encoded
        // values, which is meaningless and does not fail loudly.
        //
        // ⚠ Out-of-gamut colour arrives here with negative components — that is the point of a
        // wide working space, not a bug. `encode` clamps them.
        let lin = self.space.from_working(c);
        [
            self.space.encode(lin[0]),
            self.space.encode(lin[1]),
            self.space.encode(lin[2]),
        ]
    }
}

impl PipelineStage for OutputTransform {
    fn name(&self) -> &'static str {
        "output_transform"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}
```

- [ ] **Step 13: Build and run the goldens and the stage tests**

Run: `cargo test --workspace golden`
Expected: 78 goldens pass. If any moves, one stage's `px` is not the closure it replaced; diff that stage's `px` against the pre-task closure and fix it, never the golden.

Run: `cargo test --workspace stages::`
Expected: all pass.

- [ ] **Step 14: Commit**

```bash
git add crates/pipeline/src/stages
git commit -m "feat(pipeline): the twelve per-pixel stages implement PerPixel; apply is the one walk"
```

---

### Task 3: The runner reaches the pipeline, the masks and the GPU fallback; the property test

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (`Pipeline::run`; tests)
- Modify: `crates/pipeline/src/stages/mask.rs` (the sub-stage loop)
- Modify: `crates/gpu-pipeline/src/chain.rs` (`cpu_fallback`)

- [ ] **Step 1: Write the failing property test**

Append to `lib.rs`'s tests module (it already has helpers for building stages; if not, the constructors below are the public ones):

```rust
    /// ⚠⚠ Every per-pixel stage at once, fused against sequential, sample for sample. A stage
    /// whose `px` drifts from the closure it replaced fails here even if no golden carries it.
    #[test]
    fn a_fused_run_equals_the_sequential_application() {
        use edit_model::{ColorGrading, GradeWheel, Hsl, HslBand, Vignette};
        let cube = std::sync::Arc::new(
            crate::lut::Cube::parse(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0.1\n0 1 0\n1 1 0.2\n0 0.1 1\n1 0 1\n0 1 1\n0.9 1 1\n",
            )
            .expect("a 2^3 cube"),
        );
        let mut hsl = Hsl::default();
        hsl.bands[0] = HslBand {
            hue: 20.0,
            saturation: 30.0,
            luminance: -10.0,
        };
        let grading = ColorGrading {
            shadows: GradeWheel {
                hue: 200.0,
                saturation: 40.0,
                luminance: -5.0,
            },
            highlights: GradeWheel {
                hue: 40.0,
                saturation: 30.0,
                luminance: 5.0,
            },
            ..ColorGrading::default()
        };
        let stages: Vec<Box<dyn PipelineStage>> = vec![
            Box::new(WhiteBalanceStage::new([1.1, 0.95, 1.05])),
            Box::new(ExposureStage::new(0.7)),
            Box::new(ContrastStage::new(25.0)),
            Box::new(ToneRegionsStage::new(-20.0, 30.0, 10.0, -15.0)),
            Box::new(
                ToneCurveStage::new(&[[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]]).with_channels(
                    &[[0.0, 0.0], [0.5, 0.45], [1.0, 1.0]],
                    &[],
                    &[[0.0, 0.05], [1.0, 1.0]],
                ),
            ),
            Box::new(SaturationStage::new(20.0, 15.0)),
            Box::new(HslStage::new(&hsl)),
            Box::new(ColorGradingStage::new(&grading)),
            Box::new(VignetteStage::new(&Vignette {
                amount: -40.0,
                ..Vignette::default()
            })),
            Box::new(DisplayRenderingStage::new(None, None)),
            Box::new(CreativeLookStage::new(
                cube,
                edit_model::LutSpace::SrgbDisplay,
                60.0,
            )),
            Box::new(OutputTransform::new(color::OutputSpace::Srgb)),
        ];
        for s in &stages {
            assert!(
                s.per_pixel(8, 8).is_some(),
                "{} must be per-pixel for this test to mean anything",
                s.name()
            );
        }
        let mut img = ImageBuf::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                let t = (x * 7 + y * 3) as f32 / 97.0;
                img.set_pixel(x, y, [t * 0.9, (1.0 - t) * 0.7 + 0.05, (t * t) * 1.3]);
            }
        }
        let want = stages.iter().fold(img.clone(), |i, s| s.apply(i));
        assert_ne!(want.data(), img.data(), "fixture check: the stages move pixels");
        let got = per_pixel::run_stages_with(stages.iter().map(|b| b.as_ref()), img, true);
        let bad = got
            .data()
            .iter()
            .zip(want.data())
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(bad, None, "the fused run diverged at sample {bad:?}");
    }
```

Check the field names against `edit_model` before running (`Vignette`, `GradeWheel`, `HslBand`, `LutSpace`) and `crate::lut::Cube::parse`'s exact signature; adjust the literal, not the assertion.

- [ ] **Step 2: Run it to see it pass or fail**

Run: `cargo test --workspace a_fused_run_equals_the_sequential_application`
Expected: PASS already, because Task 2 made every `apply` the same walk. It exists to fail the day a `px` drifts.

- [ ] **Step 3: Wire the three callers**

In `lib.rs`, `Pipeline::run`:

```rust
    /// Run all stages in order.
    ///
    /// ⚠ Through [`per_pixel::run_stages`], so a run of per-pixel stages is one walk. The list
    /// itself is untouched; see that module.
    pub fn run(&self, image: ImageBuf) -> ImageBuf {
        per_pixel::run_stages(self.stages.iter().map(|s| s.as_ref()), image)
    }
```

In `mask.rs`, replace

```rust
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }
```

with

```rust
        // ⚠ Through the fused runner, so a mask's own per-pixel sub-stages are one walk over the
        // copy — the free part of fusion. The copy and the blend are unchanged.
        adjusted = crate::per_pixel::run_stages(self.stages.iter().map(|s| s.as_ref()), adjusted);
```

In `gpu-pipeline/src/chain.rs`, `cpu_fallback`:

```rust
    fn cpu_fallback(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ Every stage, not some — a partially-GPU result would be a third rendering path
        // nobody designed. Through the fused runner, like every other CPU list.
        pipeline::run_stages(self.stages.iter().map(|s| s.cpu()), image)
    }
```

- [ ] **Step 4: The walk observable through `Pipeline::run`**

Append to `lib.rs` tests:

```rust
    /// `Pipeline::run` takes one walk for a run of three per-pixel stages.
    #[test]
    fn a_pipeline_of_per_pixel_stages_takes_one_fused_walk() {
        if !per_pixel::fusion_enabled() {
            eprintln!("SKIPPED: RUSTROOM_NO_FUSION is set");
            return;
        }
        let p = Pipeline::new(vec![
            Box::new(ExposureStage::new(0.5)),
            Box::new(ContrastStage::new(10.0)),
            Box::new(SaturationStage::new(10.0, 0.0)),
        ]);
        let before = per_pixel::fused_walks();
        let _ = p.run(ImageBuf::new(8, 8));
        assert_eq!(per_pixel::fused_walks() - before, 1);
    }
```

- [ ] **Step 5: Goldens and the whole pipeline crate**

Run: `cargo test --workspace golden`
Expected: 78 pass, none moved.

Run: `cargo test --workspace pipeline`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/pipeline/src/stages/mask.rs crates/gpu-pipeline/src/chain.rs
git commit -m "feat(pipeline): Pipeline::run, a mask's sub-stages and the GPU fallback run through the fused walk"
```

---

### Task 4: `gpu_pipeline::per_pixel` — parts, the emitter, units

**Files:**
- Create: `crates/gpu-pipeline/src/per_pixel.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs` (`mod per_pixel;`)
- Modify: `crates/gpu-pipeline/src/chain.rs` (`GpuStage::per_pixel` default)

- [ ] **Step 1: Write the pure tests and the module**

Create `crates/gpu-pipeline/src/per_pixel.rs`:

```rust
//! A run of per-pixel GPU twins as ONE dispatch: the GPU side of `pipeline::per_pixel`.
//!
//! Each per-pixel twin describes itself as a [`Part`]: a WGSL function `apply_<name>` with its
//! `Params_<name>` struct, its parameter bytes, and its tables. [`emit`] concatenates a run into
//! one shader with **three bindings whatever the run's length** — the image, one uniform holding
//! a header and every member's parameters at 16-byte-aligned offsets, and one storage buffer
//! holding every member's tables — and a `main` that folds a pixel through the members in order.
//!
//! ⚠⚠ **A lone per-pixel twin is a run of one through the same emitter**, so there is one
//! definition of every stage's shader and the per-stage equivalence tests exercise this code.
//!
//! ⚠ The program cache keys on the shader text, which here is the run's shape plus its table
//! offsets, so a drag compiles once.

use crate::dispatch::Dispatch;
use crate::{GpuContext, GpuImage, GpuStage};

/// Which shared preludes a part's function needs.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Preludes {
    pub srgb: bool,
    pub luma: bool,
    pub oklab: bool,
}

/// One per-pixel twin, as the emitter needs it.
pub struct Part<'a> {
    /// The stage's name; the struct is `Params_<name>` and the function `apply_<name>`.
    pub name: &'static str,
    /// WGSL declaring `struct Params_<name>` and
    /// `fn apply_<name>(p: vec3<f32>, x: u32, y: u32, q: Params_<name>, base: u32) -> vec3<f32>`,
    /// plus any helpers, every one of them suffixed with the stage's name.
    pub wgsl: String,
    /// The parameters, laid out as `Params_<name>`. ⚠ A multiple of 16 bytes, checked by `emit`.
    pub params: Vec<u8>,
    /// The tables, in the order the function indexes them from `base`.
    pub tables: Vec<&'a [f32]>,
    pub preludes: Preludes,
    /// Whether the function reads `x` or `y`. Only the vignette does.
    pub needs_position: bool,
}

/// What `emit` produces: the shader, the uniform bytes, the concatenated tables.
pub(crate) struct Emitted {
    pub shader: String,
    pub params: Vec<u8>,
    pub tables: Vec<f32>,
}

/// The header every fused shader starts its uniform with.
const HEADER_BYTES: usize = 16;

/// Concatenate `parts` into one shader, one uniform and one table buffer.
///
/// ⚠ Pure, so the layout — offsets, bases, order — is tested with no GPU.
pub(crate) fn emit(parts: &[Part<'_>], pixels: u32, row_groups: u32, w: u32) -> Emitted {
    let mut preludes = Preludes::default();
    let mut needs_position = false;
    let mut params = Vec::with_capacity(
        HEADER_BYTES + parts.iter().map(|p| p.params.len()).sum::<usize>(),
    );
    params.extend_from_slice(&pixels.to_le_bytes());
    params.extend_from_slice(&row_groups.to_le_bytes());
    params.extend_from_slice(&w.to_le_bytes());
    params.extend_from_slice(&0u32.to_le_bytes());
    let mut tables: Vec<f32> = Vec::new();
    let mut members = String::new();
    let mut bodies = String::new();
    let mut calls = String::new();
    for (k, part) in parts.iter().enumerate() {
        assert!(
            !part.params.is_empty() && part.params.len() % 16 == 0,
            "{}'s params are {} bytes, not a positive multiple of 16",
            part.name,
            part.params.len()
        );
        preludes.srgb |= part.preludes.srgb;
        preludes.luma |= part.preludes.luma;
        preludes.oklab |= part.preludes.oklab;
        needs_position |= part.needs_position;
        let base = tables.len();
        for t in &part.tables {
            tables.extend_from_slice(t);
        }
        params.extend_from_slice(&part.params);
        members.push_str(&format!("    s{k}: Params_{},\n", part.name));
        bodies.push_str(&part.wgsl);
        bodies.push('\n');
        calls.push_str(&format!(
            "    p = apply_{}(p, x, y, P.s{k}, {base}u);\n",
            part.name
        ));
    }
    // ⚠ wgpu refuses a zero-sized buffer, the guard `Dispatch` and `GpuImage::scratch` carry.
    if tables.is_empty() {
        tables.push(0.0);
    }
    let position = if needs_position {
        "    let x = pixel % P.hdr.w;\n    let y = pixel / P.hdr.w;\n"
    } else {
        "    let x = 0u;\n    let y = 0u;\n"
    };
    let shader = format!(
        "{srgb}{luma}{oklab}{bodies}
struct Header {{
    pixels: u32,
    row_groups: u32,
    w: u32,
    pad: u32,
}};
struct Params {{
    hdr: Header,
{members}}};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> P: Params;
@group(0) @binding(2) var<storage, read> tables: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let pixel = id.y * (P.hdr.row_groups * 64u) + id.x;
    if (pixel >= P.hdr.pixels) {{ return; }}
{position}    let i = pixel * 3u;
    var p = vec3<f32>(data[i], data[i + 1u], data[i + 2u]);
{calls}    data[i] = p.x;
    data[i + 1u] = p.y;
    data[i + 2u] = p.z;
}}
",
        srgb = if preludes.srgb {
            crate::srgb_wgsl::SRGB_WGSL
        } else {
            ""
        },
        luma = if preludes.luma {
            crate::luma_wgsl::luma_wgsl()
        } else {
            String::new()
        },
        oklab = if preludes.oklab {
            crate::oklab_wgsl::OKLAB_WGSL
        } else {
            ""
        },
    );
    Emitted {
        shader,
        params,
        tables,
    }
}

/// Encode one fused dispatch over `parts`.
pub(crate) fn encode_parts(
    parts: &[Part<'_>],
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
) {
    let pixels = img.pixels();
    let (row_groups, _) = Dispatch::grid(pixels);
    let e = emit(parts, pixels, row_groups, img.width() as u32);
    Dispatch {
        label: "per_pixel",
        shader: e.shader,
        params: e.params,
        storages: &[&e.tables],
    }
    .encode(ctx, pass, img);
}

/// A per-pixel twin's own `encode`: a run of one through the emitter.
///
/// ⚠ What every per-pixel twin's `GpuStage::encode` calls, so the trait stays total and no twin
/// keeps a standalone shader of its own.
pub(crate) fn encode_single(
    stage: &dyn GpuStage,
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
) {
    let part = stage
        .per_pixel(img.width(), img.height())
        .expect("encode_single is only for a per-pixel twin");
    encode_parts(std::slice::from_ref(&part), ctx, pass, img);
}

/// Whether the environment switched fusion off. Read once.
pub fn fusion_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTROOM_NO_FUSION").is_none())
}

/// One thing a chain encodes: a fused run of per-pixel twins, or one other stage.
pub(crate) enum Unit<'a> {
    Fused(Vec<Part<'a>>),
    Single(&'a dyn GpuStage),
}

impl Unit<'_> {
    /// The names, joined — what a timed run labels its pass with.
    pub(crate) fn label(&self) -> String {
        match self {
            Unit::Fused(parts) => parts
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join("+"),
            Unit::Single(s) => s.name().to_owned(),
        }
    }

    /// This unit's output size. A fused run never changes it.
    pub(crate) fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        match self {
            Unit::Fused(_) => (w, h),
            Unit::Single(s) => s.output_dims(w, h),
        }
    }

    pub(crate) fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        match self {
            Unit::Fused(parts) => encode_parts(parts, ctx, pass, img),
            Unit::Single(s) => s.encode(ctx, pass, img, scratch),
        }
    }
}

/// The unit starting at `stages[i]` for a `w` x `h` image, and the index after it.
///
/// ⚠ Decided at the current dimensions, because a size-changing stage earlier in the chain may
/// have relabelled the image, and a part is built for the size it will run at. With `fuse` off,
/// a per-pixel twin is a fused run of one — the same emitter, so the A/B measures the grouping
/// and nothing else.
pub(crate) fn next_unit<'a>(
    stages: &'a [Box<dyn GpuStage>],
    i: usize,
    w: usize,
    h: usize,
    fuse: bool,
) -> (Unit<'a>, usize) {
    match stages[i].per_pixel(w, h) {
        None => (Unit::Single(stages[i].as_ref()), i + 1),
        Some(first) => {
            let mut parts = vec![first];
            let mut j = i + 1;
            if fuse {
                while j < stages.len() {
                    match stages[j].per_pixel(w, h) {
                        Some(p) => {
                            parts.push(p);
                            j += 1;
                        }
                        None => break,
                    }
                }
            }
            (Unit::Fused(parts), j)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &'static str, params: Vec<u8>, table: &[f32]) -> Part<'_> {
        Part {
            name,
            wgsl: format!("struct Params_{name} {{ a: f32, b: f32, c: f32, d: f32 }};\nfn apply_{name}(p: vec3<f32>, x: u32, y: u32, q: Params_{name}, base: u32) -> vec3<f32> {{ return p; }}"),
            params,
            tables: vec![table],
            preludes: Preludes::default(),
            needs_position: false,
        }
    }

    /// The header first, then each member's bytes at the next 16-byte boundary; tables laid down
    /// in order with each member's base passed as a literal; members called in list order.
    #[test]
    fn emit_lays_out_params_tables_and_calls_in_order() {
        let ta = [1.0f32, 2.0, 3.0];
        let tb = [7.0f32];
        let parts = [part("aa", vec![1u8; 16], &ta), part("bb", vec![2u8; 32], &tb)];
        let e = emit(&parts, 100, 2, 10);
        assert_eq!(e.params.len(), 16 + 16 + 32);
        assert_eq!(&e.params[0..4], &100u32.to_le_bytes());
        assert_eq!(&e.params[4..8], &2u32.to_le_bytes());
        assert_eq!(&e.params[8..12], &10u32.to_le_bytes());
        assert!(e.params[16..32].iter().all(|&b| b == 1));
        assert!(e.params[32..64].iter().all(|&b| b == 2));
        assert_eq!(e.tables, vec![1.0, 2.0, 3.0, 7.0]);
        let a = e.shader.find("p = apply_aa(p, x, y, P.s0, 0u);").expect("aa called first");
        let b = e.shader.find("p = apply_bb(p, x, y, P.s1, 3u);").expect("bb's base is 3");
        assert!(a < b, "members are called in list order");
        assert!(e.shader.contains("    s0: Params_aa,\n    s1: Params_bb,\n"));
        assert!(!e.shader.contains("let x = pixel %"), "no member needs position");
    }

    /// A part that needs position makes `main` derive x and y; no tables still binds one word.
    #[test]
    fn emit_derives_position_only_when_asked_and_never_binds_an_empty_table() {
        let mut p = part("vv", vec![0u8; 16], &[]);
        p.needs_position = true;
        let e = emit(std::slice::from_ref(&p), 1, 1, 1);
        assert!(e.shader.contains("let x = pixel % P.hdr.w;"));
        assert_eq!(e.tables, vec![0.0]);
    }

    /// A part whose params are not a multiple of 16 is a bug in that part, caught here.
    #[test]
    #[should_panic(expected = "not a positive multiple of 16")]
    fn emit_refuses_misaligned_params() {
        let p = part("xx", vec![0u8; 12], &[]);
        let _ = emit(std::slice::from_ref(&p), 1, 1, 1);
    }
}
```

In `lib.rs` add `mod per_pixel;` between `mod output_transform;` and `mod quantize;`, and `pub use per_pixel::{Part, Preludes};` beside the other re-exports.

In `chain.rs`, on `GpuStage` after `output_dims`:

```rust
    /// This stage as one per-pixel function for a `w` x `h` image, or `None`.
    ///
    /// ⚠⚠ **`None` is the default, so a stage that reads neighbours declines BY CONSTRUCTION** —
    /// sharpening, local contrast, NR, dehaze, lens, geometry and a mask inherit it and keep their
    /// own `encode`. The twelve per-pixel twins return a [`crate::per_pixel::Part`], and the
    /// chain folds a run of them into one dispatch; see that module.
    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        None
    }
```

- [ ] **Step 2: Run the pure tests**

Run: `cargo test --release -p gpu-pipeline per_pixel::tests`
Expected: `3 passed`.

- [ ] **Step 3: Commit**

```bash
git add crates/gpu-pipeline/src/per_pixel.rs crates/gpu-pipeline/src/lib.rs crates/gpu-pipeline/src/chain.rs
git commit -m "feat(gpu): per_pixel — parts, the fused emitter, and the units a chain walks"
```

---

### Task 5: The twelve GPU twins become parts

**Files:**
- Modify: `crates/gpu-pipeline/src/{white_balance,exposure,contrast,tone_regions,tone_curve,saturation,hsl,grading,vignette,display_rendering,creative_look,output_transform}.rs`

Each twin: its `SHADER` (or `shader()`) becomes a `Params_<name>` struct and an `apply_<name>` function with helpers suffixed; `pixels`/`row_groups` leave the params; `per_pixel` returns the `Part`; `encode` becomes `crate::per_pixel::encode_single(self, ctx, pass, img)`. Keep every `⚠` comment on the arithmetic where it was. Run the crate's tests after each file.

- [ ] **Step 1: white_balance.rs**

Replace `SHADER` and the `encode` body:

```rust
const WGSL: &str = r#"
struct Params_white_balance {
    gr: f32,
    gg: f32,
    gb: f32,
    pad0: f32,
};
fn apply_white_balance(p: vec3<f32>, x: u32, y: u32, q: Params_white_balance, base: u32) -> vec3<f32> {
    return vec3<f32>(p.x * q.gr, p.y * q.gg, p.z * q.gb);
}
"#;
```

and in `impl GpuStage`:

```rust
    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&self.gains[0].to_le_bytes());
        params[4..8].copy_from_slice(&self.gains[1].to_le_bytes());
        params[8..12].copy_from_slice(&self.gains[2].to_le_bytes());
        Some(crate::per_pixel::Part {
            name: "white_balance",
            wgsl: WGSL.to_string(),
            params: params.to_vec(),
            tables: Vec::new(),
            preludes: crate::per_pixel::Preludes::default(),
            needs_position: false,
        })
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        _scratch: &[wgpu::Buffer],
    ) {
        crate::per_pixel::encode_single(self, ctx, pass, img);
    }
```

- [ ] **Step 2: exposure.rs**

```rust
pub(crate) const WGSL: &str = r#"
struct Params_exposure {
    gain: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};
fn apply_exposure(p: vec3<f32>, x: u32, y: u32, q: Params_exposure, base: u32) -> vec3<f32> {
    return vec3<f32>(p.x * q.gain, p.y * q.gain, p.z * q.gain);
}
"#;
```

`per_pixel`: params `[factor, 0, 0, 0]` as bytes; `encode` → `encode_single`. Keep `clone_stage` and the `PipelineStage` impl as they are.

- [ ] **Step 3: contrast.rs**

```rust
fn wgsl() -> String {
    format!(
        r#"
struct Params_contrast {{
    slope: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}};
fn apply_contrast(p: vec3<f32>, x: u32, y: u32, q: Params_contrast, base: u32) -> vec3<f32> {{
    // ⚠ Floored at 0 exactly as the CPU stage does: a sample driven negative by an extreme slope
    // must darken to black, never flip the pixel's sign.
    return vec3<f32>(
        max({pivot}f + (p.x - {pivot}f) * q.slope, 0.0),
        max({pivot}f + (p.y - {pivot}f) * q.slope, 0.0),
        max({pivot}f + (p.z - {pivot}f) * q.slope, 0.0));
}}
"#,
        pivot = pipeline::CONTRAST_PIVOT
    )
}
```

`per_pixel`: params `[slope, 0, 0, 0]`.

- [ ] **Step 4: tone_regions.rs**

```rust
const WGSL: &str = r#"
struct Params_tone_regions {
    lut_len: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    top: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
};
fn apply_tone_regions(p: vec3<f32>, x: u32, y: u32, q: Params_tone_regions, base: u32) -> vec3<f32> {
    let l = prophoto_luminance(p.x, p.y, p.z);
    // ⚠⚠ Clamp the INDEX, never l itself. A scene-referred highlight above the table's top takes
    // the last entry, which is the EXACT answer rather than an approximation -- gain_for_luma
    // clamps its own input, so the gain really is flat up there.
    let t = clamp(l / q.top, 0.0, 1.0) * f32(q.lut_len - 1u);
    let i = min(u32(floor(t)), q.lut_len - 1u);
    let j = min(i + 1u, q.lut_len - 1u);
    // ⚠ Floored at 0 exactly as the CPU stage does.
    let gain = max(mix(tables[base + i], tables[base + j], t - f32(i)), 0.0);
    return vec3<f32>(p.x * gain, p.y * gain, p.z * gain);
}
"#;
```

`per_pixel`: params 32 bytes: `lut_len` at 0, `top` (`ToneRegionsStage::LUT_TOP`) at 16; `tables: vec![&self.table]`; `preludes: Preludes { luma: true, ..Default::default() }`.

- [ ] **Step 5: tone_curve.rs**

```rust
const WGSL: &str = r#"
struct Params_tone_curve {
    last: u32,
    stride: u32,
    pad0: u32,
    pad1: u32,
};
fn curve_tone_curve(v_in: f32, at: u32, last: u32) -> f32 {
    let v = clamp(v_in, 0.0, 1.0);
    let pos = v * f32(last);
    let lo = u32(floor(pos));
    if (lo >= last) { return tables[at + last]; }
    let frac = pos - f32(lo);
    return tables[at + lo] + frac * (tables[at + lo + 1u] - tables[at + lo]);
}
fn apply_tone_curve(p: vec3<f32>, x: u32, y: u32, q: Params_tone_curve, base: u32) -> vec3<f32> {
    // ⚠⚠ **Composite first, then the channel** — the order `ToneCurveStage::with_channels` documents.
    return vec3<f32>(
        curve_tone_curve(curve_tone_curve(p.x, base, q.last), base + q.stride, q.last),
        curve_tone_curve(curve_tone_curve(p.y, base, q.last), base + q.stride * 2u, q.last),
        curve_tone_curve(curve_tone_curve(p.z, base, q.last), base + q.stride * 3u, q.last));
}
"#;
```

`per_pixel`: params `[stride - 1, stride, 0, 0]` with `stride = ToneCurveStage::LUT_LEN as u32`; `tables: vec![&self.luts]`.

- [ ] **Step 6: saturation.rs**

```rust
fn wgsl() -> String {
    format!(
        r#"
struct Params_saturation {{
    f_sat: f32,
    vib: f32,
    pad0: f32,
    pad1: f32,
}};
fn apply_saturation(p: vec3<f32>, x: u32, y: u32, q: Params_saturation, base: u32) -> vec3<f32> {{
    // ⚠ The pivot chroma is scaled around: real luminance in the working space.
    let luma = prophoto_luminance(p.x, p.y, p.z);
    let mx = max(p.x, max(p.y, p.z));
    let mn = min(p.x, min(p.y, p.z));
    let chroma = clamp((mx - mn) / max(mx, {eps}f), 0.0, 1.0);
    let f = q.f_sat * (1.0 + q.vib * (1.0 - chroma));
    // ⚠ Floored at 0 exactly as the CPU stage does.
    return vec3<f32>(
        max(luma + (p.x - luma) * f, 0.0),
        max(luma + (p.y - luma) * f, 0.0),
        max(luma + (p.z - luma) * f, 0.0));
}}
"#,
        eps = pipeline::SATURATION_EPS
    )
}
```

`per_pixel`: params `[f_sat, vib, 0, 0]`; `preludes.luma = true`.

- [ ] **Step 7: hsl.rs**

```rust
const WGSL: &str = r#"
struct Params_hsl {
    table_len: u32,
    max_shift_deg: f32,
    chroma_eps: f32,
    chroma_fade: f32,
};
fn apply_hsl(p: vec3<f32>, x: u32, y: u32, q: Params_hsl, base: u32) -> vec3<f32> {
    let lab = prophoto_to_oklab(p.x, p.y, p.z);
    let bigL = lab.x; let A = lab.y; let B = lab.z;
    let chroma = sqrt(A*A + B*B);
    // ⚠ The early-out is a BRANCH, matching the CPU's `return rgb`.
    if (chroma < q.chroma_eps) { return p; }
    var hue = degrees(atan2(B, A));
    if (hue < 0.0) { hue = hue + 360.0; }
    let hx = hue * (f32(q.table_len) / 360.0);
    let i0 = u32(floor(hx)) % q.table_len;
    let i1 = (i0 + 1u) % q.table_len;
    let t = hx - floor(hx);
    var dh  = tables[base + i0*3u]      + (tables[base + i1*3u]      - tables[base + i0*3u])      * t;
    var sat = tables[base + i0*3u + 1u] + (tables[base + i1*3u + 1u] - tables[base + i0*3u + 1u]) * t;
    var lum = tables[base + i0*3u + 2u] + (tables[base + i1*3u + 2u] - tables[base + i0*3u + 2u]) * t;
    let ct = min(chroma / q.chroma_fade, 1.0);
    let strength = ct * ct * (3.0 - 2.0 * ct);
    dh = dh * strength; sat = sat * strength; lum = lum * strength;
    // ⚠ The same identity the CPU uses: the chroma cancels and the rotation costs two small-angle
    // polynomials.
    let d = radians(dh * q.max_shift_deg / 100.0);
    let d2 = d * d;
    let sd = d * (1.0 - d2 * (1.0/6.0 - d2 * (1.0/120.0)));
    let cd = 1.0 - d2 * (0.5 - d2 * (1.0/24.0 - d2 * (1.0/720.0)));
    let k = max(1.0 + sat / 100.0, 0.0);
    let L2 = max(bigL * (1.0 + lum / 200.0), 0.0);
    let A2 = k * (A * cd - B * sd);
    let B2 = k * (A * sd + B * cd);
    let out = oklab_to_prophoto(L2, A2, B2);
    return vec3<f32>(max(out.x, 0.0), max(out.y, 0.0), max(out.z, 0.0));
}
"#;
```

`per_pixel`: params 16 bytes: `table_len` (u32), `MAX_HUE_SHIFT_DEG`, `CHROMA_EPS`, `CHROMA_FADE`; `tables: vec![&self.table]`; `preludes.oklab = true`.

- [ ] **Step 8: grading.rs**

```rust
const WGSL: &str = r#"
struct Params_color_grading {
    lut_len: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
fn apply_color_grading(p: vec3<f32>, x: u32, y: u32, q: Params_color_grading, base: u32) -> vec3<f32> {
    let lab = prophoto_to_oklab(p.x, p.y, p.z);
    // ⚠⚠ clamp the INDEX, not lab.x -- see the file's doc comment.
    let t = clamp(lab.x, 0.0, 1.0) * f32(q.lut_len - 1u);
    let i = min(u32(floor(t)), q.lut_len - 1u);
    let j = min(i + 1u, q.lut_len - 1u);
    let f = t - f32(i);
    let da = mix(tables[base + i * 3u],      tables[base + j * 3u],      f);
    let db = mix(tables[base + i * 3u + 1u], tables[base + j * 3u + 1u], f);
    let dl = mix(tables[base + i * 3u + 2u], tables[base + j * 3u + 2u], f);
    let out = oklab_to_prophoto(lab.x + dl, lab.y + da, lab.z + db);
    // ⚠ Clamped at 0 only, never at 1: the output transform is the only place clipping belongs.
    return vec3<f32>(max(out.x, 0.0), max(out.y, 0.0), max(out.z, 0.0));
}
"#;
```

`per_pixel`: name `"color_grading"`; params `[lut_len, 0, 0, 0]`; `tables: vec![&self.table]`; `preludes.oklab = true`.

- [ ] **Step 9: vignette.rs**

```rust
const WGSL: &str = r#"
struct Params_vignette {
    x0: u32,
    y0: u32,
    elliptical: u32,
    pad0: u32,
    hw: f32,
    hh: f32,
    wu: f32,
    wv: f32,
    expo: f32,
    e0: f32,
    e1: f32,
    k: f32,
};
fn apply_vignette(p: vec3<f32>, x: u32, y: u32, q: Params_vignette, base: u32) -> vec3<f32> {
    let u = (f32(x + q.x0) + 0.5) / q.hw - 1.0;
    let v = (f32(y + q.y0) + 0.5) / q.hh - 1.0;
    let du = u * q.wu;
    let dv = v * q.wv;
    var d: f32;
    if (q.elliptical != 0u) {
        d = sqrt(du * du + dv * dv);
    } else {
        d = pow(pow(abs(du), q.expo) + pow(abs(dv), q.expo), 1.0 / q.expo);
    }
    let t = clamp((d - q.e0) / (q.e1 - q.e0), 0.0, 1.0);
    let f = q.k * (t * t * (3.0 - 2.0 * t));
    if (f <= 0.0) {
        // Squared about its own endpoint: exactly 0 at f = -1, and f = -0.5 is -2 EV.
        let gain = (1.0 + f) * (1.0 + f);
        return vec3<f32>(p.x * gain, p.y * gain, p.z * gain);
    }
    // ⚠⚠ `max(1.0 - c, 0.0)` is load-bearing: a sample already above 1.0 is left alone.
    let s = 1.0 - (1.0 - f) * (1.0 - f);
    return vec3<f32>(
        p.x + max(1.0 - p.x, 0.0) * s,
        p.y + max(1.0 - p.y, 0.0) * s,
        p.z + max(1.0 - p.z, 0.0) * s);
}
"#;
```

`per_pixel(w, h)`: resolve `(fw, fh, x0, y0)` from the region or `(w, h, 0, 0)`, `vp = pipeline::vignette_params(&self.params, fw, fh)`, params 48 bytes in the struct's order (`x0, y0, elliptical, 0, hw, hh, wu, wv, expo, e0, e1, k`), `needs_position: true`. ⚠ Return `None` when `!self.params.is_active()` or `w == 0 || h == 0`, matching the CPU's early return; the chain then encodes nothing for it through `Unit::Single` → `encode`, which must therefore also do nothing in that case: make `encode` early-return when `per_pixel` is `None`.

- [ ] **Step 10: display_rendering.rs**

```rust
const WGSL: &str = r#"
struct Params_display_rendering {
    lut_len: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    lut_max: f32,
    chroma_boost: f32,
    shadow_knee: f32,
    pad3: f32,
};
// ⚠ Mirrors `DisplayRenderingStage::map` exactly, including both of its total-function edges.
fn curve_display_rendering(x: f32, q: Params_display_rendering, base: u32) -> f32 {
    let n = q.lut_len;
    if (!(x > 0.0)) { return tables[base]; }          // ⚠ catches NaN too, unlike `x <= 0.0`
    let pos = min(x / q.lut_max, 1.0) * f32(n - 1u);
    let i = u32(floor(pos));
    if (i >= n - 1u) { return tables[base + n - 1u]; }
    return tables[base + i] + (pos - f32(i)) * (tables[base + i + 1u] - tables[base + i]);
}
fn apply_display_rendering(p: vec3<f32>, x: u32, y: u32, q: Params_display_rendering, base: u32) -> vec3<f32> {
    let o = vec3<f32>(
        curve_display_rendering(p.x, q, base),
        curve_display_rendering(p.y, q, base),
        curve_display_rendering(p.z, q, base));
    let yl = prophoto_luminance(o.x, o.y, o.z);
    let yc = clamp(yl, 0.0, 1.0);
    let shadow = min(yc / q.shadow_knee, 1.0);
    let s = 1.0 + (q.chroma_boost - 1.0) * shadow * (1.0 - yc * yc * yc * yc);
    // ⚠ Clamped: a bright saturated pixel scaled up can leave [0,1].
    return vec3<f32>(
        clamp(yl + (o.x - yl) * s, 0.0, 1.0),
        clamp(yl + (o.y - yl) * s, 0.0, 1.0),
        clamp(yl + (o.z - yl) * s, 0.0, 1.0));
}
"#;
```

`per_pixel`: params 32 bytes (`lut_len` at 0; `lut_max`, `CHROMA_BOOST`, `SHADOW_KNEE` at 16, 20, 24); `tables: vec![&self.table]`; `preludes.luma = true`.

- [ ] **Step 11: creative_look.rs**

Rename the shader's helpers with the `_creative_look` suffix and the constants to `PP_TO_SRGB_LOOK`/`SRGB_TO_PP_LOOK`; the struct:

```
struct Params_creative_look {
    size: u32,
    space: u32,
    pad0: u32,
    pad1: u32,
    dmin: vec3<f32>,
    blend: f32,
    dmax: vec3<f32>,
    pad2: f32,
};
```

`cube_at_creative_look(r, g, b, q, base)` reads `tables[base + i]`; `lookup_creative_look(rgb, q, base)`; `look_creative_look(px, q, base)`; and

```
fn apply_creative_look(p: vec3<f32>, x: u32, y: u32, q: Params_creative_look, base: u32) -> vec3<f32> {
    if (q.blend == 0.0) { return p; }
    let out = look_creative_look(p, q, base);
    return p + (out - p) * q.blend;
}
```

`per_pixel`: params 48 bytes (`size, space, 0, 0` then `dmin` ×3 + `blend`, then `dmax` ×3 + 0); `tables: vec![&self.flat]`.

- [ ] **Step 12: output_transform.rs**

```
struct Params_output_transform {
    mode: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    adobe_gamma: f32,
    prophoto_toe_end: f32,
    pad3: f32,
    pad4: f32,
};
fn encode_channel_output_transform(v: f32, q: Params_output_transform) -> f32 {
    if (q.mode == 0u) { return linear_to_srgb(v); }
    let c = clamp(v, 0.0, 1.0);
    if (q.mode == 1u) { return pow(c, 1.0 / q.adobe_gamma); }
    if (c < q.prophoto_toe_end) { return c * 16.0; }
    return pow(c, 1.0 / 1.8);
}
fn apply_output_transform(p: vec3<f32>, x: u32, y: u32, q: Params_output_transform, base: u32) -> vec3<f32> {
    // ⚠ Primaries first, in linear light, then the transfer.
    let lin = vec3<f32>(dot(q.r0.xyz, p), dot(q.r1.xyz, p), dot(q.r2.xyz, p));
    return vec3<f32>(
        encode_channel_output_transform(lin.x, q),
        encode_channel_output_transform(lin.y, q),
        encode_channel_output_transform(lin.z, q));
}
```

Keep the file's clamp-first comment block above it verbatim. `per_pixel`: params 80 bytes (`mode` at 0; rows at 16/32/48; `ADOBE_RGB_GAMMA` at 64, `PROPHOTO_TOE_END` at 68); `preludes.srgb = true`.

- [ ] **Step 13: Run the crate's tests after each file, then all of them**

Run: `cargo test --release -p gpu-pipeline`
Expected: every existing equivalence test passes unchanged (they run through `GpuChain::apply`, now the emitter with runs of one). A failure names the stage whose function drifted from its old `main`.

- [ ] **Step 14: Commit**

```bash
git add crates/gpu-pipeline/src
git commit -m "feat(gpu): the twelve per-pixel twins are parts; a lone twin is a run of one through the emitter"
```

---

### Task 6: The chain and the GPU mask walk units; the observables; the whole-tail test

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs` (`run_with`, fields, `set_fusion`, `encode_units`, tests)
- Modify: `crates/gpu-pipeline/src/mask.rs` (the sub-stage loop)
- Modify: `crates/gpu-pipeline/src/accelerate.rs` (tests only)

- [ ] **Step 1: Write the failing tests**

In `chain.rs` tests:

```rust
    /// Nine per-pixel twins are ONE fused unit, and with fusion off they are nine.
    #[test]
    fn a_run_of_per_pixel_twins_is_one_fused_unit() {
        let Some(ctx) = ctx() else { return };
        let stages = |ctx: &Arc<GpuContext>| -> Vec<Box<dyn GpuStage>> {
            vec![
                Box::new(crate::GpuWhiteBalanceStage::new([1.05, 0.98, 1.02], ctx)),
                Box::new(GpuExposureStage::new(0.4, ctx.clone())),
                Box::new(crate::GpuContrastStage::new(15.0, ctx)),
                Box::new(crate::GpuToneRegionsStage::new(
                    &edit_model::Edits {
                        highlights: -20.0,
                        shadows: 25.0,
                        ..Default::default()
                    },
                    ctx,
                )),
                Box::new(crate::GpuToneCurveStage::new(
                    &[[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]],
                    ctx,
                )),
                Box::new(crate::GpuSaturationStage::new(20.0, 10.0, ctx)),
                Box::new(crate::GpuDisplayRenderingStage::new(None, None, ctx)),
                Box::new(crate::GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    ctx,
                )),
            ]
        };
        let img = ramp(64, 48);
        let mut fused = GpuChain::new(ctx.clone(), stages(&ctx));
        fused.set_fusion(true);
        let a = fused.apply(img.clone());
        assert_eq!(fused.encode_units(), (1, 0), "one fused run, no single");
        let mut apart = GpuChain::new(ctx.clone(), stages(&ctx));
        apart.set_fusion(false);
        let b = apart.apply(img.clone());
        assert_eq!(apart.encode_units(), (8, 0), "eight runs of one");
        let d = crate::equivalence::compare(&a, &b);
        assert!(
            d.worst_levels <= 0.25,
            "fused and unfused differ by {} levels",
            d.worst_levels
        );
        assert_ne!(a.data(), img.data(), "fixture check: the chain moves pixels");
    }

    /// A non-per-pixel stage splits the run: sharpening between two per-pixel twins gives three
    /// units.
    #[test]
    fn a_spatial_stage_ends_a_fused_run() {
        let Some(ctx) = ctx() else { return };
        let mut chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(0.3, ctx.clone())),
                Box::new(crate::GpuSharpenStage::new(
                    &edit_model::Sharpening {
                        amount: 40.0,
                        radius: 1.0,
                        detail: 50.0,
                        masking: 0.0,
                    },
                    pipeline::RenderScale::FULL,
                    &ctx,
                )),
                Box::new(crate::GpuContrastStage::new(10.0, &ctx)),
            ],
        );
        chain.set_fusion(true);
        let _ = chain.apply(ramp(32, 32));
        assert_eq!(chain.encode_units(), (2, 1));
    }

    /// A timed fused run reports its members as one pass, labelled with all of them.
    #[test]
    fn a_timed_fused_run_is_one_pass() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_fusion(true);
        chain.set_timing(true);
        chain.run_rgba8(&ramp(32, 32)).expect("gpu");
        let t = chain.last_timings().expect("timed");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["exposure+contrast+exposure", "quantise"]);
    }
```

And change `three_stages` to call `c.set_fusion(false)` beside `set_timing(false)`, so the existing per-stage timing tests keep measuring one pass per stage.

In `accelerate.rs` tests, beside `a_colorimetric_frame_renders_the_same_picture_on_the_gpu`:

```rust
    /// ⚠⚠ Every per-pixel stage active at once — a channel curve, a parametric curve, a look, a
    /// region for the vignette — renders on the GPU within the twins' bound of the CPU, and the
    /// whole tail is ONE fused dispatch plus the spatial stages. First asserted to move pixels.
    #[test]
    fn the_whole_per_pixel_tail_fuses_and_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        let mut e = graded(with_hsl());
        e.white_balance = edit_model::WhiteBalance {
            kelvin: Some(4800.0),
            tint: -6.0,
        };
        e.exposure = 0.5;
        e.contrast = 20.0;
        e.highlights = -25.0;
        e.shadows = 30.0;
        e.whites = 10.0;
        e.blacks = -10.0;
        e.tone_curve.points = vec![[0.0, 0.0], [0.45, 0.5], [1.0, 1.0]];
        e.tone_curve_red.points = vec![[0.0, 0.0], [0.5, 0.47], [1.0, 1.0]];
        e.tone_curve_parametric.highlights = 20.0;
        e.saturation = 15.0;
        e.vibrance = 10.0;
        e.vignette = edit_model::Vignette {
            amount: -35.0,
            ..Default::default()
        };
        e.rendering = Some(edit_model::Rendering::Camera);
        let region = Some(Region {
            full_w: 96,
            full_h: 64,
            x0: 8,
            y0: 4,
        });
        let cpu_tail = tail_in_region(&e, region);
        let want = cpu_tail.run(scene_ramp());
        assert_ne!(
            want.data(),
            tail_in_region(&Edits::default(), region)
                .run(scene_ramp())
                .data(),
            "fixture check: the edits move pixels"
        );
        let (pipe, chain) = accelerate(
            tail_in_region(&e, region),
            &e,
            RenderScale::FULL,
            TailFacts {
                region,
                profile: ProfileRendering::colorimetric(),
                spec: RenderSpec::default(),
                prefix_offered: false,
            },
            &ctx,
        );
        assert!(pipe.stage_names().is_empty(), "every stage has a twin: {:?}", pipe.stage_names());
        let chain = chain.expect("the tail is one chain");
        let got = chain.apply(scene_ramp());
        assert_eq!(chain.encode_units(), (1, 0), "one fused dispatch for the whole tail");
        let d = crate::equivalence::compare(&want, &got);
        assert!(d.worst_levels <= 1.0, "GPU differs from CPU by {} levels", d.worst_levels);
    }
```

(`graded`, `with_hsl`, `scene_ramp` and `tail_of` exist in that module; add `tail_in_region(e, region)` beside `tail_of` as the same call with the region passed, and check the parametric-curve field's name in `edit_model`.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --release -p gpu-pipeline fused_unit`
Expected: compile errors, `no method named set_fusion`, `no method named encode_units`.

- [ ] **Step 3: The chain walks units**

In `GpuChain` add fields after `last_timings`:

```rust
    /// Whether runs of per-pixel twins fuse. From `RUSTROOM_NO_FUSION` at construction; settable
    /// per chain for the tests, like `timing`.
    fusion: bool,
    /// Fused runs and single stages this chain has encoded — the observable, since a fused chain
    /// and an unfused one produce the same picture by design.
    fused_units: AtomicUsize,
    single_units: AtomicUsize,
```

initialised in `new`:

```rust
            fusion: crate::per_pixel::fusion_enabled(),
            fused_units: AtomicUsize::new(0),
            single_units: AtomicUsize::new(0),
```

Methods beside `set_timing`:

```rust
    /// Fuse runs of per-pixel twins, or encode each on its own.
    pub fn set_fusion(&mut self, on: bool) {
        self.fusion = on;
    }

    /// `(fused runs, single stages)` this chain has encoded.
    pub fn encode_units(&self) -> (usize, usize) {
        (
            self.fused_units.load(Ordering::Relaxed),
            self.single_units.load(Ordering::Relaxed),
        )
    }
```

Replace the pass `match` in `run_with` with:

```rust
        match rec.as_mut() {
            None => {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut i = 0;
                while i < self.stages.len() {
                    let (unit, next) = crate::per_pixel::next_unit(
                        &self.stages,
                        i,
                        img.width(),
                        img.height(),
                        self.fusion,
                    );
                    self.count_unit(&unit);
                    unit.encode(ctx, &mut pass, &img, &scratch);
                    // ⚠ Relabelled BETWEEN units, so a unit after a resize dispatches over the
                    // new size. wgpu 29's pass holds no borrow of `img`, which is what makes this
                    // legal.
                    let (w, h) = unit.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                    i = next;
                }
                // ⚠ Inside the **same** pass, so it sees every stage's writes under the in-pass
                // barrier this chain already depends on and measures.
                if let Some(p) = &packed {
                    crate::quantize::encode(ctx, &mut pass, &img, p);
                }
            }
            Some(r) => {
                // ⚠⚠ ONE PASS PER UNIT, timing mode only — see `timing.rs`. A fused run is one
                // unit and reports as one pass under its members' joined names.
                let mut i = 0;
                while i < self.stages.len() {
                    let (unit, next) = crate::per_pixel::next_unit(
                        &self.stages,
                        i,
                        img.width(),
                        img.height(),
                        self.fusion,
                    );
                    self.count_unit(&unit);
                    let label = unit.label();
                    let p = r.pair(&label);
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(&label),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    unit.encode(ctx, &mut pass, &img, &scratch);
                    drop(pass);
                    let (w, h) = unit.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                    i = next;
                }
                if let Some(pk) = &packed {
                    let p = r.pair("quantise");
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("quantise"),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    crate::quantize::encode(ctx, &mut pass, &img, pk);
                }
            }
        }
```

and add a private helper:

```rust
    fn count_unit(&self, unit: &crate::per_pixel::Unit<'_>) {
        match unit {
            crate::per_pixel::Unit::Fused(_) => self.fused_units.fetch_add(1, Ordering::Relaxed),
            crate::per_pixel::Unit::Single(_) => self.single_units.fetch_add(1, Ordering::Relaxed),
        };
    }
```

The `Recorder`'s capacity is `stages.len() + 1` pairs, which a fused run only under-uses.

In `mask.rs`, replace the `for sub in &self.subs { sub.encode(...) }` loop with:

```rust
        // 3. the sub-stages, turning `img` from the base into the adjusted image in place — a
        //    run of per-pixel ones as ONE dispatch, the GPU side of fusion's free part.
        //
        // ⚠⚠ **`&scratch[4..]` is the single most dangerous expression in this stage.** Handing
        // over the whole slice lets a sub-stage write `base_r/g/b` and `cov`, and the result is a
        // plausible photograph blended against the wrong thing rather than an error.
        let mut i = 0;
        while i < self.subs.len() {
            let (unit, next) = crate::per_pixel::next_unit(
                &self.subs,
                i,
                img.width(),
                img.height(),
                crate::per_pixel::fusion_enabled(),
            );
            unit.encode(ctx, pass, img, &scratch[4..]);
            i = next;
        }
```

- [ ] **Step 4: Run the new tests and the crate**

Run: `cargo test --release -p gpu-pipeline`
Expected: all pass, including the four new ones and every existing equivalence and timing test.

- [ ] **Step 5: The sabotage**

Temporarily reverse the member order in `emit`'s `calls` (iterate `parts.iter().enumerate().rev()` for the calls only). Run `cargo test --release -p gpu-pipeline the_whole_per_pixel_tail_fuses_and_matches_the_cpu a_run_of_per_pixel_twins_is_one_fused_unit`. Expected: both FAIL on the level bound (the output transform then runs first). Revert by hand and rerun to green.

- [ ] **Step 6: Commit**

```bash
git add crates/gpu-pipeline/src/chain.rs crates/gpu-pipeline/src/mask.rs crates/gpu-pipeline/src/accelerate.rs
git commit -m "feat(gpu): a chain and a mask walk units — a run of per-pixel twins is one dispatch"
```

---

### Task 7: The gate, the measurement, the record

**Files:**
- Modify: `docs/JOURNAL.md` (a new section at the top of the log, and its Contents line)
- Modify: `CLAUDE.md` (the State block, in place)

- [ ] **Step 1: The gate, each exit code from its own command**

```bash
cargo fmt --all -- --check; echo FMT=$?
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings; echo CLIPPY=$?
cargo test --workspace 2>&1 | tail -30; echo TEST=$?
```

Expected: `FMT=0 CLIPPY=0 TEST=0`, 78 goldens unmoved.

- [ ] **Step 2: Measure, before and after, on a quiet machine**

Before is the `gpu-timestamps` branch tip (`b16625a`); after is this branch. On each, on DSCF6263 at rung 3200 and on a photo with the full tail active:

```bash
RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_XFER=1 cargo run --release -p app --example probe_straighten_drag ~/Pictures/fuji/DSCF6263.RAF 3200 2>&1 | grep -E '^median wall|^CHAIN' | tail -4
RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_GPU_TIME=1 cargo run --release -p app --example probe_straighten_drag ~/Pictures/fuji/DSCF6263.RAF 3200 2>&1 | grep -A6 '^GPUTIME' | tail -14
RUSTROOM_NO_FUSION=1 RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_XFER=1 cargo run --release -p app --example probe_straighten_drag ~/Pictures/fuji/DSCF6263.RAF 3200 2>&1 | grep -E '^median wall' 
```

Record: the untimed drag-frame median with and without fusion (the A/B in one binary), the tail chain's GPU pass times before and after, and `uptime`'s load. The tail's three passes were 7.3 ms before; the fused pass is the number to report.

- [ ] **Step 3: The journal and CLAUDE.md**

Add a `### 2026-09-26 — per-pixel fusion (branch \`per-pixel-fusion\`)` section above the GPU timestamps section, with: what shipped (the trait, the runner, the emitter, the units, the switch), the measured before/after table, the observables and the sabotage results, the 78 unmoved goldens, and any deviation from the spec. Add its Contents line. In CLAUDE.md's State block, replace the timestamps paragraph's last sentence with the fusion result in two lines, and bump the test count.

- [ ] **Step 4: Commit**

```bash
git add docs/JOURNAL.md CLAUDE.md
git commit -m "docs: per-pixel fusion in the journal and CLAUDE.md — measured before and after"
```

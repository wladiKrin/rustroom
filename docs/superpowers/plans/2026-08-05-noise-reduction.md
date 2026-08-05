# Noise Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add luminance and color noise reduction to the develop pipeline — the `detail` stage the pipeline order has always named but never had — and make the pipeline scale-aware so the preview approximates the export.

**Architecture:** One new `NoiseReductionStage` runs early (right after geometry), converts to Oklab once, denoises `L` with a self-guided filter and `a`/`b` with a multi-pass box blur, and converts back. Both filters are built on one new `spatial::box_blur` primitive that is O(1) per pixel in the radius. Because a spatial filter's result depends on the resolution it runs at, `build_pipeline` gains a `RenderScale` argument that spatial stages multiply their radii by and per-pixel stages ignore.

**Tech Stack:** Rust (Cargo workspace), `rayon` for data parallelism, `serde` for the sidecar, `egui` 0.35 for the UI, and the existing golden-image PNG regression harness.

**Design doc:** `docs/superpowers/specs/2026-08-05-noise-reduction-design.md`

**Before you start.** Every `cargo` command in this plan needs libraw discoverable by pkg-config on macOS:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Commit messages in this repo carry **no `Co-Authored-By` trailer** — that is the repo owner's explicit preference. Do not add one.

---

## File Structure

**Created:**

| File | Responsibility |
|---|---|
| `crates/pipeline/src/spatial.rs` | `box_blur` — separable running-sum box blur on a single-channel plane. The one neighbourhood primitive; a future sharpening stage reuses it. |
| `crates/pipeline/src/stages/noise_reduction.rs` | `NoiseReductionStage` — Oklab round trip, guided filter on `L`, chroma blur on `a`/`b`, plus the slider→parameter mappings. |

**Modified:**

| File | Change |
|---|---|
| `crates/edit-model/src/lib.rs` | `NoiseReduction` struct + `Edits.noise_reduction` + `is_active()`. |
| `crates/pipeline/src/lib.rs` | `RenderScale`; `build_pipeline(edits, scale)`; `Pipeline::stage_names()`; insert the NR stage. |
| `crates/pipeline/src/stages/mod.rs` | Register the new module. |
| `crates/pipeline/Cargo.toml` | Add `rayon`. |
| `crates/pipeline/tests/golden.rs` | `RenderScale::FULL` at the call site; `noise_chart()`; three new cases. |
| `crates/raw-decode/src/lib.rs` | `DecodeOptions::source_scale()`. |
| `crates/app/src/render_engine.rs` | `render_once`/`RenderEngine::new` take `source_scale`; compute the real `RenderScale`. |
| `crates/app/src/decode_worker.rs` | `DecodeFn` returns a triple; `Decoded.source_scale`. |
| `crates/app/src/buffer_cache.rs` | Test fixture gains `source_scale`. |
| `crates/app/src/main.rs` | `decode_for_edit` returns the triple; export passes `FULL`; four new sliders. |
| `crates/rustroom/src/main.rs` | Pass `RenderScale::FULL`. |
| `CLAUDE.md` | Status, corrected pipeline order, measured perf. |

**Task order is load-bearing.** Task 3 changes `build_pipeline`'s signature, and Task 5 needs `RenderScale` to exist. Tasks 1–4 are independent of each other; 5 depends on 2 and 3; 6 depends on 1 and 5.

---

## Task 1: `NoiseReduction` in the edit model

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/edit-model/src/lib.rs`:

```rust
    #[test]
    fn noise_reduction_defaults_to_off_with_midpoint_modifiers() {
        let nr = NoiseReduction::default();
        assert_eq!(nr.luminance, 0.0);
        assert_eq!(nr.color, 0.0);
        // The two modifiers sit at their midpoint, not zero: they describe *how* to
        // denoise, and zero would mean "smooth everything" / "smallest radius".
        assert_eq!(nr.detail, 50.0);
        assert_eq!(nr.smoothness, 50.0);
        assert_eq!(Edits::default().noise_reduction, nr);
    }

    #[test]
    fn noise_reduction_is_active_only_when_a_strength_is_up() {
        assert!(!NoiseReduction::default().is_active());
        // Modifiers alone must NOT activate the stage — otherwise dragging Detail would
        // insert a stage that costs an Oklab round trip and changes nothing.
        assert!(!NoiseReduction {
            detail: 100.0,
            smoothness: 0.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            luminance: 1.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            color: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_noise_reduction_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.noise_reduction, NoiseReduction::default());
    }

    #[test]
    fn partial_noise_reduction_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"noise_reduction": {"color": 40.0}}"#).unwrap();
        assert_eq!(e.noise_reduction.color, 40.0);
        assert_eq!(e.noise_reduction.luminance, 0.0);
        assert_eq!(e.noise_reduction.detail, 50.0);
        assert_eq!(e.noise_reduction.smoothness, 50.0);
    }

    #[test]
    fn noise_reduction_roundtrips_json() {
        let e = Edits {
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                detail: 70.0,
                color: 55.0,
                smoothness: 20.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model
```

Expected: FAIL — `cannot find type NoiseReduction in this scope` (and `no field noise_reduction on type Edits`).

- [ ] **Step 3: Write the implementation**

In `crates/edit-model/src/lib.rs`, insert after the `Hsl` / `HSL_BAND_NAMES` block and before `/// All edits applied to one image.`:

```rust
/// Noise reduction. The two strengths are 0..100 and default to 0 (off); `detail` and
/// `smoothness` are quality modifiers that default to 50 and do nothing on their own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    /// Luminance NR strength, 0..100. 0 = off.
    pub luminance: f32,
    /// How hard luminance detail is protected, 0..100. Higher keeps more texture — and
    /// more noise with it. Inert while `luminance` is 0.
    pub detail: f32,
    /// Color (chroma) NR strength, 0..100. 0 = off.
    pub color: f32,
    /// Chroma blur radius, 0..100. Higher removes larger color blotches. Inert while
    /// `color` is 0.
    pub smoothness: f32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            detail: 50.0,
            color: 0.0,
            smoothness: 50.0,
        }
    }
}

impl NoiseReduction {
    /// True when these settings actually change pixels.
    ///
    /// Deliberately *not* `self != &Self::default()`: `detail` and `smoothness` only
    /// modify how the two strengths behave, so moving one on its own must not insert a
    /// stage into the pipeline.
    pub fn is_active(&self) -> bool {
        self.luminance > 0.0 || self.color > 0.0
    }
}
```

Then add the field to `Edits`, immediately after `pub hsl: Hsl,`:

```rust
    pub noise_reduction: NoiseReduction,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p edit-model
```

Expected: PASS — all tests in `edit-model`, including the pre-existing ones.

- [ ] **Step 5: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): NoiseReduction params (luminance/detail/color/smoothness)"
```

---

## Task 2: `spatial::box_blur`

The one neighbourhood primitive. Separable with running sums, so cost per pixel is constant regardless of radius — which is the whole reason the guided filter is affordable at full resolution.

**Files:**
- Create: `crates/pipeline/src/spatial.rs`
- Modify: `crates/pipeline/src/lib.rs` (declare the module)
- Modify: `crates/pipeline/Cargo.toml` (add `rayon`)

- [ ] **Step 1: Add the rayon dependency**

In `crates/pipeline/Cargo.toml`, under `[dependencies]`, after `edit-model`:

```toml
rayon.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Create `crates/pipeline/src/spatial.rs` containing **only** the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_zero_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(box_blur(&p, 2, 2, 0), p);
    }

    #[test]
    fn constant_plane_stays_constant() {
        // The classic edge-normalization bug: if out-of-bounds samples are treated as
        // zero instead of shrinking the window, the borders darken here.
        let p = vec![0.375f32; 7 * 5];
        let out = box_blur(&p, 7, 5, 2);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn known_one_dimensional_case() {
        let p = vec![0.0, 0.0, 1.0, 0.0, 0.0];
        let out = box_blur(&p, 5, 1, 1);
        let third = 1.0 / 3.0;
        let expect = [0.0, third, third, third, 0.0];
        for (o, e) in out.iter().zip(expect) {
            assert!((o - e).abs() < 1e-6, "{out:?} vs {expect:?}");
        }
    }

    #[test]
    fn known_two_dimensional_case() {
        // A single 1 in the middle of a 3x3, r=1. With windows clamped to the image the
        // corner sees 4 samples, an edge 6, the centre 9 — so this one assertion covers
        // separability, edge normalization and the running sums at once.
        let mut p = vec![0.0f32; 9];
        p[4] = 1.0;
        let out = box_blur(&p, 3, 3, 1);
        let expect = [
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 9.0,
            1.0 / 6.0,
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 4.0,
        ];
        for (i, (o, e)) in out.iter().zip(expect).enumerate() {
            assert!((o - e).abs() < 1e-6, "sample {i}: {o} vs {e}");
        }
    }

    #[test]
    fn radius_larger_than_image_gives_the_global_mean() {
        let p: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let mean = p.iter().sum::<f32>() / 12.0;
        let out = box_blur(&p, 4, 3, 50);
        for v in &out {
            assert!((v - mean).abs() < 1e-5, "{v} vs {mean}");
        }
    }

    #[test]
    fn blur_preserves_horizontal_mirror_symmetry() {
        // An asymmetric implementation (e.g. an off-by-one in the sliding window) shows
        // up here even when the totals happen to look right.
        let w = 6;
        let src: Vec<f32> = vec![0.1, 0.9, 0.3, 0.3, 0.9, 0.1];
        let out = box_blur(&src, w, 1, 2);
        for i in 0..w / 2 {
            assert!(
                (out[i] - out[w - 1 - i]).abs() < 1e-6,
                "{i}: {} vs {}",
                out[i],
                out[w - 1 - i]
            );
        }
    }
}
```

Declare the module in `crates/pipeline/src/lib.rs` — add after `mod stages;`:

```rust
pub mod spatial;
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p pipeline spatial
```

Expected: FAIL to compile — `cannot find function box_blur in this scope`.

- [ ] **Step 4: Write the implementation**

Insert at the **top** of `crates/pipeline/src/spatial.rs`, above the test module:

```rust
//! Spatial helpers for the neighbourhood-based stages.
//!
//! These work on a single-channel plane (`&[f32]` of `w * h` samples) rather than an
//! `ImageBuf`, because the filters that need them operate on decorrelated channels —
//! Oklab L, a, b — not on interleaved RGB.

use rayon::prelude::*;

/// Blur `plane` with a `(2r+1) × (2r+1)` box kernel.
///
/// Separable, and each pass keeps a running sum, so the cost per pixel is **constant
/// regardless of `r`**. That is what makes a radius that scales with resolution
/// affordable — an O(r²) kernel would need ~3600 taps per pixel on a 24 Mpx export.
///
/// Borders clamp the *window*, not the samples: near an edge the window shrinks and the
/// sum is divided by the number of samples actually inside it. That keeps a constant
/// plane exactly constant, which the "treat outside as zero" variant famously does not.
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn box_blur(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    if r == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let horizontal = blur_rows(plane, w, h, r);
    blur_cols(&horizontal, w, h, r)
}

/// Horizontal pass: one running sum per row, rows farmed out to rayon.
fn blur_rows(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .zip(src.par_chunks_exact(w))
        .for_each(|(orow, srow)| {
            // Window for x = 0 is [0, min(r + 1, w)).
            let mut hi = (r + 1).min(w);
            let mut lo = 0usize;
            let mut sum: f32 = srow[..hi].iter().sum();
            for (x, o) in orow.iter_mut().enumerate() {
                *o = sum / (hi - lo) as f32;
                // Slide one step right for the next x.
                if x + r + 1 < w {
                    sum += srow[x + r + 1];
                    hi += 1;
                }
                if x >= r {
                    sum -= srow[x - r];
                    lo += 1;
                }
            }
        });
    out
}

/// Vertical pass: one running sum *per column*, all columns advanced together so every
/// memory access is a contiguous row slice.
///
/// Walking one column at a time would stride by `w` and miss cache on every access; a
/// transpose-blur-transpose would scatter-write the whole buffer four times per blur.
/// This costs about four sequential passes over the data and no extra allocation beyond
/// the accumulator row. It is deliberately not parallelized — it is memory-bandwidth
/// bound, and splitting it into row bands means re-priming `sum` per band. Revisit only
/// if a profile says so.
fn blur_cols(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    let mut sum = vec![0.0f32; w];

    let mut hi = (r + 1).min(h);
    let mut lo = 0usize;
    for y in 0..hi {
        for (s, v) in sum.iter_mut().zip(&src[y * w..(y + 1) * w]) {
            *s += v;
        }
    }

    for y in 0..h {
        let inv = 1.0 / (hi - lo) as f32;
        for (o, s) in out[y * w..(y + 1) * w].iter_mut().zip(&sum) {
            *o = s * inv;
        }
        if y + r + 1 < h {
            let add = &src[(y + r + 1) * w..(y + r + 2) * w];
            for (s, v) in sum.iter_mut().zip(add) {
                *s += v;
            }
            hi += 1;
        }
        if y >= r {
            let sub = &src[(y - r) * w..(y - r + 1) * w];
            for (s, v) in sum.iter_mut().zip(sub) {
                *s -= v;
            }
            lo += 1;
        }
    }
    out
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p pipeline spatial
```

Expected: PASS — 6 tests.

- [ ] **Step 6: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy -p pipeline --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/Cargo.toml crates/pipeline/src/spatial.rs crates/pipeline/src/lib.rs Cargo.lock
git commit -m "feat(pipeline): separable running-sum box blur (O(1) per pixel in radius)"
```

---

## Task 3: `RenderScale` and the `build_pipeline` signature

A pure refactor: thread a scale argument through, with `RenderScale::FULL` at every call site. No behaviour changes yet — every existing test must still pass untouched in meaning.

**Files:**
- Modify: `crates/pipeline/src/lib.rs`
- Modify: `crates/pipeline/tests/golden.rs:112`
- Modify: `crates/app/src/render_engine.rs:78`
- Modify: `crates/app/src/main.rs:354`
- Modify: `crates/rustroom/src/main.rs:87`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn render_scale_full_is_one() {
        assert_eq!(RenderScale::FULL.get(), 1.0);
    }

    #[test]
    fn render_scale_clamps_nonsense_input() {
        // A scale above 1 or at/below 0 is a caller bug, but the render worker must not
        // panic over it — a clamped radius is a recoverable wrong answer.
        assert_eq!(RenderScale::new(4.0).get(), 1.0);
        assert_eq!(RenderScale::new(0.0).get(), RenderScale::new(-1.0).get());
        assert!(RenderScale::new(0.0).get() > 0.0);
        assert_eq!(RenderScale::new(f32::NAN).get(), 1.0);
        assert_eq!(RenderScale::new(0.25).get(), 0.25);
    }

    #[test]
    fn per_pixel_stages_ignore_the_render_scale() {
        // Everything that existed before noise reduction is per-pixel, so scale must be
        // a no-op for it — bit for bit, not just visually.
        let edits = Edits {
            exposure: 0.7,
            contrast: 25.0,
            saturation: 30.0,
            ..Edits::default()
        };
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.1 * x as f32, 0.2, 0.05 * y as f32]);
            }
        }
        let full = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let quarter = build_pipeline(&edits, RenderScale::new(0.25)).run(img);
        assert_eq!(full.data(), quarter.data());
    }

    #[test]
    fn stage_names_are_reported_in_order() {
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["exposure", "saturation", "output_transform_srgb"]
        );
    }
```

Then update **every** existing `build_pipeline(...)` call in that same test module to pass `RenderScale::FULL` as a second argument. There are 8 of them, in: `output_transform_encodes_linear_to_srgb`, `default_pipeline_has_output_transform_last`, `nonneutral_edits_add_stages_before_output_transform`, `hsl_adds_one_stage_when_non_neutral`, `neutral_hsl_adds_no_stage`, `saturation_adds_one_stage`, `vibrance_alone_adds_one_stage`, `neutral_color_adds_no_stage`, `geometry_is_prepended_before_other_stages`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib
```

Expected: FAIL to compile — `cannot find type RenderScale in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/lib.rs`, add after the `pub use stages::...` block:

```rust
/// Ratio of the image being rendered to the full-resolution original: `1.0` at full res,
/// ≈0.27 for the 1600 px preview of a 6000 px file.
///
/// Spatial stages multiply their pixel radii by this, so a preview approximates the
/// export instead of contradicting it. Per-pixel stages ignore it entirely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderScale(f32);

impl RenderScale {
    /// Rendering at the original resolution.
    pub const FULL: RenderScale = RenderScale(1.0);

    /// Clamped to `(0, 1]`. Anything outside that (or non-finite) is a caller bug, but a
    /// clamped radius beats panicking on the render worker thread.
    pub fn new(scale: f32) -> Self {
        if scale.is_finite() {
            RenderScale(scale.clamp(1e-3, 1.0))
        } else {
            RenderScale::FULL
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}
```

Add to `impl Pipeline`, after `is_empty`:

```rust
    /// Stage names in run order. Useful when debugging a render, and it lets tests assert
    /// pipeline *order* rather than just its length.
    pub fn stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }
```

Change the signature of `build_pipeline` and its doc comment:

```rust
/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline {
```

Silence the not-yet-used parameter for this task only — Task 6 consumes it:

```rust
    let _ = scale; // consumed by the spatial stage added in the noise-reduction task
```

Put that line directly under the `let mut stages: Vec<Box<dyn PipelineStage>> = Vec::new();`.

- [ ] **Step 4: Update the four non-test call sites**

`crates/pipeline/tests/golden.rs` — change the `use` line and line 112:

```rust
use pipeline::{build_pipeline, RenderScale};
```
```rust
    let rendered = build_pipeline(edits, RenderScale::FULL).run(input);
```

`crates/app/src/render_engine.rs` — change the `use` line and line 78:

```rust
use pipeline::{build_pipeline, RenderScale};
```
```rust
    let out = build_pipeline(edits, RenderScale::FULL).run(preview); // sRGB, 0..=1
```

`crates/app/src/main.rs` line 354:

```rust
                let rendered =
                    pipeline::build_pipeline(&edits, pipeline::RenderScale::FULL).run(image);
```

`crates/rustroom/src/main.rs` line 87:

```rust
    let rendered = pipeline::build_pipeline(&edits, pipeline::RenderScale::FULL).run(decoded.image);
```

- [ ] **Step 5: Run the full test suite to verify everything passes**

```bash
cargo test --all
```

Expected: PASS, including all **14 golden references unchanged** — no existing stage is spatial, so `RenderScale::FULL` cannot alter any output.

- [ ] **Step 6: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/pipeline/tests/golden.rs crates/app/src/render_engine.rs crates/app/src/main.rs crates/rustroom/src/main.rs
git commit -m "refactor(pipeline): RenderScale on build_pipeline, plus Pipeline::stage_names"
```

---

## Task 4: `DecodeOptions::source_scale()`

`RawMetadata.width/height` reports the *processed* size, so on a `preview()` decode it already reads half-size and cannot serve as the full-res reference. But `half_size` is our own flag, so the factor is exactly 0.5 by construction — and it belongs next to the flag so it stays truthful if the options change.

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/raw-decode/src/lib.rs`:

```rust
    #[test]
    fn source_scale_reflects_half_size_decoding() {
        // The pipeline's spatial stages need to know how far a decode is from full res.
        // Metadata cannot answer that (it reports the *processed* size), but the
        // half_size flag can, exactly.
        assert_eq!(DecodeOptions::preview().source_scale(), 0.5);
        assert_eq!(DecodeOptions::linear_scene().source_scale(), 1.0);
        assert_eq!(DecodeOptions::display().source_scale(), 1.0);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p raw-decode source_scale
```

Expected: FAIL to compile — `no method named source_scale found for struct DecodeOptions`.

- [ ] **Step 3: Write the implementation**

In `crates/raw-decode/src/lib.rs`, add to `impl DecodeOptions` after `preview()`:

```rust
    /// Size of the decoded image relative to a full-resolution decode of the same file.
    ///
    /// The develop pipeline's spatial stages scale their pixel radii by how far the
    /// render is from full resolution, and this is the first of the two factors.
    /// `RawMetadata` cannot supply it — it reports the *processed* dimensions, which are
    /// already halved here — but `half_size` is our own flag, so the ratio is exact.
    pub fn source_scale(&self) -> f32 {
        if self.half_size {
            0.5
        } else {
            1.0
        }
    }
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p raw-decode source_scale
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/raw-decode/src/lib.rs
git commit -m "feat(raw-decode): DecodeOptions::source_scale for the pipeline scale seam"
```

---

## Task 5: `NoiseReductionStage`

The substance. Not wired into `build_pipeline` yet — that is Task 6, so this task stays reviewable on its own.

**Files:**
- Create: `crates/pipeline/src/stages/noise_reduction.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`
- Modify: `crates/pipeline/src/lib.rs` (re-export)

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/stages/noise_reduction.rs` with **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*; // brings in NoiseReductionStage, RenderScale, ImageBuf, NoiseReduction,
                  // PipelineStage (needed for `.apply`) and the Oklab conversions.

    /// xorshift32, so the noise below is decorrelated across channels. (Deliberately a
    /// local copy of the HSL tests' helper: a 20-line deterministic fixture is not worth
    /// a shared test-support crate, and the two copies are free to diverge.)
    struct Rng(u32);
    impl Rng {
        fn next_signed(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    /// A 64x64 patch of `base` plus per-channel noise.
    fn noisy_patch(base: [f32; 3], amp: f32) -> ImageBuf {
        let mut rng = Rng(0x1234_5678);
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.set_pixel(
                    x,
                    y,
                    [
                        base[0] + amp * rng.next_signed(),
                        base[1] + amp * rng.next_signed(),
                        base[2] + amp * rng.next_signed(),
                    ],
                );
            }
        }
        img
    }

    /// Standard deviation of the image's Oklab L — what luminance NR is supposed to move.
    /// Raw RGB sd would be a weak proxy: most of a gray patch's RGB variance lives in the
    /// chroma directions, which luminance NR does not touch.
    fn l_sd(img: &ImageBuf) -> f32 {
        let ls: Vec<f32> = img
            .data()
            .chunks_exact(3)
            .map(|p| linear_srgb_to_oklab([p[0], p[1], p[2]])[0])
            .collect();
        let mean = ls.iter().sum::<f32>() / ls.len() as f32;
        (ls.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / ls.len() as f32).sqrt()
    }

    /// Standard deviation of the Oklab a and b samples pooled — what color NR moves.
    fn chroma_sd(img: &ImageBuf) -> f32 {
        let cs: Vec<f32> = img
            .data()
            .chunks_exact(3)
            .flat_map(|p| {
                let lab = linear_srgb_to_oklab([p[0], p[1], p[2]]);
                [lab[1], lab[2]]
            })
            .collect();
        let mean = cs.iter().sum::<f32>() / cs.len() as f32;
        (cs.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cs.len() as f32).sqrt()
    }

    /// Mean Oklab a and b — the average color, which color NR must *not* move.
    fn mean_ab(img: &ImageBuf) -> (f32, f32) {
        let n = (img.width() * img.height()) as f32;
        let (mut sa, mut sb) = (0.0, 0.0);
        for p in img.data().chunks_exact(3) {
            let lab = linear_srgb_to_oklab([p[0], p[1], p[2]]);
            sa += lab[1];
            sb += lab[2];
        }
        (sa / n, sb / n)
    }

    fn run(params: NoiseReduction, img: ImageBuf) -> ImageBuf {
        NoiseReductionStage::new(&params, RenderScale::FULL).apply(img)
    }

    #[test]
    fn zero_strengths_are_an_exact_identity() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let params = NoiseReduction {
            luminance: 0.0,
            detail: 0.0,
            color: 0.0,
            smoothness: 100.0,
        };
        assert_eq!(run(params, src.clone()), src);
    }

    #[test]
    fn luminance_strength_cuts_luminance_noise() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let before = l_sd(&src);
        let after = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(
            after < before * 0.5,
            "L sd {before} -> {after} ({}x)",
            after / before
        );
    }

    #[test]
    fn a_hard_edge_keeps_its_contrast() {
        // The guided filter's entire claim. A plain blur fails this; a botched `slope`
        // term fails this.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, if x < 32 { [0.05; 3] } else { [0.75; 3] });
            }
        }
        let out = run(
            NoiseReduction {
                luminance: 100.0,
                ..Default::default()
            },
            src.clone(),
        );
        let want = src.pixel(32, 20)[0] - src.pixel(31, 20)[0];
        let got = out.pixel(32, 20)[0] - out.pixel(31, 20)[0];
        assert!(got > want * 0.9, "edge contrast {want} -> {got}");
    }

    #[test]
    fn detail_slider_trades_smoothing_for_texture() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let smooth = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                detail: 0.0,
                ..Default::default()
            },
            src.clone(),
        ));
        let preserve = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                detail: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(
            preserve > smooth * 1.5,
            "detail 0 -> sd {smooth}, detail 100 -> sd {preserve}"
        );
    }

    #[test]
    fn color_strength_cuts_chroma_noise_without_moving_the_color() {
        let src = noisy_patch([0.30, 0.15, 0.10], 0.02);
        let (a0, b0) = mean_ab(&src);
        let before = chroma_sd(&src);
        let out = run(
            NoiseReduction {
                color: 100.0,
                ..Default::default()
            },
            src,
        );
        let after = chroma_sd(&out);
        assert!(
            after < before * 0.3,
            "chroma sd {before} -> {after} ({}x)",
            after / before
        );
        let (a1, b1) = mean_ab(&out);
        assert!((a1 - a0).abs() < 0.005 && (b1 - b0).abs() < 0.005);
    }

    #[test]
    fn smoothness_widens_the_chroma_blur() {
        // A chroma wave with a 32 px period: the small radius leaves much of it, the
        // large one (window wider than the image) flattens it.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let s = (x as f32 / 32.0 * std::f32::consts::TAU).sin();
                src.set_pixel(x, y, [0.3 + 0.1 * s, 0.3, 0.3 - 0.1 * s]);
            }
        }
        let narrow = chroma_sd(&run(
            NoiseReduction {
                color: 100.0,
                smoothness: 0.0,
                ..Default::default()
            },
            src.clone(),
        ));
        let wide = chroma_sd(&run(
            NoiseReduction {
                color: 100.0,
                smoothness: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(wide < narrow, "smoothness 0 -> {narrow}, 100 -> {wide}");
    }

    #[test]
    fn a_noisy_gray_patch_goes_neutral_under_color_nr() {
        let src = noisy_patch([0.45, 0.45, 0.45], 0.02);
        let out = run(
            NoiseReduction {
                color: 100.0,
                smoothness: 100.0,
                ..Default::default()
            },
            src,
        );
        let (a, b) = mean_ab(&out);
        assert!(a.abs() < 2e-3 && b.abs() < 2e-3, "mean a {a}, b {b}");
        assert!(chroma_sd(&out) < 2e-3);
    }

    #[test]
    fn never_emits_negative_samples() {
        let src = noisy_patch([0.02, 0.01, 0.005], 0.02);
        let out = run(
            NoiseReduction {
                luminance: 100.0,
                detail: 0.0,
                color: 100.0,
                smoothness: 100.0,
            },
            src,
        );
        assert!(out.data().iter().all(|&v| v >= 0.0));
    }

    #[test]
    fn radii_scale_with_the_render_scale() {
        assert_eq!(scaled_radius(6.0, RenderScale::FULL), 6);
        assert_eq!(scaled_radius(6.0, RenderScale::new(0.25)), 2);
        // Never rounds down to zero — a zero radius would silently disable the filter.
        assert_eq!(scaled_radius(6.0, RenderScale::new(0.01)), 1);
    }

    #[test]
    fn eps_spans_two_decades_geometrically() {
        assert!((eps_for_detail(0.0) - 1e-3).abs() < 1e-9);
        assert!((eps_for_detail(100.0) - 1e-5).abs() < 1e-9);
        // Geometric midpoint, not arithmetic: the quantity is a variance spanning two
        // decades, so a linear slider would be unusable at one end.
        assert!((eps_for_detail(50.0) - 1e-4).abs() < 1e-7);
    }

    #[test]
    fn chroma_radius_grows_with_smoothness() {
        assert_eq!(chroma_radius_full(0.0), 8.0);
        assert_eq!(chroma_radius_full(100.0), 32.0);
        assert_eq!(chroma_radius_full(50.0), 20.0);
    }
}
```

Register the module — `crates/pipeline/src/stages/mod.rs` is an alphabetical list, so insert between `hsl` and `output_transform`:

```rust
pub mod hsl;
pub mod noise_reduction;
pub mod output_transform;
```

And in `crates/pipeline/src/lib.rs`, next to the other re-exports:

```rust
pub use stages::noise_reduction::NoiseReductionStage;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline noise_reduction
```

Expected: FAIL to compile — `cannot find struct NoiseReductionStage`.

- [ ] **Step 3: Write the implementation**

Insert at the **top** of `crates/pipeline/src/stages/noise_reduction.rs`, above the test module:

```rust
use crate::spatial::box_blur;
use crate::{PipelineStage, RenderScale};
use color::{linear_srgb_to_oklab, oklab_to_linear_srgb};
use core_image::ImageBuf;
use edit_model::NoiseReduction;
use rayon::prelude::*;

/// Guided-filter window radius, in **full-resolution** pixels: the scale of the noise
/// grain we are averaging out. Not user-exposed — Detail already exposes the quality
/// trade-off, and a second knob here buys nothing.
const LUM_RADIUS_FULL: f32 = 6.0;
/// Chroma blur radius at Smoothness 0 and 100, in full-resolution pixels. Far larger than
/// the luminance radius: chroma is genuinely low-frequency in real images, so a wide blur
/// costs almost no visible detail.
const COL_RADIUS_FULL: (f32, f32) = (8.0, 32.0);
/// `eps` at Detail 0 and Detail 100 — the local variance below which a neighbourhood
/// counts as noise rather than structure.
const EPS_AT_DETAIL_0: f32 = 1e-3;
const EPS_AT_DETAIL_100: f32 = 1e-5;
/// Two box passes approximate a Gaussian closely enough to avoid the cross-shaped
/// artifact a single box blur leaves, and cost nothing asymptotically.
const CHROMA_PASSES: usize = 2;

/// Convert a full-resolution radius to the resolution actually being rendered. Never
/// returns 0: that would silently turn the filter off at small preview scales.
fn scaled_radius(full_px: f32, scale: RenderScale) -> usize {
    ((full_px * scale.get()).round().max(0.0) as usize).max(1)
}

/// The guided filter's noise/structure threshold, from the Detail slider. Interpolated
/// geometrically because the quantity spans two decades.
fn eps_for_detail(detail: f32) -> f32 {
    let t = (detail / 100.0).clamp(0.0, 1.0);
    EPS_AT_DETAIL_0 * (EPS_AT_DETAIL_100 / EPS_AT_DETAIL_0).powf(t)
}

/// Chroma blur radius in full-resolution pixels, from the Smoothness slider.
fn chroma_radius_full(smoothness: f32) -> f32 {
    let t = (smoothness / 100.0).clamp(0.0, 1.0);
    COL_RADIUS_FULL.0 + (COL_RADIUS_FULL.1 - COL_RADIUS_FULL.0) * t
}

/// Luminance and color noise reduction, in Oklab.
///
/// Runs early in the pipeline — before white balance — so it denoises the source rather
/// than fighting noise that Exposure, Shadows and the tone curve have already amplified.
/// Separating luminance from chroma is the point: chroma noise wants a wide, cheap blur,
/// while luminance noise needs an edge-aware filter, because that is where detail lives.
pub struct NoiseReductionStage {
    params: NoiseReduction,
    scale: RenderScale,
}

impl NoiseReductionStage {
    pub fn new(params: &NoiseReduction, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for NoiseReductionStage {
    fn name(&self) -> &'static str {
        "noise_reduction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }

        // One Oklab round trip serves both halves.
        let n = w * h;
        let mut lum = vec![0.0f32; n];
        let mut ca = vec![0.0f32; n];
        let mut cb = vec![0.0f32; n];
        lum.par_iter_mut()
            .zip(ca.par_iter_mut())
            .zip(cb.par_iter_mut())
            .zip(image.data().par_chunks_exact(3))
            .for_each(|(((l, a), b), px)| {
                let lab = linear_srgb_to_oklab([px[0], px[1], px[2]]);
                *l = lab[0];
                *a = lab[1];
                *b = lab[2];
            });

        if self.params.luminance > 0.0 {
            let r = scaled_radius(LUM_RADIUS_FULL, self.scale);
            let eps = eps_for_detail(self.params.detail);
            let filtered = guided_self_filter(&lum, w, h, r, eps);
            let s = (self.params.luminance / 100.0).clamp(0.0, 1.0);
            for (dst, q) in lum.iter_mut().zip(filtered) {
                *dst += (q - *dst) * s;
            }
        }

        if self.params.color > 0.0 {
            let r = scaled_radius(chroma_radius_full(self.params.smoothness), self.scale);
            let s = (self.params.color / 100.0).clamp(0.0, 1.0);
            for plane in [&mut ca, &mut cb] {
                let mut blurred = plane.clone();
                for _ in 0..CHROMA_PASSES {
                    blurred = box_blur(&blurred, w, h, r);
                }
                for (dst, q) in plane.iter_mut().zip(blurred) {
                    *dst += (q - *dst) * s;
                }
            }
        }

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .zip(lum.par_iter().zip(ca.par_iter()).zip(cb.par_iter()))
            .for_each(|(px, ((&l, &a), &b))| {
                let rgb = oklab_to_linear_srgb([l, a, b]);
                // Strong smoothing can push a pixel out of gamut; clamp at 0, the same
                // convention HslStage uses.
                px[0] = rgb[0].max(0.0);
                px[1] = rgb[1].max(0.0);
                px[2] = rgb[2].max(0.0);
            });
        ImageBuf::from_vec(w, h, data)
    }
}

/// Self-guided filter (He et al.): edge-preserving smoothing at O(1) per pixel in `r`.
///
/// `slope` and `offset` are the paper's `a` and `b`, renamed because `a` and `b` are
/// already the Oklab chroma channels in this file.
///
/// The behaviour lives entirely in `slope`. Where the local variance greatly exceeds
/// `eps` — an edge — `slope → 1` and the pixel passes through untouched. Where it is far
/// below `eps` — noise — `slope → 0` and the pixel becomes its local mean.
fn guided_self_filter(p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let squared: Vec<f32> = p.iter().map(|v| v * v).collect();
    let mean = box_blur(p, w, h, r);
    let mean_sq = box_blur(&squared, w, h, r);

    let mut slope = Vec::with_capacity(p.len());
    let mut offset = Vec::with_capacity(p.len());
    for (&m, &msq) in mean.iter().zip(mean_sq.iter()) {
        // Cancellation between two nearly equal sums can push this slightly negative on
        // perfectly flat areas.
        let var = (msq - m * m).max(0.0);
        let s = var / (var + eps);
        slope.push(s);
        offset.push((1.0 - s) * m);
    }

    let mean_slope = box_blur(&slope, w, h, r);
    let mean_offset = box_blur(&offset, w, h, r);
    mean_slope
        .iter()
        .zip(mean_offset.iter())
        .zip(p.iter())
        .map(|((&ms, &mo), &v)| ms * v + mo)
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline noise_reduction
```

Expected: PASS — 11 tests.

If `luminance_strength_cuts_luminance_noise` or `detail_slider_trades_smoothing_for_texture` fails by a small margin, **do not loosen the assertion**. Check `eps_for_detail` and the `var` computation first — those thresholds were derived from the noise level the fixture actually produces (σ_L ≈ 0.004 at amp 0.02), and a miss means the filter is not doing what the math says.

- [ ] **Step 5: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy -p pipeline --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/noise_reduction.rs crates/pipeline/src/stages/mod.rs crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): NoiseReductionStage — guided filter on L, chroma blur on a/b"
```

---

## Task 6: Wire the stage into `build_pipeline`

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn noise_reduction_runs_before_every_adjustment() {
        // Early placement is the whole point: denoise the source rather than fight noise
        // that a Shadows lift has already multiplied up. Geometry stays first so NR runs
        // on the cropped pixels.
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                color: 40.0,
                ..Default::default()
            },
            white_balance: edit_model::WhiteBalance {
                temp: 20.0,
                ..Default::default()
            },
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "geometry",
                "noise_reduction",
                "white_balance",
                "exposure",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn noise_reduction_modifiers_alone_add_no_stage() {
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 0.0,
                detail: 100.0,
                smoothness: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib noise_reduction
```

Expected: FAIL — `noise_reduction_runs_before_every_adjustment` gets `["geometry", "white_balance", "exposure", "output_transform_srgb"]`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/lib.rs`, **delete** the placeholder line added in Task 3:

```rust
    let _ = scale; // consumed by the spatial stage added in the noise-reduction task
```

and insert immediately after the geometry block (before the white-balance block):

```rust
    // Noise reduction comes before every adjustment: denoising the source is far easier
    // than denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are
    // modifiers and must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        stages.push(Box::new(NoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
        )));
    }
```

- [ ] **Step 4: Run the full suite to verify it passes**

```bash
cargo test --all
```

Expected: PASS — including all **14 golden references still byte-identical** (none of them set `noise_reduction`, so no NR stage is added).

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): insert noise reduction after geometry, before white balance"
```

---

## Task 7: Golden regression references

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/nr_luminance.png`, `nr_color.png`, `nr_both.png` (generated by the harness in Step 3, then committed)

- [ ] **Step 1: Add the noisy chart and the three cases**

First change the `use` line at the top of `crates/pipeline/tests/golden.rs` so `NoiseReduction` is in scope:

```rust
use edit_model::{Edits, NoiseReduction, ToneCurve, WhiteBalance};
```

Then add after `hue_sweep_chart()`:

```rust
/// Deterministic noisy chart for the noise-reduction cases: three stacked regions — flat
/// gray, flat color, and a hard black/white vertical edge — all under per-channel
/// xorshift noise. One reference image therefore covers smoothing, chroma handling and
/// edge preservation at once. Larger than the other charts (128x64) because a filter with
/// a 6..32 px radius is meaningless on a 64x16 strip.
fn noise_chart() -> ImageBuf {
    const NW: usize = 128;
    const NH: usize = 64;
    let mut state: u32 = 0x9E37_79B9;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        ((state as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.03
    };
    let mut img = ImageBuf::new(NW, NH);
    for y in 0..NH {
        for x in 0..NW {
            let base = if y < NH / 3 {
                [0.35, 0.35, 0.35]
            } else if y < 2 * NH / 3 {
                [0.35, 0.18, 0.12]
            } else if x < NW / 2 {
                [0.05, 0.05, 0.05]
            } else {
                [0.75, 0.75, 0.75]
            };
            img.set_pixel(
                x,
                y,
                [
                    (base[0] + noise()).max(0.0),
                    (base[1] + noise()).max(0.0),
                    (base[2] + noise()).max(0.0),
                ],
            );
        }
    }
    img
}

#[test]
fn golden_nr_luminance() {
    check_golden_with(
        "nr_luminance",
        &Edits {
            noise_reduction: NoiseReduction {
                luminance: 80.0,
                detail: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_nr_color() {
    check_golden_with(
        "nr_color",
        &Edits {
            noise_reduction: NoiseReduction {
                color: 80.0,
                smoothness: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_nr_both() {
    check_golden_with(
        "nr_both",
        &Edits {
            noise_reduction: NoiseReduction {
                luminance: 60.0,
                detail: 40.0,
                color: 80.0,
                smoothness: 60.0,
            },
            ..Edits::default()
        },
        noise_chart(),
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --test golden
```

Expected: the three new tests FAIL — the reference PNGs do not exist yet. The 14 existing tests must still PASS.

- [ ] **Step 3: Generate the references**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden
```

Expected: PASS, and three new PNGs written.

- [ ] **Step 4: Inspect the generated references before trusting them**

```bash
git status --short crates/pipeline/tests
```

Expected: **exactly three new untracked PNGs** (`nr_luminance`, `nr_color`, `nr_both`) and **no modifications to the 14 existing ones**. If any existing reference changed, stop — something regressed; `git checkout` the reference files and find out what before continuing.

Open the three new PNGs and confirm they look right: the two flat regions visibly smoother than the input, and the black/white edge still hard, not a gradient.

- [ ] **Step 5: Re-run without the env var to verify they pass**

```bash
cargo test -p pipeline --test golden
```

Expected: PASS — 17 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests
git commit -m "test(pipeline): golden references for luminance and color noise reduction"
```

---

## Task 8: Plumb the real render scale through the app

Until now the app has passed `RenderScale::FULL` even for previews, so NR in the GUI would be applied at a full-res radius on a downscaled image. This task makes the preview honest.

**Files:**
- Modify: `crates/app/src/render_engine.rs`
- Modify: `crates/app/src/decode_worker.rs`
- Modify: `crates/app/src/buffer_cache.rs` (test fixture)
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/app/src/render_engine.rs`:

```rust
    #[test]
    fn render_scale_combines_the_decode_and_the_downscale() {
        // A half-size RAW decode downscaled again by 2 is a quarter of full resolution;
        // the spatial stages must be told that, or a preview lies about the export.
        assert_eq!(render_scale(8, 4, 0.5).get(), 0.25);
        // A full-size source rendered at full size is exactly 1.0.
        assert_eq!(render_scale(8, 8, 1.0).get(), 1.0);
        // Non-square: the long edge is the reference, matching downscale_to.
        assert_eq!(render_scale(10, 5, 1.0).get(), 0.5);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app render_scale
```

Expected: FAIL to compile — `cannot find function render_scale in this scope`.

- [ ] **Step 3: Update `render_engine.rs`**

Replace the `Request` struct, `RenderEngine` struct, `new`, and `render_once` in `crates/app/src/render_engine.rs`:

```rust
/// Owns a worker thread that renders against a fixed editing buffer.
pub struct RenderEngine {
    req_tx: Sender<Request>,
    res_rx: Receiver<RenderedFrame>,
    _handle: JoinHandle<()>,
}

impl RenderEngine {
    /// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
    /// same file (0.5 for a half-size RAW decode, 1.0 for JPEG/PNG/TIFF).
    /// `on_done` is called after each render completes (e.g. `ctx.request_repaint`).
    pub fn new(
        buffer: Arc<ImageBuf>,
        source_scale: f32,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<RenderedFrame>();
        let handle = thread::spawn(move || {
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                let frame = render_once(&buffer, &req.edits, req.max_dim, source_scale);
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();
            }
        });
        Self {
            req_tx,
            res_rx,
            _handle: handle,
        }
    }
```

(`request` and `poll_latest` are unchanged — leave them as they are.)

Then replace `render_once` and add `render_scale` above it:

```rust
/// How far a render is from the full-resolution original: the source buffer may already
/// be a half-size decode (`source_scale`), and `downscale_to` shrinks it further. The
/// long edge is the reference because that is what `downscale_to` caps.
fn render_scale(buffer_long: usize, rendered_long: usize, source_scale: f32) -> RenderScale {
    if buffer_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(source_scale * rendered_long as f32 / buffer_long as f32)
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure and synchronous — used by the worker and for one-off renders (e.g. baseline).
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    let out = build_pipeline(edits, scale).run(preview); // sRGB, 0..=1
    let (width, height) = (out.width(), out.height());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in out.data().chunks_exact(3) {
        rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push(255);
    }
    RenderedFrame {
        width,
        height,
        rgba,
    }
}
```

Update the three existing tests in that file to pass the new argument:

```rust
        let engine = RenderEngine::new(tiny(), 1.0, || {});
```
(in both `produces_rgba_frame_of_expected_size` and `exposure_brightens`), and:
```rust
        let f = render_once(&img, &Edits::default(), 4, 1.0);
```
(in `render_once_matches_expected_gray`).

- [ ] **Step 4: Update `decode_worker.rs`**

Change the `DecodeFn` alias (line 34):

```rust
/// Decodes a file into a linear editing buffer, a display label, and the buffer's size
/// relative to a full-resolution decode of the same file (0.5 for half-size RAW).
type DecodeFn = Arc<dyn Fn(&Path) -> Result<(ImageBuf, String, f32), String> + Send + Sync>;
```

Add the field to `Decoded`, after `baseline`:

```rust
    /// Size of `buffer` relative to a full-resolution decode — 0.5 for a half-size RAW
    /// decode. The render engine needs it to scale the spatial stages' radii.
    pub source_scale: f32,
```

And in `spawn`, replace the decode closure body:

```rust
            let outcome = decode(&req.path).map(|(image, label, source_scale)| {
                let buffer = Arc::new(image);
                let baseline = render_once(&buffer, &Edits::default(), req.max_dim, source_scale);
                Arc::new(Decoded {
                    buffer,
                    label,
                    baseline,
                    source_scale,
                })
            });
```

Update the test fixture `tiny_decode`:

```rust
    fn tiny_decode(_path: &Path) -> Result<(ImageBuf, String, f32), String> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Ok((img, "FAKE".to_string(), 1.0))
    }
```

and add one assertion to `delivers_buffer_label_and_baseline`, after the label assertion:

```rust
        assert_eq!(d.source_scale, 1.0);
```

- [ ] **Step 5: Update the `buffer_cache.rs` test fixture**

In `crates/app/src/buffer_cache.rs`, the `decoded()` helper constructs a `Decoded`. Add the new field:

```rust
            source_scale: 1.0,
```

- [ ] **Step 6: Update `main.rs`**

`decode_for_edit` returns the triple:

```rust
/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf`, a display label, and the buffer's size relative to a full-resolution
/// decode. Dispatches by file kind.
fn decode_for_edit(
    path: &std::path::Path,
) -> Result<(core_image::ImageBuf, String, f32), String> {
    if catalog::is_raw(path) {
        let opts = raw_decode::DecodeOptions::preview();
        let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
        let label = format!("{} {}", d.metadata.make, d.metadata.model);
        Ok((d.image, label, opts.source_scale()))
    } else {
        let d = image_decode::decode(path).map_err(|e| e.to_string())?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        // Non-RAW formats are decoded at full size.
        Ok((d.image, format!("{ext} {}×{}", d.width, d.height), 1.0))
    }
}
```

And at line ~220, pass the scale to the engine:

```rust
        let engine = RenderEngine::new(decoded.buffer.clone(), decoded.source_scale, move || {
            ctx2.request_repaint()
        });
```

The export path at line ~354 stays `RenderScale::FULL` — it decodes at full resolution, which is exactly what that means.

- [ ] **Step 7: Run the full suite to verify everything passes**

```bash
cargo test --all
```

Expected: PASS.

- [ ] **Step 8: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 9: Commit**

```bash
git add crates/app/src crates/rustroom/src
git commit -m "feat(app): thread the real render scale (decode + downscale) into the pipeline"
```

---

## Task 9: The Develop panel sliders

`egui` view code is not unit-testable here, so this task is verified by compiling, linting, and running the app.

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the collapsing group**

In `crates/app/src/main.rs`, immediately after the closing `});` of the HSL `"Luminance"` `CollapsingHeader` and **before** the `ui.separator();` that precedes the `"Reset all"` button:

```rust
                        egui::CollapsingHeader::new("Noise Reduction").show(ui, |ui| {
                            let nr = &mut s.edits.noise_reduction;
                            slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
                            slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
                            slider(ui, &mut nr.color, 0.0..=100.0, "Color");
                            slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
                        });
```

The ranges are `0.0..=100.0`, not `-100.0..=100.0` — negative noise reduction is not a thing.

- [ ] **Step 2: Verify it compiles and lints**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Expected: no output (success).

If the borrow checker objects to `let nr = &mut s.edits.noise_reduction;` while `slider` also captures `changed`/`settled`, inline the four field accesses instead (`&mut s.edits.noise_reduction.luminance`, etc.) — `slider` borrows only its `value` argument, so this should compile as written, but the inline form is the fallback.

- [ ] **Step 3: Run the app and verify by hand**

```bash
cargo run --release -p app
```

Always `--release` — a debug build's decode is ~25× slower.

Open a high-ISO RAW, then check each of these:
1. The **Noise Reduction** group appears below the HSL groups and expands.
2. Raising **Color** removes colored speckle from shadow areas; the image's overall color does not shift.
3. Raising **Luminance** smooths grain; raising **Detail** brings texture back.
4. **Undo/redo** (Ctrl+Z / Ctrl+Shift+Z) steps through NR changes like any other slider.
5. **Export**, then reopen the file: the sidecar `<file>.json` contains a `noise_reduction` object and the sliders come back where you left them.
6. **Before/after** toggle still works (the baseline is neutral, so it shows the noisy original).

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): Noise Reduction slider group in the Develop panel"
```

---

## Task 10: Measure, document, verify

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Measure the preview render cost**

Add a temporary timing print in `render_once` (in `crates/app/src/render_engine.rs`), directly around the `build_pipeline(...).run(preview)` line:

```rust
    let t0 = std::time::Instant::now();
    let out = build_pipeline(edits, scale).run(preview); // sRGB, 0..=1
    eprintln!("pipeline {:?} scale {:?}", t0.elapsed(), scale.get());
```

Then:

```bash
cargo run --release -p app
```

Open the same RAW each time and record four numbers from the terminal — take a warm reading (move a slider a few times first, then read):

1. NR off (all four sliders at their defaults)
2. Color 50 only
3. Luminance 50 only (Detail 50)
4. Both at 50

**Remove the timing lines afterwards** and confirm `git diff crates/app/src/render_engine.rs` is empty.

- [ ] **Step 2: Update `CLAUDE.md`**

Make these edits:

1. **Correct the pipeline-order block** (the ```` ``` ```` fenced diagram under "Core architecture"). NR belongs early, sharpening stays late:

```
decode → linearize/black-white → white balance → demosaic →
input color profile → working space (linear f32) →
noise reduction → exposure → tone (highlights/shadows/curve) →
color (HSL/grading) → sharpen → geometry (crop/lens) → output transform → encode
```

2. **Update the crate table row** for `pipeline` — it now reads "9 stages"; make it 10, and mention `spatial.rs`:

```
│   ├── pipeline/             # ✅ PipelineStage + 10 stages (incl. noise_reduction) + spatial::box_blur + build_pipeline(edits, RenderScale) + golden tests
```

3. **Replace the "▶ RESUME HERE" block's "next task" sentence** — the two candidates it lists are now resolved: noise reduction is done, vignette remains.

4. **Add a new status section** directly under the RESUME HERE block, following the style of the "HSL done" section. It must record:
   - `RenderScale` — what it is, why the pipeline needed it (spatial stages), and how it is computed (`DecodeOptions::source_scale()` × `downscale_to` ratio, because `RawMetadata` reports the *processed* size and cannot supply it).
   - The stage: guided filter on Oklab `L` (4 box blurs, O(1) per pixel in radius — the reason bilateral was rejected, since a scale-aware radius means ~3600 taps/pixel at 24 Mpx), multi-pass box blur on `a`/`b`.
   - The parameter mappings: `LUM_RADIUS_FULL = 6` px, `chroma_radius_full` = 8..32 px, `eps = 1e-3 · (1e-2)^(detail/100)` (geometric, spanning two decades), radii scaled by `RenderScale` and never rounded to 0.
   - `NoiseReduction::is_active()` is `luminance > 0 || color > 0`, **not** `!= default()`, because Detail/Smoothness are modifiers.
   - `spatial::box_blur` — separable running sums, window clamped at borders (not samples), horizontal pass rayon-parallel, vertical pass sequential over contiguous row slices to avoid a transpose.
   - Goldens now **17**, on a new 128×64 `noise_chart()`; the 14 existing references byte-identical.
   - The measured perf table from Step 1.
   - The accepted caveats from the design doc §9, especially: the preview over-smooths relative to the export because `downscale_to` has already averaged noise away, so **NR is judged at export**; chroma bleed at high Color/Smoothness; single-scale, so large luminance blotches survive; and NR runs first so with NR on, *every* slider gets slower (no per-stage caching yet).

5. **Add the new docs** to the docs list at the bottom: `specs/2026-08-05-noise-reduction-design.md` and `plans/2026-08-05-noise-reduction.md`.

- [ ] **Step 3: Full verification**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all
```

Expected: all three pass with no output from the first two, and the test run green including 17 golden tests.

- [ ] **Step 4: Confirm the existing goldens never moved**

```bash
git log --oneline --stat -- crates/pipeline/tests/golden | head -20
```

Expected: the only change to that directory across this whole branch is the **addition** of three PNGs. If any pre-existing reference shows as modified in any commit, that is a regression to investigate before finishing.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record noise reduction, the RenderScale seam, and measured perf"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` all green.
- Three new golden references committed; the 14 existing ones byte-identical.
- `build_pipeline` takes a `RenderScale`, and all four call sites pass the right one — `FULL` for export/CLI/goldens, the real ratio for previews.
- In the GUI: the four sliders work, participate in undo/redo, and round-trip through the sidecar.
- `CLAUDE.md` reflects the 10-stage pipeline, the corrected order, the scale seam, and the measured numbers.

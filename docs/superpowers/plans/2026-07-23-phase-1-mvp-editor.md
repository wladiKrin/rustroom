# Phase 1 (M1) — MVP Single-Image Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Phase 0 decode spike into a real, non-destructive develop pipeline: decode a RAW into a **linear, scene-referred** working image, run it through an ordered stage pipeline built from a serializable edit list, and export a correct-looking JPEG — first with zero edits (M1.1), then with core global adjustments (M1.2) and crop (M1.3).

**Architecture:** Decode now yields *linear* RGB (sRGB primaries) instead of libraw's baked sRGB — this is the working space all edits operate in. A `Pipeline` runs an ordered `Vec<Box<dyn PipelineStage>>`; each stage is a pure `ImageBuf -> ImageBuf` transform, unit-testable in isolation. The stage list is *built from* a serde-serializable `Edits` struct (the op-list / sidecar model). The final stage is always the output transform (linear → sRGB), after which `export` quantizes to 8-bit and writes the file. This is the seam where future edits insert before the output transform, in linear light.

**Tech Stack:** Rust workspace (existing), `rsraw`/libraw for decode, hand-coded sRGB transfer functions in a `color` crate (lcms2/ICC deferred — see Scope note), `serde`/`serde_json` for the edit sidecar, `image` crate for JPEG/TIFF encode.

---

## Scope & sequencing

This plan covers **all of Phase 1 (M1.1–M1.4)** but is detailed to executable, bite-sized steps **only for M1.1** (the make-or-break color-pipeline skeleton, which we execute now). M1.2–M1.4 are a concrete roadmap below and will each be expanded into full step-by-step tasks when reached, because their details depend on M1.1's outcome and (for M1.4) introduce a distinct subsystem.

**Two deliberate deviations from the original architecture doc, flagged up front:**
1. **lcms2/ICC deferred past M1.1.** libraw already emits data in sRGB (Rec.709) primaries via `output_color=1`, so the *only* transform needed for a correct sRGB JPEG is the sRGB transfer curve — a well-defined function we hand-code and unit-test exactly. Introducing lcms2's ICC machinery now would be premature (YAGNI). lcms2 enters when we need real camera **input** profiles or display **output** profiles (tracked as a Phase 2 item / revisited in M1.2).
2. **M1.4 (GUI) will be its own plan.** The `egui`+`wgpu` app is a separate subsystem from the headless engine; per the plans-writing scope check it gets its own spec→plan cycle once the engine (M1.1–M1.3) is trustworthy and golden-tested.

---

## File Structure (M1.1)

```
crates/
├── core-image/          # MODIFY: add `map_samples` helper (used by stages)
├── raw-decode/          # MODIFY: DecodeOptions {display, linear_scene}; set libraw gamma/no_auto_bright
├── color/               # NEW: sRGB transfer functions (linear<->encoded), exact + tested
├── edit-model/          # NEW: `Edits` struct (serde) — the op-list / sidecar model
├── pipeline/            # NEW: PipelineStage trait, OutputTransformSrgb stage, Pipeline, build_pipeline(&Edits)
├── export/              # NEW: save_jpeg / save_tiff from a display-encoded ImageBuf
└── cli/                 # MODIFY: `render` subcommand (decode→pipeline→export); `decode` uses display opts
```

**Responsibilities (each crate one job, GUI-free, headless-testable):**
- `color`: pure transfer-function math. No image types. Trivially testable against reference values.
- `edit-model`: the serializable description of *what* edits to apply. Knows nothing about *how*.
- `pipeline`: turns an `Edits` into stages and runs them. Depends on `core-image`, `color`, `edit-model`.
- `export`: pixels → file bytes. Depends on `core-image` + `image`.

---

## Data flow (M1.1)

```
RAW file
  └─ raw_decode::decode_raw(path, &DecodeOptions::linear_scene())
       → DecodedRaw { image: ImageBuf(LINEAR, sRGB primaries, scene-referred), metadata }
          └─ pipeline::build_pipeline(&Edits::default())   // == [ OutputTransformSrgb ]
               └─ Pipeline::run(image) → ImageBuf(sRGB-ENCODED, 0..=1)
                    └─ export::save_jpeg(&encoded, out.jpg, quality=92)
```

Future edits (M1.2+) insert *before* `OutputTransformSrgb`, operating on the linear image.

---

## Task 1: `color` crate — sRGB transfer functions

**Files:**
- Create: `crates/color/Cargo.toml`, `crates/color/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Add `color` to workspace members**

In root `Cargo.toml`, change the members line to:
```toml
members = ["crates/core-image", "crates/raw-decode", "crates/color", "crates/edit-model", "crates/pipeline", "crates/export", "crates/cli"]
```
(All Task-N crates are added now so the workspace stays loadable as each is created; create the directories in Step 2 of their respective tasks. If a member dir doesn't exist yet, create an empty `src/lib.rs` + minimal `Cargo.toml` as you reach it — do not leave a listed member without a manifest.)

> To keep the workspace loadable between tasks, create each new crate's `Cargo.toml` + a stub `src/lib.rs` immediately when you start its task.

- [ ] **Step 2: Write `crates/color/Cargo.toml`**

```toml
[package]
name = "color"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
```

- [ ] **Step 3: Write the failing test in `crates/color/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn srgb_encode_reference_points() {
        // Exact anchors of the sRGB OETF.
        assert!(close(linear_to_srgb(0.0), 0.0));
        assert!(close(linear_to_srgb(1.0), 1.0));
        // Linear 0.5 -> ~0.7353569 encoded (standard sRGB value).
        assert!(close(linear_to_srgb(0.5), 0.735_356_9));
        // Toe region is linear: 0.0031308 -> 0.0031308*12.92 = 0.04045.
        assert!(close(linear_to_srgb(0.003_130_8), 0.040_45));
    }

    #[test]
    fn srgb_roundtrips() {
        for &v in &[0.0f32, 0.02, 0.18, 0.5, 0.9, 1.0] {
            assert!(close(srgb_to_linear(linear_to_srgb(v)), v));
        }
    }

    #[test]
    fn encode_clamps_out_of_range() {
        assert!(close(linear_to_srgb(-0.2), 0.0));
        assert!(close(linear_to_srgb(1.5), 1.0));
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p color`
Expected: FAIL to compile — `cannot find function linear_to_srgb`.

- [ ] **Step 5: Write the implementation (top of `crates/color/src/lib.rs`)**

```rust
//! Color transfer functions. Working space is linear light (sRGB / Rec.709 primaries);
//! output encoding for display/JPEG is the sRGB OETF.
//!
//! (ICC / lcms2-based profile conversions are deferred until camera input profiles or
//! display output profiles are needed; see the Phase 1 plan's Scope note.)

/// Encode a linear-light sample to sRGB (the sRGB OETF). Input is clamped to `[0, 1]`.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Decode an sRGB-encoded sample back to linear light. Input is clamped to `[0, 1]`.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p color`
Expected: PASS (3 tests).

- [ ] **Step 7: Lint, format, commit**

```bash
cargo fmt --all && cargo clippy -p color --all-targets -- -D warnings
git add crates/color Cargo.toml && git commit -m "feat(color): add exact sRGB transfer functions"
```

---

## Task 2: `core-image` — `map_samples` helper

Stages need a clean way to transform every sample. Add one method, TDD.

**Files:**
- Modify: `crates/core-image/src/lib.rs`

- [ ] **Step 1: Add a failing test (inside the existing `mod tests`)**

```rust
    #[test]
    fn map_samples_applies_to_every_channel() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        img.set_pixel(1, 0, [0.4, 0.5, 0.6]);
        let out = img.map_samples(|s| s * 2.0);
        assert_eq!(out.pixel(0, 0), [0.2, 0.4, 0.6]);
        assert_eq!(out.pixel(1, 0), [0.8, 1.0, 1.2]);
        // dimensions preserved
        assert_eq!((out.width(), out.height()), (2, 1));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p core-image map_samples`
Expected: FAIL — no method `map_samples`.

- [ ] **Step 3: Implement (add to `impl ImageBuf`)**

```rust
    /// Return a new image with `f` applied to every sample (all channels), dimensions preserved.
    pub fn map_samples<F: Fn(f32) -> f32>(&self, f: F) -> ImageBuf {
        let data = self.data.iter().map(|&s| f(s)).collect();
        ImageBuf::from_vec(self.width, self.height, data)
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p core-image`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/core-image && git commit -m "feat(core-image): add map_samples helper"
```

---

## Task 3: `raw-decode` — linear scene-referred decode option

Give the pipeline linear data. Add `DecodeOptions` with two presets and thread it through the backend.

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`, `crates/raw-decode/src/backend_rsraw.rs`, `crates/cli/src/main.rs`

- [ ] **Step 1: Add `DecodeOptions` to `crates/raw-decode/src/lib.rs`**

Add above `decode_raw`:
```rust
/// How libraw should render the RAW.
#[derive(Debug, Clone, Copy)]
pub struct DecodeOptions {
    /// Use the camera's own white balance (falls back to auto-WB).
    pub camera_wb: bool,
    /// If true, output linear light (gamma 1.0). If false, libraw's default display gamma.
    pub linear: bool,
    /// If true, disable libraw's automatic brightness stretch (needed for scene-referred).
    pub no_auto_bright: bool,
}

impl DecodeOptions {
    /// Scene-referred linear working image (what the develop pipeline consumes).
    pub fn linear_scene() -> Self {
        Self { camera_wb: true, linear: true, no_auto_bright: true }
    }
    /// Display-ready sRGB (quick viewable dump — used by `oslr decode`).
    pub fn display() -> Self {
        Self { camera_wb: true, linear: false, no_auto_bright: false }
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self::linear_scene()
    }
}
```

- [ ] **Step 2: Change `decode_raw` signature to take options**

Replace the existing `decode_raw`:
```rust
/// Decode a RAW file to an [`ImageBuf`] (f32 RGB) + metadata, per `opts`.
///
/// With `DecodeOptions::linear_scene()` the samples are linear light in sRGB primaries.
/// With `DecodeOptions::display()` they are sRGB-encoded (display-referred).
pub fn decode_raw(path: &Path, opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::decode(&bytes, opts)
}
```

- [ ] **Step 3: Apply options in `crates/raw-decode/src/backend_rsraw.rs`**

Update the backend `decode` to accept opts and set libraw params before `process`:
```rust
use crate::{DecodeError, DecodeOptions, DecodedRaw, RawMetadata};
use core_image::ImageBuf;
use rsraw::{RawImage, BIT_DEPTH_16};

const U16_MAX_F: f32 = u16::MAX as f32;

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let mut raw =
        RawImage::open(bytes).map_err(|e| DecodeError::Backend(format!("open: {e:?}")))?;
    raw.unpack()
        .map_err(|e| DecodeError::Backend(format!("unpack: {e:?}")))?;
    raw.set_use_camera_wb(opts.camera_wb);

    {
        // Poke libraw output params directly (rsraw's high-level API doesn't expose these).
        let params = &mut raw.as_mut().params;
        params.output_color = 1; // sRGB (Rec.709) primaries
        params.no_auto_bright = u32::from(opts.no_auto_bright);
        if opts.linear {
            params.gamm[0] = 1.0; // linear: power = 1.0
            params.gamm[1] = 1.0; // linear: toe slope = 1.0
        }
    }

    let processed = raw
        .process::<{ BIT_DEPTH_16 }>()
        .map_err(|e| DecodeError::Backend(format!("process: {e:?}")))?;

    let width = processed.width() as usize;
    let height = processed.height() as usize;
    let colors = processed.colors() as usize;
    if colors != ImageBuf::CHANNELS {
        return Err(DecodeError::Format(format!(
            "expected {} colors, libraw returned {colors}",
            ImageBuf::CHANNELS
        )));
    }

    let samples: &[u16] = &processed;
    let expected = width * height * colors;
    if samples.len() != expected {
        return Err(DecodeError::Format(format!(
            "sample count {} != expected {expected}",
            samples.len()
        )));
    }

    let data: Vec<f32> = samples.iter().map(|&s| s as f32 / U16_MAX_F).collect();
    let image = ImageBuf::from_vec(width, height, data);
    let metadata = RawMetadata {
        width,
        height,
        make: raw.make().to_string(),
        model: raw.model().to_string(),
    };
    Ok(DecodedRaw { image, metadata })
}
```

> **Verify:** the field names `params`, `output_color`, `no_auto_bright`, `gamm` come from libraw's `libraw_output_params_t` (bindgen-generated). If any name/type differs, run `find ~/.cargo -path '*rsraw-sys*' -name 'bindings.rs' | xargs grep -n 'no_auto_bright\|output_color\|gamm'` and adjust. If `raw.as_mut()` fails type inference (rsraw doesn't re-export `rsraw_sys`), add `rsraw-sys = "0.1.1"` to `raw-decode`'s deps and annotate the binding.

- [ ] **Step 4: Fix the existing smoke test call site**

In the `tests` module of `lib.rs`, update the decode call:
```rust
        let decoded = decode_raw(&path, &DecodeOptions::linear_scene()).expect("decode should succeed");
```

- [ ] **Step 5: Update `oslr decode` to use display opts (`crates/cli/src/main.rs`)**

In the `decode` fn, change the call to:
```rust
    let decoded = raw_decode::decode_raw(input, &raw_decode::DecodeOptions::display())
        .with_context(|| format!("decoding {}", input.display()))?;
```

- [ ] **Step 6: Build + test**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo test -p raw-decode && cargo build -p cli`
Expected: compiles; raw-decode test passes (skips without `RAW_FIXTURE`).

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/raw-decode crates/cli && git commit -m "feat(raw-decode): add DecodeOptions with linear scene-referred output"
```

---

## Task 4: `edit-model` crate — the `Edits` op-list

The serializable description of edits. Near-empty for M1.1 (proves the sidecar seam); M1.2 adds fields.

**Files:**
- Create: `crates/edit-model/Cargo.toml`, `crates/edit-model/src/lib.rs`
- Modify: root `Cargo.toml` workspace deps (add serde)

- [ ] **Step 1: Add serde to workspace deps**

In root `Cargo.toml` `[workspace.dependencies]` add:
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Write `crates/edit-model/Cargo.toml`**

```toml
[package]
name = "edit-model"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

- [ ] **Step 3: Write the failing test in `crates/edit-model/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_default_roundtrips_json() {
        let edits = Edits::default();
        let json = serde_json::to_string(&edits).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(edits, back);
    }

    #[test]
    fn empty_object_deserializes_to_default() {
        // Forward-compat: an old/empty sidecar still loads.
        let back: Edits = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Edits::default());
    }
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p edit-model`
Expected: FAIL to compile — `cannot find type Edits`.

- [ ] **Step 5: Implement (top of `crates/edit-model/src/lib.rs`)**

```rust
//! The non-destructive edit description (the "op-list" / sidecar model).
//! Serialized to JSON alongside the original; the pipeline is built from it.

use serde::{Deserialize, Serialize};

/// All edits applied to one image. Fields are added as develop stages land (M1.2+).
/// `#[serde(default)]` on every field keeps old sidecars loadable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Edits {
    /// Schema version for future migrations.
    pub version: u32,
}
```

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p edit-model`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy -p edit-model --all-targets -- -D warnings
git add crates/edit-model Cargo.toml && git commit -m "feat(edit-model): add serializable Edits op-list"
```

---

## Task 5: `pipeline` crate — stage trait, output transform, runner

**Files:**
- Create: `crates/pipeline/Cargo.toml`, `crates/pipeline/src/lib.rs`, `crates/pipeline/src/stages/mod.rs`, `crates/pipeline/src/stages/output_transform.rs`

- [ ] **Step 1: Write `crates/pipeline/Cargo.toml`**

```toml
[package]
name = "pipeline"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
core-image = { path = "../core-image" }
color = { path = "../color" }
edit-model = { path = "../edit-model" }
```

- [ ] **Step 2: Write the failing test in `crates/pipeline/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

    #[test]
    fn output_transform_encodes_linear_to_srgb() {
        // A mid-gray linear pixel should become its sRGB-encoded value.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.5, 0.5, 0.5]);
        let out = build_pipeline(&Edits::default()).run(img);
        let [r, g, b] = out.pixel(0, 0);
        let expected = color::linear_to_srgb(0.5);
        assert!((r - expected).abs() < 1e-4);
        assert!((g - expected).abs() < 1e-4);
        assert!((b - expected).abs() < 1e-4);
    }

    #[test]
    fn default_pipeline_has_output_transform_last() {
        let p = build_pipeline(&Edits::default());
        assert_eq!(p.len(), 1); // just the output transform for M1.1
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p pipeline`
Expected: FAIL to compile — missing `build_pipeline`, `Pipeline`.

- [ ] **Step 4: Write the stage trait + runner in `crates/pipeline/src/lib.rs`**

```rust
//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

mod stages;
pub use stages::output_transform::OutputTransformSrgb;

/// One stage of the develop pipeline. Pure: no shared state, independently testable.
/// (A future GPU stage implements the same trait — the seam for the Phase 5 port.)
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, image: ImageBuf) -> ImageBuf;
}

/// An ordered pipeline of stages.
pub struct Pipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl Pipeline {
    pub fn new(stages: Vec<Box<dyn PipelineStage>>) -> Self {
        Self { stages }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Run all stages in order.
    pub fn run(&self, mut image: ImageBuf) -> ImageBuf {
        for stage in &self.stages {
            image = stage.apply(image);
        }
        image
    }
}

/// Build the ordered stage list for a set of edits.
/// M1.1: only the output transform. M1.2+ prepends adjustment stages (they run in
/// linear light, before the output transform).
pub fn build_pipeline(_edits: &Edits) -> Pipeline {
    Pipeline::new(vec![Box::new(OutputTransformSrgb)])
}
```

- [ ] **Step 5: Write `crates/pipeline/src/stages/mod.rs`**

```rust
pub mod output_transform;
```

- [ ] **Step 6: Write `crates/pipeline/src/stages/output_transform.rs`**

```rust
use crate::PipelineStage;
use core_image::ImageBuf;

/// Final stage: encode linear-light samples to sRGB for display/export.
pub struct OutputTransformSrgb;

impl PipelineStage for OutputTransformSrgb {
    fn name(&self) -> &'static str {
        "output_transform_srgb"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_samples(color::linear_to_srgb)
    }
}
```

- [ ] **Step 7: Run to verify pass**

Run: `cargo test -p pipeline`
Expected: PASS (2 tests).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all && cargo clippy -p pipeline --all-targets -- -D warnings
git add crates/pipeline Cargo.toml && git commit -m "feat(pipeline): add PipelineStage trait, runner, sRGB output transform"
```

---

## Task 6: `export` crate — write JPEG/TIFF

**Files:**
- Create: `crates/export/Cargo.toml`, `crates/export/src/lib.rs`

- [ ] **Step 1: Write `crates/export/Cargo.toml`**

```toml
[package]
name = "export"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
core-image = { path = "../core-image" }
image = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write the failing test in `crates/export/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    #[test]
    fn to_rgb8_quantizes_and_clamps() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]); // out of range -> clamped
        let bytes = to_rgb8(&img);
        assert_eq!(bytes, vec![0, 128, 255, 0, 255, 64]);
    }

    #[test]
    fn save_jpeg_writes_a_file() {
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        let path = std::env::temp_dir().join("oslr_export_test.jpg");
        save_jpeg(&img, &path, 90).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        let _ = std::fs::remove_file(&path);
    }
}
```

Note the expected bytes: `0.5*255=127.5` rounds to `128`; `0.25*255=63.75` rounds to `64`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p export`
Expected: FAIL to compile — missing `to_rgb8`, `save_jpeg`.

- [ ] **Step 4: Implement (top of `crates/export/src/lib.rs`)**

```rust
//! Encode a display-referred (sRGB, 0..=1) `ImageBuf` to an image file.

use core_image::ImageBuf;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("image encoding failed: {0}")]
    Encode(#[from] image::ImageError),
}

/// Quantize a display-encoded image to interleaved 8-bit RGB, clamping to `[0, 1]`.
pub fn to_rgb8(image: &ImageBuf) -> Vec<u8> {
    image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// Save as JPEG at the given quality (1..=100). Image must already be display-encoded.
pub fn save_jpeg(image: &ImageBuf, path: &Path, quality: u8) -> Result<(), ExportError> {
    let buf = to_rgb8(image);
    let rgb = image::RgbImage::from_raw(image.width() as u32, image.height() as u32, buf)
        .expect("buffer size matches dimensions");
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).map_err(image::ImageError::IoError)?);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, quality);
    enc.encode_image(&rgb)?;
    Ok(())
}
```

> **Verify:** confirm `image` 0.25's JPEG encoder path (`image::codecs::jpeg::JpegEncoder::new_with_quality` + `encode_image`) against `cargo doc -p image`. If the API differs, the simplest correct fallback is `image::save_buffer_with_format` / `DynamicImage::to_rgb8().save(path)`; adjust and keep the test green.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p export`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy -p export --all-targets -- -D warnings
git add crates/export Cargo.toml && git commit -m "feat(export): add to_rgb8 + save_jpeg"
```

---

## Task 7: `cli render` — end-to-end + validation

**Files:**
- Modify: `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`

- [ ] **Step 1: Add deps to `crates/cli/Cargo.toml`**

Add under `[dependencies]`:
```toml
pipeline = { path = "../pipeline" }
edit-model = { path = "../edit-model" }
export = { path = "../export" }
```

- [ ] **Step 2: Add the `render` subcommand in `crates/cli/src/main.rs`**

Add a variant to `enum Command`:
```rust
    /// Decode a RAW, run the develop pipeline (no edits yet), and export a JPEG.
    Render {
        /// Input RAW file.
        input: PathBuf,
        /// Output JPEG.
        output: PathBuf,
        /// JPEG quality 1..=100.
        #[arg(long, default_value_t = 92)]
        quality: u8,
    },
```
Add the match arm in `main`:
```rust
        Command::Render { input, output, quality } => render(&input, &output, quality),
```
Add the function:
```rust
fn render(input: &Path, output: &Path, quality: u8) -> Result<()> {
    let decoded = raw_decode::decode_raw(input, &raw_decode::DecodeOptions::linear_scene())
        .with_context(|| format!("decoding {}", input.display()))?;
    let m = &decoded.metadata;
    eprintln!("Decoded {}x{}  {} {}", m.width, m.height, m.make, m.model);

    let edits = edit_model::Edits::default();
    let rendered = pipeline::build_pipeline(&edits).run(decoded.image);

    export::save_jpeg(&rendered, output, quality)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("Wrote {}", output.display());
    Ok(())
}
```

- [ ] **Step 3: Build**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p cli`
Expected: builds `oslr` with `decode` and `render`.

- [ ] **Step 4: Manual validation (the M1.1 payoff)**

Run against a real RAW (full-res decode is slow — run in the background):
```bash
./target/debug/oslr render ~/Downloads/Katzen/DSCF6092.RAF /tmp/oslr_render.jpg
```
Then downscale + view:
```bash
sips -Z 500 /tmp/oslr_render.jpg --out /tmp/oslr_render_thumb.png
```
**Done when:** the JPEG is a natural, correctly-white-balanced, correctly-colored photo. It will look **darker** than the Phase-0 `decode` TIFF because `no_auto_bright` is on (scene-referred) — that is correct; exposure becomes a user control in M1.2. Colors and white balance must be right.

- [ ] **Step 5: Commit**

```bash
git add crates/cli && git commit -m "feat(cli): add \`render\` (decode->pipeline->JPEG)"
```

---

## Task 8: M1.1 wrap-up

- [ ] **Step 1: Full workspace check**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```
Expected: all green.

- [ ] **Step 2: Update `CLAUDE.md`**

Update the crate-layout section (mark `color`, `edit-model`, `pipeline`, `export` as created) and the "Current status" section: M1.1 complete — linear decode + PipelineStage pipeline + sRGB output transform + `oslr render` validated; note lcms2 still deferred; next = M1.2 (global adjustments). Commit:
```bash
git add CLAUDE.md && git commit -m "docs: mark M1.1 complete in CLAUDE.md"
```

---

## M1.2–M1.4 Roadmap (to be expanded into full tasks when reached)

### M1.2 — Core global adjustments
Add these stages, each **before** `OutputTransformSrgb`, operating in linear light; each gets a field on `Edits`, a unit test, and a **golden-image regression test** (the safety net established here):
- **White balance** (temp/tint as channel multipliers on linear RGB).
- **Exposure** (linear multiply by `2^ev`).
- **Contrast** (pivot around mid-gray in linear or log).
- **Highlights / shadows / whites / blacks** (tone regions).
- **Tone curve** (point curve, interpolated LUT).
- `build_pipeline` reads each `Edits` field and prepends the corresponding stage in canonical order.
- **Golden tests:** render a committed small RAW (or synthetic linear fixture) with a fixed `Edits` JSON; assert output matches a checked-in PNG within tolerance. Establish the fixture + tolerance harness in this milestone.
- **Done when:** `oslr render --edits edits.json` applies the op-list and golden tests pass.

### M1.3 — Geometry: crop & straighten
- Crop rectangle + rotation stage in `pipeline`; `Edits` gains a `geometry` field.
- Runs early (geometry before tone is fine for MVP; document the ordering choice).
- **Done when:** a cropped/straightened export matches a golden reference.

### M1.4 — Minimal GUI (separate plan)
- New `app` crate: `egui` + `wgpu`. Load RAW, show preview on a `wgpu` canvas, sliders bound to `Edits`, crop tool, histogram, Export button. Edit a **downscaled preview** for interactivity; render full-res on export.
- **This milestone gets its own spec → plan** (distinct subsystem, GUI tech). Do not implement it from this plan.
- **Done when:** open one of your RAWs, adjust with live preview, crop, and export — entirely from the GUI. 🎯 Milestone 1 complete.

---

## Self-Review notes (for the executor)

- **Spec coverage:** M1.1 (plan-of-action lines 34–37) is fully covered by Tasks 1–8: `PipelineStage` trait + op-list (`edit-model`/`pipeline`), decode→linear working space (Task 3), output transform sRGB (Task 5), correct-JPEG zero-edit validation (Task 7). M1.2–M1.4 are roadmapped, not yet task-detailed (by design — see Scope note).
- **Deviations flagged:** lcms2 deferred (hand-coded sRGB OETF suffices while libraw emits sRGB primaries); M1.4 GUI split into its own plan.
- **Type consistency:** `decode_raw(path, &DecodeOptions)` signature is updated at its one call site (cli `decode`) in Task 3 and used by `render` in Task 7. `Edits` (Task 4) is consumed by `build_pipeline` (Task 5) and `render` (Task 7). `map_samples` (Task 2) is used by `OutputTransformSrgb` (Task 5). `to_rgb8`/`save_jpeg` (Task 6) used by `render` (Task 7).
- **The two genuine unknowns** (libraw param field names in Task 3; `image` 0.25 JPEG encoder API in Task 6) each have an inline verify-and-adjust step with a fallback.
- **Done for M1.1:** `oslr render <real.raw> out.jpg` yields a correct-colored natural photo, and `cargo test --all` / `clippy` / `fmt` are green.
```

# Phase 0 — Foundation & De-risking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Cargo workspace and prove the single riskiest capability — decoding a real camera RAW file into linear RGB pixels and writing a viewable image — before building any pipeline or UI on top of it.

**Architecture:** A Cargo workspace of focused crates. Phase 0 creates three: `core-image` (pixel container), `raw-decode` (RAW → linear RGB + metadata via libraw), and `cli` (headless harness that ties decode → export). Decode is isolated behind a small interface so the backend (libraw) can be swapped later. No GUI in Phase 0.

**Tech Stack:** Rust (edition 2021), Cargo workspace, `libraw` C library via Rust bindings for decode, `image`/`tiff` crates for output, `anyhow`/`thiserror` for errors, `clap` for the CLI, `rayon` reserved for later. Dev machine is macOS (Homebrew); production target is Linux first.

---

## File Structure

```
opensource_lightroom/
├── Cargo.toml                      # [workspace] — members + shared deps
├── rust-toolchain.toml             # pin toolchain
├── .gitignore
├── rustfmt.toml
├── .github/workflows/ci.yml        # build + test + clippy + fmt on Linux
├── CLAUDE.md                       # already exists
├── docs/superpowers/plans/         # this plan
├── crates/
│   ├── core-image/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # ImageBuf: dims, f32 RGB planar/interleaved, accessors
│   ├── raw-decode/
│   │   ├── Cargo.toml
│   │   ├── build.rs                # (only if needed to locate libraw)
│   │   └── src/lib.rs              # decode_raw(path) -> DecodedRaw { ImageBuf, RawMetadata }
│   └── cli/
│       ├── Cargo.toml
│       └── src/main.rs             # subcommand: `decode <in> <out.tiff>`
└── tests/
    └── fixtures/                   # sample RAW(s) for smoke tests (may be gitignored if large)
```

**Responsibilities:**
- `core-image`: the pixel data structure everything shares. Zero external-domain deps. Independently testable.
- `raw-decode`: the *only* crate that knows about libraw. Exposes a backend-agnostic function returning `core-image` types + metadata. This is the swap seam (libraw ↔ rawler later).
- `cli`: headless consumer; the permanent test/automation harness. First real end-to-end path.

---

## Task 0: Prerequisites

**Files:** none (environment setup)

- [ ] **Step 1: Verify Rust toolchain**

Run: `cargo --version`
Expected: `cargo 1.8x` or newer. (Confirmed present: 1.85.1.)

- [ ] **Step 2: Install libraw (macOS dev machine)**

Run: `brew install libraw pkg-config`
Then verify: `pkg-config --modversion libraw`
Expected: a version string like `0.21.x`. This makes `libraw.pc` discoverable so the Rust bindings can link.

> Linux note (for later CI/production): the equivalent is `apt-get install libraw-dev pkg-config`. The CI workflow in Task 1 installs it on the runner.

- [ ] **Step 3: Confirm a real RAW file to test with**

We need one real camera RAW for the manual spike validation. Identify a path to one of the user's RAW files (`.CR2`, `.CR3`, `.NEF`, `.ARW`, `.RAF`, `.DNG`, …). Record its absolute path — it will be passed to the CLI in Task 3. If none is handy, a public-domain sample DNG works.

---

## Task 1: Workspace scaffold (M0.1)

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `rustfmt.toml`, `.github/workflows/ci.yml`
- Init: git repository

- [ ] **Step 1: Initialize git**

```bash
cd ~/Projects/opensource_lightroom
git init
```

- [ ] **Step 2: Write the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/core-image", "crates/raw-decode", "crates/cli"]

[workspace.package]
edition = "2021"
license = "GPL-3.0-or-later"
repository = "https://github.com/wladi/opensource_lightroom"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
image = { version = "0.25", default-features = false, features = ["tiff", "jpeg", "png"] }
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 3: Pin the toolchain — `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt"]
```

- [ ] **Step 4: Write `.gitignore`**

```gitignore
/target
**/*.rs.bk
Cargo.lock.orig
.DS_Store
/tests/fixtures/*.CR2
/tests/fixtures/*.CR3
/tests/fixtures/*.NEF
/tests/fixtures/*.ARW
/tests/fixtures/*.RAF
/scratch
```
(Note: `Cargo.lock` IS committed — this workspace produces binaries.)

- [ ] **Step 5: Write `rustfmt.toml`**

```toml
edition = "2021"
```

- [ ] **Step 6: Write the CI workflow — `.github/workflows/ci.yml`**

```yaml
name: CI
on:
  push:
  pull_request:
jobs:
  build-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install system deps
        run: sudo apt-get update && sudo apt-get install -y libraw-dev pkg-config
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --all-targets --all-features -- -D warnings
      - name: Build
        run: cargo build --all
      - name: Test
        run: cargo test --all
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: initialize cargo workspace and CI"
```

---

## Task 2: `core-image` — the ImageBuf type (M0.3, brought forward)

`raw-decode` needs a type to return, so we build the pixel container first. Interleaved `f32` RGB is the working representation; scene-referred linear values live here later.

**Files:**
- Create: `crates/core-image/Cargo.toml`, `crates/core-image/src/lib.rs`

- [ ] **Step 1: Write `crates/core-image/Cargo.toml`**

```toml
[package]
name = "core-image"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
```

- [ ] **Step 2: Write the failing test in `crates/core-image/src/lib.rs`**

Put the type + tests in one file for now. Start with only the test module referencing the intended API:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_image_is_zeroed_and_sized() {
        let img = ImageBuf::new(4, 3);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 3);
        assert_eq!(img.channels(), 3);
        // 4*3*3 samples, all zero
        assert_eq!(img.data().len(), 4 * 3 * 3);
        assert!(img.data().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn set_and_get_pixel_roundtrips() {
        let mut img = ImageBuf::new(2, 2);
        img.set_pixel(1, 0, [0.25, 0.5, 0.75]);
        assert_eq!(img.pixel(1, 0), [0.25, 0.5, 0.75]);
        // untouched pixel stays zero
        assert_eq!(img.pixel(0, 0), [0.0, 0.0, 0.0]);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p core-image`
Expected: FAIL to compile — `cannot find type ImageBuf`.

- [ ] **Step 4: Write the minimal implementation (top of the same `lib.rs`, above the test module)**

```rust
//! Core image container: interleaved f32 RGB, scene-referred linear working space.

/// An interleaved RGB image with `f32` samples in `[0.0, ..]` (linear light).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBuf {
    width: usize,
    height: usize,
    /// Interleaved RGB, length = width * height * 3.
    data: Vec<f32>,
}

impl ImageBuf {
    pub const CHANNELS: usize = 3;

    /// Create a zero-filled image of the given dimensions.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * Self::CHANNELS],
        }
    }

    /// Build directly from an existing interleaved RGB buffer.
    ///
    /// # Panics
    /// Panics if `data.len() != width * height * 3`.
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Self {
        assert_eq!(data.len(), width * height * Self::CHANNELS, "buffer size mismatch");
        Self { width, height, data }
    }

    pub fn width(&self) -> usize { self.width }
    pub fn height(&self) -> usize { self.height }
    pub fn channels(&self) -> usize { Self::CHANNELS }
    pub fn data(&self) -> &[f32] { &self.data }
    pub fn data_mut(&mut self) -> &mut [f32] { &mut self.data }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        (y * self.width + x) * Self::CHANNELS
    }

    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = self.index(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        let i = self.index(x, y);
        self.data[i] = rgb[0];
        self.data[i + 1] = rgb[1];
        self.data[i + 2] = rgb[2];
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p core-image`
Expected: PASS (2 tests).

- [ ] **Step 6: Lint & format**

Run: `cargo fmt --all && cargo clippy -p core-image --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(core-image): add ImageBuf interleaved f32 RGB container"
```

---

## Task 3: `raw-decode` — decode a RAW to linear RGB (M0.2, the spike)

This is the de-risking task. Wrap libraw, decode a real RAW to an `ImageBuf` plus metadata.

> **API-verification note:** libraw Rust bindings exist under a few crate names and their APIs differ. Before writing code, run `cargo search libraw` and open the chosen crate's docs (`cargo doc --open -p <crate>` or docs.rs). Use the actual method names from the installed version. The code below targets the common `libraw` crate shape (`Processor::new().decode(bytes)` → a processed RGB image); adjust names to match reality. If no binding builds cleanly against the installed libraw, fall back to the pure-Rust `rawler` crate (documented in Step 8) — the crate's public function signature stays identical, so nothing downstream changes.

**Files:**
- Create: `crates/raw-decode/Cargo.toml`, `crates/raw-decode/src/lib.rs`

- [ ] **Step 1: Add the binding dependency and inspect its API**

Run:
```bash
cargo add --package raw-decode libraw   # or the crate confirmed by `cargo search libraw`
cargo doc -p libraw --no-deps --open
```
Note the exact call to: load a file/bytes, run the decode/process, and get width, height, and an RGB byte/short buffer.

- [ ] **Step 2: Write `crates/raw-decode/Cargo.toml`**

```toml
[package]
name = "raw-decode"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
core-image = { path = "../core-image" }
thiserror.workspace = true
libraw = "0.0"   # replace with the exact version cargo add pinned
```

- [ ] **Step 3: Write the public interface + error type in `crates/raw-decode/src/lib.rs`**

Define the backend-agnostic surface FIRST — this is the swap seam:

```rust
//! RAW decoding: file on disk -> linear RGB `ImageBuf` + camera metadata.
//! The libraw backend is an implementation detail; the public API is backend-agnostic.

use core_image::ImageBuf;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not read RAW file: {0}")]
    Io(#[from] std::io::Error),
    #[error("libraw failed to decode: {0}")]
    Backend(String),
}

/// Camera/sensor metadata recovered from the RAW.
#[derive(Debug, Clone)]
pub struct RawMetadata {
    pub width: usize,
    pub height: usize,
    pub make: String,
    pub model: String,
}

/// Fully decoded RAW: demosaiced linear-ish RGB plus metadata.
#[derive(Debug, Clone)]
pub struct DecodedRaw {
    pub image: ImageBuf,
    pub metadata: RawMetadata,
}

/// Decode a RAW file to an `ImageBuf` (f32 RGB, 0.0..1.0) and metadata.
pub fn decode_raw(path: &Path) -> Result<DecodedRaw, DecodeError> {
    backend_libraw::decode(path)
}

mod backend_libraw;
```

- [ ] **Step 4: Write the libraw backend `crates/raw-decode/src/backend_libraw.rs`**

Adjust method names to the verified API from Step 1. Shape:

```rust
use crate::{DecodeError, DecodedRaw, RawMetadata};
use core_image::ImageBuf;
use std::path::Path;

pub(crate) fn decode(path: &Path) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;

    // 1. Hand the bytes to libraw and run its default processing (demosaic + WB + sRGB-ish).
    let processor = libraw::Processor::new();
    let processed = processor
        .process_8bit(&bytes)                       // <-- verify exact method
        .map_err(|e| DecodeError::Backend(format!("{e:?}")))?;

    let width = processed.width() as usize;         // <-- verify accessors
    let height = processed.height() as usize;
    let src: &[u8] = &processed;                    // RGB8 interleaved; verify

    // 2. Convert u8 RGB -> f32 [0,1] interleaved.
    let mut data = Vec::with_capacity(width * height * 3);
    for &s in src.iter() {
        data.push(s as f32 / 255.0);
    }
    let image = ImageBuf::from_vec(width, height, data);

    // 3. Pull metadata (make/model). Use sensible fallbacks if the API differs.
    let metadata = RawMetadata {
        width,
        height,
        make: processed.make().unwrap_or_default(),   // <-- verify; else "unknown"
        model: processed.model().unwrap_or_default(),
    };

    Ok(DecodedRaw { image, metadata })
}
```

> If the chosen crate exposes 16-bit output, prefer `process_16bit` and divide by 65535.0 — higher precision, same structure. Keep the `/255.0` path only if 8-bit is all that's available for the spike.

- [ ] **Step 5: Write a smoke test `crates/raw-decode/src/lib.rs` (append test module)**

The test is gated on a fixture existing so CI stays green without committing a large RAW:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> Option<PathBuf> {
        // Set RAW_FIXTURE to an absolute path to run this test locally.
        std::env::var("RAW_FIXTURE").ok().map(PathBuf::from).filter(|p| p.exists())
    }

    #[test]
    fn decodes_fixture_to_nonempty_image() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run");
            return;
        };
        let decoded = decode_raw(&path).expect("decode should succeed");
        assert!(decoded.metadata.width > 0 && decoded.metadata.height > 0);
        assert_eq!(
            decoded.image.data().len(),
            decoded.metadata.width * decoded.metadata.height * 3
        );
        // A real photo is not uniformly black.
        assert!(decoded.image.data().iter().any(|&v| v > 0.01));
    }
}
```

- [ ] **Step 6: Build and run the test**

Run: `cargo test -p raw-decode`
Expected: PASS (test prints "skipping" if no `RAW_FIXTURE` set — that's OK for now). It must at minimum **compile and link against libraw**.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(raw-decode): decode RAW to linear RGB ImageBuf via libraw"
```

- [ ] **Step 8: (Contingency) If libraw bindings won't build**

Swap the backend to pure-Rust `rawler` without changing the public API:
```bash
cargo remove -p raw-decode libraw
cargo add --package raw-decode rawler
```
Rewrite only `backend_libraw.rs` (rename to `backend_rawler.rs`, update the `mod` line) to use `rawler`'s decode + a simple demosaic. `decode_raw`'s signature and all downstream code are untouched. Commit with `feat(raw-decode): use rawler backend`.

---

## Task 4: `cli` — headless decode → TIFF (M0.2 end-to-end)

Ties it together: the permanent test harness. `decode input.raw output.tiff`.

**Files:**
- Create: `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`

- [ ] **Step 1: Write `crates/cli/Cargo.toml`**

```toml
[package]
name = "cli"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[[bin]]
name = "oslr"
path = "src/main.rs"

[dependencies]
core-image = { path = "../core-image" }
raw-decode = { path = "../raw-decode" }
anyhow.workspace = true
clap.workspace = true
image.workspace = true
```

- [ ] **Step 2: Write `crates/cli/src/main.rs`**

```rust
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "oslr", about = "Open-source Lightroom — CLI harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a RAW file and write it as a TIFF.
    Decode {
        /// Input RAW file.
        input: PathBuf,
        /// Output image (.tiff).
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decode { input, output } => decode(&input, &output),
    }
}

fn decode(input: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let decoded = raw_decode::decode_raw(input)
        .with_context(|| format!("decoding {}", input.display()))?;
    let m = &decoded.metadata;
    eprintln!("Decoded {}x{}  {} {}", m.width, m.height, m.make, m.model);

    // f32 [0,1] interleaved RGB -> 8-bit for a viewable TIFF.
    let buf: Vec<u8> = decoded
        .image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();

    image::save_buffer(
        output,
        &buf,
        m.width as u32,
        m.height as u32,
        image::ColorType::Rgb8,
    )
    .with_context(|| format!("writing {}", output.display()))?;

    eprintln!("Wrote {}", output.display());
    Ok(())
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p cli`
Expected: builds `target/debug/oslr`.

- [ ] **Step 4: Manual spike validation (the payoff)**

Run against the real RAW path from Task 0 Step 3:
```bash
cargo run -p cli -- decode /abs/path/to/photo.RAW /tmp/out.tiff
```
Expected: prints correct dimensions + camera make/model, writes `/tmp/out.tiff`.
**Open `/tmp/out.tiff` and confirm it is a recognizable, correctly-colored photo.** This is the M0.2 success criterion.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): add `oslr decode` RAW->TIFF harness"
```

---

## Task 5: Phase 0 wrap-up

- [ ] **Step 1: Full workspace check**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --all
```
Expected: all green.

- [ ] **Step 2: Update `CLAUDE.md` "Current status"**

Change the status section to reflect that Phase 0 is complete: workspace + `core-image` + `raw-decode` (libraw or rawler) + `cli decode` all working; next up is Phase 1 / M1.1 (minimal color-managed pipeline skeleton). Record which decode backend actually shipped.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: mark Phase 0 complete in CLAUDE.md"
```

---

## Self-Review notes (for the executor)

- **Spec coverage:** Task 1 = M0.1 (scaffold+CI). Tasks 3–4 = M0.2 (decode spike, end-to-end). Task 2 = M0.3 (core-image), pulled ahead because `raw-decode` depends on it. All three Phase 0 milestones covered.
- **The one genuine unknown** is the exact libraw binding API (Task 3, Steps 1/4). This is why it's a *spike* and why Step 8 gives a concrete pure-Rust fallback with an identical public interface. Everything downstream (`cli`, future pipeline) depends only on `decode_raw` / `DecodedRaw`, not on libraw.
- **Test realism:** the decode test is a gated smoke test, not a golden-image test. Exact-pixel golden tests arrive in Phase 1 / M1.2 once the color pipeline is deterministic.
- **Definition of done for Phase 0:** `cargo run -p cli -- decode <real.raw> out.tiff` produces a recognizable photo, and `cargo test --all` / `clippy` / `fmt` are green.
```

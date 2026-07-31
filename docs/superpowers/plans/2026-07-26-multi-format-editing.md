# Multi-Format Editing (JPEG / PNG / TIFF) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Rustroom open and edit JPEG/PNG/TIFF (8- and 16-bit) with the existing develop pipeline — via Open file… and the Library grid — with RAW behavior unchanged.

**Architecture:** A new `image-decode` crate decodes non-RAW images into the pipeline's **linear** working space (`image` crate `to_rgb32f` → `color::srgb_to_linear`). The shared `Thumbnail` type moves to `core-image`. `catalog` learns the image extensions and exposes a classifier. The `app` dispatches RAW vs image by file kind in its decode/thumbnail paths; everything downstream is untouched.

**Tech Stack:** Existing workspace + `image` 0.25 (`open`, `to_rgb32f`, `thumbnail`), `color::srgb_to_linear`.

---

## Spec reference

Implements `docs/superpowers/specs/2026-07-26-multi-format-editing-design.md`. CLI (`rustroom decode/render`) stays RAW-only this milestone — out of scope.

## Verified APIs

- `image::open(path) -> ImageResult<DynamicImage>`; `DynamicImage::to_rgb32f() -> Rgb32FImage` (samples in `[0,1]`, **not** linearized); `Rgb32FImage` is `ImageBuffer<Rgb<f32>, Vec<f32>>` → `.width()/.height()/.into_raw()` (interleaved RGB `Vec<f32>`); `DynamicImage::thumbnail(w,h)` + `.to_rgba8()`.
- `color::srgb_to_linear(f32) -> f32` (existing).

## File Structure

```
crates/
├── core-image/     # MODIFY: define the shared `Thumbnail` type
├── raw-decode/     # MODIFY: `pub use core_image::Thumbnail;` (drop local def)
├── image-decode/   # NEW: decode + thumbnail for JPEG/PNG/TIFF (linear working space)
│   ├── Cargo.toml
│   └── src/lib.rs
├── catalog/        # MODIFY: IMAGE_EXTENSIONS; scan includes them; pub is_raw/is_supported
└── app/            # MODIFY: dispatch RAW vs image in thumb_loader + open_path + export
```

---

## Task 1: `core-image` — house the shared `Thumbnail` type

**Files:** Modify `crates/core-image/src/lib.rs`, `crates/raw-decode/src/lib.rs`, `crates/raw-decode/src/backend_rsraw.rs`.

- [ ] **Step 1: Add `Thumbnail` to `core-image` (append to `crates/core-image/src/lib.rs`, before the tests module)**

```rust
/// A decoded, display-referred preview image (interleaved RGBA8).
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}
```

- [ ] **Step 2: Point `raw-decode` at it (`crates/raw-decode/src/lib.rs`)**

Remove the local `pub struct Thumbnail { … }` definition and replace with a re-export near the top (after `use core_image::ImageBuf;`):
```rust
pub use core_image::Thumbnail;
```
(Every existing `raw_decode::Thumbnail` reference, and `crate::Thumbnail` in the backend, keeps working via the re-export.)

- [ ] **Step 3: Build the affected crates**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p core-image -p raw-decode -p app`
Expected: builds (no other code changed — `Thumbnail` fields are identical).

- [ ] **Step 4: Test**

Run: `cargo test -p core-image -p raw-decode`
Expected: PASS (existing tests unaffected).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/core-image crates/raw-decode && git commit -m "refactor: move Thumbnail type into core-image"
```

---

## Task 2: new `image-decode` crate

**Files:** Create `crates/image-decode/Cargo.toml`, `crates/image-decode/src/lib.rs`; modify root `Cargo.toml`.

- [ ] **Step 1: Add to workspace members** (root `Cargo.toml`)

Add `"crates/image-decode",` to `members`.

- [ ] **Step 2: Write `crates/image-decode/Cargo.toml`**

```toml
[package]
name = "image-decode"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
core-image = { path = "../core-image" }
color = { path = "../color" }
image = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 3: Write impl + tests in `crates/image-decode/src/lib.rs`**

```rust
//! Decode non-RAW images (JPEG/PNG/TIFF) into the develop pipeline's working space.
//!
//! An image file is display-referred sRGB; we decode it, normalize to f32 (preserving
//! 8- or 16-bit precision via `to_rgb32f`), and linearize with the sRGB EOTF so the
//! result lives in the same linear-light space as a decoded RAW.

use core_image::{ImageBuf, Thumbnail};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ImageDecodeError {
    #[error("decoding image: {0}")]
    Decode(#[from] image::ImageError),
}

/// A decoded non-RAW image in the linear working space.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub image: ImageBuf,
    pub width: usize,
    pub height: usize,
}

/// Decode `path` to a linear-light `ImageBuf` (sRGB primaries, sRGB assumed on input).
pub fn decode(path: &Path) -> Result<DecodedImage, ImageDecodeError> {
    let rgb32f = image::open(path)?.to_rgb32f(); // samples in [0,1], sRGB-encoded
    let (w, h) = (rgb32f.width() as usize, rgb32f.height() as usize);
    let data: Vec<f32> = rgb32f
        .into_raw()
        .into_iter()
        .map(color::srgb_to_linear)
        .collect();
    Ok(DecodedImage {
        image: ImageBuf::from_vec(w, h, data),
        width: w,
        height: h,
    })
}

/// Make a display thumbnail (decode + downscale to <=512 px). The file is its own preview.
pub fn thumbnail(path: &Path) -> Result<Thumbnail, ImageDecodeError> {
    let img = image::open(path)?.thumbnail(512, 512).to_rgba8();
    Ok(Thumbnail {
        width: img.width() as usize,
        height: img.height() as usize,
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rustroom_imgdec_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            name
        ))
    }

    #[test]
    fn decode_png_linearizes_srgb() {
        // Solid mid-gray sRGB 128 -> linear ~0.216. PNG is lossless so the value is exact.
        let path = temp_path("gray.png");
        let img = image::RgbImage::from_pixel(4, 3, image::Rgb([128, 128, 128]));
        img.save(&path).unwrap();

        let d = decode(&path).expect("decode png");
        assert_eq!((d.width, d.height), (4, 3));
        assert_eq!(d.image.data().len(), 4 * 3 * 3);
        let v = d.image.pixel(0, 0)[0];
        assert!((v - 0.216).abs() < 0.01, "expected ~0.216 linear, got {v}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn thumbnail_is_downscaled() {
        let path = temp_path("big.png");
        image::RgbImage::from_pixel(1200, 800, image::Rgb([200, 100, 50]))
            .save(&path)
            .unwrap();
        let t = thumbnail(&path).expect("thumb");
        assert!(t.width <= 512 && t.height <= 512 && t.width > 0);
        assert_eq!(t.rgba.len(), t.width * t.height * 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_errors() {
        assert!(decode(Path::new("/no/such/rustroom.png")).is_err());
    }
}
```

- [ ] **Step 4: Test**

Run: `cargo test -p image-decode`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p image-decode --all-targets -- -D warnings
git add crates/image-decode Cargo.toml Cargo.lock && git commit -m "feat(image-decode): decode + thumbnail for JPEG/PNG/TIFF (linear working space)"
```

---

## Task 3: `catalog` — image extensions + classifier

**Files:** Modify `crates/catalog/src/lib.rs`.

- [ ] **Step 1: Add image extensions + public classifiers**

Add near `RAW_EXTENSIONS`:
```rust
/// Non-RAW image extensions Rustroom can edit (lowercase, without the dot).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// True if `path` is a camera RAW (needs the libraw decoder).
pub fn is_raw(path: &Path) -> bool {
    has_ext(path, RAW_EXTENSIONS)
}

/// True if Rustroom can open `path` (RAW or supported non-RAW image).
pub fn is_supported(path: &Path) -> bool {
    has_ext(path, RAW_EXTENSIONS) || has_ext(path, IMAGE_EXTENSIONS)
}
```
Replace the existing private `fn is_raw(path: &Path) -> bool { … }` with the above (the old body used the same extension check — now expressed via `has_ext`).

- [ ] **Step 2: Make `scan_folder` include supported images**

In `scan_folder`, change the filter from `is_raw(&path)` to `is_supported(&path)`:
```rust
        if path.is_file() && is_supported(&path) {
```

- [ ] **Step 3: Extend the tests**

Add to `mod tests`:
```rust
    #[test]
    fn scan_includes_images_and_raws() {
        let dir = temp_dir("mixed");
        touch(&dir, "a.raf");
        touch(&dir, "b.JPG");
        touch(&dir, "c.png");
        touch(&dir, "d.tiff");
        touch(&dir, "notes.txt");
        let names: Vec<_> = scan_folder(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        assert_eq!(names, vec!["a.raf", "b.JPG", "c.png", "d.tiff"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn classifiers() {
        assert!(is_raw(Path::new("x.CR2")));
        assert!(!is_raw(Path::new("x.jpg")));
        assert!(is_supported(Path::new("x.jpg")) && is_supported(Path::new("x.NEF")));
        assert!(!is_supported(Path::new("x.txt")));
    }
```

- [ ] **Step 4: Test**

Run: `cargo test -p catalog`
Expected: PASS (existing + 2 new).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p catalog --all-targets -- -D warnings
git add crates/catalog && git commit -m "feat(catalog): recognize JPEG/PNG/TIFF; add is_raw/is_supported"
```

---

## Task 4: `app` — dispatch RAW vs image

**Files:** Modify `crates/app/Cargo.toml`, `crates/app/src/thumb_loader.rs`, `crates/app/src/main.rs`.

- [ ] **Step 1: Add `image-decode` dep** (`crates/app/Cargo.toml` `[dependencies]`)

```toml
image-decode = { path = "../image-decode" }
```
(`catalog` is already a dependency.)

- [ ] **Step 2: Dispatch in `thumb_loader.rs`**

Replace the extraction line inside the worker loop:
```rust
                let thumb = if catalog::is_raw(&path) {
                    raw_decode::extract_thumbnail(&path).ok()
                } else {
                    image_decode::thumbnail(&path).ok()
                };
```
And update the top import (`use raw_decode::Thumbnail;` → use the shared type):
```rust
use core_image::Thumbnail;
```
Add `core-image` and `image-decode` and `catalog` to `app` deps if not present (core-image + catalog already are; add image-decode per Step 1). `raw_decode` is already a dep.

- [ ] **Step 3: Add decode-dispatch helpers to `main.rs`**

Add two free functions (near `preview_edits`):
```rust
/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf` plus a display label. Dispatches by file kind.
fn decode_for_edit(path: &std::path::Path) -> Result<(core_image::ImageBuf, String), String> {
    if catalog::is_raw(path) {
        let d = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::preview())
            .map_err(|e| e.to_string())?;
        let label = format!("{} {}", d.metadata.make, d.metadata.model);
        Ok((d.image, label))
    } else {
        let d = image_decode::decode(path).map_err(|e| e.to_string())?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        Ok((d.image, format!("{ext} {}×{}", d.width, d.height)))
    }
}

/// Full-resolution decode for export. Dispatches by file kind.
fn decode_for_export(path: &std::path::Path) -> Result<core_image::ImageBuf, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene())
            .map(|d| d.image)
            .map_err(|e| e.to_string())
    } else {
        image_decode::decode(path).map(|d| d.image).map_err(|e| e.to_string())
    }
}
```
Add `use core_image;` is unnecessary (path-qualified). Ensure `catalog` and `image_decode` are imported or path-qualified (path-qualified `catalog::` / `image_decode::` works with the crate deps).

- [ ] **Step 4: Use `decode_for_edit` in `open_path`**

Replace the `match raw_decode::decode_raw(&path, …) { Ok(decoded) => { … } Err(e) => … }` structure so the source-specific part is just the decode. Concretely, replace the top of the `Ok` handling:
```rust
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.persist_current_session();
        self.status = format!("Decoding {}…", path.display());
        let (image, make_model) = match decode_for_edit(&path) {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Decode failed: {e}");
                return;
            }
        };
        let buffer = Arc::new(image);
        // Load a sidecar (`<file>.json`) if present.
        let sidecar = path.with_extension("json");
        let edits = if sidecar.exists() {
            edit_model::Edits::from_json_file(&sidecar).unwrap_or_default()
        } else {
            Edits::default()
        };
        let bf = render_once(&buffer, &Edits::default(), PREVIEW_MAX_DIM);
        let baseline = ctx.load_texture(
            "baseline",
            egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
            egui::TextureOptions::LINEAR,
        );
        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(buffer, move || ctx2.request_repaint());
        engine.request(edits.clone(), PREVIEW_MAX_DIM);
        self.session = Some(Session {
            path,
            engine,
            edits: edits.clone(),
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            history: History::new(edits),
            show_before: false,
            crop_mode: false,
            active_handle: None,
        });
        self.status.clear();
    }
```
(This is the existing `open_path` body with the decode line swapped for `decode_for_edit` and `make_model` coming from it. Everything else is identical to what's there now.)

- [ ] **Step 5: Use `decode_for_export` in `export_dialog`**

In the spawned export closure, replace the decode line:
```rust
            let result = (|| -> anyhow::Result<()> {
                let image = decode_for_export(&src).map_err(|e| anyhow::anyhow!(e))?;
                let rendered = pipeline::build_pipeline(&edits).run(image);
                export::save_jpeg(&rendered, &out, 92)?;
                edits.to_json_file(&out.with_extension("json"))?;
                Ok(())
            })();
```
(`decode_for_export` returns `Result<_, String>`; convert to `anyhow` with `anyhow::anyhow!`.)

- [ ] **Step 6: Update the Open file… dialog filter to include images**

In `open_dialog`, widen the file-dialog filter:
```rust
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images",
                &[
                    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "jpg", "jpeg", "png",
                    "tif", "tiff",
                ],
            )
            .pick_file()
        else {
            return;
        };
```

- [ ] **Step 7: Build + manual check**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build --release -p app` then `cargo run --release -p app`:
open a JPEG via **Open file…**, edit it (sliders move the preview), export; open a folder containing a JPEG and confirm its thumbnail shows and double-click opens it into Develop.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all && cargo clippy -p app --all-targets -- -D warnings
git add crates/app && git commit -m "feat(app): open/edit/export JPEG/PNG/TIFF (dispatch RAW vs image)"
```

---

## Task 5: wrap-up

- [ ] **Step 1: Full workspace check**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```
Expected: all green (new: `image-decode` tests, `catalog` classifier tests).

- [ ] **Step 2: Manual acceptance**

Open a JPEG and a PNG (Open file… and via the Library grid); edit with WB/exposure/tone/crop/undo; export a JPEG + sidecar; reopen and confirm the sidecar loads. Open a mixed RAW+JPEG folder — thumbnails for both. RAW behavior unchanged. Clean close.

- [ ] **Step 3: Update `CLAUDE.md`** — add the `image-decode` crate; note the `Thumbnail` move to `core-image`; record multi-format editing (JPEG/PNG/TIFF, sRGB-assumed input, 16-bit precision preserved, RAW unchanged). Update the crate-layout block. Also update `README.md` (mention it edits JPEG/PNG/TIFF, not only RAW). Commit `docs: record multi-format editing`.

---

## Self-Review notes (for the executor)

- **Spec coverage:** `image-decode` decode+thumbnail (Task 2); `Thumbnail` → `core-image` (Task 1); `catalog` extensions + classifier + scan (Task 3); app dispatch in thumb_loader/open_path/export + Open-file filter (Task 4). Covered.
- **Type consistency:** `core_image::Thumbnail` is the single shared type (Tasks 1–2, used by raw-decode, image-decode, thumb_loader). `decode_for_edit -> (ImageBuf, String)` and `decode_for_export -> ImageBuf` (Task 4) consumed by `open_path`/`export_dialog`. `catalog::is_raw`/`is_supported` (Task 3) used by dispatch (Task 4).
- **The genuine unknowns:** `image` 0.25 `to_rgb32f`/`Rgb32FImage::into_raw` shape (Task 2) — verified against the installed source; adjust if `into_raw` differs (fallback: iterate `.pixels()` and read `p.0`). Non-UI logic is unit-tested; the app dispatch is manual-verified.
- **Unchanged by design:** the pipeline, edits, sidecars, crop/undo/before-after, and `raw-decode` all stay as-is — only the decode/thumbnail *entry points* dispatch.
- **Done:** `cargo test --all`/clippy/fmt green; editing a JPEG/PNG/TIFF works end-to-end from the GUI incl. the Library grid; RAW unaffected; clean close.
```

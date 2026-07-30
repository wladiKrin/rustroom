# Multi-Format Editing (JPEG / PNG / TIFF): Design Spec

**Milestone:** Phase 2 side-quest — let Rustroom open and edit non-RAW images (JPEG, PNG, TIFF), not just camera RAWs, reusing the entire existing develop pipeline.

**Status:** Approved design (2026-07-26). Implementation to follow in one plan.

---

## Goal

Open a JPEG / PNG / TIFF — via **Open file…** or by browsing it in the **Library** folder grid — and edit it with the same Develop tools (WB, exposure, tone, crop, undo/redo, before/after) and export as today. RAW support is unchanged.

## Why it's clean (the load-bearing property)

The develop pipeline operates on a **linear-light `ImageBuf`** and is *source-agnostic* — only decoding is RAW-specific. A JPEG/PNG/TIFF is display-referred sRGB; decoding it and running each sample through `color::srgb_to_linear` puts it in exactly the pipeline's working space. Everything downstream — `build_pipeline().run()`, geometry, sidecars, export, the GUI — is unchanged.

## Decisions (from brainstorming)

- **Formats:** JPEG, PNG, TIFF (8- **and** 16-bit).
- **Browsable:** non-RAW images appear in the Library grid alongside RAWs (mixed folders browse fully).
- **Input color space:** non-RAW images are assumed **sRGB** and linearized on load. ICC-aware input / non-sRGB TIFFs are deferred (consistent with the existing lcms2 deferral).
- **Precision:** decode via `image`'s `to_rgb32f()` so 16-bit sources keep their precision (f32), then `srgb_to_linear`.
- **Edits model:** identical to RAW — `<file>.json` sidecar, non-destructive, export to a new file.

---

## Components

### New `crates/image-decode`
Decodes non-RAW images into the pipeline's working space, and makes thumbnails.
- `pub fn decode(path: &Path) -> Result<DecodedImage, ImageDecodeError>` — `image::open(path)?.to_rgb32f()` → per-sample `color::srgb_to_linear` → `ImageBuf` (linear, sRGB primaries). `DecodedImage { image: ImageBuf, width, height }`.
- `pub fn thumbnail(path: &Path) -> Result<Thumbnail, ImageDecodeError>` — `image::open(path)?.thumbnail(512, 512).to_rgba8()` → `Thumbnail` (the file *is* its own preview; no embedded-preview extraction).
- Depends on `image`, `color`, `core-image`.

### `core-image` — move `Thumbnail` here
`Thumbnail { width, height, rgba }` currently lives in `raw-decode`. Move it to `core-image` so both `raw-decode` and `image-decode` return the same type. `raw-decode` re-exports or imports it; `app::thumb_loader` and the grid use `core_image::Thumbnail`.

### `catalog` — learn the image formats
- Add `IMAGE_EXTENSIONS = ["jpg","jpeg","png","tif","tiff"]`.
- `scan_folder` includes both RAW and image extensions (sorted, non-recursive, as today).
- Public classifier so callers dispatch: `pub fn is_raw(path: &Path) -> bool` (make the existing helper public) and `pub fn is_supported(path: &Path) -> bool`.

### `app` — dispatch by file kind
A small helper picks the decoder by extension; **nothing else in the app changes**:
- **Open / edit** (`open_path`): RAW → `raw_decode::decode_raw(path, preview())`; non-RAW → `image_decode::decode(path)`. Both yield an `ImageBuf` (linear) + a display label (RAW: make/model; non-RAW: e.g. `JPEG 4000×3000`). `DecodeOptions` (half-size etc.) apply to RAW only.
- **Export** (`export_dialog`): same dispatch for the full-res source decode; the rest (pipeline → `save_jpeg` → sidecar) is unchanged.
- **Thumbnails** (`thumb_loader`): RAW → `raw_decode::extract_thumbnail` (embedded JPEG); non-RAW → `image_decode::thumbnail` (decode + downscale). Both return `core_image::Thumbnail`.

## Data flow

```
Open photo.jpg (Open file… or double-click in the grid)
   → dispatch: image_decode::decode → ImageBuf(linear, sRGB primaries)
   → same Session → build_pipeline(&edits).run(...) → OutputTransformSrgb → texture
   → <photo>.json sidecar (load on open / save on switch), same crop/undo/before-after
   → Export → dispatch full-res decode → pipeline → save_jpeg + sidecar
Library grid: catalog::scan_folder now lists RAW + JPEG/PNG/TIFF;
   thumb_loader dispatches embedded-preview vs decode-and-downscale.
```

## Error handling

- A file that fails to decode (corrupt, unsupported subtype) surfaces the decoder's error as a status message; in the grid its thumbnail stays a placeholder and it's still listed. One bad file never aborts a folder.
- Unknown/other extensions are simply not listed by `scan_folder` and rejected by `is_supported`.

## Honest caveats (documented)

- **8-bit sources have less editing latitude** — no highlight recovery from clipped data, banding when pushed hard (same as Lightroom editing a JPEG). 16-bit TIFF/PNG fare better.
- **Input assumed sRGB.** A linear or wide-gamut TIFF will look wrong until ICC-aware input (deferred). Acceptable for the common case.
- **No half-size fast path for non-RAW** — full decode (fast anyway); the render engine still downscales for the preview.

## Testing strategy

- `image-decode::decode` — write a tiny synthetic image (a known solid color) to a temp **PNG** (lossless) via the `image` crate, decode it back, assert dimensions and that a mid-gray sRGB pixel of `128/255` (≈0.502 encoded) linearizes to ≈0.216 (`srgb_to_linear(0.502)`), confirming the working space is correct. (A JPEG round-trip would need a tolerance; PNG gives an exact check.)
- `image-decode::thumbnail` — decode a temp image, assert `≤512` px and non-empty RGBA.
- `catalog` — extend tests: `scan_folder` now includes JPEG/PNG/TIFF and still excludes non-images; `is_raw`/`is_supported` classify correctly.
- `core-image::Thumbnail` move — existing `raw-decode` thumbnail test still passes against the moved type.
- Dispatch + GUI — manual smoke run (open a JPEG via Open file… and via the grid; edit; export; check sidecar), as with prior GUI work.

## Crate structure

```
crates/
├── core-image/     # MODIFY: house the shared `Thumbnail` type
├── image-decode/   # NEW: decode + thumbnail for JPEG/PNG/TIFF (linear working space)
├── raw-decode/     # MODIFY: use core_image::Thumbnail (re-export for compat)
├── catalog/        # MODIFY: IMAGE_EXTENSIONS; scan includes them; pub is_raw/is_supported
└── app/            # MODIFY: dispatch RAW vs image in open_path/export/thumb_loader
```

## Done criteria

Open a JPEG/PNG/TIFF (via Open file… and via the Library grid), edit it with the full Develop toolset, export a JPEG + sidecar; mixed RAW+image folders browse with correct thumbnails; RAW behavior unchanged. `cargo test --all` / `clippy --all-targets` / `fmt` green; clean window close.

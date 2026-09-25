# Rustroom

An open-source, cross-platform alternative to Adobe Lightroom — a **non-destructive photo editor** for camera **RAW** files and standard **JPEG / PNG / TIFF** images, written in Rust.

**Linux first**, with Windows and macOS from the same codebase.

> **Status: early development, but genuinely usable.** The GUI (`Rustroom`) browses folders and a
> SQLite catalog, culls with stars/flags/labels/keywords/smart collections, and develops
> non-destructively with a full tone and colour pipeline. See [`CLAUDE.md`](CLAUDE.md) for live status,
> and [`docs/superpowers/`](docs/superpowers/) for per-feature specs and plans.

**What works today**

- **Develop:** white balance, exposure, contrast, highlights/shadows, a point tone curve with
  per-channel RGB, HSL, colour grading, clarity/texture, dehaze, noise reduction, sharpening,
  vignette, crop/straighten, perspective, lens corrections, creative 3-D LUTs, spot removal.
- **Local adjustments:** brush, linear and radial gradients, luminance and colour range masks, with
  Add/Subtract/Intersect composition between them.
- **Library:** folders, a date tree, smart collections, virtual copies, snapshots, undo/redo, presets,
  stacking of RAW+JPEG pairs, Compare and Survey views.
- **Export:** JPEG/TIFF/PNG at 8 and 16 bits, resize, output colour space with real ICC profiles,
  filename templating, watermarking, output sharpening, metadata control, soft proofing.
- **Colour management:** linear ProPhoto working space, camera input profiles from `.dcp`, a display
  profile for the preview, and an output profile at export.
- **GPU:** a `wgpu` compute tail covering 18 of the 21 pipeline stages, on by default.
- **Immich:** browse, download and edit photos from a self-hosted [Immich](https://immich.app) server,
  and upload exports back to it. Entirely optional, behind one setting.

**Not there yet:** import/ingest from a card, a parametric curve, AI subject/sky masks, DNG export,
gamut mapping (out-of-gamut colour is clipped per channel), a folder tree with move tracking, and
persistent (rather than session-only) history.

## How it works

- **Non-destructive:** originals are never modified. An edit is a parameterized entry in an ordered op-list (`Edits`), serialized to a JSON sidecar. Rendering replays the list.
- **Scene-referred, linear, float32 working space.** RAW is decoded (via libraw) to linear light; adjustments run in linear; an sRGB output transform is the final stage.
- **Staged pipeline** behind a `PipelineStage` trait — each stage is an independent, unit-tested `ImageBuf -> ImageBuf` transform, and the seam where a GPU backend slots in later.

## Build & run

Requires a Rust toolchain and libraw.

```bash
# macOS
brew install libraw pkg-config
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"

# Linux (Debian/Ubuntu)
sudo apt-get install libraw-dev pkg-config

cargo build --release
```

```bash
# Decode a RAW to a quick display TIFF
cargo run --release -p rustroom -- decode photo.RAF out.tiff

# Develop a RAW with an edits sidecar and export a JPEG
cargo run --release -p rustroom -- render photo.RAF out.jpg --edits edits.json
```

Example `edits.json`:

```json
{ "exposure": 1.2, "contrast": 25, "shadows": 40, "white_balance": { "temp": 20 } }
```

The GUI is a separate binary:

```bash
cargo run --release -p app        # builds and runs the `Rustroom` binary
```

> **Tip: use `--release` for real use.** RAW decode, the develop preview, and thumbnail
> loading are many times faster than a debug build — e.g. embedded-thumbnail extraction is
> ~2.4 s in debug vs ~0.09 s in release.

## Workspace layout

| Crate | Responsibility |
|---|---|
| `core-image` | `ImageBuf` — interleaved f32 RGB working image; `Thumbnail` |
| `raw-decode` | libraw (via `rsraw`) → linear RGB + metadata; embedded thumbnails |
| `image-decode` | JPEG/PNG/TIFF → linear RGB (via `image` + sRGB→linear); thumbnails |
| `color` | colour transfer functions (sRGB) and Oklab |
| `camera-profile` | camera input profiles from `.dcp` files |
| `lens-profile` | lens corrections (distortion, vignetting, chromatic aberration) |
| `edit-model` | the serializable `Edits` op-list / sidecar |
| `pipeline` | `PipelineStage` trait, develop stages, `build_pipeline` |
| `gpu-pipeline` | `wgpu` compute twins of the pipeline stages |
| `export` | encode a rendered image to a file, with metadata and watermark |
| `xmp` | read and merge-write `stem.xmp` sidecars |
| `catalog` | scan a folder for supported images; the SQLite catalog |
| `immich` | client for a self-hosted Immich server |
| `rustroom` | the `rustroom` command-line app (RAW) |
| `app` | the `Rustroom` desktop app (eframe/egui + wgpu) |

## License

**GPL-3.0-or-later** — see [`LICENSE`](LICENSE) for the full text. Declared in
`[workspace.package]` and inherited by every crate.

# Rustroom

An open-source, cross-platform alternative to Adobe Lightroom — a **non-destructive RAW photo editor**, written in Rust.

**Linux first**, with Windows and macOS from the same codebase. Develop (editing) is the focus; a Library/catalog (DAM) comes later.

> Status: early development. The develop pipeline (decode → linear working space → global adjustments → sRGB export) works end-to-end from the CLI; a GUI is planned. See [`CLAUDE.md`](CLAUDE.md) for live status and architecture, and [`docs/superpowers/plans/`](docs/superpowers/plans/) for the phased plan.

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

## Workspace layout

| Crate | Responsibility |
|---|---|
| `core-image` | `ImageBuf` — interleaved f32 RGB working image |
| `raw-decode` | libraw (via `rsraw`) → linear RGB + metadata |
| `color` | color transfer functions (sRGB) |
| `edit-model` | the serializable `Edits` op-list / sidecar |
| `pipeline` | `PipelineStage` trait, develop stages, `build_pipeline` |
| `export` | encode a rendered image to a file |
| `rustroom` | the `rustroom` command-line app |

## License

GPL-3.0-or-later.

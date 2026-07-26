# CLAUDE.md — Rustroom

Project-specific guidance for Claude Code working in this repository.

**Rustroom** — an open-source alternative to Adobe Lightroom. (Binary: `rustroom`. Repo folder: `~/Projects/rustroom`. Note: the Obsidian planning-notes folder is still named `opensource_lightroom`.)

> **⚠️ Maintain this file.** Whenever the architecture, crate layout, stack, conventions, or current phase meaningfully changes, update the relevant section here in the same session — this file is the durable source of truth loaded every session. Keep it accurate; prune anything that becomes wrong.

## What this is

**Rustroom** is an open-source alternative to Adobe Lightroom (Classic model): a non-destructive RAW photo editor. **Linux first**, then Windows/macOS from one codebase.

- Ambition: a **serious, usable** tool on a focused feature set — not a toy, not a full LR clone.
- Focus order: **Develop (editing) first, Library (catalog/DAM) later.**
- **Reuse proven libraries** (libraw, LittleCMS, lensfun) rather than reimplementing decoding/color science.

## Planning docs (read these for full context)

The detailed spec lives in the user's Obsidian vault at
`~/Documents/obsidian/private/opensource_lightroom/`:

- `01-lightroom-capabilities.md` — target feature set + prioritized P0–P4 scope table.
- `02-architecture-and-tech.md` — stack rationale, pipeline design, crate layout, best practices.
- `03-plan-of-action.md` — phased, step-by-step build order with "Done when" criteria.

## Stack

| Concern | Choice |
|---|---|
| Language | **Rust** (Cargo workspace) |
| RAW decode | **libraw** via bindings (swappable; `rawler` is the pure-Rust alt) |
| Color management | **LittleCMS** (`lcms2`) |
| Lens corrections | **lensfun** (later phase) |
| CPU parallelism | **`rayon`** |
| Image export I/O | **`image`** + jpeg/tiff/png crates |
| GUI | **`egui` + `wgpu`** (Slint reconsidered later) |
| Catalog DB (later) | **SQLite** via `rusqlite` |
| Edit persistence | sidecar **JSON** now → catalog DB later |

**Why Rust over C++/Qt:** cross-platform single codebase, native perf w/o GC, memory/thread safety, and — the deciding factor — `wgpu` is one GPU compute API across Vulkan/Metal/DX12/GL, making the later CPU→GPU port share one abstraction.

## Core architecture (the load-bearing decisions)

1. **Non-destructive.** Originals are never modified. An edit = a parameterized entry in an ordered **op-list**. Rendering replays the op-list. This gives history, virtual copies, and presets almost for free.
2. **Scene-referred, linear, float32 working space.** Math in linear light; display/output transform applied only at the end.
3. **Staged pipeline behind a `PipelineStage` trait.** Each stage is an independent, unit-testable `apply(image, params) -> image`. A stage's CPU impl can later be swapped for a `wgpu` compute shader **without changing callers.**
4. **Interactivity via previews.** Edit downscaled previews for instant feedback; render full-res only on export / 1:1 zoom.

Pipeline order:
```
decode → linearize/black-white → white balance → demosaic →
input color profile → working space (linear f32) →
exposure → tone (highlights/shadows/curve) → color (HSL/grading) →
detail (sharpen/NR) → geometry (crop/lens) → output transform → encode
```

## Planned crate layout (Cargo workspace)

```
opensource_lightroom/
├── Cargo.toml                # [workspace]
├── CLAUDE.md                 # this file
├── crates/
│   ├── core-image/           # ✅ ImageBuf (f32 RGB) + map_samples + map_pixels
│   ├── raw-decode/           # ✅ libraw(rsraw) wrapper; DecodeOptions{linear_scene,display}
│   ├── color/                # ✅ sRGB transfer fns (lcms2/ICC still deferred)
│   ├── edit-model/           # ✅ serde Edits op-list (WB/exposure/contrast/tone/curve)
│   ├── pipeline/             # ✅ PipelineStage + 6 stages + build_pipeline + golden tests
│   ├── export/               # ✅ to_rgb8 + save_jpeg (resize/sharpen later)
│   ├── rustroom/             # ✅ app crate; binary `rustroom` — `decode` (dump) + `render [--edits]`
│   ├── catalog/              # (later) SQLite DAM
│   └── app/                  # (later, own plan) egui + wgpu GUI
├── docs/
└── tests/                    # integration + golden reference images
```

Key rule: **`pipeline`, `color`, `export` have zero GUI dependency** and are driven headless by `cli` and tests.

## Conventions & engineering discipline

- **CI on Linux from day one:** `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check` must stay green.
- **Golden-image regression tests** (established at plan milestone M1.2): render a fixed RAW with a fixed op-list, assert output matches a checked-in reference within tolerance. Never abandon these — they make the GPU port safe.
- **Keep crates small and single-purpose.** A file growing large signals it's doing too much.
- **Color correctness is first-class** — validate against references early.
- **Design the GPU seam now, build it later.** Don't leak CPU-specific types into the pipeline interface.
- Avoid platform-specific assumptions (paths, file dialogs, GPU backend) so Win/macOS stay cheap.

## Current status

**Phase 1 / M1.2 complete.** Core global adjustments + golden regression harness live and validated. Full workspace `fmt`/`clippy`/`test --all` green.

Phase 0 (done): workspace + CI; `core-image` `ImageBuf`; `raw-decode` via **libraw through the `rsraw` crate** (not the `libraw` crate); `rustroom decode` dump validated on a Fuji X-T4 `.RAF`.

M1.1 (done): `raw-decode` `DecodeOptions` (`linear_scene()` = scene-referred linear via libraw gamma 1.0 + `no_auto_bright`, sRGB primaries, camera WB, set through `rsraw`'s `as_mut()` where `no_auto_bright` is `i32`; `display()` = baked sRGB for `rustroom decode`); `color` exact sRGB transfer fns; `edit-model` serde `Edits`; `pipeline` `PipelineStage` trait (pure `ImageBuf->ImageBuf`, the GPU-port seam) + `Pipeline` + `OutputTransformSrgb`; `export` `to_rgb8`/`save_jpeg`; `rustroom render`. **lcms2/ICC still deferred** (libraw emits sRGB primaries → only the transfer curve is needed).

M1.2 (done):
- `core-image` — added `map_pixels` (per-pixel `[f32;3]` transform) for per-channel stages.
- `edit-model` — `Edits` grew fields: `white_balance{temp,tint}`, `exposure` (EV), `contrast`, `highlights`, `shadows`, `whites`, `blacks`, `tone_curve{points}`; all neutral-by-default (`#[serde(default)]`). `Edits::from_json_file`.
- `pipeline` — five new stages, all in linear light before `OutputTransformSrgb`: `WhiteBalanceStage`, `ExposureStage`, `ContrastStage`, `ToneRegionsStage` (hi/sh/wh/bl), `ToneCurveStage` (point-curve LUT). `build_pipeline` adds a stage only when non-neutral; canonical order WB→exposure→contrast→tone-regions→tone-curve→output.
- **Golden-image regression harness** (`crates/pipeline/tests/golden.rs` + committed PNGs): renders a synthetic 4-band chart through fixed `Edits`, compares RGB8 within `TOL=2`. `UPDATE_GOLDEN=1` regenerates. Fast/deterministic (no libraw at test time). **This is the safety net for all future refactors incl. the GPU port — never abandon it.**
- `rustroom` (app crate) — `rustroom render --edits <file.json>` applies a JSON sidecar. **Validated** on the X-T4 `.RAF` (exposure/contrast/shadows/temp): visibly & correctly brighter/warmer/more contrasty than neutral.
- **MVP modeling caveats** (flagged in the M1.2 plan): WB is a *creative* luminance-normalized gain on top of camera WB; contrast/tone-regions are simple hue-preserving models in linear light — not Adobe-identical. Exposure and the curve LUT are exact.

Toolchain: stable rustc 1.97.1 (`image 0.25` needs ≥1.88).

**Dev setup note (macOS):** the `rsraw-sys` build needs libraw discoverable by pkg-config. Install with `brew install libraw pkg-config` and build with:
`export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"`
(On Linux/CI: `apt-get install libraw-dev pkg-config` — no PKG_CONFIG_PATH needed.) The `raw-decode` smoke test is gated on `RAW_FIXTURE=/abs/path.raw`; unset, it skips so `cargo test --all` stays fast/green.

**Next step:** Phase 1 / M1.3 — geometry: crop & straighten. A crop-rectangle + rotation stage in `pipeline`; `Edits` gains a `geometry` field; add a golden test for a cropped/straightened render. Note the ordering choice (geometry vs tone). See the M1.3 roadmap in `docs/superpowers/plans/2026-07-23-phase-1-mvp-editor.md` (expand to full tasks before starting). After M1.3, **M1.4 (egui+wgpu GUI) gets its own spec→plan** — distinct subsystem.

Plans: Phase 0 `docs/superpowers/plans/2026-07-22-phase-0-foundation.md`; M1.1 `.../2026-07-23-phase-1-mvp-editor.md`; M1.2 `.../2026-07-23-phase-1-m1.2-adjustments.md`.

_Update this "Current status" section as milestones land._

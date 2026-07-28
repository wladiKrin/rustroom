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
│   ├── core-image/           # ✅ ImageBuf (f32 RGB) + map_samples/map_pixels/crop/downscale_to
│   ├── raw-decode/           # ✅ libraw(rsraw); DecodeOptions{linear_scene,display,preview(half_size)}
│   ├── color/                # ✅ sRGB transfer fns (lcms2/ICC still deferred)
│   ├── edit-model/           # ✅ serde Edits op-list (WB/exposure/contrast/tone/curve/geometry)
│   ├── pipeline/             # ✅ PipelineStage + 7 stages (incl. geometry) + build_pipeline + golden tests
│   ├── export/               # ✅ to_rgb8 + save_jpeg (resize/sharpen later)
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]`
│   ├── app/                  # ✅ GUI `rustroom-gui` (eframe/egui 0.35 + wgpu). M1.4 done: lib=history/crop_tool/histogram/render_engine
│   └── catalog/              # (later) SQLite DAM
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

**🎯 Milestone 1 complete (Phase 1 / M1.4b done).** Rustroom is a usable single-image RAW editor: open → adjust (live) → crop/straighten → undo/redo → before/after → export. Full workspace `fmt`/`clippy --all-targets`/`test --all` green.

M1.4b (done) — all in `crates/app`:
- `edit-model` — `Edits::to_json_file` (sidecar write).
- `app` lib (unit-tested): `history::History<T>` (undo/redo stack), `crop_tool` (hit-test / drag / aspect-snap on normalized `CropRect`), `render_engine::render_once` (public synchronous render, used for the baseline).
- `app` bin: **undo/redo** (buttons + Ctrl+Z / Ctrl+Shift+Z; one history entry per settled edit via `Response::drag_stopped`), **sidecar** load-on-open (`<file>.json`) / save-on-export, **before/after** toggle (baseline texture from `render_once`), **crop mode** (full-frame preview via `preview_edits`, dimmed overlay + draggable rect from `crop_tool`, aspect presets that snap, Straighten slider).
- **MVP simplifications:** aspect presets *snap* the rect (no fully aspect-locked interactive drag); straighten is a slider; sidecar path is `<file>.json`.
- More egui 0.35 API notes: `Panel::top/right(id).show(ui,..)`; `Response::drag_stopped`; `ui.add_enabled` + `egui::Button::new`; `ui.toggle_value`; painter `rect_filled/rect_stroke(…, egui::CornerRadius::ZERO, …, egui::StrokeKind::Inside)/circle_filled`; `Color32::from_black_alpha`; `InputState::key_pressed(Key::Z)` + `modifiers.command`.

M1.4a (done) — `crates/app`, binary **`rustroom-gui`** (eframe/egui **0.35** on the wgpu backend; the develop pipeline stays CPU and its sRGB output is uploaded as a texture):
- `core-image` — `downscale_to(max_dim)` (area-average, linear).
- `raw-decode` — `DecodeOptions::preview()` = half-size linear decode (fast open); `half_size` set before `unpack()`.
- `app` lib (unit-tested, no egui): `render_engine` (background worker thread; coalesces superseded requests; `Arc<ImageBuf>` → `build_pipeline().run()` → RGBA), `histogram` (`histogram_rgba`).
- `app` bin (egui view, compile+clippy-verified; egui view code isn't unit-testable): Open (rfd → half-size decode), right panel with sliders bound to `Edits` + live histogram, zoom/pan (scroll + drag, Fit button), Reset all, full-res Export (rfd → full decode → pipeline → `save_jpeg`, on a thread).
- **egui 0.35 API notes** (differ from older egui): `eframe::App::ui(&mut self, ui, frame)` replaces `update`; panels are unified `egui::Panel::{top,right,left,bottom}(id).default_size(..).show(ui, ..)` (`TopBottomPanel`/`SidePanel` gone); `CentralPanel` remains; get ctx via `ui.ctx()`.
- **Known M1.4a limitations (by design):** open decode is inline (UI freezes a few s during decode — threading it is a later refinement); export status text isn't cleared on completion; "100%" zoom = 100% of the half-size buffer.

**Phase 1 / M1.3 (done).** Global adjustments + geometry (crop & straighten) + golden regression harness live and validated.

Phase 0 (done): workspace + CI; `core-image` `ImageBuf`; `raw-decode` via **libraw through the `rsraw` crate** (not the `libraw` crate); `rustroom decode` dump validated on a Fuji X-T4 `.RAF`.

M1.1 (done): `raw-decode` `DecodeOptions` (`linear_scene()` = scene-referred linear via libraw gamma 1.0 + `no_auto_bright`, sRGB primaries, camera WB, set through `rsraw`'s `as_mut()` where `no_auto_bright` is `i32`; `display()` = baked sRGB for `rustroom decode`); `color` exact sRGB transfer fns; `edit-model` serde `Edits`; `pipeline` `PipelineStage` trait (pure `ImageBuf->ImageBuf`, the GPU-port seam) + `Pipeline` + `OutputTransformSrgb`; `export` `to_rgb8`/`save_jpeg`; `rustroom render`. **lcms2/ICC still deferred** (libraw emits sRGB primaries → only the transfer curve is needed).

M1.2 (done):
- `core-image` — added `map_pixels` (per-pixel `[f32;3]` transform) for per-channel stages.
- `edit-model` — `Edits` grew fields: `white_balance{temp,tint}`, `exposure` (EV), `contrast`, `highlights`, `shadows`, `whites`, `blacks`, `tone_curve{points}`; all neutral-by-default (`#[serde(default)]`). `Edits::from_json_file`.
- `pipeline` — five new stages, all in linear light before `OutputTransformSrgb`: `WhiteBalanceStage`, `ExposureStage`, `ContrastStage`, `ToneRegionsStage` (hi/sh/wh/bl), `ToneCurveStage` (point-curve LUT). `build_pipeline` adds a stage only when non-neutral; canonical order WB→exposure→contrast→tone-regions→tone-curve→output.
- **Golden-image regression harness** (`crates/pipeline/tests/golden.rs` + committed PNGs): renders a synthetic 4-band chart through fixed `Edits`, compares RGB8 within `TOL=2`. `UPDATE_GOLDEN=1` regenerates. Fast/deterministic (no libraw at test time). **This is the safety net for all future refactors incl. the GPU port — never abandon it.**
- `rustroom` (app crate) — `rustroom render --edits <file.json>` applies a JSON sidecar. **Validated** on the X-T4 `.RAF` (exposure/contrast/shadows/temp): visibly & correctly brighter/warmer/more contrasty than neutral.
- **MVP modeling caveats** (flagged in the M1.2 plan): WB is a *creative* luminance-normalized gain on top of camera WB; contrast/tone-regions are simple hue-preserving models in linear light — not Adobe-identical. Exposure and the curve LUT are exact.

M1.3 (done):
- `core-image` — added `crop(x,y,w,h)`.
- `edit-model` — `Edits.geometry`: `Geometry { angle (straighten deg), crop: CropRect{left,top,right,bottom in [0,1]} }`; `CropRect::default` = full frame, so an absent/partial geometry is neutral.
- `pipeline` — `GeometryStage` (bilinear rotation about center, then crop) runs **first** in `build_pipeline` so adjustments act on final pixels. Straighten keeps canvas size; out-of-frame → black; crop is expected to remove black corners.
- **Golden harness** now uses the *rendered* image's dimensions (so crop cases work); added `crop_center` (32×16) + `straighten_5deg` cases — 9 golden tests total. The 7 pre-existing references are byte-identical.
- **Validated** end-to-end: `rustroom render --edits geo.json` on the X-T4 `.RAF` → correctly cropped to 70% (4372×2919) and rotated 4°.

Toolchain: stable rustc 1.97.1 (`image 0.25` needs ≥1.88).

**Dev setup note (macOS):** the `rsraw-sys` build needs libraw discoverable by pkg-config. Install with `brew install libraw pkg-config` and build with:
`export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"`
(On Linux/CI: `apt-get install libraw-dev pkg-config` — no PKG_CONFIG_PATH needed.) The `raw-decode` smoke test is gated on `RAW_FIXTURE=/abs/path.raw`; unset, it skips so `cargo test --all` stays fast/green.

**Next step:** 🎯 Milestone 1 is done. Next is **Phase 2 / M2.1 — folder browser / filmstrip** (thin catalog): browse a directory of RAWs (grid + filmstrip) with thumbnails, select-to-edit; lightweight in-memory index (no DB yet). Brainstorm + own spec→plan (a new subsystem: `catalog` + GUI browse view). Easy perf win still open: cache the downscaled preview so only the pipeline reruns per edit.

Docs (`docs/superpowers/`): specs/`2026-07-26-m1.4-gui-design.md`. plans/`2026-07-22-phase-0-foundation.md`, `2026-07-23-phase-1-mvp-editor.md` (M1.1), `2026-07-23-phase-1-m1.2-adjustments.md`, `2026-07-23-phase-1-m1.3-crop-straighten.md`, `2026-07-26-m1.4a-core-editor.md`, `2026-07-26-m1.4b-crop-history.md`.

_Update this "Current status" section as milestones land._

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
│   ├── core-image/           # ✅ ImageBuf (f32 RGB) + map_samples/map_pixels/crop/downscale_to + Thumbnail
│   ├── raw-decode/           # ✅ libraw(rsraw); DecodeOptions{linear_scene,display,preview(half_size)}; extract_thumbnail
│   ├── image-decode/         # ✅ JPEG/PNG/TIFF → linear ImageBuf (image + srgb_to_linear) + thumbnail
│   ├── color/                # ✅ sRGB transfer fns (lcms2/ICC still deferred)
│   ├── edit-model/           # ✅ serde Edits op-list (WB/exposure/contrast/tone/curve/geometry)
│   ├── pipeline/             # ✅ PipelineStage + 8 stages (incl. geometry, saturation) + build_pipeline + golden tests
│   ├── export/               # ✅ to_rgb8 + save_jpeg (resize/sharpen later)
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `rustroom-gui` (eframe/egui 0.35 + wgpu). lib=history/crop_tool/histogram/render_engine/thumb_loader
│   └── catalog/              # ✅ scan_folder → RAW+JPEG/PNG/TIFF entries; is_raw/is_supported (in-memory; SQLite DAM later)
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

> **▶ RESUME HERE (fresh session).** Everything below is committed to `main` (local; user pushes to `github.com/wladiKrin/rustroom` themselves — never push without asking). Working tree clean, all checks green. **Commit messages carry no `Co-Authored-By` trailer** (user's explicit preference). **Run the GUI:** `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"` then `cargo run --release -p app` (always `--release`). **Two candidate next tasks (user's pick):** (1) **snappy RAW switching** — thread the `open_path` decode + show the filmstrip thumbnail as an instant placeholder to kill the ~1 s UI freeze on photo switch (top perf item, still pending since M1.4a); or (2) **more color adjustments** — HSL (per-hue hue/sat/lum) or vignette, following the saturation stage as the template. Either is a new feature → brainstorm → spec → plan → executing-plans (all inline, per the workflow used all session). Design/plan docs live in `docs/superpowers/`.

**Saturation & vibrance done (first color adjustments).** One `SaturationStage` (`crates/pipeline/src/stages/saturation.rs`) handles both: per-pixel chroma scaling around the Rec.709 luma in linear light — `out = luma + (rgb - luma) * f_sat * f_vib`, clamped at 0, where `f_sat = 1 + saturation/100` and `f_vib = 1 + (vibrance/100) * (1 - chroma)` with `chroma = (max-min)/max`. So vibrance moves muted pixels most and leaves fully saturated ones alone; a neutral pixel is untouched at any setting; `saturation = -100` collapses to luma (monochrome). `build_pipeline` inserts it **after the tone curve, before the output transform** (color follows tone), only when either param is non-zero. `Edits.saturation` / `Edits.vibrance` (serde-defaulted → old sidecars load unchanged); two sliders after Blacks in the Develop panel, which inherit undo/redo, sidecar and export for free. Goldens now 11: `saturation_plus50` on the existing chart plus `vibrance_plus50` on a **new `muted_chart()`** (the main chart is pure primaries/gray, where vibrance is a no-op by construction) — `check_golden_with(name, edits, chart)` was added for that; the 9 pre-existing references stayed byte-identical. **Caveat (MVP model):** negative vibrance uses the same `(1 - chroma)` weighting, so it desaturates muted colors *more* than saturated ones — simple and symmetric, not Adobe-identical. Validated end-to-end via `rustroom render --edits` on the X-T4 `.RAF`.

**Filmstrip done (browse-layout toward "C").** On top of multi-format + M2.1/M2.2. Full workspace `fmt`/`clippy --all-targets`/`test --all` green.

Filmstrip (done, `app` only): a bottom `egui::Panel` in Develop rendering the loaded folder's thumbnails (reuses `Library` state — no new loader). Click a cell or ←/→ to switch photos via `open_entry` (auto-saves the outgoing photo's sidecar). Arrow-nav gated on `ctx.memory(|m| m.focused())` so arrows still fine-tune a focused slider. `open_entry` sets `library.selected` so grid + strip stay in sync. `show_filmstrip` toggle in the Develop toolbar (shown only when a folder is loaded); single-file opens show no strip. **Resizable** (drag top border; thumbnails scale with panel height).

Browse-polish fixes (done, from filmstrip feedback):
- `Open file…` now clears `self.library` → true single-file mode (no stale strip/grid).
- **Sidecar naming appends `.json`** to the full filename (`sidecar_path`: `photo.RAF` → `photo.RAF.json`), so a RAW and JPEG sharing a basename no longer clobber each other's edits. (Old `<name>.json` sidecars won't be found under the new scheme — fine for early dev.)
- Filmstrip is drag-resizable with height-scaled thumbnails. **egui gotcha (fixed):** a resizable `Panel` stores its *content's* rendered rect as its size each frame — if content is smaller than the panel on the resize axis it shrinks back every frame. Fix: `ui.set_min_height(ui.available_height())` (bottom panel) so content fills the panel. Applies to any future resizable panel whose content doesn't fill the resize axis.
- **Deferred (the one open item from this feedback round):** background-thread decode + instant thumbnail placeholder so RAW photo-switching isn't a ~1 s UI freeze (the half-size decode still runs *inline* on the UI thread in `open_path`). This is the M1.4a "thread the open" refinement, still pending; it'd also make first-open snappier. See "Next step".

**Multi-format editing done (JPEG/PNG/TIFF), on top of Phase 2 / M2.1+M2.2.**

Multi-format (done): **new `image-decode` crate** decodes JPEG/PNG/TIFF to the *same linear working space* as RAW (`image::open().to_rgb32f()` → `color::srgb_to_linear`; 16-bit precision preserved). `Thumbnail` moved to `core-image` (shared by both decoders). `catalog` now lists RAW + JPEG/PNG/TIFF and exposes `is_raw`/`is_supported`. The `app` dispatches RAW vs image by extension in `decode_for_edit`/`decode_for_export`/`thumb_loader` — **everything downstream (pipeline, edits, sidecar, crop/undo/before-after, export) is unchanged**, which is the payoff of the source-agnostic linear pipeline. Caveats (documented): non-RAW input assumed **sRGB** (ICC deferred); 8-bit sources have less latitude. CLI stays RAW-only.

**Phase 2 / M2.1 (folder browser) + M2.2 (thumbnail perf + lossless switching) complete.** Rustroom opens a folder and browses it.

M2.1 (done):
- **New `catalog` crate** — `scan_folder(dir) -> Vec<CatalogEntry { path, file_name }>` (RAW extensions, sorted; non-recursive; in-memory, no DB). Unit-tested.
- `raw-decode` — `extract_thumbnail(path) -> Thumbnail { width, height, rgba }`: pulls the camera's **embedded JPEG** via rsraw `extract_thumbs` (no demosaic → fast), decodes via the `image` crate. Bitmap-format thumbs handled too.
- `app` lib — `thumb_loader::ThumbLoader` (background worker; streams `ThumbResult { index, thumb }` off the UI thread, `drain()` per frame).
- `app` bin — `AppView { Library, Develop }` split (editor moved into `develop_toolbar`/`develop_body`). **Open folder…** → `catalog::scan_folder` → grid of thumbnails that fill in progressively over gray placeholders; click selects, ←/→ move, **double-click / Enter** opens into Develop (shared `open_path`). Library/Develop toggle; **Open file…** still jumps straight to Develop.
- **Deferred (toward "C" / later):** filmstrip in Develop, on-disk thumbnail cache, edit-accurate rendered thumbnails, ratings/sorting/subfolders. Thumbnails show the **camera** rendering, not Rustroom edits (embedded JPEG).

M2.2 (done — fixes from M2.1 feedback):
- `raw-decode::extract_thumbnail` now **downscales to ≤512 px** (`image` `thumbnail`), so a 13 MP embedded preview → ~0.7 MB instead of ~52 MB per grid cell (prevents GPU/RAM blowup on big folders). Note: debug per-thumb time is decode-bound (~2.4 s); **release ≈ 0.09 s** — always run `--release` for real use (noted in README).
- **Lossless photo switching:** `RustroomApp::persist_current_session` auto-writes the `<file>.json` sidecar (only when `edits != default`) before opening another photo (`open_path`) and on window close. Combined with load-on-open, switching photos never loses edits — no "discard?" dialog needed (fits the non-destructive model). Minor edge: edit-then-reset-to-default before switching leaves a stale sidecar.

**🎯 Milestone 1 (Phase 1 / M1.4b) complete.** Rustroom is a usable single-image RAW editor: open → adjust (live) → crop/straighten → undo/redo → before/after → export.

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

**Next step:** **rounded-out Develop** — saturation/vibrance landed; HSL (per-hue) and vignette are the natural follow-ups, using `stages/saturation.rs` as the template. Open perf wins: thread the `open_path` decode (the ~1 s freeze on photo switch) and cache the downscaled preview so only the pipeline reruns per edit (the sub-second slider lag). Filmstrip toward "C" is done; further "C" polish (vertical strip, ratings) deferred.

Docs (`docs/superpowers/`): specs/`2026-07-26-m1.4-gui-design.md`, `2026-07-26-m2.1-library-browser-design.md`, `2026-07-26-multi-format-editing-design.md`, `2026-07-26-filmstrip-design.md`, `2026-08-04-saturation-vibrance-design.md`. plans/`2026-07-22-phase-0-foundation.md`, `2026-07-23-phase-1-mvp-editor.md` (M1.1), `2026-07-23-phase-1-m1.2-adjustments.md`, `2026-07-23-phase-1-m1.3-crop-straighten.md`, `2026-07-26-m1.4a-core-editor.md`, `2026-07-26-m1.4b-crop-history.md`, `2026-07-26-m2.1-library-browser.md`, `2026-07-26-m2.2-thumbnail-perf-sidecar.md`, `2026-07-26-multi-format-editing.md`, `2026-07-26-filmstrip.md`, `2026-08-04-saturation-vibrance.md`.

_Update this "Current status" section as milestones land._

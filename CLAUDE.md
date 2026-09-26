# CLAUDE.md — Rustroom

Project-specific guidance for Claude Code working in this repository.

**Rustroom** — an open-source alternative to Adobe Lightroom. (Binary: `rustroom`. Repo folder: `~/Projects/rustroom`. Note: the Obsidian planning-notes folder is still named `opensource_lightroom`.)

> **⚠️ Maintain this file — and keep it SMALL.** Update architecture, crate layout, stack, conventions
> and status here when they meaningfully change; **the history goes in `docs/JOURNAL.md`, not here.**
> This file loads in full every session, and it reached **1.2 MB (~337,000 tokens)** on 2026-09-16 by
> accumulating a session-by-session narrative — more than a whole context window before any work starts.
>
> **⚠⚠ The rule that keeps it small: NEVER append a superseded block.** Overwrite the live state in
> place and move the old version to the journal. Twenty-four *"⚠ Superseded"* blocks had stacked up
> here, every one already recorded in git history. **Target: under ~400 lines.**

## What this is

**Rustroom** is an open-source alternative to Adobe Lightroom (Classic model): a non-destructive RAW photo editor. **Linux first**, then Windows/macOS from one codebase.

- Ambition: a **serious, usable** tool on a focused feature set — not a toy, not a full LR clone.
- Focus order: **Develop (editing) first, Library (catalog/DAM) later.**
- **Reuse proven libraries** (libraw, LittleCMS, lensfun) rather than reimplementing decoding/color science.

## Planning docs (read these for full context)

The detailed spec lives in the user's Obsidian vault at
`~/Documents/obsidian/private/opensource_lightroom/`.

⚠ **Reaching it needs TWO permissions, and they are separate — 2026-08-26.**
1. **The harness's directory scoping** — `mcp__ccd_directory__request_directory` with the vault path grants it; the user approves in-session.
2. **macOS TCC** — Full Disk Access for the app Claude Code launches from, System Settings → Privacy & Security, then a full quit-and-reopen. **Granted on this machine as of 2026-08-26.**

⚠ The one-call check, no permission prompt, that tells you which (if either) is missing:
```
/bin/ls ~/Documents 2>&1 | head -3
```
`Operation not permitted` means TCC; a harness refusal names the path instead. ⚠ Do not conclude from a successful folder grant that the file is readable — the grant reports success while TCC still denies it, which cost a wrong diagnosis here.

- `01-lightroom-capabilities.md` — target feature set + prioritized P0–P4 scope table. **Annotated 2026-08-26** with ✅/🟡/⬜/⛔ per capability, plus a **"deliberate divergences from Lightroom"** section (copy numbering from 0, radial mask affects the inside, no Smart Previews, post-crop vignette by stage order, panel grouped by edit group).
- `02-architecture-and-tech.md` — stack rationale, pipeline design, crate layout, best practices. **Corrected 2026-08-26** — ⚠ four things it got wrong are struck through in place: the **pipeline order** (geometry runs *first*), the **working space** (linear ProPhoto), the **stage trait** (consuming, params baked in at construction), and the **crate layout** (12, not 8). Its §8 lists what the note did not anticipate at all.
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
PREFIX: lens → noise reduction → dehaze → spots → geometry (orient·keystone·straighten·crop, ONE resample) → masked NR/dehaze
TAIL:   white balance → exposure → tone (contrast/regions/curve) → clarity/texture →
        color (saturation/HSL/grading) → masks → vignette → sharpen → camera rendering/LUT → output transform → encode
```

**Geometry ends the prefix and precedes every adjustment**, so the tail acts on the final cropped
pixels, which is also what makes the vignette post-crop for free. ⚠⚠ **Since M7.3b it is ONE
resample** (the keystone fused in, +17.6% acutance) **and it comes AFTER lens, NR, dehaze and
spots**, so a straighten, crop or keystone drag re-runs only that resample (137 → 35 ms at fit).
⚠⚠ That order is a **measured speed trade, not an image-quality win**: "NR sees cleaner noise ahead
of the resamplers" was measured and retracted. Spots live post-lens, **pre-perspective**
(`SpotMap`); brushes still live post-perspective (`FrameMap`). `grep -n 'M7.3b' docs/JOURNAL.md`.

**NR early, sharpening late** — the two halves of "detail" belong at opposite ends. Denoising
the source is far easier than denoising after Exposure/Shadows/the tone curve have amplified
the grain, and it prevents HSL chroma speckle instead of smoothing it after the fact.
Sharpening is last because it must not amplify anything a later stage would then stretch.

## Planned crate layout (Cargo workspace)

```
opensource_lightroom/
├── Cargo.toml                # [workspace]
├── CLAUDE.md                 # this file
├── crates/
│   ├── core-image/           # ✅ ImageBuf (f32 RGB) + Capture (EXIF facts, both decoders) + map_samples/map_pixels/map_pixels_xy/to_rgb8/to_rgba8 (rayon-parallel) /crop/downscale_to + Thumbnail
│   ├── raw-decode/           # ✅ libraw(rsraw); DecodeOptions{linear_scene,display,preview(half_size),demosaic}; Demosaic tiers -> user_qual; extract_thumbnail
│   ├── image-decode/         # ✅ JPEG/PNG/TIFF → linear ImageBuf (image + srgb_to_linear) + thumbnail; EXIF orientation applied in both
│   ├── color/                # ✅ sRGB transfer fns + Oklab (lcms2/ICC still deferred)
│   ├── edit-model/           # ✅ serde Edits op-list (orientation/WB/exposure/contrast/tone/curve/saturation/hsl/local_contrast/dehaze/NR/sharpening/vignette/geometry) + EditGroups partition + merge_from
│   ├── pipeline/             # ✅ PipelineStage + 14 stages (incl. geometry, noise_reduction, dehaze, saturation, hsl, local_contrast, vignette, sharpening) + spatial::{box_blur, gaussian_blur, wide_blur, min_filter, guided_filter} + build_pipeline / build_pipeline_split(_in_region) / build_prefix_steps + PrefixStep/PrefixStepKey/PrefixKey + Region + tail_kernel_radius + golden tests
│   ├── export/               # ✅ to_rgb8 + save (JPEG/PNG/TIFF) + metadata::{ExportMetadata, FileMetadata, MetadataScope} — the full XMP packet (M6.2) + watermark + output_sharpen
│   ├── xmp/                  # ✅ stem.xmp read / merge-write: rating, label, keywords, title/caption/rights/creator (M6.3 + M6.2); zero GUI, zero catalog
│   ├── immich/               # ✅ Immich client (zero GUI, `ureq`): search/buckets/album/stacks/**upload** (+ `multipart.rs`, hand-rolled — ureq has none), `Asset`/`AlbumSummary`/`Bucket`/`Upload`. ⚠ DTOs deliberately narrow; nullable-vs-required audited against a *captured* reply
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs is **1,147 total but only 78 of code** (2026-09-24; it said 681, then 1,116 — **measure by code, do not quote either number**); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. ⚠⚠ **THE MODULE LIST THAT USED TO SIT HERE IS GONE ON PURPOSE.** It enumerated ~50 of the **152 `.rs` files** under `crates/app/src` and had silently gone half-stale — it was missing `multi/{compare,survey,pane}`, `menu`, `menu_bar`, `native_menu`, `commands`, `tone_curve_tool`, `brush_tool`, `mask_tool`, `looks`, `marks` and ~40 more, which is worse than no list because this one is the *placement guide*. **`ls crates/app/src/` is the inventory; what belongs here is the RULE:** `lib.rs` declares **73** modules and **not one of them uses egui** — verify with `grep -lE '^\s*use .*egui|egui::' $(...)`, which returns empty — while `main.rs` declares **33** bin modules and egui lives only there. Logic goes in the lib, widgets in the bin.
│   └── catalog/              # ✅ scan_folder → RAW+JPEG/PNG/TIFF entries (+ mtime); SortOrder/sort (name/modified/edited, case-insensitive, name tie-break); is_raw/is_supported (in-memory; SQLite DAM later) + Description per file (v7, M6.2)
│   ⚠⚠ THE TREE ABOVE LISTS 12 OF 15 CRATES — it omits `gpu-pipeline`, `lens-profile` and `camera-profile`, and did so while also claiming to omit `immich`, which it lists. **`ls crates/` is the answer; this tree is a sketch.**
├── docs/
└── tests/                    # integration + golden reference images
```

Key rule: **`pipeline`, `color`, `export` have zero GUI dependency** and are driven headless by `cli` and tests.

## Conventions & engineering discipline

- **CI on Linux from day one:** `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check` must stay green.
- **Golden-image regression tests** (established at plan milestone M1.2): render a fixed RAW with a fixed op-list, assert output matches a checked-in reference within tolerance. Never abandon these — they make the GPU port safe.
- **Keep crates small and single-purpose.** A file growing large signals it's doing too much.
- **⚠⚠ DECIDE WHERE NEW CODE GOES *BEFORE* WRITING IT — never dump into `main.rs` and leave the split for later** (standing instruction, 2026-09-01). Every oversized file in this repo got that way one reasonable-seeming addition at a time; `main.rs` reached **4,375 lines** and `canvas.rs` **1,890** exactly like that. An agent that adds 300 lines to `main.rs` has not finished its task, it has moved work onto whoever splits it next. **The full placement table is in `docs/PARALLEL-WORK.md` §5**; the short form:
  - **Crate first, and it is a hard rule.** `pipeline`/`color`/`export`/`core-image`/`edit-model`/`catalog`/decoders have **zero GUI dependency**. `crates/app/src/lib.rs` and its modules have **no egui** — that is where unit-testable logic lives. egui belongs only in the bin modules (`theme`, `modals`, `develop`, `library`, `filmstrip`, `thumb`, `shell`, `rail`).
  - **A stage** → `pipeline/src/stages/<name>.rs`, one per file. **An edit parameter** → `edit-model`, *plus* its `EditGroups` group, `merge_from`'s destructure and the three exhaustive fixtures. **Logic behind a gesture** → a lib module taking plain types (the `crop_tool`/`mask_tool`/`panel_order` pattern); **the gesture itself** → the bin module.
  - **Two questions that catch most misplacements:** *can this be tested without opening a window?* (if yes and it is in a bin module, it is in the wrong place) and *would a second caller want this?* (if yes it is a lib module, not a private helper).
  - ⚠ **If a file you are editing already exceeds ~600 lines, say so in your report** rather than silently adding to it.
- **⚠⚠ ONE CARGO INVOCATION MODE PER SESSION — measured 2026-09-23, and it is the whole reason the
  gate ever felt slow.** A **warm** `cargo test --workspace` is **1 second**. Alternating it with
  `cargo test -p <crate>` costs **43–531 s every switch**, because `-p` resolves features for one
  crate's subgraph while `--workspace` unifies them across all members — different fingerprints, so
  each switch rebuilds. **Filter by test name, never by package:**
  `cargo test --workspace immich_source`. ⚠ Give clippy its own directory for the same reason — a
  different rustc wrapper evicts rustc's own artifacts:
  `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings`.
  ⚠ Two hypotheses were measured and **wrong**: test execution (the whole `lens-profile` suite runs
  in **2.4 s**, the goldens in **0.30 s**, so `opt-level` buys nothing) and fingerprint cost (1 s warm
  over 2.39 M files). The cost was **cold filesystem work**: `target/debug` had reached **85 GB**, and
  a fresh binary's `_dyld_start` wait scales with its size. Fixed by
  `[profile.dev] debug = "line-tables-only"` — cold run **437 s → 11 s**, tree **90 → 10 GB**.
  ⚠ A `SessionStart` hook (`scripts/target-budget.sh`, wired in the untracked
  `.claude/settings.local.json`) warns when `target/` passes **250,000 files** — a healthy tree here
  is ~37k, and the pathological one was **2.39 M**, of which ~98.5% were stale because **Cargo never
  garbage-collects**. It counts files rather than bytes, bounded by `find | head`, so it costs at
  most ~1.3 s however large the tree is; a `du` would be slowest exactly when it most needs to run.
  ⚠⚠ **THE WHOLE GATE IS ~79 SECONDS** (fmt instant, clippy 62 s cold, tests 16 s). Any document
  saying it is 15–30 minutes predates 2026-09-23 and is wrong; so is the 2026-09-19 journal note
  calling the stall "not fixable from here".
- **Color correctness is first-class** — validate against references early.
- **Design the GPU seam now, build it later.** Don't leak CPU-specific types into the pipeline interface.
- Avoid platform-specific assumptions (paths, file dialogs, GPU backend) so Win/macOS stay cheap.

## Current status

> **▶ RESUME HERE (fresh session).**
>
> **FIRST: check the real state before anything else** — this block goes stale the moment the user pushes.
>
> ```
> git branch --show-current && git log main..HEAD --oneline && git log origin/main..main --oneline
> ```
>
> Run that command rather than trusting any state written here. If a branch shows up ahead of `main`, do not start the next feature on top of it — ask whether to merge, **and never push either way; the user pushes to `github.com/wladiKrin/rustroom` himself.**
>
> **⚠ Conventions that are not negotiable:** commit messages carry **no `Co-Authored-By` trailer**; **branch per feature** without asking; **never push**.
>
> **Run the GUI:** `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"` then `cargo run --release -p app` — always `--release`, in the background, and **launch it after every big update without being asked**, saying what to look at. Skip it for small tweaks in a tuning loop.
>
> **⚠ Workflow:** every feature goes brainstorm → spec → plan → executing-plans, **inline** (the user has chosen inline every single time; offer the choice but expect that answer). Specs land in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`.
>
> **⚠ The history is in `docs/JOURNAL.md`** — 94 dated sections, newest first, every milestone's measurements, retractions and uncaught sabotages. **Grep it, never read it whole:** `grep -n '^### ' docs/JOURNAL.md` for the index, `grep -n 'M8\.11' docs/JOURNAL.md` for one milestone.

### State (2026-09-26)

**2977 tests**, **79 goldens** (⚠ 79: M5.1c added one; "78" in older notes is stale).
✅ **The resident final checkpoint (2026-09-26, branch `resident-final`, stacked on
`per-pixel-fusion`; confirmed by use and merged 2026-09-26, the whole stack together — "very smooth").** `PrefixCache` keeps the tail's input on the GPU under
`(final key, Option<Rect>)`; a one-chain tail runs from it, a miss keeps its upload, the last GPU
prefix step keeps its output, and **the settle after a handoff is a download (~101–122 → ~21–30
ms), not a render**. The tail's input travels as an `Arc` — the 2.5 ms clone per tail frame is gone.
⚠⚠ **The drag frame is ~3 ms slower** (the tail copies the kept output instead of taking the one-shot
plane in place); the fix — the first fused unit reading from the resident, a fourth binding on the
emitter — is priced in the journal and needs its own spec. Measured A/B against a worktree build of
`0877bb2` because the sidecar changed under the day's figures. `grep -n 'resident final' docs/JOURNAL.md`.
✅ **Per-pixel fusion (2026-09-26, branch `per-pixel-fusion`,
stacked on `gpu-timestamps`; in use with the resident final since 2026-09-26, no regression felt).** A run of consecutive per-pixel
stages is ONE walk on the CPU (`pipeline::per_pixel`, bit-identical) and ONE dispatch on the GPU
(`gpu_pipeline::per_pixel`, three bindings); grouping is at **run time**, the stage list is
untouched; `RUSTROOM_NO_FUSION=1` is the A/B. Measured: the GPU whole tail **35.6 → 21.2 ms** at
rung 3200, the author's sidecar drag 25.0 → 21.6, a Straighten frame ~30 → ~28. ⚠⚠ **The walk
calls a stage once per ROW** — per pixel it read 200 → 299 ms on the CPU (61 M virtual calls);
row-wise 177.6 → 172.0. ⚠ Three drag pairs at load 6–11 read fused 2–4 ms slower before the
row-wise fix, unexplained; use decides. `grep -n 'per-pixel fusion' docs/JOURNAL.md`.
✅ **GPU timestamps (same day, branch `gpu-timestamps`):** `RUSTROOM_DEBUG_GPU_TIME=1` prints a
`GPUTIME` block per chain — one compute pass per unit, its GPU time, the quantise, and the wall's
remainder as `copies+latency`. ⚠⚠ **The copies are not marked: an encoder timestamp outside a
pass deadlocks wgpu 29's Metal backend.** ⚠ A timed frame is ~10 ms slower; quote per-pass
numbers, never a timed wall. A full-frame per-pixel pass is ~2.2 ms at 6.8 Mpx, the Catmull-Rom
geometry gather 15.7. `grep -n 'GPU timestamps' docs/JOURNAL.md`.
✅ **M7.3b (2026-09-25, confirmed by use and merged 2026-09-26)**
fuses perspective into `GeometryStage`, reorders the prefix, and moves spots before perspective.
`grep -n 'M7.3b' docs/JOURNAL.md`. ⚠⚠ **Its drag speedup applies only when NR or dehaze is on**,
which is 2 and 1 of the author's 49 edited photos. Quoting "4x faster geometric drags" without that
condition overstated it, and the author noticed.
⚠ `Edits::version` is **never written** (always 0); do not trust any note giving it a value.

✅ **Stacks — same-stem files as one grid cell (2026-09-25, confirmed by use).** `Pictures/fuji` was
**830 cells for 415 photographs**; both camera folders are **100% RAW+JPEG pairs**. A **view grouping
only** — edits, sidecars, marks and the catalog untouched. ⚠⚠ **Grouping happens in
`library_state::browser_entries`, the function BOTH entry-building paths must go through** — `sources`
on folder-open and `rebuild_entries` on re-sort. A comment claiming one site is not one site; that bug
shipped. ⚠ `smart_sources` deliberately does **not** group: it evaluates every file against a rule.
⚠⚠ **Stem alone is safe here** — unlike the Immich migration, the filesystem enforces uniqueness
within a folder. ⚠ Settings → Appearance; `enabled` defaults **true** (the `gpu_tail` trap).
⚠⚠ **Both folder-content predicates (`formats_present`, `folder_overrides_hidden`) read the SCAN, not
the grouped entries** — reading `entries` made the JPEG chip flicker and silently disabled the hidden-
format override in an all-paired folder. `grep -n 'stacks: same-stem' docs/JOURNAL.md`.

⚠⚠ **Run the git command above rather than trusting any count here** — this block was wrong about the
push state twice on 2026-09-24. ⚠ Still never push. ✅ CI on Linux is green; its one failure was a test
asserting an absolute path's existence — shape **(e)** in *Standing lessons*.

✅ **Immich slice 4 — Download / Remove downloads (2026-09-24, confirmed by use).** ⚠⚠ Its
load-bearing part is a **bounded download queue** (cap **4**): `request` spawns a thread per job, so
a bulk album would have spawned **872**. ⚠ Start the next job **after** `in_flight.remove`.
⚠⚠ **Removal deletes only the image** — sidecar, catalog row and thumbnail survive, and
`cache_path`'s byte-identical re-download reattaches them. `grep -n 'Immich slice 4' docs/JOURNAL.md`.
✅ **Landed 2026-09-23/24, all confirmed by use:** per-channel RGB tone curves (⚠⚠ `apply` was already
`map_samples`, so the goldens stayed byte-identical); the **Color section** behind `ADVANCED COLOR`
(⚠⚠ not three real sections — that would split the `Color` edit group *and* the Sync dialog); the
**histogram's** sqrt scale with bins 0 and 255 out of it (⚠⚠ the test says **not** to fix its
2.2%-at-2000:1 limit by lowering the threshold); and the **Now row counting what DIFFERS** via
`Edits::changed_parameters`, ⚠⚠ derived from **serialisation, never a field list**.
`grep -n 'per-channel RGB\|made legible\|histogram.s scale' docs/JOURNAL.md`.

⚠⚠ **The Immich invariants that are easy to break again:** the catalog keys a row on the **path's** own
file name, never `CatalogEntry::file_name`; `row_is_browsable` hides an absent **Immich** row from
catalog-wide views but **never** an absent local one; `present_names` is one `read_dir` filtered through
`catalog::is_supported`. ⚠ Out of scope by decision: eviction of cached originals, the OS keychain (keys
are **plaintext**), reading Immich's own `rating`/`isFavorite`. ⚠ The **rail's** album and bucket lists
are still refetched every launch; only *asset* lists are cached. ⚠ **Auto-advance is a Settings toggle**
already `true` in the user's file, so changing the Rust default would not change his behaviour.

✅ **Merged and confirmed by use, specs under `docs/superpowers/specs/`:** the Export dialog, the
dialogs in one language, the snapshot timeline, the top bar and Library, and the five panel reworks
plus the mask off switch (2026-09-18 → 22).

⚠⚠ **This block is the LIVE STATE, not a changelog — overwrite it, never append.** It reached a full
screen of per-milestone tables twice, which is the 1.2 MB failure in miniature.

**Landed 2026-09-18, all confirmed by use:** M8.13, M8.14, the `batch_export.rs` split, the rendering
seam reaching export and thumbnails, `ProfileRendering` as a required argument, and the GPU tail on by
default. `grep -n 'M8.13\|M8.14' docs/JOURNAL.md`.

### ▶ Next action — the plan has a queue now; work it in order

⚠⚠ **RESTRUCTURED 2026-09-25 in `03-plan-of-action.md`** (Obsidian, `private/opensource_lightroom/`).
Every planned phase closed except **M7.3b** (since done, 2026-09-25), so the roadmap was re-cut:
- **Phase 9 is NEW — the ten capability gaps**, as a one-at-a-time checklist ordered cheapest-useful
  first: **M5.1c** parametric curve · **M6.4c** rendering intents/gamut · **M9.6** defringe · **M9.7**
  persistent history · **M9.8** keyword re-parenting · **M9.9** folder tree + move tracking · **M9.10**
  DNG · **M9.11** red-eye · **M9.12** import/ingest · **M9.13** AI subject/sky masks.
  ⚠⚠ **M5.1c and M6.4c keep their OWN identifiers** — they are sub-slices of closed phases, and minting
  `M9.x` names for them would put one piece of work under two names. ⚠ Phase 9's new items start at
  **M9.6** because M9.1–M9.5 are retired, not free.
- **The old Phase 9 is Phase 10** (integration, cross-platform, release). ⚠ **M9.1 (Immich) KEEPS ITS
  NAME** inside it — it shipped, and ten references point at that string. M9.2–M9.5 → **M10.2–M10.5**.
- ⛔ **M10.1 was WITHDRAWN the same day it was written — it was never real.** `LICENSE` (the full
  674-line GPL-3.0 text) has been committed since `e71330d`, and `[workspace.package]` has carried
  `license = "GPL-3.0-or-later"` all along. ⚠⚠ **Do not re-open it from a list**; it exists in the plan
  only as the record of the broken-`ls` check in *Standing lessons* below.

**▶ The order the user set: M7.3b ✅, the drag-frame fixes ✅ and GPU geometry ✅ (all merged and
confirmed by use 2026-09-26); Phase 9's checklist from the top — M5.1c (parametric curve) ✅ merged and confirmed by use
2026-09-26; M6.4c (rendering intents) is next.** A Straighten drag on
DSCF6263: **~97 → ~41 ms per frame in the app** (85.0 → 28.8 ms headless). ⚠ The remaining app-vs-probe
gap (~10–12 ms) is GPU submit+wait, and it is **NOT** the window's per-frame upload: freezing the window
changed nothing, so the shared-device idea is ⛔ declined on that measurement (`docs/BACKLOG.md`). The
real cause is unidentified. ⚠ **Proposed and not yet scheduled:** masks' *scene* sliders (WB, exposure,
contrast, tone regions) run after the global tone curve today, where Lightroom applies them as
offsets at the global slider's position. It was raised in M7.3b's brainstorm and split off as its
own milestone. It is the author's call.

⚠⚠ **The GPU port is finished** — done, declined with numbers, or blocked on a considered trade. Do
not go looking for the next port; there isn't one.

✅ **The Immich integration is COMPLETE (M9.1, 2026-09-22 → 25), and its detail has moved out of this
section** — it was 34 lines of finished work inside a *next action* list. The invariants worth keeping:
⚠⚠ **`asset.upload` is a grant SEPARATE from `asset.update`** (168-value enum), which is the whole
basis of upload-without-write-back, so upload rides its own **`immich_upload_key`**; ⚠⚠ **never delete
a staged file on an upload *attempt*** — an SSL/EOF means the *response* was lost, measured 4-in-2,373
with none actually failing to apply; ⚠⚠ **there is no mtime fallback for the upload date** (it filed an
unreadable date under the DOWNLOAD time), and `Edited` is read at **plan** time because recording an
upload bumps the sidecar's mtime; ⚠ `Settings.immich_enabled` is read **only** through
**`immich_source::connection`**, one choke point, defaulting **true** (the `gpu_tail` trap); ⚠ server
stacks have **no validator**, so freshness rests on **Reindex** alone. ⛔ **Two-way sync is DECLINED —
the user's call, not a deferral. Do not re-propose it from a list**; the table is in `docs/BACKLOG.md`.
`grep -n 'export delivery\|completely optional\|server-side stacks' docs/JOURNAL.md`.

**The small leftovers, in the order the user has shown interest:**

1. **The u8 coverage wire** — priced: **0.56–0.70 ms against an actual 3.55–6.35** at rung 3200,
   **3.4 against 15.3** at export. ⚠⚠ Blocked on a shader restructure — the shader writes one `u32`
   per sample because packing four into a word would race. **A considered trade, not an oversight.**
2. **Rename `ProfileRendering` → `TailRendering`** — `look` is not a camera-profile field. Now a
   ~120-site diff. ⚠ Its own change, never folded into another.
3. File sizes — ⚠ **in *Open items* only, measured by code.** A list kept in two places is how this
   file reached 1.2 MB.
4. Move `PrefixCache`'s tests into `prefix_cache.rs`.

⛔ **Declined with numbers, and the numbers are in `docs/BACKLOG.md` — do not re-propose from the
list alone:** the prefix seams' *download* half in general (11–13%, needs `PrefixCache` to hold a
`GpuImage`; ⚠ built for the LAST step only on `gpu-geometry`, the author's call),
spot removal on the GPU (0.05–0.4 ms against a ~1.6 ms floor), `MIN_GPU_BOX_PX` (user's call, on
use), the GPU-side coverage cache (re-priced downward).

⚠ **`MAX_TILE_DABS`** — swept; the cliff is at **2560, not 3584**. Blocked on a **portability**
question (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) that no M2 run
settles.

### The GPU port — closed (reopened once, for geometry, 2026-09-26)

⚠⚠ **19 of the 20 stages that `impl PipelineStage`** have GPU twins; only `spot_removal` is a
deliberate omission (measured and closed unbuilt). ⚠⚠ **Geometry was declined too, and that decline
was WRONG for one reason worth remembering: it priced the COMPUTE and missed the TRANSFERS.** On a
fitted rung of 6.8 Mpx a Straighten frame was 63 ms, and the twin's value was that it let the pixels
stay on the GPU: a resident input and a no-download handoff to the tail. ⚠ **Do not re-propose from a
list alone**; the order with each row's cost is in `docs/JOURNAL.md` (`grep -n "agreed order"`) and
the declines with their numbers in `docs/BACKLOG.md`. ⚠ Count by `impl PipelineStage for` and
`impl (crate::)?GpuStage for` — exposure writes the `crate::` form, and a grep without it undercounts
by one (it did, 2026-09-26).

### Open items

⚠⚠ **Live items only.** A finished item is deleted from here, not struck through — its record is in
`docs/JOURNAL.md` and its numbers in `docs/BACKLOG.md`. Four ✅ entries had stacked up here by
2026-09-18, which is the 1.2 MB failure starting again.

- **A GPU-side mask coverage cache** — ✅ measured 2026-09-16; the ceiling missed 4–8×. ⚠⚠ The cost
  is **not transfer**: a serial u8→u32 repack and a 4×-inflated download wire. **Fix those two
  first** — mechanical, no design question. Re-price after.
- ⚠ **The u8 coverage wire**, the **`ProfileRendering` → `TailRendering`** rename, **`MAX_TILE_DABS`** and
  `PrefixCache`'s test move sit under **▶ Next action** above with their numbers and blockers; the Immich
  leftovers sit in **State**. Nothing is repeated here — a list in two places is how this file hit 1.2 MB.
- The prefix-cache eviction fix's **picture** half.
- Clarity's **coalescing** on the horizontal running-sum pass; **M8.1**'s leftovers.
- ⚠⚠ **MEASURE A FILE BY ITS CODE, NOT ITS TOTAL** — this list was wrong in *both* directions until
  2026-09-24, because every figure was a total and this repo's files carry enormous test tails. Measure
  with `awk '/#\[cfg\(test\)\]/{exit} {n++}'` (code **plus** doc comments, which are heavy here).
  **The genuinely large ones:** `edit-model/src/lib.rs` **1,883**, `pipeline/src/lib.rs` **1,810**,
  `catalog/src/db.rs` **1,730**, `app/src/settings.rs` **1,046**. ⚠ `adjustments.rs` is **610**, barely
  over. ⚠⚠ **Re-measure, never requote** — this entry's own precedent figures went stale within a day,
  so a measurement written down decays exactly like the totals it was created to correct.
- The **7 temp-dir test-isolation sites** — ⚠⚠ **never run two gates concurrently.** ⚠ `immich_source`
  adds **two more** that key only on the process id, with no counter; `immich_list_cache`'s and
  `thumb_store`'s use a counter as well and are the pattern to copy.

### The open question M8.11 did not settle

M8.3j/k/l's inner-chain machinery is reachable **only on the decline path** (a gating brush, cold cache).
Kept and pinned; whether it still earns its keep is worth asking when something touches it.
`grep -n 'accelerate_inner' docs/JOURNAL.md`.

### Standing lessons (the expensive ones — full stories in `docs/JOURNAL.md`)

- **⚠⚠ A guard that enumerates a list stops guarding the moment the list grows.** The
  double-application test named three of five fields and was therefore blind to a whole milestone —
  the real sabotage left it at `1 passed` while six goldens failed. Read this before adding anything
  to `adjustment_stages`.
- **⚠⚠ A TEST THAT CANNOT FAIL IS NOT A TEST, AND FOUR SHAPES OF THAT HAVE COST THIS PROJECT REAL BUGS.**
  **(a)** A fixture that makes two different things **equal** cannot see a bug about telling them apart —
  `asset("one")` with `original_file_name: "one.jpg"` made a UUID path and a display name coincide, and
  four tests passed against a catalog that could never find its own rows. **(b)** A query that finds
  **nothing** passes silently, so assert the sample first — `labels.len() > 12` caught three separate ways
  the Color guard opened nothing, and a substring filter matching no tests exits 0. ⚠⚠ **It applies to shell
  checks, and it cost an invented milestone (M10.1) on 2026-09-25:** `ls | grep -i license` returned
  nothing, so I reported no `LICENSE` — it had been committed since `e71330d`. **`ls` here is aliased to
  `eza --long`, which prints NOTHING to a pipe unless given a path argument**, exiting 0 with empty
  stderr. Use `/bin/ls` or `git ls-files` for anything you will act on, and **prove a negative with a
  positive control**: the same command must find something known to be there. **(c)** Comparing two
  things that are **the same** proves nothing, so prove they differ first: the curve-order test asserts the
  two orders differ by >0.05 *before* asserting which happened; the GPU whole-chain test asserts the render
  differs from `neutral` before comparing backends. **(d)** When a threshold you invented fails, **find the
  real number** rather than lowering the bar — the histogram's sqrt reaches 2.2% where I asserted 5%, and
  the test was rewritten around the measurement with a note not to weaken it. **(e)** An assertion about
  an **absolute path** tests the host, not the code: `!dir.join("../../../etc/passwd").exists()` is a
  *lexical* join, so it probed `/var/folders/mt/etc/passwd` on macOS (absent → green for three days) and
  **`/etc/passwd` on the Linux runner** (present on every Linux box → red). It never read anything the
  module wrote. ⚠ **Assert on the path the code DERIVES and on the directory the test itself created** —
  `immich_source`'s `a_hostile_original_name_cannot_escape_the_cache_directory` is the shape to copy. This
  was the **first CI result after the 2026-09-21 workflow fix**, and the only failure in 985.
- **⚠⚠ A CLAIM YOU DID NOT RUN IS NOT A FINDING — AND A COMMENT ASSERTING ONE IS WORSE, BECAUSE IT
  OUTLIVES YOU.** Four times now. *(a)* I reported *"the snapshot branch never had this bug"* from one
  line of a two-branch function; the user found it that day. *(b)* A test's doc advertised catching a
  sabotage `AlbumSummary`'s fields make impossible. *(c)* ⚠⚠ A comment read *"the ONE place stacking
  happens"* — there were **two**, so stacking did nothing on folder-open. *(d)* ⚠⚠ A comment argued for
  reading the **grouped** list "so the two answers cannot differ" — the grouped list had *lost* the
  count, so "Expand all" silently set an empty set. ⚠ **Run the other branch, run the sabotage, and
  `grep` the claim before writing it down.** A comment asserting a single call site is not a single call
  site; a function both callers must go through is.
- **⚠⚠ AN EDIT POSITIONED BY WALKING OVER COMMENT LINES WILL EVENTUALLY EAT ONE, AND NOTHING IN THE
  GATE CAN SEE IT.** On 2026-09-23 a scripted extraction in `copies.rs` inserted a helper *below* a
  17-line doc block instead of above it — the doc ended up on the wrong function, `reindex` was left
  undocumented at a **three-space indent**, and every test passed. The repair then left the block
  **duplicated**, and tests passed again; only clippy's `doc_lazy_continuation` caught it. ⚠ **After
  any scripted move, diff against the pre-move commit and require the removed `///` lines to be
  empty** — `git diff <commit> -- <file> | grep '^-' | grep '///'`. Cheap enough to be routine.
- **⚠⚠ A GUARD SHARED BY TWO CALLERS WITH DIFFERENT RETRY BEHAVIOUR IS A BUG WAITING FOR THE ONE THAT
  DOES NOT RETRY.** `ImmichWorker`'s one `pending_list` flag covered two list kinds; the rail hid it by
  re-requesting every frame, so it looked correct for weeks. The first one-shot caller would have lost
  its request silently. ⚠ Second time in four days for this exact shape — see the album-click guard.
- **⚠⚠ A gate run before the last commit describes a tree nobody shipped.** M8.9's tip had to be
  re-gated before merging, which caught an unformatted commit checked only with `cargo build`.
  **Re-gate the tip, and read each exit code from its own command.**
- **⚠⚠ Measure the drag through `RUSTROOM_DEBUG_RENDER`, never a probe row.** M8.6's 12.5–13.3× was
  retracted: `probe_resamplers` priced a *stage* against a resume the real chain never took, so it
  shipped and did nothing.
- **⚠⚠ A CLAIM RESTATED OFTEN ENOUGH STOPS BEING CHECKED — and it happens to numbers, not just
  blockers.** "NR and dehaze run before geometry" propagated through four documents and was simply
  false. So did the demosaic tier's *"~8 s a photo"*: that was the `Best`→`Fast` gap, quoted for
  four milestones in support of `Best`→`Better`, which measures at **~3.7 s**. ⚠ **Before requoting
  a figure, check which two configurations it compares** — and check any *"blocked on a design
  question"* note in `docs/BACKLOG.md` against the code before trusting it.
- **⚠⚠ Almost every GUI-wiring defect here was found by USE, not by a test** — a milestone is closed when
  it has been *seen*, not when it is green. ⚠⚠ **But that is a fact about what happened, never a reason a
  thing is unreachable by a test: before saying "only use can verify", look for the harness that already
  almost covers it.** I twice called RGB-curve preview/export parity unreachable;
  `a_colorimetric_frame_renders_the_same_picture_on_the_gpu` existed and merely ran on `Edits::default()`,
  so it missed a `GPU_STAGES` site passing only the composite curve — **which compiles perfectly**. Three
  channel curves in its fixture made it a one-second check, sabotaged at **209 levels / 8,248 samples**.
- **⚠⚠ ASK THE ARTEFACT, NOT THE DESCRIPTION OF IT — twice over, and both cost real bugs.**
  **(a) An OpenAPI schema describes what a server *may* send, not what it *does*.** Three Immich defects,
  each invisible to a green suite and obvious in a minute of use: `GET /albums/{id}` returns **no `assets`
  field**; a real server pages by **`nextPage`** while leaving `nextCursor` null; and `required: true` does
  **not** mean non-null — `nullable` sits beside it, and one `null` `thumbhash` failed a whole month.
  ⚠ When one field turns out nullable, **audit them all** — two more were waiting. **Capture a real reply
  before writing a DTO.**
  **(b) A font's advertised character set is not its cmap.** The Immich rail shipped `↓` and `✕` as grey
  squares: `↓` is in **Hack only**, which is the *Monospace* family, and `✕` is in **no bundled font at
  all**. ⚠ `icons.rs` already carried this warning and recorded four features paying for it; that was the
  fifth, so the warning now also sits on **`RowAction::glyph`** — where the mistake is actually made.
  **Parse the cmap.**
- **⚠⚠ A FIXTURE YOU WROTE IS NOT EVIDENCE ABOUT SOMETHING YOU DO NOT CONTROL.** M9.1's
  `album.json` was hand-written and described *in its own plan* as "trimmed from `GET
  /albums/{id}`". It asserted an `assets` array that endpoint has never returned. The test passed
  because it checked an assumption against itself — **2,678 green tests, and the first real album
  open failed immediately.** The captured reply then exposed a second, worse bug the schema nowhere
  implies. **Every fixture must say whether it was captured or constructed**, and a constructed one
  proves only self-consistency. One real response is worth more than any number of invented ones.

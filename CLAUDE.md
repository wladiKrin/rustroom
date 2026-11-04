# CLAUDE.md — Rustroom

Project-specific guidance for Claude Code working in this repository.

**Rustroom** — an open-source alternative to Adobe Lightroom. (Binaries: `Rustroom`, the app; `rustroom-cli`, the CLI. Repo folder: `~/Projects/rustroom`. The Obsidian planning notes live in the vault folder `rustroom` — renamed from `opensource_lightroom` on 2026-09-29.)

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
`~/Documents/obsidian/private/rustroom/`.

⚠ **Reaching it needs TWO permissions, and they are separate — 2026-08-26.**
1. **The harness's directory scoping** — `mcp__ccd_directory__request_directory` with the vault path grants it; the user approves in-session.
2. **macOS TCC** — Full Disk Access for the app Claude Code launches from, System Settings → Privacy & Security, then a full quit-and-reopen. **Granted on this machine as of 2026-08-26.**

⚠ The one-call check, no permission prompt, that tells you which (if either) is missing:
```
/bin/ls ~/Documents 2>&1 | head -3
```
`Operation not permitted` means TCC; a harness refusal names the path instead. ⚠ Do not conclude from a successful folder grant that the file is readable — the grant reports success while TCC still denies it, which cost a wrong diagnosis here.

- `01-lightroom-capabilities.md` — target feature set + prioritized P0–P4 scope table. **Annotated 2026-08-26, re-swept 2026-09-29** with ✅/🟡/⬜/⛔ per capability — every ⬜ names its Phase 10 milestone — plus a **"deliberate divergences from Lightroom"** section (copy numbering from 0, radial mask affects the inside, no Smart Previews, post-crop vignette by stage order, panel grouped by edit group).
- `02-architecture-and-tech.md` — stack rationale, pipeline design, crate layout, best practices. **Corrected 2026-08-26** — ⚠ four things it got wrong are struck through in place: the **pipeline order** (geometry runs *first*), the **working space** (linear ProPhoto), the **stage trait** (consuming, params baked in at construction), and the **crate layout** (12, not 8). Its §8 lists what the note did not anticipate at all.
- `03-plan-of-action.md` — phased, step-by-step build order with "Done when" criteria. **The canonical list of open work:** Phase 10 (every open item, M10.6–M10.18) and Phase 11 (release, M11.1–M11.5).

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
rustroom/
├── Cargo.toml                # [workspace]
├── CLAUDE.md                 # this file
├── crates/
│   ├── core-image/           # ✅ ImageBuf (f32 RGB) + Capture (EXIF facts, both decoders) + map_samples/map_pixels/map_pixels_xy/to_rgb8/to_rgba8 (rayon-parallel) /crop/downscale_to + Thumbnail
│   ├── raw-decode/           # ✅ libraw(rsraw); DecodeOptions{linear_scene,display,preview(half_size),demosaic}; Demosaic tiers -> user_qual; extract_thumbnail + extract_preview (the largest embedded JPEG at its own size, M9.14)
│   ├── image-decode/         # ✅ JPEG/PNG/TIFF → linear ImageBuf (image + srgb_to_linear) + thumbnail; EXIF orientation applied in both
│   ├── color/                # ✅ sRGB transfer fns + Oklab (lcms2/ICC still deferred)
│   ├── edit-model/           # ✅ serde Edits op-list (orientation/WB/exposure/contrast/tone/curve/saturation/hsl/local_contrast/dehaze/NR/sharpening/vignette/geometry) + EditGroups partition + merge_from
│   ├── pipeline/             # ✅ PipelineStage + 14 stages (incl. geometry, noise_reduction, dehaze, saturation, hsl, local_contrast, vignette, sharpening) + spatial::{box_blur, gaussian_blur, wide_blur, min_filter, guided_filter} + build_pipeline / build_pipeline_split(_in_region) / build_prefix_steps + PrefixStep/PrefixStepKey/PrefixKey + Region + tail_kernel_radius + golden tests
│   ├── export/               # ✅ to_rgb8 + save (JPEG/PNG/TIFF) + metadata::{ExportMetadata, FileMetadata, MetadataScope} — the full XMP packet (M6.2) + watermark + output_sharpen
│   ├── xmp/                  # ✅ stem.xmp read / merge-write: rating, label, keywords, title/caption/rights/creator (M6.3 + M6.2); zero GUI, zero catalog
│   ├── immich/               # ✅ Immich client (zero GUI, `ureq`): search/buckets/album/stacks/**upload** (+ `multipart.rs`, hand-rolled — ureq has none), `Asset`/`AlbumSummary`/`Bucket`/`Upload`. ⚠ DTOs deliberately narrow; nullable-vs-required audited against a *captured* reply
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom-cli` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs holds only the module list, the shared constants and `fn main` — the state is defined in `app`/`session` and built in `bootstrap` (M10.7d; the "78 lines of code" once quoted here was the first-`#[cfg(test)]` blind spot: it held 555); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. ⚠⚠ **THE MODULE LIST THAT USED TO SIT HERE IS GONE ON PURPOSE.** It enumerated ~50 of what are now **266 `.rs` files** under `crates/app/src` (2026-10-12) and had silently gone half-stale — it was missing `multi/{compare,survey,pane}`, `menu`, `menu_bar`, `native_menu`, `commands`, `tone_curve_tool`, `brush_tool`, `mask_tool`, `looks`, `marks` and ~40 more, which is worse than no list because this one is the *placement guide*. **`ls crates/app/src/` is the inventory; what belongs here is the RULE:** `lib.rs` declares **108** modules and **not one of them uses egui** (2026-10-12: every `egui::` in them is in a comment) while `main.rs` declares **53** bin modules (2026-10-12) and egui lives only there. ⚠ Re-measure, never requote — it said 73 and 33 for weeks: count with `grep -c '^pub mod [a-z_0-9]*;' crates/app/src/lib.rs` (BSD `grep -E` has no `\s`, and zsh does not word-split a `$files` list — pipe it through `xargs`). Logic goes in the lib, widgets in the bin.
│   └── catalog/              # ✅ scan_folder → RAW+JPEG/PNG/TIFF entries (+ mtime); SortOrder/sort (name/modified/edited, case-insensitive, name tie-break); is_raw/is_supported (in-memory; SQLite DAM later) + Description per file (v7, M6.2)
│   ⚠⚠ THE TREE ABOVE LISTS 12 OF 19 CRATES — it omits `gpu-pipeline`, `lens-profile`, `camera-profile`, `ai-mask`, `test-dir` (dev-only: every test's temp directory, M10.7c), `lr-preset` (Lightroom presets read into edits, no GUI, M10.9c) and `photo-merge` (Photo Merge: HDR — frames, align, deghost, merge, M10.17a — and Panorama — features, rig, projection, seam, blend, fill, M10.17b; zero GUI; its `testing` feature exposes `synth_pano` to the app's tests). **`ls crates/` is the answer; this tree is a sketch.**
├── docs/
└── tests/                    # integration + golden reference images
```

Key rule: **`pipeline`, `color`, `export` have zero GUI dependency** and are driven headless by `cli` and tests.

## Conventions & engineering discipline

- **CI on Linux from day one:** `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check` must stay green.
- **Golden-image regression tests** (established at plan milestone M1.2): render a fixed RAW with a fixed op-list, assert output matches a checked-in reference within tolerance. Never abandon these — they make the GPU port safe.
- **Keep crates small and single-purpose.** A file growing large signals it's doing too much.
- **⚠ No two workspace binaries whose names differ only by case.** macOS's disk ignores case, so `target/`
  holds ONE of them and the last built wins — `rustroom render` once opened the app (2026-10-20). The CLI is
  therefore `rustroom-cli` (its package is still `rustroom`: `cargo run -p rustroom -- render …`);
  `test-dir`'s `no_two_binaries_share_a_name_but_for_case` guards it.
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
  ⚠ The slowness was never test execution or fingerprinting but **cold filesystem work** in an 85 GB
  `target/`; `[profile.dev] debug = "line-tables-only"` fixed it (cold **437 s → 11 s**;
  `grep -n '_dyld_start' docs/JOURNAL.md`). ⚠ A `SessionStart` hook (`scripts/target-budget.sh`, in the
  untracked `.claude/settings.local.json`) warns past **250,000 files** in `target/` (healthy ~37k) —
  **Cargo never garbage-collects**; it counts files with `find | head`, never `du`.
  ⚠⚠ **THE WHOLE GATE IS ~79 SECONDS** (fmt instant, clippy 62 s cold, tests 16 s). Any document
  saying it is 15–30 minutes predates 2026-09-23 and is wrong; so is the 2026-09-19 journal note
  calling the stall "not fixable from here".
- **Color correctness is first-class** — validate against references early.
- **⚠⚠ ANYTHING THE USER CAN SEE FOLLOWS `docs/DESIGN.md`** — the design language chosen on 2026-09-29
  (direction *B · Layered*): four text sizes and nothing below 11 pt, one accent for selection and set
  state, one form per control, a top row plus a status row each with a priority plan, hover text
  instead of on-screen hints — one sentence, no full stop, swept by `every_hover_is_one_short_sentence`. Its §12 is the checklist for a new element; a literal font size or
  `Color32` outside `theme.rs` is a defect. Mockups: <https://claude.ai/artifact/JK6A3tSpaXPo427cHZTZ7h>.
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
> **Run the GUI:** on **Linux** (Arch, since 2026-10-27) plain `cargo run --release -p app`; on **macOS** first `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`, then the same — always `--release`, in the background, and **launch it after every big update without being asked**, saying what to look at. Skip it for small tweaks in a tuning loop.
>
> **⚠ Workflow:** every feature goes brainstorm → spec → plan → executing-plans, **inline** (the user has chosen inline every single time; offer the choice but expect that answer). Specs land in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`.
>
> **⚠ The history is in `docs/JOURNAL.md`** — 212 sections, newest first, every milestone's measurements, retractions and uncaught sabotages. **Grep it, never read it whole:** `grep -n '^### ' docs/JOURNAL.md` for the index, `grep -n 'M8\.11' docs/JOURNAL.md` for one milestone.

### State (2026-11-04)

**4611 tests on Linux** (macOS last counted 4274 before M10.17b; 31 ignored), **94 golden tests** (81 reference images — `cargo test --workspace --test golden`),
gate green on **`main`** (2026-11-04, M10.18e merged). ⚠ On this Linux machine `gpu_pipeline`'s suite can stall
inside a full run (alone it passes in 4 s) and `sources::reindex_tests` can fail to exit — both pre-existing
(`grep -n 'Pre-existing, not chased' docs/JOURNAL.md`); run the remaining test binaries directly when it happens. ⚠⚠ **Run the git command above rather than
trusting any count here** — this block has been wrong about the push state before. ⚠ Still never push.
⚠ CI on Linux was **almost certainly red from 2026-09-30 to 2026-10-27** — the `Rustroom` binary did not
compile off macOS (`grep -n 'fix-linux-build' docs/JOURNAL.md`); unverified, `gh` is not installed here. Check the next run.

⚠⚠ **This block is the LIVE STATE, not a changelog — overwrite it, never append.** Finished work is
one line with a `grep` pointer; its story is in `docs/JOURNAL.md`. (Compacted 2026-09-27 from a block
that had grown to ~170 lines by appending.)

**Phase 9 — complete, all merged and confirmed by use** (`grep -n '<id>' docs/JOURNAL.md` for each):

| Id | What | The thing to remember |
|---|---|---|
| M5.1c | Parametric curve (Lightroom's model, own representation) | folded into `ToneCurveStage`'s composite table; on-curve drag has a Settings binding |
| M6.4c | Rendering intents: Clip (default), Relative, Perceptual | ⚠⚠ Clip is the default by the author's choice from crops; hue-preserving intents use an EXACT per-pixel boundary (a table cost 49 levels at the gamut corners) |
| M9.6 | Defringe, global (Lens section, eyedropper) + per mask (−100 protects) | ONE prefix step after geometry, ~62 ms at fit; edge band measured (0.02 greyed foliage) |
| M9.7 | Persistent history (Ctrl+Z survives restart) | ⚠⚠ in the CATALOG (`photo_history`), saved in `persist_current_session` — `on_exit` NEVER runs (close ends in `process::exit`) |
| — | Double-click resets a slider; a click on the rail is an undo step and takes no focus | `Edited::slider` takes a REQUIRED default; `every_slider_resets_to_its_fields_default` sweeps by role |
| M9.8 | Keywords: rename in place, drag to move, merge on collision | every change and undo rewrites XMP; ⚠ deleting a keyword now leaves the XMP too; new parent = the keyword last clicked |
| M9.9 | Folder tree + move tracking | `scan_and_relink` shared by open and Reindex; strict name+size+mtime match, a surviving original is a copy |
| M9.10 | DNG export: the finished picture as a linear DNG | `export::lj92` + `export::dng`; darktable workflow "none" renders it intact (0.00 mean), its default adds +0.66 EV; ⚠ `ColorMatrix1` at D65 (libraw ignores the illuminant tag) |
| M9.11 | Red eye + pet eye, one click each | detected ONCE at the click, stored as a circle; `RedEyeStage` after spots corrects by `pipeline::redness` = `(R−max(G,B))/R` plus a fringe pass within 0.3 r of the red core (the reported halo); real cores measure 0.44–0.57; Retouch modes Heal·Clone·Red eye·Pet eye. ⚠ Also fixed: a placed/moved spot now pushes ONE undo step |
| M9.12 | Import from a card: File → Import… ⇧⌘I | `ingest::{source,plan,copy,options,worker,entry}` (no egui) + `import_flow`/`modals::import`; cards = volumes with `DCIM`; already-imported = catalog size + capture time; layout `YYYY/YYYY-MM-DD` (default) · `YYYY/MM/DD` · `YYYY-MM-DD` · one folder; metadata merged only when non-empty; ⚠ the survey reads only the EXIF block (`image_decode::quick_look`, 0.5 ms / ≤70 KB vs 113 ms / 31 MB a RAF) |
| M9.14 | Previews: instant 1:1 in Develop, Compare and Survey | the camera's embedded JPEG (4416 px on an X-T4) is the free 1:1 of an untouched photo; rendered previews (an export job into `previews/`) only for edited photos, on leaving and on demand; a **ghost session** from the 512-px thumbnail at the click, the half and full decodes adopted underneath (`adopt_decoded`); ⚠⚠ untouched = `initial_edits(None)` OR `Edits::default()`, never `is_neutral`; ⚠⚠ the stand-in rule reads `view_region::needed_long`, never the rung; ⚠ no 512-px picture while a preview may be on its way (`preview_misses`); neighbours fetched ahead; a single Better decode of an X-T4 is **16 s** (the 7.4 s elsewhere is three in parallel) |
| M9.13 | AI masks: Subject + Select by click | crate `ai-mask` on **`tract`** (measured: 2.7–2.8× of ort; CoreML cannot compile BiRefNet); IS-Net default / BiRefNet_lite in Settings, MobileSAM for clicks; maps in **decode space**, cached on disk, fed through the brush side channel; ⚠ SAM decoder: use `low_res_masks` (the full-size output's crop is frozen landscape); ⚠ the lens warp is done ONCE per map (`post_lens`) — per pixel per frame cost 117 ms a rotation frame |

**Earlier, still worth knowing** (all merged, all confirmed by use):
- **The GPU stack** (2026-09-26): timestamps (`RUSTROOM_DEBUG_GPU_TIME=1`; ⚠ an encoder timestamp
  outside a pass deadlocks wgpu 29's Metal backend), per-pixel fusion (`RUSTROOM_NO_FUSION=1`; ⚠ the
  walk calls a stage once per ROW), and the resident final checkpoint (⚠ the drag frame ~3 ms slower,
  the fix priced in the journal). `grep -n 'GPU timestamps\|per-pixel fusion\|resident final' docs/JOURNAL.md`.
- **M7.3b** — perspective fused into `GeometryStage`; ⚠⚠ its drag speedup applies only with NR or
  dehaze on (2 and 1 of 49 photos). **Stacks** — ⚠⚠ grouping happens in
  `library_state::browser_entries`, which BOTH entry-building paths must go through; both folder-content
  predicates read the SCAN, not the grouped entries.
- ⚠ `Edits::version` is **never written** (always 0); do not trust any note giving it a value.
- ⚠⚠ **Immich invariants** (M9.1, complete): the catalog keys a row on the **path's** own file name,
  never `CatalogEntry::file_name`; `row_is_browsable` hides an absent **Immich** row from catalog-wide
  views but never an absent local one; `asset.upload` is a grant separate from `asset.update` (upload
  rides `immich_upload_key`); never delete a staged file on an upload *attempt*; no mtime fallback for
  the upload date; `Settings.immich_enabled` is read only through `immich_source::connection`.
  ⚠⚠ **The two Immich keys live in the OS keychain** (M10.15a, `rustroom_app::secrets`): only
  `SettingsStore::user_default()` reaches it — `new(path)`, every test's, never does; keys in the file
  are always the newest; writes are on a background thread. On macOS `cargo run` signs `Rustroom` with
  the local "Rustroom Dev" identity (`scripts/macos-run.sh`); ⚠⚠ the first launch after a rebuild still
  asks for the login password once — with no Team ID the item's partition list keys on each build's
  cdhash (accepted by the author 2026-10-13; a Team ID would end it).
  ⛔ **Rustroom never evicts a download on its own** (M10.15b, the author's call): Settings → Immich's
  Clear… and the startup warning past the soft limit, both through `remove_downloaded`.
  ⚠ **Immich's stars and favourite fill a photo's marks ONCE, never over Rustroom's, never written back**
  (M10.15c): the rule lives in `Catalog::fill_from_server`'s SQL (`photos.filled_rating`/`filled_flag`).
  ⚠⚠ **Rustroom writes to Immich only from Send marks' confirmation, export's upload and a publish service's
  Publish** (M10.15d, M10.18d — an album created, assets added and taken out of it, ⛔ never an asset deleted): `rating` and `isFavorite`,
  never removing a mark there, on the **Write key** (the upload key, with `asset.update`); the prior values
  go to `immich_last_send` for **Undo last send**. Every decision is `rustroom_app::immich_send`'s.
  ⛔ Two-way sync was DECLINED 2026-09-24 with reasons; the author re-opened it as **M10.15d** on
  2026-09-29 — start from `docs/BACKLOG.md`'s reasons table. ⚠ The server is a **Tailscale Funnel**: a stopped
  Tailscale on the Mac is never the cause of an Immich error.

**Phase 10 so far — M10.6 to M10.15, M10.16a, b, c1–c3 and d, M10.17a–b, M10.18a–e and M10.19c–d, merged 2026-09-29 – 11-04, all confirmed by use** (`grep -n '<id>' docs/JOURNAL.md`; what stays open inside M10.12–14 is in the table below):

| Id | What | The thing to remember |
|---|---|---|
| M10.6–7 | Immich Timeline; a hover on every control; ICC date pinned, `TailRendering`, `test-dir`, big files split | the hover sweep and the 1,000-line ceiling (*Warnings*) |
| M10.8a–d | Keyword suggestions; movable panels + a vertical filmstrip; hand stacks (catalog v12); batch rename ⌘R | a rename keeps its XMP and every catalog table |
| M10.9a–c | Preset folders, import/export; user shortcuts (Settings → Keyboard); Lightroom presets (crate `lr-preset`) | what a Lightroom preset sets that Rustroom lacks is NAMED to the user, never dropped silently |
| M10.10a/b | Image watermark; export sizing in px/cm/in with a PPI, Limit file size | sizing applies to the CROPPED picture (it used to shrink the uncropped decode); the limit lowers quality to 40, then pixels |
| M10.11a | Film negative: pick the base, each channel balanced from measured densities | a prefix step after lens, its own group OFF by default; the measurement skips densities outside −0.05…3.0 and a 15 % border (2 % once cropped) — a scan's holder had made an HP5 frame black |
| M10.11b | Grain in Effects (the vignette's group; the field is still `vignette`) | `MAX_EV` 0.7 by the author's tuning; the noise sits at pixel corners in FRAME coordinates, so a ¼-scale preview reads exactly the export's pixel |
| M10.12a | A row's op by right-click; Duplicate / Duplicate and invert; **Invert mask** for a composite — nesting ⏸ parked | the item's example was already expressible since reordering (measured table in the backlog); an inverted mask waiting for an AI map covers NOTHING (`Composite::waits_for_a_map`) |
| M10.12b | A mask's WB, Exposure, Contrast and tone regions are **offsets** to the global sliders (Lightroom's model); Temp/Tint relative ±100 | `SceneOffsetsStage` heads the tail (*Warnings*); a range/gating mask with an offset selects once, on the globally edited photo — 31 ms vs 7–9 at fit on the GPU path, the reference on the CPU (M10.14h ⏸); `MIRED_PER_STEP` 1.0 untuned |
| M10.14b/d/e | The prefix-cache eviction's picture half (a test); the drag gap decomposed and the geometry gather tiled 8×8; `catalog_scale` at 100k photos — All Photos 204 → 3.8 s | de-duplicate through `rustroom_app::unique`; the rail reads through `RailData` (*Warnings*); a and c ⛔ withdrawn on measurements |
| M10.15a–d | Immich: the keys in the OS keychain; Clear… + a soft limit warned at startup; Immich's stars and favourite filled once; **Send marks** + **Undo last send** | the Immich-invariants lines above; a rebuild asks for the login password once (no Team ID); Immich shows stars only with its Ratings feature on |
| M10.19c | ⌘/⇧-click marks folders in the rail; right-click forgets them, one confirmation, one undo step | marks keyed by PATH (rows move on collapse); `folder_tree::subtrees` lists a nested mark once, or its second delete fails mid-batch |
| M10.16d | Lens Blur: a dense, level-adaptive gather of aperture discs (Circle · Blades · Ring · Cat-eye) in three layers, focus on the Subject map's person or a picked point; the section after Effects, Pick focus, Visualize depth; a GPU twin, live (30–32 ms a frame at 1600 px) | the prefix's LAST step, waiting (no step) until its maps and focus exist; keyed on both maps' content ids; ONE lookup (`lens_blur::inputs`) for the step and its twin; the focus and the planes memoised (*Warnings*) |
| M10.16a | Depth masks: Depth Anything V2 Small (Apache-2.0) → the Depth range mask, a line picker, Show depth map; AI components now cover in a mask's NR/Dehaze/Defringe | `tract` needs the export's symbols BOUND per size and its shape annotations ignored; recipe `depth:v2` (1036, colour snap r8, equalised, 1536) — bump `DEPTH_QUERY` on any change; *Warnings* below |
| M10.16c1 | Remove: Retouch's first mode — paint over a thing, **LaMa** (the author's pick) fills it in the background from the full decode (~6 s, cached 1.4 MB); Refresh, Detect objects (MobileSAM), Opacity, the eye and Hold out per removal | removals live in DECODE space, pasted after AI denoise and before lens and the negative (*Warnings*); a fill is the network's 512², encoded by its PEAK channel (luminance clipped a sky's blue); never build a lens corrector per frame (*Warnings*) |
| M10.16c2 | Remove dust: one press looks for sensor dust in the background (no network — a disc against its ring over five radii, the ring brighter all round) and heals each spot as a Heal patch, one undo step; "Looking for dust" over the quiet line, then what it found stays under the button | the search is pumped by Develop's body (`dust_flow`), never the panel, so it lands with Retouch closed; finds are in decode space, patches post-lens through `lens_memo`; a press while the photo loads waits for it; proven on constructed dust — none confirmed in the author's library |
| M10.16c3 | Find people: YOLOX-m (whole frame at 640 + 640² tiles at 1920) finds them, MobileSAM outlines each, the small ones pre-ticked (< 35 % of the frame's height); a light border along each outline, a click toggles, a drawing looks again closer (else takes what is under it); Remove N people, each a `Removal.person` filled by c1's job | the detector sees the fill's window UPRIGHT (`remove:v3`, *Warnings*); the search reads the decode, where removed people still stand (`people_tool::not_removed` / `fresh`); finders are handed in through `people_tool::Finders` |
| M10.16b | AI denoise: **FFDNet** (default; a 26 Mpx frame 18 s on the GPU) or **DRUNet** (Settings), a background job per photo at the noise map × 2, the view's tiles first, "Full denoise" under Amount, cached on disk | the network is in EVERY key that holds its pixels and the worker carries its own (*Warnings*); `ai_mask::pth` is a CLOSED pickle subset; the result is ONE decode's, its step the prefix's first; a long GPU job submits bands |
| M10.17a | HDR merge: Photo merge → HDR… ⇧⌘H (and with the last settings ⌥⇧⌘H, no dialog) — the preview (zoom and pan), Auto align, Auto settings, Deghost None–High with its overlay, Create stack; a background merge into `<first>-HDR.dng`, a floating-point raw of the same camera (crate `photo-merge`) | exposures MEASURED, never read from EXIF; Rustroom reads float DNGs itself (*Warnings*); a gesture over a dialog never reaches the Library behind it |
| M10.17b | Panorama merge: Photo merge → Panorama… ⇧⌘M (⌥⇧⌘M with the last settings) — the preview with Auto crop's rectangle bordered as the crop tool draws it, Spherical · Cylindrical · Perspective with Auto select, Auto crop, Fill edges (LaMa), Create stack; a background merge into `<first>-Pano.dng`, a floating-point raw of the camera, capped at 16,000 px. Merges take **downloaded Immich photos**, the result going to a folder asked for once (Settings → Immich → Merged photos) | the lens is corrected inside the one resample and the DNG names no lens (*Warnings*); a result never lands in the Immich cache (`hdr_merge::out_folder`); HDR and panorama share one queue (`merge_queue`); tuned on constructed rows only — seam constants and Auto select's angles untuned on real ones |
| M10.18a | Tethered capture: File → Tethered capture… ⇧⌘T, Shoot (F12), Stop — each shot downloaded into a dated session folder, catalogued with a preset, selected and opened in Develop (crate `tether`, libgphoto2) | ⚠ **libgphoto2 is a build dependency** (`pacman -S libgphoto2`, `brew install libgphoto2` + `PKG_CONFIG_PATH`, CI's `libgphoto2-dev`); one thread owns the camera; the card is only READ; on macOS a session holds `ptpcamerad` off (`tether::Hold`); the X-T4 needs USB TETHER SHOOTING AUTO and its own shutter does not fire during a session |
| M10.18b | GPS and a map: Library → **Map** — clusters over OpenStreetMap tiles (our own layer; Settings → Behaviour's tile server), a strip of a cluster's photos or of those with no location, a drop or the location field (coordinates, or Nominatim on Enter) places the selection, Remove location, right-click menus, the map fits a new source | ⚠⚠ locations live in the CATALOG only (`photo_location`, v15: `hand` > `file` > `server`), never a file or a sidecar; a startup pass and Reindex read files' GPS; the X-T4 has no GPS of its own (the phone app tags it); nothing per frame grows with the catalog — `visible_rev`, the throttled `refresh_pins`, a virtual strip |
| M10.18c | People: Library → **People** beside Grid and Map — "Who is this?" groups of unnamed faces (YuNet finds, SFace tells apart, in the background), Immich's people from the server (a click opens all their server photos), move a face (drag, Name as…, Not in this group), Filter → People / ⌘-click (everyone ticked must be in the photo), hide views in Settings | ⚠⚠ names live in Rustroom ONLY, never Immich or a file; faces in catalog v16–v17 (`faces`, `faces_read` in `PER_FILE_TABLES`); grouping only SUGGESTS, off the UI thread; ⚠⚠ every new `Library` takes `library_state::fresh_rev()` (all at 0 left crops blank); the Immich people DTOs are constructed, not captured |
| M10.18d | Publish services: the rail's section, a folder or an Immich album per service, + / − the selection, a band above the view's grid (Publish N, the counts), a dot New / a ring Modified / Removed dimmed; plus **Edit description of N photos…** | catalog v18 keyed on `photos.id`; the fingerprint is pinned (`publish_state`); Immich: an album created, assets added and taken out — ⛔ never an asset deleted; the album DTOs are constructed, not captured |
| M10.18e | Print: File → Print… ⌘P — a page preview, Single (Fit · Fill) or a contact sheet, paper, margin, PPI and print sharpening, saved as a PDF or sent to a CUPS printer (`lpstat` / `lp`) | a page is the EXPORT's render (`ExportWorker` into a staging folder, Fill via `FitMode::Cover`, never in the export dialog); the PDF is hand-written and embeds those JPEGs as is (`print::pdf`); ⚠ printing to a printer is UNSEEN — no printer, no CUPS here |
| M10.19d | Retouch as two tasks: **Paint** (one brush, Remove · Heal · Clone · Eye, for NEW retouches only) · **Find** (People · Dust, found and reviewed alike: On the photo **Pick · Add more**, Everyone, Remove N, Cancel); one **Retouches** list of every kind, a picked row selecting it alone with its settings under the list | the photo's gestures follow `RetouchState.task` — in Find only `find_overlay`, never a Paint overlay; the list is `retouch_list::rows`; a patch Find dust made carries `Spot.dust`; the dust view's one writer is `pump_dust`; a look landing after Cancel or Remove N is dropped (people and dust) |

### ▶ Next action — Phase 10, top down

✅ Phase 9; ✅ the design language and reordering masks (2026-09-29); ✅ **M10.6–M10.15, M10.16a, b, c1–c3, d, M10.17a–b, M10.18a–e, M10.19c–d** (table above); ⛔ M10.16c4 declined.

**▶ NOW: M10.13d camera-matching colour** — the first open item not parked, top down (then M10.19a HEIC, M10.19b Move to folder for Immich photos); ask the author which first. ✅ M10.18e print merged 2026-11-04, Save PDF confirmed by the author, a printer unseen (`grep -n 'M10.18e' docs/JOURNAL.md`). ✅ An upload's chosen date in the file merged 2026-11-03, confirmed by the author (`grep -n 'upload.s chosen date' docs/JOURNAL.md`).
✅ M10.18d publish services merged 2026-11-03 with the batch description dialog, confirmed by the author ("okay that works").
✅ M10.18a tethering merged 2026-10-29, confirmed by the author with his X-T4 ("works, merge and delete";
`grep -n 'M10.18a' docs/JOURNAL.md`). ✅ M10.18c people tagging merged 2026-10-29, confirmed by the author
("works, merge and delete"; `grep -n 'M10.18c' docs/JOURNAL.md`).
✅ M10.18b GPS and a map merged 2026-10-28, confirmed by the author ("works, merge and delete that branch";
`grep -n 'M10.18b' docs/JOURNAL.md`). ✅ M10.17b panorama merge merged 2026-10-28, confirmed by the author
("works, merge and delete the branch"; `grep -n 'M10.17b' docs/JOURNAL.md`). ✅ M10.17a HDR merge merged 2026-10-27,
confirmed by the author ("works, merge and delete the branch"; `grep -n 'M10.17a' docs/JOURNAL.md`). (M10.16 complete: ⛔ c4 Reflections declined on its spike 2026-10-22 — no shippable
network removes everyday reflections; `grep -n 'M10.16c4' docs/JOURNAL.md`). ✅ M10.19d the Retouch panel as Paint · Find
merged 2026-10-22, confirmed by the author ("works, merge and delete"; `grep -n 'M10.19d' docs/JOURNAL.md`). ✅ c3 Find people merged
2026-10-22 (`grep -n 'M10.16c3' docs/JOURNAL.md`). ✅ c2 Remove dust merged 2026-10-21. ✅ c1 the Remove brush merged 2026-10-19
(`grep -n 'M10.16c1' docs/JOURNAL.md`). ✅ M10.16c1b merged 2026-10-20: a removal being filled shades the
AREA it will replace — Detect objects' whole object — sent by the job before the network runs. Left open behind it, by the author's call: M10.12c ⏸, M10.13 (a ⏸,
d), M10.14 f–h ⏸.

⚠⚠ **THE PLAN (the author, 2026-09-29): Phase 10 is every open item, worked top to bottom; Phase 11 is
release engineering, LAST.** The canonical list — every sub-item with its date, size and reasoning — is
the vault's `03-plan-of-action.md`, *Phase 10*; `docs/BACKLOG.md` maps each of its entries to an id.
`grep -n 'The re-plan' docs/JOURNAL.md` for how the list was built. Within a milestone, oldest first;
⏸ = parked or blocked.

| Milestone | Sub-items |
|---|---|
| **M10.12** the mask model | c ⏸ anchor strokes and spots in sensor space (no forward lens map) |
| **M10.13** colour & camera profiles | a ⏸ an independent C2/C3 reference (parked by the author 2026-10-12 — RawTherapee never runs here; the options are in the backlog) · d camera-matching colour out of the box (P-22), **with b's `ProfileLookTable` and c's per-photo profile folded in** (re-scoped 2026-10-12: neither changes a photo until d brings such a profile; b's `CameraCalibration` ⛔ withdrawn — a DNG-file tag, never in a profile) |
| **M10.14** performance & verification | f ⏸ incremental splatting, the two Oklab round trips, a defringe GPU twin, M8.11's inner chain (`grep -n 'accelerate_inner' docs/JOURNAL.md`) · g ⏸ the u8 coverage wire (a shader restructure), `MAX_TILE_DABS` (portability) · h ⏸ the reference photo on the GPU (parked 2026-10-13 until a need shows — 1 of 74 photos; 111–125 ms at 3200, numbers in the backlog) |
| **M10.19** found in use (added 2026-10-01) | a HEIC/HEIF photos — libraw cannot read them, so they never appear; needs a decoder · b Move to folder: download Immich assets first, then move (today skipped and counted) (c, d ✅ above) |

**Phase 11, last:** M11.2 Windows + the Linux GUI (first run and confirmed 2026-10-27 on Arch/X11; still unchecked: Wayland,
multi-monitor), M11.3 contributor docs, M11.4 `v0.1` (with the welcome screen and the empty Library's
text, moved there from M10.6 by the author), M11.5 plugins. ⛔ **M11.1 (was M10.1) is
WITHDRAWN — it was never real** (the licence has been committed since `e71330d`); do not re-open it.
⚠ **Retired ids:** M9.1–M9.5 and M10.1–M10.5 moved with their work (M9.2 → M10.2 → M11.2), so Phase 10
starts at **M10.6**; M5.1c and M6.4c kept their own ids.

**How the author closes a milestone:** he tries it in the GUI and says "works, merge and delete the
branch" — then merge `--no-ff`, delete it, mark the journal, this file, the backlog map and the vault ✅.
He reports what he sees precisely (take it literally) and asks for domain terms explained plainly first.

⚠⚠ **The GPU port is finished** — done, declined with numbers, or parked/blocked (M10.14f/g).
⛔ Declined with numbers in `docs/BACKLOG.md` — do not re-propose from a list: the prefix seams'
download half in general, spot removal on the GPU, `MIN_GPU_BOX_PX`, the GPU-side coverage cache, one
GPU device for the window and the pipeline.

### The GPU port — closed (reopened once, for geometry, 2026-09-26)

⚠⚠ **22 of the 27 stages that `impl PipelineStage`** have GPU twins (2026-10-15, Lens Blur's the newest;
`per_pixel`'s three test doubles and the GPU crate's four probes not counted). The five without — `spot_removal` (measured and closed unbuilt), `defringe`
(parked, M10.14f), `red_eye`, `negative` and `ai_denoise` (a blend; its network's GPU engine is M10.16b2) — are all **prefix steps**: every TAIL stage has a twin.
⚠⚠ **Geometry was declined too, and that decline was WRONG for one reason worth remembering: it priced the COMPUTE and missed the TRANSFERS.** On a
fitted rung of 6.8 Mpx a Straighten frame was 63 ms, and the twin's value was that it let the pixels
stay on the GPU: a resident input and a no-download handoff to the tail. ⚠ **Do not re-propose from a
list alone**; the order with each row's cost is in `docs/JOURNAL.md` (`grep -n "agreed order"`) and
the declines with their numbers in `docs/BACKLOG.md`. ⚠ Count by `impl PipelineStage for` and
`impl (crate::)?GpuStage for` — exposure writes the `crate::` form, and a grep without it undercounts
by one (it did, 2026-09-26).

### Warnings that outlive their milestone

⚠⚠ **All open work is Phase 10 above.** A finished item is deleted from there, not struck through — its
record is in `docs/JOURNAL.md`. Only what stays true after a milestone closes belongs here.

- ⚠⚠ **A macOS-only module (`#[cfg(target_os = "macos")] mod …` in `main.rs` — `native_menu`,
  `display_colorspace`) is gated at EVERY reference, its `use` lines included** — one ungated import in
  `bootstrap.rs` kept the app from compiling off macOS for four weeks, invisible to a gate run only on macOS.
  Development now happens on Linux too; a change to such a module is built on both before it merges.
- ⚠⚠ **A test's directory comes from `test_dir::TempDir` (deleted on drop) or `test_dir::path` (kept,
  swept by the next run)** — unique per test and per run under `rustroom-tests/<pid>/`. A sweep in
  `test-dir` fails on `env::temp_dir()` anywhere else; the export staging directory is the one allowed
  site. Two gates at once are safe since M10.7c (on the old code they failed *and* hung).
- ⚠⚠ **A test that waits for a background job waits on the EVENT, under a long ceiling (120 s) against a hang —
  never a fixed time budget.** The denoise worker's 1 s and 6 s budgets lost the race on a loaded full gate and
  failed `main` twice (fixed 2026-10-22, `denoise_worker.rs`'s `LONG` and its `Held` network, which holds a tile
  until the test has read the progress). ⚠⚠ **And a test that asserts an IN-BETWEEN state holds the job there
  until it has looked** (2026-10-27): one `pump_people` spawns a search and polls it, so an instant finder landed
  before "the search started" was read — held behind a `Gate` now; a held job says WHEN it is held (`Held.holding`),
  since progress is published before the tile's partial is.
- ⚠⚠ **A keyword, collection, smart-collection or date view follows the catalog** (2026-10-01):
  `source_refresh::pump_source_membership`, last in the frame, re-reads `source_members` once
  `Catalog::change_count` has moved and settled — never a list of writers. A new catalog-backed source
  must answer `source_members`, and its opener must read it too. ⚠ `rebuild_entries(None)` keeps the
  selection **by id**; `Selection::clear()` keeps the active *index*, so never use it across a rebuild.
- ⚠⚠ **A new per-file catalog table joins `PER_FILE_TABLES`** (`catalog/src/db/moves.rs`) — the one list
  both a move Rustroom makes (`move_photo`, Move to folder) and one it only notices (`relink_moved`) re-key;
  a table missing from it leaves its rows behind. ⚠ A collection or smart-collection view shows only its
  member **copies** (`Library::copies`, applied in `browser_entries`); keyword and date views every copy.
- ⚠⚠ **NO SOURCE FILE OVER 1,000 LINES OF CODE AND DOC COMMENTS — the gate enforces it** (M10.7d):
  `test-dir`'s `no_file_is_over_a_thousand_lines_of_code_and_doc`. Its measure, `code_and_doc_lines`,
  counts non-blank lines outside every `#[cfg(test)]` item **wherever it sits**, and skips a file
  declared `#[cfg(test)] mod x;`. ⚠ The old `awk` to the *first* `#[cfg(test)]` is blind past a
  mid-file test item — it read `main.rs` as 78 lines where it held 555; do not use it. Nearest the
  ceiling on 2026-10-22: `app/src/session_flow.rs` **987**, `modals/export.rs` 796,
  `develop/panel/adjustments.rs` 794, `modals/settings.rs` 789, `collections_panel.rs` 779 — add nothing to
  `session_flow.rs`.
  ⚠⚠ **Re-measure, never requote** — set `CEILING` to 600 for the list, then set it back. Split by responsibility, never to dodge the number: the M10.7d spec and plan are the method.
- ⚠⚠ **A new `Edits` field is `#[serde(default, skip_serializing_if = "<T>::is_default")]`** (M10.11a/b),
  or the default's serialization changes and every rendered preview on disk misses at once.
  `the_fingerprint_is_stable_across_calls_and_pinned` catches it; re-pin only for a deliberate format change.
- ⚠⚠ **EVERYTHING THE LIBRARY RAIL DRAWS RUNS EVERY FRAME — no work there may grow with the catalog
  or an album** (2026-10-12: a cubic folder tree and a `stat` + JSON parse per Immich asset made the
  Library crawl while Develop, which draws no rail, stayed fast). Derive once per input, look up per
  frame, and read changing state (`immich_present`) live rather than invalidating a cache from a list
  of writers. ⚠ **Find it with `sample <pid> 8`**, never by reading code — and re-sample after each
  fix: twice the next cost was hidden behind the last. `grep -n 'became slow' docs/JOURNAL.md`.
  ⚠ The rail reads the catalog through `rail_data::RailData`, once per change (M10.14e) — a new
  rail fact joins it, never a query in the draw. ⚠⚠ **De-duplicate through `rustroom_app::unique`**,
  never a `Vec` searched per item: eight such loops made All Photos 204 s at 100k photos.
- ⚠⚠ **A mask's scene sliders (WB, Temp/Tint, Exposure, Contrast, the four regions) are OFFSETS in
  `SceneOffsetsStage`** (M10.12b), never its `MaskStage` (`pipeline::look_only`). A new scene field joins
  `has_scene_offsets`, `look_only`, `scene_only`, the stage and its GPU twin. A range/gating mask with an
  offset selects on the reference photo (`pipeline::reads_reference` — ONE predicate, also the brush
  cache's `base_id`), and its look stage blends by the same plane through a `CoverageSlot`.
- ⚠⚠ **A mask's coverage is folded in THREE places and keyed in THREE** (M10.12a): the folds are
  `Composite::at` (CPU), `mask_wgsl::coverage_entry` (GPU) and the Show-mask tint; the shape keys are
  `capture_projection`, `defringe_projection` (prefix) and `TintCache::same_shape`. A new field that
  changes coverage must reach all six — `same_shape`'s exhaustive destructure forces the tint's choice,
  the projections are literals without `..`. ⚠ And a missing AI map covers **nothing**, inverted too
  (`Composite::waits_for_a_map`).
- ⚠⚠ **A new TAIL stage needs a GPU twin with a `REPLACEABLE` row AND a field set in `accelerate.rs`'s
  `every_tail_stage()`.** That fixture is hand-built: a stage with neither is invisible to
  `every_tail_stage_is_replaceable_or_a_mask`, and the GPU tail silently splits at it.
- ⚠⚠ **A map-reading component (`Mask::Ai`, `Mask::Depth` — `Mask::reads_a_map`) turns into coverage through
  ONE path, `ai_coverage::AiSource::component`** (M10.16a; the preview's brush cache, export and the tint all
  call it), and reaches a mask's NR / Dehaze / Defringe through `build_prefix_steps`'s injected `AiRasters`,
  each step's key carrying the map's CONTENT id — or a map arriving after a checkpoint was cached leaves the
  stale picture. ⚠ **No AI request from a ghost session** (`buffer_is_ghost`): its map would be cached under
  the file's key for good. ⚠ The Subject/Click cache keys are pinned as literal digests — maps on disk.
- ⚠ **A new PREFIX step trips three tests in `prefix.rs` that count or index the steps** (truncated
  key, capture stages, own key) — on purpose: renumber them; the step's `PrefixStepKey` carries its whole setting.
  ⚠ A step that waits for AI maps (Lens Blur, M10.16d) is invisible to a test built with `ai = None` —
  `every_prefix_step_has_its_own_key` is handed `lens_blur_tests::MAPS` for that reason.
- ⚠⚠ **A prefix step reading AI maps keys on their CONTENT ids** (`AiRasters::source` / `plane`), and anything
  it derives from them is memoised on those ids — `build_prefix_steps` runs EVERY FRAME, so Lens Blur's
  Subject focus (a median over every subject pixel) lives in `lens_blur_tool::subject_focus`'s memo, and
  its planes in `ai_coverage::PlaneMemo` — keyed by an exhaustive destructure of `MapRaster`, so a field
  added there must join the key. Its CPU step and GPU twin read them through ONE lookup,
  `pipeline::lens_blur::inputs`.
- ⚠⚠ **AI denoise's result belongs to ONE decode** (M10.16b): it enters the maps only over the full-resolution
  decode of its own size (`denoise_flow`; an export over its own), is answered per frame only as
  `downscale_to` of the full image (`denoised::Denoised`), and its step is the prefix's FIRST. ⚠ **A rendered
  preview never runs the network** (`DenoiseUse::Cached`): without a result its build fails and runs later.
- ⚠⚠ **A long GPU job shares the render engine's queue, and Metal runs it in order** (M10.16b2): submitted
  at once, DRUNet's tile made an editing frame wait 1,354 ms. It submits bands of ~2.7 ms, two in flight
  (`drunet::conv::SUBMISSION_MACS`): 15 ms. Any future background GPU work must do the same.
  ⚠ Since M10.16b5 (`f16` + Winograd) its result is **not** `tract`'s bits: measured 1.42e-3 on photos,
  3.21e-3 at worst (`denoised.rs`'s `TOL` / `TOL_EXTREME`); a non-finite tile is an error, never the photo.
- ⚠⚠ **AI denoise's network (`settings::DenoiseNetwork`, M10.16b6) is in EVERY key that holds its pixels** — the
  cache key, the rendered preview's (with Amount on), `ExportRender`, the download offer — and the flow's worker is
  paired with the network it was made for. A new place that stores or reads a result takes the network too, or one
  network's pixels pass for the other's. ⚠ `ai_mask::pth` reads a `.pth` through a CLOSED pickle subset: never
  widen it to "any global" — a pickle is a program.
- ⚠⚠ **Never build a lens `Corrector` per frame** — it samples its whole displacement field (290–370 ms on an
  X-T4); Retouch did, on every hover frame, until M10.16c1's review. Read it through the session's
  `ai_space::CorrectorMemo` (`Session.lens_memo`).
- ⚠⚠ **A removal (M10.16c1) lives in DECODE space and its fill is pasted right AFTER AI denoise, BEFORE lens and
  the negative** — AI denoise's result still holds the object. The fill is computed on the positive and stored in the
  film's units (`pipeline::negative::negative_of`), keyed by `remove_tool::id` (never Opacity; bump its `RECIPE` on any
  change to how a fill is made), and reaches the prefix through `AiRasters::fill` on its content id. ⚠ A decode-space
  stroke is drawn through `DecodeMap::decode_to_frame` — the lens has no forward map, its inverse is iterated.
  ⚠ Since M10.16c3 the detector (Detect objects, a person's removal) sees the fill's window UPRIGHT — the request
  carries the photo's quarter turns (`RemovalRequest.turns`); a person's removal is `Removal.person`, a box, no stroke.
- ⚠⚠ **Every reader of the undeveloped decode (`Session.buffer`) sees the positive** (M10.11a): a pixel
  through `negative_tool::view`, an image through `pipeline::negative::positive` (Auto tone, the AI
  input), and a cache of anything derived from it keyed on `edits.negative` (`ai_cache::key`, the AI
  input cache). A new eyedropper or analysis that skips it measures the negative.

- ⚠⚠ **A panorama's lens is corrected INSIDE its one resample and its DNG names no lens on purpose** (M10.17b): a named
  lens would be corrected twice by `auto_lens_correction`. The result opens with `lens_correction: Some(default)` and Auto
  crop's rectangle. ⚠ `align::align` projects its start onto a similarity — never start it from a panorama pair's
  rotation; `align::refine_from` is the homography-only path. ⚠ A stitch's pixels outside a frame's coverage or where it
  is clipped come from the OWNER in the blend (`blend::multiband`), never filler.
- ⚠⚠ **A floating-point DNG — the HDR merge's result — is read by `raw_decode::float_dng`, never libraw**
  (M10.17a): the libraw `rsraw` bundles is built without zlib (a Deflate float DNG's pixels fail to unpack) and
  clips float data to ~1 (3.0 read 1.07). `decode_raw` sniffs for it; libraw still reads its capture facts, size
  and thumbnail. ⚠ libraw leaves `cam_xyz` **empty for a DNG** — `decode_camera` derives one from `rgb_cam`.
  ⚠ A bracket's exposures are **measured, never read**: DSCF7755's EXIF says −0.42 EV, its pixels +0.002.

### Standing lessons (the expensive ones — full stories in `docs/JOURNAL.md`)

- **⚠⚠ A SOURCE SWEEP THAT READS "UP TO THE FIRST `#[cfg(test)]`" IS BLIND PAST ANY TEST ITEM PLACED
  MID-FILE** — three were, until M10.6b found a literal font size and three bare hovers behind one.
  Use `theme::tests::production_code`, which blanks only the test items.
- **⚠⚠ A guard that enumerates a list stops guarding the moment the list grows.** The
  double-application test named three of five fields and was therefore blind to a whole milestone —
  the real sabotage left it at `1 passed` while six goldens failed. Read this before adding anything
  to `adjustment_stages`.
- **⚠⚠ A TEST THAT CANNOT FAIL IS NOT A TEST, AND FOUR SHAPES OF THAT HAVE COST THIS PROJECT REAL BUGS.**
  **(a)** A fixture that makes two different things **equal** cannot see a bug about telling them apart —
  `asset("one")` with `original_file_name: "one.jpg"` made a UUID path and a display name coincide, and
  four tests passed against a catalog that could never find its own rows. **(b)** A query that finds
  **nothing** passes silently, so assert the sample first — `labels.len() > 12` caught three separate ways
  the Color guard opened nothing, and a substring filter matching no tests exits 0. ⚠⚠ **And `f32::max` IGNORES
  NaN**, so a `fold(0.0, f32::max)` parity check reads an all-NaN output as a perfect 0 — every GPU-vs-CPU check
  must fail on a non-finite value FIRST (M10.16b2; its real 0 was then proven by a sabotage, 3.4e18). ⚠⚠ **It applies to shell
  checks, and it cost an invented milestone (M10.1, now M11.1) on 2026-09-25:** `ls | grep -i license` returned
  nothing, so I reported no `LICENSE` — it had been committed since `e71330d`. **`ls` here is aliased to
  `eza --long`, which prints NOTHING to a pipe unless given a path argument**, exiting 0 with empty
  stderr. Use `/bin/ls` or `git ls-files` for anything you will act on, and **prove a negative with a
  positive control**: the same command must find something known to be there. ⚠⚠ **And a sweep that CLICKS
  must assert its targets are on the harness screen** — a slider off it "resets" untouched: the panel's reset
  sweep skipped sixteen sliders until M10.16d's sabotage stayed green (now one section per pass). **(c)** Comparing two
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

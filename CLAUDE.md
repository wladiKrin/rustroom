# CLAUDE.md — Rustroom

Project-specific guidance for Claude Code working in this repository.

**Rustroom** — an open-source alternative to Adobe Lightroom. (Binary: `rustroom`. Repo folder: `~/Projects/rustroom`. Note: the Obsidian planning-notes folder is still named `opensource_lightroom`.)

> **⚠️ Maintain this file — and keep it SMALL.** Update the architecture, crate layout, stack,
> conventions or status sections here whenever they meaningfully change. **But the history goes
> in `docs/JOURNAL.md`, not here.** This file is loaded into context in full, every session: it
> reached **1.2 MB (~337,000 tokens)** on 2026-09-16 by accumulating a session-by-session
> narrative, which is more than a whole context window before any work starts.
>
> **⚠⚠ The rule that keeps it small: NEVER append a superseded block.** Overwrite the live state
> in place and move the previous version into `docs/JOURNAL.md`. Twenty-four blocks marked
> *"⚠ Superseded"* had stacked up in here, every one of them already recorded in git history and
> in `docs/superpowers/`. **Target: this file stays under ~400 lines.** If a milestone writeup
> wants more room, it belongs in the journal.

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
geometry (crop/straighten) → noise reduction → exposure →
tone (highlights/shadows/curve) → color (HSL/grading) →
vignette → sharpen → output transform → encode
```

**Geometry runs first**, not last, so every adjustment acts on the final cropped pixels —
which is also what makes the vignette post-crop for free, with no separate mode. (Lens
corrections will join it there when `lensfun` lands.)

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
│   ├── immich/               # ✅ Immich client (zero GUI, `ureq`): search/buckets/album, `Asset`/`AlbumSummary`/`Bucket`. ⚠ DTOs deliberately narrow; nullable-vs-required audited against a *captured* reply
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs is **1,116** lines (it said 681 until 2026-09-23 — check, do not quote); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. lib=photo_info/when/export_summary/spot_tool/spot_source/visualize/grade_view/history/action_log/edit_history/selection/crop_tool/histogram/render_engine/thumb_loader/thumb_cache/thumb_window/metadata_scanner/date_tree/rule/rule_form/library_filter{RatingSet,FlagSet,LabelSet}/decode_worker/buffer_cache/view_region/full_res/format_filter/presets/export_presets/safe_name/settings/edit_clipboard/sidecar/sync/batch_export/photo_id/entries/file_rename/thumb_store/immich_source/immich_worker/immich_list_cache; bin-only modules=theme/segmented/list_row/modals{shell,settings,export,info,rule_edit,presets,edits}/name_edit/develop{mod,panel,canvas}/library/filmstrip/thumb/shell/collections_panel/immich_flow/immich_panel (egui lives here, not in the lib). ⚠ This tree also omits `crates/immich`, `crates/gpu-pipeline` and `crates/lens-profile` entirely — it is behind the workspace; trust `ls crates/`.
│   └── catalog/              # ✅ scan_folder → RAW+JPEG/PNG/TIFF entries (+ mtime); SortOrder/sort (name/modified/edited, case-insensitive, name tie-break); is_raw/is_supported (in-memory; SQLite DAM later) + Description per file (v7, M6.2)
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

### State (2026-09-23)

**2777 tests** (goldens unmoved since 2026-09-18). No branch. ⚠ The user **pushed** through
`074c62f` on 2026-09-23; `main` is **6 commits ahead of `origin/main`** — still never push.

✅ **The histogram survives a peak (2026-09-23, confirmed by use).** Square root with bins 0 and 255
**out of the scale**, in `histogram::normalised`/`envelope` — one function pair for the panel and the
tone curve's ghost, which each computed their own linear maximum before. ⚠⚠ Measured, and the limit is
pinned: 0.7 → 7.2 px at 100:1, but only 1.6 px at 2000:1, because sqrt compresses a ratio by its own
square root. **The test says not to fix that by lowering its threshold.** Ghost 0.15 → 0.35; the curve
centred like the colour wheel; the panel 320 pt. `grep -n "histogram's scale" docs/JOURNAL.md`.

⚠⚠ **A slider must never clamp a value it was merely handed.** `DragValue::range()` clamps *existing*
values by default and reports the clamp as `changed()`, so opening certain photos **froze Temp at a
slider limit** while the preset still read "As Shot". Fixed with `clamp_existing_to_range(false)` plus a
Temp range of **1667 K** matching `color::wb::solve_xy`'s own clamp, with a test comparing the two
bounds. ⚠ Not a regression from any recent work — possible since 2026-09-01. ⚠ Any slider handed an
out-of-range stored value now **displays** it rather than rewriting it.

⛔ **Per-channel RGB tone curves: considered, deferred by the user.** They make sense and are standard,
but `ToneCurve` is one `Vec<[f32; 2]>`, so it means four curves, a pipeline and golden change, a sidecar
version bump with migration, and a channel selector. Costs written down in
`2026-09-23-histogram-and-panel-design.md`.

✅ **The Now row counts what DIFFERS, not history steps** (2026-09-23). `Edits::changed_parameters`,
⚠⚠ **derived from serialisation, never a field list** — 25 fields, no `#[serde(skip)]`, so a new
parameter counts itself. ⚠ `History::steps_since` is **gone**, having lost its only caller. ✅ **Reindex
is a plain button**, not a ⋯ menu holding one item.
`grep -n 'counts difference, not history steps' docs/JOURNAL.md`.

✅ **Reindex refetches from Immich (2026-09-23) — merged and confirmed by use.**
On an Immich album or month, ⋯ → Reindex persists the session, compacts copy numbering, **removes that
album's entry from `immich_assets`** (the one load-bearing line — the session map is checked *before*
the validator and was never invalidated, so a change in Immich appeared only on a relaunch), and asks
for the matching rail list plus the asset list. ⚠⚠ This needed `ImmichWorker`'s single `pending_list`
flag **split in two**: it covered both `Albums` and `Buckets`, so one of two requests in a frame was
dropped silently and a bucket reply cleared an outstanding album fetch. The rail survived that by
re-requesting every frame; a one-shot Reindex would not have. Spec
`2026-09-23-immich-reindex-design.md`; `grep -n 'Reindex refetches' docs/JOURNAL.md`.

✅ **Immich asset lists are persisted (2026-09-23) — merged and confirmed by use.**
One release run wrote **8 lists / 2,155 assets / 618 KB**, all internally consistent, no panic, no
`.tmp` left (one holds **1,022** assets, above `PAGE_SIZE`, so paging is complete). ⚠ What remains
unverified is that a **relaunch opens one with no wait**. ⚠ Measured at **280–303 B/asset** — the
spec's ~175 KB per 872-asset album was **wrong** (really 238 KB; ~9.8 MB for 35k, not ~7). A new lib module
`immich_list_cache` writes each fetched list to `<cache>/rustroom/immich/<host_slug>-lists/`, and a
click tries the session map, then the stored list **if its validator still matches**, then a fetch.
The album validator is **`(assetCount, lastModifiedAssetTimestamp)`** — ⚠⚠ **not `updatedAt`, which
the captured reply shows identical for every album**; a bucket's is its count alone. ⚠ Validate first:
no validator (list never fetched, failed, or album unknown) means **no hit and no write**, so a stale
grid is never shown. ⚠⚠ The lists directory is a **sibling** of the originals because
`is_immich_path` is `starts_with(root)` and `asset_id_of` is a path stem. Spec
`2026-09-23-immich-list-cache-design.md`; `grep -n 'asset lists persisted' docs/JOURNAL.md`.

✅ **Immich slice 2's three use-found defects are fixed and confirmed (2026-09-23):** a **`null`
`thumbhash`** failed a whole month (⚠⚠ `required: true` does **not** mean non-null — `nullable` sits
beside it); a second album click was **silently discarded** by a shared guard; and switching *felt*
slow because a click gave **no feedback** for the couple of seconds the server spent on ~900 KB.
`grep -n 'three defects found by using' docs/JOURNAL.md`. ⚠ The CI workflow was fixed for the runner's
disk on 2026-09-21 and is **unverified until the next push**.

✅ **Immich slices 1 and 2 — merged and confirmed by use (2026-09-22/23).** A sixth rail section
lists the server's albums and a **Timeline** of years and months; a grid fills from server thumbnails
with nothing downloaded; opening a photo fetches the original into a cache directory and re-enters
`open_path`, after which every existing path applies unchanged. Immich thumbnails persist, keyed
`(asset id, updatedAt)`, and `ThumbStore` finally has a **byte budget with LRU eviction** — two of
them, 5120 MB each in Settings, on separate directories. It had **no eviction at all** since M8.2.
⚠ **No catalog migration — `db.rs` stays at v7**, which is what shape A bought. Specs
`2026-09-22-immich-browse-design.md`, `2026-09-23-immich-timeline-design.md`;
`grep -n 'M9.1 slice' docs/JOURNAL.md`.

⚠ **Immich, out of scope by decision:** catalog rows (**so marks do not persist on Immich photos**),
eviction of cached originals, the OS keychain (the API key is **plaintext** in `settings.json`),
write-back. ⚠ Slice 1's three defects — the missing `assets` field, `nextPage` paging, the absent
`catalog::sort` — are in the journal and their lesson is under **Standing lessons**.

✅ **Settled; detail in the journal:** the thumbnail store pins a concrete filename rather than
trusting "pinned hash vectors" (which pin `fnv1a`, not `Identity`'s key), and RTHM has two layouts —
`VERSION_LOCAL` 1, `VERSION_REMOTE` 2. **Both date trees** order years newest-first with months
chronological inside (`capture_days` orders `1 DESC, 2, 3`); ⚠ `date_tree::group` groups on *change*,
not direction, so it needed no change either time.

✅ **All merged and confirmed by use; specs under `docs/superpowers/specs/`:** the **Export dialog**
as a preset list beside summarised sections and **the dialogs in one language** (2026-09-22), the
**snapshot timeline** (2026-09-21), the **top bar and Library in one language** (2026-09-20), and the
five **panel reworks** (Develop, Masks, Geometry, Retouch, Presets) plus the mask off switch
(2026-09-18/19/20).

⚠⚠ **This block is the LIVE STATE, not a changelog.** Every milestone below is one line with its
outcome; the measurements, the retractions and the things that went wrong live in
`docs/JOURNAL.md`. This section reached a full screen of per-milestone tables twice — that is the
1.2 MB failure in miniature. **Overwrite it; do not append to it.**

**Landed 2026-09-18** — M8.13, M8.14, the `batch_export.rs` split, the rendering seam reaching export
and thumbnails, `ProfileRendering` becoming a required argument, and the GPU tail on by default: all
merged and confirmed by use. ⚠ The measurements, the three defects of one shape, and the one-word
`#[serde(default)]` trap that would have shipped `false` to every existing settings file are in
`docs/JOURNAL.md` — `grep -n 'M8.13\|M8.14' docs/JOURNAL.md`.

### ▶ Next action — Immich slice 3, or the leftovers

⚠⚠ **The GPU port is finished** — done, declined with numbers, or blocked on a considered trade. Do
not go looking for the next port; there isn't one.

**Live candidates, none urgent:**

0. **Immich slice 3** — catalog rows, so **ratings/flags/labels persist on Immich photos** (they do
   not today), and two-way sync. ⚠ The largest remaining gap in the integration, and the only one the
   user would notice. ⚠ The album/bucket **rail lists** are also still refetched every launch; only
   *asset* lists are cached.

1. **The u8 coverage wire** — priced: **0.56–0.70 ms against an actual 3.55–6.35** at rung 3200,
   **3.4 against 15.3** at export. ⚠⚠ Blocked on a shader restructure — the shader writes one `u32`
   per sample because packing four into a word would race. **A considered trade, not an oversight.**
2. **Rename `ProfileRendering` → `TailRendering`** — `look` is not a camera-profile field. Now a
   ~120-site diff. ⚠ Its own change, never folded into another.
3. **`crates/lens-profile/src/lib.rs` is 949 lines**, **`crates/app/src/immich_source.rs` is 821**
   and `main.rs` is **1,116**; `batch_export/worker.rs` is 1,127 (accepted — ~190 of code, the rest
   its own tests).
4. Move `PrefixCache`'s tests into `prefix_cache.rs`.

⛔ **Declined with numbers, and the numbers are in `docs/BACKLOG.md` — do not re-propose from the
list alone:** the prefix seams' *download* half (11–13%, needs `PrefixCache` to hold a `GpuImage`),
spot removal on the GPU (0.05–0.4 ms against a ~1.6 ms floor), `MIN_GPU_BOX_PX` (user's call, on
use), the GPU-side coverage cache (re-priced downward).

⚠ **`MAX_TILE_DABS`** — swept; the cliff is at **2560, not 3584**. Blocked on a **portability**
question (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) that no M2 run
settles.

### The GPU port — closed

⚠⚠ **Finished. There is no next item on the agreed order, and this is the third document to say so.**
All five rows are done, declined with numbers, or blocked on a considered trade; **18 of the 21 stages
that `impl PipelineStage` have GPU twins**, and the three that do not — `geometry`, `perspective`,
`spot_removal` — are **deliberate**, not pending. Region scoping (M8.8) answered the first two without
a shader; spot removal was **measured and closed unbuilt** (0.05–0.4 ms of work against
`GpuBrushRaster`'s ~1.6 ms floor, 1.28 of it a fixed download).

⚠ **Do not go looking for the next port, and do not re-propose from a list alone** — "last unported"
is not "worth doing". The order, with what each row cost and why, is in `docs/JOURNAL.md`
(`grep -n "agreed order" docs/JOURNAL.md`); the declines with their numbers are in `docs/BACKLOG.md`.
⚠ Count stages by `impl PipelineStage for`, never by filename: two stages are helpers and two twins
are renamed, so a filename comparison undercounts.

### Open items

⚠⚠ **Live items only.** A finished item is deleted from here, not struck through — its record is in
`docs/JOURNAL.md` and its numbers in `docs/BACKLOG.md`. Four ✅ entries had stacked up here by
2026-09-18, which is the 1.2 MB failure starting again.

- **A GPU-side mask coverage cache** — ✅ measured 2026-09-16; the ceiling missed 4–8×. ⚠⚠ The cost
  is **not transfer**: a serial u8→u32 repack and a 4×-inflated download wire. **Fix those two
  first** — mechanical, no design question. Re-price after.
- ⚠ **The u8 coverage wire**, the **`ProfileRendering` → `TailRendering`** rename and **`MAX_TILE_DABS`**
  are listed with their numbers and their blockers under **▶ Next action** above — not repeated here,
  because a list kept in two places is how this file reached 1.2 MB.
- The prefix-cache eviction fix's **picture** half.
- Clarity's **coalescing** on the horizontal running-sum pass; **M8.1**'s leftovers.
- Move `PrefixCache`'s tests into `prefix_cache.rs`.
- ⚠ **M7.3b is DEFERRED, not done.**
- ⚠ Past the ~600 threshold and wanting a split: `shell.rs` **1,379**, `main.rs` **1,116**,
  `lens-profile/src/lib.rs` **949**, `immich_source.rs` **821**, `immich_worker.rs` **698**.
  (`batch_export/worker.rs` is 1,127 deliberately — ~190 of code, the rest its own tests.)
- The **7 temp-dir test-isolation sites** — ⚠⚠ **never run two gates concurrently.** ⚠ `immich_source`
  adds **two more** that key only on the process id, with no counter; `immich_list_cache`'s and
  `thumb_store`'s use a counter as well and are the pattern to copy.
- ⚠ **Immich marks do not persist** (no catalog rows), the **API key is plaintext** in `settings.json`
  (no keychain), and there is **no eviction for the cached originals** — all three out of scope by
  decision, not oversights.
- ⚠ The **rail's** album and bucket lists are refetched on every launch; only the *asset* lists are
  cached (2026-09-23).

### The open question M8.11 did not settle

M8.3j/k/l's inner-chain machinery is reachable **only on the decline path** (a gating brush with a cold
cache). Kept, pinned with the gating-brush wedge; whether it earns its keep now that a GPU-resident
brush raster exists is worth asking when something touches it. `grep -n 'accelerate_inner' docs/JOURNAL.md`.

### Standing lessons (the expensive ones — full stories in `docs/JOURNAL.md`)

- **⚠⚠ A guard that enumerates a list stops guarding the moment the list grows.** The
  double-application test named three of five fields and was therefore blind to a whole milestone —
  the real sabotage left it at `1 passed` while six goldens failed. Read this before adding anything
  to `adjustment_stages`.
- **⚠⚠ LOWERING AN ASSERTION TO MATCH THE CODE IS HOW A TEST STOPS BEING EVIDENCE.** The first
  histogram test asserted that detail beside a 2000:1 peak would reach 5% of the well; it reaches
  **2.2%**, because square root compresses a ratio by its own square root. The measurement was the
  useful part: the test was **rewritten around it** — a table of linear-vs-sqrt pixel heights, plus a
  second test pinning the extreme case as a known limit and saying *do not fix this by lowering the
  threshold*. ⚠ When a threshold you invented fails, find out what the real number is before touching
  either side.
- **⚠⚠ "THAT OTHER BRANCH IS FINE" IS A CLAIM, AND READING ONE LINE IS NOT RUNNING IT.** Fixing the
  snapshot row's change count on 2026-09-23, I reported that "the snapshot branch never had this bug"
  — inferred from seeing `if edits == current` and never executed. The user found it the same day: with
  a snapshot taken mid-edit, set/reset/set read as **3 changes**. ⚠ The fix had been verified against
  returning **to** the reference state, which the old code already handled; passing **through** it is
  what broke, and that is the shape the reported gesture actually has. **When a fix touches one branch
  of a two-branch function, run the other branch before saying anything about it** — a probe printing
  both took one command.
- **⚠⚠ A TEST'S DOC COMMENT IS A CLAIM ABOUT WHAT IT CATCHES, AND AN UNVERIFIED ONE IS WORSE THAN
  NONE.** On 2026-09-23 a new test's comment said reading `updated_at` instead "would have produced a
  cache that never invalidates" — `AlbumSummary` has no such field, so that sabotage is impossible and
  the comment advertised a guard the test did not provide. ⚠ **Run the sabotage, then describe the one
  you ran.** Two tests did fail when the `find` became `first()`; that is what the comment now says.
  Same family as the enumerating guard above: a reassuring sentence is not a guard.
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
- **⚠⚠ Measure the drag through `RUSTROOM_DEBUG_RENDER`, never a probe row.** M8.6's 12.5–13.3×
  headline was retracted because `probe_resamplers` priced a *stage* against a resume the real chain
  never took — it shipped and did nothing on the reporter's chain.
- **⚠⚠ A CLAIM RESTATED OFTEN ENOUGH STOPS BEING CHECKED — and it happens to numbers, not just
  blockers.** "NR and dehaze run before geometry" propagated through four documents and was simply
  false. So did the demosaic tier's *"~8 s a photo"*: that was the `Best`→`Fast` gap, quoted for
  four milestones in support of `Best`→`Better`, which measures at **~3.7 s**. ⚠ **Before requoting
  a figure, check which two configurations it compares** — and check any *"blocked on a design
  question"* note in `docs/BACKLOG.md` against the code before trusting it.
- **⚠ Almost every GUI-wiring defect in this repo was found by USE, not by a test.** A milestone is
  not closed when it is green; it is closed when it has been seen.
- **⚠⚠ AN OPENAPI SCHEMA DESCRIBES WHAT A SERVER *MAY* SEND, NOT WHAT IT *DOES*.** Three defects in
  two days, every one invisible to a green suite and obvious within a minute of real use: a
  hand-written fixture asserting a field the endpoint never returns; paging by `nextCursor` when the
  server sends `nextPage`; and reading `required` **without reading `nullable` beside it** — a `null`
  `thumbhash` then failed a whole month of the timeline. ⚠ When a field turns out to be nullable,
  **audit every field against the schema** rather than patching the one that broke: two more were
  waiting. And when an endpoint's shape matters, get a **captured reply** before writing the DTO.
- **⚠⚠ A FIXTURE YOU WROTE IS NOT EVIDENCE ABOUT SOMETHING YOU DO NOT CONTROL.** M9.1's
  `album.json` was hand-written and described *in its own plan* as "trimmed from `GET
  /albums/{id}`". It asserted an `assets` array that endpoint has never returned. The test passed
  because it checked an assumption against itself — **2,678 green tests, and the first real album
  open failed immediately.** The captured reply then exposed a second, worse bug the schema nowhere
  implies. **Every fixture must say whether it was captured or constructed**, and a constructed one
  proves only self-consistency. One real response is worth more than any number of invented ones.
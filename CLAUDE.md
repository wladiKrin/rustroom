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
│   ├── immich/               # ✅ Immich client (zero GUI, `ureq`): search/buckets/album/stacks/**upload** (+ `multipart.rs`, hand-rolled — ureq has none), `Asset`/`AlbumSummary`/`Bucket`/`Upload`. ⚠ DTOs deliberately narrow; nullable-vs-required audited against a *captured* reply
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs is **1,147 total but only 78 of code** (2026-09-24; it said 681, then 1,116 — **measure by code, do not quote either number**); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. lib=photo_info/when/export_summary/spot_tool/spot_source/visualize/grade_view/history/action_log/edit_history/selection/crop_tool/histogram/render_engine/thumb_loader/thumb_cache/thumb_window/metadata_scanner/date_tree/rule/rule_form/library_filter{RatingSet,FlagSet,LabelSet}/decode_worker/buffer_cache/view_region/full_res/format_filter/presets/export_presets/safe_name/settings/edit_clipboard/sidecar/sync/batch_export/photo_id/entries/file_rename/thumb_store/immich_source/immich_worker/immich_list_cache/immich_upload/stack; bin-only modules=theme/segmented/list_row/modals{shell,settings,export,info,rule_edit,presets,edits}/name_edit/develop{mod,panel,canvas}/library/filmstrip/thumb/shell/collections_panel/immich_flow/immich_panel (egui lives here, not in the lib). ⚠ This tree also omits `crates/immich`, `crates/gpu-pipeline` and `crates/lens-profile` entirely — it is behind the workspace; trust `ls crates/`.
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

### State (2026-09-24)

**2887 tests**, **75 goldens** (unmoved). No branch, tree clean.

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
format override in an all-paired folder. `grep -n 'stacks: same-stem' docs/JOURNAL.md`. ⚠ **Run the git command above rather than trusting a number here** — the user
pushed twice on 2026-09-24 while this block said otherwise. He last pushed **`431331f`**, which
**includes the CI fix**, so the next workflow run is the one that confirms Linux green. ⚠ Still never push.

✅ **CI on Linux verified green (2026-09-24)** after one failure — a test asserting an absolute path's
existence, fixed; see shape **(e)** in *Standing lessons*.

✅ **Immich slice 4 — Download / Remove downloads (2026-09-24, confirmed by use).** ⚠⚠ Its
load-bearing part is a **bounded download queue** (cap **4**): `request` spawns a thread per job, so
a bulk album would have spawned **872**. ⚠ Start the next job **after** `in_flight.remove`.
⚠⚠ **Removal deletes only the image** — sidecar, catalog row and thumbnail survive, and
`cache_path`'s byte-identical re-download reattaches them. `grep -n 'Immich slice 4' docs/JOURNAL.md`.
✅ **Per-channel RGB tone curves** (2026-09-24) — ⚠⚠ almost no colour science changed: `apply` was
already `map_samples`, so the composite was **always** per-channel with one shared table, and the 74
goldens stayed byte-identical. ⚠ Composite first, then the channel; `CurveChannel` lives on the
**session**, never `Edits`. ✅ **The Color section, made legible** (2026-09-24) — HSL and grading behind
**`ADVANCED COLOR`**, closed by default. ⚠⚠ Not three real sections: `Section::name()` is
`EDIT_GROUP_NAMES[i]`, so that would split the `Color` edit group **and** the Sync dialog.
✅ **The histogram survives a peak** (2026-09-23) — sqrt with bins 0 and 255 out of the scale;
⚠⚠ measured, and the test says **not** to fix its 2.2%-at-2000:1 limit by lowering the threshold.
✅ **The Now row counts what DIFFERS** via `Edits::changed_parameters`, ⚠⚠ derived from
**serialisation, never a field list**. `grep -n 'per-channel RGB\|made legible\|histogram.s scale'
docs/JOURNAL.md`.

⚠⚠ **The Immich invariants that are easy to break again:** the catalog keys a row on the **path's** own
file name, never `CatalogEntry::file_name`; `row_is_browsable` hides an absent **Immich** row from
catalog-wide views but **never** an absent local one; `present_names` is one `read_dir` filtered through
`catalog::is_supported`. ⚠ Out of scope by decision: eviction of cached originals, the OS keychain (keys
are **plaintext**), reading Immich's own `rating`/`isFavorite`. ⚠ The **rail's** album and bucket lists
are still refetched every launch; only *asset* lists are cached. ⚠ **Auto-advance is a Settings toggle**
already `true` in the user's file, so changing the Rust default would not change his behaviour.

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

### ▶ Next action — Immich upload (brainstorm in progress), or the leftovers

⚠⚠ **The GPU port is finished** — done, declined with numbers, or blocked on a considered trade. Do
not go looking for the next port; there isn't one.

**Live candidates, in the order the user has shown interest:**

0. ✅ **Immich upload BUILT and confirmed by use (2026-09-24), delivery reconciled into Location.**
   An export delivers to **files, Immich, or both** — one `ExportOptions.destination: Destination`
   (`Files`/`Target`/`Both`/`Nowhere`), deliberately **not** Immich-named so the integration can later
   be switched off with "no mention of immich at all". Immich-only **stages in a temp dir, deletes on
   a CONFIRMED upload, and rescues into the export folder otherwise**. ⚠⚠ Never delete on an attempt:
   an SSL/EOF means the *response* was lost, measured 4-in-2,373 with none actually failing to apply.
   The upload date is `Captured`/`Edited`/`Uploaded`; ⚠⚠ **there is no mtime fallback — it filed an
   unreadable date under the DOWNLOAD time**, and `Edited` is read at **plan** time because recording
   an upload bumps the sidecar's mtime. A separate **`immich_upload_key`** keeps browsing read-only as
   a property of the *credential*: ⚠⚠ **`asset.upload` is a grant SEPARATE from `asset.update`**
   (168-value enum), which is the whole basis of upload-over-sync.
   `grep -n 'export delivery' docs/JOURNAL.md`.
   ⛔ **Two-way sync is DECLINED (the user's call — not a deferral):** it *mutates* authoritative
   assets and runs unattended, removing the found-by-use safety net where the blast radius is largest.
   ⚠⚠ **Do not re-propose it from a list**; the table is in `docs/BACKLOG.md`.
   ✅ **Immich is now COMPLETELY OPTIONAL behind one gate (2026-09-24).** `Settings.immich_enabled`
   is read **only** through **`immich_source::connection`** — the single choke point nine call sites
   already used — so no surface has its own condition and the export ticks needed no change.
   ⚠⚠ **Default `true`: the `gpu_tail` trap**, since `false` would make Immich vanish on upgrade.
   ⚠ The Settings tab **stays** (it holds the toggle). The downloads stay visible as **"Downloaded
   photos"**, hover included — the path itself says "immich". `grep -n 'completely optional'
   docs/JOURNAL.md`.
   ⬜ **The one Immich item left:** **honouring the server's stacks.** ⚠⚠ Local stacking (2026-09-25)
   does **nothing** for Immich and cannot: `cache_path` names files by **asset UUID**, so no two ever
   share a stem — measured, **0 pairs** against 2,913 assets. The wiring is already right (Immich goes
   through `open_scan` → `browser_entries`), so this plugs in at the same place; what it needs is the
   server's 2,379 stacks read via `GET /stacks` and mapped onto rows.

1. **The u8 coverage wire** — priced: **0.56–0.70 ms against an actual 3.55–6.35** at rung 3200,
   **3.4 against 15.3** at export. ⚠⚠ Blocked on a shader restructure — the shader writes one `u32`
   per sample because packing four into a word would race. **A considered trade, not an oversight.**
2. **Rename `ProfileRendering` → `TailRendering`** — `look` is not a camera-profile field. Now a
   ~120-site diff. ⚠ Its own change, never folded into another.
3. ⚠⚠ **This row used to rank files by TOTAL lines, contradicting the *Open items* rule directly
   below it — re-measured by code 2026-09-24, and NONE of the four is over the ~600 threshold:**
   `lens-profile/src/lib.rs` **500** (of 949), `batch_export/worker.rs` **439** (of 1,264),
   `immich_source.rs` **411** (of 896), `main.rs` **78** (of 1,147). The genuinely large files are the
   four in *Open items* — `edit-model` 1,883, `pipeline` 1,810, `catalog/db.rs` 1,730,
   `settings.rs` 1,046. ⚠ Measure with `awk '/#\[cfg\(test\)\]/{exit} {n++}'` before quoting any of this.
4. Move `PrefixCache`'s tests into `prefix_cache.rs`.

⛔ **Declined with numbers, and the numbers are in `docs/BACKLOG.md` — do not re-propose from the
list alone:** the prefix seams' *download* half (11–13%, needs `PrefixCache` to hold a `GpuImage`),
spot removal on the GPU (0.05–0.4 ms against a ~1.6 ms floor), `MIN_GPU_BOX_PX` (user's call, on
use), the GPU-side coverage cache (re-priced downward).

⚠ **`MAX_TILE_DABS`** — swept; the cliff is at **2560, not 3584**. Blocked on a **portability**
question (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) that no M2 run
settles.

### The GPU port — closed

⚠⚠ **Finished, and three documents now say so.** **18 of the 21 stages that `impl PipelineStage`** have
GPU twins; the three without — `geometry`, `perspective`, `spot_removal` — are **deliberate**, not
pending. Region scoping (M8.8) answered the first two without a shader, and spot removal was
**measured and closed unbuilt** (0.05–0.4 ms of work against `GpuBrushRaster`'s ~1.6 ms floor).

⚠ **Do not re-propose from a list alone** — "last unported" is not "worth doing". The order with each
row's cost is in `docs/JOURNAL.md` (`grep -n "agreed order" docs/JOURNAL.md`); the declines with their
numbers are in `docs/BACKLOG.md`. ⚠ Count by `impl PipelineStage for`, never by filename: two stages are
helpers and two twins are renamed, so a filename comparison undercounts.

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
- ⚠⚠ **MEASURE A FILE BY ITS CODE, NOT ITS TOTAL — this list was wrong in both directions until
  2026-09-24.** Every quoted figure here had been a *total*, and this repo's files carry enormous test
  tails, so the list named files that are fine and missed the ones that are not. Measured with
  `awk '/#\[cfg\(test\)\]/{exit} {n++}'` (code **plus** doc comments, which are heavy here):
  - **Genuinely large, and none was previously listed:** `edit-model/src/lib.rs` **1,883**,
    `pipeline/src/lib.rs` **1,810**, `catalog/src/db.rs` **1,730**, `app/src/settings.rs` **1,046**.
  - **Wrongly listed and actually fine:** `immich_source.rs` is **411** (not 896), `render_engine.rs` is
    **169** of 4,800, `develop/canvas/mod.rs` is **16** of 2,136. ⚠ `adjustments.rs` is **610**, barely
    over — not the "~1,250" its total suggests.
  - ⚠⚠ **`batch_export/worker.rs`'s own figures went stale too, and this entry used to hold them up as
    the precedent** — it said *"1,127 — ~190 of code"*; re-measured 2026-09-24 it is **1,264 total / 439
    code**. The *method* was the precedent worth keeping, not the numbers: a measurement written down
    once decays exactly like the totals this entry was created to correct. **Re-measure, never requote.**
- The **7 temp-dir test-isolation sites** — ⚠⚠ **never run two gates concurrently.** ⚠ `immich_source`
  adds **two more** that key only on the process id, with no counter; `immich_list_cache`'s and
  `thumb_store`'s use a counter as well and are the pattern to copy.
- ⚠ **Immich: the API key is plaintext** in `settings.json` (no keychain) and there is **no eviction for
  the cached originals** — both out of scope by decision. ✅ Marks **do** persist since slice 3.
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
- **⚠⚠ A TEST THAT CANNOT FAIL IS NOT A TEST, AND FOUR SHAPES OF THAT HAVE COST THIS PROJECT REAL BUGS.**
  **(a)** A fixture that makes two different things **equal** cannot see a bug about telling them apart —
  `asset("one")` with `original_file_name: "one.jpg"` made a UUID path and a display name coincide, and
  four tests passed against a catalog that could never find its own rows. **(b)** A query that finds
  **nothing** passes silently, so assert the sample first — `labels.len() > 12` caught three separate ways
  the Color guard opened nothing, and a substring filter matching no tests exits 0. **(c)** Comparing two
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
- **⚠⚠ A CLAIM YOU DID NOT RUN IS NOT A FINDING — INCLUDING A CLAIM ABOUT A TEST.** Twice in two days:
  I reported *"the snapshot branch never had this bug"* from reading one line of a two-branch function,
  and the user found it the same day; and a test's doc comment advertised catching a sabotage that
  `AlbumSummary`'s fields make impossible. ⚠ **Run the other branch before describing it, and run the
  sabotage before claiming the test catches it** — a probe printing both cases took one command, and the
  four guards of 2026-09-24 were each sabotage-measured (30.6 levels, 209 levels over 8,248 of 9,216
  samples, and so on) rather than trusted on a green run.
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
- **⚠⚠ BEFORE DECLARING SOMETHING "ONLY USE CAN VERIFY", LOOK FOR THE HARNESS THAT ALREADY ALMOST COVERS
  IT.** I twice told the user that preview-versus-export parity for the RGB curves was the one thing only
  he could check. `a_colorimetric_frame_renders_the_same_picture_on_the_gpu` already existed — it just ran
  on `Edits::default()`, so it could not see a `GPU_STAGES` site passing only the composite curve, and
  **that site compiles perfectly**. One test with three channel curves turned this project's worst failure
  shape into a one-second check, sabotage-measured at **209 levels over 8,248 of 9,216 samples**. ⚠ "Found
  by use" is a fact about what happened, never a reason a thing is unreachable by a test.
- **⚠⚠ A FIXTURE YOU WROTE IS NOT EVIDENCE ABOUT SOMETHING YOU DO NOT CONTROL.** M9.1's
  `album.json` was hand-written and described *in its own plan* as "trimmed from `GET
  /albums/{id}`". It asserted an `assets` array that endpoint has never returned. The test passed
  because it checked an assumption against itself — **2,678 green tests, and the first real album
  open failed immediately.** The captured reply then exposed a second, worse bug the schema nowhere
  implies. **Every fixture must say whether it was captured or constructed**, and a constructed one
  proves only self-consistency. One real response is worth more than any number of invented ones.
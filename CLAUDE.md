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
│   ├── rustroom/             # ✅ CLI crate; binary `rustroom` — `decode` (dump) + `render [--edits]` (RAW)
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs is 681 lines after W0 (2026-09-01); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. lib=spot_tool/spot_source/visualize/grade_view/history/action_log/edit_history/selection/crop_tool/histogram/render_engine/thumb_loader/thumb_cache/thumb_window/metadata_scanner/date_tree/rule/rule_form/library_filter{RatingSet,FlagSet,LabelSet}/decode_worker/buffer_cache/view_region/full_res/format_filter/presets/export_presets/safe_name/settings/edit_clipboard/sidecar/sync/batch_export/photo_id/entries/file_rename; bin-only modules=theme/modals/name_edit/develop{mod,panel,canvas}/library/filmstrip/thumb/shell (egui lives here, not in the lib)
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

### State (2026-09-16)

**2529 tests, 63 goldens.** The repo is **`main` alone** — no other branches, no worktrees, clean
tree. ⚠ `main` is **2 commits ahead of `origin/main`** (the CLAUDE.md split and a `.gitignore`
entry) — **not pushed; the user pushes himself.**

✅ **M8.11 — a mask's coverage and blend on the GPU — is merged, green, and CONFIRMED BY USE
(2026-09-16)** — *"works, confirmed"*, with no defects found by using it. A whole mask is one
`GpuStage`: seed, coverage, sub-stages and blend in a single compute pass, with the composite
folded into one coverage dispatch.

⚠ **Every GPU milestone in Phase 8 is now confirmed by use.**

⚠ Journal sections: `M8.11 confirmed by use (2026-09-16)` and `M8.11 — a mask's coverage and
blend on the GPU (2026-09-16)`.

### ▶ Next action — chase the 118 ms, and it is a ~10× gap on the path he actually edits through

**▶⚠⚠ A SCOPED 0.7 Mpx DEHAZE DRAG COSTS 118 ms AGAINST A 9–11 ms CACHE-HIT FRAME, AND NOTHING
EXPLAINS IT.** Found 2026-09-16 as a side-observation while re-measuring the post-NR checkpoint.
⚠ **`RUSTROOM_DEBUG_RENDER` cannot explain it — it prints no per-step lines on the scoped path**, so
the first work is **instrumentation, not optimisation**: make the scoped path report its steps, then
decompose. ⚠ One observation from a loaded machine — **reproduce it idle before scoping anything.**
Full entry at the head of `## Performance` in `docs/BACKLOG.md`.

**⚠ Why this outranks the three caching items:** they are worth 12–30 ms each and **two of them are
export-side**; this is **~107 ms on an interactive gesture**.

**✅ A MEASUREMENT WAVE RAN 2026-09-16 — four agents, four probes, all committed and re-runnable.**
It is the reason the list below changed. What it found:

- **The coverage cache's ~2 ms ceiling missed by 4–8×** (upload 7.9–15.8, download 4.0–9.4). ⚠⚠ **But
  the cost is not transfer**: `write_buffer` is 0.75–1.16 ms, exactly as M8.3g said. It is
  `GpuMaskStage::pack`'s **serial per-byte u8→u32 repack, 10.6–13.8 ms on every encode**, plus a
  download that crosses the wire as **one u32 per sample** (85–90% inflation). **Both are mechanical
  and open no design question — do them before the cache.**
- **The post-NR checkpoint's ~26 ms is CONFIRMED, and it saves ~0 under a viewport** — M8.8's
  region-scoped path takes over and no GPU prefix runs. ⚠ **M8.6's retraction shape.** Price it as an
  export win.
- **The "per-gating `MIN_GPU_BOX_PX`" item rests on a premise the measurement CONTRADICTS** — gating
  is worse at every box size, both configurations cross at the same rows. **Delete the item.**
- **`MAX_TILE_DABS`: the cliff is at 2560, not 3584**, and 1024 is the lowest value on the plateau.
  Blocked on a **portability** question no M2 run can settle.

⚠⚠ **A method lesson worth keeping: FOUR CONCURRENT TIMING AGENTS DEGRADE THEIR OWN MEASUREMENTS.**
Load hit 4.0–6.1; **4 of 7 checkpoint runs were unusable** and every `MIN_GPU_BOX_PX` crossover is an
upper bound. Reading parallelises; **timing does not**. Run probes serially, or on an idle machine.

### The agreed systematic order (GPU port)

⚠ **Structural, not popularity-driven.** The user **has not been using Rustroom yet**, so the
catalogue's 40 sidecars describe those folders and **not** what a photographer does. Three
milestones leaned on that scan; it must not be cited as a prioritisation argument again.

| # | Item | State |
|---|---|---|
| 1 | Finish the per-pixel tail | ✅ done (M8.3m–p) |
| 2 | Noise reduction on the GPU | ✅ done, M8.3q — an NR drag ~207 → ~142 ms, 1.45–1.46× |
| 3 | Port **lens correction** | ✅ **done, M8.9** — ⚠ its *value* was re-priced, not the work; see below |
| 4 | Mask coverage + the blend | ✅ done, M8.11 |
| 5 | **Spot removal** | ⬜ not started |

**⚠⚠ ITEM 3 IS DONE, AND THIS LIST SAID OTHERWISE FOR A DAY.** `crates/gpu-pipeline/src/lens_correction.rs`
is 722 lines, wired at `accelerate.rs:320`, gated by `lens_correction_is_accelerated_only_when_a_profile_is_in_hand`
— M8.9 shipped it and it is confirmed by use. **The 🟡 this row used to carry meant its *value* was
re-priced, and was read as unfinished work.** A status column must say whether the code exists, and
nothing else.

**⚠ What was re-priced, 2026-09-15 by M8.8, which took the other route first.** Giving the stage an
`input_rect_for`/`apply_region` pair took a cold 1:1 entry's prefix from **1689–1869 → 218–225 ms**
without a line of shader; lens correction is then 118 ms of that 221. **So the port's value had
already moved to EXPORT** — whole-buffer, sequential, still paying 650–696 ms a photo — and M8.10
then made those photos run four-wide, against a wall clock ~3× shorter than when it was proposed.
Measured after the fact: the port saves ~550 ms of a ~11.4 s export, **under 5%**.

**✅ Verified against the tree, 2026-09-16: 18 of 23 pipeline stages have GPU twins.** The three
without are `geometry`, `perspective` and `spot_removal` — and the first two were answered by M8.8's
region scoping rather than a shader, deliberately. **So item 5 is the only genuine port left.**

### Open items

- **The camera rendering does not reach an export** (`ProfileRendering::default()`) — `docs/BACKLOG.md`.
- **A GPU-side mask coverage cache** — ✅ **measured 2026-09-16; the ceiling missed 4–8×.** ⚠⚠ But the
  cost is **not transfer**: a serial u8→u32 repack (10.6–13.8 ms per encode) and a 4×-inflated download
  wire. **Fix those two first — they are mechanical and open no design question.** Re-price the cache after.
- **A GPU-resident base for `GpuBrushRaster`** — still open, still unmeasured: a global drag over a masked
  photo gains **1.00×, by construction**.
- **The resident GPU buffer across the prefix→tail seam** (`docs/BACKLOG.md`, head of `## Performance`) — **measured, not estimated**: ~26 ms of an 88.9 ms dehaze-drag frame is a zero-work round trip paid **twice**. ⚠ Its own entry said not to start it **before the prefix is mostly ported** — with 18 of 23 stages ported that condition now holds, so this is live rather than blocked.
- The prefix-cache eviction fix's **picture** half.
- The **7 temp-dir test-isolation sites** — ⚠⚠ **never run two gates concurrently.**
- Move `PrefixCache`'s tests into `prefix_cache.rs`.
- ⛔ ~~A per-gating `MIN_GPU_BOX_PX`~~ — **DELETED 2026-09-16: measured, and its premise is backwards.**
  Both configurations cross at the same rows; see `docs/BACKLOG.md`. The floor itself may want raising
  2–6×, but only after an idle re-run of `crates/app/examples/box_floor_sweep.rs`.
- **`MAX_TILE_DABS`** — swept; the cliff is at **2560, not 3584**. ⚠ Blocked on a **portability**
  question (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) that no M2 run settles.
- Clarity's **coalescing** on the horizontal running-sum pass; **M8.1**'s leftovers.
- ⚠ **M7.3b is DEFERRED, not done.**
- ⚠ Two oversized files grew again: `crates/app/src/batch_export.rs` (**2,298**, largest in the crate) and `crates/lens-profile/src/lib.rs` (949).

### The open question M8.11 did not settle

M8.3j/k/l's inner-chain machinery — `accelerate_inner`, `sub_stage_names`, the coverage rasteriser
hook and the shared upload — is now reachable **only on the decline path**, i.e. a gating brush with
a cold cache. That is a genuine configuration and the machinery was kept, pinned with the
gating-brush wedge; whether it earns its keep once a GPU-resident brush raster exists is worth
asking then rather than now.

### Standing lessons (the expensive ones — full stories in `docs/JOURNAL.md`)

- **⚠⚠ A guard that enumerates a list stops guarding the moment the list grows.** The
  double-application test named three of five fields and was therefore blind to a whole milestone —
  the real sabotage left it at `1 passed` while six goldens failed. Read this before adding anything
  to `adjustment_stages`.
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
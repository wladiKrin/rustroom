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

### State (2026-09-18)

**2557 tests, 74 goldens.** The repo is **`main` alone** — clean tree. ⚠ `main` is **ahead of
`origin/main`** — **not pushed; the user pushes.**

✅ **M8.14 — a GPU buffer resident across the prefix seams. Merged and CONFIRMED BY USE.** Consecutive GPU prefix steps, and the last one and the tail's first chain, hand the device
buffer over instead of downloading and re-uploading the whole frame. `DSCF6509.RAF` (312.5 MB
plane), quiet machine, warm frames:

| warm frames | before | after |
|---|---|---|
| step→step (the dehaze step) | 312.7–313.0 ms | **294.2–297.9** → saved **14.8–18.8** |
| step→tail (direct A/B) | — | saved **28.5–30.6** |
| **both seams** | | **43.3–49.4 ms ≈ 5.6–6.3% of a ~790 ms frame** |

⚠ NR is the control and did not move (243.4–245.5 vs 244.7): it is the first GPU step, so it has
nothing to take. The pre-build prediction of 4.5–6% held, at the top of its range.

⚠⚠ **An export and cold-whole-frame win, NOT a drag win.** The region-scoped path runs no GPU prefix
step at all, so it has no seam. Do not re-read this expecting a drag improvement.

⚠⚠ **The two downloads are the larger half and were DECLINED** — they need `PrefixCache` to hold a
`GpuImage` (11–13%, user's call 2026-09-18). `GpuContext` now has **two** `SharedPlane` slots
(`Mask`, `Prefix`) and merging them would undo a sabotage-pinned guard.

✅ **M8.13 — a GPU-resident base for `GpuBrushRaster`. Merged and CONFIRMED BY USE.**
A `GpuChain` before a mask deposits its output buffer; the gate takes it instead of memcpying the
same plane back up. `DSCF6509.RAF`, REALISTIC edits, warm frames 2–5, against the same build with
the take forced to miss:

| warm frames 2–5 | take forced to miss | taking |
|---|---|---|
| `stage=` (host memcpy) | 2.23–2.86 ms | **0.00** |
| `gpu=` | 10.02–19.09 ms | **5.14–6.77 ms** |
| whole frame | 88.1–109.7 ms | **78.2–80.7 ms** → **1.13–1.36×** |

⚠⚠ **About twice the plan's prediction, for a reason the plan did not name:** deleting the staging
buffer also deletes the `copy_buffer_to_buffer` that pushed **78.1 MB** across the device. **Do not
requote the ~2.2 ms memcpy as the win** — it is one of two halves.

⚠ It helps a **global drag over a masked photograph with a real edit set**, and nothing else.
Measured: no globals and a light set (exposure + contrast) build **no chain**, so there is nothing
to offer.

⚠⚠ **`CoverageRasterizer::begin_mask` is now `end_mask`, and that was forced by a defect.** The old
hook cleared the shared slot at the *top* of a mask's apply — after the preceding chain had already
deposited — so the hand-off could never have fired: green, documented and dead. It now runs from an
RAII guard at the end, covering every early return and a panic, which is also strictly tighter.

### ▶ Next action — a choice, and the machine is finally quiet

✅ **BOTH CONTENDED SWEEPS WERE RE-RUN CLEAN 2026-09-17** (load 2.11), and one overturned a
recorded finding:

- **`MIN_GPU_BOX_PX`** — the quiet run **confirms** the contended one: both configurations cross at
  **3,130,000 px** and at the shipped 512 Ki floor the GPU is ~2× slower in both. ⛔ **DECIDED: left
  at 512 Ki, the user's call, on use** — *"it felt fine while using."* ⚠⚠ **Do not re-propose this
  from the measurement**; it costs ~2 ms on a coverage-cache MISS for boxes between 512 Ki and
  ~1.1 M px, which is the magnitude nobody noticed. `docs/BACKLOG.md` carries the numbers.
- **⚠⚠ The post-NR checkpoint no longer saves ~0 on the scoped path — M8.12 changed that.** It was
  ~0 because an NR drag and a dehaze drag were *both* ~118 ms, doing whole-frame work. Now a 2-step
  scoped plan is **30.5–35.6 ms** against a 1-step's **15.3–15.9**, so it **saves ~15–20 ms**.
  Clean whole-frame bands: rung 3200 **95.1 ms** CPU-tail, **57.8** GPU-tail.

**The candidates:**

1. ✅ ~~**A GPU-resident base for `GpuBrushRaster`**~~ — **DONE 2026-09-18 (M8.13), 1.13–1.36× on a
   masked global drag. Merged and confirmed by use.**
2. ⛔ ~~**Item 5, spot removal**~~ — **CLOSED 2026-09-18, measured and not worth doing.** 0.05–0.4 ms
   for realistic use on a cached prefix stage, against a ~1.6 ms GPU floor.
3. **The u8 coverage wire** — priced: **0.56–0.70 ms against an actual 3.55–6.35** at rung 3200,
   **3.4 against 15.3** at export. ⚠⚠ Needs a shader restructure: the shader writes one `u32` per
   sample because packing four into a word would race. **A considered trade, not an oversight.**
4. ✅ ~~**`crates/app/src/batch_export.rs` is 2,298 lines**~~ — **SPLIT 2026-09-18** into a directory
   module of six files, largest 1,127. A pure move, 2547 tests unchanged.

⚠ The **GPU-side coverage cache** is re-priced **downward**: transfers were 27–58% of the rasterise
a hit avoids and are now ~10–23%, because the repack fix landed first.

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
| 5 | **Spot removal** | ⛔ **CLOSED 2026-09-18 — measured, not worth doing.** See below |

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

**✅ Re-verified against the tree 2026-09-17: 18 of 21 pipeline stages have GPU twins**, and only
**three** lack one: `geometry`, `perspective` and `spot_removal`.

**⛔ ALL THREE ARE NOW DELIBERATE RATHER THAN PENDING, so the GPU port is effectively COMPLETE.**
`geometry` and `perspective` were answered by M8.8's region scoping rather than a shader. And
**spot removal was measured on 2026-09-18 and closed without being built**: realistic retouching is
**0.05–0.4 ms** on a **cached prefix stage**, against `GpuBrushRaster`'s measured **~1.6 ms floor**
(1.28 of it a fixed download), so a port would very likely make it *slower*. ⚠ **Do not re-propose
it from the agreed order alone** — "last unported" is not "worth doing". `docs/BACKLOG.md` carries
the table and the two caveats that would reopen it.

⚠⚠ **This said "18 of 23" until 2026-09-17, which invited the wrong subtraction** — 23 − 18 = 5, and
only three are unported. `crates/pipeline/src/stages/` holds **23 files**, of which **21 implement
`PipelineStage`**: `hue_table` and `mask_coverage` implement none and are helpers folded into `hsl`
and `mask`. ⚠ Two twins are also renamed rather than missing — `color_grading` → `grading.rs`,
`sharpening` → `sharpen.rs` — so a filename comparison undercounts. **Count by
`impl PipelineStage for`, not by file.**

### Open items

- ✅ ~~**The camera rendering does not reach an export**~~ — **DONE 2026-09-18, confirmed by use.**
  It was three defects of one shape: the export lost the curve, the baseline **and the look**, a
  thumbnail lost the look, and an already-rendered source was rendered **twice** at export
  (119.32 levels). ⚠⚠ The root cause is open: the four whole-pipeline builders default
  `ProfileRendering` **silently**, so a new call site loses it with no warning — `docs/BACKLOG.md`.
- **A GPU-side mask coverage cache** — ✅ **measured 2026-09-16; the ceiling missed 4–8×.** ⚠⚠ But the
  cost is **not transfer**: a serial u8→u32 repack (10.6–13.8 ms per encode) and a 4×-inflated download
  wire. **Fix those two first — they are mechanical and open no design question.** Re-price the cache after.
- ✅ ~~**A GPU-resident base for `GpuBrushRaster`**~~ — **DONE, M8.13 (2026-09-18): 1.13–1.36× on a
  masked global drag, measured against a forced miss. Merged and confirmed by use.**
- ✅ ~~**The resident GPU buffer across the prefix→tail seam**~~ — **DONE, M8.14 (2026-09-18):
  43.3–49.4 ms, 5.6–6.3% of a whole-frame render. Merged and confirmed by use.**
  ⚠⚠ Its *download* half remains open and is the larger one — declined at 11–13%, see `docs/BACKLOG.md`.
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
- ⚠ `crates/lens-profile/src/lib.rs` is **949** lines. (`batch_export.rs` was split 2026-09-18;
  its `worker.rs` is **1,127**, deliberately — ~190 of code and ~940 of its own tests, and the
  module header says so.)

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
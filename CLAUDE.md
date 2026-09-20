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
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). ⚠ main.rs is 681 lines after W0 (2026-09-01); RustroomApp's methods live in session_flow/sources/smart_sources/library_state/catalog_ops/copies/snapshots/transfer/undo/export_flow/settings_apply/interaction/decode. lib=spot_tool/spot_source/visualize/grade_view/history/action_log/edit_history/selection/crop_tool/histogram/render_engine/thumb_loader/thumb_cache/thumb_window/metadata_scanner/date_tree/rule/rule_form/library_filter{RatingSet,FlagSet,LabelSet}/decode_worker/buffer_cache/view_region/full_res/format_filter/presets/export_presets/safe_name/settings/edit_clipboard/sidecar/sync/batch_export/photo_id/entries/file_rename; bin-only modules=theme/segmented/list_row/modals/name_edit/develop{mod,panel,canvas}/library/filmstrip/thumb/shell/collections_panel (egui lives here, not in the lib)
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

### State (2026-09-20)

**TESTCOUNT tests** on `shell-restyle` (goldens unmoved since 2026-09-18).

⚠ **One branch, `shell-restyle`, ahead of `main`: the top bar and the Library in one language** —
an accent app-mode pill, an icon pill for Grid / Compare / Survey, format chips, a thumbnail-size
slider, the Develop toolbar regrouped with an accent Export; the Library's left panel on the
section chrome with painted rows, grid cells with rounded thumbnails and a marks row under the
picture. Gated green on its tip, **GUI launched for a look, not yet confirmed by use.** Spec
`2026-09-20-shell-restyle-design.md`; `grep -n 'one language' docs/JOURNAL.md`.

✅ The five panel reworks (Develop, Masks, Geometry, Retouch, Presets) and the mask off switch are
merged and confirmed by use; specs under `docs/superpowers/specs/2026-09-1[89]-*` and
`2026-09-20-*`.

⚠⚠ **This block is the LIVE STATE, not a changelog.** Every milestone below is one line with its
outcome; the measurements, the retractions and the things that went wrong live in
`docs/JOURNAL.md`. This section reached a full screen of per-milestone tables twice — that is the
1.2 MB failure in miniature. **Overwrite it; do not append to it.**

**Landed 2026-09-18, all merged and confirmed by use:**

| | outcome |
|---|---|
| **M8.13** GPU-resident base for `GpuBrushRaster` | **1.13–1.36×** on a global drag over a masked photo |
| **M8.14** resident buffer across the prefix seams | **43.3–49.4 ms**, 5.6–6.3% of a whole-frame render — **export/cold only** |
| `batch_export.rs` split | 2,335 lines → six files, a pure move |
| the rendering seam reaches export **and** thumbnails | three defects of one shape; an already-rendered source was rendered **twice** (119.32 levels) |
| `ProfileRendering` is a **required** argument | 122 call sites, behaviour-preserving, 74 goldens unmoved |
| **the GPU tail is ON by default** | user's call; required removing a field-level `#[serde(default)]` |

⚠⚠ **The GPU default flip had a one-word trap**: the field's own doc said `#[serde(default)]` was
safe *because* the intended default matched `bool`'s. Flipping it to `true` would have loaded
**`false`** for every existing settings file. Pinned by a test that fails when the attribute returns.

### ▶ Next action — the GPU port is finished; what is left is not GPU work

⚠⚠ **Everything on the agreed GPU order is done, declined with numbers, or blocked on a considered
trade.** Do not go looking for the next port; there isn't one.

**Live candidates, none urgent:**

1. **The u8 coverage wire** — priced: **0.56–0.70 ms against an actual 3.55–6.35** at rung 3200,
   **3.4 against 15.3** at export. ⚠⚠ Blocked on a shader restructure — the shader writes one `u32`
   per sample because packing four into a word would race. **A considered trade, not an oversight.**
2. **Rename `ProfileRendering` → `TailRendering`** — `look` is not a camera-profile field. Now a
   ~120-site diff. ⚠ Its own change, never folded into another.
3. **`crates/lens-profile/src/lib.rs` is 949 lines**; `batch_export/worker.rs` is 1,127 (accepted —
   ~190 of code, the rest its own tests).
4. Move `PrefixCache`'s tests into `prefix_cache.rs`.

⛔ **Declined with numbers, and the numbers are in `docs/BACKLOG.md` — do not re-propose from the
list alone:** the prefix seams' *download* half (11–13%, needs `PrefixCache` to hold a `GpuImage`),
spot removal on the GPU (0.05–0.4 ms against a ~1.6 ms floor), `MIN_GPU_BOX_PX` (user's call, on
use), the GPU-side coverage cache (re-priced downward).

⚠ **`MAX_TILE_DABS`** — swept; the cliff is at **2560, not 3584**. Blocked on a **portability**
question (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) that no M2 run
settles.

### The agreed systematic order (GPU port)

⚠ **Structural, not popularity-driven.** The catalogue's 40 sidecars describe those folders and
**not** what a photographer does; three milestones leaned on that scan and it must not be cited as
a prioritisation argument again.

⚠⚠ **But "the user has not been using Rustroom yet" is NO LONGER TRUE, and that line sat here
unchallenged for days.** On 2026-09-18 alone, use found the export/preview divergence, the
already-rendered double render **and** the missing thumbnail look — none of which any test caught.
**Real use is now the best signal this project has**; the sidecar scan is still not.

| # | Item | State |
|---|---|---|
| 1 | Finish the per-pixel tail | ✅ done (M8.3m–p) |
| 2 | Noise reduction on the GPU | ✅ done, M8.3q — an NR drag ~207 → ~142 ms, 1.45–1.46× |
| 3 | Port **lens correction** | ✅ **done, M8.9** — ⚠ its *value* was re-priced, not the work; see below |
| 4 | Mask coverage + the blend | ✅ done, M8.11 |
| 5 | **Spot removal** | ⛔ **CLOSED 2026-09-18 — measured, not worth doing.** See below |

✅ **THE ORDER IS FULLY RESOLVED. There is no next item on it.** Rows 1–4 are built and confirmed;
row 5 is closed on a measurement. Anything further is in `docs/BACKLOG.md` with numbers attached.

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

⚠⚠ **Live items only.** A finished item is deleted from here, not struck through — its record is in
`docs/JOURNAL.md` and its numbers in `docs/BACKLOG.md`. Four ✅ entries had stacked up here by
2026-09-18, which is the 1.2 MB failure starting again.

- **A GPU-side mask coverage cache** — ✅ measured 2026-09-16; the ceiling missed 4–8×. ⚠⚠ The cost
  is **not transfer**: a serial u8→u32 repack and a 4×-inflated download wire. **Fix those two
  first** — mechanical, no design question. Re-price after.
- **The u8 coverage wire** — 0.56–0.70 ms against an actual 3.55–6.35 at rung 3200. ⚠⚠ Blocked on a
  shader restructure; **a considered trade, not an oversight.**
- **Rename `ProfileRendering` → `TailRendering`** — `look` is not a camera-profile field. A ~120-site
  diff since 2026-09-18. ⚠ Its own change.
- **`MAX_TILE_DABS`** — the cliff is at **2560, not 3584**. ⚠ Blocked on a **portability** question
  (1024 is the largest capacity keeping 3 workgroups on a 16 KB device) no M2 run settles.
- The prefix-cache eviction fix's **picture** half.
- Clarity's **coalescing** on the horizontal running-sum pass; **M8.1**'s leftovers.
- Move `PrefixCache`'s tests into `prefix_cache.rs`.
- ⚠ **M7.3b is DEFERRED, not done.**
- ⚠ `crates/lens-profile/src/lib.rs` is **949** lines. (`batch_export/worker.rs` is 1,127
  deliberately — ~190 of code, the rest its own tests, and its module header says so.)
- The **7 temp-dir test-isolation sites** — ⚠⚠ **never run two gates concurrently.**

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
# AGENTS.md — Rustroom

Guidance for anyone changing Rustroom's code — a person or a coding assistant. `CLAUDE.md` imports this file, so Claude Code
reads it every session; other assistants read `AGENTS.md` directly. The long tour is [`ARCHITECTURE.md`](ARCHITECTURE.md);
building, checking and sending a change is [`CONTRIBUTING.md`](CONTRIBUTING.md); the story behind every rule here is
`docs/JOURNAL.md` — grep it, never read it whole (`grep -n '^### ' docs/JOURNAL.md` is the index). Things declined with numbers
are in `docs/BACKLOG.md`: do not re-propose them from a list.

⚠ **Keep this file small: overwrite a rule in place when it changes, never append a superseded
version** — the history belongs in the journal. Target: under ~400 lines.

## What Rustroom is

A non-destructive RAW photo editor on Lightroom Classic's model — a Library (catalog, culling,
keywords, collections, map, people, publish services) and Develop (the edit pipeline, masks,
retouching, merges) — for macOS, Linux and Windows from one Rust codebase, GPL-3.0-or-later.
**Reuse proven libraries** rather than reimplementing decoding or colour science.

| Concern | Choice |
|---|---|
| RAW decode | libraw via `rsraw` (`rsraw-sys` vendored byte for byte + ONE MSVC patch, `VENDORED.md`); floating-point DNGs by our own reader (`raw_decode::float_dng`) |
| Other formats | `image` (JPEG/PNG/TIFF), libheif (HEIC/HEIF) |
| Colour | LittleCMS (`lcms2`), DCP camera profiles, learned camera looks |
| Lens corrections | lensfun's database via the pure-Rust `lensfun` crate |
| Parallelism / GPU | `rayon`; `wgpu` compute (one API over Metal, Vulkan, DX12) |
| GUI | `egui` / `eframe` |
| AI models | ONNX through `tract`, run locally, downloaded on first use (`ai_mask::registry`) |
| Persistence | edits in a JSON sidecar beside each photo; the catalog in SQLite (`rusqlite`) |
| Tethering | libgphoto2 (macOS and Linux; `cfg(not(windows))`) |

## Core architecture

1. **Non-destructive.** Originals are never modified. An edit is a parameterised entry in an ordered
   op-list (`Edits`); rendering replays it. History, virtual copies, snapshots and presets follow.
2. **Scene-referred, linear, float32.** Linear ProPhoto working space; the display and output
   transforms come only at the end.
3. **Staged pipeline behind `PipelineStage`.** Each stage is an independent, unit-testable transform
   with its parameters baked in at construction. The CPU stage is the reference; a GPU twin replaces
   it without changing callers. Do not leak CPU-specific types into the pipeline interface.
4. **Previews for interactivity.** Edit a downscaled preview of the visible region; render full size
   only for export and 1:1.

```
decode → linearise → white balance → demosaic → input profile → linear f32 working space →
PREFIX: AI denoise → removals → lens → negative → noise reduction → dehaze → spots → red eye →
        geometry (orient·keystone·straighten·crop, ONE resample) → defringe → masked NR/dehaze → lens blur
TAIL:   scene offsets → white balance → exposure → tone (contrast/regions/curve) → clarity/texture →
        colour (saturation/HSL/grading) → masks → effects (vignette, grain) → sharpen →
        camera rendering / look → output transform → encode
```

**Geometry ends the capture corrections and precedes every adjustment**, so the tail works on the final cropped pixels (which
also makes the vignette post-crop for free). It is ONE resample, after lens, NR, dehaze and spots, so a straighten or crop drag
re-runs only that resample — a measured **speed** trade, not an image-quality win. Spots, eyes and brush strokes are **stored in
decode space** and mapped by `pipeline::anchors::resolve` at each renderer's entry. **NR early, sharpening late:** denoising the
source beats denoising grain that exposure and curves have amplified, and sharpening must not be stretched by a later stage.

## Where new code goes

**Decide placement before writing — never dump into `main.rs` and leave the split for later.** Every
oversized file here grew one reasonable-seeming addition at a time. Keep crates small and
single-purpose. `ls crates/` is the inventory;
`ARCHITECTURE.md` describes each crate.

- **The crate boundary is a hard rule.** `pipeline`, `color`, `export`, `core-image`, `edit-model`,
  `catalog`, the decoders and the profile crates have **zero GUI dependency** and are driven headless
  by the CLI and the tests. `crates/app/src/lib.rs` and its modules have **no egui** — that is where
  unit-testable logic lives. egui lives only in the app's **bin** modules (declared in `main.rs`).
  Count them, never requote: `grep -c '^pub mod [a-z_0-9]*;' crates/app/src/lib.rs`.

| Adding | Goes in |
| --- | --- |
| A pipeline stage | `crates/pipeline/src/stages/<name>.rs`, one per file (+ a GPU twin if it is a tail stage) |
| An edit parameter | `edit-model` — **and** its `EditGroups` group, `merge_from`'s destructure, the three exhaustive fixtures |
| A reusable image primitive | `core-image`, or `pipeline/src/spatial.rs` for kernels |
| Logic behind a gesture | a **lib** module in `crates/app/src` taking plain types (the `crop_tool` / `mask_tool` / `panel_order` pattern) |
| The gesture itself | the bin module under `develop/` or beside `library.rs` |
| A dialog | `crates/app/src/modals/<flow>.rs`, grouped by flow |
| A pure function a test wants | anywhere in a **lib** — never a helper inside a bin module no test can import |

- **Two questions catch most misplacements:** *can this be tested without opening a window?* (if yes
  and it is in a bin module, it is misplaced) and *would a second caller want this?* (then it is a lib
  module, not a private helper).
- **The bin-crate dead-code trap:** in a binary crate a `pub` item with no production caller is dead
  code and `clippy -D warnings` rejects it — an item lands in the same commit as its first caller.
- **Append, never insert:** new `Edits` fields at the end of the struct and of every exhaustive fixture;
  the same for `Command`, the menus, `EDIT_GROUP_NAMES`, the `DialogSize` array, `Mask`'s variants and the goldens.
- **A catalog migration is one change with the next version number**, never two branches each adding
  one: two `if from < N` migrations are data corruption on a real catalog, not a merge conflict.
- **Headless first:** a canvas feature is model, stage and tests with no GUI, then the canvas UI — so
  a canvas bug can never be confused with a model bug.
- **Never let an agent decide a question you would have wanted asked** — a background agent with an
  open design choice picks one silently and it ships. Settle design questions first; scope an agent
  that needs evidence to measure and report. Nor let one sign off a judgement about a photograph:
  produce the evidence (a probe, a before/after) and leave the verdict to a person.
- ⚠ **If a file you are editing already exceeds ~600 lines, say so in your report.**

## The gate

`cargo fmt --all -- --check`, `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets
-- -D warnings`, `cargo test --workspace` — all green, on the tip you ship (re-gate after the last
commit; read each exit code from its own command). The whole gate is about 80 seconds.

- ⚠⚠ **One cargo invocation mode per session.** A warm `cargo test --workspace` takes a second;
  alternating it with `cargo test -p <crate>` rebuilds on every switch (43–531 s), because `-p`
  resolves features for one crate's subgraph. **Filter by test name, never by package:**
  `cargo test --workspace immich_source`. Clippy gets its own target directory for the same reason.
- Goldens: `cargo test --workspace --test golden` — never abandon them; they made the GPU port safe.
- Cargo never garbage-collects `target/`; if builds slow down, count its files (`find | head`), not `du`.
  After moving the checkout, `touch crates/test-dir/src/lib.rs`: its sweeps bake in the old path.
- On Linux, `gpu_pipeline`'s suite can stall inside a full run and `sources::reindex_tests` can fail
  to exit (pre-existing) — run the remaining test binaries directly when it happens.

## Conventions

- **No two workspace binaries whose names differ only by case** — macOS's disk ignores case; the CLI is
  `rustroom-cli` (package `rustroom`), the app `Rustroom`. `no_two_binaries_share_a_name_but_for_case`.
- **Anything a user can see follows `docs/DESIGN.md`:** four text sizes, nothing below 11 pt, one
  accent, one form per control, hover text instead of on-screen hints (one sentence, no full stop —
  `every_hover_is_one_short_sentence`); its §12 is the checklist for a new element. A literal font
  size or `Color32` outside `theme.rs` is a defect.
- **Colour correctness is first-class** — validate against references early.
- **Avoid platform assumptions** (paths, dialogs, GPU backend, words like Finder/⌘/keychain); CI tests Linux and
  builds Windows; libraries per OS in `CONTRIBUTING.md`. A platform branch is a lib no-op elsewhere (`console`).
- **Commit messages** describe the change; one feature per branch.

## The GPU

Every **tail** stage has a GPU twin. The prefix stages without one — spot removal, red eye, the negative, AI denoise's blend and
the removals' paste — were measured and declined, or wait. Also declined with numbers (`docs/BACKLOG.md`): the prefix seams'
download half, spot removal on the GPU, `MIN_GPU_BOX_PX`, the GPU-side coverage cache, one device for the window and the pipeline.
Count twins by `impl PipelineStage for` and `impl (crate::)?GpuStage for` (exposure writes the `crate::` form). Switches:
`RUSTROOM_DEBUG_GPU_TIME=1`, `RUSTROOM_NO_FUSION=1`, `RUSTROOM_DEBUG_RENDER` (measure a drag through it, never a probe row).
⚠ A geometry twin was once declined for pricing the compute and missing the transfers — price both.
⚠⚠ **A NaN test in WGSL is on the BITS** (`quantize.rs`'s `is_nan`), never `v != v`: Metal compiles with fast math, which
may fold it away — a guard green on Vulkan let a NaN pixel through on the Mac (2026-11-18). A GPU change is tested on Metal too.

## Warnings that outlive their milestone

Only what stays true after a milestone closes belongs here; each story is in `docs/JOURNAL.md`.

- ⚠⚠ **A macOS-only module (`#[cfg(target_os = "macos")] mod …` in `main.rs` — `native_menu`,
  `display_colorspace`) is gated at EVERY reference, its `use` lines included** — one ungated import in
  `bootstrap.rs` kept the app from compiling off macOS for four weeks, invisible to a gate run only on macOS.
  Development now happens on Linux and Windows too; a change to such a module is built on each before it merges.
- ⚠⚠ The preview converts into a `color::Display` (a fixed space or a monitor's interned profile), chosen in ONE place,
  `Settings::effective_display` (the OS matching colour → sRGB); the GPU converts through `Display::gpu_table()` into the texture's own plane, the sRGB plane staying the histogram's; ⚠ a table's grid is spaced by a SQUARE ROOT, CPU and shader alike (evenly, a gamma-2.2 profile was 4.66 steps off).
- ⚠ Big data lives in `Catalog::library_dir` (`%LOCALAPPDATA%` on Windows, never Roaming). A package is built by its
  `packaging/<os>/` script (the release workflow only calls it); icons come from `rustroom.svg` via `render.py`; ⚠⚠ the installer's `AppId` never changes.
  ⚠⚠ The AppImage NEVER bundles the graphics stack (Vulkan, GL, Mesa, X, Wayland) — `packaging/linux/build.sh` stops on
  any library in neither its bundle nor AppImage's `excludelist`; its `appimage` feature compiles libheif in, so
  `--all-features` on Linux needs `cmake` and `libde265-dev` (CI's clippy has them).
  ⚠ The new-version check (piece 4) is the ONE request Rustroom makes unasked — only through `rustroom_app::update_check`,
  which uses nothing from the reply but a version; a build with `RUSTROOM_NO_UPDATE_CHECK` set compiles it off.
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
  2026-09-29 — start from `docs/BACKLOG.md`'s reasons table.
- **Rules kept from finished milestones** (one line each; `grep -n '<id>' docs/JOURNAL.md` for the story):
  - M9.7 — history lives in the CATALOG (`photo_history`), saved in `persist_current_session`; `on_exit` NEVER runs (close ends in `process::exit`).
  - Sliders — `Edited::slider` takes a REQUIRED default; `every_slider_resets_to_its_fields_default` sweeps by role.
  - M9.8 / M9.9 — every keyword change and its undo rewrite the XMP; `scan_and_relink` is shared by open and Reindex (strict name + size + mtime).
  - M9.10 — DNG export writes `ColorMatrix1` at D65 (libraw ignores the illuminant tag). M9.12 — the import survey reads only the EXIF block (`image_decode::quick_look`).
  - M9.13 — MobileSAM: use `low_res_masks`; the lens warp is done ONCE per map (`post_lens`).
  - M9.14 — untouched = `initial_edits(None)` OR `Edits::default()`, never `is_neutral`; the stand-in rule reads `view_region::needed_long`, never the rung.
  - M10.9c — what a Lightroom preset sets that Rustroom lacks is NAMED to the user, never dropped silently. M10.10 — export sizing applies to the CROPPED picture.
  - M10.12c — `pipeline::anchors::resolve` runs once at EACH renderer's entry (engine, `export_one`, CLI): a new renderer calls it too; the canvas works on `Session::anchored_view` and stores through `anchor_spot` / `anchor_eye` / `stroke_to_decode`.
  - M10.13d — a learned film picked from the menu REPLACES the photo's own (`camera_look::applied`); `camera/<name>` is the one look name with a separator (`looks::is_learned`).
  - M10.14f — a GPU twin's test asserts the GPU RAN (`apply_keeping_input`): `GpuChain::apply` falls back silently; inside a test a fallback PANICS (`strict-fallback`) unless wrapped in `fallback::allow`.
  - M10.14h — a GPU scene-offsets stage and its reading masks are ALL OR NOTHING (`accelerate::reference_plan`).
  - M10.16a — the depth recipe is `depth:v2`; bump `DEPTH_QUERY` on any change; `tract` needs the export's symbols BOUND per size and its shape annotations ignored. M10.16c2 — the dust search is pumped by Develop's body (`dust_flow`), never the panel.
  - M10.17 — a merge result never lands in the Immich cache (`hdr_merge::out_folder`); HDR and panorama share one queue (`merge_queue`); a gesture over a dialog never reaches the Library behind it.
  - M10.18a — one thread owns the tethered camera; the card is only READ; on macOS a session holds `ptpcamerad` off (`tether::Hold`).
  - M10.18b — locations live in the CATALOG only (`photo_location`: hand > file > server), never a file or a sidecar.
  - M10.18c — people's names live in Rustroom ONLY, never Immich or a file; every new `Library` takes `library_state::fresh_rev()`; the Immich people and album DTOs are constructed, not captured — capture a real reply before trusting them.
  - M10.18d — publish state is keyed on `photos.id` and its fingerprint is pinned (`publish_state`). M10.18e — a printed page is the EXPORT's render; the PDF is hand-written (`print::pdf`).
  - M10.19a — a HEIC's colour profile is the libheif HANDLE's, never the decoded image's. M10.19c — folder marks are keyed by PATH.
  - M10.19d — in Retouch's Find task only `find_overlay` draws; the dust view's one writer is `pump_dust`.

## Standing lessons

The expensive ones — each cost a real bug; full stories in `docs/JOURNAL.md`.

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
  nothing, so I reported no `LICENSE` — it had been committed since `f40290b`. **`ls` here is aliased to
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

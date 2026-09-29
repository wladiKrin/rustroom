# Backlog

Deferred work, with **why** it was deferred. Nothing here is a bug or a blocker; each item was
a deliberate decision to stop somewhere.

The rule for this file: an entry records the *reasoning*, not just the wish. An item deferred for
a **design** reason and one deferred for **effort** are different things, and mixing them up is how
a hard question gets picked up as though it were a small job.

Milestone-level work lives in the roadmap
(`~/Documents/obsidian/private/rustroom/03-plan-of-action.md`); implementation detail
lives in `CLAUDE.md`. This file is for things consciously left undone.

---

---

## ⚠⚠ 2026-09-29 — everything still open is now PHASE 10, and this file maps to it

A sweep checked every entry here against the code on 2026-09-29 (`grep -n 'The re-plan' docs/JOURNAL.md`).
**Every open entry is now a Phase 10 milestone** in `03-plan-of-action.md` — the author kept them all
and added back the ones earlier decisions had put out of scope. Release engineering is **Phase 11**
(M11.1–M11.5, the former M10.1–M10.5), last. An entry below still carries its *reasoning*; its
**status and order** live in the plan. ⚠ Entries the sweep found already shipped are struck with a ✅
and a pointer, not deleted.

| Milestone | Entries in this file |
| --- | --- |
| **M10.6** your to-dos + design leftovers | (from the vault's `to dos.md` and `DESIGN.md`) |
| **M10.7** code health | *Exports are not bit-reproducible* (a); plus `CLAUDE.md`'s rename, temp-dir, file-size and `PrefixCache` items |
| **M10.8** Library & workflow | P-26's batch rename (d); *Library: … vertical filmstrip* (b) |
| **M10.9** presets & shortcuts | *Presets: no import/export…* (a); *No user-defined keyboard shortcuts* (b) |
| **M10.10** export | P-26's image watermark (a) and export sizing (b) |
| **M10.11** Develop additions | *Film negative inversion* (a); P-26's Grain (b) |
| **M10.12** the mask model | *Composites are one level deep* (a); masks' scene sliders as offsets (b, `CLAUDE.md`); *Anchor strokes and spots in sensor space* + *Brush strokes shift…* (c, ⏸) |
| **M10.13** colour & camera profiles | *C2/C3 have no independent oracle* (a); *`CameraCalibration`…* + `ProfileLookTable` (b); *Per-photo camera profile* (c); P-22 (d) |
| **M10.14** performance & verification | *What is left in `HslStage`* (a); P-21 (e); *incremental splatting*, *Two Oklab round trips* (f, ⏸); *the u8 wire*, `MAX_TILE_DABS` (g, ⏸) |
| **M10.15** Immich | the keychain, cached-original eviction, Immich's rating (a–c); *Two-way sync* (d, re-opened) |
| **M10.16** AI tools | depth masks (a); P-23 (b); P-26's content-aware remove (c) |
| **M10.17** merge | P-24 |
| **M10.18** new modules | P-25 (brought in by the author) |
| **M11.2** Windows & Linux | *The in-window menu bar has never been run*; *Multi-monitor on Linux and Windows*; P-28 |
| **M11.3** / **M11.5** | *Contributor-facing documentation* / *A plugin system* |

---

## ⚠⚠ 2026-09-01 — most of this file is now scheduled

The roadmap was re-planned on 2026-09-01 (see `03-plan-of-action.md` and `PARALLEL-WORK.md`). Items
below that were "deferred" are now **milestones with an order**, and this table is the map. An entry
still carries the reasoning that matters — read the entry, then build it under its milestone number.

| Backlog entry | Now |
| --- | --- |
| Creative 3-D LUTs | ✅ **M7.5, shipped 2026-09-04** and confirmed by use (after the handover fix). ⚠⚠ Was specced as W5 track A. M5.4b supplied the *seam*, not the answer: the blocker below — a `.cube` does not declare its input space — was still open, and W5-0 settled it. See `specs/2026-09-04-creative-luts-design.md`. |
| `ProfileToneCurve` / `ProfileLookTable` | **answered by M5.4b** — the same display-rendering question |
| Soft-proofing · gamut mapping / rendering intents | ✅ **M6.4 soft proofing shipped 2026-09-04**, confirmed by use. ✅ **Rendering intents: M6.4c, 2026-09-26** — Clip (default, by the author's choice from crops), Relative colorimetric, Perceptual. `specs/2026-09-26-m6.4c-rendering-intents-design.md`. |
| EXIF/IPTC metadata editing | ✅ **M6.2, 2026-09-08** — edited in the catalog, embedded at export, in the sidecar |
| Export filename templating · output/print sharpening | ✅ **M6.1a** / ✅ **M6.1d**, both 2026-09-01 |
| Mask intersection and subtraction | ✅ **M7.1c, 2026-09-10** — confirmed by use; Add / Subtract / Intersect between masks, and **M7.1 is complete** |
| Reorder a mask's components | ✅ **2026-09-29, widened to the masks themselves** — drag a row; the top component must add; `grep -n 'Reordering masks' docs/JOURNAL.md` |
| A better interpolator for the geometric stages | ✅ **M7.3a, 2026-09-01** — a `Settings` choice defaulting to Catmull-Rom, both samplers |
| Fuse perspective into `GeometryStage` | ✅ **M7.3b, 2026-09-25** — one gather; acutance 0.00700 → 0.00823; prefix reordered (journal) |
| Automatic vertical detection (Hough) | ✅ **M7.4, 2026-09-04** — reshaped into `Auto` / `Refine guides` |
| ~~Auto-mask brush~~ · ~~dehaze bright subjects~~ · ~~negative Texture~~ · ~~Clarity halos~~ · ~~grading in masks~~ | **M7.6** — grading in masks ✅ 2026-09-05; negative Texture and Clarity halos ✅ 2026-09-02 (W3, struck late); dehaze bright subjects ✅ 2026-09-08; auto-mask ✅ 2026-09-09 (struck 2026-09-29) |
| ~~Lens correction row batching~~ · ~~sequential export~~ · incremental splatting | **M8.1** — row batching ✅ 2026-09-01 (1178 → 309 ms); parallel export ✅ **M8.10**, 2026-09-15; incremental splatting ⏸ **M10.14f** (parked until a need shows) |
| The `wgpu` seam | ✅ **M8.3a, 2026-09-10** — one stage across the seam, an equivalence harness, and the cost measured: **3.1–4.7× slower**, bit-exact. Nothing in the app uses it. ⚠ M8.3b's first task is **not** porting a stage: hunt a zero-copy upload (the transfer is the floor at ~6 ms against 0.04 ms of compute) and measure transcendental divergence. `specs/2026-09-10-gpu-seam-design.md` |
| GPU: a resident chain and HSL | ✅ **M8.3b, 2026-09-10** — N stages share one round trip, and global HSL runs on the GPU at **~2×** (CPU ~21 ms against ~10) behind a Settings toggle, diverging by 0.00034 of an 8-bit level. ⚠ **Masked HSL is NOT accelerated** — it lives inside `MaskStage`, and it is the case reported in use as slow. `specs/2026-09-10-resident-gpu-tail-design.md` |
| GPU: spatial stages and run-merging | ✅ **M8.3c, 2026-09-11** — sharpening as compute shaders on shared scratch planes, consecutive GPU stages merged into **one** round trip, `gpu_hsl` → `gpu_tail`. **1.52×** on hsl + sharpening; ⚠⚠ the merge itself is worth **10.17 ms** (merged 21.84 ms against split 32.01), so without it a second stage was worth 1.18× — run-merging is what makes it pay at all. ⚠ **An active colour grade splits the run and takes the win to 1.04×.** `specs/2026-09-10-spatial-gpu-design.md` ✅ **confirmed by use** — and the report adds what no measurement here did: **`DragQuality::Full` is usable while dragging** with the toggle on, i.e. the interactive rung's deliberate off-switch earning its place. |
| GPU: local contrast | ✅ **M8.3n, 2026-09-13** (struck 2026-09-29). ~~Deliberately out of M8.3c's scope — it needs three blur kinds.~~ |
| GPU: colour grading | ✅ **M8.3d, 2026-09-11** — the acceptance row went **1.04× → 2.05×**, the best ratio in the bench, and `hsl + grade + sharpening` is now **one chain of three**. ⚠⚠ The GPU column barely moved (25.75 → 25.12 ms) while the CPU column climbed 12 ms: a stage joining a run that already pays the transfer is close to free. `specs/2026-09-11-gpu-grading-design.md` |
| GPU: shader and pipeline caching | ✅ **M8.3f, 2026-09-11** — the module, layout, pipelines and a staging-buffer pool survive a render (struck 2026-09-29) |
| GPU: the mask loop · the prefix | ✅ **M8.3e–q, M8.5–M8.14** — Phase 8 closed 2026-09-18 (struck 2026-09-29) |
| On-disk thumbnail cache | ✅ **M8.2, 2026-09-10** — a folder opened before fills in at **1.26 ms per entry against 190.41 ms**, 151×, for ~60 KB per photo. Caches the extracted **preview**, not the finished thumbnail. ⚠ Nothing for the *first* open, which stays ~26 s per 830 entries. `specs/2026-09-10-thumbnail-cache-design.md` |
| Decode a full-size JPEG at 1/8 scale for thumbnails | ⛔ **MEASURED AND REJECTED, 2026-09-10 — and this entry's own earlier claim was wrong.** It said DCT-scaled decode was "the only lever that moves the cold folder open", implying a large win. Measured: **1.6×** (222.3 → 143.2 ms), not the ~8× the framing suggested. Three reasons, each checked rather than argued. **(1) Entropy decoding is irreducible** — Huffman work is proportional to the compressed data, not the output size, so DCT scaling skips only the IDCT, upsampling and colour convert. Of a 26 Mpx camera JPEG's 226.9 ms, decode is **178.1 ms (78%)** and the resize only 43.8 ms. **(2) `image` 0.25 cannot do it at all** — its backend is `zune-jpeg`, whose 1×1/4×4 IDCT paths are a sparse-coefficient optimisation producing full-size blocks, not an output-scale knob. The only Rust crate that offers `scale()` is `jpeg-decoder`, which is a **slower decoder than the one already in the tree**, eating much of the gain. **(3) The embedded EXIF thumbnail is 160×120** — far too small for a 512 px cell, so the trick that makes RAW cheap does not transfer. ⚠⚠ **And M8.2 removed most of the motive**: the cold open now happens **once per folder, ever**, so this would spend a second JPEG decoder in the tree to turn a one-off ~26 s into ~19 s. Reopen only if a faster scaled decoder appears in `image` itself. |
| Replace libraw's demosaic | ✅ **M8.4, 2026-09-10 — decided OUT of scope**, and the default tier moved `Fast` → `Better` instead. `Better` sits within 0.22–0.30 of an 8-bit level of `Best` on foliage and is indistinguishable from it at 4×, so a Markesteijn/AMaZE-class reimplementation buys nothing measurable. ⚠ Costs ~2.9 s → ~7.5 s per photo opened. `specs/2026-09-10-demosaic-decision.md` |
| `main.rs` is too large | ✅ **W0, 2026-09-01** — 4,375 → 681 (917 as of 2026-09-08); `canvas.rs` 1,890 → 7 modules |
| The in-window menu bar has never been run | **M11.2** (was M9.2, then M10.2) |
| Immich integration | **M9.1** |
| Contributor-facing documentation | **M11.3** (was M9.3, then M10.3) |
| A plugin system | **M11.5** (was M9.5, then M10.5) |

⚠⚠ **One entry changed shape rather than being scheduled: EXIF/IPTC editing.** It was blocked on
"modify the original" versus "a second XMP sidecar". **Writing metadata only into the *exported*
file is neither** — the original is untouched, no new sidecar exists, and it is exactly what M6.1b
needs anyway. Metadata is edited in the catalog and embedded at export; XMP sidecars become M6.3's
problem, where they belong. **The design question that blocked this is answered; do not re-open it.**

~~Not scheduled, and deliberately:~~ **All scheduled 2026-09-29 in Phase 10, by the author's call:** the
C2/C3 oracle (M10.13a), per-photo camera profile (M10.13c), `CameraCalibration` (M10.13b),
brush-strokes-shift-under-lens-correction (M10.12c, ⏸ blocked), presets import/export (M10.9a),
user-defined shortcuts (M10.9b), vertical filmstrip (M10.8b). Three were already settled: the accent
colour (✅ `DESIGN.md` §4.2), subfolders (✅ M9.9) and the nicer decode placeholder (superseded by M9.14,
and against the author's rule that no low-resolution stand-in is ever painted).

---

## First-session reviews — the milestone-sized findings (recorded 2026-09-28)

Two reviews of a first session, an amateur's and a professional's, were written on 2026-09-28 and
deleted when their fix pass merged (`grep -n 'First-use review fixes' docs/JOURNAL.md`). Everything
small shipped; these are the items that are a milestone each, kept here with the numbers the
reviews measured so the deletion loses nothing. Ids are the professional review's.

- **P-20 · No pre-rendered full-size previews — ✅ M9.14, merged 2026-09-29, confirmed by use.** ⚠ The 7.4 s below is the three-parallel per-frame figure; a single Better decode is 16 s (`grep -n 'M9.14' docs/JOURNAL.md`). The folder-wide build was declined on that cost; the camera's embedded JPEG is the free 1:1 instead. The original entry: A RAW
  opens editable at half size in ~1 s; true 1:1 waits for the full decode, which costs **1.3 s
  (Fastest) · 3.0 s (Fast) · 7.4 s (Better, the default)** on X-T4 frames
  (`specs/2026-09-10-demosaic-decision.md`), and the session saw "Full resolution ready" ~11 s after
  opening. A focus check across a 500-frame shoot is a wall of seconds per frame. Lightroom hides the
  cost by building 1:1 previews in the background at import. The review's ask: a background
  full-size preview build after import or folder open, bounded by a disk budget like the thumbnail
  cache (`ThumbStore`, 5 GB default), with the grid and Compare able to zoom into it.
- **P-21 · Catalog scale is unproven at archive size.** → **M10.14e**. Documented testing reached ~830 files a folder
  and ~1,700 rows; the author's catalog holds 6,235 and opened a 398-photo folder without trouble. A
  working archive is 100k–1M. Ask: a synthetic 100k-row benchmark for All Photos, search, smart
  collections and the folder tree, with the numbers written down.
- **P-22 · No camera-matching colour out of the box.** → **M10.13d**. No camera profiles ship; without a user `.dcp`
  a camera gets the matrix rendering, a Fuji shooter gets no film simulations, no LUTs ship. Ask: a
  legal route to profiles (community DCP/ICC sets, a ColorChecker workflow) — the *starter looks*
  half of this shipped in the fix pass (nine built-in presets).
- **P-23 · No AI denoise.** → **M10.16b**. The guided-filter NR is pre-AI; `ai-mask` already brings `tract` and a
  model registry, so a denoise model is the natural next tenant.
- **P-24 · No HDR merge, no panorama merge.** → **M10.17a/b**. Unplanned rather than unbuilt — neither appears anywhere
  else in this file or the journal.
- **P-25 · Scope: tethering, GPS/map, people tagging, publish services, print** → **M10.18a–e**, brought
  into scope by the author 2026-09-29. Originally: out of scope by
  declaration (tethering is on the target list, untouched). Recorded so the scope statement is explicit.
- **P-26 · Small professional gaps:** → batch rename **M10.8d**, image watermark **M10.10a**, export
  sizing **M10.10b**, Grain **M10.11b**, content-aware remove **M10.16c**. text-only watermark; rename one photo at a time (the import
  template exists); export resizes by pixel long edge only (no cm/in, PPI or file-size target); no
  content-aware remove; no Grain slider.
- **P-28 · Cross-platform is a promise so far** — the GUI has only run on this Mac; **M11.2** (was M10.2).
- **P-29 / P-30 · Decisions, recorded not disputed:** the tool panel is exclusive (Masks hides the
  global sliders; tried the other way and reversed 2026-08-24), and the panel groups follow the edit
  groups rather than Lightroom's Basic panel (consistent with Sync and Paste, the stated reason).

## Blocked on a design question

These are **not** small jobs waiting for a free afternoon. Each needs a decision made before any
code, and picking wrong produces output that looks fine and is wrong.

### Creative 3-D LUTs (`.cube`) — the last part of M3.3

*Deferred 2026-08-25, after M3.3's grading half landed.*

**What it is.** Importing a 3-D lookup table — `.cube`, `.3dl`, `.look` — as a creative look:
film emulation, a colourist's grade, teal-and-orange. ⚠ Not to be confused with the two LUTs
already in the codebase: the 1-D table inside `ColorGradingStage` (an implementation detail), and
the DCP `HueSatMap` tables in `color::huesatmap` (built, colour management C3).

**The mechanical part is trivial** — a `.cube` is `LUT_3D_SIZE N` followed by N³ RGB triples with
red varying fastest; index the cube, interpolate trilinearly. Parser, interpolation and a stage is
roughly 150 lines, cheaper at runtime than the Oklab round trip, and it has an exact test available
(an identity LUT must be **bit-exactly** the identity). If this were all of it, it would already be
built.

**⚠⚠ The blocker: a `.cube` file does not say what colour space its input is in.** The format has
no field for it. Meanwhile this pipeline holds **scene-referred linear ProPhoto with values above
1.0** at every point — a space essentially no LUT in the wild was authored against. Almost all of
them expect display-referred Rec.709/sRGB, or a specific log encoding (ARRI LogC, Sony S-Log3,
ACEScct), each with its own transfer curve *and* primaries.

Three concrete consequences:

1. **A shaper is required, and getting it wrong fails silently.** You must transform into whatever
   the LUT expects and back out. Feed a Rec.709 LUT linear ProPhoto and there is no error — just a
   plausible, wrong image that reads as a stylistic choice. Same failure class as the inverted ICC
   tone curve, which is why `the_profile_agrees_with_our_own_maths` had to cross-check against an
   independent implementation rather than round-trip.
2. **⚠ The cube is bounded and our data is not — and the HueSatMap trick does not transfer.** That
   table clamps the *index* while applying the result to the real value, which works **because it
   stores a scale**. A `.cube` stores an **absolute output colour**, so above 1.0 there is nothing
   to scale and the table simply has no answer. The options are clip (destroying the headroom the
   wide working space exists for), tone-map first (which is deciding a display rendering — exactly
   the question that excluded `ProfileToneCurve`), or extrapolate past the cube edge (ill-defined).
3. **Placement collides with two things already shipped.** A display-referred LUT wants the very
   end of the chain — but after the output transform it cannot be masked, and it would have to run
   in a **fixed** space regardless of export target, or the same LUT would give different results
   exporting sRGB versus P3. And the histogram is derived from `pack`'s sRGB quantization, so a LUT
   after that point would leave the histogram pre-LUT — literally the bug the display-profile work
   shipped and caught by re-reading what consumes the bytes.

**The options, when this is picked up:**

- **Fix one convention** — treat LUTs as sRGB display-referred, round-trip into sRGB just before
  the output transform. Simple, covers most downloadable looks, silently wrong for every log LUT.
- **Declare the input space per LUT at import** — a small enum the user picks, which is what Resolve
  makes you do and is the honest answer. ⚠ Cost: each log encoding is a specific transfer function
  needing implementation *and* validation, and there is no sample footage here to check LogC or
  S-Log3 against — they would ship unverified, the position C2 was in without its oracle.
- **Full ACES IDT/ODT** — correct and well out of scope.

**Recommended shape:** the second, staged — the enum from the start, populated initially with only
the spaces that can actually be verified (sRGB display, Rec.709, linear passthrough), with log
encodings added when there is something to validate them against.

✅ **RESOLVED IN W5-0 (2026-09-04): the recommended shape was taken, and it turned out to answer the
placement question too.** The declared input space decides **where the LUT runs** — display-referred
LUTs modify the camera rendering, a linear one runs *in place of* it because it **is** the rendering.
That dissolves consequence 3 above: a film emulation stacked on our S-curve would double-render,
which is the commonest way this feature ships broken, and one declaration prevents it. Consequence 2
is settled by clipping into the cube, which is defensible **only** because a creative look is itself
a display-referred decision — this is the point in the chain where headroom is legitimately spent.

⚠ **Start the brainstorm with the space question, not with a parser.** The parser is the easy half.

### `ProfileToneCurve` (DCP)

*Deferred during colour management C3, 2026-08-25.*

Adobe's *display* rendering, baked into a camera profile. Excluded for an **architectural** reason,
not effort: the decode produces scene-referred linear, and every downstream stage is tuned against
linear luminance — `local_contrast`'s `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS`/`GAIN_MAX`/
`HALO_KNEE`, `dehaze`'s airlight percentile. Baking a display curve into the decode would leave the
buffer non-linear while every constant still assumed otherwise: invisible per stage, wrong
everywhere. It is the identity in the X-T4 profile anyway.

⚠ Shares its unanswered question with creative LUTs above: *where does a display rendering live in
a scene-referred pipeline?* Answering it once probably unblocks both.

### `ProfileLookTable` (DCP)

*Deferred during C3.* Deferred for **ordering**, not effort — the spec applies it *after* the tone
curve, so with no tone curve it would land in the wrong space, which is worse than not applying it.
It is also absent from the X-T4 profile, so it could not be tested against the files this project
validates on. The machinery is shared: a LookTable is the same 3-D HSV table as `HueSatMap`.

---

## Colour management follow-ups

Seeded from the recorded follow-ups formerly in `CLAUDE.md`; **the sections are now in
`docs/JOURNAL.md`** (split out 2026-09-16) — `grep -n '^### ' docs/JOURNAL.md` for the index.

- **C2/C3 have no independent oracle.** → **M10.13a**. `rawtherapee-cli` is installed but crashes on this machine
  (exit 133, SIGTRAP, no output). The dual-illuminant interpolation is therefore **bounded but not
  independently verified** — it is judged by construction and against libraw, not diffed against a
  reference DCP implementation. This is why `camera_profile` stays opt-in.
- **Per-photo camera profile.** → **M10.13c**. C4 is per *camera*, keyed by `Session::make_model`. Per-photo is
  what Lightroom does in its Calibration panel and becomes wanted the moment someone holds both
  "Camera Velvia" and "Adobe Standard" for one body. ⚠ It puts a *decode* parameter into `Edits`,
  which must then travel through the sidecar, presets, copy/paste, sync **and the buffer-cache
  key** — that key is where the work is.
- ✅ **Soft-proofing — shipped as M6.4, 2026-09-04.** As recorded: the preview still cannot show colour outside sRGB even when exporting P3,
  because the pipeline clips to sRGB before the display transform. The macOS colorspace work makes
  this easier: tag the layer P3 and render P3.
- ~~**Gamut mapping / rendering intents at export.**~~ ✅ **M6.4c, 2026-09-26.** The per-channel clip
  stays the default by the author's choice; perceptual and relative colorimetric are offered beside it.
- **`CameraCalibration` is neither parsed nor applied.** → **M10.13b**, with `ProfileLookTable`. A profile carrying one renders as though it
  were the identity. Recorded rather than silently mis-rendered.
- **Multi-monitor on Linux and Windows.** → **M11.2**. macOS hands the preview to Core Animation, which handles
  a display change for free. Elsewhere `display_space` is one global setting and nothing reacts to
  moving the window between monitors. Wayland's colour-management protocol is the eventual answer.

---

## Editing tools

### ✅ The rest of the stages inside a mask — CLOSED 2026-09-06 (asked for 2026-09-05, at M7.2's merge)

**Asked for directly**: *"I'd also like to add the dehaze and rest to masks."* M7.2 gave a mask
Texture, Clarity, Sharpening and HSL. This is the remainder.

⚠⚠ **IT IS TWO JOBS, NOT ONE, AND THEY ARE NOT COMPARABLE IN SIZE. Do not scope this as a single
milestone.** The split is by *where the stage lives in the pipeline*, which is the whole difficulty:

#### ✅ Part A — the tail-native two: colour grading and the tone curve. **DONE 2026-09-05.**

Spec `2026-09-05-masked-grade-and-curve-design.md`, plan `2026-09-05-masked-grade-and-curve.md`.
1991 → **2003 tests**, 55 → **57 goldens** with the 55 byte-identical. It went exactly as scoped: a
field on `LocalAdjustments`, a line in `adjustment_stages`, a neutral pass forced at compile time by
`build_pipeline_split`'s exhaustive literal, the two `..Default::default()` fixtures updated by
hand, and two collapsed panel blocks. **No structural question arose.**

⚠ **Colour grading's exclusion was re-decided rather than inherited, and it ships.** The stated
reason — *"`LocalAdjustments` is per-pixel-only and a grade is global look"* — had its first half
made false by M7.2, and its second half is a judgement about taste rather than structure.

⚠⚠ **THIS ENTRY'S OWN COST NOTE WAS WRONG, AND MEASURING IS WHAT CORRECTED IT.** It flagged
grading's 256-entry table build as the thing to measure and said a masked tone curve is *"more
expensive"*. Measured on an X-T4 at fit, against a 12.2 ms neutral: **masked curve +5.4 ms, masked
grade +15.8 ms**, with clarity at +10.1 and HSL at +23.7. **The curve is the cheapest thing a mask
can carry**, despite building the *larger* table (1024 entries against 256) — the table build is
negligible, and what predicts a masked stage's cost is whether it does an **Oklab round trip**. The
curve was only more expensive in *panel space*, which the collapsed blocks answer.

⚠ **The no-bounding-box finding reproduces on both new stages**: a small radial whose bbox is ~5.8%
of the frame costs slightly *more* than a frame-covering gradient (+6.7 vs +5.4 curve, +16.6 vs
+15.8 grade). Same as M7.2 measured for clarity and HSL.

#### ✅ Part B — dehaze and noise reduction. **DONE 2026-09-06.**

Spec `2026-09-06-masked-capture-corrections-design.md`, plan
`2026-09-06-masked-capture-corrections.md`. Both run in the **prefix**, where the global stages of
the same name run: a `capture_stages` builder beside `adjustment_stages`, a second `MaskStage`
constructor taking no region at all, and a `PrefixStepKey::MaskedCapture` keyed on a projection.

⚠⚠ **THE BLOCKER BELOW WAS FACTUALLY WRONG, AND CORRECTING IT WAS MOST OF THE MILESTONE.** It is
kept verbatim as a record. **Noise reduction and dehaze run *after* geometry**, not before —
`build_prefix_steps`' own tripwire spells the order out — so the prefix output *is* the
post-geometry image, which is exactly the space every mask is authored in. **The coordinate-space
objection, the load-bearing half, was never true.** The sentence had propagated into four
documents, including Part A's spec written the day before.

⚠ The second objection was weaker than it read too: *"runs at a different point than its global
twin"* is already true of **every** masked adjustment, since masked exposure runs after the global
tone curve. Accepted since M3.1a.

⚠⚠ **The REAL blocker was different and dehaze-only:** `MaskStage::apply` runs its sub-stages on
the buffer it is handed, which in a region render is the *region*, and `airlight` reads the
brightest 0.1% of it — so a tail-side masked dehaze would re-estimate its airlight as you panned
and the photograph would change under you. **Placement answers it rather than working around it**,
and the master region invariant is what proves that: moving the capture stages into the tail fails
it with *"the region's interior must match the whole render bit for bit"*.

⚠ **Measured, and the second column is the point:** masked NR **+29.9 ms at fit, ~0 at the native
region tail**; masked dehaze +28.7 / ~0. Being in the prefix means they are **cached** — ~29 ms
when their own slider moves and nothing at all on every other frame, where masked HSL costs
+17.9 ms every time.

⚠ Accepted and pinned by a test: a **range mask selects on pre-tonal pixels in its capture half**.
For a correction of the capture that is arguably the more correct answer, and it is what the global
stages already do.

**The superseded reasoning, kept as a record:**

Both live in the **prefix**, before geometry; `MaskStage` lives in the **tail**. So a masked one
would run *after* the tone curve, denoising or dehazing pixels the curve has already stretched —
**a different operation from the global stage wearing the same name.** Three ways out, none free:

1. **Accept the tail position and say so.** Masked NR after the curve is *not* the global stage
   confined to a mask; it is a smoothing tool with different behaviour on the same slider. ⚠ This
   project has been bitten by exactly that shape before — the Whites slider read perceptual thirds
   off linear luminance. Two things with one name that behave differently is the failure to avoid.
2. **A second, prefix-side masking stage.** Correct semantics, and expensive: prefix stages run
   **whole-buffer**, so the coverage would have to be evaluated at prefix resolution *before*
   geometry — which is a different coordinate space from the one every mask is authored in.
   ⚠ Brush strokes are already anchored pre-geometry, so they are the easy case; a linear gradient
   and both range masks are frame-relative and are not.
3. **Duplicate the stage, tuned for its position.** Honest, and doubles the surface to maintain.

⚠ **Dehaze additionally estimates a frame-global airlight from the brightest 0.1% of pixels.**
Confined to a mask that has no obvious meaning — the airlight is a property of the *scene*, not of
the region. Decide whether a masked dehaze uses the frame's airlight (defensible, and the reason it
can stay one number) or the mask's own (which would make the same slider read differently per mask).

⚠ **Skin softening is NOT a reason to do Part B**, and thinking it is would be the trap: Texture is
bipolar and −100 removes the fine band, so M7.2 already covers the commonest use anyone reaches for
NR-in-a-mask to get.

**Order:** ✅ Both done — A 2026-09-05, B 2026-09-06. The split was right: A re-ran M7.2's playbook
as predicted, and B turned out to be blocked on a *different* question from the one recorded here,
which only a brainstorm would have found.

⚠ **What is still OUTSIDE a mask, so nobody reads this entry as "masks now carry everything":** lens
correction, perspective, spot removal and geometry (all upstream of where a mask's coverage is even
defined), the vignette and the creative look (frame-wide by nature), and the output transform. A
mask carries every *adjustment*; it does not carry the geometry or the delivery.

### ⚠⚠ Anchor strokes and spots in sensor space (recorded 2026-09-03 — blocked on a MISSING MAP) → M10.12c ⏸

**Not "extend `FrameMap`", and the name matters.** `FrameMap` is not wrong: it models
orientation → straighten → crop faithfully, which is its entire domain. The bug is that the space
it maps *into* is the wrong anchor.

Brush strokes and retouch spots are anchored in **`GeometryStage`'s input**, which sits
*downstream* of lens correction and perspective. So the stored coordinate stays put while the
content under it moves: toggling either correction after painting or after placing a spot shifts
the mark — measured at up to **~3.5% at the corners**, ~0 at the centre, which on a 6246 px frame
is ~220 px and therefore **larger than a typical spot radius**.

The principled fix is to anchor in true **sensor** space and map *forward* through the distortion
when rasterizing. ⚠ **That is what blocks it: lensfun exposes only the inverse** (`Corrector::source`,
corrected → raw), because that is the direction resampling needs. A forward map has to be built by
inverting it numerically, per lens and per focal length.

⚠ It affects the brush and spot removal identically and predates both M5.3a and M5.3b, so it is its
own change rather than either milestone's.

⚠ **What is *not* blocked, and was fixed in M5.3b:** *sampling* the correct pixels. The inverse map
is exactly what a search needs — given a point in stage-input space, `perspective::source_of` then
`Corrector::source` reach the raw decode. M5.3b's auto-source composes those rather than indexing
the raw buffer with post-lens coordinates, which would have been wrong by the whole displacement
above. Only *anchoring* wants the forward map.

- ~~**Mask intersection and subtraction between masks.** `erase` works only *within* one brush; a
  radial cannot be cut out of a linear.~~ ✅ **M7.1c, 2026-09-10**, confirmed by use. A mask carries
  `refine: Vec<Refinement>` — shapes composited onto its base with **Add / Subtract / Intersect**,
  folded left to right. ⚠ The algebra is **the brush's own**: `Add` is `raster.rs`'s across-strokes
  paint step and `Subtract` is its `erase`, and the raster calls `MaskOp::combine` now, so the two
  cannot drift. `min`/`max` were rejected because they agree with those rules only at 0 and 1.

- ✅ **BUILT 2026-09-29, masks included** (`mask_order`; `grep -n 'Reordering masks' docs/JOURNAL.md`). The
  original entry: **Reorder a mask's components** (recorded 2026-09-10, **wanted** — user's call: *"the reordering
  is something we should do at some point"*). Components are **append-only, delete-any** today, and
  **order is observable**: the fold runs left to right, so subtract-then-intersect is not
  intersect-then-subtract. ⚠ The gesture is already solved — `panel_order::{drop_index, moved}` is
  the drag-to-reorder logic the Library panel's sections use, pure and tested, including a
  permutation property over every from/to pair. The work is a second call site plus deciding
  whether a reorder is one undo step. ⚠ Cheap, and **not** blocked on a design question, which is
  what distinguishes it from most of this file.

- **Composites are one level deep.** → **M10.12a**, re-opened by the author 2026-09-29. A refinement is a *shape*, never another composite, so
  `radial ∩ (brush ∪ gradient)` is not expressible. Deliberate — it is the nesting the rejected
  recursive `Mask::Combine` variant would have bought, at the cost of a tree in the panel and a
  path rather than an integer for every hit-test, drag and coverage-cache index. Revisit only with
  a real photograph that needs it.
- ~~**Auto-mask / edge detection** for the brush.~~ ✅ **2026-09-09, and M7.6 closes with it** — the last M7.6 gap; a dab paints only pixels similar to the one under its centre, gated in Oklab at render time. ⚠ Its coverage is **cached** since 2026-09-09, keyed on what produced the base — see the entry below.
- ~~**Dehaze over-corrects bright achromatic subjects** — snow, white walls, a white cat read as haze.
  The classic dark-channel-prior artifact, inherent to the model; a real fix needs large-bright-
  region detection and is its own feature.~~ ✅ **2026-09-08** — an airlight tolerance in
  `transmission`, measured on 415 frames (spec `2026-09-08-dehaze-bright-tolerance-design.md`).
  ⚠ Not region detection: the catalogue holds no genuinely hazy frame to check that against, and a
  hazy sky is locally a white wall. Left open, as a limit rather than a bug: a bright object whose
  frame's airlight is a *different* bright region (a wall behind a window, DSCF6327, 8.4% at +100).
- ~~**Negative Texture is not edge-aware**, so it softens edges within its band. Deliberate — noise
  reduction already owns edge-aware smoothing and is upstream. A guided-filter base for the negative
  half is the follow-up, at the cost of the tool behaving differently either side of zero.~~
  ✅ **2026-09-02, W3 track D** (`TEXTURE_GUIDED_EPS`, retuned by use). ⚠ Struck only on
  2026-09-08: this bullet outlived the code by six days and was copied into a "still open" list.
- ~~**Clarity has no halo suppressor**; the midtone weight is the mitigation. A `Detail`-style
  suppressor is a contained follow-up.~~ ✅ **2026-09-02, W3 track D** (`halo_suppression`,
  `169984b`). Same lapse as the bullet above.
- ~~**No output/print sharpening** for resized exports.~~ ✅ **M6.1d, 2026-09-01** — in the file's display space, deliberately.
- **Brush strokes shift when lens corrections are toggled.** → **M10.12c** ⏸ (the entry above). Strokes are anchored in
  "pre-geometry" coordinates, which means *the input to `GeometryStage`* — and lens correction now
  runs ahead of it, so enabling or disabling it redefines that basis. The shift is up to ~3.5% at
  the corners and ~0 at the centre. ⚠ The principled fix is to include distortion in `FrameMap` so
  strokes anchor in raw sensor coordinates and travel *with* the correction — which is what "a
  stroke must stay on the cat" actually demands, since the correction moves the cat. It needs the
  **forward** distortion map and the `lensfun` crate exposes only the inverse, so it means an
  iterative solve per stroke point. Deferred on the grounds that lens corrections are enabled once,
  early, not toggled mid-edit.
- ✅ **Automatic vertical detection** — **shipped as M7.4, 2026-09-04.** Kept for the `mod_pc` correction below. (recorded 2026-08-31, the last slice of perspective). Find the
  lines a guided correction would need, without the user drawing them. ⚠ Needs a **Hough
  transform** over an edge map — a new image-analysis subsystem, not a UI job — and it still needs
  the manual controls underneath it for when the detection is wrong. ⚠ The solve itself is already
  built and is a pure function: `pipeline::perspective::guides::solve` takes two vanishing points,
  so detection only has to produce those.

  ⚠⚠ **A correction to what this entry used to say:** it claimed guided *"is the half `mod_pc`
  actually serves"*. Guided shipped **without lensfun at all** — `mod_pc` returns a matrix built
  from its own estimated focal length, which cannot be decomposed exactly into our fixed-`F_NORM`
  sliders, while solving in our own model is closed-form. Do not send the next reader to that API.

- ✅ **A better interpolator for the geometric stages** — **shipped as M7.3, 2026-09-01.** (recorded 2026-08-31). ⚠⚠ **This is the
  larger win and it was found by accident** while measuring something else. Mean-absolute-Laplacian
  acutance on a real X-T4 frame: neutral **0.03186**, after **one** bilinear resample **0.01661** —
  a **48% loss from a single pass**. That already applies to straighten and to lens correction,
  which ship today; it is not specific to perspective. Bicubic or Lanczos in `sample_bilinear`
  would improve every geometric stage at once. Reproduce with
  `cargo run --release -p app --example persp_sharpness <raw>`.

- ✅ **Fuse perspective into `GeometryStage`** — **shipped as M7.3b, 2026-09-25**
  (`grep -n 'M7.3b' docs/JOURNAL.md`). Straighten + keystone acutance 0.00700 → 0.00823, equal to a
  single resample. ⚠ The anchoring danger below was real for **spots** (they moved to
  pre-perspective space, and no sidecar had both) and **not** for brushes, whose anchor space survives
  as coordinates inside the fused map. The "three passes to one" note at the end stays open: folding
  lens in would lose `fill_row`'s per-row path. The original entry, kept for the record:
  (recorded 2026-08-31). One resample instead of two when
  straighten and perspective are both active. ⚠ **Measured at ~20%** of acutance — real, but
  secondary to the interpolator question above, which is why that one came first. ⚠⚠ **And that one
  is DONE: M7.3a shipped 2026-09-01**, so the 48%-of-a-single-pass win is already taken and this
  ~20% is what is actually left. Do not read the entry above as still-open work — it is struck
  through for a reason, and `CLAUDE.md`'s own warning applies: verify a backlog item against what
  shipped before proposing it, since several were closed under another name. ⚠ The cost is
  that `FrameMap` must then invert a **homography** rather than a rotation, and its agreement test
  with `GeometryStage` is exactly what caught the rotation sign error in M3.1c-A. It also loses
  perspective's own cache checkpoint, so every crop drag would re-run the resample.

  ⚠⚠ **AND THE LARGEST DANGER WAS NOT IN THIS ENTRY UNTIL NOW (recorded 2026-09-10, from reading
  the chain rather than measuring): FUSION MOVES EVERY BRUSH STROKE AND EVERY SPOT ON ANY PHOTO
  THAT USES PERSPECTIVE.** Both are anchored in *pre-geometry source coordinates* — which is
  `GeometryStage`'s **input**, the exact buffer fusion deletes. Today that space is post-lens,
  post-perspective; fused, "pre-geometry" would mean **pre-perspective**, so the same stored
  numbers point at different pixels. This file already records the related caveat *"toggling
  perspective after painting shifts brush strokes"* — but that is a user action, where this would
  be a **silent change of meaning for sidecars already on disk**. ⚠ Verify it before committing to
  the milestone; it is read from the code, not measured.

  ⚠ **`SpotRemovalStage` sits in the middle of the chain and is a pixel edit, not a coordinate
  map**, so the two ends cannot simply be concatenated around it. Fusion forces spots to move to
  *before* perspective, which is what causes the anchoring problem above.

  ⚠ **When the saving actually lands, which is narrower and wider than it looks.** `GeometryStage`
  resamples **only when `angle_rad != 0`** (orientation is a lossless permutation, crop is a
  slice), and `PerspectiveStage` skips a neutral homography — so the double pass needs *both*
  live. That sounds like a corner case and is not: **M7.4's guided solve writes the straighten
  angle itself**, since roll falls out of the vanishing-point solve for free. Anyone using Guide
  Mode gets both set, so for the only audience perspective correction has, the double resample is
  the normal case. On a RAW it is a **third** pass, because lens correction is on by default.

  ⚠⚠ **The bigger prize, and why this entry may be the wrong shape.** Lens displacement, the
  homography and the rotation are all *coordinate maps*, and `app::spot_source::sampler_for`
  already composes their inverses to sample one pixel. The general version is **one resample for
  the whole geometric chain — three passes to one**, worth far more than this entry's three to
  two. `SpotRemovalStage` in the middle is what blocks it, and the anchoring danger gets sharper
  rather than softer.
- ~~**Colour grading does not reach masks.**~~ ✅ **Shipped 2026-09-05** (Part A of "the rest of the stages inside a mask"). ~~Still open: no visual feedback of the grading range boundaries on canvas or histogram.~~ ✅ **2026-09-08** — the grade range view (`grade_view.rs`): a "Show range" checkbox tints the photograph where the selected range reaches, read from a tap at the grading stage's *input* — the histogram and the packed pixels both sit past the display S-curve, so neither could have shown it honestly. Spec `2026-09-08-grade-range-view-design.md`.

---

## Performance

### ⛔ One GPU device for the window and the pipeline — DECLINED on a measurement (2026-09-26)

⚠⚠ **Measured before building, and the premise failed.** With `RUSTROOM_DEBUG_NO_UPLOAD` (the window
keeps its old picture and never uploads a new frame), the tail's GPU submit+wait in a real drag was
**36.8 ms median at rung 3200 (n=150)**, the same as with uploads (~34–37) and still ~10 ms over the
headless probe (~26.8). So the window's handling of new frames (the readback, the conversion and the
re-upload), which is what sharing a device would remove, is **not** where the gap goes. Add the
drawbacks (a GPU fault would take the window down; one queue would let a 700 ms export submission
freeze the UI; startup coupled to eframe) and it is not worth building. ⚠ The gap's real cause is
**unidentified**: the window's baseline drawing, or the app's render thread being scheduled
differently from a CLI probe. Neither is measured. Do not re-propose sharing without a new
measurement that points at it.

The original entry, kept for the record:

`gpu_pipeline::shared()` creates its own wgpu device, and eframe has another. In the app a Straighten
drag frame is ~41 ms against ~29 ms headless, and the whole difference is GPU submit+wait (the CPU
copies match exactly). A synthetic second-device load reproduced **part** of it (+3.8 ms median over
five runs, p90 up to 54 ms). Reusing the preview texture freed the UI thread (5.5 → 2.0 ms) and changed
the drag frame not at all. **What sharing would buy:** one queue instead of two contending, and the
tail's 8-bit output written straight into the display texture (no 27 MB readback, no `tovec`, no second
upload). ⚠ **What it costs:** the render engine would need eframe's device before the first photo opens,
and the histogram and the readout would still need bytes (a small readback, or computed on the GPU).
⚠ The ~12 ms is an upper bound on the win, not an estimate of it. Measure the window's own GPU time
first. `grep -n 'The gap, measured' docs/JOURNAL.md`.

### ⛔ CLOSED 2026-09-18 — spot removal on the GPU is NOT worth doing, and this is why (MEASURED)

Item 5 of the agreed GPU-port order, closed **without being built**. `crates/app/examples/probe_spot_cost.rs`
is committed and re-runnable.

**Measured on a 6.5 Mpx frame.** ⚠ `apply` consumes its input, so a **1.95 ms buffer clone** is
inside every raw figure; the numbers below have it subtracted, and it is paid by the real chain too.

| radius | 1 spot | 5 | 20 | 100 |
|---|---|---|---|---|
| **12 px** (a dust speck) | 0.05 | 0.07 | 0.19 | 0.69 ms |
| **47 px** (a sensor blob) | 0.07 | 0.39 | 1.67 | 7.95 ms |
| 156 px (a bird) | 0.92 | 4.38 | 17.10 | 83.30 ms |

`Clone` and `Heal` cost the same to within noise. **Realistic retouching is 0.05–0.4 ms.**

**⚠⚠ TWO THINGS MAKE IT UNLIKE EVERY STAGE PORTED BEFORE IT, AND BOTH CUT THE SAME WAY.**

1. **It is a PREFIX stage.** `PrefixCache` holds its output, so the cost is paid when the **spot
   list** changes — placing or dragging a spot — not on every frame. Every stage ported so far was
   paid per frame, which is where their value came from.
2. **Its work scales with the spots' own disc area, not with the frame** — roughly `2.25·π·r²` per
   spot. The others touch all 6.5 Mpx; this touches a few thousand pixels.

**⚠⚠ AND THE ROUND TRIP COSTS MORE THAN THE WORK.** `GpuBrushRaster` measures a **~1.6 ms floor
whatever the size**, of which M8.3g decomposed **1.28 ms as a fixed download** no size reduction
touches. Against 0.05–0.4 ms of actual work, **a port would very likely make it slower.**

⚠ **Do not re-propose this from the agreed order alone.** The order is structural and spot removal
is genuinely the last unported stage — but "last" is not "worth doing", and that list was written
before anyone priced this one.

⚠ **What is NOT measured, and would reopen it:** the fixture spreads spots so none reads another's
output. Heavily overlapping spots are unmeasured; the algorithm is per-spot so it should scale the
same, but that is reasoning rather than evidence. ⚠ Also unmeasured at **export** resolution, where
the frame is 4× larger — though the disc area scales with the frame, so the ratio should hold.


### ✅ EXPLAINED AND FIXED — a scoped dehaze drag cost 118 ms because the scoped locals did WHOLE-FRAME work (M8.12, 2026-09-17)

**Root cause.** `apply_region` for **noise reduction** and **dehaze** read
`self.apply(src.buf.clone())` and cropped the answer out of it — a full-buffer computation with a
crop bolted on. `run_region_plan` hands its **first** step `RegionSrc::whole(src)`, so a local
stage paid full-frame cost whenever it was first, and **a one-step plan saved nothing at all.**
A drag resumes at the checkpoint immediately before the dragged stage, so one-step plans are the
common case.

⚠⚠ **The proof was a differential the instrumentation made visible: the SAME stage, the SAME
1041x695 output, the SAME 1.00x margin, cost 18 ms as step 1 of a two-step plan and 101-105 ms as
step 0 of a one-step plan.** The only difference is the size of the buffer handed to it. Scoped NR
measured 93-96 ms against whole-frame CPU NR's 92.4-95.0 — the same number, because it was the same
work.

**Fixed** by cropping `src.buf` to `input_rect_for`'s rect before applying. Measured after, same
machine, comparable load:

| | before | after | |
|---|---|---|---|
| NR as step 0 | 93-96 ms | **16.5-18.9** | ~5.2x |
| dehaze as step 0 (1-step plan) | 101-105 ms | **17.1-18.9** | ~5.7x |
| 2-step plan total | 111-114 ms | **34.1-36.9** | ~3.2x |

⚠ The check that says it is really fixed: **dehaze now costs ~18 ms whether it is step 0 or step 1.**
The asymmetry is gone.

**⚠⚠ AND NO TEST COULD HAVE SEEN IT.** `perspective` and `lens_correction` — the gathers — each
carry a region-equivalence test. The two locals carried **none**, because with apply-whole-then-crop
such a test compares a computation with itself and passes for that reason. The journal's own line
*"the two paths agree byte for byte by construction — `apply` IS `apply_region`"* recorded the
tautology as if it were a guarantee. Both stages now have a real one, each with a **sabotage-verified
bound** (NR 4.8e-6 normal against 7.3e-4 with the margin deleted; dehaze 9.9e-5 against 5.2e-2).

⚠ Still open, and untouched by this: **the scoped path is the CPU's** — `gpu_prefix_steps` stays 0
because `accelerate_prefix_stage`'s twins run whole-buffer. A scoped drag now does the right amount
of work, on the wrong processor.

### The two GPU constants nobody had swept — now swept, and one premise is BACKWARDS (recorded 2026-09-16 — MEASURED)

Both probes are committed and re-runnable.

> **✅ RE-RUN ON A QUIET MACHINE 2026-09-17 (load 2.11 at start), AND THE CONTENDED NUMBERS HELD.**
> Both configurations put the whole band above parity at **3,130,000 px** — non-gating 1.42–1.45×,
> gating 1.28–1.65× — and at the shipped 512 Ki floor the GPU is **~2× slower in both**
> (non-gating 0.48–0.70×, gating 0.56–0.60×). ⚠ **So the per-gating premise stays refuted**, and
> the earlier caveat that every crossover was an upper bound turned out not to change the verdict.
>
> ⚠ The probe prints its own load, and it went **2.11 → 7.18 as the sweep began**: the CPU side is
> rayon-parallel, so the sweep loads the machine itself. A truly idle timing run is not available
> here; what matters is that both configurations meet the same conditions.
>
> **⛔ DECIDED 2026-09-17: LEAVE THE FLOOR AT 512 Ki — the user's call, on use.** *"It felt fine
> while using."* ⚠⚠ **This is a decision, not an unexamined gap, and it should not be re-proposed
> from the measurement alone.** The two candidates were 2.2× (1,129,504 px, where the GPU starts
> winning on average but the band still straddles parity) and 6× (3,130,000 px, whole band above).
>
> **What leaving it costs, so the decision can be revisited on evidence rather than re-derived:**
> for a brush box between ~512 Ki and ~1.1 M px the GPU is taken and loses — at 503,004 px, CPU
> **2.5–3.7 ms** against GPU **5.3 ms**, so roughly **2 ms**, and only on a coverage-cache MISS.
> A cache hit pays none of it. That is the magnitude a reporter would have to notice, and did not.
>
> ⚠ Revisit if the coverage cache's hit rate drops, or if a future sweep moves the crossover — not
> because the numbers below say the floor is low, which they will go on saying.

**`MIN_GPU_BOX_PX`** (`crates/gpu-pipeline/src/brush_raster.rs:526`, the **brush coverage
rasteriser** floor — *not* a box-blur floor; the box-blur crossover is `RUNNING_SUM_MIN_RADIUS` in
`box_wgsl.rs:36` and is already swept). Sweep: `crates/app/examples/box_floor_sweep.rs`.

⚠⚠ **THE PREMISE BEHIND "A PER-GATING FLOOR" IS NOT SUPPORTED — IT IS BACKWARDS.**
`brush_raster.rs:526` predicts a gating brush's crossover is *"several times lower"*. Measured,
**gating is worse than non-gating at every box size**, because the GPU uploads the whole **78 MB**
frame base whenever anything gates — a flat **7.4–8.9 ms, independent of box size** — while the
CPU's gating penalty is only ~1.4–2.0×. **Both configurations cross at the same rows**, so there is
no case for splitting the constant in two. Remove that item rather than scheduling it.

| Box | non-gating | gating |
|---|---|---|
| 503,004 px (≈ the shipped 512 Ki floor) | **0.47–0.66×** | **0.43–0.60×** |
| 1,129,504 px | parity | parity |
| 3,130,000 px | 1.04–1.62× | 1.29–1.67× |

So at the shipped floor the GPU is **about 2× slower in both configurations**. Coverage agreed
everywhere (worst 0–1 level, ≤0.004% of samples). The data would support raising 512 Ki by **2–6×**;
no value is proposed until the sweep is re-run idle. ⚠ Two open questions, undecided: a box-*pixel*
floor cannot express a cost that scales with the **frame**, and charging the whole 78 MB upload to
the rasteriser may overstate it, since M8.3j's shared upload amortises that `GpuImage` downstream.

**`MAX_TILE_DABS`** (`crates/gpu-pipeline/src/brush_wgsl.rs:49`, = 1024). Sweep:
`crates/gpu-pipeline/examples/max_tile_dabs_sweep.rs`. **It is two bounds at once** — the length of
the per-tile `var<workgroup>` shared list (overflow → every pixel in that tile re-scans the *whole*
dab list) and `4·CAP + 1032` bytes of workgroup storage, which throttles resident workgroups.

Capacity plateau **512…2048** (56.5–60.5 ms, flat); occupancy cliff at **2560** —
**277.8–292.0 ms, 4.8×**. That **narrows the recorded "between 2048 and 3584" by a rung**. The
mechanism is `⌊32768 B ÷ storage⌋` dropping from **3 resident workgroups to 2**, and 3 already
saturates. ⚠ **This M2 reports 32768 B — twice the `Limits::default()` 16384 the original 3584 was
sized from.**

Below the capacity **there is no knee: the curve is linear**, `ms ≈ 11.0 + 0.1385 × (dabs in the
busiest tile)` over 40/80/164/330/659. The only knee is the overflow wall — 659→1318 dabs/tile costs
**13.5×**, not 2×. **Risk is asymmetric: too small is a 7–13× cliff, too large is flat until 2560.**
At 1318 dabs/tile, 1024 costs **1431–1457 ms against 205.6–206.3 at 1536 — 7.0×.** The shipped 1024
is the lowest value on the plateau, i.e. the least headroom that costs nothing.

⚠ **Blocked on a portability question, not a measurement:** 1024 is exactly the largest capacity
keeping **3 workgroups resident on a 16 KB device** (2048 would leave one), and **no M2 measurement
can settle that** — this project is Linux-first and ships to unknown GPUs.
⚠⚠ **And the evidence base for the constant is gone: the 229-dab brush that justified it no longer
exists — all 40 sidecars in `~/Pictures/fuji` now hold ONE `Luminance` mask between them.** The
330-dab fixture used here is synthetic; sizing real headroom needs a real brush.


### A GPU-side mask coverage cache — worth re-measuring once M8.11 is in use (recorded 2026-09-15, at M8.11 — user's call)

> **2026-09-29:** the serial u8→u32 repack below is ✅ **fixed** (`0064ff2`, 2026-09-17 — a chunked scan);
> the download half is the **u8 coverage wire**, ⏸ **M10.14g**. The cache itself stays declined.

> **✅ MEASURED DIRECTLY 2026-09-16 (`crates/gpu-pipeline/examples/coverage_transfer_probe.rs`).
> THE ~2 ms CEILING BELOW DID NOT HOLD FOR EITHER TRANSFER** — upload **7.9–15.8 ms** (4–8×),
> download **4.0–9.4 ms** (2–5×), together **12–25 ms** against the ~4 implied. That makes the
> transfers **27–58% of the work the cache avoids, not the ~9%** this entry's "order of magnitude"
> claims. M8.3k's rasterise reproduced cleanly (43.1–45.5 ms gating at 3200), so the comparison is
> sound. Two independent methods agreed: parts summed 11.3–15.0, an in-situ `GpuChain::apply`
> differential read 11.8–15.7.
>
> **⚠⚠ AND THE REASON IT MISSED IS THAT NEITHER TRANSFER IS ONLY A TRANSFER — WHICH POINTS AT A
> CHEAPER FIX THAN THIS ENTRY.** The `write_buffer` itself is **0.75–1.16 ms**, exactly where
> M8.3g's borrowed decomposition put it; that figure was never wrong for what it described. The
> cost is elsewhere:
> - `GpuMaskStage::pack` does a **serial per-byte u8→u32 repack** (WGSL has no `u8`) costing
>   **10.6–13.8 ms — ~90% of the upload — on EVERY encode**, not once.
> - The download crosses the wire as **one u32 per sample** (`cov_bytes = w*h*4`, so 27.35 MB
>   rather than 6.84) and is narrowed back to u8 on the CPU. A u8 wire would be **0.55–0.79 ms**,
>   so **85–90% of the download is inflation and narrowing.**
>
> **▶ So most of the 12–25 ms is reachable WITHOUT answering this entry's design question** (whether
> such a cache follows `GpuCache`'s *outlive the render* rule or `PrefixCache`'s opposite one).
> Parallelising or GPU-siding the repack and narrowing the wire to u8 are mechanical. Do those
> first and re-price this afterwards.
>
> **⚠ A region render CLIPS the coverage map** (`crates/pipeline/src/stages/mask.rs` ~line 344), so
> the 6.5 MB whole-frame plane priced below occurs at **export**, not in normal 1:1 use. Interactive
> rungs are smaller: fit/1600 pair **3.2–6.9 ms**, a real brush box (1510×2019) **6.2–10.9 ms**.
> Export at 6246 with no region is **~63–140 ms per photo**.
>
> ⚠ Load was 4.5–5.1 during the run (three sibling agents building), and the CPU-bound rows are the
> contended ones — **read the bottoms of the bands**; the verdict holds on those alone.


**What it is.** M8.11 puts a mask's coverage and blend on the GPU but **keeps the existing CPU-side
coverage cache**, so a cache hit *uploads* the stored `CoverageMap` and a miss rasterises on the GPU
and *downloads* once to deposit. Keeping the plane resident between frames instead would remove both
transfers.

**What it is worth, as far as is currently known.** At rung 3200 a coverage plane is **6.5 MB as
`u8`**, which M8.3g's decomposition puts at **~2 ms**. So the ceiling is ~2 ms per transfer, against
a **31.8–43.5 ms** GPU rasterise (M8.3k) that the cache already avoids. ⚠ That is a *ceiling from an
adjacent measurement*, not a measurement of this — and composing component measurements has missed in
this crate **five times** now (M8.3g by ~2×, M8.3f by 8× in attribution, M8.3o by ~18%, M8.10 by
~12%). **Measure it before scoping it.**

**⚠ Why it was NOT folded into M8.11.** It is a caching question, and folding one into a porting
milestone is how this project's two worst cache bugs happened — the prefix cache evicting the
checkpoint a drag resumes from, and the mask tint caching a *screen rect* so a hit survived a pan.
Both were invisible in a still frame and both shipped.

**⚠ What it needs, so nobody picks it up as a small job.** `GpuCache`'s stated rule is *outlive the
render*, and this cache's rule is the opposite of `PrefixCache`'s about regions — so the two
existing caching rules in this crate disagree deliberately and a third must say which it follows. It
also needs the deposit/lookup seam, which today lives in `render_engine` and keys on
`MaskCoverageKey`, to reach into the GPU context — which is precisely the coupling M8.3l's
`SharedPlane` was shaped to avoid. ⚠⚠ And the failure mode is the dangerous one: a stale coverage
plane renders a **plausible photograph with the wrong mask**, not an error.

### ✅ FIXED 2026-09-17 — the dehaze airlight is now invariant to geometry, on both paths

> **⚠⚠ AND THIS ENTRY'S CENTRAL CLAIM WAS WRONG, IN ONE WORD.** It said below that M8.7's replay
> airlight *"does not move during a drag at all"*. **Deterministic is not invariant.** It is a
> deterministic function of the **edits**, and a Straighten drag changes an edit on every frame —
> `prefix_airlight` replayed the prefix **including geometry**, so the angle fed straight into it.
> Measured across a 0–6° drag: **6.06–6.81 levels** on `DSCF6291`, **1.47–1.71** on `DSCF6296`,
> 0.03–0.08 on `DSCF6509`. M8.7 took the wobble from ~21 levels to ~6. It did not remove it.
>
> ⚠ That sentence is what sent the 2026-09-17 investigation looking on the wrong path for an hour.
>
> **Three things were wrong, and each was found by a different means:**
>
> 1. **The global replay ran geometry.** `prefix_airlight` now **skips the resampling family**
>    (`LensCorrection`, `Perspective`, `Geometry`); spot removal and noise reduction stay, since
>    neither reframes. Measured after: **0.00 levels across ±40°.**
> 2. **A mask's capture half got no airlight at all** — `capture_stages` called `DehazeStage::new`,
>    so it estimated per frame and moved **9.4–12.1 levels**, worse than the global path. It is now
>    handed the same value.
> 3. **⚠⚠ The GPU dehaze estimated its own, and THAT is what the user actually saw.**
>    `GpuDehazeStage::new` called `pipeline::airlight(image)` on the frame the seam handed over —
>    the **rotated** one. Reported by use: *"it does not breathe for the masked one, but it does
>    slightly for the global one, especially for straighten values above 25 or below -25."* Every
>    clause of that report named a mechanism: masked stages run in the **prefix** and `accelerate`
>    is only called on the **tail**, so the masked one was already fixed; bigger angles mean bigger
>    empty corners; and it re-estimates on every construction, so it moves during a drag *and*
>    between settled positions.
>
> ⚠ **Three probes were written against the CPU path before anyone asked which path was rendering
> the screen.** All three found nothing, correctly — on `DSCF6227` the CPU counterfactual moves
> 0.08–0.44 levels. **Ask which path renders before measuring one.**
>
> ⚠ **The trade, permanent:** a crop no longer re-estimates the airlight. Hazy sky and clear
> foreground share one value. That *is* invariance, and a crop drag is continuous too.

### ⚠ The original entry, kept because its measurements are still the record (recorded 2026-09-14, at M8.7 — MEASURED)

**What it is.** `dehaze::airlight` takes the mean of the haziest 0.1% of pixels. That is an **order
statistic over the frame**, so it moves whenever the frame's content moves — and a Straighten drag
moves the frame's content on every single frame.

**Measured on DSCF6291.RAF while building M8.7's airlight parameterisation**: the *true* airlight —
what today's code computes, estimated from the stage's own input — moves **21 of 255 levels** across
an ordinary Straighten drag. So **today's shipped app already wobbles during that gesture**, before
any of M8.7's work, and nobody has reported it.

**⚠⚠ This is not M8.7's doing and M8.7 did not make it worse — it made it smaller.** The replay
airlight M8.7 ships is a deterministic function of the edits, ~~so it does not move during a drag at
all~~ — **struck through 2026-09-17: a drag changes an edit every frame, and the replay ran
geometry, so it moved 6.06–6.81 levels on `DSCF6291`** — what it costs is a **5.6-level** one-off shift against what the old code computed, measured and
accepted at the time. The three candidates measured then:

| definition | worst shift against today | stable during a drag? |
|---|---|---|
| **replay from the source, 256 px** (shipped) | **5.6 levels** | yes |
| held from the first frame of the gesture | 21 | no — it *is* the instability, frozen |
| estimated from the source buffer | 95 | no, and worse |

**⚠ The source-buffer candidate is dead** and should not be revisited: a mild 0.1–0.9 crop moves
DSCF6291's true airlight by **65 levels**, because cropping changes which pixels are in the top 0.1%.

**Why it is worth an entry.** The instability belongs to the **estimator**, not to any milestone
that consumes it, and the fix is a different question from anything M8.7 answered: a percentile that
does not move when the frame is re-framed. Candidates worth measuring rather than arguing about —
estimate on the pre-geometry buffer (stable under straighten and crop by construction, but then it
describes pixels the photograph no longer contains); a spatially-weighted statistic; or simply a
larger sample where the 0.1% is less sensitive to which pixels are present.

**⚠ Not urgent.** It has shipped unnoticed for the life of the feature, and M8.7 made the drag case
strictly better. It is recorded so that a later report of *"the haze changes as I straighten"* is
met with a measurement rather than a search.

### 🟡 A GPU buffer resident across the prefix→tail seam — UPLOAD HALF DONE (M8.14, 2026-09-18), DOWNLOAD HALF DECLINED

> ⚠⚠ **CORRECTION 2026-09-26: the step→tail half did NOT work on the byte path — the ordinary preview
> frame — from M8.14 until 2026-09-26.** `accelerate`'s arm that hands back the `output_transform`
> chain never called `mark_shared`, so the tail re-uploaded after every GPU prefix step
> (`took=false` in `RUSTROOM_DEBUG_XFER`). The M8.14 figure below was real for the path its probe
> took, which was not the one a user sees. Fixed with `the_handed_back_chain_takes_the_prefix_plane`
> (63 → 48.7 ms on a Straighten frame at rung 3200). ⚠ And **the download half is now BUILT for the
> LAST prefix step** (the `gpu-geometry` branch, the author's call as "scope C"): one resident slot
> and a no-download handoff, with the warm-behind filling the final checkpoint. That is not the
> general `PrefixCache`-of-`GpuImage` priced below. `grep -n 'GPU geometry' docs/JOURNAL.md`.
>
> **✅ THE UPLOAD HALF SHIPPED 2026-09-18 as M8.14.** Consecutive GPU prefix steps, and the last one
> and the tail's first chain, hand the device buffer over. Measured at rung 6246 on a quiet machine:
> step→step **14.8–18.8 ms**, step→tail **28.5–30.6**, together **43.3–49.4 ms ≈ 5.6–6.3%** of a
> ~790 ms whole-frame render. `crates/app/examples/probe_prefix_tail_seam.rs`.
>
> **⛔ THE DOWNLOAD HALF WAS PRICED AND DECLINED (user's call, 2026-09-18).** The two `tovec` costs
> are **25.0–29.5 ms each** — the larger half — and removing them means `PrefixCache` holding a
> `GpuImage`: `MID_BUDGET_BYTES` would start describing VRAM, and the region crop, the brush cache's
> sizing and `render_cached`'s tail seeding would each need an answer. Priced at **11–13%**.
>
> ⚠⚠ **This entry's own warning turned out to be right, and the numbers below describe a path the
> gesture does not take.** `render_engine.rs` keeps `gpu_prefix_steps` at 0 on the region-scoped
> path, so an interactive drag has **no GPU prefix step and therefore no seam**. Everything here is
> an export / cold-whole-frame win. Do not requote the 88.9 ms dehaze-drag frame as a drag saving.
>
> ⚠ Original text follows, kept as the record of what was believed before M8.14 measured it.

### ⚠⚠ A GPU buffer resident across the prefix→tail seam (recorded 2026-09-13, at M8.3o's merge — MEASURED, not estimated)

> **✅ CORROBORATED IN AN ADJACENT PLACE, M8.13 (2026-09-18).** Not this seam, but the same class of
> win one stage over: a `GpuChain` before a mask now hands its output buffer straight to the gate
> instead of the gate re-uploading the downloaded plane. Measured against the same build with the
> take forced to miss — host memcpy **2.23–2.86 ms → 0.00**, and the `gpu=` column **10.0–19.1 →
> 5.1–6.8 ms**, because the 78.1 MB device-side copy goes with it. ⚠⚠ **The device half was about
> as large as the host half and the spec had not priced it** — worth remembering when this entry
> gets scoped, since it prices a round trip the same way.
>
> **✅ RE-MEASURED 2026-09-16 through the real engine** (`RenderEngine` + `RUSTROOM_DEBUG_RENDER`,
> `crates/app/examples/bench_nr_checkpoint.rs`), now that both NR and dehaze are ported — the
> condition M8.3o named. **The ~26 ms below is CONFIRMED**: an independent zero-work round trip at
> the same resolution measured **26.5–26.8 ms** (7.5–7.6 at 1.7 Mpx).
>
> **⚠⚠ THAT "SAVES ~0 UNDER A VIEWPORT" FINDING IS STALE — M8.12 OVERTURNED IT.** It was measured
> when an NR drag read **118.3–125.3 ms** against a dehaze drag's **118.3–122.0**, indistinguishable
> — because *both* were doing whole-frame work, which is the defect M8.12 fixed. Re-measured on a
> quiet machine 2026-09-17: a 2-step scoped plan is **30.5–35.6 ms** and a 1-step **15.3–15.9**, so
> **the checkpoint saves ~15–20 ms on the scoped path.** It went from worthless to worth having,
> and the reason is that the first step stopped doing the whole frame.
>
> ⚠ Original text follows, kept because its numbers are the record of the pre-M8.12 state. ⚠ **This is M8.6's retraction shape**: the
> saving below is real, but it belongs to whole-frame renders rather than to the 1:1 editing the
> acceleration exists for. **Price this as an EXPORT-side win, or measure the path the gesture
> actually takes first.**
>
> **The post-NR checkpoint, priced both ways at 6.5 Mpx** (1.7 Mpx in brackets): it **saves**
> 64–71 ms on the GPU path and 95–97 on the CPU path per downstream frame [21–23 / 25], and **costs**
> **23.5–23.7 ms** to write on the GPU path [6.8–6.9] and *nothing* on the CPU path. ⚠ The download
> half alone (~12 ms) is an **inference, not a measurement** — isolating it needs a `gpu-pipeline`
> change.
>
> **The asymmetry, which is the thing to decide on:** dragging **NR** pays 23.5–23.7 ms (13% of the
> frame) and saves 0; dragging **dehaze** pays 0 and saves 64–71; dragging a **tail** slider pays 0
> and saves 0, because the final checkpoint serves that. As a trade, fusing would save an NR drag
> 23.5–23.7 ms and cost a dehaze drag 34.1–36.7 — **arithmetic break-even ≈ 1.5 NR drags per dehaze
> drag.** ⚠⚠ Which side of that the user is on is **his call**, and the catalogue's 40 sidecars must
> not be used to answer it.
>
> ⚠ 4 of 7 runs were unusable — sibling build agents made GPU steps come back 20–40× slow. GPU bands
> are over the two cleanest runs' minima, CPU bands over 65 stable frames.
>
> **✅ RE-MEASURED CLEAN 2026-09-17** (load 2.11, `DSCF6296`), what the checkpoint saves per
> downstream frame: rung 1600 **23.6 ms** CPU-tail / **17.8** GPU-tail; rung 3200 **95.1** /
> **57.8**. ⚠ The CPU figure matches the contended run exactly (95–97); the GPU one is lower than
> the 64–71 reported then, which is what 4 unusable runs buys you.


**What it is.** A dehaze drag at a native rung renders in **88.9 ms with both the prefix step and
the tail accelerated**, against 128.4 all-CPU. Of that 88.9, roughly **26 ms is a zero-work GPU
round trip, paid TWICE**: the prefix step downloads its result so `PrefixCache` can hold it as an
`ImageBuf` checkpoint, and the tail's chain uploads the same pixels again on the next line.
Keeping the buffer device-side across that seam is worth **~25–30 ms of the 88.9**, taking the
frame to ~60 ms (17 fps against 11).

**⚠⚠ And during a drag that checkpoint is written, used once, and invalidated by the very next
frame.** `PrefixStepKey::Dehaze` holds the dehaze parameters, so moving the slider changes the key
— the download is pure waste on exactly the interaction the acceleration exists for. It earns its
place only when you *stop* dragging and move some other slider.

**Why it was not taken at M8.3o.** The seam that milestone built hands back a
`Box<dyn PipelineStage>`, whose `apply(ImageBuf) -> ImageBuf` **forces the round trip by its
signature** — the same observation that made `GpuStage` deliberately not a `PipelineStage` at
M8.3b. Closing it is not a tweak to `accelerate_prefix_stage`; it is a change to what a prefix
checkpoint *is*.

**⚠ Two shapes, and the second is the one to think about first.**
- **Fuse the last prefix step into the tail's chain.** Cheapest, and it **destroys the
  checkpoint**, so a drag of anything *downstream* then re-runs dehaze. M8.3o priced exactly that
  trade and chose per-step; this would reverse it without the measurement changing.
- **`PrefixCache` learns to hold a `GpuImage`.** The honest one. A checkpoint becomes "CPU buffer
  **or** resident device buffer", `MID_BUDGET_BYTES` starts describing VRAM as well as RAM, and
  every consumer that reads a checkpoint as an `ImageBuf` (the region crop, the brush cache's
  sizing, `render_cached`'s tail seeding) needs an answer. ⚠ It is also what would let **two
  consecutive prefix steps** share one upload — the case that appears the moment noise reduction
  is ported and sits directly above dehaze.

**⚠ Do not start it before the prefix is mostly ported.** With one GPU prefix stage the win is one
seam; with lens correction, geometry, NR and dehaze all on the GPU it is the difference between
one round trip per frame and four. The measurement above is the floor, not the ceiling.


### ✅ FIXED 2026-09-07 — "dragging a masked exposure stutters", which was never the mask

**✅ FIXED by the interactive rung** — `docs/superpowers/specs/2026-09-07-interactive-rung-design.md`.
A live gesture drops a render ladder rung and the release restores full quality: **137.9 → ~35 ms,
7.2 → 28 fps**. ⚠⚠ **Nothing in this entry's original framing survived**: the reporter closed it
with *"the global exposure actually exhibits the same behavior"*, and `MaskStage` — its clone, its
coverage, its blend and the bounding box deferred here for the sixth time — was never involved.
**The cause was the render rung.** The rounds below are kept because the *method* is the reusable
part, and because two of the findings are still open work.

**✅ THE FRAME-TIME VARIANCE — CLOSED 2026-09-07 (user's call), ACCEPTED RATHER THAN FIXED.**

⚠⚠ **CORRECTION TO THIS ENTRY'S OWN EARLIER NUMBER: "27.0 → 268.4 ms" IS A RANGE STATISTIC DRIVEN
BY ONE FRAME, AND QUOTING IT OVERSTATED THE PROBLEM.** The real distribution over 357 rung-1600
drag frames is **median 33.4 ms, p75 41.7, p90 56.1** — 92% fall between 27 and 60 ms, 9 frames
exceed 100, and exactly **one** reached 268. ⚠ This project already records *"a whole-image mean is
the wrong statistic"* twice; `max` is worse, being defined entirely by the single most extreme
sample.

**Split by position in the session, with the two rungs separated** (mixing them is its own
confound — the last decile's alarming 93.6 ms mean was 18 legitimate *full-rung release* frames):

| decile | n | mean | frames > 60 ms |
|---|---|---|---|
| 1–2 (session start) | 76 | 52.3 / 48.4 ms | 12 |
| **3–6 (quiet)** | **156** | **30.1–32.2 ms** | **0** |
| 7–9 (zooming, mode switches) | 104 | 41.9–54.2 ms | 15 |

⚠⚠ **156 CONSECUTIVE DRAG FRAMES AT 30–32 ms WITH ZERO OUTLIERS.** The render path is steady at
~32 fps when nothing else is running, so there is nothing to optimise *in it*. The variance is
bursty and has two identified sources:

1. **Background work contending for cores.** Frames 2–3 of the session render at rung **3123** —
   the *half-size* buffer's top rung — so the full-resolution upgrade had not landed: ~3 s of
   rayon-parallel libraw decode, plus `ThumbLoader`'s 6 workers. ⚠ `CLAUDE.md` predicted this in
   words when that loader was parallelised: *"a loader that saturated every core would make editing
   sluggish for as long as a folder takes to load."* The mitigation chosen then — `cores − 1`,
   capped at 6 — reduces the contention without removing it.
2. **Rung transitions**, measured at **103.8 ms mean against 48.1 ms for non-transition frames**,
   with 67% of them over 60 ms. `prefix_ran` is 0 on both, so no extra pipeline stage runs — the
   same work simply takes longer, which points at allocator and page-fault cost when the buffer
   size alternates rather than at anything in the pipeline.

⚠ **What would be needed to go further, stated because the current instrument cannot:
`RUSTROOM_DEBUG_RENDER` logs no timestamps**, so "front-loaded" above is inferred from frame index
rather than measured against wall-clock, and nothing correlates a slow frame with what the decode
worker or thumbnail loader were doing at that moment. **Add a timestamp and an in-flight count
before theorising further.**

⚠⚠ **CLOSED ON THE JUDGEMENT, NOT ON A FIX — and the judgement is the right kind.** The reporter's
verdict after the interactive rung landed was *"much better now, no stuttering anymore"*, **with
these bursts still present**. They coincide with a folder opening, which is when nobody is yet
editing, and the one mechanism that could be attacked — `ThumbLoader`'s worker count — was already
tuned down to `cores − 1` for exactly this reason.

⚠ **Reopen it only on a fresh report, and instrument before changing anything.** The evidence that
would justify reopening is a slow frame *outside* a folder-open burst, which the quiet 156-frame
run says does not currently happen. **Add timestamps to `RUSTROOM_DEBUG_RENDER` first** — without
them nothing correlates a spike with what the decode worker or thumbnail loader were doing. A uniform 138 ms reads as lag; that
spread reads as stutter, which is the word actually used in the report. Undiagnosed, and distinct
from the mean this fix addressed.

**▶ ALSO OPEN — RANGE-MASK COVERAGE COST. DECOMPOSED 2026-09-07; NOT OPTIMISED (user's call).**
`cargo run --release -p app --example bench_range_mask <raw>` is kept. ⚠ Both its kernels are
cross-checked **bit-identical** — one against `MaskStage`, one against `ColourRange::coverage_at` —
so the ablation cannot silently describe something that no longer ships, which is what happened to
`bench_hsl` once.

| step | ms above floor | share |
|---|---|---|
| **Oklab round trip (3 `cbrt`)** | 5.89 | **42%** |
| `hypot` (chroma) | +1.00 | 7% |
| **`atan2` (hue)** | +3.64 | **26%** |
| `coverage_at` (band + gate) | +3.56 | 25% |
| — *of which `fmodf` + hoisting `span`/`w`* | *0.93* | *6.6%* |
| **total coverage, 1600 px** | **14.09** | |

For contrast a **luminance** range is **4.49 ms** above the same floor, of which **1.42 is
`linear_to_srgb`'s `powf`** — `lightness()` is about a third the cost of `hue_chroma()`.

⚠⚠ **TWO CLAIMS THIS ENTRY PREVIOUSLY MADE WERE WRONG, AND BOTH WERE MINE.**

1. **"During a tail-only drag the base pixel *is* the cached prefix output."** **False.** The masks
   loop runs **after the entire global tail** — white balance, exposure, contrast, tone regions,
   tone curve, local contrast, saturation, HSL, colour grading. So a range mask's `base` is the
   *global tail's output* and moves whenever any of those change. Cacheable across a drag of the
   mask's **own** sliders; invalidated every frame by a **global** slider.
2. **"`BrushCache` is the precedent."** Much weaker than it reads. A brush's coverage is a pure
   function of `(strokes, geometry, frame dims)` — **positional**, which is exactly why its cache is
   easy. A range mask's is a function of **pixel values at that point in the pipeline**, so its key
   must cover everything upstream of it in the tail.

⚠⚠ **AND THE `fmodf` LEAD DID NOT PAN OUT — 6.6%, NOT THE 42% THE HUE LUT FOUND IN `HslStage`.**
`coverage_at` really does call `rem_euclid` twice per pixel and really does recompute `span` and `w`
from constants, which is the identical shape to the hue LUT's largest item. **It is worth 0.93 ms.**
Recorded because the hypothesis was stated confidently one message before it was measured: **the
same code shape can carry completely different weight in two stages.**

**⚠⚠ 75.3% OF PIXELS ON A REAL PHOTOGRAPH SIT BELOW `min_chroma` AND PRODUCE COVERAGE EXACTLY 0** —
three quarters of the 14.09 ms produces nothing.

⚠⚠ **BUT A CHEAP PRE-GATE ON RAW RGB SPREAD CANNOT BE MADE SOUND, AND THAT IS PROVABLE RATHER THAN
EMPIRICAL.** Scale a pixel by *s*: `l`, `m`, `s` scale by *s*, their cube roots by *s*^(1/3), so the
Oklab chroma scales by **s^(1/3)** while the RGB spread scales by **s**. The ratio diverges as the
pixel approaches black, so **no finite `K` satisfies `chroma ≤ K · spread`**. The bench's measured
worst ratio of **16.42** is merely this image's darkest coloured pixel, and a naive `spread < 0.01`
gate got **28,769 pixels wrong**. ⚠ A gate on spread *and* brightness together might be sound; it
was not pursued.

**Two candidate approaches for whoever picks this up, neither built:**

- **Approximate the coverage path** — a fast `cbrt` plus a pseudo-angle replacing `atan2`, targeting
  **68%** of the cost. ⚠⚠ **The accuracy argument is much weaker here than the one `CLAUDE.md`
  records for deferring an approximate `cbrt` globally, and that is the whole point:** there the
  concern is every Oklab consumer's **output pixels**; here the value feeds a **smoothstep gate**,
  so an error invisible in the gate is acceptable where the same error in `HslStage`'s output is
  not. Helps every frame, global drags included; needs no cache and no invalidation rule.
- **Cache the coverage** — removes all 14.09 ms, but only while the base holds still (see
  correction 1), and costs a full-frame buffer per mask plus that invalidation key.
  ⚠⚠ **This option now has a worked precedent: the auto-mask coverage cache, 2026-09-09**
  (`specs/2026-09-09-auto-mask-coverage-cache-design.md`). It answers the invalidation key this
  entry leaves open — **`Edits` truncated to the masks *before* this one, plus the rung** — which
  is sound because everything upstream of the base is in it, and whose truncation is what makes it
  worth having: dropping this mask drops its own adjustments, so moving its slider does not
  invalidate. ⚠ It also settled the structural half that applies here identically: the cache can
  never **build** the coverage, because only `MaskStage::apply` holds the image, so the stage
  reports what it built through `BrushCoverage`'s write side and the app keys and stores it.
  ⚠ And it confirms correction 1's consequence in use: a **global** slider drag is not helped and
  cannot be.

⚠ **Stopped here deliberately (user's call, 2026-09-07): the measurement was the point.** 14 ms on a
~35 ms frame is real but no longer urgent now the interactive rung has taken the reported drag from
7 to 25 fps.

---

**▶ AGREED TO FIX (user's call, 2026-09-06).** ✅ **The measurement is done and committed** —
`cb96916`, a masked-exposure row plus the three other mask kinds and a decomposition of
`MaskStage::apply`. The first commit was a number, as this entry demanded.

**Reported in use**, and reported carefully: *"the exposure dragging does stutter a bit, but it also
did before, and without a masked dehaze, if I am being honest."*

**Measured** — X-T4 `DSCF6223.RAF`, buffer 3123x2085, fit at 1600 px, best of 5:

| case | fit total | vs `neutral` | **vs `global exposure`** |
|---|---|---|---|
| neutral | 12.7 ms | — | — |
| global exposure (no mask) | 14.4 ms | +1.6 | — |
| 1 masked exposure, gradient | 17.1 ms | +4.4 | **+2.7** |
| 1 masked exposure, small radial | 17.6 ms | +4.9 | **+3.2** |
| 1 masked exposure, luminance range | 20.7 ms | +7.9 | **+6.3** |
| 1 masked exposure, colour range | 31.3 ms | +18.6 | **+16.9** |
| 3 masked exposure, gradient | 25.2 ms | +12.5 | +10.8 (3.6 each) |

Tail-only on a warm prefix at 1400x900 — **what a drag frame actually costs when zoomed** — says
the same thing louder: neutral 8.7 ms, gradient **10.9**, luminance range **13.4**, colour range
**22.3**.

**Decomposition of `MaskStage::apply`** (region 1400x900, gradient):
`clone 0.4 ms | exposure 0.8 ms | blend 1.8 ms | sum 3.0 ms`.

⚠⚠ **THE CLONE — THIS ENTRY'S OWN HEADLINE SUSPECT — IS 13% OF THE STAGE.** It is real, it is
`0.4 ms`, and it is not the problem. The **blend** pass is 60%, and it costs **more than twice the
adjustment it is blending**. That is the fifth consecutive time cost in this area has been reasoned
wrong and measured right, and the wrong reasoning was written **in this entry**, one bullet from the
instruction not to trust it.

⚠⚠ **THE VARIABLE THAT MATTERS IS THE MASK KIND, AND IT SPANS 6x.** Identical adjustment, identical
pixels, identical everything else: a gradient adds **+2.7 ms**, a colour range adds **+16.9 ms**.
The cost is `hue_chroma()` — `prophoto_to_oklab` (three `cbrt`), `atan2`, `hypot` — evaluated on
**every pixel of the frame** to decide coverage, ahead of a one-multiply adjustment. **The coverage
evaluation is the cost.** Neither the clone nor the adjustment nor the blend arithmetic explains
any of the spread.

⚠ **So the diagnosis depends on which mask kind was being dragged, and the two answers are
different pieces of work:**
- **A range mask (luminance or colour)** — the numbers explain the report outright, and there is a
  fix with a precedent: **a range mask's coverage is a pure function of the `base` pixel**, and
  during a tail-only drag the base pixel *is the cached prefix output*, which does not change
  between drag frames. Coverage could be rasterized once per (mask, prefix output) and reused, the
  way `BrushCache` already does it for a brush. ⚠ The invalidation rule is the whole design
  question — see `CoverageSource`'s note that a range mask is keyed on `base` precisely so it does
  not chase its own effect.
- **A gradient or radial** — **+2.7 ms is not a stutter**, and the cause is then not in this stage
  at all. The investigation would be scoped wrong and should restart from what else runs per frame.

**Verified rather than assumed, while measuring:**

- ✅ **A masked-exposure drag provably never touches the prefix cache.** `build_prefix_steps`
  `continue`s unless the projection's NR or dehaze is active, so no `MaskedCapture` step exists for
  an exposure-only mask. This entry asserted it; it is now checked.
- ✅ **Drag requests coalesce.** The worker drains `req_rx` to the newest request and `poll_latest`
  drops all but the newest frame, so nothing queues up and the stutter is genuine per-frame cost,
  not accumulated lag.

⚠ **The bounding box is innocent for the SEVENTH time, and now on stronger evidence than before.**
Gradient **17.1** against small radial **17.6** — indistinguishable, on the one mask shape a bbox
could ever shrink. And for *both* range masks a bbox is meaningless **by construction**: a tonal or
hue band has no bounding box.

---

#### ⚠⚠ ROUND 2 (2026-09-07): THE REPORTER'S MASK WAS A GRADIENT/RADIAL, AND THAT EXONERATES THE RENDER PIPELINE

**Asked and answered:** the stutter was seen with a **linear gradient / radial** mask — the +2.7 ms
case, not the +16.9 ms one. So the colour-range finding above is real and worth fixing on its own
merits, but **it is not this bug**, and the coverage cache is not this bug's fix.

**A drag frame at fit priced end to end** (1600x1068, warm prefix — `tail + to_rgba8 + histogram`,
which is everything `render_cached` and `pack` do per delivered frame):

| case | tail | pack | hist | **total** |
|---|---|---|---|---|
| neutral | 8.1 | 0.4 | 1.5 | **10.0 ms** |
| global exposure | 9.4 | 0.4 | 1.5 | **11.3 ms** |
| masked exposure, gradient | 12.2 | 0.4 | 1.5 | **14.1 ms** |
| masked exposure, small radial | 14.2 | 0.4 | 1.5 | **16.2 ms** |

⚠⚠ **14.1 ms IS 71 FPS, AND 71 FPS DOES NOT STUTTER. The render pipeline does not explain the
report.** The mask adds 2.8 ms to a frame that was already 11.3 ms without it — which is exactly
what the reporter said from the other direction: *"it also did before"*. Two independent
measurements agree on the mask's share (+2.7 via `render_once`, +2.8 via the warm-prefix tail), so
this is not a measurement artefact.

⚠ **And the pack was the other plausible suspect; it is 0.4 ms.** The histogram is 1.5 ms. Neither
is worth touching.

**⚠⚠ SO THE SEARCH MOVED TO THE WRONG THREAD, AND THAT IS THE FINDING.** Rendering happens on a
**background worker**. A slow background render cannot jank the UI — it can only make the preview
*lag behind the slider*, which is a different complaint from a stutter. A stutter is the **UI
thread** missing frames, and nothing measured so far runs on the UI thread.

**▶ NEXT MEASUREMENT: UI-thread frame times during a drag, in the running app.** Not pipeline
throughput — that question is now answered. The two candidates, both on the UI thread and neither
yet measured:

1. **Rayon starving the UI thread.** The render saturates every core; the UI thread then competes
   with it for a core each frame. This predicts exactly the observed dose-response — *more* render
   work (a mask, a bigger image) means *more* starvation — and it predicts a stutter rather than
   simple lag, which is the detail the throughput numbers cannot account for.
2. **The per-frame texture handover.** `shell.rs` builds an `egui::ColorImage` from `frame.rgba`
   (~6.8 MB allocated and converted at the fit rung) and calls `ctx.load_texture` for every
   delivered frame, both on the UI thread.

⚠ **Checked and NOT implicated, so nobody re-checks them:** the history push and the sidecar write
are gated on `ed.settled`, not `ed.changed`, so a drag writes neither; render requests **coalesce**
(the worker drains `req_rx` to the newest, `poll_latest` keeps only the newest frame), so nothing
queues up and the stutter is not accumulated lag; the full-resolution upgrade is a one-off gated on
an 800 ms dwell, not per-frame work.

⚠ **A brush row was added and is +19.0 ms — but UNCACHED**, and must not be read as a drag frame:
the bench builds without a prebuilt coverage map, so it rasterizes per call where the app's
`BrushCache` rasterizes once. It prices what that cache saves, which is most of a brush.

---

#### ✅ ROUND 4 (2026-09-07): MEASURED IN THE RUNNING APP — 7.2 FPS AT RUNG 3200, AND THE MASK IS NOT INVOLVED AT ALL

**`RUSTROOM_DEBUG_RENDER=1`, 167 real drag frames on the user's own display and photograph:**

| rung | frame | region | frames | min | mean | max | fps |
|---|---|---|---|---|---|---|---|
| 1600 (fit) | 1.7 Mpx | no | 2 | 33.6 | **35.2** | 36.9 | **28** |
| **3200** | **6.8 Mpx** | **no** | **131** | **94.9** | **137.9** | **242.3** | **7.2** |
| 6246 | 6.2 Mpx | yes | 29 | 93.2 | 116.9 | 201.8 | 8.6 |

⚠⚠ **`prefix_ran 0` ON ALL 167 FRAMES.** The prefix cache is perfect and this is entirely tail +
pack. Every earlier suspicion about the prefix, the projection and the capture corrections is
closed by that one column.

⚠⚠ **AND THE REPORTER SETTLED IT INDEPENDENTLY: A *GLOBAL* EXPOSURE DRAG BEHAVES IDENTICALLY.**
So `MaskStage` is not involved — not its clone, not its coverage, not its blend, and not the
bounding box. **The whole entry's original framing was wrong, and it was wrong from the title
down.** What is left is the rung.

**ROOT CAUSE, CONFIRMED ON THE RUNNING APP: a photo that *looks* fitted is rendered whole-image at
a NATIVE rung.** `fit_cap` is 1600, but `tier_max_dim` climbs a doubling ladder as soon as the
viewport exceeds the fit texture — and on a large/HiDPI window that lands on **3200**, whole-image,
**6.8 Mpx against the fit rung's 1.7**. Four times the pixels, four times the time, on every drag
frame. The measured ratio (137.9 / 35.2 = **3.9×**) matches the pixel ratio (4.0×) almost exactly,
which is what confirms it is the rung and not something incidental to it.

⚠ **The bench understated this by ~2.6×** — it predicted 53 ms at 6.5 Mpx where the app measures
137.9 at 6.8. The bench renders a near-neutral photo; the user's tail carries the real stack (the
camera rendering, their adjustments, a mask). ⚠ **A synthetic bench prices a stage, not a session** —
read `bench_masked` for deltas between rows, never as a prediction of a drag frame.

⚠ **The variance is the "stutter" as distinct from "slow"**: 94.9 → 242.3 ms at one rung on one
photo. A uniform 138 ms would read as lag; a 2.5× spread reads as stutter. Not yet diagnosed, and
secondary to a mean of 7.2 fps.

**▶⚠⚠ THE FIX IS AN INTERACTIVE RUNG, AND THIS PROJECT HAS BUILT IT BEFORE AND REMOVED IT — READ
the "progressive fit resolution" SECTION IN `docs/JOURNAL.md` BEFORE RE-DERIVING ANY OF IT.** That feature
rendered at 1600 while interacting and re-rendered at the higher rung ~200 ms after; it was shipped,
debugged three times and deleted on 2026-08-13, and **its three bugs are recorded there** —
an oscillating gate (the condition went false the frame the sharp render landed), sharpening the
wrong buffer, and a gate made unreachable by the clamp fix.

⚠⚠ **AND ITS REMOVAL REASON IS AN ARGUMENT *FOR* DOING THIS, NOT AGAINST IT — the inversion is the
finding.** It was removed because *"the user could not see it on or off"*: the question then was
**"is the sharper render visible?"** (no). The question now is **"is the slower render felt?"**
(yes — 7.2 fps, reported twice). If a higher rung is not visibly sharper in a still frame, then
dropping to the fit rung *during a drag* costs nothing perceptually and buys 4× the frame rate.
⚠ Stated honestly: that "could not see it" was measured at a **1.44× residual**, and 1600 → 3200 is
**2×**, so it is suggestive rather than conclusive at this ratio — and this project's rule is that a
judgement about a photograph gets settled on a photograph.

---

#### ⚠⚠ ROUND 3 (2026-09-07): IT IS THE RUNG, AND EVERY NUMBER ABOVE WAS BLIND TO IT

**The symptom was pinned down first, and it is not a stutter.** Asked directly, the reporter says
the **UI stays smooth and the preview trails the slider and catches up**. That rules out both
UI-thread candidates round 2 proposed — rayon starvation and the texture handover would both make
the *slider* jerk. It is background-render **latency**, so the thing to measure is what one render
costs at the size the app actually renders.

**And the size is the variable nothing had measured.** `fit_cap` is `min(buffer_long, 1600)`, but
`tier_max_dim` climbs a **doubling ladder** as soon as the viewport is larger than the fit texture
— and `is_native` then makes it a **region render covering the whole frame**. On a large or HiDPI
window a photo that *looks* fitted is not being rendered at 1600 at all.

| rung | frame | neutral | global exposure | masked exposure (gradient) |
|---|---|---|---|---|
| 1600 (fit) | 1600x1068, 1.7 Mpx | 13.0 ms | 11.0 ms | **14.3 ms** |
| 3123 (native) | 3123x2085, **6.5 Mpx** | **38.0 ms** | **41.9 ms** | **53.2 ms** |

⚠⚠ **53 ms IS 19 FPS, AND THAT IS THE REPORT.** ⚠⚠ **AND THE NEUTRAL ROW AT THAT RUNG IS 38 ms,
WHICH IS THE OTHER HALF OF THE REPORT** — *"it also did before"*. The mask adds ~11 ms to a frame
that was already ~40 ms without it. **The mask was never the problem; the rung was**, and the mask
is simply the largest of several things stacked on an already-slow frame.

⚠ Every earlier number in this entry, and in `bench_masked` generally, is a **1600 px** number. The
4x pixel spread between the rungs is larger than any effect measured in rounds 1 and 2 put together
— **the benchmark was measuring a case the user may never be in.** ⚠ Note this is on a **half-size**
buffer; after the full-resolution upgrade `buffer_long` is 6246 and the ladder is 1600 -> 3200 ->
6246, so a large window sits at 3200 and this is if anything the optimistic reading.

**▶ TO CONFIRM BEFORE DESIGNING ANYTHING: which rung is the user's window actually on.** The bench
cannot answer it and neither can reasoning — only the running app can. `RUSTROOM_DEBUG_RENDER=1`
now prints, per frame, the render time, the rung, the frame size in Mpx, whether it is a region,
and how many prefix stages ran. ⚠ **This is a measurement, not a diagnosis, exactly as round 1's
was**, and it is the fourth time in a row this entry has been redirected by one.

⚠ **If it confirms the native rung, the shape of the fix is a rung question, not a pipeline
question** — e.g. dragging at the fit rung and settling to the native one, which is the
render-during-interaction pattern the app already applies elsewhere. **Do not scope it as
`MaskStage` work**: at 1600 the whole masked delta is 2.8 ms, and no amount of optimising it moves
a 53 ms frame meaningfully.

### ⚠ What is left in `HslStage` after the hue LUT (recorded 2026-09-05) → M10.14a

The LUT took the stage from ~29–34 ms to ~20 ms above a harness floor (1.47–1.66×). **What remains
is two things and neither is a tweak.** Decomposed shares of the *original* cost:

1. **The Oklab round trip — ~27%, and now the largest single item.** Three `cbrt` in, three cubes
   out. ⚠ Cutting it means approximating `cbrt`, which is a different accuracy question and affects
   **every** Oklab consumer — `hsl`, `noise_reduction`, `sharpening`, `local_contrast`,
   `color_grading` — not just this stage. A bit-manipulation seed plus one Newton step is the usual
   answer; the argument to make is about error in *Oklab* units, where this project has no
   established budget.
2. **`atan2` — ~13%.** ⚠⚠ **A table can be built over ANY monotonic parameterisation of hue**, so
   indexing by a cheap *pseudo-angle* (one divide, no trig) would delete `atan2` outright and stay
   exact in kind, because the table is sampled in whatever coordinate it is indexed by. Deliberately
   not taken with the LUT: it needs a sampling-density argument the uniform-in-hue table does not,
   since the pseudo-angle is non-uniform in true hue.

⚠ **Do not scope either from the old "~2×" figure in this file's history.** That estimate was wrong
three times running; `cargo run --release -p app --example bench_hsl <raw>` decomposes the stage and
is the only thing to trust here.

- **Brush: incremental splatting.** → **M10.14f** ⏸ (parked until a need shows). Region-scoped coverage brought a paint frame to ~16.6 ms, of
  which ~9 ms is rasterization. ⚠ The remaining cost is **dab area, not frame area** — at 1:1 a dab
  is 125 px across — so incremental splatting is the lever if this ever needs to be faster.
- ✅ **FIXED 2026-09-01** — one lensfun call per row and rayon over output rows; the 3200 rung's prefix 1178 → 309 ms. ⚠ The resampler is now ~2/3 of what is left. The entry as recorded: **`LensCorrectionStage` calls the lensfun API one pixel at a time, and it is measured at 7.7×
  the cost of the comparable stage.** A full-resolution render goes 4.59 s → 6.15 s (+1.56 s on
  26 Mpx), against `GeometryStage`'s 203 ms for a whole-buffer rotation at the same size. The cause
  is not the maths: `apply_geometry_distortion`, `apply_subpixel_distortion` and
  `apply_color_modification_f32` all take `width`/`rows` and are built to fill a **row** at a time,
  and the stage calls each with a 1×1 grid per pixel. Batching per row is the fix and should
  recover most of it; parallelising over rows with rayon is a further win, though note the existing
  `rotate_bilinear` is sequential too, so the stage is at least *consistent* with what is there.
- **Two Oklab round trips** → **M10.14f** ⏸ — when HSL and colour grading are both active. Fusing them would break the
  `PipelineStage` seam that exists for the GPU port.
- ~~**Export is sequential**~~ ✅ **parallel since M8.10 (2026-09-15)**, struck 2026-09-29. As recorded: sequential, deliberately: pipeline stages are already rayon-parallel internally, so
  parallel export would contend for the same cores while multiplying peak memory.
- **Demosaic is ~93% of a full-resolution decode**, and the Phase 8 (formerly 5) `wgpu` port would **not** fix it
  — that time is inside libraw's `dcraw_process`, entirely upstream of the stages wgpu would touch.

- **⚠ Exports are not bit-reproducible across runs** → **M10.7a** (found 2026-09-01 while validating M7.3).
  lcms2 stamps a **creation date** into every ICC profile it builds, so two renders of the same
  photo with the same settings differ by **one byte** in the profile header — offset 74 in a JPEG.
  Harmless in itself, and worth recording because of what it costs: **"did this change move any
  pixel?" cannot be answered with `cmp` on exported files.** Only the goldens can answer it, and
  only because they store no profile. ⚠ Fixing it means zeroing or pinning that field when the
  profile is built in `export::srgb_icc` and its siblings; check first whether a zeroed date makes
  any consumer unhappy.

---

## App and UI

- ✅ **`main.rs` — split by W0 on 2026-09-01**, 4,375 → 681, 917 as of 2026-09-08. ⚠ The files now over the ~600 marker are `settings.rs`, `edit-model/src/lib.rs`, `catalog/src/db.rs`, `batch_export.rs`, `panel/geometry.rs` and `library.rs`; `CLAUDE.md` names each when touched.
- **The in-window menu bar has never been run** → **M11.2** — only unit-tested — there is no Linux machine here
  and macOS never shows it. Flip the `!cfg!(target_os = "macos")` guard in `shell.rs` to look at it.
  Relatedly, **`native_menu.rs` can rot silently**, since CI never compiles it.
- **No user-defined keyboard shortcuts.** → **M10.9b**.
- ~~**The accent colour reaches only egui's own widgets.**~~ ✅ **Settled by the design language** (`DESIGN.md`
  §4.2, 2026-09-29): the accent marks selection and set state only, and the hand-painted colours that stay
  outside it are listed there on purpose. Struck 2026-09-29. As recorded: The crop overlay, histogram and thumbnail
  selection border are painted by hand with hardcoded colours — most visible in **light** mode,
  which has barely been looked at.
- ~~**Export has no filename templating**~~ ✅ **M6.1a, 2026-09-01.**
- **Presets: no import/export, no folders, no rename.** → **M10.9a**.
- **Library:** ~~no on-disk thumbnail cache~~ ✅ M8.2, ~~no subfolders~~ ✅ M9.9, no vertical filmstrip → **M10.8b**.
- ~~**A nicer decode placeholder**~~ ⛔ **superseded by M9.14** and against the author's rule that no
  low-resolution stand-in is ever painted (struck 2026-09-29). As recorded: run the pipeline over the thumbnail for an instant low-res
  preview, instead of showing the camera JPEG.

---

⚠ **This file was seeded from the follow-ups already recorded in the journal and is not a complete
sweep of that document.** When something is deferred, add it here *and* leave the detail in its
`docs/JOURNAL.md` section.

## EXIF/IPTC metadata editing (deferred 2026-08-26 — blocked on a DESIGN question)

**Status:** ✅ **SHIPPED as M6.2, 2026-09-08** — the design question dissolved under a narrower scope (see the table at the top). The entry below is kept as the record of why it was blocked.

~~**Status:** ⛔ blocked on a decision, not on effort. Split out of M4.2 before designing it.~~

**What is wanted:** displaying EXIF and IPTC properly (we surface almost nothing today — camera,
lens, focal length, orientation), and *editing* IPTC fields like caption, copyright and creator.

**Display is easy. Edit is the problem, and it is a conflict with this project's core value.**
Writing metadata has only two destinations:

1. **Into the RAW file itself** — which violates "originals are never modified", a cross-cutting
   attribute in `01-lightroom-capabilities` and the reason the sidecar model exists at all. This
   is not a trade worth making for a caption field.
2. **Into an XMP sidecar** — what Lightroom does. But Rustroom already has a JSON sidecar, so this
   means **two sidecars per photo with overlapping purposes**, and it reopens the XMP-interop
   question `02-architecture-and-tech` deliberately parked ("JSON now; consider XMP compatibility
   later for interop with Lightroom/darktable").

**The three candidate answers, none obviously right:**

- **Extend the existing JSON sidecar** with a metadata block. One file, no new dependency —
  but nothing else on earth reads it, so the interop that makes IPTC worth having is lost.
- **Adopt XMP as a second sidecar**, alongside the JSON one. Real interop; two files per photo,
  and a rule for which wins when they disagree.
- **Migrate the whole sidecar to XMP.** Cleanest end state, largest change, and it would mean
  re-encoding the entire `Edits` op-list into XMP's model.

⚠ **Whichever is chosen, editing needs a metadata-writing library** — `rexiv2` (C, gexiv2) or
`little-exif` (pure Rust, narrower). ⚠ `02-architecture-and-tech` lists `rexiv2`/`kamadak-exif` in
its stack table; **neither has ever been added**, and that table now records so.

**Do not treat this as a parsing job.** The parsing is the easy half.

## Immich integration (recorded 2026-08-26 — slices 1–4 BUILT, 2026-09-22/24)

**Status:** ✅ **slices 1, 2, 3 and 4 are built, merged and confirmed by use.**
⛔ **Two-way sync is DECLINED — 2026-09-24, the user's call, and it is not a deferral.** Its successor is
**uploading edited photos** (✅ built 2026-09-24), which is a different operation and a safer one. See
*Two-way sync — declined* below before re-proposing anything that writes to the server. ⚠ **2026-09-29:
the author re-opened two-way sync as M10.15d**, and brought the three smaller gaps into scope — the OS
keychain (the API keys are **plaintext** in `settings.json`) **M10.15a**, **automatic** eviction of cached
originals (slice 4 built the *manual* half) **M10.15b**, and reading Immich's own `rating`/`isFavorite`
**M10.15c**.

The user self-hosts an **Immich** server holding all their photos, and wants to open them in
Rustroom by giving a URL and credentials.

### ⚠⚠ The insight that makes this tractable: the catalog changed the answer

Rustroom's model assumes *a local file with a sidecar beside it* — `PathBuf` is a photo's identity,
`BufferCache` keys on it, sidecars live next to the file. An Immich asset has no local path, so on
the face of it this fights the architecture everywhere.

**But if assets are downloaded into a local cache directory, an Immich library is just a folder.**
`catalog`, `entries::build`, `ThumbLoader`, sidecars, marks, keywords, collections and export then
all work **unchanged**. It maps onto the model rather than fighting it. Immich also serves
thumbnails separately from originals, which maps cleanly onto `ThumbLoader` — so browsing does not
mean pulling tens of gigabytes of RAW.

### Three slices, agreed order

1. **"Open this one photo from Immich."** Authenticate, browse, pull one original into a cache,
   edit, export locally. ⚠ **Genuinely small** — the download is one HTTP call and everything after
   it already exists. It is also the workflow that plays to both tools' strengths: Immich stores
   and shares, Rustroom edits.
2. **"Browse my Immich library."** Lazy thumbnails, originals fetched on open, the catalog holding
   asset ids. Read-only. Designed to fall out of slice 1.
3. **~~Two-way sync~~ — ⚠⚠ THE SLICE THAT GOT BUILT IS NOT THE SLICE WRITTEN HERE.** Asked in
   brainstorming on 2026-09-23, the user chose **local only** of three options offered: catalog rows so
   marks persist in Rustroom, and **nothing sent to Immich**. ✅ Built and confirmed by use.
   ⛔ Two-way sync itself is now **declined outright** — see *Two-way sync — declined* below.
4. **Cache management — a Download button and a Discard button (requested 2026-09-24).** See below.

### ✅ ANSWERED 2026-09-22 — the original is the original. Do not re-research this.

**Immich serves the true original for RAW files.** `GET /assets/{id}/original` — *"Downloads the
original file of the specified asset"*. `AssetFileType` is `fullsize | preview | thumbnail | sidecar
| encoded_video` and the original is **not** in that list, because it is the asset's own
`originalPath`: derivatives are additive and the original is never one of them.
`server/src/utils/mime-types.ts` registers the full RAW set. So slice 1 keeps its point.

⚠⚠ **Slices 1–3 are BUILT and confirmed by use** (2026-09-22 to 2026-09-24): albums browse, a
**Timeline** of months, server thumbnails, download on open, edit, persist; **asset lists cached** and
revalidated against `(assetCount, lastModifiedAssetTimestamp)`; **⋯ → Reindex** refetches; **marks
persist** via catalog rows, one folder per server; the rail reads **"immich (downloaded)"** and counts
only supported images; a **circled ↓** marks downloaded photos in the grid.
`grep -n 'Immich slice' docs/JOURNAL.md`.

⚠ **The invariants that are easy to break again** — all three cost real bugs: the catalog keys a row on
the **path's own file name**, never `CatalogEntry::file_name`; `row_is_browsable` hides an absent
**Immich** row from catalog-wide views but **never** an absent local one; and `present_names` is one
`read_dir` filtered through `catalog::is_supported`, never a stat per thumbnail.

⚠ **But the question this heading asked was the wrong one to stop at.** The two defects that
actually cost time were not about originals at all: `GET /albums/{id}` returns **no `assets` field**,
and a real server pages by **`nextPage`** while leaving `nextCursor` null. Neither is discoverable
from the OpenAPI schema, and both were found by a real reply. **Before the next Immich slice, capture
a real response for every endpoint it touches** — see the fixture lesson in `CLAUDE.md`'s standing
lessons.

### ⛔ Two-way sync — DECLINED 2026-09-24 (the user's call, with reasons) → re-opened 2026-09-29 as M10.15d

> **2026-09-29:** the author put this back on the list as **M10.15d**. The reasons below stand as the
> brainstorm's starting point — the decision to revisit is the author's, and it answers the warning below.

⚠⚠ **Do not re-propose this from a list of "unbuilt Immich slices". It was considered and refused on
its merits, and the thing that replaces it is *not* a smaller version of it.** The user's words:
*"I'm unsure about the two-way sync, I feel like it's too dangerous."*

**The distinction that decided it is MUTATING versus CREATING**, not "writing versus not writing":

| | two-way sync | uploading an edit |
|---|---|---|
| operation | **overwrites** fields on an asset Immich treats as authoritative | **creates a new** asset |
| permission | **`asset.update`** — can rewrite *any* asset | a separate upload grant, which **cannot touch existing assets** |
| worst case | the user's real library silently altered, **no undo** | clutter — extra assets, deletable in Immich's own UI |
| trigger | background, continuous, **unattended** | one deliberate action |
| reversible | no | yes |

`Permission` is a ~150-value enum, so these are genuinely **separate grants** — an upload key is not
"sync, but politer".

⚠⚠ **The row that actually settles it is the TRIGGER, and the argument is from this repo's own record.**
Every Immich defect so far — the missing `assets` field, `nextPage`, the `null` `thumbhash`, the catalog
key — was found **by use, never by the green suite**. That works because the user was looking at the
thing when it broke. A background sweep over a whole library **removes that safety net exactly where the
blast radius is largest**: *"300 photos got the wrong rating three weeks ago"* is not a bug this
project's feedback loop can catch. ⚠ This is the same standing lesson as *"almost every GUI-wiring
defect here was found by USE"*, pointed at a feature instead of at a milestone.

**And the payoff was thin regardless:** colour labels and reject flags have **no Immich equivalent**, so
only ratings and favourites could ever have round-tripped.

⚠ **What would have to change for this to be reopened** — stated so a future reader does not have to
re-derive it: a way to make the write **attended and reversible** (per-photo, confirmed, with the prior
value recoverable), *and* a reason the additive path below cannot serve the same need. Neither exists
today.

### ✅ Uploading edited photos to Immich — BUILT and confirmed by use (2026-09-24)

✅ **Extended the same day, twice, both from use:** delivery is a three-way choice (**files, Immich, or
both**) rather than a flag on a file write, and it lives inside **Location** rather than a section of
its own — *"where does this go"* is one question. The upload date is an option
(`Captured`/`Edited`/`Uploaded`) with the **mtime fallback removed**, a single photo shows the resolved
date beside the dropdown, and the first unconfirmed upload of a run raises a notice naming where the
file is. Specs `2026-09-24-export-delivery-design.md` and `2026-09-24-location-reconciled-design.md`;
`grep -n 'export delivery' docs/JOURNAL.md`.

**Status:** ✅ built, gated green, **confirmed by use**. Spec
`docs/superpowers/specs/2026-09-24-immich-upload-design.md`, plan
`docs/superpowers/plans/2026-09-24-immich-upload.md`; `grep -n 'Uploading edited photos' docs/JOURNAL.md`.

✅ **The two properties only a real server could settle, both measured 2026-09-24:** the uploaded
edit's **`fileCreatedAt` is its capture time** (2026-09-22T21:01:06Z) while `createdAt` is the upload
time — so the "it landed at today's date" trap did not happen; and a byte-identical re-upload replied
**HTTP 200 `{"status":"duplicate","id":…}`** with the asset count **16,118 before and after**, so the
server dedupes and creates nothing. ⚠ The DTO fixture is now **captured**, not constructed, and its
doc says so — it also records two things the schema did not: field order is `status` then `id`, and a
duplicate is **200**, not 201.

⚠ **Shipped narrower than the section below imagined, by decision:** plain upload into the timeline;
**any** photo, not only Immich-sourced ones; no albums; not stacked with the original; no
library-wide "upload everything edited" sweep. Each remains its own later change.

**Why this was the stronger feature, not merely the safer one.** This document's own framing is *"Immich
stores and shares, Rustroom edits"* — and that loop is **currently broken**: an edit lands on local disk
and never returns to where the photos live and are shared. Sync would have moved *metadata the user can
already see in Rustroom*; upload moves **the thing they made**.

✅ **It also delivers much of what sync promised, additively.** `export/src/metadata.rs` already writes
a full XMP packet carrying **rating, label, keywords, title, caption, rights and creator**, so an
uploaded edit carries its marks *inside the file* — no permission to modify anything.
⚠⚠ **UNVERIFIED:** whether Immich maps XMP `Rating` onto its own rating field on ingest. That is a claim
about Immich, and this integration has been bitten four times by claims about Immich taken from a schema
or from memory. **Capture a real reply before designing around it.**

⚠ **The design problems, which are not the HTTP call:**

- **Duplicates.** Nudge a slider, re-export, upload → a second asset, and **checksum dedup will not catch
  it because the bytes differ**. ⚠⚠ The tempting fix — replace the previous upload — means **delete**,
  which drags the whole declined danger back in. Leaning: record the uploaded asset id in the **sidecar**
  and surface *"already uploaded on ‹date›"*, leaving the choice to the user.
- **The read-only key property is lost either way — this is the honest cost.** Mitigation worth
  designing: a **second, optional key** for uploads, so the browse path (constant, unattended) *literally
  cannot write* and the write capability exists only on the key behind one deliberate button. Same spirit
  as slice 4's *"this cannot touch the server"* hover, but enforced by the **credential** rather than by
  our code being correct.
- **Where does it land?** A "Rustroom edits" album needs **`album.write`** — another grant. The
  original's own album is the alternative.
- **Stacking the edit beside its RAW** would be ideal. ⚠ Another unverified claim about Immich's API.

### ✅ Immich stacking — VERIFIED AGAINST THE USER'S OWN SERVER, 2026-09-24

⚠⚠ **Everything here was measured against `immich.example` running **v3.2.0**, with the
OpenAPI spec taken from Immich's repo at the *same* version. It is not from memory and not from the
public docs.** The library: **16,139 assets**, 12,286 `.jpg`, 2,514 `.raf`.

**The contract.** `POST /stacks` takes one required field, `assetIds`, documented **"Asset IDs (first
becomes primary, min 2)"** — so *the caller chooses the primary by ordering*. Returns
`StackResponseDto {id, primaryAssetId, assets[]}`. Also `GET`/`DELETE /stacks`, `GET`/`PUT /stacks/{id}`,
`DELETE /stacks/{id}/assets/{assetId}`.

**Permissions — 168 values, and the ones that matter are SEPARATE grants:**
`stack.create`/`read`/`update`/`delete`, and **`asset.upload` distinct from `asset.update`**.
✅ Stacking needs only **`stack.create`** and touches **no asset field**. ✅ The user's existing key already
has it (tested: `201`). ⚠⚠ **This is the verified basis of the whole upload-over-sync argument**: an
upload-capable key *cannot* modify or delete existing assets.

⚠⚠ **THREE TRAPS, none of them visible in the schema:**

1. **`POST /search/metadata` reports `stack: null` even for a stacked asset**, while
   `GET /assets/{id}` reports `{id, primaryAssetId, assetCount}` correctly. Search is the endpoint
   Rustroom lists with, and a per-asset GET over 16k assets is not viable — so **"is this already
   stacked?" must come from enumerating `GET /stacks` once.** A constructed fixture asserting
   `Asset.stack` from a search reply would pass and be wrong.
2. **`withStacked` is INVERTED from how it reads.** On `GET /timeline/bucket`, `withStacked=true`
   *collapses* stacks (primaries only) and `false` shows every member. Measured on Aug-2026:
   **80 → 79** for one stack. Anyone would read the name the other way round.
3. **Immich's `duplicates` endpoint is PERCEPTUAL, not filename-based** — `machineLearning.duplicateDetection`
   (`maxDistance` 0.01). It found **30** of **2,379** RAW+JPEG pairs (**1.3%**). It is not the tool for
   pairing, and there is **no auto-stacking setting anywhere in v3.2.0's config**.

⚠⚠ **THE MISTAKE THIS ALMOST MADE, and it was real rather than hypothetical: PAIR ON (STEM, CAPTURE
TIME), NEVER ON STEM ALONE.** This camera's counter has **wrapped**, so `DSCF0236.JPG` exists for a
**2024-03-25** photo *and* a **2023-07-04** one. Grouping by stem alone offers the 2024 `.RAF` a 2023
`.JPG` as its partner — which would have **stacked two unrelated photos and hidden one from the
timeline**. Keying on `(stem, fileCreatedAt)` resolved it exactly and *also* recovered all 44 groups a
stem-only 1:1 rule had to reject: **2,334 → 2,378 clean pairs**, reconciling to 2,379 with the one
already stacked.

⚠⚠ **`PUT /stacks/{id}` accepts ONLY `primaryAssetId` — there is NO "add asset to stack".** It can flip
which half is the timeline face (so a JPEG-primary choice is reversible without unstacking), but adding a
third member means **`DELETE /stacks/{id}` then `POST /stacks` with all three ids** — a dissolve-and-
rebuild, not an additive call. ⚠ This is a real cost on any *"upload the edit and stack it with its
original"* design. ✅ Conversely **`DELETE /stacks` takes a bulk `BulkIdsDto {ids}`**, so the entire
migration is reversible in **one call**.

**The one-time migration** was run as an audited script (dry run → 5-pair batch → verify → rest), each
created stack id logged for reversal, JPEG first so the camera rendering stays the timeline face.
⚠ It is idempotent: it re-reads `GET /stacks` every run and skips what exists.

⚠⚠ **THE RUSTROOM-SIDE HALF IS NOT FREE AND IS NOT DONE.** Rustroom lists via `search/metadata`, which
has no stacking awareness *and does not report `stack`*, so **Rustroom's grid still shows both halves of
every stacked pair** while Immich's timeline collapses them. Any "stack pairs" feature in the app owes a
grid that honours stacks — fed from a `GET /stacks` enumeration, not from the listing.

### ✅ Local stacking BUILT (2026-09-25) — ✅ Immich's own stacks honoured too (below)

✅ **Same-stem files group into one grid cell**, independent of Immich: `Pictures/fuji` was **830 cells
for 415 photographs**, and both camera folders are **100% RAW+JPEG pairs**. A **view grouping only**;
Settings → Appearance carries the toggle, the primary choice and the participating formats. Spec
`docs/superpowers/specs/2026-09-25-local-stacking-design.md`; `grep -n 'stacks: same-stem'
docs/JOURNAL.md`.

⚠⚠ **It does nothing for Immich and cannot.** `cache_path` names files by **asset UUID**, so no two
ever share a stem — measured: **0 pairs** against 2,913 cached assets, where the two camera folders are
100%. Pairing on that server is a *server-side* fact.

### ✅ Honouring Immich's own stacks — BUILT 2026-09-25 (the Immich integration is complete)

✅ **One function does it:** `stack::key_of_with` keys a stacked asset on `(cache_root, "stack:<id>")`
instead of its stem, so all of local stacking applies unchanged and an empty map is byte-identical to
before. `GET /stacks` is **6.3 MB / ~2.9 s** (full asset DTO per member), so a **472 KB projection** is
cached — 13× — fetched as `Job::Stacks` off the UI thread and dropped by Reindex.
⚠⚠ **No validator exists**, so freshness rests on Reindex alone. ⚠ The local *Show* rule picks the
primary, so untouched pairs show the RAW where Immich's UI shows the JPEG.
`grep -n "server-side stacks" docs/JOURNAL.md`.

⛔ **Still out of scope by decision:** creating or removing stacks from Rustroom (`stack.create` and
`stack.delete` are grants the browse key does not have), reconciling the primary with Immich's, and
two-way sync.

**Status:** ✅ built 2026-09-25 (the heading's). As first recorded — ⬜ wanted, unblocked, not specced. Raised the moment the server-side migration landed:
Immich's timeline now collapses **2,378** RAW+JPEG pairs, and **Rustroom's grid still shows both halves**,
so the two views of the same library disagree.

**Why it is not free.** Rustroom lists assets with `POST /search/metadata`, which is stacking-blind **and
returns `stack: null` even for a stacked asset** (verified — see above). So the app cannot learn stack
membership from the listing it already makes. The shape that works:

- Enumerate `GET /stacks` **once per refresh** and build `HashSet<asset id> → stack`, the same
  fetch-then-index move `present_names` uses for downloads.
- Hide non-primary members from the Immich grid by default, mirroring Immich's own behaviour.
- ⚠ **Cache it like the asset lists.** `immich_list_cache` already stores lists against a validator;
  a stack enumeration wants the same treatment or it is another per-launch refetch — the gap the rail's
  album and bucket lists still have.

⚠ **Open questions, none blocking:** does the grid want a *stack badge* and a way to see the hidden
member (Immich shows a stack icon and lets you page through), or is hiding enough? And should a
Rustroom-initiated **pair-and-stack** action exist in the app at all, or was the one-time script the whole
job? ⚠⚠ If it does exist in-app, it inherits every safeguard the script needed — above all
**pair on `(stem, capture time)`, never on stem alone**, which this library proves is not paranoia.

### ✅ Immich is COMPLETELY optional behind one gate — BUILT 2026-09-24

**Status:** ✅ built, gated green. `Settings.immich_enabled` (default **true**, the `gpu_tail` trap) is
read **only** through `immich_source::connection`, the single choke point nine call sites already used —
so no surface grew its own condition and the export ticks needed no change. The Settings tab stays
(it holds the toggle); the rail section, the toolbar actions, the grid badge and the export ticks all
vanish. The downloaded folder **stays visible**, renamed *"Downloaded photos"*, with its **hover
replaced too** — the path itself says "immich" and names the server. ⚠ That withholds where the files
are, accepted because the requirement was explicit. `grep -n 'completely optional' docs/JOURNAL.md`.

⬜ **Still open:** deleting or evicting caches on disable (nothing is deleted today) → **M10.15b**, and the
OS keychain → **M10.15a**.

**Status:** ✅ built 2026-09-24 (the heading's). As first recorded — ⬜ wanted, unblocked, not specced. The user's words: *"the whole immich integration should in
the end be completely optional … a toggle in the immich header in settings with which one can completely
disable the immich support, i.e. in that case there should be **no mention of immich at all**."*

⚠⚠ **"No mention at all" is a stronger requirement than "the feature does nothing", and it is the reason
this is worth designing BEFORE more surfaces exist.** Every Immich surface must consult **one predicate**,
not its own conditional. Today the surfaces are: the **rail section**, the **Settings tab**, the grid's
**circled ↓ badge**, the toolbar's **Download / Remove downloads** buttons, the Immich **progress line**,
the **Timeline** date tree, and — once built — the Export dialog's **upload section**. A gate retrofitted
across surfaces that assume Immich exists is the expensive version of this change.

⚠ **This is why `ExportOptions` gains `upload_target: Option<UploadTarget>` and NOT an Immich-named
field** (see the upload spec). The `export` side never learns the word "Immich"; it knows only that a
delivery target may exist. With the integration off, the enum offers no variant, the dialog draws no
section, and the worker skips the step — the persisted value stays in the file and is simply never read,
which is also what keeps re-enabling lossless.

⚠ **Open questions:** does disabling **hide or delete** the caches (`immich/` originals, the two thumbnail
stores, the list cache)? Leaning **hide** — deleting on a toggle would destroy downloads the user may want
back, and slice 4 already gives them an explicit *Remove downloads*. And what happens to **catalog rows**
for Immich assets — `row_is_browsable` already hides absent Immich rows, so this may fall out for free,
but it wants checking rather than assuming.

### Slice 4 — a Download button and a Discard button (requested 2026-09-24)

**Status:** ✅ **BUILT, merged and confirmed by use (2026-09-24).** Spec
`docs/superpowers/specs/2026-09-24-immich-download-discard-design.md`; `grep -n 'Immich slice 4'
docs/JOURNAL.md`. The user's words were: *"I'd like there to be a 'download' button, that allows me to
predownload e.g. an album or selection of images from immich, as well as a discard or delete button."*

✅ **Every open question below was answered by building it, and the answers are worth keeping** because
each was a real decision: the buttons live in **both** places (rail *and* toolbar, and each hides itself
when it does not apply); the word is **"Remove downloads"**, it cannot touch the server, and the hover
says so; the **sidecar, catalog row and thumbnail all survive** a removal, which is what makes
`cache_path`'s byte-identical re-download reattach the work; progress is a line with a **Cancel**, behind
a **bounded queue capped at 4** — without it a bulk album request would have spawned **872 threads**; and
a bulk download deliberately **does not open** anything, because *"opening an image and downloading it
are two distinct things"*.

**Why it follows naturally.** Everything needed already exists and is confirmed by use:
`immich_worker`'s `Job::Original` downloads one asset to its deterministic cache path;
`immich_source::present_names` answers which assets are already here in one `read_dir`; the grid's
**circled ↓** already shows which; the Folders rail already counts them as **"immich (downloaded)"**; and
the selection model is the ordinary one, so "this album" and "these photos" are the same question the
export dialog already answers.

**What it is really for:** taking a working set offline, and getting the disk space back afterwards —
which is the other half of the *"no eviction for the cached originals"* gap that has been on the
out-of-scope list since slice 1.

⚠ **Open questions, none blocking, all worth deciding before building:**

- **Where do the buttons live?** The rail's album row (a per-album action, beside Refresh) or the
  Library's toolbar (a per-selection action, beside Export)? ⚠ Likely **both**, because "the album" and
  "what I picked" are genuinely different targets — the same asymmetry `apply_mark_to` documents for
  Compare and Survey.
- **Discard must not mean "delete from Immich."** ⚠⚠ The word is dangerous: the asset stays on the
  server, only the **local cache copy** goes. Prefer **"Remove downloads"** or **"Free up space"** over
  "Delete". The key is read-only and must stay that way, so the button *cannot* touch the server — say
  so in the hover.
- **What happens to edits?** A discarded photo's **sidecar** holds its edits, and its **catalog row**
  holds its marks. Both must survive — the path is deterministic, so re-downloading restores byte-
  identical identity and the edits reattach. ⚠ That is exactly what `cache_path`'s doc means by
  *"re-downloading an evicted or deleted file must restore byte-identical identity"*. **A discard that
  also removed the sidecar would silently destroy work.**
- **Progress and cancellation.** An album of 872 assets is a long download. `immich_worker` is one job
  per asset today; a bulk download wants a queue with a count, and the `batch_export` worker is the
  precedent for the shape.
- **Does the grid's ↓ need a third state?** "Downloading" is currently invisible —
  `ImmichWorker::is_downloading` exists and nothing draws it.

⚠ Before building: **capture a real response for every endpoint it touches.** The two defects that cost
the most time in this integration were both invisible in the OpenAPI schema.

### ⚠ Three honest costs

- **Credentials are a new kind of surface for this project.** → **M10.15a**. Rustroom has never stored a secret.
  An API key in `settings.json` in plaintext is the easy path and probably not the right one; the
  OS keychain is right and is platform-specific work.
- **Duplicated storage.** → **M10.15b**. A cached RAW is a second copy — fine for a working set, needs an eviction
  policy if it becomes a library. `BufferCache`'s LRU is the precedent.
- **It expands what Rustroom is.** Every planning note says "local files, originals never modified,
  Linux first". A network dependency is a genuine change of character — not wrong, but worth
  deciding deliberately rather than sliding into.

## Film negative inversion (recorded 2026-08-26 — a FEATURE, not a plugin) → M10.11a

**Status:** ⬜ **M10.11a** (scheduled 2026-09-29). Wanted, not blocked. A `PipelineStage` like any other.
⚠ **2026-09-29:** the eyedropper this entry calls the one new piece of machinery **exists now** — white
balance (`wb_tool`) and defringe both sample the canvas.

The user wants Lightroom's negative-inversion capability (the Negative Lab Pro workflow) for
scanned analogue film. ⚠⚠ **It arrived as a request for a plugin *system*, and it is not one.**

**What it actually is:** invert the channels, derive white balance from the **film base** colour,
apply a tone curve. **Every one of those already exists in this pipeline.** What it needs beyond
presets is a *per-photo measurement* — sampling the unexposed film base — and therefore an
eyedropper on the canvas.

So it is **a stage plus an eyedropper**, not an extension mechanism. Building a plugin system to
obtain it would be solving the wrong problem at roughly twenty times the cost.

⚠ **Why it is worth doing on its own merits:** no open-source RAW editor does film inversion well,
so it is genuinely differentiating — and the scene-referred linear pipeline is a *better* place to
do it than Lightroom's, because inversion is a radiometric operation.

⚠ **The one new piece of machinery:** an eyedropper. Nothing in Rustroom samples a pixel from the
canvas yet. It would also serve the white-balance eyedropper that `01-lightroom-capabilities`
already lists as missing — so build it once, for both.

## A plugin system (recorded 2026-08-26 — DELIBERATELY not now)

**Status:** ⛔ deferred on judgement → **M11.5**, the last milestone.

Wanted to encourage outside contributions once the project is public. Three reasons not yet:

1. **⚠⚠ A plugin API is a public contract, and this one is not ready to freeze.** In a single week
   `Edits` gained `lens_correction` and an eighth edit group, the prefix/tail split changed,
   `Library.dir` became `LibrarySource` and then gained a third variant, and `PhotoId` replaced
   `PathBuf` in six places. Freezing on top of that gives one of two bad outcomes: break plugins
   constantly, or stop changing the core. **At Phase 4 of 6, stopping changing the core is the
   expensive one.** ⚠ The GPU port will change `PipelineStage` again, which is why the revisit
   point is *after* Phase 5.
2. **It is probably not what attracts early contributors.** They want to change the *core* — that
   is where the interesting problems are. A plugin boundary is a barrier ("where does this go? is
   this allowed?"), not an invitation.
3. **The concrete want behind the request — film inversion — does not need one.** See above.

### The trade, recorded so the eventual decision is informed

| approach | verdict |
|---|---|
| **Dynamic libraries** | Rust has **no stable ABI**, so it needs a C API or `abi_stable`. And a plugin crash takes the whole app down. |
| **WASM** (wasmtime / extism) | Sandboxed, cross-platform, no ABI problem. ⚠ The real question is data movement: a 26 Mpx f32 buffer is **313 MB**, so a per-stage copy across the boundary is prohibitive. Shared linear memory makes it tractable — that is the design work. |
| **Scripting** (Lua / Rhai) | Easy to write, far too slow per-pixel at 26 Mpx — **unless** the plugin only *describes* an op-list rather than touching pixels, which covers more cases than it sounds. |
| **Declarative** | A plugin as a recipe over existing stages. Cheapest by far, and honestly covers most of what people call plugins. |

⚠⚠ **GPL-3.0 makes this a licensing question too, not only a technical one.** A dynamically-linked
plugin is plausibly a derived work, which constrains who may write closed-source ones. Decide that
deliberately *before* publishing any API.

⚠ **`PipelineStage` is already the extension seam**, and a good one — "add a stage" is a
well-shaped contribution with a golden-image test to prove it. That is what to point contributors
at meanwhile.

## Contributor-facing documentation (recorded 2026-08-26 — the cheap half of "open source") → M11.3

**Status:** 🟡 wanted, small — and **deliberately scheduled LAST, at the final alpha** (user's call,
2026-08-27). ⚠ The reason is not priority: it describes a shape that is still moving weekly, so
writing it now means writing it twice. It stays the highest-value thing for attracting
contributions *once the architecture has settled*.

⚠⚠ **`CLAUDE.md` is already an extraordinary architecture document** — the reasoning, the rejected
alternatives, the mistakes and what they cost. Very few projects have that. **But it is addressed
to an AI assistant, not to a contributor**, and it is enormous.

1. **`ARCHITECTURE.md`** — reframe the load-bearing parts of `CLAUDE.md` for a human arriving cold:
   the staged pipeline, the prefix/tail split, `RenderScale` and `Region`, the sidecar/catalog
   split, and the crate layout. **Mostly editing, not writing.**
2. **`CONTRIBUTING.md`** — how to build (⚠ the `PKG_CONFIG_PATH` line alone is a barrier nobody can
   guess), how to run and regenerate the goldens, and the sabotage-testing discipline, which is
   this project's most distinctive practice and completely invisible from outside.
3. **Small labelled issues.** Several already exist in this file.

## ✅ ~~The whole-pipeline builders default `ProfileRendering` SILENTLY~~ — CLOSED 2026-09-18

> **The silent default is gone.** All six builders take the rendering as a required argument; 122
> call sites across four crates now pass it, and `ProfileRendering::colorimetric()` is the name a
> caller that wants none uses. Behaviour-preserving: **2557 tests, 74 goldens, no golden image on
> disk changed.**
>
> ⚠⚠ **NOT "the class is closed" — "the silent default is gone".** A developer adding a call site
> will type whatever compiles, and the compiler cannot know which rendering is right. What this
> bought is a decision point and a name that looks wrong in review. A hard guarantee would need a
> test asserting every user-facing render path passes a *resolved* rendering, which is a different
> and harder piece of work and is **not** done.
>
> ⚠ A rename of `ProfileRendering` to `TailRendering` is still wanted — `look` is not a
> camera-profile field — and is now a ~120-site diff. Its own change, never folded into another.
>
> ⚠ Original text follows, as the record of what the hole was.

## ⚠⚠ The whole-pipeline builders default `ProfileRendering` SILENTLY (recorded 2026-09-18)

**Three call sites have lost the camera rendering and the creative look this way**, and the third
was reported by use rather than caught by a test:

| path | had it |
|---|---|
| the preview (`render_cached`) | ✅ always |
| the **export** (`export_one`) | ⛔ until 2026-09-18 — curve, baseline **and** the look |
| a **thumbnail** (`render_edits`) | ⛔ until 2026-09-18 — the look |

**The mechanism.** `build_pipeline`, `_with`, `_to` and `_spec` all hand
`ProfileRendering::default()` to the split builder. A new call site therefore renders without the
profile and without the user's `.cube`, **and nothing says so** — not a compile error, not a
warning, not a failing test. The default is indistinguishable from a deliberate choice.

`build_pipeline_rendered` was added 2026-09-18 so a caller that *has* a rendering has somewhere to
put it, but the silent defaults remain on the four older wrappers.

**What would actually fix it:** make the rendering a required argument and let callers that genuinely
want none pass `ProfileRendering::default()` explicitly. ⚠ That is four builders and every call site
in the workspace, which is why it was not folded into the fix that found it — the same attribution
argument M8.9 used. **Do it as its own change, and expect the diff to be wide and shallow.**

## ✅ ~~The camera rendering does not reach an export~~ — FIXED 2026-09-18, struck 2026-09-29

> `batch_export/render.rs` builds the export tail with the photo's own rendering since 2026-09-18 (its
> comment at the call: *"This was `ProfileRendering::default()` until 2026-09-18, and that was the bug"*).
> The entry below is the record.

`batch_export::export_one` builds its tail with `pipeline::ProfileRendering::default()`, so the
camera profile's **tone curve** and **baseline exposure** are absent from every exported file while
the preview applies both. Inherited rather than introduced by M8.9 — `build_pipeline_spec`, which
`export_one` used to go through, passed the same default.

⚠⚠ **Deliberately not fixed alongside M8.9's lens-correction fix**, and the reason is about
attribution rather than effort: both change the pixels of every exported RAW, and landing them
together makes the resulting diff unattributable to either. The lens half is measured at worst 187
of 255 levels; this half has **no measurement at all yet**, and taking one is the first task.

⚠ What it needs: `decode_for_export` already has the metadata in hand (it is where `resolve_lens`
runs), and `ExportSource` is the struct to carry the profile in — the same shape the lens shot now
uses. The awkward half is that `ProfileRendering` borrows, so the owned parts have to live
somewhere for the duration of the render.

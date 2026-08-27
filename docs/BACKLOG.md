# Backlog

Deferred work, with **why** it was deferred. Nothing here is a bug or a blocker; each item was
a deliberate decision to stop somewhere.

The rule for this file: an entry records the *reasoning*, not just the wish. An item deferred for
a **design** reason and one deferred for **effort** are different things, and mixing them up is how
a hard question gets picked up as though it were a small job.

Milestone-level work lives in the roadmap
(`~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md`); implementation detail
lives in `CLAUDE.md`. This file is for things consciously left undone.

---

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

Seeded from the recorded follow-ups in `CLAUDE.md`; see the sections there for detail.

- **C2/C3 have no independent oracle.** `rawtherapee-cli` is installed but crashes on this machine
  (exit 133, SIGTRAP, no output). The dual-illuminant interpolation is therefore **bounded but not
  independently verified** — it is judged by construction and against libraw, not diffed against a
  reference DCP implementation. This is why `camera_profile` stays opt-in.
- **Per-photo camera profile.** C4 is per *camera*, keyed by `Session::make_model`. Per-photo is
  what Lightroom does in its Calibration panel and becomes wanted the moment someone holds both
  "Camera Velvia" and "Adobe Standard" for one body. ⚠ It puts a *decode* parameter into `Edits`,
  which must then travel through the sidecar, presets, copy/paste, sync **and the buffer-cache
  key** — that key is where the work is.
- **Soft-proofing.** The preview still cannot show colour outside sRGB even when exporting P3,
  because the pipeline clips to sRGB before the display transform. The macOS colorspace work makes
  this easier: tag the layer P3 and render P3.
- **Gamut mapping / rendering intents at export.** Out-of-gamut colour currently clips per channel
  rather than being perceptually compressed. A much larger feature.
- **`CameraCalibration` is neither parsed nor applied.** A profile carrying one renders as though it
  were the identity. Recorded rather than silently mis-rendered.
- **Multi-monitor on Linux and Windows.** macOS hands the preview to Core Animation, which handles
  a display change for free. Elsewhere `display_space` is one global setting and nothing reacts to
  moving the window between monitors. Wayland's colour-management protocol is the eventual answer.

---

## Editing tools

- **Mask intersection and subtraction between masks.** `erase` works only *within* one brush; a
  radial cannot be cut out of a linear.
- **Auto-mask / edge detection** for the brush.
- **Dehaze over-corrects bright achromatic subjects** — snow, white walls, a white cat read as haze.
  The classic dark-channel-prior artifact, inherent to the model; a real fix needs large-bright-
  region detection and is its own feature.
- **Negative Texture is not edge-aware**, so it softens edges within its band. Deliberate — noise
  reduction already owns edge-aware smoothing and is upstream. A guided-filter base for the negative
  half is the follow-up, at the cost of the tool behaving differently either side of zero.
- **Clarity has no halo suppressor**; the midtone weight is the mitigation. A `Detail`-style
  suppressor is a contained follow-up.
- **No output/print sharpening** for resized exports.
- **Brush strokes shift when lens corrections are toggled.** Strokes are anchored in
  "pre-geometry" coordinates, which means *the input to `GeometryStage`* — and lens correction now
  runs ahead of it, so enabling or disabling it redefines that basis. The shift is up to ~3.5% at
  the corners and ~0 at the centre. ⚠ The principled fix is to include distortion in `FrameMap` so
  strokes anchor in raw sensor coordinates and travel *with* the correction — which is what "a
  stroke must stay on the cat" actually demands, since the correction moves the cat. It needs the
  **forward** distortion map and the `lensfun` crate exposes only the inverse, so it means an
  iterative solve per stroke point. Deferred on the grounds that lens corrections are enabled once,
  early, not toggled mid-edit.
- **Perspective correction (M3.2's remaining slice).** Keystone and rotation, either from manual
  sliders or from automatic vertical detection. ⚠ Unlike the three automatic corrections it is
  **not profile-driven** — it needs user input and canvas interaction, which is the half this
  project's record says is expensive and defect-prone. The maths is `mod_pc` in the `lensfun`
  crate and `Modifier::enable_perspective_correction` is already available; the work is the UI.
- **Colour grading does not reach masks.** `LocalAdjustments` is per-pixel-only and a grade is
  global look. Also **no visual feedback of the grading range boundaries** on canvas or histogram.

---

## Performance

- **Brush: incremental splatting.** Region-scoped coverage brought a paint frame to ~16.6 ms, of
  which ~9 ms is rasterization. ⚠ The remaining cost is **dab area, not frame area** — at 1:1 a dab
  is 125 px across — so incremental splatting is the lever if this ever needs to be faster.
- **⚠ `LensCorrectionStage` calls the lensfun API one pixel at a time, and it is measured at 7.7×
  the cost of the comparable stage.** A full-resolution render goes 4.59 s → 6.15 s (+1.56 s on
  26 Mpx), against `GeometryStage`'s 203 ms for a whole-buffer rotation at the same size. The cause
  is not the maths: `apply_geometry_distortion`, `apply_subpixel_distortion` and
  `apply_color_modification_f32` all take `width`/`rows` and are built to fill a **row** at a time,
  and the stage calls each with a 1×1 grid per pixel. Batching per row is the fix and should
  recover most of it; parallelising over rows with rayon is a further win, though note the existing
  `rotate_bilinear` is sequential too, so the stage is at least *consistent* with what is there.
- **Two Oklab round trips** when HSL and colour grading are both active. Fusing them would break the
  `PipelineStage` seam that exists for the GPU port.
- **Export is sequential**, deliberately: pipeline stages are already rayon-parallel internally, so
  parallel export would contend for the same cores while multiplying peak memory.
- **Demosaic is ~93% of a full-resolution decode**, and the Phase 5 `wgpu` port would **not** fix it
  — that time is inside libraw's `dcraw_process`, entirely upstream of the stages wgpu would touch.

---

## App and UI

- **`main.rs` is still ~1,345 lines** — state types, decode functions and lifecycle orchestration. A
  plausible third split, deliberately not proposed: unlike the canvas split, it is not where the
  next feature lands. Do it when it hurts.
- **The in-window menu bar has never been run**, only unit-tested — there is no Linux machine here
  and macOS never shows it. Flip the `!cfg!(target_os = "macos")` guard in `shell.rs` to look at it.
  Relatedly, **`native_menu.rs` can rot silently**, since CI never compiles it.
- **No user-defined keyboard shortcuts.**
- **The accent colour reaches only egui's own widgets.** The crop overlay, histogram and thumbnail
  selection border are painted by hand with hardcoded colours — most visible in **light** mode,
  which has barely been looked at.
- **Export has no filename templating** — `<stem>.<ext>` into a chosen folder.
- **Presets: no import/export, no folders, no rename.**
- **⚠ Edit-accurate rendered thumbnails.** The grid shows the camera's embedded JPEG, so it never
  reflects your edits. ⚠ **Virtual copies (M3.4) raised the stakes on this**: every copy of one file
  shows a *pixel-identical* thumbnail, so a black-and-white copy appears in colour and is told apart
  only by its badge and name. Deliberately kept out of M3.4 rather than pulled in — it is a
  separable feature (render per entry, cache it, invalidate on every edit) that does not need copies
  to exist, and doing both at once would mean two large simultaneous changes to the Library, which
  is the area with this project's worst defect record. Doing it fixes the copy case *and* every
  ordinary photo.
- **Library:** no on-disk thumbnail cache, no edit-accurate rendered thumbnails (the grid shows the
  camera's embedded JPEG), no ratings, no subfolders, no vertical filmstrip.
- **A nicer decode placeholder** — run the pipeline over the thumbnail for an instant low-res
  preview, instead of showing the camera JPEG.

---

⚠ **This file was seeded from the follow-ups already recorded in `CLAUDE.md` and is not a complete
sweep of that document.** When something is deferred, add it here *and* leave the detail in its
CLAUDE.md section.

## EXIF/IPTC metadata editing (deferred 2026-08-26 — blocked on a DESIGN question)

**Status:** ⛔ blocked on a decision, not on effort. Split out of M4.2 before designing it.

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

## Immich integration (recorded 2026-08-26 — wanted, likely its own milestone)

**Status:** 🟡 wanted, not blocked. Slices 1 and 2 are the agreed start; slice 3 is a follow-up.

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
3. **Two-way sync** — push edited results back as new assets, reconcile Immich albums with
   collections, keep marks in step with favourites. **Much larger**, and where the design questions
   live: what is authoritative, what happens on conflict, what happens offline.

### ⚠ Verify this FIRST, before designing anything

**Does Immich's API serve the true original for RAW files, or only a converted preview?** If it
only returns a JPEG, slice 1 loses most of its point and the whole shape changes. One probe
settles it; do not assume.

### ⚠ Three honest costs

- **Credentials are a new kind of surface for this project.** Rustroom has never stored a secret.
  An API key in `settings.json` in plaintext is the easy path and probably not the right one; the
  OS keychain is right and is platform-specific work.
- **Duplicated storage.** A cached RAW is a second copy — fine for a working set, needs an eviction
  policy if it becomes a library. `BufferCache`'s LRU is the precedent.
- **It expands what Rustroom is.** Every planning note says "local files, originals never modified,
  Linux first". A network dependency is a genuine change of character — not wrong, but worth
  deciding deliberately rather than sliding into.

## Film negative inversion (recorded 2026-08-26 — a FEATURE, not a plugin)

**Status:** 🟡 wanted, not blocked. A `PipelineStage` like any other.

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

**Status:** ⛔ deferred on judgement, revisit **after Phase 5**.

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

## Contributor-facing documentation (recorded 2026-08-26 — the cheap half of "open source")

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


## Dialog sizing and layout (recorded 2026-08-27 — reported from use)

**Status:** 🟡 wanted, awaiting effort rather than a design question. Reported as *"all pop up
windows in general are very small and cramped."*

Every dialog is an `egui::Modal` laid out with whatever width its content happens to want, and the
result is uniformly tight: the rule editor's rows wrap, the export dialog's hint text crowds its
controls, and the Information dialog's `label: value` rows have no column alignment.

⚠ **This is a `theme` job, not a per-dialog one.** Nine dialogs each grown their own spacing is how
they got here; fixing them one at a time reproduces exactly that. What the theme is missing is a
**dialog convention** the way `theme::button_row` already encodes the confirm/dismiss one:

1. A **minimum width** per dialog class — a confirmation is narrow, an editor is wide — set once,
   so no dialog has to remember.
2. **Padding inside the modal frame.** `egui::Modal`'s default frame is tight against the window
   edge, which is most of the "cramped" feeling.
3. **Aligned label columns** where a dialog is a list of `label: value` rows (Information,
   Settings, Export). `egui::Grid` does this and none of them use it.
4. A **max height with an inner `ScrollArea`**, so a long rule or a long preset list cannot push a
   dialog past the bottom of the screen.

⚠ **The compact metrics are not the cause and should not be reverted.** `theme::apply` tightens
`item_spacing` and `slider_rail_height` deliberately, for the *panel* — a panel of forty sliders is
where vertical space is scarce. A dialog is the opposite case and wants its own metrics.

⚠ **Appearance is exactly what the headless test harness cannot see** (its own recorded caveat), so
this is a change to be judged by looking at it, one dialog at a time, with the app running.

## Rearranging the Library panel's sections (recorded 2026-08-27 — asked for in use)

**Status:** 🟡 wanted, small-to-medium, no design question. Asked as *"it would be nice to be able
to rearrange the left outer panel, e.g. by dragging folders below collections."*

The five sections — Folders, Collections, Smart collections, Keywords, Dates — are collapsible and
persisted (`Settings::panel_sections`), but their **order is literal code**: five `section(..)`
calls in `collections_panel::show`.

⚠⚠ **The blocker is not the drag; it is that the sections are not data.** Each body is a distinct
closure over different `app` fields, so there is no list to permute. Making it draggable means
first making the order a value — `Vec<PanelSection>` in `Settings`, with a `match` from the enum
to the body — and only then adding the gesture. The `match` is what keeps the bodies as they are.

⚠ **A latent trap the reordering already sprang once:** `picked` and `selected` used to be locals
of the Collections block that the Keywords block happened to see, because they were sequential
statements in one closure. Moving Smart between them broke it, and the compiler caught it. Anything
shared between sections must be read **before** the panel, or the order cannot be a value at all.

⚠ egui's `dnd_drag_source` / `dnd_drop_zone` exist and are the right primitives; nothing needs
writing from scratch.

## Undo for catalog deletions (recorded 2026-08-27 — asked for in use)

**Status:** 🟡 wanted, and it is a **design question**, not only effort. Asked as *"it should have
a history, such that if one deletes something, e.g. a collection or keyword, that can be
reverted."*

Four destructive catalog actions have **no undo at all** today:

| action | what it destroys |
|---|---|
| Delete a collection | its membership rows |
| Delete a keyword | ⚠ **its whole subtree** and every assignment of any of them |
| Delete a smart collection | its rule |
| Forget a folder | the folder's photo rows, and with them **every rating, flag, label and keyword assignment** |

⚠⚠ **The precedent is `PresetStore`'s trash, and it does not transfer cleanly.** A deleted preset
is a *file*, so the trash is a directory and restoring is a rename. These are **rows across several
tables** — `keywords`, `photo_keywords`, `collections`, `collection_photos`, `photos` — and a
delete cascades. So "the trash" here is either a serialized snapshot of what was removed, or a
`deleted_at` column and every query learning to exclude it. **That choice is the design question**,
and the second option touches every existing query, which is why it should not be picked casually.

⚠ **`ActionLog` is the right home once the payload exists.** It already interleaves photo edits
with preset deletions chronologically, and `ActionKind::PresetDelete { name }` is exactly the shape
a `CollectionDelete { .. }` would take — the entry names *which* stack to pop, and the payload
lives where it already lives. ⚠ Note `reachable()`: a catalog deletion is **global**, like
`PresetDelete` and `Sync`, not scoped to a photo.

⚠ **Forgetting a folder is the one that matters most and is the hardest.** It is the only action
that takes *user-created metadata* (ratings, keywords) with it, and it is deliberately confirmed
for that reason — but a confirmation is not an undo. ⚠ It is also the only one whose restore has to
recreate rows other tables reference **by id**, so a plain re-insert gets new ids and every
membership pointing at the old ones is still dangling. Restoring must preserve the ids.

⚠ **A cheaper first slice that is worth doing on its own:** deleting a **keyword** currently takes
its children with it, silently and with one click. Even without an undo, that deserves the
confirmation `forget_folder` has, naming the count. Same for a collection with members.

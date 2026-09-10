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

---

## ⚠⚠ 2026-09-01 — most of this file is now scheduled

The roadmap was re-planned on 2026-09-01 (see `03-plan-of-action.md` and `PARALLEL-WORK.md`). Items
below that were "deferred" are now **milestones with an order**, and this table is the map. An entry
still carries the reasoning that matters — read the entry, then build it under its milestone number.

| Backlog entry | Now |
| --- | --- |
| Creative 3-D LUTs | ✅ **M7.5, shipped 2026-09-04** and confirmed by use (after the handover fix). ⚠⚠ Was specced as W5 track A. M5.4b supplied the *seam*, not the answer: the blocker below — a `.cube` does not declare its input space — was still open, and W5-0 settled it. See `specs/2026-09-04-creative-luts-design.md`. |
| `ProfileToneCurve` / `ProfileLookTable` | **answered by M5.4b** — the same display-rendering question |
| Soft-proofing · gamut mapping / rendering intents | ✅ **M6.4 soft proofing shipped 2026-09-04**, confirmed by use; gamut mapping and rendering intents are *not* in it and stay deferred. `specs/2026-09-04-soft-proofing-design.md`. |
| EXIF/IPTC metadata editing | ✅ **M6.2, 2026-09-08** — edited in the catalog, embedded at export, in the sidecar |
| Export filename templating · output/print sharpening | ✅ **M6.1a** / ✅ **M6.1d**, both 2026-09-01 |
| Mask intersection and subtraction | ✅ **M7.1c, 2026-09-10** — confirmed by use; Add / Subtract / Intersect between masks, and **M7.1 is complete** |
| Reorder a mask's components | ⬜ **wanted (user's call, 2026-09-10)** — order is observable, and the list is append-only today |
| A better interpolator for the geometric stages | ✅ **M7.3a, 2026-09-01** — a `Settings` choice defaulting to Catmull-Rom, both samplers |
| Fuse perspective into `GeometryStage` | **M7.3b** — ⬜ still open (2026-09-08) |
| Automatic vertical detection (Hough) | ✅ **M7.4, 2026-09-04** — reshaped into `Auto` / `Refine guides` |
| Auto-mask brush · ~~dehaze bright subjects~~ · ~~negative Texture~~ · ~~Clarity halos~~ · ~~grading in masks~~ | **M7.6** — grading in masks ✅ 2026-09-05; negative Texture and Clarity halos ✅ 2026-09-02 (W3, struck late); dehaze bright subjects ✅ 2026-09-08; auto-mask ⬜ |
| ~~Lens correction row batching~~ · sequential export · incremental splatting | **M8.1** — row batching ✅ 2026-09-01 (1178 → 309 ms); the other two ⬜ |
| The `wgpu` seam | ✅ **M8.3a, 2026-09-10** — one stage across the seam, an equivalence harness, and the cost measured: **3.1–4.7× slower**, bit-exact. Nothing in the app uses it. ⚠ M8.3b's first task is **not** porting a stage: hunt a zero-copy upload (the transfer is the floor at ~6 ms against 0.04 ms of compute) and measure transcendental divergence. `specs/2026-09-10-gpu-seam-design.md` |
| GPU: a resident chain and HSL | ✅ **M8.3b, 2026-09-10** — N stages share one round trip, and global HSL runs on the GPU at **~2×** (CPU ~21 ms against ~10) behind a Settings toggle, diverging by 0.00034 of an 8-bit level. ⚠ **Masked HSL is NOT accelerated** — it lives inside `MaskStage`, and it is the case reported in use as slow. `specs/2026-09-10-resident-gpu-tail-design.md` |
| GPU: spatial stages and run-merging | ✅ **M8.3c, 2026-09-11** — sharpening as compute shaders on shared scratch planes, consecutive GPU stages merged into **one** round trip, `gpu_hsl` → `gpu_tail`. **1.52×** on hsl + sharpening; ⚠⚠ the merge itself is worth **10.17 ms** (merged 21.84 ms against split 32.01), so without it a second stage was worth 1.18× — run-merging is what makes it pay at all. ⚠ **An active colour grade splits the run and takes the win to 1.04×.** `specs/2026-09-10-spatial-gpu-design.md` |
| GPU: local contrast | ⬜ Deliberately out of M8.3c's scope — it needs **three** blur kinds (`box_blur`, `wide_blur`, the guided filter) where sharpening needed one. ⚠ It is also **adjacent to nothing accelerated**: `local_contrast` runs before saturation and hsl, so porting it alone buys a third round trip rather than joining a run. |
| GPU: colour grading | ⬜ ⚠⚠ **Measured as the highest-value next port, and this was not predicted.** An active grade sits **between** `hsl` and `sharpening`, so it splits the merged run and costs 1.52× → **1.04×**. Accelerating it would restore one chain across all three and is arguably worth more than M8.3d. |
| GPU: shader and pipeline caching | ⬜ The shader module, bind-group layout and compute pipelines are rebuilt on **every encode**. The bench's round-0 column shows the cost (~150 ms first render, and a visible outlier thereafter). Contained, and nothing depends on it. |
| GPU: the mask loop · the prefix | ⬜ **M8.3d–e** — ⚠ M8.3d is a **different mechanism**, not another table entry: a mask's stages live inside `MaskStage`, which the tail sees as one opaque stage, so `accelerate` cannot reach them by name. M8.3e is where the 156.6 ms native prefix one-off is. |
| On-disk thumbnail cache | ✅ **M8.2, 2026-09-10** — a folder opened before fills in at **1.26 ms per entry against 190.41 ms**, 151×, for ~60 KB per photo. Caches the extracted **preview**, not the finished thumbnail. ⚠ Nothing for the *first* open, which stays ~26 s per 830 entries. `specs/2026-09-10-thumbnail-cache-design.md` |
| Decode a full-size JPEG at 1/8 scale for thumbnails | ⛔ **MEASURED AND REJECTED, 2026-09-10 — and this entry's own earlier claim was wrong.** It said DCT-scaled decode was "the only lever that moves the cold folder open", implying a large win. Measured: **1.6×** (222.3 → 143.2 ms), not the ~8× the framing suggested. Three reasons, each checked rather than argued. **(1) Entropy decoding is irreducible** — Huffman work is proportional to the compressed data, not the output size, so DCT scaling skips only the IDCT, upsampling and colour convert. Of a 26 Mpx camera JPEG's 226.9 ms, decode is **178.1 ms (78%)** and the resize only 43.8 ms. **(2) `image` 0.25 cannot do it at all** — its backend is `zune-jpeg`, whose 1×1/4×4 IDCT paths are a sparse-coefficient optimisation producing full-size blocks, not an output-scale knob. The only Rust crate that offers `scale()` is `jpeg-decoder`, which is a **slower decoder than the one already in the tree**, eating much of the gain. **(3) The embedded EXIF thumbnail is 160×120** — far too small for a 512 px cell, so the trick that makes RAW cheap does not transfer. ⚠⚠ **And M8.2 removed most of the motive**: the cold open now happens **once per folder, ever**, so this would spend a second JPEG decoder in the tree to turn a one-off ~26 s into ~19 s. Reopen only if a faster scaled decoder appears in `image` itself. |
| Replace libraw's demosaic | ✅ **M8.4, 2026-09-10 — decided OUT of scope**, and the default tier moved `Fast` → `Better` instead. `Better` sits within 0.22–0.30 of an 8-bit level of `Best` on foliage and is indistinguishable from it at 4×, so a Markesteijn/AMaZE-class reimplementation buys nothing measurable. ⚠ Costs ~2.9 s → ~7.5 s per photo opened. `specs/2026-09-10-demosaic-decision.md` |
| `main.rs` is too large | ✅ **W0, 2026-09-01** — 4,375 → 681 (917 as of 2026-09-08); `canvas.rs` 1,890 → 7 modules |
| The in-window menu bar has never been run | **M9.2** |
| Immich integration | **M9.1** |
| Contributor-facing documentation | **M9.3** |
| A plugin system | **M9.5** |

⚠⚠ **One entry changed shape rather than being scheduled: EXIF/IPTC editing.** It was blocked on
"modify the original" versus "a second XMP sidecar". **Writing metadata only into the *exported*
file is neither** — the original is untouched, no new sidecar exists, and it is exactly what M6.1b
needs anyway. Metadata is edited in the catalog and embedded at export; XMP sidecars become M6.3's
problem, where they belong. **The design question that blocked this is answered; do not re-open it.**

Not scheduled, and deliberately: the two C2/C3 colour-management oracle notes, per-photo camera
profile, `CameraCalibration`, brush-strokes-shift-under-lens-correction, presets import/export,
user-defined shortcuts, the accent colour reaching hand-painted widgets, subfolders, vertical
filmstrip, and the nicer decode placeholder. These stay here as recorded reasoning.

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
- ✅ **Soft-proofing — shipped as M6.4, 2026-09-04.** As recorded: the preview still cannot show colour outside sRGB even when exporting P3,
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

### ⚠⚠ Anchor strokes and spots in sensor space (recorded 2026-09-03 — blocked on a MISSING MAP)

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

- **Reorder a mask's components** (recorded 2026-09-10, **wanted** — user's call: *"the reordering
  is something we should do at some point"*). Components are **append-only, delete-any** today, and
  **order is observable**: the fold runs left to right, so subtract-then-intersect is not
  intersect-then-subtract. ⚠ The gesture is already solved — `panel_order::{drop_index, moved}` is
  the drag-to-reorder logic the Library panel's sections use, pure and tested, including a
  permutation property over every from/to pair. The work is a second call site plus deciding
  whether a reorder is one undo step. ⚠ Cheap, and **not** blocked on a design question, which is
  what distinguishes it from most of this file.

- **Composites are one level deep.** A refinement is a *shape*, never another composite, so
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
- **Brush strokes shift when lens corrections are toggled.** Strokes are anchored in
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

- **Fuse perspective into `GeometryStage`** (recorded 2026-08-31). One resample instead of two when
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
`CLAUDE.md`'s "progressive fit resolution" SECTION BEFORE RE-DERIVING ANY OF IT.** That feature
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

### ⚠ What is left in `HslStage` after the hue LUT (recorded 2026-09-05)

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

- **Brush: incremental splatting.** Region-scoped coverage brought a paint frame to ~16.6 ms, of
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
- **Two Oklab round trips** when HSL and colour grading are both active. Fusing them would break the
  `PipelineStage` seam that exists for the GPU port.
- **Export is sequential**, deliberately: pipeline stages are already rayon-parallel internally, so
  parallel export would contend for the same cores while multiplying peak memory.
- **Demosaic is ~93% of a full-resolution decode**, and the Phase 8 (formerly 5) `wgpu` port would **not** fix it
  — that time is inside libraw's `dcraw_process`, entirely upstream of the stages wgpu would touch.

- **⚠ Exports are not bit-reproducible across runs** (found 2026-09-01 while validating M7.3).
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
- **The in-window menu bar has never been run**, only unit-tested — there is no Linux machine here
  and macOS never shows it. Flip the `!cfg!(target_os = "macos")` guard in `shell.rs` to look at it.
  Relatedly, **`native_menu.rs` can rot silently**, since CI never compiles it.
- **No user-defined keyboard shortcuts.**
- **The accent colour reaches only egui's own widgets.** The crop overlay, histogram and thumbnail
  selection border are painted by hand with hardcoded colours — most visible in **light** mode,
  which has barely been looked at.
- ~~**Export has no filename templating**~~ ✅ **M6.1a, 2026-09-01.**
- **Presets: no import/export, no folders, no rename.**
- **Library:** no on-disk thumbnail cache, no subfolders, no vertical filmstrip.
- **A nicer decode placeholder** — run the pipeline over the thumbnail for an instant low-res
  preview, instead of showing the camera JPEG.

---

⚠ **This file was seeded from the follow-ups already recorded in `CLAUDE.md` and is not a complete
sweep of that document.** When something is deferred, add it here *and* leave the detail in its
CLAUDE.md section.

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

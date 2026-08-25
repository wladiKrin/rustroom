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
- **Colour grading does not reach masks.** `LocalAdjustments` is per-pixel-only and a grade is
  global look. Also **no visual feedback of the grading range boundaries** on canvas or histogram.

---

## Performance

- **Brush: incremental splatting.** Region-scoped coverage brought a paint frame to ~16.6 ms, of
  which ~9 ms is rasterization. ⚠ The remaining cost is **dab area, not frame area** — at 1:1 a dab
  is 125 px across — so incremental splatting is the lever if this ever needs to be faster.
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
- **Library:** no on-disk thumbnail cache, no edit-accurate rendered thumbnails (the grid shows the
  camera's embedded JPEG), no ratings, no subfolders, no vertical filmstrip.
- **A nicer decode placeholder** — run the pipeline over the thumbnail for an instant low-res
  preview, instead of showing the camera JPEG.

---

⚠ **This file was seeded from the follow-ups already recorded in `CLAUDE.md` and is not a complete
sweep of that document.** When something is deferred, add it here *and* leave the detail in its
CLAUDE.md section.

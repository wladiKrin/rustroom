# M3.1a — the masking architecture, proven on a linear gradient

**Status:** spec, 2026-08-13.

## Scope

Phase 3's first milestone, **M3.1 — local adjustments / masking**, is too large for one spec.
It is sliced into three, each with its own spec → plan → implement cycle:

| | what | closes |
|---|---|---|
| **3.1a** — this spec | The whole architecture, proven end to end on **one analytic mask**: linear gradient. Model, serialisation, region-aware rasterisation, pipeline composition, canvas handles, export. | — |
| **3.1b** | Radial gradient. Should be nearly free if 3.1a got the shape right; that is the test of whether it did. | — |
| **3.1c** | Brush. Genuinely more: strokes must be stored, and stroke radius scales with `RenderScale` as sharpening's does. | ⚠ **M3.1** — the milestone's own "Done when" names dodge/burn with a brush. |

Linear gradient goes first because it is the simplest primitive that still forces every hard
decision: coordinate space, region-awareness, pipeline composition, cache placement, and how
masks interact with presets.

### Why M3.1 before the rest of Phase 3

- **M3.2 lens corrections** needs a new C dependency, lensfun — the same build-system risk
  libraw was.
- **M3.3 colour grading & LUT profiles** has a hidden prerequisite: ⚠ **real colour management
  does not exist.** lcms2 is in the build but emits one static sRGB profile and nothing else —
  no input profiles, no conversions, no working-space policy.
- **M3.4 virtual copies** is the smallest and most independent; it touches `edit-model` and the
  sidecar only.

M3.1 is also what everything recent was preparation for — the `Region` seam and the canvas
split — and it is what unblocks the parked layout work, because it introduces the mask list
that the layout currently has nowhere to put.

## Decisions taken

| Question | Decision |
|---|---|
| What a mask can carry | **The per-pixel tail adjustments** — temp/tint, exposure, contrast, highlights/shadows/whites/blacks, saturation/vibrance. |
| Pipeline composition | **A `MaskStage` that owns sub-stages and blends by coverage.** |
| Where in the chain | Tail, **after HSL, before vignette**. |
| How many masks | **A list**, from the start. |

### Why that adjustment set, precisely

It is not an arbitrary subset. It is exactly the set where **"apply the stage, then blend the
result by coverage" is both correct and cheap**, because each output pixel depends only on its
own input pixel.

⚠ Spatial stages break that model: a blur inside a mask reads pixels *outside* it, so
"masked noise reduction" needs its own decision about what it even means, and costs a
whole-image pass before the blend. HSL, local contrast, dehaze, noise reduction and sharpening
are therefore **out of scope** — that is a research problem hiding inside a plumbing task.

Geometry and vignette are excluded for a different reason: geometry runs in the prefix and
changes the frame itself, and the vignette is already position-dependent.

## Design

### 1. Data model

`Edits` gains one field:

```rust
pub masks: Vec<MaskedAdjustment>,
```

```rust
/// One mask and the adjustments it carries.
pub struct MaskedAdjustment {
    pub mask: Mask,
    pub adjustments: LocalAdjustments,
}

/// The mask primitives. 3.1b adds `Radial`, 3.1c adds `Brush`.
pub enum Mask {
    Linear(LinearGradient),
}

/// A graduated filter: coverage ramps from 0 at `from` to 1 at `to`, measured along the axis
/// between them and constant perpendicular to it.
pub struct LinearGradient {
    /// Normalised frame coordinates, as `CropRect` uses — so the mask survives a resolution
    /// change, and means the same thing on the preview and the export.
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub invert: bool,
}

/// What a mask can carry: the per-pixel adjustments, and only those.
pub struct LocalAdjustments {
    pub white_balance: WhiteBalance,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub saturation: f32,
    pub vibrance: f32,
}
```

**A list, not an `Option`.** One mask would be a prototype needing rework at 3.1b; the
composition machinery has to handle N regardless.

⚠ `LocalAdjustments` repeats field *names* from `Edits`, and that is accepted — they are
genuinely different sets, and merging them would either allow nonsense (a mask carrying a
crop) or force `Edits` into nested groups, which breaks every existing sidecar.

**The duplication that would actually hurt is the stage-construction logic**, and that is
shared rather than repeated:

```rust
/// Build the per-pixel stages for one set of adjustments.
///
/// One list, two consumers: the global tail calls it with the values from `Edits`, and every
/// mask calls it with its own. A stage added here reaches both.
fn per_pixel_stages(a: &LocalAdjustments) -> Vec<Box<dyn PipelineStage>>;
```

This is the same guard `build_prefix_steps` already provides for the prefix.

### 2. Composition — `MaskStage`

```rust
pub struct MaskStage {
    mask: Mask,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
}
```

`apply` clones the input, runs `stages` over the clone, then writes back
`out = base·(1 − c) + adjusted·c` per pixel, where `c` is the mask's coverage at that pixel.

**Every existing stage is reused completely unchanged.** Nothing learns about masking, and the
`PipelineStage` seam stays opaque — which is what the eventual GPU port depends on.

**Rejected: fusing the adjustments into one per-pixel closure.** Faster, one pass, no clone —
but it requires every stage to expose its per-pixel function, which means a new trait and the
end of stages being opaque. That is precisely the seam `CLAUDE.md` says not to leak CPU
specifics through.

**Deferred, deliberately: a bounding-box optimisation.** Running the sub-stages only inside the
mask's bbox is the natural speed-up, but a linear gradient's bbox is usually the whole frame,
so it buys nothing *here*. Radial and brush masks are often small, so 3.1b/c will want it —
and `MaskStage` is built so it drops in without changing callers.

### 3. Placement in the chain

Tail, after `HslStage`, before `VignetteStage`:

```
… → tone curve → local contrast → saturation → hsl → [masks] → vignette → sharpening → output
```

- **Tail, not prefix** — dragging a mask handle changes the image every frame, and the tail is
  the ~14 ms half. A mask in the prefix would invalidate the noise-reduction checkpoint on
  every mouse move.
- **After the global per-pixel block** because a mask is the same kind of operation, localised;
  it should act on the global look rather than be re-mapped by it.
- **Before vignette and sharpening** because both have earned their end-of-chain positions:
  the vignette's "+100 = exactly white" endpoint depends on running after the tone curve, and
  sharpening must not amplify anything a later stage would then stretch.

### 4. Region-awareness — the load-bearing part

Masks are defined in **frame** coordinates, but the 1:1 view renders a **crop** of the frame.
So `MaskStage` takes `Option<Region>` and resolves coverage against the full frame, exactly as
`VignetteStage` does — including the rule that a region's own dimensions are *not* the frame's.

⚠ Get this wrong and masks slide as you zoom, which is the kind of bug that looks like a
rendering glitch rather than a coordinate error. The guard is the property test that already
exists for the vignette, extended to masks:
`a_masked_region_render_matches_the_whole_render_cropped`.

### 5. Presets, copy/paste and sync

`EditGroups` gains a **seventh group, "Masks"**, and ⚠ it is **off by default** — the same
treatment geometry gets, for the same reason: a gradient positioned for one composition is
meaningless on another, so carrying it across photos is usually wrong.

⚠ `Edits::merge_from` destructures `Edits` exhaustively, so the new field is a **compile
error** until someone assigns it a group. That is what that destructure exists for.

### 6. UI

- **Canvas**: two draggable endpoints joined by an axis line, drawn over the image. Dragging
  either endpoint or the line re-renders. One history entry per settled drag, via the
  `drag_stopped` rule the sliders already follow.
- **Panel**: a mask list — add, select, delete — and the selected mask's `LocalAdjustments`
  sliders.
- ⚠ **The list's placement is provisional.** It is exactly the new top-level concept that
  caused the layout work to be parked until after M3.1, so it goes somewhere plausible now and
  gets its real home in the layout pass. Recorded here so that placement is not mistaken for a
  considered decision.

### 7. Rendering the mask overlay

While a mask is selected, its coverage is shown as a translucent tint over the image so you
can see what you are affecting. ⚠ Drawn by the canvas, **not** by `MaskStage` — the pipeline
produces the photograph, not annotations, and mixing the two would put the overlay into
exports.

### 8. ⚠ Build it headless first

The plan should sequence this as **model → pipeline → goldens → CLI-verified, and only then
UI**. That is not a preference; it follows from two things already true of this codebase:

- `pipeline` has **zero GUI dependency** and is driven headless by the CLI and the tests, which
  the crate-layout rules state as a hard constraint.
- The sequencing notes call the CLI "our permanent test harness — it makes the engine testable
  without the GUI".

So a masked edit can be written as a JSON sidecar and rendered with
`rustroom render --edits mask.json` **before a single line of canvas code exists**. The entire
architecture — coordinate space, region-awareness, composition, blending, export — is provable
against goldens with no egui in the picture. The UI then becomes a way to author a file whose
meaning is already pinned.

That also means a failure in the canvas cannot be confused with a failure in the model, which
is the mistake that cost four diagnostic rounds during the progressive-sharpen work.

## Testing

- **Coverage is a pure function of position** — `LinearGradient::coverage(x, y, w, h) -> f32`,
  testable with no egui and no image: endpoints, midpoint, the perpendicular-invariance
  property, and `invert`.
- **`a_masked_region_render_matches_the_whole_render_cropped`** — the load-bearing one, and a
  direct descendant of the vignette's. Asserts a region render equals the whole render cropped,
  bit-identically, with a mask active.
- **`per_pixel_stages` is shared** — a test that a mask carrying exposure produces the same
  pixels as the global exposure would, where coverage is 1.
- **Goldens** for a masked exposure on the existing chart, and ⚠ **the existing goldens must
  stay byte-identical**, which is the proof the stage is inert with no masks.
- **`merge_from`** — the exhaustive-partition tests already in `edit-model` extend to the new
  group for free.
- **Back-compat**: a sidecar written before masks loads with an empty list, via
  `#[serde(default)]`.

## Accepted caveats

1. **One full-image clone and full-image stage runs per mask**, even where coverage is zero.
   The bbox optimisation is deferred to 3.1b/c where it pays.
2. **Per-pixel adjustments only.** No masked noise reduction, sharpening, clarity, dehaze or
   HSL — each needs its own decision about what "masked" means for a spatial operation.
3. **The mask list's placement is provisional**, pending the layout pass after M3.1.
4. **No mask intersection, subtraction, or range/colour masks.** Lightroom has all three; they
   are separate features on top of this architecture, not part of it.
5. **M3.1 does not close here.** The milestone's own criterion names a brush, which is 3.1c.
6. **Masks do not transfer by default** on paste, preset or sync — deliberate, and overridable
   per operation via the group checkbox.

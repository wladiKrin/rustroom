# M7.1c — boolean mask operations

**Date:** 2026-09-09
**Branch:** `m7.1c-boolean-masks`
**Status:** design, approved

## 1. What this is

`docs/BACKLOG.md:305` states the gap in one line: **"a radial cannot be cut out of a linear"**.
Today `Edits.masks` is a flat `Vec<MaskedAdjustment>`, each `{ mask, adjustments }`, and every one
is blended independently over the running image — so the only compositing that exists is a brush's
own `erase`, *within* one brush.

The two gestures this exists for:

- a **radial** over a face **minus** a brushed-out pair of eyes;
- a **linear** sky gradient **intersected** with a luminance range so it stops at the ridge.

Plus a third the user asked for and the two above do not cover: a **union**, so two overlapping
shapes carrying one adjustment apply *once* where they overlap rather than twice.

With this, Phase 7 has only **M7.3b** left in it.

## 2. Representation — a refinement list on the entry

```rust
/// How a refinement composites against the coverage built so far.
pub enum MaskOp { Add, Subtract, Intersect }

/// One shape composited onto a mask's coverage.
pub struct Refinement { pub op: MaskOp, pub mask: Mask }

pub struct MaskedAdjustment {
    pub mask: Mask,                                   // the base — unchanged
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refine: Vec<Refinement>,                      // new
    pub adjustments: LocalAdjustments,
}
```

⚠⚠ **`Mask` gains no variant and stays non-recursive.** `analytic_coverage`'s exhaustive match,
`label`, `invert_mut` and all five variants are untouched, and so is every sidecar on disk.

**The two alternatives, and what each costs.** `CLAUDE.md`'s NEXT ACTION note named both:

- A **recursive combining variant** (`Mask::Combine { op, a: Box<Mask>, b: Box<Mask> }`) makes the
  maths trivial and gives arbitrary nesting free — but the panel must render a tree, every hit-test
  and drag must say *which* sub-mask a handle belongs to, and the brush-coverage index becomes a
  path rather than an integer.
- A **per-entry op** (each `MaskedAdjustment` carries an op against the previous entry) keeps the
  flat list and is literally what Lightroom's chips do — but it conflates *"a mask"* with *"a step
  in building one"*, so the `adjustments` of every non-`New` row become dead state the UI must hide
  and the pipeline must ignore.

The refinement list keeps **one adjustment set per composite** (so no field is ever meaningless),
keeps `Mask` flat, and makes the panel two levels rather than a tree. What it costs is named in §5
and §7: the brush-coverage index goes from `i` to `(i, j)`, and the canvas gains a selected
component.

⚠ **`skip_serializing_if = "Vec::is_empty"` follows the `lens_correction` precedent**: a mask with
no refinements writes the byte-identical sidecar it always did. `#[serde(default)]` on the
container then makes every existing sidecar load with an empty list.

⚠ **`op` is required, not defaulted.** A refinement whose op is missing has no defensible reading —
guessing one silently changes the picture — so a malformed one fails the load, exactly as an
unknown `Mask` tag does.

## 3. The coverage algebra

Components fold **left to right** over the base, one running coverage `a` in `0..=1`:

| op | rule | precedent |
|---|---|---|
| `Add` | `a + (1−a)·b` | **exactly** `brush/raster.rs:371`, the across-strokes paint rule |
| `Subtract` | `a·(1−b)` | **exactly** `brush/raster.rs:367`, `erase` |
| `Intersect` | `a·b` | the third member of that same algebra |

⚠⚠ **Two of the three are already shipped rules, and that is the whole argument for the product
algebra over `min`/`max`.** A brush composites dabs within a stroke with `max` and strokes with
`over`/`erase`; picking `min`/`max` here would make **subtract-between-masks disagree with
erase-within-a-brush at every partial coverage** — a second rule wearing the first one's name, which
is the failure this file records for the Whites slider and for `hardness`-versus-`feather`.

⚠ **The trade, stated rather than discovered:** the product rules are not idempotent. `0.5 ∩ 0.5` is
`0.25` and `0.5 ∪ 0.5` is `0.75`, where `min`/`max` would give `0.5`. On a feathered gradient
intersected with a luminance range that means the ramp darkens where both are partial — which is
the correct reading of *"both conditions hold"* under independence, and is what Lightroom's
intersect does, but it is the thing most likely to be reported as "the intersection is too weak".
**If it is, the fix is a design conversation about the algebra, not a constant to tune.**

**Order is observable** and is the list order: `subtract then intersect` is not `intersect then
subtract`. Components are **append-only, delete-any** in this milestone; see §11.

⚠ Each component carries its own `invert` inside its own `Mask`, applied by that component's own
coverage before the fold sees it. There is no composite-level invert.

## 4. Pipeline — `MaskStage`

### 4.1 Resolution stays at construction

`MaskStage`'s `source: CoverageSource` becomes:

```rust
base: CoverageSource,
refine: Vec<(MaskOp, CoverageSource)>,
```

still resolved **once, in `resolve_source`**, so `apply` matches outside the pixel loop — the shape
a brush forces and the property that enum was created for. The `CoverageSource` enum stays
exhaustive, so a sixth `Mask` variant is still a compile error there.

### 4.2 The pixel loop pays for the base once, not per component

⚠⚠ **`lightness(base)` and `hue_chroma(base)` are computed at most once per pixel and shared across
every component that wants them.** Three colour-range components must cost **one** Oklab round trip,
not three — `docs/BACKLOG.md`'s decomposition puts that round trip at 42% of a range mask's coverage
cost, so getting this wrong would make the feature's cost linear in a component count that ought to
be nearly free. Computed lazily: a composite of two analytic shapes pays neither.

⚠ **Every component reads the same `base` pixel** — the buffer this stage was handed. That answers
the question the NEXT ACTION note flags (*"an intersection has to decide which base both halves
read"*) **by construction**: it is the same base a single mask reads today, so no component can
chase its own effect and there is no second rule to keep in step.

### 4.3 ⚠⚠ The `is_inert` short-circuit becomes a correctness hazard

Today:

```rust
if coverage.as_ref().is_some_and(|c| c.is_inert()) { return image; }
```

That is an **optimisation** today — an inert map reads 0 everywhere and blending at 0 is bit-exactly
the identity. With refinements it stops being one: an inert base stays inert under `Subtract`
(`0·(1−b) = 0`) and under `Intersect` (`0·b = 0`), and **does not** under `Add`
(`0 + 1·b = b`).

**Rule:** short-circuit only when the base map is inert **and** no refinement's op is `Add`.

⚠ It gets its own test, and the test's sabotage is *dropping the `Add` clause* — a mask whose base
is an unpainted brush and whose refinement is an added radial must still render the radial. That is
the exact defect this rule exists to prevent and nothing else in the suite can see it.

### 4.4 What does not change

- **Placement.** No new stage, no new `PrefixStepKey`, no `Region` work, no margin change.
- **`tail_kernel_radius`.** It sums over masks because each mask's *adjustments* contribute reach; a
  refinement carries no adjustments, so a composite's reach is its base mask's reach. Unchanged, and
  the master region invariant is therefore unchanged.
- **Groups and transfer.** `merge_from` destructures `Edits`, not `MaskedAdjustment`, so a
  refinement travels with `MASKS_GROUP` under paste, preset and sync with no edit at all.

## 5. Coverage cache — the index goes from `i` to `(i, j)`

`BrushCoverage.maps` is `Vec<Option<Arc<CoverageMap>>>` indexed by **mask**, with `built` the
matching deposit slots. A composite can hold a brush in any component, so both become nested:
`maps[i][j]`, `built[i][j]`, `with_slots(&[usize])`, `deposit(i, j, map)`. `MaskStage`'s
`deposit: Option<(Arc<BrushCoverage>, usize)>` gains the component index.

`MaskCoverageKey` gains the component index and takes **that component's** brush.

⚠⚠ **The base identity is unchanged, and that is a consequence of §4.2 rather than a convenience.**
`BrushCache::base_id` truncates `edits.masks` to `i`, and every component of mask *i* reads the same
base — components before *j* change the composite's coverage, never the pixels — so `(upstream,
max_dim)` still identifies exactly what a gating component's coverage was computed against. **A
component index in the base identity would be wrong, not merely redundant.**

⚠ `BrushCache::cap` is `2 * masks.max(1)` for the two live rungs. It becomes `2 × total components`,
counted over the mask list, so a composite does not evict its own siblings on every zoom change.

## 6. ⚠⚠ `capture_projection` must carry the refinements

`PrefixStepKey::MaskedCapture` holds `capture_projection(m)` — the mask's **shape** plus
`noise_reduction` and `dehaze` only, so a masked-*exposure* drag cannot re-run a whole-buffer
denoise. A composite's shape now includes its refinements, so the projection must clone `refine`
alongside `mask`, or **dragging a subtracted radial would leave a stale masked-NR checkpoint on
screen**.

⚠ The compiler forces this: `capture_projection` builds a `MaskedAdjustment` **struct literal with
no `..`**, so the new field is an `E0063` there — the same forcing function
`build_pipeline_split`'s exhaustive literal already provides for `LocalAdjustments`. Its existing
test (`the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest`) gains a third
direction: a refinement change **must** move the key, a masked-exposure change must not.

## 7. The app

### 7.1 Panel — mode chips above the kind buttons

A segmented row sits above the five existing kind buttons:

```
[ New mask ] [ Add to ] [ Subtract from ] [ Intersect with ]
[ Add graduated filter ] [ Add brush ] [ Add radial filter ] [ Add luminance range ] [ Add colour range ]
```

⚠ **The chips read `Add to` / `Subtract from` / `Intersect with`, not the bare op names.** Every one
of the five kind buttons already begins with "Add", so a chip labelled `Add` sits directly above
five other things called Add and says nothing about which is which — the same complaint that turned
two adjacent sliders both called "Scale" into "Lens scale". The preposition also makes the two-click
gesture read as one sentence: *Subtract from … a radial filter*. `MaskOp::label()` lives beside the
enum so the panel cannot invent a second wording.

`New mask` is the default and the five buttons behave **exactly as today**; in any other mode they
append a `Refinement` to the selected mask instead. The chip state is tool state on
`RustroomApp` — `mask_add_mode: Option<MaskOp>`, `None` meaning New — beside the brush settings,
for the brush settings' reason: it applies to the next thing added.

⚠ With **no mask selected** the chips are disabled with a hover saying so, rather than silently
falling back to New. An invisible mode whose meaning depends on selection state is the class of
thing this project's record says bites; the chips are on screen saying which one is armed.

### 7.2 Component rows

The selected mask's row expands into an indented list: the **base** first, then each refinement,
each labelled `{op} {kind}` (`Subtract Radial`). One component is selected at a time —
`Session.selected_component: usize`, `0` being the base — and it is what the shape properties, the
`Invert` checkbox and the shape sliders below edit.

⚠ `selected_component` resets to 0 whenever `selected_mask` changes or the list shrinks, and
deleting a component clamps it — `selection_after_delete`'s rule, and for its reason: silently
selecting a *different* component means the next drag edits something nobody chose.

New code goes in **`crates/app/src/develop/panel/mask_components.rs`**; `masks.rs` is 647 lines and
past the ~600 marker and gains only the call.

### 7.3 Canvas

- The **selected component** owns the handles: `mask_tool::hit_test` / `apply_drag` are called with
  that component's `Mask`. ⚠⚠ **Their signatures do not change** — the canvas chooses which `Mask`
  to hand them, so the flat `Handle` enum keeps working and 3.1b's decision is not re-litigated.
- The other components draw as **dimmed outlines** so you can see what you are cutting against.
  `mask_overlay::paint_mask` was already split on *painting*, so this is a style parameter on an
  existing function rather than a second painter.
- ⚠ **`active_mask_handle` is cleared when `selected_component` changes**, or a handle grabbed on
  one component would keep dragging while another is selected — the stale-handle bug the mask
  deletion path already guards against.
- `mask_overlay.rs` is 675 lines; it gains the dimmed pass and the component routing and is **not**
  split here.

### 7.4 The tint

The tint shows the **composite** coverage — the thing you are actually judging — rasterised at
`TINT_MAX_DIM` as today, memoised on the whole composite rather than on one brush.

⚠⚠ **It is exact for position-dependent components and an approximation for pixel-dependent ones,
and this is not new.** `mask_tint` already computes an auto-masked brush's gate from the **displayed
frame** (`TintBase`), which is 8-bit sRGB after the display rendering rather than the linear working
buffer `MaskStage` reads — the documented degradation. A `Luminance` or `Colour` component inherits
exactly that, from exactly that surface.

⚠ Mask isolation helps more than it looks: while the tint is shown, `suppressed_mask` removes *this
mask's* adjustments from the view, so the displayed frame **is** this mask's base modulo the colour
space. That is the best stand-in available without rendering a second buffer, and it is worth
recording that it is a stand-in.

## 8. Where the new code goes

| new module | holds |
|---|---|
| `crates/edit-model/src/mask_ops.rs` | `MaskOp`, `Refinement`, `MaskOp::combine`, their tests |
| `crates/pipeline/src/stages/mask_coverage.rs` | `CoverageSource` **moved out of `mask.rs`**, `lightness`, `hue_chroma`, the composite resolver and the shared-per-pixel evaluator |
| `crates/app/src/develop/panel/mask_components.rs` | the mode chips, the component list, their tests |

⚠ Three files this touches are past the ~600 marker — `stages/mask.rs` (1071),
`develop/panel/masks.rs` (647), `develop/canvas/mask_overlay.rs` (675) — and **none is split
here**, on the user's call. Moving `CoverageSource` and the two pixel helpers out of `mask.rs`
makes it *smaller*, and it is a pure move provable by unchanged tests — the sequencing that turned
M3.1b's three predicted compile errors into one match arm.

`edit-model/src/lib.rs` (~4,000) gains `mod mask_ops; pub use mask_ops::*;` and the one field.

## 9. Tests

**Algebra** (`mask_ops`): each rule at 0, 1 and partial coverage; `Add` agreeing with the
across-strokes paint rule and `Subtract` with `erase` **by calling into the same arithmetic the
raster uses**, so the two cannot drift; the fold respecting list order.

**Pipeline** (`mask_coverage`, `mask`):

- a radial subtracted from a linear yields the cut-out coverage, measured rather than eyeballed;
- an intersect with a luminance range reads the **base** pixel, never the adjusted one;
- ⚠ **an inert base with an `Add` refinement is not short-circuited** — §4.3's rule, whose sabotage
  is dropping the `Add` clause;
- ⚠ **three colour-range components cost one Oklab round trip** — §4.2 is invisible to every other
  test, because a per-component round trip renders **bit-identically** and only costs more. The
  evaluator therefore carries a `#[cfg(test)]` conversion counter and the test asserts it reads 1
  per pixel, not 3. (A counter, not a timing: a benchmark cannot fail a build.) It stays inside
  `pipeline`, so it is a private test hook rather than the cross-crate surface
  `RenderEngine::requests_issued` had to become.
- reordering two components of different ops changes the result.

**Cache** (`render_engine`): a gating brush as component *j* deposits under `(i, j)` and hits on the
next render; two components of one mask do not evict each other; the base identity is unchanged by a
component index.

**Prefix** (`pipeline`): §6's third direction.

**App**: the chips append a component rather than a mask, and `New mask` still adds a mask; a
component row deletes and clamps the selection; `hit_test` targets the **selected** component
(sabotage: always the base); `active_mask_handle` clears on a component change.

**Sidecar**: a mask with no refinements round-trips **byte-identically**; one with refinements round
trips; a legacy sidecar loads with an empty list.

**Goldens: 60 → 63** — `mask_subtract`, `mask_intersect`, `mask_add` on the existing chart. ⚠ Each
measured for clipping and discriminating power before being accepted, and each compared against the
other two *and* against a single-mask render — a composite that silently ignored its refinements
would otherwise produce a reference that passes forever. **The 60 must stay byte-identical**, which
is the proof an empty `refine` takes today's exact path.

## 10. Measurement

⚠ **This design is not driven by a cost hypothesis, and the honest thing is to say so rather than to
assert numbers.** The prediction is that an analytic component is a few flops, a `Luminance`
component is free once §4.2's sharing is in, a `Colour` component costs one shared Oklab round trip
however many there are, and a brush component costs one more rasterisation and one more cache entry.

`bench_masked` is extended with composite rows — base+1 analytic, base+1 luminance, base+3 colour,
base+1 brush — at fit and at a native region, and **the measurement is an acceptance item, not a
follow-up**. This project's record in this area has five cost estimates overturned by decomposition;
the prediction above is written down so it can be checked against, not believed.

## 11. Accepted caveats

1. **The product algebra is not idempotent** — §3's trade, and the most likely thing to be reported.
2. **Components are append-only with no reorder**, while order is observable. Reordering is a drag
   affordance in a list, which `panel_order::{drop_index, moved}` already solves and can be reused;
   deferred rather than absent.
3. **One level.** A composite cannot contain a composite. The refinement list is deliberately not a
   tree, and nesting is the recursive variant's win that was traded away.
4. **The tint approximates pixel-dependent components**, §7.4, inheriting the auto-mask gate's
   existing degradation and its colour-space caveat.
5. **A brush component costs a cache entry and a rasterisation like any other brush**, so a
   composite of three brushes at 1:1 costs three brushes.
6. **No mask-to-mask reference** — a refinement is a shape, not "mask 2". Referring to another
   entry's shape would make deletion order-dependent and is a different feature.
7. **Three oversized files grew** and none was split, per §8.

## 12. Out of scope

Reordering components; nesting; a `Difference` op; per-component opacity or feather beyond what each
`Mask` already carries; referencing another mask entry; **M7.3b**, which is the next milestone.

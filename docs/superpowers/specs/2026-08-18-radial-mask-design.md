# M3.1b — the radial gradient mask

**Status:** spec, 2026-08-18.

## Scope

The second of M3.1's three mask primitives. 3.1a established the architecture on one analytic
mask (linear) across two plans — headless, then the canvas UI. This spec adds `Mask::Radial`.

⚠ **M3.1 still does not close here.** 3.1c adds the brush, and the milestone's own "Done when"
names dodge/burn with a brush.

**This is also the test of whether 3.1a's shape was right.** The stated criterion was: if adding
a variant is mostly *(a)* a `coverage` arm, *(b)* a `mask_tool::hit_test` arm, *(c)* an overlay
arm and *(d)* a panel button, the architecture held. Anything structural is the signal to stop
and reshape before 3.1c. The verdict is recorded in "Did the architecture hold" below — read it
before starting 3.1c.

## Decisions taken

| Question | Decision |
|---|---|
| Ellipse degrees of freedom | **Centre + two radii, axis-aligned.** No rotation. |
| Which side is covered | **Inside**, with `invert` flipping it. Diverges from Lightroom. |
| Feather | **A 0–100 slider, smoothstep ramp.** No inner guide ellipse on the canvas. |
| Handle type | **One flat `Handle` enum**; `hit_test`/`apply_drag` take `Mask`, not a shape. |
| Edge drag behaviour | **Symmetric resize about a fixed centre.** |

## The aspect correction does not come back — and that is the interesting part

`LinearGradient::coverage` takes an `aspect` because coverage is a *projection*, and a
projection needs a metric. Under the anisotropic scaling between normalised space and screen
space, a line through two points survives but lines **perpendicular** to it do not, so a
diagonal gradient's iso-lines skew. That trap has bitten in three places already (`coverage`,
`mask_tool::hit_test`, the canvas guide lines).

**An axis-aligned ellipse is immune.** Anisotropic scaling maps an axis-aligned ellipse to an
axis-aligned ellipse, so

```
((u − cx)/rx)² + ((v − cy)/ry)² ≤ 1
```

is already a correct on-screen ellipse with no correction whatsoever. **Rotation is precisely
the feature that would bring the trap back** — a rotated ellipse under anisotropic scaling is
still an ellipse, but with different axes and a different angle, so a stored angle would not be
the angle on screen. That is the main reason rotation is out of scope, ahead of the extra
handle and the extra field.

⚠ `RadialGradient::coverage` therefore **takes `aspect` and ignores it**, so that `Mask::coverage`
keeps one signature across variants. This must carry a comment saying *why* it is unused, or a
future reader will "fix" the omission and silently break every radial mask. It is guarded by a
test (below) rather than by hope.

⚠ `mask_tool::hit_test` **does** still need the aspect. Grabbing a handle is a screen-space
distance test against a tolerance, which is exactly the case the correction exists for. So the
two functions differ, deliberately, and each says so.

## The model

New in `edit-model`, beside `LinearGradient`:

```rust
pub struct RadialGradient {
    pub center: [f32; 2],
    /// radius[0] is a fraction of frame **width**, radius[1] of frame **height**.
    pub radius: [f32; 2],
    /// 0 = hard edge, 100 = ramp all the way from the centre.
    pub feather: f32,
    pub invert: bool,
}
```

`Copy`, `PartialEq`, serde with container-level `#[serde(default)]` — matching `LinearGradient`
so `Mask` stays `Copy` and `MaskedAdjustment` stays trivially cloneable.

Coverage, with `f = feather/100`:

```
d = √( ((u−cx)/rx)² + ((v−cy)/ry)² )     // exactly 1.0 on the ellipse
t = clamp((1 − d) / f, 0, 1)
coverage = t²(3 − 2t)                    // smoothstep: 1 at d = 1−f, 0 at d = 1
```

then `1 − coverage` when `invert`.

**Smoothstep rather than a linear ramp**, because a linear ramp in radial distance leaves a
visible crease ring where the derivative jumps — the vignette chose smoothstep for the same
reason, and reusing its convention means the app has one feather idiom rather than two.

### Two deliberate differences from `LinearGradient`

1. **A hand-written `Default`**, not a derived one: centre `[0.5, 0.5]`, radius `[0.25, 0.25]`,
   feather `50`, `invert: false`.

   ⚠ Container-level `#[serde(default)]` fills missing fields from the **struct's** `Default`,
   so a derived one would give every partial sidecar a zero-radius point-ellipse. And
   `feather: 50` cannot come from `f32::default()` under any arrangement — a derived `Default`
   would be wrong on that field alone.

   Radius `[0.25, 0.25]` renders as an ellipse *similar to the frame*, since both components are
   fractions of their own axis. That is a defensible default rather than an accident: on a 3:2
   frame you get a 3:2 ellipse, which is usually the shape you were reaching for.

2. **A zero-radius guard returning `0.0`**, where the linear's degenerate guard returns `1.0`.
   The linear's choice was between full coverage and NaN — "the alternative reaches the blend
   and paints the frame with garbage". Here the honest answer is available: an ellipse with no
   area covers nothing. `0.0` also cannot produce garbage. Reachable only from a hand-edited
   sidecar, since the `Default` and the panel both construct a usable ellipse.

⚠ `feather ≤ FEATHER_EPS` (1e-4) takes a hard-edge branch: `coverage = if d <= 1.0 { 1.0 } else
{ 0.0 }`. This is the only thing between feather 0 and a division by zero — the same role
`WIDTH_EPS` already plays in the vignette, and load-bearing for the same reason.

## Interaction

Five handles: the centre, and one per edge at `(cx ± rx, cy)` and `(cx, cy ± ry)`.

An edge drag **resizes symmetrically about a fixed centre** — dragging the right edge by `dx`
sets `rx += dx`, dragging the left edge by `dx` sets `rx -= dx`. This matches Lightroom and is
what people expect from a shape with a marked centre. Dragging the centre translates the whole
ellipse, the analogue of the linear's `Handle::Line`.

Clamps, for the reason the linear clamps its endpoints — a shape dragged out of reach cannot be
got back:

- centre to `[0, 1]` on both axes;
- radii to `[0.01, 2.0]`. The minimum keeps the ellipse grabbable; the maximum stops a fast drag
  producing an absurd value while still allowing an ellipse twice the frame, which is a
  legitimate look.

Handle hit priority: **centre before edges**, mirroring "endpoints before the line" — aiming at
the middle should move the whole shape rather than resize it.

### The handle type

`Session.active_mask_handle` is one field and must now hold either shape's handle. Three
options were weighed:

- **One flat enum** (chosen). `Handle { From, To, Line, Center, Edge(Side) }`, and
  `hit_test`/`apply_drag` change signature to take `&Mask`/`Mask` rather than
  `&LinearGradient`/`LinearGradient`. The canvas's drag plumbing then stays shape-agnostic and
  is written once.
- **Nested** `Handle { Linear(..), Radial(..) }`. Reads better per shape and anticipates the
  brush, but the mask/handle mismatch is **still representable**, so it buys no safety — only a
  level of nesting.
- **Per-shape free functions**, canvas matches on `Mask` first. Needs the nested enum anyway for
  the `Session` field, plus two dispatch sites: most code, least payoff.

⚠ The flat enum admits a mismatched pair (`Mask::Radial` with `Handle::Line`). `apply_drag`
handles it with a `_ => m` arm returning the mask unchanged. It cannot occur — there is no UI
to change a mask's kind, let alone mid-drag — and the alternatives do not make it
unrepresentable either, so the simplest spelling wins. Degrading to "the drag does nothing" is
the right failure for a UI.

**Cost, stated rather than discovered:** `mask_tool`'s six existing linear tests churn, wrapping
their gradient in `Mask::Linear(..)`. Accepted, and arguably an improvement — the drag path is
then tested through the type the canvas actually holds.

## Canvas

`draw_mask_overlay` **splits**: the shared drag/hit-test/history/`request` plumbing stays in one
function, and a `paint_handles(painter, mask, to_screen, clip)` helper matches on `Mask`. Writing
the drag plumbing twice is exactly the duplication that would let the two shapes drift.

The radial arm paints the outer ellipse as a **64-point closed polyline** — egui has no ellipse
primitive — plus five filled dots. No dashed inner guide: per the feather decision, the outer
ellipse marks where coverage reaches zero and the ramp is numeric only.

⚠ **Every coordinate stays against `frame_rect`, never `img_rect`.** This is not a new rule; it
is the fix from the second of 3.1a's two real-use bugs, where the overlay drifted with the view
while panning at high zoom because `img_rect` covers only the rendered slice once a region
render is on. The radial arm inherits it, and the `to_screen` closure passed to `paint_handles`
is what enforces it structurally.

⚠ The overlay must call `s.request(..)` after a drag moves the mask — the first of 3.1a's two
real-use bugs. This lives in the shared half, so the radial arm gets it for free. That is one
concrete payoff of splitting on *painting* rather than on the whole function.

## Panel

Two buttons in a `ui.horizontal`: `Add graduated filter` / `Add radial filter`. Both set
`selected_mask` to the new index and fold into the existing `mask_list_changed` local — ⚠ still a
local, not a direct write to `changed`/`settled`, because the closure already captures those
through `slider`.

The list label comes from a new `Mask::label() -> &'static str` ("Gradient" / "Radial"), so the
row reads `Radial 2`. Numbering stays list-positional rather than per-kind.

The per-mask adjustment sliders are unchanged: `LocalAdjustments` is shape-independent, which
is the point of `MaskedAdjustment` being a pair.

### Shape properties get their own block

Two controls appear between the mask list and the separator, above the Temp/Exposure/… block,
because they are properties of the **shape** rather than of the adjustments:

- **Feather**, 0–100 — radial only.
- **Invert**, a checkbox — **both shapes**.

⚠ **`invert` currently has no UI at all.** It exists on `LinearGradient`, is honoured by
`coverage`, is preserved across drags and is covered by `invert_survives_a_drag` — but the panel
never exposes it, so today it is reachable only by hand-editing a sidecar. That was a survivable
gap in 3.1a, where a linear gradient can be inverted by dragging its endpoints past each other.
It is **not** survivable here: with "inside is covered", the classic subject vignette — darken
everything except the ellipse — is unreachable without it, and swapping the endpoints has no
radial analogue.

So this change closes the gap for both shapes. It is in scope rather than deferred because the
radial's headline use depends on it, and because a checkbox beside a slider costs less than a
caveat explaining why the feature is half-wired.

## What does not change

Recorded because it is the evidence, not because it is filler:

- **`MaskStage`** — calls `self.masked.mask.coverage(u, v, aspect)`. Zero lines.
- **`pipeline::build_pipeline`** — iterates `edits.masks` and builds a `MaskStage` per entry.
  Zero lines.
- **`EditGroups` / `Edits::merge_from`** — masks are already the seventh group, defaulting to
  OFF. `merge_from`'s exhaustive destructure sees a `Vec<MaskedAdjustment>` whose element type
  gained a variant, not a new field. Zero lines.
- **Sidecar format** — `Mask` is externally tagged, so existing files hold `{"Linear": {...}}`
  and load untouched. Zero lines.
- **`edit_clipboard`, `sync`, presets, batch export** — all move `Edits` wholesale.
- **`tail_kernel_radius` / `REGION_MARGIN`** — masks carry per-pixel adjustments only, so they
  contribute no kernel reach. Unchanged, and the region test's inset is unaffected.

## Testing

Every new test gets the sabotage step: break the thing it names, watch it fail, restore. This is
now a standing rule with three recorded instances of a test that named a bug it could not
detect, one of them in this very feature's predecessor.

### `edit-model`

| Test | Sabotage |
|---|---|
| `radial_coverage_is_one_at_the_centre_and_zero_outside` | invert the comparison |
| **`radial_coverage_is_independent_of_aspect`** | **multiply `u` by `aspect` and watch it fail** |
| `feather_zero_is_a_hard_edge` | remove the `FEATHER_EPS` branch (expect NaN or a panic) |
| `feather_one_hundred_ramps_from_the_centre` | clamp `f` to 0.5 |
| `radial_invert_flips_coverage` | drop the `invert` arm |
| `a_zero_radius_radial_is_inert_not_nan` | remove the guard |
| `a_sidecar_without_a_radial_still_loads` | — (pins the serde tag) |
| `a_partial_radial_gets_a_usable_default` | swap the hand-written `Default` for a derived one |

⚠ **`radial_coverage_is_independent_of_aspect` is the load-bearing one.** It is the only thing
standing between this design and a future reader adding the "missing" aspect correction that
three sibling functions all need. Assert the *same* coverage at the same `(u,v)` for aspects
1.0, 3.0 and 0.33, using an off-centre point on neither axis — a point on an axis would be
insensitive to a correction on the other one, which is the precise shape of the mistake
`hit_testing_the_line_is_aspect_correct` made in 3.1a.

### `mask_tool`

| Test | Sabotage |
|---|---|
| `the_centre_is_grabbed_in_preference_to_an_edge` | reorder the checks |
| **`hit_testing_an_edge_handle_is_aspect_correct`** | delete the aspect scaling |
| `dragging_an_edge_resizes_about_a_fixed_centre` | move the centre too |
| `dragging_the_centre_leaves_the_radii_alone` | scale the radii |
| `radii_clamp_to_a_usable_range` | remove the clamp |
| `the_centre_clamps_to_the_frame` | remove the clamp |
| `radial_invert_survives_a_drag` | drop `invert` from the rebuild |
| `a_mismatched_handle_leaves_the_mask_unchanged` | make `_ => m` panic |

⚠ **`hit_testing_an_edge_handle_is_aspect_correct` must be designed against the 3.1a lesson.**
The probe has to be offset along an axis where scaling genuinely changes the answer. Concretely:
probe the **top** edge handle with a **horizontal** offset on a 3:1 frame, where the correction
turns a 0.03 normalised offset into 0.09 frame heights and moves it outside a 0.04 tolerance. A
vertical offset from the same handle would be untouched by the scaling and would pass with the
correction deleted.

### Goldens

33 → 35: `mask_radial` and `mask_radial_inverted`, both on the existing chart with a masked
exposure lift large enough to be unambiguous.

⚠ **The existing 33 must come out byte-identical**, and that is the actual acceptance criterion
for the whole change — it is the proof the variant is additive and that nothing in the shared
path shifted. The two new files are the smaller half of the evidence.

## Did the architecture hold

To be filled in on completion, but the prediction from the design work:

- **(a) `Mask::coverage` arm** — yes, one arm, as predicted.
- **(b) `mask_tool::hit_test` arm** — yes, but with a **signature change** (`&LinearGradient` →
  `&Mask`) and a widened `Handle` enum. This is the one structural wrinkle, and it is a
  consequence of `Session` holding a single `active_mask_handle`, not of the mask model.
- **(c) canvas overlay arm** — yes, plus a split of `draw_mask_overlay` into shared plumbing and
  a matching painter. A refactor, not a reshape.
- **(d) panel button** — yes.
- **(e) unpredicted:** `Mask::label()` for the list row, a shape-specific Feather slider, a
  hand-written `Default` where the linear derived one, and an **`invert` checkbox that 3.1a
  never wired up** — a latent gap the radial turns into a blocker.
- **Zero change** in `pipeline`, `EditGroups`, `merge_from`, the sidecar format, and every
  feature that moves `Edits` wholesale.

That reads as **held**. The signature change is the only item that touches an existing seam, and
it touches interaction code rather than the model or the pipeline. Nothing here argues for
reshaping before 3.1c — though the brush will test the same `Handle` decision much harder,
since a stroke's "handles" are not points at all, and that is where the nested-enum option
becomes worth revisiting.

## Accepted caveats

1. **No rotation.** A tilted ellipse is unavailable; compensate by resizing. This is what keeps
   `coverage` free of the aspect correction, so it is a trade rather than an omission.
2. **No inner feather guide on the canvas.** The outer ellipse marks coverage zero; where the
   ramp *starts* is numeric only. A dashed inner ellipse is a contained follow-up.
3. **One clone and full stage runs per mask**, even where coverage is zero — inherited from
   3.1a, and radial masks are exactly the case where a bounding-box optimisation would start to
   pay. Still deferred; the measurement belongs with 3.1c, where masks are smallest.
4. **Per-pixel adjustments only.** Unchanged from 3.1a and unchanged by shape.
5. **No mask intersection or subtraction**, so a radial cannot yet be cut out of a linear.
6. **M3.1 does not close here.**

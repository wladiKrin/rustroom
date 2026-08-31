# Guided perspective — design

**Date:** 2026-08-31
**Follows:** `2026-08-31-perspective-correction-design.md`, whose manual sliders shipped the same
day. This is the slice that was deferred there.

## What this is

Draw up to four guide lines onto features that ought to be vertical or horizontal; Rustroom solves
for the camera rotation and **writes the answer into the existing Vertical, Horizontal and
Straighten sliders**.

## ⚠⚠ `mod_pc` is NOT used, and dropping it is the central finding

The backlog entry — written by me the same day — said guided *"is the half `mod_pc` actually
serves"*. Having worked the maths through, that is wrong, and using lensfun here would be strictly
worse:

- lensfun returns a **3×3 matrix plus a shift** (`PerspectiveState`), derived from its own three
  rotation angles and its own estimated focal length. To honour the chosen model — *solve into the
  sliders* — that matrix would have to be **decomposed** back into our pitch/yaw at our fixed
  `F_NORM`, which is not exactly representable. The result would be an approximation, and the image
  would visibly shift the moment the solve finished.
- Solving in **our own model** is exact by construction, and it is closed-form. Our rotation's
  bottom row is `[−cθ·sφ, sθ, cθ·cφ]`, and "this family of lines becomes parallel" is exactly
  `(R · ray)_z = 0`. Two families give two equations in two unknowns.

Given the vertical vanishing point `(a, b)` and the horizontal one `(c, d)`, both in our normalised
frame coordinates with `f = F_NORM`:

```
tan φ = −f·(b − d) / (a·d − b·c)          (yaw / Horizontal)
tan θ = sin φ · (a − c) / (b − d)         (pitch / Vertical)
```

⚠ The degenerate cases fall out of the same equations rather than being special-cased:
**verticals only** → `φ = 0`, `tan θ = −f / b`, which is *literally the formula already in
`perspective.rs`* and already has a test; **horizontals only** → `θ = 0`, `tan φ = f / c`.

⚠ **Roll comes free and is written to Straighten.** After `R`, the world-vertical direction
`R·(a, b, f)` has `z = 0` by construction, so it is a direction *in the image plane*; the angle
between it and screen-vertical is the residual roll. Guided therefore levels the horizon too,
which is what Lightroom's Upright does.

⚠ **The cost of the chosen model, stated plainly:** two vanishing points also determine the true
focal length (`f² = −(a·c + b·d)`, which is what lensfun's `mod_pc.rs:344-347` computes). We
**cannot use it**, because the stage's `F_NORM` is fixed. So the verticals still come out exactly
parallel — that holds for any `f` — while the residual stretch is whatever our assumed focal
implies, and **Aspect remains the control that corrects it**. Auto-setting Aspect from the
estimate is a possible follow-up, deliberately not in this slice.

## Guides are view state, never edits

`Session.guides: Vec<Guide>`, **not** a field on `Edits`.

⚠⚠ This is what makes "solve into the sliders" coherent rather than a second representation. A
guide is **scaffolding for arriving at a slider value**, not part of the photograph's description —
the same argument that keeps `selected_mask` off `Edits`. Consequences, all of them wanted: guides
never reach the sidecar, never transfer on a paste, preset or sync, and reset when you switch
photo. The `Edits` written by a guided solve are indistinguishable from the same values dialled in
by hand, which is the entire point of the chosen model.

```rust
/// One drawn guide, in frame-normalised coordinates on the RAW frame.
pub struct Guide {
    pub from: [f32; 2],
    pub to: [f32; 2],
}
```

## ⚠ The family is inferred from the line, not chosen from a menu

A guide is **vertical** if `|dy| >= |dx|`, else **horizontal**. You draw along a building edge and
it is obviously a vertical; asking which kind it is first would be a click before every gesture.

⚠ The inference is **shown**, not hidden: vertical guides paint in one colour and horizontal ones
in another, so a misclassified 45° line is visible and fixed by nudging an endpoint. A rule the
user cannot see is a rule they cannot correct.

⚠ **At most two per family.** A third in the same family is refused with a status line saying so,
rather than silently replacing one — the same reasoning that made the keyword-deletion dialog fire
only when it removes more than the keyword itself: a silent replacement is the surprising one.

## Solve on release, drawn on the uncorrected frame

While the Guides sub-mode is active, `preview_edits` additionally resets `perspective` **and**
`geometry.angle`, so the canvas shows the raw oriented frame.

⚠⚠ **This is what removes the feedback loop.** Guides sit on the image, and the solve transforms
that image — so a live solve would slide the feature out from under the line being dragged. Crop
mode already solves the identical problem the identical way, previewing with the crop reset so the
rectangle has something stable to sit on.

⚠ Because the preview resets them, guide coordinates always mean the same thing: a position on the
**raw** frame. Orientation (quarter turns and mirroring) is *not* reset — it is a lossless reframe,
and resetting it would show the photo sideways.

The solve runs on `drag_stopped`, writes `perspective.vertical`, `perspective.horizontal` and
`geometry.angle`, and pushes **one** history entry — so one gesture is one Ctrl+Z, the rule every
drag in this app follows.

## UI

In the Geometry tool's Perspective section: a **Guides** toggle and a **Clear guides** button, the
latter disabled when there are none.

- Dragging on empty canvas draws a new guide; dragging an endpoint moves it.
- Endpoint handles get the crop overlay's treatment — a dark halo under a coloured mark — for the
  reason that overlay needed it: an unhaloed mark is invisible on a bright photograph.
- Guides are drawn only while the sub-mode is on, so they never obscure ordinary editing.

⚠ The Guides toggle **does not disable the sliders**. They are the output; watching them move as
the solve lands is what explains where the numbers came from.

## Testing

- **The solve is a pure function** over vanishing points, so every case below needs no window and
  no image: verticals-only, horizontals-only, both, and each degenerate denominator.
- ⚠ **Round-trip against the forward model, which is the test that matters**: pick a known pitch
  and yaw, generate the vanishing points those angles *produce*, feed them to the solver, and
  assert it recovers the angles. That validates the solver against `Homography` rather than against
  a re-derivation of the same algebra — the mistake `FrameMap` explicitly avoided by testing
  against `GeometryStage` itself.
- ⚠ **Leverage in every geometric probe.** M3.1c-A recorded a sign error that survived because its
  probe sat at 0.34 of the half-width.
- **Family inference** at `|dx| == |dy|` exactly, which is the only ambiguous input.
- **Two guides of a family are required**; one is not enough and must leave the edits untouched
  rather than solving from nothing.
- **Parallel guides have no intersection** — the vanishing point is at infinity, meaning that
  family is already correct. Must yield zero for that axis, not a divide-by-zero.
- **Headless panel/canvas tests** for the toggle, the refusal of a third guide, and that a guide
  drag pushes exactly one history entry.
- ⚠ **Sabotages to run and record**: swap the vertical and horizontal vanishing points; drop the
  roll term; use `>` instead of `>=` in the family inference; solve on press instead of release;
  remove the perspective reset from `preview_edits`.

## Accepted caveats

1. **The estimated focal length is computed but unusable** — `F_NORM` is fixed. Verticals still
   come out exactly parallel; the residual stretch is Aspect's job.
2. **Guides are not saved.** Switching photo or quitting discards them; the slider values they
   produced persist, which is the part that matters.
3. **Exactly two guides per family**, so no least-squares fit over more lines.
4. **The solve overwrites** whatever the three sliders held. Undoable in one step.
5. **No automatic line detection** — that is the third slice, and it needs a Hough transform.
6. **Guides are drawn on the raw frame**, so the corrected result is only visible once the
   sub-mode is off.

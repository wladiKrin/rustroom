# Mask isolation, and the render-path defects the placement instruments found

**Date:** 2026-09-09
**Branch:** `m7.6-auto-mask`
**Status:** design, approved

## 1. What this is

Four changes in one seam — the render-request path shared by the canvas, the panel and the
session.

1. **Mask isolation.** While a mask's tint is shown, that mask's own adjustments are dropped from
   the view. Painting then needs no render at all.
2. **The interactive rung must not drop for a gesture that needs no render.** A pan inside the
   rendered margin, and a paint stroke on a suppressed mask, leave the picture alone; today both
   force a re-render one rung lower and a 2x blur across the whole view.
3. **`fit_image_size` derived from the buffer rather than from a rounded frame dimension**, so
   placement is rung-independent by construction.
4. **The full-resolution handover requests at the current rung and region** instead of always
   whole-image at fit.

(1) and (2) together are what deliver *"keep the full resolution even while drawing, with the mask
on"*. (3) and (4) are the two real defects the instrumentation found while looking for the
reported placement shift.

## 2. The placement shift: what was measured, and what it was not

The task began as *"fix the placement shift at a rung transition"*, reported as: while drawing at
zoom, **the image noticeably shifts, slightly to the top left**, when the render quality changes.

⚠⚠ **The photograph does not move. Two independent instruments say so, one of them on the
reporter's own session.** The hypothesis this work started from — that `fit_image_size` is
recomputed per rung and drifts — is **real but far too small**, and it is recorded here so nobody
re-derives it as the cause.

### 2.1 The placement instrument

`RUSTROOM_DEBUG_PLACE=1` (kept, in `develop/canvas/mod.rs`, the `RUSTROOM_DEBUG_BRUSH` precedent)
logs every input the placement is derived from, per frame. Over **836 real frames with 16 rung
transitions**, holding pan and zoom fixed across each transition:

| | measured |
|---|---|
| worst content shift at screen centre | **0.25 px** |
| vertical shift, every transition | **exactly 0.000 px** |

⚠ The vertical zero is **structural, not luck**: `fit = avail.y / full.y` in the constrained axis,
so `full.y * fit` is `avail.y` identically and the frame's on-screen height cannot depend on the
rung. **This mechanism can only ever move things sideways, sub-pixel.** A report of a *top-left*
translation is therefore not this.

### 2.2 The content instrument

`rung_alignment` (kept, `crates/app/examples/`) renders the same photo through the **real**
prefix+tail at each rung — **lens corrections on, as the GUI has them**, since they are the only
geometric stage whose displacement field is rebuilt per resolution — resamples the higher rung onto
the lower one's grid with `downscale_to`, and measures the translation by SSD minimisation with a
parabolic sub-pixel refinement.

| DSCF6223, 6246x4170 | 3200 vs 1600 | 6246 vs 1600 |
|---|---|---|
| lens corrections **on** | −0.008, −0.002 fit px | **+0.020, +0.003** fit px |
| lens corrections off (control) | 0.000, 0.000 | 0.000, −0.000 |

⚠ **The control reading exactly zero is what makes the lens row believable** — the harness is sound
before any row that could be blamed on a stage is trusted. At the reporter's 8.88x magnification
the worst figure is **0.18 screen px**.

### 2.3 What follows

Placement ≤0.25 px and content ≤0.18 px: **under half a screen pixel together, a quarter of a
device pixel on a 2x display.** There is no displacement to fix.

What is large and real, from the same log: `drag_quality` is `Faster`, so a stroke takes the
texture from **2.28 screen pixels per rendered pixel to 4.44** — a genuine 2x softening of the whole
visible field, exactly as designed. ⚠ **A high-contrast edge going soft and sharp again reads as
movement.** That is a property of the blur changing, not of anything being displaced, and nothing
can be moved to fix it — only the resolution change can be removed, which is what items (1) and (2)
do for the drawing case.

⚠ This is a hypothesis about perception and **only use can confirm it**. If a shift is still
visible while drawing once the rung stops changing mid-stroke, it is not the render and the next
places to instrument are the mask tint's own placement grid and the region-scoped coverage
rasterisation — neither of which has been measured.

## 3. Mask isolation

### 3.1 The rule

While the selected mask's tint is shown, that mask's adjustments are dropped from the **view**.
Everything else — every other mask, every global edit — renders normally.

Chosen over "every mask's adjustments" (which re-renders whenever you select a different mask, and
hides work on the masks you are composing against) and over "all edits, the plain before view"
(which loses the tonal context the mask was painted against).

### 3.2 The seam

`Session.suppressed_mask: Option<usize>`, the exact mirror of `Session.suppressed_spot`, which
already does this for spot removal — *"a patch leaves the view while it is adjusted, so the blemish
underneath is visible to line up against"*. `Session::view_edits` removes that entry from
`e.masks`, beside the existing `e.spots.remove(i)`.

⚠ **Removed rather than neutralised**, `suppressed_spot`'s reasoning: a mask contributes to the
picture only through its adjustments, so the two are the same thing, and removal cannot be
half-done.

⚠⚠ **One derivation site, never a set/clear pair.** It is derived unconditionally each frame in
`develop::body` from `tool == Tool::Masks && app.show_mask_tint && s.selected_mask`. The
spot-removal flicker bug was *two owners* of one suppression — the panel released what the canvas
had suppressed — and the rule that came out of it was *"an owner may release only what it
suppressed"*. An unconditional derivation from the same three facts that decide whether the tint is
drawn cannot acquire a second owner, and cannot disagree with what is on screen.

⚠ View-only. `view_edits` feeds the render alone; the sidecar, presets, paste, sync and export are
untouched, exactly as `suppressed_spot` is.

### 3.3 Painting requests no render

A stroke's release currently calls `s.request(..)`. With the mask suppressed the view edits are
unchanged, so the request would re-render an identical picture. The release site gains one guard:
request only when the edited mask is not the suppressed one.

⚠ **This is necessary but not sufficient** — see §4. `is_decidedly_dragging()` is true while
painting, so without the rung rule the canvas would still drop the rung and re-render on the
gesture's first frame, and the softness the feature exists to remove would remain.

### 3.4 What it costs, and what it buys beyond the render

**The cost, stated rather than discovered:** a slider belonging to the shown mask changes nothing on
screen. The rule stays one sentence — mask shown implies that mask's edits hidden — and the way to
see the effect is to untick "Show mask", which is what *"show them only when the mask is off"*
already means. Two alternatives were considered and declined: un-suppressing for the duration of the
slider's drag (the spot tool's pattern inverted — it gives a second owner of the suppression, the
exact shape of the flicker bug), and having a masked slider untick the box itself (a control that
changes itself, and the tint is lost mid-thought). ⚠ **This is the caveat most likely to be reported
as a bug.**

**A benefit that is not about performance:** the auto-mask tint gates against the displayed frame,
which today **already has this mask's adjustment applied** — so the tint chases its own effect,
which is precisely the failure `CoverageSource::Range` keys on `base` to avoid. Suppressed, the
reference becomes the global tail plus the *other* masks. That is identical to what `MaskStage`
hands the render-time gate whenever the selected mask is last in the list, and close otherwise.

## 4. The rung must not drop for a gesture that needs no render

### 4.1 The defect

The canvas requests on `(!covered && s.requested_region != want) || max_dim != s.render_rung`.
⚠ **The second disjunct fires even when `covered` is true.** So the first frame of a pan discards a
rung-6246 texture that already covers the viewport and replaces it with a rung-3200 one, for
nothing. The same applies to a paint stroke once the mask is suppressed.

### 4.2 The rule

**The interactive drop engages for a drag that changes the picture, not one that only changes the
view.** A pan changes the region; a paint stroke on a suppressed mask changes neither. Neither drops
the rung.

⚠⚠ **This remains a question about the gesture, never about the render, which is what keeps it from
oscillating.** The 2026-08-13 progressive-sharpen feature asked *"is the showing resolution not yet
sharp?"* — a question the render itself answers, so the render answered it, the gate flipped, and it
counted that as interaction. Nothing here can be affected by what a render produces.

⚠ The drop must still engage for a **slider** drag, which is what it was built for and what was
confirmed by use (137.9 to 40.0 ms, 7.2 to 25 fps). Every non-canvas request site uses
`s.render_max_dim()`, i.e. whatever rung the canvas last set — so a formulation that simply stopped
the canvas requesting would disable the feature outright. The condition is on the *kind of gesture*,
not on whether the canvas happens to want a frame.

### 4.3 Panning is decided by measurement, not here

⚠⚠ **The interactive rung's 137.9 ms was measured on a fitted, whole-image render of 6.8 Mpx.**
Zoomed, a render is region-bounded to the viewport: at the reporter's zoom that is **951x539 = 0.51
Mpx**, and `bench_masked` prices a neutral native region tail at **6.6 ms for 1.26 Mpx**. If the
drop is worth single-digit milliseconds there, it is not worth a 2x blur across the whole view while
panning.

**Done when:** a region tail is timed at rung 6246 against rung 3200, at the region sizes the canvas
derives for a real zoomed viewport, on a real edit stack — and the pan behaviour is chosen from that
number. ⚠ `bench_masked`'s own note applies: *"a synthetic bench prices a stage, not a session"*, so
read the two rungs as a ratio, not as a prediction of a frame time. If the drop is worth 40 ms,
panning keeps it and `Settings.drag_quality` remains the escape hatch.

## 5. `fit_image_size` derived from the buffer

`shell.rs` computes `fit_image_size = frame_dims * fit_cap / frame.max_dim`. The frame's dimensions
come from `downscale_to`, which **rounds**, so the quantisation error is amplified by
`rung / fit_cap`: measured, `full.y` reads **1068.000 at rungs 1600 and 3200 against 1068.204 at
6246** on an uncropped X-T4 frame.

Derived instead from the buffer's own dimensions, the fit cap and the crop, in floats, it depends on
no frame at all and is therefore **rung-independent by construction** — the class of bug removed
rather than the instance, which is the same move `render_max_dim` and `Session::geometry_mode` each
got.

⚠ **Worth 0.4 px. This is principled, not visible**, and it is in scope because the value's
dependence on which rung last landed is exactly the two-facts-that-disagree shape this file records
three times, not because anyone can see it.

⚠ The rendered texture keeps being drawn at its own aspect while `frame_rect` uses this one's, a
mismatch of about 0.02%. That is already true today and stays sub-pixel.

## 6. The handover requests at the current rung

`session_flow.rs`'s full-resolution handover calls `s.request(s.view_edits(tool), s.fit_cap(),
None)`. The comment above it is right that an explicit request is needed — without it the canvas
sees an unchanged rung and a covered region and asks for nothing, so the upgrade is invisible. Only
the **resolution** is wrong: at the reporter's 7.72x that landed an **8.9x magnified 1600 px
texture, held for 18 frames** — four times the softening `Faster` is supposed to give, once per
photo.

It requests at `s.render_rung` with the region the canvas last wanted. ⚠ `rendered_region` and
`requested_region` are still cleared first, for the reason already recorded: region rects are in
*rendered* pixels and those coordinates change with the buffer.

## 7. Testing

⚠⚠ **All 60 goldens must stay byte-identical.** Nothing here touches a pixel path, and that is the
assertion, not "still green".

- **Mask isolation, the carrier.** The panel's checkbox reaching `view_edits`, and `view_edits`
  dropping *that* mask and nothing else. ⚠ W5's lesson applied: the pipeline half of a feature can
  be fully proven while nothing carries the data to it. Sabotage: `view_edits` ignoring
  `suppressed_mask`.
- **A stroke on a suppressed mask issues no render.** Asserted on `RenderEngine::requests_issued`
  **on the release frame itself** — the canvas-harness rule already recorded: sampling later cannot
  see a missing request, because a subsequent frame issues one for reasons of its own. Sabotage:
  remove the guard.
- **A pan inside the margin requests nothing and leaves `render_rung` alone.** Sabotage: restore the
  unconditional `max_dim != s.render_rung` disjunct.
- **A slider drag still drops the rung.** The other half, and the one that stops the fix disabling a
  feature confirmed by use. Both halves are needed: a test that only checks the pan would pass
  against a version that never drops the rung at all.
- **`fit_image_size` is the same at two rungs.** Exactly what today's code fails, so it is written
  against the current behaviour and watched failing first.
- **The handover requests at the current rung**, not `fit_cap`. Reachable because `pump_full_res` is
  a method on `RustroomApp` that `test_app` can drive with an injected decode.

⚠ Every test is sabotage-verified, and a sabotage is believed only after a `test result:` line
appears — a sabotage that fails to compile looks exactly like one that passes. Filtered runs are
read for their **passed count**, not for `ok`.

## 8. Accepted caveats

1. **A slider belonging to the shown mask does nothing visible.** §3.4; the deliberate cost of a
   one-sentence rule.
2. **The reported shift may persist**, because the measurements say it is not a displacement. If it
   does, the next instruments are the tint's placement grid and the region-scoped coverage
   rasterisation.
3. **Panning's rung behaviour is not settled by this document** — it is settled by the measurement
   in §4.3, and the answer may be "leave it as it is".
4. **`fit_image_size` becomes an ideal rather than a measurement of the frame**, so it can differ
   from the real frame by up to half a rendered pixel. That is the point: consistent beats exact for
   a placement reference.
5. **Nothing here makes a render faster.** It removes renders that were never needed and stops one
   that was too coarse.
6. The suppression is per *selected* mask, so composing two masks against each other still means
   unticking the box.

# Geometry-mode navigation, a shared loupe, and the straighten/guides rule

*Design, 2026-09-03. Three items, all found by using W4's merged B track. Decisions settled with the
user; none is the implementer's to re-open.*

W4 landed the straighten line. Using it produced three things: the crop tool holds the view still,
which is limiting; placing a line precisely wants the loupe the guide overlay already has; and the
straighten button greys out whenever perspective guides exist, which reads as a bug.

---

## 1. A straighten line clears the guides, and the grey-out goes

**The grey-out was deliberate and it was the wrong call.** `panel/geometry.rs:185` currently reads
`let can_straighten = !s.show_guides && s.guides.is_empty();`. The reasoning behind it was sound: a
straighten line writes `geometry.angle`, one of the three values a guide solve owns, so guides left
on screen afterwards describe a solve that no longer holds — and `panel/geometry.rs:203`'s hand-edit
check structurally **cannot** catch it, because it captures `solved_before` earlier in the *same*
frame and an angle written by the canvas on the *previous* frame is already in `s.edits` by then.

But the codebase already has the rule this needs, and it is one line above:

> ⚠⚠ **Editing by hand throws the guides away, because the solve no longer describes what you see.**

**A straighten line is a hand edit through another affordance.** So it clears the guides on release,
exactly as dragging the Straighten slider does, and the gate disappears entirely.

⚠ The only reason a gate was chosen over clearing is now gone: track B was **barred from writing
`Session.guides`** for the duration of W4, so gating was the one option available to it. That
ownership boundary ended when the wave merged.

**Behaviour:** drawing a straighten line with guides present clears `Session.guides` and
`Session.active_guide`, leaves `perspective.vertical` / `perspective.horizontal` **untouched**, and
sets the new angle. That matches `Clear`'s recorded contract — *"Remove all guides. The sliders they
produced are left alone."*

⚠ Accepted cost, stated: the guides are gone with no undo of their own. The angle change is one
Ctrl+Z as before; the guides are scaffolding, not part of the photograph's description, which is why
they were never in `Edits` in the first place.

---

## 2. One loupe, shared by both overlays

`guide_overlay.rs`'s `draw_loupe` is a 168 pt window at 5× magnification, placed in the corner
**furthest from the cursor** so it never covers what is being traced, with crosshairs rather than a
dot "because a dot hides the very pixel being placed". It is `fn draw_loupe` — **private to that
module**.

It moves to a new `develop/canvas/loupe.rs` as `pub(super) fn draw_loupe`, and both overlays call it.

⚠⚠ **Extracted, never copied.** `test_support.rs` already states the rule for this exact situation:
*"Lives in its own module because the crop and mask overlays both drive it, and a helper duplicated
per overlay is how the two would drift."* Two loupes would drift in magnification, placement and
crosshair size, and nothing would fail.

⚠ **Its `img_rect` sampling is correct and must survive the move unchanged.** Its own comment is the
reason: *"the sample window is taken in `img_rect` coordinates, not `frame_rect`: `img_rect` is where
the texture is actually painted, so this stays exact whether the frame was rendered whole or as a
region. That is the one place in this overlay where `img_rect` is the right rect."* That reasoning is
about where the **texture** is, so it is unchanged by anything in §3.

**Where it shows in the straighten overlay:** at the **live end of the line** while a drag is in
progress — the point being placed — and not at all once the line has settled. The guide overlay shows
it only for the endpoint being dragged, for the same reason: a loupe pinned up permanently is chrome.

---

## 3. Pan and zoom under the Geometry tool, crop rectangle included

Today `canvas/mod.rs` does two things that hold the view still:

```rust
let rung = if tool.is_geometry() && !s.show_guides { fit_cap } else { … };   // line 111
if !tool.is_geometry() || s.show_guides { …zoom and pan… }                    // line 332
```

The recorded reason is precise and is about the **overlay**, not about the crop: *"the crop overlay
is drawn against the whole image and placed by the fit-mode maths… so zooming or panning underneath
it would misplace it."* Guide Mode is already the exception — it unpins, *"because placing an
endpoint exactly on a building edge needs real pixels; at fit resolution you are aiming at a blur"* —
and it can afford to because its overlay is anchored to `frame_rect` and survives a region render.

**So the work is to give the crop overlay that same property, then delete both gates.**

### The four coordinates that must change, and the two that must not

⚠⚠ **This is M3.1a's three-headed bug, and only one head was visible then.** The mask overlay drifted
while panning; fixing the placement alone left two silent faults — the aspect taken from a *region*
rather than the frame, and drag deltas divided by the region's dimensions, so *"a handle dragged
while zoomed moved by the wrong amount."* Both would "have surfaced later as 'handles feel wrong when
zoomed' with no obvious cause." All three heads exist here.

| `crop_overlay.rs` | now | becomes | why |
| --- | --- | --- | --- |
| `crop_screen_rect` (82–87) | `img_rect` | **`frame_rect`** | placement — head 1 |
| hit-test normalisation (162–163) | `img_rect` | **`frame_rect`** | which handle you grabbed — head 2 |
| drag deltas (170–171) | `img_rect` | **`frame_rect`** | how far a handle moves — head 3 |
| the dimming, `rects_around` (111) | `img_rect` | **`img_rect`** — unchanged | it darkens the *visible photograph* outside the crop; there is nothing beyond the painted texture to darken |
| the clip, `painter_at` / `overlay_clip` (109, 118) | `img_rect` | **`img_rect`** — unchanged | the guide overlay's rule: `img_rect` is still the right clip, widened because a stroke centres on its path |

⚠ **`img_aspect` (103) needs nothing.** It already comes from `s.baseline.size_vec2()`, not from a
rect, so it is frame-independent by construction. Two aspect sources are what the crop panel's own
comment warns about; this keeps one.

⚠ **`draw_guides`'s aspect (56–59) needs nothing either**, but its comment does: it takes the aspect
from the crop's screen rect, which stays the crop's true pixel aspect once that rect is derived from
`frame_rect`. The comment currently justifies it by naming `img_rect` and must be updated or it will
read as wrong.

### The call sites

`canvas/mod.rs:238` (`draw_crop_overlay`) and `canvas/mod.rs:318` (`crop_screen_rect`, for the
composition guides) both pass `img_rect` today. `draw_crop_overlay` gains a `frame_rect` parameter;
both call sites pass both rects.

### One dependency, already closed

⚠ Entering geometry mode **with a crop** once left `frame_rect` the wrong size: `preview_edits`
resets the crop so the frame grows, while `fit_image_size` refreshed only from *whole-image* frames,
so every frame-relative overlay sat slightly off "righting itself only when you zoomed out far enough
for a whole-image frame to land." **Guided perspective fixed that** — region frames carry
`full_w`/`full_h` and refresh it too. Without that fix this feature would be unbuildable; with it,
nothing further is needed. Do not re-introduce a whole-image-only condition there.

### What this costs

⚠ **A native-rung render in geometry mode is a real cost that the pin was also avoiding**: the
comment notes a whole-image native render "would be both expensive". Region rendering is what makes
it affordable — the tail runs on the viewport, not the frame — and that is exactly the machinery
guided perspective already relies on. Expect the first frame after zooming in to be slower, as
everywhere else.

⚠ **Straighten mode inherits this for free** and needs no separate work: its overlay is already
`frame_rect`-anchored, so once the gates go it pans and zooms like Guide Mode.

---

## 4. Testing

The headless canvas harness (`test_support.rs`) drives the real `show`, and every risk here is a
coordinate error that is **invisible at fit and wrong at zoom** — so a test that does not zoom cannot
see any of it.

- **⚠⚠ The zoomed-drag test is the one that matters.** Set `s.zoom` above fit so a region render is
  requested, drag a crop handle, and assert the crop moved by the amount the pointer did. At fit,
  `img_rect` and `frame_rect` are bit-identical — D's `frame_rect` sabotage in W4 was **inert** for
  exactly this reason — so **the sabotage for each of the three heads must be run at zoom**, or it
  proves nothing.
- **The existing four crop tests must keep their assertions verbatim.** They are the evidence the
  fit-resolution path did not move. ⚠ Including `the_crop_overlay_does_not_ask_for_a_render`, which
  pins a deliberate asymmetry: the crop overlay issues no render because `preview_edits` strips the
  crop and the overlay *is* the feedback.
- **A pan must not move the crop.** Panning changes `frame_rect`'s origin; if any of the three heads
  is still `img_rect`-based, the crop will appear to slide. That is the original mask-overlay
  symptom and deserves its own test.
- **Clearing:** drawing a straighten line with two guides present leaves `guides` empty and
  `perspective.vertical` unchanged. ⚠ Assert **both**, or a version that cleared the whole solve
  would pass.
- **The loupe:** one test that it is drawn while a straighten drag is live and not after release.
  ⚠ A shared helper called by two modules needs no *duplicate* test — the guide overlay's existing
  coverage stands — but the straighten overlay's call does.

---

## 5. Accepted caveats

1. **Guides drawn before a straighten line are lost**, with no undo of their own (§1).
2. **Geometry mode now pays for native renders** when zoomed, like every other tool (§3).
3. **The loupe magnifies the rendered texture**, so at fit resolution it magnifies preview pixels —
   the caveat guided perspective already records. It is genuinely sharp once zoom has triggered a
   native render, which §3 now makes possible in this tool.
4. **No loupe on the crop handles.** The straighten line is a placement gesture like a guide
   endpoint; a crop edge is a framing decision judged against the whole picture.

---

## 6. Done when

The crop rectangle can be dragged while zoomed and panned and its handles move with the pointer;
panning does not slide the crop; a straighten line clears the guides and leaves the perspective
sliders alone; the loupe appears at the live end of a straighten drag; and the four existing crop
tests pass **unchanged**.

⚠ The last of those is the regression evidence, and it matters more than the new tests: this change
touches the coordinate handling of a tool that has worked since M1.4b.

# M5.5b — straighten by drawing a line

*Design, 2026-09-03. W4 track B. Settled with the user in W4-0; no decision below is the agent's to
re-open.*

Drag a line along something that should be level — a horizon, a roofline, a door frame — and let go.
The photo rotates so that line lands level. `Edits.geometry.angle` already exists and already
renders; **this spec adds no `Edits` field and no pipeline stage.** It adds a gesture, one pure
function, and one overlay.

---

## 1. What already exists, and is not reopened

- **`Edits.geometry.angle`**, degrees, ±45, rendered by `GeometryStage`'s `rotate` — which
  *"rotate[s] about the image center by `angle_rad`, keeping the canvas size"*. Already exposed as
  the **Straighten** slider at `panel/geometry.rs:155`.
- **`pipeline::perspective::guides::to_frame` / `from_frame`** map a point between the raw frame and
  the corrected one. Both are `pub`. Track B **calls** them and does not touch `pipeline`.
- **`canvas_geom::frame_rect` / `overlay_clip`** — where the whole frame sits on screen, and the
  widened clip a stroke needs so its halo is not shaved at the frame edge.
- **`Guide::family`** infers vertical-vs-horizontal from `dy >= dx`, and its doc records that the
  exact-45° tie-break *"is arbitrary but must be **decided** — the alternative is a line with no
  family at all."*

---

## 2. The gesture: an armed toggle, not a modifier-drag

**A ruler toggle sits on the Straighten slider's row** in `panel/geometry.rs`. While armed, a drag
on the canvas under `Tool::Crop` draws the straighten line; on release the angle is applied and
**the mode disarms itself**.

⚠⚠ **Not a modifier-drag, and the reason is a rule this repo already records.** `shell.rs`'s
`mode_switch` comment states it plainly: *"a feature reachable only by a key nothing on screen
mentions is a feature nobody finds"*. `spot_overlay.rs:165`'s ⌥-drag is the counter-precedent, and
it is a *refinement* of a gesture that already works without it — placing a spot needs no modifier,
picking its source by hand does. A straighten line has no unmodified form to refine.

It is also Lightroom's own primary affordance: the ruler button inside the crop panel, with ⌘-drag
as its shortcut. We ship the button and not the shortcut.

**Auto-disarm after one line** follows the eyedropper's recorded rule: *"an eyedropper that stayed
armed would turn every subsequent click — including one meant to select a mask or to dismiss
something — into a white-balance change."* The same applies here with more force, because the crop
rectangle is the thing every other drag in `Tool::Crop` belongs to.

### Precedence in the `Tool::Crop` arm

`canvas/mod.rs`'s `Tool::Crop` arm becomes a three-way decision:

```
if s.show_guides        -> guide overlay      (unchanged, still exclusive)
else if s.straighten_armed -> straighten overlay
else                    -> crop overlay       (unchanged)
```

⚠ **`show_guides` is tested first and is never written by this track.** Mutual exclusion is by
*gating*, not by cross-writes — see §5.

---

## 3. `crates/app/src/straighten_line.rs` — the pure half

A new **lib** module (no egui), because the angle is a rule a test must be able to reach without
opening a window. Its whole surface:

```rust
/// The straighten angle a drawn line asks for, in degrees, clamped to ±45.
///
/// `from`/`to` are in `[0, 1]` frame coordinates; `aspect` is width over height.
pub fn angle_for_line(from: [f32; 2], to: [f32; 2], aspect: f32) -> f32;
```

Four properties, each of which has bitten this codebase before:

1. **⚠⚠ Aspect-corrected, because normalised space is anisotropic.** `dx` must be multiplied by
   `aspect` before any `atan2`. This is the *fourth* appearance of that trap —
   `LinearGradient::coverage`, `mask_tool::hit_test` and the guide overlay each had to handle it,
   and `RadialGradient::coverage` needed no correction at all for a reason specific to axis-aligned
   ellipses. **Do not assume it cancels here; it does not.** A line drawn at 45° on screen on a 3:2
   frame is not at 45° in normalised coordinates.

2. **The family decides what "level" means.** Reuse `Guide::family`'s rule on the *aspect-corrected*
   delta: `|dy| >= |dx|` is a **vertical** and is rotated to 90°, otherwise it is a **horizontal**
   and is rotated to 0°. Lightroom's straighten tool behaves exactly this way — you draw along
   whatever is meant to be straight and it works out which.

3. **⚠⚠ A line has an orientation, not a direction.** The same line drawn right-to-left must give
   the same angle. `guides.rs`'s `mean_angle` records what happens without this: *"two guides traced
   in opposite directions average to roughly a right angle — garbage."* Fold both directions into
   one half-plane before measuring.

4. **Clamped to ±45**, matching the slider's range. A line drawn at 60° from horizontal is nearer
   vertical anyway, so the family rule makes the clamp unreachable in practice — which is exactly
   why it must be asserted rather than assumed.

⚠ A degenerate line — zero length, or under a few screen points — yields **no angle and no edit**.
`guide_overlay.rs` drops a zero-length guide on release for the stated reason that *"a tap that
never became a drag leaves a zero-length guide, which has no direction and would classify
arbitrarily"*; the same tap here would rotate the photo by an arbitrary amount.

---

## 4. `develop/canvas/straighten_overlay.rs` — the bin half

- **Rubber-band during the drag**, drawn against **`frame_rect`**, clipped to
  `canvas_geom::overlay_clip(img_rect)`. ⚠ `frame_rect`, never `img_rect`: once a region render is
  on, `img_rect` covers only the rendered slice, which is the drift bug the mask and guide overlays
  each record. `overlay_clip` rather than a tight clip, because `line_segment` centres its stroke on
  the path and a tight clip shaves the outer half of a halo drawn near the frame edge.
- **Halo then mark**, the crop overlay's convention: a dark wide stroke under a light narrow one,
  because *"a white line is invisible against a bright photograph."*
- **The line is stored on the raw frame.** The cursor is on the *corrected* frame, so the press and
  drag points go through `guides::from_frame`; the stored line is drawn back through
  `guides::to_frame`. This is `guide_overlay.rs`'s exact pattern, and it is what makes the line read
  **level after release** — the feedback that says the gesture worked, which matters most when the
  correction is a subtle 0.4°.
- **⚠ Mapped through `shown_geo`, not `s.edits`** — the transform the *visible* texture was rendered
  with. The guide overlay records why: geometry is a prefix stage, so at native zoom the re-render
  costs hundreds of milliseconds and mapping through the *requested* edits puts the line at its new
  position over the previous image. Reported there as the line jumping on release when zoomed in.
- **No loupe.** The guide overlay's exists for planting an endpoint precisely on a building edge;
  a horizon needs less, and a second consumer of `draw_loupe` would mean either sharing a private
  function across overlays or duplicating it. Noted as a possible follow-up, not shipped.

### The drag lifecycle

- **Frozen for the whole drag** — no render request while the pointer is down. The guide overlay's
  recorded reason: solving live *"was built and rejected in use — reported as 'jiggling too much':
  the whole photo shifts under the cursor on every pixel of movement."* One straighten line is a
  weaker case than two guides, but the same mechanism, and freezing costs nothing to implement.
- **Applied on release only**, with one `s.history.push` — one gesture, one Ctrl+Z, the rule every
  drag on this canvas follows.
- ⚠ **`drag_stopped` must be read OUTSIDE `if let Some(p) = resp.interact_pointer_pos()`.** The
  brush canvas records that on the release frame egui often reports no interact position, and
  nesting it there *"silently skipped both the render and the history push."*
- ⚠ **`tool_drag` / `tool_drag_started`, never a bare `dragged()`.** egui reports a drag for every
  button, so the middle-drag pan binding would otherwise draw a straighten line as it passed over
  the canvas.

---

## 5. `Session` state, and the two ownership amendments

Two new fields on `Session`, **view state, never `Edits`**:

```rust
straighten_armed: bool,
straighten_line: Option<([f32; 2], [f32; 2])>,   // on the RAW frame
```

⚠ The rule they follow is already written on `picking_wb`: *"View state on the session, never in
`Edits` — arming a tool is not part of the photograph's description, so it must not reach the
sidecar or travel with a paste."* `show_guides` and `selected_mask` follow it too. `Session` is
per-photo, so both reset on a switch for free.

The line is cleared when the mode is re-armed, the tool changes, or the photo changes.

### ⚠⚠ Amendment 1 — B may write `s.edits.geometry.angle`

`docs/PARALLEL-WORK.md` §4 bars track B from "`Edits`". Read literally that makes the feature
impossible. **It means: add no `Edits` field, and do not touch the `edit-model` crate.** B writes one
existing field, adds none, and moves no golden.

### ⚠⚠ Amendment 2 — B reads `show_guides` and `guides`, and writes neither

The toggle is `add_enabled(!s.show_guides && s.guides.is_empty(), ..)`, and the `Tool::Crop`
dispatch tests `show_guides` first. Reads only.

**That gating also disposes of a wart for free.** A straighten line writes `geometry.angle`, which
is one of the three values a guide solve owns — so drawn guides would be left describing a solve
that no longer holds. `panel/geometry.rs:203` cannot catch it: it captures `solved_before` *within*
the frame, before the sliders, so an angle written by the canvas on the **previous** frame is
already in `s.edits` and does not read as a hand edit. Gating on `guides.is_empty()` makes the state
unreachable instead of guarding against it. "Clear" already leaves the solved sliders alone
(`clearing_the_guides_leaves_the_correction_alone`), so the workflow is: Clear, then draw.

---

## 6. The black corners are the user's to crop

`GeometryStage::rotate` keeps the canvas size and there is **no autoscale for straighten at all** —
verified: no `inscribed`, `constrain`, `shrink` or `autocrop` helper exists in `crop_tool.rs`,
`stages/geometry.rs` or `crop_overlay.rs`. So the Straighten slider has always left black wedges,
and the line gesture behaves identically.

⚠ **Lightroom's "constrain to image" is deliberately not implemented here**, and the reason is
consistency rather than effort: auto-shrinking on the *line* but not the *slider* makes one field
behave two ways, which is the shape this project's records keep naming as a defect found later by
use. Doing it on both would change existing slider behaviour (past M5.5b's scope) and would fight a
crop the user framed deliberately.

⚠ Consequence, stated so it is not filed as a bug: **release the drag and four black wedges appear.**
Lightroom never shows you that. If it is later judged intolerable, the largest inscribed rectangle
at a given angle is clean pure maths and belongs in `straighten_line.rs`, applied from **both**
affordances with a "only if the crop is still full-frame" guard.

---

## 7. Ownership, and what B must not touch

| B owns | B must not touch |
| --- | --- |
| new lib `app/src/straighten_line.rs` | `guide_overlay.rs` |
| new bin `develop/canvas/straighten_overlay.rs` | `Session.guides` / `show_guides` (**writes** — reads are fine, §5) |
| **one dispatch branch** in `canvas/mod.rs`'s `Tool::Crop` arm | `edit-model` (no new `Edits` field) |
| the toggle **on the Straighten slider's row** in `panel/geometry.rs` (~line 155) | `pipeline` (calls `guides::to_frame`/`from_frame`, changes nothing) |
| two view-state fields on `Session` in `main.rs` | goldens — **53, byte-identical** |
| | D's hover code at the end of `canvas/mod.rs::show` |
| | C's Auto button in the **Guides** block (~line 222) |

⚠ **The ~70 lines between B's toggle and C's Auto button in `panel/geometry.rs` is deliberate
placement, not luck.** Put the toggle beside the Straighten slider it belongs to and C's rebase is
clean; put it in the Guides block and the two collide.

⚠ **Merge order is B → D → C** (revised from §4's D → B → C). B is the only track touching all three
contended surfaces — `canvas/mod.rs`, `panel/geometry.rs`, and `Session` in `main.rs` — so §3's rule
*"the owner of the most contended surface merges first"* puts B first. B and D both append a field to
`Session`; expect a trivial conflict there and keep both.

⚠ **Files already over ~600 lines that B will edit** — `canvas/mod.rs` (992), `panel/geometry.rs`
(742), `main.rs` — must be **named in B's report**, per §5.

---

## 8. Testing

Headless, in `straighten_line.rs`, with no window:

- **A horizontal-ish line levels to 0°, a vertical-ish one squares to 90°** — the family rule, both
  branches.
- **⚠ The aspect correction is load-bearing, and the test must have leverage.** A line at 45° in
  *normalised* coordinates on a 3:2 frame is at ~33.7° on screen. Probe with a line whose two
  answers differ by more than any plausible tolerance, and **assert the on-screen angle**. M3.1a
  records the sibling failure: `hit_testing_the_line_is_aspect_correct` *"used a horizontal axis with
  the probe directly below it, where the perpendicular distance is purely vertical — it passed with
  the aspect correction deleted."* A test using an axis-aligned line here is blind for the same
  reason: `dx` or `dy` is zero and scaling it changes nothing. **Use a diagonal.**
- **The same line drawn both ways gives the same angle** — the orientation-not-direction fold.
- **A degenerate line yields no edit.**
- **The clamp holds** at ±45.

Wiring, in `canvas/mod.rs`'s test module, driving the real `canvas::show` through `test_support`:

- **⚠⚠ The wiring test is the half this project has repeatedly shipped broken.**
  `an_armed_eyedropper_sets_the_white_balance_from_a_click` exists for exactly this: *"the solve has
  its own unit tests, but nothing else can see that a click on the canvas ever reaches it."* Arm the
  mode, drag, assert `s.edits.geometry.angle` moved **and** `s.history.can_undo()`.
- **⚠ Assert on `history.can_undo()`, not only on `Edits`.** The headless-canvas work records that
  *"the edits change during the drag, from `apply_drag`; only the undo push happens on release"* —
  so an `Edits` assertion alone cannot see a missing release commit.
- **⚠ Sample the request counter on the drag frame itself.** Same record: *"once `Edits` changes, a
  later frame issues a request regardless… In the real app that later frame never comes — egui idles
  after a drag."*
- **The mode disarms** after one line.
- **A drag with the mode unarmed reaches the crop overlay**, not the straighten one — the guard on
  the guard, without which "the angle moved after a drag" means nothing.
- **The toggle is disabled while `show_guides` is on or guides exist.**

**The sabotage rule applies to every one of these**: break the thing the test names and watch it
fail, with `ran = passed + failed >= 1` confirmed so a test that did not compile cannot read as a
pass. This project has recorded **sixteen** instances of a test that could not detect the bug it was
named for, and three of the traps are live here — no leverage, a zero component in the input, and a
harness that feeds zeros.

---

## 9. Accepted caveats

1. **Black wedges after the gesture**, as the slider has always left them (§6). The most likely thing
   to be reported.
2. **Two clicks, not one** — arm, then drag. No ⌘-drag shortcut.
3. **No loupe**, so an endpoint cannot be planted with the precision the guide overlay offers.
4. **Not available while perspective guides are in force** — Clear first. Deliberate (§5).
5. The stored line is transient: switching photo or tool discards it, while the angle it produced
   persists. The rule guides already follow.

---

## 10. Done when

Dragging a line along a horizon levels the photo, the drawn line reads **level** afterwards, one
Ctrl+Z undoes it, the mode disarms itself, and the report says **what was run in the GUI** — not
only what passed. Every canvas defect in this project's record was found by use and none by a test
before it was reported.

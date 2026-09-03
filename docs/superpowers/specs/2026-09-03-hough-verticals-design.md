# M7.4 — automatic vertical detection

*Design, 2026-09-03. W4 track C. Settled with the user in W4-0; no decision below is the agent's to
re-open.*

Guided perspective works and requires you to trace the lines by hand. This is the automatic half:
one **Auto** button finds two verticals, solves, and leaves the inferred guides on screen where the
existing overlay can edit them.

⚠⚠ **This is the one track in W4 whose "Done when" is a judgement about a photograph.** §6 of
`docs/PARALLEL-WORK.md` is explicit: *"An agent can produce the comparison; it should not sign it
off."* Produce the before/after on a real off-axis building and stop; **the user signs it off.**

---

## 1. What already exists, and is not reopened

- **`Guide { from, to }`** — one drawn line in `[0, 1]` frame-normalised coordinates on the **raw**
  frame, with `family()` inferring vertical-vs-horizontal from `dy >= dx`.
- **`guides::solve(vert, horz, aspect) -> (pitch, yaw, roll)`** — the pure closed-form solver.
  ⚠ It needs **exactly two guides per family**, and `solve_angles`'s doc records that the
  single-family case *"is the general one with the other angle zero, not [a] special case bolted
  on"*. So verticals alone give pitch, plus roll from the guides' own directions.
- **`RustroomApp::solve_guides`** (`interaction.rs:167`) already reads `Session.guides`, takes at
  most two per family, writes `perspective.vertical` / `perspective.horizontal` /
  `geometry.angle`, and returns whether anything moved.
- **`guide_overlay.rs`** draws and edits whatever is in `Session.guides`, mapped through the live
  correction, with a loupe. **C gets all of that for free and touches none of it.**
- **`auto_tone_tool.rs`** is the precedent for an analysis pass: a pure function of a buffer and an
  `Edits`, reading `Session.buffer` at a fixed `ANALYSIS_DIM` of 512, with `Orientation` undone
  explicitly.

---

## 2. What the detector runs on

**The decode buffer, sampled through the lens corrector and the orientation, at a fixed analysis
dimension.**

The target coordinate space is fixed by what already exists: a `Guide` is stored **post-lens-
correction, post-orientation, pre-straighten, pre-perspective, pre-crop**. That follows from two
facts — `guides::to_frame` maps a stored guide through the perspective homography and the straighten
angle *only*, and the Guide Mode preview keeps the correction and drops just the crop
(`the_guides_preview_keeps_the_correction_and_only_drops_the_crop`). Whatever the detector reads must
land in that space, or every detected line needs an inverse map that does not exist.

So the glue builds its analysis image by sampling `Session.buffer` through:

1. **`lens_profile::Corrector`**, exactly as `spot_overlay::sampler_for` composes it from
   `s.lens.shot` and `s.edits.lens_correction`, and
2. **`Orientation`**, whose inverse is *turn back, then mirror* — `auto_tone_tool::source_crop`
   spells that out, along with the reason it matters: *"a crop taken on the left of the oriented
   image is the bottom of the buffer."*

…at a fixed long edge (`ANALYSIS_DIM`, 512, `auto_tone_tool`'s constant and its reasoning). The
result is already in Guide space, so **there is no inverse mapping anywhere in this track.**

### ⚠⚠ The lens step is the part an agent would skip, and skipping it is a plausible-looking bug

The prefix order in `pipeline/src/lib.rs:197` is **LensCorrection → Perspective → SpotRemoval →
Geometry**, so the decode buffer is *pre*-lens-correction — and CLAUDE.md records that *"lens
correction is on by default"* for RAW. On the raw buffer, barrel distortion **curves every building
edge**: each edge then fragments into several Hough lines at slightly different angles, and the
vanishing point they imply is biased. The output is not an error; it is a correction that is merely
*plausible*, which this project's records name repeatedly as its worst failure shape.

### Why not the rendered frame

- **As rendered** it already has the current perspective applied, so lines already corrected have no
  vanishing point left to find. Rejected outright.
- **With perspective and straighten stripped** it would also land in Guide space and would get
  exposure/tone contrast for free — but it costs an async render round-trip on a button press, is
  available at fit rung only (~1600 px), and C is barred from `canvas/**` so the read-back needs a
  new seam. More machinery, no accuracy gain.

⚠ And the auto-tone precedent already settled the general form of this question, for a reason that
applies here too: *"the buffer it reads is `Session.buffer`, never the rendered texture. That texture
already has the current exposure and tone applied, so analysing it would make the auto chase its own
correction."*

---

## 3. `pipeline/src/perspective/detect.rs` — the pure half

Luminance plane in, candidate segments out. Nothing here knows a GUI exists.

```rust
/// The two best near-vertical lines in a luminance plane, as frame-normalised segments.
pub fn verticals(luma: &[f32], w: usize, h: usize) -> Vec<Guide>;
```

### ⚠⚠ Do NOT reuse `visualize::edge_view`, and this contradicts the roadmap

`03-plan-of-action.md`'s M7.4 entry says *"`app::visualize::edge_view` … is already an edge map over
luminance; a Hough pass has somewhere to start."* Two reasons that is wrong:

1. **It is `|luma − gaussian_blur(luma)|`** — a high-pass tuned for **dust spots**, gain-mapped and
   clamped. It carries **no gradient direction**, and direction is exactly what makes a ±30° window
   cheap and specific rather than a full accumulator filtered after the fact.
2. **It lives in the app lib**, so `pipeline` cannot call it. The options were to move it down to
   `pipeline::spatial` first or to recompute; recomputing is right, because the thing wanted is not
   the same operator.

Compute a **Sobel** in `pipeline::spatial` instead — magnitude *and* direction. ⚠ `spatial` today has
`box_blur`, `gaussian_blur`, `wide_blur`, `guided_filter`, `min_filter` and no gradient at all, so
this is a genuinely new primitive and belongs there beside them, not private to `detect.rs`.

### The Hough pass

- **Verticals only**, ±30° of vertical. §4 below covers both choices.
- Accumulate only pixels whose **gradient direction** is consistent with a near-vertical edge and
  whose magnitude clears a threshold. That gate is the whole reason for computing direction: it keeps
  the accumulator small and stops roof diagonals voting.
- **⚠⚠ Return the two most widely SEPARATED strong lines, not the top two by vote.** `guides.rs`
  already records why: *"the vanishing point of two nearly-parallel lines sits enormously far away
  and moves wildly for a pixel of drawing error, so a roll taken from it is ill-conditioned exactly
  when the correction is gentlest."* Two near-coincident detections on one edge of one building are
  the commonest way that happens, and taking them makes the solve unstable while looking like
  success. Score candidates by strength **and** by separation.
- Convert each accumulator peak to a `Guide` spanning the frame, in `[0, 1]`.
- **Fewer than two found → return fewer than two.** The glue reports it; `detect` does not invent
  one.

---

## 4. The two scope decisions

**Verticals only.** The roadmap's wording and the milestone's own name — *"Automatic vertical
detection"*. Two verticals give the vertical vanishing point → pitch, plus roll from their
directions, which is the classic shot-looking-up-at-a-building keystone and the case that motivates
the feature. Horizontals are an obvious later slice against machinery that will then be proven.

⚠ Cost, stated: a building you are also *turned* away from needs yaw, and yaw comes from the
horizontal family. This slice does not correct that.

**±30° from vertical.** Wide enough for an aggressive wide-angle upward tilt, where the outer
verticals of a tall building lean 20–25° near the frame edges, and still specific enough to ignore
most roof diagonals.

⚠ The window was chosen from the **asymmetry of the failure modes**, not from tidiness: a missed line
means the button appears broken and there is nothing on screen to act on, while a false positive
draws a **visible, editable guide** the user nudges or deletes. ±20° is cheaper and misses exactly
the shot the feature exists for; a full 180° accumulator is pure waste with one family.

---

## 5. `crates/app/src/perspective_detect.rs` — the glue

A new **lib** module (no egui), following `auto_tone_tool.rs` line for line: a pure function of a
buffer and an `Edits`, unit-testable with no window and no RAW fixture.

```rust
/// The analysis runs on a downscale to this long edge.
pub const ANALYSIS_DIM: usize = 512;

/// Verticals detected in the photograph, in Guide coordinates.
pub fn detect_verticals(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<&LensShot>,
) -> Vec<Guide>;
```

It builds the analysis luminance plane (§2), calls `detect::verticals`, and hands back `Guide`s. ⚠
Luminance is **`color::luminance`**, the working space's Y — *"the quantity every tuned constant in
this pipeline is measured against"*, per `auto_tone`'s module doc.

## 6. One app method, beside `solve_guides`

In `interaction.rs`, next to `solve_guides` and following its shape:

1. Call `perspective_detect::detect_verticals`.
2. Fewer than two → **set `self.status`** and return. ⚠ Never silence: a button that appears to do
   nothing is the worst outcome, and the status cluster is where this app already reports what it
   just did.
3. Two or more → write `Session.guides`, **set `show_guides = true`**, call the existing
   `solve_guides`, issue `s.request(..)`, and `s.history.push`.

⚠⚠ **`show_guides = true` is not a convenience.** `canvas/mod.rs` draws the guide overlay only under
`Tool::Crop && s.show_guides`, so without it the inferred guides are invisible and the roadmap's
"Done when" — *"the guides it inferred are visible and editable"* — is not met.

⚠ One `history.push`, so one Auto click is one Ctrl+Z. `solve_guides`'s own caller in
`canvas/mod.rs::show` already does exactly this on a settled guide drag; follow it rather than
inventing a second route.

**The Auto button** goes in the **Guides** block of `panel/geometry.rs`, beside "Clear" (~line 222).

---

## 7. Ownership, and what C must not touch

| C owns | C must not touch |
| --- | --- |
| new `pipeline/src/perspective/detect.rs` | **`canvas/**` entirely** — the existing overlay draws and edits the guides for free |
| a Sobel primitive in `pipeline/src/spatial.rs` | `edit-model` (no new `Edits` field) |
| new lib `app/src/perspective_detect.rs` | `crop_tool.rs` |
| one method beside `solve_guides` in `interaction.rs` | goldens — **53, byte-identical** |
| the **Auto** button in `panel/geometry.rs`'s Guides block (~line 222) | `visualize.rs` — **read it, do not change it** (§3) |
| | B's toggle on the Straighten slider's row (~line 155) |

⚠ **C merges LAST: B → D → C.** Revised from §4's D → B → C, because B's armed-toggle decision moved
B into `panel/geometry.rs` — which C also edits — and into `Session`, making B the most contended
track. C rebases onto B's panel change; the ~70 lines between the two edit sites is what keeps that
trivial.

⚠ **`panel/geometry.rs` is 742 lines** and must be **named in C's report**, per §5. ⚠ `spatial.rs`
gains a primitive rather than `detect.rs` growing a private one, so the crate boundary in §5 holds:
`pipeline` has zero GUI dependency and `detect.rs` is driven headless by its own tests.

---

## 8. Testing

Headless in `detect.rs`, on **synthetic** planes — no RAW fixture, so it runs in CI:

- **A synthetic frame with two converging verticals is detected, and the angles come back within
  tolerance of the ones drawn.** ⚠ Assert the *recovered geometry*, not merely that two lines came
  back. `guides.rs` records the sibling failure directly:
  `a_horizontal_family_engages_the_yaw` *"only asserted both angles were non-zero — and a wrong
  angle is still an angle."* The replacement derived the vanishing points from a known rotation and
  asserted the solver recovers it; do the same here.
- **⚠ Two near-coincident strong edges plus one weaker distant edge must return the separated
  pair.** A "top two by vote" implementation passes any test that only counts results, so this is
  the only test that can see it. Build the fixture so the wrong answer is *attractive* — the
  `spot_source` lesson: *"each placed its fixture where a wrong implementation had nothing to reach
  for."*
- **A line at 40° from vertical is NOT detected; one at 25° is.** Both halves — a window that
  accepted everything and one that accepted nothing would each pass a one-sided test.
- **A flat plane returns nothing**, and does not panic.
- **Sobel direction is load-bearing**: a fixture with a strong *horizontal* edge must contribute no
  votes. Sabotage by ignoring direction and watch it fail — otherwise the ±30° window is being
  enforced by the accumulator's geometry alone and the direction gate is untested.

In `perspective_detect.rs`:

- **⚠⚠ The lens step is reached.** A fixture with a distortion profile must produce different
  guides from the same buffer with corrections off. This is the test for §2's central decision, and
  nothing else can see it — the guides look perfectly plausible either way.
- **Orientation is undone.** A quarter-turned `Orientation` must not turn the detected verticals into
  horizontals. `auto_tone_tool::source_crop`'s test is the model.

Wiring, in `interaction.rs` or `panel/geometry.rs`:

- **⚠ Clicking Auto writes `Session.guides` AND sets `show_guides`.** Assert both. A test asserting
  only the guides passes against a version whose output is invisible.
- **Clicking Auto moves the perspective sliders**, i.e. the existing `solve_guides` is actually
  reached — the wiring half this project has repeatedly shipped broken.
- **Fewer than two detected sets a status message** rather than leaving it empty.

**The sabotage rule applies to every one of these**, with `ran = passed + failed >= 1` confirmed so a
sabotage that failed to compile cannot read as a pass — the recorded trap. ⚠ And a sabotage that
changes no behaviour is **not evidence**: say so rather than counting it, as the `capture_days` and
`ON CONFLICT` entries in CLAUDE.md do.

---

## 9. Accepted caveats

1. **Verticals only** — a building you are turned away from keeps its converging horizontals (§4).
2. **±30°**, so an extreme upward tilt at the frame edge can still be missed.
3. **Two lines, not a least-squares fit over more** — inherited from `guides::solve`, which is
   exactly determined by design.
4. **The detector cannot know what a building is.** Bookshelves, railings, fence posts and window
   blinds are all excellent verticals. The mitigation is that the guides are visible and editable,
   which is the whole reason for solving immediately rather than proposing.
5. **Runs on a 512 px analysis image**, so a very fine or very short edge does not survive the
   downscale. Deliberate — it keeps the button instant on a 26 Mpx buffer.
6. **The button overwrites** whatever Vertical, Horizontal and Straighten held, undoable in one step
   — the same behaviour a manual guide solve already has.

---

## 10. Done when

**A building photographed off-axis is corrected by one button, and the inferred guides are visible
and editable.** ⚠ The agent produces the before/after on a real off-axis building and reports what
was **run in the GUI**; the user signs the photograph off (§6 of `PARALLEL-WORK.md`).

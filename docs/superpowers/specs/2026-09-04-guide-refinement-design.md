# M7.4 — Auto becomes a refinement of your own guides

*Design, 2026-09-04. Supersedes the automatic half of `2026-09-03-detector-contrast-floor-design.md`
as the route to M7.4's "Done when". Decisions settled with the user; none is the implementer's to
re-open.*

---

## 1. Why the paradigm changes

Two rounds of work fixed real faults in the detector and neither made Auto usable.

- **The contrast floor** (relative → absolute, on display-encoded luminance) recovered Weltenburg
  Abbey, which had found nothing at all.
- **The contiguity rule** stopped candidates reaching their length threshold from fragments
  scattered down the frame — measured, the winning lines had 24–41% of rows voting but a longest
  contiguous run of **7–13%**.

Both improved **edge quality**. Neither touched **selection**, and selection is the whole problem.
Reported in use: *"it seems like it just puts the guides at random, they are not fixed to the
building."* Measured after both fixes, on the two known frames, **one line is genuinely on the
building and the other is a cliff face or a person's silhouette** — real, contiguous, near-vertical
edges that no edge-quality rule can reject, because there is nothing wrong with them as edges.

⚠⚠ **A Hough accumulator cannot tell a colonnade from a cliff.** That is semantic knowledge, and the
only thing in the room that has it is the person looking at the photograph. This design gets it from
them in one gesture they already perform.

⚠ It also inverts the risk. Today a wrong Auto silently writes Vertical −100. Here Auto refines
something you pointed at, so a wrong answer is visibly wrong and locally correctable.

---

## 2. What the button does

| state | label | behaviour |
| --- | --- | --- |
| no guides | **`Auto`** | unchanged — propose two, solve, leave them editable |
| any guide exists | **`Refine guides`** | replace each near-vertical guide with the best real edge near it, or leave it exactly as drawn |

⚠⚠ **The label changes, and that is the point rather than polish.** One button doing two things with
nothing on screen saying which is precisely the shape that produced this project's greyed-out
straighten-button confusion, whose lesson is recorded as *"a rule the user cannot see is one they
cannot correct."* Two permanent buttons were rejected: one would be dead most of the time, in a
panel this project has already worked to declutter.

⚠ **Auto keeps proposing its own two when there are no guides.** Its picks are still often wrong —
but they are now a starting point you refine rather than a correction silently applied. **The honest
framing of this whole change is that Auto stops being an answer and becomes a suggestion.**

---

## 3. Only near-vertical guides are touched

`detect::verticals` finds verticals only, so a horizontal-family guide has nothing to snap to and is
returned **untouched**. Silently "improving" it would be inventing a result.

⚠ The family is inferred from the guide itself the way `guide_overlay` already infers it —
`|dy| >= |dx|` — so there is one rule for what counts as a vertical, not two that can disagree.

---

## 4. The snap metric: one constant, not two

A drawn line and a candidate differ in **angle** and in **position**, which is naturally two
thresholds in two different units, neither of which a person can judge. Instead the metric is the
quantity you would actually argue about by eye:

> the largest gap between the two lines anywhere down the frame —
> `max over y of |x_candidate(y) − x_drawn(y)|`, as a fraction of frame width.

One number. It subsumes angle and offset together: a candidate that matches your angle but sits
20 px away fails, and so does one that starts on your line and diverges.

```rust
/// How far a candidate may sit from the guide it refines, as a fraction of frame width.
pub const SNAP_RADIUS: f32 = /* chosen by measurement, per the two ⚠ notes below */;
```

⚠⚠ **The radius is load-bearing, not a tuning detail.** Without a bound, the person's silhouette
beside Walhalla's building corner captures a line aimed at the corner, and snapping re-creates
today's bug with extra steps. It must be small enough that a wrong snap is close to impossible.

⚠ **It is measured, not asserted.** A starting value near 3% of frame width is a guess, and this
milestone has already had one guess overturned by measurement — the spec's "starting band of 6–10
levels" for `MIN_EDGE_LEVELS`, which fired on 84% of frames.

---

## 5. Inside the radius, the strongest candidate wins

Not the nearest. The radius already encodes *"this is what I meant"*; within it you want the best
edge, and "nearest" would let a weak spurious line one pixel away beat the true edge three away.

⚠ Stated as a decision so it is not later found as an inconsistency: with a small radius there is
usually only one candidate in range, so this matters rarely — but when it matters it is the case
that produced the original complaint.

---

## 6. Shape of the code

**`detect::refine(luma, w, h, guides) -> Vec<Guide>`** in `pipeline`, pure, beside `verticals`.

⚠⚠ **It reuses the same candidate list — floor, direction gate, contiguity, peak-finding, all of
it — and replaces only the final selection step.** So a candidate that fails contiguity can never be
snapped to, and every property the last two rounds established still holds. Refactoring the shared
half out of `verticals` is part of the work, not a separate cleanup: two candidate pipelines would
drift, which is the rule `test_support.rs` already states for duplicated helpers.

⚠ **The separation rule does not apply here.** `MIN_SEPARATION` exists to stop the *detector*
choosing an ill-conditioned pair; here you chose the pair. Drawing two near-parallel guides is
already possible today and already ill-conditioned, so this makes nothing worse — but it is a real
caveat and belongs in §9 rather than being quietly fixed.

**`perspective_detect::refine_guides`** is the app glue, mirroring `detect_verticals` exactly.

**`interaction::auto_guides`** gains the branch. It already has the shape this needs: it reports
outward from the panel, sets `show_guides`, re-solves, requests a render, and pushes **one** history
entry so one click is one Ctrl+Z. Refinement follows all of that unchanged.

---

## 7. Reporting, and the no-match rule

⚠⚠ **A guide with no candidate in range is returned BIT-IDENTICAL, and the status line says so.**
Your line is never moved toward something you did not aim at. The status cluster is where this app
already reports what it just did, and the message must distinguish the two outcomes — otherwise you
cannot tell a guide that snapped from one that did not, which is the difference between a precise
line and a rough one:

> `Refined 2 of 2 guides.` / `Refined 1 of 2 — no edge found near the other.`

---

## 8. Testing

**The one test that matters, and the only one that can distinguish §5's choice:**

> **A strong wrong edge NEAR the drawn line, and the true edge exactly on it.** Every other test
> passes against either rule; this one pins whichever we chose. ⚠ Build it with the wrong edge
> *inside* the radius, or it tests nothing.

Also:

- **A guide with no candidate in range comes back bit-identical** — `assert_eq!` on the guide, not a
  tolerance, because "unchanged" is exact and a tolerance would hide a small unwanted move.
- **A horizontal-family guide is untouched**, even on a frame full of strong verticals.
- **A guide drawn slightly off a real edge lands on it** — the feature working at all.
- **A guide drawn far from every edge is unchanged**, which is the radius doing its job.
- **`verticals` keeps every existing test passing verbatim** through the refactor in §6. That is the
  evidence the shared half moved no behaviour, and it matters more than the new tests.
- Fixtures use the analysis plane's real shape (**512×342**), not a square. ⚠ Every fixture in
  `detect.rs` was square until this milestone, and this project has recorded an aspect bug four
  times; a square fixture structurally cannot see one.

⚠ **Every sabotage confirmed against the named test, with the edit verified present first.** This
session alone produced three sabotages that passed while proving nothing — one against a fixture
whose gaps a Gaussian band could not bridge, and two where the guard was genuinely untested.

---

## 9. Accepted caveats

1. **Auto's own two picks are still often wrong.** This change does not improve them; it makes them
   cheap to fix. That is the trade, taken deliberately.
2. **Two near-parallel hand-drawn guides remain ill-conditioned**, because refinement does not
   enforce separation — the pair is yours to choose.
3. **Horizontal guides are never refined**, since the detector has no horizontal half.
4. **A refined guide is only as good as the candidates**, so an edge the floor or the contiguity rule
   rejected cannot be snapped to however precisely you point at it.
5. **The rectangle-constrained re-detect is NOT in this spec.** It supplies roughly the same
   information as drawing the line roughly, at more interaction cost, and earns its place only where
   an edge is too faint to trace but its region can be bounded. Revisit if snapping proves
   insufficient.

---

## 10. Done when

Draw two rough guides down a building's edges, press **Refine guides**, and they land on the real
edges; a guide drawn away from any edge stays where you put it and the status line says so; and
`verticals`' existing tests pass unchanged.

⚠⚠ **M7.4's "Done when" is still a judgement about a photograph.** Produce the before/after on the
abbey and Walhalla; **the user signs it off.**

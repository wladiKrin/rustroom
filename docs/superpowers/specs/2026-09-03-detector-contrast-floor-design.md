# M7.4 — an absolute contrast floor for the vertical detector

*Design, 2026-09-03. Unblocks track C, which was held out of W4. Decisions settled with the user;
none is the implementer's to re-open.*

W4's track C built the detector and then reported, correctly, that it was not ready. This is the
one change that stands between it and landing.

---

## 1. What is actually wrong

Track C measured rather than guessed, and the numbers name a design fault rather than a tuning one.

- **Walhalla (DSCF6270).** Auto wrote **Vertical −42.5, Straighten +7.75°** and left the columns
  leaning *left*. The two detected lines sat at x ≈ **0.68 and 0.85 — in the sky, not on the
  building** — near-parallel at +4.00° and +1.00°, and cleared `MIN_SEPARATION` 0.15 at **0.17**.
- **Weltenburg Abbey (DSCF6296)**, the cleanest converging-verticals frame available: finds **0**
  and correctly reports "need two". Lowering *either* `MIN_LENGTH_FRACTION` (0.35→0.15) *or*
  `MAG_FRACTION` (0.25→0.10) makes it fire.
- **Across 70 real RAWs:** fires on **54%**; of those, **47% ask for |Vertical| ≥ 50** and 58% for
  |Straighten| ≥ 5°. It fired on a blurred kitten close-up (+95.3), a flat wall poster (+88.9) and a
  tree trunk (−38.4).

**Every threshold is relative to the frame's own single strongest gradient** — `MAG_FRACTION` is
`strongest * 0.25`. That is simultaneously too permissive on a soft frame, where *strongest* is
small and a smooth sky gradient clears a quarter of it, and too strict on the abbey, where
something brighter than the building sets the bar above the building's own edges.

⚠ A **relative** threshold structurally cannot express *"there is no vertical in this
photograph"* — which is why track C had to add `MIN_LENGTH_FRACTION` mid-flight just to stop a
landscape always returning two lines.

---

## 2. ⚠⚠ The reframing: an absolute floor on *linear* luminance would be meaningless

`perspective_detect::analysis_luma` produces `color::luminance` — **linear, scene-referred
ProPhoto**. An absolute threshold there is close to meaningless: a gradient of 0.01 in deep shadow
is a strongly visible edge, and the same 0.01 near white is invisible. One number cannot be right
across a frame, so the constant would be biased toward highlights and impossible to argue about.

**This project has already been burned by exactly this class of error**, in the Whites slider:

> The region weights read as perceptual thirds — blacks under a third, whites over two thirds — but
> `l` arrives as **linear** luminance, where 2/3 is sRGB **0.85** and 1/3 is sRGB **0.60**. So
> Whites saw only the top sliver while Blacks covered more than half the visible range… The weights
> are taken on the **display-encoded** luminance now.

So the floor gets the same treatment: **display-encode the plane before the Sobel.** After that the
threshold is expressible in **8-bit levels of contrast across an edge** — a quantity a person can
judge and disagree with.

⚠ It also disposes of the scene-referred range for free: `color::linear_to_srgb` clamps to `[0, 1]`,
and a clipped highlight has no gradient to lose anyway.

⚠ And it makes the two observed failures one story: on the **kitten** no edge clears a real contrast
floor, and on the **abbey** the building's edges stop being measured against whatever the brightest
thing in frame happens to be.

---

## 3. The encode lives inside `detect::verticals`

Not in the glue. `detect::verticals` takes a **linear** luminance plane and encodes it itself.

⚠⚠ That makes the wrong colour space **unrepresentable at the boundary** rather than something a
caller must remember — the reasoning `LensCorrectionStage` records for building its own `Corrector`
instead of accepting one: *"handing one in would make a size mismatch representable, and it would be
silently wrong rather than an error."*

`analysis_luma` keeps returning linear luminance, so its name stays honest and the app-side glue is
untouched.

---

## 4. `MIN_EDGE_LEVELS` replaces `MAG_FRACTION`

```rust
/// The contrast an edge must carry to cast a vote, in 8-bit levels across the edge.
pub const MIN_EDGE_LEVELS: f32 = /* chosen from the sweep in §5 */;

/// Sobel's response to a unit step: the kernel sums to 4 on the leading side.
const SOBEL_STEP_GAIN: f32 = 4.0;
```

A pixel votes when `mag[i] / SOBEL_STEP_GAIN * 255.0 >= MIN_EDGE_LEVELS`.

⚠ The division by the step gain is what makes the constant mean what it says. Without it the number
is a Sobel magnitude, which is four times an edge's actual contrast and therefore not a quantity
anyone can reason about.

**`MAG_FRACTION` goes entirely.** It is the direct cause of *both* failures, so keeping it in any
form keeps the bug in one of its two directions — and the abbey's edges were rejected *by the
relative test specifically*.

⚠ **Magnitudes still weight the accumulator**, so a strong edge is still preferred over a faint one
of the same length. What changes is only **who is allowed to vote**.

⚠ **The starting band is 6–10 levels and that is a starting band, not a value.** A building edge
against sky is 50+ levels; sensor noise on a smooth sky at 512 px is 1–2; blur collapses both. The
value is read off the sweep in §5. **Do not pick a number here and defend it later.**

---

## 5. How the constant is chosen, and how we know it is right

### The labelled corpus

The user labels **15–25** of the 70 RAWs as *should fire* or *should stay silent*. Five are already
known: **abbey** (must fire, and on the building), **Walhalla** (must fire on the building, or stay
silent — not on the sky), **kitten**, **wall poster**, **tree trunk** (all must stay silent).

⚠ **The labels file is not committed.** It names photographs on one machine, so it follows the
convention this project already has for `RAW_FIXTURE` and `DCP_FIXTURE`: *"Fixtures this session's
tools depend on, none of them in the repo."* The probe takes its path as an argument, and says so
plainly when it is absent rather than reporting a vacuous pass.

### The sweep

`detect_probe` gains a mode that, for each candidate value of `MIN_EDGE_LEVELS`, reports
**precision and recall** against the labels, and names which frames it got wrong.

⚠⚠ **Recall is what stops the degenerate outcome.** A floor that silences everything scores
perfectly on fire-rate — the statistic C had — and is useless. Only recall exposes it, which is why
"fires on 54%" was never by itself a verdict: some of those 70 frames genuinely have verticals.

⚠ The value is **read off the curve**, and the chosen number's doc comment records the sweep it came
from. A tuned constant with no recorded reasoning is one nobody can revisit — the rule
`MEDIAN_TARGET` and `TEXTURE_GUIDED_EPS` both follow.

### Existing fixtures will move, and that is not licence to tune

⚠ `detect.rs`'s fixtures paint 0.05 and 1.0 linear, which encode to ~0.24 and 1.0. The edges stay
strong, but **every threshold now means something different**, so some fixtures need re-judging.
**Each change must be justified by what the fixture is for** — never nudged until the test is green.
This project records three tests that asserted something false about correct code, and
`texture_minus_100_removes_the_fine_band`'s threshold moving twice, *"each time because the model
changed and never to make a red test green."*

---

## 6. Testing

**The one test no current fixture can provide**, and the only thing that can see the encode being
deleted:

> **Two frames with the same *linear* gradient — one in deep shadow, one near white.** Under a
> linear Sobel they are identical. Display-encoded, the shadow edge is far larger, because the sRGB
> curve expands the shadows. So the shadow frame clears the floor and the bright one does not.

⚠ Without it, dropping `linear_to_srgb` leaves every other test green — the fixtures all use strong
edges that clear the floor either way. This is the same shape as the sixteen recorded instances of a
test that could not detect the bug it was named for, and the direct-gate sabotage C already hit,
where `MIN_LENGTH_FRACTION` was silently doing the direction gate's work.

Also:

- **A frame whose every edge is below the floor finds nothing**, and one just above it fires — both
  halves, since a floor that admits everything and one that admits nothing each pass a one-sided
  test.
- **The constant means levels.** A synthetic step of a known contrast either side of the threshold
  behaves as the number says. Without the `SOBEL_STEP_GAIN` division this fails by a factor of four,
  which is exactly the sort of silent scaling error a "does it detect?" test cannot see.
- The existing suite passes, with any fixture change justified per §5.

**Every sabotage confirmed with `ran = passed + failed ≥ 1`, against the correct target.** ⚠ Five
sabotages in the previous session passed while proving nothing — run against `--bin` where the
module lives in the lib; a regex where `cargo test` takes a substring; a `sed` that matched nothing;
a malformed script whose write failed; and a named test that did not exist. **After any scripted
edit, assert the new text is present; after any sabotage, confirm the *named* test failed** rather
than that some suite passed.

---

## 7. Deliberately not in scope

- **`MIN_SEPARATION` is unchanged at 0.15.** The sky pair that cleared it at 0.17 may simply cease
  to exist once a real contrast floor removes smooth sky gradients from the vote. ⚠ Changing two
  constants at once means neither can be judged. If a near-parallel pair still gets through
  afterwards, that is the moment to raise it — with evidence.
- **No conditioning test** on the vanishing point. It is closer to the real mathematics than an
  offset distance, and it is a second new mechanism in a change already introducing one. Revisit
  only if the floor proves insufficient.
- **`MAX_LEAN_DEG` stays 30°**, `MIN_LENGTH_FRACTION` stays 0.35, verticals only.

---

## 8. Accepted caveats

1. **The floor is one number for the whole frame.** A photograph with a brilliantly lit facade and a
   building in deep shade gets one threshold for both. Display-encoding narrows that gap
   considerably; it does not close it.
2. **A genuinely low-contrast subject will not be detected** — a white building against an overcast
   sky may fall below any floor that keeps the kitten silent. That is the trade being made
   deliberately, and the manual guides remain underneath it.
3. **The corpus is small.** 15–25 frames means one frame moves precision or recall by several
   points, so the sweep shows a band rather than an optimum.
4. `linear_to_srgb` clamps, so detail above 1.0 is flattened before the Sobel. Irrelevant for edges,
   which need a *difference*, and a clipped region has none.

---

## 9. Done when

The abbey is corrected by one button and the guides it infers sit **on the building**; the kitten,
the poster and the tree stay silent; Walhalla either corrects from the building's own columns or
declines; and the chosen `MIN_EDGE_LEVELS` is justified by the sweep rather than asserted.

⚠⚠ **M7.4's "Done when" is a judgement about a photograph, and §6 of `PARALLEL-WORK.md` still
applies.** Produce the before/after and the numbers; **the user signs it off.**

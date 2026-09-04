# W5 track B — M6.4 soft proofing

*Design, 2026-09-04, settled in W5-0 with the user. **None of these decisions is the implementer's
to re-open.** If you meet a question this spec does not answer, `PARALLEL-WORK.md` §6 applies: stop
and report it rather than choosing.*

---

## 1. What it is, and the gap it closes

Rustroom can export sRGB, Display P3, Adobe RGB and ProPhoto, and **the preview cannot show any of
them but sRGB.** The colour-management work recorded that as its own outstanding caveat, twice:

> **no soft-proofing** — the preview still cannot show colour outside sRGB even when exporting P3,
> because the pipeline clips to sRGB before this step.

So: a **View-menu toggle** that renders the canvas through a chosen output space, **plus a
gamut-warning overlay** marking the pixels that space cannot hold.

---

## 2. ⚠⚠ The seam already exists — this track is mostly wiring

`RenderRequest` already carries `display_space: color::OutputSpace`, and `render_engine::pack`
already converts into it. That was built for the display-profile work. **Soft proofing is that same
field, pointed at the export target instead of the monitor.**

⚠ Read `pack` before writing anything. The display-profile work shipped a real bug here and caught
it by re-reading what consumes the bytes: **the histogram is derived from `pack`'s sRGB
quantisation**, and a transform applied before that point silently makes the histogram
display-referred. `the_histogram_is_unaffected_by_the_display_space` is the guard, and it must stay
green.

---

## 3. Where the controls live

**A View-menu toggle naming an output space.** Soft proofing is a *view mode* — like Before/After
and Visualize Spots — so it belongs where those live.

⚠ **Not the export dialog**, though proofing what you are about to write is the obvious instinct: it
would make a canvas view mode reachable only from a modal, and it would be active exactly when the
canvas is covered. ⚠ **Not the Develop panel**, which is for the photograph; this project already
removed the Presets panel for giving one feature two homes.

⚠ The state lives on **`Session`**, beside `display_space` and `visualize`, not on `RustroomApp` —
`Session::request` is the one funnel every render goes through and has no app access. That is the
reason `display_space` lives there, recorded in M5.3b. Cost, as there: it resets on a photo switch.

---

## 4. The gamut warning

Painted in **`pack`**, following the rule Visualize Spots established:

> ⚠⚠ **A view mode applied in `pack`, AFTER the histogram and BEFORE the display transform — never
> a pipeline stage.**

⚠⚠ **Out-of-gamut is EXACT, not a threshold.** A pixel is out of gamut when any channel falls below
0 or above 1 *after conversion into the target space* and before clipping. There is no tuning
constant here, and inventing one would be inventing a judgement.

⚠ The warning colour must be **fixed, not the theme accent**: the accent is user-switchable
including a `Neutral` setting whose whole point is to have no colour, which would defeat the
warning exactly when someone had chosen it. The crop overlay's own comment records this reasoning
and its fixed red.

⚠ The overlay is a **separate toggle** from the proof itself. Proofing without the warning is the
common case — you want to see the picture — and a warning you cannot switch off is chrome.

---

## 5. Shape of the code

| unit | home |
| --- | --- |
| the out-of-gamut test | `crates/color` — pure, beside `OutputSpace` |
| the overlay paint | `crates/app/src/render_engine.rs`, in `pack` |
| view state | `Session` (`soft_proof: Option<OutputSpace>`, `gamut_warning: bool`) |
| the menu entries | `commands.rs` + `menu.rs` |

⚠⚠ **This track writes NO `Edits` field and touches NO pipeline stage.** A proof is a property of
the *view*, exactly like `display_space` — it must not reach the sidecar, must not travel with a
paste or a preset, and must not appear in any golden. That is what keeps it disjoint from tracks A
and C, and it is the reason this track is in this wave at all.

⚠ Adding menu entries touches `Command`, `menu.rs` and the `DialogSize` array — `PARALLEL-WORK.md`
lists those as contended by "any new dialog or shortcut". **No other track in W5 adds a menu entry**,
so this track owns them. Append, never insert.

---

## 6. Testing

- **The histogram is unaffected**, proof on or off. ⚠ The existing test must stay green *and* be
  extended to the proof space — it currently covers `display_space` only, and a proof applied at the
  wrong point in `pack` would leave it passing while the histogram silently described the proofed
  image.
- **A colour inside the target gamut is not flagged; one outside is.** Both halves — a warning that
  flags everything and one that flags nothing each pass a one-sided test.
- **⚠ The discriminating fixture is a colour that is in gamut for one space and out for another** —
  a saturated green is inside ProPhoto and outside sRGB. A fixture flagged in every space cannot
  tell a working test from one that ignores the space argument entirely.
- **Proof off is bit-exactly the current behaviour**, so nothing about the ordinary path moved.
- **The warning colour is fixed**, asserted, so it cannot drift onto the accent.

---

## 7. Accepted caveats

1. **Soft proofing is per session and resets on a photo switch**, like `display_space` and
   `visualize` before it.
2. **⚠ On macOS the OS colour-manages the window**, so `effective_display_space` returns `Srgb`
   whatever the display dropdown says — the check recorded as unrunnable during W4. **Soft proofing
   is unaffected by that**, because it proofs into a space we choose rather than the monitor's; but
   the *difference between proof and no-proof* is the thing to look at, not the absolute colour.
3. **No rendering intent and no gamut mapping** — out-of-gamut colour still clips per channel at
   export, which the colour-management work records as its own outstanding item. This shows you
   where clipping will happen; it does not change what happens.
4. **The overlay marks pixels, not areas** — no smoothing, no minimum size, so a few isolated
   speckles are possible on noisy saturated content.

---

## 8. Done when

Turning the proof on for Display P3 visibly changes a saturated photograph's canvas; turning the
warning on marks the pixels sRGB cannot hold; the histogram is identical in all four combinations;
and with the proof off nothing about the current behaviour has moved.

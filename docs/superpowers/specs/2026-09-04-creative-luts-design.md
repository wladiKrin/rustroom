# W5 track A — M7.5 creative 3-D LUTs

*Design, 2026-09-04, settled in W5-0 with the user. **None of these decisions is the implementer's
to re-open.** If you meet a question this spec does not answer, `PARALLEL-WORK.md` §6 applies: stop
and report it rather than choosing.*

---

## 1. ⚠⚠ Correcting the record: M5.4b did NOT unblock this

`CLAUDE.md` said M7.5 was *"now UNBLOCKED, since M5.4b answered the design question"*. That
overstates it. M5.4b answered **where a display rendering lives** — which supplies the *seam*. The
blocker `docs/BACKLOG.md` actually records is a different one and was still open:

> ⚠⚠ **A `.cube` file does not say what colour space its input is in.** The format has no field for
> it.

Feed a Rec.709 LUT our scene-referred linear ProPhoto and there is no error — just a plausible,
wrong image that reads as a stylistic choice. Same failure class as the inverted ICC tone curve,
which is why `the_profile_agrees_with_our_own_maths` had to cross-check against an independent
implementation rather than round-trip.

⚠ **The parser is the easy half.** `.cube` is `LUT_3D_SIZE N` then N³ RGB triples with red varying
fastest; parser plus trilinear interpolation is ~150 lines. Do not start there.

---

## 2. ⚠⚠ The input space decides the PLACEMENT — this is the core of the design

A professional applies a LUT for one of two quite different reasons, and conflating them is how LUT
support ships broken:

- a **film emulation** (Portra, Velvia) is a *complete rendering* — it expects log or linear-ish
  input and produces the final look;
- a **look or grade** (teal-and-orange, a colourist's grade) is a *modifier* — it expects an
  already-rendered image.

**Stack a film emulation on top of `DisplayRenderingStage` and you double-render** — our camera
S-curve, then the LUT's — giving crushed shadows and a clipped roll-off. That is the single most
common way this feature is reported as "looks wrong".

So the space the user declares at import decides where it runs:

| declared input space | what the LUT is | where it runs |
| --- | --- | --- |
| **sRGB display**, **Rec.709** | a modifier | **after** `DisplayRenderingStage` |
| **Linear** | a rendering | **in place of** `DisplayRenderingStage` |

⚠ **Consequence, and the panel must show it:** selecting a linear LUT means the camera rendering is
not applied. Two renderings apparently both switched on, with one silently ignored, is the
invisible-state failure this project has recorded repeatedly. The Rendering section must say which
is in force.

---

## 3. Only spaces that can be verified ship

`LutSpace { SrgbDisplay, Rec709, Linear }` and no more.

⚠⚠ **Log encodings — LogC, S-Log3, ACEScct — are deliberately absent.** Each is a specific transfer
function needing implementation *and validation*, and there is **no sample footage here to validate
against**. Shipping them unverified is the position colour management C2 was in without its oracle,
and C2's own record says that is why it stayed opt-in. Add a variant when there is something to
check it with.

⚠ `Rec709` and `SrgbDisplay` differ only in transfer curve (Rec.709's is not sRGB's), and both are
implementable and checkable here. Keep them distinct rather than aliasing one to the other: a LUT
author chose one.

---

## 4. Values above 1.0 clip into the cube

The cube is bounded; our data is not. Above 1.0 the table simply has no answer, so the index clamps
to the cube's edge.

⚠⚠ **The `ProfileHueSatMap` trick does not transfer, and assuming it does is the trap.** That table
clamps the *index* while applying the result to the *real* value — which works only because it
stores a **scale**. A `.cube` stores an **absolute output colour**, so above 1.0 there is nothing to
scale.

⚠ Clipping is defensible here specifically because a creative look **is** a display-referred
decision: this is the point in the chain where headroom is legitimately spent. It would not be
defensible anywhere upstream.

---

## 5. Strength, 0–100, default 100

A lerp between the identity and the LUT's output, applied after the lookup.

⚠ Not optional: halving a look is ordinary working practice, and without it the feature is one you
fight. ⚠ Default **100** so that applying a look does what its name says — a lower default means a
LUT never looks like what its author shipped until you find the slider.

---

## 6. Where the code goes

| unit | home |
| --- | --- |
| `.cube` parsing + trilinear lookup | **new crate `lut`**, or `pipeline/src/lut.rs` — implementer's call, but **zero GUI dependency** either way |
| the stage | `pipeline/src/stages/creative_look.rs` |
| placement | `pipeline/src/lib.rs`'s tail, in the existing `match edits.rendering` region |
| the `Edits` field | `edit-model/src/lib.rs` **+ its 5 exhaustive sites** |
| discovery + panel | `crates/app` — the Rendering section of `develop/panel/` |

**The `Edits` shape:**

```rust
/// A creative look: a 3-D LUT applied as a display-referred grade.
pub struct Look {
    /// The `.cube`'s filename, resolved against the looks directory. ⚠ A NAME, not a path —
    /// see the portability caveat in §9.
    pub file: Option<String>,
    pub space: LutSpace,
    /// 0..=100. 100 is the LUT as authored.
    pub strength: f32,
}
```

⚠ It rides the **`Rendering` group** (`EDIT_GROUP_NAMES` index 9), which already exists — a look is
a rendering decision, not a Color adjustment. ⚠ Append the field at the **end** of `Edits` and the
end of every exhaustive fixture: `PARALLEL-WORK.md` rule 3.

⚠ `Look::default()` must be **hand-written**, not derived — `strength` is 100 at neutral, and the
container-level `#[serde(default)]` fills a missing field from the struct's `Default`, so a derived
one gives every partial sidecar `strength: 0`. `RadialGradient::default` records this trap twice.

---

## 7. ⚠⚠ Measure and report BEFORE fixing any conversion constant

Per the user's W5-0 call, this track has a **planned checkpoint**.

Nobody in this project has yet put a real `.cube` through this pipeline. So: build the parser, the
shaper and the stage, then **run real LUTs through each candidate space and report what they do —
and stop.** Do not tune a transfer constant, and do not decide that a conversion "looks right".

⚠ That is W1's M7.3a shape, and its point is that a stall you planned is cheaper than one you did
not. `PARALLEL-WORK.md` §6: an agent given an open design choice *does not stall — it picks one
silently and the choice ships.*

---

## 8. Testing

**The test the backlog names, and the one that catches a wrong shaper:**

> ⚠⚠ **An identity LUT must be BIT-EXACTLY the identity.** `assert_eq!`, no tolerance. A shaper that
> converts into the wrong space and back is *nearly* the identity, and a tolerance hides exactly
> that.

Also:

- **A linear LUT replaces the rendering; a display LUT follows it** — assert on the *stage list*, not
  on pixels, or the two placements are indistinguishable whenever a LUT happens to be gentle.
- **Strength 0 is bit-exactly the input** and strength 100 is bit-exactly the LUT's output.
- **A value above 1.0 clamps to the cube edge** rather than wrapping, extrapolating or producing NaN.
- **A malformed `.cube` is refused with an error**, never silently treated as the identity — a look
  that does nothing is indistinguishable from a look that is not applied.
- **Goldens: append, never insert.** ⚠ Track C also adds goldens this wave.
- **`Edits::default()` stays strictly neutral**, so all 53 existing goldens must be byte-identical.

⚠ **Every sabotage confirmed with the edit verified present, a `test result:` line seen, and the
NAMED test failing** — this project logged nine sabotages that passed while proving nothing in the
two sessions before this one, three of them the fixture's fault rather than the guard's.

---

## 9. Accepted caveats

1. **A look is global and unmaskable.** `MaskStage` runs earlier in the tail, and putting a
   display-referred operation inside its per-pixel loop breaks the property that makes that loop
   correct and cheap. Lightroom's profiles are not maskable either.
2. **⚠ A preset carrying a look references a file by name**, so it is portable only where the
   `.cube` is present. Camera profiles already behave this way. A missing file must render **without
   the look** and say so — never silently, and never as an error that stops the photo opening.
3. **No log encodings** (§3).
4. **Clipping above 1.0** (§4).
5. **Every slider still works "under" the look**, since the look is at the end of the tail. That is
   not new — the camera rendering already sits there and was confirmed by use in M5.4b.

---

## 10. Done when

A `.cube` dropped into the looks directory can be chosen, shows in the canvas and the filmstrip,
survives a paste and a preset, and can be dialled back with Strength. An identity LUT is bit-exactly
the identity. **And the space/placement report of §7 has been handed to the user, who signs off the
convention before any constant is fixed.**

---

## ⚠⚠ Task 6 checkpoint — the result, measured on REAL `.cube` files (2026-09-04)

Track A stopped here as planned and reported that **no third-party `.cube` existed on this machine**,
so its Task 6 table used synthetic stand-ins. The user authorised downloading real ones. Four were
fetched from [`aras-p/smol-cube`](https://github.com/aras-p/smol-cube) (MIT/Unlicense repo; the LUTs
themselves ship with DaVinci Resolve and Tiny Glade) and run through A's own `look_probe` on two real
X-T4 frames. **The checkpoint's premise was right: real files behave in ways no synthetic table could
have shown.**

### ✅ What the real files CONFIRM

- **The placement rule is exact.** An identity cube declared at **sRGB display** or **Rec.709**
  reproduces the *camera rendering* row **to the digit** (0.4514 / 0.06% / 0.2182), and declared at
  **Linear** reproduces the *colorimetric* row to the digit (0.3377 / 0.00% / 0.1414). A linear look
  really does run **in place of** the rendering, and a display-referred one really does modify it.
- **The shaper round trip is invisible.** `identity round trip worst **0.000** of an 8-bit level,
  **0.000%** of pixels over one level`, both spaces, both frames. A's `< 0.001` claim is confirmed on
  content it never saw. ⚠ This is the number A's tolerance-vs-`assert_eq!` decision rests on, and it
  holds.
- **A wrong Linear declaration is catastrophic, and visibly so.** A colourist's grade declared Linear
  clips **61.08%** (Cold Ice) and **91.91% → 100.00%** (Sam Kolder) of the frame into the shadows,
  the latter reaching saturation **1.0000**. Nobody would ship that by accident.

### ⛔ What the real files BREAK, and none of it was reachable synthetically

1. **⚠⚠ A REAL 65³ LUT IS REFUSED OUTRIGHT.** `resolve-LMT ACES v0.1.1.cube` fails with
   `LUT_3D_SIZE 65 is not usable (need 2..=64)`. `Cube::MAX_SIZE` is **64**, justified in its own
   comment as *"beyond that a file is more likely [corrupt]"* — **and 65 is one of the two standard
   sizes**, Resolve's high-quality default (65³ = 274 625). The cap excludes a large fraction of real
   looks on reasoning that is simply wrong. **Raise it.**
2. **⚠⚠ THE PARSER HAS NO NOTION OF A 1D INPUT SHAPER, AND THE HAZARD IS STILL LATENT.** That same
   ACES file is a *shaper* LUT:
   ```
   LUT_1D_SIZE 4095
   LUT_1D_INPUT_RANGE 0.0000019180 15.7761012587
   LUT_3D_SIZE 65
   LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353
   ```
   `grep -c LUT_1D lut.rs` is **0**, and the parser deliberately *skips* keywords it does not know —
   right for `TITLE`/`DOMAIN_*`, wrong here, because the **4 095 shaper values sit ahead of the
   274 625 cube values in the data stream**. It was refused on *size* before reaching them, so this
   has never actually been exercised. ⚠ **Raising `MAX_SIZE` without teaching the parser about
   `LUT_1D_SIZE` and `LUT_3D_INPUT_RANGE` would turn a loud refusal into a silent mis-parse** — the
   worse failure, and the one this project's record warns about repeatedly.
3. **⚠⚠ THE ARCHETYPAL FILM EMULATION DECLARES AN INPUT SPACE WE RULED OUT.**
   `resolve-DCI-P3 Kodak 2383 D65.cube` — the exact shape the placement rule was designed around —
   says in its own header: `Input: **Cineon Log**`, `Display: **DCI-P3, Gamma 2.6**`. W5-0 excluded
   log encodings deliberately ("there is no footage to validate them against"), so **all three of our
   spaces are wrong for it**, and DCI-P3 output is not modelled either.
4. **⚠⚠ AND THAT WRONG DECLARATION DOES NOT LOOK WRONG — WHICH IS THE REAL FINDING.** Run at each of
   our three spaces the Kodak LUT gives mean L 0.4697 / 0.4496 / 0.2757 with **0.00% clipping in all
   three**. Every one is a plausible photograph. **The Linear catastrophe above is what a wrong
   declaration looks like for a *grade*; for a *film emulation* there is no such signal at all**, so
   nothing in the image tells the user they have chosen wrongly. That argues the space control must be
   explicit and prominent rather than defaulted — it cannot be discovered by eye.
5. **One real look declares nothing whatsoever.** `tinyglade-Cold_Ice.cube` is a bare
   `LUT_3D_SIZE 16` with no header at all — `BACKLOG.md`'s blocker in its purest form, and the
   evidence that the space genuinely must be a user choice.

### ⚠ A defect in the probe itself, found by using it

`look_probe`'s identity diagnostic renders **`cubes[0]`**, so with `--cube` supplied it silently
measures *the first file given* while still printing "identity round trip". Handed the Kodak LUT
first it reported `worst 55.972 of an 8-bit level, 100.000% of pixels` — which reads as a
catastrophic shaper bug and is nothing of the kind. ⚠ It also labels rows with the path truncated to
30 characters, so four files under one directory print as four identical labels. **Pass a real
identity cube first, and keep the files' basenames distinct**, until both are fixed.

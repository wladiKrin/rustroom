# Colour grading (M3.3) — design

**Date:** 2026-08-25
**Milestone:** Phase 3, M3.3 — the last of the three-way colour tools.
**Scope:** shadows / midtones / highlights colour wheels, plus a Global wheel and the Blending
and Balance sliders that reshape the tonal ranges.

## Why now

M3.3 was parked with a recorded prerequisite: *"colour grading has a hidden prerequisite: real
colour management does not exist — lcms2 emits one static sRGB profile and nothing else."*
That prerequisite was satisfied on 2026-08-25. Grading now happens in a wide working space
(linear ProPhoto), is judged on a colour-corrected preview, and exports to a chosen profile.

No new dependency. The 39-golden harness already covers the seam this lands in.

## What is being built

The full Lightroom-shaped panel, not a subset:

| control | range | default |
|---|---|---|
| Shadows / Midtones / Highlights / Global — Hue | 0–360° | 0 |
| … Saturation | 0–100 | 0 |
| … Luminance | −100–100 | 0 |
| Blending | 0–100 | **50** |
| Balance | −100–100 | 0 |

Split toning is *not* built separately: midtones at zero saturation **is** split toning, so
building it first would mean building this twice.

## The model

### 1. A wheel is an additive offset in Oklab

Per pixel: ProPhoto → Oklab, add `(Δa, Δb, ΔL)`, Oklab → ProPhoto, clamp at 0.

Rejected alternatives:

- **Multiplicative tint in linear ProPhoto** (`rgb *= mix(white, tint, w)`) — cheaper, no round
  trip, physically what a coloured filter does. Rejected because it **cannot tint black**
  (`0 × anything = 0`), so shadow toning fades out exactly where it is most wanted, and because
  the hue wheel would be perceptually non-uniform — the yellow-green arc compresses badly.
  On a widget that *is* the hue circle, non-uniform angles are a lie the user can see.
- **Lift / gamma / gain** (DaVinci's three-way corrector) — elegant, and the ranges emerge from
  the maths rather than from explicit weights. Rejected because it is **incompatible with the
  chosen scope**: Blending and Balance are properties of explicit weight curves, and there is
  nothing for them to reshape in a lift/gamma/gain model.

⚠ **The grading hue and the HSL band hues are the same angle in the same space.** `BAND_HUES`
in `stages/hsl.rs` are Oklab hue angles; so is `GradeWheel::hue`. "Red" therefore means the same
thing in both parts of the Color section, which it would not if one were an sRGB hue.

Hue is measured from the **+a axis, counterclockwise** — i.e. exactly `atan2(b, a)`, with no
offset to get wrong. The widget draws 0° at the right.

### 2. The ranges are selected on Oklab *L*

⚠ **This corrects the note in CLAUDE.md**, which assumed `color::luminance()` (physical *Y*)
would be the selector. Two reasons it must not be:

- We are already in Oklab for the tint, so *L* costs nothing extra.
- Mid-grey sits near 0.6 in *L* but at 0.18 in *Y*. Selecting on *Y* would put the "midtones"
  range where most people would say highlights.

### 3. Weights are three normalised Gaussians

```
sigma  = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN)^(blending / 100)
shift  = -(balance / 100) * BALANCE_SHIFT
c      = [0.0 + shift, 0.5 + shift, 1.0 + shift]
e_i    = -((L - c_i) / sigma)^2
w_i    = exp(e_i - max(e)) / Σ exp(e_j - max(e))
```

Starting constants: `SIGMA_MIN = 0.08`, `SIGMA_MAX = 3.0`, `BALANCE_SHIFT = 0.25`. These are
*tuning* values and are expected to move once the thing is used on real photographs — the
vignette's amount model took three revisions for exactly this reason, every one of them from
looking at a picture rather than from the spec.

Three properties fall out of this shape rather than being bolted on:

- **The weights sum to exactly 1 at every lightness.** So setting all three wheels the same
  reproduces that tint uniformly, with no seams — which is also why **Global is a plain fourth
  constant added after the weighted sum**, not a fourth range.
- **Blending's limits are exactly right by construction.** As `sigma → SIGMA_MIN` the nearest
  centre wins outright; as `sigma → SIGMA_MAX` all three approach ⅓ and the grade becomes a
  uniform tint.
- **Balance slides all three centres together**, so positive Balance lets the highlight tint
  reach further down without deforming the curves.

⚠ **The `max(e)` subtraction is the standard softmax stabilisation and is load-bearing**, not
decoration: without it, an `L` far from every centre at small `sigma` underflows all three
exponentials to zero and the normalisation divides by zero. With it, the largest raw weight is
always exactly 1, so the sum is always ≥ 1.

### 4. The whole grade is a 1-D LUT on *L*

The tint at a pixel depends on nothing but its lightness, and the wheel values are constants for
a render. So `ColorGradingStage::new` precomputes a **256-entry table of `(Δa, Δb, ΔL)`**,
linearly interpolated at lookup.

This is not a micro-optimisation: direct evaluation is three `exp` per pixel, about 5 M `exp`
on a 1600×1068 preview frame.

It also settles the *"one stage or three?"* question recorded in CLAUDE.md **definitively: one
stage.** Only one stage can fold three ranges into one table, and three sequential stages would
mean three Oklab round trips with each range re-measuring lightness the previous one had already
moved — the interaction `LocalContrastStage` deliberately avoids by summing deltas against one
source.

### 5. Magnitudes

```
Δa = SAT_MAX * Σ w_i * (sat_i / 100) * cos(hue_i)   + global term
Δb = SAT_MAX * Σ w_i * (sat_i / 100) * sin(hue_i)   + global term
ΔL = LUM_MAX * Σ w_i * (lum_i / 100)                + global term
```

Starting constants: `SAT_MAX = 0.15`, `LUM_MAX = 0.15`. For scale, a vivid sRGB red has an
Oklab chroma near 0.26.

### 6. Clamping

⚠ **The LUT index clamps to [0, 1]; the pixel's own `L` does not.** Scene-referred values above
white have `L > 1`, and clamping the *value* rather than the *index* would drag every highlight
back to white — destroying exactly the headroom the wide working space exists to preserve.

This is the identical trap `ProfileHueSatMap`'s `v` has, recorded in CLAUDE.md as *"the table
index clamps at the top while the scale applies to the real value"*. It gets its own test here
for the same reason.

Output is clamped at **0 only**, matching `SaturationStage` and `HslStage`. Never at 1 — the
output transform is the only place clipping belongs.

## Placement

Tail, **after `HslStage`, before the masks loop**:

```
WB → exposure → contrast → tone regions → tone curve → local contrast →
saturation → HSL → COLOUR GRADING → masks → vignette → sharpening → output
```

- **After the tone curve and local contrast is forced.** Ranges are selected on lightness, so
  any later stage that remaps lightness would slide pixels between ranges — the argument that
  already pinned local contrast after the curve and gave the vignette its "+100 is exactly
  white" endpoint.
- **After saturation and HSL** because HSL corrects colours that are *in* the photograph, while
  a grade is a look laid over the top. Reversed, an HSL band tweak would act on the tint just
  added. Lightroom's own order agrees: HSL/Color, then Color Grading.
- **Before masks**, because `build_pipeline_split_in_region_with_brush` already carries the rule
  that a mask "should act on the global look rather than be re-mapped by it", and a grade is
  global look. The alternative — after masks, so grading sees truly final tonality — was
  rejected: a masked +1 EV on a face would then push it into the highlight range and silently
  pick up the highlight tint, a surprising coupling.

Consequences, all favourable and all worth stating because they are what makes this a small
change: it is a per-pixel, position-independent stage, so **no `Region` plumbing, no kernel
radius, no `REGION_MARGIN` change, no `RenderScale`, and no `PrefixStepKey`**. Being a tail
stage, the prefix cache is untouched. Being inert at its default, **all 39 existing goldens must
stay byte-identical** — that is the acceptance criterion for the headless half.

## Data model

```rust
pub struct GradeWheel { pub hue: f32, pub saturation: f32, pub luminance: f32 }

pub struct ColorGrading {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
    pub global: GradeWheel,
    pub blending: f32,
    pub balance: f32,
}
```

⚠ **`ColorGrading::default` is hand-written, not derived.** `blending` defaults to 50, and the
container-level `#[serde(default)]` fills a missing field from the struct's `Default` — so a
derived one would give every partial sidecar `blending: 0`, i.e. hard-edged ranges, silently.
Same trap `RadialGradient::default` records.

`GradeWheel::default` *is* derived: all three fields are neutral at zero.

`ColorGrading::is_active()` is **wheels-only**: true when any of the four wheels has
`saturation != 0 || luminance != 0`. Blending and Balance are modifiers, so dragging one alone
must not insert a stage — the rule `NoiseReduction`, `Vignette` and `Sharpening` already follow.
A hue with zero saturation is inert and must not activate the stage either.

**Edit group: Color (index 2).** No new group, no new panel section, no `EditGroups` change.
`Edits::merge_from`'s exhaustive destructure makes the assignment a compile error until made.

⚠ **Masks do not get colour grading.** `LocalAdjustments` stays per-pixel-only; a grade is a
global look, and `per_pixel_stages` is shared between the global tail and every mask.

## The widget

Chosen from three layouts mocked at the panel's real **240 px** width: a single large wheel with
a range selector, over three small side-by-side wheels and over a stacked wheel-plus-sliders
list. At 240 px, three-up gives 62 px wheels — about 30 px of usable drag radius — which makes
fine hue work fiddly. One wheel gets 188 px.

- A segmented `Shadows │ Midtones │ Highlights │ Global` selector, following the `HslChannel`
  pattern the Color section already teaches.
- Below it the wheel; below that numeric **Hue / Saturation / Luminance** sliders for the
  selected range, so every value is exact as well as draggable.
- Then a hairline, then **Blending** and **Balance**, always visible. (The hairline-inside-a-
  section idiom is the one already used before Texture/Clarity/Dehaze inside Tone.)

⚠ **egui has no colour wheel**, so this is a widget built from scratch with the painter: a
triangle fan with per-vertex colours, centre neutral, rim at constant Oklab chroma converted to
sRGB. Out-of-gamut hues clip at the rim, which honestly shows which hues are reachable.

Dragging inside the disc sets hue from the angle and saturation from the radius, clamped at the
rim. It lives in the **bin** crate with its call site, per the standing binary-crate dead-code
rule: a `pub` item in a bin crate with no caller fails `clippy -D warnings`.

## Testing

**Unit tests** (`pipeline`):

- weights sum to 1 at every lightness, swept across Blending and Balance;
- Blending 0 gives near-hard selection, Blending 100 gives near-uniform ⅓;
- Balance direction — positive lets highlights reach lower;
- LUT agrees with direct evaluation within tolerance;
- all three wheels set alike reproduces exactly that uniform tint (the sum-to-one property);
- Global is additive on top of the weighted sum;
- **a pixel with `L > 1` keeps its headroom** — the index-vs-value clamp;
- neutral params are bit-exactly the identity;
- a grey pixel is tinted (grading is not chroma-gated, unlike HSL's chroma taper).

**Goldens** — a new `grade_ramp_chart()`: a neutral lightness ramp with a muted-colour ramp
beneath it, so the grade can be seen acting on both neutral and chromatic pixels.

⚠ The ramp runs roughly **0.02 to 0.85, not 0 to 1**, so that adding chroma near the top end does
not clip and silently destroy the golden's discriminating power. This is the recorded lesson from
the exposure golden, which pinned 15 of 64 samples at 255 and could not tell one coverage from
another. **Measure the generated golden, do not merely look at it.**

⚠ The chart is authored in **sRGB and converted at construction** via `set_srgb`, per the lesson
from the ProPhoto switch: literals in a chart mean something, and read as ProPhoto they become
far more saturated colours than any camera produces.

Four cases, one per axis: `grade_split_tone` (hue/sat), `grade_luminance`, `grade_blending_wide`,
`grade_balance_high`. 39 → **43 goldens**.

⚠ **Every new test gets the sabotage step, with a compile check first** — a sabotage that fails
to compile looks exactly like one that passes.

**Validation on a real photograph** — a hand-written sidecar through `rustroom render`, on the
X-T4 `.RAF`, measured rather than squinted at: probe the graded output at several lightnesses and
confirm the tint direction and magnitude match the wheel settings.

## Plans

Two plans, one spec, following the M3.1a precedent:

1. **Headless** — `edit-model` types and groups, the stage, `build_pipeline` wiring, unit tests,
   goldens, CLI validation. Provable end to end with no GUI in existence, which means a panel bug
   can never be confused with a model bug.
2. **The widget** — the `color_wheel` painter widget, the range selector, the panel block.

## Accepted caveats

1. **Masks carry no grading** — global look only.
2. **Cost**: an Oklab round trip in the tail, next to HSL's, so two round trips when both are
   active. Fusing them would break the `PipelineStage` seam that exists for the GPU port.
   Measure with `bench_preview` and record the number rather than pre-optimising.
3. **No visual feedback of the range boundaries** on the canvas or histogram. Blending and
   Balance are judged by their effect, as in Lightroom.
4. **The tuning constants are starting values**, not settled ones.
5. **Out-of-gamut hues clip at the wheel's rim.** Deliberate and informative rather than hidden
   behind a gamut-mapped rim that would misrepresent what the slider can reach.
6. **Grading is not chroma-gated.** `HslStage` fades out on near-neutral pixels because a
   near-neutral pixel's *hue* is sensor noise; a grade *assigns* a hue rather than reading one,
   so there is nothing to be noisy. Tinting neutrals is the entire point of shadow toning.

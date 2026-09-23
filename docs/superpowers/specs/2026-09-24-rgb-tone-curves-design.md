# Per-channel RGB tone curves — design

**Date:** 2026-09-24
**Branch:** `rgb-tone-curves`
**Status:** approved — deferred twice, then sequenced after the Color-section work, which has landed

## Why, and the correction that made the case

Asked in brainstorming whether other editors have these. ⚠⚠ **They all do, and I had the history
backwards.** They are in **Photoshop** (Curves, since v1), **Lightroom Classic** and **Camera Raw**
(point curve with a channel selector), **Capture One** (Luma/RGB/R/G/B tabs), **darktable** (`rgb
curve`), **RawTherapee** (`RGB Curves`), **Affinity Photo** and **GIMP**.

I had framed them as "a fourth way to do colour" that would worsen the Color section's confusion. In
fact they are the **original** grading tool; the shadows/midtones/highlights **wheels are the friendlier
interface built over them.** Rustroom had the approachable version and not the classic one — the reverse
of how most photographers learned.

They also do something the wheels cannot: set an **exact crossover point and shape** — lift only the
deepest shadows in blue, with a precise knee — where the wheels apply smooth weighted ranges steered by
Blending and Balance.

## Why it costs almost nothing in panel complexity

⚠ The tone curve lives in **Tone**, not Color. A channel selector on the existing widget, defaulting to
the composite, adds **no row and no section**: someone who never touches it sees exactly today's widget.
And `ADVANCED COLOR` (2026-09-24) already shrank the visible colour surface.

## What is already true, and is not being changed

⚠⚠ **`ToneCurveStage::apply` is `image.map_samples(|s| self.sample(s))` — it already applies its LUT to
R, G and B independently.** So the "composite" curve is *already* a per-channel operation with one
shared curve, and this is a generalisation rather than a colour-science change:

- **The space is unchanged.** The curve runs where it runs today — linear, scene-referred, after
  exposure and the tone regions. ⚠ A curve in linear light behaves more aggressively than in a
  gamma-encoded one, which is **already true of the composite curve** and is what the widget plots.
  Changing that is a separate question and is not being answered here.
- **The clamp is unchanged.** `sample` clamps its input to `[0, 1]` before indexing, so a scene-referred
  highlight above 1.0 is pulled to the curve's top entry. ⚠⚠ The GPU shader's doc calls this out as
  deliberate parity, *"the opposite of the rule `ProfileHueSatMap`'s `v` follows"*. Per-channel inherits
  it. A shader or CPU path that "fixed" it would be a different stage wearing the same name.

## The model

`Edits` gains three fields beside `tone_curve`:

```rust
pub tone_curve_red: ToneCurve,
pub tone_curve_green: ToneCurve,
pub tone_curve_blue: ToneCurve,
```

- **Empty points = identity**, the rule the composite already follows and what keeps the stage out of
  the pipeline. ⚠ `build_pipeline` currently tests `!edits.tone_curve.points.is_empty()`; it must now
  include the stage when **any of the four** is non-empty, and there are **two** such sites
  (`lib.rs:1459` global, and `lib.rs:1158` for a mask — see scope below).
- **No `version` bump.** `Edits` carries a container-level `#[serde(default)]` and `ToneCurve::default()`
  is empty, so a sidecar written before today loads with three identity curves. ⚠ `version` is
  documented as *"for future migrations"* and nothing reads it. **A test loading a sidecar without the
  fields is the real guard**, not a number.
- **They join the `Tone` edit group**, where `tone_curve` already is. ⚠ That means `merge_from`'s
  destructure **and the three exhaustive fixtures** — the enumerating-guard sites this project is
  repeatedly bitten by.

## Scope: the global curve only, not masks

⚠ `LocalAdjustments` has its own `tone_curve` (dodge and burn with a curve). It does **not** gain
per-channel curves: Lightroom's masks have no curves at all, and a mask that wants a colour shift has
its own grading wheel already. Stated so the asymmetry is a decision rather than an oversight.

## Order: composite first, then per-channel

For each channel *c*: `out = channel_lut[c](composite_lut(v))`.

⚠ **A choice, not a standard I am able to verify.** The reasoning: the composite is the overall
*contrast* shaper and the channel curves are the *tint*, so "shape the tone, then tint it" keeps the
channels' effect predictable. The other order re-maps a tint you just set through the composite's black
and white points, so a lifted blue shadow gets pulled back by the composite. ⚠⚠ Whatever is chosen, the
CPU and the GPU must agree, and the equivalence harness is what proves it.

## The GPU twin

⚠ The shader already calls `curve()` three times, once per channel, against one LUT. It becomes **one
concatenated 4096-entry buffer** — composite, red, green, blue — with the channel's base offset added
to the index, keeping a single storage binding and the existing per-pixel dispatch.

⚠⚠ **`gpu-pipeline` uploads the CPU's table rather than rebuilding one** — the rule `ToneCurveStage::lut`
already states. Four tables, still uploaded from the CPU stage, so the two cannot diverge.

## The widget

A 4-way segmented selector above the curve — **RGB · Red · Green · Blue** — the `hsl_bands` idiom, one
layout learned once. Defaults to RGB.

- The curve and its handles draw in the channel's colour; RGB stays neutral.
- ⚠ The ghost shows the **selected channel's** histogram, which costs nothing new:
  `histogram::normalised(hist)[c]` for a channel and `envelope` for RGB already exist and already share
  one scale.
- ⚠ The selector is **view state, not an edit** — `visualize`'s rule. It belongs beside
  `HslChannel` on the session, never in `Edits`, or switching channels would push history.

## Testing

- ⚠⚠ **A golden for a per-channel curve.** The 74 existing goldens must be **unmoved** (defaults are
  identity), and one new golden exercises a red curve so the feature has pixel evidence rather than
  only unit tests.
- The stage is **skipped** when all four are empty, and **present** when only a channel curve is set —
  ⚠ the second half is the regression the `is_empty` check invites.
- CPU/GPU **equivalence** with per-channel curves set, through the existing harness.
- Order: composite-then-channel is asserted on a fixture where the two orders differ measurably.
- A sidecar written **without** the three fields loads with identity curves and renders identically.
- `merge_from` carries the three with `Tone`, and **not** with any other group; the exhaustive fixtures
  include them. ⚠ `all_nonneutral` must set them, or every "every group transfers" test passes while
  ignoring them.
- The selector pushes **no history entry**.

⚠ What no test reaches: whether the curve *feels* right in linear light at per-channel extremes. Seen in
use, or not at all.

## Deliberately not done

⛔ Per-channel curves on masks. ⛔ A parametric (region) curve. ⛔ Moving the curve to a gamma-encoded
space — a real question, but a separate one, and it would move every existing golden.

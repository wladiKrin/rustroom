# The histogram, the tone curve's ghost, and the panel's width — design

**Date:** 2026-09-23
**Branch:** `histogram-and-panel`
**Status:** approved; two questions answered by the user (scaling, per-channel curves)

## What is wrong, and the one cause behind most of it

Four reports, three of them sharing a root cause.

⚠⚠ **The scale is the single tallest bin across all three channels, taken linearly.** Both draw
sites compute it themselves, identically:

```rust
let max = hist.iter().flat_map(|c| c.iter()).copied().max().unwrap_or(1).max(1) as f32;
```

A large flat area — a sky, a wall, a studio background — puts one enormous count in one bin, and
everything else is squashed to near the floor. That is the *"large peaks drown out the whole view"*
report, and it is also the *"very static"* report: when most of the shape is pinned near zero, an
edit that redistributes midtones barely moves anything visible. **One cause, two symptoms.**

The remaining two are independent and small: the ghost behind the tone curve is drawn at
`linear_multiply(0.15)`, and the curve widget is left-aligned in a panel wider than itself.

## The scaling: square root, with the clipped edges excluded

**The user's choice**, from four offered. Heights become `sqrt(count)`, and **bins 0 and 255 are
excluded from the scale**.

- **Square root** rather than log: peaks stay clearly tall and the tails become readable, without
  log's habit of flattering almost-empty bins — sensor noise in an empty shadow should not read as
  tonal content. ⚠ Chosen over a percentile clip, which keeps true proportions but shows spikes as
  visibly cut off.
- **Excluding bins 0 and 255 from the max** is the targeted half of the fix. The most common cause of
  a scale-wrecking spike is a blown sky or a crushed black, and those are *edges*, not tonal content.
  ⚠ They are still **drawn**, at full height, clamped by `area_mesh`; and the clipping triangles
  already report them, so nothing is hidden by leaving them out of the scale.

⚠⚠ **The degenerate case has to be handled, not assumed away.** A pure black-and-white frame — or any
frame whose only content is at the two edges — has a non-edge maximum of **zero**, and dividing by it
would produce `NaN` heights. It falls back to the overall maximum, and a test pins that.

### Where it lives

A new pair of functions in `crates/app/src/histogram.rs` — **zero GUI, unit-testable without a
window** — replacing the duplicated `max` computation at both draw sites:

```rust
/// Normalised bar heights per channel: sqrt-scaled, the tallest non-edge bin filling the well.
pub fn normalised(hist: &[[u32; 256]; 3]) -> [[f32; 256]; 3];
/// The tone curve's ghost: the per-bin maximum across channels, scaled the same way.
pub fn envelope(hist: &[[u32; 256]; 3]) -> [f32; 256];
```

⚠ One scaling function, two callers, so the panel's histogram and the curve's ghost **cannot drift
apart** — which they could today, each computing its own max. `area_mesh` already takes
`&[f32; 256]` and clamps, so neither draw site changes shape.

## The other three

- **The ghost's opacity**: `linear_multiply(0.15)` → `0.35`. ⚠ A number to tune in use, not a
  derived value; it is stated as a named constant so it is one place to change.
- **The curve widget is centred**, using the **same `pad` inside a `horizontal`** the colour wheel
  already uses (`color_wheel.rs`, "centred against the strip", user's call 2026-09-02). ⚠ The pad is
  allocated beside the widget rather than by widening its rect, so the click target stays tight to
  the curve — a full-width response would let a drag grab a point from anywhere on the row.
- **The panel's default width**: `270.0` → `320.0`. ⚠ `MAX_SIZE` for the curve **stays 220**: the
  request was for the curve to be centred *like the wheel*, and the wheel is a fixed size with pads
  either side. Growing the curve to fill the panel would make the centring a no-op. ⚠ `MAX_SIZE`'s
  own doc says "The panel is 240 pt", stale since 2026-09-21; corrected.

## Deferred by decision, not overlooked

⛔ **Per-channel RGB tone curves.** The user asked whether they make sense: they do, and they are
standard — they place a colour crossover at an exact tonal position, which the grading wheels cannot.
But `ToneCurve` is one `Vec<[f32; 2]>` today, so it means four curves, a pipeline and golden change,
a sidecar version bump with migration, and a channel selector in the panel. **Its own spec**, after
these.

## Testing

In `histogram.rs`, with no window:
- A single enormous spike beside small counts: the small counts must be **visible** — above a stated
  fraction of the well — where linear scaling put them near zero. ⚠ This is the report, as a test.
- A spike in **bin 255** must not set the scale: midtone bars stay tall. Same for bin 0.
- Bins 0 and 255 are still drawn, clamped to 1.0 rather than exceeding it.
- ⚠⚠ A frame with content **only** at bins 0 and 255 must produce finite heights, not `NaN`.
- An all-zero frame produces all-zero heights and no division by zero.
- `envelope` takes the per-bin maximum across channels, so a saturated colour is not hidden.
- Monotonicity: a bin with more samples than another is never drawn shorter.

⚠ What no test reaches: whether the result *reads* better on real photographs. That is the point of
the change, so it is confirmed in use or not at all.

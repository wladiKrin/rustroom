# A display profile for the preview

Status: **design agreed 2026-08-24**, not yet implemented.

This is **piece D** of colour management, and the last one that affects what you see while editing.
A ([the wide working space](2026-08-24-wide-working-space-design.md)) and
B ([the output profile choice](2026-08-24-output-profile-design.md)) are done; C, camera input
profiles, is independent of this.

## The bug, measured rather than inferred

⚠ **This was verified before being specced, because an earlier colour finding in this project was
written into a spec on reasoning alone and turned out to be wrong.**

**wgpu never sets `CAMetalLayer.colorspace`** — `wgpu-hal/src/metal/surface.rs` touches only
`wantsExtendedDynamicRangeContent`. Apple's own SDK header says what that means:

> The colorspace of the rendered frames. **If nil, no colormatching occurs.**

So Rustroom's sRGB pixels are driven into the panel's native primaries with no conversion. This
machine's display profile, parsed from `/Library/ColorSync/Profiles/Displays/Color LCD-*.icc`:

| primary | this display | sRGB | Display P3 |
|---|---|---|---|
| red | x **0.6820** y 0.3194 | 0.6400 / 0.3300 | 0.6800 / 0.3200 |
| green | x **0.2845** y 0.6746 | 0.3000 / 0.6000 | 0.2650 / 0.6900 |
| blue | x **0.1559** y 0.0661 | 0.1500 / 0.0600 | 0.1500 / 0.0600 |

Essentially a P3 panel. Concretely: **sRGB red should be driven at 0.897 on this display and is
being driven at 1.0.** The preview is more saturated than the file it will export.

## ⚠⚠ The transform is a post-step, NOT a pipeline stage

The obvious implementation — reuse piece B's seam and render the canvas with
`build_pipeline_to(.., display_space)` — is wrong, for two independent reasons.

**1. It would silently make the histogram display-referred.** `shell.rs` computes the histogram
from the packed RGBA (`histogram_rgba(&frame.rgba)`), so anything baked into the texture lands in
the histogram too. A histogram that shifts when you change monitors is useless as a clipping
instrument, which is the one job it has.

**2. The bug is oversaturation, not missing gamut.** The preview should match the export. Making
it *also* show colour beyond sRGB is **soft-proofing** — a real feature, and a different one.

So: the pipeline keeps rendering sRGB exactly as it does now, and one conversion is applied on the
way to a texture.

Consequences, all of them good:

- **Nothing in `pipeline` changes** — no new parameter, no golden risk, no cache-key question.
  The prefix cache holds pre-tail checkpoints and never sees this.
- **The histogram stays correct** with no work at all, because the conversion happens after it.
- The default (`Srgb`) is the identity, so it costs nothing for the people it does not help.

## What `color` gains

`OutputSpace::from_srgb(c) -> [f32; 3]`, the sibling of the existing `from_working`, with two more
fused matrices in `spaces.rs`. All three spaces share D65, so no chromatic adaptation is involved
and the matrices are exact.

```rust
pub const SRGB_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [0.8224619687, 0.1775380313, 0.0000000000],
    [0.0331941989, 0.9668058011, 0.0000000000],
    [0.0170826307, 0.0723974407, 0.9105199286],
];

pub const SRGB_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [0.7151256069, 0.2848743931, 0.0000000000],
    [0.0000000000, 1.0000000000, 0.0000000000],
    [0.0000000000, 0.0411619485, 0.9588380515],
];
```

⚠ **`SRGB_TO_ADOBE_RGB`'s middle row really is `[0, 1, 0]`** — it looks like a mistake and is not.
Adobe RGB (1998) shares sRGB's red and blue primaries and differs only in green, and the green
*channel value* is preserved exactly. Cross-checked independently: sRGB green expressed in Adobe
RGB is `(0.2849, 1.0, 0.0412)`, whose luminance through Adobe's own Y coefficients
(`0.2974 / 0.6273 / 0.0753`) is **0.7151** — matching sRGB green's known 0.7152. A test pins this so
nobody "fixes" the row.

⚠ Every value in both matrices is in `[0, 1]` and no output can be negative, because sRGB's gamut
is strictly *inside* both destinations. That is the opposite of `PROPHOTO_TO_SRGB`, which must
produce negatives — so a clamp here is harmless where there it would be destructive.

## Where the conversion is applied

Two places, one shared helper.

1. **The canvas**, inside `render_engine::pack`, on the `f32` image before `to_rgba8()`. Doing it
   in float rather than on the packed bytes matters: an 8-bit → 8-bit conversion compresses the
   range and costs about 0.8 bits, which is visible as banding on smooth skies — precisely the
   thing a photo editor must not introduce into its own preview.
2. **Thumbnails**, in `thumb_loader`, at load. ⚠ **Not optional:** the filmstrip sits directly
   beside the canvas showing the same photo, so converting only the canvas would leave a visibly
   more saturated strip next to a corrected image — a mismatch more noticeable than the original
   error. Thumbnails are 8-bit sRGB with no wider data behind them, so the precision argument
   above does not apply.

Changing the setting re-renders the canvas and reloads thumbnails. ⚠ The reload must be guarded by
a change comparison, because the Settings dialog body runs every frame — the rule
`apply_view_demosaic` already follows.

## The setting

`Settings.display_space: OutputSpace`, `#[serde(default)]` → `Srgb`, chosen from a Settings
dropdown.

⚠ **ProPhoto is excluded from the list.** No such monitor exists and offering it is a pure
footgun, so the dropdown is built from a named subset rather than from `OutputSpace::ALL`. The
subset is defined in `color` beside the enum, so the two cannot drift.

**The egui chrome is deliberately not converted.** There is no "correct" appearance for UI, and
converting it would make Rustroom's greys differ from every other application on screen.

## Testing

- **`Srgb` is bit-exactly the identity** (`assert_eq!`, no tolerance) — the default must cost
  nothing and change nothing.
- **White maps to white** in both destinations, which catches a transposed or mis-scaled matrix.
- **sRGB's gamut is inside both**: no conversion of an in-range colour produces a negative or a
  value above 1.
- **The Adobe RGB green row is what it looks like**, pinned by the luminance cross-check above so
  it survives a future reader's tidy-up.
- **A saturated colour moves, and moves the right way**: sRGB red converted for P3 must come out
  *lower* in red, since a wider container needs smaller coordinates for the same colour. A test
  asserting only "it changed" would pass with the matrix inverted.
- **The histogram is unchanged by the display setting** — the property this whole design exists to
  preserve, and the one that would fail silently if someone later moved the transform into the
  pipeline.

## What this does not do

- **No soft-proofing.** The preview still cannot show colour outside sRGB, even when exporting
  Display P3, because the pipeline clips to sRGB before this step. Its own feature.
- **No calibration.** This corrects the *primaries*, not the display's measured deviation from
  them. That is what the follow-up below buys.
- **No auto-detection.** The setting is manual on every platform.

## ⚠ The named follow-up: let macOS do it

On macOS the exact fix is one property: set `CAMetalLayer.colorspace` to sRGB and Core Animation
colour-matches to the display using its **actual calibrated profile**, including per-device
calibration this setting can never see. `objc2` is already a macOS dependency from the menu-bar
work, so the plumbing exists.

It is deliberately *not* in this spec: it does nothing on Linux, which this project calls its first
platform, and it would leave the thumbnail path unaddressed. When it lands it **supersedes this
setting on macOS**, which should then be hidden rather than left to fight the OS.

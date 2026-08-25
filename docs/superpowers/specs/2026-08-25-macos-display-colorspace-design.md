# Hand the preview's colour to macOS

Status: **design agreed 2026-08-25**, not yet implemented.

The named follow-up to [piece D](2026-08-24-display-profile-design.md). D corrects the preview's
*primaries* from a manual setting; this replaces that on macOS with the operating system's own
colour matching, which knows the display's **calibrated** profile and follows the window between
monitors.

## Why the manual setting is not enough

⚠ **The multi-monitor case is the argument, not calibration.** `Settings.display_space` is one
global value read when a render is requested, and **nothing in the app reacts to a display change**
— verified, there is no monitor or scale-factor handling anywhere. So today:

- Pick Display P3 for a P3 laptop panel, drag the window to an ordinary sRGB external monitor, and
  the canvas is now converted for a gamut that monitor does not have. It renders
  **undersaturated** — wrong in the opposite direction from the bug D fixed.
- Nothing re-renders on the move, so it does not even refresh until you touch a slider.

Closing that within the manual approach means detecting the current monitor, mapping it to its ICC
profile and re-rendering on every move, per platform. That is most of a colour-management
subsystem, and the OS already has one.

Calibration is the second reason: this corrects nominal primaries, never a display's measured
deviation from them.

## The mechanism

⚠ Established while specifying D: **wgpu never sets `CAMetalLayer.colorspace`** — it touches only
`wantsExtendedDynamicRangeContent` — and Apple's SDK header says a nil colorspace means *"no
colormatching occurs"*. Setting it to sRGB tells Core Animation what our pixels are, and it
colour-matches them to *"the colorspace of the context containing this layer (typically the
display's colorspace)"*.

That single property buys, with no further code of ours:

- the display's **calibrated** profile rather than nominal primaries;
- **automatic per-display handling** when the window moves, including a window straddling two
  monitors, because compositing happens per display;
- **the egui chrome colour-managed too**, closing D's caveat that Rustroom's greys differed from
  every other macOS application.

⚠ **Documented model, not measured here.** The multi-monitor behaviour follows from Apple's
compositing model and the header above; it has not been verified on a second display. It is the
first thing to check once built.

### Two facts verified in wgpu-hal 29.0.4

- **`configure()` never touches `colorspace`** — it sets device, pixel format, framebuffer-only,
  EDR, drawable count and drawable size. So the value survives every resize and surface rebuild,
  and it is set **once at startup** rather than re-asserted per frame.
- **EDR is enabled only for `Rgba16Float`.** eframe uses `Bgra8UnormSrgb`, so there is no EDR
  interaction.

### Reaching the layer

`eframe::CreationContext::window_handle()` → `RawWindowHandle::AppKit { ns_view }` → the
`CAMetalLayer`, which is either the view's own layer or a sublayer — wgpu-hal's own occlusion
workaround walks `superlayer()` upward, which implies nesting is possible. The search therefore
checks the view's layer and then its sublayers rather than assuming either shape.

⚠ `egui_wgpu::RenderState` exposes `device`, `queue`, `adapter`, `renderer` and `surface_config`
but **no `wgpu::Surface`**, so `Surface::as_hal` is not reachable through eframe. The view is the
only door.

All the bindings are already vendored at versions matching the `objc2 0.6` the menu bar introduced:
`objc2-quartz-core` (`setColorspace`), `objc2-core-graphics` (`CGColorSpaceCreateWithName`,
`kCGColorSpaceSRGB`), `objc2-app-kit` (`NSView`).

## ⚠⚠ Exactly one mechanism is ever live

Both at once is a **double conversion**: we would convert sRGB→P3 ourselves, then Core Animation
would treat the result as sRGB and convert again, leaving the preview visibly undersaturated — and
varying per monitor. So tagging returns whether it succeeded, and one function decides:

```rust
fn effective_display_space(settings: &Settings, os_managed: bool) -> OutputSpace
```

`os_managed` → `Srgb` (the OS is doing it); otherwise `settings.display_space`.

Two properties fall out, and both matter:

- **Double conversion is unrepresentable**, rather than something the user must avoid — the same
  reasoning that made `FrameSpec::fit` `#[cfg(test)]` after it caused D's inert-setting bug.
- **A failure degrades to today's behaviour, not to nothing.** If the layer cannot be found — an
  unexpected view hierarchy, a future wgpu change — `os_managed` is false and the manual setting
  takes over. Without that, a silent failure would leave macOS *uncorrected*, which is worse than
  what D shipped.

Every site that currently reads `settings.display_space` goes through this function: the render
request, the session's stored copy, and the `ThumbLoader`.

## Thumbnails become more robust, not merely equal

On macOS neither the canvas nor the thumbnails are converted by us, and Core Animation converts the
whole window uniformly. So the filmstrip and the canvas **cannot** disagree — the mismatch D had to
handle deliberately becomes structurally impossible. Off macOS, D's behaviour is unchanged.

## Settings

macOS replaces the dropdown with a line stating the system handles the display profile. Other
platforms keep it exactly as it is.

⚠ The stored value is left alone rather than forced to `Srgb`, so a settings file shared between a
Mac and a Linux machine keeps a sensible choice for the latter.

## Testing

- **`effective_display_space` is pure and gets both branches**, including that `os_managed` wins
  over any value in the settings file — the property that prevents double conversion.
- **The fallback is tested**: `os_managed == false` must yield the manual setting, so a tagging
  failure cannot silently disable correction.
- ⚠ **The ObjC half cannot be unit tested** — it needs a live window and a real Metal layer. This
  is stated as a limitation rather than papered over with a mock that would test nothing. The
  module is kept deliberately thin for that reason.
- **By hand**: the Preview side-by-side comparison; and **the multi-monitor drag**, the case that
  motivated the whole change.

## What this does not do

- **Nothing outside macOS.** Linux and Windows keep the manual setting **and its multi-monitor
  gap**, now recorded as a known limitation rather than an oversight. Wayland's colour-management
  protocol is the eventual answer there.
- **No soft-proofing.** The preview still cannot show colour outside sRGB, because the pipeline
  clips to sRGB before the display step. ⚠ This change makes that *easier* later, not harder: tag
  the layer P3 and render P3.
- **No control over the conversion.** We hand that step to the OS by design; its exact rendering
  intent is Apple's choice, not ours.

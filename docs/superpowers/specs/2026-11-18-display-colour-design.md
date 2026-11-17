# The display's own colours on Windows and Linux (design)

> **Asked for by the author 2026-11-17** ("fix the display saturation level on the Windows and Linux machines — plan,
> spec and execute it yourself"), so the decisions below are the builder's, recorded with their reasons; the verdict
> on the picture stays the author's (`AGENTS.md`). Branch `display-colour`, cut from `v0.1-packaging-windows` (piece
> 3a, waiting on the author), because `main` does not yet build with this machine's static runtime.

## Why

On macOS the window is tagged sRGB and Core Animation matches it to the display's calibrated profile
(`display_colorspace`). On Windows and Linux nothing does: Rustroom's sRGB pixels drive the panel's native primaries
unconverted, so on a wide-gamut monitor the preview is oversaturated. The only remedy today is a manual setting with
three fixed spaces (sRGB, Display P3, Adobe RGB) — none of which is any real monitor's gamut.

## What was measured (2026-11-17, the author's Windows machine)

- **The monitor:** a Gigabyte **M27Q**. Its EDID primaries — R (0.6553, 0.3242), **G (0.2090, 0.7139)**,
  B (0.1465, 0.0508), white D65, gamma 2.2, the "sRGB is the default" flag NOT set. Its green lies far outside sRGB's
  (0.300, 0.600) and beyond Display P3's (0.265, 0.690): sRGB greens are shown much too vivid.
- **Windows assigns it no profile:** `GetICMProfileW` returns the generic `sRGB Color Space Profile.icm`.
- **Windows does not colour-manage it:** `DisplayConfigGetDeviceInfo` reports Advanced Color supported but off,
  `activeColorMode` SDR (Auto Color Management available, not enabled).
- **The display path today:** `pack` converts the float frame into `display_space` on the CPU when it is not sRGB, and
  any non-sRGB display turns OFF the GPU's byte readback and the resident handoff (M8.3g: ~5–7 ms a frame at 3200).
  Thumbnails and rendered previews are converted at load (`convert_thumb`), so no cache is keyed on the display.

## What

1. **A display profile, built with LittleCMS** (`color::display`, no GUI): from an ICC profile's bytes, or from EDID
   primaries + white + gamma (`cmsCreateRGBProfile`); sampled once into a 3-D table from sRGB-encoded values to the
   display's encoded values (relative colorimetric), applied by trilinear lookup. Its size is chosen by measuring it
   against LittleCMS's own transform, not guessed.
2. **`color::Display`** replaces `OutputSpace` on the display path only (session, frame request, `pack`, thumbnails,
   previews): a fixed space (today's three, unchanged) or a profile. `is_srgb()` keeps every fast-path gate exact.
   Exports, proofing and `OutputSpace` itself are untouched.
3. **The EDID parser** (`color::edid`, pure): header and checksum checked, the ten chromaticity bits, gamma, the
   sRGB-default flag, the monitor's name.
4. **Detection, per platform** (`rustroom_app::display_detect`): the platform layer only gathers FACTS — is the OS
   managing colour, an assigned ICC profile, the EDID — and one pure function decides, in this order:
   the OS manages colour → convert nothing (sRGB, as on macOS) · an assigned profile (not the generic sRGB one) → it ·
   an EDID that is not sRGB-flagged and not within 0.005 of sRGB's primaries → its primaries · else sRGB.
   - **Windows:** the monitor the window is on (`MonitorFromWindow` → its device → its EDID in the registry), its
     Advanced Color state (`activeColorMode` WCG or HDR, or HDR on → the OS manages), its assigned profile
     (`GetICMProfileW`).
   - **Linux** (X11 and Wayland): the EDIDs of the connected outputs in `/sys/class/drm`; one monitor, or several
     with the same primaries → used; several that differ → sRGB, and Settings says so (which one holds the window
     is not known without the compositor). No ICC lookup through colord yet.
   - **macOS:** unchanged.
   Re-checked every two seconds on the UI thread (cheap facts; the table is rebuilt only when they change), so moving
   the window to another monitor or switching Windows' colour management on is followed.
5. **Settings → Display** (Windows and Linux): **Automatic** (the new default) · sRGB · Display P3 · Adobe RGB; under
   Automatic a caption names what is in effect ("M27Q's own colours", "Windows manages colour", a profile's name,
   "sRGB — no monitor data"). A settings file that already chose P3 or Adobe RGB by hand keeps that choice.
6. **`RUSTROOM_DEBUG_DISPLAY`** also prints what detection found.

## Known costs

- A profile in effect turns off the two GPU fast paths, exactly as a manual Display P3 does today (~5–7 ms a frame at
  3200), and adds the CPU table pass (measured in the plan). A GPU display transform that keeps them is a follow-up.
- EDID primaries are the manufacturer's nominal values, not a calibration — far closer than sRGB on a wide-gamut
  panel, but a measured profile is better (and wins when assigned).
- ⚠ A monitor set to its own **sRGB mode** in its menu still reports its native primaries: Automatic would then
  desaturate. The Settings hover says to choose sRGB for that.
- ⚠ On Linux, KDE Plasma with a colour profile set colour-manages windows itself — Automatic would convert twice;
  choose sRGB there. Several different monitors on Linux fall back to sRGB.

## Not in this piece

A GPU twin of the display transform; colord profiles on Linux; HDR output; per-monitor manual choices.

## Left to the builder

The table's size and the "close to sRGB" tolerance (by measurement), the module split, the caption wording within
`docs/DESIGN.md`.

# Choosing a camera profile

Status: **design agreed 2026-08-25**, not yet implemented.

**Piece C4**, the last slice of camera input profiles. C1 moved the transform into our code, C2 read
a DCP's matrices, C3 applied its HueSatMap — all reachable only by passing a path to
`DecodeOptions`. C4 is what makes them usable from the application.

## ⚠⚠ We do not ship profiles, and that is a decision not an omission

Adobe's `.dcp` files are Adobe's. **This project cannot make a legal determination about
redistributing them, and will not pretend to.** RawTherapee bundling 162 of them is evidence about
*their* risk assessment, not permission for ours.

The safe answer is also the better engineering one: **discover profiles the user already has**.
No payload in a GPL-3.0 repository, nothing that goes stale when Adobe revises a profile, and no
claim to defend. Dropping a `.dcp` into a directory is the whole installation step.

## Where profiles come from

`<config>/rustroom/profiles/`, beside the `presets/` and `settings.json` that already live there.

⚠ Not `~/.config` on every platform — `dirs::config_dir()` follows each platform's own convention,
which on macOS is `~/Library/Application Support/rustroom/`. The same note `PresetStore` carries.

The directory is scanned on demand rather than watched: profile collections change rarely, and a
watcher is machinery for a problem nobody has.

## Matching a profile to a photo

`RawMetadata.model`, case-insensitively, against both the profile's `ProfileName` and its filename.

⚠ **The make is present in one and absent from the other.** libraw reports model `"X-T4"` while
the profile's `ProfileName` is `"FUJIFILM X-T4"`, so an equality test finds nothing. Matching is
therefore *containment* — the profile's name contains the model — which is loose enough to work and
tight enough not to match a different body.

⚠ **No alias table.** RawTherapee ships `camera_model_aliases.json` because it must match hundreds
of bodies unattended; we can be wrong and let the user pick by hand. A table we cannot test against
real files for every camera is a liability, not an asset. **Every discovered profile stays
selectable regardless of whether it matched** — the match orders and pre-selects, it never filters.

## Per camera, not per photo

The choice is stored in `Settings`, keyed by camera model.

⚠ The profile affects the **decode**, and decoded buffers are cached in `BufferCache` keyed by
path. Per-camera means changing it re-decodes the open photo, exactly as `view_demosaic` already
does, and costs no new machinery. Per-photo would put a *decode* parameter into `Edits` — which
would have to travel through the sidecar, presets, copy/paste, sync and the buffer cache key, all
of which currently describe edits rather than decoding.

⚠⚠ **Per-photo is a recorded follow-up, not a rejected idea.** It is what Lightroom does — the
Calibration panel — and it becomes genuinely wanted the moment someone holds both "Camera Velvia"
and "Adobe Standard" for one body. Since we ship nothing, most collections will have **one profile
per camera**, which is what makes per-camera sufficient *now* rather than correct forever. When it
lands, the buffer cache key and `Edits` are where the work is.

## The UI

A dropdown in the Develop panel's Edit tool, above white balance, listing:

1. **Built-in (libraw)** — always present, always first, and the default.
2. Every discovered profile, those matching the open photo's camera first.

⚠ **"Built-in" is not a fallback label, it is a real choice**: it selects C1's libraw matrix, which
is the well-tested path and the oracle C2 and C3 are measured against. Someone who dislikes a
profile must be able to get back to it.

Changing it re-decodes the open photo and persists immediately, guarded by a change comparison —
the dialog-runs-every-frame rule `apply_view_demosaic` already follows.

## Testing

- **Discovery** against a temp directory, with the store's path injected — the pattern
  `PresetStore`, `SettingsStore` and `ExportPresetStore` all use, so tests never touch the real
  config directory.
- **Matching**: `"X-T4"` finds `"FUJIFILM X-T4"`; `"X-T4"` does **not** find `"FUJIFILM X-T30"`;
  case differences do not matter. ⚠ The X-T30 case is the one that matters — a substring match
  done carelessly makes every model a prefix of some other.
- **A malformed `.dcp` in the directory is skipped, not fatal** — one bad file must not hide the
  rest.
- **The per-camera mapping survives a settings round trip**, and a settings file written before
  this field loads unchanged.
- **By hand**: pick the X-T4 profile, confirm the render changes; pick Built-in, confirm it changes
  back.

## What this does not do

- **No bundled profiles**, for the reason above.
- **No per-photo choice** — the recorded follow-up.
- **No embedded DNG profiles**, no `CameraCalibration`, no `ProfileToneCurve`, no `LookTable` —
  unchanged from C2 and C3.
- **No alias table**, so an unusual model string means picking by hand.

# The camera rendering reaches an export

**Date:** 2026-09-18
**Status:** approved
**Item:** `docs/BACKLOG.md` → *"The camera rendering does not reach an export"*

## What this is

`batch_export::export_one` builds its tail with `pipeline::ProfileRendering::default()`, so an
export is missing everything that struct carries:

| | reaches an export today |
|---|---|
| the profile's **tone curve** | ⛔ no — the built-in curve is used instead |
| the profile's **baseline exposure** | ⛔ no — `BASELINE_EV` is used instead |
| the **creative look** | ⛔ no — ⚠⚠ **not named in the backlog entry; found while scoping** |

⚠ The rendering *mode* does reach an export, because it lives in `Edits`. What does not reach it
is the profile that mode is supposed to consult.

## What it is worth — measured, and the first measurement was worthless

`crates/app/examples/probe_export_rendering.rs`, `DSCF6509.RAF` with the catalogue's X-T4 DCP:

| configuration | worst |
|---|---|
| fixture check: a synthetic curve + 0.5 EV | **156.96 levels** |
| the profile's curve only | **0.00** |
| its baseline exposure only | **0.00** |
| both | **0.00** |

⚠⚠ **The bug is REAL but INERT on this catalogue, and that is the finding.** The X-T4's
`ProfileToneCurve` is exactly `[[0,0],[1,1]]`, which `effective_profile_curve` rejects in favour of
the built-in curve, and the profile declares no `BaselineExposure`. So preview and export already
agree — by coincidence of what that one profile contains. A profile carrying a real curve diverges
by up to **~157 of 255 levels**.

⚠⚠ **The first run of that probe reported 0.00 for every row and was VACUOUS.** The rendering stage
is gated on `edits.applied_rendering()`, which defaults to `Rendering::None`, so a default `Edits`
built no stage at all. **A zero that means "no measurement" is indistinguishable from one that means
"no effect"** — the fixture check is the only reason the difference was seen. Keep it.

**Consequence for landing this:** the fix changes **no exported pixel** for this catalogue, so
nothing needs re-exporting. It closes a gap that would otherwise appear silently the day a profile
with a real curve is configured.

## Design

**1. `ExportSource` carries the profile's owned parts.** `decode_for_export` already receives the
profile path and is where `resolve_lens` runs; it calls the same `profile_rendering` helper the
preview's decode uses. ⚠ `ProfileRendering` *borrows*, so the owned halves must outlive the render
— `ExportSource` is where they live, the shape the lens shot already uses.

**2. `ExportRender` carries `looks_dir`.** The worker resolves `Edits.look.file` through
`looks::LookStore` and caches one slot, mirroring the render worker: *"a photo has one look, and
switching looks is a deliberate act, not a drag."*

⚠ Resolved **in the worker**, not in `export_one`: a batch shares one look, and loading a 786 KB
cube per photo would be per-photo work for a per-batch fact.

**3. `export_one` builds a real `ProfileRendering`.** The default disappears from that call site.

## Testing

| guard | sabotage |
|---|---|
| the profile's curve reaches an exported file | pass `curve: None` — pixels must move |
| its baseline exposure reaches one | pass `baseline_ev: None` |
| the look reaches one | pass `look: None` |
| a `.cube` that is not on this machine exports **without the look**, not without a rendering | make the missing-file path suppress the rendering |

⚠⚠ Every one of these needs a **non-identity** fixture curve. The catalogue's own profile is the
identity, so a test written against it would pass with the fix reverted.

⚠ A JPEG/PNG/TIFF source is already rendered and must **not** be rendered again — `render_cached`
strips this for that reason. The export path must agree.

## Done when

- The three parts reach an exported file, each pinned by a test that fails when it is withheld.
- FMT=0, CLIPPY=0, TEST=0, each from its own command; 74 goldens unmoved.
- ⚠ Exported pixels for **this** catalogue are unchanged — the measurement says they must be.

# Plan — the creative look reaches a thumbnail

Spec: `docs/superpowers/specs/2026-09-18-thumbnail-look.md`
Branch: `export-camera-rendering` (continues; same seam, same day)

## Task 1 — the delegating wrapper

`crates/pipeline/src/lib.rs`: `build_pipeline_rendered(edits, scale, resampler, profile)` beside
`build_pipeline_with`, delegating to the same place `build_pipeline_to` does. ⚠ No existing caller
changes.

## Task 2 — the shared look cache

`crates/app/src/thumb_loader.rs`: a `LookStore` plus `Mutex<HashMap<String, Option<Arc<Cube>>>>`,
captured in the closure `ThumbLoader::new` already builds beside the `ThumbStore`. ⚠ Negative
results cached, or a missing `.cube` is re-read once per thumbnail.

## Task 3 — `render_edits` applies it

Pass `ProfileRendering { curve: None, baseline_ev: None, look }`. ⚠ Curve and baseline stay `None`:
`thumbnail_edits` strips the rendering because the embedded JPEG is already rendered.

## Task 4 — the guards

⚠⚠ The cube fixture must move a **neutral** image — a channel permutation is a no-op on grey, which
cost a round on the export look test today.

| test | sabotage |
|---|---|
| a look changes a rendered thumbnail | `look: None` |
| a missing `.cube` still renders the thumbnail | make the failure propagate |
| a failed load is attempted once | drop the negative caching |
| the camera rendering is still stripped | remove the strip |

## Task 5 — gate and confirm

- FMT / CLIPPY / TEST, each exit code from its own command, before committing. 74 goldens.
- Launch the GUI; a photo with a look must show it in the grid **and** the filmstrip.
- **Do not merge, do not push.**

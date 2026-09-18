# Plan — the camera rendering reaches an export

Spec: `docs/superpowers/specs/2026-09-18-export-camera-rendering.md`
Branch: `export-camera-rendering` (already carries the probe)

## Task 1 — `ExportSource` carries the profile

`crates/app/src/batch_export/render.rs`: add `curve: Option<Vec<[f32; 2]>>` and
`baseline_ev: Option<f32>`.

⚠ Owned, because `ProfileRendering` borrows and the borrow must outlive the render.
⚠ `crates/app/src/decode.rs::decode_for_export` fills them from the same `profile_rendering` helper
the preview's decode uses — one derivation, so the two cannot drift.
⚠ The non-RAW arm leaves them `None`: a JPEG has no camera profile.

## Task 2 — `ExportRender` carries `looks_dir`

Set from settings at `export_flow.rs`, beside the camera profile path that is already captured
there.

## Task 3 — the worker resolves the look once

`worker.rs`: a `LookStore` plus a one-slot cache, mirroring `RenderEngine`'s worker. ⚠ Per **batch**,
not per photo — a 786 KB cube loaded per photo would be per-photo work for a per-batch fact.

## Task 4 — `export_one` builds the real `ProfileRendering`

`ProfileRendering::default()` disappears from that call site. The look arrives as a parameter,
because `export_one` must not do file I/O for a per-batch fact.

## Task 5 — the guards

⚠⚠ **Every fixture curve must be NON-IDENTITY.** The catalogue's own profile is `[[0,0],[1,1]]`, so
a test written against it passes with the fix reverted — which is exactly how this bug survived.

| test | sabotage |
|---|---|
| the curve reaches an exported file | withhold `curve` |
| the baseline exposure reaches one | withhold `baseline_ev` |
| the look reaches one | withhold `look` |
| a missing `.cube` exports without the **look**, keeping the rendering | make that path suppress the rendering |

⚠ And a test that an already-rendered source (JPEG/PNG/TIFF) is **not** rendered twice.

## Task 6 — verify and gate

- Re-run `probe_export_rendering`: the three real rows stay **0.00** for this catalogue and the
  fixture check stays large. ⚠ An exported pixel that *moves* here means the fix changed something
  the measurement said it must not.
- FMT / CLIPPY / TEST, each exit code from its own command, before committing. 74 goldens.
- Launch the GUI, export a photo, confirm by use. **Do not merge, do not push.**

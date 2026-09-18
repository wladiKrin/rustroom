# The creative look reaches a thumbnail

**Date:** 2026-09-18
**Status:** approved
**Reported by use:** *"the look is not reflected or shown in any way in the thumbnail picture"*

## What this is

`thumb_loader::render_edits` builds its pipeline with `pipeline::build_pipeline_with`, which takes
**no `ProfileRendering` at all**. So a thumbnail renders every edit except the creative look.

⚠ It is a gap rather than a decision, and the code says so: `thumbnail_edits`'s own doc reads
*"Everything else is kept, because an edit-accurate thumbnail is the whole point of rendering one at
all."*

⚠⚠ **The THIRD place `ProfileRendering` has failed to reach.** The preview has it; the export did
not until 2026-09-18; thumbnails do not. The root cause is that the whole-pipeline builders
(`build_pipeline`, `_with`, `_to`, `_spec`) default it **silently**, so a new call site gets no look
and no warning. Recorded in `docs/BACKLOG.md`; **not** refactored here, because three builders and
their callers is a separate change and mixing it in would make this diff unattributable — the same
reasoning M8.9 used for this bug's sibling.

## What already works, and must keep working

- `thumb_key::edits_fingerprint` serializes the **whole** `Edits`, so a look change already
  invalidates the cached thumbnail. No key change is needed.
- The neutral early return is `req.edits == Edits::default()`, which a look-only edit is not, so
  such a photo already reaches the pipeline.
- ⚠⚠ `thumbnail_edits` strips `rendering`, because **a thumbnail is the camera's embedded JPEG and
  has already been rendered.** That must not change. The look is a creative grade the user asked
  for; the camera rendering is not, and running ours over an already-rendered JPEG renders it twice
  — the defect found by use in the export path hours earlier.

## Design

**1. `pipeline::build_pipeline_rendered(edits, scale, resampler, profile)`** — a delegating wrapper.
The precedent is written into `build_pipeline_to`: *"Additive with a delegating wrapper — the
`Region` precedent, which gave that work zero call-site churn."*

**2. The `LookStore` is captured in the closure `ThumbLoader::new` already builds.** That closure
already captures a `ThumbStore` for exactly this reason — *"the store is captured here rather than
passed to `ThumbLoader::new`, which keeps that signature — and therefore its four call sites —
untouched."* The looks store goes beside it.

**3. ⚠⚠ A SHARED cache, because thumbnails run on `worker_count()` threads.** `RenderEngine`'s
one-slot look cache is right for one photo at a time and wrong here. A
`Mutex<HashMap<String, Option<Arc<Cube>>>>` loads each distinct `.cube` once.

⚠ The value is an `Option`: a **failed** load is cached as a miss. Without that, a preset naming a
`.cube` this machine does not have would re-read the directory once per thumbnail — 830 times on a
folder open, for a file that is not there.

**4. `render_edits` passes `ProfileRendering { curve: None, baseline_ev: None, look }`.** Curve and
baseline stay `None` on purpose: the rendering is stripped above, so supplying them would be inert
at best and a double render at worst.

## Testing

| guard | sabotage |
|---|---|
| a look changes a rendered thumbnail | pass `look: None` |
| a `.cube` this machine does not have leaves the thumbnail rendered, not blank or failed | make a failed load propagate |
| a failed load is attempted **once**, not per thumbnail | drop the negative caching |
| the camera rendering is still stripped for a thumbnail | remove the strip — it must fail |

⚠⚠ **The cube fixture must change a NEUTRAL image.** A cube that permutes channels is a perfect
no-op on grey, and every fixture here is grey — that exact mistake cost a debugging round on the
export look test the same day.

## Done when

- A photo with a look shows it in the grid and the filmstrip, confirmed **by use**.
- FMT=0, CLIPPY=0, TEST=0, each from its own command; 74 goldens unmoved.

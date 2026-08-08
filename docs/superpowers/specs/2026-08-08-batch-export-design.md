# Batch export — design

Date: 2026-08-08
Status: approved

## Goal

Export the current selection to a folder: one JPEG per photo, at a chosen quality and an
optional long-edge cap, with progress, cancellation and a per-file collision prompt.

The last of the Phase 2 gaps. It reuses `app::selection::Selection` unchanged — **that model
was designed once, on its own terms, precisely so this could.**

## Why this is not "sync, but with pixels"

Sync decodes nothing: it reads and writes sidecars, so 200 photos is 200 tiny file writes.
Batch export **decodes every photo at full resolution**, ~11 s each on the X-T4. That single
difference is why this needs a worker thread, a progress display and cancellation, none of
which sync required.

## 1. `app::batch_export` — a sequential worker

**One photo at a time, deliberately.** The pipeline stages are already rayon-parallel
*internally*, so exporting several photos concurrently would contend for the same cores while
multiplying peak memory: a 26 Mpx decode is ~312 MB as f32 RGB, and `PipelineStage::apply`
consumes and produces buffers. Sequential keeps the memory ceiling at roughly one image.

**The decode fn is injected**, with the same shape `DecodeWorker::new` already uses so the
two look alike:

```rust
decode: impl Fn(&Path) -> Result<ImageBuf, String> + Send + 'static
```

`impl Fn` rather than a bare `fn` pointer so a test can close over state — counting calls, or
failing one specific path — without threading that through the arguments.

That makes the loop — naming, collisions, skip/overwrite/keep-both, cancellation, progress
counting, the failure path — **unit-testable with no image fixtures**. Only the decode itself
is untestable, and it is one line.

## 2. Per photo: downscale *before* the pipeline

```
decode (linear, full) → downscale_to(max_edge) → build_pipeline(edits, scale).run() → save_jpeg
```

The obvious alternative — render at full resolution and shrink the result — would downscale
**sRGB-encoded** pixels, which is gamma-incorrect. Downscaling first keeps the resize in
linear light where it belongs.

It is also the path the preview already takes, so it reuses `downscale_to` and the
`RenderScale` seam **unchanged and needs no change to `pipeline` at all**, and it is faster,
because every stage then runs on fewer pixels.

`scale` is computed exactly as `render_engine::render_scale` does: the source's own scale
(1.0 — export always decodes full, never `preview()`) times `rendered_long / full_long`.

⚠ **Consequence, documented rather than hidden:** sharpening and noise-reduction radii scale
with the output, preserving the *look* rather than the pixel radius. That is what
`RenderScale` exists for, but it means a resized export is not identical to a full-res export
downsampled afterwards, and there is no separate output-sharpening pass.

## 3. Naming and collisions

Destination is `<dest_dir>/<source file stem>.jpg`.

⚠ **A collision can occur in an empty folder.** `DSCF6509.RAF` and `DSCF6509.jpg` both map to
`DSCF6509.jpg`, so the check must consider names already produced **in this run**, not only
what exists on disk. The worker therefore keeps a set of names it has claimed.

On the first unresolved collision the worker **blocks** and asks the UI:

| choice | effect |
|---|---|
| **Skip** | leave the existing file, count it as skipped |
| **Overwrite** | replace it |
| **Keep both** | first free `name-1.jpg`, `name-2.jpg`, … |
| **Cancel export** | end the run |

plus an **"apply to all remaining"** checkbox.

### ⚠ "Apply to all" must not survive a Cancel

This is the trap `Settings::apply_delete_confirm` guards, one feature later: tick the box,
think better of it, cancel — and nothing should have been armed, because the user made no
per-file choice at all.

Here it is solved by the **type** rather than by a rule: `Decision::Cancel` (§5) carries no
`apply_to_all` field, so the bad state is unrepresentable and the UI has nothing to get wrong.
That is strictly better than the delete confirm's arrangement, where the flag and the outcome
are separate values held together only by a tested invariant. The worker-level test then only
has to pin that Cancel ends the run.

## 4. Cancellation is between photos

An `Arc<AtomicBool>` checked before each item. libraw's `unpack` is not interruptible — the
same finding behind "abandon, don't abort" for preview decodes — so cancelling takes effect
once the photo in flight finishes, up to ~11 s later. The progress line says "Cancelling…" so
the delay is explained rather than mysterious.

## 5. Types and messages

```rust
pub struct ExportOptions {
    pub dest_dir: PathBuf,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Fit within this many pixels on the long edge. `None` = full resolution.
    pub max_edge: Option<usize>,
}

/// What to do about one destination that already exists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Collision { Skip, Overwrite, KeepBoth }

/// The user's answer to a `Collision` prompt.
pub enum Decision {
    Resolve { collision: Collision, apply_to_all: bool },
    Cancel,
}

pub enum ExportEvent {
    Started { total: usize },
    Item { done: usize, total: usize, name: String },
    Collision { dest: PathBuf },          // the worker is now blocked
    Finished(ExportReport),
}

pub struct ExportReport {
    pub exported: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}
```

The UI answers a `Collision` with a `Decision` on a second channel. A failed photo — decode
error, unwritable destination — is **collected and the batch continues**; one unreadable file
must not cost the other 199 their export.

## 6. Edits, and the sidecar fix

Each photo's edits come from its sidecar (absent or malformed → `Edits::default()`, via
`sidecar::load`). The **open** photo instead uses the session's in-memory edits, so unsaved
changes export.

⚠ **`export_dialog` currently writes a sidecar next to the *exported* file** —
`sidecar::save(&out, &edits)` produces `export.jpg.json`. JPEG is a supported input format, so
reopening an exported file loads those edits and applies them **a second time**, to pixels
that already have them baked in. `persist_current_session` already writes the sidecar next to
the *source*, so the line is redundant as well as wrong. **It is removed**, and batch export
never writes one.

## 7. UI

- **`Export N…`** in the Develop toolbar whenever the selection exceeds one, beside the
  existing single-photo `Export…`.
- A dialog (an `egui::Modal`, like every other): destination folder via `rfd`, JPEG quality,
  and an optional long-edge cap.
- While running, a progress line — `Exporting 12 of 47 — DSCF6509.RAF` — with **Cancel**.
- The collision dialog described in §3.

**No undo.** Export creates files rather than destroying them; the one destructive case,
overwriting, is guarded by the collision prompt instead.

## 8. Testing

**`batch_export`, with an injected decode fn and a temp directory** — no image fixtures:

- every selected photo produces `<stem>.jpg`;
- a same-stem pair (`x.RAF`, `x.jpg`) collides **within one run**, in an empty folder;
- Skip leaves the existing file byte-identical and counts it;
- Overwrite replaces it;
- Keep both writes `name-1.jpg`, and again `name-2.jpg`;
- "apply to all" stops further prompts, and the remembered choice is the one applied;
- Cancel ends the run with `cancelled: true` and leaves the remaining photos untouched;
- a decode failure is collected in `failed` and the batch continues;
- the long-edge cap is honoured, and no cap exports at full size;
- progress events count up to the total exactly once.

"Apply to all" surviving a Cancel needs no test: `Decision::Cancel` cannot carry the flag
(§3), so there is no state to assert about.

**No pipeline stage is touched — the 23 goldens must stay byte-identical.**

## 9. Accepted caveats

1. **Cancelling waits for the photo in flight** (up to ~11 s).
2. **A resized export is not a downsampled full-res export** — radii scale with the output,
   and there is no output-sharpening pass (§2).
3. **JPEG only.** TIFF and PNG export are a separate feature.
4. **No filename templating** — `<stem>.jpg`, nothing configurable.
5. **No undo**, per §7.
6. **Export settings are not persisted** between runs; `Settings` gains nothing here.
7. **Sequential**, so a 47-photo export is ~47 × 11 s. The demosaic finding
   (`user_qual` defaults to AHD, ~93% of the decode) is the lever that would move this, and it
   is tracked separately.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- In the GUI: select several photos, `Export N…`, choose a folder and a long-edge cap, watch
  progress, cancel mid-run; re-run into the same folder and exercise Skip / Overwrite / Keep
  both, with and without "apply to all".
- Exporting a photo and reopening the result no longer double-applies its edits.
- `CLAUDE.md` updated — Phase 2 complete.

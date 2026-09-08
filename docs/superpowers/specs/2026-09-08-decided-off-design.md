# "Decided off" for the rendering and the lens corrections — design

Date: 2026-09-08
Status: **design agreed, ready to plan.**

The first of the two **M5.4b leftovers**, widened (user's call) to the lens corrections, which
have the identical bug.

## What it is

`Edits.rendering` becomes `Option<Rendering>` and `Edits.lens_correction` becomes
`Option<LensCorrection>`, where `None` means **never decided** and `Some(..)` means **decided**,
including *decided off*. Turning the camera rendering or the lens corrections off on an otherwise
untouched photo then survives a photo switch, because "off" is no longer the same value as "never
touched".

## The bug, precisely

`Sidecar::is_neutral` compares a copy's edits against `Edits::default()`, and `persist` deletes a
neutral sidecar. A fresh photo is seeded from the preferences (`Settings::initial_edits`) with
`rendering: Camera` and the three lens corrections on; the seeded values are ordinary field values,
so:

- with **both** preferences on, turning the rendering off leaves the lens corrections non-default,
  the sidecar survives, and the "off" sticks — which is why the bug is easy to miss;
- with auto lens correction **off**, or on any photo where the lens corrections are the default,
  turning the rendering off makes the edits equal `Edits::default()`, the sidecar is deleted, and
  the preference reseeds `Camera` on the next open. **"Off" does not stick.**
- turning the three lens corrections off has the same failure the other way round, whenever the
  rendering is at its default.

⚠ `CLAUDE.md`'s M5.4b note recorded the first case as unconditional; it is conditional on the
other preference. The model flaw it named is right regardless: *"not decided"* and *"decided
off"* are one value, the distinction `BaselineExposure`'s `None`/`Some(0.0)` already draws one
layer down.

## The design

### 1. Two `Option` fields, `None` = never decided

```rust
/// How the scene is mapped onto a display. `None` = never decided, which renders as
/// `Rendering::None`; `Some(Rendering::None)` = decided off, which is a different fact and
/// survives a photo switch. See `applied_rendering`.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub rendering: Option<Rendering>,
/// The lens corrections, on the same rule: `None` = never decided (renders as none);
/// `Some(LensCorrection::default())` = decided off.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub lens_correction: Option<LensCorrection>,

impl Edits {
    /// What the pipeline applies: the decided value, else nothing. ⚠ The ONE read rule.
    pub fn applied_rendering(&self) -> Rendering { self.rendering.unwrap_or_default() }
    pub fn applied_lens_correction(&self) -> LensCorrection { self.lens_correction.unwrap_or_default() }
}
```

`Edits::default()` stays strictly neutral (`None`, `None`), so the 59 goldens and the CLI are
untouched — the property the M5.4b design was built to keep.

⚠ **Preference resolution stays at first open, exactly as today (user's call).**
`Settings::initial_edits` seeds `Some(Camera)` when the preference is Camera and `Some(all three
on)` when auto lens correction is on; it seeds **`None`, not `Some(off)`**, when a preference is
off — otherwise a preference set to off would write a sidecar for every photo ever opened, where
today it writes none. A later preference change still does not reach an already-opened photo; the
"resolve live at render time" alternative was offered and declined as touching four render seams.

### 2. Reading and writing

Every consumer reads through the two accessors: the pipeline's prefix builder and rendering
placement (`build_prefix_steps`, `build_pipeline_split`), the CLI, `perspective_detect`, the spot
overlay's corrector, `auto_tone_tool`'s median target, the Geometry panel's lens scale, and the
golden harness. The rule "undecided renders as off" lives in the accessors and nowhere else.

Writers write `Some`:

- the **Camera rendering** checkbox writes `Some(Camera)` / `Some(Rendering::None)`;
- the three **lens checkboxes** read `applied_lens_correction()` into a local, and write the whole
  struct back as `Some(..)` on any change;
- the Geometry panel's **lens scale** slider is drawn only while `distortion` is on, which means the
  field is already `Some`, so it binds through `if let Some(l) = &mut s.edits.lens_correction`;
- `render_engine`'s strip for an already-rendered source (JPEG/PNG/TIFF) and `thumbnail_edits`
  both set `rendering: None` — *undecided*, not decided off, because a stripped copy is a render
  input and must not read as a choice anyone made.

`merge_from` copies both `Option`s **verbatim**. Consequences, stated: a paste, preset or sync
from an undecided source makes the target undecided — in practice sources are seeded at open, so
they are decided; and **Reset adjustments** (`merge_from` from a default `Edits`) leaves the lens
corrections undecided, which renders off, as a reset does today.

### 3. Sidecars: nothing existing changes meaning

`skip_serializing_if = "Option::is_none"` means an undecided field is **absent** from the file,
never `null`. Reading:

| stored | loads as | renders | same as today? |
|---|---|---|---|
| no `rendering` key (pre-M5.4b) | `None` | colorimetric | yes |
| `"rendering": "None"` | `Some(None)`, decided off | colorimetric | yes |
| `"rendering": "Camera"` | `Some(Camera)` | camera | yes |
| no `lens_correction` key | `None` | none | yes |
| `"lens_correction": {…all false…}` | `Some(off)`, decided | none | yes |

⚠ A sidecar that already holds `"rendering": "None"` was, in the buggy case, written by exactly
the user who turned it off — so reading it as *decided off* is the reading they meant.

### 4. `is_neutral` is untouched — and that is what fixes it

A decided-off `Some(..)` is not `None`, so the edits are not `Edits::default()`, so the sidecar
is written and kept. A never-decided fresh photo with both preferences off is `None`/`None`, so
no sidecar is written for it — no litter. A fresh photo with a preference on is `Some`, so it
gets a sidecar on the first switch away, **as it does today**.

⚠ The alternative — comparing against the preference seed instead of `Edits::default()` — was
rejected: neutrality would then depend on the machine's preferences, so the same sidecar would
mean different things on two machines.

### 5. Not in scope

Live preference resolution; migrating existing sidecars (none needs it); a UI state for
"undecided" (the checkboxes show the applied value and any click decides).

## Testing

- **edit-model**: `Edits::default()` has both `None`; an undecided field applies as off; a decided
  off is not the default (`Some(Rendering::None) != None`, `Some(LensCorrection::default()) !=
  None`); an undecided field is **absent** from the serialised sidecar and `"rendering": "None"`
  loads as decided off; the pre-M5.4b sidecar test now asserts `None`; the group-transfer tests
  updated to `Some`. The three exhaustive fixtures (`pipeline::all_edits`,
  `edit_model::all_nonneutral`, `edit_clipboard`'s payload) carry `Some`, which the compiler
  forces.
- **settings**: `initial_edits(None)` seeds `Some(Camera)`/`Some(on)` under on-preferences and
  `None`/`None` under off-preferences; a stored `Some(Rendering::None)` is kept.
- **sidecar** — the bug as a test: edits with `rendering: Some(Rendering::None)` and
  `lens_correction: Some(LensCorrection::default())` and nothing else are **not** neutral, so
  `persist` writes rather than deletes; and `None`/`None` *is* neutral, so a fresh photo under
  off-preferences writes nothing.
- **panels, headless**: unticking Camera rendering writes `Some(Rendering::None)`; unticking a
  lens box writes `Some(..)` with that flag off; the existing clicks keep their assertions
  through the accessor.
- **No golden moves.**

## Files

| file | change |
|---|---|
| `crates/edit-model/src/lib.rs` | the two fields, accessors, serde attributes, `merge_from` (verbatim, unchanged shape), fixtures, tests. ⚠ ~3,700 lines; say so. |
| `crates/pipeline/src/lib.rs`, `tests/golden.rs`, `src/auto_tone.rs` callers | read through the accessors; test literals → `Some` |
| `crates/rustroom/src/main.rs` | two reads |
| `crates/app/src/settings.rs` | `initial_edits`, `initial_lens_correction -> Option`, tests. ⚠ ~1,900 lines; say so. |
| `crates/app/src/sidecar.rs` | the bug's regression tests |
| `crates/app/src/render_engine.rs`, `thumb_loader.rs`, `auto_tone_tool.rs`, `perspective_detect.rs`, `edit_clipboard.rs`, `looks.rs` (tests) | reads/strips/literals |
| `crates/app/src/develop/panel/adjustments.rs`, `panel/geometry.rs`, `panel/mod.rs` (test), `canvas/spot_overlay.rs` | writers and reads |
| `crates/app/examples/*.rs` | literals → `Some` (`--all-targets` builds them) |
| docs | roadmap M5.4b leftover (1) closed, `CLAUDE.md` |

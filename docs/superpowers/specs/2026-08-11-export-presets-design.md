# Export presets — design

**Date:** 2026-08-11
**Status:** approved, ready to plan
**Scope:** `app` only — no pipeline, no `edit-model`, no new stage

Closes **M2.4**, and with it **Phase 2**.

## Two features, not one

"Export presets" hides two things that are worth separating, and they land as **two commits**:

1. **Persistence.** `ExportOptions` currently lives only on `RustroomApp` and dies with the
   process, so every export re-picks the destination folder from scratch — even the fiftieth
   export to the same place. This is the half felt daily, and it is the smaller half.
2. **Named presets.** Saved combinations, selected from a dropdown.

Persistence is useful on its own and must not wait on the preset UI.

## What a preset contains

```rust
/// Lives in `crates/app/src/export_presets.rs`.
pub struct ExportPreset {
    /// Display name. Stored **inside** the file as well as in the filename, because the
    /// filename is sanitized and the display name should survive that — the same reason
    /// `Preset` does it.
    pub name: String,
    pub options: ExportOptions,
}
// ExportOptions = { dest_dir: PathBuf, quality: u8, max_edge: Option<usize> }
// — already Debug + Clone + PartialEq + Eq; gains Serialize + Deserialize.
```

**The destination is included.** "Web 2048 → ~/Desktop/web" and "full-res archive → the
external drive" are the workflows presets exist for, and different outputs genuinely go to
different places.

The obvious objection — selecting a preset silently moves your output folder — does not bite
here because **the export modal shows every field**, so the destination visibly changes before
Export is pressed. Hidden state would be a problem; visible state is just state.

⚠ The consequence, accepted: **a preset contains an absolute local path and is therefore
machine-specific.** This is what makes the missing-destination handling below mandatory rather
than optional, and it is why the storage argument below differs from `PresetStore`'s.

## Storage

`ExportPresetStore` in `<config>/rustroom/export-presets/`, **one JSON file per preset** — the
same shape `PresetStore` uses.

⚠ Note the argument is *not* the same. `PresetStore` chose one-file-per-preset primarily so a
develop preset is a **shareable artifact**; an export preset contains an absolute path and is
not shareable, so that reason does not carry. What does carry: no read-modify-write race on
save or delete, one corrupt file cannot take the rest down, and — the deciding one — it is
**one pattern to understand rather than two**.

Location is platform-specific via `dirs::config_dir()`. ⚠ On **macOS that is
`~/Library/Application Support/rustroom/`**, not `~/.config` — a mistake this file has already
made once.

### `app::safe_name` — the one function worth sharing

`sanitize` is currently private to `presets.rs`. It is the function that stops a preset named
`../../evil` from escaping its directory: **the single security-relevant line in either
store**. It moves to a small shared `app::safe_name` module, and `presets.rs` calls it.

Duplicating a path-escape guard is precisely how such a guard drifts out of sync and becomes a
real bug. Same reasoning that shared the guided filter between noise reduction and dehaze
rather than copying it.

Its known limitation transfers unchanged and stays documented rather than hidden: sanitizing
is many-to-one, so `a/b` and `a_b` share a file and the later save wins. Only a name that
sanitizes to *empty* is rejected.

### No trash, no undo, no delete confirmation

Deliberately **unlike** develop presets, which have a 20-entry trash and Ctrl+Z restore.

A develop preset can encode an hour of grading; an export preset is three fields retyped in
ten seconds. That asymmetry is the whole justification, and it is a real difference rather
than an economy. Deletion is immediate, and `ActionLog` is not touched.

## Persistence

`Settings` gains `export: ExportOptions`. It already exists, already loads at startup, and
`SettingsStore::load()` is already **infallible** — a missing *or malformed* file yields
defaults, so a corrupt settings file can never brick export.

**Persisted when an export actually starts**, not on every keystroke and not on cancel. These
are the *last-used* options, so what should survive is what was used: tweaking the quality
slider and then cancelling the dialog should leave the remembered settings alone, the same
rule `Settings::apply_delete_confirm` follows for the "don't ask again" tick. It also keeps
the write count to one per export rather than one per slider frame.

⚠ An older `settings.json` written before this field must still load, with the field
defaulting. `Settings` already carries `#[serde(default)]`, and `ExportOptions` will need
`Default` for that to apply — it has none today, so one is added (empty destination, the
current quality default, no long-edge cap).

## The missing-destination case

The honest cost of putting a folder inside a preset. It gets handled, not hoped away.

When the modal opens — or a preset is selected — a `dest_dir` that does not exist is shown
marked **(missing)** and **Export stays disabled** until a valid folder is picked.

A **pre-flight** check, not a per-photo failure. `batch_export` already survives an unwritable
destination without panicking (`an_unwritable_destination_is_an_error_not_a_panic`), but
failing 200 photos one at a time and reporting afterwards is a bad experience when the answer
is knowable up front.

⚠ The readiness rule is a **pure function of the path**, not logic embedded in the egui
closure — the same extraction that made `Settings::apply_delete_confirm` and `view_region`'s
maths testable. Without that, "Export is disabled when the folder is gone" is only verifiable
by clicking.

```rust
/// Whether a destination is usable. Empty = never chosen; missing = chosen once and gone.
pub fn destination_ready(dir: &Path) -> bool;
```

## UI

One row at the top of the **existing** export modal:

```
[ Preset: (none) ▼ ]  [ Save as… ]  [ Delete ]
──────────────────────────────────────────────
Destination…   ~/Desktop/web
JPEG quality   [========|--] 88
☑ Fit within a long edge   [====|------] 2048
```

Selecting a preset fills every field below it, which is what keeps the destination change
visible. No new panel: the modal already exists and already shows all three fields.

`Save as…` reuses the name-entry shape the develop-preset save dialog already has.

## Tests

Both stores take their directory **by injection**, so tests run against temp directories and
never touch the real config — the established pattern (`DecodeWorker`, `PresetStore`,
`SettingsStore`).

- **`safe_name` keeps its path-escape tests** (`../../evil` → `______evil`, `a/b\c` → `a_b_c`,
  empty-after-sanitize rejected) and gains a note that two callers now depend on it.
- **`presets`' existing tests are the proof the extraction changed nothing** — the same role
  the goldens played for the guided-filter refactor. They must pass untouched.
- **`a_preset_round_trips_through_the_store`** and
  **`a_missing_directory_lists_empty_rather_than_erroring`**, mirroring `PresetStore`.
- **`settings_round_trip_carries_the_export_options`**.
- **`cancelling_the_export_dialog_leaves_the_remembered_options_alone`** — the "persist what
  was used, not what was typed" rule, as a pure decision rather than something only observable
  by clicking Cancel.
- **`a_settings_file_from_before_export_options_still_loads`** — ⚠ the forgiving-load
  property. This is what stops an existing config from breaking the app on upgrade, and it is
  the test most likely to be skipped as "obviously fine".
- **`destination_ready`**: empty is not ready; a real temp directory is; a path that does not
  exist is not; a *file* rather than a directory is not.

## Accepted caveats

1. **Presets are machine-specific by construction** — they contain an absolute path. Copying
   one to another machine leaves a destination that reads as missing, which is at least a
   clear failure rather than a silent export to the wrong place.
2. **No import/export of the presets themselves**, and no folder organisation. Flat list, like
   develop presets.
3. **Deletion is immediate** — no trash, no undo, no confirmation. Justified above; the
   asymmetry with develop presets is intentional and documented so it does not read as an
   oversight.
4. **A non-UTF-8 destination path will not serialise to JSON.** Vanishingly rare, and it
   degrades to "not persisted" rather than to a crash, because `load()` is infallible.
5. **No per-preset file naming or format choice** — JPEG only, `<stem>.jpg`, unchanged. Those
   are separate features that `batch_export` already lists as caveats.

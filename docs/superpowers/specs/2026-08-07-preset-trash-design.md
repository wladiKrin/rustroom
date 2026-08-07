# Preset trash & undo delete — design

Date: 2026-08-07
Status: approved

## Goal

Make an accidental preset deletion recoverable. Deleting moves the preset to a bounded
trash directory instead of unlinking it, and Ctrl+Z restores the most recent one.

## Why not the photo history

`History<Edits>` cannot hold this, for three independent reasons:

1. It lives on `Session` and is **discarded when you switch photos** — delete a preset, arrow
   to the next shot, and the record is gone.
2. It stores `Edits` *values*. It has no way to express "recreate this file".
3. Interleaving a global, filesystem-level action into a per-photo stack makes Ctrl+Z
   ambiguous.

So this gets its own mechanism, sharing only the keystroke.

## 1. Bounded trash — the sizing argument

A worst-case preset (all 8 HSL bands non-neutral, 16-point tone curve) measures **2809 bytes**;
typical is ~1.5 KB. A hundred trashed presets is ~275 KB.

**Disk space is therefore never the binding constraint**, which rules out a size cap on
evidence and reframes the whole question: what actually degrades is *clutter*, not bytes.
An age cap is rejected for a worse reason — the entry you want back is precisely the one that
just silently expired, and it makes correctness depend on the system clock.

**Policy: keep the newest `TRASH_CAP = 20`, evicted at delete time.** Self-maintaining — no
startup sweep, no timer — and explainable in one sentence.

## 2. Layout

```
<config dir>/rustroom/
├── presets/     live presets
└── trash/       deleted presets
```

**A sibling directory, not `presets/.trash/`.** A subdirectory would oblige every present and
future globber of the presets directory to remember to exclude it — a standing trap.
`PresetStore::list()` stays exactly as it is.

### Filenames carry the deletion time

`trash/<13-digit unix millis>-<sanitized name>.json`. This solves **two** problems at once:

- **Ordering for eviction and undo**, without trusting file mtime — rsync and backup restores
  rewrite mtime, and it is not a fact about when the user deleted anything. Zero-padding makes
  lexicographic order identical to chronological.
- **A collision that a plain `trash/<name>.json` would have**: delete "Punchy", recreate it,
  delete it again — the second deletion would clobber the first trashed copy. With the prefix
  both survive.

⚠ **Two deletions within the same millisecond would still collide**, which a test doing two
deletes back to back hits immediately. The write bumps the timestamp by 1 ms until the path is
free.

## 3. API

`PresetStore` gains a trash directory. The constructor becomes
`PresetStore::new(dir, trash_dir)` — both injected, so tests stay off the real home directory,
and `user_default()` supplies the sibling pair.

```rust
/// Move `name` to the trash, then evict anything beyond `TRASH_CAP`.
pub fn delete(&self, name: &str) -> Result<(), PresetError>;

/// The name of the newest trashed preset, for the undo affordance's label.
pub fn latest_trashed_name(&self) -> Option<String>;

/// Restore the newest trashed preset to the live directory. `Ok(None)` if the trash is
/// empty. Returns what was restored, for the status line.
pub fn undo_delete(&self) -> Result<Option<Preset>, PresetError>;
```

Both readers rescan the trash directory; at ≤20 files of ~2 KB that is not worth caching.

## 4. Undo semantics

`RustroomApp` tracks whether a deletion is the most recent thing that happened:

- Deleting a preset arms it.
- **Any photo edit (`history.push`) disarms it**, so Ctrl+Z goes back to meaning photo undo.
- A successful restore disarms it.

While armed, **Ctrl+Z restores the newest trashed preset instead of undoing a photo edit**, and
the preset panel shows an **"Undo delete '<name>'"** button so it is not a hidden keystroke.

**Deliberately one-deep.** Repeated Ctrl+Z does *not* walk back through older deletions: that
would keep the key hijacked from photo undo indefinitely. Older entries stay recoverable by
hand from the trash directory — the same "plain JSON in a known place" property presets
already have.

The trash itself has **no UI**: no browser, no "Empty trash" button. Its job is durability
across restarts, which an in-memory stash could not provide.

## 5. Restoring over a live preset overwrites it

Restoring "Punchy" while a live "Punchy" exists replaces it, and the status line says so.
Consistent with saving, which already overwrites by name. Renaming to "Punchy (restored)"
was the alternative and is quieter, but it leaves two near-identical presets behind, which is
the worse outcome for an undo.

## 6. Testing

- Deleting moves the file to the trash and removes it from `list()`.
- `undo_delete` restores it and empties that trash slot; on an empty trash, `Ok(None)`.
- **Deleting the same name twice keeps both trashed copies** — the collision fix.
- **Two deletions in the same millisecond both survive** — the sub-millisecond fix.
- The trash never exceeds `TRASH_CAP`, and it is the **oldest** entries that go.
- Restoring over a live preset of the same name overwrites it.
- `latest_trashed_name` reports the newest, not an arbitrary one.
- A malformed file in the trash is skipped rather than fatal, matching `list()`.
- The armed/disarmed rule is a pure function on app state, tested without egui.

**No pipeline stage is touched — the 23 goldens must stay byte-identical.**

## 7. Accepted caveats

1. **One-deep undo.** Older deletions need a file manager.
2. **No trash UI at all** — no browser, no manual empty. A preset stays in the trash until 20
   further deletions push it out.
3. **Clock skew.** A system clock that jumps forward writes a far-future filename that sorts
   last and survives eviction longer than it should. Self-correcting, not guarded.
4. Restoring overwrites a same-named live preset.
5. The cap counts *entries*, not bytes — 20 entries is ~56 KB at worst.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- In the GUI: deleting a preset then pressing Ctrl+Z brings it back; moving a slider first
  makes Ctrl+Z undo the slider instead; the trash never grows past 20 files.
- `CLAUDE.md` updated.

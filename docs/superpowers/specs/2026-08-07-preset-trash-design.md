# Preset trash & undo delete — design

Date: 2026-08-07
Status: approved

## Goal

Make preset deletions recoverable. Deleting moves the preset to a bounded trash directory
instead of unlinking it, and **Ctrl+Z walks back through deletions and photo edits in one
chronological order** — so several deletions can be undone in a row, and a deletion survives
an intervening slider change.

## Why not `History<Edits>` itself

The photo history cannot *hold* this, for two reasons: it lives on `Session` and is discarded
when you switch photos, and it stores `Edits` values, so it has no way to express "recreate
this file".

But that argues only against putting preset deletions *inside* it — not against ordering the
two together. `ActionLog` (§4) keeps one order over both while each payload stays where it
belongs.

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

/// The name of the newest trashed preset.
pub fn latest_trashed_name(&self) -> Option<String>;

/// Restore the newest trashed preset **named `name`**. `Ok(None)` if there is none —
/// it may have been evicted past the cap. Returns what was restored, for the status line.
pub fn restore(&self, name: &str) -> Result<Option<Preset>, PresetError>;
```

By name rather than "whatever is newest" so an undo targets its own deletion however the
trash was reordered in between. Both readers rescan the trash directory; at ≤20 files of
~2 KB that is not worth caching.

## 4. Undo semantics — one chronological stack

> **Revised after the first implementation.** The first version made this one-deep, on the
> reasoning that a deeper undo would keep Ctrl+Z hijacked from photo undo. That was wrong on
> both counts: it made the trash nearly pointless (19 of 20 entries unreachable), and the
> ordering problem it feared is solved by *interleaving* the two rather than by choosing
> between them.

`app::action_log::ActionLog` holds a **single chronological order** over both kinds of
undoable action:

```rust
enum ActionKind { PhotoEdit, PresetDelete { name: String } }
```

It records only **which stack the next undo should pop** — payloads stay in
`Session::history` and the trash, so nothing is duplicated and both existing mechanisms work
unchanged. Ctrl+Z, Ctrl+Shift+Z and the toolbar's Undo/Redo buttons all drive it.

So *delete a preset → change a setting → Ctrl+Z → Ctrl+Z* reverses the setting and then
restores the preset, and several deletions restore in a row.

The Undo button's label becomes **"Undo delete '<name>'"** when that is what it would do, so
one control never silently performs the surprising one. The preset panel shows the same
action as a button when it is next in line.

### ⚠ Two synchronization hazards

1. **`History::push` is a no-op when the value is unchanged**, so it now returns `bool` and
   the log records a step only when one was really taken. Otherwise the two desynchronize and
   an undo pops the wrong stack.
2. **`Session::history` dies with its session.** A `PhotoEdit` entry outliving a photo switch
   would make an undo pop *this* photo's history and revert an edit the user never made here.
   `forget_photo_edits()` runs on every session change and sweeps **both** the undo and redo
   stacks; preset deletions are global and survive.

Restoring is **by name**, not "whatever is newest", so an undo targets its own deletion
however the trash was reordered in between. A preset already evicted past the cap yields
`Ok(None)` and a status line, not a failure.

The trash still has **no UI**: no browser, no "Empty trash" button.

## 5. Restoring over a live preset overwrites it

Restoring "Punchy" while a live "Punchy" exists replaces it, and the status line says so.
Consistent with saving, which already overwrites by name. Renaming to "Punchy (restored)"
was the alternative and is quieter, but it leaves two near-identical presets behind, which is
the worse outcome for an undo.

## 6. Testing

- Deleting moves the file to the trash and removes it from `list()`.
- `restore(name)` brings it back and empties that trash slot; an absent name yields
  `Ok(None)` without consuming anything else.
- **Restoring targets the named preset**, not merely the newest, when other deletions have
  buried it since.
- **Deleting the same name twice keeps both trashed copies** — the collision fix.
- **Two deletions in the same millisecond both survive** — the sub-millisecond fix.
- The trash never exceeds `TRASH_CAP`, and it is the **oldest** entries that go.
- Restoring over a live preset of the same name overwrites it.
- `latest_trashed_name` reports the newest, not an arbitrary one.
- A malformed file in the trash is skipped rather than fatal, matching `list()`.
- `ActionLog` orders the two kinds correctly: a deletion survives an intervening photo
  edit, several deletions undo newest-first, redo replays in the original order, a new action
  clears the redo stack, and `forget_photo_edits` keeps deletions in order while sweeping
  both stacks. All egui-free.
- `History::push` reports whether it recorded a step.

**No pipeline stage is touched — the 23 goldens must stay byte-identical.**

## 7. Accepted caveats

1. **Photo-edit undo history still does not survive a photo switch** — unchanged from
   before, but now visible in the log, which drops those entries on every session change.
   Preset deletions do survive.
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

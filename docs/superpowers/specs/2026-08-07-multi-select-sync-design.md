# Multi-selection & sync — design

Date: 2026-08-07
Status: approved

## Goal

Select several photos in the grid or the filmstrip, and apply the open photo's settings to
all of them. This is the piece deliberately cut out of the presets spec, so that the
selection model gets designed once, on its own terms — **batch export reuses it and gets its
own spec.**

## 1. `app::selection` — the selection model

`Library.selected: Option<usize>` becomes one struct, pure and `egui`-free so the clamping
and range arithmetic are testable rather than only visible as a wrong highlight — the same
reasoning behind `crop_tool`, `view_region` and `action_log`.

```rust
pub struct Selection { active: Option<usize>, set: BTreeSet<usize>, anchor: Option<usize> }

impl Selection {
    pub fn active(&self) -> Option<usize>;
    pub fn contains(&self, i: usize) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_;   // ascending
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Plain click: this becomes the active photo and the whole selection.
    pub fn click(&mut self, i: usize);
    /// Cmd/Ctrl-click: add or remove. Never touches the active photo.
    pub fn toggle(&mut self, i: usize);
    /// Shift-click: the inclusive range from the anchor, plus the active photo.
    pub fn extend_to(&mut self, i: usize);
    /// Select `0..n`. With no active photo yet, index 0 becomes active — otherwise this
    /// would produce a non-empty set with no active photo and break the invariant below.
    pub fn select_all(&mut self, n: usize);
    /// Back to just the active photo; empty if there is none.
    pub fn clear(&mut self);
    /// Drop indices past `n`, after the folder's contents changed.
    pub fn clamp(&mut self, n: usize);
}
```

### The invariant

> **`active == Some(i)` ⟹ `set` contains `i`; `active == None` ⟹ `set` is empty.**

Sync would otherwise have a source that is not part of what is being synced, or a selection
with no source at all. Every method preserves it, and a test asserts it after each.

`extend_to` therefore unions the anchor→`i` range **with** the active index rather than
replacing the set outright: a preceding Cmd-click can have moved the anchor away from the
active photo, and a bare range replacement would then drop it.

### ⚠ Cmd-click and Shift-click do not move the active photo

Only a plain click or arrow navigation does. Lightroom moves the active on shift-click, but
here that would kick off a **RAW decode in the middle of a range-select**, and it fights the
order of the task: choose the source, *then* extend the target range without the source
shifting underneath. Consequence, accepted: **Cmd-clicking the active photo is a no-op**
rather than deselecting it.

## 2. Interaction

| gesture | grid | filmstrip |
|---|---|---|
| click | `click(i)` | `click(i)` **and open** (today's behaviour) |
| Cmd/Ctrl-click | `toggle(i)` | `toggle(i)`, no open |
| Shift-click | `extend_to(i)` | `extend_to(i)`, no open |
| double-click / Enter | open active | — |
| ←/→ | `click(next)` | `click(next)` and open |
| Cmd/Ctrl+A | `select_all(n)` | `select_all(n)` |
| Esc | `clear()` | `clear()` |

Arrow navigation resets the selection to the new active photo. Leaving a stale multi-selection
behind while the user arrows through photos is how a later sync surprises someone.

Selected-but-not-active cells get a dimmer border than the active cell, so the source is
always identifiable.

## 3. `app::sidecar` — extracted, so sync can be tested

`sidecar_path` currently lives in `main.rs`, where sync cannot reach it and no test can
either. It moves to a small module alongside the read/write it implies:

```rust
pub fn path(photo: &Path) -> PathBuf;                 // `photo.RAF` -> `photo.RAF.json`
pub fn load(photo: &Path) -> Option<Edits>;           // None = absent *or* malformed
pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError>;
pub fn remove(photo: &Path) -> std::io::Result<()>;
```

`load` returning `None` for a malformed file matches the forgiving posture the preset and
settings stores already take.

## 4. `app::sync` — no decoding at all

Every target's edits live in a JSON sidecar, so a sync is `load → merge_from → save`.
Syncing 200 photos is 200 tiny file operations, **not 200 RAW decodes** (~11 s each). That is
what makes this cheap enough to need no progress UI, no worker thread and no cancellation.

```rust
pub struct SyncReport {
    /// Each target's state *before* the sync. `None` = it had no sidecar at all.
    pub prior: Vec<(PathBuf, Option<Edits>)>,
    pub failed: Vec<PathBuf>,
}

pub fn apply(targets: &[PathBuf], source: &Edits, groups: EditGroups) -> SyncReport;
pub fn revert(prior: &[(PathBuf, Option<Edits>)]) -> Vec<PathBuf>;   // paths that failed
```

The **source is the open session's in-memory `edits`**, not its sidecar — you sync what you
are looking at, not what was last written. The active photo is excluded from `targets`.

A failed write on one photo does not abort the batch; failures are collected and reported in
the status line.

## 5. Sync undo

`ActionKind` gains:

```rust
Sync { source: Edits, groups: EditGroups, prior: Vec<(PathBuf, Option<Edits>)> }
```

Undo calls `revert(prior)`; redo re-runs `apply`. **The `Option` is the load-bearing part:**
`None` records "this photo had no sidecar", so undo *deletes* the file the sync created
rather than writing a neutral one and leaving litter behind.

⚠ **`ActionKind` must drop its `Eq` derive.** `Edits` contains `f32` and is `PartialEq` but
not `Eq`. `PartialEq` is all the tests need.

Sync entries are global — they touch other photos' sidecars, never the live session — so
`forget_photo_edits` leaves them alone, exactly like `PresetDelete`.

At ~2 KB per target a 200-photo sync holds ~400 KB of undo state, which is not worth bounding.

### ⚠ One live-session hazard

The active photo is never a target, so no sync can make the open session stale. But a user
can sync, **switch to a synced photo**, and then Ctrl+Z — which rewrites that photo's sidecar
underneath an open session holding its own in-memory edits.

So undo and redo both check whether the current session's path is among the affected ones,
and if so reload its edits and re-render. Without that the canvas silently disagrees with
what is on disk until you switch away and back.

## 6. UI

A **`Sync 12 photos…`** button in the Develop toolbar, enabled only when the selection
exceeds one, opening the **existing** `edit_group_checkboxes` dialog — `EditGroups` doing its
third job, after copy/paste and presets.

## 7. Testing

**`Selection`** carries most of it, all headless: the invariant holds after every operation;
`extend_to` works in both directions and keeps the active photo; repeated shift-clicks extend
from the same anchor rather than the last click; `toggle` on the active is a no-op; `toggle`
on a member removes it; `clear` leaves exactly the active photo; `select_all` keeps an
existing active and adopts index 0 when there is none; `select_all(0)` leaves it empty;
`clamp` drops out-of-range indices and empties the set when the active goes away.

**`sidecar`** — path derivation (`photo.RAF` → `photo.RAF.json`), round-trip, `None` for
absent and for malformed.

**`sync`** — against a temp folder of sidecars: targets gain only the selected groups;
a target with no sidecar gets one; `prior` records `None` for exactly those; `revert` restores
edited sidecars **and deletes the ones that did not exist**; a failed target is reported
rather than aborting the batch.

**`ActionLog`** — a `Sync` entry survives `forget_photo_edits`.

**No pipeline stage is touched — the 23 goldens must stay byte-identical.**

## 8. Accepted caveats

1. **Cmd-clicking the active photo does nothing.** The price of the "modifiers never move the
   active photo" rule.
2. **No Auto-Sync** — no live application while dragging a slider. Its own feature.
3. **Sync only from Develop**, not from the Library grid: the source must be the open photo.
4. **No progress UI or cancellation.** Justified by §4 — this is file I/O on a few kilobytes
   per photo, not decoding.
5. **A sync is one undo step for the whole batch**, not per photo.
6. **Selection resets when the folder is reloaded**, and on plain navigation.
7. Sync writes sidecars for photos that had none, which is visible in the folder.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- In the GUI: Cmd-click and Shift-click build a selection in both the grid and the filmstrip,
  with the active photo distinguishable; `Sync N photos…` applies the chosen groups; Ctrl+Z
  reverses the whole batch, including removing sidecars it created.
- `CLAUDE.md` updated, including batch export as the next thing to reuse this.

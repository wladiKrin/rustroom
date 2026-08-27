# Snapshots — named saved states within one photo

**Date:** 2026-08-27
**Milestone:** M3.4's remaining slice. Closes M3.4 with it.
**Status:** design agreed, ready to plan.

## What this is

A photo can hold several **named saved states** of its own edits — "before the crop", "the
final", "the black-and-white one" — and switch between them, without creating a second Library
row. A snapshot is not a virtual copy: a copy *is* a row, exports as its own file and carries
its own marks; a snapshot is a bookmark **inside** one copy's editing.

It also carries a second job. "Persistent undo history" was dropped on 2026-08-27 because a
persisted undo *stack* is unreliable by construction — `HistoryStash::take_matching` already
discards a stashed history when the sidecar no longer matches, and across a restart the file
could have been touched by anything, so Ctrl+Z would silently work only sometimes. Snapshots
answer the same want — *"get me back to before I went black and white"* — explicitly, and can
never silently vanish.

## The blocker, and its answer

The backlog names one blocker, and it is not storage: **how does a snapshot relate to
`History<Edits>`?** A snapshot is a saved point in the same editing the undo stack already
models, and the two need one story rather than two.

**They are orthogonal, joined at exactly one point.**

| question | answer | why |
|---|---|---|
| Does taking a snapshot push an undo entry? | **No.** | It changes no `Edits`. A `History<Edits>` holds snapshots *of `Edits`*; there is nothing for it to hold. |
| Does undoing past a snapshot delete it? | **No.** | It was never in the stack. Undo walks `Edits`; snapshots live in the sidecar envelope. |
| Does a snapshot survive the per-photo history stash? | **Yes, trivially.** | `take_matching` compares only `Edits`. A snapshot taken while a history is stashed cannot invalidate it. |
| Does restoring one push an undo entry? | **Yes.** | It genuinely changes `Edits`, so `s.history.push` handles it and Ctrl+Z undoes a restore for free — one gesture, one keystroke. |

That is the whole relationship. The independence is what makes the "can never silently vanish"
property real: nothing that happens to the undo stack can reach a snapshot.

### Rejected alternatives

**A snapshot as a named entry inside the history.** One timeline, some steps named.
Conceptually tidy, and persisting it means persisting the whole undo stack — the thing dropped
yesterday. It also makes *"undo past a snapshot"* a question with no good answer: deleting it
loses work the user explicitly saved, keeping it means the stack and the snapshot list disagree
about what happened.

**A snapshot as a virtual copy hidden from the Library.** Cheapest in new types, since the copy
machinery exists. Rejected because every consumer of a copy — `next_copy` numbering, export
filenames, catalog rows, `entries::build`, `Reindex` — would grow an "except the hidden ones"
clause, and that qualifier is how each of them acquires a bug.

## Data model

```rust
/// One saved state within a copy.
pub struct Snapshot {
    /// Identity: `max + 1`, never reused.
    pub n: u32,
    /// `None` until named; the label is then "Snapshot {n}".
    pub name: Option<String>,
    /// When it was taken — local wall-clock seconds since the epoch.
    pub taken: i64,
    pub edits: Edits,
}
```

It hangs off `CopyEntry`, beside `name` and `edits`:

```rust
pub struct CopyEntry {
    pub copy: u32,
    #[serde(default)] pub name: Option<String>,
    #[serde(default)] pub edits: Edits,
    #[serde(default)] pub snapshots: Vec<Snapshot>,
}
```

- **Per copy, not per file.** Each copy has its own `Edits`, so it has its own history to
  bookmark. This is the same split marks take (per copy) and the opposite of keywords (per
  file), for the same underlying reason: a snapshot describes *the image you are making*.
- ⚠ **`#[serde(default)]` belongs on the field and is safe here** — every existing sidecar loads
  with an empty list. ⚠ It must still never be added to `Sidecar` itself; that combination is
  the documented data-loss trap (`load_all`'s doc comment records the sabotage that proved it).
- **`n` is identity, `max + 1`, never reused** — the copy-number rule for the copy-number reason:
  the label of an *unnamed* snapshot is derived from it, so reusing a number would silently
  rename an existing row.
- **`taken` is local wall-clock seconds**, matching `Capture.taken`. That decision is already
  made and recorded: wall-clock means rendering needs no timezone, and it is what a person means
  by "when I took it". `chrono` is already a dependency of `app`, so this costs nothing new.

### `label()` is the one rule

```rust
impl Snapshot {
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("Snapshot {}", self.n))
    }
}
```

One function, every consumer — the strip's row, the rename field's initial text, the name seeded
into a virtual copy. That is `entries::shown_name`'s rule, and the reason it exists: this project
has already shipped a bug where the Library printed `file_name` while a copy's real name lived
somewhere else, because two implementations of "what is this called" existed.

### `Sidecar::is_neutral` must learn about snapshots

⚠ **Without this, take a snapshot, then press "Reset adjustments", and the sidecar is deleted
with every snapshot in it.** `persist_all` removes the file when `is_neutral()`, and today that
is true of "a lone unnamed copy 0 with neutral edits". A copy holding snapshots is worth a file
however neutral its current edits are.

That function already documents this exact hazard for names — *"a name or a second copy is worth
a file even with neutral edits, or making a copy and then resetting it would delete the copy"* —
so snapshots are the third case of a rule that is already written down.

## Undo

Two new `ActionKind` variants, both **scoped to their `PhotoId`** by the same `reachable` rule
`CopyRename` uses, so Ctrl+Z on another photo cannot reach them:

```rust
/// A deleted snapshot, held so undo can put it back.
///
/// ⚠ Carries the whole entry because nothing else holds it — `CopyDelete`'s shape and reason.
/// Boxed for the reason `Sync` is: an inline `Edits` would make *every* entry in the log that
/// large, including the zero-sized `PhotoEdit`.
SnapshotDelete { id: PhotoId, entry: Box<Snapshot> },

/// A snapshot renamed. `CopyRename`'s shape exactly.
SnapshotRename { id: PhotoId, n: u32, from: Option<String>, to: Option<String> },
```

⚠ **Taking a snapshot is deliberately not undoable**, and that follows the precedent already in
the code rather than inventing one: `create_virtual_copy` pushes no action while
`delete_virtual_copy` does. Creation is additive and reversed by deleting; deletion destroys the
only record of a state. It is also exactly consistent with the table above — *taking does not
push an undo entry* — so there is one rule rather than two.

⚠ Undo and redo of a rename call **one** function with the arguments swapped, so the two
directions cannot drift. That is the rule `CopyRename` already follows.

## The strip

A **resizable** bottom panel *inside* the right panel, allocated **before** the scroll area, so
it is pinned to the panel's bottom edge and the forty sliders scroll behind it. It is visible
under **every** tool — Edit, Crop, Masks and Presets alike.

⚠ **This is a deliberate departure from the exclusive-panel rule**, which says exactly one tool's
controls are on screen at a time. A snapshot list is *navigation*, not a tool: the point of it is
that you can always see and reach the other states of the photo you are working on. Making it a
fifth rail tool would hide it behind a click precisely when it is most wanted.

### Shape

```
┌────────────────────────────────┐
│ Snapshots (3)              [+] │   header, always drawn
├────────────────────────────────┤
│ Snapshot 1        27 Aug 14:32 │   ← rows scroll within the strip
│ Warm              27 Aug 14:41 │
│ Before the crop   27 Aug 15:02 │
└────────────────────────────────┘   ← drag this edge to resize
```

- **Default height ≈ four rows** (about 110 pt at the compact metrics). Minimum is the header row
  alone, so someone who wants the space back drags it away once and it stays gone. Maximum is half
  the right panel's own height, so it can never crowd out the adjustments entirely — and because
  that maximum is derived from the live panel height rather than stored, a stored height taller
  than a shrunken window is clamped rather than obeyed.
- **Rows always scroll within whatever height the strip has**, so the four-row figure is a default
  rather than a cap.
- A row is `label()` plus its timestamp in `theme::caption` style — the timestamp is shown for
  **named** snapshots too, since it is information the name does not carry.
- **Empty state:** the header and the `+` alone, with a one-line hint.

### Interaction

- **Left-click a row restores it.** `s.edits = snap.edits.clone()`, then `s.history.push`, then
  `s.request(..)` — the same shape a preset click already takes.
  - ⚠ **It must also clear `Session::selected_mask`.** That is an index into `edits.masks`, and a
    restored state has a different list; keeping the old index is a stale pointer. Every read site
    guards with `.get(i)`, so it is harmless *today* — but a mask added later would land at that
    index and appear selected without anyone having selected it. A photo switch already clears it
    for the same reason.
- **Right-click gives a context menu:** Rename, Delete, Make virtual copy. That matches the
  Library's photo context menu, which is the app's one context-menu idiom.
  - **Rename** turns the row's label into `name_edit`'s in-place field, autofocused. Enter
    commits; clicking away abandons — never a bare `lost_focus()`, which is the rule the copy
    rename field already follows. Typing the *default* label back in stores `None`, so the
    snapshot keeps following its number.
  - **Delete** removes the row and pushes `SnapshotDelete`. The status line says "Ctrl+Z to
    restore", as deleting a copy does.
  - **Make virtual copy** reuses `create_virtual_copy` with the snapshot's edits instead of the
    current ones, and seeds the new copy's name from the snapshot's name **when it has one** — an
    unnamed snapshot gives an unnamed copy, which then follows the file's name as any unnamed
    copy does. It does not disturb the open photo's edits.

### Persistence of the height

⚠ **`Settings.snapshot_strip_height` is a `u32` of points, not an `f32`.** `Settings` derives
`Eq`, which no float satisfies; and an integer makes a NaN height *unrepresentable* rather than
something to guard — NaN defeats `clamp` entirely, since it fails every comparison and comes back
unchanged. That is the reasoning already written into `library_cell`, verbatim.

⚠ **Written when the drag settles, not per frame.** A resize runs at 60 Hz and the settings file
is not a place to write sixty times a second — the `apply_view_demosaic` rule, which this project
has now cited a dozen times.

⚠ **`#[serde(default)]` at the field**, so a settings file written before this feature still
loads. `SettingsStore::load` is infallible, so one unparseable field silently reverts *every*
setting — the guard `a_settings_file_from_before_the_format_filter_still_loads` already exists for
exactly this and gains a sibling.

### Two egui hazards, both with known fixes in this codebase

⚠ **A resizable panel shrinks back every frame unless its content fills the resize axis.** egui
stores the panel's *content's* rendered rect as its size. `ui.set_min_height(ui.available_height())`
is what the filmstrip already does about it, and this needs the same.

⚠ **The two scroll areas are siblings, not nested.** The strip's list scrolls inside the bottom
panel; the sliders scroll inside the right panel's own area; the rects are disjoint, so the wheel
is never ambiguous. This must be said in the code, because it looks exactly like the nesting the
left panel's three inner scroll areas were **removed** for on 2026-08-27 — and it is not that.

⚠ **Each row needs an `id_salt` or a `push_id`.** egui's auto-ids shift with what was drawn before
them, and the strip's contents change as snapshots are taken and deleted — the hazard that broke
the rename field under virtualisation.

## What snapshots deliberately do not do

1. **They do not travel with a paste, a preset or a sync.** Those go through `Edits::merge_from`,
   which is `Edits`-only. A snapshot is a record of *this* photo's past; carrying it to another
   photo would be meaningless. Undoing a sync restores them for free, because `SyncAction::prior`
   already stores whole `Sidecar`s.
2. **Per copy, and a new copy does not inherit them.** `create_virtual_copy` builds a fresh
   `CopyEntry` through `set_edits`, so this needs no code — but it is a decision, not an accident.
3. **No automatic snapshots** — not on import, not on first edit. The backlog is explicit that
   Lightroom keeps both a browsable History panel and named Snapshots, and that we are building
   only the second, the explicit one.
4. **No "update with current settings."** Lightroom has it; it silently destroys the state you
   saved, and taking a new snapshot and deleting the old one does the same thing visibly.
5. **No Library badge and no filtering on snapshots.** Nothing outside Develop knows they exist.
6. **They are lost if the sidecar is lost**, like every edit in this application. They travel with
   the photograph, which is the point of sidecars.

## Testing

Beyond the ordinary round trips:

- **`Tool::ALL` is looped and the `+` button asserted findable under every one.** This pins the
  "always active" requirement, which is otherwise only a visual claim no test could see. It uses
  `test_ui::find` by label against the real panel, through the `test_app` fixture.
- **The height survives a photo switch and a restart**, and the minimum can never be dragged past
  — a strip dragged out of existence with no way back is the failure mode a resizable panel
  actually has.
- **A legacy sidecar loads with an empty snapshot list**, and a snapshot-bearing one round trips.
- **Numbering never reuses**: take three, delete the second, take a fourth — it is `Snapshot 4`.
- **Restoring pushes exactly one history entry**, asserted on `history.can_undo()` rather than on
  `Edits` — the correction the headless canvas tests already had to make, since the edits change
  for reasons other than the push.

### Sabotages to run, each predicted to be caught by exactly one test

| sabotage | must fail |
|---|---|
| drop `#[serde(default)]` from `CopyEntry::snapshots` | the legacy-sidecar test |
| make `is_neutral` ignore snapshots | the reset-does-not-delete-snapshots test |
| number by `count + 1` instead of `max + 1` | the numbering test |
| make restore skip `history.push` | the one-history-entry test |
| draw the strip inside the scroll area | *(predicted inert — recorded as such rather than papered over; no test can see pinning)* |
| draw the strip only under `Tool::Edit` | the `Tool::ALL` loop |

## Accepted caveats

1. **The strip costs permanent panel height** for photos that have no snapshots, unless dragged
   away. One rule, no exceptions — the alternative (a height that collapses when empty) is two
   facts that can disagree about how tall the strip is.
2. **No snapshot is visible outside Develop**, so a photo with ten saved states looks identical
   to one with none in the Library.
3. **Taking a snapshot is not undoable**, by the `create_virtual_copy` precedent. Deleting one is.
4. **A restore replaces everything**, including the crop and every mask — a snapshot is a whole
   state, not a preset with edit groups. That is the point, and it is the caveat most likely to
   surprise.
5. **No cap on how many a photo may hold.** A `Snapshot` is an `Edits` plus a few fields; a photo
   with fifty of them has a larger sidecar and a scrolling strip, and nothing else changes.

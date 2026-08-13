# Photo edit history surviving a photo switch

**Status:** spec, 2026-08-13.

## The problem

`History<Edits>` lives on `Session`, and a `Session` dies the moment another photo is
opened. So editing photo A, moving to B and coming back leaves A with no undo stack — even
though A's *edits* were restored faithfully from its sidecar. The state the user can see
survives the switch; the state that lets them take it back does not.

The gap is wider than it looks, because `ActionLog::forget_photo_edits()` compounds it. A
`PhotoEdit` entry is **anonymous** — it records only "the next undo pops the session's
history", with no way to say *which* photo's. It therefore has to be deleted on every switch,
or an undo would pop a different photo's stack and silently revert an edit the user never
made there. Two consequences follow:

1. Storing histories per path is not sufficient on its own. Restore A's stack without
   touching the log and Ctrl+Z still does nothing, because the entries that pointed at it
   were swept.
2. The sweep is already lossy in a second way. A `Batch` — one Ctrl+V that changed the open
   photo *and* several sidecars — has its `PhotoEdit` member torn out on the next switch,
   permanently stranding the sidecar half with no way to undo it.

## Decisions taken

| Question | Decision |
|---|---|
| Ctrl+Z after switching photos | **Undoes only the open photo.** Entries belonging to other photos are skipped, and become undoable again on return. No view jump, no decode, nothing changed off-screen. |
| How long a stack lives | **Every photo, for the session.** Unbounded map, cleared when the app closes. |
| Redo after editing a different photo | **Per photo.** A new action clears only the redo entries it can reach. |

### Why undo is scoped rather than chronological

Lightroom's own answer is the second option — strict chronological order, jumping back to the
photo an undo belongs to. It was rejected because a Ctrl+Z that yanks the view to a different
image and starts a decode is a large, surprising effect for a keystroke whose entire promise
is "put it back". The third possibility — reverting A's sidecar in the background without
switching — was rejected outright: it changes a photo the user cannot see, which is the exact
silent-divergence failure mode this codebase has been bitten by repeatedly.

Global actions (`PresetDelete`, `Sync`) are unaffected and still interleave chronologically
with whichever photo is open, because they never touch a live session.

### Why the stack is not capped

`BUFFER_CACHE_CAP` is 3 because a decoded buffer is 78–313 MB. A `History<Edits>` is a `Vec`
of a few-hundred-byte struct: 500 photos at 20 steps each is roughly 4 MB. The two caps have
nothing in common, and a cap here would be a constant nobody could justify from a
measurement — while the stack that silently expired is precisely the one the user wanted.

## Design

### 1. `app::edit_history::HistoryStash` — a stash, not a store

```rust
pub struct HistoryStash { map: HashMap<PathBuf, History<Edits>> }

impl HistoryStash {
    /// Keep `history` for `path`. Histories with nothing to undo or redo are dropped,
    /// so the map grows per *edited* photo rather than per visited one.
    pub fn stash(&mut self, path: PathBuf, history: History<Edits>);

    /// Take `path`'s history back, but only if it still describes `on_disk`.
    pub fn take_matching(&mut self, path: &Path, on_disk: &Edits) -> Option<History<Edits>>;

    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

`Session` keeps owning its `History<Edits>` exactly as today, so every `s.history.*` call
site is untouched. The stash is only where a history waits between sessions.

The alternative — making the stash authoritative and deleting `Session::history` — was
rejected. Every `s.history.push` sits inside a closure already holding `&mut self` through
`self.session`, so it would need a second simultaneous `&mut self` borrow. That is the egui
borrow shape this codebase has already paid compile errors for twice, and it buys no
behavioural difference.

Putting the history inside `Decoded`/`BufferCache` was likewise rejected: that cache is
capped at 3 for memory reasons, its values are behind an `Arc` and immutable, and a history
should outlive its buffer.

### 2. One rule invalidates a stale stack, checked at one moment

A sync, paste or preset applied to a multi-selection rewrites the sidecars of photos that are
**not open**. Their stashed histories then describe a state no longer on disk, and handing
one back would let an undo restore edits the sidecar no longer contains.

The invalidation is not distributed across the writers. `take_matching` returns the history
**only if `stashed.get() == on_disk`**, and drops it otherwise. That single check subsumes
every path that can cause the divergence — `sync::apply`, `apply_to_selection`, a preset
click, `Sync` undo, `Sync` redo — including any future writer that would otherwise have to
remember a rule. It is the same preference recorded for `render_max_dim`: make the
disagreement impossible to act on rather than testing for it at N sites.

`Edits` is `PartialEq` over `f32`, so a `NaN` compares unequal and the check fails toward
"discard and start fresh" — the safe direction, exactly as `PrefixKey` fails toward a cache
miss.

The **open** photo keeps its existing eager path: `reload_if_affected` already resets
`s.history` the moment a sync rewrites the sidecar underneath a live session. That stays, and
is the only eager invalidation.

### 3. `ActionLog` becomes one list with a per-entry `undone` flag

```rust
pub enum ActionKind {
    PhotoEdit { path: PathBuf },   // was a unit variant
    PresetDelete { name: String },
    Sync(Box<SyncAction>),
    Batch(Vec<ActionKind>),
}

struct Entry { kind: ActionKind, undone: bool }

pub struct ActionLog { entries: Vec<Entry> }
```

An entry is **reachable** from the open photo iff:

- `PresetDelete` / `Sync` — always; they are global and never touch a live session.
- `PhotoEdit { path }` — iff `path` is the open photo.
- `Batch(parts)` — iff **every** part is reachable.

The operations all take the open path (`Option<&Path>`; `None` when no photo is open, which
makes only global entries reachable):

| Method | Meaning |
|---|---|
| `undo(open) -> Option<ActionKind>` | the **last** reachable entry that is not undone; marks it undone and hands the caller a clone to reverse |
| `redo(open) -> Option<ActionKind>` | the **first** reachable entry that is undone; marks it done and hands back a clone |
| `push(kind, open)` | appends, then deletes every reachable **undone** entry (the new entry is not undone, so it is never its own victim) |
| `can_undo` / `can_redo` | the same searches, as a `bool` |
| `peek_undo(open) -> Option<&ActionKind>` | what the next undo would reverse, for the button label |

Restricted to any one photo's subsequence, the reachable done entries always form a prefix
and the undone ones a suffix, so `undo` and `redo` are exact inverses per photo while global
actions order chronologically against whichever photo is open. `push`'s targeted clear is
what implements the per-photo redo decision: editing B invalidates B's redo path and the
global one, and leaves A's alone.

With a single photo open throughout, this is behaviourally identical to today's two stacks —
`undo` pops the top, `redo` restores it, `push` clears the undone suffix — which is the
regression argument.

**Rejected:** keeping two stacks and filtering them. Taking a reachable entry out of the
middle of the undo stack and pushing it back on top scrambles chronological order across
photos, which becomes observable as a wrong undo order once you switch again. Repairing that
needs a monotonic sequence number on every entry; the `undone` flag makes the ordering
intrinsic instead, and removes the middle-removal entirely.

### 4. `forget_photo_edits()` becomes `forget_edits_of(path)`

The same sweep — drop `PhotoEdit` entries, strip them from batches, drop a batch left empty —
but for one path rather than all of them. Two callers:

- `reload_if_affected`, where the open photo's history was just reset.
- `finalize_session`, when `take_matching` refuses, so a discarded stack never leaves log
  entries pointing at a history that no longer holds those steps.

Because the sweep is no longer fired on every switch, a `Batch` now survives one intact and
becomes undoable again on return, instead of being permanently half-destroyed.

### 5. Wiring in `main.rs`

- `RustroomApp` gains `histories: HistoryStash`.
- `finalize_session` — the only site that assigns `self.session`, and so the only choke point
  needed — takes the outgoing session's history into the stash, then builds the incoming one
  from `take_matching(&path, &edits)`, falling back to `History::new(edits)` plus
  `forget_edits_of(&path)`.
- Both `actions.forget_photo_edits()` calls go; `reload_if_affected` uses
  `forget_edits_of(&open)`.
- Every `ActionKind::PhotoEdit` construction (`apply_to_selection`, the settled-drag site)
  carries the open photo's path.
- The toolbar's `can_undo` / `can_redo` / `peek_undo` pass the open path, so a disabled Undo
  button means "nothing to undo **here**" rather than lying about a foreign entry.

## Testing

`HistoryStash` is pure and needs no image fixtures:

- a stashed history comes back intact, with its undo *and* redo contents
- `take_matching` refuses when the on-disk edits differ, and the entry is consumed either way
- `take_matching` on an unknown path is `None`
- a history with nothing to undo or redo is not stashed

`action_log`'s existing tests adapt to the new signatures. New properties:

- `an_edit_to_another_photo_is_not_undoable_here`
- `returning_to_a_photo_makes_its_edits_undoable_again` — the headline property, and the one
  that fails against today's code
- `a_batch_waits_until_every_member_is_reachable`, and its companion that the batch is
  **intact** on return rather than stripped
- `a_new_action_clears_only_the_redo_it_can_reach`
- `undo_and_redo_are_inverses_with_two_photos_interleaved`
- `forget_edits_of_leaves_other_photos_alone`
- a global action still orders chronologically against the open photo's edits

Each new property is to be watched failing before the implementation lands.

## Accepted caveats

1. **Session-only.** Closing the app loses every stack. Persisting to the sidecar would
   change the file format, inflate every sidecar by its whole history, and raise a migration
   question for existing files — a much larger feature than the one deferred.
2. **The map is unbounded.** Justified above: it holds `Edits`, not pixels.
3. **A sync to a closed photo discards that photo's stack**, detected lazily when it is
   reopened. The sync itself remains undoable as a global action; what is lost is the
   photo-local history from before it.
4. **Ctrl+Z on B can never reach A's edit.** The direct consequence of scoping, and the point
   of it.
5. **The stash is keyed by path**, so moving or renaming a file between visits loses its
   stack — the same assumption sidecars already make.

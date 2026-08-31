# Undo for catalog deletions

**Date:** 2026-08-27
**Status:** design agreed, ready to plan.
**Asked as:** *"it should have a history, such that if one deletes something, e.g. a collection or
keyword, that can be reverted."*

## What this is

Four destructive catalog actions have no undo at all:

| action | what it destroys |
|---|---|
| Delete a smart collection | its rule |
| Delete a collection | its membership rows |
| Delete a keyword | ⚠ **its whole subtree** and every assignment of any of them |
| Forget a folder | the folder's photo rows, and with them **every rating, flag, label and keyword assignment** |

All four become undoable through the existing `ActionLog`, so Ctrl+Z reaches them in the same
chronological order it already reaches photo edits and preset deletions.

## The measurements that reframed the problem

⚠⚠ **The backlog's central claim is wrong, and probing it is what produced this design.** It said
*"restoring must preserve the ids, or every membership pointing at them still dangles."* Two
probes against real SQLite:

1. **Explicit-rowid re-insert works and does preserve ids** — so the stated approach is possible.
2. **But it collides.** Delete a collection, create another *before* undoing, and the new one takes
   the freed id (SQLite reuses `max + 1` without `AUTOINCREMENT`); the restore then fails with
   `UNIQUE constraint failed: collections.id`. ⚠ **That is the third instance of the
   high-water-mark bug class in one day**, after `next_snapshot` and `next_copy` — and here the fix
   would be an `AUTOINCREMENT` migration recreating four tables on live user data.

**What must actually be preserved is the *relationships*, not the ids.** A cascade deletes exactly
the dependent rows, so a snapshot of a deletion is **closed under references**: everything that
pointed into the deleted set is inside the snapshot. It can therefore be re-inserted with **fresh
ids**, rewriting each child's reference from an old→new map built in the same transaction. No
collision is possible, and no schema changes — which matters, because this project treats a
migration that has already run on someone's machine as unchangeable.

## Architecture

### One action, four snapshot shapes

```rust
/// What a catalog deletion removed, enough to put it back.
///
/// ⚠ Boxed in the `ActionKind` for the reason `Sync` and `CopyDelete` are: an inline payload would
/// make *every* entry in the undo and redo stacks that large, including the zero-sized `PhotoEdit`.
CatalogDelete { snapshot: Box<CatalogSnapshot> }
```

| action | snapshot |
|---|---|
| Smart collection | the row |
| Collection | the row + its membership photo ids |
| Keyword | the **subtree** (id, parent, name) + every assignment of any of them |
| Forget folder | the folder + its photos *with their marks*, plus captures, memberships and keyword assignments |

⚠ **Global in `reachable()`**, like `PresetDelete` and `Sync`. A catalog deletion is not scoped to
a photo, so it must be undoable from wherever you happen to be.

### Restore remaps, in dependency order

Folders, photos and keywords get fresh ids; every child row's reference is rewritten from the
old→new map. Collections and smart collections are single rows with nothing to remap. Keywords are
inserted **parent first**, because a child's `parent_id` must be remapped to an id that already
exists.

⚠⚠ **A restored row may point at something that has since gone** — a membership at a collection
deleted meanwhile, a keyword assignment at a keyword deleted meanwhile. Those rows are **skipped,
not fatal**. The restore puts back everything it still can, which is the rule `add_to_collection`
already states: *"the catalog is not authoritative and must never be a reason an action fails."*
The alternative is an undo that refuses entirely because one unrelated row moved.

⚠ **The whole restore is one transaction**, so a partial failure leaves the catalog exactly as it
was rather than half-restored.

### Undo does not reopen anything

`ActionKind::CopyDelete` reopens the copy it restores, and that is right there — it is a photo you
were looking at. This is different in two ways. Deleting the collection you are browsing already
changes the Library view, so yanking it back on Ctrl+Z would move you somewhere you did not ask to
go. ⚠ And it *could not* anyway: `LibrarySource::{Collection, Keyword, Smart}` each hold the **old**
catalog id, which the remap has invalidated. The panel refreshes; the view stays put.

### A confirmation before a keyword takes its subtree

⚠⚠ **Undo does not help if you never notice**, and deleting a keyword removes its whole subtree
and every assignment of any of them from one click, with nothing said. The confirmation names what
is about to go, in the shape `forget_folder`'s already does:

> Delete "Europe"? *This also deletes 3 keywords beneath it and removes the keyword from 47
> photos. Ctrl+Z restores it.*

⚠ **Shown only when the deletion removes more than the keyword itself** — descendants or
assignments. A leaf keyword nobody has used is an unsurprising deletion, and a dialog for it is the
false alarm the `banding_warning` reasoning already rejects: *a warning that fires on ordinary
practice trains people to dismiss the one that matters.*

⚠ **The counts come from the recursive CTE `keyword_members` already uses**, so nothing new has to
be written to walk the subtree.

⚠ **Deleting a collection stays unconfirmed**, deliberately. Its scope is not surprising — the
panel shows its member count beside its name — and it destroys only membership, which undo now
restores.

### No cap on the payload

A forgotten 830-photo folder serializes to a few hundred KB held in the undo log. `PresetStore`'s
trash caps at 20 because those are files on disk that accumulate; this is memory, and ten forgotten
folders is a couple of megabytes.

## Testing

The catalog is testable in memory (`Catalog::open_in_memory`), so all of this is headless:

- **Round trip per action**: delete, undo, and assert the rows are back — including the *marks* on
  restored photos, which are the thing a forget most regrettably destroys.
- **The keyword subtree survives**, parents and children, with the hierarchy intact after remapping.
- **⚠ The collision that motivated the remap**: delete a collection, create another (which takes the
  freed id), then undo. It must succeed. That is the test the explicit-rowid design would fail, and
  it is the reason for the whole approach.
- **⚠ A membership whose collection has since been deleted is skipped, not fatal** — the rest of the
  restore still lands. Asserted by count, not by absence of an error.
- **Redo re-deletes**, so the two directions stay symmetric.
- **The confirmation fires only when it should**: a keyword with descendants or assignments is
  confirmed, a bare leaf is not. ⚠ Both halves, since a dialog that never fires and one that always
  fires are equally wrong and only the pair of assertions distinguishes them.
- **A restore is atomic**: an injected failure part-way leaves the catalog unchanged.
- ⚠ **Not tested here**: that Ctrl+Z reaches these from the UI, and that the panel refreshes. Those
  are wiring, verified by hand with the app running.

## Accepted caveats

1. **Ids change on restore.** Anything holding a catalog id across an undo is stale — which is why
   the restore does not reopen the view. Nothing persists these ids, so nothing outside memory is
   affected.
2. **Undo is session-only**, like every other entry in `ActionLog`. Quitting loses it.
3. **A keyword deletion is confirmed; a collection deletion is not.** That asymmetry is deliberate
   — a keyword's subtree is invisible at the point of deletion, a collection's member count is not.
4. **No trash browser.** A deletion is recoverable through Ctrl+Z or not at all.
5. **A restored photo row is not re-scanned.** It comes back exactly as it was recorded, including
   a `last_seen` that is now old; the next scan of that folder corrects it.

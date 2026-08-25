# Virtual copies (M3.4) — design

**Date:** 2026-08-25
**Milestone:** Phase 3, M3.4 — the virtual-copies half.
**Scope:** several independent edit versions of one file, each a first-class entry in the Library.
**Snapshots are deliberately excluded** — see *Scope*.

## Scope

**In: virtual copies.** The same file appears several times in the browser, each with its own
edits, each selectable, syncable and exportable. That is exactly what the roadmap's own test asks
for — *"two virtual copies of one RAW export differently"*.

**Out: snapshots**, as a later slice. Named saved states *within* one photo are a different feature:
different storage, different UI, and they need an answer to how they relate to `History<Edits>` and
the undo stack — a design question rather than more of the same work.

⚠ **This leaves M3.4 🟡 in the roadmap**, like M3.2 and M3.3. Named up front rather than discovered
at the end.

## The shape of the problem

**The whole feature is an identity change.** `PathBuf` is currently a photo's identity in six
places: the sidecar filename, `BufferCache`, `HistoryStash`, `Session.path`, sync targets and
export naming. A copy is the same file with different edits, so identity must become
`(path, copy)`. Everything else here is small by comparison.

Two consequences fall out, one good and one dangerous:

- **Copies share the decoded buffer for free.** `BufferCache` is keyed by path, and **stays** keyed
  by path. That *is* the roadmap's "at no storage cost" — it needs no work, only a decision not to
  change it.
- **The sidecar format has to migrate**, and this project has already been bitten by exactly the
  mechanism involved. See *Storage* below.

## Identity

```rust
pub struct PhotoId {
    pub path: PathBuf,
    /// 0 is the original; copies are numbered from 1. This same number is what the UI shows and
    /// what the export filename suffixes — see below.
    pub copy: u32,
}
```

⚠⚠ **ONE NUMBER, USED EVERYWHERE.** The identity number *is* the displayed number *is* the export
suffix: copy 1 shows as `DSCF6223 / 1` and exports as `DSCF6223_1.jpg`. The original is 0 and shows
no suffix at all.

This differs from Lightroom, which calls the original "Copy 1" and the first virtual copy "Copy 2"
— and the mockup this design was chosen from followed Lightroom, showing `/ 2` and `/ 3`. That is
**deliberately not** what is built. A display number offset from the identity number is two facts
that can disagree, which is the failure mode this project has paid for repeatedly (`render_max_dim`
derived rather than stored, `fit_image_size` likewise, `Session::geometry_mode` deleted for the same
reason). One number cannot drift from itself.

⚠ **Copy numbers are explicit, stable and never reused.** A new copy takes `max + 1`; deleting
copy 2 of 3 does **not** renumber copy 3. Renumbering would silently change a copy's export
filename and invalidate its `HistoryStash` key — a photo's undo stack would attach to a different
version than the one on screen.

## Storage

One sidecar per file, unchanged in name (`photo.RAF.json`), now holding every version:

```json
{
  "copies": [
    { "copy": 0, "name": null,  "edits": { … } },
    { "copy": 1, "name": "B&W", "edits": { … } }
  ]
}
```

### ⚠⚠ Migration will NOT use `#[serde(untagged)]`

Every existing sidecar is a bare `Edits` object. The obvious implementation — an untagged enum of
`{ NewEnvelope, LegacyEdits }` — is precisely the trap CLAUDE.md records from M3.1b:

> `an_existing_linear_sidecar_still_loads` asserted the *variant* and not the *payload*. Under
> `#[serde(untagged)]` serde reads `LinearGradient` from the whole object, and its container-level
> `#[serde(default)]` makes the unknown key be **ignored** and every field default.

⚠⚠ **CORRECTION, established by sabotage during implementation: the danger needs TWO things, not
one.** This design originally claimed untagged alone would destroy the data. Measured, it does not:
`Sidecar::copies` is a *required* field, so a legacy bare-`Edits` object fails to match the envelope
variant and correctly falls through. Untagged alone passed all 15 tests.

It becomes silent data loss only when `#[serde(default)]` is **also** added to `Sidecar` — the
obvious "forward compatibility" move — at which point every legacy sidecar matches the envelope with
an empty copy list and every edit is discarded, with a green suite. Verified: that combination fails
`a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact`.

So there are two independent guards, and both are kept because either alone suffices and neither is
obvious to the next reader: **explicit dispatch** in `load_all`, and the deliberate **absence** of
`#[serde(default)]` on `Sidecar`. Both carry comments saying so.

**Instead: parse to `serde_json::Value`, test explicitly for the `copies` key, and dispatch.** No
inference. And the test asserts the **payload** — that a legacy sidecar's exposure value arrives
intact on copy 0 — not merely that some variant matched.

### Naming

Each copy carries an optional `name`. ⚠ **The name is envelope metadata, not an `Edits` field**,
and that is load-bearing rather than tidy:

- An `Edits` field would need an edit group, and would then **travel with a paste, a preset or a
  sync** — pasting a B&W recipe onto another photo would rename it "B&W".
- It would be cleared by "Reset adjustments", which resets every group the panel shows.
- `History<Edits>` would undo it, which sounds convenient until a rename and a slider drag share
  one undo step.

Consequence, handled deliberately: because the name is outside `Edits`, `History<Edits>` cannot
undo it, so renaming gets its own `ActionKind` — see *Renaming*.

## Library

`entries` becomes one row per `(file, copy)` rather than per file, ordered with a file's copies
adjacent and ascending. `thumbs`, `Selection`, `ThumbLoader` and the derived `visible` list stay
index-parallel to it, exactly as now.

⚠ **This is the riskiest part of the change and it lands on the worst-covered code.**
Sparse-versus-dense index confusion is already this project's third-most-repeated bug — the crop
feedback loop, `render_max_dim`, and the Library grid's row breaks were all the same mistake, and
the last of those was found only by toggling a filter in real use. The mitigations: `visible`
remains the single derived list every consumer iterates, and the grid keeps counting row breaks by
**cell position, not entry index** (`the_grid_breaks_rows_by_position_not_by_entry_index` already
pins that).

⚠ **Opening a folder must now read every sidecar** to know how many copies each file has — ~822
small reads on the test folder where it currently does zero. Expected to be milliseconds, but it
**will be measured, not assumed**. If it is not, the fallback is doing it on the loader thread
rather than inline, which is where thumbnail work already happens.

### Thumbnails, and an accepted limitation

⚠ **Every copy of one file shows a pixel-identical thumbnail**, because Library thumbnails are the
camera's embedded JPEG rather than a render of the edits. A black-and-white copy appears in colour,
distinguishable only by its badge and name.

Accepted rather than fixed here. **Edit-accurate thumbnails are a separable feature** — render per
entry, cache it, invalidate on every edit — that does not need copies to exist and fixes every
ordinary photo too. Pulling it in would mean two large simultaneous changes to the Library. It is
recorded in `docs/BACKLOG.md` with this motivation attached.

## Renaming

An inline field in the **Develop panel**, beside where the filename already shows in its footer:

- Clicking into it selects the whole name.
- Typing replaces it.
- **Enter, or clicking anywhere else, commits.** No dialog, no confirmation.
- **Ctrl+Z undoes the rename.**

⚠ Undo needs `ActionKind::Rename { path, copy, from, to }` in the existing `ActionLog`, because the
name is not in `Edits`. It is scoped to its photo by the same `reachable()` rule `PhotoEdit` uses,
so Ctrl+Z on a different photo will not reach it.

⚠ The rename field takes keyboard focus, which the existing `ctx.memory(|m| m.focused())` gates
already account for — arrow navigation and Ctrl+C/V are suppressed while a text field has focus.
This adds one more such field rather than a new mechanism.

## Creating and deleting

Both live in the existing **Photo** menu section:

- **Create virtual copy** — copies the *current* photo's edits into a new copy at `max + 1`, and
  opens it. Copying current state rather than starting neutral is what makes "try a variation"
  one keystroke.
- **Delete virtual copy** — removes it from the envelope, undoable. ⚠ **Copy 0 cannot be deleted;
  it is the file.** The menu item is disabled on an original rather than hidden, so the rule
  explains itself.

⚠ Neither shortcut may be a **native** accelerator on macOS. `Shortcut::safe_as_native_accelerator`
already restricts that list to ⌘O, ⇧⌘O, ⌘E, ⌘, , ⌘[ and ⌘] — a native accelerator is consumed
before winit sees the key, which would break typing in the new rename field. These use egui's
focus-gated handling and show their chord in the label, like every other unsafe chord.

## Export

`DSCF6223_2.jpg` for copy 2; copy 0 keeps the plain stem.

⚠ **This is a second numbering scheme alongside one that already exists, and the separators are
different on purpose.** `dest_for` already produces `stem-N.ext` for **Keep Both** on a collision,
where `N` is "the first free slot in the destination folder". The two numbers mean different
things:

| | meaning | when assigned |
|---|---|---|
| `_N` | virtual copy N | a property of the photo, stable forever |
| `-N` | Nth file wanting this name | a property of the destination, assigned at export |

They compose: copy 2 exported twice into one folder gives `DSCF6223_2.jpg` then
`DSCF6223_2-1.jpg`. **The existing `-N` behaviour is not changed** — doing so would alter export
filenames for people who have no virtual copies at all.

Without `_N`, every copy would collide on every export and `batch_export`'s collision prompt would
fire once per copy, turning a correctness guard into noise.

## Sync

Sync targets become `PhotoId`s. ⚠ Two consequences:

- Writing a target is now **read-modify-write of the whole envelope**, not a whole-file overwrite:
  load every copy, replace one, save all. Two targets in the same file are two such cycles, which
  is safe because export and sync are single-threaded.
- `SyncAction`'s `prior` must snapshot **the whole file's copy list**, not one copy's `Edits`, or
  undoing a sync that touched two copies of one file would restore only one of them.

## Testing

- **The migration test asserts the payload.** A legacy bare-`Edits` sidecar must load as exactly
  one copy, numbered 0, unnamed, with its `exposure` (and one other non-default field) intact —
  not merely "some variant matched". ⚠ Sabotage by switching the dispatch to `#[serde(untagged)]`
  and confirm it fails, since that is the exact failure being guarded against.
- **Round trip**: envelope → JSON → envelope preserves numbers, names and edits.
- **Numbering is stable across a delete**: create 1 and 2, delete 1, create again — the new copy is
  3, never a reused 1 or 2.
- **`PhotoId` keying**: two copies of one path get different history stashes and the *same*
  buffer-cache entry. The second half is the storage claim and must be asserted, not assumed.
- **The name never transfers**: `merge_from` with every group selected leaves the target's name
  alone. ⚠ This is the test that would catch someone "helpfully" moving the name into `Edits`.
- **Export naming**: copy 0 plain, copy 2 as `_2`, and `_2` colliding falls back to `-1` — pinning
  that the two schemes compose rather than interfere.
- **Library layout**: entries for a file are adjacent and copy-ordered; `visible` and `Selection`
  survive a filter toggle with copies present.
- ⚠ Every new test gets the sabotage step, with a compile check first.

## Plans

Two, following M3.1a, M3.3 and M3.2:

1. **Storage and identity** — `PhotoId`, the sidecar envelope and its migration, export naming,
   `HistoryStash`/`BufferCache` keying, sync. Provable headlessly, and the migration is the part
   that must not be wrong.
2. **The Library and the UI** — entries per copy, the badge, create/delete, the inline rename field
   and its undo.

## Accepted caveats

1. **Copies share a thumbnail** — badge and name only; edit-accurate thumbnails backlogged.
2. **Snapshots are not included.**
3. **Folder open reads every sidecar**, where it previously read none.
4. **Copy numbers are never reused**, so a heavily-churned photo can reach `_47` with three copies.
   The alternative — renumbering — silently changes export filenames and breaks history keys.
5. **Two numbering schemes in export filenames** (`_N` identity, `-N` de-duplication).
6. **A copy is not a file**, so nothing outside Rustroom knows it exists; the sidecar is the only
   record.

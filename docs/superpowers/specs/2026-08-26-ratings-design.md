# Ratings, flags and colour labels (M4.2a) — design

**Date:** 2026-08-26
**Milestone:** Phase 4, M4.2 — the culling half.
**Scope:** 1–5 stars, pick/reject flags and colour labels, per virtual copy, set from the keyboard.
**Keywords and EXIF/IPTC are excluded** — see *Scope*.

## Scope

⚠⚠ **The roadmap's M4.2 is three unrelated subsystems and was decomposed before designing.** Its
bullet reads *"EXIF/IPTC display + edit; star ratings, flags, color labels; hierarchical
keywords"*.

**In: ratings, flags and colour labels** — the smallest piece, the highest daily value, and the
thing that makes the catalog pay off. You cannot cull without them.

**Out, as its own slice: keywords** — a keywords table, a join, hierarchy and an assignment UI.
Largely independent of ratings, and it will reuse the schema-migration path this slice exercises.

**Out, to `docs/BACKLOG.md`: EXIF/IPTC display and edit.** ⚠⚠ **Not effort — a design conflict.**
*Display* is easy and we surface almost nothing today. *Edit* means **writing** metadata, and there
are only two places to write it:

- **Into the RAW file**, which violates the thing Rustroom is built on. "Originals are never
  modified" is a cross-cutting attribute in `01-lightroom-capabilities`, and the whole sidecar
  design exists to honour it.
- **Into an XMP sidecar**, which is what Lightroom does — but we already have a JSON sidecar, so
  that is two sidecars per photo with overlapping purposes, and it reopens the XMP-interop question
  `02-architecture-and-tech` deliberately parked.

That is a decision, not a task. Backlogged with the reasoning, as creative LUTs and snapshots were.

## Storage

Three columns on the existing `photos` table, added by a **version 2 migration**:

```sql
ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;  -- 0..=5, 0 = unrated
ALTER TABLE photos ADD COLUMN flag   INTEGER NOT NULL DEFAULT 0;  -- 0 none, 1 pick, -1 reject
ALTER TABLE photos ADD COLUMN label  INTEGER NOT NULL DEFAULT 0;  -- 0 none, 1..=5 colour
```

⚠ **This is the first migration to run against a database that already holds real user data**, so
it is the part to be careful with. The `migrate` function is already forward-only and gated on
`from < n`; version 2 is a new gate, never an edit to version 1 — **a migration that has already
run on someone's machine cannot be changed.**

⚠ `NOT NULL DEFAULT 0` means existing rows acquire "unrated" without a backfill, and `0` is the
neutral value for all three, so "no mark" needs no special case anywhere.

## Per copy, not per file

Marks belong to `(file, copy)` — the row `photos` already keys on. A rating describes **the image
you made**, so a black-and-white copy can be rated separately from its colour original, which is
why copies exist at all.

⚠ **Accepted incoherence: rejecting a frame means rejecting each of its copies.** Reject arguably
describes the *file* rather than a version. Splitting the rules — stars per copy, reject per file —
was considered and rejected: two rules to explain, and a grid showing a rejected badge on rows
whose stars differ. Lightroom is per-virtual-copy for all three, which is evidence the incoherence
is more theoretical than felt. In practice you cull before making copies.

## Keys

⚠⚠ **`1`–`4` are already bound to the rail tools**, which is exactly where Lightroom puts stars.
Resolved by making the digits **context-dependent by view**:

| key | Library | Develop |
|---|---|---|
| `0`–`5` | set star rating (`0` clears) | `1`–`4` select a rail tool |
| `P` / `X` / `U` | pick / reject / unflag | — |
| `6`–`9` | colour label | — |

The rail only exists in Develop and culling happens in the grid, so nothing is lost either way. The
two views already diverge on keyboard handling — arrows browse in Library and fine-tune sliders in
Develop.

Rejected: **different keys for stars** (nothing else is muscle memory; every photo tool uses
digits, so this trades a real collision for an unlearnable binding) and **moving the rail** (breaks
a shipped binding for an unshipped feature).

⚠⚠ **Bare keys are stolen from text fields, and this project has recorded that twice.** The
Library now has an editable filename field *and* a new-collection field. The ratings handler goes
behind the **existing** `memory().focused()` gate that already guards the grid's arrow navigation —
the same mechanism, not a second one. A `5` typed into a collection name must not rate a photo.

## Applying to a selection

Marks apply to **the whole selection**, exactly as a paste, a preset and a sync already do. This is
the established rule for anything that acts on photos, and culling is where it matters most —
select a run of frames, press `X`.

## Showing them

- **Grid and filmstrip:** stars and flag as a small badge under the thumbnail, scaled with the
  cell like the caption already is.
- ⚠ **Nothing in Develop.** The panel is for the photograph, and adding a rating widget there would
  make it a second place to set something the keyboard already handles — the "two homes" mistake
  the Presets panel was removed for.

## Filtering

A minimum-star filter in the Library toolbar, composing with the existing format filter and sort
orders. That satisfies M4.2's own "Done when" — *"rate/keyword images and have it persist and drive
filtering"* — for the ratings half.

⚠ Filtering is applied through `Library::refilter`, the single derived-`visible` path every
consumer already iterates. **Not a second filtering mechanism**: sparse-versus-dense index
confusion is this project's third-most-repeated bug, and `visible` staying the one derived list is
the mitigation that has held.

## Plans

One. This is smaller than the catalog foundation was, and splitting storage from UI would leave a
plan whose only visible output is a schema column.

## Testing

- **The version 2 migration runs on a version 1 database and preserves its rows** — the first
  migration to meet real user data.
- **Marks survive a rename and a `Reindex` renumbering**, because they live on the row that moves.
- **Setting a mark on a selection sets it on every member**, not just the active photo.
- **The focus gate**: a digit typed into a text field must not rate anything.
- **Filtering composes** with the format filter rather than replacing it.
- ⚠ Every new test gets the sabotage step with a **compile check first**, and a sabotage that
  changes no behaviour is not evidence — that happened once already this milestone.

## Accepted caveats

1. **Rejecting a frame means rejecting each copy** — see above.
2. **No auto-advance** after rating. Lightroom offers it; it is a preference, and adding one
   before anyone has asked is speculative.
3. **Colour labels are numbers, not names.** Lightroom lets you name them; that is a settings
   feature with no bearing on the storage.
4. **No "rejected photos" cleanup action** — nothing deletes files, which stays true.
5. **Marks are catalog-only**, so they do not travel with the photos and are lost if the catalog
   is deleted. That is the split decided in M4.1: lose organisation, never work.
6. **No filtering by flag or label** in this slice, only by minimum stars. The others follow the
   same path and are additive.

# The catalog database (M4.1) — design

**Date:** 2026-08-26
**Milestone:** Phase 4, M4.1 — the catalog half's foundation.
**Scope:** one invisible SQLite catalog, plus manual collections.
**Smart collections, ratings and keywords are deliberately excluded** — see *Scope*.

## Scope

**In:** a single SQLite catalog, populated automatically when a folder is opened, and **manual
collections** on top of it.

**Out:** **ratings, flags and keywords** (M4.2) and **smart collections and search** (M4.3). A
smart collection needs a query language, which is a design question rather than more of the same
work.

⚠⚠ **M4.1's foundation has no visible payoff on its own, and that is stated up front rather than
discovered at the end.** Folder open already costs 4.6 ms of sidecar reads for 830 files, so a
database makes nothing faster today. Its value is entirely in what it enables. This is the same
shape as colour-management C1, whose acceptance criterion was *"the render is unchanged"*.
**Collections are what make this milestone visible**, which is why they are in scope.

⚠ The roadmap's M4.1 bullet reads *"images, folders, metadata, edits, keywords, collections"* —
that spans M4.1 through M4.3. This spec scopes it to the schema, the plumbing, and collections.

## The decisions, and why

### 1. Split by kind: edits stay in sidecars, library metadata lives only in the catalog

Three positions were considered. **Sidecars authoritative with the DB as a rebuildable index**
forces ratings and keywords into the sidecars too, bloating a per-photo file with library concepts.
**DB authoritative with sidecars as an export** is Lightroom's model and makes the catalog a single
point of failure — the most-complained-about part of Lightroom. **Dual-write** gives two sources
that can disagree.

Chosen: **split by kind.** An edit describes the photograph and belongs beside it; a rating
describes your relationship to it and only means anything inside a library. Two stores, so "where
does this live" is a question for every future field — but it never has two answers for the *same*
field, which is what distinguishes this from dual-write.

⚠ The property this preserves: **delete the catalog and you lose organisation, never work.**

### 2. One invisible catalog — not catalog files

⚠⚠ **This reverses the direction the discussion was initially leaning, and the scenario that
settled it is worth keeping.** With a catalog per folder, three shoots in `2026-08-holiday/`,
`2026-09-wedding/` and `2026-10-portraits/` are three databases — and *"show me every 5-star frame
this year"* becomes impossible. **The single thing a DAM exists for is defeated by the default
behaviour**, and getting a unified library would require deliberate ceremony while the frictionless
path produced permanent fragmentation. That trap only becomes visible a year in, when it is
expensive to undo.

Supporting evidence: Lightroom users overwhelmingly run **one** catalog, and Adobe's own guidance
discourages multiple ones for exactly this reason.

⚠ What a folder-local catalog would actually have bought, honestly: ratings and keywords travelling
with the folder. Under decision 1 the *edits* already travel, so the portable part is much smaller
than it first appears. Against that, a folder-local catalog cannot be created on a read-only volume
and fragments search permanently.

**Consequence: no import step, no catalog picker, no catalog concept in the UI at all.** Opening a
folder works exactly as it does today.

## Storage

`<config>/rustroom/catalog.db`, beside `settings.json` — so it inherits the platform-correct
location `dirs` already provides (`~/Library/Application Support` on macOS, **not** `~/.config`).

**`rusqlite` with the `bundled` feature.** ⚠ That compiles SQLite from source, a C dependency this
project has been glad to avoid elsewhere — but `bundled` means **no system library, no pkg-config
entry, no CI change**, which is strictly fewer build prerequisites than libraw already imposes.
Linking the system sqlite instead would add a fourth entry to a `PKG_CONFIG_PATH` line that is
already three deep.

## Schema

```sql
folders(id, path UNIQUE, last_seen)
photos(id, folder_id, file_name, copy, size, mtime,
       UNIQUE(folder_id, file_name, copy))
collections(id, name UNIQUE, created)
collection_photos(collection_id, photo_id, added,
                  PRIMARY KEY(collection_id, photo_id))
```

Version in `PRAGMA user_version`.

- ⚠ **No `rating` or `keywords` column.** That is M4.2, and adding a column is a one-line
  migration. Including it now would be schema for something nothing reads.
- ⚠ **`size` and `mtime` *are* here**, because they are what a future re-link of a moved file
  needs and they cost nothing to record during a scan. Nothing reads them yet, and that is
  deliberate — the alternative is discovering later that the data was never captured.
- ⚠⚠ **A scan never deletes photo rows.** An unplugged drive or an unmounted share would otherwise
  silently empty every collection referencing it. Rows persist; `last_seen` records what was
  present.

## Identity, and the integration that matters most

`PhotoId { path, copy }` maps to `(folder, file_name, copy)`.

⚠⚠ **Rustroom renames files itself, and `Reindex` renumbers copies. Both must update the catalog
in the same operation**, or a collection quietly loses a member. This is the highest-risk
integration in the milestone, and it is the same class as the rule virtual copies already needed
twice: *the stash and the log must let go together*. A catalog row and the file it names are two
facts that must move as one.

## Collections

Manual only. A photo may be in many; membership is explicit; deleting a collection never touches
photos or files.

## Plans

Two, following the precedent set by virtual copies, colour grading and lens corrections:

1. **The catalog foundation** — crate, schema, migration, folder sync on open, and the
   rename/reindex integration. Entirely headless, which is where the data-loss risk lives.
2. **Collections** — the panel, add and remove, filtering the grid.

## Testing

- **The migration is the dangerous part.** An existing user has no `catalog.db`; a future one will
  have an older schema. Opening must create, and must upgrade, without losing rows.
- **Rename and reindex keep collection membership** — asserted directly, because this is the one
  place a silent loss can occur.
- **A scan of a folder whose files have vanished keeps its rows**, so an unmounted drive does not
  empty a collection.
- **A photo opened twice does not become two rows** — the `UNIQUE` constraint asserted through the
  API, not just declared.
- ⚠ Every new test gets the sabotage step, with a **compile check first**: a sabotage that fails to
  compile looks exactly like one that passes, and a matcher that greps failure lines by bare test
  name finds nothing because cargo prints module paths. Both traps have cost this project a full
  round of meaningless results.

## Accepted caveats

1. **The library accumulates every folder ever opened**, including accidents. "Forget this folder"
   is deferred until it actually annoys.
2. **No re-linking of moved files** — a moved photo appears as new, and its collection membership
   stays with the old row. The columns to fix this later are recorded now.
3. **No import step**, so there is no moment at which to apply presets or metadata on ingest.
4. **One catalog**, so no separation between client and personal work — collections are the answer
   if that is ever wanted.
5. **`bundled` SQLite adds a C compile** to the build.
6. **Plan 1 is invisible.** If you can tell it shipped, something is wrong.

# Keywords (M4.2b) — design

**Date:** 2026-08-26
**Milestone:** Phase 4, M4.2 — the keywords half. Closes M4.2 with M4.2a's ratings.
**Scope:** hierarchical keywords, assigned per file, browsable and filterable.

## The two decisions, and why

### 1. Hierarchy from the start, not flat

⚠⚠ **Because flat → hierarchical is not a migration you can automate.** Nothing can infer that
`Paris`, `Lyon` and `France` form a tree; the user would have to rebuild their taxonomy by hand.
That is the worst kind of upgrade — one imposed on someone who has already done the work.

Rejected: **flat now with an unused `parent_id`**. That is the pattern used for `size`/`mtime`,
and it was justified *there* because a scan was capturing data that would otherwise be lost
forever. Here nothing is lost by adding the column later — it is speculative schema that does not
spare the user the manual re-parenting, which is the actual cost.

⚠ The payoff lands in **M4.3**: searching `Europe` and finding photos tagged `Paris` is most of
why keywords beat collections.

### 2. Per file, not per copy — and this differs from ratings on purpose

Ratings are per **copy**, because a rating describes *the image you made* and a black-and-white
version can be better than the colour one. **A keyword does not work that way.** A B&W copy of a
photo taken in Paris is still a photo taken in Paris; keywords describe *what is in the picture*,
which is identical across every copy of a file.

So `photo_keywords` is keyed on **`(folder_id, file_name)`**, not on `photos.id`.

⚠⚠ The alternative — per copy, consistent with ratings — breaks in a way you would hit quickly:
tag copy 0 "Paris", then search Paris while browsing a collection containing copy 1, and **copy 1
does not match**. Fixing that needs "assigning applies to every copy" *plus* "a new copy inherits
its source's keywords" — two rules that can drift apart. Keying on the file makes the drift
**unrepresentable**, the same reasoning that keeps `BufferCache` on the path while `HistoryStash`
is on the `PhotoId`.

⚠ Accepted cost: **two different attachment rules in one schema.** Marks hang off `photos.id`;
keywords hang off the file. That is honest rather than tidy — they describe genuinely different
things — and a comment goes on **both** tables so nobody unifies them later.

## Schema — version 3 migration

```sql
CREATE TABLE keywords (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL,
    parent_id INTEGER REFERENCES keywords(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX keywords_unique ON keywords(COALESCE(parent_id, 0), name);

CREATE TABLE photo_keywords (
    folder_id  INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    file_name  TEXT NOT NULL,
    keyword_id INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
    PRIMARY KEY (folder_id, file_name, keyword_id)
);
```

⚠⚠ **The `COALESCE` index is load-bearing, not decoration.** A plain `UNIQUE(parent_id, name)`
would allow **two root-level "Paris" keywords**, because SQLite treats `NULL`s as distinct in a
unique constraint. The result is silent duplicates that look identical in the tree. The expression
index closes it, and it needs its own test because nothing else would notice.

⚠ Version 3 is a **new gate**, never an edit to versions 1 or 2 — a migration that has already run
on someone's machine cannot be changed. This is the second time that rule applies to a database
holding real user data.

## Deleting a keyword cascades to its children

Deleting `Europe` deletes `France` and `Paris` with it. Destructive and deliberate: the
alternative — re-parenting orphans to the root — silently flattens a taxonomy you built, which is
worse than an obvious deletion. The panel names what it is about to remove.

## Implied parents are derived, never stored

Searching `Europe` finds photos tagged `Paris` through a recursive CTE at query time.

⚠ Storing the implication instead would mean rewriting every affected row whenever a keyword is
re-parented — two facts that can disagree, the failure `render_max_dim` and `Session::geometry_mode`
were each fixed for by deriving rather than storing.

## Renames must move keyword rows

⚠⚠ `photo_keywords` is keyed by file name, so **`rename_photo` must update it in the same
operation.** This is the third instance of that integration, after `photos` itself and (via row id)
collection membership. A rename that forgot it would silently strip a file's keywords.

## UI

The Library's existing left panel gains a **second section**, not a second panel: **Collections**
and **Keywords**. The keywords section mirrors the collections one exactly — a tree with counts,
`+` to assign the selection, click to browse.

⚠ That consistency is the point. Two panels doing the same kind of thing must not grow two
different idioms.

Browsing by keyword reuses `collections::scan_of`, and `LibrarySource` gains a `Keyword` variant,
so the grid, filmstrip, format filter, sort orders and star filter all work unchanged.

## Testing

- **The v3 migration runs on a v2 catalog and preserves its rows** — folders, photos, collections
  and marks.
- **Two root keywords of the same name are refused**, which the `COALESCE` index is the only thing
  preventing.
- **Two keywords of the same name under *different* parents are allowed** — the discriminating
  other half.
- **A keyword applies to every copy of its file**, asserted directly, since that is the decision
  the schema shape exists for.
- **Deleting a parent removes its children and their assignments.**
- **A renamed file keeps its keywords.**
- **The recursive query finds a child's photos from a parent.**
- ⚠ Every new test gets the sabotage step with a **compile check first**, and a sabotage that
  changes no behaviour is not evidence — that has happened twice in this phase.

## Scope — excluded

- **Keyword suggestions** ("you often tag these together") — needs usage data and a heuristic, and
  it is polish on a feature that does not exist yet.
- **Renaming or re-parenting** a keyword after creation. Additive; the schema supports it.
- **Keyword export to IPTC** — that is the backlogged metadata-writing conflict, not this.

## Accepted caveats

1. **Deleting a parent deletes its children**, and their assignments with them.
2. **No drag-and-drop** in the tree; a parent is chosen when the keyword is created.
3. **Per file**, so one copy cannot be tagged differently from another — by design.
4. **Catalog-only**, like marks: lost with the catalog, and it never travels with the photos.
5. **No renaming or re-parenting** once created.

# Smart collections — M4.3c

**Date:** 2026-08-27
**Status:** plan 1 built 2026-08-27; plan 2 (the editor, and multi-select dates) outstanding
**Slice of:** M4.3, and the one that **closes** it

## What this closes

M4.3's own "Done when" is *"a smart collection auto-populates from a rule at 10k-image scale with
acceptable performance."* Everything before it made photos findable one axis at a time; this makes
the axes compose and lets the result be saved.

## ⚠⚠ A correction: `LibraryFilter` is NOT the rule representation

It was claimed twice during M4.3a/b that `LibraryFilter` "already **is** the rule representation
smart collections need". **That is wrong**, and the reason is visible in the type split:
`Collection`, `Keyword` and `Period` are `LibrarySource` **variants**, not `LibraryFilter` fields.

So a saved `LibraryFilter` cannot express *"in my Portfolio collection, show me July"* — the very
query this milestone exists for. The filter half of a rule is free; **the source half does not
exist yet.**

## Two findings that shaped the architecture

### ⚠⚠ 1. `text` cannot be evaluated in SQL

It matches `entries::shown_name`, which reads a copy's own name from the **sidecar**, not from the
catalog. A pure-SQL rule engine is therefore impossible without duplicating copy names into the
database — which would be two facts that can disagree, the shape this project has been bitten by
repeatedly.

### ⚠ 2. That pushes evaluation in-memory, and the measurement says it is affordable

CLAUDE.md records reading every sidecar at **4.6 ms for 830 files** (0.006 ms/file) — about **60 ms
at 10,000**, plus one catalog query. So a smart collection loads All Photos and evaluates the tree
per entry.

**The payoff: the evaluator is one pure function** over the facts `refilter` already has, plus
collection and keyword membership. Testable exhaustively with no database and no GUI — which is
what a boolean engine needs most.

⚠ Accepted: a smart collection matching five photos still builds ten thousand entries first. A
SQL pre-narrowing pass is possible later for the criteria the catalog *can* evaluate, and is
deliberately not attempted now — measure before optimising.

---

## The rule

### ⚠⚠ A tree in storage, one level in the editor

```rust
pub enum Expr {
    All(Vec<Expr>),
    Any(Vec<Expr>),
    Not(Box<Expr>),
    Is(Criterion),
}
```

The editor ships Lightroom's model — `Match [all | any | none] of:` over a flat list — while the
**stored shape is a real tree**. Two reasons: the serialized form never has to change when nesting
arrives, only the editor grows; and a hand-written nested rule in the JSON already works.

⚠ The editor producing a strict subset of what the model allows is **not** a two-facts-that-disagree
problem — it is a subset, and the evaluator is the single authority.

### Criteria — the union of today's sources and today's filters

| criterion | evaluated from |
|---|---|
| `Folder(PathBuf)` | the entry's path |
| `Collection(i64)` | membership map |
| `Keyword(i64)` | membership map, **including descendants** |
| `Period { year, month, day }` | `Capture::taken` |
| `Text(String)` | `entries::shown_name` |
| `Rating { op, value }` | `Marks` |
| `Flag(Flag)` / `Label(u8)` | `Marks` |
| `Camera(String)` / `Lens(String)` | `Capture` |
| `Format(String)` | the entry's extension |
| `Iso { op, value }` / `Aperture` / `Shutter` / `FocalLength` | `Capture` |
| `FileSize { op, value }` | the catalog's `size` |

⚠⚠ **`Rating` becomes an operator (`>= 3`) rather than a set**, because an operator is what
composes under `Any`. The *filter bar* keeps its checkboxes. That is a real divergence between how
the transient filter and the saved rule model the same field — and it is the same divergence
Lightroom has between its filter bar and its smart-collection editor. **Stated here so it is a
decision rather than a discovery.**

⚠ ISO, aperture, shutter, focal length and file size are already **stored and unused**; here they
cost one enum variant each, so they are included rather than deferred again.

⚠ `Keyword` includes descendants, reusing the recursive CTE `keyword_members` already has —
searching `Europe` must find photos tagged `Paris`, which is the rule M4.2b established.

---

## Two plans

### Plan 1 — model, evaluator, storage, browsing

- `Expr`, `Criterion`, and a pure `matches(&Facts) -> bool`.
- Schema **v5**: a `smart_collections` table holding a name and the rule as JSON.
- `LibrarySource::Smart { id, name }`, loaded by evaluating the rule over All Photos.
- Panel rows beside Collections.
- **"Save as smart collection"**, capturing the *current source + filter* as a rule.

⚠ **Usable end to end with no editor**: browse, filter, save what you are looking at.

⚠ **What plan 1 deliberately cannot do:** compose *two* sources, because the UI can only be in one
at a time. "Portfolio ∩ July" needs the editor. **Plan 1 gives saved searches; plan 2 gives
composition** — which is the actual point of the milestone.

### Plan 2 — the editor, and multi-select dates

- Add/remove criteria, `all | any | none`, a value widget per criterion type.
- Shift/⌘-click in the date tree, folded in here by the user's call: multi-select periods and
  saved rules both want *"a source described by a rule"*.

---

## Testing

**The evaluator** is where the weight goes, because it is pure: every criterion in isolation; `All`
of an empty list is **true** and `Any` of an empty list is **false** (the identity elements, and
the case a half-built rule is in); `Not` inverts; nesting composes; a criterion whose fact is
**absent** fails rather than passes (the `is_some_and` rule the camera filter already follows).

**Storage** — round trip through JSON; an unknown criterion in a stored rule degrades rather than
failing the whole catalog; schema v5 keeps existing rows.

**Browsing** — a smart collection spans folders; it re-evaluates on open rather than caching
members, so a newly rated photo appears without a restart (the rule `open_collection` already
follows).

### Sabotages to run

- `All` of an empty list returns false → the identity test fails
- `Any` of an empty list returns true → likewise
- a missing fact passes instead of failing → the absent-fact test fails
- `Keyword` stops including descendants → the subtree test fails
- v5 missing from the migration-simulation drop lists → those tests fail

⚠ Each checked for **compile errors first**, by asking whether a `test result:` line appeared at
all — not by grepping for `^error:`, which matches cargo's ordinary test-failure line.

## Accepted caveats

1. **A smart collection builds all entries before filtering** — ~60 ms at 10k, plus thumbnails,
   which are windowed anyway.
2. **The editor is one level deep**, though the model is a tree.
3. **Rating is an operator in a rule and a set in the filter bar** — deliberate, see above.
4. **No live re-evaluation**: a smart collection re-runs when opened, not while you are looking at
   it. Rating a photo out of the rule leaves it on screen until you reopen.
5. **Rules are catalog-only**, like collections and keywords — lost with the catalog, never
   travelling with the photos.
6. **No OR across *sources* in plan 1**, since it captures one source; the editor lifts that.

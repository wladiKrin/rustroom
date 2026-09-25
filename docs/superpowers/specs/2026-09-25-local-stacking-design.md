# Stacks: grouping same-stem files into one grid cell

**Date:** 2026-09-25
**Status:** approved in brainstorming, ready for a plan

---

## 1. Why, and the measurement that makes the case

The user's framing: *"this is something that could be independent of immich, as my other folders also carry raw and jpeg copies of the same images."*

Measured against the real catalog on 2026-09-25:

| folder | files | photographs | RAW+JPEG pairs |
|---|---|---|---|
| `Pictures/fuji` | 830 | **415** | **415 — every one** |
| `Pictures/fuji_new_new` | 804 | **402** | **402 — every one** |
| Immich cache | 2,913 | 2,913 | **0** |
| `Desktop/512724-521236` | 37 | 37 | 0 |

⚠⚠ **Both camera folders are 100% pairs, so the grid shows every photograph twice** — 830 cells for 415
photographs. That is the problem.

⚠ **And the Immich cache has ZERO local pairs**, because its files are named by asset UUID. Stem-based
stacking does nothing there: pairing on that server is a *server-side* fact, and honouring Immich's own
2,379 stacks is a **separate feature**. The user was right that these are independent.

### Why not the format filter, which already exists

`hidden_formats` can hide `.jpg` globally today, collapsing every pair to its RAW. That is rejected as
the answer because this is a **Fuji**: film simulations mean the camera JPEG is often the keeper, so
"always hide JPEG" throws away the file you sometimes want. Stacking is per-pair and keeps both
reachable.

---

## 2. What a stack is: a view grouping, and nothing more

The user's choice of three options offered. **Edits, sidecars, marks and the catalog are untouched** —
every file keeps its own everything. A stack is a statement about what the browser *shows*.

⚠ Deliberately **not** "one photograph with shared marks". That was offered and declined, and it would
have meant a catalog decision about where a shared rating lives. ⚠ Shared *edits* were never on the
table and should not be revisited: a RAW needs demosaic and white balance while a JPEG is already
rendered, so the same `Edits` cannot mean the same thing on both.

### One place, because three bugs say so

⚠⚠ Grouping happens **once**, in the list every consumer reads. The grid, the filmstrip, `selection`,
`view_region` and `format_filter` all consume the same `Vec<PhotoEntry>`; `entries.rs`'s own module doc
records that this path "has produced three sparse-index bugs", and `format_filter`'s says "a wrong
answer here is a photo that silently is not there". Stacking in the grid alone would make the filmstrip
and the selection disagree.

So `PhotoEntry` gains one field and a new pure module does the work:

```rust
// entries.rs
/// Which stack this row belongs to, if any. `None` for a file with no same-stem sibling.
pub stack: Option<Stack>,

/// crates/app/src/stack.rs — pure, egui-free.
pub struct Stack {
    /// How many files share this stem, always ≥ 2.
    pub count: usize,
    /// True on every member of an expanded stack; false on the single row of a collapsed one.
    pub expanded: bool,
}
```

- **collapsed** → exactly **one** row survives, carrying `count`
- **expanded** → **every** member survives, each carrying `expanded: true` so the grid can bracket them
- **not stacked** → `None`, and every existing consumer behaves exactly as before

⚠ `group` returns `Vec<PhotoEntry>`, so nothing downstream changes shape. Expanding **rebuilds** the
list rather than patching indices — which is the whole point of putting it here rather than in the grid.

---

## 3. The key: (folder, stem), case-insensitive

⚠⚠ **Stem alone is safe here, and that is worth stating because it was NOT safe in the Immich
migration the day before.** There, `DSCF0236` named two different photographs because the camera's
counter had wrapped across a 16,000-asset library, and pairing needed `(stem, capture time)`. **Within
one folder the filesystem enforces uniqueness**: `DSCF0236.JPG` can exist only once.

⚠ The residual risk is two wrapped rolls merged into one folder. Accepted, because this is **view-only**:
a wrong pairing is visually confusing and one click from being undone, not a photograph hidden from a
timeline. A different bar, deliberately — not an oversight.

⚠ Case-insensitive, because `DSCF6741.JPG` beside `DSCF6741.raf` is ordinary.

---

## 4. Settings → Appearance

```rust
pub struct Stacking {
    /// Whether stacking happens at all.
    pub enabled: bool,
    /// Which formats take part. A file whose format is absent is never stacked.
    pub formats: FormatFilter,
    /// Which member a collapsed stack shows and opens.
    pub primary: StackPrimary,
}

pub enum StackPrimary { EditedElseRaw, Raw, Jpeg }
```

⚠⚠ **`enabled` defaults to `true`, and that is the `gpu_tail` trap in a new place.** `Settings` carries
a container-level `#[serde(default)]`, so a missing field takes whatever `Default` says — and `true`
**halves the grid on first launch** for an existing install. That is the behaviour the user asked for,
so it stands, but a test pins the default so it is not flipped casually in either direction.

⚠ `formats` defaults to **all supported formats participating**, which is what makes it "more general
than just raw and jpeg" as asked.

### Primary resolution, including the tiebreak the user did not specify

Sort the members by **(has edits, then format rank)** and take the first.

| setting | meaning |
|---|---|
| `EditedElseRaw` (default) | the edited member; ⚠ **if several are edited, the RAW**; if none, the RAW |
| `Raw` | always the RAW, edits ignored |
| `Jpeg` | always the JPEG, edits ignored |

"Has edits" is `entry.edits != Edits::default()` — `PhotoEntry` already carries the edits, loaded once
by `build`, so this costs nothing new.

⚠ The user's default was chosen knowing the consequence: a cell **changes which file it represents**
when you edit the non-RAW member. That only happens if you edit the JPEG, which is rarer and is
arguably what you want to see afterwards — so the instability is mild and intended.

---

## 5. Two settings that interact, stated rather than discovered

⚠⚠ **`hidden_formats` and `Stacking::formats` are two format lists.** The order is **filter, then
stack**: a hidden format cannot be a stack member. So hiding JPEG globally makes stacking a no-op on
these folders. Both settings exist and both are legitimate; this is where it is written down.

⚠ **Virtual copies never collapse.** Stacking considers **copy-0 rows only**; a copy is a version the
user deliberately made, and folding it inside a stack would hide their own work. A copy is always its
own cell, stacked sibling or not.

---

## 6. Reaching the other member

A **count badge** on the collapsed cell; clicking it expands that stack in place, clicking again
collapses it. Lightroom's idiom, so it needs no learning, and while expanded either file can be rated,
flagged or opened individually.

⚠ The expanded set is **session state** — a `HashSet` of stack keys on the app, not persisted. Rebuild
the entry list when it changes; `build` is measured at 4.6 ms for 830 files, so a rebuild is free
against a frame budget.

### Stack all / Expand all, in the Library toolbar

Two buttons, shown **only when stacking is on and the current folder actually contains a stack** — a
folder of unpaired files shows neither, exactly as the format filter hides its chips when a folder has
one format.

⚠⚠ **Library only, never Develop.** `format_filter`'s own doc records why: *"The filter decides what
the browser contains, which is a browsing decision — it has no meaning while a single photo is open,
and it sat in the Develop toolbar for no better reason than living in a shared row."* Stacking is the
same kind of decision, and putting these in a shared row would repeat a mistake already corrected once.

⚠ **Named "Stack all" and "Expand all", not "Unstack all"** (user's call, of three offered). Because
stacking here is **rule-based rather than manual**, there is nothing to permanently unstack — so
"Unstack all" could only mean "expand", while reading like "turn the feature off", which Settings also
does. The ambiguity was on one label only, so only that label changed.

Both drive the same expanded set the badge does — **one mechanism, no second source of truth**: *Stack
all* clears it, *Expand all* fills it with every stack in the folder.

---

## 7. Testing

**All of it without a window, because `stack::group` is pure:**

1. A RAW+JPEG pair collapses to **one** row carrying `count: 2`. ⚠ **Assert the input held two rows
   first** — a "collapsed to one" assertion passes vacuously against an input that never had a pair.
2. Expanded, the same stack yields **both** rows, each with `expanded: true`.
3. A file with no sibling is returned with `stack: None` and is otherwise byte-identical to its input.
4. `enabled: false` returns the input **unchanged**, length included.
5. A format absent from `formats` is never a member: `a.RAF` + `a.jpg` with JPEG excluded gives two
   unstacked rows.
6. Primary resolution: `EditedElseRaw` picks the edited JPEG over an unedited RAW; ⚠ **with both
   edited it picks the RAW**; `Raw` and `Jpeg` ignore edits entirely.
7. ⚠ **Virtual copies are untouched** — a stacked RAW with copy 1 yields the collapsed stack row *and*
   the copy as its own row.
8. Case: `A.JPG` + `a.raf` stack.
9. Two folders holding the same stem do **not** stack together.
10. `Stacking::enabled` defaults to `true` and a settings file written before the field still loads
    with it on.
11. The toolbar buttons are **absent** when stacking is off, **absent** when the folder has no stack,
    and **present** when it has one — all three, since an absence alone passes against a toolbar that
    drew nothing.

**Only use can settle:** that a grid of 415 instead of 830 reads better, that the badge is findable,
and that expanding one pair among 415 feels right.

---

## 8. Out of scope

Each is its own change, not an oversight:

- **Honouring Immich's server-side stacks** — the cache has zero local stem pairs, so this feature does
  nothing there. Still the open backlog item.
- **Manually stacking arbitrary photos**, as Lightroom allows.
- **Grouping by capture time** rather than by stem.
- **Shared marks across a stack** — offered and declined in §2.
- **Persisting the expanded set** across restarts.

---

## 9. Definition of done

1. `Pictures/fuji` shows **415** cells instead of 830, each badged `2`.
2. Clicking a badge expands that pair; clicking again collapses it.
3. **Stack all** and **Expand all** appear in the Library toolbar only when there is something to act
   on, and never in Develop.
4. All three `primary` settings behave as §4 says, including the both-edited tiebreak.
5. Turning stacking off restores exactly the previous behaviour, with the goldens unmoved.
6. ⚠ Seen working, not merely green.

# Immich slice 2: a thumbnail budget, persisted remote thumbnails, and the timeline — design

**Date:** 2026-09-23. **Branch:** `immich-timeline`. **Status:** slice 2 of `docs/BACKLOG.md`'s Immich
integration, designed in conversation 2026-09-23 against **captured** server responses. **Written to
be executed by a different agent**: everything it needs is in this file and the code it names.

## Read first

- `CLAUDE.md` — the conventions. The ones that bite here: **no `Co-Authored-By` trailer**; **never
  push**; every cargo command needs
  `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`;
  **egui lives only in the bin modules**; a file past ~600 lines is reported, not silently grown.
  ⚠⚠ **ONE cargo invocation mode** — `cargo test --workspace <name-filter>`, **never** `-p <crate>`;
  alternating them cost 43–531 s per switch, measured. The whole gate is **~79 s**.
- `docs/superpowers/specs/2026-09-22-immich-browse-design.md` — slice 1. This builds directly on it.
- ⚠⚠ **The standing lesson this slice exists under:** *a fixture you wrote is not evidence about
  something you do not control.* Slice 1 shipped two bugs behind 2,678 green tests because
  `album.json` was hand-written and asserted a field the endpoint never returns. **Every fixture
  added here states whether it was captured or constructed**, and the three load-bearing ones are
  captured from a real server (below).
- `crates/app/src/thumb_store.rs` (572 lines) — `Identity { path, mtime_ns, size }`, `key_bytes`,
  `file_name` (`fnv1a` hex + `.rthm`), `get`, `put` (write-then-rename), `size_bytes` (walks the
  directory), `clear`. ⚠ It "holds a `PathBuf` and nothing else, which is what makes it
  `Send + Sync` for free — six loader workers share one. There is no in-process index to lock:
  **the filesystem is the index**." ⚠ It has **pinned hash vectors** in its tests.
- `crates/app/src/immich_source.rs`, `immich_worker.rs` — slice 1's lib halves.
  `crates/app/src/immich_panel.rs`, `immich_flow.rs` — its bin halves.
- `crates/app/src/date_tree.rs` — `group(&[DayRow]) -> Vec<Year>`. ⚠ **Preserves input order** and
  only starts a new group when the year or month differs from the last, relying on SQL's
  `ORDER BY`; `capture_days()` orders **ascending**.
- `crates/app/src/modals/settings.rs` — the six tabs, `SettingsDraft`, and the existing
  **"Thumbnail cache — N MB [Clear]"** row in the Performance tab (~line 329).

## What the server actually returns — captured 2026-09-23

⚠ Both of these are **real replies from the user's server**, not constructed.

**`GET /timeline/buckets`** → `[{"timeBucket":"2026-08-01","count":76}, …]`

- **Buckets are monthly** — every `timeBucket` is the first of a month. The endpoint takes **no size
  parameter**, so granularity is the server's choice; this was verified rather than assumed.
- **Newest first** (descending).
- One call returns the whole library's tree with counts. 25 captured months hold **~7,600 assets**;
  the largest single month is **1,183** (2025-11), most are 50–700.

**`POST /search/metadata`** → `{"albums":{…},"assets":{"items":[…],"nextPage":"2","nextCursor":null}}`
— already captured in slice 1 as `search_real.json`, and the reason paging follows `nextPage`.

## Why the tree bounds the work, not just the navigation

`open_scan` builds a whole `Library` eagerly: `entries::build` plus one sidecar read per row, measured
at **4.6 ms for 830 files**. A flat "all photos" grid over ~35,000 assets would be a single eager pass
of that. **A month-sized bucket keeps each `Library` at 35–1,183 rows** — the same order as the
830-photo folder this project has actually measured. That is the argument for a tree over infinite
scroll, and it is about the working set rather than about navigation.

---

# Part 1 — the thumbnail budget

Independent of Immich. Ships first **because Part 2 widens a hole that has been open since M8.2**:
`ThumbStore` has no eviction at all today — no budget, no prune, only `clear()`.

## The budget

Two `Settings` fields, **megabytes**, `u32`, default **5120** each:

```rust
/// Byte budget for the local thumbnail cache, in MB.
///
/// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a missing
/// field falls back to this struct's `Default` — which is 5120. A field-level one would use
/// `u32`'s own default of **0**, i.e. a cache that evicts everything. The same trap `write_xmp`,
/// `auto_advance` and `gpu_tail` all document.
pub thumb_budget_mb: u32,
/// Byte budget for the Immich thumbnail cache, in MB. Same trap, same reason.
pub immich_thumb_budget_mb: u32,
```

⚠ **MB rather than GB** so sub-gigabyte is expressible, and because the existing Settings row already
displays MB. ⚠⚠ **The `serde` note above is not boilerplate** — for a `u32` budget the field-level
default is `0`, which would silently turn the cache into a device that deletes every entry it writes.

## Eviction

`ThumbStore` gains a budget, passed in by its callers. ⚠ It must **not** read `Settings` itself: the
store is a lib module and giving it a dependency on the settings loader would make it untestable
without one, and put a preference read inside something six worker threads share. So:

```rust
pub fn new(dir: PathBuf, budget_bytes: u64) -> Self
pub fn user_default(budget_bytes: u64) -> Self       // local dir
pub fn immich_default(budget_bytes: u64) -> Self     // sibling dir
```

⚠ The callers pass the budget. ✅ Counted rather than guessed: **`ThumbStore::user_default()` has
exactly two call sites** — `thumb_loader.rs:292` and `modals/settings.rs:331`.

⚠⚠ **`ThumbLoader::new` must therefore widen**, because it is where the store is built. ✅ It has
**five** real call sites — `sources.rs:200`, `settings_apply.rs:59`, `test_app.rs:297`,
`thumb_loader.rs:630` and `examples/bench_library_open.rs:35` — of which the last three are tests and
a bench that pass a constant. (⚠ Several in-code comments say "four call sites"; that count is stale.
Do not propagate it.) Slice 1 avoided widening this signature and used a shared cell instead, but that
was for the Immich *connection*, which must be seen **without** rebuilding the loader. A budget is
read once at construction, so a parameter is the honest shape.

⚠ **Consequence, stated so it is not a surprise: a budget change takes effect at the next folder
open**, because `sources.rs:200` is what rebuilds the loader. `settings_apply.rs:59` rebuilds it only
for a display-space change. That is acceptable for a cache budget — a folder open happens constantly
— and it is *not* acceptable to fix by rebuilding the loader on Apply, which would drop the open
folder's whole thumbnail cache (~41 ms × N, about 34 s on an 830-photo folder).

**LRU, by touching the entry on a hit.** `get` sets the entry's mtime to now with
`std::fs::File::set_times` — ✅ **verified available and working on this machine's rustc 1.98.0** by
compiling and running it, so no `filetime` dependency is needed.
⚠⚠ **Without the touch, mtime is insertion time and eviction is
FIFO, which discards the folder you revisit most** — the exact opposite of what a cache is for. One
extra syscall against a measured **1.26 ms** read is negligible.

**When a sweep runs:**

- once in `new`, so a store that is already over budget is corrected at startup;
- when an `AtomicU64` of bytes written **this session** passes **10% of the budget**, then resets.

⚠⚠ **Never on every `put`** — `size_bytes()` walks the directory, so per-`put` eviction is a
directory scan per thumbnail. ⚠ The `AtomicU64` is a **deliberate, documented departure** from
"holds a `PathBuf` and nothing else": an atomic keeps the store `Send + Sync` with no lock, which is
what lets six loader workers keep sharing one. It is a counter, not an index.

**The sweep itself:** read the directory, collect `(path, mtime, len)`, sum; while the sum exceeds
the budget, delete the oldest and subtract. Best-effort — an entry that will not delete is skipped,
exactly as `clear()` already does. ⚠ `.tmp` files from an interrupted `put` are deleted on sight
rather than counted.

## What does not change

The RTHM header, `MAGIC`, `VERSION`, `QUALITY`, the write-then-rename discipline, and **`get`'s rule
that every failure is a miss**. ⚠ A budget must never make a folder fail to open.

---

# Part 2 — persisted Immich thumbnails

Today `remote_thumbnail` fetches over HTTP on every cache miss and never writes to disk, because
`Identity::of(path)` stats a file and a not-yet-downloaded asset has none. So album thumbnails
re-fetch after every restart.

## `Identity` becomes an enum

```rust
pub enum Identity {
    /// A source file at a particular version.
    Local { path: PathBuf, mtime_ns: i64, size: u64 },
    /// A remote asset at a particular version. ⚠ No filesystem access — the whole point.
    Remote { asset_id: String, version: String },
}
```

⚠⚠ **`Local`'s `key_bytes()` must stay byte-identical.** Any change makes every thumbnail already on
the user's disk a miss, for nothing. The file's **pinned hash vectors** will catch it — do not "fix"
them, fix the code. ⚠ A test must assert one known `Local` identity still hashes to the value the
existing vectors record.

**`version` is the asset's `updatedAt`, not its `thumbhash`.** `thumbhash` tracks appearance more
tightly but is a tiny perceptual hash that could survive a re-encode unchanged — under-invalidating.
`updatedAt` over-invalidates instead (a favourite toggle costs one refetch), which matches the rule
the file already states: *"never cached under a guessed [identity], which would serve the wrong
pixels."* ⚠ This means `Asset` gains `updated_at` in `crates/immich/src/dto.rs`; the captured
`search_real.json` confirms the field is present and required.

## Two directories, two budgets

Immich entries live in a **sibling directory**, not the local one. ⚠⚠ That is what makes the two
budgets independent: accounting is `size_bytes()` per directory and neither sweep can reach the
other's entries. The alternative — one directory with a kind marker in the RTHM header — would mean
reading every entry's header just to add up its bytes. ⚠ **No `VERSION` bump**: the format is
unchanged and separate directories cannot collide, so existing local caches survive.

## The read path

`immich_source::remote_thumbnail` gains the store: consult it first, fetch only on a miss, `put` on
success. ⚠ It still declines for an already-downloaded original, so a cached local file falls through
to the ordinary loader — the rule slice 1 pinned.

## Settings UI

The local budget goes beside the existing "Thumbnail cache — N MB [Clear]" row in **Performance**;
the Immich budget is a mirrored row in the **Immich** tab, with its own size readout and Clear.
⚠ `size_bytes()` walks the directory, so both readouts are computed when the dialog **opens** and
cached in a field — never in a dialog body, which "runs sixty times a second, which is the
`apply_view_demosaic` mistake in another costume". `app.thumb_cache_bytes` is the existing precedent;
add a second field for the Immich store.

---

# Part 3 — the timeline

## The client generalises

`Client::album(id)` becomes a filtered paged search, so albums and date ranges share one tested path:

```rust
pub enum Filter {
    Album(String),
    /// RFC 3339 bounds, inclusive start and exclusive end.
    TakenBetween { after: String, before: String },
}
pub fn search(&self, filter: &Filter) -> Result<Vec<Asset>, Error>
pub fn buckets(&self) -> Result<Vec<Bucket>, Error>   // GET /timeline/buckets
```

⚠ `album(id)` stays as a thin wrapper over `search(&Filter::Album(id))` so slice 1's call sites and
tests are untouched. ⚠ The paging, the `nextPage`-before-`nextCursor` rule and the round-trip ceiling
are **unchanged** — they move with the function, they are not rewritten.

`Bucket { time_bucket: String, count: u32 }` mirrors the captured reply exactly.

## Grouping

A new `immich_source::group_buckets(&[Bucket]) -> Vec<BucketYear>` — years, each with months and a
count, in the lib so it is testable with no window and no server.

⚠ **Not `date_tree::group`.** Monthly buckets through it would produce a day node meaning "the whole
month", which is a lie in the data rather than a rendering detail. ~20 lines is cheaper than that
lie. ⚠ `date_tree.rs` is not touched.

⚠⚠ **Buckets are reversed to ascending first**, to match the local Dates section: `capture_days()`
orders ascending, and two date trees in one panel reading in opposite directions is the kind of small
inconsistency that reads as a bug. Newest-first is arguably the better default for a photo library —
but then *both* trees should change, and that is its own decision, not a side effect of this one.
⚠ A malformed `timeBucket` is **skipped**, not an error: one bad row must not empty the tree.

## The month's date range

A bucket `"2025-11-01"` becomes `takenAfter = "2025-11-01T00:00:00.000Z"` and
`takenBefore` = the first instant of the next month. ⚠ Month arithmetic is the one place this can go
quietly wrong — December must roll the year. A test covers January, December and a leap February.
⚠ The bounds are **UTC**, matching what the server sent; a photo near a month boundary may therefore
land in the neighbouring month relative to its local time. Recorded rather than guarded: `localDateTime`
exists but filtering on it is not offered by `MetadataSearchDto`.

## The UI

`immich_panel` gains a **Timeline** node beside Albums: collapsing years, each listing months with
counts, clicking a month opening it. ⚠ The bucket list is fetched **once** when the section first
needs it, through the worker like every other call — `Job::Buckets`, never inline in `show`.

`LibrarySource` gains `ImmichBucket { bucket: String, label: String }` beside `ImmichAlbum`.
⚠⚠ **Seven `match` sites on `.source` use catch-alls**, so a new variant compiles silently — slice 1
recorded this. **Review each by hand** and say in the report what each does with the new variant.

`immich_flow` gains `open_immich_bucket`, which sorts with `catalog::sort` exactly as
`open_immich_album` does. ⚠ That sort was missing from the album loader and silently ignored the
library sort setting; do not repeat it.

`entries_from_album` is renamed `entries_from_assets` — it never cared about albums.

---

## Files

| File | Change |
|---|---|
| `crates/app/src/thumb_store.rs` | ⚠ 572 lines — `Identity` enum, budget, LRU touch, sweep, `immich_default` |
| `crates/app/src/settings.rs` | ⚠ 2280 lines — two `u32` fields |
| `crates/app/src/modals/settings.rs` | two budget rows and a second size readout |
| `crates/immich/src/lib.rs` | `Filter`, `search`, `buckets`; `album` a wrapper |
| `crates/immich/src/dto.rs` | `Bucket`; `Asset::updated_at` |
| `crates/immich/tests/fixtures/buckets_real.json` | **captured**, scrubbed |
| `crates/app/src/immich_source.rs` | `group_buckets`, month ranges, store in `remote_thumbnail` |
| `crates/app/src/immich_worker.rs` | `Job::Buckets` |
| `crates/app/src/immich_panel.rs` | the Timeline node |
| `crates/app/src/immich_flow.rs` | `open_immich_bucket` |
| `crates/app/src/main.rs` | `LibrarySource::ImmichBucket`, the second cache-size field |
| `crates/app/src/thumb_loader.rs` | ⚠ 1288 lines — pass the two stores |

## Testing

- **`thumb_store`**: the pinned `Local` vectors still hold; a `Remote` identity needs no filesystem;
  a sweep brings an over-budget directory under it and keeps the **newest**; a touched entry survives
  a sweep that evicts an untouched older one; a `.tmp` leftover is deleted not counted; a budget of 0
  does not panic. ⚠ Each test gets its own temp dir — **the seven-site collision rule stands.**
- **`settings`**: the exhaustive round-trip literal carries both budgets (it will fail to compile
  otherwise, which is what it is for); a config from before them loads with **5120**, not 0.
- **`immich` crate**: `buckets_real.json` parses; monthly, descending, counts present.
- **`immich_source`**: `group_buckets` reverses to ascending, groups by year *and* month, skips a
  malformed bucket, sums counts; month ranges roll December→January and handle a leap February.
- **`immich_worker`**: `Job::Buckets` round-trips; an unconfigured server is an error outcome.
- ⚠ **No golden movement** — this adds no pipeline stage. If a golden moves, something is wrong.

## Done when

The Immich section lists a Timeline of years and months with counts; clicking a month fills the grid
from the server; thumbnails survive a restart; both budgets are visible and adjustable in Settings and
a sweep keeps each cache under its budget. Marks still do not persist on undownloaded photos — that is
slice 3.

## Deliberately out of scope

Catalog rows for Immich assets (and therefore marks on undownloaded photos); day-level buckets;
`thumbhash` placeholders; write-back; the OS keychain; changing the local Dates section's ordering.

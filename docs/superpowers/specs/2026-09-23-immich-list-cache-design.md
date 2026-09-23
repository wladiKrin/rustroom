# Persisting Immich asset lists — design

**Date:** 2026-09-23. **Branch:** `immich-list-cache`. **Status:** agreed in conversation 2026-09-23,
option (a) — validate first. **Written to be executed by a different agent.**

## Why

Opening an Immich album or timeline month costs **a couple of seconds**: the server returns the whole
list's metadata, ~900 KB for the 872-asset album. Slice 2 added an **in-memory** cache, so switching
back within a session is instant — but the first visit after every launch pays the round trip again.

## Read first

- `CLAUDE.md` — conventions. ⚠⚠ **ONE cargo invocation mode**: `cargo test --workspace <name-filter>`,
  never `-p`. The gate is ~79 s: `cargo fmt --all -- --check`, then
  `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings`, then
  `cargo test --workspace`. **Each exit code read from its own command.** No `Co-Authored-By`. Never push.
- ⚠⚠ **The standing lesson this sits under:** an OpenAPI schema describes what a server *may* send, not
  what it does. Three defects in two days came from trusting a reading of the API. **The validator below
  was chosen from a captured reply, not from the schema.**
- `crates/app/src/immich_source.rs` — ⚠ **821 lines, already past the ~600 threshold.** Nothing here is
  added to it. `cache_root`, `host_slug`, `entries_from_assets` live there.
- `crates/app/src/immich_flow.rs` — `pump_immich`, `open_immich_album`, `open_immich_bucket`.
- `crates/app/src/collections_panel.rs` — where a click is applied, and where the in-memory
  `app.immich_assets` lookup already happens.
- `crates/app/src/thumb_store.rs` — the precedent for a regenerable on-disk cache: a versioned header,
  every failure a **miss** rather than an error, write-then-rename, best-effort throughout.

## What the captured reply decided

From `crates/immich/tests/fixtures/albums_real.json`, a real `GET /albums`:

| album | `assetCount` | `lastModifiedAssetTimestamp` | `updatedAt` |
|---|---|---|---|
| analog | 872 | 2026-08-14T08:29:23.862Z | 2026-09-14T15:22:52.492Z |
| Rom | 112 | 2026-06-14T12:11:35.623Z | **2026-09-14T15:22:52.492Z** |

⚠⚠ **`updatedAt` is identical for both albums** — evidently a bulk touch — so it signals nothing about
an album's contents. `lastModifiedAssetTimestamp` differs per album and is explicitly about its assets.
**The validator is `(assetCount, lastModifiedAssetTimestamp)`.** Picking `updatedAt` would have been the
instinctive choice and would have produced a cache that never invalidated.

## The design

**A new lib module `crates/app/src/immich_list_cache.rs`** — no egui, so it is testable without a window.

**Location.** `<cache_dir>/rustroom/immich/<host_slug>-lists/`, a **sibling** of the originals directory
rather than a subdirectory of it. ⚠ `is_immich_path` matches anything under the originals root and
`asset_id_of` takes a path's stem, so a JSON living *inside* that root could be taken for an asset.
Same `host_slug` scoping, so two servers cannot share lists.

**Entries.** `album-<id>.json` and `bucket-<YYYY-MM-DD>.json`, each name passed through
`safe_name::sanitize`. ⚠ The prefix is what stops an album whose id happened to look like a date from
colliding with a bucket.

**Payload.**

```rust
struct StoredList {
    /// ⚠ Bumped whenever `immich::Asset`'s shape changes. A wrong version is a miss, so a bump
    /// invalidates cleanly instead of deserialising old fields into a new struct.
    version: u16,
    validator: Validator,
    assets: Vec<immich::Asset>,
}

enum Validator {
    /// ⚠ NOT `updatedAt` — see the table above.
    Album { asset_count: u32, last_modified: Option<String> },
    /// ⚠ A count alone: swapping one asset for another leaves it unchanged, so that edit goes
    /// unnoticed until something else changes. Accepted rather than paying a request per month; a
    /// stale list is cheap to discard and this is a cache, not a record.
    Bucket { count: u32 },
}
```

⚠ `last_modified` is `Option` because the field is not in the schema's required list — the one place
this design does trust the schema, and it costs nothing to tolerate its absence.

**API.**

```rust
pub fn load(root: &Path, key: &Key, want: &Validator) -> Option<Vec<immich::Asset>>;
pub fn store(root: &Path, key: &Key, validator: &Validator, assets: &[immich::Asset]);
pub fn lists_dir(base_url: &str) -> PathBuf;
pub enum Key { Album(String), Bucket(String) }
```

⚠⚠ **Every failure is a miss, never an error** — missing, unreadable, wrong version, mismatched
validator, malformed JSON. `ThumbStore`'s rule: *"a cache that can stop a folder opening is worse than
no cache."* ⚠ `store` is best-effort and silent, write-then-rename so a crash cannot leave a truncated
list that deserialises to garbage.

**The flow at a click**, extending what `collections_panel` already does:

1. `app.immich_assets` (in memory) → open now.
2. Otherwise `list_cache::load` with the validator built from the rail's own album/bucket list → open
   now, and populate the in-memory map so the next switch skips even the read.
3. Otherwise request as today, and `store` on arrival beside the existing in-memory insert.

⚠ Step 2 needs the rail's list to be in hand, which it is: the section fetches `/albums` or
`/timeline/buckets` when first opened, and both are small. ⚠ **Validate first** (agreed): a stale list is
never shown. A grid that rearranges under you — possibly dropping a selection — is worse than a wait the
status line already announces.

**Size, as arithmetic.** Only the modelled fields are serialised, ~200 bytes an asset: ~175 KB for the
872-asset album, ~7 MB for a 35,000-asset library across every bucket. A stale list is **replaced**, not
accumulated, so the directory holds at most one entry per album or bucket that exists — self-limiting by
the library's shape. ⚠ **No budget, deliberately**, unlike the thumbnail cache: that one was 85 GB of
never-pruned artefacts, this is bounded by how many albums and months exist.

## Testing

- Round trip: `store` then `load` with the same validator returns the assets.
- A different `asset_count`, a different `last_modified`, a different bucket `count`, and a bumped
  `version` are each a **miss**.
- Malformed JSON, an empty file and an absent file are misses, not errors.
- An unwritable directory makes `store` a no-op rather than a panic.
- `Key` naming: an album and a bucket never collide; a hostile id cannot escape the directory.
- Two hosts get different directories.
- ⚠ No golden movement — this adds no pipeline stage.

## Done when

Relaunch, click an album opened before: it fills **without** the couple-of-second wait. Add a photo to
that album in Immich, click it again: it refetches. The status line still announces a real fetch.

## Out of scope

Persisting the album or bucket *lists* themselves (they are one small call); showing a stale list while
refreshing; any budget or eviction; slice 3's catalog rows.

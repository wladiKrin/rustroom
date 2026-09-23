# Reindex refetches from Immich — design

**Date:** 2026-09-23
**Branch:** `immich-reindex-refetch` (on top of `immich-list-cache`)
**Status:** approved by the user; two open questions answered by him (scope, copy numbering)

## The problem, stated precisely

Reindex today refuses on an Immich source: *"Reindex works on a folder, not a collection, a date or
All Photos"*. So there is no way to pick up a change made in Immich without restarting Rustroom.

⚠⚠ **And the reason is narrower than "there is no refresh".** A click resolves an Immich album in
three steps (added by the list cache, 2026-09-23):

1. `immich_assets` — the in-memory map for this session
2. the stored list on disk, **only if its validator still matches**
3. a fetch

Steps 2 and 3 already notice a change: the rail's album list is refetched at launch, so a changed
`assetCount` or `lastModifiedAssetTimestamp` fails validation and the list is refetched. **Step 1 is
not validated at all.** Once an album has been opened in a session, its list is frozen for the life of
the process — even after the rail's own Refresh button, which refetches the *album list* and never the
*asset lists*.

So: a change in Immich is picked up on a **relaunch**, and never within a session.

## What Reindex will do on an Immich album or month

In order, at the button press:

1. **`persist_current_session()`** — first, exactly as the folder path does, so an unsaved edit is
   written before anything is dropped.
2. **Clear `self.session`.** ⚠ Safe only because of step 1. The folder path does the same
   (`self.session = None` before reopening); the difference is that Immich does **not** reopen the
   photo, because it may no longer exist on the server and re-downloading it is not what Reindex is
   for. **You land in the grid.** Documented, not accidental.
3. **Close copy-numbering gaps over the *current* scan** — the same loop as the folder path, extracted
   into one shared private helper. ⚠ Compacting *before* the refetch loses nothing: a photo newly
   added in Immich has no sidecar yet, so there is nothing of its to compact.
4. **Remove this album's or month's entry from `immich_assets`.** ⚠⚠ This single line is what makes a
   refetch visible; without it steps 5–6 would fetch and the click would still serve the frozen list.
5. **Request the matching rail list** — `Job::Albums` for an album, `Job::Buckets` for a month. ⚠ Not
   both: the album validator comes from the album list and a bucket's from the bucket list, so the
   other is irrelevant, and asking for both would hit the guard described below.
6. **Request the asset list** — `Job::Album`/`Job::Bucket`, which is never dropped (it bumps
   `open_seq`), and set `immich_opening` plus a status line, exactly as a click does, so the press has
   feedback during the couple of seconds the server takes.

The reply then lands on the **existing** `pump_immich` path, which inserts into `immich_assets`,
stores the list to disk with a fresh validator, and reopens through `open_immich_album`. **No new
reopen path, no new reply handler, no new state on `RustroomApp`.**

## The guard this requires

⚠⚠ `ImmichWorker::request` has **one** `pending_list` flag covering both `Job::Albums` and
`Job::Buckets`:

```rust
Job::Albums | Job::Buckets => {
    if self.pending_list { return; }
    self.pending_list = true;
    None
}
```

The rail survives this because it re-requests on **every frame** while its list is `None`, so a
dropped request is retried a frame later. **A one-shot request from Reindex would be dropped
permanently** — and silently, which is precisely the shape of the defect that made a second album
click vanish (fixed 2026-09-23 by taking user-initiated opens out of this guard).

**Fix:** split it into `pending_albums` and `pending_buckets`, one per list. This also removes a
latent per-frame collision when both rail sections are expanded with neither list fetched.

`is_listing()` keeps its meaning by returning `pending_albums || pending_buckets`.

## Deliberately not done

- **Other albums' cached lists are untouched** — the user chose "the open album plus the rail lists".
  Dropping every cached list would throw away work he may want and make later clicks slow again.
- **The stored list on disk is not deleted.** The refetch overwrites it. ⚠ And if the fetch *fails*,
  the old entry survives beside a **refreshed** rail list, so its validator mismatches and it refetches
  on the next launch — self-correcting, with no cleanup path to get wrong.
- **No reopen of the photo that was open.** See step 2.

## Known soft spots, flagged rather than hidden

- ⚠ **Steps 5 and 6 race.** The rail list is ~8 rows against up to 1,022 assets, so it lands first in
  practice, but it is not guaranteed. If it loses, the refetched list is stamped with the *old*
  validator, which mismatches on the next launch and costs **one** extra fetch. Never wrong data. Not
  worth sequencing the two jobs for.
- ⚠ **A bucket is still validated on its count alone**, so Reindex on a month whose asset count did
  not change will refetch (correct — Reindex is explicit) but the *stamp* cannot distinguish a swap.
  Unchanged by this work; recorded in the list-cache spec.
- ⚠ `crates/app/src/shell.rs` is **1,379 lines** and `crates/app/src/immich_worker.rs` is **698** —
  both past the ~600 threshold. This change adds a handful of lines to each and no new
  responsibility; neither is split here.

## Testing

Unit, in `immich_worker.rs`:
- Albums and Buckets can be in flight **together**; neither drops the other. ⚠ This is the regression
  test for the shared guard — it must fail before the split.
- A second `Albums` while one is outstanding is still dropped (the per-frame guard still guards).
- `is_listing()` is true while either is outstanding and false once both have replied.

Unit, in `copies.rs`, through `test_app::for_test()`:
- Reindex on an `ImmichAlbum` **removes that album's entry from `immich_assets`** and leaves other
  albums' entries alone.
- Reindex on an `ImmichBucket` does the same for the bucket key.
- Reindex on an Immich source with **no server configured** says so and changes nothing.
- Reindex on a `Collection`/`Periods`/`AllPhotos` still refuses, and the message now names Immich as
  supported rather than implying it is not.
- The extracted compaction helper is still exercised by the existing folder Reindex tests, which must
  pass unchanged — ⚠ the extraction is a pure move and any golden or copy test that moves means it
  was not.

⚠⚠ **What no test can reach, and what closes this:** that a change made in Immich actually shows up
after pressing Reindex. Every Immich defect so far was found by use.

## UI copy

The ⋯ menu's hover text becomes source-aware:

- folder: unchanged — *"Re-read the folder, sort it alphabetically, and close any gaps in the copy
  numbering."*
- Immich album or month: *"Fetch this album again from Immich to pick up added or removed photos, and
  close any gaps in the copy numbering."* (month: *"Fetch this month again…"*)

⚠ The label stays **Reindex**. One verb, one place; a second name for the same button would be worse
than a hover that adapts.

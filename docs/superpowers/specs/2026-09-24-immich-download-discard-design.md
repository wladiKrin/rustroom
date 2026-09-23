# Immich slice 4 — Download and Remove downloads — design

**Date:** 2026-09-24
**Branch:** `immich-download-discard`
**Status:** approved; three questions answered by the user (placement, naming, progress)

## What it is for

The user's words: *"a 'download' button, that allows me to predownload e.g. an album or selection of
images from immich, as well as a discard or delete button."*

Two jobs: **take a working set offline**, and **get the disk space back afterwards** — the other half of
the *"no eviction for the cached originals"* gap that has been out of scope since slice 1.

## Placement: both the rail and the toolbar

**The user's choice of three.** They are genuinely different targets, and the asymmetry is the one
`apply_mark_to` already documents for Compare and Survey:

- **The rail** — a `↓` and a `✕` action on every Immich **album** and **month** row. *"Take this trip
  offline."* ⚠ Album rows carry `actions: &[]` today, so the affordance already exists and is unused
  (Folders uses it for "Forget").
- **The toolbar** — **Download** and **Remove downloads** beside Export, acting on the **current
  selection**. *"These twelve, not all 872."*

## Naming: "Remove downloads", never "Delete"

**The user's choice**, and the reason matters more than the words.

⚠⚠ **The action cannot reach the server and must not read as though it could.** The API key is
read-only by the Settings help text's own instruction, so deleting from Immich is not merely out of
scope — it is **impossible with the credentials Rustroom asks for**. "Discard" or "Delete" beside a
photograph reads as *throw the photograph away*, with a hover as the only correction.

⚠ The hover says so explicitly: *"Deletes only the local copies. The photos stay on your Immich
server."*

## What Remove downloads deletes, and what it must not

**Deletes:** the cached original image file, and nothing else.

⚠⚠ **It must NOT delete the sidecar, and the naming makes that safe by construction.** A sidecar is the
full filename with `.json` **appended** — `abc.jpg` → `abc.jpg.json`, never the extension replaced — so
deleting `abc.jpg` leaves the edits untouched. And `present_names` already filters through
`catalog::is_supported`, so a sidecar was never counted as a downloaded photo either.

⚠ **It must NOT touch catalog rows.** They hold the marks (slice 3), and `row_is_browsable` already
hides an absent Immich row from catalog-wide views — so a removed photo leaves the Library tidy without
anything being deleted from the catalog.

⚠ **Thumbnails survive**, because they live in a different directory (`<slug>-thumbs`, its own LRU
budget). Removing originals must not make browsing slow again — that would defeat the point.

⚠⚠ **Re-downloading restores byte-identical identity**, which is exactly what `cache_path`'s doc means
by *"re-downloading an evicted or deleted file must restore byte-identical identity"*. The edits reattach
because the path is derived from the asset UUID and nothing else. **This is the property the whole
feature rests on, and it already exists and is already documented.**

⚠ **The open photo is skipped**, not deleted underneath its session, and the status line says how many
were skipped. Deleting the file a render is reading is the one way this feature could crash.

## Progress: a status line, plus a cancel

**The user's choice.** Reuses the `export_progress` pattern — a `String` on the app, cleared when done —
rather than a modal, because a predownload is a background job and a modal is the opposite of that.

⚠ *"Downloading 41 of 872…"* with a `✕` beside it.

## ⚠⚠ The one real implementation risk: 872 threads

`ImmichWorker::request` **spawns a thread per job**. `in_flight` dedupes but bounds nothing, so asking
for an 872-asset album today would spawn 872 threads and 872 concurrent HTTP requests.

**The worker gains a bounded queue:**

- `Job::Original` beyond `MAX_CONCURRENT_DOWNLOADS` (**4**) goes on a pending `VecDeque` instead of
  spawning. ⚠ Four because these are network-bound and the server is one machine; `batch_export`'s
  four-wide default is the precedent for the number, not for the mechanism.
- `poll()` starts the next queued job as each completes — it already runs once a frame and already
  clears `in_flight`.
- **Cancel clears the queue and lets the in-flight four finish.** ⚠ The `DecodeWorker` idiom this
  project uses everywhere: *superseded work is abandoned, not aborted.* Killing a download mid-write
  would leave a `.part` file, which `part_path` exists to make survivable but which there is no reason
  to create deliberately.

⚠ The existing single-photo download path must keep working unchanged — it is the same `Job::Original`,
and with a queue depth of one it never queues.

## Testing

In `immich_worker`, headless with a stub fetcher:
- ⚠⚠ **Never more than `MAX_CONCURRENT_DOWNLOADS` in flight**, asserted with a fetcher that counts
  concurrent entries — the whole point of the queue, and the thing a green suite would otherwise miss.
- Every queued job **does eventually run**: a queue that starts nothing after the first batch is worse
  than no queue.
- **Cancel** stops further starts and leaves the in-flight ones to complete; a second cancel is a no-op.
- ⚠ Requesting the **same asset twice** still dedupes, queued or in flight.
- A **single** download behaves exactly as before — no queueing observable.

In a new lib module for the discard half, with no window:
- ⚠⚠ **The sidecar survives**, and the test writes a real `abc.jpg.json` beside `abc.jpg` and asserts
  the image is gone and the sidecar is not. **This is the test that protects the user's work.**
- The **open photo** is skipped and counted.
- A file that is already absent is not an error.
- ⚠ Only supported images are removed — a stray `.json` or `.txt` in the cache directory is left alone.
- `immich_present` is refreshed afterwards, so the rail's count and the grid's ↓ agree immediately.

⚠ What no test reaches: whether 872 downloads over a real network behave, and whether the buttons are
where a hand expects them. Seen in use, or not at all.

## Deliberately not done

⛔ Deleting from Immich — impossible with a read-only key and never intended. ⛔ An automatic eviction
policy: this is the **manual** half, and a byte budget for originals is a separate decision the
thumbnail stores' budget is the precedent for. ⛔ Downloading a whole *server* — the rail offers albums
and months, which is as coarse as anything here gets.

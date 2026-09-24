# Immich slice 4 — Download and Remove downloads — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Predownload an album, a month or a selection; get the space back afterwards without losing an edit.

**Architecture:** `ImmichWorker` gains a bounded queue so a bulk request cannot spawn a thread per asset. A new lib module removes cached originals while leaving sidecars, catalog rows and thumbnails alone. The rail's album/month rows gain two actions; the toolbar gains two buttons acting on the selection.

**Spec:** `docs/superpowers/specs/2026-09-24-immich-download-discard-design.md`. Read it first.

---

## Conventions

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By`. Never push.** Branch `immich-download-discard`.
- ⚠⚠ `cargo test --workspace <filter>`, **never `-p`**. A filter matching nothing **exits 0 with zero tests** — check the passed count, and remember `folder_display` once matched none.
- ⚠⚠ **All three gate commands green before any commit.** On 2026-09-24 I read `CLIPPY: 101` and committed anyway.
- ⚠⚠ **A test that cannot fail is not a test.** Prove the sample, prove the fixture differs, and don't lower a threshold to pass — see `CLAUDE.md`'s standing lesson and its four shapes.
- ⚠ Scripted multi-site edits: **descending line order**, and diff the doc lines afterwards.

## File structure

| File | Responsibility |
|---|---|
| `crates/app/src/immich_worker.rs` | the bounded queue, cancel, progress counters |
| `crates/app/src/immich_cache.rs` | **New.** removing cached originals |
| `crates/app/src/immich_flow.rs` | the app-side verbs: download a set, remove a set |
| `crates/app/src/immich_panel.rs` | two row actions and their `Action` variants |
| `crates/app/src/collections_panel.rs` | dispatch the new actions |
| `crates/app/src/shell.rs` | two toolbar buttons (⚠ 1,379 lines — add nothing else) |

---

### Task 1: The queue that stops 872 threads

**Files:** `crates/app/src/immich_worker.rs`

- [ ] **Step 1: Failing tests.** ⚠ Read the file's existing stub-fetcher helpers first and follow them.

```rust
    /// ⚠⚠ **The whole point of the queue.** `request` spawns a thread per job and `in_flight` only
    /// dedupes, so before this an 872-asset album meant 872 threads and 872 concurrent HTTP requests at
    /// the user's own server.
    ///
    /// ⚠ Counts **concurrent** entries, not total calls: a fetcher that merely counted calls would pass
    /// against no queue at all.
    #[test]
    fn no_more_than_the_cap_download_at_once() {
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (l, p) = (live.clone(), peak.clone());
        let mut w = ImmichWorker::new(
            Arc::new(move |_job: Job| {
                let n = l.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(30));
                l.fetch_sub(1, Ordering::SeqCst);
                Outcome::Original { dest: PathBuf::from("x"), result: Ok(()) }
            }),
            || {},
        );
        for i in 0..40 {
            w.request(Job::Original {
                asset_id: format!("a{i}"),
                dest: PathBuf::from(format!("/tmp/a{i}.jpg")),
            });
        }
        // ⚠ Drain until everything has completed, starting queued work as `poll` does in production.
        let mut done = 0;
        settle(&mut w, |w| {
            done += w.poll().len();
            done >= 40
        });
        assert_eq!(done, 40, "every queued download must eventually run");
        let got = peak.load(Ordering::SeqCst);
        assert!(
            got <= MAX_CONCURRENT_DOWNLOADS,
            "{got} downloads ran at once, cap is {MAX_CONCURRENT_DOWNLOADS}"
        );
        assert!(got > 1, "the fixture must actually overlap, or the cap proves nothing");
    }

    /// ⚠ Cancel clears the **queue** and lets the in-flight finish — `DecodeWorker`'s idiom:
    /// superseded work is abandoned, not aborted. Aborting mid-write would leave a `.part`.
    #[test]
    fn cancelling_stops_further_starts_and_is_idempotent() { … }

    /// ⚠ The single-photo path must be unchanged: with one request nothing ever queues.
    #[test]
    fn a_lone_download_never_queues() { … }

    /// ⚠ Dedupe still holds for a queued asset, not just an in-flight one.
    #[test]
    fn the_same_asset_requested_twice_runs_once_even_when_queued() { … }
```

- [ ] **Step 2: Run — expect failures.** ⚠ The cap test must fail with a peak far above 4, not merely fail to compile; if it compiles and passes, the fixture is not overlapping.

- [ ] **Step 3: Implement.**

```rust
/// How many originals download at once.
///
/// ⚠⚠ **Four, and a queue rather than a thread per job.** `request` spawns a thread, and `in_flight`
/// only dedupes — so a bulk request for an 872-asset album meant 872 threads and 872 concurrent HTTP
/// requests at one self-hosted server. ⚠ The number follows `batch_export`'s four-wide default because
/// these are network-bound against a single machine; the *mechanism* is this queue, not that precedent.
pub const MAX_CONCURRENT_DOWNLOADS: usize = 4;
```

fields: `queued: std::collections::VecDeque<Job>`, and in `request`'s `Job::Original` arm — after the
`in_flight` dedupe succeeds — push to `queued` instead of spawning when
`in_flight.len() > MAX_CONCURRENT_DOWNLOADS`. ⚠ Factor the spawn into `fn spawn(&self, job: Job)` so
`request` and `poll` start work the same way, or they will drift.

In `poll`, after `in_flight.remove(id)`, start the next queued job. ⚠ **After** the removal, or the cap
check sees the finished job as still live.

`pub fn cancel_downloads(&mut self) -> usize` — clears `queued`, returns how many were dropped, and
`in_flight` is left alone with a comment saying why. `pub fn queued_downloads(&self) -> usize` for the
progress line.

- [ ] **Step 4: Run. Step 5: Commit.**

---

### Task 2: Removing cached originals without losing an edit

**Files:** create `crates/app/src/immich_cache.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Failing tests** — ⚠ the first one is the one that protects the user's work.

```rust
    /// ⚠⚠ **THE TEST THIS FEATURE EXISTS TO PASS.** A sidecar is `<filename>.json` **appended**, so
    /// removing `abc.jpg` cannot touch `abc.jpg.json` — but "cannot by construction" is exactly the kind
    /// of claim this project has been wrong about, so it is asserted against a real file on disk.
    #[test]
    fn removing_a_download_keeps_its_edits() {
        let d = temp_dir("edits");
        let img = d.join("abc.jpg");
        let car = d.join("abc.jpg.json");
        std::fs::write(&img, b"jpeg").unwrap();
        std::fs::write(&car, b"{\"version\":7}").unwrap();

        let out = remove_downloads(&[img.clone()], None);

        assert_eq!(out.removed, 1);
        assert!(!img.exists(), "the cached original must go");
        assert!(car.exists(), "the sidecar must NOT — it holds the edits");
        assert_eq!(std::fs::read(&car).unwrap(), b"{\"version\":7}", "and be untouched");
    }

    /// ⚠ The open photo is skipped rather than deleted under its session, and counted so the status can
    /// say so. Deleting a file a render is reading is the one way this crashes.
    #[test]
    fn the_open_photo_is_skipped_and_counted() { … }

    /// ⚠ Only supported images. A stray `.json` or `.txt` in the cache directory is not a download.
    #[test]
    fn a_non_image_is_never_removed() { … }

    /// ⚠ Already absent is not an error — two clicks in a row must not report a failure.
    #[test]
    fn an_absent_file_is_not_an_error() { … }
```

- [ ] **Step 2: Implement** a module whose doc carries the spec's reasoning (sidecars, catalog rows,
thumbnails, byte-identical re-download), and:

```rust
/// What a removal did, for the status line.
pub struct Removed { pub removed: usize, pub skipped_open: usize }

/// Delete the cached originals at `paths`, leaving everything else alone.
///
/// ⚠⚠ Deletes **only** the image file. A sidecar is `<filename>.json` appended, so it is untouched;
/// catalog rows keep the marks and `row_is_browsable` hides an absent row from catalog-wide views; and
/// thumbnails live in their own directory with their own budget, so browsing stays fast.
///
/// ⚠ `open` is the photo currently in Develop, skipped rather than deleted beneath its render.
pub fn remove_downloads(paths: &[PathBuf], open: Option<&Path>) -> Removed;
```

- [ ] **Step 3: Run, then Commit.**

---

### Task 3: The app-side verbs

**Files:** `crates/app/src/immich_flow.rs`

- [ ] `download_assets(&mut self, paths: &[PathBuf])` — maps each path to `asset_id_of`, skips what
  `immich_present` already has, requests `Job::Original` for the rest, and sets
  `immich_download_progress`. ⚠ Skipping what is present is what makes pressing it twice cheap.
- [ ] `remove_downloaded(&mut self, paths: &[PathBuf])` — calls `immich_cache::remove_downloads` with
  the open photo, then `refresh_immich_present()`, then sets the status from `Removed`.
- [ ] Progress in `pump_immich`'s `Outcome::Original` arm: a `done`/`total` pair on the app, formatted
  like `export_progress`. ⚠ Cleared when the queue empties **and** nothing is in flight.
- [ ] ⚠ A test that `download_assets` requests **nothing** for assets already present.

---

### Task 4: The rail's two actions

**Files:** `crates/app/src/immich_panel.rs`, `crates/app/src/collections_panel.rs`

- [ ] `Action::DownloadAlbum { id, name }` / `RemoveAlbum { id, name }`, and the bucket pair. ⚠ The rail
  knows the album's **asset list** only if it has been opened or cached — so the action needs the same
  three-step lookup the click path uses (session map → stored list → fetch). ⚠⚠ **If the list is not in
  hand, request it and act when it lands**, rather than silently doing nothing; say so in the status.
- [ ] Two `RowAction`s on album and month rows: `↓` *"Download"* and `✕` *"Remove downloads"*, with the
  spec's hovers. ⚠ `RowOut::action` is an index into `actions`, so the two must be read by position —
  name the indices with `const` rather than writing `0` and `1`.
- [ ] ⚠ A test that the actions are offered and dispatch distinctly — an off-by-one on the index would
  download when you meant to remove, which is the worst confusion available here.

---

### Task 5: The toolbar's two buttons

**Files:** `crates/app/src/shell.rs` (⚠ **1,379 lines** — add the two buttons and nothing else)

- [ ] Beside Export, **Library only**, enabled only when the source is Immich and the selection is
  non-empty. Labels **Download** and **Remove downloads**; the hovers from the spec.
- [ ] The progress line and its `✕` next to them, mirroring `export_progress`.
- [ ] ⚠ A test that neither button appears under a **folder** source.

---

### Task 6: Gate, then look at it

```bash
cargo fmt --all && cargo fmt --all -- --check; CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace
```

⚠ All three **0**, **75 goldens unmoved** — no pipeline stage is touched.

Then: download a small album; watch the count; cancel mid-way; **quit and relaunch** and confirm the
downloaded ones are still marked; remove them; ⚠⚠ **confirm an edited one keeps its edits after removal
and re-download**; check a folder source shows no buttons.

Then the journal and `CLAUDE.md`.

## Self-review

| Spec requirement | Task |
|---|---|
| Bounded at 4; every queued job runs | 1 |
| Cancel clears the queue, in-flight finish, idempotent | 1 |
| Lone download unchanged; dedupe holds when queued | 1 |
| Sidecar survives — asserted on disk | 2 |
| Open photo skipped and counted | 2 |
| Non-images never removed; absent is not an error | 2 |
| Present assets are not re-requested | 3 |
| Rail actions for albums **and** months, list fetched if absent | 4 |
| Toolbar buttons, Immich-only, selection-only | 5 |
| Nothing reaches the server but a download | — |
| Seen in use, including the edit round-trip | 6 |

**Soft spots:** the cap of **4** is a guess, not a measurement — one named constant. Cancel leaves up to
four downloads running, which is deliberate and worth seeing in use. `shell.rs` stays oversized.

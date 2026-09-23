# Reindex refetches from Immich — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pressing Reindex while looking at an Immich album or month fetches its asset list again, so photos added or removed in Immich appear without restarting Rustroom.

**Architecture:** Reindex gains an Immich branch that persists the session, compacts copy numbering over the current scan, drops the album's entry from the in-memory `immich_assets` map, and requests both the matching rail list and the asset list. The reply lands on the existing `pump_immich` path, so there is no new reopen path and no new state. `ImmichWorker`'s single `pending_list` flag is split in two first, because a one-shot rail request would otherwise be dropped silently.

**Tech Stack:** Rust, egui (bin modules only), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-23-immich-reindex-design.md`. Read it before Task 1.

---

## Conventions for every task

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By` trailer. Never push.** Branch is already `immich-reindex-refetch`.
- ⚠⚠ **ONE cargo invocation mode.** Always `cargo test --workspace <name-filter>`; **never `-p <crate>`** — alternating them costs 43–531 s per switch, measured.
- ⚠⚠ **The filter is a SUBSTRING, not a regex,** and a filter matching nothing **exits 0 having run zero tests.** Check the *passed count*, not the exit code. `'a|b'` matches nothing.
- **Read each exit code from its own command.** Write cargo's output to a file and `echo "EXIT: $?"` on the *same* line-sequence; a `| grep` reports grep's status, which has produced a false green here.
- ⚠ Plan snippets go stale when `cargo fmt` reformats a block. If an edit does not match, read the file rather than forcing it.
- **egui only in bin modules.** `immich_worker.rs` is a **lib** module: no egui in Task 1.

## File structure

| File | Responsibility | Size before |
|---|---|---|
| `crates/app/src/immich_worker.rs` | split `pending_list` into two flags | 698 ⚠ past ~600 |
| `crates/app/src/copies.rs` | extract the compaction loop; add the Immich branch of `reindex` | 307 |
| `crates/app/src/shell.rs` | source-aware hover text | 1,379 ⚠ past ~600 |

⚠ Both oversized files gain only a handful of lines and no new responsibility. Neither is split here; both are reported.

---

### Task 1: Albums and Buckets can be in flight together

**Files:**
- Modify: `crates/app/src/immich_worker.rs`

- [ ] **Step 1: Write the failing tests**

⚠ Find the existing test module's helper for building a worker with a stub fetcher (around
`w.request(Job::Buckets)` at ~line 472) and follow it exactly rather than inventing one.

```rust
    /// ⚠⚠ **The regression test for a shared guard.** One `pending_list` flag covered both list
    /// kinds, so asking for the album list and the bucket list in the same frame **silently dropped
    /// the second**. The rail never noticed because it re-requests every frame while its list is
    /// `None`; a one-shot request from Reindex would have been lost for good. Same shape as the guard
    /// that swallowed a second album click.
    #[test]
    fn an_albums_and_a_buckets_request_do_not_drop_each_other() {
        let (mut w, seen) = worker_recording_jobs();
        w.request(Job::Albums);
        w.request(Job::Buckets);
        let jobs = seen.lock().unwrap().clone();
        assert!(
            jobs.iter().any(|j| matches!(j, Job::Albums)),
            "the album list must be requested: {jobs:?}"
        );
        assert!(
            jobs.iter().any(|j| matches!(j, Job::Buckets)),
            "and the bucket list must NOT be swallowed by it: {jobs:?}"
        );
    }

    /// ⚠ The per-frame guard must still guard: the rail asks on every frame while it has no list, and
    /// without this that is a thread per frame.
    #[test]
    fn a_second_albums_request_while_one_is_outstanding_is_still_dropped() {
        let (mut w, seen) = worker_recording_jobs();
        w.request(Job::Albums);
        w.request(Job::Albums);
        let n = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|j| matches!(j, Job::Albums))
            .count();
        assert_eq!(n, 1, "the duplicate must be dropped");
        assert!(w.is_listing(), "and a list is still outstanding");
    }
```

⚠ If no `worker_recording_jobs` helper exists, write one next to the existing stub-fetcher helper: a
worker whose `fetch` pushes the `Job` into an `Arc<Mutex<Vec<Job>>>` and returns an empty success.
`Job` may need `#[derive(Debug)]` — add it if so; it is a plain enum.

- [ ] **Step 2: Run them to verify the first fails**

```bash
cargo test --workspace do_not_drop_each_other > /tmp/t.log 2>&1; echo "EXIT: $?"; grep -E '^test |panicked|assert' /tmp/t.log | head
```

Expected: FAIL — "the bucket list must NOT be swallowed by it".

- [ ] **Step 3: Split the flag**

Replace the field:

```rust
    /// Whether the **album list** fetch is outstanding, so an expanded section does not refetch on
    /// every frame.
    ///
    /// ⚠⚠ Two flags, one per list, and **not** one shared flag. A single flag meant
    /// `request(Albums)` immediately followed by `request(Buckets)` dropped the second one silently.
    /// The rail survived that by re-requesting every frame; Reindex asks **once**, so it would not
    /// have. Same family as the guard that swallowed a second album click.
    pending_albums: bool,
    /// Whether the **bucket list** fetch is outstanding. See [`Self::pending_albums`].
    pending_buckets: bool,
```

`new()`: replace `pending_list: false` with both fields `false`.

`is_listing()`:

```rust
    pub fn is_listing(&self) -> bool {
        self.pending_albums || self.pending_buckets
    }
```

`request()`: replace the shared arm with two:

```rust
            Job::Albums => {
                if self.pending_albums {
                    return;
                }
                self.pending_albums = true;
                None
            }
            Job::Buckets => {
                if self.pending_buckets {
                    return;
                }
                self.pending_buckets = true;
                None
            }
```

⚠ Then find **every** place that clears `pending_list` (the reply-draining side, `poll`) and clear the
matching flag instead. `grep -n pending_list crates/app/src/immich_worker.rs` must come back empty
before you run the tests.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich_worker > /tmp/t.log 2>&1; echo "EXIT: $?"; grep -c '^test ' /tmp/t.log
```

Expected: PASS, with a non-zero count and no test lost.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_worker.rs
git commit -m "One guard per list, so a one-shot request is not swallowed

A single pending_list flag covered both Albums and Buckets, so asking for both in one frame
dropped the second silently. The rail never noticed because it re-requests every frame while
its list is None; Reindex asks once, so it would have lost the request for good. Same shape as
the guard that swallowed a second album click."
```

---

### Task 2: The compaction loop becomes a shared helper

**Files:**
- Modify: `crates/app/src/copies.rs`

⚠⚠ **A pure move.** No behaviour changes in this task. The existing folder Reindex tests must pass
**unchanged** — if one needs editing, the move was not pure.

- [ ] **Step 1: Note the current test count**

```bash
cargo test --workspace copies > /tmp/before.log 2>&1; echo "EXIT: $?"; grep -c '^test ' /tmp/before.log
```

Write the number down. It must be identical at Step 4.

- [ ] **Step 2: Extract the helper**

In `copies.rs`, take the loop that starts `let mut renumbered = 0usize;` and ends before
`// ⚠ The open photo may itself have been renumbered` and move it into a new private method,
**verbatim** apart from the signature and the `return` of the count:

```rust
    /// Close gaps in the copy numbering of every photo in `scan`, and let go of the undo state and
    /// catalog rows of every copy that moved. Returns how many copies were renumbered.
    ///
    /// ⚠ Shared by the folder and the Immich branches of [`Self::reindex`]. Extracted as a pure move;
    /// the folder tests were not touched, which is what says it was one.
    ///
    /// ⚠⚠ The caller must have called `persist_current_session` first — a compaction rewrites the
    /// sidecar the open photo's edits belong in, so an unsaved change would otherwise be lost or land
    /// on a renumbered copy.
    fn compact_copy_numbering(&mut self, scan: &[catalog::CatalogEntry]) -> usize {
        let mut renumbered = 0usize;
        // ... the existing body, unchanged, iterating `for e in scan`
        renumbered
    }
```

Then in the folder branch of `reindex`, replace the loop with:

```rust
        let renumbered = self.compact_copy_numbering(&scan);
```

- [ ] **Step 3: Build**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "EXIT: $?"; grep -E '^error' -A 5 /tmp/b.log | head -20
```

- [ ] **Step 4: Run the tests — the count must be identical**

```bash
cargo test --workspace copies > /tmp/after.log 2>&1; echo "EXIT: $?"; grep -c '^test ' /tmp/after.log
```

Expected: PASS, **the same count as Step 1**, and no test file edited.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/copies.rs
git commit -m "Extract the copy-numbering compaction, a pure move

The Immich branch of Reindex needs the same loop. The folder tests were not touched, which is
what says the extraction was a move and not a rewrite."
```

---

### Task 3: Reindex refetches an Immich album or month

**Files:**
- Modify: `crates/app/src/copies.rs`

- [ ] **Step 1: Write the failing tests**

Add to `copies.rs`'s test module (follow the existing helpers there; use
`crate::test_app::for_test()`):

```rust
    /// A configured server, so the Immich branch is reachable at all.
    fn with_server(app: &mut crate::RustroomApp) {
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();
    }

    fn one_asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("{id}.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
        }
    }

    /// ⚠⚠ **The one line that makes the whole feature work.** A click resolves an album from
    /// `immich_assets` *before* the validator is consulted, and that map is never invalidated — so
    /// without this removal the refetch would happen and the click would still serve the frozen list.
    #[test]
    fn reindexing_an_immich_album_drops_its_frozen_asset_list() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.immich_assets
            .insert("mine".to_string(), vec![one_asset("a")]);
        app.immich_assets
            .insert("other".to_string(), vec![one_asset("b")]);
        app.open_immich_album(&ctx, "mine".to_string(), "Mine".to_string(), &[one_asset("a")]);

        app.reindex(&ctx);

        assert!(
            !app.immich_assets.contains_key("mine"),
            "the reindexed album's cached list must go, or the refetch changes nothing"
        );
        assert!(
            app.immich_assets.contains_key("other"),
            "another album's list must be left alone — the user chose the open album, not all of them"
        );
    }

    #[test]
    fn reindexing_an_immich_month_drops_that_months_list() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.immich_assets
            .insert("2026-08-01".to_string(), vec![one_asset("a")]);
        app.open_immich_bucket(
            &ctx,
            "2026-08-01".to_string(),
            "August 2026".to_string(),
            &[one_asset("a")],
        );

        app.reindex(&ctx);

        assert!(!app.immich_assets.contains_key("2026-08-01"));
    }

    /// ⚠ Feedback at the press, not when the reply lands: the server takes a couple of seconds, and a
    /// press with no visible effect reads as a press that did nothing — reported in use for clicks.
    #[test]
    fn reindexing_an_immich_album_says_so_at_once() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.open_immich_album(&ctx, "mine".to_string(), "Mine".to_string(), &[one_asset("a")]);

        app.reindex(&ctx);

        assert!(
            app.status.contains("Immich"),
            "the status must name where it is fetching from: {}",
            app.status
        );
        assert_eq!(app.immich_opening.as_deref(), Some("mine"));
    }

    /// ⚠ No server, no fetch — and it must say why rather than appearing to do nothing.
    #[test]
    fn reindexing_an_immich_album_with_no_server_says_so() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.open_immich_album(&ctx, "mine".to_string(), "Mine".to_string(), &[one_asset("a")]);
        app.settings.immich_url = String::new();
        app.settings.immich_key = String::new();
        app.immich_assets
            .insert("mine".to_string(), vec![one_asset("a")]);

        app.reindex(&ctx);

        assert!(
            app.status.to_lowercase().contains("immich"),
            "it must say the server is missing: {}",
            app.status
        );
        assert!(
            app.immich_assets.contains_key("mine"),
            "and it must not throw the list away when it cannot refetch it"
        );
    }

    /// ⚠ The refusal message named three unsupported sources and implied Immich was a fourth. It is
    /// now supported, so the message must not say otherwise.
    #[test]
    fn reindexing_a_collection_still_refuses_and_names_immich_as_supported() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        app.library = None;

        app.reindex(&ctx);

        assert!(
            app.status.contains("Immich"),
            "the refusal must list Immich among what works: {}",
            app.status
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test --workspace reindexing_an_immich > /tmp/t.log 2>&1; echo "EXIT: $?"; grep -E '^test |FAILED|panicked' /tmp/t.log | head
```

Expected: FAIL — the status says *"Reindex works on a folder, …"* and the map still holds the key.

- [ ] **Step 3: Add the Immich branch**

Restructure `reindex`'s opening. Replace the single `let Some(LibrarySource::Folder(dir)) = … else`
guard with a match on the source, keeping the folder arm byte-for-byte as it is today:

```rust
    pub(crate) fn reindex(&mut self, ctx: &egui::Context) {
        let source = self.library.as_ref().map(|l| l.source.clone());
        match source {
            Some(LibrarySource::Folder(dir)) => self.reindex_folder(ctx, dir),
            Some(LibrarySource::ImmichAlbum { id, name }) => {
                self.reindex_immich(ctx, Job::Album { id: id.clone(), name }, id, Job::Albums)
            }
            Some(LibrarySource::ImmichBucket { bucket, label }) => self.reindex_immich(
                ctx,
                Job::Bucket {
                    bucket: bucket.clone(),
                    label,
                },
                bucket,
                Job::Buckets,
            ),
            // ⚠ A collection, a date or All Photos has no single source to re-read, and renumbering
            // photos you are merely viewing would be surprising.
            _ => {
                self.status =
                    "Reindex works on a folder or an Immich album, not a collection, a date or \
                     All Photos"
                        .into()
            }
        }
    }

    /// Reindex an Immich album or month: compact what is here, then fetch the list again.
    ///
    /// ⚠⚠ **`key` is what must leave `immich_assets`.** A click resolves an album from that map
    /// *before* the stored list's validator is consulted, and the map is never invalidated — so
    /// without this removal the refetch would land and the next click would still serve the frozen
    /// list. That one line is the feature.
    ///
    /// ⚠ `list_job` is the **matching** rail list only — the album validator comes from the album
    /// list and a month's from the bucket list, so the other is irrelevant. It is refreshed so the
    /// refetched list can be stamped with a current validator; if the two replies race and this one
    /// loses, the stamp is stale and costs exactly one extra fetch on the next launch, never wrong
    /// data.
    fn reindex_immich(
        &mut self,
        ctx: &egui::Context,
        fetch: Job,
        key: String,
        list_job: Job,
    ) -> () {
        if immich_source::connection(&self.settings).is_none() {
            self.status = "No Immich server is configured — set one in Settings".into();
            return;
        }
        // ⚠ First, and the reason clearing the session below is safe.
        self.persist_current_session();
        let scan = self
            .library
            .as_ref()
            .map(|l| l.scan.clone())
            .unwrap_or_default();
        let renumbered = self.compact_copy_numbering(&scan);

        // ⚠ Dropped rather than reopened: the open photo may no longer exist on the server, and
        // re-downloading it is not what Reindex is for. You land in the grid. Safe because the
        // session was persisted above.
        self.session = None;
        self.rename_draft = None;
        self.snapshot_rename = None;

        self.immich_assets.remove(&key);
        self.immich.request(list_job);
        self.immich_opening = Some(key);
        self.immich.request(fetch);
        self.status = if renumbered == 0 {
            "Fetching again from Immich…".to_string()
        } else {
            format!("Fetching again from Immich… (renumbered {renumbered} copies)")
        };
        let _ = ctx;
    }
```

⚠ `Job` and `immich_source` need importing in `copies.rs`; follow whatever path form the file already
uses for `rustroom_app::…`. ⚠ Rename the existing body to `reindex_folder(&mut self, ctx, dir)` and
change **nothing** inside it beyond removing the guard that now lives in the match.

⚠ `let _ = ctx;` is a placeholder — if `ctx` ends up genuinely unused, drop the parameter instead of
silencing it. Do not leave a `let _ =` in the tree.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace copies > /tmp/t.log 2>&1; echo "EXIT: $?"; grep -cE '^test ' /tmp/t.log; grep -E 'FAILED' /tmp/t.log | head
```

Expected: PASS, and the folder tests from Task 2 still passing.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/copies.rs
git commit -m "Reindex fetches an Immich album or month again

The load-bearing line is removing the album's entry from immich_assets: a click resolves an
album from that map before the stored list's validator is consulted, and the map is never
invalidated, so a refetch alone would have changed nothing.

Only the matching rail list is refreshed, because the album validator comes from the album
list and a month's from the bucket list. The two replies race; if the rail list loses, the
stamp is stale and costs one extra fetch on the next launch, never wrong data.

The open photo is dropped rather than reopened, so you land in the grid: it may no longer
exist on the server, and re-downloading it is not what Reindex is for. Safe because the
session is persisted first."
```

---

### Task 4: The hover text says what the button will do

**Files:**
- Modify: `crates/app/src/shell.rs`

⚠ This file is **1,379 lines**. Add the helper, add nothing else.

- [ ] **Step 1: Make the text source-aware**

In `more_menu`, replace the fixed `.on_hover_text(…)` with a call to a small helper placed directly
beside it:

```rust
/// What Reindex will do, in the words of whatever is open.
///
/// ⚠ The label stays "Reindex" for every source — one verb, one place. A second name for the same
/// button would be worse than a hover that adapts.
fn reindex_hover(app: &RustroomApp) -> &'static str {
    match app.library.as_ref().map(|l| &l.source) {
        Some(LibrarySource::ImmichAlbum { .. }) => {
            "Fetch this album again from Immich to pick up added or removed photos, and close any \
             gaps in the copy numbering."
        }
        Some(LibrarySource::ImmichBucket { .. }) => {
            "Fetch this month again from Immich to pick up added or removed photos, and close any \
             gaps in the copy numbering."
        }
        _ => {
            "Re-read the folder, sort it alphabetically, and close any gaps in the copy numbering."
        }
    }
}
```

and in the menu body:

```rust
        if ui.button("Reindex").on_hover_text(reindex_hover(app)).clicked() {
```

⚠ `app` is borrowed mutably by the closure. If the borrow checker objects, compute
`let hover = reindex_hover(app);` **before** `MenuButton::new(…).ui(…)` and use the local inside.

- [ ] **Step 2: Build and run the suite**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "BUILD EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/shell.rs
git commit -m "The Reindex hover says what it will do where you are

The label stays Reindex for every source: one verb, one place, and a second name for the same
button would be worse than a hover that adapts."
```

---

### Task 5: The gate

- [ ] **Step 1: Formatting, clippy, tests — each exit code read from its own command**

```bash
cargo fmt --all
cargo fmt --all -- --check > /tmp/fmt.log 2>&1; echo "FMT EXIT: $?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/clippy.log 2>&1; echo "CLIPPY EXIT: $?"
cargo test --workspace > /tmp/test.log 2>&1; echo "TEST EXIT: $?"
```

Expected: all three **0**, the **74 goldens unmoved** (this adds no pipeline stage), and no `pending_list` left:

```bash
grep -rn 'pending_list' crates/app/src/ | wc -l
```

Expected: **0**.

- [ ] **Step 2: Re-gate the tip after any fmt churn**

⚠⚠ A gate run before the last commit describes a tree nobody shipped. If `cargo fmt` changed
anything, commit it and run the three commands again.

```bash
git status --porcelain
```

---

### Task 6: Run it

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the path**

1. Open an Immich album. Note how many photos it has.
2. **In Immich, add a photo to that album** (or remove one).
3. Back in Rustroom, ⋯ → **Reindex**. The status should say it is fetching, and the grid should come
   back with the new count — **without restarting**.
4. Do the same for a Timeline month.
5. ⋯ → Reindex on a **folder** must behave exactly as before.
6. ⋯ → Reindex on a **collection** must refuse, and its message must mention Immich.

- [ ] **Step 3: Write it up**

⚠⚠ Every Immich defect so far was found by use, not by a test. Add a dated section at the top of
`docs/JOURNAL.md` and overwrite `CLAUDE.md`'s live state **in place**. ⚠ `CLAUDE.md` is at **399**
lines against a ~400 target: something must move out rather than in.

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| `pending_list` split in two; `is_listing` keeps its meaning | 1 |
| Albums+Buckets together is a regression test that fails first | 1 |
| A second Albums is still dropped | 1 |
| Compaction shared by both branches, as a pure move | 2 |
| Persist session first, then clear it; land in the grid | 3 |
| Compact over the **current** scan, before the refetch | 3 |
| Remove the album's/month's entry from `immich_assets` | 3 |
| Only the **matching** rail list is refreshed | 3 |
| Asset list requested, `immich_opening` + status set at the press | 3 |
| No new reply handler, no new `RustroomApp` state | 3 |
| Other albums' lists untouched | 3 |
| No server ⇒ says so, throws nothing away | 3 |
| Refusal message names Immich as supported | 3 |
| Source-aware hover, label unchanged | 4 |
| No golden movement | 5 |
| Seen in use | 6 |

**Known soft spots, flagged rather than hidden:**

- **Task 3's tests assert the map and the status, not the fetch.** `test_app::for_test()`'s worker has
  a stub fetcher, so what the *server* is asked is not observed here. Task 1 covers the guard that
  could drop a request; the rest is Task 6's job. ⚠ If the stub makes `immich_opening` unreachable,
  assert the map removal and the status and say so in the commit rather than weakening the test to fit.
- **`reindex_immich` takes two `Job`s.** Slightly awkward, but it keeps one body for album and month
  and puts the album/bucket asymmetry at the call site where it is visible.
- **`shell.rs` (1,379) and `immich_worker.rs` (698)** stay oversized. Reported, not split.

# Immich slice 2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Browse the whole Immich library by month, with thumbnails that survive a restart, and a size budget on both thumbnail caches so neither grows without bound.

**Architecture:** Three ordered parts. **(1)** `ThumbStore` gains a byte budget and LRU eviction — independent of Immich, and first because part 2 widens a hole open since M8.2. **(2)** `Identity` becomes an enum so a not-yet-downloaded asset can be cached without a file to stat, in its own directory with its own budget. **(3)** `Client::album` generalises to a filtered paged search, so a month's date range and an album share one tested path, and the Immich rail section gains a Timeline of years and months built from `GET /timeline/buckets`.

**Tech Stack:** Rust, `ureq` 3.4.2, `image` (with `webp`), `egui` 0.35 in bin modules only, `std::fs::File::set_times` (verified on rustc 1.98.0).

**Spec:** `docs/superpowers/specs/2026-09-23-immich-timeline-design.md`. Read it before Task 1.

---

## Conventions for every task

Run once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **Commit messages carry NO `Co-Authored-By` trailer.** **Never push.** Branch is already `immich-timeline`.
- ⚠⚠ **ONE CARGO INVOCATION MODE.** Always `cargo test --workspace <name-filter>`; **never `cargo test -p <crate>`.** Measured 2026-09-23: alternating them costs **43–531 s per switch**; staying in one mode is **1 s** warm. This is the single biggest thing that will waste your time.
- **Read each exit code from its own command.** Piping cargo through `grep` reports *grep's* status — that has produced a false green in this repo. Write `; echo "EXIT: $?"` after the cargo command itself.
- The full gate is **~79 s**: `cargo fmt --all -- --check`, then `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings`, then `cargo test --workspace`.
- **egui only in bin modules** (`immich_panel`, `modals/settings`, `immich_flow`, `main`). `thumb_store`, `immich_source`, `immich_worker` are lib modules: **no egui**.
- ⚠ Each test that touches disk gets **its own temp dir**. The seven-site collision rule stands.

## File structure

| File | Responsibility |
|---|---|
| `crates/app/src/thumb_store.rs` | ⚠ 572 lines. `Identity` enum, the budget, the LRU touch, the sweep, `immich_default` |
| `crates/app/src/settings.rs` | ⚠ 2280 lines. Two `u32` budget fields |
| `crates/app/src/modals/settings.rs` | Two budget rows, a second size readout |
| `crates/immich/src/dto.rs` | `Bucket`, `Asset::updated_at` |
| `crates/immich/src/lib.rs` | `Filter`, `search`, `buckets`; `album` becomes a wrapper |
| `crates/immich/tests/fixtures/buckets_real.json` | **Captured** from the user's server |
| `crates/app/src/immich_source.rs` | `group_buckets`, `month_range`, the store in `remote_thumbnail` |
| `crates/app/src/immich_worker.rs` | `Job::Buckets` |
| `crates/app/src/immich_panel.rs` | The Timeline node |
| `crates/app/src/immich_flow.rs` | `open_immich_bucket` |
| `crates/app/src/main.rs` | `LibrarySource::ImmichBucket`, the second cache-size field |
| `crates/app/src/thumb_loader.rs` | ⚠ 1288 lines. `new` widens to take both budgets |

---

# Part 1 — the thumbnail budget

### Task 1: Pin the current Local filename before changing anything

⚠⚠ **This task exists because the spec was wrong about it.** The spec said the file's "pinned hash vectors" would catch a change to `Local`'s key derivation. **They will not** — they pin `fnv1a` itself (`fnv1a(b"") == 0xcbf29ce484222325`) and assert filenames *differ* when parts differ. **Nothing pins a concrete `Identity` → `file_name()` mapping**, so changing `key_bytes()` would silently invalidate every cached thumbnail on the user's disk with no test failing.

**Files:**
- Modify: `crates/app/src/thumb_store.rs` (tests module)

- [ ] **Step 1: Write the pinning test**

Add to `thumb_store.rs`'s `mod tests`, next to `the_filename_changes_with_every_part_of_the_identity`:

```rust
    /// ⚠⚠ **A concrete filename, pinned on purpose.** The vectors above pin `fnv1a`; the test
    /// beside this one pins that the parts *matter*. **Neither pins the mapping itself**, so a
    /// change to `key_bytes` — adding an enum discriminant, reordering the fields, widening a
    /// type — would silently turn every thumbnail already on a user's disk into a miss, with the
    /// whole suite still green. This is the only thing standing between that change and a cache
    /// everybody silently loses.
    #[test]
    fn a_known_local_identity_keeps_its_filename_for_ever() {
        assert_eq!(
            ident("/photos/a.RAF", 100, 200).file_name(),
            "f89fcaaf26f07b00.rthm"
        );
    }
```

- [ ] **Step 2: Run it**

```bash
cargo test --workspace a_known_local_identity; echo "EXIT: $?"
```

Expected: **PASS.** This test must pass against *unchanged* code — its job is to record what the code does today.

⚠ **If it fails**, the printed "actual" value is authoritative, not the constant above: the constant was computed offline by reimplementing `fnv1a` and `key_bytes` in Python, verified against the same three published vectors, but a reimplementation is not the code. Replace the expected value with the actual and say so in your report.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/thumb_store.rs
git commit -m "Pin the concrete cache filename for a known local identity

The existing vectors pin fnv1a and the neighbouring test pins that each part of the
identity matters. Neither pins the mapping, so changing key_bytes would silently make
every thumbnail already on disk a miss with the suite still green. The next commits
change Identity, so this goes in first."
```

---

### Task 2: The two budget fields in Settings

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing test**

Add to `settings.rs`'s `mod tests`:

```rust
    /// ⚠⚠ The trap: a field-level `#[serde(default)]` on a `u32` yields **0**, i.e. a cache that
    /// evicts everything it writes. The container-level default must supply 5120.
    #[test]
    fn a_settings_file_from_before_the_thumbnail_budgets_loads_them_at_5120() {
        let f = TempFile::new("thumb_budget_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.thumb_budget_mb, 5120);
        assert_eq!(loaded.immich_thumb_budget_mb, 5120);
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test --workspace thumbnail_budgets; echo "EXIT: $?"
```

Expected: FAIL to compile — `thumb_budget_mb` does not exist.

- [ ] **Step 3: Add the fields**

In `pub struct Settings`, after `immich_key`:

```rust
    /// Byte budget for the **local** thumbnail cache, in megabytes.
    ///
    /// ⚠⚠ No field-level `#[serde(default)]`. The struct carries a container-level one, so a
    /// missing field falls back to this struct's `Default` — 5120. A field-level one would use
    /// `u32`'s own default of **0**, turning the cache into a device that deletes every entry it
    /// writes. The same trap `write_xmp`, `auto_advance` and `gpu_tail` each document.
    ///
    /// ⚠ Megabytes rather than gigabytes so sub-gigabyte is expressible, and because the Settings
    /// row already displays MB.
    pub thumb_budget_mb: u32,
    /// Byte budget for the **Immich** thumbnail cache, in megabytes. Same trap, same reason.
    ///
    /// ⚠ A separate budget on a separate directory, so neither sweep can evict the other's
    /// entries.
    pub immich_thumb_budget_mb: u32,
```

In `impl Default for Settings`, beside `immich_url`:

```rust
            thumb_budget_mb: 5120,
            immich_thumb_budget_mb: 5120,
```

- [ ] **Step 4: Add them to the exhaustive round-trip literal**

⚠ `settings_round_trip_carries_the_export_options` is **deliberately exhaustive** — its comment says a new field must land there as a compile error forcing a decision. Add, beside `immich_key`, non-default values so the round trip proves they persist:

```rust
            // Non-default so a field re-defaulted on load cannot pass.
            thumb_budget_mb: 2048,
            immich_thumb_budget_mb: 1024,
```

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace settings::; echo "EXIT: $?"
```

Expected: PASS, including `settings_round_trip_carries_the_export_options`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "Two thumbnail-cache budgets in Settings, 5120 MB each

Megabytes so sub-gigabyte is expressible. No field-level serde(default): on a u32 it
would yield 0, a cache that deletes everything it writes."
```

---

### Task 3: The sweep

**Files:**
- Modify: `crates/app/src/thumb_store.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    /// Write `n` entries of roughly `each` bytes, oldest first, and return their paths in the
    /// order written. ⚠ mtimes are set explicitly rather than relying on write order: a
    /// filesystem with coarse timestamps would otherwise make "oldest" arbitrary.
    fn fill(store: &ThumbStore, n: usize, each: usize) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for i in 0..n {
            let id = ident(&format!("/photos/{i}.RAF"), i as i64, each as u64);
            let t = Thumbnail {
                width: 1,
                height: 1,
                rgba: vec![0u8; 4],
            };
            store.put(&id, &t);
            let p = store.dir().join(id.file_name());
            // Pad to a known size so the budget arithmetic is predictable.
            let mut bytes = std::fs::read(&p).unwrap();
            bytes.resize(each.max(bytes.len()), 0);
            std::fs::write(&p, &bytes).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            let when = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_000_000 + i as u64 * 60);
            f.set_times(std::fs::FileTimes::new().set_modified(when)).unwrap();
            out.push(p);
        }
        out
    }

    #[test]
    fn a_sweep_brings_an_over_budget_directory_under_it() {
        let dir = TempDir::new("sweep_under");
        // 10 entries of ~10 KB against a 50 KB budget.
        let store = ThumbStore::new(dir.0.clone(), 50 * 1024);
        let paths = fill(&store, 10, 10 * 1024);
        store.sweep();
        assert!(
            store.size_bytes() <= 50 * 1024,
            "still {} bytes",
            store.size_bytes()
        );
        // ⚠ The NEWEST must survive — a cache that kept the oldest would be useless.
        assert!(paths.last().unwrap().exists(), "the newest entry was evicted");
        assert!(!paths[0].exists(), "the oldest entry survived");
    }

    /// ⚠ A budget of 0 must not panic or wipe the directory in a loop.
    #[test]
    fn a_zero_budget_empties_the_store_without_panicking() {
        let dir = TempDir::new("sweep_zero");
        let store = ThumbStore::new(dir.0.clone(), 0);
        fill(&store, 3, 1024);
        store.sweep();
        assert_eq!(store.size_bytes(), 0);
    }

    /// ⚠ A `.tmp` left by an interrupted `put` is litter, not a cache entry: deleted on sight
    /// rather than counted towards the budget.
    #[test]
    fn a_leftover_tmp_is_deleted_by_a_sweep() {
        let dir = TempDir::new("sweep_tmp");
        let store = ThumbStore::new(dir.0.clone(), 1024 * 1024);
        std::fs::create_dir_all(&dir.0).unwrap();
        let stray = dir.0.join("deadbeefdeadbeef.rthm.tmp");
        std::fs::write(&stray, vec![0u8; 4096]).unwrap();
        store.sweep();
        assert!(!stray.exists(), "a stray .tmp survived the sweep");
    }

    /// ⚠ An absent directory is not an error — the store must work before anything is cached.
    #[test]
    fn a_sweep_on_an_absent_directory_does_nothing() {
        let dir = TempDir::new("sweep_absent");
        let store = ThumbStore::new(dir.0.join("not-created-yet"), 1024);
        store.sweep();
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace sweep; echo "EXIT: $?"
```

Expected: FAIL — `ThumbStore::new` takes one argument and `sweep` does not exist.

- [ ] **Step 3: Add the budget and the sweep**

Change the struct and constructors:

```rust
pub struct ThumbStore {
    dir: PathBuf,
    /// Bytes this directory may hold. `u64::MAX` means "no limit".
    budget: u64,
    /// Bytes written since the last sweep.
    ///
    /// ⚠⚠ A **deliberate, documented departure** from "holds a `PathBuf` and nothing else". An
    /// atomic keeps the store `Send + Sync` with no lock, which is what lets six loader workers
    /// keep sharing one. It is a counter, not an index — the filesystem is still the index.
    written: std::sync::atomic::AtomicU64,
}

impl ThumbStore {
    pub fn new(dir: PathBuf, budget_bytes: u64) -> Self {
        let s = Self {
            dir,
            budget: budget_bytes,
            written: std::sync::atomic::AtomicU64::new(0),
        };
        // ⚠ Once at construction, so a store left over budget by a lowered setting is corrected
        // without waiting for enough writes to trigger a sweep.
        s.sweep();
        s
    }

    pub fn user_default(budget_bytes: u64) -> Self {
        Self::new(Self::user_dir(), budget_bytes)
    }

    /// The Immich thumbnail cache — a **sibling** directory.
    ///
    /// ⚠⚠ Separate from the local one on purpose: separate accounting is then just `size_bytes()`
    /// per directory, and neither budget's sweep can reach the other's entries.
    pub fn immich_default(budget_bytes: u64) -> Self {
        Self::new(Self::user_dir().join("immich"), budget_bytes)
    }
```

✅ **Read, not guessed:** `user_default` currently builds
`dirs::config_dir()/rustroom/thumbs`. Extract exactly that into a private
`user_dir() -> PathBuf` and have both defaults use it, so `immich_default` is
`user_dir().join("immich")`.

⚠ **The test helper changes too.** `mod tests` has `struct TempDir` (process id **plus a
counter**, because "two fixtures built in the same tick have collided in this repo before") with a
`store()` method that calls `ThumbStore::new(self.0.clone())`. Give it a budget:
`fn store(&self, budget: u64) -> ThumbStore` — and update its existing callers.

Add the sweep:

```rust
    /// Delete oldest-first until the directory is within budget.
    ///
    /// ⚠ Best-effort throughout: an entry that will not delete is skipped, exactly as `clear()`
    /// already does. A sweep must never be able to stop a folder opening.
    pub fn sweep(&self) {
        if self.budget == u64::MAX {
            return;
        }
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> = Vec::new();
        let mut total: u64 = 0;
        for e in d.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if !m.is_file() {
                continue;
            }
            // ⚠ Litter from an interrupted `put`, not an entry: removed rather than counted.
            if p.extension().is_some_and(|x| x == "tmp") {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            let when = m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total += m.len();
            entries.push((p, when, m.len()));
        }
        if total <= self.budget {
            return;
        }
        // Oldest first. ⚠ mtime is the *touch* time, not the write time — `get` updates it, which
        // is what makes this LRU rather than FIFO.
        entries.sort_by_key(|(_, when, _)| *when);
        for (p, _, len) in entries {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace sweep; echo "EXIT: $?"
```

Expected: PASS, 4 tests. ⚠ The existing `ThumbStore::new` call sites will not compile yet — that is Task 5. Run only the filter above for now.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/thumb_store.rs
git commit -m "A byte budget and an oldest-first sweep for the thumbnail store

ThumbStore has had no eviction since M8.2 — no budget, no prune, only clear(). The
sweep runs at construction and deletes oldest-first until the directory is within
budget, skipping anything it cannot remove, because a sweep must never be able to stop
a folder opening. A stray .tmp from an interrupted put is litter and is deleted rather
than counted.

The bytes-written counter is an AtomicU64: a deliberate departure from 'holds a PathBuf
and nothing else', because an atomic keeps the store Send + Sync with no lock, which is
what lets six loader workers share one. It is a counter, not an index."
```

---

### Task 4: The LRU touch, and the write-triggered sweep

**Files:**
- Modify: `crates/app/src/thumb_store.rs`

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ **The difference between LRU and FIFO, and it is the whole point.** Without a touch on
    /// read, an entry's mtime is when it was *written*, so a sweep evicts the folder you revisit
    /// most. This test reads the oldest entry and then requires it to survive a sweep that must
    /// evict something.
    #[test]
    fn reading_an_entry_saves_it_from_the_next_sweep() {
        let dir = TempDir::new("lru_touch");
        let store = ThumbStore::new(dir.0.clone(), 30 * 1024);
        let paths = fill(&store, 4, 10 * 1024);
        // The oldest entry, by construction.
        let oldest = ident("/photos/0.RAF", 0, 10 * 1024);
        assert!(store.get(&oldest).is_some(), "the entry must be readable first");
        store.sweep();
        assert!(
            paths[0].exists(),
            "the entry that was just read was evicted anyway — the touch is not working"
        );
        assert!(!paths[1].exists(), "an untouched older entry should have gone");
    }

    /// ⚠ Enough writes must trigger a sweep on their own, or a session that never reopens the
    /// store could exceed the budget without limit.
    #[test]
    fn writing_past_a_tenth_of_the_budget_triggers_a_sweep() {
        let dir = TempDir::new("write_trigger");
        let store = ThumbStore::new(dir.0.clone(), 40 * 1024);
        fill(&store, 12, 10 * 1024);
        // No explicit sweep call: the puts themselves must have kept it in hand.
        assert!(
            store.size_bytes() <= 40 * 1024,
            "no sweep was triggered by writing: {} bytes",
            store.size_bytes()
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace 'lru_touch|reading_an_entry|writing_past'; echo "EXIT: $?"
```

Expected: FAIL — nothing touches on read and nothing sweeps on write.

- [ ] **Step 3: Touch on a hit**

In `get`, after the entry has decoded successfully and **before** returning `Some`:

```rust
        // ⚠⚠ This one syscall is what makes eviction LRU instead of FIFO. Without it an entry's
        // mtime is its write time, so the sweep would discard the folder you revisit most —
        // the exact opposite of what a cache is for. Negligible against a measured 1.26 ms read.
        //
        // ⚠ Best-effort: a read-only cache directory must still serve hits.
        if let Ok(f) = std::fs::File::options().write(true).open(self.dir.join(id.file_name())) {
            let _ = f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
        }
```

- [ ] **Step 4: Sweep when enough has been written**

At the end of `put`, after the successful rename:

```rust
        // ⚠ Not a sweep per `put`: `size_bytes()` walks the directory, so that would be a
        // directory scan per thumbnail. Amortised instead — sweep once per tenth of the budget
        // written, which bounds the overshoot without an index.
        let n = bytes.len() as u64;
        let before = self
            .written
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        let slack = (self.budget / 10).max(1);
        if before + n >= slack {
            self.written.store(0, std::sync::atomic::Ordering::Relaxed);
            self.sweep();
        }
```

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace 'reading_an_entry|writing_past|sweep'; echo "EXIT: $?"
```

Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/thumb_store.rs
git commit -m "Eviction is LRU, by touching an entry's mtime on a hit

Without the touch an entry's mtime is its write time, so eviction is FIFO and the
sweep discards the folder you revisit most — the opposite of what a cache is for. One
syscall against a measured 1.26 ms read. Best-effort, so a read-only cache directory
still serves hits.

Writes amortise their own sweeps: once per tenth of the budget written, never per put,
because size_bytes walks the directory."
```

---

### Task 5: Wire the budgets through `ThumbLoader` and the Settings row

**Files:**
- Modify: `crates/app/src/thumb_loader.rs`, `crates/app/src/sources.rs`, `crates/app/src/settings_apply.rs`, `crates/app/src/test_app.rs`, `crates/app/examples/bench_library_open.rs`, `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Widen `ThumbLoader::new`**

```rust
    pub fn new(
        display_space: color::OutputSpace,
        thumb_budget_bytes: u64,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
```

and inside it `let store = crate::thumb_store::ThumbStore::user_default(thumb_budget_bytes);`

⚠ **Five call sites, counted:** `sources.rs:200`, `settings_apply.rs:59`, `test_app.rs:297`, `thumb_loader.rs:630` (a test), `examples/bench_library_open.rs:35`. The two app ones pass `self.settings.thumb_budget_mb as u64 * 1024 * 1024`; the three test/bench ones pass `u64::MAX` (no limit — a test must not evict).

⚠ Several in-code comments say `ThumbLoader::new` has "four call sites". That count is stale; do not copy it into anything new.

- [ ] **Step 2: Settings UI — the local budget beside the existing row**

In `modals/settings.rs`, in the Performance tab's "Thumbnail cache" block (~line 329), the store construction becomes
`ThumbStore::user_default(d.thumb_budget_mb as u64 * 1024 * 1024)` and a slider is added under the size row:

```rust
                    ui.add(
                        egui::Slider::new(&mut d.thumb_budget_mb, 512..=51200)
                            .step_by(512.0)
                            .text("MB budget"),
                    )
                    .on_hover_text(
                        "How much disk the thumbnail cache may use. The least recently seen \
                         entries are deleted when it is exceeded. Takes effect at the next folder \
                         open, not on Apply — rebuilding the loader now would throw away the open \
                         folder's thumbnails.",
                    );
```

Add `thumb_budget_mb` and `immich_thumb_budget_mb` to `SettingsDraft`, to `from_settings`, and to `apply` (plain fields, setting `plain = true` when they differ).

- [ ] **Step 3: Build and run the whole suite**

```bash
cargo build -p app; echo "BUILD EXIT: $?"
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: both 0. ⚠ `test_app.rs` constructs `RustroomApp` exhaustively — a compile error there is only visible to `cargo test`, never `cargo build`.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src crates/app/examples
git commit -m "Wire the local thumbnail budget from Settings to the store

ThumbLoader::new takes the budget rather than reading Settings: the store is a lib
module and must not depend on the settings loader. Five call sites — the three tests
and the bench pass u64::MAX, because a test must not evict.

A budget change takes effect at the next folder open, which is what rebuilds the
loader. Rebuilding it on Apply instead would drop the open folder's whole thumbnail
cache, about 34 s on an 830-photo folder."
```

---

# Part 2 — persisted Immich thumbnails

### Task 6: `Identity` becomes an enum

**Files:**
- Modify: `crates/app/src/thumb_store.rs`

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ A remote identity must need **no filesystem at all** — that is the entire reason for the
    /// enum. `Identity::of` stats a file, and an Immich asset that has not been downloaded has
    /// none.
    #[test]
    fn a_remote_identity_needs_no_file_and_changes_with_its_version() {
        let a = Identity::remote("11111111-1111-4111-8111-111111111111", "2026-04-27T12:34:32.784Z");
        let b = Identity::remote("11111111-1111-4111-8111-111111111111", "2026-04-27T12:34:33.000Z");
        let c = Identity::remote("22222222-2222-4222-8222-222222222222", "2026-04-27T12:34:32.784Z");
        assert_ne!(a.file_name(), b.file_name(), "a new version is a new entry");
        assert_ne!(a.file_name(), c.file_name(), "a different asset is a different entry");
        assert_eq!(
            a.file_name(),
            Identity::remote("11111111-1111-4111-8111-111111111111", "2026-04-27T12:34:32.784Z")
                .file_name()
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace a_remote_identity; echo "EXIT: $?"
```

Expected: FAIL — `Identity::remote` does not exist.

- [ ] **Step 3: Make it an enum**

```rust
pub enum Identity {
    /// A source file at a particular version.
    Local {
        path: PathBuf,
        /// Nanoseconds since the Unix epoch. **Signed**, so a pre-1970 timestamp is
        /// representable rather than an error.
        mtime_ns: i64,
        size: u64,
    },
    /// A remote asset at a particular version.
    ///
    /// ⚠ `version` is Immich's `updatedAt`, not its `thumbhash`. `thumbhash` tracks appearance
    /// more tightly but is a tiny perceptual hash that could survive a re-encode unchanged —
    /// under-invalidating. `updatedAt` over-invalidates instead (a favourite toggle costs one
    /// refetch), which matches this file's rule that an entry is never cached under a guessed
    /// identity.
    Remote { asset_id: String, version: String },
}

impl Identity {
    pub fn remote(asset_id: &str, version: &str) -> Self {
        Identity::Remote {
            asset_id: asset_id.to_string(),
            version: version.to_string(),
        }
    }

    fn key_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            // ⚠⚠ **BYTE-IDENTICAL to the pre-enum version, and it must stay that way.** Any
            // change here turns every thumbnail already on a user's disk into a miss.
            // `a_known_local_identity_keeps_its_filename_for_ever` is what guards it.
            //
            // ⚠ No discriminant byte is added, deliberately: `Remote` entries live in a
            // different directory, so the two variants can never collide and a discriminant
            // would buy nothing while breaking every existing `Local` key.
            Identity::Local {
                path,
                mtime_ns,
                size,
            } => {
                v.extend_from_slice(path.to_string_lossy().as_bytes());
                v.push(0);
                v.extend_from_slice(&mtime_ns.to_le_bytes());
                v.extend_from_slice(&size.to_le_bytes());
            }
            Identity::Remote { asset_id, version } => {
                v.extend_from_slice(asset_id.as_bytes());
                v.push(0);
                v.extend_from_slice(version.as_bytes());
            }
        }
        v
    }
```

`Identity::of(path)` now returns `Identity::Local { .. }`. The test helper `ident(path, mtime, size)` builds `Identity::Local { .. }`. ⚠ `decode_header` compares identities — it must keep comparing by value, so the enum needs `PartialEq` as before.

- [ ] **Step 4: Run the store's whole test module**

```bash
cargo test --workspace thumb_store; echo "EXIT: $?"
```

Expected: PASS, including `a_known_local_identity_keeps_its_filename_for_ever`. ⚠⚠ **If that one fails, `Local`'s bytes changed — fix the code, never the constant.**

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/thumb_store.rs
git commit -m "Identity becomes an enum so a remote asset can be cached with no file to stat

Local's key bytes are byte-identical to the pre-enum version and no discriminant is
added: Remote entries live in a different directory so the variants cannot collide, and
a discriminant would break every existing Local key for nothing. The pinned filename
test from the first commit is what guards that.

version is updatedAt rather than thumbhash: thumbhash tracks appearance more tightly
but could survive a re-encode unchanged, which under-invalidates. updatedAt
over-invalidates instead, which matches this file's rule about guessed identities."
```

---

### Task 7: `Asset::updated_at`, and the store in `remote_thumbnail`

**Files:**
- Modify: `crates/immich/src/dto.rs`, `crates/app/src/immich_source.rs`, `crates/app/src/thumb_loader.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing test**

In `crates/immich/src/dto.rs`'s tests:

```rust
    /// ⚠ `updatedAt` is what versions a cached thumbnail, so it must really be present. The
    /// captured reply confirms it.
    #[test]
    fn a_real_reply_carries_updated_at() {
        let r: SearchResponse = serde_json::from_str(SEARCH_REAL).expect("must parse");
        assert_eq!(r.assets.items[0].updated_at, "2026-04-27T12:34:32.784Z");
    }
```

In `crates/app/src/immich_source.rs`'s tests:

```rust
    /// ⚠⚠ A cached thumbnail must be served without touching the network. This is the whole
    /// point of persisting them.
    #[test]
    fn a_cached_remote_thumbnail_is_served_from_the_store() {
        // ⚠⚠ `immich_source`'s test module has **no** `TempDir` helper — that lives in
        // `thumb_store`'s. Its two existing temp dirs are built from the process id **alone**,
        // which is precisely the collision `thumb_store::TempDir`'s doc says has already bitten
        // this repo ("two fixtures built in the same tick have collided"). Use a counter here,
        // and say in your report that the two existing ones should get one too.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustroom_immich_store_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::thumb_store::ThumbStore::new(dir.clone(), u64::MAX);
        let id = crate::thumb_store::Identity::remote("abc", "v1");
        let t = core_image::Thumbnail {
            width: 2,
            height: 2,
            rgba: vec![7u8; 16],
        };
        store.put(&id, &t);
        let got = store.get(&id).expect("a stored remote thumbnail must come back");
        assert_eq!((got.width, got.height), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace 'updated_at|remote_thumbnail_is_served'; echo "EXIT: $?"
```

Expected: FAIL — `Asset` has no `updated_at`.

- [ ] **Step 3: Add the field and the store lookup**

In `dto.rs`'s `Asset`:

```rust
    /// When the asset record last changed. ⚠ Used as the cache version for its thumbnail — see
    /// `thumb_store::Identity::Remote`.
    pub updated_at: String,
```

In `immich_source.rs`, `remote_thumbnail` gains the store:

```rust
pub fn remote_thumbnail(
    shared: &SharedConnection,
    store: &crate::thumb_store::ThumbStore,
    path: &Path,
    version: &str,
) -> Option<core_image::Thumbnail> {
    if is_cached(path) {
        return None;
    }
    let conn = shared.lock().ok()?.clone()?;
    if !is_immich_path(&cache_root(&conn.url), path) {
        return None;
    }
    let id = asset_id_of(path)?;
    let key = crate::thumb_store::Identity::remote(&id, version);
    // ⚠ The store first. A hit is a local read against an HTTP round trip.
    if let Some(t) = store.get(&key) {
        return Some(t);
    }
    let client = immich::Client::new(&conn.url, &conn.key).ok()?;
    let bytes = client.thumbnail(&id, immich::Size::Thumbnail).ok()?;
    let thumb = decode_thumbnail(&bytes)?;
    store.put(&key, &thumb);
    Some(thumb)
}
```

⚠ `version` has to reach the loader. `ThumbRequest` carries only `id` and `edits`, so put the version in a side map: extend the shared cell to `Arc<Mutex<Option<Connection>>>` **plus** an `Arc<Mutex<HashMap<String, String>>>` of asset id → `updatedAt`, filled by `entries_from_assets`. ⚠ Read `immich_source::shared()` before writing this and follow the same `OnceLock` idiom; do not widen `ThumbLoader::new` a second time.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace 'updated_at|remote_thumbnail|immich_source'; echo "EXIT: $?"
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/immich/src crates/app/src
git commit -m "Immich thumbnails persist to their own store and survive a restart

remote_thumbnail consults the store before fetching and writes on success, keyed on
(asset id, updatedAt). Album thumbnails previously re-fetched over HTTP after every
restart, because the on-disk store could not key an absent file."
```

---

### Task 8: The Immich cache's own budget row

**Files:**
- Modify: `crates/app/src/modals/settings.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Add the second size field**

On `RustroomApp`, beside `thumb_cache_bytes`:

```rust
    /// The Immich thumbnail cache's size, computed when the Settings dialog opens.
    ///
    /// ⚠ Its own field rather than a sum: the two caches have separate budgets, so a single
    /// number could not tell you which one is full.
    immich_thumb_cache_bytes: Option<u64>,
```

and `immich_thumb_cache_bytes: None` at the initialiser and in `test_app.rs`.

- [ ] **Step 2: Add the row to the Immich tab**

Mirroring the Performance tab's block, in the Immich tab under the connection rows:

```rust
                    ui.add_space(GAP);
                    theme::sub_label(ui, "Thumbnail cache");
                    ui.horizontal(|ui| {
                        let store = rustroom_app::thumb_store::ThumbStore::immich_default(
                            d.immich_thumb_budget_mb as u64 * 1024 * 1024,
                        );
                        let bytes = *app
                            .immich_thumb_cache_bytes
                            .get_or_insert_with(|| store.size_bytes());
                        ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                        if ui
                            .button("Clear")
                            .on_hover_text(
                                "Thumbnails downloaded from Immich, so an album you have opened \
                                 before fills in without asking the server again. Clearing costs \
                                 nothing but the time to fetch them again. Takes effect at once, \
                                 not on Apply.",
                            )
                            .clicked()
                        {
                            store.clear();
                            app.immich_thumb_cache_bytes = Some(0);
                        }
                    });
                    ui.add(
                        egui::Slider::new(&mut d.immich_thumb_budget_mb, 512..=51200)
                            .step_by(512.0)
                            .text("MB budget"),
                    )
                    .on_hover_text(
                        "How much disk Immich thumbnails may use. Separate from the local cache's \
                         budget, so neither can evict the other's entries.",
                    );
```

⚠ `size_bytes()` walks the directory, so it is computed through `get_or_insert_with` when the dialog opens and **never** in a dialog body — a body runs sixty times a second.

- [ ] **Step 3: Build and test**

```bash
cargo build -p app; echo "BUILD EXIT: $?"
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: both 0.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src
git commit -m "The Immich thumbnail cache gets its own size readout, Clear and budget

Its own field rather than a sum of the two caches: they have separate budgets, so one
number could not say which is full."
```

---

# Part 3 — the timeline

### Task 9: `Filter`, `search` and `buckets` in the client

**Files:**
- Create: `crates/immich/tests/fixtures/buckets_real.json`
- Modify: `crates/immich/src/dto.rs`, `crates/immich/src/lib.rs`

- [ ] **Step 1: Save the captured fixture**

⚠ **Captured** from the user's server on 2026-09-23, truncated to the first six months:

```json
[
  { "timeBucket": "2026-08-01", "count": 76 },
  { "timeBucket": "2026-07-01", "count": 233 },
  { "timeBucket": "2026-06-01", "count": 45 },
  { "timeBucket": "2026-05-01", "count": 436 },
  { "timeBucket": "2026-04-01", "count": 146 },
  { "timeBucket": "2025-12-01", "count": 189 }
]
```

⚠ The 2026-03 through 2026-01 rows are dropped deliberately so the fixture also exercises a **gap between months** — a tree builder that assumed contiguity would pass on a dense fixture and fail on a real library.

- [ ] **Step 2: Write the failing tests**

In `dto.rs`'s tests:

```rust
    /// ⚠⚠ **Captured**, not constructed: a real `GET /timeline/buckets` reply. It is the only
    /// evidence that buckets are monthly and newest-first — the endpoint takes no size parameter,
    /// so granularity is the server's choice and the schema does not state it.
    const BUCKETS_REAL: &str = include_str!("../tests/fixtures/buckets_real.json");

    #[test]
    fn a_real_buckets_reply_is_monthly_newest_first_with_counts() {
        let v: Vec<Bucket> = serde_json::from_str(BUCKETS_REAL).expect("buckets must parse");
        assert_eq!(v.len(), 6);
        assert_eq!(v[0].time_bucket, "2026-08-01");
        assert_eq!(v[0].count, 76);
        assert!(
            v.iter().all(|b| b.time_bucket.ends_with("-01")),
            "every bucket must be the first of a month"
        );
        assert!(
            v[0].time_bucket > v[1].time_bucket,
            "the server returns newest first"
        );
    }
```

In `lib.rs`'s tests:

```rust
    #[test]
    fn an_album_filter_and_a_date_filter_are_different_bodies() {
        let a = Filter::Album("6f2e1a94-0c44-4f51-8a7d-9b1c2e3f4a5b".into());
        let d = Filter::TakenBetween {
            after: "2025-11-01T00:00:00.000Z".into(),
            before: "2025-12-01T00:00:00.000Z".into(),
        };
        let ab = a.body(PAGE_SIZE);
        let db = d.body(PAGE_SIZE);
        assert!(ab.get("albumIds").is_some(), "an album filter sends albumIds");
        assert!(ab.get("takenAfter").is_none());
        assert_eq!(db["takenAfter"], "2025-11-01T00:00:00.000Z");
        assert_eq!(db["takenBefore"], "2025-12-01T00:00:00.000Z");
        assert!(db.get("albumIds").is_none());
        assert_eq!(db["size"], PAGE_SIZE);
    }
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test --workspace 'buckets_reply|different_bodies'; echo "EXIT: $?"
```

Expected: FAIL — `Bucket` and `Filter` do not exist.

- [ ] **Step 4: Implement**

In `dto.rs`:

```rust
/// One row of `GET /timeline/buckets`.
///
/// ⚠ `time_bucket` is `YYYY-MM-DD`, and on a real 3.2.0 server it is always the **first of a
/// month** — the endpoint takes no size parameter, so the granularity is the server's choice.
/// Captured and pinned rather than assumed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub time_bucket: String,
    pub count: u32,
}
```

In `lib.rs`, replacing `album`'s body:

```rust
/// What a search is scoped to.
pub enum Filter {
    /// ⚠ `albumIds` is deprecated in 3.2.0 and is still the only way to scope a search to an
    /// album — see `Client::search`.
    Album(String),
    /// RFC 3339 bounds: `after` inclusive, `before` exclusive.
    TakenBetween { after: String, before: String },
}

impl Filter {
    /// The request body for one page. ⚠ Public to the crate's tests so the two shapes can be
    /// asserted without a server.
    pub(crate) fn body(&self, size: u32) -> serde_json::Value {
        match self {
            Filter::Album(id) => serde_json::json!({ "albumIds": [id], "size": size }),
            Filter::TakenBetween { after, before } => serde_json::json!({
                "takenAfter": after,
                "takenBefore": before,
                "size": size,
            }),
        }
    }
}
```

`Client::album(id)` becomes `self.search(&Filter::Album(id.to_string()))`, and `search` is the old paged loop with `filter.body(PAGE_SIZE)` in place of the inline `json!`. ⚠ The `nextPage`-before-`nextCursor` rule and the 1000-round-trip ceiling move unchanged — do not rewrite them.

Add:

```rust
    /// `GET /timeline/buckets` — the whole library's month list with counts, in one call.
    pub fn buckets(&self) -> Result<Vec<Bucket>, Error> {
        self.get_json("/timeline/buckets")
    }
```

- [ ] **Step 5: Run the crate's tests**

```bash
cargo test --workspace immich; echo "EXIT: $?"
```

Expected: PASS, including every slice-1 test — `album` must still behave identically.

- [ ] **Step 6: Commit**

```bash
git add crates/immich
git commit -m "The client takes a Filter, so albums and date ranges share one paged path

album(id) is now a wrapper over search(&Filter::Album(..)), and the paging rules move
unchanged: nextPage before nextCursor, and the round-trip ceiling.

buckets_real.json is captured from a real server. It is the only evidence that buckets
are monthly and newest-first — the endpoint takes no size parameter and the schema does
not say. Three months are dropped from it deliberately so a gap between months is
exercised; a tree builder that assumed contiguity would pass on a dense fixture."
```

---

### Task 10: `group_buckets` and `month_range`

**Files:**
- Modify: `crates/app/src/immich_source.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    fn bucket(s: &str, n: u32) -> immich::Bucket {
        immich::Bucket {
            time_bucket: s.to_string(),
            count: n,
        }
    }

    /// ⚠⚠ Reversed to **ascending** to match the local Dates section: `capture_days()` orders
    /// ascending, and two date trees in one panel reading in opposite directions reads as a bug.
    #[test]
    fn group_buckets_reverses_to_ascending_and_groups_by_year() {
        let v = vec![
            bucket("2026-08-01", 76),
            bucket("2026-07-01", 233),
            bucket("2025-12-01", 189),
        ];
        let years = group_buckets(&v);
        assert_eq!(years.len(), 2);
        assert_eq!(years[0].year, 2025, "oldest year first");
        assert_eq!(years[0].count, 189);
        assert_eq!(years[1].year, 2026);
        assert_eq!(years[1].count, 76 + 233);
        assert_eq!(
            years[1].months.iter().map(|m| m.month).collect::<Vec<_>>(),
            vec![7, 8],
            "months ascending within the year"
        );
    }

    /// ⚠ A gap between months must not merge them or drop one.
    #[test]
    fn group_buckets_survives_a_gap_between_months() {
        let v = vec![bucket("2026-08-01", 5), bucket("2026-04-01", 7)];
        let years = group_buckets(&v);
        assert_eq!(years.len(), 1);
        assert_eq!(
            years[0].months.iter().map(|m| m.month).collect::<Vec<_>>(),
            vec![4, 8]
        );
    }

    /// ⚠ One malformed row must not empty the tree.
    #[test]
    fn a_malformed_bucket_is_skipped_not_fatal() {
        let v = vec![bucket("not-a-date", 9), bucket("2026-04-01", 7)];
        let years = group_buckets(&v);
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].count, 7);
    }

    /// ⚠⚠ Month arithmetic is where this goes quietly wrong: December must roll the year.
    #[test]
    fn a_month_range_rolls_december_into_january() {
        let (a, b) = month_range(2025, 12).expect("December must be a valid month");
        assert_eq!(a, "2025-12-01T00:00:00.000Z");
        assert_eq!(b, "2026-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_month_range_is_exclusive_at_the_end_and_handles_february() {
        let (a, b) = month_range(2024, 2).expect("February must be a valid month");
        assert_eq!(a, "2024-02-01T00:00:00.000Z");
        // ⚠ The next month's first instant, so leap years need no special case at all.
        assert_eq!(b, "2024-03-01T00:00:00.000Z");
    }

    #[test]
    fn an_impossible_month_is_none() {
        assert!(month_range(2025, 0).is_none());
        assert!(month_range(2025, 13).is_none());
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace 'group_buckets|month_range|malformed_bucket'; echo "EXIT: $?"
```

Expected: FAIL — neither function exists.

- [ ] **Step 3: Implement**

```rust
/// One year of Immich buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketYear {
    pub year: i32,
    pub count: u32,
    pub months: Vec<BucketMonth>,
}

/// One month of Immich buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketMonth {
    pub month: u32,
    pub count: u32,
    /// The bucket identifier the server gave, passed back verbatim when the month is opened.
    pub bucket: String,
}

/// Group the server's buckets into years and months, **ascending**.
///
/// ⚠⚠ Not `date_tree::group`. Immich's buckets are monthly, and pushing them through that
/// function would produce a day node meaning "the whole month" — a lie in the data rather than a
/// rendering detail. Twenty lines is cheaper than the lie, and `date_tree` stays untouched.
///
/// ⚠ Ascending to match the local Dates section, whose `capture_days()` orders ascending.
///
/// ⚠ A row whose `time_bucket` is not `YYYY-MM-DD` is **skipped**: one bad row must not empty the
/// tree.
pub fn group_buckets(buckets: &[immich::Bucket]) -> Vec<BucketYear> {
    let mut rows: Vec<(i32, u32, u32, String)> = buckets
        .iter()
        .filter_map(|b| {
            let s = &b.time_bucket;
            if s.len() < 7 {
                return None;
            }
            let year: i32 = s.get(0..4)?.parse().ok()?;
            let month: u32 = s.get(5..7)?.parse().ok()?;
            if !(1..=12).contains(&month) {
                return None;
            }
            Some((year, month, b.count, s.clone()))
        })
        .collect();
    rows.sort_by_key(|(y, m, _, _)| (*y, *m));

    let mut out: Vec<BucketYear> = Vec::new();
    for (year, month, count, bucket) in rows {
        if out.last().map(|y| y.year) != Some(year) {
            out.push(BucketYear {
                year,
                count: 0,
                months: Vec::new(),
            });
        }
        let y = out.last_mut().expect("just pushed");
        y.count += count;
        y.months.push(BucketMonth {
            month,
            count,
            bucket,
        });
    }
    out
}

/// The RFC 3339 bounds of one month: start inclusive, end exclusive.
///
/// ⚠⚠ December must roll the year, which is the one place this goes quietly wrong.
/// ⚠ The end is the **next month's first instant**, so leap years need no special case.
/// ⚠ UTC, matching what the server sends. A photo near a month boundary may therefore fall in the
/// neighbouring month relative to its local time; `MetadataSearchDto` offers no local-time filter.
pub fn month_range(year: i32, month: u32) -> Option<(String, String)> {
    if !(1..=12).contains(&month) {
        return None;
    }
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Some((
        format!("{year:04}-{month:02}-01T00:00:00.000Z"),
        format!("{ny:04}-{nm:02}-01T00:00:00.000Z"),
    ))
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace 'group_buckets|month_range|malformed_bucket'; echo "EXIT: $?"
```

Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_source.rs
git commit -m "Group Immich buckets into an ascending year/month tree, and a month's UTC range

Not date_tree::group: monthly buckets through it would produce a day node meaning 'the
whole month', a lie in the data. Ascending to match the local Dates section, because two
date trees in one panel reading opposite ways reads as a bug.

A malformed bucket is skipped rather than fatal, and a month range ends at the next
month's first instant so leap years need no special case — but December has to roll the
year, which is where this goes quietly wrong."
```

---

### Task 11: `Job::Buckets`

**Files:**
- Modify: `crates/app/src/immich_worker.rs`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn a_buckets_job_comes_back_as_a_buckets_result() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                Job::Buckets => Outcome::Buckets(Ok(vec![])),
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Buckets);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(matches!(got[0], Outcome::Buckets(Ok(_))));
    }

    /// ⚠ An unconfigured server must report, not panic — the section shows the message.
    #[test]
    fn a_buckets_job_without_a_connection_reports_it() {
        let f = live_fetcher(None);
        match f(Job::Buckets) {
            Outcome::Buckets(Err(msg)) => assert!(!msg.is_empty()),
            _ => panic!("an unconfigured fetcher must report an error"),
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace buckets_job; echo "EXIT: $?"
```

Expected: FAIL — `Job::Buckets` does not exist.

- [ ] **Step 3: Implement**

Add `Buckets` to `Job` and `Buckets(Result<Vec<immich::Bucket>, String>)` to `Outcome`. In `request`, treat `Job::Buckets` like `Albums`/`Album` for the `pending_list` guard; in `poll`, clear `pending_list` for it too. In `live_fetcher`, add the arm `Job::Buckets => Outcome::Buckets(client.buckets().map_err(|e| e.to_string()))`, and the unconfigured arm `Job::Buckets => Outcome::Buckets(Err(e))`.

⚠ `Job` derives `Clone, PartialEq, Eq` — a unit variant needs nothing extra.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich_worker; echo "EXIT: $?"
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_worker.rs
git commit -m "Job::Buckets, so the timeline tree is fetched off the UI thread like everything else"
```

---

### Task 12: `LibrarySource::ImmichBucket` and `open_immich_bucket`

**Files:**
- Modify: `crates/app/src/main.rs`, `crates/app/src/immich_flow.rs`

- [ ] **Step 1: Add the variant**

In `enum LibrarySource`:

```rust
    /// One month of the Immich timeline.
    ///
    /// ⚠ The bucket identifier the server gave, plus the label to show. The label is carried
    /// rather than formatted on demand so the status line and the rail agree.
    ImmichBucket { bucket: String, label: String },
```

- [ ] **Step 2: Review every catch-all**

⚠⚠ **Seven `match` sites on `.source` use catch-alls, so this compiles silently.** Slice 1 recorded the same thing. Review each and **state in your report what it does with the new variant**:

```bash
grep -rn 'match .*\.source\|match source' crates/app/src --include='*.rs'
```

Expected: `catalog_ops.rs:65`, `catalog_ops.rs:361`, `collections_panel.rs:~472`, `smart_sources.rs:69`, `sources.rs:168`, `sources.rs:333`, `sources.rs:341`. For slice 1's `ImmichAlbum` all seven were correct by accident: no rule criterion, no periods, no keyword or collection id, and forgetting a local folder leaves it open. Confirm the same holds here.

- [ ] **Step 3: Add the loader**

In `immich_flow.rs`, beside `open_immich_album`:

```rust
    /// Show one month of the Immich timeline.
    ///
    /// ⚠ Identical in shape to `open_immich_album` — including `catalog::sort`, which that
    /// function was missing at first and which silently ignored the library sort setting.
    pub(crate) fn open_immich_bucket(
        &mut self,
        ctx: &egui::Context,
        bucket: String,
        label: String,
        assets: &[immich::Asset],
    ) -> usize {
        let Some(conn) = immich_source::connection(&self.settings) else {
            self.status = "No Immich server is configured — set one in Settings".to_string();
            return 0;
        };
        let root = immich_source::cache_root(&conn.url);
        let mut scan = immich_source::entries_from_assets(assets, &root);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::ImmichBucket {
                bucket,
                label: label.clone(),
            },
            scan,
        );
        self.status = if n == assets.len() {
            format!("{n} photos in {label}")
        } else {
            format!(
                "{n} of {} photos in {label} — the rest are videos or formats Rustroom cannot open",
                assets.len()
            )
        };
        n
    }
```

Rename `entries_from_album` to `entries_from_assets` throughout (it never cared about albums), and extend `pump_immich` with:

```rust
                Outcome::Buckets(result) => self.immich_buckets = Some(result),
```

Add `immich_buckets: Option<Result<Vec<immich::Bucket>, String>>` to `RustroomApp`, initialised `None` in both `main.rs` and `test_app.rs`.

⚠ `Job::Album` currently carries `{ id, name }`; a month needs `{ bucket, label }` too. Add `Job::Bucket { bucket: String, label: String }` and an `Outcome::Bucket { bucket, label, result }` beside the album pair, and handle it in `pump_immich` by calling `open_immich_bucket`. Its `live_fetcher` arm is `client.search(&Filter::TakenBetween { .. })` built from `month_range`.

- [ ] **Step 4: Build and test**

```bash
cargo build -p app; echo "BUILD EXIT: $?"
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: both 0.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src
git commit -m "A month of the Immich timeline opens through open_scan, like an album

Including catalog::sort, which the album loader was missing at first and which silently
ignored the library sort setting. entries_from_album becomes entries_from_assets — it
never cared about albums."
```

---

### Task 13: The Timeline node in the rail

**Files:**
- Modify: `crates/app/src/immich_panel.rs`

- [ ] **Step 1: Add the node**

Extend `Action`:

```rust
    /// Ask the server for its month buckets.
    RefreshBuckets,
    /// Open one month.
    OpenBucket { bucket: String, label: String },
```

In `body`, after the albums list, a collapsing Timeline:

```rust
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Timeline")
        .id_salt("immich_timeline")
        .show(ui, |ui| match &app.immich_buckets {
            None => {
                if app.immich.is_listing() {
                    ui.label(egui::RichText::new("Loading…").weak().small());
                } else {
                    action = Action::RefreshBuckets;
                }
            }
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).weak().small());
            }
            Some(Ok(buckets)) => {
                let years = rustroom_app::immich_source::group_buckets(buckets);
                if years.is_empty() {
                    ui.label(egui::RichText::new("Nothing on the server").weak().small());
                }
                for y in years {
                    // ⚠ `id_salt` on every header: egui's auto-ids shift with what was drawn
                    // before them, so two years would otherwise collapse together — the hazard
                    // the Dates section already documents.
                    egui::CollapsingHeader::new(format!("{} ({})", y.year, y.count))
                        .id_salt(("immich_year", y.year))
                        .show(ui, |ui| {
                            for m in &y.months {
                                let label = format!("{} {}", month_name(m.month), y.year);
                                let selected = matches!(
                                    app.library.as_ref().map(|l| &l.source),
                                    Some(crate::LibrarySource::ImmichBucket { bucket, .. })
                                        if bucket == &m.bucket
                                );
                                let r = list_row::row(
                                    ui,
                                    RowSpec {
                                        name: &label,
                                        count: Some(m.count as i64),
                                        meta: "",
                                        selected,
                                        indent: 8.0,
                                        hover: "Photos are downloaded when you open them",
                                        actions: &[],
                                    },
                                );
                                if r.clicked {
                                    action = Action::OpenBucket {
                                        bucket: m.bucket.clone(),
                                        label,
                                    };
                                }
                            }
                        });
                }
            }
        });
```

Add a `month_name(u32) -> &'static str` helper in this module returning `"January"`…`"December"` and `"?"` for anything else.

⚠ `action` is assigned inside nested closures. If the borrow checker objects, collect into a local `let mut picked: Option<Action> = None;` and assign `action` after the closures close — the shape every panel in this file uses.

- [ ] **Step 2: Apply the two new actions**

In `collections_panel.rs`, beside the existing Immich arms:

```rust
        crate::immich_panel::Action::RefreshBuckets => {
            app.immich.request(rustroom_app::immich_worker::Job::Buckets);
        }
        crate::immich_panel::Action::OpenBucket { bucket, label } => {
            app.immich
                .request(rustroom_app::immich_worker::Job::Bucket { bucket, label });
        }
```

- [ ] **Step 3: Build and test**

```bash
cargo build -p app; echo "BUILD EXIT: $?"
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: both 0.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src
git commit -m "A Timeline of years and months in the Immich section

One GET /timeline/buckets gives the whole tree with counts; clicking a month searches
its date range. id_salt on every header, because egui's auto-ids shift with what was
drawn before them and two years would otherwise collapse together."
```

---

### Task 14: The gate

- [ ] **Step 1: Formatting**

```bash
cargo fmt --all; cargo fmt --all -- --check; echo "FMT EXIT: $?"
```

- [ ] **Step 2: Clippy, in its own target directory**

```bash
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings; echo "CLIPPY EXIT: $?"
```

- [ ] **Step 3: The suite**

```bash
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: all three **0**, and the **74 golden tests unmoved** — this slice adds no pipeline stage, so a moved golden means something unrelated is wrong.

- [ ] **Step 4: Commit any formatting churn**

```bash
git add -A && git commit -m "fmt"
```

---

### Task 15: Run it

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the path**

1. Settings → Performance: the thumbnail cache shows a size and an MB budget slider.
2. Settings → Immich: its own size, Clear and budget.
3. The Immich section → **Timeline** → years with counts → a month → the grid fills from the server.
4. Reopen the same month: thumbnails appear without a visible fetch.
5. Quit, relaunch, reopen it: **thumbnails still appear at once** — that is Part 2 working.
6. Open a photo, edit it, export it.

**Expected and not bugs:** marks still do not persist on undownloaded photos (slice 3); a budget change applies at the next folder open, not on Apply.

- [ ] **Step 3: Report what use found**

⚠⚠ Almost every GUI-wiring defect in this repo was found by use, not a test — and slice 1 shipped two API bugs behind a green suite. Write what happened into `docs/JOURNAL.md` as a new dated section at the top, and update `CLAUDE.md`'s live state **in place**.

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| Two `u32` MB budgets, 5120 default, no field-level serde default | 2 |
| `ThumbStore` budget, sweep at construction and on writes | 3, 4 |
| LRU by touching mtime on a hit | 4 |
| `.tmp` deleted not counted; zero budget safe; absent dir safe | 3 |
| `ThumbLoader::new` widens; five call sites; tests pass `u64::MAX` | 5 |
| Budget change applies at next folder open | 5 (hover text) |
| `Identity` enum; `Local` bytes byte-identical; no discriminant | 1, 6 |
| `version` is `updatedAt`, not `thumbhash` | 6, 7 |
| Separate directory, no `VERSION` bump | 6 |
| Store consulted before fetching, written on success | 7 |
| Two Settings rows with separate readouts and Clear | 5, 8 |
| `Filter`, `search`, `buckets`; `album` a wrapper; paging unchanged | 9 |
| Captured `buckets_real.json` | 9 |
| `group_buckets` ascending, gap-safe, malformed-safe; not `date_tree` | 10 |
| `month_range` rolls December, exclusive end, UTC | 10 |
| `Job::Buckets` off the UI thread | 11 |
| `LibrarySource::ImmichBucket`; seven catch-alls reviewed | 12 |
| `open_immich_bucket` with `catalog::sort` | 12 |
| `entries_from_album` → `entries_from_assets` | 12 |
| Timeline node with counts | 13 |
| No golden movement | 14 |
| Seen in use | 15 |

**Known soft spots, flagged rather than hidden:**

- **The pinned filename constant in Task 1** was computed by reimplementing `fnv1a` and `key_bytes` in Python, verified against the three published vectors the Rust test already pins. It is a reimplementation, not the code — Task 1 Step 2 says what to do if it disagrees.
- **Threading `updatedAt` to the thumbnail loader** (Task 7 Step 3) needs the existing `shared()` `OnceLock` idiom read before writing. The mechanism is named; the exact shape is left to whoever reads that function, because inventing a second one would be worse.
- **`action` assignment inside nested closures** (Task 13) may need a local, as the note says. The existing panels show the pattern.

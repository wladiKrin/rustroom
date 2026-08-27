# Windowed Browsing Implementation Plan (M4.3a-i)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound the Library's thumbnail memory and load only what is on screen, so a folder becomes usable immediately and a 10,000-photo source costs 256 MB instead of ~7 GB.

**Architecture:** Three new/rewritten units in the `rustroom_app` **lib** (pure arithmetic, a generic bounded cache, a demand-driven loader), then the two **bin** views are rewired to them and virtualised. The cache is keyed by `PathBuf`, so indices never leave the UI and copies of one file share one texture.

**Tech Stack:** Rust, `egui`/`eframe` 0.35, `std::sync` (Mutex + Condvar), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-27-windowed-browsing-design.md`

---

## Ground rules for this repo

Read these before Task 1. They are not optional; each has cost this project a bug already.

- **Branch:** work happens on `m4.3a-i-windowed-browsing`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command in this plan needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**, so bin tests are `cargo test -p app --bin Rustroom`.
- **`crates/app/src/lib.rs` must not depend on egui.** That is why `ThumbCache` is generic over its
  value type: the bin instantiates `ThumbCache<egui::TextureHandle>`, the lib tests use `String`.
- **⚠ In a binary crate, an item lands with its first caller.** A `pub fn` in `main.rs` or a bin
  module with no caller is dead code and `clippy -D warnings` rejects it. This has bitten this
  project eight times. It is why Tasks 5 and 6 do not split "add the helper" from "call it".
- **⚠ Check exit codes, never grep output.** `cargo test --all | grep -c "test result: FAILED"`
  returns 0 when the *build* fails, because a failed build emits no test-result lines. The same
  trap applies to piping clippy through grep — the pipeline reports grep's status.
- **⚠ `rustup update stable` before trusting a green local clippy.** `rust-toolchain.toml` says
  `channel = "stable"`, unpinned, so CI can have a newer clippy than the local toolchain. CI also
  runs `--all-targets --all-features`.

Verification command used throughout:

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/app/src/thumb_window.rs` | Pure arithmetic: which items intersect a viewport; where to scroll to centre one. No egui. | **create** |
| `crates/app/src/thumb_cache.rs` | `ThumbCache<T>`: path-keyed LRU under a byte budget, with the on-screen window pinned. Generic, so no egui. | **create** |
| `crates/app/src/thumb_loader.rs` | Demand-driven workers: a replaceable want-list of paths, an in-flight set, results keyed by path. | rewrite |
| `crates/app/src/lib.rs` | Declare the two new modules. | modify |
| `crates/app/src/main.rs` | `Library` holds a `ThumbCache` instead of `thumbs`; the three loaders build an idle `ThumbLoader`; `Library::sync_thumb_window`. | modify |
| `crates/app/src/shell.rs` | Drain results into the cache by path. | modify |
| `crates/app/src/library.rs` | Virtualised grid via `show_rows`; owns the window while the Library view is drawing. | modify |
| `crates/app/src/filmstrip.rs` | Virtualised strip via `show_viewport`; `centre_offset` replaces `scroll_to_me`. | modify |
| `crates/app/src/test_app.rs` | A `Library` fixture for headless view tests. | modify |
| `crates/app/examples/bench_thumbs.rs` | Updated to the new loader API. | modify |

---

## Task 1: `thumb_window` — the arithmetic

**Files:**
- Create: `crates/app/src/thumb_window.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/thumb_window.rs` containing *only* the test module for now, plus the two
function signatures with `todo!()` bodies so the file compiles:

```rust
//! Which items of a uniform strip or grid are on screen, and where to scroll to centre one.
//!
//! ⚠ A grid of rows and a strip of cells are the same one-dimensional problem, so one pair of
//! functions serves both views. The grid's items are **rows** (`step = cell + spacing.y`); the
//! filmstrip's items are **cells** (`step = cell + spacing.x`).
//!
//! ⚠ Deliberately egui-free — plain `f32` and `usize` — following `view_region`, whose own doc
//! says it is kept that way "so it can be tested without a window". Clamping and off-by-one
//! errors are then testable rather than only visible as a wrong image.

use std::ops::Range;

pub fn visible_span(
    offset: f32,
    extent: f32,
    step: f32,
    count: usize,
    margin: usize,
) -> Range<usize> {
    todo!()
}

pub fn centre_offset(index: usize, step: f32, extent: f32, content: f32) -> f32 {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Items are 40 long; the viewport shows 100 of them.
    const STEP: f32 = 40.0;
    const EXTENT: f32 = 100.0;

    #[test]
    fn the_top_of_the_list_starts_at_zero() {
        // Items 0, 1 and 2 intersect [0, 100).
        assert_eq!(visible_span(0.0, EXTENT, STEP, 100, 0), 0..3);
    }

    #[test]
    fn a_scrolled_viewport_starts_at_the_first_intersecting_item() {
        // ⚠ The *first intersecting* item, not the first fully visible one. At offset 100 item 2
        // spans 80..120, so it is half on screen — drawing from item 3 would leave a gap at the
        // top that looks like a rendering bug.
        assert_eq!(visible_span(100.0, EXTENT, STEP, 100, 0), 2..6);
    }

    #[test]
    fn the_margin_widens_both_ends() {
        assert_eq!(visible_span(400.0, EXTENT, STEP, 100, 2), 8..15);
    }

    #[test]
    fn the_margin_is_clamped_at_both_ends() {
        // ⚠ Both ends, in one test, because a `saturating_sub` at the start and a `min` at the end
        // are different mistakes and each is easy to make alone.
        assert_eq!(visible_span(0.0, EXTENT, STEP, 100, 5).start, 0);
        assert_eq!(visible_span(3900.0, EXTENT, STEP, 100, 5).end, 100);
    }

    #[test]
    fn an_offset_past_the_content_yields_an_empty_range() {
        // A stale scroll offset after the list shrinks must not produce start > end, which would
        // panic anything treating the range as a length.
        let span = visible_span(9000.0, EXTENT, STEP, 100, 0);
        assert!(span.start <= span.end, "{span:?}");
        assert!(span.is_empty(), "{span:?}");
    }

    #[test]
    fn an_empty_list_yields_an_empty_range() {
        assert_eq!(visible_span(0.0, EXTENT, STEP, 0, 4), 0..0);
    }

    #[test]
    fn a_degenerate_step_yields_an_empty_range() {
        // ⚠ A zero step means a zero-height cell — division would give infinity, and `as usize`
        // saturates rather than panicking, so the failure would be a silently enormous range.
        assert_eq!(visible_span(0.0, EXTENT, 0.0, 100, 0), 0..0);
    }

    #[test]
    fn a_middle_item_is_centred() {
        // Item 10 spans 400..440, centre 420; the viewport is 100 long, so it starts at 370.
        assert_eq!(centre_offset(10, STEP, EXTENT, 4000.0), 370.0);
    }

    #[test]
    fn centring_the_first_item_does_not_scroll_past_the_start() {
        assert_eq!(centre_offset(0, STEP, EXTENT, 4000.0), 0.0);
    }

    #[test]
    fn centring_the_last_item_does_not_scroll_past_the_end() {
        // ⚠ This is the half `scroll_to_me` used to get right for free. Without the upper clamp
        // the strip scrolls into blank space past the final thumbnail.
        assert_eq!(centre_offset(99, STEP, EXTENT, 4000.0), 3900.0);
    }

    #[test]
    fn content_shorter_than_the_viewport_never_scrolls() {
        assert_eq!(centre_offset(1, STEP, EXTENT, 80.0), 0.0);
    }
}
```

Add to `crates/app/src/lib.rs`, in alphabetical position (after `pub mod thumb_loader;`):

```rust
pub mod thumb_window;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib thumb_window
```

Expected: every test fails, panicking at `not yet implemented`.

- [ ] **Step 3: Write the implementation**

Replace the two `todo!()` bodies in `crates/app/src/thumb_window.rs`:

```rust
/// Indices of items whose cells intersect the viewport, widened by `margin` items each way.
///
/// `offset` is the scroll position, `extent` the viewport's length along the same axis, `step`
/// one item's pitch (its size plus the gap after it), and `count` how many items exist.
///
/// ⚠ The margin is prefetch, not display: callers draw `visible_span(.., 0)` and *load*
/// `visible_span(.., MARGIN)`.
pub fn visible_span(
    offset: f32,
    extent: f32,
    step: f32,
    count: usize,
    margin: usize,
) -> Range<usize> {
    if count == 0 || !step.is_finite() || step <= 0.0 || !offset.is_finite() || !extent.is_finite()
    {
        return 0..0;
    }
    let first = (offset / step).floor().max(0.0) as usize;
    // Inclusive: the last item whose cell starts before the viewport ends.
    let last = ((offset + extent) / step).floor().max(0.0) as usize;
    let end = last.saturating_add(margin).saturating_add(1).min(count);
    // ⚠ Clamped against `end`, not only against `count`: a stale offset past the content would
    // otherwise give start > end, which is not a range any caller can use.
    let start = first.saturating_sub(margin).min(end);
    start..end
}

/// The scroll offset that centres `index`, clamped so the ends do not scroll past the content.
///
/// ⚠ This replaces `Response::scroll_to_me`, which cannot work on a virtualised list because an
/// off-window cell is never drawn. It is also the correct fix for the drift bug recorded from
/// M2.2: egui computes a `scroll_to_me` target offset **once** and never recomputes it, so the
/// centred photo slid left as thumbnails ahead of it loaded and reflowed the content.
pub fn centre_offset(index: usize, step: f32, extent: f32, content: f32) -> f32 {
    let centre = index as f32 * step + step * 0.5;
    let max = (content - extent).max(0.0);
    (centre - extent * 0.5).clamp(0.0, max)
}
```

⚠ `!step.is_finite()` comes **first** and is not redundant: `step <= 0.0` is `false` for `NaN`, so
a NaN cell size would sail past a `<=` test alone and produce a saturated, enormous range.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib thumb_window
```

Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: clean, and the pre-existing test count rises by exactly 11.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/thumb_window.rs crates/app/src/lib.rs
git commit -m "feat(app): viewport span and centring arithmetic for windowed browsing"
```

---

## Task 2: `ThumbCache<T>` — bounded, path-keyed residency

**Files:**
- Create: `crates/app/src/thumb_cache.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/thumb_cache.rs` with the type, `todo!()` bodies, and the tests:

```rust
//! A path-keyed, byte-budgeted cache of loaded thumbnails.
//!
//! ⚠⚠ **Keyed by `PathBuf`, not by an `entries` index**, and three things fall out of that:
//! copies of one file share one texture (a Library thumbnail is the camera's embedded JPEG, so
//! every copy's is pixel-identical); a re-sort or `rebuild_entries` invalidates nothing; and
//! indices never leave the UI, which removes this project's third-most-repeated bug family —
//! sparse-versus-dense index confusion — at the boundary rather than guarding against it.
//!
//! ⚠ Generic over the value so it can live in this lib, which must not depend on egui. The bin
//! instantiates `ThumbCache<egui::TextureHandle>`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How much memory thumbnails may occupy.
///
/// ~365 thumbnails at the measured ~0.7 MB each — three to four screens at the default cell
/// size, so ordinary up-and-down browsing never evicts, while a 10,000-photo source is bounded
/// here instead of at ~7 GB.
pub const THUMB_BUDGET_BYTES: usize = 256 * 1024 * 1024;

// ⚠ A **module-level** compile-time guard, not an associated one. An associated `const` inside an
// `impl` is only evaluated where it is used, so `PrefixCache`'s first attempt at exactly this
// compiled happily with a budget of zero — a guard that did not guard.
const _: () = assert!(THUMB_BUDGET_BYTES > 0);

struct Entry<T> {
    value: T,
    bytes: usize,
    /// Monotonic recency stamp. Set when the entry is inserted and refreshed by `retain_window`.
    seq: u64,
}

pub struct ThumbCache<T> {
    map: HashMap<PathBuf, Entry<T>>,
    budget: usize,
    bytes: usize,
    clock: u64,
}

impl<T> ThumbCache<T> {
    pub fn new(budget: usize) -> Self {
        todo!()
    }

    pub fn with_default_budget() -> Self {
        Self::new(THUMB_BUDGET_BYTES)
    }

    pub fn get(&self, path: &Path) -> Option<&T> {
        todo!()
    }

    pub fn contains(&self, path: &Path) -> bool {
        todo!()
    }

    pub fn insert(&mut self, path: PathBuf, value: T, bytes: usize) {
        todo!()
    }

    pub fn retain_window(&mut self, window: &[PathBuf]) {
        todo!()
    }

    pub fn clear(&mut self) {
        todo!()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/x/{name}.RAF"))
    }

    /// Five 10-byte entries, `a`..`e`, inserted in order.
    fn cache_of_five(budget: usize) -> ThumbCache<String> {
        let mut c = ThumbCache::new(budget);
        for name in ["a", "b", "c", "d", "e"] {
            c.insert(p(name), name.to_string(), 10);
        }
        c
    }

    #[test]
    fn a_value_survives_a_round_trip() {
        let c = cache_of_five(1000);
        assert_eq!(c.get(&p("c")).map(String::as_str), Some("c"));
        assert!(c.contains(&p("a")));
        assert!(!c.contains(&p("zz")));
        assert_eq!(c.len(), 5);
        assert_eq!(c.bytes(), 50);
    }

    #[test]
    fn two_copies_of_one_file_are_one_entry() {
        // ⚠ The whole reason for path-keying. `entries` repeats a path once per copy, so an
        // index-keyed cache holds three pixel-identical textures for a file with three copies.
        let mut c = ThumbCache::new(1000);
        c.insert(p("a"), "first".into(), 10);
        c.insert(p("a"), "second".into(), 10);
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 10, "a replacement must not double-count its bytes");
        assert_eq!(c.get(&p("a")).map(String::as_str), Some("second"));
    }

    #[test]
    fn nothing_is_evicted_while_under_budget() {
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("a")]);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn a_path_outside_the_window_is_evicted_when_over_budget() {
        // Budget 30 holds three of the five.
        let mut c = cache_of_five(30);
        c.retain_window(&[p("e")]);
        assert!(c.bytes() <= 30, "over budget: {}", c.bytes());
        assert!(c.contains(&p("e")), "the window must survive");
    }

    #[test]
    fn eviction_takes_the_least_recently_windowed_first() {
        // ⚠ Recency is set by `retain_window`, not by `get`: what makes a thumbnail recent is
        // having been on screen. Here `a` is re-windowed, so `b` becomes the oldest and goes.
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("a")]);
        c.budget = 40;
        c.retain_window(&[p("e")]);
        assert!(c.contains(&p("a")), "a was re-windowed and must outlive b");
        assert!(!c.contains(&p("b")), "b was the oldest and should have gone");
        assert!(c.contains(&p("e")));
    }

    #[test]
    fn the_window_is_pinned_even_when_it_exceeds_the_budget() {
        // ⚠⚠ The load-bearing property. Eviction may never touch what is on screen, however far
        // the budget falls — otherwise a large window silently blanks the very cells being drawn,
        // and the failure looks like a slow loader rather than a bug.
        let mut c = cache_of_five(10);
        let window: Vec<PathBuf> = ["a", "b", "c", "d", "e"].iter().map(|n| p(n)).collect();
        c.retain_window(&window);
        assert_eq!(c.len(), 5, "an on-screen thumbnail was evicted");
        assert!(c.bytes() > c.budget, "the test is not exercising the pin");
    }

    #[test]
    fn a_window_naming_absent_paths_is_harmless() {
        // The window is whatever is on screen; most of it may not be loaded yet.
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("zz"), p("a")]);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn clear_empties_it() {
        let mut c = cache_of_five(1000);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }
}
```

Add to `crates/app/src/lib.rs`, before `pub mod thumb_loader;`:

```rust
pub mod thumb_cache;
```

Note the tests reach `c.budget` directly — legal because the test module is a child of the
defining module.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib thumb_cache
```

Expected: failures at `not yet implemented`.

- [ ] **Step 3: Write the implementation**

Replace the `todo!()` bodies:

```rust
    pub fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            budget,
            bytes: 0,
            clock: 0,
        }
    }

    /// ⚠ Takes `&self`, deliberately. Recency is set by `retain_window` instead, for two reasons:
    /// what makes a thumbnail recent is *being on screen*, which is exactly what `retain_window`
    /// is told; and a `&mut self` accessor called from inside the draw loop would fight the
    /// shared borrows of `entries` and `visible` that the loop already holds.
    pub fn get(&self, path: &Path) -> Option<&T> {
        self.map.get(path).map(|e| &e.value)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(path)
    }

    /// Store `value` for `path`, replacing whatever was there.
    pub fn insert(&mut self, path: PathBuf, value: T, bytes: usize) {
        self.clock += 1;
        let seq = self.clock;
        if let Some(old) = self.map.insert(
            path,
            Entry {
                value,
                bytes,
                seq,
            },
        ) {
            // ⚠ Subtract the replaced entry's bytes, or a re-inserted path double-counts and the
            // budget drifts upward until eviction starts throwing away live thumbnails.
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
    }

    /// Promote everything on screen to most-recent, then evict down to the budget.
    ///
    /// ⚠⚠ **The window is pinned, not merely preferred.** Nothing in `window` is ever evicted,
    /// however far the budget falls, so the cache may legitimately exceed its budget while a very
    /// large window is on screen.
    pub fn retain_window(&mut self, window: &[PathBuf]) {
        let pinned: HashSet<&Path> = window.iter().map(PathBuf::as_path).collect();
        for path in window {
            self.clock += 1;
            let seq = self.clock;
            if let Some(e) = self.map.get_mut(path) {
                e.seq = seq;
            }
        }
        while self.bytes > self.budget {
            let victim = self
                .map
                .iter()
                .filter(|(path, _)| !pinned.contains(path.as_path()))
                .min_by_key(|(_, e)| e.seq)
                .map(|(path, _)| path.clone());
            // `None` means everything left is pinned — stop rather than blank the screen.
            let Some(victim) = victim else { break };
            if let Some(e) = self.map.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib thumb_cache
```

Expected: `test result: ok. 8 passed`.

- [ ] **Step 5: Sabotage the pin and watch it fail**

Temporarily delete the `.filter(...)` line from `retain_window` so eviction may take a pinned
entry. Then:

```bash
cargo test -p app --lib thumb_cache 2>&1 | tail -20; echo "exit=$?"
```

Expected: `the_window_is_pinned_even_when_it_exceeds_the_budget` fails with
`an on-screen thumbnail was evicted`. **Confirm the file compiled first** — a sabotage that fails
to compile looks exactly like one that passes. Restore the line and re-run to confirm green.

- [ ] **Step 6: Full verification and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add crates/app/src/thumb_cache.rs crates/app/src/lib.rs
git commit -m "feat(app): path-keyed thumbnail cache bounded by a byte budget"
```

---

## Task 3: `ThumbLoader` — demand-driven and path-keyed

**Files:**
- Modify: `crates/app/src/thumb_loader.rs` (rewrite the struct, keep `worker_count`, `load_thumbnail`, `convert_thumb`)
- Modify: `crates/app/examples/bench_thumbs.rs`

- [ ] **Step 1: Write the failing tests**

Replace the `ThumbResult` struct, the `ThumbLoader` struct and its `impl` in
`crates/app/src/thumb_loader.rs` with the declarations below, leaving `todo!()` bodies. Keep
`worker_count`, `load_thumbnail` and `convert_thumb` exactly as they are.

```rust
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

/// One finished thumbnail (or `None` if that file had no usable preview).
///
/// ⚠ Keyed by **path**, not by an index. Indices shift under a re-sort or a copy being created;
/// a path does not, and the cache this feeds is path-keyed for the same reason.
pub struct ThumbResult {
    pub path: PathBuf,
    pub thumb: Option<Thumbnail>,
}

#[derive(Default)]
struct Queue {
    wanted: VecDeque<PathBuf>,
    in_flight: HashSet<PathBuf>,
    stop: bool,
}

/// Worker threads that load whatever the UI currently wants.
pub struct ThumbLoader {
    rx: Receiver<ThumbResult>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    _handles: Vec<JoinHandle<()>>,
}

impl ThumbLoader {
    pub fn new(
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_loader(load_thumbnail, worker_count(), display_space, on_progress)
    }

    pub fn with_loader(
        load: fn(&Path) -> Option<Thumbnail>,
        threads: usize,
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        todo!()
    }

    pub fn want(&self, paths: &[PathBuf]) {
        todo!()
    }

    pub fn drain(&self) -> Vec<ThumbResult> {
        self.rx.try_iter().collect()
    }
}
```

Now replace the whole `#[cfg(test)] mod tests` block with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Collect results until `want` of them have arrived or the deadline passes.
    fn collect(loader: &ThumbLoader, want: usize) -> Vec<ThumbResult> {
        let mut results = Vec::new();
        for _ in 0..400 {
            results.extend(loader.drain());
            if results.len() >= want {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        results.sort_by(|a, b| a.path.cmp(&b.path));
        results
    }

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/nonexistent/rustroom_{i:04}.raf")))
            .collect()
    }

    #[test]
    fn streams_results_keyed_by_path_even_on_failure() {
        // Non-existent files fail extraction -> thumb is None, but a result still streams.
        let loader = ThumbLoader::new(color::OutputSpace::Srgb, || {});
        loader.want(&paths(2));
        let results = collect(&loader, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, paths(2)[0]);
        assert_eq!(results[1].path, paths(2)[1]);
        assert!(results[0].thumb.is_none() && results[1].thumb.is_none());
    }

    #[test]
    fn every_wanted_path_is_loaded_exactly_once() {
        let loader = ThumbLoader::with_loader(|_| None, 4, color::OutputSpace::Srgb, || {});
        loader.want(&paths(50));
        let results = collect(&loader, 50);
        assert_eq!(results.len(), 50, "wrong number of results");
        let got: Vec<PathBuf> = results.iter().map(|r| r.path.clone()).collect();
        assert_eq!(got, paths(50));
    }

    #[test]
    fn a_worker_wakes_after_an_empty_queue() {
        // ⚠ The structural change from the old fixed-list loader: workers must park on an empty
        // queue and wake when the window moves, rather than returning when the list runs out.
        // Without the condvar notify they sleep forever and the grid stays grey.
        let loader = ThumbLoader::with_loader(|_| None, 2, color::OutputSpace::Srgb, || {});
        thread::sleep(std::time::Duration::from_millis(30));
        assert!(loader.drain().is_empty(), "nothing was wanted yet");
        loader.want(&paths(3));
        assert_eq!(collect(&loader, 3).len(), 3);
    }

    #[test]
    fn a_replaced_want_list_abandons_what_is_no_longer_wanted() {
        // ⚠ Replacing the list *is* the cancellation — "abandon, don't abort", the trade
        // `DecodeWorker` and `batch_export` both already make. A scrolled-past request is simply
        // never popped.
        //
        // Two workers, so at most two of the abandoned batch can already be in hand.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        fn slow(_p: &Path) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(20));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader = ThumbLoader::with_loader(slow, 2, color::OutputSpace::Srgb, || {});
        loader.want(&paths(200));
        thread::sleep(std::time::Duration::from_millis(10));
        let keep: Vec<PathBuf> = vec![PathBuf::from("/nonexistent/kept_a.raf")];
        loader.want(&keep);
        let results = collect(&loader, 1);
        assert!(
            results.iter().any(|r| r.path == keep[0]),
            "the newly wanted path never loaded"
        );
        // The abandoned batch must not grind on: only what was already in hand may finish.
        thread::sleep(std::time::Duration::from_millis(80));
        let started = STARTED.load(Ordering::SeqCst);
        assert!(
            started <= 5,
            "abandoned work kept going: {started} loads started"
        );
    }

    #[test]
    fn an_in_flight_path_is_not_requested_twice() {
        // ⚠ The want-list is rewritten every time the window moves, and consecutive lists overlap
        // almost entirely. Without an in-flight set, one slow thumbnail is fetched once per
        // window move.
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        static RELEASE: AtomicUsize = AtomicUsize::new(0);
        fn gated(_p: &Path) -> Option<Thumbnail> {
            STARTS.fetch_add(1, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while RELEASE.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            None
        }
        STARTS.store(0, Ordering::SeqCst);
        RELEASE.store(0, Ordering::SeqCst);
        let one = paths(1);
        let loader = ThumbLoader::with_loader(gated, 4, color::OutputSpace::Srgb, || {});
        for _ in 0..20 {
            loader.want(&one);
            thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            STARTS.load(Ordering::SeqCst),
            1,
            "the same path was fetched more than once while in flight"
        );
        RELEASE.store(1, Ordering::SeqCst);
        assert_eq!(collect(&loader, 1).len(), 1);
    }

    /// Workers in this test, and paths — one each, so every worker must participate.
    const PAR: usize = 4;

    #[test]
    fn the_folder_is_loaded_in_parallel() {
        // ⚠ Each load waits for its peers rather than sleeping a fixed time, so the peak is
        // determined by the loader's structure and not by how the OS happened to schedule four
        // threads. `batch_export`'s cancel test failed on Linux CI for exactly that reason.
        //
        // ⚠ `ARRIVED` only ever counts up, and that is what makes this a barrier. Releasing on
        // `LIVE` does not work: the last worker to arrive satisfies the condition, leaves, and
        // decrements it below the threshold again — the test still passes, just two seconds
        // slower, which is exactly how a broken barrier hides.
        static ARRIVED: AtomicUsize = AtomicUsize::new(0);
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        fn gated(_p: &Path) -> Option<Thumbnail> {
            ARRIVED.fetch_add(1, Ordering::SeqCst);
            let live = LIVE.fetch_add(1, Ordering::SeqCst) + 1;
            PEAK.fetch_max(live, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while ARRIVED.load(Ordering::SeqCst) < PAR && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            LIVE.fetch_sub(1, Ordering::SeqCst);
            None
        }
        let loader = ThumbLoader::with_loader(gated, PAR, color::OutputSpace::Srgb, || {});
        loader.want(&paths(PAR));
        assert_eq!(collect(&loader, PAR).len(), PAR);
        assert_eq!(
            PEAK.load(Ordering::SeqCst),
            PAR,
            "loads never overlapped — the folder is still being read one file at a time"
        );
    }

    #[test]
    fn a_dropped_loader_stops_its_workers() {
        // Switching folders must not leave the old one grinding through hundreds of files.
        //
        // ⚠ The old loader stopped because `tx.send` failed once `rx` was dropped. A parked
        // worker never reaches `send`, so `Drop` has to set the stop flag and wake them.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Option<Thumbnail> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(10));
            None
        }
        STARTED.store(0, Ordering::SeqCst);
        let loader = ThumbLoader::with_loader(counted, 2, color::OutputSpace::Srgb, || {});
        loader.want(&paths(500));
        thread::sleep(std::time::Duration::from_millis(30));
        drop(loader);
        let at_drop = STARTED.load(Ordering::SeqCst);
        thread::sleep(std::time::Duration::from_millis(120));
        let after = STARTED.load(Ordering::SeqCst);
        // Each worker may finish the file already in its hands, so allow one more apiece.
        assert!(
            after <= at_drop + 2,
            "workers kept going after the loader was dropped: {at_drop} -> {after}"
        );
    }

    #[test]
    fn more_threads_than_paths_is_harmless() {
        let loader = ThumbLoader::with_loader(|_| None, 32, color::OutputSpace::Srgb, || {});
        loader.want(&paths(2));
        assert_eq!(collect(&loader, 2).len(), 2);
        // And an empty want-list must not spawn work or panic.
        loader.want(&[]);
        thread::sleep(std::time::Duration::from_millis(20));
        assert!(loader.drain().is_empty());
    }

    #[test]
    fn the_worker_count_leaves_headroom_for_rendering() {
        let n = worker_count();
        assert!(n >= 1, "must always make progress");
        assert!(n <= 6, "must not saturate the machine: {n}");
        if let Ok(cores) = thread::available_parallelism() {
            assert!(
                n < cores.get().max(2),
                "left no core for the render pipeline"
            );
        }
    }

    #[test]
    fn a_thumbnail_is_converted_into_the_display_space() {
        // ⚠ The direction matters, not merely that it changed: an inverted transform would also
        // change the pixels, and would oversaturate exactly as the uncorrected preview does.
        let mut t = Thumbnail {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        convert_thumb(&mut t, color::OutputSpace::DisplayP3);
        assert!(
            t.rgba[0] < 255,
            "sRGB red must be driven below full in a wider space"
        );
        assert_eq!(t.rgba[3], 255, "alpha must be left alone");
    }
}
```

**⚠ Check the last test against the existing file before writing it** — copy the assertions from
the current `a_thumbnail_is_converted_into_the_display_space` verbatim rather than the sketch
above, so a passing test proves the conversion behaviour did not move.

⚠ **Move the atomics import.** The file header currently has
`use std::sync::atomic::{AtomicUsize, Ordering};`, used by the old shared cursor. The new workers
use neither, so leaving it there is an unused import and `clippy -D warnings` rejects it. Delete it
from the header and put `use std::sync::atomic::{AtomicUsize, Ordering};` in the test module
instead — both are still needed there.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib thumb_loader
```

Expected: failures at `not yet implemented`.

- [ ] **Step 3: Write the implementation**

Replace `with_loader`'s and `want`'s `todo!()` bodies, and add a `Drop` impl:

```rust
    /// The injectable form: `load` and the thread count are parameters so the whole loop is
    /// unit-testable with no image fixtures, the way `DecodeWorker` takes its decode fn.
    ///
    /// ⚠ Work is handed out through a **shared queue the UI replaces wholesale**, not a cursor
    /// over a fixed list. A folder mixes RAW and JPEG, whose extraction costs differ by a lot, so
    /// static chunking would leave threads idle while one ground through the expensive half.
    pub fn with_loader(
        load: fn(&Path) -> Option<Thumbnail>,
        threads: usize,
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel::<ThumbResult>();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let on_progress = Arc::new(on_progress);

        let n = threads.max(1);
        let handles = (0..n)
            .map(|_| {
                let queue = queue.clone();
                let tx = tx.clone();
                let on_progress = on_progress.clone();
                thread::spawn(move || {
                    let (lock, cv) = &*queue;
                    loop {
                        // Park until there is something to do, or the loader is dropped.
                        let path = loop {
                            let mut q = lock.lock().unwrap();
                            if q.stop {
                                return;
                            }
                            if let Some(p) = q.wanted.pop_front() {
                                q.in_flight.insert(p.clone());
                                break p;
                            }
                            drop(cv.wait(q).unwrap());
                        };
                        let mut thumb = load(&path);
                        // ⚠ Converted here rather than at upload, so a thumbnail is stored the
                        // way it will be shown. Not optional polish: the filmstrip sits directly
                        // beside the canvas showing the same photo, so correcting only the canvas
                        // would leave a visibly more saturated strip next to a corrected image.
                        if let Some(t) = thumb.as_mut() {
                            convert_thumb(t, display_space);
                        }
                        lock.lock().unwrap().in_flight.remove(&path);
                        // An error means the UI dropped the loader — stop rather than finish a
                        // folder nobody is looking at any more.
                        if tx.send(ThumbResult { path, thumb }).is_err() {
                            return;
                        }
                        on_progress();
                    }
                })
            })
            .collect();
        drop(tx);

        Self {
            rx,
            queue,
            _handles: handles,
        }
    }

    /// Ask for exactly these paths, in this order, discarding whatever was wanted before.
    ///
    /// ⚠⚠ **Replacing the list is the cancellation, and no cancellation mechanism is needed.** A
    /// scrolled-past request is simply never popped — "abandon, don't abort", the same trade
    /// `DecodeWorker` and `batch_export` make, because the work has no safe interruption point.
    ///
    /// ⚠ Paths already in flight are skipped. Consecutive want-lists overlap almost entirely as
    /// the window moves, so without this one slow thumbnail is fetched once per window move.
    ///
    /// ⚠ The caller filters out what it already holds — the UI owns the cache, so the UI is the
    /// only thing that knows what is loaded. A loader keeping its own shadow copy of that would
    /// be two facts that can disagree, the failure `render_max_dim` and `Session::geometry_mode`
    /// were each fixed by deriving rather than storing alongside.
    pub fn want(&self, paths: &[PathBuf]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.wanted.clear();
        for p in paths {
            if !q.in_flight.contains(p) {
                q.wanted.push_back(p.clone());
            }
        }
        drop(q);
        cv.notify_all();
    }
```

And after the `impl ThumbLoader` block:

```rust
impl Drop for ThumbLoader {
    /// ⚠ Required now that workers park. The old fixed-list loader stopped because `tx.send`
    /// failed once `rx` was dropped; a worker asleep on the condvar never reaches `send`, so it
    /// has to be told and woken.
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().stop = true;
        cv.notify_all();
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib thumb_loader
```

Expected: `test result: ok. 10 passed`.

- [ ] **Step 5: Update `bench_thumbs` to the new API**

In `crates/app/examples/bench_thumbs.rs`, replace the loader construction inside the `for threads`
loop. The old call passed `paths.clone()` first; now the paths arrive via `want`:

```rust
        let loader = ThumbLoader::with_loader(
            |p| {
                if catalog::is_raw(p) {
                    raw_decode::extract_thumbnail(p).ok()
                } else {
                    image_decode::thumbnail(p).ok()
                }
            },
            threads,
            color::OutputSpace::Srgb,
            || {},
        );
        loader.want(&paths);
```

Read the surrounding lines and keep the rest of the example (the timing loop, the `println!`)
exactly as it is.

- [ ] **Step 6: Sabotage the in-flight set and watch it fail**

Temporarily change `want` so it does not skip in-flight paths (drop the `if !q.in_flight...`
guard, pushing every path). Then:

```bash
cargo test -p app --lib thumb_loader 2>&1 | tail -20
```

Expected: `an_in_flight_path_is_not_requested_twice` fails with a count above 1. **Confirm it
compiled** before believing the result. Restore and re-run green.

- [ ] **Step 7: Full verification and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

⚠ Expect this to **fail to compile the bin** — `main.rs`, `shell.rs`, `library.rs` and
`filmstrip.rs` still use the old API. That is expected at this point; Task 4 fixes it, and Tasks 3
and 4 therefore land as **one commit**. Do not commit yet — go straight to Task 4 and commit at the
end of it.

---

## Task 4: Swap `Library.thumbs` for the cache — behaviour unchanged

The window is "every visible entry" in this task, so memory and load time do not change yet. That
is deliberate: it separates the risky *representation* change from the *windowing* change, so a
regression in either has one candidate cause.

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/shell.rs:101-114`
- Modify: `crates/app/src/library.rs`
- Modify: `crates/app/src/filmstrip.rs`

- [ ] **Step 1: Replace the field on `Library`**

In `crates/app/src/main.rs`, in `struct Library`, delete:

```rust
    thumbs: Vec<Option<egui::TextureHandle>>, // parallel to entries; None = still loading
```

and add, in its place:

```rust
    /// Loaded thumbnails, keyed by path and bounded by a byte budget.
    ///
    /// ⚠ Keyed by **path**, so copies of one file share a texture and a re-sort or a rebuild
    /// invalidates nothing. `entries` indices never reach it.
    cache: rustroom_app::thumb_cache::ThumbCache<egui::TextureHandle>,
    /// The last want-list handed to the loader.
    ///
    /// ⚠ Kept so `sync_thumb_window` can call `want` **only when the list changes**. Its body
    /// runs every frame; without the comparison it would rewrite the queue sixty times a second.
    /// The same guard `apply_view_demosaic` needs, for the same reason.
    last_want: Vec<PathBuf>,
```

- [ ] **Step 2: Add `Library::sync_thumb_window`**

In `crates/app/src/main.rs`, inside `impl Library` (below `refilter`):

```rust
    /// Tell the cache and the loader what is on screen.
    ///
    /// `window` is every path the view wants resident — the drawn cells plus the prefetch margin.
    /// Pins them in the cache, evicts down to the budget, then asks the loader for whatever is
    /// still missing.
    fn sync_thumb_window(&mut self, window: &[PathBuf]) {
        self.cache.retain_window(window);
        let missing: Vec<PathBuf> = window
            .iter()
            .filter(|p| !self.cache.contains(p))
            .cloned()
            .collect();
        // ⚠ Only when it changes. See `last_want`.
        if missing != self.last_want {
            self.loader.want(&missing);
            self.last_want = missing;
        }
    }
```

⚠ `missing` de-duplicates nothing on purpose: `window` is built from paths that are already
unique per file in Task 5 and 6 (a copy row and its original share a path, and the builder
de-duplicates there). If a duplicate did slip through, the in-flight set absorbs it.

- [ ] **Step 3: Update the three loaders**

In `crates/app/src/main.rs`, `open_folder`, `open_collection` and `open_keyword` each build a
`paths` vector and pass it to `ThumbLoader::new`. In all three:

- delete the `let paths: Vec<_> = entries.iter().map(|e| e.id.path.clone()).collect();` line and
  the now-unused `let n = entries.len();` **only if `n` is used solely for `thumbs`** — check each
  site; `n` may be used elsewhere,
- change the loader construction to `ThumbLoader::new(space, move || ctx2.request_repaint())`,
- in the `Library { … }` literal, replace `thumbs: (0..n).map(|_| None).collect(),` with:

```rust
            cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
            last_want: Vec::new(),
```

- [ ] **Step 4: Update the drain site**

In `crates/app/src/shell.rs`, replace the "Pull finished thumbnails into textures" block:

```rust
        // Pull finished thumbnails into textures.
        if let Some(lib) = &mut self.library {
            for res in lib.loader.drain() {
                if let Some(t) = res.thumb {
                    let bytes = t.rgba.len();
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    let tex = ctx.load_texture(
                        format!("thumb:{}", res.path.display()),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    lib.cache.insert(res.path, tex, bytes);
                }
            }
        }
```

⚠ The texture name is derived from the path rather than an index. Two different photos must never
share a texture name, or egui serves one image under both.

- [ ] **Step 5: Update `rebuild_entries`**

In `crates/app/src/main.rs`, `rebuild_entries` currently carries thumbnails across by hand. Delete
the whole `let known: std::collections::HashMap<PathBuf, egui::TextureHandle> = …collect();`
statement and the `lib.thumbs = lib.entries.iter().map(|e| known.get(&e.id.path).cloned()).collect();`
statement. Replace the doc comment's second paragraph (the `⚠ Rebuilds thumbs and the loader too…`
note) with:

```rust
    /// ⚠ Touches neither the thumbnail cache nor the loader: both are keyed by **path**, so a
    /// rebuild that only shifts indices invalidates nothing. This used to have to carry the
    /// textures across by hand, and getting it wrong greyed out the whole folder.
```

- [ ] **Step 6: Update `apply_display_space`**

In `crates/app/src/main.rs`, in `apply_display_space`, replace the `if let Some(lib) = …` body:

```rust
        if let Some(lib) = self.library.as_mut() {
            let ctx2 = ctx.clone();
            lib.loader = ThumbLoader::new(space, move || ctx2.request_repaint());
            // ⚠ Every cached thumbnail was converted at load, so they all describe the old space.
            lib.cache.clear();
            // ⚠ And the want-list, or `sync_thumb_window` compares against a list that is now
            // satisfied by nothing and never re-asks.
            lib.last_want.clear();
        }
```

- [ ] **Step 7: Point the two views at the cache and set the window**

In `crates/app/src/library.rs`, inside the cell loop, replace `lib.thumbs[i].as_ref()` with:

```rust
                                    lib.cache.get(&lib.entries[i].id.path),
```

Then, immediately after the `egui::Grid::new("thumb_grid").show(…)` closure returns and still
inside the `ScrollArea` closure, add:

```rust
                // ⚠ In this task the window is everything visible, so behaviour is unchanged.
                // Task 5 narrows it to the drawn rows plus a margin.
                let window: Vec<PathBuf> = visible
                    .iter()
                    .map(|&i| lib.entries[i].id.path.clone())
                    .collect();
                lib.sync_thumb_window(&window);
```

Add `use std::path::PathBuf;` to the file's imports if it is not already present.

In `crates/app/src/filmstrip.rs`, replace `lib.thumbs[i].as_ref()` with:

```rust
                            lib.cache.get(&lib.entries[i].id.path),
```

and after the `ui.horizontal(…)` closure, still inside the `ScrollArea` closure, add the same
window block (with `use std::path::PathBuf;` added to that file's imports).

⚠ Both views set the window because they are never on screen together — whichever is drawing owns
it. If that ever changes, they must contribute to a union instead of overwriting each other.

- [ ] **Step 8: Build and fix what the compiler names**

```bash
cargo build -p app 2>&1 | tail -40
```

Expected: a short list of remaining `thumbs` references. Fix each by reading its context; there
should be none outside the files above, but `cargo build` is the authority, not this plan.

- [ ] **Step 9: Full verification**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green. Test count is the Task 3 count; nothing new was added here.

- [ ] **Step 10: Run the app and confirm nothing changed**

```bash
cargo run --release -p app
```

Open a folder. The grid must fill exactly as before, the filmstrip must show thumbnails in
Develop, and creating a virtual copy (⌘') must not grey the folder out.

- [ ] **Step 11: Commit Tasks 3 and 4 together**

```bash
git add crates/app/src/thumb_loader.rs crates/app/src/main.rs crates/app/src/shell.rs \
        crates/app/src/library.rs crates/app/src/filmstrip.rs crates/app/examples/bench_thumbs.rs
git commit -m "refactor(app): key thumbnails by path and make the loader demand-driven"
```

---

## Task 5: Virtualise the grid

**Files:**
- Modify: `crates/app/src/library.rs`

- [ ] **Step 1: Write the failing test for the row mapping**

The row-break rule survives the rewrite even though its old failure mode does not, so its test is
*replaced*, not deleted. In `crates/app/src/library.rs`, add above the existing `#[cfg(test)]`
block's closing brace:

```rust
    #[test]
    fn rows_consume_the_visible_positions_densely_and_in_order() {
        // ⚠ Replaces `the_grid_breaks_rows_by_position_not_by_entry_index`. That bug — counting
        // rows by a *sparse* entry index — is now structurally impossible, because a row is
        // computed from the position directly. What still has to hold is that the rows together
        // cover every position exactly once, in order, with only the last one partial.
        const COLS: usize = 4;
        for n in [0usize, 1, 3, 4, 5, 11, 12] {
            let rows = n.div_ceil(COLS);
            let mut seen = Vec::new();
            for row in 0..rows {
                let span = cells_in_row(row, COLS, n);
                assert!(
                    span.len() <= COLS,
                    "row {row} of {n} holds {} cells",
                    span.len()
                );
                seen.extend(span);
            }
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "n = {n}");
        }
    }

    #[test]
    fn a_row_past_the_end_is_empty() {
        // A stale scroll offset can name a row that no longer exists after a filter change.
        assert!(cells_in_row(99, 4, 10).is_empty());
    }
```

And add the `todo!()` stub above `grid_columns` so the file compiles:

```rust
fn cells_in_row(row: usize, cols: usize, n_visible: usize) -> std::ops::Range<usize> {
    todo!()
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app --bin Rustroom cells_in_row
cargo test -p app --bin Rustroom rows_consume
```

Expected: panics at `not yet implemented`.

- [ ] **Step 3: Implement `cells_in_row`**

```rust
/// The positions in `visible` that belong to grid row `row`.
///
/// ⚠ Positions, never entry indices. Once a format or star filter is on, `visible` is *sparse*;
/// the grid's geometry is a function of the *position* in that list, and conflating the two is
/// the bug this project has now made three times.
fn cells_in_row(row: usize, cols: usize, n_visible: usize) -> std::ops::Range<usize> {
    let start = row.saturating_mul(cols).min(n_visible);
    let end = start.saturating_add(cols).min(n_visible);
    start..end
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --bin Rustroom cells_in_row
cargo test -p app --bin Rustroom rows_consume
```

Expected: both pass.

- [ ] **Step 5: Delete the superseded test**

Delete `the_grid_breaks_rows_by_position_not_by_entry_index` and the `grid_rows` helper above it
(the helper exists only for that test). Read the surrounding lines first — `grid_columns` and
`grid_columns_leaves_room_for_the_gaps` stay.

- [ ] **Step 6: Rewrite the grid body**

In `crates/app/src/library.rs`, replace the whole
`egui::ScrollArea::vertical().show(ui, |ui| { … })` block. The cell body is reproduced in full
below; only the loop around it changes:

```rust
        // ⚠ How many rows of prefetch to keep resident beyond the drawn ones. Latency, not
        // correctness: at 0 the grid is still right, it just shows grey while you scroll. No test
        // covers this number for that reason.
        const MARGIN_ROWS: usize = 2;

        let cols = grid_columns(ui.available_width(), cell, ui.spacing().item_spacing.x);
        let visible = lib.visible.clone();
        let rows = visible.len().div_ceil(cols);
        let name_size = ui.style().text_styles[&egui::TextStyle::Small].size * cell
            / rustroom_app::settings::DEFAULT_CELL as f32;

        let drawn = egui::ScrollArea::vertical()
            .show_rows(ui, cell, rows, |ui, row_range| {
                for row in row_range.clone() {
                    ui.horizontal(|ui| {
                        for pos in cells_in_row(row, cols, visible.len()) {
                            let i = visible[pos];
                            let box_size = egui::vec2(cell - 12.0, (cell - 12.0) * 0.66);
                            let resp = ui
                                .allocate_ui(egui::vec2(cell, cell), |ui| {
                                    // Pin the cell to its full size. Without this `allocate_ui`
                                    // reports only the space the content used, so cells varied
                                    // with their thumbnail — the filmstrip's reflow bug in its
                                    // grid form, where `egui::Grid`'s column alignment hid it.
                                    ui.set_min_size(egui::vec2(cell, cell));
                                    ui.vertical_centered(|ui| {
                                        thumb::cell(
                                            ui,
                                            box_size,
                                            lib.cache.get(&lib.entries[i].id.path),
                                            lib.entries[i].id.copy,
                                            lib.marks
                                                .get(&lib.entries[i].id)
                                                .copied()
                                                .unwrap_or_default(),
                                        );
                                        let e = &lib.entries[i];
                                        // ⚠⚠ `label()`, not `file_name`. Every copy of a file has
                                        // the same `file_name`, so the grid showed three identical
                                        // captions and a copy's own name could never appear at all.
                                        //
                                        // ⚠ Only the **active** row is editable. A text field in
                                        // every cell would fight the click that selects a photo,
                                        // and would put hundreds of them on screen at once.
                                        if let Some(r) = name_edit::caption_or_field(
                                            ui,
                                            &e.id,
                                            &e.file_name,
                                            e.name.as_deref(),
                                            rename_draft,
                                            name_size,
                                        ) {
                                            rename = Some((e.id.clone(), r));
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());

                            thumb::selection_border(
                                ui,
                                resp.rect,
                                lib.selection.active() == Some(i),
                                lib.selection.contains(i),
                            );
                            if resp.clicked() {
                                let mods = ui.input(|inp| inp.modifiers);
                                apply_click(&mut lib.selection, i, mods);
                            }
                            if resp.double_clicked() {
                                open_index = Some(i);
                            }
                        }
                    });
                }
                row_range
            })
            .inner;

        // ⚠ The window is the drawn rows widened by the prefetch margin, mapped through
        // positions to entry indices to paths. Built after drawing so it reflects what egui
        // actually laid out this frame.
        let first = drawn.start.saturating_sub(MARGIN_ROWS);
        let last = drawn.end.saturating_add(MARGIN_ROWS).min(rows);
        let mut window: Vec<PathBuf> = Vec::new();
        for row in first..last {
            for pos in cells_in_row(row, cols, visible.len()) {
                let path = &lib.entries[visible[pos]].id.path;
                // ⚠ De-duplicated: a file's copies are separate rows sharing one path, and the
                // cache holds one entry per path.
                if !window.iter().any(|p| p == path) {
                    window.push(path.clone());
                }
            }
        }
        lib.sync_thumb_window(&window);
```

⚠ Two changes inside that body against what is on disk today, and nothing else: the thumbnail now
comes from `lib.cache.get(&lib.entries[i].id.path)` (Task 4 already made that edit), and the
trailing `if (pos + 1) % cols == 0 { ui.end_row(); }` is **gone** — `egui::Grid` is no longer
breaking rows, the `ui.horizontal` per row does it. Everything else is unchanged; diff against
`git show HEAD:crates/app/src/library.rs` if anything looks off.

Notes for whoever writes this:

- `show_rows` takes the row height **without** spacing and adds `item_spacing.y` itself, so pass
  `cell`, not `cell + spacing`.
- `egui::Grid` is gone. The cell body already calls `ui.set_min_size(egui::vec2(cell, cell))`
  inside its `allocate_ui`, which is what kept cells square under `Grid`, so alignment survives.
- The `window` block sits **outside** the `ScrollArea` closure, which is why `show_rows` returns
  `row_range` — `lib` is borrowed inside the closure and cannot also be borrowed mutably there.

- [ ] **Step 7: Give each cell a stable widget id**

⚠⚠ **Virtualisation breaks egui's auto-generated widget ids, and this bites the rename field.**
`name_edit::field` adds a `TextEdit::singleline` with **no `id_salt`**, so its id comes from the
ui's id chain plus an auto-incrementing counter. `show_rows` calls `skip_ahead_auto_ids(min_row)` —
it skips `min_row` ids, but each row contains `cols` widgets, so the counter does **not** line up.
Scroll by one row with a rename open and every drawn widget's id shifts.

The failure is not subtle: the `TextEdit` loses its stored cursor/selection state, and worse, the
autofocus guard is `ui.memory(|m| m.focused()).is_none()` — the *old*, now-unreachable id still
holds focus, so the guard sees `Some(..)` and the new field never takes it. The rename box goes
dead and typing does nothing.

⚠ This hazard does **not** exist today, which is why nothing catches it: every cell is drawn every
frame in the same order, so the counter is stable. Virtualisation introduces it.

Wrap each cell body in a `push_id` keyed by the photo, so ids depend on *which photo* rather than
on how many widgets happened to be drawn first. Inside the `for pos in cells_in_row(..)` loop:

```rust
                            let i = visible[pos];
                            // ⚠⚠ A stable id per photo, not egui's auto-id counter. Under
                            // `show_rows` the counter shifts with the scroll position, which
                            // silently kills the rename field's focus. Keyed by `PhotoId`, so it
                            // is also stable across sorting and filtering.
                            ui.push_id(&lib.entries[i].id, |ui| {
                                // …the cell body from Step 6…
                            });
```

`PhotoId` derives `Hash` already (`crates/app/src/photo_id.rs:9`), so `push_id` accepts it
directly.

⚠ The closure returns `()`, so `open_index`, `rename` and the rest are still set by capture exactly
as before — but `resp` is now inside the closure, so anything after it that used `resp` must move
inside too. Move the whole body in, unchanged.

- [ ] **Step 8: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green, test count up by 2.

- [ ] **Step 9: Run the app and look at it**

```bash
cargo run --release -p app
```

Check: the grid fills the window with no gaps and no overflowing rows; scrolling is smooth and
loads as you go; the scrollbar's length matches the folder; **turn on a format filter and scroll**
— a sparse `visible` is where the old row-break bug lived, so it is the case to exercise;
selection borders land on the right cells; clicking a thumbnail still opens it.

⚠ **And the id check, which no test covers:** click a photo's caption to start renaming it, type a
character, **scroll the grid**, and confirm the field still has focus and still accepts typing.
That is Step 7's failure mode, and it is only observable by hand.

- [ ] **Step 10: Commit**

```bash
git add crates/app/src/library.rs
git commit -m "perf(app): draw and load only the visible rows of the Library grid"
```

---

## Task 6: Virtualise the filmstrip

**Files:**
- Modify: `crates/app/src/filmstrip.rs`

- [ ] **Step 1: Rewrite the strip body**

`show_rows` is vertical-only, so the horizontal strip uses `show_viewport`, which hands back the
visible rect in content coordinates. Replace the `egui::ScrollArea::horizontal().show(ui, …)`
block. The cell body is reproduced in full below; only the loop around it changes:

```rust
            // ⚠ Latency, not correctness — see the grid's MARGIN_ROWS.
            const MARGIN_CELLS: usize = 4;

            let visible = lib.visible.clone();
            let step = box_size.x + ui.spacing().item_spacing.x;
            let content = visible.len() as f32 * step;
            let extent = ui.available_width();

            // ⚠ `scroll_to_me` cannot work on a virtualised strip: an off-window cell is never
            // drawn, so there is no response to scroll to. Computing the offset is also the
            // correct fix for the M2.2 drift bug — egui computes a `scroll_to_me` target once and
            // never recomputes it, so the centred photo slid left as thumbnails ahead of it
            // loaded and reflowed the content.
            let mut area = egui::ScrollArea::horizontal();
            if want_scroll {
                if let Some(pos) = active.and_then(|a| visible.iter().position(|&i| i == a)) {
                    area = area.horizontal_scroll_offset(
                        rustroom_app::thumb_window::centre_offset(pos, step, extent, content),
                    );
                }
            }

            let span = area
                .show_viewport(ui, |ui, viewport| {
                    let span = rustroom_app::thumb_window::visible_span(
                        viewport.min.x,
                        viewport.width(),
                        step,
                        visible.len(),
                        0,
                    );
                    ui.horizontal(|ui| {
                        // Reserve the space the skipped cells would have taken, so the scrollbar
                        // and the drawn cells agree about where they are.
                        ui.add_space(span.start as f32 * step);
                        for &i in &visible[span.clone()] {
                            // ⚠ A stable id per photo — see the grid's Step 7. The filmstrip has
                            // no text field, so the failure is milder, but a click resolving
                            // against a shifted id for one frame is the same hazard and
                            // `push_id` costs nothing.
                            ui.push_id(&lib.entries[i].id, |ui| {
                                // The filmstrip's cell *is* its click target, so it opts in here.
                                let resp = thumb::cell(
                                    ui,
                                    box_size,
                                    lib.cache.get(&lib.entries[i].id.path),
                                    lib.entries[i].id.copy,
                                    lib.marks
                                        .get(&lib.entries[i].id)
                                        .copied()
                                        .unwrap_or_default(),
                                )
                                .interact(egui::Sense::click());
                                thumb::selection_border(
                                    ui,
                                    resp.rect,
                                    lib.selection.active() == Some(i),
                                    lib.selection.contains(i),
                                );
                                if resp.clicked() {
                                    let mods = ui.input(|inp| inp.modifiers);
                                    // Only a plain click moves the active photo, so only a
                                    // plain click opens — a Cmd- or Shift-click builds the
                                    // target set without triggering a decode.
                                    if apply_click(&mut lib.selection, i, mods) {
                                        open_index = Some(i);
                                    }
                                }
                            });
                        }
                        ui.add_space((visible.len() - span.end) as f32 * step);
                    });
                    span
                })
                .inner;

            let first = span.start.saturating_sub(MARGIN_CELLS);
            let last = span.end.saturating_add(MARGIN_CELLS).min(visible.len());
            let mut window: Vec<PathBuf> = Vec::new();
            for &i in &visible[first..last] {
                let path = &lib.entries[i].id.path;
                if !window.iter().any(|p| p == path) {
                    window.push(path.clone());
                }
            }
            lib.sync_thumb_window(&window);
```

⚠ Against what is on disk today, the body above differs in exactly two ways: the thumbnail comes
from `lib.cache.get(&lib.entries[i].id.path)` (Task 4 already made that edit), and the
`if want_scroll && Some(i) == active { resp.scroll_to_me(Some(egui::Align::Center)); }` lines are
**gone** — that job now belongs to `horizontal_scroll_offset` above. Diff against
`git show HEAD:crates/app/src/filmstrip.rs` if anything looks off.

`app.filmstrip_scrolled_to = active;` after the panel stays exactly as it is: it is what stops the
strip re-centring every frame.

- [ ] **Step 2: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green, test count unchanged.

- [ ] **Step 3: Run the app and look at it**

```bash
cargo run --release -p app
```

Check, in Develop with a folder loaded: the strip shows thumbnails; **arrow to a photo near the end
of the folder** and confirm it centres and *stays* centred as neighbours load — that is the M2.2
drift bug's exact reproduction; the strip scrolls smoothly by hand; resizing the panel rescales the
cells and still centres correctly; a format filter still narrows the strip.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/filmstrip.rs
git commit -m "perf(app): draw and load only the visible cells of the filmstrip"
```

---

## Task 7: Headless view test, sabotages and measurement

**Files:**
- Modify: `crates/app/src/test_app.rs`
- Modify: `crates/app/src/library.rs` (test module)
- Create: `crates/app/examples/bench_library_open.rs`

- [ ] **Step 1: Add a `Library` fixture**

In `crates/app/src/test_app.rs`, below `for_test()`, add:

```rust
/// A `Library` of `n` synthetic entries, with no files behind them.
///
/// ⚠ Exhaustive field literal on purpose, following `for_test`: a new field on `Library` is a
/// compile error here, which forces a decision about what a test should see.
pub fn library_of(n: usize) -> Library {
    let scan: Vec<catalog::CatalogEntry> = (0..n)
        .map(|i| {
            let path = std::path::PathBuf::from(format!("/nonexistent/rustroom_{i:04}.RAF"));
            catalog::CatalogEntry {
                file_name: format!("rustroom_{i:04}.RAF"),
                path,
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }
        })
        .collect();
    let entries = rustroom_app::entries::build(&scan, |_| rustroom_app::sidecar::Sidecar::default());
    let mut lib = Library {
        source: LibrarySource::Folder(std::path::PathBuf::from("/nonexistent")),
        scan,
        entries,
        cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
        last_want: Vec::new(),
        selection: Selection::default(),
        loader: ThumbLoader::new(color::OutputSpace::Srgb, || {}),
        filter: FormatFilter::default(),
        visible: Vec::new(),
        marks: std::collections::HashMap::new(),
        min_stars: 0,
    };
    lib.refilter();
    lib
}
```

⚠ Check every field name and type against `struct Library` in `main.rs` before running — the
literal must be exhaustive and the compiler is the authority. `Sidecar::default()` must exist; if
it does not, use whatever `sidecar::load_all` returns for a missing file.

- [ ] **Step 2: Write the failing headless test**

In `crates/app/src/library.rs`'s test module:

```rust
    #[test]
    fn only_the_visible_rows_are_asked_for() {
        // ⚠⚠ The point of the whole change, asserted where it is observable: with 2,000 entries
        // and a viewport a few rows tall, the want-list must be a few dozen paths, not 2,000.
        //
        // ⚠ Asserted on the *want-list*, not on the drawn cell count: drawing fewer cells is
        // egui's `show_rows` doing its job, while asking for fewer thumbnails is ours. The
        // memory and the 34-second fill are both consequences of the ask.
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.view = AppView::Library;
        app.library = Some(crate::test_app::library_of(2000));

        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(&mut app, ui, ctx));
        });

        let asked = app.library.as_ref().unwrap().last_want.len();
        assert!(asked > 0, "nothing was asked for at all");
        assert!(
            asked < 300,
            "asked for {asked} thumbnails — the grid is not windowed"
        );
    }
```

⚠ `library_of`, `for_test`, `test_ui::ctx` and `test_ui::screen` are bin-module items; this test
lives in the bin, so they are reachable. Check `test_ui::ctx()`'s exact signature before writing
the `ctx.run` call and follow the pattern already used by the canvas tests in
`crates/app/src/develop/canvas.rs` rather than the sketch above — that file has a working example
of driving a real `show` through `RawInput`.

- [ ] **Step 3: Run it**

```bash
cargo test -p app --bin Rustroom only_the_visible_rows
```

Expected: **passes**, because Tasks 5 and 6 already did the work. That is the wrong order for TDD,
so verify it discriminates:

- [ ] **Step 4: Sabotage the window and watch it fail**

Temporarily change the grid's window block to use `0..rows` instead of `first..last`. Re-run:

```bash
cargo test -p app --bin Rustroom only_the_visible_rows 2>&1 | tail -20
```

Expected: fails with `asked for 2000 thumbnails`. **Confirm it compiled.** Restore, re-run green.

- [ ] **Step 5: Run the remaining sabotages and record the results**

Run each, confirm it **compiles**, note which test fails, then restore and confirm green:

| sabotage | expected |
|---|---|
| `centre_offset` returns the unclamped value | `centring_the_last_item_does_not_scroll_past_the_end` fails |
| `sync_thumb_window` calls `want` unconditionally (drop the `!=` guard) | **nothing fails** — the guard is a per-frame cost, not a behaviour. Record it as such. |
| `MARGIN_ROWS = 0` | **nothing fails** — a margin is latency, not correctness. Record it as such. |

⚠ Two of these are expected *not* to fail, and that must be written down rather than papered over
with a test that appears to cover them. A sabotage that does not change behaviour is not evidence
about the test.

- [ ] **Step 6: Write the measurement example**

Create `crates/app/examples/bench_library_open.rs`:

```rust
//! How much does a folder cost to browse — thumbnails resident, and time to a usable grid?
//!
//! The companion to `bench_thumbs`. Run it when touching `ThumbLoader`, `ThumbCache` or the
//! grid's windowing: `cargo run --release -p app --example bench_library_open <folder>`.
use rustroom_app::thumb_cache::{ThumbCache, THUMB_BUDGET_BYTES};
use rustroom_app::thumb_loader::ThumbLoader;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench_library_open <folder>");
    let entries = catalog::scan_folder(std::path::Path::new(&dir)).expect("scan");
    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
    println!("{} entries; budget {} MB", paths.len(), THUMB_BUDGET_BYTES >> 20);

    // What a first screenful costs — the number that decides whether a folder feels instant.
    for window in [24usize, 120, paths.len()] {
        let window = window.min(paths.len());
        let wanted: Vec<PathBuf> = paths[..window].to_vec();
        let mut cache: ThumbCache<usize> = ThumbCache::with_default_budget();
        let t = Instant::now();
        let loader = ThumbLoader::new(color::OutputSpace::Srgb, || {});
        loader.want(&wanted);
        let mut got = 0usize;
        while got < window && t.elapsed().as_secs() < 300 {
            for r in loader.drain() {
                if let Some(th) = r.thumb {
                    let bytes = th.rgba.len();
                    cache.insert(r.path, bytes, bytes);
                }
                got += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        cache.retain_window(&wanted);
        println!(
            "window {window:5}: {:6.2} s, {:5} MB resident",
            t.elapsed().as_secs_f64(),
            cache.bytes() >> 20
        );
    }
}
```

- [ ] **Step 7: Measure, on the real folder**

```bash
cargo run --release -p app --example bench_library_open ~/Desktop/fuji
```

Record the three lines. The `window 24` figure is the "time to a usable grid" claim; the
`window <all>` figure is what the old behaviour cost. **Write the actual numbers into CLAUDE.md
in Step 8** — do not carry the plan's estimates forward as if they were measurements.

- [ ] **Step 8: Update the docs**

- `docs/superpowers/specs/2026-08-27-windowed-browsing-design.md` — change the header to
  `**Status:** built 2026-08-27`, and replace the estimated table's "time to fill" column with the
  measured figures.
- `CLAUDE.md` — add a section for this work following the house style: what changed, the measured
  numbers, the ⚠ findings (the pin, the path key, the `Drop` requirement now that workers park,
  `scroll_to_me` becoming arithmetic, the two inert sabotages), and the accepted caveats from the
  spec. Update the test count in the RESUME block and set the next action to **M4.3a-ii — the
  "All Photos" view and the filter bar**.
- The Obsidian roadmap at
  `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md` — record the
  M4.3 decomposition (a-i / a-ii / b / c) under M4.3 and mark a-i done.

- [ ] **Step 9: Final verification and commit**

```bash
rustup update stable
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
echo "exit=$?"
```

⚠ Check the exit code, not the output. Then:

```bash
git add -A
git commit -m "test(app): headless window test, measurement example and docs for windowed browsing"
```

⚠ **Do not push.** Report the branch and the measured numbers; the user merges and pushes.

---

## Definition of done

- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --all`
  all green, verified by exit code.
- **All 47 goldens byte-identical.** Nothing here touches `pipeline`; if a golden moves, something
  is very wrong.
- The grid and filmstrip both draw and load only what is on screen, proved by
  `only_the_visible_rows_are_asked_for` and by the sabotage in Task 7 Step 4.
- Measured figures for a first screenful versus the whole folder recorded in CLAUDE.md.
- Confirmed by hand in the running app: scrolling a filtered grid, and centring a photo at the end
  of a folder in the filmstrip.

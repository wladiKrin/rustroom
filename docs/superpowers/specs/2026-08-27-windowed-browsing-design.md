# Windowed browsing — M4.3a-i

**Date:** 2026-08-27
**Status:** built 2026-08-27
**Slice of:** M4.3 (search, smart collections, "all photos")

## Why this exists, and why it is not about search

M4.3's roadmap entry is three independent subsystems, and this document covers none of them
directly. The decomposition agreed with the user on 2026-08-27:

| slice | what | needs |
|---|---|---|
| **M4.3a-i** | **windowed browsing** — this document | nothing new; benefits every source today |
| **M4.3a-ii** | **"All Photos" view + a filter bar** over what the catalog already knows | a fourth loader; `Library.filter` gains fields |
| **M4.3b** | **capture metadata** — date taken, camera — and filtering on it | schema v4, an EXIF/libraw read during scan, a backfill decision |
| **M4.3c** | **smart collections** — a saved rule that auto-populates | a rule representation, storage, a rule editor |

M4.3's own "Done when" is *"a smart collection auto-populates from a rule at 10k-image scale with
acceptable performance"*. **The 10k requirement lands here, not on M4.3c.** "All Photos" is the
first `LibrarySource` that can be large, and the Library is fully materialised today.

### The measurement that motivates it

`library.rs:133` draws *every* visible entry each frame inside one `egui::Grid`. `Library.thumbs`
holds a live `TextureHandle` per entry, forever. `ThumbLoader` takes a fixed path list and grinds
through it in ascending order with no notion of what is on screen. `filmstrip.rs:38` is a second
consumer of the same `thumbs` and likewise draws every cell.

Arithmetic from CLAUDE.md's measured figures (≤512 px thumbnails, ~0.7 MB RGBA each, 41 ms/thumb
across 6 workers):

| library size | GPU textures | time to fill | cells laid out per frame |
|---|---|---|---|
| 860 (the author's, today) | ~600 MB | ~34 s | 860 |
| 10,000 | **~7 GB** | ~68 s | 10,000 |

**Measured afterwards** on that folder (830 entries, `cargo run --release -p app --example
bench_library_open ~/Desktop/fuji`), which confirmed the estimates and then some:

| | whole folder (the old behaviour) | first screenful (the new one) |
|---|---|---|
| time to a usable grid | **42.0 s** | **1.5 s** |
| resident textures | **552 MB** | **15 MB** |

A 120-thumbnail window — a very large one — costs 6.6 s and 79 MB. The steady state is bounded by
`THUMB_BUDGET_BYTES` at 256 MB however large the folder, so the 10,000 row above never happens.

⚠ **The 860 row is the point as much as the 10k row.** That cost is already paid on every folder
open. This slice is not speculative work for a library nobody has; it is a fix for the browser that
exists.

### Why it is its own slice

This project's precedent: M3.1a split headless-from-UI *so that a canvas bug could never be
confused with a model bug*; the brush, masking and colour grading all did the same. Windowing is
pure index arithmetic plus a bounded cache, provable against the existing 860-photo folder with
**no search code in existence**. Landing it with search would leave two candidate causes for any
grid misbehaviour instead of one.

## Architecture

### 1. `app::thumb_window` — the arithmetic, deliberately egui-free

A grid of rows and a strip of cells are the same one-dimensional problem, so one function serves
both views:

```rust
/// Indices of items whose cells intersect the viewport, widened by `margin` items each way.
pub fn visible_span(offset: f32, extent: f32, step: f32, count: usize, margin: usize) -> Range<usize>;

/// Scroll offset that centres `index`, clamped to the content.
pub fn centre_offset(index: usize, step: f32, extent: f32, content: f32) -> f32;
```

Grid: items are **rows**, `step = cell + spacing.y`. Filmstrip: items are **cells**,
`step = cell + spacing.x`.

⚠ Plain `f32`/`usize`, no egui types — the `view_region` precedent, whose own doc says it is
*"deliberately egui-free so it can be tested without a window"*. Clamping and off-by-one errors are
then testable rather than only visible as a wrong image.

`centre_offset` is what replaces `scroll_to_me`.

### 2. `app::thumb_cache::ThumbCache` — bounded residency

Replaces `Library.thumbs: Vec<Option<egui::TextureHandle>>`. An LRU under
`THUMB_BUDGET_BYTES`, bounding a 10k library instead of letting it reach 7 GB.

⚠⚠ **Shipped at 256 MB; raised to 512 MB the same day, from use.** A 415-file folder measures
276 MB, so a round trip to the end of it evicted the top ~30 thumbnails and they visibly
reloaded — the boundary was landing on ordinary folders. At 512 MB the cache holds ~749
thumbnails, so anything up to ~700 photos is browsed end-to-end and back with no eviction, and
512 matches `MID_BUDGET_BYTES`, whose *single* full-resolution checkpoint is ~313 MB. The sizing
is a compile-time assertion, so lowering it again is a build error.

The budget is guarded by a **module-level** `const _: () = assert!(…)`, not an associated one.
⚠ CLAUDE.md records that an associated `const` inside an `impl` is only evaluated where it is used,
so `PrefixCache`'s first attempt at this compiled happily with a budget of zero — a guard that did
not guard.

```rust
pub fn get(&self, path: &Path) -> Option<&egui::TextureHandle>;
pub fn insert(&mut self, path: PathBuf, tex: egui::TextureHandle, bytes: usize);
pub fn retain_window(&mut self, window: &[PathBuf]); // promotes the window, then evicts to budget
pub fn contains(&self, path: &Path) -> bool;
```

⚠ `get` does **not** touch the LRU, and recency is set by `retain_window` instead. Two reasons: the
thing that makes a thumbnail recent is *being on screen*, which is precisely what `retain_window`
is told; and a `&mut self` accessor called from inside the draw loop would fight the shared borrows
of `entries` and `visible` that the loop already holds.

⚠⚠ **The window is pinned, not merely preferred.** Eviction may never touch a path that is on
screen, however far the budget falls. Without that, a large window silently blanks the very cells
being drawn, and the failure mode is grey thumbnails that look like a slow loader rather than a
bug.

#### ⚠⚠ Keyed by `PathBuf`, not by `entries` index

This is the substantive design change, and three things fall out of it:

- **Copies of one file share one texture.** `thumbs` is index-parallel to `entries` today, and
  `open_folder`'s own comment says the path list *"deliberately repeats a path once per copy"* — so
  a file with three copies currently holds three pixel-identical 0.7 MB textures. Path-keying
  deduplicates that for free, and it is consistent with the model already recorded in CLAUDE.md:
  *copies share a pixel-identical thumbnail*, because a Library thumbnail is the camera's embedded
  JPEG.
- **A re-sort or `rebuild_entries` invalidates nothing.** `rebuild_entries` currently *must* rebuild
  `thumbs` and restart the loader because the indices have just shifted. Both obligations go away.
- **Indices never leave the UI.** Sparse-versus-dense index confusion is this project's
  third-most-repeated bug family — the grid row break, the crop feedback loop, `render_max_dim`.
  Path-keying removes the hazard at the boundary rather than guarding against it downstream.

### 3. `ThumbLoader` — demand-driven, speaking paths

```rust
ThumbLoader::new(display_space, on_progress)   // starts idle; no path list at construction
loader.want(&[PathBuf])                        // replaces the queue wholesale
loader.drain() -> Vec<ThumbResult { path, thumb }>
```

The UI computes "paths in the window that have no texture" — the window returned by
`visible_span` already includes the margin — and calls `want` **only when the list changes**. ⚠ That guard is the `apply_view_demosaic` rule, which CLAUDE.md now cites four
separate times: a body that runs every frame needs a change comparison or it does its work sixty
times a second.

Workers pop the front of the queue and park on a condvar when it is empty — they must survive an
empty queue and wake when the window moves, rather than exiting as the fixed-list version could.
An **in-flight set** stops a path being fetched twice while a partly-overlapping want-list is
written each time the window moves.

⚠ **Replacing the list is the cancellation, and no cancellation mechanism is needed.** A
scrolled-past request is simply never popped. That is "abandon, don't abort" — the trade
`DecodeWorker` and `batch_export` both already make, for the same reason: the alternative is
interrupting work that has no safe interruption point.

⚠ Why the UI owns "what is loaded" rather than the loader: the UI owns the cache, so it is the only
thing that knows. A loader that kept its own shadow copy would have to be told about every
eviction — two facts that can disagree, which is exactly the failure `render_max_dim` and
`Session::geometry_mode` were each fixed by *deriving* rather than storing alongside.

Kept unchanged: `worker_count()`'s headroom rule (`cores - 1`, capped at 6, so thumbnail loading
never starves the render pool), the injected loader fn that makes the tests fixture-free, and the
display-space conversion.

### 4. The two views

**Grid (`library.rs`)** — `ScrollArea::vertical().show_rows(…)`, which hands back the row range
directly; that range *is* the window. `egui::Grid` goes away, since it was only supplying row
breaks and column alignment, and `ui.set_min_size` already pins the cell. Rows become explicit
`ui.horizontal`.

⚠ **The `pos`-versus-`i` row-break bug becomes structurally impossible**, because the row index is
no longer counted from a sparse list. `the_grid_breaks_rows_by_position_not_by_entry_index` is
therefore *replaced* by a test of the row → position → entry-index mapping, not deleted: the rule it
pinned still matters, only its failure mode has moved.

**Filmstrip (`filmstrip.rs`)** — `ScrollArea::horizontal().show_viewport(…)`, since there is no
horizontal `show_rows`. The leading gap is allocated, then only the cells in span are drawn.

⚠ `scroll_to_me` is replaced by `centre_offset`, and **that is an improvement rather than a
workaround.** egui computes a `scroll_to_me` target offset **once**, as an absolute content-space
offset, and never recomputes it (`scroll_area.rs`: `let target_offset = state.offset[d] + delta`).
CLAUDE.md records the resulting drift bug from M2.2: the centred photo slid left as portrait
thumbnails *ahead* of it loaded and reflowed the content. Arithmetic placement has no such failure
mode, and it is the only option once an off-window cell is not drawn at all.

### 5. Ownership of the window

`Library` gains `cache: ThumbCache` and loses `thumbs`. Whichever view is drawing computes the
window and hands the want-list to the loader once per frame.

⚠ **One window at a time**, which holds only because Library and Develop are separate views and are
never on screen together. If that ever stops being true, the two views must contribute to one
union window rather than each overwriting the other's.

## What is removed

- `Library.thumbs: Vec<Option<egui::TextureHandle>>`
- The path list at `ThumbLoader` construction, in all three loaders (`open_folder`,
  `open_collection`, `open_keyword`)
- `rebuild_entries`'s obligation to rebuild `thumbs` and restart the loader
- `egui::Grid` and the row-break-by-position arithmetic in `library.rs`
- `scroll_to_me` in `filmstrip.rs`

## Testing

**Pure (`thumb_window`)** — spans at the top, middle and bottom; margins clamped at both ends; a
count of zero yielding an empty range; `centre_offset` clamped so the first and last items do not scroll past the content.

**Cache (`thumb_cache`)** — the window survives budget pressure; LRU eviction order; two copies of
one file occupy one entry; the budget is respected once the window is small.

**Loader (`thumb_loader`)** — a replaced want-list abandons stale work; an in-flight path is not
requested twice; a worker wakes after an empty queue. The four existing tests are adapted, and the
injected-loader seam means none of them needs an image fixture.

**Headless UI (`test_app` / `test_ui`)** — only in-window cells are drawn, asserted by counting
AccessKit nodes; the selected photo's centring offset is correct.

### Sabotages to run and watch fail

- remove window pinning → a visible thumbnail is evicted
- unclamp `centre_offset` → the ends scroll past the content
- drop the in-flight set → a duplicate-load count rises

⚠ **One sabotage is expected *not* to fail anything, and that will be stated rather than papered
over:** setting the margin to 0 leaves every test green, because a margin is a latency
optimisation and not a correctness property. CLAUDE.md records that a sabotage which does not
change behaviour *"is not evidence about the test"*; recording that honestly is better than
inventing a test that appears to cover it.

Every sabotage is checked for **compile errors first**. CLAUDE.md records both halves of this trap:
a sabotage that fails to compile looks exactly like one that passes, and a harness that greps for
bare test names finds nothing because cargo prints module paths.

## Measurement

Before and after, on the author's 860-photo folder: time from folder open to a usable grid, and
steady-state texture bytes. An example in the style of `bench_thumbs`, kept in the repo.

## Accepted caveats

1. **Fast scrolling shows grey cells briefly.** Inherent to loading on demand; the margin softens
   it and does not remove it. This is the caveat most likely to be reported as a bug.
2. **One window at a time**, as above.
3. **The budget is a constant, not a setting.** 256 MB was chosen from the screen arithmetic, not
   measured against a preference. It is the one number to turn down if memory proves tight.
4. **An evicted thumbnail is re-read from disk.** An on-disk thumbnail cache is a separate feature
   this does not attempt, and remains deferred as it has been since M2.1.
5. **Nothing here is visible as a feature.** As with M4.1's plan 1: if you can tell it shipped,
   something is wrong — except that folders should become usable much faster.

## Not in this slice

The "All Photos" view, the filter bar, search of any kind, capture metadata, and smart collections.
They are M4.3a-ii, M4.3b and M4.3c respectively.

# The downloaded indicator, and the Immich folder's name and count — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Folders rail calls the Immich cache "immich (downloaded)" and counts what is really in it; a circled down-arrow marks the downloaded photos in an Immich grid.

**Architecture:** One `read_dir` of the cache root answers "which assets are here" for all three, cached on `RustroomApp` and refreshed at startup, on album open, and per completed download. The indicator is hand-painted, because the bundled font carries no glyph names to verify a codepoint against.

**Spec:** `docs/superpowers/specs/2026-09-23-immich-presence-design.md`. Read it before Task 1.

---

## Conventions

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By`. Never push.** Branch `immich-presence`, on top of the unmerged `immich-catalog-rows`.
- ⚠⚠ **`cargo test --workspace <name-filter>`, never `-p`.** A filter matching nothing **exits 0 having run zero tests** — check the passed count.
- **Read `$?` from its own command**, never through a `| grep`.
- ⚠⚠ **After any scripted edit:** `git diff <commit> -- <file> | grep '^-' | grep '///'` must be empty unless intended.
- ⚠ **A fixture that makes two different things equal hides the bug.** Slice 3's `asset("one")` gave `original_file_name: "one.jpg"`, so the UUID path and the display name coincided and four tests passed against a broken catalog. Keep ids and file names unlike each other.
- ⚠ Never run two gates concurrently.

## File structure

| File | Change |
|---|---|
| `crates/app/src/immich_source.rs` | `present_names` (⚠ **821 lines**, reported) |
| `crates/app/src/immich_catalog.rs` | `folder_display_name` |
| `crates/app/src/main.rs` | `immich_present` field, filled at startup |
| `crates/app/src/immich_flow.rs` | refresh on open and on completed download |
| `crates/app/src/collections_panel.rs` | the rail's label and count |
| `crates/app/src/thumb.rs` | the hand-painted indicator |
| `crates/app/src/library.rs` | pass the flag per cell |

---

### Task 1: One read_dir answers who is here

**Files:** `crates/app/src/immich_source.rs`, `crates/app/src/immich_catalog.rs`

- [ ] **Step 1: Failing tests**

In `immich_source.rs`'s tests:

```rust
    /// ⚠⚠ One `read_dir`, never a stat per asset: the grid draws hundreds of cells sixty times a
    /// second, and a stat per cell is the `apply_view_demosaic` mistake in a new costume.
    #[test]
    fn present_names_lists_what_is_in_the_cache_directory() {
        let d = std::env::temp_dir().join(format!("rr_present_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.join("b.jpeg"), b"x").unwrap();
        let got = present_names(&d);
        assert_eq!(got.len(), 2);
        assert!(got.contains(std::ffi::OsStr::new("a.jpg")));
        assert!(!got.contains(std::ffi::OsStr::new("c.jpg")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⚠ A cache directory that does not exist yet is **empty**, not an error: nothing has been
    /// downloaded, which is a perfectly ordinary state on a fresh install.
    #[test]
    fn present_names_on_a_missing_directory_is_empty() {
        assert!(present_names(std::path::Path::new("/nonexistent-rr-cache")).is_empty());
    }
```

In `immich_catalog.rs`'s tests:

```rust
    #[test]
    fn the_immich_cache_root_is_shown_as_immich_downloaded() {
        let root = std::path::Path::new("/c/rustroom/immich/https___host_example");
        assert_eq!(folder_display_name(root, Some(root)), "immich (downloaded)");
    }

    #[test]
    fn any_other_folder_keeps_its_basename() {
        let root = std::path::Path::new("/c/rustroom/immich/https___host_example");
        let other = std::path::Path::new("/Users/me/Pictures/Iceland");
        assert_eq!(folder_display_name(other, Some(root)), "Iceland");
    }

    /// ⚠ No server configured renames nothing — and a folder that merely *looks* like a host slug is
    /// not the configured root and keeps its own name.
    #[test]
    fn nothing_is_renamed_without_a_configured_root() {
        let looks_like_one = std::path::Path::new("/somewhere/https___host_example");
        assert_eq!(
            folder_display_name(looks_like_one, None),
            "https___host_example"
        );
        let real_root = std::path::Path::new("/c/rustroom/immich/https___other");
        assert_eq!(
            folder_display_name(looks_like_one, Some(real_root)),
            "https___host_example"
        );
    }
```

- [ ] **Step 2: Run, expect failure to compile.**

```bash
cargo test --workspace present_names > /tmp/t.log 2>&1; echo "EXIT: $?"
```

- [ ] **Step 3: Implement**

`immich_source.rs`:

```rust
/// The file names present in this server's cache directory.
///
/// ⚠⚠ **One `read_dir`, answering for every asset at once.** The alternative — `is_cached` per
/// thumbnail — is a stat per cell per frame in a grid that draws hundreds of cells sixty times a
/// second.
///
/// ⚠ A directory that does not exist yields an **empty** set rather than an error: nothing downloaded
/// yet is an ordinary state, not a failure.
pub fn present_names(root: &Path) -> std::collections::HashSet<std::ffi::OsString> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Default::default();
    };
    rd.flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name())
        .collect()
}
```

`immich_catalog.rs`:

```rust
/// What the Folders rail should call this folder.
///
/// ⚠ The Immich cache root's basename is the host slug — `https___immich_example` — which
/// says nothing to anybody. **Renamed rather than hidden** (user's call): clicking it opens the cache
/// directory as an ordinary folder, which shows exactly the photos that have been downloaded, so the
/// name is honest. The full path is still the row's hover.
pub fn folder_display_name(path: &Path, immich_root: Option<&Path>) -> String {
    if immich_root == Some(path) {
        return "immich (downloaded)".to_string();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}
```

- [ ] **Step 4: Run** `cargo test --workspace present_names` then `cargo test --workspace folder_display`, checking counts.

- [ ] **Step 5: Commit**

---

### Task 2: The set, cached and kept current

**Files:** `crates/app/src/main.rs`, `crates/app/src/immich_flow.rs`

- [ ] **Step 1: The field**

On `RustroomApp`, beside the other `immich_*` fields:

```rust
    /// The cache file names this server has on disk — the Folders rail's count and the grid's
    /// downloaded indicator both read it.
    ///
    /// ⚠⚠ Cached because the alternative is a stat per thumbnail per frame. Refreshed at startup, when
    /// an Immich library opens, and by one insert per completed download — never per frame.
    immich_present: std::collections::HashSet<std::ffi::OsString>,
```

Initialise it in the constructor from `immich_source::connection(&settings)` → `present_names(cache_root)`, and `Default::default()` in `test_app::for_test`.

⚠ The exhaustive `RustroomApp { .. }` literal will force both; that is the point of it.

- [ ] **Step 2: Refresh on open, insert on download**

A helper in `immich_flow.rs`:

```rust
    /// Re-read which of this server's assets are on disk.
    ///
    /// ⚠ On opening a library rather than per frame, so a photo downloaded — or deleted — outside
    /// Rustroom is picked up at the next open.
    pub(crate) fn refresh_immich_present(&mut self) {
        self.immich_present = match self.immich_cache_root() {
            Some(root) => immich_source::present_names(&root),
            None => Default::default(),
        };
    }
```

Call it in both openers, next to `record_assets`. Then where `Outcome::Original` completes
successfully, insert that file's name:

```rust
                    // ⚠ One insert rather than a re-read: the download's own path is already in hand.
                    if let Some(name) = dest.file_name() {
                        self.immich_present.insert(name.to_os_string());
                    }
```

⚠ Read the `Outcome::Original` arm first — use whatever it actually calls the destination path.

- [ ] **Step 3: Build, then `cargo test --workspace`.** ⚠ `test_app.rs` breaks only under `cargo test`.

- [ ] **Step 4: Commit**

---

### Task 3: The rail's label and count

**Files:** `crates/app/src/collections_panel.rs`

- [ ] **Step 1: Use both**

In the folder loop, replace the `shown` computation with `folder_display_name`, and override the count
for the Immich root:

```rust
                                    let immich_root = app.immich_cache_root();
                                    for f in &folders {
                                        let shown = rustroom_app::immich_catalog::folder_display_name(
                                            &f.path,
                                            immich_root.as_deref(),
                                        );
                                        // ⚠⚠ A row exists for every asset **browsed**; the count beside
                                        // a folder is a statement about that folder's *contents*. It
                                        // read 981 against 17 files. ⚠ Only the Immich folder is
                                        // overridden — an unplugged drive keeps its own count, the same
                                        // line slice 3 drew.
                                        let count = if immich_root.as_deref() == Some(f.path.as_path()) {
                                            app.immich_present.len() as i64
                                        } else {
                                            f.count
                                        };
```

⚠ `immich_root` is computed **before** the loop, not per row. ⚠ Check `FolderRow::count`'s type and
match it rather than casting blindly.

- [ ] **Step 2: Build and gate.**

- [ ] **Step 3: Commit**

---

### Task 4: The indicator

**Files:** `crates/app/src/thumb.rs`, `crates/app/src/library.rs`

- [ ] **Step 1: Paint it**

In `thumb.rs`, add the flag to `cell_with` (and `cell` passes `false`), and after the badge block:

```rust
    // ⚠⚠ **Hand-painted, and that is forced rather than lazy.** `icons.rs` requires every codepoint to
    // be verified against the bundled Phosphor 2.1.2 TTF, because four features here have already
    // picked a glyph that turned out to be absent. That font's `post` table is **version 3.0 — no
    // glyph names** — so a codepoint can be verified as present but **not** as the right picture, and
    // shipping the wrong icon looks fine while being wrong. A circle and three strokes need no
    // verification. The histogram's clipping marks are painted for the same reason.
    if downloaded {
        let r = 6.0;
        // ⚠ Right of the badge when there is one, so neither moves the other's established position.
        let left = badge_end.unwrap_or(rect.left() + 3.0) + if badge_end.is_some() { 3.0 } else { 0.0 };
        let c = egui::pos2(left + r, rect.top() + 3.0 + r);
        let p = ui.painter_at(rect);
        p.circle_filled(c, r, egui::Color32::from_black_alpha(140));
        let ink = egui::Color32::WHITE;
        let stroke = egui::Stroke::new(1.4, ink);
        p.line_segment([c + egui::vec2(0.0, -3.0), c + egui::vec2(0.0, 2.6)], stroke);
        p.line_segment([c + egui::vec2(-2.6, 0.0), c + egui::vec2(0.0, 2.6)], stroke);
        p.line_segment([c + egui::vec2(2.6, 0.0), c + egui::vec2(0.0, 2.6)], stroke);
    }
```

⚠ `badge_end` is the badge rect's `right()` if a badge was drawn — capture it in the badge block rather
than recomputing. ⚠ A translucent black disc rather than an accent colour, so it reads on any
photograph; check it against a white sky in Task 5.

- [ ] **Step 2: Decide it per cell, in the lib**

⚠ The decision is testable and must not live in the draw call. In `immich_catalog.rs`:

```rust
/// Whether this cell should carry the downloaded indicator.
///
/// ⚠ **Immich sources only** — the user asked for it under the Immich header and nowhere else, and a
/// local photo is always "downloaded", so the mark would be noise on every cell.
pub fn shows_download_badge(
    is_immich_source: bool,
    path: &Path,
    present: &std::collections::HashSet<std::ffi::OsString>,
) -> bool {
    is_immich_source && path.file_name().is_some_and(|n| present.contains(n))
}
```

with tests: true for a present asset under an Immich source; false for an absent one; **false under a
folder source even when the file is present**.

- [ ] **Step 3: Pass it from the grid**

In `library.rs`'s `cell_with` call, compute `is_immich_source` from
`matches!(lib.source, LibrarySource::ImmichAlbum { .. } | LibrarySource::ImmichBucket { .. })` **once,
outside the cell loop**, and call `shows_download_badge` per cell.

- [ ] **Step 4: Gate**

```bash
cargo fmt --all
cargo fmt --all -- --check > /tmp/f.log 2>&1; echo "FMT EXIT: $?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

Expected: all **0**, **74 goldens unmoved**.

- [ ] **Step 5: Commit**

---

### Task 5: Look at it

```bash
cargo run --release -p app
```

1. Folders rail: **"immich (downloaded)"** with a count matching what the folder actually holds.
2. Click it — the same number of photos.
3. Open an Immich album: a circled ↓ on the downloaded ones only. ⚠ Check it against a **white sky**
   and a **dark** frame.
4. Open a photo (downloading it) — the indicator should appear on returning to the grid.
5. A **local folder** grid: no indicators anywhere.
6. ⚠ A **virtual copy** of a downloaded Immich photo: the copy badge and the indicator side by side,
   not overlapping.
7. Settings → **"Move to the next photo after rating"** — the toggle for the fourth item.

Then the journal and `CLAUDE.md` (at **406** lines; something moves out, not in).

## Self-review

| Spec requirement | Task |
|---|---|
| `present_names`, one `read_dir`, missing dir empty | 1 |
| `folder_display_name`, incl. `None` and a lookalike path | 1 |
| Cached; refreshed at startup, on open, per download | 2 |
| Rail label | 3 |
| Rail count = present, Immich folder only | 3 |
| Hand-painted indicator, top-left, beside the badge | 4 |
| Immich sources only, decided by a tested function | 4 |
| `cell` unchanged ⇒ filmstrip unchanged | 4 |
| auto-advance: no code change | — |
| No golden movement | 4 |
| Seen in use | 5 |

**Soft spots, flagged:**
- **The indicator's geometry is taste** — radius, alpha and offsets are literals in one block, to tune after looking.
- **`immich_present` is per configured server.** Changing the server in Settings should refresh it; ⚠ if it does not, that is a one-line call in the settings-apply path — check in Task 5.
- **`immich_source.rs` reaches ~841 lines.** Reported; the split remains on the open list.

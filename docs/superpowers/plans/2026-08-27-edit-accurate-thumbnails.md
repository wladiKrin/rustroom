# Edit-accurate thumbnails Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Library grid and Develop filmstrip show each photo's *edits*, by rendering the pipeline over the camera's embedded JPEG and keying the thumbnail cache by `(PhotoId, edits fingerprint)`.

**Architecture:** A new `edits_fingerprint(&Edits) -> u64` hashes the serialized edits, so it is exhaustive by construction. `ThumbCache` is re-keyed from `PathBuf` to a `ThumbKey { id, edits }`, which turns "the edits changed" into an ordinary cache miss with no invalidation code anywhere. `ThumbLoader`'s want-list carries `(PhotoId, Edits)` and its workers extract **and** render, so nothing lands on the UI thread. On an exact-key miss the cache falls back to any entry for the same `PhotoId`, so a re-render shows the slightly-stale picture rather than grey.

**Tech Stack:** Rust, `serde_json` (fingerprint), `pipeline`, `raw-decode`/`image-decode`, egui textures.

**Read first:** `docs/superpowers/specs/2026-08-27-edit-accurate-thumbnails-design.md` — above all its measurement table and the fidelity note.

**Before you start:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **No `Co-Authored-By` trailer. Never push. Never merge unprompted.** Branch: `edit-accurate-thumbnails`.

⚠⚠ **The Library is the area with this project's worst defect record** — sparse-versus-dense index confusion, the row-break bug, the greyed-out folder after `rebuild_entries`, the rename field dying under virtualisation. Two of the three properties `thumb_cache` documents must survive this change: **`rebuild_entries` still invalidates nothing**, and **indices still never leave the UI**. Task 2 has a test for each.

⚠ **Check clippy's exit code before every commit**, not just `cargo test`'s. Three commits went in this week with clippy red because a green test run was mistaken for evidence.

---

### Task 1: `edits_fingerprint`

**Files:**
- Create: `crates/app/src/thumb_key.rs`
- Modify: `crates/app/src/lib.rs` (declare the module)

⚠ A **lib** module, so it commits alone — a `pub` item in a lib is public API and does not trip `dead_code`. That is the same contrast `tool` (lib) and `HslChannel` (bin) illustrate.

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/thumb_key.rs`:

```rust
//! What a cached thumbnail is *of*: a photo, and the edits it was rendered with.

use crate::photo_id::PhotoId;
use edit_model::Edits;

/// A fingerprint of one copy's edits, for the thumbnail cache key.
///
/// ⚠⚠ Hashed from the **serialized** edits rather than field by field, and that is the point: it
/// is exhaustive by construction. `Edits` has around forty fields and cannot derive `Hash` (it
/// holds `f32`), so a hand-written hash would silently omit any field added later — and the
/// symptom is a *stale thumbnail*, which nothing would notice. Same reasoning as
/// `Edits::merge_from`'s exhaustive destructure.
pub fn edits_fingerprint(edits: &Edits) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_vec(edits) {
        Ok(bytes) => bytes.hash(&mut h),
        // ⚠ Impossible for `Edits` in practice. It must still vary per call rather than return a
        // constant: a constant would make every photo collide on one key and show each other's
        // thumbnails, which is far worse than a needless re-render.
        Err(_) => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            N.fetch_add(1, Ordering::Relaxed).hash(&mut h);
        }
    }
    h.finish()
}

/// What a cached thumbnail is of.
///
/// ⚠⚠ **The `edits` half is what makes this feature work with no invalidation code.** A changed
/// edit is simply a different key, so a sync, a paste, a preset click, an undo, a redo and a
/// snapshot restore are all correct without any of them knowing this cache exists. That is
/// `PrefixKey`'s pattern, which deliberately fails toward a miss.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ThumbKey {
    pub id: PhotoId,
    pub edits: u64,
}

impl ThumbKey {
    pub fn new(id: PhotoId, edits: &Edits) -> Self {
        Self {
            id,
            edits: edits_fingerprint(edits),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{EditGroups, EDIT_GROUP_NAMES};
    use std::path::PathBuf;

    fn id(p: &str) -> PhotoId {
        PhotoId::original(PathBuf::from(p))
    }

    #[test]
    fn the_same_edits_give_the_same_fingerprint() {
        let a = Edits::default();
        let b = Edits::default();
        assert_eq!(edits_fingerprint(&a), edits_fingerprint(&b));
    }

    /// ⚠⚠ **The exhaustiveness lives in the IMPLEMENTATION, not in this test, and it is worth
    /// being clear about that.** `edits_fingerprint` hashes the *serialization*, so a field added
    /// to `Edits` is included automatically — no test could enumerate forty fields and stay
    /// honest. What this test verifies is that the implementation really is the serialization-
    /// based one, by moving fields a hand-written hash would plausibly forget: a deep HSL band, a
    /// mask, and a nested geometry value.
    #[test]
    fn an_obscure_nested_field_still_changes_the_fingerprint() {
        let base = edits_fingerprint(&Edits::default());

        let mut deep_band = Edits::default();
        deep_band.hsl.bands[7].luminance = 12.0;
        assert_ne!(
            edits_fingerprint(&deep_band),
            base,
            "the eighth HSL band's luminance must count"
        );

        let mut cropped = Edits::default();
        cropped.geometry.crop.left = 0.1;
        assert_ne!(
            edits_fingerprint(&cropped),
            base,
            "a nested crop edge must count"
        );

        let mut angled = Edits::default();
        angled.geometry.angle = 0.25;
        assert_ne!(edits_fingerprint(&angled), base, "straighten must count");
    }

    /// ⚠ And the contrapositive, which is what makes the cache usable at all: identical edits
    /// must fingerprint identically, or every frame would be a miss and the grid would re-render
    /// forever.
    #[test]
    fn a_clone_fingerprints_identically() {
        let mut e = Edits::default();
        e.hsl.bands[3].hue = -8.0;
        e.exposure = 0.75;
        assert_eq!(edits_fingerprint(&e), edits_fingerprint(&e.clone()));
    }

    /// The whole feature: two copies of one file with different edits are different keys.
    #[test]
    fn two_copies_with_different_edits_get_different_keys() {
        let path = PathBuf::from("/a.RAF");
        let neutral = Edits::default();
        let bw = Edits {
            saturation: -100.0,
            ..Default::default()
        };
        let k0 = ThumbKey::new(PhotoId::new(path.clone(), 0), &neutral);
        let k1 = ThumbKey::new(PhotoId::new(path, 1), &bw);
        assert_ne!(k0, k1);
    }

    /// ⚠ …and two copies with the *same* edits still differ, or a copy would show its sibling's
    /// thumbnail. The `id` half of the key is load-bearing even though the edits half does the
    /// interesting work.
    #[test]
    fn two_copies_with_identical_edits_still_get_different_keys() {
        let path = PathBuf::from("/a.RAF");
        let e = Edits::default();
        assert_ne!(
            ThumbKey::new(PhotoId::new(path.clone(), 0), &e),
            ThumbKey::new(PhotoId::new(path, 1), &e)
        );
    }

    #[test]
    fn different_photos_with_the_same_edits_get_different_keys() {
        let e = Edits::default();
        assert_ne!(ThumbKey::new(id("/a.RAF"), &e), ThumbKey::new(id("/b.RAF"), &e));
    }
}
```

⚠ **`edit_model::all_nonneutral` is NOT usable here** — it is private to that crate's own `mod
tests`, and so is `pipeline::all_edits`. Do not try to import either. The exhaustiveness of this
feature comes from `serde`, not from a fixture; the tests above verify that the implementation is
the serde-based one.

⚠ Adjust the field paths (`hsl.bands[7].luminance`, `geometry.crop.left`, `geometry.angle`) to
whatever `Edits` actually declares — read `crates/edit-model/src/lib.rs` first.

- [ ] **Step 2: Declare the module**

In `crates/app/src/lib.rs`, add in alphabetical position:

```rust
pub mod thumb_key;
```

⚠ `serde_json` is already a dependency of `app`; no `Cargo.toml` change.

- [ ] **Step 3: Run to verify**

Run: `cargo test -p app --lib thumb_key 2>&1 | tail -10`
Expected: PASS, 6 tests.

- [ ] **Step 4: Sabotage**

Replace the body of `edits_fingerprint` with `0`.
Run: `cargo test -p app --lib thumb_key 2>&1 | grep -E "^test |test result:"`
Expected: **FAIL** on `an_obscure_nested_field_still_changes_the_fingerprint` and the three
"different keys" tests. ⚠ Confirm a `test result:` line appears — a sabotage that fails to compile reads
exactly like one that passes. Restore and re-run.

Then the subtler one, which is the whole point: make the fingerprint hash only `edits.exposure`.
Expected: **FAIL** on `an_obscure_nested_field_still_changes_the_fingerprint` — that is a
hand-written hash forgetting a field, which is the failure mode the serde-based implementation
exists to make impossible.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/thumb_key.rs crates/app/src/lib.rs
git commit -m "feat(thumbs): a fingerprint of a copy's edits, exhaustive by construction"
```

---

### Task 2: Re-key `ThumbCache`

**Files:**
- Modify: `crates/app/src/thumb_cache.rs` (the whole file: `PathBuf` → `ThumbKey`)

⚠⚠ **This inverts a decision that file documents deliberately** — path-keying so copies share one
texture, which is the bug being fixed. **Rewrite its module doc**, do not leave it contradicting
the code.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/app/src/thumb_cache.rs`. ⚠ Read the existing tests first and follow
their construction (the cache is generic; they instantiate `ThumbCache<String>`).

```rust
    fn key(p: &str, copy: u32, edits: u64) -> ThumbKey {
        ThumbKey {
            id: PhotoId::new(std::path::PathBuf::from(p), copy),
            edits,
        }
    }

    /// The whole feature: two copies of one file hold two textures.
    #[test]
    fn two_copies_of_one_file_no_longer_share_a_thumbnail() {
        let mut c = ThumbCache::<String>::with_default_budget();
        c.insert(key("/a.RAF", 0, 1), "colour".into(), 10);
        c.insert(key("/a.RAF", 1, 2), "b&w".into(), 10);
        assert_eq!(c.get(&key("/a.RAF", 0, 1)).map(String::as_str), Some("colour"));
        assert_eq!(c.get(&key("/a.RAF", 1, 2)).map(String::as_str), Some("b&w"));
    }

    /// ⚠⚠ **The pending state.** An exact-key miss falls back to any entry for the same photo, so
    /// a re-render shows the slightly-stale picture rather than flashing grey while a slider is
    /// dragged.
    #[test]
    fn a_changed_edit_falls_back_to_the_same_photos_previous_thumbnail() {
        let mut c = ThumbCache::<String>::with_default_budget();
        c.insert(key("/a.RAF", 0, 1), "old".into(), 10);
        assert!(c.get(&key("/a.RAF", 0, 2)).is_none(), "not an exact hit");
        assert_eq!(
            c.get_stale(&PhotoId::new(std::path::PathBuf::from("/a.RAF"), 0))
                .map(String::as_str),
            Some("old"),
            "but the previous render of *this* photo is available"
        );
    }

    /// ⚠ …and never falls back to a **different** photo's, which would show the wrong picture.
    #[test]
    fn the_stale_fallback_never_reaches_another_photo() {
        let mut c = ThumbCache::<String>::with_default_budget();
        c.insert(key("/a.RAF", 0, 1), "a".into(), 10);
        assert!(c
            .get_stale(&PhotoId::new(std::path::PathBuf::from("/b.RAF"), 0))
            .is_none());
        assert!(
            c.get_stale(&PhotoId::new(std::path::PathBuf::from("/a.RAF"), 1))
                .is_none(),
            "nor to another copy of the same file"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --lib thumb_cache 2>&1 | tail -20`
Expected: FAIL to compile — `ThumbKey` not in scope, `get_stale` not found.

- [ ] **Step 3: Re-key the cache**

In `crates/app/src/thumb_cache.rs`:

1. `use crate::photo_id::PhotoId;` and `use crate::thumb_key::ThumbKey;`.
2. Replace every `PathBuf`/`&Path` key with `ThumbKey`/`&ThumbKey` — in the `HashMap`, the LRU
   ordering, `insert`, `get`, `contains`, `retain_window` and the eviction loop.
3. Add the stale lookup:

```rust
    /// The most recent thumbnail of `id`, whatever edits it was rendered with.
    ///
    /// ⚠ For the **pending** state only: an exact-key miss means the edits changed and a render
    /// is in flight, and the previous picture of the same photo is a far better placeholder than
    /// grey. ⚠ Scoped to one `PhotoId` — never another copy, never another file — or a cell would
    /// show a different photograph.
    ///
    /// ⚠ Does **not** touch recency: what makes a thumbnail recent is being on screen, which
    /// `retain_window` already records. See `get`'s note.
    pub fn get_stale(&self, id: &PhotoId) -> Option<&V> {
        self.map
            .iter()
            .find(|(k, _)| &k.id == id)
            .map(|(_, v)| &v.value)
    }
```

⚠ Adapt the field names to whatever the entry struct actually uses — read the file.

4. **Rewrite the module doc.** It currently says copies share one texture. Replace with:

```rust
//! A cache of rendered thumbnails, keyed by photo **and edits**, byte-budgeted.
//!
//! ⚠⚠ **Keyed by [`ThumbKey`] — a `PhotoId` plus a fingerprint of that copy's `Edits`.** It was
//! keyed by `PathBuf`, deliberately, so copies of one file shared one texture; that was right
//! while a thumbnail was the camera's embedded JPEG and is exactly the bug once thumbnails show
//! edits, since a black-and-white copy appeared in colour.
//!
//! ⚠ Two of the three properties that fell out of path-keying survive, and both are load-bearing
//! in the Library, which is this project's worst area for defects: a re-sort or `rebuild_entries`
//! still invalidates nothing, and **indices never leave the UI**, which removes sparse-versus-
//! dense index confusion at the boundary rather than guarding against it downstream.
//!
//! ⚠ Making the edits part of the *key* is what means nothing has to invalidate: a sync, a paste,
//! a preset, an undo and a snapshot restore are all correct without knowing this cache exists.
//! `PrefixKey` in the render engine works the same way and for the same reason.
//!
//! ⚠ Generic over the value so it can live in this lib, which must not depend on egui. The bin
//! instantiates `ThumbCache<egui::TextureHandle>`.
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --lib thumb_cache 2>&1 | tail -5`
Expected: PASS. ⚠ The **pre-existing** tests must pass with their assertions unchanged apart from
the key type — that is the evidence the eviction, budget and pinning logic did not move.

- [ ] **Step 5: Two guards on the properties that must survive**

Add:

```rust
    /// ⚠ `rebuild_entries` must still invalidate nothing — it re-sorts and re-expands rows, and
    /// the cache is keyed by identity rather than by position. This states that as a property so
    /// a future move back to index keys breaks a test rather than the Library.
    #[test]
    fn reordering_the_entries_does_not_invalidate_anything() {
        let mut c = ThumbCache::<String>::with_default_budget();
        let a = key("/a.RAF", 0, 1);
        let b = key("/b.RAF", 0, 1);
        c.insert(a.clone(), "a".into(), 10);
        c.insert(b.clone(), "b".into(), 10);
        // The window in the opposite order — as a re-sort would produce.
        c.retain_window(&[b.clone(), a.clone()]);
        assert!(c.get(&a).is_some() && c.get(&b).is_some());
    }
```

- [ ] **Step 6: Sabotage**

Make `get_stale` drop its `k.id == id` filter (return the first entry whatever it is).
Run: `cargo test -p app --lib thumb_cache 2>&1 | grep -E "^test |test result:"`
Expected: **FAIL** on `the_stale_fallback_never_reaches_another_photo`. Confirm a `test result:`
line, then restore.

- [ ] **Step 7: Commit**

⚠ The workspace will not build yet — `main.rs` still passes paths. That is expected; **do not
commit a broken build.** Carry this task's changes into Task 3 and commit them together.

---

### Task 3: The loader carries edits and renders

**Files:**
- Modify: `crates/app/src/thumb_loader.rs`
- Modify: `crates/app/src/entries.rs` (`PhotoEntry` gains the edits)
- Modify: `crates/app/src/main.rs` (`sync_thumb_window`, `thumbnail_for`, `last_want`, the drain)
- Modify: `crates/app/src/library.rs`, `crates/app/src/filmstrip.rs` (the two window builders)
- Modify: `crates/app/src/test_app.rs` (exhaustive literals)

⚠⚠ **One commit with Task 2.** Re-keying the cache breaks every caller; the build is only green
again at the end of this task.

- [ ] **Step 1: `PhotoEntry` carries its edits**

In `crates/app/src/entries.rs`, add to `PhotoEntry`:

```rust
    /// This copy's stored edits, read from the sidecar when the row was built.
    ///
    /// ⚠ Carried so the thumbnail can be rendered with them **without re-reading the sidecar per
    /// frame**. `build` already loads the whole envelope — measured at 4.6 ms for 830 files — so
    /// this costs nothing new.
    ///
    /// ⚠ Stale for the **open** photo while it is being edited, because `persist_current_session`
    /// only writes on a photo switch or on quit. The caller substitutes `Session::edits` for that
    /// one photo — see `RustroomApp::thumb_window_key`.
    pub edits: edit_model::Edits,
```

Fill it in `build` from the same `CopyEntry` the name comes from. ⚠ For a row with no stored copy
(the original of an unedited photo), use `Edits::default()`.

⚠ `entries::build`'s tests construct `PhotoEntry` literally — the compiler will name them.

- [ ] **Step 2: The loader's request type**

In `crates/app/src/thumb_loader.rs`:

```rust
/// One thumbnail to produce: which photo, and the edits to render it with.
///
/// ⚠ Carries the `Edits` by value rather than a fingerprint: the worker has to *render* with
/// them, and re-reading the sidecar on the worker would be a second source of truth for what a
/// photo's edits are.
#[derive(Clone, PartialEq)]
pub struct ThumbRequest {
    pub id: PhotoId,
    pub edits: Edits,
}

/// One finished thumbnail (or `None` if that file had no usable preview).
pub struct ThumbResult {
    pub key: ThumbKey,
    pub thumb: Option<Thumbnail>,
}
```

Change `want(&self, paths: &[PathBuf])` to `want(&self, wanted: &[ThumbRequest])`, and the queue's
`wanted`/`in_flight` to hold `ThumbRequest`/`ThumbKey` respectively.

⚠ `in_flight` must be keyed by `ThumbKey`, not `PhotoId`: two renders of the same photo with
different edits are two different jobs, and de-duplicating on the photo alone would drop the newer
one.

- [ ] **Step 3: The loader renders**

Change the injected loader's type from `fn(&Path) -> Option<Thumbnail>` to a boxed closure, since
a `fn` pointer cannot capture:

```rust
    pub fn with_loader(
        load: Arc<dyn Fn(&ThumbRequest) -> Option<Thumbnail> + Send + Sync>,
        threads: usize,
        display_space: color::OutputSpace,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self
```

and the production one:

```rust
/// Extract a photo's embedded preview and render its edits over it.
///
/// ⚠⚠ **Rendered from the embedded JPEG, not from a decode.** Measured: 4.9 ms of render on top
/// of the ~90 ms extraction, against 934 ms for a half-size decode — 9.8×, essentially all of it
/// the decode. It matters most on *invalidation*: a changed edit costs another ~95 ms here and
/// would cost ~930 ms there, which cannot be cached away since a decoded buffer is 78 MB.
///
/// ⚠ **Accepted cost: the JPEG is 8-bit and display-referred, so highlight recovery is invisible
/// in the grid** — the headroom is gone before the pipeline sees it. Shadows compress the same
/// way. The thumbnail shows the camera's rendering plus your edits, not Rustroom's own render.
fn load_thumbnail(req: &ThumbRequest) -> Option<Thumbnail> {
    let path = &req.id.path;
    let thumb = if catalog::is_raw(path) {
        raw_decode::extract_thumbnail(path).ok()?
    } else {
        image_decode::thumbnail(path).ok()?
    };
    if req.edits == Edits::default() {
        // ⚠ Neutral edits are the common case on a first folder open; skip the round trip
        // entirely rather than paying it 830 times for a no-op.
        return Some(thumb);
    }
    Some(render_edits(thumb, &req.edits))
}

/// Run the pipeline over an already-extracted thumbnail.
///
/// ⚠ `RenderScale::new(1.0)`: the thumbnail *is* the image as far as the pipeline is concerned,
/// so spatial radii are relative to it. A scale derived from the original's dimensions would make
/// a 512 px thumbnail apply full-resolution blur radii.
fn render_edits(thumb: Thumbnail, edits: &Edits) -> Thumbnail {
    let mut buf = core_image::ImageBuf::new(thumb.width, thumb.height);
    {
        let d = buf.data_mut();
        for (i, px) in thumb.rgba.as_chunks::<4>().0.iter().enumerate() {
            for c in 0..3 {
                d[i * 3 + c] = color::srgb_to_linear(px[c] as f32 / 255.0);
            }
        }
    }
    let out = pipeline::build_pipeline(edits, pipeline::RenderScale::new(1.0)).run(buf);
    let rgba = out.to_rgba8();
    Thumbnail {
        width: out.width(),
        height: out.height(),
        rgba,
    }
}
```

⚠ **The dimensions come from `out`, not from `thumb`** — a crop changes them. Getting this wrong
paints a cropped image into the original's rect and skews it.

⚠ `Thumbnail` is `{ width: usize, height: usize, rgba: Vec<u8> }` and `ImageBuf::to_rgba8(&self)
-> Vec<u8>` — both verified, so the code above compiles as written.

⚠ **The worker builds the result's key from the request it was handed**, `ThumbKey::new(req.id.clone(),
&req.edits)` — never from anything it re-reads. A key derived from a second read of the sidecar
could disagree with the one the UI is looking up under, and the thumbnail would never be found.

- [ ] **Step 4: The app builds the window from keys**

In `crates/app/src/main.rs`:

```rust
    /// The edits a photo's thumbnail should be rendered with.
    ///
    /// ⚠⚠ **Two sources, deliberately.** For the **open** photo the edits live in `Session` and
    /// the sidecar is stale — `persist_current_session` writes only on a photo switch or on quit,
    /// which is exactly the period during which the filmstrip most needs to update. For every
    /// other photo the entry's copy, read from the sidecar when the row was built, is correct.
    fn thumb_edits(&self, e: &entries::PhotoEntry) -> Edits {
        match self.session.as_ref() {
            Some(s) if s.path == e.id.path && s.copy == e.id.copy => s.edits.clone(),
            _ => e.edits.clone(),
        }
    }
```

Change `sync_thumb_window(&mut self, window: &[PathBuf])` to take `&[ThumbRequest]`, comparing
against `last_want: Vec<ThumbRequest>`, and `contains`/`retain_window` against the requests' keys.

Change `thumbnail_for(&self, path: &Path)` to take a `&PhotoId` and use `get_stale`:

```rust
    /// The best thumbnail available for `id`, even if its edits have since changed.
    ///
    /// ⚠ `get_stale`, so the decode placeholder and the grid show the previous render rather than
    /// grey while a re-render is in flight.
    fn thumbnail_for(&self, id: &PhotoId) -> Option<egui::TextureHandle> {
        self.library.as_ref()?.cache.get_stale(id).cloned()
    }
```

⚠ Its caller at `main.rs:1556` passes `&id.path`; pass `&id`.

- [ ] **Step 5: The two window builders**

`crates/app/src/library.rs:514` and `crates/app/src/filmstrip.rs:145` build `window: Vec<PathBuf>`.
Both become `Vec<ThumbRequest>`, built from the entry and `thumb_edits`. ⚠ Both draw cells by
looking up the cache — those lookups become `ThumbKey::new(id, &edits)` with `get`, falling back to
`get_stale(&id)`.

⚠ Read both functions and keep the *virtualisation* untouched: `cells_in_row`, `push_id` per cell,
and the pinned window are all load-bearing and unrelated to this change.

- [ ] **Step 6: Build and fix the fallout**

Run: `cargo build --workspace --all-targets 2>&1 | tail -30`
Expected: a list of type errors at the call sites above plus the exhaustive literals in
`test_app.rs` and `entries.rs`'s tests. Work through them; do not silence any with `..Default`.

- [ ] **Step 7: Full green**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
cargo test -p pipeline --test golden 2>&1 | tail -2
```
Expected: `clippy exit: 0`, no failures, **49 goldens unmoved** — this touches no pipeline code.

- [ ] **Step 8: Commit Tasks 2 and 3 together**

```bash
git add -A
git commit -m "feat(thumbs): render each photo's edits into its thumbnail"
```

---

### Task 4: The loader's own tests

**Files:**
- Modify: `crates/app/src/thumb_loader.rs` (its `mod tests`)

⚠ The existing tests use the injected-loader seam and no image fixtures. Keep that.

- [ ] **Step 1: Adapt the existing tests, then add one**

The existing tests pass a `fn(&Path)`; they now pass an `Arc<dyn Fn(&ThumbRequest)>`. ⚠ Their
**assertions must not change** — that is the evidence the queue, the parking, the want-replacement
and the `Drop` wake-up did not move.

Add:

```rust
    /// ⚠ The edits must reach the render fn. Without this the loader would compile, run, and
    /// render every thumbnail neutral — the feature silently absent, with every other test green.
    #[test]
    fn the_requested_edits_reach_the_loader() {
        let seen = Arc::new(Mutex::new(Vec::<f32>::new()));
        let s = seen.clone();
        let loader = ThumbLoader::with_loader(
            Arc::new(move |req: &ThumbRequest| {
                s.lock().unwrap().push(req.edits.exposure);
                None
            }),
            1,
            color::OutputSpace::Srgb,
            || {},
        );
        loader.want(&[ThumbRequest {
            id: PhotoId::original(PathBuf::from("/a.RAF")),
            edits: Edits {
                exposure: 1.5,
                ..Default::default()
            },
        }]);
        let _ = collect(&loader, 1);
        assert_eq!(seen.lock().unwrap().as_slice(), &[1.5]);
    }

    /// ⚠ Two renders of one photo with different edits are two jobs. De-duplicating on the photo
    /// alone would drop the newer one and leave a stale thumbnail on screen for good.
    #[test]
    fn the_same_photo_with_different_edits_is_two_jobs() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let loader = ThumbLoader::with_loader(
            Arc::new(move |_: &ThumbRequest| {
                c.fetch_add(1, Ordering::Relaxed);
                None
            }),
            1,
            color::OutputSpace::Srgb,
            || {},
        );
        let id = PhotoId::original(PathBuf::from("/a.RAF"));
        loader.want(&[
            ThumbRequest { id: id.clone(), edits: Edits::default() },
            ThumbRequest {
                id,
                edits: Edits { exposure: 1.0, ..Default::default() },
            },
        ]);
        let _ = collect(&loader, 2);
        assert_eq!(n.load(Ordering::Relaxed), 2);
    }
```

⚠ `collect` is the existing helper; check its signature and the exact imports the module already
has rather than adding new ones.

- [ ] **Step 2: Run**

Run: `cargo test -p app --lib thumb_loader 2>&1 | tail -6`
Expected: PASS, existing tests included.

- [ ] **Step 3: Sabotage**

In `load_thumbnail`, return the extracted thumbnail unrendered (drop the `render_edits` call).
Run: `cargo test --all 2>&1 | grep -E "FAILED|test result: FAILED" | head`
Expected: ⚠ **predicted INERT** — no test renders real pixels, so nothing catches it. **Record
that honestly** rather than inventing a test that appears to. It is caught by Task 5's hand check,
which is the only thing that can see it.

Then sabotage `want` to de-duplicate on `id` instead of `ThumbKey`.
Expected: **FAIL** on `the_same_photo_with_different_edits_is_two_jobs`. Restore.

- [ ] **Step 4: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/thumb_loader.rs
git commit -m "test(thumbs): the loader carries edits through to the render"
```

---

### Task 5: Look at it

⚠⚠ **The acceptance test.** Appearance is what the harness cannot see, and the Task 4 sabotage
above is *inert* — a thumbnail rendered neutral would pass every test in the suite.

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

- [ ] **Step 2: The thing the feature exists for**

Open `~/Desktop/fuji`. Make a virtual copy of a photo, set its Saturation to −100, and go back to
the Library. **The copy must show in black and white beside its colour original.** That is the
whole feature and it is the one check that cannot be skipped.

- [ ] **Step 3: Live update**

In Develop with the filmstrip open, drag Exposure and let it settle. **That photo's filmstrip cell
must update** — this is the `Session`-versus-sidecar path from Task 3 Step 4, and if it does not
update, the sidecar is being read instead.

- [ ] **Step 4: No grey flicker**

Drag a slider repeatedly. The cell must show the **previous** render while the new one is in
flight, never grey. That is `get_stale`.

- [ ] **Step 5: The crop**

Crop a photo hard. Its thumbnail must change shape. ⚠ Check the grid does not reflow — `thumb::cell`
allocates its box *before* painting, which is the fix for the M2.2 reflow bug, and this is the
change most likely to expose it if that ever regressed.

- [ ] **Step 6: The honest limitation**

Blow out a highlight, then pull Highlights to −100. **The thumbnail will not recover it** — the
8-bit JPEG has no headroom. Confirm it looks acceptable rather than broken; if it does not, that is
a finding worth recording, not a bug to fix.

- [ ] **Step 7: Cost on a real folder**

Open a folder of several hundred and confirm it fills at roughly the speed it did before —
the render is 4.9 ms against ~90 ms of extraction, so it should be barely perceptible.

- [ ] **Step 8: Docs**

Add a section to `CLAUDE.md`'s "Current status" in the house style: the measurement table, the
⚠ findings, the test count, and the accepted caveats from the spec. ⚠ Remove **both** backlog
mentions — the standalone "Edit-accurate rendered thumbnails" entry *and* the clause inside the
"Library:" bullet, which names the same gap. Update the `▶ NEXT ACTION`.

```bash
git add -A && git commit -m "docs: edit-accurate thumbnails shipped"
```

⚠ **Do not push and do not merge.** Report the branch state.

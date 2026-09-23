# Persisting Immich asset lists — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A first visit to an Immich album or timeline month after relaunching is instant instead of costing a couple of seconds, without ever showing a stale list.

**Architecture:** A new lib module writes each fetched asset list to a JSON file beside the Immich cache, stamped with a validator taken from data the rail already fetched — `(assetCount, lastModifiedAssetTimestamp)` for an album, `count` for a bucket. A click loads the stored list only when that validator still matches; otherwise it fetches as before. Every failure is a miss, never an error.

**Tech Stack:** Rust, `serde`/`serde_json`, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-23-immich-list-cache-design.md`. Read it before Task 1.

---

## Conventions for every task

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By` trailer. Never push.** Branch is already `immich-list-cache`.
- ⚠⚠ **ONE cargo invocation mode.** Always `cargo test --workspace <name-filter>`; **never `-p <crate>`** — alternating them costs 43–531 s per switch, measured.
- ⚠⚠ **`cargo test`'s filter is a SUBSTRING, not a regex.** `'a|b|c'` matches nothing and **exits 0 having run zero tests** — that has already happened twice in this project. Use one filter, and check the passed count, not just the exit code.
- **Read each exit code from its own command**: write `; echo "EXIT: $?"` straight after the cargo command. Piping through `grep` reports grep's status and has produced a false green here.
- ⚠ A plan's code snippets go stale the moment `cargo fmt` reformats a block. If an edit does not match, read the file rather than forcing it.
- **egui only in bin modules.** `immich_list_cache` is a lib module: **no egui**.

## File structure

| File | Responsibility |
|---|---|
| `crates/immich/src/dto.rs` | `AlbumSummary::last_modified_asset_timestamp`; `Serialize` on `Asset` and `AssetType` |
| `crates/app/src/immich_list_cache.rs` | **New.** `Key`, `Validator`, `StoredList`, `lists_dir`, `load`, `store` |
| `crates/app/src/lib.rs` | `pub mod immich_list_cache;` |
| `crates/app/src/collections_panel.rs` | Build the validator at the click, try the cache before requesting |
| `crates/app/src/immich_flow.rs` | `store` the list when a reply arrives |

⚠ Nothing is added to `crates/app/src/immich_source.rs`: it is **821 lines**, already past the ~600 threshold.

---

### Task 1: `AlbumSummary` carries the validator field

**Files:**
- Modify: `crates/immich/src/dto.rs`

- [ ] **Step 1: Write the failing test**

Add to `dto.rs`'s `mod tests`:

```rust
    /// ⚠⚠ The album validator is `(assetCount, lastModifiedAssetTimestamp)`, **not** `updatedAt`:
    /// the captured reply shows `updatedAt` identical for both albums — a bulk touch — so it says
    /// nothing about an album's contents. This asserts the real captured values, which is the only
    /// way to know the field is there and means what it appears to.
    #[test]
    fn a_real_albums_reply_carries_the_per_album_change_signal() {
        let v: Vec<AlbumSummary> = serde_json::from_str(ALBUMS_REAL).expect("must parse");
        assert_eq!(
            v[0].last_modified_asset_timestamp.as_deref(),
            Some("2026-08-14T08:29:23.862Z")
        );
        assert_eq!(
            v[1].last_modified_asset_timestamp.as_deref(),
            Some("2026-06-14T12:11:35.623Z")
        );
        assert_ne!(
            v[0].last_modified_asset_timestamp, v[1].last_modified_asset_timestamp,
            "it must differ per album, which is the whole point of using it"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test --workspace per_album_change_signal; echo "EXIT: $?"
```

Expected: FAIL to compile — no field `last_modified_asset_timestamp`.

- [ ] **Step 3: Add the field**

In `pub struct AlbumSummary`, after `asset_count`:

```rust
    /// When an asset in this album last changed.
    ///
    /// ⚠⚠ **The album cache's validator, with `asset_count`.** Not `updated_at`: the captured reply
    /// has that identical for every album — a bulk touch — so it never signals a content change.
    /// This field differs per album and is explicitly about its assets.
    ///
    /// ⚠ `Option` because the schema does not mark it required. An album without it simply cannot be
    /// validated, so its list is refetched rather than trusted.
    pub last_modified_asset_timestamp: Option<String>,
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich; echo "EXIT: $?"
```

Expected: PASS. ⚠ Check the passed count is non-zero, not just the exit code.

- [ ] **Step 5: Commit**

```bash
git add crates/immich/src/dto.rs
git commit -m "AlbumSummary carries lastModifiedAssetTimestamp, the album cache's validator

Asserted against the captured reply rather than the schema: updatedAt is identical for
both albums there, evidently a bulk touch, so it signals nothing about contents. This
field differs per album and is about its assets."
```

---

### Task 2: `Asset` can be written as well as read

**Files:**
- Modify: `crates/immich/src/dto.rs`

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ A stored list is `Vec<Asset>` written back as JSON, so `Asset` has to serialise as well as
    /// deserialise — it derived only `Deserialize`.
    ///
    /// ⚠ `AssetType` carries `#[serde(other)]`, which is a **deserialise-only** attribute. This
    /// round-trips all three variants rather than assuming the enum survives a write: `Other` must
    /// come back as `Other`, not fail and not silently become `Image`.
    #[test]
    fn an_asset_round_trips_through_json_including_an_unknown_type() {
        let json = r#"[{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","originalMimeType":"image/x-fuji-raf","width":10,"height":20,
            "thumbhash":"t","fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"},
            {"id":"22222222-2222-4222-8222-222222222222","type":"VIDEO",
            "originalFileName":"b.MP4","width":null,"height":null,"thumbhash":null,
            "fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"},
            {"id":"33333333-3333-4333-8333-333333333333","type":"AUDIO",
            "originalFileName":"c.mp3","width":null,"height":null,"thumbhash":null,
            "fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"}]"#;
        let before: Vec<Asset> = serde_json::from_str(json).expect("must parse");
        let written = serde_json::to_string(&before).expect("must serialise");
        let after: Vec<Asset> = serde_json::from_str(&written).expect("must parse back");

        assert_eq!(after.len(), 3);
        assert!(matches!(after[0].asset_type, AssetType::Image));
        assert!(matches!(after[1].asset_type, AssetType::Video));
        assert!(
            matches!(after[2].asset_type, AssetType::Other),
            "an unknown type must survive a write as Other, not become Image"
        );
        assert_eq!(after[0].original_file_name, "a.RAF");
        assert_eq!(after[0].original_mime_type.as_deref(), Some("image/x-fuji-raf"));
        assert_eq!((after[0].width, after[0].height), (Some(10), Some(20)));
        assert!(after[1].thumbhash.is_none());
        assert_eq!(after[0].updated_at, "2025-01-02T00:00:00.000Z");
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test --workspace an_asset_round_trips; echo "EXIT: $?"
```

Expected: FAIL to compile — `Asset` does not implement `Serialize`.

- [ ] **Step 3: Add the derives**

Change `Asset`'s and `AssetType`'s derives, and add the import:

```rust
use serde::{Deserialize, Serialize};
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum AssetType {
```

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
```

⚠ Only these two. `AlbumSummary`, `Bucket`, `About`, `SearchResponse` and `SearchAssets` are never written, so leave them read-only — a type that can be written invites someone to write it.

⚠ If `serde` complains that `#[serde(other)]` cannot be combined with `Serialize`, do **not** delete the attribute: it is what stops one unknown asset type failing a whole page. Instead give `AssetType` a hand-written `Serialize` that emits `"IMAGE"`, `"VIDEO"` and `"Other"`. Try the derive first — it is expected to work.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich; echo "EXIT: $?"
```

Expected: PASS, with a non-zero count.

- [ ] **Step 5: Commit**

```bash
git add crates/immich/src/dto.rs
git commit -m "Asset can be written as well as read, so a list can be stored

Only Asset and AssetType gain Serialize; the other DTOs are never written and a type that
can be written invites someone to write it.

AssetType's #[serde(other)] is a deserialise-only attribute, so the test round-trips all
three variants rather than assuming the enum survives a write: an unknown type must come
back as Other, not silently as Image."
```

---

### Task 3: The list cache

**Files:**
- Create: `crates/app/src/immich_list_cache.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/immich_list_cache.rs` containing **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Its own temp dir per test, with a counter. `immich_source`'s older fixtures use the process
    /// id alone, which is exactly the collision `thumb_store::TempDir`'s doc says has already bitten
    /// this repo — one test deleting a directory another was reading.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_lists_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("{id}.RAF"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
        }
    }

    fn album_validator(count: u32, when: &str) -> Validator {
        Validator::Album {
            asset_count: count,
            last_modified: Some(when.to_string()),
        }
    }

    #[test]
    fn a_stored_list_comes_back_under_a_matching_validator() {
        let dir = temp_dir("round_trip");
        let key = Key::Album("abc".into());
        let v = album_validator(2, "2026-08-14T08:29:23.862Z");
        store(&dir, &key, &v, &[asset("one"), asset("two")]);
        let got = load(&dir, &key, &v).expect("a stored list must come back");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "one");
        assert_eq!(got[0].original_file_name, "one.RAF");
    }

    /// ⚠⚠ A photo added to the album changes its count. That must be a miss, or the grid would show
    /// a list that is missing the new photo for ever.
    #[test]
    fn a_changed_asset_count_is_a_miss() {
        let dir = temp_dir("count");
        let key = Key::Album("abc".into());
        store(
            &dir,
            &key,
            &album_validator(2, "2026-08-14T08:29:23.862Z"),
            &[asset("one"), asset("two")],
        );
        assert!(load(&dir, &key, &album_validator(3, "2026-08-14T08:29:23.862Z")).is_none());
    }

    /// ⚠⚠ An edit that keeps the count still moves `lastModifiedAssetTimestamp`. That is the whole
    /// reason the validator is a pair rather than just a count.
    #[test]
    fn a_changed_last_modified_is_a_miss_even_at_the_same_count() {
        let dir = temp_dir("modified");
        let key = Key::Album("abc".into());
        store(
            &dir,
            &key,
            &album_validator(2, "2026-08-14T08:29:23.862Z"),
            &[asset("one"), asset("two")],
        );
        assert!(load(&dir, &key, &album_validator(2, "2026-09-01T00:00:00.000Z")).is_none());
    }

    #[test]
    fn a_bucket_is_validated_on_its_count() {
        let dir = temp_dir("bucket");
        let key = Key::Bucket("2026-08-01".into());
        store(&dir, &key, &Validator::Bucket { count: 5 }, &[asset("one")]);
        assert!(load(&dir, &key, &Validator::Bucket { count: 5 }).is_some());
        assert!(load(&dir, &key, &Validator::Bucket { count: 6 }).is_none());
    }

    /// ⚠ An album and a bucket must never read each other's file, however their identifiers look.
    #[test]
    fn an_album_and_a_bucket_do_not_collide() {
        let dir = temp_dir("collide");
        let same = "2026-08-01";
        store(
            &dir,
            &Key::Album(same.into()),
            &album_validator(1, "x"),
            &[asset("album-one")],
        );
        store(
            &dir,
            &Key::Bucket(same.into()),
            &Validator::Bucket { count: 1 },
            &[asset("bucket-one")],
        );
        let a = load(&dir, &Key::Album(same.into()), &album_validator(1, "x")).unwrap();
        let b = load(&dir, &Key::Bucket(same.into()), &Validator::Bucket { count: 1 }).unwrap();
        assert_eq!(a[0].id, "album-one");
        assert_eq!(b[0].id, "bucket-one");
    }

    /// ⚠⚠ A key comes from the server. It must not be able to escape the directory.
    #[test]
    fn a_hostile_key_cannot_escape_the_directory() {
        let dir = temp_dir("hostile");
        let key = Key::Album("../../../etc/passwd".into());
        store(&dir, &key, &album_validator(1, "x"), &[asset("one")]);
        let escaped = dir.join("../../../etc/passwd");
        assert!(!escaped.exists(), "a file was written outside the cache");
        // And whatever it did write must live directly inside the directory.
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            assert_eq!(e.path().parent().unwrap(), dir.as_path());
        }
    }

    /// ⚠⚠ Every failure is a MISS, never an error: "a cache that can stop a folder opening is worse
    /// than no cache" — `thumb_store`'s rule.
    #[test]
    fn garbage_an_empty_file_and_an_absent_file_are_all_misses() {
        let dir = temp_dir("garbage");
        let key = Key::Album("abc".into());
        let v = album_validator(1, "x");
        assert!(load(&dir, &key, &v).is_none(), "absent");

        store(&dir, &key, &v, &[asset("one")]);
        let path = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .next()
            .unwrap();
        std::fs::write(&path, b"not json at all").unwrap();
        assert!(load(&dir, &key, &v).is_none(), "garbage");
        std::fs::write(&path, b"").unwrap();
        assert!(load(&dir, &key, &v).is_none(), "empty");
    }

    /// ⚠ A version bump invalidates cleanly rather than deserialising old fields into a new struct.
    #[test]
    fn a_stale_version_is_a_miss() {
        let dir = temp_dir("version");
        let key = Key::Album("abc".into());
        let v = album_validator(1, "x");
        store(&dir, &key, &v, &[asset("one")]);
        let path = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .next()
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("\"version\":1", "\"version\":99")).unwrap();
        assert!(load(&dir, &key, &v).is_none());
    }

    /// ⚠ An unwritable directory makes `store` a no-op, not a panic.
    #[test]
    fn storing_into_an_impossible_directory_does_nothing() {
        let dir = std::path::Path::new("/nonexistent-root-rustroom/lists");
        store(
            dir,
            &Key::Album("abc".into()),
            &album_validator(1, "x"),
            &[asset("one")],
        );
    }

    #[test]
    fn two_servers_get_different_list_directories() {
        assert_ne!(
            lists_dir("https://photos.example.com"),
            lists_dir("https://other.example.com")
        );
    }

    /// ⚠⚠ The lists directory must be a **sibling** of the originals, not inside them:
    /// `immich_source::is_immich_path` matches anything under that root and `asset_id_of` takes a
    /// path's stem, so a JSON in there could be mistaken for an asset.
    #[test]
    fn the_lists_directory_is_not_inside_the_originals_directory() {
        let url = "https://photos.example.com";
        let originals = crate::immich_source::cache_root(url);
        assert!(
            !lists_dir(url).starts_with(&originals),
            "a list file could be taken for a photo"
        );
    }
}
```

- [ ] **Step 2: Declare the module and run, to verify it fails**

Add to `crates/app/src/lib.rs`, in alphabetical position:

```rust
pub mod immich_list_cache;
```

```bash
cargo test --workspace immich_list_cache; echo "EXIT: $?"
```

Expected: FAIL to compile — `Key`, `Validator`, `store`, `load`, `lists_dir` do not exist.

- [ ] **Step 3: Write the module above the test module**

```rust
//! Remembering an Immich album's or month's asset list on disk.
//!
//! ⚠⚠ **Why this exists:** the server takes a couple of seconds over a list — ~900 KB of metadata for
//! an 872-asset album — so a first visit after every launch paid that again. Slice 2's in-memory map
//! made a *second* visit instant; this makes the first one instant too.
//!
//! ⚠⚠ **Validate first, never show stale.** A stored list is used only when the validator still
//! matches; otherwise it is refetched. A grid that rearranges under you — possibly dropping a
//! selection — is worse than a wait the status line already announces.
//!
//! ⚠ **Every failure is a miss, never an error.** `thumb_store`'s rule: a cache that can stop a folder
//! opening is worse than no cache.
//!
//! ⚠ **No egui.** A lib module, testable with no window and no server.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The on-disk format version.
///
/// ⚠ Bump whenever `immich::Asset`'s shape changes. A wrong version is a miss, so a bump invalidates
/// cleanly instead of deserialising old fields into a new struct.
const VERSION: u16 = 1;

/// Which list a file holds.
pub enum Key {
    Album(String),
    Bucket(String),
}

impl Key {
    /// This list's file name.
    ///
    /// ⚠⚠ The prefix is what stops an album whose id happens to look like a date from reading a
    /// bucket's file. ⚠ Sanitised because the identifier comes from the server and must not be able
    /// to escape the directory.
    fn file_name(&self) -> String {
        let (kind, id) = match self {
            Key::Album(id) => ("album", id),
            Key::Bucket(b) => ("bucket", b),
        };
        format!("{kind}-{}.json", crate::safe_name::sanitize(id))
    }
}

/// What makes a stored list still current.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum Validator {
    /// ⚠⚠ **Not `updatedAt`.** The captured `GET /albums` reply has that identical for every album —
    /// a bulk touch — so it never signals a content change. `assetCount` catches an addition or a
    /// removal; `lastModifiedAssetTimestamp` catches an edit that leaves the count alone.
    Album {
        asset_count: u32,
        last_modified: Option<String>,
    },
    /// ⚠ A count alone, because that is all `GET /timeline/buckets` gives. Swapping one asset for
    /// another leaves it unchanged, so that edit goes unnoticed until something else changes —
    /// accepted rather than paying an extra request per month. This is a cache, not a record.
    Bucket { count: u32 },
}

#[derive(Deserialize, Serialize)]
struct StoredList {
    version: u16,
    validator: Validator,
    assets: Vec<immich::Asset>,
}

/// Where this server's lists live.
///
/// ⚠⚠ A **sibling** of the originals directory, never inside it: `immich_source::is_immich_path`
/// matches everything under that root and `asset_id_of` takes a path's stem, so a JSON file in there
/// could be taken for a photo that has not been downloaded.
pub fn lists_dir(base_url: &str) -> PathBuf {
    let root = crate::immich_source::cache_root(base_url);
    let name = root
        .file_name()
        .map(|n| format!("{}-lists", n.to_string_lossy()))
        .unwrap_or_else(|| "lists".to_string());
    root.with_file_name(name)
}

/// The stored list for `key`, or `None` for any reason at all.
pub fn load(dir: &Path, key: &Key, want: &Validator) -> Option<Vec<immich::Asset>> {
    let bytes = std::fs::read(dir.join(key.file_name())).ok()?;
    let stored: StoredList = serde_json::from_slice(&bytes).ok()?;
    if stored.version != VERSION || &stored.validator != want {
        return None;
    }
    Some(stored.assets)
}

/// Remember `assets` for `key`, best-effort.
///
/// ⚠ Silent on failure by design — the caller already has the list, and a full disk must not become
/// an error in the grid.
pub fn store(dir: &Path, key: &Key, validator: &Validator, assets: &[immich::Asset]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let stored = StoredList {
        version: VERSION,
        validator: validator.clone(),
        assets: assets.to_vec(),
    };
    let Ok(bytes) = serde_json::to_vec(&stored) else {
        return;
    };
    // ⚠⚠ Write then rename, never write in place: a crash or a full disk mid-write would otherwise
    // leave a truncated file that parses to garbage rather than failing cleanly. `thumb_store`'s rule.
    let tmp = dir.join(format!("{}.tmp", key.file_name()));
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, dir.join(key.file_name())).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich_list_cache; echo "EXIT: $?"
```

Expected: PASS, 11 tests. ⚠ Check the count — a filter that matches nothing also exits 0.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_list_cache.rs crates/app/src/lib.rs
git commit -m "An on-disk cache for Immich asset lists, validated before use

Validate first: a stored list is used only when its validator still matches, so a stale
list is never shown. Every failure is a miss rather than an error, following thumb_store's
rule that a cache which can stop a folder opening is worse than no cache, and writes go
through a temp file and a rename so a crash cannot leave a truncated list.

The lists live in a sibling directory of the originals, not inside it: is_immich_path
matches everything under that root and asset_id_of takes a path's stem, so a JSON in there
could be taken for a photo. Keys are prefixed by kind so an album whose id looks like a
date cannot read a bucket's file, and sanitised because they come from the server."
```

---

### Task 4: Use it at the click, fill it on arrival

**Files:**
- Modify: `crates/app/src/collections_panel.rs`, `crates/app/src/immich_flow.rs`

- [ ] **Step 1: Build the validator and try the cache — albums**

In `collections_panel.rs`, replace the `Action::Open` arm:

```rust
        crate::immich_panel::Action::Open { id, name } => {
            // ⚠ Already fetched this session: open it now rather than waiting a couple of seconds
            // for the same reply again.
            if let Some(assets) = app.immich_assets.get(&id).cloned() {
                app.open_immich_album(ctx, id, name, &assets);
            } else if let Some(assets) = app.immich_cached_album(&id) {
                // ⚠ On disk from a previous run, and its validator still matches.
                app.immich_assets.insert(id.clone(), assets.clone());
                app.open_immich_album(ctx, id, name, &assets);
            } else {
                app.immich_opening = Some(id.clone());
                app.status = format!("Loading “{name}” from Immich…");
                app.immich
                    .request(rustroom_app::immich_worker::Job::Album { id, name });
            }
        }
```

- [ ] **Step 2: The same for buckets**

```rust
        crate::immich_panel::Action::OpenBucket { bucket, label } => {
            if let Some(assets) = app.immich_assets.get(&bucket).cloned() {
                app.open_immich_bucket(ctx, bucket, label, &assets);
            } else if let Some(assets) = app.immich_cached_bucket(&bucket) {
                app.immich_assets.insert(bucket.clone(), assets.clone());
                app.open_immich_bucket(ctx, bucket, label, &assets);
            } else {
                app.immich_opening = Some(bucket.clone());
                app.status = format!("Loading {label} from Immich…");
                app.immich
                    .request(rustroom_app::immich_worker::Job::Bucket { bucket, label });
            }
        }
```

- [ ] **Step 3: The two helpers, and storing on arrival**

In `immich_flow.rs`, inside `impl RustroomApp`:

```rust
    /// The validator for an album, from the rail's own already-fetched list.
    ///
    /// ⚠ `None` when the rail has no list, or does not know this album: there is then nothing to
    /// validate against, and an unvalidated load could show a stale grid.
    fn album_validator(&self, id: &str) -> Option<list_cache::Validator> {
        let albums = self.immich_albums.as_ref()?.as_ref().ok()?;
        let a = albums.iter().find(|a| a.id == id)?;
        Some(list_cache::Validator::Album {
            asset_count: a.asset_count,
            last_modified: a.last_modified_asset_timestamp.clone(),
        })
    }

    /// The validator for one month, from the rail's own bucket list.
    fn bucket_validator(&self, bucket: &str) -> Option<list_cache::Validator> {
        let buckets = self.immich_buckets.as_ref()?.as_ref().ok()?;
        let b = buckets.iter().find(|b| b.time_bucket == bucket)?;
        Some(list_cache::Validator::Bucket { count: b.count })
    }

    /// The directory this server's lists live in, if one is configured.
    fn immich_lists_dir(&self) -> Option<std::path::PathBuf> {
        let conn = immich_source::connection(&self.settings)?;
        Some(list_cache::lists_dir(&conn.url))
    }

    /// An album's stored list, if it is still current.
    pub(crate) fn immich_cached_album(&self, id: &str) -> Option<Vec<immich::Asset>> {
        let dir = self.immich_lists_dir()?;
        let want = self.album_validator(id)?;
        list_cache::load(&dir, &list_cache::Key::Album(id.to_string()), &want)
    }

    /// One month's stored list, if it is still current.
    pub(crate) fn immich_cached_bucket(&self, bucket: &str) -> Option<Vec<immich::Asset>> {
        let dir = self.immich_lists_dir()?;
        let want = self.bucket_validator(bucket)?;
        list_cache::load(&dir, &list_cache::Key::Bucket(bucket.to_string()), &want)
    }

    /// Remember a freshly fetched list on disk, so the next launch does not refetch it.
    ///
    /// ⚠ Stamped with the validator **as the rail currently sees it**. If the rail's list is stale the
    /// stamp is stale too, and the next launch refetches — a wasted fetch, never a wrong grid.
    fn remember_immich_list(&self, key: list_cache::Key, want: Option<list_cache::Validator>, assets: &[immich::Asset]) {
        let (Some(dir), Some(v)) = (self.immich_lists_dir(), want) else {
            return;
        };
        list_cache::store(&dir, &key, &v, assets);
    }
```

Add the import at the top of `immich_flow.rs`:

```rust
use rustroom_app::immich_list_cache as list_cache;
```

Then in `pump_immich`, beside each existing in-memory insert:

```rust
                            self.immich_assets.insert(id.clone(), assets.clone());
                            let v = self.album_validator(&id);
                            self.remember_immich_list(list_cache::Key::Album(id.clone()), v, &assets);
```

```rust
                            self.immich_assets.insert(bucket.clone(), assets.clone());
                            let v = self.bucket_validator(&bucket);
                            self.remember_immich_list(
                                list_cache::Key::Bucket(bucket.clone()),
                                v,
                                &assets,
                            );
```

- [ ] **Step 4: Build and run the whole suite**

```bash
cargo build -p app; echo "BUILD EXIT: $?"
cargo test --workspace; echo "TEST EXIT: $?"
```

Expected: both 0. ⚠ `test_app.rs` constructs `RustroomApp` exhaustively — a compile error there is visible only to `cargo test`, never `cargo build`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src
git commit -m "A click uses the stored list when its validator still matches

Three steps at a click: the session's in-memory map, then the stored list if its validator
matches the rail's already-fetched album or bucket list, then a fetch. The validator is
built where the click is applied, from data already in hand, so validation costs no request.

No validator means no hit — an unvalidated load could show a stale grid, so it falls
through to a fetch instead. A list is stamped with the validator as the rail currently sees
it: if that is stale the stamp is stale too and the next launch refetches, which is a wasted
fetch rather than a wrong grid."
```

---

### Task 5: The gate

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

Expected: all three **0**, and the **74 golden tests unmoved** — this adds no pipeline stage.

- [ ] **Step 4: Commit any formatting churn**

```bash
git add -A && git commit -m "fmt"
```

---

### Task 6: Run it

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the path**

1. Open an album. It takes the usual couple of seconds and says so.
2. Switch away and back — instant, from memory.
3. **Quit, relaunch, expand Immich, click the same album.** It should fill **without** the wait. This is the whole feature.
4. Add or remove a photo in that album in Immich, then click it again: it refetches, because `assetCount` moved.
5. The same for a Timeline month.

⚠ Step 3 needs the rail's album list first, which arrives when the section is expanded — one small call. A click *before* that has no validator and correctly falls through to a fetch.

- [ ] **Step 3: Report what use found**

⚠⚠ Every Immich defect so far was found by use, not by a test. Write what happened into
`docs/JOURNAL.md` as a new dated section at the top and update `CLAUDE.md`'s live state **in place**.
⚠ `CLAUDE.md` is at **399 lines** against a ~400 target: something must move to the journal rather than
being appended here.

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| `AlbumSummary::last_modified_asset_timestamp`, asserted against the capture | 1 |
| `Asset` serialisable; `AssetType`'s `#[serde(other)]` round-trip proven | 2 |
| New lib module, no egui, nothing added to `immich_source.rs` | 3 |
| Sibling lists directory, per-host, proven not inside the originals | 3 |
| `album-`/`bucket-` prefixes; sanitised keys; no collision | 3 |
| Versioned payload; a bumped version is a miss | 3 |
| Validator is `(assetCount, lastModified)` / `count`, never `updatedAt` | 1, 3 |
| Every failure a miss; write-then-rename; unwritable dir a no-op | 3 |
| Validate first — a stale list is never shown | 3, 4 |
| Validator built at the click from data already fetched; absent list ⇒ fetch | 4 |
| Store on arrival | 4 |
| No budget (arithmetic in the spec; nothing to implement) | — |
| No golden movement | 5 |
| Seen in use | 6 |

**Known soft spots, flagged rather than hidden:**

- **`AssetType` + `Serialize`.** `#[serde(other)]` is deserialise-only; the derive is expected to work but Task 2 says what to do if it does not, and the round-trip test is what proves it either way.
- **`safe_name::sanitize` maps many characters to `_`**, so it is many-to-one: two absurd album ids could in principle share a file. Album ids are UUIDs and bucket identifiers are dates, so neither can collide in practice — and a collision would be a validator mismatch and therefore a miss, not wrong data.
- **The bucket validator is a count**, so an asset swapped for another inside one month goes unnoticed. Stated in the spec as an accepted trade; do not "fix" it with a per-month request without measuring what that costs.

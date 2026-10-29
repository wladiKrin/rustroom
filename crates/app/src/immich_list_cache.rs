//! Remembering an Immich album's or month's asset list on disk.
//!
//! ⚠⚠ **Why this exists.** The server takes a couple of seconds over a list — roughly 900 KB of
//! metadata for an 872-asset album — and that was paid again on the first visit after every launch.
//! Slice 2's in-memory map made a *second* visit instant; this makes the first one instant too.
//!
//! ⚠⚠ **Validate first; never show a stale list.** A stored list is used only when its validator
//! still matches what the rail has already fetched, and otherwise refetched. A grid that rearranges
//! underneath you — possibly dropping a selection — is worse than a wait the status line announces.
//!
//! ⚠ **Every failure is a miss, never an error.** `thumb_store`'s rule: a cache that can stop a
//! folder opening is worse than no cache at all.
//!
//! ⚠ **No egui here.** A lib module, testable with no window and no server.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The on-disk format version.
///
/// ⚠ Bump this whenever `immich::Asset`'s shape changes. A mismatched version is a miss, so a bump
/// invalidates cleanly instead of deserialising old fields into a new struct.
///
/// 2 (M10.15c): `Asset` gained `is_favorite` and `exif_info` — every list is fetched once more.
const VERSION: u16 = 2;

/// Which list a file holds.
pub enum Key {
    Album(String),
    Bucket(String),
}

impl Key {
    /// This list's file name.
    ///
    /// ⚠⚠ The `album-`/`bucket-` prefix is what stops an album whose id happened to look like a date
    /// from reading a bucket's file. ⚠ Sanitised because the identifier comes from the server and
    /// must not be able to escape the directory.
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
    /// evidently a bulk touch — so it never signals a content change. `asset_count` catches an
    /// addition or a removal; `last_modified` catches an edit that leaves the count alone.
    Album {
        asset_count: u32,
        last_modified: Option<String>,
    },
    /// ⚠ A count alone, because that is all `GET /timeline/buckets` gives. One asset swapped for
    /// another inside a month leaves it unchanged, so that edit goes unnoticed until something else
    /// changes — accepted rather than paying an extra request per month. This is a cache, not a
    /// record.
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
/// could be taken for a photo that has not been downloaded yet.
pub fn lists_dir(base_url: &str) -> PathBuf {
    let root = crate::immich_source::cache_root(base_url);
    let name = root
        .file_name()
        .map(|n| format!("{}-lists", n.to_string_lossy()))
        .unwrap_or_else(|| "lists".to_string());
    root.with_file_name(name)
}

/// The stored list for `key`, or `None` for any reason whatsoever.
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
/// ⚠ Silent on failure by design: the caller already has the list in hand, and a full disk must not
/// turn into an error in the grid.
/// Drop one stored list (M10.15d: a send changed it), so its next opening fetches the new state.
/// ⚠ A list that is not there is not an error.
pub fn forget(dir: &Path, key: &Key) {
    let _ = std::fs::remove_file(dir.join(key.file_name()));
}

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
    // ⚠⚠ Write then rename, never write in place: a crash or a full disk midway would otherwise
    // leave a truncated file that parses as garbage instead of failing cleanly. `thumb_store`'s rule.
    let name = key.file_name();
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, dir.join(&name)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory of its own — `test_dir` makes it unique per test and per run.
    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("lists-{tag}"))
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
            is_favorite: false,
            exif_info: None,
            people: None,
        }
    }

    fn album_validator(count: u32, when: &str) -> Validator {
        Validator::Album {
            asset_count: count,
            last_modified: Some(when.to_string()),
        }
    }

    /// The one file `store` wrote, so a test can corrupt it without knowing the naming scheme.
    fn only_file(dir: &Path) -> PathBuf {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(v.len(), 1, "expected exactly one stored list in {dir:?}");
        v.pop().unwrap()
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

    /// ⚠⚠ A photo added to the album changes its count. That has to be a miss, or the grid would go
    /// on showing a list without the new photo for ever.
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
    /// reason the album validator is a pair rather than just a count.
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

    /// ⚠ An album that sends no `lastModifiedAssetTimestamp` must not match one that does, or a
    /// server upgrade dropping the field would silently accept every stale list.
    #[test]
    fn an_absent_last_modified_does_not_match_a_present_one() {
        let dir = temp_dir("absent");
        let key = Key::Album("abc".into());
        store(&dir, &key, &album_validator(2, "x"), &[asset("one")]);
        let none = Validator::Album {
            asset_count: 2,
            last_modified: None,
        };
        assert!(load(&dir, &key, &none).is_none());
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
        let b = load(
            &dir,
            &Key::Bucket(same.into()),
            &Validator::Bucket { count: 1 },
        )
        .unwrap();
        assert_eq!(a[0].id, "album-one");
        assert_eq!(b[0].id, "bucket-one");
    }

    /// ⚠⚠ A key comes from the server. It must not be able to escape the directory.
    ///
    /// ⚠⚠ **ASSERT ON THE PATH THIS CODE DERIVES, NEVER ON WHETHER SOME ABSOLUTE PATH EXISTS.**
    /// This test used to read `!dir.join("../../../etc/passwd").exists()` — and `Path::join` is
    /// *lexical*, it resolves nothing. Under a macOS temp dir (`/var/folders/…/T`) that pointed at a
    /// nonexistent `/var/folders/mt/etc/passwd` and passed; on the Linux runner (`/tmp`) it resolved
    /// to **`/etc/passwd`, which exists on every Linux box**, and failed. So the assertion never
    /// examined this module at all — it reported whether an unrelated system file was present, and
    /// it was green on the author's machine for exactly that reason. CI caught it on 2026-09-24.
    /// `immich_source`'s `a_hostile_original_name_cannot_escape_the_cache_directory` is the shape to
    /// copy: it checks the *derived* path and never touches disk.
    #[test]
    fn a_hostile_key_cannot_escape_the_directory() {
        let dir = temp_dir("hostile");
        let key = Key::Album("../../../etc/passwd".into());

        // The derived name is the part under this module's control: no separator, no dot-segment.
        let name = key.file_name();
        assert!(
            !name.contains('/') && !name.contains('\\') && !name.contains(".."),
            "the file name can still escape: {name:?}"
        );

        store(&dir, &key, &album_validator(1, "x"), &[asset("one")]);

        // And what it wrote is that one file, sitting directly inside the directory — no subtree.
        let written: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(written.len(), 1, "expected one file, got {written:?}");
        assert_eq!(written[0].parent().unwrap(), dir.as_path());
        assert_eq!(written[0].file_name().unwrap(), std::ffi::OsStr::new(&name));

        // It must still be readable through the same key, so an odd id is cached rather than lost.
        assert!(load(&dir, &key, &album_validator(1, "x")).is_some());
    }

    /// ⚠⚠ Every failure is a MISS, never an error — "a cache that can stop a folder opening is worse
    /// than no cache", `thumb_store`'s rule.
    #[test]
    fn garbage_an_empty_file_and_an_absent_file_are_all_misses() {
        let dir = temp_dir("garbage");
        let key = Key::Album("abc".into());
        let v = album_validator(1, "x");
        assert!(load(&dir, &key, &v).is_none(), "absent");

        store(&dir, &key, &v, &[asset("one")]);
        let path = only_file(&dir);
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
        let path = only_file(&dir);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"version\":2"),
            "the payload must carry its version: {text:.80}"
        );
        std::fs::write(&path, text.replace("\"version\":2", "\"version\":99")).unwrap();
        assert!(load(&dir, &key, &v).is_none());
    }

    /// M10.15d: a send drops the list it changed, so its next opening fetches the new state.
    #[test]
    fn a_forgotten_list_is_a_miss_and_another_is_kept() {
        let dir = temp_dir("forget");
        let (k1, k2) = (Key::Album("one".into()), Key::Album("two".into()));
        let v = album_validator(1, "x");
        store(&dir, &k1, &v, &[asset("a")]);
        store(&dir, &k2, &v, &[asset("b")]);
        forget(&dir, &k1);
        assert!(load(&dir, &k1, &v).is_none());
        assert!(load(&dir, &k2, &v).is_some());
        forget(&dir, &k1); // a second forget is not an error
    }

    /// M10.15c: lists cached before the favourite and the rating were read are fetched again.
    #[test]
    fn a_version_one_list_is_a_miss() {
        let dir = temp_dir("version-one");
        let key = Key::Album("abc".into());
        let v = album_validator(1, "x");
        store(&dir, &key, &v, &[asset("one")]);
        let path = only_file(&dir);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            load(&dir, &key, &v).is_some(),
            "fixture: the current version loads"
        );
        std::fs::write(&path, text.replace("\"version\":2", "\"version\":1")).unwrap();
        assert!(
            load(&dir, &key, &v).is_none(),
            "a pre-M10.15c list was used"
        );
    }

    /// ⚠ Storing twice replaces the file rather than accumulating, which is why no budget is needed.
    #[test]
    fn storing_again_replaces_rather_than_accumulates() {
        let dir = temp_dir("replace");
        let key = Key::Album("abc".into());
        store(&dir, &key, &album_validator(1, "x"), &[asset("one")]);
        store(&dir, &key, &album_validator(2, "y"), &[asset("two")]);
        only_file(&dir); // asserts there is exactly one, and no leftover .tmp
        let got = load(&dir, &key, &album_validator(2, "y")).unwrap();
        assert_eq!(got[0].id, "two");
    }

    /// ⚠ An unwritable directory makes `store` a no-op, not a panic.
    #[test]
    fn storing_into_an_impossible_directory_does_nothing() {
        let dir = Path::new("/nonexistent-root-rustroom/lists");
        store(
            dir,
            &Key::Album("abc".into()),
            &album_validator(1, "x"),
            &[asset("one")],
        );
        assert!(load(dir, &Key::Album("abc".into()), &album_validator(1, "x")).is_none());
    }

    #[test]
    fn two_servers_get_different_list_directories() {
        assert_ne!(
            lists_dir("https://photos.example.com"),
            lists_dir("https://other.example.com")
        );
    }

    /// ⚠⚠ The lists directory has to be a **sibling** of the originals, not inside it:
    /// `immich_source::is_immich_path` matches anything under that root and `asset_id_of` takes a
    /// path's stem, so a list file in there could be mistaken for a photo.
    #[test]
    fn the_lists_directory_is_not_inside_the_originals_directory() {
        let url = "https://photos.example.com";
        let originals = crate::immich_source::cache_root(url);
        let lists = lists_dir(url);
        assert!(
            !lists.starts_with(&originals),
            "a list file could be taken for a photo: {lists:?} inside {originals:?}"
        );
        assert_eq!(
            lists.parent(),
            originals.parent(),
            "and it should still sit beside it, under the same per-server root"
        );
    }
}

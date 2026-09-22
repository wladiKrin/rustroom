//! Turning Immich assets into the rows the ordinary Library is built from.
//!
//! ⚠⚠ **The idea the whole Immich slice rests on**: the cache path is derived deterministically
//! from the asset UUID, so it is known **before the file exists**. That is what lets a
//! `CatalogEntry` — and therefore a `PhotoId`, a `PhotoEntry` and a whole `Library` — be built
//! for an asset that has never been downloaded, with no new grid, no new selection model and no
//! new filmstrip. Opening the photo makes the path real and every existing code path then
//! applies with no special case.
//!
//! ⚠ **No egui here.** This is the unit-testable half; the rail section lives in
//! `immich_panel.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A filesystem-safe directory name for one server.
///
/// ⚠ Two servers must not share a cache directory: the same UUID on two servers is two
/// different photos. ⚠ Never empty — an empty slug would put every server's cache in one place
/// and collide them all.
pub fn host_slug(base_url: &str) -> String {
    let s = crate::safe_name::sanitize(base_url.trim());
    let s = s.trim_matches('_').trim().to_string();
    if s.is_empty() {
        "server".to_string()
    } else {
        s
    }
}

/// Where this server's originals are cached.
pub fn cache_root(base_url: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustroom")
        .join("immich")
        .join(host_slug(base_url))
}

/// The one path an asset's original always occupies.
///
/// ⚠⚠ Deterministic, and that is load-bearing: re-downloading an evicted or deleted file must
/// restore **byte-identical identity**, or anything that recorded the old path dangles.
///
/// ⚠ The UUID **is** the stem, so two cameras' `DSC_0001.NEF` cannot collide and the name a
/// hostile server sent never reaches the filesystem. The extension is preserved because it is
/// what picks the decoder, and it is sanitised for the same reason.
pub fn cache_path(root: &Path, asset: &immich::Asset) -> PathBuf {
    let ext = Path::new(&asset.original_file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = crate::safe_name::sanitize(&asset.id);
    let name = if ext.is_empty() {
        stem
    } else {
        format!("{}.{}", stem, crate::safe_name::sanitize(&ext))
    };
    root.join(name)
}

/// Has this asset's original been downloaded yet?
pub fn is_cached(path: &Path) -> bool {
    path.is_file()
}

/// The album's assets as the rows `RustroomApp::open_scan` consumes.
///
/// ⚠⚠ Returns `catalog::CatalogEntry`, **not** `PhotoEntry`: `open_scan` calls
/// `entries::build(&scan, sidecar::load_all)` itself, so producing `CatalogEntry` reuses the
/// whole loader — sidecars, virtual copies, the filter, the `ThumbLoader` — rather than half of
/// it.
///
/// ⚠ Skips videos and anything `catalog::is_supported` rejects. Its `RAW_EXTENSIONS` is narrower
/// than Immich's (no `pef`, `srw`, `3fr`, `iiq`), so some RAWs on the server are genuinely not
/// editable here. Skipped assets are **absent**, not greyed out.
pub fn entries_from_album(assets: &[immich::Asset], root: &Path) -> Vec<catalog::CatalogEntry> {
    assets
        .iter()
        .filter(|a| a.asset_type == immich::AssetType::Image)
        .filter_map(|a| {
            let path = cache_path(root, a);
            if !catalog::is_supported(&path) {
                return None;
            }
            let edited = crate::sidecar::edited_at(&path);
            Some(catalog::CatalogEntry {
                path,
                file_name: a.original_file_name.clone(),
                modified: parse_rfc3339(&a.file_modified_at),
                edited,
            })
        })
        .collect()
}

/// Parse `2025-08-14T09:21:33.000Z` into a `SystemTime`.
///
/// ⚠ Hand-rolled rather than adding `chrono` or `time` to this workspace for one field.
/// ⚠ An unparseable value yields `UNIX_EPOCH` rather than dropping the photo — the same rule
/// `CatalogEntry::modified` already documents for a file whose metadata cannot be read, and it
/// sorts to the oldest end instead of failing the album.
fn parse_rfc3339(s: &str) -> SystemTime {
    fn num(s: &str) -> Option<i64> {
        s.parse().ok()
    }
    if s.len() < 19 || !s.is_char_boundary(19) {
        return UNIX_EPOCH;
    }
    let (y, mo, d) = (num(&s[0..4]), num(&s[5..7]), num(&s[8..10]));
    let (h, mi, sec) = (num(&s[11..13]), num(&s[14..16]), num(&s[17..19]));
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (y, mo, d, h, mi, sec) else {
        return UNIX_EPOCH;
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3_600 + mi * 60 + sec;
    if secs < 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::from_secs(secs as u64)
}

/// Days since 1970-01-01 for a proleptic Gregorian date. Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, name: &str, kind: immich::AssetType) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: kind,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: 100,
            height: 100,
            thumbhash: "x".into(),
            file_modified_at: "2025-08-14T09:21:33.000Z".into(),
        }
    }

    /// ⚠⚠ **The whole design rests on this.** The path must be knowable before the file exists
    /// and identical every time, or a re-download would produce a second identity and anything
    /// that recorded the first would dangle.
    #[test]
    fn a_cache_path_is_the_same_every_time_for_the_same_asset() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset(
            "11111111-1111-4111-8111-111111111111",
            "DSCF1234.RAF",
            immich::AssetType::Image,
        );
        assert_eq!(cache_path(root, &a), cache_path(root, &a));
    }

    #[test]
    fn a_cache_path_keeps_the_uuid_and_the_extension() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset(
            "11111111-1111-4111-8111-111111111111",
            "DSCF1234.RAF",
            immich::AssetType::Image,
        );
        let p = cache_path(root, &a);
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("11111111-1111-4111-8111-111111111111"));
        assert!(name.ends_with(".RAF"), "the extension decides the decoder: {name}");
    }

    /// ⚠ A filename from the server is untrusted input. It must not be able to escape the cache.
    #[test]
    fn a_hostile_original_name_cannot_escape_the_cache_directory() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset(
            "11111111-1111-4111-8111-111111111111",
            "../../../etc/passwd.RAF",
            immich::AssetType::Image,
        );
        let p = cache_path(root, &a);
        assert_eq!(
            p.parent().unwrap(),
            root,
            "must stay directly inside the cache root"
        );
        assert!(!p.to_string_lossy().contains(".."));
    }

    #[test]
    fn two_servers_get_different_roots() {
        assert_ne!(
            host_slug("https://photos.example.com"),
            host_slug("https://other.example.com")
        );
    }

    #[test]
    fn a_host_slug_survives_a_port_a_path_and_odd_characters() {
        let s = host_slug("https://photos.example.com:2283/immich?x=1");
        assert!(!s.is_empty());
        assert!(!s.contains('/'), "a slug is one directory name: {s}");
        assert!(!s.contains(':'), "{s}");
    }

    /// ⚠ `host_slug` must never return an empty string — that would put the cache at the root of
    /// every server's shared directory and collide them all.
    #[test]
    fn a_useless_url_still_yields_a_usable_slug() {
        assert!(!host_slug("://").is_empty());
        assert!(!host_slug("").is_empty());
    }

    #[test]
    fn videos_and_unsupported_raws_are_skipped_and_the_rest_keep_their_human_names() {
        let root = std::path::Path::new("/c/immich/host");
        let assets = vec![
            asset(
                "11111111-1111-4111-8111-111111111111",
                "DSCF1234.RAF",
                immich::AssetType::Image,
            ),
            asset(
                "22222222-2222-4222-8222-222222222222",
                "MOV_0002.MP4",
                immich::AssetType::Video,
            ),
            asset(
                "33333333-3333-4333-8333-333333333333",
                "P1000003.PEF",
                immich::AssetType::Image,
            ),
            asset(
                "44444444-4444-4444-8444-444444444444",
                "holiday snap.JPG",
                immich::AssetType::Image,
            ),
        ];
        let entries = entries_from_album(&assets, root);
        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, vec!["DSCF1234.RAF", "holiday snap.JPG"]);
    }

    #[test]
    fn an_entry_carries_the_servers_modified_time_not_now() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset(
            "11111111-1111-4111-8111-111111111111",
            "DSCF1234.RAF",
            immich::AssetType::Image,
        );
        let e = &entries_from_album(&[a], root)[0];
        let secs = e
            .modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // 2025-08-14T09:21:33Z
        assert_eq!(secs, 1_755_163_293);
    }

    /// ⚠ An unparseable timestamp must not drop the photo — it sorts oldest, the same rule
    /// `CatalogEntry::modified` already documents for an unreadable file.
    #[test]
    fn an_unparseable_timestamp_becomes_the_epoch_rather_than_dropping_the_photo() {
        let root = std::path::Path::new("/c/immich/host");
        let mut a = asset(
            "11111111-1111-4111-8111-111111111111",
            "DSCF1234.RAF",
            immich::AssetType::Image,
        );
        a.file_modified_at = "not a date".into();
        let entries = entries_from_album(&[a], root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].modified, std::time::UNIX_EPOCH);
    }

    #[test]
    fn is_cached_is_false_before_the_file_exists_and_true_after() {
        let dir = std::env::temp_dir().join(format!("rustroom_immich_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.RAF");
        let _ = std::fs::remove_file(&p);
        assert!(!is_cached(&p));
        std::fs::write(&p, b"x").unwrap();
        assert!(is_cached(&p));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }
}

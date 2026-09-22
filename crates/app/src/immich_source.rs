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

/// A configured server, url and key both present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub url: String,
    pub key: String,
}

/// The configured connection, or `None` when either half is missing.
///
/// ⚠⚠ **The only reader of `Settings::immich_key`.** Everything else asks for a `Connection`, so
/// moving the key into the OS keychain later changes this function and nothing else.
pub fn connection(s: &crate::settings::Settings) -> Option<Connection> {
    let url = s.immich_url.trim();
    let key = s.immich_key.trim();
    if url.is_empty() || key.is_empty() {
        return None;
    }
    Some(Connection {
        url: url.to_string(),
        key: key.to_string(),
    })
}

/// The only writer of those two fields, for the same reason.
pub fn set_connection(s: &mut crate::settings::Settings, url: &str, key: &str) {
    s.immich_url = url.trim().to_string();
    s.immich_key = key.trim().to_string();
}

/// Decode thumbnail bytes the server sent.
///
/// ⚠⚠ Sniffs the format with `load_from_memory` rather than assuming webp. Immich's thumbnail and
/// preview formats are **both admin settings**, so either could arrive — and the workspace only
/// gained the `webp` feature for this.
pub fn decode_thumbnail(bytes: &[u8]) -> Option<core_image::Thumbnail> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    Some(core_image::Thumbnail {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

/// Is this path one of the Immich cache's?
///
/// ⚠ The test the thumbnail loader branches on. A plain prefix check, not a canonicalisation —
/// the file may not exist yet, which is the whole point.
pub fn is_immich_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// The asset id a cache path was built from — its stem.
///
/// ⚠ The inverse of [`cache_path`], and the reason the UUID leads rather than trails.
pub fn asset_id_of(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().to_string())
}

/// The connection the thumbnail loader reads.
pub type SharedConnection = std::sync::Arc<std::sync::Mutex<Option<Connection>>>;

/// The process-wide cell holding the current connection.
///
/// ⚠⚠ A `OnceLock` rather than a field on `RustroomApp`, because `ThumbLoader::new` is built from
/// inside `open_scan` where `self` is already mutably borrowed — and widening that signature
/// would touch its four call sites for no gain. `ThumbLoader::new` already captures the
/// `ThumbStore` and the `LookStore` for exactly this reason, with a comment saying so.
///
/// ⚠ A cell rather than a snapshot so that pasting a key in Settings is seen **without rebuilding
/// the loader** — a rebuild drops the open folder's whole thumbnail cache, measured at ~41 ms × N,
/// about 34 s on an 830-photo folder.
pub fn shared() -> SharedConnection {
    static STATE: std::sync::OnceLock<SharedConnection> = std::sync::OnceLock::new();
    STATE
        .get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(None)))
        .clone()
}

/// Point the shared cell at whatever the settings now say.
pub fn refresh_shared(s: &crate::settings::Settings) {
    if let Ok(mut slot) = shared().lock() {
        *slot = connection(s);
    }
}

/// Fetch and decode one Immich thumbnail, or `None` if this is not an Immich photo, the original
/// is already downloaded, or no server is configured.
///
/// ⚠⚠ Declines for an already-cached file, so a downloaded photo falls through to the ordinary
/// local path — on-disk store included — with no special case at all.
///
/// ⚠ The on-disk `ThumbStore` is deliberately **not** used here: its key is `(path, mtime, size)`
/// from `fs::metadata`, which cannot be built for a file that does not exist. The in-memory
/// `ThumbCache` still applies, keyed on `PhotoId` as it already is, so the cost is that album
/// thumbnails re-fetch after a restart. Accepted for this slice; the fix is a key of
/// `(asset id, updated_at)`.
pub fn remote_thumbnail(shared: &SharedConnection, path: &Path) -> Option<core_image::Thumbnail> {
    if is_cached(path) {
        return None;
    }
    let conn = shared.lock().ok()?.clone()?;
    if !is_immich_path(&cache_root(&conn.url), path) {
        return None;
    }
    let id = asset_id_of(path)?;
    let client = immich::Client::new(&conn.url, &conn.key).ok()?;
    let bytes = client.thumbnail(&id, immich::Size::Thumbnail).ok()?;
    decode_thumbnail(&bytes)
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

    /// A 1×1 red PNG. ⚠ Deliberately **not** a webp: the point is that the decode sniffs the
    /// format, because the server's thumbnail and preview formats are both admin settings.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn encoded_bytes_become_a_thumbnail_whatever_format_they_are_in() {
        let t = decode_thumbnail(TINY_PNG).expect("a PNG must decode");
        assert_eq!((t.width, t.height), (1, 1));
        assert_eq!(t.rgba.len(), 4);
    }

    /// A 2×2 lossless webp.
    ///
    /// ⚠⚠ **This fixture is the only thing verifying the `webp` feature is actually enabled.**
    /// webp is Immich's *default* thumbnail format, and `image` was built here with
    /// `default-features = false` — so if `webp` were ever dropped from the feature list, every
    /// album thumbnail would fail to decode and the grid would simply stay empty, with no error
    /// anywhere. A PNG fixture cannot catch that.
    const TINY_WEBP: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x1E, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38,
        0x4C, 0x11, 0x00, 0x00, 0x00, 0x2F, 0x01, 0x40, 0x00, 0x00, 0x0F, 0x10, 0xF3, 0xBF, 0xFF,
        0xF3, 0x1F, 0x0E, 0x32, 0xA2, 0xFF, 0x01, 0x00,
    ];

    #[test]
    fn a_webp_thumbnail_decodes_because_the_feature_is_enabled() {
        let t = decode_thumbnail(TINY_WEBP)
            .expect("webp must decode — it is Immich's default thumbnail format");
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(t.rgba.len(), 2 * 2 * 4);
    }

    #[test]
    fn rubbish_bytes_are_none_rather_than_a_panic() {
        assert!(decode_thumbnail(b"not an image at all").is_none());
    }

    #[test]
    fn a_path_inside_the_cache_root_is_recognised_as_an_immich_photo() {
        let root = std::path::Path::new("/c/rustroom/immich/host");
        assert!(is_immich_path(root, &root.join("11111111.RAF")));
        assert!(!is_immich_path(
            root,
            std::path::Path::new("/home/w/Pictures/a.RAF")
        ));
    }

    /// ⚠ The inverse of `cache_path`, and the reason the UUID leads rather than trails.
    #[test]
    fn the_asset_id_is_recoverable_from_a_cache_path() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset(
            "11111111-1111-4111-8111-111111111111",
            "DSCF1234.RAF",
            immich::AssetType::Image,
        );
        let p = cache_path(root, &a);
        assert_eq!(asset_id_of(&p).as_deref(), Some(a.id.as_str()));
    }

    /// ⚠⚠ A downloaded photo must fall through to the ordinary local loader, on-disk store and
    /// all. Returning a remote thumbnail for a file that is already here would re-fetch it on
    /// every cache miss forever.
    #[test]
    fn a_cached_photo_gets_no_remote_thumbnail() {
        let dir = std::env::temp_dir().join(format!("rustroom_rt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("11111111.RAF");
        std::fs::write(&p, b"x").unwrap();
        let shared: SharedConnection = std::sync::Arc::new(std::sync::Mutex::new(Some(
            Connection {
                url: "http://immich.invalid".into(),
                key: "k".into(),
            },
        )));
        assert!(
            remote_thumbnail(&shared, &p).is_none(),
            "an already-downloaded photo must not be fetched"
        );
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }

    /// ⚠ With no server configured the branch must decline instantly rather than erroring — it
    /// runs for every thumbnail of every ordinary local folder.
    #[test]
    fn an_unconfigured_connection_declines_without_touching_the_network() {
        let shared: SharedConnection = std::sync::Arc::new(std::sync::Mutex::new(None));
        let p = std::path::Path::new("/nonexistent/rustroom/11111111.RAF");
        assert!(remote_thumbnail(&shared, p).is_none());
    }

    #[test]
    fn a_connection_needs_both_a_url_and_a_key() {
        let mut s = crate::settings::Settings::default();
        assert!(connection(&s).is_none(), "unconfigured by default");
        s.immich_url = "https://photos.example.com".into();
        assert!(connection(&s).is_none(), "a URL alone is not a connection");
        s.immich_key = "abc".into();
        let c = connection(&s).expect("both set is a connection");
        assert_eq!(c.url, "https://photos.example.com");
        assert_eq!(c.key, "abc");
    }

    /// ⚠ Whitespace pasted around a key is the commonest way this fails, and the failure looks
    /// like "the server rejected the key".
    #[test]
    fn surrounding_whitespace_is_trimmed_off_both() {
        let mut s = crate::settings::Settings::default();
        s.immich_url = "  https://photos.example.com  ".into();
        s.immich_key = "  abc\n".into();
        let c = connection(&s).expect("must be a connection");
        assert_eq!(c.url, "https://photos.example.com");
        assert_eq!(c.key, "abc");
    }

    /// ⚠⚠ **The new invariant, pinned.** A `CatalogEntry` may name a file that does not exist,
    /// and `open_scan` calls `sidecar::load_all`, `import_xmp`, `queue_metadata`, `reload_marks`
    /// and `reload_captures` on every row. None of them may fail on an absent file.
    #[test]
    fn a_sidecar_for_a_file_that_does_not_exist_is_neutral_not_an_error() {
        let p = std::path::Path::new(
            "/nonexistent/rustroom/11111111-1111-4111-8111-111111111111.RAF",
        );
        let s = crate::sidecar::load_all(p);
        assert!(s.is_neutral(), "no file and no sidecar must read as neutral");
    }

    #[test]
    fn edited_at_for_a_file_that_does_not_exist_is_none() {
        let p = std::path::Path::new(
            "/nonexistent/rustroom/22222222-2222-4222-8222-222222222222.RAF",
        );
        assert!(crate::sidecar::edited_at(p).is_none());
    }

    /// ⚠ And the mapping itself must survive a whole album of absent files — which is the normal
    /// case, not an edge one: every row is absent until it is opened.
    #[test]
    fn a_whole_album_of_absent_files_still_maps_to_rows() {
        let root = std::path::Path::new("/nonexistent/rustroom/immich/host");
        let assets: Vec<immich::Asset> = (0..5)
            .map(|i| {
                asset(
                    &format!("1111111{i}-1111-4111-8111-111111111111"),
                    "DSCF1234.RAF",
                    immich::AssetType::Image,
                )
            })
            .collect();
        let entries = entries_from_album(&assets, root);
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|e| e.edited.is_none()));
        assert!(entries.iter().all(|e| !is_cached(&e.path)));
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

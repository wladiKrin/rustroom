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

/// The file names present in this server's cache directory.
///
/// ⚠⚠ **One `read_dir`, answering for every asset at once.** The alternative — [`is_cached`] per
/// thumbnail — is a stat per cell per frame in a grid that draws hundreds of cells sixty times a
/// second, which is the `apply_view_demosaic` mistake in a new costume.
///
/// ⚠ A directory that does not exist yields an **empty** set rather than an error: nothing downloaded
/// yet is an ordinary state on a fresh install, not a failure.
///
/// ⚠⚠ **Supported images only, never every file.** A `.json` sidecar sits beside every edited photo, so
/// counting files counted each edited one twice — reported in use as the rail saying **34** where 17
/// photos were there. Filtering through `catalog::is_supported` is also exactly what `scan_folder`
/// counts when the folder is opened, so the rail's number and the grid's agree by construction rather
/// than by coincidence.
pub fn present_names(root: &Path) -> std::collections::HashSet<std::ffi::OsString> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Default::default();
    };
    rd.flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name())
        .filter(|n| catalog::is_supported(Path::new(n)))
        .collect()
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
pub fn entries_from_assets(assets: &[immich::Asset], root: &Path) -> Vec<catalog::CatalogEntry> {
    // ⚠ Record each asset's version while we have it, so the thumbnail loader can key its cache
    // entries. Done here because this is the one place a library's assets all pass through.
    if let Ok(mut m) = shared_versions().lock() {
        for a in assets {
            m.insert(a.id.clone(), a.updated_at.clone());
        }
    }
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
    // ⚠⚠ **THE ONE GATE FOR THE WHOLE INTEGRATION.** Nine call sites read the connection through
    // this function — the worker's fetcher, `refresh_shared`, `immich_panel`, `copies`,
    // `immich_flow` — so a single condition here makes every one of them decline, and no surface
    // needs to grow its own `if immich_enabled`. That is deliberate: the requirement is "no mention
    // of immich at all" when it is off, and a list of places to remember is how that stops being
    // true. `remote_thumbnail` already declines without a connection, so downloaded photos fall
    // through to the ordinary local path with no special case.
    if !s.immich_enabled {
        return None;
    }
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

/// How many photos the server holds: the sum of its month counts, for the Timeline row.
pub fn photo_total(buckets: &[immich::Bucket]) -> u64 {
    buckets.iter().map(|b| u64::from(b.count)).sum()
}

/// Group the server's buckets into **years newest first, months chronological within each year**.
///
/// ⚠⚠ Not `date_tree::group`. Immich's buckets are monthly, and pushing them through that
/// function would produce a day node meaning "the whole month" — a lie in the data rather than a
/// rendering detail. Twenty lines is cheaper than the lie, and `date_tree` stays untouched.
///
/// ⚠⚠ **The ordering was settled by use, in two steps (2026-09-23).** Built fully ascending to match
/// the local Dates section, on the argument that two date trees reading opposite ways looks like a
/// bug. Use disagreed: recent months are the ones you want first. Then fully descending read oddly in
/// turn — a year's months running backwards is not how anyone thinks about a year. The answer is
/// **mixed**: years by recency, months chronological. `Catalog::capture_days` now orders the same way,
/// so the two trees agree.
///
/// ⚠ A row whose `time_bucket` is not `YYYY-MM-DD` is **skipped**: one bad row must not empty the
/// tree. ⚠ Sorted explicitly rather than trusting the server's order, so a gap between months
/// cannot merge two or drop one — and so this does not silently depend on the server continuing to
/// return them newest-first.
pub fn group_buckets(buckets: &[immich::Bucket]) -> Vec<BucketYear> {
    let mut rows: Vec<(i32, u32, u32, String)> = buckets
        .iter()
        .filter_map(|b| {
            let s = &b.time_bucket;
            let year: i32 = s.get(0..4)?.parse().ok()?;
            let month: u32 = s.get(5..7)?.parse().ok()?;
            if !(1..=12).contains(&month) {
                return None;
            }
            Some((year, month, b.count, s.clone()))
        })
        .collect();
    // ⚠⚠ **Years descend, months ascend inside them** (user's call, 2026-09-23): you pick the year by
    // recency, then read it forwards. Both levels descending was tried first and reads oddly — a
    // year's months running backwards is not how anyone thinks about a year.
    rows.sort_by_key(|&(y, m, _, _)| (std::cmp::Reverse(y), m));

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

/// Asset id → its `updatedAt`, for versioning cached thumbnails.
pub type SharedVersions =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;

/// The process-wide cell holding the versions of the assets currently on show.
///
/// ⚠⚠ A second cell rather than a field on `ThumbRequest`, which carries only `id` and `edits`.
/// Widening that would touch the whole thumbnail pipeline to pass a string the loader needs only
/// for Immich photos. Same `OnceLock` idiom as [`shared`], for the same reason.
///
/// ⚠ Filled by [`entries_from_assets`] when a library is built, so the loader always has the
/// version of whatever is on screen. An asset missing from the map simply gets no cache entry — a
/// miss, never a wrong one.
pub fn shared_versions() -> SharedVersions {
    static STATE: std::sync::OnceLock<SharedVersions> = std::sync::OnceLock::new();
    STATE
        .get_or_init(|| {
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
        })
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
/// ⚠ Consults the on-disk store first and writes on success, keyed on
/// `(asset id, updatedAt)` — which is why `Identity` is an enum. Album thumbnails therefore survive
/// a restart; before slice 2 they re-fetched every time, because the store could not key a file
/// that does not exist.
pub fn remote_thumbnail(
    shared: &SharedConnection,
    versions: &SharedVersions,
    store: &crate::thumb_store::ThumbStore,
    path: &Path,
) -> Option<core_image::Thumbnail> {
    if is_cached(path) {
        return None;
    }
    let conn = shared.lock().ok()?.clone()?;
    if !is_immich_path(&cache_root(&conn.url), path) {
        return None;
    }
    let id = asset_id_of(path)?;
    // ⚠ The on-disk store first: a hit is a local read against an HTTP round trip. An asset with
    // no known version is simply not cached — a miss, never a wrongly-keyed entry.
    let key = versions
        .lock()
        .ok()
        .and_then(|m| m.get(&id).cloned())
        .map(|v| crate::thumb_store::Identity::remote(&id, &v));
    if let Some(k) = &key {
        if let Some(t) = store.get(k) {
            return Some(t);
        }
    }
    let client = immich::Client::new(&conn.url, &conn.key).ok()?;
    let bytes = client.thumbnail(&id, immich::Size::Thumbnail).ok()?;
    let thumb = decode_thumbnail(&bytes)?;
    if let Some(k) = &key {
        store.put(k, &thumb);
    }
    Some(thumb)
}

#[cfg(test)]
mod tests {

    /// ⚠⚠ **Reported in use: the rail said 34 where 17 photos were there.** The cache directory holds
    /// a **`.json` sidecar beside every edited photo**, so counting every file counted each one twice.
    /// Only supported images are assets — which is also what `scan_folder` counts when the folder is
    /// opened, so the rail's number and the grid's now agree by construction.
    #[test]
    fn present_names_ignores_the_sidecars_beside_the_photos() {
        let d = test_dir::path("sidecar");
        std::fs::write(d.join("a.JPG"), b"x").unwrap();
        std::fs::write(d.join("a.json"), b"{}").unwrap();
        std::fs::write(d.join("b.jpg"), b"x").unwrap();
        std::fs::write(d.join("b.json"), b"{}").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();

        let got = present_names(&d);
        assert_eq!(got.len(), 2, "two photos, not five files: {got:?}");
        assert!(
            got.contains(std::ffi::OsStr::new("a.JPG")),
            "and case is kept"
        );
        assert!(!got.contains(std::ffi::OsStr::new("a.json")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⚠⚠ One `read_dir`, never a stat per asset: the grid draws hundreds of cells sixty times a
    /// second.
    #[test]
    fn present_names_lists_what_is_in_the_cache_directory() {
        let d = test_dir::path("present");
        std::fs::write(d.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.join("b.jpeg"), b"x").unwrap();
        std::fs::create_dir_all(d.join("a_directory")).unwrap();

        let got = present_names(&d);
        assert_eq!(got.len(), 2, "files only, not the directory: {got:?}");
        assert!(got.contains(std::ffi::OsStr::new("a.jpg")));
        assert!(!got.contains(std::ffi::OsStr::new("c.jpg")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⚠ A cache directory that does not exist yet is **empty**, not an error.
    #[test]
    fn present_names_on_a_missing_directory_is_empty() {
        assert!(present_names(Path::new("/nonexistent-rr-cache-dir")).is_empty());
    }

    use super::*;

    fn asset(id: &str, name: &str, kind: immich::AssetType) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: kind,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: Some(100),
            height: Some(100),
            thumbhash: Some("x".into()),
            file_modified_at: "2025-08-14T09:21:33.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
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
        assert!(
            name.ends_with(".RAF"),
            "the extension decides the decoder: {name}"
        );
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
        let entries = entries_from_assets(&assets, root);
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
        let e = &entries_from_assets(&[a], root)[0];
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
        let entries = entries_from_assets(&[a], root);
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
        let dir = test_dir::path("rt");
        let p = dir.join("11111111.RAF");
        std::fs::write(&p, b"x").unwrap();
        let shared: SharedConnection =
            std::sync::Arc::new(std::sync::Mutex::new(Some(Connection {
                url: "http://immich.invalid".into(),
                key: "k".into(),
            })));
        assert!(
            remote_thumbnail(
                &shared,
                &shared_versions(),
                &crate::thumb_store::ThumbStore::new(dir.join("store"), u64::MAX),
                &p
            )
            .is_none(),
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
        assert!(remote_thumbnail(
            &shared,
            &shared_versions(),
            &crate::thumb_store::ThumbStore::new(test_dir::path("unconf-store"), u64::MAX),
            p
        )
        .is_none());
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
        let s = crate::settings::Settings {
            immich_url: "  https://photos.example.com  ".into(),
            immich_key: "  abc\n".into(),
            ..Default::default()
        };
        let c = connection(&s).expect("must be a connection");
        assert_eq!(c.url, "https://photos.example.com");
        assert_eq!(c.key, "abc");
    }

    fn bucket(s: &str, n: u32) -> immich::Bucket {
        immich::Bucket {
            time_bucket: s.to_string(),
            count: n,
        }
    }

    #[test]
    fn photo_total_is_the_sum_of_the_months() {
        assert_eq!(
            photo_total(&[bucket("2025-03-01", 57), bucket("2025-04-01", 43)]),
            100
        );
        assert_eq!(photo_total(&[]), 0);
    }

    /// ⚠⚠ **Years newest first, months chronological inside them.** Both levels descending was tried
    /// and reads oddly: a year's months running backwards is not how anyone thinks about a year.
    #[test]
    fn group_buckets_puts_the_newest_year_first_and_reads_its_months_forwards() {
        let v = vec![
            bucket("2025-12-01", 189),
            bucket("2026-07-01", 233),
            bucket("2026-08-01", 76),
        ];
        let years = group_buckets(&v);
        assert_eq!(years.len(), 2);
        assert_eq!(years[0].year, 2026, "newest year first");
        assert_eq!(years[0].count, 76 + 233);
        assert_eq!(years[1].year, 2025);
        assert_eq!(years[1].count, 189);
        assert_eq!(
            years[0].months.iter().map(|m| m.month).collect::<Vec<_>>(),
            vec![7, 8],
            "months ascending within the year"
        );
    }

    /// ⚠ Sorted here rather than trusted from the reply, so the tree is right even if a server
    /// returns its buckets in some other order.
    #[test]
    fn group_buckets_does_not_depend_on_the_servers_own_order() {
        let shuffled = vec![
            bucket("2026-04-01", 1),
            bucket("2026-08-01", 2),
            bucket("2026-06-01", 3),
        ];
        let years = group_buckets(&shuffled);
        assert_eq!(
            years[0].months.iter().map(|m| m.month).collect::<Vec<_>>(),
            vec![4, 6, 8]
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

    /// ⚠⚠ **The new invariant, pinned.** A `CatalogEntry` may name a file that does not exist,
    /// and `open_scan` calls `sidecar::load_all`, `import_xmp`, `queue_metadata`, `reload_marks`
    /// and `reload_captures` on every row. None of them may fail on an absent file.
    #[test]
    fn a_sidecar_for_a_file_that_does_not_exist_is_neutral_not_an_error() {
        let p =
            std::path::Path::new("/nonexistent/rustroom/11111111-1111-4111-8111-111111111111.RAF");
        let s = crate::sidecar::load_all(p);
        assert!(
            s.is_neutral(),
            "no file and no sidecar must read as neutral"
        );
    }

    #[test]
    fn edited_at_for_a_file_that_does_not_exist_is_none() {
        let p =
            std::path::Path::new("/nonexistent/rustroom/22222222-2222-4222-8222-222222222222.RAF");
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
        let entries = entries_from_assets(&assets, root);
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|e| e.edited.is_none()));
        assert!(entries.iter().all(|e| !is_cached(&e.path)));
    }

    #[test]
    fn is_cached_is_false_before_the_file_exists_and_true_after() {
        let dir = test_dir::path("immich");
        let p = dir.join("a.RAF");
        let _ = std::fs::remove_file(&p);
        assert!(!is_cached(&p));
        std::fs::write(&p, b"x").unwrap();
        assert!(is_cached(&p));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }

    /// ⚠⚠ **The one gate, and both halves asserted** — `None` when the toggle is off *despite a
    /// perfectly good URL and key*, and `Some` when it is on. An absence alone would pass against a
    /// function that always returned `None`.
    #[test]
    fn the_gate_turns_a_working_connection_off() {
        let on = crate::settings::Settings {
            immich_enabled: true,
            immich_url: "https://immich.example".to_string(),
            immich_key: "a-key".to_string(),
            ..crate::settings::Settings::default()
        };
        assert!(
            connection(&on).is_some(),
            "a configured server must connect"
        );

        let off = crate::settings::Settings {
            immich_enabled: false,
            ..on.clone()
        };
        assert!(
            connection(&off).is_none(),
            "the gate must win over a valid URL and key"
        );

        // ⚠ And the pre-existing rule is unchanged: enabled but unconfigured is still `None`, so the
        // gate did not become the *only* condition.
        let bare = crate::settings::Settings {
            immich_url: String::new(),
            ..on
        };
        assert!(connection(&bare).is_none());
    }
}

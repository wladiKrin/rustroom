//! The line the top bar centres after the current photo's name: its camera, its decoded size
//! and its file size.
//!
//! ⚠ A lib module: the sentence and the byte formatting are pure, and the size cache holds no
//! egui. The bar only paints what this returns.

use std::path::{Path, PathBuf};

/// The weak line after the name: `FUJIFILM X-T4 · 6240 × 4160 · 24.3 MB`, with whichever
/// parts are known.
///
/// ⚠ No format: it is the filename's extension already (user's call, 2026-09-20). `dims` is
/// the decoded pixel size, known only once the photo is open in Develop — the Library's
/// thumbnails come from the camera's embedded JPEG and say nothing about it.
pub fn meta(camera: Option<&str>, dims: Option<(usize, usize)>, bytes: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(c) = camera.map(str::trim).filter(|c| !c.is_empty()) {
        parts.push(c.to_string());
    }
    if let Some((w, h)) = dims {
        parts.push(format!("{w} × {h}"));
    }
    if let Some(b) = bytes {
        parts.push(format_bytes(b));
    }
    parts.join("  ·  ")
}

/// What the bar knows about `path` beyond its name, from the two sources both views have.
pub struct Facts<'a> {
    pub camera: Option<&'a str>,
    pub dims: Option<(usize, usize)>,
}

/// One rule for both views, so a view switch cannot change the line.
///
/// The camera is the catalog's capture scan when it has one, else the open session's decode
/// label when the session *is* this photo; the decoded size is the session's, again only when
/// it is this photo. ⚠ Never the session's camera for a different photo: a photo selected in
/// the grid while another is open would borrow the open one's camera.
pub fn facts<'a>(
    path: &Path,
    catalog_camera: Option<&'a str>,
    session: Option<(&Path, &'a str, (usize, usize))>,
) -> Facts<'a> {
    let same = session.filter(|(p, _, _)| *p == path);
    let camera = catalog_camera
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .or_else(|| same.map(|(_, m, _)| m.trim()).filter(|m| !m.is_empty()));
    Facts {
        camera,
        dims: same.map(|(_, _, d)| d),
    }
}

/// Bytes as a photographer reads them: `812 KB`, `24.3 MB`, `1.2 GB`.
///
/// ⚠ Decimal units, the ones Finder and every camera spec sheet use, and one decimal only
/// above a megabyte — a file size is a magnitude, not a checksum.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    const MB: f64 = KB * 1000.0;
    const GB: f64 = MB * 1000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// The last file size looked up, so the bar asks the filesystem once per photo rather than
/// once per frame.
///
/// ⚠ One entry, not a map: the bar shows one photo at a time, and a map would hold a size for
/// every photo ever selected until the app quit — stale the moment a file was rewritten.
#[derive(Default)]
pub struct SizeCache {
    last: Option<(PathBuf, Option<u64>)>,
}

impl SizeCache {
    /// The size of `path`, from the cache when it is the same path as last time.
    pub fn bytes_of(&mut self, path: &Path) -> Option<u64> {
        if let Some((p, b)) = &self.last {
            if p == path {
                return *b;
            }
        }
        let b = std::fs::metadata(path).ok().map(|m| m.len());
        self.last = Some((path.to_path_buf(), b));
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meta_names_only_what_is_known() {
        assert_eq!(
            meta(Some("FUJIFILM X-T4"), Some((6240, 4160)), Some(24_300_000)),
            "FUJIFILM X-T4  ·  6240 × 4160  ·  24.3 MB"
        );
        assert_eq!(meta(None, None, None), "");
        assert_eq!(meta(Some(""), None, Some(812_000)), "812 KB");
    }

    #[test]
    fn the_facts_follow_one_rule_whichever_view_asks() {
        let a = Path::new("/p/a.RAF");
        let b = Path::new("/p/b.RAF");
        let session = Some((a, "X-T4 (decode)", (6240, 4160)));
        // The catalog wins for the camera; the session gives the size for its own photo.
        let f = facts(a, Some("FUJIFILM X-T4"), session);
        assert_eq!(f.camera, Some("FUJIFILM X-T4"));
        assert_eq!(f.dims, Some((6240, 4160)));
        // No catalog entry yet: the session's label, for the session's photo only.
        let f = facts(a, None, session);
        assert_eq!(f.camera, Some("X-T4 (decode)"));
        // ⚠ Another photo selected while `a` is open borrows nothing from the session.
        let f = facts(b, None, session);
        assert_eq!(f.camera, None);
        assert_eq!(f.dims, None);
        let f = facts(b, Some(" "), session);
        assert_eq!(f.camera, None, "blank camera names are not names");
    }

    #[test]
    fn bytes_read_as_a_photographer_reads_them() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(812_000), "812 KB");
        assert_eq!(format_bytes(24_300_000), "24.3 MB");
        assert_eq!(format_bytes(1_200_000_000), "1.2 GB");
    }

    #[test]
    fn the_size_cache_reads_a_file_once_and_follows_a_change_of_photo() {
        let dir = std::env::temp_dir().join(format!("rustroom-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.raf");
        let b = dir.join("b.raf");
        std::fs::write(&a, [0u8; 100]).unwrap();
        std::fs::write(&b, [0u8; 7]).unwrap();
        let mut cache = SizeCache::default();
        assert_eq!(cache.bytes_of(&a), Some(100));
        // ⚠ Cached: rewriting the file behind the cache is not seen until the photo changes —
        // the trade the one-entry cache makes on purpose.
        std::fs::write(&a, [0u8; 5]).unwrap();
        assert_eq!(cache.bytes_of(&a), Some(100));
        assert_eq!(cache.bytes_of(&b), Some(7));
        assert_eq!(cache.bytes_of(&a), Some(5));
        assert_eq!(cache.bytes_of(&dir.join("missing.raf")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

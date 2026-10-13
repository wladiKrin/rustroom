//! Removing cached Immich originals, and nothing else.
//!
//! ⚠⚠ **The whole feature rests on a property that already exists**: `immich_source::cache_path` is
//! derived from the asset's UUID and nothing else, and its own doc says *"re-downloading an evicted or
//! deleted file must restore byte-identical identity"*. So a removed photo can be fetched again to the
//! same path, and everything that recorded that path still points at it.
//!
//! ⚠ **The API key is read-only** by the Settings help text's own instruction, so nothing here can reach
//! the server even in principle. This deletes local copies; the photos stay on Immich.
//!
//! ⚠ **No egui.** A lib module, testable with no window and no server.

use std::path::{Path, PathBuf};

/// What a removal did, for the status line to report.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// Files actually deleted.
    pub removed: usize,
}

/// Delete the cached originals at `paths`, leaving everything else alone.
///
/// ⚠⚠ **Deletes only the image file, and three things are deliberately left behind:**
///
/// - **The sidecar**, which holds the edits. `sidecar::path` appends `.json` to the *whole* filename
///   (`abc.jpg` → `abc.jpg.json`) rather than replacing the extension, so removing `abc.jpg` cannot
///   touch it. ⚠ That is safe by construction — and it is asserted against real files anyway, because
///   "safe by construction" is exactly the kind of claim this project has been wrong about.
/// - **The catalog row**, which holds the marks (slice 3). `immich_catalog::row_is_browsable` already
///   hides an absent Immich row from catalog-wide views, so the Library stays tidy with nothing deleted.
/// - **The thumbnail**, which lives in its own directory under its own budget. Removing originals must
///   not make browsing slow again — that would defeat the point of the feature.
///
/// ⚠ **Only supported images are removed.** A stray `.json`, `.txt` or `.part` in the cache directory is
/// not a download, and `catalog::is_supported` is the same test `present_names` uses — so what this
/// removes and what the rail counts cannot disagree.
///
/// ⚠⚠ **The photo open in Develop is removed like any other** (user's call, 2026-09-24). An earlier
/// version skipped it, on the grounds that a render reading a vanishing file is how this crashes — but
/// removing everything *except* the one you are looking at is a surprising exception. **The caller closes
/// the session first**, which is why this function needs no notion of "open" at all — see
/// `RustroomApp::remove_downloaded`.
///
/// ⚠ A file that is already gone is **not** an error: pressing the button twice must not report a
/// failure.
pub fn remove_downloads(paths: &[PathBuf]) -> Removed {
    let mut out = Removed::default();
    for p in paths {
        if !catalog::is_supported(p) {
            continue;
        }
        // ⚠ `is_file` first, so an absent path is silently fine rather than an error to report.
        if p.is_file() && std::fs::remove_file(p).is_ok() {
            out.removed += 1;
        }
    }
    out
}

/// How much the downloads take: the bytes and the number of photos.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub photos: usize,
}

/// Every downloaded original in `root`, sorted — what a clear removes (M10.15b).
///
/// ⚠ Only `catalog::is_supported` files — the test `remove_downloads` and `present_names` use — so a
/// sidecar, a `.part` still being written or a `.DS_Store` is never counted or removed. A missing
/// directory is nothing downloaded.
pub fn downloads(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| catalog::is_supported(p))
        .collect();
    out.sort();
    out
}

/// The downloads' total size and count — one directory listing, ~3 ms for 1,000 photos (M10.15b).
pub fn usage(root: &Path) -> Usage {
    let mut u = Usage::default();
    for p in downloads(root) {
        if let Ok(m) = std::fs::metadata(&p) {
            u.bytes += m.len();
            u.photos += 1;
        }
    }
    u
}

/// The soft limit in bytes — **decimal**, the units `photo_info::format_bytes` prints, so "12.4 GB"
/// and "10 GB limit" are in the same unit.
pub fn limit_bytes(gb: u32) -> u64 {
    gb as u64 * 1_000_000_000
}

/// Whether startup opens the Clear dialog: Immich connected, the warning on, and the downloads
/// **strictly** over the limit.
pub fn warn_at_startup(connected: bool, warn: bool, limit_gb: u32, usage: Usage) -> bool {
    connected && warn && usage.bytes > limit_bytes(limit_gb)
}

fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".to_string()
    } else {
        format!("{n} photos")
    }
}

/// The Settings line: "74.0 MB · 21 photos", or "Nothing downloaded".
pub fn size_line(usage: Usage) -> String {
    if usage.photos == 0 {
        return "Nothing downloaded".to_string();
    }
    format!(
        "{} · {}",
        crate::photo_info::format_bytes(usage.bytes),
        photos(usage.photos)
    )
}

/// The dialog's title — the size at startup, the verb when asked from Settings.
pub fn dialog_title(usage: Usage, startup: bool) -> String {
    if startup {
        format!(
            "Immich downloads use {}",
            crate::photo_info::format_bytes(usage.bytes)
        )
    } else {
        "Clear Immich downloads".to_string()
    }
}

/// The dialog's body. ⚠ Always names the edits: what a clear keeps is the reason it is safe.
pub fn dialog_body(usage: Usage, limit_gb: u32, startup: bool) -> String {
    let n = match usage.photos {
        1 => "1 downloaded photo".to_string(),
        n => format!("{n} downloaded photos"),
    };
    let rest = format!(
        "Clearing removes {n}; edits, marks and thumbnails stay, and a photo downloads again when you open it"
    );
    if startup {
        format!("Over your {limit_gb} GB limit. {rest}")
    } else {
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory of its own — `test_dir` makes it unique per test and per run.
    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("imcache-{tag}"))
    }

    /// ⚠⚠ **THE TEST THIS FEATURE EXISTS TO PASS.** The sidecar holds the user's edits. It is safe by
    /// construction — `.json` is *appended* to the whole filename — but that is asserted against real
    /// files on disk, because "safe by construction" is the shape of claim this project has been wrong
    /// about twice in a week.
    #[test]
    fn removing_a_download_keeps_its_edits() {
        let d = temp_dir("edits");
        let img = d.join("abc.jpg");
        let car = crate::sidecar::path(&img);
        std::fs::write(&img, b"jpeg").unwrap();
        std::fs::write(&car, b"{\"version\":7}").unwrap();
        assert_eq!(
            car.file_name().unwrap().to_string_lossy(),
            "abc.jpg.json",
            "the fixture must use the real sidecar naming, or it proves nothing"
        );

        let out = remove_downloads(std::slice::from_ref(&img));

        assert_eq!(out, Removed { removed: 1 });
        assert!(!img.exists(), "the cached original must go");
        assert!(car.exists(), "the sidecar must NOT — it holds the edits");
        assert_eq!(
            std::fs::read(&car).unwrap(),
            b"{\"version\":7}",
            "and be untouched, not merely present"
        );
    }

    /// ⚠ Only images. A sidecar, a note or a leftover `.part` in the cache directory is not a download —
    /// and this is the same `is_supported` test `present_names` uses, so the two cannot disagree.
    #[test]
    fn a_non_image_is_never_removed() {
        let d = temp_dir("nonimage");
        let keep: Vec<PathBuf> = ["a.jpg.json", "notes.txt", "b.jpg.part"]
            .iter()
            .map(|n| {
                let p = d.join(n);
                std::fs::write(&p, b"x").unwrap();
                p
            })
            .collect();

        let out = remove_downloads(&keep);

        assert_eq!(out.removed, 0, "nothing here is a downloaded photo");
        for p in &keep {
            assert!(p.exists(), "{p:?} was removed and should not have been");
        }
    }

    /// ⚠ Pressing the button twice must not report a failure.
    #[test]
    fn an_absent_file_is_not_an_error() {
        let d = temp_dir("absent");
        let img = d.join("gone.jpg");
        assert_eq!(remove_downloads(&[img]), Removed::default());
    }

    /// ⚠ The ordinary case, so the counts are not only exercised by the edge cases.
    #[test]
    fn several_downloads_are_removed_and_counted() {
        let d = temp_dir("several");
        let files: Vec<PathBuf> = (0..5)
            .map(|i| {
                let p = d.join(format!("p{i}.jpg"));
                std::fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        let out = remove_downloads(&files);
        assert_eq!(out, Removed { removed: 5 });
        assert!(files.iter().all(|p| !p.exists()));
    }

    #[test]
    fn usage_and_downloads_count_images_only() {
        let d = temp_dir("usage");
        std::fs::write(d.join("a.jpg"), b"jpeg").unwrap();
        std::fs::write(d.join("a.jpg.json"), b"{}").unwrap();
        std::fs::write(d.join("b.RAF.part"), b"half").unwrap();
        std::fs::write(d.join(".DS_Store"), b"x").unwrap();
        for f in ["a.jpg", "a.jpg.json", "b.RAF.part", ".DS_Store"] {
            assert!(d.join(f).is_file(), "fixture: {f}");
        }
        assert_eq!(
            usage(&d),
            Usage {
                bytes: 4,
                photos: 1
            }
        );
        assert_eq!(downloads(&d), vec![d.join("a.jpg")]);
    }

    #[test]
    fn a_missing_or_sidecar_only_directory_is_nothing_downloaded() {
        let d = temp_dir("usage-empty");
        assert_eq!(usage(&d.join("absent")), Usage::default());
        std::fs::write(d.join("gone.jpg.json"), b"{}").unwrap();
        assert_eq!(usage(&d), Usage::default());
        assert!(downloads(&d).is_empty());
    }

    #[test]
    fn the_startup_warning_needs_all_three_and_strictly_over() {
        let at = Usage {
            bytes: 1_000_000_000,
            photos: 1,
        };
        let over = Usage {
            bytes: 1_000_000_001,
            photos: 1,
        };
        assert!(!warn_at_startup(true, true, 1, at), "exactly at the limit");
        assert!(warn_at_startup(true, true, 1, over));
        assert!(!warn_at_startup(true, false, 1, over), "the warning is off");
        assert!(
            !warn_at_startup(false, true, 1, over),
            "Immich is not connected"
        );
        assert_eq!(limit_bytes(10), 10_000_000_000);
    }

    #[test]
    fn the_size_line_counts_photos_or_says_nothing() {
        assert_eq!(
            size_line(Usage {
                bytes: 74_000_000,
                photos: 21
            }),
            "74.0 MB · 21 photos"
        );
        assert_eq!(
            size_line(Usage {
                bytes: 1_200_000_000,
                photos: 1
            }),
            "1.2 GB · 1 photo"
        );
        assert_eq!(size_line(Usage::default()), "Nothing downloaded");
    }

    #[test]
    fn the_dialog_names_the_size_the_limit_the_count_and_the_edits() {
        let u = Usage {
            bytes: 12_400_000_000,
            photos: 412,
        };
        assert_eq!(dialog_title(u, true), "Immich downloads use 12.4 GB");
        assert_eq!(dialog_title(u, false), "Clear Immich downloads");
        let at_start = dialog_body(u, 10, true);
        assert!(
            at_start.starts_with("Over your 10 GB limit. "),
            "{at_start}"
        );
        let asked = dialog_body(u, 10, false);
        assert!(!asked.contains("limit"), "{asked}");
        for b in [&at_start, &asked] {
            assert!(b.contains("412 downloaded photos;"), "{b}");
            assert!(b.contains("edits"), "the reassurance went missing: {b}");
        }
        let one = dialog_body(
            Usage {
                bytes: 31_000_000,
                photos: 1,
            },
            10,
            false,
        );
        assert!(one.contains("1 downloaded photo;"), "{one}");
    }
}

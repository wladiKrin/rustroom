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

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Its own directory per test, process id **and** a counter — the collision `thumb_store`'s doc
    /// records having already bitten this repo.
    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_imcache_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
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
}

//! Turning a collection's membership into something the browser can show.
//!
//! ⚠⚠ **A collection is browsed, not filtered.** Selecting one replaces the Library's contents
//! with its members, which may come from *any* folder ever opened. Merely filtering the folder
//! you happen to have open would reproduce exactly the fragmentation that choosing one catalog
//! over one-per-folder was meant to avoid.
//!
//! ⚠ Everything downstream keys on paths already — `entries::build`, `ThumbLoader`, `Selection`
//! and the filmstrip need no change at all. That is why this costs almost nothing.

use catalog::CatalogEntry;
use std::path::{Path, PathBuf};

/// Build a browsable scan from a collection's members.
///
/// **Both times are injected.** `edited_at` for the reason `CatalogEntry::edited` is filled by its
/// caller — the sidecar naming convention belongs to `app`, and it keeps this function testable
/// without disk. `modified_at` for a sharper one: **All Photos must not walk the filesystem.**
/// This used to call `std::fs::metadata` per member, which is 10,000 stats on a view that
/// otherwise touches no files, while the catalog has stored `mtime` since M4.1 and nothing had
/// ever read it.
///
/// ⚠ The `UNIX_EPOCH` fallback for an unreadable file now lives in the callers that stat. The rule
/// it protects is unchanged: an unreadable file sorts to the oldest end rather than failing the
/// whole load.
///
/// ⚠ One entry per **file**, not per member. `entries::build` expands copies from the sidecar
/// itself, so passing copies through here would double every one of them.
pub fn scan_of(
    members: &[(PathBuf, u32)],
    edited_at: impl Fn(&Path) -> Option<std::time::SystemTime>,
    modified_at: impl Fn(&Path) -> std::time::SystemTime,
) -> Vec<CatalogEntry> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for (path, _copy) in members {
        if seen.contains(path) {
            continue;
        }
        seen.push(path.clone());
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // ⚠ A vanished file is still listed. Dropping it would make an unplugged drive look like
        // the collection had emptied itself.
        let modified = modified_at(path);
        out.push(CatalogEntry {
            path: path.clone(),
            file_name,
            modified,
            edited: edited_at(path),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_become_a_scan_of_one_entry_per_file() {
        // ⚠ One entry per *file*, not per member: `entries::build` expands copies itself from the
        // sidecar. Passing copies through here would double them.
        let members = vec![
            (PathBuf::from("/x/a.RAF"), 0),
            (PathBuf::from("/x/a.RAF"), 1),
            (PathBuf::from("/y/b.RAF"), 0),
        ];
        let scan = scan_of(&members, |_| None, |_| std::time::SystemTime::UNIX_EPOCH);
        let names: Vec<&str> = scan.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, ["a.RAF", "b.RAF"]);
    }

    #[test]
    fn a_member_whose_file_has_vanished_is_still_listed() {
        // ⚠ A missing file must not silently disappear from the collection you put it in — an
        // unplugged drive would otherwise look like the collection had emptied itself. It is
        // listed; opening it will fail honestly.
        let members = vec![(PathBuf::from("/gone/x.RAF"), 0)];
        let scan = scan_of(&members, |_| None, |_| std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(scan.len(), 1);
        assert_eq!(scan[0].file_name, "x.RAF");
    }

    #[test]
    fn the_edited_time_is_supplied_by_the_caller() {
        // ⚠ Same layering as `scan_folder`: "last edited" means the sidecar's mtime, and the
        // sidecar naming convention belongs to `app`, not to `catalog`.
        let members = vec![(PathBuf::from("/x/a.RAF"), 0)];
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(42);
        let scan = scan_of(
            &members,
            |_| Some(when),
            |_| std::time::SystemTime::UNIX_EPOCH,
        );
        assert_eq!(scan[0].edited, Some(when));
    }

    #[test]
    fn the_modification_time_is_supplied_by_the_caller() {
        // ⚠ Same layering as `edited_at`, and for a sharper reason: All Photos must not walk the
        // filesystem. `scan_of` used to stat every member, which is 10,000 stats on a view that
        // otherwise touches no files — while the catalog has stored `mtime` since M4.1 and
        // nothing had ever read it.
        let members = vec![(PathBuf::from("/x/a.RAF"), 0)];
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(99);
        let scan = scan_of(&members, |_| None, |_| when);
        assert_eq!(scan[0].modified, when);
    }
}

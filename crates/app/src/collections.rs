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
use std::collections::HashSet;
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
    // ⚠ Through `unique::first_of_each` — a `Vec` searched per member made this quadratic: 179 s for
    // All Photos at 100,000 photos (M10.14e, `catalog_scale`).
    let mut out = Vec::new();
    for path in crate::unique::first_of_each(members.iter().map(|(p, _)| p.as_path())) {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // ⚠ A vanished file is still listed. Dropping it would make an unplugged drive look like
        // the collection had emptied itself.
        let modified = modified_at(path);
        out.push(CatalogEntry {
            path: path.to_path_buf(),
            file_name,
            modified,
            edited: edited_at(path),
        });
    }
    out
}

/// The scan a source holds now, given the one it holds and its members as just re-read — or `None`
/// when the same files are members, so nothing needs rebuilding.
///
/// ⚠ A retained file keeps its entry as it was: only a file that has just **joined** pays for its
/// times, so re-reading a view of thousands after one photo left costs no stats at all. The order is
/// left to the caller, which sorts by the Library's order anyway.
pub fn refreshed_scan(
    old: &[CatalogEntry],
    members: &[(PathBuf, u32)],
    edited_at: impl Fn(&Path) -> Option<std::time::SystemTime>,
    modified_at: impl Fn(&Path) -> std::time::SystemTime,
) -> Option<Vec<CatalogEntry>> {
    let now: HashSet<&Path> = members.iter().map(|(p, _)| p.as_path()).collect();
    let had: HashSet<&Path> = old.iter().map(|e| e.path.as_path()).collect();
    if now == had {
        return None;
    }
    let joined: Vec<(PathBuf, u32)> = members
        .iter()
        .filter(|(p, _)| !had.contains(p.as_path()))
        .cloned()
        .collect();
    let mut scan: Vec<CatalogEntry> = old
        .iter()
        .filter(|e| now.contains(e.path.as_path()))
        .cloned()
        .collect();
    scan.extend(scan_of(&joined, edited_at, modified_at));
    Some(scan)
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

    fn entry(path: &str, secs: u64) -> CatalogEntry {
        let p = PathBuf::from(path);
        CatalogEntry {
            file_name: p.file_name().unwrap().to_string_lossy().into_owned(),
            path: p,
            modified: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            edited: None,
        }
    }

    fn members(paths: &[&str]) -> Vec<(PathBuf, u32)> {
        paths.iter().map(|p| (PathBuf::from(p), 0)).collect()
    }

    #[test]
    fn the_same_files_need_no_rebuild() {
        let old = vec![entry("/x/a.RAF", 1), entry("/x/b.RAF", 2)];
        // A second copy of a member is the same file; the order is the query's, not the scan's.
        let mut now = members(&["/x/b.RAF", "/x/a.RAF"]);
        now.push((PathBuf::from("/x/a.RAF"), 1));
        assert!(refreshed_scan(&old, &now, |_| None, |_| unreachable!()).is_none());
    }

    #[test]
    fn a_file_that_left_is_dropped_and_the_rest_keep_their_entries() {
        let old = vec![entry("/x/a.RAF", 1), entry("/x/b.RAF", 2)];
        let new = refreshed_scan(
            &old,
            &members(&["/x/a.RAF"]),
            |_| None,
            |_| panic!("a retained file was stat'ed"),
        )
        .expect("b left");
        assert_eq!(new, vec![entry("/x/a.RAF", 1)]);
    }

    #[test]
    fn a_file_that_joined_is_added_with_its_times() {
        let old = vec![entry("/x/a.RAF", 1)];
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(7);
        let stats = std::cell::Cell::new(0);
        let new = refreshed_scan(
            &old,
            &members(&["/x/a.RAF", "/y/c.RAF"]),
            |_| Some(when),
            |_| {
                stats.set(stats.get() + 1);
                when
            },
        )
        .expect("c joined");
        assert_eq!(stats.get(), 1, "only the newcomer is stat'ed");
        assert_eq!(new.len(), 2);
        assert_eq!(new[0], entry("/x/a.RAF", 1), "a is untouched");
        assert_eq!(
            (new[1].file_name.as_str(), new[1].modified),
            ("c.RAF", when)
        );
        assert_eq!(new[1].edited, Some(when));
    }
}

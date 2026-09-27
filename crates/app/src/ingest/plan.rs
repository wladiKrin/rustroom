//! Deciding, before anything is copied: which photos are already imported, and where each one goes.

use super::source::Candidate;
use crate::filename_template::{NameFacts, Template};
use catalog::db::Fingerprint;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The catalog's photos, as keys a card's photo is looked up by.
#[derive(Debug, Default)]
pub struct KnownSet {
    by_capture: HashSet<(i64, i64)>,
    by_name: HashSet<(String, i64)>,
}

impl KnownSet {
    pub fn new(prints: &[Fingerprint]) -> Self {
        let mut k = Self::default();
        for f in prints {
            if let Some(t) = f.taken {
                k.by_capture.insert((f.size, t));
            }
            k.by_name.insert((f.file_name.clone(), f.size));
        }
        k
    }

    /// Whether this card photo is already in the catalog.
    ///
    /// ⚠⚠ **Size plus capture time, not the name** — an import renames, so the name on the card
    /// says nothing about what is already home. Two different shots of exactly the same byte size
    /// in the same second are the accepted false positive. Only a photo with **no** capture time
    /// falls back to name and size, where a renamed import is missed and copied again — the safe
    /// direction for that error.
    pub fn contains(&self, c: &Candidate, taken: Option<i64>) -> bool {
        let size = c.size as i64;
        match taken {
            Some(t) => self.by_capture.contains(&(size, t)),
            None => c.path.file_name().is_some_and(|n| {
                self.by_name
                    .contains(&(n.to_string_lossy().into_owned(), size))
            }),
        }
    }
}

/// `YYYY/YYYY-MM-DD` for a photo taken at `taken`, else modified at `mtime`.
///
/// ⚠ `taken` is naive wall-clock seconds (the catalog's convention), so it is read as UTC to get
/// the camera's own date back — `filename_template`'s `{date}` route. `mtime` is a true instant,
/// so it is shown in this machine's zone.
pub fn day_dir(taken: Option<i64>, mtime: i64) -> PathBuf {
    use chrono::TimeZone;
    let date = match taken.and_then(|t| chrono::DateTime::from_timestamp(t, 0)) {
        Some(d) => d.date_naive(),
        None => chrono::Local
            .timestamp_opt(mtime, 0)
            .single()
            .map_or_else(chrono::NaiveDate::default, |d| d.date_naive()),
    };
    PathBuf::from(date.format("%Y").to_string()).join(date.format("%Y-%m-%d").to_string())
}

/// Where `c` lands under `root`: its day folder, the template's name, the source's extension kept
/// verbatim — with `-N` added while the path exists on disk **or** is already `planned` by an
/// earlier photo of the same import. Nothing is ever overwritten.
pub fn destination(
    root: &Path,
    c: &Candidate,
    taken: Option<i64>,
    camera: Option<&str>,
    template: &Template,
    seq: usize,
    planned: &[PathBuf],
) -> PathBuf {
    let stem = c
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = template.render(&NameFacts {
        file_stem: &stem,
        copy: 0,
        copy_label: None,
        seq,
        taken,
        camera,
    });
    let ext = c.path.extension().map(|e| e.to_string_lossy().into_owned());
    let dir = root.join(day_dir(taken, c.mtime));
    let named = |n: usize| {
        let b = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        dir.join(match &ext {
            Some(e) => format!("{b}.{e}"),
            None => b,
        })
    };
    (0..)
        .map(named)
        .find(|p| !p.exists() && !planned.contains(p))
        .expect("an unbounded search always ends")
}

/// The deepest folder holding every one of `files` — what to open after an import: the day folder
/// when everything landed on one day, their year or the destination otherwise.
pub fn common_dir(files: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs = files.iter().filter_map(|f| f.parent());
    let mut common = dirs.next()?.to_path_buf();
    for d in dirs {
        while !d.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    Some(common)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::temp_dir;

    /// 2026-09-27 12:00:00, as naive wall-clock seconds — and noon, so reading it as local time in
    /// any zone within ±12 h still lands on the 27th.
    const NOON: i64 = 1_790_510_400;

    fn cand(name: &str, size: u64) -> Candidate {
        Candidate {
            path: PathBuf::from("/card/DCIM/100").join(name),
            size,
            mtime: 0,
        }
    }

    fn fp(name: &str, size: i64, taken: Option<i64>) -> Fingerprint {
        Fingerprint {
            file_name: name.into(),
            size,
            taken,
        }
    }

    #[test]
    fn a_photo_matches_on_size_and_capture_time_or_else_name_and_size() {
        let known = KnownSet::new(&[fp("RENAMED.RAF", 1000, Some(42)), fp("B.JPG", 2000, None)]);
        assert!(
            known.contains(&cand("A.RAF", 1000), Some(42)),
            "renamed on import, same shot"
        );
        assert!(
            !known.contains(&cand("A.RAF", 1000), Some(43)),
            "another shot of the same size"
        );
        assert!(known.contains(&cand("B.JPG", 2000), None));
        assert!(
            !known.contains(&cand("C.JPG", 2000), None),
            "with no capture time the name must match"
        );
    }

    #[test]
    fn the_day_folder_comes_from_the_capture_time_else_the_mtime() {
        assert_eq!(day_dir(Some(NOON), 0), PathBuf::from("2026/2026-09-27"));
        assert_eq!(day_dir(None, NOON), PathBuf::from("2026/2026-09-27"));
    }

    #[test]
    fn the_destination_keeps_the_extension_and_never_overwrites() {
        let root = temp_dir("dest");
        let tpl = Template::parse("{date}_{seq:4}").unwrap();
        let c = cand("DSCF0001.RAF", 5);
        let t = Some(NOON);
        let a = destination(&root, &c, t, None, &tpl, 1, &[]);
        assert_eq!(a, root.join("2026/2026-09-27/2026-09-27_0001.RAF"));
        // Planned by an earlier photo of the same import → -1, before anything is written.
        let b = destination(&root, &c, t, None, &tpl, 1, std::slice::from_ref(&a));
        assert_eq!(b, root.join("2026/2026-09-27/2026-09-27_0001-1.RAF"));
        // And a file already on disk is never overwritten.
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, b"x").unwrap();
        let c2 = destination(&root, &c, t, None, &tpl, 1, &[]);
        assert_eq!(c2, root.join("2026/2026-09-27/2026-09-27_0001-1.RAF"));
    }

    #[test]
    fn the_folder_to_open_is_the_deepest_one_holding_every_import() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(common_dir(&[]), None);
        assert_eq!(
            common_dir(&[
                p("/ph/2026/2026-09-27/A.RAF"),
                p("/ph/2026/2026-09-27/B.RAF")
            ]),
            Some(p("/ph/2026/2026-09-27"))
        );
        assert_eq!(
            common_dir(&[
                p("/ph/2026/2026-09-27/A.RAF"),
                p("/ph/2026/2026-09-28/B.RAF")
            ]),
            Some(p("/ph/2026"))
        );
        assert_eq!(
            common_dir(&[
                p("/ph/2025/2025-12-31/A.RAF"),
                p("/ph/2026/2026-01-01/B.RAF")
            ]),
            Some(p("/ph"))
        );
    }

    #[test]
    fn the_default_template_keeps_the_cards_name() {
        let root = temp_dir("keep");
        let tpl = Template::parse("{name}").unwrap();
        let d = destination(
            &root,
            &cand("DSCF0001.RAF", 5),
            Some(NOON),
            None,
            &tpl,
            1,
            &[],
        );
        assert_eq!(d, root.join("2026/2026-09-27/DSCF0001.RAF"));
    }
}

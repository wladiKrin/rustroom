//! Batch rename (M10.8d): the new names of a run of photos, from a filename template.
//!
//! ⚠ A lib module over plain values — no egui, no filesystem, no catalog. Whether a name is taken
//! is the caller's question (`taken`); the moving is `photo_move::move_all`'s.
//!
//! ⚠⚠ **One photo is the files sharing a folder and a stem** — a RAW+JPEG pair is one picture to
//! the eye and its stack is found by that stem. It takes ONE `{seq}` number and ONE new stem, and
//! each file keeps its own extension.

use crate::filename_template::{NameFacts, Template};
use crate::photo_move::Planned;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One photo to rename: its files (one folder, one stem) and what its name may say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Photo {
    pub files: Vec<PathBuf>,
    /// Capture time, naive wall-clock seconds — `{date}`.
    pub taken: Option<i64>,
    /// `"{MAKE} {model}"` — `{camera}`.
    pub camera: Option<String>,
}

/// `files` grouped into photos — by folder and stem, in the order each photo first appears.
pub fn photos_of(files: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    let mut keys: Vec<(Option<&Path>, String)> = Vec::new();
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for f in files {
        let key = (f.parent(), stem(f));
        match keys.iter().position(|k| *k == key) {
            Some(i) => {
                if !groups[i].contains(f) {
                    groups[i].push(f.clone());
                }
            }
            None => {
                keys.push(key);
                groups.push(vec![f.clone()]);
            }
        }
    }
    groups
}

/// Every file's move, for `photos` named by `template` with `{seq}` counting from `start` in order.
///
/// ⚠⚠ **Never onto a name that stays taken.** A name held by a file outside the rename (`taken`)
/// gets `-1`, `-2` — for the whole photo, Move to folder's rule. But **the renamed photos do not
/// block one another**: their current names count as free, because they are leaving — that is what
/// lets a run be renumbered or two names swapped (`move_all` makes it safe on disk).
///
/// ⚠ A photo whose name would not change **stays**, out of the plan, and its name stays reserved —
/// handing it to another photo would collide with a file that is not moving.
pub fn plan(
    photos: &[Photo],
    template: &Template,
    start: usize,
    taken: impl Fn(&Path) -> bool,
) -> Vec<Planned> {
    let bases: Vec<String> = photos
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let file_stem = p.files.first().map(|f| stem(f)).unwrap_or_default();
            template.render(&NameFacts {
                file_stem: &file_stem,
                copy: 0,
                copy_label: None,
                seq: start + k,
                taken: p.taken,
                camera: p.camera.as_deref(),
            })
        })
        .collect();
    let staying: Vec<bool> = photos
        .iter()
        .zip(&bases)
        .map(|(p, b)| named(p, b, 0) == p.files)
        .collect();
    let mut reserved: HashSet<PathBuf> = HashSet::new();
    let mut moving: HashSet<&Path> = HashSet::new();
    for (p, stays) in photos.iter().zip(&staying) {
        for f in &p.files {
            if *stays {
                reserved.insert(f.clone());
            } else {
                moving.insert(f.as_path());
            }
        }
    }
    let mut out = Vec::new();
    for (k, p) in photos.iter().enumerate() {
        if staying[k] || p.files.is_empty() {
            continue;
        }
        let free =
            |t: &PathBuf| !reserved.contains(t) && (moving.contains(t.as_path()) || !taken(t));
        let n = (0..)
            .find(|&n| named(p, &bases[k], n).iter().all(free))
            .expect("an unbounded search always ends");
        for (from, to) in p.files.iter().zip(named(p, &bases[k], n)) {
            reserved.insert(to.clone());
            if *from != to {
                out.push(Planned {
                    from: from.clone(),
                    to,
                });
            }
        }
    }
    out
}

/// The Rename dialog's preview of `plan`: the first `rows` photos as `old → new` of each photo's first
/// file, and how many more photos it renames.
pub fn preview(plan: &[Planned], rows: usize) -> (Vec<(String, String)>, usize) {
    let froms: Vec<PathBuf> = plan.iter().map(|p| p.from.clone()).collect();
    let photos = photos_of(&froms);
    let own = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let lines = photos
        .iter()
        .take(rows)
        .filter_map(|files| {
            let first = files.first()?;
            let to = &plan.iter().find(|p| p.from == *first)?.to;
            Some((own(first), own(to)))
        })
        .collect();
    (lines, photos.len().saturating_sub(rows))
}

/// A photo's files under the stem `base` (with `-n` when `n > 0`), each keeping its extension.
fn named(p: &Photo, base: &str, n: usize) -> Vec<PathBuf> {
    let b = if n == 0 {
        base.to_string()
    } else {
        format!("{base}-{n}")
    };
    p.files
        .iter()
        .map(|f| match f.extension() {
            Some(e) => f.with_file_name(format!("{b}.{}", e.to_string_lossy())),
            None => f.with_file_name(&b),
        })
        .collect()
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(files: &[&str]) -> Photo {
        Photo {
            files: files.iter().map(PathBuf::from).collect(),
            ..Photo::default()
        }
    }

    fn t(src: &str) -> Template {
        Template::parse_for_rename(src).unwrap()
    }

    /// `from → to` by file name, for readable assertions.
    fn moves(planned: &[Planned]) -> Vec<(String, String)> {
        let own = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        planned.iter().map(|p| (own(&p.from), own(&p.to))).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn photos_group_by_folder_and_stem_in_order() {
        let files: Vec<PathBuf> = ["/a/x.RAF", "/a/y.JPG", "/a/x.JPG", "/b/x.JPG", "/a/x.RAF"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let got = photos_of(&files);
        assert_eq!(
            got,
            vec![
                vec![PathBuf::from("/a/x.RAF"), PathBuf::from("/a/x.JPG")],
                vec![PathBuf::from("/a/y.JPG")],
                vec![PathBuf::from("/b/x.JPG")],
            ]
        );
    }

    #[test]
    fn seq_counts_in_order_from_the_start() {
        let photos = [photo(&["/a/p.JPG"]), photo(&["/a/q.JPG"])];
        let got = plan(&photos, &t("Trip-{seq:3}"), 7, |_| false);
        assert_eq!(
            moves(&got),
            pairs(&[("p.JPG", "Trip-007.JPG"), ("q.JPG", "Trip-008.JPG")])
        );
    }

    /// ⚠⚠ A RAW+JPEG pair is one photo: one number, both extensions kept.
    #[test]
    fn a_pair_takes_one_number_and_keeps_both_extensions() {
        let photos = [photo(&["/a/x.RAF", "/a/x.JPG"]), photo(&["/a/y.JPG"])];
        let got = plan(&photos, &t("Trip-{seq}"), 1, |_| false);
        assert_eq!(
            moves(&got),
            pairs(&[
                ("x.RAF", "Trip-1.RAF"),
                ("x.JPG", "Trip-1.JPG"),
                ("y.JPG", "Trip-2.JPG"),
            ])
        );
    }

    /// A name held outside the rename gets `-1` — for the whole pair, though only the RAW's is taken.
    #[test]
    fn a_name_taken_outside_suffixes_the_whole_photo() {
        let photos = [photo(&["/a/x.RAF", "/a/x.JPG"])];
        let got = plan(&photos, &t("Trip"), 1, |p| p == Path::new("/a/Trip.RAF"));
        assert_eq!(
            moves(&got),
            pairs(&[("x.RAF", "Trip-1.RAF"), ("x.JPG", "Trip-1.JPG")])
        );
    }

    /// ⚠⚠ The renamed photos do not block one another: a swap plans with no suffix, though both
    /// names exist on disk.
    #[test]
    fn a_swap_plans_without_suffixes() {
        let photos = [photo(&["/a/B-1.JPG"]), photo(&["/a/B-2.JPG"])];
        // Reversed order: what is called B-1 becomes B-2 and the other way round.
        let rev = [photos[1].clone(), photos[0].clone()];
        let on_disk = |p: &Path| p == Path::new("/a/B-1.JPG") || p == Path::new("/a/B-2.JPG");
        let got = plan(&rev, &t("B-{seq}"), 1, on_disk);
        assert_eq!(
            moves(&got),
            pairs(&[("B-2.JPG", "B-1.JPG"), ("B-1.JPG", "B-2.JPG")])
        );
    }

    /// A shift up by one: 001 → 002 while 002 → 003.
    #[test]
    fn a_shift_plans_without_suffixes() {
        let photos = [photo(&["/a/S-001.JPG"]), photo(&["/a/S-002.JPG"])];
        let on_disk = |p: &Path| p.ends_with("S-001.JPG") || p.ends_with("S-002.JPG");
        let got = plan(&photos, &t("S-{seq:3}"), 2, on_disk);
        assert_eq!(
            moves(&got),
            pairs(&[("S-001.JPG", "S-002.JPG"), ("S-002.JPG", "S-003.JPG")])
        );
    }

    /// ⚠ A photo whose name would not change stays out of the plan, and its name is not handed to
    /// another photo.
    #[test]
    fn an_unchanged_photo_stays_and_keeps_its_name() {
        // Day.JPG is already called Day, so it stays; z.JPG wants the same name and gets Day-1.
        let photos = [photo(&["/a/Day.JPG"]), photo(&["/a/z.JPG"])];
        let got = plan(&photos, &t("Day"), 1, |p| p == Path::new("/a/Day.JPG"));
        assert_eq!(
            moves(&got),
            pairs(&[("z.JPG", "Day-1.JPG")]),
            "Day stays, and z does not take its name"
        );
    }

    /// The preview names each photo once, by its first file, and counts the rest as photos.
    #[test]
    fn the_preview_shows_the_first_photos_and_counts_the_rest() {
        let photos = [
            photo(&["/a/x.RAF", "/a/x.JPG"]),
            photo(&["/a/y.JPG"]),
            photo(&["/a/z.JPG"]),
        ];
        let planned = plan(&photos, &t("T-{seq}"), 1, |_| false);
        let (lines, more) = preview(&planned, 2);
        assert_eq!(lines, pairs(&[("x.RAF", "T-1.RAF"), ("y.JPG", "T-2.JPG")]));
        assert_eq!(more, 1, "z, as one photo");
        assert_eq!(preview(&planned, 3).1, 0);
    }

    /// An absent `{date}` takes its separator with it.
    #[test]
    fn an_absent_date_drops_its_separator() {
        let got = plan(&[photo(&["/a/x.JPG"])], &t("{date}-{seq:3}"), 1, |_| false);
        assert_eq!(moves(&got), pairs(&[("x.JPG", "001.JPG")]));
        let dated = Photo {
            taken: Some(1_696_204_800), // 2023-10-02 00:00
            ..photo(&["/a/x.JPG"])
        };
        let got = plan(&[dated], &t("{date}-{seq:3}"), 1, |_| false);
        assert_eq!(moves(&got), pairs(&[("x.JPG", "2023-10-02-001.JPG")]));
    }
}

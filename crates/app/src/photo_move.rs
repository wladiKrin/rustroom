//! Moving photos to another folder (2026-10-01): where each one lands, and the files that go with it.
//!
//! ⚠ No egui and no catalog — the app's `move_flow` asks the catalog what is taken and makes it follow;
//! this decides names and moves bytes, and is tested on temporary directories.
//!
//! ⚠⚠ **Nothing is ever overwritten.** A name that is taken gets `-1`, `-2` (Import's rule), and the
//! move itself refuses a destination that has appeared since the plan.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

/// One file's move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Where each of `files` lands in `dest`.
///
/// `name_of` gives the name a file wants (its own, or an Immich download's original); `taken` says a
/// destination is occupied — on disk, by a sidecar, or by a catalog row of a vanished file.
///
/// ⚠⚠ **Files sharing a folder and a stem keep sharing one** — a RAW+JPEG pair is one photo to the
/// eye, and its stack is found by that stem. The suffix is chosen for the whole group, so a collision
/// on the RAW's name renames the JPEG too.
///
/// ⚠ A file already in `dest` is left out: moving it there would at best do nothing, and at worst
/// rename it against itself.
pub fn plan(
    files: &[PathBuf],
    dest: &Path,
    name_of: impl Fn(&Path) -> String,
    taken: impl Fn(&Path) -> bool,
) -> Vec<Planned> {
    let mut groups: Vec<((PathBuf, String), Vec<PathBuf>)> = Vec::new();
    let mut seen: HashSet<&Path> = HashSet::new();
    for f in files {
        if !seen.insert(f.as_path()) || f.parent() == Some(dest) {
            continue;
        }
        let key = (
            f.parent().map(Path::to_path_buf).unwrap_or_default(),
            stem(f),
        );
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(f.clone()),
            None => groups.push((key, vec![f.clone()])),
        }
    }
    let mut planned: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for (_, members) in groups {
        let wanted: Vec<(PathBuf, String)> =
            members.iter().map(|m| (m.clone(), name_of(m))).collect();
        let base = stem(Path::new(&wanted[0].1));
        let named = |n: usize, want: &str| {
            let b = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            match Path::new(want).extension() {
                Some(e) => dest.join(format!("{b}.{}", e.to_string_lossy())),
                None => dest.join(b),
            }
        };
        let n = (0..)
            .find(|&n| {
                wanted.iter().all(|(_, w)| {
                    let p = named(n, w);
                    !taken(&p) && !planned.contains(&p)
                })
            })
            .expect("an unbounded search always ends");
        for (from, w) in wanted {
            let to = named(n, &w);
            planned.insert(to.clone());
            out.push(Planned { from, to });
        }
    }
    out
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Move one file, its JSON sidecar and — for a RAW — its XMP sidecar.
pub fn move_file(p: &Planned) -> io::Result<()> {
    move_file_with(p, |a, b| std::fs::rename(a, b))
}

/// [`move_file`] with the rename injected, so a test can make it fail across volumes.
///
/// ⚠⚠ **The sidecars go with the photo or the photo goes back.** A photo moved without its JSON
/// sidecar opens neutral, its edits orphaned beside the old name — so a sidecar that will not move
/// undoes the photo's move and reports the error.
///
/// ⚠ The XMP only for a RAW (`xmp_sync::takes_sidecar`): a RAW+JPEG pair shares `stem.xmp`, and it is
/// the RAW's.
pub fn move_file_with(
    p: &Planned,
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(dir) = p.to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    relocate(&p.from, &p.to, &rename)?;
    let mut sidecars = vec![(crate::sidecar::path(&p.from), crate::sidecar::path(&p.to))];
    if crate::xmp_sync::takes_sidecar(&p.from) {
        sidecars.push((
            crate::xmp_sync::sidecar_path(&p.from),
            crate::xmp_sync::sidecar_path(&p.to),
        ));
    }
    for (a, b) in sidecars {
        if !a.exists() {
            continue;
        }
        if let Err(e) = relocate(&a, &b, &rename) {
            let _ = relocate(&p.to, &p.from, &rename);
            return Err(e);
        }
    }
    Ok(())
}

/// Rename `from` to `to`; across volumes, copy (size-checked, `ingest::copy`) and then delete.
fn relocate(
    from: &Path,
    to: &Path,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    // ⚠ `rename` replaces an existing file on Unix — refused here, whatever the plan said.
    if to.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    match rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            crate::ingest::copy::copy_verified(from, to)?;
            std::fs::remove_file(from)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn names(planned: &[Planned]) -> Vec<String> {
        planned.iter().map(|p| own(&p.to)).collect()
    }

    #[test]
    fn names_are_kept_and_a_collision_gets_a_suffix() {
        let files = vec![PathBuf::from("/a/x.RAF"), PathBuf::from("/a/y.RAF")];
        let dest = Path::new("/b");
        let planned = plan(&files, dest, own, |p| p == Path::new("/b/y.RAF"));
        assert_eq!(names(&planned), ["x.RAF", "y-1.RAF"]);
        assert_eq!(planned[0].from, files[0]);
    }

    /// ⚠⚠ A pair keeps one stem: only the RAW's name is taken, and the JPEG moves to `-1` with it.
    #[test]
    fn a_pair_keeps_one_stem() {
        let files = vec![PathBuf::from("/a/x.RAF"), PathBuf::from("/a/x.JPG")];
        let planned = plan(&files, Path::new("/b"), own, |p| p == Path::new("/b/x.RAF"));
        assert_eq!(names(&planned), ["x-1.RAF", "x-1.JPG"]);
    }

    #[test]
    fn the_same_folder_plans_nothing_and_a_duplicate_is_one_file() {
        let files = vec![
            PathBuf::from("/b/x.RAF"),
            PathBuf::from("/a/y.RAF"),
            PathBuf::from("/a/y.RAF"),
        ];
        let planned = plan(&files, Path::new("/b"), own, |_| false);
        assert_eq!(names(&planned), ["y.RAF"]);
    }

    /// An Immich download wants its original name; two that want the same one do not collide.
    #[test]
    fn a_wanted_name_is_used_and_two_wanting_one_name_are_told_apart() {
        let files = vec![
            PathBuf::from("/cache/3f2a.HEIC"),
            PathBuf::from("/cache/9c1b.HEIC"),
        ];
        let planned = plan(
            &files,
            Path::new("/b"),
            |_| "IMG_1.HEIC".to_string(),
            |_| false,
        );
        assert_eq!(names(&planned), ["IMG_1.HEIC", "IMG_1-1.HEIC"]);
    }

    /// A real directory: `a/x.RAF` with its JSON sidecar and its XMP, and `a/x.JPG` beside it.
    fn pair(tag: &str) -> (test_dir::TempDir, PathBuf, PathBuf) {
        let d = test_dir::TempDir::new(&format!("photo_move-{tag}"));
        std::fs::create_dir_all(d.join("a")).unwrap();
        let raw = d.join("a/x.RAF");
        let jpg = d.join("a/x.JPG");
        for f in [&raw, &jpg] {
            std::fs::write(f, b"bytes").unwrap();
        }
        std::fs::write(crate::sidecar::path(&raw), b"{}").unwrap();
        std::fs::write(crate::xmp_sync::sidecar_path(&raw), b"<xmp/>").unwrap();
        (d, raw, jpg)
    }

    #[test]
    fn a_raw_moves_with_its_json_and_its_xmp() {
        let (d, raw, _jpg) = pair("raw");
        let to = d.join("b/y.RAF");
        move_file(&Planned {
            from: raw.clone(),
            to: to.clone(),
        })
        .unwrap();
        assert!(to.exists() && !raw.exists());
        assert!(crate::sidecar::path(&to).exists(), "the JSON sidecar");
        assert!(crate::xmp_sync::sidecar_path(&to).exists(), "the XMP");
        assert!(
            !crate::sidecar::path(&raw).exists() && !crate::xmp_sync::sidecar_path(&raw).exists()
        );
    }

    /// The XMP is the RAW's: a JPEG of the same stem moving alone leaves it.
    #[test]
    fn a_jpeg_leaves_the_raws_xmp() {
        let (d, raw, jpg) = pair("jpeg");
        move_file(&Planned {
            from: jpg,
            to: d.join("b/x.JPG"),
        })
        .unwrap();
        assert!(crate::xmp_sync::sidecar_path(&raw).exists());
    }

    /// Across volumes `rename` fails; the file is copied and then deleted, its sidecars too.
    #[test]
    fn across_volumes_it_copies_and_deletes() {
        let (d, raw, _jpg) = pair("xdev");
        let to = d.join("b/x.RAF");
        let fails = |_: &Path, _: &Path| -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::CrossesDevices))
        };
        move_file_with(
            &Planned {
                from: raw.clone(),
                to: to.clone(),
            },
            fails,
        )
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"bytes");
        assert!(!raw.exists(), "the original is deleted after the copy");
        assert!(crate::sidecar::path(&to).exists() && !crate::sidecar::path(&raw).exists());
    }

    /// Never overwrite — a destination that appeared after the plan is refused.
    #[test]
    fn an_existing_destination_is_refused() {
        let (d, raw, _jpg) = pair("refuse");
        let to = d.join("a/x.JPG");
        let e = move_file(&Planned {
            from: raw.clone(),
            to: to.clone(),
        })
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(raw.exists() && std::fs::read(&to).unwrap() == b"bytes");
    }
}

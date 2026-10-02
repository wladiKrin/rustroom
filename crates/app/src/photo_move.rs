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

/// Move every plan's file with its sidecars, and report each one's result, in order (M10.8d).
///
/// ⚠⚠ **Names may chain.** A rename that swaps two names, or shifts a numbered run up by one, moves a
/// file onto a name another plan's file still holds — which [`move_file`] rightly refuses. Then every
/// file goes to a temporary name in its own folder first (`.rustroom-move-<i>.<ext>`: the extension
/// kept, so a RAW still takes its XMP) and only then to its destination.
///
/// ⚠ A file whose second step fails is put back under its own name; if even that fails, the error
/// says where it is. A file whose first step fails never left.
pub fn move_all(plans: &[Planned]) -> Vec<io::Result<()>> {
    move_all_with(plans, |a, b| std::fs::rename(a, b))
}

/// [`move_all`] with the rename injected, so a test can make one step fail.
pub fn move_all_with(
    plans: &[Planned],
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Vec<io::Result<()>> {
    let chained = plans.iter().any(|p| plans.iter().any(|q| q.from == p.to));
    if !chained {
        return plans.iter().map(|p| move_file_with(p, &rename)).collect();
    }
    let temps: Vec<Planned> = plans
        .iter()
        .enumerate()
        .map(|(i, p)| Planned {
            from: p.from.clone(),
            to: temp_for(&p.from, i),
        })
        .collect();
    let mut out: Vec<io::Result<()>> = temps.iter().map(|t| move_file_with(t, &rename)).collect();
    for (i, p) in plans.iter().enumerate() {
        if out[i].is_err() {
            continue;
        }
        let temp = &temps[i].to;
        let last = Planned {
            from: temp.clone(),
            to: p.to.clone(),
        };
        if let Err(e) = move_file_with(&last, &rename) {
            let back = Planned {
                from: temp.clone(),
                to: p.from.clone(),
            };
            out[i] = Err(match move_file_with(&back, &rename) {
                Ok(()) => e,
                Err(_) => io::Error::new(e.kind(), format!("{e} — left as {}", temp.display())),
            });
        }
    }
    out
}

/// Where a file waits between its two steps: hidden, in its own folder, with its own extension.
fn temp_for(from: &Path, i: usize) -> PathBuf {
    let name = match from.extension() {
        Some(e) => format!(".rustroom-move-{i}.{}", e.to_string_lossy()),
        None => format!(".rustroom-move-{i}"),
    };
    from.with_file_name(name)
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

    /// `a/<name>.RAF` with its own bytes, JSON and XMP, each saying `name`.
    fn raw_named(d: &Path, name: &str) -> PathBuf {
        let f = d.join(format!("a/{name}.RAF"));
        std::fs::write(&f, name).unwrap();
        std::fs::write(crate::sidecar::path(&f), name).unwrap();
        std::fs::write(crate::xmp_sync::sidecar_path(&f), name).unwrap();
        f
    }

    /// What the file, its JSON and its XMP at `f` say.
    fn says(f: &Path) -> [String; 3] {
        [
            f.to_path_buf(),
            crate::sidecar::path(f),
            crate::xmp_sync::sidecar_path(f),
        ]
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
    }

    /// ⚠⚠ M10.8d: two files swap names, each with its JSON and its XMP — the chained case a plain
    /// `move_file` per file refuses.
    #[test]
    fn two_files_swap_names_with_their_sidecars() {
        let d = test_dir::TempDir::new("photo_move-swap");
        std::fs::create_dir_all(d.join("a")).unwrap();
        let (a, b) = (raw_named(d.path(), "a"), raw_named(d.path(), "b"));
        let plans = [
            Planned {
                from: a.clone(),
                to: b.clone(),
            },
            Planned {
                from: b.clone(),
                to: a.clone(),
            },
        ];
        let out = move_all(&plans);
        assert!(out.iter().all(|r| r.is_ok()), "{out:?}");
        assert_eq!(says(&b), ["a", "a", "a"].map(String::from));
        assert_eq!(says(&a), ["b", "b", "b"].map(String::from));
        let left: Vec<_> = std::fs::read_dir(d.join("a")).unwrap().collect();
        assert_eq!(left.len(), 6, "no temporary file left behind");
    }

    /// A shifted run: a→b and b→c, where b is still held when a arrives.
    #[test]
    fn a_shifted_run_moves_up_by_one() {
        let d = test_dir::TempDir::new("photo_move-shift");
        std::fs::create_dir_all(d.join("a")).unwrap();
        let (a, b) = (raw_named(d.path(), "a"), raw_named(d.path(), "b"));
        let c = d.join("a/c.RAF");
        let out = move_all(&[
            Planned {
                from: a.clone(),
                to: b.clone(),
            },
            Planned {
                from: b.clone(),
                to: c.clone(),
            },
        ]);
        assert!(out.iter().all(|r| r.is_ok()), "{out:?}");
        assert_eq!(says(&b)[0], "a");
        assert_eq!(says(&c)[0], "b");
        assert!(!a.exists());
    }

    /// ⚠ A second step that fails puts the file back under its own name, with its sidecars — and the
    /// swap's other half, which then finds its destination taken, goes back too.
    #[test]
    fn a_failed_second_step_puts_the_file_back() {
        let d = test_dir::TempDir::new("photo_move-back");
        std::fs::create_dir_all(d.join("a")).unwrap();
        let (a, b) = (raw_named(d.path(), "a"), raw_named(d.path(), "b"));
        let b_raw = b.clone();
        // The first step from a temporary name onto b.RAF fails — once, so the way back is open;
        // everything else is a real rename.
        let failed = std::cell::Cell::new(false);
        let flaky = move |from: &Path, to: &Path| -> io::Result<()> {
            let from_temp = own(from).starts_with(".rustroom-move-");
            if from_temp && to == b_raw && !failed.replace(true) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            std::fs::rename(from, to)
        };
        let out = move_all_with(
            &[
                Planned {
                    from: a.clone(),
                    to: b.clone(),
                },
                Planned {
                    from: b.clone(),
                    to: a.clone(),
                },
            ],
            flaky,
        );
        assert!(out.iter().all(|r| r.is_err()), "both failed: {out:?}");
        assert_eq!(says(&a), ["a", "a", "a"].map(String::from), "a is back");
        assert_eq!(says(&b), ["b", "b", "b"].map(String::from), "b is back");
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

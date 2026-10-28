//! Where a downloaded shot goes: the camera's own name, never over a file already there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The path in `dir` for the camera's file `name`. A stem already in `dir` (any extension — a RAW+JPEG
/// pair from an earlier session) steps to `-2`, `-3`, …; `stems` remembers this session's choices, so the
/// second half of a pair follows the first.
pub fn free_name(dir: &Path, name: &str, stems: &mut HashMap<String, String>) -> PathBuf {
    // ⚠ Only the last component, and never `.`/`..`: the name comes from the camera.
    let name = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shot".to_string());
    let p = Path::new(&name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy()));
    let ext = ext.as_deref().unwrap_or("");
    let taken = |s: &str| {
        let file = dir.join(format!("{s}{ext}"));
        file.exists() || part_of(&file).exists() || stem_in(dir, s)
    };
    let chosen = match stems.get(&stem) {
        // The pair's first half chose already; step only if this extension is somehow taken too.
        Some(s) if !dir.join(format!("{s}{ext}")).exists() => s.clone(),
        _ => {
            let mut s = stem.clone();
            let mut n = 2;
            while taken(&s) {
                s = format!("{stem}-{n}");
                n += 1;
            }
            s
        }
    };
    stems.insert(stem, chosen.clone());
    dir.join(format!("{chosen}{ext}"))
}

/// Whether any file in `dir` has the stem `stem` (or is its `.part`).
fn stem_in(dir: &Path, stem: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let n = e.file_name();
        let n = n.to_string_lossy();
        let n = n.strip_suffix(".part").unwrap_or(&n);
        Path::new(n).file_stem().is_some_and(|s| s == stem)
    })
}

/// The file a download is written to until it is complete.
pub fn part_of(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn a_new_name_is_kept() {
        let d = test_dir::TempDir::new("tether-name-new");
        let mut stems = HashMap::new();
        assert_eq!(
            free_name(d.path(), "DSCF0001.RAF", &mut stems),
            d.join("DSCF0001.RAF")
        );
    }

    #[test]
    fn a_taken_name_steps_to_dash_two_then_three() {
        let d = test_dir::TempDir::new("tether-name-taken");
        touch(&d.join("DSCF0001.RAF"));
        touch(&d.join("DSCF0001-2.JPG"));
        let mut stems = HashMap::new();
        assert_eq!(
            free_name(d.path(), "DSCF0001.RAF", &mut stems),
            d.join("DSCF0001-3.RAF"),
            "a stem taken by any extension is taken"
        );
    }

    /// Review focus 1: the second half of a pair follows the first, in both directions.
    #[test]
    fn free_name_keeps_a_raw_jpeg_pair_together() {
        let d = test_dir::TempDir::new("tether-name-pair");
        let mut stems = HashMap::new();
        let raf = free_name(d.path(), "DSCF0001.RAF", &mut stems);
        touch(&raf);
        assert_eq!(
            free_name(d.path(), "DSCF0001.JPG", &mut stems),
            d.join("DSCF0001.JPG")
        );

        // A second session the same day: both step together.
        touch(&d.join("DSCF0001.JPG"));
        let mut stems = HashMap::new();
        let raf = free_name(d.path(), "DSCF0001.RAF", &mut stems);
        assert_eq!(raf, d.join("DSCF0001-2.RAF"));
        touch(&raf);
        assert_eq!(
            free_name(d.path(), "DSCF0001.JPG", &mut stems),
            d.join("DSCF0001-2.JPG")
        );
    }

    #[test]
    fn a_name_cannot_leave_the_folder() {
        let d = test_dir::TempDir::new("tether-name-escape");
        let mut stems = HashMap::new();
        let p = free_name(d.path(), "../../etc/A.JPG", &mut stems);
        assert_eq!(p, d.join("A.JPG"));
        let p = free_name(d.path(), "..", &mut stems);
        assert_eq!(p.parent(), Some(d.path()), "{p:?}");
    }

    #[test]
    fn a_leftover_part_file_takes_its_name() {
        let d = test_dir::TempDir::new("tether-name-part");
        touch(&part_of(&d.join("A.JPG")));
        let mut stems = HashMap::new();
        assert_eq!(free_name(d.path(), "A.JPG", &mut stems), d.join("A-2.JPG"));
    }
}

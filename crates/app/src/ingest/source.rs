//! Where an import reads from: cards — mounted volumes holding a `DCIM` folder — and the photos on
//! them.
//!
//! ⚠ Read-only by construction: nothing here opens a file for writing.

use std::path::{Path, PathBuf};

/// A detected memory card.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// The volume's name, as the file manager shows it.
    pub name: String,
    pub root: PathBuf,
}

/// One photo on the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    /// Modified time, seconds since the epoch — the dated folder's fallback when the file carries
    /// no capture time.
    pub mtime: i64,
}

/// Where removable volumes mount on this platform. Injected into [`cards`], so tests use a temp
/// tree rather than the machine's real mounts.
pub fn default_roots() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Volumes")]
    } else if cfg!(windows) {
        ('D'..='Z')
            .map(|c| PathBuf::from(format!("{c}:\\")))
            .filter(|p| p.exists())
            .collect()
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        vec![
            PathBuf::from(format!("/media/{user}")),
            PathBuf::from(format!("/run/media/{user}")),
        ]
    }
}

/// Every volume under `roots` holding a `DCIM` directory (any case — a FAT card may say `dcim`),
/// sorted by name. On Windows a root *is* a volume, so each root is checked itself as well as its
/// children.
pub fn cards(roots: &[PathBuf]) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    for root in roots {
        let mut vols = vec![root.clone()];
        if let Ok(rd) = std::fs::read_dir(root) {
            vols.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for vol in vols {
            if has_dcim(&vol) && !out.iter().any(|c| c.root == vol) {
                let name = vol.file_name().map_or_else(
                    || vol.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                out.push(Card { name, root: vol });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn has_dcim(vol: &Path) -> bool {
    std::fs::read_dir(vol).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            e.file_name().to_string_lossy().eq_ignore_ascii_case("DCIM") && e.path().is_dir()
        })
    })
}

/// Every supported photo under `dir`, sorted by path.
///
/// ⚠ Hidden entries are skipped — a leading `.` covers macOS's `._` AppleDouble twins (which
/// carry a photo's extension and would otherwise be imported as broken files), `.Trashes` and
/// `.Spotlight-V100`.
pub fn scan(dir: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(dir: &Path, out: &mut Vec<Candidate>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&p, out);
        } else if catalog::is_supported_file(&p) {
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            out.push(Candidate {
                path: p,
                size: md.len(),
                mtime,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::temp_dir;

    #[test]
    fn a_volume_with_dcim_is_a_card_and_one_without_is_not() {
        let root = temp_dir("cards");
        std::fs::create_dir_all(root.join("SD_CARD/DCIM/100MSDCF")).unwrap();
        std::fs::create_dir_all(root.join("Backup Disk/Photos")).unwrap();
        std::fs::create_dir_all(root.join("X/dcim")).unwrap(); // lower case on a FAT card
        let found = cards(std::slice::from_ref(&root));
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["SD_CARD", "X"]);
        assert_eq!(found[0].root, root.join("SD_CARD"));
        assert!(
            cards(&[root.join("nope")]).is_empty(),
            "a missing root is no card"
        );
    }

    #[test]
    fn scan_finds_supported_files_recursively_and_skips_hidden_ones() {
        let d = temp_dir("scan");
        std::fs::create_dir_all(d.join("DCIM/100/.hidden")).unwrap();
        std::fs::create_dir_all(d.join("DCIM/101")).unwrap();
        std::fs::write(d.join("DCIM/100/A.RAF"), b"raf").unwrap();
        std::fs::write(d.join("DCIM/101/B.JPG"), b"jpeg!").unwrap();
        std::fs::write(d.join("DCIM/100/._A.RAF"), b"x").unwrap();
        std::fs::write(d.join("DCIM/100/NOTES.TXT"), b"x").unwrap();
        std::fs::write(d.join("DCIM/100/.hidden/C.RAF"), b"x").unwrap();
        let found = scan(&d);
        let names: Vec<String> = found
            .iter()
            .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["A.RAF", "B.JPG"]);
        assert_eq!(found[0].size, 3);
        assert!(found[0].mtime > 0);
    }
}

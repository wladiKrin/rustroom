//! What an import compares a card against (M9.12): every catalogued photo's size and capture time.
//!
//! ⚠ Its own file for `history`'s reason — `db.rs` is the crate's largest. A child module, so it
//! reaches the private connection.

use super::{Catalog, CatalogError};

/// One catalogued photo, as an import sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub file_name: String,
    pub size: i64,
    /// `None` until the metadata scan (or an import, which records it at once) has read a capture.
    pub taken: Option<i64>,
}

impl Catalog {
    /// Every original photo's file name, size and capture time. A row whose size could not be read
    /// is left out: it can match nothing.
    pub fn import_fingerprints(&self) -> Result<Vec<Fingerprint>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name, p.size, c.taken
             FROM photos p
             LEFT JOIN photo_capture c
               ON c.folder_id = p.folder_id AND c.file_name = p.file_name
             WHERE p.copy = 0 AND p.size IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Fingerprint {
                file_name: r.get(0)?,
                size: r.get(1)?,
                taken: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CatalogEntry;
    use core_image::Capture;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("imported-{tag}"))
    }

    fn entry(dir: &std::path::Path, name: &str, bytes: usize) -> CatalogEntry {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        CatalogEntry {
            path,
            file_name: name.to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        }
    }

    #[test]
    fn fingerprints_carry_size_and_capture_time() {
        let dir = temp_dir("fp");
        let cat = Catalog::open_in_memory().unwrap();
        let entries = [entry(&dir, "A.RAF", 1000), entry(&dir, "B.JPG", 2000)];
        cat.sync_folder(&dir, &entries).unwrap();
        cat.set_capture(
            &dir.join("A.RAF"),
            &Capture {
                taken: Some(42),
                ..Default::default()
            },
        )
        .unwrap();
        let mut f = cat.import_fingerprints().unwrap();
        f.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        assert_eq!(
            f,
            [
                Fingerprint {
                    file_name: "A.RAF".into(),
                    size: 1000,
                    taken: Some(42)
                },
                Fingerprint {
                    file_name: "B.JPG".into(),
                    size: 2000,
                    taken: None
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

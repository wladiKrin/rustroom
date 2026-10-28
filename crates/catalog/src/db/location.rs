//! Where each photo was taken (M10.18b): one row per source in `photo_location`.
//!
//! ⚠⚠ **The winner is chosen in one place, [`WINNER`]**: `hand` beats `file` beats `server`. A `hand` row
//! with NULL lat/lon is "removed" — it hides the file's and the server's — and a `file` row with NULLs is
//! "read, none", which keeps the startup pass from reading that file again.
//!
//! ⚠ Rows are keyed by FILE: a virtual copy shares its original's location.

use super::{split, Catalog, CatalogError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the user set for a photo by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hand {
    /// Nothing: the file's or the server's location shows.
    Unset,
    /// Removed: no location shows, whatever the file says.
    Removed,
    At(f64, f64),
}

/// The winning row's lat/lon for one `(folder_id, file_name)` — NULL for "removed" or none.
const WINNER: &str = "SELECT lat, lon FROM photo_location
     WHERE folder_id = (SELECT id FROM folders WHERE path = ?1) AND file_name = ?2
       AND (source = 'hand' OR lat IS NOT NULL)
     ORDER BY CASE source WHEN 'hand' THEN 0 WHEN 'file' THEN 1 ELSE 2 END
     LIMIT 1";

const UPSERT: &str = "INSERT INTO photo_location (folder_id, file_name, source, lat, lon)
     SELECT id, ?2, ?3, ?4, ?5 FROM folders WHERE path = ?1
     ON CONFLICT(folder_id, file_name, source) DO UPDATE SET lat = excluded.lat, lon = excluded.lon";

impl Catalog {
    /// Where `photo` was taken, by the winning source; `None` when unknown or removed.
    pub fn location_of(&self, photo: &Path) -> Result<Option<(f64, f64)>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let row = self
            .conn
            .query_row(WINNER, rusqlite::params![folder, name], |r| {
                Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?))
            });
        match row {
            Ok((Some(la), Some(lo))) => Ok(Some((la, lo))),
            Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// [`Catalog::location_of`] for many photos at once — only the located ones are in the answer.
    pub fn locations_of(
        &self,
        photos: &[PathBuf],
    ) -> Result<HashMap<PathBuf, (f64, f64)>, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut out = HashMap::new();
        {
            let mut st = tx.prepare_cached(WINNER)?;
            for p in photos {
                let Some((folder, name)) = split(p) else {
                    continue;
                };
                let mut rows = st.query(rusqlite::params![folder, name])?;
                if let Some(r) = rows.next()? {
                    if let (Some(la), Some(lo)) = (r.get::<_, Option<f64>>(0)?, r.get(1)?) {
                        out.insert(p.clone(), (la, lo));
                    }
                }
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// What was set for `photo` by hand.
    pub fn hand_location(&self, photo: &Path) -> Result<Hand, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(Hand::Unset);
        };
        let row = self.conn.query_row(
            "SELECT lat, lon FROM photo_location
             WHERE folder_id = (SELECT id FROM folders WHERE path = ?1) AND file_name = ?2
               AND source = 'hand'",
            rusqlite::params![folder, name],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
        );
        match row {
            Ok((Some(la), Some(lo))) => Ok(Hand::At(la, lo)),
            Ok(_) => Ok(Hand::Removed),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Hand::Unset),
            Err(e) => Err(e.into()),
        }
    }

    /// Set `hand` for every photo in `photos`, in one transaction. ⚠ A photo whose folder the catalog
    /// does not know is skipped — `set_capture`'s rule.
    pub fn set_hand(&self, photos: &[PathBuf], hand: Hand) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        for p in photos {
            let Some((folder, name)) = split(p) else {
                continue;
            };
            match hand {
                Hand::Unset => {
                    tx.execute(
                        "DELETE FROM photo_location
                         WHERE folder_id = (SELECT id FROM folders WHERE path = ?1)
                           AND file_name = ?2 AND source = 'hand'",
                        rusqlite::params![folder, name],
                    )?;
                }
                Hand::Removed => {
                    tx.execute(
                        UPSERT,
                        rusqlite::params![folder, name, "hand", None::<f64>, None::<f64>],
                    )?;
                }
                Hand::At(la, lo) => {
                    tx.execute(UPSERT, rusqlite::params![folder, name, "hand", la, lo])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record what the file itself says — `None` is "read, none", so the startup pass skips it next time.
    pub fn set_file_location(
        &self,
        photo: &Path,
        at: Option<(f64, f64)>,
    ) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        let (la, lo) = at.unzip();
        self.conn
            .execute(UPSERT, rusqlite::params![folder, name, "file", la, lo])?;
        Ok(())
    }

    /// Record Immich's positions. Returns how many rows changed.
    pub fn set_server_locations(
        &self,
        rows: &[(PathBuf, (f64, f64))],
    ) -> Result<usize, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for (p, (la, lo)) in rows {
            let Some((folder, name)) = split(p) else {
                continue;
            };
            n += tx.execute(UPSERT, rusqlite::params![folder, name, "server", la, lo])?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Every photo whose capture was read but whose file location never was — the photos catalogued
    /// before M10.18b, for the startup pass. Across the whole catalog, originals only.
    pub fn files_without_location_read(&self) -> Result<Vec<PathBuf>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT f.path, c.file_name FROM photo_capture c
             JOIN folders f ON f.id = c.folder_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM photo_location l
                 WHERE l.folder_id = c.folder_id AND l.file_name = c.file_name AND l.source = 'file'
             )",
        )?;
        let rows = st.query_map([], |r| {
            Ok(PathBuf::from(r.get::<_, String>(0)?).join(r.get::<_, String>(1)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;
    use core_image::Capture;

    fn cat_with(names: &[&str]) -> (Catalog, Vec<PathBuf>) {
        let cat = Catalog::open_in_memory().unwrap();
        let e = entries_in("/trip", names);
        cat.sync_folder(Path::new("/trip"), &e).unwrap();
        (cat, e.into_iter().map(|e| e.path).collect())
    }

    const LISBON: (f64, f64) = (38.7139, -9.1394);
    const SINTRA: (f64, f64) = (38.80, -9.38);
    const PORTO: (f64, f64) = (41.15, -8.61);

    #[test]
    fn hand_beats_file_beats_server() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_server_locations(&[(p[0].clone(), PORTO)]).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), Some(PORTO));
        cat.set_file_location(&p[0], Some(LISBON)).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), Some(LISBON));
        cat.set_hand(&p, Hand::At(SINTRA.0, SINTRA.1)).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), Some(SINTRA));
        assert_eq!(
            cat.hand_location(&p[0]).unwrap(),
            Hand::At(SINTRA.0, SINTRA.1)
        );
    }

    /// A file read with no GPS does not hide the server's position.
    #[test]
    fn a_file_read_as_none_leaves_the_server_position() {
        let (cat, p) = cat_with(&["a.JPG"]);
        cat.set_server_locations(&[(p[0].clone(), PORTO)]).unwrap();
        cat.set_file_location(&p[0], None).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), Some(PORTO));
    }

    #[test]
    fn removed_shows_no_location_and_unset_brings_the_file_back() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_file_location(&p[0], Some(LISBON)).unwrap();
        cat.set_hand(&p, Hand::Removed).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), None);
        assert_eq!(cat.hand_location(&p[0]).unwrap(), Hand::Removed);
        cat.set_hand(&p, Hand::Unset).unwrap();
        assert_eq!(cat.location_of(&p[0]).unwrap(), Some(LISBON));
        assert_eq!(cat.hand_location(&p[0]).unwrap(), Hand::Unset);
    }

    #[test]
    fn locations_of_answers_only_located_photos() {
        let (cat, p) = cat_with(&["a.RAF", "b.RAF", "c.RAF"]);
        cat.set_file_location(&p[0], Some(LISBON)).unwrap();
        cat.set_file_location(&p[1], None).unwrap();
        let got = cat.locations_of(&p).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[&p[0]], LISBON);
    }

    /// Review focus 3: a move carries the hand location (`PER_FILE_TABLES`).
    #[test]
    fn a_moved_photo_keeps_its_hand_location() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.sync_folder(Path::new("/other"), &[]).unwrap();
        cat.set_hand(&p, Hand::At(SINTRA.0, SINTRA.1)).unwrap();
        let to = PathBuf::from("/other/a.RAF");
        cat.move_photo(&p[0], &to).unwrap();
        assert_eq!(cat.location_of(&to).unwrap(), Some(SINTRA));
        assert_eq!(cat.location_of(&p[0]).unwrap(), None);
    }

    #[test]
    fn files_without_location_read_lists_each_photo_once_until_read() {
        let (cat, p) = cat_with(&["a.RAF", "b.RAF"]);
        assert!(
            cat.files_without_location_read().unwrap().is_empty(),
            "no capture read yet"
        );
        for x in &p {
            cat.set_capture(x, &Capture::default()).unwrap();
        }
        let mut want = cat.files_without_location_read().unwrap();
        want.sort();
        assert_eq!(want, p);
        cat.set_file_location(&p[0], None).unwrap();
        assert_eq!(
            cat.files_without_location_read().unwrap(),
            vec![p[1].clone()]
        );
    }
}

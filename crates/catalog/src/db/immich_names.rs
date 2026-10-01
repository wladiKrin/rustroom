//! The original file names of Immich downloads (2026-10-01, Move to folder).
//!
//! ⚠⚠ **Why this exists.** The Immich cache names a download by its asset id (`<uuid>.HEIC`) — on
//! purpose, see `immich_source::cache_path` — and a `photos` row is keyed on that name. The name the
//! photo had on the phone (`IMG_1234.HEIC`) lived only in the asset list held in memory, so a photo
//! moved out of the cache would have kept the id as its name. `record_assets` writes it here whenever
//! an album or a month is opened, which is what lets the *immich (downloaded)* folder view rename too.
//!
//! ⚠ Not one of the per-file tables a move re-keys: the mapping belongs to the cache name, which stays
//! valid when the asset is downloaded into the cache again.

use super::{split, Catalog, CatalogError};
use std::path::Path;

impl Catalog {
    /// Remember the original name of each cache file in `folder`, as `(cache file name, original)`.
    /// A name already recorded is replaced.
    pub fn set_original_names(
        &self,
        folder: &Path,
        names: &[(String, String)],
    ) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO immich_names (folder_id, file_name, original)
                 SELECT id, ?2, ?3 FROM folders WHERE path = ?1
                 ON CONFLICT(folder_id, file_name) DO UPDATE SET original = ?3",
            )?;
            let dir = folder.to_string_lossy().to_string();
            for (name, original) in names {
                st.execute(rusqlite::params![dir, name, original])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The original name recorded for the cache file at `path`, if any.
    pub fn original_name(&self, path: &Path) -> Result<Option<String>, CatalogError> {
        let Some((folder, name)) = split(path) else {
            return Ok(None);
        };
        match self.conn.query_row(
            "SELECT n.original FROM immich_names n JOIN folders f ON f.id = n.folder_id
             WHERE f.path = ?1 AND n.file_name = ?2",
            rusqlite::params![folder, name],
            |r| r.get(0),
        ) {
            Ok(n) => Ok(Some(n)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::db::test_support::entry;
    use crate::db::Catalog;
    use std::path::Path;

    #[test]
    fn original_names_round_trip_and_an_upsert_replaces() {
        let cat = Catalog::open_in_memory().unwrap();
        let root = Path::new("/cache/host");
        cat.sync_folder(root, &[entry("3f2a.HEIC")]).unwrap();
        let cached = root.join("3f2a.HEIC");
        assert_eq!(cat.original_name(&cached).unwrap(), None, "nothing yet");
        cat.set_original_names(root, &[("3f2a.HEIC".into(), "IMG_1.HEIC".into())])
            .unwrap();
        assert_eq!(
            cat.original_name(&cached).unwrap().as_deref(),
            Some("IMG_1.HEIC")
        );
        cat.set_original_names(root, &[("3f2a.HEIC".into(), "IMG_2.HEIC".into())])
            .unwrap();
        assert_eq!(
            cat.original_name(&cached).unwrap().as_deref(),
            Some("IMG_2.HEIC")
        );
        assert_eq!(cat.original_name(&root.join("other.HEIC")).unwrap(), None);
    }
}

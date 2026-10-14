//! The last send of marks to Immich (M10.15d): what each photo it changed had before, so **Undo last
//! send** can put it back.
//!
//! ⚠ Keyed by Immich's **asset id**, not by file — the value belongs to the server's asset, so a move
//! of the cached original does not concern it (and this is not one of `PER_FILE_TABLES`). Only the
//! latest send is kept: a new one replaces it.

use super::{Catalog, CatalogError};

impl Catalog {
    /// Replace the last send with `rows` — `(asset id, prior rating, prior favourite)`.
    pub fn record_send(&self, rows: &[(String, Option<i32>, bool)]) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM immich_last_send", [])?;
        for (id, rating, favourite) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO immich_last_send (asset_id, rating, favourite)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![id, rating, *favourite as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The last send, by asset id.
    pub fn last_send(&self) -> Result<Vec<(String, Option<i32>, bool)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT asset_id, rating, favourite FROM immich_last_send ORDER BY asset_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i32>>(1)?,
                r.get::<_, i64>(2)? != 0,
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Remove `ids` from the last send — what an undo has put back.
    pub fn forget_sent(&self, ids: &[String]) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM immich_last_send WHERE asset_id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, rating: Option<i32>, fav: bool) -> (String, Option<i32>, bool) {
        (id.to_string(), rating, fav)
    }

    #[test]
    fn record_replaces_the_last_send() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.record_send(&[row("a", Some(3), false), row("b", None, true)])
            .unwrap();
        cat.record_send(&[row("c", Some(1), false)]).unwrap();
        assert_eq!(cat.last_send().unwrap(), vec![row("c", Some(1), false)]);
    }

    #[test]
    fn forget_removes_only_those() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.record_send(&[row("a", Some(3), false), row("b", Some(2), true)])
            .unwrap();
        cat.forget_sent(&["a".to_string()]).unwrap();
        assert_eq!(cat.last_send().unwrap(), vec![row("b", Some(2), true)]);
    }

    /// ⚠ An unrated prior is what an undo clears back to — a NULL that must survive the round trip.
    #[test]
    fn a_null_prior_rating_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.record_send(&[row("a", None, false)]).unwrap();
        assert_eq!(cat.last_send().unwrap(), vec![row("a", None, false)]);
    }
}

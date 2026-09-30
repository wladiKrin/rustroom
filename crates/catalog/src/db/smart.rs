//! Smart collections: a name and a stored rule, evaluated by the caller.

use super::{unix_now, Catalog, CatalogError};
use crate::snapshot::CatalogSnapshot;

/// A saved rule and the name it is browsed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartRow {
    pub id: i64,
    pub name: String,
    /// The rule, as JSON. ⚠ Text rather than columns: it is a **tree** whose shape the editor will
    /// grow into, and columns would need a migration every time a criterion is added. The
    /// evaluator in `app` is the single authority on what this text means.
    pub rule: String,
}

impl Catalog {
    /// Create a smart collection, or return the existing one of that name.
    ///
    /// ⚠ A duplicate name is not an error, for the reason `create_collection` gives: the name is
    /// the identity a user thinks in, so failing the second time "Keepers" is used is a bug.
    pub fn create_smart(&self, name: &str, rule: &str) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO smart_collections (name, rule, created) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO NOTHING",
            rusqlite::params![name, rule, unix_now()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM smart_collections WHERE name = ?1",
            [name],
            |r| r.get(0),
        )?)
    }

    /// Every smart collection, ordered by name.
    pub fn smart_collections(&self) -> Result<Vec<SmartRow>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, rule FROM smart_collections ORDER BY lower(name), name")?;
        let rows = stmt.query_map([], |r| {
            Ok(SmartRow {
                id: r.get(0)?,
                name: r.get(1)?,
                rule: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Replace a smart collection's rule, leaving its row and name alone.
    pub fn set_smart_rule(&self, id: i64, rule: &str) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE smart_collections SET rule = ?2 WHERE id = ?1",
            rusqlite::params![id, rule],
        )?;
        Ok(())
    }

    /// Remove a smart collection.
    ///
    /// ⚠ It is a **query**, so nothing it matched is affected — unlike forgetting a folder, which
    /// takes ratings and keywords with it. No confirmation, following `delete_collection`.
    pub fn delete_smart(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let (name, rule, created) = tx.query_row(
            "SELECT name, rule, created FROM smart_collections WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute("DELETE FROM smart_collections WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Smart {
            name,
            rule,
            created,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;
    use std::path::Path;

    #[test]
    fn a_smart_collection_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].name, "Keepers");
        assert_eq!(rows[0].rule, r#"{"All":[]}"#);
    }

    #[test]
    fn a_duplicate_smart_name_returns_the_existing_one() {
        // ⚠ The rule `create_collection` and `create_keyword` already follow: the name is the
        // identity a user thinks in, so failing the second time "Keepers" is used is a bug.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let b = cat.create_smart("Keepers", r#"{"Any":[]}"#).unwrap();
        assert_eq!(a, b);
        assert_eq!(cat.smart_collections().unwrap().len(), 1);
    }

    #[test]
    fn a_rule_can_be_replaced_without_changing_the_row() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.set_smart_rule(id, r#"{"Any":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1, "editing must not create a second row");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].rule, r#"{"Any":[]}"#);
    }

    #[test]
    fn deleting_a_smart_collection_touches_no_photos() {
        // ⚠ The property people actually worry about — a smart collection is a *query*, and
        // nothing it matched is affected by removing it. Unlike forgetting a folder, which takes
        // ratings and keywords with it.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.delete_smart(id).unwrap();
        assert!(cat.smart_collections().unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 1);
    }
}

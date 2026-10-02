//! The schema and its migrations: every step from version 1 to [`SCHEMA_VERSION`](super::SCHEMA_VERSION).

use super::{Catalog, CatalogError};

impl Catalog {
    /// Bring the schema up to [`SCHEMA_VERSION`].
    ///
    /// ⚠⚠ **Forward-only, by construction: every step is gated on `from < n`.** A newer Rustroom
    /// may have written a schema this build does not know, and downgrading it would destroy
    /// whatever that version added.
    ///
    /// ⚠ All four tables are created in one step although collections are used by a later slice.
    /// A migration that has already run on someone's machine cannot be edited afterwards, so the
    /// choice is to write it once now or add a second migration later for no benefit.
    pub(super) fn migrate(&self) -> Result<(), CatalogError> {
        let from: i64 = self.version()?;
        if from < 1 {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE folders (
                     id        INTEGER PRIMARY KEY,
                     path      TEXT NOT NULL UNIQUE,
                     last_seen INTEGER NOT NULL
                 );
                 CREATE TABLE photos (
                     id        INTEGER PRIMARY KEY,
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     copy      INTEGER NOT NULL,
                     size      INTEGER,
                     mtime     INTEGER,
                     last_seen INTEGER NOT NULL,
                     UNIQUE(folder_id, file_name, copy)
                 );
                 CREATE TABLE collections (
                     id      INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL UNIQUE,
                     created INTEGER NOT NULL
                 );
                 CREATE TABLE collection_photos (
                     collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
                     photo_id      INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
                     added         INTEGER NOT NULL,
                     PRIMARY KEY (collection_id, photo_id)
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if from < 2 {
            // ⚠⚠ A **new gate**, never an edit to version 1. A migration that has already run on
            // someone's machine cannot be changed, and this is the first one to meet a catalog
            // holding real folders, photos and collections.
            //
            // ⚠ `NOT NULL DEFAULT 0` means existing rows acquire "unmarked" with no backfill, and
            // 0 is the neutral value for all three — so "no mark" needs no special case anywhere.
            self.conn.execute_batch(
                "BEGIN;
                 ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE photos ADD COLUMN flag   INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE photos ADD COLUMN label  INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if from < 3 {
            // ⚠⚠ A **new gate**, never an edit to versions 1 or 2. Second time this rule applies
            // to a database holding real user data.
            //
            // ⚠⚠ The `COALESCE` index is load-bearing. A plain `UNIQUE(parent_id, name)` would
            // allow **two root-level "Paris" keywords**, because SQLite treats NULLs as distinct
            // in a unique constraint — silent duplicates that look identical in the tree.
            //
            // ⚠⚠ `photo_keywords` is keyed on the **file**, not on `photos.id` as
            // `collection_photos` is. That is not an inconsistency: a keyword describes what is
            // *in* the picture, which every copy of a file shares, while a collection membership
            // and a rating describe a particular version. **Do not unify them.**
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE keywords (
                     id        INTEGER PRIMARY KEY,
                     name      TEXT NOT NULL,
                     parent_id INTEGER REFERENCES keywords(id) ON DELETE CASCADE
                 );
                 CREATE UNIQUE INDEX keywords_unique
                     ON keywords(COALESCE(parent_id, 0), name);
                 CREATE TABLE photo_keywords (
                     folder_id  INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name  TEXT NOT NULL,
                     keyword_id INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
                     PRIMARY KEY (folder_id, file_name, keyword_id)
                 );
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        if from < 4 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–3. A migration that has already run
            // on someone's machine cannot be changed. Third time this rule applies to a database
            // holding real user data.
            //
            // ⚠⚠ Keyed on the **file**, like `photo_keywords` and unlike marks: capture metadata
            // describes the *exposure*, which every copy of a file shares, while a rating
            // describes the image you made, which they do not. **Do not unify them.**
            //
            // ⚠ Every column is nullable and a row exists as soon as a file has been *read*, even
            // if it carried no EXIF at all. The row's presence is what stops the background pass
            // re-reading that file on every folder open — 11–15 ms each time, forever, for
            // nothing.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_capture (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     taken     INTEGER,
                     camera    TEXT,
                     lens      TEXT,
                     iso       INTEGER,
                     shutter   REAL,
                     aperture  REAL,
                     focal_len REAL,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        if from < 5 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–4. Fourth time this rule meets a
            // database holding real user data.
            //
            // ⚠ The rule is **JSON text**, not columns — see `SmartRow::rule`.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE smart_collections (
                     id      INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL UNIQUE,
                     rule    TEXT NOT NULL,
                     created INTEGER NOT NULL
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
        if from < 6 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–5. Fifth time this rule meets a
            // database holding real user data.
            //
            // ⚠ When this photo's `stem.xmp` sidecar was last **imported or written by
            // Rustroom**, in unix seconds — not when the photo was edited. The whole conflict
            // rule for XMP interchange is "a sidecar newer than this is a foreign edit and
            // wins", so `0` (which every existing row acquires with no backfill) means "never
            // seen", and any sidecar on disk therefore imports on the next folder open. That is
            // the correct behaviour for a catalog that predates this column.
            self.conn.execute_batch(
                "BEGIN;
                 ALTER TABLE photos ADD COLUMN xmp_mtime INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 6;
                 COMMIT;",
            )?;
        }
        if from < 7 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–6. Sixth time this rule meets a
            // database holding real user data.
            //
            // ⚠ Keyed on the **file**, like `photo_capture`: a description says what is in the
            // picture, which every copy shares. See `Description`. Every column is NOT NULL with
            // an empty default, and `set_description` deletes the row when all four are blank,
            // so the table holds only photos that say something.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_description (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     title     TEXT NOT NULL DEFAULT '',
                     caption   TEXT NOT NULL DEFAULT '',
                     copyright TEXT NOT NULL DEFAULT '',
                     creator   TEXT NOT NULL DEFAULT '',
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 7;
                 COMMIT;",
            )?;
        }
        if from < 8 {
            // ⚠⚠ **A one-time re-read, not a schema change** — and the only reason a migration is
            // needed at all is that `files_without_capture` queues a file only when it has **no
            // capture row**, so a row already stored with `taken IS NULL` would never be looked at
            // again.
            //
            // Until 2026-09-24 the JPEG reader took the capture time from `DateTimeOriginal` alone.
            // A Fuji **SP-3000** film scanner writes that tag as the placeholder
            // `"    :  :     :  :  "` while putting the real date in `DateTimeDigitized` **and**
            // `DateTime`, so every such scan was stored with no date — which silently cost the date
            // grouping, date sorting, the `{date}` filename token, the exported XMP and an Immich
            // upload's `fileCreatedAt`. The reader now falls back through all three tags, and these
            // rows have to be dropped for the background pass to pick them up.
            //
            // ⚠ Only rows with **no date** are dropped. A row that already has one is left alone:
            // re-reading it could only produce the same answer, and the pass is not free.
            //
            // ⚠ A photo that genuinely carries no date at all is simply re-read once and stored
            // NULL again, which is the correct outcome and costs one EXIF read.
            self.conn.execute_batch(
                "BEGIN;
                 DELETE FROM photo_capture WHERE taken IS NULL;
                 PRAGMA user_version = 8;
                 COMMIT;",
            )?;
        }
        if from < 9 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–8. M9.7: a photo's undo history.
            //
            // ⚠ Keyed like `photos` — `(folder, file, copy)` — because a virtual copy is a
            // different photo with its own undo stack. The body is opaque text: this crate knows
            // nothing of `Edits`, and must not start to. See `db/history.rs`.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_history (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     copy      INTEGER NOT NULL,
                     body      TEXT NOT NULL,
                     PRIMARY KEY (folder_id, file_name, copy)
                 );
                 PRAGMA user_version = 9;
                 COMMIT;",
            )?;
        }
        if from < 10 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–9. The original names of Immich
            // downloads, which the cache names by asset id — see `db/immich_names.rs`.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE immich_names (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     original  TEXT NOT NULL,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 10;
                 COMMIT;",
            )?;
        }
        if from < 11 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–10. M10.8c: stacks made by hand — see
            // `db/stacks.rs`. One of `PER_FILE_TABLES`.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_stacks (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     stack_id  INTEGER NOT NULL,
                     top       INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 11;
                 COMMIT;",
            )?;
        }
        if from < 12 {
            // ⚠⚠ A **new gate**, never an edit to version 11 — the author's catalog already holds it.
            // M10.8c after use: a stack's members get an order (the lowest is the top) and a mark for
            // the stacks *Stack by capture time* made. v11's `top` becomes order 0, the rest by name.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_stacks_v12 (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     stack_id  INTEGER NOT NULL,
                     ord       INTEGER NOT NULL,
                     by_time   INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 INSERT INTO photo_stacks_v12 (folder_id, file_name, stack_id, ord, by_time)
                     SELECT folder_id, file_name, stack_id,
                            ROW_NUMBER() OVER (PARTITION BY stack_id ORDER BY top DESC, file_name) - 1,
                            0
                     FROM photo_stacks;
                 DROP TABLE photo_stacks;
                 ALTER TABLE photo_stacks_v12 RENAME TO photo_stacks;
                 PRAGMA user_version = 12;
                 COMMIT;",
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{entries_in, entry, TempDb};
    use crate::db::{Marks, SCHEMA_VERSION};
    use core_image::Capture;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_version_one_catalog_upgrades_and_keeps_its_rows() {
        // ⚠⚠ **The first migration to meet real user data.** Someone has a v1 catalog with
        // folders, photos and collections in it right now. Upgrading must add the columns and
        // touch nothing else — a migration that drops and recreates would destroy a library.
        let db = TempDb::new("v1v2");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            // Pretend this catalog was written by the build that shipped v1.
            //
            // ⚠⚠ **Every new migration makes this simulation incomplete.** It must undo
            // *everything* later versions added, or `migrate` re-runs a step whose objects still
            // exist and fails with "table already exists". Adding v3's tables to the drop list
            // was exactly that — the failure is loud, but only if this test exists.
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_history;
                     DROP TABLE immich_names;
                     DROP TABLE photo_stacks;
                     DROP TABLE photo_description;
                     DROP TABLE smart_collections;
                     DROP TABLE photo_capture;
                     DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
                     ALTER TABLE photos DROP COLUMN rating;
                     ALTER TABLE photos DROP COLUMN flag;
                     ALTER TABLE photos DROP COLUMN label;
                     PRAGMA user_version = 1;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v1 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1, "the photo survived");
        assert_eq!(
            cat.collections().unwrap()[0].count,
            1,
            "and so did its collection membership"
        );
        assert_eq!(
            cat.marks(Path::new("/x/a.RAF"), 0).unwrap(),
            Marks::default(),
            "an upgraded row is unmarked"
        );
    }

    #[test]
    fn a_version_two_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The second migration to meet real user data — folders, photos, collections *and*
        // marks now. Upgrading must add tables and touch nothing else.
        let db = TempDb::new("v2v3");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 4,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_history;
                     DROP TABLE immich_names;
                     DROP TABLE photo_stacks;
                     DROP TABLE photo_description;
                     DROP TABLE smart_collections;
                     DROP TABLE photo_capture;
                     DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
                     PRAGMA user_version = 2;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v2 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.collections().unwrap()[0].count, 1, "collection kept");
        assert_eq!(
            cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating,
            4,
            "marks kept"
        );
        assert!(cat.keywords().unwrap().is_empty());
    }

    #[test]
    fn a_version_five_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The fifth migration to meet real user data — folders, photos, collections, marks
        // and keywords all present. Adding a column must touch nothing else.
        let db = TempDb::new("v5v6");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            let k = cat.create_keyword("Paris", None).unwrap();
            cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 3,
                    label: 2,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_history;
                     DROP TABLE immich_names;
                     DROP TABLE photo_stacks;
                     DROP TABLE photo_description;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
                     PRAGMA user_version = 5;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v5 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.collections().unwrap()[0].count, 1, "collection kept");
        assert_eq!(cat.keywords().unwrap()[0].count, 1, "keyword kept");
        let m = cat.marks(Path::new("/x/a.RAF"), 0).unwrap();
        assert_eq!((m.rating, m.label), (3, 2), "marks kept");
        // ⚠ An upgraded row has never had its sidecar seen, which is what makes an existing
        // folder rated in Lightroom import on the next open rather than being ignored forever.
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 0)]
        );
    }

    #[test]
    fn a_version_six_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The sixth migration to meet real user data. Adding a table must touch nothing else.
        let db = TempDb::new("v6v7");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let k = cat.create_keyword("Paris", None).unwrap();
            cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 3,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.set_xmp_mtime(Path::new("/x/a.RAF"), 42).unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_history;
                     DROP TABLE immich_names;
                     DROP TABLE photo_stacks;
                     DROP TABLE photo_description;
                     PRAGMA user_version = 6;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v6 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.keywords().unwrap()[0].count, 1, "keyword kept");
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating, 3);
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 42)],
            "the sidecar clock kept"
        );
        assert_eq!(cat.description_of(Path::new("/x/a.RAF")).unwrap(), None);
    }

    /// ⚠⚠ **The v8 migration exists so a fixed reader is actually SEEN.** `files_without_capture`
    /// queues a file only when it has **no capture row**, so the 2026-09-24 date-reader fix — an
    /// SP-3000 writes `DateTimeOriginal` as a blank placeholder and the real date in
    /// `DateTimeDigitized` — would have changed nothing for photos already scanned. Dropping the
    /// **dateless** rows is what re-queues them.
    ///
    /// ⚠ And a row that already has a date must be **left alone**: re-reading it could only produce
    /// the same answer, and the background pass is not free. That is the discriminating half.
    #[test]
    fn the_v8_migration_requeues_only_the_dateless_captures() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["dated.JPG", "blank.JPG"]),
        )
        .unwrap();

        // One photo whose date was read, one whose date came back empty — the SP-3000 shape.
        cat.set_capture(
            Path::new("/a/dated.JPG"),
            &Capture {
                taken: Some(1_763_559_144),
                ..Default::default()
            },
        )
        .unwrap();
        cat.set_capture(
            Path::new("/a/blank.JPG"),
            &Capture {
                taken: None,
                camera: Some("FUJI PHOTO FILM CO., LTD. SP-3000".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            cat.files_without_capture(Path::new("/a"))
                .unwrap()
                .is_empty(),
            "both rows exist, so nothing is queued — the precondition for the bug"
        );

        // Re-run the migration from version 7, as an existing database would.
        // ⚠ The tables of versions 9 and 10 dropped too, or re-running from 7 meets them already
        // there (M9.7, Move to folder).
        cat.conn
            .execute_batch(
                "DROP TABLE photo_history; DROP TABLE immich_names; DROP TABLE photo_stacks; PRAGMA user_version = 7;",
            )
            .unwrap();
        cat.migrate().unwrap();

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["blank.JPG".to_string()],
            "only the dateless capture should be re-queued"
        );
        // ⚠ The one that had a date keeps it — untouched, not merely re-queued-and-restored.
        assert_eq!(
            cat.capture_of(Path::new("/a/dated.JPG"))
                .unwrap()
                .and_then(|c| c.taken),
            Some(1_763_559_144)
        );
    }

    /// ⚠⚠ v12 rebuilds `photo_stacks` — the author's catalog holds v11 stacks, and they must survive:
    /// v11's top becomes order 0, the other member order 1, and neither is a time stack.
    #[test]
    fn a_version_eleven_catalog_keeps_its_stacks() {
        let db = TempDb::new("v11v12");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
                .unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_stacks;
                     CREATE TABLE photo_stacks (
                         folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                         file_name TEXT NOT NULL,
                         stack_id  INTEGER NOT NULL,
                         top       INTEGER NOT NULL DEFAULT 0,
                         PRIMARY KEY (folder_id, file_name)
                     );
                     INSERT INTO photo_stacks SELECT id, 'b.RAF', 3, 1 FROM folders;
                     INSERT INTO photo_stacks SELECT id, 'a.RAF', 3, 0 FROM folders;
                     PRAGMA user_version = 11;",
                )
                .unwrap();
        }
        let cat = Catalog::open(&db.0).expect("a v11 catalog must upgrade");
        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        let mut got = cat.hand_stacks_in(Path::new("/x")).unwrap();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let ords: Vec<(String, i64, i64, bool)> = got
            .into_iter()
            .map(|(p, m)| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    m.stack,
                    m.ord,
                    m.by_time,
                )
            })
            .collect();
        assert_eq!(
            ords,
            vec![
                ("a.RAF".to_string(), 3, 1, false),
                ("b.RAF".to_string(), 3, 0, false)
            ],
            "the top first"
        );
    }
}

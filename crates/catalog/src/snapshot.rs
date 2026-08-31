//! What a catalog deletion removed, in enough detail to put it back.
//!
//! ⚠⚠ **These are re-inserted with FRESH ids, not their original ones.** Probing SQLite showed
//! that explicit-rowid restore works *but collides*: without `AUTOINCREMENT` a freed id is handed
//! to the next insert, so deleting a collection, creating another, then undoing fails with
//! `UNIQUE constraint failed`. What must survive a restore is the **relationships**, not the ids —
//! and because a cascade removes exactly the dependent rows, a snapshot is *closed under
//! references*, so remapping is total. The alternative was an `AUTOINCREMENT` migration
//! recreating four tables on live user data.
//!
//! ⚠ The ids here are therefore the **old** ones, kept only so child rows can be matched to their
//! parents while remapping.

use std::path::PathBuf;

/// One `photos` row, with its marks.
///
/// ⚠ The marks are the point. Forgetting a folder is the only action that destroys metadata the
/// user made by hand, and a restore that dropped the ratings would look like it had worked.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoSnapshot {
    pub id: i64,
    pub file_name: String,
    pub copy: u32,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub last_seen: i64,
    pub rating: u8,
    pub flag: i64,
    pub label: u8,
}

/// One `photo_capture` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSnapshot {
    pub file_name: String,
    pub taken: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub shutter: Option<f64>,
    pub aperture: Option<f64>,
    pub focal_len: Option<f64>,
}

/// One `keywords` row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeywordSnapshot {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

/// What a deletion removed.
///
/// ⚠ One enum rather than four action kinds, so `ActionLog` gains a single variant and the restore
/// is one function with one transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogSnapshot {
    Smart {
        name: String,
        rule: String,
        created: i64,
    },
    Collection {
        name: String,
        created: i64,
        /// `(photo_id, added)`. ⚠ These photo ids are **not** remapped — the photos still exist.
        /// A membership whose photo has since gone is skipped on restore.
        members: Vec<(i64, i64)>,
    },
    Keyword {
        /// The subtree, **parents before children**, so a restore can remap `parent_id` to an id
        /// that already exists.
        tree: Vec<KeywordSnapshot>,
        /// `(folder_id, file_name, keyword_id)` for every assignment of any keyword in `tree`.
        assignments: Vec<(i64, String, i64)>,
    },
    Folder {
        path: PathBuf,
        photos: Vec<PhotoSnapshot>,
        captures: Vec<CaptureSnapshot>,
        /// `(collection_id, photo_id, added)` — the photo id is remapped, the collection id is not.
        memberships: Vec<(i64, i64, i64)>,
        /// `(file_name, keyword_id)`; the folder id is remapped.
        keywords: Vec<(String, i64)>,
    },
}

impl CatalogSnapshot {
    /// What to say in the status line after an undo.
    pub fn label(&self) -> String {
        match self {
            Self::Smart { name, .. } => format!("Restored smart collection “{name}”"),
            Self::Collection { name, .. } => format!("Restored collection “{name}”"),
            Self::Keyword { tree, .. } => match tree.first() {
                Some(k) => format!("Restored keyword “{}”", k.name),
                None => "Restored keyword".to_string(),
            },
            Self::Folder { path, photos, .. } => {
                format!("Restored {} with {} photos", path.display(), photos.len())
            }
        }
    }
}

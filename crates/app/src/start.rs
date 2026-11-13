//! The start page and reopening where you left off (M11.4) — no egui.
//!
//! [`Place`] is what the Library last showed, stored in the settings: its source, mirrored from the bin's
//! `LibrarySource` variant for variant, and the photo that was selected. The bin converts between the two
//! (`place.rs`) and draws the page (`start_page.rs`); what can be reopened and what the page offers to continue
//! are decided here.

use crate::rail_data::RailData;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What the Library showed — the bin's `LibrarySource`, in a form the settings file can hold.
///
/// ⚠ Every variant is a struct (`Folder { path }`, not `Folder(PathBuf)`): an internally tagged enum cannot
/// serialise a newtype variant around a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Source {
    Folder {
        path: PathBuf,
    },
    AllPhotos,
    Smart {
        id: i64,
        name: String,
    },
    /// The date tree's periods: `(year, month, day)`, as `date_tree::Period`.
    Periods {
        periods: Vec<(i32, Option<u32>, Option<u32>)>,
        label: String,
    },
    Collection {
        id: i64,
        name: String,
    },
    Publish {
        id: i64,
        name: String,
    },
    Keyword {
        id: i64,
        name: String,
    },
    Person {
        id: i64,
        name: String,
    },
    ImmichPerson {
        id: String,
        person: i64,
        name: String,
    },
    ImmichAlbum {
        id: String,
        name: String,
    },
    ImmichBucket {
        bucket: String,
        label: String,
    },
}

/// Where the Library was: its source and the selected photo's `(path, copy)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub source: Source,
    pub selected: Option<(PathBuf, u32)>,
}

/// Whether `source` can be opened again: a folder on disk (`exists`) or still in the catalog (an unplugged drive,
/// which the rail opens from its last-seen rows); a catalog id still in its list; an Immich source only while
/// `immich_connected`. All Photos and dates always — they may show nothing, which is not a failure.
pub fn reopenable(
    source: &Source,
    rail: &RailData,
    immich_connected: bool,
    exists: impl Fn(&Path) -> bool,
) -> bool {
    match source {
        Source::Folder { path } => exists(path) || rail.folders.iter().any(|f| f.path == *path),
        Source::AllPhotos | Source::Periods { .. } => true,
        Source::Smart { id, .. } => rail.smarts.iter().any(|r| r.id == *id),
        Source::Collection { id, .. } => rail.collections.iter().any(|r| r.id == *id),
        Source::Publish { id, .. } => rail.publish.iter().any(|r| r.id == *id),
        Source::Keyword { id, .. } => rail.keywords.iter().any(|r| r.id == *id),
        Source::Person { id, .. } => rail.people.iter().any(|r| r.id == *id),
        Source::ImmichPerson { person, .. } => {
            immich_connected && rail.people.iter().any(|r| r.id == *person)
        }
        Source::ImmichAlbum { .. } | Source::ImmichBucket { .. } => immich_connected,
    }
}

/// How many recent folders the start page offers.
pub const CONTINUE_MAX: usize = 5;

/// What a Continue row opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinueKind {
    AllPhotos,
    Folder(PathBuf),
}

/// One row of the start page's Continue card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueRow {
    pub kind: ContinueKind,
    /// "All Photos", or the folder's own name.
    pub name: String,
    /// A folder's parent, with the home folder written `~`.
    pub parent: Option<String>,
    /// The photos in it, where the catalog knows.
    pub count: Option<i64>,
}

/// The Continue card's rows: All Photos while the catalog has any (`all_photos`, as the rail counts it), then
/// up to [`CONTINUE_MAX`] of `recent`, in order, that still `exists`. Empty means no card.
pub fn continue_rows(
    recent: &[PathBuf],
    rail: &RailData,
    all_photos: i64,
    exists: impl Fn(&Path) -> bool,
) -> Vec<ContinueRow> {
    let home = dirs::home_dir();
    let mut rows = Vec::new();
    if all_photos > 0 {
        rows.push(ContinueRow {
            kind: ContinueKind::AllPhotos,
            name: "All Photos".to_string(),
            parent: None,
            count: Some(all_photos),
        });
    }
    for dir in recent.iter().filter(|p| exists(p)).take(CONTINUE_MAX) {
        let parent = dir.parent().map(|p| match home.as_deref() {
            Some(h) if p.starts_with(h) => match p.strip_prefix(h) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => p.display().to_string(),
            },
            _ => p.display().to_string(),
        });
        rows.push(ContinueRow {
            kind: ContinueKind::Folder(dir.clone()),
            name: dir.file_name().map_or_else(
                || dir.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            parent,
            count: rail
                .folders
                .iter()
                .find(|f| f.path == *dir)
                .map(|f| f.count),
        });
    }
    rows
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;

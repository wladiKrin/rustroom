//! The start page and reopening where you left off (M11.4) — no egui.
//!
//! [`Place`] is what the Library last showed, stored in the settings: its source, mirrored from the bin's
//! `LibrarySource` variant for variant, and the photo that was selected. The bin converts between the two
//! (`place.rs`) and draws the page (`start_page.rs`); what can be reopened and what the page offers to continue
//! are decided here.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;

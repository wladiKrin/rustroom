//! What an import is asked to do — remembered in `Settings.import` as the options the last import
//! actually used.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The Import dialog's choices.
///
/// ⚠ Container-level `#[serde(default)]` only — `settings.rs`'s rule: a field-level default beside
/// it would make a missing field disagree with `Default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// Where the dated folders are made.
    pub destination: PathBuf,
    /// Whether a second copy is made. The folder is remembered even while this is off.
    pub backup_on: bool,
    pub backup: PathBuf,
    /// How the destination is organised (the author's request after first use).
    pub layout: Layout,
    /// With [`Layout::OneFolder`], a folder inside the destination; empty means the destination
    /// itself. Cleaned by `safe_name::folder_name` where it is used.
    pub subfolder: String,
    /// A `filename_template`; `{name}` keeps the card's names.
    pub template: String,
    /// Applied on entry only when non-empty — an empty field erases nothing.
    pub copyright: String,
    pub creator: String,
    /// Comma-separated; each becomes a top-level keyword, created if missing.
    pub keywords: String,
    /// A develop preset's name, or `None`.
    pub preset: Option<String>,
}

impl Default for ImportOptions {
    /// ⚠ Hand-written: an empty template would name every file by the fallback.
    fn default() -> Self {
        Self {
            destination: PathBuf::new(),
            backup_on: false,
            backup: PathBuf::new(),
            layout: Layout::default(),
            subfolder: String::new(),
            template: "{name}".to_string(),
            copyright: String::new(),
            creator: String::new(),
            keywords: String::new(),
            preset: None,
        }
    }
}

/// Where in the destination a photo lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Layout {
    /// `YYYY/YYYY-MM-DD` — one folder per day, grouped by year.
    #[default]
    YearDay,
    /// `YYYY/MM/DD`.
    YearMonthDay,
    /// `YYYY-MM-DD`, no year level.
    Day,
    /// Everything into one folder: the destination, or the named subfolder inside it.
    OneFolder,
}

impl Layout {
    pub const ALL: [Layout; 4] = [
        Self::YearDay,
        Self::YearMonthDay,
        Self::Day,
        Self::OneFolder,
    ];

    /// The dialog's label for this choice.
    pub fn label(self) -> &'static str {
        match self {
            Self::YearDay => "YYYY / YYYY-MM-DD",
            Self::YearMonthDay => "YYYY / MM / DD",
            Self::Day => "YYYY-MM-DD",
            Self::OneFolder => "Into one folder",
        }
    }
}

impl ImportOptions {
    /// The keywords, trimmed, without empties or repeats, in the order typed.
    pub fn keyword_list(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for k in self.keywords.split(',').map(str::trim) {
            if !k.is_empty() && !out.iter().any(|o| o == k) {
                out.push(k.to_string());
            }
        }
        out
    }

    /// Why Import cannot start, or `None` when it can — the dialog shows this as the footer note.
    pub fn blocker(&self, source: Option<&Path>, ticked: usize) -> Option<&'static str> {
        if ticked == 0 {
            return Some("Nothing ticked to import");
        }
        if self.destination.as_os_str().is_empty() {
            return Some("Choose a destination folder");
        }
        if crate::filename_template::Template::parse(&self.template).is_err() {
            return Some("The filename template is invalid");
        }
        if source.is_some_and(|s| self.destination.starts_with(s)) {
            return Some("The destination is inside the source");
        }
        if self.backup_on {
            if self.backup.as_os_str().is_empty() {
                return Some("Choose a backup folder");
            }
            if self.backup == self.destination {
                return Some("The backup is the destination");
            }
            if source.is_some_and(|s| self.backup.starts_with(s)) {
                return Some("The backup is inside the source");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> ImportOptions {
        ImportOptions {
            destination: PathBuf::from("/photos"),
            ..ImportOptions::default()
        }
    }

    #[test]
    fn keywords_are_split_trimmed_and_deduplicated_in_order() {
        let o = ImportOptions {
            keywords: " Paris, family ,,Paris,  holiday ".into(),
            ..ImportOptions::default()
        };
        assert_eq!(o.keyword_list(), ["Paris", "family", "holiday"]);
        assert!(ImportOptions::default().keyword_list().is_empty());
    }

    #[test]
    fn each_blocker_is_named_and_a_ready_import_has_none() {
        let card = Path::new("/Volumes/SD");
        assert_eq!(ready().blocker(Some(card), 3), None);
        assert_eq!(
            ready().blocker(Some(card), 0),
            Some("Nothing ticked to import")
        );
        assert_eq!(
            ImportOptions::default().blocker(Some(card), 3),
            Some("Choose a destination folder")
        );
        let bad = ImportOptions {
            template: "{nope}".into(),
            ..ready()
        };
        assert_eq!(
            bad.blocker(Some(card), 3),
            Some("The filename template is invalid")
        );
        let inside = ImportOptions {
            destination: card.join("copies"),
            ..ready()
        };
        assert_eq!(
            inside.blocker(Some(card), 3),
            Some("The destination is inside the source")
        );
        let no_backup = ImportOptions {
            backup_on: true,
            ..ready()
        };
        assert_eq!(
            no_backup.blocker(Some(card), 3),
            Some("Choose a backup folder")
        );
        let same = ImportOptions {
            backup_on: true,
            backup: PathBuf::from("/photos"),
            ..ready()
        };
        assert_eq!(
            same.blocker(Some(card), 3),
            Some("The backup is the destination")
        );
        // A backup folder remembered while backup is OFF blocks nothing.
        let off = ImportOptions {
            backup: PathBuf::from("/photos"),
            ..ready()
        };
        assert_eq!(off.blocker(Some(card), 3), None);
    }

    #[test]
    fn a_settings_file_without_import_options_loads_the_default() {
        let o: ImportOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, ImportOptions::default());
        let partial: ImportOptions = serde_json::from_str(r#"{"destination":"/photos"}"#).unwrap();
        assert_eq!(partial.template, "{name}");
    }
}

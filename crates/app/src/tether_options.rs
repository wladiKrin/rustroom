//! Tethered capture's options (M10.18a), remembered in `Settings.tether` when a session starts — the
//! import's rule, so a cancelled dialog leaves them alone. No egui.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The Tethered capture dialog's choices.
///
/// ⚠ Container-level `#[serde(default)]` only — `settings`'s rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TetherOptions {
    /// Where the session folders are made.
    pub destination: PathBuf,
    /// The session's folder name; empty means the day's date.
    pub session: String,
    /// A develop preset's name applied to each shot as it lands, or `None`.
    pub preset: Option<String>,
}

impl Default for TetherOptions {
    fn default() -> Self {
        let pictures = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            destination: pictures.join("Rustroom").join("Tethered"),
            session: String::new(),
            preset: None,
        }
    }
}

impl TetherOptions {
    /// The session's folder: `destination/<session>`, the session named `today` (`YYYY-MM-DD`) when
    /// unnamed. ⚠ The name is the user's text — `safe_name::folder_name` keeps it one folder deep.
    pub fn folder(&self, today: &str) -> PathBuf {
        let name = crate::safe_name::folder_name(&self.session);
        let name = if name.is_empty() {
            today.to_string()
        } else {
            name
        };
        self.destination.join(name)
    }
}

/// Today as `YYYY-MM-DD`, local time — the unnamed session's folder.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// "1 shot", "12 shots".
pub fn shots(n: usize) -> String {
    if n == 1 {
        "1 shot".into()
    } else {
        format!("{n} shots")
    }
}

/// The status row's line while a session runs: the camera once it is open, the shots so far.
pub fn status_line(camera: Option<&str>, shots_so_far: usize, stopping: bool) -> String {
    match (stopping, camera) {
        (true, _) => "Stopping tethering".into(),
        (false, None) => "Tethered · connecting to the camera".into(),
        (false, Some(c)) => format!("Tethered · {c} · {}", shots(shots_so_far)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(dest: &str, session: &str) -> TetherOptions {
        TetherOptions {
            destination: PathBuf::from(dest),
            session: session.into(),
            preset: None,
        }
    }

    #[test]
    fn the_session_folder_is_the_date_when_unnamed() {
        assert_eq!(
            at("/t", "").folder("2026-10-28"),
            PathBuf::from("/t/2026-10-28")
        );
        assert_eq!(
            at("/t", "  Studio Köln ").folder("2026-10-28"),
            PathBuf::from("/t/Studio Köln")
        );
    }

    #[test]
    fn a_hostile_session_name_stays_inside_the_destination() {
        for s in ["../../etc", "a/b", "..", "/abs"] {
            let f = at("/t", s).folder("2026-10-28");
            assert_eq!(
                f.parent(),
                Some(std::path::Path::new("/t")),
                "{s:?} → {f:?}"
            );
        }
    }

    #[test]
    fn the_status_line_names_the_camera_and_counts_the_shots() {
        assert_eq!(
            status_line(None, 0, false),
            "Tethered · connecting to the camera"
        );
        assert_eq!(
            status_line(Some("X-T4"), 1, false),
            "Tethered · X-T4 · 1 shot"
        );
        assert_eq!(
            status_line(Some("X-T4"), 12, false),
            "Tethered · X-T4 · 12 shots"
        );
        assert_eq!(status_line(Some("X-T4"), 12, true), "Stopping tethering");
    }

    #[test]
    fn the_default_destination_is_a_tethered_folder() {
        assert!(TetherOptions::default()
            .destination
            .ends_with("Rustroom/Tethered"));
    }
}

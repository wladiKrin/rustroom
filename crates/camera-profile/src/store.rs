//! Finding the camera profiles a user has installed.
//!
//! ⚠⚠ **Rustroom ships no profiles.** Adobe's `.dcp` files are Adobe's, and this project cannot
//! make a legal determination about redistributing them. Discovering what the user already has
//! sidesteps the question entirely — and is independently better: no payload in the repository,
//! and nothing that goes stale when a profile is revised.

use crate::CameraProfile;
use std::path::{Path, PathBuf};

/// A profile found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    pub path: PathBuf,
    /// The profile's own `ProfileName`, falling back to the file stem.
    pub name: String,
}

/// Whether a profile's name names this camera model.
///
/// ⚠⚠ Containment **at word boundaries**, not plain containment. libraw reports `"X-T4"` while the
/// profile is `"FUJIFILM X-T4"`, so equality finds nothing — but plain containment matches
/// `"X-T3"` against `"FUJIFILM X-T30"`, silently applying a different body's profile. Requiring a
/// non-alphanumeric character (or the string's end) on both sides handles the make prefix *and*
/// multi-word models like `"EOS 5D Mark IV"`.
pub fn matches_model(profile_name: &str, model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    let (hay, needle) = (profile_name.to_uppercase(), model.to_uppercase());
    let bytes = hay.as_bytes();
    hay.match_indices(&needle).any(|(i, _)| {
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let end = i + needle.len();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        before_ok && after_ok
    })
}

/// The directory profiles are read from. Path injected, like every other store here, so tests
/// never touch the real config directory.
pub struct ProfileStore {
    dir: PathBuf,
}

impl ProfileStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<platform config dir>/rustroom/profiles`, beside `presets/` and `settings.json`.
    ///
    /// ⚠ Not `~/.config` everywhere — `dirs::config_dir()` follows each platform's convention,
    /// which on macOS is `~/Library/Application Support/rustroom/`.
    pub fn user_default(config_dir: &Path) -> Self {
        Self::new(config_dir.join("rustroom").join("profiles"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every readable profile, sorted by name.
    ///
    /// ⚠ A file that will not read or parse is **skipped**, never fatal: this directory is
    /// user-managed, and a truncated download should cost one entry rather than all of them. A
    /// missing directory is empty, which is the common case on a fresh install.
    pub fn list(&self) -> Vec<ProfileEntry> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<ProfileEntry> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp")))
            .filter_map(|path| {
                let p = CameraProfile::from_file(&path).ok()?;
                let name = p.name.unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                Some(ProfileEntry { path, name })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Every profile, those matching `model` first.
    ///
    /// ⚠ The match **orders**, it never filters. An unusual model string must not make every
    /// profile unreachable — picking by hand has to stay possible, which is why this project
    /// carries no alias table.
    pub fn list_for(&self, model: &str) -> Vec<ProfileEntry> {
        let mut out = self.list();
        out.sort_by_key(|e| !matches_model(&e.name, model));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{matrix_bytes, synth, TempDir};

    fn profile_named(name: &str) -> Vec<u8> {
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        synth(&[
            (50721, 10, 9, matrix_bytes(1.0)),
            (50936, 2, n.len() as u32, n),
        ])
    }

    #[test]
    fn a_model_matches_its_own_profile() {
        assert!(matches_model("FUJIFILM X-T4", "X-T4"));
        assert!(matches_model("fujifilm x-t4", "X-T4"));
        assert!(matches_model("FUJIFILM X-T4", "x-t4"));
        assert!(matches_model("Canon EOS 5D Mark IV", "EOS 5D Mark IV"));
    }

    #[test]
    fn a_model_does_not_match_one_that_merely_extends_it() {
        // ⚠⚠ The case that matters, and the reason plain containment is not enough:
        // "FUJIFILM X-T30" *contains* "X-T3", so a careless substring test makes nearly every
        // model a prefix of some other body and silently applies the wrong profile.
        assert!(!matches_model("FUJIFILM X-T30", "X-T3"));
        assert!(!matches_model("FUJIFILM X-T4", "X-T"));
    }

    #[test]
    fn a_model_that_is_a_whole_word_prefix_does_match_and_that_is_accepted() {
        // ⚠ `"EOS 5D"` matches `"Canon EOS 5D Mark IV"`, because both boundaries are spaces. The
        // rule cannot separate those without knowing that "Mark IV" continues the model name, and
        // a table encoding that for every camera is exactly the liability this design refuses.
        //
        // ⚠ **It is accepted because the match only ORDERS, never filters, and never
        // auto-selects.** The default stays "Built-in (libraw)", so the worst case is a profile
        // listed higher than it deserves — not a photo rendered through the wrong body's profile.
        // If matching ever starts selecting, this becomes a real bug and this test is where to
        // start.
        assert!(matches_model("Canon EOS 5D Mark IV", "EOS 5D"));
    }

    #[test]
    fn an_unrelated_profile_does_not_match() {
        assert!(!matches_model("NIKON Z 8", "X-T4"));
        assert!(!matches_model("FUJIFILM X-T4", ""));
    }

    #[test]
    fn scanning_lists_profiles_and_skips_everything_else() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("notes.txt"), b"not a profile").unwrap();
        std::fs::write(dir.path().join("broken.dcp"), b"II\x00\x00").unwrap();
        std::fs::write(dir.path().join("good.dcp"), profile_named("TEST CAM")).unwrap();

        let found = ProfileStore::new(dir.path().to_path_buf()).list();
        // ⚠ One bad file must not hide the rest — a directory of profiles is user-managed, and a
        // truncated download should cost one entry, not all of them.
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].name, "TEST CAM");
    }

    #[test]
    fn a_missing_directory_is_empty_rather_than_an_error() {
        // The common case on a fresh install: nobody has made the directory yet.
        let store = ProfileStore::new(PathBuf::from("/nonexistent/rustroom/profiles"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn matching_profiles_come_first_but_nothing_is_filtered_out() {
        // ⚠ The match **orders**, it never filters. An unusual model string must not make every
        // profile unreachable — the user can still pick by hand.
        let dir = TempDir::new();
        std::fs::write(dir.path().join("a.dcp"), profile_named("NIKON Z 8")).unwrap();
        std::fs::write(dir.path().join("b.dcp"), profile_named("FUJIFILM X-T4")).unwrap();
        let found = ProfileStore::new(dir.path().to_path_buf()).list_for("X-T4");
        assert_eq!(found.len(), 2, "nothing should be filtered out");
        assert_eq!(found[0].name, "FUJIFILM X-T4", "the match should be first");
    }
}

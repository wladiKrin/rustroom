//! Loading and saving the settings file.

use super::Settings;
use crate::secrets::{KeyHome, Keys, SecretStore};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings io: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings json: {0}")]
    Json(#[from] serde_json::Error),
}

/// The settings file. Path injected so tests never touch the real home directory.
///
/// ⚠⚠ **The Immich keys are kept in the OS keychain when there is one (M10.15a)** — `user_default()`,
/// the app's only constructor, brings the keychain; `new(path)`, every test's, brings none and keeps
/// them in the file exactly as before, so no test can reach the real keychain.
///
/// ⚠⚠ **The one rule: keys in the file are always the newest** — they are only ever written there
/// when the keychain could not take them. So `load` moves them in, and the file loses them only after
/// a keychain write succeeded: no path deletes a key before another copy exists.
pub struct SettingsStore {
    path: PathBuf,
    secrets: Option<Arc<dyn SecretStore>>,
    inner: Arc<Inner>,
    /// The keychain writes in flight, for [`Self::flush`].
    writers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// What the keychain writer threads share with the store.
struct Inner {
    path: PathBuf,
    state: Mutex<KeyState>,
}

struct KeyState {
    /// The keys the keychain is known to hold.
    stored: Option<Keys>,
    /// The keys last written OR TRIED, a refused write included — a save writes only when the keys
    /// differ from these, because settings are saved on many ordinary changes, a keychain write may
    /// ask for a password, and a denied prompt must not come back on every save. The next launch
    /// tries again.
    tried: Option<Keys>,
    home: KeyHome,
    /// Keychain writes not yet answered — while any is, the file keeps its keys.
    pending: usize,
    /// The settings last loaded or saved, which a finished write re-saves without its keys.
    last: Option<Settings>,
}

impl SettingsStore {
    /// Settings at `path`, the keys in the file — every test's constructor.
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                path: path.clone(),
                state: Mutex::new(KeyState {
                    stored: None,
                    tried: None,
                    home: KeyHome::File,
                    pending: 0,
                    last: None,
                }),
            }),
            path,
            secrets: None,
            writers: Mutex::new(Vec::new()),
        }
    }

    /// Settings at `path`, the keys in `store` — the keychain's tests, with `secrets::Memory`.
    pub fn with_secrets(path: PathBuf, store: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets: Some(store),
            ..Self::new(path)
        }
    }

    /// `<platform config dir>/rustroom/settings.json`, beside the presets directory, the keys in the
    /// OS keychain.
    ///
    /// ⚠ Not `~/.config` everywhere — see [`crate::presets::PresetStore::user_default`].
    pub fn user_default() -> Self {
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom")
            .join("settings.json");
        Self::with_secrets(path, Arc::new(crate::secrets::Keychain::rustroom()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the Immich keys are kept now — what the Settings window's hover text says.
    pub fn key_home(&self) -> KeyHome {
        self.inner.lock().home
    }

    /// Wait for the keychain writes in flight — what a test checks after; the app never needs to,
    /// because the file keeps the keys until the keychain has them.
    pub fn flush(&self) {
        let writers = std::mem::take(&mut *self.writers.lock().unwrap_or_else(|e| e.into_inner()));
        for w in writers {
            let _ = w.join();
        }
    }

    /// Read the settings. **Infallible**: a missing or malformed file yields the defaults,
    /// because a corrupt settings file must never brick the app — and a keychain that refuses
    /// leaves the keys as the file has them, writing and deleting nothing.
    ///
    /// ⚠ The READ is synchronous: the keys are needed to connect, and on macOS its access prompt —
    /// once per build of an ad-hoc-signed app — comes before the window. Writes are not; see `save`.
    pub fn load(&self) -> Settings {
        let mut s: Settings = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let Some(store) = &self.secrets else {
            return s;
        };
        let file = keys_of(&s);
        if file != Keys::default() {
            // ⚠⚠ The one rule: the file's keys are the newest — move them in, and only then out.
            {
                let mut st = self.inner.lock();
                st.last = Some(s.clone());
                st.tried = Some(file.clone());
                st.pending += 1;
            }
            self.write_in_background(store, file);
            return s;
        }
        let mut st = self.inner.lock();
        match store.read() {
            Ok(Some(k)) => {
                s.immich_key = k.browse.clone();
                s.immich_upload_key = k.upload.clone();
                st.stored = Some(k.clone());
                st.tried = Some(k);
                st.home = KeyHome::Keychain;
            }
            Ok(None) => st.home = KeyHome::Keychain,
            // ⚠ Denied or unavailable: Immich is simply not connected this session.
            Err(_) => st.home = KeyHome::File,
        }
        st.last = Some(s.clone());
        s
    }

    /// Write the settings, creating the directory if needed — the keys to the keychain when they
    /// changed, **in the background**, the file keeping them until the keychain has them.
    ///
    /// ⚠⚠ In the background because a keychain write can wait on a password dialog for as long as
    /// the user takes — 39–91 s measured on the author's Mac (M10.15a); on the window's thread that
    /// froze the app. Called on change rather than on quit: it is one flag in a tiny file, and
    /// quitting is not always graceful.
    pub fn save(&self, s: &Settings) -> Result<(), SettingsError> {
        let Some(store) = &self.secrets else {
            return write_file(&self.path, s, false);
        };
        let keys = keys_of(s);
        let changed = {
            let mut st = self.inner.lock();
            st.last = Some(s.clone());
            let unchanged =
                st.tried.as_ref() == Some(&keys) || (st.tried.is_none() && keys == Keys::default());
            if !unchanged {
                st.tried = Some(keys.clone());
                st.pending += 1;
            }
            let blank = st.pending == 0
                && st.home == KeyHome::Keychain
                && st.stored.as_ref() == Some(&keys);
            write_file(&self.path, s, blank)?;
            !unchanged
        };
        if changed {
            self.write_in_background(store, keys);
        }
        Ok(())
    }

    fn write_in_background(&self, store: &Arc<dyn SecretStore>, keys: Keys) {
        let (store, inner) = (Arc::clone(store), Arc::clone(&self.inner));
        let handle = std::thread::spawn(move || {
            let ok = store.write(&keys).is_ok();
            let mut st = inner.lock();
            st.pending -= 1;
            if ok {
                st.stored = Some(keys);
                st.home = KeyHome::Keychain;
            } else if st.tried.as_ref() == Some(&keys) {
                st.home = KeyHome::File;
            }
            // ⚠⚠ The file loses the keys only now, and only if the keychain holds exactly the ones
            // last saved — a newer change still in flight keeps them in the file.
            if st.pending == 0 && st.home == KeyHome::Keychain {
                if let Some(last) = st.last.clone() {
                    if Some(keys_of(&last)) == st.stored {
                        let _ = write_file(&inner.path, &last, true);
                    }
                }
            }
        });
        self.writers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, KeyState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The file, with both key fields blank when the keychain holds them.
fn write_file(path: &Path, s: &Settings, blank_keys: bool) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = if blank_keys {
        serde_json::to_string_pretty(&Settings {
            immich_key: String::new(),
            immich_upload_key: String::new(),
            ..s.clone()
        })?
    } else {
        serde_json::to_string_pretty(s)?
    };
    std::fs::write(path, text)?;
    Ok(())
}

/// The two keys a settings value carries.
fn keys_of(s: &Settings) -> Keys {
    Keys {
        browse: s.immich_key.clone(),
        upload: s.immich_upload_key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{KeyHome, Keys, Memory, SecretStore};
    use std::sync::Arc;

    fn keys(b: &str, u: &str) -> Keys {
        Keys {
            browse: b.into(),
            upload: u.into(),
        }
    }

    /// A settings value carrying these keys.
    fn with_keys(k: &Keys) -> Settings {
        Settings {
            immich_key: k.browse.clone(),
            immich_upload_key: k.upload.clone(),
            ..Default::default()
        }
    }

    /// The file's two key fields, as written — `None` when the file is absent.
    fn file_keys(path: &Path) -> Option<Keys> {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(keys(&s("immich_key"), &s("immich_upload_key")))
    }

    fn store(dir: &test_dir::TempDir, m: &Arc<Memory>) -> SettingsStore {
        SettingsStore::with_secrets(
            dir.path().join("settings.json"),
            m.clone() as Arc<dyn SecretStore>,
        )
    }

    #[test]
    fn a_saved_file_holds_no_key_and_a_reload_restores_both() {
        let dir = test_dir::TempDir::new("keys-save");
        let m = Arc::new(Memory::new());
        let st = store(&dir, &m);
        let _ = st.load();
        st.save(&with_keys(&keys("b", "u"))).unwrap();
        st.flush();
        assert_eq!(
            file_keys(&st.path),
            Some(Keys::default()),
            "the file holds no key"
        );
        assert_eq!(m.item(), Some(keys("b", "u")));
        let back = store(&dir, &m).load();
        assert_eq!(
            (back.immich_key.as_str(), back.immich_upload_key.as_str()),
            ("b", "u")
        );
        assert_eq!(st.key_home(), KeyHome::Keychain);
    }

    #[test]
    fn legacy_plaintext_keys_move_into_the_keychain() {
        let dir = test_dir::TempDir::new("keys-migrate");
        let path = dir.path().join("settings.json");
        SettingsStore::new(path.clone())
            .save(&with_keys(&keys("old-b", "old-u")))
            .unwrap();
        assert_eq!(
            file_keys(&path),
            Some(keys("old-b", "old-u")),
            "fixture check"
        );
        let m = Arc::new(Memory::new());
        let st = store(&dir, &m);
        let s = st.load();
        st.flush();
        assert_eq!(
            (s.immich_key.as_str(), s.immich_upload_key.as_str()),
            ("old-b", "old-u")
        );
        assert_eq!(m.item(), Some(keys("old-b", "old-u")));
        assert_eq!(
            file_keys(&path),
            Some(Keys::default()),
            "the file lost them"
        );
        assert_eq!(st.key_home(), KeyHome::Keychain);
    }

    #[test]
    fn a_failed_write_keeps_the_keys_in_the_file() {
        let dir = test_dir::TempDir::new("keys-fallback");
        let path = dir.path().join("settings.json");
        SettingsStore::new(path.clone())
            .save(&with_keys(&keys("b", "u")))
            .unwrap();
        let m = Arc::new(Memory::new());
        m.fail_writes(true);
        let st = store(&dir, &m);
        let s = st.load();
        assert_eq!(s.immich_key, "b");
        // ⚠ Checked right after `load`, not only after a save: a save writes the keys back and would
        // hide a load that blanked the file first — a crash in between would lose them.
        assert_eq!(
            file_keys(&path),
            Some(keys("b", "u")),
            "the load dropped them"
        );
        st.flush();
        assert_eq!(
            file_keys(&path),
            Some(keys("b", "u")),
            "the refused write dropped them"
        );
        st.save(&s).unwrap();
        st.flush();
        assert_eq!(file_keys(&path), Some(keys("b", "u")), "never dropped");
        assert_eq!(store(&dir, &m).load().immich_upload_key, "u");
        assert_eq!(st.key_home(), KeyHome::File);
    }

    /// ⚠ A refused write (a denied prompt) is tried once, not on every ordinary save after it.
    #[test]
    fn a_refused_write_is_not_retried_on_every_save() {
        let dir = test_dir::TempDir::new("keys-no-retry");
        SettingsStore::new(dir.path().join("settings.json"))
            .save(&with_keys(&keys("b", "u")))
            .unwrap();
        let m = Arc::new(Memory::new());
        m.fail_writes(true);
        let st = store(&dir, &m);
        let mut s = st.load();
        for i in 0..5 {
            s.panel_sections.folders = i % 2 == 0;
            st.save(&s).unwrap();
        }
        st.flush();
        assert_eq!(m.attempts(), 1, "the refused write was retried");
    }

    #[test]
    fn a_failed_read_writes_and_deletes_nothing() {
        let dir = test_dir::TempDir::new("keys-denied");
        let path = dir.path().join("settings.json");
        SettingsStore::new(path.clone())
            .save(&Settings::default())
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let m = Arc::new(Memory::with_item(keys("b", "u")));
        m.fail_reads(true);
        let st = store(&dir, &m);
        let s = st.load();
        assert_eq!(
            (s.immich_key.as_str(), s.immich_upload_key.as_str()),
            ("", "")
        );
        st.save(&s).unwrap();
        st.flush();
        assert_eq!(m.writes(), 0, "nothing written");
        assert_eq!(m.item(), Some(keys("b", "u")), "the item is untouched");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the file is untouched"
        );
    }

    #[test]
    fn the_files_keys_win_over_an_older_item() {
        let dir = test_dir::TempDir::new("keys-file-wins");
        let path = dir.path().join("settings.json");
        SettingsStore::new(path.clone())
            .save(&with_keys(&keys("new-b", "new-u")))
            .unwrap();
        let m = Arc::new(Memory::with_item(keys("old-b", "old-u")));
        let st = store(&dir, &m);
        let s = st.load();
        st.flush();
        assert_eq!(s.immich_key, "new-b");
        assert_eq!(m.item(), Some(keys("new-b", "new-u")));
        assert_eq!(file_keys(&path), Some(Keys::default()));
    }

    #[test]
    fn ordinary_saves_do_not_touch_the_keychain() {
        let dir = test_dir::TempDir::new("keys-storm");
        let m = Arc::new(Memory::with_item(keys("b", "u")));
        let st = store(&dir, &m);
        let mut s = st.load();
        let writes = m.writes();
        for i in 0..10 {
            s.panel_sections.folders = i % 2 == 0;
            st.save(&s).unwrap();
        }
        st.flush();
        assert_eq!(
            m.writes(),
            writes,
            "the keychain was written on an ordinary save"
        );
    }

    #[test]
    fn a_cleared_key_stays_cleared() {
        let dir = test_dir::TempDir::new("keys-cleared");
        let m = Arc::new(Memory::with_item(keys("b", "u")));
        let st = store(&dir, &m);
        let mut s = st.load();
        assert_eq!(s.immich_key, "b", "fixture check");
        s.immich_key.clear();
        st.save(&s).unwrap();
        st.flush();
        let back = store(&dir, &m).load();
        assert_eq!(
            back.immich_key, "",
            "the keychain resurrected a cleared key"
        );
        assert_eq!(back.immich_upload_key, "u");
    }

    /// ⚠⚠ A keychain write can wait on a password dialog for as long as the user takes — measured at
    /// 39–91 s on the author's Mac. A save must return at once, the keys staying in the file until the
    /// keychain has them.
    #[test]
    fn a_slow_keychain_does_not_hold_up_a_save() {
        let dir = test_dir::TempDir::new("keys-slow");
        let m = Arc::new(Memory::new());
        m.set_delay(std::time::Duration::from_millis(400));
        let st = store(&dir, &m);
        let _ = st.load();
        let t = std::time::Instant::now();
        st.save(&with_keys(&keys("b", "u"))).unwrap();
        assert!(
            t.elapsed() < std::time::Duration::from_millis(200),
            "the save waited for the keychain"
        );
        assert_eq!(
            file_keys(&st.path),
            Some(keys("b", "u")),
            "kept in the file meanwhile"
        );
        st.flush();
        assert_eq!(file_keys(&st.path), Some(Keys::default()));
        assert_eq!(m.item(), Some(keys("b", "u")));
    }

    #[test]
    fn new_has_no_keychain() {
        let dir = test_dir::TempDir::new("keys-plain");
        let path = dir.path().join("settings.json");
        let st = SettingsStore::new(path.clone());
        assert_eq!(st.key_home(), KeyHome::File);
        st.save(&with_keys(&keys("b", "u"))).unwrap();
        assert_eq!(file_keys(&path), Some(keys("b", "u")));
    }
}

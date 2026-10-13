//! The Immich API keys in the OS credential store (M10.15a) — macOS Keychain, Windows Credential
//! Manager, Linux Secret Service — instead of plaintext `settings.json`.
//!
//! ⚠⚠ **One item for both keys** (service `Rustroom`, account `immich`, the keys as JSON): on macOS a
//! build without a Team ID asks for the login password once per item after every rebuild (the item's
//! partition list keys on each build's cdhash — even the "Rustroom Dev" signing), so one item is one
//! prompt.
//! `settings::store::SettingsStore` is the only user, and only its `user_default()` — the app's —
//! reaches the real keychain; every test's `SettingsStore::new` has no store at all.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// What the keychain item holds: the read-only browse key and the upload-capable key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Keys {
    pub browse: String,
    pub upload: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SecretError(pub String);

/// Where the keys are kept.
pub trait SecretStore: Send + Sync {
    /// The stored keys — `Ok(None)` when nothing is stored yet.
    fn read(&self) -> Result<Option<Keys>, SecretError>;
    fn write(&self, keys: &Keys) -> Result<(), SecretError>;
}

/// Where the settings' keys currently live — what the Settings window's hover text says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHome {
    Keychain,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Browse,
    Upload,
}

/// The key fields' hover sentence, saying where the key is kept.
pub fn key_hover(kind: KeyKind, home: KeyHome) -> &'static str {
    match (kind, home) {
        (KeyKind::Browse, KeyHome::Keychain) => "A read-only Immich API key, kept in your keychain",
        (KeyKind::Browse, KeyHome::File) => {
            "A read-only Immich API key, kept in plain text in settings.json"
        }
        (KeyKind::Upload, KeyHome::Keychain) => {
            "An optional Immich key that can upload and change marks, for Export and Send marks, kept in your keychain"
        }
        (KeyKind::Upload, KeyHome::File) => {
            "An optional Immich key that can upload and change marks, for Export and Send marks, kept in settings.json"
        }
    }
}

/// The OS credential store, through the `keyring` crate.
pub struct Keychain {
    service: String,
    account: String,
}

impl Keychain {
    /// Rustroom's one item.
    pub fn rustroom() -> Self {
        Self {
            service: "Rustroom".into(),
            account: "immich".into(),
        }
    }
}

impl SecretStore for Keychain {
    fn read(&self) -> Result<Option<Keys>, SecretError> {
        let entry = self.entry()?;
        match entry.get_password() {
            Ok(text) => decode(&text).map(Some),
            // ⚠ "Nothing stored yet" is not a failure — it is the first launch.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }

    fn write(&self, keys: &Keys) -> Result<(), SecretError> {
        let text = serde_json::to_string(keys).map_err(|e| SecretError(e.to_string()))?;
        self.entry()?
            .set_password(&text)
            .map_err(|e| SecretError(e.to_string()))
    }
}

impl Keychain {
    fn entry(&self) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, &self.account).map_err(|e| SecretError(e.to_string()))
    }
}

/// The item's text as keys — a malformed item is a read failure, never a panic.
fn decode(text: &str) -> Result<Keys, SecretError> {
    serde_json::from_str(text).map_err(|e| SecretError(format!("an unreadable keychain item: {e}")))
}

/// An in-memory store for tests, which can be made to fail.
#[derive(Default)]
pub struct Memory {
    item: Mutex<Option<Keys>>,
    fail_reads: AtomicBool,
    fail_writes: AtomicBool,
    writes: AtomicUsize,
    attempts: AtomicUsize,
    /// How long a write takes — a keychain waiting on a password dialog, in miniature.
    delay: Mutex<std::time::Duration>,
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_item(keys: Keys) -> Self {
        let m = Self::default();
        *m.item.lock().unwrap() = Some(keys);
        m
    }

    pub fn fail_reads(&self, on: bool) {
        self.fail_reads.store(on, Ordering::SeqCst);
    }

    pub fn fail_writes(&self, on: bool) {
        self.fail_writes.store(on, Ordering::SeqCst);
    }

    /// How many writes succeeded.
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    /// How many writes were tried, refused ones included.
    pub fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    pub fn set_delay(&self, d: std::time::Duration) {
        *self.delay.lock().unwrap() = d;
    }

    pub fn item(&self) -> Option<Keys> {
        self.item.lock().unwrap().clone()
    }
}

impl SecretStore for Memory {
    fn read(&self) -> Result<Option<Keys>, SecretError> {
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(SecretError("reading was refused".into()));
        }
        Ok(self.item())
    }

    fn write(&self, keys: &Keys) -> Result<(), SecretError> {
        // ⚠ The delay is taken BEFORE the attempt is counted, so a test that sees the count knows
        // this write's delay is fixed and may change it for the next.
        let delay = *self.delay.lock().unwrap();
        self.attempts.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(delay);
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(SecretError("writing was refused".into()));
        }
        *self.item.lock().unwrap() = Some(keys.clone());
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(b: &str, u: &str) -> Keys {
        Keys {
            browse: b.into(),
            upload: u.into(),
        }
    }

    #[test]
    fn memory_round_trips_and_counts_writes() {
        let m = Memory::new();
        assert_eq!(m.read().unwrap(), None);
        m.write(&keys("b", "u")).unwrap();
        assert_eq!(m.read().unwrap(), Some(keys("b", "u")));
        assert_eq!(m.writes(), 1);
    }

    #[test]
    fn a_memory_store_can_fail_on_demand() {
        let m = Memory::with_item(keys("b", "u"));
        m.fail_reads(true);
        assert!(m.read().is_err());
        m.fail_writes(true);
        assert!(m.write(&keys("x", "y")).is_err());
        assert_eq!(
            m.item(),
            Some(keys("b", "u")),
            "a failed write changes nothing"
        );
        assert_eq!(m.writes(), 0);
    }

    #[test]
    fn a_malformed_item_is_a_read_failure() {
        assert!(decode("not json").is_err());
        assert!(decode("{\"browse\": 1}").is_err());
        let k = keys("b", "u");
        assert_eq!(decode(&serde_json::to_string(&k).unwrap()).unwrap(), k);
    }

    #[test]
    fn every_key_hover_is_one_short_sentence() {
        for kind in [KeyKind::Browse, KeyKind::Upload] {
            for (home, place) in [
                (KeyHome::Keychain, "keychain"),
                (KeyHome::File, "settings.json"),
            ] {
                let t = key_hover(kind, home);
                assert_eq!(crate::slider_help::sentence_fault(t), None, "{t}");
                assert!(t.contains(place), "{t}");
            }
        }
    }

    /// M10.15d: the upload key is the Write key — its hover names both deliberate uses.
    #[test]
    fn the_write_key_hover_names_both_uses() {
        for home in [KeyHome::Keychain, KeyHome::File] {
            let t = key_hover(KeyKind::Upload, home);
            assert!(t.contains("Export") && t.contains("Send marks"), "{t}");
        }
    }

    /// ⚠ Touches the REAL keychain, under a throwaway account — run by hand:
    /// `cargo test --workspace the_real_keychain_round_trips -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_keychain_round_trips() {
        let k = Keychain {
            service: "Rustroom-test".into(),
            account: "throwaway".into(),
        };
        let want = keys("browse-test", "upload-test");
        k.write(&want).unwrap();
        assert_eq!(k.read().unwrap(), Some(want));
        keyring::Entry::new("Rustroom-test", "throwaway")
            .unwrap()
            .delete_credential()
            .unwrap();
        assert_eq!(k.read().unwrap(), None);
    }
}

//! What a **rendered** preview is of: a photo at a version, the edits it was rendered with,
//! and the demosaic tier — and the one rule that says a photo is untouched (M9.14).
//!
//! ⚠ The edits fingerprint here is **stable** (FNV-1a over the serialized edits), unlike
//! `thumb_key::edits_fingerprint`, because this key names a file on disk. `DefaultHasher`
//! may change between toolchains, which would turn every stored preview into a miss — the
//! trap `thumb_store::fnv1a`'s doc records.

use crate::photo_id::PhotoId;
use crate::thumb_store::{fnv1a, Identity};
use edit_model::Edits;
use raw_decode::Demosaic;

/// A stable fingerprint of one copy's edits, for a file name.
///
/// ⚠ Exhaustive by construction, like `edits_fingerprint`: hashed from the serialized edits,
/// so a field added to `Edits` later is part of it without anyone remembering.
pub fn stable_fingerprint(edits: &Edits) -> u64 {
    match serde_json::to_vec(edits) {
        Ok(bytes) => fnv1a(&bytes),
        Err(_) => 0,
    }
}

/// Untouched means **equal to the edits a fresh open assigns** — `Settings::initial_edits(None)`:
/// the camera rendering and the lens decision from the settings, everything else default.
///
/// ⚠ Not the sidecar's `is_neutral`, which compares against `Edits::default()` and would call
/// every freshly opened photo edited, since a fresh photo carries the camera rendering —
/// `a_fresh_photo_is_untouched_while_the_sidecar_rule_calls_it_edited` pins the difference.
pub fn is_untouched(edits: &Edits, fresh: &Edits) -> bool {
    edits == fresh
}

/// What a rendered preview on disk is of.
///
/// ⚠ A different tier is a different preview: a file built at Fast must not stand for one the
/// user asked for at Better. Old ones age out by the store's budget; nothing deletes them eagerly.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PreviewKey {
    pub id: PhotoId,
    pub edits: u64,
    pub tier: Demosaic,
}

impl PreviewKey {
    pub fn new(id: PhotoId, edits: &Edits, tier: Demosaic) -> Self {
        Self {
            id,
            edits: stable_fingerprint(edits),
            tier,
        }
    }

    /// The hash the file is named by: the file's identity (path, mtime, size — a re-saved
    /// original is a different photo), the copy, the edits and the tier.
    ///
    /// ⚠ A file that cannot be stat'ed keys on its path alone rather than failing: the worst
    /// case is a preview that outlives a rewrite of a file the store could not see anyway.
    pub fn file_stem(&self) -> String {
        let mut bytes = match Identity::of(&self.id.path) {
            Some(i) => i.key_bytes(),
            None => self.id.path.to_string_lossy().as_bytes().to_vec(),
        };
        bytes.extend_from_slice(&self.id.copy.to_le_bytes());
        bytes.extend_from_slice(&self.edits.to_le_bytes());
        bytes.extend_from_slice(self.tier.label().as_bytes());
        format!("{:016x}", fnv1a(&bytes))
    }

    pub fn file_name(&self) -> String {
        format!("{}.jpg", self.file_stem())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "rustroom_preview_key_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fresh_photo_is_untouched_while_the_sidecar_rule_calls_it_edited() {
        let fresh = Settings::default().initial_edits(None);
        assert!(is_untouched(&fresh, &fresh));
        // ⚠ The fixture check: the two rules must disagree, or this test proves nothing.
        assert_ne!(
            fresh,
            Edits::default(),
            "a fresh photo carries the camera rendering"
        );
        let mut touched = fresh.clone();
        touched.exposure = 0.3;
        assert!(!is_untouched(&touched, &fresh));
    }

    #[test]
    fn the_file_name_changes_with_the_edits_and_the_tier_and_not_otherwise() {
        let dir = TempDir::new("name");
        let path = dir.0.join("a.RAF");
        std::fs::write(&path, b"raw").unwrap();
        let id = PhotoId::original(path);
        let a = PreviewKey::new(id.clone(), &Edits::default(), Demosaic::Better);
        let b = PreviewKey::new(id.clone(), &Edits::default(), Demosaic::Better);
        assert_eq!(a.file_name(), b.file_name());
        assert!(a.file_name().ends_with(".jpg"));
        assert_eq!(
            a.file_stem().len(),
            16,
            "sixteen hex digits: {}",
            a.file_stem()
        );
        let c = PreviewKey::new(id.clone(), &Edits::default(), Demosaic::Fast);
        assert_ne!(a.file_name(), c.file_name(), "the tier is part of the key");
        let mut e = Edits::default();
        e.contrast = 10.0;
        let d = PreviewKey::new(id.clone(), &e, Demosaic::Better);
        assert_ne!(
            a.file_name(),
            d.file_name(),
            "the edits are part of the key"
        );
        let copy = PreviewKey::new(
            PhotoId::new(id.path.clone(), 1),
            &Edits::default(),
            Demosaic::Better,
        );
        assert_ne!(
            a.file_name(),
            copy.file_name(),
            "the copy is part of the key"
        );
    }

    #[test]
    fn the_fingerprint_is_stable_across_calls_and_pinned() {
        let a = stable_fingerprint(&Edits::default());
        assert_eq!(a, stable_fingerprint(&Edits::default()));
        // ⚠ Pinned: a change here means every preview on every user's disk becomes a miss,
        // which must be a decision (a bumped `Edits` serialization), never an accident.
        assert_eq!(a, 0xa833_78a2_2a37_ba56, "fingerprint {a:#018x}");
    }
}

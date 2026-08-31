//! What a cached thumbnail is *of*: a photo, and the edits it was rendered with.

use crate::photo_id::PhotoId;
use edit_model::Edits;

/// A fingerprint of one copy's edits, for the thumbnail cache key.
///
/// ⚠⚠ Hashed from the **serialized** edits rather than field by field, and that is the point: it
/// is exhaustive by construction. `Edits` has around forty fields and cannot derive `Hash` (it
/// holds `f32`), so a hand-written hash would silently omit any field added later — and the
/// symptom is a *stale thumbnail*, which nothing would notice. Same reasoning as
/// `Edits::merge_from`'s exhaustive destructure.
pub fn edits_fingerprint(edits: &Edits) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_vec(edits) {
        Ok(bytes) => bytes.hash(&mut h),
        // ⚠ Impossible for `Edits` in practice. It must still vary per call rather than return a
        // constant: a constant would make every photo collide on one key and show each other's
        // thumbnails, which is far worse than a needless re-render.
        Err(_) => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            N.fetch_add(1, Ordering::Relaxed).hash(&mut h);
        }
    }
    h.finish()
}

/// What a cached thumbnail is of.
///
/// ⚠⚠ **The `edits` half is what makes this feature work with no invalidation code.** A changed
/// edit is simply a different key, so a sync, a paste, a preset click, an undo, a redo and a
/// snapshot restore are all correct without any of them knowing this cache exists. That is
/// `PrefixKey`'s pattern, which deliberately fails toward a miss.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ThumbKey {
    pub id: PhotoId,
    pub edits: u64,
}

impl ThumbKey {
    pub fn new(id: PhotoId, edits: &Edits) -> Self {
        Self {
            id,
            edits: edits_fingerprint(edits),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(p: &str) -> PhotoId {
        PhotoId::original(PathBuf::from(p))
    }

    #[test]
    fn the_same_edits_give_the_same_fingerprint() {
        assert_eq!(
            edits_fingerprint(&Edits::default()),
            edits_fingerprint(&Edits::default())
        );
    }

    /// ⚠⚠ **The exhaustiveness lives in the IMPLEMENTATION, not in this test, and it is worth
    /// being clear about that.** `edits_fingerprint` hashes the *serialization*, so a field added
    /// to `Edits` is included automatically — no test could enumerate forty fields and stay
    /// honest. What this verifies is that the implementation really is the serialization-based
    /// one, by moving fields a hand-written hash would plausibly forget: a deep HSL band, and two
    /// nested geometry values.
    #[test]
    fn an_obscure_nested_field_still_changes_the_fingerprint() {
        let base = edits_fingerprint(&Edits::default());

        let mut deep_band = Edits::default();
        deep_band.hsl.bands[7].luminance = 12.0;
        assert_ne!(
            edits_fingerprint(&deep_band),
            base,
            "the eighth HSL band's luminance must count"
        );

        let mut cropped = Edits::default();
        cropped.geometry.crop.left = 0.1;
        assert_ne!(
            edits_fingerprint(&cropped),
            base,
            "a nested crop edge must count"
        );

        let mut angled = Edits::default();
        angled.geometry.angle = 0.25;
        assert_ne!(edits_fingerprint(&angled), base, "straighten must count");
    }

    /// ⚠ The contrapositive, which is what makes the cache usable at all: identical edits must
    /// fingerprint identically, or every frame would be a miss and the grid would re-render
    /// forever.
    #[test]
    fn a_clone_fingerprints_identically() {
        let mut e = Edits::default();
        e.hsl.bands[3].hue = -8.0;
        e.exposure = 0.75;
        assert_eq!(edits_fingerprint(&e), edits_fingerprint(&e.clone()));
    }

    /// ⚠⚠ **The edits half, isolated.** The two "different keys" tests below also differ by copy
    /// number, so a constant fingerprint passes them — found by the sabotage step, and exactly
    /// the shape this project keeps recording: a test that names a property it cannot see. This
    /// one holds the `PhotoId` fixed, so only the edits can distinguish the keys.
    #[test]
    fn one_photo_with_two_different_edits_gets_two_keys() {
        let same = id("/a.RAF");
        let bw = Edits {
            saturation: -100.0,
            ..Default::default()
        };
        assert_ne!(
            ThumbKey::new(same.clone(), &Edits::default()),
            ThumbKey::new(same, &bw)
        );
    }

    /// The whole feature: two copies of one file with different edits are different keys.
    #[test]
    fn two_copies_with_different_edits_get_different_keys() {
        let path = PathBuf::from("/a.RAF");
        let bw = Edits {
            saturation: -100.0,
            ..Default::default()
        };
        assert_ne!(
            ThumbKey::new(PhotoId::new(path.clone(), 0), &Edits::default()),
            ThumbKey::new(PhotoId::new(path, 1), &bw)
        );
    }

    /// ⚠ …and two copies with the *same* edits still differ, or a copy would show its sibling's
    /// thumbnail. The `id` half of the key is load-bearing even though the edits half does the
    /// interesting work.
    #[test]
    fn two_copies_with_identical_edits_still_get_different_keys() {
        let path = PathBuf::from("/a.RAF");
        let e = Edits::default();
        assert_ne!(
            ThumbKey::new(PhotoId::new(path.clone(), 0), &e),
            ThumbKey::new(PhotoId::new(path, 1), &e)
        );
    }

    #[test]
    fn different_photos_with_the_same_edits_get_different_keys() {
        let e = Edits::default();
        assert_ne!(
            ThumbKey::new(id("/a.RAF"), &e),
            ThumbKey::new(id("/b.RAF"), &e)
        );
    }
}

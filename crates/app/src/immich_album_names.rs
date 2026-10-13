//! Each Immich album's cache file names, derived once per asset list, so the rail's download count
//! costs one set lookup per asset per frame.
//!
//! ⚠⚠ **Why it exists (2026-10-12):** every album row asks for its count on EVERY Library frame.
//! Over 12 albums of 3,538 assets that meant, per frame, a JSON parse of each stored list and a
//! file-name derivation per asset — after a sidecar `stat` per asset had already been removed.
//!
//! ⚠⚠ **What is downloaded is NOT kept here.** It is read live from the `present` set, so a
//! download or a removal shows at once and no writer has to invalidate anything. Only the names
//! are kept, and they depend on the list alone — keyed by the list's content, or for a stored list
//! by the validator that decides whether it is current at all.
//!
//! ⚠ No egui.

use std::collections::HashMap;
use std::ffi::OsString;

/// Which list a set of names was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKey {
    /// A list fetched this session, by [`fingerprint`] — so a refetch that changed it is a miss.
    Session(u64),
    /// The stored list, by the validator the rail's album list gives it (`None` before that list
    /// has arrived). ⚠ A stored list is only ever written together with the session map, whose key
    /// then takes over, so a "not in hand" kept under one validator cannot outlive a fetch.
    Stored(Option<crate::immich_list_cache::Validator>),
}

/// What the names of a list depend on: each asset's id, type and original file name — never its
/// dates or thumbhash, which a refetch changes without changing a single cache file name.
pub fn fingerprint(assets: &[immich::Asset]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    assets.len().hash(&mut h);
    for a in assets {
        a.id.hash(&mut h);
        a.original_file_name.hash(&mut h);
        std::mem::discriminant(&a.asset_type).hash(&mut h);
    }
    h.finish()
}

/// The names per album, each with the key of the list it came from.
#[derive(Debug, Default)]
pub struct AlbumNames {
    by_album: HashMap<String, (ListKey, Option<Vec<OsString>>)>,
}

impl AlbumNames {
    /// Album `id`'s names for the list `key` identifies, or `None` when that list is not in hand.
    /// `derive` runs only when `key` differs from the key the album's names were derived from.
    pub fn names(
        &mut self,
        id: &str,
        key: ListKey,
        derive: impl FnOnce() -> Option<Vec<OsString>>,
    ) -> Option<&[OsString]> {
        let current = matches!(self.by_album.get(id), Some((k, _)) if *k == key);
        if !current {
            let names = derive();
            self.by_album.insert(id.to_string(), (key, names));
        }
        self.by_album.get(id).and_then(|(_, n)| n.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: &str, name: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: Some(100),
            height: Some(100),
            thumbhash: None,
            file_modified_at: "2025-08-14T09:21:33.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            is_favorite: false,
            exif_info: None,
        }
    }

    fn some(names: &[&str]) -> Option<Vec<OsString>> {
        Some(names.iter().map(OsString::from).collect())
    }

    #[test]
    fn names_are_derived_once_per_list() {
        let mut m = AlbumNames::default();
        let mut runs = 0;
        for _ in 0..3 {
            let got = m.names("al", ListKey::Session(7), || {
                runs += 1;
                some(&["a.jpg"])
            });
            assert_eq!(got.map(<[_]>::len), Some(1));
        }
        assert_eq!(runs, 1, "the same list must not be derived again");
        m.names("al", ListKey::Session(8), || {
            runs += 1;
            some(&["a.jpg", "b.jpg"])
        });
        assert_eq!(runs, 2, "a changed list is a miss");
        assert_eq!(
            m.names("al", ListKey::Session(8), || unreachable!())
                .map(<[_]>::len),
            Some(2)
        );
    }

    /// ⚠ A stored list that is not current reads as "not in hand" — and stays that way without
    /// re-reading the file every frame, until the rail's validator moves.
    #[test]
    fn a_list_not_in_hand_is_not_retried_until_its_key_moves() {
        let mut m = AlbumNames::default();
        let mut runs = 0;
        for _ in 0..3 {
            assert!(m
                .names("al", ListKey::Stored(None), || {
                    runs += 1;
                    None
                })
                .is_none());
        }
        assert_eq!(runs, 1);
        let v = crate::immich_list_cache::Validator::Album {
            asset_count: 2,
            last_modified: None,
        };
        assert!(m
            .names("al", ListKey::Stored(Some(v)), || some(&["a.jpg"]))
            .is_some());
    }

    /// Two albums never share an entry.
    #[test]
    fn each_album_has_its_own_names() {
        let mut m = AlbumNames::default();
        m.names("one", ListKey::Session(1), || some(&["a.jpg"]));
        let two = m.names("two", ListKey::Session(1), || some(&["b.jpg", "c.jpg"]));
        assert_eq!(two.map(<[_]>::len), Some(2));
    }

    #[test]
    fn the_fingerprint_follows_what_the_names_depend_on_and_nothing_else() {
        let base = vec![photo("a", "A.JPG"), photo("b", "B.RAF")];
        let mut renamed = base.clone();
        renamed[1].original_file_name = "B.MOV".into();
        let mut retyped = base.clone();
        retyped[0].asset_type = immich::AssetType::Video;
        let mut grown = base.clone();
        grown.push(photo("c", "C.JPG"));
        let mut touched = base.clone();
        touched[0].updated_at = "2027-01-01T00:00:00.000Z".into();
        touched[0].thumbhash = Some("y".into());

        let f = fingerprint(&base);
        assert_ne!(f, fingerprint(&renamed), "the extension decides the name");
        assert_ne!(
            f,
            fingerprint(&retyped),
            "the type decides whether it is shown"
        );
        assert_ne!(f, fingerprint(&grown));
        assert_eq!(
            f,
            fingerprint(&touched),
            "a refetch that changed only dates must not re-derive"
        );
    }
}

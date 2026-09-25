//! The server's stacks, reduced to what the grid needs and kept on disk.
//!
//! ⚠⚠ **A projection, not the reply.** `GET /stacks` is **6.6 MB for 2,379 stacks, measured twice at
//! ~2.9 s**, because it embeds the full asset DTO for every member. What groups a grid is two UUIDs
//! per member, which is roughly **350 KB** — so that is what is stored and what is loaded.
//!
//! ⚠⚠ **There is no validator, and that is a limitation rather than an oversight.** Asset lists
//! revalidate on `(assetCount, lastModifiedAssetTimestamp)`; `StackResponseDto` carries only `id`,
//! `primaryAssetId` and `assets`, so there is nothing to compare. Freshness therefore rests on
//! **⋯ → Reindex** dropping the file, which is how asset lists behave and the model the user already
//! has. A stack made in Immich does not appear until then (the user's call, of three options).
//!
//! ⚠ A failed or absent fetch means **no grouping**, never an error and never a stale guess: the grid
//! shows both halves, exactly as before this existed. A cache that can stop a folder opening is worse
//! than no cache — `immich_list_cache`'s rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Which stack each asset belongs to, and how large each stack is.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Projection {
    /// Asset id → stack id.
    pub of_asset: HashMap<String, String>,
    /// Stack id → member count. ⚠ Always ≥ 2; see [`project`].
    pub counts: HashMap<String, usize>,
}

impl Projection {
    /// The stack an asset belongs to, if any.
    pub fn stack_of(&self, asset_id: &str) -> Option<&str> {
        self.of_asset.get(asset_id).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.of_asset.is_empty()
    }
}

/// Reduce the server's reply to the projection.
///
/// ⚠⚠ **A stack with fewer than two members is DROPPED.** `stack::Stack::count`'s own doc forbids a
/// badge reading "1", and a one-member stack would produce exactly that. Measured on the real server
/// every stack has exactly 2 — but that is a fact about this server today, not a guarantee from the
/// API, so the code does not depend on it.
///
/// ⚠ An asset listed in two stacks takes the **last** one seen. Also not supposed to happen — 4,758
/// member ids were all distinct — but "deterministic" beats "whichever the hash map happened to keep"
/// if a server ever disagrees.
pub fn project(stacks: &[immich::Stack]) -> Projection {
    let mut p = Projection::default();
    for s in stacks {
        if s.assets.len() < 2 {
            continue;
        }
        p.counts.insert(s.id.clone(), s.assets.len());
        for m in &s.assets {
            p.of_asset.insert(m.id.clone(), s.id.clone());
        }
    }
    p
}

/// Where a server's projection lives.
///
/// ⚠ A **sibling** of the cached originals, for `immich_list_cache`'s reason: `is_immich_path` is
/// `starts_with(root)` and `asset_id_of` is a path stem, so a file inside the originals directory
/// would be mistaken for an asset.
pub fn stacks_path(base_url: &str) -> PathBuf {
    let root = crate::immich_source::cache_root(base_url);
    let name = root
        .file_name()
        .map(|n| format!("{}-stacks.json", n.to_string_lossy()))
        .unwrap_or_else(|| "stacks.json".to_string());
    root.with_file_name(name)
}

/// Read the stored projection, or `None` when there is none or it cannot be read.
pub fn load(path: &Path) -> Option<Projection> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Store the projection. ⚠ Best effort: a cache that cannot be written must not fail anything.
pub fn store(path: &Path, p: &Projection) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(p) {
        let _ = std::fs::write(path, text);
    }
}

/// Forget the stored projection, so the next need refetches it. ⚠ Reindex's half of freshness.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Temp-dir isolation on pid **and** a counter — `immich_list_cache`'s pattern, not
    /// `immich_source`'s pid-only one.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "rustroom_stackcache_{tag}_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("stacks.json")
    }

    fn stack(id: &str, members: &[&str]) -> immich::Stack {
        // ⚠ Built through the DTO's own deserialiser, so a field rename cannot make these fixtures
        // silently diverge from what the client actually parses.
        let assets: Vec<String> = members
            .iter()
            .map(|m| format!(r#"{{"id":"{m}"}}"#))
            .collect();
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","primaryAssetId":"{}","assets":[{}]}}"#,
            members.first().copied().unwrap_or(""),
            assets.join(",")
        ))
        .unwrap()
    }

    #[test]
    fn the_projection_maps_every_member_to_its_stack() {
        let p = project(&[stack("s1", &["a", "b"]), stack("s2", &["c", "d"])]);
        assert_eq!(p.of_asset.len(), 4, "four members across two stacks");
        assert_eq!(p.stack_of("a"), Some("s1"));
        assert_eq!(p.stack_of("d"), Some("s2"));
        assert_eq!(p.stack_of("nope"), None);
        assert_eq!(p.counts.get("s1"), Some(&2));
    }

    /// ⚠⚠ A one-member stack would render a badge reading "1", which `stack::Stack::count` forbids.
    #[test]
    fn a_stack_of_one_is_dropped() {
        let p = project(&[stack("lonely", &["a"]), stack("real", &["b", "c"])]);
        assert!(
            p.stack_of("a").is_none(),
            "a one-member stack must not group"
        );
        assert!(!p.counts.contains_key("lonely"));
        // ⚠ The control: the real stack in the same reply is kept, so this is about the size rule
        // rather than about `project` dropping everything.
        assert_eq!(p.stack_of("b"), Some("real"));
    }

    /// ⚠ Not supposed to happen — 4,758 member ids were all distinct on the real server — but a
    /// malformed reply must give one answer rather than an arbitrary one.
    #[test]
    fn an_asset_in_two_stacks_resolves_to_the_last() {
        let p = project(&[stack("first", &["x", "y"]), stack("second", &["x", "z"])]);
        assert_eq!(p.stack_of("x"), Some("second"));
    }

    #[test]
    fn a_projection_round_trips_through_the_file() {
        let path = temp("rt");
        let p = project(&[stack("s", &["a", "b"])]);
        store(&path, &p);
        assert_eq!(load(&path).unwrap(), p);

        clear(&path);
        assert!(
            load(&path).is_none(),
            "clear must make the next need refetch"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// ⚠ Every failure is a **miss**, never an error: an absent or corrupt file means no grouping.
    #[test]
    fn an_unreadable_cache_is_a_miss() {
        let path = temp("bad");
        assert!(load(&path).is_none(), "absent is a miss");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load(&path).is_none(), "corrupt is a miss, not a panic");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// ⚠⚠ A **sibling** of the originals directory, never inside it: `is_immich_path` is
    /// `starts_with(root)`, so a file within would be read as an asset.
    #[test]
    fn the_file_sits_beside_the_originals_not_inside_them() {
        let path = stacks_path("https://immich.example");
        let root = crate::immich_source::cache_root("https://immich.example");
        assert!(
            !path.starts_with(&root),
            "{path:?} must not be inside {root:?}"
        );
        assert_eq!(path.parent(), root.parent());
    }
}

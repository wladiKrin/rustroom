//! AI maps on disk (M9.13): computed once per photo and prompt, kept across sessions.
//!
//! ⚠ Keyed on the FILE (path, size, mtime), the model and the prompt — not on the decode's size,
//! because the map is stored in normalised decode coordinates, so a preview decode and a full
//! decode of one file share it.

use ai_mask::{ModelId, ProbMap};
use edit_model::AiPrompt;
use std::path::{Path, PathBuf};

/// The map cache's directory.
pub fn dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("rustroom").join("ai_masks"))
}

/// Where downloaded models live — data, not cache: 45–225 MB each is not something to re-fetch
/// because a cache was cleared.
pub fn models_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("rustroom").join("models"))
}

/// The cache key of `prompt` answered by `model` on `photo`.
///
/// ⚠ M10.11a: an **active** negative is part of the key — the model was shown the positive, and
/// another base or another balance is another positive. An inactive one adds nothing, so no key of a
/// photo that is not a negative changes.
pub fn key(
    photo: &Path,
    model: ModelId,
    prompt: &AiPrompt,
    negative: &edit_model::Negative,
) -> String {
    let md = std::fs::metadata(photo).ok();
    let size = md.as_ref().map_or(0, |m| m.len());
    let mtime = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let prompt = match prompt {
        AiPrompt::Subject => "subject".to_string(),
        AiPrompt::Click { point } => format!("click:{:.5}:{:.5}", point[0], point[1]),
    };
    let negative = if negative.is_active() {
        format!("|negative:{negative:?}")
    } else {
        String::new()
    };
    ai_mask::digest_hex(
        format!(
            "{}|{size}|{mtime}|{model:?}|{prompt}{negative}",
            photo.display()
        )
        .as_bytes(),
    )
}

pub fn load(dir: &Path, key: &str) -> Option<ProbMap> {
    ProbMap::from_bytes(&std::fs::read(dir.join(format!("{key}.map"))).ok()?)
}

/// Write `map` under `key`, atomically (a partial then a rename), so a crash never leaves a
/// truncated map that `load` would have to reject every time.
pub fn store(dir: &Path, key: &str, map: &ProbMap) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(format!("{key}.map"));
    let partial = dir.join(format!("{key}.map.partial"));
    std::fs::write(&partial, map.to_bytes())?;
    std::fs::rename(partial, dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M10.11a: a map found on the negative is not the positive's — the key carries the negative.
    #[test]
    fn the_key_carries_an_active_negative() {
        let photo = std::path::Path::new("/nonexistent/scan.RAF");
        let p = AiPrompt::Subject;
        let off = edit_model::Negative::default();
        let on = edit_model::Negative {
            on: true,
            base: [0.85, 0.55, 0.3],
            ..off
        };
        let k = |n: &edit_model::Negative| key(photo, ModelId::IsNet, &p, n);
        assert_ne!(k(&off), k(&on));
        assert_ne!(
            k(&on),
            k(&edit_model::Negative {
                base: [0.8, 0.55, 0.3],
                ..on
            }),
            "another base is another positive"
        );
        assert_eq!(
            k(&off),
            k(&edit_model::Negative { gamma: 0.9, ..off }),
            "an inactive negative changes nothing, so no key either"
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("aicache-{tag}"))
    }

    #[test]
    fn the_key_moves_with_every_input_and_nothing_else() {
        let d = temp_dir("key");
        let photo = d.join("A.RAF");
        std::fs::write(&photo, b"raw").unwrap();
        let click = AiPrompt::Click { point: [0.25, 0.5] };
        let k = key(
            &photo,
            ModelId::SamDecoder,
            &click,
            &edit_model::Negative::default(),
        );
        assert_eq!(
            k,
            key(
                &photo,
                ModelId::SamDecoder,
                &click,
                &edit_model::Negative::default()
            ),
            "stable"
        );
        assert_ne!(
            k,
            key(
                &photo,
                ModelId::IsNet,
                &click,
                &edit_model::Negative::default()
            ),
            "model"
        );
        assert_ne!(
            k,
            key(
                &photo,
                ModelId::SamDecoder,
                &AiPrompt::Subject,
                &edit_model::Negative::default()
            ),
            "prompt"
        );
        assert_ne!(
            k,
            key(
                &photo,
                ModelId::SamDecoder,
                &AiPrompt::Click { point: [0.26, 0.5] },
                &edit_model::Negative::default()
            ),
            "point"
        );
        let other = d.join("B.RAF");
        std::fs::write(&other, b"raw").unwrap();
        assert_ne!(
            k,
            key(
                &other,
                ModelId::SamDecoder,
                &click,
                &edit_model::Negative::default()
            ),
            "file"
        );
        std::fs::write(&photo, b"raw, re-saved").unwrap();
        assert_ne!(
            k,
            key(
                &photo,
                ModelId::SamDecoder,
                &click,
                &edit_model::Negative::default()
            ),
            "size"
        );
    }

    /// ⚠⚠ M10.16a: the Subject and Click keys are pinned as LITERAL digests, taken from the code as it
    /// stood before the map query widened the key's prompt to a `MapQuery` (2026-10-14). Every map
    /// already on disk is filed under one of these; a key that drifts orphans them all, silently — the
    /// user waits seconds for a map they already had. A missing file reads as size 0, mtime 0.
    #[test]
    fn the_subject_and_click_keys_are_pinned() {
        let photo = std::path::Path::new("/nonexistent/pin.RAF");
        let none = edit_model::Negative::default();
        assert_eq!(
            key(photo, ModelId::IsNet, &AiPrompt::Subject, &none),
            "573994b1b13ff7f2097a4d5bf4e77900e50e04db10ba2db0e90a426c0e5a7e33"
        );
        assert_eq!(
            key(
                photo,
                ModelId::SamDecoder,
                &AiPrompt::Click { point: [0.25, 0.5] },
                &none
            ),
            "a3eeb8d569c6add36d6f0c4b6f02072a3ef2953ef40eed6339b51b0456b2384e"
        );
    }

    #[test]
    fn a_stored_map_loads_back_and_leaves_no_partial() {
        let d = temp_dir("store");
        let m = ProbMap::from_fn(4, 3, |x, y| (x * y) as f32 / 6.0);
        store(&d, "k", &m).unwrap();
        assert_eq!(load(&d, "k"), Some(m));
        assert!(!d.join("k.map.partial").exists());
        assert_eq!(load(&d, "missing"), None);
    }
}

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
pub fn key(photo: &Path, model: ModelId, prompt: &AiPrompt) -> String {
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
    ai_mask::digest_hex(format!("{}|{size}|{mtime}|{model:?}|{prompt}", photo.display()).as_bytes())
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

    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("aicache-{tag}"))
    }

    #[test]
    fn the_key_moves_with_every_input_and_nothing_else() {
        let d = temp_dir("key");
        let photo = d.join("A.RAF");
        std::fs::write(&photo, b"raw").unwrap();
        let click = AiPrompt::Click { point: [0.25, 0.5] };
        let k = key(&photo, ModelId::SamDecoder, &click);
        assert_eq!(k, key(&photo, ModelId::SamDecoder, &click), "stable");
        assert_ne!(k, key(&photo, ModelId::IsNet, &click), "model");
        assert_ne!(
            k,
            key(&photo, ModelId::SamDecoder, &AiPrompt::Subject),
            "prompt"
        );
        assert_ne!(
            k,
            key(
                &photo,
                ModelId::SamDecoder,
                &AiPrompt::Click { point: [0.26, 0.5] }
            ),
            "point"
        );
        let other = d.join("B.RAF");
        std::fs::write(&other, b"raw").unwrap();
        assert_ne!(k, key(&other, ModelId::SamDecoder, &click), "file");
        std::fs::write(&photo, b"raw, re-saved").unwrap();
        assert_ne!(k, key(&photo, ModelId::SamDecoder, &click), "size");
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

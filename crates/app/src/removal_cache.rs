//! The Remove brush's fills on disk (M10.16c1): one small file a removal, kept across sessions —
//! `denoise_cache`'s pattern, keyed by `remove_tool::id`.

use pipeline::Fill;
use std::path::{Path, PathBuf};

/// The fills' directory.
pub fn dir() -> Option<PathBuf> {
    Some(crate::user_cache::base()?.join("rustroom").join("removals"))
}

/// The file's first bytes, then its version.
const MAGIC: &[u8; 4] = b"RRFL";
const VERSION: u32 = 1;
/// A fill's extension.
const EXT: &str = "rrfl";
/// The header: magic, version, the rect's four floats, the width and height.
const HEADER: usize = 4 + 4 + 16 + 8;

fn file(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{EXT}"))
}

/// Whether a fill is stored under `key` — a stat, cheap enough for every frame.
pub fn exists(dir: &Path, key: &str) -> bool {
    file(dir, key).is_file()
}

/// Remove `key`'s fill — a corrupt one, which [`load`] refused.
pub fn remove(dir: &Path, key: &str) {
    let _ = std::fs::remove_file(file(dir, key));
}

/// Write `f` under `key`, atomically (a partial then a rename): the header, then the RGB and the alpha as
/// little-endian half-floats, deflated.
pub fn store(dir: &Path, key: &str, f: &Fill) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut raw = Vec::with_capacity((f.rgb.len() + f.alpha.len()) * 2);
    for v in f.rgb.iter().chain(&f.alpha) {
        raw.extend_from_slice(&half::f16::from_f32(*v).to_le_bytes());
    }
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&raw)?;
    let packed = enc.finish()?;
    let mut bytes = Vec::with_capacity(HEADER + packed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    for v in f.rect {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(&(f.w as u32).to_le_bytes());
    bytes.extend_from_slice(&(f.h as u32).to_le_bytes());
    bytes.extend_from_slice(&packed);
    let partial = dir.join(format!("{key}.{EXT}.partial"));
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, file(dir, key))
}

/// `key`'s fill, or `None` — missing, truncated or not ours.
pub fn load(dir: &Path, key: &str) -> Option<Fill> {
    use std::io::Read;
    let bytes = std::fs::read(file(dir, key)).ok()?;
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("four bytes"));
    if word(4) != VERSION {
        return None;
    }
    let rect: [f32; 4] = std::array::from_fn(|k| f32::from_bits(word(8 + 4 * k)));
    let (w, h) = (word(24) as usize, word(28) as usize);
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(&bytes[HEADER..])
        .read_to_end(&mut raw)
        .ok()?;
    if w == 0 || h == 0 || raw.len() != w * h * 4 * 2 {
        return None;
    }
    let mut v: Vec<f32> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect();
    let alpha = v.split_off(w * h * 3);
    Some(Fill {
        rect,
        w,
        h,
        rgb: v,
        alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_fill() -> Fill {
        Fill {
            rect: [0.125, 0.25, 0.625, 0.875],
            w: 7,
            h: 5,
            rgb: (0..7 * 5 * 3).map(|i| i as f32 / 40.0).collect(),
            alpha: (0..7 * 5).map(|i| (i % 4) as f32 / 3.0).collect(),
        }
    }

    /// Back as it went, within a half-float's precision; the rect exactly.
    #[test]
    fn a_fill_round_trips_the_cache() {
        let d = test_dir::TempDir::new("removal-cache-trip");
        let f = a_fill();
        assert!(!exists(d.path(), "k"));
        store(d.path(), "k", &f).unwrap();
        assert!(exists(d.path(), "k"));
        let back = load(d.path(), "k").expect("loads");
        assert_eq!((back.rect, back.w, back.h), (f.rect, f.w, f.h));
        assert_eq!(
            (back.rgb.len(), back.alpha.len()),
            (f.rgb.len(), f.alpha.len())
        );
        for (a, b) in back
            .rgb
            .iter()
            .chain(&back.alpha)
            .zip(f.rgb.iter().chain(&f.alpha))
        {
            assert!((a - b).abs() <= b.abs() * 1e-3 + 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn a_truncated_or_foreign_file_does_not_load() {
        let d = test_dir::TempDir::new("removal-cache-corrupt");
        store(d.path(), "k", &a_fill()).unwrap();
        let path = file(d.path(), "k");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(load(d.path(), "k").is_none(), "truncated");
        std::fs::write(&path, b"RRDN0000").unwrap();
        assert!(load(d.path(), "k").is_none(), "another cache's file");
        remove(d.path(), "k");
        assert!(!exists(d.path(), "k"));
    }
}

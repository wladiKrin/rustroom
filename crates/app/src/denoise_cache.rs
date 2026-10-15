//! AI denoise's results on disk (M10.16b): the corrected full-resolution image, one file a photo and a
//! demosaic quality, kept across sessions — and the soft limit Settings → Performance shows.
//!
//! ⛔ Nothing here deletes on its own: [`remove_all`] runs only from the Clear dialog (M10.15b's rule).

use core_image::ImageBuf;
use std::path::{Path, PathBuf};

/// The denoise cache's directory.
pub fn dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("rustroom").join("denoise"))
}

/// The recipe's version in every key — the encoding, the noise estimate and the model. ⚠ Bump it when
/// any of them changes, or every cached result keeps the old picture.
pub const RECIPE: &str = "denoise:v1";

/// The cache key of `photo` decoded at `tier`.
///
/// ⚠ Keyed on the FILE (path, size, mtime) and the demosaic quality — the network's input is that decode
/// — and NOT on the film negative: the decode is the same whatever the negative's settings.
pub fn key(photo: &Path, tier: raw_decode::Demosaic) -> String {
    let md = std::fs::metadata(photo).ok();
    let size = md.as_ref().map_or(0, |m| m.len());
    let mtime = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    ai_mask::digest_hex(
        format!(
            "{}|{size}|{mtime}|{:?}|{tier:?}|{RECIPE}",
            photo.display(),
            ai_mask::ModelId::DrunetColor
        )
        .as_bytes(),
    )
}

/// The file's first bytes, then its version.
const MAGIC: &[u8; 4] = b"RRDN";
const VERSION: u32 = 1;
/// A result's extension — what [`usage`] and [`remove_all`] count, and nothing else.
const EXT: &str = "rrdn";

fn file(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{EXT}"))
}

/// Whether a result is stored under `key` — a stat, cheap enough for every frame.
pub fn exists(dir: &Path, key: &str) -> bool {
    file(dir, key).is_file()
}

/// Write `img` under `key`, atomically (a partial then a rename): `RRDN`, the version, the width and
/// height, then the samples as little-endian half-floats, deflated.
pub fn store(dir: &Path, key: &str, img: &ImageBuf) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut raw = Vec::with_capacity(img.data().len() * 2);
    for v in img.data() {
        raw.extend_from_slice(&half::f16::from_f32(*v).to_le_bytes());
    }
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&raw)?;
    let packed = enc.finish()?;
    let mut bytes = Vec::with_capacity(16 + packed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&(img.width() as u32).to_le_bytes());
    bytes.extend_from_slice(&(img.height() as u32).to_le_bytes());
    bytes.extend_from_slice(&packed);
    let dest = file(dir, key);
    let partial = dir.join(format!("{key}.{EXT}.partial"));
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, &dest)
}

/// `key`'s image, or `None` — missing, truncated or not ours.
pub fn load(dir: &Path, key: &str) -> Option<ImageBuf> {
    use std::io::Read;
    let bytes = std::fs::read(file(dir, key)).ok()?;
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("four bytes"));
    if word(4) != VERSION {
        return None;
    }
    let (w, h) = (word(8) as usize, word(12) as usize);
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut raw)
        .ok()?;
    if w == 0 || h == 0 || raw.len() != w * h * 3 * 2 {
        return None;
    }
    let data = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect();
    Some(ImageBuf::from_vec(w, h, data))
}

/// Our results in `dir` — files ending `.rrdn`, never a partial or anything else.
fn results(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXT))
        .collect()
}

/// The results' total size and count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub photos: usize,
}

pub fn usage(dir: &Path) -> Usage {
    let mut u = Usage::default();
    for p in results(dir) {
        if let Ok(m) = std::fs::metadata(&p) {
            u.bytes += m.len();
            u.photos += 1;
        }
    }
    u
}

/// Delete every result — the Clear dialog's primary button and nothing else. Returns how many went.
pub fn remove_all(dir: &Path) -> usize {
    results(dir)
        .into_iter()
        .filter(|p| std::fs::remove_file(p).is_ok())
        .count()
}

/// The Settings line: "1.2 GB · 9 photos", or "No results".
pub fn size_line(usage: Usage) -> String {
    match usage.photos {
        0 => "No results".to_string(),
        1 => format!("{} · 1 photo", crate::photo_info::format_bytes(usage.bytes)),
        n => format!(
            "{} · {n} photos",
            crate::photo_info::format_bytes(usage.bytes)
        ),
    }
}

/// The Clear dialog's title — the size at startup, the verb when asked from Settings.
pub fn dialog_title(usage: Usage, startup: bool) -> String {
    if startup {
        format!(
            "AI denoise results use {}",
            crate::photo_info::format_bytes(usage.bytes)
        )
    } else {
        "Clear AI denoise results".to_string()
    }
}

/// The dialog's body. ⚠ Always names what a clear keeps — the edits — and what it costs: a recompute.
pub fn dialog_body(usage: Usage, limit_gb: u32, startup: bool) -> String {
    let n = match usage.photos {
        1 => "1 photo's result".to_string(),
        n => format!("{n} photos' results"),
    };
    let rest = format!(
        "Clearing removes {n}; edits stay, and a photo's result is computed again when you open it"
    );
    if startup {
        format!("Over your {limit_gb} GB limit. {rest}")
    } else {
        rest
    }
}

/// Whether startup offers the Clear dialog: the warning on and the results **strictly** over the limit
/// (decimal gigabytes, `immich_cache::limit_bytes`'s units).
pub fn warn_at_startup(warn: bool, limit_gb: u32, usage: Usage) -> bool {
    warn && usage.bytes > crate::immich_cache::limit_bytes(limit_gb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [x as f32 / w as f32, 0.25, 3.0 - y as f32 / h as f32]);
            }
        }
        img
    }

    #[test]
    fn the_texts_name_the_size_and_what_a_clear_keeps() {
        let one = Usage {
            bytes: 135_000_000,
            photos: 1,
        };
        assert_eq!(size_line(Usage::default()), "No results");
        assert!(size_line(one).ends_with("· 1 photo"), "{}", size_line(one));
        assert!(dialog_title(one, true).starts_with("AI denoise results use"));
        assert_eq!(dialog_title(one, false), "Clear AI denoise results");
        let body = dialog_body(one, 10, true);
        assert!(body.starts_with("Over your 10 GB limit."), "{body}");
        assert!(body.contains("edits stay"), "{body}");
        assert!(!dialog_body(one, 10, false).contains("limit"));
    }

    #[test]
    fn a_result_round_trips_within_half_precision() {
        let d = test_dir::TempDir::new("denoise-cache-rt");
        let img = picture(37, 21);
        assert!(!exists(d.path(), "k"));
        store(d.path(), "k", &img).expect("store");
        assert!(exists(d.path(), "k"));
        let back = load(d.path(), "k").expect("load");
        assert_eq!((back.width(), back.height()), (37, 21));
        for (a, b) in img.data().iter().zip(back.data()) {
            assert!((a - b).abs() <= a.abs() * 1e-3 + 1e-4, "{a} → {b}");
        }
        assert!(load(d.path(), "missing").is_none());
    }

    /// Review Focus 5: a truncated or foreign file is rejected, never a garbage image.
    #[test]
    fn a_truncated_file_is_rejected() {
        let d = test_dir::TempDir::new("denoise-cache-bad");
        store(d.path(), "k", &picture(40, 30)).expect("store");
        let p = d.path().join("k.rrdn");
        let bytes = std::fs::read(&p).expect("read");
        std::fs::write(&p, &bytes[..bytes.len() / 2]).expect("truncate");
        assert!(load(d.path(), "k").is_none(), "truncated");
        let mut foreign = bytes.clone();
        foreign[0] = b'X';
        std::fs::write(&p, &foreign).expect("write");
        assert!(load(d.path(), "k").is_none(), "another magic");
        std::fs::write(&p, &bytes).expect("restore");
        assert!(load(d.path(), "k").is_some(), "positive control");
    }

    #[test]
    fn the_key_moves_with_the_file_and_the_tier() {
        let d = test_dir::TempDir::new("denoise-cache-key");
        let f = d.path().join("photo.raf");
        std::fs::write(&f, b"abc").expect("write");
        let a = key(&f, raw_decode::Demosaic::default());
        assert_eq!(a, key(&f, raw_decode::Demosaic::default()));
        let other = raw_decode::Demosaic::ALL
            .into_iter()
            .find(|t| *t != raw_decode::Demosaic::default())
            .expect("a second tier");
        assert_ne!(a, key(&f, other), "the tier");
        std::fs::write(&f, b"abcd").expect("grow");
        assert_ne!(a, key(&f, raw_decode::Demosaic::default()), "the size");
    }

    #[test]
    fn usage_counts_and_clear_removes() {
        let d = test_dir::TempDir::new("denoise-cache-usage");
        assert_eq!(usage(d.path()), Usage::default());
        store(d.path(), "a", &picture(20, 10)).expect("a");
        store(d.path(), "b", &picture(20, 10)).expect("b");
        std::fs::write(d.path().join("note.txt"), b"not ours").expect("other");
        let u = usage(d.path());
        assert_eq!(u.photos, 2);
        assert!(u.bytes > 0);
        assert_eq!(remove_all(d.path()), 2);
        assert_eq!(usage(d.path()), Usage::default());
        assert!(d.path().join("note.txt").exists(), "only our files go");
    }

    #[test]
    fn startup_warns_only_strictly_over_the_limit() {
        let gb = 1_000_000_000u64;
        let at = |bytes| Usage { bytes, photos: 3 };
        assert!(!warn_at_startup(true, 10, at(10 * gb)));
        assert!(warn_at_startup(true, 10, at(10 * gb + 1)));
        assert!(!warn_at_startup(false, 10, at(20 * gb)));
    }
}

//! An on-disk cache of **extracted previews** — the step before a thumbnail is rendered.
//!
//! ⚠⚠ **It caches the preview, not the finished thumbnail, and that is the design.** Extraction
//! is ~191 ms and rendering an edited thumbnail over it is ~5 ms, so keying this on edits would
//! make it useless for the case `thumb_loader` already flags as expensive — a changed edit — and
//! would leave a file behind for every edit state a photo has ever had. The two levels then each
//! cache what they are named for: disk holds previews, memory (`ThumbCache`) holds rendered
//! thumbnails, and neither needs invalidation code.
//!
//! Measured on a real 830-entry folder: extract 191.4 ms against a 1.10 ms JPEG decode, 172×.
//! `cargo run --release -p app --example bench_thumb_cache <folder>`.

/// FNV-1a, 64-bit.
///
/// ⚠⚠ **NOT `std::collections::hash_map::DefaultHasher`, and this is the whole reason this
/// function exists.** Its algorithm is explicitly allowed to change between Rust releases, so an
/// **on-disk** key built from it would silently invalidate the entire cache on a toolchain
/// upgrade — a 26-second regression appearing for no reason anybody could connect to anything.
/// `thumb_key::edits_fingerprint` gets away with `DefaultHasher` precisely because it never
/// leaves memory.
///
/// Five lines and stable by definition, which beats a dependency — the argument that already
/// kept CRC-32 hand-rolled here.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        // ⚠ The FNV-64 prime is 0x100000001b3 — **eleven** hex digits, so the grouping is
        // 3-4-4 and not 4-4-4. Writing `0x1000_0000_01b3` gives 0x1000000001b3, a different
        // number that still hashes plausibly; the pinned vectors below caught it on the first
        // run, which is exactly why they are pinned.
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

use core_image::Thumbnail;
use std::path::{Path, PathBuf};

/// What a cached entry is *of*: a source file at a particular version.
///
/// ⚠ `(path, mtime, size)` rather than a content hash. A hash means reading a 27 MB RAW, which is
/// precisely the cost this cache exists to avoid.
///
/// ⚠ **Accepted limit: mtime resolution is the filesystem's.** APFS and ext4 record nanoseconds,
/// but a filesystem with one-second granularity could give a stale hit for a file rewritten
/// within the same second *and* to exactly the same length. Recorded rather than guarded, because
/// guarding it means hashing content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub path: PathBuf,
    /// Nanoseconds since the Unix epoch. **Signed**, so a pre-1970 timestamp is representable
    /// rather than an error.
    pub mtime_ns: i64,
    pub size: u64,
}

impl Identity {
    /// The identity of `path` as it is on disk right now.
    ///
    /// ⚠ `None` when the metadata cannot be read. A file with no readable identity is simply not
    /// cached — never cached under a guessed one, which would serve the wrong pixels.
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let t = m.modified().ok()?;
        let mtime_ns = match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_nanos()).ok()?,
            // Before 1970. Representable, and the sign is what makes it distinguishable.
            Err(e) => -i64::try_from(e.duration().as_nanos()).ok()?,
        };
        Some(Self {
            path: path.to_path_buf(),
            mtime_ns,
            size: m.len(),
        })
    }

    /// The bytes the filename hash is taken over. Kept separate from [`Self::file_name`] so a test
    /// can reason about the input without reproducing the hash.
    fn key_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(self.path.to_string_lossy().as_bytes());
        v.push(0);
        v.extend_from_slice(&self.mtime_ns.to_le_bytes());
        v.extend_from_slice(&self.size.to_le_bytes());
        v
    }

    /// This entry's file name inside the cache directory: hex, flat, and safe whatever the source
    /// path contains.
    pub fn file_name(&self) -> String {
        format!("{:016x}.rthm", fnv1a(&self.key_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        // ⚠⚠ Pinned to the reference vectors so a "tidy-up" that swaps in a different hash is a
        // FAILING TEST rather than a silently emptied cache on everyone's machine. That failure
        // would otherwise be invisible: every entry simply misses and gets rewritten.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    use std::path::PathBuf;

    fn ident(path: &str, mtime: i64, size: u64) -> Identity {
        Identity {
            path: PathBuf::from(path),
            mtime_ns: mtime,
            size,
        }
    }

    #[test]
    fn the_filename_changes_with_every_part_of_the_identity() {
        // ⚠ All three, individually. A key that hashed only the path would serve a stale
        // thumbnail forever after the file was edited outside Rustroom — and a test that varied
        // only one field could not tell that apart from a correct implementation.
        let base = ident("/photos/a.RAF", 100, 200);
        assert_ne!(base.file_name(), ident("/photos/b.RAF", 100, 200).file_name());
        assert_ne!(base.file_name(), ident("/photos/a.RAF", 101, 200).file_name());
        assert_ne!(base.file_name(), ident("/photos/a.RAF", 100, 201).file_name());
        assert_eq!(base.file_name(), ident("/photos/a.RAF", 100, 200).file_name());
    }

    #[test]
    fn the_filename_is_flat_and_filesystem_safe() {
        // A source path contains separators and may contain anything else; the cache directory
        // must stay flat and every name must be openable.
        let n = ident("/a b/../c:d*e?/f.RAF", 1, 2).file_name();
        // ⚠ The stem and the extension are checked separately. The first version of this
        // assertion asked whether *every* character was a hex digit or a dot, which is false of
        // the extension's own letters — an assertion wrong about correct code, the trap this
        // project has recorded more than once. Check what is actually claimed.
        let (stem, ext) = n.split_once('.').expect("a name with an extension");
        assert!(
            !stem.is_empty() && stem.chars().all(|c| c.is_ascii_hexdigit()),
            "the stem must be pure hex, got {stem:?}"
        );
        assert_eq!(ext, "rthm", "got {n:?}");
        assert!(
            !n.contains(std::path::MAIN_SEPARATOR),
            "the directory must stay flat, got {n:?}"
        );
    }
}

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
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
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
pub enum Identity {
    /// A source file at a particular version.
    Local {
        path: PathBuf,
        /// Nanoseconds since the Unix epoch. **Signed**, so a pre-1970 timestamp is representable
        /// rather than an error.
        mtime_ns: i64,
        size: u64,
    },
    /// A remote asset at a particular version.
    ///
    /// ⚠⚠ Needs **no filesystem at all** — the entire reason this became an enum. `Identity::of`
    /// stats a file, and an Immich asset that has not been downloaded has none.
    ///
    /// ⚠ `version` is Immich's `updatedAt`, not its `thumbhash`. `thumbhash` tracks appearance more
    /// tightly but is a tiny perceptual hash that could survive a re-encode unchanged —
    /// under-invalidating. `updatedAt` over-invalidates instead (a favourite toggle costs one
    /// refetch), which matches this file's rule that an entry is never cached under a guessed
    /// identity.
    Remote { asset_id: String, version: String },
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
        Some(Identity::Local {
            path: path.to_path_buf(),
            mtime_ns,
            size: m.len(),
        })
    }

    /// A remote asset's identity, built without touching the filesystem.
    pub fn remote(asset_id: &str, version: &str) -> Self {
        Identity::Remote {
            asset_id: asset_id.to_string(),
            version: version.to_string(),
        }
    }

    /// The bytes the filename hash is taken over. Kept separate from [`Self::file_name`] so a test
    /// can reason about the input without reproducing the hash.
    pub(crate) fn key_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            // ⚠⚠ **BYTE-IDENTICAL to the pre-enum version, and it must stay that way.** Any change
            // here turns every thumbnail already on a user's disk into a miss, for nothing — and
            // nothing else would fail. `a_known_local_identity_keeps_its_filename_for_ever` is the
            // only thing guarding it.
            //
            // ⚠ No discriminant byte is added, deliberately: `Remote` entries live in a **different
            // directory**, so the two variants can never collide, and a discriminant would break
            // every existing `Local` key to buy nothing.
            Identity::Local {
                path,
                mtime_ns,
                size,
            } => {
                v.extend_from_slice(path.to_string_lossy().as_bytes());
                v.push(0);
                v.extend_from_slice(&mtime_ns.to_le_bytes());
                v.extend_from_slice(&size.to_le_bytes());
            }
            Identity::Remote { asset_id, version } => {
                v.extend_from_slice(asset_id.as_bytes());
                v.push(0);
                v.extend_from_slice(version.as_bytes());
            }
        }
        v
    }

    /// This entry's file name inside the cache directory: hex, flat, and safe whatever the source
    /// path contains.
    pub fn file_name(&self) -> String {
        format!("{:016x}.rthm", fnv1a(&self.key_bytes()))
    }
}

/// The magic at the start of every entry.
const MAGIC: &[u8; 4] = b"RTHM";

/// The on-disk format version of a **local** entry.
///
/// ⚠ Bump this whenever the header or the payload encoding changes. An entry with the wrong
/// version is a **miss**, so a bump invalidates cleanly instead of reinterpreting old bytes as
/// the new format — which is the failure a bare `.jpg` on disk could not protect against.
///
/// ⚠⚠ **Still 1, deliberately.** A local entry's header bytes are unchanged by the `Identity` enum,
/// so every thumbnail already on a user's disk keeps hitting. Bumping it would have cost everyone
/// their cache — ~190 ms per entry to rebuild, about 26 s for an 830-photo folder — for nothing.
const VERSION_LOCAL: u16 = 1;

/// The on-disk format version of a **remote** entry.
///
/// ⚠⚠ A second layout rather than a bump, because the header carries the identity's own fields and
/// a remote asset has no path, mtime or size to put there. The version field is already the layout
/// discriminator this file documents, so an entry says which shape it is and a mismatch is a clean
/// miss. Remote entries also live in their own directory, so the two can never actually meet.
const VERSION_REMOTE: u16 = 2;

/// JPEG quality for the payload.
///
/// ⚠ Measured, not chosen by taste: q90 stores a 512 px thumbnail in **61.3 KB**, which is
/// 49.7 MB for an 830-photo folder, against PNG's 267 MB and raw RGBA's 552.8 MB. It is a
/// tunable — the number is here so a future change is made against data.
const QUALITY: u8 = 90;

/// A directory of cached extracted previews.
///
/// ⚠ Holds a `PathBuf` and nothing else, which is what makes it `Send + Sync` for free — six
/// loader workers share one. There is no in-process index to lock: **the filesystem is the
/// index**. Two workers never race on one entry because `ThumbLoader`'s in-flight set already
/// prevents the same path being fetched twice.
#[derive(Debug, Clone)]
pub struct ThumbStore {
    dir: PathBuf,
    /// Bytes this directory may hold. `u64::MAX` means "no limit", which is what tests pass.
    budget: u64,
    /// Bytes written since the last sweep.
    ///
    /// ⚠⚠ A **deliberate, documented departure** from "holds a `PathBuf` and nothing else" above.
    /// An atomic keeps the store `Send + Sync` with no lock, which is what lets six loader workers
    /// keep sharing one. It is a counter, not an index — the filesystem is still the index.
    ///
    /// ⚠ `Arc` so `Clone` survives, and because two handles on one directory *should* share the
    /// accounting rather than each keeping its own idea of how much has been written.
    written: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl ThumbStore {
    /// The directory is injected so tests run against a temp dir and never touch the real home
    /// directory — the `PresetStore` and `SettingsStore` pattern.
    ///
    /// ⚠⚠ **Does NOT sweep.** It used to, "so a store left over budget by a lowered setting is
    /// corrected" — but `ThumbLoader::new` builds **two** of these on every library open, and a sweep
    /// walks the directory and stats every entry. With ~2,500 cached thumbnails that was ~2,500 stats
    /// on the UI thread every time an album was clicked. Reported in use, 2026-09-23.
    ///
    /// ⚠ The budget is still enforced: `put` sweeps once per tenth of the budget written, and
    /// [`Self::sweep`] is called explicitly at startup and when a budget changes — the two moments a
    /// store can be over budget with nothing having been written.
    pub fn new(dir: PathBuf, budget_bytes: u64) -> Self {
        Self {
            dir,
            budget: budget_bytes,
            written: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// `thumbs/` beside the catalog, in `catalog::db::Catalog::library_dir` — Local AppData on Windows.
    fn user_dir() -> PathBuf {
        catalog::db::Catalog::library_dir().join("thumbs")
    }

    /// The local thumbnail cache.
    pub fn user_default(budget_bytes: u64) -> Self {
        Self::new(Self::user_dir(), budget_bytes)
    }

    /// The Immich thumbnail cache — a **sibling** directory.
    ///
    /// ⚠⚠ Separate from the local one on purpose: accounting is then just `size_bytes()` per
    /// directory, and neither budget's sweep can reach the other's entries. The alternative — one
    /// directory with a kind marker in the RTHM header — would mean reading every entry's header
    /// just to add up its bytes.
    pub fn immich_default(budget_bytes: u64) -> Self {
        Self::new(Self::user_dir().join("immich"), budget_bytes)
    }

    /// Delete oldest-first until the directory is within budget.
    ///
    /// ⚠ Best-effort throughout: an entry that will not delete is skipped, exactly as `clear()`
    /// already does. A sweep must never be able to stop a folder opening.
    pub fn sweep(&self) {
        if self.budget == u64::MAX {
            return;
        }
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
        let mut total: u64 = 0;
        for e in d.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if !m.is_file() {
                continue;
            }
            // ⚠ Litter from an interrupted `put`, not an entry: removed rather than counted.
            if p.extension().is_some_and(|x| x == "tmp") {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            let when = m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total += m.len();
            entries.push((p, when, m.len()));
        }
        if total <= self.budget {
            return;
        }
        // ⚠ Oldest first, and mtime is the **touch** time rather than the write time — `get`
        // updates it, which is what makes this LRU instead of FIFO.
        entries.sort_by_key(|(_, when, _)| *when);
        for (p, _, len) in entries {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }

    /// Where this store keeps its files, for the Settings readout.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The cached preview for `id`, or `None` for any reason at all.
    ///
    /// ⚠ Every failure — missing, unreadable, wrong magic, wrong version, wrong identity,
    /// truncated, undecodable — is a **miss**. Nothing here may surface an error: a thumbnail
    /// cache that can stop a folder opening is worse than no cache.
    pub fn get(&self, id: &Identity) -> Option<Thumbnail> {
        let bytes = std::fs::read(self.dir.join(id.file_name())).ok()?;
        let (w, h, jpeg) = decode_header(&bytes, id)?;
        let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
        let rgba = img.to_rgba8();
        // ⚠ The header is the authority on dimensions, so a payload that disagrees with it is a
        // miss rather than a thumbnail of an unexpected size reaching the grid.
        if rgba.width() as usize != w || rgba.height() as usize != h {
            return None;
        }
        // ⚠⚠ This one syscall is what makes eviction LRU instead of FIFO. Without it an entry's
        // mtime is its write time, so the sweep would discard the folder you revisit most — the
        // exact opposite of what a cache is for. Negligible against a measured 1.26 ms read.
        //
        // ⚠ Best-effort: a read-only cache directory must still serve hits.
        if let Ok(f) = std::fs::File::options()
            .write(true)
            .open(self.dir.join(id.file_name()))
        {
            let _ =
                f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
        }
        Some(Thumbnail {
            width: w,
            height: h,
            rgba: rgba.into_raw(),
        })
    }

    /// Cache `thumb` under `id`, best-effort.
    ///
    /// ⚠ Silent on failure by design — the caller already has the thumbnail, and a full disk must
    /// not become an error in the grid.
    pub fn put(&self, id: &Identity, thumb: &Thumbnail) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let Some(bytes) = encode_entry(id, thumb) else {
            return;
        };
        // ⚠⚠ Write then rename, never write in place. A crash or a full disk mid-write would
        // otherwise leave a truncated entry that decodes to garbage rather than failing cleanly.
        // `rename` within one directory is atomic and replaces an existing file on every platform
        // this targets.
        let tmp = self.dir.join(format!("{}.tmp", id.file_name()));
        if std::fs::write(&tmp, &bytes).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, self.dir.join(id.file_name())).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        // ⚠ Not a sweep per `put`: `size_bytes()` walks the directory, so that would be a
        // directory scan per thumbnail. Amortised instead — one sweep per tenth of the budget
        // written, which bounds the overshoot without an index.
        if self.budget != u64::MAX {
            let n = bytes.len() as u64;
            let before = self
                .written
                .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            let slack = (self.budget / 10).max(1);
            if before + n >= slack {
                self.written.store(0, std::sync::atomic::Ordering::Relaxed);
                self.sweep();
            }
        }
    }
    /// Total bytes on disk.
    ///
    /// ⚠ Walks the directory, so it must be called when the Settings dialog **opens** and never
    /// in a dialog body — a body runs sixty times a second, which is the `apply_view_demosaic`
    /// mistake in another costume. An absent directory is `0`, not an error.
    pub fn size_bytes(&self) -> u64 {
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        d.flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    }

    /// Delete every entry.
    ///
    /// ⚠ Removes the **files**, not the directory, so the store keeps working afterwards rather
    /// than needing to be recreated. Best-effort: an entry that cannot be removed is skipped, and
    /// the worst outcome is that it is still there.
    ///
    /// ⚠ Deliberately **not** confirmed at the call site. This destroys nothing the app cannot
    /// rebuild — the distinction that already separates deleting a collection from forgetting a
    /// folder.
    pub fn clear(&self) {
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for e in d.flatten() {
            if e.metadata().map(|m| m.is_file()).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Parse an entry's header, returning its dimensions and the JPEG payload.
///
/// ⚠ Every length is checked before it is used. The input is a file on disk that anything could
/// have written, so an out-of-bounds slice here is a panic in a background worker.
fn decode_header<'a>(bytes: &'a [u8], want: &Identity) -> Option<(usize, usize, &'a [u8])> {
    let mut p = 0usize;
    let take = |p: &mut usize, n: usize| -> Option<&'a [u8]> {
        let s = bytes.get(*p..p.checked_add(n)?)?;
        *p += n;
        Some(s)
    };
    if take(&mut p, 4)? != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?);
    let w = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    // ⚠⚠ The identity check below is what turns a hash collision into a miss rather than someone
    // else's photograph in the grid. Each variant checks the fields it actually has.
    match want {
        Identity::Local {
            path: want_path,
            mtime_ns: want_mtime,
            size: want_size,
        } => {
            if version != VERSION_LOCAL {
                return None;
            }
            let mtime_ns = i64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
            let size = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
            let path_len = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
            let path = std::str::from_utf8(take(&mut p, path_len)?).ok()?;
            if mtime_ns != *want_mtime || size != *want_size || path != want_path.to_string_lossy()
            {
                return None;
            }
        }
        Identity::Remote {
            asset_id: want_id,
            version: want_version,
        } => {
            if version != VERSION_REMOTE {
                return None;
            }
            let id_len = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
            let asset_id = std::str::from_utf8(take(&mut p, id_len)?).ok()?;
            let ver_len = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
            let asset_version = std::str::from_utf8(take(&mut p, ver_len)?).ok()?;
            if asset_id != want_id || asset_version != want_version {
                return None;
            }
        }
    }
    Some((w, h, bytes.get(p..)?))
}

/// Serialize one entry: header, then JPEG q90 bytes.
fn encode_entry(id: &Identity, thumb: &Thumbnail) -> Option<Vec<u8>> {
    // ⚠ JPEG carries no alpha, so the payload is RGB and the alpha is reconstructed as opaque on
    // read. Sound because an extracted preview is always opaque — `bench_thumb_cache` asserts
    // that over a real folder rather than assuming it.
    let rgb: Vec<u8> = thumb
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode(
            &rgb,
            u32::try_from(thumb.width).ok()?,
            u32::try_from(thumb.height).ok()?,
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;

    let mut out = Vec::with_capacity(jpeg.len() + 128);
    out.extend_from_slice(MAGIC);
    // ⚠⚠ A local entry's bytes after this point are **exactly** what they were before the
    // `Identity` enum, which is what keeps every thumbnail already on disk a hit.
    match id {
        Identity::Local {
            path,
            mtime_ns,
            size,
        } => {
            let path = path.to_string_lossy();
            out.extend_from_slice(&VERSION_LOCAL.to_le_bytes());
            out.extend_from_slice(&u32::try_from(thumb.width).ok()?.to_le_bytes());
            out.extend_from_slice(&u32::try_from(thumb.height).ok()?.to_le_bytes());
            out.extend_from_slice(&mtime_ns.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&u32::try_from(path.len()).ok()?.to_le_bytes());
            out.extend_from_slice(path.as_bytes());
        }
        Identity::Remote { asset_id, version } => {
            out.extend_from_slice(&VERSION_REMOTE.to_le_bytes());
            out.extend_from_slice(&u32::try_from(thumb.width).ok()?.to_le_bytes());
            out.extend_from_slice(&u32::try_from(thumb.height).ok()?.to_le_bytes());
            out.extend_from_slice(&u32::try_from(asset_id.len()).ok()?.to_le_bytes());
            out.extend_from_slice(asset_id.as_bytes());
            out.extend_from_slice(&u32::try_from(version.len()).ok()?.to_le_bytes());
            out.extend_from_slice(version.as_bytes());
        }
    }
    out.extend_from_slice(&jpeg);
    Some(out)
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
        Identity::Local {
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
        assert_ne!(
            base.file_name(),
            ident("/photos/b.RAF", 100, 200).file_name()
        );
        assert_ne!(
            base.file_name(),
            ident("/photos/a.RAF", 101, 200).file_name()
        );
        assert_ne!(
            base.file_name(),
            ident("/photos/a.RAF", 100, 201).file_name()
        );
        assert_eq!(
            base.file_name(),
            ident("/photos/a.RAF", 100, 200).file_name()
        );
    }

    /// Write `n` padded entries into `dir`, oldest first, and return their paths.
    ///
    /// ⚠⚠ Writes through a store with **no budget**, so the fixture cannot be evicted while it is
    /// being built. The test then constructs its own budgeted store over the same directory, whose
    /// constructor sweep is what is under test. An earlier version wrote through the budgeted store
    /// and the zero-budget case deleted each entry before it could be padded — the code behaving
    /// correctly, and the fixture unable to cope.
    ///
    /// ⚠ mtimes are set explicitly rather than relying on write order: a filesystem with coarse
    /// timestamps would make "oldest" arbitrary, and that ordering is what every eviction test here
    /// rests on.
    fn fill(dir: &Path, n: usize, each: usize) -> Vec<PathBuf> {
        let writer = ThumbStore::new(dir.to_path_buf(), u64::MAX);
        let mut out = Vec::new();
        for i in 0..n {
            let id = ident(&format!("/photos/{i}.RAF"), i as i64, each as u64);
            let t = Thumbnail {
                width: 1,
                height: 1,
                rgba: vec![0u8; 4],
            };
            writer.put(&id, &t);
            let p = dir.join(id.file_name());
            let mut bytes = std::fs::read(&p).unwrap();
            bytes.resize(each.max(bytes.len()), 0);
            std::fs::write(&p, &bytes).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            let when = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_000_000 + i as u64 * 60);
            f.set_times(std::fs::FileTimes::new().set_modified(when))
                .unwrap();
            out.push(p);
        }
        out
    }

    /// Write `n` entries through `store` at their natural size, padding nothing.
    ///
    /// ⚠ Used where the store's **own** byte accounting matters: `fill` pads files after `put`
    /// returns, so the store would account the bytes it actually wrote while the directory grew
    /// behind its back.
    fn write_entries(store: &ThumbStore, n: usize) {
        for i in 0..n {
            let id = ident(&format!("/photos/{i}.RAF"), i as i64, 1);
            let t = Thumbnail {
                width: 8,
                height: 8,
                rgba: vec![(i % 251) as u8; 8 * 8 * 4],
            };
            store.put(&id, &t);
        }
    }

    #[test]
    fn a_sweep_brings_an_over_budget_directory_under_it() {
        let dir = TempDir::new("sweep_under");
        let paths = fill(&dir.0, 10, 10 * 1024);
        let store = ThumbStore::new(dir.0.clone(), 50 * 1024);
        store.sweep();
        assert!(
            store.size_bytes() <= 50 * 1024,
            "still {} bytes",
            store.size_bytes()
        );
        // ⚠ The NEWEST must survive — a cache that kept the oldest would be useless.
        assert!(
            paths.last().unwrap().exists(),
            "the newest entry was evicted"
        );
        assert!(!paths[0].exists(), "the oldest entry survived");
    }

    /// ⚠ A budget of 0 must not panic or loop.
    #[test]
    fn a_zero_budget_empties_the_store_without_panicking() {
        let dir = TempDir::new("sweep_zero");
        fill(&dir.0, 3, 1024);
        let store = ThumbStore::new(dir.0.clone(), 0);
        store.sweep();
        assert_eq!(store.size_bytes(), 0);
    }

    /// ⚠ A `.tmp` left by an interrupted `put` is litter, not a cache entry: deleted on sight
    /// rather than counted towards the budget.
    #[test]
    fn a_leftover_tmp_is_deleted_by_a_sweep() {
        let dir = TempDir::new("sweep_tmp");
        let store = ThumbStore::new(dir.0.clone(), 1024 * 1024);
        let stray = dir.0.join("deadbeefdeadbeef.rthm.tmp");
        std::fs::write(&stray, vec![0u8; 4096]).unwrap();
        store.sweep();
        assert!(!stray.exists(), "a stray .tmp survived the sweep");
    }

    /// ⚠ An absent directory is not an error — the store must work before anything is cached.
    #[test]
    fn a_sweep_on_an_absent_directory_does_nothing() {
        let dir = TempDir::new("sweep_absent");
        let store = ThumbStore::new(dir.0.join("not-created-yet"), 1024);
        store.sweep();
    }

    /// ⚠⚠ **The difference between LRU and FIFO, and it is the whole point.** Without a touch on
    /// read, an entry's mtime is when it was *written*, so a sweep evicts the folder you revisit
    /// most. This reads the oldest entry and then requires it to survive a sweep that must evict.
    #[test]
    fn reading_an_entry_saves_it_from_the_next_sweep() {
        let dir = TempDir::new("lru_touch");
        let paths = fill(&dir.0, 4, 10 * 1024);
        let reader = ThumbStore::new(dir.0.clone(), u64::MAX);
        let oldest = ident("/photos/0.RAF", 0, 10 * 1024);
        assert!(
            reader.get(&oldest).is_some(),
            "the entry must be readable first"
        );
        // ⚠ The sweep must now spare it.
        ThumbStore::new(dir.0.clone(), 30 * 1024).sweep();
        assert!(
            paths[0].exists(),
            "the entry that was just read was evicted anyway — the touch is not working"
        );
        assert!(
            !paths[1].exists(),
            "an untouched older entry should have gone"
        );
    }

    /// ⚠ Enough writes must trigger a sweep on their own, or a long session could exceed the
    /// budget without limit.
    ///
    /// ⚠⚠ **Self-calibrating**: the same writes go into an unbudgeted store to learn what they
    /// naturally cost, and the budget is derived from that. An earlier version guessed the entry
    /// size, and `fill`'s padding meant the store accounted a few hundred bytes while the directory
    /// grew to 120 KB — a test that failed against correct code.
    #[test]
    fn writing_past_a_tenth_of_the_budget_triggers_a_sweep() {
        let a = TempDir::new("write_trigger_unbounded");
        let unbounded = ThumbStore::new(a.0.clone(), u64::MAX);
        write_entries(&unbounded, 40);
        let natural = unbounded.size_bytes();
        assert!(natural > 0, "the fixture wrote nothing");

        let b = TempDir::new("write_trigger_budgeted");
        let budget = (natural / 4).max(1);
        let budgeted = ThumbStore::new(b.0.clone(), budget);
        write_entries(&budgeted, 40);
        // ⚠ No explicit sweep: the puts themselves must have kept it in hand.
        assert!(
            budgeted.size_bytes() <= budget * 2,
            "writing triggered no sweep: {} bytes against a {budget} budget",
            budgeted.size_bytes()
        );
    }

    /// ⚠⚠ The remote header layout must agree with itself in both directions. It is a **second**
    /// layout — version 2 — because the header carries the identity's own fields and a remote asset
    /// has no path, mtime or size to put there.
    #[test]
    fn a_remote_thumbnail_round_trips_through_the_store() {
        let dir = TempDir::new("remote_round_trip");
        let store = dir.store();
        let id = Identity::remote("11111111-1111-4111-8111-111111111111", "v1");
        let t = Thumbnail {
            width: 3,
            height: 2,
            rgba: vec![9u8; 3 * 2 * 4],
        };
        store.put(&id, &t);
        let got = store.get(&id).expect("a remote entry must come back");
        assert_eq!((got.width, got.height), (3, 2));
    }

    /// ⚠ A remote entry read under a **different version** is a miss, not someone else's pixels —
    /// the same rule the local layout already follows.
    #[test]
    fn a_remote_entry_under_a_stale_version_is_a_miss() {
        let dir = TempDir::new("remote_stale");
        let store = dir.store();
        let t = Thumbnail {
            width: 1,
            height: 1,
            rgba: vec![0u8; 4],
        };
        store.put(&Identity::remote("abc", "v1"), &t);
        assert!(
            store.get(&Identity::remote("abc", "v2")).is_none(),
            "a stale version must not serve the old thumbnail"
        );
    }

    /// ⚠⚠ A remote identity must need **no filesystem at all** — that is the entire reason for the
    /// enum. `Identity::of` stats a file, and an Immich asset that has not been downloaded has none.
    #[test]
    fn a_remote_identity_needs_no_file_and_changes_with_its_version() {
        let a = Identity::remote(
            "11111111-1111-4111-8111-111111111111",
            "2026-04-27T12:34:32.784Z",
        );
        let b = Identity::remote(
            "11111111-1111-4111-8111-111111111111",
            "2026-04-27T12:34:33.000Z",
        );
        let c = Identity::remote(
            "22222222-2222-4222-8222-222222222222",
            "2026-04-27T12:34:32.784Z",
        );
        assert_ne!(a.file_name(), b.file_name(), "a new version is a new entry");
        assert_ne!(
            a.file_name(),
            c.file_name(),
            "a different asset is a different entry"
        );
        assert_eq!(
            a.file_name(),
            Identity::remote(
                "11111111-1111-4111-8111-111111111111",
                "2026-04-27T12:34:32.784Z"
            )
            .file_name()
        );
    }

    /// ⚠⚠ **A concrete filename, pinned on purpose.** The vectors above pin `fnv1a`; the test
    /// beside this one pins that the parts *matter*. **Neither pins the mapping itself**, so a
    /// change to `key_bytes` — adding an enum discriminant, reordering the fields, widening a
    /// type — would silently turn every thumbnail already on a user's disk into a miss, with the
    /// whole suite still green. This is the only thing standing between that change and a cache
    /// everybody silently loses.
    #[test]
    fn a_known_local_identity_keeps_its_filename_for_ever() {
        assert_eq!(
            ident("/photos/a.RAF", 100, 200).file_name(),
            "f89fcaaf26f07b00.rthm"
        );
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

    /// A temp directory that cleans itself up, the `PresetStore` pattern.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — not taken from the clock.
    /// `cargo test` runs in parallel, and two fixtures built in the same tick have collided in
    /// this repo before, with one test deleting a directory another was reading.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            Self(test_dir::path(&format!("thumbs-{tag}")))
        }

        fn store(&self) -> ThumbStore {
            // ⚠ No limit: an eviction test builds its own store with an explicit budget, and
            // every other test here would be made nondeterministic by one.
            ThumbStore::new(self.0.clone(), u64::MAX)
        }

        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .map(|d| {
                    d.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A thumbnail with structure, so a transposed or truncated payload is visible.
    fn thumb(w: usize, h: usize) -> Thumbnail {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        Thumbnail {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn a_thumbnail_round_trips_through_the_store() {
        let dir = TempDir::new("roundtrip");
        let store = dir.store();
        let id = ident("/photos/a.RAF", 100, 200);
        let src = thumb(64, 48);

        assert!(store.get(&id).is_none(), "nothing cached yet");
        store.put(&id, &src);

        let got = store.get(&id).expect("the entry must come back");
        assert_eq!((got.width, got.height), (64, 48), "dimensions are exact");
        assert_eq!(got.rgba.len(), src.rgba.len());
        // ⚠ A tolerance, not equality: the payload is JPEG, which is lossy. Tight enough that a
        // transposed or wrong-channel payload still fails.
        let worst = got
            .rgba
            .iter()
            .zip(&src.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 12,
            "worst channel difference {worst} is too large for q90"
        );
        // Every thumbnail is opaque; the alpha is reconstructed on read.
        assert!(got.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn a_changed_mtime_or_size_is_a_miss() {
        // ⚠ Both, because they are two independent ways a file changes and a test covering one
        // cannot see the other going unhashed.
        let dir = TempDir::new("changed");
        let store = dir.store();
        store.put(&ident("/photos/a.RAF", 100, 200), &thumb(16, 16));
        assert!(store.get(&ident("/photos/a.RAF", 101, 200)).is_none());
        assert!(store.get(&ident("/photos/a.RAF", 100, 201)).is_none());
        assert!(store.get(&ident("/photos/a.RAF", 100, 200)).is_some());
    }

    #[test]
    fn an_identity_mismatch_under_the_right_name_is_a_miss_not_wrong_pixels() {
        // ⚠⚠ THE POINT OF THE HEADER. An FNV collision is ~3e-10 over 100k entries, but the
        // failure it would otherwise produce is **someone else's photograph in your grid**, which
        // is worth five lines to make unrepresentable. Simulated by writing one identity's entry
        // under another's file name.
        let dir = TempDir::new("collide");
        let store = dir.store();
        let a = ident("/photos/a.RAF", 1, 1);
        let b = ident("/photos/b.RAF", 2, 2);
        store.put(&a, &thumb(16, 16));
        let bytes = std::fs::read(dir.0.join(a.file_name())).unwrap();
        std::fs::write(dir.0.join(b.file_name()), bytes).unwrap();
        assert!(
            store.get(&b).is_none(),
            "a collision must miss, never mis-serve"
        );
    }

    #[test]
    fn a_wrong_version_is_a_miss_not_a_misread() {
        let dir = TempDir::new("version");
        let store = dir.store();
        let id = ident("/photos/a.RAF", 1, 1);
        store.put(&id, &thumb(16, 16));
        let p = dir.0.join(id.file_name());
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[4] = bytes[4].wrapping_add(1); // the version field
        std::fs::write(&p, bytes).unwrap();
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn a_truncated_or_garbage_entry_is_a_miss_not_a_panic() {
        let dir = TempDir::new("garbage");
        let store = dir.store();
        let id = ident("/photos/a.RAF", 1, 1);
        store.put(&id, &thumb(16, 16));
        let p = dir.0.join(id.file_name());
        let bytes = std::fs::read(&p).unwrap();
        for cut in [0usize, 3, 8, bytes.len() / 2] {
            std::fs::write(&p, &bytes[..cut]).unwrap();
            assert!(store.get(&id).is_none(), "truncated to {cut} must miss");
        }
        std::fs::write(&p, b"not a cache entry at all").unwrap();
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn a_successful_write_leaves_no_temporary_behind() {
        // ⚠ The write is atomic — a temp file then a rename — or a crash mid-write leaves a
        // truncated JPEG that decodes to garbage. This asserts the temp does not survive.
        let dir = TempDir::new("atomic");
        let store = dir.store();
        let id = ident("/photos/a.RAF", 1, 1);
        store.put(&id, &thumb(16, 16));
        assert_eq!(dir.entries(), vec![id.file_name()]);
    }

    #[test]
    fn a_store_whose_directory_cannot_be_made_degrades_to_doing_nothing() {
        // ⚠ Rooted at a path that is a FILE, so `create_dir_all` must fail. Every failure here
        // has to degrade to "extract it", never to an error the user sees.
        let dir = TempDir::new("unwritable");
        let f = dir.0.join("a-file");
        std::fs::write(&f, b"x").unwrap();
        let store = ThumbStore::new(f.join("thumbs"), u64::MAX);
        let id = ident("/photos/a.RAF", 1, 1);
        store.put(&id, &thumb(16, 16));
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn the_size_counts_every_entry_and_clearing_empties_it() {
        let dir = TempDir::new("size");
        let store = dir.store();
        assert_eq!(
            store.size_bytes(),
            0,
            "an absent directory is zero, not an error"
        );

        store.put(&ident("/photos/a.RAF", 1, 1), &thumb(32, 32));
        store.put(&ident("/photos/b.RAF", 2, 2), &thumb(32, 32));
        let two = store.size_bytes();
        assert!(two > 0, "two entries must have a size");

        store.put(&ident("/photos/c.RAF", 3, 3), &thumb(32, 32));
        assert!(store.size_bytes() > two, "a third entry must grow it");

        store.clear();
        assert_eq!(store.size_bytes(), 0);
        assert!(dir.entries().is_empty(), "clearing removes the files");
        // ⚠ And the store still works afterwards, rather than being left in a broken state.
        store.put(&ident("/photos/a.RAF", 1, 1), &thumb(16, 16));
        assert!(store.get(&ident("/photos/a.RAF", 1, 1)).is_some());
    }
}

# M8.2 On-Disk Thumbnail Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **NO `Co-Authored-By`
> trailer** — the harness will suggest one; do not add it. **NEVER push**; the user pushes to
> `github.com/wladiKrin/rustroom` himself. Work on the branch `m8.2-thumbnail-cache`, which
> already exists.
>
> ⚠ **Build environment.** Before any cargo command:
> ```
> export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
> ```
>
> ⚠ **Read every gate's exit code from a file, never through a pipe.** `cargo clippy ... > /tmp/c.log 2>&1; echo "EXIT=$?"`.
> A `cargo test ... | grep` reports *grep's* status, and this project has shipped a failing clippy
> that way. Use **session-unique log names** — stale `/tmp/t.log` from an earlier session has
> matched a wait condition instantly and given a false "gate done".
>
> ⚠ **After every scripted edit, assert the new text is present.** A `sed` that matches nothing
> exits 0. After every sabotage, confirm the **named** test failed — `test result: ok` says
> nothing about what ran, and `cargo test "a b"` matches a *substring*, not a list.

**Goal:** Reopening a folder reads thumbnails from disk in ~1 ms each instead of re-extracting
them at ~191 ms each.

**Architecture:** A new lib module `thumb_store` owns a directory of cached **extracted previews**
— not finished thumbnails — keyed by `(path, mtime, size)`, stored as a small binary header plus
JPEG q90 bytes. It slots into exactly one place: the extract step inside
`thumb_loader::load_thumbnail`. `ThumbLoader`, `ThumbCache`, `ThumbKey` and every consumer are
untouched.

**Tech Stack:** Rust, the `image` crate (already a dependency of `app`) for JPEG encode/decode,
hand-rolled FNV-1a for the filename hash (no new dependency).

**Spec:** `docs/superpowers/specs/2026-09-10-thumbnail-cache-design.md`

---

## File Structure

| | |
|---|---|
| **Create** `crates/app/src/thumb_store.rs` | The store: identity, FNV-1a, the header format, read/write/clear/size. **No egui** — it is a lib module so it is unit-testable without a window. ~260 lines including tests. |
| **Modify** `crates/app/src/lib.rs` | Declare `pub mod thumb_store;`. |
| **Modify** `crates/app/src/thumb_loader.rs:78-91` | `load_thumbnail` gains a `store` parameter and consults it; `ThumbLoader::new` captures `ThumbStore::user_default()`. |
| **Modify** `crates/app/src/modals/settings.rs` | Size readout and a Clear button. |
| **Modify** `crates/app/src/main.rs` | The modal state field holding the computed size. |

⚠ `thumb_loader.rs` is **884 lines**, already well past the ~600 marker. It gains a parameter and
about ten lines here; everything substantive goes in the new module.

---

## Task 1: FNV-1a, pinned to known vectors

**Files:**
- Create: `crates/app/src/thumb_store.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Create the module with the hash and its tests**

Create `crates/app/src/thumb_store.rs`:

```rust
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
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
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
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod thumb_store;
```

- [ ] **Step 3: Run the test**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -5
```
Expected: `test result: ok. 1 passed`.

⚠ Read the **passed count**, not the word `ok`. A filter that matches nothing prints
`0 passed; N filtered out` and reads as green.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/thumb_store.rs crates/app/src/lib.rs
git commit -m "feat: FNV-1a for the thumbnail cache's on-disk key

Not DefaultHasher: its algorithm may change between Rust releases, so an
on-disk key built from it would silently invalidate the whole cache on a
toolchain upgrade. Pinned to the published vectors."
```

---

## Task 2: Identity and the cache filename

**Files:**
- Modify: `crates/app/src/thumb_store.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `crates/app/src/thumb_store.rs`:

```rust
    use std::path::{Path, PathBuf};

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
        assert!(
            n.chars().all(|c| c.is_ascii_hexdigit() || c == '.'),
            "expected hex plus an extension, got {n:?}"
        );
        assert!(n.ends_with(".rthm"), "got {n:?}");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -20
```
Expected: FAIL — `cannot find struct, variant or union type 'Identity'`.

- [ ] **Step 3: Implement `Identity`**

Insert into `crates/app/src/thumb_store.rs`, above the `#[cfg(test)]` module:

```rust
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
```

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -5
```
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Sabotage — hash the path only, and watch a named test fail**

```bash
cp crates/app/src/thumb_store.rs /tmp/m82-ts.bak
python3 - <<'PY'
p='crates/app/src/thumb_store.rs'
s=open(p).read()
old="""        v.push(0);
        v.extend_from_slice(&self.mtime_ns.to_le_bytes());
        v.extend_from_slice(&self.size.to_le_bytes());"""
assert old in s, "sabotage did not match — the edit would have silently no-opped"
open(p,'w').write(s.replace(old, "        v.push(0);"))
PY
cargo test -p app --lib thumb_store 2>&1 | grep -E "^test |test result"
cp /tmp/m82-ts.bak crates/app/src/thumb_store.rs
```
Expected: `the_filename_changes_with_every_part_of_the_identity ... FAILED`.

⚠⚠ **Restore from the `cp` backup, never with `git checkout -- <file>`.** This project has
destroyed a whole task's uncommitted work that way, twice.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/thumb_store.rs
git commit -m "feat: identity and flat filename for the thumbnail cache

(path, mtime, size), because a content hash means reading the 27 MB file
this cache exists to avoid reading."
```

---

## Task 3: The file format — header plus JPEG, written atomically

**Files:**
- Modify: `crates/app/src/thumb_store.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module:

```rust
    /// A temp directory that cleans itself up, the `PresetStore` pattern.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — not taken from the clock.
    /// `cargo test` runs in parallel, and two fixtures built in the same tick have collided in
    /// this repo before, with one test deleting a directory another was reading.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "rustroom_thumbs_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn store(&self) -> ThumbStore {
            ThumbStore::new(self.0.clone())
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
        assert!(worst <= 12, "worst channel difference {worst} is too large for q90");
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
        assert!(store.get(&b).is_none(), "a collision must miss, never mis-serve");
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
        let store = ThumbStore::new(f.join("thumbs"));
        let id = ident("/photos/a.RAF", 1, 1);
        store.put(&id, &thumb(16, 16));
        assert!(store.get(&id).is_none());
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -20
```
Expected: FAIL — `cannot find type 'ThumbStore' in this scope`.

- [ ] **Step 3: Implement the store**

Insert into `crates/app/src/thumb_store.rs`, above the `#[cfg(test)]` module:

```rust
/// The magic at the start of every entry.
const MAGIC: &[u8; 4] = b"RTHM";

/// The on-disk format version.
///
/// ⚠ Bump this whenever the header or the payload encoding changes. An entry with the wrong
/// version is a **miss**, so a bump invalidates cleanly instead of reinterpreting old bytes as
/// the new format — which is the failure a bare `.jpg` on disk could not protect against.
const VERSION: u16 = 1;

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
}

impl ThumbStore {
    /// The directory is injected so tests run against a temp dir and never touch the real home
    /// directory — the `PresetStore` and `SettingsStore` pattern.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<platform config dir>/rustroom/thumbs/`.
    ///
    /// ⚠ That is **not** `~/.config` everywhere — `dirs::config_dir()` follows each platform's own
    /// convention: `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` on Linux,
    /// `%APPDATA%` on Windows. Falls back to a relative path, which keeps this infallible.
    pub fn user_default() -> Self {
        Self::new(
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rustroom")
                .join("thumbs"),
        )
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
    if u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?) != VERSION {
        return None;
    }
    let w = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    let mtime_ns = i64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
    let size = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
    let path_len = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    let path = std::str::from_utf8(take(&mut p, path_len)?).ok()?;
    // ⚠⚠ The identity check is what turns a hash collision into a miss rather than someone
    // else's photograph in the grid.
    if mtime_ns != want.mtime_ns || size != want.size || path != want.path.to_string_lossy() {
        return None;
    }
    Some((w, h, bytes.get(p..)?))
}

/// Serialize one entry: header, then JPEG q90 bytes.
fn encode_entry(id: &Identity, thumb: &Thumbnail) -> Option<Vec<u8>> {
    use image::ImageEncoder;
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

    let path = id.path.to_string_lossy();
    let mut out = Vec::with_capacity(jpeg.len() + path.len() + 34);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&u32::try_from(thumb.width).ok()?.to_le_bytes());
    out.extend_from_slice(&u32::try_from(thumb.height).ok()?.to_le_bytes());
    out.extend_from_slice(&id.mtime_ns.to_le_bytes());
    out.extend_from_slice(&id.size.to_le_bytes());
    out.extend_from_slice(&u32::try_from(path.len()).ok()?.to_le_bytes());
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(&jpeg);
    Some(out)
}
```

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -5
```
Expected: `test result: ok. 10 passed`.

- [ ] **Step 5: Three sabotages, each must fail a named test**

```bash
cp crates/app/src/thumb_store.rs /tmp/m82-ts.bak
run_sabotage() {
  cargo test -p app --lib thumb_store > /tmp/m82-sab.log 2>&1
  if ! grep -q "test result:" /tmp/m82-sab.log; then
    echo "DID NOT COMPILE — the sabotage proves nothing"; else
    grep -E "FAILED|test result:" /tmp/m82-sab.log | head -5; fi
  cp /tmp/m82-ts.bak crates/app/src/thumb_store.rs
}

# (a) drop the identity check -> the collision test must fail
python3 - <<'PY'
p='crates/app/src/thumb_store.rs'; s=open(p).read()
old="    if mtime_ns != want.mtime_ns || size != want.size || path != want.path.to_string_lossy() {"
assert old in s, "sabotage (a) did not match"
open(p,'w').write(s.replace(old, "    if false {"))
PY
run_sabotage

# (b) drop the version check -> the version test must fail
python3 - <<'PY'
p='crates/app/src/thumb_store.rs'; s=open(p).read()
old="    if u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?) != VERSION {\n        return None;\n    }"
assert old in s, "sabotage (b) did not match"
open(p,'w').write(s.replace(old, "    let _ = take(&mut p, 2)?;"))
PY
run_sabotage

# (c) write non-atomically -> the no-temporary test must still pass, so assert the WRITE PATH
python3 - <<'PY'
p='crates/app/src/thumb_store.rs'; s=open(p).read()
old="""        let tmp = self.dir.join(format!("{}.tmp", id.file_name()));"""
assert old in s, "sabotage (c) did not match"
open(p,'w').write(s.replace(old, """        let tmp = self.dir.join(format!("{}.tmp", id.file_name()));
        let _ = std::fs::write(&tmp, b"leftover");"""))
PY
run_sabotage
```

Expected: (a) `an_identity_mismatch_under_the_right_name_is_a_miss_not_wrong_pixels ... FAILED`;
(b) `a_wrong_version_is_a_miss_not_a_misread ... FAILED`;
(c) `a_successful_write_leaves_no_temporary_behind ... FAILED`.

⚠ If any sabotage reports `DID NOT COMPILE`, it proves nothing — fix the sabotage and re-run.
A sabotage that fails to compile looks exactly like one that passes.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m82-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test -p app --lib thumb_store > /tmp/m82-test.log 2>&1; echo "TEST=$?"; tail -3 /tmp/m82-test.log
```
Both must be `0` before committing.

```bash
git add crates/app/src/thumb_store.rs
git commit -m "feat: the thumbnail cache's on-disk format

A small header plus JPEG q90, written atomically. The header does three
jobs: a version bump invalidates cleanly rather than reinterpreting old
bytes, the identity is re-checked so a hash collision is a miss rather
than someone else's photograph in the grid, and the dimensions are the
authority so a disagreeing payload is a miss.

Every failure degrades to a miss. A thumbnail cache that can stop a
folder opening is worse than no cache."
```

---

## Task 4: Wire it into `load_thumbnail` — the carrier

**Files:**
- Modify: `crates/app/src/thumb_loader.rs:78-91`

⚠⚠ **This is the task that matters.** Every test in Tasks 1–3 can be green while the store is
never consulted — which is exactly how creative LUTs shipped with four passing pipeline tests and
were 100% non-functional. The test here counts *extractions*.

- [ ] **Step 1: Write the failing carrier test**

Append to the `tests` module in `crates/app/src/thumb_loader.rs`:

```rust
    /// ⚠⚠ THE CARRIER. Everything in `thumb_store`'s own tests can pass while `load_thumbnail`
    /// never calls it. This counts the **extractions**, which is the only observable that can
    /// tell "the cache works" from "the cache is never asked".
    ///
    /// ⚠ It needs a real file on disk, because `Identity::of` stats it — a synthetic path has no
    /// mtime and would be uncacheable, so the test would pass for the wrong reason.
    #[test]
    fn a_second_load_reads_the_store_instead_of_extracting_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = std::env::temp_dir().join(format!(
            "rustroom_carrier_{}_{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("a.png");
        // A real 8×6 PNG, so `image_decode::thumbnail` genuinely succeeds.
        let img = image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8 * 8, y as u8 * 8, 90]));
        img.save(&photo).unwrap();

        let store = crate::thumb_store::ThumbStore::new(dir.join("thumbs"));
        static EXTRACTS: AtomicUsize = AtomicUsize::new(0);
        EXTRACTS.store(0, Ordering::Relaxed);

        let req = ThumbRequest {
            id: PhotoId::original(photo.clone()),
            edits: Edits::default(),
        };
        let extract = |p: &std::path::Path| {
            EXTRACTS.fetch_add(1, Ordering::Relaxed);
            image_decode::thumbnail(p).ok()
        };

        let first = load_with(&req, pipeline::Resampler::default(), Some(&store), &extract);
        assert!(first.is_some(), "the first load must produce a thumbnail");
        assert_eq!(EXTRACTS.load(Ordering::Relaxed), 1, "the first load extracts");

        let second = load_with(&req, pipeline::Resampler::default(), Some(&store), &extract);
        assert!(second.is_some(), "the second load must produce a thumbnail");
        assert_eq!(
            EXTRACTS.load(Ordering::Relaxed),
            1,
            "the SECOND load must come from the store, not from a second extraction"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ The other half: with no store the behaviour is exactly what it was before this feature.
    /// Without this, a `load_with` that ignored its `None` and always cached would pass above.
    #[test]
    fn with_no_store_every_load_extracts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!(
            "rustroom_nostore_{}_{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("a.png");
        image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8, y as u8, 7]))
            .save(&photo)
            .unwrap();

        static N: AtomicUsize = AtomicUsize::new(0);
        N.store(0, Ordering::Relaxed);
        let req = ThumbRequest {
            id: PhotoId::original(photo),
            edits: Edits::default(),
        };
        let extract = |p: &std::path::Path| {
            N.fetch_add(1, Ordering::Relaxed);
            image_decode::thumbnail(p).ok()
        };
        let _ = load_with(&req, pipeline::Resampler::default(), None, &extract);
        let _ = load_with(&req, pipeline::Resampler::default(), None, &extract);
        assert_eq!(N.load(Ordering::Relaxed), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib thumb_loader 2>&1 | tail -20
```
Expected: FAIL — `cannot find function 'load_with' in this scope`.

- [ ] **Step 3: Implement the wiring**

In `crates/app/src/thumb_loader.rs`, replace the body of `load_thumbnail` (currently lines 78–91)
with the following. Keep the existing doc comment above `load_thumbnail` exactly as it is and add
the new one to `load_with`:

```rust
fn load_thumbnail(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    store: Option<&crate::thumb_store::ThumbStore>,
) -> Option<Thumbnail> {
    load_with(req, resampler, store, &extract_preview)
}

/// Pull the source's own preview out of it, with no cache in sight.
///
/// ⚠ Split out so the carrier test can count how many times it runs. That count is the only
/// observable that distinguishes "the cache works" from "the cache is never consulted".
fn extract_preview(path: &std::path::Path) -> Option<Thumbnail> {
    if catalog::is_raw(path) {
        raw_decode::extract_thumbnail(path).ok()
    } else {
        image_decode::thumbnail(path).ok()
    }
}

/// `load_thumbnail` with the extraction injected.
///
/// ⚠⚠ **The store caches the EXTRACTED PREVIEW, before the render — never the finished
/// thumbnail.** Extraction is ~191 ms and the render is ~5 ms, so a store keyed on edits would be
/// useless for the case that matters most while editing (a changed edit) and would leave a file
/// behind for every edit state a photo has ever had. `ThumbCache` in memory is the level that is
/// keyed on edits; this one is keyed on the file.
///
/// ⚠ `Identity::of` stats the file. A source with no readable metadata is simply not cached,
/// which degrades to today's behaviour rather than to an error.
fn load_with(
    req: &ThumbRequest,
    resampler: pipeline::Resampler,
    store: Option<&crate::thumb_store::ThumbStore>,
    extract: &dyn Fn(&std::path::Path) -> Option<Thumbnail>,
) -> Option<Thumbnail> {
    let path = req.id.path.as_path();
    let id = store.and_then(|_| crate::thumb_store::Identity::of(path));
    let cached = match (store, &id) {
        (Some(s), Some(i)) => s.get(i),
        _ => None,
    };
    let thumb = match cached {
        Some(t) => t,
        None => {
            let t = extract(path)?;
            if let (Some(s), Some(i)) = (store, &id) {
                s.put(i, &t);
            }
            t
        }
    };
    if req.edits == Edits::default() {
        // ⚠ Neutral edits are the common case on a first folder open; skip the round trip rather
        // than paying it 830 times for a no-op.
        return Some(thumb);
    }
    Some(render_edits(thumb, &thumbnail_edits(&req.edits), resampler))
}
```

Then update the two existing call sites in the same file:

```rust
// In `ThumbLoader::new`, replace the closure with one that captures a store:
let store = crate::thumb_store::ThumbStore::user_default();
Self::with_loader(
    Arc::new(move |r: &ThumbRequest| {
        load_thumbnail(r, pipeline::Resampler::default(), Some(&store))
    }),
    worker_count(),
    display_space,
    on_progress,
)
```

```rust
// In the tests module, the existing helper:
fn load_thumbnail_default(req: &ThumbRequest) -> Option<Thumbnail> {
    load_thumbnail(req, pipeline::Resampler::default(), None)
}
```

⚠ `ThumbLoader::new`'s **signature does not change**, so its four call sites — `settings_apply.rs`,
`sources.rs`, `test_app.rs` and this file's own tests — are untouched.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test -p app --lib thumb 2>&1 | tail -5
```
Expected: all `thumb_store` and `thumb_loader` tests pass, with the count up by 2.

- [ ] **Step 5: Sabotage — make the store never return anything**

```bash
cp crates/app/src/thumb_store.rs /tmp/m82-ts.bak
python3 - <<'PY'
p='crates/app/src/thumb_store.rs'; s=open(p).read()
old="    pub fn get(&self, id: &Identity) -> Option<Thumbnail> {"
assert old in s, "sabotage did not match"
open(p,'w').write(s.replace(old, old + "\n        if true { return None; }"))
PY
cargo test -p app --lib thumb > /tmp/m82-sab.log 2>&1
grep -q "test result:" /tmp/m82-sab.log && grep -E "FAILED" /tmp/m82-sab.log | head -3 || echo "DID NOT COMPILE"
cp /tmp/m82-ts.bak crates/app/src/thumb_store.rs
```
Expected: `a_second_load_reads_the_store_instead_of_extracting_again ... FAILED`.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m82-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test -p app --lib thumb > /tmp/m82-test.log 2>&1; echo "TEST=$?"; tail -3 /tmp/m82-test.log
```

```bash
git add crates/app/src/thumb_loader.rs
git commit -m "feat: load_thumbnail reads the on-disk cache before extracting

The carrier: everything in thumb_store's own tests can pass while
load_thumbnail never calls it, which is exactly how creative LUTs
shipped fully tested and completely dead. The test counts extractions.

ThumbLoader::new's signature is unchanged, so its four call sites are
untouched."
```

---

## Task 5: Settings — size readout and Clear

**Files:**
- Modify: `crates/app/src/thumb_store.rs`
- Modify: `crates/app/src/modals/settings.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing tests for `size_bytes` and `clear`**

Append to the `tests` module in `crates/app/src/thumb_store.rs`:

```rust
    #[test]
    fn the_size_counts_every_entry_and_clearing_empties_it() {
        let dir = TempDir::new("size");
        let store = dir.store();
        assert_eq!(store.size_bytes(), 0, "an absent directory is zero, not an error");

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
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -10
```
Expected: FAIL — `no method named 'size_bytes'`.

- [ ] **Step 3: Implement both**

Add to `impl ThumbStore` in `crates/app/src/thumb_store.rs`:

```rust
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
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p app --lib thumb_store 2>&1 | tail -5
```
Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Add the field that caches the size while the dialog is open**

In `crates/app/src/main.rs`, find the `RustroomApp` struct and add a field beside the other modal
scratch state:

```rust
    /// The thumbnail cache's size in bytes, read **once** when the Settings dialog opens.
    ///
    /// ⚠ `None` means "not yet read". Computing it walks the cache directory, and a dialog body
    /// runs sixty times a second — the `apply_view_demosaic` rule. The `info` dialog's `dims`
    /// field is the precedent.
    pub thumb_cache_bytes: Option<u64>,
```

Initialise it to `None` in `RustroomApp::new` and in `crates/app/src/test_app.rs`'s exhaustive
literal.

⚠ The `test_app.rs` literal is **exhaustive on purpose** — the compiler will name it. Do not
reach for `..Default::default()`.

- [ ] **Step 6: Draw the readout and the button**

In `crates/app/src/modals/settings.rs`, inside the dialog body after the existing
`ui.separator();` that follows the 1:1 view quality row, add:

```rust
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Thumbnail cache");
                // ⚠ Read once, on the first frame the dialog is open. `get_or_insert_with` is
                // what makes that "once" rather than "every frame".
                let store = rustroom_app::thumb_store::ThumbStore::user_default();
                let bytes = *app
                    .thumb_cache_bytes
                    .get_or_insert_with(|| store.size_bytes());
                ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                if ui.button("Clear").clicked() {
                    store.clear();
                    app.thumb_cache_bytes = Some(0);
                }
            });
            theme::hint(
                ui,
                "Thumbnails already extracted from your photos, so a folder opens \
                 without re-reading every file. Clearing costs nothing but the time \
                 to build it again the next time you open a folder.",
            );
```

And where the dialog is closed (the same place `app.modal = Modal::None` is set for Settings), add:

```rust
        // ⚠ Cleared on close so the next open re-reads it rather than showing a stale size.
        app.thumb_cache_bytes = None;
```

- [ ] **Step 7: Gate**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m82-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m82-test.log 2>&1; echo "TEST=$?"
grep -o "test result: ok\. [0-9]* passed" /tmp/m82-test.log | awk '{s+=$4} END {print "TOTAL:", s}'
grep -c "^test golden_" /tmp/m82-test.log
git status --short crates/pipeline/tests/
```
Expected: `CLIPPY=0`, `TEST=0`, **63 goldens ran and none moved** (the last command prints
nothing). This feature touches no pixel path, so a moved golden means something is wrong.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: thumbnail cache size readout and Clear in Settings

The size walks the directory, so it is read once when the dialog opens
and held in the modal's scratch state -- a dialog body runs sixty times
a second. Clearing is not confirmed: it destroys nothing the app cannot
rebuild."
```

---

## Task 6: Measure it, and record what was actually achieved

**Files:**
- Modify: `crates/app/examples/bench_thumb_cache.rs`

⚠⚠ The spec's `~0.15 s` warm open is a **floor extrapolated from an in-memory JPEG decode**, not
an achieved figure. It must be measured against the real store before it is quoted anywhere.

- [ ] **Step 1: Add a real cold-vs-warm measurement to the probe**

Append to `main` in `crates/app/examples/bench_thumb_cache.rs`:

```rust
    // ── 3. the achieved figure: the real store, cold then warm ─────────────────────────
    //
    // ⚠ Against a temp directory, never the user's own cache — a probe that populated the real
    // one would make its own second run meaningless.
    let tmp = std::env::temp_dir().join(format!("rustroom_bench_thumbs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let store = rustroom_app::thumb_store::ThumbStore::new(tmp.clone());

    for round in ["cold", "warm"] {
        let t = Instant::now();
        let mut hit = 0usize;
        for e in &sample {
            let p = e.path.as_path();
            let Some(id) = rustroom_app::thumb_store::Identity::of(p) else {
                continue;
            };
            if let Some(_) = store.get(&id) {
                hit += 1;
            } else if let Some(th) = if catalog::is_raw(p) {
                raw_decode::extract_thumbnail(p).ok()
            } else {
                image_decode::thumbnail(p).ok()
            } {
                store.put(&id, &th);
            }
        }
        println!(
            "\n  {round:>4} pass over {} entries: {:8.1} ms total, {:6.1} ms each, {hit} from cache",
            sample.len(),
            ms(t),
            ms(t) / sample.len() as f64
        );
    }
    println!("  cache dir now {:.1} MB", store.size_bytes() as f64 / 1048576.0);
    let _ = std::fs::remove_dir_all(&tmp);
```

- [ ] **Step 2: Run it**

```bash
cargo run --release -p app --example bench_thumb_cache ~/Pictures/fuji 60 2>&1 | tail -25
```
Expected: the warm pass reports `60 from cache` and a per-entry time **at least 50× below** the
cold pass. If the warm pass reports `0 from cache`, the store is not being hit — stop and
diagnose rather than adjusting the number.

- [ ] **Step 3: Commit the probe**

```bash
git add crates/app/examples/bench_thumb_cache.rs
git commit -m "test: measure the thumbnail cache cold vs warm, against the real store

The spec's ~0.15 s warm open was a floor extrapolated from an in-memory
JPEG decode. This is the achieved figure."
```

---

## Task 7: Documentation and the GUI check

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Add the M8.2 section to `CLAUDE.md`**

Insert a section immediately above `**M8.4 — the demosaic decision (2026-09-10).`, following the
house style — a bold one-line summary, then `⚠`-marked bullets for each finding, then an
"Accepted caveats" bullet. It must record, at minimum:

- the measured cold and warm figures from Task 6 (the **achieved** ones, not the spec's floor);
- that a full-size camera JPEG costs 2.6× a RAW to thumbnail, and why (the RAW carries an embedded
  preview and the JPEG does not) — plus that this makes a DCT-scaled JPEG decode the lever for the
  **cold** open, deliberately out of scope;
- that the store caches the **preview**, not the finished thumbnail, and the ratio that decided it;
- that `DefaultHasher` is unusable for an on-disk key and why;
- the header's three jobs;
- the accepted caveats from spec §11, verbatim in substance.

- [ ] **Step 2: Update the `▶ NEXT ACTION` block in `CLAUDE.md`**

Mark M8.2 done and point at what remains in Phase 8: **M8.1**'s leftovers (parallel export,
incremental brush splatting, the range-mask coverage cache) and **M8.3** the `wgpu` seam.

⚠ Add a new `State at handoff (2026-09-10, latest)` paragraph and demote the existing one to
`⚠ Superseded handoff follows:`, matching the established pattern. It must say **NOT confirmed by
use** until the user reports otherwise.

- [ ] **Step 3: Update `docs/BACKLOG.md`**

Change the `| On-disk thumbnail cache | **M8.2** |` row to record it as shipped, with the achieved
cold/warm figures and the spec path.

- [ ] **Step 4: Full gate on the finished tree**

```bash
cargo fmt --all --check > /tmp/m82-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m82-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m82-test.log 2>&1; echo "TEST=$?"
grep -o "test result: ok\. [0-9]* passed" /tmp/m82-test.log | awk '{s+=$4} END {print "TOTAL:", s}'
grep -c "^test golden_" /tmp/m82-test.log
git status --short crates/pipeline/tests/
```
All three exit codes must be `0`, 63 goldens must have run, and none may have moved.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: record M8.2, the on-disk thumbnail cache"
```

- [ ] **Step 6: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app
nohup ./target/release/Rustroom > /tmp/m82-gui.log 2>&1 &
sleep 6
pgrep -f "target/release/Rustroom" > /dev/null && echo "ALIVE" || cat /tmp/m82-gui.log
```

Tell the user what to check, and be explicit that the **first** open of a folder is unchanged:

1. Open `~/Pictures/fuji`. The first pass is as slow as it has always been (~26 s for 830).
2. Switch to another folder and back, or quit and relaunch. **The thumbnails should appear
   essentially at once.** This is the whole feature.
3. Settings shows a **Thumbnail cache** size of roughly 50 MB for that folder. Press **Clear** and
   it reads 0.0 MB.
4. Edit a photo. Its grid and filmstrip thumbnail must still update — the disk cache holds the
   *preview*, so the render still runs and edit-accurate thumbnails are unaffected.
5. ⚠ Modify a file outside Rustroom (`touch` one) and reopen the folder: that one thumbnail
   re-extracts and the rest do not.

---

## Self-Review

**Spec coverage.** §1 measurement → Task 6. §2 preview-not-finished → Task 4. §3 config dir →
Task 3 (`user_default`). §4 identity and FNV → Tasks 1–2. §5 file format → Task 3. §6 slot-in →
Task 4. §7 Settings → Task 5. §8 failure posture → Task 3 (`a_store_whose_directory_cannot_be_made
_degrades_to_doing_nothing`) and every `Option` return. §9 testing → Tasks 1–5, all eight unit
tests plus the carrier. §10 files → the File Structure table. §11 caveats → Task 7 Step 1.

**Placeholder scan.** No "TBD", no "add error handling", no "similar to Task N". Every code step
carries the actual code. Task 7 Step 1 describes a documentation section rather than quoting it,
which is deliberate — it must contain figures that do not exist until Task 6 runs, and inventing
them here is exactly the failure the step exists to prevent.

**Type consistency.** `Identity { path, mtime_ns, size }`, `Identity::of`, `Identity::file_name`,
`ThumbStore::{new, user_default, dir, get, put, size_bytes, clear}`, `fnv1a`, `decode_header`,
`encode_entry`, `load_with`, `extract_preview`, `MAGIC`, `VERSION`, `QUALITY`,
`app.thumb_cache_bytes` — each is defined in exactly one task and spelled identically wherever it
is used afterwards. `ThumbLoader::new` keeps its two-argument signature throughout.

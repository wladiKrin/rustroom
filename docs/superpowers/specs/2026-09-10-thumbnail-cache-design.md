# M8.2 — an on-disk thumbnail cache

**Date:** 2026-09-10
**Status:** design approved, not yet implemented.
**Done when:** reopening an 830-photo folder does not re-extract every thumbnail.

---

## 1. The measurement, which came first

`crates/app/examples/bench_thumb_cache.rs` (kept), run on the author's real folder — 830 entries,
415 `.RAF` at a mean 27.3 MB and 415 `.JPG` at a mean 11.9 MB.

| | per thumbnail | 830 entries, 6 workers |
|---|---|---|
| **extract today** | **191.4 ms** | **~26.5 s** |
| — of which RAW | 105.1 ms | |
| — of which JPEG/PNG/TIFF | 277.7 ms | |
| read back as JPEG q90 | **1.10 ms** | **~0.15 s** |

**172×.** That is the whole case for the feature.

⚠ **The 1.10 ms is a JPEG *decode*, measured from memory; a warm open also has to read ~61 KB off
disk.** For a file that size, on any machine this runs on, that is page-cache-warm or a single
small read either way — but the extrapolated ~0.15 s is a floor rather than a prediction, and it
should be re-measured against the real store before being quoted as an achieved figure.

### ⚠⚠ A finding that inverts the intuition: a full-size JPEG costs 2.6× a RAW

`raw_decode::extract_thumbnail` pulls the camera's **embedded preview** out of the RAW and decodes
only that. A 12 MB camera JPEG has no embedded preview, so `image_decode::thumbnail` decodes all
26 Mpx and then downscales. **The cache helps non-RAW sources more than RAW ones**, which is the
opposite of what "RAW files are big and slow" suggests.

⚠ It also names a *separate* optimisation that is **not** part of M8.2 and should not be folded
in: a JPEG can be decoded at 1/8 scale in the DCT domain far more cheaply than decoding it whole.
That would speed up the **cold** open, which nothing here touches. Recorded so it is not lost.

### The format is settled by the disk column, not by argument

| | on disk, 830 entries | read |
|---|---|---|
| **JPEG q90** | **49.7 MB** | 1.10 ms |
| PNG | 267.0 MB | 1.65 ms |
| raw RGBA | 552.8 MB | ~0 ms |

Raw RGBA saves 1.10 ms and costs **11×** the disk. That 1.10 ms is noise against the 191 ms it
replaces, so it buys nothing. **JPEG q90.**

⚠ q90 measures 61.3 KB per thumbnail, which is generous for a 512 px image (~2.8 bits/pixel).
It is a tunable, not a derived constant; the measured number is recorded at the constant so a
future change is made against data rather than taste.

## 2. What is cached: the **extracted preview**, not the finished thumbnail

The obvious design mirrors the in-memory cache — store the finished thumbnail keyed by `ThumbKey`
(photo + edits fingerprint), a drop-in with the same key and shape. **Rejected.**

Extraction is 191 ms; rendering an edited thumbnail over it is ~5 ms
(`bench_thumb_render`, and `thumb_loader.rs`'s own doc comment).

| | finished thumbnail | **extracted preview** |
|---|---|---|
| key | photo + edits | photo + mtime + size |
| warm open, neutral edits | 1.1 ms | 1.1 ms |
| warm open, edited | 1.1 ms | ~6 ms |
| **an edit changes** | **miss → ~196 ms** | **~6 ms** |
| files per photo | one per edit state, ever | one |

⚠⚠ **Caching the finished thumbnail makes the disk cache useless for the case `thumb_loader.rs`
already flags as the expensive one** — *"it matters most on invalidation: a changed edit costs
another ~95 ms here"* — and leaves a file behind for every edit state a photo has ever had.

The two levels then each cache what they are named for: **disk holds previews, memory holds
rendered thumbnails.** Neither needs invalidation code, for the reason `ThumbKey` already
documents — a changed edit is simply a different in-memory key, and a changed *file* is a
different on-disk one.

## 3. Where it lives

`<config>/rustroom/thumbs/` — one global cache, beside `presets/`, `trash/`, `settings.json` and
the catalog. (User's call, 2026-09-10.)

Chosen over a hidden directory beside the photos: that would write into the user's photo folders,
break on read-only media and network shares, and be swept up by backups and cloud sync. The cost,
stated: **copying a photo folder to another machine leaves its thumbnails behind**, and the cache
accumulates across every folder ever opened.

⚠ `dirs::config_dir()` follows each platform's own convention, so this is
`~/Library/Application Support/rustroom/thumbs/` on macOS and `$XDG_CONFIG_HOME/rustroom/thumbs/`
on Linux. Do not write `~/.config` — that is wrong on macOS, an error this file has made before.

## 4. Identity and the on-disk key

**Identity is `(path, mtime, size)`** — one `stat`. A content hash would mean reading 27 MB, which
is the cost the cache exists to avoid.

**The filename is a hex FNV-1a hash of that identity**, so the directory is flat and every entry
has a filesystem-safe name whatever the source path contains.

⚠⚠ **Not `DefaultHasher`.** `std::collections::hash_map::DefaultHasher`'s algorithm is explicitly
allowed to change between Rust releases, so an on-disk key built from it would **silently
invalidate the entire cache on a toolchain upgrade** — a 26-second regression appearing for no
reason anybody could connect to anything. `edits_fingerprint` gets away with it precisely because
it never leaves memory, and its doc comment should say so once this ships. FNV-1a is five lines,
stable by definition, and adds no dependency — the argument that already kept CRC-32 hand-rolled.

## 5. The file format

A small header followed by the JPEG payload, in one file:

```
magic   : b"RTHM"        4 bytes
version : u16            format version, little-endian
w, h    : u32, u32       the thumbnail's dimensions
mtime   : i64            the source's modification time, nanoseconds
size    : u64            the source's length in bytes
path    : u32 len + UTF-8 bytes
jpeg    : the rest of the file
```

The header does **three** jobs, and each is a failure it makes unrepresentable:

1. **Versioning.** A format change bumps `version`; an entry with the wrong version is a miss, not
   a misread. Without it, changing the payload silently reinterprets every existing file.
2. **Collision safety.** The identity is re-checked on read, so an FNV collision is a **miss**
   rather than **someone else's photograph in your grid**. At 64 bits over 100k entries a
   collision is ~3e-10, but the failure mode is bad enough to be worth five lines.
3. **Staleness.** Belt and braces: mtime and size are already in the hash, so a modified file
   lands on a different filename anyway.

⚠ **Writes are atomic** — write `<hex>.tmp`, then `rename` — or a crash mid-write leaves a
truncated JPEG that either fails to decode or decodes to garbage. `rename` within one directory is
atomic on every platform we target.

⚠ **Dimensions are stored rather than re-derived** from the JPEG, so a read knows the size before
decoding and a payload that disagrees with its header is detectable.

⚠ **`mtime` is nanoseconds since the Unix epoch as a signed `i64`**, so a pre-1970 timestamp is
representable rather than an error. A file whose `modified()` cannot be read at all has no
identity, so it is simply not cached — never cached under a guessed one.

⚠ **Accepted limit: mtime resolution is the filesystem's.** APFS and ext4 record nanoseconds, but
a filesystem with one-second granularity could give a stale hit for a file rewritten **within the
same second and to exactly the same length**. Recorded rather than guarded, because guarding it
means hashing content, which is the cost the whole feature exists to avoid.

## 6. Where it slots into the existing code

**One place: inside `load_thumbnail`, replacing the extract step.** Nothing else changes.

```rust
fn load_thumbnail(req: &ThumbRequest, resampler: Resampler, store: Option<&ThumbStore>) -> Option<Thumbnail> {
    let thumb = get_or_extract(store, req.id.path.as_path())?;   // 1.1 ms warm, 191 ms cold
    if req.edits == Edits::default() {
        return Some(thumb);                                       // unchanged
    }
    Some(render_edits(thumb, &thumbnail_edits(&req.edits), resampler))
}
```

`ThumbLoader`, `ThumbCache`, `ThumbKey`, `thumb_window` and every consumer of them are
**untouched**. That is the payoff of caching the preview: the seam is one function deep.

⚠ The store is threaded through the existing injected `LoadFn` closure, which already captures the
resampler. It is `Option<&ThumbStore>` for the same reason `Catalog` is an `Option`: a cache that
cannot be created must never stop the app extracting thumbnails.

⚠ **`ThumbStore` must be `Send + Sync`** — six loader workers share it. It holds a `PathBuf` and
nothing else, so this is free; there is no in-process index to lock, and the filesystem is the
index. Two workers never race on one file because `ThumbLoader`'s in-flight set already prevents
the same path being fetched twice.

## 7. Settings

A size readout and a **Clear thumbnail cache** button, beside the existing preferences.

⚠ The size means walking the directory, so it is computed **when the dialog opens** and held in
the modal's own state — the `info` dialog's `dims` precedent. A dialog body runs sixty times a
second, and a directory walk there is the `apply_view_demosaic` mistake in yet another costume.

⚠ Clearing is **not** confirmed. It destroys nothing the app cannot rebuild, which is the
distinction that already separates deleting a collection from forgetting a folder.

## 8. Failure posture

Every failure degrades to *extract it*, never to an error the user sees:

- the directory cannot be created → the store is `None`, the cache is off
- a read fails, or the header does not match → a miss, and the entry is **overwritten** by the
  fresh extraction rather than left to fail again on every future open
- a write fails → the thumbnail is still returned, just not cached

⚠ Nothing here may propagate an error into the loader. A thumbnail cache that can stop a folder
opening is worse than no cache.

## 9. Testing

The store takes its directory by injection, so all of this runs against a temp dir with no image
fixtures — the `PresetStore` and `SettingsStore` pattern.

**Unit, in `thumb_store.rs`:**

1. Round trip — write, read back, pixels within a JPEG tolerance and dimensions exact.
2. A changed **mtime** is a miss.
3. A changed **size** is a miss.
4. A wrong **version** is a miss, not a misread.
5. A **forced identity mismatch** under the right filename is a miss — never wrong pixels.
6. A **truncated or garbage** file is a miss, not a panic.
7. No `.tmp` file survives a successful write.
8. FNV-1a is **pinned to known vectors**, so a "tidy-up" that swaps in a different hash is a
   failing test rather than a silently emptied cache.

**The carrier test, which is the one that matters:** call `load_thumbnail` twice against a store
and assert the **extraction** ran exactly once, counting extractions through the injected seam.

⚠⚠ Every test above can be green while the store is never consulted at all. That is exactly how
creative LUTs shipped with four passing pipeline tests and were 100% non-functional. Ask what
carries the data to the tested code, and test *that*.

**Sabotage list** (each must fail a *named* test): make `get` always return `None`; drop the
identity check; drop the version check; write non-atomically; hash the path only, without mtime.

## 10. Files

| | |
|---|---|
| **Create** | `crates/app/src/thumb_store.rs` — the store. No egui, so it is a **lib** module. |
| **Modify** | `crates/app/src/thumb_loader.rs` — `load_thumbnail` consults the store; the production `LoadFn` closure captures it. |
| **Modify** | `crates/app/src/main.rs` — construct the store next to `PresetStore`/`SettingsStore`. |
| **Modify** | `crates/app/src/modals/settings.rs` — readout and Clear button. |

⚠ `thumb_loader.rs` is **884 lines**, already well past the ~600 marker, and gains only a
parameter and a call here. The store is its own module — which is the right placement regardless,
since a byte-level on-disk format and a worker pool are two different responsibilities.

## 11. What this does not do

1. **Nothing for the first open of a folder.** The cold path is unchanged at ~26.5 s for 830
   entries. §1 names the DCT-scaled JPEG decode as the lever that would move it, deliberately out
   of scope here.
2. **Nothing for the develop canvas.** This is the grid and the filmstrip only.
3. **An edited photo still renders on a warm open** — ~5 ms per thumbnail, by design, because the
   alternative is a file per edit state.
4. **q90 is a generational re-encode** of an already-lossy camera preview. Invisible at 512 px,
   and real.
5. **The cache is unbounded** (user's call). Settings shows the size and offers Clear; nothing
   evicts. ~60 KB per photo is 0.2% of a 27 MB RAW, and the alternatives were an
   oldest-written policy that is not really LRU, or an age cap — which this project already
   rejected once for the preset trash, because the entry you want back is the one that just
   silently expired.
6. **Thumbnails do not travel with a copied photo folder**, the stated cost of a global cache.

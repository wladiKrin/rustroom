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
}

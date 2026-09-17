# The coverage repack and the narrowing — the CPU halves

**Status:** spec · 2026-09-17
**From:** the measurement wave of 2026-09-16 (`docs/BACKLOG.md`, `## Performance`, the GPU-side mask
coverage cache entry).

## What this is, and what it deliberately is not

The wave measured the mask coverage upload at **7.9–15.8 ms** and the download at **4.0–9.4 ms**
against a borrowed ~2 ms ceiling, and found that **neither is mostly a transfer**. The actual
`write_buffer` is **0.75–1.16 ms**. This milestone takes the **CPU** work on both sides, which is
where the time is.

⚠⚠ **NOT the wire.** `brush_raster.rs` already records why coverage crosses as one `u32` per
sample: *"packing four samples into one word would mean four threads read-modify-writing the same
address — a race."* **That is a considered trade with a documented hazard, not an oversight**, and
calling its 4× "inflation" was wrong. Narrowing the wire needs a restructured dispatch (one thread
per four samples) or a second pack pass. **Re-price it after this milestone, against what is left.**

## The three defects, all CPU, all mechanical

**1. The upload repack is a serial per-byte loop.** `GpuMaskStage::pack` (`mask.rs:498`):

```rust
for (k, v) in m.data.iter().enumerate() {
    packed[start + (k >> 2)] |= (*v as u32) << ((k & 3) * 8);
}
```

Measured at **10.6–13.8 ms, ~90% of the upload, on EVERY encode** — not once. ⚠ The shifts spell
out **little-endian** packing, so this is `u32::from_le_bytes` over 4-byte chunks written the long
way round; nothing about it needs to be sequential.

**2. The download allocates 27.35 MB to throw it away.** `brush_raster.rs:435`:

```rust
let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
...
let data: Vec<u8> = words.iter().map(|&v| v.min(255) as u8).collect();
```

At rung 3200 that `to_vec()` copies **27.35 MB** into a buffer whose only use is the narrowing one
line later. ⚠ **Nobody has counted this** — it is not in the backlog entry, and it was found while
reading for this spec.

**3. The narrowing itself is serial** — 6.84 M elements on one thread.

## Goal

Make all three parallel or unnecessary, **with the produced bytes identical**.

## Non-goals

- **The wire.** See above. Deferred, and to be re-priced rather than assumed.
- **The GPU-side coverage cache.** Still reserved for its design question — *does it follow
  `GpuCache`'s "outlive the render" rule or `PrefixCache`'s opposite one?* ⚠ Doing this first is
  what lets that be priced against a smaller remainder.
- No change to what coverage *means*: same values, same layout, same `CoverageMap`.

## Design

**1. `pack`** builds each word from its four bytes instead of OR-ing byte by byte, over
`par_chunks` — `rayon` is already this crate's parallelism. ⚠ The trailing partial word must be
padded, not dropped: `words_for` rounds up with `div_ceil(4)` and the shader reads whole words.

**2 + 3** collapse into one pass: narrow **straight from the mapped range** into the `Vec<u8>`,
in parallel, with no intermediate `Vec<u32>`.

⚠ **The mapped range must still be released before `unmap`**, and the staging buffer still returned
and the plane still deposited only after `poll(Wait)` — three rules the surrounding comments state
and that this must not disturb.

## Done when

1. `pack` is parallel and its output is **byte-identical** to the old loop's.
2. The download narrows without the intermediate `Vec<u32>`, in parallel, producing an identical
   `CoverageMap.data`.
3. **Tests, in `gpu-pipeline`:** a `pack` equivalence test against a straightforward reference
   implementation, over sizes that are **not** multiples of 4, so the trailing partial word is
   exercised. ⚠ A size that divides evenly would not test the padding.
4. **A sabotage step:** breaking the packing — dropping the trailing word, or swapping the byte
   order — must make that test FAIL.
5. Re-measured with `coverage_transfer_probe` (committed 2026-09-16): report the new upload and
   download bands beside the old **7.9–15.8** and **4.0–9.4**. ⚠ Quote a band over ≥3 runs and state
   the machine's load — ⚠⚠ `julia` and `obsidian-notes-rag` have each sat at ~100% during every
   measurement on this machine so far.
6. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code read from its own command**, **74 goldens ran**, and the gated commit verified
   to BE the tip.
7. ⚠ Confirmed **by use**: drag a mask's sliders with the GPU on; the picture must not change.

## Risk

⚠ **A wrong byte order or a dropped trailing word is a plausible mask, not an error** — the same
failure class as every coverage bug this project has had. Item 4's sabotage is what stops it
shipping green, and item 7 is what stops it shipping unseen.

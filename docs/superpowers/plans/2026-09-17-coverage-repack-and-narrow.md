# The coverage repack and the narrowing: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the serial per-byte repack on the upload and the 27 MB throwaway copy plus serial narrow on the download, producing byte-identical coverage.

**Architecture:** Both are straight rewrites into chunked `u32::from_le_bytes` passes. ⚠ **No rayon**: `gpu-pipeline` does not depend on it, and a chunked scan removes a read-modify-write per byte before any thread is added. Measure first; earn the dependency only if still hot.

**Spec:** `docs/superpowers/specs/2026-09-17-coverage-repack-and-narrow.md`

⚠ Prefix cargo with the `PKG_CONFIG_PATH` export. ⚠⚠ Never run two gates concurrently.

---

### Task 1: A reference test for `pack`, before touching it

**Files:** Modify `crates/gpu-pipeline/src/mask.rs` (tests module, after `use super::*`)

- [ ] **Step 1: Write the test against the CURRENT implementation**

```rust
    /// ⚠⚠ Sizes that are **not** multiples of 4, so the trailing partial word is exercised. A size
    /// that divided evenly would not test the padding `words_for`'s `div_ceil(4)` provides, and a
    /// dropped trailing word is a plausible mask rather than an error.
    #[test]
    fn packing_a_coverage_map_is_little_endian_and_pads_the_last_word() {
        fn reference(m: &pipeline::brush::CoverageMap) -> Vec<u32> {
            let mut out = vec![0u32; (m.w * m.h).div_ceil(4)];
            for (k, v) in m.data.iter().enumerate() {
                out[k >> 2] |= (*v as u32) << ((k & 3) * 8);
            }
            out
        }
        for (w, h) in [(1usize, 1usize), (3, 3), (5, 7), (16, 16), (13, 9)] {
            let m = pipeline::brush::CoverageMap {
                x0: 0,
                y0: 0,
                w,
                h,
                data: (0..w * h).map(|i| (i * 37 % 256) as u8).collect(),
                outside: 0,
            };
            let got = GpuMaskStage::pack_maps(std::slice::from_ref(&std::sync::Arc::new(m.clone())));
            assert_eq!(got, reference(&m), "{w}x{h}");
            assert_eq!(got.len(), (w * h).div_ceil(4), "{w}x{h} word count");
        }
    }

    /// ⚠ Two maps land back to back, and the second must start on a whole-word boundary — the
    /// offset `comps` records. A short first map that ran into the second's words would give a
    /// composite mask reading its neighbour's coverage.
    #[test]
    fn two_maps_are_laid_down_without_overlapping() {
        let m = |w: usize, h: usize, fill: u8| {
            std::sync::Arc::new(pipeline::brush::CoverageMap {
                x0: 0,
                y0: 0,
                w,
                h,
                data: vec![fill; w * h],
                outside: 0,
            })
        };
        let a = m(3, 3, 0x11);
        let b = m(2, 2, 0x22);
        let got = GpuMaskStage::pack_maps(&[a.clone(), b.clone()]);
        assert_eq!(got.len(), 9usize.div_ceil(4) + 4usize.div_ceil(4));
        assert_eq!(got[0], 0x1111_1111);
        // The 9th byte is the last of `a`, alone in its word; `b` starts at the next word.
        assert_eq!(got[2], 0x0000_0011);
        assert_eq!(got[3], 0x2222_2222);
    }
```

- [ ] **Step 2: Expose a testable seam**

`pack` takes `&[Option<Arc<CoverageMap>>]` and is a method. Add an associated function beside it
that does the work on the flattened list, and have `pack` call it:

```rust
    /// The words for a list of maps, laid down back to back.
    ///
    /// ⚠ Separated from [`Self::pack`] so it can be tested without a GPU: `pack`'s input is
    /// `&[Option<..>]` shaped by the mask's components, and the packing itself has nothing to do
    /// with that shape.
    fn pack_maps(maps: &[std::sync::Arc<pipeline::brush::CoverageMap>]) -> Vec<u32> {
        let mut packed: Vec<u32> = Vec::new();
        for m in maps {
            let start = packed.len();
            packed.resize(start + words_for(m) as usize, 0);
            for (k, v) in m.data.iter().enumerate() {
                packed[start + (k >> 2)] |= (*v as u32) << ((k & 3) * 8);
            }
        }
        packed
    }

    fn pack(&self, maps: &[Option<std::sync::Arc<pipeline::brush::CoverageMap>>]) -> Vec<u32> {
        let owned: Vec<std::sync::Arc<pipeline::brush::CoverageMap>> =
            maps.iter().flatten().cloned().collect();
        let mut packed = Self::pack_maps(&owned);
        if packed.is_empty() {
            packed.push(0);
        }
        packed
    }
```

- [ ] **Step 3: Run — must PASS against the old algorithm**

`cargo test --release -q -p gpu-pipeline packing_a_coverage_map two_maps_are_laid 2>&1 | grep -E 'test result|error\['`
Expected: PASS. ⚠ The test is written against the behaviour being preserved, so it must be green
**before** the rewrite; that is what makes it a regression test rather than a new claim.

- [ ] **Step 4: Commit**

```bash
git add crates/gpu-pipeline/src/mask.rs
git commit -m "test(gpu): pin the coverage packing, padding and layout included"
```

---

### Task 2: Rewrite `pack_maps` as a chunked scan

**Files:** Modify `crates/gpu-pipeline/src/mask.rs`

- [ ] **Step 1: Replace the inner loop**

```rust
    fn pack_maps(maps: &[std::sync::Arc<pipeline::brush::CoverageMap>]) -> Vec<u32> {
        let total: usize = maps.iter().map(|m| words_for(m) as usize).sum();
        let mut packed: Vec<u32> = Vec::with_capacity(total);
        for m in maps {
            // ⚠⚠ **A chunked scan, not a byte-by-byte OR.** The old loop did a bounds-checked
            // read-modify-write of `packed[k >> 2]` for every sample — 10.6-13.8 ms per encode at
            // rung 3200, ~90% of the upload, measured 2026-09-16. The shifts it spelled out are
            // exactly little-endian order, so `from_le_bytes` over 4-byte chunks is the same
            // number written the short way and vectorises.
            let mut it = m.data.chunks_exact(4);
            packed.extend(it.by_ref().map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
            // ⚠ The trailing partial word is PADDED, never dropped: `words_for` rounds up with
            // `div_ceil(4)` and the shader reads whole words.
            let rest = it.remainder();
            if !rest.is_empty() {
                let mut last = [0u8; 4];
                last[..rest.len()].copy_from_slice(rest);
                packed.push(u32::from_le_bytes(last));
            }
        }
        packed
    }
```

- [ ] **Step 2: Run the pinning tests**

`cargo test --release -q -p gpu-pipeline packing_a_coverage_map two_maps_are_laid 2>&1 | grep -E 'test result|panicked'`
Expected: PASS, byte-identical to the reference.

- [ ] **Step 3: Commit**

```bash
git add crates/gpu-pipeline/src/mask.rs
git commit -m "perf(gpu): the coverage repack is a chunked scan, not a per-byte OR"
```

---

### Task 3: Narrow straight from the mapped range

**Files:** Modify `crates/gpu-pipeline/src/brush_raster.rs:435` and `:458`

- [ ] **Step 1: Drop the intermediate `Vec<u32>`**

Replace:

```rust
        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
```

with:

```rust
        // ⚠⚠ Narrowed straight from the mapped range. This used to `to_vec()` the whole readback
        // into a `Vec<u32>` — **27.35 MB at rung 3200** — whose only use was the narrowing a few
        // lines below. The mapped range is borrowed until `unmap`, so the narrowing has to happen
        // here rather than after it, which is the reason the old code copied at all.
        let data: Vec<u8> = slice
            .get_mapped_range()
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]).min(255) as u8)
            .collect();
```

- [ ] **Step 2: Delete the old narrowing** further down, replacing:

```rust
        // ⚠ Narrowed here rather than in the shader, because packing four samples into one word
        // would mean four threads read-modify-writing the same address — a race. See the shader.
        let data: Vec<u8> = words.iter().map(|&v| v.min(255) as u8).collect();
```

with:

```rust
        // ⚠ Coverage crosses the wire as one `u32` per sample, and that is DELIBERATE: packing
        // four samples into one word in the shader would mean four threads read-modify-writing the
        // same address — a race. See the shader. Narrowing it here is the cost of that choice;
        // ⚠⚠ it is a considered trade, not an oversight, and any attempt to shrink the wire needs
        // a restructured dispatch (one thread per four samples) or a second pack pass.
```

- [ ] **Step 3: Build and test**

```bash
cargo test --release -q -p gpu-pipeline 2>&1 | grep -E 'test result|error\[' | head -4
```
Expected: PASS. ⚠ If `words` is still referenced anywhere, the compiler says so — fix by reading
that use, not by reintroducing the copy.

- [ ] **Step 4: Commit**

```bash
git add crates/gpu-pipeline/src/brush_raster.rs
git commit -m "perf(gpu): narrow coverage straight from the mapped range"
```

---

### Task 4: Sabotage — prove the packing test has teeth

- [ ] **Step 1: Swap the byte order** in `pack_maps`: `[c[3], c[2], c[1], c[0]]`.
- [ ] **Step 2:** `cargo test --release -q -p gpu-pipeline packing_a_coverage_map 2>&1 | grep -E 'test result'` → **FAIL**.
- [ ] **Step 3: Drop the trailing word** instead — delete the `if !rest.is_empty()` block.
- [ ] **Step 4:** Run again → **FAIL** on the word count for `(1,1)`, `(3,3)`, `(5,7)`, `(13,9)`.
- [ ] **Step 5: Revert** with `git checkout crates/gpu-pipeline/src/mask.rs`, re-run → PASS.

⚠⚠ If either sabotage PASSES, stop and report: the test is not pinning the layout.

---

### Task 5: Re-measure

- [ ] **Step 1: Record the load**

```bash
uptime; ps -A -o %cpu,comm | sort -rn | head -3
```
⚠ `julia` and `obsidian-notes-rag` have each sat at ~100% during every measurement on this machine.

- [ ] **Step 2: Run the probe committed on 2026-09-16**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -q -p gpu-pipeline --example coverage_transfer_probe 2>&1 | tail -30
```

Report the new bands beside the old: upload **7.9–15.8 ms**, download **4.0–9.4 ms**, `pack`
**10.6–13.8 ms**. ⚠ Quote bands over ≥3 runs and state the load.

- [ ] **Step 3: Decide rayon on evidence.** If `pack` is still a large share after the chunked
rewrite, adding `rayon` to `gpu-pipeline` is the next step — **and only then**.

---

### Task 6: Gate, launch, record

- [ ] **Step 1:** `cargo fmt`, commit any change, **then** gate so the first run describes the tip:

```bash
cargo fmt --check >/dev/null 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1; echo "CLIPPY=$?"
cargo test --workspace >/tmp/crn_test.txt 2>&1; echo "TEST=$?"
awk '/Running tests\/golden.rs/{f=1} f&&/test result/{print "golden: " $0; exit}' /tmp/crn_test.txt
```
Then `git diff HEAD --stat` and `git status --porcelain`, both empty.

- [ ] **Step 2: Launch the GUI** and say what to look at: drag a mask's sliders with **Use the GPU**
on — **the picture must not change**, and a composite mask must still be correct.

- [ ] **Step 3: Record** in `CLAUDE.md` (overwrite in place) and `docs/JOURNAL.md`, including that
the wire was examined and deliberately left alone.

- [ ] **Step 4:** Do not merge, do not push.

## Self-Review

**Spec coverage.** Done-when 1 → Tasks 1–2. 2 → Task 3. 3 → Task 1. 4 → Task 4. 5 → Task 5.
6 → Task 6 step 1. 7 → Task 6 step 2, performed by the user.

**Placeholders.** None; every step carries its code or its exact command.

**Type consistency.** `pack_maps(&[Arc<CoverageMap>]) -> Vec<u32>` is introduced in Task 1 step 2
and rewritten in Task 2 with the same signature; `words_for` is unchanged; `CoverageMap`'s fields
(`x0`, `y0`, `w`, `h`, `data`, `outside`) match `pipeline/src/brush/raster.rs`.

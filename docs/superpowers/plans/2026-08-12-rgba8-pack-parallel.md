# A parallel 8-bit pack — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Take `render_engine::pack` from 4.6 ms to roughly 0.3 ms on a fit-resolution preview frame, with bit-identical output.

**Architecture:** `core-image` gains `ImageBuf::to_rgb8()` and `to_rgba8()`, both preallocate-and-fill and both rayon-parallel over pixels. `export::to_rgb8` keeps its name and signature and delegates. `render_engine::pack` calls `to_rgba8`, so `app` gains no `rayon` dependency. The two multipliers — dropping `Vec::push`, then threading — land in separate tasks so the bench can attribute the win rather than assume it.

**Tech Stack:** Rust, `rayon` (already a `core-image` dependency), `cargo test` / `cargo clippy --all-targets --all-features` / `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-12-rgba8-pack-parallel-design.md`

**Build setup — every `cargo` command below assumes this has been exported in the shell:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/core-image/src/lib.rs` | `quantize`, `to_rgb8`, `to_rgba8` on `ImageBuf` | Modify (add after `map_pixels_xy`, plus tests) |
| `crates/export/src/lib.rs` | `to_rgb8` delegates | Modify (`:15-21`) |
| `crates/app/src/render_engine.rs` | `pack` calls `to_rgba8` | Modify (`:147-162`) |
| `crates/app/examples/bench_preview.rs` | pack timing, fit and native | Modify (append a section) |
| `CLAUDE.md` | durable status | Modify (RESUME block + a feature section) |

No new files. `crates/pipeline/tests/golden.rs` is **deliberately untouched** — see §3 of the spec.

---

## Task 1: The conversions land in `core-image`, sequentially

Sequential first, on purpose: it isolates the `Vec::push` multiplier from the threading one, and it means the correctness tests are written against the simpler thing.

**Files:**
- Modify: `crates/core-image/src/lib.rs` (add after `map_pixels_xy`, before `crop`'s neighbours in the same `impl`)
- Test: `crates/core-image/src/lib.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/core-image/src/lib.rs`:

```rust
    /// An image with awkward samples: below 0, above 1, and the exact rounding boundaries.
    /// A quantizer that clamped wrongly, rounded wrongly or transposed channels would differ
    /// on at least one of these.
    fn awkward() -> ImageBuf {
        let mut img = ImageBuf::new(3, 2);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        img.set_pixel(2, 0, [1.0 / 512.0, 0.999, 0.001]);
        img.set_pixel(0, 1, [f32::NAN, -0.0, 2.0]);
        img.set_pixel(1, 1, [0.4980392, 0.5019608, 0.75]);
        img.set_pixel(2, 1, [0.125, 0.875, 0.33333334]);
        img
    }

    /// The reference: a plain sequential loop, written out here rather than shared with
    /// production so it can hold the old behaviour still while production changes.
    fn rgb8_reference(img: &ImageBuf) -> Vec<u8> {
        img.data()
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }

    fn rgba8_reference(img: &ImageBuf) -> Vec<u8> {
        let mut out = Vec::with_capacity(img.width() * img.height() * 4);
        for px in img.data().chunks_exact(ImageBuf::CHANNELS) {
            out.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push(255);
        }
        out
    }

    #[test]
    fn to_rgb8_matches_a_sequential_reference() {
        let img = awkward();
        assert_eq!(img.to_rgb8(), rgb8_reference(&img));
        assert_eq!(img.to_rgb8().len(), 3 * 2 * ImageBuf::CHANNELS);
    }

    #[test]
    fn to_rgba8_matches_a_sequential_reference() {
        let img = awkward();
        assert_eq!(img.to_rgba8(), rgba8_reference(&img));
        assert_eq!(img.to_rgba8().len(), 3 * 2 * 4);
    }

    #[test]
    fn to_rgba8_is_to_rgb8_with_an_opaque_alpha() {
        // Proves the two really do share one quantizer rather than having two that agree
        // today and drift tomorrow.
        let img = awkward();
        let rgb = img.to_rgb8();
        let rgba = img.to_rgba8();
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            assert_eq!(&px[..3], &rgb[i * 3..i * 3 + 3], "pixel {i} colour differs");
            assert_eq!(px[3], 255, "pixel {i} is not opaque");
        }
    }

    #[test]
    fn the_conversions_quantize_the_known_values() {
        // A spot check with the answers written out, so a reference loop that was wrong in
        // the same way as production could not hide it.
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        assert_eq!(img.to_rgb8(), vec![0, 128, 255, 0, 255, 64]);
        assert_eq!(img.to_rgba8(), vec![0, 128, 255, 255, 0, 255, 64, 255]);
    }

    #[test]
    fn the_conversions_handle_an_empty_image() {
        // Unlike `map_pixels_xy`, the chunk sizes here are non-zero constants, so there is no
        // `par_chunks_exact_mut(0)` panic to guard against. Pinned rather than trusted.
        let img = ImageBuf::new(0, 0);
        assert!(img.to_rgb8().is_empty());
        assert!(img.to_rgba8().is_empty());
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p core-image to_rgb8_matches 2>&1 | tail -10`

Expected: FAIL to compile — `no method named `to_rgb8` found for struct `ImageBuf``.

- [ ] **Step 3: Write the sequential implementation**

In `crates/core-image/src/lib.rs`, add inside `impl ImageBuf`, immediately after `map_pixels_xy`:

```rust
    /// Quantize one linear-display sample to 8 bits: clamp to `0..=1`, scale, round.
    ///
    /// Private and shared, so [`ImageBuf::to_rgb8`] and [`ImageBuf::to_rgba8`] cannot come to
    /// disagree about rounding.
    ///
    /// ⚠ A `NaN` sample ends up as 0, but **not** because of the clamp: `f32::clamp` returns
    /// `NaN` for a `NaN` input. It is the `as u8` cast that saturates it, Rust's float-to-int
    /// casts being saturating. 0 is the right answer for a sample that means nothing; the
    /// note is here because "clamp handles NaN" is the obvious wrong reading of this line.
    fn quantize(v: f32) -> u8 {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Quantize to 8-bit RGB — what an encoder wants.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.data.len()];
        for (dst, &src) in out.iter_mut().zip(self.data.iter()) {
            *dst = Self::quantize(src);
        }
        out
    }

    /// Quantize to 8-bit RGBA with an opaque alpha — the layout a GPU texture wants.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        for (dst, src) in out
            .chunks_exact_mut(4)
            .zip(self.data.chunks_exact(Self::CHANNELS))
        {
            dst[0] = Self::quantize(src[0]);
            dst[1] = Self::quantize(src[1]);
            dst[2] = Self::quantize(src[2]);
            dst[3] = 255;
        }
        out
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p core-image 2>&1 | tail -10`

Expected: PASS, all of them.

- [ ] **Step 5: Commit**

```bash
git add crates/core-image/src/lib.rs
git commit -m "feat(core-image): add to_rgb8 and to_rgba8"
```

---

## Task 2: The two production copies delegate

**Files:**
- Modify: `crates/export/src/lib.rs:15-21`
- Modify: `crates/app/src/render_engine.rs:147-162`

- [ ] **Step 1: Delegate `export::to_rgb8`**

In `crates/export/src/lib.rs`, replace:

```rust
pub fn to_rgb8(image: &ImageBuf) -> Vec<u8> {
    image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}
```

with:

```rust
/// Quantize to 8-bit RGB for encoding.
///
/// Kept as a free function with this name so no call site changes, but the quantization now
/// lives on `ImageBuf` — one implementation for the encoder and the preview canvas, which
/// cannot drift. `to_rgb8_quantizes_and_clamps` below is the guard that this delegation
/// preserved the behaviour exactly.
pub fn to_rgb8(image: &ImageBuf) -> Vec<u8> {
    image.to_rgb8()
}
```

- [ ] **Step 2: Point `pack` at `to_rgba8`**

In `crates/app/src/render_engine.rs`, replace the body of `pack`:

```rust
/// Pack a rendered sRGB image into an RGBA8 frame.
fn pack(out: ImageBuf, region: Option<Region>) -> RenderedFrame {
    let (width, height) = (out.width(), out.height());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in out.data().chunks_exact(3) {
        rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push(255);
    }
    RenderedFrame {
        width,
        height,
        rgba,
        region,
    }
}
```

with:

```rust
/// Pack a rendered sRGB image into an RGBA8 frame.
///
/// The quantization lives on `ImageBuf`, where the buffer and rayon already are — which is
/// also why `app` needs no `rayon` dependency of its own.
fn pack(out: ImageBuf, region: Option<Region>) -> RenderedFrame {
    RenderedFrame {
        width: out.width(),
        height: out.height(),
        rgba: out.to_rgba8(),
        region,
    }
}
```

- [ ] **Step 3: Run everything**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test --all 2>&1 | grep -c "test result: ok"`

Expected: no errors, no FAILED, and 21 `test result: ok` lines. The 29 goldens must be
byte-identical: `export::to_rgb8` is on the export path, so a changed quantization would move
them.

- [ ] **Step 4: Commit**

```bash
git add crates/export/src/lib.rs crates/app/src/render_engine.rs
git commit -m "refactor: one 8-bit quantization for the encoder and the canvas"
```

---

## Task 3: Measure the sequential win before threading

The point of a separate task: the spec predicts ~4.6 → ~1.2 ms from dropping `Vec::push` alone,
and that prediction should be **checked**, not folded into the parallel number and credited to
rayon.

**Files:**
- Modify: `crates/app/examples/bench_preview.rs` (append at the end of `main`)

- [ ] **Step 1: Add the pack benchmark**

Append at the end of `main` in `crates/app/examples/bench_preview.rs`:

```rust
    // The RGBA8 pack, at both render resolutions. It runs on every frame, including the ones
    // the prefix checkpoints made cheap, so it is measured on its own rather than buried in a
    // whole-frame number.
    {
        let long = buf.width().max(buf.height());
        for (label, max_dim) in [("fit", PREVIEW_MAX_DIM), ("native", long)] {
            let img = buf.downscale_to(max_dim);
            let mut best = f64::MAX;
            for _ in 0..9 {
                let t = Instant::now();
                let bytes = img.to_rgba8();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(&bytes);
                best = best.min(ms);
            }
            println!(
                "{best:8.2} ms   RGBA8 pack, {label} ({}x{})",
                img.width(),
                img.height()
            );
        }
    }
```

- [ ] **Step 2: Run it and record the sequential number**

Run: `cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF 2>&1 | tail -4`

Expected: two new lines. `fit` should be around **1.2 ms**, against the 4.59 ms the `Vec::push`
loop measured. Write the actual figures down — they go into the comment in Task 4 and into
`CLAUDE.md`.

If `fit` is not markedly below 4.59 ms, stop: the spec's central claim about `Vec::push` is
wrong and the design needs revisiting before threading is added on top of it.

- [ ] **Step 3: Commit**

```bash
git add crates/app/examples/bench_preview.rs
git commit -m "bench: time the RGBA8 pack at both render resolutions"
```

---

## Task 4: Parallelise

**Files:**
- Modify: `crates/core-image/src/lib.rs` (`to_rgb8`, `to_rgba8`)

- [ ] **Step 1: Switch both to rayon, in coarse blocks**

> **⚠ Corrected during execution.** This step first proposed `par_iter_mut()` and
> `par_chunks_exact_mut(4)` — one sample or one pixel per rayon task. Both measured **slower
> than sequential** (1.64 ms against 1.13 ms at 1600×1068), because per-item dispatch stops the
> inner work vectorising. Parallelise in coarse blocks with a sequential inner loop instead:
> that measured **0.46 ms**, and it also disproves the tempting reading that the operation is
> memory-bandwidth-bound (58.9 GB/s against 24.2).

In `crates/core-image/src/lib.rs`, add above `impl ImageBuf`:

```rust
/// Pixels per parallel task in the 8-bit conversions.
///
/// ⚠ **The granularity is the whole point, and it was measured.** Handing rayon one *pixel* per
/// task — `par_chunks_exact_mut(4)`, the obvious spelling — is **slower than not threading at
/// all**: 1.64 ms against 1.13 ms on a 1600×1068 frame, because the per-item dispatch stops the
/// inner work vectorising. A coarse block with a plain sequential loop inside gets both, and
/// runs in 0.46 ms. Anything from ~1024 up performs the same (52.8 / 58.9 / 61.3 GB/s at 1024 /
/// 8192 / 65536), so this sits in the middle of a flat region rather than on a tuned peak.
const PACK_BLOCK_PX: usize = 8192;
```

and replace the two loop bodies:

```rust
    /// Quantize to 8-bit RGB — what an encoder wants.
    ///
    /// Parallel in blocks; bit-identical to a sequential loop, since each output byte depends
    /// on exactly one input sample.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let block = PACK_BLOCK_PX * Self::CHANNELS;
        let mut out = vec![0u8; self.data.len()];
        out.par_chunks_mut(block)
            .zip(self.data.par_chunks(block))
            .for_each(|(ob, sb)| {
                for (dst, &src) in ob.iter_mut().zip(sb.iter()) {
                    *dst = Self::quantize(src);
                }
            });
        out
    }

    /// Quantize to 8-bit RGBA with an opaque alpha — the layout a GPU texture wants.
    ///
    /// Parallel in blocks, and bit-identical to a sequential loop for the same reason.
    ///
    /// ⚠ Unlike [`ImageBuf::map_pixels_xy`] there is **no zero-size guard**, and that is
    /// correct rather than an oversight: the chunk widths here are non-zero constants, so a
    /// zero-width image simply yields an empty `Vec`. `map_pixels_xy` chunks by *row*, which
    /// is 0 for a zero-width image, and `par_chunks_exact_mut(0)` panics. Using a fixed block
    /// rather than a row is what buys that.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        out.par_chunks_mut(PACK_BLOCK_PX * 4)
            .zip(self.data.par_chunks(PACK_BLOCK_PX * Self::CHANNELS))
            .for_each(|(ob, sb)| {
                for (dst, src) in ob.chunks_exact_mut(4).zip(sb.chunks_exact(Self::CHANNELS)) {
                    dst[0] = Self::quantize(src[0]);
                    dst[1] = Self::quantize(src[1]);
                    dst[2] = Self::quantize(src[2]);
                    dst[3] = 255;
                }
            });
        out
    }
```

`par_chunks_mut` rather than `par_chunks_exact_mut`, so a pixel count that is not a multiple of
the block still gets its remainder.

- [ ] **Step 2: Run the correctness tests**

Run: `cargo test -p core-image 2>&1 | tail -8 && cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

Expected: PASS. `to_rgb8_matches_a_sequential_reference` and
`to_rgba8_matches_a_sequential_reference` are the ones that matter — they compare against
sequential loops that did not change, which is the only way bit-identity gets proven here.

- [ ] **Step 3: Verify the tests can still fail**

Temporarily break the parallel version by changing `dst[3] = 255;` to `dst[3] = 254;`.

Run: `cargo test -p core-image to_rgba8_matches 2>&1 | grep -E "assertion|panicked" | head -3`

Expected: FAIL on the reference comparison. Revert, and re-run to confirm PASS. (Remember to
`touch crates/core-image/src/lib.rs` after reverting if you restore from a copy — `mv` preserves
mtime and cargo will reuse the broken artifact.)

- [ ] **Step 4: Measure**

Run: `cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF 2>&1 | tail -4`

Expected: `fit` around **0.3 ms**. Record the figure.

- [ ] **Step 5: Record all three numbers in the bench comment**

Extend the block comment above the pack section in `crates/app/examples/bench_preview.rs` with
the measured progression — `Vec::push` 4.59 ms, sequential fill, parallel — for fit and native,
so the attribution between the two multipliers is on the record rather than inferred.

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head; cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

Expected: clippy silent, no test failures, 29 goldens byte-identical.

```bash
git add crates/core-image/src/lib.rs crates/app/examples/bench_preview.rs
git commit -m "perf(core-image): parallelise the 8-bit quantization"
```

---

## Task 5: Verify in the GUI

- [ ] **Step 1: Launch**

Run: `cargo run --release -p app`

- [ ] **Step 2: Check by hand**

Open a RAW from `~/Desktop/fuji/`. The pack is on every frame, so the thing to
confirm is that **nothing looks different** — this change must be invisible except in the
timing. Specifically:

1. The image renders with correct colour and no banding, tearing or striping. A parallel fill
   that mis-chunked would show as horizontal bands.
2. Drag Exposure. Tail-only slider frames should feel at least as smooth as before.
3. Zoom to 1:1 and pan. The native pack is the 17.8 ms one, so this is where the gain is
   largest.
4. Export a JPEG and open it. `export::to_rgb8` now delegates, so this confirms the encoder
   path end to end.

- [ ] **Step 3: Fix or continue**

If anything looks wrong, reproduce it with a test before fixing.

---

## Task 6: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` (RESUME block, a new feature section, docs index, crate-layout line)

- [ ] **Step 1: Rewrite the RESUME next action**

Replace the `▶ NEXT ACTION: parallelise the RGBA8 pack` paragraph with a pointer to what is now
top of the list: **a background full-res decode for true 1:1** (item 2 in "What is left"),
carrying its existing note that `PrefixCache::MID_BUDGET_BYTES` wants revisiting because a
full-resolution checkpoint is ~312 MB.

- [ ] **Step 2: Add a feature section**

Add a section in the house style, above the per-stage prefix checkpoints one, covering:

- ⚠ **The measurement redirected the task.** It was named after parallelism, but `Vec::push`
  was costing 3.7× per output byte against `.map().collect()`; dropping it was the larger
  multiplier and threading was the second. Give all three measured numbers for fit and native.
- The quantization now lives once, on `ImageBuf`, with `export::to_rgb8` delegating — and
  ⚠ the golden harness keeping its own copy **on purpose**, because a golden that quantizes
  through production code cannot witness a change to production code.
- ⚠ Why there is no zero-size guard here when `map_pixels_xy` needs one: constant chunk widths
  versus a row width that is 0 for a zero-width image.
- `app` still has no `rayon` dependency, and why that was worth preserving.
- That bit-identity could not be proven by the existing tests, hence the sequential reference
  loops written inline in `core-image`'s tests.
- Accepted caveats from the spec.

- [ ] **Step 3: Update the crate-layout line and the docs index**

In the crate table, `core-image`'s line gains `to_rgb8/to_rgba8`. Add
`2026-08-12-rgba8-pack-parallel-design.md` to the specs list and
`2026-08-12-rgba8-pack-parallel.md` to the plans list.

- [ ] **Step 4: Final check**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head; cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the parallel 8-bit pack"
```

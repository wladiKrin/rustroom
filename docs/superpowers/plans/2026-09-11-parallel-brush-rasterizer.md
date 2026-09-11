# M8.3i — the parallel brush rasterizer: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **no `Co-Authored-By`
> trailer**. **Never push** — the user pushes to `github.com/wladiKrin/rustroom` himself. The
> branch `m8.3i-mask-loop` already exists and is checked out. Read every gate's exit code **from a
> file with a session-unique name, never through a pipe**. Restore a sabotage from a `cp` backup,
> **never `git checkout -- <file>`** (that has destroyed uncommitted work twice here). Read
> `git diff --stat` and `git diff | grep '^-'` **BEFORE** committing, not after.

**Goal:** Make the brush coverage rasteriser parallel, taking the catalogue's one real masked
photo from ~296 ms a frame to roughly half that, with output bit-identical to today's.

**Architecture:** `rasterize_in` has two sequential hot phases — an Oklab conversion of the box,
and a per-stroke scatter of dabs into a reused `layer` buffer. The first becomes a rayon row fill.
The second becomes a per-row **gather**: each row walks the dabs that touch it in stroke order,
keeps a running max in a row-local scratch, and composites straight into its own row of `cov`.
`layer` and its clear pass disappear.

**Tech Stack:** Rust, rayon (already a `pipeline` dependency), `cargo test --workspace`.

**Spec:** `docs/superpowers/specs/2026-09-11-parallel-brush-rasterizer-design.md`

**Build environment — export this before any cargo command:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File structure

| file | responsibility | change |
|---|---|---|
| `crates/pipeline/src/brush/raster.rs` | the rasteriser | the whole milestone lives here (~1,250 lines; already over the ~600 marker, **reported rather than split** — a split would move the tests away from what they guard mid-change) |
| `crates/pipeline/src/stages/mask.rs` | `MaskStage::apply` | four permanent phase timers only |
| `crates/app/examples/mask_breakdown.rs` | the decomposition probe | new file, kept |

---

### Task 1: Permanent phase instrumentation, and the probe

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`
- Modify: `crates/pipeline/src/brush/raster.rs`
- Create: `crates/app/examples/mask_breakdown.rs` (already present untracked — verify then commit)

⚠ This lands **first** because every measurement in every later task depends on it.

- [ ] **Step 1: Add the four timers to `MaskStage::apply`**

In `crates/pipeline/src/stages/mask.rs`, insert `let t_cov = std::time::Instant::now();`
immediately above the line beginning `let mut maps: Vec<Option<std::sync::Arc<crate::brush::CoverageMap>>> =`.

Insert `let d_cov = t_cov.elapsed();` immediately above the comment line
`        // ⚠ An **optimisation, not a correctness guard**`.

Replace this exact block:

```rust
        let mut adjusted = image.clone();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }
```

with:

```rust
        let t_clone = std::time::Instant::now();
        let mut adjusted = image.clone();
        let d_clone = t_clone.elapsed();
        let t_sub = std::time::Instant::now();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }
        let d_sub = t_sub.elapsed();
        let t_blend = std::time::Instant::now();
```

Replace `        image.map_pixels_xy(|x, y, base| {` with
`        let out = image.map_pixels_xy(|x, y, base| {`.

Replace this exact block (the end of `apply`):

```rust
                base[2] + (adj[2] - base[2]) * c,
            ]
        })
    }
}
```

with:

```rust
                base[2] + (adj[2] - base[2]) * c,
            ]
        });
        let d_blend = t_blend.elapsed();
        // ⚠⚠ Kept permanently, the `RUSTROOM_DEBUG_BRUSH` / `RUSTROOM_DEBUG_RENDER` precedent.
        // This split is what reframed M8.3i: it showed that a mask has NO dominant phase — HSL
        // is 84% sub-stages, a colour range 91% blend, an auto-masked brush 97% coverage — so
        // "port the mask loop" is not one thing. Rebuilding it by hand is the alternative.
        if std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
            eprintln!(
                "MASK {}x{} stages={} cov={:.2} clone={:.2} sub={:.2} blend={:.2} total={:.2}",
                w,
                h,
                self.stages.len(),
                d_cov.as_secs_f64() * 1e3,
                d_clone.as_secs_f64() * 1e3,
                d_sub.as_secs_f64() * 1e3,
                d_blend.as_secs_f64() * 1e3,
                (d_cov + d_clone + d_sub + d_blend).as_secs_f64() * 1e3,
            );
        }
        out
    }
}
```

- [ ] **Step 2: Add the two timers to `rasterize_in`**

In `crates/pipeline/src/brush/raster.rs`, insert `let t_lab = std::time::Instant::now();`
immediately above `    let base_lab: Option<Vec<[f32; 3]>> = match base {`.

Insert this immediately above `    let mut cov = vec![0.0f32; w * h];`:

```rust
    let d_lab = t_lab.elapsed();
    let t_dabs = std::time::Instant::now();
```

Then, immediately above the final `    CoverageMap {` of `rasterize_in` (the one that returns the
map — it is preceded by the `let data = cov.iter().map(..).collect();` block, **not** the early
returns and **not** any `CoverageMap` inside the test module), insert:

```rust
    let d_dabs = t_dabs.elapsed();
    // ⚠ Kept permanently. The split it reports — Oklab conversion against dab compositing — is
    // what decides whether a future change should attack the gate or the splat.
    if std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
        eprintln!(
            "RASTER box={}x{} strokes={} dabs={} lab={:.2} dabs_ms={:.2}",
            w,
            h,
            strokes.len(),
            strokes.iter().map(|s| s.dabs.len()).sum::<usize>(),
            d_lab.as_secs_f64() * 1e3,
            d_dabs.as_secs_f64() * 1e3,
        );
    }
```

- [ ] **Step 3: Verify the edits landed where they were aimed**

⚠ The recorded trap: a scripted edit that matches nothing exits 0, and one that matches too much
cuts in the wrong place.

Run:

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
```

Expected: exactly two files changed; the **only** removed lines are the four that were replaced
(`let mut adjusted = image.clone();`, the `for stage` line and its two braces / `image.map_pixels_xy(|x, y, base| {` / `        })` / `    }` / `}` as spliced above). If any other line was removed, restore from git and redo the edit by exact literal text.

- [ ] **Step 4: Build and check the probe exists**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
ls crates/app/examples/mask_breakdown.rs
cargo build --release -p app --example mask_breakdown > /tmp/rr-m83i-t1-$$.log 2>&1; echo "BUILD=$?"
```

Expected: `BUILD=0` and the file listed. If the probe is missing, it is in the working tree from
the brainstorm — recreate it from the spec's §1 table requirements (it renders `neutral`,
`1 masked exposure (gradient)`, `1 masked HSL (gradient)`, `3 masked HSL (gradient)`,
`1 masked exposure (small radial)`, `1 masked exposure (colour range)`,
`1 masked exposure (brush) uncached`, `1 masked exposure (auto brush)` at rung 3200, plus a real
sidecar row read from `<raw>.json`).

- [ ] **Step 5: Record the baseline**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_MASK=1 ./target/release/examples/mask_breakdown ~/Pictures/fuji/DSCF6274.RAF \
  > /tmp/rr-m83i-base-$$.txt 2> /tmp/rr-m83i-baseerr-$$.txt
echo "EXIT=$?"; cat /tmp/rr-m83i-base-$$.txt
grep RASTER /tmp/rr-m83i-baseerr-$$.txt | tail -5
```

Expected, within noise: `REAL SIDECAR` around **296 ms** against ~61 ms with its masks removed,
and `RASTER box=1510x2019 strokes=36 dabs=3645 lab≈55 dabs_ms≈165`. **Write these numbers into
the task's report** — every later task is judged against them.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/mask.rs crates/pipeline/src/brush/raster.rs crates/app/examples/mask_breakdown.rs
git commit -m "probe: RUSTROOM_DEBUG_MASK decomposes a mask into coverage, clone, sub-stages and blend"
```

---

### Task 2: The bit-identity reference test

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs` (test module)

⚠⚠ This test is written **against the current sequential code and must pass immediately**. That is
what validates the reference before it is used to guard a change — the rule `downscale_to`'s and
the 8-bit pack's parallelisations each followed.

- [ ] **Step 1: Write the fixture and the test**

⚠ This step does not compile on its own — it calls `rasterize_in_reference`, which Step 2 creates.
The two land in one commit.

Append to the `#[cfg(test)] mod tests` block in `crates/pipeline/src/brush/raster.rs`:

```rust
    /// A base whose left half and right half differ in Oklab, so an auto-mask gate genuinely
    /// discriminates rather than passing everything.
    struct HalfBase;
    impl BasePixels for HalfBase {
        fn at(&self, x: usize, _y: usize) -> [f32; 3] {
            if x < 40 {
                [0.05, 0.20, 0.05]
            } else {
                [0.60, 0.15, 0.50]
            }
        }
    }

    /// The fixture the bit-identity test rasterises.
    ///
    /// ⚠⚠ Every property here is load-bearing and a weaker fixture cannot see the bug it guards:
    /// **overlapping dabs** (so the running max and its short-circuit matter), an **erase**
    /// stroke (so `MaskOp::Subtract` and stroke order matter), an **auto-masked** stroke crossing
    /// the base's split (so the gate is reached and differs per dab), a **one-row** stroke (the
    /// degenerate chunk), and flow below 100 (so the cross-stroke composite is not saturating).
    fn gather_fixture() -> Brush {
        let along = |y: f32, x0: f32, x1: f32, n: usize| -> Vec<[f32; 2]> {
            (0..n)
                .map(|i| [x0 + (x1 - x0) * i as f32 / (n - 1) as f32, y])
                .collect()
        };
        Brush {
            strokes: vec![
                BrushStroke {
                    points: along(0.35, 0.15, 0.85, 12),
                    radius: 0.09,
                    hardness: 40.0,
                    flow: 70.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.42, 0.20, 0.80, 9),
                    radius: 0.07,
                    hardness: 65.0,
                    flow: 55.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.38, 0.30, 0.70, 7),
                    radius: 0.05,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: true,
                    ..Default::default()
                },
                BrushStroke {
                    points: along(0.60, 0.10, 0.90, 14),
                    radius: 0.08,
                    hardness: 50.0,
                    flow: 80.0,
                    auto_mask: true,
                    tolerance: 30.0,
                    ..Default::default()
                },
                BrushStroke {
                    points: vec![[0.50, 0.90]],
                    radius: 0.006,
                    hardness: 100.0,
                    flow: 100.0,
                    ..Default::default()
                },
            ],
            invert: false,
        }
    }

    #[test]
    fn the_rasterizer_is_bit_identical_to_its_reference() {
        // ⚠⚠ **`rasterize_in_reference` is a VERBATIM COPY of today's `rasterize_in`, living in
        // the test module and deliberately NOT shared with production.** A "reference" that
        // delegated to the function under test would compare it against itself and pass forever —
        // the vacuous shape this project records more often than any other. It is the rule
        // `downscale_to`'s and the 8-bit pack's parallelisations each followed.
        let b = gather_fixture();
        let g = Geometry::default();
        let want = rasterize_in_reference(&b, &g, 80, 60, None, Some(&HalfBase));
        let got = rasterize_in(&b, &g, 80, 60, None, Some(&HalfBase));
        let read = |m: &CoverageMap| -> Vec<u8> {
            (0..60)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .map(|(x, y)| m.at(x, y))
                .collect()
        };
        let (want, got) = (read(&want), read(&got));
        assert!(
            got.iter().any(|&v| v > 0) && got.iter().any(|&v| v == 0),
            "fixture check: the brush must both cover and not cover, or this test is vacuous"
        );
        assert!(
            got.iter().any(|&v| v > 0 && v < 255),
            "fixture check: some coverage must be partial, or the gate and the max are untested"
        );
        assert_eq!(want, got, "the rasterizer must be bit-identical to its reference");
    }
```

- [ ] **Step 2: Create the reference as a verbatim copy**

⚠ Do this by copying, not by retyping — the reference's value is that it is *exactly* today's
code.

1. Copy the whole of `pub fn rasterize_in(..) -> CoverageMap { .. }` into the `#[cfg(test)] mod
   tests` block.
2. Rename the copy to `rasterize_in_reference` and drop its `pub`.
3. Delete the two `RUSTROOM_DEBUG_MASK` timer blocks and their `let t_lab` / `let d_lab` /
   `let t_dabs` / `let d_dabs` bindings from the copy — instrumentation is not behaviour, and
   leaving it would print twice per call.
4. Put this doc comment on it:

```rust
    /// A verbatim copy of `rasterize_in` as it stood before M8.3i parallelised it: sequential
    /// Oklab conversion, and a per-stroke three-pass scatter into a reused `layer`.
    ///
    /// ⚠⚠ **Deliberately duplicated rather than shared.** Its whole value is being an
    /// independent witness: a reference that called `rasterize_in` could not see a change to
    /// `rasterize_in`. It is frozen — when the shipped function changes, this one must NOT be
    /// updated to match; if the two disagree, that is the finding.
    ///
    /// ⚠ It will drift out of resemblance over time, which is expected and is the cost of the
    /// guarantee. Delete it only when the parallel implementation has been shipped long enough
    /// that the goldens are the accepted oracle.
    #[allow(clippy::too_many_arguments)]


- [ ] **Step 3: Run it and watch it pass against the unchanged code**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p pipeline the_rasterizer_is_bit_identical_to_its_reference > /tmp/rr-m83i-t2-$$.log 2>&1
echo "TEST=$?"; grep -E "test result|passed" /tmp/rr-m83i-t2-$$.log
```

Expected: `TEST=0` and **`1 passed`**. ⚠ Read the passed-count, not the word `ok` — a filter that
matches nothing prints `0 passed; N filtered out` and reads as green.

- [ ] **Step 4: Sabotage to prove it discriminates**

Copy first: `cp crates/pipeline/src/brush/raster.rs /tmp/rr-m83i-raster-t2.bak`

In `rasterize_in_reference`, change its dab loop's `*slot = slot.max(a);` to `*slot = a;` (a wrong
combine that a weak fixture would not notice). Re-run Step 3. Expected: **`1 failed`** naming
`the_rasterizer_is_bit_identical_to_its_reference`.

⚠ If it passes, the fixture has no overlapping dabs where the later one loses — strengthen
`gather_fixture` and repeat before continuing, because that is exactly the property Task 4's
gather must preserve.

Restore: `cp /tmp/rr-m83i-raster-t2.bak crates/pipeline/src/brush/raster.rs` and confirm
`git diff --stat` shows only the intended addition.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/brush/raster.rs
git commit -m "test: a frozen sequential copy pins the rasterizer's output bit for bit"
```

---

### Task 3: The Oklab conversion goes parallel

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs`
- Modify: `crates/pipeline/src/stages/mask.rs` (only if the compiler asks)

- [ ] **Step 1: Make `BasePixels` shareable across threads**

Replace:

```rust
pub trait BasePixels {
    fn at(&self, x: usize, y: usize) -> [f32; 3];
}
```

with:

```rust
/// ⚠ `Sync` is a supertrait because `rasterize_in` reads the base from several rayon threads at
/// once when it builds `base_lab`. Every implementor here already holds only shared references,
/// so this costs nothing; the compiler names any future one that does not.
pub trait BasePixels: Sync {
    fn at(&self, x: usize, y: usize) -> [f32; 3];
}
```

- [ ] **Step 2: Replace the sequential `base_lab` build**

Replace this exact block:

```rust
            let mut v = Vec::with_capacity(w * h);
            for y in y0..y1 {
                for x in x0..x1 {
                    v.push(color::oklab::prophoto_to_oklab(b.at(x, y)));
                }
            }
            Some(v)
```

with:

```rust
            // ⚠ Rayon over rows, the shape `map_pixels_xy` and the 8-bit pack already use. Each
            // element is a pure function of one base pixel and the order is unchanged, so this is
            // bit-identical by construction rather than by tolerance.
            let mut v = vec![[0.0f32; 3]; w * h];
            v.par_chunks_exact_mut(w)
                .enumerate()
                .for_each(|(row, out)| {
                    let y = y0 + row;
                    for (i, slot) in out.iter_mut().enumerate() {
                        *slot = color::oklab::prophoto_to_oklab(b.at(x0 + i, y));
                    }
                });
            Some(v)
```

- [ ] **Step 3: Add the rayon import**

At the top of `crates/pipeline/src/brush/raster.rs`, beside the other `use` lines, add:

```rust
use rayon::prelude::*;
```

- [ ] **Step 4: Build, then run the whole pipeline suite**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p pipeline > /tmp/rr-m83i-t3-$$.log 2>&1; echo "TEST=$?"
grep -E "^test result" /tmp/rr-m83i-t3-$$.log
```

Expected: `TEST=0`, every `test result: ok`, **and the goldens among them**. If `BasePixels: Sync`
produced an error in `mask.rs`, `mask_tint.rs` or `bench_auto_mask_cov.rs`, the named type holds
something non-`Sync` — report it rather than working around it, because that would mean a base
really is not shareable.

- [ ] **Step 5: Measure the `lab` phase**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app --example mask_breakdown 2>&1 | tail -2
RUSTROOM_DEBUG_MASK=1 ./target/release/examples/mask_breakdown ~/Pictures/fuji/DSCF6274.RAF \
  2>&1 >/dev/null | grep RASTER | tail -5
```

Expected: `lab` falls from ~55 ms toward single digits; `dabs_ms` is unchanged at ~165. **Record
both numbers.**

- [ ] **Step 6: Commit**

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
git add crates/pipeline/src/brush/raster.rs
git commit -m "perf: the brush gate's Oklab conversion runs across rows"
```

---

### Task 4: The dab loop becomes a per-row gather

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs`

⚠⚠ This is the substantive change. Read §4.4 of the spec before starting: **the short-circuit
before the gate is load-bearing**, and a gather that evaluates the gate for every dab touching a
pixel is correct and dramatically slower.

- [ ] **Step 1: Replace the three-pass stroke body with one row-parallel pass**

Replace everything from `    let mut cov = vec![0.0f32; w * h];` down to and including the closing
brace of the `for stroke in &strokes {` loop — that is, the whole of the `let mut layer`
declaration, the per-stroke sub-rect computation, the clear loop, the dab loop and the composite
loop — with:

```rust
    let mut cov = vec![0.0f32; w * h];
    // ⚠ Rows per rayon task. Swept in Task 6; see its doc note below for the numbers.
    const CHUNK_ROWS: usize = 16;
    // ⚠ Below this many pixels a stroke stays sequential: the canvas tint rasterises on every
    // drag frame and every test uses a tiny fixture, where rayon's dispatch would dominate. The
    // 8-bit pack's lesson — the obvious parallel spelling measured slower than not threading.
    const SEQUENTIAL_FLOOR_PX: usize = 64 * 1024;
    for stroke in &strokes {
        if stroke.dabs.is_empty() || stroke.flow <= 0.0 {
            continue;
        }
        // Only the sub-rect this stroke touches is visited.
        let (mut slx, mut sly, mut shx, mut shy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for d in &stroke.dabs {
            slx = slx.min(d.x - d.r);
            sly = sly.min(d.y - d.r);
            shx = shx.max(d.x + d.r);
            shy = shy.max(d.y + d.r);
        }
        let sx0 = (slx.floor().clamp(x0 as f32, x1 as f32) as usize).max(x0);
        let sy0 = (sly.floor().clamp(y0 as f32, y1 as f32) as usize).max(y0);
        let sx1 = (((shx.ceil().clamp(0.0, x1 as f32) as usize) + 1).min(x1)).max(sx0);
        let sy1 = (((shy.ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1)).max(sy0);
        if sx1 <= sx0 || sy1 <= sy0 {
            continue;
        }
        let sw = sx1 - sx0;
        let op = if stroke.erase {
            edit_model::MaskOp::Subtract
        } else {
            edit_model::MaskOp::Add
        };
        // ⚠⚠ **One row of this stroke, as a GATHER rather than a scatter.** Today's code splats
        // each dab into a shared `layer`, which cannot be parallelised because dabs overlap.
        // Walking the dabs *per row* inverts that: the row owns its own scratch and its own row
        // of `cov`, so rows are independent.
        //
        // ⚠⚠ **Dabs are visited in stroke order and the short-circuit is kept.** The gate can
        // only ever reduce an alpha, so a dab whose positional alpha already loses the running
        // max cannot change the pixel however its gate resolves — which is what makes evaluating
        // the gate for most dabs unnecessary, and dabs overlap heavily (`SPACING` is a quarter
        // radius) so most of them do lose. Visiting in the original order makes the sequence of
        // comparisons identical, which is what makes the result bit-identical as well as fast.
        let row_body = |y: usize, cov_row: &mut [f32], scratch: &mut [f32]| {
            for v in scratch.iter_mut() {
                *v = 0.0;
            }
            for d in &stroke.dabs {
                let dy0 = (d.y - d.r).floor().clamp(sy0 as f32, sy1 as f32) as usize;
                let dy1 = (((d.y + d.r).ceil().clamp(0.0, sy1 as f32) as usize) + 1).min(sy1);
                if y < dy0 || y >= dy1 {
                    continue;
                }
                let dx0 = (d.x - d.r).floor().clamp(sx0 as f32, sx1 as f32) as usize;
                let dx1 = (((d.x + d.r).ceil().clamp(0.0, sx1 as f32) as usize) + 1).min(sx1);
                for x in dx0..dx1 {
                    let dist = (((x as f32 + 0.5) - d.x).powi(2)
                        + ((y as f32 + 0.5) - d.y).powi(2))
                    .sqrt();
                    let a = dab_alpha(dist, d.r, d.hard);
                    let slot = &mut scratch[x - sx0];
                    if a <= *slot {
                        continue;
                    }
                    let a = match (&d.gate, &base_lab) {
                        (Some((r, ceil)), Some(lab)) => {
                            a * auto_mask_gate(lab[(y - y0) * w + (x - x0)], *r, *ceil)
                        }
                        _ => a,
                    };
                    *slot = slot.max(a);
                }
            }
            for x in sx0..sx1 {
                let a = scratch[x - sx0] * stroke.flow;
                if a <= 0.0 {
                    continue;
                }
                let idx = x - x0;
                cov_row[idx] = op.combine(cov_row[idx], a);
            }
        };
        let r0 = sy0 - y0;
        let r1 = sy1 - y0;
        let rows = &mut cov[r0 * w..r1 * w];
        if (sy1 - sy0) * sw < SEQUENTIAL_FLOOR_PX {
            let mut scratch = vec![0.0f32; sw];
            for (ri, cov_row) in rows.chunks_mut(w).enumerate() {
                row_body(sy0 + ri, cov_row, &mut scratch);
            }
        } else {
            rows.par_chunks_mut(w * CHUNK_ROWS)
                .enumerate()
                .for_each(|(ci, chunk)| {
                    let mut scratch = vec![0.0f32; sw];
                    for (ri, cov_row) in chunk.chunks_mut(w).enumerate() {
                        row_body(sy0 + ci * CHUNK_ROWS + ri, cov_row, &mut scratch);
                    }
                });
        }
    }
```

- [ ] **Step 2: Build and run the bit-identity test**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p pipeline > /tmp/rr-m83i-t4-$$.log 2>&1; echo "TEST=$?"
grep -E "^test result" /tmp/rr-m83i-t4-$$.log
```

Expected: `TEST=0`, every `test result: ok`, **including all 63 goldens and every pre-existing
`brush::raster` test with its assertions unchanged** — that is the acceptance criterion, not
"still green".

⚠ If `the_rasterizer_is_bit_identical_to_its_reference` fails, do **not** relax it and do **not**
edit `rasterize_in_reference` to match. That copy is frozen and is the shipped behaviour. The likely causes, in order: dabs visited out of stroke order; the
short-circuit moved after the gate; `scratch` not cleared per row; `cov_row` indexed by `x - sx0`
instead of `x - x0`.

- [ ] **Step 3: Sabotage — drop a row's dabs**

⚠⚠ **CORRECTED DURING EXECUTION (Task 2).** The plan first called for reversing the dab order and
predicted a failure. **Reversing is provably INERT**: within a stroke the combine is `max`, and
the short-circuit only ever drops a dab whose gated alpha is already ≤ the running max, so the
result is exactly `max_d gated_alpha(d)` — order-independent. `*slot = a` in place of
`*slot = slot.max(a)` is inert for the same reason (that line is reached only when `a > *slot`).
**Both were tried against the reference in Task 2 and both passed.** Stroke order still matters
for the *skip's effectiveness*, i.e. for speed, and not for the value.

```bash
cp crates/pipeline/src/brush/raster.rs /tmp/rr-m83i-raster-t4.bak
```

Change `row_body`'s row test `if y < dy0 || y >= dy1 { continue; }` to
`if y < dy0 || y >= dy1.saturating_sub(1) { continue; }` — a row genuinely dropped from each dab.
Re-run Step 2. Expected: **`the_rasterizer_is_bit_identical_to_its_reference` fails.**

⚠ If it passes, `gather_fixture`'s strokes are thinner than two rows and the test cannot see a
row boundary — strengthen it and repeat before continuing.

Restore:

```bash
cp /tmp/rr-m83i-raster-t4.bak crates/pipeline/src/brush/raster.rs
git diff --stat
```

- [ ] **Step 4: Sabotage — evaluate the gate before the short-circuit**

```bash
cp crates/pipeline/src/brush/raster.rs /tmp/rr-m83i-raster-t4b.bak
```

Move the `let a = match (&d.gate, &base_lab) { .. };` block **above** the
`if a <= *slot { continue; }`. Re-run Step 2.

⚠⚠ Expected: **this passes**, and that is the finding rather than a failure. Gating before the
skip is *correct* — it is only slower. Record it as an inert sabotage rather than counting it, and
confirm the cost instead by measuring `dabs_ms` with it in place (Step 5 run twice). Restore
afterwards.

- [ ] **Step 5: Measure**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app --example mask_breakdown 2>&1 | tail -2
RUSTROOM_DEBUG_MASK=1 ./target/release/examples/mask_breakdown ~/Pictures/fuji/DSCF6274.RAF \
  > /tmp/rr-m83i-t4-out-$$.txt 2> /tmp/rr-m83i-t4-err-$$.txt
tail -4 /tmp/rr-m83i-t4-out-$$.txt
grep RASTER /tmp/rr-m83i-t4-err-$$.txt | tail -5
```

Expected: `dabs_ms` falls substantially from ~165 ms and `REAL SIDECAR` falls from ~296 ms.
**Record the numbers; do not assume a multiple.**

- [ ] **Step 6: Commit**

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
git add crates/pipeline/src/brush/raster.rs
git commit -m "perf: the dab loop is a per-row gather, parallel across rows"
```

---

### Task 5: Convert only the gating strokes' box

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs`

- [ ] **Step 1: Narrow the conversion to the gating strokes**

Replace the whole `let base_lab: Option<Vec<[f32; 3]>> = match base { .. };` expression with:

```rust
    // ⚠ Only the pixels a gating dab can read are converted. `base_lab` is still indexed over the
    // WHOLE box — the rows outside the gating span are simply never written and never read — so
    // the `(y - y0) * w + (x - x0)` arithmetic in the dab loop is unchanged.
    //
    // ⚠ Measured rather than assumed: fourteen of the real brush's thirty-six strokes are not
    // auto-masked, and their pixels are never read. If the gating union ever equals the whole box
    // this buys nothing and costs a few lines, which is the trade taken.
    let gating: Vec<&Stroke> = strokes
        .iter()
        .filter(|s| s.dabs.iter().any(|d| d.gate.is_some()))
        .collect();
    let base_lab: Option<Vec<[f32; 3]>> = match base {
        Some(b) if !gating.is_empty() => {
            let (mut gly, mut ghy) = (f32::MAX, f32::MIN);
            for d in gating.iter().flat_map(|s| s.dabs.iter()) {
                gly = gly.min(d.y - d.r);
                ghy = ghy.max(d.y + d.r);
            }
            let gy0 = (gly.floor().clamp(y0 as f32, y1 as f32) as usize).max(y0);
            let gy1 = (((ghy.ceil().clamp(0.0, y1 as f32) as usize) + 1).min(y1)).max(gy0);
            let mut v = vec![[0.0f32; 3]; w * h];
            if gy1 > gy0 {
                v[(gy0 - y0) * w..(gy1 - y0) * w]
                    .par_chunks_exact_mut(w)
                    .enumerate()
                    .for_each(|(row, out)| {
                        let y = gy0 + row;
                        for (i, slot) in out.iter_mut().enumerate() {
                            *slot = color::oklab::prophoto_to_oklab(b.at(x0 + i, y));
                        }
                    });
            }
            Some(v)
        }
        _ => None,
    };
```

⚠ Rows only, not columns: a per-row `x` span would make the fill ragged while the *index* stays
whole-box, which is more arithmetic for a smaller saving.

- [ ] **Step 2: Run the suite**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p pipeline > /tmp/rr-m83i-t5-$$.log 2>&1; echo "TEST=$?"
grep -E "^test result" /tmp/rr-m83i-t5-$$.log
```

Expected: `TEST=0` and the bit-identity test still passing — a gating dab must never read an
unwritten `[0,0,0]`, and it cannot, because every gating dab lies inside the gating span.

- [ ] **Step 3: Sabotage — narrow the span by one row at each end**

```bash
cp crates/pipeline/src/brush/raster.rs /tmp/rr-m83i-raster-t5.bak
```

Change `let gy0 = ...` to add `+ 1` and `let gy1 = ...` to subtract 1 (guarding against
underflow with `.max(gy0)`). Re-run Step 2. Expected:
**`the_rasterizer_is_bit_identical_to_its_reference` fails** — a gating dab now reads an
unconverted row. Restore from the backup.

- [ ] **Step 4: Measure whether it was worth anything**

Re-run Task 4 Step 5 and compare `lab`. **Report the number honestly even if it is zero** — the
gating union may be nearly the whole box on this sidecar.

- [ ] **Step 5: Commit**

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
git add crates/pipeline/src/brush/raster.rs
git commit -m "perf: only the gating strokes' rows are converted to Oklab"
```

---

### Task 6: Sweep the chunk size and the sequential floor

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs` (the two constants and their doc comments)

⚠ `PACK_BLOCK_PX`'s precedent: per-item rayon measured 1.64 ms against 1.13 ms sequential. Both
constants are **measured**, and the sweep goes into their doc comments so nobody re-derives it.

- [ ] **Step 1: Sweep `CHUNK_ROWS`**

For each of `1, 4, 16, 64, 256`: set `CHUNK_ROWS`, rebuild the probe, run it on
`~/Pictures/fuji/DSCF6274.RAF` three times, and record the median `dabs_ms`:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app --example mask_breakdown 2>&1 | tail -1
for i in 1 2 3; do
  RUSTROOM_DEBUG_MASK=1 ./target/release/examples/mask_breakdown ~/Pictures/fuji/DSCF6274.RAF \
    2>&1 >/dev/null | grep RASTER | tail -1
done
```

Pick a value **mid-plateau**, not on a peak. Write the table into the constant's doc comment.

- [ ] **Step 2: Sweep `SEQUENTIAL_FLOOR_PX`**

The floor protects small rasterisations. Time the whole `pipeline` test suite at
`0`, `16 * 1024`, `64 * 1024`, `256 * 1024`:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
time cargo test -p pipeline > /tmp/rr-m83i-t6-$$.log 2>&1; echo "TEST=$?"
```

and check the real sidecar's `dabs_ms` is unaffected at the chosen value (its strokes are far
above any of these). Pick the smallest value whose suite time is at the plateau, and record the
table in the doc comment.

- [ ] **Step 3: Full gate**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83i-gate-$$
cargo fmt --all -- --check > $L-fmt.log 2>&1; echo "FMT=$?" > $L-codes.txt
cargo clippy --all-targets --all-features -- -D warnings > $L-clippy.log 2>&1; echo "CLIPPY=$?" >> $L-codes.txt
cargo test --workspace > $L-test.log 2>&1; echo "TEST=$?" >> $L-codes.txt
cat $L-codes.txt
echo "--- passed:"; grep -E "^test result" $L-test.log | awk '{s+=$4} END {print s}'
echo "--- goldens ran:"; grep -c "^test golden_" $L-test.log
echo "--- failures:"; grep -c "^test result: FAILED" $L-test.log
```

⚠ Each exit code is read **from a file**, never through a pipe — a `cargo clippy … | grep`
pipeline reports grep's status and has let a failing clippy into a commit here before.

Expected: `FMT=0 CLIPPY=0 TEST=0`, **63 goldens ran**, **0 failures**, and the total passed count
is 2321 + 1 (the bit-identity test) = **2322**. ⚠ Read the count from the gate output rather than
computing it — the last milestone recorded a wrong number by trusting the arithmetic.

- [ ] **Step 4: Commit**

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
git add crates/pipeline/src/brush/raster.rs
git commit -m "perf: the row chunk and the sequential floor are swept rather than chosen"
```

---

### Task 7: Measure, launch the GUI, and record

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: The before/after on the real sidecar**

Run the probe three times on a quiet machine and take the median:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
for i in 1 2 3; do
  RUSTROOM_DEBUG_MASK=1 ./target/release/examples/mask_breakdown ~/Pictures/fuji/DSCF6274.RAF \
    > /tmp/rr-m83i-final-$i.txt 2> /tmp/rr-m83i-finalerr-$i.txt
  tail -3 /tmp/rr-m83i-final-$i.txt
  grep RASTER /tmp/rr-m83i-finalerr-$i.txt | tail -1
done
top -l 1 -n 0 | head -4
```

⚠ Record the machine's idle share alongside the numbers. Two earlier milestones here quoted
ratios taken on a busy machine and had to caveat them afterwards.

- [ ] **Step 2: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app > /tmp/rr-gui-m83i.log 2>&1 &
```

Say what to look at: **open `DSCF6274.RAF`, the one photo in `~/Pictures/fuji` with a mask**, and
**drag a global slider** (exposure at the top of the Edit panel) — that is the case the coverage
cache cannot help and the one this milestone targets. It should be visibly quicker, and
⚠⚠ **the picture must not change**: a coverage bug would show as the masked area moving, growing
or acquiring a hard edge where the brush was soft.

- [ ] **Step 3: Write the `CLAUDE.md` record**

Add an `M8.3i — the parallel brush rasterizer (2026-09-11)` section immediately above
`**M8.3h — the cost gate re-tuned**`, carrying:

- the decomposition table from the spec's §1, **with the correction that this catalogue's one real
  mask carries only `exposure: -3.6`** — every other field is neutral, which `CLAUDE.md` currently
  states wrongly. Correct the M8.3h NEXT ACTION block's claim in place.
- the finding that **no phase dominates** — it depends entirely on the mask kind.
- the before/after numbers from Step 1, with the machine's idle share.
- the short-circuit finding from §4.4, because it is the thing a future rewrite would break.
- the sabotage results, **including the inert one** (gating before the skip is correct and slower).
- accepted caveats: nothing for masked HSL's sub-stages (M8.3j); nothing for a colour range's
  blend; coverage still dominates the real photo; an auto-masked brush is still uncacheable across
  a global slider; one machine, one rung, one photographer's single masked photo.

Then rewrite the **▶ NEXT ACTION** block to name **M8.3j — the mask loop on the GPU**, carrying
the measured 82 ms-per-mask figure for masked HSL, the note that `accelerate` sees `MaskStage` as
one opaque stage, and ⚠⚠ **the fifth renumbering warning**.

Update the handoff line: tests **2322**, 63 goldens, branch `m8.3i-mask-loop`, gated, not merged,
not pushed, **not yet confirmed by use**.

- [ ] **Step 4: Commit**

```bash
git diff --stat && git diff | grep '^-' | grep -v '^---'
git add CLAUDE.md
git commit -m "docs: record M8.3i, the parallel brush rasterizer"
```

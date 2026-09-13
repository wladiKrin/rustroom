# M8.5 — the sequential resamplers, in parallel: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **⚠ Project conventions, non-negotiable.** Commit messages carry **no `Co-Authored-By`
> trailer**. **Never push** — the user pushes to `github.com/wladiKrin/rustroom` himself. Work
> happens on the branch `m8.5-parallel-resamplers`, which already exists.

**Goal:** Parallelise `geometry::rotate` and `PerspectiveStage::apply` over output rows, recovering
about 255 ms of a 529 ms resampling prefix.

**Architecture:** Both are gathers — an output pixel reads the whole source and writes only itself
— so `par_chunks_exact_mut(stride)` over output rows is **bit-identical** to the sequential form.
That is what makes the 63 goldens the acceptance test rather than something new.

**Tech Stack:** Rust, rayon (already a `pipeline` dependency and already used in `spatial.rs`).

**Spec:** `docs/superpowers/specs/2026-09-13-parallel-resamplers-design.md`

---

## Build and test commands

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- Fast loop: `cargo test -p pipeline`
- Full gate: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --workspace` — **each exit code read from its own command**, never through a pipe.

**Baseline to assert against:** 2428 tests, 63 goldens. ⚠ Both must be **unchanged**, because this
changes when work happens and never what it computes.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/pipeline/src/stages/geometry.rs` | **Modify.** `rotate` only — not `orient`, not `apply_crop`. |
| `crates/pipeline/src/stages/perspective.rs` | **Modify.** `apply`'s pixel loop, and the comment that describes it as sequential. |
| `crates/app/examples/probe_resamplers.rs` | **Modify.** Add a direct measurement of perspective's parallel form. |

---

## Task 1: Measure perspective's parallel form directly

**Why:** The spec's ~126 ms for perspective is **inferred from geometry's 5.14×**, and reasoning by
analogy is what this crate has been wrong about three times (M8.3g by ~2×, M8.3f by 8× in
attribution, M8.3o by ~18%). Measure it before claiming it.

**Files:**
- Modify: `crates/app/examples/probe_resamplers.rs`

- [ ] **Step 1: Add a parallel homography for measurement**

Add this beside `rotate_parallel` in `crates/app/examples/probe_resamplers.rs`:

```rust
/// A rayon-parallel homography, for measurement only — the same inverse map
/// `PerspectiveStage::apply` uses.
///
/// ⚠ Parallel over **output** rows, sound for the same reason the rotation's is: a gather.
fn perspective_parallel(
    src: &ImageBuf,
    params: &edit_model::Perspective,
    weights: &pipeline::resample::Weights,
) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let aspect = cx / cy;
    let hom = pipeline::perspective::Homography::from_edits(params, aspect);
    const CH: usize = ImageBuf::CHANNELS;
    let stride = w * CH;
    let mut data = vec![0.0f32; w * h * CH];
    if stride == 0 {
        return ImageBuf::from_vec(w, h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(y, orow)| {
            for (x, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                let s = hom.source_of(n);
                *out = pipeline::resample::sample(src, cx + s.0 * cy, cy + s.1 * cy, weights);
            }
        });
    ImageBuf::from_vec(w, h, data)
}
```

⚠ If `pipeline::perspective::Homography` is not public, note that and use the stage itself for the
sequential half only, reporting the parallel half as unmeasurable from outside the crate — do
**not** silently reimplement the homography, which would measure a different transform.

- [ ] **Step 2: Print the comparison**

At the end of `main`, after the existing geometry comparison, add:

```rust
    let p = edit_model::Perspective {
        vertical: 30.0,
        ..Default::default()
    };
    let pseq = best_of(3, || {
        pipeline::PerspectiveStage::new(p, Resampler::default()).apply(src.clone())
    });
    let ppar = best_of(3, || perspective_parallel(&src, &p, &weights));
    println!(
        "  perspective,             sequential {pseq:.1} ms vs rayon {ppar:.1} ms — {:.2}x",
        pseq / ppar
    );
```

- [ ] **Step 3: Run it and record the number**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
uptime
cargo run --release -p app --example probe_resamplers ~/Pictures/fuji/DSCF6296.RAF
uptime
```

Record both ratios and the load average. ⚠ If perspective's ratio differs materially from
geometry's 5.14×, **say so and say why you think so** — it is a finding about the homography's
arithmetic, not noise to smooth over.

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/probe_resamplers.rs
git commit -m "bench: price perspective's parallel form directly"
```

---

## Task 2: Parallelise `geometry::rotate`

**Files:**
- Modify: `crates/pipeline/src/stages/geometry.rs`

- [ ] **Step 1: Replace `rotate`'s body**

In `crates/pipeline/src/stages/geometry.rs`, replace the whole `rotate` function with:

```rust
/// Rotate about the image center by `angle_rad`, keeping the canvas size.
///
/// ⚠⚠ **Parallel over OUTPUT rows, and that is bit-identical rather than merely close.** This is a
/// **gather**: an output pixel reads the whole source and writes only itself, so no two threads
/// write the same byte and none reads another's output. The float operations are therefore the
/// same operations in the same order as the sequential form — which is what lets the 63 goldens
/// stand as the check on this, exactly as they do for `LensCorrectionStage`'s own rayon port.
///
/// ⚠ Measured at a native rung (3123x2085, Catmull-Rom, 8 threads): **160.5 ms sequential against
/// 31.2 ms here, 5.14x**. The sequential form was a deliberate choice that predated anyone
/// measuring it — `perspective.rs` recorded "if this ever needs to be faster, both should move
/// together", and they did.
fn rotate(src: &ImageBuf, angle_rad: f32, weights: &Weights) -> ImageBuf {
    let w = src.width();
    let h = src.height();
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = angle_rad.sin_cos();
    const CH: usize = ImageBuf::CHANNELS;
    let stride = w * CH;
    let mut data = vec![0.0f32; w * h * CH];
    // ⚠ `par_chunks_exact_mut(0)` PANICS, and a zero-width image reaches here. The same guard
    // `map_pixels_xy` and `LensCorrectionStage` already carry.
    if stride == 0 {
        return ImageBuf::from_vec(w, h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(y, orow)| {
            for (x, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                // Inverse map: sample the source at -angle.
                let sx = cx + dx * cos + dy * sin;
                let sy = cy - dx * sin + dy * cos;
                *out = resample::sample(src, sx, sy, weights);
            }
        });
    ImageBuf::from_vec(w, h, data)
}
```

- [ ] **Step 2: Add the rayon import**

At the top of the file, after `use edit_model::{...};`, add:

```rust
use rayon::prelude::*;
```

- [ ] **Step 3: Run the pipeline tests and the goldens**

```bash
cargo test -p pipeline 2>&1 | tail -20
```

Expected: **every test passes and the count is unchanged**. ⚠ Assert "unchanged", not "green" —
a refactor that changes the test count moved behaviour.

- [ ] **Step 4: Prove bit-identity on the goldens specifically**

```bash
cargo test -p pipeline --test golden 2>&1 | grep -E "^test result"
git status --short crates/pipeline/tests/
```

Expected: all 63 pass and `git status` shows **no modified golden PNG**. ⚠ That second command is
the real check: a golden that moved would be rewritten only under `UPDATE_GOLDEN=1`, so an
unmodified tree plus a passing suite is what establishes byte-identity.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/pipeline/src/stages/geometry.rs
git commit -m "perf: geometry's rotation runs across output rows"
```

---

## Task 3: Parallelise `PerspectiveStage::apply`

**Files:**
- Modify: `crates/pipeline/src/stages/perspective.rs`

- [ ] **Step 1: Replace the pixel loop**

In `crates/pipeline/src/stages/perspective.rs`, replace this block:

```rust
        // ⚠ Sequential, mirroring `geometry::rotate` in the sibling stage rather than reaching for
        // rayon. Geometry's whole-buffer rotation measures 203 ms at full resolution and nobody
        // has complained; matching it keeps the two readable side by side. If this ever needs to
        // be faster, both should move together.
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                let s = hom.source_of(n);
                let px = resample::sample(&image, cx + s.0 * cy, cy + s.1 * cy, &self.weights);
                out.set_pixel(x, y, px);
            }
        }
        out
```

with:

```rust
        // ⚠⚠ **Parallel over OUTPUT rows, and bit-identical rather than merely close.** This is a
        // **gather**: an output pixel reads the whole source and writes only itself, so the float
        // operations are the same operations in the same order as the sequential form — which is
        // what lets the 63 goldens stand as the check on it.
        //
        // ⚠ It moved together with `geometry::rotate`, which is what the note that used to sit
        // here asked for: "if this ever needs to be faster, both should move together." Measured
        // at a native rung, geometry's went 160.5 -> 31.2 ms, 5.14x.
        const CH: usize = ImageBuf::CHANNELS;
        let stride = w * CH;
        let mut data = vec![0.0f32; w * h * CH];
        // ⚠ `par_chunks_exact_mut(0)` PANICS. Unreachable behind the `w < 2` guard above, and
        // kept because that guard is about the identity case rather than about this.
        if stride == 0 {
            return ImageBuf::from_vec(w, h, data);
        }
        data.par_chunks_exact_mut(stride)
            .enumerate()
            .for_each(|(y, orow)| {
                for (x, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                    let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                    let s = hom.source_of(n);
                    *out = resample::sample(&image, cx + s.0 * cy, cy + s.1 * cy, &self.weights);
                }
            });
        ImageBuf::from_vec(w, h, data)
```

- [ ] **Step 2: Add the rayon import**

At the top of the file, after `use edit_model::Perspective;`, add:

```rust
use rayon::prelude::*;
```

- [ ] **Step 3: Run the tests and check the goldens**

```bash
cargo test -p pipeline 2>&1 | tail -20
git status --short crates/pipeline/tests/
```

Expected: count **unchanged**, no modified golden.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add crates/pipeline/src/stages/perspective.rs
git commit -m "perf: the perspective correction runs across output rows"
```

---

## Task 4: Re-measure the prefix

**Files:** none modified — this is the acceptance measurement.

- [ ] **Step 1: Run the probe three times**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
uptime
for i in 1 2 3; do
  echo "### RUN $i"
  cargo run --release -p app --example probe_resamplers ~/Pictures/fuji/DSCF6296.RAF 2>&1 \
    | grep -E "ms |x$"
done
uptime
```

- [ ] **Step 2: Record the band, not a figure**

Write down, for each of `lens_correction`, `perspective`, `geometry` and `ALL THREE`, the **band
across three runs** and the load average at both ends. ⚠ **Quote a band, not a figure** — M8.3h
had to retract a headline for comparing a point measurement against a point prediction, and M8.3q
found the same thing one milestone ago.

Against the before figures: lens 212.8 / perspective 156.0 / geometry 160.1 / **all three 528.9**.

⚠ The sequential-vs-rayon rows in the probe now compare the **new** stage against the probe's own
parallel copy, so they should read **~1.0×**. That is the expected reading and it confirms the
production stage really took the change; a ratio still near 5× means the edit did not land where
the probe is looking.

---

## Task 5: The full gate

- [ ] **Step 1: Three commands, three exit codes**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --check > /tmp/m85-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m85-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m85-test.log 2>&1; echo "TEST=$?"
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`.

⚠ **Read clippy's exit code before committing anything.** `cargo test --workspace` does **not
build examples** and `cargo run --example` does not deny warnings, so the probe's new code is
visible only here — which is exactly how M8.3q shipped a commit with `CLIPPY=101` one milestone
ago.

- [ ] **Step 2: Assert the counts are UNCHANGED**

```bash
grep -E "^test result: ok\. [0-9]+ passed" /tmp/m85-test.log | awk '{s+=$4} END {print "TOTAL:", s}'
grep -c "^test golden_" /tmp/m85-test.log
grep -c "FAILED" /tmp/m85-test.log
git status --short
```

Expected: **2428**, **63**, **0**, and a clean tree. ⚠ Unlike a feature milestone the count must
**not** move: this changes when work happens, never what it computes. A moved count means
behaviour moved.

- [ ] **Step 3: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

In the background. What to say to the user:

1. **Open a RAW with a lens profile and drag Straighten.** Should be visibly quicker to settle.
   ⚠⚠ **The picture must not change** — this is bit-identical, so anything that looks different is
   a bug rather than an improvement.
2. **Then a Perspective slider**, same expectation.
3. ⚠ **Both at 1:1**, where the prefix is whole-buffer and the cost is largest.

- [ ] **Step 4: Do NOT merge and do NOT push**

Report the gate, the band, and the before/after. The user decides.

---

## Task 6: Record it in `CLAUDE.md`

- [ ] **Step 1: Add the milestone section**

Add an **M8.5** section above `**M8.3q — noise reduction on the GPU**`, carrying: the before/after
table, the 5.14× and perspective's measured ratio, the gather argument for why the goldens are the
acceptance test, and the **Accepted caveats** from spec §5.

- [ ] **Step 2: Rewrite the next action**

⚠⚠ This is the important half. Item 3 of the agreed order says *"the three resamplers as ONE
mechanism"*, and the measurement has made that wrong in two ways — record both:

- **They are one KIND of problem, not one mechanism.** Lens correction samples a **per-channel**
  displacement field (TCA), so its field is six components per pixel and not two; perspective and
  geometry are analytic maps wanting no texture at all.
- **Two of the three did not need the GPU.** After this the resampling prefix is ~275 ms of which
  **lens correction is ~77%**, so item 3 becomes **"port lens correction"**.

Demote the current next action to `⚠ Superseded next action follows:` in the usual way, and update
the order list's item 3 to say what it is now.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record M8.5, the sequential resamplers in parallel"
```

---

## Self-review notes

**Spec coverage:** §1 → Tasks 1 and 4. §2 → Task 6 step 2. §3 → Tasks 2 and 3. §4 → Task 1
(perspective measured directly), Task 2 step 4 and Task 5 step 2 (bit-identity via unmoved
goldens). §5 → Task 6 step 1.

**Type consistency:** `resample::sample(&ImageBuf, f32, f32, &Weights) -> [f32; 3]` is used with
that signature in Tasks 1, 2 and 3. `ImageBuf::CHANNELS` and `ImageBuf::from_vec(w, h, data)` are
used identically in all three.

**Known risk, stated rather than hidden:** Task 1 depends on `pipeline::perspective::Homography`
being reachable from an example. Step 1 says what to do if it is not, rather than assuming — and
the fallback deliberately refuses to reimplement the homography, because that would measure a
different transform and report it as this one.

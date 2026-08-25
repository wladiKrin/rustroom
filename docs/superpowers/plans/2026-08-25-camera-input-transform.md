# Camera input transform (C1) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the camera→ProPhoto transform out of libraw and into `raw-decode`, producing a byte-comparable render, so that C2 has somewhere to put a DCP's matrix.

**Architecture:** libraw is asked for camera-native RGB (`output_color = 0`) and `raw-decode` applies the camera matrix itself before returning. Nothing downstream learns anything: `pipeline` and all 39 goldens are untouched. The old libraw path stays reachable behind a flag as a permanent oracle.

**Tech Stack:** Rust, libraw via `rsraw`, the existing `color` crate.

**Spec:** `docs/superpowers/specs/2026-08-25-camera-input-transform-design.md`

**Before anything:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
git checkout -b camera-input-transform
```

Branch per feature; **never push**. A real RAW is needed throughout:
`RAW=~/Desktop/fuji/DSCF6509.RAF`

---

## File Structure

| file | responsibility |
|---|---|
| `crates/raw-decode/examples/color_probe.rs` | **create** — print libraw's colour data; decides Task 2 |
| `crates/color/src/camera.rs` | **create** — compose and normalise a camera matrix; pure, no libraw |
| `crates/color/src/lib.rs` | *modify* — declare the module |
| `crates/raw-decode/src/lib.rs` | *modify* — `DecodeOptions.camera_native`; gated acceptance test |
| `crates/raw-decode/src/backend_rsraw.rs` | *modify* — `output_color = 0` and apply the matrix |

⚠ `pipeline` is **not** in this table and must not enter it. The input transform belongs to the
decoder, which is the only component that knows which camera took the picture.

---

### Task 1: find out what libraw actually gives us

⚠ **An investigation, and it decides Task 2's implementation.** Whether we can use libraw's own
`rgb_cam` or must derive the matrix from `cam_xyz` is a fact about libraw, not something to reason
about — and the row-normalisation and illuminant conventions are exactly where a plausible
derivation goes silently wrong.

**Files:**
- Create: `crates/raw-decode/examples/color_probe.rs`

- [ ] **Step 1: Write the probe**

```rust
//! Print libraw's colour data at `output_color` 0 and 4.
//!
//! `cargo run --release -p raw-decode --example color_probe <raw>`
use rsraw::RawImage;

fn main() {
    let path = std::env::args().nth(1).expect("usage: color_probe <raw>");
    for oc in [0i32, 4] {
        let bytes = std::fs::read(&path).expect("read");
        let mut raw = RawImage::open(&bytes).expect("open");
        raw.unpack().expect("unpack");
        raw.set_use_camera_wb(true);
        {
            let params = &mut raw.as_mut().params;
            params.output_color = oc;
            params.no_auto_bright = 1;
            params.gamm[0] = 1.0;
            params.gamm[1] = 1.0;
        }
        let _ = raw.process::<16>().expect("process");
        let c = &raw.as_mut().color;
        println!("=== output_color = {oc} ===");
        println!("cam_xyz  = {:?}", &c.cam_xyz[..3]);
        println!("rgb_cam  = {:?}", c.rgb_cam);
        println!("cmatrix  = {:?}", c.cmatrix);
        println!("pre_mul  = {:?}", c.pre_mul);
        println!("cam_mul  = {:?}", c.cam_mul);
    }
}
```

- [ ] **Step 2: Run it and record the numbers**

```bash
cargo run --release -p raw-decode --example color_probe "$RAW"
```

- [ ] **Step 3: Decide, by this rule**

- **If `rgb_cam` is populated and non-identity at `output_color = 0`** → Task 2 uses it directly.
  No derivation, no normalisation question, and it is libraw's own answer, so agreement is by
  construction.
- **If `rgb_cam` is identity or zero at 0** → Task 2 derives the matrix from `cam_xyz` following
  dcraw's `cam_xyz_coeff`: compose with XYZ→ProPhoto, **normalise every row to sum to 1**, invert.
  ⚠ The acceptance test in Task 3 is then the arbiter; if it fails, the transpose/inverse
  orientation is the first thing to vary, and the failure will be a colour cast, not noise.

Write the outcome into the commit message so the next reader knows which branch was taken.

- [ ] **Step 4: Commit the probe**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add crates/raw-decode/examples/color_probe.rs
git commit -m "chore(raw-decode): probe libraw's colour data"
```

⚠ Check clippy's **exit code**, not grep output — a `clippy … | grep` pipeline reports grep's
status, which has already let a failing clippy into a commit twice in this colour work.

---

### Task 2: apply the camera matrix ourselves

**Files:**
- Create: `crates/color/src/camera.rs`
- Modify: `crates/color/src/lib.rs`
- Modify: `crates/raw-decode/src/lib.rs`
- Modify: `crates/raw-decode/src/backend_rsraw.rs`

- [ ] **Step 1: Write the failing tests for the matrix maths**

Create `crates/color/src/camera.rs` with only its test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_sums_to_one() {
        // ⚠ The property that makes camera white land on output white. Without it every image
        // carries a colour cast — subtle enough to ship, bad enough to matter.
        let m = normalize_rows([[0.8, 0.1, 0.2], [0.05, 0.9, 0.1], [0.0, 0.1, 0.7]]);
        for (i, row) in m.iter().enumerate() {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "row {i} sums to {sum}");
        }
    }

    #[test]
    fn a_neutral_camera_pixel_stays_neutral() {
        // ⚠ The same property expressed the way it is actually observed: equal camera channels
        // must produce equal output channels. A row-sum test alone would pass a matrix whose
        // rows were individually normalised but wrongly ordered.
        let m = normalize_rows([[0.8, 0.1, 0.2], [0.05, 0.9, 0.1], [0.0, 0.1, 0.7]]);
        let out = crate::spaces::apply(&m, [0.5, 0.5, 0.5]);
        assert!(
            (out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6,
            "neutral in, {out:?} out"
        );
    }

    #[test]
    fn normalizing_leaves_an_already_normal_matrix_alone() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(normalize_rows(id), id);
    }

    #[test]
    fn a_degenerate_row_is_left_alone_rather_than_producing_nan() {
        // ⚠ A zero row would divide by zero and poison the whole image with NaN, which compares
        // false against every clamp and shows as black. Failing toward "unchanged" is the safe
        // direction, the same call `PrefixKey` makes for NaN.
        let m = normalize_rows([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(m[0].iter().all(|v| v.is_finite()), "got {m:?}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p color camera`
Expected: FAIL — `cannot find function normalize_rows`.

- [ ] **Step 3: Implement**

Prepend to `crates/color/src/camera.rs`:

```rust
//! The camera → working-space transform.
//!
//! ⚠ This is the **input** side of colour management: it decides what the sensor's numbers mean
//! before any edit touches them. It lives in `color` so it can be tested without libraw, and is
//! applied by `raw-decode`, which is the only component that knows which camera took the picture.

/// Scale each row so it sums to 1.
///
/// ⚠⚠ **This is what makes camera white land on output white**, and it is dcraw's own step —
/// omitting it gives every image a colour cast that is subtle enough to ship. A row that sums to
/// zero is left alone rather than divided by it: a NaN would compare false against every clamp
/// downstream and show as black.
pub fn normalize_rows(mut m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    for row in m.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-9 {
            for v in row.iter_mut() {
                *v /= sum;
            }
        }
    }
    m
}
```

Declare it in `crates/color/src/lib.rs` beside the other modules:

```rust
pub mod camera;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p color`
Expected: PASS.

- [ ] **Step 5: Sabotage**

| sabotage | must fail |
|---|---|
| drop the division (`m` returned unchanged) | `every_row_sums_to_one`, `a_neutral_camera_pixel_stays_neutral` |
| divide by `sum` unconditionally | `a_degenerate_row_is_left_alone_rather_than_producing_nan` |
| normalise *columns* instead of rows | `a_neutral_camera_pixel_stays_neutral` |

Confirm each compiles before believing the result, then revert.

- [ ] **Step 6: Add the flag and the decode branch**

In `crates/raw-decode/src/lib.rs`, add to `DecodeOptions`:

```rust
    /// Apply the camera → working-space matrix ourselves rather than letting libraw do it.
    ///
    /// ⚠ `false` selects libraw's own conversion, and it stays reachable **permanently** as an
    /// oracle: C2 and C3 each change this matrix, and a reference that does not move when the
    /// thing under test does is worth far more than the few lines it costs. It is the same role
    /// the golden harness plays for `pipeline`.
    pub camera_native: bool,
```

`linear_scene()` sets `camera_native: true`; `display()` sets `camera_native: false` — ⚠ that dump
is a debug tool and deliberately keeps asking libraw for a baked sRGB image.

In `crates/raw-decode/src/backend_rsraw.rs`, replace the hardcoded `params.output_color = 4;`:

```rust
        // ⚠ 0 = camera-native, 4 = ProPhoto. At 0 libraw still demosaics and still applies the
        // camera white balance; only the colour-space conversion is skipped, which is the step
        // we take over. See `camera_native`.
        params.output_color = if opts.camera_native { 0 } else { 4 };
```

Then, **after** `process()` and before building the `ImageBuf`, capture the matrix and apply it.
The exact source of `cam` is whichever branch Task 1 chose:

```rust
    // ⚠ Read before the borrow of `processed` ends. Whether this is libraw's own `rgb_cam` or a
    // matrix derived from `cam_xyz` was decided by `examples/color_probe.rs` — see Task 1.
    let cam: Option<[[f32; 3]; 3]> = if opts.camera_native {
        let c = &raw.as_mut().color;
        let mut m = [[0.0f32; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            row.copy_from_slice(&c.rgb_cam[i][..3]);
        }
        Some(color::camera::normalize_rows(m))
    } else {
        None
    };
```

and apply it where `data` is built:

```rust
    let image = match cam {
        Some(m) => ImageBuf::from_vec(width, height, data).map_pixels(|p| color::spaces::apply(&m, p)),
        None => ImageBuf::from_vec(width, height, data),
    };
```

Add `color = { path = "../color" }` to `crates/raw-decode/Cargo.toml` if absent.

⚠ Every existing `DecodeOptions { .. }` literal that lists fields exhaustively becomes a compile
error until it names the new field. That is the point of those literals — let the compiler find
them rather than grepping.

- [ ] **Step 7: Build and run the suite**

Run: `cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS, **39 goldens byte-identical**.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(raw-decode): apply the camera matrix ourselves"
```

---

### Task 3: prove the render did not change

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`

- [ ] **Step 1: Write the acceptance test**

Add beside the existing `RAW_FIXTURE`-gated tests in `crates/raw-decode/src/lib.rs`:

```rust
    #[test]
    fn our_camera_matrix_reproduces_librarys() {
        // ⚠⚠ **The acceptance test for this whole slice.** C1 moves a well-tested transform out
        // of libraw and into our code for zero user-visible gain, so the only claim it makes is
        // "the render is unchanged" — and this is the only test that can check it.
        //
        // ⚠ Gated on `RAW_FIXTURE`, so it cannot run in CI without a file. Stated as a
        // limitation rather than hidden: run it locally before trusting this slice.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let ours = decode_raw(
            &path,
            &DecodeOptions {
                camera_native: true,
                half_size: true,
                ..DecodeOptions::linear_scene()
            },
        )
        .expect("ours");
        let libraw = decode_raw(
            &path,
            &DecodeOptions {
                camera_native: false,
                half_size: true,
                ..DecodeOptions::linear_scene()
            },
        )
        .expect("libraw");

        assert_eq!(ours.image.width(), libraw.image.width());
        assert_eq!(ours.image.height(), libraw.image.height());

        let (mut max, mut sum, mut n) = (0.0f32, 0.0f64, 0usize);
        for y in 0..ours.image.height() {
            for x in 0..ours.image.width() {
                let (a, b) = (ours.image.pixel(x, y), libraw.image.pixel(x, y));
                for i in 0..3 {
                    let d = (a[i] - b[i]).abs();
                    max = max.max(d);
                    sum += d as f64;
                    n += 1;
                }
            }
        }
        let mean = sum / n as f64;
        eprintln!("camera matrix vs libraw: max {max:.5}, mean {mean:.6}");
        // ⚠ A tolerance, not equality: libraw works in 16-bit integers internally while we apply
        // the matrix in f32, so the two cannot be bit-identical. 2/255 is the same bar the golden
        // harness uses, expressed in the 0..1 working scale.
        assert!(
            max < 2.0 / 255.0,
            "max per-channel difference {max} exceeds the tolerance — the matrix is wrong"
        );
    }
```

- [ ] **Step 2: Run it**

```bash
RAW_FIXTURE=~/Desktop/fuji/DSCF6509.RAF cargo test --release -p raw-decode our_camera_matrix -- --nocapture
```

Expected: PASS, printing the max and mean.

⚠ **If it fails with a large `max` and a systematic sign, the matrix orientation is wrong** —
transpose it, or use its inverse, and re-run. That is what the test is for. **Do not widen the
tolerance to make it pass**: a cast is exactly what this slice exists to avoid, and a loosened
bound would hide it in every image forever.

⚠ If it cannot be made to agree, **stop and report** rather than shipping "close enough" — the
spec names that as the honest outcome.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "test(raw-decode): our camera matrix reproduces libraw's render"
```

---

### Task 4: verify in the app, then update the docs

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: PASS, 39 goldens byte-identical.

- [ ] **Step 2: Launch and look**

```bash
cargo run --release -p app
```

Open a RAW. ⚠ **The expected result is that nothing looks different.** A visible change means the
matrix is wrong — the opposite of the usual "can you see it working?" check.

- [ ] **Step 3: Update `CLAUDE.md`**

A section in the house style: what landed, the ⚠ findings (which branch Task 1's probe chose and
the numbers behind it; the row normalisation and why; the libraw path kept as a permanent oracle;
the measured max/mean against libraw), the test count, and the caveats — no visible change by
design, no DCP yet, licensing deferred to C4. Update **▶ NEXT ACTION** to **C2**.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the camera input transform (C1)"
```

⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** transform in `raw-decode` not `pipeline` (Task 2) ✓; `output_color = 0`
(Task 2) ✓; matrix verified rather than reasoned (Tasks 1, 3) ✓; row normalisation with its own
tests (Task 2) ✓; libraw path kept as a permanent oracle (Task 2 flag, Task 3 test) ✓; goldens
untouched (Tasks 2, 4) ✓; gated acceptance test with measured numbers (Task 3) ✓; caveats and the
"stop rather than ship a cast" rule (Task 3, Task 4) ✓.

**Type consistency:** `color::camera::normalize_rows([[f32;3];3]) -> [[f32;3];3]` is defined in
Task 2 and used there. `DecodeOptions.camera_native` is defined in Task 2 and used in Task 3.
`color::spaces::apply` is the existing helper, unchanged.

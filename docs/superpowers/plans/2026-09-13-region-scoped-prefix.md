# M8.6 — a region-scoped prefix, implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:executing-plans` to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **no `Co-Authored-By` trailer**
> and no AI attribution of any kind. **Never push** — the user pushes himself. Work on the current
> branch. Read the clippy exit code from its own command before every commit; `cargo test` being
> green is not evidence about clippy, and this file records that trap at least twenty-two times.

**Goal:** during a Straighten or Perspective drag at a native rung, the prefix's last step produces
only the sub-rect the tail is about to crop to — a drag frame **144.4 → ~26 ms**, bit-identical.

**Architecture:** two defaulted `PipelineStage` methods (`output_dims`, `apply_region`), overridden
by `GeometryStage` and `PerspectiveStage` only. `render_cached` engages them when **exactly the
last prefix step re-runs** and never caches the result.

**Tech stack:** Rust, rayon, the existing `pipeline` / `app` crates. No new dependency.

**Spec:** `docs/superpowers/specs/2026-09-13-region-scoped-prefix-design.md`

---

## File structure

| file | change |
|---|---|
| `crates/pipeline/src/lib.rs` | add `OutRect`; two defaulted `PipelineStage` methods; the total `output_dims` test |
| `crates/pipeline/src/stages/geometry.rs` | extract `crop_bounds`; `rotate` becomes a wrapper over a new `rotate_region`; `output_dims` + `apply_region` |
| `crates/pipeline/src/stages/perspective.rs` | extract `render`; `apply_region` |
| `crates/app/src/render_engine.rs` | the gate, the predicted dims, no cache insert, `Ran::region_scoped` |
| `crates/app/examples/probe_resamplers.rs` | a region-scoped drag row |
| `CLAUDE.md` | the section |

---

## Task 1: one rule for the crop's bounds

Pure refactor. `apply_crop` computes the crop's pixel bounds inline; `output_dims` needs the same
numbers in Task 2. ⚠ Two derivations of "where does the crop start" is precisely how the predicted
output dimensions and the real ones would come to disagree — the class of disagreement §5 of the
spec is about.

**Files:** Modify `crates/pipeline/src/stages/geometry.rs`

- [ ] **Step 1: extract `crop_bounds`, called by `apply_crop`**

Replace the whole of `apply_crop` (currently at `crates/pipeline/src/stages/geometry.rs:123-135`)
with:

```rust
/// The crop's pixel bounds `(x0, y0, x1, y1)` in an image of `w` x `h`.
///
/// ⚠⚠ **The one rule, read by both `apply_crop` and `GeometryStage::output_dims`.** Two
/// derivations of where the crop starts is how a predicted output size and the real one come to
/// disagree — and a region-scoped render would then rasterise every mask at the wrong size, which
/// is a wrong picture rather than an error.
///
/// ⚠ Found in passing and deliberately **not** fixed here: a `CropRect` with `left == 1.0` gives
/// `x0 == w`, and `clamp(x0 + 1, w)` then panics because its minimum exceeds its maximum. That is
/// pre-existing and unreachable from the crop tool, which keeps a minimum size; fixing it inside a
/// refactor whose acceptance is "no behaviour moved" would hide the change.
fn crop_bounds(w: usize, h: usize, c: &CropRect) -> (usize, usize, usize, usize) {
    let x0 = ((c.left.clamp(0.0, 1.0) * w as f32).round() as usize).min(w);
    let y0 = ((c.top.clamp(0.0, 1.0) * h as f32).round() as usize).min(h);
    let x1 = ((c.right.clamp(0.0, 1.0) * w as f32).round() as usize).clamp(x0 + 1, w);
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h as f32).round() as usize).clamp(y0 + 1, h);
    (x0, y0, x1, y1)
}

/// Crop to the normalized rectangle; a full-frame rect returns the image unchanged.
fn apply_crop(image: ImageBuf, c: &CropRect) -> ImageBuf {
    let w = image.width();
    let h = image.height();
    let (x0, y0, x1, y1) = crop_bounds(w, h, c);
    if x0 == 0 && y0 == 0 && x1 == w && y1 == h {
        return image;
    }
    image.crop(x0, y0, x1 - x0, y1 - y0)
}
```

- [ ] **Step 2: verify nothing moved**

```bash
cargo test -p pipeline 2>&1 | tail -5
cargo test -p pipeline --test golden 2>&1 | tail -5
git status --short tests/
```

Expected: all green, and **`git status` over `tests/` is empty** — a golden is rewritten only under
`UPDATE_GOLDEN=1`, so an unmodified tree plus a green run is what establishes byte-identity. ⚠ That
pair is the acceptance, not "still green".

- [ ] **Step 3: commit**

```bash
git add crates/pipeline/src/stages/geometry.rs
git commit -m "refactor: one rule for the crop's pixel bounds"
```

---

## Task 2: `OutRect`, the two defaulted methods, and the total guard

**Files:**
- Modify: `crates/pipeline/src/lib.rs`
- Modify: `crates/pipeline/src/stages/geometry.rs`

- [ ] **Step 1: add `OutRect` beside `Region` in `crates/pipeline/src/lib.rs`**

Insert immediately after the `Region` struct (which ends at `crates/pipeline/src/lib.rs:370`):

```rust
/// A sub-rect of a stage's **output**, for [`PipelineStage::apply_region`].
///
/// ⚠ Deliberately **not** [`Region`]: that type carries an origin plus the *frame's* size and takes
/// its own width and height from the buffer it describes. A stage producing a sub-rect cannot do
/// that, because the buffer does not exist yet — the same trap `render_cached`'s `clip` comment
/// already records for deriving a size as "frame minus origin".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutRect {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}
```

- [ ] **Step 2: add the two defaulted methods to `PipelineStage`**

In `crates/pipeline/src/lib.rs`, insert before the closing brace of `trait PipelineStage`
(currently `crates/pipeline/src/lib.rs:242`), after `has_coverage_rasterizer`:

```rust
    /// This stage's output dimensions for an input of `w` x `h`. Default: unchanged.
    ///
    /// ⚠⚠ **The caller needs this BEFORE the stage runs.** A region-scoped render never produces
    /// the whole frame, and the frame's dimensions are what every brush mask's coverage is
    /// rasterised at. Taking them off a region-scoped buffer would rasterise every mask at the
    /// region's size — a wrong mask, and a coverage cache that thrashes on every pan.
    ///
    /// ⚠ The identity default is correct for every stage in the tree today: lens correction,
    /// perspective, spot removal, noise reduction, dehaze and the masked capture corrections all
    /// preserve dimensions, and only `GeometryStage` does not. ⚠⚠ It is silently wrong for a
    /// future dim-changing stage, which is what
    /// `every_prefix_stages_output_dims_match_what_it_produces` exists to catch.
    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        (w, h)
    }

    /// Produce only `rect` of this stage's output. Default: this stage cannot.
    ///
    /// ⚠⚠ **`None` is what makes every other stage decline BY CONSTRUCTION.** Dehaze's airlight is
    /// estimated from the whole frame and noise reduction reads a neighbourhood, so neither can be
    /// region-scoped without changing the photograph as you pan. Inheriting the default is what
    /// stops a caller name-checking its way into that. The rule `GpuStage::cpu()` and
    /// `accelerate_prefix_stage` already follow: **fail toward the whole-buffer path**, where a
    /// wrong answer costs milliseconds and never pixels.
    ///
    /// ⚠ It **borrows** where [`Self::apply`] consumes, and that is load-bearing rather than tidy:
    /// a gather only reads its source, and the consuming signature would force a clone of the whole
    /// checkpoint — measured at 8.4 ms on a 26 Mpx buffer, most of the frame this exists to shrink.
    ///
    /// ⚠ `rect` is in the stage's **output** coordinates, so a caller must resolve it against
    /// [`Self::output_dims`] rather than against the input.
    fn apply_region(&self, _src: &ImageBuf, _rect: OutRect) -> Option<ImageBuf> {
        None
    }
```

- [ ] **Step 3: override `output_dims` on `GeometryStage`**

In `crates/pipeline/src/stages/geometry.rs`, inside `impl PipelineStage for GeometryStage`, after
`apply`:

```rust
    /// ⚠ Orientation can swap the axes and the crop reduces; the rotation keeps the canvas size,
    /// so those are the only two terms. Read through `crop_bounds`, never re-derived.
    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        let (w, h) = if self.orientation.rotate.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        };
        let (x0, y0, x1, y1) = crop_bounds(w, h, &self.crop);
        if x0 == 0 && y0 == 0 && x1 == w && y1 == h {
            (w, h)
        } else {
            (x1 - x0, y1 - y0)
        }
    }
```

Add `use crate::OutRect;` to the imports at the top of `geometry.rs` (it is needed in Task 3;
adding it now with `apply_region` absent would be an unused import, so add it in Task 3 instead).

- [ ] **Step 4: write the total guard**

In `crates/pipeline/src/lib.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    /// ⚠⚠ Total over the **whole prefix** rather than over `GeometryStage` alone, because the
    /// identity default is what every other stage inherits — and a default that is silently wrong
    /// for a future dim-changing stage is exactly the trap this guards. A region-scoped render
    /// takes the frame's dimensions from this prediction, so a stage that lies here rasterises
    /// every brush mask at the wrong size.
    #[test]
    fn every_prefix_stages_output_dims_match_what_it_produces() {
        let mut src = ImageBuf::new(37, 23);
        for y in 0..23 {
            for x in 0..37 {
                src.set_pixel(x, y, [x as f32 / 37.0, y as f32 / 23.0, 0.5]);
            }
        }
        let steps = build_prefix_steps(
            &all_edits(),
            RenderScale::new(1.0),
            None,
            Resampler::default(),
        );
        assert!(
            steps.len() >= 4,
            "fixture check: the fixture must build a real prefix, got {}",
            steps.len()
        );
        let mut img = src;
        for step in &steps {
            let predicted = step.stage.output_dims(img.width(), img.height());
            let out = step.stage.apply(img.clone());
            assert_eq!(
                predicted,
                (out.width(), out.height()),
                "`{}` predicted {predicted:?} and produced {:?}",
                step.stage.name(),
                (out.width(), out.height())
            );
            img = out;
        }
    }
```

⚠ If `all_edits()` is not in scope in that module, find the existing helper by
`grep -n "fn all_edits" crates/pipeline/src/lib.rs` and call it by whatever path it lives at. It is
the exhaustive fixture this file already uses for `PrefixKey`.

- [ ] **Step 5: run it, and sabotage it**

```bash
cargo test -p pipeline every_prefix_stages_output_dims 2>&1 | tail -5
```

Expected: PASS (`1 passed`). ⚠ Read the passed-count, not the word `ok` — a filter that matches
nothing prints `0 passed ... ok`.

Now sabotage: delete `GeometryStage::output_dims` so it falls back to the identity default.

```bash
cargo test -p pipeline every_prefix_stages_output_dims 2>&1 | tail -12
```

Expected: **FAIL**, naming `geometry` and two differing sizes. Restore the method (from a `cp`
backup taken first, never `git checkout` — this file records a `git checkout` destroying
uncommitted work) and re-run to green.

- [ ] **Step 6: gate and commit**

```bash
cargo fmt --all; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-$$.log 2>&1; echo "CLIPPY=$?"
cargo test -p pipeline 2>&1 | tail -3
git add -A crates/pipeline && git commit -m "feat: a stage can report its output size and produce a sub-rect of it"
```

⚠ Both exit codes must read 0 **before** the commit, each from its own command.

---

## Task 3: `GeometryStage::apply_region`

**Files:** Modify `crates/pipeline/src/stages/geometry.rs`

- [ ] **Step 1: `rotate` becomes a wrapper over `rotate_region`**

Replace the whole of `fn rotate` (currently `crates/pipeline/src/stages/geometry.rs:94-121`,
keeping its existing doc comment block above it) with:

```rust
fn rotate(src: &ImageBuf, angle_rad: f32, weights: &Weights) -> ImageBuf {
    rotate_region(
        src,
        angle_rad,
        weights,
        OutRect {
            x0: 0,
            y0: 0,
            w: src.width(),
            h: src.height(),
        },
    )
}

/// Rotate about the **full** image's centre, producing only `rect` of the result.
///
/// ⚠⚠ **`cx` and `cy` are the FULL output's centre, never the sub-rect's.** Using the sub-rect's
/// centre shifts the picture, and the result is a plausible photograph of the wrong part of the
/// frame — the one way to get this wrong.
///
/// ⚠⚠ **`rotate` is this function at full extent, which is what makes the two bit-identical by
/// construction rather than by comparison.** Two implementations that must agree is how they come
/// to disagree; there is only one here.
///
/// ⚠ Parallel over output rows, sound for the reason `rotate`'s own note gives: this is a gather,
/// so no two threads write the same byte and none reads another's output.
fn rotate_region(src: &ImageBuf, angle_rad: f32, weights: &Weights, rect: OutRect) -> ImageBuf {
    let w = src.width();
    let h = src.height();
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = angle_rad.sin_cos();
    const CH: usize = ImageBuf::CHANNELS;
    let stride = rect.w * CH;
    let mut data = vec![0.0f32; rect.w * rect.h * CH];
    // ⚠ `par_chunks_exact_mut(0)` PANICS, and a zero-width image reaches here. The same guard
    // `map_pixels_xy` and `LensCorrectionStage` already carry.
    if stride == 0 {
        return ImageBuf::from_vec(rect.w, rect.h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(ry, orow)| {
            let y = rect.y0 + ry;
            for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let x = rect.x0 + rx;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                // Inverse map: sample the source at -angle.
                let sx = cx + dx * cos + dy * sin;
                let sy = cy - dx * sin + dy * cos;
                *out = resample::sample(src, sx, sy, weights);
            }
        });
    ImageBuf::from_vec(rect.w, rect.h, data)
}
```

Add to the imports at the top of the file: `use crate::{OutRect, PipelineStage};` (replacing the
existing `use crate::PipelineStage;`).

- [ ] **Step 2: add `apply_region` to `impl PipelineStage for GeometryStage`**

After the `output_dims` method added in Task 2:

```rust
    fn apply_region(&self, src: &ImageBuf, rect: OutRect) -> Option<ImageBuf> {
        // ⚠ Declines on a non-identity orientation: `orient` is a whole-buffer index permutation
        // feeding the rotation's gather, so region-scoping it needs the permutation folded into
        // the sample coordinates. Uncommon — a camera's portrait orientation is applied at decode
        // through `sizes.flip`, not through `Edits::orientation`.
        if !self.orientation.is_identity() {
            return None;
        }
        let (w, h) = (src.width(), src.height());
        let (x0, y0, x1, y1) = crop_bounds(w, h, &self.crop);
        // ⚠ Refuse a rect this stage could not produce, rather than clamping it: a caller that
        // asked for the wrong thing must fall back to the whole-buffer path, not silently get a
        // different rectangle.
        if rect.w == 0 || rect.h == 0 || rect.x0 + rect.w > x1 - x0 || rect.y0 + rect.h > y1 - y0 {
            return None;
        }
        let out = OutRect {
            x0: x0 + rect.x0,
            y0: y0 + rect.y0,
            w: rect.w,
            h: rect.h,
        };
        if self.angle_rad == 0.0 {
            // No resampling at all, so a sub-rect of the crop is a sub-rect of the source — and
            // `apply`'s own guard already takes that branch for the whole image.
            return Some(src.crop(out.x0, out.y0, out.w, out.h));
        }
        Some(rotate_region(src, self.angle_rad, &self.weights, out))
    }
```

- [ ] **Step 3: write the bit-identity test**

In `geometry.rs`'s `#[cfg(test)] mod tests`:

```rust
    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut b = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // ⚠ A hard edge as well as a ramp: every kernel reproduces a linear ramp almost
                // exactly, so a ramp alone cannot tell a shifted sample from a correct one.
                let e = if x * 3 < w { 0.0 } else { 1.0 };
                b.set_pixel(x, y, [x as f32 / w as f32, y as f32 / h as f32, e]);
            }
        }
        b
    }

    /// ⚠⚠ The acceptance for the whole milestone: a region-scoped geometry must equal the
    /// whole-buffer one cropped, **byte for byte**. Both are the same gather in the same order, so
    /// a tolerance would hide exactly the failure this exists to catch — a rotation centred on the
    /// sub-rect instead of the frame, which produces a plausible photograph of the wrong place.
    #[test]
    fn a_region_of_the_geometry_matches_the_whole_geometry_cropped() {
        let src = ramp(61, 43);
        for g in [
            Geometry {
                angle: 7.0,
                ..Default::default()
            },
            Geometry {
                angle: 0.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.2,
                    right: 0.8,
                    bottom: 0.9,
                },
                ..Default::default()
            },
            Geometry {
                angle: -4.5,
                crop: CropRect {
                    left: 0.15,
                    top: 0.05,
                    right: 0.75,
                    bottom: 0.85,
                },
                ..Default::default()
            },
        ] {
            let stage = GeometryStage::new(&g, Resampler::default());
            let whole = stage.apply(src.clone());
            let (ow, oh) = stage.output_dims(src.width(), src.height());
            assert_eq!(
                (ow, oh),
                (whole.width(), whole.height()),
                "fixture check: output_dims must describe this stage"
            );
            let rect = OutRect {
                x0: 3,
                y0: 5,
                w: ow - 9,
                h: oh - 11,
            };
            let scoped = stage
                .apply_region(&src, rect)
                .expect("fixture check: this geometry must accept a region");
            let want = whole.crop(rect.x0, rect.y0, rect.w, rect.h);
            assert_eq!(
                (scoped.width(), scoped.height()),
                (want.width(), want.height())
            );
            assert_eq!(
                scoped.data(),
                want.data(),
                "angle {} : the region must be bit-identical to the whole cropped",
                g.angle
            );
        }
    }

    /// ⚠ An orientation is a whole-buffer permutation feeding the gather, so the stage must
    /// decline rather than produce a plausible-looking wrong rectangle.
    #[test]
    fn a_turned_or_mirrored_geometry_declines_a_region() {
        let src = ramp(40, 30);
        for o in [
            Orientation {
                rotate: Quarter::Cw90,
                mirror: false,
            },
            Orientation {
                rotate: Quarter::Zero,
                mirror: true,
            },
        ] {
            let stage = GeometryStage::new(
                &Geometry {
                    angle: 3.0,
                    orientation: o,
                    ..Default::default()
                },
                Resampler::default(),
            );
            assert!(
                stage
                    .apply_region(
                        &src,
                        OutRect {
                            x0: 0,
                            y0: 0,
                            w: 4,
                            h: 4
                        }
                    )
                    .is_none(),
                "{o:?} must decline"
            );
        }
    }

    /// ⚠ A rect this stage could not produce is refused, never clamped — a caller that asked for
    /// the wrong thing must fall back to the whole-buffer path rather than get a different
    /// rectangle back without being told.
    #[test]
    fn a_region_outside_the_output_is_refused() {
        let src = ramp(40, 30);
        let stage = GeometryStage::new(
            &Geometry {
                angle: 2.0,
                ..Default::default()
            },
            Resampler::default(),
        );
        let (ow, oh) = stage.output_dims(40, 30);
        for bad in [
            OutRect {
                x0: 0,
                y0: 0,
                w: ow + 1,
                h: oh,
            },
            OutRect {
                x0: ow - 1,
                y0: 0,
                w: 2,
                h: oh,
            },
            OutRect {
                x0: 0,
                y0: 0,
                w: 0,
                h: oh,
            },
        ] {
            assert!(stage.apply_region(&src, bad).is_none(), "{bad:?}");
        }
    }
```

⚠ If `ImageBuf::data()` is not public, compare with `assert_eq!(scoped, want)` — `ImageBuf` derives
`PartialEq`, which this codebase's tests already rely on. Check with
`grep -n "pub fn data" crates/core-image/src/lib.rs` first.

- [ ] **Step 4: run, and sabotage the centre**

```bash
cargo test -p pipeline --lib geometry 2>&1 | tail -6
```

Expected: all green, with the three new tests among them.

Now the sabotage that matters. In `rotate_region`, change

```rust
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
```

to use the sub-rect's centre instead:

```rust
    let cx = rect.x0 as f32 + (rect.w as f32 - 1.0) / 2.0;
    let cy = rect.y0 as f32 + (rect.h as f32 - 1.0) / 2.0;
```

```bash
cargo test -p pipeline 2>&1 | tail -8
```

Expected: **FAIL** on `a_region_of_the_geometry_matches_the_whole_geometry_cropped` **and** on the
goldens (the whole-image path goes through the same function). Restore from a `cp` backup.

- [ ] **Step 5: goldens and commit**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -3
git status --short tests/
cargo fmt --all; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-$$.log 2>&1; echo "CLIPPY=$?"
git add -A crates/pipeline && git commit -m "feat: geometry can render only a sub-rect of its output"
```

⚠ `git status --short tests/` must be **empty**.

---

## Task 4: `PerspectiveStage::apply_region`

**Files:** Modify `crates/pipeline/src/stages/perspective.rs`

- [ ] **Step 1: extract the loop, and make `apply` call it at full extent**

Replace `impl PipelineStage for PerspectiveStage`'s `apply` body so that the gather lives in one
private method both paths call. Keep every existing doc comment; the shape becomes:

```rust
impl PerspectiveStage {
    /// The gather, producing only `rect` of the output.
    ///
    /// ⚠⚠ **`cx`, `cy` and `aspect` are the FULL output's, never the sub-rect's** — the homography
    /// is defined about the frame's centre in units of its half-height, so a sub-rect's own centre
    /// would describe a different map and render a plausible photograph of the wrong place.
    ///
    /// ⚠⚠ `apply` is this at full extent, so the whole-buffer and region paths are bit-identical
    /// by construction rather than by comparison.
    fn render(&self, image: &ImageBuf, rect: OutRect) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        // Half-height is the unit, so `x` runs ±aspect and `y` runs ±1.
        let aspect = cx / cy;
        let hom = Homography::from_edits(&self.params, aspect);

        const CH: usize = ImageBuf::CHANNELS;
        let stride = rect.w * CH;
        let mut data = vec![0.0f32; rect.w * rect.h * CH];
        // ⚠ `par_chunks_exact_mut(0)` PANICS.
        if stride == 0 {
            return ImageBuf::from_vec(rect.w, rect.h, data);
        }
        data.par_chunks_exact_mut(stride)
            .enumerate()
            .for_each(|(ry, orow)| {
                let y = rect.y0 + ry;
                for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                    let x = rect.x0 + rx;
                    let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                    let s = hom.source_of(n);
                    *out = resample::sample(image, cx + s.0 * cy, cy + s.1 * cy, &self.weights);
                }
            });
        ImageBuf::from_vec(rect.w, rect.h, data)
    }
}
```

⚠ Copy the body of the existing `for_each` closure verbatim from
`crates/pipeline/src/stages/perspective.rs:81` onward rather than retyping it — the only changes are
`let y = rect.y0 + ry;`, `let x = rect.x0 + rx;`, and the sampler field name, which must match
whatever the existing code calls it (`&self.weights` or a local). Check first with
`sed -n '39,100p' crates/pipeline/src/stages/perspective.rs`.

Then `apply` becomes:

```rust
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        // ⚠⚠ The neutral case returns the image **untouched**, and that is not merely an
        // optimisation. The homography is the identity to within f32 rounding, not exactly — so
        // resampling through it shifts samples by ~1e-7 and bilinear interpolation then blends
        // neighbours, softening the whole frame for nothing. `GeometryStage` guards its rotation
        // and its crop the same way and for the same reason.
        //
        // ⚠ `build_prefix_steps` already skips an inactive stage, so this is unreachable in
        // production today. It is here so that constructing the stage unconditionally stays safe.
        if !self.params.is_active() || w < 2 || h < 2 {
            return image;
        }
        self.render(
            &image,
            OutRect {
                x0: 0,
                y0: 0,
                w,
                h,
            },
        )
    }
```

- [ ] **Step 2: add `apply_region`**

```rust
    fn apply_region(&self, src: &ImageBuf, rect: OutRect) -> Option<ImageBuf> {
        let (w, h) = (src.width(), src.height());
        // ⚠ Perspective preserves dimensions, so the output rect is bounded by the input's.
        if rect.w == 0 || rect.h == 0 || rect.x0 + rect.w > w || rect.y0 + rect.h > h {
            return None;
        }
        if !self.params.is_active() || w < 2 || h < 2 {
            // Mirrors `apply`'s neutral guard: the stage is the identity, so the region is a
            // plain sub-rect.
            return Some(src.crop(rect.x0, rect.y0, rect.w, rect.h));
        }
        Some(self.render(src, rect))
    }
```

Add `OutRect` to the file's `use crate::{...}` line.

- [ ] **Step 3: the bit-identity test**

In `perspective.rs`'s test module:

```rust
    /// ⚠ The same acceptance geometry's carries, for the same reason: both are gathers, so equality
    /// is byte-for-byte and a tolerance would hide a map centred on the wrong point.
    #[test]
    fn a_region_of_the_perspective_matches_the_whole_perspective_cropped() {
        let mut src = ImageBuf::new(61, 43);
        for y in 0..43 {
            for x in 0..61 {
                let e = if x * 3 < 61 { 0.0 } else { 1.0 };
                src.set_pixel(x, y, [x as f32 / 61.0, y as f32 / 43.0, e]);
            }
        }
        let p = edit_model::Perspective {
            vertical: 30.0,
            ..Default::default()
        };
        let stage = PerspectiveStage::new(p, Resampler::default());
        let whole = stage.apply(src.clone());
        let rect = OutRect {
            x0: 7,
            y0: 4,
            w: 21,
            h: 17,
        };
        let scoped = stage
            .apply_region(&src, rect)
            .expect("fixture check: an active perspective must accept a region");
        assert_eq!(
            scoped,
            whole.crop(rect.x0, rect.y0, rect.w, rect.h),
            "the region must be bit-identical to the whole cropped"
        );
    }
```

⚠ Match `PerspectiveStage::new`'s real signature — check with
`grep -n "fn new" -A 6 crates/pipeline/src/stages/perspective.rs`.

- [ ] **Step 4: run, sabotage, restore**

```bash
cargo test -p pipeline --lib perspective 2>&1 | tail -6
```

Sabotage: in `render`, use `rect.w`/`rect.h` for `cx`/`cy` instead of `w`/`h`. Expect the new test
**and** the goldens to fail. Restore from a `cp` backup.

- [ ] **Step 5: goldens and commit**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -3
git status --short tests/
cargo fmt --all; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-$$.log 2>&1; echo "CLIPPY=$?"
git add -A crates/pipeline && git commit -m "feat: perspective can render only a sub-rect of its output"
```

---

## Task 5: the gate in `render_cached`

**Files:** Modify `crates/app/src/render_engine.rs`

- [ ] **Step 1: add the observable to `Ran`**

In `crates/app/src/render_engine.rs`, add a field to `pub struct Ran` (at
`crates/app/src/render_engine.rs:1041`):

```rust
    /// Whether the prefix's last step produced only the region, rather than the whole frame.
    ///
    /// ⚠⚠ **An observable, because the two paths produce the same picture BY DESIGN.** A
    /// region-scoped render and a whole-buffer render cropped are bit-identical, so every
    /// equivalence test in the workspace passes against a gate that never fires — which is exactly
    /// how creative LUTs (W5) shipped fully tested and 100% dead. `prefix_stages`, `bytes` and
    /// `gpu_prefix_steps` all exist for this reason.
    pub region_scoped: bool,
```

Add `region_scoped,` to the `Ran { .. }` literal at `crates/app/src/render_engine.rs:1389`.

- [ ] **Step 2: replace the prefix-run block with the gated form**

Replace `crates/app/src/render_engine.rs:1130-1183` — the `for (i, step) in ...` loop through the
`let (input, region) = match rect { ... };` block — with:

```rust
    // ⚠⚠ **THE REGION-SCOPED PREFIX, and the gate is DERIVED rather than a flag.** Engage only
    // when *exactly* the last prefix step needs re-running — which is what a Straighten or
    // Perspective drag is, since every upstream checkpoint is keyed on edits the drag does not
    // touch. `render_cached` already knows it, so there is no drag signal to plumb and nothing
    // that can disagree with anything else.
    //
    // ⚠ Only the LAST step, and that is structural: step n-1's region-scoped output cannot feed
    // step n, because step n is a gather that reads anywhere in its source. Chaining would need
    // pre-image analysis per stage, which is the whole thing this avoids.
    //
    // ⚠ At the fitted rung `region` is `None`, so this never engages there.
    let mut scoped = None;
    // ⚠ A plain flag rather than something derived from the output's size afterwards: a derived
    // fact that can be wrong is what `Session::geometry_mode` was deleted for.
    let mut was_scoped = false;
    if start + 1 == steps.len() {
        if let Some(want) = region {
            let last = &steps[start].stage;
            let (ow, oh) = last.output_dims(img.width(), img.height());
            if let Some(r) = want.clamped(ow, oh) {
                if let Some(out) = last.apply_region(
                    &img,
                    pipeline::OutRect {
                        x0: r.x,
                        y0: r.y,
                        w: r.w,
                        h: r.h,
                    },
                ) {
                    scoped = Some((out, r, ow, oh));
                    was_scoped = true;
                }
            }
        }
    }

    let (input, rect, pw, ph, prefix_stages) = if let Some((out, r, ow, oh)) = scoped {
        // ⚠⚠ **DELIBERATELY NOT CACHED.** A region-scoped checkpoint served for a *different*
        // region is the one failure here that shows as a plausible photograph rather than an
        // error, and leaving the insert out is what makes it unrepresentable rather than guarded.
        // The cost is that a later whole-buffer render rebuilds the final checkpoint — against
        // re-running the step whole-buffer on every frame of the drag, which is what this replaces.
        (out, Some(r), ow, oh, 1)
    } else {
        // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU seam
        // clean — so each step that runs clones the checkpoint it starts from. Measured at 0.5 ms
        // (fit) and 2.1 ms (native) against the 13–117 ms each avoided stage costs.
        for (i, step) in steps.iter().enumerate().skip(start) {
            // ⚠⚠ **The prefix's GPU seam, and it must be here rather than where `steps` is built.**
            // `dehaze::airlight` is estimated from the image the stage is HANDED — the checkpoint
            // after geometry and noise reduction — so substituting up front would estimate it from
            // the raw buffer, before the crop. A slightly different airlight is a different
            // photograph, not an error.
            //
            // ⚠ The step's `key` is never touched: `PrefixStepKey` is derived from the edits, so a
            // checkpoint written by either path is a legitimate hit for the other and `PrefixCache`
            // needs no change at all.
            let accelerated = if gpu_tail {
                gpu_pipeline::shared().and_then(|ctx| {
                    gpu_pipeline::accelerate_prefix_stage(step.stage.name(), edits, scale, &img, ctx)
                })
            } else {
                None
            };
            img = Arc::new(match accelerated {
                Some(g) => {
                    gpu_prefix_steps += 1;
                    g.apply((*img).clone())
                }
                None => step.stage.apply((*img).clone()),
            });
            cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
        }
        let ran = steps.len() - start;

        // ⚠ The complete prefix output must always live in the protected tier, even when we
        // resumed at it without running anything. A prefix that *shrinks* — its last stage going
        // neutral — turns a former intermediate into the final, and leaving it in the budgeted
        // tier would let a large render evict the very thing the zoom guarantee rests on.
        // `steps.is_empty()` is excluded because there the final *is* checkpoint 0, which is
        // either free (and must not be stored) or was already inserted as a final above.
        if ran == 0 && !steps.is_empty() {
            cache.insert(key_at(steps.len()), img.clone(), Tier::Final);
        }

        // The region can only be resolved once the prefix output's dimensions are known, which is
        // why the tail is built here rather than alongside the steps. Building is a few Box
        // allocations.
        let (pw, ph) = (img.width(), img.height());
        let rect = region.and_then(|r| r.clamped(pw, ph));
        let input = match rect {
            Some(r) => img.crop(r.x, r.y, r.w, r.h),
            None => (*img).clone(),
        };
        (input, rect, pw, ph, ran)
    };
    let region_scoped = was_scoped;

    // ⚠⚠ **The frame dimensions are `(pw, ph)` — the prefix output's — and on the region-scoped
    // path they come from `output_dims` rather than from the buffer.** That buffer *is* the region
    // there, so taking its size would rasterise every brush mask at the region's size and thrash
    // the coverage cache on every pan.
    let region = rect.map(|r| Region {
        full_w: pw,
        full_h: ph,
        x0: r.x,
        y0: r.y,
    });
```

⚠ `clip` (the `let clip = rect.map(|r| (r.x, r.y, r.w, r.h));` line just below this block) is
**unchanged** and must stay: `rect` is still in scope with the same meaning on both paths, and it
carries the region's own width and height, which `Region` does not.

⚠ `gpu_prefix_steps` stays 0 on the scoped path and nothing is lost: `accelerate_prefix_stage` has
twins only for dehaze and noise reduction, neither of which can be the region-scoped step.

- [ ] **Step 3: two guard tests**

In `crates/app/src/render_engine.rs`'s test module:

```rust
    fn region_spec(max_dim: usize, w: usize, h: usize) -> FrameSpec {
        FrameSpec {
            region: Some(RegionRect { x: 1, y: 1, w, h }),
            ..FrameSpec::fit(max_dim)
        }
    }

    /// ⚠⚠ The feature itself. Every pixel test passes against a gate that never fires, because a
    /// region-scoped render and a whole-buffer one cropped are bit-identical by design.
    #[test]
    fn a_drag_of_the_last_prefix_step_renders_only_the_region() {
        let buf = varied();
        let base = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let spec = region_spec(16, 6, 5);
        let call = |e: &Edits, cache: &mut PrefixCache, bc: &mut BrushCache| {
            render_cached(
                &buf,
                e,
                spec,
                1.0,
                None,
                color::wb::Illuminant::default(),
                pipeline::ProfileRendering::default(),
                cache,
                bc,
            )
        };

        let (_, cold) = call(&base, &mut cache, &mut bc);
        assert_eq!(cold.prefix_stages, 1, "fixture check: one prefix step");

        // A Straighten drag: the same prefix, a different angle. Exactly the last step re-runs.
        let dragged = Edits {
            geometry: edit_model::Geometry {
                angle: 3.4,
                ..Default::default()
            },
            ..Edits::default()
        };
        let (frame, ran) = call(&dragged, &mut cache, &mut bc);
        assert!(ran.region_scoped, "the drag must render only the region");
        assert_eq!(ran.prefix_stages, 1);
        assert!(frame.width > 0 && frame.height > 0);
    }

    /// ⚠⚠ A region-scoped checkpoint served for a *different* region is the one failure that shows
    /// as a plausible photograph. Not inserting it is what makes that unrepresentable.
    #[test]
    fn a_region_scoped_render_leaves_no_final_checkpoint() {
        let buf = varied();
        let e = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let spec = region_spec(16, 6, 5);

        // Warm the checkpoint before geometry by rendering the whole image once.
        let (_, _) = render_cached(
            &buf,
            &e,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        let mut cache2 = PrefixCache::default();
        let (_, ran) = render_cached(
            &buf,
            &e,
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache2,
            &mut bc,
        );
        // ⚠ A cold cache runs the downscale then geometry, so `start + 1 == len` holds and the
        // gate fires; what matters is that nothing was left behind for a later region to reuse.
        if ran.region_scoped {
            let (_, again) = render_cached(
                &buf,
                &e,
                spec,
                1.0,
                None,
                color::wb::Illuminant::default(),
                pipeline::ProfileRendering::default(),
                &mut cache2,
                &mut bc,
            );
            assert!(
                again.region_scoped,
                "no final checkpoint may have been stored, so the next render scopes again"
            );
        }
    }

    /// ⚠⚠ The hazard of §5: the frame's dimensions must come from `output_dims`, never from the
    /// region-scoped buffer. Taking them off the buffer rasterises every mask at the region's size.
    #[test]
    fn a_region_scoped_render_reports_the_whole_frames_size() {
        let buf = varied();
        let e = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();
        let mut bc = BrushCache::default();
        let whole = render_cached(
            &buf,
            &e,
            FrameSpec::fit(16),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        )
        .0;
        let (frame, ran) = render_cached(
            &buf,
            &e,
            region_spec(16, 6, 5),
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut bc,
        );
        assert!(ran.region_scoped, "fixture check: the gate must have fired");
        assert_eq!(
            frame.fit_size, whole.fit_size,
            "the frame's own size must not shrink to the region's"
        );
    }
```

⚠ `varied()` and `FrameSpec::fit` are existing test helpers in that module — check their names with
`grep -n "fn varied\|fn full_prefix\|fn byte_spec" crates/app/src/render_engine.rs`. Adjust the
fixtures so the gate genuinely fires: the buffer must be large enough that `region_spec`'s rect is a
strict sub-rect of the geometry output. If `varied()` is too small, build a larger local buffer.

- [ ] **Step 4: the master invariant still holds**

```bash
cargo test -p pipeline a_region_render_matches_the_whole_render_cropped 2>&1 | tail -4
cargo test -p app --lib render_engine 2>&1 | tail -6
```

Both must pass. ⚠ Read the passed-counts.

- [ ] **Step 5: sabotage the gate, then the dims**

Sabotage A — force `scoped = None` (change `if start + 1 == steps.len()` to `if false`). Expect
`a_drag_of_the_last_prefix_step_renders_only_the_region` and
`a_region_scoped_render_reports_the_whole_frames_size` to **fail**.

Sabotage B — take the frame dims off the buffer on the scoped path (`(out.width(), out.height())`
instead of `(ow, oh)`). Expect `a_region_scoped_render_reports_the_whole_frames_size` to **fail**.

Sabotage C — insert the scoped output into the cache under `key_at(steps.len())`. Expect
`a_region_scoped_render_leaves_no_final_checkpoint` to **fail**.

Restore from `cp` backups after each. ⚠ Confirm each sabotage produced a `test result:` line — a
sabotage that fails to compile reads exactly like a pass.

- [ ] **Step 6: full gate and commit**

```bash
cargo fmt --all; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-$$.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t-$$.log 2>&1; echo "TEST=$?"
grep -c "^test golden_" /tmp/t-$$.log
git status --short tests/
git add -A crates/app && git commit -m "perf: the prefix's last step renders only the region during a drag"
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`; **63 goldens ran**; `git status` over `tests/` **empty**.

---

## Task 6: measure the real thing

**Files:** Modify `crates/app/examples/probe_resamplers.rs`

- [ ] **Step 1: add a region-scoped drag row**

Inside the drag block of `crates/app/examples/probe_resamplers.rs` (the `{ use pipeline::{...} }`
block near the end), after the existing loop, add:

```rust
        // ⚠⚠ The milestone's own row: the same Straighten drag, with the last prefix step
        // producing only the region. A projection is not a result — this crate has had three
        // composed projections miss.
        {
            use pipeline::OutRect;
            let last = dsteps.len() - 1;
            let before = &checkpoints[last];
            let (ow, oh) = dsteps[last].stage.output_dims(before.width(), before.height());
            let rect = OutRect {
                x0: 0,
                y0: 0,
                w: rw.min(ow),
                h: rh.min(oh),
            };
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let sub = dsteps[last]
                    .stage
                    .apply_region(before, rect)
                    .expect("fixture check: the last step must accept a region");
                std::hint::black_box(tail.run(sub));
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{best:8.1} ms   a `geometry` drag, REGION-SCOPED");
        }
```

- [ ] **Step 2: run it at both rungs**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
uptime
cargo run --release -p app --example probe_resamplers ~/Pictures/fuji/DSCF6296.RAF -- --full
cargo run --release -p app --example probe_resamplers ~/Pictures/fuji/DSCF6296.RAF
```

⚠ Record the load average beside the numbers, as every bench in `CLAUDE.md` does. ⚠ Run it three
times and quote a **band**, not a point — M8.3h had to retract a headline for exactly that.

- [ ] **Step 3: commit**

```bash
cargo fmt --all; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-$$.log 2>&1; echo "CLIPPY=$?"
git add crates/app/examples/probe_resamplers.rs
git commit -m "bench: price a region-scoped straighten drag"
```

---

## Task 7: the record

**Files:** Modify `CLAUDE.md`

- [ ] **Step 1: write the section**

Add an **M8.6** section immediately above the M8.5 one, carrying: the measured band from Task 6;
⚠⚠ **lens correction contributes zero to a Straighten drag, so item 3 does not touch this
symptom**; the derived gate; ⚠⚠ the frame-dims-from-the-prediction hazard; the never-cached rule;
bit-identity by construction (`rotate` *is* `rotate_region` at full extent); and all seven accepted
caveats from §8 of the spec.

Update the **▶ RESUME HERE** block: the next action becomes **port lens correction** (item 3) with
the note that it serves the cold render and the rung change rather than the straighten drag, and the
"NOT YET CONFIRMED BY USE" check is **a Straighten drag at 1:1, where the picture must not change**.

⚠ Add both new documents to the `docs (docs/superpowers/)` list at the foot of the file.

- [ ] **Step 2: commit and launch the GUI**

```bash
git add CLAUDE.md && git commit -m "docs: record M8.6, a region-scoped prefix"
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

⚠ Launch it **in the background** and say what to look at: a **Straighten drag at 1:1**, which
should settle from ~7 fps to roughly 38, and ⚠⚠ **the picture must not change** — this is
bit-identical, so anything that *looks* different is a bug rather than an improvement.

⚠ **Do not merge and do not push.** The user decides.

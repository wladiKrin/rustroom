# M8.8 — a region-scoped lens correction: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠⚠ **Conventions that are not negotiable.** Commit messages carry **no `Co-Authored-By`
> trailer**. Work on the existing branch `m8.8-lens-correction-scoped`. **Never push.** Read
> clippy's exit code from its own command, never through a pipe, and gate every commit with `&&`.
> **Never run two gates concurrently** — the 7 unfixed temp-dir sites interleave across processes.

**Goal:** `LensCorrectionStage` implements the `input_rect_for` / `apply_region` pair, so
`region_plan` stops abandoning at step 0 and a cold entry into 1:1 scopes the whole prefix chain.

**Architecture:** A frame-global displacement bound on `Corrector`, a region-aware per-channel
sampler in `resample`, and the stage refactored so `apply` **is** `apply_region` at full extent —
the `rotate` / `rotate_region` precedent, which is what makes the two bit-identical by construction
rather than by comparison.

**Tech stack:** Rust, `lens-profile` (pure-Rust lensfun port), `pipeline`, rayon.

**Spec:** `docs/superpowers/specs/2026-09-15-lens-correction-scoped-design.md`

---

## File structure

| file | change |
|---|---|
| `crates/lens-profile/src/lib.rs` | `Corrector::{max_displacement, margin}` + the exhaustive test |
| `crates/pipeline/src/resample.rs` | `sample_channel_region` |
| `crates/pipeline/src/stages/lens_correction.rs` | `render` extracted, `apply` delegates, then the pair + tests |
| `CLAUDE.md` | the milestone's record and the ▶ NEXT ACTION block |

No other file changes. `output_dims` keeps its identity default, `PrefixStepKey` is untouched, the
gate in `render_engine` is untouched, and no golden moves.

---

### Task 1: the frame-global displacement bound

**Files:**
- Modify: `crates/lens-profile/src/lib.rs`
- Test: same file, `mod tests`

- [ ] **Step 1: Write the failing test**

Add a sized corrector helper beside the existing `corrector`, and the exhaustive guarantee.
Replace the body of `fn corrector` so both read one construction:

```rust
    fn corrector(distortion: &str, want: Corrections) -> Corrector {
        corrector_sized(distortion, want, 400, 300)
    }

    fn corrector_sized(distortion: &str, want: Corrections, w: usize, h: usize) -> Corrector {
        let db = synthetic_db(distortion, TCA, VIGNETTING);
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        Corrector::new(
            &p,
            &Shot {
                focal: 50.0,
                aperture: 2.8,
            },
            w,
            h,
            want,
        )
        .unwrap()
    }
```

Then the test:

```rust
    /// ⚠⚠ **The margin's guarantee is this test, not an argument.** `max_displacement` samples at
    /// most `SAMPLE_ROWS` rows, so nothing about its own construction proves it bounds the rows it
    /// did not look at. This scans **every** pixel and **all three channels** of a real-sized
    /// frame — three, because TCA gives R, G and B different source points and the margin must
    /// bound the worst of them.
    ///
    /// ⚠ Real-sized on purpose: at 1600x1067 the sampling step is 5 rows, so a step that grew
    /// carelessly would miss peaks this test can see. A 400x300 fixture samples every row and
    /// could witness nothing.
    ///
    /// ⚠ Both distortion signs, because the bound is a magnitude and a barrel correction displaces
    /// inward where a pincushion one displaces outward.
    #[test]
    fn no_pixel_displaces_further_than_the_margin() {
        const PAD: usize = 2;
        for distortion in [BARREL, PINCUSHION] {
            let (w, h) = (1600usize, 1067usize);
            let c = corrector_sized(distortion, ALL, w, h);
            let margin = c.margin(PAD) as f32;
            let mut map = RowMap::new(w);
            let mut worst = 0.0f32;
            for y in 0..h {
                c.fill_row(y, &mut map);
                for x in 0..w {
                    for sc in map.source(x).iter() {
                        worst = worst.max((sc[0] - x as f32).hypot(sc[1] - y as f32));
                    }
                }
            }
            // Fixture check: a correction that moves nothing cannot witness a margin at all.
            assert!(
                worst > 1.0,
                "fixture check: the correction must actually displace pixels, got {worst}"
            );
            assert!(
                worst <= margin,
                "a pixel displaced {worst} px, past a margin of {margin}"
            );
        }
    }
```

- [ ] **Step 2: Run it and watch it fail to compile**

Run: `cargo test -p lens-profile no_pixel_displaces -- --exact 2>&1 | tail -20`
Expected: `error[E0599]: no method named \`margin\` found for struct \`Corrector\``

- [ ] **Step 3: Implement the bound**

Add the field to the struct (after `autoscale`):

```rust
    autoscale: f32,
    max_disp: f32,
```

Initialise it in `new`'s struct literal with `max_disp: 0.0`, and fill it **after**
`applied_scale` is set — `fill_row` reads `applied_scale`, so computing this earlier would measure
a different map:

```rust
        c.autoscale = c.compute_autoscale();
        c.applied_scale = 1.0 + (c.autoscale - 1.0) * (want.scale.clamp(0.0, 100.0) / 100.0);
        // ⚠ After `applied_scale`, never before: `fill_row` reads it, so a bound computed here
        // would describe a map this corrector does not have.
        c.max_disp = c.compute_max_displacement();
        Some(c)
```

Then the two public methods and the private computation, beside `compute_autoscale`:

```rust
    /// The largest distance any output pixel's source sits from the pixel itself, in pixels.
    ///
    /// ⚠⚠ **FRAME-GLOBAL on purpose, and that is the one real hazard in region-scoping this
    /// stage.** `PerspectiveStage::input_rect_for` bounds an output rect by its four mapped
    /// corners, which is *exact* for a homography because a homography sends lines to lines. **A
    /// radial map bows every edge**, so the image of a rectangle is a curved quadrilateral whose
    /// extreme can sit at an edge midpoint — a corner bound would under-estimate, and the failure
    /// is a dark band at one edge of the region rather than an error. No pixel can move further
    /// than the furthest-moving pixel, whatever shape the rect is.
    ///
    /// ⚠ Sampled through [`Self::fill_row`] over at most `SAMPLE_ROWS` rows, every column of each,
    /// so only the *vertical* sampling is coarse. The field is smooth — 50.6 px of displacement
    /// develops over ~3700 px of radius on the reference frame, about 0.014 px per px — so a step
    /// of tens of pixels costs a fraction of a pixel, and [`Self::margin`] adds a whole pixel on
    /// top. `no_pixel_displaces_further_than_the_margin` is what makes that a guarantee.
    pub fn max_displacement(&self) -> f32 {
        self.max_disp
    }

    /// The margin a region-scoped render must grow its request by, for a resampler of tap radius
    /// `pad`.
    ///
    /// ⚠⚠ **One definition, read by `LensCorrectionStage::input_rect_for` and by the test that
    /// guarantees it.** Two derivations of a margin is how a bound and the thing it bounds come to
    /// disagree — the shape `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were
    /// each deleted for — and here they disagree as a band at the region's edge.
    ///
    /// ⚠ The extra pixel is slack for [`Self::max_displacement`]'s vertical sampling, not
    /// decoration. It costs 0.1% of the frame on the region that matters.
    pub fn margin(&self, pad: usize) -> usize {
        self.max_disp.ceil().max(0.0) as usize + 1 + pad
    }

    /// Sample the displacement field and take the largest magnitude.
    ///
    /// ⚠ Bounded at `SAMPLE_ROWS` rows rather than a fixed step, so the cost does not grow with
    /// the frame: ~256 `fill_row` calls is ~6% of the field at 26 Mpx, and the field is 5% of the
    /// stage. The last row is always included, so the bottom border is exact.
    fn compute_max_displacement(&self) -> f32 {
        const SAMPLE_ROWS: usize = 256;
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return 0.0;
        }
        let step = h.div_ceil(SAMPLE_ROWS).max(1);
        let mut map = RowMap::new(w);
        let mut worst = 0.0f32;
        let mut y = 0usize;
        loop {
            self.fill_row(y, &mut map);
            for x in 0..w {
                for sc in map.source(x).iter() {
                    worst = worst.max((sc[0] - x as f32).hypot(sc[1] - y as f32));
                }
            }
            if y == h - 1 {
                break;
            }
            y = (y + step).min(h - 1);
        }
        worst
    }
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p lens-profile 2>&1 | tail -5`
Expected: every `lens-profile` test passes, including the new one.

- [ ] **Step 5: Sabotage — a corner bound would under-estimate**

Temporarily replace `compute_max_displacement`'s body with a four-corner probe:

```rust
        let mut worst = 0.0f32;
        for (x, y) in [(0usize, 0usize), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            for sc in self.source(x as f32, y as f32).iter() {
                worst = worst.max((sc[0] - x as f32).hypot(sc[1] - y as f32));
            }
        }
        worst
```

Run: `cargo test -p lens-profile no_pixel_displaces 2>&1 | tail -20`
Expected: **PASS or FAIL is the finding.** For a purely radial map about the frame centre the
corners *are* the maximum radius, so this may well pass — record the result honestly rather than
counting it. The sabotage that must fail is the next one.

⚠ **Take a `cp` backup of the file before sabotaging and restore from it — never `git checkout`.**

- [ ] **Step 6: Sabotage — drop the slack and the ceiling**

Replace `margin`'s body with `self.max_disp as usize + pad` (truncating, no slack).

Run: `cargo test -p lens-profile no_pixel_displaces 2>&1 | tail -20`
Expected: FAIL — `a pixel displaced 50.59 px, past a margin of 52` or similar.

Restore from the backup and confirm the test passes again.

- [ ] **Step 7: Commit**

```bash
cd ~/Projects/rustroom
cargo fmt --all && cargo clippy -p lens-profile --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Only when `CLIPPY=0`:

```bash
git add crates/lens-profile/src/lib.rs
git commit -m "feat(lens): a frame-global displacement bound, and the margin built from it

A radial map bows every edge, so PerspectiveStage's four-corner bound is
unsound here: the image of a rectangle is a curved quadrilateral whose
extreme can sit at an edge midpoint, and an under-estimate shows as a
dark band at one edge of a scoped region. The bound that IS sound is the
largest displacement anywhere in the frame.

Corrector::margin is the one definition the stage and the test both read.
no_pixel_displaces_further_than_the_margin scans every pixel and all
three channels of a real-sized frame, which is what makes the sampled
maximum a guarantee rather than an estimate."
```

---

### Task 2: a region-aware per-channel sampler

**Files:**
- Modify: `crates/pipeline/src/resample.rs`
- Test: same file, `mod tests`

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠ The mirror of `the_region_sampler_reads_full_image_coordinates` for the per-channel entry
    /// point. Coordinates are the **full image's**, so a sampler taking buffer coordinates would
    /// silently shift every read by the region's origin — a plausible photograph of the wrong part
    /// of the frame.
    #[test]
    fn the_per_channel_region_sampler_reads_full_image_coordinates() {
        let mut src = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                src.set_pixel(x, y, [x as f32, y as f32, 0.5]);
            }
        }
        let whole = crate::RegionSrc::whole(&src);
        let w = Weights::new(Resampler::Bilinear);
        // A slice of the same image, told where it sits.
        let slice = src.crop(2, 3, 4, 4);
        let part = crate::RegionSrc {
            buf: &slice,
            origin: (2, 3),
            full: (8, 8),
        };
        for ch in 0..3 {
            let a = sample_channel_region(&whole, 3.25, 4.5, ch, &w);
            let b = sample_channel_region(&part, 3.25, 4.5, ch, &w);
            assert!(
                (a - b).abs() < 1e-6,
                "channel {ch}: whole {a} against region {b}"
            );
        }
    }
```

- [ ] **Step 2: Run it and watch it fail to compile**

Run: `cargo test -p pipeline the_per_channel_region_sampler 2>&1 | tail -20`
Expected: `error[E0425]: cannot find function \`sample_channel_region\``

- [ ] **Step 3: Implement it**

Immediately after `sample_channel`:

```rust
/// Sample **one** channel of a region-scoped source at a point in **full-image** coordinates.
///
/// ⚠ The per-channel mirror of [`sample_region`], and it exists for the same reason that one does:
/// transverse chromatic aberration gives each channel its own source point, so lens correction
/// cannot go through the three-channel entry point.
///
/// ⚠⚠ **Coordinates are the FULL image's, not the buffer's**, and a tap outside the buffer reads
/// black exactly as [`sample`] does outside the frame — correct **because the margin makes it
/// unreachable**. A tap that does escape is a bug in some `input_rect_for`, and it shows up as a
/// dark band at the region's edge.
pub fn sample_channel_region(
    src: &crate::RegionSrc<'_>,
    sx: f32,
    sy: f32,
    ch: usize,
    w: &Weights,
) -> f32 {
    sample_channel(
        src.buf,
        sx - src.origin.0 as f32,
        sy - src.origin.1 as f32,
        ch,
        w,
    )
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p pipeline resample 2>&1 | tail -5`
Expected: all pass.

- [ ] **Step 5: Sabotage**

Change the two subtractions to additions. Run the test; expected FAIL with the two values
differing. Restore from a `cp` backup.

- [ ] **Step 6: Commit**

```bash
cd ~/Projects/rustroom
cargo fmt --all && cargo clippy -p pipeline --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Only when `CLIPPY=0`:

```bash
git add crates/pipeline/src/resample.rs
git commit -m "feat(resample): a region-aware per-channel sampler

The per-channel mirror of sample_region, and it exists for the reason
that one does: TCA gives each channel its own source point, so lens
correction cannot go through the three-channel entry point."
```

---

### Task 3: `apply` becomes `render` at full extent (pure refactor)

**Files:**
- Modify: `crates/pipeline/src/stages/lens_correction.rs`

⚠⚠ **This task changes no behaviour, and its acceptance is that the existing tests and all 63
goldens pass unchanged.** Doing it as its own commit is what makes that checkable.

- [ ] **Step 1: Extract `render`**

Replace `impl PipelineStage for LensCorrectionStage`'s `apply` and add a private `render`:

```rust
impl PipelineStage for LensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        // Nothing enabled, or nothing this profile can supply: pass the image through untouched.
        let Some(corrector) = self.corrector(w, h) else {
            return image;
        };
        let src = RegionSrc::whole(&image);
        self.render(&src, &corrector, Rect { x: 0, y: 0, w, h })
    }
}

impl LensCorrectionStage {
    /// Produce `rect` of the correction's output.
    ///
    /// ⚠⚠ **[`Self::apply`] is this function at full extent, which is what makes the whole-buffer
    /// and region paths bit-identical BY CONSTRUCTION rather than by comparison.** Two
    /// implementations that must agree is how they come to disagree; there is only one here.
    /// `geometry::rotate` already follows exactly this rule, and for the same reason.
    ///
    /// ⚠⚠ **The field is always filled from x = 0, never from the rect.** `fill_row` reaches
    /// lensfun's `apply_geometry_distortion`, which computes each `x` as `(xu + ns * i as f64) as
    /// f32` — **from scratch per `i`, not incrementally**. Starting the row at `rect.x` therefore
    /// re-associates an f64 addition, and f64 addition is not associative: the discrepancy is
    /// ~1e-16 against an f32 spacing of ~1.2e-7, so about one pixel in 1e9 could round to a
    /// different f32. A faint band that appears one pixel in a billion is precisely the class of
    /// thing this project has paid for. Filling from zero costs ~2% of a region render — the field
    /// is 5% of the stage, and only the prefix of each row is wasted — and it is bit-identical.
    fn render(&self, src: &RegionSrc<'_>, corrector: &Corrector, rect: Rect) -> ImageBuf {
        const CH: usize = ImageBuf::CHANNELS;
        // ⚠ The FULL image's width, never the slice in hand: `fill_row` is indexed in frame
        // coordinates and the corrector was built against the frame.
        let field_w = src.full.0;
        let stride = rect.w * CH;
        let mut data = vec![0.0f32; rect.w * rect.h * CH];
        if stride == 0 {
            // `par_chunks_exact_mut(0)` panics.
            return ImageBuf::from_vec(rect.w, rect.h, data);
        }
        // ⚠ Parallel over **output** rows, which is sound because this is a gather: an output
        // pixel reads the whole source and writes only itself. Bit-identical to the sequential
        // form, which is what lets the goldens stand as the check on it.
        //
        // ⚠ `for_each_init` rather than `for_each`: the initialiser runs once per rayon job, so
        // one `RowMap` is allocated per worker and reused down its rows. Allocating per row
        // would mean zeroing the scratch millions of times over a full-resolution render.
        data.par_chunks_exact_mut(stride).enumerate().for_each_init(
            || RowMap::new(field_w),
            |map, (ry, orow)| {
                let y = rect.y + ry;
                corrector.fill_row(y, map);
                for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                    let x = rect.x + rx;
                    let s = map.source(x);
                    let gain = map.gain(x);
                    for (ch, p) in out.iter_mut().enumerate() {
                        // ⚠ Per channel, because TCA gives each channel its own source point —
                        // which is why `resample` has a second entry point at all.
                        *p = resample::sample_channel_region(
                            src,
                            s[ch][0],
                            s[ch][1],
                            ch,
                            &self.weights,
                        ) * gain;
                    }
                }
            },
        );
        ImageBuf::from_vec(rect.w, rect.h, data)
    }
}
```

Update the imports at the top of the file:

```rust
use crate::resample::{self, Resampler, Weights};
use crate::{PipelineStage, Rect, RegionSrc};
use core_image::ImageBuf;
use lens_profile::{Corrections, Corrector, LensShot, RowMap};
use rayon::prelude::*;
```

- [ ] **Step 2: Run the stage's tests and the goldens**

Run: `cargo test -p pipeline 2>&1 | tail -5`
Expected: all pass, including all 63 `golden_*`.

- [ ] **Step 3: Confirm no golden moved**

Run: `git status --short crates/pipeline/tests`
Expected: **empty**. A golden is rewritten only under `UPDATE_GOLDEN=1`, so an unmodified tree plus
a green run is what establishes byte-identity.

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/rustroom
cargo fmt --all && cargo clippy -p pipeline --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Only when `CLIPPY=0`:

```bash
git add crates/pipeline/src/stages/lens_correction.rs
git commit -m "refactor(lens): apply is render at full extent

geometry::rotate's rule, for geometry::rotate's reason: two
implementations that must agree is how they come to disagree, and there
is only one here. No behaviour moves — the stage's own tests and all 63
goldens pass unchanged, and the tree is unmodified under tests/."
```

---

### Task 4: the `input_rect_for` / `apply_region` pair

**Files:**
- Modify: `crates/pipeline/src/stages/lens_correction.rs`
- Test: same file, `mod tests`

- [ ] **Step 1: Write the failing equivalence test**

Add to the stage's `mod tests`, after the existing resampler test:

```rust
    /// ⚠⚠ **Byte for byte, and that is achievable here where a chain carrying a local is bounded
    /// to one ulp.** Lens correction is a pure gather, and `apply` *is* `apply_region` at full
    /// extent — so anything but exact equality means the field was computed differently for the
    /// two, which is the one way this can go wrong.
    #[test]
    fn a_region_matches_the_same_part_of_the_whole() {
        let src = grid(160, 120);
        let want = Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        };
        let stage = LensCorrectionStage::new(lens_shot(), want, Resampler::CatmullRom);
        let whole = stage.apply(src.clone());
        // Fixture check: a stage that did nothing would make this test vacuous.
        assert_ne!(whole.data(), src.data(), "the correction must move pixels");

        for rect in [
            Rect {
                x: 40,
                y: 30,
                w: 48,
                h: 36,
            },
            // Against the frame's edge, where the margin clips and the taps run out.
            Rect {
                x: 0,
                y: 0,
                w: 32,
                h: 24,
            },
        ] {
            let need = stage
                .input_rect_for(rect, 160, 120)
                .expect("lens correction accepts a region");
            let slice = src.crop(need.x, need.y, need.w, need.h);
            let region_src = RegionSrc {
                buf: &slice,
                origin: (need.x, need.y),
                full: (160, 120),
            };
            let got = stage
                .apply_region(&region_src, rect)
                .expect("apply_region agrees with input_rect_for");
            assert_eq!((got.width(), got.height()), (rect.w, rect.h));
            for ry in 0..rect.h {
                for rx in 0..rect.w {
                    assert_eq!(
                        got.pixel(rx, ry),
                        whole.pixel(rect.x + rx, rect.y + ry),
                        "rect {rect:?} at ({rx}, {ry})"
                    );
                }
            }
        }
    }
```

Add `use crate::{Rect, RegionSrc};` to the test module if it is not already reachable through
`use super::*;` — it is, because the production imports carry both.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p pipeline a_region_matches_the_same_part 2>&1 | tail -20`
Expected: FAIL at `lens correction accepts a region` — the trait's `None` default.

- [ ] **Step 3: Implement the pair**

Add to `impl PipelineStage for LensCorrectionStage`, after `apply`:

```rust
    /// ⚠⚠ **The margin is frame-global, never a bound on this rect's four corners.** A homography
    /// sends lines to lines, so `PerspectiveStage` can map four corners and be exact; **a radial
    /// map bows every edge**, so the extreme can sit at an edge midpoint and a corner bound would
    /// under-estimate — a dark band at one edge of the region, which is a plausible photograph
    /// rather than an error. `Corrector::margin` is the one definition, and
    /// `no_pixel_displaces_further_than_the_margin` is its guarantee.
    ///
    /// ⚠ The tap radius comes from `self.weights.kernel()` rather than a stored `Resampler`, so
    /// the weights and the radius cannot disagree. Geometry's and perspective's rule.
    ///
    /// ⚠ Dimension-preserving, so `out` is already in the input's coordinate space and
    /// `output_dims` keeps the trait's identity default.
    fn input_rect_for(&self, out: Rect, in_w: usize, in_h: usize) -> Option<Rect> {
        if out.w == 0 || out.h == 0 || in_w == 0 || in_h == 0 {
            return None;
        }
        if out.x + out.w > in_w || out.y + out.h > in_h {
            // Refuse a rect this stage could not produce, rather than clamping it: a caller that
            // asked for the wrong thing must fall back to the whole-buffer path, not silently get
            // a different rectangle back without being told. Geometry's rule.
            return None;
        }
        let Some(corrector) = self.corrector(in_w, in_h) else {
            // Nothing enabled: an exact identity, so the rect is its own pre-image. The same
            // branch `apply` takes when it passes the image through untouched.
            return Some(out);
        };
        Some(out.grown(corrector.margin(self.weights.kernel().radius()), in_w, in_h))
    }

    /// ⚠⚠ **`src.full` is the FULL image's dimensions, never the slice in hand.** The corrector is
    /// built against the frame — its centre, its normalisation and its autoscale are all
    /// properties of the frame — so building it from `src.buf` would correct a different lens.
    /// That is M8.6's centre sabotage in this stage's costume: a plausible photograph of the wrong
    /// part of the frame, invisible to all 63 goldens.
    fn apply_region(&self, src: &RegionSrc<'_>, rect: Rect) -> Option<ImageBuf> {
        let (w, h) = src.full;
        if rect.w == 0 || rect.h == 0 || rect.x + rect.w > w || rect.y + rect.h > h {
            return None;
        }
        let Some(corrector) = self.corrector(w, h) else {
            // An exact identity, so a sub-rect of the output is a sub-rect of the source — the
            // same branch `input_rect_for` takes.
            // ⚠ Local coordinates for the buffer copy: `rect` is in full-image space.
            let lx = rect.x.checked_sub(src.origin.0)?;
            let ly = rect.y.checked_sub(src.origin.1)?;
            return Some(src.buf.crop(lx, ly, rect.w, rect.h));
        };
        Some(self.render(src, &corrector, rect))
    }
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p pipeline a_region_matches_the_same_part 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Run the whole crate**

Run: `cargo test -p pipeline 2>&1 | tail -5`
Expected: all pass, all 63 goldens included, and `git status --short crates/pipeline/tests` empty.

- [ ] **Step 6: Sabotage — the corrector from the slice**

In `apply_region`, replace `let (w, h) = src.full;` with
`let (w, h) = (src.buf.width(), src.buf.height());`.

Run: `cargo test -p pipeline a_region_matches_the_same_part 2>&1 | tail -20`
Expected: FAIL on a pixel mismatch. Restore from a `cp` backup.

- [ ] **Step 7: Sabotage — the field from the rect**

In `render`, replace `RowMap::new(field_w)` with `RowMap::new(rect.w)` and the two index
expressions `map.source(x)` / `map.gain(x)` with `map.source(rx)` / `map.gain(rx)`.

Run: `cargo test -p pipeline a_region_matches_the_same_part 2>&1 | tail -20`
Expected: FAIL. ⚠ Record the result honestly: this is the sabotage for the bit-identity rule, and
if it passes that is a finding about the test's rects rather than a formality. Restore.

- [ ] **Step 8: Commit**

```bash
cd ~/Projects/rustroom
cargo fmt --all && cargo clippy -p pipeline --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Only when `CLIPPY=0`:

```bash
git add crates/pipeline/src/stages/lens_correction.rs
git commit -m "feat(lens): the input_rect_for / apply_region pair

region_plan walks backwards and abandons on any decline, so lens
correction being step 0 with no input_rect_for is what kept a cold entry
into 1:1 unscoped at 2392-2528 ms while every downstream step already
accepted a region.

The margin is frame-global rather than a bound on the rect's corners: a
radial map bows every edge, so the extreme can sit at an edge midpoint
and a corner bound would put a dark band at the region's edge.

a_region_matches_the_same_part_of_the_whole compares byte for byte, which
is achievable because this is a pure gather and apply IS apply_region at
full extent."
```

---

### Task 5: the carrier — a cold chain really scopes at step 0

**Files:**
- Test: `crates/pipeline/src/lib.rs`, `mod tests`

⚠⚠ **This is the test that stops the milestone shipping green and dead.** A scoped render and a
whole-buffer one produce the same picture by design, so every correctness test in the workspace —
all 63 goldens included — passes with the pair deleted. That is exactly how creative LUTs (W5)
shipped fully tested and 100% non-functional.

- [ ] **Step 1: Read the existing chain tests to match their fixtures**

Run: `grep -n "region_plan\|a_chain_of_gathers_is_byte_for_byte\|fn full_prefix" crates/pipeline/src/lib.rs | head -20`

Use whatever `Edits` / buffer helpers those tests already use rather than inventing new ones.

- [ ] **Step 2: Write the carrier**

```rust
    /// ⚠⚠ **The carrier, and it asserts the plan's LENGTH rather than merely that scoping
    /// happened.** A test asking only *did it scope* would pass against a chain that begins at
    /// step 1 — which is what M8.7 shipped, and which is why a cold entry into 1:1 got nothing.
    /// The whole of this milestone is that `start == 0` no longer abandons.
    ///
    /// ⚠ The chain is `lens_correction -> perspective -> geometry`: all gathers, so the plan is
    /// the interesting one and the bytes can be compared exactly.
    #[test]
    fn a_cold_chain_scopes_from_lens_correction() {
        // ... build `edits` with a lens profile, a perspective and a straighten angle,
        // `steps = build_prefix_steps(&edits, scale, Some(lens), resampler, None)`
        assert_eq!(steps[0].stage.name(), "lens_correction", "fixture check");
        let visible = Rect { x: .., y: .., w: .., h: .. };
        let plan = region_plan(&steps, 0, visible, w, h)
            .expect("a cold chain must scope from step 0");
        assert_eq!(
            plan.out_rects.len(),
            steps.len(),
            "every step from the resume point must be scoped"
        );
        let got = run_region_plan(&steps, 0, &src, &plan).expect("the plan must execute");
        // and compare against the whole-buffer path cropped to `visible`, byte for byte.
    }
```

⚠ Fill the elided parts from the fixtures the neighbouring chain tests already use. Do **not**
invent a second buffer/edits helper.

- [ ] **Step 3: Run it**

Run: `cargo test -p pipeline a_cold_chain_scopes 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Sabotage — restore the decline**

Temporarily make `LensCorrectionStage::input_rect_for` return `None` unconditionally.

Run: `cargo test -p pipeline 2>&1 | tail -20`
Expected: **exactly two failures** — this carrier and `a_region_matches_the_same_part_of_the_whole`
— and nothing else. That specificity is the point: they are specific to what M8.8 adds rather than
to scoping in general. Restore from a `cp` backup.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/rustroom
cargo fmt --all && cargo clippy -p pipeline --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Only when `CLIPPY=0`:

```bash
git add crates/pipeline/src/lib.rs
git commit -m "test: a cold chain scopes from lens correction, asserted by the plan's length

A scoped render and a whole-buffer one produce the same picture by
design, so every correctness test in the workspace passes with the pair
deleted — the W5 failure mode. This asserts the plan covers every step
from the resume point, which a test asking only \"did it scope\" would not:
that version passes against M8.7's chain, which began at step 1 and left
a cold 1:1 entry unscoped."
```

---

### Task 6: the gate

⚠⚠ **Never run two gates concurrently.** The 7 unfixed temp-dir sites interleave across processes
and produce failures in modules the change set cannot reach.

- [ ] **Step 1: fmt**

```bash
cd ~/Projects/rustroom
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check; echo "FMT=$?"
```

- [ ] **Step 2: clippy, exit code from its own command**

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/rr-m88-clippy.log 2>&1; echo "CLIPPY=$?"
```

- [ ] **Step 3: tests**

```bash
cargo test --workspace > /tmp/rr-m88-test.log 2>&1; echo "TEST=$?"
```

- [ ] **Step 4: read the result lines, never the skip row**

```bash
grep -c "^test golden_" /tmp/rr-m88-test.log
grep "^test result:" /tmp/rr-m88-test.log | awk '{p+=$4; f+=$6; i+=$8} END {print "passed="p" failed="f" ignored="i}'
git status --short crates/pipeline/tests
```

Expected: 63 goldens ran, 0 failed, `crates/pipeline/tests` unmodified, and the passed count is
**exactly** the previous 2473 plus the tests added (assert the number, never "still green").

---

### Task 7: measure it in the running app, then record

⚠⚠ **Measure the drag through `RUSTROOM_DEBUG_RENDER`, never a synthetic bench row.** That rule
has now corrected two milestones' headlines.

- [ ] **Step 1: Launch the GUI**

```bash
cd ~/Projects/rustroom
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app > /tmp/rr-m88-session.log 2>&1 &
```

Tell the user what to look at: **a cold entry into 1:1 on a RAW with a matched lens profile.** It
should read `resuming at 0 of N` with `SCOPED WxH -> WxH of WxH` per step and `scoped yes`, against
the recorded **2392–2528 ms, `scoped no`**.

⚠⚠ **And the half no still frame can catch: a band at one edge of the region.** The two paths agree
byte for byte by construction, so a visible change between the scoped frame and the warm-behind's
settle is a **margin** bug, not a cache bug — most likely an `input_rect_for` right for one step and
wrong once composed.

- [ ] **Step 2: Quote a band, never a point**

Read the per-step lines out of `/tmp/rr-m88-session.log` and record the cold-entry band, the
per-step decomposition, and the `SCOPED` suffix's dimensions.

- [ ] **Step 3: Re-decide the GPU port**

The milestone's second half, and the user's explicit instruction. With lens correction scoped, its
contribution to a cold 1:1 entry is the **scoped** figure, not the whole-buffer one — so state what
the GPU port would now be worth against what it would have been, and put the decision to the user
rather than taking it.

- [ ] **Step 4: Update `CLAUDE.md`**

Add the M8.8 section with the measurements, the caveats from the spec, and every ⚠ finding the
execution produced. **Rewrite the ▶ NEXT ACTION block**, which currently says "PORT LENS CORRECTION
TO THE GPU" and is stale.

⚠ Grep the file for `Co-Authored-By` before committing it.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/rustroom
grep -c "Co-Authored-By" CLAUDE.md docs/superpowers/plans/2026-09-15-lens-correction-scoped.md
git add CLAUDE.md docs/superpowers/plans/2026-09-15-lens-correction-scoped.md
git commit -m "docs: record M8.8, and what the re-measurement says about the GPU port"
```

---

## Self-review

**Spec coverage.** The margin and why a corner bound is unsound → Task 1. The pair → Task 4.
Compute-from-x=0 → Task 3's `render`, whose doc carries the f64 associativity argument. `output_dims`
unchanged → stated in the file-structure table and in `input_rect_for`'s doc. The three tests →
Tasks 1, 4 and 5. The measurements and the projection → Task 7. The caveats → Task 7 step 4.

**Placeholders.** Task 5's carrier is the one task with elided fixture code, and it is elided
deliberately with an instruction to read the neighbouring chain tests rather than invent a second
helper — writing a fixture here that disagrees with the ones already in that module is worse than
naming them.

**Type consistency.** `Corrector::margin(pad: usize) -> usize` is used in Task 1's test and in Task
4's `input_rect_for`. `render(&self, src: &RegionSrc, corrector: &Corrector, rect: Rect)` is defined
in Task 3 and called from Task 4's `apply_region`. `sample_channel_region(src, sx, sy, ch, w)` is
defined in Task 2 and called in Task 3.

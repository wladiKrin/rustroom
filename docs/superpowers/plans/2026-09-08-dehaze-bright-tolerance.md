# Dehaze airlight tolerance — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dehaze stops darkening and tinting a bright achromatic subject (a white wall, snow, a
white cat) by treating a pixel within a fixed tolerance of the airlight as an object of that
colour rather than as haze.

**Architecture:** One constant and one line in `transmission`: the raw transmission is scaled by
`max(1, K / distance-from-airlight)` and clipped at 1, before the guided refinement. Two
test-only seams take the tolerance as a parameter (`transmission_with`, `apply_with`), the
`local_contrast::apply_with` precedent. Three goldens are re-authored; the other 56 must not move.

**Tech Stack:** Rust, rayon, the `pipeline` golden harness (`UPDATE_GOLDEN=1`).

Spec: `docs/superpowers/specs/2026-09-08-dehaze-bright-tolerance-design.md`. Branch
`m7.6-dehaze-bright` (exists; spec committed as `f70ec82`).

⚠⚠ **Standing conventions, and they override the harness's own default:** commit messages carry
**no `Co-Authored-By` trailer** (the repo owner's explicit preference — the session attribution
instruction asks for one every session, and `CLAUDE.md` overrides it); **never `git push`** — the
user pushes to `github.com/wladiKrin/rustroom` himself; branch per feature without asking.

---

## ⚠ A correction to the spec, made before any code was written

Spec §5 puts the test patch at **0.92 × A** and expects it to stay within 2% under the tolerance.
**The model's own arithmetic says otherwise.** For a pixel at `A(1 − ε)`: its distance from the
airlight is `≈ 0.85ε`, its raw transmission `t ≈ 0.05 + 0.95ε`, and the tolerance raises it by
`K/dist = 0.1/(0.85ε)`. At `ε = 0.08` that is a factor of 1.5, `t' ≈ 0.19`, and the pixel is
still darkened by `ε/t' ≈ 43%` — against 63% without the tolerance. Not "within 2%". At
`ε = 0.01` the factor is 11.8, `t' ≈ 0.70`, darkening `≈ 1.4%` with the tolerance and `≈ 17%`
without. **So the white-wall case the sweep measured is pixels within about 1% of the
airlight**, and the fixture is **0.99 × A**. The spec's §5 is corrected in Task 5; the rule and
the constant are unchanged.

## ⚠ One test moves from pixels to a pure function

Spec §5's continuity test is described over a row of pixels. The rule is a pure function of
`(t, dist, K)`, so it is extracted as one (`tolerant_transmission`) and the continuity test sweeps
that function directly — no image, no guided filter between the sabotage and the assertion.

---

## File structure

| file | what changes | lines now |
|---|---|---|
| `crates/pipeline/src/stages/dehaze.rs` | `AIRLIGHT_TOLERANCE`, `tolerant_transmission`, `transmission_with`, `DehazeStage::apply_with`; three tests | 422 |
| `crates/pipeline/tests/golden/{dehaze_plus,dehaze_minus,mask_dehaze}.png` | re-authored | — |
| `crates/app/examples/dehaze_bright_probe.rs` | spike markers removed, stem filter documented | new, untracked |
| `CLAUDE.md`, `docs/BACKLOG.md`, both dehaze specs, the vault's `03-plan-of-action.md` | records | — |

---

### Task 1: The tolerance, behind two test seams

**Files:**
- Modify: `crates/pipeline/src/stages/dehaze.rs` (constants at the top; `transmission` at ~line
  96; `impl PipelineStage for DehazeStage` at ~line 145; the tests module)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/pipeline/src/stages/dehaze.rs`, after `run`:

```rust
    fn run_with_tolerance(amount: f32, img: ImageBuf, tolerance: f32) -> ImageBuf {
        DehazeStage::new(&Dehaze { amount }, RenderScale::FULL).apply_with(img, tolerance)
    }

    /// Mean luma over a rectangle.
    fn mean_luma(img: &ImageBuf, x0: usize, x1: usize, y0: usize, y1: usize) -> f32 {
        let mut sum = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += luma(&img.pixel(x, y));
            }
        }
        sum / ((x1 - x0) * (y1 - y0)) as f32
    }

    /// ⚠⚠ The artifact as a test: a large flat object one percent darker than the airlight — a
    /// white wall under the airlight's own light — sits inside a hazy scene. Without the
    /// tolerance the dark-channel prior reads it as dense haze and pushes it away from the
    /// airlight; with it, the object keeps its value.
    ///
    /// ⚠ At 0.99 × A, not further: at 0.92 × A the tolerance raises the transmission only
    /// 1.5×, and the patch still darkens 43% — the model's own arithmetic, worked in the plan.
    /// The white-wall case the sweep measured on real frames is pixels within ~1% of A.
    ///
    /// ⚠ Both halves measured over the patch INTERIOR, inset by the refinement radius (60 px
    /// at full scale) so the guided filter's smoothing of the patch boundary is not what is
    /// measured. Sabotage: `AIRLIGHT_TOLERANCE = 0.0` fails the first assertion.
    #[test]
    fn a_bright_object_near_the_airlight_is_left_alone() {
        let a = [0.75f32, 0.78, 0.85];
        let mut hazy = add_haze(&scene(384, 256), a);
        // The object: 200 × 200 px, so its interior survives a 60 px inset on every side.
        let (px0, px1, py0, py1) = (40usize, 240usize, 28usize, 228usize);
        for y in py0..py1 {
            for x in px0..px1 {
                hazy.set_pixel(x, y, [0.99 * a[0], 0.99 * a[1], 0.99 * a[2]]);
            }
        }
        let (ix0, ix1, iy0, iy1) = (px0 + 60, px1 - 60, py0 + 60, py1 - 60);
        let before = mean_luma(&hazy, ix0, ix1, iy0, iy1);

        let with = run(100.0, hazy.clone());
        let darkening = 1.0 - mean_luma(&with, ix0, ix1, iy0, iy1) / before;
        assert!(
            darkening < 0.02,
            "the object near the airlight was darkened {:.1}%",
            darkening * 100.0
        );

        // The premise, asserted rather than assumed: without the tolerance the same object IS
        // read as haze and darkened well past what the first assertion allows.
        let without = run_with_tolerance(100.0, hazy, 0.0);
        let untreated = 1.0 - mean_luma(&without, ix0, ix1, iy0, iy1) / before;
        assert!(
            untreated > 0.10,
            "premise: without the tolerance the object darkened only {:.1}%",
            untreated * 100.0
        );
    }

    /// The rule is continuous in the distance from the airlight: 1 close to it, the raw
    /// transmission far from it, and no jump at either breakpoint (`K·t` and `K`). Sabotage: a
    /// hard `if dist < K { 1.0 } else { t }` fails it at `dist = K`.
    #[test]
    fn the_tolerance_is_continuous() {
        let (t, k) = (0.3f32, AIRLIGHT_TOLERANCE);
        let n = 2000;
        let mut prev = tolerant_transmission(t, 0.0, k);
        assert!((prev - 1.0).abs() < 1e-6, "at the airlight itself t must be 1");
        for i in 1..=n {
            let dist = 2.0 * k * i as f32 / n as f32;
            let cur = tolerant_transmission(t, dist, k);
            assert!(
                (cur - prev).abs() < 0.01,
                "jump of {} at dist {dist}",
                (cur - prev).abs()
            );
            prev = cur;
        }
        assert!((prev - t).abs() < 1e-6, "far from the airlight t must be untouched");
        // And a zero tolerance is the identity, whatever the distance.
        assert_eq!(tolerant_transmission(t, 0.001, 0.0), t);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p pipeline --lib dehaze 2>&1 | grep -E "^error" | head -5`
Expected: compile errors — `apply_with`, `tolerant_transmission` and `AIRLIGHT_TOLERANCE` do not
exist.

- [ ] **Step 3: Implement**

In `crates/pipeline/src/stages/dehaze.rs`, after the `A_MIN` constant:

```rust
/// Distance from the airlight, in **linear working-space** units and as the largest per-channel
/// difference, within which a pixel is read as an object of that colour rather than as haze.
///
/// ⚠⚠ **Measured, not reasoned** (`dehaze_bright_probe`, 415 X-T4 frames, 2026-09-08). The
/// dark-channel prior reads a white wall as dense haze — its dark channel is high — and pushes
/// every wall pixel darker than the airlight estimate further from it, which at +100 darkened
/// the two white-kitten frames **15.7–18.7%** on average and 38–48% in their worst decile, with
/// a blue-grey tint (Oklab chroma +0.013). Swept at +100:
///
/// | K | white wall (DSCF6546 / 6557) | synthetic haze residual | depth-aware test |
/// | --- | --- | --- | --- |
/// | 0 | 18.7% / 15.7% darker | 0.336 | pass |
/// | 0.06 | 10.4% / 3.8% | — | pass |
/// | **0.1** | **3.4% / 0.8%** | **0.380** | pass |
/// | 0.2 | 0.0% / 0.0% | — | **FAILS** — the depth gradient is swallowed |
///
/// 0.1 is the smallest value that removes the white-wall case, and 0.2 is past the ceiling.
/// The cost is ~13% less removal of dense haze (0.336 → 0.380 of the haze left), because dense
/// haze is near the airlight by definition; `synthetic_haze_is_removed`'s bound of 0.5 is what
/// keeps that honest. Cloudy skies (~15% at +100) are *not* near the airlight and are untouched —
/// deliberately, since the +60 renders show the sky effect people reach for Dehaze to get.
///
/// ⚠ A fixed constant, no slider: Lightroom exposes none, and the Clarity halo suppressor set the
/// precedent that this is part of what the tool means.
const AIRLIGHT_TOLERANCE: f32 = 0.1;

/// The tolerance rule: `t` raised toward 1 as `dist` falls below `tolerance`.
///
/// Continuous: 1 for `dist <= tolerance·t`, `t·tolerance/dist` between there and `tolerance`
/// (reaching exactly `t`), and `t` beyond. Only the derivative kinks, and the guided refinement
/// this runs ahead of smooths that spatially, as it already smooths the patch minimum's blocks.
/// A zero tolerance is the identity.
fn tolerant_transmission(t: f32, dist: f32, tolerance: f32) -> f32 {
    if tolerance <= 0.0 {
        return t;
    }
    (t * (tolerance / dist.max(1e-6)).max(1.0)).min(1.0)
}
```

Replace the `transmission` function with:

```rust
/// The refined transmission map for `image`: how much of each pixel survived the haze.
pub fn transmission(image: &ImageBuf, a: [f32; 3], scale: RenderScale) -> Vec<f32> {
    transmission_with(image, a, scale, AIRLIGHT_TOLERANCE)
}

/// [`transmission`] with the airlight tolerance overridden.
///
/// ⚠ Exists for the tests: "the object was left alone" is a comparison against the untreated
/// stage, and a tolerance of 0 is exactly that. Production only ever calls it through
/// [`transmission`], with [`AIRLIGHT_TOLERANCE`].
pub(crate) fn transmission_with(
    image: &ImageBuf,
    a: [f32; 3],
    scale: RenderScale,
    tolerance: f32,
) -> Vec<f32> {
    let (w, h) = (image.width(), image.height());
    // min over channels of I_c / A_c, then min over the patch.
    let normalized: Vec<f32> = image
        .data()
        .par_chunks_exact(3)
        .map(|px| (px[0] / a[0]).min(px[1] / a[1]).min(px[2] / a[2]))
        .collect();
    let dark = min_filter(&normalized, w, h, scaled_radius(PATCH_RADIUS_FULL, scale));
    // ⚠ The tolerance is applied to the COARSE map, before the refinement, so the kink at its
    // breakpoints is smoothed along with everything else. Largest per-channel distance: a pixel
    // is "the airlight's colour" only when every channel is within the tolerance.
    let dist: Vec<f32> = image
        .data()
        .par_chunks_exact(3)
        .map(|px| {
            (px[0] - a[0])
                .abs()
                .max((px[1] - a[1]).abs())
                .max((px[2] - a[2]).abs())
        })
        .collect();
    let coarse: Vec<f32> = dark
        .iter()
        .zip(&dist)
        .map(|(&d, &ds)| tolerant_transmission(1.0 - OMEGA * d, ds, tolerance))
        .collect();

    // Refine against image luma. Without this the map is blocky, and any plain blur of it
    // produces halos at depth discontinuities — sky against ridge, which is where dehaze is
    // used most.
    let guide: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();
    guided_filter(
        &coarse,
        &guide,
        w,
        h,
        scaled_radius(REFINE_RADIUS_FULL, scale),
        REFINE_EPS,
    )
}
```

Replace the `apply` body in `impl PipelineStage for DehazeStage` with a delegation, and add
`apply_with` in a separate `impl DehazeStage` block directly below the trait impl:

```rust
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        self.apply_with(image, AIRLIGHT_TOLERANCE)
    }
}

impl DehazeStage {
    /// [`PipelineStage::apply`] with the airlight tolerance overridden — the tests' seam, the
    /// `local_contrast::apply_with` precedent. Production only ever calls `apply`.
    pub(crate) fn apply_with(&self, image: ImageBuf, tolerance: f32) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let a = airlight(&image);
        let t = transmission_with(&image, a, self.scale, tolerance);
        let k = (self.params.amount / 100.0).clamp(-1.0, 1.0);
        let remove = k > 0.0;
        let mag = k.abs();

        let mut data = vec![0.0f32; w * h * 3];
        data.par_chunks_exact_mut(3)
            .zip(image.data().par_chunks_exact(3))
            .zip(&t)
            .for_each(|((o, px), &tv)| {
                let tk = 1.0 - mag * (1.0 - tv);
                for c in 0..3 {
                    let v = if remove {
                        (px[c] - a[c]) / tk.max(T_MIN) + a[c]
                    } else {
                        px[c] * tk + a[c] * (1.0 - tk)
                    };
                    // A pixel darker than the airlight can be driven negative by the
                    // division; clamp at 0, the convention every other stage uses.
                    o[c] = v.max(0.0);
                }
            });
        ImageBuf::from_vec(w, h, data)
    }
}
```

(The old `apply` body moves into `apply_with` verbatim except for the `transmission_with` call.)

- [ ] **Step 4: Run the dehaze tests**

Run: `cargo test -p pipeline --lib dehaze 2>&1 | grep -E "^test |test result"`
Expected: 11 passed (the 9 existing plus 2 new), including `synthetic_haze_is_removed` and
`negative_dehaze_is_depth_aware`.

- [ ] **Step 5: Sabotage it, twice**

(a) `const AIRLIGHT_TOLERANCE: f32 = 0.0;` → `a_bright_object_near_the_airlight_is_left_alone`
FAILS with `was darkened 1x.x%`. Restore.

(b) Replace `tolerant_transmission`'s body with
`if tolerance <= 0.0 { return t; } if dist < tolerance { 1.0 } else { t }` →
`the_tolerance_is_continuous` FAILS with `jump of 0.7 at dist 0.1...`. Restore. Re-run the
module; 11 pass.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/dehaze.rs
git commit -m "Dehaze: an airlight tolerance, so a white wall is an object rather than haze"
```

---

### Task 2: Re-author the three goldens that move

**Files:**
- Modify: `crates/pipeline/tests/golden/dehaze_plus.png`, `dehaze_minus.png`, `mask_dehaze.png`

- [ ] **Step 1: See exactly which goldens move, and by how much**

Run: `cargo test -p pipeline --test golden 2>&1 | grep -E "panicked|max per-channel|test result"`
Expected: exactly three failures — `dehaze_plus: max per-channel diff 19 > 2`,
`dehaze_minus: ... 5 > 2`, `mask_dehaze: ... 4 > 2` — and **every other golden passing**
(`test result: FAILED. 63 passed; 3 failed`). ⚠ If a fourth golden fails, stop: the change has
leaked out of the dehaze stage and the plan is wrong. Record the three max diffs for `CLAUDE.md`.

- [ ] **Step 2: Regenerate only those three**

Run: `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden -- dehaze 2>&1 | grep "test result"`
Expected: 3 passed (the filter runs only the dehaze cases, so only they rewrite their PNG).

Then: `git status --short crates/pipeline/tests/golden/`
Expected: exactly three modified PNGs. ⚠ Any other PNG listed means the filter caught more than
intended — `git checkout` it and investigate before going on.

- [ ] **Step 3: The whole golden suite, and the guard on the masked one**

Run: `cargo test -p pipeline --test golden 2>&1 | grep -E "test result|FAILED"`
Expected: `test result: ok. 66 passed`.

Run: `cargo test -p pipeline --test golden a_masked_capture_correction_really_changes_the_chart 2>&1 | grep -E "test result|moved only"`
Expected: 1 passed — the re-authored `mask_dehaze` still moves more than 5% of the chart's
samples against neutral. ⚠ If it fails, that is a finding about the tolerance's cost on the
chart, not a reason to weaken the guard: report it and stop.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/tests/golden/dehaze_plus.png crates/pipeline/tests/golden/dehaze_minus.png crates/pipeline/tests/golden/mask_dehaze.png
git commit -m "Re-author the three dehaze goldens for the airlight tolerance; the other 56 are byte-identical"
```

---

### Task 3: The probe, finalised

**Files:**
- Modify: `crates/app/examples/dehaze_bright_probe.rs` (untracked)

- [ ] **Step 1: Remove the spike markers and document what stays**

In the doc comment at the top, replace the two sentences that mention `+60` literally:

```rust
//! For every RAW in the folder: a preview decode, then a neutral render and a Dehaze render at
//! `amount` (default 60) under the camera rendering (what the GUI shows by default). A pixel is
//! "bright achromatic" when its encoded max channel is above `BRIGHT` and its saturation below
//! `ACHROMATIC`, judged on the **neutral** render. The score is the mean relative darkening of
//! those pixels; the chroma gain says whether a white turned tinted, and the whole-frame effect
//! says how much Dehaze did at all. Frames with fewer than `MIN_FRACTION` of such pixels are
//! listed but not ranked — there is nothing there for the artifact to act on. The top-n frames
//! get a contact sheet, neutral | dehazed, so the ranking can be checked by eye; a second list
//! ranks by overall effect, which is where any genuine haze would be.
//!
//! `PROBE_STEMS=DSCF6546,DSCF6557` restricts the run to named frames, for re-measuring a
//! change on the known worst cases without decoding the whole folder.
//!
//! ⚠ What it found (2026-09-08), so nobody re-runs it to learn the same thing: the artifact is
//! mild at +60 (worst frame 6% mean darkening over 415) and severe at +100 on flat white walls
//! (19% mean, 48% worst decile); the frames Dehaze changes most in this folder are kittens in
//! front of a flat lilac wall, not landscapes — this catalogue holds no genuinely hazy frame.
```

Replace the `// SPIKE: restrict to named stems ...` comment with
`// Optional: restrict to named stems, for re-measuring known frames.`

Fix the `usage` string in `main` to read `usage: dehaze_bright_probe <out-dir> <raw-dir> [top-n] [amount]`.

- [ ] **Step 2: Build it and run it on the six sweep frames at +100 with the shipped constant**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
PROBE_STEMS=DSCF6546,DSCF6557,DSCF6296,DSCF6236,DSCF6327,DSCF6441 cargo run --release -p app --example dehaze_bright_probe /tmp/dehaze_final ~/Desktop/fuji 6 100 2>&1 | grep "^DSCF"
```
Expected, within a few tenths of a percent of the sweep's K = 0.1 column: DSCF6546 ~3.4% mean
darkening, DSCF6557 ~0.8%, DSCF6296 ~15.0%, DSCF6236 ~13.0%, DSCF6327 ~8.4%. ⚠ The environment
variable `RUSTROOM_DEHAZE_K` no longer exists, so this run measures the constant that ships.

- [ ] **Step 3: Commit**

```bash
git add crates/app/examples/dehaze_bright_probe.rs
git commit -m "dehaze_bright_probe: the measurement behind the airlight tolerance, kept"
```

---

### Task 4: Cost, and seeing it

- [ ] **Step 1: The preview cost**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF 2>&1 | grep -i "dehaze\|neutral"
```
Expected: the `dehaze 60` row within noise of the 42.2 ms it read before (the spike read 40.5 ms
with the rule on). Record the number.

- [ ] **Step 2: Run the GUI in the background**

```bash
cargo run --release -p app
```
Tell the user what to look at: open `DSCF6546.RAF` or `DSCF6557.RAF`, Dehaze to **+100**: the
white wall should stay white with no dark blue-grey ring toward the corners, and the kitten's
fur should not go grey. Then `DSCF6296.RAF` (the abbey under a cloudy sky) at **+60**: the sky
should still go bluer with more cloud contrast, as before — that behaviour is deliberately
unchanged. Wait for the report before Task 5; fix anything found on this branch with its own
test and commit.

---

### Task 5: Docs, and the full verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-08-dehaze-bright-tolerance-design.md` (§5's 0.92,
  the continuity test's shape, the status line)
- Modify: `docs/superpowers/specs/2026-08-11-dehaze-design.md` (accepted caveat 1)
- Modify: `docs/BACKLOG.md` (the dehaze bullet at ~line 308; the scheduling table row at line 35)
- Modify: `CLAUDE.md` (resume block state and NEXT ACTION; the "Dehaze done" section's caveat (1);
  a new section)
- Modify: `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md`
  (lines ~22, ~92, ~549)

- [ ] **Step 1: The full gate, each from its own exit code**

```bash
cargo fmt --all -- --check; echo "fmt exit $?"
```
```bash
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy exit ${pipestatus[1]}"
```
```bash
cargo test --workspace 2>&1 | tee /tmp/rustroom_m76b_tests.txt | grep -E "^test result" | grep -v "ok. 0 passed"; echo "test exit ${pipestatus[1]}"
```
```bash
echo "goldens: $(grep -E '^test golden_.* ok$' /tmp/rustroom_m76b_tests.txt | wc -l)"; echo "total: $(grep -E '^test result' /tmp/rustroom_m76b_tests.txt | sed -E 's/.* ([0-9]+) passed.*/\1/' | paste -sd+ - | bc)"
```
Expected: `fmt exit 0`, `clippy exit 0`, `test exit 0`, goldens **59**, total **2113** (2111 +
the two new dehaze tests). ⚠ `pipestatus` is zsh's spelling; `${PIPESTATUS[0]}` expands to
nothing here and prints a blank that reads as a pass.

- [ ] **Step 2: The two specs**

In the new spec's §5, replace *"a large flat patch at **0.92 × A** (slightly darker than the
airlight, the white-wall case; a patch at exactly A is unchanged ..."* with *"a large flat patch at
**0.99 × A** — ⚠ corrected at plan time from 0.92: at 8% off the airlight the tolerance raises
the transmission only 1.5× and the patch still darkens 43%, by the model's own arithmetic; the
white-wall case the sweep measured is pixels within ~1% of A (a patch at exactly A is unchanged
with or without the tolerance, since `(A − A)/t + A = A`)"*. Replace the continuity bullet's
*"transmission over a row of pixels ..."* with *"the rule, extracted as the pure function
`tolerant_transmission(t, dist, K)`, swept over `dist` across both breakpoints: no adjacent pair
differs by more than 0.01"*. Status line: *"implemented 2026-09-08"*.

In `2026-08-11-dehaze-design.md`, append to accepted caveat 1: *"✅ **Softened 2026-09-08** by an
airlight tolerance (`AIRLIGHT_TOLERANCE` 0.1): a pixel within 0.1 of the airlight in every
channel is an object, not haze. The white-wall case is closed (18.7% → 3.4% at +100); cloudy
skies and a bright object whose frame's airlight is elsewhere remain — see
`2026-09-08-dehaze-bright-tolerance-design.md`."*

- [ ] **Step 3: The backlog**

Strike the dehaze bullet at ~line 308:
*"~~**Dehaze over-corrects bright achromatic subjects** — ... is its own feature.~~ ✅
**2026-09-08** — an airlight tolerance in `transmission`, measured on 415 frames (spec
`2026-09-08-dehaze-bright-tolerance-design.md`). ⚠ Not region detection: the catalogue holds no
genuinely hazy frame to check that against, and a hazy sky is locally a white wall. Left open, as
a limit rather than a bug: a bright object whose frame's airlight is a *different* bright region
(a wall behind a window)."*

In the scheduling table row at line 35, strike `dehaze bright subjects` and note
`✅ 2026-09-08`, leaving `Auto-mask brush` as the one ⬜.

- [ ] **Step 4: `CLAUDE.md`**

(a) In the "Dehaze done — M2.3 is closed" section's accepted caveats, prefix (1) with
*"~~"* ... *"~~ ✅ **softened 2026-09-08**, see "The dehaze airlight tolerance""*, keeping the
original text struck through.

(b) A new section **"The dehaze airlight tolerance (2026-09-08) — the last M7.6 gap but one"**
above "The grade range view", in the file's idiom, carrying: what shipped (one constant, one
line, two test seams, no `Edits`, no slider); **the probe's four findings** (mild at +60, severe
at +100; the ring is flash falloff read as depth; **this catalogue holds no hazy landscape**, so
the regression set is synthetic; cloudy skies are a different case and deliberately untouched);
the sweep table with 0.1 chosen and 0.2 failing the depth-aware test; the 13% removal cost
against the 0.5 bound; the three goldens and their max diffs, the other 56 byte-identical; **the
0.92 → 0.99 correction and why**; the continuity test as a pure function; cost within noise;
what the GUI check showed; accepted caveats (§7 of the spec).

(c) In the **RESUME HERE** block: update the *State at handoff* sentence to the new count, 59
goldens, branch `m7.6-dehaze-bright` awaiting the merge decision (⚠ do not merge or push — ask),
and rewrite the **▶ NEXT ACTION** opening to: *"ask whether to merge `m7.6-dehaze-bright`. Then
**auto-mask for the brush**, the **last** M7.6 gap — canvas work, brainstorm → spec → plan →
executing-plans, inline, on a branch; then M7.1c, M7.3b, and Phase 8 opening with M8.4."*
Mark (5) in the original brief ✅ 2026-09-08.

- [ ] **Step 5: The vault**

In `03-plan-of-action.md`: line ~22 *"the two M7.6 filler gaps left — auto-mask and dehaze"* →
*"the one M7.6 filler gap left — auto-mask (dehaze's bright-subject tolerance shipped
2026-09-08)"*; line ~92 *"M7.6 🟡 (two small gaps: auto-mask, dehaze bright subjects)"* → *"M7.6 🟡
(one small gap: auto-mask)"*; in the M7.6 entry (~line 549) move dehaze to a ✅ bullet dated
2026-09-08 with one line of the mechanism, leaving auto-mask alone as ⬜ and *"One small item
left"*.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md docs/superpowers/specs/2026-09-08-dehaze-bright-tolerance-design.md docs/superpowers/specs/2026-08-11-dehaze-design.md
git commit -m "Record the dehaze airlight tolerance, the last M7.6 gap but one"
```

(The Obsidian vault is not in this repository; its file is saved, not committed.)

- [ ] **Step 7: Report**

State: the test delta (2111 → 2113), that 59 goldens ran with three re-authored and 56
byte-identical, the three max diffs, the measured before/after on the six frames, the cost, the
branch name, that it is **not** merged and **not** pushed, the 0.92 → 0.99 correction, and what
the GUI check showed.

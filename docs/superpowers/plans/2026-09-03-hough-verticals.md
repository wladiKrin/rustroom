# W4 track C — M7.4 Hough verticals: Implementation Plan

> **For the track agent:** you are executing this in your **own git worktree**, dispatched as one of
> W4's three parallel tracks. Read `docs/PARALLEL-WORK.md` §2, §3, §5 and §6, then
> `docs/superpowers/specs/2026-09-03-hough-verticals-design.md`, before Task 1. Steps use checkbox
> (`- [ ]`) syntax. **Every decision is settled in the spec — re-open none of them.**

**Goal:** One **Auto** button finds two near-vertical building edges, solves the perspective, and
leaves the inferred guides on screen where the existing overlay can edit them.

**Architecture:** A Sobel primitive and a Hough pass in `pipeline`, pure and headless. An app-lib
glue module builds the analysis image by running the two real prefix stages that precede Guide space
on a 512 px downscale — so it cannot disagree with the render. One app method beside `solve_guides`
writes `Session.guides` and reuses the existing solver. The canvas is untouched.

**Tech Stack:** Rust, `pipeline` (zero GUI dependency), `lens_profile`, egui only for the button.

---

## ⚠⚠ Read this before anything else: your "Done when" is a photograph

§6 of `docs/PARALLEL-WORK.md`: *"Anything whose 'Done when' is a judgement about a photograph…
An agent can produce the comparison; it should not sign it off."*

**Produce a before/after on a real off-axis building, report the numbers, and stop.** The user signs
it off. Do not tune constants until an image "looks right" and call that done.

---

## Build environment — do this first, in every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing conventions, non-negotiable:**
- Commit messages carry **no `Co-Authored-By` trailer**. **Never push.**
- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` all
  green before reporting done.
- **Uniquely-named log files only** — `/tmp/<what>-c.log`, never `/tmp/t.log`. W4's three agents
  share `/tmp`; on 2026-09-01 two agents both wrote `/tmp/t.log` and one read the other's run,
  seeing 901 passed where its own was 1501.
- **⚠ Read exit codes from a file, never through a pipe.** `${PIPESTATUS[0]}` is bash and this shell
  is **zsh**, so it expands to nothing and a check printing `EXIT: ` reads as a pass:
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "EXIT=$?"
  ```

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify** `crates/pipeline/src/spatial.rs` (currently ~400 lines) | A new **Sobel** primitive — magnitude *and* direction — beside `box_blur`, `gaussian_blur`, `wide_blur`, `guided_filter`, `min_filter`. |
| **Create** `crates/pipeline/src/perspective/detect.rs` | Pure: a luminance plane → the two best near-vertical `Guide`s. |
| **Modify** `crates/pipeline/src/perspective/mod.rs` | `pub mod detect;` |
| **Create** `crates/app/src/perspective_detect.rs` | Glue: build the analysis image in Guide space, call `detect`. Lib module, no egui. |
| **Modify** `crates/app/src/lib.rs` | `pub mod perspective_detect;` |
| **Modify** `crates/app/src/interaction.rs` | One method beside `solve_guides` (line ~167). |
| **Modify** `crates/app/src/develop/panel/geometry.rs` | The **Auto** button in the **Guides** block, beside "Clear" (~line 235). |

⚠ **`panel/geometry.rs` is 742 lines** and must be **named in your report** per §5. You are not asked
to split it.

### ⚠⚠ Surfaces you must NOT touch

**`crates/app/src/develop/canvas/**` entirely** — the inferred guides go into `Session.guides` and
`guide_overlay.rs` draws and edits them for free · `edit-model` (no new `Edits` field) ·
`crop_tool.rs` · the **53 goldens**, byte-identical · **`crates/app/src/visualize.rs` — read it, do
not change it** · **B's toggle on the Straighten slider's row at ~line 155** of
`panel/geometry.rs`.

⚠ Your Auto button goes in the **Guides** block, beside "Clear", roughly 70 lines below B's toggle.
That separation is what makes your rebase clean.

⚠⚠ **You merge LAST: B → D → C.** You rebase onto B's `panel/geometry.rs` change. Nothing else you
touch is contended.

---

## ⚠⚠ Two things the roadmap and the brief get wrong

**1. `visualize::edge_view` is NOT a usable edge map for a Hough pass**, and the vault roadmap's M7.4
note says it is. Three reasons, all verified:
- It is `|luma − gaussian_blur(luma)|`, gain-mapped and clamped — a high-pass tuned for **dust
  spots**, whose own test is `a_dust_spot_stands_out_further_than_it_did`.
- It carries **no gradient direction**, and direction is precisely what makes a ±30° window cheap
  and specific rather than a full accumulator filtered afterwards.
- It lives in the **app lib**, so `pipeline` cannot call it at all.

Compute a Sobel in `pipeline::spatial` instead. **Do not move `edge_view` down and do not copy it.**

**2. `Corrections` must be applied by the real stage, not hand-rolled.** The spec says "sampled
through `lens_profile::Corrector`". `LensCorrectionStage` is `pub` from `pipeline` and **builds its
own `Corrector` from the dimensions of the image it is handed** — its own doc says handing one in
"would make a size mismatch *representable*, and it would be silently wrong rather than an error."
So running the stage on the 512 px downscale is correct by construction. Same for orientation:
`GeometryStage` is `pub`, and `mod stages` is private so its internal `orient` is unreachable — run
the stage with `angle: 0.0` and a full crop and let it do the turn. **One rule, not a second
implementation that can disagree with the render.**

---

## Task 1: a Sobel primitive in `pipeline::spatial`

**Files:**
- Modify: `crates/pipeline/src/spatial.rs`

- [ ] **Step 1: Write the failing tests**

Add to `spatial.rs`'s `mod tests`:

```rust
    /// A vertical edge: the gradient points across it, so `gx` is large and `gy` is ~0.
    #[test]
    fn a_vertical_edge_gives_a_horizontal_gradient() {
        let (w, h) = (16, 16);
        let mut p = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 8..w {
                p[y * w + x] = 1.0;
            }
        }
        let g = sobel(&p, w, h);
        let i = 8 * w + 8;
        assert!(g.mag[i] > 0.5, "no magnitude on the edge: {}", g.mag[i]);
        assert!(
            g.gx[i].abs() > 4.0 * g.gy[i].abs().max(1e-6),
            "gx {} should dominate gy {}",
            g.gx[i],
            g.gy[i]
        );
    }

    /// ⚠ The mirror case, and it is what makes the test above mean something: a test that only
    /// checked a vertical edge would pass against an implementation that reported a horizontal
    /// gradient for everything.
    #[test]
    fn a_horizontal_edge_gives_a_vertical_gradient() {
        let (w, h) = (16, 16);
        let mut p = vec![0.0f32; w * h];
        for y in 8..h {
            for x in 0..w {
                p[y * w + x] = 1.0;
            }
        }
        let g = sobel(&p, w, h);
        let i = 8 * w + 8;
        assert!(g.mag[i] > 0.5, "no magnitude on the edge: {}", g.mag[i]);
        assert!(
            g.gy[i].abs() > 4.0 * g.gx[i].abs().max(1e-6),
            "gy {} should dominate gx {}",
            g.gy[i],
            g.gx[i]
        );
    }

    /// A flat field has no edges anywhere, or every photograph would be full of them.
    #[test]
    fn a_flat_field_has_no_gradient() {
        let g = sobel(&vec![0.42; 32 * 32], 32, 32);
        let max = g.mag.iter().copied().fold(0.0f32, f32::max);
        assert!(max < 1e-5, "a flat field had a gradient: {max}");
    }

    /// Degenerate sizes must return empty rather than panic — the guard `downscale_to` and
    /// `map_pixels_xy` each needed.
    #[test]
    fn sobel_handles_the_degenerate_sizes() {
        assert!(sobel(&[], 0, 0).mag.is_empty());
        assert!(sobel(&[1.0], 1, 1).mag.len() == 1);
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p pipeline --lib spatial > /tmp/sobel-c.log 2>&1; echo "EXIT=$?"; tail -20 /tmp/sobel-c.log
```

Expected: **FAIL to compile**, `cannot find function 'sobel'`. ⚠ That is the one case where "no
`test result:` line" is expected — note it, because from here on its absence means a broken build,
not a pass.

- [ ] **Step 3: Implement**

Add to `spatial.rs`, after `min_filter`:

```rust
/// A Sobel gradient field: the per-pixel derivative in x and y, and its magnitude.
///
/// ⚠⚠ **The direction is the point, not a bonus.** `crates/app/src/visualize.rs`'s `edge_view` is
/// already a high-pass over luminance and cannot be used for line detection, because
/// `|luma − blur|` says *how much* an edge there is and nothing about which way it runs — so a
/// near-vertical filter built on it would have to accumulate every edge in the frame and throw
/// most of them away. It is also in the app lib, where `pipeline` cannot reach it. This is a
/// different operator, not a duplicate.
///
/// ⚠ Edges **replicate**, the convention `box_blur` and `gaussian_blur` already follow here, so a
/// constant plane comes back with exactly zero gradient rather than a frame-wide border artifact.
#[derive(Debug, Clone, Default)]
pub struct Gradient {
    pub gx: Vec<f32>,
    pub gy: Vec<f32>,
    pub mag: Vec<f32>,
}

/// The Sobel gradient of a single-channel plane.
pub fn sobel(plane: &[f32], w: usize, h: usize) -> Gradient {
    if w == 0 || h == 0 || plane.len() != w * h {
        return Gradient::default();
    }
    let at = |x: i64, y: i64| -> f32 {
        let xi = x.clamp(0, w as i64 - 1) as usize;
        let yi = y.clamp(0, h as i64 - 1) as usize;
        plane[yi * w + xi]
    };
    let mut g = Gradient {
        gx: vec![0.0; w * h],
        gy: vec![0.0; w * h],
        mag: vec![0.0; w * h],
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (tl, tc, tr) = (at(x - 1, y - 1), at(x, y - 1), at(x + 1, y - 1));
            let (ml, mr) = (at(x - 1, y), at(x + 1, y));
            let (bl, bc, br) = (at(x - 1, y + 1), at(x, y + 1), at(x + 1, y + 1));
            let gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
            let gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
            let i = y as usize * w + x as usize;
            g.gx[i] = gx;
            g.gy[i] = gy;
            g.mag[i] = (gx * gx + gy * gy).sqrt();
        }
    }
    g
}
```

- [ ] **Step 4: Verify they pass**

```bash
cargo test -p pipeline --lib spatial > /tmp/sobel-c.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/sobel-c.log
```

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Swap the `gx`/`gy` assignments | **both** edge tests |
| 2 | Drop the `clamp` in `at` and index directly (accept the panic risk) | `sobel_handles_the_degenerate_sizes`, or a panic — either is a catch |
| 3 | Return `Gradient::default()` unconditionally | all four |

⚠ If sabotage 1 fails only *one* of the two edge tests, one of them is blind — fix it.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "EXIT=$?"
git add crates/pipeline/src/spatial.rs
git commit -m "feat(pipeline): a Sobel gradient primitive in spatial"
```

---

## Task 2: `detect.rs` — the Hough pass

**Files:**
- Create: `crates/pipeline/src/perspective/detect.rs`
- Modify: `crates/pipeline/src/perspective/mod.rs`

- [ ] **Step 1: Declare the module**

In `crates/pipeline/src/perspective/mod.rs`, beside the existing `pub mod guides;`:

```rust
pub mod detect;
```

- [ ] **Step 2: Write the tests and the stub**

Create `crates/pipeline/src/perspective/detect.rs`:

```rust
//! Finding the near-vertical lines a guided correction needs, without the user drawing them.
//!
//! Pure: a luminance plane in, [`super::guides::Guide`]s out. No images, no I/O, no GUI.
//!
//! ⚠⚠ **This does NOT reuse `app::visualize::edge_view`, and the roadmap's M7.4 note that it
//! could is wrong.** That function is `|luma − gaussian_blur|` tuned for dust spots: it carries
//! no gradient *direction*, which is exactly what makes a narrow angular window cheap and
//! specific, and it lives in the app lib where this crate cannot call it. [`super::super::spatial::sobel`]
//! is the operator this needs.

use super::guides::Guide;
use crate::spatial;

/// How far from vertical a candidate line may lean, in degrees.
///
/// ⚠⚠ **Chosen from the asymmetry of the failure modes, not for tidiness.** A missed line means
/// the button appears broken and there is nothing on screen to act on; a false positive draws a
/// **visible, editable** guide the user nudges or deletes. So the window is wide rather than
/// tight: ±20° misses exactly the shot the feature exists for — a tall building from close with
/// strong upward tilt, where the outer verticals lean 20–25° near the frame edges — while a full
/// 180° accumulator is pure waste with one family.
pub const MAX_LEAN_DEG: f32 = 30.0;

/// A gradient magnitude below this fraction of the frame's strongest edge casts no vote.
const MAG_FRACTION: f32 = 0.25;

/// How many angle bins the accumulator uses across the window.
const ANGLE_BINS: usize = 61;

/// How many offset bins the accumulator uses across the frame's diagonal.
const OFFSET_BINS: usize = 180;

/// A peak must reach this fraction of the strongest peak's vote to be a candidate at all.
const PEAK_FRACTION: f32 = 0.35;

/// Candidate near-vertical lines, best first, at most two.
///
/// The plane is single-channel luminance of a frame `w` × `h`; the returned guides are in
/// `[0, 1]` frame-normalised coordinates, ready for [`super::guides::solve`].
///
/// ⚠⚠ **Returns the two most widely SEPARATED strong lines, not the top two by vote.**
/// `guides.rs` records why: "the vanishing point of two nearly-parallel lines sits enormously far
/// away and moves wildly for a pixel of drawing error, so a roll taken from it is
/// ill-conditioned exactly when the correction is gentlest." Two near-coincident detections on
/// one edge of one building are the commonest way that happens, and taking them makes the solve
/// unstable while looking like success.
pub fn verticals(_luma: &[f32], _w: usize, _h: usize) -> Vec<Guide> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paint a line of width 2 into a dark plane, from `(x0, y0)` to `(x1, y1)` in pixels.
    fn line(p: &mut [f32], w: usize, h: usize, x0: f32, y0: f32, x1: f32, y1: f32) {
        let steps = 4 * h;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let x = x0 + (x1 - x0) * t;
            let y = y0 + (y1 - y0) * t;
            for dx in -1..=1 {
                let xi = (x.round() as i64 + dx).clamp(0, w as i64 - 1) as usize;
                let yi = (y.round() as i64).clamp(0, h as i64 - 1) as usize;
                p[yi * w + xi] = 1.0;
            }
        }
    }

    fn dark(w: usize, h: usize) -> Vec<f32> {
        vec![0.05f32; w * h]
    }

    /// The angle a guide makes with vertical, in degrees, on a square frame.
    fn lean(g: &Guide) -> f32 {
        let dx = g.to[0] - g.from[0];
        let dy = g.to[1] - g.from[1];
        dx.atan2(dy).to_degrees().abs().min(180.0 - dx.atan2(dy).to_degrees().abs())
    }

    /// ⚠⚠ **Assert the recovered GEOMETRY, not merely that two lines came back.** `guides.rs`
    /// records the sibling failure directly: `a_horizontal_family_engages_the_yaw` "only asserted
    /// both angles were *non-zero* — and a wrong angle is still an angle."
    #[test]
    fn two_converging_verticals_are_found_at_the_angles_they_were_drawn() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // Converging: the left edge leans right, the right edge leans left.
        line(&mut p, w, h, 60.0, 250.0, 90.0, 10.0);
        line(&mut p, w, h, 200.0, 250.0, 170.0, 10.0);
        let found = verticals(&p, w, h);
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
        for g in &found {
            assert!(
                g.family() == super::super::guides::Family::Vertical,
                "a detected line was not classified vertical: {g:?}"
            );
            // Drawn at atan(30/240) = 7.1° from vertical.
            let l = lean(g);
            assert!(
                (l - 7.1).abs() < 4.0,
                "expected ~7.1° of lean, got {l} for {g:?}"
            );
        }
        // And they really are the two different edges, not one edge twice.
        let mid = |g: &Guide| (g.from[0] + g.to[0]) / 2.0;
        assert!(
            (mid(&found[0]) - mid(&found[1])).abs() > 0.2,
            "the two lines are the same edge: {:?} and {:?}",
            found[0],
            found[1]
        );
    }

    /// ⚠⚠ **The separation rule, and this is the ONLY test that can see it.** A "top two by vote"
    /// implementation passes every test that merely counts results. The fixture is built so the
    /// wrong answer is *attractive*: two very strong near-coincident edges on the left, and one
    /// weaker edge far to the right. The right answer takes one of the pair plus the distant one.
    ///
    /// ⚠ The `spot_source` lesson: "each placed its fixture where a wrong implementation had
    /// nothing to reach for." This one does the opposite.
    #[test]
    fn the_two_lines_returned_are_separated_rather_than_the_two_strongest() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // A pair, four pixels apart — both very strong.
        line(&mut p, w, h, 40.0, 250.0, 60.0, 10.0);
        line(&mut p, w, h, 46.0, 250.0, 66.0, 10.0);
        // One distant edge, deliberately weaker (painted at 0.6 rather than 1.0).
        let mut q = dark(w, h);
        line(&mut q, w, h, 210.0, 250.0, 190.0, 10.0);
        for (a, b) in p.iter_mut().zip(&q) {
            *a = a.max(0.05 + (b - 0.05) * 0.6);
        }
        let found = verticals(&p, w, h);
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
        let mid = |g: &Guide| (g.from[0] + g.to[0]) / 2.0;
        assert!(
            (mid(&found[0]) - mid(&found[1])).abs() > 0.3,
            "the two returned lines are the near-coincident pair: {:?} and {:?}",
            found[0],
            found[1]
        );
    }

    /// The window's far side. ⚠ Both halves, because a window that accepted everything and one
    /// that accepted nothing would each pass a one-sided test.
    #[test]
    fn a_line_beyond_the_window_is_not_found_and_one_inside_it_is() {
        let (w, h) = (256, 256);
        // 25° from vertical: inside.
        let mut inside = dark(w, h);
        line(&mut inside, w, h, 70.0, 250.0, 182.0, 10.0);
        assert!(
            !verticals(&inside, w, h).is_empty(),
            "a 25-degree line inside the window was missed"
        );
        // 50° from vertical: outside.
        let mut outside = dark(w, h);
        line(&mut outside, w, h, 20.0, 250.0, 306.0, 10.0);
        let found = verticals(&outside, w, h);
        assert!(
            found.is_empty(),
            "a 50-degree line was accepted: {found:?}"
        );
    }

    /// A photograph of a wall must produce nothing, not two lines through noise.
    #[test]
    fn a_flat_frame_finds_nothing() {
        assert!(verticals(&dark(128, 128), 128, 128).is_empty());
    }

    /// ⚠ One line is not two. `guides::solve` needs exactly two per family — "one line has no
    /// vanishing point at all" — so returning a single guide is a legitimate answer the caller
    /// must be able to see, and inventing a second one would be worse than returning none.
    #[test]
    fn a_single_edge_returns_at_most_one_line() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        line(&mut p, w, h, 128.0, 250.0, 140.0, 10.0);
        assert!(verticals(&p, w, h).len() <= 1);
    }

    /// ⚠⚠ **The direction gate is load-bearing.** A frame of strong HORIZONTAL edges must cast no
    /// votes at all. Without this, the angular window is being enforced by the accumulator's
    /// geometry alone and the gradient direction — the whole reason for computing a Sobel rather
    /// than a magnitude-only high-pass — is untested.
    #[test]
    fn strong_horizontal_edges_cast_no_votes() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        for y in [40usize, 90, 140, 190] {
            for x in 0..w {
                for dy in 0..2 {
                    p[(y + dy) * w + x] = 1.0;
                }
            }
        }
        let found = verticals(&p, w, h);
        assert!(found.is_empty(), "horizontal edges were detected: {found:?}");
    }

    #[test]
    fn detect_handles_the_degenerate_sizes() {
        assert!(verticals(&[], 0, 0).is_empty());
        assert!(verticals(&[0.5], 1, 1).is_empty());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/detect-c.log 2>&1; echo "EXIT=$?"; tail -20 /tmp/detect-c.log
```

Expected: **FAIL**, `not implemented`.

- [ ] **Step 4: Implement**

Replace the stub with:

```rust
/// One accumulator peak.
struct Peak {
    angle: f32,
    offset: f32,
    votes: f32,
}

pub fn verticals(luma: &[f32], w: usize, h: usize) -> Vec<Guide> {
    if w < 8 || h < 8 || luma.len() != w * h {
        return Vec::new();
    }
    let g = spatial::sobel(luma, w, h);
    let strongest = g.mag.iter().copied().fold(0.0f32, f32::max);
    if strongest <= 1e-6 {
        return Vec::new();
    }
    let floor = strongest * MAG_FRACTION;

    let lean_max = MAX_LEAN_DEG.to_radians();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    // Half the diagonal bounds |offset|.
    let diag = (cx * cx + cy * cy).sqrt();
    let mut acc = vec![0.0f32; ANGLE_BINS * OFFSET_BINS];

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if g.mag[i] < floor {
                continue;
            }
            // ⚠⚠ **The direction gate.** A line's own direction is perpendicular to its gradient,
            // so a near-vertical LINE has a near-horizontal GRADIENT: |gy| must be small. This is
            // what a magnitude-only edge map — `visualize::edge_view` — cannot express, and it is
            // why this accumulator stays small instead of collecting every horizon and kerb.
            let lean = g.gy[i].atan2(g.gx[i].abs().max(1e-9));
            if lean.abs() > lean_max {
                continue;
            }
            // Bin the line's angle from vertical (equal to the gradient's from horizontal).
            let bin = (((lean + lean_max) / (2.0 * lean_max)) * (ANGLE_BINS - 1) as f32).round();
            let ab = bin.clamp(0.0, (ANGLE_BINS - 1) as f32) as usize;
            let theta = -lean_max + 2.0 * lean_max * ab as f32 / (ANGLE_BINS - 1) as f32;
            // Offset of the line through this pixel, measured along x at the frame's centre row.
            let off = (x as f32 - cx) - (y as f32 - cy) * theta.tan();
            let ob = (((off + diag) / (2.0 * diag)) * (OFFSET_BINS - 1) as f32).round();
            if !(0.0..=(OFFSET_BINS - 1) as f32).contains(&ob) {
                continue;
            }
            acc[ab * OFFSET_BINS + ob as usize] += g.mag[i];
        }
    }

    // Collect peaks that are local maxima in the accumulator.
    let mut peaks: Vec<Peak> = Vec::new();
    let best = acc.iter().copied().fold(0.0f32, f32::max);
    if best <= 1e-6 {
        return Vec::new();
    }
    for ab in 0..ANGLE_BINS {
        for ob in 1..OFFSET_BINS - 1 {
            let v = acc[ab * OFFSET_BINS + ob];
            if v < best * PEAK_FRACTION {
                continue;
            }
            if v < acc[ab * OFFSET_BINS + ob - 1] || v < acc[ab * OFFSET_BINS + ob + 1] {
                continue;
            }
            let theta = -lean_max + 2.0 * lean_max * ab as f32 / (ANGLE_BINS - 1) as f32;
            let off = -diag + 2.0 * diag * ob as f32 / (OFFSET_BINS - 1) as f32;
            peaks.push(Peak {
                angle: theta,
                offset: off,
                votes: v,
            });
        }
    }
    if peaks.is_empty() {
        return Vec::new();
    }
    peaks.sort_by(|a, b| b.votes.partial_cmp(&a.votes).unwrap_or(std::cmp::Ordering::Equal));

    // ⚠⚠ **Separation, not the top two by vote.** The strongest peak is taken, then the strongest
    // peak that is far enough from it — because a vanishing point from two nearly-coincident
    // lines is ill-conditioned exactly when the correction is gentlest, which `guides.rs`
    // records as having been "reported in use as the image coming out rotated so that carefully
    // drawn vertical guides ended up not vertical at all."
    let min_sep = w as f32 * 0.15;
    let mut chosen = vec![&peaks[0]];
    if let Some(p) = peaks
        .iter()
        .skip(1)
        .find(|p| (p.offset - peaks[0].offset).abs() >= min_sep)
    {
        chosen.push(p);
    }

    // A peak becomes a guide spanning the frame, in `[0, 1]`.
    chosen
        .iter()
        .map(|p| {
            let at = |y: f32| (cx + p.offset + (y - cy) * p.angle.tan()) / w as f32;
            Guide {
                from: [at(0.0), 0.0],
                to: [at(h as f32), 1.0],
            }
        })
        .collect()
}
```

- [ ] **Step 5: Run, and expect to iterate**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/detect-c.log 2>&1; echo "EXIT=$?"; grep -A6 "test result\|panicked" /tmp/detect-c.log | head -40
```

⚠⚠ **The five constants above — `MAG_FRACTION`, `ANGLE_BINS`, `OFFSET_BINS`, `PEAK_FRACTION`,
`min_sep` — are starting values, not measured ones.** Expect to adjust them so the tests pass. Two
rules while you do:

- **Never loosen an assertion to make a test pass.** CLAUDE.md records
  `texture_minus_100_removes_the_fine_band`'s threshold moving twice, "each time because the model
  changed and never to make a red test green". If a test fails, first ask whether the **assertion**
  is right — three tests in this project asserted something false about correct code.
- **Record what you changed and why**, in the constant's own doc comment. A tuned number with no
  recorded reasoning is one nobody can revisit.

- [ ] **Step 6: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Delete the direction gate (the `if lean.abs() > lean_max { continue; }`) | `strong_horizontal_edges_cast_no_votes` **and** `a_line_beyond_the_window_is_not_found_and_one_inside_it_is` |
| 2 | Take the top two by vote (`chosen = peaks[..2]`) | `the_two_lines_returned_are_separated_rather_than_the_two_strongest` |
| 3 | Widen the window to 90° | `a_line_beyond_the_window_is_not_found_…` |
| 4 | Drop the `MAG_FRACTION` floor | `a_flat_frame_finds_nothing` |
| 5 | Return a fixed pair of vertical guides | `two_converging_verticals_are_found_at_the_angles_they_were_drawn` (the lean assertion) |

⚠ For each: confirm a `test result:` line appears (so you know it compiled), confirm the **named**
test failed, revert. If a sabotage does not fail, that is a finding about the test.

- [ ] **Step 7: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "EXIT=$?"
git add crates/pipeline/src/perspective/
git commit -m "feat(pipeline): detect near-vertical lines with a Hough pass over a Sobel field"
```

---

## Task 3: `perspective_detect.rs` — the analysis image, in Guide space

**Files:**
- Create: `crates/app/src/perspective_detect.rs`
- Modify: `crates/app/src/lib.rs`

⚠⚠ **The coordinate space is the whole content of this task.** A `Guide` is stored
**post-lens-correction, post-orientation, pre-straighten, pre-perspective, pre-crop**. That follows
from two facts you can check: `guides::to_frame` maps a stored guide through the perspective
homography and the straighten angle **only**, and `panel/geometry.rs`'s
`the_guides_preview_keeps_the_correction_and_only_drops_the_crop` pins that the preview keeps the
correction and drops just the crop. Land anywhere else and every detected line needs an inverse map
that does not exist.

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`, in alphabetical position:

```rust
pub mod perspective_detect;
```

- [ ] **Step 2: Write the tests and the stub**

Create `crates/app/src/perspective_detect.rs`:

```rust
//! The glue behind the Auto button — everything except its drawing.
//!
//! ⚠ Lives in the lib, with no egui in sight, because that is where a unit-testable rule belongs:
//! `auto_tone_tool`'s pattern, which this module follows line for line.
//!
//! ⚠⚠ **The buffer it reads is `Session.buffer`, never the rendered texture.** The texture already
//! has the current perspective applied, so lines that are already corrected have no vanishing
//! point left to find — and it is a GPU handle that cannot be sampled anyway. `auto_tone_tool`
//! records the general form of the rule: analysing the render "would make the auto chase its own
//! correction."

use core_image::ImageBuf;
use edit_model::{CropRect, Edits, Geometry};
use lens_profile::LensShot;
use pipeline::perspective::detect;
use pipeline::perspective::guides::Guide;
use pipeline::{GeometryStage, LensCorrectionStage, PipelineStage, Resampler};

/// The analysis runs on a downscale to this long edge.
///
/// ⚠ `auto_tone_tool::ANALYSIS_DIM`'s value and its reasoning: 512 px keeps the button instant on
/// a 26 Mpx buffer, and a building edge that does not survive a downscale to 512 is not the edge a
/// keystone correction is about.
pub const ANALYSIS_DIM: usize = 512;

/// The luminance plane the detector runs on, in **Guide** coordinates.
pub fn analysis_luma(
    _buffer: &ImageBuf,
    _edits: &Edits,
    _lens: Option<LensShot>,
    _resampler: Resampler,
) -> (Vec<f32>, usize, usize) {
    unimplemented!()
}

/// The near-vertical lines detected in this photograph, ready for `guides::solve`.
pub fn detect_verticals(
    _buffer: &ImageBuf,
    _edits: &Edits,
    _lens: Option<LensShot>,
    _resampler: Resampler,
) -> Vec<Guide> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with two converging near-vertical bright edges, `w` × `h`.
    fn building(w: usize, h: usize) -> ImageBuf {
        let mut v = vec![0.05f32; w * h * 3];
        let mut paint = |x: usize, y: usize| {
            let i = (y * w + x) * 3;
            v[i] = 1.0;
            v[i + 1] = 1.0;
            v[i + 2] = 1.0;
        };
        for y in 0..h {
            let t = y as f32 / h as f32;
            for (a, b) in [(0.25f32, 0.35f32), (0.75, 0.65)] {
                let x = ((a + (b - a) * (1.0 - t)) * w as f32) as usize;
                for dx in 0..2 {
                    paint((x + dx).min(w - 1), y);
                }
            }
        }
        ImageBuf::from_vec(w, h, v)
    }

    fn neutral() -> Edits {
        Edits::default()
    }

    /// The baseline: with no orientation and no lens profile, the detector finds the two edges.
    #[test]
    fn two_edges_in_a_plain_frame_are_found() {
        let b = building(256, 256);
        let found = detect_verticals(&b, &neutral(), None, Resampler::default());
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
    }

    /// ⚠⚠ **Orientation is undone, and nothing else can see this.** On a quarter-turned photo the
    /// buffer's verticals are the frame's horizontals: analysed raw, the detector would find
    /// nothing at all — or worse, find the wrong family and hand `solve` a correction for an axis
    /// nobody asked about. `auto_tone_tool::source_crop`'s test is the model, and its comment is
    /// the reason: "a crop taken on the left of the *oriented* image is the *bottom* of the
    /// buffer."
    #[test]
    fn a_quarter_turn_is_undone_before_detection() {
        // A buffer whose *own* edges run horizontally, in a photo turned a quarter so they are
        // vertical on screen.
        let mut v = vec![0.05f32; 256 * 256 * 3];
        for y in [60usize, 190] {
            for x in 0..256 {
                for dy in 0..2 {
                    let i = ((y + dy) * 256 + x) * 3;
                    v[i] = 1.0;
                    v[i + 1] = 1.0;
                    v[i + 2] = 1.0;
                }
            }
        }
        let b = ImageBuf::from_vec(256, 256, v);
        let mut e = neutral();
        e.geometry.orientation.rotate_cw();
        let found = detect_verticals(&b, &e, None, Resampler::default());
        assert_eq!(
            found.len(),
            2,
            "the orientation was not undone: {} lines found",
            found.len()
        );
    }

    /// ⚠ The analysis plane is at `ANALYSIS_DIM`, so the button stays instant on a real buffer.
    #[test]
    fn the_analysis_plane_is_downscaled() {
        let b = building(ANALYSIS_DIM * 3, ANALYSIS_DIM * 2);
        let (luma, w, h) = analysis_luma(&b, &neutral(), None, Resampler::default());
        assert_eq!(luma.len(), w * h);
        assert!(
            w.max(h) <= ANALYSIS_DIM,
            "the plane was not downscaled: {w}x{h}"
        );
    }

    /// A flat frame finds nothing, all the way through the glue.
    #[test]
    fn a_flat_frame_finds_nothing_through_the_glue() {
        let b = ImageBuf::from_vec(128, 128, vec![0.3f32; 128 * 128 * 3]);
        assert!(detect_verticals(&b, &neutral(), None, Resampler::default()).is_empty());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p app --lib perspective_detect > /tmp/pd-c.log 2>&1; echo "EXIT=$?"; tail -20 /tmp/pd-c.log
```

- [ ] **Step 4: Implement**

```rust
/// The luminance plane the detector runs on, in **Guide** coordinates.
///
/// ⚠⚠ **Built by running the two REAL prefix stages that precede Guide space**, on a downscale —
/// not by a hand-rolled sampler. `LensCorrectionStage` builds its own `Corrector` from the
/// dimensions of the image it is handed (its own doc: handing one in "would make a size mismatch
/// *representable*, and it would be silently wrong rather than an error"), so running it on the
/// 512 px plane is correct by construction. `GeometryStage` applies the orientation the same way,
/// and `mod stages` is private so its `orient` is unreachable from here in any case. One rule,
/// rather than a second implementation that can disagree with the render.
///
/// ⚠⚠ **The lens step is the part that looks skippable and is not.** The prefix order in
/// `pipeline/src/lib.rs` is LensCorrection → Perspective → SpotRemoval → Geometry, so the decode
/// buffer is *pre*-lens-correction — and lens correction is on by default for RAW. On the raw
/// buffer, barrel distortion **curves every building edge**: each edge then fragments into several
/// Hough lines at slightly different angles, and the vanishing point they imply is biased. The
/// result is not an error; it is a correction that is merely *plausible*, which this project's
/// records name repeatedly as its worst failure shape.
///
/// ⚠ `angle: 0.0` and a full crop: the straighten and the crop are *downstream* of Guide space, so
/// applying them here would put the detected lines in the wrong frame.
pub fn analysis_luma(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
) -> (Vec<f32>, usize, usize) {
    let small = buffer.downscale_to(ANALYSIS_DIM);
    let small = match (edits.lens_correction.is_active(), lens) {
        (true, Some(c)) => LensCorrectionStage::new(
            c,
            lens_profile::Corrections {
                distortion: edits.lens_correction.distortion,
                tca: edits.lens_correction.tca,
                vignetting: edits.lens_correction.vignetting,
                scale: edits.lens_correction.scale,
            },
            resampler,
        )
        .apply(small),
        _ => small,
    };
    let oriented = GeometryStage::new(
        &Geometry {
            orientation: edits.geometry.orientation,
            angle: 0.0,
            crop: CropRect::default(),
        },
        resampler,
    )
    .apply(small);

    let (w, h) = (oriented.width(), oriented.height());
    let mut luma = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // ⚠ `color::luminance` — the working space's Y, "the quantity every tuned constant in
            // this pipeline is measured against" (`auto_tone`'s module doc). Not a Rec.709 copy.
            luma.push(color::luminance(oriented.pixel(x, y)));
        }
    }
    (luma, w, h)
}

/// The near-vertical lines detected in this photograph, ready for `guides::solve`.
pub fn detect_verticals(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
) -> Vec<Guide> {
    let (luma, w, h) = analysis_luma(buffer, edits, lens, resampler);
    detect::verticals(&luma, w, h)
}
```

⚠ `crates/app`'s `Cargo.toml` may need `color` and `lens-profile` as dependencies — they are almost
certainly there already (`sampler_for` uses both). Check rather than assume.

- [ ] **Step 5: Run, verify, and iterate on the fixtures if needed**

```bash
cargo test -p app --lib perspective_detect > /tmp/pd-c.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/pd-c.log
```

⚠ If `a_quarter_turn_is_undone_before_detection` fails, check **which** direction
`Orientation::rotate_cw` turns before changing the code — the fixture may be the thing that is
wrong. `every_button_matches_its_geometric_action` in `panel/geometry.rs` is the independent model.

- [ ] **Step 6: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Skip the `GeometryStage` (return `small` directly) | `a_quarter_turn_is_undone_before_detection` |
| 2 | Skip the downscale (`let small = buffer.clone();`) | `the_analysis_plane_is_downscaled` |
| 3 | Apply `edits.geometry` whole instead of orientation-only | ⚠ **may be inert** with a neutral fixture — if so, say so, and note that the guard is defence in depth. An inert sabotage is not evidence. |

⚠ Sabotage 3 is the **lens step's** sibling and the one that matters most in production. **You cannot
test it with a synthetic fixture, because none of them has a lens profile.** Do not invent one that
appears to; instead verify it on the real RAW in Task 5 and report the measurement. CLAUDE.md's
precedent for exactly this honesty is the `capture_days` entry: *"no test can distinguish the two,
and inventing one that appeared to would be worse than saying so."*

- [ ] **Step 7: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "EXIT=$?"
git add crates/app/src/perspective_detect.rs crates/app/src/lib.rs crates/app/Cargo.toml
git commit -m "feat(app): build the vertical-detection analysis image in Guide space"
```

---

## Task 4: the Auto button

**Files:**
- Modify: `crates/app/src/interaction.rs`
- Modify: `crates/app/src/develop/panel/geometry.rs`

⚠ **One commit.** `auto_guides` is a `pub(crate)` method on a bin-crate type with no caller until
the button exists — the bin-crate dead-code trap, which CLAUDE.md records as having bitten **ten
times**. `clippy -D warnings` will reject it if you split them.

- [ ] **Step 1: Add the method beside `solve_guides`**

In `crates/app/src/interaction.rs`, immediately **after** `solve_guides` (which ends around line
195), inside the same `impl` block:

```rust
    /// Detect the verticals, write them as guides, and solve — the Auto button.
    ///
    /// Returns whether anything changed.
    ///
    /// ⚠ Beside `solve_guides` because it is the same operation with the guides supplied by a
    /// detector rather than by a drag, and it reuses that solver rather than repeating it.
    pub(crate) fn auto_guides(&mut self) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let found = rustroom_app::perspective_detect::detect_verticals(
            &s.buffer,
            &s.edits,
            s.lens.shot.clone(),
            s.resampler,
        );
        // ⚠⚠ **Two are required, and fewer must SAY SO rather than doing nothing.**
        // `guides::solve` needs exactly two per family — "one line has no vanishing point at all"
        // — and a button that silently does nothing is the worst outcome this feature can have.
        // The status cluster is where this app already reports what it just did.
        if found.len() < 2 {
            self.status = format!(
                "Found {} vertical{} — need two. Draw them by hand in Guide Mode.",
                found.len(),
                if found.len() == 1 { "" } else { "s" }
            );
            return false;
        }
        s.guides = found;
        s.active_guide = None;
        // ⚠⚠ **Guide Mode must go on, and this is not a convenience.** `canvas/mod.rs` draws the
        // guide overlay only under `Tool::Crop && s.show_guides`, so without this the inferred
        // guides are invisible and the milestone's "Done when" — "the guides it inferred are
        // visible and editable" — is not met.
        s.show_guides = true;
        let solved = self.solve_guides();
        if let Some(s) = self.session.as_mut() {
            let tool = self.tool;
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            // ⚠ One push, so one Auto click is one Ctrl+Z — the rule the guide drag's own caller
            // in `canvas/mod.rs::show` already follows.
            s.history.push(s.edits.clone());
        }
        self.status = "Perspective solved from two detected verticals.".to_string();
        solved
    }
```

⚠ `let tool = self.tool;` must be hoisted **before** `self.session.as_mut()` if the borrow checker
objects — `self.tool` is an immutable borrow of `self` and the session is a mutable one, which is
the `E0502` this codebase records at thirteen sites. `Tool` is `Copy`, so the hoist is free.

- [ ] **Step 2: Add the button**

In `crates/app/src/develop/panel/geometry.rs`, inside the `ui.horizontal(|ui| { … })` of the
**Guides** block, after the `Clear` button (~line 240), add:

```rust
        // ⚠ In the Guides block beside Clear, because what it produces *is* guides — the same
        // objects the overlay draws and the same ones Clear removes. Not beside the perspective
        // sliders, which are the solve's output rather than its input.
        if ui
            .button("Auto")
            .on_hover_text(
                "Find two vertical edges in the photograph and solve the tilt from them. The \
                 guides it infers are left on screen so you can nudge or delete them.",
            )
            .clicked()
        {
            auto = true;
        }
```

`panel::geometry::show` cannot call `RustroomApp` methods — it takes `&mut Session`. So the click
must be reported outward. Add an `auto: &mut bool` parameter to `geometry::show`, set it here, and
in `develop/panel/mod.rs`'s caller run:

```rust
    // ⚠ Run OUTSIDE the session borrow: `auto_guides` needs `&mut RustroomApp`, which
    // `&mut app.session` rules out — the same reason `presets::show` is drawn outside it.
    if auto {
        photo_edited |= app.auto_guides();
    }
```

⚠ Follow whatever shape `panel/mod.rs` already uses for a click that needs the app; read it before
adding a parameter, and if there is an established channel use that instead. **Report which you
chose.**

- [ ] **Step 3: Write the wiring tests**

⚠⚠ **The wiring half is what this project has repeatedly shipped broken.** In
`crates/app/src/develop/panel/geometry.rs`'s `mod tests`:

```rust
    /// ⚠ The button exists and is reachable. A test on `auto_guides` alone cannot see that
    /// nothing calls it — which is the defect shape this project's records name most often.
    #[test]
    fn the_auto_button_is_drawn_in_the_guides_block() {
        let mut s = session();
        let mut auto = false;
        // Drive the panel and click it; `click_in_panel`'s harness is the model.
        let _ = click_in_panel_for_auto("Auto", &mut s, &mut auto);
        assert!(auto, "clicking Auto reported nothing to the caller");
    }
```

…where `click_in_panel_for_auto` mirrors the existing `click_in_panel` helper with the extra
`&mut bool`. **Read `click_in_panel` and extend it rather than writing a second harness** — a helper
duplicated per test is how two harnesses come to disagree.

And in `crates/app/src/interaction.rs`'s test module (create one if there is none):

```rust
    /// ⚠⚠ Auto must write guides AND switch Guide Mode on. A test asserting only the guides
    /// passes against a version whose output is invisible on the canvas — which is the whole
    /// "Done when".
    #[test]
    fn auto_writes_guides_and_shows_them() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        // ⚠ A fixture the detector can actually find lines in: `open_test_photo`'s buffer is
        // flat. Replace its buffer with a two-edge frame, or assert the *fewer-than-two* path
        // and cover the success path in the GUI run instead — and SAY WHICH you did.
        crate::test_app::open_test_photo(&mut app, &ctx, 256, 256);
        let _ = app.auto_guides();
        // On a flat fixture the honest assertion is the reported failure, not silence.
        assert!(
            !app.status.is_empty(),
            "a detection that found nothing said nothing"
        );
    }

    /// ⚠ Fewer than two detected must set the status rather than doing nothing silently.
    #[test]
    fn finding_fewer_than_two_verticals_reports_it() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        assert!(!app.auto_guides(), "a flat photo solved something");
        assert!(app.status.contains("need two"), "status: {}", app.status);
        assert!(
            app.session.as_ref().expect("a session").guides.is_empty(),
            "guides were written despite no detection"
        );
    }
```

⚠ **If `test_app::open_test_photo`'s buffer cannot be given real edges without touching a file you
do not own, do not force it.** Cover the fewer-than-two path in the suite, cover the success path in
the GUI run of Task 5, and **say exactly that in your report.** Inventing a fixture that appears to
test the success path is worse than a stated gap.

- [ ] **Step 4: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Remove `s.show_guides = true;` | `auto_writes_guides_and_shows_them` (extend it to assert `show_guides` once you have a fixture that detects) |
| 2 | Replace the fewer-than-two branch with a bare `return false;` | `finding_fewer_than_two_verticals_reports_it` |
| 3 | Write the guides **before** the length check | the same test, on `guides.is_empty()` |
| 4 | Delete the `auto = true;` in the button | `the_auto_button_is_drawn_in_the_guides_block` |
| 5 | Remove the `s.history.push` | add an assertion for `can_undo` if none covers it |

- [ ] **Step 5: Full suite, goldens, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "EXIT=$?"
cargo test --all > /tmp/test-c.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-c.log | tail -5
git status --short -- crates/pipeline/tests
git add -A
git commit -m "feat(develop): an Auto button that solves perspective from detected verticals"
```

Expected: the test count is **1815 plus your new ones**. ⚠ Assert the count, not "still green" —
W0's rule.

---

## Task 5: verify, measure a real photograph, report

- [ ] **Step 1: The three gates, exit codes read from files**

```bash
cargo fmt --check > /tmp/fmt-c.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/test-c.log 2>&1; echo "TEST=$?"; grep "test result" /tmp/test-c.log
```

All three must print `=0`. ⚠ `cargo test` being green is **not** evidence about clippy — seven
recorded instances in this project.

- [ ] **Step 2: Goldens byte-identical**

```bash
git diff --stat $(git merge-base main HEAD) -- crates/pipeline/tests
```

Expected: **empty**. ⚠ Use `git merge-base main HEAD`, not `git diff main` — a plain diff shows
another branch's merged changes and reads as a violation. §3 records this trap and B is the first
track to merge, so it *will* bite you.

- [ ] **Step 3: ⚠⚠ Confirm you touched no canvas file**

```bash
git diff --stat $(git merge-base main HEAD) -- crates/app/src/develop/canvas/
```

Expected: **empty**. `canvas/**` is barred to this track outright.

- [ ] **Step 4: ⚠⚠ Measure the lens step on a real RAW**

This is the one decision no synthetic fixture can check (Task 3, sabotage 3). Write a **kept probe
example** — `crates/app/examples/detect_probe.rs`, following `curve_probe` / `sat_probe` /
`grade_probe`'s precedent — that decodes a real RAW and prints the detected guides **with lens
correction on and off**:

```bash
cargo run --release -p app --example detect_probe -- "$RAW_FIXTURE" > /tmp/probe-c.log 2>&1; echo "EXIT=$?"; cat /tmp/probe-c.log
```

Report the two sets of endpoints and the angles they imply. ⚠ **If they are identical, either the
lens step is not being applied or the lens has negligible distortion — find out which and say so.**
A number that is real but is not a measurement of what you think you measured is the trap §3
records.

- [ ] **Step 5: ⚠⚠ RUN IT IN THE GUI, on a real off-axis building**

```bash
cargo run --release -p app
```

1. Open a photograph of a building **shot looking up**, so its verticals converge.
2. Select **Geometry** (`2`). Press **Auto**.
   - The perspective sliders move; the photo de-keystones.
   - **Guide Mode switches on and two guides are visible.**
   - Drag a guide endpoint: the correction re-solves on release.
   - The status line says what happened.
3. **Ctrl+Z once** — the whole correction goes, in one step.
4. Press **Clear** — the guides go, the sliders stay (that is the existing recorded behaviour).
5. Press **Auto** on a photograph with **no verticals at all** — a landscape, a portrait. It must
   report "need two" in the status line, not sit silent and not correct anything.
6. Press **Auto** twice in a row. The second press must not compound the first into nonsense.
7. Try it on a bookshelf or a fence. ⚠ It will very likely find those — that is caveat 4, not a bug,
   and the guides being editable is the mitigation. **Report what you saw.**

- [ ] **Step 6: Produce the before/after and STOP**

Export or screenshot the building frame **before** and **after** one Auto press. ⚠⚠ **Do not sign it
off.** §6: *"An agent can produce the comparison; it should not sign it off."* Hand the user the two
images and the numbers, and say plainly that the judgement is theirs.

- [ ] **Step 7: Report**

Your report must say:

- **The before/after images and the angles Auto produced**, presented for the user's judgement —
  explicitly not signed off by you.
- **The `detect_probe` measurement** with lens correction on and off, and what you concluded.
- **What you ran in the GUI** for each of the seven checks above.
- The **test count before and after** (1815 → your number), not "still green".
- **That the 53 goldens are byte-identical**, and that `canvas/**` is untouched.
- **`panel/geometry.rs` is 742 lines** — named per §5.
- **The five tuned constants** in `detect.rs`, their final values, and what you changed them from
  and why.
- **Whether the success path is covered by a test or only by the GUI run** — say which, honestly.
- **Any sabotage that did not fail, and any that was inert.** Sabotage 3 in Task 3 is expected to be
  inert against synthetic fixtures; saying so is worth more than a clean-looking table.
- **Anything this plan did not settle.** ⚠ Report it, do not silently pick — §6 exists because "an
  agent given an open design choice does not stall; it picks one silently and the choice ships."
- ⚠ If the instructions genuinely conflict — if `cargo test --all` cannot be green without touching a
  barred file — **choose the green gate and say so.** W3 has two recorded precedents of exactly that
  being right, both flagged rather than hidden.

**Do not push.** The orchestrator merges **B → D → C**; you are last, so expect to rebase onto B's
`panel/geometry.rs` change.

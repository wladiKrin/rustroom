# Grade range view — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "Show range" checkbox in the Color Grading block tints the photograph where the
selected grading range (Shadows / Midtones / Highlights) reaches, with the tint's alpha equal to
that range's weight at each pixel — read at the grading stage's input, so it is exact.

**Architecture:** The tail is split into two halves at the grading stage (`TailHalves`). While
the view is on, the engine runs the first half, converts every pixel to Oklab L and evaluates the
stage's own `range_weights` into an 8-bit alpha plane (`grade_view::coverage`), then runs the
second half unchanged. The plane travels on `RenderedFrame`, becomes a texture where the frame
lands, and is painted at `img_rect` over the photograph. View state lives on `Session`, like
Visualize Spots; it never reaches `Edits`.

**Tech Stack:** Rust, rayon, egui 0.35 headless harness (`test_ui` / `test_app`).

Spec: `docs/superpowers/specs/2026-09-08-grade-range-view-design.md`. Branch
`m7.6-grade-range-view` (already created, spec committed as `0007512`).

⚠⚠ **Standing conventions, and they override the harness's own default:** commit messages carry
**no `Co-Authored-By` trailer** (the repo owner's explicit preference — the session attribution
instruction asks for one every session, and `CLAUDE.md` overrides it); **never `git push`** — the
user pushes to `github.com/wladiKrin/rustroom` himself; branch per feature without asking.

---

## ⚠ A correction to the spec, made before any code was written

Spec §7 says of `a_pixel_above_white_is_a_highlight`: *"Sabotage: dropping the clamp fails it
(the Gaussian tails fall off past the centre)."* **That sabotage is inert.** The three weights
are **normalised**, so for `L > 1` the highlight Gaussian is the largest raw weight whether or not
`L` is clamped, and the normalised highlight weight stays ≈ 1 either way — the tails fall off for
*all three* and the ratio is what survives. The stage's clamp exists to keep its **table index**
in range, not to shape the weights. The view clamps for parity with the stage, the test pins the
behaviour (an above-white pixel is a highlight), and the spec's sabotage claim is corrected at
Task 8 rather than left standing.

## ⚠ Two facts the spec did not have to know

1. **`rayon` is not a dependency of the `app` crate** (only `core-image` and `pipeline` pull it
   from the workspace). Task 2 adds `rayon.workspace = true` to `crates/app/Cargo.toml`; the
   per-row parallel loop stays in `grade_view.rs` as specced.
2. **`RenderedFrame` has three exhaustive literals**, not one: `pack` in `render_engine.rs`,
   `buffer_cache.rs:92` and `test_app.rs:313`. All three gain `grade_view: None` (Task 3), and the
   engine attaches the plane *after* `pack` rather than threading another parameter through it.

---

## File structure

| file | what changes | lines now |
|---|---|---|
| `crates/pipeline/src/lib.rs` | `TailHalves`, `build_pipeline_split_tapped`; the existing builder becomes a wrapper; one test | large |
| `crates/app/Cargo.toml` | `rayon.workspace = true` | — |
| `crates/app/src/lib.rs` | `pub mod grade_view;` | — |
| `crates/app/src/grade_view.rs` | **new**: `GradeRangeView`, `coverage`, `tint_rgba`, four tests | new |
| `crates/app/src/render_engine.rs` | `FrameSpec.grade_view`, `RenderedFrame.grade_view`, the tap in `render_cached`, four tests | **2594 ⚠** |
| `crates/app/src/buffer_cache.rs`, `crates/app/src/test_app.rs`, `crates/app/src/multi/pane.rs` | literal fields | — |
| `crates/app/src/main.rs` | three `Session` fields | 917 |
| `crates/app/src/session_flow.rs` | `request` passes the view; open literal | 1065 |
| `crates/app/src/interaction.rs` | `select_tool` clears the view; one test | 570 |
| `crates/app/src/develop/panel/color_wheel.rs` | `GradeRange::view`; one test | 598 |
| `crates/app/src/develop/panel/adjustments.rs` | the checkbox and derivation; two tests | **989 ⚠** |
| `crates/app/src/develop/panel/mod.rs` | passes the two session fields | 472 |
| `crates/app/src/shell.rs` | the texture landing | **1028 ⚠** |
| `crates/app/src/develop/canvas/mod.rs` | paint at `img_rect`; one test | **1697 ⚠** |
| `crates/app/examples/bench_grade_view.rs` | **new**: the cost measurement | new |
| `CLAUDE.md`, `docs/BACKLOG.md`, the spec, the vault's `03-plan-of-action.md` | records | — |

⚠ Four files already exceed the ~600-line marker (`render_engine.rs`, `adjustments.rs`,
`shell.rs`, `canvas/mod.rs`). Each gains a few lines at an existing seam and the logic lives in
`grade_view.rs`; reported in the final message rather than silently added to.

---

### Task 1: The tapped tail in `pipeline`

**Files:**
- Modify: `crates/pipeline/src/lib.rs` — `build_pipeline_split_in_region_with_brush` (line ~611)
  and its tests module (line ~1615)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module of `crates/pipeline/src/lib.rs`, directly after
`build_pipeline_is_exactly_prefix_then_tail`:

```rust
    /// ⚠⚠ The tapped tail is the tail, cut in two at the grading stage — for a grade that is
    /// active AND for one that is not, because Blending and Balance define the ranges whether or
    /// not a wheel is turned, and the split must be structural rather than "find the stage
    /// named color_grading". Sabotage: cutting one stage late fails the second assertion.
    #[test]
    fn the_tapped_tail_concatenates_to_the_tail_and_splits_at_the_grade() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
            let (tapped_prefix, halves) = build_pipeline_split_tapped(
                &edits,
                RenderScale::FULL,
                None,
                None,
                RenderSpec::default(),
                None,
                ProfileRendering::default(),
            );
            assert_eq!(tapped_prefix.stage_names(), prefix.stage_names());
            let mut joined = halves.before_grade.stage_names();
            joined.extend(halves.from_grade.stage_names());
            assert_eq!(joined, tail.stage_names(), "the halves are not the tail");
            assert!(
                !halves.before_grade.stage_names().contains(&"color_grading"),
                "the grade is in the first half"
            );
            if edits.color_grading.is_active() {
                assert_eq!(
                    halves.from_grade.stage_names().first(),
                    Some(&"color_grading"),
                    "an active grade must open the second half"
                );
            }
        }
        // The fixture must exercise both branches, or the `if` above is never taken.
        assert!(all_edits().color_grading.is_active());
        assert!(!Edits::default().color_grading.is_active());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p pipeline --lib the_tapped_tail_concatenates 2>&1 | tail -5`
Expected: compile error — `build_pipeline_split_tapped` and `TailHalves` do not exist.

- [ ] **Step 3: Implement the tapped builder**

In `crates/pipeline/src/lib.rs`, rename the body of `build_pipeline_split_in_region_with_brush`
to a new function and add the struct. Concretely: change the existing function's signature line

```rust
pub fn build_pipeline_split_in_region_with_brush(
```

to

```rust
/// The tail in two halves, split exactly where the grading stage goes.
///
/// ⚠ The split exists whether or not `ColorGradingStage` is active: Blending and Balance define
/// the ranges regardless, so "before the grade" is a structural position — after HSL, before the
/// masks loop — not a search for a stage name. The grade range view taps the buffer here.
pub struct TailHalves {
    /// Everything up to but excluding colour grading.
    pub before_grade: Pipeline,
    /// Colour grading (when active) and everything after it, ending with the output transform.
    pub from_grade: Pipeline,
}

/// As [`build_pipeline_split_in_region_with_brush`], with the tail returned in two halves.
///
/// ⚠ The untapped builder is a wrapper over this one that concatenates the halves, so every
/// existing caller — and every golden — is byte-identical by construction.
pub fn build_pipeline_split_tapped(
```

Keep the argument list and the body exactly as they are, with **two** edits inside the body:

(a) Immediately before the line `if edits.color_grading.is_active() {` (the grading push,
~line 716), insert:

```rust
    // ⚠ The tap point. Everything pushed so far is the first half.
    let before_grade = std::mem::take(&mut tail);
```

(b) Replace the function's final line `(Pipeline::new(prefix), Pipeline::new(tail))` with:

```rust
    (
        Pipeline::new(prefix),
        TailHalves {
            before_grade: Pipeline::new(before_grade),
            from_grade: Pipeline::new(tail),
        },
    )
```

Then add the wrapper directly below the tapped function:

```rust
pub fn build_pipeline_split_in_region_with_brush(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    brush: Option<&BrushCoverage>,
    spec: RenderSpec,
    lens: Option<lens_profile::LensShot>,
    profile: ProfileRendering<'_>,
) -> (Pipeline, Pipeline) {
    let (prefix, halves) =
        build_pipeline_split_tapped(edits, scale, region, brush, spec, lens, profile);
    let mut tail = halves.before_grade.into_stages();
    tail.extend(halves.from_grade.into_stages());
    (prefix, Pipeline::new(tail))
}
```

⚠ The original doc comment on `build_pipeline_split_in_region_with_brush` (whatever text sits
above the signature today) moves with the wrapper, so the public function keeps its documentation.
Also check whether `RenderSpec` derives `Default` and `ProfileRendering::default()` exists — both
are used by `build_pipeline_spec` already, so they do.

- [ ] **Step 4: Run the test and the pipeline suite**

Run: `cargo test -p pipeline 2>&1 | grep -E "^test result|FAILED|panicked" | head`
Expected: every `test result` line `ok`, including the goldens (`golden_*` tests in this crate).

- [ ] **Step 5: Sabotage it**

Move the `let before_grade = std::mem::take(&mut tail);` line to *after* the grading `if` block.
Run: `cargo test -p pipeline --lib the_tapped_tail_concatenates 2>&1 | grep -E "panicked|ok"`
Expected: FAIL with `an active grade must open the second half`. Restore the line, re-run,
expected PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "Tail halves: the tail split at the grading stage, with the old builder a wrapper"
```

---

### Task 2: `grade_view` — the plane and the tint

**Files:**
- Modify: `crates/app/Cargo.toml` (add `rayon.workspace = true` under `[dependencies]`)
- Modify: `crates/app/src/lib.rs` (add `pub mod grade_view;` in alphabetical order, after
  `pub mod full_res;`)
- Create: `crates/app/src/grade_view.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/grade_view.rs` with only the tests first:

```rust
//! The grade range view: where a colour-grading range reaches on the photograph.
//!
//! ⚠ A **view mode, not an edit** — `visualize`'s rule. It never reaches `Edits`, the sidecar,
//! an export or the CLI. The engine takes the plane at the grading stage's *input* (the tail's
//! `before_grade` half), which is the only place the ranges are exact: everything after it —
//! the grade's own luminance offsets, the masks, the display rendering S-curve, the creative
//! LUT — moves every pixel's lightness.

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    fn buf_of(pixels: &[[f32; 3]]) -> ImageBuf {
        let mut b = ImageBuf::new(pixels.len(), 1);
        for (i, p) in pixels.iter().enumerate() {
            b.set_pixel(i, 0, *p);
        }
        b
    }

    /// For a grey, Oklab L is Y^(1/3) to within 1e-6 (the ProPhoto→LMS rows sum to 1), which
    /// gives an expected value derived *without* `prophoto_to_oklab` — so this is not the
    /// function checked against itself.
    #[test]
    fn a_grey_ramp_matches_the_stages_weights() {
        let greys = [0.0f32, 0.02, 0.1, 0.18, 0.5, 0.9, 1.0];
        let buf = buf_of(&greys.map(|y| [y, y, y]));
        for range in GradeRangeView::ALL {
            for blending in [0.0f32, 50.0, 100.0] {
                for balance in [-60.0f32, 0.0, 60.0] {
                    let plane = coverage(&buf, range, blending, balance);
                    assert_eq!(plane.len(), greys.len());
                    for (i, y) in greys.iter().enumerate() {
                        let l = y.cbrt().clamp(0.0, 1.0);
                        let w = pipeline::ColorGradingStage::range_weights(l, blending, balance)
                            [range.index()];
                        let expected = (w * 255.0).round() as i32;
                        assert!(
                            (plane[i] as i32 - expected).abs() <= 1,
                            "{range:?} blending {blending} balance {balance} grey {y}: \
                             plane {} vs {expected}",
                            plane[i]
                        );
                    }
                }
            }
        }
    }

    /// ⚠⚠ The ranges are selected on Oklab L, NOT luminance — M3.3's rule. A saturated blue has
    /// an L well below its luminance's cube root: this pixel is a shadow by L and a midtone by
    /// luminance. Sabotage: `color::luminance(px).cbrt()` in `coverage` fails it.
    #[test]
    fn a_chromatic_pixel_is_placed_by_oklab_lightness() {
        let px = [0.01f32, 0.025, 0.45];
        // The premise, asserted rather than trusted: the two routes disagree on the range.
        let by_l = pipeline::ColorGradingStage::range_weights(
            color::oklab::prophoto_to_oklab(px)[0],
            0.0,
            0.0,
        )[0];
        let by_y =
            pipeline::ColorGradingStage::range_weights(color::luminance(px).cbrt(), 0.0, 0.0)[0];
        assert!(by_l > 0.9, "premise: by L this is a shadow, got {by_l}");
        assert!(by_y < 0.1, "premise: by luminance it is not, got {by_y}");

        let plane = coverage(&buf_of(&[px]), GradeRangeView::Shadows, 0.0, 0.0);
        assert!(plane[0] > 230, "the view did not follow Oklab L: {}", plane[0]);
    }

    /// A scene-referred pixel above white (`L > 1`) is a pure highlight, as the stage reads it.
    /// ⚠ The clamp in `coverage` mirrors the stage's table-index clamp for parity; the weights
    /// are normalised, so this test pins the *behaviour*, not the clamp.
    #[test]
    fn a_pixel_above_white_is_a_highlight() {
        let buf = buf_of(&[[1.6, 1.6, 1.6]]);
        assert_eq!(coverage(&buf, GradeRangeView::Highlights, 0.0, 0.0)[0], 255);
        assert_eq!(coverage(&buf, GradeRangeView::Shadows, 0.0, 0.0)[0], 0);
    }

    #[test]
    fn tint_rgba_is_the_fixed_tint_with_alpha_from_the_weight() {
        let rgba = tint_rgba(&[0, 255, 128]);
        assert_eq!(
            rgba,
            vec![0, 190, 210, 0, 0, 190, 210, 127, 0, 190, 210, 64],
            "one RGBA quad per byte: TINT, alpha = weight / 2"
        );
    }

    #[test]
    fn an_empty_buffer_gives_an_empty_plane() {
        assert!(coverage(&ImageBuf::new(0, 0), GradeRangeView::Midtones, 50.0, 0.0).is_empty());
    }
}
```

Add `pub mod grade_view;` to `crates/app/src/lib.rs` and `rayon.workspace = true` to
`crates/app/Cargo.toml` under `[dependencies]` (after `chrono = "0.4.45"`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --lib grade_view 2>&1 | tail -5`
Expected: compile error — `GradeRangeView`, `coverage`, `tint_rgba` not found.

- [ ] **Step 3: Implement**

Insert above the `#[cfg(test)]` block in `crates/app/src/grade_view.rs`:

```rust
use core_image::ImageBuf;
use rayon::prelude::*;

/// Which range the view shows.
///
/// ⚠ Three, not four: Global is a constant added after the weighted sum, not a range, so there
/// is nothing of it to show. The panel's four-way selector maps onto this with `Global => None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GradeRangeView {
    Shadows,
    Midtones,
    Highlights,
}

impl GradeRangeView {
    pub const ALL: [GradeRangeView; 3] = [Self::Shadows, Self::Midtones, Self::Highlights];

    /// The index into `range_weights`'s `[shadows, midtones, highlights]`.
    pub fn index(self) -> usize {
        match self {
            Self::Shadows => 0,
            Self::Midtones => 1,
            Self::Highlights => 2,
        }
    }
}

/// The tint the plane is painted in: teal. ⚠ Not red — the brush tint is red and the two must
/// never be confusable — and not one of the four theme accents, so it cannot read as active
/// state. Off the skin-tone axis, for the reason the accent list excludes amber.
pub const TINT: [u8; 3] = [0, 190, 210];

/// One byte per pixel: the selected range's weight at that pixel's Oklab lightness, 0..=255.
///
/// ⚠⚠ Two things in the per-pixel line are load-bearing. **`prophoto_to_oklab`, the ProPhoto
/// pair** — the buffer is in the working space, and the stage records that mixing the pairs
/// "produces a plausible-looking colour shift that is very hard to attribute later"; here it
/// would misplace the ranges plausibly. **The clamp on the index, as the stage's table lookup
/// does** — a pixel above white reads as a highlight for both, so the two agree on the
/// brightest pixels, which are the ones a highlight wheel is for.
///
/// ⚠ `range_weights` is the stage's own, never a copy: the stage, the panel strip and this view
/// therefore cannot drift.
pub fn coverage(buf: &ImageBuf, range: GradeRangeView, blending: f32, balance: f32) -> Vec<u8> {
    let (w, h) = (buf.width(), buf.height());
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let k = range.index();
    let mut out = vec![0u8; w * h];
    // Parallel over rows: each row is a substantial task and reads nothing but itself, so the
    // result is identical to the sequential loop.
    out.par_chunks_mut(w)
        .zip(buf.data().par_chunks(w * 3))
        .for_each(|(row_out, row_in)| {
            for (dst, px) in row_out.iter_mut().zip(row_in.as_chunks::<3>().0) {
                let l = color::oklab::prophoto_to_oklab(*px)[0].clamp(0.0, 1.0);
                let wgt = pipeline::ColorGradingStage::range_weights(l, blending, balance)[k];
                *dst = (wgt * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });
    out
}

/// The plane as RGBA bytes for a texture: [`TINT`] with alpha `weight / 2`, so full membership
/// reads as a 50% tint and a soft range edge reads as a soft edge — the brush tint's rule.
pub fn tint_rgba(plane: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(plane.len() * 4);
    for &a in plane {
        out.extend_from_slice(&[TINT[0], TINT[1], TINT[2], a / 2]);
    }
    out
}
```

⚠ `ImageBuf::data()` is interleaved RGB `f32`, three per pixel (`CHANNELS` = 3); `as_chunks` is
stable in this workspace (`to_rgba8` already uses it).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p app --lib grade_view 2>&1 | grep -E "^test |test result"`
Expected: 5 passed.

- [ ] **Step 5: Sabotage it**

In `coverage`, replace `color::oklab::prophoto_to_oklab(*px)[0]` with
`color::luminance(*px).cbrt()`. Run: `cargo test -p app --lib grade_view 2>&1 | grep -E "panicked|FAILED|ok"`
Expected: `a_chromatic_pixel_is_placed_by_oklab_lightness` FAILS with `the view did not follow
Oklab L`; the grey ramp still passes (for greys the two agree — which is why the chromatic test
exists). Restore, re-run, all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/app/Cargo.toml crates/app/src/lib.rs crates/app/src/grade_view.rs
git commit -m "grade_view: the range weight plane at the grading stage's input, and its tint"
```

---

### Task 3: The engine carries the view and taps the tail

**Files:**
- Modify: `crates/app/src/render_engine.rs` — `RenderedFrame` (line ~12), `FrameSpec` (~100),
  `FrameSpec::fit` (~139), `pack` (~503), `render_cached` (~838 and ~981–1000), the tests module
  (five `FrameSpec {` literals at ~1132, ~1692, ~1807, ~2326, ~2569; and new tests)
- Modify: `crates/app/src/buffer_cache.rs:92`, `crates/app/src/test_app.rs:313` (`RenderedFrame`
  literals), `crates/app/src/multi/pane.rs:138`, `crates/app/src/session_flow.rs:107` and `:386`
  (`FrameSpec` literals)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/app/src/render_engine.rs`, after the soft-proofing block's
last test (search for `// ---- soft proofing` and add a new block after that section's final
`#[test]`):

```rust
    // ---- the grade range view ------------------------------------------------------------

    use crate::grade_view::GradeRangeView;

    /// Six greys spanning black to above white, a saturated blue and a red — 8×1.
    fn ramp_fixture() -> Arc<ImageBuf> {
        let px = [
            [0.005f32, 0.005, 0.005],
            [0.05, 0.05, 0.05],
            [0.18, 0.18, 0.18],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [1.4, 1.4, 1.4],
            [0.01, 0.025, 0.45],
            [0.5, 0.1, 0.1],
        ];
        let mut buf = ImageBuf::new(px.len(), 1);
        for (i, p) in px.iter().enumerate() {
            buf.set_pixel(i, 0, *p);
        }
        Arc::new(buf)
    }

    /// A grade that is active (saturation on all three wheels), with `luminance` on every wheel
    /// set to `lum` — the offset that moves L *after* the tap.
    fn graded(lum: f32, rendering: Option<edit_model::Rendering>) -> Edits {
        let wheel = |hue: f32| edit_model::GradeWheel {
            hue,
            saturation: 70.0,
            luminance: lum,
        };
        Edits {
            color_grading: edit_model::ColorGrading {
                shadows: wheel(240.0),
                midtones: wheel(120.0),
                highlights: wheel(45.0),
                ..edit_model::ColorGrading::default()
            },
            rendering,
            ..Default::default()
        }
    }

    /// Render through the cached path at the fixture's own resolution, with the view.
    fn viewed(
        buf: &Arc<ImageBuf>,
        edits: &Edits,
        grade_view: Option<GradeRangeView>,
        region: Option<RegionRect>,
    ) -> RenderedFrame {
        render_cached(
            buf,
            edits,
            FrameSpec {
                max_dim: buf.width().max(buf.height()),
                region,
                display_space: color::OutputSpace::Srgb,
                resampler: pipeline::Resampler::default(),
                visualize: None,
                soft_proof: None,
                grade_view,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    fn plane(f: &RenderedFrame) -> Vec<u8> {
        f.grade_view
            .as_ref()
            .map(|p| p.as_ref().clone())
            .expect("the frame carries no plane")
    }

    /// ⚠⚠ The view is an overlay, never a change to the photograph: pixels, sRGB channel and
    /// histogram are byte-identical with the view on and off. Sabotage: painting the tint into
    /// `out` fails it. And `None` is the old path — no plane at all.
    #[test]
    fn the_grade_view_does_not_touch_the_photograph() {
        let buf = ramp_fixture();
        let edits = graded(0.0, None);
        let off = viewed(&buf, &edits, None, None);
        let on = viewed(&buf, &edits, Some(GradeRangeView::Shadows), None);
        assert!(off.grade_view.is_none(), "the view-off frame carries a plane");
        assert_eq!(on.rgba, off.rgba, "the view changed the texture");
        assert_eq!(on.srgb, off.srgb, "the view changed the instrument channel");
        assert_eq!(on.hist, off.hist, "the view changed the histogram");
        let p = plane(&on);
        assert_eq!(p.len(), on.width * on.height);
        // The ramp's ends: the darkest grey is a shadow, the brightest is not.
        assert!(p[0] > 200 && p[5] < 10, "the plane is not the shadow weight: {p:?}");
    }

    /// ⚠⚠ The plane is taken BEFORE the grade. A luminance wheel at +100 shifts L by 0.15 on
    /// every pixel it weights; tapped after the stage, the plane would move. Sabotage: run
    /// `from_grade` before taking the plane.
    #[test]
    fn the_plane_is_taken_before_the_grade() {
        let buf = ramp_fixture();
        for range in GradeRangeView::ALL {
            let flat = plane(&viewed(&buf, &graded(0.0, None), Some(range), None));
            let lifted = plane(&viewed(&buf, &graded(100.0, None), Some(range), None));
            assert_eq!(flat, lifted, "{range:?}: the grade's own luminance moved the plane");
        }
        // The premise: the lift really changes the photograph, so the equality above is not
        // vacuous.
        let a = viewed(&buf, &graded(0.0, None), None, None);
        let b = viewed(&buf, &graded(100.0, None), None, None);
        assert_ne!(a.srgb, b.srgb, "premise: +100 luminance changed nothing");
    }

    /// ⚠⚠ The plane is taken BEFORE the display rendering — the test that encodes spec §1 and
    /// would have failed the pack-side design outright. Sabotage: tap after
    /// `DisplayRenderingStage` (i.e. from `pack`'s output).
    #[test]
    fn the_plane_is_taken_before_the_display_rendering() {
        let buf = ramp_fixture();
        let colorimetric = graded(0.0, None);
        let rendered = graded(0.0, Some(edit_model::Rendering::Camera));
        for range in GradeRangeView::ALL {
            let a = plane(&viewed(&buf, &colorimetric, Some(range), None));
            let b = plane(&viewed(&buf, &rendered, Some(range), None));
            assert_eq!(a, b, "{range:?}: the S-curve moved the plane");
        }
        let a = viewed(&buf, &colorimetric, None, None);
        let b = viewed(&buf, &rendered, None, None);
        assert_ne!(a.srgb, b.srgb, "premise: the rendering changed nothing");
    }

    /// On a region render the plane covers the rendered slice — `w * h` of the frame, not of
    /// the whole image. `ColorImage::from_rgba_unmultiplied` panics on a mismatch rather than
    /// reporting one, so this is pinned here.
    #[test]
    fn the_plane_has_the_frames_dimensions() {
        let mut buf = ImageBuf::new(8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let v = (x + y * 8) as f32 / 48.0;
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let buf = Arc::new(buf);
        let region = Some(RegionRect {
            x: 2,
            y: 1,
            w: 4,
            h: 3,
        });
        let f = viewed(&buf, &graded(0.0, None), Some(GradeRangeView::Midtones), region);
        assert_eq!((f.width, f.height), (4, 3));
        assert_eq!(plane(&f).len(), 12);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --lib render_engine::tests::the_plane 2>&1 | tail -5`
Expected: compile error — no `grade_view` field on `FrameSpec` / `RenderedFrame`.

- [ ] **Step 3: Add the two fields and update every literal**

In `RenderedFrame` (after `pub max_dim: usize,`):

```rust
    /// The grade range view's plane — one byte per pixel, the selected range's weight — or
    /// `None` when the view is off.
    ///
    /// ⚠ Taken at the grading stage's **input** (`TailHalves::before_grade`'s output), the only
    /// place the ranges are exact; see `grade_view`. `width × height` bytes, the frame's own
    /// dimensions, by construction: no stage after the tap changes a buffer's size.
    pub grade_view: Option<Arc<Vec<u8>>>,
```

In `FrameSpec` (after `pub soft_proof: Option<Proof>,`):

```rust
    /// The grade range view, or `None` when it is off.
    ///
    /// ⚠ A view-only property of the *request*, like `visualize` and `soft_proof`: never
    /// `Edits`, never the sidecar, an export or the CLI, and it invalidates no checkpoint,
    /// because `CacheKey` is `(PrefixKey, max_dim)` and the whole tap lives in the tail.
    ///
    /// ⚠ Unlike `visualize` it is **not** a post-step in `pack`: `pack` sees the output
    /// transform's result, and the ranges are selected on lightness before the display
    /// rendering — see `render_cached`.
    pub grade_view: Option<crate::grade_view::GradeRangeView>,
```

Then add `grade_view: None,` to: `FrameSpec::fit`; the `RenderedFrame` literal in `pack`
(~line 503); `crates/app/src/buffer_cache.rs:92`; `crates/app/src/test_app.rs:313`;
`crates/app/src/multi/pane.rs:138`; `crates/app/src/session_flow.rs:386` (the open literal); and
the five test `FrameSpec {` literals in `render_engine.rs`. For `session_flow.rs:107`
(`Session::request`) write `grade_view: self.grade_view,` — the field is added to `Session` in
Task 4, so **until Task 4 lands this one is `grade_view: None,` too**; Task 4 changes it.

Run: `cargo check -p app --all-targets 2>&1 | grep -E "^error" | head`
Expected: no errors (every exhaustive literal now has the field).

- [ ] **Step 4: The tap in `render_cached`**

In `render_cached`, add `grade_view,` to the `let FrameSpec { .. } = spec;` destructure. Then
replace this block (~lines 990–1000):

```rust
    // ⚠ The **pipeline** is always sRGB here, whatever the proof does. The display transform is a
    // post-step in `pack`, deliberately outside the pipeline so the histogram stays image-referred.
    let tail = build_tail(color::OutputSpace::Srgb);
    (
        pack(
            tail.run(input),
            proofed,
            region,
            max_dim,
            display_space,
            visualize,
        ),
        ran,
    )
```

with:

```rust
    // ⚠ The **pipeline** is always sRGB here, whatever the proof does. The display transform is a
    // post-step in `pack`, deliberately outside the pipeline so the histogram stays image-referred.
    //
    // ⚠⚠ The grade range view taps the tail at the grading stage's INPUT, not `pack`'s output:
    // the ranges are selected on Oklab L before the grade's own luminance offsets, the masks,
    // the display rendering S-curve and the creative LUT move it. `None` is the path that
    // existed before this — one tail, one run, no clone. `Some` runs the same stages in the same
    // order with one read between two halves, so the frame's pixels are identical too
    // (`the_grade_view_does_not_touch_the_photograph`).
    let (out, plane) = match grade_view {
        Some(range) => {
            let (_, halves) = pipeline::build_pipeline_split_tapped(
                edits,
                scale,
                region,
                Some(&coverage),
                pipeline::RenderSpec {
                    space: color::OutputSpace::Srgb,
                    resampler,
                    baseline: wb_baseline,
                },
                None,
                profile,
            );
            let mid = halves.before_grade.run(input);
            // ⚠ Blending and Balance are read from the edits whether or not the grade is
            // active: the ranges exist before a wheel is turned.
            let g = &edits.color_grading;
            let plane = crate::grade_view::coverage(&mid, range, g.blending, g.balance);
            (halves.from_grade.run(mid), Some(Arc::new(plane)))
        }
        None => (build_tail(color::OutputSpace::Srgb).run(input), None),
    };
    let mut frame = pack(out, proofed, region, max_dim, display_space, visualize);
    frame.grade_view = plane;
    (frame, ran)
```

⚠ `build_tail` is a closure defined a few lines above that calls
`build_pipeline_split_in_region_with_brush` with these same arguments; the `Some` arm repeats
them rather than reaching into the closure, because a closure returning the halves would have to
be a second closure anyway. Keep both in step — if `build_tail` gains an argument, so does this.
⚠ `profile` is `ProfileRendering<'_>` — check whether it is `Copy`; `build_tail` is called up to
twice already (proof + plain), so it is, or is re-borrowed. If the borrow checker complains in
the `Some` arm, take `profile` by the same expression `build_tail` uses.

- [ ] **Step 5: Run the engine tests**

Run: `cargo test -p app --lib render_engine 2>&1 | grep -E "^test result|panicked|FAILED"`
Expected: all pass, including the four new tests.

- [ ] **Step 6: Sabotage it, twice**

(a) In the `Some` arm, compute the plane from `halves.from_grade.run(mid)`'s output instead:
```rust
let out = halves.from_grade.run(mid);
let plane = crate::grade_view::coverage(&out, range, g.blending, g.balance);
(out, Some(Arc::new(plane)))
```
Run: `cargo test -p app --lib render_engine::tests::the_plane 2>&1 | grep -E "panicked|ok"`
Expected: **both** `the_plane_is_taken_before_the_grade` and
`the_plane_is_taken_before_the_display_rendering` FAIL. Restore.

(b) Leave the tap in place but tint `out` (e.g. `out.set_pixel(0, 0, [0.0, 0.0, 0.0])` in the
`Some` arm). Expected: `the_grade_view_does_not_touch_the_photograph` FAILS. Restore, re-run the
module, all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/render_engine.rs crates/app/src/buffer_cache.rs crates/app/src/test_app.rs crates/app/src/multi/pane.rs crates/app/src/session_flow.rs
git commit -m "Engine: the grade range view taps the tail at the grading stage's input"
```

---

### Task 4: Session state, the panel checkbox, and the tool-switch clear

**Files:**
- Modify: `crates/app/src/main.rs` (`Session`, after the `visualize` field ~line 819)
- Modify: `crates/app/src/session_flow.rs` (`request` ~line 107; the open literal ~line 408)
- Modify: `crates/app/src/test_app.rs` (the `Session` literal, after `visualize: None,` ~line 240)
- Modify: `crates/app/src/interaction.rs` (`select_tool`, after `s.visualize = None;` ~line 100)
- Modify: `crates/app/src/develop/panel/color_wheel.rs` (`impl GradeRange`)
- Modify: `crates/app/src/develop/panel/adjustments.rs` (`show` signature ~line 332, the grading
  block ~line 400, the test at ~line 649)
- Modify: `crates/app/src/develop/panel/mod.rs` (the `Tool::Edit` arm ~line 168)

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/develop/panel/color_wheel.rs` tests:

```rust
    /// The four-way selector maps onto the three-way view, Global to nothing: Global is a
    /// constant, not a range, and there is nothing of it to show.
    #[test]
    fn every_range_but_global_has_a_view() {
        use rustroom_app::grade_view::GradeRangeView;
        assert_eq!(GradeRange::Shadows.view(), Some(GradeRangeView::Shadows));
        assert_eq!(GradeRange::Midtones.view(), Some(GradeRangeView::Midtones));
        assert_eq!(GradeRange::Highlights.view(), Some(GradeRangeView::Highlights));
        assert_eq!(GradeRange::Global.view(), None);
    }
```

In `crates/app/src/develop/panel/adjustments.rs` tests, after
`unticking_the_last_lens_correction_decides_off`:

```rust
    /// The panel body for the range-view tests: the whole Edit panel with the Color section
    /// forced open by clicking its header.
    fn range_view_body<'a>(
        edits: &'a mut edit_model::Edits,
        channel: &'a mut HslChannel,
        grade: &'a mut color_wheel::GradeRange,
        show_range: &'a mut bool,
        view: &'a mut Option<rustroom_app::grade_view::GradeRangeView>,
        buf: &'a core_image::ImageBuf,
        looks: &'a rustroom_app::looks::LooksState,
        ed: &'a mut Edited,
    ) -> impl FnMut(&mut egui::Ui) + 'a {
        move |ui: &mut egui::Ui| {
            *ed = Edited::default();
            show(
                ui,
                edits,
                channel,
                grade,
                show_range,
                view,
                SourceInfo {
                    lens: None,
                    lens_profile_found: false,
                    wb_baseline: color::wb::Illuminant::default(),
                    baseline_ev: None,
                    buffer: buf,
                    looks,
                },
                None,
                ed,
            );
        }
    }

    /// ⚠⚠ "Show range" is a VIEW MODE, not an edit — `visualize`'s test, re-run here: it asks
    /// for a render (`ed.changed`), pushes no history entry, and leaves `Edits` untouched.
    #[test]
    fn ticking_show_range_requests_the_selected_range_without_editing_anything() {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let buf = tone_fixture();
        let looks = rustroom_app::looks::LooksState::default();
        let mut edits = edit_model::Edits::default();
        let before = edits.clone();
        let mut channel = HslChannel::default();
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (false, None);
        let mut ed = Edited::default();
        let mut body = range_view_body(
            &mut edits, &mut channel, &mut grade, &mut show_range, &mut view, &buf, &looks,
            &mut ed,
        );
        crate::test_ui::click(&ctx, "Color", &mut body);
        crate::test_ui::click(&ctx, "Show range", &mut body);
        drop(body);
        assert!(show_range, "the checkbox did not take");
        assert_eq!(
            view,
            Some(rustroom_app::grade_view::GradeRangeView::Shadows),
            "the default selection is Shadows, and the view must carry it"
        );
        assert!(ed.changed, "the view asked for no render, so the canvas would not change");
        assert!(!ed.settled, "a view toggle pushed a history entry");
        assert_eq!(edits, before, "a view toggle edited the photograph");
    }

    /// ⚠ Two inputs, one value on the wire: switching the selector while the box is ticked
    /// re-renders with the new range; Global carries `None` and leaves the box ticked, so
    /// stepping back to a range brings the view straight back.
    #[test]
    fn the_selector_drives_the_view_and_global_carries_none() {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let buf = tone_fixture();
        let looks = rustroom_app::looks::LooksState::default();
        let mut edits = edit_model::Edits::default();
        let mut channel = HslChannel::default();
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (true, None);
        let mut ed = Edited::default();
        let mut body = range_view_body(
            &mut edits, &mut channel, &mut grade, &mut show_range, &mut view, &buf, &looks,
            &mut ed,
        );
        crate::test_ui::click(&ctx, "Color", &mut body);
        crate::test_ui::click(&ctx, "Mid", &mut body);
        assert_eq!(
            view,
            Some(rustroom_app::grade_view::GradeRangeView::Midtones),
            "the selector did not reach the view"
        );
        assert!(ed.changed, "a new range asked for no render");
        crate::test_ui::click(&ctx, "Global", &mut body);
        assert_eq!(view, None, "Global must carry no view");
        assert!(ed.changed, "leaving a range asked for no render");
        crate::test_ui::click(&ctx, "High", &mut body);
        drop(body);
        assert!(show_range, "Global unticked the box");
        assert_eq!(
            view,
            Some(rustroom_app::grade_view::GradeRangeView::Highlights),
            "the view did not come back after Global"
        );
    }
```

⚠ `tone_fixture()` already exists in this tests module (the lens test uses it). ⚠ The Color
section starts **collapsed** (`section(ui, SECTIONS[2], false, ..)`), so the body clicks the
section header "Color" first — check the exact header label with
`grep -n "SECTIONS" crates/app/src/develop/panel/adjustments.rs` and `Section::name`; if the
header reads differently (e.g. "Colour"), use that string. If the collapsed header does not open
on one click in the harness (animation), the `animation_time = 0.0` lines above are what makes it
immediate — they are copied from the lens test for that reason.

In `crates/app/src/interaction.rs` tests, after `guides_survive_switching_tools`:

```rust
    /// ⚠ The grade range view goes off on a tool switch, unconditionally, beside `visualize`:
    /// the checkbox lives in the Edit panel, so every change that reaches `select_tool` is a
    /// change away from it, and a canvas still tinted under Crop reads as a broken render.
    #[test]
    fn a_tool_switch_clears_the_grade_range_view() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        {
            let s = app.session.as_mut().unwrap();
            s.show_grade_range = true;
            s.grade_view = Some(rustroom_app::grade_view::GradeRangeView::Midtones);
        }
        app.select_tool(Tool::Crop);
        let s = app.session.as_ref().unwrap();
        assert!(!s.show_grade_range, "the checkbox survived the tool switch");
        assert_eq!(s.grade_view, None, "the view survived the tool switch");
    }
```

⚠ Check what `Tool` is imported as in `interaction.rs`'s tests (`use rustroom_app::tool::Tool`
or `crate::Tool`) and match it.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --bin Rustroom grade 2>&1 | tail -5`
Expected: compile errors — `show_grade_range`, `grade_view`, `GradeRange::view`, and `show`'s
arity.

- [ ] **Step 3: `Session` fields**

In `crates/app/src/main.rs`, directly after the `visualize: Option<f32>,` field:

```rust
    /// The "Show range" checkbox in the Color Grading block.
    ///
    /// ⚠ View state on `Session`, beside `visualize` and for its reason: `Session::request` is
    /// the one funnel every render goes through and cannot reach the app. Resets per photo and
    /// clears on a tool switch. ⚠ Never `Edits` — it must not reach the sidecar or a paste.
    show_grade_range: bool,
    /// What the request carries: the checkbox AND a non-Global selection, or `None`.
    ///
    /// ⚠ Two fields, one derivation site. The value depends on the checkbox and on
    /// `app.grade_range`, which lives on the app and which `request` cannot see; the panel
    /// derives this with a change guard in the one place both are in scope. Under Global it is
    /// `None` while the box stays ticked — one `Option` would untick the box the moment Global
    /// was selected.
    grade_view: Option<rustroom_app::grade_view::GradeRangeView>,
    /// The view's plane as a texture, set where the frame lands and dropped by the same
    /// assignment when a frame arrives without one.
    grade_view_texture: Option<egui::TextureHandle>,
```

In `crates/app/src/session_flow.rs`: in `Session::request`, change `grade_view: None,` (from
Task 3) to `grade_view: self.grade_view,`; in the open literal, after `visualize: None,` add
`show_grade_range: false,`, `grade_view: None,`, `grade_view_texture: None,`. In
`crates/app/src/test_app.rs`, after `visualize: None,` in the `Session` literal add the same
three lines.

- [ ] **Step 4: The clear in `select_tool`**

In `crates/app/src/interaction.rs`, directly after `s.visualize = None;`:

```rust
            // ⚠ The grade range view goes off here too, unconditionally, for the same reason:
            // its checkbox is only in the Edit panel, so every change that reaches this line is
            // a change away from it.
            s.show_grade_range = false;
            s.grade_view = None;
```

- [ ] **Step 5: `GradeRange::view`**

In `crates/app/src/develop/panel/color_wheel.rs`, inside `impl GradeRange` after `wheel_mut`:

```rust
    /// The range the grade range view shows for this selection — `None` for Global, which is
    /// a constant rather than a range.
    pub fn view(self) -> Option<rustroom_app::grade_view::GradeRangeView> {
        use rustroom_app::grade_view::GradeRangeView;
        match self {
            Self::Shadows => Some(GradeRangeView::Shadows),
            Self::Midtones => Some(GradeRangeView::Midtones),
            Self::Highlights => Some(GradeRangeView::Highlights),
            Self::Global => None,
        }
    }
```

- [ ] **Step 6: The checkbox and the derivation in the panel**

In `crates/app/src/develop/panel/adjustments.rs`, change `show`'s signature to:

```rust
pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    grade_range: &mut color_wheel::GradeRange,
    show_grade_range: &mut bool,
    grade_view: &mut Option<rustroom_app::grade_view::GradeRangeView>,
    source: SourceInfo<'_>,
    picking: Option<&mut bool>,
    ed: &mut Edited,
) {
```

and, in the Color section, directly after `ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");`:

```rust
        // ⚠⚠ A VIEW MODE, not an edit — `visualize`'s rule. It writes two session fields and
        // never `Edits`, so it re-renders (`ed.changed`) without a history entry.
        ui.checkbox(show_grade_range, "Show range").on_hover_text(
            "Tints the photograph where the selected range reaches. Affects only what you see — \
             never the render, the export or the histogram.",
        );
        // ⚠ Two inputs, one value on the wire, derived here because this is the one place the
        // checkbox and the selector are both in scope. Change-guarded: this body runs every
        // frame, and an unguarded write would request a render sixty times a second.
        let wanted = show_grade_range.then(|| grade_range.view()).flatten();
        if *grade_view != wanted {
            *grade_view = wanted;
            ed.changed = true;
        }
```

⚠ The `let g = &mut edits.color_grading;` borrow above ends at the Balance slider, and
`grade_range.view()` takes `self` by value (`GradeRange` is `Copy`), so no borrow conflict.

Update the existing test call site (~line 649, the lens test) to pass two extra arguments after
`&mut grade`: declare `let (mut show_range, mut view) = (false, None);` beside `grade` and pass
`&mut show_range, &mut view,`.

In `crates/app/src/develop/panel/mod.rs`, the `Tool::Edit` arm: after `&mut app.grade_range,`
add `&mut s.show_grade_range,` and `&mut s.grade_view,`.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p app --bin Rustroom 2>&1 | grep -E "^test result|panicked|FAILED"`
Expected: all pass, including the four new tests.

- [ ] **Step 8: Sabotage it**

Remove the change guard (write `*grade_view = wanted; ed.changed = true;` unconditionally).
Run: `cargo test -p app --bin Rustroom the_panel_leaves_a_suppression 2>&1 | grep -E "panicked|ok"`
— that existing Retouch test does not see the Edit panel, so expect it to still pass; instead
confirm the guard by running `ticking_show_range_requests_the_selected_range_without_editing_anything`
with an added temporary assertion that a **second** body pass with no click leaves `ed.changed`
false. If that is awkward, record honestly in the final report that the guard's sabotage is
covered only by the general rule (`the_auto_button` tests' "drawing the panel pushed a history
entry" pattern) and not by a dedicated test. Restore the guard either way.

Then move `s.show_grade_range = false; s.grade_view = None;` behind an `if t != Tool::Edit`
guard. Run: `cargo test -p app --bin Rustroom a_tool_switch_clears_the_grade 2>&1 | grep -E "panicked|ok"`
Expected: still passes (the test switches *to* Crop) — so add a second switch in the test,
Crop → Edit with the fields re-set, and assert they clear again. Expected with the guard: FAIL.
Restore the unconditional clear; both switches pass. Keep the two-switch test.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/session_flow.rs crates/app/src/test_app.rs crates/app/src/interaction.rs crates/app/src/develop/panel/color_wheel.rs crates/app/src/develop/panel/adjustments.rs crates/app/src/develop/panel/mod.rs
git commit -m "Show range: the checkbox, the session view state, and the tool-switch clear"
```

---

### Task 5: The texture lands, and the canvas paints it

**Files:**
- Modify: `crates/app/src/shell.rs` (the frame landing, after `s.texture = Some(..)` ~line 88)
- Modify: `crates/app/src/develop/canvas/mod.rs` (after `egui::Image::new(shown).paint_at(ui, resp.rect);` ~line 252; tests)

- [ ] **Step 1: Write the failing test**

In `crates/app/src/develop/canvas/mod.rs` tests, after
`the_guide_loupe_is_up_before_any_endpoint_is_grabbed`:

```rust
    /// ⚠ The view's texture is one extra mesh over the photograph, painted at the image rect;
    /// absent, it paints nothing. Counted rather than asserted by rect, the way the loupe is.
    #[test]
    fn a_landed_plane_is_painted_over_the_photograph() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let idle = meshes_hovering(&mut app, &ctx, None);
        {
            let s = app.session.as_mut().expect("a session");
            let rgba = rustroom_app::grade_view::tint_rgba(&vec![200u8; 80 * 60]);
            let img = egui::ColorImage::from_rgba_unmultiplied([80, 60], &rgba);
            s.grade_view_texture =
                Some(ctx.load_texture("grade_view_test", img, egui::TextureOptions::LINEAR));
        }
        let tinted = meshes_hovering(&mut app, &ctx, None);
        assert_eq!(
            tinted,
            idle + 1,
            "expected one extra mesh — the range tint; got {tinted} tinted, {idle} idle"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p app --bin Rustroom a_landed_plane_is_painted 2>&1 | grep -E "panicked|ok|error"`
Expected: FAIL — `tinted == idle` (nothing paints the texture yet).

- [ ] **Step 3: Paint it**

In `crates/app/src/develop/canvas/mod.rs`, directly after
`egui::Image::new(shown).paint_at(ui, resp.rect);`:

```rust
                    // ⚠ The grade range view, over the photograph and under every tool overlay.
                    // `resp.rect` is `img_rect` — the rendered slice — and this texture covers
                    // exactly that, whether the frame was whole or a region; the readout's
                    // exception, for the readout's reason. Every other overlay is frame-relative.
                    if let Some(tint) = &s.grade_view_texture {
                        egui::Image::new(tint).paint_at(ui, resp.rect);
                    }
```

- [ ] **Step 4: Land it**

In `crates/app/src/shell.rs`, directly after
`s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));`:

```rust
                // ⚠ The grade range view's plane becomes a texture in the same place the frame
                // does, so the two can never describe different frames — and a frame that
                // arrives without a plane (box unticked, Global, a tool switch) drops the
                // texture in this same assignment. No separate clear.
                s.grade_view_texture = frame.grade_view.as_ref().map(|plane| {
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width, frame.height],
                        &rustroom_app::grade_view::tint_rgba(plane),
                    );
                    ctx.load_texture("grade_view", img, egui::TextureOptions::LINEAR)
                });
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p app --bin Rustroom canvas 2>&1 | grep -E "^test result|panicked|FAILED"`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/shell.rs crates/app/src/develop/canvas/mod.rs
git commit -m "Canvas: the grade range plane lands as a texture and paints over the photograph"
```

---

### Task 6: Measure the cost

**Files:**
- Create: `crates/app/examples/bench_grade_view.rs`

- [ ] **Step 1: Write the probe**

```rust
//! Times the grade range view's plane — the one cost the view adds, paid only while it is on.
//!
//! Run: `cargo run --release -p app --example bench_grade_view <raw>`
//!
//! Two buffers: the fit rung (1600 px preview) and a 2000×1400 crop of the half-size decode,
//! the shape of a native-rung region. The tap itself adds nothing — the halves are the tail —
//! so `coverage` is the whole cost.
use rustroom_app::grade_view::{coverage, GradeRangeView};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: bench_grade_view <raw>"));
    let decoded =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let full = decoded.image;
    let fit = full.downscale_to(1600);
    let (cw, ch) = (2000.min(full.width()), 1400.min(full.height()));
    let region = full.crop(
        (full.width() - cw) / 2,
        (full.height() - ch) / 2,
        cw,
        ch,
    );
    for (name, buf) in [("fit", &fit), ("native region", &region)] {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let p = coverage(buf, GradeRangeView::Midtones, 50.0, 0.0);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(p);
        }
        println!(
            "{name:>14} {}x{}: coverage {best:.2} ms (best of 5)",
            buf.width(),
            buf.height()
        );
    }
}
```

- [ ] **Step 2: Run it**

Run (the X-T4 frames live under `~/Pictures` or wherever the earlier probes were pointed —
`grep -rn "DSCF" CLAUDE.md | head -3` names one):
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example bench_grade_view <raw> 2>&1 | tail -3
```
Expected: two lines with milliseconds. Record both numbers for `CLAUDE.md`. If the fit number
exceeds ~10 ms, say so in the report; it is still accepted (paid only while on) but the spec's
"single digits" expectation was wrong and the record must say so.

- [ ] **Step 3: Commit**

```bash
git add crates/app/examples/bench_grade_view.rs
git commit -m "bench_grade_view: the cost of the range plane at fit and at a native region"
```

---

### Task 7: See it

- [ ] **Step 1: Run the GUI in the background**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```
(in the background). Tell the user what to look at: open a photo, Edit → Color → Color Grading,
tick **Show range**; the picture tints teal where the Shadows range reaches; click **Mid** and
**High** and the tint moves; drag **Blending** and **Balance** and watch it spread and slide;
**Global** shows nothing and leaves the box ticked; switch to Crop and the tint is gone and the
box unticked on return; the histogram must not move while toggling.

- [ ] **Step 2: Wait for the user's report** before the docs task, and fix what is found on this
branch, each fix with its own test and commit.

---

### Task 8: Docs, and the full verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-08-grade-range-view-design.md` (§7's inert-sabotage
  claim; status line)
- Modify: `docs/BACKLOG.md` (line 352: the "Still open: no visual feedback" clause; line 35's
  table row lists four gaps, unchanged — this was the fifth, un-named there)
- Modify: `CLAUDE.md` (resume block: state and NEXT ACTION; the M3.3 section's caveat (3); a new
  section; the crate layout line for `app/` gains `grade_view`)
- Modify: `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md`
  (M7.6's entry ~line 539: "five small items" → four, this one ✅ 2026-09-08; line 92's marker)

- [ ] **Step 1: The full gate, each from its own exit code**

```bash
cargo fmt --all -- --check; echo "fmt exit $?"
```
```bash
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy exit ${PIPESTATUS[0]}"
```
```bash
cargo test --workspace 2>&1 | tee /tmp/rustroom_m76a_tests.txt | grep -E "^test result" ; echo "test exit ${PIPESTATUS[0]}"
```
```bash
grep -E "^test .*golden.* ok$" /tmp/rustroom_m76a_tests.txt | wc -l
```

Expected: `fmt exit 0`, `clippy exit 0`, `test exit 0`, every `test result` line `ok`, golden
count **59**, and — because the tapped builder is the same stages in the same order — every golden
byte-identical, which is what a passing golden test asserts. Record the new total (sum of the
`passed` numbers; it was **2095** at handoff).

- [ ] **Step 2: The spec correction**

In the spec's §7, replace the `a_pixel_above_white_is_a_highlight` bullet's sabotage clause with:
*"⚠ Corrected at plan time: that sabotage is inert — the weights are normalised, so the
highlight weight is ≈1 for `L > 1` clamped or not. The clamp is parity with the stage's table
index; the test pins the behaviour."* Update the status line to *"implemented 2026-09-08"*.

- [ ] **Step 3: The backlog**

In `docs/BACKLOG.md` line 352, change *"Still open: **no visual feedback of the grading range
boundaries** on canvas or histogram."* to *"~~Still open: no visual feedback of the grading
range boundaries.~~ ✅ **2026-09-08** — the grade range view (`grade_view.rs`), a tap at the
grading stage's input; spec `2026-09-08-grade-range-view-design.md`."*

- [ ] **Step 4: `CLAUDE.md`**

(a) In the M3.3 section's "Accepted caveats", strike (3): *~~(3) no visual feedback of the range
boundaries on canvas or histogram~~ ✅ 2026-09-08, see "The grade range view"*.

(b) In the crate layout line for `app/`, add `grade_view` to the lib list after `visualize`.

(c) Add a section **"The grade range view (M7.6, first gap)"** in this file's idiom, placed above
the most recent section, carrying: what shipped (the checkbox, the tap, the teal tint at
`img_rect`); **why the cheap designs were wrong by construction** (§1 of the spec — the S-curve
sits between the grading stage and both the histogram and `pack`); the `TailHalves` seam and
"byte-identical by construction, 59 goldens confirmed"; the two-fields-one-derivation decision
and Global; the inert sabotage correction; the measured cost (both numbers from Task 6, against
the +5.4 ms reference); the tests that encode §1 (`the_plane_is_taken_before_the_display_rendering`)
and the sabotages run; what the GUI check found (Task 7); accepted caveats (one range at a time;
no histogram; render resolution; a masked exposure does not move the ranges; resets per photo and
tool). ⚠ Name the four oversized files touched.

(d) In the **RESUME HERE** block: update the *State at handoff* sentence to the new test count,
59 goldens, branch `m7.6-grade-range-view` awaiting merge (⚠ do not merge or push — ask), and
prepend to the **▶ NEXT ACTION** paragraph:

> **▶ NEXT ACTION:** ask whether to merge `m7.6-grade-range-view` (the grade range view, M7.6's
> first gap — its section is "The grade range view" below). Then the **next M7.6 gap**, user's
> choice among the four left: (2) a halo suppressor for Clarity, (3) edge-aware negative Texture,
> (4) auto-mask for the brush, (5) dehaze's bright-achromatic over-correction — one at a time,
> each on its own branch, brainstorm → spec → plan → executing-plans, inline; measure before
> theorising.

keeping the rest of the existing paragraph (the after-M7.6 sequence) intact.

- [ ] **Step 5: The vault**

In `03-plan-of-action.md`'s M7.6 entry (~line 539), change the ⬜ bullet to list the range
boundaries as ✅ **2026-09-08** *(the grade range view — a tap at the grading stage's input,
because the histogram and the packed pixels sit past the display S-curve)* and the other four as
⬜; change "five small items left" to "four small items left"; and line 92's marker text
*"M7.6 🟡 (five small gaps)"* to *"(four small gaps)"*.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md docs/superpowers/specs/2026-09-08-grade-range-view-design.md
git commit -m "Record the grade range view, the first M7.6 gap"
```

(The Obsidian vault is not in this repository; its file is saved, not committed.)

- [ ] **Step 7: Report**

State: the test delta, that 59 goldens ran and none moved, the measured cost at fit and at a
native region, the branch name, that it is **not** merged and **not** pushed, the four oversized
files touched (`render_engine.rs`, `adjustments.rs`, `shell.rs`, `canvas/mod.rs`), the inert
sabotage corrected in the spec, and what the GUI check showed.

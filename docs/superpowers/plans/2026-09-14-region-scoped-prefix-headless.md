# M8.7 plan 1 — the region-scoped prefix, headless

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **⚠ Project conventions, non-negotiable.** Commit messages carry **no `Co-Authored-By` trailer**.
> Work happens on branch `m8.7-region-scoped-prefix`. **Never push. Never merge unprompted.**
> Before every commit, read clippy's exit code **from its own command**, never through a pipe:
> `cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"`.
> Build env: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`.

**Goal:** Give `pipeline` everything needed to region-scope a chain of prefix steps — one rect type,
a `input_rect_for` seam with four implementations, a chainable `apply_region`, a pure `region_plan`
walker, and a deterministic dehaze airlight — with no render engine involved.

**Architecture:** `input_rect_for` is the mirror of `apply_region`: it says what input rect a stage
needs to produce a given output rect. `region_plan` walks backwards from the visible rect through
the steps, abandoning if any declines. The airlight becomes a parameter computed from a fixed 256 px
replay of the prefix, so scoped and whole-buffer paths agree by construction.

**Tech Stack:** Rust, `rayon`, the existing `pipeline` crate. No new dependencies.

⚠⚠ **This plan ships INERT.** Nothing calls `region_plan` until plan 2 wires it in. That is
M3.1a's precedent and it is deliberate — but it is also the W5 failure mode (creative LUTs shipped
fully tested and 100% dead), so plan 2's carrier test is what closes it and must not be deferred.

**Spec:** `docs/superpowers/specs/2026-09-14-region-scoped-prefix-whole-design.md`

---

## File structure

| file | responsibility | change |
|---|---|---|
| `crates/pipeline/src/lib.rs` | `Rect`, `PipelineStage::input_rect_for`, `region_plan`, `RegionPlan`, `RegionSrc`, `prefix_airlight` | modify |
| `crates/pipeline/src/stages/noise_reduction.rs` | `kernel_reach`, `input_rect_for` | modify |
| `crates/pipeline/src/stages/dehaze.rs` | `kernel_reach`, `input_rect_for`, supplied airlight | modify |
| `crates/pipeline/src/stages/geometry.rs` | `input_rect_for`, chainable `apply_region` | modify |
| `crates/pipeline/src/stages/perspective.rs` | `input_rect_for`, chainable `apply_region` | modify |
| `crates/app/src/render_engine.rs` | drop `RegionRect`, use `pipeline::Rect` | modify (mechanical) |
| `crates/app/examples/airlight_shift.rs` | the committed measurement | create |

---

### Task 1: One rect type

⚠ `pipeline::OutRect { x0, y0, w, h }` and `app::RegionRect { x, y, w, h }` are structurally
identical. This milestone would otherwise add a third. The name is **`Rect`**, deliberately neutral:
`input_rect_for` returns an *input* rect and `apply_region` takes an *output* one, and a type called
`OutRect` in both positions reads as a bug to everyone who meets it later.

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (the `OutRect` definition, ~line 414)
- Modify: `crates/app/src/render_engine.rs` (the `RegionRect` definition, ~line 76, and its uses)
- Modify: `crates/pipeline/src/stages/geometry.rs`, `crates/pipeline/src/stages/perspective.rs`

- [ ] **Step 1: Rename the type and its fields in `pipeline`**

In `crates/pipeline/src/lib.rs`, replace the `OutRect` definition with:

```rust
/// A pixel rectangle, in whichever coordinate space the caller names.
///
/// ⚠⚠ **Deliberately NOT called `OutRect`.** [`PipelineStage::apply_region`] takes an *output*
/// rect and [`PipelineStage::input_rect_for`] returns an *input* one; a type whose name asserts
/// one of those reads as a bug in the other position. It replaced `app::RegionRect`, which was
/// this same struct with different field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    /// This rect clipped into a `w` x `h` image, or `None` if nothing survives.
    ///
    /// ⚠ Moved here from `app::RegionRect` so the one clipping rule lives beside the type rather
    /// than in whichever crate happened to need it first.
    pub fn clamped(self, w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let x = self.x.min(w - 1);
        let y = self.y.min(h - 1);
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self { x, y, w: cw, h: ch })
    }

    /// This rect grown by `m` pixels on every side, clipped into `w` x `h`.
    ///
    /// ⚠ Saturating on the low side and clipping on the high side, so a margin larger than the
    /// image yields the whole image rather than an empty rect or a panic.
    pub fn grown(self, m: usize, w: usize, h: usize) -> Self {
        let x = self.x.saturating_sub(m);
        let y = self.y.saturating_sub(m);
        let x1 = (self.x + self.w + m).min(w);
        let y1 = (self.y + self.h + m).min(h);
        Self { x, y, w: x1.saturating_sub(x), h: y1.saturating_sub(y) }
    }
}
```

- [ ] **Step 2: Sweep the old names**

```bash
cd ~/Projects/rustroom
grep -rln "OutRect" crates/ | xargs sed -i '' 's/\bOutRect\b/Rect/g'
grep -rln "RegionRect" crates/ | xargs sed -i '' 's/\bRegionRect\b/Rect/g'
```

⚠ **The field names differ, so this sweep is not sufficient on its own.** `pipeline` used `x0`/`y0`
and `app` used `x`/`y`. Fix the `pipeline` call sites by hand — the compiler names every one. Then
delete `app`'s now-duplicate `Rect` struct and its `clamped` impl, and add
`use pipeline::Rect;` in `render_engine.rs`.

- [ ] **Step 3: Assert the sweep landed and nothing else moved**

```bash
grep -rn "OutRect\|RegionRect" crates/ | grep -v "^Binary" || echo "OK: no old names remain"
cargo build --workspace 2>&1 | tail -5
```

Expected: `OK: no old names remain`, and a clean build.

- [ ] **Step 4: Full gate — this is a pure rename, so nothing may move**

```bash
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git status --short tests/ crates/pipeline/tests/
```

Expected: `FMT=0`, `CLIPPY=0`, **`passed=2439 failed=0`** — the count must be *unchanged*, which is
the assertion, not "still green" — and an **empty** golden tree.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -F - <<'MSG'
refactor: one Rect type for pipeline and app

pipeline::OutRect { x0, y0, w, h } and app::RegionRect { x, y, w, h } were
structurally identical with different field names, and M8.7 would otherwise
have added a third. They become pipeline::Rect { x, y, w, h }.

The name is deliberately neutral: input_rect_for returns an input rect and
apply_region takes an output one, so a type called OutRect would read as a
bug in one of the two positions.

clamped moves onto the type, and grown joins it — the margin rule M8.7
needs, saturating low and clipping high so a margin larger than the image
yields the whole image rather than an empty rect.

Test count unchanged at 2439 and no golden moved, which is what says this
renamed and moved nothing.
MSG
```

---

### Task 2: Noise reduction's kernel reach

**Files:**
- Modify: `crates/pipeline/src/stages/noise_reduction.rs`

- [ ] **Step 1: Write the failing test**

Add to that file's `mod tests`:

```rust
#[test]
fn the_kernel_reach_is_the_wider_of_the_two_halves() {
    // Luminance is a self-guided filter at radius 6, and a guided base reaches 2r — the rule
    // `local_contrast::kernel_reach` records. Chroma is a plain box blur at 8..32, so it reaches
    // its own radius. At smoothness 0 chroma is 8 and luminance's 12 wins; by smoothness 50 the
    // chroma window is 20 and it wins outright.
    let at = |lum: f32, col: f32, smooth: f32| {
        kernel_reach(
            &NoiseReduction { luminance: lum, color: col, detail: 50.0, smoothness: smooth },
            RenderScale::FULL,
        )
    };
    assert_eq!(at(50.0, 0.0, 0.0), 12, "luminance only: 2 x 6");
    assert_eq!(at(0.0, 50.0, 0.0), 8, "chroma only, smoothness 0");
    assert_eq!(at(0.0, 50.0, 100.0), 32, "chroma only, smoothness 100");
    assert_eq!(at(50.0, 50.0, 0.0), 12, "both, and luminance is wider at smoothness 0");
    assert_eq!(at(50.0, 50.0, 50.0), 20, "both, and chroma is wider at smoothness 50");
}

#[test]
fn an_inactive_half_contributes_no_reach() {
    // ⚠ The discriminating case: an implementation that always took the max of both radii would
    // keep a 32 px margin for a photo carrying no chroma NR at all.
    let p = NoiseReduction { luminance: 50.0, color: 0.0, detail: 50.0, smoothness: 100.0 };
    assert_eq!(kernel_reach(&p, RenderScale::FULL), 12, "chroma is off, so its 32 must not count");
    let q = NoiseReduction { luminance: 0.0, color: 0.0, detail: 50.0, smoothness: 100.0 };
    assert_eq!(kernel_reach(&q, RenderScale::FULL), 0, "wholly inactive reaches nothing");
}

#[test]
fn the_reach_follows_the_render_scale() {
    // ⚠⚠ At RenderScale::FULL the scaling is the identity, so every test above is structurally
    // blind to the scale being dropped — the exact blind spot `the_radii_follow_the_render_scale`
    // was written for in M8.3q. This is the only test here that can see it.
    let p = NoiseReduction { luminance: 0.0, color: 50.0, detail: 50.0, smoothness: 100.0 };
    assert_eq!(kernel_reach(&p, RenderScale::FULL), 32);
    assert_eq!(kernel_reach(&p, RenderScale::new(0.5)), 16);
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p pipeline the_kernel_reach_is_the_wider 2>&1 | tail -5
```

Expected: FAIL — `cannot find function 'kernel_reach'`.

- [ ] **Step 3: Implement it**

Add to `crates/pipeline/src/stages/noise_reduction.rs`, beside `chroma_radius_full`:

```rust
/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// ⚠⚠ The **max** of the two halves, not their sum: both are computed from the same Oklab
/// conversion of the same source and written into different channels, so neither composes with
/// the other. (Contrast `sharpening::max_kernel_radius`, where the mask blurs `|d|` on top of the
/// first blur and the two radii genuinely add.)
///
/// ⚠⚠ **The luminance half reaches `2r`, not `r`.** It is a self-guided filter: every window
/// statistic is a box blur of radius `r`, and `mean_slope`/`mean_offset` blur those again. That is
/// `local_contrast::kernel_reach`'s rule, and forgetting it is what made this repo's recorded
/// ~110 px margin estimate for M8.7 too small by 1.6x.
///
/// ⚠ An inactive half contributes nothing, so a photo carrying only luminance NR does not keep a
/// 32 px margin for a chroma blur that never runs.
///
/// Any renderer that crops a region before running this stage must keep at least this much margin
/// outside what it displays — see [`crate::region_plan`].
pub fn kernel_reach(params: &NoiseReduction, scale: RenderScale) -> usize {
    let lum = if params.luminance > 0.0 {
        2 * scaled_radius(LUM_RADIUS_FULL, scale)
    } else {
        0
    };
    let col = if params.color > 0.0 {
        scaled_radius(chroma_radius_full(params.smoothness), scale)
    } else {
        0
    };
    lum.max(col)
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline noise_reduction 2>&1 | grep -E "^test result:"
```

Expected: PASS, and the pre-existing `noise_reduction` tests unchanged.

- [ ] **Step 5: Sabotage — prove the reach test can see the 2r rule**

```bash
cp crates/pipeline/src/stages/noise_reduction.rs /tmp/nr.bak
sed -i '' 's/2 \* scaled_radius(LUM_RADIUS_FULL, scale)/scaled_radius(LUM_RADIUS_FULL, scale)/' crates/pipeline/src/stages/noise_reduction.rs
cargo test -p pipeline the_kernel_reach_is_the_wider 2>&1 | grep -E "^test result:|panicked" | head -3
cp /tmp/nr.bak crates/pipeline/src/stages/noise_reduction.rs && touch crates/pipeline/src/stages/noise_reduction.rs
```

Expected: FAIL, reporting `left: 6, right: 12`. ⚠ A sabotage that fails to *compile* reads exactly
like one that passes — confirm a `test result:` line appears before believing anything.

- [ ] **Step 6: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/noise_reduction.rs
git commit -m "feat: noise reduction reports its kernel reach

The max of the two halves, not their sum — both are computed from the same
Oklab conversion of the same source and written into different channels, so
neither composes with the other.

The luminance half reaches 2r because it is a self-guided filter: every
window statistic is a box blur of radius r and the coefficients are blurred
again. Forgetting that rule is what made the recorded ~110 px margin
estimate for M8.7 too small by 1.6x.

An inactive half contributes nothing, so a photo carrying only luminance NR
does not keep a 32 px margin for a chroma blur that never runs."
```

---

### Task 3: Dehaze's kernel reach

**Files:**
- Modify: `crates/pipeline/src/stages/dehaze.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_kernel_reach_is_the_refinement_plus_the_dark_channel_patch() {
    // ⚠⚠ The two genuinely ADD, unlike noise reduction's two halves. The dark channel is a
    // min_filter of radius 20 producing a transmission map; the guided refinement then reads a
    // 2 x 60 neighbourhood OF THAT map. So one output pixel depends on the source within
    // 120 + 20 = 140.
    let p = Dehaze { amount: 60.0 };
    assert_eq!(kernel_reach(&p, RenderScale::FULL), 140);
}

#[test]
fn an_inactive_dehaze_reaches_nothing() {
    assert_eq!(kernel_reach(&Dehaze { amount: 0.0 }, RenderScale::FULL), 0);
}

#[test]
fn the_dehaze_reach_follows_the_render_scale() {
    // ⚠ At RenderScale::FULL the scaling is the identity, so the tests above cannot see it being
    // dropped. This is the only one that can.
    let p = Dehaze { amount: 60.0 };
    assert_eq!(kernel_reach(&p, RenderScale::new(0.5)), 2 * 30 + 10);
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p pipeline the_kernel_reach_is_the_refinement 2>&1 | tail -5
```

Expected: FAIL — `cannot find function 'kernel_reach'`.

- [ ] **Step 3: Implement it**

```rust
/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// ⚠⚠ The two terms **add**, unlike `noise_reduction::kernel_reach`'s max. The dark channel is a
/// `min_filter` of radius `PATCH_RADIUS_FULL` producing a transmission map; the guided refinement
/// then reads a `2 * REFINE_RADIUS_FULL` neighbourhood **of that map**. So the dependence
/// composes: 120 + 20 = 140 full-resolution pixels.
///
/// ⚠ `2 *` because a guided base reaches `2r` — `local_contrast::kernel_reach`'s rule.
///
/// ⚠ The airlight contributes **nothing** here, and that is the whole point of it being a
/// parameter: see [`crate::prefix_airlight`]. Were it still estimated from the stage's input, this
/// stage's true reach would be the entire frame and no margin could ever be correct.
pub fn kernel_reach(params: &Dehaze, scale: RenderScale) -> usize {
    if !params.is_active() {
        return 0;
    }
    2 * scaled_radius(REFINE_RADIUS_FULL, scale) + scaled_radius(PATCH_RADIUS_FULL, scale)
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline dehaze 2>&1 | grep -E "^test result:"
```

Expected: PASS, pre-existing dehaze tests unchanged.

- [ ] **Step 5: Sabotage**

```bash
cp crates/pipeline/src/stages/dehaze.rs /tmp/dh.bak
sed -i '' 's/2 \* scaled_radius(REFINE_RADIUS_FULL, scale) + scaled_radius(PATCH_RADIUS_FULL, scale)/scaled_radius(REFINE_RADIUS_FULL, scale) + scaled_radius(PATCH_RADIUS_FULL, scale)/' crates/pipeline/src/stages/dehaze.rs
cargo test -p pipeline the_kernel_reach_is_the_refinement 2>&1 | grep -E "^test result:|left:" | head -3
cp /tmp/dh.bak crates/pipeline/src/stages/dehaze.rs && touch crates/pipeline/src/stages/dehaze.rs
```

Expected: FAIL with `left: 80, right: 140`.

- [ ] **Step 6: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/dehaze.rs
git commit -m "feat: dehaze reports its kernel reach

The two terms add, unlike noise reduction's max. The dark channel is a
min_filter of radius 20 producing a transmission map; the guided refinement
reads a 2 x 60 neighbourhood of that map, so the dependence composes to 140
full-resolution pixels.

The airlight contributes nothing, which is the whole point of making it a
parameter: were it still estimated from the stage's input, dehaze's true
reach would be the entire frame and no margin could ever be correct."
```

---

### Task 4: The `input_rect_for` seam, and the two locals

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (the `PipelineStage` trait)
- Modify: `crates/pipeline/src/stages/noise_reduction.rs`, `crates/pipeline/src/stages/dehaze.rs`

- [ ] **Step 1: Add the trait method**

In `crates/pipeline/src/lib.rs`, immediately after `apply_region`:

```rust
    /// The input rect this stage needs in order to produce `out` of its output.
    ///
    /// ⚠⚠ **`None` is the default, so every stage declines BY CONSTRUCTION.** It is the mirror of
    /// [`Self::apply_region`] and carries the same rule: **fail toward the whole-buffer path**,
    /// where a wrong answer costs milliseconds and never pixels. A stage that implements one of
    /// the pair and not the other is a bug — `region_plan` will promise an input rect for an
    /// output `apply_region` then refuses to produce.
    ///
    /// ⚠ `out` is in this stage's **output** coordinates and the result is in its **input**
    /// coordinates, which for a dimension-changing stage are different spaces. Resolve `out`
    /// against [`Self::output_dims`] first.
    ///
    /// ⚠ Implementations must clip the result into `in_w` x `in_h` themselves: a margin larger
    /// than the image is ordinary at low render scales, and [`Rect::grown`] does it.
    fn input_rect_for(&self, _out: Rect, _in_w: usize, _in_h: usize) -> Option<Rect> {
        None
    }
```

- [ ] **Step 2: Write the failing tests**

In `crates/pipeline/src/lib.rs`'s `mod tests`:

```rust
#[test]
fn a_stage_with_no_region_support_declines_to_name_an_input_rect() {
    // ⚠ The default is what makes spot removal, lens correction and the masked captures decline
    // without any of them being name-checked by the caller.
    let s = stages::exposure::ExposureStage::new(1.0);
    assert_eq!(s.input_rect_for(Rect { x: 0, y: 0, w: 4, h: 4 }, 64, 64), None);
}

#[test]
fn noise_reduction_grows_the_rect_by_its_reach_and_clips_to_the_image() {
    let p = edit_model::NoiseReduction {
        luminance: 0.0, color: 50.0, detail: 50.0, smoothness: 100.0,
    };
    let s = stages::noise_reduction::NoiseReductionStage::new(&p, RenderScale::FULL);
    // Reach 32, interior rect: grows on every side.
    let got = s.input_rect_for(Rect { x: 100, y: 100, w: 10, h: 10 }, 1000, 1000).unwrap();
    assert_eq!(got, Rect { x: 68, y: 68, w: 74, h: 74 });
    // ⚠ At the image's own corner it must CLIP rather than underflow or overrun.
    let got = s.input_rect_for(Rect { x: 0, y: 0, w: 10, h: 10 }, 20, 20).unwrap();
    assert_eq!(got, Rect { x: 0, y: 0, w: 20, h: 20 }, "clipped to the whole image");
}

#[test]
fn dehaze_grows_the_rect_by_its_reach() {
    let s = stages::dehaze::DehazeStage::new(&edit_model::Dehaze { amount: 60.0 }, RenderScale::FULL);
    let got = s.input_rect_for(Rect { x: 500, y: 500, w: 10, h: 10 }, 2000, 2000).unwrap();
    assert_eq!(got, Rect { x: 360, y: 360, w: 290, h: 290 }, "140 on every side");
}
```

- [ ] **Step 3: Run and watch them fail**

```bash
cargo test -p pipeline input_rect 2>&1 | grep -E "^test result:|left:" | head -5
```

Expected: FAIL — NR and dehaze return `None` (the default) where a `Rect` is expected.

- [ ] **Step 4: Implement both**

In `noise_reduction.rs`, inside `impl PipelineStage for NoiseReductionStage`:

```rust
    /// ⚠ Grows by [`kernel_reach`] on every side. This stage preserves dimensions, so `out` and the
    /// result are in the same space and no `output_dims` resolution is needed.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
    }
```

In `dehaze.rs`, inside `impl PipelineStage for DehazeStage`:

```rust
    /// ⚠ Grows by [`kernel_reach`] on every side. Dimension-preserving, like noise reduction.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
    }
```

⚠ If either stage does not already hold its `params` and `scale` as fields, add them — both
already take them in `new`.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p pipeline input_rect 2>&1 | grep -E "^test result:"
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/
git commit -m "feat: stages can say what input rect they need

input_rect_for is the mirror of apply_region: given an output rect, what
input does this stage read? None is the default, so spot removal, lens
correction and the masked captures all decline by construction rather than
being name-checked by the caller — the rule apply_region and GpuStage::cpu
already follow.

Noise reduction and dehaze grow by their kernel reach and clip to the image,
because a margin larger than the image is ordinary at low render scales."
```

---

### Task 5: Geometry's back-projection

**Files:**
- Modify: `crates/pipeline/src/stages/geometry.rs`

⚠⚠ **Geometry declines on a non-identity orientation, exactly as `apply_region` already does.**
`orient` is a whole-buffer index permutation feeding the rotation's gather. `input_rect_for` must
decline on the *same* condition, or the plan promises an input rect for an output the forward half
then refuses to produce.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn geometry_declines_an_input_rect_under_a_non_identity_orientation() {
    // ⚠⚠ It must decline on exactly the condition `apply_region` declines on. A pair that
    // disagrees is worse than neither: region_plan would build a plan the forward half refuses.
    let g = Geometry {
        angle: 0.0,
        crop: CropRect::default(),
        orientation: Orientation { rotate: Quarter::Cw90, mirror: false },
    };
    let s = GeometryStage::new(&g, Resampler::CatmullRom);
    let r = Rect { x: 0, y: 0, w: 4, h: 4 };
    assert_eq!(s.input_rect_for(r, 64, 64), None);
    assert!(s.apply_region(&ImageBuf::new(64, 64), r).is_none(), "the pair must agree");
}

#[test]
fn an_unrotated_crop_offsets_the_rect_and_grows_it_by_nothing() {
    // No resampling at all, so the input rect is the output rect shifted by the crop origin.
    let g = Geometry {
        angle: 0.0,
        crop: CropRect { left: 0.25, top: 0.25, right: 1.0, bottom: 1.0 },
        orientation: Orientation::default(),
    };
    let s = GeometryStage::new(&g, Resampler::CatmullRom);
    let got = s.input_rect_for(Rect { x: 0, y: 0, w: 10, h: 10 }, 100, 100).unwrap();
    assert_eq!(got, Rect { x: 25, y: 25, w: 10, h: 10 });
}

#[test]
fn a_rotation_back_projects_the_corners_and_keeps_the_tap_radius() {
    // ⚠⚠ The discriminating property, and the one a naive implementation gets wrong: the input
    // rect must CONTAIN every source pixel `rotate_region` will actually sample. This asserts it
    // against the real inverse map rather than against a re-derivation — re-deriving it in the
    // test proves only that the same mistake was made twice.
    let g = Geometry {
        angle: 8.0,
        crop: CropRect::default(),
        orientation: Orientation::default(),
    };
    let s = GeometryStage::new(&g, Resampler::CatmullRom);
    let (w, h) = (400usize, 300usize);
    let out = Rect { x: 120, y: 90, w: 40, h: 30 };
    let got = s.input_rect_for(out, w, h).unwrap();

    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = (8.0f32).to_radians().sin_cos();
    let pad = Resampler::CatmullRom.radius() as f32;
    for oy in out.y..out.y + out.h {
        for ox in out.x..out.x + out.w {
            let dx = ox as f32 - cx;
            let dy = oy as f32 - cy;
            let sx = cx + dx * cos + dy * sin;
            let sy = cy - dx * sin + dy * cos;
            // Every tap the resampler will read, not merely the centre.
            for (tx, ty) in [(sx - pad, sy - pad), (sx + pad, sy + pad)] {
                let tx = tx.clamp(0.0, w as f32 - 1.0);
                let ty = ty.clamp(0.0, h as f32 - 1.0);
                assert!(
                    tx >= got.x as f32 - 1.0 && tx <= (got.x + got.w) as f32
                        && ty >= got.y as f32 - 1.0 && ty <= (got.y + got.h) as f32,
                    "sample ({tx}, {ty}) for output ({ox}, {oy}) falls outside {got:?}"
                );
            }
        }
    }
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p pipeline -- geometry_declines an_unrotated_crop a_rotation_back_projects 2>&1 | grep -E "^test result:" 
```

⚠ `cargo test` takes **one** filter, not a list — pass several only after `--`, as above. Expected:
FAIL, three tests, `None` where a `Rect` is expected.

- [ ] **Step 3: Implement it**

In `crates/pipeline/src/stages/geometry.rs`, inside `impl PipelineStage for GeometryStage`:

```rust
    /// ⚠⚠ **Declines on a non-identity orientation, on exactly the condition
    /// [`Self::apply_region`] declines on.** The two are a pair: a plan built from one and executed
    /// by the other must never disagree about what is possible.
    ///
    /// ⚠ Otherwise two steps. The crop is a pure offset — `out` is in cropped coordinates, so the
    /// rect in the rotated canvas is `out` shifted by the crop origin. The rotation is then
    /// back-projected through **the same inverse map `rotate_region` uses**, and because a rotation
    /// is affine it sends lines to lines, so the bounding box of the four mapped corners contains
    /// the image of the whole rect **exactly** rather than conservatively.
    ///
    /// ⚠ Plus the resampler's tap radius, because `resample::sample` reads `2 * radius` taps per
    /// axis around the point it is given, not just the pixel under it.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        if !self.orientation.is_identity() {
            return None;
        }
        let (x0, y0, x1, y1) = crop_bounds(in_w, in_h, &self.crop);
        if out.w == 0 || out.h == 0 || out.x + out.w > x1 - x0 || out.y + out.h > y1 - y0 {
            return None;
        }
        let r = crate::Rect { x: x0 + out.x, y: y0 + out.y, w: out.w, h: out.h };
        if self.angle_rad == 0.0 {
            // No resampling, so a sub-rect of the crop is a sub-rect of the source — the same
            // branch `apply_region` takes.
            return Some(r);
        }
        let cx = (in_w as f32 - 1.0) / 2.0;
        let cy = (in_h as f32 - 1.0) / 2.0;
        let (sin, cos) = self.angle_rad.sin_cos();
        let (mut lo_x, mut lo_y) = (f32::INFINITY, f32::INFINITY);
        let (mut hi_x, mut hi_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        // The four corners, inclusive of the last pixel.
        let xs = [r.x as f32, (r.x + r.w - 1) as f32];
        let ys = [r.y as f32, (r.y + r.h - 1) as f32];
        for &y in &ys {
            for &x in &xs {
                let dx = x - cx;
                let dy = y - cy;
                let sx = cx + dx * cos + dy * sin;
                let sy = cy - dx * sin + dy * cos;
                lo_x = lo_x.min(sx);
                hi_x = hi_x.max(sx);
                lo_y = lo_y.min(sy);
                hi_y = hi_y.max(sy);
            }
        }
        let pad = self.resampler.radius();
        let x = (lo_x.floor().max(0.0) as usize).saturating_sub(pad);
        let y = (lo_y.floor().max(0.0) as usize).saturating_sub(pad);
        let x1b = ((hi_x.ceil().max(0.0) as usize) + pad + 1).min(in_w);
        let y1b = ((hi_y.ceil().max(0.0) as usize) + pad + 1).min(in_h);
        Some(crate::Rect {
            x,
            y,
            w: x1b.saturating_sub(x),
            h: y1b.saturating_sub(y),
        })
    }
```

⚠ `GeometryStage` must hold its `resampler`; if it does not, add the field in `new`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline geometry 2>&1 | grep -E "^test result:"
```

Expected: PASS, and **all 17 pre-existing geometry tests unchanged** — that is the evidence this
added rather than moved behaviour.

- [ ] **Step 5: Sabotage — drop the tap radius**

```bash
cp crates/pipeline/src/stages/geometry.rs /tmp/geo.bak
sed -i '' 's/let pad = self.resampler.radius();/let pad = 0usize;/' crates/pipeline/src/stages/geometry.rs
cargo test -p pipeline a_rotation_back_projects 2>&1 | grep -E "^test result:|falls outside" | head -3
cp /tmp/geo.bak crates/pipeline/src/stages/geometry.rs && touch crates/pipeline/src/stages/geometry.rs
```

Expected: FAIL with `sample (..) for output (..) falls outside Rect { .. }`.

- [ ] **Step 6: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/geometry.rs
git commit -m "feat: geometry back-projects an output rect to the input it reads

Declines on a non-identity orientation, on exactly the condition
apply_region declines on — the two are a pair, and a plan built from one and
executed by the other must never disagree about what is possible.

Otherwise the crop is a pure offset and the rotation is back-projected
through the same inverse map rotate_region uses. A rotation is affine and
sends lines to lines, so the bounding box of the four mapped corners
contains the image of the rect exactly rather than conservatively, plus the
resampler's tap radius because sample reads 2r taps per axis.

The test asserts containment against the real inverse map rather than a
re-derivation — re-deriving it would prove only that the same mistake was
made twice, which is how GeometryStage's own rotation sign error survived
once already."
```

---

### Task 6: Perspective's back-projection

**Files:**
- Modify: `crates/pipeline/src/stages/perspective.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn perspective_back_projects_every_sample_it_will_read() {
    // Same containment property as geometry's, against the real inverse map.
    let p = Perspective { vertical: 40.0, horizontal: 0.0, aspect: 0.0, scale: 0.0 };
    let s = PerspectiveStage::new(p, Resampler::CatmullRom);
    let (w, h) = (400usize, 300usize);
    let out = Rect { x: 100, y: 80, w: 50, h: 40 };
    let got = s.input_rect_for(out, w, h).unwrap();

    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let hom = Homography::from_edits(&p, cx / cy);
    let pad = Resampler::CatmullRom.radius() as f32;
    for oy in out.y..out.y + out.h {
        for ox in out.x..out.x + out.w {
            let n = ((ox as f32 - cx) / cy, (oy as f32 - cy) / cy);
            let sp = hom.source_of(n);
            let (sx, sy) = (cx + sp.0 * cy, cy + sp.1 * cy);
            for (tx, ty) in [(sx - pad, sy - pad), (sx + pad, sy + pad)] {
                let tx = tx.clamp(0.0, w as f32 - 1.0);
                let ty = ty.clamp(0.0, h as f32 - 1.0);
                assert!(
                    tx >= got.x as f32 - 1.0 && tx <= (got.x + got.w) as f32
                        && ty >= got.y as f32 - 1.0 && ty <= (got.y + got.h) as f32,
                    "sample ({tx}, {ty}) for output ({ox}, {oy}) falls outside {got:?}"
                );
            }
        }
    }
}

#[test]
fn an_inactive_perspective_asks_for_exactly_the_rect_it_was_given() {
    let p = Perspective::default();
    let s = PerspectiveStage::new(p, Resampler::CatmullRom);
    let out = Rect { x: 10, y: 10, w: 20, h: 20 };
    assert_eq!(s.input_rect_for(out, 200, 200), Some(out));
}

#[test]
fn a_corner_that_maps_past_the_horizon_declines() {
    // ⚠⚠ A homography's denominator vanishes at the horizon and the image of the rect is then
    // unbounded. `is_finite` is the guard, and it fails toward the whole-buffer path. Without it
    // the bbox would come back as garbage that silently reads the wrong pixels.
    let s = PerspectiveStage::new(
        Perspective { vertical: 100.0, horizontal: 0.0, aspect: 0.0, scale: 0.0 },
        Resampler::CatmullRom,
    );
    // A degenerate 2 px frame makes the normalisation extreme; the guard must hold rather than
    // produce a rect.
    assert!(s.input_rect_for(Rect { x: 0, y: 0, w: 1, h: 1 }, 1, 1).is_none());
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p pipeline -- perspective_back_projects an_inactive_perspective a_corner_that_maps 2>&1 | grep -E "^test result:"
```

Expected: FAIL.

- [ ] **Step 3: Implement it**

```rust
    /// ⚠ A homography sends lines to lines, so the image of a rectangle is a quadrilateral and the
    /// bounding box of its four mapped corners contains it **exactly** — the same argument as
    /// geometry's rotation, which is affine.
    ///
    /// ⚠⚠ **Except across the horizon**, where the denominator vanishes and the image is
    /// unbounded. `is_finite` on every mapped corner is the guard, and it declines — failing toward
    /// the whole-buffer path, where a wrong answer costs milliseconds rather than pixels.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        if out.w == 0 || out.h == 0 || in_w < 2 || in_h < 2 {
            return None;
        }
        if !self.params.is_active() {
            return Some(out);
        }
        let cx = (in_w as f32 - 1.0) / 2.0;
        let cy = (in_h as f32 - 1.0) / 2.0;
        let hom = Homography::from_edits(&self.params, cx / cy);
        let (mut lo_x, mut lo_y) = (f32::INFINITY, f32::INFINITY);
        let (mut hi_x, mut hi_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        let xs = [out.x as f32, (out.x + out.w - 1) as f32];
        let ys = [out.y as f32, (out.y + out.h - 1) as f32];
        for &y in &ys {
            for &x in &xs {
                let n = ((x - cx) / cy, (y - cy) / cy);
                let s = hom.source_of(n);
                let sx = cx + s.0 * cy;
                let sy = cy + s.1 * cy;
                if !sx.is_finite() || !sy.is_finite() {
                    return None;
                }
                lo_x = lo_x.min(sx);
                hi_x = hi_x.max(sx);
                lo_y = lo_y.min(sy);
                hi_y = hi_y.max(sy);
            }
        }
        let pad = self.resampler.radius();
        let x = (lo_x.floor().max(0.0) as usize).saturating_sub(pad);
        let y = (lo_y.floor().max(0.0) as usize).saturating_sub(pad);
        let x1 = ((hi_x.ceil().max(0.0) as usize) + pad + 1).min(in_w);
        let y1 = ((hi_y.ceil().max(0.0) as usize) + pad + 1).min(in_h);
        Some(crate::Rect { x, y, w: x1.saturating_sub(x), h: y1.saturating_sub(y) })
    }
```

⚠ `PerspectiveStage` must hold its `resampler` and `params`; both are already taken in `new`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline perspective 2>&1 | grep -E "^test result:"
```

Expected: PASS, and **all 54 pre-existing perspective tests unchanged**.

- [ ] **Step 5: Sabotage — remove the horizon guard**

```bash
cp crates/pipeline/src/stages/perspective.rs /tmp/persp.bak
sed -i '' 's/if !sx.is_finite() || !sy.is_finite() {/if false {/' crates/pipeline/src/stages/perspective.rs
cargo test -p pipeline a_corner_that_maps 2>&1 | grep -E "^test result:" | head -2
cp /tmp/persp.bak crates/pipeline/src/stages/perspective.rs && touch crates/pipeline/src/stages/perspective.rs
```

Expected: FAIL. ⚠ If it **passes**, the guard is unreachable on that fixture and the fixture is
wrong, not the guard — find an input whose corner genuinely maps to infinity, or record the guard as
defence in depth with the reasoning at the line. **A sabotage that does not fail is a finding about
the test, not a formality to wave through.**

- [ ] **Step 6: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/perspective.rs
git commit -m "feat: perspective back-projects an output rect to the input it reads

A homography sends lines to lines, so the image of a rectangle is a
quadrilateral and the bbox of its four mapped corners contains it exactly —
the same argument as geometry's rotation, which is affine.

Except across the horizon, where the denominator vanishes and the image is
unbounded. is_finite on every mapped corner declines, failing toward the
whole-buffer path."
```

---

### Task 7: `apply_region` becomes chainable

⚠⚠ Today `apply_region` assumes `src` is the **whole** input, which is true only for the *first*
scoped step. Chained, each step reads the previous step's region output, so the source's origin and
full logical dimensions have to travel with it.

**Files:**
- Modify: `crates/pipeline/src/lib.rs`, `crates/pipeline/src/stages/geometry.rs`,
  `crates/pipeline/src/stages/perspective.rs`, `crates/app/src/render_engine.rs` (the one call site)

- [ ] **Step 1: Add `RegionSrc` and change the signature**

In `crates/pipeline/src/lib.rs`:

```rust
/// A stage's input when only part of it exists.
///
/// ⚠⚠ **`buf` is a sub-rect of a larger logical image, and both halves are needed.** `origin` says
/// where `buf` sits in that image and `full` says how big it is — a stage whose result depends on
/// *where* a pixel is (the rotation's centre, the homography's normalisation) is wrong without
/// them, and wrong as a plausible photograph rather than as an error.
///
/// ⚠ The whole-buffer case is `origin == (0, 0)` and `full == buf`'s dimensions, which is what
/// makes M8.6's two implementations generalise rather than be rewritten.
#[derive(Debug, Clone, Copy)]
pub struct RegionSrc<'a> {
    pub buf: &'a ImageBuf,
    pub origin: (usize, usize),
    pub full: (usize, usize),
}

impl<'a> RegionSrc<'a> {
    /// The whole-buffer case.
    pub fn whole(buf: &'a ImageBuf) -> Self {
        Self { buf, origin: (0, 0), full: (buf.width(), buf.height()) }
    }

    /// Read the sample at a point in **full-image** coordinates, or `None` if `buf` does not hold
    /// it.
    ///
    /// ⚠ Returning `None` rather than clamping is deliberate: a stage reaching outside what the
    /// plan promised is a bug in `input_rect_for`, and clamping would hide it behind a plausible
    /// edge pixel.
    pub fn at(&self, x: usize, y: usize) -> Option<[f32; ImageBuf::CHANNELS]> {
        let lx = x.checked_sub(self.origin.0)?;
        let ly = y.checked_sub(self.origin.1)?;
        (lx < self.buf.width() && ly < self.buf.height()).then(|| self.buf.pixel(lx, ly))
    }
}
```

Change the trait method to:

```rust
    fn apply_region(&self, _src: &RegionSrc<'_>, _rect: Rect) -> Option<ImageBuf> {
        None
    }
```

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn a_region_src_reads_in_full_image_coordinates() {
    let mut buf = ImageBuf::new(4, 4);
    buf.set_pixel(1, 1, [0.5, 0.25, 0.125]);
    let src = RegionSrc { buf: &buf, origin: (10, 20), full: (100, 100) };
    assert_eq!(src.at(11, 21), Some([0.5, 0.25, 0.125]));
    assert_eq!(src.at(9, 21), None, "left of the region");
    assert_eq!(src.at(11, 19), None, "above the region");
    assert_eq!(src.at(14, 21), None, "right of the region");
}

#[test]
fn the_whole_buffer_case_is_the_identity() {
    let buf = ImageBuf::new(8, 6);
    let src = RegionSrc::whole(&buf);
    assert_eq!(src.origin, (0, 0));
    assert_eq!(src.full, (8, 6));
}
```

- [ ] **Step 3: Run and watch them fail, then implement**

```bash
cargo test -p pipeline -- a_region_src_reads the_whole_buffer_case 2>&1 | grep -E "^test result:"
```

Expected: FAIL (`RegionSrc` not found), then PASS after step 1's code is in.

- [ ] **Step 4: Port geometry and perspective to the new source**

Both currently take `src: &ImageBuf` and call `resample::sample(src, sx, sy, weights)`. They now
take `src: &RegionSrc<'_>`.

⚠⚠ **`resample::sample` must learn to read through a `RegionSrc`**, or every sample is offset by
the region's origin — which renders a plausible photograph of the wrong part of the image, the
exact failure M8.6's centre sabotage produced. Add beside it:

```rust
/// Sample a region-scoped source at a point in **full-image** coordinates.
///
/// ⚠⚠ Coordinates are the FULL image's, not the buffer's. A sampler that took buffer coordinates
/// would silently shift every read by the region's origin, which is a plausible photograph of the
/// wrong part of the frame — M8.6's centre sabotage, which all 63 goldens were blind to.
///
/// ⚠ A tap outside what the plan promised is **clamped to the region's own edge** rather than
/// returning black. The plan's margin is what guarantees this does not happen in the interior; at
/// the very edge of the full image it is the same replicate rule the whole-buffer path uses.
pub fn sample_region(
    src: &crate::RegionSrc<'_>,
    x: f32,
    y: f32,
    weights: &Weights,
) -> [f32; ImageBuf::CHANNELS] {
    // Delegate to the existing sampler over the sub-buffer, with the point moved into its space.
    sample(
        src.buf,
        x - src.origin.0 as f32,
        y - src.origin.1 as f32,
        weights,
    )
}
```

Then in `rotate_region` replace `resample::sample(src, sx, sy, weights)` with
`resample::sample_region(src, sx, sy, weights)`, and take `w`/`h`/`cx`/`cy` from **`src.full`**,
not from `src.buf`:

```rust
    let (w, h) = src.full;
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
```

⚠⚠ **That is the load-bearing line.** The rotation's centre is a property of the *frame*, not of
the slice in hand; taking it from `src.buf` re-centres the rotation on whatever region is being
rendered. Make the identical change in `PerspectiveStage::render` for `cx`, `cy` and `aspect`.

- [ ] **Step 5: The pre-existing region tests must still pass unchanged**

```bash
cargo test -p pipeline -- geometry perspective 2>&1 | grep -E "^test result:"
```

Expected: PASS. ⚠ The existing `a_region_scoped_render_matches...` tests call `apply_region` with a
whole buffer, so they now pass `&RegionSrc::whole(&img)` — a mechanical edit at the call sites, and
their **assertions must not change**. That is what says this generalised rather than moved behaviour.

- [ ] **Step 6: Sabotage — take the centre from the buffer instead of the frame**

```bash
cp crates/pipeline/src/stages/geometry.rs /tmp/geo2.bak
sed -i '' 's/let (w, h) = src.full;/let (w, h) = (src.buf.width(), src.buf.height());/' crates/pipeline/src/stages/geometry.rs
cargo test -p pipeline geometry 2>&1 | grep -E "^test result:" | head -2
cp /tmp/geo2.bak crates/pipeline/src/stages/geometry.rs && touch crates/pipeline/src/stages/geometry.rs
```

Expected: FAIL. ⚠ If it passes, no test exercises a non-zero origin yet — which is true until
Task 8's chain test exists. Record that and **re-run this sabotage at the end of Task 8**, where it
must fail.

- [ ] **Step 7: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src crates/app/src/render_engine.rs
git commit -m "feat: apply_region takes a RegionSrc, so scoped steps can chain

apply_region assumed src was the whole input, true only for the first
scoped step. Chained, each step reads the previous step's region output, so
the source's origin and full logical dimensions travel with it.

The whole-buffer case is origin (0,0) and full == buf's dims, which is what
makes M8.6's two implementations generalise rather than be rewritten.

sample_region reads in FULL-image coordinates. A sampler taking buffer
coordinates would shift every read by the region's origin — a plausible
photograph of the wrong part of the frame, which is M8.6's centre sabotage,
and all 63 goldens were blind to that one.

The rotation's centre and the homography's normalisation now come from
src.full rather than src.buf. That is the load-bearing line: they are
properties of the frame, not of the slice in hand."
```

---

### Task 8: `region_plan` — the backwards walk

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

⚠⚠ **Ordering note.** These tests are written against `build_prefix_steps`'s **current**
four-argument form. Task 10 adds a fifth parameter (`airlight`) and the compiler will name these
call sites along with the other 25 — add `None` to each then, not now. Writing them five-argument
here is the ordering bug this note exists to prevent.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_plan_walks_backwards_growing_the_rect_at_each_local() {
    // geometry (no angle, no crop) -> NR (reach 32) -> dehaze (reach 140).
    let mut e = Edits::default();
    e.noise_reduction = edit_model::NoiseReduction {
        luminance: 0.0, color: 50.0, detail: 50.0, smoothness: 100.0,
    };
    e.dehaze = edit_model::Dehaze { amount: 60.0 };
    let steps = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
    let visible = Rect { x: 1000, y: 1000, w: 200, h: 200 };
    let plan = region_plan(&steps, 0, visible, 4000, 3000).expect("every step supports a region");

    // The LAST step produces exactly the visible rect.
    assert_eq!(*plan.out_rects.last().unwrap(), visible);
    // Dehaze reads 140 beyond it, so NR must produce that.
    assert_eq!(plan.out_rects[plan.out_rects.len() - 2], visible.grown(140, 4000, 3000));
}

#[test]
fn a_declining_step_abandons_the_whole_plan() {
    // ⚠⚠ THE structural rule: a whole-buffer step needs a whole-buffer input, so one decline
    // anywhere at or after `start` kills everything above it too.
    let mut e = Edits::default();
    e.dehaze = edit_model::Dehaze { amount: 60.0 };
    e.spots = vec![edit_model::Spot::default()];
    let steps = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
    let visible = Rect { x: 10, y: 10, w: 20, h: 20 };
    // Starting at the spot step: it declines, so no plan.
    let spot_at = steps.iter().position(|s| s.stage.name() == "spot_removal").unwrap();
    assert!(region_plan(&steps, spot_at, visible, 400, 300).is_none());
    // Starting after it: fine.
    assert!(region_plan(&steps, spot_at + 1, visible, 400, 300).is_some());
}

#[test]
fn a_masked_capture_blocks_everything_because_it_is_last() {
    // ⚠⚠ The caveat as a test. Masked captures sit at the END of the prefix, so one declining
    // step there switches the feature off for the whole photo — there is nothing after it to scope.
    // A test that only covered spots would miss this, and spots are the *contained* case.
    let mut e = Edits::default();
    e.dehaze = edit_model::Dehaze { amount: 60.0 };
    let mut m = edit_model::Mask::default();
    m.adjustments.dehaze = edit_model::Dehaze { amount: 40.0 };
    e.masks = vec![m];
    let steps = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
    assert!(
        steps.iter().any(|s| s.stage.name() == "mask"),
        "fixture check: the masked capture step must actually be in the prefix"
    );
    let visible = Rect { x: 10, y: 10, w: 20, h: 20 };
    assert!(
        region_plan(&steps, 0, visible, 400, 300).is_none(),
        "a masked capture is last, so nothing above it can be scoped either"
    );
}

#[test]
fn the_plan_reports_the_frame_dimensions_the_whole_path_would_produce() {
    // ⚠⚠ M8.6's sharpest hazard, generalised. These feed CoverageSpec { fw, fh } and the tail's
    // Region { full_w, full_h }; taking them off a region-scoped buffer rasterises every brush
    // mask at the region's size and thrashes the coverage cache on every pan.
    let mut e = Edits::default();
    e.geometry.crop = edit_model::CropRect { left: 0.0, top: 0.0, right: 0.5, bottom: 0.5 };
    let steps = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
    let plan = region_plan(&steps, 0, Rect { x: 0, y: 0, w: 10, h: 10 }, 400, 300).unwrap();
    assert_eq!(plan.frame, (200, 150), "the CROPPED frame, not the region and not the input");
}

#[test]
fn an_empty_step_list_has_no_plan() {
    // Nothing to scope is not a plan of length zero — it is no plan, so the caller takes the
    // whole-buffer path rather than believing it scoped something.
    assert!(region_plan(&[], 0, Rect { x: 0, y: 0, w: 4, h: 4 }, 64, 64).is_none());
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p pipeline region_plan 2>&1 | grep -E "^test result:"
```

Expected: FAIL — `cannot find function 'region_plan'`.

- [ ] **Step 3: Implement it**

```rust
/// What each scoped step must produce, and the frame the whole path would have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionPlan {
    /// One output rect per step in `steps[start..]`, in that order.
    pub out_rects: Vec<Rect>,
    /// The dimensions the *whole-buffer* path would end with.
    ///
    /// ⚠⚠ Composed through [`PipelineStage::output_dims`] and read **before** any step runs.
    /// Taking them off a region-scoped buffer rasterises every brush mask at the region's size.
    pub frame: (usize, usize),
}

/// Plan a region-scoped run of `steps[start..]` that ends by producing `visible`.
///
/// Walks **backwards** from `visible`, asking each step what input it needs
/// ([`PipelineStage::input_rect_for`]) and making that the previous step's output.
///
/// ⚠⚠ **Any decline abandons the whole plan**, and that is structural rather than conservative: a
/// step that must run whole-buffer needs a whole-buffer input, so every step above it must produce
/// the full frame too. One `None` anywhere at or after `start` therefore kills everything.
///
/// ⚠ Returns `None` for an empty range as well. Nothing to scope is **no plan**, not an empty one,
/// so the caller takes the whole-buffer path rather than believing it scoped something.
///
/// ⚠ Pure, and takes plain rects on purpose: margins, back-projection, composition and the abandon
/// rule are then testable with no render engine and no window.
pub fn region_plan(
    steps: &[PrefixStep],
    start: usize,
    visible: Rect,
    in_w: usize,
    in_h: usize,
) -> Option<RegionPlan> {
    if start >= steps.len() || visible.w == 0 || visible.h == 0 {
        return None;
    }
    // Forward first: the dimensions each step sees, and the frame the path ends at.
    let mut dims = Vec::with_capacity(steps.len() - start + 1);
    let (mut w, mut h) = (in_w, in_h);
    dims.push((w, h));
    for s in &steps[start..] {
        let (ow, oh) = s.stage.output_dims(w, h);
        dims.push((ow, oh));
        w = ow;
        h = oh;
    }
    let frame = (w, h);

    // Backwards: each step's required output, from the last to `start`.
    let n = steps.len() - start;
    let mut out_rects = vec![visible.clamped(frame.0, frame.1)?; n];
    for i in (0..n).rev() {
        let (iw, ih) = dims[i];
        let need = steps[start + i]
            .stage
            .input_rect_for(out_rects[i], iw, ih)?;
        if i > 0 {
            out_rects[i - 1] = need;
        }
    }
    Some(RegionPlan { out_rects, frame })
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline region_plan 2>&1 | grep -E "^test result:"
```

Expected: PASS.

- [ ] **Step 5: Sabotage — turn the abandon rule into a skip**

```bash
cp crates/pipeline/src/lib.rs /tmp/lib.bak
sed -i '' 's/            .input_rect_for(out_rects\[i\], iw, ih)?;/            .input_rect_for(out_rects[i], iw, ih).unwrap_or(out_rects[i]);/' crates/pipeline/src/lib.rs
cargo test -p pipeline a_declining_step_abandons 2>&1 | grep -E "^test result:" | head -2
cp /tmp/lib.bak crates/pipeline/src/lib.rs && touch crates/pipeline/src/lib.rs
```

Expected: FAIL — the plan comes back `Some` where it must be `None`.

- [ ] **Step 6: Re-run Task 7's deferred sabotage**

Now that a chain test exercises a non-zero origin, the centre sabotage must fail:

```bash
cp crates/pipeline/src/stages/geometry.rs /tmp/geo3.bak
sed -i '' 's/let (w, h) = src.full;/let (w, h) = (src.buf.width(), src.buf.height());/' crates/pipeline/src/stages/geometry.rs
cargo test -p pipeline 2>&1 | grep -E "^test result:" | head -3
cp /tmp/geo3.bak crates/pipeline/src/stages/geometry.rs && touch crates/pipeline/src/stages/geometry.rs
```

Expected: FAIL. ⚠ If it still passes, **stop and write the test that can see it** before going on —
that is the single most dangerous hole in this design, and M8.6's version of it was invisible to all
63 goldens.

- [ ] **Step 7: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src/lib.rs
git commit -m "feat: region_plan walks backwards from the visible rect

Asks each step what input it needs and makes that the previous step's
output, from the last step to the resume point.

Any decline abandons the whole plan, and that is structural rather than
conservative: a step that must run whole-buffer needs a whole-buffer input,
so every step above it must produce the full frame too. One None anywhere
at or after start kills everything.

An empty range is None as well — nothing to scope is no plan, not an empty
one, so the caller takes the whole-buffer path rather than believing it
scoped something.

Pure and takes plain rects, so margins, back-projection, composition and
the abandon rule are testable with no render engine and no window."
```

---

### Task 9: Measure the airlight shift — BEFORE implementing it

⚠⚠ **This task produces a number and a decision, not a feature.** The spec commits to measuring the
shift before building on it: *"if it moves a picture visibly, that changes the design rather than
being absorbed."* Do not skip to Task 10.

**Files:**
- Create: `crates/app/examples/airlight_shift.rs`

- [ ] **Step 1: Write the probe**

```rust
//! How far the airlight moves when it is estimated from a 256 px replay of the prefix rather than
//! from dehaze's own full-resolution input.
//!
//! ⚠⚠ The measurement M8.7's spec commits to taking BEFORE the change is built. Three dehaze
//! goldens move either way; what this says is whether a *photograph* moves.
//!
//! Usage: `cargo run --release -p app --example airlight_shift <raw> [more raws...]`
use pipeline::{build_prefix_steps, PipelineStage, RenderScale, Resampler};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: airlight_shift <raw> [more raws...]");
        std::process::exit(2);
    }
    println!(
        "{:<28} {:>22} {:>22} {:>10}",
        "file", "airlight today", "airlight replayed", "8-bit"
    );
    for path in &args {
        let Ok(dec) = raw_decode::decode(
            std::path::Path::new(path),
            raw_decode::DecodeOptions::linear_scene(),
        ) else {
            eprintln!("{path}: decode failed");
            continue;
        };
        let img = dec.image;
        let mut edits = edit_model::Edits::default();
        edits.dehaze = edit_model::Dehaze { amount: 60.0 };
        // A crop and an angle, so the prefix genuinely has steps between the source and dehaze —
        // the case where the two definitions can disagree at all.
        edits.geometry.crop = edit_model::CropRect {
            left: 0.1, top: 0.1, right: 0.9, bottom: 0.9,
        };
        edits.geometry.angle = 3.0;
        edits.noise_reduction = edit_model::NoiseReduction {
            luminance: 30.0, color: 30.0, detail: 50.0, smoothness: 50.0,
        };

        let scale = RenderScale::FULL;
        let steps = build_prefix_steps(&edits, scale, None, Resampler::CatmullRom, None);
        let dehaze_at = steps.iter().position(|s| s.stage.name() == "dehaze").unwrap();

        // TODAY: run the prefix up to dehaze at full resolution, estimate from its output.
        let mut cur = img.clone();
        for s in &steps[..dehaze_at] {
            cur = s.stage.apply(cur);
        }
        let today = pipeline::stages::dehaze::airlight(&cur);

        // REPLAYED: downscale the SOURCE to 256, run the same steps at that scale, estimate.
        let replayed = pipeline::prefix_airlight(&edits, &img, Resampler::CatmullRom, None);

        // Report in 8-bit display levels, which is what a photograph is judged in.
        let enc = |v: f32| color::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0;
        let worst = (0..3)
            .map(|i| (enc(today[i]) - enc(replayed[i])).abs())
            .fold(0.0f32, f32::max);
        let name: String = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .take(28)
            .collect();
        println!(
            "{name:<28} {:>22} {:>22} {worst:>10.3}",
            format!("{:.4} {:.4} {:.4}", today[0], today[1], today[2]),
            format!("{:.4} {:.4} {:.4}", replayed[0], replayed[1], replayed[2]),
        );
    }
    println!("\n⚠ The '8-bit' column is the worst channel difference in display levels.");
    println!("  Under ~1.0 is invisible. Over ~3.0 changes a photograph and should change the design.");
}
```

⚠ This probe calls `pipeline::prefix_airlight` and the five-argument `build_prefix_steps`, both of
which Task 10 adds. **Write Task 10's
`prefix_airlight` first if the probe will not compile** — the measurement is of the *implementation*,
not of a sketch. What Task 9 forbids is building the rest of the airlight change (wiring it into
`build_prefix_steps`, re-authoring goldens) before reading the number.

- [ ] **Step 2: Run it on real frames**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example airlight_shift ~/Pictures/fuji/DSCF6296.RAF ~/Pictures/fuji/DSCF6546.RAF ~/Pictures/fuji/DSCF6291.RAF
```

⚠ `DSCF6296` (dehaze 70) and `DSCF6546` (dehaze 100) are the two frames in this catalogue that
actually use dehaze — CLAUDE.md records both.

- [ ] **Step 3: Read the number and decide**

- **Worst under ~1.0 of an 8-bit level** → proceed to Task 10 as designed. Record the measured band
  in the commit message and in the spec.
- **Worst over ~3.0** → ⚠⚠ **STOP and report.** The spec says this changes the design rather than
  being absorbed. The fallback is the spec's option C — the replay used only in the scoped path, no
  goldens moved, a small release-frame pop — and that is a decision for the user, not for the
  executor.
- **Between** → report the number and ask.

- [ ] **Step 4: Commit the probe**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/app/examples/airlight_shift.rs
git commit -m "bench: measure the airlight shift the 256 px replay introduces

The measurement M8.7's spec commits to taking BEFORE building on it. Three
dehaze goldens move either way; what this says is whether a photograph does.

Reports the worst channel difference in 8-bit display levels, on a fixture
carrying a crop, an angle and noise reduction — so the prefix genuinely has
steps between the source and dehaze, which is the only case where the two
definitions can disagree at all."
```

---

### Task 10: The airlight becomes a parameter

**Files:**
- Modify: `crates/pipeline/src/stages/dehaze.rs`, `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_supplied_airlight_is_used_verbatim_rather_than_re_estimated() {
    // ⚠⚠ The only test that can see the stage falling back to its own estimate. Two renders of
    // the SAME image with different supplied airlights must differ; if the stage re-estimates,
    // they are identical and this fails.
    let img = scene(96, 96); // the module's own fixture
    let p = Dehaze { amount: 60.0 };
    let a = DehazeStage::with_airlight(&p, RenderScale::FULL, [0.9, 0.9, 0.9]).apply(img.clone());
    let b = DehazeStage::with_airlight(&p, RenderScale::FULL, [0.5, 0.5, 0.5]).apply(img.clone());
    assert_ne!(a.data(), b.data(), "the supplied airlight must reach the pixels");
}

#[test]
fn the_replay_is_independent_of_where_the_render_resumes() {
    // ⚠⚠ THE property that made this design choose a replay from the START of the prefix. An
    // airlight that depended on `start` would change with WHICH SLIDER YOU DRAGGED — two facts
    // that can disagree, the shape render_max_dim and Session::geometry_mode were deleted for.
    let img = scene(96, 96); // the module's own fixture
    let mut e = Edits::default();
    e.dehaze = edit_model::Dehaze { amount: 60.0 };
    e.geometry.angle = 4.0;
    e.noise_reduction = edit_model::NoiseReduction {
        luminance: 30.0, color: 30.0, detail: 50.0, smoothness: 50.0,
    };
    let one = prefix_airlight(&e, &img, Resampler::CatmullRom, None);
    let two = prefix_airlight(&e, &img, Resampler::CatmullRom, None);
    assert_eq!(one, two, "deterministic");
    // And it is a function of the EDITS, so changing a downstream-only edit cannot move it.
    let mut f = e.clone();
    f.exposure = 1.5;
    assert_eq!(prefix_airlight(&f, &img, Resampler::CatmullRom, None), one);
}

#[test]
fn the_replay_moves_when_an_upstream_edit_moves() {
    // The discriminating other half: a constant would pass the test above.
    let img = scene(96, 96); // the module's own fixture
    let mut e = Edits::default();
    e.dehaze = edit_model::Dehaze { amount: 60.0 };
    let base = prefix_airlight(&e, &img, Resampler::CatmullRom, None);
    let mut f = e.clone();
    f.geometry.crop = edit_model::CropRect { left: 0.5, top: 0.5, right: 1.0, bottom: 1.0 };
    assert_ne!(
        prefix_airlight(&f, &img, Resampler::CatmullRom, None),
        base,
        "cropping to a different quarter must change what the brightest pixels are"
    );
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p pipeline -- a_supplied_airlight the_replay_is_independent the_replay_moves 2>&1 | grep -E "^test result:"
```

Expected: FAIL — `with_airlight` and `prefix_airlight` do not exist.

- [ ] **Step 3: Implement**

In `dehaze.rs`, add a constructor and a field. `DehazeStage::new` keeps its meaning (estimate from
the input) so every existing caller and test is untouched:

```rust
    /// Build with the airlight supplied rather than estimated from the stage's input.
    ///
    /// ⚠⚠ **This is what lets dehaze be region-scoped at all.** Estimating from the input makes
    /// the stage's true reach the entire frame, so no margin could ever be correct — and under a
    /// region-scoped run the estimate would come from whatever slice is on screen, which changes
    /// the photograph as you pan.
    pub fn with_airlight(p: &Dehaze, scale: RenderScale, airlight: [f32; 3]) -> Self {
        Self { params: *p, scale, airlight: Some(airlight) }
    }
```

and in `apply_with`, replace `let a = airlight(&image);` with:

```rust
        let a = self.airlight.unwrap_or_else(|| airlight(&image));
```

In `lib.rs`:

```rust
/// The airlight for `edits`, from a fixed 256 px replay of the prefix up to dehaze.
///
/// ⚠⚠ **It replays from the START of the prefix, never from a resume point.** Replaying from
/// wherever a render happens to resume would make the airlight depend on *which slider was
/// dragged* — two facts that can disagree, the shape `render_max_dim`, `fit_image_size` and
/// `Session::geometry_mode` were each deleted for. Starting from the source buffer makes it a
/// deterministic function of the edits alone: independent of the region, of the resume point, and
/// of the render scale.
///
/// ⚠ 256 px because that is what the estimator uses anyway — `AIRLIGHT_SAMPLE`, chosen at M8.3o
/// precisely so the estimate cannot vary with the render scale. The replay is a 65k-pixel run.
///
/// ⚠ Used by **both** the scoped and the whole-buffer paths, so they agree by construction rather
/// than approximately. The price is that this is not what dehaze computed before M8.7, because
/// downscale-then-process is not process-then-downscale — three goldens move.
pub fn prefix_airlight(
    edits: &Edits,
    src: &ImageBuf,
    resampler: Resampler,
    lens: Option<lens_profile::LensShot>,
) -> [f32; 3] {
    const SAMPLE: usize = 256;
    let small = src.downscale_to(SAMPLE);
    let long = small.width().max(small.height()).max(1);
    let full_long = src.width().max(src.height()).max(1);
    let scale = RenderScale::new(long as f32 / full_long as f32);
    // ⚠ `None` for the airlight, so the replay cannot recurse: dehaze is never constructed here.
    let steps = build_prefix_steps(edits, scale, lens, resampler, None);
    let upto = steps
        .iter()
        .position(|s| s.stage.name() == "dehaze")
        .unwrap_or(steps.len());
    let mut cur = small;
    for s in &steps[..upto] {
        cur = s.stage.apply(cur);
    }
    stages::dehaze::airlight(&cur)
}
```

- [ ] **Step 4: Thread it through `build_prefix_steps`**

Add an `airlight: Option<[f32; 3]>` parameter, and in the dehaze arm:

```rust
            stage: match airlight {
                Some(a) => Box::new(DehazeStage::with_airlight(&edits.dehaze, scale, a)),
                None => Box::new(DehazeStage::new(&edits.dehaze, scale)),
            },
```

⚠ The compiler names every call site. Pass `None` at each for now **except** in
`build_pipeline_split_in_region`, which Task 11 handles.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p pipeline 2>&1 | grep -E "^test result:"
```

- [ ] **Step 6: Sabotage — make the replay depend on the resume point**

```bash
cp crates/pipeline/src/lib.rs /tmp/lib2.bak
sed -i '' 's/    let mut cur = small;/    let mut cur = small; let _ = \&steps[..upto]; let upto = upto.saturating_sub(1);/' crates/pipeline/src/lib.rs
cargo test -p pipeline the_replay 2>&1 | grep -E "^test result:" | head -2
cp /tmp/lib2.bak crates/pipeline/src/lib.rs && touch crates/pipeline/src/lib.rs
```

Expected: FAIL on `the_replay_moves_when_an_upstream_edit_moves` or the independence test.

- [ ] **Step 7: Commit**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/pipeline/src
git commit -m "feat: the dehaze airlight becomes a parameter from a 256 px replay

Estimating from the stage's input makes dehaze's true reach the entire
frame, so no margin could ever be correct — and under a region-scoped run
the estimate would come from whatever slice is on screen, which changes the
photograph as you pan.

prefix_airlight replays from the START of the prefix, never from a resume
point. Replaying from wherever a render happens to resume would make the
airlight depend on which slider was dragged, which is two facts that can
disagree — the shape render_max_dim and Session::geometry_mode were each
deleted for.

256 px because that is what the estimator uses anyway: AIRLIGHT_SAMPLE was
chosen at M8.3o precisely so the estimate cannot vary with render scale.

DehazeStage::new keeps its meaning, so every existing caller is untouched
and a Dehaze drag's value is unchanged."
```

---

### Task 11: Re-author the three dehaze goldens

⚠ Only after Task 9's number came back acceptable.

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Modify: `tests/golden/dehaze_plus.png`, `dehaze_minus.png`, `mask_dehaze.png`

- [ ] **Step 1: Make the golden harness supply the replayed airlight**

The goldens render through `build_pipeline*`; those now take the airlight. Compute it once per case
with `prefix_airlight` and pass it, so the goldens test the shipped definition.

- [ ] **Step 2: Confirm exactly three references move, and no others**

```bash
cargo test -p pipeline --test golden 2>&1 | grep -E "^test result:|FAILED" | head
git status --short tests/
```

Expected: exactly `dehaze_plus`, `dehaze_minus` and `mask_dehaze` failing. ⚠⚠ **If any other golden
fails, STOP** — the airlight change has leaked into a stage it must not touch, and that is a bug
rather than a reference to re-author.

- [ ] **Step 3: Regenerate, then MEASURE rather than eyeball**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden 2>&1 | grep -E "^test result:"
git status --short tests/
```

Expected: exactly three PNGs modified.

- [ ] **Step 4: Assert the regenerated references still discriminate**

⚠⚠ A regenerated golden can be *clipped* or *flat*, and both look fine to the eye while testing
nothing. Measure:

```bash
python3 - <<'PY'
from PIL import Image
for n in ("dehaze_plus", "dehaze_minus", "mask_dehaze"):
    im = Image.open(f"tests/golden/{n}.png").convert("RGB")
    px = list(im.getdata())
    flat = [v for p in px for v in p]
    pinned = sum(1 for v in flat if v in (0, 255))
    print(f"{n}: {len(set(flat))} distinct, {pinned}/{len(flat)} pinned ({100*pinned/len(flat):.2f}%)")
PY
```

Expected: over ~100 distinct values and pinning comparable to what those references had before. ⚠ If
pinning jumped, the new airlight is clipping the chart and the *fixture* wants adjusting, not the
tolerance.

- [ ] **Step 5: Full gate and commit**

```bash
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git status --short tests/
git add -A && git commit -m "test: re-author the three dehaze goldens for the replayed airlight

The airlight is now a 256 px replay of the prefix rather than an estimate
from dehaze's own input, so its value moves slightly — downscale-then-
process is not process-then-downscale.

Exactly three references move and the other 60 are byte-identical, which is
what says the change is confined to dehaze. Measured rather than eyeballed:
distinct values and pinning are recorded above, because a regenerated
golden can be clipped or flat and both look fine to the eye while testing
nothing."
```

---

## Done when

- `cargo fmt` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --workspace`
  green together, **each exit code read from its own command**.
- **63 goldens run; 60 byte-identical, 3 re-authored.**
- The test count moved by **exactly** the number of tests added — asserted, not "still green".
- Every sabotage in this plan was run and **observed to fail**, with a `test result:` line present so
  a compile error cannot read as a pass.
- ⚠ Nothing calls `region_plan` yet. That is plan 2, and its carrier test is what stops this
  shipping green and dead.

---

## Spec coverage — what this plan does NOT do

Checked section by section against
`docs/superpowers/specs/2026-09-14-region-scoped-prefix-whole-design.md`. Deferred to **plan 2**,
deliberately and not by omission:

| spec § | item | why plan 2 |
|---|---|---|
| §7 | the gate in `render_cached`, and the swept area threshold | needs the render engine |
| §8 | the warm-behind request on settle | needs the engine's request queue |
| §8 | `prefix_cache.rs` split out as a pure move | `app`, and it goes first in plan 2 |
| §9 | `a_region_scoped_render_matches_the_whole_buffer_path_byte_for_byte`, multi-step, with a vignette | needs a full render through the engine |
| §9 | the undersized-margin sabotage | needs the byte-for-byte test above |
| §10 | the area threshold sweep, and the real drag measured through `RUSTROOM_DEBUG_RENDER` | needs the running app |

⚠⚠ **The byte-for-byte test is the carrier**, and it lives in plan 2. Until it exists this plan is
green and dead — every `input_rect_for` could return garbage and nothing here would notice, because
nothing runs the forward half against the backward half on real pixels. **Do not defer plan 2.**

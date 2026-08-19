# Region-Scoped Brush Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rasterize brush coverage over the rendered region rather than the whole frame — ~1.6 ms instead of ~24 ms at 1:1 — and, if a measurement supports it, render the pixel effect live during a drag.

**Architecture:** Smaller than it sounds. `rasterize` already clamps its bounding box to the frame; clipping means clamping to the region instead, plus skipping dabs that fall entirely outside it. `FrameMap` is untouched, so dab positions and the pre-geometry anchoring guarantee do not move. The region then enters `BrushCache`'s key — reversing a rule that is right for `PrefixCache` and wrong here.

**Tech Stack:** Rust, eframe/egui 0.35.

**Spec:** `docs/superpowers/specs/2026-08-19-region-scoped-coverage-design.md`

---

## Before you start

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Package is `app`** (`rustroom_app` is the lib target). **No `Co-Authored-By`. Never push.**
**Branch first** — four features in a row have; the user merges.

⚠ **Break the thing each test names, confirm the sabotage COMPILED, and watch it fail.** Eleven
recorded cases here of a test that could not detect its subject, plus one sabotage that failed to
compile while looking exactly like a pass.

```bash
sab () {
  if cargo build -p "$3" --tests 2>&1 | grep -q '^error'; then echo "$1: DID NOT COMPILE — invalid"
  else echo "$1: failures=$(cargo test -p "$3" "$2" 2>&1 | grep -cE 'FAILED')"; fi; }
```

⚠ **`cargo clippy … | grep -c` reports a COUNT. Read the output, not the count.** Twice in the
previous feature a non-zero count was reported and committed over.

⚠ **No golden may move.** `git status --short crates/pipeline/tests/golden/` must stay empty
throughout — with no region the behaviour is byte-identical, and that is the whole safety argument.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/brush/raster.rs` | `rasterize_in`, the clip and the dab skip | Modify |
| `crates/pipeline/src/stages/mask.rs` | Pass `self.region` as the clip | Modify |
| `crates/app/src/render_engine.rs` | Region in `BrushCache`'s key and its build | Modify |
| `crates/app/src/develop/canvas.rs` | Live rendering, **only if Task 4's number allows** | Modify |
| `crates/app/examples/bench_preview.rs` | Clipped-rasterize measurement | Modify |

No new files.

---

## Task 1: `rasterize_in`

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/pipeline/src/brush/raster.rs`'s test module:

```rust
    /// A stroke crossing most of the frame, so a clip genuinely cuts it.
    fn wide_stroke() -> Brush {
        brush(vec![BrushStroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            radius: 0.06,
            hardness: 40.0,
            flow: 100.0,
            erase: false,
        }])
    }

    #[test]
    fn a_clipped_raster_matches_the_whole_frame_one() {
        // ⚠⚠ The load-bearing test. It compares COVERAGE VALUES AT FRAME COORDINATES, never the
        // maps' fields: the boxes legitimately differ — that is the entire point of clipping — so
        // asserting on `x0` or `w` would fail against a correct implementation. Same trap
        // `a_stroke_survives_a_crop` had to avoid.
        let b = wide_stroke();
        let g = Geometry::default();
        let whole = rasterize(&b, &g, 80, 40);
        let clip = (30usize, 12usize, 20usize, 16usize);
        let part = rasterize_in(&b, &g, 80, 40, Some(clip));
        for y in clip.1..clip.1 + clip.3 {
            for x in clip.0..clip.0 + clip.2 {
                assert_eq!(part.at(x, y), whole.at(x, y), "at frame ({x}, {y})");
            }
        }
    }

    #[test]
    fn the_clip_does_not_shift_dab_positions() {
        // ⚠ The bug clipping invites: subtracting the clip origin somewhere it should not be,
        // which lands every stroke off by the region offset — visible only when zoomed, and easy
        // to mistake for a mask that "drifts". A single dab at a known frame position must stay
        // at that position under a clip whose origin is nowhere near zero.
        let b = brush(vec![dot(0.5, 0.5, 0.05, 100.0, false)]);
        let g = Geometry::default();
        let part = rasterize_in(&b, &g, 80, 40, Some((30, 12, 20, 16)));
        assert_eq!(part.at(40, 20), 255, "the dab centre is at frame (40, 20)");
        assert_eq!(part.at(31, 13), 0, "the clip's own corner is not painted");
    }

    #[test]
    fn a_dab_entirely_outside_the_clip_contributes_nothing() {
        let far = brush(vec![dot(0.9, 0.9, 0.03, 100.0, false)]);
        let part = rasterize_in(&far, &Geometry::default(), 80, 40, Some((0, 0, 20, 10)));
        assert!(part.is_inert(), "nothing inside the clip should be painted");
    }

    #[test]
    fn an_inverted_mask_still_covers_the_clipped_area() {
        // ⚠ The bbox-versus-outside bug must not come back through the clip. Inside the clip but
        // away from the stroke, an inverted mask is fully covered.
        let b = Brush {
            strokes: vec![dot(0.5, 0.5, 0.05, 100.0, false)],
            invert: true,
        };
        let part = rasterize_in(&b, &Geometry::default(), 80, 40, Some((0, 0, 20, 10)));
        assert_eq!(part.at(5, 5), 255, "inverted coverage inside the clip");
        assert_eq!(part.at(40, 20), 255, "and outside it, via `outside`");
    }

    #[test]
    fn no_clip_is_byte_identical_to_the_whole_frame() {
        let b = wide_stroke();
        let g = Geometry::default();
        assert_eq!(rasterize_in(&b, &g, 80, 40, None), rasterize(&b, &g, 80, 40));
    }

    #[test]
    fn a_clip_larger_than_the_frame_is_harmless() {
        // The canvas clamps its region, but a clip must not index out of bounds if one slips
        // through.
        let b = wide_stroke();
        let g = Geometry::default();
        let part = rasterize_in(&b, &g, 80, 40, Some((0, 0, 9999, 9999)));
        assert_eq!(part, rasterize(&b, &g, 80, 40));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline clip 2>&1 | tail -8`
Expected: FAIL — `cannot find function rasterize_in`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/brush/raster.rs`, rename the existing `pub fn rasterize` body into
`rasterize_in` with the extra parameter, and add a delegating `rasterize`:

```rust
/// Rasterize a brush into a coverage map over the frame.
///
/// The uncached whole-frame path: the CLI, every export, and the fit view.
pub fn rasterize(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
) -> CoverageMap {
    rasterize_in(brush, geometry, frame_w, frame_h, None)
}

/// Rasterize only what falls inside `clip` — the rendered region, as `(x0, y0, w, h)` in **frame
/// pixels**.
///
/// ⚠ **`FrameMap` is untouched and dab positions are unchanged.** Only the output box is clipped,
/// so a clipped map and a whole-frame one agree exactly at every frame coordinate inside the
/// clip. That is what makes this safe, and `a_clipped_raster_matches_the_whole_frame_one` is the
/// guard.
///
/// ⚠ `None` rasterizes the whole frame and is byte-identical to the pre-clip behaviour.
pub fn rasterize_in(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap {
    // ⚠ An inverted brush with no strokes covers everything, not nothing.
    let outside = if brush.invert { 255 } else { 0 };
    if brush.strokes.is_empty() || frame_w == 0 || frame_h == 0 {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }
    // The clip in frame pixels, clamped to the frame. `None` is the whole frame.
    let (cx0, cy0, cx1, cy1) = match clip {
        Some((x, y, w, h)) => (
            x.min(frame_w),
            y.min(frame_h),
            (x + w).min(frame_w),
            (y + h).min(frame_h),
        ),
        None => (0, 0, frame_w, frame_h),
    };
    if cx1 <= cx0 || cy1 <= cy0 {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    }
    let map = FrameMap::new(geometry, frame_w, frame_h);
    // ... the existing dab-collection loop, unchanged, EXCEPT the push sites below ...
```

Inside the dab-collection loop, filter each dab as it is produced. Replace the single-point push:

```rust
        let mut dabs = Vec::new();
        // ⚠ A dab entirely outside the clip cannot affect any pixel inside it, so it is dropped
        // before it can widen the bounding box or cost a buffer. This is where the saving comes
        // from on a long stroke that runs off-screen — the common case at 1:1.
        let mut push = |dabs: &mut Vec<Dab>, x: f32, y: f32| {
            if x + r < cx0 as f32 || x - r >= cx1 as f32 || y + r < cy0 as f32 || y - r >= cy1 as f32
            {
                return;
            }
            dabs.push(Dab { x, y, r, hard });
        };
        push(&mut dabs, first.0, first.1);
```

and the interpolated push:

```rust
            for i in 1..=n {
                let t = i as f32 / n as f32;
                push(&mut dabs, a.0 + dx * t, a.1 + dy * t);
            }
```

Finally, change the bounding-box clamp from the frame to the clip:

```rust
    let x0 = lx.floor().clamp(cx0 as f32, cx1 as f32) as usize;
    let y0 = ly.floor().clamp(cy0 as f32, cy1 as f32) as usize;
    let x1 = ((hx.ceil().clamp(cx0 as f32, cx1 as f32) as usize) + 1).min(cx1);
    let y1 = ((hy.ceil().clamp(cy0 as f32, cy1 as f32) as usize) + 1).min(cy1);
```

⚠ Leave every other `.min(x1)` / `.max(x0)` in the splat loops exactly as it is — they already
bound to the box, and the box is now the clipped one.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p pipeline 2>&1 | grep -E '^test result'`
Expected: all `ok`, and the golden suite still 38.

Run: `git status --short crates/pipeline/tests/golden/`
Expected: no output.

- [ ] **Step 5: Sabotage**

```bash
cp crates/pipeline/src/brush/raster.rs /tmp/rs.bak
sab () {
  if cargo build -p pipeline --tests 2>&1 | grep -q '^error'; then echo "$1: DID NOT COMPILE"
  else echo "$1: failures=$(cargo test -p pipeline "$2" 2>&1 | grep -cE 'FAILED')"; fi
  cp /tmp/rs.bak crates/pipeline/src/brush/raster.rs; touch crates/pipeline/src/brush/raster.rs; }
```

Offset the clip origin: change `x.min(frame_w)` to `(x + 1).min(frame_w)`.
`sab "S1 clip off by one" a_clipped_raster` — expect **failures ≥ 1**.

Shift dab positions: in `push`, change `dabs.push(Dab { x, y, r, hard })` to
`dabs.push(Dab { x: x - cx0 as f32, y: y - cy0 as f32, r, hard })`.
`sab "S2 dab shifted by the clip" the_clip_does_not_shift` — expect **failures ≥ 1**.

Drop the skip: make `push` always push.
`sab "S3 no dab skip" a_dab_entirely_outside` — ⚠ **may pass**, because a dropped dab and a dab
that paints nothing inside the clip give the same *output*. If it passes, that is a finding: the
skip is a pure optimisation, so **say so in its comment** and keep the test as a guard on the
output rather than on the branch.

Zero `outside` when clipping: set `let outside = 0;` after the clip is computed.
`sab "S4 inverted loses outside" an_inverted_mask_still_covers` — expect **failures ≥ 1**.

Restore all four, then `touch crates/pipeline/src/brush/raster.rs`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' -A4 | head
git add crates/pipeline/src/brush/raster.rs
git commit -F - <<'EOF'
feat(pipeline): rasterize_in — clip brush coverage to a rectangle

The bounding box already clamped to the frame; clipping is clamping to the
region instead, plus dropping dabs that fall entirely outside it before they can
widen the box or cost a buffer.

⚠ FrameMap is untouched, so dab positions and the pre-geometry anchoring
guarantee do not move. A clipped map and a whole-frame one agree exactly at every
frame coordinate inside the clip, which is the property that makes this safe.

`rasterize` delegates with `None` and is byte-identical to before — all 38
goldens unmoved.
EOF
```

---

## Task 2: `MaskStage` passes its region as the clip

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/pipeline/src/stages/mask.rs`'s test module:

```rust
    #[test]
    fn a_region_brush_render_still_matches_the_whole_frame_one() {
        // ⚠ The region path must remain bit-identical to the whole-frame path cropped, which is
        // what makes zooming safe — the same property
        // `a_region_render_matches_the_whole_render_cropped` pins for the pipeline at large. This
        // test existed before clipping; it is the one that would catch a clip that shifts or
        // truncates coverage, so re-run it deliberately rather than trusting the change.
        let whole =
            MaskStage::new(&painted(), RenderScale::FULL, &Geometry::default()).apply(flat(80, 40));
        let region = Region {
            full_w: 80,
            full_h: 40,
            x0: 30,
            y0: 10,
        };
        let got = MaskStage::new_in_region(
            &painted(),
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            None,
        )
        .apply(flat(20, 20));
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(got.pixel(x, y), whole.pixel(30 + x, 10 + y), "at ({x}, {y})");
            }
        }
    }
```

⚠ This duplicates `a_brush_mask_in_a_region_matches_the_same_part_of_the_whole` deliberately —
that test predates clipping and this one is named for what it now guards. If the existing one is
identical in body, **keep only one** and rename it; two copies of a property is a maintenance
cost, not extra safety.

- [ ] **Step 2: Pass the clip**

In `crates/pipeline/src/stages/mask.rs`, in `apply`, replace the rasterize call:

```rust
        let coverage = match &self.source {
            CoverageSource::Analytic(_) => None,
            CoverageSource::Painted {
                prebuilt: Some(c), ..
            } => Some(std::sync::Arc::clone(c)),
            CoverageSource::Painted { brush, geometry, .. } => {
                // ⚠ Clip to this render's region. `w`/`h` are the incoming image's dimensions,
                // which *are* the region's size, and `x0`/`y0` its frame-space origin. With no
                // region the clip is `None` and the result is the whole frame, byte for byte.
                let clip = self.region.map(|r| (r.x0, r.y0, w, h));
                Some(std::sync::Arc::new(crate::brush::rasterize_in(
                    brush, geometry, fw, fh, clip,
                )))
            }
        };
```

⚠ `w` and `h` are already in scope at the top of `apply` as the incoming image's dimensions —
check with `grep -n 'let (w, h) = (image.width' crates/pipeline/src/stages/mask.rs`.

- [ ] **Step 3: Verify**

Run: `cargo test -p pipeline 2>&1 | grep -E '^test result'`
Expected: all `ok`.

Run: `git status --short crates/pipeline/tests/golden/`
Expected: no output.

- [ ] **Step 4: Sabotage**

Change the clip to `(0, 0, w, h)` — the right size, the wrong origin.
`sab "S1 clip origin dropped" a_region_brush_render` — expect **failures ≥ 1**. Restore and
`touch`.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/stages/mask.rs
git commit -F - <<'EOF'
feat(pipeline): MaskStage clips brush coverage to its region

The uncached path now rasterizes only the region it is rendering. With no region
the clip is `None` and the output is byte-identical — all 38 goldens unmoved.
EOF
```

---

## Task 3: the region enters `BrushCache`'s key

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/render_engine.rs`'s test module:

```rust
    #[test]
    fn the_brush_cache_misses_when_the_region_moves() {
        // ⚠⚠ This REVERSES the rule PrefixCache follows, deliberately. Keeping the region out of
        // the key buys a cache that survives a pan — invisible, because panning re-runs the whole
        // tail anyway — and costs a full-frame rasterization on every paint frame: 23.76 ms at
        // 6246x4170, measured, and reported in use as "very sluggish at 1:1".
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let a = cache.get_or_build(&e, 200, 100, Some((0, 0, 50, 50)));
        let b = cache.get_or_build(&e, 200, 100, Some((0, 0, 50, 50)));
        assert!(Arc::ptr_eq(&a, &b), "the same region must hit");
        let c = cache.get_or_build(&e, 200, 100, Some((20, 0, 50, 50)));
        assert!(!Arc::ptr_eq(&a, &c), "a moved region must miss");
    }

    #[test]
    fn the_brush_cache_still_hits_when_only_a_slider_moves() {
        // The cache's remaining job, and the reason it exists at all: dragging an unrelated
        // slider with a brush mask present must not re-rasterize on every frame.
        let mut cache = BrushCache::default();
        let a = cache.get_or_build(&brush_edits(), 200, 100, None);
        let mut louder = brush_edits();
        louder.exposure = 1.5;
        let b = cache.get_or_build(&louder, 200, 100, None);
        assert!(
            Arc::ptr_eq(&a, &b),
            "a global slider does not touch the strokes and must hit"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app brush_cache 2>&1 | tail -8`
Expected: FAIL to compile — `get_or_build` takes 4 arguments.

- [ ] **Step 3: Add the region to the key and the build**

In `crates/app/src/render_engine.rs`:

```rust
#[derive(PartialEq)]
struct BrushKey {
    brushes: Vec<Option<edit_model::Brush>>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    /// The rendered region as `(x0, y0, w, h)` in frame pixels, or `None` for the whole frame.
    ///
    /// ⚠⚠ **In the key on purpose, and this is the OPPOSITE of `PrefixCache`'s rule.** That rule
    /// — region out of the key, so panning never invalidates — is right for the prefix, which
    /// runs whole-buffer and whose output *is* the frame. Copied to the brush it buys a cache
    /// that survives a pan (invisible, since panning re-runs the whole tail regardless) and costs
    /// a full-frame rasterization per paint frame. **Do not "unify" the two caches.**
    ///
    /// ⚠ It is also what stops a map clipped to one region being handed to a `MaskStage`
    /// rendering another, which would sample `outside` across the mismatch — a mask that fades
    /// out mid-image rather than an error.
    region: Option<(usize, usize, usize, usize)>,
}
```

`key` and `get_or_build` both gain the parameter, and the build passes it as the clip:

```rust
    fn key(
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
    ) -> BrushKey {
        BrushKey {
            brushes: edits
                .masks
                .iter()
                .map(|m| match &m.mask {
                    edit_model::Mask::Brush(b) => Some(b.clone()),
                    _ => None,
                })
                .collect(),
            geometry: edits.geometry,
            fw,
            fh,
            region,
        }
    }

    fn get_or_build(
        &mut self,
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
    ) -> Arc<pipeline::BrushCoverage> {
        let key = Self::key(edits, fw, fh, region);
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            let hit = self.entries.remove(i);
            let out = Arc::clone(&hit.1);
            self.entries.insert(0, hit);
            return out;
        }
        let maps = edits
            .masks
            .iter()
            .map(|m| match &m.mask {
                edit_model::Mask::Brush(b) => Some(Arc::new(pipeline::brush::rasterize_in(
                    b,
                    &edits.geometry,
                    fw,
                    fh,
                    region,
                ))),
                _ => None,
            })
            .collect();
        let built = Arc::new(pipeline::BrushCoverage { maps });
        self.entries.insert(0, (key, Arc::clone(&built)));
        self.entries.truncate(Self::CAP);
        built
    }
```

- [ ] **Step 4: Pass the region at the call site**

In `render_cached`, beside the tail construction, replace the existing coverage build:

```rust
    // ⚠ The clip comes from `rect`, which carries the region's own width and height.
    // `Region` holds only the origin plus the full frame's size, so deriving the size as
    // "frame minus origin" would clip to the bottom-right corner of the frame rather than to
    // the viewport — larger than intended, and silently so, because the output would still be
    // correct and merely slow.
    let clip = rect.map(|r| (r.x, r.y, r.w, r.h));
    let coverage = brush_cache.get_or_build(edits, pw, ph, clip);
```

`RegionRect` is `{ x, y, w, h }` (`render_engine.rs:30`), and `rect` is the already-clamped
`Option<RegionRect>` computed a few lines above.

- [ ] **Step 5: Confirm the two clips agree**

`MaskStage::apply` derives its own clip as `(r.x0, r.y0, w, h)` from `Region` plus the incoming
image's dimensions; `render_cached` derives it as `(r.x, r.y, r.w, r.h)` from `RegionRect`. These
must be the same rectangle.

They are, by construction: `Region { x0: r.x, y0: r.y, .. }` is built from the same `rect`, and
the cropped image handed to the tail is `img.crop(r.x, r.y, r.w, r.h)` — so the image's `w`/`h`
*are* `r.w`/`r.h`. Confirm by reading those two lines:

```bash
grep -n 'x0: r.x\|img.crop(r.x' crates/app/src/render_engine.rs
```

⚠ Worth confirming rather than assuming: a mismatch would not error. A map clipped to one
rectangle sampled by a stage expecting another reads `outside` across the difference — a mask that
fades out partway across the image, which reads as a rendering artefact rather than a bug.

- [ ] **Step 6: Verify**

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' -A4 | head`
Expected: no output. ⚠ Read it; do not count it.

- [ ] **Step 7: Sabotage**

Remove `region` from `BrushKey`.
`sab "S1 region out of key" the_brush_cache_misses_when_the_region_moves app` — expect
**failures = 1**.

Add `exposure: edits.exposure` to `BrushKey`.
`sab "S2 unrelated field in key" the_brush_cache_still_hits app` — expect **failures = 1**.

Restore both, then `touch crates/app/src/render_engine.rs`.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -F - <<'EOF'
feat(app): the rendered region enters BrushCache's key

⚠⚠ This reverses the rule PrefixCache follows, deliberately. Region-out-of-key
is right for the prefix, which runs whole-buffer and whose output IS the frame.
Copied to the brush it buys a cache that survives a pan — invisible, since
panning re-runs the whole tail anyway — and costs a full-frame rasterization on
every paint frame. Both cache sites now say why they differ.

⚠ It also makes a clip mismatch unrepresentable: a map clipped to one region
handed to a MaskStage rendering another would sample `outside` across the
mismatch, fading out mid-image rather than erroring.
EOF
```

---

## Task 4: Measure, then decide on live rendering

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`
- Modify: `crates/app/src/develop/canvas.rs` (**only if the number allows**)

- [ ] **Step 1: Add a clipped case to the bench**

In `crates/app/examples/bench_preview.rs`, the direct-rasterize block added for M3.1c-B loops over
`[("fit 1600x1068", …), ("half …"), ("full 6246x4170", …)]`. Add a clipped measurement beside it,
inside the same block:

```rust
        // ⚠ The number the live-rendering decision rests on: a full-resolution frame with the
        // coverage clipped to a viewport-sized region, which is what painting at 1:1 now does.
        for (label, fw, fh) in [("full 6246x4170", 6246usize, 4170usize)] {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let cov = pipeline::brush::rasterize_in(
                    &brush,
                    &g,
                    fw,
                    fh,
                    Some((2400, 1600, 1400, 900)),
                );
                std::hint::black_box(&cov);
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{best:8.2} ms   brush rasterize CLIPPED to 1400x900, {label}");
        }
```

- [ ] **Step 2: Run it**

```bash
cargo run --release -q -p app --example bench_preview <YOUR.RAF> 2>&1 | grep -E 'brush|region tail|pack'
```

Record the clipped number, the `region tail 1400x900` number and the native `pack` number. **A
paint frame is their sum.**

- [ ] **Step 3: Decide against the thresholds the spec fixed in advance**

⚠ **Do not skip to the conclusion you expect.** M3.1c-B measured a real frame, got a borderline
number, and only real use revealed the threshold was measuring the wrong thing.

- **Sum under ~16 ms** → lift the `drag_stopped`-only rule; go to Step 4.
- **16–30 ms** → lift it, and record in the docs that this path is known-marginal, with the number.
- **Over ~30 ms** → **stop here.** Keep tint-only feedback and record why. Tasks 1–3 already made
  every zoomed render cheaper, which stands on its own.

- [ ] **Step 4: Lift the rule (only if Step 3 says so)**

In `crates/app/src/develop/canvas.rs`, in the brush branch's motion arm, set `moved` when a point
is recorded:

```rust
                        if keep {
                            stroke.points.push(src);
                            // ⚠ Live rendering, enabled by region-scoped coverage — a paint frame
                            // is now bounded by the viewport rather than by the image. Measured
                            // before enabling; see the spec's Outcome for the number.
                            moved = true;
                        }
```

⚠ **`drag_started` stays suppressed.** Rendering a one-point stroke you are about to extend was
pure waste and caused the "initial dot" bug — a stale full-resolution render landing after the
line was finished. Live rendering means rendering on **motion**, not on press.

- [ ] **Step 5: Add the canvas test (only if the rule was lifted)**

⚠ M3.1c-B produced six bugs, every one in canvas wiring and none caught by a test. This is the
first canvas change since that finding, and the seam already exists — use it.

Add to `crates/app/src/develop/canvas.rs`'s test module:

```rust
    #[test]
    fn a_press_without_motion_requests_no_render() {
        // ⚠ The regression guard for the "initial dot" bug: a one-point stroke must not trigger a
        // full-resolution render that lands after the line is finished and paints over it. This
        // asserts the *predicate* the canvas branches on, because constructing a live `Session`
        // headlessly is not worth the machinery — the branch it guards is one line away.
        //
        // `tool_drag_started` is true on the press frame and `tool_drag` becomes true only once
        // the pointer has moved, so the `else if` arm that sets `moved` cannot run on press.
        assert!(tool_drag_for(egui::PointerButton::Primary));
    }
```

⚠ If that assertion looks weak, it is — say so in the comment rather than dressing it up. A
stronger test needs a `Session`, and this project has declined to build that machinery before.

- [ ] **Step 6: Commit**

```bash
git add crates/app/examples/bench_preview.rs crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
perf(app): measure clipped rasterization, and render live if it fits

The clipped number is what the live-rendering decision rests on: a
full-resolution frame with coverage bounded by the viewport, which is what
painting at 1:1 now does.

⚠ drag_started stays suppressed either way. Rendering a one-point stroke you are
about to extend caused the "initial dot" bug — a stale full-res render landing
after the line was finished. Live rendering means rendering on motion.
EOF
```

---

## Task 5: Validate in the GUI and document

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-19-region-scoped-coverage-design.md`

- [ ] **Step 1: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' -A4 | head
cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}'
git status --short crates/pipeline/tests/golden/
```
Expected: no fmt output, **no clippy output** (read it), about 663 tests, **no golden moved**.

- [ ] **Step 2: Launch the GUI**

```bash
cargo run --release -p app
```

Background it and ask the user to check:

1. **Paint at 1:1 with the mask shown** — smooth, and if the rule was lifted, the pixel effect
   should now appear *as you drag* rather than on release.
2. **Pan at 1:1 while a brush mask is selected** — the region changes, so the cache misses; this
   is the path the reversal trades against and it must not feel worse than before.
3. **Drag Exposure with a brush mask present** — the region is unchanged, so this must stay smooth;
   it is the cache's remaining job.
4. **Invert, then pan** — the tint must stay welded to the image and cover the whole frame.
5. **Paint, then re-crop in Geometry** — strokes stay on the subject. The anchoring guarantee is
   untouched by this change and this confirms it.

- [ ] **Step 3: Fill in the spec's Outcome**

Add an "Outcome" section recording: the clipped measurement and which threshold branch was taken;
whether the dab-skip sabotage passed (making it an optimisation rather than a correctness guard);
and anything the GUI check found. ⚠ Where the spec was wrong, say so explicitly.

- [ ] **Step 4: Update `CLAUDE.md`**

- **▶ NEXT ACTION** becomes **the button/layout work**, whose four confirmed problems are already
  recorded — remove the region-scoped entry above it.
- Add a section in the house style. The ⚠ items that must appear: (1) **the region is now in
  `BrushCache`'s key and this reverses `PrefixCache`'s rule deliberately** — both sites say why;
  (2) `FrameMap` untouched, so anchoring did not move; (3) the measured clipped number and whether
  live rendering was enabled; (4) `drag_started` stays suppressed regardless; (5) the clip must be
  the region's own size, not the frame's remainder.
- Update the test count, add both filenames to the docs list, refresh the push-state line.

- [ ] **Step 5: Commit and report**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-19-region-scoped-coverage-design.md docs/superpowers/plans/2026-08-19-region-scoped-coverage.md
git commit -F - <<'EOF'
docs: record region-scoped brush coverage
EOF
git log origin/main..HEAD --oneline | wc -l
```

⚠ **Do not push.**

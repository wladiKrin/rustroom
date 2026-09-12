# M8.3k plan 2 — wiring the GPU brush rasteriser into `MaskStage`

> **⚠ Conventions that are not negotiable (this repo):** commit messages carry **no
> `Co-Authored-By` trailer**; branch `m8.3k-gpu-brush-coverage` (plan 1 is already on it);
> **never push** — the user pushes to `github.com/wladiKrin/rustroom` himself.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** `MaskStage` builds a brush's coverage on the GPU when the app offers it a rasteriser, so
the 62–65 ms plan 1 measured becomes ~27 ms on a real frame.

**Architecture:** `pipeline` gains a `CoverageRasterizer` trait and one more defaulted
`PipelineStage` hook to inject it — the same seam shape M8.3j's `accelerate_inner` established,
for the same dependency reason. `gpu-pipeline` implements the trait for `GpuBrushRaster` and
`accelerate`'s `walk` offers it to every stage it passes.

**Spec:** `docs/superpowers/specs/2026-09-12-gpu-brush-coverage-design.md`

---

## ⚠⚠ Two corrections to the spec, both found by reading the code before planning

**1. The shared upload is NOT in this plan, and the spec's §5 oversells it.** That section calls
reusing M8.3j's resident buffer "the design's one real economy". It is real — plan 1 measured the
base upload at **~12 ms of the ~27** (15.4 ms with no base against ~27 with one) — but claiming it
needs `GpuChain` to accept a pre-resident buffer instead of an `ImageBuf`, plus `MaskStage`
holding that buffer across the coverage pass and the sub-stage chain. That is a second cross-crate
mechanism, and bolting it onto this plan would mean neither part is provable on its own.

**So plan 2 wires the rasteriser in and pays its own upload**, which plan 1 already measured as
worth 2.14–2.38×. The sharing is recorded as a follow-up **with its number** rather than assumed.

**2. ⚠⚠ THE BASE IS INDEXED WRONG FOR A REGION RENDER, AND EVERY PLAN 1 TEST IS BLIND TO IT.**
`BaseView::at` takes **frame** coordinates and maps them into a region-local buffer:

```rust
let lx = x.saturating_sub(self.x0).min(self.image.width() - 1);
let ly = y.saturating_sub(self.y0).min(self.image.height() - 1);
```

The shader does `base[(fy * img_w + fx) * 3]` — no origin, no clamp. That is correct **only** when
`x0 == y0 == 0` and the buffer covers the frame, which is exactly what plan 1's fixtures and the
probe use. **A region render at 1:1 would read pixels shifted by the region origin**, and the
symptom is an auto-mask gating against the wrong colours: a plausible mask, not an error. Task 2
fixes it and Task 3 is the test that can see it.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/lib.rs` | the seam | **Modify** — `CoverageRasterizer`, `set_coverage_rasterizer` |
| `crates/pipeline/src/brush/raster.rs` | `BaseRef` | **Modify** — the origin the GPU needs |
| `crates/pipeline/src/stages/mask.rs` | `MaskStage` | **Modify** — hold one, use it in `build_map` |
| `crates/gpu-pipeline/src/brush_raster.rs` | the impl | **Modify** — implement the trait, take the origin |
| `crates/gpu-pipeline/src/brush_wgsl.rs` | the shader | **Modify** — offset and clamp the base read |
| `crates/gpu-pipeline/src/accelerate.rs` | injection | **Modify** — `walk` offers it |
| `crates/app/examples/bench_gpu_tail.rs` | measurement | **Modify** — a global-drag row |

---

## Task 1: The seam in `pipeline`

**Files:**
- Modify: `crates/pipeline/src/lib.rs`, `crates/pipeline/src/brush/raster.rs`
- Test: `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Add `BaseRef` to `raster.rs`**

The GPU needs the plane, its dimensions **and its frame origin** — everything `BaseView` closes
over. A tuple would make the origin easy to drop at a call site, which is the bug §2 above
describes, so it is a named struct:

```rust
/// A base plane as a GPU rasteriser needs it: contiguous samples plus where they sit in the frame.
///
/// ⚠⚠ **The origin is NOT optional and is the whole reason this is a struct rather than a tuple.**
/// `BasePixels::at` takes **frame** coordinates; a region render's buffer covers only part of the
/// frame, so a consumer indexing `(fy * w + fx)` reads pixels shifted by the origin. That renders
/// an auto-mask gating against the wrong colours — a plausible mask rather than an error — and
/// every whole-frame fixture is blind to it.
#[derive(Clone, Copy)]
pub struct BaseRef<'a> {
    pub plane: &'a [f32],
    pub w: usize,
    pub h: usize,
    /// The frame coordinate of the plane's `(0, 0)`.
    pub x0: usize,
    pub y0: usize,
}
```

Export it from `brush/mod.rs`'s explicit list.

- [ ] **Step 2: Add the trait and the hook to `lib.rs`**

```rust
/// Builds a brush's coverage from an already-planned dab list.
///
/// ⚠⚠ **This exists because `MaskStage` cannot call `gpu-pipeline`.** That crate depends on
/// `pipeline` and its own header records that nothing may depend on it — so the GPU rasteriser is
/// **injected** rather than referenced, exactly as [`SubStageAccelerator`] is and for the same
/// reason. A `pipeline` that reached upward would put wgpu into the CLI, `export` and all 63
/// goldens.
///
/// ⚠ `Send + Sync` because `MaskStage` is, and a mask runs under `map_pixels_xy`'s rayon rows.
pub trait CoverageRasterizer: Send + Sync {
    /// `None` declines, and the caller falls back to the CPU rasteriser.
    ///
    /// ⚠ Declining must always be safe: an adapter that cannot allocate, a box below the floor, or
    /// a plan this implementation does not handle all return `None` and cost only the attempt.
    /// **Fail toward the CPU** — the rule `GpuStage::cpu_cost` and `PrefixKey` already follow,
    /// because a wrong answer here is wrong pixels rather than lost milliseconds.
    fn rasterize(
        &self,
        plan: &brush::DabPlan,
        invert: bool,
        base: Option<brush::BaseRef<'_>>,
    ) -> Option<brush::CoverageMap>;
}
```

and on `PipelineStage`, beside `accelerate_inner`:

```rust
    /// Offer this stage a coverage rasteriser. Default: this stage builds no coverage.
    ///
    /// ⚠ A second defaulted hook rather than a parameter on [`PipelineStage::accelerate_inner`]:
    /// that one transforms a **stage list** and this hands over a **collaborator**, so folding
    /// them together would give one method two unrelated jobs.
    fn set_coverage_rasterizer(&mut self, _r: &std::sync::Arc<dyn CoverageRasterizer>) {}
```

- [ ] **Step 3: `MaskStage` holds one and uses it**

Add the field, filled `None` by both constructors:

```rust
    /// A GPU coverage rasteriser, when the app has offered one.
    ///
    /// ⚠ `None` for the CLI, every export and all 63 goldens — none of which has a GPU context —
    /// so those paths are byte-for-byte unchanged, which is what keeps the goldens a valid guard.
    coverage_gpu: Option<std::sync::Arc<dyn crate::CoverageRasterizer>>,
```

the hook:

```rust
    fn set_coverage_rasterizer(&mut self, r: &std::sync::Arc<dyn crate::CoverageRasterizer>) {
        self.coverage_gpu = Some(r.clone());
    }
```

and in `build_map`'s `CoverageSource::Painted { brush, geometry, .. }` arm, replacing the direct
`rasterize_in` call:

```rust
                let clip = self.region.map(|r| (r.x0, r.y0, w, h));
                let view = BaseView { image: &image, x0, y0 };
                // ⚠⚠ **Planned ONCE and offered to both.** `plan_dabs` does the `SPACING`
                // interpolation, the per-dab gate reference, the clip rejection and the box; the
                // GPU and the CPU then consume the same plan, so they cannot disagree about which
                // dabs exist. A GPU path that planned its own would be a second copy of that
                // arithmetic.
                let built = crate::brush::plan_dabs(brush, geometry, fw, fh, clip, Some(&view))
                    .and_then(|plan| {
                        // ⚠ Fails toward the CPU: `None` from the rasteriser — no adapter, a box
                        // below its floor, an allocation refused — costs only the attempt.
                        let gpu = self.coverage_gpu.as_ref()?.rasterize(
                            &plan,
                            brush.invert,
                            Some(crate::brush::BaseRef {
                                plane: image.data(),
                                w: image.width(),
                                h: image.height(),
                                x0,
                                y0,
                            }),
                        )?;
                        Some(std::sync::Arc::new(gpu))
                    })
                    .unwrap_or_else(|| {
                        std::sync::Arc::new(crate::brush::rasterize_in(
                            brush, geometry, fw, fh, clip, Some(&view),
                        ))
                    });
```

⚠ `plan_dabs` is called even on the CPU path now, and `rasterize_in` plans again internally — so a
CPU fallback plans **twice**. Measured in Task 5; if it matters, `rasterize_planned` is already
extracted and the fallback can call it directly.

- [ ] **Step 4: Write the wiring test**

⚠⚠ **Pixels cannot see this.** Plan 1 established the two rasterisers agree to within 1 level, so a
`MaskStage` using the GPU renders the same picture — the W5 failure mode exactly. The test therefore
injects its **own** `CoverageRasterizer` and counts calls, which also means it needs no GPU:

```rust
    /// A rasteriser that records what it was asked and returns a map of its own.
    ///
    /// ⚠⚠ **A stub rather than the real GPU one, and a COUNTER rather than a pixel comparison.**
    /// The two rasterisers agree by design, so nothing in the output can say which ran — the same
    /// reason `PARALLEL_STROKES` and `Ran { bytes }` exist. This also lets the wiring be tested in
    /// `pipeline`, which has no GPU at all.
    struct StubRaster {
        calls: std::sync::atomic::AtomicUsize,
        origin: std::sync::Mutex<Option<(usize, usize)>>,
    }
    impl crate::CoverageRasterizer for StubRaster {
        fn rasterize(
            &self,
            plan: &crate::brush::DabPlan,
            invert: bool,
            base: Option<crate::brush::BaseRef<'_>>,
        ) -> Option<crate::brush::CoverageMap> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *self.origin.lock().unwrap() = base.map(|b| (b.x0, b.y0));
            // ⚠ Deliberately a DIFFERENT map from the CPU's — all-255 inside the box — so the test
            // can prove the stage used what it was handed rather than merely that it ran.
            Some(crate::brush::CoverageMap {
                x0: plan.x0,
                y0: plan.y0,
                w: plan.w,
                h: plan.h,
                data: vec![255; plan.w * plan.h],
                outside: if invert { 255 } else { 0 },
            })
        }
    }

    #[test]
    fn an_offered_rasterizer_builds_the_coverage_and_the_stage_uses_it() {
        let m = painted();
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let stub = std::sync::Arc::new(StubRaster {
            calls: std::sync::atomic::AtomicUsize::new(0),
            origin: std::sync::Mutex::new(None),
        });
        let as_trait: std::sync::Arc<dyn crate::CoverageRasterizer> = stub.clone();
        stage.set_coverage_rasterizer(&as_trait);

        let cpu_only = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let with_gpu = stage.apply(flat(80, 60));
        let without = cpu_only.apply(flat(80, 60));

        assert_eq!(
            stub.calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the stage must ask the rasteriser exactly once"
        );
        assert_ne!(
            with_gpu.data(),
            without.data(),
            "the stage must USE the map it was handed — an all-255 coverage cannot render the \\
             same picture as the real one"
        );
    }

    /// ⚠ Declining must fall back, not fail. A rasteriser that returns `None` must leave the
    /// render byte-identical to having offered nothing at all.
    #[test]
    fn a_rasterizer_that_declines_falls_back_to_the_cpu() {
        struct Decline;
        impl crate::CoverageRasterizer for Decline {
            fn rasterize(
                &self,
                _: &crate::brush::DabPlan,
                _: bool,
                _: Option<crate::brush::BaseRef<'_>>,
            ) -> Option<crate::brush::CoverageMap> {
                None
            }
        }
        let m = painted();
        let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let r: std::sync::Arc<dyn crate::CoverageRasterizer> = std::sync::Arc::new(Decline);
        stage.set_coverage_rasterizer(&r);
        let plain = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        assert_eq!(
            stage.apply(flat(80, 60)).data(),
            plain.apply(flat(80, 60)).data(),
            "declining must be byte-identical to offering nothing"
        );
    }
```

⚠ `painted()` and `flat()` already exist in that test module.

- [ ] **Step 5: Run and gate**

```bash
cargo test -p pipeline > /tmp/w1.log 2>&1; echo "EXIT=$?"
grep "test result: ok" /tmp/w1.log | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/' | awk '{s+=$1} END {print "tests:", s}'
grep -c "^test golden_" /tmp/w1.log
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w1c.log 2>&1; echo "CLIPPY=$?"
```

Expected: `EXIT=0`, **520 → 522**, **63** goldens and **none moved** — the goldens have no GPU
context, so `coverage_gpu` is `None` and their path is byte-for-byte what it was.

- [ ] **Step 6: Sabotage-verify**

Make `set_coverage_rasterizer` a no-op body (`{}`) and confirm
`an_offered_rasterizer_builds_the_coverage_and_the_stage_uses_it` **fails**. Restore and `touch`.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/
git commit -m "feat: a mask can be offered a coverage rasterizer"
```

---

## Task 2: The origin, and the shader that honours it

**Files:**
- Modify: `crates/gpu-pipeline/src/brush_raster.rs`, `crates/gpu-pipeline/src/brush_wgsl.rs`

- [ ] **Step 1: Change `rasterize`'s base parameter to `BaseRef`**

Replace `base: Option<(&[f32], usize, usize)>` with `base: Option<pipeline::brush::BaseRef<'_>>`
throughout `brush_raster.rs`, and carry `x0`/`y0` into `Params` as `base_x0`/`base_y0`.

⚠ Plan 1's tests and the probe pass whole-frame planes, so they become
`BaseRef { plane, w, h, x0: 0, y0: 0 }` — unchanged behaviour, which is what their continued
passing establishes.

- [ ] **Step 2: Offset and clamp the base read in the shader**

```wgsl
// ⚠⚠ **Frame coordinates in, region-local out — and the clamp is not defensive, it is the CPU's
// documented behaviour.** `BaseView::at` does `x.saturating_sub(x0).min(w - 1)`, so a frame
// coordinate left of or above the region reads the region's **edge** pixel rather than wrapping or
// erroring. `saturating_sub` on `u32` is `max(0, ...)`, which WGSL gets from `max`.
fn base_oklab(fx: u32, fy: u32) -> vec3<f32> {
    let lx = min(max(fx, p.base_x0) - p.base_x0, p.img_w - 1u);
    let ly = min(max(fy, p.base_y0) - p.base_y0, p.img_h - 1u);
    let i = (ly * p.img_w + lx) * 3u;
    return prophoto_to_oklab(base[i], base[i + 1u], base[i + 2u]);
}
```

- [ ] **Step 3: Implement the trait**

```rust
// ⚠ The trait lives in `pipeline` and is implemented here — the direction that keeps wgpu out of
// the CLI, `export` and the goldens.
impl pipeline::CoverageRasterizer for GpuBrushRaster {
    fn rasterize(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<pipeline::brush::BaseRef<'_>>,
    ) -> Option<CoverageMap> {
        // ⚠⚠ **A size floor, and it fails toward the CPU.** A tiny box cannot repay a round trip:
        // the coverage download alone is 4 bytes a pixel plus a fixed ~1.3 ms that no size
        // reduction touches (M8.3g's measurement). Swept in Task 5 rather than guessed.
        if plan.w.checked_mul(plan.h)? < MIN_GPU_BOX_PX {
            return None;
        }
        GpuBrushRaster::rasterize(self, plan, invert, base)
    }
}
```

⚠ `MIN_GPU_BOX_PX` starts at `64 * 1024` — `SEQUENTIAL_FLOOR_PX`'s value, chosen as a *starting*
point because it answers the same question on the CPU side — and is **swept in Task 5**. Do not
leave it unmeasured.

- [ ] **Step 4: Run plan 1's suite unchanged**

```bash
cargo test -p gpu-pipeline --release -- brush_raster 2>&1 | tail -15
```

Expected: **10 passed**. ⚠ Their assertions are **unchanged**, which is what makes their passing
evidence that the origin change moved no behaviour on a whole-frame plane.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w2c.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "fix: the base plane is indexed from the region's origin"
```

---

## Task 3: The region test — the guard plan 1 could not have

**Files:**
- Test: `crates/gpu-pipeline/src/brush_raster.rs`

- [ ] **Step 1: Write it**

```rust
    /// ⚠⚠ **The guard plan 1 structurally could not have: every one of its fixtures is
    /// whole-frame, so `x0 == y0 == 0` and a shader ignoring the origin agrees perfectly.** A
    /// region render's buffer covers only part of the frame, and reading it without the offset
    /// gates the auto-mask against pixels from somewhere else entirely — a plausible mask, not an
    /// error.
    ///
    /// ⚠ The fixture check is what gives it teeth: the origin must be **non-zero**, or this is
    /// just another whole-frame test wearing a different name.
    #[test]
    fn a_region_base_is_read_from_its_own_origin() {
        let Some(ctx) = ctx() else { return };
        let (plane, fw, fh) = half_base(400, 300);
        // The region: the right half of the frame, so its origin is far from zero.
        let (rx0, ry0, rw, rh) = (200usize, 60usize, 200usize, 180usize);
        assert!(rx0 > 0 && ry0 > 0, "fixture check: the origin must be non-zero");
        let mut sub = vec![0.0f32; rw * rh * 3];
        for y in 0..rh {
            let src = ((ry0 + y) * fw + rx0) * 3;
            sub[y * rw * 3..(y + 1) * rw * 3].copy_from_slice(&plane[src..src + rw * 3]);
        }

        let b = Brush {
            strokes: vec![BrushStroke {
                points: (0..16).map(|i| [0.52, 0.25 + i as f32 * 0.02]).collect(),
                radius: 0.05,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        };
        let g = Geometry::default();
        // ⚠ The CPU side sees the region through `BaseView`'s own mapping, which is the behaviour
        // being matched — so this test compares against the thing it must agree with rather than
        // against a re-derivation.
        struct RegionBase<'a>(&'a [f32], usize, usize, usize, usize);
        impl pipeline::brush::BasePixels for RegionBase<'_> {
            fn at(&self, x: usize, y: usize) -> [f32; 3] {
                let lx = x.saturating_sub(self.3).min(self.1 - 1);
                let ly = y.saturating_sub(self.4).min(self.2 - 1);
                let i = (ly * self.1 + lx) * 3;
                [self.0[i], self.0[i + 1], self.0[i + 2]]
            }
        }
        let view = RegionBase(&sub, rw, rh, rx0, ry0);
        let clip = Some((rx0, ry0, rw, rh));
        let cpu = pipeline::brush::rasterize_in(&b, &g, fw, fh, clip, Some(&view));
        let plan = pipeline::brush::plan_dabs(&b, &g, fw, fh, clip, Some(&view)).unwrap();
        assert!(
            plan.gating_rows.is_some(),
            "fixture check: the plan must gate, or the origin is never read"
        );
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(
                &plan,
                b.invert,
                Some(pipeline::brush::BaseRef {
                    plane: &sub,
                    w: rw,
                    h: rh,
                    x0: rx0,
                    y0: ry0,
                }),
            )
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }
```

- [ ] **Step 2: Run it, then sabotage the origin**

```bash
cargo test -p gpu-pipeline --release -- a_region_base_is_read 2>&1 | tail -12
```

Expected: pass. Then drop the offset in the shader (`let lx = min(fx, p.img_w - 1u);`) and expect
it to **fail** while the ten whole-frame tests stay green — that asymmetry is the finding.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w3c.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "test: a region's base is read from its own origin"
```

---

## Task 4: `accelerate` offers it

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Offer it in `walk`, beside `accelerate_inner`**

```rust
    let coverage: Arc<dyn pipeline::CoverageRasterizer> =
        Arc::new(crate::GpuBrushRaster::new(ctx));
    // ... in the loop, next to the existing hook:
        if let Some(f) = inner {
            s.accelerate_inner(f);
            // ⚠ Offered to every stage the walk sees, like the accelerator — a stage that builds
            // no coverage inherits the empty default, so one rule covers both arms.
            s.set_coverage_rasterizer(&coverage);
        }
```

⚠ Guarded by the same `inner.is_some()` as the accelerator, so the **recursive** call offers
nothing — a mask's sub-stages contain no mask and therefore no coverage, and offering into them
would be machinery for a case that cannot arise.

- [ ] **Step 2: Write the carrier test**

```rust
    /// ⚠⚠ **The carrier: `accelerate` really offers the rasteriser.** Everything else in this file
    /// would pass with the offer deleted — the mask would simply build its coverage on the CPU and
    /// render the same picture. `Pipeline::sub_stage_names_of` cannot see it either, because a
    /// coverage rasteriser is not a stage.
    #[test]
    fn accelerate_offers_a_coverage_rasterizer_to_a_mask() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![mask_carrying(edit_model::LocalAdjustments {
                exposure: 0.5,
                ..Default::default()
            })],
            ..Edits::default()
        };
        // ⚠ A brush mask, because only a painted mask rasterises coverage at all — an analytic one
        // answers from its own position and would make this test vacuous.
        let mut e = e;
        e.masks[0].mask = edit_model::Mask::Brush(edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: (0..10).map(|i| [0.3 + i as f32 * 0.03, 0.5]).collect(),
                radius: 0.05,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        });
        let accelerated = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert!(
            accelerated.mask_has_coverage_rasterizer(),
            "the mask must have been offered a coverage rasteriser"
        );
    }
```

⚠ That needs one observable. Add to `pipeline`, beside `sub_stage_names_of`:

```rust
    /// Whether the first `mask` stage holds a coverage rasteriser.
    ///
    /// ⚠ The observable for M8.3k's wiring, and it has to exist for the same reason
    /// `sub_stage_names_of` does: a coverage rasteriser changes no pixel a test can read, so
    /// without this the offer could be deleted and every test would stay green.
    pub fn mask_has_coverage_rasterizer(&self) -> bool {
        self.stages
            .iter()
            .any(|s| s.name() == "mask" && s.has_coverage_rasterizer())
    }
```

with a defaulted `fn has_coverage_rasterizer(&self) -> bool { false }` on `PipelineStage` and
`MaskStage` returning `self.coverage_gpu.is_some()`.

- [ ] **Step 3: Run, then sabotage the offer**

Delete the `s.set_coverage_rasterizer(&coverage);` line and confirm **only** that test fails.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w4c.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/ crates/pipeline/
git commit -m "feat: accelerate offers a mask its coverage rasterizer"
```

---

## Task 5: Measure it, and sweep the floor

**Files:**
- Modify: `crates/app/examples/bench_gpu_tail.rs`

- [ ] **Step 1: Add a global-drag row**

⚠⚠ **The existing `REAL SIDECAR` row cannot show this.** `best()` discards round 0 and renders the
same edits five times, so the auto-mask coverage cache hits and coverage is **free** — that is
exactly the caveat M8.3j's record carries. Coverage is only re-rasterised when a **global** edit
changes the base the gate reads, so the row must vary the edits between frames:

```rust
    // ⚠⚠ **A GLOBAL slider drag on the masked photograph — the one case brush coverage is
    // actually paid on.** `BrushCache` declines an auto-masked brush and the coverage cache's key
    // is invalidated by any global edit, so every frame of this re-rasterises. Rendering the same
    // edits repeatedly measures a cache hit and would report coverage as free.
    if let Some(e) = &real {
        let mut best_off = f64::MAX;
        let mut best_on = f64::MAX;
        for r in 0..6 {
            // ⚠ A different exposure each frame, which is what a drag is.
            let mut d = e.clone();
            d.exposure = 0.05 * (r as f32 + 1.0);
            let off = FrameSpec { gpu_tail: false, ..base_spec };
            let on = FrameSpec { gpu_tail: true, ..base_spec };
            let a = frame_ms(&engine, &d, off);
            let b = frame_ms(&engine, &d, on);
            if r > 0 {
                best_off = best_off.min(a);
                best_on = best_on.min(b);
            }
        }
        println!(
            "  {:<32} {best_off:7.2} ms {best_on:7.2} ms   {:.2}x  {:+6.2} ms",
            "REAL SIDECAR, global drag",
            best_off / best_on,
            best_off - best_on
        );
    }
```

- [ ] **Step 2: Run it three times**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
top -l 1 -n 0 | grep "CPU usage"
for i in 1 2 3; do cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6274.RAF 2>/dev/null | grep -E "rung 3200|REAL SIDECAR"; done
```

⚠ Quote a **band**, note how idle the machine was, and expect the plain `REAL SIDECAR` row to be
**unchanged** — it is the cache-hit control.

- [ ] **Step 3: Sweep `MIN_GPU_BOX_PX`**

```bash
for px in 4096 16384 65536 262144; do
  # edit the constant, rebuild, run the probe on a SMALL brush and on the real one
  :
done
```

Set it from the crossover: the smallest box at which the GPU is still faster. ⚠ Record the table in
the constant's doc comment, as `MAX_TILE_DABS` and `CHUNK_ROWS` both do. **A floor left at its
starting value is a floor nobody measured.**

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/bench_gpu_tail.rs crates/gpu-pipeline/
git commit -m "bench: a global drag on the masked photograph, and the box floor swept"
```

---

## Task 6: The gate, and the record

- [ ] **Step 1: The three gates, each exit code from its own command**

```bash
cargo fmt --all
cargo fmt --all --check > /tmp/w6-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w6-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/w6-test.log 2>&1; echo "TEST=$?"
echo "goldens:  $(grep -c '^test golden_' /tmp/w6-test.log)"
echo "GPU skips: $(grep -c 'SKIPPED: no GPU adapter' /tmp/w6-test.log)"
grep "test result: ok" /tmp/w6-test.log | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/' | awk '{s+=$1} END {print "tests:", s}'
```

Expected: all zero, **63** goldens **and none moved**, **0** skips, and the count arithmetic stated.

⚠⚠ **The goldens matter more here than in plan 1.** They render through `MaskStage` with
`coverage_gpu: None`, so 63 unmoved goldens are what establishes that the CPU path is byte-for-byte
unchanged — which is the guarantee the CLI and every export depend on.

- [ ] **Step 2: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

What to look at: **Settings → Rendering → "Use the GPU"** on, then `DSCF6274.RAF` and **drag a
global slider** — the case this milestone exists for. ⚠⚠ **The masked area must not move, grow,
harden at the brush edge, or change where the auto-mask stops.** A coverage bug renders a plausible
mask, so compare the mask's edge deliberately with the toggle on and off rather than looking for
something obviously wrong. ⚠ Then **zoom to 1:1 and pan**, which is the only way to exercise the
region origin Task 3 guards.

- [ ] **Step 3: Write the record**

Into `CLAUDE.md`, a `**M8.3k — the brush coverage rasteriser on the GPU**` section carrying: the
test-count arithmetic; the golden count and that none moved; the spec and both plan filenames; the
measured band; the capacity sweep and its cliff; the 229-dab occupancy measurement; the ordered
two-pass scan and why `atomicAdd` cannot do it; the region-origin gap plan 1 was blind to; every
sabotage including the two recorded inert; and the spec's §8 caveats plus the shared-upload
follow-up **with its ~12 ms**.

⚠ Also correct the RESUME block's next action, and note that three of Phase 8's listed candidates
measured **zero** (already recorded).

- [ ] **Step 4: Report, and stop**

⚠ **Do not merge and do not push.** The user merges (fast-forward, his call) and pushes himself.

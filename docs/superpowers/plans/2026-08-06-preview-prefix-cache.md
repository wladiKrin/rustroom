# Preview Prefix Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop re-computing the front of the develop pipeline on every slider move — cache one intermediate image after geometry and noise reduction, so an NR-on edit drops from ~50 ms to ~13 ms.

**Architecture:** `build_pipeline` is split into a *prefix* (geometry, noise reduction) and a *tail* (everything else), with `build_pipeline` redefined as `prefix ++ tail` so the two cannot drift. A `PrefixKey` — co-located with the split — captures exactly what the prefix depends on. The `RenderEngine` worker thread holds one cached prefix output keyed by it.

**Tech Stack:** Rust (Cargo workspace), `rayon`, `egui` 0.35, and the existing golden-image PNG regression harness.

**Design doc:** `docs/superpowers/specs/2026-08-06-preview-prefix-cache-design.md`

**Before you start.** Every `cargo` command needs libraw discoverable by pkg-config on macOS:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Commit messages in this repo carry **no `Co-Authored-By` trailer** — the repo owner's explicit preference. Do not add one.

**The invariant that matters most:** `build_pipeline`'s observable behaviour must not change at all. All **20 golden references must stay byte-identical** through every task. If one moves, something is wrong with the split, not with the goldens.

---

## File Structure

**Modified:**

| File | Change |
|---|---|
| `crates/pipeline/src/lib.rs` | `Pipeline::into_stages`; `build_pipeline_split`; `build_pipeline` redefined; `PrefixKey`. |
| `crates/app/src/render_engine.rs` | `rendered_long_edge`; extracted `pack`; the cache in the worker loop. |
| `crates/app/examples/bench_preview.rs` | A cached-path timing case. |
| `CLAUDE.md` | The split, the key, corrected `downscale_to` figure, measured cached timings. |

Tasks 1 → 2 → 3 → 4 → 5 in order; 2 depends on 1, and 4 depends on 2 and 3.

---

## Task 1: Split the pipeline

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/pipeline/src/lib.rs`:

```rust
    /// An `Edits` with every field non-neutral — so a stage that has drifted into the wrong
    /// half of the split shows up.
    ///
    /// Every field is listed explicitly rather than using `..Default::default()`, on purpose:
    /// adding a field to `Edits` then breaks this function's compilation, forcing whoever
    /// adds it to decide which half of the split it belongs to.
    fn all_edits() -> Edits {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            white_balance: edit_model::WhiteBalance {
                temp: 10.0,
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -30.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            version: 0,
        }
    }

    #[test]
    fn build_pipeline_is_exactly_prefix_then_tail() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
            let mut expected = prefix.stage_names();
            expected.extend(tail.stage_names());
            assert_eq!(
                build_pipeline(&edits, RenderScale::FULL).stage_names(),
                expected
            );
        }
    }

    #[test]
    fn the_prefix_holds_only_geometry_and_noise_reduction() {
        // The tripwire: if a stage is later inserted into the prefix region, this fails and
        // forces PrefixKey to be updated with its parameters.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert_eq!(prefix.stage_names(), vec!["geometry", "noise_reduction"]);
        for name in tail.stage_names() {
            assert!(
                name != "geometry" && name != "noise_reduction",
                "prefix stage {name} leaked into the tail"
            );
        }
        assert_eq!(tail.stage_names().last(), Some(&"output_transform_srgb"));
    }

    #[test]
    fn running_prefix_then_tail_matches_the_whole_pipeline() {
        // The numerical counterpart to the name-based test above: bit-identical, not close.
        let edits = all_edits();
        let mut img = ImageBuf::new(16, 12);
        for y in 0..12 {
            for x in 0..16 {
                img.set_pixel(x, y, [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
            }
        }
        let whole = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
        let split = tail.run(prefix.run(img));
        assert_eq!(whole.data(), split.data());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib prefix
```

Expected: FAIL to compile — `cannot find function build_pipeline_split in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/lib.rs`, add to `impl Pipeline` after `stage_names`:

```rust
    /// Consume the pipeline, yielding its stages — lets `build_pipeline` concatenate the two
    /// halves of `build_pipeline_split`.
    pub fn into_stages(self) -> Vec<Box<dyn PipelineStage>> {
        self.stages
    }
```

Then replace the whole of `build_pipeline` with the split plus a concatenating wrapper:

```rust
/// The develop pipeline in two halves, split where the preview cache checkpoints.
///
/// The **prefix** — geometry, then noise reduction — depends only on `edits.geometry` and
/// `edits.noise_reduction`, which is exactly what [`PrefixKey`] captures. Its output can
/// therefore be reused across every adjustment downstream, which is what makes editing with
/// noise reduction enabled affordable.
///
/// The **tail** is everything else and always ends in the output transform.
///
/// [`build_pipeline`] is defined as `prefix ++ tail`, so the two constructions cannot drift
/// apart. Add a stage here, not there.
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline) {
    let mut prefix: Vec<Box<dyn PipelineStage>> = Vec::new();

    // Geometry runs first, so all adjustments operate on the final cropped pixels.
    if edits.geometry != edit_model::Geometry::default() {
        prefix.push(Box::new(GeometryStage::new(&edits.geometry)));
    }
    // Noise reduction comes before every adjustment: denoising the source is far easier
    // than denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are
    // modifiers and must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        prefix.push(Box::new(NoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
        )));
    }

    let mut tail: Vec<Box<dyn PipelineStage>> = Vec::new();
    if edits.white_balance != edit_model::WhiteBalance::default() {
        tail.push(Box::new(WhiteBalanceStage::new(&edits.white_balance)));
    }
    if edits.exposure != 0.0 {
        tail.push(Box::new(ExposureStage::new(edits.exposure)));
    }
    if edits.contrast != 0.0 {
        tail.push(Box::new(ContrastStage::new(edits.contrast)));
    }
    if edits.highlights != 0.0 || edits.shadows != 0.0 || edits.whites != 0.0 || edits.blacks != 0.0
    {
        tail.push(Box::new(ToneRegionsStage::new(
            edits.highlights,
            edits.shadows,
            edits.whites,
            edits.blacks,
        )));
    }
    if !edits.tone_curve.points.is_empty() {
        tail.push(Box::new(ToneCurveStage::new(&edits.tone_curve.points)));
    }
    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        tail.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }
    if edits.hsl != edit_model::Hsl::default() {
        tail.push(Box::new(HslStage::new(&edits.hsl)));
    }
    // A creative effect over the finished grade, so it runs last. It is post-crop for free:
    // `GeometryStage` ran first, so the frame it sees is already cropped. It takes no
    // `RenderScale` — normalized coordinates make it resolution-independent.
    if edits.vignette.is_active() {
        tail.push(Box::new(VignetteStage::new(&edits.vignette)));
    }
    tail.push(Box::new(OutputTransformSrgb));

    (Pipeline::new(prefix), Pipeline::new(tail))
}

/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline {
    let (prefix, tail) = build_pipeline_split(edits, scale);
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    Pipeline::new(stages)
}
```

- [ ] **Step 4: Run the full suite**

```bash
cargo test --all
```

Expected: PASS, **including all 20 goldens byte-identical** — this task is a pure refactor of how the same stage list is assembled.

- [ ] **Step 5: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "refactor(pipeline): split build_pipeline into a cacheable prefix and a tail"
```

---

## Task 2: `PrefixKey`

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn prefix_key_ignores_every_downstream_field() {
        // Proves every edit field is either in the key or downstream of the checkpoint.
        // If a field is added to the prefix without being added to PrefixKey, the cache
        // would silently serve stale pixels; this is what stands in the way.
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL);

        let mut hsl = edit_model::Hsl::default();
        hsl.bands[3].hue = 40.0;
        let downstream = [
            Edits {
                white_balance: edit_model::WhiteBalance {
                    temp: 50.0,
                    tint: -20.0,
                },
                ..base.clone()
            },
            Edits {
                exposure: 2.0,
                ..base.clone()
            },
            Edits {
                contrast: 50.0,
                ..base.clone()
            },
            Edits {
                highlights: -50.0,
                ..base.clone()
            },
            Edits {
                shadows: 50.0,
                ..base.clone()
            },
            Edits {
                whites: 30.0,
                ..base.clone()
            },
            Edits {
                blacks: -30.0,
                ..base.clone()
            },
            Edits {
                tone_curve: edit_model::ToneCurve {
                    points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
                },
                ..base.clone()
            },
            Edits {
                saturation: 40.0,
                ..base.clone()
            },
            Edits {
                vibrance: 40.0,
                ..base.clone()
            },
            Edits { hsl, ..base.clone() },
            Edits {
                vignette: edit_model::Vignette {
                    amount: -70.0,
                    ..Default::default()
                },
                ..base.clone()
            },
        ];
        for e in downstream {
            assert_eq!(
                PrefixKey::new(&e, RenderScale::FULL),
                k,
                "a downstream field changed the prefix key"
            );
        }
    }

    #[test]
    fn prefix_key_changes_with_prefix_fields_and_scale() {
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL);

        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(PrefixKey::new(&geo, RenderScale::FULL), k);

        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(PrefixKey::new(&nr, RenderScale::FULL), k);

        // Scale changes the noise-reduction radii, so it belongs in the key.
        assert_ne!(PrefixKey::new(&base, RenderScale::new(0.25)), k);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib prefix_key
```

Expected: FAIL to compile — `cannot find type PrefixKey in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/lib.rs`, directly **above** `build_pipeline_split` so the two are read together:

```rust
/// Everything `build_pipeline_split`'s prefix depends on, for caching its output.
///
/// **If you add a stage to the prefix, add its parameters here.** The tests in this module
/// assert that every other edit field leaves this key unchanged, so they will fail until you
/// do — which is the point, because a prefix stage missing from the key means the preview
/// cache silently serves stale pixels.
///
/// Compared with `==` rather than hashed, so there is no collision risk. The `f32` is safe in
/// a `PartialEq`: the only pathological value is `NaN`, which compares unequal to itself and
/// so fails toward a cache *miss*.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefixKey {
    geometry: edit_model::Geometry,
    noise_reduction: edit_model::NoiseReduction,
    /// Included because it sets the noise-reduction radii.
    scale: f32,
}

impl PrefixKey {
    pub fn new(edits: &Edits, scale: RenderScale) -> Self {
        Self {
            geometry: edits.geometry,
            noise_reduction: edits.noise_reduction,
            scale: scale.get(),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): PrefixKey identifying what the cacheable prefix depends on"
```

---

## Task 3: Predict the rendered long edge

The cache needs the render scale on a **hit**, when there is no downscaled image to measure. `render_once` currently measures the downscaled image, which does not survive caching.

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/app/src/render_engine.rs`:

```rust
    #[test]
    fn the_predicted_long_edge_matches_downscale_to() {
        // The cache keys off this prediction, so it must agree with what downscale_to
        // actually produces — including the case where the image is already small enough
        // and downscale_to just clones.
        for (w, h, max_dim) in [(8, 8, 4), (10, 5, 4), (5, 10, 4), (4, 4, 16), (7, 3, 7)] {
            let img = ImageBuf::new(w, h);
            let actual = img.downscale_to(max_dim);
            let actual_long = actual.width().max(actual.height());
            assert_eq!(
                rendered_long_edge(w.max(h), max_dim),
                actual_long,
                "{w}x{h} -> max_dim {max_dim}"
            );
        }
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app rendered_long_edge
```

Expected: FAIL to compile — `cannot find function rendered_long_edge in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/render_engine.rs`, next to `render_scale`:

```rust
/// The long edge `downscale_to(max_dim)` will produce, computed without doing the downscale.
///
/// The preview cache needs the render scale on a cache *hit*, when there is no downscaled
/// image to measure. `downscale_to` never upscales and caps the long edge, so this is exact
/// rather than an estimate — asserted against it in the tests.
fn rendered_long_edge(buffer_long: usize, max_dim: usize) -> usize {
    if max_dim == 0 {
        buffer_long
    } else {
        buffer_long.min(max_dim)
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p app rendered_long_edge
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): predict the downscaled long edge without downscaling"
```

---

## Task 4: The cache

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/app/src/render_engine.rs`:

```rust
    /// A buffer with varied content, so a stale cache actually shows up as different bytes.
    /// (The flat gray `tiny()` would hide almost any staleness.)
    fn varied() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / 16.0,
                        y as f32 / 16.0,
                        ((x + y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        Arc::new(img)
    }

    #[test]
    fn every_cached_frame_matches_a_cold_render() {
        // The property that matters: caching must be invisible in the output. The sequence
        // walks a pure hit, a downstream-only change (hit), an NR change (miss), a return to
        // an earlier state (miss, since there is one entry) and a geometry change (miss).
        let buf = varied();
        let a = Edits {
            exposure: 0.5,
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // downstream only
        let c = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 10.0,
                color: 10.0,
                ..Default::default()
            },
            ..a.clone()
        }; // prefix change
        let d = Edits {
            geometry: edit_model::Geometry {
                angle: 4.0,
                ..Default::default()
            },
            ..a.clone()
        }; // prefix change

        // Guard against the whole test passing trivially: these must actually differ.
        let cold_a = render_once(&buf, &a, 8, 1.0);
        assert_ne!(cold_a.rgba, render_once(&buf, &b, 8, 1.0).rgba);
        assert_ne!(cold_a.rgba, render_once(&buf, &c, 8, 1.0).rgba);
        assert_ne!(cold_a.rgba, render_once(&buf, &d, 8, 1.0).rgba);

        let engine = RenderEngine::new(buf.clone(), 1.0, || {});
        for (i, e) in [&a, &a, &b, &c, &a, &d].iter().enumerate() {
            engine.request((*e).clone(), 8);
            let got = wait_for_frame(&engine);
            let cold = render_once(&buf, e, 8, 1.0);
            assert_eq!(got.rgba, cold.rgba, "frame {i} differs from a cold render");
        }
    }
```

Note the test waits for each frame before sending the next, so the engine's coalescing never drops a request.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app every_cached_frame
```

Expected: **PASS**, against the still-uncached engine. No new imports are needed — `edit_model::NoiseReduction` and `edit_model::Geometry` resolve through the extern crate, which is already an `app` dependency.

This one is deliberately not a red-then-green step. The cache is a pure performance change that must leave output identical, so there is no failing behaviour to drive it — the test is the **regression net** that Step 3 must not break. Watch for it passing *for the right reason*: the three `assert_ne!` guards at the top prove the four edit sets genuinely render differently, so the equality assertions cannot pass trivially.

- [ ] **Step 3: Add the cache**

In `crates/app/src/render_engine.rs`, first extract the RGBA packing so the cached path and `render_once` share it. Replace the body of `render_once` and add `pack`:

```rust
/// Pack a rendered sRGB image into an RGBA8 frame.
fn pack(out: ImageBuf) -> RenderedFrame {
    let (width, height) = (out.width(), out.height());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in out.data().chunks_exact(3) {
        rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push(255);
    }
    RenderedFrame {
        width,
        height,
        rgba,
    }
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the same
/// file; it and the downscale below together tell the pipeline's spatial stages how to scale
/// their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    pack(build_pipeline(edits, scale).run(preview))
}
```

Then add the cached path and its key type, above `render_once`:

```rust
/// The cached prefix output is valid for one set of prefix parameters at one working
/// resolution.
type CacheKey = (PrefixKey, usize);

/// Render one request, reusing the cached prefix output when the prefix parameters are
/// unchanged.
///
/// The prefix — geometry and noise reduction — is by far the expensive half once noise
/// reduction is on, and its parameters do not change when an adjustment slider moves. See
/// `docs/superpowers/specs/2026-08-06-preview-prefix-cache-design.md`.
fn render_cached(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    cache: &mut Option<(CacheKey, Arc<ImageBuf>)>,
) -> RenderedFrame {
    let buffer_long = buffer.width().max(buffer.height());
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );
    let key = (PrefixKey::new(edits, scale), max_dim);
    let (prefix, tail) = build_pipeline_split(edits, scale);

    let mid = match cache {
        Some((k, img)) if *k == key => img.clone(), // Arc clone: free
        _ => {
            let img = Arc::new(prefix.run(buffer.downscale_to(max_dim)));
            *cache = Some((key, img.clone()));
            img
        }
    };

    // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU
    // seam clean — so a cached image must be copied before the tail can run. Measured at
    // 0.6 ms against the 9 ms downscale it avoids.
    pack(tail.run((*mid).clone()))
}
```

Update the `use` line:

```rust
use pipeline::{build_pipeline, build_pipeline_split, PrefixKey, RenderScale};
```

Finally, give the worker loop its cache — replace the `render_once` call inside `RenderEngine::new`'s thread:

```rust
        let handle = thread::spawn(move || {
            let mut cache: Option<(CacheKey, Arc<ImageBuf>)> = None;
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                let frame =
                    render_cached(&buffer, &req.edits, req.max_dim, source_scale, &mut cache);
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();
            }
        });
```

- [ ] **Step 4: Run the full suite**

```bash
cargo test --all
```

Expected: PASS, including `every_cached_frame_matches_a_cold_render` and all 20 goldens.

- [ ] **Step 5: Check formatting and lints**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Expected: no output (success).

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "perf(app): cache the pipeline prefix so adjustments skip geometry and NR"
```

---

## Task 5: Measure and document

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add a cached-path case to the benchmark**

`render_once` stays uncached, so the example must time the cached path explicitly. Add to `crates/app/examples/bench_preview.rs`, after the existing `for (label, edits) in cases` loop:

```rust
    // The cached path: what a slider move actually costs once the prefix is warm. Timed
    // through the public split rather than through RenderEngine, so the number is the work
    // itself and not thread hand-off noise.
    {
        use pipeline::{build_pipeline_split, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale * PREVIEW_MAX_DIM as f32 / buffer_long as f32);
        let heavy = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            exposure: 0.5,
            ..Edits::default()
        };
        let (prefix, tail) = build_pipeline_split(&heavy, sc);
        let mid = prefix.run(buf.downscale_to(PREVIEW_MAX_DIM));

        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let out = tail.run(mid.clone());
            let mut rgba = Vec::with_capacity(out.width() * out.height() * 4);
            for px in out.data().chunks_exact(3) {
                rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push(255);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&rgba);
            best = best.min(ms);
        }
        println!("{best:8.1} ms   NR both 50 + exposure, CACHE HIT");
    }
```

- [ ] **Step 2: Measure**

```bash
cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF
```

Record the cache-hit line against the `NR both 50` line. Expect roughly 50 ms → ~13 ms. If the gap is much smaller than that, stop and find out why before writing it up.

- [ ] **Step 3: Update `CLAUDE.md`**

1. **Correct the stale figure.** The "Snappy RAW switching" section and/or the HSL perf notes cite `downscale_to (~14 ms)`. Measured now at **9.0 ms** — fix it and say it was re-measured.
2. **Crate table:** the `pipeline` row gains `build_pipeline_split` and `PrefixKey`.
3. **RESUME HERE block:** preview caching is done; the remaining named candidate is **sharpening**, plus the newly-obvious one of parallelizing the RGBA8 pack.
4. **A new status section** under the RESUME HERE block, in the style of the vignette one, recording:
   - The split: prefix = geometry + noise reduction, tail = everything else, `build_pipeline` redefined as `prefix ++ tail` **so the two cannot drift**.
   - `PrefixKey` co-located with the split, compared by `==` not hashed, and the **two staleness guards** (prefix composition test, key-completeness test over every downstream field). Say plainly what they protect against: a stage added to the prefix without being added to the key would make the cache serve stale pixels silently.
   - The cache lives in the `RenderEngine` worker thread — sole renderer, no locking, one entry, per-session because a new photo builds a new engine.
   - **The scale must be predicted from `max_dim`**, not measured from the downscaled image, because on a hit there is no downscaled image; `rendered_long_edge` is asserted against `downscale_to`.
   - The 0.6 ms deep clone per frame: `PipelineStage::apply` consumes its input, so a cached image must be copied. Kept deliberately — 15× cheaper than what it saves — rather than reshaping the trait.
   - **The measured decomposition** of a neutral render (downscale 9.0 / pipeline 7.5 / RGBA pack 4.4) and the before/after cached numbers from Step 2.
   - Caveats: cold on open; Straighten still full cost (geometry is upstream) while **crop dragging hits the cache**, because `preview_edits` resets the crop in crop mode; one entry so alternating between two heavy NR settings misses every time; and the now-relatively-larger RGBA pack being the obvious next target.
5. **Docs list:** add `specs/2026-08-06-preview-prefix-cache-design.md` and `plans/2026-08-06-preview-prefix-cache.md`.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all
```

Expected: all green.

- [ ] **Step 5: Confirm no golden moved**

```bash
git status --short crates/pipeline/tests/golden
```

Expected: **empty**. `build_pipeline`'s behaviour is unchanged by this whole feature, so any moved reference is a bug in the split.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md crates/app/examples/bench_preview.rs
git commit -m "docs: record the preview prefix cache and its measured effect"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` all green.
- **All 20 golden references byte-identical** — `build_pipeline` must be unchanged in behaviour.
- `every_cached_frame_matches_a_cold_render` passes, proving the cache is invisible in the output.
- `CLAUDE.md` records the split, the key and its guards, the corrected 9.0 ms figure, and the measured cached timings.

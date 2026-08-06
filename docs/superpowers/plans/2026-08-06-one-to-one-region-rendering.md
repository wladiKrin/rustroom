# 1:1 Region Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make zoom re-render instead of magnifying. Past a threshold, render the visible region at the editing buffer's native resolution, so detail-level edits can be judged in the app.

**Architecture:** The preview cache already runs the pipeline's *prefix* (geometry + noise reduction) whole-buffer and caches it. The region crop goes **between prefix and tail**: no inverse geometry is needed (the prefix output is what the viewport shows), and NR having run whole-buffer means no region boundary can produce a seam. `Region` is added to the pipeline as an **optional** parameter so no existing call site changes.

**Tech Stack:** Rust (Cargo workspace), `rayon`, `egui` 0.35, and the existing golden-image PNG regression harness.

**Design doc:** `docs/superpowers/specs/2026-08-06-one-to-one-region-rendering-design.md`

**Before you start.** Every `cargo` command needs libraw discoverable by pkg-config on macOS:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Commit messages carry **no `Co-Authored-By` trailer** — the repo owner's explicit preference.

**Invariants that hold through every task:**

- **All 20 golden references stay byte-identical.** Nothing here changes an existing render path; Fit mode is exactly today's behaviour with `region = None`.
- **`build_pipeline` and `build_pipeline_split` keep their current signatures.** The region is added on new functions that the old ones delegate into.

---

## File Structure

**Created:**

| File | Responsibility |
|---|---|
| `crates/app/src/view_region.rs` | The viewport→image-rect maths, pure and directly tested. This is where the bugs would otherwise hide. |

**Modified:**

| File | Change |
|---|---|
| `crates/pipeline/src/lib.rs` | `Region`; `build_pipeline_split_in_region`. |
| `crates/pipeline/src/stages/vignette.rs` | `new_in_region`; frame coords from the region when present. |
| `crates/app/src/render_engine.rs` | `RegionRect`; region crop between prefix and tail; `RenderedFrame.region`; two-entry cache. |
| `crates/app/src/lib.rs` | Declare `view_region`. |
| `crates/app/src/main.rs` | Mode selection, region requests, texture placement, crop overlay. |
| `crates/app/examples/bench_preview.rs` | A region-tail timing case. |
| `CLAUDE.md` | The two modes, optional `Region`, two-entry cache, demosaic measurements. |

Order: 1 → 2 → 3 → (4 in parallel) → 5 → 6. Task 5 depends on 2, 3 and 4.

---

## Task 1: `Region` and a region-aware vignette

**Files:**
- Modify: `crates/pipeline/src/stages/vignette.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/pipeline/src/stages/vignette.rs`:

```rust
    /// A frame with varied content, so a mis-placed vignette shows up as different bytes.
    fn varied(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        0.2 + x as f32 / (w as f32 * 2.0),
                        0.5,
                        0.8 - y as f32 / (h as f32 * 2.0),
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn a_region_render_matches_the_whole_frame_cropped() {
        // The whole point of Region: given a crop, the falloff must still be measured
        // against the full frame. Without it the vignette would re-centre on the crop.
        let full = varied(40, 24);
        let params = Vignette {
            amount: -70.0,
            midpoint: 30.0,
            roundness: 60.0,
            feather: 40.0,
        };
        let expect = VignetteStage::new(&params).apply(full.clone()).crop(11, 7, 13, 9);

        let region = Region {
            full_w: 40,
            full_h: 24,
            x0: 11,
            y0: 7,
        };
        let got = VignetteStage::new_in_region(&params, Some(region))
            .apply(full.crop(11, 7, 13, 9));

        assert_eq!(got.data(), expect.data());
    }

    #[test]
    fn a_region_covering_the_whole_frame_is_the_same_as_no_region() {
        let full = varied(20, 12);
        let params = Vignette {
            amount: -50.0,
            roundness: -40.0,
            ..Default::default()
        };
        let none = VignetteStage::new(&params).apply(full.clone());
        let whole = VignetteStage::new_in_region(
            &params,
            Some(Region {
                full_w: 20,
                full_h: 12,
                x0: 0,
                y0: 0,
            }),
        )
        .apply(full);
        assert_eq!(none.data(), whole.data());
    }

    #[test]
    fn the_region_sets_the_aspect_not_the_crop() {
        // A tall crop out of a wide frame must still use the *frame's* aspect for
        // roundness — this is the part that silently looks fine but is wrong.
        let full = varied(64, 16);
        let params = Vignette {
            amount: -80.0,
            roundness: 100.0,
            ..Default::default()
        };
        let expect = VignetteStage::new(&params).apply(full.clone()).crop(20, 2, 8, 12);
        let got = VignetteStage::new_in_region(
            &params,
            Some(Region {
                full_w: 64,
                full_h: 16,
                x0: 20,
                y0: 2,
            }),
        )
        .apply(full.crop(20, 2, 8, 12));
        assert_eq!(got.data(), expect.data());
    }
```

And to the `mod tests` block in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn a_none_region_split_is_identical_to_the_plain_split() {
        for edits in [Edits::default(), all_edits()] {
            let (p1, t1) = build_pipeline_split(&edits, RenderScale::FULL);
            let (p2, t2) = build_pipeline_split_in_region(&edits, RenderScale::FULL, None);
            assert_eq!(p1.stage_names(), p2.stage_names());
            assert_eq!(t1.stage_names(), t2.stage_names());
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline region
```

Expected: FAIL to compile — `cannot find type Region`, `no function new_in_region`.

- [ ] **Step 3: Add `Region` and the region-aware builder**

In `crates/pipeline/src/lib.rs`, directly above `PrefixKey`:

```rust
/// Where a cropped render sits inside the full frame.
///
/// `None` at a call site means "this buffer *is* the full frame" — what every existing caller
/// implicitly passes, and what position-dependent stages already assume. Only stages whose
/// output depends on absolute position need it; today that is the vignette alone, but any
/// future graduated or radial filter will want the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub full_w: usize,
    pub full_h: usize,
    pub x0: usize,
    pub y0: usize,
}
```

Rename the existing `build_pipeline_split` body to take a region, and make the old name delegate. Replace the function signature and the vignette push:

```rust
pub fn build_pipeline_split_in_region(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
) -> (Pipeline, Pipeline) {
```

…and inside it, the vignette line becomes:

```rust
    if edits.vignette.is_active() {
        tail.push(Box::new(VignetteStage::new_in_region(&edits.vignette, region)));
    }
```

Then add, immediately after it:

```rust
/// The develop pipeline in two halves, for a render covering the whole frame.
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region(edits, scale, None)
}
```

`build_pipeline` is unchanged — it already calls `build_pipeline_split`.

- [ ] **Step 4: Make the vignette region-aware**

In `crates/pipeline/src/stages/vignette.rs`, change the struct, constructors and the coordinate maths:

```rust
pub struct VignetteStage {
    params: Vignette,
    region: Option<Region>,
}

impl VignetteStage {
    /// For a render covering the whole frame.
    pub fn new(params: &Vignette) -> Self {
        Self::new_in_region(params, None)
    }

    /// For a render covering only part of the frame — the falloff is still measured against
    /// the full frame, so a cropped region is shaded exactly as that part of the whole would
    /// have been.
    pub fn new_in_region(params: &Vignette, region: Option<Region>) -> Self {
        Self {
            params: *params,
            region,
        }
    }
}
```

Add the import: `use crate::{PipelineStage, Region};`

In `apply`, replace the shape/coordinate setup. The frame dimensions drive **both** the aspect and the normalization, which is why `Shape::new` takes them too:

```rust
        // The frame the falloff is measured against, and where this buffer sits in it.
        // Without a region the buffer *is* the frame — the existing behaviour, exactly.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };

        let shape = Shape::new(p.roundness, fw, fh);
        let r_mid = lerp(R_MID.0, R_MID.1, (p.midpoint / 100.0).clamp(0.0, 1.0));
        let width = lerp(FEATHER.0, FEATHER.1, (p.feather / 100.0).clamp(0.0, 1.0)).max(WIDTH_EPS);
        let (e0, e1) = (r_mid - width * 0.5, r_mid + width * 0.5);
        let k = (p.amount / 100.0).clamp(-1.0, 1.0);
        let (hw, hh) = (fw as f32 * 0.5, fh as f32 * 0.5);

        image.map_pixels_xy(|x, y, rgb| {
            let u = ((x + x0) as f32 + 0.5) / hw - 1.0;
            let v = ((y + y0) as f32 + 0.5) / hh - 1.0;
            let f = k * smoothstep(e0, e1, shape.distance(u, v)); // -1 … 1
```

The rest of the closure is unchanged.

- [ ] **Step 5: Run the full suite**

```bash
cargo test --all
```

Expected: PASS, **including all 20 goldens byte-identical** — the `None` path is the old code.

- [ ] **Step 6: Check formatting and lints, then commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

```bash
git add crates/pipeline/src
git commit -m "feat(pipeline): optional Region so a cropped render shades like the whole frame"
```

---

## Task 2: Render a region

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/app/src/render_engine.rs`:

```rust
    /// The RGBA sub-rect of a frame, for comparing a region render against a whole one.
    fn crop_rgba(f: &RenderedFrame, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * f.width + x) * 4;
            out.extend_from_slice(&f.rgba[start..start + w * 4]);
        }
        out
    }

    #[test]
    fn a_region_render_matches_the_whole_render_cropped() {
        // The master invariant. It validates the region maths, the vignette correction and
        // the cache at once — if any of the three is wrong, this fails.
        let buf = varied();
        let edits = Edits {
            exposure: 0.4,
            vignette: edit_model::Vignette {
                amount: -80.0,
                roundness: 50.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                color: 40.0,
                ..Default::default()
            },
            ..Edits::default()
        };

        // max_dim = 16 is native for this 16x16 buffer, i.e. no downscale.
        let whole = render_cached(&buf, &edits, 16, 1.0, None, &mut PrefixCache::default());
        let (x, y, w, h) = (3, 2, 7, 6);
        assert!(x + w <= whole.width && y + h <= whole.height, "region must fit");

        let got = render_cached(
            &buf,
            &edits,
            16,
            1.0,
            Some(RegionRect { x, y, w, h }),
            &mut PrefixCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        assert_eq!(got.rgba, crop_rgba(&whole, x, y, w, h));
        assert_eq!(
            got.region,
            Some(pipeline::Region {
                full_w: whole.width,
                full_h: whole.height,
                x0: x,
                y0: y
            })
        );
    }

    #[test]
    fn a_region_bigger_than_the_image_is_clamped() {
        let buf = varied();
        let e = Edits::default();
        let got = render_cached(
            &buf,
            &e,
            16,
            1.0,
            Some(RegionRect {
                x: 10,
                y: 10,
                w: 999,
                h: 999,
            }),
            &mut PrefixCache::default(),
        );
        assert_eq!((got.width, got.height), (6, 6));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app a_region_render
```

Expected: FAIL to compile — `RegionRect` and `PrefixCache` do not exist and `render_cached` takes five arguments.

- [ ] **Step 3: Add `RegionRect`, the frame's region, and the crop**

In `crates/app/src/render_engine.rs`, update the import and `RenderedFrame`:

```rust
use pipeline::{build_pipeline, build_pipeline_split_in_region, PrefixKey, Region, RenderScale};
```

```rust
/// A finished preview frame (display sRGB, RGBA8).
pub struct RenderedFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Where this frame sits in the post-geometry image; `None` = the whole image. The canvas
    /// needs it to place the texture.
    pub region: Option<Region>,
}

/// A requested crop of the rendered image, in post-geometry pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl RegionRect {
    /// Clamp to an image of `w` x `h`, returning `None` if nothing is left.
    fn clamped(self, w: usize, h: usize) -> Option<Self> {
        let x = self.x.min(w.saturating_sub(1));
        let y = self.y.min(h.saturating_sub(1));
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self {
            x,
            y,
            w: cw,
            h: ch,
        })
    }
}
```

Update `pack` to carry the region, and both its callers:

```rust
fn pack(out: ImageBuf, region: Option<Region>) -> RenderedFrame {
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
        region,
    }
}
```

In `render_once`, the final line becomes `pack(build_pipeline(edits, scale).run(preview), None)`.

Then replace `render_cached`:

```rust
/// Render one request, reusing the cached prefix output when the prefix parameters are
/// unchanged, and optionally cropping to `region` before the tail.
///
/// The crop sits between prefix and tail deliberately: the prefix output is the
/// post-geometry image, so no inverse transform is needed, and noise reduction has already
/// run over the whole buffer, so no region boundary can produce a seam however the user pans.
/// See `docs/superpowers/specs/2026-08-06-one-to-one-region-rendering-design.md`.
fn render_cached(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    region: Option<RegionRect>,
    cache: &mut PrefixCache,
) -> RenderedFrame {
    let buffer_long = buffer.width().max(buffer.height());
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );
    let key = (PrefixKey::new(edits, scale), max_dim);

    // The prefix never depends on the region, so it is built and cached without one.
    let mid = match cache.get(&key) {
        Some(img) => img,
        None => {
            let (prefix, _) = build_pipeline_split_in_region(edits, scale, None);
            let img = Arc::new(prefix.run(buffer.downscale_to(max_dim)));
            cache.insert(key, img.clone());
            img
        }
    };

    // The region can only be resolved once the prefix output's dimensions are known, which
    // is why the split is built a second time here. Building is a few Box allocations.
    let (pw, ph) = (mid.width(), mid.height());
    let rect = region.and_then(|r| r.clamped(pw, ph));
    let (input, region) = match rect {
        Some(r) => (
            mid.crop(r.x, r.y, r.w, r.h),
            Some(Region {
                full_w: pw,
                full_h: ph,
                x0: r.x,
                y0: r.y,
            }),
        ),
        // `PipelineStage::apply` consumes its input, so a cached image must be copied.
        // Measured at 0.6 ms against the 9 ms downscale it avoids.
        None => ((*mid).clone(), None),
    };

    let (_, tail) = build_pipeline_split_in_region(edits, scale, region);
    pack(tail.run(input), region)
}
```

- [ ] **Step 4: Add a placeholder `PrefixCache`**

Task 3 makes this hold two entries; for now it is the single entry the worker already had, wrapped so the signature is stable:

```rust
/// Cached prefix outputs, keyed by prefix parameters and working resolution.
#[derive(Default)]
struct PrefixCache {
    entries: Vec<(CacheKey, Arc<ImageBuf>)>,
}

impl PrefixCache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<ImageBuf>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries[i].1.clone())
    }

    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>) {
        self.entries.insert(0, (key, img));
        self.entries.truncate(1);
    }
}
```

Update the worker loop in `RenderEngine::new`: `let mut cache = PrefixCache::default();`, and pass `req.region` (added next) plus `&mut cache` to `render_cached`. Add `region: Option<RegionRect>` to `Request` and to `RenderEngine::request`:

```rust
    /// Queue a render at the given working resolution, optionally cropped to `region`.
    pub fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        let _ = self.req_tx.send(Request {
            edits,
            max_dim,
            region,
        });
    }
```

Fix the two existing `RenderEngine::request` calls in the tests by passing `None`, and the five in `main.rs` likewise for now — Task 5 replaces them.

- [ ] **Step 5: Run the full suite**

```bash
cargo test --all
```

Expected: PASS, including the master invariant and all 20 goldens.

If `a_region_render_matches_the_whole_render_cropped` fails, the likely cause is the vignette: check that `Shape::new` receives the **frame** dimensions and not the crop's. Do not weaken the assertion — bit-identical is the property.

- [ ] **Step 6: Check formatting and lints, then commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

```bash
git add crates/app/src/render_engine.rs crates/app/src/main.rs
git commit -m "feat(app): render a cropped region between the pipeline prefix and tail"
```

---

## Task 3: Two cache entries

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn the_cache_keeps_one_entry_per_render_mode() {
        // Fit and native are different max_dims. With a single entry, every zoom in or out
        // would recompute the prefix (~123 ms with NR on).
        let buf = varied();
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let mut cache = PrefixCache::default();

        render_cached(&buf, &e, 8, 1.0, None, &mut cache); // "fit"
        render_cached(&buf, &e, 16, 1.0, None, &mut cache); // "native"
        assert_eq!(cache.len(), 2, "both modes should stay cached");

        let scale8 = render_scale(16, rendered_long_edge(16, 8), 1.0);
        let scale16 = render_scale(16, rendered_long_edge(16, 16), 1.0);
        assert!(cache.get(&(PrefixKey::new(&e, scale8), 8)).is_some());
        assert!(cache.get(&(PrefixKey::new(&e, scale16), 16)).is_some());

        // A third distinct key evicts the least recently used.
        let e2 = Edits {
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..e.clone()
        };
        render_cached(&buf, &e2, 16, 1.0, None, &mut cache);
        assert_eq!(cache.len(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app the_cache_keeps_one_entry
```

Expected: FAIL — `cache.len()` does not exist, and once added, the assertion of 2 fails against `truncate(1)`.

- [ ] **Step 3: Widen the cache to two entries**

```rust
/// Cached prefix outputs, keyed by prefix parameters and working resolution.
///
/// Two entries, so the fit and native render modes can both stay warm — otherwise every
/// zoom in or out would recompute the prefix. Most-recently-used first.
#[derive(Default)]
struct PrefixCache {
    entries: Vec<(CacheKey, Arc<ImageBuf>)>,
}

impl PrefixCache {
    /// One entry per render mode.
    const CAP: usize = 2;

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&mut self, key: &CacheKey) -> Option<Arc<ImageBuf>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let hit = self.entries.remove(i);
        let img = hit.1.clone();
        self.entries.insert(0, hit);
        Some(img)
    }

    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, img));
        self.entries.truncate(Self::CAP);
    }
}
```

- [ ] **Step 4: Run the full suite, lint, commit**

```bash
cargo test --all && cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

```bash
git add crates/app/src/render_engine.rs
git commit -m "perf(app): keep one cached prefix per render mode so zooming stays instant"
```

---

## Task 4: The viewport maths

Pure functions in their own module, because this is where off-by-one and clamping bugs live and they cannot be found by looking at the GUI.

**Files:**
- Create: `crates/app/src/view_region.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/view_region.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_visible_image_needs_all_of_it() {
        // Viewport larger than the image at 1:1 — the whole image is visible.
        let v = visible_rect(100, 60, 400.0, 300.0, 1.0, 0.0, 0.0);
        assert_eq!(v, RegionRect { x: 0, y: 0, w: 100, h: 60 });
    }

    #[test]
    fn zooming_in_needs_only_the_centre() {
        // 100x60 image at 4x in a 200x120 viewport shows 50x30 image pixels, centred.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 0.0, 0.0);
        assert_eq!(v, RegionRect { x: 25, y: 15, w: 50, h: 30 });
    }

    #[test]
    fn panning_moves_the_visible_rect_and_clamps_at_the_edges() {
        // Panning the image right by 100 screen px at 4x moves the view 25 image px left.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 100.0, 0.0);
        assert_eq!(v, RegionRect { x: 0, y: 15, w: 50, h: 30 });
        // Far past the edge still yields a valid in-bounds rect.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, -10_000.0, 0.0);
        assert_eq!(v.x + v.w, 100);
    }

    #[test]
    fn the_margin_expands_and_clips_at_the_image_edge() {
        let r = with_margin(
            RegionRect {
                x: 40,
                y: 30,
                w: 20,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 32,
                y: 22,
                w: 36,
                h: 26
            }
        );
        // At a corner the margin is clipped, not wrapped or negative.
        let r = with_margin(
            RegionRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 0,
                y: 0,
                w: 18,
                h: 18
            }
        );
    }

    #[test]
    fn containment_decides_whether_to_re_render() {
        let rendered = RegionRect {
            x: 10,
            y: 10,
            w: 50,
            h: 50,
        };
        assert!(contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(contains(rendered, rendered));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 5,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 100,
                h: 10
            }
        ));
    }

    #[test]
    fn native_mode_engages_once_the_texture_is_magnified() {
        assert!(!needs_native(0.5, 1.0));
        assert!(!needs_native(0.5, 2.0)); // fit*zoom == 1.0 exactly: still fit
        assert!(needs_native(0.5, 2.1));
        assert!(needs_native(1.0, 1.5));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Declare the module first — in `crates/app/src/lib.rs`, add `pub mod view_region;` in alphabetical order.

```bash
cargo test -p app view_region
```

Expected: FAIL to compile — none of the four functions exist.

- [ ] **Step 3: Write the implementation**

Insert at the top of `crates/app/src/view_region.rs`:

```rust
//! Pure viewport maths for region rendering: which part of the image the canvas needs, and
//! whether what is already rendered still covers it.
//!
//! Deliberately free of `egui` types so it can be tested directly — this is where clamping
//! and off-by-one mistakes would otherwise only show up as a visibly wrong image.

pub use crate::render_engine::RegionRect;

/// Whether the view is magnifying the fit-resolution texture past its own pixels, in which
/// case the render should switch to the buffer's native resolution.
///
/// `fit` is the scale that makes the whole image fit the viewport; `zoom` is the user's
/// factor on top of it. At exactly 1.0 the texture is pixel-for-pixel, which fit still
/// covers.
pub fn needs_native(fit: f32, zoom: f32) -> bool {
    fit * zoom > 1.0
}

/// The sub-rect of an `image_w` x `image_h` image visible in a viewport of `avail_w` x
/// `avail_h` screen pixels, drawn at `scale` screen pixels per image pixel and panned by
/// `pan_x`/`pan_y` screen pixels.
///
/// Always returns an in-bounds, non-empty rect: panning far past an edge clamps rather than
/// producing an empty or negative region.
pub fn visible_rect(
    image_w: usize,
    image_h: usize,
    avail_w: f32,
    avail_h: f32,
    scale: f32,
    pan_x: f32,
    pan_y: f32,
) -> RegionRect {
    let axis = |image: usize, avail: f32, pan: f32| -> (usize, usize) {
        let image_f = image as f32;
        // Screen-space span of the image, centred in the viewport then panned.
        let drawn = image_f * scale;
        let left = (avail - drawn) * 0.5 + pan;
        // Image-space coordinate at each viewport edge.
        let lo = (-left / scale).floor().max(0.0);
        let hi = ((avail - left) / scale).ceil().min(image_f);
        let lo = lo.min(image_f - 1.0) as usize;
        let hi = (hi as usize).max(lo + 1).min(image);
        (lo, hi - lo)
    };
    let (x, w) = axis(image_w, avail_w, pan_x);
    let (y, h) = axis(image_h, avail_h, pan_y);
    RegionRect { x, y, w, h }
}

/// Expand by `margin` on every side, clipped to the image. Rendering a little more than is
/// visible means small pans reuse the existing texture instead of re-rendering.
pub fn with_margin(r: RegionRect, image_w: usize, image_h: usize, margin: usize) -> RegionRect {
    let x = r.x.saturating_sub(margin);
    let y = r.y.saturating_sub(margin);
    let right = (r.x + r.w + margin).min(image_w);
    let bottom = (r.y + r.h + margin).min(image_h);
    RegionRect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    }
}

/// Whether `inner` lies entirely within `outer` — the test for "the rendered region still
/// covers what is on screen, so nothing needs re-rendering".
pub fn contains(outer: RegionRect, inner: RegionRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}
```

`RegionRect` must be `pub` in `render_engine` for the re-export; it already is.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app view_region
```

Expected: PASS — 6 tests.

If `zooming_in_needs_only_the_centre` is off by one, check the `floor`/`ceil` pair: the visible rect must *cover* the viewport, so the low edge floors and the high edge ceils. Do not adjust the expected values to match the code.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

```bash
git add crates/app/src/view_region.rs crates/app/src/lib.rs
git commit -m "feat(app): pure viewport maths for region rendering"
```

---

## Task 5: Wire it into the canvas

The part that cannot be unit-tested — verified by running the app.

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Track the render mode and rendered region on the session**

Add to `struct Session`:

```rust
    /// The region the current texture covers, as a rect, so the canvas can both place it and
    /// test containment without consulting texture dimensions. `None` = whole image.
    rendered_region: Option<RegionRect>,
    /// The region last requested, so the same one is not re-requested every frame.
    requested_region: Option<RegionRect>,
    /// Post-geometry image size at fit resolution, for the fit/zoom maths.
    fit_image_size: egui::Vec2,
    /// The editing buffer's long edge — `max_dim` for native mode.
    buffer_long: usize,
```

**Store a `RegionRect`, not the frame's `pipeline::Region`.** Containment needs width and height, and reading those from the texture breaks when `show_before` swaps in the whole-image baseline texture.

Initialise in `finalize_session`, where the decoded buffer is in hand:

```rust
            rendered_region: None,
            requested_region: None,
            // Corrected by the first frame that arrives; the placeholder only has to be
            // non-zero so the first fit calculation cannot divide by zero.
            fit_image_size: egui::vec2(1.0, 1.0),
            buffer_long: decoded.buffer.width().max(decoded.buffer.height()),
```

Add near the other constants:

```rust
/// Rendered beyond the visible rect so small pans reuse the existing texture.
const REGION_MARGIN: usize = 128;
```

- [ ] **Step 2: Store the region when a frame arrives**

In the block that pulls a finished frame into a texture (around line 785, ending in `s.texture = Some(ctx.load_texture("preview", img, …))`), record where the frame belongs and keep the fit-resolution size current. The frame binding there is `f`:

```rust
                s.rendered_region = f.region.map(|r| RegionRect {
                    x: r.x0,
                    y: r.y0,
                    w: f.width,
                    h: f.height,
                });
                // A whole-image frame is by definition a fit-resolution render, and is the
                // only thing that can tell us the post-geometry size — which changes with
                // the crop and the straighten angle.
                if f.region.is_none() {
                    s.fit_image_size = egui::vec2(f.width as f32, f.height as f32);
                }
```

- [ ] **Step 3: Drive mode and region from the canvas**

Replace the body of the `if let Some(tex) = &s.texture {` block in the central panel. The existing code computes `fit`, `draw` and `img_rect`; it now also decides the mode, computes the needed region, requests a render when the rendered one no longer covers it, and draws the texture at the region's own screen rect:

```rust
                    if let Some(tex) = &s.texture {
                        let shown = if s.show_before { &s.baseline } else { tex };
                        let avail = ui.available_size();

                        // The post-geometry image's size at the *fit* resolution, which is
                        // what fit/zoom have always been expressed against.
                        let full = s.fit_image_size;
                        let fit = (avail.x / full.x).min(avail.y / full.y).min(1.0);
                        let native = view_region::needs_native(fit, s.zoom);
                        let max_dim = if native {
                            s.buffer_long
                        } else {
                            PREVIEW_MAX_DIM
                        };

                        // Screen pixels per *rendered* image pixel.
                        let render_long = if native {
                            s.buffer_long as f32
                        } else {
                            PREVIEW_MAX_DIM as f32
                        };
                        let scale = fit * s.zoom * full.x.max(full.y) / render_long;
                        let (iw, ih) = (
                            (full.x / full.x.max(full.y) * render_long).round() as usize,
                            (full.y / full.x.max(full.y) * render_long).round() as usize,
                        );

                        let want = if native {
                            let vis = view_region::visible_rect(
                                iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y,
                            );
                            Some(view_region::with_margin(vis, iw, ih, REGION_MARGIN))
                        } else {
                            None
                        };

                        // Re-render only when what is on screen escapes what is rendered.
                        let covered = match (want, s.rendered_region) {
                            (None, None) => true,
                            (Some(w), Some(r)) => view_region::contains(r, w),
                            _ => false,
                        };
                        if !covered && s.requested_region != want {
                            s.requested_region = want;
                            s.engine.request(
                                preview_edits(&s.edits, s.crop_mode),
                                max_dim,
                                want,
                            );
                        }

                        // Place the texture: the whole image, or the region's own sub-rect.
                        let origin = ui.max_rect().center() + s.pan
                            - egui::vec2(iw as f32, ih as f32) * scale * 0.5;
                        let img_rect = match s.rendered_region {
                            Some(r) => egui::Rect::from_min_size(
                                origin + egui::vec2(r.x as f32, r.y as f32) * scale,
                                shown.size_vec2() * scale,
                            ),
                            None => egui::Rect::from_center_size(
                                ui.max_rect().center() + s.pan,
                                shown.size_vec2() * fit * s.zoom,
                            ),
                        };
                        let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                        egui::Image::new(shown).paint_at(ui, resp.rect);
```

The rest of the block — crop overlay, drag-to-pan, scroll-to-zoom — is unchanged.

All four fields this reads were added in Step 1.

**One consequence worth expecting:** `show_before` renders the whole-image baseline at preview resolution, so toggling it while zoomed shows a softer image placed by the `None` arm. That is caveat 4 in the design, not a bug to chase here.

- [ ] **Step 4: Pass `None` at the other request sites**

The five `engine.request(...)` calls outside the canvas (slider changes, undo/redo, reset) should re-request the **current** region so a slider move while zoomed re-renders what is on screen:

```rust
                    .request(
                        preview_edits(&s.edits, s.crop_mode),
                        if s.requested_region.is_some() { s.buffer_long } else { PREVIEW_MAX_DIM },
                        s.requested_region,
                    );
```

- [ ] **Step 5: Verify it compiles and lints**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Expected: no output. Expect to iterate here — this step is the one most likely to need adjustment against the real `egui` API and the actual field names in `Session`.

- [ ] **Step 6: Run the app and check by hand**

```bash
cargo run --release -p app
```

Check each of these:

1. **Zoom in past fit** — detail visibly increases rather than the image becoming blockier. This is the whole feature.
2. **Pan while zoomed** — smooth, no flicker, no black gaps at the edges of the rendered region.
3. **Pan far** — the image stays correctly placed at every edge and corner.
4. **Move a slider while zoomed** — the zoomed view updates, and stays zoomed.
5. **Turn NR up, then move Exposure while zoomed** — still responsive (the prefix is cached).
6. **Zoom out to fit and back in** — instant both ways, no recompute pause (two cache entries).
7. **A strong vignette, then zoom into a corner** — the corner is dark. If the vignette re-centres on the region, `Region` is not reaching the stage.
8. **Crop mode while zoomed** — the overlay lines up with the image.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): render the visible region at native resolution when zoomed in"
```

---

## Task 6: Measure and document

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add a region-tail case to the benchmark**

After the existing cache-hit block in `crates/app/examples/bench_preview.rs`:

```rust
    // A region tail render: what a slider move costs while zoomed in, once the native
    // prefix is warm.
    {
        use pipeline::{build_pipeline_split_in_region, Region, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale);
        let heavy = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            exposure: 0.5,
            ..Edits::default()
        };
        let t0 = Instant::now();
        let (prefix, _) = build_pipeline_split_in_region(&heavy, sc, None);
        let mid = prefix.run(buf.downscale_to(buffer_long));
        println!(
            "{:8.1} ms   native prefix, ONE-OFF ({}x{})",
            t0.elapsed().as_secs_f64() * 1000.0,
            mid.width(),
            mid.height()
        );

        let (rw, rh) = (1400.min(mid.width()), 900.min(mid.height()));
        let region = Region {
            full_w: mid.width(),
            full_h: mid.height(),
            x0: 0,
            y0: 0,
        };
        let sub = mid.crop(0, 0, rw, rh);
        let (_, tail) = build_pipeline_split_in_region(&heavy, sc, Some(region));
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            std::hint::black_box(tail.run(sub.clone()));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{best:8.1} ms   region tail {rw}x{rh}, CACHE HIT");
    }
```

- [ ] **Step 2: Measure**

```bash
cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF
```

Record the one-off native prefix and the region tail. Expect roughly 123 ms and 10 ms. If the region tail is far above 10 ms, say so rather than bury it.

- [ ] **Step 3: Update `CLAUDE.md`**

1. **Crate table:** `pipeline` gains `Region`; `app` gains `view_region`.
2. **RESUME HERE:** 1:1 rendering is done; **sharpening is now unblocked** and its design decisions are already recorded in its own spec section — point at `specs/2026-08-06-one-to-one-region-rendering-design.md` §"Why this comes before sharpening" so they are not re-derived. Note that sharpening no longer needs a `SIGMA_MIN` clamp.
3. **A new status section**, in the style of the preview-cache one, recording:
   - Two render modes, `max_dim` 1600 vs buffer long edge, switching at `fit · zoom > 1`.
   - The crop sits **between prefix and tail**: no inverse geometry (the prefix output is the post-geometry image) and **no seams**, because NR ran whole-buffer. That was the deciding argument against cropping first with per-stage margins.
   - **The cache key does not include the region** — panning re-runs only the tail and never invalidates anything.
   - Two cache entries, one per mode, so zooming in and out is instant. ~20 MB + ~78 MB.
   - `Region` is **optional and additive**: `build_pipeline_split_in_region` / `VignetteStage::new_in_region`, with the old functions delegating with `None`, so the CLI, export and all 20 goldens were untouched. Record the rejected alternatives and why (vignette-into-prefix breaks its position after the tone curve and its "white" endpoint; `ImageBuf` carrying origin pollutes the core type and changes its `PartialEq`; a full `RenderContext` churns every call site for something only zoom uses).
   - The master invariant `render_region == crop(render_whole)`, bit-identical, and that it validates region maths, vignette and cache at once.
   - `view_region` holds the viewport maths as pure functions **specifically** so clamping and off-by-one errors are testable rather than only visible.
   - **The demosaic measurements** (unpack flat ~850 ms; process 405 ms bilinear → 10,148 ms AHD; we never set `user_qual`) and the two conclusions: full-res is ~1.3 s not 11.4 s, and **the planned wgpu port would not fix it**, because that time is inside libraw's `dcraw_process`, upstream of every stage wgpu would touch.
   - Caveats: buffer-native is half of true full-res (detail reads as ~2× radius); the half-size decode skips demosaic entirely so a 1:1 view differs from the export in *kind*, not just resolution; +78 MB while zoomed; the before/after baseline stays preview-resolution; native mode renders slightly more than displayed between 0.51× and 1.0×.
4. **Docs list:** add `specs/2026-08-06-one-to-one-region-rendering-design.md` and `plans/2026-08-06-one-to-one-region-rendering.md`.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all
```

- [ ] **Step 5: Confirm no golden moved**

```bash
git status --short crates/pipeline/tests/golden
```

Expected: **empty**. No existing render path changed behaviour.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md crates/app/examples/bench_preview.rs
git commit -m "docs: record 1:1 region rendering and the demosaic measurements"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` all green.
- **All 20 golden references byte-identical.**
- The master invariant `render_region == crop(render_whole)` holds bit-identically.
- In the app: zooming past fit shows genuinely more detail, panning is smooth, zoom in/out is instant, and a vignette stays anchored to the frame when you zoom into a corner.
- `CLAUDE.md` records the modes, the optional `Region`, the two-entry cache, the demosaic measurements, and that the wgpu port would not address them.

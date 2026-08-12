//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{build_pipeline, build_pipeline_split_in_region, PrefixKey, Region, RenderScale};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

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
        if w == 0 || h == 0 {
            return None;
        }
        let x = self.x.min(w - 1);
        let y = self.y.min(h - 1);
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self { x, y, w: cw, h: ch })
    }
}

struct Request {
    edits: Edits,
    max_dim: usize,
    region: Option<RegionRect>,
}

/// Owns a worker thread that renders against a fixed editing buffer.
pub struct RenderEngine {
    req_tx: Sender<Request>,
    res_rx: Receiver<RenderedFrame>,
    _handle: JoinHandle<()>,
}

impl RenderEngine {
    /// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
    /// same file (0.5 for a half-size RAW decode, 1.0 for JPEG/PNG/TIFF).
    /// `on_done` is called after each render completes (e.g. `ctx.request_repaint`).
    pub fn new(
        buffer: Arc<ImageBuf>,
        source_scale: f32,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<RenderedFrame>();
        let handle = thread::spawn(move || {
            let mut cache = PrefixCache::default();
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                let frame = render_cached(
                    &buffer,
                    &req.edits,
                    req.max_dim,
                    source_scale,
                    req.region,
                    &mut cache,
                );
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();
            }
        });
        Self {
            req_tx,
            res_rx,
            _handle: handle,
        }
    }

    /// Queue a render at the given working resolution, optionally cropped to `region`.
    pub fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        let _ = self.req_tx.send(Request {
            edits,
            max_dim,
            region,
        });
    }

    /// Return the newest finished frame, if any (drops older ones).
    pub fn poll_latest(&self) -> Option<RenderedFrame> {
        let mut latest = None;
        while let Ok(f) = self.res_rx.try_recv() {
            latest = Some(f);
        }
        latest
    }
}

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

/// How far a render is from the full-resolution original: the source buffer may already
/// be a half-size decode (`source_scale`), and `downscale_to` shrinks it further. The
/// long edge is the reference because that is what `downscale_to` caps.
fn render_scale(buffer_long: usize, rendered_long: usize, source_scale: f32) -> RenderScale {
    if buffer_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(source_scale * rendered_long as f32 / buffer_long as f32)
}

/// Pack a rendered sRGB image into an RGBA8 frame.
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

/// A cached checkpoint is valid for one cumulative set of prefix parameters at one working
/// resolution.
type CacheKey = (PrefixKey, usize);

/// Which tier a checkpoint belongs to. The complete prefix output is a `Final`; every
/// intermediate is a `Mid`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tier {
    Final,
    Mid,
}

/// Cached prefix checkpoints, in two tiers that cannot evict each other.
///
/// `finals` holds complete prefix outputs, capped at two entries — one per render mode, so
/// zooming in and out never recomputes a prefix. That is byte for byte the policy this cache
/// had before intermediates existed, and it is preserved unconditionally: a native render must
/// never be able to evict the fit final to make room for its own intermediates.
///
/// `mids` holds the intermediates under a byte budget. Retention is effectively deepest-first,
/// and falls out of plain LRU rather than needing its own rule: within one render checkpoints
/// are inserted shallow to deep, so the deepest is the most recently used and the shallowest is
/// evicted first. That is the right order — the deeper checkpoint saves more upstream work.
///
/// Both tiers are most-recently-used first.
struct PrefixCache {
    finals: Vec<(CacheKey, Arc<ImageBuf>)>,
    mids: Vec<(CacheKey, Arc<ImageBuf>)>,
    mid_budget: usize,
}

impl Default for PrefixCache {
    /// ⚠ Hand-written rather than derived: a derived `Default` would give a budget of **0**,
    /// silently disabling every intermediate while leaving every correctness test passing.
    fn default() -> Self {
        Self::with_mid_budget(Self::MID_BUDGET_BYTES)
    }
}

impl PrefixCache {
    /// One complete prefix output per render mode (fit and native).
    const FINALS_CAP: usize = 2;

    /// Ceiling on the intermediates. 256 MB holds the full chain in both render modes on a
    /// 26 Mpx sensor (219 MB worst case); on larger files it bounds the growth instead, and
    /// native degrades to keeping only the deepest checkpoints that fit.
    const MID_BUDGET_BYTES: usize = 256 << 20;

    fn with_mid_budget(mid_budget: usize) -> Self {
        Self {
            finals: Vec::new(),
            mids: Vec::new(),
            mid_budget,
        }
    }

    /// Only the tests ask how full a tier is — production code never does.
    #[cfg(test)]
    fn final_len(&self) -> usize {
        self.finals.len()
    }

    #[cfg(test)]
    fn mid_len(&self) -> usize {
        self.mids.len()
    }

    fn bytes(img: &ImageBuf) -> usize {
        img.data().len() * std::mem::size_of::<f32>()
    }

    /// Look in both tiers, promoting a hit to the front of its own tier.
    fn get(&mut self, key: &CacheKey) -> Option<Arc<ImageBuf>> {
        for tier in [&mut self.finals, &mut self.mids] {
            if let Some(i) = tier.iter().position(|(k, _)| k == key) {
                let hit = tier.remove(i);
                let img = hit.1.clone();
                tier.insert(0, hit);
                return Some(img);
            }
        }
        None
    }

    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>, tier: Tier) {
        // A key lives in exactly one tier, but drop it from both: an edit set whose prefix
        // shrinks (its last stage going neutral) turns a former mid into a final.
        self.finals.retain(|(k, _)| k != &key);
        self.mids.retain(|(k, _)| k != &key);

        match tier {
            Tier::Final => {
                self.finals.insert(0, (key, img));
                self.finals.truncate(Self::FINALS_CAP);
            }
            Tier::Mid => {
                self.mids.insert(0, (key, img));
                let mut total: usize = self.mids.iter().map(|(_, i)| Self::bytes(i)).sum();
                while total > self.mid_budget {
                    match self.mids.pop() {
                        Some((_, evicted)) => total -= Self::bytes(&evicted),
                        None => break,
                    }
                }
            }
        }
    }
}

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

    // The prefix never depends on the region, so it is built and cached without one — which
    // is why panning re-runs only the tail and never invalidates the cache.
    let mid = match cache.get(&key) {
        Some(img) => img, // Arc clone: free
        None => {
            let (prefix, _) = build_pipeline_split_in_region(edits, scale, None);
            let img = Arc::new(prefix.run(buffer.downscale_to(max_dim)));
            cache.insert(key, img.clone(), Tier::Final);
            img
        }
    };

    // The region can only be resolved once the prefix output's dimensions are known, which is
    // why the split is built a second time here. Building is a few Box allocations.
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
        // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU
        // seam clean — so a cached image must be copied before the tail can run. Measured at
        // 0.6 ms against the 9 ms downscale it avoids.
        None => ((*mid).clone(), None),
    };

    let (_, tail) = build_pipeline_split_in_region(edits, scale, region);
    pack(tail.run(input), region)
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
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
    pack(build_pipeline(edits, scale).run(preview), None) // sRGB, 0..=1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Arc::new(img)
    }

    fn wait_for_frame(engine: &RenderEngine) -> RenderedFrame {
        for _ in 0..200 {
            if let Some(f) = engine.poll_latest() {
                return f;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no frame produced");
    }

    /// A buffer with varied content, so a stale cache actually shows up as different bytes.
    /// (The flat gray `tiny()` would hide almost any staleness.)
    fn varied_sized(n: usize) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(n, n);
        for y in 0..n {
            for x in 0..n {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / n as f32,
                        y as f32 / n as f32,
                        ((x + y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        Arc::new(img)
    }

    fn varied() -> Arc<ImageBuf> {
        varied_sized(16)
    }

    /// A distinct dummy image of `n` x `n`, for cache tests that never render.
    fn dummy(n: usize, v: f32) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(n, n);
        for y in 0..n {
            for x in 0..n {
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        Arc::new(img)
    }

    /// A cache key that differs only by `tag`, so tests can fill tiers without rendering.
    fn fake_key(tag: usize) -> CacheKey {
        let e = Edits {
            dehaze: edit_model::Dehaze {
                amount: 1.0 + tag as f32,
            },
            ..Edits::default()
        };
        (PrefixKey::new(&e, RenderScale::FULL), tag)
    }

    #[test]
    fn the_mid_budget_evicts_the_least_recently_used() {
        // Each dummy is 4*4*3*4 = 192 bytes; a budget of 400 holds two.
        let mut cache = PrefixCache::with_mid_budget(400);
        cache.insert(fake_key(0), dummy(4, 0.1), Tier::Mid);
        cache.insert(fake_key(1), dummy(4, 0.2), Tier::Mid);
        cache.insert(fake_key(2), dummy(4, 0.3), Tier::Mid);

        assert_eq!(cache.mid_len(), 2, "the budget must bound the mids tier");
        // Insertion order within one render is shallow -> deep, so the most recent insert is
        // the deepest checkpoint. Evicting the LRU is therefore evicting the shallowest.
        assert!(
            cache.get(&fake_key(2)).is_some(),
            "the deepest must survive"
        );
        assert!(cache.get(&fake_key(1)).is_some());
        assert!(
            cache.get(&fake_key(0)).is_none(),
            "the shallowest goes first"
        );
    }

    #[test]
    fn the_finals_tier_is_untouched_by_mid_pressure() {
        // The two tiers cannot evict each other: that is what preserves the zoom guarantee.
        // With a zero budget no mid survives, and both finals must still be there.
        let mut cache = PrefixCache::with_mid_budget(0);
        cache.insert(fake_key(10), dummy(4, 0.4), Tier::Final);
        cache.insert(fake_key(11), dummy(4, 0.5), Tier::Final);
        for tag in 0..5 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), 0);
        assert_eq!(cache.final_len(), 2);
        assert!(cache.get(&fake_key(10)).is_some());
        assert!(cache.get(&fake_key(11)).is_some());
    }

    #[test]
    fn the_finals_tier_still_holds_exactly_two() {
        let mut cache = PrefixCache::default();
        for tag in 0..4 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Final);
        }
        assert_eq!(cache.final_len(), 2);
        assert!(cache.get(&fake_key(3)).is_some());
        assert!(cache.get(&fake_key(2)).is_some());
        assert!(cache.get(&fake_key(0)).is_none());
    }

    #[test]
    fn the_default_budget_is_the_real_one() {
        // `#[derive(Default)]` would silently give a budget of 0 and disable every mid, which
        // would leave the whole feature inert with all correctness tests still passing.
        assert_eq!(
            PrefixCache::default().mid_budget,
            PrefixCache::MID_BUDGET_BYTES
        );
        assert!(PrefixCache::MID_BUDGET_BYTES > 0);
    }

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
        //
        // ⚠ Sharpening and local contrast *weaken* it, and that has to be said out loud
        // rather than left to an edit set that happens to omit them. A spatial stage in the
        // tail reads neighbours the cropped region genuinely does not have, so equality can
        // only hold in the interior. The excluded band is `tail_kernel_radius` wide —
        // precisely what `view_region::REGION_MARGIN` keeps off screen — and both the inset
        // and the buffer size are derived from it, so they track any future change to a
        // radius or a sigma.
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
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 70.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            ..Edits::default()
        };

        let inset = pipeline::tail_kernel_radius(&edits, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 2 * inset + 40, 2 * inset + 36);
        let n = 2 * inset + 80;
        let buf = varied_sized(n);

        // max_dim = n is native for this n x n buffer, i.e. no downscale, so the render
        // scale is exactly 1.0 and the inset above is the full-resolution kernel.
        let whole = render_cached(&buf, &edits, n, 1.0, None, &mut PrefixCache::default());

        // Guard against the test passing trivially: the spatial tail stages must actually
        // move pixels here, or the "weakened invariant" it exists to cover is not exercised.
        let unsharpened = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                local_contrast: edit_model::LocalContrast::default(),
                ..edits.clone()
            },
            n,
            1.0,
            None,
            &mut PrefixCache::default(),
        );
        assert_ne!(
            whole.rgba, unsharpened.rgba,
            "the spatial tail stages changed nothing"
        );

        assert!(
            x + w <= whole.width && y + h <= whole.height,
            "region must fit"
        );
        assert!(
            w > 2 * inset && h > 2 * inset,
            "region must be wider than twice the kernel ({inset})"
        );

        let got = render_cached(
            &buf,
            &edits,
            n,
            1.0,
            Some(RegionRect { x, y, w, h }),
            &mut PrefixCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        let (iw, ih) = (w - 2 * inset, h - 2 * inset);
        assert_eq!(
            crop_rgba(&got, inset, inset, iw, ih),
            crop_rgba(&whole, x + inset, y + inset, iw, ih),
            "the region's interior must match the whole render bit for bit"
        );
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
        assert_eq!(cache.final_len(), 2, "both modes should stay cached");

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
        assert_eq!(cache.final_len(), 2);
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
            engine.request((*e).clone(), 8, None);
            let got = wait_for_frame(&engine);
            let cold = render_once(&buf, e, 8, 1.0);
            assert_eq!(got.rgba, cold.rgba, "frame {i} differs from a cold render");
        }
    }

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

    #[test]
    fn render_scale_combines_the_decode_and_the_downscale() {
        // A half-size RAW decode downscaled again by 2 is a quarter of full resolution;
        // the spatial stages must be told that, or a preview lies about the export.
        assert_eq!(render_scale(8, 4, 0.5).get(), 0.25);
        // A full-size source rendered at full size is exactly 1.0.
        assert_eq!(render_scale(8, 8, 1.0).get(), 1.0);
        // Non-square: the long edge is the reference, matching downscale_to.
        assert_eq!(render_scale(10, 5, 1.0).get(), 0.5);
    }

    #[test]
    fn produces_rgba_frame_of_expected_size() {
        let engine = RenderEngine::new(tiny(), 1.0, || {});
        engine.request(Edits::default(), 4, None); // downscale 8 -> 4
        let f = wait_for_frame(&engine);
        assert_eq!((f.width, f.height), (4, 4));
        assert_eq!(f.rgba.len(), 4 * 4 * 4);
        // mid-gray linear 0.5 -> sRGB ~188; alpha 255
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
        assert_eq!(f.rgba[3], 255);
    }

    #[test]
    fn exposure_brightens() {
        let engine = RenderEngine::new(tiny(), 1.0, || {});
        engine.request(
            Edits {
                exposure: 2.0,
                ..Edits::default()
            },
            4,
            None,
        );
        let f = wait_for_frame(&engine);
        assert!(f.rgba[0] > 240); // much brighter
    }

    #[test]
    fn render_once_matches_expected_gray() {
        let img = tiny();
        let f = render_once(&img, &Edits::default(), 4, 1.0);
        assert_eq!((f.width, f.height), (4, 4));
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
    }
}

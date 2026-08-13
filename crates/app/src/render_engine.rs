//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{
    build_pipeline, build_pipeline_split_in_region, build_prefix_steps, PrefixKey, Region,
    RenderScale,
};
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
    /// The `max_dim` this frame was rendered at.
    ///
    /// ⚠ Carried rather than inferred. The canvas caches the post-geometry size from
    /// whole-image frames, and twice now a bug has come from inferring "whole image, therefore
    /// a fit render" — an inference that a cropped photo can make false, since a native rung
    /// times a small crop factor lands below the preview cap. This makes it a fact.
    pub max_dim: usize,
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
                // The stage count is for the tests and the bench; the worker only wants pixels.
                let (frame, _stages_run) = render_cached(
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
///
/// The quantization lives on `ImageBuf`, where the buffer and rayon already are — which is also
/// why `app` needs no `rayon` dependency of its own.
fn pack(out: ImageBuf, region: Option<Region>, max_dim: usize) -> RenderedFrame {
    RenderedFrame {
        width: out.width(),
        height: out.height(),
        rgba: out.to_rgba8(),
        region,
        max_dim,
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

/// ⚠ A zero budget would evict every intermediate on insert, leaving the whole checkpoint chain
/// inert while every correctness test still passed. Checked at compile time so it cannot regress.
///
/// It has to be a free-standing `const _` item: an *associated* const is only evaluated where it
/// is used, so writing this inside `impl PrefixCache` compiles happily with a budget of 0 — a
/// guard that does not guard, which is worse than none. Verified by setting the budget to 0 and
/// watching this fail.
const _: () = assert!(PrefixCache::MID_BUDGET_BYTES > 0);

/// The sizing decision behind [`PrefixCache::MID_BUDGET_BYTES`], asserted rather than left in a
/// comment where it could quietly stop being true.
///
/// A 26 Mpx (X-T4) full-resolution checkpoint is ~313 MB: the budget must clear one and must
/// *not* clear two, since a second would double the footprint to save ~200 ms on a rare
/// interaction. It must also clear a 40 Mpx body's (X-T5, X-H2) — which is the whole reason
/// this is 512 MiB rather than 320.
///
/// Compile-time for the same reason as the guard above: an ordinary `assert!` on constants is
/// both a clippy warning and a check that only runs if someone runs the tests.
const _: () = {
    const X_T4: usize = 6246 * 4170 * 3 * 4;
    const X_T5: usize = 7728 * 5152 * 3 * 4;
    assert!(PrefixCache::MID_BUDGET_BYTES > X_T4);
    assert!(PrefixCache::MID_BUDGET_BYTES < 2 * X_T4);
    assert!(PrefixCache::MID_BUDGET_BYTES > X_T5);
};

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

    /// Ceiling on the bytes held as intermediates.
    ///
    /// 512 MiB clears one full-resolution checkpoint (~313 MB on a 26 Mpx X-T4) but not two,
    /// which is the intent: the post-NR checkpoint makes a Dehaze drag at 1:1 reuse noise
    /// reduction, and a second checkpoint would double the footprint to save ~200 ms on a
    /// rarer interaction. ⚠ 512 rather than 320 MiB because 320 covers only 28.0 Mpx, and
    /// Fujifilm's current 40.2 Mpx bodies would get *zero* native checkpoints; 512 covers
    /// 44.7 Mpx. On a larger sensor than that, no intermediate survives and behaviour degrades
    /// to re-running the whole prefix — the ceiling working, not failing.
    ///
    /// At preview resolution it is never the binding constraint: entries are ~21 MB and
    /// [`PrefixCache::MIDS_CAP`] bites first.
    const MID_BUDGET_BYTES: usize = 512 << 20;

    /// Ceiling on the *number* of intermediates, independent of their size.
    ///
    /// ⚠ This is what makes a large byte budget safe. Dragging Straighten mints a new
    /// post-geometry key every frame, so without a count cap the tier fills with stale
    /// checkpoints bounded only by bytes — and a bigger budget would simply retain more
    /// garbage. A render's useful chain is at most 3 mids, so 6 keeps the current chain plus
    /// one previous state and discards the rest.
    const MIDS_CAP: usize = 6;

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
        std::mem::size_of_val(img.data())
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
                // Count first: it is O(1) and usually the binding constraint at preview
                // resolution, where entries are ~21 MB and the byte budget is nowhere near.
                self.mids.truncate(Self::MIDS_CAP);
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

/// Render one request, resuming from the deepest cached prefix checkpoint.
///
/// The prefix — geometry, noise reduction, dehaze — is by far the expensive half, and a slider
/// move usually changes only one of its stages, or none. So each stage's output is cached
/// separately and the walk restarts at the deepest checkpoint still valid, running only what is
/// downstream of the change. See
/// `docs/superpowers/specs/2026-08-12-second-prefix-checkpoint-design.md`.
///
/// Returns the frame and **how many prefix stages actually ran**. The worker ignores the count;
/// it exists because the correctness tests would all still pass with the cache deleted, so
/// something has to assert that work is genuinely being skipped.
fn render_cached(
    buffer: &Arc<ImageBuf>,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    region: Option<RegionRect>,
    cache: &mut PrefixCache,
) -> (RenderedFrame, usize) {
    let buffer_long = buffer.width().max(buffer.height());
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );

    // The prefix never depends on the region, so the whole chain is built and cached without
    // one — which is why panning re-runs only the tail and never invalidates a checkpoint.
    let steps = build_prefix_steps(edits, scale);
    let key_at = |n: usize| (PrefixKey::upto(&steps, n, scale), max_dim);
    let tier_at = |n: usize| {
        if n == steps.len() {
            Tier::Final
        } else {
            Tier::Mid
        }
    };

    // Resume from the deepest checkpoint still held. `n == 0` is the bare downscale.
    let mut resumed = None;
    for n in (0..=steps.len()).rev() {
        if let Some(img) = cache.get(&key_at(n)) {
            resumed = Some((n, img));
            break;
        }
    }

    let (start, mut img) = match resumed {
        Some(hit) => hit,
        None => {
            // ⚠ When nothing needs shrinking, `downscale_to` is a clone of a buffer the engine
            // already owns behind an `Arc`. Sharing it makes checkpoint 0 free in both time and
            // memory; caching it would cost a whole buffer (78 MB on an X-T4) to save 2.2 ms.
            let seed_is_free = max_dim == 0 || buffer_long <= max_dim;
            let seed = if seed_is_free {
                Arc::clone(buffer)
            } else {
                let s = Arc::new(buffer.downscale_to(max_dim));
                cache.insert(key_at(0), s.clone(), tier_at(0));
                s
            };
            (0, seed)
        }
    };

    // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU seam
    // clean — so each step that runs clones the checkpoint it starts from. Measured at 0.5 ms
    // (fit) and 2.1 ms (native) against the 13–117 ms each avoided stage costs.
    for (i, step) in steps.iter().enumerate().skip(start) {
        img = Arc::new(step.stage.apply((*img).clone()));
        cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
    }
    let ran = steps.len() - start;

    // ⚠ The complete prefix output must always live in the protected tier, even when we
    // resumed at it without running anything. A prefix that *shrinks* — its last stage going
    // neutral — turns a former intermediate into the final, and leaving it in the budgeted
    // tier would let a large render evict the very thing the zoom guarantee rests on.
    // `steps.is_empty()` is excluded because there the final *is* checkpoint 0, which is
    // either free (and must not be stored) or was already inserted as a final above.
    if ran == 0 && !steps.is_empty() {
        cache.insert(key_at(steps.len()), img.clone(), Tier::Final);
    }

    // The region can only be resolved once the prefix output's dimensions are known, which is
    // why the tail is built here rather than alongside the steps. Building is a few Box
    // allocations.
    let (pw, ph) = (img.width(), img.height());
    let rect = region.and_then(|r| r.clamped(pw, ph));
    let (input, region) = match rect {
        Some(r) => (
            img.crop(r.x, r.y, r.w, r.h),
            Some(Region {
                full_w: pw,
                full_h: ph,
                x0: r.x,
                y0: r.y,
            }),
        ),
        None => ((*img).clone(), None),
    };

    let (_, tail) = build_pipeline_split_in_region(edits, scale, region);
    (pack(tail.run(input), region, max_dim), ran)
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
    pack(build_pipeline(edits, scale).run(preview), None, max_dim) // sRGB, 0..=1
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

    /// Every prefix stage active, so a resume point is observable at each depth.
    fn full_prefix() -> Edits {
        Edits {
            exposure: 0.5,
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 40.0 },
            ..Edits::default()
        }
    }

    #[test]
    fn a_change_reruns_only_what_is_downstream_of_it() {
        // The feature itself. `every_cached_frame_matches_a_cold_render` proves the output is
        // right but would still pass with the cache deleted; this proves work is actually
        // being skipped, which is the entire point of the change.
        let buf = varied();
        let base = full_prefix();
        let mut cache = PrefixCache::default();

        let (_, cold) = render_cached(&buf, &base, 8, 1.0, None, &mut cache);
        assert_eq!(cold, 3, "a cold render runs every prefix stage");

        let (_, hit) = render_cached(&buf, &base, 8, 1.0, None, &mut cache);
        assert_eq!(hit, 0, "an unchanged prefix runs nothing");

        // A tail-only change still runs nothing: it resumes at the final checkpoint.
        let tail_only = Edits {
            exposure: -1.5,
            ..base.clone()
        };
        let (_, ran) = render_cached(&buf, &tail_only, 8, 1.0, None, &mut cache);
        assert_eq!(ran, 0, "a tail change must not touch the prefix");

        // Dehaze is last in the prefix, so only dehaze re-runs. This is the 86.8 ms case.
        let dehaze = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..base.clone()
        };
        let (_, ran) = render_cached(&buf, &dehaze, 8, 1.0, None, &mut cache);
        assert_eq!(ran, 1, "a dehaze change must resume after noise reduction");

        // Noise reduction is second, so NR and dehaze re-run but geometry does not.
        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 15.0,
                color: 15.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(&buf, &nr, 8, 1.0, None, &mut cache);
        assert_eq!(ran, 2, "an NR change must resume after geometry");

        // Geometry is first, so everything re-runs — but the downscale is still reused.
        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..base.clone()
        };
        let (_, ran) = render_cached(&buf, &geo, 8, 1.0, None, &mut cache);
        assert_eq!(ran, 3, "a geometry change invalidates the whole prefix");
    }

    #[test]
    fn the_native_seed_shares_the_buffer_rather_than_copying_it() {
        // When nothing is downscaled, `downscale_to` is a clone of a buffer the engine already
        // holds behind an `Arc`. Caching that would cost 78 MB on an X-T4 to save 2.2 ms, so
        // the chain seeds from the Arc instead and stores no checkpoint 0.
        let buf = varied(); // 16 x 16
        let base = full_prefix();

        let mut native = PrefixCache::default();
        render_cached(&buf, &base, 16, 1.0, None, &mut native);
        assert_eq!(
            native.mid_len(),
            2,
            "native should store only the post-geometry and post-NR checkpoints"
        );

        let mut fit = PrefixCache::default();
        render_cached(&buf, &base, 8, 1.0, None, &mut fit);
        assert_eq!(
            fit.mid_len(),
            3,
            "fit should additionally store the downscale"
        );
    }

    #[test]
    fn a_shrinking_prefix_does_not_serve_the_wrong_checkpoint() {
        // Turning dehaze off makes the post-NR checkpoint the *final* one. Its key is the same
        // value it had as a mid, so the tiers must not end up holding it twice with the two
        // copies free to diverge.
        let buf = varied();
        let mut cache = PrefixCache::default();
        let with = full_prefix();
        render_cached(&buf, &with, 8, 1.0, None, &mut cache);

        let without = Edits {
            dehaze: edit_model::Dehaze::default(),
            ..with.clone()
        };
        let (got, ran) = render_cached(&buf, &without, 8, 1.0, None, &mut cache);
        assert_eq!(ran, 0, "the post-NR checkpoint is now the final one");
        assert_eq!(got.rgba, render_once(&buf, &without, 8, 1.0).rgba);

        // ...and it must have been *promoted* into the protected tier, not left in the
        // budgeted one where a big render could evict the thing zooming back relies on.
        assert_eq!(cache.final_len(), 2);
        assert_eq!(cache.mid_len(), 2);
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
    fn the_finals_tier_holds_one_per_render_mode() {
        let mut cache = PrefixCache::default();
        for tag in 0..4 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Final);
        }
        assert_eq!(cache.final_len(), PrefixCache::FINALS_CAP);
        assert!(cache.get(&fake_key(3)).is_some());
        assert!(cache.get(&fake_key(2)).is_some());
        assert!(cache.get(&fake_key(0)).is_none());
    }

    #[test]
    fn the_mids_cap_bounds_stale_checkpoints() {
        // ⚠ Dragging Straighten mints a *new* post-geometry key every frame, so the mids tier
        // fills with checkpoints nobody will ever return to — bounded only by the byte budget,
        // which means a bigger budget retains more garbage. A render's useful chain is at most
        // 3 mids, so 6 keeps the current chain plus one previous state.
        let mut cache = PrefixCache::default();
        for tag in 0..20 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), PrefixCache::MIDS_CAP);
        assert!(
            cache.get(&fake_key(19)).is_some(),
            "the newest must survive"
        );
        assert!(cache.get(&fake_key(0)).is_none(), "the oldest must not");
    }

    #[test]
    fn the_budget_still_evicts_before_the_cap() {
        // Bytes bind first when the entries are large — which is the full-resolution case,
        // where one checkpoint is 313 MB and the cap is nowhere near reached.
        let mut cache = PrefixCache::with_mid_budget(400); // holds two 192-byte dummies
        for tag in 0..5 {
            cache.insert(fake_key(tag), dummy(4, 0.1), Tier::Mid);
        }
        assert_eq!(cache.mid_len(), 2);
        assert!(cache.mid_len() < PrefixCache::MIDS_CAP);
    }

    #[test]
    fn the_default_budget_is_the_real_one() {
        // `#[derive(Default)]` would silently give a budget of 0 and disable every mid, which
        // would leave the whole feature inert with all correctness tests still passing.
        // (That the const itself is non-zero is a compile-time assertion on `PrefixCache`.)
        assert_eq!(
            PrefixCache::default().mid_budget,
            PrefixCache::MID_BUDGET_BYTES
        );
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
        let (whole, _) = render_cached(&buf, &edits, n, 1.0, None, &mut PrefixCache::default());

        // Guard against the test passing trivially: the spatial tail stages must actually
        // move pixels here, or the "weakened invariant" it exists to cover is not exercised.
        let (unsharpened, _) = render_cached(
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

        let (got, _) = render_cached(
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

        for max_dim in [8usize, 16] {
            let scale = render_scale(16, rendered_long_edge(16, max_dim), 1.0);
            assert!(
                cache.get(&(PrefixKey::new(&e, scale), max_dim)).is_some(),
                "max_dim {max_dim} was evicted"
            );
        }

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
        let (got, _) = render_cached(
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
        // The master test: caching must be invisible in the output. The sequence walks a pure
        // hit, a tail-only change (resumes at the final), a dehaze change (resumes after NR),
        // an NR change (resumes after geometry), a geometry change (resumes at the seed), and
        // returns to earlier states so a stale checkpoint would show up.
        let buf = varied();
        let a = full_prefix();
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // tail only
        let c = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..a.clone()
        }; // deepest prefix stage
        let d = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 10.0,
                color: 10.0,
                ..Default::default()
            },
            ..a.clone()
        }; // middle prefix stage
        let e = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..a.clone()
        }; // first prefix stage

        // Guard against the whole test passing trivially: these must actually differ.
        let cold_a = render_once(&buf, &a, 8, 1.0);
        for (name, other) in [("b", &b), ("c", &c), ("d", &d), ("e", &e)] {
            assert_ne!(
                cold_a.rgba,
                render_once(&buf, other, 8, 1.0).rgba,
                "edit set {name} renders identically to a — the test proves nothing"
            );
        }

        let engine = RenderEngine::new(buf.clone(), 1.0, || {});
        for (i, ed) in [&a, &a, &b, &c, &a, &d, &e, &a, &c].iter().enumerate() {
            engine.request((*ed).clone(), 8, None);
            let got = wait_for_frame(&engine);
            let cold = render_once(&buf, ed, 8, 1.0);
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

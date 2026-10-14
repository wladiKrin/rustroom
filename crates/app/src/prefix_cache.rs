//! The prefix checkpoint cache.
//!
//! ⚠⚠ **Two tiers that cannot evict each other**, and the zoom guarantee depends on it: `finals`
//! holds complete prefix outputs at a fixed cap, so a native render can never evict the fit
//! final for its own intermediates.
//!
//! ⚠⚠ **Eviction is DEEPEST-FIRST and is a rule of its own**, not a consequence of insertion
//! order. Plain LRU is exactly backwards for a drag — see [`PrefixCache::evictable`], and the
//! 2026-09-14 fix it records.
//!
//! ⚠ Moved out of `render_engine.rs` verbatim on 2026-09-14, as a pure move with the test count
//! unchanged and no golden moved. **Every `⚠` comment below is the record of a decision or a
//! bug; do not reword one.**

use core_image::ImageBuf;
use pipeline::PrefixKey;
use std::sync::Arc;

/// A cached checkpoint is valid for one cumulative set of prefix parameters at one working
/// resolution.
pub(crate) type CacheKey = (PrefixKey, usize);

/// What the tail ran from: the final checkpoint, and the region rect when the frame was scoped
/// to one. See [`PrefixCache::resident_final`].
pub(crate) type TailKey = (CacheKey, Option<pipeline::Rect>);

/// Which tier a checkpoint belongs to. The complete prefix output is a `Final`; every
/// intermediate is a `Mid`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Tier {
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
/// `mids` holds the intermediates under a byte budget, and eviction takes the **deepest** entry.
///
/// ⚠⚠ **That is a rule of its own, and plain LRU is exactly backwards for the case that matters.**
/// This cache used to lean on insertion order — checkpoints go in shallow to deep, so the deepest
/// is the most recently used and the shallowest is evicted first — and defended it as *"the right
/// order, the deeper checkpoint saves more upstream work"*. **That reasoning holds for a cold
/// render and fails for a DRAG**, which is the interaction it is paid for: dragging step `g`
/// invalidates every checkpoint below it, so next frame the deep entries LRU kept are all stale
/// and the shallow one it discarded was the only usable resume point.
///
/// ⚠⚠ It bit only at full resolution, which is why it survived so long. A checkpoint is ~74 MiB
/// at rung 3123 and the 512 MB budget holds six, so `MIDS_CAP` bound it and the byte budget never
/// chose; at rung 6246 one entry is **298 MiB** and the budget holds exactly **one**, so a
/// five-step prefix evicted its own upstream checkpoints within a single render and every drag
/// frame restarted from zero — measured at **1730–2223 ms a frame**, re-running lens correction
/// each time. Ties fall back to least-recently-used, which is what the preview rungs still use.
///
/// Both tiers are most-recently-used first.
pub(crate) struct PrefixCache {
    pub(crate) finals: Vec<(CacheKey, Arc<ImageBuf>)>,
    pub(crate) mids: Vec<(CacheKey, Arc<ImageBuf>)>,
    pub(crate) mid_budget: usize,
    /// The last dehaze airlight and the key it was computed under. See [`Self::airlight`].
    airlight: Option<(pipeline::AirlightKey, [f32; 3])>,
    /// ONE checkpoint held on the GPU, with its key: the input of the last GPU prefix step that
    /// started a render. See [`Self::resident`].
    resident: Option<(CacheKey, gpu_pipeline::GpuImage)>,
    /// ONE tail input held on the GPU, with its key. See [`Self::resident_final`].
    resident_final: Option<(TailKey, gpu_pipeline::GpuImage)>,
    /// How many times [`Self::airlight`] actually ran the replay — the observable its tests read.
    #[cfg(test)]
    pub(crate) airlight_replays: usize,
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
    pub(crate) const FINALS_CAP: usize = 2;

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
    pub(crate) const MID_BUDGET_BYTES: usize = 512 << 20;

    /// Ceiling on the *number* of intermediates, independent of their size.
    ///
    /// ⚠ This is what makes a large byte budget safe. Dragging Straighten mints a new
    /// post-geometry key every frame, so without a count cap the tier fills with stale
    /// checkpoints bounded only by bytes — and a bigger budget would simply retain more
    /// garbage. A render's useful chain is at most 3 mids, so 6 keeps the current chain plus
    /// one previous state and discards the rest.
    pub(crate) const MIDS_CAP: usize = 6;

    pub(crate) fn with_mid_budget(mid_budget: usize) -> Self {
        Self {
            finals: Vec::new(),
            mids: Vec::new(),
            mid_budget,
            airlight: None,
            resident: None,
            resident_final: None,
            #[cfg(test)]
            airlight_replays: 0,
        }
    }

    /// The checkpoint under `key` as a GPU-resident image, uploading `img` only if it is not the
    /// one already held. Returns the image and whether it was uploaded now.
    ///
    /// ⚠⚠ **Why one slot is enough (2026-09-26).** A drag of a prefix step resumes from the SAME
    /// checkpoint on every frame — for a Straighten drag, the one before the geometry — so after
    /// the first frame this is a hit, and the step's 82 MB upload at rung 3200 disappears. A
    /// different drag replaces it on its first frame.
    ///
    /// ⚠ `key` is the checkpoint's full `CacheKey`, rung included, so a rung change misses rather
    /// than handing back an image of the wrong size. `None` when the GPU refuses the allocation;
    /// the caller then uploads through the chain as before.
    pub(crate) fn resident(
        &mut self,
        key: &CacheKey,
        img: &ImageBuf,
        ctx: &gpu_pipeline::GpuContext,
    ) -> Option<(&gpu_pipeline::GpuImage, bool)> {
        let hit = self.resident.as_ref().is_some_and(|(k, _)| k == key);
        if !hit {
            self.resident = Some((key.clone(), gpu_pipeline::GpuImage::upload(ctx, img)?));
        }
        self.resident.as_ref().map(|(_, r)| (r, !hit))
    }

    /// The tail's input under `key`, if it is the one held on the GPU.
    ///
    /// ⚠⚠ **The resident final (2026-09-26).** After a settle every tail-slider frame used to clone
    /// the CPU final (2.5 ms), memcpy it into staging (2.3 ms) and copy it up, before the tail ran:
    /// ~8-10 ms of a ~34 ms frame at rung 3200, on the most common gesture there is. The tail's
    /// chain copies from this image instead (~1 ms for 82 MB). ⚠ The KEY is the whole mechanism:
    /// the one-shot plane it replaces was anonymous, so it needed a thread stamp, a dimension
    /// check and a clear to be safe, and a stale take still rendered a plausible photograph. Here
    /// a hit under the wrong key is unrepresentable.
    ///
    /// ⚠ One slot: a miss replaces it, a rung switch costs one upload. Only ever READ by the tail,
    /// which copies from it, so it is never written after it is kept.
    pub(crate) fn resident_final(&self, key: &TailKey) -> Option<&gpu_pipeline::GpuImage> {
        self.resident_final
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, r)| r)
    }

    /// Keep `img` as the tail's input under `key`, replacing whatever was held.
    pub(crate) fn keep_resident_final(&mut self, key: TailKey, img: gpu_pipeline::GpuImage) {
        self.resident_final = Some((key, img));
    }

    /// The settle after a handoff: download a whole-frame resident that has no CPU final into
    /// the `Final` tier. `true` when it did.
    ///
    /// ⚠⚠ **This replaces a whole render.** A handoff frame leaves no CPU checkpoint, and the
    /// warm-behind used to re-run the last prefix step and a whole tail nobody saw — 92-108 ms
    /// measured — to leave one. The bytes are the same: the step already ran on the GPU, and this
    /// reads its output. ⚠ A rect resident is a region crop, never a final: the scoped drag's
    /// settle renders whole-buffer, as it always did.
    pub(crate) fn settle_from_resident(&mut self, ctx: &gpu_pipeline::GpuContext) -> bool {
        let Some(((key, None), r)) = self.resident_final.as_ref() else {
            return false;
        };
        if self.finals.iter().any(|(k, _)| k == key) {
            return false;
        }
        let key = key.clone();
        let Some(img) = r.download(ctx) else {
            return false;
        };
        self.insert(key, Arc::new(img), Tier::Final);
        true
    }

    /// The frame-global dehaze airlight for `edits`, replayed only when its key changes.
    ///
    /// ⚠⚠ **Measured before it was cached (2026-09-26): 12.1 ms of an 84.5 ms Straighten frame**
    /// on a 26 Mpx source, paid on every render, dehaze or not. A geometry drag never changes the
    /// key, so it now costs one key comparison.
    ///
    /// ⚠ One slot, not a map: renders alternate between a handful of edit states at most, and a
    /// miss costs one replay. ⚠ The source buffer is not in the key because this cache lives on
    /// one render worker, which owns exactly one buffer for its whole life.
    pub(crate) fn airlight(
        &mut self,
        edits: &edit_model::Edits,
        buffer: &ImageBuf,
        resampler: pipeline::Resampler,
        lens: Option<lens_profile::LensShot>,
    ) -> [f32; 3] {
        let key = pipeline::prefix_airlight_key(edits, resampler, lens.clone());
        if let Some((k, a)) = &self.airlight {
            if *k == key {
                return *a;
            }
        }
        let a = pipeline::prefix_airlight(edits, buffer, resampler, lens);
        self.airlight = Some((key, a));
        #[cfg(test)]
        {
            self.airlight_replays += 1;
        }
        a
    }

    /// Only the tests ask how full a tier is — production code never does.
    #[cfg(test)]
    pub(crate) fn final_len(&self) -> usize {
        self.finals.len()
    }

    #[cfg(test)]
    pub(crate) fn mid_len(&self) -> usize {
        self.mids.len()
    }

    pub(crate) fn bytes(img: &ImageBuf) -> usize {
        std::mem::size_of_val(img.data())
    }

    /// The entry it costs least to lose: the **deepest** checkpoint, ties broken toward the least
    /// recently used.
    ///
    /// ⚠⚠ **Deepest rather than oldest, because during a drag the deep entries are the stale
    /// ones.** Dragging prefix step `g` invalidates every checkpoint below `g`, so the render
    /// resumes at `g`, then inserts `g+1 .. n` — all of which its own next frame will miss. Plain
    /// LRU pushes the freshly-read checkpoint `g` to the back and evicts it to make room for
    /// entries that are already doomed.
    ///
    /// ⚠ `max_by_key` returns the LAST maximum, and the tuple makes that explicit: equal depths
    /// resolve to the larger index, which is further back and therefore least recently used.
    pub(crate) fn evictable(mids: &[(CacheKey, Arc<ImageBuf>)]) -> Option<usize> {
        mids.iter()
            .enumerate()
            .max_by_key(|(i, (k, _))| (k.0.depth(), *i))
            .map(|(i, _)| i)
    }

    /// Look in both tiers, promoting a hit to the front of its own tier.
    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<Arc<ImageBuf>> {
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

    pub(crate) fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>, tier: Tier) {
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
                    match Self::evictable(&self.mids) {
                        Some(i) => {
                            let (_, evicted) = self.mids.remove(i);
                            total -= Self::bytes(&evicted);
                        }
                        None => break,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_engine::fixtures::full_prefix;
    use pipeline::RenderScale;

    /// ⚠⚠ The memo's contract, both halves: a Straighten drag reuses the airlight (the 12 ms it
    /// exists to save) and an upstream edit replays it, and what it returns is exactly what a
    /// fresh `prefix_airlight` returns.
    #[test]
    fn the_airlight_is_replayed_only_when_its_key_moves() {
        let mut img = ImageBuf::new(96, 64);
        for y in 0..64 {
            for x in 0..96 {
                let v = ((x * 7 + y * 3) % 23) as f32 / 23.0;
                img.set_pixel(x, y, [v, 0.5 * v + 0.2, 1.0 - v]);
            }
        }
        let r = pipeline::Resampler::CatmullRom;
        let mut cache = PrefixCache::default();
        let mut e = edit_model::Edits {
            dehaze: edit_model::Dehaze { amount: 50.0 },
            ..Default::default()
        };
        let first = cache.airlight(&e, &img, r, None);
        assert_eq!(first, pipeline::prefix_airlight(&e, &img, r, None));
        assert_eq!(cache.airlight_replays, 1);

        for angle in [1.0, 2.0, 3.0] {
            e.geometry.angle = angle;
            assert_eq!(cache.airlight(&e, &img, r, None), first);
        }
        assert_eq!(
            cache.airlight_replays, 1,
            "a Straighten drag must not replay"
        );

        e.noise_reduction = edit_model::NoiseReduction {
            luminance: 80.0,
            color: 80.0,
            ..Default::default()
        };
        let moved = cache.airlight(&e, &img, r, None);
        assert_eq!(cache.airlight_replays, 2, "an upstream edit must replay");
        assert_eq!(moved, pipeline::prefix_airlight(&e, &img, r, None));
    }

    fn gpu() -> Option<&'static gpu_pipeline::GpuContext> {
        match gpu_pipeline::shared() {
            Some(c) => Some(c),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn small(seed: f32) -> ImageBuf {
        ImageBuf::from_vec(
            6,
            4,
            (0..6 * 4 * 3)
                .map(|i| (i as f32 * 0.01 + seed) % 1.0)
                .collect(),
        )
    }

    fn fake_key(tag: usize) -> CacheKey {
        let e = edit_model::Edits {
            dehaze: edit_model::Dehaze {
                amount: 1.0 + tag as f32,
            },
            ..Default::default()
        };
        (
            PrefixKey::new(
                &e,
                pipeline::RenderScale::FULL,
                pipeline::Resampler::Bilinear,
            ),
            tag,
        )
    }

    /// The slot is keyed on the checkpoint AND the rect: a hit needs both, and a miss replaces.
    #[test]
    fn the_resident_final_is_keyed_on_the_checkpoint_and_the_rect() {
        let Some(ctx) = gpu() else { return };
        let mut cache = PrefixCache::default();
        let rect = pipeline::Rect {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        };
        let key = (fake_key(1), None);
        assert!(cache.resident_final(&key).is_none(), "empty to start");
        let img = gpu_pipeline::GpuImage::upload(ctx, &small(0.1)).expect("upload");
        cache.keep_resident_final(key.clone(), img);
        assert!(
            cache.resident_final(&key).is_some(),
            "a hit under its own key"
        );
        assert!(
            cache.resident_final(&(fake_key(2), None)).is_none(),
            "another checkpoint misses"
        );
        assert!(
            cache.resident_final(&(fake_key(1), Some(rect))).is_none(),
            "the same checkpoint at a rect misses"
        );
        let crop = gpu_pipeline::GpuImage::upload(ctx, &small(0.5)).expect("upload");
        cache.keep_resident_final((fake_key(1), Some(rect)), crop);
        assert!(
            cache.resident_final(&key).is_none(),
            "one slot: keeping the crop replaced the whole frame"
        );
    }

    /// ⚠⚠ **The settle downloads a whole-frame resident that has no CPU final, once.** A second
    /// settle finds the final and does nothing; a rect resident is never a final.
    #[test]
    fn the_settle_downloads_the_resident_final_once_and_never_a_crop() {
        let Some(ctx) = gpu() else { return };
        let mut cache = PrefixCache::default();
        let src = small(0.2);
        let key = (fake_key(3), None);
        cache.keep_resident_final(
            key.clone(),
            gpu_pipeline::GpuImage::upload(ctx, &src).expect("upload"),
        );
        assert!(
            cache.settle_from_resident(ctx),
            "the first settle downloads"
        );
        let got = cache.get(&fake_key(3)).expect("the download is the final");
        assert_eq!(got.data(), src.data(), "sample for sample");
        assert_eq!(cache.final_len(), 1);
        assert!(
            !cache.settle_from_resident(ctx),
            "a second settle has nothing to do"
        );
        assert!(
            cache.resident_final(&key).is_some(),
            "and the resident survives the settle for the tail frames to come"
        );
        let rect = pipeline::Rect {
            x: 0,
            y: 0,
            w: 3,
            h: 2,
        };
        cache.keep_resident_final(
            (fake_key(4), Some(rect)),
            gpu_pipeline::GpuImage::upload(ctx, &small(0.3)).expect("upload"),
        );
        assert!(
            !cache.settle_from_resident(ctx),
            "a region crop is not a final and must never be cached as one"
        );
        assert_eq!(cache.final_len(), 1);
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

    #[test]
    fn the_mid_budget_evicts_the_least_recently_used() {
        // Each dummy is 4*4*3*4 = 192 bytes; a budget of 400 holds two.
        let mut cache = PrefixCache::with_mid_budget(400);
        cache.insert(fake_key(0), dummy(4, 0.1), Tier::Mid);
        cache.insert(fake_key(1), dummy(4, 0.2), Tier::Mid);
        cache.insert(fake_key(2), dummy(4, 0.3), Tier::Mid);

        assert_eq!(cache.mid_len(), 2, "the budget must bound the mids tier");
        // ⚠⚠ This tests the **tie-break**, not depth, and its comment used to claim otherwise:
        // *"insertion order within one render is shallow -> deep, so the most recent insert is the
        // deepest checkpoint; evicting the LRU is therefore evicting the shallowest."* `fake_key`
        // varies only the `max_dim` tag, so all three keys have the SAME depth — the fixture could
        // never exercise the story the comment told. Eviction is deepest-first now, and equal
        // depths still resolve to least-recently-used, which is what these three are.
        assert!(cache.get(&fake_key(2)).is_some(), "the newest survives");
        assert!(cache.get(&fake_key(1)).is_some());
        assert!(
            cache.get(&fake_key(0)).is_none(),
            "at equal depth the least recently used goes first"
        );
    }

    /// ⚠⚠ **The bug, as a unit test.** A drag resumes at the checkpoint *before* the stage it
    /// moves, and plain LRU evicted exactly that one: the render reads it, which promotes it to
    /// the front, and then its own later inserts push it to the back and out. At full resolution
    /// the budget holds a single 298 MiB checkpoint, so this happened on every frame.
    #[test]
    fn a_full_mid_tier_evicts_the_deepest_checkpoint_not_the_shallowest() {
        let e = full_prefix();
        let steps = pipeline::build_prefix_steps(
            &e,
            RenderScale::FULL,
            None,
            pipeline::Resampler::Bilinear,
            None,
            None,
        );
        assert!(
            steps.len() >= 3,
            "fixture check: a multi-step prefix is the whole point, got {}",
            steps.len()
        );
        let key = |n| (PrefixKey::upto(&steps, n, RenderScale::FULL), 0usize);
        // Each dummy is 4*4*3*4 = 192 bytes; a budget of 200 holds exactly one, which is the
        // full-resolution situation in miniature.
        let mut cache = PrefixCache::with_mid_budget(200);
        cache.insert(key(1), dummy(4, 0.1), Tier::Mid);
        cache.insert(key(2), dummy(4, 0.2), Tier::Mid);
        cache.insert(key(3), dummy(4, 0.3), Tier::Mid);

        assert_eq!(
            cache.mid_len(),
            1,
            "fixture check: the budget must hold exactly one"
        );
        assert!(
            cache.get(&key(1)).is_some(),
            "the shallowest must survive — it is the one a drag can still resume from"
        );
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_none());
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
}

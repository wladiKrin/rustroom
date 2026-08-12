# Per-stage prefix checkpoints — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cache an intermediate image at every prefix stage boundary, so changing one prefix stage re-runs only that stage and what follows it — taking a Dehaze drag with noise reduction on from 86.8 ms to ~47 ms.

**Architecture:** `pipeline` gains `build_prefix_steps(edits, scale) -> Vec<PrefixStep>`, pairing each prefix stage with a `PrefixStepKey` carrying its parameters. `PrefixKey` is redefined as the cumulative list of those keys plus the render scale, and is *derived from* `build_prefix_steps`, so a prefix stage with no key does not compile. `app::render_engine::render_cached` becomes a chain walk: find the deepest cached checkpoint, run only the steps below it, cache each result. The cache splits into two tiers that cannot evict each other — `finals` (cap 2, exactly today's policy and today's zoom guarantee) and `mids` (byte-budgeted LRU, 256 MB).

**Tech Stack:** Rust, `Arc<ImageBuf>`, `rayon` (already inside the stages), `cargo test` / `cargo clippy --all-targets --all-features` / `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-12-second-prefix-checkpoint-design.md`

**Build setup — every `cargo` command in this plan assumes this has been exported in the shell:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/lib.rs` | `PrefixStepKey`, `PrefixStep`, `build_prefix_steps`, `PrefixKey` | Modify (~lines 112–183 for the types, plus tests) |
| `crates/app/src/render_engine.rs` | Two-tier `PrefixCache`, chain walk in `render_cached` | Modify (~lines 6, 79–86, 160–254, plus tests) |
| `crates/app/examples/bench_preview.rs` | Reproducible numbers for the new checkpoints | Modify (append a section) |
| `CLAUDE.md` | Durable status | Modify (RESUME block + a feature section) |

No new files. Both changed source files already own exactly this responsibility; nothing needs splitting.

---

## Task 1: The prefix becomes a list of (key, stage) pairs

**Files:**
- Modify: `crates/pipeline/src/lib.rs:112-183` (add the types above `PrefixKey`; rewrite the prefix half of `build_pipeline_split_in_region`)
- Test: `crates/pipeline/src/lib.rs` (the `mod tests` at the bottom)

- [ ] **Step 1: Write the failing tests**

Add these two tests inside `mod tests` in `crates/pipeline/src/lib.rs`, after `the_prefix_holds_geometry_noise_reduction_and_dehaze`:

```rust
    #[test]
    fn the_prefix_steps_are_the_prefix_pipeline() {
        // build_pipeline_split_in_region is defined through build_prefix_steps, so the two
        // cannot drift: a stage added to one appears in the other.
        for edits in [Edits::default(), all_edits()] {
            let steps = build_prefix_steps(&edits, RenderScale::FULL);
            let (prefix, _) = build_pipeline_split(&edits, RenderScale::FULL);
            let from_steps: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
            assert_eq!(from_steps, prefix.stage_names());
        }
    }

    #[test]
    fn every_prefix_step_has_its_own_key() {
        // Each step's key must move with that step's parameters and with nothing else. If two
        // stages shared a key, a checkpoint would be reused across a change that altered it.
        let base = all_edits();
        let keys = |e: &Edits| -> Vec<PrefixStepKey> {
            build_prefix_steps(e, RenderScale::FULL)
                .iter()
                .map(|s| s.key)
                .collect()
        };
        let base_keys = keys(&base);
        assert_eq!(base_keys.len(), 3, "all three prefix stages should be active");

        let variants = [
            (
                0,
                Edits {
                    geometry: edit_model::Geometry {
                        angle: 7.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                1,
                Edits {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 80.0,
                        color: 80.0,
                        ..Default::default()
                    },
                    ..base.clone()
                },
            ),
            (
                2,
                Edits {
                    dehaze: edit_model::Dehaze { amount: 90.0 },
                    ..base.clone()
                },
            ),
        ];
        for (moved, e) in variants {
            let ks = keys(&e);
            for i in 0..3 {
                if i == moved {
                    assert_ne!(ks[i], base_keys[i], "step {i} should have changed");
                } else {
                    assert_eq!(ks[i], base_keys[i], "step {i} should not have changed");
                }
            }
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p pipeline the_prefix_steps_are_the_prefix_pipeline 2>&1 | tail -20`

Expected: FAIL to compile — `cannot find function `build_prefix_steps` in this scope`.

- [ ] **Step 3: Add the types and the builder**

In `crates/pipeline/src/lib.rs`, insert immediately **above** the `/// Everything [`build_pipeline_split`]'s prefix depends on…` doc comment on `PrefixKey` (line 112):

```rust
/// One prefix stage's parameters, identifying the image that stage produces.
///
/// ⚠ **Adding a prefix stage means adding a variant here.** [`build_prefix_steps`] returns
/// `(key, stage)` pairs, so a prefix stage with no key does not compile. That is deliberate:
/// a prefix stage missing from a cache key means the preview cache serves stale pixels with
/// nothing failing, and this is the guard that makes forgetting impossible rather than merely
/// caught by a test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrefixStepKey {
    Geometry(edit_model::Geometry),
    NoiseReduction(edit_model::NoiseReduction),
    Dehaze(edit_model::Dehaze),
}

/// One prefix stage together with the key identifying its output, so the renderer can cache
/// each stage's result independently.
pub struct PrefixStep {
    pub key: PrefixStepKey,
    pub stage: Box<dyn PipelineStage>,
}

/// The prefix as an ordered list of `(key, stage)` pairs.
///
/// An inactive stage produces no step — exactly as it produces no stage in the pipeline today.
/// That is what makes the checkpoint after an inactive stage *be* the checkpoint before it:
/// the cumulative keys are equal, so nothing extra is stored.
pub fn build_prefix_steps(edits: &Edits, scale: RenderScale) -> Vec<PrefixStep> {
    let mut steps: Vec<PrefixStep> = Vec::new();

    // Geometry runs first, so all adjustments operate on the final cropped pixels.
    if edits.geometry != edit_model::Geometry::default() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Geometry(edits.geometry),
            stage: Box::new(GeometryStage::new(&edits.geometry)),
        });
    }

    // Noise reduction comes before every adjustment: denoising the source is far easier than
    // denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are modifiers and
    // must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::NoiseReduction(edits.noise_reduction),
            stage: Box::new(NoiseReductionStage::new(&edits.noise_reduction, scale)),
        });
    }

    // Dehaze is a correction of the capture, not a grade, so it belongs upstream with noise
    // reduction — and after it, because dehaze is a contrast stretch that amplifies noise.
    //
    // ⚠ Being in the prefix is what makes a *frame-global* airlight possible at all: the
    // prefix always runs whole-buffer, so no region plumbing and no margin growth is needed.
    if edits.dehaze.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Dehaze(edits.dehaze),
            stage: Box::new(DehazeStage::new(&edits.dehaze, scale)),
        });
    }

    steps
}
```

- [ ] **Step 4: Redefine the prefix half of the split through it**

In `crates/pipeline/src/lib.rs`, in `build_pipeline_split_in_region`, replace the whole prefix construction — everything from `let mut prefix: Vec<Box<dyn PipelineStage>> = Vec::new();` down to and including the closing brace of the `if edits.dehaze.is_active() { … }` block, i.e. everything above `let mut tail: Vec<Box<dyn PipelineStage>> = Vec::new();` — with:

```rust
    // Defined through `build_prefix_steps` so the pipeline and the cache keys cannot drift:
    // one list, two consumers.
    let prefix: Vec<Box<dyn PipelineStage>> = build_prefix_steps(edits, scale)
        .into_iter()
        .map(|s| s.stage)
        .collect();
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p pipeline 2>&1 | tail -20`

Expected: PASS, all tests. In particular `the_prefix_holds_geometry_noise_reduction_and_dehaze`, `build_pipeline_is_exactly_prefix_then_tail` and `running_prefix_then_tail_matches_the_whole_pipeline` must still pass — they are what proves the refactor changed no behaviour.

- [ ] **Step 6: Verify the goldens are byte-identical**

Run: `cargo test -p pipeline --test golden 2>&1 | tail -5`

Expected: PASS, 29 golden tests. Do **not** set `UPDATE_GOLDEN`; a reference that needs updating here means the refactor changed behaviour and is a bug.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "refactor(pipeline): express the prefix as (key, stage) steps"
```

---

## Task 2: `PrefixKey` becomes the cumulative step key

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (the `PrefixKey` struct and impl)
- Modify: `crates/app/src/render_engine.rs` (call sites that relied on `Copy`)
- Test: `crates/pipeline/src/lib.rs` (`mod tests`)

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn a_truncated_prefix_key_ignores_the_stages_below_it() {
        // The whole feature rests on this: two edit sets that agree on the first n prefix
        // stages must share the checkpoint after stage n, however much they differ deeper.
        let a = all_edits();
        let b = Edits {
            dehaze: edit_model::Dehaze { amount: 95.0 },
            ..a.clone()
        };
        let steps_a = build_prefix_steps(&a, RenderScale::FULL);
        let steps_b = build_prefix_steps(&b, RenderScale::FULL);
        assert_eq!(steps_a.len(), 3);

        // Same up to and including noise reduction...
        for n in 0..=2 {
            assert_eq!(
                PrefixKey::upto(&steps_a, n, RenderScale::FULL),
                PrefixKey::upto(&steps_b, n, RenderScale::FULL),
                "checkpoint {n} should be shared"
            );
        }
        // ...and different once dehaze has run.
        assert_ne!(
            PrefixKey::upto(&steps_a, 3, RenderScale::FULL),
            PrefixKey::upto(&steps_b, 3, RenderScale::FULL)
        );

        // The full key is the deepest truncation, so `new` and `upto` cannot disagree.
        assert_eq!(
            PrefixKey::new(&a, RenderScale::FULL),
            PrefixKey::upto(&steps_a, steps_a.len(), RenderScale::FULL)
        );

        // Scale is part of every checkpoint, including the bare seed: it sets the radii of the
        // stages that follow.
        assert_ne!(
            PrefixKey::upto(&steps_a, 0, RenderScale::FULL),
            PrefixKey::upto(&steps_a, 0, RenderScale::new(0.25))
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p pipeline a_truncated_prefix_key 2>&1 | tail -20`

Expected: FAIL to compile — `no function or associated item named `upto` found for struct `PrefixKey``.

- [ ] **Step 3: Rewrite `PrefixKey`**

In `crates/pipeline/src/lib.rs`, replace the entire `PrefixKey` doc comment, struct and impl (lines 112–140 as they stand before this task) with:

```rust
/// Everything a cached prefix checkpoint depends on: the parameters of the prefix stages that
/// have run so far, plus the render scale.
///
/// The key is **derived from** [`build_prefix_steps`] rather than written alongside it, so a
/// prefix stage cannot be missing from it — see [`PrefixStepKey`]. The tests in this module
/// additionally assert that every *downstream* edit field leaves this key unchanged.
///
/// Compared with `==` rather than hashed, so there is no collision risk. The `f32` is safe in
/// a `PartialEq`: the only pathological value is `NaN`, which compares unequal to itself and
/// so fails toward a cache *miss*.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixKey {
    /// The keys of the prefix stages that have run, in order.
    steps: Vec<PrefixStepKey>,
    /// Included because it sets the noise-reduction and dehaze radii.
    scale: f32,
}

impl PrefixKey {
    /// The key for the output of the *whole* prefix.
    pub fn new(edits: &Edits, scale: RenderScale) -> Self {
        let steps = build_prefix_steps(edits, scale);
        Self::upto(&steps, steps.len(), scale)
    }

    /// The key for the checkpoint after the first `n` prefix stages have run.
    ///
    /// `n == 0` is the checkpoint straight after the downscale, before any stage. `n` beyond
    /// the end saturates, so a caller cannot key a checkpoint that does not exist.
    pub fn upto(steps: &[PrefixStep], n: usize, scale: RenderScale) -> Self {
        Self {
            steps: steps[..n.min(steps.len())].iter().map(|s| s.key).collect(),
            scale: scale.get(),
        }
    }
}
```

- [ ] **Step 4: Fix the `Copy` call sites in the app**

`PrefixKey` is no longer `Copy`. In `crates/app/src/render_engine.rs`, `PrefixCache::insert` currently compares `*k != key` after `key` has been moved into the comparison; make it borrow instead. Replace the body of `insert`:

```rust
    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, img));
        self.entries.truncate(Self::CAP);
    }
```

with:

```rust
    fn insert(&mut self, key: CacheKey, img: Arc<ImageBuf>) {
        self.entries.retain(|(k, _)| k != &key);
        self.entries.insert(0, (key, img));
        self.entries.truncate(Self::CAP);
    }
```

(This tier is replaced wholesale in Task 3; the edit here exists only to keep the tree compiling and the tests green between commits.)

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --all 2>&1 | tail -25`

Expected: PASS everywhere. `prefix_key_ignores_every_downstream_field`, `prefix_key_changes_with_prefix_fields_and_scale`, `the_cache_keeps_one_entry_per_render_mode` and `every_cached_frame_matches_a_cold_render` must pass **unchanged** — they are the proof that redefining the key preserved its meaning.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/app/src/render_engine.rs
git commit -m "refactor(pipeline): make PrefixKey the cumulative step key"
```

---

## Task 3: The cache becomes two tiers

**Files:**
- Modify: `crates/app/src/render_engine.rs:160-196` (the `CacheKey` type and `PrefixCache`)
- Test: `crates/app/src/render_engine.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/app/src/render_engine.rs`:

```rust
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
        assert!(cache.get(&fake_key(2)).is_some(), "the deepest must survive");
        assert!(cache.get(&fake_key(1)).is_some());
        assert!(cache.get(&fake_key(0)).is_none(), "the shallowest goes first");
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app the_mid_budget 2>&1 | tail -20`

Expected: FAIL to compile — `cannot find type `Tier` in this scope`.

- [ ] **Step 3: Replace `PrefixCache`**

In `crates/app/src/render_engine.rs`, replace everything from the `/// The cached prefix output is valid for one set of prefix parameters…` doc comment through the closing brace of `impl PrefixCache` (lines 160–196) with:

```rust
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
```

- [ ] **Step 4: Adapt the one existing test that counted entries**

`the_cache_keeps_one_entry_per_render_mode` calls `cache.len()`, which no longer exists. In `crates/app/src/render_engine.rs`, replace its two `assert_eq!(cache.len(), 2 …)` lines with `final_len()`:

```rust
        render_cached(&buf, &e, 8, 1.0, None, &mut cache); // "fit"
        render_cached(&buf, &e, 16, 1.0, None, &mut cache); // "native"
        assert_eq!(cache.final_len(), 2, "both modes should stay cached");
```

and further down:

```rust
        render_cached(&buf, &e2, 16, 1.0, None, &mut cache);
        assert_eq!(cache.final_len(), 2);
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p app render_engine 2>&1 | tail -25`

Expected: PASS. The four new cache tests plus every existing `render_engine` test, including `every_cached_frame_matches_a_cold_render` and `a_region_render_matches_the_whole_render_cropped` — the cache is restructured here but `render_cached` still only ever writes one checkpoint, so behaviour is unchanged.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): split the prefix cache into finals and budgeted mids"
```

---

## Task 4: `render_cached` becomes a chain walk

**Files:**
- Modify: `crates/app/src/render_engine.rs:6` (imports), `:79-86` (the worker call), `:204-254` (`render_cached`)
- Test: `crates/app/src/render_engine.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/app/src/render_engine.rs`:

```rust
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
```

Then **replace** the body of the existing `every_cached_frame_matches_a_cold_render` with a sequence that visits every checkpoint:

```rust
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app a_change_reruns_only 2>&1 | tail -20`

Expected: FAIL to compile — `render_cached` returns `RenderedFrame`, not a tuple: ``expected `RenderedFrame`, found `(_, _)` ``.

- [ ] **Step 3: Rewrite `render_cached`**

In `crates/app/src/render_engine.rs`, replace the whole of `render_cached` (its doc comment plus body, lines 198–254) with:

```rust
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
    (pack(tail.run(input), region), ran)
}
```

- [ ] **Step 4: Update the import and the worker call site**

In `crates/app/src/render_engine.rs`, line 6, add `build_prefix_steps` **and `PipelineStage`** to the `pipeline` import. The trait is not optional: `step.stage.apply(..)` is a trait method on a `Box<dyn PipelineStage>` and will not resolve without the trait in scope — the module has only ever called `Pipeline::run` before now.

```rust
use pipeline::{
    build_pipeline, build_pipeline_split_in_region, build_prefix_steps, PipelineStage, PrefixKey,
    Region, RenderScale,
};
```

And in `RenderEngine::new`, replace:

```rust
                let frame = render_cached(
                    &buffer,
                    &req.edits,
                    req.max_dim,
                    source_scale,
                    req.region,
                    &mut cache,
                );
```

with:

```rust
                // The stage count is for the tests and the bench; the worker only wants pixels.
                let (frame, _stages_run) = render_cached(
                    &buffer,
                    &req.edits,
                    req.max_dim,
                    source_scale,
                    req.region,
                    &mut cache,
                );
```

- [ ] **Step 5: Fix the remaining test call sites**

Three existing tests call `render_cached` and use its result directly. In `crates/app/src/render_engine.rs`:

In `a_region_render_matches_the_whole_render_cropped`, change the three calls to destructure:

```rust
        let (whole, _) = render_cached(&buf, &edits, n, 1.0, None, &mut PrefixCache::default());
```

```rust
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
```

```rust
        let (got, _) = render_cached(
            &buf,
            &edits,
            n,
            1.0,
            Some(RegionRect { x, y, w, h }),
            &mut PrefixCache::default(),
        );
```

In `a_region_bigger_than_the_image_is_clamped`:

```rust
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
```

`the_cache_keeps_one_entry_per_render_mode` discards its results already, so its `render_cached(…);` calls need no change.

- [ ] **Step 6: Run the app tests**

Run: `cargo test -p app 2>&1 | tail -30`

Expected: PASS, all of them. The ones that matter most: `a_change_reruns_only_what_is_downstream_of_it` (the feature), `every_cached_frame_matches_a_cold_render` (correctness) and `a_region_render_matches_the_whole_render_cropped` (unchanged, and it must not have needed touching beyond destructuring).

- [ ] **Step 7: Prove the counter test can fail**

Temporarily neuter the cache by setting both capacities to zero in `crates/app/src/render_engine.rs`:

```rust
    const FINALS_CAP: usize = 0;
    const MID_BUDGET_BYTES: usize = 0;
```

(Do it this way rather than by gutting the resume loop: deleting the loop leaves `let mut resumed = None;` never assigned, so the type cannot be inferred and you get a compile error instead of the test failure you are trying to observe.)

Run: `cargo test -p app a_change_reruns_only 2>&1 | tail -20`

Expected: FAIL with ``assertion `left == right` failed: an unchanged prefix runs nothing`` and `left: 3, right: 0`. Then **revert both constants** and re-run to confirm PASS. A test that cannot fail is not a test.

- [ ] **Step 8: Full workspace check**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features 2>&1 | tail -20 && cargo test --all 2>&1 | tail -15`

Expected: `fmt` silent, clippy with no warnings, all tests pass including the 29 goldens.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): resume renders from the deepest cached prefix checkpoint"
```

---

## Task 5: Measure it

**Files:**
- Modify: `crates/app/examples/bench_preview.rs` (append a section before the closing brace of `main`)

- [ ] **Step 1: Add the checkpoint benchmark**

Append this block at the end of `main` in `crates/app/examples/bench_preview.rs`, after the existing region-tail section:

```rust
    // What each new checkpoint actually saves. Timed through the public `build_prefix_steps`
    // rather than through `RenderEngine`, so the number is the work itself and not thread
    // hand-off noise — the same choice the cache-hit section above makes.
    {
        // `PipelineStage` is needed for `step.stage.apply(..)` — it is a trait method.
        use pipeline::{
            build_pipeline_split_in_region, build_prefix_steps, PipelineStage, RenderScale,
        };
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale * PREVIEW_MAX_DIM as f32 / buffer_long as f32);
        let heavy = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 60.0 },
            exposure: 0.5,
            ..Edits::default()
        };
        let steps = build_prefix_steps(&heavy, sc);
        let (_, tail) = build_pipeline_split_in_region(&heavy, sc, None);
        println!("\n  prefix steps: {}", steps.len());

        // Build every checkpoint once.
        let mut checkpoints = vec![buf.downscale_to(PREVIEW_MAX_DIM)];
        for step in &steps {
            let next = step.stage.apply(checkpoints.last().unwrap().clone());
            checkpoints.push(next);
        }

        // Resuming at checkpoint `start` means running steps[start..] plus the tail and pack.
        for start in 0..=steps.len() {
            let label = match start {
                0 => "geometry change  (resume at the downscale)",
                1 => "NR change        (resume after geometry)",
                2 => "dehaze change    (resume after NR)",
                _ => "tail slider      (resume at the final)",
            };
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let mut img = checkpoints[start].clone();
                for step in &steps[start..] {
                    img = step.stage.apply(img);
                }
                let out = tail.run(img);
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
            println!("{best:8.1} ms   {label}");
        }
    }
```

- [ ] **Step 2: Run it**

Run: `cargo run --release -p app --example bench_preview ~/Desktop/fuji/DSCF6509.RAF 2>&1 | tail -25`

Expected: five new lines. The one that matters is `dehaze change (resume after NR)` at roughly **47 ms**, against the `dehaze 60 + NR both 50` whole-render line near the top of the same output at ~87 ms. `tail slider` should be ~14 ms, matching the existing `CACHE HIT` line.

- [ ] **Step 3: Record the real numbers**

Write the measured figures into the block comment above the new section, replacing nothing else. If `dehaze change` is not close to 47 ms, stop and investigate before continuing — the whole feature is that number.

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/bench_preview.rs
git commit -m "bench: time each prefix checkpoint's resume point"
```

---

## Task 6: Verify against the real app

**Files:** none changed unless something is wrong.

- [ ] **Step 1: Launch the GUI**

Run: `cargo run --release -p app`

- [ ] **Step 2: Check the feature by hand**

Open a RAW from `~/Desktop/fuji/`. Then:

1. Set Noise Reduction luminance and colour to ~50, and Dehaze to ~40.
2. Drag the **Dehaze** slider. It should now track roughly as smoothly as Exposure does — this is the 87 ms → 47 ms case, and it is the one thing to look at.
3. Set a **Straighten** angle, then drag Noise Reduction. The rotation should no longer be recomputed on every frame.
4. Zoom past 1:1 and back to fit twice. Neither direction should recompute a prefix — that is the finals tier's guarantee, and it is the thing most at risk from this change.
5. Watch RSS in Activity Monitor while doing the above with all three prefix stages active and both zoom levels visited. Expect roughly +220 MB over the previous build.

- [ ] **Step 3: Commit nothing, or fix and re-run**

If anything above misbehaves, fix it with a test that reproduces it first.

---

## Task 7: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` (the `▶ RESUME HERE` block, and a new feature section)

- [ ] **Step 1: Rewrite the RESUME block's next action**

Replace the `▶ NEXT ACTION: a second prefix cache checkpoint.` paragraph and the table and bullet list that follow it with a short note that the checkpoints have landed, and point `NEXT ACTION` at the top item that remains: **parallelising the RGBA8 pack** in `render_engine::pack` (still a sequential `Vec::push` loop, ~35% of a cached frame).

- [ ] **Step 2: Add a feature section**

Add a section in the house style, above `**Preview prefix cache done…**`, covering:

- The measured table from Task 5, and the corrected geometry cost (13.6 ms fit / 52.2 ms native for a straighten angle, ~1 ms for a crop alone) — this replaces the claim in the old RESUME block that a second checkpoint's only win was the ~9 ms downscale.
- ⚠ The free native seed: `downscale_to` clones when nothing needs shrinking, so checkpoint 0 shares the engine's `Arc` instead of costing 78 MB to save 2.2 ms.
- ⚠ `PrefixStepKey` making a missing cache key a **compile error** rather than a test failure — the same upgrade `Edits::merge_from`'s exhaustive destructure made.
- ⚠ The two tiers existing so they cannot evict each other, and the honest note that on an X-T4 the 256 MB budget never bites: memory goes ~99 MB → ~318 MB, and the budget is a ceiling for larger sensors plus a guarantee that degradation is deepest-first rather than accidental.
- The counter test `a_change_reruns_only_what_is_downstream_of_it`, and *why* it exists: the correctness tests would all pass with the cache deleted.
- Accepted caveats, from the spec's list.

- [ ] **Step 3: Update the docs index**

Add `2026-08-12-second-prefix-checkpoint-design.md` to the specs list and `2026-08-12-second-prefix-checkpoint.md` to the plans list in the `Docs (docs/superpowers/)` paragraph at the bottom of `CLAUDE.md`.

- [ ] **Step 4: Final check**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features 2>&1 | tail -5 && cargo test --all 2>&1 | tail -10`

Expected: all green, 29 goldens byte-identical.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record per-stage prefix checkpoints"
```

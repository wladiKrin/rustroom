# Auto-mask coverage cache — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠⚠ **Conventions that are not negotiable here.** Commit messages carry **no `Co-Authored-By`
> trailer** and no AI attribution. **Never push** — the user pushes himself. Work stays on the
> existing branch `m7.6b-auto-mask-coverage-cache`.

**Goal:** Stop re-rasterising an auto-masked brush's coverage on every render, so dragging a mask's
own adjustment sliders at 1:1 costs the tail alone rather than the tail plus 42.55 ms.

**Architecture:** The stage builds the coverage (it is the only thing holding the image), so it
**reports** what it built through a write side on `BrushCoverage`; `render_engine` stores it under a
key widened with a base identity and hands it back as the `prebuilt` map that parameter already
accepts. Cache lookup stays in `app`, beside `PrefixKey`.

**Tech Stack:** Rust workspace; `pipeline` (no GUI), `rustroom_app` lib, `Rustroom` bin.

**Spec:** `docs/superpowers/specs/2026-09-09-auto-mask-coverage-cache-design.md` — read **§4.0**
first; it corrects the assumption §4 was written on.

---

## Standing rules for every task

- **The gate before every commit**, each exit code read from its **own file**, never a pipe
  (`${PIPESTATUS[0]}` is bash and this shell is zsh):

  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  cargo fmt --all
  cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
  cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
  cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
  grep -c "^test .* ok$" /tmp/t.log          # expect 2166 + what this task added
  grep -c "^test golden_.* ok$" /tmp/t.log   # expect 60, every task, every time
  ```

  ⚠ `-- -D warnings` is not optional: CI runs it, and commits have shipped here with clippy failing
  at least twelve times because a local check omitted it.

- **⚠ Read the passed-count on every filtered run.** `test result: ok` says nothing about what ran.
  `cargo test` takes a **substring, not a regex** — `"a\|b"` matches nothing and prints
  `0 passed; N filtered out`, which reads as green.

- **⚠ Copy the file before every sabotage, and restore from the copy — never `git checkout <file>`.**
  That destroyed an hour of uncommitted work in the previous milestone, which is the second time
  this repository has recorded it.

- **⚠ After any scripted edit, assert the new text is present**, and check
  `git diff | grep '^-' | wc -l` against the number of lines the change should remove. A `sed` that
  matches nothing exits 0 — and `cargo fmt` reflows long conditions, which is exactly how one
  sabotage came back falsely green last milestone.

- **⚠ A sabotage is believed only after a `test result:` line appears.** One that fails to compile
  looks exactly like one that passes.

- **All 60 goldens must stay byte-identical.** This changes *when* coverage is computed, never what
  it is. That is the assertion, not "still green".

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/lib.rs` | `BrushCoverage` — the positional coverage channel | **add** the write side |
| `crates/pipeline/src/stages/mask.rs` | `MaskStage` — builds coverage when handed none | **report** what it built |
| `crates/app/src/render_engine.rs` | `BrushCache`, `BrushKey`, `render_cached` | **per-mask entries + base identity** |
| `crates/app/examples/bench_auto_mask_cov.rs` | the cost measurement | **extend** with the after figure |

⚠ `crates/app/src/render_engine.rs` is **~2,700 lines**, well past the ~600 marker, and grows here.
Reported rather than silently added to; not split in this plan.

---

## Task 1: `BrushCoverage` gains a write side

**Why:** the stage is the only thing holding the image, so it is the only thing that can build a
gating brush's coverage. It needs somewhere to put it. See spec §4.0.

**Files:**
- Modify: `crates/pipeline/src/lib.rs:14-23`

- [ ] **Step 1: Write the failing test**

Append to `crates/pipeline/src/lib.rs` (create `#[cfg(test)] mod tests` at the end of the file if it
has none — check with `grep -n "^#\[cfg(test)\]" crates/pipeline/src/lib.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn a_map() -> std::sync::Arc<brush::CoverageMap> {
        std::sync::Arc::new(brush::CoverageMap::default())
    }

    /// ⚠⚠ The write side exists because `MaskStage` is the only thing holding the image, so it is
    /// the only thing that can build a gating brush's coverage — and `render_engine`, which owns
    /// the key, has to be able to see what it built. See the spec's §4.0.
    #[test]
    fn a_deposited_map_can_be_taken_back_out() {
        let c = BrushCoverage::with_slots(3);
        assert!(c.take_built(1).is_none(), "nothing deposited yet");
        c.deposit(1, a_map());
        assert!(c.take_built(1).is_some(), "the deposit must be readable");
        assert!(
            c.take_built(1).is_none(),
            "taking it must clear the slot, so a stale map cannot be read twice"
        );
    }

    /// ⚠ An index past the end is ignored rather than panicking: the mask list can change between
    /// the coverage being built and a stage running, exactly as `suppressed_spot`'s index can.
    #[test]
    fn depositing_out_of_range_is_ignored() {
        let c = BrushCoverage::with_slots(1);
        c.deposit(7, a_map());
        assert!(c.take_built(7).is_none());
    }

    /// ⚠ The read side is untouched, and that is what keeps every existing caller working.
    #[test]
    fn the_prebuilt_side_still_reads_by_position() {
        let c = BrushCoverage {
            maps: vec![None, Some(a_map())],
            built: Default::default(),
        };
        assert!(c.get(0).is_none());
        assert!(c.get(1).is_some());
        assert!(c.get(9).is_none(), "past the end is None, never a panic");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p pipeline --lib deposited 2>&1 | tail -15
```

Expected: **compile error**, `no function or associated item named 'with_slots'`. Do not proceed
until you have seen it.

- [ ] **Step 3: Write the implementation**

Replace `crates/pipeline/src/lib.rs:14-23` with:

```rust
pub struct BrushCoverage {
    pub maps: Vec<Option<std::sync::Arc<brush::CoverageMap>>>,
    /// Where a stage reports coverage it had to build itself.
    ///
    /// ⚠⚠ **The read side is what a caller supplies; this is what a stage gives back.** A brush
    /// that gates on the image can only be rasterized where the image is — inside
    /// `MaskStage::apply` — so `render_engine`, which owns the cache key, would otherwise never
    /// see the map it wants to keep. See the coverage-cache spec's §4.0.
    ///
    /// ⚠ `Mutex` rather than `RefCell`: this crosses into stages that must stay `Sync`. It is
    /// uncontended — `apply` runs sequentially, once per mask per tail.
    pub built: Vec<std::sync::Mutex<Option<std::sync::Arc<brush::CoverageMap>>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i).and_then(|m| m.clone())
    }

    /// A coverage set with `n` empty deposit slots and nothing prebuilt.
    pub fn with_slots(n: usize) -> Self {
        Self {
            maps: vec![None; n],
            built: (0..n).map(|_| std::sync::Mutex::new(None)).collect(),
        }
    }

    /// Report coverage built at position `i`. Out of range is ignored.
    ///
    /// ⚠ Ignored rather than panicking: the mask list can change between this set being made and a
    /// stage running, and a render is not worth aborting over a slot that no longer exists.
    pub fn deposit(&self, i: usize, map: std::sync::Arc<brush::CoverageMap>) {
        if let Some(slot) = self.built.get(i) {
            if let Ok(mut g) = slot.lock() {
                *g = Some(map);
            }
        }
    }

    /// Take what was reported at position `i`, clearing the slot.
    ///
    /// ⚠ Clearing is what stops a map surviving into a render it does not describe — the caller
    /// stores it under a key computed for *this* render.
    pub fn take_built(&self, i: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.built.get(i).and_then(|s| s.lock().ok()?.take())
    }
}
```

⚠ `vec![None; n]` needs `Option<Arc<CoverageMap>>: Clone`, which it is. If `CoverageMap` lacks
`Default`, use `(0..n).map(|_| None).collect()` instead and check with
`grep -n "pub struct CoverageMap" -B 3 crates/pipeline/src/brush/raster.rs`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline --lib deposited 2>&1 | tail -8
cargo test -p pipeline --lib depositing_out_of_range 2>&1 | tail -5
cargo test -p pipeline --lib the_prebuilt_side_still_reads 2>&1 | tail -5
```

Expected: `1 passed` each. ⚠ Read each passed-count; the first filter matches only one of the three.

- [ ] **Step 5: Fix the existing construction sites**

`BrushCoverage` is built in `render_engine.rs` as `BrushCoverage { maps }`. The compiler will name
every site. Give each `built: Default::default()` for now — Task 3 replaces that with real slots.

- [ ] **Step 6: Sabotage-verify**

Copy the file first (`cp crates/pipeline/src/lib.rs /tmp/pl.bak`). Change `take_built` to clone
instead of taking:

```rust
    pub fn take_built(&self, i: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.built.get(i).and_then(|s| s.lock().ok()?.clone())
    }
```

`a_deposited_map_can_be_taken_back_out` must fail on its third assertion. Restore from the copy and
check `git status`.

- [ ] **Step 7: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
BrushCoverage gains a write side, so a stage can report what it built

A brush that gates on the image can only be rasterized where the image is
-- inside MaskStage::apply -- so render_engine, which owns the cache key,
could never see the map it wants to keep. The read side is what a caller
supplies; this is what a stage gives back.

Mutex rather than RefCell because this crosses into stages that must stay
Sync. Uncontended: apply runs sequentially, once per mask per tail.

take_built clears the slot, so a map cannot survive into a render it does
not describe.
MSG
```

---

## Task 2: `MaskStage` reports the coverage it builds

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs:121-134` (construction), `:245-278` (the build site)

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/pipeline/src/stages/mask.rs`:

```rust
    /// ⚠⚠ **The carrier.** `BrushCoverage`'s own test proves the channel; nothing proves the stage
    /// uses it. That is the W5 shape — creative LUTs shipped with the maths fully tested and
    /// nothing carrying the data to it.
    ///
    /// ⚠ An **auto-masked** brush specifically: a plain one is prebuilt by `render_engine` and
    /// never reaches this path, so a fixture using one would pass against a stage that deposits
    /// nothing.
    #[test]
    fn an_auto_masked_brush_reports_the_coverage_it_builds() {
        let mut brush = edit_model::Brush::default();
        brush.strokes.push(edit_model::BrushStroke {
            points: vec![[0.5, 0.5]],
            radius: 0.2,
            hardness: 50.0,
            flow: 100.0,
            erase: false,
            auto_mask: true,
            tolerance: 35.0,
        });
        let masked = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(brush),
            adjustments: Default::default(),
        };
        let coverage = crate::BrushCoverage::with_slots(1);
        let stage = MaskStage::new_in_region(
            &masked,
            RenderScale::new(1.0),
            None,
            &edit_model::Geometry::default(),
            None,
            color::wb::Illuminant::default(),
            Some((&coverage, 0)),
        );
        let _ = stage.apply(chart(32, 24));
        assert!(
            coverage.take_built(0).is_some(),
            "the stage built a coverage map and did not report it, so nothing can cache it"
        );
    }
```

⚠ `chart(32, 24)` is this module's existing fixture helper — confirm its name and signature with
`grep -n "fn chart" crates/pipeline/src/stages/mask.rs`. If it takes no arguments, use it as-is.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p pipeline --lib reports_the_coverage_it_builds 2>&1 | tail -12
```

Expected: **compile error**, `this function takes 6 arguments but 7 were supplied`.

- [ ] **Step 3: Add the parameter**

In `crates/pipeline/src/stages/mask.rs`, add a field to `MaskStage`:

```rust
    /// Where to report coverage this stage had to build, and at which position.
    ///
    /// ⚠ `None` for every caller that hands a prebuilt map or does not cache — the CLI, the export
    /// path and all 60 goldens. Nothing about the rendered pixels depends on it.
    deposit: Option<(std::sync::Arc<crate::BrushCoverage>, usize)>,
```

⚠ `Arc`, not a borrow: `MaskStage` is built by `build_pipeline_split_in_region_with_brush` and
outlives the builder's scope, so a lifetime here would infect the whole stage list.

Change `new_in_region`'s signature to take
`deposit: Option<(std::sync::Arc<crate::BrushCoverage>, usize)>` as its last parameter and store it.

⚠ The test above passes `Some((&coverage, 0))`; change it to
`Some((std::sync::Arc::new(coverage), 0))` and keep a clone to assert on — or simpler, build the
coverage as an `Arc` from the start:

```rust
        let coverage = std::sync::Arc::new(crate::BrushCoverage::with_slots(1));
        // ... Some((coverage.clone(), 0)) ...
        assert!(coverage.take_built(0).is_some(), "...");
```

- [ ] **Step 4: Deposit at the build site**

In `apply`, where the `Painted { prebuilt: None }` arm builds the map (`mask.rs:270`), bind it and
report it:

```rust
                let built = std::sync::Arc::new(crate::brush::rasterize_in(
                    brush,
                    geometry,
                    fw,
                    fh,
                    clip,
                    Some(&view),
                ));
                // ⚠ Reported, not cached here. This stage holds no cache and does no lookup: what
                // identifies a base lives beside `PrefixKey`, in `render_engine`. See the
                // coverage-cache spec's §4.0.
                if let Some((sink, i)) = &self.deposit {
                    sink.deposit(*i, std::sync::Arc::clone(&built));
                }
                Some(built)
```

- [ ] **Step 5: Fix every other call site**

The compiler names them: `build_pipeline_split_in_region_with_brush` and its siblings, plus tests.
Pass `None` everywhere except the one place Task 3 wires up.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p pipeline --lib reports_the_coverage_it_builds 2>&1 | tail -6
cargo test -p pipeline 2>&1 | grep "test result" | tail -3
```

Expected: `1 passed`, and every pre-existing `pipeline` test still passing.

- [ ] **Step 7: Sabotage-verify**

Copy the file, then delete the `if let Some((sink, i))` block. The test must fail with *"built a
coverage map and did not report it"*. Restore from the copy.

- [ ] **Step 8: Gate and commit**

⚠ Confirm `grep -c "^test golden_.* ok$" /tmp/t.log` is **60**. Touching `MaskStage::apply` is
exactly where a golden would move if something slipped.

```bash
git add -u
git commit -F - <<'MSG'
MaskStage reports the coverage it builds

An auto-masked brush can only be rasterized where the image is, which is
inside apply. It now reports what it built so render_engine can keep it.

The stage holds no cache and does no lookup: what identifies a base lives
beside PrefixKey, in the app. This is one Option field and one write.

None for every caller that hands a prebuilt map or does not cache -- the
CLI, the export path and all 60 goldens -- so nothing about the rendered
pixels depends on it, which the goldens staying byte-identical is the
proof of.
MSG
```

---

## Task 3: per-mask entries, and the base identity

**Why:** one entry per render would invalidate mask 0's own coverage when mask 0's slider moved —
which is the case this exists to make free. Spec §4.1 and §4.2.

**Files:**
- Modify: `crates/app/src/render_engine.rs:755-873` (`BrushKey`, `BrushCache`), `:991` (the call)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/render_engine.rs`:

```rust
    /// An auto-masked brush mask, plus a global exposure so the two can be moved independently.
    fn auto_masked_edits(exposure: f32, mask_exposure: f32) -> Edits {
        let mut brush = edit_model::Brush::default();
        brush.strokes.push(edit_model::BrushStroke {
            points: vec![[0.5, 0.5]],
            radius: 0.3,
            hardness: 50.0,
            flow: 100.0,
            erase: false,
            auto_mask: true,
            tolerance: 35.0,
        });
        let mut m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(brush),
            adjustments: Default::default(),
        };
        m.adjustments.exposure = mask_exposure;
        Edits {
            exposure,
            masks: vec![m],
            ..Edits::default()
        }
    }

    /// ⚠⚠ **The load-bearing pair, and neither half alone means anything.** A test of the miss
    /// alone passes against a cache that never hits; a test of the hit alone passes against one
    /// that never misses.
    #[test]
    fn the_gating_coverage_survives_the_masks_own_slider_and_not_a_global_one() {
        let mut cache = BrushCache::default();
        let base = auto_masked_edits(0.0, 0.0);

        // Seed: nothing prebuilt on a cold cache, and the stage would report one.
        let cold = cache.get_or_build(&base, 64, 48, None, 1600);
        assert!(
            cold.get(0).is_none(),
            "fixture check: a cold cache must prebuild nothing, or the assertions below are about \
             the wrong thing"
        );
        cache.remember(&base, 64, 48, None, 1600, 0, std::sync::Arc::new(a_coverage_map()));

        // The mask's OWN adjustment moves: its base is the output after the masks BEFORE it, so
        // this cannot change it.
        let own = auto_masked_edits(0.0, 1.5);
        assert!(
            cache.get_or_build(&own, 64, 48, None, 1600).get(0).is_some(),
            "moving the mask's own slider must not invalidate its coverage — that is the case a \
             mask exists for, and the whole point of this cache"
        );

        // A GLOBAL adjustment moves: the masks loop runs after the entire global tail, so the base
        // really did change and the coverage must be rebuilt.
        let global = auto_masked_edits(1.5, 0.0);
        assert!(
            cache.get_or_build(&global, 64, 48, None, 1600).get(0).is_none(),
            "a global slider changes the base, so serving the old coverage would render the mask \
             against pixels it was never gated on"
        );
    }

    /// ⚠ The rung is in the base identity: the base is a different image at a different resolution.
    #[test]
    fn the_gating_coverage_does_not_cross_rungs() {
        let mut cache = BrushCache::default();
        let e = auto_masked_edits(0.0, 0.0);
        cache.remember(&e, 64, 48, None, 1600, 0, std::sync::Arc::new(a_coverage_map()));
        assert!(
            cache.get_or_build(&e, 64, 48, None, 3200).get(0).is_none(),
            "coverage built at one rung must not be served at another"
        );
    }

    /// A coverage map distinguishable from a default one, so a hit can be told from a miss.
    fn a_coverage_map() -> pipeline::brush::CoverageMap {
        pipeline::brush::CoverageMap {
            outside: 42,
            ..Default::default()
        }
    }
```

⚠ Confirm `CoverageMap`'s field names before writing this, with
`grep -n "pub struct CoverageMap" -A 12 crates/pipeline/src/brush/raster.rs`. Use what it declares.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib the_gating_coverage 2>&1 | tail -12
```

Expected: **compile error** — `get_or_build` takes 4 arguments, and `remember` does not exist.

⚠ `--lib`, not `--bin Rustroom`: `render_engine` is a **lib** module. A filter against the wrong
target returns `0 passed; N filtered out` and reads as green.

- [ ] **Step 3: Restructure the cache**

Replace `BrushKey` and `BrushCache` in `crates/app/src/render_engine.rs` with per-mask entries:

```rust
/// What one mask's cached coverage was built from.
#[derive(PartialEq)]
struct MaskCoverageKey {
    /// This mask's brush. `None` for an analytic shape, which is never cached here.
    brush: Option<edit_model::Brush>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    /// The rendered region as `(x0, y0, w, h)`, or `None` for the whole frame.
    ///
    /// ⚠⚠ **In the key on purpose, and the OPPOSITE of `PrefixCache`'s rule** — see the note this
    /// replaces. Whole-frame rasterization is 21.62 ms against 12.32 clipped for a plain brush and
    /// **80.81 against 42.55** for a gating one, measured; and for a gating brush a whole-frame map
    /// cannot be built during a region render at all, because the gate reads base pixels the
    /// region render never computes. **Do not "unify" the two caches.**
    region: Option<(usize, usize, usize, usize)>,
    /// What produced the pixels this mask's gate compared against, or `None` for a brush that does
    /// not gate.
    ///
    /// ⚠⚠ **`Edits` truncated to the masks BEFORE this one, plus the rung.** Sound because
    /// everything upstream of this mask's base is in it: the prefix stages all live in `Edits`, so
    /// does the entire global tail, and so does every earlier mask's shape and adjustments. And the
    /// truncation is what makes the cache worth having — dropping this mask drops its own
    /// adjustments with it, so moving its exposure leaves the identity unchanged.
    ///
    /// ⚠ It fails toward a **miss**, `PrefixKey`'s rule. The dangerous direction is a stale hit:
    /// coverage rendered positionally while the canvas tint shows it stopping at an edge, which is
    /// the silent preview/export divergence this project has recorded more than any other.
    base: Option<(Edits, usize)>,
}

/// Rasterized brush coverage, per mask, so an unrelated slider drag does not re-rasterize.
#[derive(Default)]
struct BrushCache {
    entries: Vec<(MaskCoverageKey, std::sync::Arc<pipeline::brush::CoverageMap>)>,
}

impl BrushCache {
    /// Two entries per mask, and the reason 2 is unchanged: two rungs are live at once, so one
    /// entry per mask would be evicted on every zoom change and rebuilt coming back.
    fn cap(masks: usize) -> usize {
        2 * masks.max(1)
    }

    /// The base identity for mask `i`, or `None` when this brush does not gate.
    fn base_id(edits: &Edits, i: usize, brush: &edit_model::Brush, max_dim: usize) -> Option<(Edits, usize)> {
        if !brush.needs_base() {
            return None;
        }
        let mut upstream = edits.clone();
        upstream.masks.truncate(i);
        Some((upstream, max_dim))
    }

    fn key(
        edits: &Edits,
        i: usize,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
        max_dim: usize,
    ) -> MaskCoverageKey {
        let brush = match edits.masks.get(i).map(|m| &m.mask) {
            Some(edit_model::Mask::Brush(b)) => Some(b.clone()),
            _ => None,
        };
        let base = brush
            .as_ref()
            .and_then(|b| Self::base_id(edits, i, b, max_dim));
        MaskCoverageKey {
            brush,
            geometry: edits.geometry,
            fw,
            fh,
            region,
            base,
        }
    }

    /// Store coverage a stage reported for mask `i`.
    fn remember(
        &mut self,
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
        max_dim: usize,
        i: usize,
        map: std::sync::Arc<pipeline::brush::CoverageMap>,
    ) {
        let key = Self::key(edits, i, fw, fh, region, max_dim);
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, map));
        self.entries.truncate(Self::cap(edits.masks.len()));
    }

    fn get_or_build(
        &mut self,
        edits: &Edits,
        fw: usize,
        fh: usize,
        region: Option<(usize, usize, usize, usize)>,
        max_dim: usize,
    ) -> std::sync::Arc<pipeline::BrushCoverage> {
        let mut maps = Vec::with_capacity(edits.masks.len());
        for i in 0..edits.masks.len() {
            let key = Self::key(edits, i, fw, fh, region, max_dim);
            if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
                // Promote the hit, so the least recently used entry is evicted first.
                let hit = self.entries.remove(pos);
                maps.push(Some(std::sync::Arc::clone(&hit.1)));
                self.entries.insert(0, hit);
                continue;
            }
            // ⚠⚠ A gating brush is **not built here** and must not be: its coverage depends on
            // pixel values, and this function has no image. `MaskStage::apply` builds it where the
            // buffer is and reports it back through `BrushCoverage::deposit`; `render_cached`
            // stores it with `remember`. See the coverage-cache spec's §4.0.
            let built = match edits.masks.get(i).map(|m| &m.mask) {
                Some(edit_model::Mask::Brush(b)) if !b.needs_base() => {
                    Some(std::sync::Arc::new(pipeline::brush::rasterize_in(
                        b,
                        &edits.geometry,
                        fw,
                        fh,
                        region,
                        None,
                    )))
                }
                _ => None,
            };
            if let Some(m) = &built {
                self.entries
                    .insert(0, (key, std::sync::Arc::clone(m)));
                self.entries.truncate(Self::cap(edits.masks.len()));
            }
            maps.push(built);
        }
        let n = maps.len();
        std::sync::Arc::new(pipeline::BrushCoverage {
            maps,
            built: (0..n).map(|_| std::sync::Mutex::new(None)).collect(),
        })
    }
}
```

- [ ] **Step 4: Wire it into `render_cached`**

At `crates/app/src/render_engine.rs:991`, pass the rung and collect the deposits after the run.
`get_or_build` gains `max_dim`; after `build_tail(..).run(input)` returns, harvest:

```rust
    // ⚠ After the run, not before: the deposits are what the stages built during it. Stored under
    // a key computed for *this* render, which is why `take_built` clears the slot.
    for i in 0..edits.masks.len() {
        if let Some(map) = coverage.take_built(i) {
            brush_cache.remember(edits, pw, ph, clip, max_dim, i, map);
        }
    }
```

⚠ Place this after **every** path that runs a tail — the proofed path builds two tails, and the
grade-view path splits it in halves. Both deposit the same map, so a second harvest is a no-op once
the slot is cleared.

⚠ `MaskStage`'s `deposit` argument must be threaded from
`build_pipeline_split_in_region_with_brush`, which already receives `Some(&coverage)`. Give it the
`Arc<BrushCoverage>` and the mask index.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p app --lib the_gating_coverage_survives 2>&1 | tail -8
cargo test -p app --lib the_gating_coverage_does_not_cross_rungs 2>&1 | tail -6
```

Expected: `1 passed` each.

- [ ] **Step 6: The four pre-existing cache tests must pass unchanged**

```bash
cargo test -p app --lib brush_cache 2>&1 | tail -10
```

Expected: `the_brush_cache_hits_and_keeps_both_zoom_levels`,
`changing_a_stroke_misses_the_brush_cache`, `the_brush_cache_misses_when_the_region_moves` and
`the_brush_cache_still_hits_when_only_a_slider_moves` all passing, **with their assertions
untouched**. ⚠ They describe a plain brush's behaviour, which this must not move; their passing
unchanged is the evidence it did not. If one needs its assertion changed, stop — that is a
behaviour change, not a refactor.

- [ ] **Step 7: Invert the declining test, do not delete it**

`the_brush_cache_declines_a_brush_that_needs_the_image` asserted the old behaviour. Replace it with:

```rust
    /// ⚠⚠ **This inverts a test rather than deleting one, and says what it used to assert.** It
    /// used to pin that `BrushCache` *declines* a gating brush — correct while nothing could
    /// identify the base. It now pins the half that still holds: the cache never **builds** one,
    /// because it has no image. It stores what `MaskStage` reports.
    ///
    /// ⚠ An inversion that changes only the assertions leaves a name and a comment that lie — the
    /// spot-removal render rule was inverted once and its comment was found still describing the
    /// old behaviour a round later.
    #[test]
    fn the_brush_cache_never_builds_a_brush_that_needs_the_image() {
        let mut cache = BrushCache::default();
        let e = auto_masked_edits(0.0, 0.0);
        assert!(
            cache.get_or_build(&e, 64, 48, None, 1600).get(0).is_none(),
            "a gating brush's coverage depends on pixel values, and this function has no image — \
             building one here would render it positionally"
        );
    }
```

- [ ] **Step 8: Sabotage-verify, twice**

Copy `render_engine.rs` first.

1. Make `base_id` return `None` for every brush (`return None;` at the top). **The global-slider
   half must fail** — a stale base would be served.
2. Make `base_id` truncate to `i + 1` instead of `i`, i.e. include this mask. **The own-slider half
   must fail**, since moving the mask's own exposure would then invalidate. That is the sabotage
   that proves the truncation is what buys the win.

Restore from the copy after each and check `git status`.

- [ ] **Step 9: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
Cache an auto-masked brush's coverage, keyed on what produced its base

Measured at 42.55 ms per render on a 1.54 Mpx region at 1:1 -- about 65% of
a masked render, against a plain brush's 12.32.

Entries are per mask, which the win depends on: one entry per render would
invalidate mask 0's own coverage when mask 0's slider moved, which is
exactly the case this exists to make free.

The base identity is Edits truncated to the masks BEFORE this one, plus the
rung. Sound because everything upstream is in it -- the prefix stages all
live in Edits, so does the whole global tail, so does every earlier mask.
And the truncation is what buys the win: dropping this mask drops its own
adjustments, so moving its exposure leaves the identity unchanged.

⚠ The cache never BUILDS a gating brush's coverage and must not: it has no
image. MaskStage builds it where the buffer is and reports it; this stores
what was reported. the_brush_cache_declines_a_brush_that_needs_the_image is
inverted rather than deleted, and says what it used to assert.

⚠ It fails toward a miss. The dangerous direction is a stale hit -- coverage
rendered positionally while the tint shows it stopping at an edge.
MSG
```

---

## Task 4: prove a stale hit shows as wrong pixels

**Why:** every test so far asserts on presence, and a wrong-but-present map is the failure that
matters. Spec §5.

**Files:**
- Modify: `crates/app/src/render_engine.rs` — `every_cached_frame_matches_a_cold_render`

- [ ] **Step 1: Extend the master invariant**

Find it with `grep -n "fn every_cached_frame_matches_a_cold_render" crates/app/src/render_engine.rs`
and add an auto-masked brush to the sequence it renders, alongside the existing edits. Add to its
doc comment:

```rust
    /// ⚠⚠ An **auto-masked brush** is in the sequence because the coverage cache's key models what
    /// produced this mask's base. Every other test of that key asserts on *presence*; a key that
    /// admits a stale base produces a map that is present and wrong, and only comparing pixels
    /// against a cold render can see it.
```

- [ ] **Step 2: Run it**

```bash
cargo test -p app --lib every_cached_frame_matches_a_cold_render 2>&1 | tail -6
```

Expected: `1 passed`.

- [ ] **Step 3: Sabotage-verify**

Copy the file. Make `base_id` return a constant `Some((Edits::default(), 0))` for every mask, so
every gating brush hits regardless of what changed upstream. The test must fail **on pixel values**,
not on a count. Restore from the copy.

⚠ If it passes, the sequence does not change the base between renders — extend it with a global
exposure change and say so in the comment. A test that cannot see a stale base proves nothing here.

- [ ] **Step 4: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
The master invariant covers an auto-masked brush

Every other test of the coverage key asserts on presence. A key that admits
a stale base produces a map that is present and wrong, and only comparing
pixels against a cold render can see that.

Sabotaged with a constant base identity, so every gating brush hits
whatever changed upstream: it fails on pixel values, which is the failure
this is for.
MSG
```

---

## Task 5: measure the win, write it up, and use it

**Files:**
- Modify: `crates/app/examples/bench_auto_mask_cov.rs`, `CLAUDE.md`

- [ ] **Step 1: Measure a warm render**

Extend `bench_auto_mask_cov` with a second section that renders the same region twice through a
`RenderEngine`-style path and reports both, so the before/after is one table rather than two runs.
⚠ Report the **ratio**, since `bench_masked`'s own note applies: a synthetic bench prices a stage,
not a session, and it understated the running app by 2.6× once.

- [ ] **Step 2: Confirm it in the running app**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app
```

Drag a **masked adjustment slider** at 1:1 with an auto-mask brush and read the per-render times.
Expect the first frame of the drag to cost coverage and the rest not to. ⚠ Compare against the
recorded baseline: **19–156 ms for a 1.7 Mpx masked region**.

- [ ] **Step 3: The GUI checklist to put in front of the user**

1. **An auto-mask brush, drag its Exposure at 1:1.** Should be markedly smoother than before.
2. **⚠⚠ Then drag a GLOBAL slider.** It should be *unchanged* — no cache can help it, and if it
   looks faster something is serving a stale base.
3. **Paint a new stroke.** The strokes changed, so coverage must rebuild; it must not show the old
   shape for a frame.
4. **Zoom between fit and 1:1 and back.** Two rungs are live, and the cap exists so returning is a
   hit.
5. **⚠ Watch for the failure that matters:** a mask that renders as a plain positional stroke while
   the tint shows it stopping at an edge. That is a stale hit and it is the one thing this must
   never do.

- [ ] **Step 4: Write the CLAUDE.md section**

A section titled **"The auto-mask coverage cache"** below the most recent entry, recording: the
measurement (42.55 vs 12.32 ms clipped, 30.24 ms of gate, ~65% of a masked render) and the after
figure; that **the cache never builds a gating brush's coverage** and why; that the base identity is
truncated `Edits` and that the truncation is what buys the win; that **panning and global slider
drags are not helped, structurally**; and the caveat list from spec §6.

⚠ `grep -n "Co-Authored-By" CLAUDE.md docs/superpowers/plans/2026-09-09-auto-mask-coverage-cache.md`
must return only prohibitions before committing.

- [ ] **Step 5: Gate and commit**

---

## Self-review

**Spec coverage.** §2's measurement → Task 5 confirms it after. §3's structural limit → recorded in
`MaskCoverageKey::region`'s comment (Task 3) and in the write-up (Task 5). §4.0's correction →
Tasks 1 and 2. §4.1 per-mask entries → Task 3. §4.2 the base identity → Task 3, with both sabotages.
§4.3 failure direction → Task 4. §4.4 capacity → `BrushCache::cap` in Task 3. §5 testing →
distributed, with the goldens asserted at every gate. §6 caveats → Task 5's write-up.

**Placeholder scan.** No "TBD" or "handle edge cases". Four steps say "confirm the shape first" and
give the exact `grep` — that is a verification instruction with a command, and it is there because
the previous plan guessed `MaskedAdjustment.adjust` when the field is `adjustments`.

**Type consistency.** `BrushCoverage::with_slots` / `deposit` / `take_built` are defined in Task 1
and used in Tasks 2 and 3. `MaskCoverageKey` replaces `BrushKey` throughout Task 3.
`BrushCache::remember` / `get_or_build` / `cap` / `base_id` / `key` are each defined once.
`MaskStage::new_in_region` gains one parameter in Task 2 and is called with it in Task 3.

**Known risk, stated rather than hidden.** Task 3's harvest must run after *every* path that runs a
tail — the proof path builds two and the grade-view path splits it. If a path is missed the cache
simply never fills for that case: a silent loss of the win, not a wrong pixel. `take_built` clearing
the slot is what keeps a second harvest harmless.

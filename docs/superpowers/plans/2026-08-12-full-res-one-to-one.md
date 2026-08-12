# A full-resolution buffer for true 1:1 — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Rustroom's 1:1 view show the real, demosaiced, full-resolution pixels instead of a half-size binned buffer — decoded in the background, triggered by zoom, without slowing the fit-resolution path.

**Architecture:** Three render tiers (Fit / NativeHalf / NativeFull) replace today's two, keeping on-screen scale inside (0.5, 1.0] in every native tier. The half-size buffer and its `RenderEngine` are untouched and keep serving Fit and NativeHalf; a second engine over a full-resolution buffer serves NativeFull. The decode reuses `DecodeWorker`. Two cache constants in `render_engine.rs` are the only change to that file.

**Tech Stack:** Rust, `egui`/`eframe`, `rayon` (inside the stages), `cargo test` / `cargo clippy --all-targets --all-features` / `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-12-full-res-one-to-one-design.md`

**Build setup — every `cargo` command below assumes this has been exported in the shell:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/view_region.rs` | `RenderTier`, `tier_for`, `clamped` | Modify (add above `needs_native`, plus tests) |
| `crates/app/src/render_engine.rs` | cache budget + count cap | Modify (2 consts, `insert`, tests) |
| `crates/app/src/full_res.rs` | **new** — `wants_upgrade`, `native_long` | Create |
| `crates/app/src/lib.rs` | export the new module | Modify (1 line) |
| `crates/app/src/main.rs` | `FullRes`, second worker, tier plumbing, handover | Modify |
| `CLAUDE.md` | durable status | Modify |

`full_res.rs` is a separate file rather than more of `main.rs` for the reason `view_region` exists: a binary crate's private logic is awkward to test, and this decision ("should we spend three seconds and 313 MB now?") must not be verifiable only by zooming in and waiting.

---

## Task 1: Three render tiers

**Files:**
- Modify: `crates/app/src/view_region.rs` (add above `needs_native`, plus tests)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/app/src/view_region.rs`:

```rust
    #[test]
    fn the_tier_boundaries_follow_the_buffer_ratio() {
        // `r` is the half-size buffer's long edge over the fit render's — ~1.95 on an X-T4.
        let r = 1.952;
        // At and below 1.0 the fit texture still has the pixels.
        assert_eq!(tier_for(0.5, 1.0, r), RenderTier::Fit);
        assert_eq!(tier_for(1.0, 1.0, r), RenderTier::Fit);
        // Just past it, only the half-size buffer does.
        assert_eq!(tier_for(1.0, 1.001, r), RenderTier::NativeHalf);
        assert_eq!(tier_for(1.0, r, r), RenderTier::NativeHalf);
        // Past the half buffer's own resolution, only a full decode does.
        assert_eq!(tier_for(1.0, r + 0.01, r), RenderTier::NativeFull);
        assert_eq!(tier_for(1.0, 8.0, r), RenderTier::NativeFull);
    }

    #[test]
    fn without_a_full_buffer_the_top_tier_falls_back() {
        // The view must keep working at today's fidelity while the decode runs, and if it
        // fails. `tier_for` says what the zoom *wants*; `clamped` says what is available.
        assert_eq!(
            tier_for(1.0, 8.0, 1.952).clamped(false),
            RenderTier::NativeHalf
        );
        assert_eq!(
            tier_for(1.0, 8.0, 1.952).clamped(true),
            RenderTier::NativeFull
        );
        // Clamping never promotes, and never touches the lower tiers.
        assert_eq!(tier_for(0.5, 1.0, 1.952).clamped(false), RenderTier::Fit);
        assert_eq!(tier_for(1.0, 1.5, 1.952).clamped(false), RenderTier::NativeHalf);
    }

    #[test]
    fn a_native_tier_never_magnifies_its_own_texture() {
        // ⚠ The invariant the three tiers exist for, and the reason a third tier was needed
        // rather than swapping the second one's buffer. `k` is rendered pixels per fit pixel,
        // so on-screen scale is fit*zoom/k. Inside a native tier it must never exceed 1.0
        // (magnifying a texture that has more pixels available) nor fall to 0.5 (rendering
        // more than about twice what is displayed — which is what made the two-tier version
        // with a full-res buffer cost ~180 ms a frame instead of 13).
        let r = 1.952_f32; // half buffer / fit
        let full_r = 2.0 * r; // full buffer / fit
        let mut z = 1.001_f32;
        while z <= full_r {
            let tier = tier_for(1.0, z, r);
            let k = match tier {
                RenderTier::Fit => 1.0,
                RenderTier::NativeHalf => r,
                RenderTier::NativeFull => full_r,
            };
            let scale = z / k;
            assert!(
                scale <= 1.0 + 1e-6,
                "zoom {z} magnifies its own texture (scale {scale})"
            );
            assert!(
                scale > 0.5 - 1e-6,
                "zoom {z} renders {:.1}x the displayed pixels",
                1.0 / scale
            );
            z += 0.01;
        }
    }

    #[test]
    fn is_native_matches_the_old_two_way_split() {
        // `needs_native` is kept and still means "render a region rather than the whole
        // image"; the tiers must agree with it or the canvas and the cache disagree.
        let r = 1.952;
        for z in [0.4_f32, 0.99, 1.0, 1.01, 1.9, 2.5, 9.0] {
            assert_eq!(
                tier_for(1.0, z, r).is_native(),
                needs_native(1.0, z),
                "zoom {z}"
            );
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app the_tier_boundaries 2>&1 | grep -E "^error" | head -4`

Expected: FAIL to compile — `cannot find function `tier_for` in this scope`.

- [ ] **Step 3: Implement**

In `crates/app/src/view_region.rs`, insert immediately **above** the `/// Whether the view is magnifying…` doc comment on `needs_native`:

```rust
/// Which resolution the canvas renders at.
///
/// Quantized to three tiers on purpose: a `max_dim` varying continuously with zoom would
/// rebuild the prefix cache on every scroll tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTier {
    /// The whole image at preview resolution.
    Fit,
    /// The half-size editing buffer's own resolution.
    NativeHalf,
    /// A full-resolution decode — the only tier that shows real demosaiced detail.
    NativeFull,
}

impl RenderTier {
    /// Whether this tier renders a *region* rather than the whole image.
    pub fn is_native(self) -> bool {
        self != RenderTier::Fit
    }

    /// Fall back to [`RenderTier::NativeHalf`] when no full-resolution buffer exists yet, so
    /// the view keeps working at the previous fidelity while the decode runs — or for good, if
    /// it fails.
    ///
    /// Kept separate from [`tier_for`] deliberately: the upgrade decision needs to know which
    /// tier the zoom *wants*, and folding availability into `tier_for` would make that
    /// circular — the tier could never ask for a buffer that does not exist yet.
    pub fn clamped(self, full_available: bool) -> RenderTier {
        match self {
            RenderTier::NativeFull if !full_available => RenderTier::NativeHalf,
            other => other,
        }
    }
}

/// The tier the current zoom calls for, ignoring what has actually been decoded.
///
/// `fit` is the scale that makes the whole image fit the viewport and `zoom` the user's factor
/// on top, so `fit * zoom` is screen pixels per *fit-render* pixel. `half_ratio` is the
/// half-size buffer's long edge divided by the fit render's (~1.95 on an X-T4): the zoom at
/// which the NativeHalf texture is itself magnified past its own pixels — exactly the reasoning
/// that puts the Fit boundary at 1.0.
pub fn tier_for(fit: f32, zoom: f32, half_ratio: f32) -> RenderTier {
    let s = fit * zoom;
    if s <= 1.0 {
        RenderTier::Fit
    } else if s <= half_ratio {
        RenderTier::NativeHalf
    } else {
        RenderTier::NativeFull
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p app view_region 2>&1 | grep -E "^test |^test result" | head -20`

Expected: PASS, including the existing `the_region_margin_exceeds_every_tail_kernel_radius` and the `visible_rect` tests, untouched.

- [ ] **Step 5: Verify the invariant test can fail**

Temporarily change `tier_for`'s middle branch from `s <= half_ratio` to `s <= half_ratio * 2.0` — i.e. let NativeHalf stretch over the range NativeFull should own.

Run: `cargo test -p app a_native_tier_never_magnifies 2>&1 | grep -E "assertion|magnifies" | head -3`

Expected: FAIL with "magnifies its own texture". Revert and re-run to confirm PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/view_region.rs
git commit -m "feat(app): three render tiers for the canvas"
```

---

## Task 2: Cache budget and count cap

**Files:**
- Modify: `crates/app/src/render_engine.rs` (`MID_BUDGET_BYTES`, add `MIDS_CAP`, `insert`, tests)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/app/src/render_engine.rs`, next to the other cache tests:

```rust
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
        assert!(cache.get(&fake_key(19)).is_some(), "the newest must survive");
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
    fn the_budget_holds_one_full_resolution_checkpoint() {
        // The sizing decision, asserted rather than left in a comment. A 26 Mpx checkpoint is
        // ~313 MB; the budget must clear it, and must *not* clear two (which would double the
        // footprint for a rarely-used second checkpoint).
        const CHECKPOINT: usize = 6246 * 4170 * 3 * 4;
        assert!(PrefixCache::MID_BUDGET_BYTES > CHECKPOINT);
        assert!(PrefixCache::MID_BUDGET_BYTES < 2 * CHECKPOINT);
        // And it must clear a 40 Mpx body's, which is why it is 512 MiB and not 320.
        assert!(PrefixCache::MID_BUDGET_BYTES > 7728 * 5152 * 3 * 4);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app the_mids_cap 2>&1 | grep -E "^error" | head -3`

Expected: FAIL to compile — `no associated item named `MIDS_CAP` found`.

- [ ] **Step 3: Implement**

In `crates/app/src/render_engine.rs`, replace the `MID_BUDGET_BYTES` const and its doc comment with:

```rust
    /// Ceiling on the bytes held as intermediates.
    ///
    /// 512 MiB clears one full-resolution checkpoint (~313 MB on a 26 Mpx X-T4) but not two,
    /// which is the intent: the post-NR checkpoint makes a Dehaze drag at 1:1 reuse noise
    /// reduction, and a second checkpoint would double the footprint to save ~200 ms on a
    /// rarer interaction. ⚠ 512 rather than 320 MiB because 320 covers only 28.0 Mpx, and
    /// Fujifilm's current 40.2 Mpx bodies would get *zero* native checkpoints; 512 covers
    /// 44.7 Mpx. On a larger sensor than that, no intermediate survives and behaviour degrades
    /// to re-running the whole prefix — the ceiling working, not failing.
    const MID_BUDGET_BYTES: usize = 512 << 20;

    /// Ceiling on the *number* of intermediates, independent of their size.
    ///
    /// ⚠ This is what makes a large byte budget safe. Dragging Straighten mints a new
    /// post-geometry key every frame, so without a count cap the tier fills with stale
    /// checkpoints bounded only by bytes — and a bigger budget would simply retain more
    /// garbage. A render's useful chain is at most 3 mids, so 6 keeps the current chain plus
    /// one previous state and discards the rest.
    const MIDS_CAP: usize = 6;
```

Then in `insert`'s `Tier::Mid` arm, add the count cap before the byte loop:

```rust
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
```

- [ ] **Step 4: Run everything**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test --all 2>&1 | grep -c "test result: ok"`

Expected: no failures, 21 `test result: ok` groups. In particular `every_cached_frame_matches_a_cold_render` and `a_change_reruns_only_what_is_downstream_of_it` must still pass — a cap of 6 is well above the 3 a chain needs, so nothing about the checkpoint feature changes.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): cap the mids tier by count as well as bytes"
```

---

## Task 3: The upgrade decision

**Files:**
- Create: `crates/app/src/full_res.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Create the module with its tests**

Create `crates/app/src/full_res.rs`:

```rust
//! When to upgrade the editing buffer to a full-resolution decode, and which long edge the
//! native tiers render at.
//!
//! Pure and `egui`-free on purpose: the alternative is a rule buried in the frame loop that
//! can only be checked by zooming in and waiting three seconds for the wrong answer.

use crate::view_region::RenderTier;

/// Whether to start a full-resolution decode now.
///
/// `wanted` is the tier the current zoom calls for — [`crate::view_region::tier_for`]'s answer
/// *before* clamping, since a clamped tier could never ask for a buffer that does not exist.
///
/// ⚠ `is_raw` is not a detail. JPEG, PNG and TIFF are already decoded at full size with
/// `source_scale` 1.0, so their 1:1 view is already honest and an "upgrade" would spend ~3 s
/// re-decoding the very same pixels.
pub fn wants_upgrade(is_raw: bool, wanted: RenderTier, has_full: bool, in_flight: bool) -> bool {
    is_raw && wanted == RenderTier::NativeFull && !has_full && !in_flight
}

/// The long edge to render a native tier at: the full-resolution buffer's when one has been
/// decoded, the editing buffer's otherwise.
///
/// An absent or failed full decode therefore degrades to exactly the previous behaviour.
pub fn native_long(buffer_long: usize, full_long: Option<usize>) -> usize {
    full_long.unwrap_or(buffer_long)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_raw_wants_an_upgrade() {
        // A full decode of a JPEG would produce the buffer we already have.
        assert!(wants_upgrade(true, RenderTier::NativeFull, false, false));
        assert!(!wants_upgrade(
            false,
            RenderTier::NativeFull,
            false,
            false
        ));
    }

    #[test]
    fn only_the_top_tier_wants_an_upgrade() {
        // Below NativeFull the half-size buffer still has every pixel the screen can show, so
        // spending 3 s and 313 MB would buy nothing.
        for tier in [RenderTier::Fit, RenderTier::NativeHalf] {
            assert!(!wants_upgrade(true, tier, false, false), "{tier:?}");
        }
    }

    #[test]
    fn an_upgrade_is_not_requested_twice() {
        // Neither while one is in flight...
        assert!(!wants_upgrade(true, RenderTier::NativeFull, false, true));
        // ...nor once one has landed. Without this the frame loop would fire a fresh 3 s
        // decode every frame for as long as you stayed zoomed in.
        assert!(!wants_upgrade(true, RenderTier::NativeFull, true, false));
    }

    #[test]
    fn an_absent_full_buffer_falls_back_to_native_half() {
        assert_eq!(native_long(3123, None), 3123);
        assert_eq!(native_long(3123, Some(6246)), 6246);
    }
}
```

- [ ] **Step 2: Export it**

In `crates/app/src/lib.rs`, add alongside the other `pub mod` lines, in alphabetical position:

```rust
pub mod full_res;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p app full_res 2>&1 | grep -E "^test |^test result" | head -8`

Expected: PASS, 4 tests.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/full_res.rs crates/app/src/lib.rs
git commit -m "feat(app): the full-resolution upgrade decision"
```

---

## Task 4: Wire it into the app

The largest task, and the only one touching `main.rs`. Split into small steps because the
borrow shapes here have bitten before.

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the decode function and its gated test**

In `crates/app/src/main.rs`, immediately after `decode_for_edit`, add:

```rust
/// Decode a RAW at **full** resolution, for the true-1:1 view.
///
/// ⚠ Uses `Demosaic::Fast` (whatever `linear_scene()` carries), **not** the export preset's
/// setting. Matching it would mean plumbing a parameter through `DecodeWorker`'s injected
/// decode fn; the measured Fast-vs-Best gap is small, which is why export defaults to Fast
/// too. See the design doc's caveat 4.
fn decode_full_res(path: &std::path::Path) -> Result<(core_image::ImageBuf, String, f32), String> {
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
    let label = format!("{} {}", d.metadata.make, d.metadata.model);
    Ok((d.image, label, opts.source_scale()))
}
```

And in `main.rs`'s `mod tests`, add:

```rust
    #[test]
    fn a_full_size_decode_is_twice_the_half_size_one() {
        // ⚠ Without this, a typo in `decode_full_res` would leave the whole feature inert but
        // entirely plausible-looking: a second engine, a second cache, a status line, and the
        // same binned pixels. Gated like the other real-decode tests; run with --release.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path.raw to run this test");
            return;
        };
        let p = std::path::Path::new(&path);
        let (half, _, half_scale) = decode_for_edit(p).expect("half decode");
        let (full, _, full_scale) = decode_full_res(p).expect("full decode");
        assert_eq!(half_scale, 0.5);
        assert_eq!(full_scale, 1.0);
        // libraw's half-size path halves each dimension, rounding down.
        assert!(
            full.width() >= half.width() * 2 - 2 && full.width() <= half.width() * 2 + 2,
            "full {} vs half {}",
            full.width(),
            half.width()
        );
    }
```

- [ ] **Step 2: Run the gated test against the real fixture**

Run: `RAW_FIXTURE=~/Desktop/fuji/DSCF6509.RAF cargo test --release -p app a_full_size_decode -- --nocapture 2>&1 | tail -5`

Expected: PASS (takes ~4 s — it does both decodes).

- [ ] **Step 3: Add the `FullRes` holder and the Session fields**

In `crates/app/src/main.rs`, add above `struct Session`:

```rust
/// A decoded full-resolution buffer and the engine that renders it.
///
/// A second engine rather than swapping the first one's buffer: `RenderEngine` owns its buffer
/// for its thread's lifetime, and keeping both means Fit-mode rendering is byte-for-byte
/// unchanged — in particular it never pays the 27.3 ms `downscale_to` a full-resolution source
/// would cost, against 9.1 ms from the half-size one.
struct FullRes {
    engine: RenderEngine,
    /// Long edge of the full-resolution buffer — `max_dim` for the NativeFull tier.
    long: usize,
}
```

Add to `struct Session`, after `buffer_long`:

```rust
    /// The full-resolution buffer and its engine, once one has been decoded. `None` until the
    /// user zooms past the half-size buffer's own resolution.
    full: Option<FullRes>,
    /// Whether the last request went to `full`'s engine. Frames from the *other* engine are
    /// discarded, so this must be set at request time, not derived at poll time.
    active_full: bool,
```

And in `finalize_session`, add to the `Session { .. }` literal after `buffer_long: ...`:

```rust
            full: None,
            active_full: false,
```

- [ ] **Step 4: Route every render request through the Session**

Replace `Session`'s `impl` block with:

```rust
impl Session {
    /// The working resolution to re-render at: the active native buffer's long edge while a
    /// region is being shown, the preview cap otherwise. Keeps an edit made while zoomed in
    /// from snapping back to fit.
    fn render_max_dim(&self) -> usize {
        if self.requested_region.is_none() {
            return PREVIEW_MAX_DIM;
        }
        if self.active_full {
            full_res::native_long(self.buffer_long, self.full.as_ref().map(|f| f.long))
        } else {
            self.buffer_long
        }
    }

    /// Send a render request to whichever engine owns the current tier's buffer.
    ///
    /// ⚠ Every request must go through here. Two engines mean two queues, and a request sent
    /// to the inactive one produces a frame that is then discarded — the edit would silently
    /// never appear.
    fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        match (self.active_full, &self.full) {
            (true, Some(f)) => f.engine.request(edits, max_dim, region),
            _ => self.engine.request(edits, max_dim, region),
        }
    }
}
```

Then replace **every** `s.engine.request(` call site with `s.request(`. There are six, at roughly lines 340, 585, 599, 967, 1853 and 1941. Find them all with:

```bash
grep -n "\.engine\.request(" crates/app/src/main.rs
```

Expected after the edit: only the one inside `finalize_session` remains (it acts on a local
`engine`, before the `Session` exists).

- [ ] **Step 5: Add the second decode worker**

In `crates/app/src/main.rs`, add next to `MAX_CONCURRENT_DECODES`:

```rust
/// One at a time: a full-resolution decode holds a 313 MB buffer, and two in flight would be
/// 626 MB of transient allocation for no gain.
const MAX_CONCURRENT_FULL_DECODES: usize = 1;
```

Add to the app struct, next to `decoder`:

```rust
    full_decoder: DecodeWorker,
    /// Path and sequence number of the full-resolution decode in flight, if any.
    full_pending: Option<(PathBuf, u64)>,
```

In `RustroomApp::new`, the existing `decoder` initialiser already moves a cloned `egui::Context`
into its `on_done` closure. A second worker needs a **second clone** — the first is moved. Add
one beside the existing clone, then the initialiser:

```rust
        // A second clone: the one above is moved into `decoder`'s closure.
        let ctx_for_full = ctx.clone();
```

```rust
            full_decoder: DecodeWorker::new(decode_full_res, MAX_CONCURRENT_FULL_DECODES, {
                move || ctx_for_full.request_repaint()
            }),
            full_pending: None,
```

Also add the module import at the top of `main.rs`, beside the other `rustroom_app` uses:

```rust
use rustroom_app::full_res;
```

- [ ] **Step 6: Absorb finished full-resolution decodes**

Add this method next to `pump_decodes`:

```rust
    /// Per-frame: absorb a finished full-resolution decode into the live session.
    fn pump_full_res(&mut self, ctx: &egui::Context) {
        for res in self.full_decoder.poll() {
            let is_current = self.full_pending.as_ref().map(|(_, seq)| *seq) == Some(res.seq);
            if is_current {
                self.full_pending = None;
            }
            // A result for a photo we have since navigated away from is dropped: the buffer is
            // per-session and dies with it, so there is nothing to keep it in.
            let Some(s) = self.session.as_mut() else {
                continue;
            };
            if !is_current || s.path != res.path {
                continue;
            }
            match res.outcome {
                Ok(decoded) => {
                    // ⚠ The baseline `DecodeWorker` rendered is discarded on purpose: the
                    // existing preview-resolution one stays, so before/after does not change
                    // under the user. The waste is one ~60 ms render on a background thread
                    // during a 3 s decode.
                    let ctx2 = ctx.clone();
                    let long = decoded.buffer.width().max(decoded.buffer.height());
                    let engine =
                        RenderEngine::new(decoded.buffer.clone(), decoded.source_scale, move || {
                            ctx2.request_repaint()
                        });
                    s.full = Some(FullRes { engine, long });
                    // ⚠ Load-bearing. Region rects are in *rendered* pixels, and those
                    // coordinates double when the buffer does — reusing them would describe
                    // the wrong quarter of the image. Clearing both forces the next frame to
                    // re-request against the new resolution.
                    s.rendered_region = None;
                    s.requested_region = None;
                    self.status = "Full resolution ready".to_string();
                }
                Err(e) => {
                    // NativeFull silently falls back to NativeHalf, i.e. today's behaviour.
                    self.status = format!("Full-resolution decode failed: {e}");
                }
            }
        }
    }
```

And call it in `ui()` immediately after `self.pump_decodes(&ctx);`:

```rust
        self.pump_full_res(&ctx);
```

- [ ] **Step 7: Choose the tier in the canvas**

In `crates/app/src/main.rs`, replace the block that begins `let native = view_region::needs_native(fit, s.zoom) && !s.crop_mode;` and runs down to the `s.engine.request(...)` call (roughly lines 1899–1943) with:

```rust
                        // Crop mode stays at fit resolution whatever the zoom: it is a framing
                        // operation, the overlay is drawn against the whole image, and a
                        // whole-image *native* render would be both expensive and placed by
                        // the fit-mode maths the overlay assumes.
                        let full_long = full.x.max(full.y);
                        let half_ratio = s.buffer_long as f32 / full_long;
                        let wanted = if s.crop_mode {
                            view_region::RenderTier::Fit
                        } else {
                            view_region::tier_for(fit, s.zoom, half_ratio)
                        };
                        // Start the 3 s decode the first time the zoom asks for detail the
                        // half-size buffer does not have. Recorded as a flag and acted on
                        // after the panels close: `s` borrows `self.session`, and requesting
                        // borrows `self.full_decoder`.
                        //
                        // ⚠ `upgrade_in_flight` is read from a local hoisted *before* the panel
                        // closure, not from `self` here: `s` already holds a borrow of `self`,
                        // and reaching for `self.full_pending` inside the closure is the same
                        // borrow shape that has cost a compile error twice in this file.
                        want_full_res = full_res::wants_upgrade(
                            catalog::is_raw(&s.path),
                            wanted,
                            s.full.is_some(),
                            upgrade_in_flight,
                        );
                        let tier = wanted.clamped(s.full.is_some());
                        let native = tier.is_native();

                        // Dimensions of the image at the resolution we render it, and the
                        // screen pixels per rendered pixel.
                        let render_long = match tier {
                            view_region::RenderTier::Fit => full_long,
                            view_region::RenderTier::NativeHalf => s.buffer_long as f32,
                            view_region::RenderTier::NativeFull => {
                                s.full.as_ref().map_or(s.buffer_long, |f| f.long) as f32
                            }
                        };
                        let k = render_long / full_long;
                        let (iw, ih) =
                            ((full.x * k).round() as usize, (full.y * k).round() as usize);
                        let scale = fit * s.zoom / k;

                        // Which part of the image the view needs, and whether what is already
                        // rendered still covers it.
                        let want = if native {
                            let vis = view_region::visible_rect(
                                iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y,
                            );
                            Some(view_region::with_margin(
                                vis,
                                iw,
                                ih,
                                view_region::REGION_MARGIN,
                            ))
                        } else {
                            None
                        };
                        let want_full_engine = tier == view_region::RenderTier::NativeFull;
                        let covered = match (want, s.rendered_region) {
                            (None, None) => true,
                            (Some(w), Some(r)) => view_region::contains(r, w),
                            _ => false,
                        };
                        // ⚠ A change of *engine* must re-request even when the region is
                        // unchanged: frames from the inactive engine are discarded, so
                        // without this the view would freeze on the old buffer's last frame.
                        if (!covered && s.requested_region != want)
                            || want_full_engine != s.active_full
                        {
                            s.requested_region = want;
                            s.active_full = want_full_engine;
                            let max_dim = render_long.round() as usize;
                            s.request(preview_edits(&s.edits, s.crop_mode), max_dim, want);
                        }
```

Declare the flag **and the hoisted in-flight check** next to the existing `photo_edited` local
in `develop_body`, before any panel closure:

```rust
        let mut want_full_res = false;
        // Read before the panels borrow `self.session`, so the canvas closure needs no second
        // borrow of `self`.
        let upgrade_in_flight = self.full_pending.is_some();
```

and act on it after the panels close, next to where `photo_edited` is consumed:

```rust
        if want_full_res {
            if let Some(s) = &self.session {
                let path = s.path.clone();
                let seq = self.full_decoder.request(path.clone(), PREVIEW_MAX_DIM);
                self.full_pending = Some((path, seq));
                self.status = "Loading full resolution…".to_string();
            }
        }
```

- [ ] **Step 8: Poll both engines**

Replace the `if let Some(frame) = s.engine.poll_latest() {` line (around 2218) and its
surrounding `if let Some(s) = &mut self.session {` with:

```rust
        if let Some(s) = &mut self.session {
            // Drain *both* engines every frame, then apply only the active one's newest frame.
            // Draining both matters: an undrained channel would accumulate whole 313 MB
            // frames. Discarding the inactive one's is safe because any change of active
            // engine re-requests — see `active_full` in the canvas.
            let from_full = s.full.as_ref().and_then(|f| f.engine.poll_latest());
            let from_half = s.engine.poll_latest();
            let frame = if s.active_full { from_full } else { from_half };
            if let Some(frame) = frame {
```

The existing body (histogram, texture upload, `rendered_region`, `fit_image_size`,
`rendered_long`) is unchanged. Note this adds **one nesting level**, so the block now needs a
second closing brace before `// Pull finished thumbnails into textures.` — `cargo fmt` will
re-indent the body once the braces balance.

- [ ] **Step 9: Build and check**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head -20`

Expected: clean. If the borrow checker objects to `self.full_pending.is_some()` inside the
closure that holds `s`, hoist it to a local `let upgrade_in_flight = self.full_pending.is_some();`
computed *before* the panel closure — the same shape the existing code uses for `photo_edited`.

- [ ] **Step 10: Run the whole suite**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test --all 2>&1 | grep -c "test result: ok"`

Expected: no failures and **21** `test result: ok` groups — unchanged, because `full_res` is a
new module inside the existing `app` lib crate rather than a new test binary.

- [ ] **Step 11: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): render 1:1 against a full-resolution decode"
```

---

## Task 5: Verify in the GUI

- [ ] **Step 1: Launch**

Run: `cargo run --release -p app`

- [ ] **Step 2: Check by hand**

Open a RAW from `~/Desktop/fuji/`. Then:

1. **Zoom in slowly past fit.** Nothing should change at first (NativeHalf, today's behaviour).
   Keep going past roughly 2× fit — the status line should read "Loading full resolution…",
   and about 3 seconds later the image should visibly **sharpen** and the status change to
   "Full resolution ready". That sharpening is the whole feature.
2. **Pan around at that zoom.** Should stay smooth; no seams at region edges.
3. **Drag Exposure while zoomed in.** Should stay responsive (~13 ms/frame).
4. **Drag Noise Reduction while zoomed in.** Will be slow (~0.5 s/frame) — expected, and the
   reason the checkpoint budget was raised. Confirm it is slow but not *broken*.
5. **Zoom back out to fit and edit.** Must be exactly as fast as before this change — the fit
   path still uses the half-size buffer.
6. **Switch to another photo and back, then zoom in again.** A second 3 s decode is expected:
   the full-resolution buffer is not cached across sessions.
7. **Open a JPEG and zoom in.** No decode should be triggered at all; the status line stays
   quiet.
8. **Watch memory** in Activity Monitor while zoomed in with NR and dehaze on. Expect roughly
   1.3 GB.

- [ ] **Step 3: Fix or continue**

If anything misbehaves, reproduce it with a test before fixing.

---

## Task 6: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Rewrite the RESUME next action**

Replace the `▶ NEXT ACTION: a background full-res decode for true 1:1` paragraph with the top
remaining item — **photo edit history surviving a photo switch** (`History<Edits>` lives on
`Session` and dies with it, so Ctrl+Z cannot reach across a switch; needs a per-path history
beside `BufferCache` plus a persistence policy). Renumber "What is left".

- [ ] **Step 2: Add a feature section**

Add a section in the house style, above the parallel-pack one, covering:

- The measured table (full decode 3026 ms / 313 MB, region tail 13.4 ms, prefix stages
  203–562 ms whole-buffer, `downscale_to` 9.1 vs 27.3 ms).
- ⚠ **Why there are three tiers and not two** — the correction that emerged while writing the
  spec: swapping the native buffer halves on-screen scale, which quadruples the render region
  and would have turned a 13.4 ms tail into ~180 ms. Give the tier table and `r ≈ 1.95`.
- ⚠ The precise form of the tier invariant, and that the loose form would produce a failing
  test (Fit has no lower bound; NativeFull magnifies above `2r` because there is no more data).
- ⚠ Two engines, and why: `RenderEngine` owns its buffer, and keeping both means fit-mode is
  byte-for-byte unchanged and never pays the 27.3 ms downscale.
- ⚠ `tier_for` / `clamped` split, and the circularity it avoids.
- ⚠ Clearing `rendered_region`/`requested_region` on handover, and why (rendered-pixel
  coordinates double).
- ⚠ `Session::request` routing, and that a request to the inactive engine is silently dropped.
- The cache sizing: 512 MiB + `MIDS_CAP = 6`, why 512 not 320 (40 Mpx bodies), and ⚠ that the
  count cap fixes a *pre-existing* leak where a Straighten drag fills the tier with garbage.
- Memory: ~1.32 GB worst case for one photo.
- Accepted caveats from the spec, including ⚠ `Demosaic::Fast` rather than the export setting.

- [ ] **Step 3: Update the crate-layout line and the docs index**

`app`'s crate-layout line gains `full_res`. Add
`2026-08-12-full-res-one-to-one-design.md` to the specs list and
`2026-08-12-full-res-one-to-one.md` to the plans list.

⚠ Also revise the **standing caveat that says 1:1 is a half-size buffer skipping demosaic** —
it appears in the "1:1 region rendering" and "Sharpening" sections and is now wrong for RAW
files once the upgrade has landed. Do not delete it; qualify it (it still holds before the
upgrade, and for the NativeHalf tier).

- [ ] **Step 4: Final check**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head; cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

Expected: all green, 29 goldens byte-identical.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the full-resolution 1:1 view"
```

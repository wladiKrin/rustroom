# M5.3a — Spot removal, headless — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clone and heal patches as an `Edits` field (`spots: Vec<Spot>`) and a prefix pipeline stage between Perspective and Geometry, proved against goldens and a real `.RAF` with no canvas code in existence.

**Architecture:** Five tasks. The model and its edit group land first (one commit, because the three exhaustive fixtures are compile errors until the field has a home). Then the stage as a pure gather-then-blend over a disc's bounding box, with `coverage(d, r, feather)` split out as a pure function so the `feather: 0` guard can be probed at exactly `d == r`. Then the stage's insertion into `build_prefix_steps` with its own `PrefixStepKey`. Then two goldens on a new chart, measured. Then a probe example against a real `.RAF`.

**Tech Stack:** Rust, serde. No rayon (a few thousand pixels per spot), no egui anywhere.

**Spec:** `docs/superpowers/specs/2026-09-02-spot-removal-headless-design.md`

---

## Before you start

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Baseline: **1627 tests, 51 goldens**, all green. No `Co-Authored-By` trailer. Never push.

⚠ After writing a test, break the thing it names and watch it fail. Confirm the sabotage (a) compiled, (b) actually changed the file (`diff` against a backup), (c) ran at least one test. Restore with `cp`, then `touch`.

⚠ Unique log names: `/tmp/w3a-*.log`. Count with Python, never awk. The exit code is the authority.

⚠⚠ **Known cross-track fallout, decided up front rather than discovered:** `crates/app/src/develop/panel/adjustments.rs` (owned by tracks C and D this wave) has two tests that assert the groups the panel does *not* show are exactly `["Geometry", "Masks"]`. A ninth group makes that `["Geometry", "Masks", "Retouch"]` — which is precisely what those tests exist to flag. That file is not this track's to edit, so the two-line test change is **reported for the merge**, not applied here. Everything else in the workspace must be green.

---

## The stage, derived

For each spot in order, on the working buffer of size `w × h`:

- `cx = centre.x · (w − 1)`, `cy = centre.y · (h − 1)`, `sx`/`sy` likewise, `r = radius · max(w, h)`.
- `dx = round(sx − cx)`, `dy = round(sy − cy)` — whole pixels, deliberately.
- Bounding box: `floor(cx − r) ..= ceil(cx + r)` clamped to the image, same for y.
- **Gather first**: for every pixel in the box, read the source at `(clamp(x + dx), clamp(y + dy))` into a scratch. Also accumulate the source-disc mean (over `d ≤ r`) and the ring mean (over `r < d ≤ 1.5r`, on the buffer *before* this spot, only pixels inside the image).
- Coverage: `f = clamp(feather/100, 0, 1)`; `d > r → 0`; `f ≤ FEATHER_EPS → 1`; else `t = clamp((r − d) / (f·r), 0, 1)`, smoothstep. At `f = 0` and `d == r` the unguarded expression is `0/0 = NaN`, so the guard is load-bearing exactly there.
- Heal: `patch = src + (ring_mean − src_mean)`, per channel, clamped at 0. Clone: `patch = src`.
- `out = lerp(dst, patch, coverage)`.

A zero or negative radius covers nothing and is skipped — `RadialGradient`'s degenerate rule, and the honest answer. A ring or source disc that contains no pixel at all (a radius below half a pixel at the image corner) falls back to a zero shift rather than dividing by zero.

---

## Task 1: The model — `Spot`, `SpotMode`, `Edits.spots`, the Retouch group

**Files:** `crates/edit-model/src/lib.rs`, `crates/pipeline/src/lib.rs` (`all_edits` only), `crates/app/src/edit_clipboard.rs` (payload fixture only).

- [ ] Add `Spot` and `SpotMode` per the spec, before `Edits`. `Spot` is `Copy`. `mode` carries a field-level `#[serde(default)]` (the spec gives `SpotMode` a `Default` of `Heal`, which is what that is for).
- [ ] Append `pub spots: Vec<Spot>` as the **last** field of `Edits`.
- [ ] `EDIT_GROUP_NAMES` becomes `[&str; 9]` with `"Retouch"` appended; `pub const RETOUCH_GROUP: usize = 8`; `EditGroups.retouch: bool` appended, `false` in `Default`, `true` in `ALL`, `false` in `NONE`; arms `8 => ...` in `get` and `get_mut`.
- [ ] `merge_from`: destructure `spots`; `if groups.retouch { self.spots = spots.clone(); }`.
- [ ] `all_nonneutral()`: `spots: vec![Spot { .. }]`. `pipeline::all_edits()`: likewise. `edit_clipboard`'s `rich`: likewise.
- [ ] Tests in `edit-model`:

```rust
#[test]
fn spots_travel_on_the_retouch_group_and_no_other() {
    let src = all_nonneutral();
    let mut with = Edits::default();
    with.merge_from(&src, EditGroups::only(RETOUCH_GROUP));
    assert_eq!(with.spots, src.spots);
    assert_eq!(with.exposure, 0.0, "only the retouch group was selected");
    for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
        if g == RETOUCH_GROUP { continue; }
        let mut without = Edits::default();
        without.merge_from(&src, EditGroups::only(g));
        assert!(without.spots.is_empty(), "group {g} ({name}) must not carry spots");
    }
}

#[test]
fn retouch_is_off_by_default_for_paste_and_presets() {
    assert!(!EditGroups::default().retouch);
    assert!(EditGroups::ALL.get(RETOUCH_GROUP));
    assert_eq!(EDIT_GROUP_NAMES[RETOUCH_GROUP], "Retouch");
}

#[test]
fn a_spot_round_trips_and_defaults_to_heal() { /* serde: full round trip; `mode` omitted → Heal; `spots` omitted → empty */ }
```

- [ ] Run: `cargo test -p edit-model`, `cargo test -p pipeline --lib`, `cargo test -p app --lib edit_clipboard`.
- [ ] Sabotage: (1) delete the `if groups.retouch` arm → `spots_travel…` fails; (2) put `spots` under `groups.masks` as well → fails on the "no other" half; (3) `retouch: true` in `Default` → `retouch_is_off…` fails.
- [ ] Commit: `feat(edit-model): Spot and SpotMode, Edits.spots, the Retouch group`.

## Task 2: The stage — `stages/spot_removal.rs`

**Files:** new `crates/pipeline/src/stages/spot_removal.rs`, `crates/pipeline/src/stages/mod.rs`, `crates/pipeline/src/lib.rs` (`pub use`).

- [ ] `pub struct SpotRemovalStage { spots: Vec<Spot> }`, `new(spots: &[Spot])`, `PipelineStage` with name `"spot_removal"`.
- [ ] `pub fn coverage(d: f32, r: f32, feather: f32) -> f32` — pure, with `FEATHER_EPS = 1e-4`.
- [ ] Tests (each fixture builds its own `ImageBuf`; every assertion names its blind spot):

1. `a_hard_edged_clone_copies_the_disc_exactly_and_nothing_else` — 64×64 image with a per-pixel pattern `[x/64, y/64, (x*y)%7/7]` so an offset error is visible; spot at (21,21) (a `fixture check` asserts `cx == 21.0` exactly), `radius: 0.125` (r = 8 exactly), source at (+20, +9), `feather: 0`, Clone. Inside `d ≤ r`: `out(x,y) == original(x+dx, y+dy)` bit-exactly. Outside: `out == original` bit-exactly. ⚠ Both halves.
2. `heal_matches_the_ring_where_clone_does_not` — 64×64: left half a 2×2 checker of 0.05/0.15 (mean 0.10), right half flat 0.8. Spot on the right (centre (48, 32), r = 6), source on the left (centre (16, 32)), feather 0. Heal: mean over the disc == 0.8 to 1e-5, and every disc pixel == src + 0.7 (the shift is per channel, tested with a tinted right half). Clone: mean over the disc == 0.10 to 1e-5. Both modes asserted so they are distinguishable.
3. `a_later_spot_reads_an_earlier_spots_result` — background 0.5, two dark discs; spot 1 clones clean onto disc A, spot 2 clones *A* onto disc B. B must come out 0.5. Reversed order (a stage reading the original) gives 0.0.
4. `an_overlapping_source_is_gathered_before_anything_is_written` — horizontal gradient, source `r/2` to the **left** (dx < 0, the direction where an in-place loop reads what it has just written). `out(x) == original(x + dx)` bit-exactly over the disc.
5. `a_source_off_the_image_clamps_to_the_edge` — source centre at `[1.6, 0.5]`: no panic, and every disc pixel equals `original(w − 1, clamp(y + dy))`.
6. `the_feather_ramps_monotonically_from_the_inner_radius_to_the_rim` — `coverage` at 200 samples of `d` in `[0, 1.2r]`: non-increasing, exactly 1 at `d ≤ (1−f)r`, exactly 0 at `d ≥ r`. Plus `feather_zero_is_a_hard_edge_even_exactly_on_the_rim`: `coverage(8.0, 8.0, 0.0) == 1.0` and finite, and an image-level probe of the pixel at exactly `d == r` (the `cx == 21` fixture) coming back equal to the source and not NaN.
7. `a_zero_radius_spot_changes_nothing` — bit-exact identity.

- [ ] Sabotage list: swap `dx`/`dy` (1, 4); use `radius · w` instead of `max(w,h)` on a 64×32 image (add a wide-frame case to 1); drop the `ring_mean − src_mean` shift (2); use `ring` on the buffer *after* the spot (2 — ring would include the patch); apply spots on a clone of the original (3); write in place without the scratch (4); replace clamp with wrap (5); delete the `FEATHER_EPS` guard (6 — the `d == r` probe only); linear ramp instead of smoothstep (6, midpoint value).
- [ ] Commit: `feat(pipeline): SpotRemovalStage — clone and heal a disc from a whole-pixel offset`.

## Task 3: Insertion — `build_prefix_steps` and `PrefixStepKey::SpotRemoval`

**Files:** `crates/pipeline/src/lib.rs`.

- [ ] `PrefixStepKey::SpotRemoval(Vec<Spot>)` — ⚠ this takes `Copy` off `PrefixStepKey`; `PrefixKey::upto` then clones (`s.key.clone()`), and `every_prefix_step_has_its_own_key`'s `keys` closure clones. Check nothing else relied on it.
- [ ] Insert between Perspective and Geometry, guarded on `!edits.spots.is_empty()`.
- [ ] Update `the_prefix_holds_perspective_geometry_noise_reduction_and_dehaze` → names now `["perspective", "spot_removal", "geometry", "noise_reduction", "dehaze"]`; `every_prefix_step_has_its_own_key` → 5 steps with a spots variant at index 1; `prefix_key_changes_with_prefix_fields_and_scale` → a spots variant.
- [ ] New tests: `no_spots_means_no_stage` (`build_prefix_steps(&Edits::default())` has no `SpotRemoval` key, and `build_pipeline` names contain no `"spot_removal"`); `a_spot_survives_a_crop` (render a chart with a spot, with and without `geometry.crop`; the crop window of the uncropped render equals the cropped render bit-exactly — fails if the stage moves after geometry).
- [ ] Sabotage: move the step after Geometry (crop test fails, order test fails); drop the `is_empty` guard (`no_spots…` fails).
- [ ] Commit: `feat(pipeline): spot removal runs in the prefix, between perspective and geometry`.

## Task 4: Goldens — `spot_clone`, `spot_heal`

**Files:** `crates/pipeline/tests/golden.rs` (appended at the very end), two PNGs.

- [ ] `spot_chart()`: 64×48, a smooth vertical sRGB gradient 0.15 → 0.75 with a warm tint, and a dark (0.05) 6 px disc at (24, 20). ⚠ The source sits **3 radii away diagonally** (down and to the right), not purely to the side: on a vertical gradient a source at the same row has the same brightness as the destination by symmetry, and the two goldens would then differ only by rounding — no discriminating power at all. Diagonal puts the source ~10 px further down the gradient, so clone shows a visibly lighter disc and heal has a real shift to undo.
- [ ] `spot_cases()` → `[("spot_clone", …), ("spot_heal", …)]`, feather 40, radius 5/64.
- [ ] `golden_spot_clone`, `golden_spot_heal`, and `the_spot_goldens_do_not_clip_and_differ` (no 0 or 255 sample; the two byte vectors differ in at least 20 samples; the heal render's disc mean is within 0.02 of its ring mean while clone's is not).
- [ ] Generate with `UPDATE_GOLDEN=1`, then run without it. `git status crates/pipeline/tests/golden/` shows exactly the two new PNGs.
- [ ] Commit: `test(pipeline): spot_clone and spot_heal goldens`.

## Task 5: Real `.RAF` validation and a probe

**Files:** `crates/app/examples/spot_probe.rs`, a sidecar in the scratchpad.

- [ ] Render `~/Desktop/fuji/DSCF6509.RAF` neutral at reduced size, look at it, pick a destination on a smooth area (sky/wall) and a source a few radii away.
- [ ] `spot_probe <raw> <cx> <cy> <sx> <sy> <radius>`: decode with `linear_scene()`, run the neutral prefix and the prefix with a heal spot and a clone spot, print the mean RGB over the destination disc, the ring and the source disc before and after each.
- [ ] Hand-written sidecar through `rustroom render --edits`, confirm it renders and the output differs from neutral only inside the disc.
- [ ] Record the numbers in this plan's "Results" section and in the report.
- [ ] Commit: `chore(app): spot_probe example for measuring a spot on a real frame`.

## Finish

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` — exit codes checked directly. Expected: everything green except the two `adjustments.rs` tests named above.
- [ ] Every golden other than the two new ones byte-identical.

---

## Results

**1627 → 1644 tests; 51 → 53 goldens.** `fmt` / `clippy --all-targets --all-features -- -D warnings` /
`test --all` all exit 0. The 51 existing goldens are byte-identical — `git status` on the golden
directory shows only `spot_clone.png` and `spot_heal.png`.

### Real `.RAF`, `DSCF6509.RAF` (6246×4170), radius 75 px, source 2185 px away

```
 neutral  disc [0.1112 0.1125 0.1122]  ring [0.1112 0.1125 0.1127]  source [0.0922 0.0915 0.0894]
   clone  disc [0.0921 0.0915 0.0893]  ring [0.1112 0.1125 0.1127]
    heal  disc [0.1116 0.1128 0.1130]  ring [0.1112 0.1125 0.1127]
```

Clone lands the source's own level — **17.2% below the ring**. Heal corrects that to **within 0.4%**
of the ring while keeping the source's texture. `cargo run --release -p app --example spot_probe`,
kept. A hand-written sidecar through `rustroom render --edits` also renders and differs from
neutral.

### Goldens, measured

`surround 167.4, heal 168.0, clone 199.8`; 724 differing samples; no sample at 0 or 255; 132
distinct values each.

### Two findings from the sabotage step

1. **The golden chart's gradient had to become diagonal.** With a purely vertical ramp, forcing
   `dx = 0` left *both* references byte-identical and the guard green — two points on the same row
   have the same brightness, so a horizontal offset error was invisible. The spec predicted the
   shape of this ("a clone test on a uniform source cannot see a wrong offset") and a one-axis
   gradient is uniform along the other axis.
2. **The heal ring must clear the blemish.** The first case used a 5 px retouch over a 6 px dust
   spot, so the annulus `r < d ≤ 1.5r` still lay *on* the dust and heal dutifully matched the thing
   being removed — patch 146 against a surround of 171. The radius is 9 px now.

### One inert sabotage, recorded rather than papered over

Deleting the degenerate-radius guard entirely leaves every test green: `coverage` already returns
0.0 for `r <= 0.0`, NaN comparisons are all false, and a negative radius inverts the bounding box
which `x0 > x1` rejects. The guard stays as defence in depth; the comment says so.

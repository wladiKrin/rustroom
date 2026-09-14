# M8.7 plan 2 — wiring the region-scoped prefix into the engine

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **⚠ Project conventions, non-negotiable.** Commit messages carry **no `Co-Authored-By` trailer**.
> Branch `m8.7-region-scoped-prefix` (plan 1 is already on it). **Never push. Never merge unprompted.**
> ⚠⚠ Gate every commit with `&&`, never `;`: `cargo clippy --all-targets --all-features -- -D warnings && git commit`.
> A semicolon commits through a failing clippy, which this project has recorded five times and which
> happened again in plan 1.
> ⚠⚠ **`cargo test --workspace` does not build examples**, and `cargo test -p <crate>` does not build
> other crates' test modules. Only `clippy --all-targets` compiles everything — it caught four breaks
> in plan 1 that green tests could not see.
> Build env: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`.

**Goal:** Make `region_plan` actually run, so a drag renders only the visible rect plus its margin
instead of the whole 26 Mpx buffer.

**Architecture:** `render_cached` replaces M8.6's single-step gate with `region_plan` +
`run_region_plan`. Frame dimensions come from the plan, read before any step runs. On settle the
engine renders scoped and then enqueues a whole-buffer render behind it, so the cache still warms.

**Tech Stack:** Rust, the existing `app` and `pipeline` crates. No new dependencies.

⚠⚠ **PLAN 1 IS GREEN AND DEAD.** `region_plan`, `run_region_plan`, `input_rect_for` on four stages
and a chainable `apply_region` all exist with **no caller**. That is the W5 failure mode — creative
LUTs shipped fully tested and 100% non-functional — and **Task 6's carrier test is the only thing
that closes it**. Do not defer Task 6.

**Spec:** `docs/superpowers/specs/2026-09-14-region-scoped-prefix-whole-design.md`
**Plan 1:** `docs/superpowers/plans/2026-09-14-region-scoped-prefix-headless.md`

---

## File structure

| file | responsibility | change |
|---|---|---|
| `crates/app/src/prefix_cache.rs` | `CacheKey`, `PrefixCache`, its tiers, budget and eviction | **create** (pure move) |
| `crates/app/src/render_engine.rs` | the gate, composed frame dims, warm-behind | modify |
| `crates/app/src/lib.rs` | declare the new module | modify |
| `crates/app/examples/bench_preview.rs` | the scoped-drag row | modify |

---

### Task 1: Extract `prefix_cache.rs` — a PURE MOVE, first

⚠⚠ **Do this before anything else, while nothing else is changing.** A move is provable exactly when
the test count is unchanged and no golden moves; bundled with the gate it stops being provable.
`render_engine.rs` is **1606 production lines** and plan 2 adds to it.

**Files:**
- Create: `crates/app/src/prefix_cache.rs`
- Modify: `crates/app/src/render_engine.rs` (remove lines ~637–901), `crates/app/src/lib.rs`

- [ ] **Step 1: Move `CacheKey`, `PrefixCache` and its `impl` verbatim**

```bash
cd ~/Projects/rustroom
grep -n "^type CacheKey\|^struct PrefixCache\|^impl PrefixCache\|^struct BrushCache" crates/app/src/render_engine.rs
```

Move everything from `type CacheKey` up to (not including) `struct BrushCache` into
`crates/app/src/prefix_cache.rs`. ⚠ **Do not reword a single comment.** The `⚠` notes on
`evictable`, `MID_BUDGET_BYTES` and the two tiers are the record of a bug fixed on 2026-09-14 and a
reworded one is a lost one.

Add to `crates/app/src/lib.rs`:

```rust
pub mod prefix_cache;
```

and in `render_engine.rs`:

```rust
use crate::prefix_cache::{CacheKey, PrefixCache};
```

⚠ `CacheKey` and `PrefixCache` were private; they become `pub(crate)` or `pub` as the compiler
demands. ⚠⚠ **Move the `PrefixCache` tests with them** — a test left behind cannot reach a private
field and will fail in a way that looks like the move broke something.

- [ ] **Step 2: Diff the `⚠` comments, which is W0's rule and earned its place twice**

```bash
git stash && grep -o "⚠[^\"]*" crates/app/src/render_engine.rs | sort > /tmp/warn-before.txt
git stash pop && cat crates/app/src/render_engine.rs crates/app/src/prefix_cache.rs | grep -o "⚠[^\"]*" | sort > /tmp/warn-after.txt
comm -23 /tmp/warn-before.txt /tmp/warn-after.txt
```

Expected: **empty**. Anything printed is a finding dropped by the move.

- [ ] **Step 3: Gate — the count must be UNCHANGED**

```bash
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git status --short tests/golden/
```

Expected: `FMT=0`, `CLIPPY=0`, **`passed=2467 failed=0`** — unchanged, which is the assertion for a
pure move rather than "still green" — and an empty golden tree.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -F - <<'MSG'
refactor: prefix_cache out of render_engine, a pure move

render_engine.rs holds the cache, the worker, the render ladder, the scaling
and the RGBA pack in 1606 production lines, and M8.7's wiring adds to it.
The cache is self-contained — its own tiers, budget and eviction rule — and
is the part the wiring touches least.

Done first and alone, because a move is provable exactly when the test count
is unchanged and no golden moves; bundled with the gate it stops being
provable.

Test count unchanged at 2467 and no golden moved. Every ⚠ comment diffed
before and after — W0's rule, and the notes on evictable and MID_BUDGET_BYTES
are the record of a bug fixed on 2026-09-14.
MSG
```

---

### Task 2: The gate — replace M8.6's single-step check

**Files:**
- Modify: `crates/app/src/render_engine.rs` (`render_cached`, the block at ~line 1188)

- [ ] **Step 1: Read what is there now**

```bash
grep -n -B6 -A40 "start + 1 == steps.len()" crates/app/src/render_engine.rs
```

M8.6's gate fires only when exactly the last step needs re-running. It is replaced wholesale.

- [ ] **Step 2: Write the new gate**

Replace the `if start + 1 == steps.len() { ... }` block with:

```rust
    // ⚠⚠ **Derived, never a flag.** Engage when a region is wanted and every step from `start`
    // supports one; there is no drag signal to plumb and therefore nothing that can disagree with
    // anything else — the rule `Session::geometry_mode` was deleted for. It follows that this also
    // speeds the COLD entry into 1:1, not only drags.
    //
    // ⚠ At the fitted rung `region` is `None`, so it never engages there.
    let mut scoped = None;
    let mut was_scoped = false;
    if let Some(want) = region {
        let visible = pipeline::Rect {
            x: want.x,
            y: want.y,
            w: want.w,
            h: want.h,
        };
        if let Some(plan) =
            pipeline::region_plan(&steps, start, visible, img.width(), img.height())
        {
            // ⚠⚠ The area gate. A margined region close to the whole frame costs what the
            // whole-buffer path costs AND is not cached, so the settle render repeats it — strictly
            // worse. The threshold is SWEPT in task 4, not chosen.
            let margined: u64 = plan.out_rects.first().map_or(0, |r| r.w as u64 * r.h as u64);
            let whole = img.width() as u64 * img.height() as u64;
            if whole > 0 && margined * 100 < whole * SCOPE_MAX_AREA_PCT {
                if let Some(out) = pipeline::run_region_plan(&steps, start, &img, &plan) {
                    let last = *plan.out_rects.last().expect("a plan has at least one rect");
                    scoped = Some((out, last, plan.frame));
                    was_scoped = true;
                }
            }
        }
    }
```

and the arm that consumes it:

```rust
    let (input, rect, pw, ph, prefix_stages) = if let Some((out, last, frame)) = scoped {
        // ⚠⚠ **DELIBERATELY NOT CACHED**, M8.6's rule kept for M8.6's reason: a region-scoped
        // checkpoint served for a *different* region is the one failure here that shows as a
        // plausible photograph rather than an error, and leaving the insert out makes it
        // unrepresentable rather than guarded.
        //
        // ⚠⚠ `frame` comes from the PLAN, composed through `output_dims` and read before any step
        // ran. Taking it from `out` would be the region's size, which rasterises every brush mask
        // at the wrong size and thrashes the coverage cache on every pan.
        (
            out,
            Some(RegionRect { x: last.x, y: last.y, w: last.w, h: last.h }),
            frame.0,
            frame.1,
            plan_len,
        )
    } else {
```

⚠ `plan_len` is `steps.len() - start`; bind it where `scoped` is built so the debug line reports how
many steps the scoped chain covered.

- [ ] **Step 3: Add the threshold constant**

```rust
/// The largest share of the frame a margined region may occupy and still be worth scoping, in
/// percent.
///
/// ⚠⚠ **Swept, not chosen.** `MIN_GPU_BOX_PX` began at a value low by ~8x and `MAX_TILE_DABS` was
/// sized by a *limit* rather than the data and measured 5x slower than the CPU it replaced. See
/// task 4 for the sweep this number comes from.
///
/// ⚠ Above it the scoped path costs roughly what the whole-buffer path costs **and is not cached**,
/// so the settle render repeats the work — strictly worse than not scoping.
const SCOPE_MAX_AREA_PCT: u64 = 50;
```

- [ ] **Step 4: Build and run**

```bash
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git status --short tests/golden/
```

⚠⚠ **`a_region_render_matches_the_whole_render_cropped` is the one to watch.** It is the master
region invariant and it now exercises a multi-step scoped chain. If it fails, **read where** — a
band at one edge is an under-sized margin (a bug in some `input_rect_for`), scattered one-ulp
differences are `box_blur`'s running sum and are expected for a chain carrying noise reduction.

- [ ] **Step 5: Commit** (message: the gate is derived, the frame comes from the plan, scoped output
is never cached).

---

### Task 3: The carrier test — the ONLY thing that stops this shipping dead

⚠⚠ **Plan 1 plus task 2 changes no pixel any existing test can read**: a scoped render and a whole
one produce the same picture by design, so **every correctness test in the workspace passes with the
gate deleted**. That is precisely how creative LUTs shipped fully tested and 100% non-functional.

**Files:**
- Modify: `crates/app/src/render_engine.rs` (the `Ran` observable and its test)

- [ ] **Step 1: Extend the observable**

`render_cached` already returns `Ran { prefix_stages, bytes }` and `scoped` is already printed in the
debug line. Add `scoped: bool` to `Ran`, **derived from what happened** rather than tracked
alongside — two facts that can disagree is the shape `render_max_dim` was deleted for.

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn a_drag_of_an_upstream_step_renders_only_the_region() {
    // ⚠⚠ THE carrier. Nothing else in the workspace can see whether the gate fired: a scoped
    // render and a whole-buffer one produce the same picture BY DESIGN, so every correctness test
    // passes with the gate deleted. That is the W5 failure mode exactly.
    let buf = std::sync::Arc::new(structured_buffer(600, 400));
    let mut engine = RenderEngine::new(buf.clone(), /* … */);

    // A prefix with a gather then a local, and a region well inside the frame.
    let mut e = Edits::default();
    e.geometry.angle = 4.0;
    e.noise_reduction = edit_model::NoiseReduction {
        luminance: 0.0, color: 40.0, detail: 50.0, smoothness: 0.0,
    };
    let region = Some(RegionRect { x: 200, y: 140, w: 120, h: 90 });

    // Cold render, whole buffer: the chain has nothing cached, so it scopes from step 0.
    let (_, ran) = engine.render_for_test(&e, /* max_dim */ 600, region);
    assert!(ran.scoped, "a region render with a supporting chain must scope");
    assert!(
        ran.prefix_stages >= 2,
        "fixture check: the scoped chain must cover more than one step, got {}",
        ran.prefix_stages
    );

    // ⚠ And it must DECLINE when there is no region — the fitted rung.
    let (_, ran) = engine.render_for_test(&e, 600, None);
    assert!(!ran.scoped, "no region means no scoping");
}

#[test]
fn a_masked_capture_switches_scoping_off_for_that_photo() {
    // ⚠⚠ The caveat as a behaviour. Masked captures are LAST in the prefix, so one declining step
    // there means nothing above it can be scoped either — the feature is off for that photo.
    // Recorded as a test so a later reader meets it as a decision rather than as a mystery.
    // … build edits carrying a masked dehaze, assert !ran.scoped …
}
```

- [ ] **Step 3: Sabotage — delete the gate and watch ONLY the carrier fail**

```bash
cp crates/app/src/render_engine.rs /tmp/re.bak
python3 - <<'PY'
import io
p="crates/app/src/render_engine.rs"; s=io.open(p,encoding="utf-8").read()
old="    if let Some(want) = region {"
assert s.count(old)==1
io.open(p,"w",encoding="utf-8").write(s.replace(old,"    if false {",1))
print("SABOTAGE APPLIED")
PY
cargo test -p app --lib 2>&1 | grep -E "^test result:|^---- " | head -6
cp /tmp/re.bak crates/app/src/render_engine.rs && touch crates/app/src/render_engine.rs
```

Expected: **the carrier fails and nothing else does.** ⚠⚠ If other tests fail too, good — but if
**nothing** fails, the carrier is not carrying and must be fixed before going on.

- [ ] **Step 4: Commit.**

---

### Task 4: Sweep the area threshold

⚠ A constant nobody can re-measure is a constant nobody can defend.

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`

- [ ] **Step 1: Add a scoped-drag row** that renders the same Straighten drag at several region
  sizes, printing ms against the margined region's share of the frame.

- [ ] **Step 2: Run it on a real RAW at 1:1**

```bash
cargo run --release -p app --example bench_preview ~/Pictures/fuji/DSCF6296.RAF
```

- [ ] **Step 3: Set `SCOPE_MAX_AREA_PCT` from the crossover**, and record the sweep in its doc
  comment. ⚠ Pick the value on the **conservative** side — the gate must fail toward the whole-buffer
  path, where a wrong answer costs milliseconds and never pixels.

- [ ] **Step 4: Commit with the table in the message.**

---

### Task 5: Warm the checkpoint behind the settled render

⚠⚠ Without this the gesture inverts: fluid while dragging, then **~874 ms of nothing** when you let
go, because no drag frame cached anything and the settle render must build the whole-buffer
checkpoint from scratch.

**Files:**
- Modify: `crates/app/src/render_engine.rs`, `crates/app/src/session_flow.rs`

- [ ] **Step 1:** After a scoped render settles, enqueue a whole-buffer render at the same key.
- [ ] **Step 2:** ⚠ It must be **superseded** by a new gesture — the "abandon, don't abort" rule
  `DecodeWorker` and `batch_export` already follow. The worker already coalesces superseded requests;
  do not add a second mechanism.
- [ ] **Step 3:** ⚠⚠ **The swap is invisible only because plan 1 made the two paths agree.** A chain
  of gathers is byte-for-byte and a chain with a local agrees to one ulp, so the warm render replaces
  the scoped one with the same pixels. **Do not weaken either acceptance test to make this easier.**
- [ ] **Step 4: Commit.**

---

### Task 6: Measure the real drag

⚠⚠ **Through `RUSTROOM_DEBUG_RENDER`, never a probe row.** M8.6's retraction exists because
`probe_resamplers` priced a *stage* against a resume the app could not perform, and this file's own
rule — *a synthetic bench prices a stage, not a session* — was already written when it happened.

- [ ] **Step 1: Launch the GUI** (always `--release`, in the background, and say what to look at):

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app
```

- [ ] **Step 2: Drag Straighten at 1:1** on a photo with NR and dehaze active. The log must read
  `scoped yes`, and the per-step lines must show the scoped steps producing the **region's**
  dimensions rather than 6246x4170.

- [ ] **Step 3: Record the band, not a figure.** The baseline is **891.8 ms** (13 frames, 799–1218)
  and the projection is **~113 ms**, explicitly unmeasured. ⚠ Quote a band — M8.3h had to retract a
  headline for quoting a point reading.

- [ ] **Step 4: Check what a still frame cannot.** ⚠⚠ **Pan at 1:1 immediately after a drag**: the
  scoped output is never cached, so the first pan rebuilds the checkpoint. And ⚠ **the picture must
  not change** between the drag and the settle — if it does, the warm-behind render disagrees with
  the scoped one and that is a bug in the margin, not in the cache.

- [ ] **Step 5: Update `CLAUDE.md`** with the measured band, and **`docs/BACKLOG.md`** with the
  airlight-estimator instability (21 levels during a Straighten drag, pre-existing, its own change).

---

## Done when

- `fmt` / `clippy --all-targets --all-features -- -D warnings` / `test --workspace` green together,
  **each exit code read from its own command**.
- **63 goldens run and none move.** ⚠ The airlight already changed in plan 1 and no golden could see
  it; nothing here should change a pixel at all.
- The test count moved by **exactly** the tests added — asserted, not "still green".
- **The gate-deletion sabotage fails the carrier and nothing else.**
- The drag measured through the running app, as a band, with the projection marked unmeasured.

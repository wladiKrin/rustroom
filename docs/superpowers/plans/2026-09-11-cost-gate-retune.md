# M8.3h — the cost gate re-tuned: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `TRANSFER_COST` from a value measured two milestones ago (34.0) to the round trip as
it is today (27.0), so that a lone trailing `output_transform` — the colorimetric frame, and the
trailing run of every masked photo — stops being refused.

**Architecture:** One constant, its doc comment, one permanent row in an existing probe, and three
tests. `worth_a_chain`, `flush`, `REPLACEABLE` and every shader are untouched: this changes what
the gate is *told*, never how it works.

**Tech Stack:** Rust, `gpu-pipeline` (wgpu 29), `pipeline`, `app`'s bench examples.

**Spec:** `docs/superpowers/specs/2026-09-11-cost-gate-retune-design.md`

---

## ⚠ Conventions that are not negotiable

- **Commit messages carry NO `Co-Authored-By` trailer.** Grep this plan and any doc for it before
  committing.
- **Never push.** The user pushes to `github.com/wladiKrin/rustroom` himself.
- The branch **already exists**: `m8.3h-cost-gate`. Do not create another.
- **Every cargo command needs the build env first:**
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- ⚠ **Iterate GPU work with `--release`.** Acquiring an adapter costs 44–75 s in a debug build
  against 21 ms in release.
- ⚠⚠ **Read every gate's exit code from its own file, never through a pipe**, and use
  session-unique log names — `/tmp/t.log` left over from an earlier session has matched stale
  content here before.
- ⚠⚠ **Restore a sabotage from a `cp` backup, never `git checkout -- <file>`.** That has destroyed
  uncommitted work in this repo twice.
- ⚠⚠ **Before every commit, read `git diff --stat` and `git diff | grep '^-'`.** A scripted edit
  spliced by a computed index deleted ten tests on the M8.3g branch and was committed. Read the
  diff *before*, not after.
- ⚠ After any scripted edit, **assert the new text is present and print that it landed.** `cargo
  fmt` reflows lines, so a patch written against pre-fmt text silently matches nothing.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/gpu-pipeline/src/accelerate.rs` | the gate and its constant | `TRANSFER_COST` 34.0 → 27.0, doc comment rewritten, three tests added |
| `crates/gpu-pipeline/examples/probe_rt.rs` | what a round trip costs | a permanent byte-path row |
| `CLAUDE.md` | the durable record | a new M8.3h section, the handoff and NEXT ACTION rewritten |

No new file, no new module, no signature change anywhere.

---

## Task 1: The two tests that pin the decision, then the constant

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs` (test module, and `TRANSFER_COST`)

- [ ] **Step 1: Write both failing tests**

Add to the `mod tests` block in `crates/gpu-pipeline/src/accelerate.rs`, after
`exposure_alone_is_not_worth_a_chain`:

```rust
    /// ⚠⚠ **Built from the stages' OWN `cpu_cost`, never from literals.** A test asserting
    /// `worth_a_chain(&[28.6])` writes the same number twice and passes against any pair that
    /// happens to agree — it guards nothing at all. Reading the cost off the real stage means
    /// that if a cost is ever re-measured, this still describes the truth rather than a stale
    /// pair of digits.
    ///
    /// ⚠⚠ The `display_rendering` half is not symmetry, it is the argument for a **single
    /// conservative constant**. `CreativeLookStage` sits between `display_rendering` and
    /// `output_transform`, so with a creative look active `display_rendering` is a run of its
    /// own — and that run is **not trailing**, so it can never take M8.3g's byte path. A
    /// byte-valued threshold (21.0) would accept it and then pay the float price of 26.8 for
    /// 25.7 ms of work. One constant refuses it by construction.
    #[test]
    fn a_lone_output_transform_chains_and_a_lone_display_rendering_does_not() {
        let Some(ctx) = ctx() else { return };
        let ot = GpuOutputTransformStage::new(RenderSpec::default().space, &ctx);
        let dr = GpuDisplayRenderingStage::new(None, None, &ctx);
        assert!(
            worth_a_chain(&[ot.cpu_cost()]),
            "a lone trailing output_transform ({} ms) must clear the gate — it is the \
             colorimetric frame and the trailing run of every masked photo",
            ot.cpu_cost()
        );
        assert!(
            !worth_a_chain(&[dr.cpu_cost()]),
            "a lone display_rendering ({} ms) must NOT clear the gate: it is never trailing, so \
             it cannot take the byte path, and accepting it would pay the float price for less \
             work",
            dr.cpu_cost()
        );
    }

    /// ⚠⚠ **The test that would have caught the staleness as a BEHAVIOUR rather than as a
    /// number.** A colorimetric frame's whole tail is `["output_transform"]`, and from M8.3e
    /// until M8.3h it built **zero chains** — the gate refusing 28.6 ms of work against a 34.0 ms
    /// threshold that M8.3f and M8.3g had already invalidated. Nothing in the suite could see
    /// that, because every other test asserts on what a chain *contains* rather than on whether
    /// one exists at all.
    #[test]
    fn a_colorimetric_tail_builds_a_chain() {
        let Some(ctx) = ctx() else { return };
        let e = Edits::default();
        let (_, tail) = pipeline::build_pipeline_split_in_region(&e, SCALE, None);
        assert_eq!(
            tail.stage_names(),
            vec!["output_transform"],
            "fixture check: a colorimetric tail is exactly one stage, and this test means \
             nothing if that ever stops being true"
        );
        let (rest, chain) = split(tail, &e, &ctx);
        assert!(
            chain.is_some(),
            "the lone output_transform must now be a chain"
        );
        assert!(
            rest.stage_names().is_empty(),
            "and nothing must be left on the CPU, got {:?}",
            rest.stage_names()
        );
    }
```

- [ ] **Step 2: Run both and verify they FAIL against the shipped 34.0**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-t1-$$
cargo test --release -p gpu-pipeline a_lone_output_transform_chains a_colorimetric_tail_builds > $L.log 2>&1
echo "EXIT=$?"
grep -E "^test result:|panicked at|must clear the gate|must now be a chain" $L.log
```

Expected: **FAIL**, both of them — `a lone trailing output_transform (28.6 ms) must clear the gate`
and `the lone output_transform must now be a chain`.

⚠ **Read the passed-count, not the word `ok`.** `cargo test <filter>` takes a **substring, not a
regex or a list**; a filter matching nothing prints `0 passed … ok`, which reads as green. If the
count is 0, the filter is wrong and this step proved nothing.

- [ ] **Step 3: Move the constant and rewrite its doc comment**

In `crates/gpu-pipeline/src/accelerate.rs`, replace the whole doc comment and the constant:

```rust
/// What one round trip costs, in the same units as [`GpuStage::cpu_cost`] — milliseconds per 6.8
/// megapixels, measured by `gpu-pipeline`'s `probe_rt` on an Apple M2.
///
/// ⚠⚠ **THE MEASUREMENT, AND WHY IT MOVED (M8.3h).** This was **34.0** from M8.3e until M8.3h,
/// and two later milestones made the round trip cheaper *without re-measuring it*: **M8.3f**'s
/// resource cache recovered 9.46 ms per render, and **M8.3g**'s 8-bit readback took the download
/// from 13.20 ms to 4.78. Re-measured at 3200x2136: **26.80 ms**, rounded here to 27.0. For two
/// milestones the gate was refusing runs that would have paid, using a number that had stopped
/// describing reality.
///
/// ⚠⚠ **THIS IS THE FLOAT ROUND TRIP, DELIBERATELY, THOUGH THE CASE IT UNLOCKS TAKES THE BYTE
/// PATH.** The byte round trip measures **21.02 ms** and is *not* used, because a lone
/// `display_rendering` run (25.7 ms) can occur — `CreativeLookStage` sits between it and
/// `output_transform`, so a creative look makes it a run of its own — and **that run is not
/// trailing, so it can never take the byte path**. A byte-valued threshold would accept it and
/// pay the float price for less work. One conservative constant refuses it by construction,
/// with no second rule to keep in step. Revisit only when a ported stage's cost lands between
/// 21 and 27.
///
/// ⚠⚠ **STANDING RULE: anything that changes the upload, the dispatch count or the download must
/// re-run `probe_rt` and update this.** That is how it went stale, and it is recorded as a rule
/// rather than a test on purpose — a timing assertion cannot fail a build honestly, and this
/// project already records that a benchmark is not a test.
///
/// ⚠ **The comparison is scale-free, which is why this is one number and not one per rung.** Both
/// sides are linear in pixel count, and both paths measure it: float 7.23 -> 26.80 ms and byte
/// 5.22 -> 21.02 ms across a 4.0x pixel ratio.
const TRANSFER_COST: f32 = 27.0;
```

- [ ] **Step 4: Run both tests again and verify they PASS**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-t1b-$$
cargo test --release -p gpu-pipeline a_lone_output_transform_chains a_colorimetric_tail_builds > $L.log 2>&1
echo "EXIT=$?"
grep -E "^test result:" $L.log
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Run the whole gpu-pipeline suite — the gate moved, so every gate-dependent test must be re-checked**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-t1c-$$
cargo test --release -p gpu-pipeline > $L.log 2>&1
echo "EXIT=$?"
grep -E "^test result:" $L.log
grep -c "SKIPPED: no GPU" $L.log
```

Expected: `EXIT=0`, all passing, `0` skips.

⚠ **If `exposure_alone_is_not_worth_a_chain` or `a_run_cheaper_than_the_transfer_is_left_on_the_cpu`
fails, that is a real finding, not a test to adjust** — exposure is 1.3 ms and `[0.5, 1.2]` is
1.7 ms, both far under 27.0, so they must still pass. A failure means the constant landed wrong.

- [ ] **Step 6: Commit**

```bash
git diff --stat
git diff | grep '^-[^-]'
git add crates/gpu-pipeline/src/accelerate.rs
git commit -F - <<'MSG'
fix: the cost gate's transfer constant was two milestones stale

TRANSFER_COST was measured at M8.3e. M8.3f's resource cache recovered
9.46 ms per render and M8.3g's readback took the download 13.20 -> 4.78,
and neither re-measured it. probe_rt now reads 26.80 ms at rung 3200
against a shipped 34.0, so the gate has been refusing runs that pay.

27.0, deliberately the FLOAT round trip even though the case it unlocks
takes the byte path: a lone display_rendering run (25.7) can occur with a
creative look active, is never trailing, and so can never take the byte
path — a byte-valued threshold would accept it and pay the float price.

Exactly one decision changes: a lone trailing output_transform (28.6)
starts chaining, which is the colorimetric frame and the trailing run of
every masked photo. Both built ZERO chains before this.

The decision test reads each stage's own cpu_cost rather than repeating
a literal, so it still describes the truth if a cost is re-measured.
MSG
```

---

## Task 2: The frame-level equivalence check

The colorimetric frame now takes a GPU path it has never taken. Its pieces are covered — the
output-transform equivalence test, and M8.3g's byte-vs-float frame test — but the *combination* is
new.

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs` (test module)

- [ ] **Step 1: Write the test**

Add to `mod tests`, directly below `a_colorimetric_tail_builds_a_chain`:

```rust
    /// ⚠⚠ **A colorimetric frame now takes a GPU path it has never taken, so it must still be
    /// the CPU's picture.** The pieces are covered — `output_transform`'s own equivalence test,
    /// and M8.3g's byte-against-float frame test — but nothing covered the *combination*, which
    /// is exactly what M8.3h newly creates. A chain that renders a plausible-but-different
    /// neutral photograph would pass every other test in this file.
    ///
    /// ⚠ The bound is the **per-stage** one (0.01 of an 8-bit level), not the three-stage chain's
    /// 0.02: this is a one-stage chain, and the looser bound exists for accumulation across
    /// several transcendental stages that is not happening here.
    #[test]
    fn a_colorimetric_frame_renders_the_same_picture_on_the_gpu() {
        let Some(ctx) = ctx() else { return };
        let e = Edits::default();
        let cpu = tail_of(&e).run(scene_ramp());
        let gpu = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        )
        .run(scene_ramp());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }
```

- [ ] **Step 2: Run it**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-t2-$$
cargo test --release -p gpu-pipeline a_colorimetric_frame_renders > $L.log 2>&1
echo "EXIT=$?"
grep -E "^test result:|worst" $L.log
```

Expected: `test result: ok. 1 passed`.

⚠ If it fails, **do not widen the bound**. A one-stage chain diverging by more than 0.01 of a level
is a finding about `GpuOutputTransformStage`, not a number to adjust — this repo records that
rule for the three-stage bound and it applies harder here.

- [ ] **Step 3: Commit**

```bash
git diff --stat
git diff | grep '^-[^-]'
git add crates/gpu-pipeline/src/accelerate.rs
git commit -m "test: a colorimetric frame renders the same picture through its new chain"
```

---

## Task 3: `probe_rt` keeps the byte-path row

It is the evidence that one constant suffices, and the thing a future re-measure runs.

**Files:**
- Modify: `crates/gpu-pipeline/examples/probe_rt.rs`

- [ ] **Step 1: Add the import**

Change the `use` line:

```rust
use gpu_pipeline::{GpuChain, GpuExposureStage, GpuOutputTransformStage};
```

- [ ] **Step 2: Add the byte-path measurement**

Immediately after the existing `println!` that reports the one-stage chain, still inside the
`for (w, h)` loop:

```rust
        // ⚠⚠ **The byte path, measured end to end rather than derived by subtraction.** M8.3g's
        // decomposition puts the download at 13.20 ms (f32) against 4.78 (RGBA8), so subtracting
        // predicts 18.4 ms here. It measures ~21. That is M8.3g's own headline finding — a total
        // can be right while its components do not compose — and it is why this row exists
        // instead of a comment doing the arithmetic.
        //
        // ⚠ This is the evidence that `TRANSFER_COST` can stay a **single** conservative
        // constant: see its doc comment for the `display_rendering` case that rules out using
        // this number as a second threshold.
        let mut bbest = f64::MAX;
        for _ in 0..5 {
            let chain = GpuChain::new(
                std::sync::Arc::clone(ctx),
                vec![Box::new(GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    ctx,
                ))],
            );
            let t = Instant::now();
            let out = chain.run_rgba8(&img);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            assert!(out.is_some(), "the byte path must be available");
            std::hint::black_box(&out);
            bbest = bbest.min(ms);
        }
        println!("{w}x{h}: one-stage chain, BYTE path  {bbest:.2} ms  <- trailing output_transform");
```

- [ ] **Step 3: Run it and record both numbers**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p gpu-pipeline --example probe_rt
```

Expected, within noise of:
```
1600x1068 (20.5 MB): one-stage chain 7.23 ms  <- essentially the round trip
1600x1068: one-stage chain, BYTE path  5.22 ms  <- trailing output_transform
3200x2136 (82.0 MB): one-stage chain 26.80 ms  <- essentially the round trip
3200x2136: one-stage chain, BYTE path  21.02 ms  <- trailing output_transform
```

⚠ **If the float figure has moved more than ~2 ms from 26.80, stop and re-derive the constant**
rather than proceeding — that is the standing rule firing on its first outing.

- [ ] **Step 4: Commit**

```bash
git diff --stat
git diff | grep '^-[^-]'
git add crates/gpu-pipeline/examples/probe_rt.rs
git commit -F - <<'MSG'
probe: probe_rt reports the byte round trip beside the float one

The evidence that TRANSFER_COST can stay a single conservative constant,
and the thing to re-run whenever the upload, the dispatch count or the
download changes.

Measured rather than derived: subtracting M8.3g's components predicts
18.4 ms and it measures 21.02 — the third time in three milestones that
components have failed to compose.
MSG
```

---

## Task 4: Sabotage — prove the tests see the decision

**Files:**
- Temporarily modify: `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Back the file up with `cp`**

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/accelerate-m83h.rs.bak
```

⚠⚠ **A `cp` backup, never `git checkout -- <file>`.** That command has destroyed uncommitted work
in this repo twice, most recently on the boolean-masks branch.

- [ ] **Step 2: Put the stale constant back**

```bash
python3 - <<'PY'
import io
p='crates/gpu-pipeline/src/accelerate.rs'
s=io.open(p,encoding='utf-8').read()
a='const TRANSFER_COST: f32 = 27.0;'
assert s.count(a)==1, f"expected exactly one, found {s.count(a)} — the sabotage did NOT apply"
s=s.replace(a,'const TRANSFER_COST: f32 = 34.0;')
io.open(p,'w',encoding='utf-8').write(s)
print("SABOTAGE APPLIED")
PY
```

⚠ The `assert` and the printed confirmation are the point: a scripted edit that silently matched
nothing would leave the next step "passing" against correct code, which has happened twice here.

- [ ] **Step 3: Run and confirm the right tests fail**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-sab-$$
cargo test --release -p gpu-pipeline > $L.log 2>&1
echo "EXIT=$?"
grep -E "^test result:" $L.log
grep -E "^failures:" -A 10 $L.log
```

Expected: **FAILED**, naming `a_lone_output_transform_chains_and_a_lone_display_rendering_does_not`
and `a_colorimetric_tail_builds_a_chain`.

⚠⚠ **Confirm a `test result:` line appears at all.** A sabotage that fails to *compile* prints no
result line and reads exactly like a clean pass — a trap this repo records.

⚠ `a_colorimetric_frame_renders_the_same_picture_on_the_gpu` will **pass** under this sabotage, and
that is correct rather than a gap: with no chain built, the accelerated pipeline *is* the CPU one,
so the two agree trivially. Record it as inert for this sabotage rather than counting it.

- [ ] **Step 4: Restore and verify the restore**

```bash
cp /tmp/accelerate-m83h.rs.bak crates/gpu-pipeline/src/accelerate.rs
touch crates/gpu-pipeline/src/accelerate.rs
git status --short
```

Expected: **no output** — a clean tree is what proves the restore was byte-identical.

⚠ `touch` after the restore: `cp` can leave an older mtime, and cargo then reuses the artifact
built from the sabotaged source — which has surfaced here as tests failing against correct code.

---

## Task 5: Measure it on the real engine

**Files:** none modified. This task produces numbers for the record.

- [ ] **Step 1: Run the bench**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6291.RAF
```

- [ ] **Step 2: Compare against the pre-change baseline**

Baseline, measured on this branch's parent at rung 3200:

| case | CPU | GPU | |
|---|---|---|---|
| TYPICAL of this catalogue | 78.62 ms | 33.69 | 2.33× |
| unedited, camera rendering | 57.17 | 28.58 | 2.00× |
| **unedited, colorimetric** | **36.80** | **36.84** | **1.00×** |
| hsl only | 105.32 | 25.30 | 4.16× |
| hsl + grade + sharpening | 174.33 | 44.07 | 3.96× |

Expected after: **"unedited, colorimetric" leaves 1.00×** and lands near ~1.2×. Every other row
should be unchanged within noise — they already chained.

⚠⚠ **The predicted gain is ~7.6 ms against a measured ±4 ms noise floor**, so a single run is not
a result. If the colorimetric row has not clearly moved, take M8.3g's treatment: an **A/B on one
binary and one machine state**, best of five, and state the outcome as a range. Do **not** quote a
single run as the figure.

⚠ Record whether the machine was quiet (`top -l 1 -o cpu -n 5`). Every bench in this file's record
carries that caveat because two of them were contaminated.

- [ ] **Step 3: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
nohup cargo run --release -p app > /tmp/rustroom-gui-m83h.log 2>&1 &
echo launched
```

**What to look at:** an **unedited, colorimetric** photo — rendering off, lens corrections off —
with `gpu_tail` **on**. ⚠⚠ **The picture must not change.** That frame now takes a GPU path it has
never taken, so this is the one case where a wrong chain would show. ⚠ Compare the **histogram**
with the toggle on and off, not just the image: a one-level shift is invisible on a photograph.

---

## Task 6: The record, the gate, the commit

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Write the M8.3h section**

Add a new section immediately **before** the `**M8.3g — the 8-bit readback` section. It must carry:

- the measurement (26.80 float / 21.02 byte at rung 3200) and the shipped 34.0 it replaces;
- ⚠⚠ **why it went stale** — M8.3f and M8.3g each made the round trip cheaper after the constant
  was written, which is the transferable lesson: *a constant that describes a measured cost goes
  stale when something else improves that cost, and nothing in the build will say so*;
- ⚠⚠ **the chain-count probe correcting the going-in assumption** — a mask does not split a run
  into two chains, it truncates the run so neither half clears the gate, and a masked photograph
  built **zero** chains;
- ⚠⚠ **the byte figure measured rather than derived**, 21.02 against a subtraction predicting 18.4
  — the third consecutive milestone in which components failed to compose;
- the `display_rendering` argument for one conservative constant;
- the standing rule, and that it is a rule rather than a test on purpose;
- the before/after bench rows;
- the caveats from the spec's §7, including ⚠⚠ **that this does not make the mask loop
  unnecessary**.

- [ ] **Step 2: Rewrite the RESUME block's handoff and NEXT ACTION**

- Demote the current `**State at handoff (2026-09-11, latest):**` line to `⚠ Superseded handoff
  follows:` and write a new one for M8.3h.
- ⚠⚠ **The numbering:** this took the M8.3h slot, so **the mask loop becomes M8.3i**. The current
  NEXT ACTION block calls it M8.3h and is stale — rewrite it, and say the shift explicitly.
- The new NEXT ACTION is **M8.3i, the mask loop**, and it should carry the measurements this
  branch produced: 1 masked HSL is 121 ms and 3 masked HSL is 290 ms at rung 3200, a masked photo
  gains only ~7.6 ms from this slice, and **1 of the catalogue's 40 sidecars has a mask** — which
  is real evidence against the milestone and must be recorded beside the use report that argues
  for it, not instead of it.

- [ ] **Step 3: Run the full gate, exit codes read from files**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/rr-m83h-final-$$
cargo fmt --all --check > $L-fmt.log 2>&1; echo "FMT=$?" > $L-fmt.code
cargo clippy --workspace --all-targets --all-features -- -D warnings > $L-clippy.log 2>&1; echo "CLIPPY=$?" > $L-clippy.code
cargo test --workspace --release > $L-test.log 2>&1; echo "TEST=$?" > $L-test.code
cat $L-fmt.code $L-clippy.code $L-test.code
echo "--- total passed:"; grep -oE "^test result: ok\. [0-9]+ passed" $L-test.log | awk '{s+=$4} END {print s}'
echo "--- goldens ran:"; grep -cE "^test golden_" $L-test.log
echo "--- gpu skips:"; grep -c "SKIPPED: no GPU" $L-test.log
echo "--- failures:"; grep -c "^test result: FAILED" $L-test.log
```

Expected: `FMT=0 CLIPPY=0 TEST=0`, **63 goldens ran**, **0 GPU skips**, **0 failures**.

⚠⚠ **All 63 goldens must be byte-identical.** They are CPU-only, which is precisely what makes
them the proof that no pixel path moved. A golden that shifts here means something far worse than
a re-tuned constant.

⚠ **Read the values, not the wrapper's exit status.** A `grep -c` finding zero matches exits 1 and
has twice been misreported here as a failing gate.

- [ ] **Step 4: Commit and STOP**

```bash
git diff --stat
git diff | grep '^-[^-]'
git add CLAUDE.md
git commit -F - <<'MSG'
docs: record M8.3h, the cost gate re-tuned

TRANSFER_COST 34.0 -> 27.0. It was measured at M8.3e and two later
milestones made the round trip cheaper without re-measuring it.

The transferable lesson: a constant describing a measured cost goes stale
when something else improves that cost, and nothing in the build says so.
Recorded as a standing rule on the constant rather than as a test, because
a timing assertion cannot fail a build honestly.

Also recorded, both produced by measuring rather than reasoning:

  - a mask does not split a run into two chains, it truncates the run so
    neither half clears the gate — a masked photograph built ZERO chains,
    which corrected the going-in assumption;
  - the byte round trip is 21.02 ms measured against 18.4 predicted by
    subtracting M8.3g's components, the third consecutive milestone in
    which components failed to compose.

The mask loop becomes M8.3i, and the record carries the evidence against
it (1 of 40 sidecars has a mask) beside the use report for it.
MSG
```

⚠⚠ **Do NOT merge and do NOT push.** The user merges on his own call and pushes himself.

---

## Self-review

**Spec coverage.** §2 measurement → Task 3 re-runs and records it. §2.1 table → Task 1's decision
test reads the same two costs off the real stages. §2.2 chain counts → recorded in Task 6. §3
one-conservative-constant and the `display_rendering` argument → Task 1's doc comment and the test's
second assertion. §4 what changes → Tasks 1 and 3. §5 testing, including the sabotage → Tasks 1, 2
and 4. §6 confirmation → Task 5. §7 caveats → Task 6. No gaps.

**Placeholder scan.** No TBD, no "add error handling", no "similar to Task N". Every code step
carries the code; every command carries its expected output.

**Type consistency.** `worth_a_chain(&[f32])`, `GpuOutputTransformStage::new(OutputSpace, &Arc<GpuContext>)`,
`GpuDisplayRenderingStage::new(Option<&[[f32;2]]>, Option<f32>, &Arc<GpuContext>)`,
`split(Pipeline, &Edits, &Arc<GpuContext>) -> (Pipeline, Option<GpuChain>)`, `tail_of(&Edits) -> Pipeline`,
`scene_ramp() -> ImageBuf`, `crate::compare(&ImageBuf, &ImageBuf) -> Divergence` with
`.worst_levels`, `.differing`, `.total` — all read from the existing file rather than invented, and
used identically in every task.

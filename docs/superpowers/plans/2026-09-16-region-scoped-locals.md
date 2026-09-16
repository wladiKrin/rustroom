# M8.12 — the region-scoped locals actually scope: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `apply_region` for `noise_reduction` and `dehaze` compute only the margined input rect instead of the whole source buffer, so a region-scoped render of a local stage does work proportional to the region.

**Architecture:** Both stages already have a correct `input_rect_for` that grows `out` by `kernel_reach` on every side. The fix is three lines in each `apply_region`: resolve `need`, crop `src.buf` to it, apply, then crop `out` out of the result. No change to `region_plan`, `run_region_plan`, the area gate, or the airlight.

**Tech Stack:** Rust, `crates/pipeline`, `cargo test -p pipeline`, golden-image regression tests.

**Spec:** `docs/superpowers/specs/2026-09-16-region-scoped-locals.md`

⚠ **Run every `cargo` command with:**
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠⚠ **Never run two gates concurrently** — this repo has 7 temp-dir test-isolation sites.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/stages/noise_reduction.rs` | luminance + chroma NR | `apply_region` crops to `need`; one new fixture; one new test |
| `crates/pipeline/src/stages/dehaze.rs` | dark-channel dehaze | `apply_region` crops to `need`; one new test |

Both files are ~450 and ~470 lines — under the ~600 marker, no split needed.

---

### Task 1: A sized noisy fixture for noise reduction

`noisy_patch` is hard-coded 64×64. The new test needs an image larger than `4 * kernel_reach`, so the rect can be interior by more than the margin on every side.

**Files:**
- Modify: `crates/pipeline/src/stages/noise_reduction.rs:309` (the `noisy_patch` fixture)

- [ ] **Step 1: Replace `noisy_patch` with a sized version it delegates to**

Replace the existing `noisy_patch` body (keeping its doc comment and signature) so that it reads:

```rust
    /// A 64x64 patch of `base` plus per-channel noise.
    fn noisy_patch(base: [f32; 3], amp: f32) -> ImageBuf {
        noisy_rect(64, 64, base, amp)
    }

    /// A `w`x`h` patch of `base` plus per-channel noise.
    ///
    /// ⚠ The RNG is seeded identically to `noisy_patch`, so a region comparison is between two
    /// runs over the SAME pixels rather than over two different noises.
    fn noisy_rect(w: usize, h: usize, base: [f32; 3], amp: f32) -> ImageBuf {
        let mut rng = Rng(0x1234_5678);
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // ⚠ The patch is authored in **sRGB** and converted into the working space,
                // exactly as the golden charts are. Writing these numbers straight in would
                // have them read as ProPhoto — a different, more saturated colour, whose
                // chroma noise this stage then handles differently.
                img.set_pixel(
                    x,
                    y,
                    color::srgb_to_prophoto([
                        base[0] + amp * rng.next_signed(),
                        base[1] + amp * rng.next_signed(),
                        base[2] + amp * rng.next_signed(),
                    ]),
                );
            }
        }
        img
    }
```

- [ ] **Step 2: Verify the existing tests still pass unchanged**

Run: `cargo test --release -p pipeline noise_reduction 2>&1 | tail -5`
Expected: PASS, same count as before. This is a pure refactor of a fixture.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/stages/noise_reduction.rs
git commit -m "test(pipeline): a sized noisy fixture, so a region test can be interior"
```

---

### Task 2: The failing region test for noise reduction

**Files:**
- Modify: `crates/pipeline/src/stages/noise_reduction.rs` (tests module, after `use super::*` at line 291)

- [ ] **Step 1: Write the failing test**

Add to the tests module:

```rust
    /// ⚠⚠ **The test this stage did not have, and could not have had while `apply_region`
    /// computed the whole buffer and cropped** — it would have compared a computation with
    /// itself and passed for that reason. `perspective` and `lens_correction`, the gathers, have
    /// carried the equivalent all along.
    #[test]
    fn a_region_matches_the_same_part_of_the_whole() {
        let params = NoiseReduction {
            luminance: 40.0,
            detail: 50.0,
            color: 40.0,
            smoothness: 50.0,
        };
        let reach = kernel_reach(&params, RenderScale::FULL);
        assert!(reach > 0, "a zero reach would make the margin below vacuous");
        let stage = NoiseReductionStage::new(&params, RenderScale::FULL);

        let side = 4 * reach + 64;
        let src = noisy_rect(side, side, [0.45, 0.5, 0.55], 0.06);
        let whole = stage.apply(src.clone());
        // Fixture check: a stage that moved nothing would make the comparison vacuous.
        assert_ne!(whole.data(), src.data(), "the stage must move pixels");

        // ⚠⚠ INTERIOR by more than `reach` on every side. A rect touching the frame edge would
        // pass even with the margin deleted, because there the whole-buffer path clamps its own
        // window too — so an edge rect tests nothing about the margin.
        let rect = Rect {
            x: 2 * reach,
            y: 2 * reach,
            w: 64,
            h: 64,
        };
        let got = stage
            .apply_region(&crate::RegionSrc::whole(&src), rect)
            .expect("a region inside the buffer must be produced");
        let want = whole.crop(rect.x, rect.y, rect.w, rect.h);
        assert_eq!(got.width(), rect.w);
        assert_eq!(got.height(), rect.h);
        assert_eq!(
            got.data(),
            want.data(),
            "a region must be byte-identical to that part of the whole"
        );
    }
```

- [ ] **Step 2: Run it and confirm it PASSES for the wrong reason**

Run: `cargo test --release -p pipeline a_region_matches_the_same_part_of_the_whole -- --exact --nocapture 2>&1 | tail -6`

⚠⚠ **Expected: PASS.** That is not a failure of the plan — it is the defect. `apply_region`
currently computes `self.apply(src.buf.clone())`, i.e. exactly `whole`, so the assertion is a
tautology. **Task 4's sabotage is what will prove the test has teeth.** Do not skip it.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/stages/noise_reduction.rs
git commit -m "test(pipeline): noise reduction's missing region-equivalence test"
```

---

### Task 3: Make noise reduction's `apply_region` crop to the margin

**Files:**
- Modify: `crates/pipeline/src/stages/noise_reduction.rs:123-134` (`apply_region`)

- [ ] **Step 1: Replace the body**

Replace:

```rust
        let ox = out.x.checked_sub(src.origin.0)?;
        let oy = out.y.checked_sub(src.origin.1)?;
        if ox + out.w > src.buf.width() || oy + out.h > src.buf.height() {
            return None;
        }
        let done = self.apply(src.buf.clone());
        Some(done.crop(ox, oy, out.w, out.h))
```

with:

```rust
        // ⚠⚠ **Crop to what the kernel actually reaches BEFORE applying.** Until 2026-09-16 this
        // cloned the whole buffer and cropped the answer out, which is byte-correct and does
        // full-frame work: a scoped 0.7 Mpx drag measured 93-96 ms, the same as the whole 6.5 Mpx
        // frame, because it WAS the whole frame. The doc comment above already said "the clone is
        // of the margined region"; the code did not.
        let need = self.input_rect_for(out, src.full.0, src.full.1)?;
        let lx = need.x.checked_sub(src.origin.0)?;
        let ly = need.y.checked_sub(src.origin.1)?;
        if lx + need.w > src.buf.width() || ly + need.h > src.buf.height() {
            return None;
        }
        // ⚠ `out` is inside `need` by construction — `input_rect_for` grows it — so these cannot
        // underflow. `checked_sub` anyway: a future `input_rect_for` that under-asks should fail
        // loudly here rather than produce a shifted band.
        let ix = out.x.checked_sub(need.x)?;
        let iy = out.y.checked_sub(need.y)?;
        let done = self.apply(src.buf.crop(lx, ly, need.w, need.h));
        Some(done.crop(ix, iy, out.w, out.h))
```

- [ ] **Step 2: Run the region test**

Run: `cargo test --release -p pipeline a_region_matches_the_same_part_of_the_whole -- --exact --nocapture 2>&1 | tail -6`
Expected: PASS — and now it means something, because the two sides are different computations.

- [ ] **Step 3: Run the whole stage's tests**

Run: `cargo test --release -p pipeline noise_reduction 2>&1 | tail -5`
Expected: PASS, no test lost.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/src/stages/noise_reduction.rs
git commit -m "perf(pipeline): noise reduction's region crops to the margin, not the frame"
```

---

### Task 4: Sabotage — prove the noise-reduction test has teeth

**Files:**
- Temporarily modify: `crates/pipeline/src/stages/noise_reduction.rs` (`input_rect_for`)

- [ ] **Step 1: Delete the margin**

Change `input_rect_for` from:

```rust
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
```

to:

```rust
        let _ = (in_w, in_h);
        Some(out)
```

- [ ] **Step 2: Run the region test and confirm it FAILS**

Run: `cargo test --release -p pipeline a_region_matches_the_same_part_of_the_whole -- --exact --nocapture 2>&1 | tail -12`
Expected: **FAIL** on `a region must be byte-identical to that part of the whole`.

⚠⚠ **If it PASSES, stop.** The test is not testing the margin, and shipping it would be the
guard-that-cannot-fail this milestone exists to remove. Re-check that the rect is interior by
more than `reach` and that `reach > 0` for the chosen params.

- [ ] **Step 3: Revert the sabotage**

```bash
git checkout crates/pipeline/src/stages/noise_reduction.rs
```

- [ ] **Step 4: Confirm the revert restored a passing test**

Run: `cargo test --release -p pipeline a_region_matches_the_same_part_of_the_whole -- --exact 2>&1 | tail -4`
Expected: PASS.

- [ ] **Step 5: Record the result**

No commit — the tree is unchanged. Note the observed FAIL text for the final report.

---

### Task 5: The failing region test for dehaze

⚠ Dehaze's `apply_region` returns `None` unless an airlight was supplied, so the stage must be
built with `with_airlight`, not `new`.

**Files:**
- Modify: `crates/pipeline/src/stages/dehaze.rs` (tests module, after `use super::*` at line 375)

- [ ] **Step 1: Write the test**

```rust
    /// ⚠⚠ **The test this stage did not have**, for the same reason noise reduction did not:
    /// while `apply_region` computed the whole buffer and cropped, this assertion compared a
    /// computation with itself.
    #[test]
    fn a_region_matches_the_same_part_of_the_whole() {
        let params = Dehaze { amount: 60.0 };
        let reach = kernel_reach(&params, RenderScale::FULL);
        assert!(reach > 0, "a zero reach would make the margin below vacuous");

        let side = 4 * reach + 64;
        let clear = scene(side, side);
        let a = [0.72, 0.76, 0.80];
        let hazy = add_haze(&clear, a);
        // ⚠ The airlight is SUPPLIED, exactly as `prefix_airlight` supplies it in the engine.
        // Estimated per-call it would differ between the whole buffer and a slice, and the
        // comparison below would measure that rather than the margin.
        let stage = DehazeStage::with_airlight(&params, RenderScale::FULL, a);

        let whole = stage.apply(hazy.clone());
        // Fixture check: a stage that moved nothing would make the comparison vacuous.
        assert_ne!(whole.data(), hazy.data(), "the stage must move pixels");

        // ⚠⚠ INTERIOR by more than `reach` on every side — an edge rect would pass with the
        // margin deleted, because the whole-buffer path clamps its window there too.
        let rect = Rect {
            x: 2 * reach,
            y: 2 * reach,
            w: 64,
            h: 64,
        };
        let got = stage
            .apply_region(&crate::RegionSrc::whole(&hazy), rect)
            .expect("a region inside the buffer, with an airlight, must be produced");
        let want = whole.crop(rect.x, rect.y, rect.w, rect.h);
        assert_eq!(got.width(), rect.w);
        assert_eq!(got.height(), rect.h);
        assert_eq!(
            got.data(),
            want.data(),
            "a region must be byte-identical to that part of the whole"
        );
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test --release -p pipeline dehaze::tests::a_region_matches 2>&1 | tail -6`
⚠ Expected: **PASS for the wrong reason**, as in Task 2. Task 7 proves it.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/stages/dehaze.rs
git commit -m "test(pipeline): dehaze's missing region-equivalence test"
```

---

### Task 6: Make dehaze's `apply_region` crop to the margin

**Files:**
- Modify: `crates/pipeline/src/stages/dehaze.rs:291-303` (`apply_region`)

- [ ] **Step 1: Replace the body**

Replace:

```rust
        let ox = out.x.checked_sub(src.origin.0)?;
        let oy = out.y.checked_sub(src.origin.1)?;
        if ox + out.w > src.buf.width() || oy + out.h > src.buf.height() {
            return None;
        }
        let done = self.apply(src.buf.clone());
        Some(done.crop(ox, oy, out.w, out.h))
```

with:

```rust
        // ⚠⚠ **Crop to what the kernel actually reaches BEFORE applying.** Until 2026-09-16 this
        // cloned the whole buffer and cropped the answer out: a one-step scoped plan measured
        // 101-105 ms against the 18 ms the same stage costs as step 1 of a two-step plan, the
        // only difference being the size of the buffer handed to it.
        //
        // ⚠ Safe only because the airlight is SUPPLIED — the `self.airlight?` above. Estimated
        // from the slice, cropping the input would change the estimate and the photograph would
        // move as you pan.
        let need = self.input_rect_for(out, src.full.0, src.full.1)?;
        let lx = need.x.checked_sub(src.origin.0)?;
        let ly = need.y.checked_sub(src.origin.1)?;
        if lx + need.w > src.buf.width() || ly + need.h > src.buf.height() {
            return None;
        }
        let ix = out.x.checked_sub(need.x)?;
        let iy = out.y.checked_sub(need.y)?;
        let done = self.apply(src.buf.crop(lx, ly, need.w, need.h));
        Some(done.crop(ix, iy, out.w, out.h))
```

- [ ] **Step 2: Run dehaze's tests**

Run: `cargo test --release -p pipeline dehaze 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/stages/dehaze.rs
git commit -m "perf(pipeline): dehaze's region crops to the margin, not the frame"
```

---

### Task 7: Sabotage — prove the dehaze test has teeth

**Files:**
- Temporarily modify: `crates/pipeline/src/stages/dehaze.rs` (`input_rect_for`)

- [ ] **Step 1: Delete the margin**

Change `input_rect_for` from:

```rust
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
```

to:

```rust
        let _ = (in_w, in_h);
        Some(out)
```

- [ ] **Step 2: Run and confirm FAIL**

Run: `cargo test --release -p pipeline dehaze::tests::a_region_matches 2>&1 | tail -12`
Expected: **FAIL** on byte-identity.

⚠⚠ **If it PASSES, stop and report** — same rule as Task 4.

- [ ] **Step 3: Revert**

```bash
git checkout crates/pipeline/src/stages/dehaze.rs
```

- [ ] **Step 4: Confirm PASS restored**

Run: `cargo test --release -p pipeline dehaze::tests::a_region_matches 2>&1 | tail -4`
Expected: PASS.

---

### Task 8: The full gate, each exit code from its own command

⚠⚠ **`cargo build` alone is not a gate** — this repo's standing lesson. Read three exit codes.

- [ ] **Step 1: Run the three, separately**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --check >/dev/null 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1; echo "CLIPPY=$?"
cargo test --workspace >/tmp/m812_test.txt 2>&1; echo "TEST=$?"
tail -20 /tmp/m812_test.txt
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`. If `FMT=1`, run `cargo fmt` and re-read all three.

- [ ] **Step 2: Confirm the goldens actually RAN, not merely passed**

```bash
grep -c 'golden' /tmp/m812_test.txt
grep -E 'test result:.*(golden|pipeline)' /tmp/m812_test.txt | head
```

Expected: the 63 golden tests present and passing. ⚠ A golden suite that was filtered out is not
a green golden suite — this repo has been caught by that distinction before.

- [ ] **Step 3: Confirm `crates/pipeline/tests` is unmodified**

```bash
git status --porcelain crates/pipeline/tests
```

Expected: empty. Goldens must move because the code is right, never because the fixture was edited.

- [ ] **Step 4: Commit if anything changed**

```bash
git status --porcelain
```
If `cargo fmt` altered files, commit them:
```bash
git add -A && git commit -m "style: cargo fmt"
```

---

### Task 9: Re-measure the drag through the real engine

⚠⚠ **Through `RUSTROOM_DEBUG_RENDER`, never a synthetic probe row.** M8.6's headline was retracted
for pricing a stage against a resume the real chain never took.

- [ ] **Step 1: Check the machine's load and record it**

```bash
uptime; ps -A -o %cpu,comm | sort -rn | head -3
```
⚠ Record this in the report. `obsidian-notes-rag` and `julia` have each been at ~100% during
earlier runs on this machine, and every number below is an upper bound while they are.

- [ ] **Step 2: Run the instrumented bench**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_RENDER=1 cargo run --release -q -p app --example bench_nr_checkpoint \
  ~/Pictures/fuji/DSCF6296.RAF 2>&1 | grep -E 'scoped step|scoped TOTAL' | tail -30
```

Expected, against the before-numbers in the spec:
- `scoped step 0 noise_reduction` falls from **93–96 ms** toward the ~18 ms band.
- `scoped step 1 dehaze` in a one-step plan falls from **101–105 ms** toward ~18 ms.
- The `margin` column is unchanged — 1.36× and 1.00×. **It was never the problem.**

- [ ] **Step 3: Quote a band, not a figure**

At least 3 runs' worth of lines are printed by the bench's own repetition. Report ranges.

⚠ If the numbers do NOT move, the fix is not reaching the measured path — return to Phase 1 of
`superpowers:systematic-debugging` rather than adjusting the fix.

---

### Task 10: Record it

**Files:**
- Modify: `CLAUDE.md` (State, Next action)
- Modify: `docs/BACKLOG.md` (the 118 ms entry at the head of `## Performance`)
- Modify: `docs/JOURNAL.md` (new dated section, newest first)

- [ ] **Step 1: Resolve the BACKLOG entry**

Mark `### ⚠⚠ A SCOPED 0.7 Mpx DEHAZE DRAG COSTS 118 ms AND NOTHING EXPLAINS IT` as ✅ EXPLAINED
AND FIXED, with the root cause in one line and the measured before/after band.

- [ ] **Step 2: Update `CLAUDE.md` in place**

⚠⚠ **Overwrite the live state; do NOT append a superseded block.** Move the replaced text to
`docs/JOURNAL.md`. Target: `CLAUDE.md` stays under ~400 lines.

- [ ] **Step 3: Add the journal section**

Record: the defect, the doc/implementation divergence, the vacuous-test shape, the sabotage
results from Tasks 4 and 7, and the measured band.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md docs/JOURNAL.md
git commit -m "docs: M8.12 — the scoped locals did whole-frame work, and no test could see it"
```

- [ ] **Step 5: Do NOT merge and do NOT push**

Report the branch state and let the user decide. ⚠ Re-gate the tip before any merge: a gate run
before the last commit describes a tree nobody shipped.

---

## Self-Review

**Spec coverage.** Done-when 1 → Tasks 3, 6. Done-when 2 (real tests, interior rect) → Tasks 2, 5.
Done-when 3 (sabotage) → Tasks 4, 7. Done-when 4 (three exit codes) → Task 8 step 1. Done-when 5
(goldens ran) → Task 8 step 2. Done-when 6 (re-measure, band, load) → Task 9. Risk (pan at 1:1)
→ ⚠ **not covered by any task, and cannot be: it is a judgement about a photograph.** Task 10
reports it as owed, and the user performs it.

**Placeholders.** None: every code step carries the full replacement text, every run step an exact
command and its expected output.

**Type consistency.** `need`/`lx`/`ly`/`ix`/`iy` are used identically in Tasks 3 and 6.
`RegionSrc::whole`, `input_rect_for`, `kernel_reach`, `Rect`, `RenderScale::FULL`,
`DehazeStage::with_airlight` and `NoiseReductionStage::new` all match the signatures in the tree
as of this branch. `noisy_rect` is defined in Task 1 and used in Task 2.

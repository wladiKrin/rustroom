# A GPU-resident base for `GpuBrushRaster`: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a GPU chain has just produced the image a gating brush's gate needs, hand that buffer to `GpuBrushRaster` instead of staging a second copy of the same bytes.

**Architecture:** `SharedPlane<GpuImage>` already carries a buffer between two collaborators who never meet. Today it runs rasteriser → sub-stage. This adds the **producer** side on `GpuChain`'s output and a **take** in `GpuBrushRaster`, ahead of its upload.

**Spec:** `docs/superpowers/specs/2026-09-18-gpu-brush-resident-base.md`

⚠ Prefix cargo with `PKG_CONFIG_PATH`. ⚠⚠ Never run two gates concurrently. ⚠ Read each exit code
from its own command, **before** committing.

---

### Task 1: Does the producer actually fire? — verify, do not assume

**⚠⚠ THE MILESTONE CHANGES SHAPE IF THIS FAILS.** Reading the code says a `GpuChain` is flushed
immediately before a declining mask (`accelerate.rs:523`), so a `GpuImage` exists just before its
download. Two things could make that never happen in practice:

- the mask is the **first** tail stage, so `run` is empty and `flush` returns `None`;
- `worth_a_chain` refuses the run, so the stages go back on the CPU and no `GpuImage` is built.

**Files:** Create `crates/app/examples/probe_brush_base_producer.rs`

- [ ] **Step 1: Write a probe that reports, for a gating-brush mask on a real frame:**
  the tail's stage names in order; whether a `GpuChain` precedes the mask; and the chain's output
  dimensions against the base the rasteriser asks for. Drive it through `gpu_pipeline::accelerate`
  with edits that carry **a global exposure** (the drag case) and, separately, **no global edits**.

- [ ] **Step 2: Run it.** Expected: with a global edit, a chain precedes the mask and its output
  dimensions equal the gate base's. Without one, no chain — which is the case this milestone cannot
  help and must not claim to.

- [ ] **Step 3: ⚠ If no chain ever precedes the mask, STOP and report.** Building a hand-off with no
  producer is the W5 failure mode — shipping green and dead.

- [ ] **Step 4: Commit the probe.**

---

### Task 2: The chain offers its output

**Files:** Modify `crates/gpu-pipeline/src/chain.rs` (the download point), `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1:** Give `GpuChain` an opt-in `offer_output` flag, set by `accelerate` **only** on a
chain flushed immediately before a declining mask. ⚠ Opt-in, not always-on: a deposit nobody takes
must be cleared, and the fewer producers the slot has, the fewer ways it is taken by the wrong frame.

- [ ] **Step 2:** At the point `GpuChain::run` downloads, when the flag is set, `deposit` the
`GpuImage` **before** the download, with its true `w`/`h`.

```rust
// ⚠⚠ Deposited BEFORE the download and only when asked. `SharedPlane` is one-shot and
// thread-stamped for the reason its own docs give: "a sub-stage chain that found a previous
// apply's plane still resident would render those pixels. The failure is a plausible photograph,
// not an error." A second producer is exactly how that happens, so this one is opt-in.
if self.offer_output {
    ctx.shared_plane().deposit(img.width(), img.height(), img.clone());
}
```

- [ ] **Step 3:** Build and run `cargo test --release -q -p gpu-pipeline`.
- [ ] **Step 4: Commit.**

---

### Task 3: `GpuBrushRaster` takes it

**Files:** Modify `crates/gpu-pipeline/src/brush_raster.rs:248-277`

- [ ] **Step 1:** Before staging, try the slot. On a hit, skip the staging memcpy entirely; on a
miss, upload exactly as now.

```rust
// ⚠⚠ Take before staging, and fall through on a miss. `take` matches on DIMENSIONS, so a plane
// from another frame at another size cannot be taken — and one at the SAME size from the wrong
// frame is what the thread stamp and the one-shot slot exist to prevent.
// ⚠ A miss is the ordinary case, not an error: the mask may be the tail's first stage, or the run
// may not have been worth a chain. Uploading then is correct, not a fallback.
let taken = ctx.shared_plane().take(b.w, b.h);
```

⚠ On the taking path `d_stage` stays zero, which is what the re-measurement in Task 6 reads.

- [ ] **Step 2:** Build; run the crate's tests.
- [ ] **Step 3: Commit.**

---

### Task 4: The coverage is unchanged — byte-identical, both paths

**Files:** `crates/gpu-pipeline/src/brush_raster.rs` tests

- [ ] **Step 1:** A test that rasterises the same gating brush twice — once with a deposit waiting,
once without — and asserts the two `CoverageMap`s are **byte-identical**.

⚠ A fixture check first: assert the taking path really took, or the test compares the upload path
with itself and passes for that reason. **That tautology has been written twice already today.**

- [ ] **Step 2: Run.** Expected PASS.
- [ ] **Step 3: Commit.**

---

### Task 5: Sabotage — three, because the failure is a plausible photograph

- [ ] **Step 1:** Make `take` always return `None`. → Task 4's test must still pass (pixels
unchanged, timing only) **and** its fixture check must FAIL, proving the check has teeth.
- [ ] **Step 2:** Deposit a plane of the **wrong dimensions**. → must be refused by `take`'s
dimension match, not rendered.
- [ ] **Step 3:** Deposit a plane whose **contents differ** from the true base. → Task 4's test must
FAIL. ⚠⚠ This is the one that matters: it is the "wrong frame's pixels" failure in miniature.
- [ ] **Step 4:** Revert each; confirm green.

⚠⚠ If step 3 passes, the test is not comparing what it claims to and the milestone is unguarded.

---

### Task 6: Re-measure, gate, launch

- [ ] **Step 1:** `RUSTROOM_DEBUG_MASK=1` on `box_floor_sweep` — `stage=` must read **0.00** on the
taking path against **2.04–2.23** before. Quote the whole-frame drag as a **band**, with the load.
⚠ Do not subtract the `gpu` delta to claim a total: the spec records that part of it is the gate
reading the base, which this does not remove.
- [ ] **Step 2:** `cargo fmt`; commit any change; **then** gate, reading three exit codes separately;
confirm 74 goldens ran and `git diff HEAD --stat` is empty.
- [ ] **Step 3:** Launch the GUI. **Paint an edge-detecting brush mask, then drag a global slider.**
It must be quicker, and ⚠⚠ **the mask must not change shape or drift as the slider moves** — that is
the plausible-photograph failure and no test can see it.
- [ ] **Step 4:** Record in `CLAUDE.md` (overwrite in place) and `docs/JOURNAL.md`, including
whether the prefix→tail resident buffer is now cheaper for sharing this mechanism.
- [ ] **Step 5:** Do not merge, do not push.

## Self-Review

**Spec coverage.** Done-when 1 → Task 1. 2 → Tasks 2–3. 3 → Task 4. 4 → Task 5. 5 → Task 5 steps 2–3.
6 → Task 6 step 1. 7 → Task 6 step 2. 8 → Task 6 step 3, performed by the user.

**Placeholders.** Task 1's probe is described by what it must report rather than by full source,
because its shape depends on what `accelerate` exposes — ⚠ the one place in this plan where the
engineer must write code the plan does not hand them.

**Type consistency.** `SharedPlane::deposit(w, h, payload)` and `take(w, h) -> Option<T>` match
`shared_plane.rs:84,101`. `BaseRef { plane, w, h, x0, y0 }` is unchanged — **no wgpu type enters
`pipeline`**, which is the constraint that rules out passing the buffer through the trait.

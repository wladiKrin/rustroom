# M8.3h — the cost gate re-tuned

**Status:** design approved 2026-09-11, inline execution authorised in the same breath
("looks right, write the spec and execute").

⚠⚠ **NUMBERING.** This takes the **M8.3h** slot, so **the mask loop becomes M8.3i**. `CLAUDE.md`'s
NEXT ACTION block calling the mask loop M8.3h is stale the moment this lands, and rewriting it is
part of the work. This is the fourth renumbering in Phase 8; the rule the project follows is that
whatever ships next takes the next letter, and the record says so explicitly rather than leaving
two documents disagreeing.

---

## 1. What this is

`accelerate`'s cost gate refuses a run of GPU-capable stages whose total CPU work does not exceed
one round trip. The constant it compares against, `TRANSFER_COST = 34.0`, was measured at **M8.3e**
— and **two later milestones made the round trip cheaper without re-measuring it**:

- **M8.3f**, the GPU resource cache, recovered 9.46 ms per render.
- **M8.3g**, the 8-bit readback, took the download from 13.20 ms to 4.78 ms.

So the gate has been refusing runs that would now pay, using a number that stopped describing
reality two milestones ago.

This slice re-measures it and moves it. It touches one constant, one doc comment, one example and
the tests that pin the decision. **It does not change how the gate works, only what it is told.**

---

## 2. The measurement

Re-measured with the existing `gpu-pipeline/examples/probe_rt`, Apple M2, one-stage chain, best of
five:

| | 1600×1068 (20.5 MB) | 3200×2136 (82.0 MB) |
|---|---|---|
| **float round trip** | 7.2–7.3 ms | **23.0–26.8 ms** |
| **byte round trip** (trailing `output_transform`) | 5.1–5.2 ms | **15.7–21.0 ms** |
| `TRANSFER_COST` as shipped | — | 34.0 |

⚠⚠ **QUOTED AS RANGES, AND THE FIRST DRAFT OF THIS SPEC DID NOT.** It stated 26.80 and 21.02 from
a single run each. Seven runs on a quiet machine (69% idle) show those were both the *top* of
their bands. A single run is not a measurement at this magnitude — the same lesson M8.3g's
control row exists for, arriving here against my own spec.

⚠ Both paths are **scale-free**, which is what keeps this one constant rather than one per rung:
on the first run, 26.80 / 7.23 = 3.71 and 21.02 / 5.22 = 4.03 against a pixel ratio of 4.0. That
was M8.3e's own justification for a single number and it still holds.

### ⚠⚠ A claim this spec made and now retracts

An earlier draft said: *"the byte figure was measured, not derived, and deriving it would have
been wrong by 45%"* — M8.3g's components predict 26.80 − 8.42 = **18.4 ms**, against a first
reading of **21.02**.

**That was wrong, and it was wrong in an instructive way.** With the byte trip measured over seven
runs at **15.7–21.0 ms**, the prediction of 18.4 sits comfortably *inside* the band. The components
composed perfectly well here. What produced the "45% error" was comparing a **point prediction
against a single top-of-band reading** — which is the very mistake M8.3g's control row was
introduced to make visible, committed here while citing M8.3g's lesson as the reason for it.

⚠ The measurement was still the right thing to do: it produced the range, and the range is what
makes the constant defensible. But it did not catch the failure it was credited with, and saying
so is worth more than the tidier story.

### 2.1 What the gate is actually compared against

Every ported stage's `cpu_cost`, in ms per 6.8 Mpx, against the three candidate thresholds:

| stage | `cpu_cost` | vs **34.0** (shipped) | vs **26.8** (float) | vs **21.0** (byte) |
|---|---|---|---|---|
| `exposure` | 1.3 | ✗ | ✗ | ✗ |
| `tone_regions` | 18.0 | ✗ | ✗ | ✗ |
| `display_rendering` | 25.7 | ✗ | ✗ | ✓ |
| **`output_transform`** | **28.6** | **✗** | **✓** | ✓ |
| `color_grading` | 48.0 | ✓ | ✓ | ✓ |
| `sharpening` | 54.0 | ✓ | ✓ | ✓ |
| `hsl` | 60.0 | ✓ | ✓ | ✓ |

**Exactly one decision changes at 26.8: a lone trailing `output_transform` starts chaining.**

### 2.2 What that is worth, and where

Measured on the real engine (`bench_gpu_tail`, DSCF6291.RAF, rung 3200, whole frames):

| case | CPU | GPU | | chains built |
|---|---|---|---|---|
| TYPICAL of this catalogue | 78.62 ms | 33.69 | 2.33× | 1 |
| TYPICAL + 1 masked exposure | 98.20 | 68.97 | 1.42× | 1 |
| 1 masked exposure (gradient) | 53.73 | 55.12 | **0.97×** | **0** |
| 1 masked HSL (gradient) | 121.43 | 121.36 | **1.00×** | **0** |
| 3 masked HSL | 290.21 | 289.19 | **1.00×** | **0** |
| unedited, colorimetric | 36.80 | 36.84 | **1.00×** | **0** |

⚠⚠ **THE CHAIN COUNTS ARE MEASURED, NOT INFERRED, AND THEY CORRECTED THE GOING-IN ASSUMPTION.**
A temporary probe printed each tail and its chain count. The expectation was that a mask *splits*
a run and pays two round trips. **It does not.** A mask ends the run, and then neither piece
clears the gate: `["mask", "output_transform"]` reports `chains: 0 (+0 trailing)`, because the only
accelerable stage left is `output_transform` at 28.6 against a threshold of 34.0. **The GPU toggle
is inert on a masked photograph — not merely unhelpful.** Right direction, wrong mechanism; the
probe is what settled it.

So moving the threshold to 27.0 gives a chain to:

- the **colorimetric** frame, which has read 1.00× in every bench table since M8.3e;
- the **trailing run of every masked photo**, which today gets nothing at all;
- the post-mask run generally.

Each of those is a lone trailing `output_transform`, which means each takes the **byte** path
(sRGB display space, no Visualize Spots, no soft proof — the default), so the work replaced is
28.6 ms of CPU by a 15.7–21.0 ms round trip: **roughly 8–13 ms at rung 3200**, and ⚠ that spread
is itself wider than the ±4 ms noise floor, so the frame-level confirmation in §6 is what decides
the real figure.

---

## 3. The decision: one constant, deliberately the float cost

`TRANSFER_COST = 27.0` — rounded up from the **top** of the measured float band (23.0–26.8),
which is what makes it conservative rather than central.

**Why the float number rather than the byte one, when the case it unlocks takes the byte path.**
A lone `display_rendering` run (25.7) can occur: `CreativeLookStage` sits between
`display_rendering` and `output_transform`, so with a creative look active the tail is
`[… display_rendering, creative_look, output_transform]` and `display_rendering` becomes a run of
its own. **That run is not trailing, so it can never take the byte path**, so a byte-valued
threshold (21.0) would accept it on the strength of a saving it cannot have.

⚠⚠ **Stated honestly, and weaker than this spec first claimed.** The first draft said accepting it
would "pay the float price of 26.80 for 25.7 ms of work — a net loss". With the float trip measured
at **23.0–26.8**, 25.7 sits *inside* the band: accelerating it is not a clear loss, merely not a
clear win. What settles it is the gate's own stated property — **fail toward the CPU, because a
wrong number costs milliseconds and never pixels** — not an arithmetic certainty. One conservative
constant refuses it by construction, with no second rule to keep in step.

**Rejected: two constants, float 27 and byte 21, with `accelerate` told whether the float is
needed.** Measured against §2.1 it buys **nothing today** — the only stage that can end a tail is
`output_transform` at 28.6, already above both thresholds — and it costs a signature change plus a
gate that must predict which path a chain will take. `accelerate` cannot see the third condition
(`display_space`, Visualize Spots, soft proofing are decided in `render_engine`), so it would
sometimes be wrong. Revisit only when a ported stage's cost lands between 21 and 27.

**Rejected: modelling GPU execution as well** (`transfer + dispatches`). A model change rather than
a re-tune, and it points the wrong way: four dispatches are 11.61 ms, so it would *raise* effective
thresholds and refuse runs that currently pay.

**Rejected: going below the measurement.** The gate's stated property is that it fails toward the
CPU — a wrong number costs milliseconds and never pixels — and a threshold under the measured
transfer inverts that.

---

## 4. What changes

**`crates/gpu-pipeline/src/accelerate.rs`** — `TRANSFER_COST: f32 = 34.0` becomes `27.0`, and its
doc comment is rewritten to carry:

- the measurement as a **range** (23.0–26.8 ms float, 3200×2136, Apple M2, `probe_rt`);
- **why it moved** — M8.3f and M8.3g each made the round trip cheaper after the constant was
  written, which is how it went stale;
- the byte figure (21.02) and the `display_rendering` reason it is deliberately not used;
- ⚠⚠ **the standing rule:** anything that changes the upload, the dispatch count or the download
  must re-run `probe_rt` and update this constant. *A recorded rule rather than a test* — a timing
  assertion cannot fail a build honestly, and this project already records that a benchmark is not
  a test.

**`crates/gpu-pipeline/examples/probe_rt.rs`** — gains a permanent byte-path row beside the float
one. It is the evidence that one constant suffices, and it is what a future re-measure runs.

Nothing else. `worth_a_chain`, `flush`, `REPLACEABLE`, every `cpu_cost` and every shader are
untouched.

---

## 5. Testing

**The decision test must not restate the constant.** A test asserting `worth_a_chain(&[28.6])`
guards nothing: it encodes the same literal twice and passes against any pair that happens to
agree. Instead it builds the **real** `GpuOutputTransformStage` and `GpuDisplayRenderingStage`,
reads each stage's own `cpu_cost()`, and asserts the gate's verdict:

- a run of `output_transform` alone **is** worth a chain;
- a run of `display_rendering` alone **is not**.

So if a stage cost is ever re-measured, the test still describes the truth rather than a stale
pair of numbers — and the `display_rendering` half is what pins the §3 argument for the
conservative constant.

**The behaviour test is the one that would have caught the staleness.** Drive `accelerate` with a
colorimetric tail — the shape the probe reported as `["output_transform"]` — and assert a chain
comes back where none did before.

⚠⚠ **The sabotage that decides whether either test is real: put `34.0` back.** The behaviour test
must fail. If it does not, it is not seeing the decision, and the test is wrong rather than the
code.

**What must not move.** All **63 goldens byte-identical** — they are CPU-only, which is precisely
what makes them the proof that no pixel path moved. And the colorimetric frame now takes a GPU path
it has never taken, so it must render **identically to the CPU**: the pieces are covered by the
output-transform equivalence test and by M8.3g's `the_byte_path_and_the_float_path_produce_the_same_frame`,
but the *combination* at frame level is new and gets its own check.

---

## 6. How it is confirmed

Re-run `bench_gpu_tail` on DSCF6291.RAF. Expected: **"unedited, colorimetric" leaves 1.00×**, and
the masked rows leave 0.97–1.00×.

⚠⚠ The predicted gain is **roughly 8–13 ms at rung 3200 against a measured ±4 ms noise floor**, so it gets
M8.3g's treatment exactly: an **A/B on one binary and one machine state**, best of five, stated as
a range rather than a single figure. A single run quoted as a result would be indistinguishable
from noise — which is the error M8.3g's control row was introduced to make visible.

---

## 7. Accepted caveats

1. **The win is one case and modest** — roughly 8–13 ms at rung 3200 on frames whose tail ends in
   a lone `output_transform`, and the band is wide enough that §6's frame measurement is what
   settles the real number. Nothing else changes.
2. ⚠⚠ **This does not make the mask loop unnecessary.** A masked photograph gains ~8–13 ms and
   remains fundamentally unaccelerated at 121 ms (1 masked HSL) and 290 ms (3 masked HSL) at rung
   3200. M8.3i is still the milestone that addresses it.
3. **The gate still ignores GPU execution entirely.** It compares CPU work against the transfer and
   says nothing about the dispatches, which is a model limitation this slice deliberately does not
   take on.
4. **Measured on one machine, one adapter (Apple M2, Metal), at two rungs.** The constant is
   machine-specific, as it has been since M8.3e.
5. **A lone `display_rendering` run stays on the CPU** at 25.7 ms — correctly, per §3, but it is
   real work left unaccelerated whenever a creative look is active.
6. **`gpu_tail` is still off by default**, so a fresh install sees no change from this.

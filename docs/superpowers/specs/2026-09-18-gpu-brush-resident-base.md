# A GPU-resident base for `GpuBrushRaster`

**Status:** spec · 2026-09-18
**Justification:** the user intends to use **brush masks with edge detection**. ⚠ That matters
because the measurement alone did not justify this: the catalogue holds **one `Luminance` mask
across 40 sidecars** and no brushes, so nothing in it exercises this path. The stated intent is the
reason to build it, and it should be recorded as such rather than dressed up as a measured win.

## The defect

A **gating** brush (auto-mask, edge-detecting) decides its coverage by reading the photograph. So:

- `GpuMaskStage::new` **declines** it — the stage is built from `Edits`, before the photograph
  exists (`mask.rs:289`). The CPU `MaskStage` runs instead.
- That stage rasterises through `GpuBrushRaster`, which needs the photograph as a **gate base**.
- `PipelineStage::apply` takes an `ImageBuf`, so the base is a **CPU slice** and must be uploaded.
- On a steady frame the coverage cache serves it and none of this happens. **But a global slider
  drag changes the base the gate reads, so the key is invalidated every frame** — every frame is a
  miss, and every frame re-uploads the whole base.

**Measured, same box (1002×502, 12 dabs), non-gating against gating:**

| | `stage` | `gpu` | `read` |
|---|---|---|---|
| no base | 0.00 | 4.92–5.10 | 0.11–0.19 |
| base (**78.1 MB**) | **2.04–2.23** | 7.61–10.16 | 0.12–0.15 |

**So the base costs ~2.1 ms of host memcpy plus part of a 2.5–5.1 ms device delta**, per frame of
such a drag.

⚠⚠ **There is no cheap version of this fix.** `chain.rs:218` already measured `queue.write_buffer`
at **3.90 ms against staging's 1.86** for 19.6 MB, and records that *"the memcpy into mapped memory
is the floor and wgpu exposes no way around it"*. The only way to remove the upload is **not to
upload** — which is this milestone.

## ⚠ Two things measured but NOT explained, recorded so they are not mistaken for noise

1. **`gpu` is bimodal** — 7.61–7.65 or 10.14–10.16, with one 8.89 and nothing between. Two states,
   run to run, cause unknown. It is a **2.5 ms swing on every gating rasterise**, which is the same
   order as this whole milestone.
2. **The `2.5–5.1 ms` device delta is not all transfer.** It also contains the dispatch *reading*
   the base to evaluate the gate, which a resident base does **not** remove. ⚠⚠ `brush_raster.rs:474`
   warns that this project *"has been wrong about exactly that kind of subtraction three times"* —
   so **~2.1 ms is the floor of the win and ~7 ms its ceiling**, and the true figure is unknown.

## Goal

When the base a gating brush needs is **already on the device**, `GpuBrushRaster` takes it instead
of uploading a second copy of the same bytes.

## Design, and the one thing it rests on

`SharedPlane<GpuImage>` already exists for precisely this shape of problem — *"a one-shot hand-off
slot for a GPU buffer two collaborators both need … the two participants never meet, so the hand-off
goes through the one thing both hold, the `GpuContext`."* Today it runs **rasteriser → sub-stage**:
the rasteriser uploads the base, deposits it, and the mask's first sub-stage takes it.

This milestone adds the **producer side**: whatever last held the image as a `GpuImage` before the
CPU `MaskStage` received it deposits that buffer, and `GpuBrushRaster` **takes** it rather than
staging a fresh copy.

⚠⚠ **TASK 1 VERIFIES THE PREMISE, AND THE MILESTONE CHANGES SHAPE IF IT FAILS.** It is not
established that a `GpuImage` of the right dimensions exists at that moment. The tail chain
downloads to an `ImageBuf` at its end, and whether the mask's input *is* that download — same
dimensions, same frame, same call stack — is an assumption. **If no such buffer exists, there is
nothing to make resident and the work is a different milestone.** Measure before building.

## ⚠⚠ The hazard, and it is this crate's worst one

`SharedPlane` is **one-shot and thread-stamped**, and its own docs say why: *"a sub-stage chain that
found a previous apply's plane still resident would render those pixels. The failure is a plausible
photograph, not an error."* Adding a **second producer** to a slot that already has one is exactly
how a plane gets taken by the wrong frame.

⚠ `begin_mask` exists to drop per-apply state for this reason and must keep doing so. A deposit that
outlives its frame is the defect, not a leak.

## Non-goals

- **Not** making `GpuMaskStage` accept a gating brush. That is *"the open question M8.11 did not
  settle"*, and it is a different milestone.
- **Not** the prefix→tail resident buffer, though it is **the same underlying mechanism** — a GPU
  buffer surviving a stage boundary. ⚠ Whichever lands first should make the second cheaper; say so
  in the report rather than duplicating the plumbing.
- Not the bimodal `gpu` column. Recorded above, chased separately if at all.

## Done when

1. Task 1 has **established or refuted** that a same-frame, same-dimension `GpuImage` exists to
   deposit. ⚠ If refuted, stop and report — do not build a mechanism with no producer.
2. `GpuBrushRaster` takes a deposited base when one matches, and uploads when none does.
3. **The coverage it produces is unchanged** — byte-identical to the uploading path, pinned by a
   test that runs both and compares.
4. **A sabotage step:** making the take always miss must not change any pixel, only the timing; and
   making it take a **wrong-sized** plane must fail loudly rather than render.
5. ⚠⚠ **A test that a deposit cannot outlive its frame**, in the shape of `SharedPlane`'s existing
   thread and one-shot guards. The failure this prevents is a plausible photograph.
6. Re-measured with `RUSTROOM_DEBUG_MASK`: `stage=` falls to **0.00** on the taking path, and the
   whole-frame drag figure is quoted as a **band** with the machine's load.
7. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code read from its own command and read BEFORE committing**; 74 goldens ran.
8. ⚠ Confirmed **by use**, which is the only check that matters here: paint an **edge-detecting
   brush mask**, then drag a **global** slider. It must be quicker, and **the mask must not change
   shape or drift** as the slider moves.

## Risk

The failure mode is a **plausible photograph** — a mask built from the wrong frame's pixels. Items
4, 5 and 8 are the whole guard; the goldens cannot see this path at all, because no golden carries
a gating brush.

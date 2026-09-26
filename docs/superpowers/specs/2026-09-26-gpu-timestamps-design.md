# GPU timestamps: per-stage GPU time in the running app

**Date:** 2026-09-26 · **Branch:** `gpu-timestamps` · **Status:** approved in brainstorm

## Why

The only GPU figure the project has is a CPU stopwatch around submit and wait. For a Straighten
drag at rung 3200 it reads ~27 ms headless and ~34 in the app, and it hides three different
things: how long each stage actually ran on the GPU, how long the copies took, and how long the
work sat queued before the GPU started it. `docs/PIPELINE-REVIEW-2026-09-26.md` estimates
3–4 ms per full-frame per-pixel dispatch by subtracting a guessed gather time from that stopwatch;
that estimate is what the per-pixel fusion spec rests on, and the ~10 ms app-versus-probe gap is
unexplained because nothing separates GPU execution from queue latency. A guess is not a number
to build on. This spec is the instrument.

## The constraint that shapes it

wgpu 29's Metal backend offers `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS` when the
device samples counters at stage boundaries, which Apple silicon does. `TIMESTAMP_QUERY_INSIDE_PASSES`
needs `AtDispatchBoundary`, which Apple GPUs do not offer. So a timestamp can be written at the
beginning and end of a **compute pass**, and on the encoder between passes, but never between two
dispatches inside one pass. Today a chain runs every stage in one pass. Per-stage timing therefore
means one pass per stage, in timing mode only. The journal (M8.3b) already established that a
sabotage putting each dispatch in its own pass leaves every test green: the split changes no pixel.

## Design

### 1. The feature

`GpuContext::new` requests `TIMESTAMP_QUERY | TIMESTAMP_QUERY_INSIDE_ENCODERS` intersected with
what the adapter offers, and records `timestamps: bool` and the queue's `get_timestamp_period()`.
Requesting a feature the adapter has cannot make `request_device` fail where it succeeded before.

### 2. The switch

`RUSTROOM_DEBUG_GPU_TIME=1` turns timing on. `GpuChain` carries a `timing: bool`, read from the
environment at construction, with `set_timing(bool)` so a test can turn it on for one chain without
touching the process environment. Timing is off, and the code path is byte-for-byte today's, when
the flag is unset or the feature is absent. A device without the feature prints one line saying so,
once per process.

### 3. What is recorded

A new module `crates/gpu-pipeline/src/timing.rs` owns a `Recorder`: a `QuerySet` of timestamps, a
resolve buffer (`QUERY_RESOLVE | COPY_SRC`) and a mappable copy (`MAP_READ | COPY_DST`), plus the
labels in the order the passes were opened. `GpuChain::run_with`, in timing mode, records:

- **one compute pass per stage**, each with `timestamp_writes` at its beginning and end, labelled
  with the stage's name;
- the quantise pass, in bytes mode, with its own beginning and end.

After the last pass the encoder resolves the query set and copies the result beside the readback.
The mapped values are converted through the timestamp period into milliseconds.

**Amended after the first run (2026-09-26): the copies are NOT marked.** The design as approved
bracketed the upload and readback copies with encoder timestamps, which `TIMESTAMP_QUERY_INSIDE_ENCODERS`
promises and the M2 grants. In wgpu 29's Metal backend a `write_timestamp` outside a pass is
deferred onto a dummy blit encoder (its own source says "here be dragons"), the command buffer then
never reaches `Completed`, and `Device::wait` checks for nothing else, so `poll(Wait)` spins forever
on the render worker. With the two marks in, the timed-bytes test hung; without them it passes in
0.04 s. So a block lists the passes only, and the summary's `copies+latency` is the wall's remainder
beyond the passes: the device-side copies and the queue latency together. The copies' host halves
are already on the `RUSTROOM_DEBUG_XFER` line (`memcpy`, `tovec`); their device halves can be had
by differencing a resident-input run against a staged one, which is a probe rather than a mark.

### 4. What is printed

One block per chain, on stderr beside the existing `RUSTROOM_DEBUG_XFER` line:

```
GPUTIME display_rendering+output_transform 3200x2136
  display_rendering   3.41 ms
  output_transform    3.28 ms
  quantise            1.12 ms
  gpu span            7.81 ms   wall submit+wait 24.10 ms   copies+latency 16.29 ms
```

`gpu span` is the first pass's begin to the last pass's end on the GPU's clock; `wall` is the CPU
stopwatch the chain already keeps; `copies+latency` is their difference, the device-side copies and
the queue latency together (see §3's amendment). A `Timings` value with the same content is kept on
the chain (`last_timings()`) for the tests and for a future probe that wants a table rather than
text.

### 5. The handoff

A `Download::None` chain normally submits and returns without waiting. In timing mode it resolves,
copies, waits and reads before returning, so its numbers are per chain like every other's. This
serialises the two submissions the handoff usually overlaps. The untimed wall line is the control
for what that costs, and the block says `(waited for timing)` on such a chain.

### 6. Granularity, stated

The stage is the unit. A mask, sharpening, local contrast, NR, dehaze, lens and geometry each issue
several dispatches inside their own `encode`, which is handed a pass and cannot open another; each
reports one number. A prefix step is a one-stage chain, so it gets compute and transfer separately
for free.

### 7. The perturbation, stated

One pass per stage costs pass boundaries the real chain does not pay. Each block prints the wall
time of the timed run, and the untimed `RUSTROOM_DEBUG_XFER` line prints the wall time of the real
one; the difference is the cost of measuring. Nobody has to remember to compare, and no figure from
a timed run is quoted as a frame time.

### 8. Placement

All in `gpu-pipeline`: the feature request in `context.rs`, the `Recorder`, `Timings` and the
printing in the new `timing.rs`, the marks in `chain.rs`. `chain.rs` is 566 code lines; the marks
add a few lines each and the bookkeeping lives in `timing.rs` so it does not cross 600.
`probe_straighten_drag` gains a line in its header saying the flag prints the table; it already
replays a real drag through the real engine and needs no other change. Nothing in `app` changes.

## Tests

- A timed run and an untimed run of the same chain produce identical bytes (the pass split is
  inert). The fixture must first be shown to move pixels.
- With timing on, a three-stage chain's `last_timings()` holds three stage entries in stage order,
  each positive and each below the chain's wall time, plus the quantise entry in bytes mode; a
  sabotage that keeps the single pass reports one entry and fails the name list.
- With timing off, `last_timings()` is `None` and the chain issues no query set.
- The once-only "unsupported" line is a function taking `supported: bool`, tested with a counter.
- GPU tests skip without an adapter, as every test in the crate does.

## Out of scope

- Timestamps inside a mask's or a spatial stage's own dispatches.
- A timeline across chains within one frame, or across the prefix's chains and the tail's.
- Anything in the `app` crate, including `RUSTROOM_DEBUG_RENDER`'s lines.
- Any change to the untimed path.

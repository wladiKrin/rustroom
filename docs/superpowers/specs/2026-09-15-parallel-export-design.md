# M8.10 — a parallel batch export

**Date:** 2026-09-15
**Status:** approved, one plan

## 1. Why this, and why now

M8.9 measured a per-photo export as **~90% decode**: 10.78 s of an 11.4 s export at the `Best`
demosaic tier, against a render of 0.58–0.61 s. And `batch_export` runs **one photo at a time, by
design** — its own comment says why:

> The pipeline stages are already rayon-parallel *internally*, so exporting several photos at once
> would contend for the same cores while multiplying peak memory — a 26 Mpx decode is ~312 MB as
> f32 RGB. One at a time keeps the ceiling at roughly one image.

⚠⚠ **That reasoning is sound about the RENDER and wrong about the DECODE, which is the 90%.**
libraw's decode is essentially single-threaded, so a sequential batch leaves seven of eight cores
idle for almost its whole run. Measured, eight X-T4 RAWs at the default tier (⚠ load ~4.3, **not a
quiet machine**):

| width | total | vs sequential | peak decode buffers |
|---|---|---|---|
| 1 | 60.40 s | 1.02× | 298 MiB |
| 2 | 32.57 s | 1.88× | 596 MiB |
| 3 | 25.16 s | 2.44× | 894 MiB |
| **4** | **19.52 s** | **3.14×** | **1192 MiB** |
| 6 | 18.47 s | 3.32× | 1788 MiB |
| 8 | 13.47 s | 4.55× | 2385 MiB |

⚠ The 3 and 6 rows are confounded by tail effects at eight files — width 6 runs 6 then 2, leaving
four workers idle for the last round. The clean points are the widths that divide 8: **1.02 / 1.88 /
3.14 / 4.55×**, roughly `width^0.73`.

⚠⚠ **It does not flatten, so there is no knee to hide behind: this is a straight memory-for-time
trade.** That is why §3 makes it a setting rather than a constant.

**For scale: M8.9's entire GPU port bought under 5% of an export.** This is 3.14× at the default.

## 2. ⚠⚠ Decode-ahead is NOT the design, and the numbers say so before any code

The obvious shape — decode photo *N+1* while rendering photo *N* — costs one extra buffer and
**buys almost nothing here**: the render is 0.6 s against a 7.7 s decode, so hiding it saves ~7%.
The win requires **genuinely concurrent decodes**. Recorded so nobody re-derives the cheap idea and
wonders why it disappointed.

## 3. The shape

### 3.1 Two phases, and the split is what makes the second one simple

**Phase 1 — plan, sequential, no decoding.** For each job: resolve the metadata, render the filename
template, check `claimed` and `dest.exists()`, and settle any collision. All of that is a `stat` per
photo — milliseconds for a hundred. It yields a `Vec<PlannedExport>`; a skipped photo is dropped
here and never reaches phase 2.

**Phase 2 — N workers, each doing a whole photo.** A shared `AtomicUsize` cursor over the planned
list; each worker takes the next index and does decode → render → write → embed for it.

⚠ **Not a producer/consumer split.** A pool of decoders feeding a single renderer is the tempting
shape and it buys nothing: the render is 8% of a photo, so a worker that renders its own is idle for
that 8% rather than handing work over, and there is no channel, no buffering and no second place for
a 298 MiB buffer to be alive. ⚠ It also means **peak memory is N whole photos**, which is the honest
accounting and what §3.3 bounds.

⚠ **A shared cursor rather than static chunks**, `ThumbLoader`'s rule for `ThumbLoader`'s reason: a
selection mixes RAW and JPEG whose decode costs differ by an order of magnitude, so chunking would
leave workers idle.

### 3.2 ⚠⚠ Collisions are resolved UP FRONT, which is what makes the parallel phase decision-free

Today the worker **blocks** on a decision channel mid-run — it asks about photo 3 while 4–20 wait.
Concurrently that is a stall behind one dialog, and a state that has to be reasoned about per photo.

Settling every name before any decode starts removes the decision from phase 2 entirely, and it is
a better shape for the person exporting: answer once — or once with *apply to all* — and walk away.

⚠ **The visible change, stated rather than glossed:** prompts arrive before anything starts rather
than as the run goes. User's call, 2026-09-15.

⚠ `{seq}` is assigned in phase 1 from the job's **position in the list**, exactly as today, so a
skipped photo still does not renumber its neighbours.

### 3.3 ⚠⚠ The width is a Settings value, because the binding constraint is RAM and nothing here can read it

`Settings.export_concurrency`, **default 4**, clamped to `1..=available_parallelism()`.

⚠ A constant was rejected on the curve: the range 2–8 spans **596 MiB to 2.4 GB** and **1.88× to
4.55×**, which is far too much spread for one number to serve a 8 GB laptop and a 64 GB desktop.
⚠ Deriving it from the **core count** was rejected for a sharper reason: it scales with the wrong
thing. The constraint is memory, and nothing here can read how much is free.

⚠ Clamped at the read, `Settings::panel_order`'s rule — the file is hand-editable, so `0` must
repair to 1 rather than hang, and 64 must repair to the core count rather than allocate 19 GB.

### 3.4 What each event now means

- `Started { total }` — the **planned** count, after skips. Unchanged in shape, narrower in meaning.
- `Item { done, total, name }` — `done` is a **count of completions**, not a position, and `name` is
  whatever just finished. ⚠ **Out of order, and honestly so**: with four in flight the fourth photo
  can finish before the second.
- `Collision` — now only ever sent during phase 1.
- `Finished(report)` — the report is merged from the workers.

### 3.5 Cancellation is unchanged in spirit and wider in effect

Each worker checks the flag before starting a photo **and again after its decode**, exactly as the
sequential loop does — "abandon, don't abort", because libraw's `unpack` cannot be interrupted.

⚠ **The change: cancelling now abandons up to N photos' decodes rather than one.** That is up to
~30 s of background CPU on a four-wide run, spent and discarded. Inherent, and the same trade the
sequential version already makes for one.

## 4. What this does NOT do

- ⚠ **It does not make a single export faster.** One photo is one decode; this is about a batch.
- ⚠ **It does not touch the decode's own cost**, which is ~93% libraw's demosaic. The **tier** is
  the other lever and it is a dropdown: `Best` 10.78 s against `Fast` 2.82 s, and M8.4's own
  validated comparison found `Better` and `Best` *"indistinguishable at 4× nearest-neighbour"*. The
  two compose — `Fast` at four-wide would take a four-photo batch from ~45 s to under 4.
- ⚠ It does not parallelise the **preview**, which decodes one photo at a time because that is what
  a person is looking at.

## 5. The tests that matter

⚠⚠ **Concurrency must be asserted structurally, never by wall-clock.** `ThumbLoader`'s own test is
the precedent and the reason: it asserts **peak simultaneous decodes**, which is deterministic on
any machine, where a timing assertion is a CI flake waiting to happen — this project has three on
record.

1. **The carrier**: the same jobs at width 1 and width 4 produce the **same files**, and the
   width-4 run reaches a peak concurrency above 1. Either half alone passes against a bug.
2. **No decode starts before every collision is answered** — the property §3.2 exists for, and the
   only thing that can see a decision leaking into phase 2.
3. **Progress reaches `total`** with completions counted rather than positions.
4. **Cancellation** stops the run and leaves the in-flight photos unwritten.
5. **The width repairs**: 0 → 1, and past the core count → the core count.

## 6. Accepted caveats

1. **Peak memory is N whole photos** — ~1.2 GB of decode buffers at the default, plus their
   renders. That is the trade, and the setting is how it is made.
2. **Progress is out of order.**
3. **Prompts come up front.**
4. **Cancelling abandons up to N decodes.**
5. **A single export is unchanged.**
6. The renders now contend with the decodes for the same rayon pool — which is why the measured
   3.14× is on decodes alone and the end-to-end figure will be a little lower. ⚠ **The plan's last
   task measures the real thing**, and this spec carries no predicted end-to-end ratio: composing
   component measurements has missed in this crate three times.
7. One machine, one camera, one demosaic tier, and the bench machine was **not quiet**.

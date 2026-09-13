# M8.3q — noise reduction on the GPU

**Status:** design, 2026-09-13. Item 2 of the agreed systematic order in `CLAUDE.md`.

**One line:** `NoiseReductionStage` gets a GPU twin behind the existing `accelerate_prefix_stage`
seam — the second acceleration outside the tail, and the cheaper case of the guided filter M8.3o
already shipped.

---

## 1. Why this, and the headline is a drag rather than a stage

`CLAUDE.md`'s next-action block says NR is *"~84 ms at a native rung"*. Measured before writing a
line of this spec — `bench_preview` on `DSCF6296.RAF`, native rung 3123×2085 (6.5 Mpx), Apple M2,
⚠ **load average 5.43, so not a quiet machine**, which biases the CPU column and makes any ratio
computed from it slightly optimistic:

| | CPU |
|---|---|
| `noise_reduction` ALONE (whole buffer) | **86.1 ms** |
| `dehaze` ALONE (whole buffer) | 98.6 |
| **drag `noise_reduction` (resume before it)** | **216.4 ms** |
| drag `dehaze` (resume before it) | 132.0 |
| tail slider, nothing in the prefix re-runs | 34.3 |

**86.1 ms confirms the ~84 ms estimate** — unusually, a number in that file that held.

⚠⚠ **But the row that matters is 216.4 ms, and the next-action block does not carry it. An NR drag
is the most expensive drag anywhere in the prefix — half again more than a Dehaze drag — because
noise reduction is the *first* prefix step, so moving its slider re-runs NR *and* dehaze *and* the
tail.** Dehaze got the cheaper half of the seam it built. That is a better argument for this
milestone than "every primitive already exists", and it is the row this spec leads with.

⚠ **No prediction of the post-milestone drag time appears in this document.** Composing dehaze's
measured 1.47× onto NR's 86.1 ms is exactly the error this crate has made three times — M8.3g
over-predicted a whole frame by ~2×, M8.3f was right in magnitude and wrong by 8× in attribution,
M8.3o was over-optimistic by ~18%. The last task of the plan re-measures the real thing, and that
number is the milestone's result.

---

## 2. Scope

**In:** the global `NoiseReductionStage`, accelerated per step across `accelerate_prefix_stage`.

**Out, and each for a stated reason:**

- **Masked NR** — it runs as its own prefix step through `capture_projection`. M8.3o left masked
  dehaze untouched for the same reason and the agreed order puts mask work at item 4. Decided with
  the user, 2026-09-13.
- **Chaining NR with dehaze into one round trip.** M8.3o priced it — per-step gives a Dehaze drag
  ~99 ms against chaining's ~113 — and chose per-step because chaining destroys the **post-NR
  checkpoint** that lets a Dehaze drag resume after noise reduction. ⚠ That section also says
  chaining *wins* an NR drag and *"becomes right if NR is ported **and** the checkpoint's value is
  re-measured"*. The first half is now true and the second is not, so this milestone leaves the
  seam per-step, as the next-action block instructs.
- **The resident GPU buffer across the prefix→tail seam** (`docs/BACKLOG.md`, head of
  `## Performance`) — that entry says plainly not to start it before the prefix is mostly ported,
  and the prefix will be 4 of 6 on the CPU after this.

---

## 3. What NR is, and why it is the cheap case

`NoiseReductionStage::apply` is four things:

1. **One Oklab round trip** — RGB → three planes `L`, `a`, `b`.
2. **Luminance**: `guided_filter(&lum, &lum, w, h, r, eps)`, then lerp `lum += (q − lum) · s`.
   ⚠ **Self-guided** (`p` *is* `guide`), which is exactly the case `spatial::guided_filter`'s
   pointer-identity shortcut slims to **two** statistic blurs. Dehaze needed the general
   cross-guided form and all six.
3. **Chroma**: `CHROMA_PASSES = 2` box blurs on each of `a` and `b`, then the same lerp with its
   own strength.
4. **Oklab → RGB**, clamped at 0.

Every primitive is already on the GPU: `oklab_wgsl::OKLAB_WGSL` since M8.3d, `box_wgsl::box_pass`
since M8.3n, and the self-guided filter shape from `local_contrast`'s negative-Texture path.
**Nothing new has to be written.**

**Radii, and which shader body each takes.** `box_wgsl::RUNNING_SUM_MIN_RADIUS` is 7.

| | full-res | at scale 1.0 | at a fit scale | body |
|---|---|---|---|---|
| luminance (`LUM_RADIUS_FULL`) | 6 | r=6 | r=1..3 | **naive**, at every rung |
| chroma (`COL_RADIUS_FULL`) | 8..32 | r=8..32 | r=2..8 | **running** at a native rung |

⚠ The luminance radius is **below the crossover at every rung the app can reach**, so the guided
filter always takes the naive gather — its cheap case, and the sweep in `box_wgsl`'s header says
that is the right side to be on at r≤6. Neither radius is near the crossover, so the exact value
of that constant is not load-bearing here either.

---

## 4. The plane layout, which is the one real design choice

A GPU scratch plane is `w·h·4` bytes. At a **native rung** (6.5 Mpx) that is 26 MB; at **full
resolution** on an X-T4 (6246×4170, 26 Mpx) it is **99.4 MiB**. ⚠⚠ That second number is why this
matters rather than being bookkeeping: **the prefix always runs whole-buffer**, so NR is the
*second stage ever* to allocate at full resolution, and the first one killed the render worker.

Three layouts were considered:

| | planes | at 26 Mpx | |
|---|---|---|---|
| A. store `L`, `a`, `b` and their originals, plus statistics and tmp | 8 | 795 MiB | simplest, worst memory |
| B. one seed pass, both halves live at once | 6 | 596 MiB | keeps `L` live across both halves |
| **C. two sequenced phases, `apply` recomputes Oklab** ✅ | **4** | **398 MiB** | **shipped** |

**C, in full.** `p0`..`p3`:

- **`seed_l`** — `p0 = L`, from `prophoto_to_oklab(img)`.
- **Luminance phase**, only when `luminance > 0`:
  - `square`: `p1 = L·L`, `copy_l`: `p2 = L`
  - blur `p1` and `p2` at `r_lum`, `p3` as tmp → `mean_gsq`, `mean_g`
  - `slope_offset`: reads both, writes `p1 = slope`, `p2 = offset`
  - blur `p1` and `p2` again → `mean_slope`, `mean_offset`
  - `combine_lum`: `p1 = p1·p0 + p2` — the guided result, with `p0` as the guide.
- **Chroma phase**, only when `color > 0`: `seed_c` writes `p0 = a`, `p2 = b` — planes the
  luminance phase has finished with — then two box passes on each at `r_col`, `p3` as tmp.
- **`apply_nr`** — **recomputes** `prophoto_to_oklab` from `img`, lerps each channel against
  `p1`/`p0`/`p2`, converts back, clamps at 0.

⚠ **The recompute is what buys the four planes.** Storing three originals costs four more planes
to save one `prophoto_to_oklab` per pixel — three `cbrt` and two matrix multiplies, against
398 MiB of allocation on the one stage in this crate where allocation is the known hazard. It is
**bit-identical by construction**: same input bytes, same function, and `img` is not written until
`apply_nr` itself.

⚠ `scratch_planes()` returns **4 unconditionally**, following `GpuDehazeStage`'s and
`GpuLocalContrastStage`'s precedent — a count that varied with the sliders would be a second place
the pass sequence's plane usage is decided, and the two could disagree.

⚠ `apply_nr` **branches on each strength being above zero** rather than relying on a plane holding
a harmless value. Buffers are zero-initialised today, but `0.0 · NaN` is `NaN`, and a lerp against
an uninitialised plane is the kind of thing that is correct until a driver changes. The CPU stage
branches on exactly the same two conditions.

---

## 5. Constants are substituted, never transcribed

`LUM_RADIUS_FULL`, `COL_RADIUS_FULL`, `EPS_AT_DETAIL_0`, `EPS_AT_DETAIL_100`, `CHROMA_PASSES`,
`eps_for_detail` and `chroma_radius_full` are private to `stages::noise_reduction` today. They are
re-exported from `pipeline` and **substituted into the WGSL**, the rule `dehaze` and
`local_contrast` already follow: a substituted constant *cannot* disagree, where a transcribed one
is merely checked for agreeing. `color`'s own doc records six copies of a Rec.709 luma triple that
all went wrong on one day.

⚠ `eps` and both radii are computed **CPU-side in `GpuNoiseReductionStage::new`** from the same
functions the CPU stage calls, and arrive as a uniform and as shader literals. That makes the two
paths identical by construction rather than by comparison — `dehaze`'s airlight reasoning, applied
to a cheaper quantity.

---

## 6. The guards

**Equivalence against the CPU stage**, on a noisy fixture, in **three configurations** — luminance
only, colour only, both — because `apply_nr` branches on each. Compared in 8-bit levels through
`equivalence::compare`, the standard this crate has used since M8.3k.

⚠ **Not bit-identical, and the spec says so in advance rather than discovering it.** NR inherits
all three recorded divergence classes: subnormal flush-to-zero (M8.3a), FMA contraction in the
lerp and in `combine_lum` (M8.3m), and the box blur's division (M8.3n).

**A raw-float test for the zero clamp.** `compare` quantises to 8-bit levels, where a negative
sample and 0 are the same value — so dropping `max(v, 0.0)` is invisible to it. This is the fourth
recorded instance of that trap (M8.3e's output transform, then `contrast`, `tone_curve` and
`saturation` in one day).

**A carrier for the seam.** `accelerate_prefix_stage("noise_reduction", ..)` must return `Some`,
and a name that is not in the table must still return `None`. Without the first, every equivalence
test passes against a stage the seam never offers — the W5 failure mode.

**Checkpoint sharing.** `a_checkpoint_written_by_either_path_is_a_hit_for_the_other`, M8.3o's
guard, extended to NR: render once on each path through one `PrefixCache` and assert the second
runs no prefix step. `PrefixStepKey` is derived from the **edits**, never from the stage, so this
should hold with no change to `PrefixCache` — and "should hold" is a claim until a test says it.

**Sabotages, each predicted to be caught by exactly one test:**

1. `eps` interpolated arithmetically rather than geometrically.
2. `CHROMA_PASSES` collapsed from two to one.
3. The variance clamp dropped from `slope_offset`.
4. The lerp inverted (`q + (dst − q)·s`).
5. The radius scaling dropped, so a fit render uses full-resolution radii.
6. The seam's arm removed.
7. The zero clamp removed — **raw-float test only**.

⚠ A sabotage that removes a `{placeholder}` leaves the `format!` argument unused and **fails to
compile, which reads exactly like a pass** (M8.3m's trap, hit again in M8.3o). Every sabotage here
changes a *value*, and the runner checks for a `test result:` line before believing anything.

---

## 7. Measurement

`bench_preview`'s native block already prints `noise_reduction ALONE (whole buffer)` and already
has a GPU comparison block for dehaze — it currently prints `— \`noise_reduction\` has no GPU
twin`. Filling that line in is the milestone's last task. `cpu_cost` ships at the **bottom** of the
measured band, the conservative direction: a lower number makes a step less likely to chain, and
the gate must fail toward the CPU.

---

## 8. Confirming by use

⚠⚠ **Start at 1:1 with noise reduction, because that is the configuration that killed the worker
when dehaze landed.** NR will be the second stage ever to allocate a full-resolution GPU image.
The gate in `GpuImage::new` and `guarded` make that a CPU fallback rather than a dead worker, but
it has been exercised by exactly one stage.

⚠ A scratch plane is a third the size of the image, so the existing `GpuImage::new` ceiling check
already implies the planes fit — worth stating, because `GpuImage::scratch` has no gate of its own
and that looks like an omission.

Then: drag **Luminance** and **Color** separately at a native rung with `gpu_tail` on, and check
the picture does not change. ⚠ **The half a still frame cannot catch is the chroma blur's border**
— `box_blur`'s window shrinks at an edge and the sum is divided by the count inside it, where the
alternative reading darkens a margin by roughly half. It is M8.3n's hazard arriving in a second
stage, guarded by that milestone's `a_constant_plane_stays_uniform`; treat a later report of *"a
band of colour at the edge of the frame with Color up"* as that test's territory.

---

## 9. Accepted caveats

1. **Only when you drag Luminance or Color** — the prefix is per-stage cached, so unlike a tail
   stage it costs nothing on any other slider. That is also why the ceiling is low.
2. **Masked NR is untouched.**
3. **Four scratch planes** — 104 MB at 6.5 Mpx, 398 MiB at full resolution.
4. **Not bit-identical** — three inherited divergence classes, §6.
5. **The prefix is still 4 of 6 on the CPU** after this, and the three resamplers are the largest
   number left anywhere (lens correction alone is ~210 ms and on by default for every RAW).
6. One machine, one adapter (Apple M2, Metal), and the bench machine was **not quiet**.
7. **`gpu_tail` is still off by default.**

# M8.5 — the sequential resamplers, in parallel

**Status:** design, 2026-09-13. A precursor to item 3 of the agreed systematic order, and it
changes what item 3 is.

**One line:** `geometry::rotate` and `PerspectiveStage::apply` are single-threaded gathers on an
eight-core machine; rayon over output rows recovers about **255 ms of a 529 ms resampling prefix**
for a few lines, with no GPU involved.

---

## 1. The measurement, which is the whole argument

`crates/app/examples/probe_resamplers.rs` (kept), `DSCF6296.RAF`, native rung 3123×2085
(6.5 Mpx), Catmull-Rom, 8 threads. ⚠ Load 3.99, so **not a quiet machine**.

| prefix step | cost | parallel today? |
|---|---|---|
| `lens_correction` | **212.8 ms** | ✅ rayon over output rows |
| `perspective` | **156.0** | ❌ **sequential** |
| `geometry` | **160.1** | ❌ **sequential** |
| **all three** | **528.9 ms** | |
| *(a bare clone, included in every row above)* | *2.0* | |

**And the row that decides it: geometry's rotation is 160.5 ms sequential against 31.2 ms
through rayon — 5.14×.**

⚠ `CLAUDE.md`'s standing "~210 ms" for lens correction is **confirmed** at 212.8. It is the one
figure in that block the measurement leaves standing.

---

## 2. What this reframes, and it is the next milestone rather than this one

The next-action block says *"the three resamplers — geometry, perspective, lens correction — as
ONE mechanism. Upload the displacement field as a two-channel texture."* Two corrections:

- **They are one KIND of problem and not one mechanism.** Lens correction samples a **displacement
  field** lensfun computes, **per channel** — TCA gives R, G and B different source points, which
  is exactly why `resample::sample_channel` exists. So its field is six components per pixel, not
  two. Perspective is an analytic **homography** and geometry an analytic **rotation**: a few
  arithmetic ops per pixel, wanting no texture at all.
- **⚠⚠ Two of the three do not need the GPU, they need eight cores.** After this change the
  resampling prefix is roughly **275 ms of which lens correction is ~77%**, so the honest next
  question is *"port lens correction"*, not *"port three resamplers"*.

⚠⚠ **And that is why this lands first rather than alongside.** Setting a GPU stage's `cpu_cost`
against code with a known 5× of fat in it is **the M8.3h staleness failure exactly** — that
milestone had to retune a constant because two later milestones made the quantity it described
cheaper and neither re-measured. M8.3n records the same rule as *"a CPU win rides along, and it is
a prerequisite rather than a bonus"*.

---

## 3. The change

**`geometry::rotate`** and **`PerspectiveStage::apply`**, each parallelised over **output rows**
with `par_chunks_exact_mut(stride)`, in the shape `LensCorrectionStage` already uses.

**⚠ Why that is sound, and why it needs no new test to establish it:** both are **gathers** — an
output pixel reads the whole source and writes only itself, so no two threads write the same byte
and none reads another's output. The result is therefore **bit-identical** to the sequential form,
which is what lets the **63 goldens stand as the check**. That is the argument
`LensCorrectionStage`'s own rayon port already records at its `for_each` site.

**⚠ `rotate` only, not "geometry".** `GeometryStage::apply` is orient → rotate → crop. `orient` is
an index permutation and `apply_crop` a sub-rect copy; both are cheap and neither is what the
160.1 ms is. Parallelising them would be machinery for nothing and would widen the diff the
goldens have to cover.

**⚠ The `stride == 0` guard comes along.** `par_chunks_exact_mut(0)` **panics**, and a zero-width
image reaches it. Already recorded twice in this repo — in `map_pixels_xy` and in
`LensCorrectionStage` — so this is a rule being applied, not discovered.

**⚠ `perspective.rs`'s comment gets honoured and then corrected.** It reads *"Sequential,
mirroring `geometry::rotate` … If this ever needs to be faster, both should move together."* They
move together here, and the comment must stop describing a sequential stage — a comment that
contradicts the code beneath it is the fault `LocalAdjustments`' doc was corrected for.

---

## 4. What is measured, and what is asserted

**⚠⚠ Perspective's parallel form is measured DIRECTLY, not inferred from geometry's 5.14×.** They
are the same shape — a sequential gather calling `resample::sample` — and that is precisely the
reasoning by analogy this crate has been wrong about three times. If it comes back materially
different, that is a finding about the homography's arithmetic rather than noise.

**The acceptance is bit-identity, not a tolerance.** The 63 goldens must stay **byte-identical**,
and the test count must be **unchanged** — this changes when work happens, never what it computes.
⚠ "Still green" is not the assertion; "unchanged at 2428" is.

⚠ A **5.14× on eight cores is plausible but is above what a memory-bound gather usually reaches**,
so it is worth one sentence rather than silent acceptance: if the speed-up came from doing *less*
work it would show as a moved golden. The goldens are therefore the guard on the number as well as
on the pixels.

---

## 5. Accepted caveats

1. **Nothing here helps a photo with no straighten and no perspective** — both stages are skipped
   entirely when inactive, so this is paid only by photographs that use them.
2. **Lens correction is untouched** and remains the largest single number in the prefix at
   ~212.8 ms; it is already parallel, so the only lever left on it is the GPU.
3. **The prefix is still whole-buffer**, so these costs are paid on every rung change and every
   prefix-slider drag, as before.
4. **More contention with the render pool**, in principle: three prefix stages now compete for the
   same rayon pool the tail's stages use. They run sequentially within one render, so this is a
   statement about a busy machine rather than a new race.
5. One machine, one adapter, one photograph, and the bench machine was **not quiet** (load 3.99).

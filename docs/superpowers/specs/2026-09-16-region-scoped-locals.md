# M8.12 — the region-scoped locals actually scope

**Status:** spec · 2026-09-16
**Found by:** measuring the unexplained 118 ms scoped dehaze drag recorded at the head of
`docs/BACKLOG.md` `## Performance`.

## The defect, in one line

`apply_region` for **noise reduction** and **dehaze** computes the **whole source buffer** and
crops the answer out of it, so a region-scoped render of a local stage does full-frame work.

```rust
// crates/pipeline/src/stages/dehaze.rs:301  and  noise_reduction.rs:132 — identical
let done = self.apply(src.buf.clone());
Some(done.crop(ox, oy, out.w, out.h))
```

## The evidence

Measured through the real engine with the per-step instrumentation added in this branch
(`RUSTROOM_DEBUG_RENDER`, `bench_nr_checkpoint`, `DSCF6296.RAF`, frame 3123×2085 = 6.51 Mpx,
viewport region 1041×695 = 0.723 Mpx).

| Case | Stage | Its **input** | Measured |
|---|---|---|---|
| drag NR — 2-step plan | NR as **step 0** | whole **6.51 Mpx** | **93–96 ms** |
| | dehaze as step 1 | carried **0.986 Mpx** | **18 ms** |
| drag dehaze — 1-step plan | dehaze as **step 0** | whole **6.51 Mpx** | **101–105 ms** |

**The same stage, the same 1041×695 output, the same 1.00× margin, 5.8× apart.** Input areas differ
by 6.6×; the times track the **input**, not the output.

⚠⚠ **The corroboration is exact: scoped NR measures 93–96 ms and whole-frame CPU NR at 6.51 Mpx
measures 92.4–95.0 ms.** They are the same number because it is the same work.

## Why the first step is the one that pays

`run_region_plan` hands step 0 `RegionSrc::whole(src)` — the entire buffer — and every later step
the carried buffer, which is already cropped to the previous step's output rect. So a **local**
stage pays full-buffer cost whenever it is first, and **a one-step plan on a local stage saves
nothing at all**. A drag resumes at the checkpoint immediately before the dragged stage, so
one-step plans are the common case, not the corner case.

## ⚠⚠ Why no test caught it

`perspective` and `lens_correction` — the **gathers** — each carry a region-equivalence test
(`a_region_of_the_perspective_matches_the_whole_perspective_cropped`,
`a_region_matches_the_same_part_of_the_whole`). **`dehaze` and `noise_reduction` carry none.**

With apply-whole-then-crop such a test is **vacuously true**: the two paths agree byte-for-byte
because they are literally the same computation. The journal's own line — *"the two paths agree
byte for byte by construction — `apply` **is** `apply_region` at …"* — records the tautology as if
it were a guarantee. **It is the guard-that-cannot-fail shape this project already has a standing
lesson about.**

## Goal

Make `apply_region` for both locals do work proportional to the **margined input rect** rather than
to the whole buffer, with the result **byte-identical** for every pixel in `out`.

## Non-goals

- **No GPU work.** `accelerate_prefix_stage`'s twins run whole-buffer; a scoped chain stays the
  CPU's. That coupling is real but it is a separate milestone.
- **No change to `region_plan`, `run_region_plan` or the area gate.** The rects it computes are
  already correct — only the stages misuse them.
- **No change to the airlight.** It is supplied frame-globally and must stay that way; a stage that
  estimates from the slice in hand is the failure `apply_region`'s `None` default exists to prevent.
- Not the other three stages. `geometry`, `perspective`, `lens_correction` are real gathers already.

## Design

In each local's `apply_region`, crop the input to what the stage actually needs **before** applying:

```rust
let need = self.input_rect_for(out, src.full.0, src.full.1)?;   // full-image coords
let lx = need.x.checked_sub(src.origin.0)?;                      // into src.buf
let ly = need.y.checked_sub(src.origin.1)?;
if lx + need.w > src.buf.width() || ly + need.h > src.buf.height() {
    return None;                                                 // fail toward the whole-buffer path
}
let done = self.apply(src.buf.crop(lx, ly, need.w, need.h));
Some(done.crop(out.x - need.x, out.y - need.y, out.w, out.h))
```

`input_rect_for` already exists on both stages and already grows by `kernel_reach` on every side.

## The correctness argument, and the hazard it must survive

⚠⚠ **`box_blur`'s window SHRINKS at a buffer edge.** So `apply` on a cropped slice does *not* equal
`apply` on the whole buffer everywhere — it differs within `kernel_reach` of the slice's own edge.
The margin exists precisely so those differing pixels fall **outside** `out`:

- `need` = `out` grown by `kernel_reach` on every side.
- A pixel inside `out` is at least `kernel_reach` from `need`'s edge, so its full window lies inside
  `need` and holds the same samples it holds in the whole buffer.
- Therefore every pixel of `out` is byte-identical; every pixel that differs is cropped away.

**That argument is exactly what the existing docstring already claims `region_plan` guarantees. It
has simply never been exercised, because the implementation never took a margined slice.**

⚠ Where `need` is clamped by the buffer's real edge, the shrinking window is what the whole-buffer
path does there too, so the two still agree.

## Done when

1. Both locals crop to `need` before applying.
2. **A real region-equivalence test exists for each** — `apply_region` over an interior rect agrees
   with `apply` over the whole buffer cropped to the same rect. ⚠ The rect must be **interior**, far
   enough from the frame edge that the margin is not clamped, or the test proves nothing about it.

   ⚠⚠ **CORRECTED DURING EXECUTION: "byte-identical" above was WRONG, and this spec made the error
   the journal already records a previous spec making.** `box_blur` keeps a running sum, so it
   reaches each position by a different history of adds and subtracts depending on where the row
   starts — see `a_chain_with_a_local_agrees_to_within_a_rounding`. **The criterion is agreement to
   within a rounding**, with the bound sized per stage from measurement at both ends:

   | stage | measured normal | margin deleted | bound | why |
   |---|---|---|---|---|
   | noise reduction | 4.8e-6 | 7.3e-4 (150x) | 1e-5 | the sibling test's bound |
   | dehaze | 9.9e-5 | 5.2e-2 (527x) | 1e-3 | divides by `T_MIN` 0.1, amplifying a wobble ~10x |

   ⚠ Measured against a **larger** fixture the divergence does **not shrink as the margin widens**
   (NR 9.7e-6 / 1.1e-5 / 8.8e-6 at 1x/2x/3x), which is what rules out an under-sized `kernel_reach`
   and leaves rounding as the explanation.
3. **A sabotage step:** deleting the `kernel_reach` growth from `input_rect_for` must make those new
   tests FAIL. A test that passes with the margin removed is testing nothing.
4. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code read from its own command**.
5. **63 goldens ran and none moved.**
6. Re-measured through `bench_nr_checkpoint` with `RUSTROOM_DEBUG_RENDER`: a one-step scoped dehaze
   drag falls from ~104 ms toward the ~18 ms the two-step case already achieves. ⚠ Quote a band over
   at least 3 runs, and say the machine's load.

## Risk

**The failure mode is a plausible photograph, not an error** — a seam or a band at the region's edge
during a pan. Item 3's sabotage is what stops this shipping green and blind, and a still frame will
not catch it: the check is a **pan at 1:1** with NR and dehaze both active.

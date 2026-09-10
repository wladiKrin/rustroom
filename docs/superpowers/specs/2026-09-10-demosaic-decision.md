# M8.4 — the demosaic decision

**Date:** 2026-09-10
**Status:** decided. **Replacing libraw's demosaic is OUT of scope. The default tier moves `Fast` → `Better`.**

M8.4 asked for "an explicit in-or-out-of-scope decision written down, with a foliage
comparison behind it". This is that decision and that comparison.

---

## 1. The question, and why it was open

Two facts had been recorded and neither had been acted on.

**(a) Demosaic is ~93% of a full-resolution decode, and the `wgpu` port would not touch a
millisecond of it.** That time is inside libraw's `dcraw_process`, upstream of every stage
`wgpu` would port. So "the GPU port will fix decode times" was never true.

**(b) The `Fast` default was validated on the wrong content.** It was chosen by comparing
*hair-strand separation* against `Best` on a kitten frame — and X-Trans demosaic does not fail
on hair. It fails on **foliage and fine repeating texture**, with the "worms" artefact.

## 2. The measurement

`crates/app/examples/demosaic_foliage.rs` (kept). Four X-T4 frames named by the user, each
decoded at all four tiers, **rendered through the real pipeline at default edits**, and
compared inside a hand-picked **stress** region with a flat **control** region beside it.

Numbers are 8-bit display levels against `Best`.

| frame | tier | stress mean | stress p99 | control mean | decode |
|---|---|---|---|---|---|
| **6291** bushes / sign face | Fastest | 2.71 | 11.5 | 2.15 | 1.3 s |
| | Fast | 2.27 | 9.2 | 1.87 | 2.9 s |
| | **Better** | **0.30** | 3.4 | **0.08** | 7.4 s |
| **6226** treeline / sky | Fastest | 1.82 | 6.1 | 1.19 | 1.3 s |
| | Fast | 1.57 | 5.1 | 1.14 | 2.8 s |
| | **Better** | **0.22** | 2.5 | **0.15** | 7.4 s |
| **6377** grass / sky | Fastest | 3.94 | 15.6 | 2.50 | 1.4 s |
| | Fast | 3.27 | 12.8 | 2.47 | 3.0 s |
| | **Better** | **0.29** | 4.5 | **0.32** | 7.5 s |
| **6400** birch / wall | Fastest | 3.21 | 19.5 | 2.62 | 1.4 s |
| | Fast | 2.49 | 13.1 | 2.08 | 3.0 s |
| | **Better** | **0.22** | 3.1 | **0.23** | 7.5 s |

`Best` is the reference and reads 0.000 everywhere by construction.

### ⚠⚠ The control column is what the decision rests on, and the stress column alone is misleading

`Fast` differs from `Best` by **1.1–2.5 levels on flat sky and flat rendered wall**. That is
not "Fast loses fine detail" — it is VNG rendering the whole image differently, sensor noise
included. Its stress number is only ~1.2–1.6× its control, so most of that difference is not
about foliage at all.

`Better` has the opposite shape: on 6291 it is **0.30 on the bushes against 0.08 on the sign
face**, a 3.5× concentration. A difference that lives only where detail lives is the signature
of a real demosaic difference, and only `Better` has it.

**A stress-only table would have ranked the tiers in the same order and for the wrong reason.**
This is the third time in this project a summary statistic has pointed the wrong way — after
the dehaze probe's "haziest frames" (which found flat lilac walls) and the colour-grading work's
whole-image mean (which cancelled a split tone against itself).

### What the pictures show

4× nearest-neighbour montages, `Fastest | Fast | Better | Best`, in `/tmp/m84/crops`:

- **6291** — a thin twig through the centre is a distinct line in `Better`/`Best` and a smeared
  blob in `Fastest`/`Fast`.
- **6226** — the distant treeline keeps individual leaf-clump speckle in `Better`/`Best` and
  goes to mush in the first two.
- **6400**, **6377** — same direction, less dramatic.
- `Better` and `Best` are **indistinguishable at 4×**.

## 3. The decision

**Out of scope: replacing libraw's demosaic.** Doing it means Markesteijn/AMaZE-class work on
X-Trans — a project in its own right, whose errors surface as exactly the fine-detail artefacts
one zooms in to inspect. The measurement gives no reason to: `Better` already sits within
0.22–0.30 of an 8-bit level of `Best` on the content the concern was about.

**In scope, and done: the default tier moves `Fast` → `Better`.** The original concern was
correct — `Fast` does lose real foliage structure — and the fix costs a tier change rather than
a demosaic implementation.

### What it costs

Opening a photo goes from ~2.9 s to ~7.5 s of background decode on an X-T4. That is paid
**eagerly, on every photo you stop on**, not once per export, so it is a real trade and not a
free upgrade. It was made knowingly: the full-resolution decode already runs off the UI thread
behind a settle delay, the old buffer stays on screen throughout, and the tier remains a
`Settings` choice for anyone who wants the speed back.

### Where the default lives

`Demosaic`'s `#[default]`, and now **only** there. It previously lived in four places at once —
the enum plus three hardcoded `Fast` literals in `DecodeOptions::{linear_scene, display,
preview}` — so moving it would have left three of them quietly saying the old thing while still
reading as "the default". They derive from `Demosaic::default()` now. The CLI's
`--demosaic` default moved with it.

It feeds **both** `Settings.view_demosaic` (the 1:1 view) and `ExportOptions.demosaic` (export),
and both move together. That is supported by the same measurement: these are full-resolution
decodes in both cases.

⚠ **Anyone whose `settings.json` already holds `"Fast"` keeps it.** A stored value beats a
changed default — the trap recorded under the panel-section work — and here that is the safe
direction: nobody's open times change without them asking.

## 4. Caveats on the measurement itself

1. **6377 is soft at the sample point.** That frame appears slightly out of focus or
   wind-blurred there, so it has less fine structure to lose than its large numbers suggest.
2. **These are full-resolution decodes.** The preview usually renders downscaled, which
   attenuates a per-pixel artefact, so the difference while editing is smaller than the crops.
3. **Four frames, one camera, one photographer's taste.** The tier ordering is not in doubt;
   the exact crossover between "visible" and "not" is a judgement about photographs.
4. **The first version of this probe was wrong twice**, and both faults are recorded in the
   example's own doc comments so they are not repeated: an automatic "busiest window" picker
   landed on road-sign lettering rather than foliage, and the metric and the saved crops were
   computed in two different colour spaces — neither of them the one a judgement is made in.

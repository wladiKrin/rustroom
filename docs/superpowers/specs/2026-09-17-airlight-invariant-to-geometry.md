# The dehaze airlight becomes invariant to geometry

**Status:** spec · 2026-09-17
**Measured by:** `crates/app/examples/probe_masked_airlight.rs`, committed on this branch.

## The defect, in two halves

The airlight is the mean of the haziest 0.1% of pixels — an **order statistic over the frame**. It
therefore moves whenever the frame's content moves, and a Straighten drag moves it every frame.

Measured across a 0–6° drag, in 8-bit levels:

| photo | GLOBAL (the M8.7 replay) | MASKED (estimated per frame) |
|---|---|---|
| `DSCF6291` | **6.06–6.81** | **9.19–10.70** |
| `DSCF6296` | **1.47–1.71** | **10.70–11.13** |
| `DSCF6509` | 0.03–0.08 | 0.05–0.06 |

**1. ⚠⚠ `docs/BACKLOG.md` IS WRONG ABOUT M8.7, AND THE ERROR IS A WORD.** It says the replay
airlight *"does not move during a drag at all"*. That conflates **deterministic** with
**invariant**: it is a deterministic function of the **edits**, and a Straighten drag changes an
edit on every frame. `prefix_airlight` replays the prefix **including geometry**, so the angle feeds
straight into it. M8.7 took the wobble from ~21 levels to ~6; it did not remove it.

**2. A mask's capture half gets no airlight at all.** `MaskStage::new_capture` builds through
`capture_stages`, which calls `DehazeStage::new`. It estimates from its own input every frame, and
moves **more** than the global path.

⚠ **`DSCF6509` is why one photograph is not an answer**: a dark scene whose airlight sits at ~0.113
barely moves. Had it been the only frame measured, the item would have closed as a non-issue.

## Goal

**The airlight becomes a property of the scene rather than of the framing.** Straightening,
cropping, correcting perspective or applying a lens profile must not change it — on either path.

## Design

**1. The replay skips the resampling family.** In `prefix_airlight`, skip steps keyed
`LensCorrection`, `Perspective` and `Geometry` while replaying. `SpotRemoval` and `NoiseReduction`
stay: neither reframes, and both are part of what dehaze actually sees.

**2. The masked path is supplied the same value.** Thread `airlight` through
`MaskStage::new_capture` into `capture_stages`, so a mask's dehaze is built with
`DehazeStage::with_airlight` exactly as the global one is. The value is already in scope at the
call site in `build_prefix_steps`.

## ⚠⚠ The trade, stated rather than discovered

**Cropping into a different part of the scene will no longer re-estimate the airlight.** A crop into
a hazy sky and a crop into a clear foreground will share one value. That is not a side effect — **it
is what invariance means**, and the alternative is the wobble this milestone exists to remove. A
crop drag is continuous too, so re-estimating on crop would reintroduce the defect in the one
gesture most likely to expose it.

⚠ This is a **one-off change to what every dehazed-and-cropped photograph renders as**, in the
family of M8.7's accepted 5.6-level shift. It is not a regression; it is the point.

## Done when

1. `prefix_airlight` skips the three resampling keys, and the skip list is written **once**.
2. `capture_stages` takes an airlight and passes it to `DehazeStage::with_airlight`.
3. **A test that the airlight is unchanged by angle, crop, perspective and a lens profile** — same
   edits otherwise, several geometries, one value.
4. **A sabotage step:** putting `Geometry` back into the replay must make that test FAIL.
5. **⚠ A checkable prediction: NO golden moves.** All three dehaze goldens — `dehaze_plus`,
   `dehaze_minus`, `mask_dehaze` — use `Edits::default()` geometry, so skipping a no-op step is
   identity for them. **If a golden moves, this change did something it was not asked to do**, and
   that must be understood before it is accepted rather than blessed by regenerating the reference.
6. Re-run `probe_masked_airlight` on all three photographs: **both rows at or near 0.00 levels**.
7. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code from its own command**, 74 goldens ran, gated commit verified to BE the tip.
8. ⚠ Confirmed **by use**: dehaze a hazy photograph, then drag Straighten. The dehazed tone must
   not breathe as the angle moves.

## Risk

The failure mode is a **plausible photograph**: a dehazed frame that is subtly wrong everywhere
rather than visibly broken. Item 4's sabotage and item 5's golden prediction are what stop it
shipping green, and item 8 is what stops it shipping unseen.

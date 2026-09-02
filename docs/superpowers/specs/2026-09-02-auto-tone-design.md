# M5.4a — Auto Tone

*Design, 2026-09-02. Wave 3, track C.*

One button that sets **Exposure, Whites and Blacks** from the photograph's own histogram. The
predictable, levels-style auto — not a look.

⚠⚠ **This is 5.4a only.** 5.4b — a default camera rendering, and the question *"where does a
display rendering live in a scene-referred pipeline?"* — is the highest-leverage design decision on
the board and is brainstormed in the main session, never by an agent (`docs/PARALLEL-WORK.md` §6).
Do not touch it, and do not let auto tone grow into it.

---

## 1. What it sets, and what it leaves alone

**Sets:** `exposure`, `whites`, `blacks`. **Leaves alone:** contrast, highlights, shadows, and
everything else. Three values that mean "put the black point, the white point and the middle where
they belong" are explainable and undoable; a five-slider auto is a taste the user did not ask for.

**Not applied on open** — Lightroom does not either. A button, in the Tone section, that writes
the three sliders and marks the edit **once**, so it is one Ctrl+Z: the model the white-balance
presets already use (`wb_tool::apply_preset` → `ed.mark_changed()`).

---

## 2. The analysis — `pipeline::auto_tone`

```rust
pub struct AutoTone { pub exposure: f32, pub whites: f32, pub blacks: f32 }
pub fn analyse(img: &ImageBuf, crop: Option<CropRect>) -> AutoTone;
```

Pure, no I/O, no egui, in `crates/pipeline/src/auto_tone.rs`. On luminance via `color::luminance`
(the working space's Y — the one every tuned constant in this pipeline is measured against).

1. **Restrict to the crop.** Pixels outside `crop` are not the photograph. Rotation is ignored
   (the crop rect on the unrotated frame is close enough for percentiles) — say so in a comment.
2. **Percentiles**: `p_lo` at 0.1 %, `p_hi` at 99.9 %, and the **median**, of luminance over the
   region. A sorted sample of at most ~250k values is plenty; do not sort 26 Mpx.
3. **Exposure** = `log2(MEDIAN_TARGET / median)`, clamped to **±2.5 EV**. `MEDIAN_TARGET` is a
   named constant, a *starting value* the user tunes after seeing results (§4) — start at **0.16**
   linear, a little under 18 % grey, because photographs' medians sit below mid-grey.
   ⚠ Guard `median ≤ 0` (a black frame) → exposure 0, never `inf`.
4. **Whites and blacks, solved against the real stage.** Do **not** write a formula for what the
   sliders do — invert the stage itself. With exposure applied (`v · 2^EV`), find by bisection the
   `whites` in `[-100, 100]` for which `ToneRegionsStage` maps `p_hi` to `WHITE_TARGET` (0.98), and
   the `blacks` for which it maps `p_lo` to `BLACK_TARGET` (0.01). Each solved independently with
   the other at 0 (the interaction is small and accepted). The stage's mapping is monotone in each
   slider, which is what makes bisection valid — assert that in a test rather than assume it.
   ⚠ **This is the "one rule" principle**: the auto agrees with the sliders by construction, and
   if the tone-regions model ever changes, the auto follows it with no edit here.
5. Everything clamped to the sliders' own ranges.

**The buffer it reads:** `Session.buffer` — the `Arc<ImageBuf>` the white-balance eyedropper already
reads, **not** the rendered texture (which already has the current tone applied and would make the
auto chase its own correction; the eyedropper section records this trap). Downscale to **512 px**
first: percentiles of a 512 px downscale differ negligibly from full resolution, and it keeps the
button instant on a 26 Mpx buffer. ⚠ **Not** white-balanced first: WB is a luminance-normalised
gain, so it moves the percentiles by well under the ±2.5 EV clamp's resolution. Say so.

---

## 3. The button

In `develop/panel/adjustments.rs`, the Tone section: an **"Auto"** button beside the section's
first slider, hover text *"Sets Exposure, Whites and Blacks from the histogram"*. It reads
`session.buffer`, calls `analyse` with `edits.geometry.crop`, writes the three fields,
`ed.mark_changed()`. The panel body runs sixty times a second — the analysis runs **on the click
only**. Thread the buffer in the way `white_balance` receives `baseline`: a parameter, not a
global.

---

## 4. Signing off — you produce the evidence, the user decides

The "done when" is a judgement about a photograph, which `PARALLEL-WORK.md` §6 says an agent must
not sign off. So the deliverable includes a **kept** example,
`cargo run --release -p app --example auto_tone_probe <raw> [out_dir]`, that writes before/after
JPEGs at fit resolution and prints the chosen exposure/whites/blacks and the three percentiles. Run
it on **three** frames — `~/Desktop/fuji/DSCF6509.RAF` and two others of different brightness —
and put the six files where the user can open them. Report the numbers. If `MEDIAN_TARGET` or the
clip targets look wrong on those frames, **say which way** and stop; the user tunes them.

---

## 5. Tests

1. **A uniform grey** of value `g` → exposure exactly `log2(MEDIAN_TARGET / g)`, whites and blacks
   ≈ 0 (nothing to clip).
2. **Outliers land on the targets**: a ramp with 0.1 % of pixels pinned dark and 0.1 % bright →
   after applying the solved values *through the real stage*, `p_hi` ≈ 0.98 and `p_lo` ≈ 0.01.
   This is the load-bearing test: it is the only one that can see a bisection against the wrong
   stage or a target typo.
3. **Monotonicity**: `ToneRegionsStage` output at a fixed input is monotone in `whites` and in
   `blacks` across the slider range — the premise bisection rests on, asserted.
4. **The crop restricts the analysis**: a bright border outside the crop must not move the result;
   with the crop removed it must.
5. **Degenerate inputs**: an all-black image, a 1×1 image, `NaN` samples → finite outputs inside
   the slider ranges, no panic.
6. **Clamps**: a frame 6 EV under lands at −2.5, not −6.
7. **The button writes all three and marks the edit once** — a headless panel test through
   `test_ui`, the `merely_drawing_the_panel_leaves_the_temperature_following_the_file` shape:
   drawing without clicking changes nothing.

**Sabotage** each; predicted blind spot: a uniform-grey test cannot see a wrong percentile, which is
what test 2 is for.

---

## 6. Ownership

**Yours:** new `pipeline/src/auto_tone.rs` (+ `pub mod` in `lib.rs`), the Auto button and its
parameter threading in `develop/panel/adjustments.rs` and `panel/mod.rs`, the probe example.

**Not yours:** `Edits` (**no new field** — this writes existing ones), pipeline stage order,
goldens, `Settings`, the canvas, `local_contrast.rs`/`color_wheel.rs` (D).

⚠ `adjustments.rs` is over 600 lines; say so in your report rather than silently adding to it, and
put the button's logic (not its drawing) somewhere testable — `wb_tool`'s pattern.

---

## 7. Accepted caveats

1. Three sliders only; contrast, highlights and shadows are untouched.
2. Rotation is ignored when restricting to the crop.
3. Whites and blacks are solved independently; their small interaction is accepted.
4. The targets are starting values; the user tunes them from the probe's output.
5. Not applied on open — that is 5.4b's territory, and a different question.

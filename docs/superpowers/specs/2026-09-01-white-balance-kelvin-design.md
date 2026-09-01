# M5.2 — white balance as an instrument

*Design, 2026-09-01.*

`WhiteBalance` stops being a pair of arbitrary ±100 sliders and becomes **Kelvin + tint**, with an
eyedropper and presets. One model serves RAW and non-RAW alike; only the matrix differs.

---

## Why

`WhiteBalanceStage` is `[1 + 0.4·t, 1 − 0.3·g, 1 − 0.4·t]`, luminance-normalised, with coefficients
chosen by hand. There is no reference point, so a cast is corrected by nudging until it stops looking
wrong — and, more seriously, **a white balance copied or synced between two photos is meaningless**,
because the slider describes a *shift* and not a *target*.

⚠⚠ **The strongest evidence for this milestone is that the sliders have never been used.** All **49
copies** in the author's two folders store `{"temp": 0.0, "tint": 0.0}`. Nothing has ever been
converted because nothing has ever been set.

---

## 1. One model, both file types

```rust
pub struct WhiteBalance {
    /// The illuminant declared for the scene, in Kelvin. `None` = the file's own baseline.
    pub kelvin: Option<f32>,
    /// Green–magenta, perpendicular to the locus. 0 = none.
    pub tint: f32,
}
```

⚠⚠ **`Option<f32>`, not a magic zero, and it is load-bearing.** `None` means *the file's baseline* —
the as-shot temperature for a RAW, D65 for a JPEG — so `Edits::default()` stays neutral **without
knowing which kind of file it describes**. That is the rule the entire golden harness rests on.

### ⚠⚠ The reference for a non-RAW file is its own white point

An earlier draft of this design used an **enum** — `Relative` for non-RAW, `Kelvin` for RAW — on the
reasoning that a JPEG has no camera neutral and therefore no Kelvin. **That conflated two things.**
A JPEG has been white-balanced *by the camera*, so a neutral object in it reads as equal RGB: its
baseline **is** sRGB's white point, D65. That is not a convention being invented; it is what the file
is.

So the machinery is identical and only the matrix differs:

| Source | Matrix | Baseline |
| --- | --- | --- |
| RAW | libraw's `cam_xyz` | the as-shot neutral, solved to Kelvin |
| JPEG · PNG · TIFF | sRGB primaries | D65, by construction |

Semantics are the same on both: **the illuminant I am declaring the scene to have had**, with the
file's baseline being where nothing changes. This removes the enum, the `match` arms, and the
undefined paste across a RAW→JPEG boundary.

### ⚠⚠ The caveat this buys, confirmed rather than assumed

**The same Kelvin does not produce the same result on a RAW and its own camera JPEG.** The author's
X-T4 RAW solves to **5165 K**; every JPEG sits at 6500 K. Setting 5165 leaves the RAW unchanged and
cools the JPEG visibly.

⚠ The obvious mitigation — read a baseline from the JPEG — was **checked and is dead**: of **433
JPEGs**, **zero** carry `ColorTemperature`. `Light Source` reads `Unknown`, and the Fuji maker note
offers only `White Balance Fine Tune: Red +0, Blue −40`, a relative nudge. ⚠ Worse, **430 of 430
were shot on Auto**, so the camera chose a different temperature per frame and recorded none of them
— meaning even a recoverable baseline would not be *shared* between two JPEGs of one scene. D65 at
least gives every JPEG the same baseline, which is what makes a Kelvin value transferable.

⚠ Cross-format transfer is **not** a design hole: White Balance is already its own `EditGroups`
group (index 0), so excluding it from a paste or sync is an existing first-class gesture.

---

## 2. The maths

Two directions. Only one exists.

- **Forward — exists.** `color::camera::neutral_temperature` maps a camera neutral to Kelvin. It
  takes two matrices for the DCP dual-illuminant interpolation; ⚠ **passing one matrix twice
  degenerates correctly**, because `blend(m, m, g)` is `m` for any `g`. So it works from libraw's
  `cam_xyz` and needs **no `.dcp`** — which matters, since Rustroom ships no profiles.
- **Inverse — new, and the actual work.** Kelvin + tint → xy on the daylight/Planckian locus → XYZ →
  the source's matrix → per-channel gains. There is no blackbody or locus maths anywhere in
  `crates/color` today.

⚠ The roadmap previously said *"the temperature solver already exists"*. That is **half true and
misleading**: the readout is free, the slider is not.

New module **`color::wb`**: pure functions, no I/O, no egui. Gains are luminance-normalised as today,
so a white-balance change moves colour and not brightness.

---

## 3. Where the conversion happens

**Not in `pipeline`.** The rule is already recorded for C1: *"`pipeline` is source-agnostic and a
JPEG has no camera matrix; the decoder is the only component that knows which camera took the
picture."*

The source's matrix is therefore a **per-render parameter**, exactly as `RenderScale` and `Resampler`
are: an additive entry point with the existing one delegating, defaulting to **sRGB primaries**.

- `WhiteBalanceStage` keeps taking gains and learns nothing new.
- RAW renders pass `cam_xyz`, carried on `Decoded` → `Session` the way the lens profile already is.
- ⚠ Goldens are synthetic buffers with no camera, so they take the sRGB default and **do not move** —
  except `warm_wb`, see below.

---

## 4. The UI

- **Kelvin slider** with a real number, and a **tint** slider.
- **Eyedropper**: click a neutral on the canvas, solve back to Kelvin + tint. ⚠ Shares its
  canvas-picking code with M6.5d's numeric colour readout; build either and the other is nearly free.
- **Presets**: As Shot, Auto, Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash.
  - ⚠ **"As Shot" sets `kelvin: None`**, never the numeric value. `None` keeps *following* the file;
    freezing the number would strand the photo if the profile or decode ever changed — the same rule
    an unlabelled virtual copy already follows.
  - ⚠ **"Auto" is a real analysis pass**, not a preset value, and is the one item here that could be
    cut without harming the rest. Cut it if it fights.

⚠ No baseline readout in the panel (user's call): the divergence in §1 is understood by anyone who
shoots RAW+JPEG, and a permanent label costs panel space to restate it.

---

## 5. Migration

**No migration code.** Serde ignores unknown fields, so an existing `{"temp": 0.0, "tint": 0.0}`
reads as `kelvin: None, tint: 0.0` — exactly right, and every one of the 49 real copies is that.

⚠ A **non-zero** legacy `temp` would be silently dropped. That is the honest cost of a value that
never had a Kelvin meaning: the old and new models trace different surfaces in gain space, so even a
best fit would shift the photo. No such value exists in this catalogue.

⚠⚠ **`golden_warm_wb` must be re-authored, not regenerated.** Its `temp: 50` has no Kelvin
equivalent, so regenerating it would bake whatever the new code happens to do into the reference.
Choose a Kelvin deliberately and the golden means something again.

---

## 6. Testing

1. **⚠⚠ Cross-check the locus against published values, not only against ourselves.** A
   self-consistent pair of wrong transforms passes a round trip happily. The ProPhoto matrices were
   validated exactly this way, and it is what caught a 1e-4 discrepancy that turned out to be
   rounding rather than error.
2. **Round trip**: a neutral solved to Kelvin, fed back through the inverse, gives gains of
   `[1, 1, 1]`. ⚠ Only meaningful *with* (1); alone it is self-consistency.
3. **`None` is exactly neutral** — gains `[1, 1, 1]` on both matrices, bit-exactly, or `Edits`
   stops being neutral by default and every golden moves.
4. **The sRGB default leaves the goldens byte-identical**, which is what proves the per-render
   parameter is inert when unused.
5. **A JPEG and a RAW at the same Kelvin differ**, asserted rather than discovered — it is the
   caveat in §1 and the thing most likely to be "fixed" by someone later.

⚠ Sabotage each, and confirm each **ran**: a filter matching no test prints `test result: ok. 0
passed; 0 failed` and reads as success. This has been hit three times in one day.

---

## 7. Accepted caveats

1. **A RAW and its own JPEG diverge at the same Kelvin.** Inherent; the data to fix it does not exist.
2. **A legacy non-zero `temp` is dropped.** None exists here.
3. **Non-RAW correction is limited** — 8-bit, already clipped, and the camera's rendering is not a
   simple gain. True of any JPEG edit.
4. **McCamy's approximation** is ~2% over the range in use, already recorded on the forward solve.
5. **No baseline readout**, by choice.
6. **"Auto" is optional** and may be cut.

# The profile's HueSatMap

Status: **design agreed 2026-08-25**, not yet implemented.

**Piece C3**, following [C2](2026-08-25-dcp-matrices-design.md). This is the slice where a camera
profile finally changes what you see.

## ⚠⚠ Scoped from the data, not from the DNG spec's table of contents

C3 was originally scoped as "`ProfileHueSatMap` + `ProfileLookTable` + `ProfileToneCurve`".
Inspecting the actual profiles changed that:

| tag | `FUJIFILM X-T4.dcp` | `FUJIFILM X-T1.dcp` |
|---|---|---|
| `HueSatMap` | **present**, dual-illuminant, 90×30×1 | present |
| `LookTable` | **absent** | present |
| `ProfileToneCurve` | present but **`[0,0,1,1]` — the identity** | present |

So for this camera the HueSatMap *is* the whole of C3. And it is not subtle:

- **hue shift −36.3° to +6.8°, mean |shift| 11.3°**
- **saturation scale 0.95 to 1.95, mean 1.20**
- value scale 0.91 to 1.40

⚠ After C1 and C2, where "no visible change" was the success criterion, **this one you will see**.

## ⚠⚠ Why `ProfileToneCurve` is excluded, and it is not effort

The DNG order is matrices → HueSatMap → **ProfileToneCurve** → LookTable. That curve is Adobe's
*display* rendering — what makes a raw file look like a photo in Lightroom by default.

**Rustroom's decode produces scene-referred linear**, and every downstream stage is tuned against
linear luminance: `local_contrast`'s `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS`/`GAIN_MAX`/
`HALO_KNEE`, `dehaze`'s airlight percentile. Baking a display curve into the decode would leave
that buffer non-linear while every constant still assumed it was, which is the kind of error that
is invisible per-stage and wrong everywhere.

It is also the identity in this profile, so excluding it costs nothing here.

## ⚠ Why `LookTable` is deferred, and it is not effort either

The spec applies it **after** the tone curve. With no tone curve, applying it puts it in the wrong
space — worse than not applying it. It is also **absent from the X-T4 profile**, so it could not be
tested on the files this project actually validates against.

The machinery is shared: a LookTable is the same 3-D HSV table as a HueSatMap. When a tone-curve
answer exists, adding it is small.

## The transform

Applied per pixel, after C2's matrix, on linear ProPhoto:

1. RGB → HSV.
2. Index the table by hue and saturation; interpolate.
3. `h += hueShift`, `s *= satScale`, `v *= valScale`.
4. HSV → RGB.

⚠ **`valDivisions` is 1 for this profile, so the interpolation is 2-D**, over hue and saturation
only. The code still handles a 3-D table — other cameras have them — but the tested path is 2-D.

⚠ **Hue wraps, saturation and value clamp.** Hue is circular so its index is modulo the divisions;
saturation and value are not, and their indices clamp at the ends.

⚠⚠ **Scene-referred values exceed 1.0, and HSV's `v` is `max(r,g,b)`.** The table index must clamp
at the top **while the scale still applies to the real value** — otherwise every highlight above
1.0 would be dragged back to the table's edge, quietly compressing exactly the headroom that piece
A's wide working space exists to preserve.

⚠ **A neutral is unaffected by construction**: `s == 0`, so `satScale` multiplies zero and the hue
shift is applied to an undefined hue that converts back to the same grey. Worth a test precisely
because it is structural — if a neutral ever moves, the HSV round trip is wrong, not the table.

**Dual-illuminant**: `HueSatMapData1`/`2` are interpolated entry-wise with the same weight C2
already computes from the camera neutral. One weight, two consumers — the matrices and the table.

## Testing

- **Parse against known values** extracted by the independent Python reader, as C2's matrices were.
- **An identity table changes nothing** — zero shift, unit scales, bit-exact.
- **A known entry produces its exact shift**, on a colour that lands on a grid point so no
  interpolation is involved.
- **Neutrals are unchanged**, exactly.
- **The HSV round trip is the identity** to f32 tolerance, including for `v > 1`.
- **Values above 1.0 keep their headroom** — the specific regression the clamping note describes.
- **Measured on the X-T4 file**: mean and max change against the C2 render, reported as numbers.
  ⚠ Unlike C1 and C2, a **large** difference is the expected result here.

⚠ The independent oracle is still unavailable — `rawtherapee-cli` crashes on this machine
(exit 133), as recorded in C2. So the same honesty applies: `camera_profile` stays **opt-in**, and
the visible result is judged by eye against a known-good rendering rather than diffed against a
reference implementation.

## What this does not do

- **No `ProfileToneCurve`**, for the architectural reason above.
- **No `LookTable`**, for the ordering reason above.
- **No profile discovery or UI** — still C4, with the licensing question.
- **No `CameraCalibration`** — unchanged from C2.

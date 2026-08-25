# Read a DCP's matrices

Status: **design agreed 2026-08-25**, not yet implemented.

**Piece C2**, following [C1](2026-08-25-camera-input-transform-design.md), which moved the
camera→working-space transform into `raw-decode`. C2 fills that seam from a real Adobe camera
profile instead of libraw's derived matrix.

## ⚠ What this will look like: almost nothing

Adobe's `ColorMatrix2` is where libraw's `cam_xyz` comes from in the first place, so a DCP's
matrices and libraw's differ modestly. **The visible half of camera profiles is C3's
`ProfileHueSatMap` and `ProfileLookTable`.** Recorded up front so a correct C2 is not mistaken for
a broken one — the same warning C1 needed, for the same reason.

What C2 does buy, and C1 could not:

- **`ForwardMatrix`**, which maps camera→XYZ(D50) directly with the white balance folded in,
  rather than inverting a ColorMatrix;
- **dual-illuminant interpolation**, so the transform follows the actual white balance instead of
  being fixed at one illuminant.

## Opt-in, so it cannot regress anything

`DecodeOptions.camera_profile: Option<PathBuf>`, defaulting to `None` = C1's libraw matrix.

⚠ This is the design decision that makes C2 safe. Every existing render is untouched, the C1
oracle stays live, and the acceptance test can diff the two paths on demand. Profile *discovery*
and the UI are **C4**, along with the licensing question about redistributing Adobe's files.

## What the profile contains

Read from `FUJIFILM X-T4.dcp`, which ships with RawTherapee:

| tag | value |
|---|---|
| `CalibrationIlluminant1` | 17 — Standard A, 2856 K |
| `CalibrationIlluminant2` | 23 — D50 |
| `ColorMatrix1/2` | 3×3, XYZ → camera |
| `ForwardMatrix1/2` | 3×3, camera → XYZ (D50) |
| `CameraCalibration1/2` | **absent** — so the identity |

⚠⚠ **The ForwardMatrix rows sum to `(0.9643, 1.0, 0.8251)`** — the D50 white point to four
decimals. That is the DNG specification's constraint, not a coincidence, and it is the strongest
self-check available: a parser that transposes the matrix or mis-reads the rationals will fail it.

⚠ **ProPhoto is a D50 space and `ForwardMatrix` outputs XYZ D50, so there is no chromatic
adaptation at the end** — just XYZ(D50)→ProPhoto. One less place to get a white point wrong.

## The transform

1. Take the camera neutral from libraw's `cam_mul` — the white balance actually applied.
2. Find the interpolation weight between the two illuminants from that neutral. The DNG spec does
   this iteratively: guess a temperature, build the matrix, compute the neutral's xy, re-derive
   the temperature, repeat.
3. Interpolate `ForwardMatrix1`/`ForwardMatrix2` (and the ColorMatrices, for the neutral solve).
4. Apply camera→XYZ(D50)→ProPhoto.

⚠ **Step 2 is where this will go wrong first**, and it is not something to reason about: the
iteration, the temperature↔xy conversion and the direction of the blend are all easy to get
plausibly wrong. It is validated by rendering, not by inspection.

## Validation, since "matches libraw" no longer applies

C1's acceptance criterion was that the render did not change. C2 deliberately changes it, so the
oracle has to come from outside:

- **⚠⚠ CORRECTED AFTER IMPLEMENTATION: the RawTherapee oracle was NOT available.**
  `rawtherapee-cli` is installed but **crashes on this machine — exit 133 (SIGTRAP), no output,
  empty log**. So the independent implementation this plan leaned on could not be run, and C2
  ships with weaker validation than intended. What replaced it, and its limits, are below.
- **The ForwardMatrix white-point property** above, as a unit test on the parser.
- **Parsed values checked against the numbers in this document**, which were extracted by an
  independent Python reader — not by the code under test.
- **A neutral camera pixel must come out neutral**, the same property C1 relies on.
- **Both endpoints**: a neutral matching illuminant 1 must select `ForwardMatrix1` essentially
  exactly, and likewise for 2. An interpolation that is wrong in the middle is often still right
  at the ends, so the middle needs the RawTherapee comparison; the ends can be pinned exactly.

### ⚠ What actually validates the interpolation, absent the oracle

**The magnitude argument.** Both ForwardMatrices have rows summing to D50 white, so they agree on a
neutral to 1e-4 — an inverted illuminant choice is **completely invisible on greys**. On coloured
content they differ by **0.020–0.034**. Our render differs from libraw's by a mean of **0.0018**,
an order of magnitude less. Choosing the wrong matrix would show at ~0.03, so the agreement is
evidence the daylight matrix was selected correctly.

`the_illuminant_choice_is_invisible_on_neutrals_and_visible_on_colour` records exactly that, so
nobody concludes from the neutral test alone that the interpolation is verified.

⚠ **This is weaker than an independent implementation and is not presented as equivalent.** The
sign and magnitude of the choice are constrained; the precise blend in the middle of the calibrated
range is not. `camera_profile` stays **off by default**, which is what the plan required in exactly
this situation. Nothing depends on C2 landing.

## Structure

| unit | responsibility |
|---|---|
| `crates/camera-profile/` (new crate) | Parse a `.dcp` into a plain struct. No libraw, no colour maths. |
| `color::camera` (existing) | The interpolation and the camera→ProPhoto composition. |
| `raw-decode` | Load the profile when asked, hand the matrix to the existing C1 seam. |

⚠ A **new crate** rather than a module in `raw-decode`: DCP parsing is TIFF-IFD reading with no
dependency on libraw or on our colour code, and it is the one part testable purely against a file.
Keeping it separate is what lets its tests run in CI where the RAW-gated ones cannot.

## What this does not do

- **No `ProfileHueSatMap`, `ProfileLookTable` or `ProfileToneCurve`** — C3, and the visible half.
- **No profile discovery, no UI, no bundling.** C4, with the licensing question.
- **No embedded DNG profiles.** Only a `.dcp` file the caller names.
- **`CameraCalibration` is not applied.** Absent from the X-T4 profile; a profile that has one
  will be parsed and its calibration **ignored**, which must be recorded rather than silently
  mis-rendered.

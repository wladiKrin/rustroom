# Camera input transform in our own code

Status: **design agreed 2026-08-25**, not yet implemented.

**Piece C1** of colour management, and the first of four slices of piece C. It moves the
camera→working-space transform out of libraw and into `raw-decode`, **without changing a single
pixel**. It exists so that C2 has somewhere to put a DCP's matrix.

## Why this slice has no visible payoff, stated up front

libraw's `cam_xyz` is derived from Adobe's own `ColorMatrix2` for most cameras, so replacing
libraw's application of it with our own should produce the *same image*. That is the point:
**the acceptance criterion is that the render is unchanged.**

⚠ The visible half of camera profiles — "Adobe Standard" versus "Camera Velvia" — lives in the
`ProfileHueSatMap` and `ProfileLookTable` of a DCP, which is slice **C3**. C1 and C2 are the
infrastructure that makes C3 possible, and neither will look like much on its own. Recorded here
so nobody mistakes a correct C1 for a failed one.

## The four slices

| slice | what it is |
|---|---|
| **C1** (this) | Decode camera-native and apply camera→ProPhoto ourselves, reproducing libraw |
| C2 | Parse a DCP; use its `ColorMatrix`/`ForwardMatrix` with dual-illuminant interpolation |
| C3 | `ProfileHueSatMap`, `ProfileLookTable`, `ProfileToneCurve` — the visible "look" |
| C4 | Choosing profiles in the UI, and where they come from |

⚠ **Licensing is unresolved and belongs to C4, not here.** The DCP files on this machine ship with
RawTherapee and are Adobe's. Redistribution needs the DNG SDK licence checked before anything is
bundled; reading a profile the user already has avoids the question. C1 touches none of it.

## Where the transform goes

**In `raw-decode`, immediately after libraw returns** — not as a `PipelineStage`.

⚠ `pipeline` is deliberately source-agnostic: a JPEG has no camera matrix, and a camera-specific
stage there would be a lie for every non-RAW input. The decoder is the only component that knows
which camera took the picture, so the input profile belongs to it. Consequences, all good:

- `pipeline`, `PrefixKey`, the render engine and **all 39 goldens are untouched**;
- the seam C2 needs already exists — swap where the matrix comes from, change nothing else.

libraw is asked for `output_color = 0` (camera-native RGB, still demosaiced and still
white-balanced by `use_camera_wb`), and `raw-decode` applies camera→ProPhoto itself.

## ⚠⚠ The matrix derivation is verified, never reasoned

The matrix is `cam_xyz` (camera→XYZ) composed with XYZ→ProPhoto. Two details decide whether it is
right, and **neither will be settled from memory**:

- **dcraw normalises each row of the composed matrix to sum to 1.** That is what makes camera
  white land on output white. Omit it and *every* image carries a colour cast — subtle enough to
  ship, bad enough to matter. Exactly the failure `white_maps_to_white` exists for elsewhere.
- **The illuminant convention** of dcraw's tables versus ProPhoto's D50 white point.

The design is therefore: replicate dcraw's `cam_xyz_coeff`, then **verify the rendered result
against libraw's own output on a real `.RAF`**. If they disagree, the derivation is wrong and we
learn it in one test run rather than shipping a cast. This is the discipline that caught the
ProPhoto-luminance error, applied before the fact instead of after.

## The libraw path stays, permanently

`DecodeOptions` gains `camera_native: bool` — `true` once this lands, `false` selecting today's
libraw conversion.

⚠ **Not a temporary A/B switch.** C2 and C3 will each change this matrix, and a reference
implementation to diff against is worth far more than the few lines it costs. It is the same role
the golden harness plays for `pipeline`: an oracle that does not move when the thing under test
does. `rustroom decode`'s debug dump keeps using it too.

## Testing

- **Unit, on the matrix maths**: every row sums to 1; a neutral camera pixel maps to a neutral
  output pixel; the matrix is invertible. These run without libraw.
- **All 39 goldens byte-identical** — they never reach `raw-decode`, so any movement means the
  change leaked somewhere it should not have.
- **⚠ The acceptance test is gated on `RAW_FIXTURE`**: decode the same file both ways and assert
  the two images agree within tolerance. It is the only test that can prove the claim this slice
  makes, and it cannot run in CI without a fixture — stated as a limitation, not hidden.
- **Measured, not eyeballed**: max and mean per-channel difference on the X-T4 file, reported as
  numbers.

## What this does not do

- **No visible change.** If you can see one, it is a bug.
- **No DCP.** Still libraw's camera matrix, merely applied by us. That is C2.
- **No profile choice, no look tables, no profile tone curve.** C3 and C4.
- **JPEG/PNG/TIFF are untouched** — they have no camera matrix and go on assuming sRGB.

## ⚠ The risk, named

This moves a well-tested transform out of libraw and into our code, for zero user-visible gain.
X-Trans is the awkward case and `output_color = 0` changes what libraw hands back, so the X-T4
file is both the risk and the test. If the two paths cannot be made to agree, the honest outcome is
to stop and reconsider C2 rather than ship a "close enough" cast.

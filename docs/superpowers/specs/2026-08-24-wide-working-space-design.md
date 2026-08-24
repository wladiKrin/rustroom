# A wide working space — linear ProPhoto

Status: **design agreed 2026-08-24**, not yet implemented.

This is **piece A** of colour management. B (output profile choice), C (camera input profiles) and
D (display profile for the preview) are separate and follow from it — see "What this does not do".

## The starting point, stated correctly

⚠ **Colour management is not absent today, and describing it that way misleads.** `backend_rsraw.rs`
sets `params.output_color = 1`, so libraw applies its own camera matrix and hands the pipeline
**linear light in sRGB (Rec.709) primaries**. There *is* an input transform. What is wrong is the
space it lands in.

sRGB's gamut is small. Saturated reds, deep cyans and most of what a sensor records at the edges of
its response are **clipped before a single slider is touched**. Everything downstream then edits data
that has already lost information.

## The working space: linear ProPhoto (ROMM)

Chosen over linear Rec.2020 and linear ACEScg for one practical reason and one behavioural one:

- **libraw emits it directly** — `output_color = 4`. No matrix of ours in the decode path, so no
  opportunity for us to get the camera transform wrong. Rec.2020 and ACEScg would both require
  deriving our own from libraw's XYZ or ACES output.
- **It is what Lightroom and ACR use internally**, so rendering behaviour matches the reference
  application photographers will compare against.

⚠ ProPhoto's usual objection — banding in 16-bit — **does not apply here**: the pipeline is `f32`
end to end, from decode to output transform.

## ⚠⚠ The finding that shapes everything: ProPhoto luma is useless

Computing the ROMM→XYZ matrix from ProPhoto's own primaries gives a luminance row of:

```
ProPhoto:  R 0.2880   G 0.7119   B 0.0001
sRGB:      R 0.2126   G 0.7152   B 0.0722     (the familiar Rec.709 weights)
```

**Blue contributes essentially nothing.** That is not an arithmetic slip — ProPhoto's blue primary
sits at y = 0.0001 on the chromaticity diagram, almost exactly where luminance is zero.

Six stages take a luma and act on it, and using the working space's own coefficients — the
mathematically correct-sounding move — breaks all of them:

- **Sharpening** would stop sharpening blue detail entirely. A blue sky gradient, an iris, water:
  none of that detail is in the luma at all.
- **Saturation and vibrance** rotate colour around luma, so blues would swing differently from
  everything else in the frame.
- **Dehaze** estimates airlight from luminance, and haze is blue.

### So lightness becomes Oklab L

`color::lightness()` returns Oklab **L**, and every stage that wanted a luma uses it.

Oklab is defined from XYZ, so L is **working-space independent** — the same value whether the pixel
is expressed in sRGB, ProPhoto or Rec.2020 — and it does not collapse blue. The crate already has
Oklab, and four stages already use it, so this turns "eight primary-dependent stages" into one
conversion done once.

Rejected alternatives, with the reason each fails:

- **Rec.709 luma computed after converting to sRGB primaries.** Preserves today's behaviour exactly
  and barely moves the goldens — but it measures brightness in a space we are not editing in, and
  out-of-gamut colours go negative on the way.
- **ProPhoto's own luma.** Internally consistent and produces the blue-detail failure above.

⚠ **The cost, stated plainly: Oklab L is a cube-root perceptual lightness, not linear luminance.**
Sharpening, clarity and dehaze change in *character*, not merely in magnitude. This has to be judged
by eye on a real photograph; tolerance alone cannot tell a better result from a worse one. It also
invalidates five tuned constants — see the section below, which is the part of this change most
likely to ship broken and unnoticed.

## What `color` gains

| addition | notes |
|---|---|
| ProPhoto ↔ XYZ (D50) | Derived from ROMM primaries, checked against published matrices. |
| Bradford D50 → D65 | Oklab is defined at D65; ProPhoto is a D50 space. Skipping the adaptation is a silent ~2% error, not a visible failure. |
| XYZ → Oklab | Ottosson's LMS step, factored out of the existing sRGB path. |
| `prophoto_to_oklab` / `oklab_to_prophoto` | The compositions the stages call. |
| `prophoto_to_srgb` | For the output transform. |
| `lightness()` | Oklab L. **One function**, replacing the Rec.709 triple currently duplicated in six files. |

⚠ **`linear_srgb_to_oklab` stays.** The golden charts and any sRGB-sourced image still need it. It
gains a sibling; it is not replaced.

⚠ The six duplicated copies of `0.2126 / 0.7152 / 0.0722` are a **pre-existing** smell that this
change makes acute: six copies of a constant that is now wrong is six chances to fix it differently.

## The eight stages

| stage | what changes |
|---|---|
| `white_balance` | `LUMA` const → `lightness()` |
| `tone_regions` | Rec.709 luma → `lightness()` |
| `saturation` | `LUMA_R`/luma → `lightness()` |
| `dehaze` | luma → `lightness()`; airlight estimation now perceptual |
| `sharpening` | luma → `lightness()`; Oklab call → ProPhoto variant |
| `local_contrast` | luma → `lightness()`; Oklab call → ProPhoto variant |
| `hsl` | Oklab call → ProPhoto variant |
| `noise_reduction` | Oklab call → ProPhoto variant |

Untouched, because they are primary-independent: `exposure` and `vignette` (scalar multiplies),
`geometry` (resampling), `mask` (blending), `tone_curve` (a per-channel LUT).

## ⚠⚠ Five tuned constants assume *linear* luminance and must be re-derived

This is the part most easily missed, because nothing about it fails to compile. Several stages
carry constants chosen against linear luma, and Oklab L is a cube root — so the same number means
something different.

| constant | where | why it breaks |
|---|---|---|
| `0.18` mid-grey | `local_contrast`'s midtone weight `t = l / (l + 0.18)` | **The sharpest case.** 0.18 is mid-grey in *linear* light; in Oklab L mid-grey is ≈ **0.565**. Left alone, the weight peaks at roughly a quarter-stop above black — Clarity's midtone protection would apply to shadows instead of midtones, which is close to the opposite of its purpose. |
| `LUMA_EPS = 1e-3` | `sharpening` | A near-black guard against dividing by ~0. Cube root lifts small values hard: linear 1e-3 is L ≈ 0.1, so the guard would fire across a huge swath of the shadows rather than at the very bottom. |
| `GAIN_MAX = 4.0` | `sharpening` | Bounds the halo in near-black regions, expressed as a ratio of linear luma. The ratio means something different against L. |
| `HALO_KNEE = 0.05` | `sharpening` | A local-contrast magnitude in luma units. |
| `A_MIN` / airlight percentile | `dehaze` | The dark-channel prior and its airlight are gathered on a brightness statistic; the distribution's shape changes under a cube root. |

⚠ **These are re-derived, not carried over.** Each is a number to be recomputed against L and then
checked on a real photograph — the same way the vignette's amount model took three revisions before
it felt right, and for the same reason: a constant like this cannot be settled from the spec.

⚠ **`local_contrast`'s `0.18` is the one to get right first.** It is the only one whose wrong value
produces a plausible-looking image rather than an obviously broken one, so it is the one that could
ship unnoticed.

## ⚠⚠ The goldens, and how the charts are built

All 38 move. That **removes the check this project normally leans on** — *"the other goldens stayed
byte-identical, which proves the change was scoped"*. Nothing survives to prove it, so a different
guarantee has to replace it.

**The charts are re-authored in sRGB and converted to working space as they are built.** Today they
write RGB values straight into an `ImageBuf`; after the switch those same numbers would silently mean
*ProPhoto* red — a far more saturated colour than the chart intends. Converting at construction:

- preserves the **intent** ("this swatch is sRGB pure red"), so a reviewer can still recognise the
  chart;
- keeps the neutral golden's output nearly unchanged;
- and gives back a real check — **the `neutral` golden should land within `TOL` of its current
  bytes**, because sRGB → ProPhoto → sRGB is a round trip. If it does not, a matrix is wrong.

That single property is the strongest evidence available that the colour maths is right, and it is
why the chart change is part of this spec rather than an incidental tidy-up.

## Testing

- **Round trips**: `prophoto_to_srgb(srgb_to_prophoto(c)) ≈ c` and the Oklab pair, to ~1e-5.
- **Known values**: ProPhoto→XYZ checked against the published ROMM matrix; D50→D65 against a
  published Bradford matrix. ⚠ Against *published references*, not against a second derivation of our
  own — re-deriving in the test proves only that the same mistake was made twice.
- **`lightness()` does not collapse blue**: pure blue must have clearly non-zero L. This is the
  regression guard for the whole finding above, and it fails loudly if anyone "simplifies"
  `lightness` back to working-space luma.
- **The midtone weight peaks at mid-grey**: `local_contrast`'s `t` must reach its maximum at a
  mid-grey pixel, not in the shadows. That is the test which catches the `0.18` → `0.565`
  re-derivation being forgotten, and it is the one constant whose wrong value still looks
  plausible.
- **The `neutral` golden stays within `TOL`** — the round-trip property, as above.
- **Judged on a real X-T4 `.RAF`**: a saturated subject rendered before and after, checked for the
  gamut win actually appearing, and sharpening/clarity/dehaze checked for character change.

## Two plans

1. **`color` only.** The matrices, the Oklab compositions and `lightness()`, with their tests.
   **No behaviour change and no golden moves** — it ships provable value on its own and de-risks the
   arithmetic before anything depends on it.
2. **The switch.** Decode flag, output transform, the eight stages, the chart authoring, regenerated
   goldens, and the judgement on a real file.

## What this does not do

- **No output profile choice.** Export stays sRGB. The output transform becomes the seam where
  Display P3 and Adobe RGB drop in — that is piece **B**, and it is only meaningful *after* this,
  because a P3 file exported from an sRGB working space contains no P3 colour.
- **No camera input profiles.** libraw's matrix still does camera→working; DCP and embedded profiles
  are piece **C**.
- **No display profile.** The preview is still sent to the screen as sRGB, so a wide-gamut monitor
  will still oversaturate it. That is piece **D**, and this change does not make it worse.
- **`rustroom decode`'s display dump keeps asking libraw for sRGB.** It is a debug tool, not an
  export path.

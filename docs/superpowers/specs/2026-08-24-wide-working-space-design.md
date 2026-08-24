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

## ⚠⚠ CORRECTED 2026-08-24: the "ProPhoto luma collapses blue" finding was wrong

**An earlier version of this spec claimed ProPhoto's luminance row makes blue vanish, and specified
Oklab L to work around it. That was a mistake, and it is recorded here rather than quietly deleted
because the reasoning error is instructive.**

The observation was real: ProPhoto's ROMM→XYZ luminance row is `R 0.2880, G 0.7119, B 0.0001`
against sRGB's `0.2126 / 0.7152 / 0.0722`. The inference from it was not.

**Luminance is a physical quantity.** Taking a colour into ProPhoto and measuring Y with ProPhoto's
row returns what it always was:

| colour (sRGB) | Y via Rec.709 | Y via ProPhoto row |
|---|---|---|
| blue `[0,0,1]` | 0.0722 | 0.0606 |
| red `[1,0,0]` | 0.2126 | 0.2225 |
| green `[0,1,0]` | 0.7152 | 0.7169 |
| sky `[.35,.55,.9]` | 0.5327 | 0.5267 |

The residual differences are chromatic adaptation, not a change of meaning.

**The `0.0001` applies to ProPhoto's own blue *primary*** — a far more saturated blue than sRGB's,
which genuinely does carry almost no luminance. **Generalising from the primary to all blues was the
error**, and it was never checked against a real colour before being written down.

### What follows: luma stays linear

`color::luminance()` returns ProPhoto's Y. Every stage that wanted a luma uses it, and:

- **The five constants tuned against linear luma carry over unchanged** — `local_contrast`'s
  `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS` and `GAIN_MAX` and `HALO_KNEE`, `dehaze`'s airlight
  percentile. No re-derivation, and no re-tuning against a photograph.
- **Sharpening, clarity and dehaze keep their character.** A cube-root perceptual lightness would
  have changed how they feel, not merely their numbers.
- The six duplicated copies of the Rec.709 triple still collapse into **one** definition, which was
  the other half of the original motivation and remains worth doing.

⚠ `luminance_matches_rec709` in `color` is the guard: it checks the value against the Rec.709 result
for real colours, which is exactly the check whose absence let the wrong finding stand.

⚠ **Oklab is still needed**, and `prophoto_to_oklab` / `oklab_to_prophoto` from plan 1 are correct
and unaffected — `hsl`, `noise_reduction`, `sharpening` and `local_contrast` use Oklab for chroma
work, which is a separate matter from luma.

## What `color` gains

| addition | notes |
|---|---|
| ProPhoto ↔ XYZ (D50) | Derived from ROMM primaries, checked against published matrices. |
| Bradford D50 → D65 | Oklab is defined at D65; ProPhoto is a D50 space. Skipping the adaptation is a silent ~2% error, not a visible failure. |
| XYZ → Oklab | Ottosson's LMS step, factored out of the existing sRGB path. |
| `prophoto_to_oklab` / `oklab_to_prophoto` | The compositions the stages call. |
| `prophoto_to_srgb` | For the output transform. |
| `luminance()` + `PROPHOTO_LUMA` | ProPhoto's Y row. **One definition**, replacing the Rec.709 triple currently duplicated in six files. |

⚠ **`linear_srgb_to_oklab` stays.** The golden charts and any sRGB-sourced image still need it. It
gains a sibling; it is not replaced.

⚠ The six duplicated copies of `0.2126 / 0.7152 / 0.0722` are a **pre-existing** smell that this
change makes acute: six copies of a constant that is now wrong is six chances to fix it differently.

## The eight stages

| stage | what changes |
|---|---|
| `white_balance` | local `LUMA` const → `color::luminance()` |
| `tone_regions` | local `LUMA` const → `color::luminance()` |
| `saturation` | `LUMA_R`/luma → `color::luminance()` |
| `dehaze` | local `luma` fn → `color::luminance()`; airlight statistic unchanged in kind |
| `sharpening` | local `luma` fn → `color::luminance()`; Oklab call → ProPhoto variant |
| `local_contrast` | local `luma` fn → `color::luminance()`; Oklab call → ProPhoto variant |
| `hsl` | Oklab call → ProPhoto variant |
| `noise_reduction` | Oklab call → ProPhoto variant |

Untouched, because they are primary-independent: `exposure` and `vignette` (scalar multiplies),
`geometry` (resampling), `mask` (blending), `tone_curve` (a per-channel LUT).

## The tuned constants are unaffected

⚠ **Recorded because an earlier draft said the opposite.** `local_contrast`'s `MID_GRAY = 0.18`,
`sharpening`'s `LUMA_EPS`, `GAIN_MAX` and `HALO_KNEE`, and `dehaze`'s airlight percentile are all
expressed against **linear luminance** — and `luminance()` is linear luminance. They carry over
with no change and no re-tuning.

That is a direct consequence of the correction above: it was only the cube-root perceptual lightness
that would have invalidated them.

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
- **`luminance()` agrees with Rec.709 for real colours** — the guard whose absence let the wrong
  finding stand, and the one that keeps anyone from "fixing" the weights back to sRGB's.
- **The `neutral` golden stays within `TOL`** — the round-trip property, as above.
- **Judged on a real X-T4 `.RAF`**: a saturated subject rendered before and after, checked for the
  gamut win actually appearing, and sharpening/clarity/dehaze checked for character change.

## Two plans

1. **`color` only.** The matrices, the Oklab compositions and `luminance()`, with their tests. **Landed 2026-08-24.**
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

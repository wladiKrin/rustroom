# Output profile choice at export

Status: **design agreed 2026-08-24**, not yet implemented.

This is **piece B** of colour management, and it only became meaningful when
[piece A](2026-08-24-wide-working-space-design.md) landed: a Display P3 file exported from an sRGB
working space contains no P3 colour. Now that the pipeline works in linear ProPhoto, the gamut is
there and the output transform is the one place that throws it away.

## What exists today

`OutputTransformSrgb` converts linear ProPhoto to sRGB primaries, then applies the sRGB transfer
curve. `export::srgb_icc()` builds one static sRGB profile behind a `OnceLock` and attaches it to
all five formats. Both are hardcoded.

The whole feature is: make that destination a choice.

## The spaces

| space | primaries | white | transfer curve |
|---|---|---|---|
| **sRGB** (default) | Rec.709 | D65 | sRGB piecewise (γ 2.4 + linear toe) |
| **Display P3** | DCI-P3 | D65 | **sRGB piecewise** — same curve, wider primaries |
| **Adobe RGB (1998)** | Adobe | D65 | pure γ **563/256 = 2.19921875** |
| **ProPhoto** | ROMM | **D50** | γ 1.8 + linear toe below 0.001953125 |

Rec.2020 is left out as YAGNI until something asks for it.

⚠ **Display P3 takes the sRGB curve, not gamma 2.2.** Only its primaries differ from sRGB. This is
a common error and an invisible one — the image looks *nearly* right, off by a few percent in the
shadows, which reads as a subtle contrast difference rather than as a bug.

⚠ **Adobe RGB's gamma is 563/256, not 2.2.** The two differ by less than 0.1%, but writing 2.2 into
an ICC profile means the file disagrees with the specification it claims to follow.

⚠ **ProPhoto is a D50 space.** Its profile carries a D50 white point where the other three carry
D65; that is correct and must not be "unified".

## ⚠⚠ ProPhoto → ProPhoto is the identity

Exporting ProPhoto applies **no matrix at all** — the working space goes straight out through the
transfer curve. It is the only lossless option, and the only one where nothing is clipped.

That is worth stating as a property rather than leaving it to fall out of a general matrix path,
because it is also the thing a test can pin exactly (`assert_eq!`, not a tolerance).

## `OutputSpace` lives in `color`

```rust
pub enum OutputSpace { Srgb, DisplayP3, AdobeRgb, ProPhoto }

impl OutputSpace {
    pub const ALL: [OutputSpace; 4];
    /// Linear ProPhoto working space → this space's linear primaries. Identity for ProPhoto.
    pub fn from_working(self, c: [f32; 3]) -> [f32; 3];
    /// This space's transfer curve, clamping to [0, 1].
    pub fn encode(self, v: f32) -> f32;
    pub fn label(self) -> &'static str;
    /// Whether 8-bit output in this space is worth warning about. See below.
    pub fn warns_in_8_bit(self) -> bool;
}
```

It goes in `color` because the pipeline stage needs it and `pipeline` already depends on `color`.
`export` gains `color` as a dependency to pick up the same enum for its ICC profiles — a cheap edge,
since `color` is pure arithmetic with no dependencies of its own.

The three matrices (ProPhoto → sRGB / P3 / Adobe RGB) are **fused offline** from primaries, white
points and a Bradford adaptation and shipped as `const`, matching how `spaces.rs` already handles
`PROPHOTO_TO_SRGB`. One multiply per pixel, not three.

## The output transform gains a parameter

`OutputTransformSrgb` becomes `OutputTransform { space }`.

`build_pipeline(edits, scale)` **keeps its signature** and delegates with `Srgb`; a new
`build_pipeline_to(edits, scale, space)` serves export. This is the `Region` precedent exactly —
additive, existing functions delegate with the old behaviour, so the CLI's other paths, the render
engine, `build_pipeline_split*` and all 38 goldens compile untouched.

⚠ **The preview stays sRGB, always.** The display is assumed sRGB — a display profile is piece **D**
— so wiring the canvas to this choice would make it lie about what you are editing. Only export and
`rustroom render` read the space.

## ICC profiles are built, not merely tagged

`export::icc_for(space) -> &'static [u8]` replaces `srgb_icc()`, caching each profile in its own
`OnceLock`. Verified available in lcms2 6.1.1:

- `Profile::new_srgb()` — kept for sRGB, since lcms2's own is canonical.
- `Profile::new_rgb(white: &CIExyY, primaries: &CIExyYTRIPLE, curves: &[&ToneCurve])` — the other three.
- `ToneCurve::new(gamma)` for Adobe RGB; `ToneCurve::new_parametric(4, ..)` for the two piecewise
  curves (ICC parametric type 4 is `Y = (aX+b)^g` above `d`, `cX` below — the shape both need).

⚠ **Parametric curve direction is the trap here.** An ICC tone curve describes device→linear, which
is the *inverse* of what our `encode` applies. Getting it backwards produces a file that is
self-consistently wrong: it opens, it has the right primaries, and it looks washed out or crushed
in a way easily mistaken for a rendering preference. The round-trip test below is what catches it.

## The choice is an export option

`ExportOptions.space`, `#[serde(default)]` so existing settings files and export presets load
unchanged. That gets it persisted as last-used *and* carried by export presets for free — the same
treatment `format` and `demosaic` already have.

The export dialog gets a dropdown. The CLI gets `rustroom render --space`, taking the same token
vocabulary approach as `--format` and `--demosaic`, with the token list built from `ALL` so a fifth
space cannot be missing from an error message.

## ⚠ The 8-bit banding warning — and why it fires for ProPhoto only

Wide gamut plus 8 bits bands: the same 256 levels per channel spread across a larger volume of
colour, so smooth gradients posterise.

**Decided: warn, never block.** A hint appears in the export dialog when the combination arises, and
the export proceeds. The argument is not merely permissiveness — *this session already shipped a
protective disable and it was rejected*: greying out the last ticked format read as "the ones I
didn't click went dark". A blocked combination has the same shape, and the state it prevents here
is one click from being fixed.

⚠ **Narrowed from what was first proposed, and the narrowing matters.** The obvious rule is "any
wide space + any 8-bit format", but that fires on cases which are ordinary practice:

- **8-bit Display P3 is what Apple ships by default** for photos on every iPhone.
- **8-bit Adobe RGB is the standard print-delivery format** and has been for twenty years.
- **8-bit ProPhoto is the one everybody warns against**, Adobe included.

A warning that fires on the first two is a false alarm, and a false alarm trains people to dismiss
the warning that matters. So `warns_in_8_bit()` is true for `ProPhoto` alone, and the method carries
that reasoning in its doc comment so the next person does not "complete" the table.

The condition is a pure function of `(space, format)`, so it is tested without a dialog.

## Testing

- **Round trip per space** — `encode` then decode returns the input; and the matrix pair round-trips
  for in-gamut colour.
- **White maps to white** in every space, which is what catches a missing or reversed Bradford
  adaptation. ⚠ ProPhoto's D50 white point makes this a real check rather than a formality.
- **ProPhoto is bit-exactly the identity** — `assert_eq!`, no tolerance, guarding against someone
  routing it through a general matrix path that is 1-ulp off.
- **⚠⚠ The ICC agrees with our own maths.** Build each profile, transform a known colour through
  **lcms2** from that profile to lcms2's built-in sRGB, and compare against the result of our own
  matrix and curve. Two independent implementations of the same transform must agree — this is the
  test that catches an inverted parametric curve, wrong primaries, or a wrong white point, none of
  which any other test here can see.
- **The capability test that justifies the feature**: a colour outside sRGB survives a P3 export
  while the sRGB export clips it. If this fails the feature does nothing, however well it is wired.
- **One new golden, `output_p3`**, rendering the existing chart to Display P3 — a reference for the
  non-default path, which otherwise has none.
- **All 38 existing goldens stay byte-identical**, since `Srgb` is the default. That is the proof
  this change is additive.

## What this does not do

- **No gamut mapping.** Out-of-gamut colour clips per channel rather than being perceptually
  compressed. Rendering intents are a separate feature and a much larger one.
- **No 16-bit JPEG** — the format has none — so ProPhoto is only really usable via TIFF/PNG 16.
- **No display profile.** The preview cannot show P3; that is piece **D**, and this does not make it
  worse.
- **`rustroom decode`'s debug dump keeps asking libraw for sRGB.** It is a diagnostic, not an export.

# Lens corrections (M3.2) — design

**Date:** 2026-08-25
**Milestone:** Phase 3, M3.2 — the automatic, profile-driven half.
**Scope:** distortion, transverse chromatic aberration (TCA) and vignetting, corrected from a lens
profile. **Perspective correction is deliberately excluded** — see *Scope* below.

## The dependency question, settled by probing

M3.2 has carried one recorded risk since Phase 3 was decomposed: *"a new C dependency, with the
build risk libraw carried"*. **That risk is gone.**

`lensfun = "0.7"` is a **pure-Rust port** of LensFun — zero dependencies, LGPL-3.0-or-later
(compatible with this project's GPL-3.0-or-later), and it **embeds the whole database** (56 XML
files, gzipped at build time). No C library, no glib, no `pkg-config` entry, no CI change, no
Windows/Linux build fight.

Two alternatives were considered and rejected:

- **The C library via bindings.** It is the reference implementation and its database updates
  independently via `lensfun-update-data`. ⚠ But it drags in **glib** — measured during probing,
  a heavier transitive dependency than libraw carried — and there is no maintained Rust binding,
  so we would write one. This is exactly the risk the pure-Rust crate removes.
- **Parsing the database and implementing the models ourselves.** The three models in play
  (`ptlens` distortion, `poly3` TCA, `pa` vignetting) are published polynomials, perhaps 150 lines,
  and there is a strong precedent: `camera-profile` parses `.dcp` itself rather than linking
  Adobe's SDK. Rejected as the *first* move because every correctness bug would be ours — but it
  remains a **contained fallback** if the port proves wrong, since we would already understand the
  database format.

⚠ **Accepted cost of the port:** it is a third-party crate at 0.7.0 by a single author, so
correctness is ours to verify. Its repository carries a `tests/cpp-vs-rust/` suite (excluded from
the published package), which suggests validation against the original, but that is evidence rather
than proof. The mitigation is the synthetic-profile testing below, which pins behaviour against
coefficients we choose ourselves rather than against the crate's own judgement.

## What probing established

Run against the user's own files and gear before any design was written:

- **libraw reports the lens name exactly, and it matches the database verbatim.** `XF23mmF2 R WR`
  and `XF16-80mmF4 R OIS WR` both resolve. The X-T4 is present with the right mount and crop factor
  (1.534). Focal length and aperture come through per shot, which is what the interpolation needs.
- **Both lenses carry all three corrections.** The zoom has distortion and TCA calibrated at six
  focal lengths and vignetting per focal × aperture, so interpolation across the zoom range is real.
- **The corrections compute sensibly on the real frame** (6246×4170):

  | | distortion at the corner | vignette gain at the corner |
  |---|---|---|
  | XF23mm f/2 | source (6277, 4191) — pincushion | **4.79×** (≈2.3 EV) |
  | XF16-80 @ 48.5mm f/4 | source (6387, 4264) — pincushion | 1.16× |
  | XF16-80 @ 16mm f/4 | source (6025, 4022) — **barrel** | 1.44× |

  The signs match the calibration data (barrel at the wide end, pincushion beyond ~35 mm).

- **⚠ There is no double-correction risk, and this was verified rather than assumed.** libraw's
  generated bindings contain **no** distortion, geometry or vignetting fields — zero matches across
  6,389 lines. `dcraw_process` performs demosaic, white balance, colour conversion and gamma, and no
  geometric correction. Fuji's embedded correction parameters are therefore not being applied by
  the decoder, so lensfun's correction is the only one in the chain.

## Scope

**In:** distortion, TCA, vignetting — the three that are *automatic*. They share one code path
(`lensfun::Modifier`), one set of metadata (lens + focal + aperture) and one pipeline position, and
they are complete once a profile matches and a box is ticked.

**Out — perspective correction, as its own later slice.** It is not profile-driven: it needs manual
keystone/rotation sliders or automatic vertical detection, a different maths path (`mod_pc`), and
canvas interaction. This project's record is unambiguous that interactive canvas work is the
expensive and defect-prone half, and shipping the automatic three first proves the geometry seam
before an interactive tool is built on it.

⚠ **This leaves the roadmap's M3.2 partially done, and that is named up front rather than
discovered at the end** — the situation M3.3's LUT half had to be corrected into.

## Data model

`Edits.lens_correction`, in the **Detail** edit group. Detail is already defined in this codebase as
the capture-specific group — *"noise, lens, output size"* — so no new group and no new panel section.

```rust
pub struct LensCorrection {
    pub distortion: bool,
    pub tca: bool,
    pub vignetting: bool,
    /// How far to scale the corrected image, 0..100. 0 leaves the empty edges that distortion
    /// correction produces; 100 scales until they are gone.
    pub scale: f32,
}
```

⚠ **`Default` is hand-written, not derived** — `scale` is 100 at neutral. A derived one would give
every partial sidecar `scale: 0`, silently. The third instance of this trap after `RadialGradient`
and `ColorGrading`.

`is_active()` is **the three booleans only**. `scale` is a modifier and must not insert a stage on
its own — the rule `NoiseReduction`, `Vignette`, `Sharpening` and `ColorGrading` all follow.

**⚠ `Edits::default()` stays strictly neutral** (all three false). Defaulting them true would dent a
load-bearing invariant *in a way the golden harness structurally cannot see* — the goldens are
synthetic buffers with no lens metadata, so the stage would stay inert there while every real photo
changed. Instead the GUI applies a preference; see *Enabling it* below.

## Metadata

`raw-decode` surfaces the lens on `Decoded`: make, model, focal length, aperture. Read from
`rsraw`'s `LensInfo` plus `raw.focal_len()` / `raw.aperture()`.

⚠ **Two fields of `rsraw::LensInfo` are unreliable and must not be used.** Probing showed
`min_focal`/`max_focal` reporting `16-16` for the 16-80 zoom, and `mounts` returning the garbage
string `"IL UM"`. The per-shot `raw.focal_len()` is correct and is what the interpolation needs;
the mount is not needed at all, because matching goes through the camera.

**RAW only.** JPEG/PNG/TIFF carry no lens data through `image-decode` — the `image` crate exposes
orientation and little else — so `lens` is `None` there and the controls are disabled.

## The stage

`LensCorrectionStage`, **first in the prefix, before `GeometryStage`**:

```
lens_correction → geometry → noise_reduction → dehaze   (prefix)
```

The calibration is defined in the uncropped, unrotated sensor frame, so it must precede the crop and
the straighten. Consequences, all favourable:

- **Prefix membership means it runs whole-buffer**, so there is no `Region` plumbing, no kernel
  margin and no `REGION_MARGIN` change, despite being a spatial resample.
- **It needs no `RenderScale`.** The correction is normalised, so passing the *rendered* dimensions
  to `Modifier::new` yields the same correction at preview and export — the property the creative
  vignette already has.
- A new `PrefixStepKey::LensCorrection(LensCorrection)`. ⚠ The lens *metadata* need not enter the
  key: the render engine is rebuilt per photo, so per-photo constants cannot vary within one cache.

**Resampling.** `apply_geometry_distortion` returns an output→source coordinate map; sample the
source bilinearly, as `GeometryStage` already does for rotation.

⚠⚠ **THERE IS NO COMBINED SUBPIXEL+GEOMETRY CALL IN THIS PORT.** The C library has
`ApplySubpixelGeometryDistortion`; here `apply_subpixel_distortion` applies **TCA only** — it seeds
the coordinates from the pixel grid, applies the TCA kernel and un-normalises, with no distortion
term. So distortion and TCA cannot be obtained as one map from the public API.

**Resolution: sum the displacements, one resample.** Evaluate both maps on the same output grid and
take `source_c = distortion(x, y) + (tca_c(x, y) - (x, y))`. This is exact to first order and the
neglected cross term is bounded: TCA is ~0.06% of radius at worst in these profiles and distortion
~3.5%, so at a 3700 px radius the error is ~0.08 px — an order of magnitude below the resampling
kernel. The alternative, two sequential resampling passes, would double the cost *and* soften the
image through double interpolation, which is a far larger error than the one being avoided.

⚠ Guard it with tests that pin the degenerate cases exactly: with TCA off the result must equal
distortion-only, and with distortion off it must equal TCA-only.

With TCA enabled,
`apply_subpixel_distortion` gives a per-channel map, so three samples per pixel. Samples outside the
source frame produce black.

**Vignetting** is `apply_color_modification_f32`, a per-pixel gain from radius — applied before the
coordinate remap, following lensfun's own order.

## The edge policy

Correcting barrel distortion stretches the corners outward, so the corrected frame needs source
pixels that were never captured — measured at ~3.5% outside the sensor on the XF16-80 at 16 mm.

**A scale slider, defaulting to autoscale (100).** At 0 the empty wedges are left for the user to
crop; at 100 the image is scaled until they are gone.

⚠ **The behaviour is not symmetric across the zoom range, which is why the slider is worth its
place.** The same lens shows *barrel* at 16 mm (corners stretch out, edges go empty) and
*pincushion* from ~35 mm (the corrected image overflows the frame and loses edge data regardless).
So a fixed autoscale silently crops at the long end, and a fixed no-scale leaves black wedges at the
wide end; neither single answer is right across one lens.

⚠⚠ **WE MUST COMPUTE THE AUTOSCALE OURSELVES — the crate has no such API.** Verified: no
`get_auto_scale`, no `enable_scaling`, no `LF_MODIFY_SCALE` equivalent anywhere in the crate. The C
library has `lfModifier::GetAutoScale`; this port does not. An earlier draft of this design asserted
otherwise and was wrong.

The consequence is worth stating precisely, because it moves the cost between the options rather
than within them: **a fixed autoscale (the rejected option B) needs exactly the same computation**,
so this does not make the slider more expensive than a fixed default — it makes *leaving the black
edges alone* the only genuinely cheaper answer, and that was rejected on behaviour.

The computation, following what `GetAutoScale` does: evaluate the output→source map at the four
corners and the four edge midpoints, and for each find the scale that would pull its required source
coordinate back inside the frame; take the maximum. Roughly 40 lines, and **exactly testable against
a synthetic profile** — a known barrel coefficient implies a computable autoscale.

## Enabling it

**Neutral in `Edits`; applied by preference on open.** `Settings.auto_lens_correction` (default
**on**) turns the three corrections on for a photo that has **no sidecar yet**. A photo already
edited keeps exactly what it had.

This is the only option of the three considered that keeps `Edits::default() == neutral` genuinely
intact rather than intact-looking. ⚠ **Accepted cost: the GUI and the CLI then disagree on a fresh
photo** — the CLI renders neutral, the GUI renders corrected. That must be documented rather than
discovered.

**No match:** the controls are disabled and the panel names the lens it could not find, so a miss is
diagnosable rather than mysteriously inert.

**⚠ Lens vignetting correction is a different feature from the creative `Vignette`**, which stays
where it is at the end of the tail. One is corrective and pre-crop, the other creative and
post-crop. The UI must not let them be confused.

## ⚠ The brush-mask interaction

Brush strokes are anchored in **pre-geometry coordinates**, which today means *the input to
`GeometryStage`*. Inserting lens correction ahead of it redefines that basis, so **toggling lens
correction after painting shifts every stroke** — up to ~3.5% at the corners, ~0 at the centre.

The principled fix is to include distortion in `FrameMap`, so strokes anchor in raw sensor
coordinates and travel *with* the correction — which is what "a stroke is content-referred, it must
stay on the cat" actually demands, since the correction moves the cat. ⚠ It needs the **forward**
distortion map, and the crate exposes only the inverse (output→source), so it would mean an
iterative solve per stroke point.

**Decision: accept the shift, document it, and put the principled fix in `docs/BACKLOG.md`.**
Enabling lens corrections is something done once and early, not toggled mid-edit.

## Testing

**⚠ The key enabler: `Database::load_str` takes XML, so tests can define a synthetic lens with
coefficients we choose** and assert exact behaviour — no RAW fixture, no gating, deterministic in
CI. That makes real **goldens** possible for lens correction, which would otherwise have been
impossible: a real profile cannot be exercised by a synthetic chart, and a gated test is no evidence
in CI.

- **A zero-coefficient lens must be bit-exactly the identity.** The strongest available test, and
  the analogue of `an_identity_table_changes_nothing`.
- **A known barrel coefficient must move a known point by a computed amount**, checked against the
  ptlens polynomial evaluated in the test rather than through the crate — the same rule the golden
  harness follows in keeping its own quantiser.
- **Vignetting: a synthetic `pa` profile must lift a corner by a computed factor and leave the
  centre at exactly 1.0.**
- **The autoscale must equal the value computed by hand from the synthetic coefficients**, and a
  zero-distortion lens must autoscale to exactly 1.0.
- **Goldens** on a chart with strong straight edges, through a synthetic profile, one per
  correction plus one for the scale slider.
- **The three corrections must be independent** — enabling TCA must not change geometry, and so on.
- **Real-`.RAF` validation is confirmation, not the only evidence.** ⚠ Specifically: look at the
  XF23's **4.79× corner lift** on a real frame before trusting it. It is 2.3 stops, plausible for a
  fast prime wide open but on the high side, and it is the number most likely to be wrong.

⚠ Every new test gets the sabotage step, with a compile check first.

## Plans

Two, following M3.1a and M3.3:

1. **Headless** — the dependency, lens metadata through `raw-decode`, the `Edits` type and group,
   the stage, `build_pipeline` wiring, unit tests, goldens, CLI validation on a real `.RAF`.
2. **The panel** — the Detail block, matched-lens display, the three checkboxes, the scale slider,
   and `Settings.auto_lens_correction` with its apply-on-open rule.

## Accepted caveats

1. **Brush strokes shift if corrections are toggled after painting** — backlogged, reasoning above.
2. **RAW only.** JPEG/PNG/TIFF have no lens metadata through `image-decode`.
3. **The bundled database goes stale with the crate version** rather than tracking the user's
   system, which the C library's `lensfun-update-data` would have handled.
4. **A lens absent from the database gets nothing** — no manual profile picking, no generic model.
5. **Correction quality is lensfun's calibration data**, contributed by volunteers and in these
   entries measured on *other* bodies (the XF23 vignetting was taken with an X-T2, the distortion
   with an X-T10). Its own comment on that lens reads *"the target was not really trustworthy."*
6. **The GUI and CLI disagree on a fresh photo**, by design — see *Enabling it*.
7. **Perspective correction is not included** — a later slice.
8. **The autoscale is our own implementation, not the crate's**, because the crate has none. It
   follows `GetAutoScale`'s approach but is not validated against it; the synthetic-profile tests are
   the evidence.
9. **Distortion and TCA are composed by summing displacements**, not by lensfun's exact
   composition, because the port exposes no combined call. Error bounded at ~0.08 px; see *The
   stage*.

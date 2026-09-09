//! Where a mask's coverage comes from, and how a composite's components are folded into
//! one number per pixel.
//!
//! ⚠ Split out of `stages/mask.rs`, which was 1080 lines before this milestone added to it.
//! The move is deliberately **pure** — every doc comment came across verbatim and no test was
//! edited — so the unchanged tests in `mask.rs` are the evidence it changed no behaviour.

use edit_model::Geometry;

/// Where a mask's coverage comes from.
///
/// ⚠ Resolved **once at construction**, so `apply` matches on it outside the pixel loop. This is
/// the shape a brush forces: `Mask::analytic_coverage` is a pure function of position and a brush
/// is sampled data, so the two cannot share one code path without one of them lying.
pub(super) enum CoverageSource {
    /// Linear and radial — a pure function of position.
    Analytic(edit_model::Mask),
    /// Brush.
    ///
    /// ⚠ Kept **unrasterized** when no prebuilt map is supplied: the frame's dimensions are not
    /// known until `apply`, because with no region the frame *is* the image and the pipeline is
    /// built before the image flows through it. That path is correct and slow, and it is the one
    /// the CLI and export walk — they render once and have nothing to reuse.
    Painted {
        brush: edit_model::Brush,
        geometry: Geometry,
        /// A map the caller already rasterized, from `render_engine`'s cache.
        ///
        /// ⚠ It **must** have been built at the same `fw`/`fh`. The cache key includes them, so
        /// this holds by construction — a mismatch would silently sample the wrong pixels rather
        /// than erroring, which is why the key must never lose them.
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    },
    /// A luminance range — a pure function of the **pixel's value**.
    ///
    /// ⚠⚠ The third kind, and the reason this enum earns its keep a second time. Analytic masks
    /// answer from `(u, v)` and a brush answers from a rasterized buffer; a range mask needs the
    /// sample, which `apply`'s `map_pixels_xy` closure already has in hand. Widening
    /// `Mask::analytic_coverage` to take a pixel would churn every variant for one that wants it.
    ///
    /// ⚠ **Keyed on the `base` pixel — the value *before* this mask's own adjustment.** Reading
    /// the adjusted one would make the mask chase its own effect: lifting the shadows moves them
    /// out of the band that selected them, so the correction would fade as it applied.
    ///
    /// ⚠ A colour range is the same *kind* and joins below as a sibling variant.
    Range(edit_model::LuminanceRange),
    /// A colour range — a pure function of the **pixel's hue**, gated on its chroma.
    ///
    /// ⚠ The sibling [`CoverageSource::Range`] anticipated, and it inherits every one of that
    /// variant's notes: it is keyed on the `base` pixel, never the adjusted one, or the mask
    /// chases its own effect — a saturation lift moves a colour out of the band that selected it,
    /// so the correction would fade as it applied.
    Colour(edit_model::ColourRange),
}

/// Display-referred lightness of a working-space pixel, in 0..=100.
///
/// ⚠ **`color::luminance` is the one luminance rule in this codebase** — it returns ProPhoto's Y,
/// which is real physical luminance and therefore agrees with Rec.709 for real colours. A
/// hand-rolled weight triple here would be invisible per stage and wrong everywhere.
///
/// ⚠ `linear_to_srgb` clamps its input to `[0, 1]`, which is exactly the *index* clamp a range
/// mask wants and never touches the pixel itself: a specular highlight still selects at the top of
/// the scale while keeping every bit of its headroom for the adjustment that follows.
pub(super) fn lightness(rgb: [f32; 3]) -> f32 {
    color::linear_to_srgb(color::luminance(rgb)) * 100.0
}

/// Oklab hue in degrees and Oklab chroma of a working-space pixel.
///
/// ⚠ **The same measurement `HslStage` makes, from the same function** — `prophoto_to_oklab`,
/// `b.atan2(a)`, `a.hypot(b)` — so "red" means the same angle to a colour range mask as it does to
/// the Red band of HSL. A second hue convention here would be invisible per stage and disagree
/// everywhere; `BAND_HUES` is the table both are measured against.
///
/// ⚠ `atan2` returns `-180..=180`. Nothing normalises it here on purpose:
/// `ColourRange::coverage_at` takes both its span and its offset with `rem_euclid(360)`, so a
/// negative angle is already handled by the one wrap rule rather than by a second one.
///
/// ⚠ Chroma is **not** clamped and must not be: it is the gate's own input, and a wide-gamut
/// colour legitimately reads above anything sRGB can express.
pub(super) fn hue_chroma(rgb: [f32; 3]) -> (f32, f32) {
    let [_, a, b] = color::oklab::prophoto_to_oklab(rgb);
    (b.atan2(a).to_degrees(), a.hypot(b))
}

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
    /// An AI mask (M9.13): a raster computed outside the pipeline and handed in through
    /// `BrushCoverage`, already resampled to the frame with its threshold and invert applied.
    /// ⚠⚠ **No map means coverage 0** — the network has not answered yet (or its model is missing),
    /// and a mask that silently covered everything would be the worse failure.
    Raster {
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    },
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
pub fn lightness(rgb: [f32; 3]) -> f32 {
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
pub fn hue_chroma(rgb: [f32; 3]) -> (f32, f32) {
    let [_, a, b] = color::oklab::prophoto_to_oklab(rgb);
    (b.atan2(a).to_degrees(), a.hypot(b))
}

/// Per-pixel facts a pixel-dependent component reads, computed **at most once per pixel** and
/// shared by every component that wants them.
///
/// ⚠⚠ **Three colour-range components must cost ONE Oklab round trip, not three.**
/// `docs/BACKLOG.md`'s decomposition puts that round trip at 42% of a range mask's coverage cost,
/// so a per-component conversion would make a composite's cost linear in a component count that
/// ought to be nearly free — and it would be **invisible**, because the pixels come out
/// bit-identical either way and it merely costs more. That is why the counter below exists.
///
/// ⚠ Constructed per pixel and dropped there: two `Option`s on the stack, and different pixels
/// have different values, so there is nothing to share across them.
#[derive(Default)]
pub(super) struct BaseFacts {
    lightness: Option<f32>,
    hue_chroma: Option<(f32, f32)>,
}

// How many Oklab conversions this thread's evaluator has performed.
//
// ⚠ **Thread-local, not a global counter.** libtest runs tests in parallel threads, so a shared
// `AtomicUsize` would let two tests using it race — and a racing guard is either flaky or
// vacuous, which is worse than none.
//
// ⚠ A counter rather than a timing: a benchmark cannot fail a build, and the sharing this guards
// is otherwise unobservable.
//
// ⚠ Line comments rather than `///`: a doc comment on a macro invocation is `unused_doc_comments`,
// which `-D warnings` rejects.
#[cfg(test)]
thread_local! {
    static CONVERSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reset this thread's conversion count.
#[cfg(test)]
pub(super) fn conversions_reset() {
    CONVERSIONS.with(|c| c.set(0));
}

/// Read this thread's conversion count.
#[cfg(test)]
pub(super) fn conversions() -> usize {
    CONVERSIONS.with(|c| c.get())
}

impl BaseFacts {
    fn lightness(&mut self, rgb: [f32; 3]) -> f32 {
        *self.lightness.get_or_insert_with(|| lightness(rgb))
    }

    fn hue_chroma(&mut self, rgb: [f32; 3]) -> (f32, f32) {
        *self.hue_chroma.get_or_insert_with(|| {
            #[cfg(test)]
            CONVERSIONS.with(|c| c.set(c.get() + 1));
            hue_chroma(rgb)
        })
    }
}

/// Where one pixel sits, and the rasterized maps a painted component reads.
pub(super) struct PixelCtx<'a> {
    /// Position in the **frame's** normalised coordinates.
    pub(super) u: f32,
    pub(super) v: f32,
    pub(super) aspect: f32,
    /// Frame pixel coordinates, for indexing a coverage map.
    ///
    /// ⚠ FRAME coordinates, never the region-relative ones — the note `MaskStage::apply` already
    /// carries, because coverage is defined over the frame and a region-relative index would slide
    /// the mask as you pan.
    pub(super) fx: usize,
    pub(super) fy: usize,
    /// One entry per component, index 0 being the base. `None` where the component is not painted,
    /// or where its map could not be built.
    pub(super) maps: &'a [Option<std::sync::Arc<crate::brush::CoverageMap>>],
}

/// A mask's coverage: a base shape plus the refinements folded onto it.
///
/// ⚠ Resolved **once at construction**, exactly as the single [`CoverageSource`] was, so `apply`
/// still matches outside the pixel loop.
pub(super) struct Composite {
    pub(super) base: CoverageSource,
    pub(super) refine: Vec<(edit_model::MaskOp, CoverageSource)>,
    /// The whole-mask Invert (M10.12a), applied after the last refinement.
    pub(super) invert: bool,
}

impl Composite {
    /// Coverage of one component, in `0.0..=1.0`.
    ///
    /// ⚠⚠ `base_px` is the pixel **before this mask's own adjustment**, for every component. A
    /// mask keyed on its own output chases its effect — lifting the shadows moves them out of the
    /// band that selected them — and every component reading the *same* base is what stops there
    /// being a second rule to keep in step.
    fn source_at(
        src: &CoverageSource,
        j: usize,
        cx: &PixelCtx<'_>,
        base_px: [f32; 3],
        facts: &mut BaseFacts,
    ) -> f32 {
        match src {
            CoverageSource::Analytic(m) => {
                m.analytic_coverage(cx.u, cx.v, cx.aspect).unwrap_or(0.0)
            }
            CoverageSource::Range(r) => r.coverage(facts.lightness(base_px)),
            CoverageSource::Colour(r) => {
                let (hue, chroma) = facts.hue_chroma(base_px);
                r.coverage_at(hue, chroma)
            }
            CoverageSource::Painted { .. } | CoverageSource::Raster { .. } => {
                match cx.maps.get(j).and_then(|m| m.as_ref()) {
                    Some(cov) => cov.at(cx.fx, cx.fy) as f32 / 255.0,
                    None => 0.0,
                }
            }
        }
    }

    /// The composite's coverage at one pixel: the base, then each refinement folded on **in list
    /// order**.
    pub(super) fn at(&self, cx: &PixelCtx<'_>, base_px: [f32; 3], facts: &mut BaseFacts) -> f32 {
        let mut a = Self::source_at(&self.base, 0, cx, base_px, facts).clamp(0.0, 1.0);
        for (j, (op, src)) in self.refine.iter().enumerate() {
            let b = Self::source_at(src, j + 1, cx, base_px, facts).clamp(0.0, 1.0);
            a = op.combine(a, b);
        }
        let a = a.clamp(0.0, 1.0);
        // ⚠ After the fold, never before it: inverting the base alone is a different mask.
        if self.invert {
            1.0 - a
        } else {
            a
        }
    }

    /// Whether an inert base map is enough to skip this mask entirely.
    ///
    /// ⚠⚠ **An inert base does NOT make an inert composite.** `Subtract` and `Intersect` preserve
    /// zero (`0·(1−b)` and `0·b`); `Add` does not (`0 + 1·b = b`), and there the added component is
    /// the whole mask. Short-circuiting on the base alone would make an added component vanish
    /// whenever the base happened to be an unpainted brush — **silently**, because the render is
    /// then simply the unmasked photograph rather than anything that looks like an error.
    /// Inverting is the second way (M10.12a): `1 − 0` is everything.
    pub(super) fn base_inert_is_enough(&self) -> bool {
        !self.invert
            && !self
                .refine
                .iter()
                .any(|(op, _)| matches!(op, edit_model::MaskOp::Add))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{ColourRange, LinearGradient, LuminanceRange, Mask, MaskOp};

    fn analytic(m: Mask) -> CoverageSource {
        CoverageSource::Analytic(m)
    }

    /// A gradient covering `0.0` at the left edge and `1.0` at the right.
    fn ramp() -> CoverageSource {
        analytic(Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        }))
    }

    /// A gradient that is already at full coverage everywhere in the frame.
    fn everything() -> CoverageSource {
        analytic(Mask::Linear(LinearGradient {
            from: [-1.0, 0.0],
            to: [-0.5, 0.0],
            invert: false,
        }))
    }

    /// A context for a composite with no painted component.
    fn ctx(u: f32, v: f32) -> PixelCtx<'static> {
        PixelCtx {
            u,
            v,
            aspect: 1.0,
            fx: 0,
            fy: 0,
            maps: &[],
        }
    }

    fn coverage(c: &Composite, px: [f32; 3]) -> f32 {
        c.at(&ctx(0.5, 0.5), px, &mut BaseFacts::default())
    }

    #[test]
    fn a_composite_with_no_refinements_is_its_base() {
        let c = Composite {
            base: ramp(),
            refine: Vec::new(),
            invert: false,
        };
        let got = coverage(&c, [0.5; 3]);
        assert!((got - 0.5).abs() < 1e-4, "got {got}");
    }

    #[test]
    fn a_subtracted_component_cuts_out_of_the_base() {
        let c = Composite {
            base: everything(),
            refine: vec![(MaskOp::Subtract, ramp())],
            invert: false,
        };
        let got = coverage(&c, [0.5; 3]);
        assert!(
            (got - 0.5).abs() < 1e-4,
            "1.0 minus 0.5 coverage is 0.5, got {got}"
        );
    }

    #[test]
    fn an_intersected_component_is_the_product() {
        let c = Composite {
            base: ramp(),
            refine: vec![(MaskOp::Intersect, ramp())],
            invert: false,
        };
        let got = coverage(&c, [0.5; 3]);
        assert!((got - 0.25).abs() < 1e-4, "0.5 * 0.5 is 0.25, got {got}");
    }

    #[test]
    fn three_colour_components_cost_one_oklab_round_trip() {
        // ⚠⚠ **The only test that can see the sharing.** A per-component round trip renders
        // bit-identically and merely costs three times as much, so nothing else in the suite
        // distinguishes them — not a golden, not a pixel comparison, not a region invariant.
        let colour = || CoverageSource::Colour(ColourRange::default());
        let c = Composite {
            base: colour(),
            refine: vec![(MaskOp::Add, colour()), (MaskOp::Add, colour())],
            invert: false,
        };
        conversions_reset();
        let _ = coverage(&c, [0.3, 0.4, 0.5]);
        assert_eq!(
            conversions(),
            1,
            "three colour components must share one conversion"
        );
    }

    #[test]
    fn a_composite_of_only_analytic_shapes_converts_nothing() {
        // ⚠ The other half: the facts are computed **lazily**, so a composite that never asks for
        // a pixel pays nothing at all. A version that computed them up front would pass the test
        // above and fail this one.
        let c = Composite {
            base: ramp(),
            refine: vec![(MaskOp::Intersect, ramp())],
            invert: false,
        };
        conversions_reset();
        let _ = coverage(&c, [0.3, 0.4, 0.5]);
        assert_eq!(conversions(), 0);
    }

    #[test]
    fn a_luminance_component_reads_the_base_pixel() {
        let c = Composite {
            base: CoverageSource::Range(LuminanceRange::default()),
            refine: Vec::new(),
            invert: false,
        };
        let dark = coverage(&c, [0.01; 3]);
        let bright = coverage(&c, [0.9; 3]);
        assert!(
            dark > bright,
            "the default range is the darker half: {dark} vs {bright}"
        );
    }

    #[test]
    fn an_inert_base_is_only_enough_without_an_add() {
        // ⚠⚠ The `is_inert` hazard as a unit test. `MaskStage` carries the end-to-end one; this
        // pins the rule itself, whose sabotage is dropping the `Add` clause.
        let c = |op| Composite {
            base: ramp(),
            refine: vec![(op, ramp())],
            invert: false,
        };
        assert!(c(MaskOp::Subtract).base_inert_is_enough());
        assert!(c(MaskOp::Intersect).base_inert_is_enough());
        assert!(
            !c(MaskOp::Add).base_inert_is_enough(),
            "an added component is the whole mask when the base covers nothing"
        );
        assert!(
            Composite {
                base: ramp(),
                refine: Vec::new(),
                invert: false,
            }
            .base_inert_is_enough(),
            "with no refinement at all the old fast path must still apply"
        );
    }

    #[test]
    fn the_fold_follows_list_order() {
        // ⚠ Order is observable, so the same two components in the other order must differ.
        let a = Composite {
            base: everything(),
            refine: vec![(MaskOp::Subtract, ramp()), (MaskOp::Add, ramp())],
            invert: false,
        };
        let b = Composite {
            base: everything(),
            refine: vec![(MaskOp::Add, ramp()), (MaskOp::Subtract, ramp())],
            invert: false,
        };
        let (x, y) = (coverage(&a, [0.5; 3]), coverage(&b, [0.5; 3]));
        assert!(
            (x - y).abs() > 0.1,
            "the two orders must differ: {x} vs {y}"
        );
    }

    #[test]
    fn an_inverted_composite_is_exactly_one_minus_the_fold() {
        // ⚠ Over a ramp and with a refinement, so the flip is checked at partial coverage and
        // after the fold — a flip applied to the base alone gives a different number.
        for u in [0.0, 0.1, 0.37, 0.5, 0.83, 1.0] {
            let c = |invert| Composite {
                base: everything(),
                refine: vec![(MaskOp::Subtract, ramp())],
                invert,
            };
            let f = |c: &Composite| c.at(&ctx(u, 0.5), [0.5; 3], &mut BaseFacts::default());
            let (plain, inv) = (f(&c(false)), f(&c(true)));
            assert!(
                (inv - (1.0 - plain)).abs() < 1e-6,
                "u {u}: {inv} vs 1 − {plain}"
            );
        }
    }

    #[test]
    fn an_inverted_mask_is_never_inert_on_its_base() {
        // ⚠⚠ Inverting zero covers everything, so an inert base is not enough to skip the mask.
        let c = Composite {
            base: ramp(),
            refine: Vec::new(),
            invert: true,
        };
        assert!(
            !c.base_inert_is_enough(),
            "an inverted fold of nothing is the whole frame"
        );
    }
}

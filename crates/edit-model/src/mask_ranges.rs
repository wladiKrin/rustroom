//! The range masks: luminance, colour and (M10.16a) depth.

use serde::{Deserialize, Serialize};

/// A luminance range mask: coverage is full inside `[low, high]` and ramps away outside it.
///
/// ⚠⚠ **This is the first mask that is a function of the PIXEL rather than of its POSITION.**
/// [`LinearGradient`] and [`RadialGradient`] answer [`Mask::analytic_coverage`], which never sees
/// a pixel value; [`Brush`] is sampled data. A range mask is a third kind, and that is why
/// `MaskStage` resolves it to its own `CoverageSource` variant rather than widening
/// `analytic_coverage`'s signature — every existing variant would then have to accept a pixel it
/// has no use for.
///
/// ⚠ **`low`/`high` are DISPLAY-REFERRED lightness, 0..=100**, not the scene-referred linear
/// luminance the pipeline works in. Two reasons, and the second is the load-bearing one: mid-grey
/// sits at 0.18 in linear light, so a linear slider would crowd almost every tone a photographer
/// cares about into its bottom fifth; and 0..=100 maps linearly onto the histogram's own 0..=255,
/// so "everything below the hump" is a number you can read off the screen. The conversion is
/// `linear_to_srgb(luminance(rgb)) * 100`, and it lives at the single call site in
/// `pipeline::stages::mask` — `edit-model` deliberately has no `color` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LuminanceRange {
    /// Lower edge of the fully-covered band, in display-referred lightness 0..=100.
    pub low: f32,
    /// Upper edge of the fully-covered band, in display-referred lightness 0..=100.
    pub high: f32,
    /// 0 = a hard edge at each end of the band, 100 = the widest ramp.
    ///
    /// ⚠ Named and scaled to match [`RadialGradient::feather`] and the vignette's, so the app has
    /// one feather idiom rather than three. As there, the ramp is a **smoothstep**: a linear ramp
    /// leaves a visible crease where its derivative jumps, which on a smooth sky is exactly the
    /// artefact a range mask is reached for to avoid.
    pub feather: f32,
    /// Flip coverage, as every other shape does.
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived**, for [`RadialGradient::default`]'s reason: the container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one would
/// give every partial sidecar `high: 0` — a band of zero width at black — and `feather: 0`.
///
/// The default selects **the darker half with a soft shoulder**: a visible, obviously-a-mask
/// starting point, and one click of Invert away from the highlights. A default covering the whole
/// range would be indistinguishable from a global adjustment, which reads as the mask doing
/// nothing.
impl Default for LuminanceRange {
    fn default() -> Self {
        Self {
            low: 0.0,
            high: 50.0,
            feather: 50.0,
            invert: false,
        }
    }
}

impl LuminanceRange {
    /// The widest ramp `feather` can ask for, in lightness points either side of the band.
    ///
    /// Half the full 0..=100 range, so feather 100 reaches from one end of the scale to the other
    /// and the mask degenerates gracefully into "everything, weighted" rather than clipping.
    ///
    /// ⚠ `pub` so a GPU twin substitutes it rather than transcribing it — the rule
    /// `pipeline::SATURATION_EPS` already follows.
    pub const FEATHER_SCALE: f32 = 50.0;

    /// Below this the ramp is treated as a hard edge.
    ///
    /// ⚠⚠ **Load-bearing at exactly ONE input, and it is narrower than it looks — measured, not
    /// reasoned.** The obvious guess, copied from [`RadialGradient::FEATHER_EPS`], is that
    /// `w == 0` divides by zero at `l == lo`. It does: `rising` is then `0.0/0.0` = NaN. But
    /// `falling` at that point is `+inf`, clamping to `1.0`, and **`f32::min` returns the
    /// non-NaN operand** — so the NaN is silently absorbed and the answer comes out *correct*.
    /// A test probing a normal band's edge therefore cannot see this guard at all; the sabotage
    /// step is what established that.
    ///
    /// The one input where both shoulders are NaN together, so `min` has nothing to fall back on,
    /// is a **zero-width band probed exactly on itself** — `low == high == l` at feather 0, which
    /// is two slider drags away. Unguarded, coverage is NaN, `MaskStage`'s blend propagates it,
    /// and the selected tones quantize to **black**.
    ///
    /// ⚠ `pub` so a GPU twin substitutes it rather than transcribing it.
    pub const FEATHER_EPS: f32 = 1e-4;

    /// Coverage for a pixel whose display-referred lightness is `lightness`, in `0.0..=1.0`.
    ///
    /// ⚠⚠ **The INDEX is clamped to `[0, 100]`; the pixel's own value is not.** Scene-referred
    /// samples above white encode to lightness above 100, and clamping *them* would be the
    /// `ProfileHueSatMap` trap — dragging every highlight back to white and destroying the
    /// headroom the wide working space exists for. Here only the *selector* is clamped, and the
    /// adjustment still applies to the untouched pixel, so a `high` of 100 correctly means "and
    /// everything brighter".
    pub fn coverage(&self, lightness: f32) -> f32 {
        let c = band(
            lightness.clamp(0.0, 100.0),
            self.low,
            self.high,
            self.feather,
        );
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

/// Coverage of `x` (on the 0..=100 scale) by the band `[low, high]` with a `feather` 0..=100 ramp
/// either side — the luminance range's band, and since M10.16a the depth range's too: ONE function,
/// so the two sliders that look alike behave alike (`luminance_and_depth_share_one_band`).
///
/// ⚠ The luminance range's GPU twin transcribes this, substituting [`LuminanceRange::FEATHER_SCALE`]
/// and [`LuminanceRange::FEATHER_EPS`]; a change here is a change there.
fn band(x: f32, low: f32, high: f32, feather: f32) -> f32 {
    // ⚠ Sorted rather than trusted. The sliders can be dragged past each other and the
    // sidecar is hand-editable; an unsorted pair would silently make the band empty, which
    // reads as the mask having stopped working rather than as a value being wrong.
    let (lo, hi) = (low.min(high), low.max(high));
    let w = (feather / 100.0).clamp(0.0, 1.0) * LuminanceRange::FEATHER_SCALE;
    if w <= LuminanceRange::FEATHER_EPS {
        if x >= lo && x <= hi {
            1.0
        } else {
            0.0
        }
    } else {
        let rising = ((x - (lo - w)) / w).clamp(0.0, 1.0);
        let falling = (((hi + w) - x) / w).clamp(0.0, 1.0);
        let t = rising.min(falling);
        t * t * (3.0 - 2.0 * t)
    }
}

/// A depth range mask (M10.16a): coverage is full for the distances inside `[near, far]` and ramps
/// away outside them — over the photo's estimated depth map, which the app computes once with a
/// depth network and caches outside the edits.
///
/// ⚠ **`near`/`far` are POSITIONS in this photo, 0..=100** — 0 its nearest point, 100 its farthest —
/// not distances: the network's depth is relative, an order of nearer and farther with no metres.
/// The band itself is the luminance range's ([`band`]), feather idiom and all.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DepthRange {
    /// The nearest edge of the fully-covered band, 0 (the nearest point) ..= 100 (the farthest).
    pub near: f32,
    /// The farthest edge of the fully-covered band.
    pub far: f32,
    /// 0 = a hard edge at each end of the band, 100 = the widest ramp.
    pub feather: f32,
    /// Flip coverage, as every other shape does.
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived**, for [`LuminanceRange`]'s reason: the container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one would give
/// a partial sidecar a zero-width band at the nearest point.
///
/// The default selects **the far half with a soft shoulder** — a visibly-a-mask start aimed at the
/// commonest use, the background; Invert is one click to the foreground.
impl Default for DepthRange {
    fn default() -> Self {
        Self {
            near: 50.0,
            far: 100.0,
            feather: 50.0,
            invert: false,
        }
    }
}

impl DepthRange {
    /// Coverage at a depth-map `position`, 0 (nearest) ..= 1 (farthest). ⚠ As the luminance range's,
    /// only the selector is clamped — and a non-finite position selects as the nearest point rather
    /// than turning the blend into a NaN.
    pub fn coverage(&self, position: f32) -> f32 {
        let x = if position.is_finite() {
            (position * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let c = band(x, self.near, self.far, self.feather);
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

/// A colour range mask: coverage is full inside the hue band `[low, high]` and ramps away
/// outside it, gated so that near-neutral pixels are never selected.
///
/// The companion to [`LuminanceRange`], mirroring it field for field with **two deliberate
/// differences**: hue wraps, and a chroma gate.
///
/// ⚠ **`low`/`high` are Oklab hue DEGREES, 0..=360** — the same angle `HslStage`'s `BAND_HUES`
/// are measured in, so "red" means the same thing to both. As with [`LuminanceRange`], the
/// conversion from a working-space pixel lives at the single call site in
/// `pipeline::stages::mask`: `edit-model` deliberately has no `color` dependency, which is why
/// there is no pixel-facing wrapper here and [`ColourRange::coverage_at`] takes the hue and
/// chroma already in hand.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourRange {
    /// Lower edge of the fully-covered band, in Oklab hue degrees 0..=360.
    pub low: f32,
    /// Upper edge. ⚠⚠ May be **less** than `low`, which means the band crosses 0° — see
    /// `a_range_across_zero_selects_red_and_not_everything_else`.
    pub high: f32,
    /// 0 = a hard edge at each end, 100 = the widest ramp.
    ///
    /// ⚠ Named and scaled to match [`LuminanceRange::feather`], [`RadialGradient::feather`] and
    /// the vignette's, so the app keeps one feather idiom rather than four. Smoothstep, as there:
    /// a linear ramp leaves a visible crease where its derivative jumps, which on a smooth sky is
    /// exactly the artefact a range mask is reached for to avoid.
    pub feather: f32,
    /// Below this chroma a pixel is not selected at all.
    ///
    /// ⚠⚠ **Load-bearing, not a refinement.** A near-neutral pixel's hue is sensor noise:
    /// `HslStage` measured chroma ~0.003 at ±0.6% noise against ~0.06 for a real colour, and
    /// **37.8× noise amplification** without a taper. Exposed as a control rather than fixed
    /// because what counts as "coloured enough" genuinely differs between a pastel skin tone and
    /// a saturated sign — which is exactly the judgement HSL's fixed `CHROMA_FADE` cannot make.
    pub min_chroma: f32,
    /// Flip coverage, as every other shape does.
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived**, for [`LuminanceRange::default`]'s reason: the container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one would
/// give every partial sidecar a band of **zero width at 0°** plus `feather: 0` and
/// `min_chroma: 0` — a mask that selects nothing, silently.
///
/// The default selects **the warm half of the wheel with a soft shoulder**: 0°..=120° spans reds,
/// oranges and yellows, which is the band a colour range is most often reached for, and it is
/// visibly a selection rather than a global adjustment. `min_chroma` starts at HSL's measured
/// `CHROMA_FADE` — the value that separates a real colour from sensor noise on this project's own
/// measurement — so the gate is doing its job before anyone touches the slider.
impl Default for ColourRange {
    fn default() -> Self {
        Self {
            low: 0.0,
            high: 120.0,
            feather: 50.0,
            min_chroma: 0.02,
            invert: false,
        }
    }
}

impl ColourRange {
    /// The widest ramp `feather` can ask for, in hue degrees either side of the band.
    ///
    /// Half a turn, so feather 100 reaches all the way round from either edge and the mask
    /// degenerates gracefully into "everything, weighted" rather than clipping — the analogue of
    /// [`LuminanceRange::FEATHER_SCALE`] being half its 0..=100 scale.
    ///
    /// ⚠ `pub` so a GPU twin substitutes it rather than transcribing it.
    pub const FEATHER_SCALE: f32 = 180.0;

    /// Below this the ramp is treated as a hard edge.
    ///
    /// ⚠ Load-bearing for the same reason [`LuminanceRange::FEATHER_EPS`] is, and reachable by
    /// the same two slider drags: with `w == 0` the shoulders are `0.0/0.0`, and a zero-width
    /// band probed exactly on itself has no non-NaN operand for `f32::min` to fall back on.
    /// `MaskStage`'s blend then propagates the NaN and the selected colours quantize to black.
    ///
    /// ⚠ `pub` so a GPU twin substitutes it rather than transcribing it.
    pub const FEATHER_EPS: f32 = 1e-4;

    /// The chroma over which coverage fades in, above `min_chroma`.
    ///
    /// ⚠ A taper rather than a cutoff, which is the shape `HslStage` arrived at by measurement:
    /// a hard threshold puts a visible edge through a smooth gradient wherever the photograph
    /// happens to cross it, in exactly the near-neutral region where the hue driving that edge is
    /// noise. Its own `CHROMA_FADE` is this width.
    ///
    /// ⚠ `pub` so a GPU twin substitutes it rather than transcribing it.
    pub const CHROMA_FADE: f32 = 0.02;

    /// Coverage for a pixel of Oklab hue `hue_degrees` and Oklab chroma `chroma`, in `0.0..=1.0`.
    ///
    /// ⚠⚠ **The hue axis WRAPS, and that is the one place this must not mirror
    /// [`LuminanceRange`].** Lightness is an interval and hue is a circle: sRGB red sits at 29.23°
    /// and magenta at 328.36°, either side of 0°, so a non-wrapping band cannot express the single
    /// most commonly masked colour. `low > high` therefore means the band **crosses 0°** —
    /// `340 → 20` selects red — rather than being sorted into `20..340`, which is what
    /// `LuminanceRange` correctly does with a crossed pair and would here select every colour but
    /// the one asked for.
    ///
    /// ⚠ Both the offset and the span are taken with `rem_euclid(360)`, so the wrapped and
    /// unwrapped cases are **one expression with no branch**. A branch on `low > high` is a second
    /// rule that can disagree with the first.
    pub fn coverage_at(&self, hue_degrees: f32, chroma: f32) -> f32 {
        let span = (self.high - self.low).rem_euclid(360.0);
        let offset = (hue_degrees - self.low).rem_euclid(360.0);
        let w = (self.feather / 100.0).clamp(0.0, 1.0) * Self::FEATHER_SCALE;
        let band = if w <= Self::FEATHER_EPS {
            if offset <= span {
                1.0
            } else {
                0.0
            }
        } else {
            // Distance outside the band, measured the short way round from whichever edge is
            // nearer: 0 inside it, growing to at most half a turn opposite.
            let past = (offset - span).max(0.0);
            let before = (360.0 - offset).max(0.0);
            let t = (1.0 - past.min(before) / w).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        // ⚠⚠ The chroma gate, applied BEFORE `invert`. Inverting must give "everything except
        // these colours", and a near-neutral pixel has no colour to be excepted — putting the
        // gate after the flip would make an inverted mask select every grey in the photograph,
        // which is precisely the speckle this gate exists to prevent, at full strength.
        let gate = ((chroma - self.min_chroma) / Self::CHROMA_FADE).clamp(0.0, 1.0);
        let gate = gate * gate * (3.0 - 2.0 * gate);
        let c = band * gate;
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mask;

    /// M10.16a: ONE band function — a depth range at position `x / 100` covers exactly what a
    /// luminance range covers at lightness `x`, for every band, feather and point.
    #[test]
    fn luminance_and_depth_share_one_band() {
        let mut checked = 0;
        for (low, high) in [(0.0, 50.0), (20.0, 80.0), (60.0, 60.0), (90.0, 10.0)] {
            for feather in [0.0, 1e-5, 30.0, 100.0] {
                let l = LuminanceRange {
                    low,
                    high,
                    feather,
                    invert: false,
                };
                let d = DepthRange {
                    near: low,
                    far: high,
                    feather,
                    invert: false,
                };
                for i in 0..=200 {
                    let x = i as f32 / 2.0;
                    // ⚠ The luminance range is asked at exactly the band point the depth range turns
                    // `p` into (`p * 100`), so the two are compared bit for bit: `x / 100 * 100` is
                    // not `x` in f32 (52.5 comes back 52.499996), and at a hard edge that ulp is a
                    // whole step of coverage — a difference of input, not of band.
                    let p = x / 100.0;
                    assert_eq!(
                        l.coverage(p * 100.0).to_bits(),
                        d.coverage(p).to_bits(),
                        "band {low}..{high} feather {feather} at {x}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 3000);
    }

    /// Review Focus 5: the sliders dragged past each other, and a zero-width band at feather 0.
    #[test]
    fn a_depth_band_is_sorted_and_finite_at_its_edges() {
        let d = |near, far, feather| DepthRange {
            near,
            far,
            feather,
            invert: false,
        };
        assert_eq!(
            d(80.0, 20.0, 10.0).coverage(0.5),
            d(20.0, 80.0, 10.0).coverage(0.5)
        );
        assert_eq!(d(50.0, 50.0, 0.0).coverage(0.5), 1.0);
        for i in 0..=100 {
            let c = d(50.0, 50.0, 0.0).coverage(i as f32 / 100.0);
            assert!(c.is_finite(), "{i}");
        }
        // Out-of-range positions are clamped as selectors, never NaN.
        assert!(!d(0.0, 100.0, 0.0).coverage(f32::NAN).is_nan());
        assert_eq!(d(90.0, 100.0, 0.0).coverage(1.5), 1.0);
    }

    /// The far half: a visibly-a-mask start aimed at the background.
    #[test]
    fn the_depth_default_is_the_far_half() {
        let d = DepthRange::default();
        assert_eq!(d.coverage(1.0), 1.0);
        assert_eq!(d.coverage(0.0), 0.0);
        assert_eq!(d.coverage(0.5), 1.0, "the band's own near edge");
        assert!(
            d.coverage(0.3) > 0.0 && d.coverage(0.3) < 1.0,
            "the feathered shoulder"
        );
        let inv = DepthRange { invert: true, ..d };
        assert_eq!(inv.coverage(1.0), 0.0);
        // A partial sidecar fills from the hand-written default, not a derived zero.
        let m: Mask = serde_json::from_str(r#"{"Depth":{}}"#).unwrap();
        assert_eq!(m, Mask::Depth(DepthRange::default()));
    }

    #[test]
    fn a_luminance_range_has_no_analytic_coverage() {
        // ⚠ The same reason the brush answers `None`, arrived at from the other direction: a
        // brush is sampled data, a range is a function of the *pixel*. Neither can be answered
        // from `(u, v)`, and a `0.0` arm here would compile, run, and produce a mask that
        // silently covers nothing — which on a range mask is indistinguishable from "the tones
        // you asked for are not in this photograph".
        let m = Mask::Luminance(LuminanceRange::default());
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.0), None);
    }

    #[test]
    fn a_luminance_range_covers_inside_its_band_and_not_outside_it() {
        let r = LuminanceRange {
            low: 30.0,
            high: 60.0,
            feather: 0.0,
            invert: false,
        };
        assert_eq!(r.coverage(45.0), 1.0, "the middle of the band");
        assert_eq!(r.coverage(30.0), 1.0, "the lower edge is inside");
        assert_eq!(r.coverage(60.0), 1.0, "the upper edge is inside");
        assert_eq!(r.coverage(29.0), 0.0, "below");
        assert_eq!(r.coverage(61.0), 0.0, "above");
    }

    #[test]
    fn feather_widens_the_band_symmetrically_and_ramps_smoothly() {
        // ⚠ Probes the *shoulder*, not merely either side of it. A test that only asked "inside
        // is 1, far outside is 0" passes with the ramp deleted entirely — the discriminating
        // question is whether the values between are strictly ordered and land at 0.5 exactly
        // half a ramp-width out, which is what a smoothstep centred on the edge gives.
        let r = LuminanceRange {
            low: 40.0,
            high: 60.0,
            feather: 20.0, // ramp half-width = 10 lightness points
            invert: false,
        };
        assert_eq!(r.coverage(50.0), 1.0, "still full inside the band");
        let below = r.coverage(35.0);
        let above = r.coverage(65.0);
        assert!(
            (below - 0.5).abs() < 1e-5,
            "half a ramp below the band should be half covered: {below}"
        );
        assert!(
            (above - below).abs() < 1e-5,
            "the two shoulders must be mirror images: {below} vs {above}"
        );
        assert!(
            r.coverage(38.0) > below && below > r.coverage(32.0),
            "the shoulder must be monotonic"
        );
        assert_eq!(r.coverage(29.0), 0.0, "past the ramp entirely");
    }

    #[test]
    fn feather_zero_is_a_hard_edge_exactly_on_the_boundary() {
        // Probes **on** the boundary rather than either side of it — but see the test below:
        // ⚠ this one is NOT what guards `FEATHER_EPS`, and believing otherwise is the trap.
        let r = LuminanceRange {
            low: 25.0,
            high: 75.0,
            feather: 0.0,
            invert: false,
        };
        assert_eq!(r.coverage(25.0), 1.0, "exactly on the lower edge");
        assert_eq!(r.coverage(75.0), 1.0, "exactly on the upper edge");
    }

    #[test]
    fn a_zero_width_band_at_feather_zero_does_not_produce_nan() {
        // ⚠⚠ **The only test that can see the `FEATHER_EPS` guard, and the sabotage step is what
        // proved that.** Removing the guard leaves the whole suite green, because at an ordinary
        // band's edge exactly one shoulder is NaN and `f32::min` returns the other — measured:
        // `rising=NaN falling=1 -> 1`. Both shoulders are NaN together only when the band has
        // **zero width and the probe sits on it**, where `min(NaN, NaN)` is NaN, the blend in
        // `MaskStage` propagates it, and the selected tones quantize to black.
        //
        // Two slider drags away, so this is a reachable state rather than a curiosity.
        for feather in [0.0, LuminanceRange::FEATHER_EPS / 2.0] {
            let r = LuminanceRange {
                low: 50.0,
                high: 50.0,
                feather,
                invert: false,
            };
            let c = r.coverage(50.0);
            assert!(c.is_finite(), "coverage must never be NaN: {c}");
            assert_eq!(c, 1.0, "a zero-width band still selects its own tone");
            assert_eq!(r.coverage(49.0), 0.0, "and nothing either side of it");
        }
    }

    #[test]
    fn invert_flips_the_coverage() {
        let mut r = LuminanceRange {
            low: 0.0,
            high: 50.0,
            feather: 0.0,
            invert: false,
        };
        assert_eq!(r.coverage(25.0), 1.0);
        assert_eq!(r.coverage(75.0), 0.0);
        r.invert = true;
        assert_eq!(r.coverage(25.0), 0.0);
        assert_eq!(r.coverage(75.0), 1.0);
    }

    #[test]
    fn a_lightness_above_white_is_selected_by_a_band_reaching_the_top() {
        // ⚠⚠ Scene-referred headroom. A specular highlight encodes above 100, and `high: 100`
        // must mean "and everything brighter" — clamping the *index* is what delivers that,
        // where clamping the pixel would be the `ProfileHueSatMap` trap.
        let r = LuminanceRange {
            low: 80.0,
            high: 100.0,
            feather: 0.0,
            invert: false,
        };
        assert_eq!(r.coverage(140.0), 1.0, "well above white");
        assert_eq!(r.coverage(-20.0), 0.0, "and below black is still outside");
    }

    #[test]
    fn a_crossed_pair_is_sorted_rather_than_empty() {
        // Dragging `low` past `high` must select the band between them, not nothing at all.
        let r = LuminanceRange {
            low: 70.0,
            high: 30.0,
            feather: 0.0,
            invert: false,
        };
        assert_eq!(r.coverage(50.0), 1.0);
        assert_eq!(r.coverage(20.0), 0.0);
    }

    #[test]
    fn the_default_range_is_the_darker_half_with_a_soft_shoulder() {
        // ⚠ Asserts the **values**, not the mechanism, so it catches both a derived `Default`
        // (which gives `high: 0` — a zero-width band at black, i.e. a mask that covers nothing)
        // and someone changing the numbers without meaning to.
        let r = LuminanceRange::default();
        assert_eq!((r.low, r.high, r.feather), (0.0, 50.0, 50.0));
        assert!(!r.invert);
        assert_eq!(r.coverage(20.0), 1.0, "shadows are fully covered");
        assert_eq!(r.coverage(100.0), 0.0, "white is not");
    }

    #[test]
    fn a_partial_luminance_range_json_gets_the_struct_default() {
        // ⚠ The container-level `#[serde(default)]` fills from `Default`, so a sidecar naming
        // only `low` must still get a usable band — the trap a derived `Default` would spring.
        let r: LuminanceRange = serde_json::from_str(r#"{"low":10.0}"#).unwrap();
        assert_eq!(r.low, 10.0);
        assert_eq!(r.high, 50.0);
        assert_eq!(r.feather, 50.0);
    }

    #[test]
    fn a_luminance_mask_round_trips_through_json() {
        // ⚠ `Mask` is externally tagged, so this is `{"Luminance": {...}}` and every sidecar
        // written before today still loads. Asserts the PAYLOAD, not merely the variant — the
        // lesson `an_existing_sidecar_without_a_brush_still_loads` records.
        let m = Mask::Luminance(LuminanceRange {
            low: 12.0,
            high: 34.0,
            feather: 56.0,
            invert: true,
        });
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("Luminance"), "externally tagged: {s}");
        let back: Mask = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }

    /// ⚠⚠ **Hue WRAPS and lightness does not — the one place this must not mirror
    /// [`LuminanceRange`].** sRGB red sits at 29.23° in Oklab and magenta at 328.36°, both in
    /// `BAND_HUES`, so without wrapping *the single most commonly masked colour is the one the
    /// control cannot express*.
    ///
    /// ⚠ Asserts BOTH halves: that red is covered and that green is not. A wrap bug that selects
    /// everything passes a coverage-only assertion.
    #[test]
    fn a_range_across_zero_selects_red_and_not_everything_else() {
        let r = ColourRange {
            low: 340.0,
            high: 20.0,
            feather: 0.0,
            min_chroma: 0.01,
            invert: false,
        };
        assert!(
            r.coverage_at(350.0, 0.1) > 0.9,
            "350° is inside a 340→20 band"
        );
        assert!(r.coverage_at(10.0, 0.1) > 0.9, "10° is inside it too");
        assert!(
            r.coverage_at(150.0, 0.1) < 0.1,
            "green must NOT be selected"
        );
    }

    /// ⚠⚠ **The chroma gate is load-bearing, not a refinement.** `HslStage` measured **37.8× noise
    /// amplification** on a near-neutral patch without one: a near-neutral pixel's hue is sensor
    /// noise, so neighbouring pixels land in different hues and get different treatment.
    ///
    /// ⚠ The fixture is a NOISY near-neutral, not a perfect grey — a perfect grey has an undefined
    /// hue and passes against a broken gate too.
    #[test]
    fn a_near_neutral_pixel_is_not_selected_however_well_its_hue_matches() {
        let r = ColourRange {
            low: 0.0,
            high: 60.0,
            feather: 0.0,
            min_chroma: 0.02,
            invert: false,
        };
        // Hue dead-centre in the band, chroma at sensor-noise level.
        assert!(
            r.coverage_at(30.0, 0.003) < 0.1,
            "noise-level chroma was selected"
        );
        assert!(
            r.coverage_at(30.0, 0.06) > 0.9,
            "a real colour must still be selected"
        );
    }

    /// ⚠⚠ **Added because the sabotage step showed the test above could not see the field it
    /// names.** Reading `min_chroma` as a fixed 0 leaves the `CHROMA_FADE` taper doing the work on
    /// its own, which still rejects a noise-level 0.003 (coverage 0.06, under that test's 0.1
    /// threshold) — so "ignore `min_chroma`" passed while the *control* did nothing. What
    /// discriminates is that the same pixel must land on either side of the gate depending on
    /// where the slider is put, which is the entire reason this is exposed rather than fixed.
    #[test]
    fn min_chroma_is_a_control_and_not_a_fixed_threshold() {
        let at = |min_chroma| {
            ColourRange {
                low: 0.0,
                high: 60.0,
                feather: 0.0,
                min_chroma,
                invert: false,
            }
            .coverage_at(30.0, 0.03)
        };
        assert!(
            at(0.0) > 0.9,
            "with the gate open, a muted colour is selected"
        );
        assert!(
            at(0.05) < 0.1,
            "and with it raised past that colour's chroma, the same pixel is not"
        );
    }

    /// ⚠⚠ Probe **ON** the boundary, not either side of it. `RadialGradient`'s
    /// `feather_zero_is_a_hard_edge` probed either side and never on it, and so PASSED with its
    /// guard deleted, because IEEE infinity clamped to the right answer.
    #[test]
    fn feather_zero_is_a_hard_edge_on_the_boundary_itself() {
        let r = ColourRange {
            low: 0.0,
            high: 60.0,
            feather: 0.0,
            min_chroma: 0.0,
            invert: false,
        };
        let on = r.coverage_at(60.0, 0.1);
        assert!(on.is_finite(), "coverage on the boundary was {on}");
        assert!(r.coverage_at(59.0, 0.1) > 0.9 && r.coverage_at(61.0, 0.1) < 0.1);
    }

    /// ⚠ Invert flips coverage, as every other shape does.
    #[test]
    fn invert_flips_the_band() {
        let base = ColourRange {
            low: 0.0,
            high: 60.0,
            feather: 0.0,
            min_chroma: 0.0,
            invert: false,
        };
        let inv = ColourRange {
            invert: true,
            ..base
        };
        assert!((base.coverage_at(30.0, 0.1) + inv.coverage_at(30.0, 0.1) - 1.0).abs() < 1e-6);
    }

    /// ⚠⚠ Hand-written `Default`, not derived. `LuminanceRange::default`'s comment records why: the
    /// container-level `#[serde(default)]` fills a missing field from the struct's `Default`, so a
    /// derived one gives every partial sidecar `feather: 0`, `min_chroma: 0` and a band of zero
    /// width at 0° — a mask selecting nothing, silently.
    #[test]
    fn the_default_selects_a_recognisable_band_with_a_soft_shoulder() {
        let d = ColourRange::default();
        assert!(d.high > d.low, "a zero-width band selects nothing: {d:?}");
        assert!(d.feather > 0.0);
        assert!(d.min_chroma > 0.0);
    }

    #[test]
    fn a_colour_range_has_no_analytic_coverage() {
        // ⚠⚠ The same reason a brush and a luminance range have none, and the one the plan's
        // "return `Some(..)`" line cannot mean: this function is handed a position and an aspect,
        // and a colour range selects on the pixel's hue and chroma. `Some(_)` here would be a
        // number invented from coordinates — a mask that quietly selects by position while
        // claiming to select by colour.
        let m = Mask::Colour(ColourRange::default());
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.0), None);
    }

    #[test]
    fn a_colour_mask_round_trips_through_json() {
        // ⚠ `Mask` is externally tagged, so this is `{"Colour": {...}}` and every sidecar written
        // before today still loads. Asserts the PAYLOAD, not merely the variant — the lesson
        // `an_existing_sidecar_without_a_brush_still_loads` records.
        let m = Mask::Colour(ColourRange {
            low: 340.0,
            high: 20.0,
            feather: 12.0,
            min_chroma: 0.05,
            invert: true,
        });
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("Colour"), "externally tagged: {s}");
        let back: Mask = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_partial_colour_range_json_gets_the_struct_default() {
        // ⚠ The container-level `#[serde(default)]` fills from `Default`, so a sidecar naming only
        // `low` must still get a usable band — the trap a derived `Default` would spring.
        let r: ColourRange = serde_json::from_str(r#"{"low":200.0}"#).unwrap();
        assert_eq!(r.low, 200.0);
        assert_eq!(r.high, 120.0);
        assert_eq!(r.feather, 50.0);
        assert_eq!(r.min_chroma, 0.02);
    }
}

//! The colour spaces an export can target.
//!
//! ⚠ Each space is a **pair**: a set of primaries (the matrix) and a transfer curve. Mixing one
//! space's primaries with another's curve produces a file that opens, has plausible colour, and
//! is quietly wrong — which is why they live together on one type rather than as two parallel
//! lookups a caller could combine incorrectly.

use crate::spaces::{
    apply, PROPHOTO_TO_ADOBE_RGB, PROPHOTO_TO_DISPLAY_P3, PROPHOTO_TO_SRGB, SRGB_TO_ADOBE_RGB,
    SRGB_TO_DISPLAY_P3, SRGB_TO_PROPHOTO,
};

/// Adobe RGB (1998)'s encoding gamma.
///
/// ⚠ **563/256, not 2.2.** The two differ by under 0.1%, but 2.2 is a value the specification
/// does not contain, and a profile claiming to be Adobe RGB should carry Adobe RGB's number.
const ADOBE_RGB_GAMMA: f32 = 563.0 / 256.0;

/// ROMM's linear-segment threshold, 1/512.
const PROPHOTO_TOE_END: f32 = 0.001_953_125;

/// A space an export can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum OutputSpace {
    /// The safe default: every browser, every screen, every recipient.
    #[default]
    Srgb,
    /// sRGB's curve with wider primaries. What modern phones and displays use.
    DisplayP3,
    /// The long-standing print-delivery space.
    AdobeRgb,
    /// The working space itself — the only lossless option.
    ProPhoto,
}

impl OutputSpace {
    pub const ALL: [OutputSpace; 4] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
        OutputSpace::ProPhoto,
    ];

    /// The spaces a *display* can plausibly be.
    ///
    /// ⚠ Deliberately not `ALL`: no ProPhoto monitor exists, so offering it is a pure footgun.
    /// Building a display list from `ALL` is exactly how it would creep back in.
    pub const DISPLAYS: [OutputSpace; 3] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
    ];

    /// Linear **sRGB** → this space's linear primaries.
    ///
    /// ⚠ The sibling of [`Self::from_working`], and a different starting point: that one leaves
    /// the ProPhoto working space, this one leaves already-rendered sRGB output. The display
    /// transform uses this because the preview pipeline still renders sRGB — see the spec for
    /// why that is deliberate and not an oversight.
    ///
    /// ⚠ sRGB is the identity, returned without touching a matrix.
    #[inline]
    pub fn from_srgb(self, c: [f32; 3]) -> [f32; 3] {
        match self {
            OutputSpace::Srgb => c,
            OutputSpace::DisplayP3 => apply(&SRGB_TO_DISPLAY_P3, c),
            OutputSpace::AdobeRgb => apply(&SRGB_TO_ADOBE_RGB, c),
            // Not reachable through `DISPLAYS`; defined so the match is total and a future
            // caller cannot get silence.
            OutputSpace::ProPhoto => apply(&SRGB_TO_PROPHOTO, c),
        }
    }

    /// Convert one **sRGB-encoded 8-bit** pixel into this space, encoded 8-bit.
    ///
    /// ⚠ For thumbnails only, which are 8-bit camera JPEGs with no float behind them. Anywhere
    /// a float is available, use [`Self::from_srgb`] before quantizing instead: an 8-bit → 8-bit
    /// conversion compresses the range and costs roughly 0.8 bits, which shows as banding on a
    /// smooth gradient.
    #[inline]
    pub fn convert_srgb8(self, c: [u8; 3]) -> [u8; 3] {
        if self == OutputSpace::Srgb {
            return c;
        }
        let lin = c.map(|v| crate::srgb_to_linear(v as f32 / 255.0));
        let out = self.from_srgb(lin);
        [
            (self.encode(out[0]) * 255.0).round() as u8,
            (self.encode(out[1]) * 255.0).round() as u8,
            (self.encode(out[2]) * 255.0).round() as u8,
        ]
    }

    /// Linear ProPhoto working space → this space's linear primaries.
    ///
    /// ⚠ **ProPhoto is the identity**, returned without touching a matrix. It *is* the working
    /// space, so exporting it converts nothing and clips nothing.
    ///
    /// ⚠ Produces negative components for colour outside the target gamut. That is correct here;
    /// [`Self::encode`] is where clipping belongs.
    #[inline]
    pub fn from_working(self, c: [f32; 3]) -> [f32; 3] {
        match self {
            OutputSpace::Srgb => apply(&PROPHOTO_TO_SRGB, c),
            OutputSpace::DisplayP3 => apply(&PROPHOTO_TO_DISPLAY_P3, c),
            OutputSpace::AdobeRgb => apply(&PROPHOTO_TO_ADOBE_RGB, c),
            OutputSpace::ProPhoto => c,
        }
    }

    /// This space's transfer curve. Input is clamped to `[0, 1]`.
    ///
    /// ⚠ Display P3 shares sRGB's curve — only the primaries differ. Using gamma 2.2 there is a
    /// common error and an invisible one.
    #[inline]
    pub fn encode(self, v: f32) -> f32 {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => crate::linear_to_srgb(v),
            OutputSpace::AdobeRgb => v.clamp(0.0, 1.0).powf(1.0 / ADOBE_RGB_GAMMA),
            OutputSpace::ProPhoto => {
                let v = v.clamp(0.0, 1.0);
                if v < PROPHOTO_TOE_END {
                    v * 16.0
                } else {
                    v.powf(1.0 / 1.8)
                }
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::AdobeRgb => "Adobe RGB",
            OutputSpace::ProPhoto => "ProPhoto",
        }
    }

    /// Whether 8-bit output in this space is worth warning about.
    ///
    /// ⚠⚠ **True for ProPhoto only, and that narrowness is deliberate — do not "complete" this
    /// table.** Banding does scale with gamut volume, but 8-bit Display P3 is what every iPhone
    /// ships by default and 8-bit Adobe RGB has been the standard print-delivery format for
    /// twenty years. A warning that fires on ordinary practice is a false alarm, and false
    /// alarms train people to dismiss the warning that matters. 8-bit ProPhoto is the case
    /// Adobe itself warns against.
    pub fn warns_in_8_bit(self) -> bool {
        matches!(self, OutputSpace::ProPhoto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_space_maps_white_to_white() {
        // ⚠ The check that each matrix carries its Bradford adaptation the right way round.
        // Without it, D50 white lands off-white — a warm cast on every image, subtle enough to
        // ship. ProPhoto's own D50 white point makes this a real check, not a formality.
        for s in OutputSpace::ALL {
            let w = s.from_working([1.0, 1.0, 1.0]);
            for (i, v) in w.iter().enumerate() {
                assert!(
                    (v - 1.0).abs() < 1e-3,
                    "{}: white channel {i} became {v}",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn prophoto_is_bit_exactly_the_identity() {
        // ⚠ Not "close to" the identity. ProPhoto *is* the working space, so exporting it must
        // apply no matrix at all — it is the one lossless option. Routing it through a general
        // matrix path would leave it a ulp off and this assertion is what forbids that.
        for c in [[0.18, 0.5, 0.9], [0.0, 1.0, 0.0], [-0.2, 1.4, 0.03]] {
            assert_eq!(OutputSpace::ProPhoto.from_working(c), c);
        }
    }

    #[test]
    fn display_p3_holds_a_green_srgb_clips() {
        // ⚠ The capability the whole feature exists for, as a test. P3's own green primary is
        // outside sRGB, so converting it to sRGB yields negative components while converting it
        // to P3 gives back exactly the primary. If this fails the feature does nothing, however
        // well the UI is wired.
        let p3_green_in_working = [0.2139, 0.8859, 0.0508];
        let in_srgb = OutputSpace::Srgb.from_working(p3_green_in_working);
        assert!(
            in_srgb.iter().any(|v| *v < -0.05),
            "P3 green should fall outside sRGB, got {in_srgb:?}"
        );
        let in_p3 = OutputSpace::DisplayP3.from_working(p3_green_in_working);
        assert!(
            in_p3[0].abs() < 0.02 && (in_p3[1] - 1.0).abs() < 0.02 && in_p3[2].abs() < 0.02,
            "P3 green should be in gamut in P3, got {in_p3:?}"
        );
    }

    #[test]
    fn display_p3_uses_the_srgb_transfer_curve() {
        // ⚠ The error this exists to prevent is invisible: using gamma 2.2 for P3 gives an image
        // that is off by a few percent in the shadows, which reads as a contrast preference
        // rather than as a bug. Only the primaries differ from sRGB.
        for v in [0.0, 0.002, 0.18, 0.5, 1.0] {
            assert_eq!(
                OutputSpace::DisplayP3.encode(v),
                OutputSpace::Srgb.encode(v),
                "P3 and sRGB must share a curve, differed at {v}"
            );
        }
    }

    #[test]
    fn the_curves_match_their_specifications_at_mid_grey() {
        // Known values, computed from each specification rather than from a second copy of our
        // own implementation — re-deriving in the test proves only that the same mistake was
        // made twice.
        let want = [
            (OutputSpace::Srgb, 0.461356),
            (OutputSpace::DisplayP3, 0.461356),
            (OutputSpace::AdobeRgb, 0.458529),
            (OutputSpace::ProPhoto, 0.385711),
        ];
        for (s, w) in want {
            let got = s.encode(0.18);
            assert!(
                (got - w).abs() < 1e-4,
                "{}: encode(0.18) = {got}, want {w}",
                s.label()
            );
        }
    }

    #[test]
    fn the_prophoto_toe_meets_its_power_segment() {
        // ROMM's linear toe below 1/512 and its 1.8 power segment are continuous at the join:
        // both give exactly 0.03125. A discontinuity there would be a visible step in deep
        // shadow, and a wrong threshold or slope produces exactly that.
        //
        // ⚠⚠ This **sweeps for a jump anywhere** rather than probing at 1/512, and the
        // difference is not stylistic. A version that sampled either side of the *specified*
        // threshold passed with `PROPHOTO_TOE_END` set to 0.01 — both probes then landed inside
        // the toe branch, where the bug is inert, and the discontinuity sat untested at 0.01.
        // Caught by the sabotage step. A test that names the threshold cannot find a threshold
        // that moved.
        let mut prev = OutputSpace::ProPhoto.encode(0.0);
        let mut v = 0.0f32;
        while v <= 0.05 {
            let cur = OutputSpace::ProPhoto.encode(v);
            assert!(
                (cur - prev).abs() < 1e-3,
                "the curve jumps from {prev} to {cur} at {v}"
            );
            prev = cur;
            v += 1e-5;
        }
        // And the join is where the specification puts it, at the value it specifies.
        let at = OutputSpace::ProPhoto.encode(1.0 / 512.0);
        assert!(
            (at - 0.03125).abs() < 1e-4,
            "the join should sit at 0.03125, got {at}"
        );
    }

    #[test]
    fn every_curve_clamps_to_the_encodable_range() {
        // ⚠ The upper bound is a tolerance and the lower one is exact, which is not an
        // inconsistency: `clamp(0.0, 1.0)` then a power gives exactly 0.0 below, but sRGB's
        // `1.055 * 1.0 - 0.055` evaluates to 0.99999994 in f32. An `assert_eq!` against 1.0
        // there asserts something false about correct code — the arithmetic is fine and the
        // clamp is doing its job.
        for s in OutputSpace::ALL {
            assert_eq!(s.encode(-0.5), 0.0, "{} let a negative through", s.label());
            let top = s.encode(2.0);
            assert!(
                top <= 1.0 && (top - 1.0).abs() < 1e-6,
                "{} let a value above 1 through: {top}",
                s.label()
            );
        }
    }

    #[test]
    fn only_prophoto_warns_in_eight_bit() {
        // ⚠ Deliberately narrow. 8-bit Display P3 is what every iPhone ships and 8-bit Adobe RGB
        // is the standard print-delivery format; warning on those is a false alarm, and false
        // alarms train people to dismiss the warning that matters.
        for s in OutputSpace::ALL {
            assert_eq!(
                s.warns_in_8_bit(),
                s == OutputSpace::ProPhoto,
                "{} warned unexpectedly",
                s.label()
            );
        }
    }

    #[test]
    fn every_space_has_a_distinct_label() {
        let mut seen = Vec::new();
        for s in OutputSpace::ALL {
            assert!(!seen.contains(&s.label()), "duplicate label {}", s.label());
            seen.push(s.label());
        }
    }

    #[test]
    fn srgb_is_bit_exactly_the_identity_for_the_display_transform() {
        // ⚠ The default must cost nothing and change nothing. `assert_eq!`, not a tolerance:
        // routing sRGB through a general matrix path would leave it a ulp off, and a preview
        // that shifts by a ulp when nobody asked is a regression nobody would find.
        for c in [[0.18, 0.5, 0.9], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]] {
            assert_eq!(OutputSpace::Srgb.from_srgb(c), c);
        }
    }

    #[test]
    fn the_display_transform_maps_white_to_white() {
        for s in OutputSpace::DISPLAYS {
            let w = s.from_srgb([1.0, 1.0, 1.0]);
            for (i, v) in w.iter().enumerate() {
                assert!(
                    (v - 1.0).abs() < 1e-4,
                    "{}: white channel {i} became {v}",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn srgb_fits_inside_every_display_space() {
        // ⚠ sRGB's gamut is strictly inside both destinations, so no in-range colour can leave
        // [0, 1]. If this ever fails a matrix is wrong — it is not a case to start clamping.
        for s in OutputSpace::DISPLAYS {
            for c in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.2, 0.7, 0.4],
            ] {
                let out = s.from_srgb(c);
                for (i, v) in out.iter().enumerate() {
                    assert!(
                        (-1e-6..=1.0 + 1e-6).contains(v),
                        "{}: {c:?} channel {i} left the range: {v}",
                        s.label()
                    );
                }
            }
        }
    }

    #[test]
    fn a_wider_container_needs_smaller_coordinates() {
        // ⚠ The direction test, and the reason it is not merely "the value changed": an
        // *inverted* matrix would also change the value, and would oversaturate exactly as the
        // uncorrected preview does — i.e. it would look like no fix at all while passing.
        // Red is the channel where sRGB and P3 differ most.
        let red = OutputSpace::DisplayP3.from_srgb([1.0, 0.0, 0.0]);
        assert!(
            red[0] < 0.9,
            "sRGB red should need less than full red in P3, got {red:?}"
        );
        assert!(
            red[1] > 0.0 && red[2] > 0.0,
            "sRGB red is less saturated than P3 red, so it needs some green and blue: {red:?}"
        );
    }

    #[test]
    fn each_primary_stays_dominated_by_its_own_channel() {
        // ⚠⚠ This exists because **a row swap survived every other test here.** Both of
        // `SRGB_TO_DISPLAY_P3`'s first two rows sum to 1.0, so swapping them still maps white to
        // white; the magnitudes stay in range, and "red needs less than full red" is still true
        // of the wrong row. The same trap `grey_has_no_chroma_in_prophoto` hit for Oklab —
        // neutrals are preserved by any permutation, so they can never detect one.
        //
        // Converting cannot change which primary a colour *is*, so each converted primary must
        // still be dominated by its own channel. That catches any row or column permutation.
        for s in OutputSpace::DISPLAYS {
            for (ch, c) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .into_iter()
                .enumerate()
            {
                let out = s.from_srgb(c);
                let biggest = (0..3).max_by(|a, b| out[*a].total_cmp(&out[*b])).unwrap();
                assert_eq!(
                    biggest,
                    ch,
                    "{}: sRGB primary {ch} became {out:?}, dominated by channel {biggest}",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn the_display_transform_preserves_luminance() {
        // ⚠⚠ The strongest test here, and it exists because weaker ones let two different
        // matrix permutations through. A row swap survived white-maps-to-white (both rows sum
        // to 1.0, so any permutation preserves neutrals) and a *within-row* swap survived even
        // the per-channel dominance check.
        //
        // Luminance is physical: changing which primaries a colour is expressed in cannot
        // change how bright it is. So Y computed with each space's own coefficients must equal
        // Y computed with Rec.709 on the sRGB original. That is an independent quantity, not a
        // re-derivation of the matrix, and essentially any error in it moves Y.
        //
        // ⚠ It also pins `SRGB_TO_ADOBE_RGB`'s middle row, which is literally `[0, 1, 0]` and
        // reads as a copy-paste error: sRGB green lands at (0.2849, 1.0, 0.0412), whose Adobe
        // luminance comes back to sRGB green's 0.7152.
        const Y_SRGB: [f32; 3] = [0.212639, 0.7151687, 0.0721923];
        const Y_P3: [f32; 3] = [0.2289746, 0.6917385, 0.0792869];
        const Y_ADOBE: [f32; 3] = [0.297345, 0.6273636, 0.0752915];
        let dot = |w: [f32; 3], c: [f32; 3]| w[0] * c[0] + w[1] * c[1] + w[2] * c[2];

        for (s, y) in [
            (OutputSpace::DisplayP3, Y_P3),
            (OutputSpace::AdobeRgb, Y_ADOBE),
        ] {
            for c in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.35, 0.55, 0.2],
                [0.9, 0.1, 0.4],
            ] {
                let want = dot(Y_SRGB, c);
                let got = dot(y, s.from_srgb(c));
                assert!(
                    (got - want).abs() < 1e-3,
                    "{}: {c:?} has luminance {want} in sRGB but {got} after conversion",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn prophoto_is_not_offered_as_a_display() {
        // ⚠ No such monitor exists. Offering it is a pure footgun, and building the list from
        // `ALL` is exactly how it would creep back in.
        assert!(!OutputSpace::DISPLAYS.contains(&OutputSpace::ProPhoto));
        for s in [
            OutputSpace::Srgb,
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
        ] {
            assert!(
                OutputSpace::DISPLAYS.contains(&s),
                "{} should be offered as a display",
                s.label()
            );
        }
    }

    #[test]
    fn the_eight_bit_helper_agrees_with_the_float_one() {
        // ⚠ Two implementations of one transform, so they must be pinned to each other. The
        // 8-bit path exists only for thumbnails, which have no float behind them; anywhere a
        // float is available the float path is the correct one.
        for s in OutputSpace::DISPLAYS {
            for v in [0u8, 1, 64, 128, 200, 255] {
                let src = [v, 255 - v, 128];
                let eight = s.convert_srgb8(src);
                let float = {
                    let lin = src.map(|c| crate::srgb_to_linear(c as f32 / 255.0));
                    let out = s.from_srgb(lin);
                    out.map(|c| (s.encode(c) * 255.0).round() as u8)
                };
                for i in 0..3 {
                    assert!(
                        eight[i].abs_diff(float[i]) <= 1,
                        "{}: {src:?} channel {i}: 8-bit {} vs float {}",
                        s.label(),
                        eight[i],
                        float[i]
                    );
                }
            }
        }
    }
}

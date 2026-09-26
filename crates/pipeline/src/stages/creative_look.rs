//! A creative look: a 3-D LUT laid over the finished photograph.
//!
//! ⚠⚠ **The declared [`LutSpace`] decides WHERE this stage runs, not merely how it shapes.** A
//! display-referred LUT *modifies* the camera rendering; a linear one runs **in place of** it,
//! because it **is** the rendering. That placement lives in `lib.rs`; this file owns only the
//! shaping, the lookup and the strength blend.

use crate::lut::Cube;
use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::LutSpace;
use std::sync::Arc;

/// Encode a **linear sRGB / Rec.709** sample into `space`'s own transfer.
///
/// ⚠⚠ **Rec.709's transfer is not sRGB's**, though the two are routinely confused: BT.709 is a
/// 0.45 power with a 4.5 slope below 0.018, sRGB a 1/2.4 power with a 12.92 slope below
/// 0.0031308. Both are **published constants, not fitted ones** — nothing here is tuned, and
/// nothing here may be. `rec709_and_srgb_are_not_the_same_curve` is the guard against aliasing.
///
/// ⚠ These live here rather than in `color` deliberately, and it is a wave-scoping compromise
/// rather than a placement judgement: `color::output_space` is another track's surface this wave.
/// **They belong in `color` beside `linear_to_srgb`** once the wave merges — a second caller
/// (soft proofing, a Rec.709 output space) will want them.
fn encode(v: f32, space: LutSpace) -> f32 {
    match space {
        LutSpace::SrgbDisplay => color::linear_to_srgb(v),
        LutSpace::Rec709 => {
            let v = v.clamp(0.0, 1.0);
            if v < 0.018 {
                4.5 * v
            } else {
                1.099 * v.powf(0.45) - 0.099
            }
        }
        // ⚠ Unreachable in the stage — `Linear` takes the no-shaper branch — but total anyway, so
        // a future caller cannot get a silent identity where it expected a curve.
        LutSpace::Linear => v,
    }
}

/// The inverse of [`encode`]. `each_transfer_inverts_its_own_encode` is the guard that the two
/// are a matched pair rather than two independently plausible curves.
fn decode(v: f32, space: LutSpace) -> f32 {
    match space {
        LutSpace::SrgbDisplay => color::srgb_to_linear(v),
        LutSpace::Rec709 => {
            let v = v.clamp(0.0, 1.0);
            if v < 0.081 {
                v / 4.5
            } else {
                ((v + 0.099) / 1.099).powf(1.0 / 0.45)
            }
        }
        LutSpace::Linear => v,
    }
}

/// A 3-D LUT applied as a creative look.
pub struct CreativeLookStage {
    cube: Arc<Cube>,
    space: LutSpace,
    /// `strength / 100`, precomputed. 1.0 is the LUT exactly as authored.
    blend: f32,
}

impl CreativeLookStage {
    pub fn new(cube: Arc<Cube>, space: LutSpace, strength: f32) -> Self {
        Self {
            cube,
            space,
            blend: (strength / 100.0).clamp(0.0, 1.0),
        }
    }

    /// One pixel: shape in, look up, shape back, blend.
    fn look(&self, px: [f32; 3]) -> [f32; 3] {
        // ⚠⚠ **`LutSpace::Linear` takes NO shaper**, and that single branch is what makes a linear
        // LUT a *rendering* rather than a grade: the data reaching this stage is already
        // scene-linear in the working space, so a LUT authored against linear input wants it
        // untouched. Shaping it here would be applying a transfer the LUT then applies again.
        if self.space.is_the_rendering() {
            return self.cube.lookup(px);
        }
        // ⚠ The primaries matrix as well as the transfer. A `.cube` authored for sRGB expects
        // sRGB's primaries; feeding it our ProPhoto values shaped only by the transfer is the
        // "converts into the wrong space" failure, and it is *nearly* right — which is exactly
        // why `an_identity_lut_through_the_srgb_shaper_returns_the_input` measures the margin
        // against it rather than trusting a tolerance.
        //
        // ⚠ Rec.709 shares sRGB's primaries and differs only in transfer, so one matrix serves
        // both. That is not an approximation; it is the definition.
        let lin = color::spaces::prophoto_to_srgb(px);
        let shaped = [
            encode(lin[0], self.space),
            encode(lin[1], self.space),
            encode(lin[2], self.space),
        ];
        let out = self.cube.lookup(shaped);
        let back = [
            decode(out[0], self.space),
            decode(out[1], self.space),
            decode(out[2], self.space),
        ];
        color::spaces::srgb_to_prophoto(back)
    }
}

impl crate::per_pixel::PerPixel for CreativeLookStage {
    fn px(&self, _x: usize, _y: usize, px: [f32; 3]) -> [f32; 3] {
        let t = self.blend;
        // ⚠⚠ **Short-circuit at 0, and it is a correctness guard rather than a speed one.**
        // The shaper clamps out-of-gamut values, so running it at strength 0 and then
        // lerping back would leave a residue on saturated colours — a look dialled to zero
        // that is not quite no look. `strength_zero_is_bit_exactly_the_input` is the guard.
        if t == 0.0 {
            return px;
        }
        let out = self.look(px);
        // ⚠ `a + (b - a) * t`, so `t = 1` returns `b` exactly and `t = 0` returns `a`
        // exactly. `a*(1-t) + b*t` does neither in f32.
        [
            px[0] + (out[0] - px[0]) * t,
            px[1] + (out[1] - px[1]) * t,
            px[2] + (out[2] - px[2]) * t,
        ]
    }
}

impl PipelineStage for CreativeLookStage {
    fn name(&self) -> &'static str {
        "creative_look"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use std::sync::Arc;

    /// An identity cube of size `n`, as text.
    fn identity(n: usize) -> Arc<Cube> {
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        Arc::new(Cube::parse(&s).expect("identity parses"))
    }

    /// A cube that is emphatically **not** the identity: it inverts every channel. Nothing about
    /// it is symmetric under a transposed index or a skipped shaper.
    fn inverting(n: usize) -> Arc<Cube> {
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| 1.0 - i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        Arc::new(Cube::parse(&s).expect("inverting parses"))
    }

    fn one_pixel(rgb: [f32; 3]) -> ImageBuf {
        ImageBuf::from_vec(1, 1, rgb.to_vec())
    }

    /// ⚠⚠ **Strength 0 is BIT-EXACTLY the input.** `assert_eq!`: a lerp that is nearly the
    /// identity is exactly what a tolerance hides, and a look dialled to zero must be
    /// indistinguishable from no look at all — including through the shaper, which at strength 0
    /// must not be allowed to leave a residue.
    #[test]
    fn strength_zero_is_bit_exactly_the_input() {
        let px = [0.2, 0.45, 0.8];
        for space in LutSpace::ALL {
            let s = CreativeLookStage::new(inverting(9), space, 0.0);
            let got = s.apply(one_pixel(px)).pixel(0, 0);
            assert_eq!(got, px, "strength 0 moved the pixel in {:?}", space);
        }
    }

    /// ⚠ And strength 100 is bit-exactly the LUT's own output, so the ends of the slider mean what
    /// they say. Asserted in `Linear`, the one space where "the LUT's own output" is a value this
    /// test can name without going through a shaper.
    #[test]
    fn strength_one_hundred_is_bit_exactly_the_lut() {
        let cube = inverting(9);
        let px = [0.2, 0.45, 0.8];
        let s = CreativeLookStage::new(cube.clone(), LutSpace::Linear, 100.0);
        let got = s.apply(one_pixel(px)).pixel(0, 0);
        assert_eq!(got, cube.lookup(px));
    }

    /// ⚠⚠ **The only test that can see the 0..100 scaling at all, and it was MISSING until a
    /// sabotage proved it.** Replacing `strength / 100.0` with `strength` passed every other test
    /// here: `clamp(0.0, 1.0)` sends 0 to 0 and 100 to 1 under both, so the two endpoint guards
    /// are blind to the scale by construction. The plan predicted
    /// `strength_zero_is_bit_exactly_the_input` would catch it; it cannot. Only an intermediate
    /// strength can.
    #[test]
    fn strength_fifty_is_the_midpoint_between_the_input_and_the_lut() {
        let cube = inverting(9);
        let px = [0.2, 0.45, 0.8];
        let full = cube.lookup(px);
        let s = CreativeLookStage::new(cube, LutSpace::Linear, 50.0);
        let got = s.apply(one_pixel(px)).pixel(0, 0);
        for c in 0..3 {
            let want = px[c] + (full[c] - px[c]) * 0.5;
            assert!(
                (got[c] - want).abs() < 1e-6,
                "half strength is not half way: got {got:?}, input {px:?}, full {full:?}"
            );
            // ⚠ And it must actually be *between* the two, not merely near one of them — a
            // blend stuck at 1.0 is the failure this catches.
            assert!(
                (got[c] - full[c]).abs() > 1e-3,
                "half strength landed on the LUT's own output: {got:?} vs {full:?}"
            );
        }
    }

    /// ⚠⚠ **A linear LUT needs NO shaper**, so an identity one is bit-exactly the identity. That
    /// is the branch which makes a linear LUT a *rendering* rather than a grade, and this is the
    /// only exact check available — see `an_identity_lut_through_the_srgb_shaper_returns_the_input`
    /// for why the display spaces cannot have one.
    #[test]
    fn an_identity_lut_in_linear_is_bit_exactly_the_identity() {
        let s = CreativeLookStage::new(identity(17), LutSpace::Linear, 100.0);
        for px in [
            [0.0, 0.0, 0.0],
            [0.25, 0.5, 0.75],
            [1.0, 1.0, 1.0],
            [0.18; 3],
        ] {
            assert_eq!(s.apply(one_pixel(px)).pixel(0, 0), px, "moved {px:?}");
        }
    }

    /// ⚠⚠ **NOT bit-exact, and the reason is arithmetic rather than a bug** — recorded here
    /// because the plan expected `assert_eq!` and it is unattainable. The display shaper is a
    /// 3×3 primaries matrix and a power-law transfer, each way; in f32 neither round-trips
    /// exactly (measured: the matrix alone is bit-exact for 1364/3000 probes, the sRGB transfer
    /// alone for 4153/10001, worst error 2.4e-7 in both).
    ///
    /// ⚠ **What makes the tolerance discriminating rather than permissive is the second
    /// assertion**, not the first: a shaper that forgets the primaries matrix — the exact
    /// "converts into the wrong space and back" failure this test exists for — misses by more
    /// than 1e-2, five orders of magnitude outside the bound. A tolerance alone would prove
    /// nothing; a tolerance with a measured margin against the wrong answer does.
    #[test]
    fn an_identity_lut_through_the_srgb_shaper_returns_the_input() {
        // ⚠ Neutral and near-neutral only. A saturated ProPhoto value is outside sRGB's gamut,
        // so the shaper's transfer clamps it — see `a_display_look_clips_to_the_luts_gamut`,
        // which asserts that deliberately.
        let probes = [[0.18; 3], [0.5; 3], [0.05; 3], [0.4, 0.42, 0.38]];
        for space in [LutSpace::SrgbDisplay, LutSpace::Rec709] {
            let s = CreativeLookStage::new(identity(33), space, 100.0);
            for px in probes {
                let got = s.apply(one_pixel(px)).pixel(0, 0);
                for c in 0..3 {
                    assert!(
                        (got[c] - px[c]).abs() < 1e-5,
                        "{space:?} shaper moved {px:?} to {got:?}"
                    );
                }
            }
        }

        // The margin. Encoding without the primaries matrix — treating a ProPhoto value as
        // though it were already sRGB — is the wrong-space bug, and it is nowhere near 1e-5.
        //
        // ⚠⚠ **The probe must be CHROMATIC.** A first draft used neutral grey and the margin
        // collapsed to 2e-8: ProPhoto and sRGB are both normalised so a neutral maps to a
        // neutral, which means grey cannot see a missing primaries matrix at all. That is the
        // fixture failing to discriminate, not the guard being redundant — the failure mode this
        // project has recorded three times.
        let px = [0.5f32, 0.2, 0.1];
        let right = color::spaces::prophoto_to_srgb(px);
        let worst = (0..3)
            .map(|c| (px[c] - right[c]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst > 1e-2,
            "the wrong-space margin has collapsed, so the tolerance above proves nothing: \
             {px:?} vs {right:?}"
        );
    }

    /// ⚠⚠ **A display-referred look clips to the LUT's own gamut, and that is a real cost the
    /// user has to sign off.** A ProPhoto value outside sRGB goes negative under the matrix, and
    /// the transfer clamps it — so the look does not merely grade a saturated colour, it
    /// desaturates it. Defensible only because a look IS a display-referred decision (spec §4);
    /// it would not be defensible anywhere upstream.
    #[test]
    fn a_display_look_clips_to_the_luts_gamut() {
        // Saturated ProPhoto green: well outside sRGB.
        let px = [0.0, 0.9, 0.0];
        assert!(
            color::spaces::prophoto_to_srgb(px)[0] < -0.05,
            "the fixture is not actually out of sRGB gamut"
        );
        let s = CreativeLookStage::new(identity(33), LutSpace::SrgbDisplay, 100.0);
        let got = s.apply(one_pixel(px)).pixel(0, 0);
        // ⚠ Measured over ALL THREE channels. The clip shows up mostly in the channels that went
        // negative and came back at zero — green itself barely moves (0.900 → 0.873), so
        // watching green alone would report "nothing clipped" while the colour had in fact been
        // desaturated from [0, 0.9, 0] to [0.33, 0.87, 0.12].
        let worst = (0..3)
            .map(|c| (got[c] - px[c]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst > 0.05,
            "an out-of-gamut colour came back unchanged, so nothing clipped: {got:?}"
        );
    }

    /// ⚠ Rec.709's transfer is **not** sRGB's, and the two must stay distinguishable — a LUT
    /// author chose one. If they were aliased this test would fail.
    #[test]
    fn rec709_and_srgb_are_not_the_same_curve() {
        let mut worst = 0.0f32;
        for i in 0..=1000 {
            let v = i as f32 / 1000.0;
            worst =
                worst.max((encode(v, LutSpace::Rec709) - encode(v, LutSpace::SrgbDisplay)).abs());
        }
        assert!(worst > 0.01, "Rec.709 has been aliased to sRGB: {worst}");
    }

    /// ⚠ Both display transfers round-trip their own inverse, which is what makes the shaper's
    /// two halves a matched pair rather than two independently plausible curves.
    #[test]
    fn each_transfer_inverts_its_own_encode() {
        for space in [LutSpace::SrgbDisplay, LutSpace::Rec709] {
            for i in 0..=1000 {
                let v = i as f32 / 1000.0;
                let back = decode(encode(v, space), space);
                assert!((back - v).abs() < 1e-5, "{space:?} at {v} came back {back}");
            }
        }
    }
}

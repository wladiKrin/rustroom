use crate::PipelineStage;
use core_image::ImageBuf;

/// Scales Highlights and Shadows — the broad pair.
const STRENGTH: f32 = 0.5;
/// Scales Whites and Blacks — the end pair.
///
/// ⚠⚠ **Its own constant, because the end pair is narrower and must still have authority.**
/// `^3` concentrates a slider's weight toward its end, which also *lowers* it everywhere — and at
/// the shared 0.5 that made Blacks weaker in real shadows than the hard cutoff it replaced, so
/// Auto Tone could no longer pull a black point down to its target. 0.7 restores that reach.
///
/// ⚠ Deliberately **below** the point where the end pair would overtake the broad pair in the
/// midtones: at mid-grey Blacks contributes 0.106 against Shadows' 0.142, so the two stay
/// distinguishable, while Whites overtakes Highlights above linear ~0.46 — which is exactly what
/// a white point *should* do. `whites_are_more_end_weighted_than_highlights` pins it.
const END_STRENGTH: f32 = 0.7;

/// Highlights/shadows/whites/blacks as a single luminance-region-weighted gain.
/// All amounts ~[-100, 100]; positive brightens that region. Hue-preserving.
pub struct ToneRegionsStage {
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
}

impl ToneRegionsStage {
    pub fn new(highlights: f32, shadows: f32, whites: f32, blacks: f32) -> Self {
        Self {
            highlights: highlights / 100.0,
            shadows: shadows / 100.0,
            whites: whites / 100.0,
            blacks: blacks / 100.0,
        }
    }

    /// The four region weights at luminance `l`.
    ///
    /// ⚠⚠ **The weights are shaped on the DISPLAY-encoded luminance, not the linear one, and
    /// that correction is the whole of this function's history.** The curves below read as
    /// perceptual thirds — blacks under a third, whites over two thirds — but `l` arrives as
    /// *linear* luminance, where 2/3 is sRGB **0.85** and 1/3 is sRGB **0.60**. So Whites saw
    /// only the very top sliver of the range while Blacks covered more than half of it, which is
    /// neither what the names say nor what the sliders were tuned against.
    ///
    /// Concretely, the bug this fixes: in a scene-referred buffer the 99.9th percentile of an
    /// ordinary frame lands near linear 0.1–0.3 even *after* an auto exposure, which is below
    /// 2/3 — so **Whites had no leverage at all on most photographs** and parked at 0 or a
    /// boundary. Encoding first puts both sliders over the range photographs actually occupy.
    ///
    /// ⚠ `linear_to_srgb` is the pipeline's own output transfer curve, so "two thirds" here means
    /// two thirds of what you will *see* — the frame the sliders are judged in.
    pub fn gain_for_luma(&self, l: f32) -> f32 {
        let e = color::linear_to_srgb(l.clamp(0.0, 1.0));
        let shadow_w = (1.0 - e) * (1.0 - e);
        let highlight_w = e * e;
        // ⚠⚠ **A fourth power, not a hard cutoff, and this is the second half of the fix.**
        // `max(0, 3e − 2)` is exactly zero below two thirds, so on any photograph whose brightest
        // content does not reach there the Whites slider did *nothing* — it could not even be
        // solved for, and Auto Tone parked it at 0 or at a boundary. A power curve is never zero,
        // so the slider always has leverage, while `^3` against the broad pair's `^2` keeps it
        // twice as end-weighted at mid-grey and so still clearly a *white point* control rather
        // than a second Highlights.
        //
        // ⚠ A cube rather than a fourth power, and the difference was measured rather than
        // guessed: `^4` is so end-weighted that **Blacks came out weaker in real shadows than the
        // hard cutoff it replaced** (0.32 against 0.85 at linear 0.05), which traded one defect
        // for another. The cube keeps 0.43 there and still falls to 0.008 by linear 0.6.
        let black_w = (1.0 - e).powi(3);
        let white_w = e.powi(3);
        1.0 + STRENGTH * (self.shadows * shadow_w + self.highlights * highlight_w)
            + END_STRENGTH * (self.blacks * black_w + self.whites * white_w)
    }

    /// The largest luminance the gain table resolves.
    ///
    /// ⚠⚠ **Exactly 1.0, and that is a fact about the model rather than a bound chosen to be
    /// safe.** [`Self::gain_for_luma`] clamps its input to `[0, 1]` before encoding, so the gain
    /// above linear 1.0 *is* the gain at 1.0 — a scene-referred highlight takes the last entry and
    /// gets the right answer, not an approximation of it.
    pub const LUT_TOP: f32 = 1.0;

    /// ⚠ **Swept, not chosen**, by `the_gain_table_resolves_the_curve` against a bound of a tenth
    /// of an 8-bit level on the gain, with all four sliders hard over:
    ///
    /// | entries | worst drift | margin |
    /// |---|---|---|
    /// | 1024 | 0.000280 | 1.4x |
    /// | **2048** | **0.000076** | **5.1x** |
    /// | 4096 | 0.000018 | 21x |
    ///
    /// ⚠⚠ 1024 *passes*, and is not taken: a 1.4x margin is a trap for whoever next touches the
    /// weights, and 8 KB is not a number worth defending. The worst case sits at luminance
    /// **0.0034** in every row — the table is linear in luminance while the weights are shaped on
    /// the **display encoding** of it, so its resolution is worst exactly where
    /// `linear_to_srgb`'s 12.92x toe is steepest. The same reason `DisplayRenderingStage` needed
    /// 4096 rather than 1024.
    pub const LUT_SIZE: usize = 2048;

    /// The gain at every table entry, for a GPU stage to upload.
    ///
    /// ⚠⚠ Built **here**, by the stage that owns [`Self::gain_for_luma`], so there is still
    /// exactly one implementation of the weight shaping — including the display encode, which is
    /// the fix for "the Whites slider did nothing on most photographs". A shader rebuilding it in
    /// WGSL is how the two would come to disagree about something nobody re-derives, and **a wrong
    /// gain renders a plausible photograph rather than an error**.
    pub fn gain_lut(&self) -> Vec<f32> {
        (0..Self::LUT_SIZE)
            .map(|i| self.gain_for_luma(i as f32 / (Self::LUT_SIZE - 1) as f32 * Self::LUT_TOP))
            .collect()
    }
}

impl crate::per_pixel::PerPixel for ToneRegionsStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let l = color::luminance([r, g, b]);
        let gain = self.gain_for_luma(l).max(0.0);
        [r * gain, g * gain, b * gain]
    }
}

impl PipelineStage for ToneRegionsStage {
    fn name(&self) -> &'static str {
        "tone_regions"
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

    /// ⚠⚠ **This table does not exist on the CPU, so building it introduces an error the CPU stage
    /// does not have** — unlike `tone_curve` and `display_rendering`, which are already tables.
    /// The size is therefore measured rather than chosen, the recipe `DisplayRenderingStage`'s own
    /// 4096 came from.
    ///
    /// ⚠ The bound is a tenth of an 8-bit level **on the gain**, which is what the pixel sees.
    /// ⚠⚠ If this fails, raise `LUT_SIZE`. Never loosen the bound — that is the "assertion
    /// adjusted to fit the code" trap this repo records repeatedly.
    #[test]
    fn the_gain_table_resolves_the_curve() {
        // ⚠ All four sliders hard over, so no region is inert and the curve is as steep as the
        // model allows. A gentle fixture would pass at any table size.
        let s = ToneRegionsStage::new(100.0, 100.0, 100.0, -100.0);
        let lut = s.gain_lut();
        let n = lut.len();
        let mut worst: f32 = 0.0;
        let mut at = 0.0;
        for i in 0..=40_000 {
            let l = i as f32 / 40_000.0 * ToneRegionsStage::LUT_TOP;
            let t = (l / ToneRegionsStage::LUT_TOP).clamp(0.0, 1.0) * (n - 1) as f32;
            let j = (t.floor() as usize).min(n - 1);
            let k = (j + 1).min(n - 1);
            let from_table = lut[j] + (t - j as f32) * (lut[k] - lut[j]);
            let d = (from_table - s.gain_for_luma(l)).abs();
            if d > worst {
                worst = d;
                at = l;
            }
        }
        assert!(
            worst < 0.1 / 255.0,
            "the table drifts {worst} from the curve at luminance {at}; raise LUT_SIZE"
        );
    }

    /// ⚠ The other half: above 1.0 the table's last entry is the **exact** answer, not an
    /// approximation, because `gain_for_luma` clamps. A `LUT_TOP` chosen as a safe bound rather
    /// than derived would make this false and nothing else would notice.
    #[test]
    fn the_gain_is_flat_above_the_tables_top() {
        let s = ToneRegionsStage::new(100.0, 100.0, 100.0, -100.0);
        let top = s.gain_for_luma(ToneRegionsStage::LUT_TOP);
        for l in [1.0, 1.5, 4.0, 64.0] {
            assert_eq!(s.gain_for_luma(l), top, "gain moved at luminance {l}");
        }
    }

    /// ⚠⚠ **The defect this stage shipped with: Whites did nothing on most photographs.** Its
    /// weight was `max(0, 3l − 2)` on *linear* luminance, so it was exactly zero below linear
    /// 0.667 — and in a scene-referred buffer the brightest 0.1% of an ordinary frame sits near
    /// 0.1–0.3 even after an auto exposure. Auto Tone could not solve for it and parked it at 0
    /// or at a boundary.
    ///
    /// ⚠ The assertion is that the gain is **strictly** above 1 at a realistic highlight, not
    /// merely that it is large somewhere: a test probing only l = 1.0 passes against the broken
    /// version, which is why it went unnoticed.
    #[test]
    fn whites_have_leverage_where_photographs_actually_are() {
        let s = ToneRegionsStage::new(0.0, 0.0, 100.0, 0.0);
        for l in [0.12, 0.28, 0.40, 0.60] {
            assert!(
                s.gain_for_luma(l) > 1.001,
                "whites are inert at linear {l}: gain {}",
                s.gain_for_luma(l)
            );
        }
        // And still strongest at the top — it is a white point, not a second Exposure.
        assert!(s.gain_for_luma(0.9) > s.gain_for_luma(0.3));
    }

    /// ⚠ Blacks is the mirror, and it was the half that *worked* — because `max(0, 1 − 3l)` on
    /// linear luminance covers everything below sRGB 0.60, which is over half the visible range.
    /// It is properly concentrated now, so it must still bite hard in real shadows.
    #[test]
    fn blacks_bite_in_the_shadows_and_barely_reach_the_midtones() {
        let s = ToneRegionsStage::new(0.0, 0.0, 0.0, 100.0);
        assert!(
            s.gain_for_luma(0.02) > 1.1,
            "blacks are weak in the shadows"
        );
        let mid = s.gain_for_luma(0.18);
        assert!(
            mid > 1.0 && mid < 1.15,
            "blacks reach too far into the midtones: {mid}"
        );
    }

    /// ⚠⚠ **Whites and Highlights must stay distinguishable**, or the panel has two sliders doing
    /// one job. `^4` against `^2` is what keeps Whites end-weighted; if they ever converge, the
    /// end pair has stopped being a white/black point control.
    #[test]
    fn whites_are_more_end_weighted_than_highlights() {
        let w = ToneRegionsStage::new(0.0, 0.0, 100.0, 0.0);
        let h = ToneRegionsStage::new(100.0, 0.0, 0.0, 0.0);
        // At mid-grey highlights should dominate; near white they converge.
        let mid_ratio = (h.gain_for_luma(0.18) - 1.0) / (w.gain_for_luma(0.18) - 1.0);
        assert!(
            mid_ratio > 1.4,
            "whites reach as far into the midtones as highlights (ratio {mid_ratio})"
        );
        assert!(w.gain_for_luma(1.0) >= h.gain_for_luma(1.0) - 1e-6);
    }

    #[test]
    fn all_zero_is_identity() {
        let s = ToneRegionsStage::new(0.0, 0.0, 0.0, 0.0);
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.2, 0.5, 0.8]);
        let out = s.apply(img);
        let [r, g, b] = out.pixel(0, 0);
        assert!((r - 0.2).abs() < 1e-6 && (g - 0.5).abs() < 1e-6 && (b - 0.8).abs() < 1e-6);
    }

    #[test]
    fn positive_shadows_brighten_darks_more_than_lights() {
        let s = ToneRegionsStage::new(0.0, 100.0, 0.0, 0.0);
        assert!(s.gain_for_luma(0.05) > s.gain_for_luma(0.9));
        assert!(s.gain_for_luma(0.9) >= 1.0);
    }

    #[test]
    fn positive_highlights_brighten_lights_more_than_darks() {
        let s = ToneRegionsStage::new(100.0, 0.0, 0.0, 0.0);
        assert!(s.gain_for_luma(0.95) > s.gain_for_luma(0.1));
    }
}

use crate::PipelineStage;
use core_image::ImageBuf;

/// Guards the chroma ratio against division by zero on black pixels.
const EPS: f32 = 1e-6;

/// Chroma scaling around the luma axis: a uniform part (`saturation`) times a part
/// weighted toward already-muted pixels (`vibrance`).
///
/// `out = luma + (rgb - luma) * f_sat * f_vib`, clamped at 0.
pub struct SaturationStage {
    /// 1 + saturation/100: 0 = monochrome, 1 = neutral, 2 = double chroma.
    f_sat: f32,
    /// vibrance/100, applied as `1 + vib * (1 - chroma)`.
    vib: f32,
}

impl SaturationStage {
    /// Both amounts ~[-100, 100]; `(0, 0)` is the identity.
    pub fn new(saturation: f32, vibrance: f32) -> Self {
        Self {
            f_sat: 1.0 + saturation / 100.0,
            vib: vibrance / 100.0,
        }
    }
}

impl PipelineStage for SaturationStage {
    fn name(&self) -> &'static str {
        "saturation"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (f_sat, vib) = (self.f_sat, self.vib);
        image.map_pixels(|[r, g, b]| {
            // ⚠ The pivot chroma is scaled around: real luminance in the working space.
            let luma = color::luminance([r, g, b]);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            // Current chroma in [0, 1]; 0 for neutrals, 1 for a fully saturated color.
            let chroma = ((max - min) / max.max(EPS)).clamp(0.0, 1.0);
            let f = f_sat * (1.0 + vib * (1.0 - chroma));
            [
                (luma + (r - luma) * f).max(0.0),
                (luma + (g - luma) * f).max(0.0),
                (luma + (b - luma) * f).max(0.0),
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Luminance of a linear **ProPhoto** triple — the value chroma is scaled around.
    ///
    /// ⚠ A private copy of the working space's weights, deliberately **not** a call to
    /// `color::luminance`. A test that computes its expectation through production code cannot
    /// witness a change to production code — the same reason the golden harness keeps its own
    /// quantiser. It was Rec.709 until the working space changed, and measuring ProPhoto data
    /// with sRGB weights is what made this test fail.
    fn luma(px: [f32; 3]) -> f32 {
        0.2880402 * px[0] + 0.7118741 * px[1] + 0.0000857 * px[2]
    }

    fn chroma(px: [f32; 3]) -> f32 {
        px[0].max(px[1]).max(px[2]) - px[0].min(px[1]).min(px[2])
    }

    fn apply_one(sat: f32, vib: f32, px: [f32; 3]) -> [f32; 3] {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, px);
        SaturationStage::new(sat, vib).apply(img).pixel(0, 0)
    }

    #[test]
    fn zero_is_identity() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(0.0, 0.0, px);
        for i in 0..3 {
            assert!(
                (out[i] - px[i]).abs() < 1e-6,
                "channel {i}: {out:?} vs {px:?}"
            );
        }
    }

    #[test]
    fn minus_100_is_monochrome_preserving_luma() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(-100.0, 0.0, px);
        let l = luma(px);
        for i in 0..3 {
            assert!(
                (out[i] - l).abs() < 1e-6,
                "channel {i}: {out:?} vs luma {l}"
            );
        }
    }

    #[test]
    fn neutral_pixel_is_unchanged() {
        let px = [0.3, 0.3, 0.3];
        let out = apply_one(80.0, 60.0, px);
        for i in 0..3 {
            assert!((out[i] - px[i]).abs() < 1e-6, "channel {i}: {out:?}");
        }
    }

    #[test]
    fn positive_saturation_widens_chroma() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(50.0, 0.0, px);
        assert!(chroma(out) > chroma(px) * 1.4, "{out:?}");
    }

    #[test]
    fn vibrance_favours_muted_pixels() {
        // Muted: chroma/max = 0.2. Saturated: chroma/max = 1.0 (vibrance is a no-op there).
        let muted = [0.5, 0.45, 0.4];
        let saturated = [0.5, 0.05, 0.0];
        let muted_gain = chroma(apply_one(0.0, 50.0, muted)) / chroma(muted);
        let saturated_gain = chroma(apply_one(0.0, 50.0, saturated)) / chroma(saturated);
        assert!(
            muted_gain > saturated_gain + 0.2,
            "muted {muted_gain} should beat saturated {saturated_gain}"
        );
    }

    #[test]
    fn never_emits_negative_samples() {
        let out = apply_one(100.0, 100.0, [0.02, 0.01, 0.0]);
        assert!(out.iter().all(|&v| v >= 0.0), "{out:?}");
    }
}

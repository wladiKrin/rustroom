use crate::PipelineStage;
use core_image::ImageBuf;

/// Final stage: convert the linear **ProPhoto** working space to sRGB and encode it.
///
/// ⚠ This is where the pipeline's gamut is decided. Everything upstream works in ProPhoto, which
/// holds far more saturated colour than sRGB can express; the clipping happens here, once, at the
/// last possible moment — rather than silently in the decode, which is what the old sRGB working
/// space did.
pub struct OutputTransformSrgb;

impl PipelineStage for OutputTransformSrgb {
    fn name(&self) -> &'static str {
        "output_transform_srgb"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ Two steps, and the order is not negotiable: primaries first, in linear light, then
        // the transfer curve. Encoding before converting would apply a matrix to gamma-encoded
        // values, which is meaningless and does not fail loudly.
        //
        // ⚠ Out-of-gamut colour arrives here with negative components — that is the point of a
        // wide working space, not a bug. `linear_to_srgb` clamps them.
        image
            .map_pixels(color::prophoto_to_srgb)
            .map_samples(color::linear_to_srgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_authored_in_srgb_comes_back_unchanged() {
        // ⚠ The property the whole switch rests on: ProPhoto→sRGB is exact for in-gamut colour,
        // so a value converted *into* the working space must come back out where it started. If
        // this drifts, every one of the 38 goldens is wrong in the same invisible way — and the
        // golden harness's `neutral` case is precisely this property at image scale.
        let mut img = ImageBuf::new(3, 1);
        let srgb = [[0.18, 0.18, 0.18], [0.9, 0.2, 0.1], [0.1, 0.4, 0.8]];
        for (x, c) in srgb.iter().enumerate() {
            img.set_pixel(x, 0, color::srgb_to_prophoto(*c));
        }
        let out = OutputTransformSrgb.apply(img);
        for (x, c) in srgb.iter().enumerate() {
            let got = out.pixel(x, 0);
            for i in 0..3 {
                let want = color::linear_to_srgb(c[i]);
                assert!(
                    (got[i] - want).abs() < 2e-3,
                    "pixel {x} channel {i}: got {}, want {want}",
                    got[i]
                );
            }
        }
    }

    #[test]
    fn out_of_gamut_colour_is_clipped_here_and_only_here() {
        // ⚠ ProPhoto's own primary green is outside sRGB, so the matrix yields a negative
        // component and the transfer curve clamps it to 0. The clamp belongs at this boundary:
        // doing it earlier would discard the very data the wide space exists to carry.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.0, 1.0, 0.0]);
        let got = OutputTransformSrgb.apply(img).pixel(0, 0);
        assert!(
            got.iter().all(|v| (0.0..=1.0).contains(v)),
            "output left the encodable range: {got:?}"
        );
        assert_eq!(got[0], 0.0, "the negative red component should clamp to 0");
    }
}

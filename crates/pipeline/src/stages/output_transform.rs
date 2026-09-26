use crate::PipelineStage;
use core_image::ImageBuf;

/// Final stage: convert the linear **ProPhoto** working space to the chosen output space and
/// encode it.
///
/// ⚠ This is where the pipeline's gamut is decided. Everything upstream works in ProPhoto, which
/// holds far more saturated colour than sRGB can express; the clipping happens here, once, at the
/// last possible moment — rather than silently in the decode, which is what the old sRGB working
/// space did.
///
/// ⚠ **The preview always passes `Srgb`.** The display is assumed sRGB until a display profile
/// exists, so letting the canvas follow the export choice would make it lie about what is being
/// edited.
pub struct OutputTransform {
    space: color::OutputSpace,
    intent: color::RenderingIntent,
    gamut: color::GamutMap,
}

impl OutputTransform {
    /// ⚠ The intent decides what happens to colour `space` cannot hold (M6.4c). The default,
    /// `Clip`, is the per-channel clamp this stage always did; the other two keep hue and
    /// lightness and move chroma only. See `color::gamut` for why the clip stayed the default.
    pub fn new(space: color::OutputSpace, intent: color::RenderingIntent) -> Self {
        Self {
            space,
            intent,
            gamut: color::GamutMap::for_space(space),
        }
    }

    pub fn space(&self) -> color::OutputSpace {
        self.space
    }

    pub fn intent(&self) -> color::RenderingIntent {
        self.intent
    }
}

impl crate::per_pixel::PerPixel for OutputTransform {
    fn px(&self, _x: usize, _y: usize, c: [f32; 3]) -> [f32; 3] {
        // ⚠ Two steps, and the order is not negotiable: primaries first, in linear light, then
        // the transfer curve. Encoding before converting would apply a matrix to gamma-encoded
        // values, which is meaningless and does not fail loudly.
        //
        // ⚠ Out-of-gamut colour arrives here with negative components — that is the point of a
        // wide working space, not a bug. Under `Clip` the map is the identity and `encode` clamps
        // each channel; under the other two it brings the colour inside at constant hue first and
        // `encode`'s clamp is left only float residue (measured under 1e-3).
        let lin = self.space.from_working(self.gamut.map(self.intent, c));
        [
            self.space.encode(lin[0]),
            self.space.encode(lin[1]),
            self.space.encode(lin[2]),
        ]
    }
}

impl PipelineStage for OutputTransform {
    fn name(&self) -> &'static str {
        "output_transform"
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
        let out = OutputTransform::new(color::OutputSpace::Srgb, color::RenderingIntent::default())
            .apply(img);
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
        let got = OutputTransform::new(color::OutputSpace::Srgb, color::RenderingIntent::default())
            .apply(img)
            .pixel(0, 0);
        assert!(
            got.iter().all(|v| (0.0..=1.0).contains(v)),
            "output left the encodable range: {got:?}"
        );
        assert_eq!(got[0], 0.0, "the negative red component should clamp to 0");
    }

    #[test]
    fn the_space_decides_the_output_and_srgb_is_unchanged() {
        // ⚠ Two claims in one test, and both matter. The sRGB half is the regression guard: it is
        // the default, so if it moved, all 38 goldens would be wrong in the same invisible way. The
        // P3 half is the capability: a saturated green must come out *differently*, or the parameter
        // is being ignored and every space silently writes sRGB.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, color::srgb_to_prophoto([0.1, 0.9, 0.2]));

        let srgb =
            OutputTransform::new(color::OutputSpace::Srgb, color::RenderingIntent::default())
                .apply(img.clone())
                .pixel(0, 0);
        for (i, want) in [0.1_f32, 0.9, 0.2].iter().enumerate() {
            let want = color::linear_to_srgb(*want);
            assert!(
                (srgb[i] - want).abs() < 2e-3,
                "sRGB channel {i}: got {}, want {want}",
                srgb[i]
            );
        }

        let p3 = OutputTransform::new(
            color::OutputSpace::DisplayP3,
            color::RenderingIntent::default(),
        )
        .apply(img)
        .pixel(0, 0);
        assert!(
            (p3[1] - srgb[1]).abs() > 1e-3,
            "P3 output should differ from sRGB, got {p3:?} vs {srgb:?}"
        );
    }

    #[test]
    fn a_prophoto_export_clips_nothing_in_gamut() {
        // ⚠ ProPhoto is the identity in the matrix, so a working-space value inside [0,1] must reach
        // the encoder untouched — nothing is out of gamut in its own space. Any negative or clipped
        // channel here means a matrix crept back in.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.05, 0.8, 0.4]);
        let got = OutputTransform::new(
            color::OutputSpace::ProPhoto,
            color::RenderingIntent::default(),
        )
        .apply(img)
        .pixel(0, 0);
        for (i, v) in [0.05_f32, 0.8, 0.4].iter().enumerate() {
            let want = color::OutputSpace::ProPhoto.encode(*v);
            assert!(
                (got[i] - want).abs() < 1e-6,
                "channel {i}: got {}, want {want}",
                got[i]
            );
        }
    }
}

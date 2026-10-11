//! How big an export is (M10.10b): Lightroom's ways of saying it, in pixels or in a print size.
//!
//! ⚠⚠ **EVERY RULE HERE IS APPLIED TO THE FINAL, CROPPED PICTURE** — [`OutputSize::plan`] is handed
//! its size, never the decode's. Until M10.10b the long edge was applied to the uncropped decode, so a
//! photo cropped to half its width came out at half the size asked for; a print size makes that
//! unacceptable, since "30 × 20 cm" means the print.
//!
//! ⚠ A pure module: no pixels, no files. The app predicts the final size, asks for a [`SizePlan`] and
//! renders to it; the PPI the plan carries is what the file says.

use serde::{Deserialize, Deserializer, Serialize};

/// What the size is set by — Lightroom's *Resize to Fit* list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitMode {
    /// The photo's own size.
    #[default]
    Original,
    /// Fit inside a box `width` wide and `height` high, as given.
    WidthHeight,
    /// Fit inside a box either way round — a 30 × 20 cm print takes a portrait as 20 × 30.
    Dimensions,
    LongEdge,
    ShortEdge,
    Megapixels,
    Percentage,
}

impl FitMode {
    /// The order the dialog lists — beside the variants, `WatermarkCorner::ALL`'s rule.
    pub const ALL: [FitMode; 7] = [
        FitMode::Original,
        FitMode::WidthHeight,
        FitMode::Dimensions,
        FitMode::LongEdge,
        FitMode::ShortEdge,
        FitMode::Megapixels,
        FitMode::Percentage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FitMode::Original => "Original size",
            FitMode::WidthHeight => "Width & height",
            FitMode::Dimensions => "Dimensions",
            FitMode::LongEdge => "Long edge",
            FitMode::ShortEdge => "Short edge",
            FitMode::Megapixels => "Megapixels",
            FitMode::Percentage => "Percentage",
        }
    }

    /// Whether the mode is set by lengths, which can be a print size.
    pub fn uses_length(self) -> bool {
        matches!(
            self,
            FitMode::WidthHeight | FitMode::Dimensions | FitMode::LongEdge | FitMode::ShortEdge
        )
    }
}

/// The unit the lengths are given in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    #[default]
    Px,
    Cm,
    In,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 3] = [LengthUnit::Px, LengthUnit::Cm, LengthUnit::In];

    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Px => "px",
            LengthUnit::Cm => "cm",
            LengthUnit::In => "in",
        }
    }

    /// `v` of this unit in pixels at `ppi`.
    pub fn to_px(self, v: f32, ppi: f32) -> f32 {
        match self {
            LengthUnit::Px => v,
            LengthUnit::Cm => v / CM_PER_INCH * ppi,
            LengthUnit::In => v * ppi,
        }
    }

    /// `px` pixels in this unit at `ppi`.
    pub fn from_px(self, px: f32, ppi: f32) -> f32 {
        match self {
            LengthUnit::Px => px,
            LengthUnit::Cm => px / ppi * CM_PER_INCH,
            LengthUnit::In => px / ppi,
        }
    }
}

const CM_PER_INCH: f32 = 2.54;

/// How big an export is.
///
/// ⚠ **Each mode keeps its own number**, so switching to Megapixels and back loses nothing — the
/// author's rule for anything adjustable. The four lengths are in `unit`; [`Self::set_unit`]
/// converts them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSize {
    pub fit: FitMode,
    pub unit: LengthUnit,
    pub long_edge: f32,
    pub short_edge: f32,
    pub width: f32,
    pub height: f32,
    pub megapixels: f32,
    pub percent: f32,
    /// Pixels per inch: what a print size is measured in, and what every file says.
    pub ppi: u16,
    /// Never scale a photo up; a print size it cannot fill is reached by a lower PPI instead.
    pub dont_enlarge: bool,
}

/// ⚠ The values are finite by construction — the dialog's boxes are ranged and JSON has no NaN —
/// so equality is total, and `ExportOptions` can stay `Eq`.
impl Eq for OutputSize {}

impl Default for OutputSize {
    /// ⚠⚠ **Hand-written, not derived** — a container-level `#[serde(default)]` fills a missing
    /// field from **this**, and a derived one would give every older file a 0 PPI and zero lengths.
    /// `Watermark::default`'s trap.
    fn default() -> Self {
        Self {
            fit: FitMode::Original,
            unit: LengthUnit::Px,
            long_edge: 2048.0,
            short_edge: 1536.0,
            width: 1920.0,
            height: 1080.0,
            megapixels: 12.0,
            percent: 50.0,
            ppi: 300,
            dont_enlarge: true,
        }
    }
}

/// What an export of one photo comes out at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizePlan {
    pub width: usize,
    pub height: usize,
    /// The PPI the file carries — lower than asked when Don't enlarge kept a print size reachable.
    pub ppi: f32,
    /// A print size the photo's pixels could not fill at the asked PPI.
    pub below_ppi: bool,
}

impl OutputSize {
    pub const PPI_RANGE: (u16, u16) = (1, 2400);

    /// A long edge in pixels — what every export before M10.10b was set by.
    pub fn long_edge_px(px: usize) -> Self {
        Self {
            fit: FitMode::LongEdge,
            unit: LengthUnit::Px,
            long_edge: px as f32,
            ..Self::default()
        }
    }

    /// Change the unit, converting the four lengths through the PPI so the size stays the same.
    pub fn set_unit(&mut self, unit: LengthUnit) {
        let ppi = f32::from(self.ppi.max(1));
        let from = self.unit;
        for v in [
            &mut self.long_edge,
            &mut self.short_edge,
            &mut self.width,
            &mut self.height,
        ] {
            *v = unit.from_px(from.to_px(*v, ppi), ppi);
        }
        self.unit = unit;
    }

    /// The size a photo whose final, cropped picture is `final_w` × `final_h` comes out at.
    pub fn plan(&self, final_w: usize, final_h: usize) -> SizePlan {
        let ppi = f32::from(self.ppi.max(1));
        let (w, h) = (final_w.max(1) as f64, final_h.max(1) as f64);
        let (long, short) = (w.max(h), w.min(h));
        let px = |v: f32| f64::from(self.unit.to_px(v, ppi));
        let asked = match self.fit {
            FitMode::Original => 1.0,
            FitMode::WidthHeight => (px(self.width) / w).min(px(self.height) / h),
            FitMode::Dimensions => {
                let (a, b) = (px(self.width), px(self.height));
                (a.max(b) / long).min(a.min(b) / short)
            }
            FitMode::LongEdge => px(self.long_edge) / long,
            FitMode::ShortEdge => px(self.short_edge) / short,
            FitMode::Megapixels => (f64::from(self.megapixels) * 1e6 / (w * h)).sqrt(),
            FitMode::Percentage => f64::from(self.percent) / 100.0,
        };
        let asked = if asked.is_finite() && asked > 0.0 {
            asked
        } else {
            1.0
        };
        let clamped = asked > 1.0 && self.dont_enlarge;
        let k = if clamped { 1.0 } else { asked };
        // ⚠ Only a print size is kept by lowering the PPI: in pixels, megapixels or a percentage
        // there is no physical size to keep, and the file says the PPI as set.
        let print = clamped && self.fit.uses_length() && self.unit != LengthUnit::Px;
        SizePlan {
            width: ((w * k).round() as usize).max(1),
            height: ((h * k).round() as usize).max(1),
            ppi: if print { ppi / asked as f32 } else { ppi },
            below_ppi: print,
        }
    }
}

/// Read `ExportOptions::size`, or the `max_edge` it replaced — `"max_edge": 2048` is a long edge of
/// 2048 px, `null` the photo's own size.
///
/// ⚠ For `#[serde(alias = "max_edge", deserialize_with = "…")]`: an options file or export preset
/// written before M10.10b must size exactly as it did.
pub fn size_or_max_edge<'de, D: Deserializer<'de>>(d: D) -> Result<OutputSize, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Old(Option<usize>),
        New(OutputSize),
    }
    Ok(match Either::deserialize(d)? {
        Either::Old(Some(px)) => OutputSize::long_edge_px(px),
        Either::Old(None) => OutputSize::default(),
        Either::New(s) => s,
    })
}

/// `image` — display-encoded in `space` — resized to `w` × `h` **in linear light**: the encoding
/// undone, Lanczos3, encoded again (M10.10b).
///
/// ⚠⚠ Resizing the code values would darken every edge and fine texture, since averaging encoded
/// values is not averaging light. For enlarging past the photo's pixels and for shrinking a file
/// under its size limit — both after the render, where the buffer is already encoded.
pub fn resample_encoded(
    image: &core_image::ImageBuf,
    space: color::OutputSpace,
    w: usize,
    h: usize,
) -> core_image::ImageBuf {
    use image::imageops::{resize, FilterType};
    let (iw, ih) = (image.width() as u32, image.height() as u32);
    let linear: Vec<f32> = image.data().iter().map(|&v| space.decode(v)).collect();
    let buf = image::Rgb32FImage::from_raw(iw, ih, linear).expect("an RGB buffer of its own size");
    let out = resize(&buf, w.max(1) as u32, h.max(1) as u32, FilterType::Lanczos3);
    let mut img = core_image::ImageBuf::new(w.max(1), h.max(1));
    for (d, s) in img.data_mut().iter_mut().zip(out.into_raw()) {
        *d = space.encode(s);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use FitMode::*;
    use LengthUnit::*;

    fn size(fit: FitMode, unit: LengthUnit) -> OutputSize {
        OutputSize {
            fit,
            unit,
            ..OutputSize::default()
        }
    }

    fn dims(p: SizePlan) -> (usize, usize) {
        (p.width, p.height)
    }

    #[test]
    fn the_default_is_inert_and_its_other_fields_are_not_zero() {
        // ⚠ Both halves: a derived `Default` would also be inert (Original), and would hand every
        // options file that predates these fields a zero long edge and a 0 PPI.
        let d = OutputSize::default();
        assert_eq!(d.fit, Original);
        assert_eq!(dims(d.plan(6000, 4000)), (6000, 4000));
        assert_eq!(
            (d.long_edge, d.short_edge, d.width, d.height),
            (2048.0, 1536.0, 1920.0, 1080.0)
        );
        assert_eq!(
            (d.megapixels, d.percent, d.ppi, d.dont_enlarge),
            (12.0, 50.0, 300, true)
        );
    }

    #[test]
    fn each_mode_sizes_a_landscape_and_a_portrait() {
        let long = OutputSize {
            long_edge: 2048.0,
            ..size(LongEdge, Px)
        };
        assert_eq!(dims(long.plan(6000, 4000)), (2048, 1365));
        assert_eq!(dims(long.plan(4000, 6000)), (1365, 2048));
        let short = OutputSize {
            short_edge: 1000.0,
            ..size(ShortEdge, Px)
        };
        assert_eq!(dims(short.plan(6000, 4000)), (1500, 1000));
        assert_eq!(dims(short.plan(4000, 6000)), (1000, 1500));
        let pct = OutputSize {
            percent: 25.0,
            ..size(Percentage, Px)
        };
        assert_eq!(dims(pct.plan(6000, 4000)), (1500, 1000));
        let mp = OutputSize {
            megapixels: 6.0,
            ..size(Megapixels, Px)
        };
        let p = mp.plan(6000, 4000);
        let got = (p.width * p.height) as f64;
        assert!((got / 6e6 - 1.0).abs() < 0.01, "{got}");
        assert!((p.width as f64 / p.height as f64 - 1.5).abs() < 0.01);
    }

    #[test]
    fn a_box_in_centimetres_gives_exact_pixels() {
        let wh = OutputSize {
            width: 30.0,
            height: 20.0,
            ..size(WidthHeight, Cm)
        };
        let p = wh.plan(6000, 4000);
        assert_eq!(dims(p), (3543, 2362));
        assert_eq!(p.ppi, 300.0);
        assert!(!p.below_ppi);
        // ⚠ Width & height is the box as given: a portrait fits inside 30 wide and 20 high.
        assert_eq!(dims(wh.plan(4000, 6000)), (1575, 2362));
        // Dimensions fits it either way round.
        let any = OutputSize {
            fit: Dimensions,
            ..wh
        };
        assert_eq!(dims(any.plan(4000, 6000)), (2362, 3543));
        assert_eq!(dims(any.plan(6000, 4000)), (3543, 2362));
    }

    #[test]
    fn dont_enlarge_keeps_the_pixels_and_lowers_the_ppi_of_a_print_size() {
        let print = OutputSize {
            long_edge: 70.0,
            ..size(LongEdge, Cm)
        };
        let p = print.plan(6240, 4160);
        assert_eq!(dims(p), (6240, 4160));
        assert!((p.ppi - 226.42).abs() < 0.05, "{}", p.ppi);
        assert!(p.below_ppi);
        let up = OutputSize {
            dont_enlarge: false,
            ..print
        };
        let p = up.plan(6240, 4160);
        assert_eq!(dims(p), (8268, 5512));
        assert_eq!(p.ppi, 300.0);
        assert!(!p.below_ppi);
        // In pixels there is no print size to keep: the PPI stays as set.
        let px = OutputSize {
            long_edge: 9000.0,
            ..size(LongEdge, Px)
        };
        let p = px.plan(6240, 4160);
        assert_eq!(dims(p), (6240, 4160));
        assert_eq!(p.ppi, 300.0);
        assert!(!p.below_ppi);
    }

    #[test]
    fn original_keeps_the_photo_and_carries_the_ppi() {
        let p = OutputSize {
            ppi: 240,
            ..OutputSize::default()
        }
        .plan(6000, 4000);
        assert_eq!(dims(p), (6000, 4000));
        assert_eq!(p.ppi, 240.0);
        assert!(!p.below_ppi);
    }

    #[test]
    fn switching_units_converts_and_round_trips() {
        let mut s = size(WidthHeight, Px);
        let px_plan = s.plan(6000, 4000);
        let before = (s.long_edge, s.short_edge, s.width, s.height);
        s.set_unit(Cm);
        // 2048 px at 300 PPI is 6.827 in, 17.34 cm.
        assert!((s.long_edge - 17.34).abs() < 0.01, "{}", s.long_edge);
        assert_eq!(
            dims(s.plan(6000, 4000)),
            dims(px_plan),
            "the same size in cm"
        );
        s.set_unit(In);
        assert!((s.long_edge - 6.827).abs() < 0.001, "{}", s.long_edge);
        s.set_unit(Px);
        let after = (s.long_edge, s.short_edge, s.width, s.height);
        for (a, b) in [
            (after.0, before.0),
            (after.1, before.1),
            (after.2, before.2),
            (after.3, before.3),
        ] {
            assert!((a - b).abs() < 0.01, "{a} came back from {b}");
        }
    }

    #[derive(serde::Deserialize, Debug)]
    struct Holder {
        #[serde(default, alias = "max_edge", deserialize_with = "size_or_max_edge")]
        size: OutputSize,
    }

    #[test]
    fn an_old_max_edge_reads_as_a_long_edge_in_pixels() {
        let h: Holder = serde_json::from_str(r#"{"max_edge":2048}"#).expect("old");
        assert_eq!(h.size, OutputSize::long_edge_px(2048));
        assert_eq!((h.size.fit, h.size.unit), (LongEdge, Px));
        assert_eq!(h.size.long_edge, 2048.0);
        let h: Holder = serde_json::from_str(r#"{"max_edge":null}"#).expect("old, unset");
        assert_eq!(h.size.fit, Original);
        let h: Holder = serde_json::from_str("{}").expect("absent");
        assert_eq!(h.size, OutputSize::default());
        let new = OutputSize {
            width: 30.0,
            height: 20.0,
            ppi: 240,
            dont_enlarge: false,
            ..size(Dimensions, Cm)
        };
        let json = format!(r#"{{"size":{}}}"#, serde_json::to_string(&new).unwrap());
        let h: Holder = serde_json::from_str(&json).expect("new");
        assert_eq!(h.size, new);
    }
}

#[cfg(test)]
mod resample_tests {
    use super::*;
    use color::OutputSpace;
    use core_image::ImageBuf;

    #[test]
    fn resizing_averages_light_not_code_values() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0; 3]);
        img.set_pixel(1, 0, [1.0; 3]);
        let one = resample_encoded(&img, OutputSpace::Srgb, 1, 1);
        let v = one.pixel(0, 0)[0];
        // Half the light is sRGB 0.735; averaging the code values would give 0.5.
        let want = OutputSpace::Srgb.encode(0.5);
        assert!((v - want).abs() < 0.01, "{v}, want {want}");
        assert_eq!((one.width(), one.height()), (1, 1));
    }

    #[test]
    fn enlarging_a_flat_colour_keeps_it() {
        let mut img = ImageBuf::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.25, 0.5, 0.75]);
            }
        }
        let big = resample_encoded(&img, OutputSpace::DisplayP3, 12, 6);
        assert_eq!((big.width(), big.height()), (12, 6));
        for v in big.data().as_chunks::<3>().0 {
            assert!(
                (v[0] - 0.25).abs() < 1e-3 && (v[2] - 0.75).abs() < 1e-3,
                "{v:?}"
            );
        }
    }
}

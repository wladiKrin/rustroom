//! Core image container: interleaved f32 RGB, scene-referred linear working space.

use rayon::prelude::*;

/// An interleaved RGB image with `f32` samples in linear light (typically `0.0..=1.0`,
/// but values may exceed 1.0 for scene-referred highlights).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBuf {
    width: usize,
    height: usize,
    /// Interleaved RGB, length = width * height * 3.
    data: Vec<f32>,
}

/// Pixels per parallel task in the 8-bit conversions.
///
/// ⚠ **The granularity is the whole point, and it was measured.** Handing rayon one *pixel* per
/// task — `par_chunks_exact_mut(4)`, the obvious spelling — is **slower than not threading at
/// all**: 1.64 ms against 1.13 ms on a 1600×1068 frame, because the per-item dispatch stops the
/// inner work vectorising. A coarse block with a plain sequential loop inside gets both, and
/// runs in 0.46 ms. Anything from ~1024 up performs the same (52.8 / 58.9 / 61.3 GB/s at 1024 /
/// 8192 / 65536), so this sits in the middle of a flat region rather than on a tuned peak.
const PACK_BLOCK_PX: usize = 8192;

impl ImageBuf {
    pub const CHANNELS: usize = 3;

    /// Create a zero-filled image of the given dimensions.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * Self::CHANNELS],
        }
    }

    /// Build directly from an existing interleaved RGB buffer.
    ///
    /// # Panics
    /// Panics if `data.len() != width * height * 3`.
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Self {
        assert_eq!(
            data.len(),
            width * height * Self::CHANNELS,
            "buffer size mismatch"
        );
        Self {
            width,
            height,
            data,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn channels(&self) -> usize {
        Self::CHANNELS
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        (y * self.width + x) * Self::CHANNELS
    }

    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = self.index(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        let i = self.index(x, y);
        self.data[i] = rgb[0];
        self.data[i + 1] = rgb[1];
        self.data[i + 2] = rgb[2];
    }

    /// Return a new image with `f` applied to every sample (all channels), dimensions preserved.
    ///
    /// Parallel across cores: every sample is independent, so the result is identical to the
    /// sequential one, bit for bit. `f` must therefore be `Sync + Send`.
    pub fn map_samples<F: Fn(f32) -> f32 + Sync + Send>(&self, f: F) -> ImageBuf {
        let mut data = vec![0.0f32; self.data.len()];
        data.par_iter_mut()
            .zip(self.data.par_iter())
            .for_each(|(out, &s)| *out = f(s));
        ImageBuf::from_vec(self.width, self.height, data)
    }

    /// Extract an axis-aligned sub-rectangle. Requires `x + w <= width` and `y + h <= height`.
    ///
    /// # Panics
    /// Panics if the rectangle exceeds the image bounds, or if `w == 0 || h == 0`.
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> ImageBuf {
        assert!(w > 0 && h > 0, "crop size must be non-zero");
        assert!(
            x + w <= self.width && y + h <= self.height,
            "crop rectangle out of bounds"
        );
        let mut data = Vec::with_capacity(w * h * Self::CHANNELS);
        for row in y..y + h {
            let start = (row * self.width + x) * Self::CHANNELS;
            let end = start + w * Self::CHANNELS;
            data.extend_from_slice(&self.data[start..end]);
        }
        ImageBuf::from_vec(w, h, data)
    }

    /// Return a new image with `f` applied to each RGB pixel, dimensions preserved.
    ///
    /// Parallel across cores: pixels are independent, so the result is identical to the
    /// sequential one, bit for bit. `f` must therefore be `Sync + Send`.
    pub fn map_pixels<F: Fn([f32; 3]) -> [f32; 3] + Sync + Send>(&self, f: F) -> ImageBuf {
        let mut data = vec![0.0f32; self.data.len()];
        data.par_chunks_exact_mut(Self::CHANNELS)
            .zip(self.data.par_chunks_exact(Self::CHANNELS))
            .for_each(|(out, px)| {
                out.copy_from_slice(&f([px[0], px[1], px[2]]));
            });
        ImageBuf::from_vec(self.width, self.height, data)
    }

    /// Return a new image with `f` applied to each RGB pixel *together with its `(x, y)`*,
    /// dimensions preserved. For effects whose result depends on position — vignette today;
    /// graduated and radial filters, and lens corrections, later.
    ///
    /// Parallel across rows: pixels are independent, so the result is identical to the
    /// sequential one, bit for bit. `f` must therefore be `Sync + Send`.
    pub fn map_pixels_xy<F: Fn(usize, usize, [f32; 3]) -> [f32; 3] + Sync + Send>(
        &self,
        f: F,
    ) -> ImageBuf {
        let row = self.width * Self::CHANNELS;
        let mut data = vec![0.0f32; self.data.len()];
        if row == 0 {
            // `par_chunks_exact_mut(0)` panics.
            return ImageBuf::from_vec(self.width, self.height, data);
        }
        data.par_chunks_exact_mut(row)
            .zip(self.data.par_chunks_exact(row))
            .enumerate()
            .for_each(|(y, (orow, srow))| {
                // ⚠ `as_chunks` rather than `chunks_exact`: it yields `[f32; CHANNELS]` arrays
                // instead of slices, so the pixel is a value the optimiser knows the length of.
                // Required by clippy's `chunks_exact_to_as_chunks` from 1.98 onward, and it also
                // lets the closure's result be assigned directly rather than copied slice-wise.
                for (x, (out, px)) in orow
                    .as_chunks_mut::<{ Self::CHANNELS }>()
                    .0
                    .iter_mut()
                    .zip(srow.as_chunks::<{ Self::CHANNELS }>().0)
                    .enumerate()
                {
                    *out = f(x, y, *px);
                }
            });
        ImageBuf::from_vec(self.width, self.height, data)
    }

    /// Quantize one display-referred sample to 8 bits: clamp to `0..=1`, scale, round.
    ///
    /// Private and shared, so [`ImageBuf::to_rgb8`] and [`ImageBuf::to_rgba8`] cannot come to
    /// disagree about rounding.
    ///
    /// ⚠ A `NaN` sample ends up as 0, but **not** because of the clamp: `f32::clamp` returns
    /// `NaN` for a `NaN` input. It is the `as u8` cast that saturates it, Rust's float-to-int
    /// casts being saturating. 0 is the right answer for a sample that means nothing; the
    /// note is here because "clamp handles NaN" is the obvious wrong reading of this line.
    fn quantize(v: f32) -> u8 {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Quantize to 8-bit RGB — what an encoder wants.
    ///
    /// Parallel in blocks; bit-identical to a sequential loop, since each output byte depends
    /// on exactly one input sample.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let block = PACK_BLOCK_PX * Self::CHANNELS;
        let mut out = vec![0u8; self.data.len()];
        out.par_chunks_mut(block)
            .zip(self.data.par_chunks(block))
            .for_each(|(ob, sb)| {
                for (dst, &src) in ob.iter_mut().zip(sb.iter()) {
                    *dst = Self::quantize(src);
                }
            });
        out
    }

    /// Quantize to 8-bit RGBA with an opaque alpha — the layout a GPU texture wants.
    ///
    /// Parallel in blocks, and bit-identical to a sequential loop for the same reason.
    ///
    /// ⚠ Unlike [`ImageBuf::map_pixels_xy`] there is **no zero-size guard**, and that is
    /// correct rather than an oversight: the chunk widths here are non-zero constants, so a
    /// zero-width image simply yields an empty `Vec`. `map_pixels_xy` chunks by *row*, which
    /// is 0 for a zero-width image, and `par_chunks_exact_mut(0)` panics. Using a fixed block
    /// rather than a row is what buys that.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        out.par_chunks_mut(PACK_BLOCK_PX * 4)
            .zip(self.data.par_chunks(PACK_BLOCK_PX * Self::CHANNELS))
            .for_each(|(ob, sb)| {
                for (dst, src) in ob
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(sb.as_chunks::<{ Self::CHANNELS }>().0)
                {
                    dst[0] = Self::quantize(src[0]);
                    dst[1] = Self::quantize(src[1]);
                    dst[2] = Self::quantize(src[2]);
                    dst[3] = 255;
                }
            });
        out
    }

    /// Downscale so the longer edge is at most `max_dim`, preserving aspect ratio, using
    /// area averaging in the current (linear) light. Never upscales; returns a copy if
    /// already small enough or if `max_dim == 0`.
    ///
    /// Parallel across **output rows**: each output pixel averages its own source rectangle
    /// and reads nothing else, so the result is identical to the sequential one, bit for bit.
    /// Rows rather than the fixed blocks [`ImageBuf::to_rgba8`] uses, because a row here is
    /// already a substantial task — a 1600-wide output row averages tens of thousands of
    /// source pixels, so there is none of the per-item dispatch overhead that made a
    /// per-pixel split of the 8-bit pack slower than not threading at all.
    pub fn downscale_to(&self, max_dim: usize) -> ImageBuf {
        let long = self.width.max(self.height);
        // ⚠ The empty check is not redundant with the one below, and it is load-bearing twice
        // over. `dw`/`dh` are clamped up to 1, so a 0 x 5 source used to produce a 1 x 2 image
        // of `NaN` — the averaging loop found no source pixels and divided by zero. It also
        // guarantees the row chunk width below is non-zero, which `par_chunks_exact_mut`
        // requires.
        if max_dim == 0 || long <= max_dim || self.width == 0 || self.height == 0 {
            return self.clone();
        }
        let scale = max_dim as f32 / long as f32;
        let dw = (self.width as f32 * scale).round().max(1.0) as usize;
        let dh = (self.height as f32 * scale).round().max(1.0) as usize;

        let mut data = vec![0.0f32; dw * dh * Self::CHANNELS];
        data.par_chunks_exact_mut(dw * Self::CHANNELS)
            .enumerate()
            .for_each(|(oy, orow)| {
                let sy0 = oy * self.height / dh;
                let sy1 = (((oy + 1) * self.height / dh).max(sy0 + 1)).min(self.height);
                for (ox, out) in orow
                    .as_chunks_mut::<{ Self::CHANNELS }>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    let sx0 = ox * self.width / dw;
                    let sx1 = (((ox + 1) * self.width / dw).max(sx0 + 1)).min(self.width);
                    let mut acc = [0.0f32; 3];
                    let mut n = 0.0f32;
                    for sy in sy0..sy1 {
                        for sx in sx0..sx1 {
                            let p = self.pixel(sx, sy);
                            acc[0] += p[0];
                            acc[1] += p[1];
                            acc[2] += p[2];
                            n += 1.0;
                        }
                    }
                    out[0] = acc[0] / n;
                    out[1] = acc[1] / n;
                    out[2] = acc[2] / n;
                }
            });
        ImageBuf::from_vec(dw, dh, data)
    }
}

/// A decoded, display-referred preview image (interleaved RGBA8).
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image with awkward samples: below 0, above 1, and the exact rounding boundaries.
    /// A quantizer that clamped wrongly, rounded wrongly or transposed channels would differ
    /// on at least one of these.
    fn awkward() -> ImageBuf {
        let mut img = ImageBuf::new(3, 2);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        img.set_pixel(2, 0, [1.0 / 512.0, 0.999, 0.001]);
        img.set_pixel(0, 1, [f32::NAN, -0.0, 2.0]);
        img.set_pixel(1, 1, [0.4980392, 0.5019608, 0.75]);
        img.set_pixel(2, 1, [0.125, 0.875, 0.33333334]);
        img
    }

    /// The reference: a plain sequential loop, written out here rather than shared with
    /// production so it can hold the old behaviour still while production changes.
    fn rgb8_reference(img: &ImageBuf) -> Vec<u8> {
        img.data()
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }

    fn rgba8_reference(img: &ImageBuf) -> Vec<u8> {
        let mut out = Vec::with_capacity(img.width() * img.height() * 4);
        for px in img.data().as_chunks::<{ ImageBuf::CHANNELS }>().0 {
            out.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push(255);
        }
        out
    }

    /// The area-average downscale as a plain sequential loop, written out here rather than
    /// shared with production so it can hold the old behaviour still while production changes.
    fn downscale_reference(img: &ImageBuf, max_dim: usize) -> ImageBuf {
        let long = img.width().max(img.height());
        if max_dim == 0 || long <= max_dim {
            return img.clone();
        }
        let scale = max_dim as f32 / long as f32;
        let dw = (img.width() as f32 * scale).round().max(1.0) as usize;
        let dh = (img.height() as f32 * scale).round().max(1.0) as usize;
        let mut out = ImageBuf::new(dw, dh);
        for oy in 0..dh {
            let sy0 = oy * img.height() / dh;
            let sy1 = (((oy + 1) * img.height() / dh).max(sy0 + 1)).min(img.height());
            for ox in 0..dw {
                let sx0 = ox * img.width() / dw;
                let sx1 = (((ox + 1) * img.width() / dw).max(sx0 + 1)).min(img.width());
                let mut acc = [0.0f32; 3];
                let mut n = 0.0f32;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = img.pixel(sx, sy);
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                        n += 1.0;
                    }
                }
                out.set_pixel(ox, oy, [acc[0] / n, acc[1] / n, acc[2] / n]);
            }
        }
        out
    }

    /// A non-uniform image, so a mis-indexed row would show up rather than averaging away.
    fn varied(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / w as f32,
                        y as f32 / h as f32,
                        ((x * 7 + y * 13) % 11) as f32 / 11.0,
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn downscale_matches_a_sequential_reference() {
        // ⚠ Parallelising over output rows must be invisible in the output. Sizes chosen so
        // the source/output ratio is non-integral in both axes and the row count exceeds any
        // plausible thread count, so a row-indexing mistake cannot hide.
        for (w, h, max_dim) in [
            (300_usize, 200_usize, 64_usize),
            (200, 300, 64),
            (301, 199, 97),
            (64, 64, 63),
            (17, 5, 4),
        ] {
            let img = varied(w, h);
            let got = img.downscale_to(max_dim);
            let want = downscale_reference(&img, max_dim);
            assert_eq!(
                (got.width(), got.height()),
                (want.width(), want.height()),
                "{w}x{h} -> {max_dim}"
            );
            assert_eq!(got.data(), want.data(), "{w}x{h} -> {max_dim}");
        }
    }

    #[test]
    fn downscale_handles_the_degenerate_sizes() {
        // ⚠ Unlike `to_rgba8`, the row chunk width here is *derived*, so a zero would panic in
        // `par_chunks_exact_mut`. It is unreachable: `dw`/`dh` are clamped to at least 1, and
        // a zero-dimension image takes the early clone path. Pinned rather than trusted.
        assert_eq!(ImageBuf::new(0, 0).downscale_to(16).width(), 0);
        assert_eq!(ImageBuf::new(0, 5).downscale_to(2).width(), 0);
        // An extreme ratio still yields at least one pixel per axis.
        let thin = varied(1000, 3);
        let out = thin.downscale_to(4);
        assert_eq!((out.width(), out.height()), (4, 1));
        assert_eq!(out.data(), downscale_reference(&thin, 4).data());
    }

    #[test]
    fn to_rgb8_matches_a_sequential_reference() {
        let img = awkward();
        assert_eq!(img.to_rgb8(), rgb8_reference(&img));
        assert_eq!(img.to_rgb8().len(), 3 * 2 * ImageBuf::CHANNELS);
    }

    #[test]
    fn to_rgba8_matches_a_sequential_reference() {
        let img = awkward();
        assert_eq!(img.to_rgba8(), rgba8_reference(&img));
        assert_eq!(img.to_rgba8().len(), 3 * 2 * 4);
    }

    #[test]
    fn to_rgba8_is_to_rgb8_with_an_opaque_alpha() {
        // Proves the two really do share one quantizer rather than having two that agree
        // today and drift tomorrow.
        let img = awkward();
        let rgb = img.to_rgb8();
        let rgba = img.to_rgba8();
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(&px[..3], &rgb[i * 3..i * 3 + 3], "pixel {i} colour differs");
            assert_eq!(px[3], 255, "pixel {i} is not opaque");
        }
    }

    #[test]
    fn the_conversions_quantize_the_known_values() {
        // A spot check with the answers written out, so a reference loop that was wrong in
        // the same way as production could not hide it.
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        assert_eq!(img.to_rgb8(), vec![0, 128, 255, 0, 255, 64]);
        assert_eq!(img.to_rgba8(), vec![0, 128, 255, 255, 0, 255, 64, 255]);
    }

    #[test]
    fn the_conversions_handle_an_empty_image() {
        // Unlike `map_pixels_xy`, the chunk sizes here are non-zero constants, so there is no
        // `par_chunks_exact_mut(0)` panic to guard against. Pinned rather than trusted.
        let img = ImageBuf::new(0, 0);
        assert!(img.to_rgb8().is_empty());
        assert!(img.to_rgba8().is_empty());
    }

    #[test]
    fn new_image_is_zeroed_and_sized() {
        let img = ImageBuf::new(4, 3);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 3);
        assert_eq!(img.channels(), 3);
        assert_eq!(img.data().len(), 4 * 3 * 3);
        assert!(img.data().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn set_and_get_pixel_roundtrips() {
        let mut img = ImageBuf::new(2, 2);
        img.set_pixel(1, 0, [0.25, 0.5, 0.75]);
        assert_eq!(img.pixel(1, 0), [0.25, 0.5, 0.75]);
        assert_eq!(img.pixel(0, 0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn from_vec_preserves_data() {
        let data = vec![0.1; 2 * 2 * 3];
        let img = ImageBuf::from_vec(2, 2, data.clone());
        assert_eq!(img.data(), data.as_slice());
    }

    #[test]
    fn crop_extracts_subrect() {
        let mut img = ImageBuf::new(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                img.set_pixel(x, y, [x as f32, y as f32, 0.0]);
            }
        }
        let c = img.crop(1, 0, 2, 2);
        assert_eq!((c.width(), c.height()), (2, 2));
        assert_eq!(c.pixel(0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(c.pixel(1, 1), [2.0, 1.0, 0.0]);
    }

    #[test]
    fn downscale_solid_color_is_preserved() {
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.2, 0.4, 0.6]);
            }
        }
        let out = img.downscale_to(2);
        assert_eq!((out.width(), out.height()), (2, 2));
        for p in out.data().as_chunks::<3>().0 {
            assert!(
                (p[0] - 0.2).abs() < 1e-5 && (p[1] - 0.4).abs() < 1e-5 && (p[2] - 0.6).abs() < 1e-5
            );
        }
    }

    #[test]
    fn downscale_averages_a_block() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.0, 0.0]);
        img.set_pixel(1, 0, [1.0, 1.0, 1.0]);
        let out = img.downscale_to(1);
        assert_eq!((out.width(), out.height()), (1, 1));
        assert!((out.pixel(0, 0)[0] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn downscale_never_upscales_and_keeps_aspect() {
        let img = ImageBuf::new(10, 4);
        let out = img.downscale_to(100);
        assert_eq!((out.width(), out.height()), (10, 4));
        let out2 = ImageBuf::new(10, 4).downscale_to(5);
        assert_eq!((out2.width(), out2.height()), (5, 2));
    }

    #[test]
    fn map_samples_applies_to_every_channel() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        img.set_pixel(1, 0, [0.4, 0.5, 0.6]);
        let out = img.map_samples(|s| s * 2.0);
        assert_eq!(out.pixel(0, 0), [0.2, 0.4, 0.6]);
        assert_eq!(out.pixel(1, 0), [0.8, 1.0, 1.2]);
        assert_eq!((out.width(), out.height()), (2, 1));
    }

    #[test]
    fn map_pixels_xy_supplies_coordinates() {
        // Writing the coordinates into the output is the most direct possible assertion
        // that each closure call got the right (x, y).
        let img = ImageBuf::new(3, 2);
        let out = img.map_pixels_xy(|x, y, _| [x as f32, y as f32, 0.0]);
        assert_eq!((out.width(), out.height()), (3, 2));
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(out.pixel(x, y), [x as f32, y as f32, 0.0], "at {x},{y}");
            }
        }
    }

    #[test]
    fn map_pixels_xy_can_read_the_source_pixel() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        img.set_pixel(1, 0, [0.4, 0.5, 0.6]);
        let out = img.map_pixels_xy(|x, _, rgb| [rgb[0] + x as f32, rgb[1], rgb[2]]);
        assert_eq!(out.pixel(0, 0), [0.1, 0.2, 0.3]);
        assert_eq!(out.pixel(1, 0), [1.4, 0.5, 0.6]);
    }

    #[test]
    fn map_pixels_xy_handles_a_zero_width_image() {
        // `par_chunks_exact_mut(0)` panics, so the empty case needs its own path.
        let out = ImageBuf::new(0, 0).map_pixels_xy(|_, _, rgb| rgb);
        assert_eq!((out.width(), out.height()), (0, 0));
        assert!(out.data().is_empty());
    }

    #[test]
    fn map_pixels_transforms_per_pixel() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.2, 0.4, 0.6]);
        img.set_pixel(1, 0, [0.1, 0.1, 0.1]);
        let out = img.map_pixels(|[r, g, b]| [b, g * 2.0, r]);
        assert_eq!(out.pixel(0, 0), [0.6, 0.8, 0.2]);
        assert_eq!(out.pixel(1, 0), [0.1, 0.2, 0.1]);
        assert_eq!((out.width(), out.height()), (2, 1));
    }
}

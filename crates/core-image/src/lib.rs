//! Core image container: interleaved f32 RGB, scene-referred linear working space.

/// An interleaved RGB image with `f32` samples in linear light (typically `0.0..=1.0`,
/// but values may exceed 1.0 for scene-referred highlights).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBuf {
    width: usize,
    height: usize,
    /// Interleaved RGB, length = width * height * 3.
    data: Vec<f32>,
}

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
    pub fn map_samples<F: Fn(f32) -> f32>(&self, f: F) -> ImageBuf {
        let data = self.data.iter().map(|&s| f(s)).collect();
        ImageBuf::from_vec(self.width, self.height, data)
    }

    /// Return a new image with `f` applied to each RGB pixel, dimensions preserved.
    pub fn map_pixels<F: Fn([f32; 3]) -> [f32; 3]>(&self, f: F) -> ImageBuf {
        let mut data = Vec::with_capacity(self.data.len());
        for px in self.data.chunks_exact(Self::CHANNELS) {
            let [r, g, b] = f([px[0], px[1], px[2]]);
            data.push(r);
            data.push(g);
            data.push(b);
        }
        ImageBuf::from_vec(self.width, self.height, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

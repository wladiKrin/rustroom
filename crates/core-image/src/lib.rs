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
}

//! Comparing a GPU render against the CPU one.
//!
//! ⚠⚠ **This harness is what protects the shipped path once a GPU render is the default.** The 63
//! goldens keep testing the CPU pipeline byte-for-byte and never move (user's call, 2026-09-10),
//! so from M8.3b onward they no longer test what ships — this does. It therefore has to be as
//! trustworthy as they are, which is why its own tests use awkward inputs rather than pretty ones.

use core_image::ImageBuf;

/// How far two renders of the same input differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divergence {
    /// The largest single-sample difference, in **8-bit display levels**.
    pub worst_levels: f32,
    /// How many samples differ at all.
    pub differing: usize,
    /// How many samples were compared.
    pub total: usize,
}

impl Divergence {
    /// True when the two renders are bit-identical.
    pub fn is_exact(&self) -> bool {
        self.differing == 0 && self.worst_levels == 0.0
    }

    /// The share of samples that differ, as a percentage.
    pub fn differing_percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.differing as f64 / self.total as f64 * 100.0
    }
}

/// Compare two renders of the same input.
///
/// ⚠ **In 8-bit display levels, not raw linear floats.** A linear difference near black reads as
/// enormous while being invisible, and a judgement about a photograph is made in levels — the
/// correction the Whites slider's weights and the M8.4 demosaic comparison each needed.
///
/// ⚠ A size mismatch reports **infinite** divergence rather than comparing what overlaps. Zipping
/// two different lengths silently ignores the tail, so a truncated GPU readback would look like
/// agreement.
pub fn compare(a: &ImageBuf, b: &ImageBuf) -> Divergence {
    if a.width() != b.width() || a.height() != b.height() || a.data().len() != b.data().len() {
        return Divergence {
            worst_levels: f32::INFINITY,
            differing: usize::MAX,
            total: 0,
        };
    }
    let mut worst = 0.0f32;
    let mut differing = 0usize;
    for (x, y) in a.data().iter().zip(b.data()) {
        if x != y {
            differing += 1;
            let d = (color::linear_to_srgb(*x) - color::linear_to_srgb(*y)).abs() * 255.0;
            worst = worst.max(d);
        }
    }
    Divergence {
        worst_levels: worst,
        differing,
        total: a.data().len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(data: &[f32]) -> ImageBuf {
        ImageBuf::from_vec(data.len() / 3, 1, data.to_vec())
    }

    #[test]
    fn identical_images_diverge_by_nothing() {
        let a = img(&[0.1, 0.2, 0.3, 0.9, 0.5, 0.4]);
        let d = compare(&a, &a);
        assert_eq!(d.worst_levels, 0.0);
        assert_eq!(d.differing, 0);
        assert!(d.is_exact());
    }

    #[test]
    fn the_difference_is_reported_in_eight_bit_levels() {
        // ⚠⚠ **Levels, not raw floats.** "Half a level" is the unit a judgement about a
        // photograph is made in, and a linear difference near black reads as enormous while being
        // invisible — the reasoning the Whites slider's weights and the demosaic comparison were
        // each corrected under. 4/255 apart in the encoded domain must read as ~4.
        let a = img(&[0.0, 0.0, 0.0]);
        let b = img(&[color::srgb_to_linear(4.0 / 255.0), 0.0, 0.0]);
        let d = compare(&a, &b);
        assert!(
            (d.worst_levels - 4.0).abs() < 0.5,
            "expected about 4 levels, got {}",
            d.worst_levels
        );
        assert_eq!(d.differing, 1);
    }

    #[test]
    fn a_size_mismatch_is_reported_rather_than_compared() {
        // ⚠ Comparing buffers of different lengths by zipping them would silently ignore the
        // tail, so a truncated GPU readback would look like agreement.
        let a = img(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        let b = img(&[0.1, 0.2, 0.3]);
        let d = compare(&a, &b);
        assert!(!d.is_exact(), "different sizes can never be exact");
        assert!(
            d.worst_levels.is_infinite(),
            "and the divergence must be unmistakable"
        );
    }

    #[test]
    fn a_tiny_linear_difference_near_black_is_reported_as_large() {
        // ⚠⚠ The half that proves the encoding is doing work rather than being decoration. Near
        // black the sRGB curve is steep, so a linear difference far too small to matter in the
        // highlights is several levels here — which is exactly the asymmetry a raw-float
        // comparison would hide, and the reason this harness converts at all.
        let a = img(&[0.0, 0.0, 0.0]);
        let b = img(&[0.001, 0.0, 0.0]);
        let d = compare(&a, &b);
        assert!(
            d.worst_levels > 3.0,
            "0.001 in linear light is several levels near black, got {}",
            d.worst_levels
        );
    }
}

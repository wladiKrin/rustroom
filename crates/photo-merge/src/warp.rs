//! Reading a frame at a fractional position — its normalised raw signal, Catmull-Rom.
//!
//! ⚠ The kernel's weights are `pipeline::resample`'s own table, never a second construction of it.

use crate::frame::Frame;
use crate::lens::FrameLens;
use pipeline::resample::Weights;
use pipeline::Resampler;

const TAPS: usize = 4;

/// Catmull-Rom over a [`Frame`].
pub struct Sampler {
    lut: Vec<f32>,
    phases: usize,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let w = Weights::new(Resampler::CatmullRom);
        assert_eq!(w.taps(), TAPS, "Catmull-Rom is four taps");
        Self {
            lut: w.lut().to_vec(),
            phases: w.phases(),
        }
    }

    #[inline]
    fn weights(&self, f: f32) -> &[f32] {
        let p = ((f * self.phases as f32) as usize).min(self.phases - 1);
        &self.lut[p * TAPS..p * TAPS + TAPS]
    }

    /// The normalised raw signal at `(x, y)` — `None` when a tap falls outside the frame or on a
    /// clipped sample, so a clipped highlight never bleeds into its neighbours as a valid value.
    pub fn raw_at(&self, f: &Frame, x: f32, y: f32) -> Option<[f32; 3]> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let (bx, by) = (x.floor(), y.floor());
        let (x0, y0) = (bx as i64 - 1, by as i64 - 1);
        if x0 < 0 || y0 < 0 || x0 + 3 >= f.width as i64 || y0 + 3 >= f.height as i64 {
            return None;
        }
        let (wx, wy) = (self.weights(x - bx), self.weights(y - by));
        let mut out = [0f32; 3];
        for (j, wyj) in wy.iter().enumerate() {
            let sy = (y0 + j as i64) as usize;
            let mut row = [0f32; 3];
            for (i, wxi) in wx.iter().enumerate() {
                if *wxi == 0.0 || *wyj == 0.0 {
                    continue;
                }
                let sx = (x0 + i as i64) as usize;
                if f.clipped(sx, sy) {
                    return None;
                }
                let p = f.raw(sx, sy);
                for c in 0..3 {
                    row[c] += wxi * p[c];
                }
            }
            for c in 0..3 {
                out[c] += wyj * row[c];
            }
        }
        Some(out)
    }
}

impl Sampler {
    /// The normalised raw signal at corrected pixel `(x, y)` of `f`, through `lens`: each channel read
    /// where the lens puts it, times the vignetting gain — and whether any tap read a clipped sample.
    /// `None` only outside the frame. [`FrameLens::NoProfile`] and [`FrameLens::NoLens`] read `(x, y)`.
    pub fn raw_at_corrected(
        &self,
        f: &Frame,
        lens: &FrameLens,
        x: f32,
        y: f32,
    ) -> Option<([f32; 3], bool)> {
        let Some((src, gain)) = lens.at(x, y) else {
            return self.channels_at(f, [[x, y]; 3]);
        };
        let (mut v, clipped) = self.channels_at(f, src)?;
        for c in v.iter_mut() {
            *c *= gain;
        }
        Some((v, clipped))
    }

    /// Each channel `c` read at `at[c]`, and whether any tap of any channel was clipped.
    fn channels_at(&self, f: &Frame, at: [[f32; 2]; 3]) -> Option<([f32; 3], bool)> {
        let mut out = [0f32; 3];
        let mut clipped = false;
        for (c, [x, y]) in at.into_iter().enumerate() {
            if !(x.is_finite() && y.is_finite()) {
                return None;
            }
            let (bx, by) = (x.floor(), y.floor());
            let (x0, y0) = (bx as i64 - 1, by as i64 - 1);
            if x0 < 0 || y0 < 0 || x0 + 3 >= f.width as i64 || y0 + 3 >= f.height as i64 {
                return None;
            }
            let (wx, wy) = (self.weights(x - bx), self.weights(y - by));
            for (j, wyj) in wy.iter().enumerate() {
                if *wyj == 0.0 {
                    continue;
                }
                let sy = (y0 + j as i64) as usize;
                let mut row = 0f32;
                for (i, wxi) in wx.iter().enumerate() {
                    if *wxi == 0.0 {
                        continue;
                    }
                    let sx = (x0 + i as i64) as usize;
                    clipped |= f.clipped(sx, sy);
                    row += wxi * f.raw(sx, sy)[c];
                }
                out[c] += wyj * row;
            }
        }
        Some((out, clipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    fn ramp(w: usize, h: usize) -> Frame {
        let px: Vec<u16> = (0..w * h)
            .flat_map(|i| {
                let v = (1000 + 300 * (i % w) + 50 * (i / w)) as u16;
                [v, v / 2, v / 3]
            })
            .collect();
        Frame::new(w, h, px, [1.0, 1.0, 1.0], None)
    }

    #[test]
    fn the_sampler_reproduces_a_sample_at_integer_positions() {
        let f = ramp(12, 9);
        let s = Sampler::new();
        for (x, y) in [(2, 2), (5, 4), (9, 6)] {
            let got = s.raw_at(&f, x as f32, y as f32).expect("inside");
            assert_eq!(got, f.raw(x, y), "({x}, {y})");
        }
    }

    /// Between samples on a linear ramp, Catmull-Rom is exact.
    #[test]
    fn the_sampler_interpolates_a_ramp() {
        let f = ramp(12, 9);
        let got = Sampler::new().raw_at(&f, 4.5, 3.0).unwrap();
        let (a, b) = (f.raw(4, 3), f.raw(5, 3));
        for c in 0..3 {
            assert!((got[c] - (a[c] + b[c]) / 2.0).abs() < 1e-5, "{got:?}");
        }
    }

    #[test]
    fn a_tap_on_a_clipped_sample_gives_none() {
        let mut px: Vec<u16> = vec![1000; 12 * 9 * 3];
        px[(4 * 12 + 6) * 3] = 65535;
        let f = Frame::new(12, 9, px, [1.0; 3], None);
        let s = Sampler::new();
        assert!(s.raw_at(&f, 5.5, 4.0).is_none(), "a tap reads (6, 4)");
        assert!(s.raw_at(&f, 2.0, 2.0).is_some());
    }

    /// At a whole pixel the outer taps weigh nothing, so a clipped neighbour there is not read.
    #[test]
    fn a_clipped_sample_under_a_zero_weight_does_not_count() {
        let mut px: Vec<u16> = vec![1000; 12 * 9 * 3];
        px[(4 * 12 + 6) * 3] = 65535;
        let f = Frame::new(12, 9, px, [1.0; 3], None);
        assert!(Sampler::new().raw_at(&f, 5.0, 4.0).is_some());
    }

    #[test]
    fn outside_the_frame_gives_none() {
        let f = ramp(12, 9);
        let s = Sampler::new();
        assert!(s.raw_at(&f, 0.5, 4.0).is_none(), "a tap at x = -1");
        assert!(s.raw_at(&f, -3.0, 4.0).is_none());
        assert!(s.raw_at(&f, 5.0, 8.5).is_none());
    }
}

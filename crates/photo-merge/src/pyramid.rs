//! Log luminance in a pyramid, with what is valid to compare — what [`crate::align`] registers.
//!
//! **Log** luminance because there an exposure difference is a constant: a frame two stops darker
//! reads `log2(L) − 2` everywhere it holds information, so one offset absorbs it. Valid means not
//! clipped and above [`FLOOR`] — below it a 14-bit sensor's noise is most of the signal.

use crate::frame::Frame;
use rayon::prelude::*;

/// The darkest luminance compared, ~10 stops under white.
pub const FLOOR: f32 = 1.0 / 1024.0;

/// One level: `w × h`, row-major.
pub struct Level {
    pub w: usize,
    pub h: usize,
    /// `log2(max(luma, FLOOR))`.
    pub l: Vec<f32>,
    pub valid: Vec<bool>,
}

/// Level 0 is the frame; each next level is a 2 × 2 average, down to the first at most `coarsest`
/// px on its long edge. A block holding an invalid pixel is invalid.
pub struct LumPyramid {
    levels: Vec<Level>,
}

impl LumPyramid {
    pub fn build(f: &Frame, coarsest: usize) -> Self {
        let (w, h) = (f.width, f.height);
        let rows: Vec<(Vec<f32>, Vec<bool>)> = (0..h)
            .into_par_iter()
            .map(|y| {
                let lin: Vec<f32> = (0..w).map(|x| f.luma(x, y)).collect();
                let valid = (0..w).map(|x| !f.clipped(x, y) && lin[x] > FLOOR).collect();
                (lin, valid)
            })
            .collect();
        let mut lin: Vec<f32> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
        let mut valid: Vec<bool> = rows.iter().flat_map(|r| r.1.iter().copied()).collect();
        let (mut cw, mut ch) = (w, h);
        let mut levels = vec![Level {
            w,
            h,
            l: logs(&lin),
            valid: valid.clone(),
        }];
        while cw.max(ch) > coarsest && cw >= 2 && ch >= 2 {
            let (nw, nh) = (cw / 2, ch / 2);
            let mut nl = vec![0f32; nw * nh];
            let mut nv = vec![false; nw * nh];
            for y in 0..nh {
                for x in 0..nw {
                    let idx = [
                        2 * y * cw + 2 * x,
                        2 * y * cw + 2 * x + 1,
                        (2 * y + 1) * cw + 2 * x,
                        (2 * y + 1) * cw + 2 * x + 1,
                    ];
                    nl[y * nw + x] = idx.iter().map(|&i| lin[i]).sum::<f32>() * 0.25;
                    nv[y * nw + x] = idx.iter().all(|&i| valid[i]);
                }
            }
            levels.push(Level {
                w: nw,
                h: nh,
                l: logs(&nl),
                valid: nv.clone(),
            });
            (lin, valid, cw, ch) = (nl, nv, nw, nh);
        }
        Self { levels }
    }

    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    pub fn level(&self, i: usize) -> &Level {
        &self.levels[i]
    }
}

fn logs(lin: &[f32]) -> Vec<f32> {
    lin.iter().map(|v| v.max(FLOOR).log2()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    #[test]
    fn the_pyramid_stops_at_the_coarsest_size() {
        let f = Frame::new(600, 400, vec![20000; 600 * 400 * 3], [1.0; 3], None);
        let p = LumPyramid::build(&f, 128);
        let top = p.level(p.levels() - 1);
        assert!(top.w.max(top.h) <= 128, "{}×{}", top.w, top.h);
        assert!(p.level(p.levels() - 2).w.max(p.level(p.levels() - 2).h) > 128);
        assert_eq!((p.level(0).w, p.level(0).h), (600, 400));
    }

    /// Clipped and black pixels are not compared, and a block holding one is invalid above.
    #[test]
    fn clipped_and_black_pixels_are_invalid() {
        let mut px = vec![20000u16; 8 * 8 * 3];
        px[..3].copy_from_slice(&[65535, 20000, 20000]);
        px[(5 * 8 + 5) * 3..(5 * 8 + 5) * 3 + 3].copy_from_slice(&[0, 0, 0]);
        let p = LumPyramid::build(&Frame::new(8, 8, px, [1.0; 3], None), 2);
        let (l0, l1) = (p.level(0), p.level(1));
        assert!(!l0.valid[0] && !l0.valid[5 * 8 + 5] && l0.valid[1]);
        assert!(!l1.valid[0] && !l1.valid[2 * 4 + 2] && l1.valid[1]);
    }
}

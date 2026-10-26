//! Deghost: what moved between frames is taken from one frame alone.

/// A plane over the reference frame, at its own (smaller) size — a ghost mask, 0–1.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Mask {
    /// The value at normalised `(u, v)` ∈ [0, 1]², bilinear between cell centres.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy)
            + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy
    }
}

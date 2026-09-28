//! A network's answer: a probability per pixel, 8-bit, over the photo it was asked about.

/// A probability map, 0–255, row-major, `w × h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbMap {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

const MAGIC: &[u8; 4] = b"RRAI";

impl ProbMap {
    pub fn from_fn(w: u32, h: u32, f: impl Fn(u32, u32) -> f32) -> Self {
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                data.push((f(x, y).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        Self { w, h, data }
    }

    /// The probability at normalised `(u, v)`, bilinear, clamped at the edges.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        if self.w == 0 || self.h == 0 || !u.is_finite() || !v.is_finite() {
            return 0.0;
        }
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: u32, y: u32| self.data[(y * self.w + x) as usize] as f32 / 255.0;
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bot = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bot * fy
    }

    /// Undo `q` clockwise quarter turns — the model saw the photo upright, the map is stored in the
    /// decode's own (sensor) orientation.
    pub fn rotate_back(&self, q: u8) -> Self {
        let mut m = self.clone();
        for _ in 0..(4 - q % 4) % 4 {
            m = m.rotate_cw();
        }
        m
    }

    fn rotate_cw(&self) -> Self {
        let (w, h) = (self.h, self.w);
        let mut data = vec![0u8; self.data.len()];
        for y in 0..self.h {
            for x in 0..self.w {
                // (x, y) → (h − 1 − y, x)
                let (nx, ny) = (self.h - 1 - y, x);
                data[(ny * w + nx) as usize] = self.data[(y * self.w + x) as usize];
            }
        }
        Self { w, h, data }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend(self.w.to_le_bytes());
        out.extend(self.h.to_le_bytes());
        out.extend(&self.data);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 12 || &b[..4] != MAGIC {
            return None;
        }
        let w = u32::from_le_bytes(b[4..8].try_into().ok()?);
        let h = u32::from_le_bytes(b[8..12].try_into().ok()?);
        let data = b[12..].to_vec();
        (data.len() as u64 == u64::from(w) * u64::from(h)).then_some(Self { w, h, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_round_trips_and_rejects_a_truncated_file() {
        let m = ProbMap::from_fn(3, 2, |x, y| (x + y) as f32 / 3.0);
        assert_eq!(ProbMap::from_bytes(&m.to_bytes()), Some(m.clone()));
        let b = m.to_bytes();
        assert_eq!(ProbMap::from_bytes(&b[..b.len() - 1]), None);
        assert_eq!(ProbMap::from_bytes(b"nope"), None);
    }

    #[test]
    fn rotating_back_undoes_the_upright_turn() {
        // A 3×2 map with one marked pixel at (2, 0).
        let m = ProbMap::from_fn(3, 2, |x, y| if (x, y) == (2, 0) { 1.0 } else { 0.0 });
        let upright = m.rotate_cw(); // what the model saw after one clockwise turn
        assert_eq!((upright.w, upright.h), (2, 3));
        assert_eq!(upright.rotate_back(1), m);
        assert_eq!(m.rotate_back(0), m);
        assert_eq!(m.rotate_cw().rotate_cw().rotate_back(2), m);
    }

    #[test]
    fn sampling_is_bilinear_and_clamped() {
        let m = ProbMap::from_fn(2, 1, |x, _| x as f32);
        assert!((m.at(0.5, 0.5) - 0.5).abs() < 0.01);
        assert_eq!(m.at(0.0, 0.5), 0.0);
        assert_eq!(m.at(1.0, 0.5), 1.0);
        assert_eq!(m.at(f32::NAN, 0.5), 0.0);
    }
}

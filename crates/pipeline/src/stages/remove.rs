//! Content-aware removal (M10.16c1): each removal's fill pasted over the decode — the prefix's step
//! right after AI denoise. The filling itself ran once, in the background (`rustroom_app::remove_tool`);
//! this only pastes.

use crate::PipelineStage;
use core_image::ImageBuf;
use rayon::prelude::*;
use std::sync::Arc;

/// One removal's fill: a patch over the decode, at whatever resolution it was computed at.
///
/// ⚠ `rect` is normalised over the **decode** (`[x0, y0, x1, y1]`), so one fill serves every preview rung
/// and the export: it is resampled to the frame it lands on. `rgb` is in the decode's own (linear) units.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub rect: [f32; 4],
    pub w: usize,
    pub h: usize,
    /// `w × h`, interleaved.
    pub rgb: Vec<f32>,
    /// `w × h`, 0..=1 — how much of the fill covers each pixel (the stroke's soft edge).
    pub alpha: Vec<f32>,
}

impl Fill {
    /// Whether its buffers match its size and its rect is not empty.
    fn is_sound(&self) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        self.w > 0
            && self.h > 0
            && self.rgb.len() == self.w * self.h * 3
            && self.alpha.len() == self.w * self.h
            && x1 > x0
            && y1 > y0
    }

    /// RGB and alpha at fill pixel `(fx, fy)` (pixel centres at integers), bilinear, clamped.
    fn sample(&self, fx: f32, fy: f32) -> ([f32; 3], f32) {
        let fx = fx.clamp(0.0, (self.w - 1) as f32);
        let fy = fy.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let wts = [
            (x0, y0, (1.0 - tx) * (1.0 - ty)),
            (x1, y0, tx * (1.0 - ty)),
            (x0, y1, (1.0 - tx) * ty),
            (x1, y1, tx * ty),
        ];
        let (mut rgb, mut a) = ([0f32; 3], 0f32);
        for (x, y, k) in wts {
            let i = y * self.w + x;
            for (c, v) in rgb.iter_mut().enumerate() {
                *v += self.rgb[i * 3 + c] * k;
            }
            a += self.alpha[i] * k;
        }
        (rgb, a)
    }
}

/// The removals that have a fill, in order, each with its Opacity (0..=1).
pub struct RemoveStage {
    fills: Vec<(Arc<Fill>, f32)>,
}

impl RemoveStage {
    pub fn new(fills: Vec<(Arc<Fill>, f32)>) -> Self {
        Self { fills }
    }
}

impl PipelineStage for RemoveStage {
    fn name(&self) -> &'static str {
        "remove"
    }

    /// `photo + (fill − photo) · alpha · opacity` over each fill's rect, the next fill over the last.
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let mut out = image;
        for (fill, opacity) in &self.fills {
            paste(&mut out, fill, *opacity);
        }
        out
    }
}

/// One fill over `img`, in place: every frame pixel whose centre lies in the rect, sampled where it falls
/// in the fill. Rows in parallel.
fn paste(img: &mut ImageBuf, f: &Fill, opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity == 0.0 || !f.is_sound() {
        return;
    }
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return;
    }
    let [x0, y0, x1, y1] = f.rect;
    // The pixels whose centre `(i + 0.5) / n` lies in `[lo, hi)` — one wider each way, then the exact test.
    let span = |lo: f32, hi: f32, n: usize| {
        let a = ((lo * n as f32 - 1.5).floor().max(0.0) as usize).min(n);
        let b = ((hi * n as f32 + 1.5).ceil().max(0.0) as usize).min(n);
        (a..b).filter(move |&i| {
            let c = (i as f32 + 0.5) / n as f32;
            c >= lo && c < hi
        })
    };
    let cols: Vec<usize> = span(x0, x1, w).collect();
    let rows: Vec<usize> = span(y0, y1, h).collect();
    let (Some(&first), Some(&last)) = (rows.first(), rows.last()) else {
        return;
    };
    let to_fill = |c: f32, lo: f32, hi: f32, n: usize| (c - lo) / (hi - lo) * n as f32 - 0.5;
    img.data_mut()
        .par_chunks_mut(w * 3)
        .enumerate()
        .skip(first)
        .take(last + 1 - first)
        .filter(|(y, _)| rows.binary_search(y).is_ok())
        .for_each(|(y, row)| {
            let fy = to_fill((y as f32 + 0.5) / h as f32, y0, y1, f.h);
            for &x in &cols {
                let fx = to_fill((x as f32 + 0.5) / w as f32, x0, x1, f.w);
                let (rgb, a) = f.sample(fx, fy);
                let t = a * opacity;
                if t <= 0.0 {
                    continue;
                }
                let p = &mut row[x * 3..x * 3 + 3];
                for c in 0..3 {
                    p[c] += (rgb[c] - p[c]) * t;
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineStage;
    use core_image::ImageBuf;
    use std::sync::Arc;

    /// A 40 × 30 frame, every value its own.
    fn frame() -> ImageBuf {
        ImageBuf::from_vec(
            40,
            30,
            (0..40 * 30 * 3).map(|i| (i % 97) as f32 / 97.0).collect(),
        )
    }

    /// A 4 × 4 fill of one colour and one alpha over the frame's middle half.
    fn fill(rgb: f32, alpha: f32) -> Arc<Fill> {
        Arc::new(Fill {
            rect: [0.25, 0.2, 0.75, 0.8],
            w: 4,
            h: 4,
            rgb: vec![rgb; 4 * 4 * 3],
            alpha: vec![alpha; 4 * 4],
        })
    }

    /// Inside the rect at alpha 1 and Opacity 0.5 the midpoint of photo and fill, exactly; outside the
    /// rect untouched, exactly; at alpha 0 nothing anywhere.
    #[test]
    fn a_fill_pastes_inside_its_alpha_at_its_opacity() {
        let src = frame();
        let out = RemoveStage::new(vec![(fill(1.0, 1.0), 0.5)]).apply(src.clone());
        let (mut inside, mut outside) = (0, 0);
        for y in 0..30 {
            for x in 0..40 {
                let (s, o) = (src.pixel(x, y), out.pixel(x, y));
                let (u, v) = ((x as f32 + 0.5) / 40.0, (y as f32 + 0.5) / 30.0);
                if (0.25..0.75).contains(&u) && (0.2..0.8).contains(&v) {
                    inside += 1;
                    for c in 0..3 {
                        assert!(
                            (o[c] - (s[c] + 1.0) / 2.0).abs() < 1e-6,
                            "({x}, {y}) {c}: {} from {}",
                            o[c],
                            s[c]
                        );
                    }
                } else {
                    outside += 1;
                    assert_eq!(o, s, "({x}, {y}) outside the rect moved");
                }
            }
        }
        assert_eq!((inside, outside), (20 * 18, 40 * 30 - 20 * 18));
        let none = RemoveStage::new(vec![(fill(1.0, 0.0), 1.0)]).apply(src.clone());
        assert_eq!(none.data(), src.data(), "alpha 0 pastes nothing");
    }

    /// The fill is sampled where it lies: a fill whose columns rise left to right lands rising, and its
    /// rect's edges sit where they were measured — at any frame size (a preview rung or the export).
    #[test]
    fn a_fill_lands_where_its_rect_says_at_any_size() {
        let ramp = Arc::new(Fill {
            rect: [0.5, 0.0, 1.0, 1.0],
            w: 8,
            h: 2,
            rgb: (0..8 * 2).flat_map(|i| [(i % 8) as f32 / 7.0; 3]).collect(),
            alpha: vec![1.0; 16],
        });
        for (w, h) in [(16, 4), (64, 9)] {
            let out = RemoveStage::new(vec![(ramp.clone(), 1.0)]).apply(ImageBuf::from_vec(
                w,
                h,
                vec![-1.0; w * h * 3],
            ));
            let row: Vec<f32> = (0..w).map(|x| out.pixel(x, h / 2)[0]).collect();
            assert!(row[..w / 2].iter().all(|&v| v == -1.0), "{row:?}");
            assert!(
                row[w / 2..].windows(2).all(|p| p[1] >= p[0]),
                "{row:?} does not rise"
            );
            assert!(row[w / 2] < 0.2 && row[w - 1] > 0.8, "{row:?}");
        }
    }
}

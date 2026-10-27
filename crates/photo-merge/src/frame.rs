//! One photo of a bracket, as libraw decoded it (`raw_decode::decode_camera`): demosaiced, in the
//! camera's own space, white-balanced by libraw's multipliers, **16-bit** — half an `f32` frame,
//! because a merge may hold nine of them (156 MB each at 26 Mpx).

use std::sync::Arc;

/// The share of libraw's 65535 ceiling at which a channel counts as clipped. libraw's highlight mode
/// "clip" saturates a channel at 65535 after its multiplier, so a sample this close to it holds no
/// information about the light — the frame gives that pixel up and a darker frame takes it.
pub const CLIP: f32 = 0.98;

const CLIP_U16: u16 = (CLIP * 65535.0) as u16;

/// One frame of a bracket.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Interleaved RGB, libraw's camera space, white-balanced (`raw × pre_mul`, clipped at 65535).
    pub samples: Arc<Vec<u16>>,
    /// libraw's applied multipliers, the smallest 1.
    pub pre_mul: [f32; 3],
    /// Shutter × ISO ÷ N², in stops, when the file says — see [`exposure_ev`].
    pub ev: Option<f32>,
}

impl Frame {
    pub fn new(
        width: usize,
        height: usize,
        samples: Vec<u16>,
        pre_mul: [f32; 3],
        ev: Option<f32>,
    ) -> Self {
        assert_eq!(samples.len(), width * height * 3, "a frame's samples");
        Self {
            width,
            height,
            samples: Arc::new(samples),
            pre_mul,
            ev,
        }
    }

    /// A frame from libraw's camera decode, its samples copied.
    pub fn from_camera(c: &raw_decode::CameraFrame) -> Self {
        Self::new(
            c.width,
            c.height,
            c.samples.clone(),
            c.pre_mul,
            exposure_ev(&c.capture),
        )
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> &[u16] {
        let i = (y * self.width + x) * 3;
        &self.samples[i..i + 3]
    }

    /// Whether any channel of the pixel is at libraw's ceiling.
    #[inline]
    pub fn clipped(&self, x: usize, y: usize) -> bool {
        self.at(x, y).iter().any(|&s| s >= CLIP_U16)
    }

    /// The normalised raw signal: the white balance divided back out, `(raw − black) / (white −
    /// black)` per channel.
    #[inline]
    pub fn raw(&self, x: usize, y: usize) -> [f32; 3] {
        let s = self.at(x, y);
        [0, 1, 2].map(|c| s[c] as f32 / 65535.0 / self.pre_mul[c])
    }

    /// Brightness as the frame shows it (white-balanced), `0.25 R + 0.5 G + 0.25 B`, 0–1.
    #[inline]
    pub fn luma(&self, x: usize, y: usize) -> f32 {
        let s = self.at(x, y);
        (0.25 * s[0] as f32 + 0.5 * s[1] as f32 + 0.25 * s[2] as f32) / 65535.0
    }

    /// Half the size by 2 × 2 averages; a block with one clipped sample is clipped whole.
    pub fn half(&self) -> Frame {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut out = vec![0u16; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 3];
                let mut clipped = false;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (
                        (2 * x + dx).min(self.width - 1),
                        (2 * y + dy).min(self.height - 1),
                    );
                    clipped |= self.clipped(sx, sy);
                    for (c, v) in self.at(sx, sy).iter().enumerate() {
                        sum[c] += *v as u32;
                    }
                }
                let o = &mut out[(y * w + x) * 3..(y * w + x) * 3 + 3];
                for c in 0..3 {
                    o[c] = if clipped {
                        u16::MAX
                    } else {
                        ((sum[c] + 2) / 4) as u16
                    };
                }
            }
        }
        Frame::new(w, h, out, self.pre_mul, self.ev)
    }
}

/// How much light a frame let in, in stops: `log2(shutter × ISO ÷ N²)` — bigger is brighter.
/// `None` when the file does not record all three.
pub fn exposure_ev(c: &core_image::Capture) -> Option<f32> {
    let (s, iso, n) = (c.shutter?, c.iso? as f32, c.aperture?);
    let v = (s * iso / (n * n)).log2();
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(px: &[[u16; 3]], w: usize, pre_mul: [f32; 3]) -> Frame {
        Frame::new(
            w,
            px.len() / w,
            px.iter().flatten().copied().collect(),
            pre_mul,
            None,
        )
    }

    /// A sample is libraw's `raw × pre_mul`; dividing it back gives the raw signal whatever the
    /// frame's white balance — which is what makes two frames comparable.
    #[test]
    fn a_frame_divides_out_its_multipliers() {
        let pre = [2.0, 1.0, 1.5];
        let s = |v: f32, m: f32| (v * m * 65535.0).round() as u16;
        let f = frame(&[[s(0.25, 2.0), s(0.25, 1.0), s(0.25, 1.5)]], 1, pre);
        for v in f.raw(0, 0) {
            assert!((v - 0.25).abs() < 1e-4, "{:?}", f.raw(0, 0));
        }
        assert!(!f.clipped(0, 0));
    }

    /// A channel at libraw's ceiling is clipped, and one clipped sample clips its pixel.
    #[test]
    fn a_channel_at_the_ceiling_clips_the_pixel() {
        let f = frame(&[[1000, 65535, 1000], [1000, 1000, 1000]], 2, [1.0; 3]);
        assert!(f.clipped(0, 0));
        assert!(!f.clipped(1, 0));
    }

    #[test]
    fn half_marks_a_block_with_one_clipped_sample_clipped() {
        let mut px = vec![[1000u16, 2000, 3000]; 16];
        px[5] = [65535, 2000, 3000];
        let f = frame(&px, 4, [1.0; 3]);
        let h = f.half();
        assert_eq!((h.width, h.height), (2, 2));
        assert!(h.clipped(0, 0), "the block holding (1, 1) is clipped");
        assert!(!h.clipped(1, 1));
        assert_eq!(h.raw(1, 1), f.raw(3, 3));
    }

    /// Shutter × ISO ÷ N², in stops: a stop more light per doubling of the shutter.
    #[test]
    fn exposure_doubles_with_the_shutter() {
        let c = |s: f32| core_image::Capture {
            shutter: Some(s),
            iso: Some(200),
            aperture: Some(8.0),
            ..Default::default()
        };
        let (a, b) = (exposure_ev(&c(1.0 / 125.0)), exposure_ev(&c(1.0 / 62.5)));
        assert!((b.unwrap() - a.unwrap() - 1.0).abs() < 1e-5);
        assert_eq!(exposure_ev(&core_image::Capture::default()), None);
    }
}

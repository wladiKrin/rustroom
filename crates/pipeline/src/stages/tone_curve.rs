use crate::PipelineStage;
use core_image::ImageBuf;

const LUT_SIZE: usize = 1024;

/// Point tone curve compiled to a LUT. Points are (x, y) in [0, 1].
pub struct ToneCurveStage {
    lut: Vec<f32>,
}

impl ToneCurveStage {
    /// Build from control points. Fewer than 2 points => identity.
    pub fn new(points: &[[f32; 2]]) -> Self {
        let mut pts: Vec<[f32; 2]> = points
            .iter()
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());

        let lut = (0..LUT_SIZE)
            .map(|i| {
                let x = i as f32 / (LUT_SIZE - 1) as f32;
                Self::eval(&pts, x)
            })
            .collect();
        Self { lut }
    }

    /// Piecewise-linear evaluation; identity if fewer than 2 points.
    fn eval(pts: &[[f32; 2]], x: f32) -> f32 {
        if pts.len() < 2 {
            return x;
        }
        if x <= pts[0][0] {
            return pts[0][1];
        }
        if x >= pts[pts.len() - 1][0] {
            return pts[pts.len() - 1][1];
        }
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if x >= a[0] && x <= b[0] {
                let span = b[0] - a[0];
                let t = if span > 1e-9 { (x - a[0]) / span } else { 0.0 };
                return a[1] + t * (b[1] - a[1]);
            }
        }
        x
    }

    fn sample(&self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        let pos = v * (LUT_SIZE - 1) as f32;
        let i = pos.floor() as usize;
        if i >= LUT_SIZE - 1 {
            return self.lut[LUT_SIZE - 1];
        }
        let frac = pos - i as f32;
        self.lut[i] + frac * (self.lut[i + 1] - self.lut[i])
    }
}

impl PipelineStage for ToneCurveStage {
    fn name(&self) -> &'static str {
        "tone_curve"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_samples(|s| self.sample(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_curve_passes_through() {
        let s = ToneCurveStage::new(&[[0.0, 0.0], [1.0, 1.0]]);
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((s.sample(v) - v).abs() < 2e-3);
        }
    }

    #[test]
    fn empty_points_is_identity() {
        let s = ToneCurveStage::new(&[]);
        assert!((s.sample(0.37) - 0.37).abs() < 2e-3);
    }

    #[test]
    fn lift_midpoint_brightens_mids() {
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!((s.sample(0.5) - 0.7).abs() < 2e-3);
        assert!(s.sample(0.25) > 0.25);
    }
}

use crate::spline::Spline;
use crate::PipelineStage;
use core_image::ImageBuf;

const LUT_SIZE: usize = 1024;

/// Point tone curve compiled to a LUT. Points are (x, y) in [0, 1].
pub struct ToneCurveStage {
    lut: Vec<f32>,
    /// Per-channel tables, composed **after** [`Self::lut`].
    ///
    /// ⚠⚠ `None` when every channel curve is empty, which keeps [`Self::apply`] on the cheap
    /// `map_samples` path — the common case by far, and byte-identical to before this existed.
    channels: Option<[Vec<f32>; 3]>,
}

/// One interpolation, shared by the composite and the channels.
///
/// ⚠⚠ **Factored rather than written twice**, so the composite and a channel cannot round
/// differently — and so the GPU shader has one function to mirror rather than two. The clamp is on
/// the **input**: a scene-referred highlight above 1.0 is pulled to the table's top entry, which is
/// this stage's long-standing behaviour and the opposite of the rule `ProfileHueSatMap`'s `v` follows.
fn sample_table(lut: &[f32], v: f32) -> f32 {
    let last = lut.len() - 1;
    let v = v.clamp(0.0, 1.0);
    let pos = v * last as f32;
    let i = pos.floor() as usize;
    if i >= last {
        return lut[last];
    }
    let frac = pos - i as f32;
    lut[i] + frac * (lut[i + 1] - lut[i])
}

/// A table for `points`, or the identity when they are empty.
fn build_lut(points: &[[f32; 2]]) -> Vec<f32> {
    let spline = Spline::new(points);
    (0..LUT_SIZE)
        .map(|i| spline.eval(i as f32 / (LUT_SIZE - 1) as f32))
        .collect()
}

impl ToneCurveStage {
    /// Build from control points. Fewer than 2 points => identity.
    pub fn new(points: &[[f32; 2]]) -> Self {
        Self {
            lut: build_lut(points),
            channels: None,
        }
    }

    /// Add per-channel curves, composed **after** the composite.
    ///
    /// ⚠ Empty points for a channel mean identity — the rule the composite already follows — and all
    /// three empty leaves the stage exactly as [`Self::new`] built it, on the cheap path.
    ///
    /// ⚠⚠ **Composite first, then the channel.** A choice rather than a verified standard: the
    /// composite is the overall *contrast* shaper and the channels are the *tint*, so "shape the tone,
    /// then tint it" keeps a channel's effect predictable. The other order re-maps a tint through the
    /// composite's black and white points, pulling a lifted blue shadow back. ⚠ The GPU shader mirrors
    /// this order; changing it means changing both.
    pub fn with_channels(
        mut self,
        red: &[[f32; 2]],
        green: &[[f32; 2]],
        blue: &[[f32; 2]],
    ) -> Self {
        if red.is_empty() && green.is_empty() && blue.is_empty() {
            return self;
        }
        self.channels = Some([build_lut(red), build_lut(green), build_lut(blue)]);
        self
    }

    /// The four tables, composite first — what `gpu-pipeline` uploads.
    ///
    /// ⚠⚠ Taken from here rather than rebuilt there, the rule [`Self::lut`] already states: two
    /// constructions of one curve is how a preview stops being evidence about the render. ⚠ An
    /// identity table is returned for a channel with no curve, because the shader is unconditional.
    pub fn luts(&self) -> [Vec<f32>; 4] {
        let identity: Vec<f32> = build_lut(&[]);
        match &self.channels {
            None => [
                self.lut.clone(),
                identity.clone(),
                identity.clone(),
                identity,
            ],
            Some(ch) => [
                self.lut.clone(),
                ch[0].clone(),
                ch[1].clone(),
                ch[2].clone(),
            ],
        }
    }

    /// The curve's output for one input sample, straight out of the LUT.
    ///
    /// ⚠ Public so the panel's curve widget can *plot the function this stage applies* rather
    /// than re-deriving it — two implementations of one curve is how a preview stops being
    /// evidence about the render.
    /// The compiled curve, for a consumer that samples it itself.
    ///
    /// ⚠⚠ **`gpu-pipeline` uploads THIS table rather than rebuilding one**, the rule
    /// [`crate::ColorGradingStage::lut`] and `HueTable::entries` already follow: two constructions
    /// of one curve is how a preview stops being evidence about the render.
    pub fn lut(&self) -> &[f32] {
        &self.lut
    }

    /// Entries in [`Self::lut`].
    pub const LUT_LEN: usize = LUT_SIZE;

    pub fn sample(&self, v: f32) -> f32 {
        sample_table(&self.lut, v)
    }
}

impl PipelineStage for ToneCurveStage {
    fn name(&self) -> &'static str {
        "tone_curve"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        match &self.channels {
            // ⚠ The common path, untouched: one table over every sample.
            None => image.map_samples(|s| self.sample(s)),
            Some(ch) => image
                .map_pixels(|p| std::array::from_fn(|c| sample_table(&ch[c], self.sample(p[c])))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 image with all three samples at `v`.
    fn flat(v: f32) -> ImageBuf {
        let mut b = ImageBuf::new(1, 1);
        for s in b.data_mut().iter_mut() {
            *s = v;
        }
        b
    }

    /// ⚠⚠ **A DIFFERENT curve per channel.** Three copies of one curve would pass against an
    /// implementation that ignored the channel index entirely — the fixture trap this project paid for
    /// on 2026-09-23.
    #[test]
    fn a_per_channel_curve_moves_only_its_own_channel() {
        let lift = [[0.0, 0.2], [1.0, 1.0]];
        let drop = [[0.0, 0.0], [1.0, 0.8]];
        let out = ToneCurveStage::new(&[])
            .with_channels(&lift, &[], &drop)
            .apply(flat(0.5));
        let px = [out.data()[0], out.data()[1], out.data()[2]];
        assert!(px[0] > 0.52, "red was lifted: {px:?}");
        assert!((px[1] - 0.5).abs() < 2e-3, "green untouched: {px:?}");
        assert!(px[2] < 0.48, "blue was dropped: {px:?}");
    }

    /// ⚠ The order the spec chose, on a fixture where the two orders differ measurably.
    #[test]
    fn the_composite_runs_before_the_channel_curve() {
        let composite = [[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]];
        let red = [[0.0, 0.5], [1.0, 1.0]];
        let got = ToneCurveStage::new(&composite)
            .with_channels(&red, &[], &[])
            .apply(flat(0.5))
            .data()[0];

        let composite_then_red =
            ToneCurveStage::new(&red).sample(ToneCurveStage::new(&composite).sample(0.5));
        let red_then_composite =
            ToneCurveStage::new(&composite).sample(ToneCurveStage::new(&red).sample(0.5));
        assert!(
            (composite_then_red - red_then_composite).abs() > 0.05,
            "the fixture must distinguish the two orders, else this proves nothing"
        );
        assert!(
            (got - composite_then_red).abs() < 2e-3,
            "expected composite-then-channel: got {got}, wanted {composite_then_red}"
        );
    }

    /// ⚠ With no channel curve the cheap path must be kept, and byte-identical to today.
    #[test]
    fn with_no_channel_curves_the_result_is_unchanged() {
        let c = [[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let plain = ToneCurveStage::new(&c).apply(flat(0.4));
        let built = ToneCurveStage::new(&c)
            .with_channels(&[], &[], &[])
            .apply(flat(0.4));
        assert_eq!(plain.data(), built.data());
    }

    /// ⚠⚠ The input clamp is **inherited, not re-decided**: a scene-referred highlight above 1.0 is
    /// still pulled to the table's top entry, because that is what the composite has always done and
    /// what the GPU shader mirrors.
    #[test]
    fn a_channel_curve_inherits_the_input_clamp() {
        let red = [[0.0, 0.0], [1.0, 0.5]];
        let out = ToneCurveStage::new(&[])
            .with_channels(&red, &[], &[])
            .apply(flat(4.0));
        assert!(
            (out.data()[0] - 0.5).abs() < 2e-3,
            "clamped to the top entry, got {}",
            out.data()[0]
        );
    }

    /// ⚠ `luts` hands out four tables even when no channel curve is set, because the shader is
    /// unconditional — and the three spares must be the identity, not copies of the composite.
    #[test]
    fn luts_pads_with_identity_when_no_channel_curve_is_set() {
        let luts = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]]).luts();
        assert_eq!(luts.len(), 4);
        assert!(luts[0][LUT_SIZE / 2] > 0.8, "the composite is curved");
        for (c, lut) in luts.iter().enumerate().skip(1) {
            let mid = lut[LUT_SIZE / 2];
            assert!(
                (mid - 0.5).abs() < 2e-3,
                "channel {c} should be identity: {mid}"
            );
        }
    }

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
    fn a_single_point_is_still_the_identity() {
        // ⚠ A curve is *built* one point at a time, so "one point" is a state the widget can
        // legitimately be in. It must pass the image through untouched rather than flatten it
        // to that point's value.
        let s = ToneCurveStage::new(&[[0.5, 0.9]]);
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((s.sample(v) - v).abs() < 2e-3, "v={v} -> {}", s.sample(v));
        }
    }

    #[test]
    fn duplicate_x_values_collapse_rather_than_poisoning_the_curve() {
        // Two points at the same x give an infinite secant, and one `NaN` tangent spreads
        // through the whole LUT. The curve must stay usable.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.3], [0.5, 0.8], [1.0, 1.0]]);
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            assert!(s.sample(v).is_finite(), "v={v} -> {}", s.sample(v));
        }
    }

    #[test]
    fn a_non_finite_point_does_not_panic() {
        // Reachable from a hand-edited sidecar: the points are plain JSON.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [f32::NAN, 0.5], [1.0, 1.0]]);
        assert!((s.sample(0.5) - 0.5).abs() < 2e-3);
    }

    #[test]
    fn lift_midpoint_brightens_mids() {
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!((s.sample(0.5) - 0.7).abs() < 2e-3);
        assert!(s.sample(0.25) > 0.25);
    }

    #[test]
    fn two_points_reduce_to_the_straight_line() {
        // ⚠ The regression guard on the *previous* implementation: with two points the
        // Hermite tangents are both the single secant, so the cubic is exactly the line. Any
        // two-point curve anyone has already saved renders unchanged.
        let s = ToneCurveStage::new(&[[0.2, 0.1], [0.8, 0.9]]);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let x = 0.2 + t * 0.6;
            assert!((s.sample(x) - (0.1 + t * 0.8)).abs() < 2e-3, "x={x}");
        }
    }

    /// A knee sharp enough that an unconstrained cubic visibly overshoots it: nearly flat up
    /// to 0.4, then a near-vertical rise, then nearly flat again.
    fn knee() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [0.4, 0.05], [0.5, 0.9], [1.0, 1.0]]
    }

    #[test]
    fn a_sharp_knee_never_reverses_contrast() {
        // ⚠⚠ **The test the whole interpolant exists for.** A Catmull-Rom through `knee()`
        // dips *below* 0.05 approaching x=0.4 and rises *above* 0.9 after x=0.5 — brighter in,
        // darker out. Sampling densely is what catches it: at the control points themselves
        // every interpolant agrees, and the overshoot lives strictly between them.
        let s = ToneCurveStage::new(&knee());
        let mut prev = s.sample(0.0);
        for i in 1..=2000 {
            let x = i as f32 / 2000.0;
            let y = s.sample(x);
            assert!(
                y >= prev - 1e-4,
                "contrast reversal at x={x}: {prev} -> {y}",
            );
            prev = y;
        }
    }

    #[test]
    fn a_sharp_knee_stays_inside_its_control_points() {
        // The other half of monotonicity, and the one a "non-decreasing" check alone cannot
        // see: a curve can rise everywhere and still bulge past the value it is heading for.
        let pts = knee();
        let s = ToneCurveStage::new(&pts);
        for i in 0..=2000 {
            let x = i as f32 / 2000.0;
            // Bracketing control points for this x.
            let lo = pts.iter().rev().find(|p| p[0] <= x).unwrap();
            let hi = pts.iter().find(|p| p[0] >= x).unwrap();
            let (a, b) = (lo[1].min(hi[1]), lo[1].max(hi[1]));
            let y = s.sample(x);
            assert!(
                y >= a - 2e-3 && y <= b + 2e-3,
                "overshoot at x={x}: {y} outside [{a}, {b}]",
            );
        }
    }

    #[test]
    fn a_curve_is_smooth_where_the_old_one_had_a_facet() {
        // ⚠ The reason for the change, stated as a property: at a control point the two
        // segments must agree on their slope. The piecewise-linear predecessor did not — that
        // corner is the facet visible in a sky.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]]);
        let d = 4e-3;
        let at = |x: f32| (s.sample(x + d) - s.sample(x - d)) / (2.0 * d);
        for &x in &[0.25f32, 0.75] {
            let (before, after) = (at(x - 2.0 * d), at(x + 2.0 * d));
            assert!(
                (before - after).abs() < 0.25,
                "slope jumps at x={x}: {before} -> {after}",
            );
        }
    }

    #[test]
    fn a_flat_segment_stays_flat() {
        // A plateau between two equal control points must not bulge: the tangents at both of
        // its ends are forced to zero for exactly this.
        let s = ToneCurveStage::new(&[[0.0, 0.0], [0.3, 0.6], [0.7, 0.6], [1.0, 1.0]]);
        for i in 0..=20 {
            let x = 0.3 + (i as f32 / 20.0) * 0.4;
            assert!((s.sample(x) - 0.6).abs() < 2e-3, "x={x} -> {}", s.sample(x));
        }
    }

    /// ⚠ The three-point endpoint can overshoot where the one-sided slope could not, so its two
    /// guards are load-bearing. A curve that turns sharply at the end is where they bite.
    #[test]
    fn the_endpoint_formula_still_cannot_overshoot() {
        for pts in [
            vec![[0.0, 0.0], [0.1, 0.9], [1.0, 1.0]],
            vec![[0.0, 0.0], [0.9, 0.1], [1.0, 1.0]],
            vec![[0.0, 0.0], [0.05, 0.02], [0.95, 0.99], [1.0, 1.0]],
        ] {
            let sp = Spline::new(&pts);
            for i in 0..=1000 {
                let x = i as f32 / 1000.0;
                let y = sp.eval(x);
                assert!(
                    (-1e-5..=1.0 + 1e-5).contains(&y),
                    "overshoot at x={x}: y={y} for {pts:?}"
                );
            }
            // And monotone: a tone curve that reverses is a contrast reversal.
            let mut prev = sp.eval(0.0);
            for i in 1..=1000 {
                let y = sp.eval(i as f32 / 1000.0);
                assert!(y >= prev - 1e-5, "reversal at {i} for {pts:?}");
                prev = y;
            }
        }
    }

    /// ⚠ The property that keeps every existing two-point sidecar byte-identical: with two points
    /// Hermite must reduce **exactly** to the straight line, so the endpoint formula is skipped.
    #[test]
    fn two_points_still_reduce_to_the_exact_straight_line() {
        let sp = Spline::new(&[[0.0, 0.2], [1.0, 0.8]]);
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((sp.eval(x) - (0.2 + 0.6 * x)).abs() < 1e-6, "at x={x}");
        }
    }
}

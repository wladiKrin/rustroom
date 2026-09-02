use crate::spatial::{gaussian_blur, kernel_radius, wide_blur, wide_blur_reach};
use crate::{PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::LocalContrast;
use rayon::prelude::*;

/// Texture's blur sigma, in **full-resolution pixels**. Distinctly coarser than sharpening's
/// default of 1.0, so the two own different bands. A tuning constant — see the spec.
pub const TEXTURE_SIGMA: f32 = 2.0;
/// Clarity's blur sigma, in **full-resolution pixels**. Large enough to read as local rather
/// than global contrast, and small enough that the reach it implies (120 px) stays under
/// `app::view_region::REGION_MARGIN`. A tuning constant — see the spec.
pub const CLARITY_SIGMA: f32 = 40.0;

/// Luma that the midtone weight peaks at: mid-gray.
const MID_GRAY: f32 = 0.18;
/// What fraction of Clarity survives at a hard edge — the suppressor's floor, `s` in
/// `stages/sharpening.rs`'s Detail model.
///
/// A fixed constant with no slider, because Lightroom has no halo control either: this is part
/// of what "Clarity" means rather than a decision to hand to the user.
pub(crate) const CLARITY_HALO_FLOOR: f32 = 0.25;
/// Coarse-band excursion at which the suppressor is half engaged.
///
/// ⚠ **Derived, not copied from `sharpening::HALO_KNEE` (0.05).** That knee is in luma-delta
/// units on the *fine* band; this one measures the **coarse** band, whose excursions are far
/// larger — on the golden chart `|d_c|` is 0.158 at the hard edge against 0.048 max / 0.017
/// mean in the texture region. 0.08 sits between the two, so an edge is well above it and
/// texture well below.
pub(crate) const CLARITY_HALO_KNEE: f32 = 0.08;

/// Near-black guard for the ratio. Load-bearing, exactly as in `stages/sharpening.rs`:
/// without it a dark pixel beside a bright edge divides by ~0 and the gain explodes.
const LUMA_EPS: f32 = 1e-3;
/// Caps a halo in near-black regions at +2 stops.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma, in linear light. Same as `stages/sharpening.rs`.
fn luma(px: &[f32]) -> f32 {
    // ⚠ Delegates to `color::luminance`, which carries the **working space's** weights. This
    // used to be a local copy of the Rec.709 triple — one of six, all of which became wrong the
    // day the working space stopped being sRGB.
    color::luminance([px[0], px[1], px[2]])
}

/// Midtone weight for Clarity: 1 at mid-gray, falling smoothly to 0 at black and at white.
///
/// `t = l / (l + MID_GRAY)` maps mid-gray to exactly 0.5 and needs no `powf`, which is why
/// it is preferred to an `l^(1/2.2)` proxy.
///
/// Applied to Clarity **only**. Without it, +Clarity at a 40 px radius clips skies and blocks
/// up shadows. Texture's band is low-amplitude and cannot reach the clip on its own, so
/// weighting it would cost work for no protection — the asymmetry is justified by amplitude,
/// not taste.
fn midtone_weight(l: f32) -> f32 {
    let t = l / (l + MID_GRAY);
    4.0 * t * (1.0 - t)
}

/// Halo suppression for Clarity's coarse band: 1 for a small excursion, falling toward
/// `floor` for a large one.
///
/// ⚠ **The same model `stages/sharpening.rs`'s Detail uses**, verbatim in form. `d_c` is
/// largest exactly *at* an edge — that is what a high-pass band is — so an unsuppressed
/// Clarity puts its strongest lift precisely where the halo is visible. Above the knee the
/// contribution collapses toward `floor`; in texture and gentle gradients `|d_c|` is below the
/// knee and passes at full strength.
///
/// The midtone weight is not enough on its own: it protects the *extremes of the tone range*,
/// which is a different question from *how strong the local excursion is*, and a hard edge in
/// the midtones has full weight.
fn halo_suppression(d: f32, floor: f32) -> f32 {
    floor + (1.0 - floor) / (1.0 + (d.abs() / CLARITY_HALO_KNEE).powi(2))
}

/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// The **max** of the two bands, not their sum: both are measured against the same source
/// luma and their deltas are added, so neither composes with the other. (Contrast
/// `sharpening::max_kernel_radius`, where the mask blurs `|d|` on top of the first blur and
/// the two radii genuinely add.) An inactive band contributes nothing.
///
/// Any renderer that crops a region before running this stage must keep at least this much
/// margin outside what it displays — see `crate::tail_kernel_radius`.
pub fn kernel_reach(params: &LocalContrast, scale: RenderScale) -> usize {
    let s = scale.get();
    let t = if params.texture != 0.0 {
        kernel_radius(TEXTURE_SIGMA * s)
    } else {
        0
    };
    let c = if params.clarity != 0.0 {
        wide_blur_reach(CLARITY_SIGMA * s)
    } else {
        0
    };
    t.max(c)
}

/// Texture and Clarity: local contrast in a fine and a coarse band.
///
/// The same operation as `SharpenStage` — `d = luma - blur(luma)` on Rec.709 luma in linear
/// light, re-applied as a scalar RGB gain — at two much different radii. Working on luma and
/// re-applying as a ratio means no colour fringing on a chromatic edge, hue and saturation
/// ratios preserved exactly, and **no Oklab round trip**.
///
/// **One stage, not two.** Both bands are measured against the *same* source luma and their
/// deltas summed, so the sliders are exactly additive. Two sequential stages would make
/// Clarity operate on Texture's output, and neither slider would be independently
/// predictable.
///
/// `k = amount/100` with no extra gain constant, so the endpoints mean something: **-100
/// removes the band and +100 doubles it** (for Clarity, in the midtones, where the weight is
/// 1).
pub struct LocalContrastStage {
    params: LocalContrast,
    scale: RenderScale,
}

impl LocalContrastStage {
    pub fn new(params: &LocalContrast, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for LocalContrastStage {
    fn name(&self) -> &'static str {
        "local_contrast"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        self.apply_with(image, CLARITY_HALO_FLOOR)
    }
}

impl LocalContrastStage {
    /// [`PipelineStage::apply`] with the halo suppressor's floor overridden.
    ///
    /// ⚠ Exists for the tests: "the suppressor cuts the overshoot" is a comparison of two
    /// runs, and a floor of exactly 1.0 is the unsuppressed stage. Production only ever calls
    /// it through `apply`, with [`CLARITY_HALO_FLOOR`].
    pub(crate) fn apply_with(&self, image: ImageBuf, halo_floor: f32) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let n = w * h;
        let s = self.scale.get();

        let l: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();

        let k_t = (self.params.texture / 100.0).clamp(-1.0, 1.0);
        let k_c = (self.params.clarity / 100.0).clamp(-1.0, 1.0);

        // Both bands read `l`, never each other's output. That is what makes the two
        // sliders additive rather than sequential.
        let mut delta = vec![0.0f32; n];
        if k_t != 0.0 {
            // Small sigma: a direct Gaussian kernel, which is cheap here and is the only
            // thing that can express a sub-pixel blur once RenderScale shrinks it.
            let base = gaussian_blur(&l, w, h, TEXTURE_SIGMA * s);
            delta
                .par_iter_mut()
                .zip(&l)
                .zip(&base)
                .for_each(|((d, &lv), &b)| *d += k_t * (lv - b));
        }
        if k_c != 0.0 {
            // Large sigma: iterated box passes, O(1) in the radius. A true Gaussian here
            // would be ~13 G operations on a 26 Mpx export.
            let base = wide_blur(&l, w, h, CLARITY_SIGMA * s);
            delta
                .par_iter_mut()
                .zip(&l)
                .zip(&base)
                .for_each(|((d, &lv), &b)| {
                    *d += k_c * (lv - b) * midtone_weight(lv) * halo_suppression(lv - b, halo_floor)
                });
        }

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .zip(image.data().par_chunks_exact(3))
            .zip(&delta)
            .zip(&l)
            .for_each(|(((o, px), &d), &lv)| {
                // Clamped at 0 below, so the stage can never emit a negative sample and
                // needs no separate clamp — the same structural property VignetteStage has.
                let gain = (1.0 + d / lv.max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
                o[0] = px[0] * gain;
                o[1] = px[1] * gain;
                o[2] = px[2] * gain;
            });
        ImageBuf::from_vec(w, h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All fixtures are 256 px wide on purpose: Clarity's kernel reaches 120 px at
    /// `RenderScale::FULL`, so on a 128 px image its blur is essentially the global mean and
    /// every measurement below would be about the border handling instead of about the
    /// filter.
    const S: usize = 256;

    /// A coarse 128 px-period sine (Clarity's band) with a fine 6 px-period sine on top
    /// (Texture's band), tinted so the hue-ratio property is exercised.
    fn two_band() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let coarse = 0.40 + 0.10 * (x as f32 / 128.0 * std::f32::consts::TAU).sin();
                let fine = 0.02 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
                let v = coarse + fine;
                img.set_pixel(x, y, [v, v * 0.8, v * 0.6]);
            }
        }
        img
    }

    /// Fine detail only: a 6 px-period sine on a flat mid gray.
    fn fine_only() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = 0.40 + 0.02 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// Coarse structure only: a 128 px-period sine, with nothing in the fine band.
    ///
    /// A *linear* ramp would be a poor fixture here — it is a fixed point of any symmetric
    /// blur, so Texture would be exactly inert on it and the test could not tell "Texture
    /// ignores coarse structure" apart from "Texture does nothing".
    fn coarse_only() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = 0.40 + 0.10 * (x as f32 / 128.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    fn run(params: LocalContrast, img: ImageBuf) -> ImageBuf {
        LocalContrastStage::new(&params, RenderScale::FULL).apply(img)
    }

    /// The stage with the halo suppressor's floor overridden — `1.0` is the unsuppressed
    /// stage, which is what "with and without" comparisons run against.
    fn run_with_floor(params: LocalContrast, img: ImageBuf, floor: f32) -> ImageBuf {
        LocalContrastStage::new(&params, RenderScale::FULL).apply_with(img, floor)
    }

    /// The hard edge's first bright row in [`hard_edge`].
    const EDGE: usize = 3 * S / 4;

    /// A hard horizontal edge at three quarters height — 0.2 above, 0.6 below — like the
    /// golden chart's. Grey, so luma is the sample value and expectations can be computed
    /// inline.
    fn hard_edge() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = if y < EDGE { 0.2 } else { 0.6 };
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// The largest excursion past the band's own value beside the edge — bright side
    /// upward, dark side downward — over the interior columns. Each column is flat in `y`
    /// except for the step, so the source's own value in that column *is* the band's.
    fn edge_overshoot(out: &ImageBuf, src: &ImageBuf) -> f32 {
        let mut worst = 0.0f32;
        for x in 30..S - 30 {
            for y in 0..S {
                let (o, s) = (out.pixel(x, y)[1], src.pixel(x, y)[1]);
                let e = if y >= EDGE { o - s } else { s - o };
                worst = worst.max(e);
            }
        }
        worst
    }

    #[test]
    fn the_halo_suppressor_cuts_the_edge_overshoot_and_keeps_fine_texture() {
        // ⚠ Two runs compared, not one measured: floor 1.0 is the unsuppressed stage, and
        // "the overshoot is small" alone cannot tell a suppressor from a weak Clarity.
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 60.0,
        };
        let src = hard_edge();
        let with = edge_overshoot(&run(cla, src.clone()), &src);
        let without = edge_overshoot(&run_with_floor(cla, src.clone(), 1.0), &src);
        assert!(
            without > 0.05,
            "fixture check: no halo to suppress ({without})"
        );
        assert!(
            with < without * 0.6,
            "the edge overshoot went {without} -> {with}, not suppressed"
        );

        // The other half: fine texture keeps at least 90% of the contrast Clarity gave it
        // unsuppressed. Measured on the *added* contrast, which is the stricter reading of
        // "retained within 10%" — the total span would pass with the suppressor set to
        // swallow half of Clarity's contribution.
        let fine = fine_only();
        let base = fine_span(&fine);
        let added_with = fine_span(&run(cla, fine.clone())) - base;
        let added_without = fine_span(&run_with_floor(cla, fine.clone(), 1.0)) - base;
        assert!(
            added_without > 0.0,
            "fixture check: Clarity added nothing to the sine"
        );
        assert!(
            added_with > added_without * 0.9,
            "fine texture kept only {added_with} of the {added_without} Clarity added"
        );
    }

    #[test]
    fn a_full_pass_through_floor_is_bit_identical_to_the_old_maths() {
        // The plumbing check: at floor 1.0 the suppressor is exactly 1.0 and the stage must
        // reproduce the pre-suppressor formula to the bit — computed here from the same
        // primitives, in the same order, on a tinted image so the RGB gain path is covered.
        let src = two_band();
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 60.0,
        };
        let out = run_with_floor(cla, src.clone(), 1.0);
        let l: Vec<f32> = src
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| luma(p))
            .collect();
        let base = wide_blur(&l, S, S, CLARITY_SIGMA);
        let k_c = cla.clarity / 100.0;
        let mut moved = 0usize;
        for (i, (o, px)) in out
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(src.data().as_chunks::<3>().0)
            .enumerate()
        {
            let d = k_c * (l[i] - base[i]) * midtone_weight(l[i]);
            let gain = (1.0 + d / l[i].max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
            for k in 0..3 {
                assert_eq!(o[k], px[k] * gain, "sample {i} channel {k}");
            }
            if o[0] != px[0] {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing changed, so nothing was compared");
    }

    /// Mean absolute luma change over the interior, avoiding the border band where the
    /// blur's edge handling dominates.
    fn mean_abs_change(a: &ImageBuf, b: &ImageBuf, border: usize) -> f32 {
        let (mut sum, mut count) = (0.0f32, 0usize);
        for y in border..a.height() - border {
            for x in border..a.width() - border {
                sum += (a.pixel(x, y)[1] - b.pixel(x, y)[1]).abs();
                count += 1;
            }
        }
        sum / count as f32
    }

    /// Peak-to-trough of the fine sine across the interior of a central row.
    fn fine_span(img: &ImageBuf) -> f32 {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for x in 30..S - 30 {
            let v = img.pixel(x, S / 2)[1];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    }

    #[test]
    fn both_zero_is_an_exact_identity() {
        let src = two_band();
        assert_eq!(run(LocalContrast::default(), src.clone()), src);
    }

    #[test]
    fn a_flat_region_is_untouched() {
        // d = 0 in both bands, so the gain is exactly 1.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, [0.42, 0.30, 0.18]);
            }
        }
        let out = run(
            LocalContrast {
                texture: 100.0,
                clarity: 100.0,
            },
            src.clone(),
        );
        for (a, b) in out.data().iter().zip(src.data()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn texture_and_clarity_are_exactly_additive() {
        // ⚠ The entire justification for one stage rather than two. Both bands are measured
        // against the *same* source luma and summed, so the sliders cannot interact. Two
        // sequential stages would make Clarity operate on Texture's output instead, and
        // neither slider would be independently predictable.
        //
        // gain = 1 + (dt + dc)/L, so out(t,c) = out(t,0) + out(0,c) - src, exactly — as
        // long as amounts stay mild enough that GAIN_MAX never engages.
        let src = two_band();
        let t = run(
            LocalContrast {
                texture: 40.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let c = run(
            LocalContrast {
                texture: 0.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let both = run(
            LocalContrast {
                texture: 40.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let mut moved = 0usize;
        for (i, (((&bo, &to), &co), &so)) in both
            .data()
            .iter()
            .zip(t.data())
            .zip(c.data())
            .zip(src.data())
            .enumerate()
        {
            let expect = to + co - so;
            assert!((bo - expect).abs() < 1e-5, "sample {i}: {bo} vs {expect}");
            if (bo - so).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing changed, so additivity was not tested");
    }

    #[test]
    fn texture_selects_the_fine_band_while_clarity_reaches_the_coarse_one() {
        // ⚠ Without this, a wrong sigma passes silently and every other test still goes
        // green.
        //
        // Note what is *not* asserted: that Texture beats Clarity on fine detail. Both are
        // high-pass, and Clarity's passband strictly *contains* Texture's — a 40 px blur of
        // a 6 px sine is the mean, so Clarity sees the fine band at nearly full strength too.
        // (Measured: ~0.011 for each.) An earlier draft of this test asserted the opposite
        // and would have failed against a correct implementation.
        //
        // The real discriminator is the other direction: Texture is band-*selective* and
        // barely sees coarse structure, while Clarity sees it strongly. Expected magnitudes
        // on these fixtures: coarse_t ~ 3e-4, fine_t ~ 1.1e-2, coarse_c ~ 4.7e-2.
        let tex = LocalContrast {
            texture: 100.0,
            clarity: 0.0,
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 100.0,
        };

        let fine = fine_only();
        let coarse = coarse_only();
        let fine_t = mean_abs_change(&run(tex, fine.clone()), &fine, 30);
        let coarse_t = mean_abs_change(&run(tex, coarse.clone()), &coarse, 30);
        let coarse_c = mean_abs_change(&run(cla, coarse.clone()), &coarse, 30);

        assert!(
            fine_t > coarse_t * 10.0,
            "texture is not fine-selective: fine {fine_t} vs coarse {coarse_t}"
        );
        assert!(
            coarse_c > coarse_t * 20.0,
            "clarity does not reach the coarse band: {coarse_c} vs texture's {coarse_t}"
        );
    }

    #[test]
    fn texture_minus_100_removes_the_fine_band() {
        // The endpoint claim: k = -1 subtracts the whole band, leaving the blurred base.
        //
        // The residual is not slop — it is exactly what a sigma-2 Gaussian retains of a 6 px
        // sine, about 11%. If this starts failing, check TEXTURE_SIGMA against the fixture's
        // 6 px period before touching the tolerance.
        let src = fine_only();
        let out = run(
            LocalContrast {
                texture: -100.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let (before, after) = (fine_span(&src), fine_span(&out));
        assert!(
            after < before * 0.2,
            "fine band went from {before} to {after}, not removed"
        );
    }

    #[test]
    fn texture_plus_100_roughly_doubles_the_fine_band() {
        // The other endpoint. About 1.89x rather than exactly 2, for the same reason:
        // the sigma-2 base retains ~11% of a 6 px sine, so `d` is 89% of the band.
        let src = fine_only();
        let out = run(
            LocalContrast {
                texture: 100.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let ratio = fine_span(&out) / fine_span(&src);
        assert!(
            (1.7..=2.1).contains(&ratio),
            "fine band scaled by {ratio}, expected about 1.9"
        );
    }

    #[test]
    fn midtone_weighting_protects_the_extremes() {
        // ⚠ Clarity only. Without it, +Clarity at a 40 px radius clips skies and blocks up
        // shadows — the standard complaint about the tool.
        //
        // The same coarse structure at three luma levels. Its amplitude is *proportional* to
        // the base, so `d` scales with the base too and the relative change below isolates
        // w(l) exactly. An absolute amplitude would go negative at base 0.01 and need
        // clamping, which would bias the comparison toward passing.
        //
        // Expected, from w = 4t(1-t) with t = l/(l+0.18): w(0.18) = 1.00, w(0.95) = 0.54,
        // w(0.01) = 0.20 — so mid/bright ~ 1.9 and mid/dark ~ 5.0.
        let ramp_at = |base: f32| {
            let mut img = ImageBuf::new(S, S);
            for y in 0..S {
                for x in 0..S {
                    let v = base * (1.0 + 0.25 * (x as f32 / 128.0 * std::f32::consts::TAU).sin());
                    img.set_pixel(x, y, [v; 3]);
                }
            }
            img
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 100.0,
        };
        // Relative change, so the comparison is about w(l) and not about absolute level.
        let relative = |base: f32| {
            let src = ramp_at(base);
            let out = run(cla, src.clone());
            mean_abs_change(&out, &src, 30) / base
        };
        let mid = relative(0.18);
        let bright = relative(0.95);
        let dark = relative(0.01);
        assert!(
            mid > bright * 1.5,
            "highlights not protected: {mid} vs {bright}"
        );
        assert!(mid > dark * 1.5, "shadows not protected: {mid} vs {dark}");
    }

    #[test]
    fn texture_is_not_midtone_weighted() {
        // The asymmetry is deliberate and justified by amplitude: the fine band cannot
        // reach the clip on its own, so weighting it would cost work for no protection.
        let sine_at = |base: f32| {
            let mut img = ImageBuf::new(S, S);
            for y in 0..S {
                for x in 0..S {
                    let v = base * (1.0 + 0.05 * (x as f32 / 6.0 * std::f32::consts::TAU).sin());
                    img.set_pixel(x, y, [v; 3]);
                }
            }
            img
        };
        let tex = LocalContrast {
            texture: 100.0,
            clarity: 0.0,
        };
        let relative = |base: f32| {
            let src = sine_at(base);
            mean_abs_change(&run(tex, src.clone()), &src, 30) / base
        };
        let mid = relative(0.18);
        let bright = relative(0.95);
        assert!(
            (mid / bright - 1.0).abs() < 0.05,
            "texture is level-dependent: {mid} vs {bright}"
        );
    }

    #[test]
    fn hue_ratios_are_preserved() {
        // Applying the correction as a scalar RGB gain is what buys this: no colour
        // fringing, and no Oklab round trip to pay for.
        let src = two_band();
        let out = run(
            LocalContrast {
                texture: 60.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let mut moved = 0usize;
        for (o, s) in out
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(src.data().as_chunks::<3>().0)
        {
            if o[0] <= 0.0 {
                continue;
            }
            assert!((o[1] / o[0] - s[1] / s[0]).abs() < 1e-4);
            assert!((o[2] / o[0] - s[2] / s[0]).abs() < 1e-4);
            if (o[0] - s[0]).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing changed, so nothing was tested");
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        // Includes a near-black band, where the LUMA_EPS guard and GAIN_MAX have to hold.
        let mut src = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = if x < S / 2 { 1e-6 } else { 1.0 };
                src.set_pixel(x, y, [v; 3]);
            }
        }
        let out = run(
            LocalContrast {
                texture: -100.0,
                clarity: 100.0,
            },
            src.clone(),
        );
        assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
        for (o, s) in out.data().iter().zip(src.data()) {
            assert!(*o <= s * GAIN_MAX + 1e-6, "{o} exceeds {s} * {GAIN_MAX}");
        }
    }

    #[test]
    fn radii_scale_with_the_render_scale() {
        // A full-resolution radius rendered at 1/4 scale is a quarter-size blur, so the
        // preview approximates the export rather than contradicting it.
        let both = LocalContrast {
            texture: 50.0,
            clarity: 50.0,
        };
        let full = kernel_reach(&both, RenderScale::FULL);
        let quarter = kernel_reach(&both, RenderScale::new(0.25));
        assert_eq!(full, 120); // wide_blur_reach(40.0)
        assert!(quarter < full, "reach {quarter} did not shrink from {full}");
        // Two different render scales must actually produce different pixels, or the scale
        // is being ignored somewhere inside `apply`.
        let src = two_band();
        let a = LocalContrastStage::new(&both, RenderScale::FULL).apply(src.clone());
        let b = LocalContrastStage::new(&both, RenderScale::new(0.25)).apply(src);
        assert_ne!(a, b);
    }

    #[test]
    fn kernel_reach_is_the_max_of_the_two_bands_and_zero_when_inactive() {
        // ⚠ max, not sum: the two bands are measured against the same source luma and
        // summed, so neither composes with the other — unlike sharpening's two Gaussians,
        // where the mask blurs |d| on top of the first blur and the radii genuinely add.
        let tex = LocalContrast {
            texture: 50.0,
            clarity: 0.0,
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 50.0,
        };
        let both = LocalContrast {
            texture: 50.0,
            clarity: 50.0,
        };
        assert_eq!(kernel_reach(&tex, RenderScale::FULL), 6); // ceil(3 * 2.0)
        assert_eq!(kernel_reach(&cla, RenderScale::FULL), 120);
        assert_eq!(kernel_reach(&both, RenderScale::FULL), 120);
        assert_eq!(
            kernel_reach(&LocalContrast::default(), RenderScale::FULL),
            0
        );
    }
}

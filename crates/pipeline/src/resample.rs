//! Resampling kernels — how a pixel is read at a fractional coordinate.
//!
//! Three stages resample: [`GeometryStage`](crate::GeometryStage)'s straighten,
//! [`PerspectiveStage`](crate::stages::perspective::PerspectiveStage), and
//! [`LensCorrectionStage`](crate::LensCorrectionStage). Every one of them goes through this
//! module, so the kernel is one choice rather than three.
//!
//! ⚠⚠ **THE CHOICE WAS MADE FROM MEASUREMENT, NOT FROM REPUTATION.**
//! `cargo run --release -p app --example resample_kernels <raw>` measured all four against real
//! X-T4 frames. Acutance *retained* after one straighten, and peak overshoot in 8-bit levels:
//!
//! | kernel | retained | cost | max ringing | detail/noise |
//! |---|---|---|---|---|
//! | [`Resampler::Bilinear`] | 54–58% | 1× | **0.0** | 2.69 |
//! | [`Resampler::Mitchell`] | 56–60% | 2.4× | 2.7 | 2.71 |
//! | [`Resampler::CatmullRom`] | **72–74%** | 1.8× | 12–19 | 2.44 |
//! | [`Resampler::Lanczos3`] | 82–84% | 3.7× | 28 | 2.38 |
//!
//! (An unresampled reference scores 2.17 detail/noise, so the sharp kernels' gain is genuine
//! detail rather than retained grain.) Catmull-Rom is the default: it buys back a third of the
//! acutance a bilinear pass throws away, at ringing that has to be hunted for. The others are
//! kept because the trade is a matter of taste and of subject — and because bilinear is the only
//! kernel with *provably* zero overshoot, and is what every export before today used.
//!
//! ⚠ **Black outside the frame**, exactly as the two hand-written samplers this replaces did.
//! That is also an adoption cost worth naming: a 6-tap kernel bleeds black **3 px** into the
//! frame where bilinear bleeds 1, so a straighten crop has to discard a slightly wider border.

use core_image::ImageBuf;
use serde::{Deserialize, Serialize};

/// The widest support any kernel here has, in taps per axis — Lanczos3's six.
pub const MAX_TAPS: usize = 6;

/// Sub-pixel phases in the weight lookup table: 1/1024 of a pixel, far below the quantisation
/// of an 8-bit output.
///
/// ⚠ The table exists because a naive Lanczos3 evaluates 24 `sin` calls per output pixel, and
/// shipping it that way would cost several times the 3.7× the measurement reports. Bilinear
/// never touches it — see [`Weights::axis`].
const LUT_PHASES: usize = 1024;

/// Which kernel reads a pixel at a fractional coordinate.
///
/// ⚠ Named by **algorithm**, deliberately unlike `raw_decode::Demosaic`, which is named by tier
/// because "DCB" is false on X-Trans. Here the names are accurate for every image, so they are
/// honest — but they mean nothing to a photographer on their own, which is what [`Self::hint`]
/// is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Resampler {
    /// Two taps per axis. What Rustroom shipped until M7.3, and the only kernel that provably
    /// cannot overshoot: its weights are non-negative, so an output sample always lies inside
    /// the bracket of the four it read.
    Bilinear,
    /// Mitchell–Netravali with `B = C = 1/3`. Softer than Catmull-Rom, with the least ringing of
    /// the three cubics.
    ///
    /// ⚠ **Not an interpolating filter.** `B > 0` puts weight 0.0556 on each neighbour even at
    /// zero phase, so a Mitchell resample softens the image *even where nothing moved*. That is
    /// inherent to the family and is the real reason it reads as the soft option.
    Mitchell,
    /// Mitchell–Netravali with `B = 0, C = 0.5`. The default: sharp, interpolating, and the
    /// standard "cubic without objectionable ringing".
    #[default]
    CatmullRom,
    /// Windowed sinc, `a = 3`. The sharpest, and the only one that rings visibly.
    Lanczos3,
}

impl Resampler {
    /// Every variant, softest first — the order the settings dropdown lists them in, and the
    /// order both acutance and ringing increase in.
    pub const ALL: [Resampler; 4] = [
        Resampler::Bilinear,
        Resampler::Mitchell,
        Resampler::CatmullRom,
        Resampler::Lanczos3,
    ];

    /// The name shown in the UI and accepted by `rustroom render --resampler`.
    pub fn label(self) -> &'static str {
        match self {
            Resampler::Bilinear => "Bilinear",
            Resampler::Mitchell => "Mitchell",
            Resampler::CatmullRom => "Catmull-Rom",
            Resampler::Lanczos3 => "Lanczos3",
        }
    }

    /// One line describing the trade, for the settings dialog.
    ///
    /// ⚠ Kept here beside [`Self::label`] rather than in the app, unlike `view_quality_hint` —
    /// that one quotes seconds measured on one camera, which is presentation. Sharpness against
    /// ringing is a property of the kernel itself and is true of every image, so the two strings
    /// live together where they cannot come to describe different kernels.
    pub fn hint(self) -> &'static str {
        match self {
            Resampler::Bilinear => {
                "Softest, and the only one that cannot overshoot. What Rustroom used before."
            }
            Resampler::Mitchell => "Slightly sharper than bilinear, with almost no ringing.",
            Resampler::CatmullRom => {
                "Recovers about a third of the detail a bilinear pass loses. Faint haloes on \
                 hard edges."
            }
            Resampler::Lanczos3 => {
                "Sharpest, and visibly haloed on high-contrast edges. Also the slowest."
            }
        }
    }

    /// Half-width of the support, in pixels. There are `2 * radius` taps per axis.
    pub fn radius(self) -> usize {
        match self {
            Resampler::Bilinear => 1,
            Resampler::CatmullRom | Resampler::Mitchell => 2,
            Resampler::Lanczos3 => 3,
        }
    }

    /// Taps per axis.
    pub fn taps(self) -> usize {
        2 * self.radius()
    }

    /// Whether a resample at an exact integer coordinate reproduces the source pixel.
    ///
    /// ⚠ False for [`Resampler::Mitchell`] alone. Stated as a method rather than left implicit
    /// because it is the property that decides whether a *no-op* geometry change costs anything.
    pub fn is_interpolating(self) -> bool {
        self != Resampler::Mitchell
    }

    /// The kernel's value at distance `t` from the sample point.
    pub fn weight(self, t: f32) -> f32 {
        let x = t.abs();
        match self {
            Resampler::Bilinear => {
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Resampler::CatmullRom => mitchell_netravali(x, 0.0, 0.5),
            Resampler::Mitchell => mitchell_netravali(x, 1.0 / 3.0, 1.0 / 3.0),
            Resampler::Lanczos3 => {
                if x < 3.0 {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

/// The Mitchell–Netravali cubic family. `B = 0, C = 0.5` is Catmull-Rom; `B = C = 1/3` is
/// Mitchell.
///
/// ⚠ Every member of this family is a partition of unity — the four taps sum to exactly 1 at any
/// phase — so the normalisation in [`axis_weights`] is a rounding correction here and genuinely
/// load-bearing only for Lanczos3.
fn mitchell_netravali(x: f32, b: f32, c: f32) -> f32 {
    let x2 = x * x;
    let x3 = x2 * x;
    if x < 1.0 {
        ((12.0 - 9.0 * b - 6.0 * c) * x3 + (-18.0 + 12.0 * b + 6.0 * c) * x2 + (6.0 - 2.0 * b))
            / 6.0
    } else if x < 2.0 {
        ((-b - 6.0 * c) * x3
            + (6.0 * b + 30.0 * c) * x2
            + (-12.0 * b - 48.0 * c) * x
            + (8.0 * b + 24.0 * c))
            / 6.0
    } else {
        0.0
    }
}

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-8 {
        1.0
    } else {
        let p = std::f32::consts::PI * x;
        p.sin() / p
    }
}

/// Weights for one axis at fractional offset `f`, written into `out[..taps]` and normalised.
///
/// Tap `i` sits at source index `floor(coord) - radius + 1 + i`, so its distance from the sample
/// point is `f + (radius - 1 - i)`.
///
/// ⚠ The normalisation divides by the sum over **all** taps, including those a sample will then
/// find out of bounds — which is what reproduces the shipping sampler's black-outside darkening
/// at the frame edge rather than quietly renormalising it away.
fn axis_weights(k: Resampler, f: f32, out: &mut [f32; MAX_TAPS]) {
    let r = k.radius() as f32;
    let n = k.taps();
    let mut sum = 0.0;
    for (i, w) in out.iter_mut().enumerate().take(n) {
        *w = k.weight(f + (r - 1.0 - i as f32));
        sum += *w;
    }
    if sum != 0.0 {
        for w in out.iter_mut().take(n) {
            *w /= sum;
        }
    }
}

/// A kernel with its weight table built, ready to sample with.
///
/// ⚠ Built once per stage rather than per pixel: the table is 24 KB and building it evaluates
/// the kernel 6144 times.
pub struct Weights {
    kernel: Resampler,
    taps: usize,
    radius: i32,
    /// Row-major, `LUT_PHASES * taps` entries. **Empty for [`Resampler::Bilinear`]**, which is
    /// computed exactly — see [`Self::axis`].
    lut: Vec<f32>,
}

impl Weights {
    pub fn new(kernel: Resampler) -> Self {
        let taps = kernel.taps();
        let radius = kernel.radius() as i32;
        let lut = if kernel == Resampler::Bilinear {
            Vec::new()
        } else {
            let mut rows = vec![0.0; LUT_PHASES * taps];
            let mut buf = [0.0f32; MAX_TAPS];
            for p in 0..LUT_PHASES {
                axis_weights(kernel, p as f32 / LUT_PHASES as f32, &mut buf);
                rows[p * taps..p * taps + taps].copy_from_slice(&buf[..taps]);
            }
            rows
        };
        Self {
            kernel,
            taps,
            radius,
            lut,
        }
    }

    /// Which kernel this table is for.
    pub fn kernel(&self) -> Resampler {
        self.kernel
    }

    /// Fill `out[..taps]` with the weights for phase `f ∈ [0, 1)`.
    ///
    /// ⚠⚠ **BILINEAR IS COMPUTED EXACTLY AND MUST STAY THAT WAY.** Two things would otherwise
    /// break bit-identity with the sampler this replaced, and bit-identity is what proves the
    /// refactor moved no behaviour for the kernel that used to ship. First, a 1024-phase table
    /// quantises `f` and so changes every weight. Second, the generic tap distance for `i = 1`
    /// is `f - 1`, and `1.0 - (1.0 - f)` is **not** `f` in f32 — for `f = 0.1` it is 0.100000024.
    /// Writing `[1 - f, f]` is both exact and the honest algebra.
    fn axis(&self, f: f32, out: &mut [f32; MAX_TAPS]) {
        if self.kernel == Resampler::Bilinear {
            out[0] = 1.0 - f;
            out[1] = f;
            return;
        }
        let p = ((f * LUT_PHASES as f32) as usize).min(LUT_PHASES - 1);
        out[..self.taps].copy_from_slice(&self.lut[p * self.taps..p * self.taps + self.taps]);
    }
}

/// The top-left tap's source index for a sample at `s`, and the phase within the pixel.
fn tap_origin(s: f32, radius: i32) -> (i32, f32) {
    let base = s.floor();
    (base as i32 - radius + 1, s - base)
}

/// Sample all three channels of `src` at fractional `(sx, sy)`; outside the frame reads black.
pub fn sample(src: &ImageBuf, sx: f32, sy: f32, w: &Weights) -> [f32; 3] {
    let (x0, fx) = tap_origin(sx, w.radius);
    let (y0, fy) = tap_origin(sy, w.radius);
    let mut wx = [0.0f32; MAX_TAPS];
    let mut wy = [0.0f32; MAX_TAPS];
    w.axis(fx, &mut wx);
    w.axis(fy, &mut wy);

    let (iw, ih) = (src.width() as i32, src.height() as i32);
    let mut out = [0.0f32; 3];
    for (j, &wyj) in wy.iter().enumerate().take(w.taps) {
        let yi = y0 + j as i32;
        if yi < 0 || yi >= ih {
            continue;
        }
        let mut row = [0.0f32; 3];
        for (i, &wxi) in wx.iter().enumerate().take(w.taps) {
            let xi = x0 + i as i32;
            if xi < 0 || xi >= iw {
                continue;
            }
            let p = src.pixel(xi as usize, yi as usize);
            row[0] += wxi * p[0];
            row[1] += wxi * p[1];
            row[2] += wxi * p[2];
        }
        out[0] += wyj * row[0];
        out[1] += wyj * row[1];
        out[2] += wyj * row[2];
    }
    out
}

/// Sample a region-scoped source at a point in **full-image** coordinates.
///
/// ⚠⚠ **Coordinates are the FULL image's, not the buffer's.** A sampler taking buffer coordinates
/// would silently shift every read by the region's origin — a plausible photograph of the wrong
/// part of the frame, which is M8.6's centre sabotage and which all 63 goldens were blind to.
///
/// ⚠ A tap outside the buffer reads black, exactly as [`sample`] does outside the frame. That is
/// correct **because the margin makes it unreachable**: [`crate::region_plan`] asks every stage
/// what it needs and grows the request accordingly, so within the rect a stage is asked to produce,
/// every tap it reads is inside the buffer it was given. A tap that does escape is a bug in some
/// `input_rect_for`, and it shows up as a dark band at the region's edge.
pub fn sample_region(src: &crate::RegionSrc<'_>, sx: f32, sy: f32, w: &Weights) -> [f32; 3] {
    sample(
        src.buf,
        sx - src.origin.0 as f32,
        sy - src.origin.1 as f32,
        w,
    )
}

/// Sample **one** channel of `src` at fractional `(sx, sy)`; outside the frame reads black.
///
/// ⚠ Per channel rather than per pixel, because transverse chromatic aberration gives each
/// channel its own source point. That requirement is why this is a second entry point rather
/// than something [`sample`] could be trimmed down to.
pub fn sample_channel(src: &ImageBuf, sx: f32, sy: f32, ch: usize, w: &Weights) -> f32 {
    let (x0, fx) = tap_origin(sx, w.radius);
    let (y0, fy) = tap_origin(sy, w.radius);
    let mut wx = [0.0f32; MAX_TAPS];
    let mut wy = [0.0f32; MAX_TAPS];
    w.axis(fx, &mut wx);
    w.axis(fy, &mut wy);

    let (iw, ih) = (src.width() as i32, src.height() as i32);
    let mut out = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate().take(w.taps) {
        let yi = y0 + j as i32;
        if yi < 0 || yi >= ih {
            continue;
        }
        let mut row = 0.0f32;
        for (i, &wxi) in wx.iter().enumerate().take(w.taps) {
            let xi = x0 + i as i32;
            if xi < 0 || xi >= iw {
                continue;
            }
            row += wxi * src.pixel(xi as usize, yi as usize)[ch];
        }
        out += wyj * row;
    }
    out
}

/// Sample **one** channel of a region-scoped source at a point in **full-image** coordinates.
///
/// ⚠ The per-channel mirror of [`sample_region`], and it exists for the reason that one does:
/// transverse chromatic aberration gives each channel its own source point, so lens correction
/// cannot go through the three-channel entry point.
///
/// ⚠⚠ **Coordinates are the FULL image's, not the buffer's.** A sampler taking buffer coordinates
/// would silently shift every read by the region's origin — a plausible photograph of the wrong
/// part of the frame, which is M8.6's centre sabotage and which all 63 goldens were blind to.
///
/// ⚠ A tap outside the buffer reads black exactly as [`sample`] does outside the frame — correct
/// **because the margin makes it unreachable**. A tap that does escape is a bug in some
/// `input_rect_for`, and it shows up as a dark band at the region's edge.
pub fn sample_channel_region(
    src: &crate::RegionSrc<'_>,
    sx: f32,
    sy: f32,
    ch: usize,
    w: &Weights,
) -> f32 {
    sample_channel(
        src.buf,
        sx - src.origin.0 as f32,
        sy - src.origin.1 as f32,
        ch,
        w,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The sampler Rustroom shipped before M7.3**, copied here verbatim.
    ///
    /// ⚠ Deliberately *not* shared with production — a reference that delegates to the thing
    /// under test can witness nothing. This is the same trade the golden harness makes by
    /// keeping its own copy of the 8-bit quantizer.
    fn legacy_bilinear(src: &ImageBuf, sx: f32, sy: f32) -> [f32; 3] {
        let w = src.width() as i32;
        let h = src.height() as i32;
        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;
        let get = |xi: i32, yi: i32| -> [f32; 3] {
            if xi < 0 || yi < 0 || xi >= w || yi >= h {
                [0.0, 0.0, 0.0]
            } else {
                src.pixel(xi as usize, yi as usize)
            }
        };
        let p00 = get(x0, y0);
        let p10 = get(x0 + 1, y0);
        let p01 = get(x0, y0 + 1);
        let p11 = get(x0 + 1, y0 + 1);
        let mut out = [0.0f32; 3];
        for c in 0..3 {
            let top = p00[c] * (1.0 - fx) + p10[c] * fx;
            let bot = p01[c] * (1.0 - fx) + p11[c] * fx;
            out[c] = top * (1.0 - fy) + bot * fy;
        }
        out
    }

    /// The per-channel sampler `lens_correction` shipped before M7.3, likewise verbatim.
    fn legacy_bilinear_channel(src: &ImageBuf, sx: f32, sy: f32, ch: usize) -> f32 {
        let (w, h) = (src.width() as i32, src.height() as i32);
        let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
        let at = |x: i32, y: i32| -> f32 {
            if x < 0 || y < 0 || x >= w || y >= h {
                0.0
            } else {
                src.pixel(x as usize, y as usize)[ch]
            }
        };
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bot * fy
    }

    /// A frame with no two pixels alike, so a tap read from the wrong place is visible rather
    /// than averaged away.
    fn noisy(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = (x * 7 + y * 13) as f32;
                img.set_pixel(
                    x,
                    y,
                    [
                        (t * 0.017).sin() * 0.5 + 0.5,
                        (t * 0.031).cos() * 0.5 + 0.5,
                        (t * 0.011).sin() * 0.4 + 0.4,
                    ],
                );
            }
        }
        img
    }

    /// Coordinates covering the interior, both borders and well outside the frame — so the
    /// black-outside path is exercised as much as the interior one.
    fn probes() -> Vec<(f32, f32)> {
        let mut v = Vec::new();
        for i in -3..24 {
            for j in -3..18 {
                v.push((i as f32 + 0.317, j as f32 + 0.826));
                v.push((i as f32, j as f32 + 0.5));
                v.push((i as f32 + 0.5, j as f32));
                v.push((i as f32, j as f32));
            }
        }
        v
    }

    /// ⚠⚠ **THE TEST THAT PROVES THE REFACTOR MOVED NO BEHAVIOUR.** `assert_eq!` on `f32`, not a
    /// tolerance: every export and every golden rendered before M7.3 went through
    /// `legacy_bilinear`, and "close enough" would let a real change hide in the last bit.
    #[test]
    fn bilinear_is_bit_identical_to_the_sampler_it_replaces() {
        let img = noisy(21, 15);
        let w = Weights::new(Resampler::Bilinear);
        for (sx, sy) in probes() {
            assert_eq!(
                sample(&img, sx, sy, &w),
                legacy_bilinear(&img, sx, sy),
                "at ({sx}, {sy})"
            );
        }
    }

    /// The per-channel half of the same guarantee — a separate sampler in production, so a
    /// separate reference here.
    #[test]
    fn the_per_channel_bilinear_is_bit_identical_too() {
        let img = noisy(21, 15);
        let w = Weights::new(Resampler::Bilinear);
        for (sx, sy) in probes() {
            for ch in 0..3 {
                assert_eq!(
                    sample_channel(&img, sx, sy, ch, &w),
                    legacy_bilinear_channel(&img, sx, sy, ch),
                    "at ({sx}, {sy}) channel {ch}"
                );
            }
        }
    }

    /// The two entry points must agree channel for channel, or TCA-corrected pixels would differ
    /// from straightened ones for no reason anybody could see.
    #[test]
    fn sampling_one_channel_agrees_with_sampling_all_three() {
        let img = noisy(19, 17);
        for k in Resampler::ALL {
            let w = Weights::new(k);
            for (sx, sy) in probes() {
                let all = sample(&img, sx, sy, &w);
                for (ch, want) in all.iter().enumerate() {
                    assert_eq!(sample_channel(&img, sx, sy, ch, &w), *want, "{k:?}");
                }
            }
        }
    }

    /// A kernel whose weights do not sum to 1 modulates brightness with sub-pixel phase — a
    /// moiré that reads as a sensor problem rather than a resampler one.
    #[test]
    fn every_kernel_sums_to_one_at_every_phase() {
        for k in Resampler::ALL {
            let w = Weights::new(k);
            for p in 0..257 {
                let f = p as f32 / 257.0;
                let mut buf = [0.0f32; MAX_TAPS];
                w.axis(f, &mut buf);
                let sum: f32 = buf.iter().take(k.taps()).sum();
                assert!((sum - 1.0).abs() < 1e-5, "{k:?} at phase {f} sums to {sum}");
            }
        }
    }

    /// ⚠⚠ **THIS FIRST ASSERTED SOMETHING FALSE ABOUT CORRECT CODE, and the false half is the
    /// finding.** It demanded every kernel reproduce the source at an integer coordinate.
    /// Bilinear, Catmull-Rom and Lanczos3 do; **Mitchell does not**, and that is inherent to
    /// `B > 0` rather than a coefficient error. [`Resampler::is_interpolating`] exists so the
    /// asymmetry is stated rather than rediscovered.
    #[test]
    fn only_mitchell_is_not_interpolating() {
        for k in Resampler::ALL {
            let w = Weights::new(k);
            let mut buf = [0.0f32; MAX_TAPS];
            w.axis(0.0, &mut buf);
            let centre = k.radius() - 1;
            if !k.is_interpolating() {
                assert!(
                    buf[centre] < 0.9,
                    "{k:?} claims not to interpolate but its centre tap is {}",
                    buf[centre]
                );
                continue;
            }
            for (i, wi) in buf.iter().enumerate().take(k.taps()) {
                let want = if i == centre { 1.0 } else { 0.0 };
                assert!(
                    (wi - want).abs() < 1e-6,
                    "{k:?} tap {i} is {wi}, want {want}"
                );
            }
        }
    }

    /// The whole-image consequence of the tap test above: an interpolating kernel asked for the
    /// pixels it already has hands them back untouched, so a no-op resample costs nothing.
    #[test]
    fn an_interpolating_kernel_reproduces_the_frame_at_integer_coordinates() {
        let img = noisy(23, 19);
        for k in Resampler::ALL {
            let w = Weights::new(k);
            let mut worst = 0.0f32;
            for y in 3..16 {
                for x in 3..20 {
                    let got = sample(&img, x as f32, y as f32, &w);
                    let want = img.pixel(x, y);
                    for c in 0..3 {
                        worst = worst.max((got[c] - want[c]).abs());
                    }
                }
            }
            if k.is_interpolating() {
                assert!(worst < 1e-6, "{k:?} moved an unmoved pixel by {worst}");
            } else {
                assert!(
                    worst > 1e-3,
                    "{k:?} should soften even at zero phase ({worst})"
                );
            }
        }
    }

    /// A step edge, wide enough that Lanczos3's six taps stay in bounds for every probe — so
    /// nothing below measures the black-outside border instead of the kernel.
    fn step_edge() -> ImageBuf {
        let mut img = ImageBuf::new(14, 1);
        for x in 0..14 {
            img.set_pixel(x, 0, if x < 7 { [0.0; 3] } else { [1.0; 3] });
        }
        img
    }

    /// Worst excursion outside `[0, 1]` anywhere along the edge.
    fn peak_overshoot(k: Resampler) -> f32 {
        let img = step_edge();
        let w = Weights::new(k);
        let mut worst = 0.0f32;
        // ⚠ Swept across the *whole* interior, not just the edge pixel: a cubic's overshoot peaks
        // one to two pixels away from the transition, so probing only at x = 7 finds nothing.
        for i in 0..=600 {
            let sx = 4.0 + i as f32 / 100.0;
            let v = sample(&img, sx, 0.0, &w)[0];
            worst = worst.max((-v).max(v - 1.0));
        }
        worst
    }

    /// ⚠ The property that makes bilinear worth keeping in the list: its weights are
    /// non-negative, so an output sample always lies inside the bracket it read.
    #[test]
    fn only_bilinear_stays_inside_the_bracket() {
        for k in Resampler::ALL {
            let worst = peak_overshoot(k);
            if k == Resampler::Bilinear {
                assert!(worst < 1e-6, "bilinear overshot by {worst}");
            } else {
                assert!(worst > 1e-4, "{k:?} did not overshoot at all ({worst})");
            }
        }
    }

    /// The ordering the whole comparison rests on. If it inverts, a coefficient is wrong.
    #[test]
    fn ringing_increases_from_mitchell_to_catmull_rom_to_lanczos() {
        let m = peak_overshoot(Resampler::Mitchell);
        let c = peak_overshoot(Resampler::CatmullRom);
        let l = peak_overshoot(Resampler::Lanczos3);
        assert!(m < c, "Mitchell {m} should ring less than Catmull-Rom {c}");
        assert!(c < l, "Catmull-Rom {c} should ring less than Lanczos3 {l}");
    }

    /// Outside the frame reads black, which is what the two samplers this replaces did and what
    /// the straighten crop is sized against.
    #[test]
    fn far_outside_the_frame_is_black() {
        let img = noisy(9, 9);
        for k in Resampler::ALL {
            let w = Weights::new(k);
            assert_eq!(sample(&img, -20.0, 4.0, &w), [0.0; 3], "{k:?}");
            assert_eq!(sample(&img, 4.0, 40.0, &w), [0.0; 3], "{k:?}");
            assert_eq!(sample_channel(&img, -20.0, 4.0, 1, &w), 0.0, "{k:?}");
        }
    }

    /// ⚠ An empty frame must return black rather than index out of bounds — `ImageBuf::new(0, 0)`
    /// is reachable through the degenerate paths the geometry stages already guard.
    #[test]
    fn an_empty_frame_samples_to_black() {
        let img = ImageBuf::new(0, 0);
        for k in Resampler::ALL {
            let w = Weights::new(k);
            assert_eq!(sample(&img, 0.5, 0.5, &w), [0.0; 3], "{k:?}");
        }
    }

    /// Every variant reachable from the settings dropdown must be listed exactly once, or a
    /// kernel added later is silently unreachable.
    #[test]
    fn the_default_is_catmull_rom_and_every_variant_is_listed() {
        assert_eq!(Resampler::default(), Resampler::CatmullRom);
        for k in Resampler::ALL {
            assert_eq!(Resampler::ALL.iter().filter(|o| **o == k).count(), 1);
            assert!(!k.label().is_empty());
            assert!(!k.hint().is_empty());
            assert_eq!(k.taps(), 2 * k.radius());
            assert!(k.taps() <= MAX_TAPS);
        }
    }

    /// ⚠ The per-channel mirror of the region sampler's contract: coordinates are the **full
    /// image's**, so a sampler taking buffer coordinates would silently shift every read by the
    /// region's origin — a plausible photograph of the wrong part of the frame.
    ///
    /// ⚠ Catmull-Rom rather than bilinear, so the taps reach two pixels either side and the
    /// comparison genuinely exercises the offset rather than a 2x2 neighbourhood that happens to
    /// sit well inside both buffers.
    #[test]
    fn the_per_channel_region_sampler_reads_full_image_coordinates() {
        let mut src = ImageBuf::new(12, 12);
        for y in 0..12 {
            for x in 0..12 {
                src.set_pixel(x, y, [x as f32, y as f32, (x + y) as f32 * 0.25]);
            }
        }
        let whole = crate::RegionSrc::whole(&src);
        let w = Weights::new(Resampler::CatmullRom);
        // The same image, sliced and told where the slice sits.
        let slice = src.crop(2, 3, 8, 7);
        let part = crate::RegionSrc {
            buf: &slice,
            origin: (2, 3),
            full: (12, 12),
        };
        for ch in 0..3 {
            let a = sample_channel_region(&whole, 5.25, 6.5, ch, &w);
            let b = sample_channel_region(&part, 5.25, 6.5, ch, &w);
            assert_eq!(a, b, "channel {ch}: whole {a} against region {b}");
        }
    }
}

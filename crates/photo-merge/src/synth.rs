//! Test-only brackets with a known truth: an analytic scene rendered through known motions and
//! exposures, clipped and noisy as a sensor would be — so alignment, exposure and merge are judged
//! against the scene itself, with no resampling error in the fixture.
//!
//! ⚠ Constructed, not captured: it proves the algorithms do what they claim on a scene they can
//! see; the real brackets (`HDR_FIXTURE`) are the other half.

use crate::frame::Frame;
use crate::homography::Homography;
use core_image::ImageBuf;

/// The multipliers every synthetic frame carries — not 1, so dividing them out is exercised.
pub const PRE_MUL: [f32; 3] = [2.0, 1.0, 1.5];

/// Something that is in one frame and not the others: inside the rectangle (reference pixels),
/// frame `frame`'s radiance is multiplied by `gain`.
#[derive(Clone, Copy)]
pub struct Mover {
    pub frame: usize,
    pub rect: (f64, f64, f64, f64),
    pub gain: f32,
}

/// The scene's normalised raw radiance at reference pixel `(u, v)` of a `w × h` reference: a
/// smooth light field spanning ~12 stops (deep shadow at the top left, past white at the bottom
/// right) under a texture of sines, wavelengths 8–60 px, so every region has detail to align on.
pub fn scene(u: f64, v: f64, w: usize, h: usize) -> [f32; 3] {
    let (x, y) = (u / w as f64, v / h as f64);
    let light =
        -9.0 + 7.0 * x + 4.0 * y + 1.5 * (-((x - 0.3).powi(2) + (y - 0.7).powi(2)) * 8.0).exp();
    let t = 0.55 * (u * 0.21 + 0.3).sin() * (v * 0.17).sin()
        + 0.35 * (u * 0.05 + v * 0.07).sin()
        + 0.30 * (u * 0.11 - v * 0.13 + 1.0).sin()
        + 0.25 * ((u + v) * 0.31).sin()
        + 0.20 * (v * 0.43 + (u * 0.02).sin() * 3.0).sin();
    let l = 2f64.powf(light + t);
    let tint = 0.15 * (u * 0.03 + v * 0.02).sin();
    [l * (1.0 + tint), l, l * (1.0 - tint)].map(|c| c as f32)
}

/// A tiny deterministic generator: splitmix64 and Box–Muller.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// A bracket of `w × h` frames: frame `i` is exposed `evs[i]` stops from the scene (its `ev` says
/// so too) and shows the scene through `motion[i]` — reference pixels → frame pixels. Shot noise
/// (a full well of ~32k electrons) and 3 levels of read noise; clipped at libraw's ceiling.
/// Returns the frames and the scene's radiance on the reference grid.
pub fn bracket(
    w: usize,
    h: usize,
    evs: &[f32],
    motion: &[Homography],
    seed: u64,
    mover: Option<Mover>,
) -> (Vec<Frame>, ImageBuf) {
    let frames = evs
        .iter()
        .zip(motion)
        .enumerate()
        .map(|(i, (&ev, m))| {
            let inv = m.inverse().expect("invertible motion");
            let mut rng = Noise(seed.wrapping_mul(31).wrapping_add(i as u64));
            let mut px = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = inv.apply(x as f64, y as f64);
                    let mut r = scene(u, v, w, h);
                    if let Some(mv) = mover.filter(|mv| mv.frame == i) {
                        let (x0, y0, x1, y1) = mv.rect;
                        if (x0..x1).contains(&u) && (y0..y1).contains(&v) {
                            r = r.map(|c| c * mv.gain);
                        }
                    }
                    for c in 0..3 {
                        let e = (r[c] * 2f32.powf(ev) * PRE_MUL[c]) as f64 * 65535.0;
                        let electrons = e * 0.5;
                        let noisy = e + rng.gauss() * (electrons.max(0.0).sqrt() * 2.0 + 3.0);
                        px.push(noisy.round().clamp(0.0, 65535.0) as u16);
                    }
                }
            }
            Frame::new(w, h, px, PRE_MUL, Some(ev))
        })
        .collect();
    let mut truth = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            truth.set_pixel(x, y, scene(x as f64, y as f64, w, h));
        }
    }
    (frames, truth)
}

/// A turn by `deg` about the centre of a `w × h` frame, then a shift.
pub fn turn(deg: f64, dx: f64, dy: f64, w: usize, h: usize) -> Homography {
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    let (s, c) = deg.to_radians().sin_cos();
    Homography::translation(cx + dx, cy + dy)
        .compose(&Homography([c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0]))
        .compose(&Homography::translation(-cx, -cy))
}

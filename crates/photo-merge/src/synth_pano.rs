//! Test-only panoramas with a known truth (M10.17b): a radiance defined on the sphere of directions,
//! photographed by a camera turning about its lens — so every frame is an exact view of one scene, and
//! the rig, the gains and the seams are judged against the rotations that made them.
//!
//! The texture is a sum of plane waves in **direction** space, so it is smooth (no aliasing between
//! frames) and has detail at every place a feature or a seam is judged. Over it: a sky brightening
//! toward the zenith above the horizon, a dark horizon line along latitude 0, dark vertical poles below
//! it every 0.35 rad of longitude, and — per shot — a bright square that is there in one frame only.
//!
//! ⚠ Constructed, not captured: it proves the algorithms on a scene they can see; the author's own
//! panoramas (`PANO_FIXTURE`) are the other half.

use crate::frame::Frame;
use crate::synth::{Noise, PRE_MUL};

/// A camera-to-world rotation from yaw (about +y, turning right), pitch (about +x, looking up is
/// negative y) and roll (about +z), applied roll first. Camera axes: x right, y down, z forward.
pub fn camera_to_world(yaw: f64, pitch: f64, roll: f64) -> [[f64; 3]; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
    let rz = [[cr, -sr, 0.0], [sr, cr, 0.0], [0.0, 0.0, 1.0]];
    mul(&mul(&ry, &rx), &rz)
}

fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn apply_t(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2])
}

/// One frame of a panorama: where the camera pointed and how the frame was exposed.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
    /// Linear exposure relative to the scene (2^EV).
    pub gain: f32,
    /// Light lost at the frame's corner, as a share: `1 − vignette · r²`, `r` 1 at the corner.
    pub vignette: f32,
    /// A dark square (0.15 of the scene — someone in dark clothes, never clipped) seen in this frame only:
    /// `[longitude, latitude, half side]`, radians.
    pub square: Option<[f64; 3]>,
}

impl Shot {
    pub fn rotation(&self) -> [[f64; 3]; 3] {
        camera_to_world(self.yaw, self.pitch, self.roll)
    }
}

/// A scene on the sphere of directions.
pub struct Scene {
    /// Plane waves: a unit wave vector scaled by its angular frequency, a phase and an amplitude.
    waves: Vec<([f64; 3], f64, f64)>,
}

impl Scene {
    /// The scene seeded `seed` — two seeds are two unrelated places.
    pub fn new(seed: u64) -> Self {
        let mut rng = Noise(seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(7));
        let waves = (0..48)
            .map(|i| {
                let dir = loop {
                    let v = [0, 1, 2].map(|_| rng.next() * 2.0 - 1.0);
                    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if n > 0.2 && n <= 1.0 {
                        break v.map(|c| c / n);
                    }
                };
                // Periods of ~0.03–0.12 rad: 13–50 px at the tests' 420-px focal length.
                let omega = 50.0 + 160.0 * rng.next();
                let amp = 0.9 / (1.0 + i as f64 * 0.06);
                (
                    dir.map(|c| c * omega),
                    rng.next() * std::f64::consts::TAU,
                    amp,
                )
            })
            .collect();
        Self { waves }
    }

    /// Longitude (right positive) and latitude (up positive) of a direction.
    pub fn lon_lat(d: [f64; 3]) -> (f64, f64) {
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = d.map(|c| c / n);
        (d[0].atan2(d[2]), (-d[1]).atan2(d[0].hypot(d[2])))
    }

    /// Within `margin` radians of the horizon line or a pole — hard steps a sampled frame cannot hold, so a
    /// truth comparison skips them.
    pub fn near_a_line(d: [f64; 3], margin: f64) -> bool {
        let (lon, lat) = Self::lon_lat(d);
        let pole = (lon / 0.35).round() * 0.35;
        lat.abs() < 0.004 + margin || (lat < margin && (lon - pole).abs() < 0.0035 + margin)
    }

    /// The scene's radiance (normalised raw, before the frame's gain) toward direction `d`.
    pub fn radiance(&self, d: [f64; 3]) -> [f32; 3] {
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let u = d.map(|c| c / n);
        let (lon, lat) = Self::lon_lat(u);
        let t: f64 = self
            .waves
            .iter()
            .map(|(k, ph, a)| a * (k[0] * u[0] + k[1] * u[1] + k[2] * u[2] + ph).sin())
            .sum::<f64>()
            / 4.0;
        // Above the horizon the texture fades into a sky that brightens toward the zenith.
        let sky = ((lat - 0.12) / 0.25).clamp(0.0, 1.0);
        let mut stops = -2.6 + t * (1.0 - 0.85 * sky) + sky * (1.0 + 1.2 * lat);
        if lat.abs() < 0.004 {
            stops -= 1.8;
        }
        let pole = (lon / 0.35).round() * 0.35;
        if lat < -0.004 && (lon - pole).abs() < 0.0035 {
            stops -= 1.6;
        }
        let l = 2f64.powf(stops);
        let tint = 0.12 * (t * 1.7).sin();
        [l * (1.0 + tint), l, l * (1.0 - tint * 0.8)].map(|c| c as f32)
    }
}

/// The ray of pixel `(x, y)` of a `w × h` frame at focal length `focal` px, in world directions.
pub fn ray(shot: &Shot, focal: f64, w: usize, h: usize, x: f64, y: f64) -> [f64; 3] {
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    apply(&shot.rotation(), [(x - cx) / focal, (y - cy) / focal, 1.0])
}

/// Where world direction `d` lands in a frame of `shot`, `None` behind it or outside the frame.
pub fn project(shot: &Shot, focal: f64, w: usize, h: usize, d: [f64; 3]) -> Option<[f64; 2]> {
    let c = apply_t(&shot.rotation(), d);
    if c[2] <= 1e-9 {
        return None;
    }
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    let p = [c[0] / c[2] * focal + cx, c[1] / c[2] * focal + cy];
    ((0.0..=(w - 1) as f64).contains(&p[0]) && (0.0..=(h - 1) as f64).contains(&p[1])).then_some(p)
}

/// Pixel `p` of `a`'s frame, as seen by `b`'s — the truth a match is judged against.
pub fn map(a: &Shot, b: &Shot, focal: f64, w: usize, h: usize, p: [f64; 2]) -> Option<[f64; 2]> {
    project(b, focal, w, h, ray(a, focal, w, h, p[0], p[1]))
}

/// The frames of `shots`, `w × h` at `focal` px: shot noise and read noise as `synth::bracket`'s,
/// clipped at libraw's ceiling, `PRE_MUL` applied, no EXIF exposure.
pub fn render(scene: &Scene, shots: &[Shot], w: usize, h: usize, focal: f64) -> Vec<Frame> {
    use rayon::prelude::*;
    shots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let rows: Vec<Vec<u16>> = (0..h)
                .into_par_iter()
                .map(|y| {
                    let mut rng = Noise(
                        (i as u64 + 1)
                            .wrapping_mul(1_000_003)
                            .wrapping_add(y as u64),
                    );
                    let mut row = Vec::with_capacity(w * 3);
                    let half_diag = ((w * w + h * h) as f64).sqrt() / 2.0;
                    for x in 0..w {
                        let d = ray(s, focal, w, h, x as f64, y as f64);
                        let mut r = scene.radiance(d);
                        if let Some([lon0, lat0, half]) = s.square {
                            let (lon, lat) = Scene::lon_lat(d);
                            if (lon - lon0).abs() < half && (lat - lat0).abs() < half {
                                r = r.map(|c| c * 0.15);
                            }
                        }
                        let rr = ((x as f64 - (w as f64 - 1.0) / 2.0).powi(2)
                            + (y as f64 - (h as f64 - 1.0) / 2.0).powi(2))
                        .sqrt()
                            / half_diag;
                        let fall = 1.0 - s.vignette * (rr * rr) as f32;
                        for c in 0..3 {
                            let e = (r[c] * s.gain * fall * PRE_MUL[c]) as f64 * 65535.0;
                            let noisy = e + rng.gauss() * ((e * 0.5).max(0.0).sqrt() * 2.0 + 3.0);
                            row.push(noisy.round().clamp(0.0, 65535.0) as u16);
                        }
                    }
                    row
                })
                .collect();
            Frame::new(w, h, rows.concat(), PRE_MUL, None)
        })
        .collect()
}

/// A synthetic row: its shots, frame size and focal length.
pub struct Row {
    pub shots: Vec<Shot>,
    pub w: usize,
    pub h: usize,
    pub focal: f64,
}

/// `n` frames turning right, neighbours sharing `overlap` of a frame's width; gains alternating ±0.5 EV
/// (the middle frame at 0), the second frame rolled 2°, pitches wandering ±0.6°, a light vignette.
/// Landscape frames are 480 × 320, portrait 320 × 480; the focal length is 420 px either way.
pub fn row(n: usize, overlap: f32, portrait: bool) -> Row {
    let (w, h) = if portrait { (320, 480) } else { (480, 320) };
    let focal = 420.0;
    let hfov = 2.0 * ((w as f64 / 2.0) / focal).atan();
    let step = hfov * (1.0 - overlap as f64);
    let mid = (n as f64 - 1.0) / 2.0;
    let shots = (0..n)
        .map(|i| {
            let k = i as f64 - mid;
            Shot {
                yaw: k * step,
                pitch: (0.6f64).to_radians() * ((i as f64 * 1.7).sin()),
                roll: if i == 1 { 2f64.to_radians() } else { 0.0 },
                gain: if i == n / 2 {
                    1.0
                } else if i % 2 == 0 {
                    2f32.powf(0.5)
                } else {
                    2f32.powf(-0.5)
                },
                vignette: 0.25,
                square: None,
            }
        })
        .collect();
    Row { shots, w, h, focal }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The share of `a`'s pixels that `b` also sees.
    fn shared(a: &Shot, b: &Shot, r: &Row) -> f64 {
        let mut n = 0;
        let step = 4;
        let mut total = 0;
        for y in (0..r.h).step_by(step) {
            for x in (0..r.w).step_by(step) {
                total += 1;
                if map(a, b, r.focal, r.w, r.h, [x as f64, y as f64]).is_some() {
                    n += 1;
                }
            }
        }
        n as f64 / total as f64
    }

    #[test]
    fn the_synthetic_row_overlaps_as_asked() {
        for overlap in [0.25f32, 0.4] {
            let r = row(4, overlap, false);
            for i in 0..3 {
                let s = shared(&r.shots[i], &r.shots[i + 1], &r);
                assert!(
                    (s - overlap as f64).abs() < 0.06,
                    "overlap {overlap}: frames {i}/{}: {s}",
                    i + 1
                );
            }
            assert!(
                shared(&r.shots[0], &r.shots[3], &r) < 0.01,
                "the ends do not overlap"
            );
        }
    }

    #[test]
    fn a_ray_projects_back_to_its_pixel() {
        let r = row(3, 0.3, true);
        let s = &r.shots[1];
        let p = project(
            s,
            r.focal,
            r.w,
            r.h,
            ray(s, r.focal, r.w, r.h, 37.25, 401.5),
        )
        .unwrap();
        assert!(
            (p[0] - 37.25).abs() < 1e-9 && (p[1] - 401.5).abs() < 1e-9,
            "{p:?}"
        );
    }

    #[test]
    fn the_scene_has_a_sky_and_a_textured_ground() {
        let s = Scene::new(1);
        let sky = s.radiance([0.0, -1.0, 1.2])[1];
        let ground = s.radiance([0.0, 0.5, 1.0])[1];
        assert!(sky > ground * 1.5, "sky {sky}, ground {ground}");
    }
}

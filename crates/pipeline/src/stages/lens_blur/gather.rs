//! The blur itself: a dense gather of aperture-shaped discs, level by level, in three layers.
//!
//! ⚠⚠ **Dense, not sampled** (a ruling while planning d1): a sparse sample pattern sees a point highlight
//! only where a sample lands on it, so a street light became a scatter of specks instead of a disc. Here
//! every pixel within a disc's reach is visited — at a working resolution chosen per SOURCE pixel from
//! its own radius ([`RCAP`]), so a disc never spans more than a few working pixels and the cost stays
//! nearly constant whatever the photograph's size.

use super::radius::{radius, Params};
use core_image::ImageBuf;

/// The largest disc radius, in working pixels, any level gathers — a source goes to the finest level
/// (½, ¼, …) at which its disc fits. Larger: crisper disc edges, slower.
pub const RCAP: usize = 8;

/// The blurred frame. `img` is linear; `depth` (0 near … 1 far) and `subject` (alpha 0..=1) are `w × h`
/// planes. In-focus pixels with nothing in front of them come out bit-identical to the input.
pub fn render(img: &ImageBuf, depth: &[f32], subject: Option<&[f32]>, p: &Params) -> ImageBuf {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || p.radius_px < 0.5 || depth.len() != w * h {
        return img.clone();
    }
    let alpha: Vec<f32> = match subject {
        Some(s) if s.len() == w * h => s
            .iter()
            .map(|&a| {
                if a.is_finite() {
                    a.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect(),
        _ => vec![0.0; w * h],
    };
    // Each pixel's radius (none inside the subject) and its layer: nearer than the sharp zone is the
    // foreground; the sharp zone and everything behind it, the background.
    let near_edge = p.focus - p.half_range;
    let c: Vec<f32> = (0..w * h)
        .map(|i| radius(p, depth[i]) * (1.0 - alpha[i]))
        .collect();
    let fg: Vec<bool> = depth
        .iter()
        .map(|&d| d.is_finite() && d < near_edge)
        .collect();
    let boosted = boost(img, p.boost);
    let mut acc = Accum::new(w * h);
    let max_c = c.iter().cloned().fold(0.0f32, f32::max);
    if max_c >= 0.5 {
        for l in 0..=level_of(max_c) {
            let lev = Level::build(&boosted, &c, &fg, &alpha, l);
            if lev.has_sources {
                lev.gather(p.bokeh).add_into(&mut acc, w, h, lev.k);
            }
        }
    }
    composite(img, &c, &alpha, &acc)
}

/// The level a source of radius `c` (full pixels, ≥ 0.5) is gathered at: the finest scale ½, ¼, …
/// at which its disc spans at most [`RCAP`] working pixels.
fn level_of(c: f32) -> usize {
    let mut l = 0;
    while c * 0.5f32.powi(l as i32 + 1) > RCAP as f32 {
        l += 1;
    }
    l
}

/// Lift the highlights well above the frame's typical brightness — `1 + 3·boost/100` at most — so they
/// carry the energy that makes discs. ⚠ Above the 99th percentile's luminance, rising to full at three
/// times it: a threshold AT the percentile would lift a plain mid-grey frame, every pixel of which is
/// its own 99th percentile.
fn boost(img: &ImageBuf, boost: f32) -> ImageBuf {
    if boost <= 0.0 {
        return img.clone();
    }
    let lum: Vec<f32> = img
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&c| color::luminance(c))
        .collect();
    let mut sorted: Vec<f32> = lum.iter().cloned().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return img.clone();
    }
    let at = ((sorted.len() - 1) as f32 * 0.99).round() as usize;
    let (_, l99, _) = sorted.select_nth_unstable_by(at, f32::total_cmp);
    let l99 = l99.max(1e-6);
    let k = 3.0 * boost.clamp(0.0, 100.0) / 100.0;
    let mut out = img.clone();
    for (px, &l) in out.data_mut().as_chunks_mut::<3>().0.iter_mut().zip(&lum) {
        let t = ((l - l99) / (2.0 * l99)).clamp(0.0, 1.0);
        let f = 1.0 + k * t * t * (3.0 - 2.0 * t);
        px.iter_mut().for_each(|v| *v *= f);
    }
    out
}

/// One scale's sources, two layers each (0 background, 1 foreground), per working pixel: the mean
/// colour, the fraction of the working pixel the layer's sources cover, and their mean radius in
/// working pixels.
struct Level {
    k: usize,
    w: usize,
    h: usize,
    col: [Vec<[f32; 3]>; 2],
    frac: [Vec<f32>; 2],
    rad: [Vec<f32>; 2],
    has_sources: bool,
}

impl Level {
    fn build(img: &ImageBuf, c: &[f32], fg: &[bool], alpha: &[f32], l: usize) -> Level {
        let (fw, fh) = (img.width(), img.height());
        let k = 1usize << (l + 1);
        let (w, h) = (fw.div_ceil(k), fh.div_ceil(k));
        let n = w * h;
        let mut col = [vec![[0f32; 3]; n], vec![[0f32; 3]; n]];
        let mut frac = [vec![0f32; n], vec![0f32; n]];
        let mut rad = [vec![0f32; n], vec![0f32; n]];
        let mut has_sources = false;
        for y in 0..fh {
            for x in 0..fw {
                let i = y * fw + x;
                let wgt = 1.0 - alpha[i];
                if c[i] < 0.5 || wgt <= 0.0 || level_of(c[i]) != l {
                    continue;
                }
                has_sources = true;
                let (layer, j) = (fg[i] as usize, (y / k) * w + x / k);
                let px = img.pixel(x, y);
                for ch in 0..3 {
                    col[layer][j][ch] += px[ch] * wgt;
                }
                frac[layer][j] += wgt;
                rad[layer][j] += c[i] * wgt;
            }
        }
        let area = (k * k) as f32;
        for layer in 0..2 {
            for j in 0..n {
                let f = frac[layer][j];
                if f > 0.0 {
                    col[layer][j] = col[layer][j].map(|v| v / f);
                    rad[layer][j] = rad[layer][j] / f / k as f32;
                    frac[layer][j] = f / area;
                }
            }
        }
        Level {
            k,
            w,
            h,
            col,
            frac,
            rad,
            has_sources,
        }
    }

    /// Every working pixel's received light — a dense pass over the disc of reach [`RCAP`] around it.
    fn gather(&self, kind: edit_model::Bokeh) -> Gathered {
        use rayon::prelude::*;
        let (w, h, k) = (self.w, self.h, self.k);
        let (fw, fh) = ((w * k) as f32, (h * k) as f32);
        let pos = |j: usize| {
            [
                (((j % w) as f32 + 0.5) * k as f32 / fw).clamp(0.0, 1.0),
                (((j / w) as f32 + 0.5) * k as f32 / fh).clamp(0.0, 1.0),
            ]
        };
        // Each source's normaliser: 1 / the light-spreading footprint — exact over the receivers for a
        // small disc (an analytic area would over-brighten a disc smaller than a pixel), the shape's
        // area beyond.
        let norm: [Vec<f32>; 2] = [0, 1].map(|layer| {
            (0..w * h)
                .into_par_iter()
                .map(|j| {
                    if self.frac[layer][j] <= 0.0 {
                        return 0.0;
                    }
                    let (rq, at) = (self.rad[layer][j].max(1e-3), pos(j));
                    if rq >= 3.0 {
                        return 1.0 / (rq * rq * super::aperture::area(kind, at));
                    }
                    let reach = rq.ceil() as i32 + 1;
                    let mut sum = 0.0;
                    for dy in -reach..=reach {
                        for dx in -reach..=reach {
                            sum += cover(kind, dx, dy, rq, at);
                        }
                    }
                    if sum > 0.0 {
                        1.0 / sum
                    } else {
                        0.0
                    }
                })
                .collect()
        });
        let r = RCAP as i32;
        let mut out = Gathered::new(w * h);
        out.rows_mut(w).enumerate().for_each(|(py, row)| {
            for (px, cell) in row.iter_mut().enumerate() {
                for dy in -r..=r {
                    let qy = py as i32 + dy;
                    if qy < 0 || qy >= h as i32 {
                        continue;
                    }
                    for dx in -r..=r {
                        let qx = px as i32 + dx;
                        if qx < 0 || qx >= w as i32 {
                            continue;
                        }
                        let q = qy as usize * w + qx as usize;
                        let d2 = (dx * dx + dy * dy) as f32;
                        for (layer, norm) in norm.iter().enumerate() {
                            let f = self.frac[layer][q];
                            if f <= 0.0 {
                                continue;
                            }
                            let rq = self.rad[layer][q].max(1e-3);
                            if d2 > (rq + 1.0) * (rq + 1.0) {
                                continue;
                            }
                            // The receiver as seen from the source's disc: `p − q`.
                            let sw = cover(kind, -dx, -dy, rq, pos(q));
                            if sw <= 0.0 {
                                continue;
                            }
                            let e = f * sw * norm[q];
                            cell.add(layer, self.col[layer][q], e);
                        }
                    }
                }
            }
        });
        out
    }
}

/// How much of the receiver pixel at integer offset `(dx, dy)` a disc of radius `rq` (working pixels)
/// covers, weighted by the aperture.
///
/// ⚠ **Anti-aliased:** a pixel straddling the disc's edge is tested at 2 × 2 sub-positions and averaged.
/// A hard test at the pixel's centre makes a ragged disc on the pixel grid — it reaches its full radius
/// along the axes and falls short between them (measured 17.75 vs 16 px on a circle of 16), which at a
/// coarse level is a visibly octagonal highlight. Away from the edge one test at the centre suffices.
fn cover(kind: edit_model::Bokeh, dx: i32, dy: i32, rq: f32, pos: [f32; 2]) -> f32 {
    let dist = ((dx * dx + dy * dy) as f32).sqrt();
    // A pixel's half-diagonal.
    let half = std::f32::consts::FRAC_1_SQRT_2;
    if (dist - half) / rq > 1.0 {
        return 0.0;
    }
    // Inside this fraction of the radius every shape is either fully open or smooth (the ring).
    let inner = match kind {
        edit_model::Bokeh::Blades => (std::f32::consts::PI / 6.0).cos(),
        _ => 0.5,
    };
    let at = |x: f32, y: f32| super::aperture::weight(kind, [x / rq, y / rq], pos);
    let (x, y) = (dx as f32, dy as f32);
    if (dist + half) / rq <= inner {
        return at(x, y);
    }
    0.25 * (at(x - 0.25, y - 0.25)
        + at(x + 0.25, y - 0.25)
        + at(x - 0.25, y + 0.25)
        + at(x + 0.25, y + 0.25))
}

/// One working pixel's received light, per layer: the light's sum and its weight.
#[derive(Clone, Copy, Default)]
struct Cell {
    num: [[f32; 3]; 2],
    den: [f32; 2],
}

impl Cell {
    fn add(&mut self, layer: usize, col: [f32; 3], e: f32) {
        for (n, c) in self.num[layer].iter_mut().zip(col) {
            *n += c * e;
        }
        self.den[layer] += e;
    }
}

struct Gathered(Vec<Cell>);

impl Gathered {
    fn new(n: usize) -> Self {
        Gathered(vec![Cell::default(); n])
    }

    fn rows_mut(
        &mut self,
        w: usize,
    ) -> impl rayon::iter::IndexedParallelIterator<Item = &mut [Cell]> {
        use rayon::prelude::*;
        self.0.par_chunks_mut(w)
    }

    /// Add this level, bilinearly upsampled, into the full-resolution sums.
    fn add_into(&self, acc: &mut Accum, fw: usize, fh: usize, k: usize) {
        use rayon::prelude::*;
        let (w, h) = (fw.div_ceil(k), fh.div_ceil(k));
        let cells = &self.0;
        acc.0.par_chunks_mut(fw).enumerate().for_each(|(y, row)| {
            let v = ((y as f32 + 0.5) / k as f32 - 0.5).clamp(0.0, (h - 1) as f32);
            let (y0, ty) = (v.floor() as usize, v.fract());
            let y1 = (y0 + 1).min(h - 1);
            for (x, a) in row.iter_mut().enumerate() {
                let u = ((x as f32 + 0.5) / k as f32 - 0.5).clamp(0.0, (w - 1) as f32);
                let (x0, tx) = (u.floor() as usize, u.fract());
                let x1 = (x0 + 1).min(w - 1);
                let taps = [
                    (y0 * w + x0, (1.0 - tx) * (1.0 - ty)),
                    (y0 * w + x1, tx * (1.0 - ty)),
                    (y1 * w + x0, (1.0 - tx) * ty),
                    (y1 * w + x1, tx * ty),
                ];
                for (j, t) in taps {
                    if t <= 0.0 {
                        continue;
                    }
                    let c = &cells[j];
                    for layer in 0..2 {
                        for ch in 0..3 {
                            a.num[layer][ch] += c.num[layer][ch] * t;
                        }
                        a.den[layer] += c.den[layer] * t;
                    }
                }
            }
        });
    }
}

/// The full-resolution sums, every level's added in.
struct Accum(Vec<Cell>);

impl Accum {
    fn new(n: usize) -> Self {
        Accum(vec![Cell::default(); n])
    }
}

/// The three layers, back to front: the blurred background (normalised — where the subject hid it,
/// filled from what surrounds it), the sharp pixels over it, the foreground's blur over everything.
///
/// ⚠⚠ Where a pixel is in focus (`c == 0`), not in the subject's partial alpha, and no foreground
/// reaches it, it is returned AS IS — no arithmetic at all, so it is bit-identical to the input.
fn composite(img: &ImageBuf, c: &[f32], alpha: &[f32], acc: &Accum) -> ImageBuf {
    use rayon::prelude::*;
    let w = img.width();
    let mut out = img.clone();
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    out.data_mut()
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let i = y * w + x;
                let src = [px[0], px[1], px[2]];
                let cell = &acc.0[i];
                // Sharp where the blur is under a pixel and a half — blended into the blur below it.
                let t = (1.0 - c[i] / 1.5).clamp(0.0, 1.0);
                let mut v = if t >= 1.0 {
                    src
                } else if cell.den[0] > 1e-6 {
                    let bg = cell.num[0].map(|n| n / cell.den[0]);
                    lerp(bg, src, t)
                } else {
                    src
                };
                let a = alpha[i];
                if a >= 1.0 {
                    v = src;
                } else if a > 0.0 {
                    v = lerp(v, src, a);
                }
                let cover = cell.den[1].min(1.0);
                if cover > 0.0 {
                    let fg = cell.num[1].map(|n| n / cell.den[1]);
                    v = lerp(v, fg, cover);
                }
                px.copy_from_slice(&v);
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::Bokeh;

    fn params(focus: f32, radius_px: f32, bokeh: Bokeh) -> Params {
        Params {
            focus,
            half_range: 0.0,
            radius_px,
            bokeh,
            boost: 0.0,
        }
    }

    /// A `w × h` frame from a function of `(x, y)` → (colour, depth).
    fn scene(
        w: usize,
        h: usize,
        f: impl Fn(usize, usize) -> ([f32; 3], f32),
    ) -> (ImageBuf, Vec<f32>) {
        let mut img = ImageBuf::new(w, h);
        let mut depth = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (c, d) = f(x, y);
                img.set_pixel(x, y, c);
                depth[y * w + x] = d;
            }
        }
        (img, depth)
    }

    /// A 2 × 2 point of `value` at `(cx, cy)` on black, everything at depth 1, focus 0.
    fn point(w: usize, h: usize, cx: usize, cy: usize, value: f32) -> (ImageBuf, Vec<f32>) {
        scene(w, h, |x, y| {
            let lit = (cx..cx + 2).contains(&x) && (cy..cy + 2).contains(&y);
            ([if lit { value } else { 0.0 }; 3], 1.0)
        })
    }

    fn total(img: &ImageBuf) -> f64 {
        img.data().iter().map(|&v| v as f64).sum::<f64>()
    }

    /// The farthest lit pixel from `(cx, cy)` along the ray at `deg`, lit meaning above 5 % of the peak.
    fn reach(img: &ImageBuf, cx: f32, cy: f32, deg: f32) -> f32 {
        let peak = img.data().iter().cloned().fold(0.0f32, f32::max);
        let (s, c) = deg.to_radians().sin_cos();
        let mut far = 0.0;
        for i in 0..400 {
            let t = i as f32 * 0.25;
            let (x, y) = (cx + t * c, cy + t * s);
            if x < 0.0 || y < 0.0 || x >= img.width() as f32 || y >= img.height() as f32 {
                break;
            }
            if img.pixel(x as usize, y as usize)[0] > 0.05 * peak {
                far = t;
            }
        }
        far
    }

    #[test]
    fn a_point_behind_focus_becomes_a_disc_and_keeps_its_light() {
        let (img, depth) = point(160, 100, 80, 50, 50.0);
        let out = render(&img, &depth, None, &params(0.0, 12.0, Bokeh::Circle));
        let r = reach(&out, 81.0, 51.0, 0.0);
        assert!((r - 12.0).abs() <= 2.0, "the disc's radius is {r}");
        let (before, after) = (total(&img), total(&out));
        assert!(
            (after / before - 1.0).abs() < 0.02,
            "light {before} → {after}"
        );
    }

    #[test]
    fn each_shape_draws_its_disc() {
        let (img, depth) = point(200, 200, 100, 100, 50.0);
        let disc = |kind, pos_x: usize| {
            let (img, depth) = if pos_x == 100 {
                (img.clone(), depth.clone())
            } else {
                point(200, 200, pos_x, 100, 50.0)
            };
            render(&img, &depth, None, &params(0.0, 16.0, kind))
        };
        let circle = disc(Bokeh::Circle, 100);
        let (c0, c30) = (
            reach(&circle, 101.0, 101.0, 0.0),
            reach(&circle, 101.0, 101.0, 30.0),
        );
        assert!((c0 - c30).abs() <= 1.0, "a circle is round: {c0} vs {c30}");
        let blades = disc(Bokeh::Blades, 100);
        let (b0, b30) = (
            reach(&blades, 101.0, 101.0, 0.0),
            reach(&blades, 101.0, 101.0, 30.0),
        );
        assert!(b0 > b30 * 1.08, "a hexagon has corners: {b0} vs {b30}");
        let ring = disc(Bokeh::Ring, 100);
        assert!(
            ring.pixel(101, 101)[0] < ring.pixel(113, 101)[0],
            "a ring's rim is brighter"
        );
        let centred = disc(Bokeh::CatEye, 100);
        let (e0, e90) = (
            reach(&centred, 101.0, 101.0, 0.0),
            reach(&centred, 101.0, 101.0, 90.0),
        );
        assert!(
            (e0 - e90).abs() <= 1.0,
            "a cat-eye is round at the centre: {e0} vs {e90}"
        );
        let edge = disc(Bokeh::CatEye, 180);
        let (r0, r90) = (
            reach(&edge, 181.0, 101.0, 180.0),
            reach(&edge, 181.0, 101.0, 90.0),
        );
        assert!(
            r0 < r90 * 0.9,
            "toward the edge it narrows radially: {r0} vs {r90}"
        );
    }

    /// The halo: a sharp red subject in front of a blurred green-blue texture. Beside its outline the
    /// blurred background must not take on its red.
    #[test]
    fn no_halo_behind_a_sharp_subject() {
        let (w, h) = (160, 120);
        let inside = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 80.0, y as f32 - 60.0);
            dx * dx + dy * dy < 30.0 * 30.0
        };
        let texture = |x: usize, y: usize| [0.05, 0.2 + 0.1 * ((x / 4 + y / 4) % 2) as f32, 0.3];
        let (img, depth) = scene(w, h, |x, y| {
            if inside(x, y) {
                ([0.9, 0.05, 0.05], 0.2)
            } else {
                (texture(x, y), 1.0)
            }
        });
        let subject: Vec<f32> = (0..w * h)
            .map(|i| if inside(i % w, i / w) { 1.0 } else { 0.0 })
            .collect();
        let out = render(
            &img,
            &depth,
            Some(&subject),
            &params(0.2, 16.0, Bokeh::Circle),
        );
        let mut worst = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - 80.0, y as f32 - 60.0);
                let d = (dx * dx + dy * dy).sqrt();
                if (31.0..37.0).contains(&d) {
                    worst = worst.max(out.pixel(x, y)[0] - 0.05);
                }
            }
        }
        assert!(
            worst < 0.02,
            "the subject's red reached the background: +{worst}"
        );
        // ⚠ The other halo — a DARK rim, where the subject hid the background and nothing filled the
        // hole: the blurred background beside the outline is as bright as farther out.
        let ring = |r0: f32, r1: f32| {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy) = (x as f32 - 80.0, y as f32 - 60.0);
                    if (r0..r1).contains(&(dx * dx + dy * dy).sqrt()) {
                        sum += out.pixel(x, y)[1];
                        n += 1.0;
                    }
                }
            }
            sum / n
        };
        let (near, far) = (ring(31.0, 37.0), ring(46.0, 52.0));
        assert!(
            near > far * 0.9,
            "a dark rim beside the subject: {near} vs {far}"
        );
        // And the subject itself is untouched.
        assert_eq!(out.pixel(80, 60), img.pixel(80, 60));
    }

    #[test]
    fn a_foreground_spreads_over_the_subject() {
        let (w, h) = (160, 100);
        let (img, depth) = scene(w, h, |x, _| {
            if (40..48).contains(&x) {
                ([1.0; 3], 0.0) // a white bar near the camera
            } else {
                ([0.2; 3], 0.5) // the subject's plane
            }
        });
        let out = render(&img, &depth, None, &params(0.5, 16.0, Bokeh::Circle));
        // Partial, and fading: a blurred bar spreads its light thinner the farther it reaches.
        let (beside, farther) = (out.pixel(54, 50)[0], out.pixel(62, 50)[0]);
        assert!(
            beside > 0.25 && beside < 0.6,
            "the bar's blur beside it is {beside} — none, or opaque"
        );
        assert!(
            farther < beside,
            "the blur does not fade: {farther} vs {beside}"
        );
        assert_eq!(
            out.pixel(120, 50),
            img.pixel(120, 50),
            "far from the bar, in focus, untouched"
        );
    }

    #[test]
    fn in_focus_pixels_are_the_input_exactly() {
        let (img, depth) = scene(120, 80, |x, y| {
            let v = 0.1 + 0.8 * ((x * 7 + y * 13) % 17) as f32 / 17.0;
            ([v, v * 0.9, v * 0.8], if x < 60 { 0.5 } else { 1.0 })
        });
        let out = render(&img, &depth, None, &params(0.5, 10.0, Bokeh::Circle));
        // ⚠ All of the in-focus half, the columns the far half's blurred light reaches (x ≥ 50)
        // included — checking only x < 40, out of its reach, could not see arithmetic done on a pixel
        // that should have been passed through (that sabotage was inert, 2026-10-14).
        for y in 0..80 {
            for x in 0..60 {
                assert_eq!(
                    out.pixel(x, y).map(f32::to_bits),
                    img.pixel(x, y).map(f32::to_bits),
                    "({x},{y})"
                );
            }
        }
        assert_ne!(
            out.pixel(100, 40),
            img.pixel(100, 40),
            "the far half is blurred"
        );
    }

    #[test]
    fn boost_brightens_highlight_discs_and_leaves_midtones() {
        let (img, depth) = scene(160, 100, |x, y| {
            let lit = (80..82).contains(&x) && (50..52).contains(&y);
            ([if lit { 5.0 } else { 0.18 }; 3], 1.0)
        });
        let with = |boost| {
            render(
                &img,
                &depth,
                None,
                &Params {
                    boost,
                    ..params(0.0, 12.0, Bokeh::Circle)
                },
            )
        };
        let (plain, boosted) = (with(0.0), with(100.0));
        assert!(
            boosted.pixel(81, 51)[0] > plain.pixel(81, 51)[0] * 1.3,
            "the disc did not brighten"
        );
        assert!(
            (boosted.pixel(20, 20)[0] - plain.pixel(20, 20)[0]).abs() < 1e-4,
            "mid-grey moved"
        );
    }

    /// Review Focus 3: degenerate frames and the strongest blur stay finite and keep their size.
    #[test]
    fn tiny_frames_and_full_amount_stay_finite() {
        for (w, h) in [(1, 1), (3, 2), (64, 40)] {
            let (img, depth) = scene(w, h, |x, y| ([((x + y) % 3) as f32; 3], (x % 2) as f32));
            let out = render(
                &img,
                &depth,
                None,
                &params(0.0, 0.03 * w.max(h) as f32, Bokeh::CatEye),
            );
            assert_eq!((out.width(), out.height()), (w, h));
            assert!(out.data().iter().all(|v| v.is_finite()));
        }
        let (img, depth) = scene(64, 40, |_, _| ([0.5; 3], f32::NAN));
        let out = render(&img, &depth, None, &params(0.5, 30.0, Bokeh::Circle));
        assert!(out.data().iter().all(|v| v.is_finite()));
    }
}

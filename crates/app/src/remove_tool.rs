//! The Remove brush's fill (M10.16c1): the context window around a stroke, the stroke's soft mask, the
//! network's run and the patch it leaves — no egui, and no network of its own: the caller hands one in.
//!
//! ⚠ Everything here is in DECODE pixels (`Removal`'s space): the caller hands the undeveloped decode —
//! through the negative's positive (*Warnings*) — at full resolution, and the patch it gets back is
//! normalised over it, so it lands on every preview rung and the export alike.

use crate::ai_denoise;
use core_image::ImageBuf;
use edit_model::Removal;
use rayon::prelude::*;

/// The network's side — the window is scaled to it, and the fill is kept at it.
///
/// ⚠ The fill is stored at 512 × 512 whatever the window: everything inside it came out of a 512 square,
/// so a decode-resolution copy would only be bigger (a 3000-px window: 144 MB against 4 MB), not sharper.
pub const SIDE: usize = ai_mask::inpaint::SIDE;
/// The window's side against the hole's extent — the context LaMa fills from.
pub const CONTEXT: f32 = 2.5;
/// The window's floor in decode pixels, where the decode allows: a small object still sees 512 px.
pub const MIN_SIDE: usize = SIDE;
/// The stroke's soft edge: alpha 1 out to its radius, 0 at this × its radius — and the network's hole is
/// everywhere the alpha is above 0, so every pixel the paste touches was filled.
pub const HOLE_GROW: f32 = 1.25;
/// At most this many of the stroke's points prompt the detector — its decoder is built per point count.
pub const MAX_PROMPT_POINTS: usize = 8;
/// Detect objects grows the object it found by this many network pixels, then softens it by half that.
const DETECT_GROW: usize = 6;
/// Refresh's direction step: each variant moves the window a golden angle further round.
const GOLDEN_ANGLE: f32 = 2.399_963;
/// ⚠ In every fill's key: bump it on ANY change to how a fill is made, or the old ones are served.
/// v2 (2026-10-19): the window encoded by its peak channel — v1's fills of a sky came back green.
/// v3 (2026-10-21, M10.16c3): the detector sees the window upright — Detect objects' fills on a portrait change.
const RECIPE: &str = "remove:v3";

/// The inpainting network: [`SIDE`]² display-referred RGB (interleaved, 0..1) and a hole mask (1 = fill)
/// → the same RGB filled.
pub type Net<'a> = &'a mut dyn FnMut(&[f32], &[f32]) -> Result<Vec<f32>, String>;

/// The object finder: the same RGB and the prompt's points and box (normalised over the window) → a
/// probability per pixel.
pub type Detect<'a> = &'a mut dyn FnMut(&[f32], &[[f32; 2]], [f32; 4]) -> Result<Vec<f32>, String>;

/// Whether `r`'s fill asks the object finder: Detect objects' strokes, and every person (M10.16c3) — whose hole
/// is the finder's object alone.
pub fn needs_detector(r: &Removal) -> bool {
    r.detect || r.person.is_some()
}

/// The Remove brush between strokes (M10.16c1) — Retouch's first and default mode.
///
/// ⚠ On the app beside `SpotTool` and `EyeTool`: the mode the panel shows is derived from the three.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoveTool {
    /// Remove is the mode — off in Heal, Clone and the eye modes.
    pub active: bool,
    /// Detect objects, for the strokes still to be painted.
    pub detect: bool,
    /// The brush radius, a fraction of the decode's long edge.
    pub radius: f32,
}

impl Default for RemoveTool {
    fn default() -> Self {
        Self {
            active: true,
            detect: false,
            radius: 0.02,
        }
    }
}

/// A painted point joins the stroke only this far (× the radius) from the last one — dense enough for
/// a smooth band, sparse enough that a slow drag does not pile up hundreds of points.
pub const POINT_SPACING: f32 = 0.25;

/// `p` (decode-normalised) onto `points` when it is at least [`POINT_SPACING`] × the radius from the last
/// one, in decode pixels of a `w × h` decode; the first always.
pub fn add_point(points: &mut Vec<[f32; 2]>, p: [f32; 2], radius: f32, w: usize, h: usize) {
    let far = match points.last() {
        None => true,
        Some(q) => {
            let (dx, dy) = ((p[0] - q[0]) * w as f32, (p[1] - q[1]) * h as f32);
            (dx * dx + dy * dy).sqrt() >= POINT_SPACING * radius * w.max(h) as f32
        }
    };
    if far && p.iter().all(|v| v.is_finite()) {
        points.push(p);
    }
}

/// The topmost removal whose stroke, soft edge included, covers decode-normalised `at` of a `w × h`
/// decode — the one a click selects.
pub fn hit_test(removals: &[Removal], at: [f32; 2], w: usize, h: usize) -> Option<usize> {
    let (x, y) = (at[0] * w as f32, at[1] * h as f32);
    removals.iter().enumerate().rev().find_map(|(i, r)| {
        // M10.16c3: a person is hit anywhere inside their box.
        if let Some(b) = r.person {
            let inside = (b[0]..=b[2]).contains(&at[0]) && (b[1]..=b[3]).contains(&at[1]);
            return inside.then_some(i);
        }
        let pts: Vec<[f32; 2]> = r
            .points
            .iter()
            .map(|p| [p[0] * w as f32, p[1] * h as f32])
            .collect();
        let reach = r.radius.max(0.0) * w.max(h) as f32 * HOLE_GROW;
        (distance(&pts, x, y) <= reach).then_some(i)
    })
}

/// The hole's box in decode pixels — the stroke's points grown by its soft edge — clamped to the decode.
fn hole_box(r: &Removal, w: usize, h: usize) -> [f32; 4] {
    // M10.16c3: a person's hole is inside their box.
    if let Some(b) = r.person {
        return [
            (b[0] * w as f32).clamp(0.0, w as f32),
            (b[1] * h as f32).clamp(0.0, h as f32),
            (b[2] * w as f32).clamp(0.0, w as f32),
            (b[3] * h as f32).clamp(0.0, h as f32),
        ];
    }
    let rad = r.radius.max(0.0) * w.max(h) as f32 * HOLE_GROW;
    let (mut b, mut any) = ([f32::MAX, f32::MAX, f32::MIN, f32::MIN], false);
    for p in &r.points {
        let (x, y) = (p[0] * w as f32, p[1] * h as f32);
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        any = true;
        b = [
            b[0].min(x - rad),
            b[1].min(y - rad),
            b[2].max(x + rad),
            b[3].max(y + rad),
        ];
    }
    if !any {
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        b = [cx, cy, cx, cy];
    }
    [
        b[0].clamp(0.0, w as f32),
        b[1].clamp(0.0, h as f32),
        b[2].clamp(0.0, w as f32),
        b[3].clamp(0.0, h as f32),
    ]
}

/// The decode crop a removal is filled from, `[x0, y0, x1, y1]` in decode pixels: a square of
/// [`CONTEXT`] × the hole's extent, at least [`MIN_SIDE`], inside the decode, holding the whole hole.
/// `variant` (Refresh) scales it by 1, 1.25 or 1.5 and moves it a golden angle round per step.
pub fn window(r: &Removal, w: usize, h: usize) -> [usize; 4] {
    if w == 0 || h == 0 {
        return [0, 0, w, h];
    }
    let b = hole_box(r, w, h);
    let ext = (b[2] - b[0]).max(b[3] - b[1]);
    let scale = 1.0 + 0.25 * (r.variant % 3) as f32;
    let want = (ext * CONTEXT * scale).max(MIN_SIDE as f32);
    let sw = (want.round() as usize).clamp(1, w);
    let sh = (want.round() as usize).clamp(1, h);
    let (mut cx, mut cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    if r.variant > 0 {
        let a = r.variant as f32 * GOLDEN_ANGLE;
        cx += 0.15 * want * a.cos();
        cy += 0.15 * want * a.sin();
    }
    // Wherever the window can hold the hole, it does.
    let keep = |c: f32, lo: f32, hi: f32, s: f32| {
        if hi - lo <= s {
            c.clamp(hi - s / 2.0, lo + s / 2.0)
        } else {
            (lo + hi) / 2.0
        }
    };
    let cx = keep(cx, b[0], b[2], sw as f32);
    let cy = keep(cy, b[1], b[3], sh as f32);
    let x0 = (cx - sw as f32 / 2.0).round().clamp(0.0, (w - sw) as f32) as usize;
    let y0 = (cy - sh as f32 / 2.0).round().clamp(0.0, (h - sh) as f32) as usize;
    [x0, y0, x0 + sw, y0 + sh]
}

/// The distance from decode pixel `(x, y)` to the stroke's polyline, in decode pixels.
fn distance(points: &[[f32; 2]], x: f32, y: f32) -> f32 {
    let seg = |a: [f32; 2], b: [f32; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx * dx + dy * dy;
        let t = if len > 0.0 {
            (((x - a[0]) * dx + (y - a[1]) * dy) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ((a[0] + t * dx - x).powi(2) + (a[1] + t * dy - y).powi(2)).sqrt()
    };
    match points {
        [] => f32::MAX,
        [p] => seg(*p, *p),
        _ => points
            .windows(2)
            .map(|s| seg(s[0], s[1]))
            .fold(f32::MAX, f32::min),
    }
}

/// The stroke's soft alpha over `crop` (decode pixels of a `w × h` decode) at `side × side`: 1 out to its
/// radius, a smoothstep to 0 at [`HOLE_GROW`] × it.
pub fn mask(r: &Removal, crop: [usize; 4], w: usize, h: usize, side: usize) -> Vec<f32> {
    let pts: Vec<[f32; 2]> = r
        .points
        .iter()
        .map(|p| [p[0] * w as f32, p[1] * h as f32])
        .collect();
    let rad = r.radius.max(0.0) * w.max(h) as f32;
    let outer = rad * HOLE_GROW;
    let (cw, ch) = ((crop[2] - crop[0]) as f32, (crop[3] - crop[1]) as f32);
    (0..side * side)
        .into_par_iter()
        .map(|i| {
            let x = crop[0] as f32 + ((i % side) as f32 + 0.5) / side as f32 * cw;
            let y = crop[1] as f32 + ((i / side) as f32 + 0.5) / side as f32 * ch;
            let d = distance(&pts, x, y);
            if d <= rad {
                1.0
            } else if d >= outer {
                0.0
            } else {
                let t = (outer - d) / (outer - rad);
                t * t * (3.0 - 2.0 * t)
            }
        })
        .collect()
}

/// `win` of `img` at `side × side`, interleaved: each output pixel the mean of the decode pixels under
/// it, or a bilinear sample where it is smaller than one.
fn resample(img: &ImageBuf, win: [usize; 4], side: usize) -> Vec<f32> {
    let (ww, wh) = ((win[2] - win[0]) as f32, (win[3] - win[1]) as f32);
    let (sx, sy) = (ww / side as f32, wh / side as f32);
    let (iw, ih) = (img.width(), img.height());
    (0..side * side)
        .into_par_iter()
        .flat_map_iter(|i| {
            let (fx, fy) = ((i % side) as f32, (i / side) as f32);
            let (x0, x1) = (win[0] as f32 + fx * sx, win[0] as f32 + (fx + 1.0) * sx);
            let (y0, y1) = (win[1] as f32 + fy * sy, win[1] as f32 + (fy + 1.0) * sy);
            let px = if sx >= 1.0 && sy >= 1.0 {
                let (a, b) = (
                    x0.floor() as usize,
                    (x1.ceil() as usize).min(iw).max(x0 as usize + 1),
                );
                let (c, d) = (
                    y0.floor() as usize,
                    (y1.ceil() as usize).min(ih).max(y0 as usize + 1),
                );
                let mut sum = [0f32; 3];
                for y in c..d {
                    for x in a..b {
                        let p = img.pixel(x, y);
                        for k in 0..3 {
                            sum[k] += p[k];
                        }
                    }
                }
                let n = ((b - a) * (d - c)).max(1) as f32;
                sum.map(|v| v / n)
            } else {
                let (x, y) = (
                    ((x0 + x1) / 2.0 - 0.5).clamp(0.0, (iw - 1) as f32),
                    ((y0 + y1) / 2.0 - 0.5).clamp(0.0, (ih - 1) as f32),
                );
                let (xa, ya) = (x.floor() as usize, y.floor() as usize);
                let (xb, yb) = ((xa + 1).min(iw - 1), (ya + 1).min(ih - 1));
                let (tx, ty) = (x - xa as f32, y - ya as f32);
                let (p00, p10, p01, p11) = (
                    img.pixel(xa, ya),
                    img.pixel(xb, ya),
                    img.pixel(xa, yb),
                    img.pixel(xb, yb),
                );
                std::array::from_fn(|k| {
                    (p00[k] * (1.0 - tx) + p10[k] * tx) * (1.0 - ty)
                        + (p01[k] * (1.0 - tx) + p11[k] * tx) * ty
                })
            };
            px.into_iter()
        })
        .collect()
}

/// The encoding a window is filled in: its 99.5th-percentile CHANNEL at `ai_denoise::WHITE`.
///
/// ⚠⚠ Not `Encoding::of`, which scales by LUMINANCE — right for denoise, wrong here: in a blue sky the blue
/// channel sits far above the luminance, clipped at 1, and the fill came back with 42 % of its blue gone —
/// the green tint the author saw (2026-10-19, `the_fill_keeps_a_bright_sky_s_colour`).
fn peak_encoding(img: &ImageBuf) -> ai_denoise::Encoding {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return ai_denoise::Encoding { scale: 1.0 };
    }
    let step = w.max(h).div_ceil(pipeline::lens_blur::GRID).max(1);
    let mut peaks: Vec<f32> = (0..h)
        .step_by(step)
        .flat_map(|y| (0..w).step_by(step).map(move |x| (x, y)))
        .map(|(x, y)| {
            color::prophoto_to_srgb(img.pixel(x, y))
                .into_iter()
                .fold(0f32, f32::max)
        })
        .filter(|v| v.is_finite())
        .collect();
    if peaks.is_empty() {
        return ai_denoise::Encoding { scale: 1.0 };
    }
    let k = ((peaks.len() - 1) as f32 * 0.995).round() as usize;
    let (_, p, _) = peaks.select_nth_unstable_by(k, f32::total_cmp);
    ai_denoise::Encoding {
        scale: ai_denoise::WHITE / p.max(1e-6),
    }
}

/// Rows of a `side × side` plane of `n` channels mirrored left to right, in place.
fn mirror(v: &mut [f32], side: usize, n: usize) {
    for row in v.chunks_mut(side * n) {
        for x in 0..side / 2 {
            for c in 0..n {
                row.swap(x * n + c, (side - 1 - x) * n + c);
            }
        }
    }
}

/// `plane` (0..1, `side²`) above 0.5, grown by `by` pixels — a square max filter, two passes.
fn dilate(plane: &[f32], side: usize, by: usize) -> Vec<f32> {
    let on: Vec<bool> = plane.iter().map(|&v| v >= 0.5).collect();
    let rows: Vec<bool> = (0..side * side)
        .map(|i| {
            let (x, y) = (i % side, i / side);
            (x.saturating_sub(by)..(x + by + 1).min(side)).any(|xx| on[y * side + xx])
        })
        .collect();
    (0..side * side)
        .map(|i| {
            let (x, y) = (i % side, i / side);
            (y.saturating_sub(by)..(y + by + 1).min(side)).any(|yy| rows[yy * side + x]) as u8
                as f32
        })
        .collect()
}

/// `plane` averaged over a `(2 r + 1)²` box, edges clamped — two passes.
fn soften(plane: &[f32], side: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let (lo, hi) = if horizontal {
                    (x.saturating_sub(r), (x + r + 1).min(side))
                } else {
                    (y.saturating_sub(r), (y + r + 1).min(side))
                };
                let sum: f32 = (lo..hi)
                    .map(|k| {
                        if horizontal {
                            src[y * side + k]
                        } else {
                            src[k * side + x]
                        }
                    })
                    .sum();
                sum / (hi - lo) as f32
            })
            .collect()
    };
    pass(&pass(plane, true), false)
}

/// The detector's prompt: up to [`MAX_PROMPT_POINTS`] of the stroke's points, evenly spaced, and the
/// stroke's box grown by 15 % a side — all normalised over `win`.
fn prompt(r: &Removal, win: [usize; 4], w: usize, h: usize) -> (Vec<[f32; 2]>, [f32; 4]) {
    let (ww, wh) = ((win[2] - win[0]) as f32, (win[3] - win[1]) as f32);
    // M10.16c3: a person by their box alone, as the network found it — the spike prompted MobileSAM so.
    if let Some(b) = r.person {
        return (
            Vec::new(),
            [
                ((b[0] * w as f32 - win[0] as f32) / ww).clamp(0.0, 1.0),
                ((b[1] * h as f32 - win[1] as f32) / wh).clamp(0.0, 1.0),
                ((b[2] * w as f32 - win[0] as f32) / ww).clamp(0.0, 1.0),
                ((b[3] * h as f32 - win[1] as f32) / wh).clamp(0.0, 1.0),
            ],
        );
    }
    let to_win = |p: &[f32; 2]| {
        [
            ((p[0] * w as f32 - win[0] as f32) / ww).clamp(0.0, 1.0),
            ((p[1] * h as f32 - win[1] as f32) / wh).clamp(0.0, 1.0),
        ]
    };
    let n = r.points.len();
    let step = n.div_ceil(MAX_PROMPT_POINTS).max(1);
    let points: Vec<[f32; 2]> = r.points.iter().step_by(step).map(to_win).collect();
    let rad = r.radius.max(0.0) * w.max(h) as f32;
    let (mut b, mut any) = ([f32::MAX, f32::MAX, f32::MIN, f32::MIN], false);
    for p in &r.points {
        let (x, y) = (p[0] * w as f32, p[1] * h as f32);
        any = true;
        b = [
            b[0].min(x - rad),
            b[1].min(y - rad),
            b[2].max(x + rad),
            b[3].max(y + rad),
        ];
    }
    if !any {
        return (points, [0.0, 0.0, 1.0, 1.0]);
    }
    let (gx, gy) = (0.15 * (b[2] - b[0]), 0.15 * (b[3] - b[1]));
    let bbox = [
        ((b[0] - gx - win[0] as f32) / ww).clamp(0.0, 1.0),
        ((b[1] - gy - win[1] as f32) / wh).clamp(0.0, 1.0),
        ((b[2] + gx - win[0] as f32) / ww).clamp(0.0, 1.0),
        ((b[3] + gy - win[1] as f32) / wh).clamp(0.0, 1.0),
    ];
    (points, bbox)
}

/// A `side²` plane of `n` channels turned `turns` clockwise quarter turns.
fn turn_square(v: &[f32], side: usize, n: usize, turns: u8) -> Vec<f32> {
    let mut cur = v.to_vec();
    for _ in 0..turns % 4 {
        let mut next = vec![0f32; cur.len()];
        for y in 0..side {
            for x in 0..side {
                // Clockwise: (x, y) → (side − 1 − y, x) — `ai_worker::turn_point` in pixels.
                let to = (x * side + side - 1 - y) * n;
                next[to..to + n].copy_from_slice(&cur[(y * side + x) * n..(y * side + x + 1) * n]);
            }
        }
        cur = next;
    }
    cur
}

/// A normalised box turned `turns` clockwise quarter turns — its two corners turned, put back in order.
fn turn_box(b: [f32; 4], turns: u8) -> [f32; 4] {
    let a = crate::ai_worker::turn_point([b[0], b[1]], turns);
    let c = crate::ai_worker::turn_point([b[2], b[3]], turns);
    [
        a[0].min(c[0]),
        a[1].min(c[1]),
        a[0].max(c[0]),
        a[1].max(c[1]),
    ]
}

/// What a removal will replace: its window, normalised over the decode, and the soft alpha over it at
/// `side²` — the fill's own mask (M10.16c1b: the canvas shades it while the network works).
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub rect: [f32; 4],
    pub side: usize,
    pub alpha: Vec<f32>,
}

/// A removal's first step: its window, encoded for the network, and the area the fill will cover — known
/// before the network runs, so it can be shown while it does.
pub struct Prepared {
    rect: [f32; 4],
    enc: ai_denoise::Encoding,
    rgb: Vec<f32>,
    alpha: Vec<f32>,
}

impl Prepared {
    pub fn area(&self) -> Area {
        Area {
            rect: self.rect,
            side: SIDE,
            alpha: self.alpha.clone(),
        }
    }
}

/// The first step of [`fill`]: the window around `r` in `img` — the undeveloped decode, positive, full
/// resolution — encoded for the network, and the stroke's soft alpha with Detect objects' object joined.
/// The detector sees the window turned upright by `turns` clockwise quarter turns, and its answer is turned
/// back (M10.16c3).
pub fn prepare(
    img: &ImageBuf,
    r: &Removal,
    negative: &edit_model::Negative,
    turns: u8,
    detect: Option<Detect<'_>>,
) -> Result<Prepared, String> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || (r.points.is_empty() && r.person.is_none()) {
        return Err("nothing to fill".into());
    }
    if r.person.is_some() && detect.is_none() {
        return Err("a person needs the object finder".into());
    }
    let win = window(r, w, h);
    let mut crop = img.crop(win[0], win[1], win[2] - win[0], win[3] - win[1]);
    if negative.is_active() {
        crop = crop.map_pixels(|p| pipeline::negative::positive(p, negative));
    }
    let enc = peak_encoding(&crop);
    let rgb: Vec<f32> = resample(&crop, [0, 0, crop.width(), crop.height()], SIDE)
        .chunks(3)
        .flat_map(|p| enc.encode([p[0], p[1], p[2]]))
        .collect();
    // ⚠ A person has no stroke: this alpha is 0 everywhere, and the finder's object is the whole hole.
    let mut alpha = mask(r, win, w, h, SIDE);
    if needs_detector(r) {
        if let Some(detect) = detect {
            let (points, bbox) = prompt(r, win, w, h);
            // ⚠ M10.16c3: upright for MobileSAM — the spike measured a small person lying sideways (a portrait's
            // decode orientation) outlined at IoU 0.62–0.94 against upright. The window is square: a turn keeps
            // its size.
            let up_rgb = turn_square(&rgb, SIDE, 3, turns);
            let up_points: Vec<[f32; 2]> = points
                .iter()
                .map(|&p| crate::ai_worker::turn_point(p, turns))
                .collect();
            let probs = detect(&up_rgb, &up_points, turn_box(bbox, turns))?;
            if probs.len() != SIDE * SIDE {
                return Err(format!("the detector answered {} values", probs.len()));
            }
            let probs = turn_square(&probs, SIDE, 1, (4 - turns % 4) % 4);
            let object = soften(&dilate(&probs, SIDE, DETECT_GROW), SIDE, DETECT_GROW / 2);
            for (a, o) in alpha.iter_mut().zip(object) {
                *a = a.max(o);
            }
        }
    }
    Ok(Prepared {
        rect: [
            win[0] as f32 / w as f32,
            win[1] as f32 / h as f32,
            win[2] as f32 / w as f32,
            win[3] as f32 / h as f32,
        ],
        enc,
        rgb,
        alpha,
    })
}

/// The second step of [`fill`]: the network over the prepared window, the result back in the decode's
/// units — through the negative's inverse on a film scan.
pub fn finish(
    p: Prepared,
    r: &Removal,
    negative: &edit_model::Negative,
    net: Net<'_>,
) -> Result<pipeline::Fill, String> {
    let Prepared {
        rect,
        enc,
        rgb,
        alpha,
    } = p;
    let mut hole: Vec<f32> = alpha.iter().map(|&a| (a > 0.0) as u8 as f32).collect();
    let mut input = rgb;
    // Refresh: the odd variants ask about the window mirrored — another question even where it cannot move.
    let flip = r.variant % 2 == 1;
    if flip {
        mirror(&mut input, SIDE, 3);
        mirror(&mut hole, SIDE, 1);
    }
    let mut out = net(&input, &hole)?;
    if out.len() != SIDE * SIDE * 3 {
        return Err(format!("the network answered {} values", out.len()));
    }
    if out.iter().any(|v| !v.is_finite()) {
        return Err("the network answered a non-finite value".into());
    }
    if flip {
        mirror(&mut out, SIDE, 3);
    }
    let rgb = out
        .chunks(3)
        .flat_map(|p| pipeline::negative::negative_of(enc.decode([p[0], p[1], p[2]]), negative))
        .collect();
    Ok(pipeline::Fill {
        rect,
        w: SIDE,
        h: SIDE,
        rgb,
        alpha,
    })
}

/// One removal's fill from `img` — the undeveloped decode, positive, full resolution: [`prepare`], then
/// [`finish`].
///
/// `detect` is asked only when the removal has Detect objects or is a person; the object it finds joins the
/// hole. `turns` (clockwise quarter turns to upright) is the detector's: it sees the window upright.
///
/// ⚠⚠ A film negative (`negative` active): the network sees the POSITIVE — the Warnings' rule for every
/// reader of the undeveloped decode — and the fill goes back through [`pipeline::negative::negative_of`],
/// because the removals' step runs before the negative's inversion. So a fill stays right when the
/// negative's settings change later: it is what the film would have held there.
pub fn fill(
    img: &ImageBuf,
    r: &Removal,
    negative: &edit_model::Negative,
    turns: u8,
    net: Net<'_>,
    detect: Option<Detect<'_>>,
) -> Result<pipeline::Fill, String> {
    finish(prepare(img, r, negative, turns, detect)?, r, negative, net)
}

/// The vertices an area is drawn through: an `(n + 1)²` grid over `rect`, row-major, each a decode point and
/// its texture coordinate — mapped to the screen one by one, so the shading follows crop, rotation and lens.
pub fn area_grid(rect: [f32; 4], n: usize) -> Vec<([f32; 2], [f32; 2])> {
    let n = n.max(1);
    (0..=n)
        .flat_map(|gy| (0..=n).map(move |gx| (gx, gy)))
        .map(|(gx, gy)| {
            let (u, v) = (gx as f32 / n as f32, gy as f32 / n as f32);
            (
                [
                    rect[0] + u * (rect[2] - rect[0]),
                    rect[1] + v * (rect[3] - rect[1]),
                ],
                [u, v],
            )
        })
        .collect()
}

/// Whether two removals share one fill — everything but Opacity, which only blends it.
pub fn same_fill(a: &Removal, b: &Removal) -> bool {
    a.points == b.points
        && a.radius == b.radius
        && a.detect == b.detect
        && a.variant == b.variant
        && a.person == b.person
}

/// A fill's cache key: the photo, the stroke, Detect objects, Refresh's variant, the network and
/// [`RECIPE`] — never Opacity, which only blends a fill.
pub fn id(photo_key: &str, r: &Removal, model: ai_mask::ModelId) -> String {
    let mut s = format!(
        "{RECIPE}|{photo_key}|{}|{}|{}|{}",
        model.spec().file,
        r.radius.to_bits(),
        r.detect as u8,
        r.variant
    );
    for p in &r.points {
        s.push_str(&format!("|{},{}", p[0].to_bits(), p[1].to_bits()));
    }
    // M10.16c3: only when there is one — a stroke's key is untouched by the field.
    if let Some(b) = r.person {
        let [a, c, d, e] = b.map(f32::to_bits);
        s.push_str(&format!("|person,{a},{c},{d},{e}"));
    }
    ai_mask::digest_hex(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Removal;

    fn stroke(points: Vec<[f32; 2]>, radius: f32) -> Removal {
        Removal {
            points,
            radius,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: None,
        }
    }

    fn person(b: [f32; 4]) -> Removal {
        Removal {
            points: Vec::new(),
            radius: 0.0,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: Some(b),
        }
    }

    /// M10.16c3: a person's window is built around their box and holds it whole.
    #[test]
    fn a_person_s_window_holds_their_box() {
        let (w, h) = (3000, 2000);
        let r = person([0.40, 0.30, 0.45, 0.60]);
        let win = window(&r, w, h);
        assert!(
            win[0] as f32 <= 0.40 * w as f32 && win[2] as f32 >= 0.45 * w as f32,
            "{win:?}"
        );
        assert!(
            win[1] as f32 <= 0.30 * h as f32 && win[3] as f32 >= 0.60 * h as f32,
            "{win:?}"
        );
        let ext = 0.30 * h as f32;
        assert!(
            (win[2] - win[0]) as f32 >= CONTEXT * ext * 0.99,
            "the context around the person: {win:?}"
        );
    }

    /// A person's hole is the detector's object only — no stroke to join — prompted by their box alone.
    #[test]
    fn a_person_s_hole_is_the_detector_s_object_only() {
        let img = ImageBuf::from_vec(1200, 800, vec![0.3; 1200 * 800 * 3]);
        let r = person([0.45, 0.40, 0.55, 0.70]);
        let mut asked = None;
        let mut detect = |_: &[f32], points: &[[f32; 2]], bbox: [f32; 4]| {
            asked = Some((points.to_vec(), bbox));
            // A disc in the window's middle.
            Ok((0..SIDE * SIDE)
                .map(|i| {
                    let (x, y) = ((i % SIDE) as f32 - 256.0, (i / SIDE) as f32 - 256.0);
                    ((x * x + y * y).sqrt() < 40.0) as u8 as f32
                })
                .collect())
        };
        let p = prepare(
            &img,
            &r,
            &edit_model::Negative::default(),
            0,
            Some(&mut detect),
        )
        .unwrap();
        let (points, bbox) = asked.expect("the detector was asked");
        assert!(points.is_empty(), "a person is prompted by the box alone");
        assert!(
            bbox[0] < bbox[2] && bbox[1] < bbox[3] && bbox.iter().all(|v| (0.0..=1.0).contains(v)),
            "{bbox:?}"
        );
        let a = p.area();
        assert!(a.alpha[256 * SIDE + 256] > 0.99, "the object is the hole");
        assert_eq!(
            a.alpha[10 * SIDE + 10],
            0.0,
            "nothing outside the object — no stroke was joined"
        );
    }

    /// Without a detector a person cannot be filled — there is no stroke to fall back on.
    #[test]
    fn a_person_needs_the_detector() {
        let img = ImageBuf::from_vec(600, 400, vec![0.3; 600 * 400 * 3]);
        let r = person([0.4, 0.4, 0.5, 0.7]);
        assert!(prepare(&img, &r, &edit_model::Negative::default(), 0, None).is_err());
        assert!(needs_detector(&r));
        assert!(!needs_detector(&stroke(vec![[0.5, 0.5]], 0.02)));
    }

    #[test]
    fn a_person_s_key_and_fill_follow_their_box() {
        let (a, b) = (person([0.1, 0.1, 0.2, 0.5]), person([0.1, 0.1, 0.2, 0.6]));
        assert!(!same_fill(&a, &b));
        assert_ne!(
            id("photo", &a, ai_mask::ModelId::Lama),
            id("photo", &b, ai_mask::ModelId::Lama)
        );
        let c = Removal {
            opacity: 40.0,
            ..a.clone()
        };
        assert!(same_fill(&a, &c), "Opacity only blends");
        assert_eq!(
            id("photo", &a, ai_mask::ModelId::Lama),
            id("photo", &c, ai_mask::ModelId::Lama)
        );
    }

    #[test]
    fn a_click_inside_a_person_s_box_hits_their_removal() {
        let rs = vec![stroke(vec![[0.9, 0.9]], 0.01), person([0.2, 0.2, 0.4, 0.8])];
        assert_eq!(hit_test(&rs, [0.3, 0.5], 1000, 1000), Some(1));
        assert_eq!(hit_test(&rs, [0.5, 0.5], 1000, 1000), None);
    }

    /// M10.16c3: MobileSAM outlines a small person lying sideways worse (IoU 0.62–0.94 against upright, the
    /// spike) — so the detector is handed the window and its prompt turned upright, and its answer is turned
    /// back. The decode's bright top-left quarter shows each turn.
    #[test]
    fn the_detector_sees_the_window_upright() {
        let (w, h) = (SIDE, SIDE);
        let img = ImageBuf::from_vec(
            w,
            h,
            (0..w * h)
                .flat_map(|i| {
                    let v = if i % w < w / 2 && i / w < h / 2 {
                        0.8
                    } else {
                        0.05
                    };
                    [v, v, v]
                })
                .collect(),
        );
        // A stroke left of the middle with Detect objects; the window's floor is the whole 512² decode.
        let r = Removal {
            detect: true,
            ..stroke(vec![[0.3, 0.5]], 0.05)
        };
        assert_eq!(
            window(&r, w, h),
            [0, 0, w, h],
            "the fixture: the window is the decode"
        );
        // The upright window's corners, clockwise from the top-left.
        let corners = [(5, 5), (SIDE - 6, 5), (SIDE - 6, SIDE - 6), (5, SIDE - 6)];
        for turns in 0..4u8 {
            let mut seen: Option<(Vec<f32>, [f32; 4])> = None;
            let mut detect = |rgb: &[f32], _: &[[f32; 2]], bbox: [f32; 4]| {
                seen = Some((rgb.to_vec(), bbox));
                // The upright top-left corner marked.
                Ok((0..SIDE * SIDE)
                    .map(|i| (i % SIDE < 32 && i / SIDE < 32) as u8 as f32)
                    .collect())
            };
            let p = prepare(
                &img,
                &r,
                &edit_model::Negative::default(),
                turns,
                Some(&mut detect),
            )
            .unwrap();
            let (rgb, bbox) = seen.expect("asked");
            // The decode's bright top-left, turned `turns` times clockwise, is this corner of what it saw.
            let lit: Vec<usize> = (0..4)
                .filter(|&k| rgb[(corners[k].1 * SIDE + corners[k].0) * 3] > 0.3)
                .collect();
            assert_eq!(lit, vec![turns as usize], "turns {turns}");
            // The prompt turned with it: its box's centre where the stroke's (0.3, 0.5) turns to.
            let want = crate::ai_worker::turn_point([0.3, 0.5], turns);
            let got = [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0];
            assert!(
                (got[0] - want[0]).abs() < 0.01 && (got[1] - want[1]).abs() < 0.01,
                "turns {turns}: the prompt at {got:?}, the stroke turned to {want:?}"
            );
            // Its answer's upright top-left, turned back, sits at the decode corner the turn took there.
            let back = corners[(4 - turns as usize) % 4];
            let a = p.area();
            assert!(
                a.alpha[back.1 * SIDE + back.0] > 0.5,
                "turns {turns}: the answer turned back"
            );
        }
    }

    #[test]
    fn the_tool_starts_in_remove_mode() {
        let t = RemoveTool::default();
        assert!(t.active && !t.detect && t.radius > 0.0, "{t:?}");
    }

    /// A slow drag adds a point only every quarter radius; a fast one at each frame.
    #[test]
    fn a_stroke_point_is_kept_only_past_a_quarter_radius() {
        let (w, h) = (4000, 3000);
        let radius = 0.01; // 40 px; a quarter is 10 px
        let mut pts = Vec::new();
        for x in 0..40 {
            add_point(
                &mut pts,
                [(1000.0 + x as f32 * 2.0) / w as f32, 0.5],
                radius,
                w,
                h,
            );
        }
        // Every gap at least a quarter radius (10 px) and less than that plus one 2-px step.
        assert_eq!(pts[0], [0.25, 0.5], "the first point always");
        assert!(pts.len() >= 6, "{pts:?}");
        for g in pts.windows(2) {
            let gap = (g[1][0] - g[0][0]) * w as f32;
            assert!((9.99..12.01).contains(&gap), "a {gap} px gap in {pts:?}");
        }
        let n = pts.len();
        add_point(&mut pts, [f32::NAN, 0.5], radius, w, h);
        assert_eq!(pts.len(), n, "a NaN is never a point");
    }

    #[test]
    fn hit_test_finds_the_topmost_removal_under_a_point() {
        let (w, h) = (2000, 1000);
        let a = stroke(vec![[0.2, 0.5], [0.4, 0.5]], 0.02); // 40 px radius, a 50 px reach
        let b = stroke(vec![[0.35, 0.5]], 0.02);
        let both = [a.clone(), b];
        assert_eq!(hit_test(&both, [0.3, 0.5], w, h), Some(0), "on a's line");
        assert_eq!(hit_test(&both, [0.35, 0.52], w, h), Some(1), "b is on top");
        assert_eq!(
            hit_test(&both, [0.3, 0.5 + 45.0 / 1000.0], w, h),
            Some(0),
            "in the soft edge"
        );
        assert_eq!(
            hit_test(&both, [0.3, 0.5 + 60.0 / 1000.0], w, h),
            None,
            "past it"
        );
        assert_eq!(hit_test(&[], [0.3, 0.5], w, h), None);
    }

    /// The hole's extent in decode pixels: the stroke's box grown by the soft edge.
    fn hole_box(r: &Removal, w: usize, h: usize) -> [f32; 4] {
        let rad = r.radius * w.max(h) as f32 * HOLE_GROW;
        let xs = r.points.iter().map(|p| p[0] * w as f32);
        let ys = r.points.iter().map(|p| p[1] * h as f32);
        [
            xs.clone().fold(f32::MAX, f32::min) - rad,
            ys.clone().fold(f32::MAX, f32::min) - rad,
            xs.fold(f32::MIN, f32::max) + rad,
            ys.fold(f32::MIN, f32::max) + rad,
        ]
    }

    fn contains(win: [usize; 4], b: [f32; 4], w: usize, h: usize) -> bool {
        win[0] as f32 <= b[0].max(0.0)
            && win[1] as f32 <= b[1].max(0.0)
            && win[2] as f32 >= b[2].min(w as f32)
            && win[3] as f32 >= b[3].min(h as f32)
    }

    #[test]
    fn a_window_grows_around_the_stroke() {
        let (w, h) = (6000, 4000);
        // A small stroke: the window's floor, 512, centred on it.
        let small = stroke(vec![[0.5, 0.5], [0.51, 0.5]], 0.005);
        let win = window(&small, w, h);
        assert_eq!((win[2] - win[0], win[3] - win[1]), (512, 512), "{win:?}");
        assert!(contains(win, hole_box(&small, w, h), w, h), "{win:?}");
        // A large one: 2.5 × its extent, square, still holding it.
        let large = stroke(vec![[0.4, 0.4], [0.55, 0.45]], 0.02);
        let b = hole_box(&large, w, h);
        let win = window(&large, w, h);
        let side = win[2] - win[0];
        assert_eq!(side, win[3] - win[1], "square: {win:?}");
        let ext = (b[2] - b[0]).max(b[3] - b[1]);
        assert!(
            (side as f32 - ext * CONTEXT).abs() <= 2.0,
            "{side} vs {ext}"
        );
        assert!(contains(win, b, w, h), "{win:?} vs {b:?}");
    }

    /// Review Focus 1: a stroke at the photo's corner — the window stays inside the decode and still
    /// holds all of the stroke that is inside it.
    #[test]
    fn a_window_at_the_edge_stays_inside() {
        let (w, h) = (6000, 4000);
        for p in [[0.0, 0.0], [0.995, 0.99], [0.0, 0.999], [1.0, 0.5]] {
            let r = stroke(vec![p], 0.01);
            for variant in 0..4 {
                let r = Removal {
                    variant,
                    ..r.clone()
                };
                let win = window(&r, w, h);
                assert!(win[0] < win[2] && win[1] < win[3], "{win:?}");
                assert!(win[2] <= w && win[3] <= h, "{p:?}/{variant}: {win:?}");
                assert!(
                    contains(win, hole_box(&r, w, h), w, h),
                    "{p:?}/{variant}: {win:?}"
                );
            }
        }
        // A decode smaller than the floor: the whole decode.
        let r = stroke(vec![[0.5, 0.5]], 0.05);
        assert_eq!(window(&r, 300, 200), [0, 0, 300, 200]);
    }

    /// Review Focus 3: Refresh fills from another window, and its key moves with it.
    #[test]
    fn refresh_moves_the_window_and_the_key() {
        let (w, h) = (6000, 4000);
        let r = stroke(vec![[0.5, 0.5], [0.52, 0.51]], 0.01);
        let mut seen = vec![window(&r, w, h)];
        for variant in 1..4 {
            let v = Removal {
                variant,
                ..r.clone()
            };
            let win = window(&v, w, h);
            assert!(!seen.contains(&win), "variant {variant} reused {win:?}");
            assert!(contains(win, hole_box(&v, w, h), w, h), "{win:?}");
            seen.push(win);
            assert_ne!(
                id("p", &r, ai_mask::ModelId::Lama),
                id("p", &v, ai_mask::ModelId::Lama)
            );
        }
    }

    /// Where the window cannot move (the stroke fills a small decode), Refresh still asks the network
    /// something else: the odd variants see the window mirrored.
    #[test]
    fn refresh_changes_the_input_even_where_the_window_cannot_move() {
        let img = gradient(300, 200);
        let r = stroke(vec![[0.3, 0.5], [0.7, 0.5]], 0.2);
        let inputs: Vec<Vec<f32>> = (0..2)
            .map(|variant| {
                let mut got = Vec::new();
                fill(
                    &img,
                    &Removal {
                        variant,
                        ..r.clone()
                    },
                    &edit_model::Negative::default(),
                    0,
                    &mut |rgb, _| {
                        got = rgb.to_vec();
                        Ok(rgb.to_vec())
                    },
                    None,
                )
                .unwrap();
                got
            })
            .collect();
        assert_eq!(
            window(&r, 300, 200),
            window(
                &Removal {
                    variant: 1,
                    ..r.clone()
                },
                300,
                200
            )
        );
        assert_ne!(inputs[0], inputs[1], "the same question twice");
    }

    #[test]
    fn opacity_is_not_in_the_key() {
        let r = stroke(vec![[0.5, 0.5]], 0.01);
        let faint = Removal {
            opacity: 20.0,
            ..r.clone()
        };
        assert_eq!(
            id("p", &r, ai_mask::ModelId::Lama),
            id("p", &faint, ai_mask::ModelId::Lama)
        );
        let detect = Removal {
            detect: true,
            ..r.clone()
        };
        assert_ne!(
            id("p", &r, ai_mask::ModelId::Lama),
            id("p", &detect, ai_mask::ModelId::Lama)
        );
        assert_ne!(
            id("p", &r, ai_mask::ModelId::Lama),
            id("q", &r, ai_mask::ModelId::Lama)
        );
    }

    /// Fills on disk are named by this key — a change to it orphans every one of them.
    #[test]
    fn the_key_is_pinned() {
        let r = Removal {
            points: vec![[0.25, 0.5], [0.3, 0.55]],
            radius: 0.02,
            detect: true,
            variant: 3,
            opacity: 60.0,
            person: None,
        };
        assert_eq!(
            id("photo-key", &r, ai_mask::ModelId::Lama),
            // ⚠ Re-pinned 2026-10-21 for `remove:v3` (M10.16c3) — a deliberate change to how a fill is made.
            "35d3a9f88fc6da31e658df7c55630238a12c132c3d387c31d5a2213fa3f712ff"
        );
    }

    /// A smooth linear-light gradient, every pixel its own.
    fn gradient(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [x as f32 / w as f32 * 0.5, y as f32 / h as f32 * 0.5, 0.2],
                );
            }
        }
        img
    }

    /// A "network" that paints the hole one grey and leaves the rest — the patch must carry that grey
    /// inside the stroke, no alpha outside its soft edge, and a rect where the window was.
    #[test]
    fn the_fill_replaces_only_the_hole() {
        let (w, h) = (2000, 1500);
        let img = gradient(w, h);
        let r = stroke(vec![[0.5, 0.5], [0.55, 0.52]], 0.01);
        let mut holes = 0;
        let f = fill(
            &img,
            &r,
            &edit_model::Negative::default(),
            0,
            &mut |rgb, mask| {
                assert_eq!(
                    (rgb.len(), mask.len()),
                    (
                        ai_mask::inpaint::SIDE.pow(2) * 3,
                        ai_mask::inpaint::SIDE.pow(2)
                    )
                );
                holes = mask.iter().filter(|&&m| m >= 0.5).count();
                Ok(rgb
                    .chunks(3)
                    .zip(mask)
                    .flat_map(|(p, &m)| {
                        if m >= 0.5 {
                            [0.5; 3]
                        } else {
                            [p[0], p[1], p[2]]
                        }
                    })
                    .collect())
            },
            None,
        )
        .unwrap();
        assert!(holes > 100, "the network saw a hole of {holes} pixels");
        let win = window(&r, w, h);
        let want = [
            win[0] as f32 / w as f32,
            win[1] as f32 / h as f32,
            win[2] as f32 / w as f32,
            win[3] as f32 / h as f32,
        ];
        assert_eq!(f.rect, want);
        assert_eq!((f.w, f.h), (ai_mask::inpaint::SIDE, ai_mask::inpaint::SIDE));
        let grey = peak_encoding(&img.crop(win[0], win[1], win[2] - win[0], win[3] - win[1]))
            .decode([0.5; 3]);
        let (rad, b) = (r.radius * w.max(h) as f32, hole_box(&r, w, h));
        let (mut core, mut far) = (0, 0);
        for fy in 0..f.h {
            for fx in 0..f.w {
                // The fill pixel's centre in decode pixels, and its distance from the stroke's box.
                let x = win[0] as f32 + (fx as f32 + 0.5) / f.w as f32 * (win[2] - win[0]) as f32;
                let y = win[1] as f32 + (fy as f32 + 0.5) / f.h as f32 * (win[3] - win[1]) as f32;
                let i = fy * f.w + fx;
                let a = f.alpha[i];
                let d = distance_to(&r, w, h, x, y);
                if d < rad * 0.9 {
                    core += 1;
                    assert_eq!(a, 1.0, "({fx}, {fy}) in the stroke");
                    for (v, g) in f.rgb[i * 3..i * 3 + 3].iter().zip(grey) {
                        assert!((v - g).abs() < 1e-4, "({fx}, {fy})");
                    }
                }
                if x < b[0] - 2.0 || x > b[2] + 2.0 || y < b[1] - 2.0 || y > b[3] + 2.0 {
                    far += 1;
                    assert_eq!(a, 0.0, "({fx}, {fy}) outside the stroke");
                }
            }
        }
        assert!(
            core > 50 && far > 1000,
            "{core} in the stroke, {far} outside"
        );
    }

    /// Review Focus 4: on a film negative the network sees the positive, and the fill it returns lands back
    /// in the negative's units — through the negative step it is the network's answer again.
    #[test]
    fn the_fill_reads_the_positive() {
        let n = edit_model::Negative {
            on: true,
            base: [0.8, 0.5, 0.3],
            gamma: 0.6,
            low: [0.05; 3],
            high: [1.6; 3],
            mono: false,
        };
        // A flat negative: one film colour everywhere, so every resampled pixel is exactly it.
        let film = [0.2, 0.15, 0.1];
        let img = ImageBuf::from_vec(1200, 900, film.repeat(1200 * 900));
        let r = stroke(vec![[0.5, 0.5]], 0.01);
        let win = window(&r, 1200, 900);
        let pos = img
            .crop(win[0], win[1], win[2] - win[0], win[3] - win[1])
            .map_pixels(|p| pipeline::negative::positive(p, &n));
        let enc = peak_encoding(&pos);
        let want_in = enc.encode(pipeline::negative::positive(film, &n));
        let mut seen = Vec::new();
        let f = fill(
            &img,
            &r,
            &n,
            0,
            &mut |rgb, mask| {
                seen = rgb[..3].to_vec();
                Ok(rgb
                    .chunks(3)
                    .zip(mask)
                    .flat_map(|(p, &m)| {
                        if m >= 0.5 {
                            [0.6, 0.4, 0.2]
                        } else {
                            [p[0], p[1], p[2]]
                        }
                    })
                    .collect())
            },
            None,
        )
        .unwrap();
        for c in 0..3 {
            assert!(
                (seen[c] - want_in[c]).abs() < 1e-5,
                "the network saw {seen:?}, the positive is {want_in:?}"
            );
        }
        let centre = (f.h / 2) * f.w + f.w / 2;
        assert_eq!(
            f.alpha[centre], 1.0,
            "the fixture: the centre is in the stroke"
        );
        let back = pipeline::negative::positive(
            [
                f.rgb[centre * 3],
                f.rgb[centre * 3 + 1],
                f.rgb[centre * 3 + 2],
            ],
            &n,
        );
        let want = enc.decode([0.6, 0.4, 0.2]);
        for c in 0..3 {
            assert!(
                (back[c] - want[c]).abs() < 1e-3 * want[c].max(0.1),
                "{back:?} vs {want:?}"
            );
        }
    }

    /// The author's report (2026-10-19): a removal in front of a sky came out greenish. A network that
    /// changes nothing must give a bright blue sky back as it was — every channel, not just the luminance
    /// the encoding was scaled by (blue sits far above it in a sky, and clipped there).
    #[test]
    fn the_fill_keeps_a_bright_sky_s_colour() {
        let (w, h) = (1600, 1200);
        // Linear sRGB, a saturated sky brightening downwards — as a decode holds it, in linear ProPhoto.
        let sky = |y: f32| {
            let t = y / h as f32;
            [0.10 + 0.25 * t, 0.22 + 0.28 * t, 0.70 + 0.20 * t]
        };
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, color::srgb_to_prophoto(sky(y as f32 + 0.5)));
            }
        }
        let r = stroke(vec![[0.5, 0.5], [0.55, 0.5]], 0.02);
        let f = fill(
            &img,
            &r,
            &edit_model::Negative::default(),
            0,
            &mut |rgb, _| Ok(rgb.to_vec()),
            None,
        )
        .unwrap();
        let win = window(&r, w, h);
        let mut checked = 0;
        for fy in (0..f.h).step_by(17) {
            for fx in (0..f.w).step_by(17) {
                let i = fy * f.w + fx;
                if f.alpha[i] < 1.0 {
                    continue;
                }
                let y = win[1] as f32 + (fy as f32 + 0.5) / f.h as f32 * (win[3] - win[1]) as f32;
                let want = sky(y);
                let got =
                    color::prophoto_to_srgb([f.rgb[i * 3], f.rgb[i * 3 + 1], f.rgb[i * 3 + 2]]);
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() <= 0.01 * want[c] + 1e-3,
                        "channel {c} at ({fx}, {fy}): {got:?} against the sky's {want:?}"
                    );
                }
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "the fixture: no fill pixel in the stroke was checked"
        );
    }

    /// The author's call (2026-10-19): Detect objects shows the area it will fill while it works. That area is
    /// handed out before the network runs — and it IS the fill's own mask: the same window, the same alpha.
    #[test]
    fn the_area_is_the_fills_own_mask() {
        let img = gradient(2000, 1500);
        let r = Removal {
            detect: true,
            ..stroke(vec![[0.5, 0.5], [0.52, 0.5]], 0.01)
        };
        let side = SIDE;
        let object = move || -> Vec<f32> {
            (0..side * side)
                .map(|i| ((i % side) > side / 3 && (i % side) < side / 2) as u8 as f32)
                .collect()
        };
        let neg = edit_model::Negative::default();
        let prepared = prepare(&img, &r, &neg, 0, Some(&mut |_, _, _| Ok(object()))).unwrap();
        let area = prepared.area();
        let f = fill(
            &img,
            &r,
            &neg,
            0,
            &mut |rgb, _| Ok(rgb.to_vec()),
            Some(&mut |_, _, _| Ok(object())),
        )
        .unwrap();
        assert_eq!(area.rect, f.rect);
        assert_eq!((area.side, area.alpha.len()), (f.w, f.alpha.len()));
        assert_eq!(area.alpha, f.alpha, "what is shown is what is replaced");
        assert!(
            area.alpha.iter().filter(|&&a| a > 0.0).count() > side * side / 8,
            "the fixture: the detected object is in the area"
        );
        let done = finish(prepared, &r, &neg, &mut |rgb, _| Ok(rgb.to_vec())).unwrap();
        assert_eq!(done, f, "prepare then finish is fill");
    }

    /// The area is drawn through a grid over its rect — its corners on the rect's, its texture spanning 0..1.
    #[test]
    fn the_area_grid_spans_its_rect() {
        let rect = [0.2, 0.3, 0.6, 0.9];
        let g = area_grid(rect, 4);
        assert_eq!(g.len(), 25, "(4 + 1)² vertices");
        assert_eq!(g[0], ([0.2, 0.3], [0.0, 0.0]));
        assert_eq!(g[24], ([0.6, 0.9], [1.0, 1.0]));
        assert_eq!(g[4], ([0.6, 0.3], [1.0, 0.0]), "row-major, left to right");
        let ([x, y], [u, v]) = g[12];
        assert!((x - 0.4).abs() < 1e-6 && (y - 0.6).abs() < 1e-6 && u == 0.5 && v == 0.5);
    }

    #[test]
    fn same_fill_ignores_opacity_alone() {
        let r = stroke(vec![[0.5, 0.5]], 0.01);
        assert!(same_fill(
            &r,
            &Removal {
                opacity: 20.0,
                ..r.clone()
            }
        ));
        assert!(!same_fill(
            &r,
            &Removal {
                variant: 1,
                ..r.clone()
            }
        ));
        assert!(!same_fill(
            &r,
            &Removal {
                detect: true,
                ..r.clone()
            }
        ));
        assert!(!same_fill(&r, &stroke(vec![[0.6, 0.5]], 0.01)));
    }

    /// The distance from decode pixel `(x, y)` to the stroke, in decode pixels — the test's own.
    fn distance_to(r: &Removal, w: usize, h: usize, x: f32, y: f32) -> f32 {
        let p: Vec<[f32; 2]> = r
            .points
            .iter()
            .map(|p| [p[0] * w as f32, p[1] * h as f32])
            .collect();
        let seg = |a: [f32; 2], b: [f32; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx * dx + dy * dy;
            let t = if len > 0.0 {
                (((x - a[0]) * dx + (y - a[1]) * dy) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ((a[0] + t * dx - x).powi(2) + (a[1] + t * dy - y).powi(2)).sqrt()
        };
        if p.len() == 1 {
            return seg(p[0], p[0]);
        }
        p.windows(2)
            .map(|s| seg(s[0], s[1]))
            .fold(f32::MAX, f32::min)
    }

    /// Detect objects: the network's hole grows to what the detector found, the prompt is the stroke in
    /// the window's own coordinates, and the detector is never asked without the flag.
    #[test]
    fn detect_grows_the_hole_to_the_object() {
        let (w, h) = (2000, 1500);
        let img = gradient(w, h);
        let r = Removal {
            detect: true,
            ..stroke(vec![[0.5, 0.5]], 0.004)
        };
        let (mut plain_hole, mut grown_hole) = (0, 0);
        let mut asked = None;
        let side = ai_mask::inpaint::SIDE;
        // The "object": the window's middle quarter.
        let object: Vec<f32> = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                ((side / 4..3 * side / 4).contains(&x) && (side / 4..3 * side / 4).contains(&y))
                    as u8 as f32
            })
            .collect();
        fill(
            &img,
            &r,
            &edit_model::Negative::default(),
            0,
            &mut |rgb, m| {
                grown_hole = m.iter().filter(|&&v| v >= 0.5).count();
                Ok(rgb.to_vec())
            },
            Some(&mut |_rgb, points, bbox| {
                asked = Some((points.to_vec(), bbox));
                Ok(object.clone())
            }),
        )
        .unwrap();
        fill(
            &img,
            &Removal {
                detect: false,
                ..r.clone()
            },
            &edit_model::Negative::default(),
            0,
            &mut |rgb, m| {
                plain_hole = m.iter().filter(|&&v| v >= 0.5).count();
                Ok(rgb.to_vec())
            },
            Some(&mut |_, _, _| panic!("asked without Detect objects")),
        )
        .unwrap();
        assert!(
            grown_hole >= side * side / 4,
            "the object's {} pixels, the hole {grown_hole}",
            side * side / 4
        );
        assert!(
            plain_hole < grown_hole / 4,
            "the stroke alone: {plain_hole}"
        );
        let (points, bbox) = asked.expect("the detector was asked");
        assert!(
            !points.is_empty() && points.len() <= MAX_PROMPT_POINTS,
            "{points:?}"
        );
        for p in &points {
            assert!(
                p[0] >= bbox[0] && p[0] <= bbox[2] && p[1] >= bbox[1] && p[1] <= bbox[3],
                "{p:?} outside {bbox:?}"
            );
            assert!(
                (p[0] - 0.5).abs() < 0.05 && (p[1] - 0.5).abs() < 0.05,
                "{p:?}: the stroke sits mid-window"
            );
        }
        assert!(bbox.iter().all(|v| (0.0..=1.0).contains(v)), "{bbox:?}");
    }
}

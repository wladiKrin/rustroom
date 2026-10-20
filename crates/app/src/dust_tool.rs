//! Remove dust (M10.16c2): sensor dust found in the undeveloped decode — soft, dark, round blobs in smooth
//! areas — and turned into Heal patches. No network: a contrast of each disc against its ring, in log
//! luminance, over a few radii, gated by the local texture.
//!
//! ⚠ It reads the decode as captured, a film negative included: dust blocks light wherever it sits — on the
//! sensor's glass or on the film in the scanner — so it is darker in what was captured either way.

use core_image::ImageBuf;
use rayon::prelude::*;

/// The sensitivity the slider starts at, 0..=100.
pub const DEFAULT_SENSITIVITY: f32 = 50.0;
/// At most this many finds — the strongest.
pub const MAX_FINDS: usize = 200;
/// The working image's long side: dust 10–60 px across on a 26 Mpx frame stays 5–30 px here.
const WORK: usize = 3000;
/// The radii looked at, in working pixels.
///
/// ⚠ From 3, not 2 (measured on the author's f/18–f/22 frames, 2026-10-20): radius 2 found specks of one to
/// three pixels — distant birds, single dead pixels — never sensor dust, whose shadow is soft and wider.
const RADII: [f32; 5] = [3.0, 4.5, 7.0, 10.0, 15.0];
/// The blur the texture's background is taken against, in working pixels — four times the widest dust, or
/// a large disc would be taken for background and lose its contrast.
const BACKGROUND: f32 = 60.0;
/// A disc must stand this many times above the texture around it — 4 at Sensitivity 0, 2 at 100.
fn snr(sensitivity: f32) -> f32 {
    4.0 - 2.0 * sensitivity.clamp(0.0, 100.0) / 100.0
}
/// The darkest dust, in log luminance — the centre against its ring's median, 2.5 radii out (~45 % darker).
/// ⚠ Not the detection contrast: that ring is a blur centred on the spot, so it takes in the spot and reads a
/// disc 70 % darker as 0.30–0.48 depending on the radius. The author's frames showed stones and birds far
/// darker than any dust shadow.
const MAX_DARKENING: f32 = 0.6;
/// Every direction around a find must be this share of its contrast brighter than its centre: a line, an
/// edge or a stone among stones has a dark direction, a dust shadow on a smooth sky none.
const RING_SHARE: f32 = 0.35;
/// The directions the ring is read in.
const RING_DIRECTIONS: usize = 16;
/// Below this fraction of the frame's 99th-percentile luminance a spot is noise, not dust.
const DARK_FLOOR: f32 = 0.05;

/// One spot of dust.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Find {
    /// Normalised over the decode.
    pub centre: [f32; 2],
    /// A fraction of the decode's long edge.
    pub radius: f32,
    /// How much darker than its ring, in log luminance.
    pub contrast: f32,
}

/// A patch's radius against its dust's — the soft edge covered, and the ring Heal measures off the dust.
pub const SPOT_SCALE: f32 = 1.6;

/// Each find as a Heal patch in the patches' space: its centre through `post_lens` (decode → post-lens),
/// [`SPOT_SCALE`] × its radius within the spot tool's limits, the tool's feather, a source picked by
/// `spot_source::pick` over `sample` (3 radii up, else down, when it finds none — the click's rule). A find
/// whose centre sits inside an existing patch, or one already made, is skipped.
pub fn to_spots(
    finds: &[Find],
    post_lens: &dyn Fn([f32; 2]) -> [f32; 2],
    aspect: f32,
    existing: &[edit_model::Spot],
    tool: crate::spot_tool::SpotTool,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> Vec<edit_model::Spot> {
    let (sx, sy) = crate::spot_tool::long_edge_scale(aspect);
    let inside = |p: [f32; 2], s: &edit_model::Spot| {
        (((p[0] - s.centre[0]) * sx).powi(2) + ((p[1] - s.centre[1]) * sy).powi(2)).sqrt()
            < s.radius
    };
    let mut made: Vec<edit_model::Spot> = Vec::new();
    for f in finds {
        let centre = post_lens(f.centre);
        if !centre.iter().all(|v| (0.0..=1.0).contains(v)) {
            continue;
        }
        if existing.iter().chain(&made).any(|s| inside(centre, s)) {
            continue;
        }
        let radius = (SPOT_SCALE * f.radius)
            .clamp(crate::spot_tool::MIN_RADIUS, crate::spot_tool::MAX_RADIUS);
        let taken: Vec<edit_model::Spot> = existing.iter().chain(&made).copied().collect();
        let source = crate::spot_source::pick(sample, centre, radius, aspect, &taken)
            .unwrap_or_else(|| {
                let up = centre[1] - 3.0 * radius;
                [
                    centre[0],
                    if up >= 0.0 {
                        up
                    } else {
                        (centre[1] + 3.0 * radius).min(1.0)
                    },
                ]
            });
        made.push(edit_model::Spot {
            centre,
            source,
            radius,
            feather: tool.feather,
            mode: edit_model::SpotMode::Heal,
        });
    }
    made
}

/// Whether a spot whose ring reads `ring` (each direction's brightness above the centre, log luminance) is
/// dust: brighter all round — every direction at least [`RING_SHARE`] of the median, so no line, edge or
/// neighbouring stone runs through it — and darkened by no more than [`MAX_DARKENING`].
fn dust_like(mut ring: [f32; RING_DIRECTIONS]) -> bool {
    ring.sort_by(f32::total_cmp);
    let darkening = ring[RING_DIRECTIONS / 2];
    darkening > 0.0 && darkening <= MAX_DARKENING && ring[0] >= RING_SHARE * darkening
}

/// The least contrast a find needs at `sensitivity` (0..=100): 8 % darker at 0, ten times fainter at 100 —
/// 2.5 % at the default.
fn min_contrast(sensitivity: f32) -> f32 {
    0.08 * 10f32.powf(-sensitivity.clamp(0.0, 100.0) / 100.0)
}

/// A disc's radius is this many times the radius it answers most strongly at — measured on soft discs of
/// 4–16 px (the ring-against-centre contrast peaks at about 0.6 of the radius).
const RADIUS_OF: f32 = 1.6;

/// The dust in `img` (the undeveloped decode, any size) at `sensitivity` — strongest first.
pub fn find(img: &ImageBuf, sensitivity: f32) -> Vec<Find> {
    find_with_progress(img, sensitivity, &|_| {})
}

/// [`find`], telling `progress` how far it has got (0..=1, never going back) — after the preparation and
/// after each radius, the six steps of about equal cost.
pub fn find_with_progress(img: &ImageBuf, sensitivity: f32, progress: &dyn Fn(f32)) -> Vec<Find> {
    let steps = (RADII.len() + 1) as f32;
    let (dw, dh) = (img.width(), img.height());
    if dw < 16 || dh < 16 {
        return Vec::new();
    }
    let small;
    let work = if dw.max(dh) > WORK {
        small = img.downscale_to(WORK);
        &small
    } else {
        img
    };
    let (w, h) = (work.width(), work.height());
    let scale = dw.max(dh) as f32 / w.max(h) as f32;
    let lum: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| color::luminance(work.pixel(i % w, i / w)).max(0.0))
        .collect();
    let log: Vec<f32> = lum.par_iter().map(|&v| v.max(1e-4).ln()).collect();
    // ⚠ Box-iterated for the wide blurs: a true Gaussian's cost grows with its sigma, and these reach 60 px.
    let blur = |p: &[f32], s: f32| {
        if s < 3.0 {
            pipeline::spatial::gaussian_blur(p, w, h, s)
        } else {
            pipeline::spatial::wide_blur(p, w, h, s)
        }
    };
    // The detail the texture is measured on: gradients and vignetting gone, at four times the widest dust.
    let base = blur(&log, BACKGROUND);
    let detail: Vec<f32> = log.par_iter().zip(&base).map(|(l, b)| l - b).collect();
    let squared: Vec<f32> = detail.par_iter().map(|d| d * d).collect();
    let floor = {
        let mut sorted: Vec<f32> = lum.iter().step_by(7).copied().collect();
        let k = ((sorted.len().max(1) - 1) as f32 * 0.99) as usize;
        let (_, p, _) = sorted.select_nth_unstable_by(k, f32::total_cmp);
        *p * DARK_FLOOR
    };
    let background = blur(&lum, 24.0);
    let need = min_contrast(sensitivity);
    let snr = snr(sensitivity);
    // The centre's darkness read nearly sharp: the radius's own blur dilutes a small dark core, and a stone
    // 70 % darker then reads as dust (measured).
    let sharp = blur(&log, 1.0);
    progress(1.0 / steps);
    let mut candidates: Vec<(usize, f32, f32)> = Vec::new();
    for (step, &r) in RADII.iter().enumerate() {
        let inner = blur(&log, 0.6 * r);
        let ring = blur(&log, 2.5 * r);
        let mean = blur(&detail, 4.0 * r);
        let energy = blur(&squared, 4.0 * r);
        let c: Vec<f32> = ring.par_iter().zip(&inner).map(|(o, i)| o - i).collect();
        let found: Vec<(usize, f32, f32)> = (1..h - 1)
            .into_par_iter()
            .flat_map_iter(|y| {
                let (c, mean, energy, background, inner, sharp) =
                    (&c, &mean, &energy, &background, &inner, &sharp);
                (1..w - 1).filter_map(move |x| {
                    let i = y * w + x;
                    let v = c[i];
                    if v < need || background[i] < floor {
                        return None;
                    }
                    let texture = (energy[i] - mean[i] * mean[i]).max(0.0).sqrt();
                    if v < snr * texture {
                        return None;
                    }
                    let peak = [
                        i - w - 1,
                        i - w,
                        i - w + 1,
                        i - 1,
                        i + 1,
                        i + w - 1,
                        i + w,
                        i + w + 1,
                    ]
                    .iter()
                    .all(|&j| c[j] <= v);
                    if !peak {
                        return None;
                    }
                    // The ring itself — 16 directions, 2.5 r out, on the radius's blur — against the centre
                    // read nearly sharp: brighter than it in every direction (no line, edge or neighbouring
                    // stone), and not too much brighter for dust.
                    let centre = sharp[i];
                    let mut ring = [0f32; RING_DIRECTIONS];
                    for (k, s) in ring.iter_mut().enumerate() {
                        let a = k as f32 / RING_DIRECTIONS as f32 * std::f32::consts::TAU;
                        let (sx, sy) = (x as f32 + 2.5 * r * a.cos(), y as f32 + 2.5 * r * a.sin());
                        if sx < 0.0 || sy < 0.0 || sx >= (w - 1) as f32 || sy >= (h - 1) as f32 {
                            return None;
                        }
                        *s = inner[sy.round() as usize * w + sx.round() as usize] - centre;
                    }
                    dust_like(ring).then_some((i, r, v))
                })
            })
            .collect();
        candidates.extend(found);
        progress((step + 2) as f32 / steps);
    }
    // Strongest first; a weaker candidate near a stronger one is the same spot at another radius, or its edge.
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
    let reach = 2.0 * RADII[RADII.len() - 1];
    let mut kept: Vec<(usize, f32, f32)> = Vec::new();
    for c in candidates {
        let (x, y) = ((c.0 % w) as f32, (c.0 / w) as f32);
        let near = kept.iter().any(|k| {
            let (kx, ky) = ((k.0 % w) as f32, (k.0 / w) as f32);
            (kx - x).powi(2) + (ky - y).powi(2) < (reach.min(2.0 * (k.1.max(c.1)) + 2.0)).powi(2)
        });
        if !near {
            kept.push(c);
            if kept.len() == MAX_FINDS {
                break;
            }
        }
    }
    kept.into_iter()
        .map(|(i, r, v)| Find {
            centre: [
                ((i % w) as f32 + 0.5) / w as f32,
                ((i / w) as f32 + 0.5) / h as f32,
            ],
            radius: RADIUS_OF * r * scale / dw.max(dh) as f32,
            contrast: v,
        })
        .collect()
}

/// A [`find`] on its own thread, so the window keeps drawing while it looks (the author, 2026-10-20: a press
/// froze the window and then often showed nothing).
pub struct Search {
    /// How far it has got, as `f32` bits.
    progress: std::sync::Arc<std::sync::atomic::AtomicU32>,
    finds: std::sync::mpsc::Receiver<Vec<Find>>,
}

impl Search {
    /// Starts looking for the dust in `img` at `sensitivity`.
    pub fn spawn(img: std::sync::Arc<ImageBuf>, sensitivity: f32) -> Self {
        let (search, tx) = Self::answered_by(0.0);
        let progress = std::sync::Arc::clone(&search.progress);
        std::thread::Builder::new()
            .name("rustroom-dust".into())
            .spawn(move || {
                let store = |p: f32| {
                    progress.store(p.to_bits(), std::sync::atomic::Ordering::Relaxed);
                };
                let finds = find_with_progress(&img, sensitivity, &store);
                store(1.0);
                let _ = tx.send(finds);
            })
            .expect("the dust thread starts");
        search
    }

    /// A search whose finds arrive through the sender, standing at `progress` — the panel's tests hold one
    /// open to see the window while it looks.
    pub fn answered_by(progress: f32) -> (Self, std::sync::mpsc::Sender<Vec<Find>>) {
        let (tx, finds) = std::sync::mpsc::channel();
        let progress = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(progress.to_bits()));
        (Self { progress, finds }, tx)
    }

    /// How far it has got, 0..=1.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(std::sync::atomic::Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    /// The finds, once they are there — handed over once.
    pub fn poll(&self) -> Option<Vec<Find>> {
        self.finds.try_recv().ok()
    }
}

/// Remove dust over one photo — it lives on the session, so another photo starts clean.
#[derive(Default)]
pub struct Run {
    /// The Sensitivity of a press made while the photo was still loading: the search starts once it has.
    pub queued: Option<f32>,
    pub search: Option<Search>,
    /// What the last press did, shown under the button until the next one.
    pub said: Option<String>,
}

/// What a press did, given how many patches it `added` out of `found` finds.
pub fn outcome(added: usize, found: usize) -> String {
    match (added, found) {
        (0, 0) => "No dust found".to_string(),
        (0, _) => "No new dust found".to_string(),
        (1, _) => "Removed 1 dust spot".to_string(),
        (n, _) => format!("Removed {n} dust spots"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    /// A grey-blue linear gradient, `0.2 … 0.7` left to right.
    fn gradient(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| 0.2 + 0.5 * (i % w) as f32 / w as f32)
            .collect()
    }

    /// `lum` with soft discs `(cx, cy, radius, darkening)` multiplied in — a logistic edge a quarter of the
    /// radius wide, as an out-of-focus shadow on the sensor is.
    fn with_discs(mut lum: Vec<f32>, w: usize, discs: &[(f32, f32, f32, f32)]) -> Vec<f32> {
        for (i, v) in lum.iter_mut().enumerate() {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            for &(cx, cy, r, d) in discs {
                let rho = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                *v *= 1.0 - d / (1.0 + ((rho - r) / (0.25 * r)).exp());
            }
        }
        lum
    }

    fn image(lum: &[f32], w: usize, h: usize) -> ImageBuf {
        ImageBuf::from_vec(
            w,
            h,
            lum.iter().flat_map(|&v| [0.8 * v, 0.9 * v, v]).collect(),
        )
    }

    const DISCS: [(f32, f32, f32, f32); 5] = [
        (300.0, 300.0, 4.0, 0.15),
        (800.0, 300.0, 8.0, 0.10),
        (1300.0, 400.0, 12.0, 0.08),
        (500.0, 900.0, 16.0, 0.06),
        (1200.0, 900.0, 20.0, 0.05),
    ];

    #[test]
    fn finds_soft_dark_discs_on_a_gradient() {
        let (w, h) = (1600, 1200);
        let img = image(&with_discs(gradient(w, h), w, &DISCS), w, h);
        let finds = find(&img, DEFAULT_SENSITIVITY);
        for &(cx, cy, r, d) in &DISCS {
            let hit = finds.iter().find(|f| {
                let (x, y) = (f.centre[0] * w as f32, f.centre[1] * h as f32);
                ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() <= 0.3 * r.max(4.0)
            });
            let f =
                hit.unwrap_or_else(|| panic!("the disc at ({cx}, {cy}), r {r}, {d}: {finds:?}"));
            let px = f.radius * w.max(h) as f32;
            assert!(px >= r / 2.0 && px <= r * 2.0, "r {r} read as {px} px");
            assert!(f.contrast > 0.0);
        }
        assert_eq!(finds.len(), DISCS.len(), "nothing else is dust: {finds:?}");
    }

    /// A deterministic texture: white noise blurred a little, ±20 % — foliage or cloud, never dust.
    fn texture(w: usize, h: usize) -> Vec<f32> {
        let mut seed = 0x9e37_79b9_u32;
        let noise: Vec<f32> = (0..w * h)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();
        let soft = pipeline::spatial::gaussian_blur(&noise, w, h, 1.5);
        let peak = soft.iter().fold(0f32, |a, v| a.max(v.abs())).max(1e-6);
        gradient(w, h)
            .iter()
            .zip(&soft)
            .map(|(b, n)| b * (1.0 + 0.2 * n / peak))
            .collect()
    }

    #[test]
    fn none_on_texture() {
        let (w, h) = (1600, 1200);
        let finds = find(&image(&texture(w, h), w, h), DEFAULT_SENSITIVITY);
        assert!(
            finds.len() <= 2,
            "{} finds on texture: {:?}",
            finds.len(),
            &finds[..finds.len().min(5)]
        );
    }

    #[test]
    fn a_bright_spot_is_not_dust() {
        let (w, h) = (800, 600);
        // A dark control beside it: the detector is working, and still the bright disc is not dust.
        let discs = [(250.0, 300.0, 8.0, -0.10), (550.0, 300.0, 8.0, 0.10)];
        let img = image(&with_discs(gradient(w, h), w, &discs), w, h);
        let finds = find(&img, DEFAULT_SENSITIVITY);
        assert_eq!(finds.len(), 1, "{finds:?}");
        assert!(
            (finds[0].centre[0] * w as f32 - 550.0).abs() < 4.0,
            "{finds:?}"
        );
    }

    #[test]
    fn nothing_in_near_black() {
        let (w, h) = (800, 600);
        // The left half bright, the right near-black, a disc in the dark half.
        let lum: Vec<f32> = (0..w * h)
            .map(|i| if i % w < w / 2 { 0.7 } else { 0.0014 })
            .collect();
        // The same disc in the bright half is the control: it is found.
        let discs = [(600.0, 300.0, 8.0, 0.15), (200.0, 300.0, 8.0, 0.15)];
        let img = image(&with_discs(lum, w, &discs), w, h);
        let finds = find(&img, DEFAULT_SENSITIVITY);
        assert!(
            finds
                .iter()
                .any(|f| (f.centre[0] * w as f32 - 200.0).abs() < 4.0),
            "the control disc in the bright half was missed: {finds:?}"
        );
        assert!(
            finds.iter().all(|f| f.centre[0] < 0.5),
            "dust found in near-black: {finds:?}"
        );
    }

    #[test]
    fn sensitivity_finds_more_as_it_rises() {
        let (w, h) = (1600, 600);
        let discs: Vec<(f32, f32, f32, f32)> = [0.01, 0.02, 0.03, 0.04, 0.06, 0.08, 0.10, 0.12]
            .iter()
            .enumerate()
            .map(|(i, &d)| (100.0 + 190.0 * i as f32, 300.0, 8.0, d))
            .collect();
        let img = image(&with_discs(gradient(w, h), w, &discs), w, h);
        let n = |s: f32| find(&img, s).len();
        let (lo, mid, hi) = (n(10.0), n(50.0), n(90.0));
        assert!(
            hi >= mid && mid >= lo && hi > lo,
            "10: {lo}, 50: {mid}, 90: {hi}"
        );
    }

    /// Measured on the author's frames (2026-10-20): dark stones in gravel were found — one stone among
    /// others is darker than its ring's AVERAGE, but its neighbours make dark directions in the ring itself.
    #[test]
    fn a_stone_among_stones_is_not_dust() {
        let (w, h) = (800, 600);
        // A row of three discs 2.5 radii apart — stones — and a lone control disc on clear sky.
        let discs = [
            (385.0, 400.0, 6.0, 0.2),
            (400.0, 400.0, 6.0, 0.2),
            (415.0, 400.0, 6.0, 0.2),
            (400.0, 150.0, 6.0, 0.2),
        ];
        let finds = find(
            &image(&with_discs(gradient(w, h), w, &discs), w, h),
            DEFAULT_SENSITIVITY,
        );
        assert!(
            finds
                .iter()
                .any(|f| (f.centre[1] * h as f32 - 150.0).abs() < 4.0),
            "the lone control was missed: {finds:?}"
        );
        assert!(
            finds
                .iter()
                .all(|f| (f.centre[1] * h as f32 - 400.0).abs() > 20.0),
            "a stone among stones was taken for dust: {finds:?}"
        );
    }

    /// A disc 70 % darker than the sky is a thing in the scene, never a dust shadow.
    #[test]
    fn a_scene_object_too_dark_for_dust_is_not_dust() {
        let (w, h) = (800, 600);
        let discs = [(250.0, 300.0, 6.0, 0.7), (550.0, 300.0, 6.0, 0.15)];
        let finds = find(
            &image(&with_discs(gradient(w, h), w, &discs), w, h),
            DEFAULT_SENSITIVITY,
        );
        assert_eq!(finds.len(), 1, "{finds:?}");
        assert!(
            (finds[0].centre[0] * w as f32 - 550.0).abs() < 4.0,
            "{finds:?}"
        );
    }

    /// The ring rule itself: an even ring is dust; a line through the spot (two opposite directions as dark
    /// as the centre), an edge (half the ring dark) or a spot far too dark is not.
    #[test]
    fn dust_is_brighter_all_round() {
        let even = [0.12; RING_DIRECTIONS];
        assert!(dust_like(even));
        let mut line = even;
        line[0] = 0.0;
        line[RING_DIRECTIONS / 2] = 0.0;
        assert!(!dust_like(line), "a line through the spot");
        let mut edge = even;
        for v in edge.iter_mut().take(RING_DIRECTIONS / 2 - 1) {
            *v = -0.05;
        }
        assert!(!dust_like(edge), "an edge");
        assert!(!dust_like([0.9; RING_DIRECTIONS]), "far too dark for dust");
        assert!(!dust_like([-0.1; RING_DIRECTIONS]), "a bright spot");
    }

    /// The author (2026-10-20): pressing Remove dust "often nothing is happening". The search runs in the
    /// background, says how far it has got, and hands back what `find` would.
    #[test]
    fn a_search_reports_its_progress_and_its_finds() {
        let (w, h) = (1600, 1200);
        let img = std::sync::Arc::new(image(&with_discs(gradient(w, h), w, &DISCS), w, h));
        let search = Search::spawn(std::sync::Arc::clone(&img), DEFAULT_SENSITIVITY);
        let start = std::time::Instant::now();
        let mut seen = Vec::new();
        let finds = loop {
            seen.push(search.progress());
            if let Some(f) = search.poll() {
                break f;
            }
            assert!(start.elapsed().as_secs() < 120, "the search never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(
            finds,
            find(&img, DEFAULT_SENSITIVITY),
            "the same finds as `find`"
        );
        assert!(
            seen.windows(2).all(|p| p[1] >= p[0]),
            "progress never goes back: {seen:?}"
        );
        assert!((0.0..=1.0).contains(&search.progress()));
        assert_eq!(search.progress(), 1.0, "done is all the way");
        assert!(search.poll().is_none(), "handed over once");
    }

    /// The steps are reported as they finish — the preparation and each radius — ending at 1.
    #[test]
    fn find_reports_each_step() {
        let (w, h) = (400, 300);
        let img = image(&gradient(w, h), w, h);
        let seen = std::cell::RefCell::new(Vec::new());
        find_with_progress(&img, DEFAULT_SENSITIVITY, &|p| seen.borrow_mut().push(p));
        let steps = (RADII.len() + 1) as f32;
        let want: Vec<f32> = (1..=RADII.len() + 1).map(|k| k as f32 / steps).collect();
        assert_eq!(seen.into_inner(), want);
    }

    fn finds() -> Vec<Find> {
        [[0.2, 0.3], [0.5, 0.5], [0.8, 0.6]]
            .iter()
            .map(|&centre| Find {
                centre,
                radius: 0.004,
                contrast: 0.05,
            })
            .collect()
    }

    fn flat(_: [f32; 2]) -> [f32; 3] {
        [0.4, 0.45, 0.5]
    }

    #[test]
    fn every_find_becomes_a_heal_patch() {
        let tool = crate::spot_tool::SpotTool::default();
        let spots = to_spots(&finds(), &|d| d, 1.5, &[], tool, &flat);
        assert_eq!(spots.len(), 3);
        for (s, f) in spots.iter().zip(finds()) {
            assert_eq!(s.mode, edit_model::SpotMode::Heal);
            assert_eq!(s.centre, f.centre);
            assert!(
                (s.radius - SPOT_SCALE * f.radius).abs() < 1e-7,
                "{}",
                s.radius
            );
            assert_eq!(s.feather, tool.feather);
            assert_ne!(s.source, s.centre, "a patch copied onto itself");
        }
    }

    /// Review Focus 2: Remove dust pressed again adds nothing where a patch already sits — nor twice for one
    /// spot found twice.
    #[test]
    fn a_find_under_an_existing_patch_is_skipped() {
        let tool = crate::spot_tool::SpotTool::default();
        let first = to_spots(&finds(), &|d| d, 1.5, &[], tool, &flat);
        assert!(to_spots(&finds(), &|d| d, 1.5, &first, tool, &flat).is_empty());
        let twice: Vec<Find> = finds().into_iter().chain(finds()).collect();
        assert_eq!(to_spots(&twice, &|d| d, 1.5, &[], tool, &flat).len(), 3);
    }

    /// Review Focus 3: through a lens's distortion each patch lands where its dust is — the patches' space is
    /// after the lens, the finds are in the decode.
    #[test]
    fn a_find_lands_on_its_dust_through_the_lens() {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>M</maker><model>C</model><mount>X</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>M</maker><model>L 50mm</model><mount>X</mount><cropfactor>1.0</cropfactor>
                <calibration><distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/></calibration>
            </lens></lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let corr = lens_profile::Corrector::new(
            &db.find("M", "C", "L 50mm").unwrap(),
            &lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
            3000,
            2000,
            lens_profile::Corrections {
                distortion: true,
                tca: false,
                vignetting: false,
                scale: 1.0,
            },
        );
        assert!(corr.is_some(), "the fixture's lens corrects");
        let dm = crate::ai_space::DecodeMap::new(
            &edit_model::Geometry::default(),
            edit_model::Perspective::default(),
            3000,
            2000,
            corr,
            3000,
            2000,
        );
        let tool = crate::spot_tool::SpotTool::default();
        let spots = to_spots(&finds(), &|d| dm.post_lens_of(d), 1.5, &[], tool, &flat);
        assert_eq!(spots.len(), 3);
        let mut moved = 0f32;
        for (s, f) in spots.iter().zip(finds()) {
            let back = dm.post_lens_to_decode(s.centre);
            assert!(
                (back[0] - f.centre[0]).abs() < 1e-4 && (back[1] - f.centre[1]).abs() < 1e-4,
                "{:?} came back as {back:?}",
                f.centre
            );
            moved =
                moved.max((s.centre[0] - f.centre[0]).abs() + (s.centre[1] - f.centre[1]).abs());
        }
        assert!(
            moved > 1e-3,
            "the fixture: the lens moved nothing ({moved})"
        );
    }
}

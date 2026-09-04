//! Finding the near-vertical lines a guided correction needs, without the user drawing them.
//!
//! Pure: a luminance plane in, [`super::guides::Guide`]s out. No images, no I/O, no GUI.
//!
//! ⚠⚠ **This does NOT reuse `app::visualize::edge_view`, and the roadmap's M7.4 note that it
//! could is wrong.** That function is `|luma − gaussian_blur|` tuned for dust spots: it carries
//! no gradient *direction*, which is exactly what makes a narrow angular window cheap and
//! specific, and it lives in the app lib where this crate cannot call it.
//! [`crate::spatial::sobel`] is the operator this needs.

use super::guides::Guide;
use crate::spatial;

/// How far from vertical a candidate line may lean, in degrees.
///
/// ⚠⚠ **Chosen from the asymmetry of the failure modes, not for tidiness.** A missed line means
/// the button appears broken and there is nothing on screen to act on; a false positive draws a
/// **visible, editable** guide the user nudges or deletes. So the window is wide rather than
/// tight: ±20° misses exactly the shot the feature exists for — a tall building from close with
/// strong upward tilt, where the outer verticals lean 20–25° near the frame edges — while a full
/// 180° accumulator is pure waste with one family.
pub const MAX_LEAN_DEG: f32 = 30.0;

/// The contrast an edge must carry to cast a vote, in **8-bit levels across the edge**.
///
/// ⚠⚠ **Absolute, where this was a fraction of the frame's own strongest gradient — and that
/// relative form was the direct cause of both of M7.4's measured failures.** It was simultaneously
/// too permissive on a soft frame, where a smooth sky gradient clears a quarter of a small maximum
/// (Walhalla: two lines found *in the sky*, giving Vertical −42.5 and columns leaning left), and
/// too strict on a contrasty one, where something brighter than the building sets the bar above the
/// building's own edges (Weltenburg Abbey: nothing found at all). ⚠ A relative threshold also
/// structurally cannot express *"there is no vertical in this photograph"*, which is why
/// [`MIN_LENGTH_FRACTION`] had to be added mid-flight just to stop a landscape returning two lines.
///
/// ⚠ Only meaningful because [`verticals`] display-encodes first: in linear light the same number
/// is a strong edge in shadow and an invisible one near white.
///
/// ⚠⚠ **Read off `detect_probe --sweep`, and the evidence is recorded here so the choice can be
/// revisited — the rule `MEDIAN_TARGET` and `TEXTURE_GUIDED_EPS` both follow.**
///
/// Across 70 of the author's RAWs, the fraction firing (two lines found) against the floor —
/// and the whole table moves once [`MIN_RUN_FRACTION`] is doing the noise rejection:
///
/// | levels | 2 | 4 | 6 | **8** | 12 | 16 | 24 | 32 | 48 | 64 | 96 |
/// |---|---|---|---|---|---|---|---|---|---|---|---|
/// | fires, votes counted anywhere | 97% | 96% | 93% | 84% | 53% | 34% | 29% | 27% | 20% | 13% | 7% |
/// | fires, run required contiguous | 54% | 37% | 29% | **26%** | 20% | 16% | 13% | 11% | 6% | 4% | 1% |
///
/// ⚠⚠ **The two constants trade off, and that is why this is 8 rather than 24.** Without
/// contiguity the floor was the *only* thing rejecting noise, so it had to be high — 24, where the
/// fire rate was 29%. With contiguity, floor 8 fires on **26%**, better than 24 ever managed, *and*
/// Weltenburg Abbey is detected again: its stonework is faint under an overcast sky, so it needs a
/// low floor, and it is lost above about 10.
///
/// ⚠ **A value of 24 was shipped briefly and was wrong for a reason worth keeping**: it was chosen
/// while the length test still counted votes scattered anywhere along a line, so it was
/// compensating for a different bug. Fixing that bug moved the right answer.
///
/// ⚠ **Still outstanding: this is a fire *rate*, not precision and recall.** A rate cannot
/// distinguish "fires less" from "fires better", and the labelled corpus that would is the user's
/// to write.
pub const MIN_EDGE_LEVELS: f32 = 8.0;

/// Sobel's response to a unit step: the kernel sums to 4 on the leading side.
///
/// ⚠ Dividing by it is what makes [`MIN_EDGE_LEVELS`] mean the contrast *across an edge* rather
/// than a raw Sobel magnitude, which is four times larger and a quantity nobody can reason about.
const SOBEL_STEP_GAIN: f32 = 4.0;

/// How many angle bins the accumulator uses across the window.
///
/// ⚠ Unchanged from the plan's 61, i.e. 1° per bin across ±30°. The lean assertion in
/// `two_converging_verticals_are_found_at_the_angles_they_were_drawn` has a 4° tolerance, so the
/// quantisation contributes about an eighth of the budget it is allowed.
const ANGLE_BINS: usize = 61;

/// How many offset bins the accumulator uses across the frame.
///
/// ⚠ Unchanged from the plan's 180 — about 2 px per bin on a 256 px fixture and about 4 px on the
/// 512 px analysis image, which is the width of a painted building edge rather than finer.
///
/// ⚠ The span is `cx + cy·tan(MAX_LEAN_DEG)`, **not** the frame's half-diagonal as the plan had
/// it: a line leaning the full 30° through a bottom corner has an offset of 202 px on a 256 px
/// frame where the half-diagonal is 181, so a diagonal-sized range silently discarded the votes
/// of exactly the steepest lines the window exists to admit.
const OFFSET_BINS: usize = 180;

/// A peak must reach this fraction of the strongest peak's vote to be a candidate at all.
///
/// ⚠⚠ **Lowered from the plan's starting value of 0.35 to 0.25, and the reason is a real
/// property of the accumulator rather than a red test being made green.** Two near-coincident
/// edges — the case the separation rule exists for — land in adjacent offset bins and their votes
/// partly merge, so the *strongest* peak in that frame is close to twice a lone edge's. A
/// genuinely weaker but well-separated third edge then has to clear a fraction of that doubled
/// peak, and at 0.35 the fixture's deliberately-weaker distant edge was rejected before the
/// separation rule ever saw it. 0.25 admits it. The cost is stated in caveat 4: a wider net for
/// false positives, which is the asymmetry [`MAX_LEAN_DEG`] is already chosen on — a false
/// positive is a visible, editable guide.
const PEAK_FRACTION: f32 = 0.25;

/// How long a line must be, as a fraction of the frame height, to be a candidate at all.
///
/// ⚠⚠ **A SIXTH CONSTANT, NOT IN THE PLAN — and the feature does not work without it.** Every
/// other threshold here is *relative* to the frame's own strongest edge, and a relative threshold
/// structurally cannot express "there is no vertical in this photograph": whatever stray
/// gradients exist define the maximum, and a fraction of that maximum is then always cleared. A
/// flat frame is silent only because its gradient is exactly zero, which no photograph is.
///
/// The concrete case is the milestone's own GUI check — Auto on a landscape must report "need
/// two" rather than inventing guides — and the test that found it is
/// `a_line_beyond_the_window_is_not_found_and_one_inside_it_is`, where the few stray gradients at
/// a rejected line's endpoints became the strongest thing in the accumulator and were duly
/// returned.
///
/// ⚠ It is measured against a **count of voting pixels**, in a second accumulator beside the
/// magnitude-weighted one, and not against those magnitudes. Thresholding the magnitude sum
/// conflates *length* with *contrast*: a short, very contrasty stub outvotes a long, faint
/// building edge, so an 18 px stub cleared a threshold meant to require 35% of the frame's
/// height. A count says what the constant claims to say, and is independent of how contrasty the
/// edge happens to be. The magnitudes still decide which peak is *best*, which is what keeps a
/// strong edge preferred over a faint one of the same length.
///
/// ⚠⚠ **The run must be CONTIGUOUS, and that is the whole rule.** Counting votes anywhere along
/// the infinite line lets a candidate reach its threshold from fragments scattered over the frame —
/// a silhouette here, a railing there, a cloud edge — and be declared a building edge. Reported in
/// use as the guides being "put at random, not fixed to the building", and measured: on the
/// author's frames the winning lines had 24–41% of rows voting but a longest contiguous run of
/// **7–13%**. They were statistically collinear noise, not edges.
///
/// ⚠ The accumulated *count* is still used, as a cheap prefilter: a contiguous run can never
/// exceed the total, so `count < MIN_RUN_FRACTION · h` rules a bin out without walking it.
///
/// ⚠ Measured on real frames, a genuine building edge reaches about **28%** of the frame height,
/// which is what bounds this from above — a building rarely fills the frame, so its verticals are
/// shorter than the frame is tall.
const MIN_RUN_FRACTION: f32 = 0.20;

/// How far either side of a candidate line the contiguity walk looks for support, in pixels.
///
/// ⚠ It is the offset **bin** half-width, not a tuning knob: the accumulator collects a line's
/// votes into a bin a few pixels wide, so support is not confined to one column. A single-column
/// walk understates the run for any line that is not exactly axis-aligned — which was the first
/// version of this measurement, and it would have set the threshold far too low.
fn offset_band(off_max: f32) -> i64 {
    ((off_max / (OFFSET_BINS - 1) as f32).ceil() as i64).max(1)
}

/// The longest run of consecutive rows in which this line has support.
///
/// ⚠ It repeats the vote condition exactly — magnitude over the floor **and** the direction gate —
/// because a horizontal edge crossing the candidate is not evidence for it. Dropping the gate here
/// would let the very frames the gate exists to reject supply contiguous support.
#[allow(clippy::too_many_arguments)]
fn longest_run(
    g: &crate::spatial::Gradient,
    w: usize,
    h: usize,
    cx: f32,
    cy: f32,
    angle: f32,
    offset: f32,
    floor: f32,
    lean_max: f32,
    band: i64,
) -> usize {
    let t = angle.tan();
    let (mut best, mut run) = (0usize, 0usize);
    for y in 0..h {
        let x = (cx + offset + (y as f32 - cy) * t).round() as i64;
        let supported = (-band..=band).any(|d| {
            let c = x + d;
            if !(0..w as i64).contains(&c) {
                return false;
            }
            let i = y * w + c as usize;
            g.mag[i] >= floor && g.gy[i].atan2(g.gx[i].abs().max(1e-9)).abs() <= lean_max
        });
        if supported {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// How far apart, as a fraction of the frame width, two chosen lines' offsets must be.
///
/// ⚠ Unchanged from the plan's 0.15. On a 256 px fixture that is 38 px, which rejects the 6 px
/// pair in `the_two_lines_returned_are_separated_rather_than_the_two_strongest` by a wide margin
/// and admits the 110 px spread of a real converging pair by a wider one.
const MIN_SEPARATION: f32 = 0.15;
/// How far a candidate may sit from the guide it refines, as a fraction of frame width.
///
/// ⚠⚠ **The bound is load-bearing, not a tuning detail.** Without it a strong edge that is not the
/// one you aimed at captures your line — the person's silhouette beside Walhalla's building corner
/// is the measured case — and refinement re-creates the very bug it exists to fix. It must be small
/// enough that a wrong snap is close to impossible.
///
/// ⚠ **Measured with `detect_overlay --refine`, not asserted** — this milestone has already had one
/// guess overturned by measurement, the spec's "starting band of 6–10 levels" for
/// [`MIN_EDGE_LEVELS`], which fired on 84% of frames.
///
/// Guides traced 1–3% off the true edge on the author's two known frames had to move:
///
/// | frame | drawn | moved |
/// |---|---|---|
/// | Abbey | 0.90 → 0.98 | 0.011 |
/// | Abbey | 0.87 → 0.95 | 0.021 |
/// | Abbey | 0.31 → 0.32 | 0.027 |
/// | Walhalla | 0.55 → 0.59 | 0.022 |
/// | Walhalla | 0.59 → 0.63 | 0.024 |
///
/// So 0.03 admits every observed placement error with a little margin. ⚠ The competing wrong edge
/// on those frames — Walhalla's sky line against its building corner — is **0.16 away, five times
/// the radius**, so there is no case here where a guide aimed at the building could capture it.
///
/// ⚠⚠ **A guide traced along a LEANING edge is the case that matters, and measuring with a
/// straight vertical probe measures the wrong thing**: against an edge leaning 16° the gap at one
/// end is large however well the line is placed, so the first measurement showed the abbey never
/// snapping at all. `--refine` takes `top:bottom` for that reason.
pub const SNAP_RADIUS: f32 = 0.03;

/// One accumulator peak.
struct Peak {
    angle: f32,
    offset: f32,
    votes: f32,
}

/// Everything a caller needs to turn accumulator peaks into lines, without re-deriving the frame's
/// geometry.
///
/// ⚠ Exists so [`verticals_with_floor`] and [`refine`] share one candidate pipeline — floor,
/// direction gate, contiguity, peak ranking — and differ only in how they *choose*. Two pipelines
/// would drift, which is the rule `test_support.rs` already states for duplicated helpers.
struct Candidates {
    /// Surviving peaks, strongest first.
    peaks: Vec<Peak>,
    cx: f32,
    cy: f32,
}

impl Candidates {
    /// No candidates at all — the degenerate frames.
    fn empty() -> Self {
        Self {
            peaks: Vec::new(),
            cx: 0.0,
            cy: 0.0,
        }
    }

    /// Where this peak's line crosses row `y`, in pixels.
    fn at(&self, p: &Peak, y: f32) -> f32 {
        self.cx + p.offset + (y - self.cy) * p.angle.tan()
    }

    /// A peak as a frame-normalised guide spanning the full height.
    fn guide(&self, p: &Peak, w: usize, h: usize) -> Guide {
        Guide {
            from: [self.at(p, 0.0) / w as f32, 0.0],
            to: [self.at(p, h as f32) / w as f32, 1.0],
        }
    }
}

/// Candidate near-vertical lines, best first, at most two.
///
/// The plane is single-channel luminance of a frame `w` × `h`; the returned guides are in
/// `[0, 1]` frame-normalised coordinates, ready for [`super::guides::solve`].
///
/// ⚠⚠ **Returns the two most widely SEPARATED strong lines, not the top two by vote.**
/// `guides.rs` records why: "the vanishing point of two nearly-parallel lines sits enormously far
/// away and moves wildly for a pixel of drawing error, so a roll taken from it is
/// ill-conditioned exactly when the correction is gentlest." Two near-coincident detections on
/// one edge of one building are the commonest way that happens, and taking them makes the solve
/// unstable while looking like success.
pub fn verticals(luma: &[f32], w: usize, h: usize) -> Vec<Guide> {
    verticals_with_floor(luma, w, h, MIN_EDGE_LEVELS)
}

/// As [`verticals`], with the contrast floor supplied in 8-bit levels.
///
/// ⚠⚠ **Exists so `detect_probe --sweep` can measure a candidate floor against a labelled corpus,
/// and [`verticals`] delegates here rather than the two sharing copied code.** A second
/// implementation is how a swept value comes to be right for a detector nobody ships — the sweep
/// must measure exactly what production runs.
fn candidates(luma: &[f32], w: usize, h: usize, floor_levels: f32) -> Candidates {
    if w < 8 || h < 8 || luma.len() != w * h {
        return Candidates::empty();
    }
    // ⚠⚠ **Display-encoded before the Sobel, and this is what lets the floor below be an absolute
    // number.** `luma` arrives as `color::luminance` — linear, scene-referred ProPhoto — where a
    // gradient of 0.01 is a strongly visible edge in shadow and invisible near white. One
    // threshold cannot be right across a frame in that space.
    //
    // ⚠⚠ This project has the identical bug on record in the Whites slider: "the region weights
    // read as perceptual thirds… but `l` arrives as **linear** luminance, where 2/3 is sRGB 0.85
    // and 1/3 is sRGB 0.60", so "the Whites slider did nothing on most photographs". The weights
    // are taken on display-encoded luminance now, and so is this.
    //
    // ⚠ Done **here** rather than in `perspective_detect::analysis_luma`, so a caller cannot hand
    // this function the wrong colour space — the reasoning `LensCorrectionStage` records for
    // building its own `Corrector` rather than accepting one: handing it in "would make a size
    // mismatch representable, and it would be silently wrong rather than an error."
    //
    // ⚠ `linear_to_srgb` clamps to [0, 1], which also bounds the scene-referred range. Nothing is
    // lost: a clipped highlight has no gradient to find.
    let encoded: Vec<f32> = luma.iter().copied().map(color::linear_to_srgb).collect();
    let g = spatial::sobel(&encoded, w, h);
    // ⚠ An absolute floor, in the encoded units the Sobel just measured. Nothing about the rest of
    // the frame can raise or lower it — which is the whole point.
    let floor = floor_levels / 255.0 * SOBEL_STEP_GAIN;

    let lean_max = MAX_LEAN_DEG.to_radians();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    // The furthest a line within the window can sit from the centre column — see `OFFSET_BINS`.
    let off_max = cx + cy * lean_max.tan();
    // The angle each bin stands for, precomputed: the sweep below reads all of them per pixel.
    let angles: Vec<f32> = (0..ANGLE_BINS)
        .map(|ab| -lean_max + 2.0 * lean_max * ab as f32 / (ANGLE_BINS - 1) as f32)
        .collect();
    let tans: Vec<f32> = angles.iter().map(|a| a.tan()).collect();
    let mut acc = vec![0.0f32; ANGLE_BINS * OFFSET_BINS];
    // How many pixels voted for each line, as against how strongly — see `MIN_LENGTH_FRACTION`.
    let mut cnt = vec![0.0f32; ANGLE_BINS * OFFSET_BINS];

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if g.mag[i] < floor {
                continue;
            }
            // ⚠⚠ **The direction gate.** A line's own direction is perpendicular to its gradient,
            // so a near-vertical LINE has a near-horizontal GRADIENT: |gy| must be small. This is
            // what a magnitude-only edge map — `visualize::edge_view` — cannot express, and it is
            // why this accumulator stays small instead of collecting every horizon and kerb.
            //
            // ⚠⚠ **Only the MAGNITUDE of the direction is used, and both signs are discarded on
            // purpose.** A line has an orientation but no direction: a bright edge on a dark
            // ground produces gradients pointing opposite ways along its two flanks, and either
            // is equally good evidence for the same line. Because the gate below is symmetric,
            // `|atan2(gy, |gx|)|` is *identically equal* to the folded
            // `|atan2(-gy·sgn(gx), |gx|)|`, so folding the gradient into one half-plane first
            // would be dead code — it was written, found inert by its own sabotage, and removed.
            //
            // ⚠ That equality holds only while the direction is a **gate**. An earlier version
            // used the per-pixel direction to *pick* the single angle bin to vote in, and there
            // the sign is load-bearing: unfolded, the two flanks of one line report leans of
            // opposite sign, vote into bins symmetric about vertical, and the recovered angle
            // collapses toward zero whatever was drawn. See the sweep's own note below.
            let lean = g.gy[i].atan2(g.gx[i].abs().max(1e-9));
            if lean.abs() > lean_max {
                continue;
            }
            // ⚠⚠ **The direction is the GATE; the angle comes from the accumulator.** The plan's
            // code used the per-pixel gradient direction to *pick* the single angle bin this
            // pixel votes in, and that is a different and much more fragile algorithm than the
            // spec's — which says the gate "keeps the accumulator small and stops roof diagonals
            // voting" and nothing more. Picking the bin fails for a reason worth recording: the
            // direction of one pixel's gradient is a *local* estimate, noisy by several degrees
            // on a real edge, so a line's pixels scatter across a dozen 1° bins and both the
            // peak and the recovered angle are diluted. Sweeping the window instead lets every
            // pixel of one line vote coherently for the same (angle, offset) pair, which is what
            // makes the peak sharp — the classic Hough argument.
            for (ab, &t) in tans.iter().enumerate() {
                // Offset of the line through this pixel, measured along x at the centre row.
                let off = (x as f32 - cx) - (y as f32 - cy) * t;
                let ob = (((off + off_max) / (2.0 * off_max)) * (OFFSET_BINS - 1) as f32).round();
                if !(0.0..=(OFFSET_BINS - 1) as f32).contains(&ob) {
                    continue;
                }
                acc[ab * OFFSET_BINS + ob as usize] += g.mag[i];
                cnt[ab * OFFSET_BINS + ob as usize] += 1.0;
            }
        }
    }

    // Collect peaks that are local maxima in the accumulator.
    let mut peaks: Vec<Peak> = Vec::new();
    let best = acc.iter().copied().fold(0.0f32, f32::max);
    if best <= 1e-6 {
        return Candidates::empty();
    }
    // ⚠ Absolute as well as relative — see `MIN_RUN_FRACTION`. Without the absolute half a
    // photograph with no vertical in it still returns two. This is the cheap prefilter; the
    // contiguity walk below is what actually decides.
    let min_len = MIN_RUN_FRACTION * h as f32;
    for ab in 0..ANGLE_BINS {
        for ob in 1..OFFSET_BINS - 1 {
            let v = acc[ab * OFFSET_BINS + ob];
            if v < best * PEAK_FRACTION || cnt[ab * OFFSET_BINS + ob] < min_len {
                continue;
            }
            if v < acc[ab * OFFSET_BINS + ob - 1] || v < acc[ab * OFFSET_BINS + ob + 1] {
                continue;
            }
            // ⚠ A local maximum in the **angle** direction too, which the sweep makes necessary:
            // one line's votes form a ridge across neighbouring angle bins, and without this
            // every bin of that ridge is a candidate describing the same edge.
            if ab > 0 && v < acc[(ab - 1) * OFFSET_BINS + ob] {
                continue;
            }
            if ab + 1 < ANGLE_BINS && v < acc[(ab + 1) * OFFSET_BINS + ob] {
                continue;
            }
            let off = -off_max + 2.0 * off_max * ob as f32 / (OFFSET_BINS - 1) as f32;
            peaks.push(Peak {
                angle: angles[ab],
                offset: off,
                votes: v,
            });
        }
    }
    // ⚠⚠ **Hough finds candidates; this is what verifies them.** The accumulator cannot express
    // contiguity — it sums independent votes — so a candidate is walked once and kept only if its
    // support is one unbroken run. Done here rather than per bin because the prefilter above has
    // already discarded almost everything, and the walk is O(h) per survivor.
    let band = offset_band(off_max);
    let min_run = (MIN_RUN_FRACTION * h as f32).ceil() as usize;
    peaks.retain(|p| {
        longest_run(&g, w, h, cx, cy, p.angle, p.offset, floor, lean_max, band) >= min_run
    });

    // ⚠ Strongest first, here rather than in either caller: both want that order, and sorting
    // twice is how the two would come to disagree about which peak is "best".
    peaks.sort_by(|a, b| {
        b.votes
            .partial_cmp(&a.votes)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Candidates { peaks, cx, cy }
}

/// Candidate near-vertical lines, best first, at most two.
///
/// ⚠ The choosing half. Everything before it lives in [`candidates`], which [`refine`] shares.
pub fn verticals_with_floor(luma: &[f32], w: usize, h: usize, floor_levels: f32) -> Vec<Guide> {
    let c = candidates(luma, w, h, floor_levels);
    if c.peaks.is_empty() {
        return Vec::new();
    }

    // ⚠⚠ **Separation, not the top two by vote.** The strongest peak is taken, then the strongest
    // peak that is far enough from it — because a vanishing point from two nearly-coincident
    // lines is ill-conditioned exactly when the correction is gentlest, which `guides.rs`
    // records as having been "reported in use as the image coming out rotated so that carefully
    // drawn vertical guides ended up not vertical at all."
    let min_sep = w as f32 * MIN_SEPARATION;
    let mut chosen = vec![&c.peaks[0]];
    if let Some(p) = c
        .peaks
        .iter()
        .skip(1)
        .find(|p| (p.offset - c.peaks[0].offset).abs() >= min_sep)
    {
        chosen.push(p);
    }
    chosen.iter().map(|p| c.guide(p, w, h)).collect()
}

/// Each guide replaced by the strongest real edge near it, or returned exactly as drawn.
///
/// ⚠⚠ **It reuses [`candidates`] whole — floor, direction gate, contiguity, peak ranking — and
/// replaces only the choice.** So an edge that failed contiguity can never be snapped to, and every
/// property the contrast-floor and contiguity work established still holds here.
///
/// ⚠ [`MIN_SEPARATION`] deliberately does **not** apply: it exists to stop the *detector* choosing
/// an ill-conditioned pair, and here the pair is the user's own. Drawing two near-parallel guides is
/// already possible and already ill-conditioned; this makes nothing worse.
pub fn refine(luma: &[f32], w: usize, h: usize, guides: &[Guide]) -> Vec<Guide> {
    let c = candidates(luma, w, h, MIN_EDGE_LEVELS);
    let radius = SNAP_RADIUS * w as f32;
    guides
        .iter()
        .map(|d| {
            // ⚠ Verticals only, and the family test is `|dy| >= |dx|` — the same rule the guide
            // overlay uses to colour a guide, so there is one definition of "vertical" rather than
            // two that can disagree.
            if (d.to[1] - d.from[1]).abs() < (d.to[0] - d.from[0]).abs() {
                return *d;
            }
            // ⚠⚠ The metric is the **largest gap anywhere down the frame**, which subsumes angle
            // and offset in one number: a candidate matching the angle but sitting 20 px away
            // fails, and so does one starting on the line and diverging. Two thresholds in two
            // different units would be a quantity nobody can judge by eye.
            //
            // ⚠ Evaluated at the two ends only. Both lines are straight, so their separation is
            // monotonic between them and the maximum is necessarily at an endpoint.
            let drawn_at = |y: f32| {
                let t = y / h as f32;
                (d.from[0] + (d.to[0] - d.from[0]) * t) * w as f32
            };
            // ⚠ `find` on a list already sorted strongest-first **is** "the strongest within the
            // radius" — there is no second sort, and adding one would be a second ranking rule.
            c.peaks
                .iter()
                .find(|p| {
                    let gap = |y: f32| (c.at(p, y) - drawn_at(y)).abs();
                    gap(0.0).max(gap(h as f32)) <= radius
                })
                .map_or(*d, |p| c.guide(p, w, h))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paint a bright line into a dark plane, from `(x0, y0)` to `(x1, y1)` in pixels, with a
    /// smooth cross-section.
    ///
    /// ⚠⚠ **The smooth profile is load-bearing, and the plan's hard-pixel version tested
    /// something else entirely.** Painting `p[..] = 1.0` at rounded coordinates makes a
    /// **staircase**, not a line — and a staircase is genuinely built out of axis-aligned
    /// micro-edges, which any gradient operator will faithfully report. Two concrete
    /// consequences, both of which read as detector bugs and are not:
    ///
    /// * A line of slope −0.125 steps one pixel in x every **8 rows**, so it is a vertical run
    ///   almost everywhere. Sobel returns `gy ≈ 0` along it and the direction says "lean 0",
    ///   which is why the recovered angle came back as exactly zero for a line drawn at 7.1°.
    /// * A line at 50° steps one pixel every **0.84 rows**, so its risers are real short vertical
    ///   edges. They pass a near-vertical gate, and the out-of-window line was duly "detected".
    ///
    /// A photographic edge at the 512 px analysis size is anti-aliased over a pixel or two, so a
    /// Gaussian cross-section (σ = 1.2 px, wide enough for a 3×3 Sobel to measure a direction
    /// from) is the faithful fixture. Both flanks of the line are still present, so the
    /// half-plane fold in `verticals` remains load-bearing.
    ///
    /// ⚠ It clamps to the frame, so every caller must keep its endpoints **inside** — see
    /// `a_line_beyond_the_window_is_not_found_and_one_inside_it_is`, whose original fixture did
    /// not and accidentally painted a vertical up the right edge.
    fn line(p: &mut [f32], w: usize, h: usize, x0: f32, y0: f32, x1: f32, y1: f32) {
        const SIGMA: f32 = 1.2;
        const REACH: i64 = 4;
        let steps = 8 * h;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let xc = x0 + (x1 - x0) * t;
            let yc = y0 + (y1 - y0) * t;
            let yi = match yc.round() as i64 {
                y if (0..h as i64).contains(&y) => y as usize,
                _ => continue,
            };
            let base = xc.round() as i64;
            for dx in -REACH..=REACH {
                let xi = match base + dx {
                    x if (0..w as i64).contains(&x) => x as usize,
                    _ => continue,
                };
                let d = xi as f32 - xc;
                let cov = (-(d * d) / (SIGMA * SIGMA)).exp();
                let v = 0.05 + 0.95 * cov;
                let cell = &mut p[yi * w + xi];
                *cell = cell.max(v);
            }
        }
    }

    fn dark(w: usize, h: usize) -> Vec<f32> {
        vec![0.05f32; w * h]
    }

    /// The angle a guide makes with vertical, in degrees, on a square frame.
    fn lean(g: &Guide) -> f32 {
        let dx = g.to[0] - g.from[0];
        let dy = g.to[1] - g.from[1];
        let a = dx.atan2(dy).to_degrees().abs();
        a.min(180.0 - a)
    }

    /// ⚠⚠ **Assert the recovered GEOMETRY, not merely that two lines came back.** `guides.rs`
    /// records the sibling failure directly: `a_horizontal_family_engages_the_yaw` "only asserted
    /// both angles were *non-zero* — and a wrong angle is still an angle."
    #[test]
    fn two_converging_verticals_are_found_at_the_angles_they_were_drawn() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // Converging: the left edge leans right, the right edge leans left.
        line(&mut p, w, h, 60.0, 250.0, 90.0, 10.0);
        line(&mut p, w, h, 200.0, 250.0, 170.0, 10.0);
        let found = verticals(&p, w, h);
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
        for g in &found {
            assert!(
                g.family() == super::super::guides::Family::Vertical,
                "a detected line was not classified vertical: {g:?}"
            );
            // Drawn at atan(30/240) = 7.1° from vertical.
            let l = lean(g);
            assert!(
                (l - 7.1).abs() < 4.0,
                "expected ~7.1° of lean, got {l} for {g:?}"
            );
        }
        // And they really are the two different edges, not one edge twice.
        let mid = |g: &Guide| (g.from[0] + g.to[0]) / 2.0;
        assert!(
            (mid(&found[0]) - mid(&found[1])).abs() > 0.2,
            "the two lines are the same edge: {:?} and {:?}",
            found[0],
            found[1]
        );
    }

    /// ⚠⚠ **The separation rule, and this is the ONLY test that can see it.** A "top two by vote"
    /// implementation passes every test that merely counts results. The fixture is built so the
    /// wrong answer is *attractive*: two very strong near-coincident edges on the left, and one
    /// weaker edge far to the right. The right answer takes one of the pair plus the distant one.
    ///
    /// ⚠ The `spot_source` lesson: "each placed its fixture where a wrong implementation had
    /// nothing to reach for." This one does the opposite.
    #[test]
    fn the_two_lines_returned_are_separated_rather_than_the_two_strongest() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // A pair, six pixels apart — both very strong.
        line(&mut p, w, h, 40.0, 250.0, 60.0, 10.0);
        line(&mut p, w, h, 46.0, 250.0, 66.0, 10.0);
        // One distant edge, deliberately weaker (painted at 0.6 rather than 1.0).
        let mut q = dark(w, h);
        line(&mut q, w, h, 210.0, 250.0, 190.0, 10.0);
        for (a, b) in p.iter_mut().zip(&q) {
            *a = a.max(0.05 + (b - 0.05) * 0.6);
        }
        let found = verticals(&p, w, h);
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
        let mid = |g: &Guide| (g.from[0] + g.to[0]) / 2.0;
        assert!(
            (mid(&found[0]) - mid(&found[1])).abs() > 0.3,
            "the two returned lines are the near-coincident pair: {:?} and {:?}",
            found[0],
            found[1]
        );
    }

    /// The window's far side. ⚠ Both halves, because a window that accepted everything and one
    /// that accepted nothing would each pass a one-sided test.
    ///
    /// ⚠⚠ **The out-of-window fixture had to be re-authored, and the plan's version tested the
    /// opposite of what it claimed.** It ran a 50° line from (20, 250) to (306, 10) — which
    /// leaves the frame at x = 255, where `line`'s clamp pins it and paints a **43 px vertical
    /// segment** up the right edge. The detector then correctly found a vertical that the fixture
    /// had accidentally drawn, and the assertion read as a detector bug. Both endpoints are
    /// inside the frame now, so the only thing in the plane is the 50° line.
    #[test]
    fn a_line_beyond_the_window_is_not_found_and_one_inside_it_is() {
        let (w, h) = (256, 256);
        // 25° from vertical: inside.
        let mut inside = dark(w, h);
        line(&mut inside, w, h, 70.0, 250.0, 182.0, 10.0);
        assert!(
            !verticals(&inside, w, h).is_empty(),
            "a 25-degree line inside the window was missed"
        );
        // ~50° from vertical: outside. Endpoints inside the frame, so nothing is clamped.
        let mut outside = dark(w, h);
        line(&mut outside, w, h, 20.0, 200.0, 200.0, 48.0);
        let found = verticals(&outside, w, h);
        assert!(found.is_empty(), "a 50-degree line was accepted: {found:?}");
    }

    /// A photograph of a wall must produce nothing, not two lines through noise.
    /// Two vertical edges of the **same linear contrast**, one in deep shadow and one near white.
    ///
    /// The shadow pair steps 0.02 → 0.06 linear, the highlight pair 0.86 → 0.90: both a linear
    /// delta of exactly 0.04, giving 30.6 and 4.4 8-bit levels once encoded.
    fn same_linear_delta(w: usize, h: usize, low: f32) -> Vec<f32> {
        let mut p = vec![low; w * h];
        for y in 0..h {
            for x in w / 2..w {
                p[y * w + x] = low + 0.04;
            }
        }
        p
    }

    /// ⚠⚠ **The encode is what makes an absolute floor mean anything, and this is the only test
    /// that can see it deleted.** Under a *linear* Sobel these two frames are identical — the same
    /// 0.04 delta — so any threshold treats them alike. Display-encoded, the sRGB curve expands the
    /// shadows: **0.02 → 0.06 spans 30.6 8-bit levels while 0.86 → 0.90 spans 4.4** — a factor of
    /// seven. The shadow edge is strongly visible and the highlight edge is nearly
    /// invisible, which is exactly the judgement the detector must make and cannot make on linear
    /// values.
    ///
    /// ⚠ Those two numbers were computed, not estimated; recompute them if the fixture's values
    /// change, because a wrong number in this comment is what would make a later reader trust the
    /// wrong threshold.
    ///
    /// ⚠ This project has the same bug on record in the Whites slider: perceptual thirds read off
    /// linear luminance meant "the Whites slider did nothing on most photographs".
    #[test]
    fn a_shadow_edge_and_a_highlight_edge_of_equal_linear_contrast_are_not_equal() {
        let (w, h) = (128, 128);
        // ⚠⚠ An **explicit** floor, not [`MIN_EDGE_LEVELS`]. This test is about the *encode*, so
        // tuning the shipped constant must not be able to perturb it — which it silently did once,
        // when the constant moved past the shadow edge's own contrast and turned a test of the
        // colour space into an upper bound on a tuning value.
        const FLOOR: f32 = 15.0;
        let shadow = verticals_with_floor(&same_linear_delta(w, h, 0.02), w, h, FLOOR);
        let highlight = verticals_with_floor(&same_linear_delta(w, h, 0.86), w, h, FLOOR);
        assert!(
            !shadow.is_empty(),
            "the shadow edge — about 31 levels of contrast — was not detected"
        );
        assert!(
            highlight.is_empty(),
            "the highlight edge — under 5 levels — was detected: {highlight:?}, so the plane is \
             being measured linearly and the floor means nothing"
        );
    }

    /// A single vertical step of a known contrast, in 8-bit levels, on a mid-grey field.
    ///
    /// Built in **display** units and converted back to linear, because that is the space the
    /// constant is expressed in and [`verticals`] re-encodes internally.
    fn step_of_levels(w: usize, h: usize, levels: f32) -> Vec<f32> {
        let base = 0.5f32;
        let hi = base + levels / 255.0;
        let mut p = vec![color::srgb_to_linear(base); w * h];
        for y in 0..h {
            for x in w / 2..w {
                p[y * w + x] = color::srgb_to_linear(hi);
            }
        }
        p
    }

    /// ⚠⚠ **The constant means what it says.** Without dividing the Sobel response by its step
    /// gain the threshold is four times what the name claims, and **no "does it detect?" test can
    /// see a silent factor of four** — it would simply behave as though the constant were 4×
    /// larger, which reads as a tuning choice rather than a bug.
    ///
    /// ⚠ Asserts **both halves**: a floor that admits everything and one that admits nothing each
    /// pass a one-sided test.
    #[test]
    fn the_floor_is_expressed_in_levels_of_edge_contrast() {
        let (w, h) = (128, 128);
        let below = MIN_EDGE_LEVELS - 2.0;
        let above = MIN_EDGE_LEVELS + 2.0;
        assert!(
            verticals(&step_of_levels(w, h, below), w, h).is_empty(),
            "an edge of {below} levels cleared a floor of {MIN_EDGE_LEVELS}"
        );
        assert!(
            !verticals(&step_of_levels(w, h, above), w, h).is_empty(),
            "an edge of {above} levels did not clear a floor of {MIN_EDGE_LEVELS}"
        );
    }

    /// ⚠⚠ **Every other fixture in this file is SQUARE, and the real analysis plane is 512×342.**
    /// This project has recorded an aspect-ratio bug four times — `LinearGradient::coverage`,
    /// `mask_tool::hit_test`, the brush radius and `lean_deg` — and a square fixture cannot see
    /// one, because at `w == h` an x-normalisation and a y-normalisation are the same number.
    ///
    /// A vertical line painted at a known column must come back at that column.
    #[test]
    fn a_vertical_line_on_a_wide_frame_comes_back_where_it_was_painted() {
        let (w, h) = (512, 342);
        for &at in &[0.25f32, 0.5, 0.75] {
            let x = at * (w - 1) as f32;
            let mut p = dark(w, h);
            line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
            let found = verticals(&p, w, h);
            assert!(!found.is_empty(), "nothing found for a line at {at}");
            let g = &found[0];
            assert!(
                (g.from[0] - at).abs() < 0.02 && (g.to[0] - at).abs() < 0.02,
                "a vertical line painted at x={at} came back at ({:.4} -> {:.4})",
                g.from[0],
                g.to[0]
            );
        }
    }

    /// ⚠⚠ **The test that names the contiguity rule, and the two fixtures carry the SAME total
    /// number of voting pixels — only their distribution differs.** That is what makes it a test of
    /// contiguity rather than of length: a rule counting votes anywhere along the line cannot tell
    /// these two apart, and that is exactly the bug it exists for.
    ///
    /// Reported in use as the guides being "put at random, not fixed to the building". Measured on
    /// the author's own frames, the winning lines had 24–41% of rows voting but a longest
    /// *contiguous* run of 7–13% — scattered collinear noise, not an edge.
    #[test]
    fn scattered_collinear_fragments_are_not_a_line_but_one_run_of_them_is() {
        let (w, h) = (512, 342);
        let x = 0.5 * (w - 1) as f32;
        // Half the frame height of voting pixels, in eight short fragments spread over all of it.
        let mut scattered = dark(w, h);
        let seg = h / 16;
        for k in 0..8 {
            let y0 = (k * 2 * seg + 2) as f32;
            line(&mut scattered, w, h, x, y0, x, y0 + seg as f32);
        }
        // The same total, in one unbroken run.
        let mut solid = dark(w, h);
        line(&mut solid, w, h, x, 2.0, x, 2.0 + (8 * seg) as f32);

        assert!(
            verticals(&scattered, w, h).is_empty(),
            "eight scattered fragments were read as a building edge: {:?}",
            verticals(&scattered, w, h)
        );
        assert!(
            !verticals(&solid, w, h).is_empty(),
            "one continuous edge of the same total length was not detected"
        );
    }

    /// ⚠⚠ **The contiguity walk repeats the direction gate, and this is the only fixture that can
    /// see it dropped.** Found by sabotage: removing the gate from `longest_run` left every other
    /// test green, because in every one of them a candidate's gaps contain nothing at all.
    ///
    /// Here the gaps between scattered vertical fragments are filled by strong **horizontal**
    /// bands. Gated, they are not evidence for a near-vertical line, the run stays as short as the
    /// fragments, and the candidate is rejected. Ungated, they bridge every gap and eight unrelated
    /// stubs are promoted into one full-height "building edge" — which is the original bug wearing
    /// a different hat.
    #[test]
    fn horizontal_bands_crossing_a_candidate_are_not_support_for_it() {
        let (w, h) = (512, 342);
        let x = 0.5 * (w - 1) as f32;
        let seg = h / 16;
        let mut p = dark(w, h);
        for k in 0..8 {
            let y0 = (k * 2 * seg + 2) as f32;
            line(&mut p, w, h, x, y0, x, y0 + seg as f32);
        }
        // ⚠⚠ The gaps are filled with a repeating vertical RAMP, not with bands. A symmetric band
        // has zero vertical derivative at its own centre, so it bridges only its flanks — a first
        // attempt using σ = 1.2 bands covered about five rows of a twenty-one row gap and the
        // sabotage went uncaught. A sawtooth gives **every** row a strong purely-vertical gradient,
        // which is exactly the thing the direction gate must refuse to count.
        const PERIOD: usize = 8;
        for k in 0..8 {
            let gap = (k * 2 * seg + 2 + seg)..((k + 1) * 2 * seg + 2).min(h);
            for y in gap {
                let v = 0.05 + 0.9 * (y % PERIOD) as f32 / (PERIOD - 1) as f32;
                for xi in 0..w {
                    let cell = &mut p[y * w + xi];
                    *cell = cell.max(v);
                }
            }
        }
        let found = verticals(&p, w, h);
        assert!(
            found.is_empty(),
            "horizontal bands bridged the gaps and promoted scattered stubs to a line: {found:?}"
        );
    }

    /// A guide drawn as a straight vertical at frame fraction `at`, spanning the full height.
    fn drawn_at(at: f32) -> Guide {
        Guide {
            from: [at, 0.0],
            to: [at, 1.0],
        }
    }

    /// ⚠ A guide drawn slightly off a real edge lands on it — the feature working at all.
    #[test]
    fn a_guide_drawn_near_an_edge_snaps_onto_it() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.5 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        // Drawn 1.5% of the frame width off the real edge, well inside the radius.
        let out = refine(&p, w, h, &[drawn_at(0.515)]);
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].from[0] - 0.5).abs() < 0.01,
            "the guide did not snap onto the edge: {:?}",
            out[0]
        );
    }

    /// ⚠⚠ **A guide with no candidate in range comes back BIT-IDENTICAL.** `assert_eq!` on the
    /// guide, not a tolerance: "unchanged" is exact, and a tolerance would hide a small unwanted
    /// move. Without this rule the silhouette beside Walhalla's building corner captures a line
    /// aimed at the corner, and refinement re-creates the bug it exists to fix.
    #[test]
    fn a_guide_far_from_every_edge_is_returned_unchanged() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.2 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        let drawn = drawn_at(0.8);
        let out = refine(&p, w, h, &[drawn]);
        assert_eq!(out, vec![drawn], "a guide was dragged to a distant edge");
    }

    /// ⚠⚠ **The one test that can distinguish "strongest in range" from "nearest in range."**
    /// A weak edge sits 1% away and a strong one 2.5% away, both inside the radius. Every other
    /// test here passes against either rule.
    #[test]
    fn inside_the_radius_the_strongest_candidate_wins_not_the_nearest() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let weak = 0.51 * (w - 1) as f32;
        let strong = 0.475 * (w - 1) as f32;
        // The weak edge is short, so it clears contiguity but collects far fewer votes.
        line(&mut p, w, h, weak, 2.0, weak, 0.30 * h as f32);
        line(&mut p, w, h, strong, 2.0, strong, h as f32 - 3.0);
        let out = refine(&p, w, h, &[drawn_at(0.5)]);
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].from[0] - 0.475).abs() < 0.01,
            "the nearer weak edge won over the stronger one: {:?}",
            out[0]
        );
    }

    /// ⚠ A horizontal-family guide has nothing to snap to — the detector finds verticals only —
    /// so it is returned untouched even on a frame full of strong verticals. Silently "improving"
    /// it would be inventing a result.
    ///
    /// ⚠⚠ **The guide is a SHORT, SHALLOW one, and that is what makes this test able to see the
    /// family check at all.** A full-width horizontal guide sweeps x across the whole frame, so its
    /// gap from any near-vertical candidate is enormous and the radius rejects it whether or not
    /// the family check exists — the first fixture here was exactly that, and its sabotage went
    /// uncaught. This one spans 2% of the width against 1.8% of the height: horizontal family by a
    /// hair, and geometrically well inside the snap radius of the edge at 0.5.
    ///
    /// ⚠ So for a *typical* wide horizontal guide the family check is defence in depth, and it is
    /// load-bearing only for a stub like this. Both facts are worth knowing before anyone removes
    /// it.
    #[test]
    fn a_horizontal_guide_is_never_refined() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.5 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        let drawn = Guide {
            from: [0.50, 0.400],
            to: [0.52, 0.418],
        };
        assert!(
            (drawn.to[1] - drawn.from[1]).abs() < (drawn.to[0] - drawn.from[0]).abs(),
            "fixture check: this guide must be horizontal-family"
        );
        let out = refine(&p, w, h, &[drawn]);
        assert_eq!(out, vec![drawn], "a horizontal guide was refined");
    }

    #[test]
    fn a_flat_frame_finds_nothing() {
        assert!(verticals(&dark(128, 128), 128, 128).is_empty());
    }

    /// ⚠ One line is not two. `guides::solve` needs exactly two per family — "one line has no
    /// vanishing point at all" — so returning a single guide is a legitimate answer the caller
    /// must be able to see, and inventing a second one would be worse than returning none.
    #[test]
    fn a_single_edge_returns_at_most_one_line() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        line(&mut p, w, h, 128.0, 250.0, 140.0, 10.0);
        assert!(verticals(&p, w, h).len() <= 1);
    }

    /// ⚠⚠ **The direction gate is load-bearing.** A frame of strong HORIZONTAL edges must cast no
    /// votes at all. Without this, the angular window is being enforced by the accumulator's
    /// geometry alone and the gradient direction — the whole reason for computing a Sobel rather
    /// than a magnitude-only high-pass — is untested.
    #[test]
    fn strong_horizontal_edges_cast_no_votes() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        for y in [40usize, 90, 140, 190] {
            for x in 0..w {
                for dy in 0..2 {
                    p[(y + dy) * w + x] = 1.0;
                }
            }
        }
        let found = verticals(&p, w, h);
        assert!(
            found.is_empty(),
            "horizontal edges were detected: {found:?}"
        );
    }

    /// ⚠⚠ **The one test that can actually see the direction gate, and the sabotage step is what
    /// forced it.** Deleting the gate left `strong_horizontal_edges_cast_no_votes` green: a few
    /// horizontal bands cross any given near-vertical line only once or twice, so the votes never
    /// reach [`MIN_LENGTH_FRACTION`] and the *length* threshold quietly enforced what the gate
    /// was supposed to. A test that names a guard another guard is doing the work of is the
    /// standing failure this project records.
    ///
    /// A smooth vertical ramp — a gradient sky — is the fixture that discriminates, because
    /// **every** pixel carries a strong purely-vertical gradient. Ungated, every pixel votes,
    /// every candidate line collects a full frame-height of votes, and the detector confidently
    /// returns two edges of a building that is not there. Gated, nothing votes at all.
    /// ⚠⚠ **The fixture's amplitude had to be raised when the contrast floor landed, and NOT
    /// raising it would have silently disarmed this test.** It was a single smooth ramp spanning
    /// 0.1 → 0.9 over 256 rows, i.e. **0.64 8-bit levels per row** once display-encoded — far below
    /// [`MIN_EDGE_LEVELS`]. So the floor rejected every pixel before the direction gate ever saw
    /// one, and deleting the gate outright left the whole suite green. Verified by sabotage at the
    /// time, which is the only reason it was caught.
    ///
    /// ⚠ It is a **repeating** ramp now — period 8 rows, 0.05 → 0.95 linear, about **25 levels per
    /// row** — so every pixel still carries a purely vertical gradient and now every pixel is also
    /// allowed to vote. The property under test is unchanged; only the contrast is, and it is
    /// changed because the model moved, never to make a red test green.
    #[test]
    fn a_gradient_sky_casts_no_votes() {
        let (w, h) = (256, 256);
        const PERIOD: usize = 8;
        let mut p = vec![0.0f32; w * h];
        for y in 0..h {
            let v = 0.05 + 0.9 * (y % PERIOD) as f32 / (PERIOD - 1) as f32;
            for x in 0..w {
                p[y * w + x] = v;
            }
        }
        let found = verticals(&p, w, h);
        assert!(
            found.is_empty(),
            "a gradient sky was read as a building: {found:?}"
        );
    }

    /// A single steeply leaning edge is recovered at **its own** angle, not flattened toward
    /// vertical. A steep line is what discriminates: at a shallow angle a detector that always
    /// answered "vertical" would sit inside any sane tolerance.
    ///
    /// ⚠ This test was written to guard a half-plane fold of the gradient, and its sabotage
    /// found the fold **inert** — with a symmetric gate the folded and unfolded expressions are
    /// identically equal, so the fold was removed rather than kept with a test that could not see
    /// it. The test earns its place on the weaker property it really does pin, which sabotages 2
    /// and 5 both fail against.
    #[test]
    fn one_leaning_edge_is_recovered_at_its_own_angle_not_at_zero() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // atan(110/220) = 26.6° from vertical, inside the window and far from zero.
        line(&mut p, w, h, 60.0, 240.0, 170.0, 20.0);
        let found = verticals(&p, w, h);
        assert_eq!(found.len(), 1, "expected one line, got {}", found.len());
        let l = lean(&found[0]);
        assert!(
            (l - 26.6).abs() < 4.0,
            "expected ~26.6° of lean, got {l} for {:?}",
            found[0]
        );
    }

    /// ⚠⚠ **A photograph with no building edge in it must come back empty, and only an ABSOLUTE
    /// threshold can say so.** This is the production case the milestone's GUI checklist names —
    /// Auto on a landscape must report "need two" — and it is the test that
    /// [`MIN_LENGTH_FRACTION`] exists for. The fixture is full of vertical detail that is
    /// perfectly real and far too *short* to be a building edge: every relative threshold in
    /// this module is cleared by it, because it is the strongest thing in its own frame.
    ///
    /// ⚠ The stubs sit **side by side in one band**, deliberately. An earlier version of this
    /// fixture stacked four of them per column at a shared `x` — which is a *dashed vertical
    /// line*, spanning 71% of the frame, and a Hough pass is entirely right to find it. Collinear
    /// short segments are a line; only segments that share no line are isolated detail.
    #[test]
    fn a_frame_of_short_vertical_detail_is_not_a_building() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        // Six stubs, each a fourteenth of the frame height, in one horizontal band.
        for (k, x) in [30.0f32, 70.0, 110.0, 150.0, 190.0, 230.0]
            .iter()
            .enumerate()
        {
            let skew = (k as f32 - 2.5) * 1.5;
            line(&mut p, w, h, *x, 100.0, x + skew, 118.0);
        }
        let found = verticals(&p, w, h);
        assert!(
            found.is_empty(),
            "short vertical detail was read as a building: {found:?}"
        );
    }

    /// ⚠ The discriminating other half: the same fixture with the stubs joined into two full-height
    /// edges **is** detected. Without this, a threshold that rejected everything would pass the
    /// test above, which is the one-sided-window trap in a new costume.
    #[test]
    fn the_same_detail_run_the_full_height_is_a_building() {
        let (w, h) = (256, 256);
        let mut p = dark(w, h);
        line(&mut p, w, h, 40.0, 240.0, 58.0, 16.0);
        line(&mut p, w, h, 205.0, 240.0, 187.0, 16.0);
        assert_eq!(verticals(&p, w, h).len(), 2);
    }

    #[test]
    fn detect_handles_the_degenerate_sizes() {
        assert!(verticals(&[], 0, 0).is_empty());
        assert!(verticals(&[0.5], 1, 1).is_empty());
    }
}

//! Choosing a plausible source for a retouch spot.
//!
//! ⚠⚠ **No `ImageBuf`: the search runs over an injected sampler.** `FrameMap::inverse_point`
//! returns **post-lens, post-perspective** coordinates while `Session.buffer` is the **raw
//! decode**, so indexing one with the other is wrong by the distortion — recorded at ~3.5% at the
//! corners, which on a 6246 px frame is ~220 px and therefore *larger than a typical spot radius*.
//! Near the edges such a search would sample a different piece of the picture, score against the
//! wrong target, and return a confidently wrong source. Silently.
//!
//! ⚠ Taking a closure instead leaves that composition to the caller, keeps this a pure function
//! with no dependency on `lens_profile` or `perspective`, and lets the tests pass a synthetic
//! field with no image at all. The `DecodeWorker` injected-decode pattern, for its reason.

use edit_model::Spot;

/// Candidate directions and distances: 16 × 5 = 80 probes, a few milliseconds and only on a click.
const ANGLES: usize = 16;
const DISTANCES: [f32; 5] = [2.0, 3.0, 4.0, 5.0, 6.0];

/// Where the destination's signature ring is sampled, as multiples of the radius.
///
/// ⚠⚠ **Outset from the rim, not flush with it, and this is A's Wave 3 finding arriving in the
/// search.** That finding — *the retouch radius must exceed the blemish, or heal measures a ring
/// that is still lying on the spot* — applies here identically: a flush ring on a too-small radius
/// **is** the blemish, so the signature becomes the blemish's own colour and the search
/// confidently returns a source that looks like the thing being removed.
const RING: [f32; 3] = [1.2, 1.4, 1.6];

/// Points sampled around each ring.
const RING_SAMPLES: usize = 24;

/// ⚠ A disc is sampled on a coarse lattice rather than per pixel: a mean and a variance need
/// nothing finer, and it is what keeps a large-radius search in single-digit milliseconds.
const DISC_STEPS: i32 = 3;

/// Per-axis scaling that turns a long-edge offset into normalised per-axis offsets.
fn axes(aspect: f32) -> (f32, f32) {
    if aspect >= 1.0 {
        (1.0, aspect)
    } else {
        (1.0 / aspect, 1.0)
    }
}

/// Distance between two normalised points in long-edge units.
///
/// ⚠ Deliberately a private copy rather than a dependency on `spot_tool`: neither module should
/// need the other, and this is four lines.
fn long_edge_gap(a: [f32; 2], b: [f32; 2], aspect: f32) -> f32 {
    let (sx, sy) = if aspect >= 1.0 {
        (1.0, 1.0 / aspect)
    } else {
        (aspect, 1.0)
    };
    (((b[0] - a[0]) * sx).powi(2) + ((b[1] - a[1]) * sy).powi(2)).sqrt()
}

fn mean(samples: &[[f32; 3]]) -> [f32; 3] {
    let n = samples.len().max(1) as f32;
    let mut m = [0.0; 3];
    for s in samples {
        for c in 0..3 {
            m[c] += s[c];
        }
    }
    m.map(|v| v / n)
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum()
}

fn variance(samples: &[[f32; 3]], m: [f32; 3]) -> f32 {
    let n = samples.len().max(1) as f32;
    samples.iter().map(|s| distance2(*s, m)).sum::<f32>() / n
}

fn sample_ring(
    sample: &impl Fn([f32; 2]) -> [f32; 3],
    c: [f32; 2],
    r: f32,
    aspect: f32,
) -> Vec<[f32; 3]> {
    let (ax, ay) = axes(aspect);
    let mut out = Vec::with_capacity(RING.len() * RING_SAMPLES);
    for m in RING {
        for k in 0..RING_SAMPLES {
            let t = k as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
            out.push(sample([
                (c[0] + m * r * t.cos() * ax).clamp(0.0, 1.0),
                (c[1] + m * r * t.sin() * ay).clamp(0.0, 1.0),
            ]));
        }
    }
    out
}

fn sample_disc(
    sample: &impl Fn([f32; 2]) -> [f32; 3],
    c: [f32; 2],
    r: f32,
    aspect: f32,
) -> Vec<[f32; 3]> {
    let (ax, ay) = axes(aspect);
    let mut out = Vec::new();
    for iy in -DISC_STEPS..=DISC_STEPS {
        for ix in -DISC_STEPS..=DISC_STEPS {
            let (fx, fy) = (ix as f32 / DISC_STEPS as f32, iy as f32 / DISC_STEPS as f32);
            if fx * fx + fy * fy > 1.0 {
                continue;
            }
            out.push(sample([
                (c[0] + fx * r * ax).clamp(0.0, 1.0),
                (c[1] + fy * r * ay).clamp(0.0, 1.0),
            ]));
        }
    }
    out
}

/// A plausible source for a spot at `dest` of `radius` (long-edge units).
///
/// `sample` returns the colour at a normalised point **in the spot-removal stage's input space**;
/// mapping that back through the lens and perspective inverses is the caller's job. `None` when
/// nothing qualifies — the caller then places a fixed offset rather than refusing the click.
///
/// ⚠ Scored on the colour distance from the destination's ring **plus** the variance inside the
/// candidate: a flat patch copies more cleanly than an edge.
pub fn pick(
    sample: impl Fn([f32; 2]) -> [f32; 3],
    dest: [f32; 2],
    radius: f32,
    aspect: f32,
    existing: &[Spot],
) -> Option<[f32; 2]> {
    if !(radius.is_finite() && radius > 0.0) || !aspect.is_finite() || aspect <= 0.0 {
        return None;
    }
    let (ax, ay) = axes(aspect);
    let target = mean(&sample_ring(&sample, dest, radius, aspect));

    let mut best: Option<([f32; 2], f32)> = None;
    for k in 0..ANGLES {
        let t = k as f32 / ANGLES as f32 * std::f32::consts::TAU;
        for m in DISTANCES {
            let c = [
                dest[0] + m * radius * t.cos() * ax,
                dest[1] + m * radius * t.sin() * ay,
            ];
            // Wholly inside the frame, each axis measured in its own normalised units.
            if c[0] - radius * ax < 0.0
                || c[0] + radius * ax > 1.0
                || c[1] - radius * ay < 0.0
                || c[1] + radius * ay > 1.0
            {
                continue;
            }
            // ⚠ Never overlapping the destination: a source disc that overlaps its own
            // destination copies part of the blemish back onto itself.
            if long_edge_gap(c, dest, aspect) < 2.0 * radius {
                continue;
            }
            // ⚠ Nor an existing spot's destination, which is a blemish somebody is removing.
            if existing
                .iter()
                .any(|s| long_edge_gap(c, s.centre, aspect) < radius + s.radius)
            {
                continue;
            }
            let disc = sample_disc(&sample, c, radius, aspect);
            let m_disc = mean(&disc);
            let score = distance2(m_disc, target) + variance(&disc, m_disc);
            if best.is_none_or(|(_, b)| score < b) {
                best = Some((c, score));
            }
        }
    }
    best.map(|(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::SpotMode;

    /// A frame that is flat grey on the left half and flat red on the right half.
    fn split(p: [f32; 2]) -> [f32; 3] {
        if p[0] < 0.5 {
            [0.5, 0.5, 0.5]
        } else {
            [0.8, 0.1, 0.1]
        }
    }

    /// ⚠⚠ **The destination sits NEAR the edge, and that is what makes this test able to fail.**
    /// An earlier version used 0.25 and 0.75, where every candidate within `6r` lies on the
    /// destination's own side — so no candidate was ever across the edge and the assertion could
    /// not be violated by any implementation. Found by sabotage, not by review: the standing
    /// lesson that a test can name a property it cannot see.
    ///
    /// At 0.56 the candidate band spans 0.38–0.74, straddling the edge, while the signature ring
    /// at 1.2r–1.6r stays at 0.512–0.608 and is therefore wholly on the red side.
    #[test]
    fn the_pick_stays_on_the_matching_side_of_an_edge() {
        let r = 0.03;
        let got = pick(split, [0.56, 0.5], r, 1.0, &[]).expect("a source");
        assert!(got[0] > 0.5, "picked grey for a red destination: {got:?}");
        let got = pick(split, [0.44, 0.5], r, 1.0, &[]).expect("a source");
        assert!(got[0] < 0.5, "picked red for a grey destination: {got:?}");
    }

    /// A gently textured light field, a **flat dark** region beyond `x = 0.58`, and a dark blemish
    /// disc of radius `r` at `dest`.
    ///
    /// ⚠⚠ **The texture is load-bearing.** With a uniform light field every candidate has zero
    /// variance and identical colour, so neither the ring's colour nor the scoring's target term
    /// can change the answer — which is exactly why the first version of these tests could not
    /// fail. Texturing the light side means a scorer that used only flatness would prefer the flat
    /// dark region, making the target term observable.
    fn blemish_field(dest: [f32; 2], r: f32) -> impl Fn([f32; 2]) -> [f32; 3] {
        move |p: [f32; 2]| {
            let d = ((p[0] - dest[0]).powi(2) + (p[1] - dest[1]).powi(2)).sqrt();
            if d < r || p[0] > 0.58 {
                return [0.05, 0.05, 0.05];
            }
            let t = 0.6 + 0.03 * ((p[0] * 97.0).sin() * (p[1] * 89.0).sin());
            [t, t, t]
        }
    }

    /// ⚠⚠ **The score must follow the ring's COLOUR, not only the candidate's flatness.** Dropping
    /// the target term leaves a scorer that always prefers the flattest patch — here the dark
    /// region — however wrong its colour is. A uniform fixture cannot see that, because a flat
    /// light patch and a flat dark patch both score zero.
    #[test]
    fn the_score_follows_the_rings_colour_and_not_only_flatness() {
        let dest = [0.5, 0.5];
        let r = 0.02;
        let field = blemish_field(dest, r);
        let got = pick(&field, dest, r, 1.0, &[]).expect("a source");
        assert!(
            field(got)[0] > 0.4,
            "picked the flat dark region over a matching light one: {got:?}"
        );
    }

    /// ⚠ A source disc that overlaps its own destination copies part of the blemish back onto
    /// itself.
    #[test]
    fn the_pick_never_overlaps_the_destination() {
        let dest = [0.5, 0.5];
        let r = 0.04;
        let got = pick(|_| [0.5, 0.5, 0.5], dest, r, 1.0, &[]).expect("a source");
        let d = ((got[0] - dest[0]).powi(2) + (got[1] - dest[1]).powi(2)).sqrt();
        assert!(
            d >= 2.0 * r,
            "source overlaps the destination: d={d}, r={r}"
        );
    }

    /// ⚠ And never on an existing spot's destination, which is a blemish somebody is removing.
    ///
    /// ⚠⚠ **The existing spot sits exactly where the search looks first**, and that is what makes
    /// this test able to fail. On a uniform field every candidate scores identically, so the first
    /// one tried wins — angle 0, distance 2r, i.e. `[0.58, 0.5]`. An earlier version put the
    /// existing spot at `[0.5, 0.36]`, somewhere the search would never have chosen anyway, so
    /// deleting the rejection changed nothing and the test passed. Found by sabotage.
    #[test]
    fn the_pick_avoids_an_existing_spots_destination() {
        let (dest, r) = ([0.5f32, 0.5], 0.04f32);
        let occupied = [dest[0] + 2.0 * r, dest[1]];
        let existing = [Spot {
            centre: occupied,
            source: [0.1, 0.1],
            radius: r,
            feather: 50.0,
            mode: SpotMode::Heal,
        }];
        let got = pick(|_| [0.5, 0.5, 0.5], dest, r, 1.0, &existing).expect("a source");
        let d = ((got[0] - occupied[0]).powi(2) + (got[1] - occupied[1]).powi(2)).sqrt();
        assert!(
            d >= 2.0 * r,
            "the source disc overlaps an existing blemish: d={d}, got={got:?}"
        );
    }

    /// ⚠⚠ **A's Wave 3 finding, one layer up.** The ring must be sampled OUTSIDE the blemish. A
    /// ring flush with the rim *is* the blemish, so the signature becomes the blemish's own colour
    /// and the search hunts for a source that looks like the thing being removed.
    ///
    /// ⚠ The fixture needs a dark region **elsewhere** for a wrongly-dark signature to be attracted
    /// to. On a uniformly light field a wrong target changes nothing, because every candidate is
    /// equally light — which is why the first version of this test passed with the ring flush.
    #[test]
    fn the_ring_is_sampled_outside_the_blemish() {
        let dest = [0.5, 0.5];
        let r = 0.02;
        let field = blemish_field(dest, r);
        // The blemish really is dark, and the light field really is light — a fixture check, so a
        // broken fixture cannot make this pass vacuously.
        assert!(field(dest)[0] < 0.1, "fixture check: the blemish is dark");
        assert!(
            field([0.5, 0.4])[0] > 0.4,
            "fixture check: the field is light"
        );

        let got = pick(&field, dest, r, 1.0, &[]).expect("a source");
        assert!(
            field(got)[0] > 0.4,
            "picked a source matching the blemish: {got:?}"
        );
    }

    /// ⚠ A degenerate radius must not divide by zero or spin — it returns `None` and the caller
    /// falls back.
    #[test]
    fn a_degenerate_radius_yields_none() {
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], 0.0, 1.0, &[]), None);
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], -1.0, 1.0, &[]), None);
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], f32::NAN, 1.0, &[]), None);
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], 0.02, 0.0, &[]), None);
    }

    /// ⚠ A destination in the corner has few legal candidates; the search must still find one
    /// rather than returning `None` and making the tool feel broken at the edges.
    #[test]
    fn a_corner_destination_still_finds_a_source() {
        assert!(pick(|_| [0.5; 3], [0.06, 0.06], 0.02, 1.0, &[]).is_some());
    }
}

//! The defringe controls' rules (M9.6): the eyedropper's pick and the hue-range widget's drags.
//!
//! ⚠ Lives in the lib with no egui, the `wb_tool` / `tone_curve_tool` pattern: the panel and the
//! canvas supply the gesture; everything here is a pure function.
//!
//! Hues are **Oklab hue in degrees**, the space `pipeline::stages::defringe` selects in. The
//! widget's domain is `[0, 360]`; a range never wraps inside the widget.

use edit_model::Defringe;

/// The narrowest a hue range may be, degrees.
pub const MIN_WIDTH: f32 = 10.0;
/// Half the width of a range the eyedropper sets, degrees.
pub const PICK_HALF: f32 = 20.0;
/// The amount a pick raises a zero amount to, so a click visibly does something.
pub const PICK_AMOUNT: f32 = 40.0;
/// Below this Oklab chroma a sample is grey and says nothing about a fringe's hue.
pub const MIN_CHROMA: f32 = 0.03;

/// Which fringe a hue belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fringe {
    Purple,
    Green,
}

impl Fringe {
    pub fn label(self) -> &'static str {
        match self {
            Fringe::Purple => "purple",
            Fringe::Green => "green",
        }
    }
}

/// The family of Oklab hue `h` (degrees): purple is blue through magenta (220°..20°, across 0°),
/// green is yellow-green through green (90°..190°); reds, oranges, yellows and cyans are neither.
pub fn family(h: f32) -> Option<Fringe> {
    let h = h.rem_euclid(360.0);
    if !(20.0..220.0).contains(&h) {
        Some(Fringe::Purple)
    } else if (90.0..190.0).contains(&h) {
        Some(Fringe::Green)
    } else {
        None
    }
}

/// The hue of the most saturated of `pixels` (linear ProPhoto), if any is saturated enough to have
/// one. ⚠ The most saturated, not the mean: a 5×5 patch clicked on a 3 px fringe also holds the
/// edge beside it, and a mean would read a desaturated blend of the two.
pub fn most_chromatic_hue(pixels: impl IntoIterator<Item = [f32; 3]>) -> Option<f32> {
    let (c, h) = pixels
        .into_iter()
        .map(|p| {
            let [_, a, b] = color::oklab::prophoto_to_oklab(p);
            (a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
        })
        .fold(
            (0.0f32, 0.0f32),
            |best, x| if x.0 > best.0 { x } else { best },
        );
    (c >= MIN_CHROMA).then_some(h)
}

/// `[centre − PICK_HALF, centre + PICK_HALF]`, shifted to fit inside `[0, 360]`.
fn centred(centre: f32) -> [f32; 2] {
    let lo = (centre - PICK_HALF).clamp(0.0, 360.0 - 2.0 * PICK_HALF);
    [lo, lo + 2.0 * PICK_HALF]
}

/// The eyedropper: set the range of the fringe `hue` belongs to around it, and raise that fringe's
/// amount from zero. `None` for a hue that is no fringe colour — the defringe is left alone.
pub fn pick(d: Defringe, hue: f32) -> Option<(Defringe, Fringe)> {
    let f = family(hue)?;
    let mut out = d;
    let (range, amount) = match f {
        Fringe::Purple => (&mut out.purple_hue, &mut out.purple_amount),
        Fringe::Green => (&mut out.green_hue, &mut out.green_amount),
    };
    *range = centred(hue.rem_euclid(360.0));
    if *amount <= 0.0 {
        *amount = PICK_AMOUNT;
    }
    Some((out, f))
}

/// Drag one handle (`0` low, `1` high) of a range to `deg`: clamped to `[0, 360]`, never closer
/// than [`MIN_WIDTH`] to the other handle.
pub fn drag_handle(range: [f32; 2], handle: usize, deg: f32) -> [f32; 2] {
    let [lo, hi] = range;
    if handle == 0 {
        [deg.clamp(0.0, hi - MIN_WIDTH), hi]
    } else {
        [lo, deg.clamp(lo + MIN_WIDTH, 360.0)]
    }
}

/// Drag the whole band by `delta` degrees, keeping its width and staying inside `[0, 360]`.
pub fn drag_band(range: [f32; 2], delta: f32) -> [f32; 2] {
    let width = range[1] - range[0];
    let lo = (range[0] + delta).clamp(0.0, 360.0 - width);
    [lo, lo + width]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ProPhoto colour whose Oklab hue is (close to) `deg`, at moderate lightness and chroma.
    fn at_hue(deg: f32) -> [f32; 3] {
        let r = deg.to_radians();
        color::oklab::oklab_to_prophoto([0.6, 0.12 * r.cos(), 0.12 * r.sin()])
    }

    #[test]
    fn families_split_the_circle_and_leave_warm_and_cyan_hues_out() {
        assert_eq!(family(310.0), Some(Fringe::Purple));
        assert_eq!(family(5.0), Some(Fringe::Purple), "across 0°");
        assert_eq!(family(260.0), Some(Fringe::Purple));
        assert_eq!(family(140.0), Some(Fringe::Green));
        assert_eq!(family(50.0), None, "orange");
        assert_eq!(family(205.0), None, "cyan");
    }

    /// ⚠ The most saturated sample wins over a majority of grey ones — and grey alone is no pick.
    #[test]
    fn the_most_chromatic_sample_gives_the_hue() {
        let grey = [0.3f32; 3];
        let mut patch = vec![grey; 24];
        patch.push(at_hue(300.0));
        let h = most_chromatic_hue(patch).expect("a saturated sample");
        assert!((h - 300.0).abs() < 1.0, "{h}");
        assert_eq!(most_chromatic_hue(vec![grey; 25]), None);
    }

    #[test]
    fn a_purple_pick_centres_the_purple_range_and_raises_its_amount_only() {
        let (d, f) = pick(Defringe::default(), 300.0).unwrap();
        assert_eq!(f, Fringe::Purple);
        assert_eq!(d.purple_hue, [280.0, 320.0]);
        assert_eq!(d.purple_amount, PICK_AMOUNT);
        assert_eq!(d.green_amount, 0.0);
        assert_eq!(d.green_hue, Defringe::DEFAULT_GREEN_HUE);
        // An amount already set is kept.
        let set = Defringe {
            purple_amount: 70.0,
            ..Default::default()
        };
        assert_eq!(pick(set, 300.0).unwrap().0.purple_amount, 70.0);
    }

    #[test]
    fn a_green_pick_sets_green_and_a_non_fringe_hue_sets_nothing() {
        let (d, f) = pick(Defringe::default(), 140.0).unwrap();
        assert_eq!(f, Fringe::Green);
        assert_eq!(d.green_hue, [120.0, 160.0]);
        assert_eq!(d.purple_amount, 0.0);
        assert!(pick(Defringe::default(), 50.0).is_none());
    }

    #[test]
    fn a_pick_near_the_ends_stays_inside_the_widget() {
        let (d, _) = pick(Defringe::default(), 355.0).unwrap();
        assert_eq!(d.purple_hue, [320.0, 360.0]);
        let (d, _) = pick(Defringe::default(), 5.0).unwrap();
        assert_eq!(d.purple_hue, [0.0, 40.0]);
    }

    #[test]
    fn handles_never_cross_and_the_band_keeps_its_width() {
        assert_eq!(drag_handle([100.0, 150.0], 0, 145.0), [140.0, 150.0]);
        assert_eq!(drag_handle([100.0, 150.0], 1, 20.0), [100.0, 110.0]);
        assert_eq!(drag_handle([100.0, 150.0], 0, -30.0), [0.0, 150.0]);
        assert_eq!(drag_handle([100.0, 150.0], 1, 400.0), [100.0, 360.0]);
        assert_eq!(drag_band([100.0, 150.0], 30.0), [130.0, 180.0]);
        assert_eq!(drag_band([300.0, 350.0], 30.0), [310.0, 360.0]);
        assert_eq!(drag_band([10.0, 60.0], -30.0), [0.0, 50.0]);
    }
}

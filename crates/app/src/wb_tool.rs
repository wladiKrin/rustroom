//! White-balance presets and the eyedropper solve — the logic behind the Kelvin control.
//!
//! ⚠ Lives in the lib, with no egui in sight, because that is where a unit-testable rule belongs.
//! The panel and the canvas supply the gesture; everything here is a pure function.

use color::wb::Illuminant;
use edit_model::WhiteBalance;

/// The named illuminants the panel offers.
///
/// ⚠ **"Auto" is deliberately absent.** It is a real analysis pass over the pixels, not a preset
/// value, and the design marks it as the one item here that can be cut without harming the rest.
/// Adding it means adding a *measurement*, which belongs beside the eyedropper rather than in a
/// table of constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbPreset {
    AsShot,
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
}

impl WbPreset {
    /// ⚠ The order the panel draws them in, so the buttons cannot re-invent it — the rule
    /// `EDIT_GROUP_NAMES`, `FlagSet::ALL` and `Tool::ALL` already follow.
    pub const ALL: [WbPreset; 7] = [
        WbPreset::AsShot,
        WbPreset::Daylight,
        WbPreset::Cloudy,
        WbPreset::Shade,
        WbPreset::Tungsten,
        WbPreset::Fluorescent,
        WbPreset::Flash,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WbPreset::AsShot => "As Shot",
            WbPreset::Daylight => "Daylight",
            WbPreset::Cloudy => "Cloudy",
            WbPreset::Shade => "Shade",
            WbPreset::Tungsten => "Tungsten",
            WbPreset::Fluorescent => "Fluorescent",
            WbPreset::Flash => "Flash",
        }
    }

    /// The temperature this preset declares, or `None` for As Shot.
    ///
    /// Conventional values, the ones every raw converter uses.
    ///
    /// ⚠⚠ **Every preset must name a DISTINCT temperature, and Flash originally did not.** It
    /// shared Daylight's 5500 K, so clicking either lit up both — reported as "I don't understand
    /// what flash is doing, pressing it also always activates daylight". Two controls that cannot
    /// be told apart are worse than one, because you assume you have misunderstood something.
    /// 5600 K is the correct value anyway: it is the electronic-flash and daylight-film standard,
    /// where 5500 K is Adobe's daylight figure. `every_preset_names_a_distinct_temperature` is
    /// the standing guard.
    pub fn kelvin(self) -> Option<f32> {
        match self {
            WbPreset::AsShot => None,
            WbPreset::Daylight => Some(5500.0),
            WbPreset::Flash => Some(5600.0),
            WbPreset::Cloudy => Some(6500.0),
            WbPreset::Shade => Some(7500.0),
            WbPreset::Tungsten => Some(2850.0),
            WbPreset::Fluorescent => Some(3800.0),
        }
    }
}

/// The temperature the Temp slider shows: the declared one, or the file's own baseline.
///
/// ⚠⚠ **This is the whole reason the slider can be always visible.** It shows what the photo is
/// *effectively* balanced to, so an untouched photo reads its own as-shot value rather than a
/// constant — a tungsten frame opening at 6504 K would make the number meaningless. ⚠ Reading it
/// must not *store* it: `None` survives until the slider is actually moved, or merely opening the
/// panel would freeze a value nobody chose on every photo.
pub fn effective_kelvin(wb: &WhiteBalance, baseline: Illuminant) -> f32 {
    wb.kelvin.unwrap_or(baseline.kelvin)
}

/// Apply a preset to a white balance.
///
/// ⚠⚠ **As Shot stores `None`, never the numeric baseline.** `None` keeps the photo *following*
/// its file, so a re-decode or a changed camera profile carries it along; freezing the number
/// would strand the photo at a value nobody chose — the rule an unlabelled virtual copy already
/// follows. It clears the tint too, because "as shot" means the camera's whole choice and not
/// half of it.
pub fn apply_preset(wb: &mut WhiteBalance, p: WbPreset) {
    wb.kelvin = p.kelvin();
    if p == WbPreset::AsShot {
        wb.tint = 0.0;
    }
}

/// The white balance a picked pixel implies, given what the photo is currently balanced to.
///
/// ⚠⚠ **`rgb` must be the pixel BEFORE white balance is applied**, or the eyedropper chases its
/// own correction: pick a grey, the picture shifts, pick the same grey again and it shifts
/// further. The same trap the luminance range mask keys on `base` to avoid.
///
/// ⚠ The solve says what illuminant *made* that pixel non-neutral, and the answer is expressed
/// against the source's own baseline — so picking a neutral on an already-balanced photo returns
/// the baseline and changes nothing.
pub fn solve_pick(rgb: [f32; 3], baseline: Illuminant) -> WhiteBalance {
    let xyz = color::spaces::apply(&color::spaces::PROPHOTO_TO_XYZ, rgb);
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !sum.is_finite() || sum.abs() < 1e-9 {
        return WhiteBalance::default();
    }
    // ⚠ The picked pixel's chromaticity *is* the illuminant that would make it neutral: it is
    // what the light left behind on a surface we are declaring to be grey.
    let picked = color::wb::solve_xy((xyz[0] / sum, xyz[1] / sum));
    // The buffer already carries `baseline`'s correction, so the light that produced this pixel
    // is the composition of the two — expressed, as everything downstream expects, as an absolute
    // illuminant measured on the locus.
    let declared = color::wb::solve_xy(color::wb::with_tint(
        color::wb::locus_xy(picked.kelvin * baseline.kelvin / color::wb::BASELINE_KELVIN),
        picked.tint + baseline.tint,
    ));
    WhiteBalance {
        kelvin: Some(declared.kelvin),
        tint: declared.tint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ An unset white balance reads its own file's temperature, not a constant — otherwise a
    /// tungsten frame opens showing 6504 K and the number tells you nothing.
    #[test]
    fn an_unset_temperature_reads_the_files_own_baseline() {
        let wb = WhiteBalance::default();
        assert_eq!(effective_kelvin(&wb, Illuminant::new(3100.0, 0.0)), 3100.0);
        assert_eq!(effective_kelvin(&wb, Illuminant::new(9000.0, 0.0)), 9000.0);
    }

    /// ⚠ And a declared one wins over the baseline, or the slider would ignore what you set.
    #[test]
    fn a_declared_temperature_wins_over_the_baseline() {
        let wb = WhiteBalance {
            kelvin: Some(4200.0),
            tint: 0.0,
        };
        assert_eq!(effective_kelvin(&wb, Illuminant::new(3100.0, 0.0)), 4200.0);
    }

    /// ⚠ As Shot must store `None`. Freezing the number is the failure this test exists for, and
    /// it would look completely correct until the photo was re-decoded.
    #[test]
    fn as_shot_stores_none_rather_than_a_number() {
        let mut wb = WhiteBalance {
            kelvin: Some(4200.0),
            tint: 5.0,
        };
        apply_preset(&mut wb, WbPreset::AsShot);
        assert_eq!(wb.kelvin, None);
        assert_eq!(wb.tint, 0.0, "As Shot clears the tint too");
    }

    /// ⚠ Every preset must name a temperature, or one of them silently does nothing — and a
    /// button that does nothing is indistinguishable from one that is already selected.
    #[test]
    fn every_preset_except_as_shot_sets_a_temperature() {
        for p in WbPreset::ALL {
            let mut wb = WhiteBalance::default();
            apply_preset(&mut wb, p);
            match p {
                WbPreset::AsShot => assert_eq!(wb.kelvin, None),
                _ => assert!(wb.kelvin.is_some(), "{p:?} set no temperature"),
            }
        }
    }

    /// ⚠⚠ **Two presets at the same temperature are indistinguishable in the panel**, because the
    /// selection highlight can only compare the value they set. Flash and Daylight both sat at
    /// 5500 K and both lit up together, which is how this was reported. A count of distinct
    /// values is the only assertion that can see it — checking that each preset *has* a
    /// temperature, which the test below does, cannot.
    #[test]
    fn every_preset_names_a_distinct_temperature() {
        let mut seen: Vec<f32> = WbPreset::ALL.iter().filter_map(|p| p.kelvin()).collect();
        let total = seen.len();
        seen.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a preset table"));
        seen.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        assert_eq!(
            seen.len(),
            total,
            "two presets share a temperature and would light up together"
        );
    }

    /// ⚠ A preset leaves the tint alone — it declares a temperature, not a whole white balance.
    /// Clearing it would silently discard a green/magenta correction the photo needed.
    #[test]
    fn a_temperature_preset_leaves_the_tint_alone() {
        let mut wb = WhiteBalance {
            kelvin: None,
            tint: 22.0,
        };
        apply_preset(&mut wb, WbPreset::Tungsten);
        assert_eq!(wb.tint, 22.0);
    }

    /// ⚠⚠ The property that makes the eyedropper usable: clicking a grey card on a photo that is
    /// already correctly balanced must change nothing at all.
    #[test]
    fn picking_an_already_neutral_pixel_returns_the_baseline() {
        for base in [
            Illuminant::default(),
            Illuminant::new(5165.0, 0.0),
            Illuminant::new(3200.0, 12.0),
        ] {
            let got = solve_pick([0.5, 0.5, 0.5], base);
            let k = got
                .kelvin
                .expect("the eyedropper always sets a temperature");
            assert!(
                (k - base.kelvin).abs() < base.kelvin * 0.03,
                "baseline {}K came back as {k}K",
                base.kelvin
            );
            assert!(
                (got.tint - base.tint).abs() < 6.0,
                "baseline tint {} came back as {}",
                base.tint,
                got.tint
            );
        }
    }

    /// ⚠ Direction: picking a **warm** patch means the light was warm, so the declared
    /// temperature must go *down* from the baseline — and the render then cools the picture,
    /// which is what removes the cast the user just clicked on.
    #[test]
    fn picking_a_warm_patch_declares_a_warmer_light() {
        let base = Illuminant::default();
        let warm = solve_pick([0.6, 0.5, 0.4], base).kelvin.unwrap();
        let cool = solve_pick([0.4, 0.5, 0.6], base).kelvin.unwrap();
        assert!(
            warm < base.kelvin,
            "a warm patch should read below the baseline, got {warm}K"
        );
        assert!(
            cool > base.kelvin,
            "a cool patch should read above the baseline, got {cool}K"
        );
    }

    /// A black pixel has no chromaticity at all, and must not produce an absurd temperature.
    #[test]
    fn picking_black_changes_nothing() {
        assert_eq!(
            solve_pick([0.0, 0.0, 0.0], Illuminant::default()),
            WhiteBalance::default()
        );
    }
}

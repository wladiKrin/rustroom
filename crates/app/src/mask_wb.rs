//! A mask's Temp and Tint as the panel shows and writes them (M10.12b): relative shifts, −100..=100,
//! Lightroom's local white balance — with a legacy absolute white balance read as the shift it
//! amounted to (`pipeline::mask_wb_shift`), and cleared by the first edit.
//!
//! ⚠ No egui — `develop/panel/mask_adjust.rs` draws the two sliders through these.

use edit_model::LocalAdjustments;

/// The `(temp, tint)` the sliders show: the relative shift, legacy included.
pub fn shown(a: &LocalAdjustments, baseline: color::wb::Illuminant) -> (f32, f32) {
    pipeline::mask_wb_shift(a, baseline)
}

/// Write the sliders' `(temp, tint)` and clear the legacy absolute value — from here on the mask
/// carries only the relative form, which `shown` then reads back unchanged.
pub fn set(a: &mut LocalAdjustments, temp: f32, tint: f32) {
    a.temp = temp;
    a.tint = tint;
    a.white_balance = edit_model::WhiteBalance::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::WhiteBalance;

    fn baseline() -> color::wb::Illuminant {
        color::wb::Illuminant::new(5500.0, 0.0)
    }

    #[test]
    fn a_legacy_mask_shows_the_shift_it_amounted_to() {
        let a = LocalAdjustments {
            white_balance: WhiteBalance {
                kelvin: Some(6500.0),
                tint: 4.0,
            },
            ..Default::default()
        };
        let (t, n) = shown(&a, baseline());
        let want = (1.0e6 / 5500.0 - 1.0e6 / 6500.0) / pipeline::MIRED_PER_STEP;
        assert!((t - want).abs() < 1e-3, "{t} vs {want}");
        assert_eq!(n, 4.0);
    }

    #[test]
    fn setting_writes_the_relative_form_and_clears_the_legacy_one() {
        let mut a = LocalAdjustments {
            white_balance: WhiteBalance {
                kelvin: Some(6500.0),
                tint: 4.0,
            },
            ..Default::default()
        };
        let (t, n) = shown(&a, baseline());
        set(&mut a, t + 10.0, n);
        assert_eq!(
            a.white_balance,
            WhiteBalance::default(),
            "the legacy value is gone"
        );
        let (t2, n2) = shown(&a, baseline());
        assert!((t2 - (t + 10.0)).abs() < 1e-4 && n2 == n, "{t2} {n2}");
    }

    /// The render follows: a mask set through `set` renders like its legacy form at the same values.
    #[test]
    fn the_converted_mask_renders_like_the_legacy_one() {
        let legacy = LocalAdjustments {
            white_balance: WhiteBalance {
                kelvin: Some(6500.0),
                tint: 4.0,
            },
            ..Default::default()
        };
        let mut converted = legacy.clone();
        let (t, n) = shown(&legacy, baseline());
        set(&mut converted, t, n);
        assert_eq!(
            pipeline::mask_wb_shift(&converted, baseline()),
            pipeline::mask_wb_shift(&legacy, baseline())
        );
    }
}

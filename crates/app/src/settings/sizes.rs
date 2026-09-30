//! Sizes the interface remembers, each clamped into its range: grid cells, the custom crop ratio, the
//! snapshot strip, dialogs.

use super::Settings;

/// The Library grid's default cell size, and the size its type scale is relative to.
///
/// ⚠ 224, up from 168 (2026-09-28): on a wide window the 168-pt cells read as postage stamps in
/// a void, and the first-use review took the grid for "still loading" (A-09).
pub const DEFAULT_CELL: u32 = 224;

/// The smallest and largest cell the buttons will reach.
pub const CELL_RANGE: (u32, u32) = (108, 320);

/// The custom crop ratio's default, 5:4 — a print ratio none of the five preset chips offers.
pub const DEFAULT_CUSTOM_ASPECT: [u32; 2] = [5, 4];

/// Each side of a custom ratio, inclusive. 1000 lets 2.35:1 be written as 235:100.
pub const CUSTOM_ASPECT_RANGE: (u32, u32) = (1, 1000);

/// Survey's tile range, in points. ⚠ Wider at the top than the grid's: a survey of three photos
/// on a large monitor should be able to fill it, which is the whole complaint that put a size
/// control here.
pub const SURVEY_CELL_RANGE: (u32, u32) = (140, 2400);

/// Survey's default tile size. Larger than [`DEFAULT_CELL`] because a survey holds a handful of
/// photographs where the grid holds hundreds.
pub const DEFAULT_SURVEY_CELL: u32 = 320;

/// How much one press of the grid-size buttons moves the cell.
pub const CELL_STEP: u32 = 28;

/// The snapshot strip's default outer height — about four rows at the compact metrics.
pub const DEFAULT_STRIP_HEIGHT: u32 = 110;

/// The smallest and largest the snapshot strip can be dragged to.
///
/// ⚠ The minimum is the header row alone, so someone who wants the space back can drag it away
/// once and have it stay gone. The maximum here is a ceiling on the *stored* value; the panel
/// additionally caps itself at half the live panel height, so a shrunken window clamps further.
pub const STRIP_HEIGHT_RANGE: (u32, u32) = (28, 400);

/// The largest a dialog may be *stored* as, in points.
///
/// ⚠ A ceiling on the stored value only. The dialog additionally clamps itself to 80% of the live
/// viewport, so a size chosen on a large monitor cannot strand it off-screen on a small one.
pub const MAX_DIALOG_EDGE: f32 = 2000.0;

impl Settings {
    /// The grid cell size in points, guaranteed usable.
    pub fn clamped_cell(&self) -> f32 {
        self.library_cell.clamp(CELL_RANGE.0, CELL_RANGE.1) as f32
    }

    /// The custom crop ratio as a pixel width / height, guaranteed finite and positive.
    pub fn custom_aspect_ratio(&self) -> f32 {
        let [w, h] = self.custom_aspect;
        let w = w.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        let h = h.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        w / h
    }

    /// Survey's tile size in points, guaranteed usable.
    ///
    /// ⚠ Its **own** value rather than the grid's, because the two are used for opposite things:
    /// the grid is for finding a photo among hundreds and wants small cells, a survey is for
    /// looking at a handful and wants large ones. Sharing the number would make sizing one
    /// resize the other.
    pub fn clamped_survey_cell(&self) -> f32 {
        self.survey_cell
            .clamp(SURVEY_CELL_RANGE.0, SURVEY_CELL_RANGE.1) as f32
    }

    /// The survey tile size a pinch of `factor` would give. [`Self::zoomed_cell`]'s rule, over
    /// the survey's own wider range.
    pub fn zoomed_survey_cell(&self, factor: f32) -> u32 {
        let now = self.clamped_survey_cell() as u32;
        if !factor.is_finite() || factor <= 0.0 {
            return now;
        }
        let mut next = (now as f32 * factor).round() as u32;
        if next == now {
            match factor.partial_cmp(&1.0) {
                Some(std::cmp::Ordering::Greater) => next = now.saturating_add(1),
                Some(std::cmp::Ordering::Less) => next = now.saturating_sub(1),
                _ => {}
            }
        }
        next.clamp(SURVEY_CELL_RANGE.0, SURVEY_CELL_RANGE.1)
    }

    /// The snapshot strip's height in points, guaranteed usable.
    pub fn clamped_strip_height(&self) -> f32 {
        self.snapshot_strip_height
            .clamp(STRIP_HEIGHT_RANGE.0, STRIP_HEIGHT_RANGE.1) as f32
    }

    /// The size dialog `id` should open at: what it was dragged to, else `default`, clamped.
    ///
    /// ⚠ Clamped rather than trusted — the settings file is user-editable, and a zero width would
    /// make a dialog unreachable. `clamped_cell`'s rule.
    pub fn dialog_size(&self, id: &str, min: (f32, f32), default: (f32, f32)) -> (f32, f32) {
        let (w, h) = match self.dialog_sizes.get(id) {
            Some([w, h]) => (*w as f32, *h as f32),
            None => default,
        };
        (
            w.clamp(min.0, MAX_DIALOG_EDGE),
            h.clamp(min.1, MAX_DIALOG_EDGE),
        )
    }

    /// Record the size dialog `id` is currently at.
    ///
    /// ⚠ In memory only — the **file** is written when the dialog closes, by `modals::draw`. A
    /// resize runs at 60 Hz and the settings file is not a place to write sixty times a second:
    /// the `apply_view_demosaic` rule.
    pub fn remember_dialog_size(&mut self, id: &str, size: (f32, f32)) {
        let v = [
            size.0.round().max(0.0) as u32,
            size.1.round().max(0.0) as u32,
        ];
        // ⚠ Only on a real change, so a dialog merely being open does not dirty the settings and
        // cause a needless file write when it closes.
        if self.dialog_sizes.get(id) != Some(&v) {
            self.dialog_sizes.insert(id.to_string(), v);
        }
    }

    /// The cell size after a pinch or ctrl+scroll of `factor`.
    ///
    /// ⚠⚠ **A step of at least one point is forced whenever a gesture is under way**, and that is
    /// the whole reason this is not a plain multiply. A trackpad pinch reports about 1.01 per
    /// frame; against a whole-point cell that rounds straight back to the same integer, so the
    /// grid would sit still until you pinched hard — indistinguishable from the gesture not being
    /// wired up at all.
    ///
    /// ⚠ egui's `zoom_delta` is a **multiplicative factor that is 1.0 when idle**, not a delta.
    /// Treating it as a delta collapses the size to nothing on every idle frame; this project has
    /// already paid for that once, in the canvas navigation work.
    pub fn zoomed_cell(&self, factor: f32) -> u32 {
        let now = self.clamped_cell() as u32;
        // ⚠ A non-finite or non-positive factor would otherwise produce a nonsense size through
        // the `as u32` cast, which saturates rather than erroring.
        if !factor.is_finite() || factor <= 0.0 {
            return now;
        }
        let mut next = (now as f32 * factor).round() as u32;
        if next == now {
            match factor.partial_cmp(&1.0) {
                Some(std::cmp::Ordering::Greater) => next = now.saturating_add(1),
                Some(std::cmp::Ordering::Less) => next = now.saturating_sub(1),
                _ => {}
            }
        }
        next.clamp(CELL_RANGE.0, CELL_RANGE.1)
    }

    /// The cell size one press of the bigger/smaller buttons would give.
    ///
    /// ⚠ Stepped from the *clamped* value, so a nonsense stored size is corrected by the first
    /// press rather than needing several to climb back into range.
    pub fn stepped_cell(&self, bigger: bool) -> u32 {
        let now = self.clamped_cell() as u32;
        let next = if bigger {
            now.saturating_add(CELL_STEP)
        } else {
            now.saturating_sub(CELL_STEP)
        };
        next.clamp(CELL_RANGE.0, CELL_RANGE.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_edited_custom_aspect_is_repaired_rather_than_dividing_by_zero() {
        let mut s = Settings {
            custom_aspect: [0, 7],
            ..Default::default()
        };
        let r = s.custom_aspect_ratio();
        assert!(r.is_finite() && r > 0.0, "{r}");
        assert!(
            (r - 1.0 / 7.0).abs() < 1e-6,
            "a zero repairs to the minimum: {r}"
        );
        s.custom_aspect = [5, 4];
        assert!((s.custom_aspect_ratio() - 1.25).abs() < 1e-6);
        s.custom_aspect = [50_000, 1];
        assert!(
            (s.custom_aspect_ratio() - 1000.0).abs() < 1e-6,
            "clamped at the top"
        );
    }

    #[test]
    fn an_unusable_cell_size_never_reaches_the_grid() {
        // ⚠ The settings file is hand-editable, and a zero cell divides by zero when the column
        // count is worked out.
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(0).clamped_cell(), CELL_RANGE.0 as f32);
        assert_eq!(with(9999).clamped_cell(), CELL_RANGE.1 as f32);
        assert_eq!(with(200).clamped_cell(), 200.0, "a sane value is untouched");
    }

    #[test]
    fn the_grid_size_buttons_stop_at_the_ends() {
        // ⚠ Both directions, because a step that saturated only upward would let the cell walk
        // below the range and take the column count to zero.
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(CELL_RANGE.1).stepped_cell(true), CELL_RANGE.1);
        assert_eq!(with(CELL_RANGE.0).stepped_cell(false), CELL_RANGE.0);
        assert_eq!(
            with(DEFAULT_CELL).stepped_cell(true),
            DEFAULT_CELL + CELL_STEP
        );
        assert_eq!(
            with(DEFAULT_CELL).stepped_cell(false),
            DEFAULT_CELL - CELL_STEP
        );
        assert_eq!(
            with(0).stepped_cell(true),
            CELL_RANGE.0 + CELL_STEP,
            "a nonsense stored size is corrected by the first press"
        );
    }

    #[test]
    fn an_idle_gesture_leaves_the_grid_size_alone() {
        // ⚠ `zoom_delta` is a *factor* that is exactly 1.0 when nothing is happening, not a
        // delta. Anything that moved the size at 1.0 would shrink the grid on every idle frame.
        let s = Settings::default();
        assert_eq!(s.zoomed_cell(1.0), DEFAULT_CELL);
    }

    #[test]
    fn a_gentle_pinch_still_moves_the_grid() {
        // ⚠⚠ The failure this guards is silence, not wrongness. A trackpad reports ~1.01 per
        // frame; `168 * 1.01 = 169.68`, which rounds to 170 — but at 108 a 1.001 factor rounds
        // straight back to 108, and the control would appear dead until pinched hard.
        let small = Settings {
            library_cell: CELL_RANGE.0,
            ..Settings::default()
        };
        assert_eq!(small.zoomed_cell(1.001), CELL_RANGE.0 + 1);
        let big = Settings {
            library_cell: CELL_RANGE.1,
            ..Settings::default()
        };
        assert_eq!(big.zoomed_cell(0.999), CELL_RANGE.1 - 1);
    }

    #[test]
    fn zooming_stops_at_the_ends_and_survives_nonsense() {
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(CELL_RANGE.1).zoomed_cell(4.0), CELL_RANGE.1);
        assert_eq!(with(CELL_RANGE.0).zoomed_cell(0.25), CELL_RANGE.0);
        // ⚠ `as u32` saturates rather than erroring, so a non-finite factor would otherwise
        // produce a silently absurd size.
        assert_eq!(with(200).zoomed_cell(f32::NAN), 200);
        assert_eq!(with(200).zoomed_cell(f32::INFINITY), 200);
        assert_eq!(with(200).zoomed_cell(0.0), 200);
        assert_eq!(with(200).zoomed_cell(-2.0), 200);
    }

    #[test]
    fn a_nonsense_stored_strip_height_is_clamped_into_range() {
        let too_small = Settings {
            snapshot_strip_height: 0,
            ..Settings::default()
        };
        assert_eq!(
            too_small.clamped_strip_height(),
            STRIP_HEIGHT_RANGE.0 as f32
        );
        let too_big = Settings {
            snapshot_strip_height: 99_999,
            ..Settings::default()
        };
        assert_eq!(too_big.clamped_strip_height(), STRIP_HEIGHT_RANGE.1 as f32);
    }

    /// ⚠ The default must sit inside the range, or the very first frame is clamped to something
    /// nobody chose.
    #[test]
    fn the_default_strip_height_is_within_its_own_range() {
        assert!(DEFAULT_STRIP_HEIGHT >= STRIP_HEIGHT_RANGE.0);
        assert!(DEFAULT_STRIP_HEIGHT <= STRIP_HEIGHT_RANGE.1);
    }

    /// ⚠ The file is user-editable and a zero width would make a dialog unreachable, so a stored
    /// size is clamped rather than trusted — `clamped_cell`'s rule.
    #[test]
    fn a_nonsense_stored_dialog_size_is_clamped_into_the_classs_range() {
        let mut s = Settings::default();
        s.dialog_sizes.insert("x".into(), [0, 0]);
        assert_eq!(
            s.dialog_size("x", (400.0, 120.0), (480.0, 320.0)),
            (400.0, 120.0),
            "clamped up to the class minimum"
        );

        s.dialog_sizes.insert("x".into(), [99_999, 99_999]);
        let (w, h) = s.dialog_size("x", (400.0, 120.0), (480.0, 320.0));
        assert!(w <= MAX_DIALOG_EDGE && h <= MAX_DIALOG_EDGE, "clamped down");
    }

    /// An unresized dialog gets its class default, not zero.
    #[test]
    fn an_unresized_dialog_gets_its_class_default() {
        let s = Settings::default();
        assert_eq!(
            s.dialog_size("never", (400.0, 120.0), (480.0, 320.0)),
            (480.0, 320.0)
        );
    }

    /// A remembered size is returned as stored.
    #[test]
    fn a_remembered_dialog_size_comes_back() {
        let mut s = Settings::default();
        s.remember_dialog_size("x", (612.0, 400.0));
        assert_eq!(
            s.dialog_size("x", (400.0, 120.0), (480.0, 320.0)),
            (612.0, 400.0)
        );
    }
}

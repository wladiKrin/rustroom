//! A mask's adjustments, partitioned into the four sections the panel shows them under.
//!
//! ⚠ The Masks panel draws a mask's [`LocalAdjustments`] under the same four headers as the
//! Edit panel — White Balance, Tone, Color, Detail — each with a modified dot and a reset for
//! exactly that group. Those two need "does this group differ" and "reset this group", and both
//! are answered here through **one** function that copies a group's fields from one set to
//! another, so the two can never disagree about what a group contains.
//!
//! ⚠⚠ **`copy_group` destructures `LocalAdjustments` exhaustively, with no `..`.** A new field
//! then fails to compile until it is assigned to a group. This is the standing lesson about
//! guards that enumerate a list: a match over named fields stops guarding the moment the struct
//! grows, and a `..` would make that silent.

use crate::LocalAdjustments;

/// The four sections a mask's adjustments are shown under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalGroup {
    WhiteBalance,
    Tone,
    Color,
    Detail,
}

impl LocalGroup {
    pub const ALL: [LocalGroup; 4] = [
        LocalGroup::WhiteBalance,
        LocalGroup::Tone,
        LocalGroup::Color,
        LocalGroup::Detail,
    ];

    /// The header's name — the same words the Edit panel uses.
    pub fn name(self) -> &'static str {
        match self {
            LocalGroup::WhiteBalance => "White Balance",
            LocalGroup::Tone => "Tone",
            LocalGroup::Color => "Color",
            LocalGroup::Detail => "Detail",
        }
    }
}

impl LocalAdjustments {
    /// Whether `group` differs from its default.
    pub fn group_differs(&self, group: LocalGroup) -> bool {
        let mut reset = self.clone();
        reset.reset_group(group);
        reset != *self
    }

    /// Put `group` back to its default, leaving the other groups alone.
    pub fn reset_group(&mut self, group: LocalGroup) {
        self.copy_group(&LocalAdjustments::default(), group);
    }

    /// Copy `group`'s fields from `from`.
    ///
    /// ⚠ Every field is named on both sides, and the compiler enforces it. Dehaze and noise
    /// reduction run in the prefix, but they are *shown* with Presence and Detail — the panel's
    /// grouping is the workflow's, exactly as `mask_adjust`'s doc records.
    fn copy_group(&mut self, from: &LocalAdjustments, group: LocalGroup) {
        let LocalAdjustments {
            noise_reduction,
            dehaze,
            white_balance,
            exposure,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            tone_curve,
            saturation,
            vibrance,
            local_contrast,
            sharpening,
            hsl,
            color_grading,
        } = self;
        match group {
            LocalGroup::WhiteBalance => {
                *white_balance = from.white_balance;
            }
            LocalGroup::Tone => {
                *exposure = from.exposure;
                *contrast = from.contrast;
                *highlights = from.highlights;
                *shadows = from.shadows;
                *whites = from.whites;
                *blacks = from.blacks;
                *tone_curve = from.tone_curve.clone();
                *local_contrast = from.local_contrast;
                *dehaze = from.dehaze;
            }
            LocalGroup::Color => {
                *vibrance = from.vibrance;
                *saturation = from.saturation;
                *hsl = from.hsl;
                *color_grading = from.color_grading;
            }
            LocalGroup::Detail => {
                *noise_reduction = from.noise_reduction;
                *sharpening = from.sharpening;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One representative field per group, moved off its default.
    fn moved(group: LocalGroup) -> LocalAdjustments {
        let mut a = LocalAdjustments::default();
        match group {
            LocalGroup::WhiteBalance => a.white_balance.tint = 10.0,
            LocalGroup::Tone => a.dehaze.amount = 20.0,
            LocalGroup::Color => a.hsl.bands[2].hue = 15.0,
            LocalGroup::Detail => a.noise_reduction.color = 30.0,
        }
        a
    }

    #[test]
    fn one_moved_field_differs_in_exactly_its_group() {
        // ⚠ Both halves for every group: a `group_differs` that always said yes, or one that
        // checked the wrong fields, would each pass a one-sided test.
        for g in LocalGroup::ALL {
            let a = moved(g);
            for h in LocalGroup::ALL {
                assert_eq!(
                    a.group_differs(h),
                    g == h,
                    "{} moved; asked about {}",
                    g.name(),
                    h.name()
                );
            }
        }
    }

    #[test]
    fn reset_group_clears_exactly_its_group() {
        for g in LocalGroup::ALL {
            let mut a = LocalAdjustments::default();
            a.white_balance.tint = 10.0;
            a.dehaze.amount = 20.0;
            a.hsl.bands[2].hue = 15.0;
            a.noise_reduction.color = 30.0;
            a.reset_group(g);
            for h in LocalGroup::ALL {
                assert_eq!(
                    a.group_differs(h),
                    g != h,
                    "reset {}; {} should {}differ",
                    g.name(),
                    h.name(),
                    if g != h { "" } else { "not " }
                );
            }
        }
    }

    #[test]
    fn resetting_every_group_reaches_the_default() {
        // ⚠ The property that proves the four groups cover the struct: a field in no group
        // would survive all four resets and fail this.
        let mut a = LocalAdjustments {
            exposure: 1.0,
            contrast: 2.0,
            highlights: 3.0,
            shadows: 4.0,
            whites: 5.0,
            blacks: 6.0,
            saturation: 7.0,
            vibrance: 8.0,
            ..Default::default()
        };
        a.white_balance.tint = 9.0;
        a.tone_curve.points.push([0.5, 0.6]);
        a.local_contrast.texture = 10.0;
        a.dehaze.amount = 11.0;
        a.hsl.bands[0].saturation = 12.0;
        a.color_grading.blending = 13.0;
        a.noise_reduction.luminance = 14.0;
        a.sharpening.amount = 15.0;
        assert!(a.is_active(), "fixture: everything moved");
        for g in LocalGroup::ALL {
            a.reset_group(g);
        }
        assert_eq!(a, LocalAdjustments::default());
    }
}

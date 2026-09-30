//! The "don't ask again" confirmations, folded into the settings the same way everywhere.

use super::Settings;

/// What a confirm dialog decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// Dismissed — nothing happens, and any "don't ask again" tick is discarded.
    Cancelled,
    /// Confirmed, carrying the "Don't ask again" tick.
    Confirmed { suppress_future: bool },
}

/// The shared rule behind both confirm folds: `suppress_future` is honoured **only** on
/// `Confirmed`. One implementation because that rule is the load-bearing part, and two copies
/// of it would be two chances to get it wrong.
fn apply_confirm(flag: &mut bool, outcome: ConfirmOutcome) -> bool {
    match outcome {
        ConfirmOutcome::Cancelled => false,
        ConfirmOutcome::Confirmed { suppress_future } => {
            if suppress_future {
                *flag = false;
            }
            true
        }
    }
}

impl Settings {
    /// Fold a delete-confirm outcome into the settings. Returns whether to actually delete.
    ///
    /// ⚠ `suppress_future` is honoured **only** on `Confirmed`. Ticking "Don't ask again"
    /// and then cancelling must leave the guard armed — otherwise a user disarms a guard
    /// they just declined to use. Pure and separated from the egui code precisely so that
    /// rule is testable.
    pub fn apply_delete_confirm(&mut self, outcome: ConfirmOutcome) -> bool {
        apply_confirm(&mut self.confirm_preset_delete, outcome)
    }

    /// The same fold, for **export** presets and their own flag.
    pub fn apply_export_delete_confirm(&mut self, outcome: ConfirmOutcome) -> bool {
        apply_confirm(&mut self.confirm_export_preset_delete, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_delete_confirmation_defaults_to_on() {
        assert!(Settings::default().confirm_export_preset_delete);
    }

    #[test]
    fn the_two_delete_guards_are_independent() {
        // ⚠ The reason they are separate flags. A develop preset goes to a trash and can be
        // restored; an export preset is deleted outright. Disarming the guard on the
        // recoverable deletion must not disarm it on the irrecoverable one.
        let mut s = Settings::default();
        s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true,
        });
        assert!(!s.confirm_preset_delete);
        assert!(
            s.confirm_export_preset_delete,
            "suppressing the develop-preset guard disarmed the export one"
        );
    }

    #[test]
    fn cancelling_never_suppresses_the_export_confirmation() {
        let mut s = Settings::default();
        assert!(!s.apply_export_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_export_preset_delete);
        // Ticking the box and then cancelling must leave the guard armed.
        assert!(!s.apply_export_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_export_preset_delete);
    }

    #[test]
    fn confirming_with_the_box_ticked_suppresses_the_export_confirmation() {
        let mut s = Settings::default();
        assert!(s.apply_export_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true
        }));
        assert!(!s.confirm_export_preset_delete);
    }

    #[test]
    fn the_delete_confirm_defaults_to_on() {
        assert!(Settings::default().confirm_preset_delete);
    }

    #[test]
    fn cancelling_never_suppresses_the_confirmation() {
        // ⚠ The trap. Tick "Don't ask again", think better of the deletion, hit Cancel —
        // and the guard you just declined to use must still be armed next time.
        let mut s = Settings::default();
        assert!(!s.apply_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_preset_delete, "cancel disarmed the confirmation");
    }

    #[test]
    fn confirming_with_the_box_ticked_suppresses_future_confirmations() {
        let mut s = Settings::default();
        assert!(s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true
        }));
        assert!(!s.confirm_preset_delete);
    }

    #[test]
    fn confirming_without_the_box_leaves_the_setting_alone() {
        let mut s = Settings::default();
        assert!(s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: false
        }));
        assert!(s.confirm_preset_delete);
    }
}

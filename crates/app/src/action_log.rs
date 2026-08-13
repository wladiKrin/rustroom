//! One chronological undo ordering across *kinds* of action.
//!
//! Photo edits and preset deletions are undoable, but their payloads live in different
//! places: photo snapshots in `Session::history`, deleted presets in the preset trash. This
//! log records only **which stack the next undo should pop from**, so the payloads are never
//! duplicated and the two existing mechanisms keep working unchanged.
//!
//! That is what makes "delete a preset, change a setting, press Ctrl+Z twice" bring the
//! preset back — the two actions sit in one order rather than in two competing stacks.

use edit_model::{EditGroups, Edits};
use std::path::PathBuf;

/// A single undoable action, in the order it happened.
///
/// ⚠ Not `Eq`: `Edits` holds `f32`s, so it is `PartialEq` only. That is all the tests need.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind {
    /// A step was recorded in the history of the photo at `path`.
    ///
    /// ⚠ The path is what makes the entry mean anything after a photo switch. It used to be
    /// a unit variant, and an anonymous entry can only say "pop whatever history is current"
    /// — which after a switch is a *different photo's*. That is the sole reason every such
    /// entry had to be destroyed whenever the session changed.
    PhotoEdit { path: PathBuf },
    /// A preset was moved to the trash. Named so a redo can delete the same one again, and
    /// so an undo restores *that* preset rather than whatever happens to be newest.
    PresetDelete { name: String },
    /// One photo's settings were applied to several others' sidecars.
    ///
    /// **Boxed on purpose:** the payload is a few hundred bytes, and an unboxed variant
    /// would make every entry in the undo and redo stacks that large — including the
    /// zero-sized `PhotoEdit`, which is by far the most common.
    Sync(Box<SyncAction>),
    /// Several actions performed by one gesture, undone and redone as a unit.
    ///
    /// Pasting into a multi-selection is the motivating case: it changes the open photo
    /// through its session *and* the rest through their sidecars, which are two different
    /// mechanisms — but one Ctrl+V must stay one Ctrl+Z.
    Batch(Vec<ActionKind>),
}

/// What a [`ActionKind::Sync`] needs to be reversed *and* replayed.
///
/// `prior` restores each target — `None` means the photo had no sidecar, so undoing must
/// delete the file the sync created — while `source` and `groups` re-apply it. Global, like
/// `PresetDelete`: it never touches the live session.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncAction {
    pub source: Edits,
    pub groups: EditGroups,
    pub prior: Vec<(PathBuf, Option<Edits>)>,
}

/// Undo/redo ordering over [`ActionKind`]. Mirrors `History`'s shape deliberately.
#[derive(Debug, Default)]
pub struct ActionLog {
    undo: Vec<ActionKind>,
    redo: Vec<ActionKind>,
}

impl ActionLog {
    /// Record a new action. Clears the redo stack, like any undo history.
    pub fn push(&mut self, kind: ActionKind) {
        self.undo.push(kind);
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What the next undo would reverse, without performing it — for a button label.
    pub fn peek_undo(&self) -> Option<&ActionKind> {
        self.undo.last()
    }

    /// Take the most recent action for undoing. The caller performs the reversal.
    pub fn undo(&mut self) -> Option<ActionKind> {
        let k = self.undo.pop()?;
        self.redo.push(k.clone());
        Some(k)
    }

    /// Take the most recently undone action for redoing.
    pub fn redo(&mut self) -> Option<ActionKind> {
        let k = self.redo.pop()?;
        self.undo.push(k.clone());
        Some(k)
    }

    /// Drop every photo edit, keeping preset deletions in their relative order.
    ///
    /// ⚠ Called when the session changes. `Session::history` is discarded on a photo switch,
    /// so a `PhotoEdit` entry that outlived its session would make an undo pop a *different*
    /// photo's history — silently reverting an edit the user never made here. Preset
    /// deletions are global and survive.
    pub fn forget_photo_edits(&mut self) {
        Self::sweep(&mut self.undo);
        Self::sweep(&mut self.redo);
    }

    /// Drop photo edits from one stack, **including those nested in a batch**.
    ///
    /// A batch keeps its other members: stranding the sidecar half of a paste with no way
    /// to undo it would be worse than losing the session half, which is unreachable anyway.
    /// A batch left with nothing is not an undo step and goes.
    fn sweep(stack: &mut Vec<ActionKind>) {
        for kind in stack.iter_mut() {
            if let ActionKind::Batch(parts) = kind {
                parts.retain(|p| !matches!(p, ActionKind::PhotoEdit { .. }));
            }
        }
        stack.retain(|kind| match kind {
            ActionKind::PhotoEdit { .. } => false,
            ActionKind::Batch(parts) => !parts.is_empty(),
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn del(name: &str) -> ActionKind {
        ActionKind::PresetDelete {
            name: name.to_string(),
        }
    }

    fn edit(path: &str) -> ActionKind {
        ActionKind::PhotoEdit {
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn undo_returns_actions_most_recent_first() {
        let mut log = ActionLog::default();
        log.push(del("A"));
        log.push(edit("/a.RAF"));
        assert_eq!(log.undo(), Some(edit("/a.RAF")));
        assert_eq!(log.undo(), Some(del("A")));
        assert_eq!(log.undo(), None);
    }

    #[test]
    fn a_preset_delete_survives_an_intervening_photo_edit() {
        // The scenario this whole module exists for: delete a preset, change a setting,
        // Ctrl+Z twice — the first undo reverses the setting, the second brings the preset
        // back. Two separate stacks could never express this ordering.
        let mut log = ActionLog::default();
        log.push(del("Punchy"));
        log.push(edit("/a.RAF"));

        assert_eq!(log.undo(), Some(edit("/a.RAF")));
        assert_eq!(log.undo(), Some(del("Punchy")));
    }

    #[test]
    fn several_deletions_undo_newest_first() {
        let mut log = ActionLog::default();
        for n in ["A", "B", "C"] {
            log.push(del(n));
        }
        assert_eq!(log.undo(), Some(del("C")));
        assert_eq!(log.undo(), Some(del("B")));
        assert_eq!(log.undo(), Some(del("A")));
        assert!(!log.can_undo());
    }

    #[test]
    fn redo_replays_in_the_original_order() {
        let mut log = ActionLog::default();
        log.push(del("A"));
        log.push(edit("/a.RAF"));
        log.undo();
        log.undo();
        assert!(!log.can_undo());
        assert!(log.can_redo());

        assert_eq!(log.redo(), Some(del("A")));
        assert_eq!(log.redo(), Some(edit("/a.RAF")));
        assert_eq!(log.redo(), None);
        assert!(log.can_undo());
    }

    #[test]
    fn a_new_action_clears_the_redo_stack() {
        let mut log = ActionLog::default();
        log.push(del("A"));
        log.undo();
        assert!(log.can_redo());
        log.push(edit("/a.RAF"));
        assert!(
            !log.can_redo(),
            "a new action must invalidate the redo path"
        );
    }

    #[test]
    fn peek_does_not_consume() {
        let mut log = ActionLog::default();
        log.push(del("A"));
        assert_eq!(log.peek_undo(), Some(&del("A")));
        assert_eq!(log.peek_undo(), Some(&del("A")));
        assert!(log.can_undo());
    }

    fn sync_entry() -> ActionKind {
        ActionKind::Sync(Box::new(SyncAction {
            source: Edits {
                exposure: 1.0,
                ..Default::default()
            },
            groups: EditGroups::default(),
            prior: vec![(PathBuf::from("/a/b.RAF"), None)],
        }))
    }

    #[test]
    fn a_sync_survives_a_photo_switch() {
        // A sync touches other photos' sidecars, never the live session, so it is global
        // in exactly the way a preset deletion is.
        let mut log = ActionLog::default();
        log.push(sync_entry());
        log.push(edit("/a.RAF"));
        log.forget_photo_edits();
        assert_eq!(log.undo(), Some(sync_entry()));
        assert!(!log.can_undo());
    }

    #[test]
    fn a_batch_is_one_step() {
        // The log must hand back the whole group at once; splitting it would make one
        // Ctrl+V take two Ctrl+Z.
        let mut log = ActionLog::default();
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        log.push(batch.clone());
        log.push(edit("/a.RAF"));

        assert_eq!(log.undo(), Some(edit("/a.RAF")));
        assert_eq!(log.undo(), Some(batch.clone()));
        assert!(!log.can_undo());
        assert_eq!(log.redo(), Some(batch));
    }

    #[test]
    fn forgetting_photo_edits_reaches_inside_a_batch() {
        // ⚠ A batch's `PhotoEdit` member belongs to a session that is now gone, so undoing
        // it would pop a *different* photo's history — the very bug this method exists to
        // prevent. But dropping the whole batch would strand the sidecar half of a paste
        // with no way to undo it, so the member goes and the rest stays.
        let mut log = ActionLog::default();
        log.push(ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]));
        log.forget_photo_edits();
        assert_eq!(log.undo(), Some(ActionKind::Batch(vec![sync_entry()])));
    }

    #[test]
    fn a_batch_of_only_photo_edits_is_dropped_entirely() {
        let mut log = ActionLog::default();
        log.push(ActionKind::Batch(vec![edit("/a.RAF"), edit("/a.RAF")]));
        log.forget_photo_edits();
        assert!(!log.can_undo(), "an empty batch is not an undo step");
    }

    #[test]
    fn a_sync_orders_with_everything_else() {
        let mut log = ActionLog::default();
        log.push(sync_entry());
        log.push(edit("/a.RAF"));
        assert_eq!(log.undo(), Some(edit("/a.RAF")));
        assert_eq!(log.undo(), Some(sync_entry()));
    }

    #[test]
    fn forgetting_photo_edits_keeps_deletions_in_order() {
        // ⚠ A PhotoEdit that outlived its session would pop a different photo's history.
        let mut log = ActionLog::default();
        log.push(del("A"));
        log.push(edit("/a.RAF"));
        log.push(del("B"));
        log.push(edit("/a.RAF"));

        log.forget_photo_edits();
        assert_eq!(log.undo(), Some(del("B")));
        assert_eq!(log.undo(), Some(del("A")));
        assert!(!log.can_undo());
    }

    #[test]
    fn forgetting_photo_edits_cleans_the_redo_stack_too() {
        // Otherwise a redo after a photo switch would replay into the new session.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"));
        log.push(del("A"));
        log.undo();
        log.undo();

        log.forget_photo_edits();
        assert_eq!(log.redo(), Some(del("A")));
        assert_eq!(log.redo(), None, "the photo edit should be gone from redo");
    }
}

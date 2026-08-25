//! One chronological undo ordering across *kinds* of action.
//!
//! Photo edits and preset deletions are undoable, but their payloads live in different
//! places: photo snapshots in `Session::history`, deleted presets in the preset trash. This
//! log records only **which stack the next undo should pop from**, so the payloads are never
//! duplicated and the two existing mechanisms keep working unchanged.
//!
//! That is what makes "delete a preset, change a setting, press Ctrl+Z twice" bring the
//! preset back — the two actions sit in one order rather than in two competing stacks.

use crate::photo_id::PhotoId;
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
    PhotoEdit { id: PhotoId },
    /// A deleted virtual copy, held so undo can put it back.
    ///
    /// ⚠ Carries the whole `CopyEntry` because nothing else holds it — unlike `PhotoEdit`, whose
    /// payload lives in `Session::history`, and `PresetDelete`, whose payload is in the trash.
    /// Boxed for the reason `Sync` is: an inline `Edits` would make *every* entry in the log that
    /// large, including the zero-sized `PhotoEdit`.
    CopyDelete {
        path: PathBuf,
        entry: Box<crate::sidecar::CopyEntry>,
    },
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
    /// The copies the sync was applied to.
    ///
    /// ⚠ Kept explicitly because `prior` is per **file** and cannot say which copies were
    /// touched — redo would otherwise re-sync copy 0 of every file involved, which is not what
    /// was done the first time.
    pub targets: Vec<PhotoId>,
    /// ⚠ The whole file's `Sidecar` per touched file, not one copy's `Edits` — see
    /// `sync::SyncReport::prior`. Two copies of one file share a sidecar.
    pub prior: Vec<(PathBuf, crate::sidecar::Sidecar)>,
}

/// One entry in the log, plus whether it is currently undone.
///
/// ⚠ A flag rather than two stacks. Filtering two stacks by reachability means taking an
/// entry out of the *middle* of one and pushing it back on the *top* of the other, which
/// scrambles chronological order across photos — observable as a wrong undo order the next
/// time you switch, and repairable only with a sequence number on every entry. Marking in
/// place makes the ordering intrinsic instead.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    kind: ActionKind,
    undone: bool,
}

/// Whether `kind` can be undone or redone while `open` is the photo on screen.
///
/// A preset deletion or a sync never touches a live session, so it is reachable from
/// anywhere. A photo edit is reachable only from its own photo: undoing it from elsewhere
/// would either change an image the user cannot see or yank the view to it, and both were
/// rejected.
///
/// ⚠ A batch is reachable only when **every** member is, so one Ctrl+V stays one Ctrl+Z. It
/// waits, intact, until you return to the photo it touched.
fn reachable(kind: &ActionKind, open: Option<&PhotoId>) -> bool {
    match kind {
        ActionKind::PhotoEdit { id } => open == Some(id),
        // ⚠ A deleted copy has no `PhotoId` left to be reachable *from*, so it is global like a
        // preset deletion rather than scoped like an edit.
        ActionKind::PresetDelete { .. } | ActionKind::Sync(_) | ActionKind::CopyDelete { .. } => {
            true
        }
        ActionKind::Batch(parts) => parts.iter().all(|p| reachable(p, open)),
    }
}

/// Undo/redo ordering over [`ActionKind`], scoped to the photo on screen.
#[derive(Debug, Default)]
pub struct ActionLog {
    entries: Vec<Entry>,
}

impl ActionLog {
    /// Record a new action.
    ///
    /// Clears the redo path **this action can reach** — edits to the open photo, and global
    /// actions. Another photo's undone step is left alone: nothing done here invalidates it,
    /// and discarding it would reintroduce "your stack is gone" one level down.
    pub fn push(&mut self, kind: ActionKind, open: Option<&PhotoId>) {
        self.entries
            .retain(|e| !(e.undone && reachable(&e.kind, open)));
        self.entries.push(Entry {
            kind,
            undone: false,
        });
    }

    pub fn can_undo(&self, open: Option<&PhotoId>) -> bool {
        self.undo_index(open).is_some()
    }

    pub fn can_redo(&self, open: Option<&PhotoId>) -> bool {
        self.redo_index(open).is_some()
    }

    /// What the next undo would reverse, without performing it — for a button label.
    pub fn peek_undo(&self, open: Option<&PhotoId>) -> Option<&ActionKind> {
        Some(&self.entries[self.undo_index(open)?].kind)
    }

    /// Take the most recent reachable action for undoing. The caller performs the reversal.
    pub fn undo(&mut self, open: Option<&PhotoId>) -> Option<ActionKind> {
        let i = self.undo_index(open)?;
        self.entries[i].undone = true;
        Some(self.entries[i].kind.clone())
    }

    /// Take the most recently undone reachable action for redoing.
    pub fn redo(&mut self, open: Option<&PhotoId>) -> Option<ActionKind> {
        let i = self.redo_index(open)?;
        self.entries[i].undone = false;
        Some(self.entries[i].kind.clone())
    }

    /// The last reachable entry still in force.
    fn undo_index(&self, open: Option<&PhotoId>) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|e| !e.undone && reachable(&e.kind, open))
    }

    /// The first reachable entry that has been undone.
    ///
    /// ⚠ *First*, not last, and that is what makes redo the exact inverse of undo. Undo
    /// always takes the last reachable entry in force, so within any one photo's subsequence
    /// the entries in force form a prefix and the undone ones a suffix — the first undone one
    /// is the boundary, which is the one undone most recently.
    fn redo_index(&self, open: Option<&PhotoId>) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.undone && reachable(&e.kind, open))
    }

    /// Drop the edits of one photo, keeping every other photo's and all global actions.
    ///
    /// ⚠ Called only when that photo's `History<Edits>` has actually been discarded — when a
    /// sync rewrote the sidecar under a live session, or when `HistoryStash` refused to hand
    /// a stale stack back. An entry left pointing at a history that no longer holds its steps
    /// would make an undo do nothing at all, silently.
    ///
    /// A batch keeps its other members: stranding the sidecar half of a paste with no way to
    /// undo it would be worse than losing the session half, which is unreachable anyway. A
    /// batch left with nothing is not an undo step and goes.
    pub fn forget_edits_of(&mut self, id: &PhotoId) {
        let mine = |k: &ActionKind| matches!(k, ActionKind::PhotoEdit { id: i } if i == id);
        for e in self.entries.iter_mut() {
            if let ActionKind::Batch(parts) = &mut e.kind {
                parts.retain(|p| !mine(p));
            }
        }
        self.entries.retain(|e| match &e.kind {
            ActionKind::Batch(parts) => !parts.is_empty(),
            k => !mine(k),
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
        ActionKind::PhotoEdit { id: id(path) }
    }

    /// A photo's original, from a path — the shape every pre-existing test means.
    fn id(path: &str) -> PhotoId {
        PhotoId::original(PathBuf::from(path))
    }

    /// The two photos the reachability tests switch between.
    ///
    /// ⚠ These return `&'static PhotoId` via `OnceLock` specifically so that every call site
    /// below keeps its exact previous form. The pre-existing tests passing **unchanged** is the
    /// evidence that re-keying the log on `PhotoId` preserved its behaviour for ordinary photos.
    fn a() -> Option<&'static PhotoId> {
        static A: std::sync::OnceLock<PhotoId> = std::sync::OnceLock::new();
        Some(A.get_or_init(|| id("/a.RAF")))
    }

    fn b() -> Option<&'static PhotoId> {
        static B: std::sync::OnceLock<PhotoId> = std::sync::OnceLock::new();
        Some(B.get_or_init(|| id("/b.RAF")))
    }

    #[test]
    fn undo_returns_actions_most_recent_first() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        log.push(edit("/a.RAF"), a());
        assert_eq!(log.undo(a()), Some(edit("/a.RAF")));
        assert_eq!(log.undo(a()), Some(del("A")));
        assert_eq!(log.undo(a()), None);
    }

    #[test]
    fn a_preset_delete_survives_an_intervening_photo_edit() {
        // The scenario this whole module exists for: delete a preset, change a setting,
        // Ctrl+Z twice — the first undo reverses the setting, the second brings the preset
        // back. Two separate stacks could never express this ordering.
        let mut log = ActionLog::default();
        log.push(del("Punchy"), a());
        log.push(edit("/a.RAF"), a());

        assert_eq!(log.undo(a()), Some(edit("/a.RAF")));
        assert_eq!(log.undo(a()), Some(del("Punchy")));
    }

    #[test]
    fn several_deletions_undo_newest_first() {
        let mut log = ActionLog::default();
        for n in ["A", "B", "C"] {
            log.push(del(n), a());
        }
        assert_eq!(log.undo(a()), Some(del("C")));
        assert_eq!(log.undo(a()), Some(del("B")));
        assert_eq!(log.undo(a()), Some(del("A")));
        assert!(!log.can_undo(a()));
    }

    #[test]
    fn redo_replays_in_the_original_order() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.undo(a());
        assert!(!log.can_undo(a()));
        assert!(log.can_redo(a()));

        assert_eq!(log.redo(a()), Some(del("A")));
        assert_eq!(log.redo(a()), Some(edit("/a.RAF")));
        assert_eq!(log.redo(a()), None);
        assert!(log.can_undo(a()));
    }

    #[test]
    fn a_new_action_clears_the_redo_stack() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        log.undo(a());
        assert!(log.can_redo(a()));
        log.push(edit("/a.RAF"), a());
        assert!(
            !log.can_redo(a()),
            "a new action must invalidate the redo path"
        );
    }

    #[test]
    fn peek_does_not_consume() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        assert_eq!(log.peek_undo(a()), Some(&del("A")));
        assert_eq!(log.peek_undo(a()), Some(&del("A")));
        assert!(log.can_undo(a()));
    }

    fn sync_entry() -> ActionKind {
        ActionKind::Sync(Box::new(SyncAction {
            source: Edits {
                exposure: 1.0,
                ..Default::default()
            },
            groups: EditGroups::default(),
            targets: vec![PhotoId::original(PathBuf::from("/a/b.RAF"))],
            prior: vec![(
                PathBuf::from("/a/b.RAF"),
                crate::sidecar::Sidecar::default(),
            )],
        }))
    }

    #[test]
    fn a_sync_survives_a_photo_switch() {
        // A sync touches other photos' sidecars, never the live session, so it is global
        // in exactly the way a preset deletion is.
        let mut log = ActionLog::default();
        log.push(sync_entry(), a());
        log.push(edit("/a.RAF"), a());
        log.forget_edits_of(&id("/a.RAF"));
        assert_eq!(log.undo(a()), Some(sync_entry()));
        assert!(!log.can_undo(a()));
    }

    #[test]
    fn a_batch_is_one_step() {
        // The log must hand back the whole group at once; splitting it would make one
        // Ctrl+V take two Ctrl+Z.
        let mut log = ActionLog::default();
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        log.push(batch.clone(), a());
        log.push(edit("/a.RAF"), a());

        assert_eq!(log.undo(a()), Some(edit("/a.RAF")));
        assert_eq!(log.undo(a()), Some(batch.clone()));
        assert!(!log.can_undo(a()));
        assert_eq!(log.redo(a()), Some(batch));
    }

    #[test]
    fn a_sync_orders_with_everything_else() {
        let mut log = ActionLog::default();
        log.push(sync_entry(), a());
        log.push(edit("/a.RAF"), a());
        assert_eq!(log.undo(a()), Some(edit("/a.RAF")));
        assert_eq!(log.undo(a()), Some(sync_entry()));
    }

    #[test]
    fn an_edit_to_another_photo_is_not_undoable_here() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        assert!(!log.can_undo(b()));
        assert_eq!(log.undo(b()), None);
    }

    #[test]
    fn returning_to_a_photo_makes_its_edits_undoable_again() {
        // The headline property, and the one that fails against the old anonymous entries:
        // A's step is merely out of reach from B, not destroyed.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        assert!(!log.can_undo(b()), "not reachable from B");
        assert_eq!(
            log.undo(a()),
            Some(edit("/a.RAF")),
            "but still there on return"
        );
    }

    #[test]
    fn a_new_action_clears_only_the_redo_it_can_reach() {
        // Photo A has an undone step waiting. Editing B invalidates nothing about A.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.push(edit("/b.RAF"), b());
        assert_eq!(
            log.redo(a()),
            Some(edit("/a.RAF")),
            "B's edit cannot invalidate A's redo path"
        );
    }

    #[test]
    fn a_new_action_clears_its_own_photos_redo() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.push(edit("/a.RAF"), a());
        assert!(
            !log.can_redo(a()),
            "the classic rule still holds within a photo"
        );
    }

    #[test]
    fn a_new_action_clears_the_global_redo_from_any_photo() {
        // A preset deletion is reachable everywhere, so an edit to any photo invalidates it.
        let mut log = ActionLog::default();
        log.push(del("P"), a());
        log.undo(a());
        log.push(edit("/b.RAF"), b());
        assert!(!log.can_redo(a()));
        assert_eq!(log.redo(b()), None);
    }

    #[test]
    fn a_batch_waits_until_every_member_is_reachable() {
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        let mut log = ActionLog::default();
        log.push(batch.clone(), a());
        assert!(!log.can_undo(b()), "half of it belongs to A");
        assert_eq!(log.undo(a()), Some(batch));
    }

    #[test]
    fn a_batch_survives_a_visit_to_another_photo_intact() {
        // ⚠ The wart this rework removes. The old sweep tore the `PhotoEdit` member out of
        // every batch on every switch, permanently stranding the sidecar half of a paste.
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        let mut log = ActionLog::default();
        log.push(batch.clone(), a());
        assert!(!log.can_undo(b()));
        assert_eq!(log.undo(a()), Some(batch), "intact, not half destroyed");
    }

    #[test]
    fn undo_and_redo_are_inverses_with_two_photos_interleaved() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(del("P"), a());
        log.push(edit("/b.RAF"), b());

        // On B: B's own edit is newest, then the global deletion. A's edit is out of reach.
        assert_eq!(log.undo(b()), Some(edit("/b.RAF")));
        assert_eq!(log.undo(b()), Some(del("P")));
        assert_eq!(log.undo(b()), None, "A's edit is not reachable from B");

        // Redo replays in the order the entries were undone.
        assert_eq!(log.redo(b()), Some(del("P")));
        assert_eq!(log.redo(b()), Some(edit("/b.RAF")));
        assert_eq!(log.redo(b()), None);
    }

    #[test]
    fn with_no_photo_open_only_global_actions_are_reachable() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(del("P"), a());
        assert_eq!(log.undo(None), Some(del("P")));
        assert_eq!(log.undo(None), None);
    }

    #[test]
    fn forgetting_one_photos_edits_leaves_other_photos_alone() {
        // ⚠ The whole reason the sweep is targeted now: photo B's stack is still in the
        // stash, so its entries must survive photo A's being discarded.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(edit("/b.RAF"), b());
        log.forget_edits_of(&id("/a.RAF"));

        assert_eq!(log.undo(a()), None, "A's entry is gone");
        assert_eq!(log.undo(b()), Some(edit("/b.RAF")), "B's is not");
    }

    #[test]
    fn forgetting_a_photos_edits_reaches_inside_a_batch() {
        // The member belongs to a history that was just discarded, so undoing it would pop
        // steps that history no longer holds. Dropping the whole batch instead would strand
        // the sidecar half of a paste with no way to undo it, so the member goes and the
        // rest stays.
        let mut log = ActionLog::default();
        log.push(ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]), a());
        log.forget_edits_of(&id("/a.RAF"));
        assert_eq!(log.undo(a()), Some(ActionKind::Batch(vec![sync_entry()])));
    }

    #[test]
    fn a_batch_of_only_that_photos_edits_is_dropped_entirely() {
        let mut log = ActionLog::default();
        log.push(ActionKind::Batch(vec![edit("/a.RAF"), edit("/a.RAF")]), a());
        log.forget_edits_of(&id("/a.RAF"));
        assert!(!log.can_undo(a()), "an empty batch is not an undo step");
    }

    #[test]
    fn forgetting_a_photos_edits_clears_its_undone_entries_too() {
        // Otherwise a redo after the discard would replay into a history that is gone.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.forget_edits_of(&id("/a.RAF"));
        assert!(!log.can_redo(a()));
    }

    #[test]
    fn forgetting_a_photos_edits_keeps_global_actions_in_order() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        log.push(edit("/a.RAF"), a());
        log.push(del("B"), a());
        log.forget_edits_of(&id("/a.RAF"));

        assert_eq!(log.undo(a()), Some(del("B")));
        assert_eq!(log.undo(a()), Some(del("A")));
        assert!(!log.can_undo(a()));
    }

    #[test]
    fn undo_on_the_original_cannot_reach_a_copys_edit() {
        // ⚠ The half the stash alone does not give you. CLAUDE.md, on the per-photo edit
        // history: "A per-path history alone does nothing, because `ActionKind::PhotoEdit` was
        // anonymous." With `PhotoEdit` carrying a bare path, an edit made on copy 1 would be
        // reachable from the original and Ctrl+Z there would pop the wrong stack.
        use crate::photo_id::PhotoId;
        let path = std::path::PathBuf::from("/a.RAF");
        let original = PhotoId::original(path.clone());
        let copy = PhotoId::new(path, 1);

        let mut log = ActionLog::default();
        log.push(ActionKind::PhotoEdit { id: copy.clone() }, Some(&copy));
        assert!(
            log.undo(Some(&original)).is_none(),
            "the original must not reach a copy's edit"
        );
        assert!(log.undo(Some(&copy)).is_some(), "its own copy must");
    }
}

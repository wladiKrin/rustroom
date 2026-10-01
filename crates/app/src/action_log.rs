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
    /// A photo **file** renamed on disk, with its sidecar.
    ///
    /// ⚠ Its own entry rather than part of `PhotoEdit`: a filename is not an edit, so
    /// `History<Edits>` cannot undo it. ⚠ And it is **global**, not scoped to a photo — the
    /// photo it names no longer exists at the old path, so there is nothing for `reachable` to
    /// compare against.
    Rename { from: PathBuf, to: PathBuf },
    /// A virtual **copy** renamed — its own label, not the file's.
    ///
    /// ⚠ Distinct from `Rename` because they change different things and are reachable from
    /// different places. A file rename is global (the photo no longer exists at the old path);
    /// a copy rename is scoped to its copy by the same rule `PhotoEdit` uses, so Ctrl+Z on
    /// another photo cannot reach it.
    ///
    /// ⚠ Not a `PhotoEdit`: the name lives in the sidecar envelope rather than in `Edits`, so
    /// `History<Edits>` has nothing to pop for it.
    CopyRename {
        id: PhotoId,
        from: Option<String>,
        to: Option<String>,
    },
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
    /// A deleted snapshot, held so undo can put it back.
    ///
    /// ⚠ Carries the whole entry because nothing else holds it — `CopyDelete`'s shape and its
    /// reason. Boxed for the reason `Sync` is: an inline `Edits` would make **every** entry in
    /// the log that large, including the zero-sized `PhotoEdit`, which is by far the commonest.
    ///
    /// ⚠⚠ **Taking** a snapshot has no variant, deliberately. It follows `create_virtual_copy`,
    /// which pushes nothing while `delete_virtual_copy` does: creation is additive and reversed
    /// by deleting, deletion destroys the only record of a state. It is the same rule the design
    /// states — taking a snapshot is not an undo step, because it changes no `Edits`.
    SnapshotDelete {
        id: PhotoId,
        entry: Box<crate::sidecar::Snapshot>,
    },
    /// A snapshot renamed — `CopyRename`'s shape exactly, and scoped the same way.
    SnapshotRename {
        id: PhotoId,
        n: u32,
        from: Option<String>,
        to: Option<String>,
    },
    /// A catalog row was deleted — a collection, a smart collection, a keyword subtree, or a
    /// whole folder — held so undo can put it back.
    ///
    /// ⚠⚠ **Global, not scoped to a photo.** A collection or a keyword is not a property of any
    /// one photograph, and forgetting a folder may close the very library the deletion happened
    /// in — so there would be nothing left for `reachable` to compare against. `PresetDelete`'s
    /// rule, for `PresetDelete`'s reason.
    ///
    /// ⚠ Boxed for the reason `Sync` is: a `Folder` snapshot holds every photo row in the folder,
    /// and an inline variant would make **every** entry in the log that large, including the
    /// zero-sized `PhotoEdit`.
    CatalogDelete {
        snapshot: Box<catalog::snapshot::CatalogSnapshot>,
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
    /// A keyword renamed (M9.8). ⚠ Global, `CatalogDelete`'s rule: a keyword is not a property of
    /// any one photograph.
    KeywordRename { id: i64, from: String, to: String },
    /// A keyword moved to another parent (`None`: the top level).
    KeywordMove {
        id: i64,
        from: Option<i64>,
        to: Option<i64>,
    },
    /// One keyword merged into another. Boxed for `Sync`'s reason: the record can list many
    /// assignments, and an inline variant would make every entry in the log that large.
    KeywordMerge(Box<catalog::db::KeywordMerge>),
    /// A keyword put on (`on`) or taken off files (M10.8a). ⚠⚠ `files` are only the ones whose
    /// state **changed** — undoing a tag must not strip it from a photo that already had it.
    KeywordTag {
        keyword: i64,
        on: bool,
        files: Vec<PathBuf>,
    },
    /// A keyword created (M10.8a). Undo deletes it; redo puts it back under its own id, so a later
    /// entry naming that id still means it.
    KeywordCreate {
        id: i64,
        name: String,
        parent: Option<i64>,
    },
    /// Copies put into (`add`) or taken out of a collection (2026-10-01). ⚠⚠ `members` are only the
    /// copies whose membership **changed** — undoing an add must not take out one already in.
    CollectionChange {
        collection: i64,
        add: bool,
        members: Vec<PhotoId>,
    },
    /// Photos moved to another folder (2026-10-01): each file's `(from, to)`, as it happened. Undo
    /// moves them back, redo moves them again — the sidecars and the catalog rows with them.
    PhotoMove { moves: Vec<(PathBuf, PathBuf)> },
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
        ActionKind::PhotoEdit { id }
        | ActionKind::CopyRename { id, .. }
        | ActionKind::SnapshotDelete { id, .. }
        | ActionKind::SnapshotRename { id, .. } => open == Some(id),
        // ⚠ A deleted copy has no `PhotoId` left to be reachable *from*, so it is global like a
        // preset deletion rather than scoped like an edit.
        ActionKind::PresetDelete { .. }
        | ActionKind::Sync(_)
        | ActionKind::CopyDelete { .. }
        | ActionKind::CatalogDelete { .. }
        | ActionKind::KeywordRename { .. }
        | ActionKind::KeywordMove { .. }
        | ActionKind::KeywordMerge(_)
        | ActionKind::KeywordTag { .. }
        | ActionKind::KeywordCreate { .. }
        | ActionKind::CollectionChange { .. }
        | ActionKind::PhotoMove { .. }
        | ActionKind::Rename { .. } => true,
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

    /// Give `id` `undo` steps to undo and `redo` steps to redo, **older than anything already
    /// here** (M9.7).
    ///
    /// ⚠⚠ For a history restored from the catalog: its steps predate this session, so they go at
    /// the FRONT — in force, then undone — and Ctrl+Z reaches them only after this session's own.
    /// Without it the restored stack would sit unreachable: this log, not the stack, decides what
    /// an undo pops. Within the photo's subsequence the in-force entries still form a prefix and
    /// the undone ones a suffix, which is what `redo_index` relies on — the caller seeds only a
    /// photo with no entries here (`finalize_session` has just forgotten them).
    pub fn seed(&mut self, id: &PhotoId, undo: usize, redo: usize) {
        let entry = |undone| Entry {
            kind: ActionKind::PhotoEdit { id: id.clone() },
            undone,
        };
        let front: Vec<Entry> = (0..undo)
            .map(|_| entry(false))
            .chain((0..redo).map(|_| entry(true)))
            .collect();
        self.entries.splice(0..0, front);
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
    fn seeded_steps_come_after_this_sessions_and_redo_reaches_them() {
        let mut log = ActionLog::default();
        // This session: a preset deleted, then an edit to `a`.
        log.push(del("p"), a());
        log.push(edit("/a.RAF"), a());
        // `a`'s restored history: two to undo, one to redo.
        log.seed(&id("/a.RAF"), 2, 1);
        let order: Vec<_> = std::iter::from_fn(|| log.undo(a())).collect();
        assert_eq!(
            order,
            vec![edit("/a.RAF"), del("p"), edit("/a.RAF"), edit("/a.RAF")],
            "this session first, then the restored steps"
        );
        // Everything undone: redo replays from the oldest step, the restored ones included.
        assert_eq!(log.redo(a()), Some(edit("/a.RAF")));
    }

    #[test]
    fn a_new_edit_clears_a_seeded_redo() {
        let mut log = ActionLog::default();
        log.seed(&id("/a.RAF"), 1, 2);
        assert!(log.can_redo(a()));
        log.push(edit("/a.RAF"), a());
        assert!(
            !log.can_redo(a()),
            "a new edit invalidates the restored future"
        );
        assert!(!log.can_undo(b()), "and another photo cannot reach them");
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

    #[test]
    fn a_copy_rename_is_scoped_to_its_copy_unlike_a_file_rename() {
        // ⚠ The two renames differ deliberately. A **file** rename is global: after it the photo
        // is at a different path, so scoping it to the old one would make it unreachable the
        // moment it took effect. A **copy** rename leaves the file where it is, so it scopes to
        // its copy like any other per-photo action — and must *not* be reachable from the
        // original, or Ctrl+Z there would rename a copy the user is not looking at.
        use crate::photo_id::PhotoId;
        let path = std::path::PathBuf::from("/a.RAF");
        let original = PhotoId::original(path.clone());
        let one = PhotoId::new(path.clone(), 1);
        let two = PhotoId::new(path, 2);

        let mut log = ActionLog::default();
        log.push(
            ActionKind::CopyRename {
                id: one.clone(),
                from: None,
                to: Some("Warm".into()),
            },
            Some(&one),
        );
        assert!(
            log.undo(Some(&original)).is_none(),
            "the original must not reach a copy's rename"
        );
        assert!(
            log.undo(Some(&two)).is_none(),
            "nor must a sibling copy of the same file"
        );
        assert!(log.undo(Some(&one)).is_some(), "its own copy must");
    }

    #[test]
    fn a_file_rename_is_undoable_from_anywhere() {
        // ⚠ Global rather than photo-scoped: after the rename the photo is at a *different*
        // path, so scoping the entry to the old one would make it unreachable the moment it
        // took effect — the undo would be recorded and never usable.
        let mut log = ActionLog::default();
        log.push(
            ActionKind::Rename {
                from: PathBuf::from("/a.RAF"),
                to: PathBuf::from("/b.RAF"),
            },
            None,
        );
        assert!(log.undo(a()).is_some(), "reachable with a photo open");
        log.redo(a());
        assert!(log.undo(None).is_some(), "and with none open");
    }

    fn a_snapshot() -> Box<crate::sidecar::Snapshot> {
        Box::new(crate::sidecar::Snapshot {
            n: 1,
            name: None,
            taken: 0,
            edits: Edits::default(),
        })
    }

    /// ⚠ Scoped to its own photo, exactly as `CopyRename` is: Ctrl+Z while looking at another
    /// photo must not resurrect a snapshot you cannot see.
    #[test]
    fn a_snapshot_deletion_is_reachable_only_from_its_own_photo() {
        let mine = id("/a.RAF");
        let other = id("/b.RAF");
        let mut log = ActionLog::default();
        log.push(
            ActionKind::SnapshotDelete {
                id: mine.clone(),
                entry: a_snapshot(),
            },
            Some(&mine),
        );

        assert!(
            log.undo(Some(&other)).is_none(),
            "unreachable from another photo"
        );
        assert!(log.undo(Some(&mine)).is_some(), "reachable from its own");
    }

    /// ⚠ Scoped to the **copy**, not merely the file: copy 0 must not reach copy 1's rename.
    #[test]
    fn a_snapshot_rename_is_reachable_only_from_its_own_copy() {
        let mine = PhotoId::new(PathBuf::from("/a.RAF"), 1);
        let other = id("/a.RAF");
        let mut log = ActionLog::default();
        log.push(
            ActionKind::SnapshotRename {
                id: mine.clone(),
                n: 1,
                from: None,
                to: Some("Warm".to_string()),
            },
            Some(&mine),
        );

        assert!(
            log.undo(Some(&other)).is_none(),
            "copy 0 must not reach copy 1's snapshot rename"
        );
        assert!(log.undo(Some(&mine)).is_some());
    }

    fn catalog_delete(name: &str) -> ActionKind {
        ActionKind::CatalogDelete {
            snapshot: Box::new(catalog::snapshot::CatalogSnapshot::Collection {
                name: name.to_string(),
                created: 0,
                members: Vec::new(),
            }),
        }
    }

    /// ⚠⚠ **Global, and this is the test that matters.** Forgetting a folder can close the very
    /// library it happened in, and a collection belongs to no photograph — so scoped to a photo
    /// the undo would be unreachable exactly when it is wanted. Deleting from the Library grid
    /// with no photo open is the concrete case: `open` is `None`.
    #[test]
    fn a_catalog_deletion_is_undoable_from_anywhere() {
        let mut log = ActionLog::default();
        log.push(catalog_delete("Portfolio"), a());
        assert!(log.can_undo(b()), "reachable from another photo");
        assert!(log.can_undo(None), "and with no photo open at all");
        assert!(matches!(
            log.undo(None),
            Some(ActionKind::CatalogDelete { .. })
        ));
    }

    /// ⚠ A photo edit must not be able to *reach past* a catalog deletion and undo it out of
    /// order — the ordering is one chronological log, exactly as it is for a preset deletion.
    #[test]
    fn a_catalog_deletion_undoes_after_the_edit_that_followed_it() {
        let mut log = ActionLog::default();
        log.push(catalog_delete("Portfolio"), a());
        log.push(edit("/a.RAF"), a());
        assert!(matches!(log.undo(a()), Some(ActionKind::PhotoEdit { .. })));
        assert!(matches!(
            log.undo(a()),
            Some(ActionKind::CatalogDelete { .. })
        ));
    }
}

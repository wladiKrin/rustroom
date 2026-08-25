//! Per-photo undo histories that outlive their sessions.
//!
//! `History<Edits>` lives on `Session`, and a `Session` dies the moment another photo is
//! opened — so without this, editing photo A, moving to B and coming back leaves A with no
//! undo stack, even though its *edits* were restored faithfully from the sidecar. This is
//! where a history waits in between.
//!
//! Deliberately a **stash, not a store**: while a photo is open its `Session` still owns the
//! authoritative history. Making this authoritative instead would force every
//! `s.history.push` call site — each already holding `&mut self` through `self.session` — to
//! take a second simultaneous `&mut self` borrow, for no behavioural difference.

use crate::history::History;
use crate::photo_id::PhotoId;
use edit_model::Edits;
use std::collections::HashMap;

/// Undo histories for photos that are not currently open, keyed by source path.
///
/// Unbounded on purpose. A `History<Edits>` is a `Vec` of a few-hundred-byte struct — 500
/// photos at 20 steps each is roughly 4 MB — so `BufferCache`'s cap of 3, which exists
/// because a decoded buffer is 78–313 MB, has no bearing here.
#[derive(Default)]
pub struct HistoryStash {
    /// ⚠ Keyed by [`PhotoId`], not by path: two copies of one file are two photos with two undo
    /// stacks. `BufferCache` deliberately keys the other way — see its doc comment.
    map: HashMap<PhotoId, History<Edits>>,
}

impl HistoryStash {
    /// Keep `history` for `path`.
    ///
    /// A history with nothing to undo or redo is *removed* rather than stored: it carries
    /// nothing a fresh `History::new` would not, and dropping it keeps the map growing per
    /// photo actually edited rather than per photo visited. Removing rather than ignoring
    /// matters — otherwise resetting a photo whose stack was stashed earlier would bring the
    /// stale stack back on return.
    pub fn stash(&mut self, id: PhotoId, history: History<Edits>) {
        if !history.can_undo() && !history.can_redo() {
            self.map.remove(&id);
            return;
        }
        self.map.insert(id, history);
    }

    /// Take `path`'s history back, but **only if it still describes `on_disk`**.
    ///
    /// ⚠ This one check is what makes the feature safe against every writer of another
    /// photo's sidecar — a sync, a paste into a multi-selection, a preset click, and the undo
    /// or redo of any of them. Rather than obliging each of those, and every future one, to
    /// remember to invalidate, a stashed history that no longer matches disk is discarded
    /// here, at the single moment it would be used.
    ///
    /// `Edits` is `PartialEq` over `f32`, so a `NaN` compares unequal and this fails toward
    /// "discard and start fresh" — the safe direction, exactly as `PrefixKey` fails toward a
    /// cache miss.
    ///
    /// The entry is consumed either way: a history that did not match will not match later.
    pub fn take_matching(&mut self, id: &PhotoId, on_disk: &Edits) -> Option<History<Edits>> {
        let history = self.map.remove(id)?;
        (history.get() == on_disk).then_some(history)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn edits(exposure: f32) -> Edits {
        Edits {
            exposure,
            ..Default::default()
        }
    }

    /// A history holding one recorded step: from `edits(0.0)` to `edits(1.0)`.
    fn edited() -> History<Edits> {
        let mut h = History::new(edits(0.0));
        assert!(h.push(edits(1.0)), "the fixture must record a real step");
        h
    }

    #[test]
    fn a_stashed_history_comes_back_intact() {
        let mut stash = HistoryStash::default();
        stash.stash(PhotoId::original(PathBuf::from("/a.RAF")), edited());

        let mut back = stash
            .take_matching(&PhotoId::original(PathBuf::from("/a.RAF")), &edits(1.0))
            .expect("the sidecar still matches, so the history must come back");
        assert!(
            back.undo(),
            "the undo stack must have survived the round trip"
        );
        assert_eq!(*back.get(), edits(0.0));
        assert!(back.can_redo(), "and so must the redo stack");
    }

    #[test]
    fn a_history_whose_photo_changed_underneath_it_is_discarded() {
        // ⚠ The property the module hangs on. A sync or a paste into a multi-selection
        // rewrites the sidecars of photos that are *not open*, so the stack left behind
        // describes a state that is no longer on disk.
        let mut stash = HistoryStash::default();
        stash.stash(PhotoId::original(PathBuf::from("/a.RAF")), edited());
        assert!(stash
            .take_matching(&PhotoId::original(PathBuf::from("/a.RAF")), &edits(9.0))
            .is_none());
    }

    #[test]
    fn a_mismatched_history_is_consumed_rather_than_left_behind() {
        let mut stash = HistoryStash::default();
        stash.stash(PhotoId::original(PathBuf::from("/a.RAF")), edited());
        stash.take_matching(&PhotoId::original(PathBuf::from("/a.RAF")), &edits(9.0));
        assert!(stash.is_empty(), "it will not match later either");
    }

    #[test]
    fn an_unknown_photo_has_no_history() {
        let mut stash = HistoryStash::default();
        assert!(stash
            .take_matching(
                &PhotoId::original(PathBuf::from("/never-seen.RAF")),
                &edits(0.0)
            )
            .is_none());
    }

    #[test]
    fn a_history_with_nothing_to_undo_is_not_stashed() {
        let mut stash = HistoryStash::default();
        stash.stash(
            PhotoId::original(PathBuf::from("/a.RAF")),
            History::new(edits(0.0)),
        );
        assert!(
            stash.is_empty(),
            "it carries nothing a fresh History::new would not"
        );
    }

    #[test]
    fn stashing_an_empty_history_replaces_an_older_one() {
        // Otherwise "Reset all" on a photo visited earlier would resurrect the old stack
        // on return, with steps that no longer lead anywhere.
        let mut stash = HistoryStash::default();
        stash.stash(PhotoId::original(PathBuf::from("/a.RAF")), edited());
        stash.stash(
            PhotoId::original(PathBuf::from("/a.RAF")),
            History::new(edits(1.0)),
        );
        assert!(stash.is_empty());
    }

    #[test]
    fn photos_keep_separate_histories() {
        let mut stash = HistoryStash::default();
        stash.stash(PhotoId::original(PathBuf::from("/a.RAF")), edited());
        stash.stash(PhotoId::original(PathBuf::from("/b.RAF")), edited());
        assert_eq!(stash.len(), 2);
        assert!(stash
            .take_matching(&PhotoId::original(PathBuf::from("/a.RAF")), &edits(1.0))
            .is_some());
        assert!(stash
            .take_matching(&PhotoId::original(PathBuf::from("/b.RAF")), &edits(1.0))
            .is_some());
    }

    #[test]
    fn two_copies_of_one_file_keep_separate_stacks() {
        // ⚠ The reason the stash re-keys. Keyed by path, editing a copy would hand you the
        // original's undo stack, and `take_matching`'s equality check would then silently
        // discard whichever stack lost.
        use crate::photo_id::PhotoId;
        let mut s = HistoryStash::default();
        let path = std::path::PathBuf::from("/x/a.RAF");

        let mut h0 = History::new(Edits::default());
        h0.push(Edits {
            exposure: 1.0,
            ..Edits::default()
        });
        let mut h1 = History::new(Edits::default());
        h1.push(Edits {
            exposure: 2.0,
            ..Edits::default()
        });

        s.stash(PhotoId::original(path.clone()), h0);
        s.stash(PhotoId::new(path.clone(), 1), h1);
        assert_eq!(s.len(), 2, "one stack per copy, not per file");

        let got = s
            .take_matching(
                &PhotoId::new(path, 1),
                &Edits {
                    exposure: 2.0,
                    ..Edits::default()
                },
            )
            .expect("copy 1's stack");
        assert_eq!(got.get().exposure, 2.0);
    }
}

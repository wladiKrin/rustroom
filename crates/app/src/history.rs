//! Generic undo/redo history over snapshots of a value.

/// Holds a current value plus undo/redo stacks of prior snapshots.
pub struct History<T> {
    current: T,
    undo: Vec<T>,
    redo: Vec<T>,
}

impl<T: Clone + PartialEq> History<T> {
    pub fn new(initial: T) -> Self {
        Self {
            current: initial,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn get(&self) -> &T {
        &self.current
    }

    /// Record `value` as a new distinct step. No-op if equal to the current value.
    /// Clears the redo stack.
    ///
    /// **Returns whether a step was actually recorded.** Callers that mirror this history
    /// in an ordering elsewhere — see `action_log` — must not record a step that this
    /// silently dropped, or the two desynchronize and an undo pops the wrong stack.
    pub fn push(&mut self, value: T) -> bool {
        if value == self.current {
            return false;
        }
        self.undo.push(self.current.clone());
        self.current = value;
        self.redo.clear();
        true
    }

    /// Rewrite the current step in place, recording nothing — for a change that is not an edit (M10.12c's
    /// migration of old marks into decode space, which renders the same picture). Undo and redo are untouched.
    pub fn replace_current(&mut self, value: T) {
        self.current = value;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back. Returns true if something changed.
    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.current, prev));
            true
        } else {
            false
        }
    }

    /// How many entries the undo side holds — the changes applied since the initial state
    /// that are still in the present.
    ///
    /// ⚠⚠ **This is a count of entries, not a measure of difference.** A value set and then set back
    /// leaves two entries with nothing changed, so anything answering "has this changed at all?"
    /// must compare against [`Self::initial`] instead. The snapshot strip read "2 changes so far" on
    /// a photo that was back where it opened, because it asked this.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// The value this history started from.
    ///
    /// ⚠ `undo[0]` is by construction the value passed to [`Self::new`] — every `push` appends the
    /// *previous* present, so the oldest entry is the original. With an empty undo side the present
    /// **is** the original, which is exactly the state after undoing everything.
    pub fn initial(&self) -> &T {
        self.undo.first().unwrap_or(&self.current)
    }

    /// The undo side, oldest first — for persisting (M9.7).
    pub fn undo_steps(&self) -> &[T] {
        &self.undo
    }

    /// The redo side, the next redo LAST — for persisting (M9.7).
    pub fn redo_steps(&self) -> &[T] {
        &self.redo
    }

    /// How many steps the redo side holds.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Rebuild a history from its parts, in [`Self::undo_steps`] / [`Self::redo_steps`] order.
    ///
    /// ⚠ No `PartialEq` pruning: the parts are taken as recorded, which is what a restored history
    /// must be — a step that happened stays a step, even one that set a value back.
    pub fn from_parts(undo: Vec<T>, current: T, redo: Vec<T>) -> Self {
        Self {
            current,
            undo,
            redo,
        }
    }

    /// Step forward. Returns true if something changed.
    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.current, next));
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {

    /// ⚠⚠ The state a history **started from**, which is what "has this photo changed at all?" has to
    /// be asked against. `undo_depth` counts *entries*, so a value set and then set back reads as two
    /// changes when nothing differs — the snapshot strip said "2 changes so far" on a photo that was
    /// back where it opened.
    #[test]
    fn the_initial_value_survives_pushes_and_undos() {
        let mut h = History::new(7i32);
        assert_eq!(*h.initial(), 7, "with no pushes it is the present");
        h.push(8);
        h.push(9);
        assert_eq!(*h.initial(), 7, "and is not buried by later pushes");
        h.undo();
        assert_eq!(*h.initial(), 7);
        h.undo();
        assert_eq!(
            *h.initial(),
            7,
            "undone all the way back, the present IS the initial value"
        );
        h.redo();
        assert_eq!(*h.initial(), 7);
    }

    /// ⚠ The case the snapshot strip got wrong: back at the start, with a deep history behind it.
    #[test]
    fn returning_to_the_initial_value_is_visible_even_with_a_deep_history() {
        let mut h = History::new(0i32);
        h.push(5);
        h.push(0);
        assert_eq!(h.undo_depth(), 2, "two entries were pushed");
        assert_eq!(
            h.get(),
            h.initial(),
            "but the present is back where it started"
        );
    }

    use super::*;

    #[test]
    fn push_undo_redo_cycle() {
        let mut h = History::new(0i32);
        h.push(1);
        h.push(2);
        assert_eq!(*h.get(), 2);
        assert!(h.undo());
        assert_eq!(*h.get(), 1);
        assert!(h.undo());
        assert_eq!(*h.get(), 0);
        assert!(!h.undo());
        assert!(h.redo());
        assert_eq!(*h.get(), 1);
    }

    #[test]
    fn push_equal_is_noop() {
        let mut h = History::new(5i32);
        assert!(!h.push(5), "an unchanged value must report no step");
        assert!(!h.can_undo());
        assert!(h.push(6), "a changed value must report a step");
    }

    #[test]
    fn push_clears_redo() {
        let mut h = History::new(0i32);
        h.push(1);
        h.undo();
        assert!(h.can_redo());
        h.push(9);
        assert!(!h.can_redo());
        assert_eq!(*h.get(), 9);
    }
}

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

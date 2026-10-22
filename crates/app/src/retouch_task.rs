//! What the Retouch tool is doing (M10.19d, the author's option B): **Paint** — a drag on the photo retouches with
//! the brush — or **Find** — people or dust are looked for, then picked or added to on the photo. The app's, beside
//! the brush tools; remembered for the run; no egui.

/// What a drag on the photo does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Task {
    /// The brush retouches.
    #[default]
    Paint,
    /// What was found is picked, or added to.
    Find,
}

/// What Find looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FindKind {
    #[default]
    People,
    Dust,
}

/// What the photo does while a find is reviewed — the switch that makes drawing to look again visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gesture {
    /// A click picks or unpicks one.
    #[default]
    Pick,
    /// A drawing looks again there, closer, and adds what it finds.
    Add,
}

/// The Retouch tool's task, what it finds and the photo's gesture — Pick again after every search lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetouchState {
    pub task: Task,
    pub find: FindKind,
    pub gesture: Gesture,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_starts_painting_people_picking() {
        let s = RetouchState::default();
        assert_eq!(
            (s.task, s.find, s.gesture),
            (Task::Paint, FindKind::People, Gesture::Pick)
        );
    }
}

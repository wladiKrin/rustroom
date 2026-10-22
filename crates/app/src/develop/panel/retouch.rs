//! The Retouch panel (M10.19d, the author's option B of `https://claude.ai/artifact/7ACHJJ2zFpWPJ3WspdRYcC`):
//! **Paint · Find** at the top decides what the photo does; under it the brush (Paint) or the Find card (Find); under
//! both, the one list of every retouch with the selected one's settings.
//!
//! ⚠ Composition only: the brush is `retouch_brush`, finding `retouch_find`, the list `retouch_items`, the eye
//! settings `retouch_eyes`.

use super::{retouch_brush, retouch_find, retouch_items, Edited};
use crate::segmented::{Mark, Segment};
use crate::Session;
use eframe::egui;
use rustroom_app::eye_tool::EyeTool;
use rustroom_app::icons;
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::retouch_task::{RetouchState, Task};
use rustroom_app::spot_tool::SpotTool;

const PAINT_HOVER: &str = "A drag on the photo retouches it with the brush";
const FIND_HOVER: &str = "Finds people or dust, to pick on the photo the ones to remove";

/// Draw the Retouch controls.
///
/// ⚠ The tools are `&mut` to fields of `RustroomApp` while `s` borrows another — **disjoint** fields, which is what
/// makes both legal (the `masks::BrushTool` precedent).
pub(crate) fn show_with(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    remove: &mut RemoveTool,
    state: &mut RetouchState,
    ed: &mut Edited,
) {
    // ⚠ Once per frame, before anything reads them: undo, paste, snapshots and the canvas all change the counts
    // without passing through this panel.
    s.sync_spot_flags();
    s.removal.sync(s.edits.removals.len());
    let font = egui::TextStyle::Body.resolve(ui.style());
    crate::segmented::segmented_marked(
        ui,
        &mut state.task,
        &[
            Segment {
                value: Task::Paint,
                label: "Paint",
                hover: PAINT_HOVER,
                mark: Mark::Icon(icons::PAINT),
            },
            Segment {
                value: Task::Find,
                label: "Find",
                hover: FIND_HOVER,
                mark: Mark::Icon(icons::FIND),
            },
        ],
        28.0,
        font,
    );
    ui.add_space(6.0);
    match state.task {
        Task::Paint => retouch_brush::show(ui, s, tool, eyes, remove, ed),
        Task::Find => retouch_find::show(ui, s, tool, eyes, remove, state, ed),
    }
    ui.add_space(6.0);
    retouch_items::show(ui, s, tool, eyes, remove, state, ed);
}

#[cfg(test)]
pub(crate) mod tests;

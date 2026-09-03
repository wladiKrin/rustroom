//! Gestures that are neither a panel slider nor a canvas drag: choosing a rail tool,
//! rotating, arrowing through the filmstrip, and solving the drawn perspective guides.
//!
//! Split out of `main.rs` unchanged.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::format_filter;
use rustroom_app::selection::Selection;
use rustroom_app::tool::Tool;

/// Every bare culling key, and the egui key it is pressed with.
///
/// ⚠ **One table, three views.** The grid, Compare and Survey all cull, and three copies of
/// this list is how two of them come to disagree about whether `9` is a colour label.
const CULLING_KEYS: [(char, egui::Key); 13] = [
    ('0', egui::Key::Num0),
    ('1', egui::Key::Num1),
    ('2', egui::Key::Num2),
    ('3', egui::Key::Num3),
    ('4', egui::Key::Num4),
    ('5', egui::Key::Num5),
    ('6', egui::Key::Num6),
    ('7', egui::Key::Num7),
    ('8', egui::Key::Num8),
    ('9', egui::Key::Num9),
    ('p', egui::Key::P),
    ('x', egui::Key::X),
    ('u', egui::Key::U),
];

/// The culling action this frame's keys ask for, if any.
///
/// ⚠⚠ **No command, control or alt — but Shift is allowed, and that was a real bug.** ⌘X is cut
/// and ⌘A is select-all, so those must not cull. Shift is different: `Shift+X` is how anyone
/// types the capital `X` the on-screen hints show, and blocking every modifier meant the key the
/// hint named did nothing. Reported from Survey, where the hint reads "X rejects".
///
/// ⚠ The physical key is the same either way — `Key::X` is `Key::X` — so allowing Shift changes
/// no other binding.
///
/// ⚠ The caller must still gate on `memory().focused()` — this reads raw key state and knows
/// nothing about the search field or the rename field a digit might be typed into.
pub(crate) fn culling_key(i: &egui::InputState) -> Option<rustroom_app::marks::MarkAction> {
    if i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt {
        return None;
    }
    CULLING_KEYS
        .iter()
        .find(|(_, key)| i.key_pressed(*key))
        .and_then(|(c, _)| rustroom_app::marks::action_for(*c))
}

/// Apply a click to the selection according to its modifiers. Returns whether this was a
/// plain click, which is the only gesture that moves the active photo — and therefore the
/// only one the filmstrip turns into an open.
pub(crate) fn apply_click(sel: &mut Selection, i: usize, mods: egui::Modifiers) -> bool {
    if mods.command {
        sel.toggle(i);
        false
    } else if mods.shift {
        sel.extend_to(i);
        false
    } else {
        sel.click(i);
        true
    }
}

impl RustroomApp {
    /// Select a rail tool, re-rendering when it actually changed.
    ///
    /// ⚠ The re-render is not optional and it lives here rather than at each call site:
    /// `preview_edits` strips the crop in geometry mode, so entering or leaving Crop without a
    /// request leaves the overlay drawn over a *cropped* preview — the M3.1a
    /// `draw_mask_overlay` bug. The rail, the 1–4 keys and the Photo menu all come through
    /// here, so none of them can forget it.
    pub(crate) fn select_tool(&mut self, t: Tool) {
        if self.tool == t {
            return;
        }
        self.tool = t;
        let tool = self.tool;
        if let Some(s) = &mut self.session {
            // ⚠ A tool change can interrupt a retouch drag, and neither the canvas nor the panel
            // gets its release frame in that case — so a suppressed spot would stay invisible with
            // nothing on screen to explain it. Cleared here, where every route into a tool change
            // already converges.
            s.suppressed_spot = None;
            // ⚠ Visualize Spots turns itself off here too. A canvas stuck in a high-contrast grey
            // view after switching to Edit reads as a broken render, and it is the single most
            // likely thing in the Retouch tool to be reported as a bug. The request below is
            // what brings the photograph back.
            //
            // ⚠ Unconditional, deliberately: the view can only be switched on from the Retouch
            // panel, so every tool change that reaches this line is a change *away* from it. A
            // `t != Tool::Retouch` guard was tried and its sabotage went uncaught — an
            // unreachable guard is an untested one.
            s.visualize = None;
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        }
    }

    /// Turn the photo a quarter turn, moving the crop with it.
    ///
    /// ⚠ The `CropRect` transform stays in the same function as the orientation change. They
    /// are one act: separating them is exactly how a rotated crop lands on different pixels.
    pub(crate) fn rotate(&mut self, clockwise: bool) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        let g = &mut s.edits.geometry;
        if clockwise {
            g.orientation.rotate_cw();
            g.crop = g.crop.rotated_cw();
        } else {
            g.orientation.rotate_ccw();
            for _ in 0..3 {
                g.crop = g.crop.rotated_cw();
            }
        }
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        if s.history.push(s.edits.clone()) {
            if let Some(path) = self.open_photo() {
                self.actions
                    .push(ActionKind::PhotoEdit { id: path.clone() }, Some(&path));
            }
        }
    }

    /// ←/→ move to the previous/next photo in the loaded folder — but only when no egui
    /// widget has keyboard focus, so arrows still fine-tune a focused slider.
    pub(crate) fn handle_develop_arrows(&mut self, ctx: &egui::Context) {
        let nav = {
            let Some(lib) = &self.library else { return };
            if lib.visible.is_empty() || ctx.memory(|m| m.focused().is_some()) {
                return;
            }
            // ⚠ `cur` may not itself be visible: hiding the open photo's format does not close
            // it, so `step` has to cope with starting from a hidden index.
            let cur = lib.selection.active().unwrap_or(0);
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    format_filter::step(&lib.visible, cur, true)
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    format_filter::step(&lib.visible, cur, false)
                } else {
                    None
                }
            })
        };
        if let Some(i) = nav {
            if self.library.as_ref().and_then(|l| l.selection.active()) != Some(i) {
                self.open_entry(ctx, i);
            }
        }
    }

    /// Solve the drawn guides and write the answer into the sliders.
    ///
    /// ⚠⚠ **The guides are the input and the sliders are the output — there is no second stored
    /// representation.** Afterwards the `Edits` are indistinguishable from the same values
    /// dialled in by hand, which is the whole point of the chosen model.
    ///
    /// Returns whether anything changed, so the caller records exactly one history entry — the
    /// contract `draw_crop_overlay` already has.
    pub(crate) fn solve_guides(&mut self) -> bool {
        use pipeline::perspective::guides::{self, Family};
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let of = |f: Family| -> Vec<guides::Guide> {
            // ⚠ At most two per family: the solve is exactly determined, not a least-squares fit.
            s.guides
                .iter()
                .copied()
                .filter(|g| g.family() == f)
                .take(2)
                .collect()
        };
        // ⚠⚠ The **frame** aspect, not the sensor's: guides are in `[0, 1]` of the displayed
        // frame, and `solve` converts them to the centred, half-height units the maths needs.
        // Getting that conversion wrong is what made an earlier version merely plausible.
        let aspect = s.fit_image_size.x / s.fit_image_size.y.max(1.0);
        let (pitch, yaw, roll) =
            guides::solve(&of(Family::Vertical), &of(Family::Horizontal), aspect);
        let to_slider = |rad: f32| {
            (rad.to_degrees() / pipeline::perspective::MAX_ANGLE_DEG * 100.0).clamp(-100.0, 100.0)
        };
        let before = (s.edits.perspective, s.edits.geometry.angle);
        s.edits.perspective.vertical = to_slider(pitch);
        s.edits.perspective.horizontal = to_slider(yaw);
        s.edits.geometry.angle = roll.to_degrees().clamp(-45.0, 45.0);
        (s.edits.perspective, s.edits.geometry.angle) != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(modifiers: egui::Modifiers) -> Option<rustroom_app::marks::MarkAction> {
        let ctx = egui::Context::default();
        let mut got = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::X,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
                ..Default::default()
            },
            |ui| {
                got = ui.input(culling_key);
            },
        );
        got
    }

    /// ⚠⚠ **Shift must not block a culling key, and nothing covered this until a sabotage
    /// restoring `modifiers.any()` went uncaught.** `Shift+X` is how anyone types the capital `X`
    /// the on-screen hints show, so blocking every modifier meant the key the hint named did
    /// nothing at all — reported from Survey, where the hint reads "X rejects".
    #[test]
    fn shift_does_not_block_a_culling_key() {
        let want = Some(rustroom_app::marks::MarkAction::Flag(
            catalog::db::Flag::Reject,
        ));
        assert_eq!(keys(egui::Modifiers::NONE), want, "bare x did not cull");
        assert_eq!(keys(egui::Modifiers::SHIFT), want, "Shift+X did not cull");
    }

    /// ⚠ And the modifiers that genuinely mean something else still do: ⌘X is cut, ⌘A is
    /// select-all. A culling key is bare-or-shifted, or it is not a culling key.
    #[test]
    fn the_editing_modifiers_still_block_a_culling_key() {
        for m in [
            egui::Modifiers::COMMAND,
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
        ] {
            assert_eq!(keys(m), None, "{m:?} was treated as a culling key");
        }
    }

    /// ⚠⚠ **Two guides per family are required, and the sabotage step is what forced this test.**
    /// A single line has no vanishing point at all, so one guide must leave the edits alone rather
    /// than solving from nothing — and nothing covered that until a sabotage making one guide
    /// sufficient went **uncaught**.
    #[test]
    fn one_guide_solves_nothing_and_two_solve_something() {
        use pipeline::perspective::guides::Guide;
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);

        let push = |app: &mut RustroomApp, from: [f32; 2], to: [f32; 2]| {
            app.session
                .as_mut()
                .unwrap()
                .guides
                .push(Guide { from, to });
        };
        // One converging vertical: no vanishing point, so nothing to solve.
        push(&mut app, [0.2, 0.0], [0.35, 1.0]);
        assert!(!app.solve_guides(), "one guide must not change the edits");
        assert_eq!(
            app.session.as_ref().unwrap().edits.perspective,
            edit_model::Perspective::default()
        );

        // A second, converging on the first, gives one.
        push(&mut app, [0.8, 0.0], [0.65, 1.0]);
        assert!(app.solve_guides(), "two guides must solve");
        assert_ne!(
            app.session.as_ref().unwrap().edits.perspective.vertical,
            0.0,
            "the vertical slider should have moved"
        );
    }

    /// ⚠⚠ **Guides survive a trip to another tool and back.** They live on `Session`, which is
    /// per-photo, while `Tool` lives on `RustroomApp` — so nothing in a tool switch can touch
    /// them. Asserted rather than assumed, because "it happens to work" and "it is guaranteed"
    /// look identical until someone adds a reset.
    #[test]
    fn guides_survive_switching_tools() {
        use pipeline::perspective::guides::Guide;
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);

        app.tool = rustroom_app::tool::Tool::Crop;
        let s = app.session.as_mut().unwrap();
        s.show_guides = true;
        s.guides.push(Guide {
            from: [0.2, 0.0],
            to: [0.35, 1.0],
        });

        app.tool = rustroom_app::tool::Tool::Edit;
        app.tool = rustroom_app::tool::Tool::Masks;
        app.tool = rustroom_app::tool::Tool::Crop;

        let s = app.session.as_ref().unwrap();
        assert_eq!(s.guides.len(), 1, "the guide should still be there");
        assert!(s.show_guides, "and the sub-mode should still be on");
    }
}

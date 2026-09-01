//! Gestures that are neither a panel slider nor a canvas drag: choosing a rail tool,
//! rotating, arrowing through the filmstrip, and solving the drawn perspective guides.
//!
//! Split out of `main.rs` unchanged.

use crate::*;

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

//! Where the canvas meets decode-space marks (M10.12c): the session's [`Anchoring`], the spots, eyes and strokes as
//! their stages see them, and old marks moved into decode space once the photo's lens is known.
//!
//! ⚠ The overlays work in the stages' own spaces — the maps, the hit tests and the auto-source sampler are all
//! written there — on [`Session::anchored_view`]; every gesture stores what it made through
//! `pipeline::anchors::{anchor_spot, anchor_eye}` or `Anchoring::stroke_to_decode`.

use crate::Session;
use pipeline::anchors::{self, Anchoring};

impl Session {
    /// The maps between decode space and the stages' spaces, for this photo's lens and edits. Cheap: the lens field
    /// is memoised in `pipeline::anchors`.
    pub(crate) fn anchoring(&self) -> Anchoring {
        Anchoring::for_edits(
            self.lens.shot.as_ref(),
            &self.edits,
            self.buffer.width(),
            self.buffer.height(),
        )
    }

    /// The edits with every mark in its stage's space — what the overlays draw and hit-test.
    pub(crate) fn anchored_view(&self) -> edit_model::Edits {
        anchors::resolve(&self.edits, &self.anchoring()).into_owned()
    }

    /// Move the photo's marks from before M10.12c into decode space, keeping where they render. No undo step: the
    /// picture does not change, and an older history entry's marks still render where they did.
    ///
    /// ⚠ Never from a ghost session (`buffer_is_ghost`): its lens facts may not be in yet, and with no lens a mark
    /// would be stored as if the photo had none.
    pub(crate) fn migrate_anchors(&mut self) -> bool {
        if self.buffer_is_ghost || !anchors::has_legacy(&self.edits) {
            return false;
        }
        let a = self.anchoring();
        let was = self.edits.clone();
        let changed = anchors::migrate(&mut self.edits, &a);
        // ⚠ The step on screen is the same picture, so it is rewritten rather than pushed: a push would be an undo
        // step that changes nothing, and leaving it stale would make the next no-op gesture record one.
        if changed && *self.history.get() == was {
            self.history.replace_current(self.edits.clone());
        }
        changed
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use eframe::egui;

    /// Constructed: the pipeline's synthetic barrel lens (`ptlens` b = 0.05).
    pub(crate) fn lens() -> lens_profile::LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        lens_profile::LensShot {
            profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    /// A photo open in Develop with a lens, its correction on, and one patch placed before M10.12c.
    pub(crate) fn lens_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 60);
        let s = app.session.as_mut().unwrap();
        s.lens.shot = Some(lens());
        s.edits.lens_correction = Some(edit_model::LensCorrection {
            distortion: true,
            ..Default::default()
        });
        app
    }

    /// Opening a photo moves its old marks into decode space — as the SAME step, rendering where they did —
    /// through Develop's body, every frame.
    #[test]
    fn develop_moves_old_marks_into_decode_space_as_the_same_step() {
        let ctx = egui::Context::default();
        let mut app = lens_app(&ctx);
        let s = app.session.as_mut().unwrap();
        s.edits.spots = vec![edit_model::Spot {
            centre: [0.1, 0.12],
            source: [0.3, 0.12],
            radius: 0.03,
            feather: 50.0,
            mode: edit_model::SpotMode::Heal,
            dust: false,
            decode: false,
        }];
        assert!(s.history.push(s.edits.clone()));
        let (depth, old) = (s.history.undo_depth(), s.edits.spots[0]);
        let c = ctx.clone();
        let _ = ctx.run_ui(Default::default(), |ui| {
            crate::develop::body(&mut app, ui, &c)
        });
        let s = app.session.as_mut().unwrap();
        let now = s.edits.spots[0];
        assert!(now.decode, "the patch was not moved into decode space");
        assert!(
            (now.centre[0] - old.centre[0]).abs() > 1e-3,
            "the fixture's lens must move a corner patch: {now:?}"
        );
        let back = s.anchored_view().spots[0];
        assert!(
            (back.centre[0] - old.centre[0]).abs() < 1e-5
                && (back.centre[1] - old.centre[1]).abs() < 1e-5
        );
        assert_eq!(s.history.undo_depth(), depth, "a migration is no undo step");
        assert_eq!(
            s.history.get(),
            &s.edits,
            "history's present is the migrated edits"
        );
        assert!(!s.migrate_anchors(), "once only");
    }
}

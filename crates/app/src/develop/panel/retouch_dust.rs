//! The Retouch panel's Remove dust (M10.16c2): the button and its Sensitivity, beside the Find dust view.
//!
//! ⚠ Its own file, `retouch_eyes`'s reason: `retouch.rs` sits at the line ceiling.

use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::SpotTool;

/// The line under the button while it looks.
const LOOKING_HOVER: &str =
    "Each spot of sensor dust it finds becomes a Heal patch you can move or delete";

/// Remove dust, what it is doing or what it did, and its Sensitivity (the author, 2026-10-20: a press
/// "often" seemed to do nothing). The search and its patches are `dust_flow`'s.
pub(super) fn show(ui: &mut egui::Ui, s: &mut Session, tool: &mut SpotTool) {
    ui.add_space(6.0);
    let looking = s.dust.search.is_some();
    if ui
        .add_enabled(!looking, egui::Button::new("Remove dust"))
        .hover("Finds sensor dust in the photo and heals each spot with a patch")
        .clicked()
    {
        s.press_remove_dust(tool.dust);
    }
    if let Some(search) = &s.dust.search {
        super::progress_line::show(ui, "Looking for dust", search.progress(), LOOKING_HOVER);
    } else if s.dust.queued.is_some() {
        theme::hint(ui, "Waits for the photo to load");
    } else if let Some(said) = &s.dust.said {
        theme::hint(ui, said);
    }
    ui.add(
        super::slider::Slider::new(&mut tool.dust, 0.0..=100.0)
            .text("Sensitivity")
            .help(slider_help::text(Scope::Patch, "Sensitivity"))
            .reset_to(rustroom_app::dust_tool::DEFAULT_SENSITIVITY),
    );
}

#[cfg(test)]
mod tests {
    use crate::develop::panel::{retouch, Edited};

    /// A 1600 × 1200 grey-blue frame with three soft dark discs — dust on a smooth sky.
    fn dusty() -> core_image::ImageBuf {
        let (w, h) = (1600, 1200);
        let discs = [
            (300.0f32, 300.0f32, 8.0f32, 0.12f32),
            (900.0, 500.0, 10.0, 0.10),
            (1300.0, 900.0, 6.0, 0.15),
        ];
        let data = (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let mut v = 0.3 + 0.3 * x / w as f32;
                for &(cx, cy, r, d) in &discs {
                    let rho = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                    v *= 1.0 - d / (1.0 + ((rho - r) / (0.25 * r)).exp());
                }
                [0.8 * v, 0.9 * v, v]
            })
            .collect();
        core_image::ImageBuf::from_vec(w, h, data)
    }

    struct Panel {
        ctx: eframe::egui::Context,
        app: crate::RustroomApp,
    }

    impl Panel {
        fn new(buffer: core_image::ImageBuf) -> Self {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
            app.session.as_mut().unwrap().buffer = std::sync::Arc::new(buffer);
            Self { ctx, app }
        }

        fn draw(&mut self) -> impl FnMut(&mut eframe::egui::Ui) + '_ {
            // ⚠ The app's own tools — Remove, Retouch's default mode, included: where the author pressed it.
            |ui| {
                let (mut t, mut e, mut r) =
                    (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool);
                let mut ed = Edited::default();
                retouch::show_with(
                    ui,
                    self.app.session.as_mut().unwrap(),
                    &mut t,
                    &mut e,
                    &mut r,
                    &mut ed,
                );
                (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool) = (t, e, r);
            }
        }

        fn click(&mut self, label: &str) {
            let ctx = self.ctx.clone();
            crate::test_ui::click(&ctx, label, self.draw());
        }

        /// Frames until the search has landed — `true` when its patches were one undo step.
        fn settle(&mut self) -> bool {
            let start = std::time::Instant::now();
            let mut pushed = false;
            loop {
                pushed |= self.app.pump_dust(&self.ctx);
                let d = &self.session().dust;
                if d.search.is_none() && d.queued.is_none() {
                    return pushed;
                }
                assert!(start.elapsed().as_secs() < 120, "the search never landed");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        fn labels(&mut self) -> Vec<String> {
            let ctx = self.ctx.clone();
            crate::test_ui::nodes(&ctx, self.draw())
                .into_iter()
                .map(|n| n.label)
                .collect()
        }

        fn session(&self) -> &crate::Session {
            self.app.session.as_ref().unwrap()
        }

        fn spots(&self) -> Vec<edit_model::Spot> {
            self.session().edits.spots.clone()
        }

        fn said(&self) -> Option<&str> {
            self.session().dust.said.as_deref()
        }
    }

    /// Review Focus 5: one press, the dust healed — Heal patches, one undo step, the count said under the
    /// button and on the status row.
    #[test]
    fn remove_dust_adds_heal_patches_as_one_step() {
        let mut p = Panel::new(dusty());
        let depth = p.session().history.undo_depth();
        p.click("Remove dust");
        assert!(p.settle(), "one undo step");
        let spots = p.spots();
        assert_eq!(spots.len(), 3, "{spots:?}");
        assert!(spots.iter().all(|s| s.mode == edit_model::SpotMode::Heal));
        assert_eq!(p.session().history.undo_depth(), depth + 1);
        assert_eq!(p.said(), Some("Removed 3 dust spots"));
        assert_eq!(p.app.status, "Removed 3 dust spots");
        assert!(
            p.labels().iter().any(|l| l == "Removed 3 dust spots"),
            "said under the button"
        );
    }

    /// The author (2026-10-20): "I can't see the patches" — pressed in Remove, the mode Retouch opens in, the
    /// new patches were drawn nowhere. Once it adds some, Retouch shows the patches.
    #[test]
    fn remove_dust_shows_the_patches_it_made() {
        let mut p = Panel::new(dusty());
        assert!(p.app.remove_tool.active, "the fixture: pressed in Remove");
        p.click("Remove dust");
        p.settle();
        assert_eq!(p.spots().len(), 3);
        assert!(
            !p.app.remove_tool.active,
            "still in Remove, the patches unseen"
        );
        assert_eq!(p.app.eye_tool.active, None);
    }

    /// The author (2026-10-20): "if it's not finding dust, it would be good to get a status message saying
    /// so" — and it stays under the button.
    #[test]
    fn a_photo_without_dust_says_so() {
        let mut p = Panel::new(core_image::ImageBuf::from_vec(
            1600,
            1200,
            vec![0.4; 1600 * 1200 * 3],
        ));
        p.click("Remove dust");
        assert!(!p.settle(), "no step for nothing");
        assert!(p.spots().is_empty());
        assert_eq!(p.said(), Some("No dust found"));
        assert!(p.labels().iter().any(|l| l == "No dust found"));
    }

    #[test]
    fn pressing_remove_dust_twice_adds_nothing_more() {
        let mut p = Panel::new(dusty());
        p.click("Remove dust");
        p.settle();
        p.click("Remove dust");
        assert!(!p.settle(), "no step for nothing");
        assert_eq!(p.spots().len(), 3);
        assert_eq!(p.said(), Some("No new dust found"));
    }

    /// "Or if it's still looking for anything a progress bar": while it looks, the line says so — and the
    /// button waits, so a second press cannot start a second search over the first.
    #[test]
    fn while_it_looks_the_line_says_so_and_the_button_waits() {
        let mut p = Panel::new(dusty());
        let (search, answer) = rustroom_app::dust_tool::Search::answered_by(0.4);
        p.app.session.as_mut().unwrap().dust.search = Some(search);
        assert!(p.labels().iter().any(|l| l == "Looking for dust"));
        p.click("Remove dust");
        answer.send(Vec::new()).unwrap();
        p.settle();
        assert!(p.spots().is_empty(), "a second search ran: {:?}", p.spots());
        assert_eq!(p.said(), Some("No dust found"));
        assert!(!p.labels().iter().any(|l| l == "Looking for dust"));
    }

    /// Review Focus 4: on the stand-in a photo shows while it loads, nothing is looked for — the press waits,
    /// says so, and is answered once the photo is there.
    #[test]
    fn remove_dust_waits_for_the_photo() {
        let mut p = Panel::new(core_image::ImageBuf::from_vec(
            400,
            300,
            vec![0.4; 400 * 300 * 3],
        ));
        p.click("Remove dust");
        p.app.pump_dust(&p.ctx.clone());
        assert!(p.spots().is_empty());
        assert!(p
            .labels()
            .iter()
            .any(|l| l == "Waits for the photo to load"));
        p.app.session.as_mut().unwrap().buffer = std::sync::Arc::new(dusty());
        p.settle();
        assert_eq!(p.spots().len(), 3);
    }
}

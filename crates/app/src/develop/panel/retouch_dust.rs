//! The Retouch panel's Remove dust (M10.16c2): the button and its Sensitivity, beside the Find dust view.
//!
//! ⚠ Its own file, `retouch_eyes`'s reason: `retouch.rs` sits at the line ceiling.

use super::Edited;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::SpotTool;

/// Below this long side the decode is the stand-in shown while the photo loads — too small to find dust in.
const LOADED: usize = 1500;

/// Remove dust and its Sensitivity. `true` when a press added patches — the caller then shows them, in a patch
/// mode (in Remove they are drawn nowhere).
pub(super) fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    ed: &mut Edited,
) -> bool {
    ui.add_space(6.0);
    let mut added = false;
    if ui
        .button("Remove dust")
        .hover("Finds sensor dust in the photo and heals each spot with a patch")
        .clicked()
    {
        added = remove(s, tool, ed) > 0;
    }
    ui.add(
        super::slider::Slider::new(&mut tool.dust, 0.0..=100.0)
            .text("Sensitivity")
            .help(slider_help::text(Scope::Patch, "Sensitivity"))
            .reset_to(rustroom_app::dust_tool::DEFAULT_SENSITIVITY),
    );
    added
}

/// The dust in the decode, healed: one Heal patch per spot not already patched, one undo step. Returns how
/// many it added.
fn remove(s: &mut Session, tool: &SpotTool, ed: &mut Edited) -> usize {
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    if dw.max(dh) < LOADED {
        ed.note = Some("Waits for the photo to load".to_string());
        return 0;
    }
    let finds = rustroom_app::dust_tool::find(&s.buffer, tool.dust);
    // The patches live after the lens — the decode's own size, so its own aspect.
    let corr = s.lens_memo.get(s.lens.shot.as_ref(), &s.edits, dw, dh);
    let lens = rustroom_app::ai_space::DecodeMap::with_shared(
        &edit_model::Geometry::default(),
        edit_model::Perspective::default(),
        dw,
        dh,
        corr,
        dw,
        dh,
    );
    let sample = crate::develop::canvas::spot_overlay::sampler_for(s);
    let spots = rustroom_app::dust_tool::to_spots(
        &finds,
        &|d| lens.post_lens_of(d),
        dw as f32 / dh as f32,
        &s.edits.spots,
        *tool,
        &sample,
    );
    let n = spots.len();
    s.edits.spots.extend(spots);
    s.sync_spot_flags();
    ed.note = Some(match (n, finds.is_empty()) {
        (0, true) => "Found no dust".to_string(),
        (0, false) => "Found no new dust".to_string(),
        (1, _) => "Removed 1 dust spot".to_string(),
        (n, _) => format!("Removed {n} dust spots"),
    });
    if n > 0 {
        ed.discrete();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::panel::retouch;

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

        fn click(&mut self, label: &str) -> Edited {
            let ctx = self.ctx.clone();
            let mut seen = Edited::default();
            // ⚠ The app's own tools — Remove, Retouch's default mode, included: where the author pressed it.
            let (mut t, mut e, mut r) =
                (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool);
            crate::test_ui::click(&ctx, label, |ui| {
                let mut ed = Edited::default();
                retouch::show_with(
                    ui,
                    self.app.session.as_mut().unwrap(),
                    &mut t,
                    &mut e,
                    &mut r,
                    &mut ed,
                );
                if ed.changed || ed.settled || ed.note.is_some() {
                    seen = ed;
                }
            });
            self.app.spot_tool = t;
            self.app.remove_tool = r;
            seen
        }

        fn spots(&self) -> Vec<edit_model::Spot> {
            self.app.session.as_ref().unwrap().edits.spots.clone()
        }
    }

    /// Review Focus 5: one press, the dust healed — Heal patches, one undo step, the count said.
    #[test]
    fn remove_dust_adds_heal_patches_as_one_step() {
        let mut p = Panel::new(dusty());
        let ed = p.click("Remove dust");
        let spots = p.spots();
        assert_eq!(spots.len(), 3, "{spots:?}");
        assert!(spots.iter().all(|s| s.mode == edit_model::SpotMode::Heal));
        assert!(ed.changed && ed.settled, "one undo step");
        assert_eq!(ed.note.as_deref(), Some("Removed 3 dust spots"));
    }

    /// The author (2026-10-20): "I can't see the patches" — pressed in Remove, the mode Retouch opens in, the
    /// new patches were drawn nowhere. Once it adds some, Retouch shows the patches.
    #[test]
    fn remove_dust_shows_the_patches_it_made() {
        let mut p = Panel::new(dusty());
        assert!(p.app.remove_tool.active, "the fixture: pressed in Remove");
        p.click("Remove dust");
        assert_eq!(p.spots().len(), 3);
        assert!(
            !p.app.remove_tool.active,
            "still in Remove, the patches unseen"
        );
        assert_eq!(p.app.eye_tool.active, None);
    }

    #[test]
    fn pressing_remove_dust_twice_adds_nothing_more() {
        let mut p = Panel::new(dusty());
        p.click("Remove dust");
        let ed = p.click("Remove dust");
        assert_eq!(p.spots().len(), 3);
        assert!(!ed.settled, "no step for nothing");
        assert_eq!(ed.note.as_deref(), Some("Found no new dust"));
    }

    /// Review Focus 4: on the stand-in a photo shows while it loads, nothing is looked for.
    #[test]
    fn remove_dust_waits_for_the_photo() {
        let mut p = Panel::new(core_image::ImageBuf::from_vec(
            400,
            300,
            vec![0.4; 400 * 300 * 3],
        ));
        let ed = p.click("Remove dust");
        assert!(p.spots().is_empty());
        assert_eq!(ed.note.as_deref(), Some("Waits for the photo to load"));
    }
}

//! Find dust's job (M10.16c2, a review since M10.19d): a press starts the search in the background — or once the photo
//! has loaded — and what it finds lands as a REVIEW: the spots shown on the photo, ticked, until Remove N spots heals
//! them as Heal patches marked `dust`, one undo step. A drawing in Add more looks again under it.
//!
//! ⚠ Pumped by Develop's body, not by the Retouch panel: a search that lands while the panel is closed still lands.
//! ⚠ The dust view (`Session.visualize`) is written here each frame while Retouch is the tool — the review's one
//! writer: on while dust is reviewed in Find, off otherwise.

use crate::{RustroomApp, Session};
use eframe::egui;
use rustroom_app::dust_tool::{self, Search};
use rustroom_app::retouch_task::{FindKind, Gesture, Task};

/// Below this long side the decode is the stand-in shown while the photo loads — too small to find dust in.
const LOADED: usize = 1500;

/// How often the panel redraws while a search runs — its line moves, and it lands within a frame of this.
const REDRAW: std::time::Duration = std::time::Duration::from_millis(50);

impl Session {
    /// A press of Find dust at `sensitivity`: any review ends, and the search starts now or once the photo has
    /// loaded.
    pub(crate) fn press_find_dust(&mut self, sensitivity: f32) {
        self.dust.said = None;
        self.dust.active = false;
        self.dust.finds.clear();
        self.dust.ticked.clear();
        self.dust.look = None;
        self.dust.there = None;
        if self.buffer.width().max(self.buffer.height()) < LOADED {
            self.dust.queued = Some(sensitivity);
            return;
        }
        self.dust.queued = None;
        self.dust.search = Some(Search::spawn(
            std::sync::Arc::clone(&self.buffer),
            sensitivity,
        ));
    }

    /// Remove N spots: the ticked finds healed as patches marked `dust`, the review ended. Returns how many patches
    /// were made — the caller records the one undo step.
    pub(crate) fn remove_dust(&mut self, tool: rustroom_app::spot_tool::SpotTool) -> usize {
        let ticked: Vec<dust_tool::Find> = self
            .dust
            .finds
            .iter()
            .zip(&self.dust.ticked)
            .filter(|(_, &t)| t)
            .map(|(f, _)| *f)
            .collect();
        // ⚠ M10.12c: made in the patches' working space, stored in decode space like every other patch.
        let an = self.anchoring();
        let spots: Vec<edit_model::Spot> = self
            .dust_patches(&ticked, tool)
            .into_iter()
            .map(|sp| pipeline::anchors::anchor_spot(&an, edit_model::Spot { dust: true, ..sp }))
            .collect();
        let n = spots.len();
        self.edits.spots.extend(spots);
        self.sync_spot_flags();
        self.dust.active = false;
        self.dust.finds.clear();
        self.dust.ticked.clear();
        self.dust.look = None;
        self.dust.said = None;
        n
    }

    /// The finds as Heal patches not already there, in the patches' space — after the lens, the decode's
    /// own size and so its own aspect.
    fn dust_patches(
        &mut self,
        finds: &[dust_tool::Find],
        tool: rustroom_app::spot_tool::SpotTool,
    ) -> Vec<edit_model::Spot> {
        let (dw, dh) = (self.buffer.width(), self.buffer.height());
        // ⚠ M10.12c: through the same anchoring the patches are stored by, so a find lands back on itself exactly.
        let an = self.anchoring();
        let sample = crate::develop::canvas::spot_overlay::sampler_for(self);
        // The patches already there, in the same working space, so an overlap is measured like for like.
        let there = self.anchored_view().spots;
        dust_tool::to_spots(
            finds,
            &|d| an.decode_to_post_lens(d),
            dw as f32 / dh as f32,
            &there,
            tool,
            &sample,
        )
    }
}

impl RustroomApp {
    /// Once per Develop frame: the dust view follows the review; a waiting press starts; a drawing looks there; a
    /// search that lands becomes the review, or joins it.
    pub(crate) fn pump_dust(&mut self, ctx: &egui::Context) {
        let state = self.retouch;
        let radius = self.remove_tool.radius;
        let retouching = self.tool == rustroom_app::tool::Tool::Retouch;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let view = (retouching
            && state.task == Task::Find
            && state.find == FindKind::Dust
            && s.dust.active
            && s.dust.view)
            .then_some(s.dust.contrast);
        if retouching && s.visualize != view {
            s.visualize = view;
            s.request(
                s.view_edits(self.tool),
                s.render_max_dim(),
                s.requested_region,
            );
        }
        if let Some(sensitivity) = s.dust.queued {
            s.press_find_dust(sensitivity);
        }
        if s.dust.queued.is_some() {
            ctx.request_repaint_after(REDRAW);
            return;
        }
        // The author's (a), for dust: a drawing in the review looks again in the area around it, closely.
        if s.dust.search.is_none() && s.dust.active {
            if let Some(points) = s.dust.look.take() {
                let (w, h) = (s.buffer.width(), s.buffer.height());
                let rect = rustroom_app::people_tool::look_rect(&points, radius, w, h);
                let [x0, y0] = [(rect[0] * w as f32) as usize, (rect[1] * h as f32) as usize];
                let [x1, y1] = [
                    ((rect[2] * w as f32) as usize).clamp(x0 + 1, w),
                    ((rect[3] * h as f32) as usize).clamp(y0 + 1, h),
                ];
                let crop = std::sync::Arc::new(s.buffer.crop(x0, y0, x1 - x0, y1 - y0));
                s.dust.said = None;
                s.dust.there = Some((rect, points));
                s.dust.search = Some(Search::spawn(crop, dust_tool::LOOK_SENSITIVITY));
            }
        }
        let Some(search) = &s.dust.search else {
            return;
        };
        let Some(finds) = search.poll() else {
            ctx.request_repaint_after(REDRAW);
            return;
        };
        s.dust.search = None;
        let (w, h) = (s.buffer.width(), s.buffer.height());
        match s.dust.there.take() {
            // Remove or Cancel while it looked: the review it was for has ended (as people's).
            Some(_) if !s.dust.active => {}
            // A look there: what is new joins the review, ticked — else the drawing itself.
            Some((rect, points)) => {
                let placed: Vec<dust_tool::Find> = finds
                    .into_iter()
                    .map(|f| dust_tool::place(f, rect, w, h))
                    .collect();
                let mut new = dust_tool::fresh(placed, &s.dust.finds, w, h);
                if new.is_empty() {
                    let d = dust_tool::drawn(&points, radius, w, h);
                    new = dust_tool::fresh(vec![d], &s.dust.finds, w, h);
                }
                if new.is_empty() {
                    s.dust.said = Some("No new dust there".to_string());
                }
                for f in new {
                    s.dust.finds.push(f);
                    s.dust.ticked.push(true);
                }
            }
            // A whole search: the review, every spot ticked — on even with none, so a drawing can still point. ⚠ Nothing
            // said: the review's readout already reads "No dust found".
            None => {
                self.retouch.gesture = Gesture::Pick;
                s.dust.said = None;
                s.dust.ticked = vec![true; finds.len()];
                s.dust.finds = finds;
                s.dust.active = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::RustroomApp;
    use eframe::egui;
    use rustroom_app::dust_tool::{Find, Search};
    use rustroom_app::retouch_task::{FindKind, Gesture, Task};
    use std::sync::Arc;

    /// An app in Retouch with a loaded, flat photo — no dust in it.
    fn app() -> (RustroomApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.tool = rustroom_app::tool::Tool::Retouch;
        app.session.as_mut().unwrap().buffer = Arc::new(core_image::ImageBuf::from_vec(
            1600,
            1200,
            vec![0.4; 1600 * 1200 * 3],
        ));
        (app, ctx)
    }

    fn find(x: f32, y: f32) -> Find {
        Find {
            centre: [x, y],
            radius: 0.004,
            contrast: 0.1,
        }
    }

    fn dust(app: &RustroomApp) -> &rustroom_app::dust_tool::Run {
        &app.session.as_ref().unwrap().dust
    }

    fn land(app: &mut RustroomApp, ctx: &egui::Context) {
        let start = std::time::Instant::now();
        while dust(app).search.is_some() {
            app.pump_dust(ctx);
            assert!(start.elapsed().as_secs() < 60, "the search never landed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// M10.19d: a search lands as a review — every spot shown and ticked, no patch made yet.
    #[test]
    fn a_dust_search_lands_as_a_review_all_ticked() {
        let (mut app, ctx) = app();
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().dust.search = Some(search);
        tx.send(vec![find(0.2, 0.2), find(0.5, 0.5), find(0.8, 0.3)])
            .unwrap();
        app.pump_dust(&ctx);
        let d = dust(&app);
        assert!(d.active && d.search.is_none());
        assert_eq!(d.finds.len(), 3);
        assert_eq!(d.ticked, vec![true; 3]);
        assert!(d.said.is_none());
        assert!(
            app.session.as_ref().unwrap().edits.spots.is_empty(),
            "nothing healed yet"
        );
    }

    /// Nothing found: the review on with nothing in it — its readout says so, nothing else does.
    #[test]
    fn nothing_found_opens_an_empty_review() {
        let (mut app, ctx) = app();
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().dust.search = Some(search);
        tx.send(Vec::new()).unwrap();
        app.pump_dust(&ctx);
        assert!(dust(&app).finds.is_empty());
        assert_eq!(dust(&app).said, None, "said twice with the readout");
        assert!(
            dust(&app).active,
            "on, so a drawing can still point at a spot it missed"
        );
    }

    /// Final review: Cancel or Remove N while a look there runs — its landing is dropped, as people's is, or its finds
    /// would sit in an ended review and "No new dust there" would read under the button.
    #[test]
    fn a_look_landing_after_the_review_ended_is_dropped() {
        let (mut app, ctx) = app();
        let s = app.session.as_mut().unwrap();
        s.dust.active = true;
        s.dust.look = Some(vec![[0.7, 0.6], [0.71, 0.6]]);
        app.pump_dust(&ctx);
        assert!(dust(&app).there.is_some(), "a look there runs");
        // Cancel, as the panel's does.
        let s = app.session.as_mut().unwrap();
        s.dust.active = false;
        s.dust.finds.clear();
        s.dust.ticked.clear();
        s.dust.look = None;
        s.dust.said = None;
        land(&mut app, &ctx);
        let d = dust(&app);
        assert!(d.finds.is_empty(), "{:?}", d.finds);
        assert!(d.said.is_none(), "{:?}", d.said);
        assert!(!d.active && d.there.is_none());
    }

    /// Review Focus 4: Remove N spots heals the ticked ones as Heal patches marked `dust`, and ends the review.
    #[test]
    fn removing_dust_spots_makes_marked_heal_patches() {
        let (mut app, _) = app();
        let tool = app.spot_tool;
        let s = app.session.as_mut().unwrap();
        s.dust.active = true;
        s.dust.finds = vec![
            find(0.2, 0.2),
            find(0.5, 0.5),
            find(0.8, 0.3),
            find(0.3, 0.8),
        ];
        s.dust.ticked = vec![true, true, false, true];
        let n = s.remove_dust(tool);
        assert_eq!(n, 3);
        let spots = &app.session.as_ref().unwrap().edits.spots;
        assert_eq!(spots.len(), 3);
        assert!(spots
            .iter()
            .all(|sp| sp.dust && sp.mode == edit_model::SpotMode::Heal));
        let d = dust(&app);
        assert!(!d.active && d.finds.is_empty() && d.ticked.is_empty());
    }

    /// Review Focus 4: the dust view is on while dust is reviewed in Find, off otherwise.
    #[test]
    fn the_dust_view_follows_the_review() {
        let (mut app, ctx) = app();
        app.retouch.task = Task::Find;
        app.retouch.find = FindKind::Dust;
        app.session.as_mut().unwrap().dust.active = true;
        app.pump_dust(&ctx);
        let contrast = dust(&app).contrast;
        assert_eq!(app.session.as_ref().unwrap().visualize, Some(contrast));
        app.session.as_mut().unwrap().dust.view = false;
        app.pump_dust(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().visualize,
            None,
            "its checkbox"
        );
        app.session.as_mut().unwrap().dust.view = true;
        app.retouch.task = Task::Paint;
        app.pump_dust(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().visualize,
            None,
            "not while painting"
        );
        app.retouch.task = Task::Find;
        app.session.as_mut().unwrap().dust.active = false;
        app.pump_dust(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().visualize,
            None,
            "not after the review"
        );
    }

    /// The author's (a) for dust: a drawing in the review looks again there; on a flat photo it finds nothing, so the
    /// drawing itself joins the review, ticked.
    #[test]
    fn a_dust_look_there_adds_what_it_finds_or_the_drawing() {
        let (mut app, ctx) = app();
        let s = app.session.as_mut().unwrap();
        s.dust.active = true;
        s.dust.finds = vec![find(0.2, 0.2)];
        s.dust.ticked = vec![false];
        s.dust.look = Some(vec![[0.7, 0.6], [0.71, 0.6]]);
        app.pump_dust(&ctx);
        assert!(dust(&app).look.is_none(), "taken");
        assert!(
            dust(&app).search.is_some() && dust(&app).there.is_some(),
            "a look there runs"
        );
        land(&mut app, &ctx);
        let d = dust(&app);
        assert_eq!(d.finds.len(), 2);
        assert_eq!(
            d.ticked,
            vec![false, true],
            "the new one ticked, the old untouched"
        );
        assert!(
            (d.finds[1].centre[0] - 0.705).abs() < 1e-3,
            "{:?}",
            d.finds[1]
        );
        assert!(d.there.is_none());
    }

    /// Pick again after a whole search lands, so drawing to add is always a choice made on purpose.
    #[test]
    fn a_landing_resets_the_gesture_to_pick() {
        let (mut app, ctx) = app();
        app.retouch.gesture = Gesture::Add;
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().dust.search = Some(search);
        tx.send(vec![find(0.2, 0.2)]).unwrap();
        app.pump_dust(&ctx);
        assert_eq!(app.retouch.gesture, Gesture::Pick);
    }
}

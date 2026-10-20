//! Remove dust's job (M10.16c2): a press starts the search in the background — or once the photo has
//! loaded — and the finds land as Heal patches, one undo step, shown in a patch mode.
//!
//! ⚠ Pumped by Develop's body, not by the Retouch panel: a search that lands while the panel is closed
//! still becomes its patches, and its undo step is recorded with the frame's other edits.

use crate::{RustroomApp, Session};
use eframe::egui;
use rustroom_app::dust_tool::{self, Search};

/// Below this long side the decode is the stand-in shown while the photo loads — too small to find dust in.
const LOADED: usize = 1500;

/// How often the panel redraws while a search runs — its line moves, and it lands within a frame of this.
const REDRAW: std::time::Duration = std::time::Duration::from_millis(50);

impl Session {
    /// A press of Remove dust at `sensitivity`: the search starts now, or once the photo has loaded.
    pub(crate) fn press_remove_dust(&mut self, sensitivity: f32) {
        self.dust.said = None;
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

    /// The finds as Heal patches not already there, in the patches' space — after the lens, the decode's
    /// own size and so its own aspect.
    fn dust_patches(
        &mut self,
        finds: &[dust_tool::Find],
        tool: rustroom_app::spot_tool::SpotTool,
    ) -> Vec<edit_model::Spot> {
        let (dw, dh) = (self.buffer.width(), self.buffer.height());
        let corr = self
            .lens_memo
            .get(self.lens.shot.as_ref(), &self.edits, dw, dh);
        let lens = rustroom_app::ai_space::DecodeMap::with_shared(
            &edit_model::Geometry::default(),
            edit_model::Perspective::default(),
            dw,
            dh,
            corr,
            dw,
            dh,
        );
        let sample = crate::develop::canvas::spot_overlay::sampler_for(self);
        dust_tool::to_spots(
            finds,
            &|d| lens.post_lens_of(d),
            dw as f32 / dh as f32,
            &self.edits.spots,
            tool,
            &sample,
        )
    }
}

impl RustroomApp {
    /// Once per Develop frame: a waiting press starts, a finished search becomes its patches. `true` when it
    /// added some — one undo step, which the caller records.
    pub(crate) fn pump_dust(&mut self, ctx: &egui::Context) -> bool {
        let tool = self.spot_tool;
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        if let Some(sensitivity) = s.dust.queued {
            s.press_remove_dust(sensitivity);
        }
        if s.dust.queued.is_some() {
            ctx.request_repaint_after(REDRAW);
            return false;
        }
        let Some(search) = &s.dust.search else {
            return false;
        };
        let Some(finds) = search.poll() else {
            ctx.request_repaint_after(REDRAW);
            return false;
        };
        s.dust.search = None;
        let spots = s.dust_patches(&finds, tool);
        let said = dust_tool::outcome(spots.len(), finds.len());
        s.dust.said = Some(said.clone());
        self.status = said;
        if spots.is_empty() {
            return false;
        }
        s.edits.spots.extend(spots);
        s.sync_spot_flags();
        s.request(
            s.view_edits(self.tool),
            s.render_max_dim(),
            s.requested_region,
        );
        // Shown in a patch mode — in Remove, Retouch's first, the patches are drawn nowhere.
        self.remove_tool.active = false;
        self.eye_tool.active = None;
        s.selected_eye = None;
        s.removal.selected = None;
        s.history.push(s.edits.clone())
    }
}

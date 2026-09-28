//! Zoom and pan for a photo drawn from a **preview** — Compare's two panes (M9.14).
//!
//! ⚠⚠ Replaces the `RenderEngine` panes, which decoded each photo at half size (~1 s, 78 MB,
//! and a worker thread apiece) and could still only show a 2× upscale at 1:1. A pane now holds
//! no pixels at all, only its view state; the texture comes from `RustroomApp::preview_textures`
//! — the camera's JPEG or a rendered preview — and the thumbnail stands in until it lands.
//!
//! ⚠ Each pane's view is its own (the author's call): the two frames are not registered to
//! each other, so a shared view would put the eye you are inspecting in one pane and an ear in
//! the other. Independent views let you put the same *feature* in both.

use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::view_region;

pub(crate) struct PreviewPane {
    pub id: PhotoId,
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl PreviewPane {
    /// ⚠ Starts at fit, not at zero — a derived `Default` on `zoom` would draw nothing.
    pub fn new(id: PhotoId) -> Self {
        Self {
            id,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }

    /// A zoom step about the pointer, then a pan. ⚠ At fit there is nothing hidden to reveal,
    /// so the pan snaps back to zero — the rule the Develop canvas follows.
    pub fn navigate(
        &mut self,
        img: egui::Rect,
        zoom_factor: f32,
        pointer: Option<egui::Pos2>,
        pan: egui::Vec2,
    ) {
        if zoom_factor != 1.0 {
            let next = view_region::zoom_step(self.zoom, zoom_factor);
            if let Some(p) = pointer {
                let c = img.center();
                self.pan += view_region::zoom_anchor_shift(
                    (p.x - c.x, p.y - c.y),
                    (self.pan.x, self.pan.y),
                    self.zoom,
                    next,
                )
                .into();
            }
            self.zoom = next;
        }
        self.pan += pan;
        if self.zoom <= 1.0 {
            self.pan = egui::Vec2::ZERO;
        }
    }

    /// Screen points per texture pixel: the fit ratio, times the zoom.
    pub fn draw_scale(&self, img: egui::Rect, tex: egui::Vec2) -> f32 {
        (img.width() / tex.x.max(1.0)).min(img.height() / tex.y.max(1.0)) * self.zoom
    }
}

/// The panes of one view, kept in step with what it shows.
#[derive(Default)]
pub(crate) struct PaneSet {
    panes: Vec<PreviewPane>,
}

impl PaneSet {
    pub fn clear(&mut self) {
        self.panes.clear();
    }

    pub fn pane_mut(&mut self, id: &PhotoId) -> Option<&mut PreviewPane> {
        self.panes.iter_mut().find(|p| p.id == *id)
    }

    /// Keep the panes still wanted, in place, and add the new ones at fit.
    ///
    /// ⚠ A pane whose photo is no longer shown is dropped, so stepping through candidates never
    /// accumulates view state for photos that left — and a photo that comes back starts at fit.
    pub fn sync(&mut self, want: &[PhotoId]) {
        self.panes.retain(|p| want.contains(&p.id));
        for id in want {
            if !self.panes.iter().any(|p| p.id == *id) {
                self.panes.push(PreviewPane::new(id.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(n: &str) -> PhotoId {
        PhotoId::original(PathBuf::from(format!("/nonexistent/{n}.RAF")))
    }

    fn img() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
    }

    #[test]
    fn the_wanted_photos_get_panes_and_a_photo_no_longer_shown_loses_its_pane() {
        let mut set = PaneSet::default();
        set.sync(&[id("a"), id("b")]);
        assert!(set.pane_mut(&id("a")).is_some());
        assert!(set.pane_mut(&id("b")).is_some());
        set.pane_mut(&id("a")).unwrap().zoom = 3.0;
        set.sync(&[id("a"), id("c")]);
        assert!(
            set.pane_mut(&id("b")).is_none(),
            "the pane for a photo that left the view was kept"
        );
        assert_eq!(
            set.pane_mut(&id("a")).unwrap().zoom,
            3.0,
            "a pane still wanted keeps its view"
        );
        assert_eq!(
            set.pane_mut(&id("c")).unwrap().zoom,
            1.0,
            "a new pane is at fit"
        );
    }

    #[test]
    fn zooming_one_pane_leaves_the_other_alone() {
        let mut set = PaneSet::default();
        set.sync(&[id("a"), id("b")]);
        set.pane_mut(&id("a"))
            .expect("pane a")
            .navigate(img(), 2.0, None, egui::Vec2::ZERO);
        assert!(
            set.pane_mut(&id("a")).unwrap().zoom > 1.0,
            "pane a did not zoom"
        );
        assert_eq!(
            set.pane_mut(&id("b")).unwrap().zoom,
            1.0,
            "zooming one pane moved the other"
        );
    }

    #[test]
    fn a_new_pane_starts_at_fit_and_panning_at_fit_does_nothing() {
        let mut pane = PreviewPane::new(id("a"));
        assert_eq!(pane.zoom, 1.0);
        assert_eq!(pane.pan, egui::Vec2::ZERO);
        pane.navigate(img(), 1.0, None, egui::vec2(50.0, 50.0));
        assert_eq!(pane.pan, egui::Vec2::ZERO);
        pane.navigate(img(), 3.0, None, egui::Vec2::ZERO);
        pane.navigate(img(), 1.0, None, egui::vec2(50.0, 50.0));
        assert_ne!(pane.pan, egui::Vec2::ZERO, "zoomed in, panning must work");
    }

    #[test]
    fn the_draw_scale_fits_the_texture_then_zooms() {
        let mut pane = PreviewPane::new(id("a"));
        let tex = egui::vec2(4416.0, 2944.0);
        let fit = pane.draw_scale(img(), tex);
        assert!(
            (fit - 400.0 / 4416.0).abs() < 1e-6,
            "fit by the wider axis: {fit}"
        );
        pane.zoom = 2.0;
        assert!((pane.draw_scale(img(), tex) - 2.0 * fit).abs() < 1e-6);
    }

    #[test]
    fn clearing_drops_the_panes() {
        let mut set = PaneSet::default();
        set.sync(&[id("a")]);
        set.clear();
        assert!(set.pane_mut(&id("a")).is_none());
    }
}

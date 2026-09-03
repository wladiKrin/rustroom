//! One side of Compare: a real decode with its own render engine.
//!
//! ⚠⚠ **Compare is the one culling view that does NOT draw from the thumbnail cache, and that is
//! the whole point of it.** Thumbnails are capped at 512 px, so zooming one magnifies a blur —
//! and choosing between two nearly identical frames is *precisely* a question about sharpness and
//! focus, which is the one thing a thumbnail cannot answer. Survey keeps the thumbnails, because
//! composition and expression read fine at 512 px and a wall of decodes would not fit in memory.
//!
//! ⚠ A pane is a cut-down `Session`: a buffer, an engine, a texture. It deliberately carries no
//! history, no snapshots and no sidecar state — nothing here edits anything, so a real `Session`
//! would be most of a photo editor standing behind a picture you are only looking at.

use crate::PREVIEW_MAX_DIM;
use eframe::egui;
use rustroom_app::decode_worker::Decoded;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::{FrameSpec, RenderEngine, RenderedFrame};
use rustroom_app::view_region;
use std::sync::Arc;

pub(crate) struct Pane {
    pub id: PhotoId,
    engine: RenderEngine,
    /// The editing buffer's long edge — the top rung of this pane's render ladder.
    buffer_long: usize,
    tex: Option<egui::TextureHandle>,
    /// The frame currently on screen, so its size is known without asking the texture.
    shown: Option<RenderedFrame>,
    /// What the last request asked for, so an unchanged view issues none.
    requested: Option<(usize, u64)>,
    edits: edit_model::Edits,
    /// This pane's own magnification. 1.0 is fit.
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl Pane {
    pub fn new(
        id: PhotoId,
        decoded: &Arc<Decoded>,
        edits: edit_model::Edits,
        ctx: &egui::Context,
    ) -> Self {
        let ctx2 = ctx.clone();
        let buffer_long = decoded.buffer.width().max(decoded.buffer.height());
        Self {
            id,
            engine: RenderEngine::new(
                decoded.buffer.clone(),
                decoded.source_scale,
                decoded.lens.shot.clone(),
                decoded.wb_baseline,
                decoded.profile.clone(),
                decoded.scene_referred,
                move || ctx2.request_repaint(),
            ),
            buffer_long,
            tex: None,
            shown: None,
            requested: None,
            edits,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }

    /// Fold this frame's navigation into the pane's own view.
    ///
    /// ⚠ Zoom before pan: the anchor correction depends on the *new* zoom, and the other order
    /// anchors against a stale factor — the order the Develop canvas records.
    ///
    /// ⚠ At fit there is nothing hidden to reveal, so a stray pan must not push the photo off
    /// into the grey with nothing to bring it back. The canvas records the same rule.
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

    /// The bottom rung: what a fit render is capped to.
    pub fn fit_cap(&self) -> usize {
        view_region::fit_cap(self.buffer_long, PREVIEW_MAX_DIM)
    }

    /// Adopt new edits, if they have changed.
    ///
    /// ⚠ Cheaper than rebuilding the pane: the engine's buffer and prefix cache survive, and a
    /// changed edit is an ordinary re-request. Rebuilding would throw away a ~78 MB decode to
    /// react to a slider that was moved in Develop.
    pub fn set_edits(&mut self, edits: edit_model::Edits) {
        if self.edits != edits {
            self.edits = edits;
            self.requested = None;
        }
    }

    /// Ask for a render if the view has moved since the last one, then take any finished frame.
    ///
    /// ⚠⚠ **Whole-image renders, never regions.** The Develop canvas renders a sub-rect at high
    /// zoom because a full-resolution buffer at rung 6246 is 26 Mpx of tail work on every slider
    /// move. Nothing is being edited here: the render happens once per rung rather than per
    /// frame, and a whole image is what lets panning be a texture offset with no re-render at all.
    pub fn update(
        &mut self,
        img: egui::Rect,
        zoom: f32,
        space: color::OutputSpace,
        ctx: &egui::Context,
    ) {
        let max_dim =
            view_region::tier_max_dim(self.fit_cap(), self.buffer_long, self.fit_ratio(img), zoom);
        let key = (max_dim, space as u64);
        if self.requested != Some(key) {
            self.requested = Some(key);
            self.engine.request(
                self.edits.clone(),
                FrameSpec {
                    max_dim,
                    region: None,
                    display_space: space,
                    resampler: pipeline::Resampler::default(),
                    visualize: None,
                },
            );
        }
        if let Some(f) = self.engine.poll_latest() {
            self.tex = Some(ctx.load_texture(
                format!("compare-{}-{}", self.id.path.display(), self.id.copy),
                egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba),
                egui::TextureOptions::LINEAR,
            ));
            self.shown = Some(f);
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.tex.as_ref()
    }

    /// Points of screen per pixel of a *fit-rung* frame — what [`view_region::tier_max_dim`]
    /// calls `fit`, so this pane climbs the same ladder the Develop canvas does.
    ///
    /// ⚠ Before the first frame lands there is nothing to measure, and the pane's own extent is
    /// the best available guess. It over-estimates at worst, which picks a rung one too high
    /// once — where under-estimating would leave the photo soft with nothing to correct it.
    fn fit_ratio(&self, img: egui::Rect) -> f32 {
        let (fw, fh) = self
            .shown
            .as_ref()
            .map(|f| (f.width.max(1) as f32, f.height.max(1) as f32))
            .unwrap_or((img.width().max(1.0), img.height().max(1.0)));
        let fitted_long = (fw.max(fh)) * (img.width() / fw).min(img.height() / fh);
        fitted_long / self.fit_cap().max(1) as f32
    }

    /// The scale to draw the shown texture at so the photo appears at `zoom` of its fit size.
    ///
    /// ⚠⚠ **Derived from the frame the texture actually came from, never from the rung that was
    /// asked for.** A render lands a frame or two after the request, and a crop makes the frame
    /// smaller than its cap — so a scale computed from `max_dim` would size the picture by a
    /// number that does not describe it. This project has paid for exactly that confusion twice,
    /// in `render_max_dim` and in `fit_image_size`. Measuring the texture is self-correcting: a
    /// higher rung gives a larger texture and a proportionally smaller scale, so the picture
    /// holds still on screen and only gets sharper.
    pub fn draw_scale(&self, img: egui::Rect, zoom: f32) -> f32 {
        let Some(f) = &self.shown else { return 1.0 };
        let (w, h) = (f.width.max(1) as f32, f.height.max(1) as f32);
        (img.width() / w).min(img.height() / h) * zoom
    }
}

/// Compare's two panes and the view they share.
///
/// ⚠⚠ **Each pane zooms and pans on its own** (user's call, 2026-09-02). Shipping this shared was
/// a defensible reading — "which is sharper *here*" wants the same part of both frames — but the
/// two photographs are not registered to each other: they were shot seconds apart, handheld, and
/// the subject moved. A shared view then puts the eye you are inspecting in one pane and an ear
/// in the other, and the sharing actively gets in the way of the alignment it was meant to
/// guarantee. Independent views let you put the same *feature* in both, which is what actually
/// answers the question.
#[derive(Default)]
pub(crate) struct CompareView {
    panes: Vec<Pane>,
    /// The one decode in flight, so a miss does not re-request every frame.
    pending: Option<std::path::PathBuf>,
}

impl CompareView {
    /// Reset the view. Called on leaving Compare, so the panes' buffers and workers are dropped
    /// rather than held for a view nobody is looking at.
    pub fn clear(&mut self) {
        self.panes.clear();
        self.pending = None;
    }

    pub fn pane_mut(&mut self, id: &PhotoId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == *id)
    }

    /// Keep exactly the panes the view is showing, and ask for the decode of anything missing.
    ///
    /// ⚠ **At most one decode in flight.** `DecodeWorker` caps concurrency at two and keeps only
    /// the newest queued request, so firing both panes at once can drop one — and the dropped one
    /// would never be retried, leaving a pane stuck on its thumbnail with nothing to say why.
    ///
    /// ⚠ A failed decode leaves `pending` set for that path, so it is asked for **once**. The
    /// pane then stays on its thumbnail rather than the view retrying a decode that will fail
    /// again on every frame.
    pub fn sync(
        &mut self,
        want: &[(PhotoId, edit_model::Edits)],
        cache: &mut rustroom_app::buffer_cache::BufferCache,
        decoder: &mut rustroom_app::decode_worker::DecodeWorker,
        ctx: &egui::Context,
    ) {
        self.panes
            .retain(|p| want.iter().any(|(id, _)| *id == p.id));
        if let Some(p) = &self.pending {
            if !want.iter().any(|(id, _)| id.path == *p) {
                self.pending = None;
            }
        }
        for (id, edits) in want {
            if let Some(pane) = self.panes.iter_mut().find(|p| p.id == *id) {
                pane.set_edits(edits.clone());
                continue;
            }
            if let Some(decoded) = cache.get(&id.path) {
                if self.pending.as_deref() == Some(id.path.as_path()) {
                    self.pending = None;
                }
                self.panes
                    .push(Pane::new(id.clone(), &decoded, edits.clone(), ctx));
            } else if self.pending.is_none() {
                self.pending = Some(id.path.clone());
                decoder.request(id.path.clone(), PREVIEW_MAX_DIM);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::buffer_cache::BufferCache;
    use rustroom_app::decode_worker::DecodeWorker;
    use std::path::PathBuf;

    fn id(n: &str) -> PhotoId {
        PhotoId::original(PathBuf::from(format!("/nonexistent/{n}.RAF")))
    }

    fn worker() -> DecodeWorker {
        DecodeWorker::new(
            |_: &std::path::Path| Err("no decoder in tests".to_string()),
            2,
            || {},
        )
    }

    fn want(ids: &[PhotoId]) -> Vec<(PhotoId, edit_model::Edits)> {
        ids.iter()
            .map(|i| (i.clone(), edit_model::Edits::default()))
            .collect()
    }

    /// ⚠⚠ **At most one decode in flight, and this is the assertion that can see it.**
    /// `DecodeWorker` caps concurrency and keeps only the newest queued request, so asking for
    /// both panes at once can drop one — and a dropped request is never retried, leaving that
    /// pane stuck on its thumbnail with nothing to say why.
    #[test]
    fn only_one_decode_is_asked_for_at_a_time() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        view.sync(&want(&[id("a"), id("b")]), &mut cache, &mut w, &ctx);
        assert_eq!(w.requests_issued(), 1, "both panes were requested at once");
    }

    /// ⚠ And it does not re-ask every frame: a view drawn at 60 Hz would otherwise queue sixty
    /// decodes a second for a photo that is already on its way.
    #[test]
    fn a_pending_decode_is_not_asked_for_again() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        let ids = want(&[id("a"), id("b")]);
        for _ in 0..5 {
            view.sync(&ids, &mut cache, &mut w, &ctx);
        }
        assert_eq!(
            w.requests_issued(),
            1,
            "the same decode was queued repeatedly"
        );
    }

    /// A buffer already in the cache builds its pane immediately — the common case, since the
    /// photo you were just editing is still there.
    #[test]
    fn a_cached_buffer_becomes_a_pane_without_a_decode() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        cache.insert(id("a").path.clone(), crate::test_app::fake_decode());
        view.sync(&want(&[id("a")]), &mut cache, &mut w, &ctx);
        assert!(view.pane_mut(&id("a")).is_some(), "no pane was built");
        assert_eq!(
            w.requests_issued(),
            0,
            "a cached buffer still triggered a decode"
        );
    }

    /// ⚠⚠ **A pane whose photo is no longer shown is dropped.** Each one holds a decode and a
    /// render worker, so a view that accumulated them across a session of stepping through
    /// candidates would leak both — and nothing else in the app would notice.
    #[test]
    fn a_pane_for_a_photo_no_longer_shown_is_dropped() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        for n in ["a", "b"] {
            cache.insert(id(n).path.clone(), crate::test_app::fake_decode());
        }
        view.sync(&want(&[id("a"), id("b")]), &mut cache, &mut w, &ctx);
        assert!(view.pane_mut(&id("a")).is_some());
        view.sync(&want(&[id("b")]), &mut cache, &mut w, &ctx);
        assert!(
            view.pane_mut(&id("a")).is_none(),
            "the pane for a photo that left the view was kept"
        );
        assert!(view.pane_mut(&id("b")).is_some());
    }

    /// ⚠⚠ **Each pane's view is its own** (user's call). Zooming one must leave the other alone:
    /// the two frames are not registered to each other — shot seconds apart, handheld, subject
    /// moving — so a shared view puts the eye you are inspecting in one pane and an ear in the
    /// other. Independent views let you put the same *feature* in both.
    #[test]
    fn zooming_one_pane_leaves_the_other_alone() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        for n in ["a", "b"] {
            cache.insert(id(n).path.clone(), crate::test_app::fake_decode());
        }
        view.sync(&want(&[id("a"), id("b")]), &mut cache, &mut w, &ctx);
        let img = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        view.pane_mut(&id("a"))
            .expect("pane a")
            .navigate(img, 2.0, None, egui::Vec2::ZERO);
        assert!(
            view.pane_mut(&id("a")).unwrap().zoom > 1.0,
            "pane a did not zoom"
        );
        assert_eq!(
            view.pane_mut(&id("b")).unwrap().zoom,
            1.0,
            "zooming one pane moved the other"
        );
    }

    /// ⚠ A pane starts at fit, not at zero — a derived `Default` on the field would draw nothing.
    #[test]
    fn a_new_pane_starts_at_fit() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        cache.insert(id("a").path.clone(), crate::test_app::fake_decode());
        view.sync(&want(&[id("a")]), &mut cache, &mut w, &ctx);
        let pane = view.pane_mut(&id("a")).expect("pane");
        assert_eq!(pane.zoom, 1.0);
        assert_eq!(pane.pan, egui::Vec2::ZERO);
    }

    /// ⚠ At fit there is nothing hidden to reveal, so a stray pan must not push the photo off
    /// into the grey with nothing to bring it back — the rule the Develop canvas already follows.
    #[test]
    fn panning_at_fit_does_nothing() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        cache.insert(id("a").path.clone(), crate::test_app::fake_decode());
        view.sync(&want(&[id("a")]), &mut cache, &mut w, &ctx);
        let img = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let pane = view.pane_mut(&id("a")).expect("pane");
        pane.navigate(img, 1.0, None, egui::vec2(50.0, 50.0));
        assert_eq!(pane.pan, egui::Vec2::ZERO);
        pane.navigate(img, 3.0, None, egui::Vec2::ZERO);
        pane.navigate(img, 1.0, None, egui::vec2(50.0, 50.0));
        assert_ne!(pane.pan, egui::Vec2::ZERO, "zoomed in, panning must work");
    }

    /// ⚠ Clearing drops the panes, which is what frees ~78 MB and a worker thread each.
    #[test]
    fn clearing_drops_the_panes_and_the_view() {
        let ctx = crate::test_ui::ctx();
        let mut view = CompareView::default();
        let mut cache = BufferCache::new(3);
        let mut w = worker();
        cache.insert(id("a").path.clone(), crate::test_app::fake_decode());
        view.sync(&want(&[id("a")]), &mut cache, &mut w, &ctx);
        view.clear();
        assert!(view.pane_mut(&id("a")).is_none());
    }
}

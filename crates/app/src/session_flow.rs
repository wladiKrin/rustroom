//! Opening a photo and keeping its [`Session`] current: the decode handover, the
//! full-resolution upgrade, and the per-frame pumps that drive both.
//!
//! Split out of `main.rs` unchanged. ⚠ `Session` still means "fully decoded" — the
//! half-open state lives in [`PendingOpen`], and nothing downstream knows about it.

use crate::{
    preview_edits, AppView, PendingOpen, RustroomApp, Session, DECODE_DEBOUNCE, PREVIEW_MAX_DIM,
};
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::decode_worker::Decoded;
use rustroom_app::full_res;
use rustroom_app::history::History;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
use rustroom_app::sidecar;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// The photo as it was before any adjustment, at the **current framing**.
///
/// ⚠ Geometry survives on purpose — see [`Session::view_edits`]. A free function rather than a
/// method so it can be tested without building a `Session`, which needs a decoded buffer and a
/// texture.
pub(crate) fn before_edits(edits: &Edits) -> Edits {
    Edits {
        geometry: edits.geometry,
        ..Default::default()
    }
}

impl Session {
    /// The working resolution for a re-render driven from a panel rather than the canvas.
    ///
    /// **The rung the canvas last chose**, whatever it was.
    ///
    /// ⚠⚠ This used to fall back to `fit_cap()` whenever `requested_region` was `None`, on the
    /// reasoning that a stored rung and a stored region are two facts that can disagree. The
    /// reasoning was right about the hazard and wrong about the remedy, because **"no region"
    /// stopped implying "the fit rung"** the moment the fit clamp was removed: once `fit` may
    /// exceed 1.0 the *whole image* can be visible at a native rung, and the canvas then stores
    /// `render_rung = 3200` with `requested_region = None`.
    ///
    /// ⚠ Reported in use: crop a photo, move any slider, and the image goes soft. A crop makes
    /// the image smaller, so `fit` is larger and the ladder climbs — while the whole cropped
    /// frame still fits on screen, so there is no region. The slider then re-rendered at half the
    /// resolution the canvas was displaying, and the canvas would not correct it, because from
    /// its side neither the rung nor the region had changed. Nudging the zoom crossed a rung
    /// boundary and put it right until the next slider move.
    ///
    /// ⚠ The original hazard is still handled, at its source: `pump_full_res` clears the region
    /// **and** resets this rung, so the two cannot drift apart there either.
    pub(crate) fn render_max_dim(&self) -> usize {
        self.render_rung
    }

    /// The ladder's base rung for this session's buffer.
    ///
    /// ⚠ Every whole-image request must use this, not `PREVIEW_MAX_DIM`. They differ for an
    /// image smaller than the cap, and the canvas identifies a fit frame by comparing the
    /// frame's own `max_dim` against this — so a request at the wrong value produces a frame
    /// that is silently never accepted as the fit size.
    pub(crate) fn fit_cap(&self) -> usize {
        view_region::fit_cap(self.buffer_long, PREVIEW_MAX_DIM)
    }

    /// Send a render request. A one-line delegate: it no longer routes anywhere, but "every
    /// render request goes through one place" is worth keeping.
    pub(crate) fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        // ⚠⚠ Recorded so the guide overlay can be drawn against what is **on screen** rather than
        // what has just been asked for. Perspective is a *prefix* stage, so changing it drops the
        // prefix cache — at native zoom the re-render costs hundreds of milliseconds, during which
        // the guides would sit at their new positions over the previous image.
        self.pending_geo
            .set((edits.perspective, edits.geometry.angle));
        self.engine.request(
            edits,
            FrameSpec {
                max_dim,
                region,
                display_space: self.display_space,
                resampler: self.resampler,
            },
        );
    }
}

impl Session {
    /// The edits to render for the current view.
    ///
    /// ⚠ **Every render request goes through this**, so nothing can forget the before/after
    /// state. That is the point: `show_before` used to be handled by swapping in a texture the
    /// decode worker had rendered once, at preview resolution, which looked correct at fit and
    /// visibly pixelated at any zoom.
    ///
    /// ⚠ **"Before" keeps the geometry and drops the adjustments**, rather than being fully
    /// neutral. A fully neutral render is *uncropped*, so it has different dimensions from the
    /// edited one — the region and placement maths would describe different content, and
    /// toggling while zoomed would jump to somewhere else in the photo. Holding the framing
    /// fixed is what makes a zoomed comparison mean anything.
    pub(crate) fn view_edits(&self, tool: Tool) -> Edits {
        let edits = if self.show_before {
            before_edits(&self.edits)
        } else {
            self.edits.clone()
        };
        preview_edits(&edits, tool)
    }
}

impl RustroomApp {
    /// Save the current session's edits to its `<file>.json` sidecar, if it has any.
    /// Called before switching photos / on exit so edits are never lost.
    /// The open photo, which is what the action log's photo-edit entries must match to be
    /// undoable right now.
    ///
    /// ⚠ Returns an owned `PathBuf` rather than a borrow: every caller passes it *into* a
    /// `&mut self.actions` method, and a borrow of `self` living across that call does not
    /// compile. One clone per user gesture is not worth fighting the borrow checker over.
    /// Which photo the undo log should scope to.
    ///
    /// Every site that scopes undo goes through here, which is what keeps "which photo is this?"
    /// a single answer rather than one per call site.
    pub(crate) fn open_photo(&self) -> Option<PhotoId> {
        self.session
            .as_ref()
            .map(|s| PhotoId::new(s.path.clone(), s.copy))
    }

    pub(crate) fn persist_current_session(&mut self) {
        let Some(s) = &self.session else { return };
        // `persist`, not `save`: neutral edits *delete* the sidecar. Skipping the write
        // would leave a stale one behind, so "Reset all" survived only until the next
        // switch away and back.
        // ⚠ `persist_copy`, not `persist`: two copies of one file share a sidecar, and
        // whole-file writes would make the last-saved copy silently overwrite the other.
        let path = s.path.clone();
        let _ = sidecar::persist_copy(&path, s.copy, &s.edits);
        // ⚠⚠ **The row's cached edits must follow the sidecar, or the thumbnail goes stale the
        // moment this photo stops being the open one.** `thumb_request` prefers the live session's
        // edits *only while the photo is open*; every other row renders from `PhotoEntry::edits`,
        // which `entries::build` read at folder-open time. Without this, resetting a crop updated
        // the filmstrip immediately and then reverted to the old cropped thumbnail as soon as you
        // arrowed away — reported 2026-09-02, and it held for every edit, not just geometry.
        self.refresh_entry_edits(&[path]);
    }

    /// Re-read `paths`' sidecars into their rows' cached edits.
    ///
    /// ⚠⚠ **`PhotoEntry::edits` is a cache of the sidecar that nothing else invalidates.** It is
    /// what an *unopened* photo's thumbnail renders through, so any write to a sidecar must be
    /// followed by this or that photo's thumbnail shows a state that is no longer on disk — with
    /// nothing on screen to say so. There are exactly three writers, and each has its own test:
    /// [`Self::persist_current_session`] (the open photo, on a switch or on quit),
    /// `apply_to_selection` (a paste or a sync), and the `Sync` arms of undo and redo.
    ///
    /// ⚠ Re-reads rather than taking the edits as an argument, deliberately: a sync merges only
    /// the selected *groups* into each target's existing edits, so the result differs per photo
    /// and only the file knows it. At 0.006 ms a sidecar, 200 targets cost ~1.2 ms.
    ///
    /// ⚠ **Not `rebuild_entries`**, which would also re-sort — and under the "last edited" order
    /// that reorders the filmstrip under someone mid-edit, which `CLAUDE.md` records as worse
    /// than a slightly stale order.
    pub(crate) fn refresh_entry_edits(&mut self, paths: &[PathBuf]) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        for e in lib.entries.iter_mut() {
            if paths.contains(&e.id.path) {
                e.edits = sidecar::load_copy(&e.id.path, e.id.copy).unwrap_or_default();
            }
        }
    }

    /// Apply `src`'s selected groups to the open photo and re-render.
    ///
    /// Returns whether a history step was actually recorded — `History::push` is a no-op
    /// when nothing changed, and logging a step that did not happen would desynchronize the
    /// action log from the session history. The caller decides how to log it, which is what
    /// lets a paste bundle this together with a sync into one undo step.
    pub(crate) fn apply_edits_to_session(&mut self, src: &Edits, groups: EditGroups) -> bool {
        // ⚠ Hoisted before the session borrow: `self.tool` is an immutable borrow of `self`
        // and `&mut self.session` is a mutable one, so reading it afterwards is E0502.
        // `Tool` is `Copy`, so the hoist is free.
        let tool = self.tool;
        let Some(s) = &mut self.session else {
            return false;
        };
        s.edits.merge_from(src, groups);
        let pushed = s.history.push(s.edits.clone());
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        pushed
    }

    /// Re-render after the session's edits were changed by an undo or a redo.
    pub(crate) fn rerender_current(&mut self) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        s.edits = s.history.get().clone();
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }

    /// Start opening `path`: instant from the cache, otherwise a debounced background decode
    /// with the filmstrip thumbnail as a placeholder. Never blocks the UI thread.
    pub(crate) fn open_path(&mut self, ctx: &egui::Context, id: PhotoId) {
        self.persist_current_session();

        // ⚠ The cache is keyed by **path**, so opening a copy of an already-decoded file is a
        // hit — which is exactly M3.4's "no storage cost".
        if let Some(decoded) = self.cache.get(&id.path) {
            self.pending = None;
            self.finalize_session(ctx, id, decoded);
            return;
        }

        let placeholder = self.thumbnail_for(&id);
        let path = id.path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.status = format!("Decoding {name}…");
        self.pending = Some(PendingOpen {
            id,
            placeholder,
            start_at: Instant::now() + DECODE_DEBOUNCE,
            seq: None,
        });
    }

    /// The library thumbnail for `path`, if the folder is loaded and that thumbnail has
    /// already arrived. `TextureHandle` is `Arc`-backed, so cloning it is cheap.
    pub(crate) fn thumbnail_for(&self, id: &PhotoId) -> Option<egui::TextureHandle> {
        // ⚠ `get_stale`: the best thumbnail available for this photo, even if its edits have
        // since changed. The decode placeholder and the grid then show the previous render rather
        // than grey while a re-render is in flight.
        self.library.as_ref()?.cache.get_stale(id).cloned()
    }

    /// The edits a photo's thumbnail should be rendered with.
    ///
    /// ⚠⚠ **Two sources, deliberately.** For the **open** photo the edits live in `Session` and
    /// the sidecar is stale — `persist_current_session` writes only on a photo switch or on quit,
    /// which is exactly the period during which the filmstrip most needs to update. For every
    /// other photo the entry's copy, read from the sidecar when the row was built, is correct.
    pub(crate) fn live_edits(&self) -> Option<(PathBuf, u32, Edits)> {
        let s = self.session.as_ref()?;
        Some((s.path.clone(), s.copy, s.edits.clone()))
    }

    /// Build the editing `Session` from a decoded photo (cache hit or worker result).
    pub(crate) fn finalize_session(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        decoded: Arc<Decoded>,
    ) {
        let path = id.path.clone();
        // ⚠ The only place `self.session` is ever assigned, and so the only choke point the
        // stash needs: the outgoing history is put away here whether the switch came from a
        // cache hit or from a finished background decode.
        if let Some(old) = self.session.take() {
            self.histories
                .stash(PhotoId::new(old.path, old.copy), old.history);
        }
        // ⚠ Scanned here, once per photo, rather than in the panel body which runs every frame.
        // Cheap enough per photo; ruinous sixty times a second.
        self.camera_profiles_cache = camera_profile::store::ProfileStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        )
        .list_for(&decoded.label);

        // ⚠ The preference applies only to a photo with **no sidecar**. A photo already edited
        // keeps exactly what it had, which is what lets `Edits::default()` stay strictly neutral
        // while fresh photos still open corrected — see `Settings::initial_lens_correction`.
        let stored = sidecar::load_copy(&path, id.copy);
        let mut edits = stored.clone().unwrap_or_default();
        edits.lens_correction = self.settings.initial_lens_correction(stored.is_some());
        let edits = edits;
        let history = match self.histories.take_matching(&id, &edits) {
            Some(h) => h,
            None => {
                // Either this photo has no stack, or the one it had describes a state no
                // longer on disk — a sync or a paste rewrote its sidecar while it was closed.
                // Any log entry pointing at that stack would now pop steps it does not hold.
                self.actions.forget_edits_of(&id);
                History::new(edits.clone())
            }
        };

        // ⚠ One read for both the name and the snapshots, rather than a `load_all` per field:
        // they come from the same envelope, and a second read is a second chance to disagree.
        let envelope = sidecar::load_all(&id.path);

        let bf = &decoded.baseline;
        let baseline = ctx.load_texture(
            "baseline",
            egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
            egui::TextureOptions::LINEAR,
        );

        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(
            decoded.buffer.clone(),
            decoded.source_scale,
            decoded.lens.shot.clone(),
            decoded.wb_baseline,
            move || ctx2.request_repaint(),
        );
        let buffer_long = decoded.buffer.width().max(decoded.buffer.height());
        // `fit_cap`, not `PREVIEW_MAX_DIM`: they differ for an image smaller than the cap, and
        // the canvas only accepts a frame as the fit size when its `max_dim` matches.
        let fit_cap = view_region::fit_cap(buffer_long, PREVIEW_MAX_DIM);
        // ⚠ The display space must be passed here too. This call used `FrameSpec::fit`, which
        // hardcodes sRGB, so **every photo opened uncorrected** and stayed that way until some
        // unrelated edit forced a re-render — which made the whole setting look inert. Found by
        // instrumenting `pack`, not by reading the code, which looks right at every call site.
        // `FrameSpec::fit` is now `#[cfg(test)]` so this cannot recur.
        engine.request(
            edits.clone(),
            FrameSpec {
                max_dim: fit_cap,
                region: None,
                display_space: self
                    .settings
                    .effective_display_space(self.os_manages_colour),
                resampler: self.settings.resampler,
            },
        );
        // Non-RAW formats are decoded at full size, so their buffer is already the real thing.
        let buffer_is_full_res = !catalog::is_raw(&path);

        self.session = Some(Session {
            display_space: self
                .settings
                .effective_display_space(self.os_manages_colour),
            resampler: self.settings.resampler,
            path,
            copy: id.copy,
            // ⚠ Read from the sidecar rather than carried from the Library entry, so a session
            // opened without a Library (single-file mode) still knows its own name.
            name: envelope.label_of(id.copy).map(str::to_string),
            snapshots: envelope.snapshots_of(id.copy).to_vec(),
            engine,
            edits,
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            lens: decoded.lens.clone(),
            wb_baseline: decoded.wb_baseline,
            buffer: decoded.buffer.clone(),
            picking_wb: false,
            zoom: 1.0,
            fit_scale: 1.0,
            pan: egui::Vec2::ZERO,
            history,
            show_before: false,
            active_handle: None,
            selected_mask: None,
            active_mask_handle: None,
            guides: Vec::new(),
            active_guide: None,
            shown_geo: (edit_model::Perspective::default(), 0.0),
            pending_geo: std::cell::Cell::new((edit_model::Perspective::default(), 0.0)),
            show_guides: false,
            rendered_region: None,
            requested_region: None,
            // Corrected by the first frame that arrives; the placeholder only has to be
            // non-zero so the first fit calculation cannot divide by zero.
            fit_image_size: egui::vec2(1.0, 1.0),
            rendered_long: 1.0,
            buffer_long,
            buffer_is_full_res,
            opened_at: Instant::now(),
            // ⚠ `fit_cap`, not `PREVIEW_MAX_DIM`: they differ for an image smaller than the
            // cap, and this value is now what every panel-driven render asks for.
            render_rung: view_region::fit_cap(buffer_long, PREVIEW_MAX_DIM),
        });
        self.status.clear();
    }

    /// Per-frame: start the full-resolution decode once the session has settled.
    ///
    /// ⚠ It must schedule its own repaint while waiting. egui does not redraw an idle window,
    /// so without this the dwell could expire with nothing to notice, and the decode would
    /// start only when the user next moved the mouse.
    pub(crate) fn maybe_request_full_res(&mut self, ctx: &egui::Context) {
        let in_flight = self.full_pending.is_some();
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if !catalog::is_raw(&s.path) || s.buffer_is_full_res || in_flight {
            return;
        }
        let age = s.opened_at.elapsed();
        if !full_res::wants_upgrade(true, false, false, age) {
            ctx.request_repaint_after(full_res::SETTLE - age);
            return;
        }
        let path = s.path.clone();
        let seq = self.full_decoder.request(path.clone(), PREVIEW_MAX_DIM);
        self.full_pending = Some((path, seq));
        self.status = "Loading full resolution…".to_string();
    }

    /// Per-frame: absorb a finished full-resolution decode into the live session.
    pub(crate) fn pump_full_res(&mut self, ctx: &egui::Context) {
        let tool = self.tool;
        for res in self.full_decoder.poll() {
            let is_current = self.full_pending.as_ref().map(|(_, seq)| *seq) == Some(res.seq);
            if is_current {
                self.full_pending = None;
            }
            // A result for a photo we have since navigated away from is dropped: the buffer is
            // per-session and dies with it, so there is nothing to keep it in.
            let Some(s) = self.session.as_mut() else {
                continue;
            };
            if !is_current || s.path != res.path {
                continue;
            }
            match res.outcome {
                Ok(decoded) => {
                    // ⚠ The baseline `DecodeWorker` rendered is discarded on purpose: the
                    // existing preview-resolution one stays, so before/after does not change
                    // under the user. The waste is one ~60 ms render on a background thread
                    // during a 3 s decode.
                    // ⚠ The old engine is *replaced*, not kept alongside: once every rung
                    // renders from the full buffer the half-size one is dead weight. Dropping
                    // it disconnects its channel, so its worker thread returns on its own. Its
                    // prefix cache goes too — a one-off cost at the moment of upgrade.
                    let ctx2 = ctx.clone();
                    s.buffer_long = decoded.buffer.width().max(decoded.buffer.height());
                    s.engine = RenderEngine::new(
                        decoded.buffer.clone(),
                        decoded.source_scale,
                        // ⚠ The profile must be carried across the full-resolution handover, or
                        // corrections would silently switch off the moment the upgrade lands.
                        s.lens.shot.clone(),
                        // ⚠ Carried across the handover for the same reason the profile is: a
                        // baseline that reverted to D65 here would shift the white balance of
                        // every RAW the moment the full-resolution decode landed.
                        s.wb_baseline,
                        move || ctx2.request_repaint(),
                    );
                    // ⚠ Replaced with the engine's, or the eyedropper would read the half-size
                    // buffer at full-resolution coordinates.
                    s.buffer = decoded.buffer.clone();
                    s.buffer_is_full_res = true;
                    // ⚠ Load-bearing. Region rects are in *rendered* pixels, and those
                    // coordinates change with the buffer — reusing them would describe the
                    // wrong part of the image. Clearing both forces the next frame to
                    // re-request against the new resolution.
                    s.rendered_region = None;
                    s.requested_region = None;
                    s.render_rung = s.fit_cap();
                    // ⚠ Ask for a frame explicitly. At the fit rung the canvas would otherwise
                    // see an unchanged rung and a covered region and request nothing at all —
                    // leaving the old buffer's texture on screen, so the upgrade would be
                    // invisible until something else happened to trigger a render.
                    s.request(s.view_edits(tool), s.fit_cap(), None);
                    // ⚠ Only worth saying where it is visible. A full-resolution decode can
                    // finish while the user is browsing the Library, where the message means
                    // nothing and merely displaces something that does.
                    if self.view == AppView::Develop {
                        self.status = "Full resolution ready".to_string();
                    }
                }
                Err(e) => {
                    // The ladder simply tops out at the half-size buffer, i.e. the behaviour
                    // before this feature existed.
                    self.status = format!("Full-resolution decode failed: {e}");
                }
            }
        }
    }

    /// Per-frame pump: fire the debounce timer, then absorb finished decodes.
    pub(crate) fn pump_decodes(&mut self, ctx: &egui::Context) {
        if let Some(p) = &mut self.pending {
            if p.seq.is_none() {
                let now = Instant::now();
                if now >= p.start_at {
                    let path = p.id.path.clone();
                    // `p` borrows `self.pending`, the call borrows `self.decoder` — disjoint
                    // fields, so the borrow checker allows both at once.
                    p.seq = Some(self.decoder.request(path, PREVIEW_MAX_DIM));
                } else {
                    ctx.request_repaint_after(p.start_at - now);
                }
            }
        }

        for res in self.decoder.poll() {
            let is_current = self.pending.as_ref().and_then(|p| p.seq) == Some(res.seq);
            match res.outcome {
                Ok(decoded) => {
                    // Cache even superseded decodes — that work is already paid for.
                    self.cache.insert(res.path.clone(), decoded.clone());
                    if is_current {
                        // ⚠ The **pending** id, not `res.path`: the decode is keyed by file, but
                        // which *copy* was being opened is only known here.
                        let id = self
                            .pending
                            .take()
                            .map(|p| p.id)
                            .unwrap_or_else(|| PhotoId::original(res.path.clone()));
                        self.finalize_session(ctx, id, decoded);
                    }
                }
                Err(e) => {
                    if is_current {
                        self.pending = None;
                        self.status = format!("Decode failed: {e}");
                    }
                }
            }
        }
    }

    pub(crate) fn open_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images",
                &[
                    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "jpg", "jpeg", "png",
                    "tif", "tiff",
                ],
            )
            .pick_file()
        else {
            return;
        };
        // ⚠ Single-file mode has no Library, so no copies are listed — open the original.
        // Its sidecar may still hold copies; they simply are not browsable until a folder is
        // opened, which is the same limitation single-file mode already has for the filmstrip.
        self.open_path(ctx, PhotoId::original(path));
        self.library = None;
        self.view = AppView::Develop;
    }

    pub(crate) fn open_entry(&mut self, ctx: &egui::Context, index: usize) {
        let id = match self.library.as_mut() {
            Some(lib) => match lib.entries.get(index) {
                Some(e) => {
                    lib.selection.click(index);
                    e.id.clone()
                }
                None => return,
            },
            None => return,
        };
        self.open_path(ctx, id);
        self.view = AppView::Develop;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    #[test]
    fn the_before_view_keeps_the_geometry_and_drops_the_adjustments() {
        // ⚠⚠ The property the whole before/after rework rests on. A *fully* neutral render is
        // uncropped, so it has different dimensions from the edited one — the region and
        // placement maths would then describe different content and toggling while zoomed
        // would jump elsewhere in the photo. Keeping the framing is what makes the comparison
        // a comparison.
        let mut edits = Edits {
            exposure: 1.5,
            ..Default::default()
        };
        edits.geometry.crop.left = 0.25;
        edits.geometry.angle = 3.0;
        edits.vignette.amount = -40.0;

        let before = super::before_edits(&edits);
        assert_eq!(before.geometry.crop.left, 0.25, "the crop must survive");
        assert_eq!(before.geometry.angle, 3.0, "the straighten must survive");
        assert_eq!(before.exposure, 0.0, "the adjustments must not");
        assert_eq!(before.vignette.amount, 0.0);
    }

    /// ⚠⚠ **The bug this reproduces: a thumbnail reverted the moment you navigated away.**
    /// `thumb_request` prefers the live session's edits only while the photo is open; every other
    /// row renders through `PhotoEntry::edits`, which `entries::build` read at folder-open time.
    /// So editing a photo updated its filmstrip thumbnail immediately and then showed the *old*
    /// state as soon as you arrowed to the next one — reported 2026-09-02 for a geometry reset,
    /// and true of every edit.
    ///
    /// ⚠ Asserted through `thumb_request` rather than on the field, because the request's key is
    /// what actually decides which thumbnail is shown: a test on `entries[i].edits` alone would
    /// pass against a `thumb_request` that ignored it.
    #[test]
    fn an_edited_photo_keeps_its_new_thumbnail_after_you_navigate_away() {
        let dir = test_app::TestDir::new("staleedits");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let mut lib = test_app::library_of(1);
        lib.entries[0].id = rustroom_app::photo_id::PhotoId::original(photo.clone());
        lib.entries[0].edits = Edits::default();
        app.library = Some(lib);

        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        if let Some(s) = &mut app.session {
            s.path = photo.clone();
            s.edits.exposure = 1.75;
        }

        // While the photo is open the live edits win, so the thumbnail is already right.
        let live = app.live_edits();
        let open_key = app
            .library
            .as_ref()
            .unwrap()
            .thumb_request(0, live.as_ref());
        assert_eq!(
            open_key.edits.exposure, 1.75,
            "fixture check: live edits apply"
        );

        // Navigating away persists the session — and must carry the edits into the row.
        app.persist_current_session();
        app.session = None;

        let away = app.library.as_ref().unwrap().thumb_request(0, None);
        assert_eq!(
            away.edits.exposure, 1.75,
            "the thumbnail reverted to the pre-edit state after navigating away"
        );
    }

    /// ⚠ And the other direction: a photo nobody edited must not pick up somebody else's edits.
    /// An assertion that only checked "the edited row updated" would pass against a refresh that
    /// wrote the same edits into every row.
    #[test]
    fn refreshing_one_row_leaves_the_others_alone() {
        let dir = test_app::TestDir::new("staleothers");
        let a = dir.0.join("a.RAF");
        let b = dir.0.join("b.RAF");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        sidecar::persist_copy(
            &a,
            0,
            &Edits {
                exposure: 2.0,
                ..Default::default()
            },
        )
        .unwrap();

        let mut app = test_app::for_test();
        let mut lib = test_app::library_of(2);
        lib.entries[0].id = rustroom_app::photo_id::PhotoId::original(a.clone());
        lib.entries[1].id = rustroom_app::photo_id::PhotoId::original(b.clone());
        app.library = Some(lib);

        app.refresh_entry_edits(&[a.clone(), b.clone()]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.entries[0].edits.exposure, 2.0,
            "a's sidecar was not read"
        );
        assert_eq!(
            lib.entries[1].edits.exposure, 0.0,
            "b has no sidecar and must stay neutral"
        );
    }

    /// A `Decoded` with nothing behind it, so `finalize_session` can be driven directly.
    /// ⚠⚠ Drives the **real** `finalize_session`, not a hand-assigned field. Read once when the
    /// photo opens and never in a panel body — the `apply_view_demosaic` rule; `Session::name`
    /// caches the copy name from the same envelope for the same reason.
    #[test]
    fn opening_a_photo_caches_its_snapshots_and_its_name() {
        let dir = test_app::TestDir::new("sessionsnaps");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let mut sc = rustroom_app::sidecar::Sidecar::default();
        sc.set_edits(1, Edits::default());
        sc.set_label(1, Some("Warm".to_string()));
        sc.add_snapshot(
            1,
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Before the crop".to_string()),
                taken: 0,
                edits: Edits::default(),
            },
        );
        rustroom_app::sidecar::save_all(&photo, &sc).unwrap();

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.finalize_session(
            &ctx,
            PhotoId::new(photo.clone(), 1),
            test_app::fake_decode(),
        );

        let s = app.session.as_ref().expect("the session must exist");
        assert_eq!(s.name.as_deref(), Some("Warm"), "the name still caches");
        assert_eq!(s.snapshots.len(), 1, "and so do the snapshots");
        assert_eq!(s.snapshots[0].label(), "Before the crop");
    }

    /// ⚠ Snapshots are **per copy**: opening copy 0 must not show copy 1's.
    #[test]
    fn a_session_caches_only_its_own_copys_snapshots() {
        let dir = test_app::TestDir::new("percopysnaps");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let mut sc = rustroom_app::sidecar::Sidecar::default();
        sc.set_edits(0, Edits::default());
        sc.add_snapshot(
            1,
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: None,
                taken: 0,
                edits: Edits::default(),
            },
        );
        rustroom_app::sidecar::save_all(&photo, &sc).unwrap();

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );

        assert!(
            app.session.as_ref().unwrap().snapshots.is_empty(),
            "copy 0 has none of its own"
        );
    }

    /// ⚠⚠ **Requesting a render must not move the guide overlay.** `shown_geo` describes the
    /// texture on screen and may only advance when a frame actually lands; `pending_geo` is what
    /// was asked for. Collapsing the two puts guides at their new positions over the previous
    /// image — reported as the line jumping on release when fully zoomed in, where a perspective
    /// change drops the prefix cache and the re-render takes hundreds of milliseconds.
    #[test]
    fn a_render_request_records_what_was_asked_for_without_moving_the_overlay() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let s = app.session.as_mut().unwrap();
        s.shown_geo = (edit_model::Perspective::default(), 0.0);

        let mut e = s.edits.clone();
        e.perspective.vertical = 40.0;
        e.geometry.angle = 2.5;
        s.request(e, 512, None);

        assert_eq!(
            s.pending_geo.get(),
            (
                edit_model::Perspective {
                    vertical: 40.0,
                    ..Default::default()
                },
                2.5
            ),
            "the request must be recorded"
        );
        assert_eq!(
            s.shown_geo,
            (edit_model::Perspective::default(), 0.0),
            "but what is on screen has not changed yet"
        );
    }

    /// ⚠⚠ **The regression this shipped, reported in use 2026-09-01: crop a photo, move a slider,
    /// and the image goes soft until you nudge the zoom.**
    ///
    /// `render_max_dim` used to return `fit_cap()` whenever `requested_region` was `None`, on the
    /// reasoning that "no region" meant "the fit rung". That stopped being true when the fit
    /// clamp was removed: once `fit` may exceed 1.0 the **whole image** can be visible at a
    /// native rung. A crop makes the image smaller, so `fit` is larger and the ladder climbs,
    /// while the whole cropped frame still fits on screen — so the canvas stores a native rung
    /// with no region, and every panel-driven render then asked for half the resolution being
    /// displayed.
    ///
    /// ⚠ The canvas could not correct it: from its side neither the rung nor the region had
    /// changed, so it re-requested nothing. That is why it persisted until a zoom crossed a rung
    /// boundary.
    #[test]
    fn a_panel_render_uses_the_rung_the_canvas_chose_even_with_no_region() {
        let ctx = egui::Context::default();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 1200, 900);
        let s = app.session.as_mut().expect("a session");
        let fit = s.fit_cap();

        // What the canvas stores for a cropped photo whose whole frame is still on screen.
        s.requested_region = None;
        s.render_rung = fit * 2;

        assert_eq!(
            s.render_max_dim(),
            fit * 2,
            "a panel edit must re-render at the rung on screen, not fall back to the fit cap"
        );

        // And the ordinary fit case is unchanged.
        s.render_rung = fit;
        assert_eq!(s.render_max_dim(), fit);
    }
}

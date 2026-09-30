//! Opening a photo and keeping its [`Session`] current: the decode handover, the
//! full-resolution upgrade, and the per-frame pumps that drive both.
//!
//! Split out of `main.rs` unchanged. ⚠ `Session` still means "fully decoded" — the
//! half-open state lives in [`PendingOpen`], and nothing downstream knows about it.

use crate::{AppView, PendingOpen, RustroomApp, Session, DECODE_DEBOUNCE, PREVIEW_MAX_DIM};
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::decode_worker::{Decoded, ProfileParts};
use rustroom_app::full_res;
use rustroom_app::history::History;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::{FrameSpec, Proof, RegionRect, RenderEngine};
use rustroom_app::sidecar;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Edits to render for the preview. In Geometry mode the crop is reset to the full frame so
/// the whole straightened image shows under the crop overlay.
///
/// ⚠ **Only the crop.** The orientation must survive, or the preview would show a
/// differently-oriented image from the one being edited — and the crop overlay would then be
/// drawn against a frame that does not match it.
pub(crate) fn preview_edits(edits: &Edits, tool: Tool) -> Edits {
    let mut e = edits.clone();
    if tool.is_geometry() {
        e.geometry.crop = edit_model::CropRect::default();
    }
    e
}

/// The photo as it was before any adjustment, at the **current framing**.
///
/// ⚠ Geometry survives on purpose — see [`Session::view_edits`]. A free function rather than a
/// method so it can be tested without building a `Session`, which needs a decoded buffer and a
/// texture.
pub(crate) fn before_edits(edits: &Edits) -> Edits {
    // ⚠ The camera rendering and the lens correction stay: "before" is how the file came in,
    // profile and all, so an untouched RAW shows no change. Dropping them made Before a flat,
    // colorimetric frame nobody had ever seen (P-08).
    Edits {
        geometry: edits.geometry,
        rendering: edits.rendering,
        lens_correction: edits.lens_correction,
        ..Default::default()
    }
}

/// How far past the visible area to render, in **rendered** pixels, so a pan does not
/// immediately leave what has been rendered.
///
/// ⚠⚠ **Measured, not chosen.** `bench_pan_rung` (kept) sweeps it at the native rung on an X-T4,
/// with the prefix cached — which is the whole per-frame cost of a pan, since panning changes no
/// edits:
///
/// | margin | region | tail | pan before a re-render |
/// |---|---|---|---|
/// | 0 | 953x541 | 7.3 ms | 0 pt |
/// | 64 | 1081x669 | 9.3 ms | 146 pt |
/// | **256** | **1465x1053** | **23.5 ms** | **583 pt** |
/// | 512 | 1977x1565 | 36.7 ms | 1165 pt |
/// | 1024 | 3001x2589 | 97.2 ms | 2330 pt |
///
/// ⚠ **The area grows as `(w + 2m)(h + 2m)` while the distance it buys grows only as `m`**, so
/// this cannot simply be raised until panning is free — 1024 costs 97 ms for four times the
/// travel of 256. 256 buys about a quarter of a 2167 pt canvas.
///
/// ⚠ **It is paid by every render at this rung, not only by pans.** That is affordable because a
/// slider drag drops a rung, where the same margin in rendered pixels covers four times the
/// screen area and costs about a quarter; the full-rung figure is paid on settle and on crossing
/// a pan boundary, not per frame.
const PAN_MARGIN: usize = 256;

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
    /// The [`pipeline::RenderScale`] the worker will use for a render at `max_dim`.
    ///
    /// ⚠ Delegates to the engine's own function rather than re-deriving it. The canvas needs this
    /// to size its region request, and a second derivation would be two facts that can disagree —
    /// the shape `render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for.
    pub(crate) fn render_scale(&self, max_dim: usize) -> pipeline::RenderScale {
        rustroom_app::render_engine::scale_from_full(self.full_long, self.buffer_long, max_dim)
    }

    /// How far beyond the visible rect a region render must reach, in **rendered** pixels.
    ///
    /// ⚠⚠ Derived per render from the edits, because no constant can do it: masks carry spatial
    /// adjustments and **chain**, so N masked clarities reach N x 120 px and the stack is
    /// unbounded. This replaced `view_region::REGION_MARGIN`, which had 57 px of slack against a
    /// worst case that masks made meaningless.
    ///
    /// ⚠ Takes the edits **actually being rendered** rather than `self.edits`, so a before/after
    /// view reserves what it draws. The failure direction is safe either way — `view_edits` only
    /// ever drops adjustments, never adds — but reserving the truth is cheaper than reserving a
    /// bound.
    pub(crate) fn render_margin(&self, edits: &Edits, max_dim: usize) -> usize {
        // ⚠⚠ **Two jobs, and the `max` is what keeps them from trading against each other.** The
        // kernel radius is a **correctness** floor: render less than the tail reads and the edge
        // of the visible area is a subtly wrong band with nothing to error. `PAN_MARGIN` is a
        // **comfort** term on top, so an ordinary pan does not leave the rendered region and see
        // the backdrop. Reducing the first to afford the second would be silently wrong.
        pipeline::tail_kernel_radius(edits, self.render_scale(max_dim)).max(PAN_MARGIN)
    }

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
                intent: self.canvas_intent,
                resampler: self.resampler,
                gpu_tail: self.gpu_tail,
                visualize: self.visualize,
                // ⚠ Built here rather than stored assembled, so "warn against nothing" is not
                // representable: the flag only reaches the render when a space is being proofed
                // into. Two independent toggles on the session, one value on the wire.
                soft_proof: self.soft_proof.map(|space| Proof {
                    space,
                    intent: self.canvas_intent,
                    warn: self.gamut_warning,
                }),
                grade_view: self.grade_view,
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
        let mut e = preview_edits(&edits, tool);
        // ⚠ Removed rather than zeroed: a spot with a zero radius still runs the stage, and
        // `SpotRemovalStage`'s own guard treats a non-positive radius as covering nothing — so
        // dropping it is both cheaper and unambiguous.
        // ⚠ The held-out patches (the rows' toggles) and the drag-suppressed one, highest index
        // first so each removal leaves the lower indices meaning what they meant.
        let mut gone: Vec<usize> = self
            .spot_held
            .iter()
            .take(e.spots.len())
            .enumerate()
            .filter(|(_, &held)| held)
            .map(|(i, _)| i)
            .collect();
        if let Some(i) = self.suppressed_spot {
            if i < e.spots.len() {
                gone.push(i);
            }
        }
        gone.sort_unstable();
        gone.dedup();
        for i in gone.into_iter().rev() {
            e.spots.remove(i);
        }
        // M9.11: the eye being moved or resized leaves the view too, `suppressed_spot`'s reason.
        if let Some(i) = self.suppressed_eye {
            if i < e.eyes.len() {
                e.eyes.remove(i);
            }
        }
        // ⚠⚠ **Mask isolation.** While a mask's tint is shown, that mask's adjustments leave the
        // view, so painting changes no pixels and needs no render — which is what keeps the rung
        // from changing mid-stroke. Every other mask and every global edit still render.
        //
        // ⚠ Removed rather than neutralised, `suppressed_spot`'s reasoning: a mask contributes to
        // the picture only through its adjustments, so the two are the same thing and removal
        // cannot be half-done.
        e
    }

    /// Keep `mask_shown` the masks' length. A mask that arrived by undo, paste or a snapshot
    /// starts with its overlay off; the panel turns it on for a mask it adds itself.
    pub(crate) fn sync_mask_shown(&mut self) {
        self.mask_shown.resize(self.edits.masks.len(), false);
    }

    /// Drop every patch hold-out; true if any was on.
    ///
    /// ⚠ Patches only: a switched-off mask is an edit (`MaskedAdjustment::disabled`) and stays
    /// off across tools, in the export and in the sidecar — that is the point of it.
    pub(crate) fn release_hold_outs(&mut self) -> bool {
        let any = self.spot_held.iter().any(|&h| h);
        self.spot_held.iter_mut().for_each(|h| *h = false);
        any
    }

    /// The masks whose overlay is shown, as indices into `edits.masks`.
    pub(crate) fn shown_masks(&self) -> Vec<usize> {
        self.mask_shown
            .iter()
            .take(self.edits.masks.len())
            .enumerate()
            .filter(|(_, &on)| on)
            .map(|(i, _)| i)
            .collect()
    }

    /// Whether every mask's overlay is shown (false with no masks at all).
    pub(crate) fn all_masks_shown(&self) -> bool {
        let n = self.edits.masks.len();
        n > 0 && self.shown_masks().len() == n
    }

    /// Show or hide every mask's overlay.
    pub(crate) fn set_all_masks_shown(&mut self, on: bool) {
        self.mask_shown = vec![on; self.edits.masks.len()];
    }

    /// Keep the patch flags the patches' length: a new patch is shown and not held out.
    pub(crate) fn sync_spot_flags(&mut self) {
        let n = self.edits.spots.len();
        self.spot_shown.resize(n, true);
        self.spot_held.resize(n, false);
    }

    /// Whether every patch's outline is shown (false with no patches at all).
    pub(crate) fn all_spots_shown(&self) -> bool {
        let n = self.edits.spots.len();
        n > 0 && self.spot_shown.len() >= n && self.spot_shown.iter().take(n).all(|&on| on)
    }

    /// Show or hide every patch's outline.
    pub(crate) fn set_all_spots_shown(&mut self, on: bool) {
        self.spot_shown = vec![on; self.edits.spots.len()];
    }

    /// Which component of the selected mask the panel and the canvas act on, **clamped**.
    ///
    /// ⚠⚠ **One read rule, so a stale index cannot make the panel and the canvas disagree.**
    /// `selected_component` is reset when a different mask is picked and clamped when a component
    /// is deleted, but the mask list can also shrink under it — `Remove all masks`, a snapshot
    /// restore, a paste, a sync — and a site that forgets degrades to *the base is selected*
    /// rather than to the sliders editing one shape while the handles drag another.
    ///
    /// ⚠ `0` whenever nothing is selected: the base always exists, so there is no empty state.
    pub(crate) fn component_index(&self) -> usize {
        let Some(i) = self.selected_mask else {
            return 0;
        };
        let n = self.edits.masks.get(i).map_or(0, |m| m.refine.len());
        self.selected_component.min(n)
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
        // ⚠⚠ M9.7: the history goes WITH the sidecar, here, and not in `eframe::App::on_exit` —
        // which was tried first and NEVER RUNS: closing the window (⌘Q included) ends in
        // `std::process::exit(0)` in `shell.rs`, the workaround for a macOS teardown crash, right
        // after calling this. Found by use: the catalog held zero rows. Written together, the
        // stored history always ends at exactly the edits on disk, which is what a restore checks.
        self.persist_history(&PhotoId::new(path.clone(), s.copy), &s.history);
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
        // ⚠⚠ **An Immich photo may not be on disk yet**, because the browse grid builds rows for
        // cache paths that are known before the file exists. Enqueue the download and return; the
        // worker's Ok outcome re-enters this function with the file in place, so there is **one**
        // opening path rather than two.
        //
        // ⚠ Before `persist_current_session`: nothing about the current session changes when we
        // decline to switch away from it. The persist happens on the second entry, which is a real
        // switch.
        //
        // ⚠⚠ **Scoped to the Immich cache deliberately, and an absent *local* file still falls
        // through to the behaviour it always had.** Reporting "not there" for any missing file
        // reads as an improvement and is not this change's business: `open_path` is reached from
        // ten places including `undo` and `snapshots`, where an early return would silently skip
        // an operation the user asked for. Widening it is its own change, with its own reasoning.
        if !id.path.is_file() {
            if let Some(job) = self.immich_download_for(&id.path) {
                self.immich.request(job);
                self.status = "Downloading from Immich…".to_string();
                return;
            }
        }
        // M9.14: the photo being left — if edited and without a preview, it is built behind you.
        let leaving = self
            .session
            .as_ref()
            .map(|s| (PhotoId::new(s.path.clone(), s.copy), s.edits.clone()));
        self.persist_current_session();
        if let Some((left, edits)) = leaving {
            if left != id {
                self.queue_leave_build(left, edits);
            }
        }

        // ⚠ The cache is keyed by **path**, so opening a copy of an already-decoded file is a
        // hit — which is exactly M3.4's "no storage cost".
        if let Some(decoded) = self.cache.get(&id.path) {
            self.pending = None;
            self.finalize_session(ctx, id, decoded);
            return;
        }

        // 2026-09-29: a **ghost session** at the click — the cached 512-px preview as the
        // editing buffer, so the panel, the histogram, the guides and the zoom keys exist now
        // rather than ~1.5 s later when the half decode lands and replaces the buffer
        // (`adopt_decoded`). Nothing is cached from it; a miss simply waits as before.
        let ghost = rustroom_app::thumb_store::Identity::of(&id.path)
            .and_then(|ident| self.ghost_store.get(&ident));
        if let Some(thumb) = ghost {
            let decoded = rustroom_app::ghost::ghost_decoded(&thumb, PREVIEW_MAX_DIM);
            self.finalize_session_with_view(ctx, id.clone(), decoded, (1.0, egui::Vec2::ZERO));
            if let Some(s) = self.session.as_mut() {
                s.buffer_is_ghost = true;
                // ⚠ Whatever the format: a JPEG file's ghost is 512 px, not the file.
                s.buffer_is_full_res = false;
            }
        }

        let placeholder = self.thumbnail_for(&id);
        let path = id.path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.status = format!("Decoding {name}…");
        if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
            eprintln!("open  {:8.1} ms  pending {name}", crate::debug_ms());
        }
        // ⚠ The sidecar's edits through the same defaults a session gets, so the request's
        // untouched flag agrees with what `finalize_session` will hold a second later.
        let preview_request = self.preview_request(
            id.clone(),
            self.settings
                .initial_edits(sidecar::load_copy(&id.path, id.copy)),
        );
        self.pending = Some(PendingOpen {
            id,
            placeholder,
            start_at: Instant::now() + DECODE_DEBOUNCE,
            seq: None,
            preview_request,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit_long: 0.0,
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

    /// Store `history` in the catalog as `id`'s, or drop its row when it holds nothing to undo or
    /// redo (M9.7). A catalog error costs only the history, never the edits, so it is logged and
    /// swallowed.
    pub(crate) fn persist_history(&self, id: &PhotoId, history: &History<Edits>) {
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let result = if rustroom_app::history_store::worth_keeping(history) {
            cat.save_history(
                &id.path,
                id.copy,
                &rustroom_app::history_store::encode(history),
            )
        } else {
            cat.delete_history(&id.path, id.copy)
        };
        if let Err(e) = result {
            eprintln!("history: could not store {}: {e}", id.path.display());
        }
    }

    /// `id`'s history from a previous run, if the catalog holds one whose present is exactly
    /// `on_disk` (M9.7).
    ///
    /// ⚠⚠ The `HistoryStash::take_matching` rule, for its reason: a sync, a paste, a preset or
    /// another tool may have rewritten the sidecar since, and a history ending anywhere else would
    /// undo into a state the photo never passed through. Such a row is deleted, not kept.
    pub(crate) fn restored_history(&self, id: &PhotoId, on_disk: &Edits) -> Option<History<Edits>> {
        let cat = self.catalog.as_ref()?;
        let text = cat.load_history(&id.path, id.copy).ok().flatten()?;
        match rustroom_app::history_store::decode(&text) {
            Some(h) if h.get() == on_disk => Some(h),
            _ => {
                let _ = cat.delete_history(&id.path, id.copy);
                None
            }
        }
    }

    /// Build the editing `Session` from a decoded photo (cache hit or worker result).
    pub(crate) fn finalize_session(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        decoded: Arc<Decoded>,
    ) {
        self.finalize_session_with_view(ctx, id, decoded, (1.0, egui::Vec2::ZERO));
    }

    /// `finalize_session` with the view the pending preview was left at — the zoom and pan the
    /// user made while the decode ran carry over, so the picture does not jump back to fit.
    pub(crate) fn finalize_session_with_view(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        decoded: Arc<Decoded>,
        view: (f32, egui::Vec2),
    ) {
        let path = id.path.clone();
        // ⚠ The only place `self.session` is ever assigned, and so the only choke point the
        // stash needs: the outgoing history is put away here whether the switch came from a
        // cache hit or from a finished background decode.
        let stale = self.session.as_ref().and_then(|old| old.texture.clone());
        if let Some(old) = self.session.take() {
            let old_id = PhotoId::new(old.path, old.copy);
            // ⚠ Persisted at the same choke point it is stashed at (M9.7), so the catalog and the
            // stash cannot disagree about which photos have a history.
            self.persist_history(&old_id, &old.history);
            self.histories.stash(old_id, old.history);
        }
        // ⚠ Scanned here, once per photo, rather than in the panel body which runs every frame.
        // Cheap enough per photo; ruinous sixty times a second.
        self.camera_profiles_cache = camera_profile::store::ProfileStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        )
        .list_for(&decoded.label);

        // ⚠ The preferences apply only to a photo with **no sidecar**. One already edited keeps
        // exactly what it had, which is what lets `Edits::default()` stay strictly neutral while
        // fresh photos still open corrected and rendered — see `Settings::initial_edits`.
        let stored = sidecar::load_copy(&path, id.copy);
        let edits = self.settings.initial_edits(stored);
        let history = match self.histories.take_matching(&id, &edits) {
            Some(h) => h,
            None => {
                // Either this photo has no stack, or the one it had describes a state no
                // longer on disk — a sync or a paste rewrote its sidecar while it was closed.
                // Any log entry pointing at that stack would now pop steps it does not hold.
                self.actions.forget_edits_of(&id);
                // ⚠ M9.7: a history from a previous run, if the catalog holds one that still
                // describes the edits on disk — the stash's own rule, applied to it.
                match self.restored_history(&id, &edits) {
                    Some(h) => {
                        self.actions.seed(&id, h.undo_depth(), h.redo_depth());
                        h
                    }
                    None => History::new(edits.clone()),
                }
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

        // ⚠ Scanned once per photo, beside the camera profiles and for the same reason: the
        // panel body runs sixty times a second and must not read a directory.
        let look_store = rustroom_app::looks::LookStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        );
        self.looks.refresh(&look_store);
        // ⚠ Resolved once, here: this is where a sidecar or a preset can have brought a look
        // that is not on this machine, which is the case the spec's §9.2 requires be *said*.
        self.looks.sync(edits.look.file.as_deref(), &look_store);
        let mut profile = decoded.profile.clone();
        profile.looks_dir = Some(look_store.dir().to_path_buf());

        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(
            decoded.buffer.clone(),
            decoded.source_scale,
            decoded.lens.shot.clone(),
            decoded.wb_baseline,
            profile,
            decoded.scene_referred,
            move || ctx2.request_repaint(),
        );
        let buffer_long = decoded.buffer.width().max(decoded.buffer.height());
        // ⚠ Derived once from the buffer and its scale, so it is the same number before and after
        // the full-resolution handover — see `Session::full_long`.
        let full_long = if decoded.source_scale > 0.0 {
            (buffer_long as f32 / decoded.source_scale).round() as usize
        } else {
            buffer_long
        };
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
                intent: self.settings.canvas_intent,
                resampler: self.settings.resampler,
                gpu_tail: self.settings.gpu_tail,
                visualize: None,
                // ⚠ The proof is per session and a session is per photo, so a photo always opens
                // unproofed — matching the field's initialiser below rather than reaching for a
                // state that does not exist yet.
                soft_proof: None,
                grade_view: None,
            },
        );
        // Non-RAW formats are decoded at full size, so their buffer is already the real thing.
        let buffer_is_full_res = !catalog::is_raw(&path);

        self.session = Some(Session {
            display_space: self
                .settings
                .effective_display_space(self.os_manages_colour),
            canvas_intent: self.settings.canvas_intent,
            resampler: self.settings.resampler,
            gpu_tail: self.settings.gpu_tail,
            visualize: None,
            show_grade_range: false,
            grade_view: None,
            grade_view_texture: None,
            straighten_armed: false,
            straighten_drag: None,
            straighten_line: None,
            readout_bytes: None,
            clip_texture: None,
            canvas_frame_rect: None,
            readout: None,
            // ⚠ Off on every photo, like `visualize` and for the same reason: the state lives on
            // the session, which is rebuilt per photo. Recorded as an accepted caveat rather than
            // an oversight.
            soft_proof: None,
            gamut_warning: false,
            path,
            copy: id.copy,
            // ⚠ Read from the sidecar rather than carried from the Library entry, so a session
            // opened without a Library (single-file mode) still knows its own name.
            name: envelope.label_of(id.copy).map(str::to_string),
            snapshots: envelope.snapshots_of(id.copy).to_vec(),
            engine,
            edits,
            texture: None,
            stale,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            lens: decoded.lens.clone(),
            wb_baseline: decoded.wb_baseline,
            baseline_ev: decoded.profile.baseline_ev,
            buffer: decoded.buffer.clone(),
            picking_wb: false,
            picking_fringe: false,
            zoom: view.0,
            fit_scale: 1.0,
            pan: view.1,
            history,
            show_before: false,
            active_handle: None,
            selected_mask: None,
            selected_component: 0,
            selected_spot: None,
            suppressed_spot: None,
            mask_shown: Vec::new(),
            spot_shown: Vec::new(),
            spot_held: Vec::new(),
            soft_next: false,
            backdrop: None,
            spot_drag: Default::default(),
            selected_eye: None,
            suppressed_eye: None,
            eye_drag: Default::default(),
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
            rendered_full: (1, 1),
            rendered_long: 1.0,
            buffer_long,
            full_long,
            buffer_is_full_res,
            buffer_is_ghost: false,
            opened_at: Instant::now(),
            // ⚠ `fit_cap`, not `PREVIEW_MAX_DIM`: they differ for an image smaller than the
            // cap, and this value is now what every panel-driven render asks for.
            render_rung: view_region::fit_cap(buffer_long, PREVIEW_MAX_DIM),
        });
        self.status.clear();
        if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
            eprintln!("open  {:8.1} ms  session ready", crate::debug_ms());
        }
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
        // ⚠ A ghost waits for its half decode first: two RAW decodes of one file at once would
        // only slow the one the user is waiting on.
        if !catalog::is_raw(&s.path) || s.buffer_is_full_res || s.buffer_is_ghost || in_flight {
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
        for res in self.full_decoder.poll() {
            let is_current = self.full_pending.as_ref().map(|(_, seq)| *seq) == Some(res.seq);
            if is_current {
                self.full_pending = None;
            }
            // A result for a photo we have since navigated away from is dropped: the buffer is
            // per-session and dies with it, so there is nothing to keep it in.
            let same_photo = self.session.as_ref().is_some_and(|s| s.path == res.path);
            if !is_current || !same_photo {
                continue;
            }
            match res.outcome {
                Ok(decoded) => {
                    self.adopt_decoded(ctx, &decoded, true);
                    if self.view == AppView::Develop {
                        self.status = "Full resolution ready".to_string();
                    }
                }
                Err(e) => {
                    self.status = format!("Full-resolution decode failed: {e}");
                }
            }
        }
    }

    /// Swap the open session's buffer for a better decode of the same photo, keeping its
    /// edits, history, view and texture — the half decode replacing a ghost, and the full
    /// decode replacing the half. `full_res` says which the new buffer is.
    ///
    /// ⚠ The engine is *replaced*, not kept alongside: once every rung renders from the new
    /// buffer the old one is dead weight. Dropping it disconnects its channel, so its worker
    /// thread returns on its own. Its prefix cache goes too — a one-off cost at the moment of
    /// upgrade.
    ///
    /// ⚠⚠ A ghost carried defaults for the lens, the white-balance baseline, the camera and the
    /// full size, so a ghost handover takes all of those from the decode; a half→full handover
    /// keeps the session's, which came from the same file's real decode — a baseline that
    /// reverted to D65 here would shift the white balance the moment the upgrade landed.
    pub(crate) fn adopt_decoded(
        &mut self,
        ctx: &egui::Context,
        decoded: &Arc<Decoded>,
        full_res: bool,
    ) {
        let tool = self.tool;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let was_ghost = s.buffer_is_ghost;
        let buffer_long = decoded.buffer.width().max(decoded.buffer.height());
        if was_ghost {
            s.lens = decoded.lens.clone();
            s.wb_baseline = decoded.wb_baseline;
            s.baseline_ev = decoded.profile.baseline_ev;
            s.make_model = decoded.label.clone();
            s.full_long = if decoded.source_scale > 0.0 {
                (buffer_long as f32 / decoded.source_scale).round() as usize
            } else {
                buffer_long
            };
            // ⚠ The ghost's rung was its own 512 px; the new buffer's ladder starts at its fit cap.
            s.render_rung = view_region::fit_cap(buffer_long, PREVIEW_MAX_DIM);
        }
        let ctx2 = ctx.clone();
        s.buffer_long = buffer_long;
        s.engine = RenderEngine::new(
            decoded.buffer.clone(),
            decoded.source_scale,
            s.lens.shot.clone(),
            s.wb_baseline,
            // ⚠⚠ **Not `decoded.profile` — see `handover_profile`.** The decode sets
            // `looks_dir: None` by design, so rebuilding straight from it dropped the looks
            // directory and every creative look stopped loading ~4 s after the photo opened.
            handover_profile(&decoded.profile, s.engine.looks_dir()),
            decoded.scene_referred,
            move || ctx2.request_repaint(),
        );
        // ⚠ Replaced with the engine's, or the eyedropper would read the old buffer at the new
        // buffer's coordinates.
        s.buffer = decoded.buffer.clone();
        s.buffer_is_full_res = full_res;
        s.buffer_is_ghost = false;
        s.stale = None;
        // ⚠ Load-bearing. Region rects are in *rendered* pixels, and those coordinates change
        // with the buffer — reusing them would describe the wrong part of the image. Clearing
        // both forces the next frame to re-request against the new resolution.
        s.rendered_region = None;
        s.requested_region = None;
        // ⚠ Ask for a frame explicitly, at the CURRENT rung: the canvas would otherwise see an
        // unchanged rung and a covered region and request nothing at all.
        s.request(s.view_edits(tool), s.render_rung, None);
        if was_ghost {
            self.camera_profiles_cache = camera_profile::store::ProfileStore::user_default(
                &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
            )
            .list_for(&decoded.label);
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
                    if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
                        eprintln!("open  {:8.1} ms  decode requested", crate::debug_ms());
                    }
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
                    if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
                        eprintln!(
                            "open  {:8.1} ms  decode landed (current={is_current})",
                            crate::debug_ms()
                        );
                    }
                    if is_current {
                        // ⚠ The **pending** id, not `res.path`: the decode is keyed by file, but
                        // which *copy* was being opened is only known here. The view made on the
                        // preview meanwhile comes along with it.
                        let (id, view) = self
                            .pending
                            .take()
                            .map(|p| (p.id, (p.zoom, p.pan)))
                            .unwrap_or_else(|| {
                                (PhotoId::original(res.path.clone()), (1.0, egui::Vec2::ZERO))
                            });
                        // A ghost session for this photo adopts the buffer and keeps its edits,
                        // history and view; without one the session is built now.
                        let ghost_here = self.session.as_ref().is_some_and(|s| {
                            s.buffer_is_ghost && s.path == id.path && s.copy == id.copy
                        });
                        if ghost_here {
                            let full = !catalog::is_raw(&id.path);
                            self.adopt_decoded(ctx, &decoded, full);
                            self.status.clear();
                        } else {
                            self.finalize_session_with_view(ctx, id, decoded, view);
                        }
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

/// The profile the render engine gets at the **full-resolution handover**.
///
/// ⚠⚠ **NOT `decoded.profile` — that is where the look silently died.** `decode.rs` sets
/// `looks_dir: None` deliberately ("the decode has no business knowing where looks live"), and
/// `finalize_session` fills it in afterwards. The handover rebuilt the engine straight from the
/// decode, so ~4 s after every RAW opened — `full_res::SETTLE` plus the decode — the engine lost
/// the directory and **every creative look stopped loading**, with no error and no visible cause.
/// Reported as selecting a look and moving Strength 0→100 changing nothing.
///
/// ⚠ The comment at that call site already warned that the profile must survive the handover; it
/// was written for the curve and the baseline, and the field added later was not covered by it.
/// This function exists so the rule is one expression a test can reach.
fn handover_profile(
    decoded: &ProfileParts,
    current_looks_dir: Option<&std::path::Path>,
) -> ProfileParts {
    let mut profile = decoded.clone();
    profile.looks_dir = current_looks_dir.map(|p| p.to_path_buf());
    profile
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    /// 2026-09-29: a cached thumbnail makes a **ghost session** at the click — the panel, the
    /// histogram and the zoom keys exist before the decode; the pending decode still runs.
    #[test]
    fn a_cached_thumbnail_makes_a_ghost_session_at_the_click() {
        let dir = crate::test_app::TestDir::new("ghost");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"raw").unwrap();
        let ident = rustroom_app::thumb_store::Identity::of(&photo).expect("identity");
        app.ghost_store.put(
            &ident,
            &core_image::Thumbnail {
                width: 6,
                height: 4,
                rgba: vec![128; 6 * 4 * 4],
            },
        );
        assert!(app.session.is_none(), "fixture check");

        app.open_path(&ctx, PhotoId::original(photo.clone()));

        let s = app.session.as_ref().expect("a ghost session at the click");
        assert!(s.buffer_is_ghost);
        assert!(!s.buffer_is_full_res);
        assert_eq!(s.path, photo);
        assert_eq!(s.buffer_long, 6, "the ghost's buffer is the thumbnail");
        assert!(app.pending.is_some(), "the real decode is still on its way");
        // And a photo with no cached thumbnail gets no ghost, as before.
        let other = dir.0.join("b.RAF");
        std::fs::write(&other, b"raw").unwrap();
        app.open_path(&ctx, PhotoId::original(other.clone()));
        assert!(
            app.session.as_ref().is_some_and(|s| s.path == photo),
            "without a thumbnail the previous session stays until the decode lands"
        );
    }

    /// The decode landing on a ghost keeps what the user did meanwhile — edits, view — and
    /// takes the facts the ghost only had defaults for from the real decode.
    #[test]
    fn adopting_a_decode_keeps_the_edits_and_the_view_and_drops_the_ghost() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 4, 2);
        {
            let s = app.session.as_mut().expect("session");
            s.buffer_is_ghost = true;
            s.buffer_is_full_res = false;
            s.make_model = String::new();
            s.edits.exposure = 0.7;
            s.zoom = 3.0;
            s.pan = egui::vec2(5.0, -7.0);
        }
        app.adopt_decoded(&ctx, &test_app::fake_decode(), false);
        let s = app.session.as_ref().expect("session");
        assert!(!s.buffer_is_ghost);
        assert!(
            !s.buffer_is_full_res,
            "a half decode is not full resolution"
        );
        assert_eq!(
            s.buffer_long, 8,
            "the decode's buffer replaced the thumbnail"
        );
        assert_eq!(s.edits.exposure, 0.7, "an edit made on the ghost survived");
        assert_eq!(
            (s.zoom, s.pan),
            (3.0, egui::vec2(5.0, -7.0)),
            "and so did the view"
        );
        assert_eq!(
            s.make_model, "Test Camera",
            "the camera comes from the real decode"
        );
        assert_eq!(s.full_long, 8);
    }

    /// ⚠⚠ **The handover must not throw a zoomed view back to fit resolution.** The explicit
    /// request is necessary — without it the canvas sees an unchanged rung and a covered region
    /// and asks for nothing, so the upgrade is invisible. Only the resolution was wrong: at 7.72x
    /// zoom `fit_cap` is an 8.9x magnified blur, measured held for 18 frames on every photo.
    #[test]
    fn the_handover_requests_at_the_current_rung() {
        let dir = crate::test_app::TestDir::new("handoverrung");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        let photo = dir.0.join("a.RAF");
        {
            let s = app.session.as_mut().expect("session");
            s.path = photo.clone();
            // A zoomed session, sitting on a rung above the fit cap.
            s.buffer_long = 3200;
            s.render_rung = 3200;
            // ⚠ Load-bearing, and omitting it made this test pass against the unfixed code: the
            // fixture opens with `buffer_is_full_res: true`, so the poll loop below breaks on its
            // first pump and the handover never runs. The fixture check then asserts a flag that
            // was already true and proves nothing. The sibling handover test resets it for the
            // same reason.
            s.buffer_is_full_res = false;
        }

        let seq = app.full_decoder.request(photo.clone(), 16);
        app.full_pending = Some((photo.clone(), seq));

        // ⚠ Deadline rather than an unbounded wait: a regression must fail, never hang CI.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            app.pump_full_res(&ctx);
            if app.session.as_ref().is_some_and(|s| s.buffer_is_full_res) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let s = app.session.as_ref().expect("session");
        assert!(
            s.buffer_is_full_res,
            "fixture check: the handover never ran, so this test proves nothing"
        );
        assert_ne!(
            s.render_rung,
            s.fit_cap(),
            "the handover reset the view to fit resolution — at zoom that is an 8.9x blur"
        );
    }

    /// A mask carrying one exposure value, so the tests below can tell masks apart by what they
    /// do rather than by position alone.
    fn masked_exposure(ev: f32) -> edit_model::MaskedAdjustment {
        let mut m = edit_model::MaskedAdjustment::default();
        m.adjustments.exposure = ev;
        m
    }

    /// ⚠ The rule: the suppressed mask leaves the view and **nothing else does**. A test that
    /// only checked the count would pass against a version that dropped the wrong one.
    #[test]
    fn held_patches_leave_the_view_together_with_the_drag_suppressed_one() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().expect("session");
        let spot = |x: f32| edit_model::Spot {
            centre: [x, 0.5],
            source: [0.1, 0.1],
            radius: 0.02,
            feather: 50.0,
            mode: edit_model::SpotMode::Heal,
        };
        s.edits.spots = vec![spot(0.2), spot(0.5), spot(0.8)];
        s.spot_held = vec![true, false, false];
        // ⚠ The drag suppression names the same patch as a held flag once: no double removal.
        s.suppressed_spot = Some(2);
        let left: Vec<f32> = s
            .view_edits(rustroom_app::tool::Tool::Retouch)
            .spots
            .iter()
            .map(|sp| sp.centre[0])
            .collect();
        assert_eq!(left, vec![0.5]);
        s.suppressed_spot = Some(0);
        let left: Vec<f32> = s
            .view_edits(rustroom_app::tool::Tool::Retouch)
            .spots
            .iter()
            .map(|sp| sp.centre[0])
            .collect();
        assert_eq!(left, vec![0.5, 0.8]);
        s.sync_spot_flags();
        assert!(!s.all_spots_shown() || s.spot_shown == vec![true, true, true]);
        s.set_all_spots_shown(false);
        assert!(!s.all_spots_shown());
        s.set_all_spots_shown(true);
        assert!(s.all_spots_shown());
    }

    #[test]
    fn shown_masks_follow_the_flags_and_the_mask_count() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().expect("session");
        s.edits.masks = vec![masked_exposure(0.5), masked_exposure(1.5)];
        s.sync_mask_shown();
        assert_eq!(
            s.mask_shown,
            vec![false, false],
            "a synced mask starts hidden"
        );
        assert!(!s.all_masks_shown());
        s.mask_shown[1] = true;
        assert_eq!(s.shown_masks(), vec![1]);
        s.set_all_masks_shown(true);
        assert!(s.all_masks_shown());
        // ⚠ A flag past the masks' end — left by a delete elsewhere — must not name a mask.
        s.mask_shown.push(true);
        assert_eq!(s.shown_masks(), vec![0, 1]);
        s.edits.masks.clear();
        s.sync_mask_shown();
        assert!(s.mask_shown.is_empty());
        assert!(!s.all_masks_shown(), "no masks is not \"all shown\"");
    }

    /// ⚠⚠ **The read rule for the selected component.** `selected_component` is reset when a
    /// different mask is picked and clamped when a component is deleted — but the mask list can
    /// also shrink under it (Remove all masks, a snapshot restore, a paste, a sync). Clamping at
    /// the *read* is what makes a site that forgets degrade to "the base is selected" rather than
    /// to the panel editing one shape while the canvas drags another.
    #[test]
    fn the_component_index_is_clamped_to_the_masks_own_components() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().expect("session");
        let mut m = masked_exposure(0.5);
        m.refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::default(),
        });
        s.edits.masks = vec![m];
        s.selected_mask = Some(0);

        s.selected_component = 1;
        assert_eq!(s.component_index(), 1, "a real component is returned as-is");

        s.selected_component = 7;
        assert_eq!(
            s.component_index(),
            1,
            "past the end clamps to the last component"
        );

        s.edits.masks[0].refine.clear();
        assert_eq!(
            s.component_index(),
            0,
            "a shrunken list falls back to the base"
        );

        s.selected_mask = None;
        assert_eq!(
            s.component_index(),
            0,
            "with no mask selected the answer is the base"
        );
    }

    #[test]
    fn the_handover_call_site_carries_the_looks_directory_onto_the_new_engine() {
        let dir = crate::test_app::TestDir::new("handoverlooks");
        let ctx = crate::test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);

        // An engine that knows where looks live, as `finalize_session` builds one.
        let photo = dir.0.join("a.RAF");
        let looks = dir.0.join("looks");
        {
            let s = app.session.as_mut().expect("session");
            s.path = photo.clone();
            s.buffer_is_full_res = false;
            s.engine = RenderEngine::new(
                s.buffer.clone(),
                1.0,
                None,
                color::wb::Illuminant::default(),
                ProfileParts {
                    looks_dir: Some(looks.clone()),
                    ..Default::default()
                },
                true,
                || {},
            );
            assert_eq!(
                s.engine.looks_dir(),
                Some(looks.as_path()),
                "fixture check: the engine must start with a looks directory"
            );
        }

        // Drive a real full-resolution decode through the injected stub, whose profile carries
        // `looks_dir: None` exactly as the real decode does.
        let seq = app.full_decoder.request(photo.clone(), 16);
        app.full_pending = Some((photo.clone(), seq));

        // ⚠ Deadline rather than an unbounded wait: a regression must fail, never hang CI.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            app.pump_full_res(&ctx);
            if app.session.as_ref().is_some_and(|s| s.buffer_is_full_res) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let s = app.session.as_ref().expect("session");
        assert!(
            s.buffer_is_full_res,
            "fixture check: the handover never ran, so this test proves nothing"
        );
        assert_eq!(
            s.engine.looks_dir(),
            Some(looks.as_path()),
            "the handover dropped the looks directory — every creative look stops loading"
        );
    }

    /// ⚠⚠ **The bug this reproduces: every creative look stopped working ~4 s after a RAW
    /// opened.** `full_res::SETTLE` is 800 ms and the full decode ~3 s, so the handover always
    /// landed before anyone could pick a look — which made it read as looks never working at all
    /// rather than as something that expired.
    ///
    /// ⚠ The decode cannot know where looks live and sets `looks_dir: None` by design, so a
    /// handover that takes the decode's profile verbatim is *guaranteed* to drop it. The session's
    /// engine already holds the resolved directory; carrying it is the whole fix.
    #[test]
    fn the_full_resolution_handover_keeps_the_looks_directory() {
        let dir = std::path::PathBuf::from("/looks");
        // What the decode always hands over: no directory, because it cannot know one.
        let decoded = ProfileParts {
            looks_dir: None,
            ..Default::default()
        };
        let carried = super::handover_profile(&decoded, Some(dir.as_path()));
        assert_eq!(
            carried.looks_dir.as_deref(),
            Some(dir.as_path()),
            "the handover dropped the looks directory, so every look stops loading"
        );
        // ⚠ And it must not invent one when the session had none, or a photo opened before any
        // look existed would start claiming a directory the session never resolved.
        let none = super::handover_profile(&decoded, None);
        assert_eq!(
            none.looks_dir, None,
            "the handover invented a looks directory"
        );
    }

    /// The region margin grows when the edits reach further.
    ///
    /// ⚠⚠ A fixed constant structurally cannot do this. Masks chain, so N masked clarities reach
    /// N x 120 px into the input and no constant covers an unbounded stack — which is why
    /// `REGION_MARGIN` is gone rather than merely enlarged.
    ///
    /// ⚠ The failure mode this guards is **silent**: too small a margin renders a subtly wrong
    /// band at the edge of the visible area, and nothing errors.
    #[test]
    fn the_render_margin_follows_the_edits_it_will_render() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 64, 48);
        let s = app.session.as_mut().expect("session");
        let rung = s.buffer_long;

        let plain = s.render_margin(&s.edits.clone(), rung);
        // ⚠⚠ **Three masked clarities, not one, and `PAN_MARGIN` is why.** The margin is
        // `max(kernel reach, PAN_MARGIN)`, so a single masked clarity's ~120 px reach sits under
        // the 256 px floor and this read `256 vs 256` — the guard could no longer see the
        // property it exists to guard. Masks **chain**, so three reach ~360 and clear it. The
        // floor never lowers the margin, so correctness was never at risk; what was at risk was
        // this test's ability to notice the derivation being removed.
        for _ in 0..3 {
            s.edits.masks.push(edit_model::MaskedAdjustment {
                mask: edit_model::Mask::default(),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    local_contrast: edit_model::LocalContrast {
                        texture: 0.0,
                        clarity: 100.0,
                    },
                    ..Default::default()
                },
                disabled: false,
            });
        }
        let masked = s.render_margin(&s.edits.clone(), rung);

        assert!(
            masked > plain,
            "the margin ignored a masked clarity: {masked} vs {plain}"
        );
    }

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

    /// ⚠ Before keeps the camera rendering and the lens correction: an untouched RAW must show
    /// the same picture in both halves, or "before" is a frame the user never saw (P-08).
    #[test]
    fn before_keeps_the_rendering_and_the_lens_state() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            lens_correction: Some(edit_model::LensCorrection::default()),
            exposure: 1.0,
            ..Default::default()
        };
        let before = super::before_edits(&edits);
        assert_eq!(before.rendering, Some(edit_model::Rendering::Camera));
        assert_eq!(
            before.lens_correction,
            Some(edit_model::LensCorrection::default())
        );
        assert_eq!(before.exposure, 0.0);
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

    /// M9.7 — the whole cycle through the REAL open path and a catalog FILE both instances share:
    /// edit three times, quit, start a new app on the same catalog, reopen, and Ctrl+Z walks back
    /// through the old edits in order.
    fn history_run(tag: &str) -> (test_app::TestDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = test_app::TestDir::new(tag);
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();
        rustroom_app::sidecar::save(&photo, &Edits::default()).unwrap();
        let db = dir.0.join("catalog.db");
        let cat = catalog::db::Catalog::open(&db).unwrap();
        cat.sync_folder(
            &dir.0,
            &[catalog::CatalogEntry {
                path: photo.clone(),
                file_name: "a.RAF".to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        (dir, photo, db)
    }

    fn app_on(db: &std::path::Path) -> crate::RustroomApp {
        let mut app = test_app::for_test();
        app.catalog = Some(catalog::db::Catalog::open(db).unwrap());
        app
    }

    fn exposure(app: &crate::RustroomApp) -> f32 {
        app.session.as_ref().unwrap().edits.exposure
    }

    /// Open `photo`, move exposure through 0.1, 0.2, 0.3 as three history steps, save the sidecar
    /// and quit.
    fn edit_and_quit(db: &std::path::Path, photo: &std::path::Path) {
        let ctx = test_ui::ctx();
        let mut app = app_on(db);
        let id = PhotoId::original(photo.to_path_buf());
        app.finalize_session(&ctx, id.clone(), test_app::fake_decode());
        for ev in [0.1, 0.2, 0.3] {
            let s = app.session.as_mut().unwrap();
            s.edits.exposure = ev;
            s.history.push(s.edits.clone());
            app.actions.push(
                rustroom_app::action_log::ActionKind::PhotoEdit { id: id.clone() },
                Some(&id),
            );
        }
        // ⚠⚠ The REAL quit path: `shell.rs` calls exactly this on a close request and then
        // `process::exit`s, so no `on_exit` hook ever runs. An earlier version of this test called
        // `on_exit` by hand and passed while the app saved nothing.
        app.persist_current_session();
    }

    #[test]
    fn a_photos_history_survives_a_restart() {
        let (_dir, photo, db) = history_run("history_restart");
        edit_and_quit(&db, &photo);

        let ctx = test_ui::ctx();
        let mut app = app_on(&db);
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );
        assert_eq!(
            exposure(&app),
            0.3,
            "the edits come from the sidecar as always"
        );
        let mut seen = Vec::new();
        for _ in 0..3 {
            app.undo_action(&ctx);
            seen.push(exposure(&app));
        }
        assert_eq!(
            seen,
            vec![0.2, 0.1, 0.0],
            "Ctrl+Z walks back through the old edits"
        );
        app.undo_action(&ctx);
        assert_eq!(exposure(&app), 0.0, "and stops at the start of the history");
        app.redo_action(&ctx);
        assert_eq!(exposure(&app), 0.1, "redo replays forward");
    }

    /// Switching photos stores the outgoing history — the stash choke point, not only `on_exit` —
    /// and a photo left with nothing to undo has no row.
    #[test]
    fn switching_photos_stores_the_outgoing_history() {
        let (dir, photo, db) = history_run("history_switch");
        let other = dir.0.join("b.RAF");
        std::fs::write(&other, b"not a real raw").unwrap();
        let ctx = test_ui::ctx();
        let mut app = app_on(&db);
        app.catalog
            .as_ref()
            .unwrap()
            .sync_folder(
                &dir.0,
                &[catalog::CatalogEntry {
                    path: other.clone(),
                    file_name: "b.RAF".to_string(),
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    edited: None,
                }],
            )
            .unwrap();
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );
        {
            let s = app.session.as_mut().unwrap();
            s.edits.exposure = 0.4;
            s.history.push(s.edits.clone());
        }
        app.finalize_session(
            &ctx,
            PhotoId::original(other.clone()),
            test_app::fake_decode(),
        );
        let cat = app.catalog.as_ref().unwrap();
        assert!(
            cat.load_history(&photo, 0).unwrap().is_some(),
            "a's history was stored"
        );
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );
        assert_eq!(
            app.catalog
                .as_ref()
                .unwrap()
                .load_history(&other, 0)
                .unwrap(),
            None,
            "b had nothing to undo, so it has no row"
        );
    }

    /// ⚠⚠ The sidecar changed between the two runs — a sync, a paste, another tool — so the stored
    /// history no longer ends where the photo is: it must be discarded, never undone into.
    #[test]
    fn a_history_that_no_longer_matches_the_sidecar_is_discarded() {
        let (_dir, photo, db) = history_run("history_stale");
        edit_and_quit(&db, &photo);
        rustroom_app::sidecar::save(
            &photo,
            &Edits {
                exposure: 1.5,
                ..Default::default()
            },
        )
        .unwrap();

        let ctx = test_ui::ctx();
        let mut app = app_on(&db);
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );
        app.undo_action(&ctx);
        assert_eq!(
            exposure(&app),
            1.5,
            "nothing to undo: the stale history was dropped"
        );
        assert_eq!(
            app.catalog
                .as_ref()
                .unwrap()
                .load_history(&photo, 0)
                .unwrap(),
            None,
            "and its row deleted"
        );
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

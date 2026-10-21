//! The photo open in Develop: the decoded `Session`, and the `PendingOpen` that stands in while it
//! decodes.

use edit_model::Edits;
use eframe::egui;
use rustroom_app::decode_worker::LensFacts;
use rustroom_app::history::History;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::{RegionRect, RenderEngine};
use rustroom_app::{crop_tool, mask_tool};
use std::path::PathBuf;
use std::time::Instant;

/// A photo whose decode is scheduled or in flight. `Session` still means "fully decoded",
/// so nothing downstream needs to know about this half-open state.
pub(crate) struct PendingOpen {
    pub(crate) id: PhotoId,
    /// The filmstrip thumbnail shown while decoding, when the folder is loaded.
    pub(crate) placeholder: Option<egui::TextureHandle>,
    /// Debounce deadline — the decode starts once `Instant::now()` passes this.
    pub(crate) start_at: Instant,
    /// The worker sequence number, once the decode has actually been requested.
    pub(crate) seq: Option<u64>,
    /// What to ask the preview loader for while the decode runs (M9.14): the camera JPEG or a
    /// rendered preview stands in for the thumbnail placeholder the moment it lands.
    pub(crate) preview_request: rustroom_app::preview_loader::PreviewRequest,
    /// The view the user has already made on the preview — zoom and pan work while the decode
    /// runs, and the session inherits them so nothing jumps when it lands (M9.14).
    pub(crate) zoom: f32,
    pub(crate) pan: egui::Vec2,
    /// The preview's fitted long edge in points at zoom 1, written by the canvas each frame —
    /// what ⌘1 divides the preview's pixel size by. Zero until the canvas has drawn once.
    pub(crate) fit_long: f32,
}

pub struct Session {
    pub(crate) path: PathBuf,
    /// Which copy of `path` this session is editing. `0` is the original.
    ///
    /// ⚠ Added beside `path` rather than replacing it with a `PhotoId`: `path` has many call
    /// sites that genuinely want the **file** — the decode, the buffer cache, the thumbnail.
    /// Only the sidecar, the undo scope and the label want the version.
    pub(crate) copy: u32,
    /// This copy's own name, when it has one. Always `None` on the original, whose name is the
    /// filename on disk.
    pub(crate) name: Option<String>,
    /// This copy's snapshots, cached from the sidecar when the photo opened.
    ///
    /// ⚠⚠ **Cached rather than read where it is drawn.** The strip lives in the panel body, which
    /// runs every frame; a `load_all` there would read the file sixty times a second — the
    /// `apply_view_demosaic` rule. `name` above is cached from the same envelope for the same
    /// reason.
    ///
    /// ⚠ Kept in step by the `RustroomApp::*_snapshot` methods, which are the only writers. A
    /// second place mutating the sidecar's snapshots without updating this is two facts that can
    /// disagree.
    pub(crate) snapshots: Vec<rustroom_app::sidecar::Snapshot>,
    pub(crate) engine: RenderEngine,
    pub(crate) edits: Edits,
    pub(crate) texture: Option<egui::TextureHandle>,
    /// The previous photo's last frame, shown dimmed while a ghost session waits for its
    /// preview — never the ghost's own 512-px render, which read as a low-resolution flash.
    pub(crate) stale: Option<egui::TextureHandle>,
    pub(crate) baseline: egui::TextureHandle,
    pub(crate) hist: [[u32; 256]; 3],
    pub(crate) make_model: String,
    /// The lens the shot was taken with, and the profile matched to it.
    ///
    /// ⚠ Kept on the session so the panel can name a lens it could *not* match. A miss is the
    /// commonest reason the controls do nothing, and the only way to act on it is to be told
    /// which name failed.
    pub(crate) lens: LensFacts,
    /// The editing buffer, shared with the render worker.
    ///
    /// ⚠ Kept **only** so the white-balance eyedropper can read a pixel *before* any stage has
    /// touched it: sampling the rendered texture would chase its own correction, since that
    /// texture already has the white balance applied. An `Arc` clone costs nothing — the worker
    /// owns the same allocation.
    ///
    /// ⚠ It must be replaced at the full-resolution handover along with `engine`, or the
    /// eyedropper would read the half-size buffer at full-resolution coordinates.
    pub(crate) buffer: std::sync::Arc<core_image::ImageBuf>,
    /// The eyedropper is armed: the next click on the canvas sets the white balance.
    ///
    /// ⚠ View state on the session, never in `Edits` — arming a tool is not part of the
    /// photograph's description, so it must not reach the sidecar or travel with a paste. The
    /// rule `selected_mask` and `show_guides` already follow.
    pub(crate) picking_wb: bool,
    /// The defringe eyedropper is armed: the next click on the canvas sets a fringe's hue range
    /// (M9.6). View state, `picking_wb`'s rule.
    pub(crate) picking_fringe: bool,
    /// The film-base eyedropper is armed: the next click on the canvas picks the film base and
    /// measures the frame (M10.11a). View state, `picking_wb`'s rule.
    pub(crate) picking_negative: bool,
    /// Lens Blur's Pick focus is armed: the next click on the canvas sets a Point focus there
    /// (M10.16d). View state, `picking_wb`'s rule.
    pub(crate) picking_focus: bool,
    /// The illuminant the decode left this photo balanced to — the Kelvin slider's zero point.
    ///
    /// ⚠⚠ Kept on the session for the same reason `lens` is: it is a fact about the file that
    /// cannot change within one session, and **it must survive the full-resolution handover** or
    /// the white balance would silently shift the moment the upgrade lands. It also has to reach
    /// export, since a Kelvin in the sidecar means nothing without the baseline it was set
    /// against.
    pub(crate) wb_baseline: color::wb::Illuminant,
    /// The camera profile's `BaselineExposure` in stops, when it declares one.
    ///
    /// ⚠ Kept beside `wb_baseline` and for its reason: a fact about the file that cannot change
    /// within one session. The **panel** needs it, not only the renderer — Auto Tone has to aim
    /// the scene median at a target divided by the rendering's gain, or the two both place the
    /// level and stack.
    pub(crate) baseline_ev: Option<f32>,
    pub(crate) zoom: f32,
    /// The scale that fits `fit_image_size` into the canvas, as of the last frame.
    ///
    /// ⚠ Written by the canvas, which is the only thing that knows the viewport. "Zoom to 1:1"
    /// needs it and cannot compute it — the same reason `fit_image_size` is stored rather than
    /// derived. It is a cache of per-frame view geometry, never an edit.
    pub(crate) fit_scale: f32,
    pub(crate) pan: egui::Vec2,
    pub(crate) history: History<Edits>,
    pub(crate) show_before: bool,
    pub(crate) active_handle: Option<crop_tool::Handle>,
    /// Which mask the canvas is editing, if any. Index into `edits.masks`.
    ///
    /// ⚠ **View state, not an edit.** It must not reach the sidecar, transfer on a paste or
    /// appear in a preset — `Edits` is the file format, and a selection is not part of the
    /// photograph's description. `Session` is per-photo, so this resets on a switch for free.
    pub(crate) selected_mask: Option<usize>,
    /// Which **component** of the selected mask the shape controls and the canvas handles act on.
    /// `0` is the mask's base shape; `j > 0` is the `j-1`th refinement.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule, for its reason.
    ///
    /// ⚠ Read through [`Session::component_index`], never directly: that clamps against the
    /// selected mask's real component count, so a site that forgets to reset this degrades to
    /// "the base is selected" rather than leaving the panel's sliders on one shape and the
    /// canvas's handles on another.
    pub(crate) selected_component: usize,
    /// Which retouch spot is selected, if any.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule, for its reason: a selection is
    /// not part of the photograph's description, so it must not reach the sidecar or travel with
    /// a paste. `Session` is per-photo, so this resets on a switch for free.
    pub(crate) selected_spot: Option<usize>,
    /// The mask handle this drag is moving, if the drag started on one.
    ///
    /// ⚠ Load-bearing for panning: a drag that did *not* start on a handle must still pan the
    /// image, so this records whether the mask claimed the gesture.
    pub(crate) active_mask_handle: Option<mask_tool::Handle>,
    /// A spot the *view* leaves out while it is being adjusted.
    ///
    /// ⚠⚠ **Why: while you drag a patch, its old effect is still on screen, hiding the very
    /// blemish you are trying to line up with.** Reported as "it shows the previous retouching
    /// there, I think it should just disappear, so I can once again better align the positions".
    /// Suppressing it reveals the untouched pixels underneath for the duration of the gesture.
    ///
    /// ⚠ **View state, never `Edits`** — the same standing as `selected_spot`, and applied in
    /// `view_edits`, which is where `preview_edits` already strips the crop in geometry mode. The
    /// sidecar, an export and the CLI never see it.
    ///
    /// ⚠ Two renders per gesture, not one per frame: one when the suppression begins and one when
    /// it ends. `SpotRemovalStage` is in the prefix, so each costs a whole-buffer noise-reduction
    /// and dehaze pass — which is exactly why the per-frame version was reported as laggy.
    pub(crate) suppressed_spot: Option<usize>,
    /// Per mask, whether its overlay is tinted on the canvas — the row's eye. View state, never
    /// part of `Edits`; `sync_mask_shown` keeps it the masks' length, and a mask that arrives by
    /// undo, paste or a snapshot starts with its overlay off.
    ///
    /// ⚠ Since 2026-09-20 the eye is the overlay **only**; switching a mask off is
    /// `MaskedAdjustment::disabled` in the edits — its own toggle beside the eye — because the
    /// user wants an off mask out of the export too.
    pub(crate) mask_shown: Vec<bool>,
    /// Per patch, whether its outline is drawn on the canvas — the old "Show patches", per
    /// patch (2026-09-20). View state; `sync_spot_flags` keeps it the patches' length, and a
    /// patch that arrives by any route starts shown, as outlines always did.
    pub(crate) spot_shown: Vec<bool>,
    /// Per patch, whether it is held out of the view so that what it covers shows — the
    /// per-patch twin of a mask's eye. View state; `view_edits` removes held patches together
    /// with the drag-suppressed one.
    pub(crate) spot_held: Vec<bool>,
    /// Set for exactly one frame after a suppression change, so the canvas lets the deliberately
    /// softer render stand instead of sharpening it away on the same frame.
    ///
    /// ⚠⚠ **A baton passed across one frame, not a second owner of a fact.** `develop::body` runs
    /// the panels before the canvas, so without it the canvas sees `max_dim != s.render_rung` in
    /// the very frame the drop was made and requests full quality immediately — the soft render
    /// is superseded before it can be shown, and the toggle takes the full 95-300 ms after all.
    /// That is not the spot-removal shape: nothing else decides this, and it is consumed by the
    /// one reader.
    pub(crate) soft_next: bool,
    /// The last **whole-image** texture, kept as a backdrop under a region render, with the
    /// `fit_size` it was rendered at.
    ///
    /// ⚠⚠ **This is what stops the edges of a pan being BLACK.** A region render paints only
    /// `img_rect`; everything outside it was simply not painted, so revealing new area while
    /// panning showed black rectangles that snapped to image when the next region landed.
    /// Reported as making panning at 1:1 "very stuttery" and "very unpleasant".
    ///
    /// ⚠ The alternative — render the whole image at the native rung — was **measured and
    /// rejected**: 26 Mpx against a region's 0.5, about 300 ms per render, and paid on every
    /// slider move as well as every pan. This costs one texture and one `paint_at`, and no
    /// rendering at all.
    ///
    /// ⚠ Keyed on the `fit_size` it was rendered at, and dropped when that no longer matches.
    /// A crop or an orientation change moves the frame, and a backdrop from before it would be
    /// painted at the wrong scale — visibly, since it is drawn across the whole frame rect.
    pub(crate) backdrop: Option<(egui::TextureHandle, (f32, f32))>,
    /// What the retouch gesture is doing, across frames.
    ///
    /// ⚠⚠ **On the session, not a local in `canvas::show`** — `active_mask_handle`'s home, for a
    /// reason found the hard way: `show` runs fresh every frame, so a local resets between the
    /// press and the release and the release then sees no gesture at all. The drag placed nothing
    /// and the lifecycle looked perfect from every angle until it was instrumented.
    pub(crate) spot_drag: crate::develop::canvas::spot_overlay::SpotDrag,
    /// Which eye is selected (M9.11). View state, never `Edits` — `selected_spot`'s rule.
    /// ⚠ Selecting an eye clears `selected_spot` and vice versa, so the panel's sliders always
    /// edit exactly one thing.
    pub(crate) selected_eye: Option<usize>,
    /// An eye the view leaves out while it is moved or resized — `suppressed_spot`'s twin, so the
    /// red pupil underneath is visible to aim at. Applied in `view_edits`.
    pub(crate) suppressed_eye: Option<usize>,
    /// The eye gesture across frames — on the session for `spot_drag`'s reason.
    pub(crate) eye_drag: crate::develop::canvas::eye_overlay::EyeDrag,
    /// The lens corrector the canvas maps through, built once per lens and decode size (M10.16c1's review).
    pub(crate) lens_memo: rustroom_app::ai_space::CorrectorMemo,
    /// Remove dust (M10.16c2): a press waiting for the photo, its search, what the last one did — gone with the
    /// photo.
    pub(crate) dust: rustroom_app::dust_tool::Run,
    /// Find people (M10.16c3): a press waiting, its search, the people found and which go — gone with the photo.
    pub(crate) people: rustroom_app::people_tool::Run,
    /// The people's outlines as textures, uploaded once each — the canvas's half of `people`.
    pub(crate) people_view: crate::develop::canvas::people_overlay::PeopleView,
    /// The Remove gesture and the selected removal (M10.16c1) — `spot_drag`'s reason.
    pub(crate) removal: crate::develop::canvas::remove_overlay::RemovalUi,
    /// Drawn perspective guides.
    ///
    /// ⚠⚠ **View state, never `Edits`.** A guide is scaffolding for arriving at a slider value,
    /// not part of the photograph's description — `selected_mask`'s rule. So guides never reach
    /// the sidecar, never travel with a paste, preset or sync, and reset on a photo switch. That
    /// is what makes "solve into the sliders" one representation rather than two.
    pub(crate) guides: Vec<pipeline::perspective::guides::Guide>,
    /// The perspective and straighten the **visible** texture was rendered with, and the pair the
    /// newest request will produce.
    ///
    /// ⚠⚠ Two, not one: a render is asynchronous, so "what was asked for" and "what is showing"
    /// genuinely differ for as long as it takes. The guide overlay is drawn against `shown_geo`,
    /// so a guide stays on the feature it was traced onto until the image actually catches up —
    /// which at native zoom is hundreds of milliseconds, because perspective is a *prefix* stage
    /// and changing it drops the prefix cache.
    pub(crate) shown_geo: (edit_model::Perspective, f32),
    /// ⚠ A `Cell` so `request` can stay `&self`. It is called from places already holding a
    /// shared borrow of the session, and `&mut self` there is an `E0502`.
    pub(crate) pending_geo: std::cell::Cell<(edit_model::Perspective, f32)>,
    /// Whether the Guides sub-mode of the Geometry tool is on.
    ///
    /// ⚠ On `Session` rather than `RustroomApp`, beside the guides themselves — which is what
    /// lets `view_edits` read it directly and leaves all seven of its call sites untouched.
    pub(crate) show_guides: bool,
    /// `(guide index, 0 = from | 1 = to)` while an endpoint is being dragged.
    pub(crate) active_guide: Option<(usize, usize)>,
    /// The region the current texture covers, as a rect, so the canvas can both place it and
    /// test containment without consulting texture dimensions — which would be wrong while
    /// `show_before` swaps in the whole-image baseline. `None` = whole image.
    pub(crate) rendered_region: Option<RegionRect>,
    /// The region last requested, so the same one is not re-requested every frame.
    pub(crate) requested_region: Option<RegionRect>,
    /// Post-geometry image size at fit resolution, for the fit/zoom maths.
    pub(crate) fit_image_size: egui::Vec2,
    /// The full post-geometry frame's size in **rendered** pixels — what a region was cut from,
    /// or the frame's own dimensions when it arrived whole.
    ///
    /// ⚠ [`Self::rendered_long`] is derived from this at the single write site rather than being
    /// read from the frame separately: two spellings of one fact that are computed independently
    /// is the shape `render_max_dim` and `fit_image_size` were each fixed for.
    pub(crate) rendered_full: (usize, usize),
    /// Long edge of the *full* post-geometry image at the resolution the current texture was
    /// rendered at. Placement must use this rather than the current mode's: while a frame
    /// from the other mode is still in flight the two differ, and using the current one drew
    /// the old texture far off-screen — a white flash when zooming out quickly.
    pub(crate) rendered_long: f32,
    /// The editing buffer's long edge — the top rung of the render ladder.
    pub(crate) buffer_long: usize,
    /// The long edge of a **full-resolution** decode of this file, in pixels.
    ///
    /// ⚠⚠ Derived from the buffer and its `source_scale` **once**, so it is the same number
    /// before and after the full-resolution handover. `buffer_long` is not usable for this: a RAW
    /// opens at half size, so a size shown in pixels against it would silently double a few
    /// seconds after the photo appeared.
    pub(crate) full_long: usize,
    /// Whether `engine`'s buffer is the full-resolution decode.
    ///
    /// True from the start for JPEG/PNG/TIFF, which are decoded at full size already.
    pub(crate) buffer_is_full_res: bool,
    /// Whether `engine`'s buffer is the 512-px ghost (`ghost.rs`): the session exists, the real
    /// decode is on its way, and the full-resolution upgrade waits for it.
    pub(crate) buffer_is_ghost: bool,
    /// When this session was created — the dwell the upgrade trigger waits on.
    pub(crate) opened_at: Instant,
    /// The rung the canvas last chose, so an edit made from a panel re-renders at the tier
    /// currently on screen rather than snapping back to fit. Read through
    /// [`Session::render_max_dim`], never directly.
    pub(crate) render_rung: usize,
    /// The display's primaries, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here rather than reached through the app because `Session::request` — which every
    /// render goes through — has no access to `RustroomApp`. `apply_display_space` writes it on
    /// the open session so a change takes effect without reopening the photo.
    pub(crate) display_space: color::OutputSpace,
    /// The canvas's rendering intent, copied from `Settings` when the session opens — held here
    /// for `display_space`'s reason. `apply_canvas_intent` writes it on the open session.
    pub(crate) canvas_intent: color::RenderingIntent,
    /// The resampling kernel, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here for the same reason `display_space` is: `Session::request` is the one funnel
    /// every render goes through and it cannot reach `RustroomApp`. `apply_resampler` writes it
    /// on the open session so a change takes effect without reopening the photo.
    pub(crate) resampler: pipeline::Resampler,
    /// Run the **global** HSL stage on the GPU, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here for `resampler`'s reason directly above: `Session::request` is the one funnel
    /// every render goes through and it cannot reach `RustroomApp`. `apply_gpu_tail` writes it on
    /// the open session so a change takes effect without reopening the photo.
    pub(crate) gpu_tail: bool,
    /// The Visualize Spots threshold, or `None` when the view is off.
    ///
    /// ⚠ On `Session`, not `RustroomApp`, for the reason `display_space` gives: `Session::request`
    /// is the one funnel every render goes through and has no access to the app, so the render
    /// reads it from here. A second copy on the app would be two facts that can disagree — the
    /// shape `render_max_dim` and `geometry_mode` were each fixed for. The cost is that it resets
    /// on a photo switch, like `selected_spot`; `select_tool` also clears it, since a canvas
    /// stuck in the grey edge view after switching to Edit reads as a broken render.
    pub(crate) visualize: Option<f32>,
    /// The "Show range" checkbox in the Color Grading block.
    ///
    /// ⚠ View state on `Session`, beside `visualize` and for its reason: `Session::request` is
    /// the one funnel every render goes through and cannot reach the app. Resets per photo and
    /// clears on a tool switch. ⚠ Never `Edits` — it must not reach the sidecar or a paste.
    pub(crate) show_grade_range: bool,
    /// What the request carries: the checkbox AND a non-Global selection, or `None`.
    ///
    /// ⚠ Two fields, one derivation site. The value depends on the checkbox and on
    /// `app.grade_range`, which lives on the app and which `request` cannot see; the panel
    /// derives this with a change guard in the one place both are in scope. Under Global it is
    /// `None` while the box stays ticked — one `Option` would untick the box the moment Global
    /// was selected.
    pub(crate) grade_view: Option<rustroom_app::grade_view::GradeRangeView>,
    /// The view's plane as a texture, set where the frame lands and dropped by the same
    /// assignment when a frame arrives without one.
    pub(crate) grade_view_texture: Option<egui::TextureHandle>,
    /// The straighten-line gesture is armed: the next drag under `Tool::Crop` draws a line
    /// instead of moving the crop rectangle.
    ///
    /// ⚠ View state on the session, never in `Edits` — the rule `picking_wb` states: "arming a
    /// tool is not part of the photograph's description, so it must not reach the sidecar or
    /// travel with a paste." `show_guides` and `selected_mask` follow it too. `Session` is
    /// per-photo, so this resets on a switch for free.
    pub(crate) straighten_armed: bool,
    /// The line being drawn, in **raw**-frame normalised coordinates, while the drag is live.
    ///
    /// ⚠⚠ **On `Session`, never a local in `canvas::show`.** M5.3b records the exact failure: the
    /// spot drag state "was a **local** in `canvas::show` at first — reset every frame, so the
    /// release saw no gesture", and "one `eprintln` found it after the lifecycle had read
    /// perfectly three times." It is `Session.spot_drag` now, and this follows it.
    pub(crate) straighten_drag: Option<([f32; 2], [f32; 2])>,
    /// The last straighten line, on the **raw** frame, drawn mapped through the correction so it
    /// reads level — the feedback that says the gesture worked.
    ///
    /// ⚠ View state, never `Edits`, for `straighten_armed`'s reason. Cleared when the mode is
    /// re-armed, the tool changes, or the photo changes (the last is free — `Session` is
    /// per-photo).
    pub(crate) straighten_line: Option<([f32; 2], [f32; 2])>,
    /// The rendered frame's sRGB bytes, kept for the colour readout, with the dimensions they
    /// describe.
    ///
    /// ⚠⚠ **Not the texture.** `texture` is an `egui::TextureHandle`, i.e. GPU-side, and cannot
    /// be sampled CPU-side — which is exactly why `guide_overlay::draw_loupe` magnifies it by
    /// *drawing* it with UVs. These bytes are `pack`'s `srgb8`, the same ones `hist` is bucketed
    /// from, and deliberately **not** `RenderedFrame.rgba`, which is in the display's primaries
    /// and would give a readout that changes with the monitor.
    ///
    /// ⚠ The dimensions travel **with** the bytes rather than being re-derived from
    /// `rendered_region` or the texture. Two facts that can disagree is the shape
    /// `render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for.
    ///
    /// ⚠ An `Arc`, so retaining it costs nothing beyond the memory the frame already held.
    pub(crate) readout_bytes: Option<(std::sync::Arc<Vec<u8>>, usize, usize)>,
    /// The clipping overlay for the frame `readout_bytes` describes, when `J` is on and
    /// something clips; painted at the preview's own rectangle.
    pub(crate) clip_texture: Option<egui::TextureHandle>,
    /// Where the whole photo sat on screen last frame — what lights-out cuts its hole around
    /// and the info card anchors to.
    pub(crate) canvas_frame_rect: Option<egui::Rect>,
    /// The centre of the canvas on screen last frame — what AI denoise's job is aimed at (M10.16b4).
    pub(crate) canvas_view_centre: Option<egui::Pos2>,
    /// The sampled value under the cursor, or `None`.
    ///
    /// ⚠ The **value**, not a formatted string: the swatch and the numbers are two renderings of
    /// one fact, and storing the string would leave the swatch deriving a colour from text. One
    /// fact, formatted at the edge that draws it.
    ///
    /// ⚠ Cleared by `canvas::show` on every frame and set again only by an actual hover over the
    /// image. A stale number left on screen after the pointer leaves is worse than none, because
    /// it gets trusted.
    pub(crate) readout: Option<[u8; 3]>,
    /// The output space the canvas is being proofed into, or `None`.
    ///
    /// ⚠ On `Session`, not `RustroomApp`: `Session::request` is the one funnel every render goes
    /// through and it has no app access. That is why `display_space` and `visualize` live here —
    /// and, as there, the cost is that it resets on a photo switch.
    ///
    /// ⚠⚠ **View state, never `Edits`.** A proof is a property of the *view*, exactly like
    /// `display_space`: it must not reach the sidecar, must not travel with a paste, a preset or
    /// a sync, and must not appear in any golden.
    pub(crate) soft_proof: Option<color::OutputSpace>,
    /// Whether out-of-gamut pixels are marked while proofing.
    ///
    /// ⚠ A separate toggle from the proof itself: proofing without the warning is the common case
    /// — you want to see the picture — and a warning you cannot switch off is chrome. It has no
    /// effect at all while `soft_proof` is `None`, which is why `Session::request` folds the two
    /// into one `Proof` rather than sending them separately.
    pub(crate) gamut_warning: bool,
}

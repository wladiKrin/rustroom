//! The background worker: requests in, frames out, superseded requests coalesced, and the
//! warm-behind render once a gesture settles.

use super::brush_cache::BrushCache;
use super::frame::{FrameSpec, RenderedFrame};
use super::render::{render_cached, render_cached_with};
use crate::prefix_cache::PrefixCache;
use core_image::ImageBuf;
use edit_model::Edits;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

struct Request {
    edits: Edits,
    spec: FrameSpec,
}

/// Owns a worker thread that renders against a fixed editing buffer.
pub struct RenderEngine {
    req_tx: Sender<Request>,
    res_rx: Receiver<RenderedFrame>,
    _handle: JoinHandle<()>,
    /// How many renders have been asked for over this engine's life.
    ///
    /// ⚠⚠ **Unconditional, not `#[cfg(test)]`, and that is forced rather than sloppy.** This type
    /// lives in the **lib**; its only test-time consumer (`develop/canvas.rs`) lives in the
    /// **bin**, and when the bin's test target builds, the lib is a dependency compiled *without*
    /// `cfg(test)` — so a gated field would simply not exist where it is needed.
    ///
    /// It earns two `usize` of production surface because the alternative is untestable: "a drag
    /// mutated `Edits` but never asked for a render" leaves no trace anywhere else, and that bug
    /// has shipped — the mask overlay kept showing its previous position until some unrelated
    /// edit happened to request a frame.
    requests: Arc<std::sync::atomic::AtomicUsize>,
    /// How many **warm-behind** renders this engine's worker has run.
    ///
    /// ⚠⚠ An observable, because a warm render produces no frame and changes no pixel: the cache
    /// it leaves behind makes the *next* whole-buffer render fast, and nothing in any output can
    /// distinguish a warmed engine from one that never warmed. Every equivalence test in the
    /// workspace passes with the mechanism deleted — `requests_issued`, `prefix_stages`, `bytes`
    /// and `region_scoped` all exist for exactly this reason.
    ///
    /// ⚠ Unconditional rather than `#[cfg(test)]`, for `requests_issued`'s reason.
    warmed: Arc<std::sync::atomic::AtomicUsize>,
    /// How many settles downloaded the resident final instead of rendering. See `warmed`.
    settle_downloads: Arc<std::sync::atomic::AtomicUsize>,
    /// Where this engine's worker looks for `.cube` files, kept so the **full-resolution
    /// handover** can carry it onto the replacement engine.
    ///
    /// ⚠⚠ The decode cannot supply it — `decode.rs` sets `looks_dir: None` on purpose — so the
    /// handover rebuilding from `decoded.profile` silently dropped it, and every creative look
    /// stopped loading ~4 s after a RAW opened. Retained here rather than re-derived at the
    /// handover, because "where looks live" existing in two places is how the two come to
    /// disagree.
    looks_dir: Option<std::path::PathBuf>,
    /// What Camera matching applies to this photo (M10.13d) — kept for the panel's line.
    camera_look: Option<crate::camera_look::Resolved>,
    /// The photo's AI mask maps (M9.13), shared with the worker's `BrushCache`. The session writes
    /// here when a map arrives and then asks for a render.
    ai_maps: crate::ai_coverage::SharedAiMaps,
}

impl RenderEngine {
    /// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
    /// same file (0.5 for a half-size RAW decode, 1.0 for JPEG/PNG/TIFF).
    /// `on_done` is called after each render completes (e.g. `ctx.request_repaint`).
    /// `lens` is the resolved profile for this photo, or `None` when the file records no lens or
    /// none matched.
    ///
    /// ⚠ **Captured here rather than passed per request.** The engine is rebuilt per photo, so a
    /// profile cannot vary within one engine's life — the same shape `DecodeWorker` uses for the
    /// camera profile, and for the same reason: a per-request parameter would impose a choice on
    /// callers that have no business making it.
    pub fn new(
        buffer: Arc<ImageBuf>,
        source_scale: f32,
        lens: Option<lens_profile::LensShot>,
        wb_baseline: color::wb::Illuminant,
        // ⚠ Captured, not per request: neither can change without a re-decode. The profile is a
        // `Settings` choice that rebuilds the decode worker, and whether a buffer is
        // scene-referred is a property of the file.
        profile: crate::decode_worker::ProfileParts,
        scene_referred: bool,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let looks_dir = profile.looks_dir.clone();
        let kept_looks_dir = looks_dir.clone();
        let kept_camera_look = profile.camera_look.clone();
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<RenderedFrame>();
        let warmed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let warmed_worker = Arc::clone(&warmed);
        let settle_downloads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let settles_worker = Arc::clone(&settle_downloads);
        let ai_maps = crate::ai_coverage::SharedAiMaps::default();
        let ai_maps_worker = Arc::clone(&ai_maps);
        let handle = thread::spawn(move || {
            let warmed = warmed_worker;
            let settle_downloads = settles_worker;
            let mut cache = PrefixCache::default();
            // ⚠⚠ **Resolved HERE rather than on the UI thread**, and cached by name: a 64³ cube
            // is 786 KB and this loop runs on every slider frame. One slot is enough — a photo
            // has one look, and switching looks is a deliberate act, not a drag.
            let look_store = looks_dir.map(crate::looks::LookStore::new);
            let mut look_cache: Option<(String, Arc<pipeline::lut::Cube>)> = None;
            // Lives beside the prefix cache and for the same reason: this thread is the sole
            // renderer, so neither needs locking.
            let mut brush_cache = BrushCache {
                ai: ai_maps_worker,
                ..BrushCache::default()
            };
            // ⚠ Holds the request that ended a warm-behind wait, so nothing is ever dropped on the
            // floor: the wait below **takes** a request out of the channel to learn the gesture is
            // still live, and the next turn of the loop must serve that one rather than block.
            let mut pending: Option<Request> = None;
            loop {
                let mut req = match pending.take() {
                    Some(r) => r,
                    None => match req_rx.recv() {
                        Ok(r) => r,
                        Err(_) => return,
                    },
                };
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                // The stage count is for the tests and the bench; the worker only wants pixels.
                // ⚠⚠ **A source that is already rendered never renders again.** A JPEG, a PNG
                // or a TIFF has been through whatever produced it; applying ours on top would
                // render the photograph twice. Stripped here rather than at every call site, so
                // no caller can forget.
                let edits = if scene_referred {
                    req.edits
                } else {
                    edit_model::Edits {
                        // ⚠ Undecided, not decided off: a stripped copy is a render
                        // input, not a choice anyone made.
                        rendering: None,
                        ..req.edits
                    }
                };
                // ⚠⚠ M10.12c: marks stored in decode space, mapped to the spaces their stages draw in — here,
                // once, beside the strip above, so no render path can forget. The buffer is the decode at
                // `source_scale`; only its aspect matters.
                let edits = match pipeline::anchors::resolve_for(
                    &edits,
                    lens.as_ref(),
                    buffer.width(),
                    buffer.height(),
                ) {
                    std::borrow::Cow::Borrowed(_) => edits,
                    std::borrow::Cow::Owned(e) => e,
                };
                // ⚠ A look that will not load leaves `look` as `None`, which renders the photo
                // *without the look* and keeps the camera rendering — never an error that stops
                // the photo opening. The panel is where the user is told; see `LooksState`.
                if let (Some(store), Some(file)) = (look_store.as_ref(), edits.look.file.as_deref())
                {
                    if edits.look.is_active() && look_cache.as_ref().is_none_or(|(n, _)| n != file)
                    {
                        look_cache = store
                            .load(file)
                            .ok()
                            .map(|c| (file.to_owned(), Arc::new(c)));
                    }
                }
                let look = look_cache
                    .as_ref()
                    .filter(|(n, _)| {
                        edits.look.is_active() && edits.look.file.as_deref() == Some(n.as_str())
                    })
                    .map(|(_, c)| c);

                // ⚠ Env-gated, like `RUSTROOM_DEBUG_DISPLAY` below and `RUSTROOM_DEBUG_BRUSH`
                // before it, and added for the same reason both of those were: what a drag frame
                // costs depends on the **rung** the viewport put it on, and no benchmark can
                // guess that — `bench_masked` measures 1.7 Mpx at rung 1600 and 6.5 Mpx at the
                // native rung, a 4x spread, with only the running window able to say which one a
                // user is actually waiting for.
                let debug_render = std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some();
                let t0 = std::time::Instant::now();
                // ⚠ Bound once and reused by the warm-behind render below. `TailRendering`
                // is `Copy` precisely so a second render cannot be handed a different one.
                let rendering = pipeline::TailRendering {
                    curve: profile.curve.as_deref(),
                    baseline_ev: profile.baseline_ev,
                    look,
                    camera_look: crate::camera_look::applied(
                        &edits,
                        profile.camera_look.as_ref().and_then(|c| c.cube.as_ref()),
                    ),
                };
                // ⚠ The live frame may hand off (no final checkpoint); the settle pass below may not.
                let (frame, ran) = render_cached_with(
                    &buffer,
                    &edits,
                    req.spec,
                    source_scale,
                    lens.clone(),
                    wb_baseline,
                    rendering,
                    &mut cache,
                    &mut brush_cache,
                    true,
                );
                if debug_render {
                    eprintln!(
                        "render: {:6.1} ms  rung {:5}  frame {}x{} ({:.1} Mpx)  region {}  masks {}  prefix_ran {}  scoped {}  bytes {}  handoff {}  tail_up {}  hit {}",
                        t0.elapsed().as_secs_f64() * 1000.0,
                        req.spec.max_dim,
                        frame.width,
                        frame.height,
                        (frame.width * frame.height) as f64 / 1e6,
                        if frame.region.is_some() { "yes" } else { "no " },
                        edits.masks.len(),
                        ran.prefix_stages,
                        // ⚠ M8.6's observable in the running app. The two paths produce the same
                        // picture by design, so without this a gate that never fires looks
                        // exactly like one that does.
                        if ran.region_scoped { "yes" } else { "no " },
                        if ran.bytes { "yes" } else { "no " },
                        if ran.handoff { "yes" } else { "no " },
                        ran.tail_uploads,
                        if ran.resident_final_hit { "yes" } else { "no " },
                    );
                }
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();

                // ⚠⚠ **WARM BEHIND, and without it the gesture INVERTS: fluid while dragging,
                // then a long stall the moment you let go.** A scoped render is deliberately never
                // cached, so no drag frame leaves a checkpoint and the settled whole-buffer render
                // has to build the entire prefix from scratch — measured at ~200 ms on the
                // reporter's chain. This spends that in the background instead.
                //
                // ⚠⚠ **The settle is detected from the CHANNEL, not from a drag signal.** A
                // gesture produces a request per UI frame, so a quiet channel for longer than a
                // frame interval *is* the settle — which keeps the whole mechanism inside the
                // worker and adds nothing that could disagree with the derived gate.
                //
                // ⚠ A request arriving during the wait is **taken and carried forward**, never
                // discarded: the warm render simply never starts, so there is nothing to abandon
                // and no second coalescing mechanism beside the one above.
                //
                // ⚠⚠ The frame is **discarded**. Sending it would replace the canvas texture with
                // a whole-image one nobody asked for — the point is the cache it leaves behind,
                // and the swap is invisible only because plan 1 made the two paths agree.
                // ⚠⚠ A HANDOFF frame leaves no final checkpoint either (2026-09-26), so it settles
                // too: without this, releasing a Straighten drag and then moving Exposure would
                // re-run the geometry for the first tail edit. ⚠⚠ **But its settle is a DOWNLOAD,
                // not a render** — its final is already on the GPU as the resident final, and
                // reading it back (~10 ms) replaces a re-run of the last step and a whole tail
                // nobody saw (92-108 ms measured). A scoped drag has no whole-frame resident and
                // renders whole-buffer as it always did.
                if ran.region_scoped || ran.handoff {
                    match req_rx.recv_timeout(WARM_BEHIND) {
                        Ok(newer) => pending = Some(newer),
                        Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {
                            let t1 = std::time::Instant::now();
                            let settled = ran.handoff
                                && gpu_pipeline::shared()
                                    .is_some_and(|ctx| cache.settle_from_resident(ctx));
                            if settled {
                                settle_downloads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if debug_render {
                                    eprintln!(
                                        "  settle: {:6.1} ms  rung {:5}  downloaded the resident final",
                                        t1.elapsed().as_secs_f64() * 1000.0,
                                        req.spec.max_dim,
                                    );
                                }
                            } else {
                                let (_, warm) = render_cached(
                                    &buffer,
                                    &edits,
                                    FrameSpec {
                                        region: None,
                                        ..req.spec
                                    },
                                    source_scale,
                                    lens.clone(),
                                    wb_baseline,
                                    rendering,
                                    &mut cache,
                                    &mut brush_cache,
                                );
                                warmed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if debug_render {
                                    eprintln!(
                                        "  warm behind: {:6.1} ms  rung {:5}  prefix_ran {}",
                                        t1.elapsed().as_secs_f64() * 1000.0,
                                        req.spec.max_dim,
                                        warm.prefix_stages,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        });
        Self {
            req_tx,
            res_rx,
            _handle: handle,
            requests: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            warmed,
            settle_downloads,
            looks_dir: kept_looks_dir,
            camera_look: kept_camera_look,
            ai_maps,
        }
    }

    /// Queue a render at the given working resolution, optionally cropped to `region`.
    pub fn request(&self, edits: Edits, spec: FrameSpec) {
        // ⚠ Bumped before the send, and unconditionally: a send to a dropped receiver returns
        // `Err`, but a request that could not be delivered is still one that was *asked for*,
        // which is what the tests assert about.
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.req_tx.send(Request { edits, spec });
    }

    /// The AI mask maps this engine's renders read (M9.13). Write a map, then request a render.
    pub fn ai_maps(&self) -> crate::ai_coverage::SharedAiMaps {
        Arc::clone(&self.ai_maps)
    }

    /// Where this engine loads `.cube` files from, so the full-resolution handover can carry it.
    ///
    /// ⚠ Unconditional rather than `#[cfg(test)]`, for `requests_issued`'s reason: `session_flow`
    /// is in the **bin** and this is the **lib**, which the bin's test target compiles without
    /// `cfg(test)`, so a gated accessor would not exist where it is needed.
    pub fn looks_dir(&self) -> Option<&std::path::Path> {
        self.looks_dir.as_deref()
    }

    /// What Camera matching applies to this photo (M10.13d), `None` for a file naming no film simulation.
    pub fn camera_look(&self) -> Option<&crate::camera_look::Resolved> {
        self.camera_look.as_ref()
    }

    /// How many renders have been asked for. See the field's doc comment for why this is not
    /// `#[cfg(test)]`.
    pub fn requests_issued(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many warm-behind renders the worker has run. See the field's doc for why it exists.
    pub fn warm_renders(&self) -> usize {
        self.warmed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many settles downloaded the resident final rather than rendering. See `warmed`.
    pub fn settle_downloads(&self) -> usize {
        self.settle_downloads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Return the newest finished frame, if any (drops older ones).
    pub fn poll_latest(&self) -> Option<RenderedFrame> {
        let mut latest = None;
        while let Ok(f) = self.res_rx.try_recv() {
            latest = Some(f);
        }
        latest
    }
}

/// How long a quiet request channel means the gesture has settled, after a region-scoped render.
///
/// ⚠⚠ **Detected from the channel rather than from a drag signal**, which is what keeps the whole
/// mechanism inside the worker: a live gesture produces a request per UI frame, so silence for
/// longer than a frame interval *is* the settle. A `dragging` flag threaded down from the canvas
/// would be a second fact beside the derived gate, and the two could disagree.
///
/// ⚠ Comfortably above a 60 Hz frame (16.7 ms) and below anything a person would notice, so a
/// paused-but-still-held drag warms once and costs one frame of extra latency when it resumes —
/// the "abandon, don't abort" trade `DecodeWorker` and `batch_export` already make.
const WARM_BEHIND: std::time::Duration = std::time::Duration::from_millis(150);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_engine::fixtures::{
        chained_edits, chained_spec, full_prefix, hazy_buffer, hsl_edits, tiny, varied, varied_rect,
    };
    use crate::render_engine::render::render_once;

    const LONG: std::time::Duration = std::time::Duration::from_secs(120);

    /// ⚠ A 120-s ceiling against a hang, never a time budget (AGENTS.md): the old 1 s lost the race
    /// on a loaded full gate — `a_handed_off_frame_is_settled_by_a_download` takes 0.8 s alone.
    fn wait_for_frame(engine: &RenderEngine) -> RenderedFrame {
        let deadline = std::time::Instant::now() + LONG;
        while std::time::Instant::now() < deadline {
            if let Some(f) = engine.poll_latest() {
                return f;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no frame produced");
    }

    /// Poll `f` until it holds, or fail at a deadline.
    ///
    /// ⚠ A deadline rather than an open wait: a regression must **fail** rather than hang CI for
    /// hours, which is the rule `the_folder_is_loaded_in_parallel` was rewritten for.
    fn wait_until(mut f: impl FnMut() -> bool, what: &str) {
        let deadline = std::time::Instant::now() + LONG;
        while std::time::Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn chained_engine(buf: &Arc<ImageBuf>) -> RenderEngine {
        RenderEngine::new(
            buf.clone(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        )
    }

    /// M10.12c: the preview resolves a decode-space spot exactly as export does — the frame equals its resolved
    /// twin's, and differs from the frame without it.
    #[test]
    fn the_preview_draws_an_anchored_spot_where_its_resolved_twin_is() {
        let buf = varied_rect(128, 96);
        // Constructed: a synthetic barrel lens, the pipeline's own fixture.
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
        let lens = lens_profile::LensShot {
            profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        };
        let engine = RenderEngine::new(
            buf.clone(),
            1.0,
            Some(lens.clone()),
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        let base = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Edits::default()
        };
        let anchored = Edits {
            spots: vec![edit_model::Spot {
                centre: [0.12, 0.15],
                source: [0.5, 0.5],
                radius: 0.06,
                feather: 0.0,
                mode: edit_model::SpotMode::Clone,
                dust: false,
                decode: true,
            }],
            ..base.clone()
        };
        let a = pipeline::anchors::Anchoring::for_edits(Some(&lens), &anchored, 128, 96);
        let twin = pipeline::anchors::resolve(&anchored, &a).into_owned();
        let frame = |e: &Edits| {
            engine.request(e.clone(), FrameSpec::fit(128));
            wait_for_frame(&engine).rgba
        };
        let plain = frame(&base);
        let got = frame(&anchored);
        assert_ne!(got, plain, "the spot must change the frame");
        assert_eq!(got, frame(&twin));
    }

    /// ⚠⚠ **Without this the gesture inverts: fluid while dragging, then a stall the moment you
    /// let go.** A scoped render is deliberately never cached, so no drag frame leaves a
    /// checkpoint behind and the settled whole-buffer render builds the entire prefix from
    /// scratch — measured at ~200 ms on the reporter's own chain.
    ///
    /// ⚠ A warm render produces no frame and changes no pixel, so nothing in any output can see
    /// it and every equivalence test in the workspace passes with the mechanism deleted. The
    /// counter is the only thing that can.
    #[test]
    fn a_settled_scoped_render_warms_the_whole_buffer_checkpoint_behind_it() {
        let buf = varied_rect(128, 96);
        let engine = chained_engine(&buf);
        engine.request(chained_edits(), chained_spec());
        let frame = wait_for_frame(&engine);
        assert_eq!(
            (frame.width, frame.height),
            (24, 18),
            "fixture check: the region render really happened"
        );
        wait_until(|| engine.warm_renders() == 1, "the warm-behind render");
    }

    /// ⚠ The other half, and a test asserting only the first would pass against a worker that
    /// warmed after **every** render — which would spend a whole-buffer prefix behind a render
    /// that had just built one.
    #[test]
    fn a_whole_buffer_render_warms_nothing_behind_it() {
        let buf = varied_rect(128, 96);
        let engine = chained_engine(&buf);
        engine.request(chained_edits(), FrameSpec::fit(128));
        let _ = wait_for_frame(&engine);
        std::thread::sleep(WARM_BEHIND * 3);
        assert_eq!(
            engine.warm_renders(),
            0,
            "nothing was scoped, so there is no checkpoint to warm"
        );
    }

    /// ⚠⚠ **The warm-behind wait TAKES a request out of the channel to learn the gesture is still
    /// live, so a dropped one is the way this mechanism can lose a frame** — and a lost frame
    /// during a drag is the canvas silently showing a stale position, which is a bug this project
    /// has already shipped once (the mask overlay kept its previous position until some unrelated
    /// edit happened to request a frame). Every request issued must produce a frame.
    ///
    /// ⚠ What this **cannot** assert is that no warm render fires *mid*-gesture. That property is
    /// held structurally by `recv_timeout` rather than by a test, and deliberately so: asserting
    /// it means asserting that six renders finish inside 150 ms of wall clock, which is an
    /// assertion about OS scheduling. This project has three recorded CI flakes from exactly that,
    /// and `wait_for_frame`'s own deadline is the only timing this test depends on.
    #[test]
    fn a_gesture_loses_no_frame_and_warms_once_it_stops() {
        let buf = varied_rect(128, 96);
        let engine = chained_engine(&buf);
        let spec = chained_spec();
        for i in 0..6 {
            let mut e = chained_edits();
            e.geometry.angle = 4.0 + i as f32 * 0.25;
            engine.request(e, spec);
            // ⚠ Panics at its own deadline if the frame never comes, which is what makes
            // "nothing was dropped" an assertion rather than a hope.
            let frame = wait_for_frame(&engine);
            assert_eq!(
                (frame.width, frame.height),
                (24, 18),
                "frame {i} came back the wrong size"
            );
        }
        wait_until(
            || engine.warm_renders() >= 1,
            "the warm-behind render once the gesture stops",
        );
    }

    /// ⚠⚠ **A handed-off frame is settled by a DOWNLOAD, not a render (2026-09-26).** The
    /// warm-behind used to re-run the last step and a whole tail nobody saw; now it reads the
    /// resident final into the CPU cache — `settle_downloads` says so, `warm_renders` stays 0 —
    /// and the next tail frame renders what a from-scratch render renders.
    #[test]
    fn a_handed_off_frame_is_settled_by_a_download() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let engine = chained_engine(&hazy_buffer());
        let mut e = hsl_edits();
        e.geometry.angle = 3.0;
        let spec = FrameSpec {
            gpu_tail: true,
            ..FrameSpec::fit(200)
        };
        engine.request(e.clone(), spec);
        let _ = wait_for_frame(&engine);
        wait_until(|| engine.settle_downloads() == 1, "the settle download");
        std::thread::sleep(WARM_BEHIND * 3);
        assert_eq!(engine.warm_renders(), 0, "nothing was re-rendered");
        e.exposure = 0.7;
        engine.request(e.clone(), spec);
        let frame = wait_for_frame(&engine);
        let (want, _) = render_cached(
            &hazy_buffer(),
            &e,
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!(
            frame.rgba, want.rgba,
            "the settled final feeds the next tail frame"
        );
    }

    #[test]
    fn every_cached_frame_matches_a_cold_render() {
        // The master test: caching must be invisible in the output. The sequence walks a pure
        // hit, a tail-only change (resumes at the final), a dehaze change (resumes after NR),
        // an NR change (resumes after geometry), a geometry change (resumes at the seed), and
        // returns to earlier states so a stale checkpoint would show up.
        let buf = varied();
        // ⚠⚠ **An auto-masked brush is in the sequence, and it is what makes a stale base
        // visible.** Every other test of the coverage key asserts on *presence*; a key that admits
        // a stale base produces a map that is present and **wrong**, and only comparing pixels
        // against a cold render can see that. `b` below moves a **global** slider, which really
        // does change this mask's base — so the return leg is where a stale hit would show.
        let a = Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.5, 0.5]],
                        radius: 0.35,
                        hardness: 100.0,
                        flow: 100.0,
                        auto_mask: true,
                        tolerance: 35.0,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..full_prefix()
        };
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // tail only — and it changes the mask's base
        let c = Edits {
            dehaze: edit_model::Dehaze { amount: 85.0 },
            ..a.clone()
        }; // deepest prefix stage
        let d = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 10.0,
                color: 10.0,
                ..Default::default()
            },
            ..a.clone()
        }; // middle prefix stage
        let e = Edits {
            geometry: edit_model::Geometry {
                angle: 9.0,
                ..Default::default()
            },
            ..a.clone()
        }; // first prefix stage

        // Guard against the whole test passing trivially: these must actually differ.
        let cold_a = render_once(
            &buf,
            &a,
            8,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        for (name, other) in [("b", &b), ("c", &c), ("d", &d), ("e", &e)] {
            assert_ne!(
                cold_a.rgba,
                render_once(
                    &buf,
                    other,
                    8,
                    1.0,
                    color::OutputSpace::Srgb,
                    pipeline::Resampler::default(),
                    None,
                )
                .rgba,
                "edit set {name} renders identically to a — the test proves nothing"
            );
        }

        let engine = RenderEngine::new(
            buf.clone(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        for (i, ed) in [&a, &a, &b, &c, &a, &d, &e, &a, &c].iter().enumerate() {
            engine.request((*ed).clone(), FrameSpec::fit(8));
            let got = wait_for_frame(&engine);
            let cold = render_once(
                &buf,
                ed,
                8,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            );
            assert_eq!(got.rgba, cold.rgba, "frame {i} differs from a cold render");
        }
    }

    #[test]
    fn produces_rgba_frame_of_expected_size() {
        let engine = RenderEngine::new(
            tiny(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        engine.request(Edits::default(), FrameSpec::fit(4)); // downscale 8 -> 4
        let f = wait_for_frame(&engine);
        assert_eq!((f.width, f.height), (4, 4));
        assert_eq!(f.rgba.len(), 4 * 4 * 4);
        // mid-gray linear 0.5 -> sRGB ~188; alpha 255
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
        assert_eq!(f.rgba[3], 255);
    }

    #[test]
    fn exposure_brightens() {
        let engine = RenderEngine::new(
            tiny(),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        engine.request(
            Edits {
                exposure: 2.0,
                ..Edits::default()
            },
            FrameSpec::fit(4),
        );
        let f = wait_for_frame(&engine);
        assert!(f.rgba[0] > 240); // much brighter
    }

    #[test]
    fn requests_are_counted() {
        // ⚠⚠ This counter exists because "a drag mutated `Edits` but never asked for a render"
        // is invisible from outside — the request goes into a channel and vanishes. That bug
        // shipped: the mask overlay kept showing its previous position until some *unrelated*
        // edit happened to request a frame, which is why moving a slider appeared to fix it.
        //
        // ⚠ The alternative was asserting on `Session::requested_region`, which is a *proxy*
        // that misses exactly the whole-image fit case where the bug actually occurred.
        let spec = || FrameSpec {
            max_dim: 64,
            region: None,
            display_space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: pipeline::Resampler::default(),
            gpu_tail: false,
            visualize: None,
            soft_proof: None,
            grade_view: None,
        };
        let engine = RenderEngine::new(
            Arc::new(ImageBuf::new(4, 2)),
            1.0,
            None,
            color::wb::Illuminant::default(),
            Default::default(),
            true,
            || {},
        );
        assert_eq!(engine.requests_issued(), 0, "nothing asked for yet");

        engine.request(Edits::default(), spec());
        assert_eq!(engine.requests_issued(), 1);

        engine.request(Edits::default(), spec());
        assert_eq!(engine.requests_issued(), 2, "each request counts");
    }
}

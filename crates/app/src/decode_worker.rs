//! Background decode worker: decodes a file into the linear editing buffer off the UI
//! thread, one thread per request (capped), and renders the neutral baseline frame while
//! it already holds the buffer.
//!
//! Superseded requests are **abandoned, not aborted**: the doomed thread runs to completion
//! (libraw offers no practical mid-`unpack` cancellation) while the newly requested photo
//! decodes in parallel. Callers drop stale results by comparing sequence numbers.

use crate::render_engine::{render_once, RenderedFrame};
use core_image::ImageBuf;
use edit_model::Edits;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// A decoded photo, ready to become an editing session.
/// What the file said about its lens, and what we could match it to.
///
/// ⚠ Resolved on the **decode thread**, not the UI thread: matching needs the bundled LensFun
/// database, and even behind a `OnceLock` the first parse decompresses 56 XML files.
#[derive(Default, Clone)]
pub struct LensFacts {
    /// The lens name the file recorded, if any.
    pub name: Option<String>,
    /// The matched profile plus this shot's focal length and aperture, if one was found.
    pub shot: Option<lens_profile::LensShot>,
}

pub struct Decoded {
    /// The linear working buffer (shared with the render engine and the cache).
    pub buffer: Arc<ImageBuf>,
    /// Display label, e.g. "Fujifilm X-T4" or "JPEG 6000×4000".
    pub label: String,
    /// Neutral-edits render, used for the before/after baseline texture.
    pub baseline: RenderedFrame,
    /// Size of `buffer` relative to a full-resolution decode — 0.5 for a half-size RAW
    /// decode. The render engine needs it to scale the spatial stages' radii.
    pub source_scale: f32,
    /// The lens, and whether a profile matched it.
    pub lens: LensFacts,
    /// The illuminant the decode left the picture balanced to — the zero point of the Kelvin
    /// slider. D65 for anything that is not a RAW, which is what a camera JPEG genuinely is.
    pub wb_baseline: color::wb::Illuminant,
    /// The camera profile's tone curve, when one was loaded and it has one.
    pub profile: ProfileParts,
    /// Whether the buffer holds **scene-referred** light.
    ///
    /// ⚠⚠ False for JPEG/PNG/TIFF, which have *already* been rendered by whatever produced them.
    /// The display rendering is skipped entirely when this is false — applying ours on top of a
    /// camera's would render the photograph twice. The decoder is the only component that knows,
    /// so the flag rides with the decode rather than being inferred from the extension later.
    pub scene_referred: bool,
}

/// What the **tail's rendering** needs, owned so the render worker can capture it.
///
/// ⚠ One struct rather than separate fields, because they must always travel together: a curve
/// that arrived without its baseline would render under this renderer's own measured constant
/// instead of the profile's, which is a wrong picture rather than an error. Grouping them also
/// keeps `RenderEngine::new` inside clippy's argument limit.
///
/// ⚠ **The name is now narrower than the contents** — `looks_dir` is not a camera-profile field.
/// A deliberate wave-scoping choice rather than a judgement: `ProfileParts` and
/// `pipeline::TailRendering` are the owned and borrowed halves of one seam and want the same
/// rename to `TailRendering`, and renaming both would churn call sites in files another track is
/// editing concurrently. Worth doing once the wave merges.
#[derive(Debug, Clone, Default)]
pub struct ProfileParts {
    /// The profile's `ProfileToneCurve`, when it has one.
    pub curve: Option<Vec<[f32; 2]>>,
    /// The profile's `BaselineExposure` in stops. `None` falls back to the renderer's own
    /// measured constant; `Some(0.0)` is a profile declaring no shift, which is different.
    pub baseline_ev: Option<f32>,
    /// Where creative looks are read from, so the render worker can resolve `Edits.look.file`.
    ///
    /// ⚠ The **directory**, not the loaded table: the look is chosen by `Edits`, which arrives
    /// per request, while everything else here is fixed for the photo. Passing a table would
    /// mean rebuilding the engine on every look change.
    ///
    /// ⚠ `None` means no looks are available — every render then behaves as though the `.cube`
    /// were missing, which is the documented fallback rather than a failure.
    pub looks_dir: Option<std::path::PathBuf>,
    /// The look Camera matching applies to this photo (M10.13d) — resolved by the decode, which reads the
    /// file, and carried across the full-resolution handover with the rest of this struct.
    pub camera_look: Option<crate::camera_look::Resolved>,
}

/// What a decode reports about a file beyond its pixels.
///
/// ⚠ A struct rather than a tuple's tail because it reached four members and grew a fifth here;
/// a positional `f32` beside an `Illuminant` is exactly the argument that gets passed in the
/// wrong order with nothing to notice.
#[derive(Clone, Default)]
pub struct SourceFacts {
    /// Display label, e.g. "Fujifilm X-T4" or "JPEG 6000×4000".
    pub label: String,
    /// Size of the buffer relative to a full-resolution decode.
    pub source_scale: f32,
    pub lens: LensFacts,
    pub wb_baseline: color::wb::Illuminant,
    /// The camera profile's tone curve, when one was loaded and it has one.
    pub profile: ProfileParts,
    /// Whether the buffer holds **scene-referred** light — see [`Decoded::scene_referred`].
    pub scene_referred: bool,
}

/// The outcome of one decode request.
pub struct DecodeResult {
    /// Matches the value returned by [`DecodeWorker::request`]; anything older is stale.
    pub seq: u64,
    pub path: PathBuf,
    pub outcome: Result<Arc<Decoded>, String>,
}

/// Decodes a file into a linear editing buffer, a display label, and the buffer's size
/// relative to a full-resolution decode of the same file (0.5 for half-size RAW).
type DecodeFn = Arc<dyn Fn(&Path) -> Result<(ImageBuf, SourceFacts), String> + Send + Sync>;

struct Request {
    seq: u64,
    path: PathBuf,
    max_dim: usize,
}

/// Spawns a decode thread per request, up to `max_concurrent` at a time. Further requests
/// wait in a one-slot queue that always keeps the newest.
pub struct DecodeWorker {
    decode: DecodeFn,
    on_done: Arc<dyn Fn() + Send + Sync>,
    max_concurrent: usize,
    in_flight: usize,
    queued: Option<Request>,
    next_seq: u64,
    res_tx: Sender<DecodeResult>,
    res_rx: Receiver<DecodeResult>,
}

impl DecodeWorker {
    /// `decode` is injected so tests can drive the worker without libraw. `on_done` runs on
    /// the worker thread after each result (e.g. `ctx.request_repaint`).
    pub fn new(
        decode: impl Fn(&Path) -> Result<(ImageBuf, SourceFacts), String> + Send + Sync + 'static,
        max_concurrent: usize,
        on_done: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (res_tx, res_rx) = channel();
        Self {
            decode: Arc::new(decode),
            on_done: Arc::new(on_done),
            max_concurrent: max_concurrent.max(1),
            in_flight: 0,
            queued: None,
            next_seq: 0,
            res_tx,
            res_rx,
        }
    }

    /// Queue a decode of `path`, rendering its baseline at `max_dim`. Returns the sequence
    /// number identifying this request.
    pub fn request(&mut self, path: PathBuf, max_dim: usize) -> u64 {
        self.next_seq += 1;
        let req = Request {
            seq: self.next_seq,
            path,
            max_dim,
        };
        if self.in_flight < self.max_concurrent {
            self.spawn(req);
        } else {
            // Newest wins: an older queued request is dropped, never run.
            self.queued = Some(req);
        }
        self.next_seq
    }

    /// How many decodes have been asked for, ever.
    ///
    /// ⚠ Not `#[cfg(test)]`, for the reason `RenderEngine::requests_issued` records: this is the
    /// only quantity that says "a decode was *requested*" synchronously. `in_flight` is not a
    /// substitute — a decode that has already finished, or failed, is no longer in flight, so a
    /// test asserting on it would pass whether or not the request was ever made.
    pub fn requests_issued(&self) -> u64 {
        self.next_seq
    }

    /// Collect finished results and start the queued request if a slot freed up.
    /// Call once per frame.
    pub fn poll(&mut self) -> Vec<DecodeResult> {
        let mut out = Vec::new();
        while let Ok(res) = self.res_rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            out.push(res);
        }
        while self.in_flight < self.max_concurrent {
            match self.queued.take() {
                Some(req) => self.spawn(req),
                None => break,
            }
        }
        out
    }

    fn spawn(&mut self, req: Request) {
        let decode = self.decode.clone();
        let on_done = self.on_done.clone();
        let tx = self.res_tx.clone();
        self.in_flight += 1;
        std::thread::spawn(move || {
            let outcome = decode(&req.path).map(|(image, facts)| {
                let source_scale = facts.source_scale;
                let buffer = Arc::new(image);
                // ⚠ `Srgb` regardless of the display setting: this baseline texture is never drawn.
                // `show_before` is an ordinary render now, and the baseline survives only for
                // its dimensions (`develop/panel/geometry.rs`).
                let baseline = render_once(
                    &buffer,
                    &Edits::default(),
                    req.max_dim,
                    source_scale,
                    color::Display::SRGB,
                    // ⚠ The kernel is irrelevant here and provably so, not merely by
                    // assumption: `Edits::default()` activates no geometry, no perspective and
                    // no lens correction, so `build_prefix_steps` produces no resampling stage
                    // at all. The parameter is passed rather than the function reaching for a
                    // constant, so that a baseline rendered with *real* edits one day would
                    // have to decide.
                    pipeline::Resampler::default(),
                    // ⚠ `None` and provably harmless, by the same argument as the resampler
                    // above: `Edits::default()` activates no dehaze, so no airlight is ever
                    // consulted. Passed rather than assumed, so a baseline rendered with real
                    // edits one day would have to decide.
                    None,
                );
                Arc::new(Decoded {
                    buffer,
                    label: facts.label,
                    baseline,
                    source_scale,
                    lens: facts.lens,
                    wb_baseline: facts.wb_baseline,
                    profile: facts.profile,
                    scene_referred: facts.scene_referred,
                })
            });
            let _ = tx.send(DecodeResult {
                seq: req.seq,
                path: req.path,
                outcome,
            });
            on_done();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn tiny_decode(_path: &Path) -> Result<(ImageBuf, SourceFacts), String> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Ok((
            img,
            SourceFacts {
                label: "FAKE".to_string(),
                source_scale: 1.0,
                ..SourceFacts::default()
            },
        ))
    }

    /// Poll until `n` results have arrived (or 5 s elapse).
    fn wait_for(worker: &mut DecodeWorker, n: usize) -> Vec<DecodeResult> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < n && Instant::now() < deadline {
            got.extend(worker.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    #[test]
    fn delivers_buffer_label_and_baseline() {
        let mut w = DecodeWorker::new(tiny_decode, 2, || {});
        let seq = w.request(PathBuf::from("/fake/a.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seq, seq);
        let d = got[0].outcome.as_ref().expect("decode should succeed");
        assert_eq!(d.label, "FAKE");
        assert_eq!(d.source_scale, 1.0);
        assert_eq!(d.buffer.width(), 8);
        // The baseline is rendered at max_dim, i.e. the 8x8 buffer downscaled to 4x4.
        assert_eq!((d.baseline.width, d.baseline.height), (4, 4));
    }

    #[test]
    fn propagates_errors_and_keeps_serving() {
        let mut w = DecodeWorker::new(
            |p: &Path| {
                if p.ends_with("bad.raf") {
                    Err("boom".to_string())
                } else {
                    tiny_decode(p)
                }
            },
            2,
            || {},
        );
        w.request(PathBuf::from("/fake/bad.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert_eq!(
            got[0].outcome.as_ref().err().map(String::as_str),
            Some("boom")
        );

        w.request(PathBuf::from("/fake/good.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert!(got[0].outcome.is_ok());
    }

    #[test]
    fn never_runs_more_than_max_concurrent_decodes() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (l, p) = (live.clone(), peak.clone());
        let mut w = DecodeWorker::new(
            move |path: &Path| {
                let n = l.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(80));
                l.fetch_sub(1, Ordering::SeqCst);
                tiny_decode(path)
            },
            2,
            || {},
        );
        for i in 0..4 {
            w.request(PathBuf::from(format!("/fake/{i}.raf")), 4);
        }
        let _ = wait_for(&mut w, 3);
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn queue_keeps_only_the_newest_request() {
        // Capacity 1: "a" occupies the worker while b, c, d queue — only the newest (d) runs.
        let mut w = DecodeWorker::new(
            |p: &Path| {
                std::thread::sleep(Duration::from_millis(60));
                tiny_decode(p)
            },
            1,
            || {},
        );
        w.request(PathBuf::from("/fake/a.raf"), 4);
        w.request(PathBuf::from("/fake/b.raf"), 4);
        w.request(PathBuf::from("/fake/c.raf"), 4);
        let seq_d = w.request(PathBuf::from("/fake/d.raf"), 4);

        let got = wait_for(&mut w, 2);
        let names: Vec<String> = got
            .iter()
            .map(|r| r.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.raf".to_string(), "d.raf".to_string()]);
        assert_eq!(got.last().unwrap().seq, seq_d);
    }
}

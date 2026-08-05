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
type DecodeFn = Arc<dyn Fn(&Path) -> Result<(ImageBuf, String, f32), String> + Send + Sync>;

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
        decode: impl Fn(&Path) -> Result<(ImageBuf, String, f32), String> + Send + Sync + 'static,
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
            let outcome = decode(&req.path).map(|(image, label, source_scale)| {
                let buffer = Arc::new(image);
                let baseline = render_once(&buffer, &Edits::default(), req.max_dim, source_scale);
                Arc::new(Decoded {
                    buffer,
                    label,
                    baseline,
                    source_scale,
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

    fn tiny_decode(_path: &Path) -> Result<(ImageBuf, String, f32), String> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Ok((img, "FAKE".to_string(), 1.0))
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

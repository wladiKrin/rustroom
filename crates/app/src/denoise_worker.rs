//! AI denoise's background job (M10.16b): one thread, one photo at a time — the tiled run, its
//! progress, the cache file written — so the window never waits on minutes of network.
//!
//! ⚠⚠ **The newest request wins.** A request names the job the app WANTS; a running job for any
//! other key cancels itself at its next tile and is never delivered — so switching photos mid-run
//! cannot hand the previous photo's result to the next one (Review Focus 2).

use crate::ai_denoise::{self, TileNet};
use core_image::ImageBuf;
use std::sync::{Arc, Mutex};

/// One photo's result: from the cache when it is there, else from the network over `image`.
pub struct DenoiseRequest {
    /// `denoise_cache::key` — echoed in the result, the file it is stored under.
    pub key: String,
    /// The full-resolution decode; `None` to load from the cache only.
    pub image: Option<Arc<ImageBuf>>,
}

/// What came back.
pub enum DenoiseResult {
    Done {
        key: String,
        image: Arc<ImageBuf>,
    },
    /// The result so far (M10.16b4) — the original wherever no tile has landed; `done` tiles of it.
    /// ⚠ Never cached: only `Done` is.
    Partial {
        key: String,
        image: Arc<ImageBuf>,
        done: usize,
    },
    Failed {
        key: String,
        error: String,
    },
    /// A load found no usable result — none there, or a corrupt one, now removed: the photo goes the
    /// normal way, and this is no failure.
    Missing {
        key: String,
    },
}

/// Makes the network on the worker's thread — `tract`'s `Drunet` in the app, a fake in the tests.
pub type NetFactory = Box<dyn FnOnce() -> Result<Box<dyn TileNet + Send>, String> + Send>;

pub struct DenoiseWorker {
    tx: std::sync::mpsc::Sender<DenoiseRequest>,
    rx: std::sync::mpsc::Receiver<DenoiseResult>,
    /// The key the app wants now — a running job for another key stops at its next tile.
    wanted: Arc<Mutex<Option<String>>>,
    /// The running job's key and its tiles done of total.
    progress: Arc<Mutex<Option<(String, usize, usize)>>>,
    /// Where the view is, decode-normalised — the job's next tile is the one nearest it (M10.16b4).
    focus: Arc<Mutex<[f32; 2]>>,
}

impl DenoiseWorker {
    /// The worker's thread; `cache` (when given) is where a result is looked for first and written once
    /// computed. The result so far is published at most once a second.
    pub fn spawn(
        net: NetFactory,
        cache: Option<std::path::PathBuf>,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        Self::spawn_publishing(net, cache, repaint, std::time::Duration::from_secs(1))
    }

    /// [`DenoiseWorker::spawn`], publishing the result so far at most every `every`.
    pub fn spawn_publishing(
        net: NetFactory,
        cache: Option<std::path::PathBuf>,
        repaint: impl Fn() + Send + 'static,
        every: std::time::Duration,
    ) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<DenoiseRequest>();
        let (done, rx) = std::sync::mpsc::channel::<DenoiseResult>();
        let wanted: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let progress: Arc<Mutex<Option<(String, usize, usize)>>> = Arc::new(Mutex::new(None));
        let focus: Arc<Mutex<[f32; 2]>> = Arc::new(Mutex::new([0.5, 0.5]));
        let (want, prog, aim) = (
            Arc::clone(&wanted),
            Arc::clone(&progress),
            Arc::clone(&focus),
        );
        std::thread::Builder::new()
            .name("rustroom-denoise".into())
            .spawn(move || {
                let mut factory = Some(net);
                let mut net: Option<Box<dyn TileNet + Send>> = None;
                let mut load_error: Option<String> = None;
                let is_wanted = |key: &str| want.lock().is_ok_and(|w| w.as_deref() == Some(key));
                let set_progress = |p: Option<(String, usize, usize)>| {
                    *prog.lock().unwrap_or_else(|e| e.into_inner()) = p;
                };
                while let Ok(mut req) = jobs.recv() {
                    // ⚠ The newest queued request wins; anything older is not wanted any more.
                    while let Ok(newer) = jobs.try_recv() {
                        req = newer;
                    }
                    if !is_wanted(&req.key) {
                        continue;
                    }
                    let key = req.key;
                    let cached = cache
                        .as_deref()
                        .and_then(|d| crate::denoise_cache::load(d, &key));
                    let result = match (cached, req.image) {
                        (Some(img), _) => Some(Ok(img)),
                        (None, None) => {
                            // ⚠ A file there that did not load is corrupt: removed, so the photo goes
                            // the normal way and is computed again (Review Focus 5).
                            if let Some(d) = &cache {
                                crate::denoise_cache::remove(d, &key);
                            }
                            let _ = done.send(DenoiseResult::Missing { key });
                            repaint();
                            continue;
                        }
                        (None, Some(image)) => {
                            if net.is_none() && load_error.is_none() {
                                match factory.take().map(|f| f()) {
                                    Some(Ok(n)) => net = Some(n),
                                    Some(Err(e)) => load_error = Some(e),
                                    None => load_error = Some("the network did not load".into()),
                                }
                            }
                            match net.as_mut() {
                                None => Some(Err(load_error.clone().unwrap_or_default())),
                                Some(n) => {
                                    let mut last = std::time::Instant::now();
                                    let out = ai_denoise::run_progressive(
                                        &image,
                                        n.as_mut(),
                                        &|| *aim.lock().unwrap_or_else(|e| e.into_inner()),
                                        &mut |t| {
                                            set_progress(Some((key.clone(), t.done(), t.total())));
                                            if !is_wanted(&key) {
                                                return false;
                                            }
                                            if t.done() < t.total() && last.elapsed() >= every {
                                                last = std::time::Instant::now();
                                                let _ = done.send(DenoiseResult::Partial {
                                                    key: key.clone(),
                                                    image: Arc::new(t.result().clone()),
                                                    done: t.done(),
                                                });
                                                repaint();
                                            }
                                            true
                                        },
                                    );
                                    set_progress(None);
                                    // ⚠ Whatever the outcome: the GPU's activations go (Review Focus 4).
                                    n.finished();
                                    // Cancelled (`Ok(None)`): another photo is wanted now.
                                    out.transpose().inspect(|r| {
                                        // ⚠ Stored even if no longer wanted: it is this photo's
                                        // result all the same, and its next open finds it.
                                        if let (Ok(img), Some(dir)) = (r, &cache) {
                                            let _ = crate::denoise_cache::store(dir, &key, img);
                                        }
                                    })
                                }
                            }
                        }
                    };
                    let sent = match result {
                        Some(Ok(img)) if is_wanted(&key) => Some(DenoiseResult::Done {
                            key,
                            image: Arc::new(img),
                        }),
                        Some(Err(error)) => Some(DenoiseResult::Failed { key, error }),
                        _ => None,
                    };
                    if let Some(r) = sent {
                        let _ = done.send(r);
                        repaint();
                    }
                }
            })
            .expect("the denoise thread");
        Self {
            tx,
            rx,
            wanted,
            progress,
            focus,
        }
    }

    /// Where the view is now, decode-normalised — read by the running job before every tile.
    pub fn set_focus(&self, f: [f32; 2]) {
        *self.focus.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// Ask for `r` — and for nothing else: a job for another key now stops at its next tile.
    pub fn request(&self, r: DenoiseRequest) {
        *self.wanted.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.key.clone());
        let _ = self.tx.send(r);
    }

    /// Results that arrived since the last poll.
    pub fn poll(&self) -> Vec<DenoiseResult> {
        self.rx.try_iter().collect()
    }

    /// `key`'s tiles done of total, while it runs.
    pub fn progress(&self, key: &str) -> Option<(usize, usize)> {
        self.progress
            .lock()
            .ok()?
            .as_ref()
            .filter(|(k, _, _)| k == key)
            .map(|(_, d, t)| (*d, *t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A network that returns its input, slowly enough to be overtaken.
    struct Slow;
    impl TileNet for Slow {
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: &[f32]) -> Result<Vec<f32>, String> {
            std::thread::sleep(std::time::Duration::from_millis(30));
            Ok(rgb.to_vec())
        }
    }

    fn worker(cache: Option<std::path::PathBuf>) -> DenoiseWorker {
        DenoiseWorker::spawn(
            Box::new(|| Ok(Box::new(Slow) as Box<dyn TileNet + Send>)),
            cache,
            || {},
        )
    }

    /// A photo of `w × h` that takes several tiles.
    fn photo(w: usize, h: usize) -> Arc<ImageBuf> {
        Arc::new(ImageBuf::from_vec(w, h, vec![0.2; w * h * 3]))
    }

    /// A network that returns its input and counts the jobs it was told ended.
    struct Counting(Arc<std::sync::atomic::AtomicUsize>);
    impl TileNet for Counting {
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: &[f32]) -> Result<Vec<f32>, String> {
            Ok(rgb.to_vec())
        }
        fn finished(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Review Focus 4: the network is told when a job ends — the GPU frees its activations.
    #[test]
    fn the_network_is_told_when_a_job_ends() {
        let ended = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let e = ended.clone();
        let w = DenoiseWorker::spawn(
            Box::new(move || Ok(Box::new(Counting(e)) as Box<dyn TileNet + Send>)),
            None,
            || {},
        );
        w.request(DenoiseRequest {
            key: "a".into(),
            image: Some(photo(600, 300)),
        });
        assert_eq!(wait(&w, 1).len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(ended.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    fn wait(w: &DenoiseWorker, want: usize) -> Vec<DenoiseResult> {
        let mut got = Vec::new();
        for _ in 0..600 {
            got.extend(w.poll());
            if got.len() >= want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        got
    }

    #[test]
    fn a_job_runs_reports_progress_and_writes_the_cache() {
        let d = test_dir::TempDir::new("denoise-worker-run");
        let w = worker(Some(d.path().to_path_buf()));
        w.request(DenoiseRequest {
            key: "a".into(),
            image: Some(photo(600, 300)),
        });
        let mut seen = None;
        for _ in 0..200 {
            if let Some(p) = w.progress("a") {
                seen = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(seen.map(|p| p.1), Some(2), "two tiles across: {seen:?}");
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [DenoiseResult::Done { key, .. }] if key == "a"),
            "{} results",
            got.len()
        );
        assert!(
            crate::denoise_cache::load(d.path(), "a").is_some(),
            "the file was written"
        );
    }

    /// Review Focus 2: a newer request overtakes a running job, which is never delivered.
    #[test]
    fn a_job_for_another_photo_is_dropped() {
        let w = worker(None);
        w.request(DenoiseRequest {
            key: "first".into(),
            image: Some(photo(2000, 1200)),
        });
        std::thread::sleep(std::time::Duration::from_millis(60));
        w.request(DenoiseRequest {
            key: "second".into(),
            image: Some(photo(600, 300)),
        });
        let got = wait(&w, 1);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut keys: Vec<String> = got
            .into_iter()
            .chain(w.poll())
            .map(|r| match r {
                DenoiseResult::Done { key, .. }
                | DenoiseResult::Partial { key, .. }
                | DenoiseResult::Failed { key, .. }
                | DenoiseResult::Missing { key } => key,
            })
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["second".to_string()],
            "the first was not dropped"
        );
    }

    #[test]
    fn a_network_that_will_not_load_reports_a_failure() {
        let w = DenoiseWorker::spawn(Box::new(|| Err("no model".into())), None, || {});
        w.request(DenoiseRequest {
            key: "a".into(),
            image: Some(photo(64, 48)),
        });
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [DenoiseResult::Failed { key, error }] if key == "a" && error.contains("no model")),
            "{} results",
            got.len()
        );
    }

    /// Review Focus 5: a corrupt result is removed and reported missing — so the flow computes it
    /// again — never a failure that sticks for the run, never a garbage image.
    #[test]
    fn a_corrupt_cached_result_is_removed_and_reported_missing() {
        let d = test_dir::TempDir::new("denoise-worker-corrupt");
        std::fs::write(d.path().join("k.rrdn"), b"RRDN garbage").unwrap();
        assert!(crate::denoise_cache::exists(d.path(), "k"), "the fixture");
        let w = DenoiseWorker::spawn(
            Box::new(|| panic!("the network was made")),
            Some(d.path().to_path_buf()),
            || {},
        );
        w.request(DenoiseRequest {
            key: "k".into(),
            image: None,
        });
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [DenoiseResult::Missing { key }] if key == "k"),
            "{} results",
            got.len()
        );
        assert!(!crate::denoise_cache::exists(d.path(), "k"), "still there");
    }

    /// A cached result is loaded without making the network — the model need not be there.
    #[test]
    fn a_cached_result_is_loaded_without_the_network() {
        let d = test_dir::TempDir::new("denoise-worker-load");
        let stored = ImageBuf::from_vec(8, 4, vec![0.25; 8 * 4 * 3]);
        crate::denoise_cache::store(d.path(), "k", &stored).unwrap();
        let w = DenoiseWorker::spawn(
            Box::new(|| panic!("the network was made")),
            Some(d.path().to_path_buf()),
            || {},
        );
        w.request(DenoiseRequest {
            key: "k".into(),
            image: None,
        });
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [DenoiseResult::Done { key, image }] if key == "k" && image.width() == 8),
            "{} results",
            got.len()
        );
        w.request(DenoiseRequest {
            key: "absent".into(),
            image: None,
        });
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [DenoiseResult::Missing { key }] if key == "absent"),
            "a load of nothing is missing, not failed"
        );
    }

    /// Review Focus 3: partial results arrive while the job runs; the cache file appears only with the
    /// last tile; the finished result comes last.
    #[test]
    fn partial_results_arrive_and_only_the_finished_one_is_stored() {
        let d = test_dir::TempDir::new("denoise-worker-partial");
        let w = DenoiseWorker::spawn_publishing(
            Box::new(|| Ok(Box::new(Slow) as Box<dyn TileNet + Send>)),
            Some(d.path().to_path_buf()),
            || {},
            std::time::Duration::ZERO,
        );
        w.request(DenoiseRequest {
            key: "a".into(),
            image: Some(photo(1100, 700)),
        });
        let mut partials = 0;
        let mut done = false;
        for _ in 0..600 {
            for r in w.poll() {
                match r {
                    DenoiseResult::Partial { key, .. } => {
                        assert_eq!(key, "a");
                        assert!(!done, "a partial after the finished result");
                        partials += 1;
                        if partials == 1 {
                            assert!(
                                !crate::denoise_cache::exists(d.path(), "a"),
                                "a partial was stored"
                            );
                        }
                    }
                    DenoiseResult::Done { .. } => done = true,
                    _ => panic!("a failure"),
                }
            }
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(done && partials >= 2, "{partials} partials, done {done}");
        assert!(crate::denoise_cache::exists(d.path(), "a"));
    }

    /// The focus set before a job reaches it: the first tile is the one nearest it.
    #[test]
    fn the_focus_reaches_the_job() {
        struct Where(Arc<std::sync::Mutex<Vec<[f32; 3]>>>);
        impl TileNet for Where {
            fn run(
                &mut self,
                rgb: &[f32],
                _: usize,
                _: usize,
                _: &[f32],
            ) -> Result<Vec<f32>, String> {
                self.0.lock().unwrap().push([rgb[0], rgb[1], rgb[2]]);
                Ok(rgb.to_vec())
            }
        }
        let seen: Arc<std::sync::Mutex<Vec<[f32; 3]>>> = Default::default();
        let s = Arc::clone(&seen);
        let w = DenoiseWorker::spawn(
            Box::new(move || Ok(Box::new(Where(s)) as Box<dyn TileNet + Send>)),
            None,
            || {},
        );
        w.set_focus([1.0, 1.0]);
        let (pw, ph) = (1100, 700);
        let enc = crate::ai_denoise::Encoding { scale: 1.0 };
        let img = Arc::new(ImageBuf::from_vec(
            pw,
            ph,
            (0..pw * ph)
                .flat_map(|i| {
                    enc.decode([
                        (i % pw) as f32 / pw as f32,
                        (i / pw) as f32 / ph as f32,
                        0.5,
                    ])
                })
                .collect(),
        ));
        w.request(DenoiseRequest {
            key: "a".into(),
            image: Some(img),
        });
        // ⚠ Until the FINISHED result: a slow (debug) job publishes a partial one after a second.
        let mut finished = false;
        for _ in 0..1000 {
            finished |= w
                .poll()
                .iter()
                .any(|r| matches!(r, DenoiseResult::Done { .. }));
            if finished {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(finished, "the job never finished");
        // ⚠ The job encodes in the photo's own encoding (`Encoding::of`), monotonic in x and y: the
        // bottom-right tile's corner has the largest of both among every tile's corner.
        let all = seen.lock().unwrap();
        assert_eq!(all.len(), 6, "3 × 2 tiles");
        let (mx, my) = all
            .iter()
            .fold((0f32, 0f32), |(a, b), p| (a.max(p[0]), b.max(p[1])));
        assert_eq!(
            (all[0][0], all[0][1]),
            (mx, my),
            "the bottom-right tile first: {:?}",
            all[0]
        );
    }
}

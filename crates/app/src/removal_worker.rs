//! The Remove brush's background job (M10.16c1): one thread, one fill at a time, in the order asked —
//! the cache first, else the network over the decode — so the window never waits on a fill.
//!
//! ⚠ Unlike AI denoise's worker, a newer request does not cancel an older one: each is one removal of
//! the photo, all of them wanted. The app drops results for a photo it has left.

use crate::removal::{Detector, Net};
use core_image::ImageBuf;
use pipeline::Fill;
use std::sync::Arc;

/// One removal's fill: from the cache when it is there, else computed over `image`.
pub struct RemovalRequest {
    /// `remove_tool::id` — echoed in the result, the file it is stored under.
    pub key: String,
    /// The full-resolution decode; `None` to load from the cache only.
    pub image: Option<Arc<ImageBuf>>,
    pub removal: edit_model::Removal,
    /// The photo's film negative — the network sees the positive (`remove_tool::fill`).
    pub negative: edit_model::Negative,
    /// The photo's clockwise quarter turns to upright — the detector sees the window so (M10.16c3).
    pub turns: u8,
}

/// What came back.
pub enum RemovalResult {
    /// What the fill will cover, sent before the network runs (M10.16c1b) — only for a fill being computed.
    Area {
        key: String,
        area: Arc<crate::remove_tool::Area>,
    },
    Done {
        key: String,
        fill: Arc<Fill>,
    },
    Failed {
        key: String,
        error: String,
    },
    /// A load found no usable fill — none there, or a corrupt one, now removed: it is computed the normal
    /// way, and this is no failure.
    Missing {
        key: String,
    },
}

/// Makes a network on the worker's thread — LaMa and MobileSAM in the app, stand-ins in the tests.
pub type NetFactory = Box<dyn FnOnce() -> Result<Net, String> + Send>;
pub type DetectFactory = Box<dyn FnOnce() -> Result<Detector, String> + Send>;

pub struct RemovalWorker {
    tx: std::sync::mpsc::Sender<RemovalRequest>,
    rx: std::sync::mpsc::Receiver<RemovalResult>,
}

impl RemovalWorker {
    /// The worker's thread; `cache` (when given) is where a fill is looked for first and written once
    /// computed. Each network is made on its first use, once — a failure to make it fails every fill
    /// that needs it, with its reason.
    pub fn spawn(
        net: NetFactory,
        detect: DetectFactory,
        cache: Option<std::path::PathBuf>,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<RemovalRequest>();
        let (done, rx) = std::sync::mpsc::channel::<RemovalResult>();
        std::thread::Builder::new()
            .name("rustroom-remove".into())
            .spawn(move || {
                let mut nets = Lazy::new(net);
                let mut detectors = Lazy::new(detect);
                while let Ok(req) = jobs.recv() {
                    let key = req.key.clone();
                    let on_area = |area| {
                        let _ = done.send(RemovalResult::Area {
                            key: key.clone(),
                            area,
                        });
                        repaint();
                    };
                    let result = run(req, cache.as_deref(), &mut nets, &mut detectors, &on_area);
                    let _ = done.send(match result {
                        Ok(Some(fill)) => RemovalResult::Done { key, fill },
                        Ok(None) => RemovalResult::Missing { key },
                        Err(error) => RemovalResult::Failed { key, error },
                    });
                    repaint();
                }
            })
            .expect("the remove thread starts");
        Self { tx, rx }
    }

    pub fn request(&self, req: RemovalRequest) {
        let _ = self.tx.send(req);
    }

    /// Everything that came back since the last call.
    pub fn poll(&self) -> Vec<RemovalResult> {
        self.rx.try_iter().collect()
    }
}

/// A thing made on first use, once; its failure kept.
struct Lazy<T> {
    make: Option<Box<dyn FnOnce() -> Result<T, String> + Send>>,
    made: Option<Result<T, String>>,
}

impl<T> Lazy<T> {
    fn new(make: Box<dyn FnOnce() -> Result<T, String> + Send>) -> Self {
        Self {
            make: Some(make),
            made: None,
        }
    }

    fn get(&mut self) -> Result<&mut T, String> {
        if self.made.is_none() {
            let make = self.make.take().expect("made once");
            self.made = Some(make());
        }
        match self.made.as_mut().expect("made above") {
            Ok(t) => Ok(t),
            Err(e) => Err(e.clone()),
        }
    }
}

/// One request: the cached fill, `None` for a load that found none (a corrupt file removed), or the fill
/// computed and stored — its area handed to `on_area` before the network runs.
fn run(
    req: RemovalRequest,
    cache: Option<&std::path::Path>,
    nets: &mut Lazy<Net>,
    detectors: &mut Lazy<Detector>,
    on_area: &dyn Fn(Arc<crate::remove_tool::Area>),
) -> Result<Option<Arc<Fill>>, String> {
    if let Some(f) = cache.and_then(|d| crate::removal_cache::load(d, &req.key)) {
        return Ok(Some(Arc::new(f)));
    }
    let Some(image) = req.image else {
        if let Some(d) = cache {
            crate::removal_cache::remove(d, &req.key);
        }
        return Ok(None);
    };
    let detector = if crate::remove_tool::needs_detector(&req.removal) {
        Some(detectors.get()?)
    } else {
        None
    };
    let prepared = crate::remove_tool::prepare(
        &image,
        &req.removal,
        &req.negative,
        req.turns,
        detector.map(|d| d.as_mut() as crate::remove_tool::Detect<'_>),
    )?;
    on_area(Arc::new(prepared.area()));
    let net = nets.get()?;
    let fill = crate::remove_tool::finish(prepared, &req.removal, &req.negative, net.as_mut())?;
    if let Some(d) = cache {
        let _ = crate::removal_cache::store(d, &req.key, &fill);
    }
    Ok(Some(Arc::new(fill)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removal() -> edit_model::Removal {
        edit_model::Removal {
            points: vec![[0.5, 0.5]],
            radius: 0.02,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: None,
        }
    }

    fn image() -> Arc<ImageBuf> {
        Arc::new(ImageBuf::from_vec(
            800,
            600,
            (0..800 * 600 * 3).map(|i| (i % 53) as f32 / 53.0).collect(),
        ))
    }

    /// A ceiling against a hang, never a time budget — ten seconds lost the race on a loaded CI runner (2026-11-24).
    const LONG: std::time::Duration = std::time::Duration::from_secs(120);

    /// The first `n` results, waited for up to [`LONG`].
    fn wait(w: &RemovalWorker, n: usize) -> Vec<RemovalResult> {
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        while got.len() < n && start.elapsed() < LONG {
            got.extend(w.poll());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        got
    }

    /// The results that end a job — every one but an area.
    fn ends(got: Vec<RemovalResult>) -> Vec<RemovalResult> {
        got.into_iter()
            .filter(|r| !matches!(r, RemovalResult::Area { .. }))
            .collect()
    }

    fn never() -> NetFactory {
        Box::new(|| panic!("the network was made"))
    }

    fn no_detector() -> DetectFactory {
        Box::new(|| Err("no detector here".to_string()))
    }

    /// A computed fill is delivered and stored; asked again, it comes from the cache — the network is made
    /// once and never asked twice for it.
    #[test]
    fn a_computed_fill_is_stored_and_then_loaded() {
        let d = test_dir::TempDir::new("removal-worker-compute");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let net: NetFactory = Box::new(move || {
            Ok(Box::new(move |rgb: &[f32], _: &[f32]| {
                c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(rgb.to_vec())
            }) as Net)
        });
        let w = RemovalWorker::spawn(net, no_detector(), Some(d.path().to_path_buf()), || {});
        let req = |image: Option<Arc<ImageBuf>>| RemovalRequest {
            key: "k".into(),
            image,
            removal: removal(),
            negative: edit_model::Negative::default(),
            turns: 0,
        };
        w.request(req(Some(image())));
        let got = ends(wait(&w, 2));
        assert!(matches!(&got[..], [RemovalResult::Done { key, .. }] if key == "k"));
        assert!(crate::removal_cache::exists(d.path(), "k"), "stored");
        w.request(req(None));
        let got = wait(&w, 1);
        assert!(matches!(&got[..], [RemovalResult::Done { .. }]), "loaded");
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    /// M10.16c1b: a computed fill sends its area BEFORE the network runs — so the canvas shades it while it
    /// does — and the area is the fill's own mask. A cached fill sends no area: it is there at once.
    ///
    /// ⚠⚠ Order alone cannot see "before the network": an area sent after the network but ahead of the fill
    /// still arrives first, and the first version of this test stayed green under exactly that sabotage. So
    /// the stand-in network waits until the test has the area in hand, and records whether it was let go.
    #[test]
    fn the_area_arrives_before_the_network_runs() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = test_dir::TempDir::new("removal-worker-area");
        let go = Arc::new(AtomicBool::new(false));
        let let_go = Arc::new(AtomicBool::new(false));
        let (g, l) = (go.clone(), let_go.clone());
        let net: NetFactory = Box::new(move || {
            Ok(Box::new(move |rgb: &[f32], _: &[f32]| {
                let start = std::time::Instant::now();
                while !g.load(Ordering::SeqCst)
                    && start.elapsed() < std::time::Duration::from_secs(3)
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                l.store(g.load(Ordering::SeqCst), Ordering::SeqCst);
                Ok(rgb.to_vec())
            }) as Net)
        });
        let w = RemovalWorker::spawn(net, no_detector(), Some(d.path().to_path_buf()), || {});
        let req = |image: Option<Arc<ImageBuf>>| RemovalRequest {
            key: "k".into(),
            image,
            removal: removal(),
            negative: edit_model::Negative::default(),
            turns: 0,
        };
        w.request(req(Some(image())));
        let first = wait(&w, 1);
        let area = match &first[..] {
            [RemovalResult::Area { key, area }] if key == "k" => area.clone(),
            _ => panic!("expected the area first"),
        };
        go.store(true, Ordering::SeqCst);
        let fill = match &wait(&w, 1)[..] {
            [RemovalResult::Done { fill, .. }] => fill.clone(),
            _ => panic!("expected the fill"),
        };
        assert!(
            let_go.load(Ordering::SeqCst),
            "the network ran before the area was handed out"
        );
        assert_eq!((area.rect, &area.alpha), (fill.rect, &fill.alpha));
        w.request(req(None));
        let got = wait(&w, 1);
        assert!(
            matches!(&got[..], [RemovalResult::Done { .. }]),
            "a cached fill sends no area"
        );
    }

    /// A load with nothing cached is `Missing`, never a failure — and needs no network.
    #[test]
    fn a_load_with_nothing_cached_is_missing() {
        let d = test_dir::TempDir::new("removal-worker-missing");
        let w = RemovalWorker::spawn(never(), no_detector(), Some(d.path().to_path_buf()), || {});
        w.request(RemovalRequest {
            key: "absent".into(),
            image: None,
            removal: removal(),
            negative: edit_model::Negative::default(),
            turns: 0,
        });
        let got = wait(&w, 1);
        assert!(matches!(&got[..], [RemovalResult::Missing { key }] if key == "absent"));
    }

    /// A network that cannot be made fails each fill that needs it with its reason; the worker lives on.
    #[test]
    fn a_network_that_does_not_load_fails_with_its_reason() {
        let w = RemovalWorker::spawn(
            Box::new(|| Err("no LaMa here".to_string())),
            no_detector(),
            None,
            || {},
        );
        for k in ["a", "b"] {
            w.request(RemovalRequest {
                key: k.into(),
                image: Some(image()),
                removal: removal(),
                negative: edit_model::Negative::default(),
                turns: 0,
            });
        }
        // ⚠ Each sends its area first (M10.16c1b), then fails — the network is made after the area.
        let got = ends(wait(&w, 4));
        assert_eq!(got.len(), 2);
        for r in got {
            assert!(matches!(r, RemovalResult::Failed { error, .. } if error.contains("no LaMa")));
        }
    }
}

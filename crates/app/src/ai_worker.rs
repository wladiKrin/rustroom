//! The AI mask worker (M9.13): one background thread that runs the networks, keeps them loaded,
//! and writes every answer to the disk cache before replying.
//!
//! ⚠ `decode_worker`'s shape — an mpsc request channel, a result channel, a repaint callback — and
//! no egui. The networks sit behind [`Runner`] so the threading and the orientation arithmetic are
//! testable without a 200 MB model.
//!
//! ⚠⚠ **The network sees the photo UPRIGHT.** The decode is in the sensor's orientation; the
//! spike's first mask came out sideways because its loader ignored the rotation. So the input is
//! turned by `Orientation::rotate`, a click point is turned with it, and the map is turned back
//! before it is stored — the cache is always in decode orientation.

use ai_mask::{ProbMap, SubjectModel};
use core_image::ImageBuf;
use image::RgbImage;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What to ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiJob {
    Subject(SubjectModel),
    /// A click, decode-normalised.
    Click([f32; 2]),
}

pub struct AiRequest {
    /// The cache key (`ai_cache::key`) — echoed in the result.
    pub key: String,
    /// The photo, for the click embedding's identity: a second click on the same photo reuses it.
    pub photo: PathBuf,
    /// The model input, already upright (`model_input`).
    pub image: Arc<RgbImage>,
    /// Clockwise quarter turns that made it upright.
    pub turns: u8,
    pub job: AiJob,
}

pub struct AiResult {
    pub key: String,
    pub map: Result<Arc<ProbMap>, String>,
}

/// The networks, as the worker sees them.
pub trait Runner: Send {
    fn subject(&mut self, model: SubjectModel, img: &RgbImage) -> Result<ProbMap, String>;
    /// `p` is normalised over `img` (upright).
    fn click(&mut self, photo: &Path, img: &RgbImage, p: [f32; 2]) -> Result<ProbMap, String>;
}

/// The real networks through `ai_mask`, loaded on first use and kept.
pub struct TractRunner {
    models: PathBuf,
    subjects: Vec<(SubjectModel, ai_mask::Subject)>,
    sam: Option<ai_mask::Sam>,
    /// The last photo's embedding: a second click on it costs ~0.1 s instead of ~1.7 s.
    embedding: Option<(PathBuf, ai_mask::Embedding)>,
}

impl TractRunner {
    pub fn new(models: PathBuf) -> Self {
        Self {
            models,
            subjects: Vec::new(),
            sam: None,
            embedding: None,
        }
    }

    fn path(&self, id: ai_mask::ModelId) -> PathBuf {
        self.models.join(id.spec().file)
    }
}

impl Runner for TractRunner {
    fn subject(&mut self, model: SubjectModel, img: &RgbImage) -> Result<ProbMap, String> {
        if !self.subjects.iter().any(|(m, _)| *m == model) {
            let s = ai_mask::Subject::load(&self.path(model.model()), model)
                .map_err(|e| e.to_string())?;
            self.subjects.push((model, s));
        }
        let (_, s) = self
            .subjects
            .iter()
            .find(|(m, _)| *m == model)
            .expect("just loaded");
        s.run(img).map_err(|e| e.to_string())
    }

    fn click(&mut self, photo: &Path, img: &RgbImage, p: [f32; 2]) -> Result<ProbMap, String> {
        if self.sam.is_none() {
            self.sam = Some(ai_mask::Sam::new(
                &self.path(ai_mask::ModelId::SamEncoder),
                &self.path(ai_mask::ModelId::SamDecoder),
            ));
        }
        let sam = self.sam.as_mut().expect("just made");
        if self.embedding.as_ref().map(|(ph, _)| ph.as_path()) != Some(photo) {
            let e = sam.embed(img).map_err(|e| e.to_string())?;
            self.embedding = Some((photo.to_path_buf(), e));
        }
        let (_, e) = self.embedding.as_ref().expect("just embedded");
        sam.click(e, p).map_err(|e| e.to_string())
    }
}

/// A clockwise quarter turn of a normalised point: `(u, v) → (1 − v, u)`.
pub fn turn_point(p: [f32; 2], turns: u8) -> [f32; 2] {
    let mut q = p;
    for _ in 0..turns % 4 {
        q = [1.0 - q[1], q[0]];
    }
    q
}

/// The decode as a network wants it: sRGB-encoded 8-bit, exposure normalised, long side ≤ 1024,
/// turned upright by `turns` clockwise quarter turns.
///
/// ⚠ Networks are trained on ordinary photographs, not linear sensor data — a linear RAW decode
/// is dark and flat to them. So: ProPhoto → sRGB, scaled until the mean luminance is 0.18
/// (bounded, so a black frame is not amplified into noise), encoded, clipped.
pub fn model_input(buffer: &ImageBuf, turns: u8) -> RgbImage {
    let small = buffer.downscale_to(1024);
    let (w, h) = (small.width(), small.height());
    let n = (w * h).max(1) as f32;
    let mean: f32 = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| color::luminance(small.pixel(x, y)))
        .sum::<f32>()
        / n;
    let gain = (0.18 / mean.max(1e-6)).clamp(0.25, 8.0);
    let mut img = RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let p = color::prophoto_to_srgb(small.pixel(x as usize, y as usize));
        image::Rgb(
            p.map(|c| (color::linear_to_srgb((c * gain).clamp(0.0, 1.0)) * 255.0).round() as u8),
        )
    });
    for _ in 0..turns % 4 {
        img = image::imageops::rotate90(&img);
    }
    img
}

/// The worker thread's handle.
pub struct AiWorker {
    tx: Sender<AiRequest>,
    rx: Receiver<AiResult>,
}

impl AiWorker {
    /// Start the thread. `cache` is where maps are written; `on_done` wakes the UI.
    pub fn spawn(
        mut runner: Box<dyn Runner>,
        cache: Option<PathBuf>,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, jobs) = channel::<AiRequest>();
        let (done, rx) = channel();
        std::thread::spawn(move || {
            for r in jobs {
                let map = match r.job {
                    AiJob::Subject(m) => runner.subject(m, &r.image),
                    AiJob::Click(p) => runner.click(&r.photo, &r.image, turn_point(p, r.turns)),
                }
                // ⚠ Back to the decode's own orientation, the cache's only orientation.
                .map(|m| m.rotate_back(r.turns));
                if let (Ok(m), Some(dir)) = (&map, &cache) {
                    let _ = crate::ai_cache::store(dir, &r.key, m);
                }
                if done
                    .send(AiResult {
                        key: r.key,
                        map: map.map(Arc::new),
                    })
                    .is_err()
                {
                    return;
                }
                on_done();
            }
        });
        Self { tx, rx }
    }

    pub fn request(&self, r: AiRequest) {
        let _ = self.tx.send(r);
    }

    /// Every finished result since the last call.
    pub fn poll(&self) -> Vec<AiResult> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A network that answers "the point it was given" as a single hot pixel on a 4×2 map, and
    /// records what it saw.
    /// What the fake network was asked: the point, and the input's size.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<([f32; 2], (u32, u32))>>>;

    struct Fake {
        seen: Seen,
    }
    impl Runner for Fake {
        fn subject(&mut self, _: SubjectModel, img: &RgbImage) -> Result<ProbMap, String> {
            Ok(ProbMap::from_fn(img.width(), img.height(), |x, _| {
                if x == 0 {
                    1.0
                } else {
                    0.0
                }
            }))
        }
        fn click(&mut self, _: &Path, img: &RgbImage, p: [f32; 2]) -> Result<ProbMap, String> {
            self.seen
                .lock()
                .unwrap()
                .push((p, (img.width(), img.height())));
            Err("no model".into())
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_aiworker_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn wait(w: &AiWorker, n: usize) -> Vec<AiResult> {
        let mut out = Vec::new();
        for _ in 0..500 {
            out.extend(w.poll());
            if out.len() >= n {
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the worker answered {} of {n}", out.len());
    }

    #[test]
    fn a_quarter_turn_moves_a_point_and_four_bring_it_home() {
        let p = [0.2, 0.1];
        assert_eq!(turn_point(p, 0), p);
        let q = turn_point(p, 1);
        assert!(
            (q[0] - 0.9).abs() < 1e-6 && (q[1] - 0.2).abs() < 1e-6,
            "{q:?}"
        );
        let r = turn_point(p, 4);
        assert!((r[0] - p[0]).abs() < 1e-6 && (r[1] - p[1]).abs() < 1e-6);
    }

    #[test]
    fn the_model_input_is_encoded_normalised_and_upright() {
        // A 4×2 linear mid-grey buffer with one bright pixel at (3, 0).
        let mut b = ImageBuf::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                b.set_pixel(x, y, [0.18; 3]);
            }
        }
        b.set_pixel(3, 0, [0.9; 3]);
        let img = model_input(&b, 1);
        assert_eq!(
            (img.width(), img.height()),
            (2, 4),
            "one quarter turn swaps the axes"
        );
        // Rotated clockwise, the top-right pixel goes to the bottom-right.
        let bright = img.get_pixel(1, 3)[0];
        let grey = img.get_pixel(0, 0)[0];
        assert!(
            bright > grey,
            "the marked pixel did not travel: {bright} vs {grey}"
        );
        assert!(
            (95..=130).contains(&grey),
            "mid-grey should encode near 118: {grey}"
        );
    }

    #[test]
    fn a_subject_map_is_turned_back_and_cached_before_the_reply() {
        let cache = temp_dir("cache");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let w = AiWorker::spawn(
            Box::new(Fake { seen: seen.clone() }),
            Some(cache.clone()),
            || {},
        );
        // Upright image 2×4 (a portrait), seen after one clockwise turn.
        w.request(AiRequest {
            key: "k1".into(),
            photo: PathBuf::from("/p/A.RAF"),
            image: Arc::new(RgbImage::new(2, 4)),
            turns: 1,
            job: AiJob::Subject(SubjectModel::IsNet),
        });
        w.request(AiRequest {
            key: "k2".into(),
            photo: PathBuf::from("/p/A.RAF"),
            image: Arc::new(RgbImage::new(2, 4)),
            turns: 1,
            job: AiJob::Click([0.2, 0.1]),
        });
        let out = wait(&w, 2);
        assert_eq!(out[0].key, "k1");
        let m = out[0].map.as_ref().expect("the fake answers subjects");
        assert_eq!((m.w, m.h), (4, 2), "turned back to the decode's landscape");
        assert!(crate::ai_cache::load(&cache, "k1").is_some(), "not cached");
        // The click reached the network turned upright, and its error came back, uncached.
        assert!(out[1].map.is_err());
        assert!(crate::ai_cache::load(&cache, "k2").is_none());
        let (p, size) = seen.lock().unwrap()[0];
        assert!(
            (p[0] - 0.9).abs() < 1e-6 && (p[1] - 0.2).abs() < 1e-6,
            "{p:?}"
        );
        assert_eq!(size, (2, 4));
    }
}

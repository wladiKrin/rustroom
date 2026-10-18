//! AI denoise's result as the render reads it (M10.16b): the corrected full-resolution photo answered at
//! any frame's size, and the export's rule for finding — or computing — it.

use crate::ai_denoise::TileNet;
use core_image::ImageBuf;
use raw_decode::Demosaic;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The frames held at once — the fit rung and one native rung, an Amount drag's two sizes.
const HELD: usize = 2;

/// The denoised photo at full resolution, which the prefix's first step reads at the frame's size.
///
/// ⚠⚠ A size is answered only as `ImageBuf::downscale_to` of the full image — the SAME call that made the
/// prefix's input from the decode — so the blend lines up pixel for pixel. A size that call does not give
/// exactly is refused, and the step passes its input through. ⚠ It must be the decode's own size: the
/// session inserts it only over the full-resolution decode, an export only over a decode of its size.
pub struct Denoised {
    full: Arc<ImageBuf>,
    /// Downscaled frames, newest first — an Amount drag re-runs the blend, never the downscale.
    held: Mutex<Vec<Arc<ImageBuf>>>,
}

impl std::fmt::Debug for Denoised {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Denoised({:?})", self.size())
    }
}

impl Denoised {
    pub fn new(full: Arc<ImageBuf>) -> Self {
        Self {
            full,
            held: Mutex::new(Vec::new()),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.full.width(), self.full.height())
    }
}

impl pipeline::ImageSource for Denoised {
    fn image(&self, w: usize, h: usize) -> Option<Arc<ImageBuf>> {
        if (w, h) == self.size() {
            return Some(Arc::clone(&self.full));
        }
        let lock = || self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = lock().iter().find(|i| (i.width(), i.height()) == (w, h)) {
            return Some(Arc::clone(hit));
        }
        let d = self.full.downscale_to(w.max(h));
        if (d.width(), d.height()) != (w, h) {
            return None;
        }
        let d = Arc::new(d);
        let mut held = lock();
        held.retain(|i| (i.width(), i.height()) != (w, h));
        held.insert(0, Arc::clone(&d));
        held.truncate(HELD);
        Some(d)
    }
}

/// A result's identity, from its cache key — the key names the file, the decode and the recipe, so the
/// same key is the same pixels. ⚠ Not a hash of the samples: 312 MB of them would cost a second a result.
pub fn id_of(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish().max(1)
}

/// The demosaic quality `photo`'s key carries: a RAW's own; a JPEG, PNG or TIFF has none, so one fixed
/// value, and its preview's and export's keys agree.
pub fn tier_for(photo: &Path, tier: Demosaic) -> Demosaic {
    if catalog::is_raw(photo) {
        tier
    } else {
        Demosaic::default()
    }
}

/// The DRUNet network from the models directory, made on the thread that runs it.
pub fn drunet(models: &Path) -> Result<Box<dyn TileNet + Send>, String> {
    let spec = ai_mask::ModelId::DrunetColor.spec();
    let net = ai_mask::denoise::Drunet::load(&models.join(spec.file)).map_err(|e| e.to_string())?;
    Ok(Box::new(net))
}

/// How an export finds a photo's denoised result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseUse {
    /// The cache at the export's own demosaic quality, or the network run there and then — minutes a
    /// photo, and the export's status says so. The user's export.
    Compute,
    /// The cache only, at `tier` — the session's, where its jobs store. ⚠ A rendered preview never runs
    /// the network: without a result its build fails, and the photo is built again once one exists.
    Cached(Demosaic),
}

/// Where an export's result comes from: the results folder, and the models folder with the network
/// made from it — on the export's own thread.
pub struct Sources<F> {
    pub cache: Option<PathBuf>,
    pub models: Option<PathBuf>,
    pub net: F,
}

/// The real folders and the job's network — on the GPU when `gpu` (Settings' *Use the GPU*).
pub fn user_sources(
    gpu: bool,
) -> Sources<impl FnOnce(&Path) -> Result<Box<dyn TileNet + Send>, String>> {
    Sources {
        cache: crate::denoise_cache::dir(),
        models: crate::ai_cache::models_dir(),
        net: move |models: &Path| network(models, gpu),
    }
}

/// DRUNet's weights from the model file as the GPU crate's tensors.
pub fn gpu_weights(model: &Path) -> Result<gpu_pipeline::Weights, String> {
    Ok(ai_mask::denoise::weights(model)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(name, shape, data)| (name, gpu_pipeline::Tensor { shape, data }))
        .collect())
}

/// The network the job runs (M10.16b2): DRUNet on the GPU when `gpu` is asked and an adapter exists, with
/// `tract`'s behind it for any tile the GPU fails; otherwise `tract`'s alone.
pub fn network(models: &Path, gpu: bool) -> Result<Box<dyn TileNet + Send>, String> {
    if gpu {
        if let Some(ctx) = gpu_pipeline::shared().cloned() {
            let file = models.join(ai_mask::ModelId::DrunetColor.spec().file);
            match gpu_weights(&file).and_then(|w| gpu_pipeline::GpuDrunet::new(ctx, &w)) {
                Ok(g) => {
                    let m = models.to_path_buf();
                    return Ok(Box::new(Engine::new(
                        Some(Box::new(g)),
                        Box::new(move || drunet(&m)),
                    )));
                }
                Err(e) => eprintln!("rustroom: AI denoise runs on the CPU — {e}"),
            }
        }
    }
    drunet(models)
}

/// A factory for the fallback network, called at most once.
pub type Fallback = Box<dyn FnOnce() -> Result<Box<dyn TileNet + Send>, String> + Send>;

/// The primary network until it fails, then the fallback — made once, on the first failure — for that
/// tile and every later one. ⚠ A failed tile is never kept: it runs again on the fallback (Review
/// Focus 1).
pub struct Engine {
    primary: Option<Box<dyn TileNet + Send>>,
    fallback: Option<Fallback>,
    cpu: Option<Box<dyn TileNet + Send>>,
}

impl Engine {
    pub fn new(primary: Option<Box<dyn TileNet + Send>>, fallback: Fallback) -> Self {
        Self {
            primary,
            fallback: Some(fallback),
            cpu: None,
        }
    }
}

impl TileNet for Engine {
    fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: &[f32]) -> Result<Vec<f32>, String> {
        if let Some(p) = self.primary.as_mut() {
            match p.run(rgb, w, h, sigma) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    eprintln!("rustroom: AI denoise on the GPU failed ({e}); the CPU takes over");
                    self.primary = None;
                }
            }
        }
        if self.cpu.is_none() {
            let make = self.fallback.take().ok_or("the CPU network did not load")?;
            self.cpu = Some(make()?);
        }
        self.cpu.as_mut().expect("made above").run(rgb, w, h, sigma)
    }

    fn finished(&mut self) {
        for n in [self.primary.as_mut(), self.cpu.as_mut()]
            .into_iter()
            .flatten()
        {
            n.finished();
        }
    }
}

/// `image`'s denoised result for an export of `photo` decoded at `tier`, with its id — the cache, else
/// (under [`DenoiseUse::Compute`]) the network, `on_compute` called first. `keep_going` answers `false` to
/// cancel. A result is stored once computed.
pub fn for_export<F>(
    photo: &Path,
    image: &ImageBuf,
    tier: Demosaic,
    using: DenoiseUse,
    sources: Sources<F>,
    on_compute: impl FnOnce(),
    keep_going: &dyn Fn() -> bool,
) -> Result<(Arc<Denoised>, u64), String>
where
    F: FnOnce(&Path) -> Result<Box<dyn TileNet + Send>, String>,
{
    let Sources { cache, models, net } = sources;
    let tier = match using {
        DenoiseUse::Compute => tier,
        DenoiseUse::Cached(t) => t,
    };
    let key = crate::denoise_cache::key(photo, tier_for(photo, tier));
    let fits = |d: &ImageBuf| (d.width(), d.height()) == (image.width(), image.height());
    let found = cache
        .as_deref()
        .and_then(|c| crate::denoise_cache::load(c, &key))
        .filter(fits);
    let full = match (found, using) {
        (Some(d), _) => d,
        (None, DenoiseUse::Cached(_)) => {
            return Err("AI denoise has not run on this photo yet".to_string())
        }
        (None, DenoiseUse::Compute) => {
            let models = models.ok_or("no models folder for AI denoise")?;
            if !ai_mask::download::present(&ai_mask::ModelId::DrunetColor.spec(), &models) {
                return Err("the AI denoise model is not downloaded".to_string());
            }
            on_compute();
            let mut n = net(&models)?;
            let d = crate::ai_denoise::run(image, n.as_mut(), &|_, _| keep_going())?
                .ok_or("cancelled")?;
            if let Some(c) = &cache {
                let _ = crate::denoise_cache::store(c, &key, &d);
            }
            d
        }
    };
    Ok((Arc::new(Denoised::new(Arc::new(full))), id_of(&key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::ImageSource;

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let data = (0..w * h * 3)
            .map(|i| (i % 97) as f32 / 97.0)
            .collect::<Vec<_>>();
        ImageBuf::from_vec(w, h, data)
    }

    /// A frame is the full image through `downscale_to` — the prefix input's own call — and a second
    /// read of it is the held one.
    #[test]
    fn a_frame_is_the_full_image_downscaled_as_the_decode_is() {
        let full = ramp(300, 200);
        let d = Denoised::new(Arc::new(full.clone()));
        let want = full.downscale_to(150);
        let got = d.image(want.width(), want.height()).expect("a frame");
        assert_eq!(got.data(), want.data());
        assert_eq!(d.image(300, 200).expect("full").data(), full.data());
        assert_eq!(d.held.lock().unwrap().len(), 1, "held for the next read");
        assert_eq!(
            d.image(want.width(), want.height()).unwrap().data(),
            want.data()
        );
    }

    /// The full size is the result itself, never a copy: a 312 MB copy on every prefix run at 1:1 cost
    /// 110–160 ms a partial result while panning (M10.16b6, `bench_denoise_pan`).
    #[test]
    fn the_full_size_is_answered_without_a_copy() {
        let full = Arc::new(ramp(300, 200));
        let d = Denoised::new(Arc::clone(&full));
        assert!(Arc::ptr_eq(&d.image(300, 200).expect("full"), &full));
    }

    /// Review Focus 1: a size the decode's downscale would not give — another photo's frame, a
    /// region — is refused, never stretched into place.
    #[test]
    fn a_size_it_cannot_answer_exactly_is_refused() {
        let d = Denoised::new(Arc::new(ramp(300, 200)));
        assert!(d.image(150, 150).is_none());
        assert!(d.image(200, 300).is_none());
        assert!(d.image(150, 100).is_some(), "positive control");
    }

    /// A network that adds `0.1` to every value — visibly not the input.
    struct Lift;
    impl TileNet for Lift {
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: &[f32]) -> Result<Vec<f32>, String> {
            Ok(rgb.iter().map(|v| v + 0.1).collect())
        }
    }

    /// A network that adds 0.1 for its first `ok` tiles, then fails like a lost device.
    /// ⚠ The trait object needs `Send`, so its counter is an `Arc<AtomicUsize>`.
    struct FailAfter {
        ok: usize,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl TileNet for FailAfter {
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: &[f32]) -> Result<Vec<f32>, String> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.ok {
                Ok(rgb.iter().map(|v| v + 0.1).collect())
            } else {
                Err("device lost".into())
            }
        }
    }

    /// Review Focus 1: the GPU fails on its second tile — that tile and every later one run on the CPU,
    /// and the CPU network is made once.
    #[test]
    fn a_failing_gpu_hands_the_rest_to_the_cpu() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let gpu_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let made = std::sync::Arc::new(AtomicUsize::new(0));
        let m = made.clone();
        let mut e = Engine::new(
            Some(Box::new(FailAfter {
                ok: 1,
                calls: gpu_calls.clone(),
            })),
            Box::new(move || {
                m.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Lift) as Box<dyn TileNet + Send>)
            }),
        );
        for _ in 0..3 {
            let out = e.run(&[0.5; 3], 1, 1, &[1.0]).expect("a tile");
            assert!((out[0] - 0.6).abs() < 1e-6);
        }
        assert_eq!(
            gpu_calls.load(Ordering::SeqCst),
            2,
            "the GPU is not tried again"
        );
        assert_eq!(made.load(Ordering::SeqCst), 1, "the CPU network made once");
    }

    /// Review Focus 2: no primary — the fallback from the first tile.
    #[test]
    fn without_the_gpu_the_cpu_network_runs() {
        let mut e = Engine::new(
            None,
            Box::new(|| Ok(Box::new(Lift) as Box<dyn TileNet + Send>)),
        );
        assert!((e.run(&[0.5; 3], 1, 1, &[1.0]).unwrap()[0] - 0.6).abs() < 1e-6);
    }

    /// The largest |GPU − tract| on the real model, in the network's 0…1 units — measured, never tuned
    /// to pass. b2's direct `f32` kernel measured **exactly 0** (2026-10-15). Since M10.16b5 the GPU
    /// stores in `f16` and runs the 3 × 3 convolutions as Winograd F(2 × 2, 3 × 3): measured on the M2
    /// **1.10e-3 at 64² and 1.42e-3 at 296 × 200** (2026-10-16) — a third of one 8-bit level (3.9e-3).
    /// 3e-3 is twice the larger number and still under that level; never raise it without a new number.
    const TOL: f32 = 3e-3;

    /// [`TOL`] for the extremes a clamped photo can give `f16` (M10.16b5's review, measured on the M2,
    /// 2026-10-16): flat white at σ 40 1.41e-3, black 1.6e-6, a one-pixel 0/1 checkerboard at σ 1 1.66e-3
    /// and **at σ 40 3.21e-3** — 0.82 of one 8-bit level, the worst found, and none overflowed. Twice it,
    /// like `TOL`; never raise it without a new number.
    const TOL_EXTREME: f32 = 6.5e-3;

    /// DRUNet on the GPU against `tract` on the real model: a 64 × 64 tile and a 296 × 200 one (sides
    /// multiples of 8, the second not of 64), then the extremes `f16` must hold.
    #[test]
    #[ignore = "needs the model and a GPU; set RUSTROOM_DRUNET"]
    fn gpu_drunet_matches_tract() {
        let path =
            std::path::PathBuf::from(std::env::var("RUSTROOM_DRUNET").expect("RUSTROOM_DRUNET"));
        let Some(ctx) = gpu_pipeline::shared().cloned() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        let mut cpu = ai_mask::denoise::Drunet::load(&path).expect("tract");
        let mut gpu =
            gpu_pipeline::GpuDrunet::new(ctx, &gpu_weights(&path).expect("weights")).expect("gpu");
        let mut worsts = Vec::new();
        for (w, h) in [(64, 64), (296, 200)] {
            let rgb: Vec<f32> = (0..w * h * 3)
                .map(|i| ((i * 7919) % 1000) as f32 / 1000.0)
                .collect();
            let a = cpu.run(&rgb, w, h, &vec![15.0; w * h]).expect("cpu");
            let b = gpu.run(&rgb, w, h, 15.0).expect("gpu");
            // ⚠ NaN first: `f32::max` ignores it, and an all-NaN GPU output once read here as a perfect 0.
            let bad = b.iter().filter(|v| !v.is_finite()).count();
            eprintln!("{w}×{h}: tract {:?}…, GPU {:?}…", &a[..3], &b[..3]);
            assert_eq!(bad, 0, "{w}×{h}: {bad} non-finite values from the GPU");
            let worst = a
                .iter()
                .zip(&b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            let moved = a
                .iter()
                .zip(&rgb)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            eprintln!("{w}×{h}: max |gpu − tract| = {worst:e} (the network moved values by up to {moved:.3})");
            assert!(
                moved > 0.01,
                "the network changed nothing — the comparison would be empty"
            );
            worsts.push(worst);
        }
        // `f16` against DRUNet's largest activations (M10.16b5's review): the extremes a clamped photo can
        // give — flat white and black, a one-pixel checkerboard — at the lowest and highest σ.
        let mut extremes = Vec::new();
        let n = 64 * 64;
        let checker: Vec<f32> = (0..n * 3)
            .map(|i| ((i / 3 % 64 + i / 3 / 64) % 2) as f32)
            .collect();
        for (name, rgb, sigma) in [
            ("white", vec![1.0; n * 3], 40.0),
            ("black", vec![0.0; n * 3], 1.0),
            ("checker", checker.clone(), 1.0),
            ("checker", checker, 40.0),
        ] {
            let a = cpu.run(&rgb, 64, 64, &vec![sigma; n]).expect("cpu");
            // A non-finite tile is the GPU's Err, so an overflow fails here.
            let b = gpu.run(&rgb, 64, 64, sigma).expect("gpu");
            let worst = a
                .iter()
                .zip(&b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            eprintln!("{name} at σ {sigma}: max |gpu − tract| = {worst:e}");
            extremes.push(worst);
        }
        // ⚠ Every case measured before any is judged — a first failure hid the second's number.
        assert!(worsts.iter().all(|w| *w < TOL), "{worsts:?}");
        assert!(extremes.iter().all(|w| *w < TOL_EXTREME), "{extremes:?}");
    }

    /// A file of the model's pinned size — `download::present`'s whole test.
    fn present_model(models: &Path) {
        let spec = ai_mask::ModelId::DrunetColor.spec();
        std::fs::create_dir_all(models).unwrap();
        std::fs::File::create(models.join(spec.file))
            .unwrap()
            .set_len(spec.bytes)
            .unwrap();
    }

    fn no_net(_: &Path) -> Result<Box<dyn TileNet + Send>, String> {
        panic!("the network must not run")
    }

    /// The export loads the cached result — at its own tier under `Compute`, at the session's under
    /// `Cached` — and never runs the network for it.
    #[test]
    fn the_export_loads_the_cache() {
        let d = test_dir::TempDir::new("denoised-export-cache");
        let photo = d.path().join("a.RAF");
        std::fs::write(&photo, b"raw").unwrap();
        let cache = d.path().join("cache");
        let img = ramp(40, 30);
        let stored = ramp(40, 30).downscale_to(40);
        let key = crate::denoise_cache::key(&photo, Demosaic::Fast);
        crate::denoise_cache::store(&cache, &key, &stored).unwrap();
        let dirs = || Sources {
            cache: Some(cache.clone()),
            models: None,
            net: no_net,
        };
        let (got, id) = for_export(
            &photo,
            &img,
            Demosaic::Fast,
            DenoiseUse::Compute,
            dirs(),
            || panic!("no compute"),
            &|| true,
        )
        .expect("loaded");
        assert_eq!(id, id_of(&key));
        let back = crate::denoise_cache::load(&cache, &key).unwrap();
        assert_eq!(got.image(40, 30).unwrap().data(), back.data());
        let other = Demosaic::ALL
            .into_iter()
            .find(|t| *t != Demosaic::Fast)
            .unwrap();
        assert!(
            for_export(
                &photo,
                &img,
                other,
                DenoiseUse::Cached(Demosaic::Fast),
                dirs(),
                || panic!("no compute"),
                &|| true,
            )
            .is_ok(),
            "a preview reads the session's tier"
        );
        // ⚠ With the model present, so a compute would get as far as saying so — and panic.
        let models = d.path().join("models");
        present_model(&models);
        let missing = for_export(
            &photo,
            &img,
            other,
            DenoiseUse::Cached(other),
            Sources {
                cache: Some(cache.clone()),
                models: Some(models),
                net: no_net,
            },
            || panic!("no compute"),
            &|| true,
        );
        assert!(missing.is_err(), "a preview never computes");
    }

    /// Under `Compute` a missing result is computed, said first, and stored.
    #[test]
    fn the_export_computes_a_missing_result_and_stores_it() {
        let d = test_dir::TempDir::new("denoised-export-compute");
        let photo = d.path().join("a.png");
        std::fs::write(&photo, b"png").unwrap();
        let (cache, models) = (d.path().join("cache"), d.path().join("models"));
        present_model(&models);
        let img = ImageBuf::from_vec(64, 48, vec![0.2; 64 * 48 * 3]);
        let said = std::cell::Cell::new(false);
        let (got, _) = for_export(
            &photo,
            &img,
            Demosaic::Fast,
            DenoiseUse::Compute,
            Sources {
                cache: Some(cache.clone()),
                models: Some(models),
                net: |_: &Path| Ok(Box::new(Lift) as Box<dyn TileNet + Send>),
            },
            || said.set(true),
            &|| true,
        )
        .expect("computed");
        assert!(said.get(), "the status was told");
        let frame = got.image(64, 48).unwrap();
        assert!(
            (frame.data()[0] - img.data()[0]).abs() > 0.01,
            "the network's change is in the result"
        );
        let key = crate::denoise_cache::key(&photo, Demosaic::default());
        assert!(crate::denoise_cache::load(&cache, &key).is_some(), "stored");
    }

    /// The result in the maps renders at Amount 100 as the stored image, at the full size and at a rung.
    #[test]
    fn a_cached_result_renders_at_amount_100_as_the_stored_image() {
        let full = ramp(300, 200);
        let stored = Arc::new(ImageBuf::from_vec(300, 200, vec![0.4; 300 * 200 * 3]));
        let mut maps = crate::ai_coverage::AiMaps::default();
        maps.set_denoised(Some((Arc::new(Denoised::new(Arc::clone(&stored))), 7)));
        let edits = edit_model::Edits {
            ai_denoise: edit_model::AiDenoise { amount: 100.0 },
            ..Default::default()
        };
        let src = crate::ai_coverage::AiSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (300, 200),
        };
        let steps = pipeline::build_prefix_steps(
            &edits,
            pipeline::RenderScale::FULL,
            None,
            pipeline::Resampler::Bilinear,
            None,
            Some(&src),
        );
        assert_eq!(steps.len(), 1, "the denoise step alone");
        let out = steps[0].stage.apply(full.clone());
        assert_eq!(out.data(), stored.data());
        let rung = full.downscale_to(150);
        let out = steps[0].stage.apply(rung);
        assert_eq!(out.data(), stored.downscale_to(150).data());
    }
}

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
    fn image(&self, w: usize, h: usize) -> Option<ImageBuf> {
        if (w, h) == self.size() {
            return Some((*self.full).clone());
        }
        let lock = || self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = lock().iter().find(|i| (i.width(), i.height()) == (w, h)) {
            return Some((**hit).clone());
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
        Some((*d).clone())
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

impl Sources<fn(&Path) -> Result<Box<dyn TileNet + Send>, String>> {
    /// The real folders and DRUNet.
    pub fn user_default() -> Self {
        Self {
            cache: crate::denoise_cache::dir(),
            models: crate::ai_cache::models_dir(),
            net: drunet,
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
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: f32) -> Result<Vec<f32>, String> {
            Ok(rgb.iter().map(|v| v + 0.1).collect())
        }
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

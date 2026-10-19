//! The Remove brush's jobs (M10.16c1), outside any thread or window: a photo's key, which models a
//! removal needs, what to do next for one, the real networks, and the export's fills.

use crate::denoise_tool::Next;
use crate::remove_tool;
use core_image::ImageBuf;
use edit_model::{Edits, Removal};
use pipeline::Fill;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The network the fills are made with — the author's pick from the spike's sheets (M10.16c1).
pub const MODEL: ai_mask::ModelId = ai_mask::ModelId::Lama;

/// The photo's part of a fill's key: the FILE (path, size, mtime) — a fill is of these pixels.
///
/// ⚠ Not the demosaic quality: a fill is a 512 square, far coarser than the tiers' differences. Not the
/// film negative's settings either — a fill is stored in the negative's own units (`remove_tool::fill`).
pub fn photo_key(photo: &Path) -> String {
    let md = std::fs::metadata(photo).ok();
    let size = md.as_ref().map_or(0, |m| m.len());
    let mtime = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    ai_mask::digest_hex(format!("{}|{size}|{mtime}", photo.display()).as_bytes())
}

/// The models `edits`' removals need that `models` lacks: the inpainting network for any removal, and
/// MobileSAM's two files once one has Detect objects.
pub fn missing_models(edits: &Edits, models: &Path) -> Vec<ai_mask::ModelId> {
    let mut want = Vec::new();
    if !edits.removals.is_empty() {
        want.push(MODEL);
    }
    if edits.removals.iter().any(|r| r.detect) {
        want.extend(ai_mask::registry::CLICK_MODELS);
    }
    want.into_iter()
        .filter(|m| !ai_mask::download::present(&m.spec(), models))
        .collect()
}

/// One removal's facts this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// Its fill is in the maps (or held for them).
    pub has_fill: bool,
    pub cached: bool,
    /// Every model it needs is downloaded.
    pub models_present: bool,
    /// The session's decode is the full-resolution one and not a ghost.
    pub full_decode: bool,
    pub running: bool,
    pub failed: bool,
}

/// What to do for one removal. ⚠ A cached fill needs neither the model nor the full decode; computing one
/// needs both — never from a ghost session or the half decode, whose pixels it would be made of.
pub fn next(f: &Facts) -> Next {
    if f.has_fill || f.running || f.failed {
        Next::Nothing
    } else if f.cached {
        Next::Load
    } else if !f.models_present {
        Next::NeedModel
    } else if !f.full_decode {
        Next::WaitForDecode
    } else {
        Next::Run
    }
}

/// The inpainting network, boxed as `remove_tool::fill` takes it.
pub type Net = Box<dyn FnMut(&[f32], &[f32]) -> Result<Vec<f32>, String> + Send>;
/// The object finder, boxed as `remove_tool::fill` takes it.
pub type Detector =
    Box<dyn FnMut(&[f32], &[[f32; 2]], [f32; 4]) -> Result<Vec<f32>, String> + Send>;

/// LaMa from `models`, loaded once (~7 s).
pub fn lama(models: &Path) -> Result<Net, String> {
    let mut net = ai_mask::inpaint::Inpaint::load(&models.join(MODEL.spec().file))
        .map_err(|e| e.to_string())?;
    Ok(Box::new(move |rgb, mask| {
        net.run(rgb, mask).map_err(|e| e.to_string())
    }))
}

/// MobileSAM from `models`: the window embedded, then prompted by the stroke's points and box — its answer
/// at `remove_tool::SIDE`².
pub fn sam(models: &Path) -> Result<Detector, String> {
    let [enc, dec] = ai_mask::registry::CLICK_MODELS.map(|m| models.join(m.spec().file));
    for f in [&enc, &dec] {
        if !f.is_file() {
            return Err(format!("{} is not downloaded", f.display()));
        }
    }
    let mut sam = ai_mask::Sam::new(&enc, &dec);
    let side = remove_tool::SIDE;
    Ok(Box::new(move |rgb, points, bbox| {
        let img = image::RgbImage::from_fn(side as u32, side as u32, |x, y| {
            let i = (y as usize * side + x as usize) * 3;
            image::Rgb([0, 1, 2].map(|c| (rgb[i + c].clamp(0.0, 1.0) * 255.0).round() as u8))
        });
        let e = sam.embed(&img).map_err(|e| e.to_string())?;
        let m = sam
            .prompt(&e, points, Some(bbox))
            .map_err(|e| e.to_string())?;
        Ok((0..side * side)
            .map(|i| {
                m.at(
                    ((i % side) as f32 + 0.5) / side as f32,
                    ((i / side) as f32 + 0.5) / side as f32,
                )
            })
            .collect())
    }))
}

/// Makes a network from the models folder.
pub type Make<T> = Box<dyn FnOnce(&Path) -> Result<T, String>>;

/// Where an export's fills come from.
pub struct Sources {
    pub cache: Option<PathBuf>,
    pub models: Option<PathBuf>,
    /// Makes the networks, called at most once each and only for a fill missing from the cache.
    pub net: Make<Net>,
    pub detect: Make<Detector>,
}

/// The user's: the real cache and models, LaMa and MobileSAM.
pub fn user_sources() -> Sources {
    Sources {
        cache: crate::removal_cache::dir(),
        models: crate::ai_cache::models_dir(),
        net: Box::new(lama),
        detect: Box::new(sam),
    }
}

/// Every removal's fill for an export of `photo` (`image` its decode, full resolution), each with its
/// removal — the cache, else, with `compute`, the network there and then (Review Focus 5: an export never
/// shows the object a removal took away), stored once computed. Without `compute` (a rendered preview,
/// which never runs a network) a missing fill is an error: the build runs later.
pub fn for_export(
    photo: &Path,
    image: &ImageBuf,
    edits: &Edits,
    compute: bool,
    sources: Sources,
) -> Result<Vec<(Removal, Arc<Fill>)>, String> {
    let Sources {
        cache,
        models,
        net,
        detect,
    } = sources;
    let (mut net_make, mut detect_make) = (Some(net), Some(detect));
    let (mut net, mut detector): (Option<Net>, Option<Detector>) = (None, None);
    let key = photo_key(photo);
    let mut out = Vec::new();
    for r in &edits.removals {
        let id = remove_tool::id(&key, r, MODEL);
        if let Some(f) = cache
            .as_deref()
            .and_then(|c| crate::removal_cache::load(c, &id))
        {
            out.push((r.clone(), Arc::new(f)));
            continue;
        }
        if !compute {
            return Err("a removal has not been filled on this photo yet".to_string());
        }
        let models = models.as_deref().ok_or("no models folder for Remove")?;
        if !ai_mask::download::present(&MODEL.spec(), models) {
            return Err("the Remove model is not downloaded".to_string());
        }
        if net.is_none() {
            net = Some((net_make.take().ok_or("the Remove model did not load")?)(
                models,
            )?);
        }
        if r.detect && detector.is_none() {
            if let Some(make) = detect_make.take() {
                detector = Some(make(models)?);
            }
        }
        let n = net.as_mut().expect("made above");
        let d = if r.detect {
            detector
                .as_mut()
                .map(|d| d.as_mut() as remove_tool::Detect<'_>)
        } else {
            None
        };
        let f = remove_tool::fill(image, r, &edits.negative, n.as_mut(), d)?;
        if let Some(c) = &cache {
            let _ = crate::removal_cache::store(c, &id, &f);
        }
        out.push((r.clone(), Arc::new(f)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removal(x: f32) -> Removal {
        Removal {
            points: vec![[x, 0.5]],
            radius: 0.02,
            detect: false,
            variant: 0,
            opacity: 100.0,
        }
    }

    fn photo(d: &test_dir::TempDir) -> (PathBuf, ImageBuf) {
        let p = d.join("photo.jpg");
        std::fs::write(&p, b"stand-in").unwrap();
        let img = ImageBuf::from_vec(
            900,
            600,
            (0..900 * 600 * 3).map(|i| (i % 89) as f32 / 89.0).collect(),
        );
        (p, img)
    }

    /// The test's network: each value inverted, its calls counted.
    fn sources(cache: &Path, models: &Path, calls: Arc<std::sync::atomic::AtomicUsize>) -> Sources {
        Sources {
            cache: Some(cache.to_path_buf()),
            models: Some(models.to_path_buf()),
            net: Box::new(move |_| {
                Ok(Box::new(move |rgb: &[f32], _: &[f32]| {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(rgb.iter().map(|v| 1.0 - v).collect())
                }) as Net)
            }),
            detect: Box::new(|_| Err("no detector in this test".to_string())),
        }
    }

    /// A cached fill is used as it is: no model, no network.
    #[test]
    fn a_cached_fill_is_loaded_without_the_network() {
        let d = test_dir::TempDir::new("removal-export-cached");
        let (p, img) = photo(&d);
        let r = removal(0.4);
        let f = Fill {
            rect: [0.1, 0.1, 0.5, 0.5],
            w: 2,
            h: 2,
            rgb: vec![0.25; 12],
            alpha: vec![1.0; 4],
        };
        let id = remove_tool::id(&photo_key(&p), &r, MODEL);
        crate::removal_cache::store(&d.join("cache"), &id, &f).unwrap();
        let e = Edits {
            removals: vec![r.clone()],
            ..Edits::default()
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let got = for_export(
            &p,
            &img,
            &e,
            false,
            sources(&d.join("cache"), &d.join("none"), calls.clone()),
        )
        .expect("the cached fill");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, r);
        assert_eq!(got[0].1.rect, f.rect);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    /// Review Focus 5: a fill never computed is computed by the export there and then, and stored — never
    /// exported with the object; a rendered preview (no `compute`) refuses rather than shows it.
    #[test]
    fn an_export_computes_a_missing_fill() {
        let d = test_dir::TempDir::new("removal-export-missing");
        let (p, img) = photo(&d);
        let models = d.join("models");
        let e = Edits {
            removals: vec![removal(0.3), removal(0.7)],
            ..Edits::default()
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cache = d.join("cache");
        assert!(
            for_export(&p, &img, &e, false, sources(&cache, &models, calls.clone())).is_err(),
            "a rendered preview never runs the network"
        );
        assert!(
            for_export(&p, &img, &e, true, sources(&cache, &models, calls.clone())).is_err(),
            "no model, no fill — and no export with the object"
        );
        // `present` reads the size alone: a sparse file of LaMa's stands in for it.
        std::fs::create_dir_all(&models).unwrap();
        std::fs::File::create(models.join(MODEL.spec().file))
            .unwrap()
            .set_len(MODEL.spec().bytes)
            .unwrap();
        let got = for_export(&p, &img, &e, true, sources(&cache, &models, calls.clone()))
            .expect("computed");
        assert_eq!(got.len(), 2, "both removals filled");
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        for r in &e.removals {
            assert!(
                crate::removal_cache::exists(&cache, &remove_tool::id(&photo_key(&p), r, MODEL)),
                "stored once computed"
            );
        }
    }

    #[test]
    fn the_models_a_removal_needs() {
        let d = test_dir::TempDir::new("removal-models");
        let plain = Edits {
            removals: vec![removal(0.5)],
            ..Edits::default()
        };
        assert_eq!(missing_models(&plain, d.path()), vec![MODEL]);
        let detect = Edits {
            removals: vec![Removal {
                detect: true,
                ..removal(0.5)
            }],
            ..Edits::default()
        };
        assert_eq!(
            missing_models(&detect, d.path()),
            vec![
                MODEL,
                ai_mask::ModelId::SamEncoder,
                ai_mask::ModelId::SamDecoder
            ]
        );
        assert!(missing_models(&Edits::default(), d.path()).is_empty());
    }

    /// Never from a ghost session or the half decode; a cached fill loads whatever the decode.
    #[test]
    fn no_job_from_a_ghost_session() {
        let ghost = Facts {
            models_present: true,
            full_decode: false,
            ..Facts::default()
        };
        assert_eq!(next(&ghost), Next::WaitForDecode);
        assert_eq!(
            next(&Facts {
                cached: true,
                ..ghost
            }),
            Next::Load
        );
        assert_eq!(
            next(&Facts {
                full_decode: true,
                ..ghost
            }),
            Next::Run
        );
        assert_eq!(
            next(&Facts {
                models_present: false,
                ..ghost
            }),
            Next::NeedModel
        );
        for done in [
            Facts {
                has_fill: true,
                ..ghost
            },
            Facts {
                running: true,
                ..ghost
            },
            Facts {
                failed: true,
                ..ghost
            },
        ] {
            assert_eq!(
                next(&Facts {
                    full_decode: true,
                    ..done
                }),
                Next::Nothing
            );
        }
    }
}

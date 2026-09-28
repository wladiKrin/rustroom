//! An AI map as coverage: the threshold and the invert applied, resampled into a frame (M9.13).

use crate::ai_space::DecodeMap;
use ai_mask::ProbMap;
use edit_model::AiMask;
use pipeline::brush::CoverageMap;
use rayon::prelude::*;

/// The probability maps a photo's AI masks have, by prompt — what the render reads.
///
/// ⚠ Keyed by the PROMPT, not the cache key: the render has the edits, not the file, and a prompt
/// is exactly what a mask names. The session fills it from the worker and the disk cache (with the
/// Settings' subject model), and asks for a render; a prompt with no entry renders as nothing.
#[derive(Debug, Default, Clone)]
pub struct AiMaps {
    entries: Vec<(edit_model::AiPrompt, std::sync::Arc<ProbMap>)>,
    /// Bumped on every change — what the canvas tint keys on, so a map arriving redraws it.
    generation: u64,
}

impl AiMaps {
    pub fn get(&self, prompt: &edit_model::AiPrompt) -> Option<&std::sync::Arc<ProbMap>> {
        self.entries
            .iter()
            .find(|(p, _)| p == prompt)
            .map(|(_, m)| m)
    }

    pub fn insert(&mut self, prompt: edit_model::AiPrompt, map: std::sync::Arc<ProbMap>) {
        self.entries.retain(|(p, _)| *p != prompt);
        self.entries.push((prompt, map));
        self.generation += 1;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Every prompt `edits` asks, in mask then component order.
pub fn prompts(edits: &edit_model::Edits) -> Vec<edit_model::AiPrompt> {
    let mut out: Vec<edit_model::AiPrompt> = Vec::new();
    for m in &edits.masks {
        for mask in std::iter::once(&m.mask).chain(m.refine.iter().map(|r| &r.mask)) {
            if let edit_model::Mask::Ai(a) = mask {
                if !out.contains(&a.prompt) {
                    out.push(a.prompt);
                }
            }
        }
    }
    out
}

/// The model that answers `prompt` — the Settings' subject model, or MobileSAM for a click.
pub fn model_for(
    prompt: &edit_model::AiPrompt,
    subject: ai_mask::SubjectModel,
) -> ai_mask::ModelId {
    match prompt {
        edit_model::AiPrompt::Subject => subject.model(),
        edit_model::AiPrompt::Click { .. } => ai_mask::ModelId::SamDecoder,
    }
}

/// The maps `edits` needs for `photo`, from the disk cache — what an export renders with, and what
/// the session loads when a photo opens. A prompt with no cached map is simply absent.
pub fn maps_for(
    photo: &std::path::Path,
    edits: &edit_model::Edits,
    subject: ai_mask::SubjectModel,
    cache: &std::path::Path,
) -> AiMaps {
    let mut maps = AiMaps::default();
    for p in prompts(edits) {
        let key = crate::ai_cache::key(photo, model_for(&p, subject), &p);
        if let Some(m) = crate::ai_cache::load(cache, &key) {
            maps.insert(p, std::sync::Arc::new(m));
        }
    }
    maps
}

/// The store the session writes and the render worker reads.
pub type SharedAiMaps = std::sync::Arc<std::sync::RwLock<AiMaps>>;

/// How far either side of the threshold the soft edge reaches, on the probability scale.
const SOFTNESS: f32 = 0.15;

/// A probability as coverage. `threshold` is the **Refine** slider, −100..=100: positive moves
/// the threshold down (the mask grows), negative moves it up (it shrinks); 0 is the network's own
/// 0.5.
pub fn refine(p: f32, threshold: f32, invert: bool) -> f32 {
    let t = 0.5 - threshold.clamp(-100.0, 100.0) / 250.0;
    let x = ((p - (t - SOFTNESS)) / (2.0 * SOFTNESS)).clamp(0.0, 1.0);
    let c = x * x * (3.0 - 2.0 * x);
    if invert {
        1.0 - c
    } else {
        c
    }
}

/// `map` resampled over the frame (or the `clip` rectangle of it — `(x0, y0, w, h)` in frame
/// pixels, a region render's), as the 8-bit [`CoverageMap`] the pipeline reads for a brush.
pub fn to_frame(
    map: &ProbMap,
    mask: &AiMask,
    dm: &DecodeMap,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap {
    let (fw, fh) = dm.frame_size();
    let (x0, y0, w, h) = clip.unwrap_or((0, 0, fw as usize, fh as usize));
    let data: Vec<u8> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let [u, v] = dm.frame_to_decode((x0 + x) as f32 + 0.5, (y0 + y) as f32 + 0.5);
                let c = refine(map.at(u, v), mask.threshold, mask.invert);
                (c * 255.0).round() as u8
            })
        })
        .collect();
    CoverageMap {
        x0,
        y0,
        w,
        h,
        data,
        // ⚠ Outside the rectangle only a region render could ask, and the rectangle is its region;
        // the value still follows the invert, `CoverageMap::outside`'s rule for a brush.
        outside: if mask.invert { 255 } else { 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Geometry, Perspective};

    #[test]
    fn refine_grows_and_shrinks_the_mask_and_inverts() {
        assert!(
            (refine(0.5, 0.0, false) - 0.5).abs() < 1e-6,
            "0 keeps the network's 0.5"
        );
        assert_eq!(refine(0.0, 0.0, false), 0.0);
        assert_eq!(refine(1.0, 0.0, false), 1.0);
        assert!(
            refine(0.4, 100.0, false) > refine(0.4, 0.0, false),
            "+ grows"
        );
        assert!(
            refine(0.6, -100.0, false) < refine(0.6, 0.0, false),
            "− shrinks"
        );
        for p in [0.1, 0.45, 0.8] {
            assert!((refine(p, 20.0, true) - (1.0 - refine(p, 20.0, false))).abs() < 1e-6);
        }
        // Monotone in p.
        let mut last = 0.0;
        for i in 0..=100 {
            let c = refine(i as f32 / 100.0, -30.0, false);
            assert!(c >= last);
            last = c;
        }
    }

    #[test]
    fn a_map_lands_in_the_frame_where_it_belongs() {
        // The left half of the decode is the subject.
        let map = ProbMap::from_fn(40, 20, |x, _| if x < 20 { 1.0 } else { 0.0 });
        let dm = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            80,
            40,
            None,
            80,
            40,
        );
        let cov = to_frame(&map, &AiMask::default(), &dm, None);
        assert_eq!((cov.w, cov.h), (80, 40));
        assert_eq!(cov.at(10, 20), 255);
        assert_eq!(cov.at(70, 20), 0);
        let inv = to_frame(
            &map,
            &AiMask {
                invert: true,
                ..AiMask::default()
            },
            &dm,
            Some((60, 0, 20, 40)),
        );
        assert_eq!((inv.x0, inv.w), (60, 20));
        assert_eq!(inv.at(70, 20), 255, "inverted: the right half is covered");
        assert_eq!(inv.at(10, 20), 255, "outside the clip follows the invert");
    }
}

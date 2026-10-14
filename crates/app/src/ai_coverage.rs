//! An AI or depth map as coverage: the threshold or band and the invert applied, resampled into a
//! frame (M9.13, M10.16a).

use crate::ai_space::DecodeMap;
use ai_mask::ProbMap;
use edit_model::AiMask;
use pipeline::brush::CoverageMap;
use rayon::prelude::*;

/// What a map answers (M10.16a): an AI prompt — the subject, a click — or the photo's depth.
///
/// ⚠ The map store's key, widened from `AiPrompt` when the depth map arrived. Not an `AiPrompt`
/// variant: a depth range is its own `Mask` kind, and an `AiMask` asking for depth with a Refine
/// threshold would be a combination nothing could mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapQuery {
    Prompt(edit_model::AiPrompt),
    Depth,
}

/// The depth map's query string in its cache key — its RECIPE's version (`depth_map::RECIPE`).
/// ⚠ Bump it when the recipe changes, or every cached map keeps the old look.
pub const DEPTH_QUERY: &str = "depth:v1";

impl MapQuery {
    /// The map a component reads, or `None` for one that reads none.
    pub fn of(mask: &edit_model::Mask) -> Option<MapQuery> {
        match mask {
            edit_model::Mask::Ai(a) => Some(MapQuery::Prompt(a.prompt)),
            edit_model::Mask::Depth(_) => Some(MapQuery::Depth),
            _ => None,
        }
    }

    /// What the cache key says for this query. ⚠⚠ The prompt strings are byte-identical to the ones
    /// keyed before M10.16a — `ai_cache::the_subject_and_click_keys_are_pinned` — so no map on disk is
    /// orphaned.
    pub fn cache_str(&self) -> String {
        match self {
            MapQuery::Prompt(edit_model::AiPrompt::Subject) => "subject".to_string(),
            MapQuery::Prompt(edit_model::AiPrompt::Click { point }) => {
                format!("click:{:.5}:{:.5}", point[0], point[1])
            }
            MapQuery::Depth => DEPTH_QUERY.to_string(),
        }
    }
}

/// The maps a photo's AI and depth masks have, by query — what the render reads.
///
/// ⚠ Keyed by the QUERY, not the cache key: the render has the edits, not the file, and a query is
/// exactly what a mask names. The session fills it from the worker and the disk cache (with the
/// Settings' subject model), and asks for a render; a query with no entry renders as nothing.
#[derive(Debug, Default, Clone)]
pub struct AiMaps {
    /// Each map with its content id ([`content_id`]), computed once as it arrives.
    entries: Vec<(MapQuery, std::sync::Arc<ProbMap>, u64)>,
    /// Bumped on every change — what the canvas tint keys on, so a map arriving redraws it.
    generation: u64,
}

/// A map's identity by CONTENT — the same pixels are the same map wherever they came from. What the
/// prefix's cached checkpoints key a map on (M10.16a): an address could be reused by the next map.
fn content_id(map: &ProbMap) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (map.w, map.h, &map.data).hash(&mut h);
    h.finish().max(1)
}

impl AiMaps {
    pub fn get(&self, query: &MapQuery) -> Option<&std::sync::Arc<ProbMap>> {
        self.entries
            .iter()
            .find(|(q, _, _)| q == query)
            .map(|(_, m, _)| m)
    }

    /// The content id of `query`'s map, or `None` when it has none.
    pub fn id(&self, query: &MapQuery) -> Option<u64> {
        self.entries
            .iter()
            .find(|(q, _, _)| q == query)
            .map(|(_, _, id)| *id)
    }

    pub fn insert(&mut self, query: MapQuery, map: std::sync::Arc<ProbMap>) {
        let id = content_id(&map);
        self.entries.retain(|(q, _, _)| *q != query);
        self.entries.push((query, map, id));
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

/// Every map `edits` reads, in mask then component order, each once.
pub fn queries(edits: &edit_model::Edits) -> Vec<MapQuery> {
    let mut out: Vec<MapQuery> = Vec::new();
    for m in &edits.masks {
        for mask in std::iter::once(&m.mask).chain(m.refine.iter().map(|r| &r.mask)) {
            if let Some(q) = MapQuery::of(mask) {
                if !out.contains(&q) {
                    out.push(q);
                }
            }
        }
    }
    out
}

/// The model that answers `query` — the Settings' subject model, MobileSAM for a click, Depth
/// Anything for the depth.
pub fn model_for(query: &MapQuery, subject: ai_mask::SubjectModel) -> ai_mask::ModelId {
    match query {
        MapQuery::Prompt(edit_model::AiPrompt::Subject) => subject.model(),
        MapQuery::Prompt(edit_model::AiPrompt::Click { .. }) => ai_mask::ModelId::SamDecoder,
        MapQuery::Depth => ai_mask::ModelId::DepthAnythingV2Small,
    }
}

/// The maps `edits` needs for `photo`, from the disk cache — what an export renders with, and what
/// the session loads when a photo opens. A query with no cached map is simply absent.
pub fn maps_for(
    photo: &std::path::Path,
    edits: &edit_model::Edits,
    subject: ai_mask::SubjectModel,
    cache: &std::path::Path,
) -> AiMaps {
    let mut maps = AiMaps::default();
    for q in queries(edits) {
        let key = crate::ai_cache::key(photo, model_for(&q, subject), &q, &edits.negative);
        if let Some(m) = crate::ai_cache::load(cache, &key) {
            maps.insert(q, std::sync::Arc::new(m));
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

/// `map` (decode space) warped into **post-lens** space, memoised per `(map, lens_key)`.
///
/// ⚠⚠ **Why this exists: an AI mask made geometry drags and a panned tint sluggish** (the author's
/// report, 2026-09-28). Building the lens corrector costs **43 ms** and every frame pixel then paid a
/// lens lookup: **117 ms** a fitted rotation frame and **69 ms** a tint miss on an X-T4 RAF
/// (`examples/ai_coverage_bench.rs`). Warping the map ONCE — at the map's own ~0.7 Mpx — leaves each
/// frame only the geometry's inverse, with no corrector at all. `make_corrector` runs on a miss only.
pub fn post_lens(
    map: &std::sync::Arc<ProbMap>,
    lens_key: u64,
    decode: (usize, usize),
    make_corrector: impl FnOnce() -> Option<lens_profile::Corrector>,
) -> std::sync::Arc<ProbMap> {
    type Memo = Vec<(usize, u64, std::sync::Arc<ProbMap>)>;
    static MEMO: std::sync::Mutex<Memo> = std::sync::Mutex::new(Vec::new());
    if lens_key == 0 {
        return std::sync::Arc::clone(map);
    }
    let id = std::sync::Arc::as_ptr(map) as usize;
    if let Some((_, _, m)) = MEMO
        .lock()
        .expect("post-lens memo")
        .iter()
        .find(|(i, k, _)| *i == id && *k == lens_key)
    {
        return std::sync::Arc::clone(m);
    }
    let Some(corr) = make_corrector() else {
        return std::sync::Arc::clone(map);
    };
    let (dw, dh) = (decode.0.max(1) as f32, decode.1.max(1) as f32);
    let warped = std::sync::Arc::new({
        let (w, h) = (map.w, map.h);
        let rows: Vec<Vec<u8>> = (0..h)
            .into_par_iter()
            .map(|y| {
                (0..w)
                    .map(|x| {
                        // Post-lens pixel centre → the decode point it came from (green channel).
                        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                        let src = corr.source(u * dw, v * dh)[1];
                        (map.at(src[0] / dw, src[1] / dh) * 255.0).round() as u8
                    })
                    .collect()
            })
            .collect();
        ProbMap {
            w,
            h,
            data: rows.concat(),
        }
    });
    let mut memo = MEMO.lock().expect("post-lens memo");
    memo.insert(0, (id, lens_key, std::sync::Arc::clone(&warped)));
    // A handful: a few photos' masks at one lens setting each. A map is ≤1 MB.
    memo.truncate(8);
    warped
}

/// The finest grid, in pixels on its long side, at which coverage is sampled before interpolation.
///
/// ⚠ The maps themselves are at most 1024 px long, so sampling a 6,240 px frame at every pixel buys
/// nothing but time — **123 ms** a full-resolution frame against a few ms on this grid.
const GRID_LONG: usize = 1024;

/// `map` resampled over the frame (or the `clip` rectangle of it — `(x0, y0, w, h)` in frame
/// pixels, a region render's), as the 8-bit [`CoverageMap`] the pipeline reads for a brush.
///
/// ⚠ `map` must already be in the space `dm` maps into: post-lens ([`post_lens`]) when `dm` was
/// built without a corrector.
pub fn to_frame(
    map: &ProbMap,
    mask: &AiMask,
    dm: &DecodeMap,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap {
    let (fw, fh) = dm.frame_size();
    let (x0, y0, w, h) = clip.unwrap_or((0, 0, fw as usize, fh as usize));
    // Sample on a coarse grid (stride `s`), then interpolate — see `GRID_LONG`.
    let s = w.max(h).div_ceil(GRID_LONG).max(1);
    let (gw, gh) = (w.div_ceil(s) + 1, h.div_ceil(s) + 1);
    let grid: Vec<f32> = (0..gh)
        .into_par_iter()
        .flat_map_iter(|gy| {
            (0..gw).map(move |gx| {
                let (x, y) = (
                    (gx * s).min(w.saturating_sub(1)),
                    (gy * s).min(h.saturating_sub(1)),
                );
                let [u, v] = dm.frame_to_decode((x0 + x) as f32 + 0.5, (y0 + y) as f32 + 0.5);
                refine(map.at(u, v), mask.threshold, mask.invert)
            })
        })
        .collect();
    let data: Vec<u8> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let grid = &grid;
            let (gy, fy) = (y / s, (y % s) as f32 / s as f32);
            let gy1 = (gy + 1).min(gh - 1);
            (0..w).map(move |x| {
                let (gx, fx) = (x / s, (x % s) as f32 / s as f32);
                let gx1 = (gx + 1).min(gw - 1);
                let g = |i: usize, j: usize| grid[j * gw + i];
                let top = g(gx, gy) * (1.0 - fx) + g(gx1, gy) * fx;
                let bot = g(gx, gy1) * (1.0 - fx) + g(gx1, gy1) * fx;
                ((top * (1.0 - fy) + bot * fy) * 255.0).round() as u8
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

    /// M10.16a: every map the edits read, once, in mask then component order — a depth range's too.
    #[test]
    fn queries_list_each_map_once_in_mask_order() {
        use edit_model::{AiPrompt, Mask, MaskOp, MaskedAdjustment, Refinement};
        let refine = |mask| Refinement {
            op: MaskOp::Add,
            mask,
        };
        let click = AiPrompt::Click { point: [0.3, 0.6] };
        let edits = edit_model::Edits {
            masks: vec![
                MaskedAdjustment {
                    mask: Mask::Depth(edit_model::DepthRange::default()),
                    ..Default::default()
                },
                MaskedAdjustment {
                    mask: Mask::Ai(AiMask::default()),
                    refine: vec![refine(Mask::Depth(edit_model::DepthRange::default()))],
                    ..Default::default()
                },
                MaskedAdjustment {
                    mask: Mask::Linear(Default::default()),
                    refine: vec![refine(Mask::Ai(AiMask {
                        prompt: click,
                        ..Default::default()
                    }))],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            queries(&edits),
            vec![
                MapQuery::Depth,
                MapQuery::Prompt(AiPrompt::Subject),
                MapQuery::Prompt(click)
            ]
        );
        assert_eq!(MapQuery::of(&Mask::Linear(Default::default())), None);
        assert_eq!(
            model_for(&MapQuery::Depth, ai_mask::SubjectModel::IsNet),
            ai_mask::ModelId::DepthAnythingV2Small
        );
    }

    /// A map's id follows its CONTENT — the prefix key's identity for it (M10.16a): the same pixels
    /// arriving again are the same map, any other pixels another.
    #[test]
    fn a_maps_id_follows_its_content() {
        let a = std::sync::Arc::new(ProbMap::from_fn(4, 2, |x, _| x as f32 / 3.0));
        let mut maps = AiMaps::default();
        maps.insert(MapQuery::Depth, std::sync::Arc::clone(&a));
        let first = maps.id(&MapQuery::Depth).expect("an id");
        maps.insert(MapQuery::Depth, std::sync::Arc::new((*a).clone()));
        assert_eq!(
            maps.id(&MapQuery::Depth),
            Some(first),
            "the same pixels, the same id"
        );
        maps.insert(
            MapQuery::Depth,
            std::sync::Arc::new(ProbMap::from_fn(4, 2, |x, _| 1.0 - x as f32 / 3.0)),
        );
        assert_ne!(maps.id(&MapQuery::Depth), Some(first), "other pixels");
        assert_eq!(
            maps.id(&MapQuery::Prompt(edit_model::AiPrompt::Subject)),
            None
        );
    }

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

    /// ⚠ The grid is a speed trade, so its price is pinned: on a smooth map a large frame, sampled on
    /// the grid, stays within a couple of levels of the exact per-pixel answer.
    #[test]
    fn grid_sampling_stays_within_two_levels_of_per_pixel_sampling() {
        let map = ProbMap::from_fn(256, 128, |x, y| {
            0.5 + 0.5 * ((x as f32 / 40.0).sin() * (y as f32 / 30.0).cos())
        });
        let (fw, fh) = (3000, 1500);
        let dm = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            fw,
            fh,
            None,
            fw,
            fh,
        );
        let a = AiMask {
            threshold: 100.0, // the softest ramp, so the comparison sees the interpolation
            ..AiMask::default()
        };
        let cov = to_frame(&map, &a, &dm, None);
        let mut worst = 0i32;
        for (x, y) in [(0, 0), (1234, 567), (2999, 1499), (1501, 3), (7, 1400)] {
            let [u, v] = dm.frame_to_decode(x as f32 + 0.5, y as f32 + 0.5);
            let exact = (refine(map.at(u, v), a.threshold, a.invert) * 255.0).round() as i32;
            worst = worst.max((cov.at(x, y) as i32 - exact).abs());
        }
        assert!(worst <= 2, "grid sampling is {worst} levels off");
    }

    #[test]
    fn the_lens_warp_is_skipped_without_a_lens_and_memoised_with_one() {
        let map = std::sync::Arc::new(ProbMap::from_fn(8, 4, |x, _| x as f32 / 7.0));
        let same = post_lens(&map, 0, (8, 4), || panic!("no lens, no corrector"));
        assert!(std::sync::Arc::ptr_eq(&same, &map));
        // A key with no corrector available falls back to the map itself, uncached.
        let fallback = post_lens(&map, 42, (8, 4), || None);
        assert!(std::sync::Arc::ptr_eq(&fallback, &map));
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

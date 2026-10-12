//! Brush and AI mask coverage, cached per mask and per component so a slider drag never rebuilds a map.

use edit_model::Edits;
use std::sync::Arc;

/// Where a coverage map is being built: the frame it spans, the slice of it wanted, and the rung.
///
/// ⚠ A params struct because `key`, `remember` and `get_or_build` all pass the same four values,
/// and `remember` reached clippy's argument limit. That is a fair complaint and this project's
/// standing answer to it is a struct, not an allowance — `FrameSpec`, `NameField`, `AspectLock`
/// and `GradeUi` are the precedents.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct CoverageSpec {
    pub(super) fw: usize,
    pub(super) fh: usize,
    pub(super) region: Option<(usize, usize, usize, usize)>,
    pub(super) max_dim: usize,
}

/// What one mask's cached coverage was built from.
#[derive(PartialEq)]
pub(super) struct MaskCoverageKey {
    /// An AI component's mask and the identity of the map it was built from (M9.13) — a new map
    /// arriving for the same prompt is a new key.
    ai: Option<(edit_model::AiMask, usize)>,
    /// This component's brush. `None` for an analytic shape, which is never cached here.
    brush: Option<edit_model::Brush>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    /// The rendered region as `(x0, y0, w, h)` in frame pixels, or `None` for the whole frame.
    ///
    /// ⚠⚠ **In the key on purpose, and this is the OPPOSITE of [`PrefixCache`]'s rule.** That rule
    /// — region out of the key, so panning never invalidates — is right for the prefix, which runs
    /// whole-buffer and whose output *is* the frame. Copied here it costs a full-frame
    /// rasterization on every paint frame: measured at 21.62 ms against 12.32 clipped for a plain
    /// brush, and **80.81 against 42.55** for a gating one.
    ///
    /// ⚠⚠ And for a **gating** brush a whole-frame map cannot be built during a region render at
    /// all: the gate reads base pixels the region render never computes. Not merely uncached —
    /// unknowable. **Do not "unify" the two caches.**
    ///
    /// ⚠ It also makes a clip mismatch unrepresentable: a map clipped to one region handed to a
    /// `MaskStage` rendering another would sample `outside` across the difference — a mask that
    /// fades out partway across the image rather than an error.
    region: Option<(usize, usize, usize, usize)>,
    /// What produced the pixels this mask's gate compared against, or `None` for a brush that does
    /// not gate.
    ///
    /// ⚠⚠ **`Edits` reduced to what reaches this mask's base, plus the rung** ([`BrushCache::base_id`]).
    /// Since M10.12b that is one of two things. A mask that `pipeline::reads_reference` selects on
    /// the reference photo — the global edits and NO mask. Any other: every earlier mask whole, and
    /// every later one through its **scene offsets** alone, which `SceneOffsetsStage` applies at the
    /// head of the tail — before this mask's stage — while their look sliders run after it.
    ///
    /// ⚠⚠ **This mask's own adjustments are never in it, and that is what makes the cache worth
    /// having** — moving its exposure or saturation leaves the identity unchanged. That is the case
    /// a mask exists for and the only thing this buys: a **global** slider genuinely changes the
    /// base, and no cache can help it.
    ///
    /// ⚠ Until M10.12b it was the masks before this one, truncated — which a later mask's offset now
    /// reaches past, so a gating brush would have been served a map gated on pixels that no longer
    /// existed (`a_gating_brushs_base_follows_a_later_masks_offset`).
    ///
    /// ⚠ It fails toward a **miss**, [`PrefixKey`]'s rule. The dangerous direction is a stale hit:
    /// coverage rendered positionally while the canvas tint shows it stopping at an edge, which is
    /// the silent preview/export divergence this project has recorded more than any other.
    base: Option<(Edits, usize)>,
}

/// Rasterized brush coverage, **per mask**, so an unrelated slider drag does not re-rasterize.
///
/// ⚠⚠ **Per mask rather than per render, and the win depends on it.** One entry holding every
/// mask's map would be invalidated by a change to any one of them — including mask 0's own
/// adjustment invalidating mask 0's own coverage, which is exactly the case this exists to make
/// free.
///
/// ⚠ It misses on every frame *while painting*, and that is correct — the strokes really did
/// change, so there is nothing to reuse.
#[derive(Default)]
pub(super) struct BrushCache {
    pub(super) entries: Vec<(MaskCoverageKey, Arc<pipeline::brush::CoverageMap>)>,
    /// The AI maps, shared with the session (M9.13).
    pub(super) ai: crate::ai_coverage::SharedAiMaps,
}

impl BrushCache {
    /// Two entries per **component**, and the reason 2 is unchanged from
    /// [`PrefixCache::FINALS_CAP`]: two rungs are live at once, so one entry per component would
    /// be evicted on every zoom change and rebuilt coming back.
    ///
    /// ⚠ Per component rather than per mask — a composite of three brushes needs three rows kept,
    /// and counting masks would make a composite's own siblings evict each other on every render.
    ///
    /// ⚠ A `CoverageMap` is 8-bit and **bbox-bounded**, so a stroke over a quarter of a 1.5 Mpx
    /// region is ~390 KB. Four masks at two rungs is a few megabytes against the prefix cache's
    /// 512 MB budget.
    fn cap(edits: &Edits) -> usize {
        let components: usize = edits.masks.iter().map(|m| 1 + m.refine.len()).sum();
        2 * components.max(1)
    }

    /// The base identity for mask `i`, or `None` when this brush does not gate on the image.
    ///
    /// ⚠⚠ **No component index here, and that is not an omission.** Every component of mask `i`
    /// reads the *same* base — the buffer the stage was handed — because components change the
    /// composite's coverage, never its pixels. A component index in the base identity would be
    /// wrong rather than merely redundant.
    fn base_id(
        edits: &Edits,
        i: usize,
        brush: &edit_model::Brush,
        max_dim: usize,
    ) -> Option<(Edits, usize)> {
        if !brush.needs_base() {
            return None;
        }
        let mut upstream = edits.clone();
        upstream.masks = if edits.masks.get(i).is_some_and(pipeline::reads_reference) {
            // ⚠ The reference photo (M10.12b): the global edits and no mask at all. Its map is built
            // and deposited by `SceneOffsetsStage`, through the same `reads_reference`.
            Vec::new()
        } else {
            // ⚠ The masks before this one whole; a later one only through its scene offsets, which
            // reach this base in `SceneOffsetsStage` at the head of the tail. Its look sliders run
            // after this mask's stage — keying on them would miss for nothing.
            edits.masks[..i.min(edits.masks.len())]
                .iter()
                .cloned()
                .chain(
                    edits.masks[i.min(edits.masks.len())..]
                        .iter()
                        .filter(|m| !m.disabled && m.adjustments.has_scene_offsets())
                        .map(|m| edit_model::MaskedAdjustment {
                            adjustments: pipeline::scene_only(&m.adjustments),
                            ..m.clone()
                        }),
                )
                .collect()
        };
        Some((upstream, max_dim))
    }

    /// The shape at component `j` of mask `i` — `0` is the base, `j > 0` the `j-1`th refinement.
    ///
    /// ⚠ One resolver, used by `key` and by `get_or_build`. Two derivations of "which shape is
    /// this component" is how the key and the thing it keys would come to disagree.
    fn mask_at(edits: &Edits, i: usize, j: usize) -> Option<&edit_model::Mask> {
        let m = edits.masks.get(i)?;
        match j.checked_sub(1) {
            None => Some(&m.mask),
            Some(k) => m.refine.get(k).map(|r| &r.mask),
        }
    }

    /// The key for component `j` of mask `i`.
    ///
    /// ⚠⚠ **There is deliberately NO component index in the key, and the sabotage step is what
    /// established that.** The plan added one, on the reasoning that a composite's base and its
    /// refinements are different coverage under one mask index — and forcing it to `(i, 0)` left
    /// every test passing, including the one written to catch exactly that. The reason nothing
    /// caught it is that there is nothing to catch: the key already holds **the brush itself**,
    /// plus the geometry, the frame size, the region and the base identity, which is the complete
    /// list of things a component's coverage is a function of. Two components with identical
    /// brushes therefore have identical coverage and *should* share one entry; an index would
    /// only prevent a legitimate hit.
    ///
    /// ⚠ `i` and `j` are still parameters because they say **which shape** this is — see
    /// [`Self::mask_at`] — and `i` alone decides the base identity.
    fn key(
        edits: &Edits,
        i: usize,
        j: usize,
        spec: CoverageSpec,
        ai: &crate::ai_coverage::AiMaps,
    ) -> MaskCoverageKey {
        let brush = match Self::mask_at(edits, i, j) {
            Some(edit_model::Mask::Brush(b)) => Some(b.clone()),
            _ => None,
        };
        let base = brush
            .as_ref()
            .and_then(|b| Self::base_id(edits, i, b, spec.max_dim));
        // ⚠ The map's address is its identity: a new map for the same prompt is a new `Arc`.
        // No map is address 0 — cached as nothing, and missed the moment one arrives.
        let ai = match Self::mask_at(edits, i, j) {
            Some(edit_model::Mask::Ai(a)) => {
                Some((*a, ai.get(&a.prompt).map_or(0, |m| Arc::as_ptr(m) as usize)))
            }
            _ => None,
        };
        MaskCoverageKey {
            ai,
            brush,
            geometry: edits.geometry,
            fw: spec.fw,
            fh: spec.fh,
            region: spec.region,
            base,
        }
    }

    /// Store coverage a stage reported for component `j` of mask `i`.
    pub(super) fn remember(
        &mut self,
        edits: &Edits,
        spec: CoverageSpec,
        i: usize,
        j: usize,
        map: Arc<pipeline::brush::CoverageMap>,
    ) {
        let key = Self::key(edits, i, j, spec, &self.ai.read().expect("ai maps"));
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, map));
        self.entries.truncate(Self::cap(edits));
    }

    /// [`Self::get_or_build`] with no decode and no lens — the brush tests', which have no AI mask.
    #[cfg(test)]
    fn build_for_test(
        &mut self,
        edits: &Edits,
        spec: CoverageSpec,
    ) -> Arc<pipeline::BrushCoverage> {
        self.get_or_build(edits, spec, (0, 0), None)
    }

    /// `decode` is the source buffer's size and `lens` its lens shot — what an AI component needs to
    /// map frame pixels into its decode-space map (M9.13).
    pub(super) fn get_or_build(
        &mut self,
        edits: &Edits,
        spec: CoverageSpec,
        decode: (usize, usize),
        lens: Option<&lens_profile::LensShot>,
    ) -> Arc<pipeline::BrushCoverage> {
        let ai = self.ai.read().expect("ai maps").clone();
        // Built once per call, and only if an AI component needs it.
        let mut decode_map: Option<crate::ai_space::DecodeMap> = None;
        // ⚠ One row per mask, one slot per component within it — index 0 being the mask's base
        // shape. The row exists even for a mask with no brush at all, so the stage's `(i, j)`
        // always addresses the slot the cache computed a key for.
        let mut maps: Vec<Vec<Option<Arc<pipeline::brush::CoverageMap>>>> =
            Vec::with_capacity(edits.masks.len());
        for (i, m) in edits.masks.iter().enumerate() {
            let mut row = Vec::with_capacity(1 + m.refine.len());
            for j in 0..=m.refine.len() {
                let key = Self::key(edits, i, j, spec, &ai);
                if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
                    // Promote the hit, so the least recently used entry is the one evicted.
                    let hit = self.entries.remove(pos);
                    row.push(Some(Arc::clone(&hit.1)));
                    self.entries.insert(0, hit);
                    continue;
                }
                // ⚠⚠ **A gating brush is not built here, and must not be.** Its coverage depends
                // on pixel values and this function has no image; building one positionally is
                // what would render it as a plain stroke while the canvas tint showed it stopping
                // at an edge. `MaskStage::apply` builds it where the buffer is and reports it back
                // through `BrushCoverage::deposit`; `render_cached` stores it with `remember`.
                let built = match Self::mask_at(edits, i, j) {
                    Some(edit_model::Mask::Brush(b)) if !b.needs_base() => {
                        Some(Arc::new(pipeline::brush::rasterize_in(
                            b,
                            &edits.geometry,
                            spec.fw,
                            spec.fh,
                            spec.region,
                            None,
                        )))
                    }
                    // ⚠ M9.13: an AI component is its map, resampled into this frame with its
                    // threshold and invert — no map, no coverage (`CoverageSource::Raster`).
                    // ⚠ Warped through the lens ONCE per map and lens setting (`post_lens`), so a
                    // geometry drag pays only the geometry's inverse — see `post_lens`'s numbers.
                    Some(edit_model::Mask::Ai(a)) => ai.get(&a.prompt).map(|map| {
                        let lensed = crate::ai_coverage::post_lens(
                            map,
                            crate::ai_space::lens_key(lens, edits, decode.0, decode.1),
                            decode,
                            || {
                                lens.and_then(|l| {
                                    crate::ai_space::corrector_for(l, edits, decode.0, decode.1)
                                })
                            },
                        );
                        let dm = decode_map.get_or_insert_with(|| {
                            crate::ai_space::DecodeMap::new(
                                &edits.geometry,
                                edits.perspective,
                                spec.fw,
                                spec.fh,
                                None,
                                decode.0,
                                decode.1,
                            )
                        });
                        Arc::new(crate::ai_coverage::to_frame(&lensed, a, dm, spec.region))
                    }),
                    _ => None,
                };
                if let Some(built) = &built {
                    self.entries.insert(0, (key, Arc::clone(built)));
                    self.entries.truncate(Self::cap(edits));
                }
                row.push(built);
            }
            maps.push(row);
        }
        let built = maps
            .iter()
            .map(|row| row.iter().map(|_| std::sync::Mutex::new(None)).collect())
            .collect();
        Arc::new(pipeline::BrushCoverage { maps, built })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brush_cache_never_builds_a_brush_that_needs_the_image() {
        // ⚠⚠ **What makes the dangerous state unrepresentable rather than documented.** A map
        // built here is rasterized without pixels, so an auto-masked brush served from one would
        // render positionally while the canvas tint showed it stopping at an edge — a silent
        // divergence between preview and export.
        //
        // ⚠⚠ **This test was INVERTED rather than deleted, and it used to assert something
        // stronger: that the cache *declines* a gating brush outright.** That was right while
        // nothing could identify the base. The cache now stores one, keyed on what produced that
        // base — but it still never **builds** one, because it has no image, and that is the half
        // that keeps the dangerous state unrepresentable. An inversion that changes only the
        // assertions leaves a name and a comment that lie, which is what happened once when the
        // spot-removal render rule was inverted.
        let gating = |auto: bool| {
            let mut e = Edits::default();
            e.masks.push(edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.3, 0.3], [0.7, 0.7]],
                        radius: 0.2,
                        auto_mask: auto,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: Default::default(),
                disabled: false,
                invert: false,
            });
            e
        };
        let mut cache = BrushCache::default();
        let built = cache.build_for_test(
            &gating(true),
            CoverageSpec {
                fw: 64,
                fh: 64,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            built.maps[0][0].is_none(),
            "an auto-masked brush must not be prebuilt"
        );

        // ⚠ The discriminating half: a plain brush must STILL be cached, or this would be a
        // performance regression wearing a correctness fix.
        let built = cache.build_for_test(
            &gating(false),
            CoverageSpec {
                fw: 64,
                fh: 64,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            built.maps[0][0].is_some(),
            "a plain brush must still be cached"
        );
    }

    /// An auto-masked brush mask, plus a global exposure, so the two can be moved independently.
    fn auto_masked_edits(exposure: f32, mask_exposure: f32) -> Edits {
        let mut m = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![edit_model::BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.3,
                    hardness: 100.0,
                    flow: 100.0,
                    auto_mask: true,
                    tolerance: 35.0,
                    ..Default::default()
                }],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: Default::default(),
            disabled: false,
            invert: false,
        };
        m.adjustments.exposure = mask_exposure;
        Edits {
            exposure,
            masks: vec![m],
            ..Edits::default()
        }
    }

    // ---- M10.12b: the base a gating brush really reads ----

    /// A linear mask over everything with these adjustments — a second mask beside the brush.
    fn linear(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(Default::default()),
            adjustments: a,
            ..Default::default()
        }
    }

    /// The gating brush (mask 0) carrying `a`, then `later` (mask 1).
    fn brush_then(a: edit_model::LocalAdjustments, later: edit_model::MaskedAdjustment) -> Edits {
        let mut e = auto_masked_edits(0.0, 0.0);
        e.masks[0].adjustments = a;
        e.masks.push(later);
        e
    }

    fn base_of(e: &Edits) -> Option<(Edits, usize)> {
        let edit_model::Mask::Brush(b) = &e.masks[0].mask else {
            panic!("fixture check: mask 0 is the brush")
        };
        BrushCache::base_id(e, 0, b, 1600)
    }

    fn sat(v: f32) -> edit_model::LocalAdjustments {
        edit_model::LocalAdjustments {
            saturation: v,
            ..Default::default()
        }
    }

    fn ev(v: f32) -> edit_model::LocalAdjustments {
        edit_model::LocalAdjustments {
            exposure: v,
            ..Default::default()
        }
    }

    /// ⚠⚠ Since M10.12b a LATER mask's scene offset reaches this brush's base (`SceneOffsetsStage`
    /// runs every mask's offsets at the head of the tail), so it must miss — keyed on the masks
    /// before this one alone, the old key served a map gated on pixels that no longer exist.
    #[test]
    fn a_gating_brushs_base_follows_a_later_masks_offset() {
        assert_ne!(
            base_of(&brush_then(sat(20.0), linear(ev(0.5)))),
            base_of(&brush_then(sat(20.0), linear(ev(0.9))))
        );
    }

    /// …while a later mask's LOOK slider runs after this brush's stage and must not cost a miss —
    /// for a later mask with an offset too, whose scene part IS in the key.
    #[test]
    fn a_gating_brushs_base_ignores_a_later_masks_look_slider() {
        let later = |s: f32| {
            linear(edit_model::LocalAdjustments {
                exposure: 0.5,
                saturation: s,
                ..Default::default()
            })
        };
        assert_eq!(
            base_of(&brush_then(sat(20.0), linear(sat(10.0)))),
            base_of(&brush_then(sat(20.0), linear(sat(40.0))))
        );
        assert_eq!(
            base_of(&brush_then(sat(20.0), later(10.0))),
            base_of(&brush_then(sat(20.0), later(40.0)))
        );
    }

    /// A gating brush with its own offset selects on the reference photo — the global edits, no mask
    /// at all — so its own sliders and every other mask leave it, and a global slider moves it.
    ///
    /// ⚠ The brush is mask **1**, behind another mask: as mask 0 the old key — the masks before it —
    /// was already "no mask", and this test passed against it.
    #[test]
    fn a_gating_brush_with_an_offset_is_keyed_on_the_reference() {
        let with = |earlier: edit_model::MaskedAdjustment, own: edit_model::LocalAdjustments| {
            let mut e = auto_masked_edits(0.0, 0.0);
            e.masks[0].adjustments = own;
            e.masks.insert(0, earlier);
            let edit_model::Mask::Brush(b) = &e.masks[1].mask else {
                panic!("fixture check: mask 1 is the brush")
            };
            let id = BrushCache::base_id(&e, 1, b, 1600);
            (e, id)
        };
        let both = |e: f32, s: f32| edit_model::LocalAdjustments {
            exposure: e,
            saturation: s,
            ..Default::default()
        };
        let (_, reference) = with(linear(sat(10.0)), both(1.0, 20.0));
        assert_eq!(
            reference,
            with(linear(sat(10.0)), both(1.0, 50.0)).1,
            "its own look slider"
        );
        assert_eq!(
            reference,
            with(linear(sat(10.0)), both(2.0, 20.0)).1,
            "its own offset"
        );
        assert_eq!(
            reference,
            with(linear(sat(40.0)), both(1.0, 20.0)).1,
            "an earlier mask's look"
        );
        assert_eq!(
            reference,
            with(linear(ev(0.7)), both(1.0, 20.0)).1,
            "an earlier mask's offset"
        );
        let (mut global, _) = with(linear(sat(10.0)), both(1.0, 20.0));
        global.exposure = 0.5;
        let edit_model::Mask::Brush(b) = &global.masks[1].mask else {
            unreachable!()
        };
        assert_ne!(
            reference,
            BrushCache::base_id(&global, 1, b, 1600),
            "a global slider"
        );
    }

    /// A coverage map distinguishable from a default one, so a hit can be told from a miss.
    fn a_coverage_map() -> Arc<pipeline::brush::CoverageMap> {
        Arc::new(pipeline::brush::CoverageMap {
            outside: 42,
            ..Default::default()
        })
    }

    /// ⚠⚠ **The load-bearing pair, and neither half alone means anything.** A test of the miss
    /// alone passes against a cache that never hits; a test of the hit alone passes against one
    /// that never misses.
    #[test]
    fn the_gating_coverage_survives_the_masks_own_slider_and_not_a_global_one() {
        let mut cache = BrushCache::default();
        let base = auto_masked_edits(0.0, 0.0);
        assert!(
            cache
                .build_for_test(
                    &base,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_none(),
            "fixture check: a cold cache must prebuild nothing for a gating brush, or the \
             assertions below are about the wrong thing"
        );
        cache.remember(
            &base,
            CoverageSpec {
                fw: 64,
                fh: 48,
                region: None,
                max_dim: 1600,
            },
            0,
            0,
            a_coverage_map(),
        );

        // ⚠ The mask's OWN adjustment moves. Its base is the output after the masks BEFORE it, so
        // this cannot change it — and that is the whole reason the key truncates.
        let own = auto_masked_edits(0.0, 1.5);
        assert!(
            cache
                .build_for_test(
                    &own,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_some(),
            "moving the mask's own slider must not invalidate its coverage — that is the case a \
             mask exists for, and the only thing this cache buys"
        );

        // ⚠ A GLOBAL adjustment moves. The masks loop runs after the entire global tail, so the
        // base really did change.
        let global = auto_masked_edits(1.5, 0.0);
        assert!(
            cache
                .build_for_test(
                    &global,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 1600
                    }
                )
                .get(0, 0)
                .is_none(),
            "a global slider changes the base, so serving the old coverage would render the mask \
             against pixels it was never gated on"
        );
    }

    /// ⚠ The rung is in the base identity: the base is a different image at a different resolution.
    #[test]
    fn the_gating_coverage_does_not_cross_rungs() {
        let mut cache = BrushCache::default();
        let e = auto_masked_edits(0.0, 0.0);
        cache.remember(
            &e,
            CoverageSpec {
                fw: 64,
                fh: 48,
                region: None,
                max_dim: 1600,
            },
            0,
            0,
            a_coverage_map(),
        );
        assert!(
            cache
                .build_for_test(
                    &e,
                    CoverageSpec {
                        fw: 64,
                        fh: 48,
                        region: None,
                        max_dim: 3200
                    }
                )
                .get(0, 0)
                .is_none(),
            "coverage built at one rung must not be served at another"
        );
    }

    fn brush_edits() -> Edits {
        Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.3, 0.5], [0.7, 0.5]],
                        radius: 0.1,
                        hardness: 50.0,
                        flow: 100.0,
                        erase: false,
                        ..Default::default()
                    }],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        }
    }

    /// Whether two coverage sets share mask 0's **map**, which is what "did not re-rasterize"
    /// means.
    ///
    /// ⚠⚠ **The observable moved and the old one silently stopped working.** These tests compared
    /// the outer `Arc<BrushCoverage>`, which was one cached object per render. Entries are per
    /// mask now and the set is assembled fresh each call, so outer identity is **never** equal —
    /// the three `ptr_eq` assertions began failing and, worse, the two `!ptr_eq` ones began
    /// passing **vacuously**. Only running them showed it.
    fn same_map(a: &Arc<pipeline::BrushCoverage>, b: &Arc<pipeline::BrushCoverage>) -> bool {
        same_map_at(a, b, 0, 0)
    }

    /// The same question for one component of one mask.
    ///
    /// ⚠ A composite's base and its refinements are different coverage under one mask index, so
    /// "did this shape re-rasterize" is only answerable per component.
    fn same_map_at(
        a: &Arc<pipeline::BrushCoverage>,
        b: &Arc<pipeline::BrushCoverage>,
        i: usize,
        j: usize,
    ) -> bool {
        match (a.get(i, j), b.get(i, j)) {
            (Some(x), Some(y)) => Arc::ptr_eq(&x, &y),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn the_brush_cache_hits_and_keeps_both_zoom_levels() {
        // ⚠ Every other test in this file would pass with the brush cache deleted, because it is
        // a caching change and nothing else. This one asserts identity of the Arc, which is the
        // only observable difference between a hit and a rebuild.
        let mut cache = BrushCache::default();
        let edits = brush_edits();
        let a = cache.build_for_test(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let b = cache.build_for_test(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &b), "the second call must hit");

        let c = cache.build_for_test(
            &edits,
            CoverageSpec {
                fw: 400,
                fh: 200,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "different frame dims must miss");

        // ⚠ The assertion that pins CAP = 2 rather than 1. With a single entry the 400x200 build
        // would have evicted the 200x100 one, and this would rebuild — which is exactly the zoom
        // thrash the capacity exists to prevent.
        let d = cache.build_for_test(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &d), "CAP=2 must keep both zoom levels warm");
    }

    #[test]
    fn changing_a_stroke_misses_the_brush_cache() {
        // The other half: a hit must depend on the strokes, not merely on the dimensions.
        let mut cache = BrushCache::default();
        let a = cache.build_for_test(
            &brush_edits(),
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let mut edited = brush_edits();
        if let edit_model::Mask::Brush(b) = &mut edited.masks[0].mask {
            b.strokes[0].points.push([0.8, 0.6]);
        }
        let c = cache.build_for_test(
            &edited,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "an extra point must miss");
    }

    #[test]
    fn the_coverage_index_follows_the_mask_position_not_the_active_count() {
        // ⚠ An INACTIVE mask still owns a slot. A counter over active masks would misalign every
        // map after the first inactive one — the same sparse-versus-dense index bug this codebase
        // has hit three times before.
        let mut edits = brush_edits();
        let inactive = edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments::default(),
            disabled: false,
            invert: false,
        };
        edits.masks.insert(0, inactive);
        let mut cache = BrushCache::default();
        let cov = cache.build_for_test(
            &edits,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(cov.get(0, 0).is_none(), "slot 0 is the analytic mask");
        assert!(cov.get(1, 0).is_some(), "slot 1 is the brush");
    }

    /// A composite whose base **and** refinement are both brushes, with different strokes so
    /// their maps are distinguishable.
    fn composite_of_two_brushes() -> Edits {
        let stroke = |x: f32| edit_model::BrushStroke {
            points: vec![[x, 0.5]],
            radius: 0.1,
            hardness: 50.0,
            flow: 100.0,
            erase: false,
            ..Default::default()
        };
        let brush = |x: f32| {
            edit_model::Mask::Brush(edit_model::Brush {
                strokes: vec![stroke(x)],
                invert: false,
            })
        };
        let mut e = brush_edits();
        e.masks[0].mask = brush(0.3);
        e.masks[0].refine = vec![edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: brush(0.7),
        }];
        e
    }

    fn whole_frame() -> CoverageSpec {
        CoverageSpec {
            fw: 200,
            fh: 100,
            region: None,
            max_dim: 1600,
        }
    }

    #[test]
    fn two_components_of_one_mask_do_not_evict_each_other() {
        // ⚠ The cap counts **components**, not masks: a composite of two brushes must keep both
        // maps at both live rungs, or every zoom change would re-rasterize the pair.
        let mut cache = BrushCache::default();
        let e = composite_of_two_brushes();
        let a = cache.build_for_test(&e, whole_frame());
        assert!(a.get(0, 0).is_some(), "the base's map must be built");
        assert!(a.get(0, 1).is_some(), "the refinement's map must be built");
        let b = cache.build_for_test(&e, whole_frame());
        assert!(same_map_at(&a, &b, 0, 0), "the base must hit");
        assert!(same_map_at(&a, &b, 0, 1), "the refinement must hit");
    }

    #[test]
    fn changing_one_component_misses_only_that_component() {
        // ⚠⚠ The discriminating test. Without the component in the key both shapes collide on one
        // entry, so touching either would invalidate the other — and the symptom is not an error
        // but a shape rendering positionally while its tint shows the gate working.
        let mut cache = BrushCache::default();
        let e = composite_of_two_brushes();
        let a = cache.build_for_test(&e, whole_frame());
        let mut moved = e.clone();
        if let Some(edit_model::Mask::Brush(b)) =
            moved.masks[0].refine.get_mut(0).map(|r| &mut r.mask)
        {
            b.strokes[0].points.push([0.75, 0.55]);
        }
        let b = cache.build_for_test(&moved, whole_frame());
        assert!(
            same_map_at(&a, &b, 0, 0),
            "the untouched base must still hit"
        );
        assert!(
            !same_map_at(&a, &b, 0, 1),
            "the changed refinement must miss"
        );
    }

    #[test]
    fn the_brush_cache_misses_when_the_region_moves() {
        // ⚠⚠ This REVERSES the rule `PrefixCache` follows, deliberately. Keeping the region out
        // of the key buys a cache that survives a pan — invisible, because panning re-runs the
        // whole tail anyway — and costs a full-frame rasterization on every paint frame:
        // 23.76 ms at 6246x4170, measured, and reported in use as "very sluggish at 1:1".
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let a = cache.build_for_test(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        let b = cache.build_for_test(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        assert!(same_map(&a, &b), "the same region must hit");
        let c = cache.build_for_test(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((20, 0, 50, 50)),
                max_dim: 1600,
            },
        );
        assert!(!same_map(&a, &c), "a moved region must miss");
    }

    #[test]
    fn the_brush_cache_still_hits_when_only_a_slider_moves() {
        // The cache's remaining job, and the reason it exists at all: dragging an unrelated
        // slider with a brush mask present must not re-rasterize on every frame.
        let mut cache = BrushCache::default();
        let a = cache.build_for_test(
            &brush_edits(),
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        let mut louder = brush_edits();
        louder.exposure = 1.5;
        let b = cache.build_for_test(
            &louder,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: None,
                max_dim: 1600,
            },
        );
        assert!(
            same_map(&a, &b),
            "a global slider does not touch the strokes and must hit"
        );
    }

    #[test]
    fn a_clipped_cache_entry_covers_only_its_region() {
        // ⚠ The clip really is applied, not merely keyed on. Without it the two entries would be
        // identical maps and this would pass for the wrong reason, so it asserts a frame pixel
        // *outside* the clip reads nothing.
        let mut cache = BrushCache::default();
        let e = brush_edits();
        let cov = cache.build_for_test(
            &e,
            CoverageSpec {
                fw: 200,
                fh: 100,
                region: Some((0, 0, 40, 100)),
                max_dim: 1600,
            },
        );
        let map = cov.get(0, 0).expect("the brush mask has a map");
        assert_eq!(map.at(120, 50), 0, "beyond the clip there is no coverage");
    }
}

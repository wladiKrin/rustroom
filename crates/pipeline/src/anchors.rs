//! Where a stored mark is, and where the stage that draws it works (M10.12c).
//!
//! ⚠⚠ **A spot, an eye or a brush stroke marked `decode` is stored in DECODE space** — normalised over the
//! undeveloped decode, before lens correction, `Removal::points`' space — so it stays on its content when lens
//! correction or perspective changes. The stages still work where they always did: spots and eyes in the
//! post-lens, pre-perspective frame, strokes in the post-perspective, pre-geometry frame. [`resolve`] maps one
//! to the other, once, at each renderer's entry; nothing downstream changes. A mark without the flag is from
//! before and already in its stage's space — it passes through untouched, so no saved photo renders differently.
//!
//! ⚠ The lens has no forward map (lensfun gives only the corrected → raw direction resampling needs), so
//! decode → post-lens is solved by fixed-point iteration — `DecodeMap::post_lens_of`'s method; eight steps land
//! within a hundredth of a pixel.
//!
//! ⚠⚠ **The corrector is built at a NOMINAL size, never the decode's.** Lensfun normalises by the image's own
//! size, so a normalised point maps the same at any size — `the_nominal_corrector_agrees_with_a_full_size_one`
//! measures it — while a full-size build samples its whole field (290–370 ms on an X-T4). It is memoised on the
//! lens, the corrections and the aspect, because a renderer resolves on every frame.

use crate::lens_grid::DisplacementGrid;
use crate::perspective::Homography;
use edit_model::{Edits, LensCorrection, Mask, Perspective};
use lens_profile::{Corrections, Corrector, LensShot};
use std::borrow::Cow;
use std::sync::{Arc, Mutex, OnceLock};

/// The nominal corrector's long edge, in pixels.
const NOMINAL: usize = 1024;

/// Fixed-point steps from decode to post-lens.
const STEPS: usize = 8;

/// The displacement grid's step at the nominal size. ⚠ A point is looked up through the grid, never the corrector:
/// lensfun one point at a time made 20,000 stroke points cost 20 ms, every frame.
const GRID_STEP: usize = 4;

/// The maps between decode space and the stages' spaces, for one photo's lens, corrections and perspective.
pub struct Anchoring {
    lens: Option<(Arc<DisplacementGrid>, f32, f32)>,
    hom: Option<Homography>,
    aspect: f32,
}

impl Anchoring {
    /// `decode_w × decode_h` is the decode's size (only its aspect matters). No lens, an inactive correction or a
    /// profile that corrects nothing leaves the lens half the identity; an inactive perspective the other half.
    pub fn new(
        lens: Option<&LensShot>,
        lc: LensCorrection,
        p: Perspective,
        decode_w: usize,
        decode_h: usize,
    ) -> Self {
        let aspect = decode_w.max(1) as f32 / decode_h.max(1) as f32;
        let (nw, nh) = if aspect >= 1.0 {
            (NOMINAL, ((NOMINAL as f32 / aspect).round() as usize).max(1))
        } else {
            (((NOMINAL as f32 * aspect).round() as usize).max(1), NOMINAL)
        };
        let lens = lens
            .filter(|_| lc.is_active())
            .and_then(|l| corrector(l, lc, nw, nh))
            .map(|c| (c, nw as f32, nh as f32));
        Self {
            lens,
            // ⚠ The same `from_edits` at the same aspect as `GeometryStage`'s fused path and `SpotMap`.
            hom: p.is_active().then(|| Homography::from_edits(&p, aspect)),
            aspect,
        }
    }

    /// [`Self::new`] for `edits`' own corrections and perspective.
    pub fn for_edits(
        lens: Option<&LensShot>,
        edits: &Edits,
        decode_w: usize,
        decode_h: usize,
    ) -> Self {
        Self::new(
            lens,
            edits.applied_lens_correction(),
            edits.perspective,
            decode_w,
            decode_h,
        )
    }

    /// Whether both halves are the identity — every point maps to itself.
    pub fn is_identity(&self) -> bool {
        self.lens.is_none() && self.hom.is_none()
    }

    /// A post-lens point → the decode point it was read from (the green channel, `DecodeMap`'s choice: TCA moves
    /// the three apart by about a pixel).
    pub fn post_lens_to_decode(&self, p: [f32; 2]) -> [f32; 2] {
        match &self.lens {
            // ⚠ A normalised point is a pixel's CENTRE at `i + 0.5`; the field is indexed by pixel, as the lens
            // stage reads it. Without the half pixel the nominal and the full-size maps disagree by ~2 px.
            Some((g, w, h)) => {
                let s = g.source(p[0] * w - 0.5, p[1] * h - 0.5, 1);
                [(s[0] + 0.5) / w, (s[1] + 0.5) / h]
            }
            None => p,
        }
    }

    /// A decode point → the post-lens point that reads it: [`Self::post_lens_to_decode`] inverted by iteration.
    pub fn decode_to_post_lens(&self, d: [f32; 2]) -> [f32; 2] {
        let mut p = d;
        if self.lens.is_some() {
            for _ in 0..STEPS {
                let at = self.post_lens_to_decode(p);
                p = [p[0] + d[0] - at[0], p[1] + d[1] - at[1]];
            }
        }
        p
    }

    /// A decode point → a stroke's space: post-lens, then the keystone forward.
    pub fn decode_to_stroke(&self, d: [f32; 2]) -> [f32; 2] {
        let p = self.decode_to_post_lens(d);
        match &self.hom {
            Some(h) => self.unit(h.output_of(self.centred(p))),
            None => p,
        }
    }

    /// A point in a stroke's space → decode: the keystone's inverse, then the lens's.
    pub fn stroke_to_decode(&self, s: [f32; 2]) -> [f32; 2] {
        let p = match &self.hom {
            Some(h) => self.unit(h.source_of(self.centred(s))),
            None => s,
        };
        self.post_lens_to_decode(p)
    }

    /// `[0, 1]` → the homography's space: centred, half-height 1 (`SpotMap::hom_of`).
    fn centred(&self, q: [f32; 2]) -> (f32, f32) {
        ((q[0] - 0.5) * 2.0 * self.aspect, (q[1] - 0.5) * 2.0)
    }

    fn unit(&self, n: (f32, f32)) -> [f32; 2] {
        [n.0 / (2.0 * self.aspect) + 0.5, n.1 / 2.0 + 0.5]
    }
}

/// The field of the corrector the lens stage would build, at `w × h` — memoised, because [`resolve`] runs every
/// frame.
fn corrector(
    l: &LensShot,
    lc: LensCorrection,
    w: usize,
    h: usize,
) -> Option<Arc<DisplacementGrid>> {
    type Memo = Vec<(String, Option<Arc<DisplacementGrid>>)>;
    static MEMO: OnceLock<Mutex<Memo>> = OnceLock::new();
    let key = format!("{l:?}|{lc:?}|{w}x{h}");
    let memo = MEMO.get_or_init(Default::default);
    if let Some((_, c)) = memo.lock().ok()?.iter().find(|(k, _)| *k == key) {
        return c.clone();
    }
    let c = Corrector::new(
        &l.profile,
        &l.shot,
        w,
        h,
        Corrections {
            distortion: lc.distortion,
            tca: lc.tca,
            vignetting: lc.vignetting,
            scale: lc.scale,
        },
    )
    .map(|c| Arc::new(DisplacementGrid::build(&c, w, h, GRID_STEP)));
    let mut m = memo.lock().ok()?;
    // A handful of photos' worth: one entry per lens, corrections and aspect in use.
    if m.len() >= 16 {
        m.remove(0);
    }
    m.push((key, c.clone()));
    c
}

/// Whether any mark in `edits` is stored in decode space.
pub fn has_anchored(edits: &Edits) -> bool {
    edits.spots.iter().any(|s| s.decode)
        || edits.eyes.iter().any(|e| e.decode)
        || brushes(edits).any(|b| b.strokes.iter().any(|s| s.decode))
}

/// Whether any mark in `edits` is still in its stage's own space — from before M10.12c.
pub fn has_legacy(edits: &Edits) -> bool {
    edits.spots.iter().any(|s| !s.decode)
        || edits.eyes.iter().any(|e| !e.decode)
        || brushes(edits).any(|b| b.strokes.iter().any(|s| !s.decode && !s.points.is_empty()))
}

fn brushes(edits: &Edits) -> impl Iterator<Item = &edit_model::Brush> {
    edits.masks.iter().flat_map(|m| {
        std::iter::once(&m.mask)
            .chain(m.refine.iter().map(|r| &r.mask))
            .filter_map(|k| match k {
                Mask::Brush(b) => Some(b),
                _ => None,
            })
    })
}

fn brushes_mut(edits: &mut Edits) -> impl Iterator<Item = &mut edit_model::Brush> {
    edits.masks.iter_mut().flat_map(|m| {
        std::iter::once(&mut m.mask)
            .chain(m.refine.iter_mut().map(|r| &mut r.mask))
            .filter_map(|k| match k {
                Mask::Brush(b) => Some(b),
                _ => None,
            })
    })
}

/// `edits` with every decode-space mark mapped to its stage's space and its flag cleared — what every renderer
/// builds its pipeline from. Borrowed, untouched, when nothing is anchored.
pub fn resolve<'a>(edits: &'a Edits, a: &Anchoring) -> Cow<'a, Edits> {
    if !has_anchored(edits) {
        return Cow::Borrowed(edits);
    }
    let mut e = edits.clone();
    for s in e.spots.iter_mut().filter(|s| s.decode) {
        s.centre = a.decode_to_post_lens(s.centre);
        s.source = a.decode_to_post_lens(s.source);
        s.decode = false;
    }
    for y in e.eyes.iter_mut().filter(|y| y.decode) {
        y.centre = a.decode_to_post_lens(y.centre);
        y.decode = false;
    }
    for b in brushes_mut(&mut e) {
        for s in b.strokes.iter_mut().filter(|s| s.decode) {
            for p in &mut s.points {
                *p = a.decode_to_stroke(*p);
            }
            s.decode = false;
        }
    }
    Cow::Owned(e)
}

/// Move every mark still in its stage's space into decode space, keeping where it renders — what opening a photo
/// does once its lens is known. Returns whether anything changed.
pub fn migrate(edits: &mut Edits, a: &Anchoring) -> bool {
    let mut changed = false;
    for s in edits.spots.iter_mut().filter(|s| !s.decode) {
        s.centre = a.post_lens_to_decode(s.centre);
        s.source = a.post_lens_to_decode(s.source);
        s.decode = true;
        changed = true;
    }
    for y in edits.eyes.iter_mut().filter(|y| !y.decode) {
        y.centre = a.post_lens_to_decode(y.centre);
        y.decode = true;
        changed = true;
    }
    for b in brushes_mut(edits) {
        for s in b.strokes.iter_mut().filter(|s| !s.decode) {
            for p in &mut s.points {
                *p = a.stroke_to_decode(*p);
            }
            s.decode = true;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
#[path = "anchors_tests.rs"]
mod tests;

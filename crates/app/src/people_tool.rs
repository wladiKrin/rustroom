//! Find people (M10.16c3), outside any thread or window: a search over the upright photo — YOLOX-m's boxes and
//! MobileSAM's outline of each — handed back in DECODE space, the pre-tick, and the removals the ticked ones become.
//!
//! ⚠ The networks see the photo upright (`ai_worker::model_input`); everything kept is turned back to the decode's
//! orientation, the space removals live in.

use crate::remove_tool::Area;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// A person this tall — a share of the upright frame's height — or taller is the photo's subject: found, shown,
/// never pre-ticked. Right for the subject on all seven of the spike's sheets.
pub const SUBJECT: f32 = 0.35;
/// The side a person's outline is kept at for the canvas — a preview; the fill makes its own at 512.
pub const AREA_SIDE: usize = 128;
/// The outline's rectangle: the box grown by this share of its size a side, so the shading has room.
const AREA_MARGIN: f32 = 0.1;

/// One person found, in decode space.
#[derive(Debug, Clone)]
pub struct Person {
    pub bbox: [f32; 4],
    pub score: f32,
    /// Their share of the upright frame's height — what [`SUBJECT`] is measured on.
    pub tall: f32,
    /// Their outline for the canvas — c1b's shading.
    pub area: Arc<Area>,
    /// The shading's border, decode-normalised closed loops (`contour::loops` at one half) — computed once here,
    /// never per frame.
    pub outline: Vec<Vec<[f32; 2]>>,
}

/// One person as a finder answers: in the upright image, their outline over the whole of it.
pub struct UprightPerson {
    pub bbox: [f32; 4],
    pub score: f32,
    pub mask: ai_mask::ProbMap,
}

/// What finds the people in an upright image, telling its progress (0..=1).
pub type Finder =
    Box<dyn FnOnce(&image::RgbImage, &dyn Fn(f32)) -> Result<Vec<UprightPerson>, String> + Send>;

/// YOLOX-m from `models`, then MobileSAM embedded once and prompted with each box — the people three quarters of
/// the progress, their outlines the rest.
pub fn real_finder(models: PathBuf) -> Finder {
    Box::new(move |img, progress| {
        let yolox = models.join(ai_mask::ModelId::YoloxM.spec().file);
        let people = ai_mask::people::People::load(&yolox).map_err(|e| e.to_string())?;
        let found = people
            .find(img, &|p| progress(0.75 * p))
            .map_err(|e| e.to_string())?;
        let [enc, dec] = ai_mask::registry::CLICK_MODELS.map(|m| models.join(m.spec().file));
        let mut sam = ai_mask::Sam::new(&enc, &dec);
        let e = sam.embed(img).map_err(|e| e.to_string())?;
        let n = found.len().max(1) as f32;
        let mut out = Vec::with_capacity(found.len());
        for (k, f) in found.into_iter().enumerate() {
            let mask = sam
                .prompt(&e, &[], Some(f.bbox))
                .map_err(|e| e.to_string())?;
            out.push(UprightPerson {
                bbox: f.bbox,
                score: f.score,
                mask,
            });
            progress(0.75 + 0.25 * (k + 1) as f32 / n);
        }
        Ok(out)
    })
}

/// What looks again where the author drew (M10.16c3, the author's (a)): the upright area, the drawing's box over it
/// (normalised, upright), the progress — the people there, or what is under the drawing.
pub type ThereFinder = Box<
    dyn FnOnce(&image::RgbImage, [f32; 4], &dyn Fn(f32)) -> Result<Vec<UprightPerson>, String>
        + Send,
>;

/// YOLOX-m's whole-frame pass over the area — close enough for a small person to be many pixels tall — then
/// MobileSAM; nobody found, MobileSAM's object under the drawing ([`there_people`]).
pub fn real_there_finder(models: PathBuf) -> ThereFinder {
    Box::new(move |img, drawn, progress| {
        let yolox = models.join(ai_mask::ModelId::YoloxM.spec().file);
        let people = ai_mask::people::People::load(&yolox).map_err(|e| e.to_string())?;
        let found = people.find_whole(img).map_err(|e| e.to_string())?;
        progress(0.4);
        let [enc, dec] = ai_mask::registry::CLICK_MODELS.map(|m| models.join(m.spec().file));
        let mut sam = ai_mask::Sam::new(&enc, &dec);
        let e = sam.embed(img).map_err(|e| e.to_string())?;
        progress(0.9);
        there_people(found, drawn, &mut |b| {
            sam.prompt(&e, &[], Some(b)).map_err(|e| e.to_string())
        })
    })
}

/// The people under a drawing: each find outlined from its box — or, the author's (a), nobody found, the object
/// under the drawing outlined from the drawing's box and offered as one, its box the outline's extent. An empty
/// outline is nobody.
pub fn there_people(
    found: Vec<ai_mask::people::Found>,
    drawn: [f32; 4],
    outline: &mut dyn FnMut([f32; 4]) -> Result<ai_mask::ProbMap, String>,
) -> Result<Vec<UprightPerson>, String> {
    if !found.is_empty() {
        return found
            .into_iter()
            .map(|f| {
                Ok(UprightPerson {
                    bbox: f.bbox,
                    score: f.score,
                    mask: outline(f.bbox)?,
                })
            })
            .collect();
    }
    let mask = outline(drawn)?;
    Ok(extent(&mask)
        .map(|bbox| UprightPerson {
            bbox,
            score: 0.0,
            mask,
        })
        .into_iter()
        .collect())
}

/// The box of a map's pixels at one half or above, normalised — `None` for an empty map.
fn extent(m: &ai_mask::ProbMap) -> Option<[f32; 4]> {
    let (w, h) = (m.w as usize, m.h as usize);
    let mut b = [usize::MAX, usize::MAX, 0, 0];
    let mut any = false;
    for (i, &v) in m.data.iter().enumerate() {
        if v >= 128 {
            let (x, y) = (i % w, i / w);
            b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
            any = true;
        }
    }
    any.then(|| {
        [
            b[0] as f32 / w as f32,
            b[1] as f32 / h as f32,
            b[2] as f32 / w as f32,
            b[3] as f32 / h as f32,
        ]
    })
}

/// The area looked at around a drawing grows by this share of the drawing's size on each side.
const LOOK_GROW: f32 = 0.5;
/// …and is at least this many decode pixels a side — the network's own square.
const LOOK_MIN: f32 = ai_mask::people::SIDE as f32;

/// The area to look again in, decode-normalised: the drawing's box (`points`, its brush `radius` a share of the long
/// edge, in a `w × h` decode), grown by half its size on each side, at least [`LOOK_MIN`] decode pixels a side,
/// moved inside the photo.
pub fn look_rect(points: &[[f32; 2]], radius: f32, w: usize, h: usize) -> [f32; 4] {
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let r = radius.max(0.0) * wf.max(hf);
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in points {
        let (x, y) = (p[0] * wf, p[1] * hf);
        b = [
            b[0].min(x - r),
            b[1].min(y - r),
            b[2].max(x + r),
            b[3].max(y + r),
        ];
    }
    if b[0] > b[2] {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let side = |lo: f32, hi: f32, full: f32| {
        let want = ((hi - lo) * (1.0 + 2.0 * LOOK_GROW))
            .max(LOOK_MIN)
            .min(full);
        let c = (lo + hi) / 2.0;
        let start = (c - want / 2.0).clamp(0.0, full - want);
        (start / full, (start + want) / full)
    };
    let (x0, x1) = side(b[0], b[2], wf);
    let (y0, y1) = side(b[1], b[3], hf);
    [x0, y0, x1, y1]
}

/// `p`, found in the area `rect` (decode-normalised), placed in the photo: its box, shading and border mapped from
/// the area's units into the decode's. Its `tall` stays the area's — the pre-tick is the whole search's.
pub fn place(p: Person, rect: [f32; 4]) -> Person {
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    let at = |q: [f32; 2]| [rect[0] + q[0] * rw, rect[1] + q[1] * rh];
    let bx = |b: [f32; 4]| {
        let (a, c) = (at([b[0], b[1]]), at([b[2], b[3]]));
        [a[0], a[1], c[0], c[1]]
    };
    let area = Area {
        rect: bx(p.area.rect),
        side: p.area.side,
        alpha: p.area.alpha.clone(),
    };
    Person {
        bbox: bx(p.bbox),
        score: p.score,
        tall: p.tall,
        area: Arc::new(area),
        outline: p
            .outline
            .into_iter()
            .map(|l| l.into_iter().map(at).collect())
            .collect(),
    }
}

/// Of `new`, those neither on the list (`existing`) nor removed — a find overlapping one by half (IoU) is it.
pub fn fresh(
    new: Vec<Person>,
    existing: &[Person],
    removals: &[edit_model::Removal],
) -> Vec<Person> {
    not_removed(new, removals)
        .into_iter()
        .filter(|p| !existing.iter().any(|e| iou(e.bbox, p.bbox) > SAME_PERSON))
        .collect()
}

/// A normalised box turned `turns` clockwise quarter turns — its corners turned, put back in order.
fn turn_box(b: [f32; 4], turns: u8) -> [f32; 4] {
    let a = crate::ai_worker::turn_point([b[0], b[1]], turns);
    let c = crate::ai_worker::turn_point([b[2], b[3]], turns);
    [
        a[0].min(c[0]),
        a[1].min(c[1]),
        a[0].max(c[0]),
        a[1].max(c[1]),
    ]
}

/// `p` in decode space: the photo was turned `turns` clockwise quarter turns to stand upright, so every point
/// goes back the rest of the circle — and each texel of the outline asks the upright mask where it turns to.
pub fn to_decode(p: UprightPerson, turns: u8) -> Person {
    let back = (4 - turns % 4) % 4;
    let [x0, y0, x1, y1] = p.bbox;
    let (gx, gy) = (AREA_MARGIN * (x1 - x0), AREA_MARGIN * (y1 - y0));
    let rect = turn_box(
        [
            (x0 - gx).max(0.0),
            (y0 - gy).max(0.0),
            (x1 + gx).min(1.0),
            (y1 + gy).min(1.0),
        ],
        back,
    );
    let s = AREA_SIDE;
    let alpha: Vec<f32> = (0..s * s)
        .map(|i| {
            let (u, v) = (
                ((i % s) as f32 + 0.5) / s as f32,
                ((i / s) as f32 + 0.5) / s as f32,
            );
            let d = [
                rect[0] + u * (rect[2] - rect[0]),
                rect[1] + v * (rect[3] - rect[1]),
            ];
            let up = crate::ai_worker::turn_point(d, turns % 4);
            p.mask.at(up[0], up[1])
        })
        .collect();
    let outline = crate::contour::loops(&alpha, s, s, 0.5)
        .into_iter()
        .map(|l| {
            l.into_iter()
                .map(|[u, v]| {
                    [
                        rect[0] + u / s as f32 * (rect[2] - rect[0]),
                        rect[1] + v / s as f32 * (rect[3] - rect[1]),
                    ]
                })
                .collect()
        })
        .collect();
    Person {
        bbox: turn_box(p.bbox, back),
        score: p.score,
        tall: y1 - y0,
        area: Arc::new(Area {
            rect,
            side: s,
            alpha,
        }),
        outline,
    }
}

/// Everyone but the photo's subjects ([`SUBJECT`]).
pub fn pre_ticked(people: &[Person]) -> Vec<bool> {
    people.iter().map(|p| p.tall < SUBJECT).collect()
}

/// One removal per ticked person: their box, no stroke, the full Opacity.
pub fn removals(people: &[Person], ticked: &[bool]) -> Vec<edit_model::Removal> {
    people
        .iter()
        .zip(ticked)
        .filter(|(_, &t)| t)
        .map(|(p, _)| edit_model::Removal {
            points: Vec::new(),
            radius: 0.0,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: Some(p.bbox),
        })
        .collect()
}

/// The person a click at decode point `at` picks: the smallest box holding it — one standing in front of
/// another is the smaller.
pub fn hit(people: &[Person], at: [f32; 2]) -> Option<usize> {
    people
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            (p.bbox[0]..=p.bbox[2]).contains(&at[0]) && (p.bbox[1]..=p.bbox[3]).contains(&at[1])
        })
        .min_by(|(_, a), (_, b)| {
            let size = |p: &Person| (p.bbox[2] - p.bbox[0]) * (p.bbox[3] - p.bbox[1]);
            size(a).total_cmp(&size(b))
        })
        .map(|(i, _)| i)
}

/// A find overlapping a person's removal this much (IoU) is that person again.
const SAME_PERSON: f32 = 0.5;

fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let i = area([
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]);
    i / (area(a) + area(b) - i).max(1e-9)
}

/// The people `removals` has not removed yet — the search reads the decode, where removed people still stand.
pub fn not_removed(people: Vec<Person>, removals: &[edit_model::Removal]) -> Vec<Person> {
    people
        .into_iter()
        .filter(|p| {
            !removals
                .iter()
                .filter_map(|r| r.person)
                .any(|b| iou(b, p.bbox) > SAME_PERSON)
        })
        .collect()
}

/// What the panel says of `n` people found, `ticked` of them to go.
pub fn summary(n: usize, ticked: usize) -> String {
    match n {
        0 => "No people found".to_string(),
        1 => format!("1 person · {ticked} to remove"),
        n => format!("{n} people · {ticked} to remove"),
    }
}

/// The button that removes them.
pub fn remove_label(ticked: usize) -> String {
    match ticked {
        1 => "Remove 1 person".to_string(),
        n => format!("Remove {n} people"),
    }
}

/// A search on its own thread, so the window keeps drawing while it looks — Remove dust's shape.
pub struct Search {
    /// How far it has got, as `f32` bits.
    progress: Arc<AtomicU32>,
    found: Receiver<Result<Vec<Person>, String>>,
}

impl Search {
    /// Starts finding the people in `img` — the photo turned upright by `turns` — with `finder`.
    pub fn spawn(img: Arc<image::RgbImage>, turns: u8, finder: Finder) -> Self {
        Self::run(img, turns, finder, None)
    }

    /// Starts looking again in `img` — the area `rect` of the photo (decode-normalised), turned upright by `turns`
    /// — where the author drew `drawn` (normalised over `img`, upright).
    pub fn spawn_there(
        img: Arc<image::RgbImage>,
        turns: u8,
        rect: [f32; 4],
        drawn: [f32; 4],
        finder: ThereFinder,
    ) -> Self {
        Self::run(
            img,
            turns,
            Box::new(move |img, progress| finder(img, drawn, progress)),
            Some(rect),
        )
    }

    /// The thread: the finder over `img`, each answer back in decode orientation, then into `rect` when it is an
    /// area of the photo.
    fn run(img: Arc<image::RgbImage>, turns: u8, finder: Finder, rect: Option<[f32; 4]>) -> Self {
        let (search, tx) = Self::answered_by(0.0);
        let progress = Arc::clone(&search.progress);
        std::thread::Builder::new()
            .name("rustroom-people".into())
            .spawn(move || {
                let store = |p: f32| progress.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                let found = finder(&img, &store).map(|people| {
                    people
                        .into_iter()
                        .map(|p| to_decode(p, turns))
                        .map(|p| match rect {
                            Some(r) => place(p, r),
                            None => p,
                        })
                        .collect()
                });
                store(1.0);
                let _ = tx.send(found);
            })
            .expect("the people thread starts");
        search
    }

    /// A search whose answer arrives through the sender, standing at `progress` — the panel's tests hold one
    /// open to see the window while it looks.
    pub fn answered_by(progress: f32) -> (Self, Sender<Result<Vec<Person>, String>>) {
        let (tx, found) = std::sync::mpsc::channel();
        let progress = Arc::new(AtomicU32::new(progress.to_bits()));
        (Self { progress, found }, tx)
    }

    /// How far it has got, 0..=1.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    /// The people, once they are there — handed over once.
    pub fn poll(&self) -> Option<Result<Vec<Person>, String>> {
        self.found.try_recv().ok()
    }
}

/// What a press of Find people waits for before the search can start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The full decode — not the stand-in shown while the photo loads, never a ghost session.
    Photo,
    /// YOLOX-m and MobileSAM, offered in the download dialog.
    Models,
}

/// What makes the finders — the real networks; a test hands in its own.
#[derive(Clone, Copy)]
pub struct Finders {
    pub all: fn(PathBuf) -> Finder,
    pub there: fn(PathBuf) -> ThereFinder,
}

impl Default for Finders {
    fn default() -> Self {
        Self {
            all: real_finder,
            there: real_there_finder,
        }
    }
}

/// Find people over one photo — it lives on the session, so another photo starts clean.
#[derive(Default)]
pub struct Run {
    /// Find people pressed this frame — the flow starts the search, or waits.
    pub pressed: bool,
    /// The people view is on — from a search's landing, found or not, until Remove, Cancel, another press or
    /// another photo. While it is, the canvas picks people and a drawing means "look there".
    pub active: bool,
    /// A drawing made in the people view, decode-normalised, for the flow to look again under (the author's (a)).
    pub look: Option<Vec<[f32; 2]>>,
    /// The running search is a look there — its people join the list rather than make it.
    pub there: bool,
    pub finders: Finders,
    pub waiting: Option<Wait>,
    pub search: Option<Search>,
    /// The people found, shown on the photo until Remove, Cancel, another press or another photo.
    pub people: Vec<Person>,
    /// Which of them go — one per person.
    pub ticked: Vec<bool>,
    /// What the last search ended with when it left nobody to show: "No people found", or why it failed.
    pub said: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encloses(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
        let mut c = false;
        for (i, a) in poly.iter().enumerate() {
            let b = poly[(i + 1) % poly.len()];
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                c = !c;
            }
        }
        c
    }

    fn empty_area() -> Area {
        Area {
            rect: [0.0; 4],
            side: AREA_SIDE,
            alpha: vec![0.0; AREA_SIDE * AREA_SIDE],
        }
    }

    /// One person as a finder answers: `bbox` upright, their outline a small square at `mask_at`.
    fn upright(bbox: [f32; 4], mask_at: [f32; 2]) -> UprightPerson {
        let mask = ai_mask::ProbMap::from_fn(256, 256, |x, y| {
            let (u, v) = ((x as f32 + 0.5) / 256.0, (y as f32 + 0.5) / 256.0);
            ((u - mask_at[0]).abs() < 0.03 && (v - mask_at[1]).abs() < 0.03) as u8 as f32
        });
        UprightPerson {
            bbox,
            score: 0.9,
            mask,
        }
    }

    /// Review Focus 1: for every turn, a person's box and outline land on them in the decode — a portrait's
    /// people where they stand, not turned away.
    #[test]
    fn a_find_lands_on_its_person_for_every_turn() {
        for turns in 0..4u8 {
            // Upright, the person stands at x 0.20–0.30, y 0.40–0.90; their outline's middle at (0.25, 0.65).
            let p = to_decode(upright([0.20, 0.40, 0.30, 0.90], [0.25, 0.65]), turns);
            // Where the upright point sits in the decode: turned back the rest of the circle.
            let back = (4 - turns) % 4;
            let d = crate::ai_worker::turn_point([0.25, 0.65], back);
            assert!(
                p.bbox[0] < d[0] && d[0] < p.bbox[2] && p.bbox[1] < d[1] && d[1] < p.bbox[3],
                "turns {turns}: the box {:?} misses {d:?}",
                p.bbox
            );
            // The box's own size turned: a tall person upright is wide in a decode turned a quarter.
            let (bw, bh) = (p.bbox[2] - p.bbox[0], p.bbox[3] - p.bbox[1]);
            let (want_w, want_h) = if turns % 2 == 0 {
                (0.1, 0.5)
            } else {
                (0.5, 0.1)
            };
            assert!(
                (bw - want_w).abs() < 1e-4 && (bh - want_h).abs() < 1e-4,
                "turns {turns}: {:?}",
                p.bbox
            );
            let a = &p.area;
            let at = |q: [f32; 2]| {
                let (u, v) = (
                    (q[0] - a.rect[0]) / (a.rect[2] - a.rect[0]),
                    (q[1] - a.rect[1]) / (a.rect[3] - a.rect[1]),
                );
                let (x, y) = (
                    ((u * AREA_SIDE as f32) as usize).min(AREA_SIDE - 1),
                    ((v * AREA_SIDE as f32) as usize).min(AREA_SIDE - 1),
                );
                a.alpha[y * AREA_SIDE + x]
            };
            assert!(
                at(d) > 0.5,
                "turns {turns}: the outline is under the person"
            );
            let off = crate::ai_worker::turn_point([0.22, 0.45], back);
            assert!(at(off) < 0.5, "turns {turns}: and only there");
            assert!((p.tall - 0.5).abs() < 1e-5, "the height is the upright one");
            // The border follows the outline (the author, 2026-10-21): one loop, around the person.
            assert_eq!(p.outline.len(), 1, "turns {turns}: {:?}", p.outline.len());
            assert!(
                encloses(&p.outline[0], d),
                "turns {turns}: the border goes round them"
            );
            assert!(
                !encloses(&p.outline[0], off),
                "turns {turns}: and only them"
            );
        }
    }

    #[test]
    fn people_shorter_than_a_third_of_the_frame_start_ticked() {
        let mk = |tall: f32| Person {
            bbox: [0.0; 4],
            score: 0.9,
            tall,
            area: Arc::new(empty_area()),
            outline: Vec::new(),
        };
        assert_eq!(
            pre_ticked(&[mk(0.62), mk(0.12), mk(0.349), mk(0.35)]),
            vec![false, true, true, false]
        );
    }

    #[test]
    fn each_ticked_person_becomes_a_removal_of_their_box() {
        let mk = |x: f32| Person {
            bbox: [x, 0.2, x + 0.1, 0.6],
            score: 0.9,
            tall: 0.4,
            area: Arc::new(empty_area()),
            outline: Vec::new(),
        };
        let rs = removals(&[mk(0.1), mk(0.4), mk(0.7)], &[true, false, true]);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].person, Some([0.1, 0.2, 0.2, 0.6]));
        assert_eq!(rs[1].person, Some([0.7, 0.2, 0.8, 0.6]));
        assert!(rs
            .iter()
            .all(|r| r.points.is_empty() && r.opacity == 100.0 && !r.detect && r.variant == 0));
    }

    #[test]
    fn the_search_reports_its_progress_and_its_people() {
        let img = Arc::new(image::RgbImage::new(200, 100));
        let finder: Finder = Box::new(|_, progress| {
            progress(0.5);
            Ok(vec![upright([0.1, 0.1, 0.2, 0.5], [0.15, 0.3])])
        });
        let s = Search::spawn(img, 1, finder);
        let start = std::time::Instant::now();
        let got = loop {
            if let Some(r) = s.poll() {
                break r;
            }
            assert!(start.elapsed().as_secs() < 10, "the search never landed");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let people = got.expect("found");
        assert_eq!(people.len(), 1);
        // Turned back from the upright image: the decode was turned once to stand it up.
        let want = to_decode(upright([0.1, 0.1, 0.2, 0.5], [0.15, 0.3]), 1).bbox;
        assert_eq!(people[0].bbox, want);
        assert_eq!(s.progress(), 1.0);
        assert!(s.poll().is_none(), "handed over once");
    }

    #[test]
    fn a_search_that_fails_says_why() {
        let img = Arc::new(image::RgbImage::new(20, 10));
        let s = Search::spawn(img, 0, Box::new(|_, _| Err("no model".to_string())));
        let start = std::time::Instant::now();
        let got = loop {
            if let Some(r) = s.poll() {
                break r;
            }
            assert!(start.elapsed().as_secs() < 10);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(got.err().as_deref(), Some("no model"));
    }

    /// A click on the photo picks the person under it — the smallest box holding it, so one standing in front of
    /// another can be picked.
    #[test]
    fn a_click_picks_the_smallest_person_under_it() {
        let mk = |b: [f32; 4]| Person {
            bbox: b,
            score: 0.9,
            tall: 0.4,
            area: Arc::new(empty_area()),
            outline: Vec::new(),
        };
        let people = [mk([0.1, 0.1, 0.6, 0.9]), mk([0.3, 0.4, 0.4, 0.8])];
        assert_eq!(hit(&people, [0.35, 0.5]), Some(1));
        assert_eq!(hit(&people, [0.15, 0.5]), Some(0));
        assert_eq!(hit(&people, [0.8, 0.5]), None);
    }

    /// Final review: the search reads the decode, where people already removed still stand — a find on a person's
    /// removal is them again, and is not offered twice. A stroke removal is not a person.
    #[test]
    fn people_already_removed_are_not_found_again() {
        let mk = |b: [f32; 4]| Person {
            bbox: b,
            score: 0.9,
            tall: 0.4,
            area: Arc::new(empty_area()),
            outline: Vec::new(),
        };
        let removed = edit_model::Removal {
            points: Vec::new(),
            radius: 0.0,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: Some([0.10, 0.10, 0.20, 0.50]),
        };
        let stroke = edit_model::Removal {
            points: vec![[0.75, 0.3]],
            radius: 0.05,
            person: None,
            ..removed.clone()
        };
        let left = not_removed(
            vec![
                mk([0.11, 0.11, 0.21, 0.52]),
                mk([0.40, 0.10, 0.50, 0.50]),
                mk([0.70, 0.10, 0.80, 0.50]),
            ],
            &[removed, stroke],
        );
        let boxes: Vec<[f32; 4]> = left.iter().map(|p| p.bbox).collect();
        assert_eq!(
            boxes,
            vec![[0.40, 0.10, 0.50, 0.50], [0.70, 0.10, 0.80, 0.50]]
        );
    }

    fn near(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// Look there: the drawing's box, its brush included, grown by half its size on each side — and at least the
    /// network's 640² square of decode pixels — inside the photo.
    #[test]
    fn the_area_looked_at_is_the_drawing_grown() {
        let (w, h) = (6000, 4000);
        // A stroke 300 decode px wide and 1000 tall, its brush 0.
        let r = look_rect(&[[0.475, 0.375], [0.525, 0.625]], 0.0, w, h);
        let (rw, rh) = ((r[2] - r[0]) * w as f32, (r[3] - r[1]) * h as f32);
        assert!((rh - 2000.0).abs() < 2.0, "grown by half a side: {rh}");
        assert!(
            (rw - 640.0).abs() < 2.0,
            "at least the network's square: {rw}"
        );
        assert!(
            ((r[0] + r[2]) / 2.0 - 0.5).abs() < 1e-4,
            "centred on the drawing: {r:?}"
        );
        // The brush counts: a single point with a radius of 0.05 of the long side is 600 px across.
        let b = look_rect(&[[0.5, 0.5]], 0.05, w, h);
        assert!(((b[2] - b[0]) * w as f32 - 1200.0).abs() < 2.0, "{b:?}");
        // At the corner: inside the photo, still the square's size.
        let e = look_rect(&[[0.01, 0.01]], 0.0, w, h);
        assert_eq!([e[0], e[1]], [0.0, 0.0], "{e:?}");
        assert!(((e[2] - e[0]) * w as f32 - 640.0).abs() < 2.0, "{e:?}");
    }

    /// A person found in the area is placed back in the photo — box, shading and border.
    #[test]
    fn a_person_found_there_is_placed_in_the_photo() {
        let p = Person {
            bbox: [0.5, 0.5, 1.0, 1.0],
            score: 0.9,
            tall: 0.5,
            area: Arc::new(Area {
                rect: [0.0, 0.0, 1.0, 1.0],
                ..empty_area()
            }),
            outline: vec![vec![[0.5, 0.5], [1.0, 0.5], [1.0, 1.0]]],
        };
        let q = place(p, [0.2, 0.4, 0.6, 0.8]);
        assert!(near(q.bbox, [0.4, 0.6, 0.6, 0.8]), "{:?}", q.bbox);
        assert!(near(q.area.rect, [0.2, 0.4, 0.6, 0.8]), "{:?}", q.area.rect);
        let o = q.outline[0][0];
        assert!(
            (o[0] - 0.4).abs() < 1e-5 && (o[1] - 0.6).abs() < 1e-5,
            "{o:?}"
        );
    }

    /// (a), the author's pick: people found there are each outlined; nobody found, what is under the drawing is
    /// outlined from its box and offered — its box the outline's own extent; an empty answer, nobody.
    #[test]
    fn nobody_found_there_takes_what_is_under_the_drawing() {
        // MobileSAM's stand-in: the middle half of the box it is asked about.
        let half = |b: [f32; 4]| {
            ai_mask::ProbMap::from_fn(100, 100, move |x, y| {
                let (u, v) = ((x as f32 + 0.5) / 100.0, (y as f32 + 0.5) / 100.0);
                let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                ((u - cx).abs() < (b[2] - b[0]) / 4.0 && (v - cy).abs() < (b[3] - b[1]) / 4.0) as u8
                    as f32
            })
        };
        let drawn = [0.5, 0.2, 0.9, 0.9];
        let mut asked = Vec::new();
        let found = vec![ai_mask::people::Found {
            bbox: [0.1, 0.1, 0.3, 0.6],
            score: 0.8,
        }];
        let got = there_people(found, drawn, &mut |b| {
            asked.push(b);
            Ok(half(b))
        })
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].bbox, [0.1, 0.1, 0.3, 0.6], "the network's find");
        assert_eq!(
            asked,
            vec![[0.1, 0.1, 0.3, 0.6]],
            "outlined from its own box"
        );
        asked.clear();
        let got = there_people(Vec::new(), drawn, &mut |b| {
            asked.push(b);
            Ok(half(b))
        })
        .unwrap();
        assert_eq!(asked, vec![drawn], "nobody found: the drawing's box asked");
        assert_eq!(got.len(), 1, "what is under the drawing offered");
        let b = got[0].bbox;
        assert!(
            (b[0] - 0.6).abs() < 0.02 && (b[2] - 0.8).abs() < 0.02,
            "its box the outline's extent: {b:?}"
        );
        let none = there_people(Vec::new(), drawn, &mut |_| {
            Ok(ai_mask::ProbMap::from_fn(8, 8, |_, _| 0.0))
        })
        .unwrap();
        assert!(none.is_empty(), "an empty outline is nobody");
    }

    /// Only people neither on the list nor removed join it.
    #[test]
    fn only_new_people_join_the_list() {
        let mk = |x: f32| Person {
            bbox: [x, 0.1, x + 0.1, 0.5],
            score: 0.9,
            tall: 0.4,
            area: Arc::new(empty_area()),
            outline: Vec::new(),
        };
        let removed = edit_model::Removal {
            points: Vec::new(),
            radius: 0.0,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: Some([0.4, 0.1, 0.5, 0.5]),
        };
        let new = fresh(vec![mk(0.11), mk(0.41), mk(0.7)], &[mk(0.1)], &[removed]);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].bbox[0], 0.7);
    }

    /// A search there hands the finder the upright area and the drawing's box, and lands in the photo.
    #[test]
    fn a_search_there_lands_in_the_photo() {
        let img = Arc::new(image::RgbImage::new(100, 100));
        let finder: ThereFinder = Box::new(|img, drawn, _| {
            assert_eq!((img.width(), img.height()), (100, 100));
            Ok(vec![upright(
                drawn,
                [(drawn[0] + drawn[2]) / 2.0, (drawn[1] + drawn[3]) / 2.0],
            )])
        });
        let s = Search::spawn_there(img, 0, [0.5, 0.5, 1.0, 1.0], [0.0, 0.0, 0.5, 0.5], finder);
        let start = std::time::Instant::now();
        let got = loop {
            if let Some(r) = s.poll() {
                break r;
            }
            assert!(start.elapsed().as_secs() < 10);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let p = got.unwrap();
        assert_eq!(p.len(), 1);
        assert!(near(p[0].bbox, [0.5, 0.5, 0.75, 0.75]), "{:?}", p[0].bbox);
    }

    #[test]
    fn the_summary_counts_people_and_picks() {
        assert_eq!(summary(0, 0), "No people found");
        assert_eq!(summary(1, 1), "1 person · 1 to remove");
        assert_eq!(summary(12, 9), "12 people · 9 to remove");
        assert_eq!(remove_label(1), "Remove 1 person");
        assert_eq!(remove_label(9), "Remove 9 people");
    }
}

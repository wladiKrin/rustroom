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
    let alpha = (0..s * s)
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
    Person {
        bbox: turn_box(p.bbox, back),
        score: p.score,
        tall: y1 - y0,
        area: Arc::new(Area {
            rect,
            side: s,
            alpha,
        }),
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
        let (search, tx) = Self::answered_by(0.0);
        let progress = Arc::clone(&search.progress);
        std::thread::Builder::new()
            .name("rustroom-people".into())
            .spawn(move || {
                let store = |p: f32| progress.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                let found = finder(&img, &store)
                    .map(|people| people.into_iter().map(|p| to_decode(p, turns)).collect());
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

/// Find people over one photo — it lives on the session, so another photo starts clean.
#[derive(Default)]
pub struct Run {
    /// Find people pressed this frame — the flow starts the search, or waits.
    pub pressed: bool,
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
        }
    }

    #[test]
    fn people_shorter_than_a_third_of_the_frame_start_ticked() {
        let mk = |tall: f32| Person {
            bbox: [0.0; 4],
            score: 0.9,
            tall,
            area: Arc::new(empty_area()),
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
        };
        let people = [mk([0.1, 0.1, 0.6, 0.9]), mk([0.3, 0.4, 0.4, 0.8])];
        assert_eq!(hit(&people, [0.35, 0.5]), Some(1));
        assert_eq!(hit(&people, [0.15, 0.5]), Some(0));
        assert_eq!(hit(&people, [0.8, 0.5]), None);
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

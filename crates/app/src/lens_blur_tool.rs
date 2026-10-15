//! Lens Blur's focus (M10.16d): which depth is sharp, read from the cached maps — and what the section
//! says while they compute.
//!
//! ⚠ No egui: the section and the click are the bin's (`develop::panel`, `develop::canvas`); the rule
//! is here, on plain types — the `depth_tool` pattern.

use crate::ai_coverage::{AiMaps, MapQuery};
use ai_mask::ProbMap;
use edit_model::{LensBlur, LensFocus};

/// The maps Lens Blur reads while it is on: the depth, and in Subject mode the Subject map — nothing
/// while it is off, whatever its other settings say.
pub fn queries(lb: &LensBlur) -> Vec<MapQuery> {
    if !lb.on {
        return Vec::new();
    }
    match lb.focus {
        LensFocus::Subject => vec![
            MapQuery::Depth,
            MapQuery::Prompt(edit_model::AiPrompt::Subject),
        ],
        LensFocus::Point { .. } => vec![MapQuery::Depth],
    }
}

/// The focus depth, 0 nearest … 1 farthest, read in **decode** space — where both maps and a Point
/// live. Subject → the median depth where the subject's probability is over 0.5, `None` when it is
/// nowhere (the caller falls back to the centre); Point → the median of the 3 × 3 map pixels around
/// the point, so one stray pixel cannot set the focus.
pub fn focus_depth(lb: &LensBlur, depth: &ProbMap, subject: Option<&ProbMap>) -> Option<f32> {
    match lb.focus {
        LensFocus::Point { point } => Some(depth_near(depth, point)),
        LensFocus::Subject => subject_median(depth, subject?),
    }
}

/// The median depth under the subject — sampled at each of the Subject map's own pixels over 0.5.
fn subject_median(depth: &ProbMap, subject: &ProbMap) -> Option<f32> {
    let (w, h) = (subject.w as usize, subject.h as usize);
    let mut under: Vec<f32> = subject
        .data
        .iter()
        .enumerate()
        .filter(|(_, &p)| p > 127)
        .map(|(i, _)| {
            let (x, y) = (i % w, i / w);
            depth.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
        })
        .collect();
    if under.is_empty() {
        return None;
    }
    let mid = under.len() / 2;
    Some(*under.select_nth_unstable_by(mid, f32::total_cmp).1)
}

/// Subject mode's focus for the maps `ids` names — the median under the subject, or with no subject
/// in the map the depth at the decode centre (the section says so).
///
/// ⚠⚠ **Memoised on the two maps' content ids**: the render builds its prefix steps every frame and
/// asks for the focus each time, and the median reads every subject pixel — milliseconds a frame for
/// an answer that changes only when a map does.
pub fn subject_focus(ids: (u64, u64), depth: &ProbMap, subject: &ProbMap) -> f32 {
    type Memo = Vec<((u64, u64), f32)>;
    static MEMO: std::sync::Mutex<Memo> = std::sync::Mutex::new(Vec::new());
    if let Some(&(_, f)) = MEMO
        .lock()
        .expect("subject focus memo")
        .iter()
        .find(|(k, _)| *k == ids)
    {
        return f;
    }
    let f = subject_median(depth, subject).unwrap_or_else(|| depth_near(depth, [0.5, 0.5]));
    let mut memo = MEMO.lock().expect("subject focus memo");
    memo.insert(0, (ids, f));
    memo.truncate(8);
    f
}

/// The median depth of the 3 × 3 map **pixels** around `point` (decode-normalised) — snapped to
/// pixels, never interpolated, so the answer is a depth something in the photo has: at a boundary a
/// blend would focus on the gap between two objects.
pub fn depth_near(depth: &ProbMap, point: [f32; 2]) -> f32 {
    if depth.w == 0 || depth.h == 0 {
        return 0.0;
    }
    let (w, h) = (depth.w as i64, depth.h as i64);
    // ⚠ A non-finite coordinate reads as 0 — `as` saturates, and NaN goes to 0.
    let px = |v: f32, n: i64| ((v.clamp(0.0, 1.0) * n as f32) as i64).min(n - 1);
    let (cx, cy) = (px(point[0], w), px(point[1], h));
    let mut patch: Vec<u8> = (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| (cx + dx, cy + dy)))
        .filter(|&(x, y)| (0..w).contains(&x) && (0..h).contains(&y))
        .map(|(x, y)| depth.data[(y * w + x) as usize])
        .collect();
    patch.sort_unstable();
    patch[patch.len() / 2] as f32 / 255.0
}

/// Whether the Subject map found a subject — any probability over 0.5.
pub fn subject_found(subject: &ProbMap) -> bool {
    subject.data.iter().any(|&p| p > 127)
}

/// What the section says while a map it needs computes — the depth first, then (Subject mode) the
/// subject — or `None` when it has them or Lens Blur is off.
pub fn waiting(lb: &LensBlur, maps: &AiMaps) -> Option<&'static str> {
    queries(lb)
        .into_iter()
        .find(|q| maps.get(q).is_none())
        .map(|q| crate::ai_coverage::waiting_for(&q))
}

/// What the section says when Subject mode found nobody — and focused at the centre instead.
pub const NO_SUBJECT: &str = "No subject found — focusing at the centre";

/// The section's one line: a map computing ([`waiting`]) or, in Subject mode, no subject in its map —
/// `None` when there is nothing to say.
pub fn status(lb: &LensBlur, maps: &AiMaps) -> Option<&'static str> {
    if let Some(w) = waiting(lb, maps) {
        return Some(w);
    }
    let subject = maps.get(&MapQuery::Prompt(edit_model::AiPrompt::Subject))?;
    (lb.on && lb.focus == LensFocus::Subject && !subject_found(subject)).then_some(NO_SUBJECT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 100 × 50: depth 0.2 on the left half, 0.8 on the right.
    fn halves() -> ProbMap {
        ProbMap::from_fn(100, 50, |x, _| if x < 50 { 0.2 } else { 0.8 })
    }

    /// A subject over the columns `x0..x1`.
    fn subject(x0: u32, x1: u32) -> ProbMap {
        ProbMap::from_fn(
            100,
            50,
            |x, _| if (x0..x1).contains(&x) { 0.9 } else { 0.1 },
        )
    }

    fn on() -> LensBlur {
        LensBlur {
            on: true,
            ..LensBlur::default()
        }
    }

    fn point(p: [f32; 2]) -> LensBlur {
        LensBlur {
            focus: LensFocus::Point { point: p },
            ..on()
        }
    }

    /// The median under the subject — the side most of it stands on, whichever that is.
    #[test]
    fn subject_focus_is_the_median_depth_under_the_subject() {
        let d = halves();
        let near = focus_depth(&on(), &d, Some(&subject(30, 60))).unwrap();
        assert!((near - 0.2).abs() < 0.01, "{near}: mostly on the left");
        let far = focus_depth(&on(), &d, Some(&subject(40, 70))).unwrap();
        assert!((far - 0.8).abs() < 0.01, "{far}: mostly on the right");
    }

    #[test]
    fn no_subject_no_subject_focus() {
        let nothing = ProbMap::from_fn(100, 50, |_, _| 0.3);
        assert!(!subject_found(&nothing));
        assert_eq!(focus_depth(&on(), &halves(), Some(&nothing)), None);
        assert!(subject_found(&subject(10, 12)), "positive control");
        assert_eq!(
            focus_depth(&on(), &halves(), None),
            None,
            "Subject mode with no map at all"
        );
    }

    /// Point → the 3 × 3 median: a single stray map pixel under the point does not move it.
    #[test]
    fn point_focus_is_the_three_by_three_median() {
        let stray = ProbMap::from_fn(100, 50, |x, y| if (x, y) == (75, 25) { 0.0 } else { 0.8 });
        let at = [75.5 / 100.0, 25.5 / 50.0];
        let f = focus_depth(&point(at), &stray, None).unwrap();
        assert!((f - 0.8).abs() < 0.01, "{f}");
        let left = focus_depth(&point([0.1, 0.5]), &halves(), None).unwrap();
        assert!((left - 0.2).abs() < 0.01, "{left}");
        assert!(
            (depth_near(&stray, at) - 0.8).abs() < 0.01,
            "the same rule, exposed for the centre fallback"
        );
    }

    /// Waiting first; then, in Subject mode, an empty Subject map says the focus fell back.
    #[test]
    fn the_section_says_when_there_is_no_subject() {
        let mut maps = AiMaps::default();
        assert_eq!(status(&on(), &maps), Some("Estimating depth…"));
        maps.insert(MapQuery::Depth, Arc::new(halves()));
        maps.insert(
            MapQuery::Prompt(edit_model::AiPrompt::Subject),
            Arc::new(ProbMap::from_fn(100, 50, |_, _| 0.1)),
        );
        assert_eq!(status(&on(), &maps), Some(NO_SUBJECT));
        assert_eq!(
            status(&point([0.5, 0.5]), &maps),
            None,
            "Point reads no subject"
        );
        maps.insert(
            MapQuery::Prompt(edit_model::AiPrompt::Subject),
            Arc::new(subject(10, 20)),
        );
        assert_eq!(status(&on(), &maps), None);
    }

    /// The depth first — nothing blurs without it — then the subject, in Subject mode only.
    #[test]
    fn waiting_names_the_depth_first_then_the_subject() {
        let mut maps = AiMaps::default();
        assert_eq!(waiting(&on(), &maps), Some("Estimating depth…"));
        assert_eq!(
            waiting(&point([0.5, 0.5]), &maps),
            Some("Estimating depth…")
        );
        maps.insert(MapQuery::Depth, Arc::new(halves()));
        assert_eq!(waiting(&on(), &maps), Some("Finding the subject…"));
        assert_eq!(
            waiting(&point([0.5, 0.5]), &maps),
            None,
            "Point needs no subject"
        );
        maps.insert(
            MapQuery::Prompt(edit_model::AiPrompt::Subject),
            Arc::new(subject(10, 20)),
        );
        assert_eq!(waiting(&on(), &maps), None);
        let off = LensBlur {
            on: false,
            ..LensBlur::default()
        };
        assert_eq!(
            waiting(&off, &AiMaps::default()),
            None,
            "off waits for nothing"
        );
    }
}

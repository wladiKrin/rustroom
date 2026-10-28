//! Which of the Library's photos get a pin, and where (M10.18b). No egui.
//!
//! ⚠ A RAW+JPEG pair is ONE grid entry, so it is one pin — at its own position, or else at its pair
//! half's: the phone-tagged JPEG of a pair whose RAW carries no GPS still places the pair. Halves are
//! matched by folder and upper-cased stem, the stacking rule's. ⚠⚠ A location REMOVED by hand is final:
//! the sibling's position never brings back a pin the user took away.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The catalog's locations, indexed for [`pins`] — built once per catalog change, not per view.
#[derive(Debug, Default)]
pub struct Located {
    at: HashMap<PathBuf, (f64, f64)>,
    by_stem: HashMap<(PathBuf, String), (f64, f64)>,
    removed: HashSet<PathBuf>,
}

impl Located {
    /// From `Catalog::all_locations_and_removals`.
    pub fn new(at: HashMap<PathBuf, (f64, f64)>, removed: HashSet<PathBuf>) -> Self {
        let by_stem = at
            .iter()
            .filter_map(|(p, a)| Some((stem_key(p)?, *a)))
            .collect();
        Self {
            at,
            by_stem,
            removed,
        }
    }

    /// Where `path`'s entry goes: its own position, else its pair half's — never for a removed one.
    pub fn of(&self, path: &Path) -> Option<(f64, f64)> {
        if let Some(a) = self.at.get(path) {
            return Some(*a);
        }
        if self.removed.contains(path) {
            return None;
        }
        stem_key(path).and_then(|k| self.by_stem.get(&k).copied())
    }
}

/// The pins for one view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pins {
    /// `(entry index, (lat, lon))` for every located visible entry, in visible order.
    pub points: Vec<(usize, (f64, f64))>,
    /// Visible entries without a location, in visible order — the strip's "No location".
    pub unplaced: Vec<usize>,
}

fn stem_key(p: &Path) -> Option<(PathBuf, String)> {
    Some((
        p.parent()?.to_path_buf(),
        p.file_stem()?.to_string_lossy().to_uppercase(),
    ))
}

/// The pins for `visible` (`(entry index, path)`).
pub fn pins<'a>(visible: impl Iterator<Item = (usize, &'a Path)>, located: &Located) -> Pins {
    let mut out = Pins::default();
    for (i, path) in visible {
        match located.of(path) {
            Some(at) => out.points.push((i, at)),
            None => out.unplaced.push(i),
        }
    }
    out
}

/// The status row's sentence for the Map view.
pub fn status(p: &Pins) -> String {
    let total = p.points.len() + p.unplaced.len();
    let photos = crate::status_text::photos(total);
    format!("{} of {photos} have a location", p.points.len())
}

/// Which of `n` cells of `cell` points (gap included) a strip scrolled to `offset` and `width` wide
/// shows, with one cell of margin each side — so the strip lays out and asks thumbnails for those only.
pub fn visible_cells(n: usize, cell: f32, offset: f32, width: f32) -> Range<usize> {
    if n == 0 || cell <= 0.0 {
        return 0..0;
    }
    let first = ((offset / cell).floor() as isize - 1).max(0) as usize;
    let last = (((offset + width) / cell).ceil() as usize + 1).min(n);
    first.min(last)..last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn located(at: &[(&str, (f64, f64))], removed: &[&str]) -> Located {
        Located::new(
            at.iter().map(|(p, a)| (path(p), *a)).collect(),
            removed.iter().map(|p| path(p)).collect(),
        )
    }

    /// Review focus 5: the pair's located half places the pair.
    #[test]
    fn a_pair_is_one_pin_at_its_located_half() {
        let l = located(&[("/trip/DSCF0001.jpg", (38.7, -9.1))], &[]);
        let raf = path("/trip/DSCF0001.RAF");
        let other = path("/trip/DSCF0002.RAF");
        let elsewhere = path("/home/DSCF0001.RAF");
        let p = pins(
            [
                (0, raf.as_path()),
                (1, other.as_path()),
                (2, elsewhere.as_path()),
            ]
            .into_iter(),
            &l,
        );
        assert_eq!(p.points, vec![(0, (38.7, -9.1))]);
        assert_eq!(
            p.unplaced,
            vec![1, 2],
            "another folder's stem is not a pair"
        );
    }

    /// The review's #5: Remove location on a pair's entry is final, whatever its JPEG says.
    #[test]
    fn a_removed_pair_is_not_placed_by_its_other_half() {
        let l = located(
            &[("/trip/DSCF0001.JPG", (38.7, -9.1))],
            &["/trip/DSCF0001.RAF"],
        );
        let raf = path("/trip/DSCF0001.RAF");
        let p = pins([(0, raf.as_path())].into_iter(), &l);
        assert!(p.points.is_empty(), "{p:?}");
    }

    #[test]
    fn the_status_counts_located_photos() {
        let p = Pins {
            points: vec![(0, (1.0, 1.0)), (3, (2.0, 2.0))],
            unplaced: vec![1],
        };
        assert_eq!(status(&p), "2 of 3 photos have a location");
    }

    /// The review's #1: a strip of 10,000 lays out a screenful, not 10,000.
    #[test]
    fn the_strip_draws_only_the_cells_in_view() {
        assert_eq!(visible_cells(10_000, 70.0, 0.0, 700.0), 0..11);
        assert_eq!(visible_cells(10_000, 70.0, 7000.0, 700.0), 99..111);
        assert_eq!(visible_cells(10_000, 70.0, 699_000.0, 700.0), 9984..9997);
        assert_eq!(
            visible_cells(10_000, 70.0, 699_300.0, 700.0),
            9989..10_000,
            "the end"
        );
        assert_eq!(visible_cells(3, 70.0, 0.0, 700.0), 0..3);
        assert_eq!(visible_cells(0, 70.0, 0.0, 700.0), 0..0);
    }
}

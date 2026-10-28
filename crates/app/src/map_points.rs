//! Which of the Library's photos get a pin, and where (M10.18b). No egui.
//!
//! ⚠ A RAW+JPEG pair is ONE grid entry, so it is one pin — at its own position, or else at its pair
//! half's: the phone-tagged JPEG of a pair whose RAW carries no GPS still places the pair. Halves are
//! matched by folder and upper-cased stem, the stacking rule's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// The pins for `visible` (`(entry index, path)`), from the catalog's `located` photos.
pub fn pins<'a>(
    visible: impl Iterator<Item = (usize, &'a Path)>,
    located: &HashMap<PathBuf, (f64, f64)>,
) -> Pins {
    let by_stem: HashMap<(PathBuf, String), (f64, f64)> = located
        .iter()
        .filter_map(|(p, at)| Some((stem_key(p)?, *at)))
        .collect();
    let mut out = Pins::default();
    for (i, path) in visible {
        let at = located
            .get(path)
            .copied()
            .or_else(|| stem_key(path).and_then(|k| by_stem.get(&k).copied()));
        match at {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// Review focus 5: the pair's located half places the pair.
    #[test]
    fn a_pair_is_one_pin_at_its_located_half() {
        let located = HashMap::from([(path("/trip/DSCF0001.jpg"), (38.7, -9.1))]);
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
            &located,
        );
        assert_eq!(p.points, vec![(0, (38.7, -9.1))]);
        assert_eq!(
            p.unplaced,
            vec![1, 2],
            "another folder's stem is not a pair"
        );
    }

    #[test]
    fn the_status_counts_located_photos() {
        let p = Pins {
            points: vec![(0, (1.0, 1.0)), (3, (2.0, 2.0))],
            unplaced: vec![1],
        };
        assert_eq!(status(&p), "2 of 3 photos have a location");
    }
}

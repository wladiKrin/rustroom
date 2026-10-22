//! The one list of retouches (M10.19d): every removal, person, patch and eye of a photo as rows — grouped by kind in a
//! fixed order, each kind numbered on its own — and the selection across them. Computed from the edits every time,
//! never stored; no egui.
//!
//! ⚠ Three vectors hold the retouches (`removals` in decode space, `spots` and `eyes` after the lens); this list only
//! names them. A row's flags (its eye, Hold out) stay with each vector's own owner.

use edit_model::{Edits, EyeKind, SpotMode};

/// What a row is — its label and its brush follow from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A painted removal.
    Removal,
    /// A removal of a person Find people found.
    Person,
    Heal,
    Clone,
    /// A Heal patch Find dust made.
    Dust,
    RedEye,
    PetEye,
}

/// Which retouch a row is, by its place in its own vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Removal(usize),
    Spot(usize),
    Eye(usize),
}

/// The brush a kind is made and edited with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brush {
    Remove,
    Heal,
    Clone,
    Eye,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub item: Item,
    pub label: String,
}

fn name(kind: Kind) -> &'static str {
    match kind {
        Kind::Removal => "Removal",
        Kind::Person => "Person",
        Kind::Heal => "Heal",
        Kind::Clone => "Clone",
        Kind::Dust => "Dust",
        Kind::RedEye => "Red eye",
        Kind::PetEye => "Pet eye",
    }
}

/// `item`'s kind in `e`, `None` past its vector's end.
pub fn kind_of(e: &Edits, item: Item) -> Option<Kind> {
    match item {
        Item::Removal(i) => e.removals.get(i).map(|r| {
            if r.person.is_some() {
                Kind::Person
            } else {
                Kind::Removal
            }
        }),
        Item::Spot(i) => e.spots.get(i).map(|s| match (s.dust, s.mode) {
            (true, _) => Kind::Dust,
            (false, SpotMode::Heal) => Kind::Heal,
            (false, SpotMode::Clone) => Kind::Clone,
        }),
        Item::Eye(i) => e.eyes.get(i).map(|y| match y.kind {
            EyeKind::Red => Kind::RedEye,
            EyeKind::Pet => Kind::PetEye,
        }),
    }
}

/// Every retouch of `e`: removals, then patches, then eyes, each in its own order, each kind numbered apart.
pub fn rows(e: &Edits) -> Vec<Row> {
    let items = (0..e.removals.len())
        .map(Item::Removal)
        .chain((0..e.spots.len()).map(Item::Spot))
        .chain((0..e.eyes.len()).map(Item::Eye));
    let mut seen: Vec<(Kind, usize)> = Vec::new();
    items
        .filter_map(|item| {
            let kind = kind_of(e, item)?;
            let n = match seen.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    seen.push((kind, 1));
                    1
                }
            };
            Some(Row {
                kind,
                item,
                label: format!("{} {n}", name(kind)),
            })
        })
        .collect()
}

/// How many retouches `e` holds.
pub fn count(e: &Edits) -> usize {
    e.removals.len() + e.spots.len() + e.eyes.len()
}

/// The brush `kind` is made and edited with.
pub fn brush_of(kind: Kind) -> Brush {
    match kind {
        Kind::Removal | Kind::Person => Brush::Remove,
        Kind::Heal | Kind::Dust => Brush::Heal,
        Kind::Clone => Brush::Clone,
        Kind::RedEye | Kind::PetEye => Brush::Eye,
    }
}

/// The selected retouch — the session keeps one index per vector, at most one of them set.
pub fn selected(spot: Option<usize>, removal: Option<usize>, eye: Option<usize>) -> Option<Item> {
    spot.map(Item::Spot)
        .or(removal.map(Item::Removal))
        .or(eye.map(Item::Eye))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Edits, Eye, EyeKind, Removal, Spot, SpotMode};

    fn stroke() -> Removal {
        Removal {
            points: vec![[0.5, 0.5]],
            radius: 0.02,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: None,
        }
    }

    fn person() -> Removal {
        Removal {
            points: Vec::new(),
            person: Some([0.1, 0.1, 0.2, 0.5]),
            ..stroke()
        }
    }

    fn spot(mode: SpotMode, dust: bool) -> Spot {
        Spot {
            centre: [0.5, 0.5],
            source: [0.4, 0.5],
            radius: 0.02,
            feather: 50.0,
            mode,
            dust,
        }
    }

    fn eye(kind: EyeKind) -> Eye {
        Eye {
            centre: [0.5, 0.5],
            radius: 0.01,
            kind,
            pupil_size: 50.0,
            darken: 50.0,
            catchlight: None,
        }
    }

    fn edits() -> Edits {
        Edits {
            removals: vec![stroke(), person(), stroke()],
            spots: vec![
                spot(SpotMode::Heal, false),
                spot(SpotMode::Clone, false),
                spot(SpotMode::Heal, true),
                spot(SpotMode::Heal, false),
            ],
            eyes: vec![eye(EyeKind::Red), eye(EyeKind::Pet)],
            ..Edits::default()
        }
    }

    #[test]
    fn rows_group_by_kind_and_number_each_kind_apart() {
        let r = rows(&edits());
        let got: Vec<(&str, Item, Kind)> = r
            .iter()
            .map(|r| (r.label.as_str(), r.item, r.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Removal 1", Item::Removal(0), Kind::Removal),
                ("Person 1", Item::Removal(1), Kind::Person),
                ("Removal 2", Item::Removal(2), Kind::Removal),
                ("Heal 1", Item::Spot(0), Kind::Heal),
                ("Clone 1", Item::Spot(1), Kind::Clone),
                ("Dust 1", Item::Spot(2), Kind::Dust),
                ("Heal 2", Item::Spot(3), Kind::Heal),
                ("Red eye 1", Item::Eye(0), Kind::RedEye),
                ("Pet eye 1", Item::Eye(1), Kind::PetEye),
            ]
        );
        assert_eq!(count(&edits()), 9);
    }

    /// Review Focus 5: deleting one kind's retouch renumbers that kind only.
    #[test]
    fn deleting_one_kind_renumbers_only_that_kind() {
        let mut e = edits();
        e.spots.remove(0);
        let labels: Vec<String> = rows(&e).into_iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                "Removal 1",
                "Person 1",
                "Removal 2",
                "Clone 1",
                "Dust 1",
                "Heal 1",
                "Red eye 1",
                "Pet eye 1"
            ]
        );
    }

    #[test]
    fn each_kind_has_its_brush() {
        assert_eq!(brush_of(Kind::Removal), Brush::Remove);
        assert_eq!(brush_of(Kind::Person), Brush::Remove);
        assert_eq!(brush_of(Kind::Heal), Brush::Heal);
        assert_eq!(brush_of(Kind::Dust), Brush::Heal);
        assert_eq!(brush_of(Kind::Clone), Brush::Clone);
        assert_eq!(brush_of(Kind::RedEye), Brush::Eye);
        assert_eq!(brush_of(Kind::PetEye), Brush::Eye);
        let e = edits();
        assert_eq!(kind_of(&e, Item::Spot(2)), Some(Kind::Dust));
        assert_eq!(kind_of(&e, Item::Removal(1)), Some(Kind::Person));
        assert_eq!(kind_of(&e, Item::Eye(5)), None, "out of range");
    }

    #[test]
    fn the_selection_is_whichever_is_set() {
        assert_eq!(selected(Some(2), None, None), Some(Item::Spot(2)));
        assert_eq!(selected(None, Some(1), None), Some(Item::Removal(1)));
        assert_eq!(selected(None, None, Some(0)), Some(Item::Eye(0)));
        assert_eq!(selected(None, None, None), None);
    }
}

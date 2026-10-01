//! Where the side panels and the filmstrip sit (M10.8b): the Library rail left or right, Develop's
//! edit panel left or right, the filmstrip at the bottom, left or right — each on its own, chosen from
//! the View menu and remembered (the author's choices, 2026-10-01).
//!
//! ⚠ No egui: the bin turns a [`Side`] into a panel. What lives here is the one rule that is easy to get
//! wrong — the order Develop's pieces are allocated in.

use serde::{Deserialize, Serialize};

/// A side of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Where the filmstrip sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum StripPlace {
    #[default]
    Bottom,
    Left,
    Right,
}

impl StripPlace {
    /// The side it is on, if it is on one.
    pub fn side(self) -> Option<Side> {
        match self {
            StripPlace::Bottom => None,
            StripPlace::Left => Some(Side::Left),
            StripPlace::Right => Some(Side::Right),
        }
    }
}

/// The three choices. The default is the layout Rustroom has always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub library_rail: Side,
    pub edit_panel: Side,
    pub filmstrip: StripPlace,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            library_rail: Side::Left,
            edit_panel: Side::Right,
            filmstrip: StripPlace::Bottom,
        }
    }
}

/// One of Develop's movable pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    Filmstrip,
    ToolRail,
    EditPanel,
}

/// The order Develop's pieces are drawn in — **outermost first**, because egui gives a side panel the
/// room the panels before it left.
///
/// ⚠⚠ A strip on the edit panel's side goes **first**, so it sits at the window edge with the tool
/// rail and the panel inside it — the sliders stay beside the photo (the author's choice). The tool
/// rail always comes before the panel: it is the panel's outer edge. A bottom strip comes **after**
/// the side panels, so it spans only the photo column, as it always has.
pub fn develop_order(l: &Layout) -> [Piece; 3] {
    if l.filmstrip.side() == Some(l.edit_panel) {
        [Piece::Filmstrip, Piece::ToolRail, Piece::EditPanel]
    } else {
        [Piece::ToolRail, Piece::EditPanel, Piece::Filmstrip]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_layout_rustroom_always_had() {
        let l = Layout::default();
        assert_eq!(
            (l.library_rail, l.edit_panel, l.filmstrip),
            (Side::Left, Side::Right, StripPlace::Bottom)
        );
        let parsed: Layout = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, l, "a settings file from before M10.8b");
    }

    /// The strip goes outermost only when it shares the edit panel's side; the rail always precedes
    /// the panel. All six pairs.
    #[test]
    fn the_strip_is_outermost_only_on_the_panels_side() {
        use Piece::*;
        for panel in [Side::Left, Side::Right] {
            for strip in [StripPlace::Bottom, StripPlace::Left, StripPlace::Right] {
                let l = Layout {
                    edit_panel: panel,
                    filmstrip: strip,
                    ..Layout::default()
                };
                let want = if strip.side() == Some(panel) {
                    [Filmstrip, ToolRail, EditPanel]
                } else {
                    [ToolRail, EditPanel, Filmstrip]
                };
                assert_eq!(develop_order(&l), want, "{panel:?} {strip:?}");
            }
        }
        // ⚠ The sample: the rule produces both orders, so the loop is not one case six times.
        let shared = Layout {
            edit_panel: Side::Left,
            filmstrip: StripPlace::Left,
            ..Layout::default()
        };
        assert_ne!(develop_order(&shared), develop_order(&Layout::default()));
    }

    #[test]
    fn other_flips_and_a_place_knows_its_side() {
        assert_eq!(Side::Left.other(), Side::Right);
        assert_eq!(Side::Right.other(), Side::Left);
        assert_eq!(StripPlace::Bottom.side(), None);
        assert_eq!(StripPlace::Right.side(), Some(Side::Right));
    }
}

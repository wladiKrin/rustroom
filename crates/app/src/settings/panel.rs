//! The Develop panel: its sections, their order, and which are collapsed.

use super::Settings;
use serde::{Deserialize, Serialize};

/// One section of the Library panel.
///
/// ⚠ Exists so the panel's **order** can be a value rather than five literal calls. The bodies
/// stay exactly where they are; `collections_panel::show` matches this to each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelSection {
    Folders,
    Collections,
    Smart,
    Keywords,
    Dates,
    Immich,
}

impl PanelSection {
    /// Every section, in the order the panel shipped with.
    ///
    /// ⚠ Folders first because "where photos come from" is the outer concept, and All Photos —
    /// the whole catalog — is the natural top of it. That reasoning is `collections_panel`'s and
    /// is preserved here as the default rather than re-derived.
    pub const ALL: [PanelSection; 6] = [
        Self::Folders,
        Self::Collections,
        Self::Smart,
        Self::Keywords,
        Self::Dates,
        // ⚠ Last, so an existing `panel_order` — which `Settings::panel_order` repairs by
        // appending whatever `ALL` member it lacks — grows the new section at the bottom of the
        // rail rather than shuffling the five somebody already arranged.
        Self::Immich,
    ];
}

/// Which of the Library panel's sections are expanded.
///
/// ⚠⚠ **Persisted here rather than left to egui**, because this app implements no
/// `eframe::App::save` — egui's own collapsing state dies with the process, so a collapsed
/// section would spring open on every launch and the control would be worth nothing.
///
/// ⚠⚠ **All CLOSED by default** (user's call, 2026-08-27). The first version defaulted them open,
/// on the argument that an existing user should lose no discoverability — but the point of the
/// control is that the panel does not show every option at once, and a default showing them all
/// makes it something you must set up before it helps. A collapsed header still names its
/// section, so nothing is hidden, only folded.
///
/// ⚠ `Default` is **derived**, which is correct only while every section defaults to *closed* —
/// `false` is `bool::default()`. Flip any of them and it must become a hand-written impl, the way
/// `RadialGradient::default` and `ColorGrading::default` already are. `..._still_loads` asserts
/// the value rather than the mechanism, so it catches either way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PanelSections {
    pub folders: bool,
    pub collections: bool,
    pub smart: bool,
    pub keywords: bool,
    pub dates: bool,
    /// ⚠ Defaults to closed like the rest, so the derived `Default` above stays correct.
    pub immich: bool,
}

impl Settings {
    /// The panel's section order, guaranteed to contain every section exactly once.
    ///
    /// ⚠⚠ **Repaired rather than trusted.** The file is user-editable, so the stored order can
    /// omit a section, list one twice, or be empty. Duplicates are dropped and anything missing is
    /// appended in default order, so **a section can never become unreachable** whatever the file
    /// says. That is `FormatFilter`'s hidden-by-exception robustness: the safe state is the one
    /// you get by default, not one you have to maintain.
    pub fn panel_order(&self) -> Vec<PanelSection> {
        let mut out: Vec<PanelSection> = Vec::with_capacity(PanelSection::ALL.len());
        for s in &self.panel_order {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        for s in PanelSection::ALL {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ `settings.json` is hand-editable, so the stored order can be any list at all. A section
    /// missing from it must still be reachable.
    #[test]
    fn a_stored_order_missing_a_section_gets_it_appended() {
        let s = Settings {
            panel_order: vec![PanelSection::Dates, PanelSection::Folders],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got[0], PanelSection::Dates, "the stored prefix is kept");
        assert_eq!(got[1], PanelSection::Folders);
        assert_eq!(
            got.len(),
            PanelSection::ALL.len(),
            "and every section is present"
        );
        for sec in PanelSection::ALL {
            assert!(got.contains(&sec), "{sec:?} must be reachable");
        }
    }

    #[test]
    fn a_duplicated_section_appears_once() {
        let s = Settings {
            panel_order: vec![
                PanelSection::Folders,
                PanelSection::Folders,
                PanelSection::Dates,
            ],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got.len(), PanelSection::ALL.len());
        assert_eq!(
            got.iter().filter(|x| **x == PanelSection::Folders).count(),
            1
        );
    }

    #[test]
    fn an_empty_stored_order_gives_the_default() {
        let s = Settings {
            panel_order: Vec::new(),
            ..Settings::default()
        };
        assert_eq!(s.panel_order(), PanelSection::ALL.to_vec());
    }

    /// ⚠ The default really is the order the panel shipped with — Folders first, because "where
    /// photos come from" is the outer concept.
    #[test]
    fn the_default_order_is_the_shipped_one() {
        assert_eq!(
            Settings::default().panel_order(),
            PanelSection::ALL.to_vec()
        );
        assert_eq!(PanelSection::ALL[0], PanelSection::Folders);
    }
}

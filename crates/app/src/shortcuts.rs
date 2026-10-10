//! A user's own keyboard shortcuts (M10.9b): the bindings they changed, over the menu's defaults.
//!
//! ⚠ A lib module over the menu model — no egui. `Settings::shortcuts` stores **only what was
//! changed**; [`apply`] lays it over [`menu::model_with`], and [`menu::model_for`] is the one bound
//! model every reader uses. A chord a command takes is **taken over** from whoever held it (the
//! author's choice); [`refusal`] names the chords that cannot be taken at all.

use crate::menu::{self, Command, Item, Section, Shortcut};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One changed binding: the command, by [`command_key`], and its chord — `None` for none at all.
///
/// ⚠ An entry naming no command (a renamed variant, a removed command) is ignored, never an error —
/// Settings must always load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rebinding {
    pub command: String,
    pub shortcut: Option<Shortcut>,
}

/// The commands whose chords stay the system's edit keys: the window turns ⌘C and ⌘V into copy and
/// paste before a key arrives, and ⌘Z / ⇧⌘Z are what every text field undoes with.
pub const FIXED: [Command; 4] = [
    Command::Undo,
    Command::Redo,
    Command::CopySettings,
    Command::PasteSettings,
];

/// A command's stable name in `Settings` — its `Debug` form: `SelectTool(Crop)`,
/// `StackByTime(Some(5))`.
pub fn command_key(c: Command) -> String {
    format!("{c:?}")
}

/// `sections` with each overridden command's shortcut replaced — the bound model.
pub fn apply(mut sections: Vec<Section>, overrides: &[Rebinding]) -> Vec<Section> {
    fn walk(items: &mut [Item], overrides: &[Rebinding]) {
        for item in items {
            match item {
                Item::Action {
                    command, shortcut, ..
                } => {
                    let key = command_key(*command);
                    if let Some(r) = overrides.iter().find(|r| r.command == key) {
                        *shortcut = r.shortcut;
                    }
                }
                Item::Submenu { items, .. } => walk(items, overrides),
                Item::Separator => {}
            }
        }
    }
    for s in &mut sections {
        walk(&mut s.items, overrides);
    }
    sections
}

/// One command as the Keyboard tab lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The menu it is in.
    pub menu: &'static str,
    /// Its label, a submenu item's as "Stack by capture time ▸ 5 s".
    pub label: String,
    pub command: Command,
    pub shortcut: Option<Shortcut>,
}

/// Every command in menu order, with its bound shortcut.
pub fn entries(recent: &[PathBuf], overrides: &[Rebinding]) -> Vec<Entry> {
    fn walk(menu: &'static str, prefix: &str, items: &[Item], out: &mut Vec<Entry>) {
        for item in items {
            match item {
                Item::Action {
                    label,
                    command,
                    shortcut,
                } => out.push(Entry {
                    menu,
                    label: format!("{prefix}{label}"),
                    command: *command,
                    shortcut: *shortcut,
                }),
                Item::Submenu { label, items } => {
                    walk(menu, &format!("{prefix}{label} ▸ "), items, out)
                }
                Item::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for s in menu::model_for(recent, overrides) {
        walk(s.title, "", &s.items, &mut out);
    }
    out
}

/// A command's shortcut under these overrides.
pub fn shortcut_of(c: Command, overrides: &[Rebinding]) -> Option<Shortcut> {
    entries(&[], overrides)
        .into_iter()
        .find(|e| e.command == c)
        .and_then(|e| e.shortcut)
}

/// A command's shipped shortcut.
pub fn default_of(c: Command) -> Option<Shortcut> {
    shortcut_of(c, &[])
}

/// A command's label, for a note — "Stack", "Stack by capture time ▸ 5 s".
pub fn label_of(c: Command) -> String {
    entries(&[], &[])
        .into_iter()
        .find(|e| e.command == c)
        .map(|e| e.label)
        .unwrap_or_else(|| command_key(c))
}

/// Why `sc` cannot be chosen, or `None` when it can.
pub fn refusal(sc: Shortcut) -> Option<&'static str> {
    let plain_cmd = sc.cmd && !sc.alt && !sc.shift;
    if !sc.cmd && !sc.alt && (sc.key.is_ascii_digit() || matches!(sc.key, 'P' | 'X' | 'U')) {
        return Some("Rates, labels and flags photos");
    }
    if let Some(c) = FIXED.iter().find(|c| default_of(**c) == Some(sc)) {
        return Some(match c {
            Command::Undo => "Undoes, as everywhere on this system",
            Command::Redo => "Redoes, as everywhere on this system",
            Command::CopySettings => "Copies, as everywhere on this system",
            _ => "Pastes, as everywhere on this system",
        });
    }
    match (plain_cmd, sc.key) {
        (true, 'A') => Some("Selects all photos"),
        (true, 'X') => Some("Cuts, as everywhere on this system"),
        (true, 'Q') => Some("Quits the app"),
        (true, 'H') => Some("Hides the app"),
        (true, 'M') => Some("Minimises the window"),
        (true, 'W') => Some("Closes the window"),
        _ if sc.cmd && sc.alt && !sc.shift && sc.key == 'H' => Some("Hides the other apps"),
        _ => None,
    }
}

/// Give `c` the chord `sc` (`None`: no shortcut). A chord another command holds is **taken over** —
/// that command is left with none — and returned, so the tab can say whose it was.
///
/// ⚠ An override equal to the default is dropped, so `Settings` keeps only real changes.
pub fn assign(overrides: &mut Vec<Rebinding>, c: Command, sc: Option<Shortcut>) -> Option<Command> {
    let mut loser = None;
    if let Some(sc) = sc {
        let holder = entries(&[], overrides)
            .into_iter()
            .find(|e| e.shortcut == Some(sc) && e.command != c)
            .map(|e| e.command);
        if let Some(h) = holder {
            set(overrides, h, None);
            loser = Some(h);
        }
    }
    set(overrides, c, sc);
    loser
}

fn set(overrides: &mut Vec<Rebinding>, c: Command, sc: Option<Shortcut>) {
    let key = command_key(c);
    overrides.retain(|r| r.command != key);
    if default_of(c) != sc {
        overrides.push(Rebinding {
            command: key,
            shortcut: sc,
        });
    }
}

/// Put `c` back on its default.
pub fn reset(overrides: &mut Vec<Rebinding>, c: Command) {
    let key = command_key(c);
    overrides.retain(|r| r.command != key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    fn of(c: Command, o: &[Rebinding]) -> Option<Shortcut> {
        shortcut_of(c, o)
    }

    #[test]
    fn an_override_replaces_the_default_and_none_removes_it() {
        let mut o = Vec::new();
        assert_eq!(
            of(Command::Stack, &o),
            Some(Shortcut::cmd('G')),
            "the sample"
        );
        assign(&mut o, Command::Stack, Some(Shortcut::cmd('J').with_alt()));
        assert_eq!(of(Command::Stack, &o), Some(Shortcut::cmd('J').with_alt()));
        assign(&mut o, Command::Stack, None);
        assert_eq!(of(Command::Stack, &o), None);
        reset(&mut o, Command::Stack);
        assert_eq!(of(Command::Stack, &o), Some(Shortcut::cmd('G')));
        assert!(o.is_empty(), "a reset leaves nothing behind");
    }

    /// ⚠⚠ A chord another command holds moves — that one is left with none, and is named.
    #[test]
    fn a_taken_chord_moves_and_the_loser_is_named() {
        let mut o = Vec::new();
        let loser = assign(&mut o, Command::TakeSnapshot, Some(Shortcut::cmd('G')));
        assert_eq!(loser, Some(Command::Stack));
        assert_eq!(of(Command::TakeSnapshot, &o), Some(Shortcut::cmd('G')));
        assert_eq!(of(Command::Stack, &o), None, "Stack lost it");
        let free = assign(
            &mut o,
            Command::TakeSnapshot,
            Some(Shortcut::cmd('J').with_alt()),
        );
        assert_eq!(free, None, "a free chord takes from nobody");
    }

    /// A binding back to the default leaves no override.
    #[test]
    fn a_default_equal_override_drops_out() {
        let mut o = Vec::new();
        assign(&mut o, Command::Stack, None);
        assign(&mut o, Command::Stack, Some(Shortcut::cmd('G')));
        assert!(o.is_empty(), "{o:?}");
    }

    #[test]
    fn the_refused_chords_say_why() {
        for sc in [
            Shortcut::bare('5'),
            Shortcut::bare('P'),
            Shortcut {
                shift: true,
                ..Shortcut::bare('X')
            },
            Shortcut::cmd('A'),
            Shortcut::cmd('X'),
            Shortcut::cmd('Q'),
            Shortcut::cmd('W'),
            Shortcut::cmd('H').with_alt(),
            Shortcut::cmd('Z'),
            Shortcut::cmd_shift('Z'),
            Shortcut::cmd('C'),
            Shortcut::cmd('V'),
        ] {
            assert!(refusal(sc).is_some(), "{} should be refused", sc.label());
        }
        for sc in [
            Shortcut::bare('B'),
            Shortcut::cmd('5'),
            Shortcut::cmd('S').with_alt(),
            Shortcut::cmd_shift('A'),
            Shortcut::bare(menu::fn_key(5).unwrap()),
        ] {
            assert_eq!(refusal(sc), None, "{} should be allowed", sc.label());
        }
    }

    /// ⚠ An entry naming no command is ignored — `Settings` must always load.
    #[test]
    fn an_unknown_command_is_ignored() {
        let o = vec![Rebinding {
            command: "NoSuchCommand".into(),
            shortcut: Some(Shortcut::cmd('B')),
        }];
        let bound = entries(&[], &o);
        assert_eq!(bound, entries(&[], &[]));
    }

    /// Every command has its own key, and the key names it back.
    #[test]
    fn every_command_key_is_unique() {
        let all = entries(&[], &[]);
        assert!(all.len() > 50, "the sample: {}", all.len());
        let mut keys: Vec<String> = all.iter().map(|e| command_key(e.command)).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "two commands share a key");
        assert_eq!(
            command_key(Command::SelectTool(Tool::Crop)),
            "SelectTool(Crop)"
        );
    }

    #[test]
    fn a_submenu_item_names_its_submenu() {
        let e = entries(&[], &[]);
        assert!(
            e.iter()
                .any(|e| e.label.starts_with("Stack by capture time ▸ ")),
            "{:?}",
            e.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rebindings_round_trip_through_json() {
        let o = vec![
            Rebinding {
                command: command_key(Command::Stack),
                shortcut: Some(Shortcut::cmd('J').with_alt()),
            },
            Rebinding {
                command: command_key(Command::Export),
                shortcut: None,
            },
        ];
        let back: Vec<Rebinding> =
            serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
        // A chord saved before ⌥ existed still reads.
        let old: Shortcut =
            serde_json::from_str(r#"{"cmd":true,"shift":false,"key":"G"}"#).unwrap();
        assert_eq!(old, Shortcut::cmd('G'));
    }

    #[test]
    fn labels_name_alt_and_the_f_keys() {
        let f5 = menu::fn_key(5).unwrap();
        assert_eq!(menu::fn_number(f5), Some(5));
        assert_eq!(menu::fn_key(13), None);
        let l = Shortcut::cmd('S').with_alt().label();
        assert!(
            l.contains('S') && (l.contains('⌥') || l.contains("Alt+")),
            "{l}"
        );
        assert!(Shortcut::bare(f5).label().ends_with("F5"));
        if cfg!(target_os = "macos") {
            assert_eq!(Shortcut::cmd_shift('S').with_alt().label(), "⌥⇧⌘S");
        }
    }
}

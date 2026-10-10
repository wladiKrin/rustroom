//! Keys: which `Command` a key press is, and the key each shortcut names.

use eframe::egui;
use rustroom_app::menu;
use rustroom_app::menu::Command;

/// Turn this frame's key presses into a command, using the menu model as the single source of
/// truth for which chord means what.
///
/// ⚠ Gated on nothing having keyboard focus — the rule arrow navigation and the rail already
/// follow. Without it, typing "d" into the Save-preset name field jumps to Develop.
///
/// ⚠ **Not used for ⌘C / ⌘V.** `egui-winit` matches the clipboard chords first and pushes
/// `Event::Copy` / `Event::Paste` *instead of* a `Key` event, so `key_pressed(Key::C)` can
/// never fire — a trap this codebase has already paid for once. Those stay in
/// `handle_clipboard_keys`, which reads the semantic events.
pub(crate) fn pressed_command(
    ctx: &egui::Context,
    overrides: &[rustroom_app::shortcuts::Rebinding],
) -> Option<Command> {
    if ctx.memory(|m| m.focused()).is_some() {
        return None;
    }
    let mut hit = None;
    let mut bare_y_bound = false;
    // ⚠⚠ The **bound** model (M10.9b): a user's own chord fires its command, and the default it
    // replaced does not.
    for section in menu::model_for(&[], overrides) {
        menu::walk(&section.items, &mut |item| {
            let menu::Item::Action {
                command,
                shortcut: Some(sc),
                ..
            } = item
            else {
                return;
            };
            bare_y_bound |= !sc.cmd && !sc.alt && sc.key == 'Y';
            if hit.is_some() || (sc.cmd && matches!(sc.key, 'C' | 'V')) {
                return;
            }
            let Some(key) = egui_key(sc.key) else {
                return;
            };
            let pressed = ctx.input(|i| {
                i.key_pressed(key)
                    && i.modifiers.command == sc.cmd
                    && i.modifiers.shift == sc.shift
                    && i.modifiers.alt == sc.alt
            });
            if pressed {
                hit = Some(*command);
            }
        });
    }
    // ⚠ An alias yields to a bound shortcut on its key — a user who gave `Y` to something else
    // meant that, not Compare.
    if hit.is_none() {
        for (key, cmd) in menu::ALIASES {
            if *key == 'Y' && bare_y_bound {
                continue;
            }
            let Some(k) = egui_key(*key) else { continue };
            let pressed = ctx.input(|i| {
                i.key_pressed(k) && !i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt
            });
            if pressed {
                hit = Some(*cmd);
                break;
            }
        }
    }
    hit
}

/// Every key a chord can name, as the model writes it and as egui reports it (M10.9b: every letter,
/// digit, punctuation key and F1–F12). ⚠ One table, read both ways — [`egui_key`] and [`key_char`]
/// — so recording a chord and pressing it cannot disagree about a key.
const KEYS: &[(char, egui::Key)] = &[
    ('A', egui::Key::A),
    ('B', egui::Key::B),
    ('C', egui::Key::C),
    ('D', egui::Key::D),
    ('E', egui::Key::E),
    ('F', egui::Key::F),
    ('G', egui::Key::G),
    ('H', egui::Key::H),
    ('I', egui::Key::I),
    ('J', egui::Key::J),
    ('K', egui::Key::K),
    ('L', egui::Key::L),
    ('M', egui::Key::M),
    ('N', egui::Key::N),
    ('O', egui::Key::O),
    ('P', egui::Key::P),
    ('Q', egui::Key::Q),
    ('R', egui::Key::R),
    ('S', egui::Key::S),
    ('T', egui::Key::T),
    ('U', egui::Key::U),
    ('V', egui::Key::V),
    ('W', egui::Key::W),
    ('X', egui::Key::X),
    ('Y', egui::Key::Y),
    ('Z', egui::Key::Z),
    ('0', egui::Key::Num0),
    ('1', egui::Key::Num1),
    ('2', egui::Key::Num2),
    ('3', egui::Key::Num3),
    ('4', egui::Key::Num4),
    ('5', egui::Key::Num5),
    ('6', egui::Key::Num6),
    ('7', egui::Key::Num7),
    ('8', egui::Key::Num8),
    ('9', egui::Key::Num9),
    ('\u{F704}', egui::Key::F1),
    ('\u{F705}', egui::Key::F2),
    ('\u{F706}', egui::Key::F3),
    ('\u{F707}', egui::Key::F4),
    ('\u{F708}', egui::Key::F5),
    ('\u{F709}', egui::Key::F6),
    ('\u{F70A}', egui::Key::F7),
    ('\u{F70B}', egui::Key::F8),
    ('\u{F70C}', egui::Key::F9),
    ('\u{F70D}', egui::Key::F10),
    ('\u{F70E}', egui::Key::F11),
    ('\u{F70F}', egui::Key::F12),
    (',', egui::Key::Comma),
    ('.', egui::Key::Period),
    ('/', egui::Key::Slash),
    (';', egui::Key::Semicolon),
    ('\'', egui::Key::Quote),
    ('[', egui::Key::OpenBracket),
    (']', egui::Key::CloseBracket),
    ('\\', egui::Key::Backslash),
    ('-', egui::Key::Minus),
    ('+', egui::Key::Plus),
    ('=', egui::Key::Equals),
    ('`', egui::Key::Backtick),
    ('\t', egui::Key::Tab),
];

/// The model's key character as an egui key.
///
/// ⚠ Returns `Option` rather than panicking: an unmappable chord should make its menu item
/// mouse-only, not take the app down. `every_shortcut_maps_to_an_egui_key` makes sure none of
/// the shipped ones actually do that.
fn egui_key(c: char) -> Option<egui::Key> {
    KEYS.iter().find(|(k, _)| *k == c).map(|(_, e)| *e)
}

/// The model's key character for an egui key — what recording a chord reads (M10.9b). ⚠ `Tab` is
/// not offered: in a dialog it moves the focus.
pub(crate) fn key_char(k: egui::Key) -> Option<char> {
    KEYS.iter()
        .find(|(c, e)| *e == k && *c != '\t')
        .map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One press of a bare, unmodified key.
    fn bare_key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn the_composition_guide_key_is_gated_on_nothing_having_focus() {
        // ⚠⚠ **Both halves, and neither is optional.** A gate that never lets the key through
        // and one that never blocks it are equally wrong, and only the pair can tell them
        // apart — the rule `a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not`
        // already follows. `O` is bare, so a text field must swallow it: the Save-preset name
        // field and the Library search box are both one keystroke away.
        let ctx = egui::Context::default();

        let mut unfocused = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![bare_key(egui::Key::O)],
                ..Default::default()
            },
            |_ui| unfocused = pressed_command(&ctx, &[]),
        );
        assert_eq!(
            unfocused,
            Some(Command::CycleCompositionGuide),
            "with nothing focused, O must reach the command"
        );

        // Give a text field the focus, then press the same key again.
        let mut text = String::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text))
                .request_focus();
        });
        let mut focused = Some(Command::CycleCompositionGuide);
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![bare_key(egui::Key::O)],
                ..Default::default()
            },
            |ui| {
                // ⚠ Read **before** the field is drawn, deliberately. A `TextEdit` consumes the
                // key events it handles, so reading afterwards would pass even with the focus
                // gate deleted — it would be testing egui's consumption, not our guard. The
                // field is still added, so egui does not drop its focus at the end of the pass.
                focused = pressed_command(&ctx, &[]);
                ui.add(egui::TextEdit::singleline(&mut text));
            },
        );
        assert_eq!(
            focused, None,
            "a key typed into a text field must not reach a command"
        );
    }

    #[test]
    fn every_shortcut_maps_to_an_egui_key() {
        // ⚠ A chord in the model with no egui key is a menu item whose shortcut silently does
        // nothing on Windows and Linux — and invisible on macOS, where the native menu handles
        // the safe ones anyway.
        for section in menu::model() {
            for item in section.items {
                if let menu::Item::Action {
                    label,
                    shortcut: Some(sc),
                    ..
                } = item
                {
                    assert!(
                        egui_key(sc.key).is_some(),
                        "{label}'s shortcut key {:?} has no egui mapping",
                        sc.key
                    );
                }
            }
        }
    }

    /// Press one bare key and return what `pressed_command` made of it.
    fn bare(k: egui::Key) -> Option<menu::Command> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut got = None;
        let _ = ctx.run_ui(input, |ui| {
            got = pressed_command(ui.ctx(), &[]);
        });
        got
    }

    #[test]
    fn a_bare_c_really_reaches_the_dispatcher() {
        // ⚠⚠ **The one fact this feature could not be built on faith.** `egui-winit` matches the
        // clipboard chords *first* and pushes `Event::Copy` **instead of** a `Key` event, so
        // `key_pressed(Key::C)` can never see ⌘C — a trap this codebase has already paid for.
        // Reading its source settles the other half: `is_copy_command` is
        // `modifiers.command && key == C`, so a **bare** C is untouched. This asserts our end of
        // it — that the bare chord is not skipped by `pressed_command`'s own ⌘C/⌘V exclusion,
        // which tests `sc.cmd` and would swallow both if it tested only the character.
        assert_eq!(bare(egui::Key::C), Some(menu::Command::ShowCompare));
        assert_eq!(bare(egui::Key::N), Some(menu::Command::ShowSurvey));
    }

    #[test]
    fn a_bare_key_is_ignored_while_a_text_field_has_focus() {
        // ⚠ The guard on the guard: `C` and `N` are ordinary characters, so without this the
        // Library's search field would jump to Compare on every "c" typed into it. Asserted
        // through the *same* function, or it would be a test about a different code path.
        let ctx = egui::Context::default();
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut got = None;
        let _ = ctx.run_ui(input, |ui| {
            got = pressed_command(ui.ctx(), &[]);
        });
        assert_eq!(got, None, "a bare C typed into a field switched view");
    }

    /// Press `k` with these modifiers under `overrides`, and return the command it fired.
    fn chord(
        k: egui::Key,
        modifiers: egui::Modifiers,
        overrides: &[rustroom_app::shortcuts::Rebinding],
    ) -> Option<menu::Command> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let mut got = None;
        let _ = ctx.run_ui(input, |ui| got = pressed_command(ui.ctx(), overrides));
        got
    }

    /// ⚠⚠ M10.9b: a rebound chord fires its new command, and the default it replaced no longer
    /// fires anything.
    #[test]
    fn a_rebound_chord_fires_its_new_command_and_the_old_one_does_not() {
        use rustroom_app::menu::{Command, Shortcut};
        let cmd = egui::Modifiers::COMMAND;
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            chord(egui::Key::G, cmd, &[]),
            Some(Command::Stack),
            "the sample"
        );
        let mut o = Vec::new();
        rustroom_app::shortcuts::assign(
            &mut o,
            Command::Stack,
            Some(Shortcut::cmd('S').with_alt()),
        );
        assert_eq!(chord(egui::Key::S, cmd_alt, &o), Some(Command::Stack));
        assert_eq!(chord(egui::Key::G, cmd, &o), None, "⌘G is free now");
        assert_eq!(chord(egui::Key::S, cmd, &o), None, "⌥ is part of the chord");
    }

    /// The `Y` alias for Compare yields to a user's own bare `Y`.
    #[test]
    fn the_alias_yields_to_a_bound_y() {
        use rustroom_app::menu::{Command, Shortcut};
        let none = egui::Modifiers::NONE;
        assert_eq!(
            chord(egui::Key::Y, none, &[]),
            Some(Command::ShowCompare),
            "the sample"
        );
        let mut o = Vec::new();
        rustroom_app::shortcuts::assign(&mut o, Command::TakeSnapshot, Some(Shortcut::bare('Y')));
        assert_eq!(chord(egui::Key::Y, none, &o), Some(Command::TakeSnapshot));
    }

    /// Recording reads the same table pressing does: every key that can be recorded maps back.
    #[test]
    fn every_recordable_key_maps_both_ways() {
        let mut n = 0;
        for (c, k) in KEYS {
            if *c == '\t' {
                assert_eq!(key_char(*k), None, "Tab is not recorded");
                continue;
            }
            assert_eq!(key_char(*k), Some(*c));
            assert_eq!(egui_key(*c), Some(*k));
            n += 1;
        }
        // 26 letters, 10 digits, 12 F-keys, 12 punctuation keys.
        assert_eq!(n, 60, "the sample");
    }
}

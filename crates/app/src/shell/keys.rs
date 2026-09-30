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
pub(super) fn pressed_command(ctx: &egui::Context) -> Option<Command> {
    if ctx.memory(|m| m.focused()).is_some() {
        return None;
    }
    let mut hit = None;
    for section in menu::model() {
        menu::walk(&section.items, &mut |item| {
            if hit.is_some() {
                return;
            }
            let menu::Item::Action {
                command,
                shortcut: Some(sc),
                ..
            } = item
            else {
                return;
            };
            if sc.cmd && matches!(sc.key, 'C' | 'V') {
                return;
            }
            let Some(key) = egui_key(sc.key) else {
                return;
            };
            let pressed = ctx.input(|i| {
                i.key_pressed(key) && i.modifiers.command == sc.cmd && i.modifiers.shift == sc.shift
            });
            if pressed {
                hit = Some(*command);
            }
        });
        if hit.is_some() {
            break;
        }
    }
    if hit.is_none() {
        for (key, cmd) in menu::ALIASES {
            let Some(k) = egui_key(*key) else { continue };
            let pressed =
                ctx.input(|i| i.key_pressed(k) && !i.modifiers.command && !i.modifiers.shift);
            if pressed {
                hit = Some(*cmd);
                break;
            }
        }
    }
    hit
}

/// The model's key character as an egui key.
///
/// ⚠ Returns `Option` rather than panicking: an unmappable chord should make its menu item
/// mouse-only, not take the app down. `every_shortcut_maps_to_an_egui_key` makes sure none of
/// the shipped ones actually do that.
fn egui_key(c: char) -> Option<egui::Key> {
    Some(match c {
        'O' => egui::Key::O,
        'I' => egui::Key::I,
        'E' => egui::Key::E,
        'Z' => egui::Key::Z,
        // ⚠ Mapped even though `pressed_command` skips ⌘C / ⌘V. This table is a *total*
        // mapping from the model's key characters to egui keys; whether a given chord is
        // consulted is a separate decision, made there and documented there. A hole here
        // would make the completeness test unable to say what it means.
        'C' => egui::Key::C,
        'V' => egui::Key::V,
        'G' => egui::Key::G,
        'D' => egui::Key::D,
        'N' => egui::Key::N,
        'R' => egui::Key::R,
        'K' => egui::Key::K,
        'Q' => egui::Key::Q,
        'T' => egui::Key::T,
        'L' => egui::Key::L,
        'J' => egui::Key::J,
        'Y' => egui::Key::Y,
        '\t' => egui::Key::Tab,
        '+' => egui::Key::Plus,
        '-' => egui::Key::Minus,
        '0' => egui::Key::Num0,
        '1' => egui::Key::Num1,
        '2' => egui::Key::Num2,
        '3' => egui::Key::Num3,
        '4' => egui::Key::Num4,
        '5' => egui::Key::Num5,
        ',' => egui::Key::Comma,
        '[' => egui::Key::OpenBracket,
        ']' => egui::Key::CloseBracket,
        '\'' => egui::Key::Quote,
        '\\' => egui::Key::Backslash,
        _ => return None,
    })
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
            |_ui| unfocused = pressed_command(&ctx),
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
                focused = pressed_command(&ctx);
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
            got = pressed_command(ui.ctx());
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
            got = pressed_command(ui.ctx());
        });
        assert_eq!(got, None, "a bare C typed into a field switched view");
    }
}

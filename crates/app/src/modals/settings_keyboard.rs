//! Settings → Keyboard (M10.9b): every menu command and its shortcut. Click a shortcut, press the new
//! keys; a chord another command holds is taken over (the author's choice), and the row says from whom.
//!
//! ⚠ In the Settings **draft** like every other tab — nothing lands before Apply. The rules (take-over,
//! refusal, reset) are the lib's `shortcuts`; this draws them and reads the keys.

use crate::theme;
use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::menu::{Command, Shortcut};
use rustroom_app::shortcuts::{self, Rebinding};

/// The tab's part of the draft.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct KeyboardDraft {
    /// The bindings being edited — only the changes, as `Settings::shortcuts` keeps them.
    pub overrides: Vec<Rebinding>,
    /// The command whose shortcut is being recorded.
    pub recording: Option<Command>,
    pub filter: String,
    /// What the last recording did, under its row: the text, and whether it was refused.
    pub note: Option<(Command, String, bool)>,
}

impl KeyboardDraft {
    pub fn from(overrides: &[Rebinding]) -> Self {
        Self {
            overrides: overrides.to_vec(),
            ..Self::default()
        }
    }

    /// Escape while recording stops it — called **before** the dialog draws, since `dialog` closes on
    /// an Escape it sees first. Returns whether it took the key.
    pub fn take_escape(&mut self, ctx: &egui::Context) -> bool {
        if self.recording.is_none() || !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return false;
        }
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        self.recording = None;
        true
    }
}

/// The tab's body.
pub fn show(ui: &mut egui::Ui, d: &mut KeyboardDraft, recent: &[std::path::PathBuf]) {
    if let Some(c) = d.recording {
        record(ui, d, c);
    }
    ui.add(
        egui::TextEdit::singleline(&mut d.filter)
            .hint_text("Search commands")
            .desired_width(f32::INFINITY),
    )
    .hover("Narrows the list to the commands whose name or keys contain this");
    let want = d.filter.trim().to_lowercase();
    let entries = shortcuts::entries(recent, &d.overrides);
    let mut menu = "";
    for e in &entries {
        let chord = e.shortcut.map(|s| s.label()).unwrap_or_default();
        let hit = want.is_empty()
            || e.label.to_lowercase().contains(&want)
            || e.menu.to_lowercase().contains(&want)
            || chord.to_lowercase().contains(&want);
        if !hit {
            continue;
        }
        if e.menu != menu {
            menu = e.menu;
            ui.add_space(4.0);
            theme::sub_label(ui, menu);
        }
        row(ui, d, e);
    }
    ui.add_space(8.0);
    if ui
        .add_enabled(!d.overrides.is_empty(), egui::Button::new("Reset all"))
        .hover("Puts every shortcut back to Rustroom's own")
        .clicked()
    {
        d.overrides.clear();
        d.note = None;
        d.recording = None;
    }
}

/// One command: its name, its shortcut button, a Reset when it differs from its own, and the note.
fn row(ui: &mut egui::Ui, d: &mut KeyboardDraft, e: &shortcuts::Entry) {
    let fixed = shortcuts::FIXED.contains(&e.command);
    let changed = d
        .overrides
        .iter()
        .any(|r| r.command == shortcuts::command_key(e.command));
    // ⚠ Pinned to its width, never a bare `with_layout(right_to_left)` — the 2026-09-19 growth bug.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 2.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(&e.label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if d.recording == Some(e.command) {
                    "Press keys…".to_string()
                } else {
                    e.shortcut.map(|s| s.label()).unwrap_or_else(|| "—".into())
                };
                let r = ui.add_enabled(
                    !fixed,
                    egui::Button::new(text).min_size(egui::vec2(96.0, 0.0)),
                );
                let name = format!("Shortcut for {}", e.label);
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, !fixed, &name)
                });
                let r = if fixed {
                    // ⚠ A disabled control's hover is not its description — recorded by hand, so a
                    // screen reader and `hover_coverage` still get the sentence.
                    let why = "Keeps the system's own edit keys";
                    theme::describe(&r, why);
                    r.on_disabled_hover_text(why)
                } else {
                    r.hover("Records the next keys you press as this command's shortcut")
                };
                if r.clicked() {
                    d.recording = Some(e.command);
                    d.note = None;
                }
                if changed
                    && ui
                        .small_button("Reset")
                        .hover("Puts back this command's own shortcut")
                        .clicked()
                {
                    shortcuts::reset(&mut d.overrides, e.command);
                    d.note = None;
                }
            });
        },
    );
    if let Some((c, text, refused)) = &d.note {
        if *c == e.command {
            if *refused {
                theme::error_hint(ui, text);
            } else {
                theme::hint(ui, text);
            }
        }
    }
}

/// Read this frame's keys into the command being recorded.
///
/// ⚠ ⌘C, ⌘V and ⌘X arrive as `Copy`, `Paste` and `Cut` — the window turns them into those before a
/// key exists — so they are read as the chords they are, and refused.
fn record(ui: &egui::Ui, d: &mut KeyboardDraft, c: Command) {
    let events = ui.input(|i| i.events.clone());
    for ev in events {
        let chord = match ev {
            egui::Event::Copy => Some(Shortcut::cmd('C')),
            egui::Event::Cut => Some(Shortcut::cmd('X')),
            egui::Event::Paste(_) => Some(Shortcut::cmd('V')),
            egui::Event::Key {
                key: egui::Key::Delete | egui::Key::Backspace,
                pressed: true,
                ..
            } => {
                shortcuts::assign(&mut d.overrides, c, None);
                d.recording = None;
                d.note = Some((c, "Has no shortcut now".to_string(), false));
                return;
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => crate::shell::keys::key_char(key).map(|k| Shortcut {
                cmd: modifiers.command,
                shift: modifiers.shift,
                alt: modifiers.alt,
                key: k,
            }),
            _ => None,
        };
        let Some(sc) = chord else { continue };
        d.recording = None;
        if let Some(why) = shortcuts::refusal(sc) {
            d.note = Some((c, format!("{} — {why}", sc.label()), true));
            return;
        }
        d.note = shortcuts::assign(&mut d.overrides, c, Some(sc)).map(|lost| {
            (
                c,
                format!("Taken from {}", shortcuts::label_of(lost)),
                false,
            )
        });
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// Frames of the tab, its draft kept between them.
    struct Harness {
        ctx: egui::Context,
        d: KeyboardDraft,
        t: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                ctx: test_ui::ctx(),
                d: KeyboardDraft::default(),
                t: 10.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            // ⚠ On from the context's own clock: `test_ui::click` runs frames of its own, and egui
            // refuses a time that goes backwards.
            self.t = self.t.max(self.ctx.input(|i| i.time)) + 0.05;
            let d = &mut self.d;
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 4000.0),
                    )),
                    time: Some(self.t),
                    modifiers,
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| show(ui, d, &[]));
                },
            );
        }

        fn click(&mut self, label: &str) {
            let d = &mut self.d;
            test_ui::click(&self.ctx, label, |ui| show(ui, d, &[]));
        }

        fn press(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            self.frame(
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
            );
        }

        fn of(&self, c: Command) -> Option<Shortcut> {
            shortcuts::shortcut_of(c, &self.d.overrides)
        }
    }

    fn cmd_alt() -> egui::Modifiers {
        egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        }
    }

    /// Click a command's shortcut, press ⌥⌘S: it is that command's now.
    #[test]
    fn clicking_and_pressing_sets_the_shortcut() {
        let mut h = Harness::new();
        assert_eq!(h.of(Command::TakeSnapshot), None, "the sample");
        h.click("Shortcut for Take snapshot");
        assert_eq!(h.d.recording, Some(Command::TakeSnapshot));
        h.press(egui::Key::S, cmd_alt());
        assert_eq!(
            h.of(Command::TakeSnapshot),
            Some(Shortcut::cmd('S').with_alt())
        );
        assert_eq!(h.d.recording, None, "one chord, then done");
    }

    /// ⚠⚠ A chord another command holds moves here, and the note names the one that lost it.
    #[test]
    fn a_taken_chord_moves_and_says_from_whom() {
        let mut h = Harness::new();
        h.click("Shortcut for Take snapshot");
        h.press(egui::Key::G, egui::Modifiers::COMMAND);
        assert_eq!(h.of(Command::TakeSnapshot), Some(Shortcut::cmd('G')));
        assert_eq!(h.of(Command::Stack), None);
        let note = h.d.note.clone().expect("a note");
        assert_eq!((note.1.as_str(), note.2), ("Taken from Stack", false));
    }

    /// A culling key is refused, and the shortcut stays what it was.
    #[test]
    fn a_refused_chord_says_why_and_changes_nothing() {
        let mut h = Harness::new();
        h.click("Shortcut for Stack");
        h.press(egui::Key::Num5, egui::Modifiers::NONE);
        assert_eq!(h.of(Command::Stack), Some(Shortcut::cmd('G')));
        let note = h.d.note.clone().expect("a note");
        assert!(note.2 && note.1.contains("Rates"), "{note:?}");
    }

    /// Delete clears; Reset puts the default back; Reset all clears every change.
    #[test]
    fn delete_clears_and_reset_restores() {
        let mut h = Harness::new();
        h.click("Shortcut for Stack");
        h.press(egui::Key::Delete, egui::Modifiers::NONE);
        assert_eq!(h.of(Command::Stack), None);
        h.click("Reset");
        assert_eq!(h.of(Command::Stack), Some(Shortcut::cmd('G')));
        h.click("Shortcut for Unstack");
        h.press(egui::Key::Delete, egui::Modifiers::NONE);
        assert!(!h.d.overrides.is_empty(), "the sample");
        // ⚠ Narrowed first: the full list runs past the harness's screen, and a click there lands on
        // nothing.
        h.d.filter = "stack".into();
        h.click("Reset all");
        assert!(h.d.overrides.is_empty());
    }

    /// The fixed rows cannot be clicked into recording.
    #[test]
    fn the_system_edit_keys_cannot_be_recorded() {
        let mut h = Harness::new();
        h.click("Shortcut for Undo");
        assert_eq!(h.d.recording, None);
    }

    /// Escape while recording stops it — and is taken, so the dialog does not see it.
    #[test]
    fn escape_stops_recording_and_is_taken() {
        let ctx = egui::Context::default();
        let mut d = KeyboardDraft {
            recording: Some(Command::Stack),
            ..KeyboardDraft::default()
        };
        let mut seen_after = true;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                assert!(d.take_escape(ui.ctx()));
                seen_after = ui.input(|i| i.key_pressed(egui::Key::Escape));
            },
        );
        assert_eq!(d.recording, None);
        assert!(!seen_after, "the dialog must not see the Escape");
    }

    /// The search narrows the list.
    #[test]
    fn the_search_narrows_the_list() {
        let mut h = Harness::new();
        h.d.filter = "stack".into();
        let labels: Vec<String> = test_ui::nodes(&h.ctx, |ui| show(ui, &mut h.d, &[]))
            .into_iter()
            .map(|n| n.label)
            .collect();
        assert!(
            labels.iter().any(|l| l == "Shortcut for Stack"),
            "{labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "Shortcut for Export…"),
            "{labels:?}"
        );
    }
}

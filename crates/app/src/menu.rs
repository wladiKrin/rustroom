//! The menu model: what the menus contain, independent of how they are drawn.
//!
//! ⚠ **No egui and no muda here.** `lib.rs`'s rule keeps these units testable without a
//! windowing system, and this module has two renderers — an in-window `egui::MenuBar` and a
//! native macOS NSMenu — which is exactly why the model must not know about either.

use crate::tool::Tool;
use std::path::{Path, PathBuf};

/// Everything the app can be asked to do from a menu, a key, or the rail.
///
/// ⚠ One enum with **one exhaustive dispatcher**, so a command that nothing performs is a
/// compile error rather than a menu item that silently does nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Command {
    OpenFile,
    OpenFolder,
    /// Open the n-th remembered folder, most recent first (`Settings::recent_folders`).
    OpenRecent(usize),
    Import,
    Export,
    /// Build rendered previews for the selection, or the open photo (M9.14).
    BuildPreviews,
    Settings,
    Undo,
    Redo,
    TakeSnapshot,
    CreateVirtualCopy,
    DeleteVirtualCopy,
    CopySettings,
    PasteSettings,
    PasteSettingsWith,
    SyncSelection,
    ResetAdjustments,
    ShowLibrary,
    ShowDevelop,
    ShowCompare,
    ShowSurvey,
    ZoomIn,
    ZoomOut,
    FitView,
    ZoomActual,
    ToggleBeforeAfter,
    ToggleFilmstrip,
    /// Stack the selection by hand (M10.8c) — ⌘G.
    Stack,
    /// Take apart the hand stacks the selection touches — ⇧⌘G.
    Unstack,
    /// Make the active photo the top of its hand stack.
    MakeTopOfStack,
    /// Stack runs of photos taken within this many seconds of each other; `None` takes the time stacks
    /// apart.
    StackByTime(Option<u32>),
    /// Hide or show the panels, the rail and the filmstrip — Tab, as in Lightroom.
    TogglePanels,
    /// Cycle the surround: normal → dimmed → black, the picture alone — L, as in Lightroom.
    LightsOut,
    /// A capture-info card over the picture's corner — I, as in Lightroom.
    ToggleInfoOverlay,
    /// Clipped highlights in red and clipped shadows in blue over the picture — J, as in Lightroom.
    ToggleClipping,
    ToggleMaskOverlay,
    CycleCompositionGuide,
    SelectTool(Tool),
    RotateLeft,
    RotateRight,
    SavePreset,
    KeyboardShortcuts,
    About,
    /// Show the canvas through an output space, or `None` to stop proofing.
    ///
    /// ⚠ A **view** mode, which is why it lives here beside Before/After rather than in the export
    /// dialog — proofing what you are about to write is the obvious instinct, and it would make a
    /// canvas view mode reachable only from a modal that covers the canvas.
    SoftProof(Option<color::OutputSpace>),
    /// Toggle the marking of pixels the proof space cannot hold.
    GamutWarning,
    /// How the canvas brings colour sRGB cannot hold inside it (M6.4c).
    ///
    /// ⚠ Beside the proof on purpose: flipping the intent while looking at a saturated subject is
    /// the comparison that decides it, and a Settings dialog would cover the canvas.
    RenderingIntent(color::RenderingIntent),
}

/// A keyboard chord, in the model's own terms so neither renderer's types leak in here.
///
/// `key` is the uppercase character or digit as printed on the key: `'O'`, `'Z'`, `'1'`,
/// `'\\'`, `'['`, `','`, `'0'`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shortcut {
    /// Cmd on macOS, Ctrl elsewhere.
    pub cmd: bool,
    pub shift: bool,
    pub key: char,
}

impl Shortcut {
    pub const fn cmd(key: char) -> Self {
        Self {
            cmd: true,
            shift: false,
            key,
        }
    }
    pub const fn cmd_shift(key: char) -> Self {
        Self {
            cmd: true,
            shift: true,
            key,
        }
    }
    pub const fn bare(key: char) -> Self {
        Self {
            cmd: false,
            shift: false,
            key,
        }
    }

    /// Whether this chord may be registered as a **native** menu accelerator.
    ///
    /// ⚠⚠ The rule that removes the whole text-field hazard, and neither half is optional.
    ///
    /// A native accelerator is consumed by the menu *before* the window sees the key. So a
    /// **bare key** would steal every `g`, `d` or `2` typed into the Save-preset name field,
    /// and a **text-editing chord** (⌘Z/C/V/X/A) would break editing inside it. Everything
    /// else — ⌘O, ⌘E, ⌘, , ⌘[ — has no meaning in a text field and is safe.
    ///
    /// The unsafe ones are still shown in the menu and still work: egui handles them, gated on
    /// `memory().focused()` exactly as it does today.
    pub fn safe_as_native_accelerator(self) -> bool {
        self.cmd && !matches!(self.key, 'Z' | 'C' | 'V' | 'X' | 'A')
    }

    /// How the chord is written for a human, e.g. `⌘O` on macOS, `Ctrl+O` elsewhere.
    pub fn label(self) -> String {
        if cfg!(target_os = "macos") {
            format!(
                "{}{}{}",
                if self.shift { "⇧" } else { "" },
                if self.cmd { "⌘" } else { "" },
                key_name(self.key)
            )
        } else {
            let mut s = String::new();
            if self.cmd {
                s.push_str("Ctrl+");
            }
            if self.shift {
                s.push_str("Shift+");
            }
            s.push_str(&key_name(self.key));
            s
        }
    }
}

/// One entry in a menu.
/// How a key character reads in a chord label: itself, except the one control character the
/// model uses, which has a name.
fn key_name(c: char) -> String {
    match c {
        '\t' => "Tab".to_owned(),
        c => c.to_string(),
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    Action {
        /// ⚠ A `String` rather than `&'static str` because the soft-proof entries are built from
        /// [`color::OutputSpace::DISPLAYS`] and name themselves through `OutputSpace::label`. A
        /// second static table of space names beside that one is exactly how the menu and the
        /// export dropdown would come to call the same space different things.
        label: String,
        command: Command,
        shortcut: Option<Shortcut>,
    },
    Separator,
    /// A nested menu. ⚠ Every consumer recurses through [`walk`], the one traversal, so an
    /// item inside a submenu cannot be reachable from one renderer and invisible to another.
    Submenu {
        label: String,
        items: Vec<Item>,
    },
}

/// One top-level menu.
#[derive(Clone, Debug)]
pub struct Section {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn action(label: impl Into<String>, command: Command, shortcut: Option<Shortcut>) -> Item {
    Item::Action {
        label: label.into(),
        command,
        shortcut,
    }
}

/// The menu bar's contents.
///
/// ⚠ The **Photo** section is what makes the rail keyboard-reachable, numbered 1–4 the way
/// Lightroom numbers its modules. Those are bare keys, so they never become native
/// accelerators — see [`Shortcut::safe_as_native_accelerator`].
pub fn model() -> Vec<Section> {
    model_with(&[])
}

/// The model with "Open recent" filled from the remembered folders, most recent first.
pub fn model_with(recent: &[PathBuf]) -> Vec<Section> {
    vec![
        Section {
            title: "File",
            items: file_items(recent),
        },
        Section {
            title: "Edit",
            items: vec![
                action("Undo", Command::Undo, Some(Shortcut::cmd('Z'))),
                action("Redo", Command::Redo, Some(Shortcut::cmd_shift('Z'))),
                Item::Separator,
                action(
                    "Copy settings",
                    Command::CopySettings,
                    Some(Shortcut::cmd('C')),
                ),
                action(
                    "Paste settings",
                    Command::PasteSettings,
                    Some(Shortcut::cmd('V')),
                ),
                action("Paste settings…", Command::PasteSettingsWith, None),
                Item::Separator,
                action("Sync selection…", Command::SyncSelection, None),
                action("Reset adjustments", Command::ResetAdjustments, None),
            ],
        },
        Section {
            title: "View",
            items: view_items(),
        },
        Section {
            title: "Photo",
            items: vec![
                action(
                    "Edit",
                    Command::SelectTool(Tool::Edit),
                    Some(Shortcut::bare('E')),
                ),
                action(
                    "Crop & Geometry",
                    Command::SelectTool(Tool::Crop),
                    Some(Shortcut::bare('R')),
                ),
                action(
                    "Masks",
                    Command::SelectTool(Tool::Masks),
                    Some(Shortcut::bare('K')),
                ),
                action(
                    "Retouch",
                    Command::SelectTool(Tool::Retouch),
                    Some(Shortcut::bare('Q')),
                ),
                action(
                    "Presets",
                    Command::SelectTool(Tool::Presets),
                    Some(Shortcut::bare('T')),
                ),
                Item::Separator,
                // ⚠ ⌘' **is** safe as a native accelerator, and it is worth knowing why: the
                // hazard is a **bare** key (stolen from every text field) or a text-editing
                // chord (⌘Z/C/V/X/A, which break editing inside one). A plain apostrophe typed
                // into the copy-rename field is not the ⌘' chord, so nothing is swallowed.
                // ⚠ **No shortcut, and that is a decision.** A bare key is stolen from every
                // text field and a text-editing chord breaks editing inside one — see
                // `Shortcut::safe_as_native_accelerator`. Lightroom binds none to this either.
                action("Take snapshot", Command::TakeSnapshot, None),
                action(
                    "Create virtual copy",
                    Command::CreateVirtualCopy,
                    Some(Shortcut::cmd('\'')),
                ),
                action("Delete virtual copy", Command::DeleteVirtualCopy, None),
                Item::Separator,
                action("Rotate left", Command::RotateLeft, Some(Shortcut::cmd('['))),
                action(
                    "Rotate right",
                    Command::RotateRight,
                    Some(Shortcut::cmd(']')),
                ),
                Item::Separator,
                // M10.8c: stacks by hand. ⌘G is safe as a native accelerator — not a text chord.
                action("Stack", Command::Stack, Some(Shortcut::cmd('G'))),
                action("Unstack", Command::Unstack, Some(Shortcut::cmd_shift('G'))),
                action("Make top of stack", Command::MakeTopOfStack, None),
                Item::Submenu {
                    label: "Stack by capture time".to_owned(),
                    items: STACK_GAPS
                        .iter()
                        .map(|&(label, secs)| action(label, Command::StackByTime(secs), None))
                        .collect(),
                },
                Item::Separator,
                action("Save as preset…", Command::SavePreset, None),
                Item::Separator,
                action("Build previews", Command::BuildPreviews, None),
            ],
        },
        Section {
            title: "Help",
            items: vec![
                action("Keyboard shortcuts", Command::KeyboardShortcuts, None),
                action("About Rustroom", Command::About, None),
            ],
        },
    ]
}

/// What the menus' check marks reflect (M10.8b).
///
/// ⚠ The layout's three choices were View items for one build and moved to Settings → Appearance at
/// the author's word (2026-10-01): where a panel sits is a setting, not a view toggle. The marks stayed
/// for the toggles that are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    pub filmstrip_shown: bool,
}

/// Whether `command`'s item is checked — `None` for an ordinary action, which carries no mark.
///
/// ⚠ From the **command**, so `Item` keeps its shape and both renderers ask the same question.
pub fn checked(command: Command, s: &MenuState) -> Option<bool> {
    match command {
        Command::ToggleFilmstrip => Some(s.filmstrip_shown),
        _ => None,
    }
}

/// Stack by capture time's gaps (M10.8c): what the submenus offer, and in seconds.
///
/// ⚠ *None* first (the author, after use): it takes apart the stacks this menu made, and only those.
pub const STACK_GAPS: [(&str, Option<u32>); 7] = [
    ("None", None),
    ("Within 1 second", Some(1)),
    ("Within 2 seconds", Some(2)),
    ("Within 5 seconds", Some(5)),
    ("Within 10 seconds", Some(10)),
    ("Within 30 seconds", Some(30)),
    ("Within 1 minute", Some(60)),
];

/// The View menu's items.
///
/// ⚠ Its own function because the soft-proof entries are **built from a list** rather than typed
/// out, which a `vec![]` literal cannot express — and because appending to a menu that is already
/// the longest one reads better as a sequence of pushes than as a wall of literals.
fn view_items() -> Vec<Item> {
    let mut items = vec![
        action("Library", Command::ShowLibrary, Some(Shortcut::bare('G'))),
        action("Develop", Command::ShowDevelop, Some(Shortcut::bare('D'))),
        // ⚠⚠ Bare `C` is safe, and that was **verified in egui-winit's source** rather
        // than assumed. `is_copy_command` is `modifiers.command && key == C`, so only
        // the ⌘C chord is swallowed into `Event::Copy`; a bare `C` emits an ordinary
        // `Event::Key`. Being bare, neither ever becomes a native accelerator — see
        // `safe_as_native_accelerator` — so neither can steal a character typed into
        // the search field or the new-collection field.
        action("Compare", Command::ShowCompare, Some(Shortcut::bare('C'))),
        action("Survey", Command::ShowSurvey, Some(Shortcut::bare('N'))),
        Item::Separator,
        action("Zoom in", Command::ZoomIn, Some(Shortcut::cmd('+'))),
        action("Zoom out", Command::ZoomOut, Some(Shortcut::cmd('-'))),
        action("Zoom to fit", Command::FitView, Some(Shortcut::cmd('0'))),
        action("Zoom to 1:1", Command::ZoomActual, Some(Shortcut::cmd('1'))),
        action(
            "Before / After",
            Command::ToggleBeforeAfter,
            Some(Shortcut::bare('\\')),
        ),
        Item::Separator,
        action("Filmstrip", Command::ToggleFilmstrip, None),
        action("Show all mask overlays", Command::ToggleMaskOverlay, None),
        // ⚠ A **bare** key, so it never becomes a native accelerator — see
        // `safe_as_native_accelerator`. `O` is Lightroom's, which is the whole argument
        // for it; ⌘O is Open file and the two chords are distinct.
        action(
            "Composition guide",
            Command::CycleCompositionGuide,
            Some(Shortcut::bare('O')),
        ),
        action(
            "Hide panels",
            Command::TogglePanels,
            Some(Shortcut::bare('\t')),
        ),
        action("Lights out", Command::LightsOut, Some(Shortcut::bare('L'))),
        action(
            "Info overlay",
            Command::ToggleInfoOverlay,
            Some(Shortcut::bare('I')),
        ),
        action(
            "Clipping overlay",
            Command::ToggleClipping,
            Some(Shortcut::bare('J')),
        ),
    ];

    // ⚠⚠ **Appended, never inserted.** `PARALLEL-WORK.md` lists this menu among the surfaces any
    // new dialog or shortcut contends for; appending is what keeps two tracks' additions from
    // colliding in the middle of a list.
    //
    // ⚠ Built from `DISPLAYS`, not `ALL`: no ProPhoto monitor exists, and proofing into the
    // working space is the identity, so the entry would be a control that does nothing.
    items.push(Item::Separator);
    // ⚠ One submenu for the lot: soft proofing and rendering intents are colour-management
    // terms a first-time user has no use for, and seven of them at the top level of View were
    // the first thing both first-use reviews called jargon.
    let mut colour = vec![action("Soft proof: off", Command::SoftProof(None), None)];
    for space in color::OutputSpace::DISPLAYS {
        colour.push(action(
            format!("Soft proof: {}", space.label()),
            Command::SoftProof(Some(space)),
            None,
        ));
    }
    // ⚠ A separate entry from the proof itself, deliberately: proofing without the warning is the
    // common case, and a warning you cannot switch off is chrome.
    colour.push(action("Gamut warning", Command::GamutWarning, None));
    colour.push(Item::Separator);
    for intent in color::RenderingIntent::ALL {
        colour.push(action(
            format!("Rendering intent: {}", intent.label()),
            Command::RenderingIntent(intent),
            None,
        ));
    }
    items.push(Item::Submenu {
        label: "Colour management".to_owned(),
        items: colour,
    });

    // ⚠ **No shortcuts on any of these.** A bare key is stolen from every text field and a
    // text-editing chord breaks editing inside one — `Shortcut::safe_as_native_accelerator`
    // records the rule — and there is no obvious unclaimed ⌘ chord for a five-entry family.
    items
}

fn file_items(recent: &[PathBuf]) -> Vec<Item> {
    let mut items = vec![
        action("Open file…", Command::OpenFile, Some(Shortcut::cmd('O'))),
        action(
            "Open folder…",
            Command::OpenFolder,
            Some(Shortcut::cmd_shift('O')),
        ),
    ];
    if !recent.is_empty() {
        items.push(Item::Submenu {
            label: "Open recent".to_owned(),
            items: recent
                .iter()
                .enumerate()
                .map(|(i, p)| action(folder_name(p), Command::OpenRecent(i), None))
                .collect(),
        });
    }
    items.extend([
        Item::Separator,
        action("Import…", Command::Import, Some(Shortcut::cmd_shift('I'))),
        action("Export…", Command::Export, Some(Shortcut::cmd('E'))),
        Item::Separator,
        action("Settings…", Command::Settings, Some(Shortcut::cmd(','))),
    ]);
    items
}

/// A folder's own name for a menu entry, or the whole path when it has none (a volume root).
pub fn folder_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// A second bare key for a command that already has one, for a hand that knows Lightroom:
/// `Y` is its Compare. Read by the shortcut matcher after the model, listed by the shortcuts
/// dialog, and checked against the model and the culling keys by the tests.
pub const ALIASES: &[(char, Command)] = &[('Y', Command::ShowCompare)];

/// Visit every item in menu order, descending into submenus.
///
/// ⚠ **The one traversal.** The shortcut matcher, the shortcuts dialog, the glyph audit and the
/// tests all go through it, so a nested item cannot be reached by one and missed by another.
pub fn walk<'a, F: FnMut(&'a Item)>(items: &'a [Item], f: &mut F) {
    for item in items {
        f(item);
        if let Item::Submenu { items, .. } = item {
            walk(items, f);
        }
    }
}

/// Every command the model contains, in menu order, submenus included.
pub fn commands() -> Vec<Command> {
    let mut out = Vec::new();
    for s in model() {
        walk(&s.items, &mut |i| {
            if let Item::Action { command, .. } = i {
                out.push(*command);
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_items_share_a_shortcut() {
        // ⚠ Two items on one chord means one of them is unreachable, and which one wins
        // depends on iteration order — the kind of bug that looks like "the menu is flaky".
        let mut seen: Vec<Shortcut> = Vec::new();
        for s in model() {
            walk(&s.items, &mut |i| {
                if let Item::Action {
                    shortcut: Some(sc), ..
                } = i
                {
                    seen.push(*sc);
                }
            });
        }
        let total = seen.len();
        seen.sort_by_key(|s| (s.cmd, s.shift, s.key));
        seen.dedup();
        assert_eq!(seen.len(), total, "two menu items share a shortcut");
    }

    #[test]
    fn every_command_appears_exactly_once() {
        // A command in two places is two entry points that will drift apart.
        let mut c = commands();
        let total = c.len();
        c.sort_by_key(|c| format!("{c:?}"));
        c.dedup();
        assert_eq!(c.len(), total, "a command appears twice in the menu");
    }

    #[test]
    fn every_tool_is_reachable_from_the_photo_menu() {
        // ⚠ Stated over `Tool::ALL`, so a fifth tool cannot ship with no way to select it by
        // keyboard — which would compile and look fine.
        let cmds = commands();
        for t in Tool::ALL {
            assert!(
                cmds.contains(&Command::SelectTool(t)),
                "{t:?} has no menu entry"
            );
        }
    }

    #[test]
    fn no_bare_key_or_text_chord_is_a_native_accelerator() {
        // ⚠⚠ The load-bearing safety rule. A bare key as a native accelerator steals every
        // such character typed into a text field; ⌘C/⌘V/⌘Z break editing inside one.
        for s in [
            Shortcut::bare('G'),
            Shortcut::bare('1'),
            Shortcut::bare('\\'),
            // ⚠ The composition-guide key. Bare, so every `o` typed into the Save-preset name
            // field would be swallowed if this ever became native — and ⌘O below must stay safe,
            // so the two cases share a character and differ only in the modifier.
            Shortcut::bare('O'),
            // ⚠ Compare and Survey. Bare `C` shares its character with ⌘C, which is a *text*
            // chord and unsafe for the other reason — so this pair covers both halves of the
            // rule at once.
            Shortcut::bare('C'),
            Shortcut::bare('N'),
            Shortcut::cmd('Z'),
            Shortcut::cmd('C'),
            Shortcut::cmd('V'),
            Shortcut::cmd_shift('Z'),
        ] {
            assert!(
                !s.safe_as_native_accelerator(),
                "{s:?} must not become a native accelerator"
            );
        }
        for s in [Shortcut::cmd('O'), Shortcut::cmd('E'), Shortcut::cmd(',')] {
            assert!(
                s.safe_as_native_accelerator(),
                "{s:?} is safe and should be native"
            );
        }
    }

    #[test]
    fn every_display_space_is_reachable_as_a_proof() {
        // ⚠ Stated over `DISPLAYS`, so a space added there cannot ship with no way to proof into
        // it — which would compile and look fine.
        let cmds = commands();
        for s in color::OutputSpace::DISPLAYS {
            assert!(
                cmds.contains(&Command::SoftProof(Some(s))),
                "{} has no proof entry",
                s.label()
            );
        }
        // ⚠ Both the way in and the way out. A proof that cannot be switched off is a mode the
        // user is stuck in until they change photo.
        assert!(cmds.contains(&Command::SoftProof(None)), "no way to stop");
        assert!(cmds.contains(&Command::GamutWarning));
    }

    /// ⚠ Stated over `RenderingIntent::ALL`, so an intent added there cannot ship unreachable.
    #[test]
    fn every_rendering_intent_is_reachable() {
        let cmds = commands();
        for i in color::RenderingIntent::ALL {
            assert!(
                cmds.contains(&Command::RenderingIntent(i)),
                "{} has no menu entry",
                i.label()
            );
        }
    }

    #[test]
    fn the_working_space_is_not_offered_as_a_proof() {
        // ⚠ Proofing into ProPhoto is the identity — `from_working` returns its argument — so the
        // entry would be a control that provably does nothing. Building the list from
        // `OutputSpace::ALL` is exactly how it would creep back in.
        assert!(!commands().contains(&Command::SoftProof(Some(color::OutputSpace::ProPhoto))));
    }

    #[test]
    fn the_proof_entries_name_their_spaces_the_way_the_rest_of_the_app_does() {
        // ⚠ The labels come from `OutputSpace::label`, not from a second table typed out here.
        // Two tables is how the View menu and the export dropdown come to call one space by two
        // names — and the menu's would be the one nobody notices is wrong.
        let mut labels: Vec<String> = Vec::new();
        for s in model() {
            walk(&s.items, &mut |i| {
                if let Item::Action { label, .. } = i {
                    labels.push(label.clone());
                }
            });
        }
        for s in color::OutputSpace::DISPLAYS {
            assert!(
                labels.iter().any(|l| l.contains(s.label())),
                "no menu entry names {}",
                s.label()
            );
        }
    }

    #[test]
    fn every_section_has_a_title_and_at_least_one_action() {
        // An empty menu renders as a title that opens nothing.
        for s in model() {
            assert!(!s.title.is_empty());
            assert!(
                s.items.iter().any(|i| matches!(i, Item::Action { .. })),
                "{} has no actions",
                s.title
            );
        }
    }

    #[test]
    fn every_shortcut_has_a_label() {
        for s in model().into_iter().flat_map(|s| s.items) {
            if let Item::Action {
                shortcut: Some(sc),
                label,
                ..
            } = s
            {
                assert!(
                    !sc.label().is_empty(),
                    "{label} has an unprintable shortcut"
                );
            }
        }
    }

    /// ⚠⚠ Digits and P, X, U are the culling keys in every view, Develop included, so no menu
    /// item may sit on one as a bare shortcut — the tools used to, and rating a photo in
    /// Develop switched tools instead.
    #[test]
    fn no_bare_shortcut_is_a_culling_key() {
        for s in model() {
            walk(&s.items, &mut |i| {
                if let Item::Action {
                    label,
                    shortcut: Some(sc),
                    ..
                } = i
                {
                    assert!(
                        sc.cmd || !(sc.key.is_ascii_digit() || matches!(sc.key, 'P' | 'X' | 'U')),
                        "{label} sits on the culling key {}",
                        sc.key
                    );
                }
            });
        }
    }

    /// The File menu names the remembered folders, most recent first, and drops the submenu
    /// when there are none.
    #[test]
    fn open_recent_names_the_folders_in_order() {
        let recent = [PathBuf::from("/p/Katzen"), PathBuf::from("/p/Concert 2026")];
        let file = &model_with(&recent)[0];
        let sub = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Submenu { label, items } if label == "Open recent" => Some(items),
                _ => None,
            })
            .expect("an Open recent submenu");
        let names: Vec<(&str, Command)> = sub
            .iter()
            .filter_map(|i| match i {
                Item::Action { label, command, .. } => Some((label.as_str(), *command)),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec![
                ("Katzen", Command::OpenRecent(0)),
                ("Concert 2026", Command::OpenRecent(1))
            ]
        );
        assert!(!model()[0]
            .items
            .iter()
            .any(|i| matches!(i, Item::Submenu { .. })));
    }

    /// An alias must be a bare key nothing else uses — not a model shortcut, not a culling key —
    /// and must name a command the model has.
    #[test]
    fn aliases_collide_with_nothing() {
        let mut bare: Vec<char> = Vec::new();
        for s in model() {
            walk(&s.items, &mut |i| {
                if let Item::Action {
                    shortcut: Some(sc), ..
                } = i
                {
                    if !sc.cmd {
                        bare.push(sc.key);
                    }
                }
            });
        }
        let cmds = commands();
        for (key, cmd) in ALIASES {
            assert!(!bare.contains(key), "{key} is already a menu shortcut");
            assert!(
                !(key.is_ascii_digit() || matches!(key, 'P' | 'X' | 'U')),
                "{key} is a culling key"
            );
            assert!(cmds.contains(cmd), "{cmd:?} is not in the menu");
        }
    }

    #[test]
    fn tab_reads_as_tab() {
        assert_eq!(Shortcut::bare('\t').label(), "Tab");
    }

    /// The *Filmstrip* toggle's mark follows the state; an ordinary action has none.
    #[test]
    fn the_filmstrip_mark_follows_the_state() {
        let shown = MenuState {
            filmstrip_shown: true,
        };
        let hidden = MenuState {
            filmstrip_shown: false,
        };
        assert_eq!(checked(Command::ToggleFilmstrip, &shown), Some(true));
        assert_eq!(checked(Command::ToggleFilmstrip, &hidden), Some(false));
        assert_eq!(checked(Command::ZoomIn, &shown), None, "an ordinary action");
    }
}

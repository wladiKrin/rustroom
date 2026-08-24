//! The macOS system menu bar, built from the same model the in-window bar uses.
//!
//! ⚠ **The only `#[cfg]`-gated module in the app, and deliberately thin.** Linux CI never
//! compiles this, so every line here is unverified by CI — keep the decisions in `menu.rs` and
//! `commands.rs`, which are built everywhere, and keep this to installation and plumbing.

use eframe::egui;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use rustroom_app::menu::{self, Command, Item};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// The live menu and its id → command map.
    ///
    /// ⚠ Thread-local rather than a field on `RustroomApp`, for a concrete reason: muda's
    /// types are `!Send`/`!Sync` and macOS menus panic if touched off the main thread. Keeping
    /// them here means `RustroomApp` stays plain and no `#[cfg]` leaks into its definition.
    static MENU: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

struct Installed {
    /// Held only to keep the menu alive; never read.
    _menu: Menu,
    by_id: HashMap<MenuId, Command>,
}

/// Build and install the menu. Call once, from the app's constructor.
pub fn install(ctx: &egui::Context) {
    let menu = Menu::new();
    let mut by_id = HashMap::new();

    for section in menu::model() {
        let sub = Submenu::new(section.title, true);
        for item in section.items {
            match item {
                Item::Separator => {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                }
                Item::Action {
                    label,
                    command,
                    shortcut,
                } => {
                    // ⚠ Only *safe* chords become real accelerators. A bare key would steal
                    // every such character typed into a text field, and ⌘C/⌘V/⌘Z would break
                    // editing inside one — see `Shortcut::safe_as_native_accelerator`. The
                    // rest are shown in the label and handled by egui, focus-gated.
                    let (text, accel) = match shortcut {
                        Some(sc) if sc.safe_as_native_accelerator() => {
                            (label.to_string(), accelerator(sc))
                        }
                        Some(sc) => (format!("{label}   {}", sc.label()), None),
                        None => (label.to_string(), None),
                    };
                    let mi = MenuItem::new(text, true, accel);
                    by_id.insert(mi.id().clone(), command);
                    let _ = sub.append(&mi);
                }
            }
        }
        let _ = menu.append(&sub);
    }

    menu.init_for_nsapp();

    // ⚠ egui does not redraw an idle window, and a menu click generates no winit event — so
    // without an explicit repaint the command would sit in the channel until the user happened
    // to move the mouse. Same class of bug as the full-resolution trigger needing
    // `request_repaint_after`.
    //
    // ⚠ The handler only wakes the frame loop; `poll` below still drains the channel. Do not
    // move the dispatch in here — it would need `&mut RustroomApp` from inside a `'static`
    // closure.
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |_event: MenuEvent| {
        ctx.request_repaint();
    }));

    MENU.with(|m| *m.borrow_mut() = Some(Installed { _menu: menu, by_id }));
}

/// Drain any menu activations since the last frame.
pub fn poll() -> Vec<Command> {
    let mut out = Vec::new();
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        MENU.with(|m| {
            if let Some(installed) = m.borrow().as_ref() {
                if let Some(cmd) = installed.by_id.get(&event.id) {
                    out.push(*cmd);
                }
            }
        });
    }
    out
}

fn accelerator(sc: menu::Shortcut) -> Option<muda::accelerator::Accelerator> {
    use muda::accelerator::{Accelerator, Code, Modifiers, CMD_OR_CTRL};
    let code = match sc.key {
        'O' => Code::KeyO,
        'E' => Code::KeyE,
        ',' => Code::Comma,
        '[' => Code::BracketLeft,
        ']' => Code::BracketRight,
        '0' => Code::Digit0,
        _ => return None,
    };
    let mut mods = Modifiers::empty();
    if sc.cmd {
        // ⚠ `CMD_OR_CTRL` is a **free const in `accelerator`**, not a `Modifiers` associated
        // const, and it resolves to `SUPER` on macOS. Hardcoding `CONTROL` would give every
        // File-menu item the wrong glyph and the wrong key.
        mods |= CMD_OR_CTRL;
    }
    if sc.shift {
        mods |= Modifiers::SHIFT;
    }
    Some(Accelerator::new(Some(mods), code))
}

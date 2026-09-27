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
use std::sync::mpsc;
use std::sync::Mutex;
use std::sync::OnceLock;

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

/// Our own channel, fed by the event handler.
///
/// ⚠⚠ **`MenuEvent::receiver()` is dead once `set_event_handler(Some(..))` is called.** muda's
/// `send` calls the handler *instead of* pushing to its channel — so the first version of this
/// module polled a channel that could never receive anything, and every menu item silently did
/// nothing. The handler must therefore forward the event itself.
///
/// ⚠ A `Mutex` because muda requires the handler to be `Send + Sync`, even though it only ever
/// fires on the main thread.
struct Events {
    tx: Mutex<mpsc::Sender<MenuId>>,
    rx: Mutex<mpsc::Receiver<MenuId>>,
}

static EVENTS: OnceLock<Events> = OnceLock::new();

fn events() -> &'static Events {
    EVENTS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        Events {
            tx: Mutex::new(tx),
            rx: Mutex::new(rx),
        }
    })
}

/// Stop AppKit injecting Emoji & Symbols, Start Dictation and friends into our Edit menu.
///
/// ⚠ **AppKit adds these to any menu titled "Edit", uninvited.** They are not ours and they
/// do nothing useful in a photo editor. The two documented defaults keys suppress them.
///
/// ⚠ `registerDefaults`, **not** `setBool_forKey` — registering applies for this process only,
/// where setting would write into the user's own preferences plist. We have no business
/// changing that.
fn suppress_injected_edit_items() {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{ns_string, NSDictionary, NSUserDefaults};
    let keys = [
        ns_string!("NSDisabledDictationMenuItem"),
        ns_string!("NSDisabledCharacterPaletteMenuItem"),
    ];
    // ⚠ `&**` twice: `ns_string!` gives `&NSString`, which derefs to `&NSObject` and then to
    // `&AnyObject` — the value type `registerDefaults` insists on.
    let yes: [&AnyObject; 2] = [&**ns_string!("YES"), &**ns_string!("YES")];
    let dict = NSDictionary::from_slices(&keys, &yes);
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&dict) };
}

/// Build and install the menu. Call once, from the app's constructor.
pub fn install(ctx: &egui::Context) {
    suppress_injected_edit_items();
    let menu = Menu::new();
    let mut by_id = HashMap::new();

    // ⚠⚠ **The application menu must come first, or macOS eats the first real menu.** AppKit
    // always treats the *first* submenu of the main menu as the app menu and relabels it with
    // the app name, whatever its own title says. Without this, "File" was absorbed — Open
    // file, Open folder and Export appeared under "Rustroom" and there was no File menu at all.
    let app_menu = Submenu::new("Rustroom", true);
    let about = MenuItem::new("About Rustroom", true, None);
    by_id.insert(about.id().clone(), Command::About);
    let _ = app_menu.append(&about);
    let _ = app_menu.append(&PredefinedMenuItem::separator());
    // ⚠ Settings belongs in the application menu on macOS, with ⌘, — that is where every Mac
    // user looks for it. The model keeps it under File for the in-window bar, which has no
    // application menu, and the File rendering below skips it.
    let settings = MenuItem::new("Settings…", true, accelerator(menu::Shortcut::cmd(',')));
    by_id.insert(settings.id().clone(), Command::Settings);
    let _ = app_menu.append(&settings);
    let _ = app_menu.append(&PredefinedMenuItem::separator());
    let _ = app_menu.append(&PredefinedMenuItem::hide(None));
    let _ = app_menu.append(&PredefinedMenuItem::hide_others(None));
    let _ = app_menu.append(&PredefinedMenuItem::show_all(None));
    let _ = app_menu.append(&PredefinedMenuItem::separator());
    let _ = app_menu.append(&PredefinedMenuItem::quit(None));
    let _ = menu.append(&app_menu);

    for section in menu::model() {
        let sub = Submenu::new(section.title, true);
        for item in section.items {
            match item {
                Item::Separator => {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                }
                // ⚠ About and Settings live in the application menu on macOS, where users
                // look for them — so they are skipped here rather than shown twice. The model
                // keeps both in their cross-platform places for the in-window bar, which has
                // no application menu.
                Item::Action {
                    command: Command::About | Command::Settings,
                    ..
                } => {}
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
    // ⚠ The handler does two things and needs both. It **forwards the event to our own
    // channel**, because setting a handler at all stops muda's own `receiver()` from ever
    // seeing it; and it **requests a repaint**, because egui does not redraw an idle window and
    // a menu click generates no winit event, so the command would otherwise sit unread until
    // the mouse moved.
    //
    // ⚠ The dispatch itself stays in `poll` — a `'static` closure cannot hold
    // `&mut RustroomApp`.
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Ok(tx) = events().tx.lock() {
            let _ = tx.send(event.id().clone());
        }
        ctx.request_repaint();
    }));

    MENU.with(|m| *m.borrow_mut() = Some(Installed { _menu: menu, by_id }));
}

/// Drain any menu activations since the last frame.
pub fn poll() -> Vec<Command> {
    let mut out = Vec::new();
    let Ok(rx) = events().rx.lock() else {
        return out;
    };
    while let Ok(id) = rx.try_recv() {
        MENU.with(|m| {
            if let Some(installed) = m.borrow().as_ref() {
                if let Some(cmd) = installed.by_id.get(&id) {
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
        'I' => Code::KeyI,
        'E' => Code::KeyE,
        ',' => Code::Comma,
        '[' => Code::BracketLeft,
        ']' => Code::BracketRight,
        '0' => Code::Digit0,
        '1' => Code::Digit1,
        // ⚠ `+` sits on the `=` key; its physical code is Equal.
        '+' => Code::Equal,
        '-' => Code::Minus,
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

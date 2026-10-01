//! The macOS system menu bar, built from the same model the in-window bar uses.
//!
//! ⚠ **The only `#[cfg]`-gated module in the app, and deliberately thin.** Linux CI never
//! compiles this, so every line here is unverified by CI — keep the decisions in `menu.rs` and
//! `commands.rs`, which are built everywhere, and keep this to installation and plumbing.

use eframe::egui;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
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
    /// The items that carry a check mark (M10.8b), by command — what [`sync`] sets.
    checks: Vec<(Command, CheckMenuItem)>,
    /// The state the marks were last set from; `None` until the first [`sync`].
    synced: Option<menu::MenuState>,
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
        // "Enter Full Screen" in the View menu; the window's own green button still works.
        ns_string!("NSFullScreenMenuItemEverywhere"),
    ];
    // ⚠ `&**` twice: `ns_string!` gives `&NSString`, which derefs to `&NSObject` and then to
    // `&AnyObject` — the value type `registerDefaults` insists on.
    let yes: [&AnyObject; 3] = [
        &**ns_string!("YES"),
        &**ns_string!("YES"),
        &**ns_string!("NO"),
    ];
    let dict = NSDictionary::from_slices(&keys, &yes);
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&dict) };
}

/// Build and install the menu. Call once, from the app's constructor.
pub fn install(ctx: &egui::Context, recent: &[std::path::PathBuf]) {
    suppress_injected_edit_items();
    forbid_window_tabbing();
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

    let mut checks = Vec::new();
    for section in menu::model_with(recent) {
        let sub = Submenu::new(section.title, true);
        append_items(&sub, section.items, &mut by_id, &mut checks);
        let _ = menu.append(&sub);
    }

    menu.init_for_nsapp();
    strip_injected_items();

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

    MENU.with(|m| {
        *m.borrow_mut() = Some(Installed {
            _menu: menu,
            by_id,
            checks,
            synced: None,
        })
    });
}

/// Drain any menu activations since the last frame.
/// Rebuild the menu with a new "Open recent" list — a no-op until [`install`] has run, which is
/// what keeps the headless tests (and any second window) away from AppKit.
pub fn refresh(ctx: &egui::Context, recent: &[std::path::PathBuf]) {
    if MENU.with(|m| m.borrow().is_none()) {
        return;
    }
    install(ctx, recent);
}

/// Set the check marks from `state` (M10.8b). ⚠ Called every frame and cheap: AppKit is touched only
/// when the state differs from the one last set — and after an `install`, which starts unset.
pub fn sync(state: &menu::MenuState) {
    MENU.with(|m| {
        let mut m = m.borrow_mut();
        let Some(installed) = m.as_mut() else { return };
        if installed.synced.as_ref() == Some(state) {
            return;
        }
        for (command, item) in &installed.checks {
            if let Some(on) = menu::checked(*command, state) {
                item.set_checked(on);
            }
        }
        installed.synced = Some(*state);
    });
}

pub fn poll() -> Vec<Command> {
    // ⚠ AppKit adds AutoFill, Emoji & Symbols and Enter Full Screen *after* `install` has
    // stripped the menu — lazily, in the first moments of the run — so the strip repeats on
    // every poll for a while and then stops. Measured: Edit's two were back by the first frame
    // that could be inspected, and "Enter Full Screen" arrived later still, once the window had
    // grown to its remembered size; thirty seconds covers a slow launch under load.
    static STRIP_UNTIL: OnceLock<std::time::Instant> = OnceLock::new();
    let until =
        *STRIP_UNTIL.get_or_init(|| std::time::Instant::now() + std::time::Duration::from_secs(30));
    if std::time::Instant::now() < until {
        strip_injected_items();
    }
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

/// muda reads `&` as a mnemonic marker on every platform and drops it, which turned "Crop &
/// Geometry" into "Crop  Geometry" in the Photo menu; `&&` is its escape for a literal ampersand.
fn native_label(label: &str) -> String {
    label.replace('&', "&&")
}

/// The items AppKit adds to any main menu on its own — "AutoFill" and "Emoji & Symbols" into a
/// menu titled Edit, tab-bar items and "Enter Full Screen" into one titled View. To a new user
/// they read as features that do nothing.
///
/// ⚠ English titles, on purpose: the `NSUserDefaults` keys in
/// [`suppress_injected_edit_items`] are the language-independent half of this and stop most of
/// the injection before it happens; this list is the fallback for the ones AppKit keeps adding
/// (AutoFill has no documented default). A non-English system keeps only that first half.
const INJECTED_TITLES: [&str; 6] = [
    "AutoFill",
    "Emoji & Symbols",
    "Start Dictation…",
    "Show Tab Bar",
    "Show All Tabs",
    "Enter Full Screen",
];

/// Remove AppKit's injected items from every top-level menu, and the separators they leave
/// dangling at either end. Runs once, right after `init_for_nsapp`, when they are already there.
fn strip_injected_items() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(main) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return;
    };
    for i in 0..main.numberOfItems() {
        if let Some(sub) = main.itemAtIndex(i).and_then(|it| it.submenu()) {
            strip_from(&sub);
        }
    }
}

fn strip_from(menu: &objc2_app_kit::NSMenu) {
    let mut i = menu.numberOfItems();
    while i > 0 {
        i -= 1;
        let Some(item) = menu.itemAtIndex(i) else {
            continue;
        };
        let title = item.title().to_string();
        if INJECTED_TITLES.contains(&title.as_str()) {
            menu.removeItem(&item);
        }
    }
    // A separator that now leads or trails the menu belonged to a removed item.
    for at_end in [true, false] {
        loop {
            let n = menu.numberOfItems();
            if n == 0 {
                break;
            }
            let idx = if at_end { n - 1 } else { 0 };
            match menu.itemAtIndex(idx) {
                Some(it) if it.isSeparatorItem() => menu.removeItem(&it),
                _ => break,
            }
        }
    }
}

/// Stop AppKit offering "Show Tab Bar" / "Show All Tabs" in the View menu: window tabbing is an
/// AppKit feature this single-window app never uses.
fn forbid_window_tabbing() {
    use objc2_app_kit::NSWindow;
    use objc2_foundation::MainThreadMarker;
    if let Some(mtm) = MainThreadMarker::new() {
        NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    }
}

/// Append the model's items to a native submenu, recursing into nested submenus.
///
/// ⚠ About and Settings are skipped: they live in the application menu, where macOS expects
/// them, and the app menu is built by hand above.
fn append_items(
    sub: &Submenu,
    items: Vec<Item>,
    by_id: &mut HashMap<MenuId, Command>,
    checks: &mut Vec<(Command, CheckMenuItem)>,
) {
    for item in items {
        match item {
            Item::Separator => {
                let _ = sub.append(&PredefinedMenuItem::separator());
            }
            Item::Submenu { label, items } => {
                let nested = Submenu::new(native_label(&label), true);
                append_items(&nested, items, by_id, checks);
                let _ = sub.append(&nested);
            }
            Item::Action {
                command: Command::About | Command::Settings,
                ..
            } => {}
            Item::Action {
                label,
                command,
                shortcut,
            } => {
                // ⚠ The chord is part of the label for the unsafe ones — see
                // `Shortcut::safe_as_native_accelerator`: a bare key or a text-editing chord as a
                // real accelerator would fire while typing in a field.
                let (text, accel) = match shortcut {
                    Some(sc) if sc.safe_as_native_accelerator() => {
                        (label.to_string(), accelerator(sc))
                    }
                    Some(sc) => (format!("{label}   {}", sc.label()), None),
                    None => (label.to_string(), None),
                };
                // M10.8b: a command with a mark is a checkable item, set by `sync` — not here, since
                // the state is not known at install.
                let unset = menu::MenuState {
                    filmstrip_shown: true,
                };
                if menu::checked(command, &unset).is_some() {
                    let mi = CheckMenuItem::new(native_label(&text), true, false, accel);
                    by_id.insert(mi.id().clone(), command);
                    let _ = sub.append(&mi);
                    checks.push((command, mi));
                } else {
                    let mi = MenuItem::new(native_label(&text), true, accel);
                    by_id.insert(mi.id().clone(), command);
                    let _ = sub.append(&mi);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ampersand_is_escaped_for_muda() {
        assert_eq!(native_label("Crop & Geometry"), "Crop && Geometry");
        assert_eq!(native_label("Open file…"), "Open file…");
    }

    /// The model's own labels all survive the escape: none contains a literal `&&`.
    #[test]
    fn no_menu_label_already_uses_mudas_escape() {
        for section in menu::model() {
            for item in section.items {
                if let Item::Action { label, .. } = item {
                    assert!(!label.contains("&&"), "{label:?}");
                }
            }
        }
    }
}

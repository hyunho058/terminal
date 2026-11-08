//! macOS Application menu setup, configuration seeding, and settings opener.
//!
//! ## View-menu items are their chords
//!
//! Every View item stands for an in-app chord, and choosing it **replays that chord**:
//! the action posts [`UserEvent::Menu`], and the loop feeds [`MenuCommand::chord`] through
//! `Running::on_key`, the same entry point the keystroke reaches and the one the IME's
//! chord replay already uses. Overlays that own the keyboard (the rename field, the Agent
//! Monitor) therefore treat a menu click exactly as they treat the keystroke, and no
//! second implementation of "toggle the sidebar" exists to drift.
//!
//! Neither path can fire twice for one keystroke. winit does not override
//! `performKeyEquivalent:`, so AppKit offers a Cmd keyDown to the main menu first: when an
//! enabled item matches, its action runs and the key never reaches winit; when none
//! matches (some layouts under 2-Set Korean), the keyDown reaches winit and the in-app
//! matcher fires instead. An item with no action is disabled and matches nothing, which is
//! why these items used to be inert, and why their stated shortcuts could drift (the
//! sidebar's read Cmd+B while the chord was Cmd+Shift+S).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::{define_class, sel, ClassType, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{ns_string, NSObject, NSString};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

use crate::seam::KeyInput;
use crate::terminal_app::UserEvent;

/// A View-menu item, named by the in-app chord it replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    ToggleSidebar,
    ToggleMonitor,
    ActualSize,
    ZoomIn,
    ZoomOut,
}

impl MenuCommand {
    /// Every command, in View-menu order (a separator follows `ToggleMonitor`).
    pub const ALL: [MenuCommand; 5] = [
        MenuCommand::ToggleSidebar,
        MenuCommand::ToggleMonitor,
        MenuCommand::ActualSize,
        MenuCommand::ZoomIn,
        MenuCommand::ZoomOut,
    ];

    /// The item's title.
    pub fn title(self) -> &'static str {
        match self {
            MenuCommand::ToggleSidebar => "Toggle Sidebar",
            MenuCommand::ToggleMonitor => "Toggle AI Monitor",
            MenuCommand::ActualSize => "Actual Size",
            MenuCommand::ZoomIn => "Zoom In",
            MenuCommand::ZoomOut => "Zoom Out",
        }
    }

    /// The keystroke the item replays, as winit delivers it on an ABC layout: a logical
    /// key plus the modifiers held. The item's key equivalent is derived from this, so the
    /// shortcut the menu shows is, by construction, the one it performs.
    fn keystroke(self) -> (&'static str, ModifiersState, KeyCode) {
        let cmd = ModifiersState::SUPER;
        let cmd_shift = ModifiersState::SUPER | ModifiersState::SHIFT;
        match self {
            MenuCommand::ToggleSidebar => ("S", cmd_shift, KeyCode::KeyS),
            MenuCommand::ToggleMonitor => ("M", cmd_shift, KeyCode::KeyM),
            MenuCommand::ActualSize => ("0", cmd, KeyCode::Digit0),
            MenuCommand::ZoomIn => ("=", cmd, KeyCode::Equal),
            MenuCommand::ZoomOut => ("-", cmd, KeyCode::Minus),
        }
    }

    /// The chord this item replays, as a [`KeyInput`] for `Running::on_key`.
    pub fn chord(self) -> KeyInput {
        let (key, mods, code) = self.keystroke();
        KeyInput {
            key: Key::Character(key.into()),
            text: None,
            mods,
            physical: PhysicalKey::Code(code),
        }
    }

    /// The AppKit key equivalent and modifier mask derived from [`keystroke`](Self::keystroke).
    /// AppKit spells a Shift chord as the lowercase character plus an explicit Shift mask.
    pub fn key_equivalent(self) -> (String, NSEventModifierFlags) {
        let (key, mods, _) = self.keystroke();
        let mut mask = NSEventModifierFlags::Command;
        if mods.shift_key() {
            mask |= NSEventModifierFlags::Shift;
        }
        (key.to_ascii_lowercase(), mask)
    }

    /// The item's `tag`, which is how the shared action tells the commands apart.
    pub fn tag(self) -> isize {
        self as isize + 1
    }

    /// Inverse of [`tag`](Self::tag); `None` for an item that is not a command.
    pub fn from_tag(tag: isize) -> Option<MenuCommand> {
        MenuCommand::ALL.into_iter().find(|c| c.tag() == tag)
    }
}

thread_local! {
    /// The loop's proxy, set by `App::start`. Main-thread only, as every menu action is.
    static MENU_PROXY: RefCell<Option<EventLoopProxy<UserEvent>>> = const { RefCell::new(None) };
}

/// Hand the menu the proxy its actions wake the loop through. Before this runs (no
/// window yet) a chosen command is dropped, as the keystroke would have nothing to act on.
pub fn set_menu_proxy(proxy: EventLoopProxy<UserEvent>) {
    MENU_PROXY.with(|p| *p.borrow_mut() = Some(proxy));
}

fn post(cmd: MenuCommand) {
    MENU_PROXY.with(|p| match p.borrow().as_ref() {
        Some(proxy) => {
            let _ = proxy.send_event(UserEvent::Menu(cmd));
        }
        None => tracing::debug!(?cmd, "menu command before the loop started; dropped"),
    });
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SettingsMenuTarget"]
    struct SettingsMenuTarget;

    impl SettingsMenuTarget {
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&NSMenuItem>) {
            open_settings();
        }

        #[unsafe(method(performMenuCommand:))]
        fn perform_menu_command(&self, sender: Option<&NSMenuItem>) {
            if let Some(cmd) = sender.and_then(|item| MenuCommand::from_tag(item.tag())) {
                post(cmd);
            }
        }
    }
);

unsafe impl Send for SettingsMenuTarget {}
unsafe impl Sync for SettingsMenuTarget {}

fn settings_target() -> &'static SettingsMenuTarget {
    static TARGET: OnceLock<Retained<SettingsMenuTarget>> = OnceLock::new();
    TARGET.get_or_init(|| unsafe { objc2::msg_send![SettingsMenuTarget::class(), new] })
}

/// Build the native macOS Application menu containing About, Settings, Services,
/// Hide, Hide Others, Show All, and Quit items.
pub fn build_app_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("Crewterm"));

    // 0: About Crewterm
    let about = NSMenuItem::new(mtm);
    about.setTitle(ns_string!("About Crewterm"));
    unsafe { about.setAction(Some(sel!(orderFrontStandardAboutPanel:))) };
    menu.addItem(&about);

    // 1: Separator
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // 2: Settings... (key equivalent ",")
    let settings = NSMenuItem::new(mtm);
    settings.setTitle(ns_string!("Settings..."));
    settings.setKeyEquivalent(ns_string!(","));
    unsafe {
        settings.setAction(Some(sel!(openSettings:)));
        settings.setTarget(Some(settings_target()));
    }
    menu.addItem(&settings);

    // 3: Separator
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // 4: Services
    let services = NSMenuItem::new(mtm);
    services.setTitle(ns_string!("Services"));
    let services_menu = NSMenu::new(mtm);
    services_menu.setTitle(ns_string!("Services"));
    services.setSubmenu(Some(&services_menu));
    menu.addItem(&services);

    // 5: Separator
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // 6: Hide Crewterm (key equivalent "h")
    let hide = NSMenuItem::new(mtm);
    hide.setTitle(ns_string!("Hide Crewterm"));
    hide.setKeyEquivalent(ns_string!("h"));
    unsafe { hide.setAction(Some(sel!(hide:))) };
    menu.addItem(&hide);

    // 7: Hide Others (key equivalent "h", Command | Option)
    let hide_others = NSMenuItem::new(mtm);
    hide_others.setTitle(ns_string!("Hide Others"));
    hide_others.setKeyEquivalent(ns_string!("h"));
    hide_others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
    unsafe { hide_others.setAction(Some(sel!(hideOtherApplications:))) };
    menu.addItem(&hide_others);

    // 8: Show All
    let show_all = NSMenuItem::new(mtm);
    show_all.setTitle(ns_string!("Show All"));
    unsafe { show_all.setAction(Some(sel!(unhideAllApplications:))) };
    menu.addItem(&show_all);

    // 9: Separator
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // 10: Quit Crewterm (key equivalent "q")
    let quit = NSMenuItem::new(mtm);
    quit.setTitle(ns_string!("Quit Crewterm"));
    quit.setKeyEquivalent(ns_string!("q"));
    unsafe { quit.setAction(Some(sel!(terminate:))) };
    menu.addItem(&quit);

    menu
}

/// Build the native macOS View menu.
pub fn build_view_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("View"));

    for cmd in MenuCommand::ALL {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(cmd.title()));
        let (key, mask) = cmd.key_equivalent();
        item.setKeyEquivalent(&NSString::from_str(&key));
        item.setKeyEquivalentModifierMask(mask);
        item.setTag(cmd.tag());
        unsafe {
            item.setAction(Some(sel!(performMenuCommand:)));
            item.setTarget(Some(settings_target()));
        }
        menu.addItem(&item);
        if cmd == MenuCommand::ToggleMonitor {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
    }

    menu
}

/// Build the native macOS Window menu.
pub fn build_window_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("Window"));

    // Minimize (Cmd+M)
    let minimize = NSMenuItem::new(mtm);
    minimize.setTitle(ns_string!("Minimize"));
    minimize.setKeyEquivalent(ns_string!("m"));
    unsafe { minimize.setAction(Some(sel!(performMiniaturize:))) };
    menu.addItem(&minimize);

    // Zoom
    let zoom = NSMenuItem::new(mtm);
    zoom.setTitle(ns_string!("Zoom"));
    unsafe { zoom.setAction(Some(sel!(performZoom:))) };
    menu.addItem(&zoom);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // Bring All to Front
    let front = NSMenuItem::new(mtm);
    front.setTitle(ns_string!("Bring All to Front"));
    unsafe { front.setAction(Some(sel!(arrangeInFront:))) };
    menu.addItem(&front);

    menu
}

/// Install the Application menu into `NSApplication::sharedApplication(mtm).mainMenu()`.
pub fn install_app_menu(mtm: MainThreadMarker) {
    extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        fn class_getClassMethod(
            cls: *const std::ffi::c_void,
            name: *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        fn method_setImplementation(
            m: *mut std::ffi::c_void,
            imp: unsafe extern "C" fn(
                *const std::ffi::c_void,
                *const std::ffi::c_void,
            ) -> objc2::runtime::Bool,
        ) -> unsafe extern "C" fn(
            *const std::ffi::c_void,
            *const std::ffi::c_void,
        ) -> objc2::runtime::Bool;
        fn pthread_main_np() -> std::ffi::c_int;
    }

    unsafe extern "C" fn always_main(
        _cls: *const std::ffi::c_void,
        _sel: *const std::ffi::c_void,
    ) -> objc2::runtime::Bool {
        objc2::runtime::Bool::YES
    }

    let is_secondary = unsafe { pthread_main_np() == 0 };
    let method = if is_secondary {
        let cls = unsafe { objc_getClass(c"NSThread".as_ptr()) };
        let sel = unsafe { sel_registerName(c"isMainThread".as_ptr()) };
        if !cls.is_null() && !sel.is_null() {
            unsafe { class_getClassMethod(cls, sel) }
        } else {
            std::ptr::null_mut()
        }
    } else {
        std::ptr::null_mut()
    };

    let old_imp = if !method.is_null() {
        unsafe { Some(method_setImplementation(method, always_main)) }
    } else {
        None
    };

    let app_menu = build_app_menu(mtm);
    let main_menu = NSMenu::new(mtm);
    let top_item = NSMenuItem::new(mtm);
    top_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&top_item);

    let view_menu = build_view_menu(mtm);
    let view_top_item = NSMenuItem::new(mtm);
    view_top_item.setTitle(ns_string!("View"));
    view_top_item.setSubmenu(Some(&view_menu));
    main_menu.addItem(&view_top_item);

    let window_menu = build_window_menu(mtm);
    let window_top_item = NSMenuItem::new(mtm);
    window_top_item.setTitle(ns_string!("Window"));
    window_top_item.setSubmenu(Some(&window_menu));
    main_menu.addItem(&window_top_item);

    let app = NSApplication::sharedApplication(mtm);
    app.setMainMenu(Some(&main_menu));
    app.setWindowsMenu(Some(&window_menu));

    if let Some(services_item) = app_menu.itemAtIndex(4) {
        if let Some(services_menu) = services_item.submenu() {
            app.setServicesMenu(Some(&services_menu));
        }
    }

    if let Some(old) = old_imp {
        unsafe {
            method_setImplementation(method, old);
        }
    }
}

/// Default configuration template embedded from repo root.
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../../config.example.toml");

/// Ensure configuration file exists at the given path, creating parent dirs and
/// writing default template if absent.
pub fn ensure_config_file_exists_at(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TEMPLATE)?;
    Ok(path.to_path_buf())
}

/// Ensure configuration file exists at `Config::path()`.
pub fn ensure_config_file_exists() -> std::io::Result<PathBuf> {
    let path = common::config::Config::path()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
    ensure_config_file_exists_at(&path)
}

/// Open the configuration file with the system default editor via `/usr/bin/open`.
pub fn open_settings_file(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Ensure configuration exists and open the native macOS Settings window.
/// Falls back to opening the config file with the default editor if outside the main thread.
pub fn open_settings() {
    if let Some(mtm) = MainThreadMarker::new() {
        crate::settings_window::show_settings_window(mtm);
    } else {
        match ensure_config_file_exists() {
            Ok(path) => {
                if let Err(e) = open_settings_file(&path) {
                    tracing::warn!("failed to open settings file {}: {e}", path.display());
                }
            }
            Err(e) => {
                tracing::warn!("failed to ensure settings file exists: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MenuCommand;
    use crate::input::{font_action, is_sidebar_toggle_chord, tab_action, FontAction};

    // Each item's chord must be claimed by the matcher that implements it, and by no other
    // chord family; `overlay.rs` tests the monitor's, whose matcher is private there.
    #[test]
    fn each_chord_is_claimed_by_its_own_matcher() {
        for cmd in MenuCommand::ALL {
            let k = cmd.chord();
            let font = font_action(&k.key, &k.mods);
            let sidebar = is_sidebar_toggle_chord(&k.key, &k.mods);
            let expected_font = match cmd {
                MenuCommand::ActualSize => Some(FontAction::Reset),
                MenuCommand::ZoomIn => Some(FontAction::Increase),
                MenuCommand::ZoomOut => Some(FontAction::Decrease),
                MenuCommand::ToggleSidebar | MenuCommand::ToggleMonitor => None,
            };
            assert_eq!(font, expected_font, "{cmd:?}");
            assert_eq!(sidebar, cmd == MenuCommand::ToggleSidebar, "{cmd:?}");
            assert_eq!(tab_action(&k.key, &k.mods), None, "{cmd:?}");
        }
    }

    #[test]
    fn tags_round_trip_and_zero_is_no_command() {
        for cmd in MenuCommand::ALL {
            assert_eq!(MenuCommand::from_tag(cmd.tag()), Some(cmd));
        }
        // An untagged NSMenuItem reads tag 0; it must not alias the first command.
        assert_eq!(MenuCommand::from_tag(0), None);
    }
}

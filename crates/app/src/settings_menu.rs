//! macOS Application menu setup, configuration seeding, and settings opener.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::{define_class, sel, ClassType, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{ns_string, NSObject};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SettingsMenuTarget"]
    struct SettingsMenuTarget;

    impl SettingsMenuTarget {
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&NSMenuItem>) {
            open_settings();
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
    menu.setTitle(ns_string!("mac-terminal"));

    // 0: About mac-terminal
    let about = NSMenuItem::new(mtm);
    about.setTitle(ns_string!("About mac-terminal"));
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

    // 6: Hide mac-terminal (key equivalent "h")
    let hide = NSMenuItem::new(mtm);
    hide.setTitle(ns_string!("Hide mac-terminal"));
    hide.setKeyEquivalent(ns_string!("h"));
    unsafe { hide.setAction(Some(sel!(hide:))) };
    menu.addItem(&hide);

    // 7: Hide Others (key equivalent "h", Command | Option)
    let hide_others = NSMenuItem::new(mtm);
    hide_others.setTitle(ns_string!("Hide Others"));
    hide_others.setKeyEquivalent(ns_string!("h"));
    hide_others.setKeyEquivalentModifierMask(
        NSEventModifierFlags::Command | NSEventModifierFlags::Option,
    );
    unsafe { hide_others.setAction(Some(sel!(hideOtherApplications:))) };
    menu.addItem(&hide_others);

    // 8: Show All
    let show_all = NSMenuItem::new(mtm);
    show_all.setTitle(ns_string!("Show All"));
    unsafe { show_all.setAction(Some(sel!(unhideAllApplications:))) };
    menu.addItem(&show_all);

    // 9: Separator
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // 10: Quit mac-terminal (key equivalent "q")
    let quit = NSMenuItem::new(mtm);
    quit.setTitle(ns_string!("Quit mac-terminal"));
    quit.setKeyEquivalent(ns_string!("q"));
    unsafe { quit.setAction(Some(sel!(terminate:))) };
    menu.addItem(&quit);

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

    let app = NSApplication::sharedApplication(mtm);
    app.setMainMenu(Some(&main_menu));

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
    let path = common::config::Config::path().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string())
    })?;
    ensure_config_file_exists_at(&path)
}

/// Open the configuration file with the system default editor via `/usr/bin/open`.
pub fn open_settings_file(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Ensure configuration exists and open it with the default editor.
pub fn open_settings() {
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

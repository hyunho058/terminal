//! TT1 — Acceptance tests for top-settings-menu R1: Native AppKit Menu Assembly and Installation (D1, D3, D6).
//!
//! Asserts that:
//! - R1.1: `app::build_app_menu(mtm)` constructs an `NSMenu` adhering to standard macOS
//!   Application menu conventions with About, Settings (⌘,), Services, Hide, Hide Others,
//!   Show All, and Quit items separated by standard dividers.
//! - R1.2: `app::install_app_menu(mtm)` sets the Application menu on the top-level
//!   `NSApplication::sharedApplication(mtm).mainMenu()`.

use objc2::sel;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

/// Acquire a `MainThreadMarker` for testing on macOS.
///
/// In standard test harnesses running on secondary worker threads, fall back to
/// `new_unchecked()` so AppKit menu objects can be constructed and verified.
fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().unwrap_or_else(|| unsafe { MainThreadMarker::new_unchecked() })
}

/// R1.1: Menu structure and standard actions assembly.
///
/// Given: A valid MainThreadMarker
/// When: `app::build_app_menu(mtm)` is called
/// Then: Returns an NSMenu whose items include:
///   0. "About mac-terminal" (`orderFrontStandardAboutPanel:`)
///   1. Separator
///   2. "Settings..." (key equivalent `","`)
///   3. Separator
///   4. "Services" (with submenu)
///   5. Separator
///   6. "Hide mac-terminal" (`hide:`)
///   7. "Hide Others" (`hideOtherApplications:`)
///   8. "Show All" (`unhideAllApplications:`)
///   9. Separator
///   10. "Quit mac-terminal" (`terminate:`)
#[test]
fn r1_1_build_app_menu_structure_and_standard_actions() {
    let mtm = main_thread();
    let menu = app::build_app_menu(mtm);

    assert_eq!(
        menu.numberOfItems(),
        11,
        "Application menu must contain exactly 11 items (including separators)"
    );

    // 0: About Crewterm
    let about = menu.itemAtIndex(0).expect("item 0: About");
    assert_eq!(about.title().to_string(), "About Crewterm");
    assert_eq!(
        about.action(),
        Some(sel!(orderFrontStandardAboutPanel:)),
        "About item must trigger orderFrontStandardAboutPanel:"
    );
    assert!(!about.isSeparatorItem());

    // 1: Separator
    let sep1 = menu.itemAtIndex(1).expect("item 1: Separator");
    assert!(sep1.isSeparatorItem(), "item 1 must be a separator");

    // 2: Settings...
    let settings = menu.itemAtIndex(2).expect("item 2: Settings");
    assert_eq!(settings.title().to_string(), "Settings...");
    assert_eq!(
        settings.keyEquivalent().to_string(),
        ",",
        "Settings key equivalent must be comma"
    );
    assert!(!settings.isSeparatorItem());

    // 3: Separator
    let sep2 = menu.itemAtIndex(3).expect("item 3: Separator");
    assert!(sep2.isSeparatorItem(), "item 3 must be a separator");

    // 4: Services
    let services = menu.itemAtIndex(4).expect("item 4: Services");
    assert_eq!(services.title().to_string(), "Services");
    assert!(
        services.submenu().is_some(),
        "Services item must have a submenu"
    );
    assert!(!services.isSeparatorItem());

    // 5: Separator
    let sep3 = menu.itemAtIndex(5).expect("item 5: Separator");
    assert!(sep3.isSeparatorItem(), "item 5 must be a separator");

    // 6: Hide Crewterm
    let hide = menu.itemAtIndex(6).expect("item 6: Hide");
    assert_eq!(hide.title().to_string(), "Hide Crewterm");
    assert_eq!(
        hide.action(),
        Some(sel!(hide:)),
        "Hide item must trigger hide:"
    );
    assert!(!hide.isSeparatorItem());

    // 7: Hide Others
    let hide_others = menu.itemAtIndex(7).expect("item 7: Hide Others");
    assert_eq!(hide_others.title().to_string(), "Hide Others");
    assert_eq!(
        hide_others.action(),
        Some(sel!(hideOtherApplications:)),
        "Hide Others item must trigger hideOtherApplications:"
    );
    assert!(!hide_others.isSeparatorItem());

    // 8: Show All
    let show_all = menu.itemAtIndex(8).expect("item 8: Show All");
    assert_eq!(show_all.title().to_string(), "Show All");
    assert_eq!(
        show_all.action(),
        Some(sel!(unhideAllApplications:)),
        "Show All item must trigger unhideAllApplications:"
    );
    assert!(!show_all.isSeparatorItem());

    // 9: Separator
    let sep4 = menu.itemAtIndex(9).expect("item 9: Separator");
    assert!(sep4.isSeparatorItem(), "item 9 must be a separator");

    // 10: Quit Crewterm
    let quit = menu.itemAtIndex(10).expect("item 10: Quit");
    assert_eq!(quit.title().to_string(), "Quit Crewterm");
    assert_eq!(
        quit.action(),
        Some(sel!(terminate:)),
        "Quit item must trigger terminate:"
    );
    assert!(!quit.isSeparatorItem());
}

/// R1.2: Menu bar installation on NSApplication.
///
/// Given: A valid MainThreadMarker
/// When: `app::install_app_menu(mtm)` is called
/// Then: `NSApplication::sharedApplication(mtm).mainMenu()` is populated with a top menu bar
/// containing the assembled Application menu.
#[test]
fn r1_2_install_app_menu_populates_main_menu() {
    let mtm = main_thread();
    app::install_app_menu(mtm);

    let app = NSApplication::sharedApplication(mtm);
    let main_menu = app.mainMenu().expect("main menu must be populated on NSApplication");
    assert!(
        main_menu.numberOfItems() >= 1,
        "main menu must contain at least one item representing the application menu bar item"
    );

    let top_item = main_menu.itemAtIndex(0).expect("first item in top menu bar");
    let app_menu = top_item.submenu().unwrap_or_else(|| main_menu.clone());

    assert_eq!(
        app_menu.numberOfItems(),
        11,
        "installed application menu must contain all 11 items"
    );

    let about_item = app_menu.itemAtIndex(0).expect("about item");
    assert_eq!(about_item.title().to_string(), "About Crewterm");
    assert_eq!(about_item.action(), Some(sel!(orderFrontStandardAboutPanel:)));

    let settings_item = app_menu.itemAtIndex(2).expect("settings item");
    assert_eq!(settings_item.title().to_string(), "Settings...");
    assert_eq!(settings_item.keyEquivalent().to_string(), ",");

    let quit_item = app_menu.itemAtIndex(10).expect("quit item");
    assert_eq!(quit_item.title().to_string(), "Quit Crewterm");
    assert_eq!(quit_item.action(), Some(sel!(terminate:)));
}

#[test]
fn r1_3_view_and_window_menus_assembled() {
    let mtm = main_thread();
    let view_menu = app::build_view_menu(mtm);
    assert_eq!(view_menu.title().to_string(), "View");
    assert!(view_menu.numberOfItems() >= 5);

    let window_menu = app::build_window_menu(mtm);
    assert_eq!(window_menu.title().to_string(), "Window");
    assert!(window_menu.numberOfItems() >= 3);
}

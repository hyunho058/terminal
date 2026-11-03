//! Native macOS Settings Window implemented using AppKit (`objc2-app-kit`).
//!
//! Provides a standard macOS preference window with tabs for Font & Grid,
//! Sidebar, and AI Monitor settings. All changes are saved directly to
//! `config.toml` via [`common::config::save_config`] and live-reloaded into the
//! terminal emulator in real time.

use std::sync::{Mutex, OnceLock};

use common::config::{
    Config, FONT_SIZE_DEFAULT, FONT_SIZE_MAX, FONT_SIZE_MIN, MONITOR_WIDTH_MAX, MONITOR_WIDTH_MIN,
    SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN,
};
use objc2::rc::Retained;
use objc2::{define_class, sel, ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn,
    NSTabView, NSTabViewItem, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};

struct SettingsControls {
    window: Retained<NSWindow>,
    font_family: Retained<NSTextField>,
    font_size: Retained<NSTextField>,
    ligatures: Retained<NSButton>,
    columns: Retained<NSTextField>,
    rows: Retained<NSTextField>,
    sidebar_width: Retained<NSTextField>,
    git_branch: Retained<NSButton>,
    agent_status: Retained<NSButton>,
    sidebar_collapsed: Retained<NSButton>,
    ai_monitor_width: Retained<NSTextField>,
    ai_monitor_open: Retained<NSButton>,
    status_label: Retained<NSTextField>,
}

unsafe impl Send for SettingsControls {}
unsafe impl Sync for SettingsControls {}

static CONTROLS: OnceLock<Mutex<Option<SettingsControls>>> = OnceLock::new();

fn controls_lock() -> &'static Mutex<Option<SettingsControls>> {
    CONTROLS.get_or_init(|| Mutex::new(None))
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[name = "SettingsWindowTarget"]
    struct SettingsWindowTarget;

    impl SettingsWindowTarget {
        #[unsafe(method(applySettings:))]
        fn apply_settings(&self, _sender: Option<&NSButton>) {
            apply_current_ui_settings();
        }

        #[unsafe(method(resetDefaults:))]
        fn reset_defaults(&self, _sender: Option<&NSButton>) {
            reset_ui_to_defaults();
        }

        #[unsafe(method(openConfigFile:))]
        fn open_config_file(&self, _sender: Option<&NSButton>) {
            if let Ok(path) = Config::path() {
                let _ = crate::settings_menu::open_settings_file(&path);
            }
        }
    }
);

unsafe impl Send for SettingsWindowTarget {}
unsafe impl Sync for SettingsWindowTarget {}

fn window_target() -> &'static SettingsWindowTarget {
    static TARGET: OnceLock<Retained<SettingsWindowTarget>> = OnceLock::new();
    TARGET.get_or_init(|| unsafe { objc2::msg_send![SettingsWindowTarget::class(), new] })
}

/// Apply values currently in the UI fields to `config.toml`.
fn apply_current_ui_settings() {
    let guard = controls_lock().lock().unwrap();
    let Some(controls) = guard.as_ref() else {
        return;
    };

    let mut cfg = Config::load().unwrap_or_default();

    // 1. Font family
    let family_str = controls.font_family.stringValue().to_string();
    if !family_str.trim().is_empty() {
        cfg.font.family = family_str.trim().to_string();
    }

    // 2. Font size
    let size_str = controls.font_size.stringValue().to_string();
    if let Ok(size) = size_str.trim().parse::<f32>() {
        cfg.font.size_pt = size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
    }

    // 3. Ligatures
    cfg.font.ligatures = controls.ligatures.state() == NSControlStateValueOn;

    // 4. Grid columns & rows
    let cols_str = controls.columns.stringValue().to_string();
    if let Ok(cols) = cols_str.trim().parse::<u16>() {
        cfg.grid.columns = cols.max(20);
    }
    let rows_str = controls.rows.stringValue().to_string();
    if let Ok(r) = rows_str.trim().parse::<u16>() {
        cfg.grid.rows = r.max(5);
    }

    // 5. Sidebar
    let sb_str = controls.sidebar_width.stringValue().to_string();
    if let Ok(w) = sb_str.trim().parse::<f32>() {
        cfg.sidebar.width_px = w.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
    }
    cfg.sidebar.show_git_branch = controls.git_branch.state() == NSControlStateValueOn;
    cfg.sidebar.show_agent_status = controls.agent_status.state() == NSControlStateValueOn;
    cfg.sidebar.collapsed = controls.sidebar_collapsed.state() == NSControlStateValueOn;

    // 6. AI Monitor
    let ai_str = controls.ai_monitor_width.stringValue().to_string();
    if let Ok(w) = ai_str.trim().parse::<f32>() {
        cfg.ai_monitor.width_px = w.clamp(MONITOR_WIDTH_MIN, MONITOR_WIDTH_MAX);
    }
    cfg.ai_monitor.open = controls.ai_monitor_open.state() == NSControlStateValueOn;

    // Save
    match Config::path() {
        Ok(path) => {
            if let Err(e) = common::config::save_config(&path, &cfg) {
                tracing::error!(error = %e, "failed to save settings to config.toml");
                let err_msg = format!("Save failed: {e}");
                controls
                    .status_label
                    .setStringValue(&NSString::from_str(&err_msg));
            } else {
                controls
                    .status_label
                    .setStringValue(ns_string!("Settings saved and applied!"));
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to determine config.toml path");
            let err_msg = format!("Config error: {e}");
            controls
                .status_label
                .setStringValue(&NSString::from_str(&err_msg));
        }
    }
}

/// Reset the UI inputs and persisted `config.toml` to default settings.
fn reset_ui_to_defaults() {
    let guard = controls_lock().lock().unwrap();
    let Some(controls) = guard.as_ref() else {
        return;
    };

    let def = Config::default();

    controls
        .font_family
        .setStringValue(&NSString::from_str(&def.font.family));
    controls
        .font_size
        .setStringValue(&NSString::from_str(&format!("{:.1}", def.font.size_pt)));
    controls.ligatures.setState(if def.font.ligatures {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    controls
        .columns
        .setStringValue(&NSString::from_str(&def.grid.columns.to_string()));
    controls
        .rows
        .setStringValue(&NSString::from_str(&def.grid.rows.to_string()));

    controls
        .sidebar_width
        .setStringValue(&NSString::from_str(&format!("{:.1}", def.sidebar.width_px)));
    controls.git_branch.setState(if def.sidebar.show_git_branch {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    controls.agent_status.setState(if def.sidebar.show_agent_status {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    controls.sidebar_collapsed.setState(if def.sidebar.collapsed {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });

    controls.ai_monitor_width.setStringValue(&NSString::from_str(&format!(
        "{:.1}",
        def.ai_monitor.width_px
    )));
    controls.ai_monitor_open.setState(if def.ai_monitor.open {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });

    if let Ok(path) = Config::path() {
        if let Err(e) = common::config::save_config(&path, &def) {
            let err_msg = format!("Reset save failed: {e}");
            controls
                .status_label
                .setStringValue(&NSString::from_str(&err_msg));
        } else {
            controls
                .status_label
                .setStringValue(ns_string!("Reset to defaults and applied!"));
        }
    }
}

/// Show or bring to front the Native macOS Settings Window.
pub fn show_settings_window(mtm: MainThreadMarker) {
    let mut guard = controls_lock().lock().unwrap();

    if let Some(controls) = guard.as_ref() {
        controls.window.makeKeyAndOrderFront(None);
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        return;
    }

    // Ensure the config file exists so current or default values can be read.
    let _ = crate::settings_menu::ensure_config_file_exists();
    let current_cfg = Config::load().unwrap_or_default();

    // 1. Create Window (520 × 440)
    let win_rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(520.0, 440.0));
    let mask = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            win_rect,
            mask,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(ns_string!("Settings"));
    window.center();
    unsafe { window.setReleasedWhenClosed(false) };

    let content_view = NSView::initWithFrame(mtm.alloc(), win_rect);

    // 2. Tab View (x=20, y=75, w=480, h=345)
    let tab_rect = NSRect::new(NSPoint::new(20.0, 75.0), NSSize::new(480.0, 345.0));
    let tab_view = NSTabView::initWithFrame(mtm.alloc(), tab_rect);

    // --- Tab 1: Font & Grid ---
    let tab1_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 310.0)),
    );

    // Font Family
    let lbl_family = NSTextField::labelWithString(ns_string!("Font Family:"), mtm);
    lbl_family.setFrame(NSRect::new(
        NSPoint::new(20.0, 255.0),
        NSSize::new(140.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_family);

    let txt_family =
        NSTextField::textFieldWithString(&NSString::from_str(&current_cfg.font.family), mtm);
    txt_family.setFrame(NSRect::new(
        NSPoint::new(170.0, 255.0),
        NSSize::new(270.0, 24.0),
    ));
    tab1_view.addSubview(&txt_family);

    // Font Size
    let lbl_size = NSTextField::labelWithString(ns_string!("Font Size (pt):"), mtm);
    lbl_size.setFrame(NSRect::new(
        NSPoint::new(20.0, 210.0),
        NSSize::new(140.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_size);

    let txt_size = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.font.size_pt)),
        mtm,
    );
    txt_size.setFrame(NSRect::new(
        NSPoint::new(170.0, 210.0),
        NSSize::new(100.0, 24.0),
    ));
    tab1_view.addSubview(&txt_size);

    let lbl_size_hint = NSTextField::labelWithString(
        &NSString::from_str(&format!(
            "({:.0} - {:.0} pt, default {:.0})",
            FONT_SIZE_MIN, FONT_SIZE_MAX, FONT_SIZE_DEFAULT
        )),
        mtm,
    );
    lbl_size_hint.setFrame(NSRect::new(
        NSPoint::new(280.0, 210.0),
        NSSize::new(180.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_size_hint);

    // Ligatures
    let chk_ligatures = unsafe {
        NSButton::checkboxWithTitle_target_action(
            ns_string!("Enable Programming Ligatures (!=, =>, ...)"),
            None,
            None,
            mtm,
        )
    };
    chk_ligatures.setFrame(NSRect::new(
        NSPoint::new(170.0, 165.0),
        NSSize::new(280.0, 24.0),
    ));
    chk_ligatures.setState(if current_cfg.font.ligatures {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab1_view.addSubview(&chk_ligatures);

    // Grid Columns
    let lbl_cols = NSTextField::labelWithString(ns_string!("Default Columns:"), mtm);
    lbl_cols.setFrame(NSRect::new(
        NSPoint::new(20.0, 115.0),
        NSSize::new(140.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_cols);

    let txt_cols = NSTextField::textFieldWithString(
        &NSString::from_str(&current_cfg.grid.columns.to_string()),
        mtm,
    );
    txt_cols.setFrame(NSRect::new(
        NSPoint::new(170.0, 115.0),
        NSSize::new(100.0, 24.0),
    ));
    tab1_view.addSubview(&txt_cols);

    // Grid Rows
    let lbl_rows = NSTextField::labelWithString(ns_string!("Default Rows:"), mtm);
    lbl_rows.setFrame(NSRect::new(
        NSPoint::new(20.0, 70.0),
        NSSize::new(140.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_rows);

    let txt_rows = NSTextField::textFieldWithString(
        &NSString::from_str(&current_cfg.grid.rows.to_string()),
        mtm,
    );
    txt_rows.setFrame(NSRect::new(
        NSPoint::new(170.0, 70.0),
        NSSize::new(100.0, 24.0),
    ));
    tab1_view.addSubview(&txt_rows);

    let item1 = unsafe { NSTabViewItem::initWithIdentifier(mtm.alloc(), None) };
    item1.setLabel(ns_string!("Font & Grid"));
    item1.setView(Some(&tab1_view));
    tab_view.addTabViewItem(&item1);

    // --- Tab 2: Sidebar & AI Monitor ---
    let tab2_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 310.0)),
    );

    // Sidebar Width
    let lbl_sb_width = NSTextField::labelWithString(ns_string!("Sidebar Width (px):"), mtm);
    lbl_sb_width.setFrame(NSRect::new(
        NSPoint::new(20.0, 255.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_sb_width);

    let txt_sb_width = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.sidebar.width_px)),
        mtm,
    );
    txt_sb_width.setFrame(NSRect::new(
        NSPoint::new(180.0, 255.0),
        NSSize::new(100.0, 24.0),
    ));
    tab2_view.addSubview(&txt_sb_width);

    let lbl_sb_hint = NSTextField::labelWithString(
        &NSString::from_str(&format!(
            "({:.0} - {:.0} px)",
            SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX
        )),
        mtm,
    );
    lbl_sb_hint.setFrame(NSRect::new(
        NSPoint::new(290.0, 255.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_sb_hint);

    // Sidebar Git Branch
    let chk_git_branch = unsafe {
        NSButton::checkboxWithTitle_target_action(
            ns_string!("Show Git Branch on Tab Cards"),
            None,
            None,
            mtm,
        )
    };
    chk_git_branch.setFrame(NSRect::new(
        NSPoint::new(180.0, 215.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_git_branch.setState(if current_cfg.sidebar.show_git_branch {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab2_view.addSubview(&chk_git_branch);

    // Sidebar Agent Status
    let chk_agent_status = unsafe {
        NSButton::checkboxWithTitle_target_action(
            ns_string!("Show AI Agent Status Marker"),
            None,
            None,
            mtm,
        )
    };
    chk_agent_status.setFrame(NSRect::new(
        NSPoint::new(180.0, 180.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_agent_status.setState(if current_cfg.sidebar.show_agent_status {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab2_view.addSubview(&chk_agent_status);

    // Sidebar Collapsed
    let chk_sb_collapsed = unsafe {
        NSButton::checkboxWithTitle_target_action(
            ns_string!("Start Sidebar Collapsed"),
            None,
            None,
            mtm,
        )
    };
    chk_sb_collapsed.setFrame(NSRect::new(
        NSPoint::new(180.0, 145.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_sb_collapsed.setState(if current_cfg.sidebar.collapsed {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab2_view.addSubview(&chk_sb_collapsed);

    // AI Monitor Width
    let lbl_mon_width = NSTextField::labelWithString(ns_string!("AI Monitor Width (px):"), mtm);
    lbl_mon_width.setFrame(NSRect::new(
        NSPoint::new(20.0, 95.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_mon_width);

    let txt_mon_width = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.ai_monitor.width_px)),
        mtm,
    );
    txt_mon_width.setFrame(NSRect::new(
        NSPoint::new(180.0, 95.0),
        NSSize::new(100.0, 24.0),
    ));
    tab2_view.addSubview(&txt_mon_width);

    let lbl_mon_hint = NSTextField::labelWithString(
        &NSString::from_str(&format!(
            "({:.0} - {:.0} px)",
            MONITOR_WIDTH_MIN, MONITOR_WIDTH_MAX
        )),
        mtm,
    );
    lbl_mon_hint.setFrame(NSRect::new(
        NSPoint::new(290.0, 95.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_mon_hint);

    // AI Monitor Open
    let chk_mon_open = unsafe {
        NSButton::checkboxWithTitle_target_action(
            ns_string!("AI Monitor Open on Start (Cmd+Shift+M)"),
            None,
            None,
            mtm,
        )
    };
    chk_mon_open.setFrame(NSRect::new(
        NSPoint::new(180.0, 55.0),
        NSSize::new(280.0, 24.0),
    ));
    chk_mon_open.setState(if current_cfg.ai_monitor.open {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab2_view.addSubview(&chk_mon_open);

    let item2 = unsafe { NSTabViewItem::initWithIdentifier(mtm.alloc(), None) };
    item2.setLabel(ns_string!("Sidebar & AI Monitor"));
    item2.setView(Some(&tab2_view));
    tab_view.addTabViewItem(&item2);

    content_view.addSubview(&tab_view);

    // 3. Status Label (x=20, y=50, w=480, h=20)
    let status_label = NSTextField::labelWithString(ns_string!(""), mtm);
    status_label.setFrame(NSRect::new(
        NSPoint::new(20.0, 50.0),
        NSSize::new(480.0, 20.0),
    ));
    content_view.addSubview(&status_label);

    // 4. Action Buttons (y=16)
    let target = window_target();

    // "Reset Defaults"
    let btn_reset = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Reset Defaults"),
            Some(target),
            Some(sel!(resetDefaults:)),
            mtm,
        )
    };
    btn_reset.setFrame(NSRect::new(
        NSPoint::new(20.0, 16.0),
        NSSize::new(125.0, 30.0),
    ));
    content_view.addSubview(&btn_reset);

    // "Open config.toml"
    let btn_open = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Open config.toml"),
            Some(target),
            Some(sel!(openConfigFile:)),
            mtm,
        )
    };
    btn_open.setFrame(NSRect::new(
        NSPoint::new(155.0, 16.0),
        NSSize::new(140.0, 30.0),
    ));
    content_view.addSubview(&btn_open);

    // "Save & Apply"
    let btn_apply = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Save & Apply"),
            Some(target),
            Some(sel!(applySettings:)),
            mtm,
        )
    };
    btn_apply.setFrame(NSRect::new(
        NSPoint::new(375.0, 16.0),
        NSSize::new(125.0, 30.0),
    ));
    content_view.addSubview(&btn_apply);

    window.setContentView(Some(&content_view));
    window.makeKeyAndOrderFront(None);

    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    // Save controls for action dispatch
    *guard = Some(SettingsControls {
        window,
        font_family: txt_family,
        font_size: txt_size,
        ligatures: chk_ligatures,
        columns: txt_cols,
        rows: txt_rows,
        sidebar_width: txt_sb_width,
        git_branch: chk_git_branch,
        agent_status: chk_agent_status,
        sidebar_collapsed: chk_sb_collapsed,
        ai_monitor_width: txt_mon_width,
        ai_monitor_open: chk_mon_open,
        status_label,
    });
}

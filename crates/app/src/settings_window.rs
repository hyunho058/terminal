//! Native macOS Settings Window implemented using AppKit (`objc2-app-kit`).
//!
//! Provides a standard macOS preference window with tabs for Font & Grid,
//! Theme & Colors, and Sidebar & AI Monitor settings. All changes are saved
//! directly to `config.toml` via [`common::config::save_config`] and live-reloaded
//! into the terminal emulator in real time.

use std::sync::{Mutex, OnceLock};

use common::config::{
    Config, FONT_SIZE_MAX, FONT_SIZE_MIN, MONITOR_WIDTH_MAX, MONITOR_WIDTH_MIN,
    SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN,
};
use objc2::rc::Retained;
use objc2::{define_class, sel, ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn,
    NSSlider, NSTabView, NSTabViewItem, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};

struct SettingsControls {
    window: Retained<NSWindow>,
    // Tab 1: Font & Grid
    font_family: Retained<NSTextField>,
    font_size: Retained<NSTextField>,
    font_size_slider: Retained<NSSlider>,
    ligatures: Retained<NSButton>,
    columns: Retained<NSTextField>,
    rows: Retained<NSTextField>,
    // Tab 2: Theme & Colors
    bg_color: Retained<NSTextField>,
    fg_color: Retained<NSTextField>,
    // Tab 3: Sidebar & AI Monitor
    sidebar_width: Retained<NSTextField>,
    sidebar_width_slider: Retained<NSSlider>,
    git_branch: Retained<NSButton>,
    agent_status: Retained<NSButton>,
    sidebar_collapsed: Retained<NSButton>,
    ai_monitor_width: Retained<NSTextField>,
    ai_monitor_width_slider: Retained<NSSlider>,
    ai_monitor_open: Retained<NSButton>,
    // Bottom status
    status_label: Retained<NSTextField>,
}

unsafe impl Send for SettingsControls {}
unsafe impl Sync for SettingsControls {}

static CONTROLS: OnceLock<Mutex<Option<SettingsControls>>> = OnceLock::new();

fn controls_lock() -> &'static Mutex<Option<SettingsControls>> {
    CONTROLS.get_or_init(|| Mutex::new(None))
}

fn parse_hex_color(s: &str, fallback: u32) -> u32 {
    let clean = s
        .trim()
        .trim_start_matches('#')
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u32::from_str_radix(clean, 16).unwrap_or(fallback)
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

        #[unsafe(method(fontSizeSliderChanged:))]
        fn font_size_slider_changed(&self, _sender: Option<&NSSlider>) {
            let guard = controls_lock().lock().unwrap();
            if let Some(c) = guard.as_ref() {
                let val = c.font_size_slider.doubleValue() as f32;
                c.font_size
                    .setStringValue(&NSString::from_str(&format!("{val:.1}")));
            }
        }

        #[unsafe(method(sidebarWidthSliderChanged:))]
        fn sidebar_width_slider_changed(&self, _sender: Option<&NSSlider>) {
            let guard = controls_lock().lock().unwrap();
            if let Some(c) = guard.as_ref() {
                let val = c.sidebar_width_slider.doubleValue() as f32;
                c.sidebar_width
                    .setStringValue(&NSString::from_str(&format!("{val:.1}")));
            }
        }

        #[unsafe(method(monitorWidthSliderChanged:))]
        fn monitor_width_slider_changed(&self, _sender: Option<&NSSlider>) {
            let guard = controls_lock().lock().unwrap();
            if let Some(c) = guard.as_ref() {
                let val = c.ai_monitor_width_slider.doubleValue() as f32;
                c.ai_monitor_width
                    .setStringValue(&NSString::from_str(&format!("{val:.1}")));
            }
        }

        #[unsafe(method(applyPresetDefaultDark:))]
        fn apply_preset_default_dark(&self, _sender: Option<&NSButton>) {
            set_theme_fields(0x18181B, 0xE4E4E7);
        }

        #[unsafe(method(applyPresetTokyoNight:))]
        fn apply_preset_tokyo_night(&self, _sender: Option<&NSButton>) {
            set_theme_fields(0x1A1B26, 0xC0CAF5);
        }

        #[unsafe(method(applyPresetCatppuccin:))]
        fn apply_preset_catppuccin(&self, _sender: Option<&NSButton>) {
            set_theme_fields(0x1E1E2E, 0xCDD6F4);
        }

        #[unsafe(method(applyPresetSolarized:))]
        fn apply_preset_solarized(&self, _sender: Option<&NSButton>) {
            set_theme_fields(0x002B36, 0x839496);
        }

        #[unsafe(method(applyPresetLight:))]
        fn apply_preset_light(&self, _sender: Option<&NSButton>) {
            set_theme_fields(0xFAFAFA, 0x18181B);
        }
    }
);

unsafe impl Send for SettingsWindowTarget {}
unsafe impl Sync for SettingsWindowTarget {}

fn window_target() -> &'static SettingsWindowTarget {
    static TARGET: OnceLock<Retained<SettingsWindowTarget>> = OnceLock::new();
    TARGET.get_or_init(|| unsafe { objc2::msg_send![SettingsWindowTarget::class(), new] })
}

fn set_theme_fields(bg: u32, fg: u32) {
    let guard = controls_lock().lock().unwrap();
    if let Some(c) = guard.as_ref() {
        c.bg_color
            .setStringValue(&NSString::from_str(&format!("#{bg:06X}")));
        c.fg_color
            .setStringValue(&NSString::from_str(&format!("#{fg:06X}")));
    }
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
        let clamped = size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        cfg.font.size_pt = clamped;
        controls.font_size_slider.setDoubleValue(clamped as f64);
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

    // 5. Theme
    let bg_str = controls.bg_color.stringValue().to_string();
    cfg.theme.background = parse_hex_color(&bg_str, cfg.theme.background);
    let fg_str = controls.fg_color.stringValue().to_string();
    cfg.theme.foreground = parse_hex_color(&fg_str, cfg.theme.foreground);

    // 6. Sidebar
    let sb_str = controls.sidebar_width.stringValue().to_string();
    if let Ok(w) = sb_str.trim().parse::<f32>() {
        let clamped = w.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
        cfg.sidebar.width_px = clamped;
        controls.sidebar_width_slider.setDoubleValue(clamped as f64);
    }
    cfg.sidebar.show_git_branch = controls.git_branch.state() == NSControlStateValueOn;
    cfg.sidebar.show_agent_status = controls.agent_status.state() == NSControlStateValueOn;
    cfg.sidebar.collapsed = controls.sidebar_collapsed.state() == NSControlStateValueOn;

    // 7. AI Monitor
    let ai_str = controls.ai_monitor_width.stringValue().to_string();
    if let Ok(w) = ai_str.trim().parse::<f32>() {
        let clamped = w.clamp(MONITOR_WIDTH_MIN, MONITOR_WIDTH_MAX);
        cfg.ai_monitor.width_px = clamped;
        controls
            .ai_monitor_width_slider
            .setDoubleValue(clamped as f64);
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
    controls
        .font_size_slider
        .setDoubleValue(def.font.size_pt as f64);
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
        .bg_color
        .setStringValue(&NSString::from_str(&format!(
            "#{:06X}",
            def.theme.background
        )));
    controls
        .fg_color
        .setStringValue(&NSString::from_str(&format!(
            "#{:06X}",
            def.theme.foreground
        )));

    controls
        .sidebar_width
        .setStringValue(&NSString::from_str(&format!("{:.1}", def.sidebar.width_px)));
    controls
        .sidebar_width_slider
        .setDoubleValue(def.sidebar.width_px as f64);
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
    controls
        .ai_monitor_width_slider
        .setDoubleValue(def.ai_monitor.width_px as f64);
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
    let target = window_target();

    // 1. Create Window (520 × 450)
    let win_rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(520.0, 450.0));
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

    // 2. Tab View (x=20, y=75, w=480, h=355)
    let tab_rect = NSRect::new(NSPoint::new(20.0, 75.0), NSSize::new(480.0, 355.0));
    let tab_view = NSTabView::initWithFrame(mtm.alloc(), tab_rect);

    // =========================================================================
    // Tab 1: Font & Grid
    // =========================================================================
    let tab1_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 320.0)),
    );

    // Font Family
    let lbl_family = NSTextField::labelWithString(ns_string!("Font Family:"), mtm);
    lbl_family.setFrame(NSRect::new(
        NSPoint::new(20.0, 260.0),
        NSSize::new(130.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_family);

    let txt_family =
        NSTextField::textFieldWithString(&NSString::from_str(&current_cfg.font.family), mtm);
    txt_family.setFrame(NSRect::new(
        NSPoint::new(160.0, 260.0),
        NSSize::new(280.0, 24.0),
    ));
    tab1_view.addSubview(&txt_family);

    // Font Size: Label + Slider + Numeric Box
    let lbl_size = NSTextField::labelWithString(ns_string!("Font Size (pt):"), mtm);
    lbl_size.setFrame(NSRect::new(
        NSPoint::new(20.0, 215.0),
        NSSize::new(130.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_size);

    let slider_size = NSSlider::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(160.0, 215.0), NSSize::new(170.0, 24.0)),
    );
    slider_size.setMinValue(FONT_SIZE_MIN as f64);
    slider_size.setMaxValue(FONT_SIZE_MAX as f64);
    slider_size.setDoubleValue(current_cfg.font.size_pt as f64);
    unsafe {
        slider_size.setTarget(Some(target));
        slider_size.setAction(Some(sel!(fontSizeSliderChanged:)));
    }
    tab1_view.addSubview(&slider_size);

    let txt_size = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.font.size_pt)),
        mtm,
    );
    txt_size.setFrame(NSRect::new(
        NSPoint::new(340.0, 215.0),
        NSSize::new(60.0, 24.0),
    ));
    tab1_view.addSubview(&txt_size);

    let lbl_size_hint = NSTextField::labelWithString(ns_string!("pt"), mtm);
    lbl_size_hint.setFrame(NSRect::new(
        NSPoint::new(408.0, 215.0),
        NSSize::new(40.0, 22.0),
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
        NSPoint::new(160.0, 170.0),
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
        NSSize::new(130.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_cols);

    let txt_cols = NSTextField::textFieldWithString(
        &NSString::from_str(&current_cfg.grid.columns.to_string()),
        mtm,
    );
    txt_cols.setFrame(NSRect::new(
        NSPoint::new(160.0, 115.0),
        NSSize::new(80.0, 24.0),
    ));
    tab1_view.addSubview(&txt_cols);

    // Grid Rows
    let lbl_rows = NSTextField::labelWithString(ns_string!("Default Rows:"), mtm);
    lbl_rows.setFrame(NSRect::new(
        NSPoint::new(20.0, 70.0),
        NSSize::new(130.0, 22.0),
    ));
    tab1_view.addSubview(&lbl_rows);

    let txt_rows = NSTextField::textFieldWithString(
        &NSString::from_str(&current_cfg.grid.rows.to_string()),
        mtm,
    );
    txt_rows.setFrame(NSRect::new(
        NSPoint::new(160.0, 70.0),
        NSSize::new(80.0, 24.0),
    ));
    tab1_view.addSubview(&txt_rows);

    let item1 = unsafe { NSTabViewItem::initWithIdentifier(mtm.alloc(), None) };
    item1.setLabel(ns_string!("Font & Grid"));
    item1.setView(Some(&tab1_view));
    tab_view.addTabViewItem(&item1);

    // =========================================================================
    // Tab 2: Theme & Colors
    // =========================================================================
    let tab2_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 320.0)),
    );

    let lbl_theme_title =
        NSTextField::labelWithString(ns_string!("Terminal Color Scheme:"), mtm);
    lbl_theme_title.setFrame(NSRect::new(
        NSPoint::new(20.0, 260.0),
        NSSize::new(200.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_theme_title);

    // Background color
    let lbl_bg = NSTextField::labelWithString(ns_string!("Background (Hex):"), mtm);
    lbl_bg.setFrame(NSRect::new(
        NSPoint::new(20.0, 215.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_bg);

    let txt_bg = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("#{:06X}", current_cfg.theme.background)),
        mtm,
    );
    txt_bg.setFrame(NSRect::new(
        NSPoint::new(180.0, 215.0),
        NSSize::new(120.0, 24.0),
    ));
    tab2_view.addSubview(&txt_bg);

    // Foreground color
    let lbl_fg = NSTextField::labelWithString(ns_string!("Foreground (Hex):"), mtm);
    lbl_fg.setFrame(NSRect::new(
        NSPoint::new(20.0, 170.0),
        NSSize::new(150.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_fg);

    let txt_fg = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("#{:06X}", current_cfg.theme.foreground)),
        mtm,
    );
    txt_fg.setFrame(NSRect::new(
        NSPoint::new(180.0, 170.0),
        NSSize::new(120.0, 24.0),
    ));
    tab2_view.addSubview(&txt_fg);

    // Presets section
    let lbl_presets = NSTextField::labelWithString(ns_string!("Presets:"), mtm);
    lbl_presets.setFrame(NSRect::new(
        NSPoint::new(20.0, 120.0),
        NSSize::new(80.0, 22.0),
    ));
    tab2_view.addSubview(&lbl_presets);

    // Preset 1: Default Dark
    let btn_p1 = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Default Dark"),
            Some(target),
            Some(sel!(applyPresetDefaultDark:)),
            mtm,
        )
    };
    btn_p1.setFrame(NSRect::new(
        NSPoint::new(100.0, 115.0),
        NSSize::new(105.0, 28.0),
    ));
    tab2_view.addSubview(&btn_p1);

    // Preset 2: Tokyo Night
    let btn_p2 = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Tokyo Night"),
            Some(target),
            Some(sel!(applyPresetTokyoNight:)),
            mtm,
        )
    };
    btn_p2.setFrame(NSRect::new(
        NSPoint::new(215.0, 115.0),
        NSSize::new(105.0, 28.0),
    ));
    tab2_view.addSubview(&btn_p2);

    // Preset 3: Catppuccin
    let btn_p3 = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Catppuccin"),
            Some(target),
            Some(sel!(applyPresetCatppuccin:)),
            mtm,
        )
    };
    btn_p3.setFrame(NSRect::new(
        NSPoint::new(330.0, 115.0),
        NSSize::new(105.0, 28.0),
    ));
    tab2_view.addSubview(&btn_p3);

    // Preset 4: Solarized
    let btn_p4 = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Solarized"),
            Some(target),
            Some(sel!(applyPresetSolarized:)),
            mtm,
        )
    };
    btn_p4.setFrame(NSRect::new(
        NSPoint::new(100.0, 75.0),
        NSSize::new(105.0, 28.0),
    ));
    tab2_view.addSubview(&btn_p4);

    // Preset 5: Light Mode
    let btn_p5 = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Light Mode"),
            Some(target),
            Some(sel!(applyPresetLight:)),
            mtm,
        )
    };
    btn_p5.setFrame(NSRect::new(
        NSPoint::new(215.0, 75.0),
        NSSize::new(105.0, 28.0),
    ));
    tab2_view.addSubview(&btn_p5);

    let item2 = unsafe { NSTabViewItem::initWithIdentifier(mtm.alloc(), None) };
    item2.setLabel(ns_string!("Theme & Colors"));
    item2.setView(Some(&tab2_view));
    tab_view.addTabViewItem(&item2);

    // =========================================================================
    // Tab 3: Sidebar & AI Monitor
    // =========================================================================
    let tab3_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 320.0)),
    );

    // Sidebar Width: Label + Slider + Numeric Box
    let lbl_sb_width = NSTextField::labelWithString(ns_string!("Sidebar Width (px):"), mtm);
    lbl_sb_width.setFrame(NSRect::new(
        NSPoint::new(20.0, 260.0),
        NSSize::new(150.0, 22.0),
    ));
    tab3_view.addSubview(&lbl_sb_width);

    let slider_sb = NSSlider::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(180.0, 260.0), NSSize::new(170.0, 24.0)),
    );
    slider_sb.setMinValue(SIDEBAR_WIDTH_MIN as f64);
    slider_sb.setMaxValue(SIDEBAR_WIDTH_MAX as f64);
    slider_sb.setDoubleValue(current_cfg.sidebar.width_px as f64);
    unsafe {
        slider_sb.setTarget(Some(target));
        slider_sb.setAction(Some(sel!(sidebarWidthSliderChanged:)));
    }
    tab3_view.addSubview(&slider_sb);

    let txt_sb_width = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.sidebar.width_px)),
        mtm,
    );
    txt_sb_width.setFrame(NSRect::new(
        NSPoint::new(360.0, 260.0),
        NSSize::new(65.0, 24.0),
    ));
    tab3_view.addSubview(&txt_sb_width);

    let lbl_sb_hint = NSTextField::labelWithString(ns_string!("px"), mtm);
    lbl_sb_hint.setFrame(NSRect::new(
        NSPoint::new(432.0, 260.0),
        NSSize::new(30.0, 22.0),
    ));
    tab3_view.addSubview(&lbl_sb_hint);

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
        NSPoint::new(180.0, 220.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_git_branch.setState(if current_cfg.sidebar.show_git_branch {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab3_view.addSubview(&chk_git_branch);

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
        NSPoint::new(180.0, 185.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_agent_status.setState(if current_cfg.sidebar.show_agent_status {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab3_view.addSubview(&chk_agent_status);

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
        NSPoint::new(180.0, 150.0),
        NSSize::new(270.0, 24.0),
    ));
    chk_sb_collapsed.setState(if current_cfg.sidebar.collapsed {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    tab3_view.addSubview(&chk_sb_collapsed);

    // AI Monitor Width: Label + Slider + Numeric Box
    let lbl_mon_width = NSTextField::labelWithString(ns_string!("AI Monitor Width (px):"), mtm);
    lbl_mon_width.setFrame(NSRect::new(
        NSPoint::new(20.0, 95.0),
        NSSize::new(150.0, 22.0),
    ));
    tab3_view.addSubview(&lbl_mon_width);

    let slider_mon = NSSlider::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(180.0, 95.0), NSSize::new(170.0, 24.0)),
    );
    slider_mon.setMinValue(MONITOR_WIDTH_MIN as f64);
    slider_mon.setMaxValue(MONITOR_WIDTH_MAX as f64);
    slider_mon.setDoubleValue(current_cfg.ai_monitor.width_px as f64);
    unsafe {
        slider_mon.setTarget(Some(target));
        slider_mon.setAction(Some(sel!(monitorWidthSliderChanged:)));
    }
    tab3_view.addSubview(&slider_mon);

    let txt_mon_width = NSTextField::textFieldWithString(
        &NSString::from_str(&format!("{:.1}", current_cfg.ai_monitor.width_px)),
        mtm,
    );
    txt_mon_width.setFrame(NSRect::new(
        NSPoint::new(360.0, 95.0),
        NSSize::new(65.0, 24.0),
    ));
    tab3_view.addSubview(&txt_mon_width);

    let lbl_mon_hint = NSTextField::labelWithString(ns_string!("px"), mtm);
    lbl_mon_hint.setFrame(NSRect::new(
        NSPoint::new(432.0, 95.0),
        NSSize::new(30.0, 22.0),
    ));
    tab3_view.addSubview(&lbl_mon_hint);

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
    tab3_view.addSubview(&chk_mon_open);

    let item3 = unsafe { NSTabViewItem::initWithIdentifier(mtm.alloc(), None) };
    item3.setLabel(ns_string!("Sidebar & AI Monitor"));
    item3.setView(Some(&tab3_view));
    tab_view.addTabViewItem(&item3);

    content_view.addSubview(&tab_view);

    // 3. Status Label (x=20, y=50, w=480, h=20)
    let status_label = NSTextField::labelWithString(ns_string!(""), mtm);
    status_label.setFrame(NSRect::new(
        NSPoint::new(20.0, 50.0),
        NSSize::new(480.0, 20.0),
    ));
    content_view.addSubview(&status_label);

    // 4. Action Buttons (y=16)
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
        font_size_slider: slider_size,
        ligatures: chk_ligatures,
        columns: txt_cols,
        rows: txt_rows,
        bg_color: txt_bg,
        fg_color: txt_fg,
        sidebar_width: txt_sb_width,
        sidebar_width_slider: slider_sb,
        git_branch: chk_git_branch,
        agent_status: chk_agent_status,
        sidebar_collapsed: chk_sb_collapsed,
        ai_monitor_width: txt_mon_width,
        ai_monitor_width_slider: slider_mon,
        ai_monitor_open: chk_mon_open,
        status_label,
    });
}

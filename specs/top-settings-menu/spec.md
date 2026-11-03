# Spec: top-settings-menu

## Meta
- **Created**: 2026-11-02
- **Type**: dev
- **Status**: approved
- **Mode**: unattended
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-11-02

## Goal
I'd like to add setting menue on top side. like this image /Users/hyunhokim/.cache/mac-terminal/pasted/paste-1793662917-0.png

## Non-goals
- Adding GUI/widget toolkit dependencies (e.g., iced, egui) — prohibited by `.claude/rules/no-gui-crates.md`.
- Building an in-terminal GUI preferences panel or modal settings dialog (settings are TOML-configured and live-reloaded via `config.toml`).
- Browser-specific menu items shown in the Chrome reference screenshot (e.g., "Delete Browsing Data...", "Import Bookmarks and Settings...", "Warn Before Quitting").
- Cross-platform menu abstractions (macOS-only per `.claude/rules/macos-only.md`).

## Confirmed Goal
Add a native macOS top Application menu (`NSMenu`) on the system menu bar conforming to standard macOS AppKit conventions (as shown in the reference image), providing:
- "About mac-terminal" (`orderFrontStandardAboutPanel:`) (inferred)
- "Settings..." menu item with keyboard equivalent `⌘,` (`Cmd+,`), which resolves the configuration file path (`config.toml`), seeds default configuration if the file does not yet exist, and opens it using the system default editor via `open` or `NSWorkspace` (inferred)
- Standard macOS system menu affordances: "Services" submenu, "Hide mac-terminal" (`⌘H`), "Hide Others" (`⌥⌘H`), "Show All", and "Quit mac-terminal" (`⌘Q`) (inferred)
- In-app `Cmd+,` chord handling or AppKit action routing ensuring `Settings...` is triggered reliably via both menu click and keyboard shortcut (inferred)
- Clean integration within `crates/app` using existing `objc2` and `objc2-app-kit` platform bindings on the main thread during application launch (inferred)

## Research
- **Platform bindings & GUI rules**: `.claude/rules/no-gui-crates.md:1-20` strictly prohibits widget toolkits (`iced`, `egui`, etc.) but explicitly allows `objc2` and `objc2-app-kit` for native macOS affordances (menus, clipboard). `.claude/rules/macos-only.md:1-12` specifies macOS ARM64 only without cross-platform shims.
- **App dependencies**: `crates/app/Cargo.toml:22-24` already depends on `objc2 = "0.6.4"`, `objc2-app-kit = "0.3.2"`, and `objc2-foundation = "0.3.2"` with default features enabled, providing `NSMenu`, `NSMenuItem`, `NSApplication`, and `NSWorkspace`.
- **Application lifecycle & main thread**: `crates/app/src/main.rs:24-77` sets up the winit event loop and spawns background workers on the main thread before starting `App`. `crates/app/src/terminal_app.rs:97-100` demonstrates obtaining `MainThreadMarker::new()` and accessing `NSApplication::sharedApplication(mtm)`.
- **macOS menu bar architecture**: In AppKit, the top menu bar is set via `NSApplication::setMainMenu(&main_menu)`. The main menu contains top-level items whose submenus represent menu bars (the first being the Application menu titled with the app name). Standard items ("About", "Services", "Hide", "Hide Others", "Show All", "Quit") connect to standard selectors (`orderFrontStandardAboutPanel:`, `hide:`, `hideOtherApplications:`, `unhideAllApplications:`, `terminate:`).
- **Configuration path & seeding**: `crates/common/src/config.rs:192-206` defines `Config::path()` returning `~/Library/Application Support/mac-terminal/config.toml`. `config.example.toml:1-39` provides an annotated exemplar. If `config.toml` does not exist, creating parent directories and writing a template ensures immediate usability when opened.
- **Live settings reloading**: `crates/app/src/settings.rs:61-74` applies changes from `ConfigWatcher` dynamically when `config.toml` changes, updating renderer insets, font size, cell metrics, tab colors, and IME preedit styling without restarting.
- **Command invocation pattern**: `crates/app/src/capture.rs:1-30` uses `std::process::Command` to invoke native macOS commands like `screencapture`. Invoking `/usr/bin/open` with `config.toml` opens the user's preferred editor seamlessly.
- **Key dispatch & chord detection**: `crates/app/src/terminal_app.rs:2086-2236` and `crates/app/src/input.rs:76-107` (`chord_key`) handle key combinations. Adding `is_settings_chord` (detecting `Cmd+,`) in `input.rs` and intercepting it in `terminal_app.rs` ensures `Cmd+,` operates reliably from keyboard input alongside menu item selection.

## Decisions

### D1: Native AppKit menu bar initialized via `objc2-app-kit` `NSMenu` on main thread
- **Status**: resolved
- **Rationale**: The user request and screenshot demonstrate a standard macOS top menu bar application menu. `.claude/rules/no-gui-crates.md` explicitly allows `objc2` and `objc2-app-kit` for native platform affordances like menus. AppKit requires all menu objects to be created and modified on the main thread. Setting `NSApplication::sharedApplication(mtm).setMainMenu(&main_menu)` during app startup initializes the menu bar natively without GUI toolkit dependencies.

### D2: "Settings..." opens `config.toml` via `/usr/bin/open`, seeding defaults if absent
- **Status**: resolved
- **Rationale**: The project config is file-based and live-reloaded on save by `ConfigWatcher` (`crates/app/src/settings.rs`), while widget toolkits are strictly forbidden (`no-gui-crates.md`). Invoking `/usr/bin/open` with `Config::path()` (`~/Library/Application Support/mac-terminal/config.toml`) opens the configuration in the user's default text editor. If the file does not exist, creating parent directories and copying `config.example.toml` (or writing the default TOML) guarantees the user immediately sees a valid, documented configuration template.
- **Alternatives rejected**: In-app modal dialog (violates `no-gui-crates`), terminal-based editor inside PTY (interrupts active shell workflow and requires complex terminal multiplexing).

### D3: Menu item layout adheres to standard macOS Application Menu conventions
- **Status**: resolved
- **Rationale**: Replicates the standard macOS Application menu structure visible in the reference image:
  1. "About mac-terminal" (`orderFrontStandardAboutPanel:`)
  2. Separator
  3. "Settings..." (key equivalent `","`, modifier `Cmd`)
  4. Separator
  5. "Services" (submenu linked to `NSApplication.setServicesMenu`)
  6. Separator
  7. "Hide mac-terminal" (`⌘H`, `hide:`)
  8. "Hide Others" (`⌥⌘H`, `hideOtherApplications:`)
  9. "Show All" (`unhideAllApplications:`)
  10. Separator
  11. "Quit mac-terminal" (`⌘Q`, `terminate:`)
  Browser-specific items in the reference image ("Delete Browsing Data...", "Import Bookmarks and Settings...") are rejected as out of scope.

### D4: Dual-path activation via AppKit menu action and in-app `Cmd+,` key chord
- **Status**: assumed
- **Rationale**: AppKit automatically routes `Cmd+,` when a menu item has `keyEquivalent = ","` and a valid action. However, because winit captures keyboard input when the window is focused, adding `is_settings_chord` in `crates/app/src/input.rs` and routing it in `terminal_app.rs` ensures that `Cmd+,` triggers settings reliably regardless of window focus state or event capture nuances. Both paths call the single shared function `open_settings()`.

### D5: Non-blocking execution of `/usr/bin/open` with error handling
- **Status**: assumed
- **Rationale**: Launching `/usr/bin/open` via `std::process::Command::new("/usr/bin/open").arg(&path).spawn()` runs asynchronously without blocking the winit event loop or frame rendering. If file creation or process spawning fails, a warning is emitted via `tracing::warn!` without crashing the application.

### D6: Dedicated `crates/app/src/settings_menu.rs` module for menu construction
- **Status**: assumed
- **Rationale**: Keeps `terminal_app.rs` and `main.rs` clean. All menu-related logic (`install_app_menu`, `open_settings`, target-action delegates, and menu structure assembly) is contained within `settings_menu.rs`, exposing a clean public API `install_app_menu(mtm: MainThreadMarker)`.

## Constraints
- **C1: macOS ARM64 only**: Enforced by `.claude/rules/macos-only.md`. No cross-platform `#[cfg]` abstractions.
- **C2: No GUI/widget crates**: Enforced by `.claude/rules/no-gui-crates.md`. Only `winit` and `objc2` allowed.
- **C3: Main thread execution**: AppKit `NSMenu` and `NSApplication` operations must execute on the main thread using `objc2::MainThreadMarker`.
- **C4: Non-blocking UI**: Process spawning and config file preparation must not block the winit event loop.

## Known Gaps
- (none)

## Requirements

### R0: macOS Application Menu with Settings Integration — D1, D2, D3, D4, D5, D6
The top-level application provides a native macOS Application menu in the system menu bar containing About, Settings (⌘,), Services, Hide (⌘H), Hide Others (⌥⌘H), Show All, and Quit (⌘Q), allowing the user to view and open app settings cleanly.

#### R0.1: Main menu bar contains the Application menu (manual)
- **Given**: The terminal application is launched on macOS
- **When**: The user observes the system top menu bar
- **Then**: An Application menu labeled with the app name ("mac-terminal") is present and clickable at the top left of the screen

#### R0.2: Settings item is visible with Command-Comma shortcut (manual)
- **Given**: The Application menu is opened from the system top menu bar
- **When**: The user inspects the menu items
- **Then**: A "Settings..." menu item is displayed with key equivalent `⌘,` alongside About, Services, Hide, Hide Others, Show All, and Quit items separated by standard dividers

### R1: Native AppKit Menu Assembly and Installation — D1, D3, D6
The application constructs and registers an `NSMenu` instance containing the standard macOS Application menu on the main thread during startup.

#### R1.1: Menu structure and standard actions assembly
- **Given**: A valid `MainThreadMarker`
- **When**: `crates/app/src/settings_menu.rs::build_app_menu(mtm)` is called
- **Then**: It returns a retained `NSMenu` whose items include "About mac-terminal" (`orderFrontStandardAboutPanel:`), separator, "Settings..." (key equivalent `","`), separator, "Services" (`NSMenu`), separator, "Hide mac-terminal" (`hide:`), "Hide Others" (`hideOtherApplications:`), "Show All" (`unhideAllApplications:`), separator, and "Quit mac-terminal" (`terminate:`)

#### R1.2: Menu bar installation on NSApplication
- **Given**: A valid `MainThreadMarker` and the application event loop initialized on the main thread
- **When**: `crates/app/src/settings_menu.rs::install_app_menu(mtm)` is called
- **Then**: `NSApplication::sharedApplication(mtm).mainMenu()` is populated with a top menu bar containing the assembled Application menu

### R2: Settings File Resolution, Seeding, and Editor Launch — D2, D5, D6
Activating settings resolves the `config.toml` path, creates default template configuration if the file does not exist, and launches the system default editor.

#### R2.1: Seed default configuration when config.toml is missing
- **Given**: A target file path where no file exists (e.g. temporary path or missing `Config::path()`)
- **When**: `crates/app/src/settings_menu.rs::ensure_config_file_exists_at(path)` is called
- **Then**: Parent directories are created, default TOML content is written to `path`, and `Ok(path.to_path_buf())` is returned

#### R2.2: Existing configuration preserved without overwrite
- **Given**: An existing `config.toml` exists at `path` with user modifications
- **When**: `crates/app/src/settings_menu.rs::ensure_config_file_exists_at(path)` is called
- **Then**: The file on disk is untouched and `Ok(path.to_path_buf())` is returned with the existing file path

#### R2.3: Launch system default editor via open command
- **Given**: A valid configuration file path exists
- **When**: `crates/app/src/settings_menu.rs::open_settings_file(&path)` is called
- **Then**: It executes `/usr/bin/open` with `path` as an argument without blocking the caller and returns `Ok(())`

#### R2.4: Safe error handling on launch or file creation failure
- **Given**: `ensure_config_file_exists` or `/usr/bin/open` encounters an I/O error
- **When**: `crates/app/src/settings_menu.rs::open_settings()` handles the error
- **Then**: A warning is logged via `tracing::warn!`, the function returns without crashing, and the terminal event loop continues running

### R3: In-App Keyboard Chord Detection and Dispatch for Cmd+, — D4
Pressing `Cmd+,` while the application window is focused intercepts the key combination and triggers `open_settings()`.

#### R3.1: Identify Cmd+, chord in input module
- **Given**: A keyboard input event with modifier `ModifiersState::SUPER` and key `,`
- **When**: `crates/app/src/input.rs::is_settings_chord(input)` is called
- **Then**: Returns `true` for `Cmd+,` and `false` for `,` without Cmd, or other keys with Cmd

#### R3.2: Dispatch Cmd+, to open_settings in application event handler
- **Given**: The terminal window has focus and receives a key event matching `is_settings_chord`
- **When**: `crates/app/src/terminal_app.rs::Running::on_key` evaluates the input
- **Then**: It invokes `crates/app/src/settings_menu.rs::open_settings()`, returns `true` (consuming the key event), and prevents `,` from being emitted to `encode_key` or the active PTY

## Tasks

### T1: Scaffolding settings_menu module and input chord stub [infra]
- **Fulfills**: R0
- **Depends on**: (none)
- Sets up `crates/app/src/settings_menu.rs` module stub, adds `is_settings_chord` stub in `crates/app/src/input.rs`, and registers/re-exports testable symbols (`build_app_menu`, `install_app_menu`, `ensure_config_file_exists`, `ensure_config_file_exists_at`, `open_settings_file`, `open_settings`, `is_settings_chord`) in `crates/app/src/lib.rs` for integration test accessibility.

### TT1: acceptance tests for R1's GWTs
- **Fulfills**: R1 (tests)
- **Depends on**: T1
- Adds `crates/app/tests/tt1_settings_menu_assembly.rs` asserting `build_app_menu` and `install_app_menu` build the expected `NSMenu` structure on the main thread with matching menu items, titles, key equivalents, and selectors.

### TT2: acceptance tests for R2's GWTs
- **Fulfills**: R2 (tests)
- **Depends on**: T1
- Adds `crates/app/tests/tt2_settings_file_launch.rs` asserting `ensure_config_file_exists_at` creates parent dirs and seeds default config if absent in a tempdir, preserves existing content when present, and `open_settings` logs warnings on error without panicking.

### TT3: acceptance tests for R3's GWTs
- **Fulfills**: R3 (tests)
- **Depends on**: T1
- Adds `crates/app/tests/tt3_settings_chord_dispatch.rs` asserting `is_settings_chord` identifies `Cmd+,` and rejects non-`Cmd+,` inputs, and key event routing consumes the chord.

### T2: Implement settings file resolution, default seeding, and /usr/bin/open launcher [vertical]
- **Fulfills**: R2
- **Depends on**: T1, TT2
- Implements `ensure_config_file_exists_at(path)`, `ensure_config_file_exists()`, `open_settings_file(path)`, and `open_settings()` in `crates/app/src/settings_menu.rs`, seeding missing configuration from default TOML and invoking `/usr/bin/open` asynchronously.

### T3: Implement AppKit Application menu assembly and NSApplication installation [vertical]
- **Fulfills**: R0, R1
- **Depends on**: T1, TT1, T2
- Implements `build_app_menu(mtm)` and `install_app_menu(mtm)` in `crates/app/src/settings_menu.rs` connecting About, Settings (linked to `open_settings`), Services, Hide, Hide Others, Show All, and Quit. Wires `install_app_menu` into application startup in `crates/app/src/main.rs`.

### T4: Implement Cmd+, chord detection in input.rs and wire key dispatch in terminal_app.rs [vertical]
- **Fulfills**: R3
- **Depends on**: T1, TT3, T2  ← parallel with T3
- Implements `is_settings_chord` in `crates/app/src/input.rs` and routes it in `crates/app/src/terminal_app.rs` `Running::on_key` to call `open_settings()` and return `true` (consuming the event).

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)

## Spec Review
- **Verdict**: PASS
- **Reviews**: 3 rounds (Round 1: 4 findings addressed for readiness, testability, and coverage; Round 2: T1 fulfills requirement addressed; Round 3: PASS with 0 issues)
- **Issues**: none

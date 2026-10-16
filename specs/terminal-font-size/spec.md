# Spec: terminal-font-size

## Meta
- **Created**: 2026-10-16
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-10-16

## Goal
Add function for font size up and down on terminal.

## Non-goals
- Changing font family or font weights
- Custom color themes or unrelated appearance settings
- Overhauling unrelated preferences UI

## Confirmed Goal
Allow users to dynamically adjust the terminal's font size (increase, decrease, and reset to default size) within defined minimum and maximum bounds, ensuring the terminal grid/layout reflows properly to match the updated font metrics and automated tests verify the behavior.

## Research
- **Font configuration & defaults**: `FontConfig` in `crates/common/src/config.rs:76-82` defines `family: String`, `size_pt: f32`, and `ligatures: bool`, with default `size_pt: 14.0` (`crates/common/src/config.rs:131-142`). Currently `Config::normalize()` clamps `sidebar.width_px` and `ai_monitor.width_px` (`crates/common/src/config.rs:219-237`), while `font.size_pt` has no clamp or write-back helper (`save_table` in `crates/common/src/config.rs:334-382` is already parameterized over table name and key-value entries).
- **Renderer font metrics & live config application**:
  - `cell_size_px(cfg, scale_factor)` derives cell dimensions in physical pixels from `cfg.font.size_pt * scale_factor as f32` as `((px * 0.6).round().max(1.0), (px * 1.2).round().max(1.0))` (`crates/renderer/src/lib.rs:1913-1916`).
  - `Renderer::apply_config(&mut self, cfg: &Config)` (`crates/renderer/src/lib.rs:819-891`) re-derives `self.cell_size` and `self.ppem`, rebuilds the `Atlas` at the new `ppem` (`Atlas::new(&self.device, &self.fonts, self.ppem)`), recreates `self.glyph_bind_group`, invalidates `self.last_snapshot = None`, and calls `self.sync_left_inset()` (which writes uniforms and chains `self.sync_right_inset()`, since `monitor_band_px` depends on `self.cell_size.0` for `MIN_TERMINAL_COLS`).
  - Sidebar cards use their own fixed-size `CardAtlases` (`17px` and `12.5px` scaled by `scale_factor`) independent of `font.size_pt` (`crates/renderer/src/sidebar_text.rs:1-10`). However, `monitor_panel.rs` (`crates/renderer/src/monitor_panel.rs:547, 662, 828`) and `preedit_layer.rs` (`crates/renderer/src/preedit_layer.rs:383, 406, 518`) use `ctx.atlas` and `ctx.cell`, automatically invalidating their bind groups via `ctx.atlas.epoch()` and re-laying out when `ctx.cell` changes.
- **Grid refit & live settings**: `settings::apply` (`crates/app/src/settings.rs:61-74`) calls `renderer.apply_config(cfg)`, then `settings::refit(renderer, tabs)` (`crates/app/src/settings.rs:55-59`) which queries `renderer.grid_size()` and calls `tabs.resize_all(cols, rows)` across all open tabs.
- **Keyboard dispatch & chord handling**:
  - Key dispatch order in `Running::on_key` (`crates/app/src/terminal_app.rs:2086-2236`): Escape for open card menu → `overlays.dispatch` → `router.dispatch` → `is_sidebar_toggle_chord` (`Cmd+Shift+S`) → `tab_action` (`Cmd+T/W/[/]/1..9`) → `encode_key` to active PTY.
  - Korean 2-Set IME chord normalization (`input::chord_key` in `crates/app/src/input.rs:76-107` and mid-composition replay in `crates/app/src/ime_chord.rs:96-141`) passes non-letter ASCII printable keys (`+`, `-`, `=`, `0`) through unchanged when `Cmd` is held.
  - `Renderer` currently holds runtime overrides for `sidebar_override: SidebarOverride` (`crates/renderer/src/lib.rs:186-189, 231`) and `monitor_width_override: Option<f32>` (`crates/renderer/src/lib.rs:247`), with echo-suppression rings `written_sidebar` and `written_monitor` in `Running` (`crates/app/src/terminal_app.rs:298-305, 2374-2408`).
- **Build, test, and architecture rules**:
  - Dependency direction: `common ← term-core ← renderer ← app` (`.claude/rules/dependency-direction.md`, `CLAUDE.md`).
  - Verification commands: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo mutants -f <file> -j 4` (`CLAUDE.md`).
  - No `unwrap()` outside tests (`CLAUDE.md`).

## Decisions

### D1: Font size changes reuse the `Renderer` atlas-swap + `settings::refit` pipeline
- **Status**: assumed
- **Rationale**: L1 shows `Renderer::apply_config` (`crates/renderer/src/lib.rs:819-891`) already re-derives `cell_size` and `ppem`, rebuilds `Atlas` from the shared `FontSet` mmaps without disk I/O, updates `glyph_bind_group`, invalidates `last_snapshot`, and re-syncs both side insets (`sync_left_inset` → `sync_right_inset`), after which `settings::refit` (`crates/app/src/settings.rs:55-59`) resizes all tabs to the new `renderer.grid_size()`. Reusing this pipeline avoids a second glyph-scaling mechanism. Rejected alternative: scaling glyph quads in the vertex shader without rebuilding the atlas (produces blurry/pixelated glyphs and breaks `cell_size_px` integer rounding invariants).

### D2: Sidebar card text remains at fixed design sizes; grid, preedit, and ai-monitor follow terminal `cell_size`
- **Status**: assumed
- **Rationale**: Per `crates/renderer/src/sidebar_text.rs:1-10` (`sidebar-weather-cards` D2), the left tab sidebar is application chrome with its own `CardAtlases` (`17px` title, `12.5px` meta scaled only by `scale_factor`). Meanwhile, the terminal grid, inline IME `PreeditLayer`, and `MonitorPanel` read `ctx.cell` and `ctx.atlas` (keying their text bind groups on `ctx.atlas.epoch()`). Rejected alternative: scaling sidebar card fonts with terminal font size (violates `sidebar_text.rs` D2 and breaks the fixed `260..=440` logical px card layout).

### D3: `Config::normalize` clamps `font.size_pt` at load/reload with a warning and falls back to `14.0` on non-finite values
- **Status**: assumed
- **Rationale**: `crates/common/src/config.rs:219-258` already clamps `sidebar.width_px` and `ai_monitor.width_px` via `clamp_width` during `Config::load_from` so a hand-edited typo or `NaN` in `config.toml` warns once and clamps rather than failing startup or poisoning `cell_size_px` / `grid_dims` with `NaN` or `0`. Applying the same normalization rule to `font.size_pt` keeps config loading consistent. Rejected alternative: rejecting out-of-range `font.size_pt` with a parse error (violates `config.rs` D2 that cosmetic fields never block terminal startup).

### D4: Narrow-window terminal column floor (`MIN_TERMINAL_COLS = 20`) automatically re-evaluates on font size increase
- **Status**: assumed
- **Rationale**: Increasing `font.size_pt` widens `cell_size.0`, which increases `MIN_TERMINAL_COLS as f32 * cell_w` in `monitor_band_px` (`crates/renderer/src/lib.rs:1982-1991`). Calling `sync_right_inset()` whenever font size changes ensures the docked `ai-monitor` panel automatically squeezes or force-closes if the larger font leaves fewer than 20 terminal columns, without altering the persisted `[ai_monitor].open` intent. Rejected alternative: capping font size dynamically based on window width (confusing UX when resizing windows).

### D5: Crate dependency direction and `GridSnapshot` contract remain unchanged
- **Status**: assumed
- **Rationale**: `common ← term-core ← renderer ← app` is a hard architectural rule (`.claude/rules/dependency-direction.md`). `term-core` only sees the new `(cols, rows)` via `Tabs::resize_all` (`settings::refit`); `GridSnapshot` needs no font-size field. Rejected alternative: storing font size inside `term-core::Terminal` (violates headless GPU-free separation).

### D6: Idle-CPU cache and atlas bind-group epoch invariants are preserved
- **Status**: assumed
- **Rationale**: Rebuilding `Atlas` bumps `Atlas::epoch()` (via the texture factory in `crates/renderer/src/atlas.rs`) and sets `self.last_snapshot = None` + `inner.redraw()`, forcing exactly one re-shape on the frame following a font size change while keeping 0 re-shapes and 0 bind-group rebuilds on subsequent idle frames (`docs/architecture.md` G1). Rejected alternative: clearing or rebuilding buffers per frame.

### D7: Keyboard chords `Cmd+=` / `Cmd++` (+1 pt), `Cmd+-` (-1 pt), and `Cmd+0` (reset to 14.0 pt), bounded to `[8.0, 32.0]` pt
- **Status**: resolved
- **Rationale**: Standard macOS terminal zoom chords (`Cmd+=` and `Cmd+Shift+=` / `+` for increase, `Cmd+-` for decrease, `Cmd+0` for reset to `FONT_SIZE_DEFAULT = 14.0`) with `FONT_SIZE_STEP = 1.0`, `FONT_SIZE_MIN = 8.0`, and `FONT_SIZE_MAX = 32.0`. Below `8.0 pt` (`cell_w = 5px` at 1×), glyphs become illegible; above `32.0 pt` (`cell_w = 19px` at 1×), a standard laptop window with sidebar open fits barely 25 columns. Reset targets `FONT_SIZE_DEFAULT` (`14.0`) deterministically so `Cmd+0` always returns to the known baseline even if `config.toml` was saved at a zoomed size in a previous session. Rejected alternatives: `±2.0 pt` step with `[6.0, 48.0]` bounds (too coarse per press and allows unusable extremes); resetting to the session's initial config value (since changes persist to `config.toml`, the "initial" value on next launch would be the zoomed value, making `Cmd+0` unable to return to `14.0`).

### D8: Font size is window-global across all open tabs
- **Status**: resolved
- **Rationale**: `Renderer` owns a single glyph `Atlas` and `cell_size` shared by the window (`crates/renderer/src/lib.rs:206, 216`), and `settings::refit` (`crates/app/src/settings.rs:55-59`) resizes all tabs together (`tabs.resize_all(cols, rows)`). Keeping font size window-global matches existing live-config reload behavior and avoids rebuilding the GPU glyph atlas on every tab switch. Rejected alternative: per-tab font size (would force an expensive 2048² `Atlas::new` + `Grid::resize` on tab switch or require N separate GPU atlases).

### D9: Persist `[font].size_pt` to `config.toml` via surgical `save_font_size`, with runtime override and echo suppression
- **Status**: resolved
- **Rationale**: Mirrors the exact persistence architecture of `[sidebar]` and `[ai_monitor]` (`crates/common/src/config.rs:294-318`, `crates/app/src/terminal_app.rs:1439-1528, 2374-2408`):
  1. `common::config::save_font_size(path, size_pt)` calls the existing surgical `save_table(path, "font", &[("size_pt", fmt_f32(size_pt))])`, preserving comments, formatting, `family`, and `ligatures` byte-for-byte via atomic `config.toml.tmp` rename.
  2. `Renderer` stores `font_size_pt: f32` (from `Config`) alongside `font_size_override: Option<f32>`, exposing `effective_font_size(&self) -> f32` (`self.font_size_override.unwrap_or(self.font_size_pt)`), `font_size_logical(&self) -> f32`, `set_font_size(&mut self, size_pt: f32) -> bool`, and `clear_font_override(&mut self)`. Crucially, `Renderer::apply_config` updates `self.font_size_pt = cfg.font.size_pt` and derives `cell_size` and `ppem` from `self.effective_font_size()` (via `cell_size_for_pt(self.effective_font_size(), self.scale_factor)`), so an unrelated config reload while `font_override_authoritative` is true (e.g., if `save_font_size` failed on a read-only disk) never wipes the user's runtime font override.
  3. `Running` holds `last_written_font: Option<f32>` (the most recent write only) and `font_override_authoritative: bool`. Unlike the sidebar's `RECENT_WRITES` ring, one value is kept because font sizes are whole-point steps that recur (14→15→14): a ring would hold most plausible hand edits and swallow them as echoes (zoom 14→15→16→15→14, hand-edit to 16, and the window stays at 14). The accepted cost: a delivery read between two rapid zoom writes reads as a human edit for one delivery, and the next delivery settles it. On a successful save, `last_applied_cfg.font.size_pt` is updated so the watcher's own echo is suppressed by `suppress_delivery` (`delivered == applied`), while an external human edit to `[font].size_pt` in `config.toml` classifies as `ConfigDelivery::HumanEdit`, clears `font_size_override`, and applies live.
  4. **Steelman counterargument against persisting runtime zoom (`Cmd++`/`Cmd+-`) to `config.toml`**: In browsers or some terminals (e.g., iTerm2 temporary zoom), users might temporarily zoom in during a screen share and expect a new window or restart to return to `14.0 pt`. Why persistence wins here: (a) this terminal is a single-window app where `[sidebar]` and `[ai_monitor]` layout changes already persist immediately via `save_table`, creating a consistent "your workspace stays how you left it" mental model; and (b) `Cmd+0` is explicitly defined in D7 to reset to `FONT_SIZE_DEFAULT = 14.0` (rather than session-initial config) and persist `14.0`, giving users a single-chord recovery after a screen share even across restarts.
  Rejected alternative: session-only in-memory zoom (loses user's preferred font size on restart and diverges from how sidebar and monitor sizing behave in this app).

### D10: Boundary no-op guard — clamping to an unchanged `size_pt` skips atlas rebuild, `settings::refit`, and config persistence
- **Status**: resolved
- **Rationale**: Pressing `Cmd+-` at `FONT_SIZE_MIN` (`8.0`), `Cmd+=`/`Cmd++` at `FONT_SIZE_MAX` (`32.0`), or `Cmd+0` when already at `FONT_SIZE_DEFAULT` (`14.0`) still consumes the chord (`Handled::Consumed` / early return before `encode_key`) so the key never leaks to the PTY shell, but `Renderer::set_font_size` clamps `size_pt` to `[FONT_SIZE_MIN, FONT_SIZE_MAX]` (falling back to `FONT_SIZE_DEFAULT` if non-finite) and returns `false` without rebuilding `Atlas` or clearing `last_snapshot` when `(clamped - self.effective_font_size()).abs() == 0.0`. Skipping the no-op prevents an unnecessary 2048² GPU texture allocation, preserves the user's active grid text selection (which `Grid::resize` inside `settings::refit` clears), avoids sending a spurious `SIGWINCH` to every tab's shell, and avoids touching `config.toml`. Rejected alternative: unconditionally running `set_font_size` + `refit` + `persist_font_size` on every chord press (destroys active text selection and wastes GPU/disk work when already at a boundary).

### D11: Dispatch priority, modal interaction, and Korean IME compatibility for font chords
- **Status**: resolved
- **Rationale**:
  1. **Chord matching (`input::font_action`)**: Pure function `pub fn font_action(key: &Key, mods: &ModifiersState) -> Option<FontAction>` with variants `FontAction::{Increase, Decrease, Reset}`. Requires `mods.super_key() && !mods.alt_key() && !mods.control_key()`. Matches `Key::Character`:
     - `"="` or `"+"` (with or without `Shift`, since `+` requires `Shift` on US/ANSI layouts) → `Some(FontAction::Increase)`
     - `"-"` when `!mods.shift_key()` → `Some(FontAction::Decrease)`
     - `"0"` when `!mods.shift_key()` → `Some(FontAction::Reset)`
  2. **Dispatch order in `Running::on_key`**: Placed alongside app-level window chords (`is_sidebar_toggle_chord` and `tab_action`, after `overlays.dispatch` and `router.dispatch`, before `encode_key`). When `RenameOverlay` (priority 200) is open, it consumes keys before reaching `font_action`. When `font_action` matches while a sidebar card menu is open, it dismisses the card menu (`self.close_card_menu()`) for parity with `do_tab_action` and `finish_consumed_key`.
  3. **Post-change UI bookkeeping**: When `do_font_action` changes the effective font size (`renderer.set_font_size(target)` returns `true`), it snapshots `before = self.renderer.grid_size()` before the change and `after = self.renderer.grid_size()` after, calls `settings::refit(&self.renderer, &mut self.tabs)` when `input::needs_refit(before, after)` is true (avoiding unnecessary `Grid::resize` selection clears if `(cols, rows)` happened not to change), re-clamps sidebar scroll and hover (`sidebar_input` reads `cell_size`), calls `self.update_ime_cursor_area()` (safe no-op when `!self.composing`), calls `self.inner.redraw()`, and persists via `self.persist_font_size()`.
  4. **Korean 2-Set IME**: Under 2-Set Korean, `=`, `+`, `-`, `0` are ASCII graphics, so both idle `input::chord_key` and mid-composition `ime_chord::replay_decision` pass them through intact to `on_key`.
  Rejected alternative: handling font chords before `overlays.dispatch` (would allow zooming while typing inside the modal tab-rename input field, violating `D8a.1` keyboard ownership).

## Constraints
- **Dependency direction**: `common ← term-core ← renderer ← app` (`.claude/rules/dependency-direction.md`). `term-core` must not depend on font size or GPU/window types.
- **No GUI crates**: Pure `wgpu`/Metal rendering (`.claude/rules/no-gui-crates.md`).
- **Idle-CPU and bind-group cache invariants**: Rebuilding the glyph `Atlas` must occur only when `effective_font_size` (or `apply_config`) actually changes `ppem`, never per frame (`docs/architecture.md` G1, `specs/atlas-swap-bind-invalidation`).
- **Surgical config persistence**: Writing `[font].size_pt` to `config.toml` must use `save_table` + atomic rename (`config.toml.tmp`), preserving all comments, key order, and unrelated sections byte-for-byte.
- **No `unwrap()` outside tests** (`CLAUDE.md`).
- **Verification gates**: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo mutants` on modified files.

## Known Gaps
- **G1: `Renderer::apply_config` still rebuilds the `Atlas` on non-font config reloads** (inherited from `specs/atlas-swap-bind-invalidation` G-b). While `set_font_size` guards against no-op rebuilds when the clamped `size_pt` is unchanged, `apply_config` (triggered by external `config.toml` edits) continues to rebuild the atlas using `self.effective_font_size()`. Optimizing `apply_config` to skip `Atlas::new` when `ppem` is unchanged remains a separate concern.
- **G2: `Atlas::new` allocation failure fallback** (inherited from `specs/atlas-swap-bind-invalidation` G-d). If GPU texture allocation fails during `set_font_size` or `apply_config`, the old `Atlas` is retained while `cell_size` and `ppem` reflect the new size.

## Requirements

### R0: Dynamic terminal font size adjustment across the window — Confirmed Goal, D7, D8

#### R0.1: End-to-end font size increase, decrease, and reset refits all open tabs and persists
- **Given**: A running terminal window with two open tabs at the default `14.0 pt` font size
- **When**: The user presses `Cmd+=` (or `Cmd++`), `Cmd+-`, or `Cmd+0`
- **Then**: The window's effective font size changes to `15.0 pt`, `13.0 pt`, or `14.0 pt` respectively (clamped to `[8.0, 32.0]`), both open tabs resize to the new `(cols, rows)` derived from the updated `cell_size`, and `[font].size_pt` is persisted to `config.toml`

### R1: Font size bounds, normalization, and surgical config persistence (`crates/common`) — D3, D5, D7, D9

#### R1.1: Font size constants exported in `common::config`
- **Given**: `crates/common/src/config.rs`
- **When**: Downstream crates import font sizing constants
- **Then**: `FONT_SIZE_MIN` (`8.0_f32`), `FONT_SIZE_MAX` (`32.0_f32`), `FONT_SIZE_DEFAULT` (`14.0_f32`), and `FONT_SIZE_STEP` (`1.0_f32`) are publicly exported and `FontConfig::default().size_pt` equals `FONT_SIZE_DEFAULT`

#### R1.2: `Config::normalize` clamps out-of-range `font.size_pt` with a warning
- **Given**: A `Config` loaded from TOML with `font.size_pt = 4.0` (below `8.0`) or `font.size_pt = 60.0` (above `32.0`)
- **When**: `Config::load_from` (or `Config::normalize`) runs
- **Then**: `cfg.font.size_pt` is clamped to `8.0` or `32.0` respectively and a single `tracing::warn!` is emitted naming `"font.size_pt"`

#### R1.3: `Config::normalize` replaces non-finite `font.size_pt` with `FONT_SIZE_DEFAULT` (`14.0`)
- **Given**: A `Config` with `font.size_pt = f32::NAN` or `f32::INFINITY`
- **When**: `Config::normalize` runs
- **Then**: `cfg.font.size_pt` is set to `FONT_SIZE_DEFAULT` (`14.0`) and a `tracing::warn!` is emitted

#### R1.4: `save_font_size` surgically updates `[font].size_pt` while preserving unrelated TOML content
- **Given**: An existing `config.toml` containing comments, `[font].family = "JetBrains Mono"`, `[font].ligatures = true`, `[sidebar]`, and `[ai_monitor]` tables
- **When**: `common::config::save_font_size(path, 16.0)` is called
- **Then**: `[font].size_pt` on disk is updated to `16.0` via atomic `config.toml.tmp` rename, and all comments, `[font].family`, `[font].ligatures`, and other tables remain byte-for-byte intact

### R2: Renderer font size override, atlas rebuild, inset re-evaluation, and cache invariants (`crates/renderer`) — D1, D2, D4, D5, D6, D9, D10

#### R2.1: `set_font_size` updates runtime override, `cell_size`, `ppem`, and rebuilds `Atlas` when clamped size changes
- **Given**: A `Renderer` initialized at `14.0 pt` (`font_size_override == None`)
- **When**: `renderer.set_font_size(15.0)` is called
- **Then**: `renderer.set_font_size(15.0)` returns `true`, `renderer.font_size_logical()` returns `15.0`, `cell_size` and `ppem` update to match `15.0 * scale_factor`, `Atlas` is rebuilt with an incremented `Atlas::epoch()`, `glyph_bind_group` is recreated, `last_snapshot` is set to `None`, and `sync_left_inset()` / `sync_right_inset()` update the viewport and background uniforms

#### R2.2: `set_font_size` returns `false` and skips `Atlas` rebuild when clamped size equals effective font size
- **Given**: A `Renderer` whose `effective_font_size()` is `8.0` (or `32.0`, or `14.0`)
- **When**: `renderer.set_font_size(7.0)` (clamps to `8.0`), `renderer.set_font_size(33.0)` (clamps to `32.0`), or `renderer.set_font_size(14.0)` is called
- **Then**: `renderer.set_font_size(...)` returns `false`, `Atlas::epoch()` is unchanged, and `last_snapshot` is not cleared

#### R2.3: `apply_config` preserves authoritative runtime font override while updating baseline `font_size_pt`
- **Given**: A `Renderer` with `font_size_pt = 14.0` and `font_size_override = Some(18.0)`
- **When**: `renderer.apply_config(&cfg)` is called with a `Config` where `cfg.font.size_pt == 14.0` (e.g., an unrelated reload while `font_override_authoritative` is true)
- **Then**: `renderer.font_size_pt` updates to `14.0`, `renderer.font_size_logical()` remains `18.0`, and `cell_size` and `ppem` are derived from `18.0`

#### R2.4: `clear_font_override` restores effective font size to config baseline
- **Given**: A `Renderer` with `font_size_pt = 16.0` and `font_size_override = Some(18.0)`
- **When**: `renderer.clear_font_override()` is called followed by `renderer.apply_config(&cfg)` (with `cfg.font.size_pt = 16.0`)
- **Then**: `renderer.font_size_override` is `None` and `renderer.font_size_logical()` returns `16.0`

#### R2.5: Font size increase re-evaluates right inset against `MIN_TERMINAL_COLS` (`20`) without mutating persisted monitor open state
- **Given**: A window with the `ai-monitor` panel open where increasing `cell_size.0` leaves fewer than `MIN_TERMINAL_COLS` (`20`) terminal columns alongside the docked monitor band
- **When**: `renderer.set_font_size(32.0)` runs `sync_right_inset()`
- **Then**: `monitor_band_px` narrows or collapses to `0.0` so the terminal viewport retains at least `20` columns, while `renderer.monitor_open()` remains `true`

#### R2.6: Sidebar `CardAtlases` remain at fixed design sizes while `PreeditLayer` and `MonitorPanel` invalidate bind groups via `Atlas::epoch`
- **Given**: A `Renderer` with sidebar cards, an active IME `PreeditLayer`, and an open `MonitorPanel`
- **When**: `renderer.set_font_size(18.0)` increments `Atlas::epoch()` and updates `cell_size`
- **Then**: Sidebar `CardAtlases` (`17px` title / `12.5px` meta) are not rebuilt, while `PreeditLayer` and `MonitorPanel` detect the new `ctx.atlas.epoch()` and `ctx.cell` on the next `prepare` pass and rebuild their text bind groups once

### R3: Pure keyboard chord classification and Korean IME pass-through (`crates/app/src/input.rs`, `crates/app/src/ime_chord.rs`) — D7, D11

#### R3.1: `font_action` maps `Cmd+=` and `Cmd++` (with or without `Shift`) to `FontAction::Increase`
- **Given**: `ModifiersState::SUPER` or `ModifiersState::SUPER | ModifiersState::SHIFT` (without `ALT` or `CONTROL`)
- **When**: `input::font_action(key, &mods)` is called with `Key::Character("=")` or `Key::Character("+")`
- **Then**: It returns `Some(FontAction::Increase)`

#### R3.2: `font_action` maps unshifted `Cmd+-` to `FontAction::Decrease` and unshifted `Cmd+0` to `FontAction::Reset`
- **Given**: `ModifiersState::SUPER` (with `!mods.shift_key()`, `!mods.alt_key()`, and `!mods.control_key()`)
- **When**: `input::font_action(key, &mods)` is called with `Key::Character("-")` or `Key::Character("0")`
- **Then**: It returns `Some(FontAction::Decrease)` for `"-"` and `Some(FontAction::Reset)` for `"0"`

#### R3.3: `font_action` rejects chords with `Ctrl`, `Alt`, missing `Super`, or shifted `-` / `0`
- **Given**: Key events with `CONTROL`, `ALT`, no `SUPER`, or `SUPER | SHIFT` combined with `"-"` / `"_"` / `"0"` / `")"`
- **When**: `input::font_action(key, &mods)` is called
- **Then**: It returns `None`

#### R3.4: Korean 2-Set IME chord normalization and mid-composition replay preserve `=`, `+`, `-`, and `0`
- **Given**: Korean 2-Set IME active (either idle or mid-Hangul composition) and `ModifiersState::SUPER` held
- **When**: `input::chord_key` or `ime_chord::replay_decision` processes `Key::Character("=" | "+" | "-" | "0")`
- **Then**: `input::chord_key` returns the character unchanged so `input::font_action` matches `Increase`, `Decrease`, or `Reset`

### R4: App-level font action execution, modal priority, and boundary no-op handling (`crates/app/src/terminal_app.rs`) — D1, D7, D8, D10, D11

#### R4.1: `Running::on_key` dispatches `font_action` after modal overlays and router, dismissing any open sidebar card menu
- **Given**: `Running` state with no modal overlay open and the sidebar card context menu open (`open_menu.is_some()`)
- **When**: `Running::on_key` receives `Cmd+=` (`FontAction::Increase`)
- **Then**: The sidebar card menu is closed via `close_card_menu()`, `do_font_action(FontAction::Increase)` executes, and the key is consumed without calling `encode_key`

#### R4.2: Modal `RenameOverlay` consumes key events before `font_action` is reached
- **Given**: `Running` state with the tab `RenameOverlay` open (priority 200 in `overlays`)
- **When**: The user presses `Cmd+=`, `Cmd+-`, or `Cmd+0`
- **Then**: `overlays.dispatch` consumes the key event and `do_font_action` is not called

#### R4.3: `do_font_action` resizes all tabs via `settings::refit` when `grid_size` changes, updates sidebar scroll/hover and IME cursor area, and schedules redraw
- **Given**: `Running` state at `14.0 pt` where `renderer.set_font_size(15.0)` returns `true` and changes `renderer.grid_size()`
- **When**: `do_font_action(FontAction::Increase)` runs
- **Then**: `settings::refit(&self.renderer, &mut self.tabs)` resizes every tab to the new `(cols, rows)`, sidebar scroll offset and hover state are re-evaluated against the new `cell_size`, `update_ime_cursor_area()` updates the IME candidate window rect if composing, `inner.redraw()` is requested, and `persist_font_size()` writes `[font].size_pt = 15.0`

#### R4.4: Boundary no-op chord is consumed without PTY leak, `settings::refit` selection clear, or config write
- **Given**: `Running` state already at `FONT_SIZE_MIN` (`8.0 pt`), `FONT_SIZE_MAX` (`32.0 pt`), or `FONT_SIZE_DEFAULT` (`14.0 pt`) with an active text selection on the terminal grid
- **When**: The user presses `Cmd+-` at `8.0 pt`, `Cmd+=` at `32.0 pt`, or `Cmd+0` at `14.0 pt`
- **Then**: The key event is consumed before `encode_key` (no bytes sent to PTY), `renderer.set_font_size` returns `false`, `settings::refit` is not called (active grid selection remains intact and no `SIGWINCH` is sent), and `persist_font_size` is not called

### R5: File-watcher echo suppression and external human-edit reconciliation for `[font].size_pt` (`crates/app`) — D9

#### R5.1: Self-written `[font].size_pt` reload is classified as `Echo` and suppressed
- **Given**: `persist_font_size()` succeeded in writing `size_pt = 15.0`, recording `last_written_font = Some(15.0)` and setting `last_applied_cfg.font.size_pt = 15.0`
- **When**: The config file watcher delivers a `Config` with `font.size_pt = 15.0` (and unchanged other fields)
- **Then**: `classify_delivery` matches `15.0` against `last_written_font` as `ConfigDelivery::Echo`, `suppress_delivery` returns `true` (`delivered == last_applied_cfg`), and `settings::apply` is skipped

#### R5.2: External human edit to `[font].size_pt` clears `font_size_override` and applies live across all tabs
- **Given**: `Running` state with `font_size_override = Some(15.0)` and `last_written_font != Some(20.0)` (including when `20.0` was written earlier in the session but is not the latest write)
- **When**: A user edits `config.toml` externally to `size_pt = 20.0` and the file watcher delivers the reloaded `Config`
- **Then**: `classify_delivery` returns `ConfigDelivery::HumanEdit`, `renderer.clear_font_override()` and `font_override_authoritative = false` are executed, and `settings::apply` rebuilds the atlas at `20.0 pt` and refits all tabs

#### R5.3: Failed `save_font_size` marks `font_override_authoritative` so unrelated config reloads retain the runtime font size
- **Given**: `save_font_size` returned an `Err` (e.g., read-only config file), leaving `font_override_authoritative = true` and `font_size_override = Some(16.0)` while `config.toml` on disk still has `size_pt = 14.0`
- **When**: An unrelated config reload (or watcher notification where `cfg.font.size_pt == 14.0`) arrives
- **Then**: Because `font_override_authoritative` is `true` and `cfg.font.size_pt == last_applied_cfg.font.size_pt` (`14.0`), `clear_font_override()` is not called and `renderer.font_size_logical()` remains `16.0`

## Tasks

### T1: Add font size constants, `Config::normalize` clamping, and surgical `save_font_size` in `crates/common` [infra]
- **Fulfills**: R1 (R1.1, R1.2, R1.3, R1.4)
- **Depends on**: (none)
- **Files**: `crates/common/src/config.rs`
- **Scope**:
  - Export `FONT_SIZE_MIN` (`8.0_f32`), `FONT_SIZE_MAX` (`32.0_f32`), `FONT_SIZE_DEFAULT` (`14.0_f32`), and `FONT_SIZE_STEP` (`1.0_f32`), and set `FontConfig::default().size_pt = FONT_SIZE_DEFAULT`.
  - In `Config::normalize(&mut self)`, clamp `self.font.size_pt` via `clamp_width("font.size_pt", self.font.size_pt, FONT_SIZE_MIN, FONT_SIZE_MAX, FONT_SIZE_DEFAULT)`.
  - Add `pub fn save_font_size(path: &Path, size_pt: f32) -> Result<()>` (returning `crate::error::Result<()>`) delegating to `save_table(path, "font", &[("size_pt", fmt_f32(size_pt))])`.
  - Add unit tests in `crates/common/src/config.rs` verifying R1.1–R1.4.

### T2: Add `Renderer` font size override, `set_font_size` atlas rebuild, and inset/epoch invariants in `crates/renderer` [infra]
- **Fulfills**: R2 (R2.1, R2.2, R2.3, R2.4, R2.5, R2.6)
- **Depends on**: T1
- **Files**: `crates/renderer/src/lib.rs`
- **Scope**:
  - Store `font_size_pt: f32` (initialized from `cfg.font.size_pt`) and `font_size_override: Option<f32>` (`None`) on `Renderer`.
  - Extract pure `clamp_font_size(size_pt: f32) -> f32` and `cell_size_for_pt(size_pt: f32, scale_factor: f64) -> (f32, f32)` helpers (`cell_size_px` delegates to `cell_size_for_pt`).
  - Implement `effective_font_size(&self) -> f32`, `pub fn font_size_logical(&self) -> f32`, `pub fn clear_font_override(&mut self)`, and `pub fn set_font_size(&mut self, size_pt: f32) -> bool` (clamps `size_pt`; returns `false` on boundary no-op without rebuilding `Atlas` or clearing `last_snapshot`; on change updates `font_size_override`, `cell_size`, `ppem`, rebuilds `Atlas` and `glyph_bind_group` through the single swap site `swap_atlas` shared with `apply_config`, clears `last_snapshot = None`, and calls `sync_left_inset()` which chains `sync_right_inset()`).
  - Update `Renderer::apply_config` to set `self.font_size_pt = cfg.font.size_pt` and derive `cell_size` and `ppem` from `self.effective_font_size()`.
  - Add tests in `crates/renderer/src/lib.rs` verifying R2.1–R2.6.

### T3: Add pure `FontAction` chord classification, Korean IME pass-through tests, and generic echo-suppression helpers in `crates/app` [infra]
- **Fulfills**: R3 (R3.1, R3.2, R3.3, R3.4)
- **Depends on**: T1  ← parallel with T2
- **Files**: `crates/app/src/input.rs`, `crates/app/src/ime_chord.rs`
- **Scope**:
  - Define `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FontAction { Increase, Decrease, Reset }` and `pub fn font_action(key: &Key, mods: &ModifiersState) -> Option<FontAction>` in `crates/app/src/input.rs`.
  - Generalize `classify_delivery<T: Copy + PartialEq>`, `remember_write<T: Copy>`, and `needs_persist<T: PartialEq>` in `crates/app/src/input.rs` so they operate on both `(f32, bool)` and scalar `f32`.
  - Add unit tests in `crates/app/src/input.rs` and `crates/app/src/ime_chord.rs` verifying R3.1–R3.4 and scalar `f32` echo suppression.

### T4: Wire font chords, `do_font_action` refit/redraw/persistence, and watcher echo suppression in `crates/app/src/terminal_app.rs` [vertical]
- **Fulfills**: R0 (R0.1), R4 (R4.1, R4.2, R4.3, R4.4), R5 (R5.1, R5.2, R5.3)
- **Depends on**: T2, T3
- **Files**: `crates/app/src/terminal_app.rs`
- **Scope**:
  - Add `last_written_font: Option<f32>` and `font_override_authoritative: bool` to `Running`.
  - Dispatch `input::font_action(&input.key, &input.mods)` in `Running::on_key` after `overlays.dispatch`, `router.dispatch`, and `is_sidebar_toggle_chord` (closing any open card menu via `self.close_card_menu()`, calling `self.do_font_action(action)`, and returning early, matching `do_tab_action`).
  - Implement `Running::do_font_action` (computing target from `self.renderer.font_size_logical()`, returning early if `!self.renderer.set_font_size(target)`, calling `settings::refit` when `input::needs_refit(before, after)` holds, re-clamping sidebar scroll and hover, updating IME cursor area, requesting `inner.redraw()`, and calling `self.persist_font_size()`).
  - Implement `Running::persist_font_size` using `common::config::save_font_size` + `last_written_font` + `font_override_authoritative`.
  - Reconcile `last_written_font` in `UserEvent::ConfigReloaded` (draining `inner.config_rx`) alongside `written_sidebar` and `written_monitor`.
  - Add tests in `crates/app/src/terminal_app.rs` verifying R0.1, R4.1–R4.4, and R5.1–R5.3.

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)

## Spec Review
- **Verdict**: PASS
- **Reviews**: 2
- **Fixed**:
  - [Grounding] R2.5 — replaced `renderer.monitor_open_intent()` with the actual method name `renderer.monitor_open()` (`crates/renderer/src/lib.rs:764`).
  - [Grounding] R4.1 — replaced `card_menu.is_some()` with the actual field name `open_menu.is_some()` (`crates/app/src/terminal_app.rs:227`).
  - [Grounding] R5.1 — replaced `ConfigDelivery::SelfWrite` with the actual variant `ConfigDelivery::Echo` (`crates/app/src/input.rs:314-319, 366-372`).
  - [Grounding] T1 — replaced `Result<(), ConfigError>` with `Result<()>` (`crate::error::Result<()>`, `crates/common/src/config.rs:13, 294`).
  - [Grounding] T4 — aligned `Running::on_key` and config-reload identifiers with `&input.key, &input.mods`, `self.close_card_menu()`, and `UserEvent::ConfigReloaded` (`crates/app/src/terminal_app.rs:2216, 2357`).
- **Open**:
  - (none)

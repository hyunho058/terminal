# Unattended Orchestration Report: mouse-button-reporting

## Summary
- **Status**: Completed (Green)
- **Branch**: `fix/jump-button`
- **Spec Path**: `specs/mouse-button-reporting/spec.md`
- **Pattern Selected**: Sequential Pipeline (T1 infra -> parallel TT1..TT3 -> T2 vertical)
- **Requirement Check**: 16 of 16 met (0 unmet)
- **Grounds Ratio**: test 15 / read 1
- **Commit**: on `fix/jump-button`, see `git log -1`

## Outcome Table
| GWT | verdict | grounds | code | evidence |
|-----|---------|---------|------|----------|
| R0.1 | met | read | ok | crates/app/src/terminal_app.rs:2863-2895, crates/app/src/terminal_app.rs:2918-2938, crates/app/src/input.rs:3442-3504 |
| R1.1 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_1_sgr_press_encoding passed |
| R1.2 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_2_sgr_release_encoding passed |
| R1.3 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_3_legacy_press_encoding passed |
| R1.4 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_4_legacy_release_encoding passed |
| R1.5 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_5_x10_omits_release passed |
| R1.6 | met | test | ok | crates/app/tests/tt1_mouse_button_encoding.rs::r1_6_legacy_coordinate_clamp passed |
| R2.1 | met | test | ok | crates/app/tests/tt2_mouse_button_modifiers.rs::r2_1_cmd_click_bypasses passed |
| R2.2 | met | test | ok | crates/app/tests/tt2_mouse_button_modifiers.rs::r2_2_shift_click_bypasses passed |
| R2.3 | met | test | ok | crates/app/tests/tt2_mouse_button_modifiers.rs::r2_3_tracking_off_bypasses passed |
| R2.4 | met | test | ok | crates/app/tests/tt2_mouse_button_modifiers.rs::r2_4_active_tracking_reports passed |
| R3.1 | met | test | ok | crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_1_press_resolves_to_report passed |
| R3.2 | met | test | ok | crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_2_release_resolves_to_report passed |
| R3.3 | met | test | ok | crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_3_selection_drag_suppressed passed |
| R3.4 | met | test | ok | crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_4_right_middle_button_press passed |
| R3.5 | met | test | ok | crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_5_release_omission_resolves_to_ignore passed |

## Decisions Assumed
- **D1**: Pure routing and encoding functions (`mouse_button_route`, `encode_mouse_button_report`, `mouse_button_bytes`, `grid_press_action`, `grid_release_action`) in `crates/app/src/input.rs`, re-exported in `crates/app/src/lib.rs`.
- **D2**: Dual wire formats: SGR `?1006` (`\x1b[<{btn};{col};{row}{M/m}`) with 1-based indexing and no coordinate ceiling, and Legacy X10 format with coordinate clamp (>223 dropped to prevent byte corruption).
- **D3**: Modifier bypass: ⌘-click (`mods.super_key()`) is reserved for terminal hyperlinks per `specs/hyperlinks/spec.md` D17/R4.11; Shift-click (`mods.shift_key()`) is reserved for manual text selection.
- **D4**: Latch state `mouse_reported: Option<(MouseButton, u16, u16)>` in `terminal_app.rs:Running` to ensure release events cleanly match press coordinates and avoid selection desync.
- **D5**: Support for Left (0), Middle (1), and Right (2) buttons on grid clicks; sidebar/overlays remain restricted to Left click only.

## Parked Tasks
- (none)

## Modified Files
- `crates/app/src/input.rs`
- `crates/app/src/lib.rs`
- `crates/app/src/terminal_app.rs`
- `crates/app/tests/tt1_mouse_button_encoding.rs`
- `crates/app/tests/tt2_mouse_button_modifiers.rs`
- `crates/app/tests/tt3_mouse_button_lifecycle.rs`

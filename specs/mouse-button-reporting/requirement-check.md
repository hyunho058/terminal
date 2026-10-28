# requirement-check — mouse-button-reporting
spec: sha256 b71dac8a0b5522abb228ef673fac640335f3e77fc2168162889e92f3d09aadbb
head: 1421f47e521b4f64467c765c3341f1f8900eb72a on fix/jump-button

task T1 done
task T2 done
phase3 complete

## round 0
| GWT | verdict | grounds | code | reason |
|-----|---------|---------|------|--------|
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
unmet: 0 of 16 in scope
dispatched this round: (none)
blocked: (none)

gate passed: passed
run ended at 1421f47e521b4f64467c765c3341f1f8900eb72a, commit due: passed





# gwt-tests — mouse-button-reporting
spec: sha256 b71dac8a0b5522abb228ef673fac640335f3e77fc2168162889e92f3d09aadbb

## R0 — (no TT)
- R0.1: manual: goal-level

## TT1 — R1 — L2: D1, D2
- run: `cargo test -p app --test tt1_mouse_button_encoding`
- R1.1: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_1_sgr_press_encoding
- R1.2: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_2_sgr_release_encoding
- R1.3: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_3_legacy_press_encoding
- R1.4: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_4_legacy_release_encoding
- R1.5: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_5_x10_omits_release
- R1.6: test: crates/app/tests/tt1_mouse_button_encoding.rs::r1_6_legacy_coordinate_clamp
- files:
  - crates/app/tests/tt1_mouse_button_encoding.rs  sha256: bdfab5bf927c090b42edce4b0d59298f35dba863916c56a78d87b33193468d36  blob: d380594209d23e118d68b3102ca4d24f2b8cebf8

## TT2 — R2 — L2: D3
- run: `cargo test -p app --test tt2_mouse_button_modifiers`
- R2.1: test: crates/app/tests/tt2_mouse_button_modifiers.rs::r2_1_cmd_click_bypasses
- R2.2: test: crates/app/tests/tt2_mouse_button_modifiers.rs::r2_2_shift_click_bypasses
- R2.3: test: crates/app/tests/tt2_mouse_button_modifiers.rs::r2_3_tracking_off_bypasses
- R2.4: test: crates/app/tests/tt2_mouse_button_modifiers.rs::r2_4_active_tracking_reports
- files:
  - crates/app/tests/tt2_mouse_button_modifiers.rs  sha256: 15994b9028ed6c9cbb020df2eb0e071a4950a9d015177bbdd04e928831128c05  blob: cfe1a3707e53336e1035f3bd34198e2dc48b6740

## TT3 — R3 — L2: D4, D5
- run: `cargo test -p app --test tt3_mouse_button_lifecycle`
- R3.1: test: crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_1_press_resolves_to_report
- R3.2: test: crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_2_release_resolves_to_report
- R3.3: test: crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_3_selection_drag_suppressed
- R3.4: test: crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_4_right_middle_button_press
- R3.5: test: crates/app/tests/tt3_mouse_button_lifecycle.rs::r3_5_release_omission_resolves_to_ignore
- files:
  - crates/app/tests/tt3_mouse_button_lifecycle.rs  sha256: dc8e3530701fc1de8d4da8c5e010eec4b8e52bc2e30794a84ac13baa4ffff56a  blob: 365fe4e02d1d31a523c53c4bf0dce4606b9ad2a1

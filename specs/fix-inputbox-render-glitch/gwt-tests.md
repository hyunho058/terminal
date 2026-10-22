# gwt-tests — fix-inputbox-render-glitch
spec: sha256 1db9e0f5fc0fa9dd64ce704d843e324f0dede3623a51d6c6a93d252f149e5843

## R0 — (no TT)
- R0.1: manual: goal-level
- R0.2: manual: goal-level
## TT1 — R1 — L2: D1, D5
- run: `cargo test -p renderer --test tt1_renderer_continuation_tests`
- R1.1: test: crates/renderer/tests/tt1_renderer_continuation_tests.rs::valid_wide_pair_skips_continuation_column_in_row_chars
- R1.2: test: crates/renderer/tests/tt1_renderer_continuation_tests.rs::orphan_continuation_cell_without_leader_is_treated_as_blank_space_in_row_chars
- R1.3: test: crates/renderer/tests/tt1_renderer_continuation_tests.rs::cell_span_returns_2_only_for_double_width_leader_followed_by_continuation_spacer
- files:
  - crates/renderer/tests/tt1_renderer_continuation_tests.rs  sha256: d6405ed05fdd89548eb5ccebef818ffa21972ff9c703c6f62846bd95a6ac24ae  blob: b28171f387ccd49ca883b78cc7d52929819edbd4

## TT2 — R2 — L2: D2
- run: `cargo test -p term-core --test tt2_clear_repair_tests`
- R2.1: test: crates/term-core/tests/tt2_clear_repair_tests.rs::r2_1_clear_line_from_cursor_on_continuation_spacer_blanks_orphan_leader
- R2.2: test: crates/term-core/tests/tt2_clear_repair_tests.rs::r2_2_clear_line_from_cursor_at_end_of_line_with_pending_wrap_blanks_last_col
- R2.3: test: crates/term-core/tests/tt2_clear_repair_tests.rs::r2_3_clear_from_cursor_repairs_bisected_wide_char_on_cursor_row
- R2.4: test: crates/term-core/tests/tt2_clear_repair_tests.rs::r2_4_clear_to_cursor_repairs_bisected_wide_char_on_cursor_row
- files:
  - crates/term-core/tests/tt2_clear_repair_tests.rs  sha256: 715262d926c367fbecb38d22cf14cc01c552037b3a2fee360c95acd4db0fb753  blob: aca79026d98fef0c0641d96d0ecf527e31e37df8

## TT3 — R3 — L2: D3
- run: `cargo test -p term-core --test tt3_pair_breaking_tests`
- R3.1: test: crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_narrow_over_spacer_blanks_leader
- R3.2: test: crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_narrow_over_leader_blanks_spacer
- R3.3: test: crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_wide_over_split_cleans_both_and_partners
- R3.4: test: crates/term-core/tests/tt3_pair_breaking_tests.rs::test_repair_wide_pairs_eliminates_orphan_spacers_and_leaders
- files:
  - crates/term-core/tests/tt3_pair_breaking_tests.rs  sha256: 5d96cf9b3e77e34978f455529dc13b32951eb6404297b59bbc68a9d77b07b8bb  blob: 25d66754c2659461305a4e94e11430bc6921debe

## TT4 — R4 — L2: D4, D6
- run: `cargo test -p term-core --test tt4_text_extraction_parity_tests`
- R4.1: test: crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_1_grid_snapshot_to_text_extracts_orphan_continuation_as_space
- R4.2: test: crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_2_grid_selection_text_extracts_orphan_continuation_as_space
- R4.3: test: crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_3_cell_memory_layout_unchanged
- files:
  - crates/term-core/tests/tt4_text_extraction_parity_tests.rs  sha256: fb32a1cfec88feccb182c5f748af32ddc741d79eeecd56fa458e8abd5ad78761  blob: 48ef247ac6f9e8ab05929b784543d469ae0bf764

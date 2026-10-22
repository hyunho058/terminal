# requirement-check — fix-inputbox-render-glitch
spec: sha256 1db9e0f5fc0fa9dd64ce704d843e324f0dede3623a51d6c6a93d252f149e5843
head: e112f038f38d034ded863b1eb92f9f5f6647a1fe on fix/inputbox-render-glitch
task T1 done
task T2 done
task T3 done
task T4 done
phase3 complete

## round 0
| GWT | verdict | grounds | code | reason |
|-----|---------|---------|------|--------|
| R0.1 | met | read | ok | crates/term-core/tests/vt_tui.rs:566 |
| R0.2 | met | read | ok | crates/term-core/tests/vt_tui.rs:608 |
| R1.1 | met | test | ok | crates/renderer/tests/tt1_renderer_continuation_tests.rs::valid_wide_pair_skips_continuation_column_in_row_chars passed |
| R1.2 | met | test | ok | crates/renderer/tests/tt1_renderer_continuation_tests.rs::orphan_continuation_cell_without_leader_is_treated_as_blank_space_in_row_chars passed |
| R1.3 | met | test | ok | crates/renderer/tests/tt1_renderer_continuation_tests.rs::cell_span_returns_2_only_for_double_width_leader_followed_by_continuation_spacer passed |
| R2.1 | met | test | ok | crates/term-core/tests/tt2_clear_repair_tests.rs::r2_1_clear_line_from_cursor_on_continuation_spacer_blanks_orphan_leader passed |
| R2.2 | met | test | ok | crates/term-core/tests/tt2_clear_repair_tests.rs::r2_2_clear_line_from_cursor_at_end_of_line_with_pending_wrap_blanks_last_col passed |
| R2.3 | met | test | ok | crates/term-core/tests/tt2_clear_repair_tests.rs::r2_3_clear_from_cursor_repairs_bisected_wide_char_on_cursor_row passed |
| R2.4 | met | test | ok | crates/term-core/tests/tt2_clear_repair_tests.rs::r2_4_clear_to_cursor_repairs_bisected_wide_char_on_cursor_row passed |
| R3.1 | met | test | ok | crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_narrow_over_spacer_blanks_leader passed |
| R3.2 | met | test | ok | crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_narrow_over_leader_blanks_spacer passed |
| R3.3 | met | test | ok | crates/term-core/tests/tt3_pair_breaking_tests.rs::test_break_wide_pair_wide_over_split_cleans_both_and_partners passed |
| R3.4 | met | test | ok | crates/term-core/tests/tt3_pair_breaking_tests.rs::test_repair_wide_pairs_eliminates_orphan_spacers_and_leaders passed |
| R4.1 | met | test | ok | crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_1_grid_snapshot_to_text_extracts_orphan_continuation_as_space passed |
| R4.2 | met | test | ok | crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_2_grid_selection_text_extracts_orphan_continuation_as_space passed |
| R4.3 | met | test | ok | crates/term-core/tests/tt4_text_extraction_parity_tests.rs::r4_3_cell_memory_layout_unchanged passed |
unmet: 0 of 16 in scope
dispatched this round: (none)
blocked: (none)
run ended at e112f038f38d034ded863b1eb92f9f5f6647a1fe, commit due: passed

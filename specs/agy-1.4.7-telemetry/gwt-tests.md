# gwt-tests — agy-1.4.7-telemetry
spec: sha256 9c63b1afd40ce7f70fa3c62ae5b16a90dcdb780e34cfba1b285b585d3d29e186

## R0 — (no TT)
- R0.1: manual: goal-level

## TT1 — R1 — L2: D1
- run: `cargo test -p ai-monitor --test tt1_agy_147_tail_subagent`
- R1.1: test: crates/ai-monitor/tests/tt1_agy_147_tail_subagent.rs::r1_1_singular_subagent_header
- R1.2: test: crates/ai-monitor/tests/tt1_agy_147_tail_subagent.rs::r1_2_plural_subagent_header
- files:
  - crates/ai-monitor/tests/tt1_agy_147_tail_subagent.rs  sha256: 9fd98f4d8753f7714e68c9332c988135ab1f469e4aa81b40c93cf08c8e544f3d  blob: 5f87c40c9c34456d50e0998f77fb8fb332e7b2dc

## TT2 — R2 — L2: D2, D3, D6, D8
- run: `cargo test -p ai-monitor --test tt2_agy_147_tool_registry`
- R2.1: test: crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs::r2_1_read_file_dispatched_and_paired_with_generic_result_as_file_op_read
- R2.2: test: crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs::r2_2_write_file_and_edit_file_dispatched_and_paired_with_generic_result_as_file_op_write
- R2.3: test: crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs::r2_3_ag_reported_type_rewrites_generic_with_subagent_creation_header_to_invoke_subagent
- R2.4: test: crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs::r2_4_ag_reported_type_rewrites_generic_with_command_exited_to_run_command
- R2.5: test: crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs::r2_5_legacy_tool_names_and_exit_strings_remain_supported
- files:
  - crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs  sha256: 232dc53ca1ad92d7e9a6a50787872d329b6a0ef5acd443e6262fa9eb74611d5f  blob: 969a4c514d8c176d4769e0b574f8c579d6beb602

## TT3 — R3 — L2: D4, D5, D9
- run: `cargo test -p ai-monitor --test tt3_agy_147_subagent_lifecycle`
- R3.1: test: crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs::r3_1_subagent_metadata_extraction_from_invoke_subagent_args
- R3.2: test: crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs::r3_2_subagent_in_flight_state_upon_launch_receipt
- R3.3: test: crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs::r3_3_subagent_completion_on_system_message
- R3.4: test: crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs::r3_4_multi_message_or_malformed_system_message_handling
- files:
  - crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs  sha256: 7992db090b2f29318dff98939dff4a52aa2e4e3a2941a88b7d5c253e8099c1bd  blob: 02b13b41d97310603d5379d2c83546c88b970ce3

## TT4 — R4 — L2: D7
- run: `cargo test -p ai-monitor --test tt4_agy_147_token_usage`
- R4.1: test: crates/ai-monitor/tests/tt4_agy_147_token_usage.rs::r4_1_cumulative_token_counting_from_planner_response
- R4.2: test: crates/ai-monitor/tests/tt4_agy_147_token_usage.rs::r4_2_context_occupancy_from_planner_response
- files:
  - crates/ai-monitor/tests/tt4_agy_147_token_usage.rs  sha256: babf3ca43fca7615ce6d4499d51bb41562b90159c57adece8385aab2f9a1c517  blob: 7d1c095e98d5ed56a3470310acfdbc7498853154

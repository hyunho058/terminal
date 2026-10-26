# Spec: agy-1.4.7-telemetry

## Meta
- **Created**: 2026-10-24
- **Type**: dev
- **Status**: approved
- **Mode**: unattended
- **Branch**: fix/agy-1.4.7-telemetry
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-10-24

## Goal
Antigravity CLI 1.4.7 runs live subagents and tools, but `crates/ai-monitor` displays:
- ACTIVE AGENTS: `(no agents yet)`
- FILE ACTIVITY: `(no file activity)`
- RECENT CALLS: `(no other tool calls)`
- Left sidebar card: `Waiting for input` (purple) instead of `Working` (blue)
- CACHE and CONTEXT: 0 / blank

Mismatches identified between Antigravity CLI 1.4.7 transcripts and `crates/ai-monitor`:
1. **Sidebar Card "Waiting" instead of "Working"**: In `tail.rs:281-320`, `subagent_starts` matches only `Created the following subagents:` (plural). CLI 1.4.7 writes singular `Created the following subagent:` when spawning a single subagent, causing `subagent_starts()` to return empty and the card to fall back to Waiting after 10s.
2. **ACTIVE AGENTS (no agents yet)**:
   - `invoke_subagent` expects result step type `INVOKE_SUBAGENT`, but CLI 1.4.7 emits `GENERIC`.
   - Subagent role/type is now passed directly in `invoke_subagent` args (`Role`, `TypeName`) rather than via a preceding `define_subagent` call.
   - `invoke_subagent` result step has `status: "DONE"` immediately upon spawn; actual completion arrives via a `SYSTEM_MESSAGE` from the subagent's `conversationId`.
3. **FILE ACTIVITY, RECENT CALLS, CACHE, CONTEXT blank**:
   - Tool names & step types: `read_file` (with `Path` arg) replaces `view_file` (with `AbsolutePath`); `write_file` and `edit_file` (with `Path` arg) replace `write_to_file` / `replace_file_content` (with `TargetFile`); result steps report as `GENERIC`.
   - `run_command` result body: CLI 1.4.7 writes `"Command exited with code"` instead of `"The command exited with code"`.
   - Token Usage: CLI 1.4.7 records `input_tokens`, `cache_read_tokens`, and `output_tokens` on `PLANNER_RESPONSE` steps.

## Non-goals
- Changes to non-Antigravity telemetry dialects (Claude Code, Codex, Gemini, Aider).
- Modifying UI renderers or layout code in `app` or `renderer` crates.
- Tailing child transcripts under `.gemini/antigravity-cli/brain/<id>/` directly.
- Breaking backwards compatibility with earlier Antigravity transcripts (plural subagent header, `view_file`, `write_to_file`, `replace_file_content`, `The command exited with code`).

## Confirmed Goal
Update `crates/ai-monitor` (`tail.rs` and `telemetry.rs`) so that Antigravity CLI 1.4.7 sessions correctly parse and reflect:
1. **Sidebar Card Working State**: `subagent_starts` in `tail.rs` recognizes both singular (`Created the following subagent:`) and plural (`Created the following subagents:`) headers, keeping the sidebar card in `Working` state while a subagent is active (inferred: backwards compatibility with older transcripts must be preserved).
2. **ACTIVE AGENTS Tracking**:
   - Pair `invoke_subagent` dispatches with `GENERIC` result steps.
   - Extract subagent type and description from `invoke_subagent` args (`TypeName`, `Role`) when present, falling back to preceding `define_subagent` definitions.
   - Treat `invoke_subagent` result step as spawning the subagent (setting in-flight state) and complete it when a `SYSTEM_MESSAGE` from `conversationId` is folded (inferred: mirrors the completion pattern observed in CLI 1.4.7).
3. **FILE ACTIVITY and RECENT CALLS Tracking**:
   - Register `read_file` as `FileOp::Read` using arg key `Path` expecting `GENERIC`.
   - Register `write_file` and `edit_file` as `FileOp::Write` using arg key `Path` expecting `GENERIC`.
   - Update `ag_reported_type` to recognize `run_command` results starting with `"Command exited with code"` as well as `"The command exited with code"`.
4. **Token & Cache Metrics**:
   - Fold token usage (`input_tokens`, `cache_read_tokens`, `output_tokens`) from Antigravity `PLANNER_RESPONSE` steps into `TelemetrySnapshot` token counters and context reading (inferred: update cumulative counters accurately across turns).
Done when:
All existing tests in `crates/ai-monitor` pass and new unit/integration tests verify CLI 1.4.7 transcript parsing (subagent lifecycle, file activity, command results, and token accounting) for both legacy and 1.4.7 formats.

## Research
- `crates/ai-monitor/src/antigravity/tail.rs:281`: `const SUBAGENT_HEADER: &str = "Created the following subagents:";` — hardcoded plural prefix in `subagent_starts` causes failure to recognize singular `Created the following subagent:` in Antigravity CLI 1.4.7.
- `crates/ai-monitor/src/antigravity/tail.rs:314-330`: `subagent_starts` strips `SUBAGENT_HEADER`, then parses `conversationId` UUIDs. `completed_ids` (`tail.rs:338-349`) already parses `[Message] timestamp=<ts> sender=<id>` to complete subagents in `tail.rs`.
- `crates/ai-monitor/src/telemetry.rs:149-186`: `ag_tool` definition table maps tool names to `ToolKind`, expected result step type, and path arg key. Currently maps `view_file` to `"VIEW_FILE"` (`AbsolutePath`), `write_to_file`/`replace_file_content` to `"CODE_ACTION"` (`TargetFile`), and `invoke_subagent` to `"INVOKE_SUBAGENT"`. Lacks `read_file`, `write_file`, and `edit_file`.
- `crates/ai-monitor/src/telemetry.rs:887-890`: `ag_subagent_def` is only populated when `name == "define_subagent"`. Antigravity 1.4.7 passes `TypeName` and `Role` directly on `invoke_subagent` args.
- `crates/ai-monitor/src/telemetry.rs:906-930`: `ag_result` matches pending calls against `step_type` rewritten by `ag_reported_type`.
- `crates/ai-monitor/src/telemetry.rs:996-1039`: `ToolKind::Agent` handling immediately marks `completed = true` if `status == "DONE"`. It has no handler for subagent in-flight lifecycle or `SYSTEM_MESSAGE` completions.
- `crates/ai-monitor/src/telemetry.rs:1853-1860`: `ag_reported_type` only checks `content.contains("The command exited with code")` to rewrite `GENERIC` to `RUN_COMMAND`. In CLI 1.4.7, the phrase is `"Command exited with code"`.
- `crates/ai-monitor/src/telemetry.rs:837-853`: `PLANNER_RESPONSE` handling only increments `turns_assistant` or dispatches tools, ignoring `input_tokens`, `cache_read_tokens`, and `output_tokens` fields present on 1.4.7 `PLANNER_RESPONSE` steps.
- `crates/ai-monitor/src/telemetry.rs:1211-1235`: Token folding updates `total_input`, `total_output`, `total_cache_read`, `total_cache_creation`, `context_used`, and `context_high_water`. Antigravity tokens can feed directly into these cumulative counters and gauge.
- `crates/app/src/dashboard.rs:720-766`: `ACTIVE AGENTS` dashboard section displays `t.subagents` from `TelemetrySnapshot`. An empty or erroneously completed subagent list causes `(no agents yet)` or 0s elapsed time.
- `CLAUDE.md`: Build/test commands: `cargo test -p ai-monitor`.

## Decisions

### D1: Recognize Singular and Plural Subagent Headers in `tail.rs`
- **Status**: resolved
- **Rationale**: In `tail.rs:subagent_starts`, match both `"Created the following subagent:"` and `"Created the following subagents:"` by stripping the common prefix `"Created the following subagent"` and checking that the remainder is either `":"` or `"s:"`. This ensures sessions with a single spawned subagent in CLI 1.4.7 immediately register the pending subagent id and prevent the sidebar card from dropping to Waiting.
- **Rejected alternative**: Regex-based header matching, rejected due to regex compilation overhead and unnecessary memory allocations in the hot tail-reading path.

### D2: Register `read_file`, `write_file`, and `edit_file` in `ag_tool` Expecting `GENERIC`
- **Status**: resolved
- **Rationale**: Map `read_file` to `ToolKind::File(FileOp::Read)` with path arg `"Path"` expecting `"GENERIC"`. Map `write_file` and `edit_file` to `ToolKind::File(FileOp::Write)` with path arg `"Path"` expecting `"GENERIC"`. Retain legacy `view_file` (expecting `"VIEW_FILE"`, `"AbsolutePath"`) and `write_to_file`/`replace_file_content` (expecting `"CODE_ACTION"`, `"TargetFile"`) for backward compatibility.
- **Rejected alternative**: Rewriting file tool results to fake step types (`VIEW_FILE`, `CODE_ACTION`) in `ag_reported_type`, rejected because result step bodies for files contain arbitrary user text that cannot be reliably pattern-matched. Expecting `"GENERIC"` fits standard FIFO pairing.

### D3: Rewrite `GENERIC` to `INVOKE_SUBAGENT` in `ag_reported_type` on Subagent Launch Receipt
- **Status**: resolved
- **Rationale**: When a result step has `step_type == "GENERIC"` and its `content` starts with `"Created the following subagent:"` or `"Created the following subagents:"` (after any timestamp metadata), `ag_reported_type` rewrites its type to `"INVOKE_SUBAGENT"`. This allows `ag_tool("invoke_subagent")` to continue expecting `"INVOKE_SUBAGENT"` and avoids collision with other `GENERIC` results.
- **Rejected alternative**: Changing `invoke_subagent` to expect `"GENERIC"` directly in `ag_tool`, rejected because `invoke_subagent` would then compete in FIFO order with file operations and `manage_task` if multiple calls are dispatched simultaneously.

### D4: Extract Subagent `TypeName` and `Role` from `invoke_subagent` Arguments
- **Status**: resolved
- **Rationale**: In `ag_dispatch`, when `name == "invoke_subagent"`, extract `TypeName` as `subagent_type` and `Role` as `description`. Store them on `AgCall`. In `ag_result` for `ToolKind::Agent`, use the call's extracted metadata, falling back to `self.ag_subagent_def` only if `TypeName` or `Role` is absent.
- **Rejected alternative**: Overwriting `self.ag_subagent_def` globally on each invocation, rejected because it can overwrite previously defined custom subagent definitions if multiple dispatches overlap.

### D5: Model Subagent Lifecycle as Asynchronous (In-Flight Launch -> `SYSTEM_MESSAGE` Completion)
- **Status**: resolved
- **Rationale**: In `ag_result` for `ToolKind::Agent`, when the receipt contains a subagent creation header (`Created the following subagent` singular or plural), the subagent is initialized in `self.subagents` with `completed: false` and `end_ms: None`. In `apply_antigravity`, when `step_type == "SYSTEM_MESSAGE"`, parse sender ids from lines beginning with `[Message] timestamp=` having `sender=<conversationId>`, and mark matching subagents in `self.subagents` as `completed: true` with `end_ms = ts`.
- **Steelman counterargument & mitigation**:
  - *Counterargument*: Decoupling completion to out-of-band `SYSTEM_MESSAGE` creates potential leaks if a subagent crashes without emitting a message, trapping it in `completed: false`. Furthermore, older test `tests/telemetry.rs:3072` (`r2_5`) asserted `assert!(a.completed, "status DONE")` on the launch step itself.
  - *Mitigation*:
    1. *Memory & display bounding*: Unfinished subagents are strictly bounded by `SUBAGENTS_CAP` (32 entries in `VecDeque`). An in-flight subagent computes elapsed time from `start_ms` up to `now` (or the last event timestamp), exactly matching Claude Code's background agent behavior (`telemetry.rs:1543-1560`).
    2. *Terminal signal rule*: Any `SYSTEM_MESSAGE` carrying `sender=<conversationId>` marks the subagent completed (measured in `tail.rs:334`: 57 of 57 messages were terminal). If multiple messages arrive from the same sender, the latest timestamp updates `end_ms` (last-wins for timestamps).
    3. *Test contract update*: The assertions in `crates/ai-monitor/tests/telemetry.rs:3072` (`r2_5`) and `crates/ai-monitor/tests/telemetry.rs:2885` (`antigravity_fixture_folds_to_expected_telemetry`)—which assumed `status == "DONE"` on the launch receipt indicated completion—were based on the faulty assumption documented in former comment AG5 ("an Antigravity invocation completes from its own step's status and is never notified out of band"). With the empirical evidence that 1.4.7 reports completion via `SYSTEM_MESSAGE`, both tests are updated to assert `!a.completed` and `a.end_ms.is_none()` upon launch receipt, with `r2_5` asserting `a.completed` after a subsequent `SYSTEM_MESSAGE` arrives, aligning with `crates/ai-monitor/tests/tt1_agy_pending_background.rs`.
- **Rejected alternative**: Treating `status == "DONE"` on the launch receipt as completed, rejected because it produces instantaneous (0s) subagents and blanks out `ACTIVE AGENTS` in `ai-monitor` while subagents are actively working.

### D6: Support Both Command Output Markers in `ag_reported_type`
- **Status**: resolved
- **Rationale**: Update `ag_reported_type` to rewrite `GENERIC` to `"RUN_COMMAND"` when content contains either `"Command exited with code"` (CLI 1.4.7) or `"The command exited with code"` (legacy corpus).
- **Rejected alternative**: Matching only `"Command exited with code"`, rejected because legacy fixtures would fail to pair command results.

### D7: Fold Token Usage and Context Metrics from `PLANNER_RESPONSE`
- **Status**: resolved
- **Rationale**: On `PLANNER_RESPONSE` steps in `apply_antigravity`, extract `input_tokens`, `cache_read_tokens`, and `output_tokens` as `u64`. When present, add to `self.total_input`, `self.total_output`, and `self.total_cache_read`. Compute `context_used = input_tokens.saturating_add(cache_read_tokens)` and update `self.context_high_water = self.context_high_water.max(context_used)`.
- **Rejected alternative**: Storing per-turn token snapshots in a separate table, rejected because `TelemetrySnapshot` already exposes aggregate cost counters and a latest-value context occupancy gauge.

### D8: Bounded FIFO Pairing for `GENERIC` Dispatches
- **Status**: resolved
- **Rationale**: Dispatched file tools expecting `"GENERIC"` are paired with `"GENERIC"` result steps using the existing `ag_pending.iter().position(|c| c.expects == Some(step_type))` FIFO queue. Because tool calls within a `PLANNER_RESPONSE` turn are executed and reported sequentially, FIFO ordering correctly pairs dispatches with their respective outcomes.
- **Rejected alternative**: Unordered pairing or heuristics based on path matching in result bodies, rejected due to risk of false positive matches on file contents.

### D9: Robust Handling of Malformed or Multi-Message `SYSTEM_MESSAGE` Content
- **Status**: resolved
- **Rationale**: Parsing of `SYSTEM_MESSAGE` content checks lines for `[Message] timestamp=` and extracts the `sender=<uuid>` token. If the sender is not a known subagent id in `self.subagents`, the message is ignored without creating orphan rows or error increments (identical to `on_task_notification` in `telemetry.rs:1536`). If a message lacks a timestamp or has unparseable sender text, no fields are modified. If multiple `SYSTEM_MESSAGE` lines arrive from the same sender, the first completes it and subsequent ones update `end_ms` if their timestamp is greater (monotone completion).
- **Rejected alternative**: Panicking or creating synthetic un-named subagents for unknown senders, rejected because system messages include unrelated task notices and timer firings.

## Constraints
- **Zero panics**: All token math must use saturating arithmetic (`saturating_add`); all JSON and text parsing must be `Option`-guarded.
- **Zero UI changes**: Only `crates/ai-monitor` (`tail.rs`, `telemetry.rs`, and test files) is modified.
- **Backwards compatibility**: All existing tests in `crates/ai-monitor` and `crates/app` must continue to pass.

## Known Gaps
- (none)

## Requirements

### R0: Antigravity CLI 1.4.7 Live Telemetry and Tail Synchronization

#### R0.1: Full session playback reflects working status, active subagents, file activities, command results, and token metrics end-to-end (manual)
- **Given**: A real Antigravity CLI 1.4.7 transcript with running subagent, file edits, command execution, and token fields
- **When**: The terminal dashboard processes the transcript through `crates/ai-monitor`
- **Then**: The sidebar card displays `Working` while subagents are running, `ACTIVE AGENTS` lists the subagent with live elapsed time, `FILE ACTIVITY` lists reads and edits, `RECENT CALLS` contains command executions, and `CACHE`/`CONTEXT` gauges show positive token counts

### R1: Subagent Start Recognition in Tail Scanning — D1

#### R1.1: Singular subagent creation header in `tail_state`
- **Given**: A transcript whose final turn includes a subagent launch with `Created the following subagent:\n{\n  "conversationId": "<uuid>"\n}` and no completion message
- **When**: Evaluated via `ai_monitor::antigravity::tail_state` (or unit test on `subagent_starts` in `tail.rs`)
- **Then**: Returns `Some(TailState { last: Some(LastMessage::AssistantBusy), .. })`

#### R1.2: Plural subagent creation header in `tail_state`
- **Given**: A transcript whose final turn includes a subagent launch with `Created the following subagents:\n{\n  "conversationId": "<uuid>"\n}` and no completion message
- **When**: Evaluated via `ai_monitor::antigravity::tail_state` (or unit test on `subagent_starts` in `tail.rs`)
- **Then**: Returns `Some(TailState { last: Some(LastMessage::AssistantBusy), .. })`

### R2: Tool Registry and Result Step Typing — D2, D3, D6, D8

#### R2.1: `read_file` dispatched and paired with `GENERIC` result as `FileOp::Read`
- **Given**: An initialized `SessionTelemetry` with Antigravity format
- **When**: A `PLANNER_RESPONSE` dispatching `read_file` with arg `Path: "/foo/bar.rs"` followed by a `GENERIC` result step is folded via `SessionTelemetry::apply`
- **Then**: `snapshot().files` contains a `FileActivity` entry with `op: FileOp::Read`, `path: "/foo/bar.rs"`, and `total_tool_calls` increments by 1

#### R2.2: `write_file` and `edit_file` dispatched and paired with `GENERIC` result as `FileOp::Write`
- **Given**: An initialized `SessionTelemetry` with Antigravity format
- **When**: A `PLANNER_RESPONSE` dispatching `write_file` and `edit_file` with arg `Path: "/foo/baz.rs"` followed by `GENERIC` result steps is folded via `SessionTelemetry::apply`
- **Then**: `snapshot().files` contains `FileActivity` entries with `op: FileOp::Write`, `path: "/foo/baz.rs"`, and `total_tool_calls` increments accordingly

#### R2.3: `ag_reported_type` rewrites `GENERIC` with subagent creation header to `INVOKE_SUBAGENT`
- **Given**: A `PLANNER_RESPONSE` dispatching `invoke_subagent` followed by a result step of type `"GENERIC"` and content beginning with `"Created the following subagent:"` or `"Created the following subagents:"`
- **When**: Folded via public `SessionTelemetry::apply` (or unit test on `ag_reported_type`)
- **Then**: The subagent dispatch pairs with the result step without becoming an orphan, populating `snapshot().subagents`

#### R2.4: `ag_reported_type` rewrites `GENERIC` with `"Command exited with code"` to `RUN_COMMAND`
- **Given**: A `PLANNER_RESPONSE` dispatching `run_command` followed by a result step of type `"GENERIC"` and content containing `"Command exited with code 0."`
- **When**: Folded via public `SessionTelemetry::apply` (or unit test on `ag_reported_type`)
- **Then**: The command dispatch pairs with the result step, recording 1 tool in `snapshot().tools` and 0 orphan tool results

#### R2.5: Legacy tool names and exit strings remain supported
- **Given**: Steps dispatching legacy `view_file` (`AbsolutePath`), `write_to_file` (`TargetFile`), and result containing `"The command exited with code 0."`
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: Legacy tools are correctly paired into `snapshot().files` and `snapshot().tools` with zero orphan tool results

### R3: Subagent Metadata and Asynchronous Lifecycle — D4, D5, D9

#### R3.1: Subagent metadata extraction from `invoke_subagent` args
- **Given**: A `PLANNER_RESPONSE` dispatching `invoke_subagent` with `TypeName: "research"` and `Role: "Spec Reviewer"`
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: The resulting `Subagent` in `snapshot().subagents` has `subagent_type: Some("research")` and `description: Some("Spec Reviewer")`

#### R3.2: Subagent in-flight state upon launch receipt
- **Given**: An `invoke_subagent` dispatch followed by a result step containing `"Created the following subagent:"` with conversation id `CID`
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: `snapshot().subagents` contains an entry with `id: CID`, `completed: false`, and `end_ms: None`

#### R3.3: Subagent completion on `SYSTEM_MESSAGE`
- **Given**: A subagent in `snapshot().subagents` with `id: CID` and `completed: false`
- **When**: A `SYSTEM_MESSAGE` step with content `[Message] timestamp=2026-10-24T18:37:17Z sender=CID priority=HIGH` is folded via `SessionTelemetry::apply`
- **Then**: The subagent with `id: CID` in `snapshot().subagents` transitions to `completed: true` with `end_ms` set to the timestamp in milliseconds

#### R3.4: Multi-message or malformed `SYSTEM_MESSAGE` handling
- **Given**: A `SYSTEM_MESSAGE` step with an unknown sender id or malformed header
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: No panic occurs, existing subagent entries remain unmodified, and no orphan errors are recorded

### R4: Token Usage and Context Occupancy Accounting — D7

#### R4.1: Cumulative token counting from `PLANNER_RESPONSE`
- **Given**: A `PLANNER_RESPONSE` step with `input_tokens: 3415`, `output_tokens: 29`, `cache_read_tokens: 97161`
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: `snapshot().total_input` increases by 3415, `snapshot().total_output` increases by 29, and `snapshot().total_cache_read` increases by 97161

#### R4.2: Context occupancy from `PLANNER_RESPONSE`
- **Given**: A `PLANNER_RESPONSE` step with `input_tokens: 3415`, `cache_read_tokens: 97161`
- **When**: Folded via `SessionTelemetry::apply`
- **Then**: `snapshot().context_used` equals 100576 (3415 + 97161) and `snapshot().context_high_water` is at least 100576

## Tasks

### T1: Test fixtures and transcript helpers for Antigravity 1.4.7 [infra]
- **Fulfills**: R0
- **Depends on**: (none)
- Sets up helper fixtures and builder functions in `crates/ai-monitor/tests/` representing Antigravity 1.4.7 transcript steps (singular subagent launch receipt, `read_file`/`write_file`/`edit_file` tools, `"Command exited with code 0."`, `SYSTEM_MESSAGE` completions, and token usage blocks).

### TT1: Acceptance tests for R1's GWTs [tests]
- **Fulfills**: R1 (tests)
- **Depends on**: T1
- Implements acceptance test file `crates/ai-monitor/tests/tt1_agy_147_tail_subagent.rs` testing `ai_monitor::antigravity::tail_state` against transcripts ending in pending subagents with both singular and plural headers (and direct unit tests in `tail.rs::tests`).

### TT2: Acceptance tests for R2's GWTs [tests]
- **Fulfills**: R2 (tests)
- **Depends on**: T1
- Implements acceptance test file `crates/ai-monitor/tests/tt2_agy_147_tool_registry.rs` testing `read_file`, `write_file`, `edit_file`, command exit status strings, and step rewriting via `SessionTelemetry::apply`.

### TT3: Acceptance tests for R3's GWTs [tests]
- **Fulfills**: R3 (tests)
- **Depends on**: T1
- Implements acceptance test file `crates/ai-monitor/tests/tt3_agy_147_subagent_lifecycle.rs` testing `TypeName`/`Role` arg extraction, in-flight spawn state, and `SYSTEM_MESSAGE` completion.

### TT4: Acceptance tests for R4's GWTs [tests]
- **Fulfills**: R4 (tests)
- **Depends on**: T1
- Implements acceptance test file `crates/ai-monitor/tests/tt4_agy_147_token_usage.rs` testing `PLANNER_RESPONSE` token folding and context occupancy calculations.

### T2: Support singular & plural subagent headers in tail scanner [vertical]
- **Fulfills**: R1
- **Depends on**: T1, TT1
- Updates `crates/ai-monitor/src/antigravity/tail.rs` `subagent_starts` to accept both `"Created the following subagent:"` and `"Created the following subagents:"`. Parallel with T3.

### T3: Register 1.4.7 file tools and command exit phrase rewriting in telemetry [vertical]
- **Fulfills**: R2
- **Depends on**: T1, TT2
- Updates `crates/ai-monitor/src/telemetry.rs`:
  - Adds `read_file` (`Path`), `write_file` (`Path`), and `edit_file` (`Path`) to `ag_tool` expecting `"GENERIC"`.
  - Updates `ag_reported_type` to rewrite `"GENERIC"` to `"RUN_COMMAND"` for `"Command exited with code"`.
  - Updates `ag_reported_type` to rewrite `"GENERIC"` to `"INVOKE_SUBAGENT"` on subagent creation headers.
  Parallel with T2.

### T4: Implement subagent direct arg extraction and asynchronous SYSTEM_MESSAGE lifecycle [vertical]
- **Fulfills**: R3
- **Depends on**: T3, TT3
- Updates `crates/ai-monitor/src/telemetry.rs`:
  - Extracts `TypeName` and `Role` in `ag_dispatch` when `name == "invoke_subagent"`.
  - Marks subagent `completed: false` and `end_ms: None` upon spawn receipt in `ag_result`.
  - Handles `SYSTEM_MESSAGE` in `apply_antigravity` by parsing `sender=<uuid>` and completing matching in-flight subagents.

### T5: Implement token usage and context occupancy folding from PLANNER_RESPONSE [vertical]
- **Fulfills**: R4
- **Depends on**: T4, TT4
- Updates `crates/ai-monitor/src/telemetry.rs` `apply_antigravity` for `"PLANNER_RESPONSE"`:
  - Extracts `input_tokens`, `cache_read_tokens`, and `output_tokens`.
  - Updates `total_input`, `total_output`, `total_cache_read`.
  - Updates `context_used` and `context_high_water`.

### T6: End-to-end regression verification and test assertion updates [vertical]
- **Fulfills**: R0
- **Depends on**: T2, T5
- Updates `crates/ai-monitor/tests/telemetry.rs:3072` (`r2_5`) and `crates/ai-monitor/tests/telemetry.rs:2885` (`antigravity_fixture_folds_to_expected_telemetry`) to reflect the async subagent lifecycle (testing in-flight followed by `SYSTEM_MESSAGE` completion). Runs full `cargo test -p ai-monitor` test suite to verify no regressions.

## External Dependencies

### Pre-work
- (none)

### Post-work
- (none)

## Spec Review
- **Verdict**: PASS
- **Reviews**: 3
- **Fixed**:
  - [Grounding] T6, D5 — updated `tests/telemetry.rs:3072` path to `crates/ai-monitor/tests/telemetry.rs:3072`
  - [Readiness] TT1, R1.1, R1.2 — specified testing tail state via public `ai_monitor::antigravity::tail_state` and field `last: Some(LastMessage::AssistantBusy)`
  - [Readiness] TT2, R2.3, R2.4 — specified testing step type rewriting via public `SessionTelemetry::apply`
  - [Grounding] T4, T5, D5, D7 — corrected handler function name from `apply_step` to `apply_antigravity`
  - [Consistency] T6, D5 — updated `crates/ai-monitor/tests/telemetry.rs:2885` (`antigravity_fixture_folds_to_expected_telemetry`) to expect `!a.completed` and `a.end_ms.is_none()`
- **Open**:
  - (none)






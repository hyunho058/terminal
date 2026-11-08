//! Acceptance tests for R2: Tool Registry and Result Step Typing (D2, D3, D6, D8).
//!
//! Verifies that Antigravity CLI 1.4.7 tool dispatches (`read_file`, `write_file`,
//! `edit_file`, `invoke_subagent`, and `run_command`) correctly pair with `GENERIC`
//! result steps, and that legacy tool mappings remain backwards-compatible.

use ai_monitor::telemetry::{FileOp, LogFormat, SessionTelemetry, TelemetrySnapshot};

fn ag_step(i: u64, ty: &str, extra: &str) -> String {
    format!(
        r#"{{"step_index":{i},"source":"MODEL","type":"{ty}","status":"DONE","created_at":"2026-10-24T18:{:02}:00Z"{extra}}}"#,
        i
    )
}

fn ag_dispatch(i: u64, name: &str, args: &str) -> String {
    ag_step(
        i,
        "PLANNER_RESPONSE",
        &format!(r#","tool_calls":[{{"name":"{name}","args":{{{args}}}}}]"#),
    )
}

fn ag_fold(lines: &[String]) -> TelemetrySnapshot {
    let mut t = SessionTelemetry::with_format(LogFormat::Antigravity);
    for l in lines {
        t.apply(l);
    }
    t.snapshot()
}

#[test]
fn r2_1_read_file_dispatched_and_paired_with_generic_result_as_file_op_read() {
    let s = ag_fold(&[
        ag_dispatch(1, "read_file", r#""Path":"\"/foo/bar.rs\"""#),
        ag_step(2, "GENERIC", r#","content":"file contents here""#),
    ]);

    assert_eq!(s.total_tool_calls, 1, "total_tool_calls increments by 1");
    assert_eq!(
        s.orphan_tool_results, 0,
        "paired without becoming an orphan"
    );
    assert!(
        s.files
            .iter()
            .any(|f| f.op == FileOp::Read && f.path == "/foo/bar.rs"),
        "snapshot().files contains FileActivity entry with op: FileOp::Read, path: /foo/bar.rs"
    );
}

#[test]
fn r2_2_write_file_and_edit_file_dispatched_and_paired_with_generic_result_as_file_op_write() {
    let s = ag_fold(&[
        ag_dispatch(1, "write_file", r#""Path":"\"/foo/baz.rs\"""#),
        ag_step(2, "GENERIC", r#","content":"write output""#),
        ag_dispatch(3, "edit_file", r#""Path":"\"/foo/baz.rs\"""#),
        ag_step(4, "GENERIC", r#","content":"edit output""#),
    ]);

    assert_eq!(
        s.total_tool_calls, 2,
        "total_tool_calls increments accordingly"
    );
    assert_eq!(
        s.orphan_tool_results, 0,
        "paired without becoming an orphan"
    );
    let writes: Vec<_> = s
        .files
        .iter()
        .filter(|f| f.op == FileOp::Write && f.path == "/foo/baz.rs")
        .collect();
    assert_eq!(
        writes.len(),
        2,
        "snapshot().files contains FileActivity entries with op: FileOp::Write, path: /foo/baz.rs"
    );
}

#[test]
fn r2_3_ag_reported_type_rewrites_generic_with_subagent_creation_header_to_invoke_subagent() {
    // Both singular (CLI 1.4.7) and plural (legacy) headers should rewrite GENERIC to INVOKE_SUBAGENT
    let s = ag_fold(&[
        ag_dispatch(
            1,
            "invoke_subagent",
            r#""TypeName":"\"research\"","Role":"\"Reviewer\"""#,
        ),
        ag_step(
            2,
            "GENERIC",
            r#","content":"Created At: 2026-10-24T18:00:00Z\nCompleted At: 2026-10-24T18:00:01Z\nCreated the following subagent:\n{\n  \"conversationId\": \"ed14342c-0000-4000-8000-00000000beef\"\n}""#,
        ),
        ag_dispatch(
            3,
            "invoke_subagent",
            r#""TypeName":"\"coder\"","Role":"\"Implementer\"""#,
        ),
        ag_step(
            4,
            "GENERIC",
            r#","content":"Created At: 2026-10-24T18:00:02Z\nCompleted At: 2026-10-24T18:00:03Z\nCreated the following subagents:\n{\n  \"conversationId\": \"7640f000-2222-4333-8444-b55555555555\"\n}""#,
        ),
    ]);

    assert_eq!(
        s.orphan_tool_results, 0,
        "subagent dispatches pair with result steps without becoming orphans"
    );
    assert_eq!(
        s.subagents.len(),
        2,
        "populates snapshot().subagents for both singular and plural receipts"
    );
    assert!(s
        .subagents
        .iter()
        .any(|a| a.id == "ed14342c-0000-4000-8000-00000000beef"));
    assert!(s
        .subagents
        .iter()
        .any(|a| a.id == "7640f000-2222-4333-8444-b55555555555"));
}

#[test]
fn r2_4_ag_reported_type_rewrites_generic_with_command_exited_to_run_command() {
    let s = ag_fold(&[
        ag_dispatch(
            1,
            "run_command",
            r#""CommandLine":"\"cargo test\"","toolSummary":"\"Run test suite\"""#,
        ),
        ag_step(
            2,
            "GENERIC",
            r#","content":"Created At: 2026-10-24T11:26:44-07:00\nCompleted At: 2026-10-24T11:26:44-07:00\nCommand exited with code 0.\n\nCommand output:\ntest result: ok.\n""#,
        ),
    ]);

    assert_eq!(s.orphan_tool_results, 0, "0 orphan tool results");
    assert_eq!(s.tools.len(), 1, "recording 1 tool in snapshot().tools");
    assert_eq!(s.tools[0].name, "Run test suite");
}

#[test]
fn r2_5_legacy_tool_names_and_exit_strings_remain_supported() {
    let s = ag_fold(&[
        // legacy view_file expecting VIEW_FILE and AbsolutePath
        ag_dispatch(1, "view_file", r#""AbsolutePath":"\"/legacy/view.rs\"""#),
        ag_step(2, "VIEW_FILE", r#","content":"legacy view contents""#),
        // legacy write_to_file expecting CODE_ACTION and TargetFile
        ag_dispatch(3, "write_to_file", r#""TargetFile":"\"/legacy/write.rs\"""#),
        ag_step(4, "CODE_ACTION", r#","content":"legacy write contents""#),
        // legacy exit string "The command exited with code 0."
        ag_dispatch(
            5,
            "run_command",
            r#""CommandLine":"\"cargo check\"","toolSummary":"\"Check build\"""#,
        ),
        ag_step(
            6,
            "GENERIC",
            r#","content":"The command exited with code 0.""#,
        ),
    ]);

    assert_eq!(s.orphan_tool_results, 0, "zero orphan tool results");
    assert!(
        s.files
            .iter()
            .any(|f| f.op == FileOp::Read && f.path == "/legacy/view.rs"),
        "legacy view_file correctly paired into snapshot().files"
    );
    assert!(
        s.files
            .iter()
            .any(|f| f.op == FileOp::Write && f.path == "/legacy/write.rs"),
        "legacy write_to_file correctly paired into snapshot().files"
    );
    assert_eq!(
        s.tools.len(),
        1,
        "legacy command correctly paired into snapshot().tools"
    );
    assert_eq!(s.tools[0].name, "Check build");
}

/// A `RUNNING` step announcing a background task, as both `run_command` and `schedule`
/// report one under 1.4.7 (session `29b623ae`, steps 52 and 1071).
fn ag_background_start(i: u64, description: &str) -> String {
    format!(
        r#"{{"step_index":{i},"source":"MODEL","type":"GENERIC","status":"RUNNING","created_at":"2026-10-24T18:{i:02}:00Z","content":"Created At: 2026-10-25T20:50:05-07:00\nTool is running as a background task with task id: conv/task-{i}\nTask Description: {description}\n"}}"#
    )
}

/// `schedule` reports its timer through the same "background task" line as a background
/// `run_command`. Reading that line as a command result orphaned the timer's step, left the
/// `schedule` call pending, and let it claim the NEXT `GENERIC` result — so every later
/// `GENERIC` tool was attributed one call too early and the last of them orphaned
/// (22 orphans over session `29b623ae`'s 1.4.7 tail).
#[test]
fn r2_6_schedule_background_start_pairs_with_schedule_not_the_next_generic_call() {
    let s = ag_fold(&[
        ag_dispatch(1, "schedule", r#""toolSummary":"\"Wait 5s\"""#),
        ag_background_start(2, "Timer: 5s, Prompt: Check subagent message"),
        ag_dispatch(3, "manage_subagents", r#""toolSummary":"\"Check agents\"""#),
        ag_step(
            4,
            "GENERIC",
            r#","content":"Completed At: x\nagents: 1 running""#,
        ),
        ag_dispatch(5, "read_file", r#""Path":"\"/a.rs\"""#),
        ag_step(6, "GENERIC", r#","content":"Showing lines 1-10""#),
    ]);

    assert_eq!(s.orphan_tool_results, 0, "every result has its own call");
    assert_eq!(s.total_tool_calls, 3);
    let names: Vec<_> = s.tools.iter().map(|t| t.name.as_str()).collect();
    // `tools` is newest-first.
    assert_eq!(
        names,
        ["Check agents", "Wait 5s"],
        "each result credited to its own call"
    );
    assert!(
        s.files
            .iter()
            .any(|f| f.op == FileOp::Read && f.path == "/a.rs"),
        "read_file's result reaches the file ring"
    );
}

/// The other owner of the same line: a background `run_command` must still claim it.
#[test]
fn r2_7_background_run_command_still_pairs_with_its_start_step() {
    let s = ag_fold(&[
        ag_dispatch(1, "run_command", r#""toolSummary":"\"Search caches\"""#),
        ag_background_start(2, "grep -rn foo ~/.cache"),
    ]);

    assert_eq!(s.orphan_tool_results, 0);
    let names: Vec<_> = s.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Search caches"]);
}

/// A result body can QUOTE a marker: this `read_file` shows a transcript, whose escaped
/// JSON contains `Command exited with code`. Matched anywhere, the body was re-typed as a
/// command result and orphaned, though `read_file` was the call waiting for it.
#[test]
fn r2_8_a_marker_quoted_inside_a_result_body_does_not_retype_it() {
    let quoted = r#"Created At: x\\nCompleted At: x\\nCommand exited with code 0.\\nCreated the following subagent:"#;
    let s = ag_fold(&[
        ag_dispatch(1, "read_file", r#""Path":"\"/t.jsonl\"""#),
        ag_step(
            2,
            "GENERIC",
            &format!(
                r#","content":"Created At: x\nShowing lines 1-2 of 2\n1: {{\"content\":\"{quoted}\"}}\n""#
            ),
        ),
    ]);

    assert_eq!(s.orphan_tool_results, 0, "read_file claims its own result");
    assert!(s
        .files
        .iter()
        .any(|f| f.op == FileOp::Read && f.path == "/t.jsonl"));
}

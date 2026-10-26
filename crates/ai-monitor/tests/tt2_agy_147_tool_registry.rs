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

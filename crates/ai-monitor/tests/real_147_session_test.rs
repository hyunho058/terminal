//! Real-world integration test against Antigravity CLI 1.4.7 transcript from session 29b623ae.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;
use ai_monitor::telemetry::{FileOp, LogFormat, SessionTelemetry};

const REAL_SESSION_PATH: &str =
    "/Users/hyunhokim/.gemini/antigravity-cli/brain/29b623ae-28d4-4cdc-a986-6031033c115a/.system_generated/logs/transcript.jsonl";

#[test]
fn test_live_session_29b623ae_file_if_present() {
    let path = Path::new(REAL_SESSION_PATH);
    if !path.exists() {
        println!("Real session file not present, skipping live file test.");
        return;
    }

    let file = File::open(path).expect("open real session transcript");
    let reader = BufReader::new(file);

    let mut telemetry = SessionTelemetry::with_format(LogFormat::Antigravity);
    let mut line_count = 0;
    let mut saw_write_during_stream = false;

    for line in reader.lines() {
        let line = line.expect("read line");
        if !line.trim().is_empty() {
            telemetry.apply(&line);
            line_count += 1;
            if !saw_write_during_stream
                && telemetry
                    .snapshot()
                    .files
                    .iter()
                    .any(|f| f.op == FileOp::Write)
            {
                saw_write_during_stream = true;
            }
        }
    }

    assert!(line_count > 100, "Real transcript has > 100 lines");
    let s = telemetry.snapshot();

    // 1. ACTIVE AGENTS was empty before the fix; now it must be populated!
    assert!(
        !s.subagents.is_empty(),
        "ACTIVE AGENTS must not be empty on real 1.4.7 transcript"
    );
    let reviewer = s
        .subagents
        .iter()
        .find(|a| a.description.as_deref() == Some("Spec Reviewer"))
        .expect("Spec Reviewer subagent should be captured from args");
    assert_eq!(reviewer.subagent_type.as_deref(), Some("research"));
    assert!(reviewer.completed, "Spec Reviewer completed via SYSTEM_MESSAGE");

    // 2. FILE ACTIVITY was empty before; now it must capture read_file and edit_file!
    assert!(
        !s.files.is_empty(),
        "FILE ACTIVITY must contain file operations"
    );
    assert_eq!(
        s.files.len(),
        32,
        "FILE ACTIVITY ring buffer filled up to FILES_CAP (32)"
    );
    assert!(
        s.files.iter().any(|f| f.op == FileOp::Read),
        "Must capture read_file operations"
    );
    assert!(
        saw_write_during_stream,
        "Must capture edit_file / write_file operations during transcript playback"
    );

    // 3. RECENT CALLS must contain executed commands and have 0 orphan results!
    assert!(
        !s.tools.is_empty(),
        "RECENT CALLS must contain tool/command executions"
    );
    assert_eq!(
        s.orphan_tool_results, 0,
        "All 1.4.7 tool calls must pair with 0 orphans"
    );

    // 4. Token & Cache metrics were 0 before; now they must be positive!
    assert!(
        s.total_input > 0,
        "total_input must be populated from PLANNER_RESPONSE"
    );
    assert!(
        s.total_cache_read > 0,
        "total_cache_read must be populated from prompt caching"
    );
    assert!(
        s.context_used > 0,
        "context_used occupancy gauge must be positive"
    );
    assert!(
        s.context_high_water >= s.context_used,
        "context_high_water must track max occupancy"
    );
}

#[test]
fn test_exact_steps_from_session_29b623ae_in_flight_and_completion() {
    let mut telemetry = SessionTelemetry::with_format(LogFormat::Antigravity);

    // Step 125: invoke_subagent with Role and TypeName in args
    let step_125 = r#"{"step_index":125,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:36:30Z","input_tokens":2812,"output_tokens":457,"cache_read_tokens":88839,"tool_calls":[{"name":"invoke_subagent","args":{"Model":"\"inherit\"","Prompt":"\"review\"","Role":"\"L2 Clarity Reviewer\"","TypeName":"\"research\""}}]}"#;
    telemetry.apply(step_125);

    // Step 126: GENERIC result step with singular "Created the following subagent:"
    let step_126 = r#"{"step_index":126,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-10-24T18:36:31Z","content":"Created At: 2026-10-24T11:36:31-07:00\nCompleted At: 2026-10-24T11:36:31-07:00\nCreated the following subagent:\n{\n  \"conversationId\":  \"abec5595-2f52-43fe-81bf-2ff6c8bae8f2\",\n  \"logAbsoluteUri\":  \"file:///tmp\"\n}"}"#;
    telemetry.apply(step_126);

    // Step 127: tool-less reply while subagent is running
    let step_127 = r#"{"step_index":127,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:36:32Z","input_tokens":2128,"output_tokens":39,"cache_read_tokens":90234,"content":"And waiting for the L2 Clarity Reviewer to complete the review."}"#;
    telemetry.apply(step_127);

    // Verify subagent is in-flight:
    let s = telemetry.snapshot();
    assert_eq!(s.subagents.len(), 1);
    let sub = &s.subagents[0];
    assert_eq!(sub.id, "abec5595-2f52-43fe-81bf-2ff6c8bae8f2");
    assert_eq!(sub.subagent_type.as_deref(), Some("research"));
    assert_eq!(sub.description.as_deref(), Some("L2 Clarity Reviewer"));
    assert!(
        !sub.completed,
        "Subagent MUST be in-flight before SYSTEM_MESSAGE arrives"
    );
    assert_eq!(sub.end_ms, None);

    // Verify tail_state sees this as AssistantBusy (sidebar card shows Working, not Waiting!)
    let dir = std::env::temp_dir().join(format!("ai-monitor-real-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let transcript_path = dir.join("transcript.jsonl");
    let content = format!(
        "{}\n{}\n{}\n{}\n",
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-10-24T18:36:00Z","content":"<USER_REQUEST>check</USER_REQUEST>"}"#,
        step_125,
        step_126,
        step_127
    );
    std::fs::write(&transcript_path, content).unwrap();

    let state = antigravity::tail_state(&transcript_path).expect("tail state exists");
    assert_eq!(
        state.last,
        Some(LastMessage::AssistantBusy),
        "Sidebar card MUST be AssistantBusy (Working), NOT Waiting!"
    );

    // Step 128: SYSTEM_MESSAGE arrives completing the subagent
    let step_128 = r#"{"step_index":128,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","created_at":"2026-10-24T18:37:17Z","content":"<SYSTEM_MESSAGE>\n[Message] timestamp=2026-10-24T18:37:17Z sender=abec5595-2f52-43fe-81bf-2ff6c8bae8f2 priority=MESSAGE_PRIORITY_HIGH content=PASS\n</SYSTEM_MESSAGE>"}"#;
    telemetry.apply(step_128);

    let s2 = telemetry.snapshot();
    assert!(
        s2.subagents[0].completed,
        "Subagent MUST be completed after SYSTEM_MESSAGE arrives"
    );
    assert!(s2.subagents[0].end_ms.is_some());
}

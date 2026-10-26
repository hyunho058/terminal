//! TT3: Acceptance tests for R3 of `specs/agy-1.4.7-telemetry/spec.md`.
//!
//! Subagent Metadata and Asynchronous Lifecycle — D4, D5, D9.

use ai_monitor::telemetry::{LogFormat, SessionTelemetry};
use serde_json::json;

const CID: &str = "c5fdd0df-a369-4025-a618-7905f77cbd04";

fn init_session() -> SessionTelemetry {
    SessionTelemetry::with_format(LogFormat::Antigravity)
}

fn dispatch_invoke_subagent(type_name: &str, role: &str) -> String {
    json!({
        "step_index": 1,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "created_at": "2026-10-24T18:00:00Z",
        "tool_calls": [{
            "name": "invoke_subagent",
            "args": {
                "TypeName": format!("\"{type_name}\""),
                "Role": format!("\"{role}\"")
            }
        }]
    })
    .to_string()
}

fn subagent_launch_receipt(cid: &str) -> String {
    json!({
        "step_index": 2,
        "source": "MODEL",
        "type": "GENERIC",
        "status": "DONE",
        "created_at": "2026-10-24T18:00:01Z",
        "content": format!(
            "Created At: 2026-10-24T11:38:22-07:00\nCompleted At: 2026-10-24T11:38:22-07:00\nCreated the following subagent:\n{{\n  \"conversationId\": \"{cid}\",\n  \"logAbsoluteUri\": \"file:///tmp/brain/{cid}\"\n}}"
        )
    })
    .to_string()
}

fn system_message(created_at: &str, content: &str) -> String {
    json!({
        "step_index": 3,
        "source": "SYSTEM",
        "type": "SYSTEM_MESSAGE",
        "status": "DONE",
        "created_at": created_at,
        "content": content
    })
    .to_string()
}

/// R3.1: Subagent metadata extraction from `invoke_subagent` args
///
/// Given: A `PLANNER_RESPONSE` dispatching `invoke_subagent` with `TypeName: "research"` and `Role: "Spec Reviewer"`
/// When: Folded via `SessionTelemetry::apply`
/// Then: The resulting `Subagent` in `snapshot().subagents` has `subagent_type: Some("research")` and `description: Some("Spec Reviewer")`
#[test]
fn r3_1_subagent_metadata_extraction_from_invoke_subagent_args() {
    let mut session = init_session();

    session.apply(&dispatch_invoke_subagent("research", "Spec Reviewer"));
    session.apply(&subagent_launch_receipt(CID));

    let snap = session.snapshot();
    assert_eq!(snap.subagents.len(), 1, "expected 1 subagent entry");
    let sub = &snap.subagents[0];
    assert_eq!(
        sub.subagent_type.as_deref(),
        Some("research"),
        "subagent_type must match TypeName arg"
    );
    assert_eq!(
        sub.description.as_deref(),
        Some("Spec Reviewer"),
        "description must match Role arg"
    );
}

/// R3.2: Subagent in-flight state upon launch receipt
///
/// Given: An `invoke_subagent` dispatch followed by a result step containing `"Created the following subagent:"` with conversation id `CID`
/// When: Folded via `SessionTelemetry::apply`
/// Then: `snapshot().subagents` contains an entry with `id: CID`, `completed: false`, and `end_ms: None`
#[test]
fn r3_2_subagent_in_flight_state_upon_launch_receipt() {
    let mut session = init_session();

    session.apply(&dispatch_invoke_subagent("research", "Spec Reviewer"));
    session.apply(&subagent_launch_receipt(CID));

    let snap = session.snapshot();
    assert_eq!(snap.subagents.len(), 1, "expected 1 subagent entry");
    let sub = &snap.subagents[0];
    assert_eq!(sub.id, CID, "subagent id must match CID");
    assert!(
        !sub.completed,
        "subagent must be in-flight (completed: false) upon launch receipt"
    );
    assert_eq!(
        sub.end_ms, None,
        "in-flight subagent must have end_ms: None upon launch receipt"
    );
}

/// R3.3: Subagent completion on `SYSTEM_MESSAGE`
///
/// Given: A subagent in `snapshot().subagents` with `id: CID` and `completed: false`
/// When: A `SYSTEM_MESSAGE` step with content `[Message] timestamp=2026-10-24T18:37:17Z sender=CID priority=HIGH` is folded via `SessionTelemetry::apply`
/// Then: The subagent with `id: CID` in `snapshot().subagents` transitions to `completed: true` with `end_ms` set to the timestamp in milliseconds
#[test]
fn r3_3_subagent_completion_on_system_message() {
    let mut session = init_session();

    session.apply(&dispatch_invoke_subagent("research", "Spec Reviewer"));
    session.apply(&subagent_launch_receipt(CID));

    // Notice line according to R3.3:
    // [Message] timestamp=2026-10-24T18:37:17Z sender=CID priority=HIGH
    let notice = format!(
        "The following is a <SYSTEM_MESSAGE> not actually sent by the user.\n\n<SYSTEM_MESSAGE>\n[Message] timestamp=2026-10-24T18:37:17Z sender={CID} priority=HIGH content=Task completed\n</SYSTEM_MESSAGE>"
    );
    session.apply(&system_message("2026-10-24T18:37:17Z", &notice));

    let snap = session.snapshot();
    assert_eq!(snap.subagents.len(), 1, "expected 1 subagent entry");
    let sub = &snap.subagents[0];
    assert_eq!(sub.id, CID);
    assert!(
        sub.completed,
        "subagent must transition to completed: true on SYSTEM_MESSAGE"
    );
    // 2026-10-24T18:37:17Z in epoch milliseconds is 1_792_867_037_000
    assert_eq!(
        sub.end_ms,
        Some(1_792_867_037_000),
        "end_ms must be timestamp in milliseconds"
    );
}

/// R3.4: Multi-message or malformed `SYSTEM_MESSAGE` handling
///
/// Given: A `SYSTEM_MESSAGE` step with an unknown sender id or malformed header
/// When: Folded via `SessionTelemetry::apply`
/// Then: No panic occurs, existing subagent entries remain unmodified, and no orphan errors are recorded
#[test]
fn r3_4_multi_message_or_malformed_system_message_handling() {
    let mut session = init_session();

    session.apply(&dispatch_invoke_subagent("research", "Spec Reviewer"));
    session.apply(&subagent_launch_receipt(CID));

    // Initial state check
    let before_sub = session.snapshot().subagents[0].clone();

    // 1. Unknown sender id: should be safely ignored
    let unknown_sender_msg = system_message(
        "2026-10-24T18:30:00Z",
        "[Message] timestamp=2026-10-24T18:30:00Z sender=00000000-0000-0000-0000-000000000000 priority=HIGH content=ignore me",
    );
    session.apply(&unknown_sender_msg);

    let snap = session.snapshot();
    assert_eq!(
        snap.subagents[0], before_sub,
        "existing subagent must remain unmodified when sender is unknown"
    );
    assert_eq!(
        snap.orphan_tool_results, 0,
        "unknown sender must not record orphan tool results"
    );
    assert_eq!(
        snap.orphan_task_updates, 0,
        "unknown sender must not record orphan task updates"
    );
    assert_eq!(
        snap.parse_errors, 0,
        "unknown sender must not record parse errors"
    );

    // 2. Malformed header: should not panic or corrupt state
    let malformed_msg = system_message(
        "2026-10-24T18:31:00Z",
        "[Message] malformed sender without valid timestamp or sender tokens",
    );
    session.apply(&malformed_msg);

    let snap = session.snapshot();
    assert_eq!(
        snap.subagents[0], before_sub,
        "existing subagent must remain unmodified on malformed header"
    );
    assert_eq!(
        snap.parse_errors, 0,
        "malformed message must not increment parse_errors"
    );

    // 3. Multi-message monotone completion (D9):
    // First message completes subagent at 2026-10-24T18:37:17Z (1_792_867_037_000 ms)
    let msg1 = format!("[Message] timestamp=2026-10-24T18:37:17Z sender={CID} priority=HIGH");
    session.apply(&system_message("2026-10-24T18:37:17Z", &msg1));

    let snap1 = session.snapshot();
    assert!(snap1.subagents[0].completed);
    assert_eq!(snap1.subagents[0].end_ms, Some(1_792_867_037_000));

    // Second message from same sender with later timestamp 2026-10-24T18:40:00Z (1_792_867_200_000 ms)
    let msg2 = format!("[Message] timestamp=2026-10-24T18:40:00Z sender={CID} priority=HIGH");
    session.apply(&system_message("2026-10-24T18:40:00Z", &msg2));

    let snap2 = session.snapshot();
    assert!(snap2.subagents[0].completed);
    assert_eq!(
        snap2.subagents[0].end_ms,
        Some(1_792_867_200_000),
        "later message from same sender updates end_ms monotonically"
    );

    // Stale/earlier message arriving later (2026-10-24T18:35:00Z = 1_792_866_900_000 ms) must not regress end_ms
    let msg_stale = format!("[Message] timestamp=2026-10-24T18:35:00Z sender={CID} priority=HIGH");
    session.apply(&system_message("2026-10-24T18:35:00Z", &msg_stale));

    let snap3 = session.snapshot();
    assert_eq!(
        snap3.subagents[0].end_ms,
        Some(1_792_867_200_000),
        "stale earlier timestamp must not regress end_ms"
    );
}

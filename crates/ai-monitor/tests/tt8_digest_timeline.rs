//! TT8 — acceptance tests for `specs/session-overview-ui` R8.1 and R8.6 (D12, D19): the
//! digest's turns end only once background work is done, and each subagent carries a
//! report preview from either result form.
//!
//! Every case folds Claude Code JSONL records through `SessionTelemetry::apply` on an
//! accumulator built `with_digest()` and reads `snapshot()`. Record shapes: a human prompt
//! is a `user` record with `origin.kind: "human"`; a background launch is an `Agent`
//! result whose `toolUseResult` carries `isAsync` + `async_launched` + `agentId`; a task
//! notification is a `user` record with `origin.kind: "task-notification"` and a string
//! `<task-notification>` body.

use ai_monitor::telemetry::{SessionTelemetry, Turn};
use serde_json::{json, Value};

const BG_AGENT: &str = "a7c3e1f09b2d4e6f8";

/// `2026-10-31T10:{min}:{sec}Z`.
fn stamp(min: u32, sec: u32) -> String {
    format!("2026-10-31T10:{min:02}:{sec:02}.000Z")
}

/// Epoch ms of [`stamp`].
fn ms(min: u32, sec: u32) -> u64 {
    // 2026-10-31T00:00:00Z
    const DAY_MS: u64 = 1_793_404_800_000;
    DAY_MS + 10 * 3_600_000 + u64::from(min) * 60_000 + u64::from(sec) * 1_000
}

fn human(min: u32, text: &str) -> Value {
    json!({"type": "user", "timestamp": stamp(min, 0), "sessionId": "sess-tt8",
           "origin": {"kind": "human"},
           "message": {"role": "user", "content": text}})
}

fn agent_call(min: u32, sec: u32, id: &str, description: &str) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, sec), "sessionId": "sess-tt8",
           "message": {"id": format!("msg_{id}"), "role": "assistant", "stop_reason": "tool_use",
                       "content": [{"type": "tool_use", "id": id, "name": "Agent",
                                    "input": {"description": description,
                                              "subagent_type": "general-purpose",
                                              "prompt": "do it"}}]}})
}

fn tool_result(min: u32, sec: u32, id: &str, content: &str, tur: Value) -> Value {
    json!({"type": "user", "timestamp": stamp(min, sec), "sessionId": "sess-tt8",
           "toolUseResult": tur,
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": id, "content": content}]}})
}

fn background_launch(min: u32, sec: u32, id: &str) -> Value {
    tool_result(
        min,
        sec,
        id,
        "Async agent launched successfully.",
        json!({"isAsync": true, "status": "async_launched", "agentId": BG_AGENT}),
    )
}

fn end_turn(min: u32) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, 0), "sessionId": "sess-tt8",
           "message": {"id": format!("msg_end_{min}"), "role": "assistant",
                       "stop_reason": "end_turn",
                       "content": [{"type": "text", "text": "Okay."}]}})
}

fn notification(min: u32, agent: &str, result: &str) -> Value {
    let body = format!(
        "<task-notification>\n<task-id>{agent}</task-id>\n<status>completed</status>\n\
         <summary>Agent \"bg\" completed</summary>\n<result>{result}</result>\n\
         </task-notification>"
    );
    json!({"type": "user", "timestamp": stamp(min, 0), "sessionId": "sess-tt8",
           "origin": {"kind": "task-notification"}, "promptSource": "system",
           "message": {"role": "user", "content": body}})
}

fn fold(t: &mut SessionTelemetry, records: &[Value]) {
    for r in records {
        t.apply(&r.to_string());
    }
}

// ---- R8.1 -------------------------------------------------------------------------

/// R8.1: prompt 10:00, background `Agent` 10:01, `end_turn` 10:02, its notification 10:09,
/// `end_turn` 10:10 → one turn `[10:00, 10:10]`; before the notification, `end_ms` is
/// `None`.
#[test]
fn r8_1_turn_ends_only_when_background_work_is_done() {
    let mut t = SessionTelemetry::new().with_digest();
    fold(
        &mut t,
        &[
            human(0, "scan the repository in the background"),
            agent_call(1, 0, "toolu_bg", "Scan the repository"),
            background_launch(1, 1, "toolu_bg"),
            end_turn(2),
        ],
    );
    assert_eq!(
        t.snapshot().turns,
        vec![Turn {
            start_ms: ms(0, 0),
            end_ms: None,
        }],
        "before the notification is folded the turn is still open"
    );

    fold(
        &mut t,
        &[notification(9, BG_AGENT, "Scanned it."), end_turn(10)],
    );
    assert_eq!(
        t.snapshot().turns,
        vec![Turn {
            start_ms: ms(0, 0),
            end_ms: Some(ms(10, 0)),
        }],
        "one turn [10:00, 10:10]"
    );
}

// ---- R8.6 -------------------------------------------------------------------------

/// R8.6: a synchronous `Agent` result with 6 non-blank lines previews its first 4 (≤ 400
/// chars); a background agent's notification `<result>` of 2 lines previews those 2.
#[test]
fn r8_6_report_preview_from_both_result_forms() {
    let sync_report = "Alpha finding\n\nBravo finding\n  \nCharlie finding\nDelta finding\n\
                       Echo finding\nFoxtrot finding";
    let mut t = SessionTelemetry::new().with_digest();
    fold(
        &mut t,
        &[
            human(0, "review and scan"),
            agent_call(1, 0, "toolu_sync", "Review the diff"),
            tool_result(
                3,
                0,
                "toolu_sync",
                sync_report,
                json!({"status": "completed", "agentId": "a0sync0000000001"}),
            ),
            agent_call(4, 0, "toolu_bg", "Scan in background"),
            background_launch(4, 1, "toolu_bg"),
            notification(8, BG_AGENT, "\nFirst result line\n\nSecond result line\n"),
        ],
    );
    let s = t.snapshot();
    assert_eq!(s.subagents.len(), 2, "two subagents: {:?}", s.subagents);

    // The snapshot's subagent ring is newest first; look each up by its tool_use id.
    let by_id = |id: &str| {
        s.subagents
            .iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("subagent {id}: {:?}", s.subagents))
    };
    let first = by_id("toolu_sync")
        .report_preview
        .clone()
        .expect("the synchronous agent has a report preview");
    let first_lines: Vec<&str> = first.lines().collect();
    assert_eq!(
        first_lines,
        [
            "Alpha finding",
            "Bravo finding",
            "Charlie finding",
            "Delta finding"
        ],
        "the first 4 non-blank lines"
    );
    assert!(first.chars().count() <= 400, "≤ 400 chars: {first:?}");

    let second = by_id("toolu_bg")
        .report_preview
        .clone()
        .expect("the background agent has a report preview");
    let second_lines: Vec<&str> = second.lines().collect();
    assert_eq!(
        second_lines,
        ["First result line", "Second result line"],
        "its 2 lines"
    );
}

/// R8.6's cap: 6 non-blank lines of 150 chars — the first 4 would be ~600 chars, so the
/// preview stops at 400 and is still a prefix of those lines.
#[test]
fn r8_6_report_preview_is_at_most_400_chars() {
    let line = |c: char| c.to_string().repeat(150);
    let report = ['a', 'b', 'c', 'd', 'e', 'f']
        .iter()
        .map(|c| line(*c))
        .collect::<Vec<_>>()
        .join("\n");
    let mut t = SessionTelemetry::new().with_digest();
    fold(
        &mut t,
        &[
            human(0, "review"),
            agent_call(1, 0, "toolu_long", "Long review"),
            tool_result(2, 0, "toolu_long", &report, json!({"status": "completed"})),
        ],
    );
    let preview = t.snapshot().subagents[0]
        .report_preview
        .clone()
        .expect("a report preview");
    assert!(
        preview.chars().count() <= 400,
        "capped at 400 chars, got {}",
        preview.chars().count()
    );
    assert!(
        preview.starts_with(&line('a')),
        "it begins with the first line"
    );
    assert!(!preview.contains('e'), "never reaches the fifth line");
}

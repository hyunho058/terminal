//! TT11 — acceptance test for `specs/session-overview-ui` R11.3 (D24): an Antigravity
//! transcript, folded with the digest flag on, yields the same digest a Claude log does —
//! the subagent, the newest human prompt's time and the turn it opened.
//!
//! The steps are real-shaped Antigravity transcript lines (`step_index`, `source`, `type`,
//! `status`, `created_at`), copied in shape from the 1.4.7 fixtures in
//! `tt3_agy_147_subagent_lifecycle.rs` and `antigravity.rs`: a `USER_INPUT`, a
//! `PLANNER_RESPONSE` dispatching `invoke_subagent`, the `Created the following subagent:`
//! receipt, the subagent's `SYSTEM_MESSAGE … sender=<id>` completion, and a tool-less reply.

use ai_monitor::telemetry::{LogFormat, SessionTelemetry, Turn};
use serde_json::json;

const CID: &str = "c5fdd0df-a369-4025-a618-7905f77cbd04";

/// 2026-10-31T10:00:00Z in epoch ms — the `USER_INPUT` step (t1).
const T1: u64 = 1_793_440_800_000;
/// 2026-10-31T10:03:00Z — the tool-less reply (t2).
const T2: u64 = T1 + 180_000;

fn user_input() -> String {
    json!({
        "step_index": 0,
        "source": "USER_EXPLICIT",
        "type": "USER_INPUT",
        "status": "DONE",
        "created_at": "2026-10-31T10:00:00Z",
        "content": "<USER_REQUEST>\nreview the spec for gaps\n</USER_REQUEST>"
    })
    .to_string()
}

fn dispatch_subagent() -> String {
    json!({
        "step_index": 1,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "created_at": "2026-10-31T10:00:05Z",
        "tool_calls": [{
            "name": "invoke_subagent",
            "args": {
                "TypeName": "\"research\"",
                "Role": "\"Spec Reviewer\""
            }
        }]
    })
    .to_string()
}

fn launch_receipt() -> String {
    json!({
        "step_index": 2,
        "source": "MODEL",
        "type": "GENERIC",
        "status": "DONE",
        "created_at": "2026-10-31T10:00:06Z",
        "content": format!(
            "Created At: 2026-10-31T03:00:06-07:00\nCompleted At: 2026-10-31T03:00:06-07:00\nCreated the following subagent:\n{{\n  \"conversationId\": \"{CID}\",\n  \"logAbsoluteUri\": \"file:///tmp/brain/{CID}\"\n}}"
        )
    })
    .to_string()
}

fn subagent_done() -> String {
    json!({
        "step_index": 3,
        "source": "SYSTEM",
        "type": "SYSTEM_MESSAGE",
        "status": "DONE",
        "created_at": "2026-10-31T10:02:00Z",
        "content": format!(
            "The following is a <SYSTEM_MESSAGE> not actually sent by the user.\n\n<SYSTEM_MESSAGE>\n[Message] timestamp=2026-10-31T10:02:00Z sender={CID} priority=HIGH content=Task completed\n</SYSTEM_MESSAGE>"
        )
    })
    .to_string()
}

fn reply() -> String {
    json!({
        "step_index": 4,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "created_at": "2026-10-31T10:03:00Z",
        "content": "The reviewer found two gaps; both are fixed in the spec."
    })
    .to_string()
}

#[test]
fn r11_3_antigravity_transcript_yields_a_digest() {
    let mut t = SessionTelemetry::with_format(LogFormat::Antigravity).with_digest();
    for line in [
        user_input(),
        dispatch_subagent(),
        launch_receipt(),
        subagent_done(),
        reply(),
    ] {
        t.apply(&line);
    }

    let snap = t.snapshot();

    assert_eq!(
        snap.subagents.len(),
        1,
        "one subagent: {:#?}",
        snap.subagents
    );
    let sub = &snap.subagents[0];
    assert_eq!(sub.id, CID);
    assert!(sub.completed, "the subagent is completed: {sub:#?}");

    assert_eq!(
        snap.last_prompt_ms,
        Some(T1),
        "last_prompt_ms is the USER_INPUT step's time"
    );
    assert_eq!(
        snap.turns,
        vec![Turn {
            start_ms: T1,
            end_ms: Some(T2),
        }],
        "one turn [t1, t2]"
    );
}

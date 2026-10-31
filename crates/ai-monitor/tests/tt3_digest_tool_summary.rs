//! Acceptance tests for `specs/session-overview-ui` R3.6 and R3.7 (D11): the per-tool
//! summary a digest fold records, and that a plain (non-digest) fold does no digest work.
//!
//! Every case goes through the public boundary only: `SessionTelemetry::apply` per JSONL
//! line, then `snapshot()`.

use ai_monitor::telemetry::{SessionTelemetry, TelemetrySnapshot};
use serde_json::{json, Value};

/// `2026-10-31T09:{min}:{sec}Z` as the log writes it.
fn stamp(min: u32, sec: u32) -> String {
    format!("2026-10-31T09:{min:02}:{sec:02}.000Z")
}

fn prompt(min: u32, text: &str) -> Value {
    json!({"type": "user", "timestamp": stamp(min, 0), "sessionId": "tt3",
           "origin": {"kind": "human"},
           "message": {"role": "user", "content": text}})
}

fn tool_use(min: u32, id: &str, name: &str, input: Value) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, 0), "sessionId": "tt3",
           "message": {"id": format!("msg_{id}"), "role": "assistant", "stop_reason": "tool_use",
                       "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}})
}

fn tool_result(min: u32, id: &str, content: &str, tur: Value) -> Value {
    json!({"type": "user", "timestamp": stamp(min, 30), "sessionId": "tt3",
           "toolUseResult": tur,
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": id, "content": content}]}})
}

fn turn_end(min: u32) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, 0), "sessionId": "tt3",
           "message": {"id": format!("msg_end_{min}"), "role": "assistant",
                       "stop_reason": "end_turn",
                       "content": [{"type": "text", "text": "ok"}]}})
}

/// A finished (call + result) non-agent tool at minute `min`.
fn done(min: u32, id: &str, name: &str, input: Value) -> Vec<Value> {
    vec![
        tool_use(min, id, name, input),
        tool_result(min, id, "ok", json!({"stdout": "ok"})),
    ]
}

fn apply_all(t: &mut SessionTelemetry, lines: &[Value]) {
    for l in lines {
        t.apply(&l.to_string());
    }
}

/// The R3.6 cases, in order: (name, input, expected `last_tool` summary).
fn cases() -> Vec<(&'static str, Value, Option<String>)> {
    vec![
        (
            "Bash",
            json!({"command": "git status\n--short"}),
            Some("git status".to_string()),
        ),
        (
            "Edit",
            json!({"file_path": "/a/b/lib.rs"}),
            Some("lib.rs".to_string()),
        ),
        (
            "Grep",
            json!({"pattern": "fn main"}),
            Some("fn main".to_string()),
        ),
        (
            "WebFetch",
            json!({"url": "https://docs.rs/x"}),
            Some("docs.rs".to_string()),
        ),
        (
            "Bash",
            json!({"command": "a".repeat(200)}),
            Some(format!("{}…", "a".repeat(79))),
        ),
        ("Foo", json!({"command": "not summarised"}), None),
    ]
}

/// The R3.6 log as one sequence (a prompt, the six tools, then a synchronous `Agent`
/// call and an end of turn), used by R3.7.
fn whole_log() -> Vec<Value> {
    let mut log = vec![prompt(0, "tidy up")];
    for (i, (name, input, _)) in cases().into_iter().enumerate() {
        let min = 1 + u32::try_from(i).expect("small");
        log.extend(done(min, &format!("toolu_{i}"), name, input));
    }
    log.push(tool_use(
        10,
        "toolu_agent",
        "Agent",
        json!({"description": "Review the diff", "subagent_type": "reviewer"}),
    ));
    log.push(tool_result(
        10,
        "toolu_agent",
        "looks fine\nno issues",
        json!({"status": "completed", "agentId": "tt3sync"}),
    ));
    log.push(turn_end(11));
    log
}

#[test]
fn r3_6_last_tool_summaries_and_agent_skips_last_work_tool() {
    let mut t = SessionTelemetry::new().with_digest();
    apply_all(&mut t, &[prompt(0, "tidy up")]);
    for (i, (name, input, want)) in cases().into_iter().enumerate() {
        let min = 1 + u32::try_from(i).expect("small");
        apply_all(&mut t, &done(min, &format!("toolu_{i}"), name, input));
        let s = t.snapshot();
        let summary = s.last_tool.as_ref().and_then(|d| d.1.clone());
        assert_eq!(summary, want, "last_tool summary after {name} (case {i})");
    }

    // An `Agent` call updates `last_tool` but not `last_work_tool`.
    let before = t.snapshot();
    let work_before = before.last_work_tool.clone();
    assert!(work_before.is_some(), "the Foo call is a work tool");
    apply_all(
        &mut t,
        &[
            tool_use(
                10,
                "toolu_agent",
                "Agent",
                json!({"description": "Review the diff", "subagent_type": "reviewer"}),
            ),
            tool_result(
                10,
                "toolu_agent",
                "looks fine",
                json!({"status": "completed", "agentId": "tt3sync"}),
            ),
        ],
    );
    let after = t.snapshot();
    assert_eq!(
        after.last_tool.as_ref().map(|d| d.0.as_str()),
        Some("Agent"),
        "an Agent call updates last_tool"
    );
    assert_eq!(
        after.last_work_tool, work_before,
        "an Agent call does not update last_work_tool"
    );
}

fn strip_digest(mut s: TelemetrySnapshot) -> TelemetrySnapshot {
    s.last_tool = None;
    s.last_work_tool = None;
    s.last_prompt_ms = None;
    s.turns.clear();
    s.events.clear();
    for a in &mut s.subagents {
        a.report_preview = None;
    }
    s
}

#[test]
fn r3_7_plain_fold_has_no_digest_and_matches_old_fields() {
    let log = whole_log();
    let mut plain = SessionTelemetry::new();
    let mut with = SessionTelemetry::new().with_digest();
    apply_all(&mut plain, &log);
    apply_all(&mut with, &log);
    let p = plain.snapshot();
    let d = with.snapshot();

    assert_eq!(p.last_tool, None);
    assert_eq!(p.last_work_tool, None);
    assert_eq!(p.last_prompt_ms, None);
    assert!(p.turns.is_empty(), "turns: {:?}", p.turns);
    assert!(p.events.is_empty(), "events: {:?}", p.events);
    assert!(
        p.subagents.iter().all(|a| a.report_preview.is_none()),
        "report_preview must be None without the digest flag"
    );

    // The comparison below must not be vacuous: the digest fold folded something and
    // both folds saw the subagent.
    assert!(d.last_tool.is_some());
    assert!(!p.subagents.is_empty());

    assert_eq!(strip_digest(d), p);
}

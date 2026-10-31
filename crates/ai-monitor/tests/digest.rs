//! The session digest (`specs/session-overview-ui` T1: R3.6, R3.7, R8.1, R8.6).
//!
//! Every case folds JSONL lines through the public boundary, `SessionTelemetry::apply`,
//! and reads `snapshot()`. The record shapes are the ones `tt1_claude_background.rs` and
//! `telemetry.rs` use: a human prompt carries `origin.kind: "human"`, a background
//! launch is a `toolUseResult` with `isAsync` + `async_launched`, a notification is a
//! `user` record with `origin.kind: "task-notification"` and a string body.

use ai_monitor::telemetry::{EventKind, SessionTelemetry, TelemetrySnapshot, Turn};
use serde_json::{json, Value};

const AGENT_ID: &str = "aee2b85e39714d85f";

/// `2026-10-31T10:{min}:{sec}Z` as the log writes it.
fn stamp(min: u32, sec: u32) -> String {
    format!("2026-10-31T10:{min:02}:{sec:02}.000Z")
}

/// The epoch ms of `stamp(min, sec)`.
fn ms(min: u32, sec: u32) -> u64 {
    // 2026-10-31T00:00:00Z
    const DAY: u64 = 1_793_404_800_000;
    DAY + 10 * 3_600_000 + u64::from(min) * 60_000 + u64::from(sec) * 1_000
}

fn human(min: u32, text: &str) -> Value {
    json!({"type": "user", "timestamp": stamp(min, 0), "sessionId": "s",
           "origin": {"kind": "human"},
           "message": {"role": "user", "content": text}})
}

fn call(min: u32, sec: u32, id: &str, name: &str, input: Value) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, sec), "sessionId": "s",
           "message": {"id": format!("msg_{id}"), "role": "assistant", "stop_reason": "tool_use",
                       "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}})
}

fn result(min: u32, sec: u32, id: &str, content: &str, tur: Value) -> Value {
    json!({"type": "user", "timestamp": stamp(min, sec), "sessionId": "s",
           "toolUseResult": tur,
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": id, "content": content}]}})
}

fn end_turn(min: u32) -> Value {
    json!({"type": "assistant", "timestamp": stamp(min, 0), "sessionId": "s",
           "message": {"id": format!("msg_end_{min}"), "role": "assistant",
                       "stop_reason": "end_turn",
                       "content": [{"type": "text", "text": "Done."}]}})
}

fn notification(min: u32, id: &str, status: &str, result: &str) -> Value {
    let body = format!(
        "<task-notification>\n<task-id>{id}</task-id>\n<status>{status}</status>\n\
         <summary>Agent finished</summary>\n<result>{result}</result>\n</task-notification>"
    );
    json!({"type": "user", "timestamp": stamp(min, 0), "sessionId": "s",
           "origin": {"kind": "task-notification"}, "promptSource": "system",
           "message": {"role": "user", "content": body}})
}

/// A finished call: its `tool_use` at `min:00` and its result at `min:01`.
fn finished(min: u32, id: &str, name: &str, input: Value) -> [Value; 2] {
    [
        call(min, 0, id, name, input),
        result(min, 1, id, "ok", json!({"stdout": "ok"})),
    ]
}

fn fold(t: &mut SessionTelemetry, records: &[Value]) {
    for r in records {
        t.apply(&r.to_string());
    }
}

fn digest() -> SessionTelemetry {
    SessionTelemetry::new().with_digest()
}

fn last_summary(s: &TelemetrySnapshot) -> Option<String> {
    s.last_tool.as_ref().and_then(|t| t.1.clone())
}

// ---- R3.6 -------------------------------------------------------------------------

#[test]
fn r3_6_last_tool_summary_follows_the_d11_table() {
    let mut t = digest();
    let long = "x".repeat(200);
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "Bash",
            json!({"command": "git status\n--short"}),
            Some("git status".into()),
        ),
        (
            "Edit",
            json!({"file_path": "/a/b/lib.rs"}),
            Some("lib.rs".into()),
        ),
        (
            "Grep",
            json!({"pattern": "fn main"}),
            Some("fn main".into()),
        ),
        (
            "WebFetch",
            json!({"url": "https://docs.rs/x"}),
            Some("docs.rs".into()),
        ),
        (
            "Bash",
            json!({"command": long}),
            Some(format!("{}…", "x".repeat(79))),
        ),
        ("Foo", json!({"command": "ignored"}), None),
    ];
    for (i, (name, input, want)) in cases.into_iter().enumerate() {
        let id = format!("toolu_{i}");
        let min = u32::try_from(i).expect("small");
        fold(&mut t, &finished(min, &id, name, input));
        let s = t.snapshot();
        assert_eq!(last_summary(&s), want, "after {name}");
        let (got_name, _, start) = s.last_tool.clone().expect("last_tool");
        assert_eq!(got_name, name);
        assert_eq!(start, Some(ms(min, 0)));
        assert_eq!(s.last_work_tool, s.last_tool, "{name} is work");
    }
    let summary = last_summary(&{
        let mut u = digest();
        fold(
            &mut u,
            &finished(0, "t", "Bash", json!({"command": "x".repeat(200)})),
        );
        u.snapshot()
    })
    .expect("summary");
    assert_eq!(summary.chars().count(), 80);
}

#[test]
fn r3_6_whitespace_collapses_and_more_tools_summarise() {
    let mut t = digest();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "Bash",
            json!({"command": "  ls   -la\t /tmp  \nnext"}),
            "ls -la /tmp",
        ),
        ("Read", json!({"file_path": "/x/y/main.rs"}), "main.rs"),
        ("Write", json!({"file_path": "notes.md"}), "notes.md"),
        (
            "NotebookEdit",
            json!({"file_path": "/n/a.ipynb"}),
            "a.ipynb",
        ),
        ("Glob", json!({"pattern": "**/*.rs"}), "**/*.rs"),
        ("WebSearch", json!({"query": "wgpu metal"}), "wgpu metal"),
        ("Skill", json!({"skill": "run-term"}), "run-term"),
        (
            "WebFetch",
            json!({"url": "https://user@example.com:8443/p?q=1"}),
            "example.com",
        ),
    ];
    for (i, (name, input, want)) in cases.into_iter().enumerate() {
        let min = u32::try_from(i).expect("small");
        fold(&mut t, &finished(min, &format!("t{i}"), name, input));
        assert_eq!(last_summary(&t.snapshot()).as_deref(), Some(want), "{name}");
    }
}

#[test]
fn r3_6_an_agent_call_updates_last_tool_but_not_last_work_tool() {
    let mut t = digest();
    fold(
        &mut t,
        &finished(0, "b", "Bash", json!({"command": "cargo test"})),
    );
    fold(
        &mut t,
        &[
            call(
                1,
                0,
                "a",
                "Agent",
                json!({"description": "Explore the parser"}),
            ),
            result(2, 0, "a", "Report.", json!({"status": "completed"})),
        ],
    );
    let s = t.snapshot();
    assert_eq!(
        s.last_tool,
        Some((
            "Agent".to_string(),
            Some("Explore the parser".to_string()),
            Some(ms(1, 0))
        ))
    );
    assert_eq!(
        s.last_work_tool,
        Some((
            "Bash".to_string(),
            Some("cargo test".to_string()),
            Some(ms(0, 0))
        ))
    );
}

// ---- R3.7 -------------------------------------------------------------------------

/// A log exercising every digest path: prompts, tools, a sync agent, a background agent
/// and its notification, a turn held open by it.
fn full_log() -> Vec<Value> {
    let mut log = vec![human(0, "fix the build")];
    log.extend(finished(0, "b1", "Bash", json!({"command": "cargo build"})));
    log.extend(finished(
        1,
        "e1",
        "Edit",
        json!({"file_path": "/r/src/lib.rs"}),
    ));
    log.push(call(
        2,
        0,
        "s1",
        "Agent",
        json!({"description": "Review", "subagent_type": "reviewer"}),
    ));
    log.push(result(
        3,
        0,
        "s1",
        "line one\nline two",
        json!({"status": "completed", "agentId": "sync1"}),
    ));
    log.push(call(
        4,
        0,
        "bg",
        "Agent",
        json!({"description": "Background scan"}),
    ));
    log.push(result(
        4,
        1,
        "bg",
        "Async agent launched successfully.",
        json!({"isAsync": true, "status": "async_launched", "agentId": AGENT_ID}),
    ));
    log.push(end_turn(5));
    log.push(notification(9, AGENT_ID, "completed", "found it\nall good"));
    log.push(end_turn(10));
    log.push(human(12, "now ship it"));
    log.extend(finished(12, "f1", "Foo", json!({})));
    log
}

fn without_digest_fields(mut s: TelemetrySnapshot) -> TelemetrySnapshot {
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
fn r3_7_the_plain_fold_does_no_digest_work_and_agrees_on_every_old_field() {
    let log = full_log();
    let mut plain = SessionTelemetry::new();
    let mut dig = digest();
    fold(&mut plain, &log);
    fold(&mut dig, &log);
    let p = plain.snapshot();
    let d = dig.snapshot();

    assert_eq!(p.last_tool, None);
    assert_eq!(p.last_work_tool, None);
    assert_eq!(p.last_prompt_ms, None);
    assert!(p.turns.is_empty());
    assert!(p.events.is_empty());
    assert!(p.subagents.iter().all(|a| a.report_preview.is_none()));

    // The digest fold did fold something, so the comparison below is not vacuous.
    assert!(d.last_tool.is_some());
    assert_eq!(d.last_prompt_ms, Some(ms(12, 0)));
    assert_eq!(d.turns.len(), 2);
    assert!(!d.events.is_empty());
    assert!(d.subagents.iter().all(|a| a.report_preview.is_some()));

    assert_eq!(without_digest_fields(d), p);
}

#[test]
fn r3_7_restarted_keeps_the_digest_flag_and_the_plain_one_stays_plain() {
    let log = full_log();
    let mut dig = digest().restarted();
    let mut plain = SessionTelemetry::new().restarted();
    fold(&mut dig, &log);
    fold(&mut plain, &log);
    assert!(dig.snapshot().last_tool.is_some());
    assert!(plain.snapshot().last_tool.is_none());
}

// ---- R8.1 -------------------------------------------------------------------------

#[test]
fn r8_1_a_turn_ends_only_when_its_background_work_is_done() {
    let mut t = digest();
    fold(
        &mut t,
        &[
            human(0, "scan the repo in the background"),
            call(1, 0, "bg", "Agent", json!({"description": "Scan"})),
            result(
                1,
                1,
                "bg",
                "Async agent launched successfully.",
                json!({"isAsync": true, "status": "async_launched", "agentId": AGENT_ID}),
            ),
            end_turn(2),
        ],
    );
    let open = [Turn {
        start_ms: ms(0, 0),
        end_ms: None,
    }];
    assert_eq!(t.snapshot().turns, open, "end_turn with work pending");

    fold(&mut t, &[notification(9, AGENT_ID, "completed", "done")]);
    assert_eq!(
        t.snapshot().turns,
        open,
        "the notification resumes the turn"
    );
    assert_eq!(
        t.snapshot().last_prompt_ms,
        Some(ms(0, 0)),
        "a notification is not a prompt"
    );

    fold(&mut t, &[end_turn(10)]);
    let s = t.snapshot();
    assert_eq!(
        s.turns,
        [Turn {
            start_ms: ms(0, 0),
            end_ms: Some(ms(10, 0)),
        }]
    );
    let finished: Vec<u64> = s
        .events
        .iter()
        .filter(|e| e.kind == EventKind::TurnFinished)
        .map(|e| e.at_ms)
        .collect();
    assert_eq!(finished, [ms(10, 0)], "one TurnFinished, at the real end");
}

#[test]
fn r8_1_a_turn_with_no_background_work_ends_at_end_turn() {
    let mut t = digest();
    fold(&mut t, &[human(0, "hi")]);
    fold(&mut t, &finished(0, "b", "Bash", json!({"command": "ls"})));
    fold(&mut t, &[end_turn(1)]);
    assert_eq!(
        t.snapshot().turns,
        [Turn {
            start_ms: ms(0, 0),
            end_ms: Some(ms(1, 0)),
        }]
    );
}

#[test]
fn r8_1_a_new_prompt_closes_an_open_turn_and_newest_comes_first() {
    let mut t = digest();
    fold(&mut t, &[human(0, "first"), human(3, "second")]);
    let s = t.snapshot();
    assert_eq!(
        s.turns,
        [
            Turn {
                start_ms: ms(3, 0),
                end_ms: None,
            },
            Turn {
                start_ms: ms(0, 0),
                end_ms: Some(ms(3, 0)),
            },
        ]
    );
    assert!(
        s.events.iter().all(|e| e.kind != EventKind::TurnFinished),
        "an interrupted turn did not finish"
    );
}

#[test]
fn r8_1_turns_and_events_keep_the_newest_64() {
    let mut t = digest();
    for i in 0..70u32 {
        let (min, sec) = (i / 60, i % 60);
        let rec = json!({"type": "user", "timestamp": stamp(min, sec), "sessionId": "s",
                         "origin": {"kind": "human"},
                         "message": {"role": "user", "content": "go"}});
        fold(&mut t, &[rec]);
        let end = json!({"type": "assistant", "timestamp": stamp(min, sec), "sessionId": "s",
                         "message": {"id": format!("m{i}"), "role": "assistant",
                                     "stop_reason": "end_turn", "content": []}});
        fold(&mut t, &[end]);
    }
    let s = t.snapshot();
    assert_eq!(s.turns.len(), 64);
    assert_eq!(s.events.len(), 64);
    assert_eq!(s.turns[0].start_ms, ms(1, 9), "newest first");
}

// ---- R8.6 -------------------------------------------------------------------------

#[test]
fn r8_6_report_preview_from_the_sync_result_and_the_notification() {
    let mut t = digest();
    let report = "Line 1\n\nLine 2\n   \nLine 3\nLine 4\nLine 5\nLine 6";
    fold(
        &mut t,
        &[
            human(0, "go"),
            call(1, 0, "sync", "Agent", json!({"description": "Sync review"})),
            result(
                2,
                0,
                "sync",
                report,
                json!({"status": "completed", "agentId": "a-sync"}),
            ),
            call(3, 0, "bg", "Agent", json!({"description": "Bg scan"})),
            result(
                3,
                1,
                "bg",
                "Async agent launched successfully.",
                json!({"isAsync": true, "status": "async_launched", "agentId": AGENT_ID}),
            ),
            notification(
                8,
                AGENT_ID,
                "completed",
                "\nFirst finding\n\nSecond finding\n",
            ),
        ],
    );
    let s = t.snapshot();
    let preview = |id: &str| {
        s.subagents
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.report_preview.clone())
    };
    assert_eq!(
        preview("sync").as_deref(),
        Some("Line 1\nLine 2\nLine 3\nLine 4")
    );
    assert_eq!(
        preview("bg").as_deref(),
        Some("First finding\nSecond finding")
    );

    let kinds: Vec<&EventKind> = s.events.iter().map(|e| &e.kind).collect();
    assert!(kinds.contains(&&EventKind::TaskStart {
        description: "Sync review".into()
    }));
    assert!(kinds.contains(&&EventKind::TaskDone {
        description: "Sync review".into()
    }));
    assert!(kinds.contains(&&EventKind::TaskDone {
        description: "Bg scan".into()
    }));
}

#[test]
fn r8_6_report_preview_is_capped_at_400_chars() {
    let mut t = digest();
    let line = "y".repeat(150);
    let report = format!("{line}\n{line}\n{line}\n{line}");
    fold(
        &mut t,
        &[
            call(1, 0, "sync", "Agent", json!({"description": "Long"})),
            result(2, 0, "sync", &report, json!({"status": "completed"})),
        ],
    );
    let p = t.snapshot().subagents[0]
        .report_preview
        .clone()
        .expect("preview");
    assert_eq!(p.chars().count(), 400);
    assert!(p.ends_with('…'));
}

#[test]
fn r8_6_a_failed_notification_is_a_task_failed_event() {
    let mut t = digest();
    fold(
        &mut t,
        &[
            call(3, 0, "bg", "Agent", json!({"description": "Bg scan"})),
            result(
                3,
                1,
                "bg",
                "Async agent launched successfully.",
                json!({"isAsync": true, "status": "async_launched", "agentId": AGENT_ID}),
            ),
            notification(8, AGENT_ID, "failed", "boom"),
        ],
    );
    let s = t.snapshot();
    assert_eq!(s.events[0].at_ms, ms(8, 0));
    assert_eq!(
        s.events[0].kind,
        EventKind::TaskFailed {
            description: "Bg scan".into()
        }
    );
}

#[test]
fn events_record_finished_tools_with_their_summaries_newest_first() {
    let mut t = digest();
    fold(&mut t, &finished(0, "a", "Bash", json!({"command": "ls"})));
    fold(
        &mut t,
        &finished(1, "b", "Read", json!({"file_path": "/x/Cargo.toml"})),
    );
    let s = t.snapshot();
    assert_eq!(
        s.events
            .iter()
            .map(|e| (e.at_ms, e.kind.clone()))
            .collect::<Vec<_>>(),
        [
            (
                ms(1, 1),
                EventKind::Tool {
                    name: "Read".into(),
                    summary: Some("Cargo.toml".into())
                }
            ),
            (
                ms(0, 1),
                EventKind::Tool {
                    name: "Bash".into(),
                    summary: Some("ls".into())
                }
            ),
        ]
    );
}

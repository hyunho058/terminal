//! Acceptance tests for `specs/claude-background-working` R1–R4: a Claude Code turn that
//! ended (`end_turn`) while background work it started is still out reads
//! `AssistantBusy`, not `AssistantEndTurn`.
//!
//! Every case goes through the public boundary, `ai_monitor::activity::tail_state`, over a
//! log written to a temp file. The record helpers mirror the spec's Requirements preamble;
//! the shapes are the ones measured in its Research section.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ai_monitor::activity::{tail_state, LastMessage, PROMPT_SCAN_CAP};
use serde_json::{json, Value};

use LastMessage::{AssistantBusy, AssistantEndTurn, UserToolResult};

/// Realistic ids: `Bash` task ids are short base-36, agent ids 17 hex digits.
const B1: &str = "b0zk564sc";
const B2: &str = "bq7m2x9rt";
const A1: &str = "aee2b85e39714d85f";
const A2: &str = "a03fb09c51d2e7a64";

fn log(tag: &str, records: &[Value]) -> PathBuf {
    let mut body = String::new();
    for r in records {
        body.push_str(&r.to_string());
        body.push('\n');
    }
    write(tag, body.as_bytes())
}

fn write(tag: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aimon-tt1-claude-bg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let p = dir.join(format!("{tag}.jsonl"));
    std::fs::write(&p, bytes).expect("write");
    p
}

fn last(tag: &str, records: &[Value]) -> Option<LastMessage> {
    tail_state(&log(tag, records)).expect("tail").last
}

// ---- record helpers (the spec's Requirements preamble) ----

fn human(text: &str) -> Value {
    json!({"type": "user", "origin": {"kind": "human"},
           "message": {"role": "user", "content": text}})
}

fn tool() -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "tool_use",
           "content": [{"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {}}]}})
}

fn end() -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn",
           "content": [{"type": "text", "text": "Started it in the background."}]}})
}

/// A `user` tool-result record carrying `result` as its `toolUseResult`.
fn result_with(result: Value) -> Value {
    json!({"type": "user", "toolUseResult": result,
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": "toolu_01", "content": "ok"}]}})
}

fn result() -> Value {
    result_with(json!({"stdout": "ok"}))
}

fn bash(id: &str) -> Value {
    result_with(json!({"stdout": "", "backgroundTaskId": id}))
}

fn agent(id: &str) -> Value {
    result_with(json!({"isAsync": true, "status": "async_launched", "agentId": id}))
}

fn fork(id: &str) -> Value {
    result_with(json!({"background": true, "status": "forked", "agentId": id}))
}

fn resume(id: &str) -> Value {
    result_with(json!({"success": true, "resumedAgentId": id}))
}

fn notification(id: &str, status: &str) -> String {
    format!(
        "<task-notification>\n<task-id>{id}</task-id>\n<status>{status}</status>\n\
         <summary>Background command completed</summary>\n</task-notification>"
    )
}

fn enq(id: &str) -> Value {
    enq_status(id, "completed")
}

fn enq_status(id: &str, status: &str) -> Value {
    json!({"type": "queue-operation", "operation": "enqueue",
           "timestamp": "2026-10-22T05:34:00.000Z", "sessionId": "s",
           "content": notification(id, status)})
}

fn notif(id: &str) -> Value {
    json!({"type": "user", "origin": {"kind": "task-notification"}, "promptSource": "system",
           "message": {"role": "user", "content": notification(id, "completed")}})
}

/// `Notif(id)` with its text in a `text` block rather than a bare string, behind a block of
/// another type.
fn notif_array(id: &str) -> Value {
    json!({"type": "user", "origin": {"kind": "task-notification"}, "promptSource": "system",
           "message": {"role": "user", "content": [
               {"type": "image", "source": {"type": "base64", "data": "AAAA"}},
               {"type": "text", "text": notification(id, "completed")}]}})
}

fn queued(id: &str) -> Value {
    json!({"type": "attachment", "attachment": {"type": "queued_command",
           "commandMode": "task-notification", "prompt": notification(id, "completed")}})
}

fn stop(id: &str) -> Value {
    result_with(json!({"task_id": id, "message": format!("Successfully stopped task: {id}")}))
}

fn peer() -> Value {
    json!({"type": "user", "isMeta": true, "origin": {"kind": "peer"},
           "message": {"role": "user",
                       "content": "Another Claude session sent a message: progress update"}})
}

/// `[Human, …turn]`: every turn starts after a human prompt.
fn turn(records: Vec<Value>) -> Vec<Value> {
    let mut out = vec![human("run the slow thing")];
    out.extend(records);
    out
}

// ---- R1: pending work at an end_turn reads AssistantBusy, finished work as today ----

#[test]
fn r1_1_each_start_form_with_no_completion_is_pending() {
    for (what, start) in [
        ("bash", bash(B1)),
        ("agent", agent(A1)),
        ("fork", fork(A1)),
        ("resume", resume(A1)),
    ] {
        assert_eq!(
            last(&format!("r1_1-{what}"), &turn(vec![tool(), start, end()])),
            Some(AssistantBusy),
            "{what}"
        );
    }
}

#[test]
fn r1_2_each_completion_form_clears_it() {
    for (what, c) in [
        ("enq", enq(B1)),
        ("notif", notif(B1)),
        ("queued", queued(B1)),
        ("stop", stop(B1)),
        ("notif-array", notif_array(B1)),
    ] {
        let records = turn(vec![tool(), bash(B1), c, tool(), result(), end()]);
        assert_eq!(
            last(&format!("r1_2-{what}"), &records),
            Some(AssistantEndTurn),
            "{what}"
        );
    }
}

#[test]
fn r1_3_a_completion_written_after_the_end_turn_clears_it() {
    let records = turn(vec![tool(), bash(B1), end(), enq(B1)]);
    assert_eq!(last("r1_3", &records), Some(AssistantEndTurn));
}

#[test]
fn r1_4_any_one_unreported_start_keeps_it_pending() {
    let ids = [B1, B2, A1, A2];
    let starts = || vec![tool(), bash(B1), bash(B2), agent(A1), agent(A2)];
    for missing in ids {
        let mut records = starts();
        records.push(end());
        records.extend(ids.iter().filter(|i| **i != missing).map(|i| enq(i)));
        assert_eq!(
            last(&format!("r1_4-a-{missing}"), &turn(records)),
            Some(AssistantBusy),
            "{missing} unreported"
        );
    }
    let mut records = starts();
    records.push(end());
    records.extend(ids.iter().map(|i| enq(i)));
    assert_eq!(last("r1_4-b", &turn(records)), Some(AssistantEndTurn));
}

#[test]
fn r1_5_a_completion_for_a_different_id_does_not_clear_a_start() {
    let records = turn(vec![tool(), bash(B1), end(), enq(B2)]);
    assert_eq!(last("r1_5", &records), Some(AssistantBusy));
}

#[test]
fn r1_6_a_completion_clears_only_starts_older_than_it() {
    let base = || vec![agent(A1), notif(A1), tool(), resume(A1), end()];
    assert_eq!(last("r1_6-a", &turn(base())), Some(AssistantBusy));
    let mut done = base();
    done.push(enq(A1));
    assert_eq!(last("r1_6-b", &turn(done)), Some(AssistantEndTurn));
}

#[test]
fn r1_7_never_waiting_from_a_pending_turn_at_any_age() {
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("bash", turn(vec![tool(), bash(B1), end()])),
        ("agent", turn(vec![tool(), agent(A1), end()])),
        ("fork", turn(vec![tool(), fork(A1), end()])),
        ("resume", turn(vec![tool(), resume(A1), end()])),
        ("other-id", turn(vec![tool(), bash(B1), end(), enq(B2)])),
        (
            "resumed",
            turn(vec![agent(A1), notif(A1), tool(), resume(A1), end()]),
        ),
    ];
    for (what, records) in cases {
        let path = log(&format!("r1_7-{what}"), &records);
        let f = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        f.set_modified(hour_ago).expect("set mtime");
        drop(f);
        let state = tail_state(&path).expect("tail");
        assert_eq!(state.mtime, hour_ago, "{what}: mtime was set");
        assert_eq!(state.last, Some(AssistantBusy), "{what}");
    }
}

#[test]
fn r1_8_a_peer_message_completes_nothing() {
    let records = turn(vec![tool(), agent(A1), end(), peer(), end()]);
    assert_eq!(last("r1_8", &records), Some(AssistantBusy));
}

#[test]
fn r1_9_only_an_end_turn_triggers_the_walk() {
    assert_eq!(
        last("r1_9-tool", &turn(vec![tool(), bash(B1), tool()])),
        Some(AssistantBusy)
    );
    assert_eq!(
        last("r1_9-result", &turn(vec![tool(), bash(B1), result()])),
        Some(UserToolResult)
    );
}

// ---- R2: only the listed start shapes start anything ----

#[test]
fn r2_1_unlisted_background_looking_results_start_nothing() {
    for (what, r) in [
        ("monitor", json!({"taskId": "m1", "persistent": true})),
        (
            "foreground-agent",
            json!({"status": "completed", "content": [{"type": "text", "text": "done"}]}),
        ),
        (
            "async-completed",
            json!({"isAsync": true, "status": "completed", "agentId": A1}),
        ),
        (
            "background-not-forked",
            json!({"background": true, "status": "completed", "agentId": A1}),
        ),
        (
            "forked-not-background",
            json!({"status": "forked", "agentId": A1}),
        ),
        (
            "stdout-text",
            json!({"stdout": format!("Command running in background with ID: {B1}")}),
        ),
    ] {
        let records = turn(vec![tool(), result_with(r), end()]);
        assert_eq!(
            last(&format!("r2_1-{what}"), &records),
            Some(AssistantEndTurn),
            "{what}"
        );
    }
}

#[test]
fn r2_2_a_start_field_outside_a_user_record_starts_nothing() {
    let assistant = json!({"type": "assistant", "toolUseResult": {"backgroundTaskId": B1},
        "message": {"role": "assistant", "stop_reason": "tool_use", "content": []}});
    let blob = json!({"type": "api-request-blob", "toolUseResult": {"backgroundTaskId": B1},
        "message": {"role": "user", "content": "copy of the request"}});
    for (what, a) in [("assistant", assistant), ("blob", blob)] {
        let records = turn(vec![tool(), a, end()]);
        assert_eq!(
            last(&format!("r2_2-{what}"), &records),
            Some(AssistantEndTurn),
            "{what}"
        );
    }
}

#[test]
fn r2_3_a_non_string_id_starts_nothing() {
    for (what, r) in [
        ("numeric", json!({"stdout": "", "backgroundTaskId": 7})),
        (
            "null-agent",
            json!({"isAsync": true, "status": "async_launched", "agentId": null}),
        ),
    ] {
        let records = turn(vec![tool(), result_with(r), end()]);
        assert_eq!(
            last(&format!("r2_3-{what}"), &records),
            Some(AssistantEndTurn),
            "{what}"
        );
    }
}

// ---- R3: only the listed completion shapes complete anything ----

#[test]
fn r3_1_a_notification_in_the_wrong_record_type_completes_nothing() {
    let text = notification(B1, "completed");
    let cases = [
        (
            "dequeue",
            json!({"type": "queue-operation", "operation": "dequeue", "content": text}),
        ),
        (
            "remove",
            json!({"type": "queue-operation", "operation": "remove", "content": text}),
        ),
        (
            "prompt-attachment",
            json!({"type": "attachment", "attachment": {"type": "queued_command",
                   "commandMode": "prompt", "prompt": text}}),
        ),
    ];
    for (what, c) in cases {
        let records = turn(vec![tool(), bash(B1), end(), c]);
        assert_eq!(
            last(&format!("r3_1-{what}"), &records),
            Some(AssistantBusy),
            "{what}"
        );
    }
    let quoting = result_with(json!({"stdout": text}));
    let records = turn(vec![tool(), bash(B1), tool(), quoting, end()]);
    assert_eq!(last("r3_1-quoted", &records), Some(AssistantBusy));
}

#[test]
fn r3_2_the_notification_text_must_begin_the_content() {
    let c = json!({"type": "queue-operation", "operation": "enqueue",
                   "content": format!("see <task-notification>\n<task-id>{B1}</task-id>")});
    let records = turn(vec![tool(), bash(B1), end(), c]);
    assert_eq!(last("r3_2", &records), Some(AssistantBusy));
}

#[test]
fn r3_3_any_status_completes() {
    for status in ["failed", "killed"] {
        let records = turn(vec![tool(), bash(B1), end(), enq_status(B1, status)]);
        assert_eq!(
            last(&format!("r3_3-{status}"), &records),
            Some(AssistantEndTurn),
            "{status}"
        );
    }
}

// ---- R4: the walk's boundaries ----

fn r4_1_with(second: Value) -> Vec<Value> {
    vec![
        human("start the server"),
        tool(),
        bash(B1),
        end(),
        second,
        tool(),
        result(),
        end(),
    ]
}

#[test]
fn r4_1_starts_before_the_last_human_prompt_do_not_count() {
    assert_eq!(
        last("r4_1", &r4_1_with(human("now do this"))),
        Some(AssistantEndTurn)
    );
}

#[test]
fn r4_2_each_kind_of_human_prompt_is_a_boundary() {
    let cases = [
        (
            "slash-command",
            json!({"type": "user", "message": {"role": "user",
                   "content": "<command-name>/model</command-name>"}}),
        ),
        (
            "continuation",
            json!({"type": "user", "message": {"role": "user", "content":
                   "This session is being continued from a previous conversation."}}),
        ),
        (
            "array-text",
            json!({"type": "user", "origin": {"kind": "human"}, "message": {"role": "user",
                   "content": [{"type": "text", "text": "now do this"}]}}),
        ),
        (
            "image-only",
            json!({"type": "user", "message": {"role": "user", "content": [
                   {"type": "image", "source": {"type": "base64", "data": "AAAA"}}]}}),
        ),
    ];
    for (what, prompt) in cases {
        assert_eq!(
            last(&format!("r4_2-{what}"), &r4_1_with(prompt)),
            Some(AssistantEndTurn),
            "{what}"
        );
    }
}

#[test]
fn r4_3_notifications_peers_meta_records_and_tool_results_are_not_boundaries() {
    let cases = [
        ("notif-other", notif(B2)),
        ("peer", peer()),
        (
            "meta",
            json!({"type": "user", "isMeta": true, "message": {"role": "user",
                   "content": "Caveat: the messages below were generated by the user"}}),
        ),
        (
            "bare-tool-result",
            json!({"type": "user", "message": {"role": "user", "content": [
                   {"type": "tool_result", "tool_use_id": "toolu_01", "content": "ok"}]}}),
        ),
        (
            "blob",
            json!({"type": "api-request-blob", "message": {"role": "user",
                   "content": "a copy of the outgoing request"}}),
        ),
    ];
    for (what, x) in cases {
        let records = vec![
            human("start it"),
            tool(),
            bash(B1),
            x,
            tool(),
            result(),
            end(),
        ];
        assert_eq!(
            last(&format!("r4_3-{what}"), &records),
            Some(AssistantBusy),
            "{what}"
        );
    }
}

/// Foreground `tool`/`result` pairs totalling at least `bytes` bytes of log.
fn filler(bytes: usize) -> Vec<Value> {
    let pad = "x".repeat(4000);
    let r = result_with(json!({"stdout": pad}));
    let per = tool().to_string().len() + r.to_string().len() + 2;
    let mut out = Vec::new();
    for _ in 0..bytes.div_ceil(per) {
        out.push(tool());
        out.push(r.clone());
    }
    out
}

#[test]
fn r4_4_a_pending_start_found_before_the_windows_edge_reads_busy() {
    let far = filler(PROMPT_SCAN_CAP as usize + 64 * 1024);
    let mut with = vec![human("long ago")];
    with.extend(far.clone());
    with.extend([tool(), bash(B1), end()]);
    assert_eq!(last("r4_4-with", &with), Some(AssistantBusy));

    let mut without = vec![human("long ago")];
    without.extend(far);
    without.extend([tool(), result(), end()]);
    assert_eq!(last("r4_4-without", &without), Some(AssistantEndTurn));
}

#[test]
fn r4_5_a_start_beyond_the_window_is_not_seen() {
    let mut records = vec![human("long ago"), tool(), bash(B1)];
    records.extend(filler(PROMPT_SCAN_CAP as usize + 64 * 1024));
    records.push(end());
    assert_eq!(last("r4_5", &records), Some(AssistantEndTurn));
}

#[test]
fn r4_6_a_torn_newest_line_gives_no_signal() {
    let mut body = String::new();
    for r in turn(vec![tool(), bash(B1), end()]) {
        body.push_str(&r.to_string());
        body.push('\n');
    }
    let torn = enq(B1).to_string();
    body.push_str(&torn[..torn.len() / 2]);
    let path = write("r4_6", body.as_bytes());
    assert_eq!(tail_state(&path).expect("tail").last, None);
}

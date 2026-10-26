//! Step builders shared by the `tt*_agy_*` acceptance tests (`specs/agy-background-working`).
//!
//! Every placeholder id is grammar-valid (the spec's Requirements preamble): a test that used
//! a literal `T` or `S` would be rejected by the id shape and leave the anchor and status
//! guards untested.

#![allow(dead_code)]

use std::path::PathBuf;

use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;
use serde_json::{json, Value};

pub const T: &str = "2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451";
/// `T` but for its task number.
pub const T_OTHER: &str = "2091e717-a2d0-4967-9d63-96b8a5a868b3/task-452";
pub const T2: &str = "7640f000-2222-4333-8444-b55555555555/task-7";
pub const S: &str = "ed14342c-0000-4000-8000-00000000beef";
pub const S2: &str = "ed14342c-1111-4222-8333-a44444444444";

/// A per-test transcript path under the temp dir.
pub fn path(file: &str, tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ai-monitor-agy-pending-{file}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir.join(format!("{tag}.jsonl"))
}

/// `steps` as `agy` appends them: one JSON object per line, `step_index` numbered in order.
pub fn bytes(steps: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let mut s = s.clone();
        s["step_index"] = json!(i);
        out.extend_from_slice(s.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

pub fn write(file: &str, tag: &str, steps: &[Value]) -> PathBuf {
    let p = path(file, tag);
    std::fs::write(&p, bytes(steps)).expect("write transcript");
    p
}

/// `tail_state(..).last` for `steps` written to disk.
pub fn last_of(file: &str, tag: &str, steps: &[Value]) -> Option<LastMessage> {
    antigravity::tail_state(&write(file, tag, steps))
        .expect("a readable transcript has a state")
        .last
}

pub fn step(kind: &str) -> Value {
    json!({"source": "MODEL", "type": kind, "status": "DONE",
           "created_at": "2026-10-19T00:00:00Z"})
}

pub fn with_content(mut s: Value, content: &str) -> Value {
    s["content"] = json!(content);
    s
}

pub fn running(content: &str) -> Value {
    let mut s = with_content(step("GENERIC"), content);
    s["status"] = json!("RUNNING");
    s
}

pub fn user_input() -> Value {
    let mut s = with_content(
        step("USER_INPUT"),
        "<USER_REQUEST>\ndo the thing\n</USER_REQUEST>",
    );
    s["source"] = json!("USER_EXPLICIT");
    s
}

pub fn dispatch(name: &str) -> Value {
    let mut s = step("PLANNER_RESPONSE");
    s["tool_calls"] = json!([{"name": name, "args": {}}]);
    s
}

pub fn reply() -> Value {
    with_content(
        step("PLANNER_RESPONSE"),
        "Tests are executing. Awaiting completion notification.",
    )
}

/// The start text of a background task, id `id`, followed by `after`.
pub fn start_text(id: &str, after: &str) -> String {
    format!(
        "Created At: 2026-10-19T00:04:24-07:00\nTool is running as a background task with \
         task id: {id}{after}"
    )
}

/// Start(T): a `RUNNING` step whose content begins with the task-start header.
pub fn start(id: &str) -> Value {
    running(&start_text(
        id,
        "\nTask Description: cargo test --workspace",
    ))
}

/// The header of an `invoke_subagent` result naming `ids`.
pub fn sub_text(ids: &[&str]) -> String {
    let list: Vec<String> = ids
        .iter()
        .map(|id| {
            format!(
                "{{\n  \"conversationId\": \"{id}\",\n  \"logAbsoluteUri\": \"file:///tmp\"\n}}"
            )
        })
        .collect();
    format!(
        "Created At: 2026-10-19T00:04:24-07:00\nCompleted At: 2026-10-19T00:04:25-07:00\n\
         Created the following subagents:\n{}",
        list.join("\n")
    )
}

/// Sub(S…): a `GENERIC` step whose content begins with the subagent header.
pub fn sub(ids: &[&str]) -> Value {
    with_content(step("GENERIC"), &sub_text(ids))
}

/// The header of an Antigravity 1.4.7 single `invoke_subagent` result naming `id`.
pub fn sub_text_singular(id: &str) -> String {
    format!(
        "Created At: 2026-10-24T11:38:22-07:00\nCompleted At: 2026-10-24T11:38:22-07:00\n\
         Created the following subagent:\n{{\n  \"conversationId\": \"{id}\",\n  \"logAbsoluteUri\": \"file:///tmp\"\n}}"
    )
}

/// Sub singular in 1.4.7: a `GENERIC` step whose content begins with singular subagent header.
pub fn sub_singular(id: &str) -> Value {
    with_content(step("GENERIC"), &sub_text_singular(id))
}

/// Antigravity 1.4.7 command output text: "Command exited with code {code}."
pub fn cmd_output_147(code: i32) -> String {
    format!("Created At: 2026-10-24T11:26:44-07:00\nCompleted At: 2026-10-24T11:26:44-07:00\nCommand exited with code {code}.\n\nCommand output:\n")
}

/// Planner response step carrying Antigravity 1.4.7 token usage.
pub fn planner_with_tokens(
    tool_calls: Option<Vec<Value>>,
    input_tokens: u64,
    cache_read_tokens: u64,
    output_tokens: u64,
) -> Value {
    let mut s = step("PLANNER_RESPONSE");
    s["input_tokens"] = json!(input_tokens);
    s["cache_read_tokens"] = json!(cache_read_tokens);
    s["output_tokens"] = json!(output_tokens);
    if let Some(calls) = tool_calls {
        s["tool_calls"] = json!(calls);
    }
    s
}

/// Notice(id, body): a `SYSTEM_MESSAGE` carrying the `[Message] … sender=<id>` header.
pub fn notice(id: &str, body: &str) -> Value {
    with_content(
        step("SYSTEM_MESSAGE"),
        &format!(
            "The following is a <SYSTEM_MESSAGE> not actually sent by the user.\n\n\
             <SYSTEM_MESSAGE>\n[Message] timestamp=2026-10-19T00:00:00Z sender={id} \
             priority=MESSAGE_PRIORITY_HIGH content={body}\n</SYSTEM_MESSAGE>"
        ),
    )
}

/// The `Task id "<id>" finished with result:` body.
pub fn finished(id: &str) -> String {
    format!("Task id \"{id}\" finished with result:\n\nThe command exited with code 0.")
}

/// The R1.1 turn: `[dispatch run_command, Start(T), dispatch manage_task, GENERIC DONE, reply]`.
pub fn r1_1_turn() -> Vec<Value> {
    vec![
        user_input(),
        dispatch("run_command"),
        start(T),
        dispatch("manage_task"),
        with_content(step("GENERIC"), &format!("Task: {T}\nStatus: RUNNING")),
        reply(),
    ]
}

//! `specs/claude-background-working` R5.4: the background-work rule re-measured against the
//! local `~/.claude/projects` corpus. `#[ignore]`d — it reads the developer's own logs, which
//! are private and move by the hour — and it passes without asserting when they are absent.
//!
//! ```text
//! cargo test -p ai-monitor --test corpus_claude_pending -- --ignored --nocapture
//! ```
//!
//! For every `end_turn` that is the last record of its message, the log prefix ending there is
//! classified by `tail_state` and by a reference walk written here, independently of
//! `activity.rs`, from the spec's D2–D4. The two must agree: that is the gate. The outcome table
//! (what came next, against whether work was pending) is printed to be recorded, not asserted,
//! because the corpus moves.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use ai_monitor::activity::{tail_state, LastMessage, PROMPT_SCAN_CAP, TAIL_CAP};
use serde_json::Value;

fn s<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer).and_then(Value::as_str)
}

fn is_true(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool) == Some(true)
}

/// D3's message text, spelled out separately from `activity.rs`.
fn text_of(v: &Value) -> Option<&str> {
    match v.pointer("/message/content")? {
        Value::String(t) => Some(t),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| s(b, "/type") == Some("text"))
            .and_then(|b| s(b, "/text")),
        _ => None,
    }
}

fn task_id(text: &str) -> Option<String> {
    let t = text.trim_start();
    if !t.starts_with("<task-notification>") {
        return None;
    }
    let open = t.find("<task-id>")? + "<task-id>".len();
    let close = t[open..].find("</task-id>")?;
    Some(t[open..open + close].to_string())
}

fn human(v: &Value) -> bool {
    if s(v, "/type") != Some("user") || v.get("toolUseResult").is_some() || is_true(v, "isMeta") {
        return false;
    }
    if matches!(
        s(v, "/origin/kind"),
        Some("task-notification") | Some("peer")
    ) {
        return false;
    }
    if text_of(v).and_then(task_id).is_some() {
        return false;
    }
    let blocks = v.pointer("/message/content").and_then(Value::as_array);
    !blocks.is_some_and(|b| b.iter().any(|b| s(b, "/type") == Some("tool_result")))
}

fn completion(v: &Value) -> Option<String> {
    match s(v, "/type")? {
        "queue-operation" if s(v, "/operation") == Some("enqueue") => task_id(s(v, "/content")?),
        "attachment"
            if s(v, "/attachment/type") == Some("queued_command")
                && s(v, "/attachment/commandMode") == Some("task-notification") =>
        {
            task_id(s(v, "/attachment/prompt")?)
        }
        "user" => {
            if s(v, "/origin/kind") == Some("task-notification") {
                task_id(text_of(v)?)
            } else {
                s(v, "/toolUseResult/task_id").map(str::to_string)
            }
        }
        _ => None,
    }
}

fn starts(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if s(v, "/type") != Some("user") {
        return out;
    }
    let Some(r) = v.get("toolUseResult") else {
        return out;
    };
    if let Some(id) = s(r, "/backgroundTaskId") {
        out.push(id.to_string());
    }
    let launched = (is_true(r, "isAsync") && s(r, "/status") == Some("async_launched"))
        || (is_true(r, "background") && s(r, "/status") == Some("forked"));
    if launched {
        if let Some(id) = s(r, "/agentId") {
            out.push(id.to_string());
        }
    }
    for k in ["/resumedAgentId", "/revivedAgentId"] {
        if let Some(id) = s(r, k) {
            out.push(id.to_string());
        }
    }
    out
}

/// D4 over `window`: newest first, stop at a human prompt or an unreadable line.
fn reference_pending(window: &[u8]) -> bool {
    let text = String::from_utf8_lossy(window);
    let mut done: HashSet<String> = HashSet::new();
    for line in text.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return false;
        };
        if human(&v) {
            return false;
        }
        if let Some(id) = completion(&v) {
            done.insert(id);
        }
        if starts(&v).iter().any(|id| !done.contains(id)) {
            return true;
        }
    }
    false
}

fn outcome(next: Option<&Value>) -> &'static str {
    match next {
        None => "end of file",
        Some(v) if s(v, "/origin/kind") == Some("task-notification") => "task-notification",
        Some(v) if s(v, "/origin/kind") == Some("peer") => "peer",
        Some(v) if human(v) => "human prompt",
        Some(_) => "other",
    }
}

fn corpus() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let root = PathBuf::from(home).join(".claude/projects");
    let Ok(dirs) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            if f.path().extension().is_some_and(|e| e == "jsonl") {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}

#[test]
#[ignore = "reads the local ~/.claude/projects corpus"]
fn r5_4_the_rule_holds_on_the_real_corpus() {
    let logs = corpus();
    if logs.is_empty() {
        eprintln!("no ~/.claude/projects corpus; nothing to measure");
        return;
    }
    let scratch = std::env::temp_dir().join(format!("aimon-corpus-{}.jsonl", std::process::id()));
    // (outcome, pending) -> turns
    let mut table: BTreeMap<(&str, bool), usize> = BTreeMap::new();
    let (mut turns, mut skipped, mut mismatches) = (0usize, 0usize, Vec::new());

    for log in &logs {
        let Ok(bytes) = std::fs::read(log) else {
            continue;
        };
        // (byte offset just past the line, parsed record) for every complete line.
        let mut records: Vec<(usize, Value)> = Vec::new();
        let mut at = 0;
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            at += line.len();
            if let Ok(v) = serde_json::from_slice::<Value>(line) {
                records.push((at, v));
            }
        }
        for (i, (end, v)) in records.iter().enumerate() {
            if s(v, "/type") != Some("assistant")
                || s(v, "/message/stop_reason") != Some("end_turn")
            {
                continue;
            }
            let id = s(v, "/message/id");
            let later_same_message = records[i + 1..].iter().any(|(_, w)| {
                s(w, "/type") == Some("assistant") && id.is_some() && s(w, "/message/id") == id
            });
            if later_same_message {
                continue;
            }
            turns += 1;
            let line_start = if i == 0 { 0 } else { records[i - 1].0 };
            if (*end - line_start) as u64 >= TAIL_CAP {
                // The end_turn itself is wider than the tail: today's reader cannot see it
                // either, so there is no end_turn to hold. Counted, not compared.
                skipped += 1;
                continue;
            }
            // tail_state reads only the final PROMPT_SCAN_CAP bytes, so those bytes alone are
            // the same input as the whole prefix.
            let from = end.saturating_sub(PROMPT_SCAN_CAP as usize);
            let window = &bytes[from..*end];
            std::fs::write(&scratch, window).expect("write scratch");
            let got = tail_state(&scratch).and_then(|t| t.last);
            let pending = reference_pending(window);
            let expected = Some(if pending {
                LastMessage::AssistantBusy
            } else {
                LastMessage::AssistantEndTurn
            });
            if got != expected {
                mismatches.push(format!(
                    "{}:{}: got {got:?}, reference {expected:?}",
                    log.display(),
                    i
                ));
            }
            let next = records[i + 1..]
                .iter()
                .map(|(_, w)| w)
                .find(|w| matches!(s(w, "/type"), Some("user" | "assistant")));
            *table.entry((outcome(next), pending)).or_default() += 1;
        }
    }
    let _ = std::fs::remove_file(&scratch);

    println!(
        "{} logs, {turns} end_turn turns ({skipped} wider than TAIL_CAP, not compared)",
        logs.len()
    );
    println!(
        "{:<20} {:>8} {:>16}",
        "next record", "pending", "nothing pending"
    );
    for o in [
        "task-notification",
        "peer",
        "human prompt",
        "end of file",
        "other",
    ] {
        println!(
            "{o:<20} {:>8} {:>16}",
            table.get(&(o, true)).copied().unwrap_or(0),
            table.get(&(o, false)).copied().unwrap_or(0)
        );
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

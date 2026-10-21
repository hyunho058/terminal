//! Corpus re-measurement for `specs/agy-background-working` (T3, Confirmed Goal bullet 4).
//!
//! Reads the author's real Antigravity transcripts, so it is `#[ignore]`d and skips when the
//! directory is absent. For every tool-less reply after a `USER_INPUT`, it classifies the
//! transcript prefix ending at that reply through `tail_state` and tallies the result against
//! what happened next. Run with
//! `cargo test -p ai-monitor --test corpus_agy_pending -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;
use serde_json::Value;

const BOOKKEEPING: &[&str] = &[
    "CHECKPOINT",
    "SYSTEM_MESSAGE",
    "CONVERSATION_HISTORY",
    "ERROR_MESSAGE",
];

fn kind(step: &Value) -> &str {
    step.get("type").and_then(Value::as_str).unwrap_or("")
}

fn is_reply(step: &Value) -> bool {
    kind(step) == "PLANNER_RESPONSE"
        && match step.get("tool_calls") {
            None => true,
            Some(Value::Array(calls)) => calls.is_empty(),
            Some(_) => false,
        }
}

/// The turn did background work in the plain sense: a `RUNNING` step or a subagent header.
fn is_background(step: &Value) -> bool {
    let content = step.get("content").and_then(Value::as_str).unwrap_or("");
    step.get("status").and_then(Value::as_str) == Some("RUNNING")
        || content.contains("Created the following subagents:")
}

fn transcripts() -> Option<Vec<PathBuf>> {
    let brain = PathBuf::from(std::env::var_os("HOME")?).join(".gemini/antigravity-cli/brain");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(brain).ok()?.flatten() {
        let t = entry.path().join(".system_generated/logs/transcript.jsonl");
        if t.is_file() {
            out.push(t);
        }
    }
    Some(out)
}

#[test]
#[ignore = "reads the real ~/.gemini/antigravity-cli corpus; run with --ignored"]
fn the_pending_rule_holds_on_the_real_corpus() {
    let Some(files) = transcripts() else {
        eprintln!("no antigravity corpus: skipped");
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("ai-monitor-corpus-agy-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("mkdir");
    let probe = scratch.join("prefix.jsonl");
    let mut table: BTreeMap<(bool, String, &str), usize> = BTreeMap::new();
    let mut waiting_after_background = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let steps: Vec<Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap_or(Value::Null))
            .collect();
        let mut last_user = None;
        for (i, step) in steps.iter().enumerate() {
            if kind(step) == "USER_INPUT" {
                last_user = Some(i);
            }
            let Some(user) = last_user else { continue };
            if !is_reply(step) {
                continue;
            }
            let background = steps[user + 1..=i].iter().any(is_background);
            let outcome = steps[i + 1..]
                .iter()
                .map(kind)
                .find(|k| !BOOKKEEPING.contains(k))
                .map_or("eof", |k| {
                    if k == "USER_INPUT" {
                        "ended"
                    } else {
                        "continued"
                    }
                });
            let prefix: String = lines[..=i].iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(&probe, prefix).expect("write prefix");
            let last = antigravity::tail_state(&probe).and_then(|s| s.last);
            if background
                && matches!(
                    last,
                    Some(LastMessage::AssistantEndTurn | LastMessage::AssistantAsksUser)
                )
            {
                waiting_after_background.push(format!("{}:{i}", file.display()));
            }
            *table
                .entry((background, format!("{last:?}"), outcome))
                .or_default() += 1;
        }
    }
    println!("transcripts: {}", files.len());
    for ((background, last, outcome), n) in &table {
        let bg = if *background { "bg" } else { "fg" };
        println!("{n:5}  {bg:2}  {last:28}  {outcome}");
    }
    assert!(
        waiting_after_background.is_empty(),
        "background turns read Waiting: {waiting_after_background:?}"
    );
}

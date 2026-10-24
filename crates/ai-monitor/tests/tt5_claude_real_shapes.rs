//! `specs/claude-background-working` R5.1: the background-work rule over records cut from a
//! real Claude Code 2.1.300 session log, not hand-built ones.
//!
//! The fixture is one turn: a human prompt (an accepted suggestion), a `Bash` call with
//! `run_in_background`, its launch result (`toolUseResult.backgroundTaskId`), an
//! `api-request-blob` (role `system`), the `end_turn`, and then, two hours later, the
//! `queue-operation` enqueue/dequeue pair and the `task-notification` user record that woke
//! Claude. Home paths, the session `cwd`, `gitBranch` and `serverClassifierContext` are
//! scrubbed; every other field is as written.

use std::path::PathBuf;

use ai_monitor::activity::{tail_state, LastMessage};

const FIXTURE: &str = include_str!("fixtures/tt5_claude_background_pending.jsonl");

/// The fixture's lines up to and including the first one containing `marker`, as a log file.
fn prefix_through(tag: &str, marker: &str) -> PathBuf {
    let lines: Vec<&str> = FIXTURE.lines().collect();
    let end = lines
        .iter()
        .position(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("fixture has a line containing {marker}"));
    let mut body = lines[..=end].join("\n");
    body.push('\n');
    let dir = std::env::temp_dir().join(format!("aimon-tt5-claude-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let p = dir.join(format!("{tag}.jsonl"));
    std::fs::write(&p, body).expect("write");
    p
}

#[test]
fn r5_1_a_real_background_turn_reads_busy_then_clears() {
    let ended = prefix_through("ended", r#""stop_reason":"end_turn""#);
    assert_eq!(
        tail_state(&ended).expect("tail").last,
        Some(LastMessage::AssistantBusy),
        "the background Bash has not reported back at the end_turn"
    );

    let notified = prefix_through("enqueued", r#""operation":"enqueue""#);
    assert_eq!(
        tail_state(&notified).expect("tail").last,
        Some(LastMessage::AssistantEndTurn),
        "the enqueue completes it: today's result"
    );
}

#[test]
fn the_fixture_is_scrubbed() {
    for needle in ["/Users/", "serverClassifierContext", "\"cwd\""] {
        assert!(!FIXTURE.contains(needle), "fixture still holds {needle}");
    }
}

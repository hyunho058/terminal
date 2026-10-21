//! TT4: R4 of `specs/agy-background-working`. Real step shapes.
//!
//! `tt4_antigravity_background_pending.jsonl` is steps 447–455 of the transcript the bug was
//! observed on (`brain/2091e717-…`, 2026-10-19): the user input, a `git status`, a
//! backgrounded `cargo test --workspace`, a `manage_task` poll, the "Awaiting completion
//! notification." reply, and then the task's completion notice. Long output is cut to a few
//! lines and local paths are replaced with `/tmp`.

mod agy_steps;

use agy_steps::path;
use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;

const FILE: &str = "tt4";
const FIXTURE: &str = include_str!("fixtures/tt4_antigravity_background_pending.jsonl");

fn last_of_lines(tag: &str, text: &str, lines: usize) -> Option<LastMessage> {
    let prefix: String = text.lines().take(lines).map(|l| format!("{l}\n")).collect();
    let p = path(FILE, tag);
    std::fs::write(&p, prefix).expect("write");
    antigravity::tail_state(&p).expect("readable").last
}

#[test]
fn r4_1_the_observed_transcript_reads_busy_then_clears() {
    assert_eq!(FIXTURE.lines().count(), 9, "fixture shape");
    assert!(FIXTURE
        .lines()
        .nth(7)
        .is_some_and(|l| l.contains("Awaiting completion notification.")));
    assert_eq!(
        last_of_lines("r4-1-pending", FIXTURE, 8),
        Some(LastMessage::AssistantBusy)
    );
    assert_eq!(last_of_lines("r4-1-finished", FIXTURE, 9), None);
}

#[test]
fn r4_2_the_existing_fixtures_reply_after_an_unreported_subagent_reads_busy() {
    let fixture = include_str!("fixtures/antigravity_transcript.jsonl");
    assert!(fixture
        .lines()
        .nth(12)
        .is_some_and(|l| l.contains("\"step_index\": 12")));
    assert_eq!(
        last_of_lines("r4-2", fixture, 13),
        Some(LastMessage::AssistantBusy)
    );
}

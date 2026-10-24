//! TT3: R3 of `specs/agy-background-working`. The walk's boundaries (D4). R3.5, the byte
//! budget, is checked by the unit tests in `tail.rs`, where the counting byte source lives.

mod agy_steps;

use agy_steps::*;
use ai_monitor::activity::{LastMessage, PROMPT_SCAN_CAP};
use serde_json::{json, Value};

const FILE: &str = "tt3";

#[test]
fn r3_1_starts_before_the_last_user_input_do_not_count() {
    let steps = [
        user_input(),
        start(T),
        reply(),
        user_input(),
        dispatch("view_file"),
        step("VIEW_FILE"),
        reply(),
    ];
    assert_eq!(
        last_of(FILE, "r3-1", &steps),
        Some(LastMessage::AssistantEndTurn)
    );
}

/// Foreground work totalling more than `bytes`, so a `USER_INPUT` before it falls outside
/// the back-scan window.
fn padding(bytes: u64) -> Vec<Value> {
    let mut result = step("VIEW_FILE");
    result["content"] = json!(format!("Created At: …\n{}", "x".repeat(4000)));
    let pairs = bytes / 4000 + 1;
    (0..pairs)
        .flat_map(|_| [dispatch("view_file"), result.clone()])
        .collect()
}

#[test]
fn r3_2_a_pending_start_inside_the_window_reads_busy_past_its_edge() {
    let mut steps = vec![user_input()];
    steps.extend(padding(PROMPT_SCAN_CAP));
    let without = {
        let mut s = steps.clone();
        s.push(reply());
        s
    };
    steps.extend([dispatch("run_command"), start(T), reply()]);
    let with_start = bytes(&steps);
    let first_line = with_start.iter().position(|b| *b == b'\n').expect("a line");
    assert!(
        (with_start.len() - first_line) as u64 > PROMPT_SCAN_CAP,
        "the USER_INPUT lies outside the window"
    );
    assert_eq!(
        last_of(FILE, "r3-2", &steps),
        Some(LastMessage::AssistantBusy)
    );
    assert_eq!(last_of(FILE, "r3-2-without", &without), None);
}

#[test]
fn r3_3_a_notice_after_the_reply_clears_the_start() {
    let mut steps = r1_1_turn();
    steps.extend([notice(T, &finished(T)), step("CHECKPOINT")]);
    assert_eq!(last_of(FILE, "r3-3", &steps), None);
}

#[test]
fn r3_4_another_reply_or_bookkeeping_does_not_end_the_hold() {
    let mut steps = r1_1_turn();
    steps.extend([reply(), step("CHECKPOINT")]);
    assert_eq!(
        last_of(FILE, "r3-4", &steps),
        Some(LastMessage::AssistantBusy)
    );
}

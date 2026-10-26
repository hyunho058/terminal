//! TT1: Acceptance tests for requirement R1 of `specs/agy-1.4.7-telemetry`.
//! Subagent Start Recognition in Tail Scanning — D1.

mod agy_steps;

use agy_steps::*;
use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;

const FILE: &str = "tt1_agy_147_tail";

#[test]
fn r1_1_singular_subagent_header() {
    let turn = vec![
        user_input(),
        dispatch("invoke_subagent"),
        sub_singular(S),
        reply(),
    ];
    let path = write(FILE, "r1_1_singular", &turn);
    let state = antigravity::tail_state(&path).expect("readable transcript has tail_state");
    assert_eq!(state.last, Some(LastMessage::AssistantBusy));
}

#[test]
fn r1_2_plural_subagent_header() {
    let turn = vec![
        user_input(),
        dispatch("invoke_subagent"),
        sub(&[S]),
        reply(),
    ];
    let path = write(FILE, "r1_2_plural", &turn);
    let state = antigravity::tail_state(&path).expect("readable transcript has tail_state");
    assert_eq!(state.last, Some(LastMessage::AssistantBusy));
}

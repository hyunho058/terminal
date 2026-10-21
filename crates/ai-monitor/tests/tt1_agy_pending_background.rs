//! TT1: R1 of `specs/agy-background-working`. Pending background work reads
//! `AssistantBusy`; work that has reported back reads as it did before.

mod agy_steps;

use std::fs::File;
use std::time::{Duration, SystemTime};

use agy_steps::*;
use ai_monitor::activity::LastMessage;
use ai_monitor::antigravity;
use serde_json::Value;

const FILE: &str = "tt1";

fn with(mut turn: Vec<Value>, more: Vec<Value>) -> Vec<Value> {
    turn.extend(more);
    turn
}

#[test]
fn r1_1_a_task_with_no_notice_is_pending() {
    assert_eq!(
        last_of(FILE, "r1-1", &r1_1_turn()),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r1_2_a_tasks_notice_by_either_recogniser_clears_it() {
    let canceled = with_content(
        step("SYSTEM_MESSAGE"),
        &format!("<SYSTEM_MESSAGE>\nTask id \"{T}\" was canceled with result:\n</SYSTEM_MESSAGE>"),
    );
    for (what, done) in [
        ("both recognisers", notice(T, &finished(T))),
        (
            "sender header only (a timer)",
            notice(T, "Check if cargo test completed"),
        ),
        ("Task id text only", canceled),
    ] {
        let steps = with(r1_1_turn(), vec![done]);
        assert_eq!(last_of(FILE, "r1-2", &steps), None, "{what}");
    }
}

#[test]
fn r1_3_a_subagent_with_no_report_is_pending_and_its_report_clears_it() {
    let turn = vec![
        user_input(),
        dispatch("invoke_subagent"),
        sub(&[S]),
        reply(),
    ];
    assert_eq!(
        last_of(FILE, "r1-3-pending", &turn),
        Some(LastMessage::AssistantBusy)
    );
    let reported = with(turn, vec![notice(S, "TT4 — R4 tests written")]);
    assert_eq!(last_of(FILE, "r1-3-reported", &reported), None);
}

fn r1_4_turn(notices: &[&str]) -> Vec<Value> {
    let mut steps = vec![
        user_input(),
        dispatch("run_command"),
        start(T),
        dispatch("run_command"),
        start(T2),
        dispatch("invoke_subagent"),
        sub(&[S, S2]),
    ];
    steps.extend(notices.iter().map(|id| notice(id, "done")));
    steps.push(reply());
    steps
}

#[test]
fn r1_4_any_one_unreported_start_keeps_it_pending() {
    assert_eq!(
        last_of(FILE, "r1-4-a", &r1_4_turn(&[T, T2, S])),
        Some(LastMessage::AssistantBusy)
    );
    // Each of the four, left out alone, is enough.
    for missing in [T, T2, S, S2] {
        let others: Vec<&str> = [T, T2, S, S2]
            .into_iter()
            .filter(|id| *id != missing)
            .collect();
        assert_eq!(
            last_of(FILE, "r1-4-each", &r1_4_turn(&others)),
            Some(LastMessage::AssistantBusy),
            "{missing} unreported"
        );
    }
}

#[test]
fn r1_4_every_start_reported_is_not_pending() {
    assert_eq!(last_of(FILE, "r1-4-b", &r1_4_turn(&[T, T2, S, S2])), None);
}

#[test]
fn r1_5_a_notice_for_a_different_id_does_not_clear_a_start() {
    let steps = with(r1_1_turn(), vec![notice(T_OTHER, &finished(T_OTHER))]);
    assert_eq!(
        last_of(FILE, "r1-5", &steps),
        Some(LastMessage::AssistantBusy)
    );
}

fn r1_6_turn() -> Vec<Value> {
    let mut steps = r1_1_turn();
    let reply = steps.pop().expect("the reply");
    steps.extend([dispatch("frobnicate"), step("SOME_NEW_KIND"), reply]);
    steps
}

#[test]
fn r1_6_pending_outranks_an_unlisted_tool_or_kind() {
    assert_eq!(
        last_of(FILE, "r1-6", &r1_6_turn()),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r1_7_never_waiting_at_any_age() {
    let busy_cases: Vec<(&str, Vec<Value>)> = vec![
        ("r1.1", r1_1_turn()),
        (
            "r1.3",
            vec![
                user_input(),
                dispatch("invoke_subagent"),
                sub(&[S]),
                reply(),
            ],
        ),
        ("r1.4a", r1_4_turn(&[T, T2, S])),
        (
            "r1.5",
            with(r1_1_turn(), vec![notice(T_OTHER, &finished(T_OTHER))]),
        ),
        ("r1.6", r1_6_turn()),
    ];
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
    for (what, steps) in busy_cases {
        let path = write(FILE, &format!("r1-7-{what}"), &steps);
        File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(an_hour_ago))
            .expect("set mtime");
        let state = antigravity::tail_state(&path).expect("readable");
        assert!(state.mtime <= an_hour_ago, "{what}: mtime was set");
        assert_eq!(state.last, Some(LastMessage::AssistantBusy), "{what}");
    }
}

#[test]
fn r1_8_a_start_seen_twice_is_one_pending_id() {
    let mut steps = r1_1_turn();
    steps.insert(3, start(T));
    steps.push(notice(T, &finished(T)));
    assert_eq!(last_of(FILE, "r1-8", &steps), None);
}

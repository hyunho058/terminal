//! TT2: R2 of `specs/agy-background-working`. Only a start step's own header starts
//! anything (D2).

mod agy_steps;

use agy_steps::*;
use ai_monitor::activity::LastMessage;
use serde_json::Value;

const FILE: &str = "tt2";

fn turn(middle: Value) -> Vec<Value> {
    vec![user_input(), dispatch("run_command"), middle, reply()]
}

/// A command result whose output quotes both start headers.
fn quoting_result() -> Value {
    with_content(
        step("GENERIC"),
        &format!(
            "Created At: …\nCompleted At: …\n\nThe command exited with code 0.\nOutput:\n{}\n{}\n",
            start_text(T, ""),
            sub_text(&[S]),
        ),
    )
}

#[test]
fn r2_1_quoted_start_text_starts_nothing() {
    assert_eq!(
        last_of(FILE, "r2-1", &turn(quoting_result())),
        Some(LastMessage::AssistantEndTurn)
    );
}

#[test]
fn r2_1_control_the_same_task_id_at_a_running_steps_start_is_pending() {
    assert_eq!(
        last_of(FILE, "r2-1-control-task", &turn(start(T))),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r2_1_control_a_result_that_begins_with_the_subagent_header_is_pending() {
    assert_eq!(
        last_of(FILE, "r2-1-control-sub", &turn(sub(&[S]))),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r2_2_a_running_step_quoting_the_marker_mid_content_starts_nothing() {
    let quoted = running(&format!(
        "Created At: …\nsome output\nTool is running as a background task with task id: {T}"
    ));
    assert_eq!(last_of(FILE, "r2-2", &turn(quoted)), None);
}

#[test]
fn r2_2_control_the_marker_right_after_created_at_is_pending() {
    assert_eq!(
        last_of(FILE, "r2-2-control", &turn(start(T))),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r2_2_a_done_step_with_the_start_header_starts_nothing() {
    // D2's status guard on its own: the header is anchored, the status is not RUNNING.
    let done = with_content(step("GENERIC"), &start_text(T, "\nTask Description: x"));
    assert_eq!(
        last_of(FILE, "r2-2-done", &turn(done)),
        Some(LastMessage::AssistantEndTurn)
    );
}

#[test]
fn r2_3_a_running_step_without_the_marker_starts_nothing() {
    assert_eq!(last_of(FILE, "r2-3", &turn(running("Created At: …"))), None);
}

#[test]
fn r2_4_the_id_is_cut_at_the_last_digit() {
    for (what, after) in [("a period", "."), ("a comma", ","), ("end of content", "")] {
        let steps = vec![
            user_input(),
            dispatch("run_command"),
            running(&start_text(T, after)),
            reply(),
            notice(T, &finished(T)),
        ];
        assert_eq!(last_of(FILE, "r2-4", &steps), None, "{what}");
    }
}

#[test]
fn r2_5_text_without_the_id_shape_starts_nothing() {
    for (what, id) in [
        ("not an id", "not-a-task-id"),
        (
            "no task number",
            "2091e717-a2d0-4967-9d63-96b8a5a868b3/task-",
        ),
        (
            "upper-case uuid",
            "2091E717-A2D0-4967-9D63-96B8A5A868B3/task-1",
        ),
        ("bare uuid", "2091e717-a2d0-4967-9d63-96b8a5a868b3"),
    ] {
        let steps = turn(running(&start_text(id, "\nTask Description: x")));
        assert_eq!(last_of(FILE, "r2-5", &steps), None, "{what}");
    }
}

//! Acceptance tests for `specs/session-overview-ui` R3.1–R3.4 (D4): the Now doing
//! column's first line, through `app::overview::now_doing`.

use ai_monitor::telemetry::Subagent;
use app::overview::{now_doing, NowDoingInput};
use app::SessionDigest;
use renderer::overview_model::{Bucket, DialogSubject};

fn text(bucket: Bucket, subject: Option<&DialogSubject>, digest: Option<&SessionDigest>) -> String {
    now_doing(&NowDoingInput {
        bucket,
        subject,
        digest,
    })
}

fn running(id: &str, start_ms: u64) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: Some("general-purpose".to_string()),
        description: Some(format!("task {id}")),
        model: None,
        start_ms: Some(start_ms),
        end_ms: None,
        completed: false,
        agent_id: None,
        ended_ok: true,
        report_preview: None,
    }
}

type Tool = (String, Option<String>, Option<u64>);

fn tool(name: &str, summary: Option<&str>, start_ms: Option<u64>) -> Tool {
    (name.to_string(), summary.map(str::to_string), start_ms)
}

/// A digest whose newest finished tool (and newest work tool) is `last`.
fn digest_with(last: Option<Tool>, subagents: Vec<Subagent>) -> SessionDigest {
    SessionDigest {
        subagents,
        last_tool: last.clone(),
        last_work_tool: last,
        ..SessionDigest::default()
    }
}

#[test]
fn r3_1_needs_you_by_subject() {
    let cmd = DialogSubject::Command("ls -d ~/.config".to_string());
    let q = DialogSubject::Question;
    let plan = DialogSubject::Plan;
    assert_eq!(
        text(Bucket::NeedsYou, Some(&cmd), None),
        "Approval needed · ls -d ~/.config"
    );
    assert_eq!(text(Bucket::NeedsYou, Some(&q), None), "Question waiting");
    assert_eq!(
        text(Bucket::NeedsYou, Some(&plan), None),
        "Plan approval waiting"
    );
    assert_eq!(text(Bucket::NeedsYou, None, None), "Waiting for your input");
}

#[test]
fn r3_2_working_on_a_tool() {
    let with_summary = digest_with(
        Some(tool("Bash", Some("git branch -d feat/x"), Some(5_000))),
        vec![],
    );
    assert_eq!(
        text(Bucket::Working, None, Some(&with_summary)),
        "Bash · git branch -d feat/x"
    );

    let no_summary = digest_with(Some(tool("Bash", None, Some(5_000))), vec![]);
    assert_eq!(text(Bucket::Working, None, Some(&no_summary)), "Bash");

    let no_tool = digest_with(None, vec![]);
    assert_eq!(text(Bucket::Working, None, Some(&no_tool)), "Working");
}

#[test]
fn r3_3_waiting_on_subagents() {
    let two = || vec![running("a", 500), running("b", 1000)];

    for (label, work_start) in [
        ("equal start", Some(Some(1000))),
        ("older", Some(Some(900))),
        ("absent", None),
    ] {
        let last = work_start.map(|s| tool("Bash", Some("cargo test"), s));
        let d = digest_with(last, two());
        assert_eq!(
            text(Bucket::Working, None, Some(&d)),
            "Waiting on 2 subagents",
            "last_work_tool {label}"
        );
    }

    let newer = digest_with(Some(tool("Bash", Some("cargo test"), Some(1001))), two());
    assert_eq!(
        text(Bucket::Working, None, Some(&newer)),
        "Bash · cargo test",
        "a work tool newer than the newest running subagent wins"
    );

    let one = digest_with(None, vec![running("a", 1000)]);
    assert_eq!(
        text(Bucket::Working, None, Some(&one)),
        "Waiting on 1 subagent"
    );
}

#[test]
fn r3_4_idle_unknown_and_non_claude() {
    let d = digest_with(Some(tool("Bash", Some("ls"), Some(1))), vec![]);
    assert_eq!(
        text(Bucket::Idle, None, Some(&d)),
        "Turn finished · ready for next prompt"
    );
    assert_eq!(text(Bucket::Agent, None, None), "Agent running");
    // A Working Codex session: no log digest.
    assert_eq!(text(Bucket::Working, None, None), "Working");
}

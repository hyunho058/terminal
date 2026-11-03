//! TT11 — acceptance test for `specs/session-overview-ui` R11.1 (D25): the tab label (and the
//! overview row's prompt line) is a *human* prompt only. `activity::recent_prompts` must admit
//! only records `activity::is_human_prompt` accepts, so a task notification, a peer message and
//! an `isMeta` skill injection never become the label.
//!
//! The log is a real temp file holding Claude-shaped records (`type`, `message.role`,
//! `origin.kind`, `isMeta`) as Claude Code 2.1.28x writes them.

use std::io::Write;
use std::path::PathBuf;

use ai_monitor::activity::recent_prompts;
use serde_json::json;

const SESSION: &str = "5d1e2c4a-0000-4000-8000-00000000r111";

fn base(uuid: &str, ts: &str) -> serde_json::Value {
    json!({
        "type": "user",
        "uuid": uuid,
        "sessionId": SESSION,
        "timestamp": ts,
        "cwd": "/Users/tester/IdeaProjects/terminal",
        "userType": "external",
    })
}

fn human_prompt() -> String {
    let mut v = base("u-1", "2026-10-31T10:00:00.000Z");
    v["message"] = json!({"role": "user", "content": "fix the duplicate G7 numbering"});
    v.to_string()
}

fn skill_injection() -> String {
    let mut v = base("u-2", "2026-10-31T10:00:01.000Z");
    v["isMeta"] = json!(true);
    v["message"] = json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": "Base directory for this skill: /Users/tester/.claude/skills/specify\n\n# Specify\n\nTurn a goal into a spec."
        }]
    });
    v.to_string()
}

fn peer_message() -> String {
    let mut v = base("u-3", "2026-10-31T10:05:00.000Z");
    v["origin"] = json!({"kind": "peer"});
    v["message"] = json!({
        "role": "user",
        "content": "peer agent reports: the numbering scan found two G7 headings"
    });
    v.to_string()
}

fn task_notification() -> String {
    let mut v = base("u-4", "2026-10-31T10:06:00.000Z");
    v["origin"] = json!({"kind": "task-notification"});
    v["message"] = json!({
        "role": "user",
        "content": "<task-notification>\n<task-id>a1b2c3d4e5f6</task-id>\n<status>completed</status>\n<summary>Agent \"Scan headings\" completed</summary>\n<result>Found two G7 headings.</result>\n</task-notification>"
    });
    v.to_string()
}

/// A fresh directory under the system temp dir, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_log(lines: &[String]) -> (TempDir, PathBuf) {
    let dir = std::env::temp_dir().join(format!("tt11-digest-label-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{SESSION}.jsonl"));
    let mut f = std::fs::File::create(&path).expect("create log");
    for l in lines {
        writeln!(f, "{l}").expect("write line");
    }
    (TempDir(dir), path)
}

#[test]
fn r11_1_label_skips_notification_peer_and_meta_records() {
    // Oldest first on disk, so newest first they read: notification, peer, meta, human.
    let (_dir, log) = write_log(&[
        human_prompt(),
        skill_injection(),
        peer_message(),
        task_notification(),
    ]);

    let prompts = recent_prompts(&log);

    assert_eq!(
        prompts.first().map(String::as_str),
        Some("fix the duplicate G7 numbering"),
        "the first entry is the human prompt: {prompts:#?}"
    );
    for p in &prompts {
        assert!(
            !p.contains("task-notification") && !p.contains("task-id"),
            "a task notification is not a prompt: {prompts:#?}"
        );
        assert!(
            !p.contains("peer agent reports"),
            "a peer message is not a prompt: {prompts:#?}"
        );
        assert!(
            !p.contains("Base directory for this skill"),
            "an isMeta skill injection is not a prompt: {prompts:#?}"
        );
    }
}

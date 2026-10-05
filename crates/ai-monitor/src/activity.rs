//! Bounded tail read of a session JSONL — the raw activity signal (D7 of the
//! `tab-metadata` feature). Additive to `ai-monitor`: it uses only `std` +
//! `serde_json` (via the existing [`parse_line`](crate::parse_line)), changes no
//! existing function, and leaves `Cargo.toml` untouched (R6.9).
//!
//! [`backfill`](crate::backfill) reads the *whole* file and is unusable on a poll loop;
//! this reads at most the final [`TAIL_CAP`] bytes and returns only the role of the last
//! parseable line, plus the file's `(mtime, len)` so the caller can memoise on them.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

use serde_json::Value;

use crate::parse_line;

/// The most we ever read from a log's tail.
pub const TAIL_CAP: u64 = 8 * 1024;

/// How many recent human user prompts `recent_prompts` returns — enough for the app to skip
/// a run of short/continuation prompts (D10) and still find a substantive one.
const PROMPT_TAIL: usize = 8;

/// How the LAST *meaningful* message (a line whose `message.role` is `assistant` or
/// `user`) classifies for the input-waiting rule (D16 of `left-sidebar-waiting-area`).
/// The app maps `AssistantEndTurn` and `AssistantAsksUser` → `Waiting` and every other
/// variant → `Busy`; a `None` on [`TailState::last`] (no meaningful line, or the newest
/// record could not be robustly parsed — truncated by the tail cap / half-written) must
/// NOT be read as Waiting (conservative bias: a false "waiting" is the worst failure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastMessage {
    /// assistant turn whose `stop_reason == "end_turn"`: control handed back to the human.
    AssistantEndTurn,
    /// assistant turn that called a tool from [`BLOCKING_TOOLS`] — a question put to the
    /// human. Its `stop_reason` is `"tool_use"` like any other tool call, but the agent
    /// cannot proceed until a person answers, so it is a *waiting* signal, not busy.
    AssistantAsksUser,
    /// assistant turn mid-work (`stop_reason == "tool_use"`, or anything not `end_turn`).
    AssistantBusy,
    /// user line carrying a top-level `toolUseResult` (a tool result the agent is consuming).
    UserToolResult,
    /// user line without `toolUseResult` (a real human prompt the agent will answer next).
    UserPrompt,
}

/// The role of the last complete line of a log, plus the facts the caller memoises on and
/// the D16 input-waiting classification of the last meaningful message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailState {
    pub role: String,
    pub mtime: SystemTime,
    pub len: u64,
    /// The last meaningful (assistant/user) message's classification (D16), or `None` when
    /// there is none or the newest record is not robustly parseable (see [`LastMessage`]).
    pub last: Option<LastMessage>,
}

/// The role of the LAST parseable line in `log`, plus the file's `mtime` and `len`.
/// Reads at most the final [`TAIL_CAP`] bytes. `None` if the file is missing, empty, or
/// has no parseable line in that window. Never panics (D14).
///
/// This is the cheap *activity* read (busy/waiting), which only needs the last message.
/// The tab-identity *label* needs the last human prompt, which can sit far outside this
/// 8 KiB window (a single assistant turn + tool output easily exceeds it) — that is
/// [`recent_prompts`], a separate, wider read.
pub fn tail_state(log: &Path) -> Option<TailState> {
    let mut file = std::fs::File::open(log).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    if len == 0 {
        return None;
    }
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    // Seek to the last TAIL_CAP bytes and read forward — a genuine cap, so a 200 KiB log
    // costs 8 KiB of I/O (R6.1).
    let start = len.saturating_sub(TAIL_CAP);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_CAP).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);

    // The last parseable line. Scanning from the end skips a half-written final line and
    // the partial first line left by seeking into the middle of a record.
    let role = text.lines().rev().find_map(parse_line)?.role;
    // The D16 input-waiting signal from the last *meaningful* message (see below).
    let last = last_meaningful(&text);
    Some(TailState {
        role,
        mtime,
        len,
        last,
    })
}

/// The most we scan for the tab-identity label (tab-identity). Wider than [`TAIL_CAP`]
/// because a human prompt can sit far behind the newest activity — in an active session,
/// assistant turns + tool output between two human prompts routinely exceed 8 KiB. Still
/// bounded so a multi-MB log cannot cost an unbounded read; a prompt older than this window
/// is covered by the caller's carry-forward cache (D6) until the next human prompt lands.
pub const PROMPT_SCAN_CAP: u64 = 1024 * 1024;

/// The recent *human* user prompts in `log`, newest first, capped to `PROMPT_TAIL` — the
/// RAW candidates for the tab label (which one wins is the app's D10 policy, not ours).
/// Reads at most the final [`PROMPT_SCAN_CAP`] bytes. `None`-safe: a missing/empty file
/// yields an empty vec, never a panic (D14).
pub fn recent_prompts(log: &Path) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(log) else {
        return Vec::new();
    };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    let len = meta.len();
    let start = len.saturating_sub(PROMPT_SCAN_CAP);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.take(PROMPT_SCAN_CAP).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    recent_user_prompts(&String::from_utf8_lossy(&buf))
}

/// The recent *human* user prompts in `text`, newest first, capped to [`PROMPT_TAIL`]. A
/// candidate is a complete JSON line whose `message.role` is `user`, that carries no
/// top-level `toolUseResult` (that would be a tool result the agent consumed, not a human
/// turn — matching [`classify_user`]), and that has non-empty text content. The pick of
/// which candidate labels the tab (D10) is the app's, not ours.
fn recent_user_prompts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if let Some(p) = user_prompt_text(&v) {
                out.push(p);
                if out.len() >= PROMPT_TAIL {
                    break;
                }
            }
        }
    }
    out
}

/// The one-line text of `v` iff it is a human user prompt (role `user`, no `toolUseResult`)
/// with non-empty content that is not a Claude Code command-wrapper turn; otherwise `None`.
fn user_prompt_text(v: &Value) -> Option<String> {
    if v.pointer("/message/role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    if v.get("toolUseResult").is_some() {
        return None; // a tool result the agent consumed, not a human turn
    }
    let flat = crate::one_line(&message_text(v)?);
    if flat.is_empty() || is_command_wrapper(&flat) {
        return None;
    }
    Some(flat)
}

/// Claude Code injects synthetic `role: "user"` turns for its slash-commands and their
/// output — a tag-wrapped payload (`<command-name>…`, `<command-message>…`, `<command-args>…`,
/// `<command-contents>…`, `<local-command-stdout>…`, `<local-command-caveat>…`), not something
/// the person typed. Skip them so a tab label is always a genuine human prompt (tab-identity).
fn is_command_wrapper(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("<command-") || t.starts_with("<local-command-")
}

/// The first non-blank `message.content[].text` (array form), or the `message.content`
/// string (string form), of `v`. `None` when there is no textual content — e.g. a user
/// turn whose content is only tool-result / image blocks.
fn message_text(v: &Value) -> Option<String> {
    if let Some(items) = v.pointer("/message/content").and_then(Value::as_array) {
        for it in items {
            if let Some(t) = it.get("text").and_then(Value::as_str) {
                if !t.trim().is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        return None;
    }
    v.pointer("/message/content")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Classify the LAST *meaningful* line — one whose `message.role` is `assistant` or `user`
/// (D16). Scanning from the end, a non-empty line that is NOT complete JSON and sits
/// *after* (i.e. newer than) the meaningful line means the newest record was truncated by
/// the tail cap or is half-written, so we return `None`; the caller then keeps its existing
/// (never-Waiting) fallback — a false "waiting" is the worst failure. Non-meaningful
/// complete lines (summary/system/ai-title/…) are skipped, not fatal.
fn last_meaningful(text: &str) -> Option<LastMessage> {
    let mut unreadable_after = false;
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => match v.pointer("/message/role").and_then(Value::as_str) {
                Some("assistant") => {
                    return if unreadable_after {
                        None
                    } else {
                        Some(classify_assistant(&v))
                    };
                }
                Some("user") => {
                    return if unreadable_after {
                        None
                    } else {
                        Some(classify_user(&v))
                    };
                }
                // A complete but non-meaningful line (its `message.role` is absent/other):
                // ignore it and keep scanning back for the real signal.
                _ => {}
            },
            // A non-empty line that is not complete JSON: the newest record is unreadable.
            Err(_) => unreadable_after = true,
        }
    }
    None
}

/// The CLOSED list of tool names that block on a human *by definition*: the agent emits
/// the call and then cannot make progress until a person answers it. Both are Claude Code
/// tools whose only possible resolution is human input — `AskUserQuestion` puts a question
/// to the user, `ExitPlanMode` asks them to approve a plan.
///
/// **Under Claude Code 2.1.28x these entries do not fire.** That version writes an assistant
/// `tool_use` record to the log only once the tool *returns*, so while one of these dialogs
/// is open its call is not on disk at all, and [`last_meaningful`] lands on whatever came
/// before it — typically the previous tool's `tool_result`, which reads `Busy`. Measured
/// 2026-10-04: an `AskUserQuestion` dialog stayed on screen for 10+ minutes with the log's
/// mtime frozen and no record for it in the file, and a 0.6 s `Bash` call was absent from its
/// own log while it ran, then appeared together with its `tool_result`. A record's
/// `timestamp` is when it was *created*, not when it was *written*, which is why the corpus
/// sample below could not have shown this. These dialogs can only be caught on screen, which
/// is the job of the rules in `crates/app/src/prompt.rs` (`shows_blocking_prompt`).
///
/// The entries are kept on purpose. They are harmless: a pending call is never on disk, and
/// once it is written its `tool_result` follows immediately, so they cannot fire wrongly.
/// And they come back to life, unchanged, if a Claude Code version writes pending calls
/// again. [`LastMessage::AssistantAsksUser`] itself is live regardless: Antigravity reaches it
/// through its own list (`BLOCKING_TOOLS` in `antigravity/tail.rs`, `ask_question`), because
/// Antigravity writes the dispatching step before the answer.
///
/// This list is deliberately closed and exact-matched. It must never grow to cover tools
/// that merely *tend* to be slow or that a permission prompt *might* gate (`Bash`, `Edit`,
/// …): those resolve on their own in the common case, and a wrong entry here manufactures a
/// false "waiting" — the worst failure this module can produce. A new entry needs the same
/// bar: blocking on a human is the tool's definition, not a mode it sometimes enters.
///
/// Evidence bar per entry: for `AskUserQuestion` the record's **shape** is log-verified,
/// and *when it reaches disk* is not. Every record sampled (400+ across the author's
/// `~/.claude/projects/**/*.jsonl`) carries `stop_reason: "tool_use"` with the name in a
/// `tool_use` block, with no exceptions. But the sample read records that had already been
/// written, so it could not say whether one is present while its question is pending — and
/// under 2.1.28x it is not (above). The "answering line landed 99 s later" once cited here
/// compared two `timestamp`s, i.e. creation times, so it never showed the call on disk
/// during those 99 s. That corpus is private and mutating, so the shape is the reproducible
/// claim here, not any exact count.
/// `ExitPlanMode` is **by construction** — no real `tool_use` record was available to
/// sample, and under auto-accept-plan it can resolve without a person. Both directions of
/// that uncertainty are safe: a wrong shape simply never matches and the turn reads `Busy`.
const BLOCKING_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode"];

/// An assistant message is `AssistantEndTurn` iff its stop reason is `end_turn`
/// (`.message.stop_reason`, else top-level `.stopReason`). Otherwise, if it called a
/// [`BLOCKING_TOOLS`] tool it is `AssistantAsksUser` (`stop_reason` is `tool_use` there
/// too, which is why the tool name — not the stop reason — is the signal); else
/// `AssistantBusy`. `end_turn` is checked FIRST so the existing signal keeps precedence.
fn classify_assistant(v: &Value) -> LastMessage {
    let stop = v
        .pointer("/message/stop_reason")
        .and_then(Value::as_str)
        .or_else(|| v.get("stopReason").and_then(Value::as_str));
    if stop == Some("end_turn") {
        LastMessage::AssistantEndTurn
    } else if calls_blocking_tool(v) {
        LastMessage::AssistantAsksUser
    } else {
        LastMessage::AssistantBusy
    }
}

/// Whether `v`'s content carries a `tool_use` block whose `name` is in [`BLOCKING_TOOLS`].
/// Array content only — the string form of `message.content` carries no tool calls. Exact
/// name match; an absent/odd shape simply yields `false` (never panics, D14).
///
/// Unlike the `stop_reason` check, this needs the record's whole `content` array to be
/// inside [`TAIL_CAP`], since the `tool_use` block can be the last thing in it. Measured
/// over the sampled real `AskUserQuestion` records: median 2.2 KiB, p90 3.5 KiB, max
/// 6.0 KiB — inside the 8 KiB window, but with only ~25% headroom. A record that outgrew
/// the cap is cut, and either [`last_meaningful`] sees an unreadable newest line and
/// returns `None` (caller falls back to the never-Waiting recency rule) or, if nothing
/// parseable remains in the window at all, [`tail_state`] itself returns `None` (caller
/// reads `Unknown`). Both paths miss the signal rather than fake it. If that headroom ever
/// closes, raise [`TAIL_CAP`] — do not loosen the match.
fn calls_blocking_tool(v: &Value) -> bool {
    let Some(items) = v.pointer("/message/content").and_then(Value::as_array) else {
        return false;
    };
    items.iter().any(|b| {
        b.get("type").and_then(Value::as_str) == Some("tool_use")
            && b.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| BLOCKING_TOOLS.contains(&n))
    })
}

/// A user message is `UserToolResult` iff it carries a top-level `toolUseResult` (the agent
/// is still working); otherwise it is a real human prompt, `UserPrompt`.
fn classify_user(v: &Value) -> LastMessage {
    if v.get("toolUseResult").is_some() {
        LastMessage::UserToolResult
    } else {
        LastMessage::UserPrompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpfile(tag: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aimon-activity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let p = dir.join(format!("{tag}.jsonl"));
        std::fs::write(&p, bytes).expect("write");
        p
    }

    #[test]
    fn missing_or_empty_is_none() {
        assert!(tail_state(Path::new("/no/such/log.jsonl")).is_none());
        let empty = tmpfile("empty", b"");
        assert!(tail_state(&empty).is_none());
    }

    #[test]
    fn reads_only_the_tail_of_a_large_log() {
        // R6.1 — the head parses one way, the tail another; the tail must win.
        let mut bytes = Vec::new();
        // ~192 KiB of "user" lines up front …
        let head = br#"{"type":"user","message":{"role":"user"}}"#;
        while bytes.len() < 192 * 1024 {
            bytes.extend_from_slice(head);
            bytes.push(b'\n');
        }
        // … then a final assistant line.
        bytes.extend_from_slice(br#"{"type":"assistant","message":{"role":"assistant"}}"#);
        bytes.push(b'\n');
        let log = tmpfile("big", &bytes);
        let ts = tail_state(&log).expect("tail");
        assert_eq!(ts.role, "assistant", "the tail line's role, not the head's");
        assert_eq!(ts.len, bytes.len() as u64);
    }

    #[test]
    fn ignores_a_half_written_final_line() {
        // A trailing partial (unterminated) JSON line is skipped for the last complete one.
        let body = b"{\"type\":\"assistant\"}\n{\"type\":\"user\",\"incomplete\":";
        let log = tmpfile("partial", body);
        let ts = tail_state(&log).expect("tail");
        assert_eq!(ts.role, "assistant");
    }

    // ---- the D16 last-meaningful-message signal ----

    #[test]
    fn last_message_end_turn_is_the_waiting_signal() {
        let body =
            br#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#;
        let log = tmpfile("endturn", body);
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantEndTurn)
        );
    }

    #[test]
    fn last_message_tool_use_is_busy() {
        let body =
            br#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#;
        let log = tmpfile("tooluse", body);
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantBusy)
        );
    }

    /// The real `AskUserQuestion` record shape, transcribed verbatim from a live session
    /// log (`~/.claude/projects/**/*.jsonl`): `stop_reason: "tool_use"` — indistinguishable
    /// from any other tool call by stop reason alone — with the tool name in a `tool_use`
    /// content block. These tests pin how the classifier reads that *shape*. They do not show
    /// a path that fires live for Claude: under Claude Code 2.1.28x this record is not on disk
    /// while the human decides, and is written only with the answer right behind it (see
    /// [`BLOCKING_TOOLS`]).
    const ASK_USER_QUESTION_LINE: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
        r#""content":[{"type":"tool_use","id":"toolu_01","name":"AskUserQuestion","#,
        r#""input":{"questions":[]}}]}}"#
    );

    #[test]
    fn ask_user_question_is_a_waiting_signal_not_busy() {
        // The bug: AskUserQuestion carries stop_reason "tool_use", so it used to read
        // AssistantBusy and the sidebar never saw a rising edge into Waiting.
        let log = tmpfile("askuser", ASK_USER_QUESTION_LINE.as_bytes());
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantAsksUser)
        );
    }

    #[test]
    fn exit_plan_mode_is_a_waiting_signal() {
        // The other half of the closed list — plan approval also blocks on a human.
        let body = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
            r#""content":[{"type":"tool_use","id":"toolu_02","name":"ExitPlanMode","#,
            r#""input":{"plan":"do the thing"}}]}}"#
        );
        let log = tmpfile("exitplan", body.as_bytes());
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantAsksUser)
        );
    }

    #[test]
    fn ordinary_tool_calls_stay_busy() {
        // The closed list is exact-matched: a normal tool call — even a long-running one,
        // and even one whose name merely resembles a blocking tool — is still Busy. This
        // is the conservative bias: only a tool that blocks on a human by definition may
        // yield Waiting.
        for name in [
            "Bash",
            "Edit",
            "Read",
            "AskUserQuestionHelper",
            "askuserquestion",
        ] {
            let body = format!(
                concat!(
                    r#"{{"type":"assistant","message":{{"role":"assistant","stop_reason":"tool_use","#,
                    r#""content":[{{"type":"tool_use","id":"toolu_03","name":"{}","input":{{}}}}]}}}}"#
                ),
                name
            );
            let log = tmpfile(&format!("busy-{name}"), body.as_bytes());
            assert_eq!(
                tail_state(&log).expect("tail").last,
                Some(LastMessage::AssistantBusy),
                "{name} is not a blocking tool"
            );
        }
    }

    #[test]
    fn blocking_tool_name_outside_a_tool_use_block_stays_busy() {
        // Only a `type: "tool_use"` block counts. The tool NAME appearing in assistant
        // prose (e.g. "I'll use AskUserQuestion next") must not fake a waiting signal.
        let body = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
            r#""content":[{"type":"text","text":"next I will call AskUserQuestion"},"#,
            r#"{"type":"tool_use","id":"toolu_04","name":"Read","input":{}}]}}"#
        );
        let log = tmpfile("prose-mention", body.as_bytes());
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantBusy),
            "a name in prose is not a tool call"
        );
    }

    #[test]
    fn end_turn_wins_over_the_blocking_tool_check() {
        // Ordering: `end_turn` is checked FIRST, so the pre-existing signal keeps
        // precedence and this change cannot reclassify anything that already worked.
        let body = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","#,
            r#""content":[{"type":"tool_use","id":"toolu_05","name":"AskUserQuestion","input":{}}]}}"#
        );
        let log = tmpfile("endturn-and-ask", body.as_bytes());
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantEndTurn)
        );
    }

    #[test]
    fn an_answered_question_is_busy_again() {
        // Once the human answers, the tool_result user line is the newest meaningful
        // message — so the tab must leave Waiting and read Busy again (the falling edge
        // `waiting.rs` needs to clear the band item).
        let body = format!(
            "{ASK_USER_QUESTION_LINE}\n{}\n",
            r#"{"type":"user","message":{"role":"user"},"toolUseResult":{"answer":"ok"}}"#
        );
        let log = tmpfile("askuser-answered", body.as_bytes());
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::UserToolResult)
        );
    }

    #[test]
    fn truncated_record_after_a_question_suppresses_the_signal() {
        // Conservative bias holds for the NEW signal too: a half-written newer line means
        // the newest record is unreadable → `None`, never Waiting.
        let body = format!(
            "{ASK_USER_QUESTION_LINE}\n{}",
            r#"{"type":"user","message":{"role":"user""#
        );
        let log = tmpfile("askuser-truncated", body.as_bytes());
        assert_eq!(tail_state(&log).expect("tail").last, None);
    }

    #[test]
    fn last_message_stop_reason_falls_back_to_top_level() {
        // stop_reason may appear top-level as `stopReason` rather than under `.message`.
        let body =
            br#"{"type":"assistant","stopReason":"end_turn","message":{"role":"assistant"}}"#;
        let log = tmpfile("topstop", body);
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantEndTurn)
        );
    }

    #[test]
    fn last_message_user_tool_result_vs_prompt() {
        let res = tmpfile(
            "uresult",
            br#"{"type":"user","message":{"role":"user"},"toolUseResult":{"stdout":"ok"}}"#,
        );
        assert_eq!(
            tail_state(&res).expect("tail").last,
            Some(LastMessage::UserToolResult)
        );
        let prompt = tmpfile(
            "uprompt",
            br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
        );
        assert_eq!(
            tail_state(&prompt).expect("tail").last,
            Some(LastMessage::UserPrompt)
        );
    }

    #[test]
    fn non_meaningful_trailing_lines_are_skipped() {
        // A summary line written after the assistant turn has no `message.role`; it is
        // ignored, so the assistant end_turn remains the last meaningful message.
        let body = b"{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"end_turn\"}}\n{\"type\":\"summary\",\"summary\":\"x\"}\n";
        let log = tmpfile("summary-after", body);
        assert_eq!(
            tail_state(&log).expect("tail").last,
            Some(LastMessage::AssistantEndTurn)
        );
    }

    // ---- recent human prompts (tab-identity, raw candidates) ----

    #[test]
    fn prompts_are_human_turns_newest_first() {
        // Only role=user lines without toolUseResult, newest first; tool results and
        // assistant turns are excluded.
        let body = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"first task"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user"},"toolUseResult":{"stdout":"ok"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"second task"}]}}"#,
            "\n",
        );
        let log = tmpfile("prompts", body.as_bytes());
        assert_eq!(recent_prompts(&log), vec!["second task", "first task"]);
    }

    #[test]
    fn prompts_skip_non_text_user_turns() {
        // A user turn whose content is only a tool_result block (no text) is not a prompt.
        let body = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"real prompt"}]}}"#,
            "\n",
        );
        let log = tmpfile("prompts-notext", body.as_bytes());
        assert_eq!(recent_prompts(&log), vec!["real prompt"]);
    }

    #[test]
    fn prompts_skip_claude_code_command_wrappers() {
        // Claude Code's slash-command turns arrive as role=user with tag-wrapped text; they
        // are not human prompts and must not become tab labels (real-data regression).
        let body = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"add a retry to the fetch helper"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<command-name>/effort</command-name> <command-args>xhigh</command-args>"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<local-command-stdout>Set effort level to xhigh</local-command-stdout>"}]}}"#,
            "\n",
        );
        let log = tmpfile("prompts-cmdwrap", body.as_bytes());
        assert_eq!(
            recent_prompts(&log),
            vec!["add a retry to the fetch helper"],
            "command-wrapper turns excluded; only the real prompt remains"
        );
    }

    #[test]
    fn prompts_empty_when_no_human_turn() {
        let body =
            br#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#;
        let log = tmpfile("prompts-none", body);
        assert!(recent_prompts(&log).is_empty());
    }

    #[test]
    fn recent_prompts_finds_a_human_prompt_far_beyond_the_activity_tail() {
        // ROOT-CAUSE regression (tab-identity): the sole human prompt sits at the very start,
        // then >8 KiB of assistant/tool output pushes it far outside the activity TAIL_CAP.
        // `tail_state` (8 KiB) cannot see it, but `recent_prompts` (PROMPT_SCAN_CAP) must.
        let mut body = Vec::new();
        body.extend_from_slice(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"실제 사람 프롬프트"}]}}"#.as_bytes(),
        );
        body.push(b'\n');
        // ~40 KiB of assistant/tool filler after it (well past the 8 KiB activity window).
        let filler =
            br#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#;
        while body.len() < 40 * 1024 {
            body.extend_from_slice(filler);
            body.push(b'\n');
        }
        let log = tmpfile("prompt-far-back", &body);
        // The activity tail cannot reach the prompt…
        assert!(
            body.len() as u64 - TAIL_CAP > 100,
            "filler pushes the prompt outside TAIL_CAP"
        );
        // …but the wider prompt scan finds it.
        assert_eq!(recent_prompts(&log), vec!["실제 사람 프롬프트"]);
    }

    #[test]
    fn truncated_newest_record_suppresses_the_waiting_signal() {
        // The last COMPLETE meaningful line is end_turn, but a newer half-written line
        // follows (as a >TAIL_CAP record cut by the cap would look) — the newest record is
        // unreadable, so `last` must be None so the caller never infers Waiting.
        let body = b"{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"end_turn\"}}\n{\"type\":\"user\",\"message\":{\"role\":\"user\"";
        let log = tmpfile("truncated", body);
        assert_eq!(
            tail_state(&log).expect("tail").last,
            None,
            "newest record unreadable → no Waiting signal"
        );
    }
}

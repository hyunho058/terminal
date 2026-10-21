//! Antigravity's activity signal: the last meaningful step of a `transcript.jsonl`,
//! classified into Claude's [`LastMessage`] (`agent-card-status` D1, D10).
//!
//! [`crate::activity::tail_state`] cannot answer this. It is keyed on Claude's record shape
//! (`message.role` user/assistant), and no Antigravity step carries one, so an Antigravity
//! transcript reads as "no meaningful line" there, forever. This is the same question put to
//! the other format, and it returns the *same* [`TailState`] on purpose: the app maps it with
//! Claude's unchanged `state_from`, so "Waiting" means one thing for both agents by
//! construction, not by a promise kept in two places (D10). The busy/waiting policy stays in
//! the app; this module only says what the newest step *is*.
//!
//! Two reads, and the second is conditional (D14):
//!
//! - the final [`TAIL_CAP`] bytes decide the classification, exactly as Claude's does;
//! - **only** when the deciding step is a tool-less reply, a scan back of up to
//!   [`PROMPT_SCAN_CAP`] bytes looks for the last `USER_INPUT` to rule out background work
//!   (D13), and for background work the turn started that has not reported back
//!   (`agy-background-working` D1, which amends D13). Every other outcome costs the 8 KiB
//!   tail and nothing more (R2.7).
//!
//! Both reads go through one byte source ([`state_from_reads`]), which is what lets a test
//! count the bytes a classification cost without a file, a clock or any global state.
//!
//! Read-only, like the rest of [`crate::antigravity`]: the transcript is opened for reading.
//! No FD or `libc` code (AG11, `agent-card-status` R9.3) — which conversation a tab holds is
//! `app`'s question; this is handed a path.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

use serde_json::Value;

use crate::activity::{LastMessage, TailState, PROMPT_SCAN_CAP, TAIL_CAP};
use crate::telemetry::AG_RESULT_STEPS;

/// Steps the CLI writes *around* a turn rather than as part of it — skipped, so a reply
/// followed by a checkpoint still reads as the reply (`agent-card-status` D1, R1.6).
///
/// Closed and exact, like every list here. A kind that is neither on this list nor a
/// classified one is not skipped: it decides, as "no robust signal" (R1.7). Skipping it
/// instead would let a stale reply behind a step we do not understand read as Waiting.
const BOOKKEEPING_STEPS: &[&str] = &[
    "CHECKPOINT",
    "SYSTEM_MESSAGE",
    "CONVERSATION_HISTORY",
    "ERROR_MESSAGE",
];

/// Antigravity's human-blocking tools: dispatching one means the agent cannot go on until a
/// person answers (`agent-card-status` D1). The mirror of Claude's `AskUserQuestion` /
/// `ExitPlanMode` list in [`crate::activity`], held to the same bar — blocking on a human is
/// the tool's definition, not a mode it sometimes enters.
///
/// Why the *dispatch* and not the `ASK_QUESTION` step: a step is appended when it
/// completes, so `ASK_QUESTION` lands carrying the answer. While the question is on screen
/// the newest record is the `PLANNER_RESPONSE` naming `ask_question` (11 of the 12 measured
/// `ASK_QUESTION` steps are preceded by exactly that).
const BLOCKING_TOOLS: &[&str] = &["ask_question"];

/// The tools a finished turn may have dispatched and still read as finished — the D13
/// **allow-list** (`agent-card-status` D13).
///
/// A reply is the model talking, and on 21 of 111 measured replies it was talking *while*
/// its subagents or background tasks still ran ("waiting for the final subagent", 96 s
/// before the next step). All 21 had background work since the last user input, and no
/// reply without it was ever followed by more model steps (0 of 56). So a reply is a
/// finished turn only when everything since the last `USER_INPUT` is foreground work.
///
/// An allow-list, not a deny-list of the background tools seen (`invoke_subagent`,
/// `define_subagent`, `manage_subagents`, `send_message`, `manage_task`, `schedule`): on the
/// measured corpus the two agree, and they differ exactly on a tool nobody has measured. A
/// deny-list reads a reply after a new or renamed background tool as Waiting — the worst
/// failure this project names — while this reads it as no robust signal.
///
/// Amended by `agy-background-working` D1: background work that has not yet *reported back*
/// outranks this list, and reads `AssistantBusy` (see [`turn_verdict`]).
const FOREGROUND_TOOLS: &[&str] = &[
    "run_command",
    "view_file",
    "list_dir",
    "grep_search",
    "find_by_name",
    "write_to_file",
    "replace_file_content",
    "ask_question",
];

/// The classification of `transcript`'s last meaningful step, plus its `mtime` and `len`
/// (`agent-card-status` D10, R1).
///
/// `Some` for every file that can be opened and read — an empty one included, as
/// `last: None` — so the caller's recency fallback always has the `mtime` it measures from
/// (R1.8, R1.9). `None` only when the file cannot be read at all.
///
/// `role` is always empty. The app's `state_from` reads it only when `last` is `None`, and
/// only to ask whether it is `"user"`, which would hold the tab `Busy` for 100 s after the
/// last write. Antigravity has no role field to report, and inventing one here would be a
/// mapping nothing could observe — the fallback it drives is exactly D1's: `Busy` within
/// 10 s of the last write, else `Idle` (R1.9).
pub fn tail_state(transcript: &Path) -> Option<TailState> {
    let mut file = File::open(transcript).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    state_from_reads(len, mtime, |cap| read_last(&mut file, len, cap))
}

/// The last `cap` bytes of the first `len` bytes of `file`.
///
/// Bounded by the `len` the caller `stat`ed, not by end of file. The transcript is live, and
/// the back-scan runs after the tail read: reading to EOF would let the two windows describe
/// different files — a tail that ended in a reply and a back-scan that saw the dispatch
/// appended after it — and the classification would be of neither.
fn read_last(file: &mut File, len: u64, cap: u64) -> Option<Vec<u8>> {
    let window = cap.min(len);
    file.seek(SeekFrom::Start(len - window)).ok()?;
    let mut buf = Vec::new();
    file.take(window).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// [`tail_state`] over a byte source: `read_last(cap)` yields the transcript's final `cap`
/// bytes. Every byte the classification costs passes through it, which is how R2.7 is
/// observed — a test hands in a counting source and asserts the total.
fn state_from_reads(
    len: u64,
    mtime: SystemTime,
    mut read_last: impl FnMut(u64) -> Option<Vec<u8>>,
) -> Option<TailState> {
    let tail = read_last(TAIL_CAP)?;
    let last = last_step(&String::from_utf8_lossy(&tail), || {
        read_last(PROMPT_SCAN_CAP)
    });
    Some(TailState {
        role: String::new(),
        mtime,
        len,
        last,
    })
}

/// Classify the newest non-bookkeeping step in `tail` (`agent-card-status` D1).
///
/// The walk runs newest first and stops at the **first** line it cannot read as a step. It
/// does not skip past it to an older one: the unreadable line is the newest record — half
/// written, or longer than the tail and so visible only as a fragment — and whatever it is,
/// it is newer than any step behind it. Classifying the step behind it is how a turn that
/// has moved on would read as the reply that ended the previous one: a stale Waiting, the
/// worst failure. Claude's reader holds the same rule
/// ([`crate::activity::tail_state`]'s `unreadable_after`); the answer is `None`, and the
/// caller's fallback never yields Waiting (R1.8).
///
/// `back_scan` is called only for a tool-less reply (D14, R2.7).
fn last_step(tail: &str, back_scan: impl FnOnce() -> Option<Vec<u8>>) -> Option<LastMessage> {
    for line in tail.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let step: Value = serde_json::from_str(line).ok()?;
        let kind = step_kind(&step)?;
        if BOOKKEEPING_STEPS.contains(&kind) {
            continue;
        }
        return classify(kind, &step, back_scan);
    }
    None
}

/// D1's table for one meaningful step.
fn classify(
    kind: &str,
    step: &Value,
    back_scan: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<LastMessage> {
    match kind {
        "PLANNER_RESPONSE" => match step.get("tool_calls") {
            // The model's reply: a finished turn, if the turn did only foreground work.
            None => finished_turn(back_scan),
            // An empty array dispatched nothing, so it is a reply like any other — the
            // reading telemetry's turn count already makes of it (AG3). Unmeasured: all
            // 1,336 `tool_calls` in the corpus are non-empty arrays.
            Some(Value::Array(calls)) if calls.is_empty() => finished_turn(back_scan),
            Some(Value::Array(calls)) => Some(if calls.iter().any(is_blocking) {
                LastMessage::AssistantAsksUser
            } else {
                // Dispatched and running, or dispatched and held on a permission dialog:
                // the log cannot tell them apart, which is why dialogs come from the
                // screen (`agent-card-status` D4), not from here.
                LastMessage::AssistantBusy
            }),
            // Present but not an array: which half of the step is intact is a guess, and a
            // guess is not a robust signal.
            Some(_) => None,
        },
        "USER_INPUT" => Some(LastMessage::UserPrompt),
        // Any status, `RUNNING` included: a background task's step is a result the agent
        // has in hand, not a question put to the user.
        kind if AG_RESULT_STEPS.contains(&kind) => Some(LastMessage::UserToolResult),
        // A kind this reader has not been taught. It decides — as no robust signal — rather
        // than being skipped (see `BOOKKEEPING_STEPS`).
        _ => None,
    }
}

/// The classification of a tool-less reply, from the back-scan window (`agent-card-status`
/// D13, D14, as amended by `agy-background-working` D1): `AssistantBusy` while background work
/// the turn started has not reported back, `AssistantEndTurn` when the turn did only
/// foreground work, otherwise no robust signal — `None`, which the app's fallback never reads
/// as Waiting.
fn finished_turn(back_scan: impl FnOnce() -> Option<Vec<u8>>) -> Option<LastMessage> {
    let window = back_scan()?;
    match turn_verdict(&String::from_utf8_lossy(&window)) {
        Turn::Pending => Some(LastMessage::AssistantBusy),
        Turn::Foreground => Some(LastMessage::AssistantEndTurn),
        Turn::Undecided => None,
    }
}

/// What the steps since the last `USER_INPUT` say about the reply that ends them.
#[derive(Debug, PartialEq)]
enum Turn {
    /// A background task or subagent started in the turn has not reported back
    /// (`agy-background-working` D1).
    Pending,
    /// Every step is foreground work (D13).
    Foreground,
    /// Background work cannot be ruled out, and none is known to be pending.
    Undecided,
}

/// Walk `window` newest first to the last `USER_INPUT` (`agy-background-working` D4).
///
/// Newest first, every completion notice is seen before the start it answers, so a start
/// whose id is not yet in the completed set is pending — and nothing older can undo that,
/// which is why the walk returns at once. Pending outranks the foreground verdict (D1): an
/// unlisted tool or kind in the same turn does not hide work still running.
///
/// The walk also ends at a line it cannot read, or at the window's start. Neither is a
/// `USER_INPUT`, so with nothing pending found the answer is [`Turn::Undecided`] (D13's
/// three ways to fail, D14: an 8 KiB tail reaches the last user input for only 23 of 111
/// measured replies, 1 MiB for all of them). Steps written after the reply — a notice
/// landing — are in the window and walked like any other, so a completion clears the hold
/// as soon as it is written.
fn turn_verdict(window: &str) -> Turn {
    let mut completed: HashSet<String> = HashSet::new();
    let mut foreground = true;
    for line in window.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(step) = serde_json::from_str::<Value>(line) else {
            break;
        };
        if step_kind(&step) == Some("USER_INPUT") {
            return if foreground {
                Turn::Foreground
            } else {
                Turn::Undecided
            };
        }
        if step_kind(&step) == Some("SYSTEM_MESSAGE") {
            let ids = content(&step).map(completed_ids).unwrap_or_default();
            completed.extend(ids.into_iter().map(String::from));
        }
        if started_ids(&step).iter().any(|id| !completed.contains(*id)) {
            return Turn::Pending;
        }
        foreground &= is_foreground(&step);
    }
    Turn::Undecided
}

/// The `Tool is running as a background task` marker (`agy-background-working` D2 form 1).
const TASK_START: &str = "Tool is running as a background task with task id: ";
/// The header of an `invoke_subagent` result (D2 form 2).
const SUBAGENT_HEADER: &str = "Created the following subagents:";
/// A completion notice's header line, up to its timestamp (D3).
const NOTICE_HEADER: &str = "[Message] timestamp=";
/// A task's completion text, up to its quoted id (D3).
const TASK_ID_TEXT: &str = "Task id \"";

/// The background ids `step` starts (`agy-background-working` D2): a task id from a
/// `RUNNING` step's own header, and every subagent id under an `invoke_subagent` result's
/// header. Both are read only at the content's start, so output that *quotes* either — a
/// command that printed another transcript — starts nothing (measured: an unanchored match
/// gave a false pending on one corpus reply; the anchored one gave none).
fn started_ids(step: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = task_start(step).into_iter().collect();
    ids.extend(subagent_starts(step));
    ids
}

/// D2 form 1: `status: "RUNNING"`, and content that *begins* `Created At: <line>\n` followed
/// directly by [`TASK_START`] and a task id. Measured 2026-10-19: 58 of 58 real starts have
/// exactly this shape (41 `GENERIC`, 17 `RUN_COMMAND`), and the 2 other occurrences of the
/// marker were quoted inside `DONE` steps.
fn task_start(step: &Value) -> Option<&str> {
    if step.get("status").and_then(Value::as_str) != Some("RUNNING") {
        return None;
    }
    let body = after_line(content(step)?, "Created At: ")?;
    task_id_at(body.strip_prefix(TASK_START)?)
}

/// D2 form 2: content that *begins* `Created At: <line>\n`, optionally `Completed At:
/// <line>\n`, then [`SUBAGENT_HEADER`]; every `"conversationId": "<uuid>"` after it. Keyed on
/// the header, not the step's `type`: current `agy` writes `GENERIC` here, the fixture
/// `INVOKE_SUBAGENT`.
fn subagent_starts(step: &Value) -> Vec<&str> {
    let Some(body) = content(step).and_then(|c| after_line(c, "Created At: ")) else {
        return Vec::new();
    };
    let body = after_line(body, "Completed At: ").unwrap_or(body);
    let Some(list) = body.strip_prefix(SUBAGENT_HEADER) else {
        return Vec::new();
    };
    list.split("\"conversationId\"")
        .skip(1)
        .filter_map(|after| {
            let quoted = after.trim_start().strip_prefix(':')?.trim_start();
            let id = uuid_at(quoted.strip_prefix('"')?)?;
            quoted[1 + id.len()..].starts_with('"').then_some(id)
        })
        .collect()
}

/// The ids a `SYSTEM_MESSAGE`'s `content` completes (`agy-background-working` D3), by two
/// independent recognisers: a line beginning `[Message] timestamp=<ts> sender=<id>`, and the
/// text `Task id "<id>" ` with any status word after it. Any message from an id counts as its
/// end — measured, 57 of 57 messages from a task were terminal (45 `finished`, 10 `was
/// canceled`, 2 timer firings with no `Task id` text at all). A subagent's report has only
/// the first: its body is free text.
fn completed_ids(content: &str) -> Vec<&str> {
    let senders = content.lines().filter_map(|line| {
        let rest = line.strip_prefix(NOTICE_HEADER)?;
        let rest = &rest[rest.find(' ')?..];
        sender_id_at(rest.strip_prefix(" sender=")?)
    });
    let texts = content.match_indices(TASK_ID_TEXT).filter_map(|(at, _)| {
        let rest = &content[at + TASK_ID_TEXT.len()..];
        let id = task_id_at(rest)?;
        rest[id.len()..].starts_with("\" ").then_some(id)
    });
    senders.chain(texts).collect()
}

/// The text after `s`'s first line, if that line begins with `label`.
fn after_line<'a>(s: &'a str, label: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(label)?;
    Some(&rest[rest.find('\n')? + 1..])
}

/// A conversation id at the start of `s`: 36 characters of lowercase hex and `-` (D2).
fn uuid_at(s: &str) -> Option<&str> {
    let id = s.get(..36)?;
    id.bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
        .then_some(id)
}

/// A task id at the start of `s`: `<uuid>/task-<digits>`, ending at the last digit, so the
/// newline, `.`, `,` or `"` after it is never part of it (D2). A start and its completion are
/// cut by this one rule, or they would never match.
fn task_id_at(s: &str) -> Option<&str> {
    let uuid = uuid_at(s)?;
    let rest = s[uuid.len()..].strip_prefix("/task-")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then(|| &s[..s.len() - rest.len() + digits])
}

/// A notice's sender: a task id when one is there, else the bare conversation id of a
/// subagent (D3).
fn sender_id_at(s: &str) -> Option<&str> {
    task_id_at(s).or_else(|| uuid_at(s))
}

/// A step's `content`, when it is a string.
fn content(step: &Value) -> Option<&str> {
    step.get("content").and_then(Value::as_str)
}

/// D13 for one step: "known foreground work" — a kind this reader knows, not `status:
/// RUNNING` (a tool running as a background task), not an `INVOKE_SUBAGENT` result, and no
/// dispatch outside [`FOREGROUND_TOOLS`].
///
/// The kind check is the allow-list's own reasoning applied one level up. A step kind nobody
/// has measured — a new or renamed one, or a step with no `type` at all — is exactly where
/// unmeasured background work would show up first, and letting it through would read the
/// reply after it as Waiting, the worst failure. So it is refused, as an unlisted tool is.
fn is_foreground(step: &Value) -> bool {
    let known = step_kind(step).is_some_and(|k| {
        k == "PLANNER_RESPONSE" || BOOKKEEPING_STEPS.contains(&k) || AG_RESULT_STEPS.contains(&k)
    });
    let running = step.get("status").and_then(Value::as_str) == Some("RUNNING");
    let subagent = step_kind(step) == Some("INVOKE_SUBAGENT");
    known && !running && !subagent && dispatches_only_foreground(step)
}

/// Whether every tool `step` dispatched is on [`FOREGROUND_TOOLS`] — vacuously so for a step
/// that dispatched nothing. A call with no readable name is not on the list: an unnamed tool
/// is exactly the unmeasured one the allow-list exists to refuse.
fn dispatches_only_foreground(step: &Value) -> bool {
    match step.get("tool_calls") {
        None => true,
        Some(Value::Array(calls)) => calls
            .iter()
            .all(|call| call_name(call).is_some_and(|n| FOREGROUND_TOOLS.contains(&n))),
        Some(_) => false,
    }
}

/// Whether `call` names a [`BLOCKING_TOOLS`] tool. Exact match, so a name that merely
/// resembles one (`ask_question_v2`, `ASK_QUESTION`) stays busy.
fn is_blocking(call: &Value) -> bool {
    call_name(call).is_some_and(|n| BLOCKING_TOOLS.contains(&n))
}

/// A step's kind — its `type`, present on every one of the 3,045 measured steps.
fn step_kind(step: &Value) -> Option<&str> {
    step.get("type").and_then(Value::as_str)
}

/// A `tool_calls` entry's tool name — its `name`, present on every one measured.
fn call_name(call: &Value) -> Option<&str> {
    call.get("name").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A transcript of `steps`, one JSON object per line, as `agy` writes it.
    fn transcript(steps: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            let mut s = s.clone();
            s["step_index"] = json!(i);
            out.extend_from_slice(s.to_string().as_bytes());
            out.push(b'\n');
        }
        out
    }

    fn step(kind: &str) -> Value {
        json!({"source": "MODEL", "type": kind, "status": "DONE",
               "created_at": "2026-09-28T00:00:00Z"})
    }

    fn dispatch(name: &str) -> Value {
        let mut s = step("PLANNER_RESPONSE");
        s["tool_calls"] = json!([{"name": name, "args": {}}]);
        s
    }

    fn reply() -> Value {
        let mut s = step("PLANNER_RESPONSE");
        s["content"] = json!("Done.");
        s
    }

    /// A foreground dispatch and its result whose content pads it to ~1 KiB, so a run of
    /// them pushes the last `USER_INPUT` out of the 8 KiB tail.
    fn foreground_filler(bytes: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut size = 0;
        while size < bytes {
            let mut result = step("VIEW_FILE");
            result["content"] = json!("x".repeat(1000));
            size += 1100;
            out.push(dispatch("view_file"));
            out.push(result);
        }
        out
    }

    /// Classify `bytes` through the same entry point [`tail_state`] uses, returning the
    /// classification and the number of bytes the byte source handed out.
    fn classify_counting(bytes: &[u8]) -> (Option<LastMessage>, usize) {
        let mut read = 0;
        let state = state_from_reads(bytes.len() as u64, SystemTime::UNIX_EPOCH, |cap| {
            let start = bytes.len().saturating_sub(cap as usize);
            read += bytes.len() - start;
            Some(bytes[start..].to_vec())
        })
        .expect("an in-memory source is always readable");
        (state.last, read)
    }

    #[test]
    fn r2_7_only_a_tool_less_reply_reads_past_the_tail() {
        // A transcript well past TAIL_CAP whose last meaningful step is each of the
        // non-reply kinds: the classification must cost the 8 KiB tail and not a byte more.
        let mut head = vec![step("USER_INPUT")];
        head.extend(foreground_filler(40 * 1024));
        for (what, last) in [
            ("a dispatch", dispatch("run_command")),
            ("an ask_question dispatch", dispatch("ask_question")),
            ("a tool result", step("RUN_COMMAND")),
            ("a running tool result", {
                let mut s = step("GENERIC");
                s["status"] = json!("RUNNING");
                s
            }),
            ("a user input", step("USER_INPUT")),
            ("an unlisted kind", step("SOME_NEW_KIND")),
        ] {
            let mut steps = head.clone();
            steps.push(last);
            steps.push(step("CHECKPOINT"));
            let bytes = transcript(&steps);
            assert!(
                bytes.len() as u64 > TAIL_CAP,
                "{what}: larger than the tail"
            );
            let (_, read) = classify_counting(&bytes);
            assert!(
                read as u64 <= TAIL_CAP,
                "{what}: read {read} bytes, more than the {TAIL_CAP}-byte tail"
            );
        }
    }

    #[test]
    fn r2_7_a_tool_less_reply_does_run_the_back_scan() {
        // The other half, so the counter above is shown to be live: the same transcript
        // ending in a reply reads past the tail, and that read is what finds the user input.
        let mut steps = vec![step("USER_INPUT")];
        steps.extend(foreground_filler(40 * 1024));
        steps.push(reply());
        let bytes = transcript(&steps);
        let (last, read) = classify_counting(&bytes);
        assert_eq!(last, Some(LastMessage::AssistantEndTurn));
        assert!(
            read as u64 > TAIL_CAP,
            "read {read} bytes: no back-scan ran"
        );
    }

    #[test]
    fn r3_5_a_pending_reply_costs_the_same_back_scan_and_no_more() {
        // `agy-background-working` R3.5: walking for pending work re-uses D14's window. A
        // pending reply reads the tail plus that window, and nothing else.
        let mut start = step("GENERIC");
        start["status"] = json!("RUNNING");
        start["content"] = json!(
            "Created At: x\nTool is running as a background task with task id: \
             2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451\n"
        );
        let mut steps = vec![step("USER_INPUT")];
        steps.extend(foreground_filler(40 * 1024));
        steps.extend([dispatch("run_command"), start, reply()]);
        let bytes = transcript(&steps);
        let (last, read) = classify_counting(&bytes);
        assert_eq!(last, Some(LastMessage::AssistantBusy));
        assert!(
            read as u64 <= TAIL_CAP + PROMPT_SCAN_CAP,
            "read {read} bytes"
        );
    }

    #[test]
    fn a_back_scan_that_cannot_be_read_is_no_robust_signal() {
        // The tail read succeeds, the back-scan's fails: without the window, background
        // work cannot be ruled out, so the reply is `None`, never a finished turn.
        let bytes = transcript(&[step("USER_INPUT"), reply()]);
        let mut calls = 0;
        let state = state_from_reads(bytes.len() as u64, SystemTime::UNIX_EPOCH, |_| {
            calls += 1;
            (calls == 1).then(|| bytes.clone())
        })
        .expect("the tail was readable");
        assert_eq!(state.last, None);
    }

    #[test]
    fn a_step_kind_nobody_measured_is_not_foreground_work() {
        // D13's "known foreground work": an unlisted kind, or a step with no kind, between
        // the user input and the reply cannot be ruled out as background work — so the
        // reply is no robust signal, as it is after an unlisted tool.
        let mut untyped = step("GENERIC");
        untyped.as_object_mut().expect("an object").remove("type");
        for (what, between) in [
            ("an unlisted kind", step("SOME_NEW_KIND")),
            ("a step with no kind", untyped),
        ] {
            let bytes = transcript(&[step("USER_INPUT"), between, reply()]);
            assert_eq!(classify_counting(&bytes).0, None, "{what}");
        }
        // Control: every known kind in the same place leaves it a finished turn.
        for kind in ["CHECKPOINT", "SYSTEM_MESSAGE", "VIEW_FILE", "GENERIC"] {
            let bytes = transcript(&[step("USER_INPUT"), step(kind), reply()]);
            assert_eq!(
                classify_counting(&bytes).0,
                Some(LastMessage::AssistantEndTurn),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_tail_that_cannot_be_read_is_no_state_at_all() {
        let state = state_from_reads(10, SystemTime::UNIX_EPOCH, |_| None);
        assert_eq!(state, None);
    }
}

//! Headless assertions for Antigravity session resolution (`ai-monitor-antigravity` R1).
//!
//! Every case builds its own mock `~/.gemini/antigravity-cli` in a temp directory, so
//! nothing here reads the author's real corpus, no clock is consulted, and the answers are
//! the same on every machine and every run. The one thing these tests cannot build is an
//! open file descriptor held by another process — that half of the ladder is `app`'s
//! (AG11), and `crates/app/src/presence_fd.rs` tests it against a real descriptor.

use ai_monitor::antigravity::Probed;
use std::path::{Path, PathBuf};

use ai_monitor::activity::{LastMessage, TailState, PROMPT_SCAN_CAP, TAIL_CAP};
use ai_monitor::antigravity;
use serde_json::{json, Value};

const PARENT: &str = "4de5fc70-2d8a-44c2-8e25-a6901c93fb86";
const SUBAGENT: &str = "ed14342c-1111-4222-8333-a44444444444";
const OTHER: &str = "7640f000-2222-4333-8444-b55555555555";

/// A fresh mock antigravity root. Removed first, so a re-run never inherits a previous one.
fn root(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!("ai-monitor-agy-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("presence")).expect("presence dir");
    base
}

/// Create `brain/<id>/…/transcript.jsonl` with one step at `created_at`, and return its path.
fn write_transcript(root: &Path, id: &str, created_at: &str) -> PathBuf {
    let path = antigravity::transcript_in(root, id).expect("uuid-shaped id");
    std::fs::create_dir_all(path.parent().expect("logs dir")).expect("mkdir");
    std::fs::write(
        &path,
        format!(
            "{{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\
             \"status\":\"DONE\",\"created_at\":\"{created_at}\"}}\n"
        ),
    )
    .expect("write transcript");
    path
}

/// `brain/<id>/` with no transcript yet — a conversation that exists but has written nothing.
fn write_brain_dir_only(root: &Path, id: &str) {
    let dir = antigravity::brain_dir_in(root, id).expect("uuid-shaped id");
    std::fs::create_dir_all(&dir).expect("mkdir");
}

/// A stale `presence/<id>.lock`: on disk, held by nobody.
fn write_stale_lock(root: &Path, id: &str) {
    let lock = antigravity::presence_lock_in(root, id).expect("uuid-shaped id");
    std::fs::write(lock, b"").expect("write lock");
}

/// One `history.jsonl` record. `conversation` is `None` for the `slash_command` records that
/// make up 31 of the 43 lines measured — they carry a workspace but no id at all.
fn history_line(workspace: &str, ts: u64, conversation: Option<&str>) -> String {
    let id = match conversation {
        Some(c) => format!(",\"conversationId\":\"{c}\""),
        None => String::new(),
    };
    format!(
        "{{\"display\":\"do a thing\",\"timestamp\":{ts},\"workspace\":\"{workspace}\",\
         \"type\":\"user_input\"{id}}}\n"
    )
}

fn write_history(root: &Path, lines: &[String]) {
    std::fs::write(antigravity::history_path_in(root), lines.concat()).expect("write history");
}

// ---- paths ------------------------------------------------------------------

#[test]
fn paths_hang_off_the_root_in_the_shape_agy_writes() {
    let r = Path::new("/agy");
    assert_eq!(
        antigravity::presence_dir_in(r),
        PathBuf::from("/agy/presence")
    );
    assert_eq!(
        antigravity::presence_lock_in(r, PARENT),
        Some(PathBuf::from(format!("/agy/presence/{PARENT}.lock")))
    );
    assert_eq!(
        antigravity::brain_dir_in(r, PARENT),
        Some(PathBuf::from(format!("/agy/brain/{PARENT}")))
    );
    assert_eq!(
        antigravity::transcript_in(r, PARENT),
        Some(PathBuf::from(format!(
            "/agy/brain/{PARENT}/.system_generated/logs/transcript.jsonl"
        ))),
        "the tailed file is transcript.jsonl — not transcript_full.jsonl, not a chunk mirror"
    );
    assert_eq!(
        antigravity::history_path_in(r),
        PathBuf::from("/agy/history.jsonl")
    );
}

#[test]
fn an_id_that_is_not_uuid_shaped_never_becomes_a_path_component() {
    // The path guard, at every entry point that joins an id. These strings arrive from a
    // kernel fd listing and another program's JSON, so "it will always be a UUID" is not a
    // property this crate may assume.
    let r = Path::new("/agy");
    for hostile in ["../../etc/passwd", "", "..", PARENT.trim_end_matches('6')] {
        assert_eq!(antigravity::brain_dir_in(r, hostile), None, "{hostile:?}");
        assert_eq!(antigravity::transcript_in(r, hostile), None, "{hostile:?}");
        assert_eq!(
            antigravity::presence_lock_in(r, hostile),
            None,
            "{hostile:?}"
        );
    }
}

#[test]
fn a_lock_basename_yields_its_conversation_id_and_nothing_else_does() {
    assert_eq!(
        antigravity::conversation_id_from_lock(Path::new(&format!("/agy/presence/{PARENT}.lock"))),
        Some(PARENT.to_string())
    );
    for not_a_lock in [
        format!("/agy/presence/{PARENT}.json"),
        "/agy/presence/scratch.lock".to_string(),
        "/agy/presence".to_string(),
        "/agy/presence/../../etc/passwd.lock".to_string(),
    ] {
        assert_eq!(
            antigravity::conversation_id_from_lock(Path::new(&not_a_lock)),
            None,
            "{not_a_lock}"
        );
    }
}

// ---- R1.1 the FD tier -------------------------------------------------------

#[test]
fn a_probed_conversation_id_resolves_its_transcript_marked_owned() {
    // R1.1 — `app` probed an open presence FD and named the conversation exactly, so this
    // resolves without consulting anything else and the attachment is confirmed.
    let r = root("probed");
    let expected = write_transcript(&r, PARENT, "2026-08-14T03:48:57Z");

    let got = antigravity::resolve_in(&r, Probed::Conversation(PARENT), Path::new("/work/proj"))
        .expect("a probed id with a transcript on disk resolves");
    assert_eq!(got.path, expected);
    assert!(got.owned, "an open FD names the conversation exactly");
    assert_eq!(
        got.last_event_ms,
        Some(1_786_679_337_000), // 2026-08-14T03:48:57Z
        "last step time comes from the step envelope's Z-suffixed created_at"
    );
}

#[test]
fn a_probed_id_does_not_fall_back_to_an_earlier_conversation() {
    // R1.6 — a freshly launched `agy` holds its lock before it writes a step. The honest
    // answer is "not written yet", NOT the conversation this workspace ran last: that one is
    // a real transcript, and attaching to it would stream live, wrong telemetry.
    let r = root("probed-cold");
    write_transcript(&r, OTHER, "2026-08-14T03:00:00Z");
    write_history(&r, &[history_line("/work/proj", 1_000, Some(OTHER))]);

    assert_eq!(
        antigravity::resolve_in(&r, Probed::Conversation(PARENT), Path::new("/work/proj")),
        None,
        "a probed conversation with no transcript is unattached, not re-pointed"
    );
}

// ---- R1.2 the lock tier proves nothing from disk ----------------------------

#[test]
fn a_stale_lock_file_alone_resolves_nothing() {
    // R1.2 — `presence/<uuid>.lock` on disk with no holder is not a session. This crate never
    // reads the presence directory at all, which is the strongest form of that: a complete,
    // fully-written transcript beside a stale lock is still unreachable without either an FD
    // or a `history.jsonl` record.
    let r = root("stale-lock");
    write_stale_lock(&r, SUBAGENT);
    write_transcript(&r, SUBAGENT, "2026-08-14T04:00:00Z");

    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")),
        None,
        "lock files outlive their sessions; existence is not attachment"
    );
}

// ---- R1.3 the history.jsonl tier -------------------------------------------

#[test]
fn history_names_the_conversation_for_this_workspace_marked_unowned() {
    // R1.3 — the fallback: `history.jsonl` pairs a conversation with an absolute workspace
    // path, and `brain/<id>/` confirms it. Unowned, because the record says this workspace
    // used that conversation — not that this process is running it.
    let r = root("history");
    let expected = write_transcript(&r, PARENT, "2026-08-14T03:48:57Z");
    write_transcript(&r, OTHER, "2026-08-14T05:00:00Z"); // a different workspace's session
    write_history(
        &r,
        &[
            history_line("/work/other", 5_000, Some(OTHER)),
            history_line("/work/proj", 1_000, None), // a slash_command record: no id
            history_line("/work/proj", 2_000, Some(PARENT)),
        ],
    );

    let got = antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj"))
        .expect("history names a conversation for this workspace");
    assert_eq!(got.path, expected);
    assert!(!got.owned, "a history record is not proof of ownership");
    assert_eq!(
        antigravity::history_conversation_in(&r, Path::new("/work/proj")),
        Some(PARENT.to_string())
    );
}

#[test]
fn the_newest_history_record_for_the_workspace_wins() {
    // Recency INSIDE `history.jsonl`, over the record's own `timestamp` — the log's account
    // of what the user did most recently here. This is the only recency in the module, and
    // it is not the one R1.4 forbids: it ranks user interactions, not files on disk.
    let r = root("history-newest");
    write_transcript(&r, PARENT, "2026-08-14T03:00:00Z");
    let newer = write_transcript(&r, OTHER, "2026-08-14T04:00:00Z");
    write_history(
        &r,
        &[
            history_line("/work/proj", 9_000, Some(OTHER)),
            history_line("/work/proj", 1_000, Some(PARENT)),
        ],
    );

    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj"))
            .map(|res| res.path)
            .as_deref(),
        Some(newer.as_path())
    );
}

#[test]
fn a_history_record_whose_brain_directory_is_gone_is_skipped() {
    // `history.jsonl` is another program's undocumented on-disk detail and it is never
    // pruned, so it names conversations whose directories have since been deleted. The next
    // candidate down takes over rather than the whole tier failing.
    let r = root("history-pruned");
    let survivor = write_transcript(&r, PARENT, "2026-08-14T03:00:00Z");
    write_history(
        &r,
        &[
            history_line("/work/proj", 1_000, Some(PARENT)),
            history_line("/work/proj", 9_000, Some(OTHER)), // newest, but brain/ is gone
        ],
    );

    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")).map(|res| res.path),
        Some(survivor)
    );
}

#[test]
fn a_history_conversation_with_no_transcript_yet_is_unattached() {
    // R1.6 again, on the fallback tier: `brain/<id>/` exists but nothing has been written
    // into it. There is nothing to tail, so the panel waits.
    let r = root("history-empty-brain");
    write_brain_dir_only(&r, PARENT);
    write_history(&r, &[history_line("/work/proj", 1_000, Some(PARENT))]);

    assert_eq!(
        antigravity::history_conversation_in(&r, Path::new("/work/proj")),
        Some(PARENT.to_string()),
        "the tier still names it — brain/<id>/ is there"
    );
    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")),
        None,
        "but with no transcript there is nothing to attach to"
    );
}

// ---- R1.4 no recency tiebreak across brain/ --------------------------------

#[test]
fn a_running_subagent_never_wins_the_fallback() {
    // R1.4 — THE test this task exists to pin, and the one most likely to be "helpfully"
    // undone later by adding a newest-transcript tiebreak.
    //
    // The setup is the real failure mode, not a contrivance: 18 of 51 transcripts on the
    // measured machine are subagent children, they carry no marker distinguishing them from
    // a top-level conversation, and a live subagent is the newest writer in `brain/`
    // *precisely* while its parent waits — which is exactly when a fallback runs. Here the
    // subagent's transcript is the newest file on disk and absent from `history.jsonl`. It
    // must never be returned, under either outcome of the tier above it.
    let r = root("subagent");
    let parent_log = write_transcript(&r, PARENT, "2026-08-14T03:00:00Z");
    let subagent_log = write_transcript(&r, SUBAGENT, "2026-08-14T09:59:59Z");
    // Written second, so it is the newest transcript in `brain/` by mtime as well as by
    // content — asserted rather than assumed, because a tiebreak this test forbids would
    // otherwise be forbidden against a setup that could not have triggered it.
    assert_eq!(
        newest_transcript_by_mtime(&r).as_deref(),
        Some(subagent_log.as_path()),
        "the child must be the newest writer, or this test proves nothing"
    );

    // (a) history names the parent: the parent wins, and being newest buys the child nothing.
    write_history(&r, &[history_line("/work/proj", 1_000, Some(PARENT))]);
    let got = antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj"))
        .expect("history names the parent");
    assert_eq!(
        got.path, parent_log,
        "the child must not displace its parent"
    );

    // (b) history names nothing for this workspace: unattached — NOT the newest transcript.
    write_history(&r, &[history_line("/work/other", 1_000, Some(OTHER))]);
    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")),
        None,
        "with no history record the answer is unattached, never the newest file in brain/"
    );
}

/// The transcript under `brain/` with the newest mtime — i.e. exactly what a reintroduced
/// recency tiebreak would return. Written here, in the test, so the rejected rule is
/// *demonstrated* rather than described: the test asserts this picks the subagent and that
/// resolution does not. It must never appear in `src/`.
///
/// Ordering comes from write order alone: APFS records nanosecond mtimes, so a file written
/// second is newer than one written first without any sleeping or clock manipulation.
fn newest_transcript_by_mtime(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for conv in std::fs::read_dir(root.join("brain")).ok()?.flatten() {
        let path = conv
            .path()
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

// ---- R1.5 / R1.6 nothing resolvable ----------------------------------------

#[test]
fn an_unknown_workspace_resolves_nothing_without_error() {
    // R1.5 — no FD, and this workspace has never appeared in `history.jsonl`. The panel shows
    // its waiting placeholder rather than adopting whatever else is on the machine.
    let r = root("unknown-workspace");
    write_transcript(&r, PARENT, "2026-08-14T03:00:00Z");
    write_history(
        &r,
        &[history_line("/work/somewhere-else", 1_000, Some(PARENT))],
    );

    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/fresh")),
        None
    );
}

#[test]
fn an_empty_or_absent_root_resolves_nothing_without_error() {
    // R1.6 — a machine that has never run `agy`, and one that has run it but written no
    // history. Neither is an error; both are "unattached".
    let r = root("empty");
    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")),
        None
    );
    assert_eq!(
        antigravity::resolve_in(&r, Probed::Conversation(PARENT), Path::new("/work/proj")),
        None
    );

    let missing = r.join("nope");
    assert_eq!(
        antigravity::resolve_in(&missing, Probed::NoAgent, Path::new("/work/proj")),
        None
    );
    assert_eq!(
        antigravity::history_conversation_in(&missing, Path::new("/work/proj")),
        None
    );
}

#[test]
fn hostile_history_input_is_survived_line_by_line() {
    // Constraints (total fault tolerance): a truncated write, a non-JSON line, a record whose
    // fields are the wrong JSON types, and an id that is not UUID-shaped. Every one is
    // skipped; the one good record still resolves, and nothing panics.
    let r = root("hostile-history");
    let expected = write_transcript(&r, PARENT, "2026-08-14T03:00:00Z");
    write_history(
        &r,
        &[
            "not json at all\n".to_string(),
            "{\"workspace\":\"/work/proj\",\"conversationId\":\"../../etc/passwd\"}\n".to_string(),
            "{\"workspace\":42,\"conversationId\":[],\"timestamp\":\"soon\"}\n".to_string(),
            format!("{{\"workspace\":\"/work/proj\",\"conversationId\":\"{OTHER}\",\"timestamp\":null}}\n"),
            history_line("/work/proj", 1_000, Some(PARENT)),
            "{\"workspace\":\"/work/proj\",\"conversa".to_string(), // truncated tail
        ],
    );

    assert_eq!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")).map(|res| res.path),
        Some(expected),
        "a good record survives every malformed one around it"
    );
}

#[test]
fn a_transcript_with_no_parseable_step_time_still_resolves() {
    // `last_event_ms` is a freshness hint, not a precondition. A transcript whose tail holds
    // no parseable `created_at` — a partial first write, a step with a local-offset time only
    // — still attaches, with the time reported as unknown rather than fabricated.
    let r = root("undated");
    let path = antigravity::transcript_in(&r, PARENT).expect("uuid");
    std::fs::create_dir_all(path.parent().expect("logs")).expect("mkdir");
    std::fs::write(
        &path,
        "{\"step_index\":0,\"content\":\"Created At: 2026-07-26T09:09:03-07:00\"}\n",
    )
    .expect("write");

    let got = antigravity::resolve_in(&r, Probed::Conversation(PARENT), Path::new("/work/proj"))
        .expect("resolves");
    assert_eq!(got.path, path);
    assert_eq!(
        got.last_event_ms, None,
        "an in-content local-offset timestamp is a different clock and must not be read"
    );
}

/// The reported defect, pinned (AG6 amended).
///
/// Reproduces the exact live sequence: the user `/exit`s conversation A, starts a new `agy`,
/// and the panel re-points during the ~7 seconds before the new process writes its presence
/// lock. Measured on the real machine — process 00:07:22, lock and transcript both 00:07:29,
/// panel 00:07:25 — the old ladder fell through to `history.jsonl`, found A's own `/exit`
/// record (the newest for that workspace) and rendered A's 11-minute-old telemetry as the
/// tab's live session.
///
/// The fix is a type: a live agent that named no conversation is `LiveButUnnamed`, and the
/// history tier is unreachable from there. Note what the old signature could not express —
/// both states were `None`.
#[test]
fn a_live_agent_never_adopts_an_older_conversation_from_history() {
    let r = root("live-vs-history");
    // The conversation the user just exited: in history, with a real transcript.
    write_transcript(&r, OTHER, "2026-08-15T06:55:53Z");
    write_history(&r, &[history_line("/work/proj", 1_000, Some(OTHER))]);
    // The conversation the tab is ACTUALLY running. Seconds old: no lock readable yet, and
    // — the structural point — no history record either, because it has run no slash command.
    write_brain_dir_only(&r, PARENT);

    assert_eq!(
        antigravity::resolve_in(&r, Probed::LiveButUnnamed, Path::new("/work/proj")),
        None,
        "a live agent whose lock is not readable yet must resolve NOTHING — every \
         candidate history can offer is necessarily an older conversation"
    );

    // The same directory state, with no agent under the tab at all, is the one case the
    // history tier is for: nothing live to be wrong about.
    assert!(
        antigravity::resolve_in(&r, Probed::NoAgent, Path::new("/work/proj")).is_some(),
        "with no agent running, history remains the legitimate fallback"
    );

    // And once the lock appears, the probe names it directly.
    write_transcript(&r, PARENT, "2026-08-15T07:07:29Z");
    let got = antigravity::resolve_in(&r, Probed::Conversation(PARENT), Path::new("/work/proj"))
        .expect("the probed conversation resolves");
    assert!(got.path.to_string_lossy().contains(PARENT), "{got:?}");
    assert!(got.owned, "a probed conversation is owned");
}

/// A transcript is identified by the directory that holds it, never by what it says (AG5).
///
/// The temptation is to scrape `conversationId` out of the file, and on the author's corpus
/// that would be worse than useless: 6 of 60 transcripts contain that key and in every one
/// it is a *child* subagent's id, spliced into an `INVOKE_SUBAGENT` result. A scraper would
/// therefore label those six sessions with a conversation that is not theirs. This pins the
/// path as the source, and the round trip through `transcript_in` is what keeps the two
/// spellings of the layout from drifting apart.
#[test]
fn a_transcripts_conversation_id_comes_from_its_directory_not_its_contents() {
    let r = root("id-from-path");
    let log = write_transcript(&r, PARENT, "2026-08-15T07:07:29Z");
    // The trap, made concrete: the file names the CHILD and nothing else.
    std::fs::write(
        &log,
        format!("{{\"content\":\"{{\\\"conversationId\\\": \\\"{SUBAGENT}\\\"}}\"}}\n"),
    )
    .expect("write");

    assert_eq!(
        antigravity::conversation_id_of(&log).as_deref(),
        Some(PARENT),
        "the id is the brain/<id>/ directory — the subagent id inside the file must not win"
    );
}

/// Only a real transcript path yields an id; anything else yields `None` rather than
/// whatever string happens to sit three levels up.
#[test]
fn a_path_that_is_not_a_transcript_names_no_conversation() {
    let r = root("id-rejects");
    let good = antigravity::transcript_in(&r, PARENT).expect("uuid-shaped id");

    for (bad, why) in [
        (
            r.join("brain").join(PARENT).join("transcript.jsonl"),
            "the two intervening dirs are absent",
        ),
        (
            r.join("brain")
                .join("not-a-uuid")
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl"),
            "the directory is not UUID-shaped",
        ),
        (
            r.join("brain")
                .join(PARENT)
                .join(".system_generated")
                .join("elsewhere")
                .join("transcript.jsonl"),
            "the parent dir is not `logs`",
        ),
        (
            PathBuf::from("transcript.jsonl"),
            "there is no parent at all",
        ),
    ] {
        assert_eq!(
            antigravity::conversation_id_of(&bad),
            None,
            "{why}: {}",
            bad.display()
        );
    }

    assert_eq!(
        antigravity::conversation_id_of(&good).as_deref(),
        Some(PARENT),
        "the canonical layout still resolves — otherwise the guards above prove nothing"
    );
}

// ---- agent-card-status: the sidebar's tail reader (R1, R2) --------------------------------
//
// `antigravity::tail_state` classifies a transcript's last meaningful step into Claude's
// `LastMessage`, so the app's unchanged `state_from` maps both agents with one rule (D10).
// Every case writes its own transcript into a temp directory; none reads the real corpus.

/// Write `bytes` as a transcript file named for `tag`, and return its path.
fn write_bytes(tag: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ai-monitor-agy-tail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(format!("{tag}.jsonl"));
    std::fs::write(&path, bytes).expect("write transcript");
    path
}

/// `steps` as `agy` appends them: one JSON object per line, `step_index` numbered in order.
fn steps_bytes(steps: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let mut s = s.clone();
        s["step_index"] = json!(i);
        out.extend_from_slice(s.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

fn write_steps(tag: &str, steps: &[Value]) -> PathBuf {
    write_bytes(tag, &steps_bytes(steps))
}

fn read(path: &Path) -> TailState {
    antigravity::tail_state(path).expect("an existing readable transcript always has a state")
}

fn last_of(tag: &str, steps: &[Value]) -> Option<LastMessage> {
    read(&write_steps(tag, steps)).last
}

/// A step of `kind` in the envelope every measured step carries.
fn step(kind: &str) -> Value {
    json!({"source": "MODEL", "type": kind, "status": "DONE",
           "created_at": "2026-09-28T07:19:00Z"})
}

fn with_status(mut s: Value, status: &str) -> Value {
    s["status"] = json!(status);
    s
}

fn user_input() -> Value {
    let mut s = step("USER_INPUT");
    s["source"] = json!("USER_EXPLICIT");
    s["content"] = json!("<USER_REQUEST>\ndo the thing\n</USER_REQUEST>");
    s
}

/// A `PLANNER_RESPONSE` dispatching `names`, in one `tool_calls` array.
fn dispatch(names: &[&str]) -> Value {
    let mut s = step("PLANNER_RESPONSE");
    s["tool_calls"] = names
        .iter()
        .map(|n| json!({"name": n, "args": {"toolAction": "\"Doing it\""}}))
        .collect::<Vec<_>>()
        .into();
    s
}

/// A tool-less `PLANNER_RESPONSE`: the model's reply.
fn reply() -> Value {
    let mut s = step("PLANNER_RESPONSE");
    s["content"] = json!("It is `ROWS_CAP` in atlas.rs.");
    s
}

/// A foreground dispatch and its result, repeated until they total at least `bytes`.
fn foreground_work(bytes: usize) -> Vec<Value> {
    let mut result = step("VIEW_FILE");
    result["content"] = json!(format!("Created At: …\n{}", "x".repeat(2000)));
    let pair = [dispatch(&["view_file"]), result];
    // Every pair serialises to the same size (bar a digit of `step_index`), so count pairs
    // rather than re-serialising the whole run on every iteration.
    let pairs = bytes.div_ceil(steps_bytes(&pair).len());
    pair.iter().cycle().take(2 * pairs).cloned().collect()
}

const RESULT_KINDS: [&str; 9] = [
    "GENERIC",
    "RUN_COMMAND",
    "VIEW_FILE",
    "LIST_DIRECTORY",
    "GREP_SEARCH",
    "CODE_ACTION",
    "FIND",
    "INVOKE_SUBAGENT",
    "ASK_QUESTION",
];

const BOOKKEEPING_KINDS: [&str; 4] = [
    "CHECKPOINT",
    "SYSTEM_MESSAGE",
    "CONVERSATION_HISTORY",
    "ERROR_MESSAGE",
];

const ALLOW_LIST: [&str; 8] = [
    "run_command",
    "view_file",
    "list_dir",
    "grep_search",
    "find_by_name",
    "write_to_file",
    "replace_file_content",
    "ask_question",
];

// ---- R1: the last meaningful step --------------------------------------------------------

#[test]
fn r1_1_a_reply_after_foreground_work_is_a_finished_turn() {
    let steps = [
        user_input(),
        dispatch(&["run_command"]),
        step("RUN_COMMAND"),
        dispatch(&["view_file"]),
        step("VIEW_FILE"),
        reply(),
    ];
    assert_eq!(
        last_of("r1-1-foreground", &steps),
        Some(LastMessage::AssistantEndTurn)
    );
    // The shortest finished turn: a question answered with no tool at all.
    assert_eq!(
        last_of("r1-1-direct", &[user_input(), reply()]),
        Some(LastMessage::AssistantEndTurn)
    );
}

/// The real-format fixture ends in a reply (step 16) whose turn, since the `USER_INPUT` at
/// step 13, dispatched only `grep_search`.
#[test]
fn r1_1_the_real_fixture_ends_in_a_finished_turn() {
    let path = write_bytes(
        "r1-1-fixture",
        include_bytes!("fixtures/antigravity_transcript.jsonl"),
    );
    assert_eq!(read(&path).last, Some(LastMessage::AssistantEndTurn));
}

#[test]
fn r1_2_an_ask_question_dispatch_asks_the_user() {
    assert_eq!(
        last_of("r1-2-ask", &[user_input(), dispatch(&["ask_question"])]),
        Some(LastMessage::AssistantAsksUser)
    );
    // "Include", not "is only": one blocking call in the batch blocks the batch, in
    // either order.
    for (tag, calls) in [
        ("r1-2-ask-last", ["run_command", "ask_question"]),
        ("r1-2-ask-first", ["ask_question", "view_file"]),
    ] {
        assert_eq!(
            last_of(tag, &[user_input(), dispatch(&calls)]),
            Some(LastMessage::AssistantAsksUser),
            "{calls:?}"
        );
    }
    // A pending question is a question whatever ran before it: D13 governs replies only.
    let after_background = [
        user_input(),
        dispatch(&["manage_task"]),
        with_status(step("GENERIC"), "RUNNING"),
        dispatch(&["ask_question"]),
    ];
    assert_eq!(
        last_of("r1-2-ask-after-bg", &after_background),
        Some(LastMessage::AssistantAsksUser)
    );
}

#[test]
fn r1_3_any_other_dispatch_is_busy() {
    // Allow-listed or not, measured or not: a dispatch is work in flight. Names that only
    // resemble the blocking tool stay busy — the match is exact.
    for name in [
        "run_command",
        "view_file",
        "manage_task",
        "invoke_subagent",
        "frobnicate",
        "ask_question_v2",
        "ASK_QUESTION",
        "Ask_Question",
    ] {
        assert_eq!(
            last_of(&format!("r1-3-{name}"), &[user_input(), dispatch(&[name])]),
            Some(LastMessage::AssistantBusy),
            "{name}"
        );
    }
    // A call whose name cannot be read is not a blocking call.
    let mut nameless = step("PLANNER_RESPONSE");
    nameless["tool_calls"] = json!([{"args": {}}]);
    assert_eq!(
        last_of("r1-3-nameless", &[user_input(), nameless]),
        Some(LastMessage::AssistantBusy)
    );
}

#[test]
fn r1_4_every_result_kind_is_a_tool_result_in_any_status() {
    for kind in RESULT_KINDS {
        for status in ["DONE", "RUNNING", "ERROR"] {
            let steps = [
                user_input(),
                dispatch(&["run_command"]),
                with_status(step(kind), status),
            ];
            assert_eq!(
                last_of(&format!("r1-4-{kind}-{status}"), &steps),
                Some(LastMessage::UserToolResult),
                "{kind} {status}"
            );
        }
    }
}

#[test]
fn r1_5_a_user_input_is_a_prompt() {
    assert_eq!(
        last_of("r1-5", &[user_input(), reply(), user_input()]),
        Some(LastMessage::UserPrompt)
    );
}

#[test]
fn r1_6_bookkeeping_after_the_deciding_step_is_skipped() {
    // All four, in the order the CLI can write them after a turn.
    let mut steps = vec![user_input(), reply()];
    steps.extend(BOOKKEEPING_KINDS.map(step));
    assert_eq!(
        last_of("r1-6-all", &steps),
        Some(LastMessage::AssistantEndTurn)
    );
    // And each on its own, so every entry of the list is load-bearing.
    for kind in BOOKKEEPING_KINDS {
        assert_eq!(
            last_of(
                &format!("r1-6-{kind}"),
                &[user_input(), reply(), step(kind)]
            ),
            Some(LastMessage::AssistantEndTurn),
            "{kind} after a reply"
        );
        assert_eq!(
            last_of(
                &format!("r1-6-dispatch-{kind}"),
                &[user_input(), dispatch(&["run_command"]), step(kind)]
            ),
            Some(LastMessage::AssistantBusy),
            "{kind} after a dispatch"
        );
    }
}

#[test]
fn r1_7_an_unlisted_step_kind_is_no_robust_signal() {
    assert_eq!(
        last_of("r1-7-alone", &[user_input(), step("SOME_NEW_KIND")]),
        None
    );
    // It decides rather than being skipped: the finished reply behind it is not reached.
    assert_eq!(
        last_of(
            "r1-7-after-reply",
            &[user_input(), reply(), step("SOME_NEW_KIND")]
        ),
        None
    );
}

#[test]
fn r1_8_a_last_line_longer_than_the_tail_is_no_robust_signal() {
    let mut long = step("RUN_COMMAND");
    long["content"] = json!("y".repeat(2 * TAIL_CAP as usize));
    let bytes = steps_bytes(&[user_input(), reply(), long]);
    let path = write_bytes("r1-8-long", &bytes);
    let state = read(&path);
    assert_eq!(state.last, None);
    assert_eq!(state.len, bytes.len() as u64);
}

#[test]
fn r1_8_an_empty_transcript_is_a_state_with_no_signal() {
    let state = read(&write_bytes("r1-8-empty", b""));
    assert_eq!(state.last, None);
    assert_eq!(state.len, 0);
}

/// The newest record is never skipped to classify an older one. A half-written line, or a
/// complete line that is not a step, is newer than the reply behind it — reading the reply
/// would be a stale Waiting.
#[test]
fn r1_8_an_unreadable_newest_line_is_not_skipped() {
    let newest: [(&str, &[u8]); 3] = [
        (
            "half-written",
            br#"{"step_index": 9, "source": "MODEL", "type": "PLANNER_RE"#,
        ),
        (
            "corrupt",
            b"{\"step_index\": 9, \"type\": \"RUN_COMMAND\"}}\n",
        ),
        ("no-kind", b"{\"step_index\": 9, \"status\": \"DONE\"}\n"),
    ];
    for (what, tail) in newest {
        for (before, expect_if_skipped) in [
            (reply(), LastMessage::AssistantEndTurn),
            (dispatch(&["run_command"]), LastMessage::AssistantBusy),
        ] {
            let mut bytes = steps_bytes(&[user_input(), before]);
            bytes.extend_from_slice(tail);
            let path = write_bytes(&format!("r1-8-{what}-{expect_if_skipped:?}"), &bytes);
            assert_eq!(
                read(&path).last,
                None,
                "{what} newest line must not fall through to {expect_if_skipped:?}"
            );
        }
    }
}

#[test]
fn blank_lines_are_not_steps() {
    let mut bytes = steps_bytes(&[user_input(), reply()]);
    bytes.extend_from_slice(b"\n  \n");
    let path = write_bytes("blank-lines", &bytes);
    assert_eq!(read(&path).last, Some(LastMessage::AssistantEndTurn));
}

#[test]
fn r1_9_the_reader_reports_mtime_len_and_never_a_user_role() {
    let mut long = step("RUN_COMMAND");
    long["content"] = json!("y".repeat(2 * TAIL_CAP as usize));
    let no_signal: [(&str, Vec<u8>); 5] = [
        ("r1-9-empty", Vec::new()),
        (
            "r1-9-unlisted",
            steps_bytes(&[user_input(), step("SOME_NEW_KIND")]),
        ),
        ("r1-9-long", steps_bytes(&[user_input(), long])),
        (
            "r1-9-background",
            steps_bytes(&[
                user_input(),
                with_status(step("RUN_COMMAND"), "RUNNING"),
                reply(),
            ]),
        ),
        ("r1-9-no-user-input", steps_bytes(&[reply()])),
    ];
    for (tag, bytes) in no_signal {
        let path = write_bytes(tag, &bytes);
        let meta = std::fs::metadata(&path).expect("stat");
        let state = read(&path);
        assert_eq!(state.last, None, "{tag}");
        // `state_from` reads `role` only now, and `"user"` would hold the tab Busy for
        // 100 s instead of D1's 10 s window.
        assert_ne!(state.role, "user", "{tag}");
        assert_eq!(state.len, bytes.len() as u64, "{tag}");
        assert_eq!(state.len, meta.len(), "{tag}");
        assert_eq!(state.mtime, meta.modified().expect("mtime"), "{tag}");
    }
    // The same facts ride along with a robust signal.
    let path = write_steps("r1-9-signal", &[user_input(), reply()]);
    let meta = std::fs::metadata(&path).expect("stat");
    let state = read(&path);
    assert_eq!(state.last, Some(LastMessage::AssistantEndTurn));
    assert_eq!(state.len, meta.len());
    assert_eq!(state.mtime, meta.modified().expect("mtime"));
}

#[test]
fn r1_9_only_an_unreadable_transcript_has_no_state() {
    assert_eq!(
        antigravity::tail_state(Path::new("/no/such/transcript.jsonl")),
        None
    );
    // Opens, but cannot be read as a file.
    let dir = std::env::temp_dir().join(format!("ai-monitor-agy-tail-dir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    assert_eq!(antigravity::tail_state(&dir), None);
}

// ---- R2: a reply after background work (D13, D14) ----------------------------------------

#[test]
fn r2_1_a_background_task_since_the_user_input_is_no_robust_signal() {
    for kind in ["RUN_COMMAND", "GENERIC"] {
        let steps = [
            user_input(),
            dispatch(&["run_command"]),
            with_status(step(kind), "RUNNING"),
            reply(),
        ];
        assert_eq!(last_of(&format!("r2-1-{kind}"), &steps), None, "{kind}");
    }
}

/// "Since the last user input" is a boundary: background work in an earlier turn does not
/// make this turn's reply ambiguous.
#[test]
fn r2_1_background_work_before_the_last_user_input_does_not_count() {
    let steps = [
        user_input(),
        dispatch(&["invoke_subagent"]),
        step("INVOKE_SUBAGENT"),
        with_status(step("RUN_COMMAND"), "RUNNING"),
        reply(),
        user_input(),
        dispatch(&["grep_search"]),
        step("GREP_SEARCH"),
        reply(),
    ];
    assert_eq!(
        last_of("r2-1-earlier-turn", &steps),
        Some(LastMessage::AssistantEndTurn)
    );
}

#[test]
fn r2_2_a_subagent_since_the_user_input_is_no_robust_signal() {
    // The result step alone, with no `invoke_subagent` dispatch beside it, so this is the
    // step-kind condition and not the allow-list's.
    assert_eq!(
        last_of(
            "r2-2-subagent",
            &[user_input(), step("INVOKE_SUBAGENT"), reply()]
        ),
        None
    );
}

/// The fixture's first reply (step 12) is the measured shape D13 exists for: a turn that
/// defined and invoked a subagent, then narrated. Its subagent has not reported back by then,
/// so it reads Busy (`agy-background-working` D1, R4.2, which amends D13's `None`).
#[test]
fn r2_2_the_real_fixtures_reply_after_an_unreported_subagent_is_busy() {
    let fixture = include_str!("fixtures/antigravity_transcript.jsonl");
    let through_step_12: String = fixture.lines().take(13).map(|l| format!("{l}\n")).collect();
    assert!(through_step_12
        .lines()
        .last()
        .is_some_and(|l| l.contains("\"step_index\": 12")));
    let path = write_bytes("r2-2-fixture", through_step_12.as_bytes());
    assert_eq!(read(&path).last, Some(LastMessage::AssistantBusy));
}

#[test]
fn r2_3_a_dispatch_outside_the_allow_list_is_no_robust_signal() {
    // The two cases R2.3 names, then every background tool measured.
    for name in [
        "manage_task",
        "frobnicate",
        "invoke_subagent",
        "define_subagent",
        "manage_subagents",
        "send_message",
        "schedule",
    ] {
        let steps = [user_input(), dispatch(&[name]), step("GENERIC"), reply()];
        assert_eq!(last_of(&format!("r2-3-{name}"), &steps), None, "{name}");
    }
    // Every call in a batch must be allow-listed, not just one of them.
    let mixed = [
        user_input(),
        dispatch(&["run_command", "manage_task"]),
        step("RUN_COMMAND"),
        step("GENERIC"),
        reply(),
    ];
    assert_eq!(last_of("r2-3-mixed", &mixed), None);
    // A call whose name cannot be read is an unmeasured tool.
    let mut nameless = step("PLANNER_RESPONSE");
    nameless["tool_calls"] = json!([{"args": {}}]);
    assert_eq!(
        last_of(
            "r2-3-nameless",
            &[user_input(), nameless, step("GENERIC"), reply()]
        ),
        None
    );
}

#[test]
fn r2_4_the_allow_list_reads_as_foreground() {
    // One dispatch of each, and nothing else.
    let mut steps = vec![user_input()];
    for name in ALLOW_LIST {
        steps.push(dispatch(&[name]));
        steps.push(step("GENERIC"));
    }
    steps.push(reply());
    assert_eq!(
        last_of("r2-4-all", &steps),
        Some(LastMessage::AssistantEndTurn)
    );
    // Each on its own, so every entry of the list is load-bearing.
    for name in ALLOW_LIST {
        let steps = [user_input(), dispatch(&[name]), step("GENERIC"), reply()];
        assert_eq!(
            last_of(&format!("r2-4-{name}"), &steps),
            Some(LastMessage::AssistantEndTurn),
            "{name}"
        );
    }
}

/// A step the back-scan cannot check cannot be ruled foreground.
#[test]
fn a_step_the_back_scan_cannot_read_is_no_robust_signal() {
    let mut bytes = steps_bytes(&[user_input(), dispatch(&["view_file"])]);
    bytes.extend_from_slice(b"{\"step_index\": 2, \"type\": \"VIEW_FI\n");
    bytes.extend_from_slice(&steps_bytes(&[reply()]));
    assert_eq!(read(&write_bytes("unreadable-in-span", &bytes)).last, None);

    let mut odd = step("PLANNER_RESPONSE");
    odd["tool_calls"] = json!({"name": "view_file"});
    assert_eq!(
        last_of(
            "odd-tool-calls-in-span",
            &[user_input(), odd, step("VIEW_FILE"), reply()]
        ),
        None
    );
}

/// `tool_calls` present but not an array: which half of the step is intact is a guess.
#[test]
fn a_planner_response_with_malformed_tool_calls_is_no_robust_signal() {
    let mut odd = step("PLANNER_RESPONSE");
    odd["tool_calls"] = json!({"name": "ask_question"});
    assert_eq!(last_of("odd-tool-calls", &[user_input(), odd]), None);
}

/// An empty `tool_calls` dispatched nothing, so it is a reply — held to D13 like any other.
#[test]
fn a_planner_response_with_empty_tool_calls_is_a_reply() {
    let mut empty = step("PLANNER_RESPONSE");
    empty["tool_calls"] = json!([]);
    assert_eq!(
        last_of("empty-tool-calls", &[user_input(), empty.clone()]),
        Some(LastMessage::AssistantEndTurn)
    );
    assert_eq!(
        last_of(
            "empty-tool-calls-after-bg",
            &[user_input(), with_status(step("GENERIC"), "RUNNING"), empty]
        ),
        None
    );
}

#[test]
fn r2_5_the_back_scan_reaches_a_user_input_beyond_the_tail() {
    let work = foreground_work(64 * 1024);
    let mut steps = vec![user_input()];
    steps.extend(work.clone());
    steps.push(reply());
    let bytes = steps_bytes(&steps);
    let from_user_input_to_end = bytes.len() - steps_bytes(&[user_input()]).len();
    assert!(
        from_user_input_to_end as u64 > TAIL_CAP && (bytes.len() as u64) < PROMPT_SCAN_CAP,
        "the user input lies beyond the tail and within the back-scan window"
    );
    assert_eq!(
        read(&write_bytes("r2-5-foreground", &bytes)).last,
        Some(LastMessage::AssistantEndTurn)
    );

    // The same span with a background task at its far end — outside the tail, so only the
    // back-scan can see it.
    let mut steps = vec![user_input(), with_status(step("RUN_COMMAND"), "RUNNING")];
    steps.extend(work);
    steps.push(reply());
    let bytes = steps_bytes(&steps);
    let after_running = bytes.len() - steps_bytes(&steps[..2]).len();
    assert!(
        after_running as u64 > TAIL_CAP,
        "the RUNNING step ends before the tail begins"
    );
    assert_eq!(read(&write_bytes("r2-5-running", &bytes)).last, None);
}

#[test]
fn r2_6_no_user_input_within_the_window_is_no_robust_signal() {
    // The user input exists, but more than PROMPT_SCAN_CAP of foreground work follows it.
    let mut steps = vec![user_input()];
    steps.extend(foreground_work(PROMPT_SCAN_CAP as usize + 16 * 1024));
    steps.push(reply());
    let after_user_input = steps_bytes(&steps).len() - steps_bytes(&steps[..1]).len();
    assert!(
        after_user_input as u64 > PROMPT_SCAN_CAP,
        "the user input ends before the back-scan window begins"
    );
    assert_eq!(last_of("r2-6-beyond-window", &steps), None);
    // And a transcript that holds none at all.
    assert_eq!(
        last_of(
            "r2-6-none",
            &[dispatch(&["view_file"]), step("VIEW_FILE"), reply()]
        ),
        None
    );
}

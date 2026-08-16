//! Headless assertions for Antigravity session resolution (`ai-monitor-antigravity` R1).
//!
//! Every case builds its own mock `~/.gemini/antigravity-cli` in a temp directory, so
//! nothing here reads the author's real corpus, no clock is consulted, and the answers are
//! the same on every machine and every run. The one thing these tests cannot build is an
//! open file descriptor held by another process — that half of the ladder is `app`'s
//! (AG11), and `crates/app/src/presence_fd.rs` tests it against a real descriptor.

use ai_monitor::antigravity::Probed;
use std::path::{Path, PathBuf};

use ai_monitor::antigravity;

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

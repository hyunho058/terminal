//! Antigravity's activity state for the sidebar card (`agent-card-status` D11).
//!
//! Claude's path ([`super::activity`]) has to *find* a session log — rank a project
//! directory, cross-check a registry — because its logs carry no owner. Antigravity names its
//! transcript exactly: the conversation whose presence lock the tab's `agy` holds open
//! ([`crate::presence_fd::conversation_of_pid`]). So this module is thin, memoised glue —
//! probe → transcript → [`ai_monitor::antigravity::tail_state`] → Claude's own
//! [`state_from`] — and "Waiting" means the same thing for both agents because it is the same
//! function deciding it (D10).
//!
//! Two costs, both bounded, because this runs per Antigravity tab per refresh:
//!
//! - **the FD probe** lists every open file of the `agy` process, so it runs at most once per
//!   [`RESOLVE_TTL_TICKS`] per pid — resolved or not, so an `agy` whose lock is not written
//!   yet costs one listing per TTL, not one per tick — and at once for a pid not seen before;
//! - **the transcript** is `stat`ed on each refresh and read only when its `(mtime, len)`
//!   moved (the rule Claude's `tail_role` applies — tab-metadata's R6.7, not this
//!   spec's).
//!
//! Only the probe's tier 1 is used — the lock — never `history.jsonl`: a live `agy` that
//! names no conversation is `LiveButUnnamed`, for which `ai_monitor::antigravity::resolve_in`
//! deliberately answers nothing (an older conversation in the same workspace would be real,
//! live, wrong). The transcript path is built with `transcript_in` rather than through
//! `resolve_in` because the latter also reads the transcript's tail for the panel's
//! `last_event_ms`, a read the card has no use for and would pay on every probe.
//!
//! **A1 (named, not enforced):** an `agy` process holds one conversation for at least one
//! TTL. If it switches conversation inside the same process, the card shows the previous
//! one's state until the next probe finds the new lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ai_monitor::activity::{LastMessage, TailState};

use super::activity::{state_from, MEMO_MAX, MEMO_TTL_TICKS, RESOLVE_TTL_TICKS};
use super::AgentState;

/// Where an Antigravity tab's state comes from. Injected, like `Deriver`'s `agent_of` and
/// `start_ms_of`, so the whole chain is drivable over a temp tree with every read counted —
/// and so no test of the worker reaches the real `~/.gemini`.
pub(super) struct Sources {
    /// `~/.gemini/antigravity-cli`, or `None` when `$HOME` is unset (then every Antigravity
    /// tab is `Unknown`, and nothing is probed).
    pub(super) root: Option<PathBuf>,
    /// The conversation id of the presence lock a pid holds open.
    pub(super) conversation_of: fn(u32) -> Option<String>,
    /// The transcript's last-meaningful-step classification.
    pub(super) tail_of: fn(&Path) -> Option<TailState>,
}

impl Sources {
    pub(super) fn real() -> Sources {
        Sources {
            root: ai_monitor::antigravity::antigravity_root(),
            conversation_of: crate::presence_fd::conversation_of_pid,
            tail_of: ai_monitor::antigravity::tail_state,
        }
    }
}

/// Per-`agy`-pid resolution and tail, reclaimed like the Claude memos (untouched for
/// [`MEMO_TTL_TICKS`], capped at [`MEMO_MAX`]).
#[derive(Default)]
pub(super) struct Memo {
    by_pid: HashMap<u32, Entry>,
}

struct Entry {
    /// The tick of the last probe; `None` before the first.
    probed_at: Option<u64>,
    /// The transcript the last probe named, if it named one that exists.
    transcript: Option<PathBuf>,
    /// The last classification of that transcript, with the `(mtime, len)` it was read at.
    tail: Option<TailState>,
    last_touch: u64,
}

impl Memo {
    fn reclaim(&mut self, tick: u64) {
        self.by_pid
            .retain(|_, e| tick.saturating_sub(e.last_touch) < MEMO_TTL_TICKS);
        while self.by_pid.len() > MEMO_MAX {
            let oldest = self
                .by_pid
                .iter()
                .min_by_key(|(_, e)| e.last_touch)
                .map(|(pid, _)| *pid);
            match oldest {
                Some(pid) => {
                    self.by_pid.remove(&pid);
                }
                None => break,
            }
        }
    }
}

/// The activity state of the `agy` at `pid`, at `tick`/`now` (`agent-card-status` D11).
///
/// `Unknown` — which the card shows as no status (D3) — whenever the session cannot be read:
/// no root, no lock yet, a lock naming a conversation with no transcript, or a transcript
/// that cannot be read. A readable transcript never yields `Unknown`: [`state_from`] falls
/// back to recency when its last step is no robust signal (D1).
pub(super) fn state(
    memo: &mut Memo,
    src: &Sources,
    pid: u32,
    tick: u64,
    now: SystemTime,
) -> AgentState {
    let Some(root) = src.root.as_deref() else {
        return AgentState::Unknown;
    };
    let e = memo.by_pid.entry(pid).or_insert(Entry {
        probed_at: None,
        transcript: None,
        tail: None,
        last_touch: tick,
    });
    e.last_touch = tick;
    let state = refresh(e, src, root, pid, tick, now);
    // After the touch, so the entry just used is the newest and never the one evicted.
    memo.reclaim(tick);
    state
}

/// One refresh of one pid's entry: re-probe when due, then classify the transcript,
/// re-reading it only when its `(mtime, len)` moved.
fn refresh(
    e: &mut Entry,
    src: &Sources,
    root: &Path,
    pid: u32,
    tick: u64,
    now: SystemTime,
) -> AgentState {
    if probe_due(e.probed_at, tick) {
        let found = (src.conversation_of)(pid)
            .and_then(|id| ai_monitor::antigravity::transcript_in(root, &id))
            .filter(|p| p.is_file());
        if found != e.transcript {
            e.tail = None; // another conversation: its predecessor's classification is moot
        }
        e.transcript = found;
        e.probed_at = Some(tick);
    }
    let Some(path) = e.transcript.as_deref() else {
        return AgentState::Unknown;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return AgentState::Unknown;
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let unchanged = e
        .tail
        .as_ref()
        .is_some_and(|t| t.mtime == mtime && t.len == meta.len());
    if !unchanged {
        e.tail = (src.tail_of)(path);
    }
    match &e.tail {
        Some(t) => state_from(t.last, &t.role, t.mtime, now),
        None => AgentState::Unknown,
    }
}

/// Whether the `agy` at `pid`'s transcript last classified as
/// [`LastMessage::AssistantAsksUser`] — its last step an `ask_question` dispatch
/// (session-overview-ui D2). A pure memo read, valid right after [`state`] refreshed the
/// entry this tick; the caller pairs it with that state being `Waiting`, so a stale tail
/// behind an unreadable transcript (state `Unknown`) never reads as asking.
pub(super) fn asks_user(memo: &Memo, pid: u32) -> bool {
    memo.by_pid
        .get(&pid)
        .and_then(|e| e.tail.as_ref())
        .is_some_and(|t| t.last == Some(LastMessage::AssistantAsksUser))
}

/// The transcript the `agy` at `pid`'s last probe named (session-overview-ui D24): the path
/// [`state`] just classified, published on graph passes for the overview's digest. A pure
/// memo read, valid right after [`state`] refreshed the entry this tick — so it costs no
/// probe and no read of its own.
pub(super) fn transcript(memo: &Memo, pid: u32) -> Option<PathBuf> {
    memo.by_pid.get(&pid).and_then(|e| e.transcript.clone())
}

/// Whether a pid last probed at `probed_at` is probed again at `tick`: at once when it never
/// was, then once every [`RESOLVE_TTL_TICKS`] — the Claude path's own re-resolution period.
fn probe_due(probed_at: Option<u64>, tick: u64) -> bool {
    probed_at.is_none_or(|at| tick.saturating_sub(at) >= RESOLVE_TTL_TICKS)
}

/// Fakes shared by this module's tests and the worker's: a temp Antigravity root, and
/// probe/read functions that count their calls on the calling thread.
///
/// Thread-local, not the global `probe` counters: every call happens on the test's own
/// thread, so a thread-local is exactly the scope wanted and needs no `serial()` lock.
#[cfg(test)]
pub(super) mod fake {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};

    use ai_monitor::activity::TailState;

    thread_local! {
        pub(in crate::tabmeta) static PROBES: Cell<u32> = const { Cell::new(0) };
        pub(in crate::tabmeta) static READS: Cell<u32> = const { Cell::new(0) };
    }

    pub(in crate::tabmeta) const CONV_C: &str = "cccccccc-1111-2222-3333-444444444444";
    pub(in crate::tabmeta) const CONV_D: &str = "dddddddd-1111-2222-3333-444444444444";

    /// A finished turn: the user asked, one foreground command ran, the model replied.
    pub(in crate::tabmeta) const FINISHED: &str = concat!(
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE"}"#,
        "\n",
        r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"run_command","args":{}}]}"#,
        "\n",
        r#"{"step_index":2,"source":"SYSTEM","type":"RUN_COMMAND","status":"DONE"}"#,
        "\n",
        r#"{"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Done."}"#,
        "\n",
    );
    /// A turn with a command dispatched and no result yet.
    pub(in crate::tabmeta) const DISPATCHED: &str = concat!(
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE"}"#,
        "\n",
        r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"run_command","args":{}}]}"#,
        "\n",
    );

    /// A turn whose last step is an `ask_question` dispatch: the model asked the user.
    pub(in crate::tabmeta) const ASKING: &str = concat!(
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE"}"#,
        "\n",
        r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"ask_question","args":{}}]}"#,
        "\n",
    );

    pub(in crate::tabmeta) fn reset() {
        PROBES.with(|c| c.set(0));
        READS.with(|c| c.set(0));
    }

    pub(in crate::tabmeta) fn probes() -> u32 {
        PROBES.with(Cell::get)
    }

    pub(in crate::tabmeta) fn reads() -> u32 {
        READS.with(Cell::get)
    }

    fn count(c: &'static std::thread::LocalKey<Cell<u32>>) {
        c.with(|c| c.set(c.get() + 1));
    }

    /// The `agy` holds conversation C's lock.
    pub(in crate::tabmeta) fn holds_c(_pid: u32) -> Option<String> {
        count(&PROBES);
        Some(CONV_C.to_string())
    }

    /// The `agy` holds conversation D's lock.
    pub(in crate::tabmeta) fn holds_d(_pid: u32) -> Option<String> {
        count(&PROBES);
        Some(CONV_D.to_string())
    }

    /// The `agy` holds no readable lock yet (`LiveButUnnamed`).
    pub(in crate::tabmeta) fn holds_none(_pid: u32) -> Option<String> {
        count(&PROBES);
        None
    }

    /// The real classifier, counted.
    pub(in crate::tabmeta) fn read(path: &Path) -> Option<TailState> {
        count(&READS);
        ai_monitor::antigravity::tail_state(path)
    }

    /// A fresh Antigravity root under the temp dir holding `transcripts` as
    /// `(conversation id, contents)`.
    pub(in crate::tabmeta) fn root(tag: &str, transcripts: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tabmeta-ag-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir root");
        for (id, text) in transcripts {
            let path = transcript(&root, id);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir logs");
            std::fs::write(&path, text).expect("write transcript");
        }
        root
    }

    pub(in crate::tabmeta) fn transcript(root: &Path, id: &str) -> PathBuf {
        ai_monitor::antigravity::transcript_in(root, id).expect("a UUID-shaped id")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::fake::{self, CONV_C, CONV_D, DISPATCHED, FINISHED};
    use super::*;

    fn sources(root: &Path, conversation_of: fn(u32) -> Option<String>) -> Sources {
        Sources {
            root: Some(root.to_path_buf()),
            conversation_of,
            tail_of: fake::read,
        }
    }

    /// `now` a few seconds after the transcript's own `mtime`, so the recency fallback is
    /// inside its busy window whatever the file system's clock did.
    fn soon_after(path: &Path, secs: u64) -> SystemTime {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .expect("a written transcript has an mtime");
        mtime + Duration::from_secs(secs)
    }

    #[test]
    fn the_lock_names_the_transcript_read() {
        // R4.1 — two conversations in one root, and `history.jsonl` naming D as this
        // workspace's newest; the one the pid's lock names decides, both ways round. C's turn
        // is over (Waiting); D has a command dispatched (Busy).
        fake::reset();
        let root = fake::root("lock", &[(CONV_C, FINISHED), (CONV_D, DISPATCHED)]);
        std::fs::write(
            ai_monitor::antigravity::history_path_in(&root),
            format!(
                "{{\"workspace\":\"/proj/x\",\"conversationId\":\"{CONV_C}\",\"timestamp\":1}}\n\
                 {{\"workspace\":\"/proj/x\",\"conversationId\":\"{CONV_D}\",\"timestamp\":9}}\n"
            ),
        )
        .expect("write history");
        let now = soon_after(&fake::transcript(&root, CONV_C), 1);
        let mut memo = Memo::default();
        assert_eq!(
            state(&mut memo, &sources(&root, fake::holds_c), 7, 0, now),
            AgentState::Waiting
        );
        let mut memo = Memo::default();
        assert_eq!(
            state(&mut memo, &sources(&root, fake::holds_d), 7, 0, now),
            AgentState::Busy
        );
    }

    #[test]
    fn an_unreadable_session_is_unknown() {
        // R4.2 — no lock yet, a lock naming a conversation with no transcript, and no root:
        // each is `Unknown`, and with no root nothing is even probed.
        fake::reset();
        let now = SystemTime::now();
        let root = fake::root("unknown", &[(CONV_D, DISPATCHED)]);
        let mut memo = Memo::default();
        assert_eq!(
            state(&mut memo, &sources(&root, fake::holds_none), 7, 0, now),
            AgentState::Unknown,
            "no lock"
        );
        let mut memo = Memo::default();
        assert_eq!(
            state(&mut memo, &sources(&root, fake::holds_c), 7, 0, now),
            AgentState::Unknown,
            "C has no transcript"
        );
        assert_eq!(fake::reads(), 0, "nothing to read");
        fake::reset();
        let no_root = Sources {
            root: None,
            conversation_of: fake::holds_c,
            tail_of: fake::read,
        };
        let mut memo = Memo::default();
        assert_eq!(state(&mut memo, &no_root, 7, 0, now), AgentState::Unknown);
        assert_eq!(fake::probes(), 0, "no root, no probe");
    }

    #[test]
    fn the_probe_runs_once_per_ttl_resolved_or_not() {
        // R4.3 — at the first refresh, then exactly every RESOLVE_TTL_TICKS; an `agy` with no
        // lock yet is not re-probed every tick either.
        for (what, conversation_of) in [
            ("resolving", fake::holds_c as fn(u32) -> Option<String>),
            ("no lock", fake::holds_none),
        ] {
            fake::reset();
            let root = fake::root("ttl", &[(CONV_C, FINISHED)]);
            let src = sources(&root, conversation_of);
            let mut memo = Memo::default();
            let now = SystemTime::now();
            let mut probed_on = Vec::new();
            for tick in 0..3 * RESOLVE_TTL_TICKS {
                let before = fake::probes();
                let _ = state(&mut memo, &src, 7, tick, now);
                if fake::probes() > before {
                    probed_on.push(tick);
                }
            }
            assert_eq!(
                probed_on,
                vec![0, RESOLVE_TTL_TICKS, 2 * RESOLVE_TTL_TICKS],
                "{what}"
            );
        }
    }

    #[test]
    fn a_new_pid_is_probed_at_once() {
        // R4.4 — pid 7's probe is one tick old; the tab's `agy` is now pid 8.
        fake::reset();
        let root = fake::root("newpid", &[(CONV_C, FINISHED)]);
        let src = sources(&root, fake::holds_c);
        let mut memo = Memo::default();
        let now = SystemTime::now();
        let _ = state(&mut memo, &src, 7, 0, now);
        let _ = state(&mut memo, &src, 8, 1, now);
        assert_eq!(fake::probes(), 2);
    }

    #[test]
    fn an_unchanged_transcript_is_not_reread() {
        // R4.5 — the second refresh reuses the classification; an append is read.
        fake::reset();
        let root = fake::root("reread", &[(CONV_C, DISPATCHED)]);
        let path = fake::transcript(&root, CONV_C);
        let src = sources(&root, fake::holds_c);
        let mut memo = Memo::default();
        let now = soon_after(&path, 1);
        assert_eq!(state(&mut memo, &src, 7, 0, now), AgentState::Busy);
        assert_eq!(state(&mut memo, &src, 7, 1, now), AgentState::Busy);
        assert_eq!(fake::reads(), 1, "unchanged → no second read");

        // The turn finishes: the file grows, so it is read again and the state follows.
        std::fs::write(&path, FINISHED).expect("rewrite");
        let now = soon_after(&path, 1);
        assert_eq!(state(&mut memo, &src, 7, 2, now), AgentState::Waiting);
        assert_eq!(fake::reads(), 2);
    }

    #[test]
    fn a_step_with_no_robust_signal_falls_back_to_recency_never_waiting() {
        // R3.2 — Claude's `state_from`, unchanged: Busy within 10 s of the last write, then
        // Idle, and past STALE_LIMIT still Idle — never Waiting, never Unknown.
        fake::reset();
        let unlisted = concat!(
            r#"{"step_index":0,"source":"SYSTEM","type":"SOME_NEW_KIND","status":"DONE"}"#,
            "\n"
        );
        let root = fake::root("recency", &[(CONV_C, unlisted)]);
        let path = fake::transcript(&root, CONV_C);
        let src = sources(&root, fake::holds_c);
        let mut memo = Memo::default();
        for (secs, want) in [
            (5, AgentState::Busy),
            (50, AgentState::Idle),
            (200, AgentState::Idle),
        ] {
            assert_eq!(
                state(&mut memo, &src, 7, 0, soon_after(&path, secs)),
                want,
                "{secs}s"
            );
        }
    }

    /// Two transcripts of the **same byte length** that classify differently: a finished
    /// turn (Waiting) and a dispatched command (Busy). What makes a stale memo visible — a
    /// `(mtime, len)` key that happens to match cannot tell them apart; only a re-read can.
    fn same_length_pair() -> (String, String) {
        let busy = DISPATCHED.to_string();
        let head = concat!(
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE"}"#,
            "\n",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":""#,
        );
        let tail = "\"}\n";
        let pad = busy.len() - head.len() - tail.len();
        let reply = format!("{head}{}{tail}", "x".repeat(pad));
        assert_eq!(reply.len(), busy.len());
        (reply, busy)
    }

    /// Set `path`'s mtime, so a test controls the `(mtime, len)` key exactly.
    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(t))
            .expect("set mtime");
    }

    #[test]
    fn switching_conversation_drops_the_old_classification() {
        // A1's recovery half — when a re-probe names another conversation, the next state is
        // that transcript's, even when its (mtime, len) equals the old one's exactly.
        fake::reset();
        let (reply, busy) = same_length_pair();
        let root = fake::root("switch", &[(CONV_C, &reply), (CONV_D, &busy)]);
        let (c, d) = (
            fake::transcript(&root, CONV_C),
            fake::transcript(&root, CONV_D),
        );
        let t = SystemTime::now() - Duration::from_secs(1);
        set_mtime(&c, t);
        set_mtime(&d, t);
        let now = t + Duration::from_secs(1);
        let mut memo = Memo::default();
        assert_eq!(
            state(&mut memo, &sources(&root, fake::holds_c), 7, 0, now),
            AgentState::Waiting
        );
        assert_eq!(
            state(
                &mut memo,
                &sources(&root, fake::holds_d),
                7,
                RESOLVE_TTL_TICKS,
                now
            ),
            AgentState::Busy,
            "D's own state, not C's classification reused"
        );
    }

    #[test]
    fn either_half_of_the_key_moving_is_a_change() {
        // R4.5's other side — the memo is reused only when BOTH mtime and len are unchanged:
        // a rewrite to the same length (a new mtime), and a rewrite to another length under
        // the old mtime, are each read again.
        fake::reset();
        let (reply, busy) = same_length_pair();
        let root = fake::root("key", &[(CONV_C, &busy)]);
        let path = fake::transcript(&root, CONV_C);
        let src = sources(&root, fake::holds_c);
        let t0 = SystemTime::now() - Duration::from_secs(5);
        set_mtime(&path, t0);
        let now = t0 + Duration::from_secs(1);
        let mut memo = Memo::default();
        assert_eq!(state(&mut memo, &src, 7, 0, now), AgentState::Busy);

        std::fs::write(&path, &reply).expect("same length");
        set_mtime(&path, t0 + Duration::from_secs(1));
        assert_eq!(
            state(&mut memo, &src, 7, 1, now),
            AgentState::Waiting,
            "new mtime"
        );

        std::fs::write(&path, DISPATCHED.trim_end()).expect("another length");
        set_mtime(&path, t0 + Duration::from_secs(1));
        assert_eq!(
            state(&mut memo, &src, 7, 2, now),
            AgentState::Busy,
            "new len"
        );
        assert_eq!(fake::reads(), 3);
    }

    #[test]
    fn every_classification_reaches_the_card_through_state_from() {
        // R3.1 — each last step, through this module into Claude's unchanged `state_from`:
        // a finished turn and an `ask_question` dispatch hand the turn to the user; a
        // dispatch, a tool result and the user's input are the agent's to act on.
        fake::reset();
        let user =
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE"}"#;
        let ask = r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"ask_question","args":{}}]}"#;
        let result = r#"{"step_index":2,"source":"SYSTEM","type":"RUN_COMMAND","status":"DONE"}"#;
        for (what, text, want) in [
            ("a finished turn", FINISHED.to_string(), AgentState::Waiting),
            (
                "an ask_question dispatch",
                format!("{user}\n{ask}\n"),
                AgentState::Waiting,
            ),
            ("a dispatch", DISPATCHED.to_string(), AgentState::Busy),
            (
                "a tool result",
                format!("{DISPATCHED}{result}\n"),
                AgentState::Busy,
            ),
            ("the user's input", format!("{user}\n"), AgentState::Busy),
        ] {
            let root = fake::root("every", &[(CONV_C, &text)]);
            let now = soon_after(&fake::transcript(&root, CONV_C), 1);
            let mut memo = Memo::default();
            assert_eq!(
                state(&mut memo, &sources(&root, fake::holds_c), 7, 0, now),
                want,
                "{what}"
            );
        }
    }

    #[test]
    fn nothing_under_the_root_is_written() {
        // R9.2 — the tree's listing, every file's bytes and every mtime are identical after
        // the probe and the reads.
        fn listing(root: &Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
            let mut out = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("readable") {
                    let path = entry.expect("an entry").path();
                    let meta = std::fs::metadata(&path).expect("stat");
                    if meta.is_dir() {
                        stack.push(path.clone());
                    }
                    let bytes = if meta.is_file() {
                        std::fs::read(&path).expect("read")
                    } else {
                        Vec::new()
                    };
                    out.push((path, bytes, meta.modified().expect("mtime")));
                }
            }
            out.sort();
            out
        }
        fake::reset();
        let root = fake::root("readonly", &[(CONV_C, FINISHED)]);
        // The rest of the tree R9.2's Given names: a presence lock and the history log.
        let presence = ai_monitor::antigravity::presence_dir_in(&root);
        std::fs::create_dir_all(&presence).expect("mkdir presence");
        std::fs::write(presence.join(format!("{CONV_C}.lock")), b"").expect("write lock");
        std::fs::write(
            ai_monitor::antigravity::history_path_in(&root),
            format!(
                "{{\"workspace\":\"/proj/x\",\"conversationId\":\"{CONV_C}\",\"timestamp\":1}}\n"
            ),
        )
        .expect("write history");
        let before = listing(&root);
        let src = sources(&root, fake::holds_c);
        let mut memo = Memo::default();
        for tick in 0..2 * RESOLVE_TTL_TICKS {
            let _ = state(&mut memo, &src, 7, tick, SystemTime::now());
        }
        assert_eq!(listing(&root), before);
    }

    #[test]
    fn untouched_pids_are_reclaimed_and_the_memo_is_capped() {
        // The memo cannot grow without bound: an `agy` that exited is dropped once untouched
        // for MEMO_TTL_TICKS (and not a tick earlier), and at most MEMO_MAX pids are held,
        // the least recently touched evicted first.
        fake::reset();
        let root = fake::root("reclaim", &[(CONV_C, FINISHED)]);
        let src = sources(&root, fake::holds_c);
        let now = SystemTime::now();
        for (tick, kept) in [(MEMO_TTL_TICKS - 1, true), (MEMO_TTL_TICKS, false)] {
            let mut memo = Memo::default();
            let _ = state(&mut memo, &src, 7, 0, now);
            let _ = state(&mut memo, &src, 8, tick, now);
            assert_eq!(memo.by_pid.contains_key(&7), kept, "tick {tick}");
            assert!(memo.by_pid.contains_key(&8));
        }

        let mut big = Memo::default();
        let last = MEMO_MAX as u32 + 9;
        for pid in 0..=last {
            let _ = state(&mut big, &src, pid, u64::from(pid), now);
        }
        assert_eq!(big.by_pid.len(), MEMO_MAX);
        assert!(big.by_pid.contains_key(&last), "the pid just used survives");
        assert!(big.by_pid.contains_key(&10), "the oldest kept");
        assert!(!big.by_pid.contains_key(&9), "the last of the evicted");
    }
}

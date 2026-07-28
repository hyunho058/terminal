//! Agent *activity* — busy vs. waiting — for a detected Claude agent (D7). A separate
//! axis from presence (D5): the process table says *whether* an agent is running, the
//! session JSONL says *what it is doing*.
//!
//! The busy/idle verdict is app policy, so [`state_from`] is a pure function here, not in
//! `ai-monitor` (which only supplies the raw [`TailState`]). Two cwd-keyed memos keep the
//! steady state cheap (R6.7/R6.8): the **log path** is memoised and re-resolved every
//! [`RESOLVE_TTL_TICKS`] (a new session makes a new file), and the **tail role** is
//! memoised on the log's `(mtime, len)` so an idle log is read once per append — one
//! `stat` per tick, no read (R6.7). Both are shared by cwd, so two tabs in one project
//! share them (D9's rule), and reclaimed by disuse.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ai_monitor::activity::{tail_state, LastMessage};

use super::AgentState;

/// A log appended-to within this window reads `Busy` (D7).
const BUSY_WINDOW: Duration = Duration::from_secs(10);
/// The role clause fires only while the log is younger than this (D7) — so an abandoned
/// session whose last line is a `user` tool result does not read `Busy` forever (R6.10).
const STALE_LIMIT: Duration = Duration::from_secs(100);
/// A memoised log path is re-resolved after this many ticks (≈10 s), so a newly-started
/// session is picked up (R6.8).
const RESOLVE_TTL_TICKS: u64 = 20;
/// An entry untouched for this many ticks (≈60 s) is reclaimed.
const MEMO_TTL_TICKS: u64 = 120;
/// Hard cap on each memo; oldest-touched evicted first.
const MEMO_MAX: usize = 64;

/// Pure: the activity rule (D7 + D16). The `last` meaningful-message classification is the
/// precise input-waiting signal and wins when present. TWO classifications mean the agent
/// has handed control to the human and so map to `Waiting`: an assistant `end_turn` (the
/// turn is over) and an assistant `AssistantAsksUser` (it called a blocking tool such as
/// `AskUserQuestion` and cannot proceed until answered). Every other classification →
/// `Busy`. When there is no robust `last` signal (no meaningful line, or the newest record
/// was truncated by the tail cap) we fall back to the existing recency rule — `Busy` if the
/// log was appended within `BUSY_WINDOW`, **or** the last line's role is `"user"` and
/// younger than `STALE_LIMIT`, else `Idle`. The fallback never yields `Waiting`: a false
/// "waiting" is the worst failure (conservative bias, R3.x quality bar).
pub(crate) fn state_from(
    last: Option<LastMessage>,
    role: &str,
    mtime: SystemTime,
    now: SystemTime,
) -> AgentState {
    match last {
        // Both precise signals mean "the human owns the next move" (D16 → R3.1).
        Some(LastMessage::AssistantEndTurn) | Some(LastMessage::AssistantAsksUser) => {
            return AgentState::Waiting
        }
        Some(LastMessage::AssistantBusy)
        | Some(LastMessage::UserToolResult)
        | Some(LastMessage::UserPrompt) => return AgentState::Busy, // D16 → R3.2/R3.3/R3.4
        None => {} // no robust signal → recency fallback (never Waiting)
    }
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    if age <= BUSY_WINDOW {
        return AgentState::Busy; // recency clause
    }
    if role == "user" && age < STALE_LIMIT {
        return AgentState::Busy; // unanswered-turn clause, bounded
    }
    AgentState::Idle
}

/// The two cwd-keyed activity memos (D7).
pub(crate) struct ActivityMemo {
    path: HashMap<PathBuf, PathEntry>,
    role: HashMap<PathBuf, RoleEntry>,
}

struct PathEntry {
    log: Option<PathBuf>,
    resolved_at: u64,
    last_read_tick: u64,
}

struct RoleEntry {
    log: PathBuf,
    mtime: SystemTime,
    len: u64,
    role: String,
    last: Option<LastMessage>,
    last_read_tick: u64,
    /// The last *substantive* user-prompt label derived for this cwd (tab-identity, D10).
    /// Persisted across reads: when a newer tail has no substantive prompt (only short /
    /// continuation turns, or the prompt scrolled past the tail cap) the previous label is
    /// carried forward, so an idle agent keeps its last label (D3/D6). `None` only until the
    /// first substantive prompt is seen.
    label: Option<String>,
}

impl ActivityMemo {
    pub(crate) fn new() -> ActivityMemo {
        ActivityMemo {
            path: HashMap::new(),
            role: HashMap::new(),
        }
    }

    fn reclaim(&mut self, tick: u64) {
        self.path
            .retain(|_, e| tick.saturating_sub(e.last_read_tick) < MEMO_TTL_TICKS);
        self.role
            .retain(|_, e| tick.saturating_sub(e.last_read_tick) < MEMO_TTL_TICKS);
        evict_oldest(&mut self.path, |e| e.last_read_tick);
        evict_oldest(&mut self.role, |e| e.last_read_tick);
    }
}

fn evict_oldest<V>(map: &mut HashMap<PathBuf, V>, key: impl Fn(&V) -> u64) {
    while map.len() > MEMO_MAX {
        let oldest = map
            .iter()
            .min_by_key(|(_, v)| key(v))
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
}

/// The activity state for a Claude agent whose session lives under `projects_root` for
/// `cwd`, at `tick`/`now` (D7). `Unknown` when the log cannot be resolved or read (R6.5)
/// — the one thing that state still asserts is presence, which the consumer must honor
/// (R6.6). Entry point; does not touch `worker.rs`.
pub(crate) fn state_of_cwd(
    memo: &mut ActivityMemo,
    projects_root: &Path,
    cwd: &Path,
    tick: u64,
    now: SystemTime,
) -> AgentState {
    let out = match resolve_path(memo, projects_root, cwd, tick) {
        Some(log) => match tail_role(memo, cwd, &log, tick) {
            Some((last, role, mtime)) => state_from(last, &role, mtime, now),
            None => AgentState::Unknown,
        },
        None => AgentState::Unknown,
    };
    memo.reclaim(tick);
    out
}

/// The resolved log path for `cwd`, memoised and re-resolved every `RESOLVE_TTL_TICKS`
/// (R6.8). The `read_dir` inside `resolve_log_in` is the expensive part, so it is counted
/// (`LOG_RESOLVE`) and skipped while the memo is fresh.
fn resolve_path(
    memo: &mut ActivityMemo,
    projects_root: &Path,
    cwd: &Path,
    tick: u64,
) -> Option<PathBuf> {
    if let Some(e) = memo.path.get_mut(cwd) {
        e.last_read_tick = tick;
        if tick.saturating_sub(e.resolved_at) < RESOLVE_TTL_TICKS {
            return e.log.clone();
        }
    }
    #[cfg(test)]
    super::probe::LOG_RESOLVE.bump(1);
    let log = ai_monitor::resolve_log_in(projects_root, cwd);
    memo.path.insert(
        cwd.to_path_buf(),
        PathEntry {
            log: log.clone(),
            resolved_at: tick,
            last_read_tick: tick,
        },
    );
    log
}

/// The tail role + D16 `last` signal for `log`, memoised on `(mtime, len)` (R6.7). A cheap
/// `stat` revalidates; only a changed log triggers the bounded tail read. On a changed log
/// the tab-identity `label` is re-derived from the fresh prompts (D10) and carried forward
/// when none is substantive (D6); it lands in the memo so [`label_of_cwd`] can read it with
/// no further I/O.
fn tail_role(
    memo: &mut ActivityMemo,
    cwd: &Path,
    log: &Path,
    tick: u64,
) -> Option<(Option<LastMessage>, String, SystemTime)> {
    let (mtime, len) = stat_log(log)?;
    if let Some(e) = memo.role.get_mut(cwd) {
        e.last_read_tick = tick;
        if e.log == *log && e.mtime == mtime && e.len == len {
            return Some((e.last, e.role.clone(), e.mtime)); // unchanged → no read (R6.7)
        }
    }
    // Carry the previous label forward if the fresh scan has no substantive prompt (D6/D10).
    let prev_label = memo.role.get(cwd).and_then(|e| e.label.clone());
    let ts = tail_state(log)?;
    // The label needs the last *human* prompt, which routinely sits far outside the 8 KiB
    // activity tail (a single assistant turn + tool output exceeds it), so it gets a separate,
    // wider read (up to PROMPT_SCAN_CAP) — only here, on a changed log (still memoised, R6.7).
    let label = pick_label(&ai_monitor::activity::recent_prompts(log)).or(prev_label);
    #[cfg(test)]
    {
        super::probe::LOG_READ.bump(1);
        // Honest wrapper accounting (R6.1): the activity tail is bounded to TAIL_CAP; the
        // label scan adds up to PROMPT_SCAN_CAP more, both only on a changed log.
        super::probe::LOG_READ_BYTES.bump(
            len.min(ai_monitor::activity::TAIL_CAP)
                + len.min(ai_monitor::activity::PROMPT_SCAN_CAP),
        );
    }
    memo.role.insert(
        cwd.to_path_buf(),
        RoleEntry {
            log: log.to_path_buf(),
            mtime: ts.mtime,
            len: ts.len,
            role: ts.role.clone(),
            last: ts.last,
            last_read_tick: tick,
            label,
        },
    );
    Some((ts.last, ts.role, ts.mtime))
}

/// The memoised tab-identity label for `cwd`, if one has been derived (tab-identity, D6).
/// Pure memo read — no filesystem I/O — so the worker gets the label for free after the
/// activity pass warmed the role memo this tick. `None` before any substantive prompt was
/// seen, or when the cwd's log never resolved (→ the card falls back to the cwd title).
pub(crate) fn label_of_cwd(memo: &ActivityMemo, cwd: &Path) -> Option<String> {
    memo.role.get(cwd).and_then(|e| e.label.clone())
}

/// Minimum character count for a prompt to count as substantive (D10). Below this it reads
/// as a short acknowledgement ("y", "네", "go") rather than a task, so it is skipped.
const MIN_LABEL_CHARS: usize = 8;

/// The tab label from the recent prompts (newest first): the newest *substantive* one (D10).
/// Short prompts and known continuation words are skipped so a run of "계속"/"y" does not
/// collapse every tab to the same label; when none qualifies the caller carries the previous
/// label forward. App policy — not `ai-monitor`'s — so it lives here beside [`state_from`].
fn pick_label(prompts: &[String]) -> Option<String> {
    prompts.iter().find(|p| is_substantive(p)).cloned()
}

/// A prompt is substantive when it is at least [`MIN_LABEL_CHARS`] long and is not a bare
/// continuation word (D10).
fn is_substantive(s: &str) -> bool {
    let t = s.trim();
    t.chars().count() >= MIN_LABEL_CHARS && !is_continuation(t)
}

/// Whether `s` is a bare continuation / acknowledgement, so it should not become a label
/// (D10). Whole-string, case-insensitive match against a small dictionary; the initial set
/// is tunable. Length-based skipping already handles the short ones — this catches the
/// longer continuation words ("continue").
fn is_continuation(s: &str) -> bool {
    const WORDS: &[&str] = &[
        "계속",
        "continue",
        "y",
        "yes",
        "네",
        "응",
        "다시",
        "go",
        "ok",
        "okay",
        "진행",
        "sure",
        "proceed",
        "next",
        "continue.",
    ];
    let lower = s.trim().to_lowercase();
    WORDS.contains(&lower.as_str())
}

/// `stat` the log for `(mtime, len)`, counting the revalidation probe.
fn stat_log(log: &Path) -> Option<(SystemTime, u64)> {
    #[cfg(test)]
    super::probe::LOG_STAT.bump(1);
    let meta = std::fs::metadata(log).ok()?;
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Some((mtime, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(mtime: SystemTime, secs: u64) -> SystemTime {
        mtime + Duration::from_secs(secs)
    }

    // ---- the recency fallback rule (`last == None`) (R6.2, R6.3, R6.4, R6.10, R11.4) ----

    #[test]
    fn state_from_recency_clause() {
        // R6.2 — appended 2 s ago → Busy regardless of role.
        let m = SystemTime::UNIX_EPOCH;
        assert_eq!(state_from(None, "assistant", m, at(m, 2)), AgentState::Busy);
    }

    #[test]
    fn state_from_unanswered_user_turn() {
        // R6.3 — 60 s old but role user → Busy (the role clause).
        let m = SystemTime::UNIX_EPOCH;
        assert_eq!(state_from(None, "user", m, at(m, 60)), AgentState::Busy);
    }

    #[test]
    fn state_from_settled_assistant_is_idle() {
        // R6.4 — 60 s old assistant → Idle (earns the ready mark).
        let m = SystemTime::UNIX_EPOCH;
        assert_eq!(
            state_from(None, "assistant", m, at(m, 60)),
            AgentState::Idle
        );
    }

    #[test]
    fn state_from_role_clause_is_bounded() {
        // R6.10 — user role at 200 s (> STALE_LIMIT) → Idle; at 90 s → still Busy.
        let m = SystemTime::UNIX_EPOCH;
        assert_eq!(state_from(None, "user", m, at(m, 200)), AgentState::Idle);
        assert_eq!(state_from(None, "user", m, at(m, 90)), AgentState::Busy);
    }

    // ---- the D16 input-waiting rule (R3.1–R3.5) ----

    #[test]
    fn state_from_end_turn_is_waiting() {
        // R3.1 — last meaningful message is assistant end_turn → Waiting. The D16 signal
        // wins over recency: even a just-appended log reads Waiting, not Busy.
        let m = SystemTime::UNIX_EPOCH;
        assert_eq!(
            state_from(
                Some(LastMessage::AssistantEndTurn),
                "assistant",
                m,
                at(m, 2)
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn state_from_asks_user_is_waiting() {
        // The agent called a blocking tool (AskUserQuestion / ExitPlanMode) — control is
        // with the human, so this is the second precise Waiting signal. Like end_turn it
        // beats recency: a just-appended log still reads Waiting, not Busy.
        let m = SystemTime::UNIX_EPOCH;
        for age in [0, 2, 60, 300] {
            assert_eq!(
                state_from(
                    Some(LastMessage::AssistantAsksUser),
                    "assistant",
                    m,
                    at(m, age)
                ),
                AgentState::Waiting,
                "AssistantAsksUser → Waiting at age {age}s"
            );
        }
    }

    #[test]
    fn state_from_busy_signals_are_busy_not_waiting() {
        // R3.2/R3.3/R3.4 — tool_use, tool_result, and a fresh human prompt all → Busy,
        // even when the log is old enough that the recency fallback would say Idle.
        let m = SystemTime::UNIX_EPOCH;
        for last in [
            LastMessage::AssistantBusy,
            LastMessage::UserToolResult,
            LastMessage::UserPrompt,
        ] {
            assert_eq!(
                state_from(Some(last), "assistant", m, at(m, 300)),
                AgentState::Busy,
                "{last:?} → Busy"
            );
        }
    }

    #[test]
    fn state_from_none_never_reports_waiting() {
        // Conservative bias — no robust signal falls back to recency, which never yields
        // Waiting (a false "waiting" is the worst failure).
        let m = SystemTime::UNIX_EPOCH;
        for age in [0, 2, 60, 200] {
            assert_ne!(
                state_from(None, "assistant", m, at(m, age)),
                AgentState::Waiting
            );
            assert_ne!(state_from(None, "user", m, at(m, age)), AgentState::Waiting);
        }
    }

    // ---- the memoised path (R6.7, R6.8, R6.5) with hand-rolled temp dirs (R11.2) ----

    fn projects_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tabmeta-act-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir root");
        p
    }

    /// Create the `<root>/<encoded-cwd>/<name>.jsonl` a Claude session would.
    fn write_log(root: &Path, cwd: &Path, name: &str, body: &[u8]) -> PathBuf {
        let dir = root.join(ai_monitor::encode_cwd(cwd));
        std::fs::create_dir_all(&dir).expect("mkdir project");
        let p = dir.join(format!("{name}.jsonl"));
        std::fs::write(&p, body).expect("write log");
        p
    }

    #[test]
    fn unresolvable_log_is_unknown() {
        // R6.5 — a cwd with no session dir → Unknown.
        let _g = super::super::probe::serial();
        let root = projects_root("unknown");
        let cwd = Path::new("/some/project/x");
        let mut memo = ActivityMemo::new();
        assert_eq!(
            state_of_cwd(&mut memo, &root, cwd, 0, SystemTime::now()),
            AgentState::Unknown
        );
    }

    #[test]
    fn unchanged_log_is_not_reread() {
        // R6.7 — a warm, unchanged log costs one stat, no read, no resolve.
        let _g = super::super::probe::serial();
        let root = projects_root("warm");
        let cwd = Path::new("/proj/warm");
        write_log(
            &root,
            cwd,
            "s1",
            br#"{"type":"assistant","message":{"role":"assistant"}}"#,
        );
        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        // First pass warms both memos.
        let _ = state_of_cwd(&mut memo, &root, cwd, 0, now);
        super::super::probe::reset_all();
        let _ = state_of_cwd(&mut memo, &root, cwd, 1, now);
        assert_eq!(
            super::super::probe::LOG_STAT.get(),
            1,
            "one revalidation stat"
        );
        assert_eq!(super::super::probe::LOG_READ.get(), 0, "no re-read");
        assert_eq!(super::super::probe::LOG_RESOLVE.get(), 0, "no re-resolve");
    }

    #[test]
    fn log_path_memo_expires() {
        // R6.8 — path resolved within RESOLVE_TTL_TICKS reuses; past it re-resolves.
        let _g = super::super::probe::serial();
        let root = projects_root("expire");
        let cwd = Path::new("/proj/expire");
        write_log(
            &root,
            cwd,
            "s1",
            br#"{"type":"assistant","message":{"role":"assistant"}}"#,
        );
        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = state_of_cwd(&mut memo, &root, cwd, 0, now);
        // A few ticks later: reuse, no resolve.
        super::super::probe::reset_all();
        let _ = state_of_cwd(&mut memo, &root, cwd, 6, now);
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "still fresh → reuse"
        );
        // Past the TTL: re-resolve.
        super::super::probe::reset_all();
        let _ = state_of_cwd(&mut memo, &root, cwd, RESOLVE_TTL_TICKS + 1, now);
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            1,
            "expired → re-resolve"
        );
    }

    #[test]
    fn busy_log_reads_busy_end_to_end() {
        // A freshly-written assistant log with no stop_reason → Busy (D16 AssistantBusy).
        let _g = super::super::probe::serial();
        let root = projects_root("busy");
        let cwd = Path::new("/proj/busy");
        write_log(
            &root,
            cwd,
            "s1",
            br#"{"type":"assistant","message":{"role":"assistant"}}"#,
        );
        let mut memo = ActivityMemo::new();
        // `now` == the file's mtime (just written) → age 0 → Busy.
        let now = std::fs::metadata(root.join(ai_monitor::encode_cwd(cwd)).join("s1.jsonl"))
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now());
        assert_eq!(
            state_of_cwd(&mut memo, &root, cwd, 0, now),
            AgentState::Busy
        );
    }

    // ---- D16 end-to-end: newest jsonl → AgentState (R3.1–R3.5) ----

    /// The four-turn conversation prefix every D16 fixture shares, ending just before the
    /// decisive last line each test appends. Proves we read the LAST meaningful message.
    const CONVO_PREFIX: &str = concat!(
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"go"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user"},"toolUseResult":{"stdout":"ok"}}"#,
        "\n",
    );

    fn state_of_fixture(tag: &str, cwd: &str, body: &str) -> AgentState {
        let root = projects_root(tag);
        let cwd = Path::new(cwd);
        write_log(&root, cwd, "s1", body.as_bytes());
        let mut memo = ActivityMemo::new();
        state_of_cwd(&mut memo, &root, cwd, 0, SystemTime::now())
    }

    #[test]
    fn end_turn_session_reads_waiting() {
        // R3.1 — last meaningful message is assistant end_turn → Waiting.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n",
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#
        );
        assert_eq!(
            state_of_fixture("d16-waiting", "/proj/d16-waiting", &body),
            AgentState::Waiting
        );
    }

    #[test]
    fn tool_use_session_reads_busy() {
        // R3.2 — last assistant message is mid-work (tool_use) → Busy, not Waiting.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n",
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#
        );
        assert_eq!(
            state_of_fixture("d16-tooluse", "/proj/d16-tooluse", &body),
            AgentState::Busy
        );
    }

    #[test]
    fn tool_result_session_reads_busy() {
        // R3.3 — last user line carries a toolUseResult → Busy.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n",
            r#"{"type":"user","message":{"role":"user"},"toolUseResult":{"stdout":"done"}}"#
        );
        assert_eq!(
            state_of_fixture("d16-result", "/proj/d16-result", &body),
            AgentState::Busy
        );
    }

    #[test]
    fn human_prompt_session_reads_busy() {
        // R3.4 — last user line has no toolUseResult (person just typed) → Busy.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"and now this"}]}}"#
        );
        assert_eq!(
            state_of_fixture("d16-prompt", "/proj/d16-prompt", &body),
            AgentState::Busy
        );
    }

    /// The real `AskUserQuestion` record, transcribed verbatim from a live session log:
    /// `stop_reason: "tool_use"` plus a `tool_use` content block naming the tool.
    const ASK_USER_QUESTION_LINE: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
        r#""content":[{"type":"tool_use","id":"toolu_01","name":"AskUserQuestion","#,
        r#""input":{"questions":[]}}]}}"#
    );

    #[test]
    fn ask_user_question_session_reads_waiting() {
        // ROOT-CAUSE regression: an agent blocked on AskUserQuestion used to read Busy
        // (its stop_reason is "tool_use"), so `waiting.rs` never saw a rising edge and the
        // sidebar never surfaced the question. End-to-end: real jsonl shape → Waiting.
        let _g = super::super::probe::serial();
        let body = format!("{CONVO_PREFIX}{ASK_USER_QUESTION_LINE}\n");
        assert_eq!(
            state_of_fixture("d16-askuser", "/proj/d16-askuser", &body),
            AgentState::Waiting
        );
    }

    #[test]
    fn answered_question_session_reads_busy_again() {
        // The falling edge `waiting.rs` needs to clear the band item: once the human
        // answers, the tool_result line is newest and the tab leaves Waiting.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{ASK_USER_QUESTION_LINE}\n{}\n",
            r#"{"type":"user","message":{"role":"user"},"toolUseResult":{"answer":"ok"}}"#
        );
        assert_eq!(
            state_of_fixture("d16-answered", "/proj/d16-answered", &body),
            AgentState::Busy
        );
    }

    #[test]
    fn ordinary_tool_call_session_still_reads_busy() {
        // The closed list must not widen: a normal tool call is unchanged: Busy, never
        // Waiting. This is the guard against the fix manufacturing false "waiting"s.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n",
            concat!(
                r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
                r#""content":[{"type":"tool_use","id":"toolu_09","name":"Bash","input":{}}]}}"#
            )
        );
        assert_eq!(
            state_of_fixture("d16-bash", "/proj/d16-bash", &body),
            AgentState::Busy
        );
    }

    #[test]
    fn no_log_session_is_unknown() {
        // R3.5 — a cwd with no resolvable jsonl → Unknown, no waiting signal.
        let _g = super::super::probe::serial();
        let root = projects_root("d16-none");
        let mut memo = ActivityMemo::new();
        assert_eq!(
            state_of_cwd(
                &mut memo,
                &root,
                Path::new("/proj/d16-none"),
                0,
                SystemTime::now()
            ),
            AgentState::Unknown
        );
    }

    #[test]
    fn truncated_last_line_is_not_waiting() {
        // Conservative bias — the last COMPLETE meaningful line is end_turn, but a newer
        // half-written line follows (as a >TAIL_CAP record cut by the cap would look). The
        // newest record is unreadable, so we must NOT report Waiting; the recency fallback
        // on the fresh file yields Busy.
        let _g = super::super::probe::serial();
        let body = format!(
            "{CONVO_PREFIX}{}\n{}",
            r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#,
            r#"{"type":"user","message":{"role":"user""# // truncated: no close, no newline
        );
        let state = state_of_fixture("d16-trunc", "/proj/d16-trunc", &body);
        assert_ne!(
            state,
            AgentState::Waiting,
            "truncated newest record → not Waiting"
        );
        assert_eq!(
            state,
            AgentState::Busy,
            "fresh file → recency fallback Busy"
        );
    }

    // ---- tab-identity: the label (D10 pick policy, D6 carry-forward) ----

    #[test]
    fn pick_label_policy() {
        // D10 — the newest *substantive* prompt wins; short and continuation turns skipped.
        assert_eq!(
            pick_label(&["계속".into(), "add pagination to the list".into()]),
            Some("add pagination to the list".to_string())
        );
        assert_eq!(pick_label(&["y".into(), "go".into()]), None, "all skipped");
        assert_eq!(pick_label(&[]), None);
        assert!(
            !is_substantive("continue"),
            "dictionary catches the 8-char word"
        );
        assert!(!is_substantive("네"), "too short");
        assert!(is_substantive("refactor X"));
    }

    #[test]
    fn label_picks_substantive_prompt_end_to_end() {
        // The newest human prompt becomes the label after one activity pass warms the memo.
        let _g = super::super::probe::serial();
        let root = projects_root("label-basic");
        let cwd = Path::new("/proj/label-basic");
        write_log(
            &root,
            cwd,
            "s1",
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"refactor the auth module"}]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#,
                "\n",
            )
            .as_bytes(),
        );
        let mut memo = ActivityMemo::new();
        let _ = state_of_cwd(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            label_of_cwd(&memo, cwd).as_deref(),
            Some("refactor the auth module")
        );
    }

    #[test]
    fn label_skips_continuation_for_prior_substantive() {
        // D10 — newest prompt is "계속"; the label is the earlier substantive prompt.
        let _g = super::super::probe::serial();
        let root = projects_root("label-cont");
        let cwd = Path::new("/proj/label-cont");
        write_log(
            &root,
            cwd,
            "s1",
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"implement the parser"}]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"계속"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        let mut memo = ActivityMemo::new();
        let _ = state_of_cwd(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            label_of_cwd(&memo, cwd).as_deref(),
            Some("implement the parser")
        );
    }

    #[test]
    fn label_persists_when_new_tail_has_no_substantive_prompt() {
        // D6 — once a label is set, a later change adding only a continuation turn keeps the
        // previous label (carry-forward), so an idle agent keeps its identity.
        use std::io::Write;
        let _g = super::super::probe::serial();
        let root = projects_root("label-persist");
        let cwd = Path::new("/proj/label-persist");
        let log = write_log(
            &root,
            cwd,
            "s1",
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the flaky test"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        let mut memo = ActivityMemo::new();
        let _ = state_of_cwd(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            label_of_cwd(&memo, cwd).as_deref(),
            Some("fix the flaky test")
        );
        // Append a continuation-only turn → len changes → re-read; label carries forward.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("append");
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"go"}}]}}}}"#
        )
        .expect("write");
        let _ = state_of_cwd(&mut memo, &root, cwd, 1, SystemTime::now());
        assert_eq!(
            label_of_cwd(&memo, cwd).as_deref(),
            Some("fix the flaky test"),
            "carry-forward keeps the last substantive label"
        );
    }

    #[test]
    fn label_none_without_resolvable_log() {
        // No session dir was ever read for this cwd → no label (card falls back to cwd).
        let memo = ActivityMemo::new();
        assert_eq!(label_of_cwd(&memo, Path::new("/proj/never")), None);
    }
}

//! Agent *activity* — busy vs. waiting — and the tab-identity label, for a detected Claude
//! agent (D7). A separate axis from presence (D5): the process table says *whether* an agent
//! is running, the session JSONL says *what it is doing*.
//!
//! # Which log, and why it is not the directory's newest
//!
//! Resolution is keyed on the tab's **own session**, not on its working directory
//! (`tabmeta-session-keying`). The directory cannot answer the question: every window working
//! in one project — sibling tabs of this app, Warp, iTerm, VS Code — appends to the same
//! `~/.claude/projects/<encoded-cwd>/`, so ranking it picks one winner for everyone. This
//! module used to do exactly that, and the module doc used to state the consequence as if it
//! were a design ("Both are shared by cwd, so two tabs in one project share them"): two tabs
//! in one project were *guaranteed* to render the same label and the same badge.
//!
//! [`resolve_log`] instead asks the tab's own `claude` process which session it is running,
//! in four tiers — see [`Resolution`] and [`Naming`]. Ranking survives only as the last of
//! them, for a tab that genuinely cannot be identified.
//!
//! # The two memos, and why their keys differ
//!
//! The busy/idle verdict is app policy, so [`state_from`] is a pure function here, not in
//! `ai-monitor` (which only supplies the raw `TailState`). Two memos keep the steady state
//! cheap (R6.7/R6.8), and they are keyed differently on purpose:
//!
//! - **`role`** — the tail read and the derived label — is keyed on the **resolved log path**
//!   and revalidated on `(mtime, len)`, so an unchanged log costs one `stat` and no read.
//!   Keying on the log is what makes two sessions independent, and it is also what stops the
//!   label carry-forward from leaking across a session boundary.
//! - **`path`** — the memo that amortises the expensive `read_dir` — stays keyed on **cwd**,
//!   because it now serves the ranking tier *only*, where there is no session id and the cwd
//!   is all there is.
//!
//! Both are reclaimed by disuse.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ai_monitor::activity::{tail_state, LastMessage};

use super::{AgentState, ClaudeId};

/// A log appended-to within this window reads `Busy` (D7).
const BUSY_WINDOW: Duration = Duration::from_secs(10);
/// The role clause fires only while the log is younger than this (D7) — so an abandoned
/// session whose last line is a `user` tool result does not read `Busy` forever (R6.10).
const STALE_LIMIT: Duration = Duration::from_secs(100);
/// A memoised log path is re-resolved after this many ticks (≈10 s), so a newly-started
/// session is picked up (R6.8).
pub(super) const RESOLVE_TTL_TICKS: u64 = 20;
/// An entry untouched for this many ticks (≈60 s) is reclaimed.
pub(super) const MEMO_TTL_TICKS: u64 = 120;
/// Hard cap on each memo; oldest-touched evicted first.
pub(super) const MEMO_MAX: usize = 64;

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

/// A resolved log **and the `stat` that proved it exists**.
///
/// The stat travels with the path deliberately. Resolution has to stat the candidate anyway
/// — that is how it tells "this session has not written yet" from "the directory I guessed
/// was wrong" — so if [`tail_role`] then took its own, every Claude tab would pay two stats
/// per refresh where it used to pay one. Carrying `(mtime, len)` here makes the single-stat
/// budget a property of the types rather than a discipline someone has to remember.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) log: PathBuf,
    mtime: SystemTime,
    len: u64,
}

/// What [`resolve_log`] concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A log to read — from any tier.
    Found(Found),
    /// The registry named this tab's session outright and its log **does not exist yet**.
    ///
    /// Distinct from [`Resolution::None`] because it is a *positive* finding: we know which
    /// session this tab is running and we know it has written nothing. The honest render is
    /// "an agent is here, no task yet" — never a neighbour's log. Reached only from
    /// [`Naming::Registry`]; see [`Naming`] for why the argv tier does not qualify.
    NamedButUnwritten,
    /// Nothing to read: no agent, no candidate, or an empty/absent project directory.
    None,
}

/// Which tier named a candidate — and therefore **what a missing file at that path proves**.
///
/// This distinction is the whole reason the two tiers are separate. Both produce a session id
/// straight from the process, so the *id* is authoritative in both. They differ in where the
/// **directory** comes from, and that is what a missing file tests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Naming {
    /// `~/.claude/sessions/<pid>.json` supplied the session id **and** the cwd it was
    /// registered under, so both halves of the path are authoritative. A missing file
    /// therefore means what it appears to mean: not written yet → [`Resolution::NamedButUnwritten`].
    Registry,
    /// A `--resume <id>` in argv supplied the id, but the directory had to be guessed from
    /// the tab's live cwd — a source that drifts (`Ctrl+Z`, `cd`, `fg`), and one a session
    /// resumed from elsewhere never matched in the first place. A missing file here proves
    /// nothing, so it must fall through to ranking rather than strand the tab in a permanent
    /// no-label state. This is precisely the tab the argv tier exists to help.
    Argv,
}

/// The two activity memos — keyed differently on purpose; see the module doc.
pub(crate) struct ActivityMemo {
    /// cwd → the ranked log, for the ranking tier only.
    path: HashMap<PathBuf, PathEntry>,
    /// **log path** → its tail state and derived label.
    role: HashMap<PathBuf, RoleEntry>,
}

struct PathEntry {
    log: Option<PathBuf>,
    resolved_at: u64,
    last_read_tick: u64,
}

struct RoleEntry {
    mtime: SystemTime,
    len: u64,
    role: String,
    last: Option<LastMessage>,
    last_read_tick: u64,
    /// The last *substantive* user-prompt label derived for **this log** (tab-identity, D10).
    /// Persisted across reads: when a newer tail has no substantive prompt (only short /
    /// continuation turns, or the prompt scrolled past the tail cap) the previous label is
    /// carried forward, so an idle agent keeps its last label (D3/D6). `None` only until the
    /// first substantive prompt is seen.
    ///
    /// **Keyed on the log, which is what makes the carry-forward safe.** While this map was
    /// cwd-keyed, a tab whose resolution moved from one session's log to another's inherited
    /// the *old* session's label — the newer log had no substantive prompt yet, so the
    /// carry-forward supplied the previous one, and both tabs in the project rendered the
    /// same wrong name. Under a log key there is simply no prior entry to inherit from.
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

/// Which log this tab should read — the four-tier ladder (D1).
///
/// | Tier | Condition | Candidate directory | On a missing file |
/// |---|---|---|---|
/// | 1 | registry record validates | the record's own cwd | [`Resolution::NamedButUnwritten`] |
/// | 1b | argv carries `--resume <id>` | the tab's live cwd | fall through to ranking |
/// | 2 | tier 1 named it, file absent | — | *is* the result |
/// | 3 | nothing nameable | the tab's live cwd | [`Resolution::None`] |
///
/// The existence test is [`stat_log`], not a separate `is_file`, and its `(mtime, len)` is
/// carried out on [`Found`] — so tier 1 and tier 3 each cost exactly one `stat`, the same as
/// before this feature. (A tier-1b *miss* pays a second stat when it falls through to
/// ranking; that path is rare and is the price of not stranding the tab.)
///
/// `sessions_root` is a parameter rather than `$HOME` so every tier is testable against a
/// temp directory — the same reason `projects_root` already is.
pub(crate) fn resolve_log(
    memo: &mut ActivityMemo,
    projects_root: &Path,
    sessions_root: &Path,
    cwd: &Path,
    claude: Option<ClaudeId>,
    tick: u64,
) -> Resolution {
    resolve_log_with(
        memo,
        projects_root,
        sessions_root,
        cwd,
        claude,
        tick,
        super::proc::proc_args,
    )
}

/// [`resolve_log`]'s body, generic over the argv read so tier 1b is testable without a live
/// process running `claude --resume` — the same `_with` split `tabsession::registered_id_with`,
/// `overlay::session_rotated_with` and `ai_monitor::resolve_owned_session_with` already use.
///
/// Without this seam tier 1b's *routing* is unobservable: reaching it requires a real pid whose
/// argv carries a `--resume`, which a unit test cannot manufacture. That matters more than it
/// sounds — the tier-1b miss falling through to ranking (rather than to
/// [`Resolution::NamedButUnwritten`]) is the asymmetry that stops a resumed-from-elsewhere tab
/// being stranded with no label at all, and it deserves a test rather than an argument.
fn resolve_log_with(
    memo: &mut ActivityMemo,
    projects_root: &Path,
    sessions_root: &Path,
    cwd: &Path,
    claude: Option<ClaudeId>,
    tick: u64,
    read_argv: impl Fn(u32) -> Vec<std::ffi::OsString>,
) -> Resolution {
    let out = resolve_inner(
        memo,
        projects_root,
        sessions_root,
        cwd,
        claude,
        tick,
        read_argv,
    );
    // Once per tab per refresh — the same *frequency* the reclaim ran at when it lived at the
    // end of the old `state_of_cwd`, and it must not depend on which tier won (a tab parked in
    // tier 2 still has to let the memos age out).
    //
    // Its *position within the tick* did move, though, and that is worth naming rather than
    // hiding behind "same cadence": it now runs BEFORE `state_of_found` touches this tab's
    // role entry, where it used to run after. At the `MEMO_MAX` cap that means `evict_oldest`
    // can drop the entry of the very tab being derived, if that entry is the globally
    // oldest-touched. The cost is one extra tail read on the next refresh — a cache wobble,
    // never a wrong answer — and it needs more than `MEMO_MAX` distinct live logs to happen at
    // all. Accepted rather than fixed, because the alternative (reclaiming after the state
    // read) would put a second reclaim call on a path that already has one.
    memo.reclaim(tick);
    out
}

#[allow(clippy::too_many_arguments)]
fn resolve_inner(
    memo: &mut ActivityMemo,
    projects_root: &Path,
    sessions_root: &Path,
    cwd: &Path,
    claude: Option<ClaudeId>,
    tick: u64,
    read_argv: impl Fn(u32) -> Vec<std::ffi::OsString>,
) -> Resolution {
    // Tiers 1 / 1b — ask the tab's own process which session it is running.
    if let Some((path, naming)) =
        claude.and_then(|id| named_candidate(sessions_root, projects_root, cwd, id, read_argv))
    {
        if let Some((mtime, len)) = stat_log(&path) {
            return Resolution::Found(Found {
                log: path,
                mtime,
                len,
            });
        }
        if naming == Naming::Registry {
            return Resolution::NamedButUnwritten; // tier 2
        }
        // Tier 1b guessed the directory, so its miss is not evidence — fall through.
    }
    // Tier 3 — rank the directory. Unchanged behaviour for a tab we cannot identify.
    match resolve_path(memo, projects_root, cwd, tick) {
        Some(log) => match stat_log(&log) {
            Some((mtime, len)) => Resolution::Found(Found { log, mtime, len }),
            None => Resolution::None,
        },
        None => Resolution::None,
    }
}

/// The log path this tab's `claude` names for itself, and which tier named it.
///
/// Tier 1 is tried first and, when it produces a candidate, tier 1b is never reached — which
/// is what keeps the `sysctl` argv read off the common path entirely.
fn named_candidate(
    sessions_root: &Path,
    projects_root: &Path,
    cwd: &Path,
    id: ClaudeId,
    read_argv: impl Fn(u32) -> Vec<std::ffi::OsString>,
) -> Option<(PathBuf, Naming)> {
    // Tier 1. Requires a start time: `validated_session_id` compares the record against the
    // process now holding that pid, and `~/.claude/sessions/` files outlive their processes,
    // so without a start time there is nothing to validate against and the record could name
    // a stranger's session on a recycled pid. No start time → skip to tier 1b rather than
    // believe the record.
    if let Some(start_ms) = id.start_ms {
        #[cfg(test)]
        super::probe::SESSION_REG.bump(1);
        if let Some(reg) = ai_monitor::registered_session_in(sessions_root, id.pid) {
            if let Some(sid) = ai_monitor::validated_session_id(&reg, start_ms) {
                // The RECORD's cwd, not the tab's: it names the directory `claude` registered
                // under, which is where it actually writes. The tab's live cwd is a proxy that
                // drifts if the shell moves after `claude` started.
                return Some((log_path(projects_root, &reg.cwd, sid), Naming::Registry));
            }
        }
    }
    // Tier 1b. The id is authoritative; the directory is the best guess available.
    let sid = resume_id_from(&read_argv(id.pid))?;
    Some((log_path(projects_root, cwd, &sid), Naming::Argv))
}

/// `<projects_root>/<encoded-cwd>/<session_id>.jsonl`. The id has already been through
/// `ai_monitor::looks_like_session_id` on both tiers, so it cannot escape the directory.
fn log_path(projects_root: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    projects_root
        .join(ai_monitor::encode_cwd(cwd))
        .join(format!("{session_id}.jsonl"))
}

/// The session id in an argv, from `--resume <id>` / `-r <id>` / `--resume=<id>`. **Pure.**
///
/// The same three spellings `tabsession::ProcTable::resume_id` accepts — but fed here from
/// `sysctl` rather than from a `/bin/ps` fork, which is what makes an argv tier affordable on
/// a per-tick worker at all.
///
/// The id goes through `ai_monitor::looks_like_session_id` before it is returned, and that is
/// a **path guard rather than input hygiene**: the return value is joined onto a directory as
/// a filename, so an argv of `--resume ../../etc/passwd` must not survive this function.
fn resume_id_from(args: &[std::ffi::OsString]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_string_lossy();
        if a == "--resume" || a == "-r" {
            let next = it.next()?.to_string_lossy().into_owned();
            return ai_monitor::looks_like_session_id(&next).then_some(next);
        }
        if let Some(rest) = a.strip_prefix("--resume=") {
            return ai_monitor::looks_like_session_id(rest).then(|| rest.to_string());
        }
    }
    None
}

/// The ranked log for `cwd`, memoised and re-resolved every `RESOLVE_TTL_TICKS` (R6.8).
/// The `read_dir` inside `resolve_log_in` is the expensive part, so it is counted
/// (`LOG_RESOLVE`) and skipped while the memo is fresh.
///
/// **Reached only from the ranking tier now.** A tab whose session is named never calls this,
/// so an identified tab performs no `read_dir` at all — `LOG_RESOLVE` drops from "once every
/// TTL" to zero for it.
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

/// The activity state for a resolved log, at `tick`/`now` (D7). `Unknown` when the tail
/// cannot be read (R6.5) — the one thing that state still asserts is presence, which the
/// consumer must honor (R6.6).
pub(crate) fn state_of_found(
    memo: &mut ActivityMemo,
    found: &Found,
    tick: u64,
    now: SystemTime,
) -> AgentState {
    match tail_role(memo, &found.log, found.mtime, found.len, tick) {
        Some((last, role, mtime)) => state_from(last, &role, mtime, now),
        None => AgentState::Unknown,
    }
}

/// The tail role + D16 `last` signal for `log`, memoised on `(mtime, len)` (R6.7). The stat
/// arrives from [`resolve_log`] rather than being taken here, so a warm tab pays one stat
/// total. Only a changed log triggers the bounded tail read. On a changed log the
/// tab-identity `label` is re-derived from the fresh prompts (D10) and carried forward when
/// none is substantive (D6); it lands in the memo so [`label_of_log`] can read it with no
/// further I/O.
fn tail_role(
    memo: &mut ActivityMemo,
    log: &Path,
    mtime: SystemTime,
    len: u64,
    tick: u64,
) -> Option<(Option<LastMessage>, String, SystemTime)> {
    if let Some(e) = memo.role.get_mut(log) {
        e.last_read_tick = tick;
        if e.mtime == mtime && e.len == len {
            return Some((e.last, e.role.clone(), e.mtime)); // unchanged → no read (R6.7)
        }
    }
    // Carry the previous label forward if the fresh scan has no substantive prompt (D6/D10).
    // Keyed on the log, so this can only ever carry THIS session's own earlier label.
    let prev_label = memo.role.get(log).and_then(|e| e.label.clone());
    let ts = tail_state(log)?;
    // The label needs the last *human* prompt, which routinely sits far outside the 8 KiB
    // activity tail (a single assistant turn + tool output exceeds it), so it gets a separate,
    // wider read (up to PROMPT_SCAN_CAP) — only here, on a changed log (still memoised, R6.7).
    let label = pick_label(&ai_monitor::activity::recent_prompts(log)).or(prev_label);
    #[cfg(test)]
    {
        super::probe::LOG_READ.bump(1);
        // Honest wrapper accounting (R6.1): the activity tail is bounded to TAIL_CAP; the
        // label scan adds up to PROMPT_SCAN_CAP more, both only on a changed log. The tail
        // read may add one more PROMPT_SCAN_CAP back-scan, when the tail ends in an
        // `end_turn` (`claude-background-working` D7). `TailState` does not say whether it
        // ran, so this is an upper bound that counts it every time.
        super::probe::LOG_READ_BYTES.bump(
            len.min(ai_monitor::activity::TAIL_CAP)
                + 2 * len.min(ai_monitor::activity::PROMPT_SCAN_CAP),
        );
    }
    memo.role.insert(
        log.to_path_buf(),
        RoleEntry {
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

/// The memoised tab-identity label for `log`, if one has been derived (tab-identity, D6).
/// Pure memo read — no filesystem I/O — so the worker gets the label for free after the
/// activity pass warmed the role memo this tick. `None` before any substantive prompt was
/// seen on this session (→ the card falls back to the cwd title).
pub(crate) fn label_of_log(memo: &ActivityMemo, log: &Path) -> Option<String> {
    memo.role.get(log).and_then(|e| e.label.clone())
}

/// The memoised last-meaningful-message classification of `log`, read the same way as
/// [`label_of_log`]: a pure memo read with no filesystem I/O, valid right after
/// [`state_of_found`] warmed the entry this tick. The session overview's `graph_asks_user`
/// (session-overview-ui D2) is this being [`LastMessage::AssistantAsksUser`].
pub(crate) fn last_of_log(memo: &ActivityMemo, log: &Path) -> Option<LastMessage> {
    memo.role.get(log).and_then(|e| e.last)
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
    use std::ffi::OsString;

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

    // ---- the ladder + the memos, against hand-rolled temp dirs (R11.2, R7.1) ----

    fn projects_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tabmeta-act-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir root");
        p
    }

    /// A temp stand-in for `~/.claude/sessions`. Its existence as a *parameter* is what makes
    /// tiers 1/1b/2 testable at all (R7.1) — going through `$HOME` would make these tests
    /// depend on the developer's own live Claude sessions.
    fn sessions_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tabmeta-sess-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir sessions");
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

    /// Write the `<sessions_root>/<pid>.json` a live `claude` writes about itself. Field
    /// names transcribed from a real record (`sessionId` / `cwd` / `startedAt`).
    fn write_registry(root: &Path, pid: u32, session_id: &str, cwd: &Path, started_at_ms: u64) {
        let body = format!(
            r#"{{"pid":{pid},"sessionId":"{session_id}","cwd":"{}","startedAt":{started_at_ms}}}"#,
            cwd.display()
        );
        std::fs::write(root.join(format!("{pid}.json")), body).expect("write registry");
    }

    /// A well-formed session id (the ladder rejects anything that is not UUID-shaped).
    const SID_A: &str = "02a3919c-58e8-446b-babc-d3dc616543c5";
    const SID_B: &str = "26fa908b-3225-428e-9ca2-de04e31c115d";

    fn claude_at(pid: u32, start_ms: u64) -> Option<ClaudeId> {
        Some(ClaudeId {
            pid,
            start_ms: Some(start_ms),
        })
    }

    /// Drive the full ladder the way `worker` does: resolve once, then read state from the
    /// same `Found`. Returns the state and the resolution so a test can assert on either.
    fn drive(
        memo: &mut ActivityMemo,
        projects: &Path,
        sessions: &Path,
        cwd: &Path,
        claude: Option<ClaudeId>,
        tick: u64,
        now: SystemTime,
    ) -> (Resolution, AgentState) {
        let res = resolve_log(memo, projects, sessions, cwd, claude, tick);
        let state = match &res {
            Resolution::Found(f) => state_of_found(memo, f, tick, now),
            _ => AgentState::Unknown,
        };
        (res, state)
    }

    /// The ranking tier alone — no process identity, which is what every pre-existing test
    /// in this module exercised before sessions were keyed on.
    fn ranked(
        memo: &mut ActivityMemo,
        projects: &Path,
        cwd: &Path,
        tick: u64,
        now: SystemTime,
    ) -> AgentState {
        let sessions = projects.join("no-sessions-here");
        drive(memo, projects, &sessions, cwd, None, tick, now).1
    }

    #[test]
    fn unresolvable_log_is_unknown() {
        // R2.9 / R6.5 — a cwd with no session dir → Unknown.
        let _g = super::super::probe::serial();
        let root = projects_root("unknown");
        let cwd = Path::new("/some/project/x");
        let mut memo = ActivityMemo::new();
        assert_eq!(
            ranked(&mut memo, &root, cwd, 0, SystemTime::now()),
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
        let _ = ranked(&mut memo, &root, cwd, 0, now);
        super::super::probe::reset_all();
        let _ = ranked(&mut memo, &root, cwd, 1, now);
        assert_eq!(
            super::super::probe::LOG_STAT.get(),
            1,
            "one revalidation stat"
        );
        assert_eq!(super::super::probe::LOG_READ.get(), 0, "no re-read");
        assert_eq!(super::super::probe::LOG_RESOLVE.get(), 0, "no re-resolve");
    }

    #[test]
    fn a_changed_log_accounts_the_conditional_back_scan() {
        // claude-background-working R5.3 — the app cannot see whether the tail read ran its
        // back-scan, so a changed log is accounted at the upper bound.
        let _g = super::super::probe::serial();
        let root = projects_root("backscan-bytes");
        let cwd = Path::new("/proj/backscan-bytes");
        let line =
            br#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#;
        let mut body = Vec::new();
        while (body.len() as u64) <= ai_monitor::activity::PROMPT_SCAN_CAP {
            body.extend_from_slice(line);
            body.push(b'\n');
        }
        write_log(&root, cwd, "s1", &body);
        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        let _ = ranked(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            super::super::probe::LOG_READ.get(),
            1,
            "one changed-log read"
        );
        assert_eq!(
            super::super::probe::LOG_READ_BYTES.get(),
            ai_monitor::activity::TAIL_CAP + 2 * ai_monitor::activity::PROMPT_SCAN_CAP
        );
    }

    #[test]
    fn log_path_memo_expires() {
        // R6.8 / R3.5 — the cwd-keyed `path` memo still serves the ranking tier: fresh within
        // RESOLVE_TTL_TICKS, re-resolved past it.
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
        let _ = ranked(&mut memo, &root, cwd, 0, now);
        // A few ticks later: reuse, no resolve.
        super::super::probe::reset_all();
        let _ = ranked(&mut memo, &root, cwd, 6, now);
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "still fresh → reuse"
        );
        // Past the TTL: re-resolve.
        super::super::probe::reset_all();
        let _ = ranked(&mut memo, &root, cwd, RESOLVE_TTL_TICKS + 1, now);
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
        assert_eq!(ranked(&mut memo, &root, cwd, 0, now), AgentState::Busy);
    }

    // ---- the four tiers (R2.1-R2.9) ----

    #[test]
    fn tier1_resolves_by_name_from_the_records_own_cwd() {
        // R2.1 — the record's cwd wins over the tab's live cwd. The two differ here exactly
        // as they do after a `Ctrl+Z` / `cd` / `fg`, and only the record's names the
        // directory `claude` actually writes into.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1-regcwd");
        let sessions = sessions_root("t1-regcwd");
        let reg_cwd = Path::new("/proj/a");
        let tab_cwd = Path::new("/proj/b");
        let want = write_log(&projects, reg_cwd, SID_A, br#"{"type":"assistant"}"#);
        // A decoy under the TAB's cwd, so picking the wrong directory is observable.
        write_log(&projects, tab_cwd, SID_B, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, reg_cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let res = resolve_log(
            &mut memo,
            &projects,
            &sessions,
            tab_cwd,
            claude_at(101, 1_000),
            0,
        );
        match res {
            Resolution::Found(f) => assert_eq!(f.log, want),
            other => panic!("expected the record's log, got {other:?}"),
        }
    }

    #[test]
    fn tier1_never_scans_the_directory() {
        // R2.2 — a named session does no `read_dir`, however many neighbours exist and
        // however much newer they are. This is the property that makes the fix a fix.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1-noscan");
        let sessions = sessions_root("t1-noscan");
        let cwd = Path::new("/proj/noscan");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        std::thread::sleep(Duration::from_millis(20));
        for n in ["aaa", "bbb", "ccc"] {
            write_log(&projects, cwd, n, br#"{"type":"assistant"}"#);
        }
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        let res = resolve_log(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
        );
        assert!(matches!(res, Resolution::Found(_)));
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "tier 1 must never read_dir"
        );
    }

    #[test]
    fn a_stale_record_is_rejected_and_does_not_reach_tier1() {
        // R2.3 — the record predates the process now holding that pid (a recycled pid), so
        // `validated_session_id` must refuse it. `~/.claude/sessions/` files outlive their
        // processes — 5 files for 4 live processes, when measured — so without this guard a
        // recycled pid presents a stranger's record and we adopt it with total confidence.
        //
        // **The fixture is built so the test can actually fail.** An earlier version put the
        // stranger's log alone in the directory, which made tier 1 and tier 3 resolve to the
        // SAME file: deleting the guard entirely left the whole suite green. Here the
        // stranger's session (SID_A, named by the stale record) and the ranked winner (SID_B,
        // newer by mtime) are different files, so believing the record is observable.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1-stale");
        let sessions = sessions_root("t1-stale");
        let cwd = Path::new("/proj/stale");
        let strangers = write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        std::thread::sleep(Duration::from_millis(20));
        let ranked_winner = write_log(&projects, cwd, SID_B, br#"{"type":"assistant"}"#);
        // Record written at 1_000; the process holding the pid started at 9_000 — the record
        // predates it, so it cannot describe it (`checked_sub` is the stale-file test).
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        // Injected argv (empty) keeps this hermetic: the real `proc_args(101)` would sysctl
        // whatever process happens to hold pid 101 on this machine.
        let res = resolve_log_with(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 9_000),
            0,
            |_| Vec::new(),
        );
        match res {
            Resolution::Found(f) => {
                assert_eq!(
                    f.log, ranked_winner,
                    "a stale record must be refused and the directory ranked instead — \
                     resolving to {strangers:?} means the recycled-pid guard is gone"
                );
            }
            other => panic!("a stale record must fall through to ranking, not {other:?}"),
        }
        assert!(
            super::super::probe::LOG_RESOLVE.get() >= 1,
            "the ranking tier must actually have run"
        );
    }

    #[test]
    fn tier2_refuses_to_guess_when_the_named_log_is_absent() {
        // R2.6 — THE regression test for the reported bug. The record names a session that
        // has written nothing (measured live: a 95s-old session with no project dir at all),
        // and the directory is full of other sessions' logs. The honest answer is "no task
        // yet", never a neighbour's prompt.
        let _g = super::super::probe::serial();
        let projects = projects_root("t2-unwritten");
        let sessions = sessions_root("t2-unwritten");
        let cwd = Path::new("/proj/unwritten");
        write_log(&projects, cwd, "neighbour-one", br#"{"type":"assistant"}"#);
        write_log(&projects, cwd, "neighbour-two", br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000); // SID_A.jsonl deliberately absent

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        let (res, state) = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            SystemTime::now(),
        );
        assert_eq!(res, Resolution::NamedButUnwritten);
        assert_eq!(state, AgentState::Unknown);
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "tier 2 must NOT fall through to ranking"
        );
        assert_eq!(label_of_log(&memo, Path::new("whatever")), None);
    }

    #[test]
    fn resume_id_parser_accepts_the_three_spellings_and_rejects_traversal() {
        // R2.4's parser half, pure over its input. All three spellings `claude` accepts, and
        // the rejection cases — which are a PATH GUARD, not input hygiene: this string is
        // joined onto a directory as a filename.
        let argv = |xs: &[&str]| -> Vec<OsString> { xs.iter().map(OsString::from).collect() };

        assert_eq!(
            resume_id_from(&argv(&["claude", "--resume", SID_A])).as_deref(),
            Some(SID_A)
        );
        assert_eq!(
            resume_id_from(&argv(&["claude", "-r", SID_A])).as_deref(),
            Some(SID_A)
        );
        assert_eq!(
            resume_id_from(&argv(&["claude", &format!("--resume={SID_A}")])).as_deref(),
            Some(SID_A)
        );
        // Traversal must not survive, in either spelling.
        assert_eq!(
            resume_id_from(&argv(&["claude", "--resume", "../../etc/passwd"])),
            None
        );
        assert_eq!(
            resume_id_from(&argv(&["claude", "--resume=../../etc/passwd"])),
            None
        );
        // A non-UUID, a dangling flag, and an argv with no resume at all.
        assert_eq!(resume_id_from(&argv(&["claude", "--resume", "nope"])), None);
        assert_eq!(resume_id_from(&argv(&["claude", "--resume"])), None);
        assert_eq!(resume_id_from(&argv(&["claude", "--continue"])), None);
        assert_eq!(resume_id_from(&[]), None);
    }

    #[test]
    fn tier1b_resolves_a_resumed_session_from_argv() {
        // R2.4's routing half — the POSITIVE path, which needs the injected argv reader
        // because a unit test cannot spawn a real `claude --resume`.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1b-hit");
        let sessions = sessions_root("t1b-hit"); // deliberately empty → tier 1 misses
        let cwd = Path::new("/proj/t1b-hit");
        let want = write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        // A newer decoy: if tier 1b were not consulted, ranking would return this instead.
        std::thread::sleep(Duration::from_millis(20));
        write_log(&projects, cwd, SID_B, br#"{"type":"assistant"}"#);

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        let res = resolve_log_with(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            |_| {
                vec![
                    OsString::from("claude"),
                    OsString::from("--resume"),
                    OsString::from(SID_A),
                ]
            },
        );
        match res {
            Resolution::Found(f) => assert_eq!(f.log, want, "argv names the session, not mtime"),
            other => panic!("expected the argv-named log, got {other:?}"),
        }
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "a named session never scans, whichever tier named it"
        );
    }

    #[test]
    fn tier1b_miss_falls_through_to_ranking_rather_than_stranding_the_tab() {
        // R2.7 — the asymmetry, and the single most important routing test in this module.
        //
        // Tier 1 and tier 1b both produce an authoritative session ID; they differ in where
        // the DIRECTORY comes from. Tier 1's is the registry record's own cwd, so a missing
        // file proves "not written yet" → tier 2. Tier 1b's is the tab's live cwd — a guess —
        // so a missing file there proves nothing: the session may simply have been resumed
        // from a different directory and be logging happily under its origin's project dir.
        //
        // Routing that miss to tier 2 would leave a live, logging session showing no label,
        // permanently, and it would do so to exactly the tabs tier 1b exists to help.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1b-miss");
        let sessions = sessions_root("t1b-miss");
        let cwd = Path::new("/proj/t1b-miss");
        // SID_A.jsonl is ABSENT under this cwd; only a neighbour exists.
        let neighbour = write_log(&projects, cwd, SID_B, br#"{"type":"assistant"}"#);

        let mut memo = ActivityMemo::new();
        let res = resolve_log_with(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            |_| {
                vec![
                    OsString::from("claude"),
                    OsString::from("--resume"),
                    OsString::from(SID_A),
                ]
            },
        );
        assert_ne!(
            res,
            Resolution::NamedButUnwritten,
            "tier 2 is reserved for tier 1, whose directory is authoritative"
        );
        match res {
            Resolution::Found(f) => assert_eq!(f.log, neighbour, "fell through to ranking"),
            other => panic!("a tier-1b miss must rank, not answer {other:?}"),
        }
    }

    #[test]
    fn a_registry_miss_and_no_argv_still_ranks() {
        // The other half of the tier-1b gate: no record AND no `--resume` → plain ranking,
        // which is today's behaviour for a tab that cannot be identified at all.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1b-neither");
        let sessions = sessions_root("t1b-neither");
        let cwd = Path::new("/proj/neither");
        let only = write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);

        let mut memo = ActivityMemo::new();
        let res = resolve_log_with(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            |_| vec![OsString::from("claude")],
        );
        match res {
            Resolution::Found(f) => assert_eq!(f.log, only),
            other => panic!("expected the ranked log, got {other:?}"),
        }
    }

    #[test]
    fn tier1_hit_never_reads_argv() {
        // R2.5 — the argv `sysctl` is gated on a tier-1 miss, so an ordinary identified tab
        // never pays it.
        let _g = super::super::probe::serial();
        let projects = projects_root("t1-noargv");
        let sessions = sessions_root("t1-noargv");
        let cwd = Path::new("/proj/noargv");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        let _ = resolve_log(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
        );
        assert_eq!(
            super::super::probe::PROC_ARGS.get(),
            0,
            "tier-1 hit ⇒ the argv gate stays closed"
        );
    }

    #[test]
    fn tier3_is_todays_ranking_unchanged() {
        // R2.8 — an unidentifiable tab gets exactly what it got before this feature.
        let _g = super::super::probe::serial();
        let projects = projects_root("t3-same");
        let sessions = sessions_root("t3-same");
        let cwd = Path::new("/proj/t3");
        write_log(&projects, cwd, "old", br#"{"type":"assistant"}"#);
        std::thread::sleep(Duration::from_millis(20));
        write_log(&projects, cwd, "new", br#"{"type":"assistant"}"#);

        let mut memo = ActivityMemo::new();
        let res = resolve_log(&mut memo, &projects, &sessions, cwd, None, 0);
        match res {
            Resolution::Found(f) => assert_eq!(
                Some(f.log),
                ai_monitor::resolve_log_in(&projects, cwd),
                "the ranking tier must agree with ai_monitor::resolve_log_in exactly"
            ),
            other => panic!("expected a ranked log, got {other:?}"),
        }
    }

    #[test]
    fn degraded_inputs_never_panic() {
        // R2.9 / R8.2 — malformed registry JSON, a 0-byte log, and a dead pid all degrade.
        let _g = super::super::probe::serial();
        let projects = projects_root("degraded");
        let sessions = sessions_root("degraded");
        let cwd = Path::new("/proj/degraded");
        std::fs::write(sessions.join("101.json"), b"{not json").expect("write junk");
        write_log(&projects, cwd, SID_A, b""); // 0 bytes

        let mut memo = ActivityMemo::new();
        // Malformed record → tier 1 cannot name anything → ranking finds the empty log,
        // whose tail cannot be parsed → Unknown, not a panic.
        let (_, state) = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            SystemTime::now(),
        );
        assert_eq!(state, AgentState::Unknown);
        // A pid that cannot exist, with no logs anywhere.
        let empty = projects_root("degraded-empty");
        let (res, state) = drive(
            &mut memo,
            &empty,
            &sessions,
            Path::new("/proj/nothing"),
            claude_at(u32::MAX, 1_000),
            1,
            SystemTime::now(),
        );
        assert_eq!(res, Resolution::None);
        assert_eq!(state, AgentState::Unknown);
    }

    // ---- the memos follow the session (R3.1-R3.6, R5.1, R5.2) ----

    #[test]
    fn two_sessions_in_one_directory_keep_separate_labels() {
        // R3.1 / R0.1 — the defect, inverted: two tabs, one project, two labels.
        let _g = super::super::probe::serial();
        let projects = projects_root("two-sessions");
        let sessions = sessions_root("two-sessions");
        let cwd = Path::new("/proj/shared");
        let log_a = write_log(
            &projects,
            cwd,
            SID_A,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"refactor the auth module"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        let log_b = write_log(
            &projects,
            cwd,
            SID_B,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the flaky test"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        write_registry(&sessions, 101, SID_A, cwd, 1_000);
        write_registry(&sessions, 202, SID_B, cwd, 2_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(202, 2_000),
            0,
            now,
        );

        assert_eq!(
            label_of_log(&memo, &log_a).as_deref(),
            Some("refactor the auth module")
        );
        assert_eq!(
            label_of_log(&memo, &log_b).as_deref(),
            Some("fix the flaky test")
        );
        assert_ne!(label_of_log(&memo, &log_a), label_of_log(&memo, &log_b));
    }

    #[test]
    fn two_tabs_on_one_session_share_the_memo_entry() {
        // R3.2 — sharing is correct when the session really IS shared (two tabs attached to
        // one tmux pane): the second tab costs no read.
        let _g = super::super::probe::serial();
        let projects = projects_root("shared-session");
        let sessions = sessions_root("shared-session");
        let cwd = Path::new("/proj/one-session");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        super::super::probe::reset_all();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        assert_eq!(
            super::super::probe::LOG_READ.get(),
            0,
            "one session → one entry → no second read"
        );
    }

    #[test]
    fn carry_forward_does_not_cross_sessions() {
        // R3.3 — THE second defect mechanism. Session A has a substantive prompt; session B
        // has written only an assistant line. Under the old cwd key, B inherited A's label
        // and both cards showed the same wrong name. Under a log key there is nothing to
        // inherit.
        let _g = super::super::probe::serial();
        let projects = projects_root("no-cross-carry");
        let sessions = sessions_root("no-cross-carry");
        let cwd = Path::new("/proj/carry");
        let log_a = write_log(
            &projects,
            cwd,
            SID_A,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"implement the parser"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        // B is NEWER by mtime, so the old ranking would also have preferred it — proving the
        // label leak was independent of which log won.
        std::thread::sleep(Duration::from_millis(20));
        let log_b = write_log(
            &projects,
            cwd,
            SID_B,
            br#"{"type":"assistant","message":{"role":"assistant"}}"#,
        );
        write_registry(&sessions, 101, SID_A, cwd, 1_000);
        write_registry(&sessions, 202, SID_B, cwd, 2_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        // Warm A first — this is what used to poison the shared cwd entry.
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        assert_eq!(
            label_of_log(&memo, &log_a).as_deref(),
            Some("implement the parser")
        );
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(202, 2_000),
            0,
            now,
        );
        assert_eq!(
            label_of_log(&memo, &log_b),
            None,
            "session B must NOT inherit session A's label"
        );
    }

    #[test]
    fn carry_forward_still_works_within_one_session() {
        // R3.4 — the fix must not break tab-identity D6: an idle agent keeps its own label.
        use std::io::Write;
        let _g = super::super::probe::serial();
        let projects = projects_root("carry-within");
        let sessions = sessions_root("carry-within");
        let cwd = Path::new("/proj/within");
        let log = write_log(
            &projects,
            cwd,
            SID_A,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the flaky test"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        assert_eq!(
            label_of_log(&memo, &log).as_deref(),
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
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            1,
            now,
        );
        assert_eq!(
            label_of_log(&memo, &log).as_deref(),
            Some("fix the flaky test"),
            "carry-forward within a session is tab-identity D6 and must survive"
        );
    }

    #[test]
    fn a_named_tab_costs_one_stat_and_no_scan() {
        // R5.1 / R5.2 — the whole warm budget, in one assertion block. The single stat is
        // the load-bearing one: resolution takes it and hands it to `tail_role`, which no
        // longer takes its own.
        let _g = super::super::probe::serial();
        let projects = projects_root("budget");
        let sessions = sessions_root("budget");
        let cwd = Path::new("/proj/budget");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        super::super::probe::reset_all();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            1,
            now,
        );
        assert_eq!(super::super::probe::LOG_STAT.get(), 1, "exactly one stat");
        assert_eq!(
            super::super::probe::LOG_READ.get(),
            0,
            "unchanged → no read"
        );
        assert_eq!(
            super::super::probe::LOG_RESOLVE.get(),
            0,
            "a named tab never scans"
        );
        assert_eq!(
            super::super::probe::SESSION_REG.get(),
            1,
            "one registry read per refresh"
        );
    }

    #[test]
    fn the_registry_read_is_never_memoised() {
        // R4.3 — it IS the rotation signal, so skipping it would paint a finished session.
        let _g = super::super::probe::serial();
        let projects = projects_root("no-memo");
        let sessions = sessions_root("no-memo");
        let cwd = Path::new("/proj/nomemo");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        super::super::probe::reset_all();
        for tick in 0..3 {
            let _ = resolve_log(
                &mut memo,
                &projects,
                &sessions,
                cwd,
                claude_at(101, 1_000),
                tick,
            );
        }
        assert_eq!(
            super::super::probe::SESSION_REG.get(),
            3,
            "one read per refresh, never skipped"
        );
    }

    #[test]
    fn a_rotation_is_followed_on_the_next_refresh() {
        // R4.1 — `/clear` keeps the pid and swaps the session id. The card must follow
        // immediately, and must NOT keep the pre-rotation label.
        let _g = super::super::probe::serial();
        let projects = projects_root("rotate");
        let sessions = sessions_root("rotate");
        let cwd = Path::new("/proj/rotate");
        let log_a = write_log(
            &projects,
            cwd,
            SID_A,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"the old task"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        let log_b = write_log(
            &projects,
            cwd,
            SID_B,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"the new task"}]}}"#,
                "\n",
            )
            .as_bytes(),
        );
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let (res, _) = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        assert_eq!(res, Resolution::Found(found_at(&log_a)));
        assert_eq!(label_of_log(&memo, &log_a).as_deref(), Some("the old task"));

        // Same pid, new session — exactly what `/clear` writes.
        write_registry(&sessions, 101, SID_B, cwd, 1_000);
        let (res, _) = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            1,
            now,
        );
        assert_eq!(res, Resolution::Found(found_at(&log_b)), "followed at once");
        assert_eq!(label_of_log(&memo, &log_b).as_deref(), Some("the new task"));
    }

    /// The `Found` a test expects for `log`, with the stat read back from disk.
    fn found_at(log: &Path) -> Found {
        let (mtime, len) = stat_log(log).expect("stat fixture");
        Found {
            log: log.to_path_buf(),
            mtime,
            len,
        }
    }

    #[test]
    fn memo_reclaim_and_eviction_still_bound_both_maps() {
        // R3.6 — the key changed; the bounds did not.
        let _g = super::super::probe::serial();
        let projects = projects_root("reclaim");
        let sessions = sessions_root("reclaim");
        let cwd = Path::new("/proj/reclaim");
        write_log(&projects, cwd, SID_A, br#"{"type":"assistant"}"#);
        write_registry(&sessions, 101, SID_A, cwd, 1_000);

        let mut memo = ActivityMemo::new();
        let now = SystemTime::now();
        let _ = drive(
            &mut memo,
            &projects,
            &sessions,
            cwd,
            claude_at(101, 1_000),
            0,
            now,
        );
        assert_eq!(memo.role.len(), 1);
        // Far past MEMO_TTL_TICKS with nothing touching it → reclaimed.
        let _ = resolve_log(
            &mut memo,
            &projects,
            &sessions,
            Path::new("/proj/elsewhere"),
            None,
            MEMO_TTL_TICKS + 1,
        );
        assert!(
            memo.role.is_empty(),
            "an untouched role entry must age out under the new key too"
        );

        // The hard cap still holds under many distinct log keys.
        let mut big = ActivityMemo::new();
        for i in 0..(MEMO_MAX + 10) {
            big.role.insert(
                PathBuf::from(format!("/log/{i}.jsonl")),
                RoleEntry {
                    mtime: SystemTime::UNIX_EPOCH,
                    len: 0,
                    role: String::new(),
                    last: None,
                    last_read_tick: i as u64,
                    label: None,
                },
            );
        }
        big.reclaim(MEMO_MAX as u64 + 10);
        assert!(big.role.len() <= MEMO_MAX, "cap still enforced");
    }

    // ---- D16 end-to-end: newest jsonl → AgentState (R3.1–R3.5 of tab-metadata) ----

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
        ranked(&mut memo, &root, cwd, 0, SystemTime::now())
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
        // This pins the record's SHAPE only: under Claude Code 2.1.28x the record is not on
        // disk while the question is open, so this is not a live Claude path — see
        // `pending_question_session_reads_busy`.
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

    /// The log tail of a Claude Code 2.1.28x session with an `AskUserQuestion` dialog open on
    /// screen, in the shape measured on 2026-10-04 (`claude-ask-dialog-waiting` Research),
    /// content stripped: the previous `Edit` call, its `tool_result`, then eight role-less
    /// lines — a `hook_success` attachment, five metadata records, and two more attachments.
    /// The question's own `tool_use` is absent: it is not written until the tool returns.
    const PENDING_QUESTION_TAIL: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
        r#""content":[{"type":"tool_use","id":"toolu_10","name":"Edit","input":{}}]}}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","#,
        r#""tool_use_id":"toolu_10","content":""}]},"toolUseResult":{"filePath":""}}"#,
        "\n",
        r#"{"type":"attachment","attachment":{"type":"hook_success","hookEvent":"PostToolUse"}}"#,
        "\n",
        r#"{"type":"last-prompt","lastPrompt":""}"#,
        "\n",
        r#"{"type":"ai-title","aiTitle":""}"#,
        "\n",
        r#"{"type":"mode","mode":"normal"}"#,
        "\n",
        r#"{"type":"permission-mode","permissionMode":"auto"}"#,
        "\n",
        r#"{"type":"atis-latch","atis":""}"#,
        "\n",
        r#"{"type":"attachment","attachment":{"type":"total_tokens_reminder","text":""}}"#,
        "\n",
        r#"{"type":"attachment","attachment":{"type":"deferred_tools_record","entries":[]}}"#,
        "\n",
    );

    #[test]
    fn pending_question_session_reads_busy() {
        // R6.1 of `claude-ask-dialog-waiting` — THE reason the question/plan screen rules in
        // `prompt.rs` exist. With the dialog up, the pending AskUserQuestion is not on disk,
        // the role-less lines after the last tool_result are skipped, and so the newest
        // meaningful message is the previous Edit's tool_result: UserToolResult → Busy, and
        // the card reads Working for as long as the human takes to answer. No log rule can
        // turn this into Waiting; only the screen can.
        let _g = super::super::probe::serial();
        let root = projects_root("d16-pending");
        let cwd = Path::new("/proj/d16-pending");
        let log = write_log(&root, cwd, "s1", PENDING_QUESTION_TAIL.as_bytes());
        let ts = tail_state(&log).expect("tail");
        assert_eq!(
            ts.last,
            Some(LastMessage::UserToolResult),
            "the previous tool's result, not the open question"
        );
        // Ten minutes on, as measured: the log's mtime stayed frozen while the dialog was up.
        // That is past STALE_LIMIT, where the recency fallback says Idle, so Busy here can
        // only come from `last` — not from a freshly written file.
        let mut memo = ActivityMemo::new();
        let now = ts.mtime + Duration::from_secs(600);
        assert_eq!(ranked(&mut memo, &root, cwd, 0, now), AgentState::Busy);
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
            ranked(
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
        let log = write_log(
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
        let _ = ranked(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            label_of_log(&memo, &log).as_deref(),
            Some("refactor the auth module")
        );
    }

    #[test]
    fn label_skips_continuation_for_prior_substantive() {
        // D10 — newest prompt is "계속"; the label is the earlier substantive prompt.
        let _g = super::super::probe::serial();
        let root = projects_root("label-cont");
        let cwd = Path::new("/proj/label-cont");
        let log = write_log(
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
        let _ = ranked(&mut memo, &root, cwd, 0, SystemTime::now());
        assert_eq!(
            label_of_log(&memo, &log).as_deref(),
            Some("implement the parser")
        );
    }

    #[test]
    fn label_none_for_a_log_never_read() {
        // No pass ever resolved this log → no label (the card falls back to the cwd title).
        let memo = ActivityMemo::new();
        assert_eq!(label_of_log(&memo, Path::new("/proj/never.jsonl")), None);
    }
}

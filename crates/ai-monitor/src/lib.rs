//! `ai-monitor` — headless reader of the active session's AI (Claude Code) JSONL
//! logs (D1). UI/GPU-free (SD2): it locates the log, backfills the recent tail,
//! watches for appends, and parses each line into an [`AiEvent`]. The overlay
//! that renders these lives in the app/renderer (SD3/SD4); this crate only reads.
//!
//! Read-only (C3): logs are opened for reading, never written.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use notify::{RecursiveMode, Watcher};
use serde_json::Value;

use crate::telemetry::{SessionTelemetry, TelemetrySnapshot};

/// Bounded tail read of a session log — the activity signal consumed by the sidebar's
/// `tab-metadata` worker (D7). Purely additive; uses only `std` + `serde_json`.
pub mod activity;

/// Stateful session telemetry — the accumulating fold behind the ai-monitor status
/// dashboard (`telemetry-model`). Pure over its input, bounded, GPU/UI-free (SD2/SD4).
pub mod telemetry;

/// One parsed log event: who spoke and a one-line summary (D1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiEvent {
    pub role: String,
    pub summary: String,
}

/// Encode a working directory the way Claude Code names its project dir:
/// non-alphanumeric path separators (`/`, `.`) become `-`.
/// `/Users/x/my.proj` → `-Users-x-my-proj`.
pub fn encode_cwd(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    s.chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// What Claude Code itself records about a running session, keyed by pid.
///
/// `~/.claude/sessions/<pid>.json` is written by the `claude` process for its own pid and
/// names its session **exactly** — no inference, no timing. It is the answer four rounds
/// of this feature tried to derive from clock arithmetic and could not: two processes
/// alive before a log is written are indistinguishable by start time, because start time
/// simply does not record who wrote what.
///
/// Verified against every live `claude` on the machine (2026-08-01): 5/5 had a file, every
/// `session_id` named an existing log, every `cwd` matched, and `started_at_ms` sat
/// 1.3–2.0s after the true process start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub session_id: String,
    pub cwd: PathBuf,
    /// When the process recorded itself, epoch millis — slightly after its real start.
    /// The caller compares this against the live process to reject a **stale file whose
    /// pid has been reused**, which is not hypothetical: the same sample held 5 files for
    /// 4 live processes.
    pub started_at_ms: u64,
}

/// Another live `claude` working in the same project — a sibling tab of this app, or any
/// other application's session.
///
/// Carries **process facts only**: the pid, and when that process started. It deliberately
/// does NOT carry the rival's session id, even though the caller could read it once at
/// panel-open. The watch thread re-reads each rival's record while unattached (a rival in
/// its first ~2s has no record yet, and that is exactly the state in which it would
/// otherwise keep vetoing), so a resolved id would be stale by the time it mattered. The
/// pid is a filename key into `~/.claude/sessions/`; this crate never queries the kernel
/// with it — that stays in `app`, which owns `libc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rival {
    pub pid: u32,
    /// The rival process's start time, epoch millis, measured by the caller.
    pub started_ms: u64,
}

/// Read `<root>/<pid>.json`. Split from [`registered_session`] so it is testable without
/// a real `$HOME`. Any missing field, bad JSON, or unreadable file yields `None` — this
/// is an undocumented on-disk detail of another program, so it must never be load-bearing
/// beyond "use it when it is there".
pub fn registered_session_in(sessions_root: &Path, pid: u32) -> Option<Registered> {
    let text = std::fs::read_to_string(sessions_root.join(format!("{pid}.json"))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(Registered {
        session_id: v.get("sessionId").and_then(Value::as_str)?.to_string(),
        cwd: PathBuf::from(v.get("cwd").and_then(Value::as_str)?),
        started_at_ms: v.get("startedAt").and_then(Value::as_u64)?,
    })
}

/// `~/.claude/sessions/<pid>.json`, if `$HOME` is set and the file parses.
pub fn registered_session(pid: u32) -> Option<Registered> {
    let home = std::env::var_os("HOME")?;
    registered_session_in(&PathBuf::from(home).join(".claude").join("sessions"), pid)
}

/// How far after a process's true start `claude` may record itself before we stop believing
/// the record describes *that* process.
///
/// Measured at 1.3–2.0s across five live sessions (2026-08-01) and again at +2.2s on
/// 2026-08-02; a minute is far beyond that. The bound exists for **pid reuse**:
/// `~/.claude/sessions/` files outlive their processes (5 files for 4 live processes, when
/// measured), so a recycled pid can present a stranger's record.
pub const REGISTRY_DRIFT_MS: u64 = 60_000;

/// The session id in `reg`, believed only if the record post-dates the process now holding
/// that pid — `None` when it cannot be trusted.
///
/// **One rule, one function** (R3.3). This is applied to our *own* tab's record by
/// `app::tabsession` and to every *rival's* record by [`resolve_owned_session`]. It lived
/// only in `app` before, and duplicating it here for rivals is exactly the shape that has
/// already produced two defects in this feature: two copies of a rule that must agree.
///
/// `checked_sub` is the stale-file test, not arithmetic hygiene — a record written BEFORE
/// the process started cannot describe it, so `None` is the answer rather than something to
/// paper over with `saturating_sub`.
pub fn validated_session_id(reg: &Registered, process_start_ms: u64) -> Option<&str> {
    let drift = reg.started_at_ms.checked_sub(process_start_ms)?;
    (drift <= REGISTRY_DRIFT_MS).then_some(reg.session_id.as_str())
}

/// `~/.claude/projects` (or `None` if `$HOME` is unset).
pub fn claude_projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

/// Resolve the active session log for `cwd` under a given projects root: the
/// newest `*.jsonl` in `<root>/<encoded-cwd>/` (D2). Split out from [`resolve_log`]
/// so it is testable without touching a real `$HOME`.
pub fn resolve_log_in(projects_root: &Path, cwd: &Path) -> Option<PathBuf> {
    let dir = projects_root.join(encode_cwd(cwd));
    newest_jsonl(&dir)
}

/// Resolve the active session log for `cwd` under the real `~/.claude/projects`.
pub fn resolve_log(cwd: &Path) -> Option<PathBuf> {
    resolve_log_in(&claude_projects_dir()?, cwd)
}

/// The project directory Claude Code writes `cwd`'s session logs into, whether or
/// not it exists yet. `None` only when there is no `~/.claude/projects` at all.
pub fn project_dir(cwd: &Path) -> Option<PathBuf> {
    Some(claude_projects_dir()?.join(encode_cwd(cwd)))
}

/// How many candidate logs a resolution will tail-read.
///
/// `mtime >= last_event` always holds — writing a line sets the mtime — so ranking
/// by mtime ranks by an *upper bound* on the answer. That makes the shortlist a
/// strong heuristic, **not** a proof: a winner is excluded only if 16 other logs
/// were all touched after its last event while none of them recorded a later event.
/// That needs 16 concurrently-written sessions in one project directory, so the
/// bound holds in practice — but it is a bound, and worth saying so rather than
/// claiming the exclusion is impossible.
const RESOLVE_CANDIDATES: usize = 16;

/// The log in `dir` whose newest parseable event is the most recent — the session
/// that most recently *did something*.
///
/// **Not the newest mtime.** A log's mtime advances on any write, including writes
/// that carry no timestamp, so mtime answers "was this file written to" where the
/// question is "is this session active". Measured on a real project directory: the
/// mtime-newest log's last event was 15 minutes stale while a second log — older by
/// mtime — was the one being worked in. 16 of 94 lines in one real log carry no
/// timestamp, so this is routine rather than exotic.
///
/// Cost is bounded twice over: at most [`RESOLVE_CANDIDATES`] files, and at most
/// [`activity::TAIL_CAP`] bytes read from each. Returns the path and that event
/// time, so the caller can apply its own freshness rule without re-reading.
/// What the caller knows about which session it is looking for.
///
/// Empty means "no idea" — resolution then falls back to ranking the directory,
/// which is all three previous rounds of this feature could ever do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    /// The session id, when the caller could name it outright.
    pub session_id: Option<String>,
    /// When the caller's process started. A session it began has its first event
    /// just after this.
    pub since_ms: Option<u64>,
}

/// A resolved log, and whether it was *confirmed* to be the caller's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub last_event_ms: Option<u64>,
    /// True when the [`Wanted`] hint identified this log — by name, or by a first
    /// event that follows the caller's own start. False when it is merely the most
    /// recently active log in the directory, which may belong to any application.
    pub owned: bool,
}

/// How long AFTER the `claude` process starts its session's first event may appear.
/// The window is deliberately **asymmetric** — see [`FIRST_EVENT_LEAD_MS`] for the other
/// side, which is nearly zero because it encodes a physical impossibility rather than a
/// tolerance.
///
/// A session this tab started writes its first event seconds after the process —
/// **measured, not assumed**: across every live `claude` on this machine that owned a
/// log, `first_event − process_start` was +1s, +2s, +1s, +2s, +1s (n=5, 2026-08-01), and
/// never negative. 120s is generous against that: a cold start's first write is not
/// guaranteed inside 30s, and failing to recognise our own session is the more expensive
/// error.
///
/// Bounding this side matters because a one-sided `first >= since − SLACK` rule admits
/// *any* log whose first event is later, including one that began five hours after; under
/// strict resolution that log would be the sole thing reaching the panel, and it would
/// stick. Sessions genuinely older than the process are named by tier 1
/// (`claude --resume <id>`), which never reaches this bound.
///
/// **This bound does not apply to a sole candidate with no EFFECTIVE, KNOWN rivals.** A tab
/// where `claude` runs but the user does not type for a long time writes its first event
/// well past 120s; that log is still adopted, because uniqueness accounts for it without
/// needing distance. `the_upper_bound_stops_at_120s_after_the_process` covers both halves:
/// the bound separates *rivals*, and a lone candidate passes it by.
///
/// Both qualifiers are load-bearing, and "unrivalled" now names a condition the code
/// branches on rather than a figure of speech. The sentence previously read:
///
/// > **This bound does not apply to a sole, unrivalled candidate.**
///
/// which left two readings open, and the wrong one is reachable in each direction:
/// - *effective* — a rival whose own record names a different session did not write this
///   log and does not count. Counting the raw list instead makes the rival fix inert: for a
///   rival to have vetoed us it must satisfy `since < r <= first`, which forces
///   `first − since >= r − since`, so any rival far enough out to veto also pushes the log
///   past this bound. Lifting the veto then changes nothing.
/// - *known* — an empty rival list is also what a FAILED enumeration looks like (an
///   unreadable cwd, ours or a rival's). Since this exemption adopts at unbounded distance
///   with `owned: true`, granting it on a failed read confirms a foreign log and latches it.
///
/// Two earlier drafts of this paragraph are worth naming, since decisions were taken on
/// each. The first said such a tab "fills in on the first write" — false: `since_ms` is
/// the process start and never moves, so no re-resolution rescues a first event past the
/// bound. The second said the tab was therefore empty *permanently* — true at the time,
/// and the reason uniqueness was added.
///
/// **What this bound cannot save.** A session resumed *without* an id in argv —
/// `claude --continue` / `-c`, a fork, a compacted transcript — inherits the original
/// conversation's first event, so it misses this window by days, and tier 1 has no id to
/// match. Neither tier confirms it and the panel shows its empty state for the life of
/// the tab. Accepted deliberately; see `specs/ai-monitor-session-isolation/spec.md`
/// § Known Gaps for the two fixes that were tested and rejected, and the shape of the
/// follow-up.
const FIRST_EVENT_SLACK_MS: u64 = 120_000;

/// How far a session's first event may precede its own process start — a clock-skew
/// allowance, **not** a matching window.
///
/// The rule this encodes is physical: *a session cannot write an event before the process
/// that creates it exists.* Anything earlier belongs to a different session, however
/// close in time. Only measurement error should ever produce a negative delta, so this is
/// sized for that and nothing more: process start comes from `PROC_PIDTBSDINFO` and log
/// timestamps from the same wall clock on the same machine, and the 5 genuine matches
/// measured on 2026-08-01 were all *positive* (+1s, +2s, +1s, +2s, +1s).
///
/// It was 120_000 — a symmetric window — and that shipped the defect this constant
/// exists to close. Two tabs in one project, `claude` started 16s apart: tab 1's log had
/// a first event **15s before tab 2's process existed**, the symmetric rule called that a
/// match, and both tabs rendered one dashboard. Under D4 the wrong attachment was
/// immediate and permanent. Widening this is not a tuning knob — it re-admits impossible
/// matches, so any increase needs the same evidence this decrease has.
const FIRST_EVENT_LEAD_MS: u64 = 5_000;

/// Resolve a log in `dir` that the caller can *prove* is theirs — or nothing.
///
/// Two tiers, in order: the id `want` names outright, then the log whose first event
/// dates it to the caller's own process start (within [`FIRST_EVENT_SLACK_MS`]).
/// When neither confirms, the answer is `None` rather than a guess: the directory
/// records no application, window or tab — Warp, iTerm and every other mac-terminal
/// window writing in the same project all append here — so an unconfirmed pick is a
/// foreign session as often as not.
///
/// Every [`Resolved`] this returns has `owned: true` by construction; there is no
/// code path here that builds `owned: false`. This is what production calls.
/// `rivals` are the start times of the OTHER live `claude` processes working in the same
/// project — sibling tabs of this app, and other applications' sessions alike. They are
/// what makes tier 2 an *assignment* rather than a guess: a log belongs to the latest
/// process that started at or before its first event, so a rival that started after us
/// but still before the log was written owns it, not us. Pass `&[]` when unknown; the
/// rule then degrades to the distance test alone, which is where this started.
/// `rivals_known` says whether the caller could actually enumerate the rivals, as opposed
/// to handing back an empty list because a read failed. It gates the uniqueness rescue:
/// an empty rival set is a *privilege* under that rule, and a privilege must never be
/// granted by a failed read.
pub fn resolve_owned_session(
    dir: &Path,
    want: &Wanted,
    rivals: &[Rival],
    rivals_known: bool,
) -> Option<Resolved> {
    resolve_owned_session_with(dir, want, rivals, rivals_known, registered_session)
}

/// [`resolve_owned_session`]'s body, generic over the registry read so the rival rules are
/// testable without a real `~/.claude/sessions` — the same split `app`'s
/// `registered_id_with` and `resolved_root_with` already use.
fn resolve_owned_session_with(
    dir: &Path,
    want: &Wanted,
    rivals: &[Rival],
    rivals_known: bool,
    read: impl Fn(u32) -> Option<Registered>,
) -> Option<Resolved> {
    // Named outright: nothing to rank, and nothing to fall back to.
    //
    // A session id here is *authoritative* — it comes from `claude` naming its own session
    // (`~/.claude/sessions/<pid>.json`) or from `--resume <id>` in its argv — so if its log
    // does not exist yet, the honest answer is "not written yet", NOT a timing guess. The
    // fall-through this used to do is what let a tab with a perfectly known session id
    // adopt a sibling's log during the window before its own first write.
    if let Some(id) = want.session_id.as_deref() {
        let path = dir.join(format!("{id}.jsonl"));
        return path.is_file().then(|| Resolved {
            last_event_ms: last_event_in_tail(&path),
            path,
            owned: true,
        });
    }
    // Dated — the FALLBACK tier, and it is worth being explicit about its standing.
    //
    // Tier 1 above answers by NAME, from a record `claude` writes about itself. Measured
    // 2026-08-02 on 2.1.220: that record appears 1.1-2.2s after process start, with no
    // activity required, through a plain pty, through zsh, through a generated ZDOTDIR and
    // through this app's own PTY. So in the ordinary case tier 2 is never reached.
    //
    // It is NOT vestigial, and must not be documented as covering a closed list of cases.
    // The registry is another program's undocumented on-disk detail (see `Registered`,
    // which states it "must never be load-bearing beyond 'use it when it is there'"), and
    // in the same 2026-08-02 sample 1 of 5 live `claude` processes had no record after
    // 1h17m for reasons never identified. Tier 2 exists because tier 1 can be absent for
    // reasons we do not control — which is also why the uniqueness rescue below was
    // narrowed rather than deleted.
    //
    // The caller's process started at `since`, so its session's FIRST event
    // follows shortly AFTER — never before. The two sides of this window mean different
    // things and are sized differently on purpose: below is a clock-skew allowance on an
    // impossibility, above is a real tolerance for how long a session may take to write.
    // A symmetric window here is what let one tab's log be confirmed as another's,
    // 15 seconds before that tab's process had even started.
    if let Some(since) = want.since_ms {
        // Everything that could be ours: a log that began at or after our process did
        // (the hard invariant — see FIRST_EVENT_LEAD_MS), and that no OTHER live claude
        // has a better claim to.
        //
        // The rival test is what turns this from a distance heuristic into an assignment.
        // A log is written by exactly one process, and that process must have existed
        // when it was written; among the processes that qualify, the latest one to start
        // is the writer. So a rival that started after us but at or before this log's
        // first event outranks us for it — no matter how close the log sits to our own
        // start. Without this, two tabs opened seconds apart in one project each see the
        // other's session as plausibly their own, which is the defect this feature has
        // now been through twice: once with the tabs one way round, once the other.
        // Which session each rival is running, read once for the whole call (R5.4).
        let claims = rival_claims(rivals, &read);
        // Whether any rival could still be THIS candidate's writer, per candidate. A rival
        // that names another session is not merely out-voted, it is out of the question —
        // and D3's rescue keys on this *effective* emptiness rather than on `rivals.len()`.
        // Testing the raw list instead makes the two rules cancel: a rival far enough out
        // to have vetoed us also puts the log outside the slack, so lifting the veto
        // changes nothing. That is not hypothetical — it is this feature's own reported
        // timeline (rival at +28s, first event at +900s).
        // Contested-ness travels WITH each candidate rather than being accumulated across
        // the loop: a rival that contests some other candidate says nothing about this one,
        // and the rescue is decided for the single candidate that survives.
        let after: Vec<(u64, PathBuf, bool)> = candidates(dir)
            .into_iter()
            .filter_map(|p| {
                let (first, content_id) = head_facts(&p);
                let first = first?;
                if first + FIRST_EVENT_LEAD_MS < since {
                    return None; // predates us: impossible
                }
                let cand_id = candidate_identity(&p, content_id);
                // `<= first`, with NO skew allowance — deliberately asymmetric with the
                // test above, because the two comparisons mean different things.
                //
                // Above, `+ FIRST_EVENT_LEAD_MS` tolerates measurement error in *our own*
                // claim, and erring wide there costs nothing: it only keeps a candidate we
                // then have to justify. Here, widening REFUSES a log, and a rival that
                // started after the log was already written provably did not write it. An
                // earlier revision used `<= first + LEAD` here too, out of a false sense of
                // symmetry, and it blanked the panel of the tab that actually wrote the
                // log whenever a second tab opened within LEAD of it — the ordinary
                // workflow, and a worse failure than the one it was fixing.
                //
                // D9's three outcomes replace the bare start-time test. A rival is only
                // *inferred* to outrank us when nothing is known about it; when its own
                // record names a session, that record answers the question directly and
                // timing is not consulted at all.
                let mut outranked = false;
                let mut contested = false;
                for (r_start, r_id) in &claims {
                    match claim_of(r_id.as_deref(), cand_id.as_deref()) {
                        // Outcome 1 — it owns this log. Absolute: no distance, slack or
                        // uniqueness rule can hand it to us.
                        Claim::OwnsIt => {
                            outranked = true;
                            contested = true;
                        }
                        // Outcome 2 — it is writing some other session, or has written
                        // nothing at all. Either way it did not write THIS log, so it
                        // neither vetoes nor counts against the rescue.
                        Claim::Elsewhere => {}
                        // Outcome 3 — nothing known: the original timing veto stands.
                        Claim::Unknown => {
                            contested = true;
                            if *r_start > since && *r_start <= first {
                                outranked = true;
                            }
                        }
                    }
                }
                (!outranked).then_some((first, p, contested))
            })
            .collect();
        // Uniqueness discriminates where distance cannot: if exactly one session began
        // after us and nothing else can account for it, it is ours however long the user
        // sat at the splash before typing. The alternative is a panel empty for the life
        // of the tab, since `since` is the process start and never moves.
        //
        // TWO conditions gate it, and each closes a defect this feature actually shipped:
        //
        // - `!contested` — no rival could still be THIS candidate's writer. Keying on the
        //   raw rival count instead makes the veto fix inert: for a rival to have silenced
        //   us at all it must satisfy `since < r <= first`, which forces
        //   `first - since >= r - since`, so any rival far enough out to veto also pushes
        //   the log past the slack. Lifting the veto then changes nothing and the writer
        //   stays empty — on this feature's own reported timeline.
        //
        // - `rivals_known` — the caller actually enumerated. An empty rival list is also
        //   what a FAILED read looks like (an unreadable cwd, ours or a rival's), and
        //   since this arm adopts at unbounded distance with `owned: true`, granting it on
        //   a failed read would confirm a foreign log and latch it.
        //
        // With two or more candidates, uniqueness proves nothing and proximity takes over:
        // the nearest inside the slack, or nothing.
        let owned = match after.len() {
            1 if !after[0].2 && rivals_known => after.into_iter().next(),
            _ => after
                .into_iter()
                .filter(|(first, _, _)| *first <= since.saturating_add(FIRST_EVENT_SLACK_MS))
                .min_by_key(|(first, _, _)| first.abs_diff(since)),
        };
        if let Some((_, path, _)) = owned {
            return Some(Resolved {
                last_event_ms: last_event_in_tail(&path),
                path,
                owned: true,
            });
        }
    }
    None
}

/// Resolve the log for `dir`, preferring one the caller can prove is theirs and
/// falling back to ranking the directory when nothing confirms.
///
/// **Production calls [`resolve_owned_session`], not this.**
///
/// Its behavior is **unchanged** by the rival-assignment work, and structurally so rather
/// than test-by-test: it hard-codes `&[]` with `rivals_known: true`, so every rule that
/// change introduced — the per-rival record check, the effective-set gate, the
/// known/unknown flag — is inert here by construction. The uniqueness rescue is
/// unconditionally enabled on this path, exactly as before.
///
/// This is the ranked-fallback contract, retained under test with no production caller: it is
/// the only thing keeping [`most_recent_session`]'s ranking, the `owned: false` half
/// of [`Resolved`], and the app's displacement/re-attach rules exercised. Its
/// fallback answers "the session that most recently wrote here", which may belong to
/// any application — never "this tab's session".
pub fn resolve_session(dir: &Path, want: &Wanted) -> Option<Resolved> {
    resolve_owned_session(dir, want, &[], true).or_else(|| {
        most_recent_session(dir).map(|(path, last_event_ms)| Resolved {
            path,
            last_event_ms,
            owned: false,
        })
    })
}

/// What one bounded head read of `log` tells us: when it began, and which session its own
/// lines say is writing it.
///
/// Both come from a **single** read because both are needed per candidate and the read is
/// the expensive part. `session_id` is the field D13 turns on: it answers "which session is
/// *writing* this file", where the filename stem only answers "which session *created* it".
/// The two disagree for a resumed / forked / compacted transcript, which is precisely the
/// case where trusting the stem hands a live rival's log to the wrong tab.
fn head_facts(log: &Path) -> (Option<u64>, Option<String>) {
    let Ok(f) = std::fs::File::open(log) else {
        return (None, None);
    };
    let mut buf = Vec::new();
    if f.take(activity::TAIL_CAP).read_to_end(&mut buf).is_err() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut first = None;
    let mut session_id = None;
    for v in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        if let Some(t) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(telemetry::parse_ts)
        {
            first = Some(first.map_or(t, |cur: u64| cur.min(t)));
        }
        if session_id.is_none() {
            session_id = v
                .get("sessionId")
                .or_else(|| v.get("session_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    (first, session_id)
}

/// How a live rival relates to one candidate log — D9's three outcomes.
enum Claim {
    /// Its record names THIS log: the rival owns it and we must never take it.
    OwnsIt,
    /// Its record names some OTHER session: it is writing elsewhere, or nowhere, and
    /// either way it did not write this log. It does not veto.
    Elsewhere,
    /// No record, or one that failed validation. Nothing is known, so the start-time
    /// veto — the only rule available before this change — still applies.
    Unknown,
}

/// Which session each rival is running, read ONCE per resolution rather than per candidate
/// (R5.4: 16 candidates × 3 rivals must cost 3 reads, not 48).
fn rival_claims(
    rivals: &[Rival],
    read: &impl Fn(u32) -> Option<Registered>,
) -> Vec<(u64, Option<String>)> {
    rivals
        .iter()
        .map(|rv| {
            let id = read(rv.pid)
                .and_then(|reg| validated_session_id(&reg, rv.started_ms).map(str::to_string));
            (rv.started_ms, id)
        })
        .collect()
}

/// Classify one rival against one candidate.
fn claim_of(rival_id: Option<&str>, candidate_id: Option<&str>) -> Claim {
    match (rival_id, candidate_id) {
        (Some(r), Some(c)) if r == c => Claim::OwnsIt,
        (Some(_), _) => Claim::Elsewhere,
        (None, _) => Claim::Unknown,
    }
}

/// The identity of a candidate log: what its own lines say, else its filename stem.
///
/// The stem is a **fallback, not a peer**. When the content id is unreadable we know less,
/// not more, so the comparison must not be allowed to conclude "different session" from it
/// (R1.5) — callers treat a stem-only match as evidence of ownership and a stem-only
/// mismatch as no evidence at all.
fn candidate_identity(path: &Path, content_id: Option<String>) -> Option<String> {
    content_id.or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
    })
}

/// The `*.jsonl` files worth considering, newest-touched first and capped.
fn candidates(dir: &Path) -> Vec<PathBuf> {
    let mut by_mtime: Vec<(std::time::SystemTime, PathBuf)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .map(|e| {
                let m = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (m, e.path())
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    by_mtime.sort_by_key(|(m, _)| std::cmp::Reverse(*m));
    by_mtime.truncate(RESOLVE_CANDIDATES);
    by_mtime.into_iter().map(|(_, p)| p).collect()
}

pub fn most_recent_session(dir: &Path) -> Option<(PathBuf, Option<u64>)> {
    let mut best: Option<(Option<u64>, PathBuf)> = None;
    for path in candidates(dir) {
        let last = last_event_in_tail(&path);
        // A log with no parseable event in its tail still beats nothing at all, but
        // loses to any log that has one — `None` sorts below every `Some`. Ties keep
        // the first seen, and candidates arrive newest-touched first.
        let better = match &best {
            None => true,
            Some((b, _)) => last > *b,
        };
        if better {
            best = Some((last, path));
        }
    }
    best.map(|(last, path)| (path, last))
}

/// The newest parseable `timestamp` in the final [`activity::TAIL_CAP`] bytes of
/// `log`, in epoch millis. `None` if the file is unreadable, empty, or has no
/// timestamped line in that window.
fn last_event_in_tail(log: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(log).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(activity::TAIL_CAP);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.take(activity::TAIL_CAP).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // No need to drop the first line after seeking: a record cut mid-way fails to
    // parse anyway, and skipping unconditionally would discard a COMPLETE line
    // whenever the seek happens to land on a newline — losing the only timestamp in
    // the window, which reads as "this session is undated". `activity::tail_state`
    // does not skip either.
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            v.get("timestamp")
                .and_then(Value::as_str)
                .and_then(telemetry::parse_ts)
        })
        .max()
}

/// The most-recently-modified `*.jsonl` in `dir`, if any.
///
/// This is a **directory**-level heuristic with no binding to any process: a live
/// `claude` holds no open fd on its own log (it opens, appends, closes), so there is
/// nothing to attribute a file to a PTY's descendant by. Callers must treat the
/// result as "the session that most recently wrote here", not "this tab's session"
/// — see [`LogWatcher::spawn`], which re-resolves until the caller says it has
/// attached, and the app's attach rule, which refuses to present a log that stopped
/// writing before the window existed.
pub fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| mtime >= *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Parse one JSONL line into an [`AiEvent`]. Returns `None` only if the line is
/// not valid JSON (R1.5 — malformed lines are skipped, never panic). A valid JSON
/// object always yields an event (role/summary best-effort).
pub fn parse_line(line: &str) -> Option<AiEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let role = v
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| v.pointer("/message/role").and_then(Value::as_str))
        .unwrap_or("?")
        .to_string();
    let summary = event_summary(&v);
    Some(AiEvent { role, summary })
}

/// Best-effort one-line summary: first text content, else a `summary`/`content`
/// string field, else the event type.
fn event_summary(v: &Value) -> String {
    // message.content[].text (assistant/user turns)
    if let Some(items) = v.pointer("/message/content").and_then(Value::as_array) {
        for it in items {
            if let Some(t) = it.get("text").and_then(Value::as_str) {
                return one_line(t);
            }
        }
    }
    if let Some(s) = v.get("summary").and_then(Value::as_str) {
        return one_line(s);
    }
    if let Some(s) = v.pointer("/message/content").and_then(Value::as_str) {
        return one_line(s);
    }
    v.get("type")
        .and_then(Value::as_str)
        .unwrap_or("event")
        .to_string()
}

/// Collapse to a single trimmed line, capped for the overlay.
pub(crate) fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 200 {
        flat.chars().take(199).collect::<String>() + "…"
    } else {
        flat
    }
}

/// Read the last `n` parseable events already in `log`, plus the file's current
/// byte length (the offset to tail from). `notify` only signals *future* appends,
/// so the overlay must backfill what's already there (D1). Missing file → empty.
pub fn backfill(log: &Path, n: usize) -> (Vec<AiEvent>, u64) {
    let Ok(bytes) = std::fs::read(log) else {
        return (Vec::new(), 0);
    };
    let len = bytes.len() as u64;
    let text = String::from_utf8_lossy(&bytes);
    let mut events: Vec<AiEvent> = text.lines().filter_map(parse_line).collect();
    if events.len() > n {
        events.drain(0..events.len() - n);
    }
    (events, len)
}

/// Fold the whole of `log` into a fresh [`SessionTelemetry`], returning it plus the
/// file's current byte length — the offset to tail from.
///
/// `notify` signals only *future* appends, so opening the overlay must seed the
/// accumulator from what is already in the file. This runs where [`backfill`] runs:
/// on the caller's thread at open, which already reads and parses the whole file
/// today, so folding instead of building `AiEvent`s is the same order of cost.
/// Missing file → empty state, offset 0, no error.
pub fn backfill_telemetry(log: &Path) -> (SessionTelemetry, u64) {
    let mut t = SessionTelemetry::new();
    let Ok(bytes) = std::fs::read(log) else {
        return (t, 0);
    };
    let len = bytes.len() as u64;
    for line in String::from_utf8_lossy(&bytes).lines() {
        t.apply(line);
    }
    (t, len)
}

/// A running tail on a log file. Dropping it stops the watch thread.
///
/// The thread OWNS the [`SessionTelemetry`] and sends an immutable snapshot per
/// append batch (SD5) — not the raw line (which would move JSON parsing onto the UI
/// thread) and not a per-line event (which is what discarded the data this feature
/// exists to accumulate). `Arc` because Claude Code appends line-by-line, so "one
/// send per batch" is in practice one send per line, and a bare snapshot would deep
/// clone ~112 ring entries plus the tasks map every time.
pub struct LogWatcher {
    _watcher: notify::RecommendedWatcher,
    pub rx: Receiver<Update>,
}

/// What to watch, and what the caller already knows about it.
///
/// A struct rather than five parameters: they are one decision — *which* session this
/// watch is for — and passing them separately invites a call site that gets the
/// pairing wrong.
pub struct Watch {
    /// The project directory to watch.
    pub dir: PathBuf,
    /// The log resolved so far, if any.
    pub log: Option<PathBuf>,
    /// What the caller knows about which session it wants, used on re-resolution.
    pub want: Wanted,
    /// Whether `log` was confirmed to be the caller's.
    pub owned: bool,
    /// The other live `claude` processes in this project — see [`resolve_owned_session`].
    ///
    /// **Shared, not captured.** Membership changes during an open: a sibling that starts
    /// `claude` after the panel opened would never veto, so its log could be adopted with
    /// `owned: true`; a sibling that exits would keep vetoing and silence a tab that now
    /// legitimately owns its log. The caller republishes through this handle from the
    /// thread that already enumerates processes; the watcher reads it under the lock and
    /// clones, never holding it across file I/O.
    pub rivals: RivalHandle,
    /// Set by the caller once it has attached; the watcher then stops re-resolving.
    pub attached: Arc<AtomicBool>,
}

/// The live rival set, shared between the producer (`app`) and the watch thread.
pub type RivalHandle = Arc<Mutex<RivalSet>>;

/// The rivals, and whether that list is a COMPLETE enumeration.
///
/// `known: false` is the safe default — it withholds the uniqueness rescue — and it is
/// what [`Default`] yields, so a caller that forgets to publish cannot accidentally grant
/// the strongest branch in the resolver.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RivalSet {
    pub list: Vec<Rival>,
    pub known: bool,
}

impl RivalSet {
    /// A fully-enumerated set.
    pub fn known(list: Vec<Rival>) -> Self {
        RivalSet { list, known: true }
    }
}

/// One snapshot, and whether the log it came from was confirmed to be the caller's.
#[derive(Clone, Debug)]
pub struct Update {
    pub snap: Arc<TelemetrySnapshot>,
    /// See [`Resolved::owned`]. Carried per update because re-resolution can turn an
    /// unowned attachment into an owned one — the moment the tab's own session
    /// finally writes.
    pub owned: bool,
}

impl LogWatcher {
    /// Start tailing `log` from `start_offset`, folding each newly-appended complete
    /// line into `telemetry` (seed it with [`backfill_telemetry`]).
    ///
    /// Sends ONE `Arc<TelemetrySnapshot>` per append batch and calls `wake` once, so
    /// the caller can wake its event loop and drain. `wake` runs on the watch thread
    /// — keep it cheap (e.g. an `EventLoopProxy::send_event`).
    pub fn spawn(
        watch: Watch,
        mut telemetry: SessionTelemetry,
        start_offset: u64,
        wake: impl Fn() + Send + 'static,
    ) -> Option<LogWatcher> {
        let Watch {
            dir,
            log,
            want,
            mut owned,
            rivals,
            attached,
        } = watch;
        let watch_dir = dir.clone();
        let (tx, rx) = mpsc::channel::<Update>();
        let mut offset = start_offset;
        let mut partial = String::new();
        let mut target = log;
        // *Whether* to keep re-resolving is the CALLER's decision, shared by handle.
        //
        // That question — "has this window attached to a session yet" — has exactly
        // one author, `app::overlay`, because the last time it was implemented twice
        // the two copies disagreed and pinned the watcher to a log the display would
        // never show. This crate does not re-derive it; it reads the flag. (The app's
        // rule for *when* to attach has since changed — a confirmed, non-empty log
        // attaches on sight rather than having to prove it wrote since the window
        // opened — which is precisely why it is stated there and not here.)
        //
        // *What* re-resolution may adopt is this crate's, and it is strict:
        // `resolve_owned_session` only, so the target can move only to a log the
        // caller can prove is its own — named by `--resume`, or dated to the caller's
        // own process start. A hint that confirms nothing answers `None` on every
        // callback and the target never moves. That is the point rather than a
        // shortfall: the project directory records no application, window or tab, so
        // a tab with no identifiable session must show nothing rather than adopt
        // whatever else happens to be writing there. Consequently the only value
        // re-resolution can assign to `owned` is `true`; the field remains because
        // the caller's initial value still flows through untouched when no switch
        // happens, and because `Update.owned` is what the app reads to decide
        // attachment.
        //
        // Until the caller attaches, a different file appearing may well be the tab's
        // own session writing for the first time, so a *confirmed* one takes over.
        // Once the caller has attached, we stay: two concurrently active sessions in
        // one project would otherwise trade the attachment on every line, resetting
        // the accumulator and re-reading the whole file each time — and, worse,
        // replacing data already on screen.

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let mut switched = false;
            if !attached.load(Ordering::Relaxed) {
                // A new session's first line lands here: the file did not exist when
                // the overlay opened, so no amount of tailing the old target would
                // ever find it.
                if let Some(Resolved {
                    path: newest,
                    owned: newly_owned,
                    ..
                }) = {
                    // Re-read membership AND each rival's record on every re-resolution:
                    // a sibling in its first ~2s has no record yet, and that is exactly
                    // the state in which it would otherwise keep vetoing us.
                    let snapshot = rivals.lock().map(|g| g.clone()).unwrap_or_default();
                    resolve_owned_session(&dir, &want, &snapshot.list, snapshot.known)
                } {
                    if Some(&newest) != target.as_ref() {
                        owned = newly_owned;
                        target = Some(newest);
                        offset = 0;
                        partial.clear();
                        telemetry = SessionTelemetry::new();
                        switched = true;
                    }
                }
            }
            let Some(target) = target.clone() else { return };
            let name = target.file_name();
            if !switched && !ev.paths.iter().any(|p| p.file_name() == name) {
                return;
            }
            // Read everything past our offset; parse whole lines, buffer a partial.
            let Ok(mut f) = std::fs::File::open(&target) else {
                return;
            };
            let Ok(size) = f.metadata().map(|m| m.len()) else {
                return;
            };
            let mut reset = false;
            if size < offset {
                // File shrank/rotated — restart from the top AND drop the
                // accumulator. Until there was accumulated state a rewind was
                // enough; continuing to fold a different session's lines into the
                // previous session's totals would silently produce nonsense.
                offset = 0;
                partial.clear();
                telemetry = SessionTelemetry::new();
                reset = true;
            }
            if f.seek(SeekFrom::Start(offset)).is_err() {
                return;
            }
            // Read to the *actual* EOF and advance the offset by exactly the bytes
            // consumed — not the pre-read `size`, which can be stale if a writer
            // appends between the metadata() call and this read (→ duplicates).
            // Byte read + lossy decode also survives a multibyte split (matches
            // `backfill`), where `read_to_string` would error and stall.
            let mut buf = Vec::new();
            let Ok(n) = f.read_to_end(&mut buf) else {
                return;
            };
            offset += n as u64;
            partial.push_str(&String::from_utf8_lossy(&buf));
            let mut folded = 0usize;
            while let Some(nl) = partial.find('\n') {
                let line: String = partial.drain(..=nl).collect();
                telemetry.apply(&line);
                folded += 1;
            }
            // ONE snapshot per batch, not per line: snapshots are cumulative, so
            // every one but the last is stale before the UI thread wakes.
            //
            // A RESET must emit too, even with nothing folded. A rotation typically
            // truncates to zero first, so the callback fires, the state resets and the
            // read yields no complete line; under a "≥1 line" condition nothing would
            // be sent and this crate would have silently dropped a state change. What
            // the CALLER does with an empty snapshot is the caller's decision — the
            // app deliberately refuses it rather than blank a panel someone is
            // reading. Reporting the change and deciding on it are different jobs.
            // Bytes read with no complete line (a buffered
            // partial) correctly send nothing: no state changed.
            // ONE snapshot, not two: it deep-clones five rings plus the task map
            // and sorts, on every append batch.
            let snap = Arc::new(telemetry.snapshot());
            if (folded > 0 || reset || switched) && tx.send(Update { snap, owned }).is_ok() {
                wake();
            }
        })
        .ok()?;
        // The DIRECTORY, not the file: a session that has not written yet has no
        // file to watch, and that is exactly the case this exists to cover.
        //
        // If the project directory does not exist either — the first Claude session
        // ever run in this cwd, the purest form of the same case — `watch` errors on
        // the missing path and the overlay would never arm at all, with nothing to
        // retry it. Fall back to the nearest existing ancestor, **recursively**, so
        // the directory's creation and the appends to the log inside it are both
        // seen; a non-recursive watch on `~/.claude/projects` reports only its direct
        // children, so it would notice the new directory and then go deaf to every
        // write in it. Creating the directory ourselves would be simpler and is
        // deliberately not done: it is a write under `~/.claude`, which this contract
        // fences off.
        //
        // The cost is events from other projects while this one is empty. It is
        // bounded — each triggers one `read_dir` plus at most RESOLVE_CANDIDATES
        // bounded tail reads (<= 128 KiB), it stops the moment the caller attaches,
        // it runs on the watch thread rather than the render thread, and the watcher
        // exists only while the panel is open.
        let (watch_path, mode) = if watch_dir.is_dir() {
            (watch_dir, RecursiveMode::NonRecursive)
        } else {
            // Exactly one level up — the projects root — and no further. If that is
            // missing too then no Claude session has ever run anywhere, and arming a
            // recursive watch on `$HOME` to wait for the first one is not a trade
            // worth making: every write in the user's home directory would wake this
            // callback for as long as the panel stayed open.
            let root = watch_dir.parent()?;
            if !root.is_dir() {
                return None;
            }
            (root.to_path_buf(), RecursiveMode::Recursive)
        };
        watcher.watch(&watch_path, mode).ok()?;
        Some(LogWatcher {
            _watcher: watcher,
            rx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rival identified only by when it started — the shape every tier-2 test used
    /// before rivals carried a pid. The pid is irrelevant to the timing veto, so these
    /// get a distinct dummy that resolves to no registry file, which is exactly the
    /// "no record, fall back to the start-time veto" case they were written for.
    fn rival(started_ms: u64) -> Rival {
        Rival {
            pid: u32::MAX,
            started_ms,
        }
    }

    /// Epoch millis -> RFC3339 UTC, so a test can state a time once as a number instead of
    /// keeping a hand-computed string in sync with it.
    fn iso(ms: u64) -> String {
        let secs = (ms / 1000) as i64;
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        let (h, mi, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.000Z")
    }

    /// A rival that registered its own session, `drift_ms` after its process start.
    fn registered_rival(started_ms: u64, session_id: &str, drift_ms: u64) -> Registered {
        Registered {
            session_id: session_id.to_string(),
            cwd: PathBuf::from("/proj"),
            started_at_ms: started_ms + drift_ms,
        }
    }

    /// Write a log whose FIRST event is `first_ms`, carrying `content_id` on its lines.
    fn write_log(dir: &Path, stem: &str, first_ms: u64, content_id: Option<&str>) {
        let ts = iso(first_ms);
        let sid = match content_id {
            Some(id) => format!(",\"sessionId\":\"{id}\""),
            None => String::new(),
        };
        std::fs::write(
            dir.join(format!("{stem}.jsonl")),
            format!("{{\"type\":\"assistant\",\"timestamp\":\"{ts}\"{sid},\"message\":{{}}}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn an_unreadable_content_id_falls_back_to_the_stem_and_keeps_the_veto() {
        // R1.5 — the content id is a SHARPER signal than the stem, not a replacement that
        // may fail open. A log whose head carries no `sessionId` tells us less, so the
        // comparison must not be allowed to conclude "a different session" from it.
        //
        // Here the rival's record names the log's STEM. If the missing content id were
        // treated as "no match", the rival would be disqualified (outcome 2), the effective
        // set would empty, and we would adopt the rival's own live log with `owned: true`.
        const OURS: u64 = 1_785_240_000_000;
        const RIVAL_START: u64 = OURS - 5_000; // earlier than us: no timing veto to fall back on
        let dir = temp_dir("no-content-id");
        write_log(&dir, "theirs", OURS + 1_000, None); // no sessionId on the line

        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let read = |_: u32| Some(registered_rival(RIVAL_START, "theirs", 1_000));
        assert_eq!(
            resolve_owned_session_with(
                &dir,
                &want,
                &[Rival {
                    pid: 9,
                    started_ms: RIVAL_START,
                }],
                true,
                read,
            ),
            None,
            "unreadable content id must not read as 'a different session'"
        );
    }

    #[test]
    fn each_rival_registry_is_read_once_per_resolution_not_once_per_candidate() {
        // R5.4 — the read is per RIVAL, hoisted out of the candidate loop. Placed naively
        // inside the per-candidate filter it would be O(candidates x rivals) file reads on
        // every fs event, stacked on the 16-candidate head reads resolution already pays.
        const OURS: u64 = 1_785_240_000_000;
        let dir = temp_dir("read-count");
        for i in 0..8 {
            write_log(
                &dir,
                &format!("log{i}"),
                OURS + 1_000 + i,
                Some(&format!("log{i}")),
            );
        }

        let calls = std::cell::Cell::new(0usize);
        let read = |_: u32| {
            calls.set(calls.get() + 1);
            None
        };
        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let rivals: Vec<Rival> = (1..=3)
            .map(|pid| Rival {
                pid,
                started_ms: OURS - 1_000,
            })
            .collect();

        let _ = resolve_owned_session_with(&dir, &want, &rivals, true, read);
        assert_eq!(
            calls.get(),
            3,
            "8 candidates x 3 rivals must cost 3 registry reads, not 24"
        );
    }

    #[test]
    fn the_rival_handle_is_read_live_not_captured() {
        // R6.2 — membership churn in BOTH directions. The watcher holds a handle, so a
        // sibling that starts `claude` after the panel opened begins vetoing, and one that
        // exits stops. Captured-by-value, both are wrong for the life of the open: the new
        // rival never vetoes (its log can be adopted with `owned: true` — F2), and the dead
        // one vetoes forever (silencing a tab that now owns its log).
        //
        // Exercised through the same lock-then-clone the watch callback performs, since
        // driving a real fs event would test `notify`, not this rule.
        const OURS: u64 = 1_785_240_000_000;
        const FIRST: u64 = OURS + 30_000;
        let dir = temp_dir("live-rival-handle");
        write_log(&dir, "log", FIRST, Some("log"));
        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let read = |_: u32| None; // no records: the timing veto governs
        let handle: RivalHandle = Arc::new(Mutex::new(RivalSet::known(Vec::new())));

        let resolve = |h: &RivalHandle| {
            let snap = h.lock().map(|g| g.clone()).unwrap_or_default();
            resolve_owned_session_with(&dir, &want, &snap.list, snap.known, read)
        };

        assert!(resolve(&handle).is_some(), "no rivals yet: the log is ours");

        // A sibling starts mid-open, between us and the log's first event.
        handle.lock().unwrap().list.push(Rival {
            pid: 4242,
            started_ms: OURS + 10_000,
        });
        assert_eq!(
            resolve(&handle),
            None,
            "a rival that appeared after open must still be able to outrank us"
        );

        // ...and exits again.
        handle.lock().unwrap().list.clear();
        assert!(
            resolve(&handle).is_some(),
            "a rival that exited must stop vetoing"
        );
    }

    // ---- rival-assignment: the reported defect, end to end ----

    const A_PID: u32 = 5550;
    const B_PID: u32 = 5771;

    #[test]
    fn the_writer_keeps_its_log_when_an_idle_registered_sibling_cannot_have_written_it() {
        // R0.1 + R2.3 — THE discriminating test. Tab A starts, tab B starts 28s later and
        // stays idle, A types 15 minutes in and writes the only log.
        //
        // Before this change: A's log was vetoed by B on start times alone, A resolved to
        // `None`, and B adopted A's log via uniqueness with `owned: true`.
        //
        // An implementation that gates the rescue on the RAW rival list also fails here:
        // B stops vetoing, but `rivals.len() == 1` keeps the slack filter on and 900s is
        // far outside it. Only the effective set — empty once B is disqualified — lets A
        // through. Two separate L2 review passes were needed to find that.
        const A_START: u64 = 1_785_240_000_000;
        const B_START: u64 = A_START + 28_000;
        const FIRST: u64 = A_START + 900_000;

        let dir = temp_dir("writer-keeps-its-log");
        write_log(&dir, "a-session", FIRST, Some("a-session"));

        let want_a = Wanted {
            session_id: None,
            since_ms: Some(A_START),
        };
        // B registered ~2s after starting, naming its OWN session, which has no log yet.
        let read = |pid: u32| match pid {
            B_PID => Some(registered_rival(B_START, "b-session", 2_000)),
            _ => None,
        };

        let a = resolve_owned_session_with(
            &dir,
            &want_a,
            &[Rival {
                pid: B_PID,
                started_ms: B_START,
            }],
            true,
            read,
        )
        .expect("A wrote this log and must keep it");
        assert_eq!(a.path.file_name().unwrap(), "a-session.jsonl");
        assert!(a.owned);

        // And the idle sibling gets nothing rather than A's dashboard (F2).
        let want_b = Wanted {
            session_id: None,
            since_ms: Some(B_START),
        };
        assert_eq!(
            resolve_owned_session_with(
                &dir,
                &want_b,
                &[Rival {
                    pid: A_PID,
                    started_ms: A_START
                }],
                true,
                read,
            ),
            None,
            "B is 900s from this log and did not write it"
        );
    }

    #[test]
    fn a_rival_that_owns_the_log_vetoes_absolutely() {
        // R1.1 — the record names THIS log. No distance, slack or uniqueness rule may
        // hand it to us, even though we would otherwise be a perfect timing match.
        const OURS: u64 = 1_785_240_000_000;
        const RIVAL_START: u64 = OURS - 5_000; // started BEFORE us: no timing veto at all
        let dir = temp_dir("rival-owns-it");
        write_log(&dir, "theirs", OURS + 1_000, Some("theirs"));

        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let read = |_: u32| Some(registered_rival(RIVAL_START, "theirs", 1_000));
        assert_eq!(
            resolve_owned_session_with(
                &dir,
                &want,
                &[Rival {
                    pid: 9,
                    started_ms: RIVAL_START
                }],
                true,
                read,
            ),
            None,
            "its own record says it is writing this file"
        );
    }

    #[test]
    fn the_content_session_id_beats_the_filename_stem() {
        // R1.4 / D13 — a forked or compacted transcript keeps appending to the ORIGINAL
        // file, so its `sessionId` and the filename disagree. Comparing against the stem
        // alone reads that as "a different session", disqualifies the true writer, and —
        // because the effective set then goes empty — adopts its live log at unbounded
        // distance with `owned: true`. The content id is what closes it.
        const OURS: u64 = 1_785_240_000_000;
        const RIVAL_START: u64 = OURS - 5_000;
        let dir = temp_dir("content-id-wins");
        // filename stem `old-file`, but the lines say `forked-session` is writing.
        write_log(&dir, "old-file", OURS + 1_000, Some("forked-session"));

        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let read = |_: u32| Some(registered_rival(RIVAL_START, "forked-session", 1_000));
        assert_eq!(
            resolve_owned_session_with(
                &dir,
                &want,
                &[Rival {
                    pid: 9,
                    started_ms: RIVAL_START
                }],
                true,
                read,
            ),
            None,
            "the rival IS the writer; the stem disagreeing must not disqualify it"
        );
    }

    #[test]
    fn an_unknown_rival_set_never_earns_the_unbounded_rescue() {
        // R4.3 / D12 — the same input that succeeds with `rivals_known: true` must fail
        // with `false`. An empty rival list is also what a FAILED enumeration looks like,
        // and this arm adopts at any distance with `owned: true`.
        const OURS: u64 = 1_785_240_000_000;
        let dir = temp_dir("unknown-rivals");
        write_log(&dir, "far-away", OURS + 5_000_000, Some("far-away"));
        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        let read = |_: u32| None;

        assert!(
            resolve_owned_session_with(&dir, &want, &[], true, read).is_some(),
            "known-empty: the sole candidate is ours"
        );
        assert_eq!(
            resolve_owned_session_with(&dir, &want, &[], false, read),
            None,
            "unknown: the same emptiness proves nothing"
        );
    }

    #[test]
    fn a_stale_rival_record_falls_back_to_the_timing_veto() {
        // R3.1 / R1.3 — a record that PREDATES the process now holding that pid is a
        // recycled-pid artefact. It must not be believed, and the rival then behaves
        // exactly as it did before this change.
        const OURS: u64 = 1_785_240_000_000;
        const RIVAL_START: u64 = OURS + 10_000;
        const FIRST: u64 = OURS + 30_000;
        let dir = temp_dir("stale-rival-record");
        write_log(&dir, "log", FIRST, Some("log"));
        let want = Wanted {
            session_id: None,
            since_ms: Some(OURS),
        };
        // Written 60s BEFORE this process started: impossible, therefore stale.
        let stale = |_: u32| {
            Some(Registered {
                session_id: "someone-else".to_string(),
                cwd: PathBuf::from("/proj"),
                started_at_ms: RIVAL_START - 60_000,
            })
        };
        assert_eq!(
            resolve_owned_session_with(
                &dir,
                &want,
                &[Rival {
                    pid: 9,
                    started_ms: RIVAL_START
                }],
                true,
                stale,
            ),
            None,
            "unbelievable record -> the start-time veto still applies"
        );
    }

    /// A fully-enumerated empty rival set, behind the handle `Watch` now takes.
    /// `known: true` because these tests genuinely have no rivals — as opposed to having
    /// failed to look, which is what `RivalSet::default()` means.
    fn no_rivals() -> RivalHandle {
        Arc::new(Mutex::new(RivalSet::known(Vec::new())))
    }

    /// A caller that has not attached yet: the watcher is free to re-resolve.
    fn unattached() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!("ai-monitor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn encodes_and_resolves_newest_log() {
        assert_eq!(
            encode_cwd(Path::new("/Users/x/my.proj")),
            "-Users-x-my-proj"
        );

        let root = temp_dir("resolve");
        let cwd = Path::new("/a/b");
        let proj = root.join(encode_cwd(cwd));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old.jsonl"), "{}\n").unwrap();
        // Ensure a distinct, later mtime for the newest file.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newest = proj.join("new.jsonl");
        std::fs::write(&newest, "{}\n").unwrap();

        assert_eq!(resolve_log_in(&root, cwd), Some(newest));
        // Unknown cwd → no dir → None (graceful, C4).
        assert_eq!(resolve_log_in(&root, Path::new("/nope")), None);
    }

    #[test]
    fn backfills_last_n_existing_events() {
        let dir = temp_dir("backfill");
        let log = dir.join("s.jsonl");
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}"#,
            r#"{"type":"summary","summary":"did a thing"}"#,
        ];
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();

        let (events, len) = backfill(&log, 10);
        assert_eq!(events.len(), 3, "all three existing events read");
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].summary, "hello");
        assert_eq!(events[2].summary, "did a thing");
        assert!(len > 0, "offset is the file length for tailing");

        // Cap keeps only the last N.
        let (capped, _) = backfill(&log, 2);
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].role, "assistant");
    }

    #[test]
    fn tail_emits_each_append_exactly_once() {
        // Retained from the AiEvent era with its ASSERTIONS rewritten against
        // snapshot fields — the no-duplicate property it guards is the whole reason
        // the read loop advances the offset by bytes actually consumed rather than
        // by a possibly-stale `size`.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("tail");
        let log = dir.join("s.jsonl");
        std::fs::write(&log, "").unwrap();
        let (t, off) = backfill_telemetry(&log);
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(log.clone()),
                want: Wanted::default(),
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        let append = |line: &str| {
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            writeln!(f, "{line}").unwrap();
        };
        let usage = |id: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","usage":{{"output_tokens":{out}}}}}}}"#
            )
        };

        append(&usage("m1", 10));
        let s1 = w.rx.recv_timeout(Duration::from_secs(10)).expect("first");
        assert_eq!(s1.snap.total_output, 10);

        append(&usage("m2", 5));
        let s2 = w.rx.recv_timeout(Duration::from_secs(10)).expect("second");
        assert_eq!(
            s2.snap.total_output, 15,
            "cumulative, and m1 counted exactly once"
        );

        // Nothing re-emitted: a stale-size offset would re-read and re-fold m1/m2.
        assert!(
            w.rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "no duplicate snapshots"
        );
    }

    #[test]
    fn backfill_and_tail_agree_on_the_model_across_the_boundary() {
        // Two paths reach the same field and they must not disagree: `open()` folds the
        // whole file from scratch, then the watcher tails only what is appended after
        // it. A rule that read one line at a time differently from a batch — say,
        // "replace only when the value differs from the previous LINE" — would agree on
        // both of these separately and disagree the moment a `/model` switch landed
        // across the seam, which is precisely where a live session puts it.
        //
        // Three placements of the switch relative to the boundary, and the invariant is
        // the same each time: tailing from a mid-file offset must land on exactly what
        // folding the finished file yields.
        use std::io::Write;
        use std::time::Duration;

        let fable = |id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","model":"claude-fable-5","usage":{{"output_tokens":1}}}}}}"#
            )
        };
        let opus = |id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","model":"claude-opus-5","usage":{{"output_tokens":1}}}}}}"#
            )
        };
        let synthetic = |id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","model":"<synthetic>","usage":{{"output_tokens":1}}}}}}"#
            )
        };

        // (tag, lines present at open, lines appended afterwards)
        let cases: [(&str, Vec<String>, Vec<String>); 3] = [
            // The switch is IN the tail — the backfill has only ever seen fable.
            (
                "switchintail",
                vec![fable("a"), fable("b")],
                vec![opus("c"), opus("d")],
            ),
            // The switch is in the BACKFILL — the tail only ever sees opus.
            (
                "switchinbackfill",
                vec![fable("a"), opus("b")],
                vec![opus("c")],
            ),
            // Nothing but a sentinel arrives after the switch: it must not undo it.
            (
                "sentinelaftertheswitch",
                vec![fable("a"), opus("b")],
                vec![synthetic("c")],
            ),
        ];

        for (tag, seeded, appended) in cases {
            let dir = temp_dir(tag);
            let log = dir.join("s.jsonl");
            std::fs::write(&log, format!("{}\n", seeded.join("\n"))).unwrap();

            let (t, off) = backfill_telemetry(&log);
            let at_open = t.snapshot().model.clone();
            let w = LogWatcher::spawn(
                Watch {
                    dir: dir.clone(),
                    log: Some(log.clone()),
                    want: Wanted::default(),
                    owned: false,
                    rivals: no_rivals(),
                    attached: unattached(),
                },
                t,
                off,
                || {},
            )
            .expect("spawn");

            {
                let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
                for line in &appended {
                    writeln!(f, "{line}").unwrap();
                }
            }

            let mut tailed = at_open;
            while let Ok(s) = w.rx.recv_timeout(Duration::from_secs(3)) {
                tailed = s.snap.model.clone();
            }

            // The reference: the same bytes, folded in one pass by a fresh accumulator —
            // what `open()` on the finished file would have produced.
            let (whole, _) = backfill_telemetry(&log);
            assert_eq!(
                tailed,
                whole.snapshot().model,
                "{tag}: backfill+tail must converge on what a from-scratch fold yields"
            );
            assert_eq!(
                tailed.as_deref(),
                Some("claude-opus-5"),
                "{tag}: and that is the model the session ended on"
            );
        }
    }

    #[test]
    fn the_registry_names_a_session_and_shrugs_at_anything_odd() {
        // The shape `claude` actually writes, taken verbatim from a live file.
        let root = temp_dir("registry");
        std::fs::write(
            root.join("45326.json"),
            r#"{"pid":45326,"sessionId":"2b1ca0f2-8a99-4aff-90f1-115839f17f01","cwd":"/Users/x/IdeaProjects/terminal","startedAt":1785449383738,"procStart":"Thu Jul 30 22:09:42 2026","version":"2.1.220","kind":"interactive","status":"busy"}"#,
        )
        .unwrap();
        let r = registered_session_in(&root, 45326).expect("reads the record");
        assert_eq!(r.session_id, "2b1ca0f2-8a99-4aff-90f1-115839f17f01");
        assert_eq!(r.cwd, PathBuf::from("/Users/x/IdeaProjects/terminal"));
        assert_eq!(r.started_at_ms, 1_785_449_383_738);

        // This is an undocumented on-disk detail of another program, so every way it can
        // disappoint us must be a `None` rather than a panic or a wrong answer.
        assert_eq!(registered_session_in(&root, 99999), None, "no file");
        std::fs::write(root.join("1.json"), "not json").unwrap();
        assert_eq!(registered_session_in(&root, 1), None, "unparseable");
        std::fs::write(root.join("2.json"), r#"{"pid":2,"cwd":"/x"}"#).unwrap();
        assert_eq!(registered_session_in(&root, 2), None, "no sessionId");
        std::fs::write(
            root.join("3.json"),
            r#"{"sessionId":"s","cwd":"/x","startedAt":"yesterday"}"#,
        )
        .unwrap();
        assert_eq!(
            registered_session_in(&root, 3),
            None,
            "startedAt not a number"
        );
    }

    #[test]
    fn an_authoritative_id_whose_log_is_missing_resolves_to_nothing_not_a_guess() {
        // The reason tier 1 no longer falls through. When `claude` has named its own
        // session, a missing log means "not written yet" — a fact — and guessing by time
        // from there is how a tab with a perfectly known session id came to adopt a
        // sibling's log in the window before its first write.
        let dir = temp_dir("authoritative");
        // A sibling's log sits here, freshly written and temptingly close in time.
        std::fs::write(
            dir.join("sibling.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T12:00:01.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let want = Wanted {
            session_id: Some("11111111-1111-1111-1111-111111111111".into()),
            since_ms: Some(1_785_585_600_000), // 12:00:00Z — tier 2 would take the sibling
        };
        assert_eq!(
            resolve_owned_session(&dir, &want, &[], true),
            None,
            "we know which session is ours; it just has not written yet"
        );

        // The moment our own log appears, it is named exactly — no timing involved.
        std::fs::write(
            dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T12:05:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let r = resolve_owned_session(&dir, &want, &[], true).expect("named outright");
        assert_eq!(
            r.path.file_name().unwrap(),
            "11111111-1111-1111-1111-111111111111.jsonl"
        );
        assert!(r.owned);
    }

    #[test]
    fn a_named_session_wins_outright() {
        // `claude --resume <id>` puts the answer on the command line, so there is
        // nothing to rank: the id names the file.
        let dir = temp_dir("named");
        let id = "54e576c2-bd76-4acd-8eed-bda54f60a155";
        std::fs::write(
            dir.join(format!("{id}.jsonl")),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-23T10:00:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        // A far more recently active log in the same directory must NOT win.
        std::fs::write(
            dir.join("other.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:59:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        let r = resolve_session(
            &dir,
            &Wanted {
                session_id: Some(id.to_string()),
                since_ms: None,
            },
        )
        .expect("resolves");
        assert_eq!(r.path.file_name().unwrap(), format!("{id}.jsonl").as_str());
        assert!(r.owned, "named by the process, so confirmed");
    }

    #[test]
    fn a_session_started_by_this_tab_beats_a_more_active_foreign_one() {
        // The reported case, in miniature: another application's session is live in
        // the same project directory and has the newer event, but it began days
        // before this tab's shell existed. Ranking alone picks it; the hint does not.
        let dir = temp_dir("owned");
        // Warp's: first event five days ago, still writing.
        std::fs::write(
            dir.join("foreign.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-23T17:06:00.000Z\",\"message\":{}}\n\
             {\"type\":\"assistant\",\"timestamp\":\"2026-07-28T20:44:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        // Ours: started just after the tab's claude process did, quieter.
        std::fs::write(
            dir.join("mine.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T20:40:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        // Ranking alone: the foreign log, because it is genuinely the most active.
        let (ranked, _) = most_recent_session(&dir).expect("ranks");
        assert_eq!(ranked.file_name().unwrap(), "foreign.jsonl");

        // With the tab's process start time, ours — and it is marked confirmed.
        let claude_started = 1_785_271_140_000; // 2026-07-28T20:39:00Z
        let r = resolve_session(
            &dir,
            &Wanted {
                session_id: None,
                since_ms: Some(claude_started),
            },
        )
        .expect("resolves");
        assert_eq!(r.path.file_name().unwrap(), "mine.jsonl");
        assert!(r.owned);
    }

    #[test]
    fn two_tabs_on_one_project_get_their_own_sessions() {
        // Both tabs' sessions qualify on "started after my process did", so the match
        // has to be the nearest one rather than the earliest.
        let dir = temp_dir("twotabs");
        std::fs::write(
            dir.join("first.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T20:47:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("second.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T20:52:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        let pick = |since: u64| {
            resolve_session(
                &dir,
                &Wanted {
                    session_id: None,
                    since_ms: Some(since),
                },
            )
            .map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned())
        };
        // tab 1's claude started 20:46:30, tab 2's at 20:51:30.
        assert_eq!(pick(1_785_271_590_000).as_deref(), Some("first.jsonl"));
        assert_eq!(pick(1_785_271_890_000).as_deref(), Some("second.jsonl"));
    }

    #[test]
    fn an_empty_hint_degrades_to_ranking_and_claims_nothing() {
        // No claude process, no permission, a sandbox: resolution must be no worse
        // than not having looked, and must not pretend the result is confirmed.
        let dir = temp_dir("nohint");
        std::fs::write(
            dir.join("a.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:00:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let r = resolve_session(&dir, &Wanted::default()).expect("still resolves");
        assert_eq!(r.path.file_name().unwrap(), "a.jsonl");
        assert!(!r.owned, "a guess is never reported as confirmed");

        // A named session that does not exist also degrades rather than failing.
        let r = resolve_session(
            &dir,
            &Wanted {
                session_id: Some("00000000-0000-0000-0000-000000000000".into()),
                since_ms: None,
            },
        )
        .expect("degrades");
        assert!(!r.owned);
    }

    #[test]
    fn nothing_confirmable_resolves_to_none_rather_than_a_foreign_log() {
        // The defect under repair. The directory holds one log and it is another
        // application's; the hint names a session that does not exist here and dates
        // the tab's process to a time nothing in the directory matches. The ranked
        // fallback would hand back `foreign.jsonl` with `owned: false`, and every
        // caller that ignored the flag would show it. Strict resolution answers
        // `None` — the assertion is against `None`, not against a different path.
        let dir = temp_dir("nothingconfirmable");
        std::fs::write(
            dir.join("foreign.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-23T17:06:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        let want = Wanted {
            session_id: Some("00000000-0000-0000-0000-000000000000".into()),
            since_ms: Some(1_785_271_140_000), // 2026-07-28T20:39:00Z — five days later
        };
        assert_eq!(resolve_owned_session(&dir, &want, &[], true), None);

        // ...while the retained fallback still degrades to ranking, unchanged. The
        // two functions differ in exactly this: what happens when nothing confirms.
        let ranked = resolve_session(&dir, &want).expect("the fallback still answers");
        assert_eq!(ranked.path.file_name().unwrap(), "foreign.jsonl");
        assert!(!ranked.owned);
    }

    #[test]
    fn an_empty_hint_confirms_nothing_whatever_the_directory_holds() {
        // No claude process, no permission to read one, a sandbox: with neither an id
        // nor a process start there is nothing to separate these logs from any other
        // application's, so directory contents cannot change the answer.
        let dir = temp_dir("emptyhintstrict");
        std::fs::write(
            dir.join("a.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:00:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:30:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        assert_eq!(
            resolve_owned_session(&dir, &Wanted::default(), &[], true),
            None
        );
        // An empty directory is the same answer for a different reason — stated so a
        // future change cannot pass this test by resolving the contents case only.
        let empty = temp_dir("emptyhintemptydir");
        assert_eq!(
            resolve_owned_session(&empty, &Wanted::default(), &[], true),
            None
        );
    }

    #[test]
    fn the_process_that_wrote_a_log_keeps_it_when_a_later_tab_opens() {
        // The other half of the rival rule, and the one it is easy to get backwards.
        //
        //   we start at T and write at T+1s; a second tab starts at T+3s
        //
        // That rival did not exist when the log was written, so it cannot be the writer
        // and must not outrank us. An earlier revision compared rivals with the same
        // `+ FIRST_EVENT_LEAD_MS` slack used for our own claim; at LEAD = 5s that made a
        // rival starting 2s AFTER the write outrank the process that made it, and the
        // writing tab's panel went blank whenever a second tab opened within ~6s.
        const T: u64 = 1_785_585_600_000; // 2026-08-01T12:00:00Z
        let dir = temp_dir("writerkeeps");
        std::fs::write(
            dir.join("mine.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T12:00:01.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let want = Wanted {
            session_id: None,
            since_ms: Some(T),
        };

        for rival_at in [T + 3_000, T + 1_001, T + 60_000] {
            let r = resolve_owned_session(&dir, &want, &[rival(rival_at)], true)
                .unwrap_or_else(|| panic!("rival at +{}ms must not take our log", rival_at - T));
            assert_eq!(r.path.file_name().unwrap(), "mine.jsonl");
        }

        // A rival that started at or before the write DOES outrank us — the boundary,
        // straddled so neither side can drift.
        assert!(
            resolve_owned_session(&dir, &want, &[rival(T + 1_000)], true).is_none(),
            "a rival that existed exactly at the write is a plausible writer"
        );
        assert!(
            resolve_owned_session(&dir, &want, &[rival(T + 999)], true).is_none(),
            "and so is one that started just before it"
        );
    }

    #[test]
    fn a_sibling_tab_that_started_later_owns_the_log_it_wrote() {
        // The MIRROR of the field report, and the case that sent this feature round a
        // third time. The reported defect was tab 2 (fresh) showing tab 1's session; the
        // mirror is tab 1 — idle, having written nothing — showing tab 2's.
        //
        //   tab A: claude starts 11:38:25, user never types, no log
        //   tab B: claude starts 11:38:41, writes at 11:38:42
        //
        // B's log is at or after A's start, so distance alone makes it a candidate for A;
        // being the only one, uniqueness would hand it over. What refuses it is the rival
        // test: B's process started after A's and before that log was written, so B is
        // the writer. A owns nothing and must be told so.
        const A_START: u64 = 1_785_584_305_000; // 11:38:25Z
        const B_START: u64 = 1_785_584_321_000; // 11:38:41Z
        let dir = temp_dir("mirrorsibling");
        std::fs::write(
            dir.join("tabB.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T11:38:42.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        // Tab A, told about tab B: nothing is its own.
        assert_eq!(
            resolve_owned_session(
                &dir,
                &Wanted {
                    session_id: None,
                    since_ms: Some(A_START),
                },
                &[rival(B_START)],
                true,
            ),
            None,
            "the later process wrote that log; tab A owns nothing"
        );

        // Tab B, told about tab A: the log is B's — a rival that started EARLIER never
        // outranks, or no session could ever be claimed.
        let r = resolve_owned_session(
            &dir,
            &Wanted {
                session_id: None,
                since_ms: Some(B_START),
            },
            &[rival(A_START)],
            true,
        )
        .expect("tab B wrote it");
        assert_eq!(r.path.file_name().unwrap(), "tabB.jsonl");
        assert!(r.owned);

        // Without the rival list — the degraded case — A wrongly adopts B's log. Pinned
        // so the value of passing rivals is visible rather than assumed.
        assert!(
            resolve_owned_session(
                &dir,
                &Wanted {
                    session_id: None,
                    since_ms: Some(A_START),
                },
                &[],
                true,
            )
            .is_some(),
            "documents what knowing nothing about rivals costs"
        );
    }

    #[test]
    fn a_log_that_began_hours_after_the_process_is_not_this_tab_s() {
        // The reported scenario. The tab's own `claude` started at 10:04 and never
        // wrote a transcript; the only log in the project began at 15:30, five and a
        // half hours later, from another application. The old one-sided bound
        // (`first + SLACK >= since`) admitted it — anything at or after
        // `since - 120s` qualified — and `min_by_key` then reported it as confirmed.
        let dir = temp_dir("hourslater");
        std::fs::write(
            dir.join("foreign.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T15:30:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        // Alone and unrivalled it IS adopted — the deliberate trade for a tab left idle
        // at the splash, whose own first event can be arbitrarily late.
        const SINCE: u64 = 1_785_233_040_000; // 2026-07-28T10:04:00Z
        let want = Wanted {
            session_id: None,
            since_ms: Some(SINCE),
        };
        assert!(
            resolve_owned_session(&dir, &want, &[], true).is_some(),
            "sole unrivalled candidate since we started"
        );

        // The moment anything else can account for it, it is refused: a rival process
        // that started in between...
        assert_eq!(
            resolve_owned_session(&dir, &want, &[rival(SINCE + 60_000)], true),
            None,
            "a later process wrote it"
        );

        // ...or a second, promptly-written session, which uniqueness no longer covers and
        // which beats it on distance.
        let rival = temp_dir("hourslaterrival");
        std::fs::write(
            rival.join("late.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T15:30:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::fs::write(
            rival.join("prompt.jsonl"), // SINCE + 30s
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T10:04:30.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let r = resolve_owned_session(&rival, &want, &[], true).expect("the prompt one confirms");
        assert_eq!(r.path.file_name().unwrap(), "prompt.jsonl");
    }

    #[test]
    fn the_originally_reported_foreign_session_is_still_refused() {
        // Regression guard on the defect that started this feature (`HANDOFF.md`),
        // re-checked against uniqueness — "sole candidate wins at any distance" is
        // exactly the shape that could let it back in.
        //
        // It cannot: the reported foreign session was `up 18h55m`, so its FIRST event
        // long predates the tab's `claude`. It never enters the candidate set, and
        // uniqueness only ever selects from that set.
        let dir = temp_dir("handoffregression");
        std::fs::write(
            dir.join("foreign.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-27T15:09:00.000Z\",\"message\":{}}\n\
             {\"type\":\"assistant\",\"timestamp\":\"2026-07-28T10:05:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        assert_eq!(
            resolve_owned_session(
                &dir,
                &Wanted {
                    session_id: None,
                    since_ms: Some(1_785_233_088_000), // 2026-07-28T10:04:48Z
                },
                &[],
                true,
            ),
            None,
            "a session older than this process is not a candidate, sole or otherwise"
        );
    }

    #[test]
    fn the_upper_bound_stops_at_120s_after_the_process() {
        // The window is ASYMMETRIC — this pins the generous side, where the number is a
        // real tolerance for how long a session may take to write its first event. The
        // tight side is `a_log_that_predates_the_process_is_not_that_process_s_session`,
        // where the number is a clock-skew allowance on an impossibility. Sizing them
        // the same is what shipped the two-tabs-one-dashboard defect.
        const S: u64 = 1_785_240_000_000; // 2026-07-28T12:00:00Z
        let since = Wanted {
            session_id: None,
            since_ms: Some(S),
        };

        // Both present: the one inside the window wins, and the outsider does not
        // merely lose the nearest-match comparison — see the sole-candidate dir below.
        let dir = temp_dir("upperbound");
        std::fs::write(
            dir.join("inside.jsonl"), // S + 119_000
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T12:01:59.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("outside.jsonl"), // S + 121_000
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T12:02:01.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let r =
            resolve_owned_session(&dir, &since, &[], true).expect("the inside candidate confirms");
        assert_eq!(r.path.file_name().unwrap(), "inside.jsonl");
        assert!(r.owned);

        // Alone in its own directory, and with no rival process to outrank us, the
        // outsider is ACCEPTED — by uniqueness, not by this bound. The slack separates
        // rivals; it is not a gate a sole candidate must pass. Pinned here so the two
        // rules are not confused: an earlier revision asserted `None` on exactly this
        // input, and that is what left a tab idle at the splash showing an empty panel
        // for the life of its process.
        let dir2 = temp_dir("upperboundalone");
        std::fs::write(
            dir2.join("outside.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T12:02:01.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let sole = resolve_owned_session(&dir2, &since, &[], true)
            .expect("sole candidate, no rival claims it");
        assert_eq!(sole.path.file_name().unwrap(), "outside.jsonl");

        // ...but a rival process that started between us and that log owns it instead.
        assert_eq!(
            resolve_owned_session(&dir2, &since, &[rival(S + 60_000)], true),
            None,
            "a process that started closer to the write has the better claim"
        );
    }

    #[test]
    fn a_log_that_predates_the_process_is_not_that_process_s_session() {
        // The invariant, stated as a test: a session cannot write an event before the
        // process that creates it exists.
        //
        // This REPLACES `a_session_that_began_just_before_the_process_start_still_qualifies`,
        // which asserted the opposite and justified itself with "the process start is
        // observed after the fact, so a first event legitimately predates it". That
        // reasoning was wrong: `hint_for` reads the true start from `PROC_PIDTBSDINFO`,
        // not the time of the walk. The false premise is what licensed the symmetric
        // window that shipped the two-tabs-one-dashboard defect.
        let since = Wanted {
            session_id: None,
            since_ms: Some(1_785_240_000_000), // 2026-07-28T12:00:00Z
        };

        // 60s before the process: impossible, and no longer admitted.
        let dir = temp_dir("predates");
        std::fs::write(
            dir.join("earlier.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T11:59:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        assert_eq!(
            resolve_owned_session(&dir, &since, &[], true),
            None,
            "a log whose first event predates the process is a different session"
        );

        // Inside the skew allowance it still confirms: only measurement error should
        // ever produce a negative delta, and the rule must not punish it.
        let dir2 = temp_dir("predatesskew");
        std::fs::write(
            dir2.join("skewed.jsonl"), // S - 2_000, inside FIRST_EVENT_LEAD_MS
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T11:59:58.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        let r = resolve_owned_session(&dir2, &since, &[], true).expect("clock skew is tolerated");
        assert_eq!(r.path.file_name().unwrap(), "skewed.jsonl");
        assert!(r.owned);
    }

    #[test]
    fn the_skew_allowance_is_pinned_at_its_stated_value() {
        // Without this the MAGNITUDE of `FIRST_EVENT_LEAD_MS` is untested — the sign is
        // an invariant, but 5s versus 10s versus 60s is a judgement, and a judgement no
        // test pins is one a later edit can move silently. Straddle it exactly.
        let since = Wanted {
            session_id: None,
            since_ms: Some(1_785_240_000_000), // 2026-07-28T12:00:00Z
        };

        // S − 5_000: the last accepted value.
        let inside = temp_dir("skewinside");
        std::fs::write(
            inside.join("edge.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T11:59:55.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        assert!(
            resolve_owned_session(&inside, &since, &[], true).is_some(),
            "exactly FIRST_EVENT_LEAD_MS early still confirms"
        );

        // S − 5_001: the first rejected value.
        let outside = temp_dir("skewoutside");
        std::fs::write(
            outside.join("edge.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T11:59:54.999Z\",\"message\":{}}\n",
        )
        .unwrap();
        assert_eq!(
            resolve_owned_session(&outside, &since, &[], true),
            None,
            "one millisecond past the allowance is a different session"
        );
    }

    #[test]
    fn among_qualifying_logs_the_nearest_to_the_process_start_wins() {
        // R2.4's nearest-match rule, pinned directly.
        //
        // It used to be covered incidentally by tests that straddled the old symmetric
        // window, but tightening the lower bound turned those into filter tests: their
        // rivals are now rejected outright rather than out-ranked, so inverting
        // `min_by_key` to `max_by_key` left the entire workspace green. Both candidates
        // here sit AFTER the process start and inside the upper bound, so only the
        // ranking can separate them.
        let dir = temp_dir("nearestwins");
        std::fs::write(
            dir.join("near.jsonl"), // S + 10s
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T12:00:10.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("far.jsonl"), // S + 90s — still qualifying, merely further
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T12:01:30.000Z\",\"message\":{}}\n",
        )
        .unwrap();

        let r = resolve_owned_session(
            &dir,
            &Wanted {
                session_id: None,
                since_ms: Some(1_785_240_000_000), // 2026-07-28T12:00:00Z
            },
            &[],
            true,
        )
        .expect("both qualify; one must win");
        assert_eq!(
            r.path.file_name().unwrap(),
            "near.jsonl",
            "nearest to the process start, not merely the first qualifier"
        );
    }

    #[test]
    fn two_tabs_started_sixteen_seconds_apart_do_not_share_a_dashboard() {
        // The reported defect, reproduced from the measured timeline (2026-08-01).
        //
        //   tab 1: claude pid 5550 starts 11:38:25, writes c2abd08b at 11:38:26 (+1s)
        //   tab 2: claude pid 5771 starts 11:38:41, writes NOTHING
        //
        // The symmetric window scored |11:38:26 − 11:38:41| = 15s <= 120s and handed tab
        // 1's live session to tab 2 with `owned: true`; being non-empty, D4 attached it
        // immediately and permanently. Both tabs rendered one dashboard — the exact
        // failure this whole feature exists to prevent, through a door left open by the
        // window's shape. Two tabs opened sixteen seconds apart is the ordinary workflow,
        // not an edge case.
        let dir = temp_dir("twotabs16s");
        // 2026-08-01T11:38:26Z
        std::fs::write(
            dir.join("c2abd08b.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T11:38:26.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        const TAB1_START: u64 = 1_785_584_305_000; // 11:38:25Z
        const TAB2_START: u64 = 1_785_584_321_000; // 11:38:41Z, 16s later

        // Tab 1 owns it: its process precedes the first event by a second.
        let r = resolve_owned_session(
            &dir,
            &Wanted {
                session_id: None,
                since_ms: Some(TAB1_START),
            },
            &[],
            true,
        )
        .expect("tab 1's own session");
        assert_eq!(r.path.file_name().unwrap(), "c2abd08b.jsonl");

        // Tab 2 owns nothing. It must be told so, not handed tab 1's session.
        assert_eq!(
            resolve_owned_session(
                &dir,
                &Wanted {
                    session_id: None,
                    since_ms: Some(TAB2_START),
                },
                &[],
                true,
            ),
            None,
            "tab 2's claude did not exist when this log was written"
        );
    }

    #[test]
    fn resolution_ranks_by_last_event_not_by_mtime() {
        // The measured case. In a real project directory `539f86bf` had the newest
        // mtime (12:48) but a 15-minute-stale newest event (12:33), while the session
        // actually being worked in, `c422a74e`, was SECOND by mtime with the newest
        // event (12:44). Ranking by mtime picks the wrong one — and then the app's
        // freshness rule disowns it and blanks a live panel.
        use std::time::Duration;

        let dir = temp_dir("rank");
        let live = dir.join("live.jsonl");
        let touched = dir.join("touched.jsonl");
        std::fs::write(
            &live,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:44:29.553Z\",\"message\":{\"id\":\"a\"}}\n",
        )
        .unwrap();
        // Written LATER (so: newer mtime) but carrying an OLDER event — exactly what a
        // timestamp-less write does to a log.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            &touched,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:33:02.860Z\",\"message\":{\"id\":\"b\"}}\n",
        )
        .unwrap();

        assert_eq!(
            newest_jsonl(&dir).as_deref(),
            Some(touched.as_path()),
            "mtime ranking picks the merely-touched log (the defect)"
        );
        let (picked, last) = most_recent_session(&dir).expect("resolves");
        assert_eq!(picked, live, "event ranking picks the live session");
        assert_eq!(last, Some(1_785_267_869_553), "and reports its last event");
    }

    #[test]
    fn resolution_reads_a_bounded_tail_of_a_bounded_candidate_set() {
        // Cost has to be bounded twice over: this runs on the watch thread on every
        // event until a target latches.
        let dir = temp_dir("bounded");
        // 40 logs, each far larger than the tail cap.
        let filler = "{\"type\":\"system\",\"pad\":\"".to_string() + &"x".repeat(4096) + "\"}\n";
        for i in 0..40 {
            let mut body = filler.repeat(8); // ~32 KiB, 4x TAIL_CAP
            body.push_str(&format!(
                "{{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T{:02}:00:00.000Z\",\"message\":{{}}}}\n",
                i % 24
            ));
            std::fs::write(dir.join(format!("s{i:02}.jsonl")), body).unwrap();
        }
        const { assert!(RESOLVE_CANDIDATES <= 16) };
        const { assert!(activity::TAIL_CAP <= 8 * 1024) };
        // Resolves without reading the ~1.3 MB of log bodies: only the last 8 KiB of
        // at most 16 files, i.e. <= 128 KiB.
        let (picked, last) = most_recent_session(&dir).expect("resolves");
        assert!(picked.starts_with(&dir));
        assert!(last.is_some(), "the timestamp sits inside the tail window");
    }

    #[test]
    fn a_log_with_no_event_in_its_tail_loses_to_one_that_has_one() {
        let dir = temp_dir("undatedrank");
        std::fs::write(dir.join("undated.jsonl"), "{\"type\":\"system\"}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            dir.join("dated.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:44:29.553Z\",\"message\":{}}\n",
        )
        .unwrap();
        let (picked, _) = most_recent_session(&dir).expect("resolves");
        assert_eq!(picked.file_name().unwrap(), "dated.jsonl");

        // ... and with nothing dated at all, the newest-touched wins rather than
        // nothing being resolved.
        let dir2 = temp_dir("allundated");
        std::fs::write(dir2.join("a.jsonl"), "{\"type\":\"system\"}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir2.join("b.jsonl"), "{\"type\":\"system\"}\n").unwrap();
        let (picked, last) = most_recent_session(&dir2).expect("resolves");
        assert_eq!(picked.file_name().unwrap(), "b.jsonl");
        assert_eq!(last, None);
    }

    #[test]
    fn a_newer_log_appearing_takes_over_the_watch() {
        // The reported defect: a brand-new session had written NO log when the
        // overlay opened, so the newest file in the directory was an unrelated
        // session and tailing it could never find the real one. The watch is on the
        // directory, so the new file's first line switches the target.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("reresolve");
        let old = dir.join("old.jsonl");
        std::fs::write(
            &old,
            "{\"type\":\"assistant\",\"requestId\":\"o\",\"message\":{\"id\":\"o\",\"usage\":{\"output_tokens\":700}}}\n",
        )
        .unwrap();
        let (t, off) = backfill_telemetry(&old);
        assert_eq!(
            t.snapshot().total_output,
            700,
            "attached to the old session"
        );
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(old.clone()),
                // `claude --resume fresh` — the tab can name its session outright, so
                // tier 1 confirms `fresh.jsonl` the instant it exists. The hint is not
                // decoration: re-resolution is strict, and these fixture lines carry no
                // `timestamp`, so tier 2 could never date them to a process start. Note
                // the id names a file that does NOT exist yet, which is the case under
                // test — tier 1 answers `None` until the tab's session writes.
                want: Wanted {
                    session_id: Some("fresh".into()),
                    since_ms: None,
                },
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        // The tab's own session finally writes its first line, into a NEW file.
        let fresh = dir.join("fresh.jsonl");
        let mut f = std::fs::File::create(&fresh).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","requestId":"n","message":{{"id":"n","usage":{{"output_tokens":3}}}}}}"#
        )
        .unwrap();
        drop(f);

        let snap =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("switched");
        assert_eq!(
            snap.snap.total_output, 3,
            "the NEW file's totals, with no residue of the old session"
        );
    }

    #[test]
    fn a_missing_project_directory_still_arms_the_watch() {
        // The first Claude session ever run in a cwd: the project directory does not
        // exist, so watching it directly errors and the overlay would never arm at
        // all — with nothing to retry it. The watch falls back to the nearest
        // existing ancestor, and the log is picked up when it finally appears.
        use std::io::Write;
        use std::time::Duration;

        let root = temp_dir("nodir"); // stands in for ~/.claude/projects
        let missing = root.join("-Users-x-brand-new");
        assert!(!missing.exists());

        let w = LogWatcher::spawn(
            Watch {
                dir: missing.clone(),
                log: None,
                // The tab names its session, so tier 1 confirms `first.jsonl` once the
                // directory and the file both exist. Required, not incidental: the
                // fixture line below carries no `timestamp`, so `first_event` returns
                // `None` and tier 2 can never confirm it.
                want: Wanted {
                    session_id: Some("first".into()),
                    since_ms: None,
                },
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            SessionTelemetry::new(),
            0,
            || {},
        )
        .expect("arms on an ancestor rather than giving up");

        std::fs::create_dir_all(&missing).unwrap();
        let mut f = std::fs::File::create(missing.join("first.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","requestId":"n","message":{{"id":"n","usage":{{"output_tokens":42}}}}}}"#
        )
        .unwrap();
        drop(f);

        let snap =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("attached");
        assert_eq!(snap.snap.total_output, 42);
    }

    #[test]
    fn an_unowned_log_appended_before_attach_sends_no_update() {
        // The watcher half of the defect (the overlay half is `open()`'s resolution).
        // A tab whose hint confirms nothing sits in a project directory another
        // application is actively writing in. Before this, re-resolution ranked the
        // directory, adopted the foreign log on its first append and handed it up as
        // an `Update` — the same defect one frame later, differing only in which side
        // of the channel it arrived on.
        //
        // The assertion is that NOTHING is sent, not that what is sent gets refused:
        // an `Update` the overlay declines has still had the foreign file opened,
        // read and folded into a snapshot, and one caller that forgets to consult
        // `owned` puts it on screen. Strictness belongs where the file is chosen.
        //
        // Mechanically: `want` confirms nothing, so re-resolution answers `None` on
        // every callback; the watch began with no target at all, so it never acquires
        // one and the callback returns before it reads a byte.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("unownedpreattach");
        let foreign = dir.join("foreign.jsonl");
        std::fs::write(
            &foreign,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-23T17:06:00.000Z\",\"requestId\":\"f\",\"message\":{\"id\":\"f\",\"usage\":{\"output_tokens\":900}}}\n",
        )
        .unwrap();

        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: None,
                want: Wanted::default(),
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            SessionTelemetry::new(),
            0,
            || {},
        )
        .expect("spawn");

        // The foreign session keeps working while our panel is open and unattached —
        // precisely the moment the ranked fallback used to adopt it.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&foreign)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","timestamp":"2026-07-28T20:44:00.000Z","requestId":"g","message":{{"id":"g","usage":{{"output_tokens":11}}}}}}"#
            )
            .unwrap();
        }

        assert!(
            w.rx.recv_timeout(Duration::from_secs(1)).is_err(),
            "no Update at all — the overlay is never offered the foreign log's telemetry"
        );
    }

    #[test]
    fn no_projects_root_means_no_watch_rather_than_a_home_wide_one() {
        // The fallback climbs exactly one level. If ~/.claude/projects itself is
        // absent, no session has ever run anywhere, and arming a recursive watch on
        // $HOME to wait for the first one would wake this callback on every write in
        // the user's home directory for as long as the panel stayed open.
        let root = temp_dir("noroot");
        let nowhere = root.join("gone").join("-Users-x-proj");
        assert!(
            LogWatcher::spawn(
                Watch {
                    dir: nowhere,
                    log: None,
                    want: Wanted::default(),
                    owned: false,
                    rivals: no_rivals(),
                    attached: unattached()
                },
                SessionTelemetry::new(),
                0,
                || {}
            )
            .is_none(),
            "declines to arm rather than watching an ancestor outside the projects root"
        );
    }

    #[test]
    fn a_frozen_watcher_refuses_a_log_its_hint_would_otherwise_confirm() {
        // The freeze, pinned INDEPENDENTLY of strict resolution.
        //
        // Every other freeze test spawns with a `want` that confirms nothing, so once
        // resolution went strict they all passed for the wrong reason: the rival was
        // refused by `resolve_owned_session`, not by the flag. Measured, not guessed —
        // replacing the guard with `if true || !attached.load(..)` left the whole suite
        // green, which means the flag could have been deleted outright and CI would not
        // have noticed. It is this crate's only coordination primitive with `app`, so
        // that is not an acceptable state to leave it in.
        //
        // The separation here is the hint: it names `other`, which tier 1 WOULD confirm.
        // So resolution positively wants to switch, and the only thing stopping it is
        // the flag. Remove the guard and this test fails — the accumulator resets to the
        // rival's totals.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("frozenagainstconfirmable");
        let mine = dir.join("mine.jsonl");
        std::fs::write(&mine, "").unwrap();
        let (t, off) = backfill_telemetry(&mine);
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(mine.clone()),
                // Names the RIVAL, not ours — the whole point.
                want: Wanted {
                    session_id: Some("other".into()),
                    since_ms: None,
                },
                owned: true,
                rivals: no_rivals(),
                attached: Arc::new(AtomicBool::new(true)), // the caller already attached
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        let append = |path: &std::path::Path, id: &str, out: u64| {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","usage":{{"output_tokens":{out}}}}}}}"#
            )
            .unwrap();
        };

        append(&mine, "a", 10);
        assert_eq!(
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("our own append")
                .snap
                .total_output,
            10
        );

        // The log the hint NAMES appears. An unfrozen watcher confirms it by id,
        // switches, and resets the accumulator to 999.
        append(&dir.join("other.jsonl"), "z", 999);
        append(&mine, "b", 5);
        let mut last = None;
        while let Ok(s) = w.rx.recv_timeout(Duration::from_secs(3)) {
            last = Some(s);
        }
        assert_eq!(
            last.expect("still ours").snap.total_output,
            15,
            "cumulative on OUR log — the flag held against a log resolution would have confirmed"
        );
    }

    #[test]
    fn an_attached_caller_freezes_the_target() {
        // Once the caller says it has attached, another session writing in the same
        // project must not steal the watch — that would reset the accumulator and
        // replace data already on screen, which is exactly what the panel must never
        // do. The watcher holds no copy of the ownership rule; it reads this flag.
        //
        // NOTE: with strict resolution this one also passes degenerately (its empty
        // `want` confirms nothing, so the rival is refused twice over). The freeze is
        // pinned independently by
        // `a_frozen_watcher_refuses_a_log_its_hint_would_otherwise_confirm` above.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("frozen");
        let mine = dir.join("mine.jsonl");
        std::fs::write(&mine, "").unwrap();
        let (t, off) = backfill_telemetry(&mine);
        let attached = Arc::new(AtomicBool::new(true)); // the caller already attached
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(mine.clone()),
                want: Wanted::default(),
                owned: false,
                rivals: no_rivals(),
                attached,
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        let append = |path: &std::path::Path, id: &str, out: u64, ts: &str| {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","timestamp":"{ts}","requestId":"{id}","message":{{"id":"{id}","usage":{{"output_tokens":{out}}}}}}}"#
            )
            .unwrap();
        };

        append(&mine, "a", 10, "2026-07-28T19:44:29.553Z");
        assert_eq!(
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("our own append")
                .snap
                .total_output,
            10
        );

        // A rival session writes a STRICTLY newer event — so an unfrozen watcher
        // would certainly switch to it, and this test would certainly notice.
        append(
            &dir.join("other.jsonl"),
            "z",
            999,
            "2026-07-28T23:59:59.999Z",
        );
        append(&mine, "b", 5, "2026-07-28T19:44:30.000Z");
        let mut last = None;
        while let Ok(s) = w.rx.recv_timeout(Duration::from_secs(3)) {
            last = Some(s);
        }
        assert_eq!(
            last.expect("still ours").snap.total_output,
            15,
            "cumulative on OUR log — never reset, never the rival's 999"
        );
    }

    #[test]
    fn attaching_mid_flight_stops_the_re_resolution() {
        // The caller attaches while the watcher is running (it opened the panel before
        // its session had written). From that instant the target is frozen.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("midflight");
        let a = dir.join("a.jsonl");
        std::fs::write(
            &a,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:00:00.000Z\",\"requestId\":\"a\",\"message\":{\"id\":\"a\",\"usage\":{\"output_tokens\":1}}}\n",
        )
        .unwrap();
        let (t, off) = backfill_telemetry(&a);
        let attached = Arc::new(AtomicBool::new(false));
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(a.clone()),
                // The hint names `b`, so tier 1 confirms it for the first phase —
                // without it strict re-resolution answers `None` and the take-over
                // below could never happen.
                //
                // READ THIS BEFORE TRUSTING THE SECOND PHASE: `c` is then refused for
                // TWO independent reasons — the frozen flag, which is what this test
                // was written for, and the fact that the hint does not confirm `c`
                // either. Removing the `attached` guard alone would not make this
                // test fail. Stated rather than hidden: one `want` can name only one
                // id, so a fixture that keeps the first phase working cannot also make
                // `c` confirmable, and the freeze is no longer separable here. What the
                // test still pins on its own is the first phase and the sequencing:
                // a confirmed newcomer is adopted while the caller is free, and the
                // totals never regress afterwards.
                want: Wanted {
                    session_id: Some("b".into()),
                    since_ms: None,
                },
                owned: false,
                rivals: no_rivals(),
                attached: attached.clone(),
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        // Unattached: a newer-evented file takes over.
        let b = dir.join("b.jsonl");
        let mut f = std::fs::File::create(&b).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-07-28T19:30:00.000Z","requestId":"b","message":{{"id":"b","usage":{{"output_tokens":7}}}}}}"#
        )
        .unwrap();
        drop(f);
        let mut seen = None;
        while let Ok(s) = w.rx.recv_timeout(Duration::from_secs(3)) {
            seen = Some(s);
        }
        assert_eq!(
            seen.expect("switched").snap.total_output,
            7,
            "took over while free"
        );

        // The caller now attaches; a third, newer-evented file must NOT take over.
        attached.store(true, Ordering::Relaxed);
        let c = dir.join("c.jsonl");
        let mut f = std::fs::File::create(&c).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-07-28T19:59:00.000Z","requestId":"c","message":{{"id":"c","usage":{{"output_tokens":500}}}}}}"#
        )
        .unwrap();
        drop(f);
        let mut after = None;
        while let Ok(s) = w.rx.recv_timeout(Duration::from_secs(3)) {
            after = Some(s);
        }
        if let Some(s) = after {
            assert_eq!(s.snap.total_output, 7, "frozen on b — c never adopted");
        }
    }

    #[test]
    fn a_dated_incumbent_outranks_an_undated_newcomer() {
        // Consequence of ranking by event rather than mtime, pinned deliberately: a
        // new session whose first lines carry no timestamp does not take over until it
        // writes a dated one. Real logs date their assistant/user lines, so the delay
        // is one line long — but it is a behaviour change worth stating.
        let dir = temp_dir("incumbent");
        std::fs::write(
            dir.join("dated.jsonl"),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-28T19:00:00.000Z\",\"message\":{}}\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("undated.jsonl"), "{\"type\":\"system\"}\n").unwrap();
        let (picked, _) = most_recent_session(&dir).expect("resolves");
        assert_eq!(picked.file_name().unwrap(), "dated.jsonl");
    }

    #[test]
    fn rotation_resets_the_accumulator_and_emits() {
        // A rotation truncates to zero, so no complete line follows. Under a
        // "\u{2265}1 line folded" send condition nothing would be emitted and the UI would
        // keep showing the PREVIOUS session's totals as though live.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("rotate");
        let log = dir.join("s.jsonl");
        std::fs::write(&log, "").unwrap();
        let (t, off) = backfill_telemetry(&log);
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(log.clone()),
                want: Wanted::default(),
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","requestId":"a","message":{{"id":"a","usage":{{"output_tokens":99}}}}}}"#
            )
            .unwrap();
        }
        let before =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("pre-rotation");
        assert_eq!(before.snap.total_output, 99);

        // Truncate: a new session in the same path.
        std::fs::write(&log, "").unwrap();
        let after =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("a reset must emit even with no line to fold");
        assert_eq!(
            after.snap.total_output, 0,
            "previous session's totals dropped"
        );

        // ...and the fresh accumulator folds the NEW session from zero.
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","requestId":"b","message":{{"id":"b","usage":{{"output_tokens":4}}}}}}"#
            )
            .unwrap();
        }
        let fresh =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("new session line");
        assert_eq!(fresh.snap.total_output, 4, "only the new session's tokens");
    }

    #[test]
    fn a_partial_line_folds_nothing_and_sends_nothing() {
        // R3.3 — bytes with no terminating newline are buffered, not folded. Sending
        // a snapshot here would publish a state that did not change.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("partial");
        let log = dir.join("s.jsonl");
        std::fs::write(&log, "").unwrap();
        let (t, off) = backfill_telemetry(&log);
        let w = LogWatcher::spawn(
            Watch {
                dir: dir.clone(),
                log: Some(log.clone()),
                want: Wanted::default(),
                owned: false,
                rivals: no_rivals(),
                attached: unattached(),
            },
            t,
            off,
            || {},
        )
        .expect("spawn");

        let write = |bytes: &str| {
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            f.write_all(bytes.as_bytes()).unwrap();
        };

        // no trailing newline -> incomplete record
        write(
            r#"{"type":"assistant","requestId":"a","message":{"id":"a","usage":{"output_tokens":7}}}"#,
        );
        assert!(
            w.rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "a buffered partial must not emit"
        );

        // completing it flushes exactly one snapshot carrying the folded line
        write("\n");
        let s =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("flush on newline");
        assert_eq!(s.snap.total_output, 7);
    }

    #[test]
    fn backfill_telemetry_seeds_from_the_existing_file() {
        let dir = temp_dir("seed");
        let log = dir.join("s.jsonl");
        std::fs::write(
            &log,
            "{\"type\":\"assistant\",\"requestId\":\"a\",\"message\":{\"id\":\"a\",\"usage\":{\"output_tokens\":42}}}\n",
        )
        .unwrap();
        let (t, off) = backfill_telemetry(&log);
        assert_eq!(t.snapshot().total_output, 42);
        assert!(off > 0, "offset is the file length, to tail from");

        // Missing file: empty state, offset 0, no panic.
        let (t2, off2) = backfill_telemetry(Path::new("/definitely/not/here.jsonl"));
        assert_eq!(t2.snapshot().total_output, 0);
        assert_eq!(off2, 0);
    }

    #[test]
    fn parse_is_malformed_safe() {
        assert!(parse_line("not json at all").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("{ truncated").is_none());
        // Valid JSON always yields an event, even with unknown shape.
        let ev = parse_line(r#"{"type":"tool_use"}"#).unwrap();
        assert_eq!(ev.role, "tool_use");
        // Missing file backfills to empty, no panic.
        let (e, off) = backfill(Path::new("/definitely/not/here.jsonl"), 5);
        assert!(e.is_empty() && off == 0);
    }
}

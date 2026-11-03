//! Which Claude session is running in *this tab* — answered from the tab's own
//! process tree rather than from the log directory.
//!
//! `~/.claude/projects/<encoded-cwd>/` records no application, window or tab: Warp,
//! iTerm, VS Code and every other Crewterm window working in the same project all
//! append to the same directory. Measured case: a newly opened tab showed
//! `up 116h40m · idle 0s` from a `claude --resume` running under
//! `Warp.app/Contents/MacOS/stable`. No amount of ranking *within* that directory can
//! separate those sessions, because the discriminating fact is not in it.
//!
//! The process tree has it. The app spawns a shell per tab and knows its pid
//! (`Pty::child_pid`), the `claude` process is one of its descendants, and that
//! process either names its session outright (`--resume <id>`) or dates it (its own
//! start time, which the session's first log event follows within seconds).
//!
//! Read-only observation throughout — a process listing and `proc_pidinfo`, the same
//! interface `session::cwd_of_pid` already uses. No signals, no writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
// `SystemTime`/`UNIX_EPOCH` moved to the test module with `start_ms`'s body
// (tabmeta-session-keying D6) — only the tests still need a wall clock here.
use std::time::Duration;

/// The chosen `claude`'s log directory, **and how much may be concluded from it**.
///
/// The permission travels in the same value as the directory on purpose. The two resolution
/// tiers fail very differently against a wrong directory: tier 1 looks the log up by exact
/// filename, so a wrong directory merely fails to find one, while tier 2's asymmetric
/// `since_ms` window plus rival assignment can return **another application's** log with
/// `owned: true` — which the overlay then latches. So "unproven directory, tier 2 enabled" is a
/// state that must not exist; carrying the provenance here makes it *unrepresentable* rather
/// than forbidden by a rule someone has to remember at each call site.
///
/// Every other field on [`SessionHint`] states an invariant of its own — `pid` documents that it
/// is not an identity alone, `rivals_known` that `false` is the safe value. A bare
/// `Option<PathBuf>` would be the odd one out, and it would also drop the one bit
/// [`resolve_hint_cwd`]'s disagreement row exists to produce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintCwd {
    /// The kernel and the process's own registry record agree, or only one of them was
    /// available at all. Both tiers may run against this directory.
    Proven(PathBuf),
    /// They disagree, so the assumption the whole design rests on — that `claude` writes to
    /// `encode_cwd` of the directory it runs in — may be violated. Usable for tier 1's
    /// exact-filename lookup, which cannot confirm a stranger; **never** for tier 2's timing
    /// window. The path is the *registry's*: `claude`'s own statement about where it is
    /// running wins the tie, because that is a question only `claude` can answer directly.
    Disputed(PathBuf),
}

impl HintCwd {
    /// The directory itself. Both variants name one — they differ in what may be done with it.
    pub fn path(&self) -> &Path {
        match self {
            HintCwd::Proven(p) | HintCwd::Disputed(p) => p,
        }
    }

    /// May tier 2 — `Wanted::since_ms`, the timing window plus rival assignment — run against
    /// this directory? Only against one the chosen `claude` is *provably* in.
    pub fn tier2_ok(&self) -> bool {
        matches!(self, HintCwd::Proven(_))
    }
}

/// What the tab's live `claude` process says about which session it is running.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionHint {
    /// The `claude` process this hint describes.
    ///
    /// Carried so a caller can ask that *same* process the same question again later
    /// without re-running [`hint_for`] — which forks `/bin/ps`. `/clear` rotates the
    /// session inside one process, so re-reading `~/.claude/sessions/<pid>.json` for this
    /// pid is the whole rotation signal, and it costs one small file read rather than a
    /// process listing (`ai-monitor-session-rotation`).
    ///
    /// `None` only from [`Default`] — a real hint always names the process it came from.
    /// It is **not** an identity on its own: pids are recycled, so it must always be
    /// paired with `started_ms` and put through [`ai_monitor::validated_session_id`].
    pub pid: Option<u32>,
    /// Named exactly, from `--resume <id>`. When present nothing else is needed.
    pub session_id: Option<String>,
    /// When the process started, epoch millis. A session started by this process has
    /// its first event just after this — the fallback when the id is not on the
    /// command line, and *only* a fallback: it dates a session rather than naming it.
    pub started_ms: Option<u64>,
    /// The directory the chosen `claude` is running in — **its own**, never the tab shell's
    /// and never the pane shell's.
    ///
    /// The hint and the directory it is resolved against have to describe the same process.
    /// While they were derived separately the two could disagree — a tmux pane working outside
    /// its tab shell's directory, or a pane that `cd`s after launching `claude` — and the
    /// resolver would then hunt for this process's log somewhere it never writes. A cwd *gate*
    /// used to reject those cases wholesale, which cost every cross-directory tmux tab its
    /// panel entirely. Carrying the directory on the hint removes the second candidate rather
    /// than comparing it, and costs no extra syscall: it is the value `hint_for` already reads
    /// for rival matching.
    ///
    /// `None` is not merely missing information. The caller must then suppress tier-2
    /// (`since_ms`) resolution, because tier 2 against a directory this process is not provably
    /// in can confirm **another application's** log with `owned: true` and latch it. Tier 1 is
    /// unaffected: it looks the log up by exact filename, so a wrong directory merely fails to
    /// find one. See [`HintCwd`] for the other two states.
    pub cwd: Option<HintCwd>,
    /// Start times of the OTHER live `claude` processes working in the same directory —
    /// sibling tabs of this app, and other applications' sessions alike.
    ///
    /// `started_ms` alone dates a session but cannot *assign* one: two tabs opened
    /// seconds apart in one project each look like a plausible owner of the other's log.
    /// These turn the question into an assignment — a log belongs to the latest process
    /// that started at or before its first event — which is the only thing that separates
    /// two tabs whose own timings are indistinguishable. Empty when the process table is
    /// unreadable, which degrades to the timing rule alone.
    pub rival_starts: Vec<ai_monitor::Rival>,
    /// Whether `rival_starts` is a COMPLETE enumeration, as opposed to an empty or partial
    /// list left by a read that failed.
    ///
    /// Load-bearing since the uniqueness rescue was narrowed: an empty rival set is no
    /// longer a mere degradation, it is the switch that re-enables adopting a sole
    /// candidate at any distance. Granting that privilege on a failed read would confirm a
    /// foreign log with `owned: true` and latch it. `false` is the safe value, and it is
    /// what `Default` yields.
    pub rivals_known: bool,
}

impl SessionHint {
    /// Is there anything here to resolve with?
    pub fn is_empty(&self) -> bool {
        self.session_id.is_none() && self.started_ms.is_none()
    }
}

/// How deep to walk from the tab's shell. `zsh -> claude` is depth 1; a wrapper or two
/// (`zsh -> npm -> node -> claude`) is depth 3. Bounded so a pathological tree cannot
/// turn a panel toggle into a traversal of every process on the machine.
const MAX_DEPTH: u32 = 6;
/// How many processes the walk will look at in total, across all depths.
const MAX_VISITED: usize = 512;

/// The session hint for the tab whose shell is `shell_pid`, or `None` when the tab is
/// not running Claude Code at all.
///
/// Every failure — a dead pid, a permission refusal, an unreadable process table —
/// yields `None`, and the caller falls back to ranking the log directory. Being unable
/// to identify the tab's session must never be worse than not having looked.
/// Which agent this tab is running, and that process's pid.
///
/// Separate from [`hint_for`] rather than folded into it, because the two answer different
/// questions and only one of them generalises. `hint_for` builds Claude's *session identity*
/// — a `--resume` id, a start time, a rival set — machinery that exists because Claude's logs
/// record no owner. Antigravity's presence lock names its conversation outright, so none of
/// that applies; what the overlay needs from this side is only "which agent, and which pid",
/// and asking `hint_for` to carry a field meaningless to its own resolution would blur a
/// boundary that is currently exact.
///
/// The classification is `tabmeta::agent`'s, reused rather than reimplemented: it already
/// handles `agy`, the auto-updater's `agy.<id>.old` in-place rename, and the interpreter/argv
/// gate, each of which cost a spec to get right (`specs/antigravity-agy-detection`).
///
/// Resolves through the tmux pane root first, matching `hint_for`, so a tab whose agent runs
/// inside a pane is found rather than missed.
pub fn agent_for(shell_pid: u32) -> Option<(crate::tabmeta::AgentKind, u32)> {
    let root = resolved_root(shell_pid);
    // The pane root first, then the tab's own subtree. `resolved_root` returns `shell_pid`
    // unchanged for a non-tmux tab, so the second call is skipped in the common case rather
    // than paying for a duplicate walk.
    let found = crate::tabmeta::agent::agent_of_pid(root).or_else(|| {
        (root != shell_pid).then(|| crate::tabmeta::agent::agent_of_pid(shell_pid))?
    })?;
    Some((found.kind, found.pid))
}

pub fn hint_for(shell_pid: u32) -> Option<SessionHint> {
    let table = ProcTable::read()?;
    let root = resolved_root(shell_pid);
    hint_for_with(
        &table,
        root,
        shell_pid,
        cwd_of_pid,
        ai_monitor::registered_session,
        start_ms,
    )
}

/// [`hint_for`]'s body, with all three impure reads injected.
///
/// The split exists because the directory chain below is the most safety-critical logic in this
/// module and, unlike `choose_claude`, `registered_id_with`, `resolved_root_with` and
/// `rival_starts_checked`, it had no injection point at all: `hint_for` called `cwd_of_pid`,
/// `registered_session` and `start_ms` directly, so every row of that chain would have shipped
/// with no headless test. A machine with no tmux and no live `claude` can now drive all five.
fn hint_for_with(
    table: &ProcTable,
    root: u32,
    shell_pid: u32,
    cwd_of: impl Fn(u32) -> Option<PathBuf>,
    registered: impl Fn(u32) -> Option<ai_monitor::Registered>,
    start_of: impl Fn(u32) -> Option<u64>,
) -> Option<SessionHint> {
    let claude = choose_claude(root, shell_pid, |p| table.find_claude(p))?;
    let started = start_of(claude);
    // The whole record, not just its id: `.cwd` is the process's own statement about where it
    // is running, and the cross-check below is the only remaining detector of a violated A1.
    // Validated against the process start first, so a stale file left on a recycled pid names
    // neither a session nor a directory.
    let reg = validated_registration(&registered, claude, started);
    // The chosen `claude`'s OWN cwd — never the tab shell's and never the pane shell's (D1).
    let kernel = cwd_of(claude);
    let (cwd, mixed_basis) =
        resolve_hint_cwd(claude, kernel, reg.as_ref().map(|r| r.cwd.as_path()));

    // ORDER IS LOAD-BEARING, not incidental. The rival enumeration is fed the directory the
    // chain just resolved, not the raw kernel read. Passing `kernel` here instead would mean
    // that in exactly the state the registry fallback exists for — kernel unreadable, registry
    // present, tier 2 still allowed — `rival_starts_checked`'s first guard returns
    // `(vec![], false)`, so tier 2 would run in the right directory with **no rival able to
    // veto it**, leaving only the ±120s distance test. That is the "two tabs in one project,
    // `claude` seconds apart" defect that has already shipped once.
    let (rivals, enumerated) =
        table.rival_starts_checked(claude, cwd.as_ref().map(HintCwd::path), &cwd_of, &start_of);
    Some(SessionHint {
        pid: Some(claude),
        // Ask the process what session it is running, before trying to infer it.
        // `--resume` is only the argv spelling of the same question and stays as the
        // fallback for a `claude` old enough not to write the registry file.
        session_id: reg
            .map(|r| r.session_id)
            .or_else(|| table.resume_id(claude)),
        started_ms: started,
        cwd,
        rival_starts: rivals,
        // A MIXED-BASIS enumeration never earns the rescue, whatever it found. On the two rows
        // where our own directory came from the registry while every rival's still comes from
        // the kernel (`rival_starts_checked` reads `cwd_of` per rival), a rival whose cwd merely
        // *differs* takes a bare `continue` that leaves `known` true — so a systematic skew
        // between the two sources silently drops every rival and yields `(vec![], known: true)`.
        // `rivals_known`'s own doc calls that flag "the switch that re-enables adopting a sole
        // candidate at any distance", and granting it on a guess "would confirm a foreign log
        // with `owned: true` and latch it". Today an unreadable own cwd already withholds it via
        // guard (1); resolving the directory from elsewhere must not quietly *widen* that.
        rivals_known: enumerated && !mixed_basis,
    })
}

/// D7's five rows: which directory the panel opens on, and whether tier 2 may run against it.
///
/// | kernel `proc_pidinfo` | validated registry record | directory | tier 2 | result |
/// |---|---|---|---|---|
/// | present | absent    | kernel   | allowed    | `Proven`   |
/// | present | agrees    | kernel   | allowed    | `Proven`   |
/// | present | disagrees | registry | suppressed, rescue withheld | `Disputed` |
/// | absent  | present   | registry | allowed, rescue withheld  | `Proven` |
/// | absent  | absent    | —        | suppressed | `None`     |
///
/// The second return is whether the answer is **mixed-basis** — our directory from the registry
/// while rivals are still enumerated through the kernel. Rows 3 and 4 are; see the caller.
///
/// **Why the disagreement row suppresses rather than merely re-points.** Deleting the old cwd
/// gate removed the last comparison in the system, so a violation of the assumption everything
/// rests on — that `claude` writes to `encode_cwd` of the directory it runs in — would otherwise
/// be *undetectable*: tier 1 would silently find nothing and the panel would simply look empty.
/// Here the two independent sources **are** the check. When they disagree we are using a
/// directory chosen on the weaker of two contradicting claims, which is precisely the input tier
/// 2 must never be given; tier 1 still resolves it by exact filename, which is the tier that
/// cannot confirm a stranger. That makes the detector actionable rather than merely observed.
fn resolve_hint_cwd(
    pid: u32,
    kernel: Option<PathBuf>,
    registry: Option<&Path>,
) -> (Option<HintCwd>, bool) {
    match (kernel, registry) {
        // Rows 1 & 2 — one source, or two that agree. The kernel is the direct answer and the
        // registry has nothing to add.
        (Some(k), None) => {
            note_cwd_dispute(pid, None);
            (Some(HintCwd::Proven(k)), false)
        }
        (Some(k), Some(r)) if k == r => {
            note_cwd_dispute(pid, None);
            (Some(HintCwd::Proven(k)), false)
        }
        // Row 3 — they disagree. `claude`'s own record wins the tie and tier 2 is suppressed.
        (Some(k), Some(r)) => {
            note_cwd_dispute(pid, Some((&k, r)));
            (Some(HintCwd::Disputed(r.to_path_buf())), true)
        }
        // Row 4 — our read failed; the process's own record answers. Unlike row 3 there is no
        // evidence of a systematic skew, so the veto is worth having and tier 2 stays allowed —
        // but the enumeration is mixed-basis, so the rescue is forfeited.
        (None, Some(r)) => {
            note_cwd_dispute(pid, None);
            (Some(HintCwd::Proven(r.to_path_buf())), true)
        }
        // Row 5 — nothing to open on. `open()` falls back to the pane's cwd with tier 2 off.
        (None, None) => {
            note_cwd_dispute(pid, None);
            (None, false)
        }
    }
}

/// Pids currently in the disputed state.
///
/// Process-wide because `hint_for` has two callers on two threads — `Overlay::open` on the winit
/// event loop and `refresh_rivals` on a worker — and the rate discipline has to hold across both.
static DISPUTED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Log a cwd disagreement **once per transition into it**, `debug!` thereafter.
///
/// Rate discipline is part of the decision, not a detail. `hint_for` runs from `refresh_rivals`
/// per *append burst* while the panel is unarmed, so an unguarded `warn!` would emit continuously
/// for a stable violation and train the reader to ignore it. The in-repo rule
/// (`tabmeta/worker.rs`'s `note_activity_gap`) is that routine failures stay at `debug!` and only
/// transitions warn — and, as there, a pid that leaves the state is forgotten so a later
/// disagreement warns again.
fn note_cwd_dispute(pid: u32, dispute: Option<(&Path, &Path)>) {
    let Ok(mut seen) = DISPUTED.lock() else {
        return;
    };
    let transition = dispute_transition(&mut seen, pid, dispute.is_some());
    drop(seen);
    let Some((kernel, registry)) = dispute else {
        return;
    };
    // A1 is an assumption about a program this project does not control, so its violation must
    // be visible in a log rather than only in behaviour.
    if transition {
        tracing::warn!(
            pid,
            kernel = %kernel.display(),
            registry = %registry.display(),
            "claude's recorded cwd disagrees with the kernel's: opening on its own record and \
             suppressing tier-2 resolution"
        );
    } else {
        tracing::debug!(pid, registry = %registry.display(), "cwd still disputed");
    }
}

/// The transition rule, pure so it is testable without a logger: `true` only when `pid` is
/// *entering* the disputed state.
fn dispute_transition(seen: &mut Vec<u32>, pid: u32, disputed: bool) -> bool {
    match (disputed, seen.iter().position(|p| *p == pid)) {
        (true, None) => {
            seen.push(pid);
            true
        }
        (true, Some(_)) => false,
        // Forgotten, so a later disagreement for this pid warns rather than whispering.
        (false, Some(i)) => {
            seen.swap_remove(i);
            false
        }
        (false, None) => false,
    }
}

/// The `~/.claude/sessions/<pid>.json` record for `pid`, **validated against this process**.
///
/// The validation is not ceremony. `~/.claude/sessions/` holds a file per pid and they
/// outlive the process — the sample that motivated this had **5 files for 4 live
/// processes** — so on a recycled pid an unvalidated read names a stranger's session with
/// total confidence, which is precisely the failure this feature exists to remove. A
/// record may only be believed when it was written just *after* the process we are asking
/// about started.
///
/// Returns the whole record rather than the session id it used to project down to, because the
/// directory chain needs `.cwd` from the same validated read. Note what that makes load-bearing:
/// a record can only be validated when the process start is readable, so the "both sources
/// failed" row needs *three* failures, not two.
fn validated_registration(
    read: impl FnOnce(u32) -> Option<ai_monitor::Registered>,
    pid: u32,
    started_ms: Option<u64>,
) -> Option<ai_monitor::Registered> {
    let started = started_ms?;
    let reg = read(pid)?;
    // ONE rule, shared with the rival check in `ai_monitor::resolve_owned_session`
    // (R3.3). It used to live here alone; a second copy for rivals is precisely the
    // shape that has already produced two defects in this feature.
    ai_monitor::validated_session_id(&reg, started)?;
    Some(reg)
}

/// [`validated_registration`] projected down to its session id.
///
/// Test-only since the chain began needing the whole record. Kept as a projection rather than
/// folded away so that `a_recycled_pid_never_inherits_the_dead_process_s_session` — which pins
/// the stale-record guard, the most safety-critical rule this module applies — keeps passing
/// **byte-unedited**; same call, and the same reason, as [`ProcTable::rival_starts`] below.
#[cfg(test)]
fn registered_id_with(
    read: impl FnOnce(u32) -> Option<ai_monitor::Registered>,
    pid: u32,
    started_ms: Option<u64>,
) -> Option<String> {
    validated_registration(read, pid, started_ms).map(|r| r.session_id)
}

/// How long to wait for the tmux root before giving up and deriving from the tab's own shell.
/// Generous for a local unix-socket query, imperceptible on a keypress.
const RESOLVE_BUDGET: Duration = Duration::from_millis(250);

/// The tmux-aware root, with a bounded wait.
///
/// `hint_for` runs on the **winit event-loop thread** — `Overlay::open` is reached from a
/// keypress and from every tab switch while the panel is visible. `tmux list-clients` talks to a
/// user-space server over a unix socket and can block indefinitely (a SIGSTOPped server, a wedged
/// `$TMUX_TMPDIR`), which would freeze input and rendering, not merely the overlay. `/bin/ps`,
/// the existing precedent here, is kernel-backed and always terminates; this is not.
///
/// So the resolve runs on a worker thread and this waits [`RESOLVE_BUDGET`] for it, falling back
/// to the tab's own pid — the same "no re-root" answer every other failure yields. A hung server
/// then costs the *feature*, never the UI. The honest price is one blocked thread leaked per
/// hang, which is vastly cheaper than a frozen terminal.
/// `pub(crate)` so the ai-monitor overlay can reuse this exact guard when resolving the pane's
/// *cwd* on open (tmux-pane-cwd D12). Sharing it is deliberate: a second copy of a 250 ms
/// timeout around a call that can hang the UI is precisely the kind of safety logic that must
/// not be duplicated.
pub(crate) fn resolved_root(shell_pid: u32) -> u32 {
    resolved_root_with(crate::tabmeta::tmux::effective_root_uncached, shell_pid)
}

/// [`resolved_root`]'s body, generic over the resolver so the budget is testable without
/// actually hanging the suite.
fn resolved_root_with(resolve: impl FnOnce(u32) -> u32 + Send + 'static, shell_pid: u32) -> u32 {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may already have timed out and gone; that is the expected path, not an
        // error, so the send result is deliberately discarded.
        let _ = tx.send(resolve(shell_pid));
    });
    rx.recv_timeout(RESOLVE_BUDGET).unwrap_or(shell_pid)
}

/// Which `claude` this tab's session hint should describe: the tmux pane's when one is running
/// there, else whatever the tab's own subtree holds.
///
/// Pure — the process walk arrives through an injected closure, so the fallback order and the
/// single-walk guard are table-testable with no tmux, no live tty and no live pid. It returns the
/// chosen **pid** rather than a built `SessionHint` for the same reason: building one would need
/// `start_ms`, an uninjected syscall that would make these tests machine-dependent.
///
/// **There is no directory comparison here, and its absence is the fix.** A cwd gate used to admit
/// the pane's result only on a positive `Some(a) == Some(b)` match between that `claude`'s cwd and
/// the **tab shell's** — which under tmux is the *client's*. A tab that ran `tmux attach` from `~`
/// onto a pane working in a project therefore failed the compare, fell through to a subtree whose
/// only descendant is the tmux client, and got no hint at all: `NO SESSION` for a session being
/// written the whole time.
///
/// The gate existed to stop a hint about process P being resolved against a directory P is not
/// writing into. That is a real hazard, and it is now closed one level up instead:
/// [`SessionHint::cwd`] carries the chosen `claude`'s **own** directory, so hint and directory
/// describe the same process by construction and there is no second candidate left to compare
/// (D1, D2). Every version of the old failure needed two candidates.
///
/// The consequence for this function is that it performs **no** cwd read on any path — which is
/// why the reader parameter is gone from the signature rather than left in place and unused. Its
/// other two contracts are unchanged: the pane wins over the tab's own `claude` when both exist,
/// and a pane with no `claude` still falls through to the tab's own subtree rather than
/// answering `None`.
fn choose_claude(root: u32, shell_pid: u32, find: impl Fn(u32) -> Option<u32>) -> Option<u32> {
    // The pane branch: entered only when a pane was genuinely resolved, so a non-tmux tab does
    // not pay a second identical walk.
    if root != shell_pid {
        if let Some(pid) = find(root) {
            return Some(pid);
        }
        // A pane with no `claude` falls through. Never `None` — the tab's own subtree is what
        // today's code searches, so this path cannot be worse than today.
    }
    find(shell_pid)
}

/// This tab's cwd, from its pid. A third in-repo copy of the same `proc_pidinfo` call
/// (`session.rs` has a private one; `tabmeta::proc`'s is `pub(crate)` but sits inside a private
/// `mod proc`), forced by the module boundary rather than by carelessness.
fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    // `!=` rather than `<` for the reason given at `session::cwd_of_pid`: the
    // success value is exactly `size`, and `<` carries a `>` mutant that is
    // equivalent and therefore unkillable.
    if n != size {
        return None;
    }
    let raw = &info.pvi_cdir.vip_path;
    let bytes = unsafe {
        std::slice::from_raw_parts(raw.as_ptr() as *const u8, std::mem::size_of_val(raw))
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&bytes[..end]).ok().map(PathBuf::from)
}

/// The process table: `pid -> (ppid, argv)`, read once per resolution.
///
/// **Identification is by argv, not by the kernel's process name.** Claude Code
/// replaces its own: `proc_pidinfo`'s `pbi_comm` for a live `claude` reads `2.1.220`
/// — its version — so a walk matching on that finds nothing at all, silently. The
/// command line carries both `claude` and `--resume <id>`.
struct ProcTable {
    rows: HashMap<u32, (u32, String)>,
    kids: HashMap<u32, Vec<u32>>,
}

impl ProcTable {
    fn read() -> Option<ProcTable> {
        let out = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,args="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Self::parse(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Split out from [`read`](Self::read) so the parsing is testable without a
    /// process table.
    fn parse(text: &str) -> ProcTable {
        let mut rows = HashMap::new();
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (Some(pid), Some(ppid), Some(argv0)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
                continue; // a wrapped or malformed row
            };
            let rest = it.collect::<Vec<_>>().join(" ");
            let args = match rest.is_empty() {
                true => argv0.to_string(),
                false => format!("{argv0} {rest}"),
            };
            rows.insert(pid, (ppid, args));
            kids.entry(ppid).or_default().push(pid);
        }
        ProcTable { rows, kids }
    }

    /// Breadth-first search for a `claude` descendant of `root`.
    ///
    /// Breadth-first on purpose: the tab's own `claude` is shallow, and a deep
    /// unrelated subtree (a build, a language server) should not be walked first.
    fn find_claude(&self, root: u32) -> Option<u32> {
        let mut frontier = vec![root];
        let mut visited = 0usize;
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            for pid in frontier {
                visited += 1;
                if visited > MAX_VISITED {
                    return None;
                }
                if pid != root && self.is_claude(pid) {
                    return Some(pid);
                }
                if let Some(children) = self.kids.get(&pid) {
                    next.extend(children.iter().copied());
                }
            }
            if next.is_empty() {
                return None;
            }
            frontier = next;
        }
        None
    }

    /// Is `pid` a `claude` process? Judged by argv[0]'s file name, so a bare `claude`
    /// and an absolute path to one both match.
    fn is_claude(&self, pid: u32) -> bool {
        self.rows.get(&pid).is_some_and(|(_, args)| {
            args.split_whitespace()
                .next()
                .and_then(|a| a.rsplit('/').next())
                .is_some_and(|exe| exe == "claude")
        })
    }

    /// The id in `--resume <id>`, when the process was started that way.
    /// Start times of every OTHER live `claude` working in `mine`.
    ///
    /// Deliberately not limited to this app's tabs: the question tier 2 has to answer is
    /// "did some other process write this log", and Warp's `claude` answers it exactly as
    /// well as a sibling tab's. The whole process table is already in hand
    /// (`ps -axo`), so this costs one `cwd_of_pid` + `start_ms` per `claude` on the
    /// machine — single digits in practice, and it runs at panel-open, never per frame.
    ///
    /// A rival whose cwd cannot be read is skipped rather than assumed to match: claiming
    /// a rival that is not there would refuse a log that really is ours, and failing to
    /// identify our own session must never be worse than not having looked.
    #[cfg(test)]
    fn rival_starts(
        &self,
        mine_pid: u32,
        mine_cwd: Option<&std::path::Path>,
        cwd_of: impl Fn(u32) -> Option<PathBuf>,
        start_of: impl Fn(u32) -> Option<u64>,
    ) -> Vec<ai_monitor::Rival> {
        self.rival_starts_checked(mine_pid, mine_cwd, cwd_of, start_of)
            .0
    }

    // `rival_starts` is no longer on the production path — `hint_for` needs the
    // known/unknown flag and so calls `rival_starts_checked` directly. It is kept as a
    // test-only projection rather than deleted so that
    // `rival_starts_names_every_other_claude_in_the_same_project` keeps passing
    // **byte-unedited**: that test pins the "an unreadable cwd claims nothing" contract,
    // and rewriting it in the same change that redefines emptiness is exactly the move
    // that would hide a regression in it (R4.4).

    /// [`rival_starts`](Self::rival_starts) plus whether the enumeration was **complete**.
    ///
    /// Three ways to come back empty-or-short without having actually looked, and all three
    /// must report `false`. Each drop is individually correct for the veto — inventing a
    /// rival we cannot read would refuse a log that really is ours — but since the rescue
    /// was narrowed, the same drop *also* decides whether an unbounded adopt is permitted,
    /// and there it must not be trusted:
    ///
    /// 1. our own cwd is unreadable, so no rival can be placed in our project at all;
    /// 2. a rival's cwd is unreadable, so it cannot be matched against ours;
    /// 3. a rival's start time is unreadable, so it cannot be ordered.
    fn rival_starts_checked(
        &self,
        mine_pid: u32,
        mine_cwd: Option<&std::path::Path>,
        cwd_of: impl Fn(u32) -> Option<PathBuf>,
        start_of: impl Fn(u32) -> Option<u64>,
    ) -> (Vec<ai_monitor::Rival>, bool) {
        let Some(mine_cwd) = mine_cwd else {
            return (Vec::new(), false); // (1) cannot compare projects; claim nothing
        };
        let mut rivals = Vec::new();
        let mut known = true;
        for pid in self
            .rows
            .keys()
            .copied()
            .filter(|pid| *pid != mine_pid && self.is_claude(*pid))
        {
            let Some(cwd) = cwd_of(pid) else {
                known = false; // (2)
                continue;
            };
            if cwd.as_path() != mine_cwd {
                continue; // a different project: correctly not a rival
            }
            let Some(started_ms) = start_of(pid) else {
                known = false; // (3)
                continue;
            };
            rivals.push(ai_monitor::Rival { pid, started_ms });
        }
        (rivals, known)
    }

    fn resume_id(&self, pid: u32) -> Option<String> {
        let (_, args) = self.rows.get(&pid)?;
        let mut toks = args.split_whitespace();
        while let Some(t) = toks.next() {
            if t == "--resume" || t == "-r" {
                return toks
                    .next()
                    .filter(|s| looks_like_session_id(s))
                    .map(str::to_string);
            }
            if let Some(rest) = t.strip_prefix("--resume=") {
                return looks_like_session_id(rest).then(|| rest.to_string());
            }
        }
        None
    }
}

/// When `pid` started, epoch millis, from `proc_pidinfo`.
///
/// Taken from the kernel rather than parsed out of `ps`, whose `lstart` column is a
/// locale-formatted local time — exactly the sort of thing that reads correctly on the
/// machine it was written on and shifts by hours elsewhere.
///
/// **A delegation, and deliberately so** (tabmeta-session-keying D6). The body moved to
/// `tabmeta::proc` when the sidebar's metadata worker began needing the same
/// value: this number is the input to `ai_monitor::validated_session_id`, which exists solely
/// to reject a stale `~/.claude/sessions/` record left behind on a recycled pid. Two copies
/// of that input, drifting apart, is the failure mode `validated_session_id`'s own doc
/// describes as having already produced two defects in this feature. One rule, one function.
fn start_ms(pid: u32) -> Option<u64> {
    // The **uncounted** body: same implementation (D6's whole point — one rule, one
    // function), but without `tabmeta`'s `PROC_START` cost probe. The panel has no business
    // firing the sidebar's instrumentation, and while it did, `tabmeta`'s absolute
    // assertions on that counter were a latent flake — these tests hold no probe lock.
    crate::tabmeta::proc::start_ms_uncounted(pid)
}

/// A session id is a UUID, and it becomes a filename — so anything that is not one is
/// rejected rather than joined onto a path.
/// A delegation (tabmeta-session-keying): the rule moved to `ai_monitor`, which owns the
/// session-id vocabulary, so the panel's `--resume` parser here and the sidebar card's argv
/// tier apply one guard rather than two copies that can drift apart.
fn looks_like_session_id(s: &str) -> bool {
    ai_monitor::looks_like_session_id(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// The third `proc_pidinfo` copy resolves its own process, exactly as
    /// `session.rs` and `tabmeta::proc` assert about theirs.
    ///
    /// It had no test at all, which only became visible when the blanket
    /// `exclude_re` covering "replace < with > in cwd_of_pid" came out of
    /// `.cargo/mutants.toml`: that pattern was unanchored, so it silenced the
    /// guard mutant in all three copies while its comment described one. With it
    /// gone, this copy reported four survivors — the whole function could be
    /// replaced with `None` — and the two that have tests reported none. That is
    /// the cost of an over-broad exclusion, and it is why the fix was to rewrite
    /// the guard rather than to widen the amnesty.
    #[test]
    fn cwd_of_pid_resolves_this_process() {
        let got = cwd_of_pid(std::process::id()).expect("own cwd is resolvable");
        let expected = std::env::current_dir().expect("current_dir");
        assert_eq!(
            got.canonicalize().ok(),
            expected.canonicalize().ok(),
            "cwd_of_pid(self) must equal current_dir()"
        );
    }

    /// The failure arm: `proc_pidinfo` reports an error for a pid that cannot
    /// exist, and the short-read guard must turn that into `None` rather than
    /// reading the zeroed struct as a path.
    #[test]
    fn cwd_of_unusable_pid_is_none() {
        assert!(cwd_of_pid(0).is_none());
    }

    /// A process table shaped like the measured one: two Crewterm tabs each with
    /// their own `claude`, and Warp running a resumed session in the same project.
    const SAMPLE: &str = "\
34637     1 ./target/debug/app
34643 34637 /bin/zsh
35303 34643 claude
35378 34637 /bin/zsh
35404 35378 /usr/local/bin/claude
53653   978 -zsh -g --no_rcs
10502 53653 claude --resume 54e576c2-bd76-4acd-8eed-bda54f60a155
  978   973 /Applications/Warp.app/Contents/MacOS/stable terminal-server
";

    #[test]
    fn finds_each_tabs_own_claude_and_no_one_elses() {
        let t = ProcTable::parse(SAMPLE);
        assert_eq!(t.find_claude(34643), Some(35303), "tab 1");
        assert_eq!(
            t.find_claude(35378),
            Some(35404),
            "tab 2, absolute path argv0"
        );
        assert_eq!(
            t.find_claude(53653),
            Some(10502),
            "Warp's, from Warp's shell"
        );
        // From the app itself, depth 2 — reachable, and one of its own tabs'.
        assert!(matches!(t.find_claude(34637), Some(35303) | Some(35404)));
        // The search reaches through intermediates: Warp.app -> terminal-server ->
        // zsh -> claude is depth 3.
        assert_eq!(t.find_claude(973), Some(10502));
        // A pid that is not in the table at all yields nothing rather than someone
        // else's session.
        assert_eq!(t.find_claude(99_999), None);
        // ... and so does a leaf.
        assert_eq!(
            t.find_claude(35303),
            None,
            "claude itself has no claude under it"
        );
    }

    #[test]
    fn a_recycled_pid_never_inherits_the_dead_process_s_session() {
        // The guard that makes reading `~/.claude/sessions/<pid>.json` safe at all.
        //
        // Those files outlive the process that wrote them — the sample this was built
        // from held 5 files for 4 live `claude` processes — so a pid handed out again
        // later finds a record describing a stranger's session and, being an exact
        // session id, it would be believed completely. That is this feature's original
        // defect with more confidence behind it, so the record is only trusted when it was
        // written just *after* the process now holding the pid started.
        const START: u64 = 1_785_449_383_000;
        let reg = |at: u64| {
            move |_pid: u32| {
                Some(ai_monitor::Registered {
                    session_id: "2b1ca0f2-8a99-4aff-90f1-115839f17f01".into(),
                    cwd: PathBuf::from("/proj/a"),
                    started_at_ms: at,
                })
            }
        };

        // Written 1.7s after this process started — the measured norm. Believed.
        assert_eq!(
            registered_id_with(reg(START + 1_700), 45326, Some(START)).as_deref(),
            Some("2b1ca0f2-8a99-4aff-90f1-115839f17f01")
        );
        // Exactly at the drift limit, and one millisecond past it.
        assert!(registered_id_with(
            reg(START + ai_monitor::REGISTRY_DRIFT_MS),
            45326,
            Some(START)
        )
        .is_some());
        assert_eq!(
            registered_id_with(
                reg(START + ai_monitor::REGISTRY_DRIFT_MS + 1),
                45326,
                Some(START)
            ),
            None
        );
        // Written BEFORE this process existed: a leftover from the pid's last owner.
        assert_eq!(registered_id_with(reg(START - 1), 45326, Some(START)), None);
        assert_eq!(
            registered_id_with(reg(START - 86_400_000), 45326, Some(START)),
            None,
            "a day-old record for a recycled pid claims nothing"
        );
        // No record, or no readable process start: infer nothing.
        assert_eq!(registered_id_with(|_| None, 45326, Some(START)), None);
        assert_eq!(registered_id_with(reg(START + 1_700), 45326, None), None);
    }

    /// Rivals now carry `(pid, started_ms)`; these assertions are about the START TIMES
    /// they always were about, so project the payload and leave the expectations alone.
    fn starts_of(rivals: Vec<ai_monitor::Rival>) -> Vec<u64> {
        rivals.into_iter().map(|r| r.started_ms).collect()
    }

    // ---- rival-assignment R4: known-empty vs unknown ----

    #[test]
    fn a_fully_enumerated_empty_set_is_known() {
        // R4.1 — the process table was read, every rival was placeable, and none was a
        // rival. Only this state may re-enable the unbounded uniqueness rescue.
        let t = ProcTable::parse(SAMPLE);
        let proj = PathBuf::from("/proj/a");
        let elsewhere = |_: u32| Some(PathBuf::from("/proj/elsewhere"));
        let start = |pid: u32| Some(1_000_000 + u64::from(pid));
        let (rivals, known) = t.rival_starts_checked(35303, Some(&proj), elsewhere, start);
        assert!(rivals.is_empty(), "nobody else works in our project");
        assert!(known, "and we could see that for every one of them");
    }

    #[test]
    fn an_unreadable_own_cwd_is_unknown_not_empty() {
        // R4.2 — path (1). The old code returned an empty vec here and the caller could
        // not tell it apart from "no rivals exist".
        let t = ProcTable::parse(SAMPLE);
        let same = |_: u32| Some(PathBuf::from("/proj/a"));
        let start = |pid: u32| Some(1_000_000 + u64::from(pid));
        let (rivals, known) = t.rival_starts_checked(35303, None, same, start);
        assert!(rivals.is_empty());
        assert!(
            !known,
            "an empty list from a failed read must not read as known-empty"
        );
    }

    #[test]
    fn a_rival_dropped_for_an_unreadable_read_is_unknown() {
        // R4.3 — paths (2) and (3). The single rival is dropped, the list ends up empty,
        // and that emptiness must NOT grant the rescue. This is the failure path the L2
        // review found on its third pass, after the first version of D12 listed only two
        // ways to come back empty.
        let t = ProcTable::parse(SAMPLE);
        let proj = PathBuf::from("/proj/a");
        let start = |pid: u32| Some(1_000_000 + u64::from(pid));

        // (2) the rival's own cwd cannot be read
        let (rivals, known) = t.rival_starts_checked(35303, Some(&proj), |_| None, start);
        assert!(rivals.is_empty());
        assert!(!known, "cwd unreadable -> unknown");

        // (3) the rival's start time cannot be read
        let same = |_: u32| Some(PathBuf::from("/proj/a"));
        let (rivals, known) = t.rival_starts_checked(35303, Some(&proj), same, |_| None);
        assert!(rivals.is_empty());
        assert!(!known, "start unreadable -> unknown");
    }

    #[test]
    fn rival_starts_names_every_other_claude_in_the_same_project() {
        // The signal that turns "date this session" into "assign this session". SAMPLE
        // holds four `claude` processes: 35303 and 35404 under this app's two tabs, 10502
        // under Warp, and the caller. Pure here — both syscalls are injected — so this is
        // table-testable with no live process.
        let t = ProcTable::parse(SAMPLE);
        let proj = PathBuf::from("/proj/a");
        let cwd = |pid: u32| {
            Some(PathBuf::from(match pid {
                10502 => "/proj/elsewhere", // Warp, but working somewhere else
                _ => "/proj/a",
            }))
        };
        let start = |pid: u32| Some(1_000_000 + u64::from(pid));

        // From 35303's point of view: its sibling tab counts, Warp-in-another-project
        // does not, and it never lists itself.
        let mut got = starts_of(t.rival_starts(35303, Some(&proj), cwd, start));
        got.sort_unstable();
        assert_eq!(
            got,
            vec![1_035_404],
            "the sibling tab's claude, and only it"
        );

        // Warp working in the SAME project is a rival like any other — the rule is about
        // which process wrote the log, not which application it belongs to.
        let same = |_: u32| Some(PathBuf::from("/proj/a"));
        let mut got = starts_of(t.rival_starts(35303, Some(&proj), same, start));
        got.sort_unstable();
        assert_eq!(got, vec![1_010_502, 1_035_404]);

        // An unreadable cwd claims nothing rather than guessing: a rival invented here
        // would refuse a log that really is ours.
        assert!(t.rival_starts(35303, None, same, start).is_empty());
        assert!(t
            .rival_starts(35303, Some(&proj), |_| None, start)
            .is_empty());
    }

    #[test]
    fn reads_the_session_id_off_a_resumed_process() {
        let t = ProcTable::parse(SAMPLE);
        assert_eq!(
            t.resume_id(10502).as_deref(),
            Some("54e576c2-bd76-4acd-8eed-bda54f60a155")
        );
        assert_eq!(t.resume_id(35303), None, "a fresh session names nothing");

        // The other spellings, and a rejected one.
        let t = ProcTable::parse(
            "1 0 claude --resume=54e576c2-bd76-4acd-8eed-bda54f60a155\n\
             2 0 claude -r 54e576c2-bd76-4acd-8eed-bda54f60a155\n\
             3 0 claude --resume ../../etc/passwd\n",
        );
        assert!(t.resume_id(1).is_some());
        assert!(t.resume_id(2).is_some());
        assert_eq!(
            t.resume_id(3),
            None,
            "not a session id, not joined to a path"
        );
    }

    #[test]
    fn a_process_named_claude_is_identified_by_argv_not_by_its_process_name() {
        // The defect this module was rewritten around: Claude Code sets its own
        // process name to its version, so `pbi_comm` reads "2.1.220" and matching on
        // it finds nothing. argv still says `claude`.
        let t = ProcTable::parse("5 0 claude\n6 0 2.1.220\n7 0 /opt/homebrew/bin/claude --foo\n");
        assert!(t.is_claude(5));
        assert!(!t.is_claude(6), "the version is not an executable name");
        assert!(t.is_claude(7));
    }

    #[test]
    fn a_malformed_or_empty_table_yields_nothing_rather_than_panicking() {
        let t = ProcTable::parse("");
        assert_eq!(t.find_claude(1), None);
        let t = ProcTable::parse("not a row\n\n   \nxx yy zz\n1 0\n");
        assert_eq!(t.find_claude(1), None);
        assert_eq!(t.resume_id(99), None);
    }

    #[test]
    fn the_walk_terminates_on_a_cycle() {
        // A parent/child cycle cannot happen on a real system, but the walk must not
        // depend on that: `MAX_DEPTH` and `MAX_VISITED` bound it either way.
        let t = ProcTable::parse("1 2 sh\n2 1 sh\n");
        assert_eq!(t.find_claude(1), None);
    }

    #[test]
    fn this_process_start_time_is_readable_and_sane() {
        let started = start_ms(std::process::id()).expect("own start time");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(started <= now, "not in the future");
        assert!(now - started < 24 * 3_600_000, "not a day old");
        assert_eq!(start_ms(u32::MAX), None, "a dead pid yields nothing");
    }

    #[test]
    fn start_ms_is_the_same_function_the_sidebar_uses() {
        // R1.4 / R7.5 — the panel and the sidebar card must validate registry records
        // against the SAME process-start number. This asserts the delegation is real: if
        // someone re-inlines a second `proc_pidinfo` here, the two could drift and a stale
        // record would be accepted by one path and rejected by the other.
        let pid = std::process::id();
        assert_eq!(
            start_ms(pid),
            crate::tabmeta::proc::start_ms_uncounted(pid),
            "tabsession::start_ms must delegate to tabmeta::proc's shared body"
        );
        assert_eq!(
            start_ms(u32::MAX),
            crate::tabmeta::proc::start_ms_uncounted(u32::MAX)
        );
    }

    #[test]
    fn a_session_id_must_look_like_one() {
        assert!(looks_like_session_id(
            "54e576c2-bd76-4acd-8eed-bda54f60a155"
        ));
        assert!(!looks_like_session_id("../../etc/passwd"));
        assert!(!looks_like_session_id("54e576c2"));
        assert!(!looks_like_session_id(""));
        assert!(!looks_like_session_id(
            "54e576c2/bd76/4acd/8eed/bda54f60a155"
        ));
        assert!(!looks_like_session_id(
            "zzzzzzzz-bd76-4acd-8eed-bda54f60a155"
        ));
    }

    #[test]
    fn an_empty_hint_is_recognisable() {
        assert!(SessionHint::default().is_empty());
        assert!(!SessionHint {
            started_ms: Some(1),
            ..Default::default()
        }
        .is_empty());
    }

    // ---- the tmux re-root: choose_claude, against fakes (R1, R3, R4) ----

    /// A tmux-shaped table: the tab's shell (34643) has a `tmux` client child, while the pane's
    /// shell (74877) hangs off the tmux **server** (90662) with `claude` beneath it. This is the
    /// measured shape, and it is why `find_claude(tab_shell)` finds nothing today.
    const TMUX_SAMPLE: &str = "\
34637     1 ./target/debug/app
34643 34637 /bin/zsh
75071 34643 tmux
90662     1 tmux
74877 90662 -zsh
74922 74877 claude --resume 54e576c2-bd76-4acd-8eed-bda54f60a155
";

    #[test]
    fn a_tmux_tab_finds_nothing_from_its_own_shell_but_does_from_the_pane() {
        // R3.1 / R3.2 — the defect this feature fixes, and the premise the hop relies on. The
        // pane is a child of the tmux SERVER, so it is not in the tab's subtree at any depth.
        let t = ProcTable::parse(TMUX_SAMPLE);
        assert_eq!(
            t.find_claude(34643),
            None,
            "the defect: nothing from the tab"
        );
        assert_eq!(
            t.find_claude(74877),
            Some(74922),
            "found from the pane root"
        );
    }

    /// A tab with a `claude` in **both** subtrees — its own, and its tmux pane's. Without this
    /// shape the interesting cases are unobservable: with only one candidate, "the pane wins"
    /// and "the rejected pane falls through to the tab" both look identical to "found the only
    /// one there was".
    const BOTH_SAMPLE: &str = "\
34637     1 ./target/debug/app
34643 34637 /bin/zsh
35303 34643 claude
75071 34643 tmux
90662     1 tmux
74877 90662 -zsh
74922 74877 claude
";

    #[test]
    fn the_pane_wins_over_the_tabs_own_claude_when_both_exist() {
        // R2.2 — "the pane is the more specific answer and must win when both exist."
        // Anchors the ORDER: a tab-first implementation would return 35303 here. Survives the
        // gate's deletion unchanged apart from the dropped reader; this contract was never the
        // gate's.
        let t = ProcTable::parse(BOTH_SAMPLE);
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p)),
            Some(74922),
            "the pane's claude, not the tab's own"
        );
    }

    #[test]
    fn a_pane_with_no_claude_falls_through_to_the_tabs_own_claude() {
        // R2.3 — was `a_rejected_pane_falls_through_to_the_tabs_own_claude`. There is no
        // rejection any more (D2), but the guarantee it was protecting is the same one and
        // stays: the pane branch must never answer `None` where the tab's own subtree has an
        // answer. That is the load-bearing "never worse than today" property, and a `return
        // None` in the pane branch is still exactly what would break it.
        //
        // It also absorbs the old `the_tab_subtree_is_retried_and_is_never_cwd_gated`, which
        // asserted this same fall-through with a divergent cwd reader. With no reader left to
        // diverge, that test's table and assertion became byte-identical to this one's — kept as
        // two names it would have been apparent coverage, so it is recorded here instead.
        let t = ProcTable::parse(
            "34643 34637 /bin/zsh\n\
             35303 34643 claude\n\
             75071 34643 tmux\n\
             90662     1 tmux\n\
             74877 90662 -zsh\n",
        );
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p)),
            Some(35303),
            "falls through to the tab's own claude — never None"
        );
    }

    #[test]
    fn a_pane_working_elsewhere_is_admitted() {
        // R2.1 — was `a_pane_working_elsewhere_is_not_admitted`, and the inversion IS the fix.
        //
        // The property that test named is still enforced, one level up and in a stronger form:
        // it worried that a pane working in another project would have its `started_ms` resolved
        // against the TAB's projects directory, where the asymmetric `since_ms` filter can return
        // a stranger's log with `owned: true`. The answer is no longer "refuse the pane" — which
        // cost every cross-directory tmux tab its panel — but "resolve it against its OWN
        // project", which `SessionHint::cwd` now carries (D1). Nothing is resolved against a
        // directory its process is not in, and the pane is admitted.
        //
        // TMUX_SAMPLE is the measured shape: the tab's shell has only a tmux client under it, so
        // before this change the fall-through found nothing and `hint_for` returned `None`.
        // Also absorbs `the_pane_wins_when_its_claude_shares_the_tabs_cwd`, whose premise (a cwd
        // that agrees) no longer names a distinguishable case.
        let t = ProcTable::parse(TMUX_SAMPLE);
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p)),
            Some(74922),
            "the pane's claude is admitted regardless of where the tab shell sits"
        );
    }

    /// The production half of this file, comments stripped — the idiom used for the source-text
    /// pins in `overlay.rs`, for the same reason: a rule that lives in the *shape* of a call has
    /// no runtime seam to assert against.
    fn production_half() -> String {
        let src = include_str!("tabsession.rs");
        let (production, tests) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("no `mod tests` marker to split on");
        assert!(
            !tests.is_empty() && production.len() > src.len() / 4,
            "the split found the marker in an implausible place"
        );
        production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with("///"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_directory_comes_from_the_found_claude_not_the_pane_shell() {
        // R1.1 — was `the_gate_compares_the_found_claude_not_the_pane_shell`. The gate is gone,
        // but the reason it compared the found `claude` rather than the pane's `-zsh` is the
        // whole of D1 and is now the rule for the DIRECTORY itself: the pane shell moves with
        // every `cd`, while the projects directory that matters is the one `claude` writes into.
        //
        // A source-text pin because the rule lives in which pid is passed to `cwd_of_pid`, and
        // both spellings type-check. `hint_for`'s three pids — `shell_pid`, `root` and `claude` —
        // are all `u32`, so a transposition is a one-token edit that compiles.
        let p = production_half();
        assert!(
            p.contains("let kernel = cwd_of(claude);"),
            "the hint's directory must be read from the CHOSEN claude's pid"
        );
        assert!(
            p.contains("resolve_hint_cwd(claude, kernel,"),
            "and that value must be what the provenance chain resolves"
        );
        for wrong in [
            "cwd_of(shell_pid)",
            "cwd_of(root)",
            "cwd_of_pid(shell_pid)",
            "cwd_of_pid(root)",
        ] {
            assert!(
                !p.contains(wrong),
                "neither the tab shell's cwd nor the pane shell's may be read here ({wrong}) — \
                 that is the pair of candidates D1 exists to collapse to one"
            );
        }
    }

    #[test]
    fn choose_claude_reads_no_cwd_on_any_path() {
        // R2.4 — was `an_unreadable_cwd_never_counts_as_agreement`, which pinned that `None ==
        // None` must not read as agreement. There is no agreement test left to fool: the
        // function reads no cwd at all, so an unreadable one cannot change its answer.
        //
        // Asserted structurally rather than with a counting closure, because the parameter is
        // gone: a counter would have nowhere to be called from and would pass vacuously — the
        // exact failure R8.2 records below. The pin is that the signature stays reader-free, so
        // re-introducing one is a visible edit rather than a quiet re-gate.
        //
        // D8 prescribed that this test become "D3/D7's fallback chain". That property did not
        // vanish, it MOVED: an unreadable cwd is now answered one level up, by
        // `row_4_a_failed_kernel_read_falls_back_to_the_registry_without_the_rescue`,
        // `row_5_both_sources_failing_yields_no_directory` and
        // `an_unreadable_process_start_cannot_validate_a_record`. What is left here is the
        // narrower thing only this function can still get wrong.
        let p = production_half();
        assert!(
            p.contains("fn choose_claude(root: u32, shell_pid: u32, find: impl Fn(u32) -> Option<u32>) -> Option<u32> {"),
            "choose_claude must take the walk and nothing else"
        );
        let body = p
            .split("fn choose_claude")
            .nth(1)
            .and_then(|rest| rest.split("\nfn ").next())
            .expect("choose_claude body");
        // Anchor first: a split that found the wrong region would make the negative below
        // pass without asserting anything — the same vacuity R8.2 deletes a test for.
        assert!(
            body.contains("find(shell_pid)") && body.contains("if root != shell_pid"),
            "the extracted region is not choose_claude's body"
        );
        assert!(
            !body.contains("cwd"),
            "no cwd may be read inside choose_claude on any path"
        );

        // And behaviourally, across the three shapes: re-rooted, not re-rooted, and a dead pid.
        let t = ProcTable::parse(TMUX_SAMPLE);
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p)),
            Some(74922)
        );
        assert_eq!(choose_claude(34643, 34643, |p| t.find_claude(p)), None);
        assert_eq!(choose_claude(99_999, 99_999, |p| t.find_claude(p)), None);
    }

    #[test]
    fn nothing_re_rooted_means_exactly_one_walk() {
        // R8.2 — was `nothing_re_rooted_means_exactly_one_walk_and_no_cwd_read`. The walk count
        // is still a real non-occurrence and stays. Its `cwds == 0` half was DELETED rather than
        // kept: after D2 `choose_claude` performs no cwd read on any input, so that assertion
        // held for every possible argument and could never fail — apparent coverage. Same call
        // this repo has already made once, at `tmux.rs:648-651`.
        //
        // The cost it was really guarding moved rather than vanished: `hint_for` is now where a
        // cwd is read, and that it is read exactly once per invocation is pinned by
        // `the_directory_costs_no_additional_syscall` against the `hint_for_with` seam.
        let t = ProcTable::parse("1 0 /bin/zsh\n");
        let walks = std::cell::RefCell::new(0usize);
        let got = choose_claude(
            7,
            7, // root == shell_pid: the non-tmux case
            |p| {
                *walks.borrow_mut() += 1;
                t.find_claude(p)
            },
        );
        assert_eq!(got, None);
        assert_eq!(
            *walks.borrow(),
            1,
            "the guard suppressed a second identical walk"
        );
    }

    // ---- the directory chain: D7's five rows, through the `hint_for_with` seam (R3, R4) ----

    /// TMUX-shaped, plus a second `claude` (55502) elsewhere in the table so the rival
    /// enumeration has something to find or drop.
    const CHAIN_SAMPLE: &str = "\
34637     1 ./target/debug/app
34643 34637 /bin/zsh
75071 34643 tmux
90662     1 tmux
74877 90662 -zsh
74922 74877 claude
55501     1 /bin/zsh
55502 55501 claude
";
    const TAB: u32 = 34643;
    const PANE: u32 = 74877;
    const OURS: u32 = 74922;
    const RIVAL: u32 = 55502;
    const PROC_START: u64 = 1_785_449_383_000;
    const SID: &str = "2b1ca0f2-8a99-4aff-90f1-115839f17f01";

    /// A record `claude` wrote for itself, 1.7s after start — the measured norm, so it
    /// validates against `PROC_START`.
    fn record(cwd: &str) -> ai_monitor::Registered {
        ai_monitor::Registered {
            session_id: SID.into(),
            cwd: PathBuf::from(cwd),
            started_at_ms: PROC_START + 1_700,
        }
    }

    /// The chain under test, with all three reads supplied. The pane's `claude` (`OURS`) is the
    /// one chosen, so this drives exactly the path a `tmux attach` tab takes.
    fn run_chain(
        kernel: impl Fn(u32) -> Option<PathBuf>,
        registry: Option<ai_monitor::Registered>,
        start: impl Fn(u32) -> Option<u64>,
    ) -> SessionHint {
        let t = ProcTable::parse(CHAIN_SAMPLE);
        hint_for_with(&t, PANE, TAB, kernel, move |_| registry.clone(), start)
            .expect("the pane's claude is always found in CHAIN_SAMPLE")
    }

    /// Every rival read succeeds, in `/proj/a`, with an orderable start.
    fn all_in(dir: &'static str) -> impl Fn(u32) -> Option<PathBuf> {
        move |_| Some(PathBuf::from(dir))
    }
    fn starts(_pid: u32) -> Option<u64> {
        Some(PROC_START)
    }

    #[test]
    fn hint_cwd_carries_the_directory_and_the_permission() {
        // R1.2 — the encoding check. Both variants name a directory; only one licenses tier 2.
        // Carrying the permission IN the value is what makes "unproven directory with tier 2
        // enabled" unrepresentable rather than merely forbidden.
        let proven = HintCwd::Proven(PathBuf::from("/proj/a"));
        let disputed = HintCwd::Disputed(PathBuf::from("/proj/b"));
        assert_eq!(proven.path(), Path::new("/proj/a"));
        assert_eq!(disputed.path(), Path::new("/proj/b"));
        assert!(proven.tier2_ok());
        assert!(
            !disputed.tier2_ok(),
            "tier 2 must never run on a disputed dir"
        );
    }

    #[test]
    fn row_1_kernel_only_is_proven() {
        // R3.1 — nothing to cross-check against, and the kernel is the direct answer.
        let h = run_chain(all_in("/proj/a"), None, starts);
        assert_eq!(h.cwd, Some(HintCwd::Proven(PathBuf::from("/proj/a"))));
        assert!(h.cwd.as_ref().is_some_and(HintCwd::tier2_ok));
        // Tier 2's permission travels as `tier2_ok`; `open()` is what turns it into
        // `Wanted::since_ms`, and `started_ms` is the value it will use.
        assert_eq!(h.started_ms, Some(PROC_START));
    }

    #[test]
    fn row_2_kernel_and_registry_agreeing_is_proven() {
        // R3.2 — two independent sources, one answer.
        let h = run_chain(all_in("/proj/a"), Some(record("/proj/a")), starts);
        assert_eq!(h.cwd, Some(HintCwd::Proven(PathBuf::from("/proj/a"))));
        assert_eq!(
            h.session_id.as_deref(),
            Some(SID),
            "and the same validated record still names the session"
        );
    }

    #[test]
    fn row_3_a_disagreement_is_disputed_and_the_registry_wins() {
        // R3.3 — the only remaining detector of a violated A1, and it must be ACTIONABLE: the
        // registry is `claude`'s own statement about where it runs, so it wins the tie, and
        // tier 2 is suppressed because we are now using the weaker of two contradicting claims.
        let h = run_chain(all_in("/proj/a"), Some(record("/proj/b")), starts);
        assert_eq!(h.cwd, Some(HintCwd::Disputed(PathBuf::from("/proj/b"))));
        assert!(
            !h.cwd.as_ref().is_some_and(HintCwd::tier2_ok),
            "a disputed directory licenses tier 1 only"
        );
        // Row 3 is MIXED-BASIS — our directory came from the registry while every rival's still
        // comes from the kernel — so it forfeits the rescue exactly as row 4 does. D7's table
        // states this only for row 4; it is asserted here because this fixture is the one place
        // it could silently flip: `enumerated` stays `true` (no rival read fails), so nothing
        // else in the chain would force `rivals_known` false if row 3's `mixed_basis` were
        // changed to `false`.
        assert!(
            !h.rivals_known,
            "a directory chosen against a contradicting kernel never earns the uniqueness rescue"
        );
    }

    #[test]
    fn row_4_a_failed_kernel_read_falls_back_to_the_registry_without_the_rescue() {
        // R3.4 / R4.2 — tier 2 stays allowed (our read simply failed; there is no evidence of a
        // systematic skew), but the enumeration is mixed-basis, so the uniqueness rescue is
        // forfeited. Today an unreadable own cwd hits `rival_starts_checked`'s guard (1) and
        // yields `known: false`; resolving the directory from elsewhere must not WIDEN that into
        // a `known: true` that grants an unbounded adopt.
        let h = run_chain(
            |pid| (pid != OURS).then(|| PathBuf::from("/proj/a")),
            Some(record("/proj/a")),
            starts,
        );
        assert_eq!(h.cwd, Some(HintCwd::Proven(PathBuf::from("/proj/a"))));
        assert!(h.cwd.as_ref().is_some_and(HintCwd::tier2_ok));
        assert!(
            !h.rivals_known,
            "a registry-sourced directory never earns the uniqueness rescue"
        );
    }

    #[test]
    fn row_5_both_sources_failing_yields_no_directory() {
        // R3.5 — `open()` falls back to `pane_cwd` with tier 2 off. Still a hint: a live, named
        // session must not go invisible for a fallible syscall, which is the class of failure
        // this whole feature exists to remove.
        let h = run_chain(|_| None, None, starts);
        assert_eq!(h.cwd, None);
        assert_eq!(h.pid, Some(OURS), "the hint itself still names its process");
    }

    #[test]
    fn a_tab_with_no_claude_yields_no_hint_at_all() {
        // R1.4 — the whole chain must answer `None`, not an empty-but-present hint. The
        // overlay reads the `Option`'s **shape** (`tab_has_claude`, set before
        // `unwrap_or_default()` flattens it) to choose between "(no Claude session in this
        // tab)" and "(this tab's session has not written a log yet)", so the two are different
        // answers rather than degrees of the same one.
        //
        // Headless, unlike `a_real_lookup_on_a_dead_pid_is_none` below, which reads the real
        // process table and is therefore machine-dependent: this drives the chain over a fake
        // tmux tree whose pane and tab subtree both hold nothing but a shell and a tmux client.
        let t = ProcTable::parse(
            "34643 34637 /bin/zsh\n\
             75071 34643 tmux\n\
             90662     1 tmux\n\
             74877 90662 -zsh\n",
        );
        assert!(
            hint_for_with(&t, PANE, TAB, all_in("/proj/a"), |_| None, starts).is_none(),
            "no claude in the pane and none in the tab's own subtree means no hint"
        );
        // And a readable directory does not rescue it: the directory describes a process, so
        // with no process there is nothing for it to describe.
        assert!(hint_for_with(
            &t,
            PANE,
            TAB,
            all_in("/proj/a"),
            |_| Some(record("/proj/a")),
            starts
        )
        .is_none());
    }

    #[test]
    fn an_unreadable_process_start_cannot_validate_a_record() {
        // R3.7 — the third input the last row actually needs. Validation is
        // `validated_session_id`, which compares the record against the PROCESS START; with no
        // start there is nothing to compare, so a present record is not a validated one and the
        // chain degrades to row 5 rather than believing a possibly-recycled pid's file.
        let h = run_chain(|_| None, Some(record("/proj/a")), |_| None);
        assert_eq!(h.cwd, None, "an unvalidated record supplies no directory");
        assert_eq!(h.session_id, None, "nor a session id");
        assert_eq!(h.started_ms, None);
    }

    #[test]
    fn suppression_is_expressed_on_the_permission_never_by_blanking_the_hint() {
        // R3.6 — the field this is NOT allowed to use. Zeroing `started_ms` to suppress tier 2
        // holds the same quantity and would look equivalent, while breaking two unrelated
        // things: `baseline_from` would yield `None`, killing rotation detection for the life of
        // the open, and `rival_refresh_needed(attached, armed = false)` would then return `true`
        // forever, putting a `/bin/ps` fork and a 250 ms tmux wait on every append burst.
        for h in [
            run_chain(all_in("/proj/a"), Some(record("/proj/b")), starts), // row 3
            run_chain(|_| None, None, starts),                             // row 5
        ] {
            assert_eq!(
                h.started_ms,
                Some(PROC_START),
                "the real process start survives suppression"
            );
        }
    }

    #[test]
    fn the_resolved_directory_feeds_the_rival_enumeration() {
        // R4.1 — the ordering defect. `rival_starts_checked`'s FIRST guard returns
        // `(vec![], false)` when our own cwd is `None`, so passing the raw kernel read here
        // would mean that in exactly the state row 4 exists for, tier 2 runs in the right
        // directory with an empty rival list — no rival able to veto, only the ±120s distance
        // test left. That is the "two tabs in one project, `claude` seconds apart" defect that
        // has already shipped once.
        let h = run_chain(
            |pid| (pid != OURS).then(|| PathBuf::from("/proj/a")),
            Some(record("/proj/a")),
            starts,
        );
        assert_eq!(
            h.rival_starts.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![RIVAL],
            "the rival in the RESOLVED directory is enumerated, not dropped by guard (1)"
        );
        // ... and the veto being populated is exactly why `known: false` is not a regression:
        // the rescue is withheld while the veto still works.
        assert!(!h.rivals_known);
    }

    #[test]
    fn the_directory_costs_no_additional_syscall() {
        // R1.3, and the assertion `nothing_re_rooted_means_exactly_one_walk` handed over when
        // its own `cwds == 0` became vacuous. One read for the chosen `claude` plus one per
        // other `claude` in the table — the same count `hint_for` paid before this feature.
        // The pane branch's two extra reads (the deleted gate compared `claude`'s cwd against
        // the tab shell's) are gone outright, so the total on a tmux tab FELL by two.
        let calls = std::cell::RefCell::new(Vec::new());
        let t = ProcTable::parse(CHAIN_SAMPLE);
        let got = hint_for_with(
            &t,
            PANE,
            TAB,
            |pid| {
                calls.borrow_mut().push(pid);
                Some(PathBuf::from("/proj/a"))
            },
            |_| None,
            starts,
        );
        assert!(got.is_some());
        let mut seen = calls.borrow().clone();
        seen.sort_unstable();
        assert_eq!(
            seen,
            vec![RIVAL, OURS],
            "exactly one read for the chosen claude, one per rival, and nothing for either shell"
        );
    }

    #[test]
    fn a_cwd_dispute_warns_once_per_transition() {
        // D7's rate discipline, as a rule rather than as a log assertion. `hint_for` runs from
        // `refresh_rivals` per append burst while unarmed, so an unguarded `warn!` would emit
        // continuously for a STABLE violation — the in-repo rule (`worker.rs`'s
        // `note_activity_gap`) is that only transitions warn.
        let mut seen = Vec::new();
        assert!(dispute_transition(&mut seen, 42, true), "entering: warn");
        assert!(!dispute_transition(&mut seen, 42, true), "still: debug");
        assert!(!dispute_transition(&mut seen, 42, true));
        // A second pid disputes independently of the first.
        assert!(dispute_transition(&mut seen, 43, true));
        // Leaving the state forgets it, so a later disagreement warns again rather than
        // whispering for the life of the process.
        assert!(!dispute_transition(&mut seen, 42, false));
        assert!(dispute_transition(&mut seen, 42, true), "re-entering: warn");
        assert!(!dispute_transition(&mut seen, 99, false), "never disputed");
    }

    #[test]
    fn a_disputed_row_is_actually_wired_to_the_warning() {
        // R3.3 — the test above pins the once-per-transition RULE; this pins that anything is
        // CONNECTED to it. The distinction is the whole reason this repo writes source-text
        // pins: a rule can be perfectly unit-tested while nothing calls it, and here that
        // failure would be silent in the worst possible place — D2 deleted the last comparison
        // in the system, so this `warn!` is A1's **only** remaining detector. Unwire it and a
        // violated A1 shows up as a panel that merely looks empty.
        //
        // No behavioural alternative exists: this repo has no test-side `tracing` capture
        // (only `common/src/logging.rs` touches `tracing_subscriber`), and adding a global
        // subscriber for one assertion would be a heavier seam than the rule deserves.
        let p = production_half();
        assert!(
            p.contains("note_cwd_dispute(pid, Some((&k, r)));"),
            "the disagreement arm must report BOTH directories — the kernel's and the \
             registry's — or the log cannot say what disagreed with what"
        );
        // The transition branch must warn, and name all three values R3.3 asks for.
        let warn = p
            .split("if transition {")
            .nth(1)
            .and_then(|rest| rest.split("} else {").next())
            .expect("note_cwd_dispute's transition branch");
        assert!(
            warn.contains("tracing::warn!"),
            "a transition into the disputed state must WARN, not whisper"
        );
        // The full field spelling, not the bare name: the message literal itself says "disagrees
        // with the kernel's", so a bare `contains("kernel")` would still pass after the field was
        // deleted — the assertion would be satisfied by the prose describing it.
        for field in [
            "pid,",
            "kernel = %kernel.display()",
            "registry = %registry.display()",
        ] {
            assert!(
                warn.contains(field),
                "the warning must carry `{field}` — a bare 'cwd disputed' is undiagnosable"
            );
        }
        // ...and the repeat path must stay at `debug!`. `hint_for` runs from `refresh_rivals`
        // per append burst while unarmed, so a `warn!` here would emit continuously for a
        // STABLE violation and train the reader to ignore the one message that matters.
        assert!(
            p.contains("tracing::debug!(pid, registry = %registry.display()"),
            "a repeat must stay at debug!, per the in-repo rule that routine failures never warn"
        );
    }

    // ---- the resolve budget (R5) ----

    #[test]
    fn a_hung_resolver_degrades_to_the_tabs_own_pid_within_the_budget() {
        // R5.1 — the property that keeps a wedged tmux server from freezing the terminal: it
        // must RETURN, and return the fallback.
        let start = std::time::Instant::now();
        let got = resolved_root_with(
            |_| {
                std::thread::sleep(RESOLVE_BUDGET * 8);
                99999 // never observed
            },
            4242,
        );
        assert_eq!(got, 4242, "budget expired -> the tab's own pid");
        assert!(
            start.elapsed() < RESOLVE_BUDGET * 4,
            "must return near the budget, not wait out the resolver"
        );
    }

    #[test]
    fn a_prompt_resolver_value_is_used() {
        // R5.2 — the bound costs nothing in the normal case.
        assert_eq!(resolved_root_with(|_| 74877, 34643), 74877);
    }

    #[test]
    fn a_real_lookup_on_a_dead_pid_is_none() {
        assert_eq!(hint_for(u32::MAX), None);
    }
}

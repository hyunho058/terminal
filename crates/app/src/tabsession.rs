//! Which Claude session is running in *this tab* — answered from the tab's own
//! process tree rather than from the log directory.
//!
//! `~/.claude/projects/<encoded-cwd>/` records no application, window or tab: Warp,
//! iTerm, VS Code and every other mac-terminal window working in the same project all
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
use std::path::PathBuf;
// `SystemTime`/`UNIX_EPOCH` moved to the test module with `start_ms`'s body
// (tabmeta-session-keying D6) — only the tests still need a wall clock here.
use std::time::Duration;

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
pub fn hint_for(shell_pid: u32) -> Option<SessionHint> {
    let table = ProcTable::read()?;
    let root = resolved_root(shell_pid);
    let claude = choose_claude(root, shell_pid, |p| table.find_claude(p), cwd_of_pid)?;
    let mine = cwd_of_pid(claude);
    let started = start_ms(claude);
    // The known/unknown label is decided HERE, not inside `rival_starts` — that function's
    // contract (empty on an unreadable cwd) is pinned by
    // `rival_starts_names_every_other_claude_in_the_same_project`, and redefining its
    // return value would change that test's meaning. One level up costs nothing.
    let (rivals, rivals_known) =
        table.rival_starts_checked(claude, mine.as_deref(), cwd_of_pid, start_ms);
    Some(SessionHint {
        pid: Some(claude),
        // Ask the process what session it is running, before trying to infer it.
        // `--resume` is only the argv spelling of the same question and stays as the
        // fallback for a `claude` old enough not to write the registry file.
        session_id: registered_id(claude, started).or_else(|| table.resume_id(claude)),
        started_ms: started,
        rival_starts: rivals,
        rivals_known,
    })
}

/// The session id `claude` recorded for its own pid, validated against this process.
///
/// The validation is not ceremony. `~/.claude/sessions/` holds a file per pid and they
/// outlive the process — the sample that motivated this had **5 files for 4 live
/// processes** — so on a recycled pid an unvalidated read names a stranger's session with
/// total confidence, which is precisely the failure this feature exists to remove. A
/// record may only be believed when it was written just *after* the process we are asking
/// about started.
fn registered_id(pid: u32, started_ms: Option<u64>) -> Option<String> {
    registered_id_with(ai_monitor::registered_session, pid, started_ms)
}

/// The rule, with the file read injected so the guard is testable without a real
/// `~/.claude/sessions` or a live process.
fn registered_id_with(
    read: impl FnOnce(u32) -> Option<ai_monitor::Registered>,
    pid: u32,
    started_ms: Option<u64>,
) -> Option<String> {
    let started = started_ms?;
    let reg = read(pid)?;
    // ONE rule, shared with the rival check in `ai_monitor::resolve_owned_session`
    // (R3.3). It used to live here alone; a second copy for rivals is precisely the
    // shape that has already produced two defects in this feature.
    ai_monitor::validated_session_id(&reg, started).map(str::to_string)
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
/// **in the same directory**, else whatever the tab's own subtree holds.
///
/// Pure — every syscall arrives through an injected closure, so the fallback order, the
/// single-walk guard and the cwd gate are all table-testable with no tmux, no live tty and no
/// live pid. It returns the chosen **pid** rather than a built `SessionHint` for the same reason:
/// building one would need `start_ms`, an uninjected syscall that would make these tests
/// machine-dependent.
///
/// The cwd gate is the part that prevents a regression rather than adding a feature. Without it,
/// a pane working in a *different* project yields a hint that resolution applies to the **tab
/// shell's** projects directory: the session id names no file there, so control reaches the
/// `since_ms` branch, whose filter is asymmetric (`since - LEAD <= first <= since + SLACK`,
/// `specs/ai-monitor-session-isolation/spec.md` D3). Two-sided is narrower, not sufficient: a pane
/// working elsewhere can still have started within ±120s of a log in the *tab's* project
/// directory — two processes a minute apart is an ordinary morning — and that log comes back
/// `owned: true`, which the overlay then latches. Strict resolution makes such a false
/// confirmation **worse** than it was, not better: it is now the only thing that can reach the
/// panel, there is no ranked fallback below it to soften the error, and a confirmed non-empty
/// result is attached to permanently. So a pane result is admitted **only** on a positive
/// `Some(a) == Some(b)` cwd match; a divergent or unreadable cwd falls through to the tab's own
/// subtree, which is exactly what today's code searches.
fn choose_claude(
    root: u32,
    shell_pid: u32,
    find: impl Fn(u32) -> Option<u32>,
    cwd_of: impl Fn(u32) -> Option<PathBuf>,
) -> Option<u32> {
    // The pane branch: entered only when a pane was genuinely resolved, so a non-tmux tab does
    // not pay a second identical walk.
    if root != shell_pid {
        if let Some(pid) = find(root) {
            // Compare the found `claude`'s cwd, NOT the pane shell's: the pane's `-zsh` moves
            // with every `cd`, while the projects directory that matters is the one `claude`
            // itself writes into.
            if let (Some(a), Some(b)) = (cwd_of(pid), cwd_of(shell_pid)) {
                if a == b {
                    return Some(pid);
                }
            }
            // Divergent or unreadable: fall through. Never `None` — the tab's own subtree is
            // what today's code searches, so this path cannot be worse than today.
        }
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
    if n < size {
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

    /// A process table shaped like the measured one: two mac-terminal tabs each with
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
        // R1.2 — "the pane is the more specific answer and must win when both exist and agree."
        // Anchors the ORDER: a tab-first implementation would return 35303 here.
        let t = ProcTable::parse(BOTH_SAMPLE);
        let same = |_: u32| Some(PathBuf::from("/proj/a"));
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), same),
            Some(74922),
            "the pane's claude, not the tab's own"
        );
    }

    #[test]
    fn a_rejected_pane_falls_through_to_the_tabs_own_claude() {
        // D2(c) — the load-bearing "never worse than today" guarantee: every pid this function
        // could find BEFORE the change, it still finds. A pane whose claude works elsewhere is
        // not admitted, but that must not blind the tab to its own agent — which is exactly
        // what a `return None` in the pane branch would do.
        let t = ProcTable::parse(BOTH_SAMPLE);
        let divergent = |pid: u32| {
            Some(PathBuf::from(if pid == 74922 {
                "/proj/other"
            } else {
                "/proj/a"
            }))
        };
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), divergent),
            Some(35303),
            "falls through to the tab's own claude — never None"
        );
    }

    #[test]
    fn the_pane_wins_when_its_claude_shares_the_tabs_cwd() {
        // R1.2 / R4.4 — the feature working: pane preferred, cwd agrees.
        let t = ProcTable::parse(TMUX_SAMPLE);
        let same = |_: u32| Some(PathBuf::from("/proj/a"));
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), same),
            Some(74922)
        );
    }

    #[test]
    fn a_pane_working_elsewhere_is_not_admitted() {
        // R4.1 — the regression this gate exists to prevent. Without it the pane's started_ms
        // reaches resolution against the TAB's projects dir, whose asymmetric since_ms filter
        // (`since - LEAD <= first <= since + SLACK`) still admits a wrong log that merely began within
        // ±120s of the pane's claude, returning it with owned:true. Under strict resolution that
        // mistake is worse than it was: nothing ranks below it, and the panel attaches for good.
        let t = ProcTable::parse(TMUX_SAMPLE);
        let divergent = |pid: u32| {
            Some(PathBuf::from(if pid == 74922 {
                "/proj/other"
            } else {
                "/proj/a"
            }))
        };
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), divergent),
            None,
            "no claude under the tab's own shell, and the pane's is not admitted"
        );
    }

    #[test]
    fn the_gate_compares_the_found_claude_not_the_pane_shell() {
        // R4.6 — the pane's `-zsh` moves with every `cd`; the directory that matters is the one
        // `claude` writes into. Here the pane SHELL diverges but its `claude` agrees: admit.
        let t = ProcTable::parse(TMUX_SAMPLE);
        let shell_diverges = |pid: u32| {
            Some(PathBuf::from(if pid == 74877 {
                "/proj/elsewhere"
            } else {
                "/proj/a"
            }))
        };
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), shell_diverges),
            Some(74922),
            "the pane shell's cwd is irrelevant; the claude's is what counts"
        );
    }

    #[test]
    fn an_unreadable_cwd_never_counts_as_agreement() {
        // R4.5 — a naive `!=` would conflate "different" with "could not read", and worse,
        // `None == None` would ADMIT exactly the case the gate exists to block.
        let t = ProcTable::parse(TMUX_SAMPLE);
        for cwd_of in [
            (|_: u32| None) as fn(u32) -> Option<PathBuf>,
            |pid: u32| (pid != 74922).then(|| PathBuf::from("/proj/a")),
            |pid: u32| (pid == 74922).then(|| PathBuf::from("/proj/a")),
        ] {
            assert_eq!(
                choose_claude(74877, 34643, |p| t.find_claude(p), cwd_of),
                None,
                "only a positive Some(a) == Some(b) may admit the pane"
            );
        }
    }

    #[test]
    fn the_tab_subtree_is_retried_and_is_never_cwd_gated() {
        // R1.3 — a tab-provenance hit is about the tab's own project by construction, so gating
        // it on the PANE's cwd would suppress a hint today's code returns. The pane here has no
        // agent; the tab's own subtree does.
        let t = ProcTable::parse(
            "34643 34637 /bin/zsh\n\
             35303 34643 claude\n\
             75071 34643 tmux\n\
             90662     1 tmux\n\
             74877 90662 -zsh\n",
        );
        // Even with a cwd reader that reports divergence for everything.
        let divergent = |pid: u32| Some(PathBuf::from(format!("/proj/{pid}")));
        assert_eq!(
            choose_claude(74877, 34643, |p| t.find_claude(p), divergent),
            Some(35303),
            "the tab's own claude is returned, ungated"
        );
    }

    #[test]
    fn nothing_re_rooted_means_exactly_one_walk_and_no_cwd_read() {
        // R1.4 / R4.3 — counting closures are the only way to assert a non-occurrence here.
        let t = ProcTable::parse("1 0 /bin/zsh\n");
        let walks = std::cell::RefCell::new(0usize);
        let cwds = std::cell::RefCell::new(0usize);
        let got = choose_claude(
            7,
            7, // root == shell_pid: the non-tmux case
            |p| {
                *walks.borrow_mut() += 1;
                t.find_claude(p)
            },
            |p| {
                *cwds.borrow_mut() += 1;
                cwd_of_pid(p)
            },
        );
        assert_eq!(got, None);
        assert_eq!(
            *walks.borrow(),
            1,
            "the guard suppressed a second identical walk"
        );
        assert_eq!(*cwds.borrow(), 0, "the cwd gate is inside the pane branch");
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

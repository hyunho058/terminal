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
use std::time::{SystemTime, UNIX_EPOCH};

/// What the tab's live `claude` process says about which session it is running.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionHint {
    /// Named exactly, from `--resume <id>`. When present nothing else is needed.
    pub session_id: Option<String>,
    /// When the process started, epoch millis. A session started by this process has
    /// its first event just after this — the fallback when the id is not on the
    /// command line, and *only* a fallback: it dates a session rather than naming it.
    pub started_ms: Option<u64>,
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
    let claude = table.find_claude(shell_pid)?;
    Some(SessionHint {
        session_id: table.resume_id(claude),
        started_ms: start_ms(claude),
    })
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
fn start_ms(pid: u32) -> Option<u64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    let ms = info
        .pbi_start_tvsec
        .saturating_mul(1_000)
        .saturating_add(info.pbi_start_tvusec / 1_000);
    // A start time in the future is not a usable date to match against — better no
    // hint than a wrong one.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    (ms > 0 && ms <= now).then_some(ms)
}

/// A session id is a UUID, and it becomes a filename — so anything that is not one is
/// rejected rather than joined onto a path.
fn looks_like_session_id(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes().iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_real_lookup_on_a_dead_pid_is_none() {
        assert_eq!(hint_for(u32::MAX), None);
    }
}

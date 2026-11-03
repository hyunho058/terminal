//! Which `presence/<uuid>.lock` a process holds **open** — the FD half of the Antigravity
//! resolution ladder (`ai-monitor-antigravity` AG1/AG11).
//!
//! **A lock file existing proves nothing.** `agy` leaves its presence locks on disk when it
//! exits — the directory held 30 of them for a handful of live sessions when measured — so
//! reading the directory would attach the panel to long-dead conversations. Only an open
//! file descriptor says "this pid is running that conversation right now" (R1.2), and the
//! lock's basename is the conversation id, which makes one FD probe an exact answer rather
//! than a timing heuristic.
//!
//! Lives in `app` rather than `ai-monitor` (AG11): enumerating another process's descriptors
//! needs `proc_pidinfo`/`proc_pidfdinfo`, and `ai-monitor` has neither `libc` nor any
//! subprocess use. `app` already makes these calls in four places (`session.rs`,
//! `tabmeta/proc.rs`), so this adds a flavor, not a new syscall surface. `app` resolves
//! pid → conversation id; `ai-monitor` owns paths, tailing and parsing from there.
//!
//! Read-only: we ask the kernel what `pid` has open. Nothing here opens, creates, locks or
//! writes anything under `~/.gemini/antigravity-cli` — including the lock files themselves.

use std::path::{Path, PathBuf};
use std::ptr;

use libc::{c_int, c_void, proc_fdinfo, PROC_PIDLISTFDS, PROX_FDTYPE_VNODE};

/// `PROC_PIDFDVNODEPATHINFO` from `<sys/proc_info.h>`: the `proc_pidfdinfo` flavor that
/// reports a vnode descriptor's **path**.
///
/// libc 0.2.186 exports `proc_pidfdinfo`, `proc_fdinfo`, `PROC_PIDLISTFDS`,
/// `PROX_FDTYPE_VNODE` and `vnode_info_path` — everything this call needs except this one
/// constant and the struct it fills. Both are declared here rather than left as a bare `2`
/// and a byte blob, which is the same disposition `tabmeta/proc.rs` took for `NODEV`.
const PROC_PIDFDVNODEPATHINFO: c_int = 2;

/// Fixed per-call descriptor budget. A process with more open files than this is not one we
/// need to audit exhaustively — we are looking for one lock, and the sizing call below tells
/// us how many there actually are.
const MAX_FDS: usize = 4096;

/// Descriptors a process may open between the sizing call and the read.
///
/// The two `proc_pidinfo` calls are not atomic with respect to the target process, so asking
/// for exactly the reported count would silently truncate a process that opened a file in
/// between — and the descriptor we want is the one `agy` opened most recently in a fresh
/// session. Over-allocating a few entries costs 8 bytes each.
const FD_SLACK: usize = 32;

/// `struct proc_fileinfo` (`<sys/proc_info.h>`) — the per-descriptor header that precedes
/// the vnode info in [`VnodeFdInfoWithPath`].
///
/// Declared in full rather than as padding so the layout is checkable by eye against the
/// header. None of its fields is read: it exists to place `pvip` at the right offset, which
/// `abi_layout_matches_the_kernel_struct` pins with a size assertion and
/// `probe_finds_a_lock_this_process_holds_open` proves end to end.
#[allow(dead_code)]
#[repr(C)]
struct ProcFileInfo {
    fi_openflags: u32,
    fi_status: u32,
    fi_offset: libc::off_t,
    fi_type: i32,
    fi_guardflags: u32,
}

/// `struct vnode_fdinfowithpath` (`<sys/proc_info.h>`) — what
/// [`PROC_PIDFDVNODEPATHINFO`] fills in.
#[repr(C)]
struct VnodeFdInfoWithPath {
    pfi: ProcFileInfo,
    pvip: libc::vnode_info_path,
}

/// The conversation id of the presence lock `pid` holds open, under the real
/// `~/.gemini/antigravity-cli/presence` (R1.1). `None` when `$HOME` is unset, when the
/// process is gone, when we may not inspect it, or when it holds no lock.
pub(crate) fn conversation_of_pid(pid: u32) -> Option<String> {
    let root = ai_monitor::antigravity::antigravity_root()?;
    conversation_of_pid_in(&ai_monitor::antigravity::presence_dir_in(&root), pid)
}

/// [`conversation_of_pid`] against an explicit presence directory, so the probe is testable
/// against a temp dir instead of the author's real `~/.gemini` — the same root-as-parameter
/// split `ai_monitor::resolve_log_in` and `registered_session_in` use.
pub(crate) fn conversation_of_pid_in(presence_dir: &Path, pid: u32) -> Option<String> {
    // Canonicalize OUR side of the comparison. `proc_pidfdinfo` reports the kernel's fully
    // resolved path, so a directory reached through a symlink comes back as `/private/var/…`
    // while the same path as we hold it reads `/var/…` — the exact trap
    // `ai_monitor::touches_project_log` documents having shipped once already. Resolving the
    // directory once puts both sides in the same spelling; if it cannot be resolved (it does
    // not exist yet) the raw path is still the right thing to compare, and will simply match
    // nothing.
    let dir = std::fs::canonicalize(presence_dir).unwrap_or_else(|_| presence_dir.to_path_buf());
    first_lock_in(&dir, &open_vnode_paths(pid))
}

/// Pure: the first conversation id among `paths` that names a lock **in** `dir` (R1.1).
///
/// Split out so the matching rule is table-testable without a live process. Three conditions,
/// each doing real work: the parent directory pins it to *this* presence directory rather
/// than any file called `<uuid>.lock` elsewhere on the machine; the `.lock` extension and the
/// UUID shape are `ai_monitor`'s, applied there because that crate owns the id vocabulary and
/// the path guard that keeps a hostile basename from becoming a path component.
///
/// "First" is well defined because [`open_vnode_paths`] sorts by descriptor number. A live
/// `agy` holds one presence lock, so the tiebreak is theoretical — but an unspecified order
/// would make the panel's answer differ run to run if that ever stopped being true, which is
/// the same reasoning `tabmeta::proc::parse_childpids` sorts for.
fn first_lock_in(dir: &Path, paths: &[PathBuf]) -> Option<String> {
    paths
        .iter()
        .filter(|p| p.parent() == Some(dir))
        .find_map(|p| ai_monitor::antigravity::conversation_id_from_lock(p))
}

/// Every vnode (regular file / directory) path `pid` currently holds open, by ascending
/// descriptor number. Empty on any failure — a dead pid, a process we may not inspect, or a
/// short read.
fn open_vnode_paths(pid: u32) -> Vec<PathBuf> {
    let entry = std::mem::size_of::<proc_fdinfo>();
    // Sizing call: a NULL buffer asks how many BYTES the listing needs.
    let needed =
        unsafe { libc::proc_pidinfo(pid as c_int, PROC_PIDLISTFDS, 0, ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let cap = (needed as usize / entry)
        .saturating_add(FD_SLACK)
        .min(MAX_FDS);
    let mut buf = vec![
        proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0,
        };
        cap
    ];
    let got = unsafe {
        libc::proc_pidinfo(
            pid as c_int,
            PROC_PIDLISTFDS,
            0,
            buf.as_mut_ptr().cast::<c_void>(),
            (cap * entry) as c_int,
        )
    };
    if got <= 0 {
        return Vec::new();
    }
    // `got` is a BYTE count here (unlike `proc_listchildpids`, whose libproc wrapper divides
    // by the entry size for you), and it is clamped to what we actually allocated — a process
    // that grew its table past `cap` between the two calls would otherwise walk off the end.
    let n = (got as usize / entry).min(cap);
    let mut fds: Vec<i32> = buf[..n]
        .iter()
        .filter(|e| e.proc_fdtype == PROX_FDTYPE_VNODE as u32)
        .map(|e| e.proc_fd)
        .collect();
    fds.sort_unstable();
    fds.into_iter()
        .filter_map(|fd| vnode_path(pid, fd))
        .collect()
}

/// The filesystem path behind one of `pid`'s vnode descriptors, via
/// `proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO)`. `None` on any failure or an empty path.
fn vnode_path(pid: u32, fd: i32) -> Option<PathBuf> {
    let mut info: VnodeFdInfoWithPath = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<VnodeFdInfoWithPath>() as c_int;
    let n = unsafe {
        libc::proc_pidfdinfo(
            pid as c_int,
            fd,
            PROC_PIDFDVNODEPATHINFO,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if n < size {
        return None; // error, or the fd closed under us between the listing and now
    }
    // vip_path is a fixed 1024-byte, NUL-terminated C path — read exactly as
    // `session.rs`/`tabmeta::proc` read `pvi_cdir.vip_path`.
    let raw = &info.pvip.vip_path;
    let bytes = unsafe {
        std::slice::from_raw_parts(raw.as_ptr() as *const u8, std::mem::size_of_val(raw))
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&bytes[..end]).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELD: &str = "4de5fc70-2d8a-44c2-8e25-a6901c93fb86";
    const STALE: &str = "ed14342c-1111-4222-8333-a44444444444";

    /// A private temp directory, removed and recreated per test — the same shape
    /// `ai_monitor`'s in-file `temp_dir` uses.
    fn temp_presence(tag: &str) -> PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!("agy-presence-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("presence");
        std::fs::create_dir_all(&dir).expect("temp presence dir");
        dir
    }

    #[test]
    fn abi_layout_matches_the_kernel_struct() {
        // The one thing a hand-rolled `#[repr(C)]` can get silently wrong is an offset, and a
        // wrong offset here yields a plausible-looking garbage path rather than an error. Sizes
        // from <sys/proc_info.h> on arm64: proc_fileinfo is 24 (two u32, an 8-aligned off_t, an
        // i32 and a u32), vnode_info_path is 152 + MAXPATHLEN.
        assert_eq!(std::mem::size_of::<ProcFileInfo>(), 24);
        assert_eq!(std::mem::size_of::<libc::vnode_info_path>(), 152 + 1024);
        assert_eq!(std::mem::size_of::<VnodeFdInfoWithPath>(), 24 + 152 + 1024);
    }

    #[test]
    fn probe_finds_a_lock_this_process_holds_open() {
        // R1.1 end to end against a REAL descriptor: the test process plays `agy`, holding a
        // presence lock open while it probes its own pid. This is what validates the FFI —
        // a wrong flavor constant, offset or size cannot produce this id by accident.
        let dir = temp_presence("held");
        let held = dir.join(format!("{HELD}.lock"));
        std::fs::write(&held, b"").expect("write lock");
        let _open = std::fs::File::open(&held).expect("hold the lock open");

        assert_eq!(
            conversation_of_pid_in(&dir, std::process::id()),
            Some(HELD.to_string()),
            "an open descriptor on presence/<uuid>.lock must resolve <uuid>"
        );
    }

    #[test]
    fn a_lock_file_nobody_holds_is_not_resolved() {
        // R1.2 — the whole reason this probe exists. Stale locks outlive their sessions (30
        // files for a handful of live sessions, when measured), so existence on disk must
        // never resolve. Both shapes are covered: a directory holding only a stale lock
        // resolves nothing, and a directory holding one of each resolves only the held one.
        let dir = temp_presence("stale");
        std::fs::write(dir.join(format!("{STALE}.lock")), b"").expect("write stale lock");

        assert_eq!(
            conversation_of_pid_in(&dir, std::process::id()),
            None,
            "a lock file on disk with no holder is not a session"
        );

        let held = dir.join(format!("{HELD}.lock"));
        std::fs::write(&held, b"").expect("write held lock");
        let _open = std::fs::File::open(&held).expect("hold the lock open");
        assert_eq!(
            conversation_of_pid_in(&dir, std::process::id()),
            Some(HELD.to_string()),
            "the held lock wins over the stale one beside it"
        );
    }

    #[test]
    fn a_dead_pid_yields_nothing_rather_than_panicking() {
        // The failure shapes the overlay will actually hit: the tab's agy exited between the
        // process walk and the probe, or never existed.
        let dir = temp_presence("dead");
        assert_eq!(conversation_of_pid_in(&dir, 0), None);
        assert_eq!(conversation_of_pid_in(&dir, u32::MAX), None);
    }

    #[test]
    fn a_missing_presence_directory_is_absent_not_an_error() {
        // R1.5/R1.6 — a machine that has never run `agy` has no presence directory at all.
        // `canonicalize` fails there, and the raw path must simply match nothing.
        let missing = std::env::temp_dir().join("agy-presence-does-not-exist/presence");
        assert_eq!(conversation_of_pid_in(&missing, std::process::id()), None);
    }

    #[test]
    fn only_uuid_named_locks_inside_the_presence_directory_match() {
        // The pure matching rule, without a live process. Each rejection is a real path the
        // probe can see: `agy` holds its cwd and its transcript open too, and a traversal
        // string is a perfectly good filename.
        let dir = Path::new("/tmp/agy/presence");
        let ok = dir.join(format!("{HELD}.lock"));
        assert_eq!(
            first_lock_in(dir, std::slice::from_ref(&ok)),
            Some(HELD.to_string())
        );

        for rejected in [
            dir.join(format!("{HELD}.json")),     // not a lock
            dir.join("not-a-uuid.lock"),          // fails the path guard
            dir.join("../../../etc/passwd.lock"), // traversal: guard rejects the stem
            Path::new("/tmp/agy/other").join(format!("{HELD}.lock")), // wrong directory
            PathBuf::from("/tmp/agy/presence"),   // the directory's own fd
        ] {
            assert_eq!(
                first_lock_in(dir, std::slice::from_ref(&rejected)),
                None,
                "{rejected:?} must not resolve a conversation"
            );
        }

        // A real listing mixes them; the one valid lock is still found.
        assert_eq!(
            first_lock_in(
                dir,
                &[
                    PathBuf::from("/tmp/agy/presence"),
                    dir.join("not-a-uuid.lock"),
                    ok,
                ]
            ),
            Some(HELD.to_string())
        );
    }

    #[test]
    fn this_process_has_at_least_its_own_open_files_listed() {
        // Proves the listing half is live rather than silently returning empty — without
        // which every assertion above would pass on a probe that never worked.
        let dir = temp_presence("listing");
        let f = dir.join("held.txt");
        std::fs::write(&f, b"").expect("write");
        let _open = std::fs::File::open(&f).expect("open");
        let canon = std::fs::canonicalize(&f).expect("canonicalize");
        assert!(
            open_vnode_paths(std::process::id()).contains(&canon),
            "a file this process holds open must appear in its own fd listing"
        );
    }
}

/// R0.1's composition, run against **live agent processes** (G9).
///
/// Every other test in this feature drives a piece: `agent_of_pid` over a synthetic table,
/// `first_lock_in` over a temp directory, `antigravity::resolve_in` over a mock root. G9 names
/// what none of them reach — `agent_for` and the non-`_in` `conversation_of_pid` composed end
/// to end, on real processes, where a wrong `$HOME`, an unreadable descriptor or a canonical
/// `/private/var` spelling would each break the chain invisibly.
///
/// `#[ignore]` because it needs a live `agy` and a live `claude` on the machine, which CI has
/// not got. It is a **manual gate**, run deliberately:
///
/// ```text
/// cargo test --bin crewterm r0_1_live -- --ignored --nocapture
/// ```
///
/// It cannot close G9 by itself: focus switching runs through `overlay::open` and a winit
/// event loop, and no test process can press Cmd+Shift+M. What it does close is the half that
/// does not need a human — that the two agents are told apart and resolve to *different*
/// logs, which is the "Claude telemetry is not shown against the Antigravity session" half of
/// R0.1's Then.
#[cfg(test)]
#[test]
#[ignore = "needs a live agy and a live claude; run manually"]
fn r0_1_live_agents_are_told_apart_and_resolve_to_different_logs() {
    use crate::tabmeta::AgentKind;

    // Find one live process of each kind by asking the same classifier the panel uses, so
    // this cannot pass against a `pgrep` that agrees with nothing in the product.
    let ps = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,args="])
        .output()
        .expect("ps");
    let text = String::from_utf8_lossy(&ps.stdout);
    let mut found: Vec<(AgentKind, u32, u32)> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        // Ask through the PARENT, which is what a tab actually holds: the overlay knows a
        // shell pid and walks down. Probing the agent pid directly would skip `agent_for`'s
        // tree walk — the very step this test exists to exercise.
        if let Some((kind, agent_pid)) = crate::tabsession::agent_for(ppid) {
            if agent_pid == pid && !found.iter().any(|(k, _, _)| *k == kind) {
                found.push((kind, agent_pid, ppid));
            }
        }
    }

    let agy = found.iter().find(|(k, _, _)| *k == AgentKind::Antigravity);
    let claude = found.iter().find(|(k, _, _)| *k == AgentKind::Claude);
    let (Some(&(_, agy_pid, agy_shell)), Some(&(_, claude_pid, claude_shell))) = (agy, claude)
    else {
        panic!(
            "this gate needs one live `agy` AND one live `claude`, each under a shell. \
             Classified this run: {found:?}. Start the missing one in a tab and re-run."
        );
    };
    println!("agy   pid={agy_pid} under shell {agy_shell}");
    println!("claude pid={claude_pid} under shell {claude_shell}");

    // 1. The routing half of R0.1: two live agents, told apart by the same call the overlay
    //    makes, from the shell pid a tab actually has.
    assert_ne!(
        agy_pid, claude_pid,
        "two distinct agents must not resolve to one process"
    );

    // 2. The FD probe, against a descriptor a real `agy` is holding right now — the
    //    non-`_in` path, with the real `$HOME` and the real canonicalization.
    let conversation = conversation_of_pid(agy_pid).unwrap_or_else(|| {
        panic!(
            "live agy pid {agy_pid} named no conversation. If it started seconds ago its \
             presence lock may not exist yet (~7s) — re-run. Otherwise this is R1.1 failing."
        )
    });
    println!("conversation = {conversation}");

    // 3. Resolution through the probed id, to a transcript that is really on disk.
    let cwd = std::path::PathBuf::from(
        std::env::var("HOME").expect("HOME"), // any real dir: a probed id ignores cwd
    );
    let resolved = ai_monitor::antigravity::resolve(
        ai_monitor::antigravity::Probed::Conversation(&conversation),
        &cwd,
    )
    .expect("a probed conversation resolves its transcript");
    assert!(
        resolved.path.is_file(),
        "resolved transcript must exist: {}",
        resolved.path.display()
    );
    assert!(resolved.owned, "a probed conversation is owned");
    assert!(
        resolved.path.to_string_lossy().contains(&conversation),
        "the transcript must be the probed conversation's own: {}",
        resolved.path.display()
    );
    println!("transcript = {}", resolved.path.display());

    // 4. The "not shown against" half: the Claude tab's log is a DIFFERENT file. If these
    //    ever coincided, focusing one tab would show the other's telemetry — which is the
    //    failure R0.1's Then is written to exclude.
    let claude_hint = crate::tabsession::hint_for(claude_shell);
    let claude_dir = claude_hint
        .as_ref()
        .and_then(|h| h.cwd.as_ref())
        .and_then(|c| ai_monitor::project_dir(c.path()));
    println!("claude log dir = {claude_dir:?}");
    if let Some(dir) = claude_dir.as_deref() {
        assert_ne!(
            Some(dir),
            resolved.path.parent(),
            "the Claude tab and the Antigravity tab must not share a log directory"
        );
        assert!(
            dir.starts_with(std::env::var("HOME").expect("HOME")),
            "the Claude directory resolves under $HOME/.claude, not into ~/.gemini: {}",
            dir.display()
        );
    }
}

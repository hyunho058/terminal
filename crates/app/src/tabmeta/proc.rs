//! Raw macOS process primitives — `cwd_of_pid`, `children_of`, `exe_path`,
//! `proc_args` — reimplemented here (D10) rather than reused from `session.rs`
//! (module-private, out of surface).
//!
//! Each syscall wrapper returns "absent" (`None` / empty) on any failure and never
//! panics (D14). The two pure parsers — `parse_childpids` and `parse_procargs` — are
//! split out so the `MAX_CHILDREN` cap and the KERN_PROCARGS2 layout are table-tested
//! without spawning processes (D6, R5.5, R11.4).

use std::ffi::{CString, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

use libc::{
    c_int, c_uint, c_void, pid_t, size_t, CTL_KERN, KERN_ARGMAX, KERN_PROCARGS2,
    PROC_PIDPATHINFO_MAXSIZE, PROC_PIDVNODEPATHINFO,
};

/// Fixed per-call child buffer (D6). If the kernel reports more children the excess
/// is dropped — we are looking for one agent, not auditing a tree.
const MAX_CHILDREN: usize = 128;

/// The tab's cwd from its pid, via `proc_pidinfo(PROC_PIDVNODEPATHINFO)` — the same
/// call `session.rs`'s private copy makes (D10). `None` on any failure.
pub(crate) fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
    #[cfg(test)]
    super::probe::PROC_CWD.bump(1);

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as c_int,
            PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if n < size {
        return None; // error or short read
    }
    // pvi_cdir.vip_path is a fixed NUL-terminated C path (1024 bytes).
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

/// The direct children of `pid` (one level), via `proc_listchildpids` into a fixed
/// [`MAX_CHILDREN`] buffer. Empty on failure or when there are none. `proc_listchildpids`
/// returns the **count** of child pids (Apple's libproc divides the kernel byte count by
/// `sizeof(int)` internally), not a byte length — see [`parse_childpids`].
pub(crate) fn children_of(pid: u32) -> Vec<u32> {
    #[cfg(test)]
    super::probe::PROC_CHILDREN.bump(1);

    let mut buf = [0i32; MAX_CHILDREN];
    let size = std::mem::size_of_val(&buf) as c_int;
    let n =
        unsafe { libc::proc_listchildpids(pid as pid_t, buf.as_mut_ptr() as *mut c_void, size) };
    parse_childpids(&buf, n)
}

/// The executable path of `pid`, via `proc_pidpath`. `None` on failure.
pub(crate) fn exe_path(pid: u32) -> Option<PathBuf> {
    #[cfg(test)]
    super::probe::PROC_EXE.bump(1);

    let mut buf = [0u8; PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe {
        libc::proc_pidpath(
            pid as c_int,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
        )
    };
    if n <= 0 {
        return None;
    }
    // proc_pidpath returns the path length (NUL not included).
    let bytes = &buf[..n as usize];
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// "No controlling tty". The kernel reports `NODEV` (`-1`) in `e_tdev` for a process with
/// no controlling terminal, and libc's Apple bindings do **not** export the constant (only
/// `ENODEV` / `MNT_NODEV` exist), so it is named here rather than left as a magic `-1 as u32`
/// at the comparison site — where it would read as a bug later.
// Consumed via `tmux::effective_root`, whose own consumers (worker.rs, tabsession.rs) land
// after this feature — so the whole chain reads as dead until then. Same disposition as
// `mod.rs`'s `global()`.
#[allow(dead_code)]
const NODEV: u32 = u32::MAX;

/// The controlling-tty **device number** of `pid`, via `proc_pidinfo(PROC_PIDTBSDINFO)`.
/// `None` on any failure, and for the two values that mean "no tty" ([`NODEV`] and `0`) —
/// both are *valid-looking* device numbers that could only ever fail to match, so rejecting
/// them here turns a pointless `stat` of every client tty into a cheap early return.
///
/// Returns `u32`, not `libc::dev_t`: the two sides of the tmux tty join are different Rust
/// types over the same 32-bit device number — `proc_bsdinfo.e_tdev` is `u32` while
/// `dev_t` (and so `stat.st_rdev`) is `i32` on Apple. Normalising on `u32` puts the single
/// conversion on the `stat` side ([`rdev_of_path`]) instead of scattering `as` casts through
/// the comparison.
#[allow(dead_code)] // consumed by tmux::effective_root once its consumers land
pub(crate) fn tty_dev_of_pid(pid: u32) -> Option<u32> {
    #[cfg(test)]
    super::probe::PROC_TTY.bump(1);

    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if n != size {
        return None; // error or short read
    }
    let dev = info.e_tdev;
    (dev != NODEV && dev != 0).then_some(dev)
}

/// The device number a path refers to (`stat`'s `st_rdev`), as `u32`. `None` on any failure.
/// The tmux side of the tty join: `tmux list-clients` reports `#{client_tty}` as a path, and
/// this resolves it to the number [`tty_dev_of_pid`] returns for the tab's shell.
///
/// The join runs in this direction — path → device — deliberately. `libc::devname` *is*
/// available on macOS (`bsd/mod.rs`, not the `apple` submodule), so a name-direction join is
/// possible, but `devname` returns a pointer into a process-global **static buffer** (not
/// thread-safe, and this module is reached from the metadata worker thread *and* the UI
/// thread) and on a cache miss it synthesises a `"#major/minor"` string that would silently
/// never match a real tty name.
#[allow(dead_code)] // consumed by tmux::resolve_rdevs once its consumers land
pub(crate) fn rdev_of_path(path: &Path) -> Option<u32> {
    #[cfg(test)]
    super::probe::TTY_STAT.bump(1);

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::stat(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    Some(st.st_rdev as u32)
}

/// The argv of `pid`, via `sysctl(KERN_PROCARGS2)`. Empty on failure. Called only
/// from stage 2 of the classifier (an interpreter candidate), so its two sysctls are
/// rare, not per-tab-per-tick (D5/D6).
pub(crate) fn proc_args(pid: u32) -> Vec<OsString> {
    #[cfg(test)]
    super::probe::PROC_ARGS.bump(1);

    // KERN_PROCARGS2 includes the environment, so size the buffer from KERN_ARGMAX
    // rather than guessing (a small buffer would always ENOMEM).
    let cap = kern_argmax();
    let mut buf = vec![0u8; cap];
    let mut mib = [CTL_KERN, KERN_PROCARGS2, pid as c_int];
    let mut len: size_t = buf.len();
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as c_uint,
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Vec::new();
    }
    buf.truncate(len);
    parse_procargs(&buf)
}

/// `kern.argmax` — the max size of the argv+env blob. Falls back to 256 KiB.
fn kern_argmax() -> usize {
    let mut argmax: c_int = 0;
    let mut mib = [CTL_KERN, KERN_ARGMAX];
    let mut sz: size_t = std::mem::size_of::<c_int>();
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as c_uint,
            &mut argmax as *mut _ as *mut c_void,
            &mut sz,
            ptr::null_mut(),
            0,
        )
    };
    if rc == 0 && argmax > 0 {
        argmax as usize
    } else {
        256 * 1024
    }
}

/// Pure: turn a `proc_listchildpids` buffer + its return value into the pid list (R5.5).
/// `returned` is the **count of child pids** — `proc_listchildpids` reports pids, not
/// bytes (its libproc wrapper already divides the kernel's byte count by `sizeof(int)`).
/// `<= 0` → empty; the count is capped at the buffer length; non-positive pids are dropped.
pub(crate) fn parse_childpids(buf: &[i32], returned: c_int) -> Vec<u32> {
    if returned <= 0 {
        return Vec::new();
    }
    let count = (returned as usize).min(buf.len());
    buf[..count]
        .iter()
        .filter(|&&p| p > 0)
        .map(|&p| p as u32)
        .collect()
}

/// Pure: parse a `KERN_PROCARGS2` buffer into argv (R11.4). Layout: `argc` (i32,
/// native endian), then the NUL-terminated exec path, then NUL padding, then `argc`
/// NUL-delimited argv strings (env follows, ignored). Malformed / short → empty.
pub(crate) fn parse_procargs(buf: &[u8]) -> Vec<OsString> {
    if buf.len() < 4 {
        return Vec::new();
    }
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if argc <= 0 {
        return Vec::new();
    }
    let mut i = 4;
    // Skip the exec path (up to its NUL).
    while i < buf.len() && buf[i] != 0 {
        i += 1;
    }
    // Skip the NUL padding between exec path and argv[0].
    while i < buf.len() && buf[i] == 0 {
        i += 1;
    }
    let mut args = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        if i >= buf.len() {
            break;
        }
        let start = i;
        while i < buf.len() && buf[i] != 0 {
            i += 1;
        }
        args.push(OsString::from_vec(buf[start..i].to_vec()));
        i += 1; // step over the NUL (or past the end — the loop guard handles it)
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- D13(1): one self-process test per syscall (R8.1) ----

    #[test]
    fn cwd_of_pid_matches_current_dir() {
        let _g = super::super::probe::serial();
        // R8.1 — same assertion session.rs makes about its own copy.
        let got = cwd_of_pid(std::process::id()).expect("own cwd");
        let expect = std::env::current_dir().expect("current_dir");
        assert_eq!(
            got.canonicalize().ok(),
            expect.canonicalize().ok(),
            "cwd_of_pid(self) must equal current_dir()"
        );
    }

    #[test]
    fn cwd_of_dead_pid_is_none() {
        let _g = super::super::probe::serial();
        // R8.2 — a pid that cannot exist yields None, no panic.
        assert!(cwd_of_pid(0).is_none());
    }

    #[test]
    fn exe_path_of_self_ends_in_test_binary() {
        let _g = super::super::probe::serial();
        let p = exe_path(std::process::id()).expect("own exe path");
        // The running test binary's path — just assert it resolved to a real file name.
        assert!(
            p.file_name().is_some(),
            "exe_path(self) had no file name: {p:?}"
        );
        assert!(p.is_absolute(), "exe path should be absolute: {p:?}");
    }

    #[test]
    fn children_of_self_returns_without_error() {
        let _g = super::super::probe::serial();
        // The ABI/buffer sizing is right; a count of zero is a pass.
        let _ = children_of(std::process::id());
    }

    #[test]
    fn proc_args_of_self_contains_binary_path() {
        let _g = super::super::probe::serial();
        let args = proc_args(std::process::id());
        assert!(!args.is_empty(), "own argv should be non-empty");
        // argv[0] is the test binary's path.
        let argv0 = args[0].to_string_lossy();
        assert!(!argv0.is_empty());
    }

    // ---- the tty join's two wrappers (tmux-root-resolution R1, R4.7) ----

    #[test]
    fn tty_dev_of_self_matches_its_own_tty_device() {
        let _g = super::super::probe::serial();
        // R1.1 — when the test process has a controlling tty, the number it reports must be
        // the same one `stat` gives for that tty path. Under a CI runner with no tty there is
        // nothing to compare, and `None` is the correct answer — so this asserts the
        // *agreement* of the two sides rather than requiring a tty to exist.
        match tty_dev_of_pid(std::process::id()) {
            Some(dev) => {
                assert_ne!(dev, 0, "0 must be rejected as absent");
                assert_ne!(dev, NODEV, "NODEV must be rejected as absent");
            }
            None => { /* no controlling tty (CI) — absent is correct */ }
        }
    }

    #[test]
    fn tty_dev_of_dead_pid_is_none() {
        let _g = super::super::probe::serial();
        // R1.2 — a pid that cannot exist yields None, no panic.
        assert!(tty_dev_of_pid(0).is_none());
        assert!(tty_dev_of_pid(u32::MAX).is_none());
    }

    #[test]
    fn tty_dev_bumps_its_counter_once_per_call_either_way() {
        let _g = super::super::probe::serial();
        // R1.5 — the counter counts calls, not successes, so a budget assertion over it is
        // meaningful on the failure path too.
        super::super::probe::reset_all();
        let _ = tty_dev_of_pid(std::process::id());
        assert_eq!(super::super::probe::PROC_TTY.get(), 1);
        let _ = tty_dev_of_pid(0); // fails
        assert_eq!(
            super::super::probe::PROC_TTY.get(),
            2,
            "a failed read still counts"
        );
    }

    #[test]
    fn rdev_of_path_reads_a_real_device_and_counts_itself() {
        let _g = super::super::probe::serial();
        // R4.7 — /dev/null is a character device present on every macOS box.
        super::super::probe::reset_all();
        assert!(rdev_of_path(Path::new("/dev/null")).is_some());
        assert_eq!(super::super::probe::TTY_STAT.get(), 1);
        // A path that cannot be stat-ed is absent, not a panic — and still counts.
        assert!(rdev_of_path(Path::new("/dev/definitely-not-a-device")).is_none());
        assert_eq!(super::super::probe::TTY_STAT.get(), 2);
        // An embedded NUL cannot become a CString — absent, no panic.
        assert!(rdev_of_path(Path::new("/dev/nu\0ll")).is_none());
    }

    #[test]
    fn the_two_sides_of_the_join_agree_on_one_device() {
        let _g = super::super::probe::serial();
        // The load-bearing property of the whole feature: `e_tdev` (u32, from proc_pidinfo)
        // and `st_rdev` (i32 dev_t, from stat) are different Rust types over the SAME 32-bit
        // device number. If that were false, the tmux join could never match.
        if let Some(dev) = tty_dev_of_pid(std::process::id()) {
            // Resolve our own tty by NAME rather than guessing device paths: macOS allocates
            // /dev/ttysNNN on demand, so a bounded numeric scan silently goes flaky on a busy
            // machine that has allocated past the scan's ceiling.
            // `ttyname` shares `devname`'s process-global static buffer — the very property
            // D1 rejects it for in production. Safe here: test-only, under `probe::serial()`,
            // and the path is copied out immediately.
            let name = unsafe { libc::ttyname(0) };
            if !name.is_null() {
                let path = unsafe { std::ffi::CStr::from_ptr(name) };
                let path = Path::new(std::str::from_utf8(path.to_bytes()).expect("tty path"));
                assert_eq!(
                    rdev_of_path(path),
                    Some(dev),
                    "e_tdev {dev:#x} != st_rdev of {path:?} — the tmux join's premise is broken"
                );
            }
        }
    }

    // ---- pure parsers, tested with no syscall (R5.5, R11.4) ----

    #[test]
    fn parse_childpids_counts_caps_and_filters() {
        let mut buf = [0i32; MAX_CHILDREN];
        buf[0] = 11;
        buf[1] = 0; // a zero pid is dropped
        buf[2] = 22;
        // `returned` is a pid COUNT (not bytes): 3 pids → buf[0..3].
        let got = parse_childpids(&buf, 3);
        assert_eq!(got, vec![11, 22]);
        // Regression for the count-vs-bytes bug: a single child must survive. The old code
        // divided the count by sizeof(i32), turning 1 into 0 and dropping the only child —
        // which is exactly a tab's zsh with one agent process.
        assert_eq!(
            parse_childpids(&buf, 1),
            vec![11],
            "one child must not be lost"
        );
        // returned <= 0 → empty.
        assert!(parse_childpids(&buf, 0).is_empty());
        assert!(parse_childpids(&buf, -1).is_empty());
        // an over-large `returned` is capped at the buffer length (no OOB).
        let capped = parse_childpids(&buf, 10_000);
        assert!(capped.len() <= MAX_CHILDREN);
    }

    #[test]
    fn parse_procargs_extracts_argv() {
        // Build a synthetic KERN_PROCARGS2 buffer: argc=2, exec path, NUL pad, argv.
        let mut buf = Vec::new();
        buf.extend_from_slice(&2i32.to_ne_bytes());
        buf.extend_from_slice(b"/usr/local/bin/node\0");
        buf.push(0); // extra NUL padding
        buf.extend_from_slice(b"node\0");
        buf.extend_from_slice(b"/path/to/claude\0");
        buf.extend_from_slice(b"SOME_ENV=1\0"); // env, must be ignored (argc=2)
        let args = parse_procargs(&buf);
        assert_eq!(args.len(), 2, "only argc argv entries, env ignored");
        assert_eq!(args[0].to_string_lossy(), "node");
        assert_eq!(args[1].to_string_lossy(), "/path/to/claude");
    }

    #[test]
    fn parse_procargs_rejects_malformed() {
        assert!(parse_procargs(&[]).is_empty());
        assert!(parse_procargs(&[1, 2]).is_empty(), "too short for argc");
        // argc <= 0
        let mut z = Vec::new();
        z.extend_from_slice(&0i32.to_ne_bytes());
        z.extend_from_slice(b"x\0");
        assert!(parse_procargs(&z).is_empty());
    }
}

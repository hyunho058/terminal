//! Raw macOS process primitives — `cwd_of_pid`, `children_of`, `exe_path`,
//! `proc_args` — reimplemented here (D10) rather than reused from `session.rs`
//! (module-private, out of surface).
//!
//! Each syscall wrapper returns "absent" (`None` / empty) on any failure and never
//! panics (D14). The two pure parsers — `parse_childpids` and `parse_procargs` — are
//! split out so the `MAX_CHILDREN` cap and the KERN_PROCARGS2 layout are table-tested
//! without spawning processes (D6, R5.5, R11.4).

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
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

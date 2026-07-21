//! Bounded tail read of a session JSONL — the raw activity signal (D7 of the
//! `tab-metadata` feature). Additive to `ai-monitor`: it uses only `std` +
//! `serde_json` (via the existing [`parse_line`](crate::parse_line)), changes no
//! existing function, and leaves `Cargo.toml` untouched (R6.9).
//!
//! [`backfill`](crate::backfill) reads the *whole* file and is unusable on a poll loop;
//! this reads at most the final [`TAIL_CAP`] bytes and returns only the role of the last
//! parseable line, plus the file's `(mtime, len)` so the caller can memoise on them.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

use crate::parse_line;

/// The most we ever read from a log's tail.
pub const TAIL_CAP: u64 = 8 * 1024;

/// The role of the last complete line of a log, plus the facts the caller memoises on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailState {
    pub role: String,
    pub mtime: SystemTime,
    pub len: u64,
}

/// The role of the LAST parseable line in `log`, plus the file's `mtime` and `len`.
/// Reads at most the final [`TAIL_CAP`] bytes. `None` if the file is missing, empty, or
/// has no parseable line in that window. Never panics (D14).
pub fn tail_state(log: &Path) -> Option<TailState> {
    let mut file = std::fs::File::open(log).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    if len == 0 {
        return None;
    }
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    // Seek to the last TAIL_CAP bytes and read forward — a genuine cap, so a 200 KiB log
    // costs 8 KiB of I/O (R6.1).
    let start = len.saturating_sub(TAIL_CAP);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_CAP).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);

    // The last parseable line. Scanning from the end skips a half-written final line and
    // the partial first line left by seeking into the middle of a record.
    let role = text.lines().rev().find_map(parse_line)?.role;
    Some(TailState { role, mtime, len })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpfile(tag: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aimon-activity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let p = dir.join(format!("{tag}.jsonl"));
        std::fs::write(&p, bytes).expect("write");
        p
    }

    #[test]
    fn missing_or_empty_is_none() {
        assert!(tail_state(Path::new("/no/such/log.jsonl")).is_none());
        let empty = tmpfile("empty", b"");
        assert!(tail_state(&empty).is_none());
    }

    #[test]
    fn reads_only_the_tail_of_a_large_log() {
        // R6.1 — the head parses one way, the tail another; the tail must win.
        let mut bytes = Vec::new();
        // ~192 KiB of "user" lines up front …
        let head = br#"{"type":"user","message":{"role":"user"}}"#;
        while bytes.len() < 192 * 1024 {
            bytes.extend_from_slice(head);
            bytes.push(b'\n');
        }
        // … then a final assistant line.
        bytes.extend_from_slice(br#"{"type":"assistant","message":{"role":"assistant"}}"#);
        bytes.push(b'\n');
        let log = tmpfile("big", &bytes);
        let ts = tail_state(&log).expect("tail");
        assert_eq!(ts.role, "assistant", "the tail line's role, not the head's");
        assert_eq!(ts.len, bytes.len() as u64);
    }

    #[test]
    fn ignores_a_half_written_final_line() {
        // A trailing partial (unterminated) JSON line is skipped for the last complete one.
        let body = b"{\"type\":\"assistant\"}\n{\"type\":\"user\",\"incomplete\":";
        let log = tmpfile("partial", body);
        let ts = tail_state(&log).expect("tail");
        assert_eq!(ts.role, "assistant");
    }
}

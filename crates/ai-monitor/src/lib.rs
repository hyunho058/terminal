//! `ai-monitor` — headless reader of the active session's AI (Claude Code) JSONL
//! logs (D1). UI/GPU-free (SD2): it locates the log, backfills the recent tail,
//! watches for appends, and parses each line into an [`AiEvent`]. The overlay
//! that renders these lives in the app/renderer (SD3/SD4); this crate only reads.
//!
//! Read-only (C3): logs are opened for reading, never written.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

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

/// The most-recently-modified `*.jsonl` in `dir`, if any.
fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
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
    pub rx: Receiver<Arc<TelemetrySnapshot>>,
}

impl LogWatcher {
    /// Start tailing `log` from `start_offset`, folding each newly-appended complete
    /// line into `telemetry` (seed it with [`backfill_telemetry`]).
    ///
    /// Sends ONE `Arc<TelemetrySnapshot>` per append batch and calls `wake` once, so
    /// the caller can wake its event loop and drain. `wake` runs on the watch thread
    /// — keep it cheap (e.g. an `EventLoopProxy::send_event`).
    pub fn spawn(
        log: PathBuf,
        mut telemetry: SessionTelemetry,
        start_offset: u64,
        wake: impl Fn() + Send + 'static,
    ) -> Option<LogWatcher> {
        let dir = log.parent()?.to_path_buf();
        let (tx, rx) = mpsc::channel::<Arc<TelemetrySnapshot>>();
        let mut offset = start_offset;
        let mut partial = String::new();
        let target = log.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let name = target.file_name();
            if !ev.paths.iter().any(|p| p.file_name() == name) {
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
            // truncates to zero first, so the callback fires, the state resets, the
            // read yields no complete line — and under a "≥1 line" condition no
            // snapshot would be sent, leaving the UI showing the PREVIOUS session's
            // totals as though live. Bytes read with no complete line (a buffered
            // partial) correctly send nothing: no state changed.
            if (folded > 0 || reset) && tx.send(Arc::new(telemetry.snapshot())).is_ok() {
                wake();
            }
        })
        .ok()?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
        Some(LogWatcher {
            _watcher: watcher,
            rx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let w = LogWatcher::spawn(log.clone(), t, off, || {}).expect("spawn");

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
        assert_eq!(s1.total_output, 10);

        append(&usage("m2", 5));
        let s2 = w.rx.recv_timeout(Duration::from_secs(10)).expect("second");
        assert_eq!(
            s2.total_output, 15,
            "cumulative, and m1 counted exactly once"
        );

        // Nothing re-emitted: a stale-size offset would re-read and re-fold m1/m2.
        assert!(
            w.rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "no duplicate snapshots"
        );
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
        let w = LogWatcher::spawn(log.clone(), t, off, || {}).expect("spawn");

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
        assert_eq!(before.total_output, 99);

        // Truncate: a new session in the same path.
        std::fs::write(&log, "").unwrap();
        let after =
            w.rx.recv_timeout(Duration::from_secs(10))
                .expect("a reset must emit even with no line to fold");
        assert_eq!(after.total_output, 0, "previous session's totals dropped");

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
        assert_eq!(fresh.total_output, 4, "only the new session's tokens");
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
        let w = LogWatcher::spawn(log.clone(), t, off, || {}).expect("spawn");

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
        assert_eq!(s.total_output, 7);
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

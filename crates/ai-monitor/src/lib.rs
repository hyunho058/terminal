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
pub fn most_recent_session(dir: &Path) -> Option<(PathBuf, Option<u64>)> {
    let mut by_mtime: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .map(|e| {
            let m = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (m, e.path())
        })
        .collect();
    by_mtime.sort_by_key(|(m, _)| std::cmp::Reverse(*m));
    by_mtime.truncate(RESOLVE_CANDIDATES);

    let mut best: Option<(Option<u64>, PathBuf)> = None;
    for (_, path) in by_mtime {
        let last = last_event_in_tail(&path);
        // A log with no parseable event in its tail still beats nothing at all, but
        // loses to any log that has one — `None` sorts below every `Some`.
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
        dir: PathBuf,
        log: Option<PathBuf>,
        mut telemetry: SessionTelemetry,
        start_offset: u64,
        attached: Arc<AtomicBool>,
        wake: impl Fn() + Send + 'static,
    ) -> Option<LogWatcher> {
        let watch_dir = dir.clone();
        let (tx, rx) = mpsc::channel::<Arc<TelemetrySnapshot>>();
        let mut offset = start_offset;
        let mut partial = String::new();
        let mut target = log;
        // Whether to keep re-resolving is the CALLER's decision, shared by handle.
        //
        // It is the same question as "is this log this window's session", and that
        // question is answered in exactly one place — `app::overlay::proves_ownership`
        // — because the last time it was implemented twice the two copies disagreed
        // and pinned the watcher to a log the display would never show. This crate
        // does not re-derive it; it reads the answer.
        //
        // Until the caller attaches, a different file appearing may well be the tab's
        // own session writing for the first time, so we re-resolve to it. Once the
        // caller has attached, we stay: two concurrently active sessions in one
        // project would otherwise trade the attachment on every line, resetting the
        // accumulator and re-reading the whole file each time — and, worse, replacing
        // data already on screen.

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let mut switched = false;
            if !attached.load(Ordering::Relaxed) {
                // A new session's first line lands here: the file did not exist when
                // the overlay opened, so no amount of tailing the old target would
                // ever find it.
                if let Some((newest, _)) = most_recent_session(&dir) {
                    if Some(&newest) != target.as_ref() {
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
            if (folded > 0 || reset || switched) && tx.send(snap).is_ok() {
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
        let w = LogWatcher::spawn(dir.clone(), Some(log.clone()), t, off, unattached(), || {})
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
        let w = LogWatcher::spawn(dir.clone(), Some(old.clone()), t, off, unattached(), || {})
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
            snap.total_output, 3,
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
            missing.clone(),
            None,
            SessionTelemetry::new(),
            0,
            unattached(),
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
        assert_eq!(snap.total_output, 42);
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
                nowhere,
                None,
                SessionTelemetry::new(),
                0,
                unattached(),
                || {}
            )
            .is_none(),
            "declines to arm rather than watching an ancestor outside the projects root"
        );
    }

    #[test]
    fn an_attached_caller_freezes_the_target() {
        // Once the caller says it has attached, another session writing in the same
        // project must not steal the watch — that would reset the accumulator and
        // replace data already on screen, which is exactly what the panel must never
        // do. The watcher holds no copy of the ownership rule; it reads this flag.
        use std::io::Write;
        use std::time::Duration;

        let dir = temp_dir("frozen");
        let mine = dir.join("mine.jsonl");
        std::fs::write(&mine, "").unwrap();
        let (t, off) = backfill_telemetry(&mine);
        let attached = Arc::new(AtomicBool::new(true)); // the caller already attached
        let w = LogWatcher::spawn(dir.clone(), Some(mine.clone()), t, off, attached, || {})
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
            last.expect("still ours").total_output,
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
            dir.clone(),
            Some(a.clone()),
            t,
            off,
            attached.clone(),
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
            seen.expect("switched").total_output,
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
            assert_eq!(s.total_output, 7, "frozen on b — c never adopted");
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
        let w = LogWatcher::spawn(dir.clone(), Some(log.clone()), t, off, unattached(), || {})
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
        let w = LogWatcher::spawn(dir.clone(), Some(log.clone()), t, off, unattached(), || {})
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

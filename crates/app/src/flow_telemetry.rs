//! The session graph's own subagent source (session-flow-viewer D10, D18).
//!
//! While the graph is open, one background thread folds every session log the tabmeta
//! worker published (`TabMeta.log` / `PaneMeta.log`) into an
//! [`ai_monitor::SessionTelemetry`] and publishes `log → Vec<Subagent>` into a slot the
//! overlay reads, behind a generation counter, waking the loop the overlay way.
//!
//! **Why a thread of its own, not the tabmeta worker** (D10, rejected alternative): the worker
//! tails the last few lines of a log for a state, every 500 ms; a subagent list needs the
//! whole log folded once and then followed. That is the wrong cost shape for a per-tick pass.
//!
//! **Lifecycle** (D10). [`spawn`] starts the thread with a fresh stop flag; [`FlowTelemetry::stop`]
//! sets it and returns — the UI thread never joins, so closing the graph during a large
//! backfill cannot stall the event loop. The thread checks the flag between lines and between
//! logs and exits on its own. A reopen spawns a new thread with a **new slot** while the old
//! one winds down, and the flag is re-checked under the slot's lock immediately before every
//! publish, so a thread that has been told to stop can write into no slot and bump no
//! generation — not even its own — once `stop` has returned (R7.3).
//!
//! **Tailing is a poll of the file length, not `ai_monitor::LogWatcher`.** The watcher is
//! built around one Claude *project directory* per panel (re-resolution, rival sets, sibling
//! wakes); here the paths are already resolved by the worker and there may be many. A
//! length check every [`PASS_INTERVAL`] on a handful of files, only while the graph is open,
//! is the "equivalent" the spec allows and costs nothing while it is closed (G1).
//!
//! **The backfill is `backfill_telemetry`'s fold, line by line.** `ai_monitor::backfill_telemetry`
//! reads and folds the whole file in one call, which cannot be interrupted, and R7.3 requires
//! a stopped thread to finish within 2 s even mid-backfill. So the same fold — a fresh
//! Claude-format `SessionTelemetry` with `apply` per line, exactly what `backfill_telemetry`
//! does for a Claude log — runs here with the stop flag checked between lines. One difference,
//! deliberate: an unterminated last line is held until its newline arrives, because the tail
//! continues from that offset and folding half a JSON record would count a parse error and
//! lose the record.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use ai_monitor::telemetry::{SessionTelemetry, Subagent};

/// Where one spawn publishes: each referenced log's subagents, in log order (D10).
///
/// One slot **per spawn**: the overlay replaces it on every open and reads only the current
/// one, which is half of what keeps a thread left over from a previous open out of the model
/// (the other half is the stop check under this lock).
pub type SubagentSlot = Arc<Mutex<HashMap<PathBuf, Vec<Subagent>>>>;

/// How often the thread re-checks its logs for appended bytes.
const PASS_INTERVAL: Duration = Duration::from_millis(250);

/// The sleep granularity inside one interval, so a stop is noticed within this long even
/// while idle.
const STOP_POLL: Duration = Duration::from_millis(25);

// `Worker::idle` counts the interval out in whole `STOP_POLL` steps and stops on equality.
const _: () = assert!(PASS_INTERVAL
    .as_nanos()
    .is_multiple_of(STOP_POLL.as_nanos()));

/// A running subagent thread. Dropping it stops the thread (never joins).
pub struct FlowTelemetry {
    stop: Arc<AtomicBool>,
    /// The next log set, taken by the thread at the start of its next pass.
    pending: Arc<Mutex<Option<Vec<PathBuf>>>>,
    slot: SubagentSlot,
    thread: Option<JoinHandle<()>>,
}

impl FlowTelemetry {
    /// Replace the set of logs to follow. A log no longer in the set is dropped from the
    /// next publish (R7.2, D14: a closed tab's watcher goes on the next pass).
    pub fn update_logs(&self, logs: Vec<PathBuf>) {
        *lock(&self.pending) = Some(logs);
    }

    /// Tell the thread to exit, and return at once (D10: the UI thread never joins).
    ///
    /// The flag is set **while holding the slot's lock**, and the thread checks it under the
    /// same lock before writing. So a publish already in progress completes before this
    /// returns, and none can start after: "after `stop` returned the old thread never
    /// publishes into its own slot again" (R7.3) holds by construction, not by timing. The
    /// lock is held only across a map swap, never across file I/O, so this cannot block on
    /// a backfill.
    pub fn stop(&self) {
        let _guard = lock(&self.slot);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether the thread has exited. `true` for a thread that could not be started.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for FlowTelemetry {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start following `logs` (D10). Publishes into `slot`, bumps `generation` once per publish
/// and then calls `wake`.
///
/// `generation` is the **overlay's**, handed to every spawn, so it is monotonic across opens
/// and the overlay's cache key can never match a model built from a previous open's data.
pub fn spawn(
    logs: Vec<PathBuf>,
    slot: SubagentSlot,
    generation: Arc<AtomicU64>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> FlowTelemetry {
    let stop = Arc::new(AtomicBool::new(false));
    let pending = Arc::new(Mutex::new(Some(logs)));
    let worker = Worker {
        stop: stop.clone(),
        pending: pending.clone(),
        slot: slot.clone(),
        generation,
        wake,
    };
    let thread = std::thread::Builder::new()
        .name("flow-telemetry".into())
        .spawn(move || worker.run());
    let thread = match thread {
        Ok(h) => Some(h),
        Err(e) => {
            // D18: the graph still renders every session's status, just with no subagents.
            tracing::warn!("session graph: could not start the subagent thread: {e}");
            None
        }
    };
    FlowTelemetry {
        stop,
        pending,
        slot,
        thread,
    }
}

/// Poison is not a reason to lose the graph: the data is a plain map, valid at every point.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One followed log.
struct Tail {
    telemetry: SessionTelemetry,
    /// Bytes folded so far — always just past a newline.
    offset: u64,
    /// The last subagent list computed, `None` until the first fold.
    subagents: Option<Vec<Subagent>>,
}

impl Tail {
    fn new() -> Tail {
        Tail {
            telemetry: SessionTelemetry::new(),
            offset: 0,
            subagents: None,
        }
    }
}

/// What one advance of one log found.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Unchanged,
    Changed,
    Stopped,
}

struct Worker {
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<Option<Vec<PathBuf>>>>,
    slot: SubagentSlot,
    generation: Arc<AtomicU64>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run(self) {
        let mut logs: Vec<PathBuf> = Vec::new();
        let mut watched: HashMap<PathBuf, Tail> = HashMap::new();
        // D18: an unreadable log is named once, not once per pass; a success re-arms it.
        let mut warned: HashSet<PathBuf> = HashSet::new();
        loop {
            if self.stopped() {
                return;
            }
            let mut dirty = false;
            if let Some(next) = lock(&self.pending).take() {
                logs = next;
                let before = watched.len();
                watched.retain(|p, _| logs.contains(p));
                warned.retain(|p| logs.contains(p));
                dirty |= watched.len() != before;
            }
            for path in &logs {
                if self.stopped() {
                    return;
                }
                let mut fresh = None;
                let tail = match watched.get_mut(path) {
                    Some(t) => t,
                    None => fresh.insert(Tail::new()),
                };
                match advance(path, tail, &self.stop) {
                    Ok(Step::Stopped) => return,
                    Ok(step) => {
                        dirty |= step == Step::Changed;
                        warned.remove(path);
                        if let Some(t) = fresh {
                            watched.insert(path.clone(), t);
                        }
                    }
                    Err(e) => {
                        // R7.4: absent from the publish, retried on the next pass.
                        if warned.insert(path.clone()) {
                            tracing::warn!(
                                "session graph: cannot read session log {}: {e}",
                                path.display()
                            );
                        }
                    }
                }
            }
            if dirty && !self.publish(&watched) {
                return;
            }
            if !self.idle() {
                return;
            }
        }
    }

    /// Replace the slot's map with what `watched` holds. `false` when stopped — the work is
    /// discarded, and nothing is written or bumped (R7.3).
    fn publish(&self, watched: &HashMap<PathBuf, Tail>) -> bool {
        let map: HashMap<PathBuf, Vec<Subagent>> = watched
            .iter()
            .filter_map(|(p, t)| t.subagents.clone().map(|s| (p.clone(), s)))
            .collect();
        {
            let mut slot = lock(&self.slot);
            if self.stopped() {
                return false;
            }
            *slot = map;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        (self.wake)();
        true
    }

    /// Sleep one pass interval in short steps. `false` once stopped.
    ///
    /// `!=`, not `<`: the interval is a whole number of steps (asserted beside the
    /// constants), so the loop ends exactly on it, and `<=` would be one step too long — a
    /// mutant no test can tell from a slow scheduler.
    fn idle(&self) -> bool {
        let mut slept = Duration::ZERO;
        while slept != PASS_INTERVAL {
            if self.stopped() {
                return false;
            }
            std::thread::sleep(STOP_POLL);
            slept += STOP_POLL;
        }
        !self.stopped()
    }
}

/// Fold whatever complete lines `path` gained since `tail.offset`.
///
/// A file shorter than the offset was truncated or replaced: the accumulator restarts from
/// the top, as `LogWatcher` does (`SessionTelemetry::restarted`).
fn advance(path: &Path, tail: &mut Tail, stop: &AtomicBool) -> std::io::Result<Step> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut changed = tail.subagents.is_none();
    if len < tail.offset {
        tail.telemetry = tail.telemetry.restarted();
        tail.offset = 0;
        changed = true;
    }
    // After the restart above `len >= tail.offset`, so "unequal" is "bytes were appended";
    // `>=` here would only seek and read nothing.
    if len != tail.offset {
        file.seek(SeekFrom::Start(tail.offset))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        if let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') {
            for line in buf[..last_nl].split(|&b| b == b'\n') {
                if stop.load(Ordering::SeqCst) {
                    return Ok(Step::Stopped);
                }
                tail.telemetry.apply(&String::from_utf8_lossy(line));
            }
            tail.offset += last_nl as u64 + 1;
            changed = true;
        }
    }
    if !changed {
        return Ok(Step::Unchanged);
    }
    let subagents = tail.telemetry.snapshot().subagents;
    if tail.subagents.as_ref() == Some(&subagents) {
        return Ok(Step::Unchanged);
    }
    tail.subagents = Some(subagents);
    Ok(Step::Changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flow-telemetry-unit-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("log.jsonl")
    }

    const AGENT: &str = r#"{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"Explore","description":"d"},"timestamp":"2026-08-19T00:00:00.000Z"}"#;

    #[test]
    fn an_unterminated_line_waits_for_its_newline() {
        let p = temp("partial");
        std::fs::write(&p, AGENT).expect("write");
        let mut tail = Tail::new();
        let stop = AtomicBool::new(false);
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Changed));
        assert_eq!(tail.offset, 0, "nothing folded before the newline");
        assert_eq!(tail.subagents.as_deref().map(<[_]>::len), Some(0));

        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Changed));
        assert_eq!(tail.subagents.as_deref().map(<[_]>::len), Some(1));
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Unchanged));
    }

    #[test]
    fn a_truncated_log_restarts_the_fold() {
        let p = temp("truncate");
        std::fs::write(&p, format!("{AGENT}\n{AGENT}\n")).expect("write");
        let mut tail = Tail::new();
        let stop = AtomicBool::new(false);
        let _ = advance(&p, &mut tail, &stop);
        std::fs::write(&p, "{}\n").expect("write");
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Changed));
        assert_eq!(tail.offset, 3);
        assert_eq!(tail.subagents.as_deref().map(<[_]>::len), Some(0));
    }

    #[test]
    fn a_set_stop_flag_abandons_the_fold() {
        let p = temp("stop");
        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        let mut tail = Tail::new();
        let stop = AtomicBool::new(true);
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Stopped));
    }

    const AGENT2: &str = r#"{"type":"tool_use","id":"t2","name":"Agent","input":{"subagent_type":"Plan","description":"e"},"timestamp":"2026-08-19T00:00:01.000Z"}"#;

    /// Poll `cond` every 10 ms for at most `limit`.
    fn within(limit: Duration, cond: impl Fn() -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < limit {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    fn start(p: &Path) -> (FlowTelemetry, SubagentSlot, Arc<AtomicU64>) {
        let slot: SubagentSlot = Arc::default();
        let generation = Arc::new(AtomicU64::new(0));
        let h = spawn(
            vec![p.to_path_buf()],
            slot.clone(),
            generation.clone(),
            Arc::new(|| {}),
        );
        (h, slot, generation)
    }

    fn worker(stop: bool) -> Worker {
        Worker {
            stop: Arc::new(AtomicBool::new(stop)),
            pending: Arc::default(),
            slot: Arc::default(),
            generation: Arc::default(),
            wake: Arc::new(|| {}),
        }
    }

    #[test]
    fn dropping_the_handle_stops_the_thread() {
        let p = temp("drop");
        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        let (h, slot, generation) = start(&p);
        assert!(within(Duration::from_secs(5), || generation
            .load(Ordering::SeqCst)
            >= 1));
        drop(h);
        // The thread owns the only other clone of the slot; it goes when the thread exits.
        assert!(
            within(Duration::from_secs(5), || Arc::strong_count(&slot) == 1),
            "dropping the handle did not stop the thread"
        );
    }

    #[test]
    fn stop_while_idle_finishes_the_thread() {
        let p = temp("idle-stop");
        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        let (h, _slot, generation) = start(&p);
        assert!(within(Duration::from_secs(5), || generation
            .load(Ordering::SeqCst)
            >= 1));
        h.stop();
        assert!(within(Duration::from_secs(2), || h.is_finished()), "R7.3");
    }

    #[test]
    fn stop_mid_backfill_finishes_without_publishing() {
        let p = temp("big");
        std::fs::write(&p, format!("{AGENT}\n").repeat(200_000)).expect("write");
        let (h, slot, generation) = start(&p);
        h.stop();
        assert!(within(Duration::from_secs(2), || h.is_finished()), "R7.3");
        assert_eq!(generation.load(Ordering::SeqCst), 0);
        assert!(lock(&slot).is_empty());
    }

    #[test]
    fn an_unchanged_log_is_not_republished() {
        let p = temp("unchanged");
        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        let (h, slot, generation) = start(&p);
        assert!(within(Duration::from_secs(5), || generation
            .load(Ordering::SeqCst)
            == 1));
        assert_eq!(lock(&slot).get(&p).map(Vec::len), Some(1));
        // Three passes over an unchanged file: no further publish.
        std::thread::sleep(PASS_INTERVAL * 3);
        assert_eq!(generation.load(Ordering::SeqCst), 1);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .expect("open");
        std::io::Write::write_all(&mut f, format!("{AGENT2}\n").as_bytes()).expect("append");
        assert!(within(Duration::from_secs(5), || lock(&slot)
            .get(&p)
            .map(Vec::len)
            == Some(2)));
        assert_eq!(generation.load(Ordering::SeqCst), 2);
        drop(h);
    }

    #[test]
    fn idle_sleeps_one_pass_interval() {
        let w = worker(false);
        let t = std::time::Instant::now();
        assert!(w.idle());
        let slept = t.elapsed();
        assert!(slept >= PASS_INTERVAL, "{slept:?}");
        assert!(slept < Duration::from_secs(5), "{slept:?}");
    }

    #[test]
    fn idle_returns_at_once_when_stopped() {
        let w = worker(true);
        let t = std::time::Instant::now();
        assert!(!w.idle());
        assert!(t.elapsed() < PASS_INTERVAL);
    }

    #[test]
    fn folded_bytes_are_not_read_again() {
        // The tail is incremental: a log whose length did not change is not re-folded from
        // the top, even if (here, deliberately) the bytes behind the offset were rewritten.
        let p = temp("same-len");
        std::fs::write(&p, format!("{AGENT}\n")).expect("write");
        let mut tail = Tail::new();
        let stop = AtomicBool::new(false);
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Changed));
        let offset = tail.offset;
        std::fs::write(&p, format!("{}\n", " ".repeat(AGENT.len()))).expect("write");
        assert_eq!(advance(&p, &mut tail, &stop).ok(), Some(Step::Unchanged));
        assert_eq!(tail.offset, offset);
        assert_eq!(tail.subagents.as_deref().map(<[_]>::len), Some(1));
    }
}

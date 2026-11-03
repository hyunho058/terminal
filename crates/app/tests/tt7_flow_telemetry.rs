//! Acceptance tests for session-flow-viewer R7: a viewer-owned telemetry thread maps the
//! published session logs to their subagents (D10, D18). Written from the spec's Thens; every
//! app item is reached through the `app` crate root (D17), and `Subagent` through its own
//! crate.
//!
//! Fixtures are Claude Code log lines in the shapes `ai_monitor::telemetry::SessionTelemetry::apply`
//! recognises (copied from `crates/ai-monitor/tests/telemetry.rs`): an `Agent` `tool_use`, its
//! async-launch receipt carrying `toolUseResult.agentId` (so the agent is NOT yet completed), and a
//! bare-string `<task-notification>` that ends it.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ai_monitor::telemetry::Subagent;
use app::{spawn_flow_telemetry, DigestSlot, FlowTelemetry};

const DEADLINE: Duration = Duration::from_secs(5);

// ---- fixtures ---------------------------------------------------------------

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tt7-flow-telemetry-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An `Agent` tool_use and its async-launch receipt: one subagent, running.
fn agent_lines(tool_id: &str, agent_id: &str) -> String {
    format!(
        concat!(
            r#"{{"type":"tool_use","id":"{t}","name":"Agent","input":{{"subagent_type":"general-purpose","description":"sweep"}},"timestamp":"2026-08-19T00:00:00.000Z"}}"#,
            "\n",
            r#"{{"type":"tool_result","tool_use_id":"{t}","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{a}"}}}}"#,
            "\n"
        ),
        t = tool_id,
        a = agent_id
    )
}

/// A `user` record whose bare-string content is a completed task-notification for `agent_id`.
fn notification_line(agent_id: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"2026-08-19T00:04:00.000Z","message":{{"content":"<task-notification>\n<task-id>{agent_id}</task-id>\n<tool-use-id>toolu_wake</tool-use-id>\n<status>completed</status>\n<summary>swept the crate</summary>\n</task-notification>"}}}}"#
    ) + "\n"
}

fn write(path: &Path, body: &str) {
    fs::write(path, body).expect("write fixture");
}

fn append(path: &Path, body: &str) {
    let mut f = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open fixture for append");
    f.write_all(body.as_bytes()).expect("append fixture");
    f.flush().expect("flush fixture");
}

// ---- harness ----------------------------------------------------------------

fn new_slot() -> DigestSlot {
    Arc::new(Mutex::new(HashMap::new()))
}

/// A wake that counts its calls.
fn counting_wake() -> (Arc<dyn Fn() + Send + Sync>, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    (
        Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }),
        n,
    )
}

/// The slot's subagents per log. The slot holds each log's whole `SessionDigest`
/// (session-overview-ui D19); these tests are about which logs publish and their subagents.
fn read(slot: &DigestSlot) -> HashMap<PathBuf, Vec<Subagent>> {
    slot.lock()
        .expect("slot lock")
        .iter()
        .map(|(p, d)| (p.clone(), d.subagents.clone()))
        .collect()
}

/// Poll `cond` against the slot until it holds or the deadline passes.
fn wait_for(
    slot: &DigestSlot,
    what: &str,
    cond: impl Fn(&HashMap<PathBuf, Vec<Subagent>>) -> bool,
) {
    let start = Instant::now();
    loop {
        if cond(&read(slot)) {
            return;
        }
        if start.elapsed() > DEADLINE {
            panic!("timed out waiting for: {what}; slot = {:?}", read(slot));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn completed_of(map: &HashMap<PathBuf, Vec<Subagent>>, log: &Path, agent: &str) -> Option<bool> {
    map.get(log)?
        .iter()
        .find(|s| s.agent_id.as_deref() == Some(agent))
        .map(|s| s.completed)
}

// ---- R7.1 -------------------------------------------------------------------

#[test]
fn r7_1_referenced_logs_are_backfilled_and_tailed() {
    let dir = TempDir::new("r7-1");
    let a = dir.path("a.jsonl");
    let b = dir.path("b.jsonl");
    write(&a, &agent_lines("toolu_a", "agent-a"));
    write(&b, &agent_lines("toolu_b", "agent-b"));

    let slot = new_slot();
    let generation = Arc::new(AtomicU64::new(0));
    let (wake, wakes) = counting_wake();
    let t: FlowTelemetry = spawn_flow_telemetry(
        vec![a.clone(), b.clone()],
        slot.clone(),
        generation.clone(),
        wake,
    );

    // Backfill: each path maps to its own Vec<Subagent>.
    wait_for(&slot, "both logs backfilled", |m| {
        completed_of(m, &a, "agent-a").is_some() && completed_of(m, &b, "agent-b").is_some()
    });
    let m = read(&slot);
    assert_eq!(m[&a].len(), 1, "a holds exactly its own agent: {m:?}");
    assert_eq!(m[&b].len(), 1, "b holds exactly its own agent: {m:?}");
    assert_eq!(completed_of(&m, &a, "agent-a"), Some(false));
    let gen_before = generation.load(Ordering::SeqCst);
    let wakes_before = wakes.load(Ordering::SeqCst);
    assert!(gen_before > 0, "a publish bumped the generation");
    assert!(wakes_before > 0, "a publish called wake");

    // Tail: the appended notification completes a's agent.
    append(&a, &notification_line("agent-a"));
    wait_for(&slot, "agent-a completed after append", |m| {
        completed_of(m, &a, "agent-a") == Some(true)
    });
    assert!(
        generation.load(Ordering::SeqCst) > gen_before,
        "the tailed publish increased the generation"
    );
    assert!(
        wakes.load(Ordering::SeqCst) > wakes_before,
        "the tailed publish called wake"
    );
    let m = read(&slot);
    assert_eq!(
        completed_of(&m, &b, "agent-b"),
        Some(false),
        "b is still mapped"
    );

    t.stop();
}

// ---- R7.2 -------------------------------------------------------------------

#[test]
fn r7_2_unreferenced_logs_are_dropped() {
    let dir = TempDir::new("r7-2");
    let a = dir.path("a.jsonl");
    let b = dir.path("b.jsonl");
    write(&a, &agent_lines("toolu_a", "agent-a"));
    write(&b, &agent_lines("toolu_b", "agent-b"));

    let slot = new_slot();
    let (wake, _) = counting_wake();
    let t = spawn_flow_telemetry(
        vec![a.clone(), b.clone()],
        slot.clone(),
        Arc::new(AtomicU64::new(0)),
        wake,
    );
    wait_for(&slot, "both logs published", |m| {
        m.contains_key(&a) && m.contains_key(&b)
    });

    t.update_logs(vec![a.clone()]);
    wait_for(&slot, "b dropped after update_logs", |m| {
        !m.contains_key(&b)
    });

    t.stop();
}

// ---- R7.3 -------------------------------------------------------------------

#[test]
fn r7_3_stop_never_blocks_and_never_publishes_late() {
    let dir = TempDir::new("r7-3");
    let big = dir.path("big.jsonl");
    let small = dir.path("small.jsonl");
    // A large backfill: many Agent pairs, then a lot of unrelated lines.
    let mut body = String::new();
    for i in 0..20_000 {
        body.push_str(&agent_lines(&format!("toolu_{i}"), &format!("agent-{i}")));
    }
    for i in 0..60_000 {
        body.push_str(&format!(
            r#"{{"type":"user","timestamp":"2026-08-19T00:00:01.000Z","message":{{"content":"filler line {i} to make the backfill long"}}}}"#
        ));
        body.push('\n');
    }
    write(&big, &body);
    write(&small, &agent_lines("toolu_s", "agent-s"));

    let generation = Arc::new(AtomicU64::new(0));
    let old_slot = new_slot();
    let (wake, _) = counting_wake();
    let old = spawn_flow_telemetry(
        vec![big.clone()],
        old_slot.clone(),
        generation.clone(),
        wake.clone(),
    );

    let t0 = Instant::now();
    old.stop();
    let stop_took = t0.elapsed();
    // Taken the moment `stop` returned: nothing after this may land in the old slot.
    let old_at_stop = read(&old_slot);

    let new_slot = new_slot();
    let new = spawn_flow_telemetry(
        vec![small.clone()],
        new_slot.clone(),
        generation.clone(),
        wake,
    );

    assert!(
        stop_took < Duration::from_millis(250),
        "stop returned without joining the backfill (took {stop_took:?})"
    );

    // The old handle finishes on its own within 2 s.
    let start = Instant::now();
    while !old.is_finished() {
        assert!(
            new_slot.lock().expect("slot").get(&big).is_none(),
            "the new slot never holds the old thread's log"
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "old telemetry thread still running 2 s after stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // The new spawn publishes its own log, and only its own.
    wait_for(&new_slot, "new spawn publishes small", |m| {
        m.contains_key(&small)
    });
    // Give a straggler every chance to show itself.
    std::thread::sleep(Duration::from_millis(300));
    let n = read(&new_slot);
    assert!(
        !n.contains_key(&big),
        "the new slot never contains entries written by the old thread: {:?}",
        n.keys()
    );
    assert_eq!(
        read(&old_slot),
        old_at_stop,
        "the old thread never published into its own slot after stop returned"
    );

    new.stop();
}

// ---- R7.4 -------------------------------------------------------------------

#[test]
fn r7_4_unreadable_log_is_absent_not_fatal() {
    let dir = TempDir::new("r7-4");
    let missing = dir.path("missing.jsonl");
    let good = dir.path("good.jsonl");
    write(&good, &agent_lines("toolu_g", "agent-g"));
    assert!(!missing.exists());

    let slot = new_slot();
    let (wake, _) = counting_wake();
    let t = spawn_flow_telemetry(
        vec![missing.clone(), good.clone()],
        slot.clone(),
        Arc::new(AtomicU64::new(0)),
        wake,
    );

    wait_for(&slot, "the readable log still publishes", |m| {
        m.contains_key(&good)
    });
    assert!(
        !read(&slot).contains_key(&missing),
        "a missing log has no entry"
    );
    assert!(!t.is_finished(), "the thread keeps running");

    // A later-created file at that path is picked up on a following pass.
    write(&missing, &agent_lines("toolu_m", "agent-m"));
    wait_for(&slot, "the late-created log is picked up", |m| {
        completed_of(m, &missing, "agent-m").is_some()
    });
    assert!(!t.is_finished(), "the thread is still running");

    t.stop();
}

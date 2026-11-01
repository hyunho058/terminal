//! TT8 — acceptance test for `specs/session-overview-ui` R8.9 (D19): the flow telemetry
//! thread publishes each followed log's whole `SessionDigest` into the overlay's
//! `DigestSlot`, and a log gaining a subagent launch reaches the slot on the next pass with
//! the generation counter advanced.
//!
//! Driven through `app::spawn_flow_telemetry` with a real file under the temp dir, as
//! `tt7_flow_telemetry.rs` does; the open overlay is stood in by the slot, the generation
//! and the wake it would hand the thread. Fixture lines are Claude Code records.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ai_monitor::telemetry::SessionTelemetry;
use app::{spawn_flow_telemetry, DigestSlot, SessionDigest};

const DEADLINE: Duration = Duration::from_secs(5);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tt8-overview-digest-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn human(min: u32, text: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"2026-10-31T10:{min:02}:00.000Z","sessionId":"sess-r89","origin":{{"kind":"human"}},"message":{{"role":"user","content":"{text}"}}}}"#
    ) + "\n"
}

fn bash(min: u32, id: &str) -> String {
    format!(
        concat!(
            r#"{{"type":"assistant","timestamp":"2026-10-31T10:{m:02}:00.000Z","sessionId":"sess-r89","message":{{"id":"msg_{id}","role":"assistant","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"cargo build"}}}}]}}}}"#,
            "\n",
            r#"{{"type":"user","timestamp":"2026-10-31T10:{m:02}:01.000Z","sessionId":"sess-r89","toolUseResult":{{"stdout":"ok"}},"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}}}}"#,
            "\n"
        ),
        m = min,
        id = id
    )
}

/// A background `Agent` launch: its `tool_use` and the async-launch receipt.
fn agent_launch(min: u32, id: &str, agent_id: &str, description: &str) -> String {
    format!(
        concat!(
            r#"{{"type":"assistant","timestamp":"2026-10-31T10:{m:02}:00.000Z","sessionId":"sess-r89","message":{{"id":"msg_{id}","role":"assistant","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"description":"{d}","subagent_type":"Explore","prompt":"go"}}}}]}}}}"#,
            "\n",
            r#"{{"type":"user","timestamp":"2026-10-31T10:{m:02}:01.000Z","sessionId":"sess-r89","toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{a}"}},"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"Async agent launched successfully."}}]}}}}"#,
            "\n"
        ),
        m = min,
        id = id,
        a = agent_id,
        d = description
    )
}

fn append(path: &Path, body: &str) {
    let mut f = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open fixture for append");
    f.write_all(body.as_bytes()).expect("append fixture");
    f.flush().expect("flush fixture");
}

/// The digest a digest-flagged fold of `path`'s current contents gives.
fn expected_digest(path: &Path) -> SessionDigest {
    let mut t = SessionTelemetry::new().with_digest();
    for line in fs::read_to_string(path).expect("read fixture").lines() {
        t.apply(line);
    }
    SessionDigest::from_snapshot(t.snapshot())
}

fn digest_of(slot: &DigestSlot, log: &Path) -> Option<SessionDigest> {
    slot.lock().expect("slot lock").get(log).cloned()
}

fn wait_for(
    slot: &DigestSlot,
    what: &str,
    cond: impl Fn(Option<&SessionDigest>) -> bool,
    log: &Path,
) {
    let start = Instant::now();
    loop {
        if cond(digest_of(slot, log).as_ref()) {
            return;
        }
        assert!(
            start.elapsed() < DEADLINE,
            "timed out waiting for: {what}; slot entry = {:?}",
            digest_of(slot, log)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// R8.9: a Claude log gains a subagent launch line → the next pass publishes that log's
/// `SessionDigest` with the new subagent, and the generation counter advances.
#[test]
fn r8_9_digest_with_new_subagent_reaches_the_slot() {
    let dir = TempDir::new("r8-9");
    let log = dir.0.join("session.jsonl");
    fs::write(&log, human(0, "build it") + &bash(1, "toolu_b1")).expect("write fixture");

    let slot: DigestSlot = Arc::new(Mutex::new(HashMap::new()));
    let generation = Arc::new(AtomicU64::new(0));
    let t = spawn_flow_telemetry(
        vec![log.clone()],
        slot.clone(),
        generation.clone(),
        Arc::new(|| {}),
    );

    wait_for(&slot, "the log's first publish", |d| d.is_some(), &log);
    let gen_before = generation.load(Ordering::SeqCst);

    append(
        &log,
        &agent_launch(2, "toolu_new", "a1b2c3d4e5f60718", "Explore the parser"),
    );
    wait_for(
        &slot,
        "the new subagent in the published digest",
        |d| d.is_some_and(|d| d.subagents.iter().any(|s| s.id == "toolu_new")),
        &log,
    );

    let got = digest_of(&slot, &log).expect("the log's digest");
    assert_eq!(
        got,
        expected_digest(&log),
        "the slot holds that log's SessionDigest"
    );
    let new = got
        .subagents
        .iter()
        .find(|s| s.id == "toolu_new")
        .expect("the new subagent");
    assert_eq!(new.description.as_deref(), Some("Explore the parser"));
    assert!(
        generation.load(Ordering::SeqCst) > gen_before,
        "the generation counter advanced"
    );

    t.stop();
}

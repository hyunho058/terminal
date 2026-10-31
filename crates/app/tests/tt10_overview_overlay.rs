//! TT10 — acceptance tests for `specs/session-overview-ui` R10.1 and R10.2 (D18, D19):
//! the open overview rebuilds once per minute and not per frame, and a closed overview
//! costs nothing new.
//!
//! Both drive a real `FlowOverlay` with no tabmeta worker (`None`): the snapshot is stood
//! in through `sync_snapshot`, and the sidebar rows are published on this test thread's
//! shared `TabModelHandle`, as `tt7_graph_header.rs` does.
//!
//! **Written against T8's constructor and one seam T8 adds.** `FlowOverlay::new(wake,
//! None, OverviewHandle::new())` takes an `OverviewHandle` only after T8, and
//! `FlowOverlay::set_now_ms(u64)` — the clock R10.1's "`now_ms` crosses a minute
//! boundary" needs — does not exist in `flow.rs` today (no `now` seam is there). Until T8
//! lands, this file does not compile; that is why it is separate from the other TT10
//! files. The seam's contract as assumed here: every later rebuild-key check reads the
//! set value as `now_ms` instead of the wall clock.
//!
//! R10.2's "the ai-monitor open" is represented by the ai-monitor's own fold,
//! `SessionTelemetry::new()`, which is what its Then names; the ai-monitor overlay itself
//! is not instantiated.

use std::fs::{self, OpenOptions};
use std::io::{Sink, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai_monitor::telemetry::SessionTelemetry;
use app::{
    Agent, AgentKind, AgentState, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, TabId, TabMeta,
    TabMetaSnapshot,
};
use renderer::overview_panel::OverviewHandle;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

const MINUTE_MS: u64 = 60_000;
const BASE_MS: u64 = 1_793_440_800_000;

// ---- fixtures ----------------------------------------------------------------------------

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("f".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

fn press_chord(overlay: &mut FlowOverlay) {
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
    assert_eq!(
        overlay.on_key(&cmd_shift_f(), &mut ctx),
        Handled::Consumed,
        "fixture: the chord is the overlay's"
    );
}

fn row(id: u64) -> SidebarRow {
    SidebarRow {
        tab_id: id,
        title: format!("t{id}"),
        ..SidebarRow::default()
    }
}

/// A Claude tab in `state`, reading `log` (or none).
fn claude(state: AgentState, log: Option<PathBuf>) -> TabMeta {
    let agent = Agent {
        kind: AgentKind::Claude,
        state,
    };
    TabMeta {
        agent: Some(agent),
        graph_agent: Some(agent),
        cwd: Some(PathBuf::from("/Users/u/scratch")),
        graph_agent_cwd: Some(PathBuf::from("/Users/u/scratch")),
        log,
        ..TabMeta::default()
    }
}

/// One Claude tab published as a sidebar row on this thread, and a fresh, closed overlay.
fn overlay_with_one_tab() -> FlowOverlay {
    TabModelHandle::shared().set(TabBarModel {
        titles: vec![row(1)],
        ..TabBarModel::default()
    });
    FlowOverlay::new(Arc::new(|| {}), None, OverviewHandle::new())
}

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tt10-overview-overlay-{tag}-{}-{nanos}",
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

/// `2026-10-31T10:{min}:{sec}Z` as the log writes it.
fn stamp(min: u32, sec: u32) -> String {
    format!("2026-10-31T10:{min:02}:{sec:02}.000Z")
}

/// Claude Code log lines (the shapes `crates/ai-monitor/tests/digest.rs` folds): a human
/// prompt, a finished `Bash` call, a background `Agent` launch with its completed
/// notification, and an `end_turn` — every digest field has something to fold.
fn log_lines(min: u32) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"user","timestamp":"{}","sessionId":"s","origin":{{"kind":"human"}},"message":{{"role":"user","content":"run the tests"}}}}"#,
            stamp(min, 0)
        ),
        format!(
            r#"{{"type":"assistant","timestamp":"{}","sessionId":"s","message":{{"id":"msg_b{min}","role":"assistant","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"toolu_b{min}","name":"Bash","input":{{"command":"cargo test"}}}}]}}}}"#,
            stamp(min, 1)
        ),
        format!(
            r#"{{"type":"user","timestamp":"{}","sessionId":"s","toolUseResult":{{"stdout":"ok"}},"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_b{min}","content":"ok"}}]}}}}"#,
            stamp(min, 2)
        ),
        format!(
            r#"{{"type":"assistant","timestamp":"{}","sessionId":"s","message":{{"id":"msg_a{min}","role":"assistant","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"toolu_a{min}","name":"Agent","input":{{"description":"Scan","subagent_type":"Explore"}}}}]}}}}"#,
            stamp(min, 3)
        ),
        format!(
            r#"{{"type":"user","timestamp":"{}","sessionId":"s","toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"agent{min}"}},"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_a{min}","content":"Async agent launched successfully."}}]}}}}"#,
            stamp(min, 4)
        ),
        format!(
            r#"{{"type":"user","timestamp":"{}","sessionId":"s","origin":{{"kind":"task-notification"}},"promptSource":"system","message":{{"role":"user","content":"<task-notification>\n<task-id>agent{min}</task-id>\n<status>completed</status>\n<summary>Agent finished</summary>\n<result>Scanned.</result>\n</task-notification>"}}}}"#,
            stamp(min, 5)
        ),
        format!(
            r#"{{"type":"assistant","timestamp":"{}","sessionId":"s","message":{{"id":"msg_e{min}","role":"assistant","stop_reason":"end_turn","content":[{{"type":"text","text":"Done."}}]}}}}"#,
            stamp(min, 6)
        ),
    ]
}

fn append(path: &Path, lines: &[String]) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open log for append");
    for l in lines {
        f.write_all(l.as_bytes()).expect("append");
        f.write_all(b"\n").expect("append newline");
    }
    f.flush().expect("flush");
}

// ---- R10.1 -------------------------------------------------------------------------------

// R10.1
#[test]
fn r10_1_crossing_a_minute_rebuilds_once_and_frames_within_it_do_not() {
    let mut overlay = overlay_with_one_tab();
    // Last ms of a minute: the next ms crosses the boundary.
    let last_of_minute = BASE_MS + 59_999;
    overlay.set_now_ms(BASE_MS);
    press_chord(&mut overlay);
    assert!(overlay.visible(), "fixture: the overview is open");
    overlay.sync_snapshot(TabMetaSnapshot::for_test(
        vec![(TabId(1), claude(AgentState::Busy, None))],
        true,
    ));
    overlay.poll();

    // Settle: frames inside the same minute, nothing else changing.
    for now in [BASE_MS + 1, BASE_MS + 30_000, last_of_minute] {
        overlay.set_now_ms(now);
        overlay.poll();
    }
    let before = overlay.rebuilds();
    assert!(
        before > 0,
        "fixture: the open overview built its model at least once"
    );

    // Cross the boundary.
    overlay.set_now_ms(BASE_MS + MINUTE_MS);
    overlay.poll();
    assert_eq!(
        overlay.rebuilds(),
        before + 1,
        "crossing the minute boundary rebuilds exactly once"
    );

    // Further frames within the new minute.
    for now in [
        BASE_MS + MINUTE_MS,
        BASE_MS + MINUTE_MS + 1,
        BASE_MS + MINUTE_MS + 30_000,
        BASE_MS + 2 * MINUTE_MS - 1,
    ] {
        overlay.set_now_ms(now);
        overlay.poll();
        assert_eq!(
            overlay.rebuilds(),
            before + 1,
            "a frame at +{} ms within the same minute rebuilt",
            now - BASE_MS - MINUTE_MS
        );
    }
}

// ---- R10.2 -------------------------------------------------------------------------------

// R10.2
#[test]
fn r10_2_closed_overlay_costs_nothing_new_while_logs_grow() {
    let dir = TempDir::new("r10-2");
    let log = dir.path("session.jsonl");
    let first = log_lines(0);
    append(&log, &first);

    let mut overlay = overlay_with_one_tab();
    press_chord(&mut overlay);
    overlay.sync_snapshot(TabMetaSnapshot::for_test(
        vec![(TabId(1), claude(AgentState::Busy, Some(log.clone())))],
        true,
    ));
    overlay.poll();
    // Preconditions, so the closed-state checks below are not vacuous.
    assert!(
        overlay.telemetry_running(),
        "fixture: the open overview runs its telemetry thread"
    );
    assert!(
        overlay.rebuilds() > 0,
        "fixture: the open overview built its model"
    );

    // Close it.
    press_chord(&mut overlay);
    assert!(!overlay.visible(), "fixture: the overview is closed");
    let rebuilds_at_close = overlay.rebuilds();

    // Logs grow while it is closed, and the loop keeps polling every overlay.
    let more = log_lines(10);
    append(&log, &more);
    for _ in 0..10 {
        overlay.poll();
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        !overlay.telemetry_running(),
        "a closed overview runs no telemetry thread"
    );
    assert_eq!(
        overlay.rebuilds(),
        rebuilds_at_close,
        "a closed overview does not rebuild while logs grow"
    );

    // The ai-monitor's fold of the same lines does no digest work (R3.7).
    let mut plain = SessionTelemetry::new();
    for l in first.iter().chain(more.iter()) {
        plain.apply(l);
    }
    let s = plain.snapshot();
    assert_eq!(s.last_tool, None, "last_tool");
    assert_eq!(s.last_work_tool, None, "last_work_tool");
    assert_eq!(s.last_prompt_ms, None, "last_prompt_ms");
    assert!(s.turns.is_empty(), "turns: {:?}", s.turns);
    assert!(s.events.is_empty(), "events: {:?}", s.events);
    assert!(
        s.subagents.iter().all(|a| a.report_preview.is_none()),
        "report_preview: {:?}",
        s.subagents
    );
}

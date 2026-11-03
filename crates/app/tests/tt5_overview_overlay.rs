//! TT5 — acceptance tests for `specs/session-overview-ui` R5.3 and the reopen half of R5.5
//! (D8, D10): the chevron toggles a row's tree through the real `FlowOverlay`, and the
//! expanded set survives a close and reopen.
//!
//! Written against T8's constructor `FlowOverlay::new(wake, None, OverviewHandle)`; this file
//! does not compile until T8 switches the overlay to the overview handle (expected).
//!
//! The overlay runs with no tabmeta worker: the snapshot is stood in through
//! `sync_snapshot`, the sidebar rows through this thread's shared `TabModelHandle`. Subagents
//! reach the model the real way — a Claude log on disk, followed by the overlay's own flow
//! telemetry thread — so the test polls until the digest has landed. The chevron's point is
//! taken from `layout_overview` over the overlay's published model and handed to the handle
//! with `publish`, standing in for a composed frame.

use std::fs;
use std::io::Sink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::{
    Agent, AgentKind, AgentState, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, TabId, TabMeta,
    TabMetaSnapshot,
};
use renderer::overview_layout::layout_overview;
use renderer::overview_model::{OverviewHit, OverviewModel, SessionRowData};
use renderer::overview_panel::OverviewHandle;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

const DEADLINE: Duration = Duration::from_secs(5);
const VIEWPORT: [f32; 2] = [1440.0, 900.0];
/// The row with three subagents.
const TAB: u64 = 3;
/// A Claude row with no subagents (no log).
const BARE: u64 = 4;

// ---- fixtures -----------------------------------------------------------------------------

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tt5-overview-overlay-{tag}-{}-{nanos}",
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

/// An `Agent` tool_use and its async-launch receipt: one running subagent.
fn agent_lines(tool_id: &str, agent_id: &str, desc: &str) -> String {
    format!(
        concat!(
            r#"{{"type":"tool_use","id":"{t}","name":"Agent","input":{{"subagent_type":"Explore","description":"{d}"}},"timestamp":"2026-08-19T00:00:00.000Z"}}"#,
            "\n",
            r#"{{"type":"tool_result","tool_use_id":"{t}","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{a}"}}}}"#,
            "\n"
        ),
        t = tool_id,
        a = agent_id,
        d = desc
    )
}

/// A Claude log with three running subagents.
fn three_subagent_log(dir: &TempDir) -> PathBuf {
    let log = dir.path("session.jsonl");
    let body = agent_lines("toolu_1", "ag1", "find x")
        + &agent_lines("toolu_2", "ag2", "fix y")
        + &agent_lines("toolu_3", "ag3", "plan z");
    fs::write(&log, body).expect("write fixture log");
    log
}

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("f".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

fn claude_busy() -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state: AgentState::Busy,
    }
}

fn claude_meta(log: Option<&Path>) -> TabMeta {
    let cwd = PathBuf::from("/Users/tester/IdeaProjects/terminal");
    TabMeta {
        agent: Some(claude_busy()),
        graph_agent: Some(claude_busy()),
        cwd: Some(cwd.clone()),
        graph_agent_cwd: Some(cwd),
        branch: Some("main".to_string()),
        log: log.map(Path::to_path_buf),
        ..TabMeta::default()
    }
}

fn row(id: u64) -> SidebarRow {
    SidebarRow {
        tab_id: id,
        title: format!("t{id}"),
        ..SidebarRow::default()
    }
}

fn publish_rows(ids: &[u64]) {
    TabModelHandle::shared().set(TabBarModel {
        titles: ids.iter().copied().map(row).collect(),
        ..TabBarModel::default()
    });
}

fn snapshot(log: &Path) -> TabMetaSnapshot {
    TabMetaSnapshot::for_test(
        vec![
            (TabId(TAB), claude_meta(Some(log))),
            (TabId(BARE), claude_meta(None)),
        ],
        true,
    )
}

fn toggle(overlay: &mut FlowOverlay) {
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 2, 0, None);
    assert_eq!(overlay.on_key(&cmd_shift_f(), &mut ctx), Handled::Consumed);
}

fn find_row(model: &OverviewModel, tab: u64) -> Option<SessionRowData> {
    model
        .rows
        .iter()
        .find(|r| r.tab == tab && r.pane == 0)
        .cloned()
}

/// Poll the overlay until its published model holds tab [`TAB`] with three subagents.
fn wait_for_subagents(overlay: &mut FlowOverlay, handle: &OverviewHandle) -> SessionRowData {
    let start = Instant::now();
    loop {
        overlay.poll();
        if let Some(r) = find_row(&handle.model(), TAB) {
            if r.tree.len() == 3 {
                return r;
            }
        }
        if start.elapsed() > DEADLINE {
            panic!(
                "timed out waiting for tab {TAB}'s three subagents; model = {:#?}",
                handle.model()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Lay the published model out, publish its hits as a frame would, and return the centre of
/// `(TAB, 0)`'s chevron. Also asserts the subagent-less row has no chevron hit (R5.3).
fn publish_frame_and_find_chevron(handle: &OverviewHandle) -> [f32; 2] {
    let layout = layout_overview(&handle.model(), VIEWPORT, 1.0, handle.scroll());
    assert!(
        !layout
            .hits
            .iter()
            .any(|(h, _)| *h == OverviewHit::Expand(BARE, 0)),
        "a row with no subagents has no chevron hit: {:#?}",
        layout.hits
    );
    let rect = layout
        .hits
        .iter()
        .find(|(h, _)| *h == OverviewHit::Expand(TAB, 0))
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("no chevron hit for ({TAB}, 0): {:#?}", layout.hits));
    let max = layout.max_scroll();
    handle.publish(layout.hits, max);
    let point = [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0];
    assert_eq!(
        handle.hit_at(point),
        Some(OverviewHit::Expand(TAB, 0)),
        "precondition: the handle resolves the chevron point"
    );
    point
}

fn press(overlay: &mut FlowOverlay, point: [f32; 2]) {
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 2, 0, None);
    assert_eq!(overlay.on_press(point, &mut ctx), Handled::Consumed);
    assert!(overlay.visible(), "a chevron press keeps the overlay open");
}

/// An open overlay with tabs 3 (three subagents) and 4 (none), once tab 3's digest landed.
fn open_with_subagents(log: &Path) -> (FlowOverlay, OverviewHandle) {
    publish_rows(&[TAB, BARE]);
    let handle = OverviewHandle::new();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    toggle(&mut overlay);
    assert!(overlay.visible(), "fixture: the chord opened the overlay");
    overlay.sync_snapshot(snapshot(log));
    let r = wait_for_subagents(&mut overlay, &handle);
    assert!(!r.expanded, "fixture: rows start collapsed");
    (overlay, handle)
}

// ---- R5.3 ---------------------------------------------------------------------------------

#[test]
fn r5_3_chevron_press_toggles_the_tree() {
    let dir = TempDir::new("r5-3");
    let log = three_subagent_log(&dir);
    let (mut overlay, handle) = open_with_subagents(&log);

    // First press: expanded, with three full tree lines.
    let point = publish_frame_and_find_chevron(&handle);
    press(&mut overlay, point);
    overlay.poll();
    let r = find_row(&handle.model(), TAB).expect("tab 3's row after the first press");
    assert!(r.expanded, "(3, 0) is in the expanded set after one press");
    assert_eq!(r.tree.len(), 3, "three tree lines: {:#?}", r.tree);
    for (i, line) in r.tree.iter().enumerate() {
        assert_eq!(line.subagent_type, "Explore", "line {i}: type");
        assert_eq!(line.status_word, "running", "line {i}: status word");
        assert!(!line.duration.is_empty(), "line {i}: a duration is shown");
    }
    let descs: Vec<&str> = r.tree.iter().map(|l| l.description.as_str()).collect();
    assert_eq!(
        descs,
        ["find x", "fix y", "plan z"],
        "descriptions in log order"
    );

    // Second press: collapsed again.
    let point = publish_frame_and_find_chevron(&handle);
    press(&mut overlay, point);
    overlay.poll();
    let r = find_row(&handle.model(), TAB).expect("tab 3's row after the second press");
    assert!(
        !r.expanded,
        "a second press removes (3, 0) from the expanded set"
    );
}

// ---- R5.5 (reopen half) -------------------------------------------------------------------

#[test]
fn r5_5_expanded_row_survives_close_and_reopen() {
    let dir = TempDir::new("r5-5");
    let log = three_subagent_log(&dir);
    let (mut overlay, handle) = open_with_subagents(&log);

    let point = publish_frame_and_find_chevron(&handle);
    press(&mut overlay, point);
    overlay.poll();
    assert!(
        find_row(&handle.model(), TAB).is_some_and(|r| r.expanded),
        "precondition: (3, 0) expanded"
    );

    // Close, then reopen.
    toggle(&mut overlay);
    assert!(!overlay.visible(), "the chord closed the overlay");
    toggle(&mut overlay);
    assert!(overlay.visible(), "the chord reopened the overlay");
    overlay.sync_snapshot(snapshot(&log));
    let r = wait_for_subagents(&mut overlay, &handle);
    assert!(r.expanded, "after reopen (3, 0) is still expanded");
}

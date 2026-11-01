//! TT8 — acceptance test for `specs/session-overview-ui` R8.5's press (D10, D13): in a
//! session's detail, a press on lane 2 (`OverviewHit::Lane(2)`) moves the selection from
//! the default newest lane (6) to lane 2, and the panel follows.
//!
//! Drives a real `FlowOverlay` built over T8's `renderer::overview_panel::OverviewHandle`
//! (`FlowOverlay::new(wake, None, handle)`), so this file does not compile until T8 switches
//! the overlay from the old graph's `FlowHandle` — that is its red state. Everything else in
//! TT8 lives in files that compile now; the selection data itself is also covered by
//! `tt8_overview_detail.rs` through `build_detail(.., Some(2), ..)`.
//!
//! The session is one unsplit Claude tab whose log (a temp file) holds seven finished
//! synchronous `Agent` calls, all inside the last hour, so the detail has seven lanes. The
//! overlay's flow telemetry thread folds that file; no tabmeta worker runs (`None`), the
//! snapshot is stood in through `sync_snapshot`. Presses resolve through hits published on
//! the handle — the same seam the draw layer uses after every composed frame.

use std::fs;
use std::io::Sink;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::{
    Agent, AgentKind, AgentState, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, RepoId, TabId,
    TabMeta, TabMetaSnapshot,
};
use renderer::overview_model::{DetailData, OverviewHit, View};
use renderer::overview_panel::OverviewHandle;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

const TAB: u64 = 1;
const DEADLINE: Duration = Duration::from_secs(5);
const POINT: [f32; 2] = [150.0, 130.0];
const RECT: [f32; 4] = [100.0, 100.0, 200.0, 60.0];

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tt8-overview-overlay-{tag}-{}-{}",
            std::process::id(),
            now_ms()
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for epoch `ms` (days → civil date, proleptic Gregorian).
fn iso(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

/// A human prompt at `t`, then seven synchronous `Agent` calls `task 0` … `task 6`, the
/// i-th started at `t + 2 + 5i` minutes and answered one minute later.
fn seven_agent_log(t: u64) -> String {
    let mut s = format!(
        r#"{{"type":"user","timestamp":"{}","sessionId":"sess-r85","origin":{{"kind":"human"}},"message":{{"role":"user","content":"review everything"}}}}"#,
        iso(t)
    ) + "\n";
    for i in 0..7u64 {
        let start = t + (2 + 5 * i) * 60_000;
        let id = format!("toolu_{i}");
        s += &format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"sess-r85","message":{{"id":"msg_{id}","role":"assistant","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"description":"task {i}","subagent_type":"Explore","prompt":"go"}}}}]}}}}"#,
            ts = iso(start)
        );
        s += "\n";
        s += &format!(
            r#"{{"type":"user","timestamp":"{ts}","sessionId":"sess-r85","toolUseResult":{{"status":"completed","agentId":"agent{i}"}},"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"report {i}"}}]}}}}"#,
            ts = iso(start + 60_000)
        );
        s += "\n";
    }
    s
}

fn cmd_shift_f() -> KeyInput {
    KeyInput {
        key: Key::Character("F".into()),
        text: None,
        mods: ModifiersState::SUPER | ModifiersState::SHIFT,
        physical: PhysicalKey::Code(KeyCode::KeyF),
    }
}

fn claude_meta(log: PathBuf) -> TabMeta {
    let agent = Agent {
        kind: AgentKind::Claude,
        state: AgentState::Busy,
    };
    let root = PathBuf::from("/Users/tester/IdeaProjects/terminal");
    TabMeta {
        agent: Some(agent),
        graph_agent: Some(agent),
        cwd: Some(root.clone()),
        graph_agent_cwd: Some(root.clone()),
        branch: Some("main".to_string()),
        log: Some(log),
        repo: Some(RepoId {
            common: root.join(".git"),
            main_root: root,
            linked: false,
        }),
        ..TabMeta::default()
    }
}

fn press(overlay: &mut FlowOverlay, handle: &OverviewHandle, hit: OverviewHit) {
    handle.publish(vec![(hit, RECT)], 0);
    assert_eq!(
        handle.hit_at(POINT),
        Some(hit),
        "precondition: the handle resolves the point"
    );
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
    assert_eq!(overlay.on_press(POINT, &mut ctx), Handled::Consumed);
    overlay.poll();
}

fn detail(handle: &OverviewHandle) -> Option<DetailData> {
    match handle.model().view {
        View::Detail(d) => Some(*d),
        View::Overview => None,
    }
}

/// R8.5: the detail opens with lane 6 (the newest) selected; a press on lane 2 selects
/// lane 2 and the panel shows its subagent.
#[test]
fn r8_5_press_on_lane_two_selects_it() {
    let dir = TempDir::new("r8-5");
    let log = dir.0.join("session.jsonl");
    let t0 = now_ms() - 45 * 60_000;
    fs::write(&log, seven_agent_log(t0)).expect("write fixture");

    TabModelHandle::shared().set(TabBarModel {
        titles: vec![SidebarRow {
            tab_id: TAB,
            title: "terminal".to_string(),
            ..SidebarRow::default()
        }],
        ..TabBarModel::default()
    });
    let handle = OverviewHandle::new();
    let mut overlay = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    let mut sink: Sink = std::io::sink();
    let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
    overlay.on_key(&cmd_shift_f(), &mut ctx);
    assert!(
        overlay.visible(),
        "precondition: the chord opened the overlay"
    );
    overlay.sync_snapshot(TabMetaSnapshot::for_test(
        vec![(TabId(TAB), claude_meta(log))],
        true,
    ));

    // Wait for the telemetry thread to fold the log into the row.
    let start = Instant::now();
    loop {
        overlay.poll();
        let m = handle.model();
        if m.rows.len() == 1 && m.rows[0].tree.len() == 7 {
            break;
        }
        assert!(
            start.elapsed() < DEADLINE,
            "precondition: the row with seven subagents; model = {m:#?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let pane = handle.model().rows[0].pane;

    press(&mut overlay, &handle, OverviewHit::Row(TAB, pane));
    let d = detail(&handle).expect("a row press opened the detail");
    assert_eq!(d.lanes.len(), 7, "seven lanes: {:#?}", d.lanes);
    assert_eq!(d.selected, Some(6), "the newest lane is selected first");
    assert_eq!(d.panel.as_ref().map(|p| p.title.as_str()), Some("task 6"));

    press(&mut overlay, &handle, OverviewHit::Lane(2));
    let d = detail(&handle).expect("still in the detail");
    assert_eq!(d.selected, Some(2), "the pressed lane is selected");
    let selected: Vec<bool> = d.lanes.iter().map(|l| l.selected).collect();
    assert_eq!(selected, [false, false, true, false, false, false, false]);
    assert_eq!(
        d.panel.as_ref().map(|p| p.title.as_str()),
        Some("task 2"),
        "the panel shows lane 2's subagent"
    );
    assert!(overlay.visible(), "a lane press keeps the overlay open");
}

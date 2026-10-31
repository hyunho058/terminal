//! TT7 — acceptance tests for `specs/session-overview-ui` R7 (D10, D20, D22): navigation
//! between the overview, the detail and the terminal — presses, keys and the hits they land on.
//!
//! Every test drives a real `FlowOverlay` built over the renderer's `OverviewHandle` (the T8
//! constructor; this file does not compile until T8 switches the overlay over). No GPU frame
//! runs here, so the test stands in for the draw layer: it publishes the hit boxes a frame
//! would have published (`OverviewHandle::publish`) and presses a point inside one of them,
//! exactly the seam `tt9_graph_click.rs` used for the old graph. R7.8 publishes the hits the
//! real `layout_detail` computes for the overlay's own detail model.
//!
//! The sessions come from the same two sources the overlay reads in the app: the sidebar rows on
//! this test thread's `TabModelHandle::shared()` and a `TabMetaSnapshot` stood in through
//! `sync_snapshot` (no tabmeta worker). A Claude session's digest comes from a real log file the
//! overlay's own telemetry thread folds.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::{
    Agent, AgentKind, AgentState, AppCtx, AppRequest, FlowOverlay, Handled, KeyInput, Overlay,
    TabId, TabMeta, TabMetaSnapshot, WheelDelta,
};
use renderer::overview_layout::layout_detail;
use renderer::overview_model::{DetailData, Filter, OverviewHit, OverviewModel, View};
use renderer::overview_panel::OverviewHandle;
use renderer::sidebar_row::{SidebarRow, TabBarModel, TabModelHandle};
use winit::keyboard::{Key, KeyCode, ModifiersState as M, NamedKey, PhysicalKey};

const DEADLINE: Duration = Duration::from_secs(5);
const CELL_H: f32 = 16.0;
/// A max scroll large enough that publishing hits never clamps the scroll under test.
const MAX_SCROLL: usize = 10;

// ---- hit boxes a frame would publish (physical px, scale 1) ------------------

const ROW: [f32; 4] = [0.0, 200.0, 1400.0, 56.0];
const EXPAND: [f32; 4] = [1300.0, 206.0, 44.0, 44.0];
const OPEN: [f32; 4] = [1350.0, 206.0, 44.0, 44.0];
const BANNER: [f32; 4] = [1100.0, 80.0, 200.0, 30.0];
const FILTER_WORKING: [f32; 4] = [400.0, 140.0, 100.0, 30.0];
const BACK: [f32; 4] = [20.0, 20.0, 140.0, 30.0];
const FOCUS: [f32; 4] = [1200.0, 20.0, 150.0, 36.0];

/// Inside the row, outside both of its buttons.
const ROW_POINT: [f32; 2] = [300.0, 228.0];

fn centre(r: [f32; 4]) -> [f32; 2] {
    [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
}

// ---- keys --------------------------------------------------------------------

fn character(ch: &str, mods: M, code: KeyCode) -> KeyInput {
    KeyInput {
        key: Key::Character(ch.into()),
        text: if mods.super_key() {
            None
        } else {
            Some(ch.into())
        },
        mods,
        physical: PhysicalKey::Code(code),
    }
}

fn named(key: NamedKey, text: Option<&str>, code: KeyCode) -> KeyInput {
    KeyInput {
        key: Key::Named(key),
        text: text.map(str::to_string),
        mods: M::empty(),
        physical: PhysicalKey::Code(code),
    }
}

fn cmd_shift_f() -> KeyInput {
    character("F", M::SUPER | M::SHIFT, KeyCode::KeyF)
}

fn esc() -> KeyInput {
    named(NamedKey::Escape, Some("\u{1b}"), KeyCode::Escape)
}

fn enter() -> KeyInput {
    named(NamedKey::Enter, Some("\r"), KeyCode::Enter)
}

fn plain_a() -> KeyInput {
    character("a", M::empty(), KeyCode::KeyA)
}

fn cmd_t() -> KeyInput {
    character("t", M::SUPER, KeyCode::KeyT)
}

// ---- dispatch ----------------------------------------------------------------

fn key(o: &mut FlowOverlay, input: &KeyInput) -> (Handled, Vec<AppRequest>) {
    let mut pty: Vec<u8> = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    let handled = o.on_key(input, &mut ctx);
    (handled, ctx.take_requests())
}

fn press(o: &mut FlowOverlay, point: [f32; 2]) -> (Handled, Vec<AppRequest>) {
    let mut pty: Vec<u8> = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    let handled = o.on_press(point, &mut ctx);
    (handled, ctx.take_requests())
}

fn activations(requests: &[AppRequest]) -> Vec<AppRequest> {
    requests
        .iter()
        .filter(|r| matches!(r, AppRequest::ActivateTab(_)))
        .cloned()
        .collect()
}

// ---- sessions ----------------------------------------------------------------

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tt7-overview-overlay-{tag}-{}-{nanos}",
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

/// A Claude Code log of session `abc-123`: a human prompt, then a background `Agent` launch —
/// one running subagent.
fn write_claude_log(path: &Path) {
    let lines = [
        r#"{"type":"user","timestamp":"2026-10-31T10:00:00.000Z","sessionId":"abc-123","origin":{"kind":"human"},"message":{"role":"user","content":"scan the repo"}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-31T10:01:00.000Z","sessionId":"abc-123","message":{"id":"msg_bg","role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"bg","name":"Agent","input":{"subagent_type":"Explore","description":"Background scan"}}]}}"#,
        r#"{"type":"user","timestamp":"2026-10-31T10:01:01.000Z","sessionId":"abc-123","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"aee2b85e39714d85f"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg","content":"Async agent launched successfully."}]}}"#,
    ];
    fs::write(path, lines.join("\n") + "\n").expect("write the Claude log");
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// An unsplit tab running `kind`, working (`Busy` → the Working bucket).
fn meta(kind: AgentKind, log: Option<&Path>) -> TabMeta {
    let agent = Some(Agent {
        kind,
        state: AgentState::Busy,
    });
    let cwd = std::env::temp_dir().join("tt7-overview-project");
    TabMeta {
        agent,
        graph_agent: agent,
        cwd: Some(cwd.clone()),
        graph_agent_cwd: Some(cwd),
        log: log.map(Path::to_path_buf),
        ..TabMeta::default()
    }
}

/// Publish `tabs` as this thread's sidebar rows (what the overlay reads in the app).
fn publish_rows(tabs: &[u64]) {
    TabModelHandle::shared().set(TabBarModel {
        titles: tabs.iter().map(|&t| row(t)).collect(),
        active: 0,
        ..TabBarModel::default()
    });
}

/// The test's world: which tabs exist and what the worker published for them.
struct World {
    tabs: Vec<(u64, TabMeta)>,
}

impl World {
    fn snapshot(&self) -> TabMetaSnapshot {
        TabMetaSnapshot::for_test(
            self.tabs
                .iter()
                .map(|(t, m)| (TabId(*t), m.clone()))
                .collect(),
            true,
        )
    }

    /// Make the rows and the snapshot say what `self` says (no rebuild).
    fn stand_in(&self, o: &mut FlowOverlay) {
        let ids: Vec<u64> = self.tabs.iter().map(|(t, _)| *t).collect();
        publish_rows(&ids);
        o.sync_snapshot(self.snapshot());
    }
}

/// Poll until `pred` holds on the handle's model, or fail with `what`.
fn pump(
    o: &mut FlowOverlay,
    h: &OverviewHandle,
    what: &str,
    pred: impl Fn(&OverviewModel) -> bool,
) {
    let start = Instant::now();
    loop {
        o.poll();
        if pred(&h.model()) {
            return;
        }
        if start.elapsed() > DEADLINE {
            panic!("precondition not reached within {DEADLINE:?}: {what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn has_row(m: &OverviewModel, tab: u64, pane: u32) -> bool {
    m.rows.iter().any(|r| r.tab == tab && r.pane == pane)
}

fn detail(m: &OverviewModel) -> Option<&DetailData> {
    match &m.view {
        View::Detail(d) => Some(d),
        View::Overview => None,
    }
}

/// Open the overlay over `world` and pump until the overview shows `(tab, 0)`.
fn open(o: &mut FlowOverlay, h: &OverviewHandle, world: &World, tab: u64) {
    let (handled, _) = key(o, &cmd_shift_f());
    assert_eq!(handled, Handled::Consumed, "precondition: the chord opens");
    assert!(o.visible(), "precondition: the overlay is open");
    world.stand_in(o);
    pump(o, h, "the overview shows the session's row", |m| {
        has_row(m, tab, 0) && m.view == View::Overview
    });
}

/// A new, closed overlay over a fresh handle.
fn overlay() -> (FlowOverlay, OverviewHandle) {
    let handle = OverviewHandle::new();
    let o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
    (o, handle)
}

/// The overview's hits for the row `(tab, 0)`: the row, then its two buttons above it.
fn publish_row_hits(h: &OverviewHandle, tab: u64) {
    h.publish(
        vec![
            (OverviewHit::Filter(Filter::Working), FILTER_WORKING),
            (OverviewHit::Banner(tab, 0), BANNER),
            (OverviewHit::Row(tab, 0), ROW),
            (OverviewHit::Expand(tab, 0), EXPAND),
            (OverviewHit::Open(tab, 0), OPEN),
        ],
        MAX_SCROLL,
    );
}

/// The detail's header hits for `(tab, 0)`.
fn publish_detail_hits(h: &OverviewHandle, tab: u64) {
    h.publish(
        vec![
            (OverviewHit::Back, BACK),
            (OverviewHit::Focus(tab, 0), FOCUS),
        ],
        MAX_SCROLL,
    );
}

/// From the overview, press `(tab, 0)`'s row and pump until the detail is for it.
fn enter_detail(o: &mut FlowOverlay, h: &OverviewHandle, tab: u64) {
    publish_row_hits(h, tab);
    let (handled, _) = press(o, ROW_POINT);
    assert_eq!(handled, Handled::Consumed);
    pump(o, h, "the row press opened the detail", |m| {
        detail(m).is_some_and(|d| d.tab == tab && d.pane == 0)
    });
}

fn claude_world(dir: &TempDir) -> World {
    let log = dir.path("abc-123.jsonl");
    write_claude_log(&log);
    World {
        tabs: vec![(4, meta(AgentKind::Claude, Some(&log)))],
    }
}

// ---- R7.1 --------------------------------------------------------------------

#[test]
fn r7_1_row_press_outside_its_buttons_opens_the_detail() {
    let dir = TempDir::new("r7-1");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    publish_row_hits(&h, 4);
    assert_eq!(
        h.hit_at(ROW_POINT),
        Some(OverviewHit::Row(4, 0)),
        "precondition: the point is inside the row and outside both buttons"
    );

    let (handled, requests) = press(&mut o, ROW_POINT);
    o.poll();

    assert_eq!(handled, Handled::Consumed);
    let model = h.model();
    let d = detail(&model).expect("the view is Detail after a row press");
    assert_eq!((d.tab, d.pane), (4, 0), "the detail is for (4, 0)");
    assert!(
        activations(&requests).is_empty(),
        "a row press requested a tab switch: {requests:?}"
    );
    assert!(o.visible(), "the overlay stays open");
}

// ---- R7.2 --------------------------------------------------------------------

#[test]
fn r7_2_open_banner_and_focus_switch_to_the_tab_and_close() {
    let dir = TempDir::new("r7-2");

    // The row's `Open` button, in the overview.
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    publish_row_hits(&h, 4);
    let (handled, requests) = press(&mut o, centre(OPEN));
    assert_eq!(handled, Handled::Consumed, "Open: consumed");
    assert_eq!(
        activations(&requests),
        vec![AppRequest::ActivateTab(TabId(4))],
        "Open: asks to activate tab 4"
    );
    assert!(!o.visible(), "Open: the overlay closes");

    // The banner's `Open session →`, in the overview.
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    publish_row_hits(&h, 4);
    let (handled, requests) = press(&mut o, centre(BANNER));
    assert_eq!(handled, Handled::Consumed, "Banner: consumed");
    assert_eq!(
        activations(&requests),
        vec![AppRequest::ActivateTab(TabId(4))],
        "Banner: asks to activate tab 4"
    );
    assert!(!o.visible(), "Banner: the overlay closes");

    // The detail's `Focus terminal`.
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    enter_detail(&mut o, &h, 4);
    publish_detail_hits(&h, 4);
    let (handled, requests) = press(&mut o, centre(FOCUS));
    assert_eq!(handled, Handled::Consumed, "Focus: consumed");
    assert_eq!(
        activations(&requests),
        vec![AppRequest::ActivateTab(TabId(4))],
        "Focus: asks to activate tab 4"
    );
    assert!(!o.visible(), "Focus: the overlay closes");
}

// ---- R7.3 --------------------------------------------------------------------

#[test]
fn r7_3_keys_by_view() {
    let dir = TempDir::new("r7-3");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    enter_detail(&mut o, &h, 4);

    // Every other key is swallowed in the detail.
    for (name, k) in [("a", plain_a()), ("Enter", enter()), ("Cmd+T", cmd_t())] {
        let (handled, requests) = key(&mut o, &k);
        o.poll();
        assert_eq!(
            handled,
            Handled::Consumed,
            "detail: {name} was not swallowed"
        );
        assert!(requests.is_empty(), "detail: {name} queued {requests:?}");
        assert!(o.visible(), "detail: {name} closed the overlay");
        assert!(
            detail(&h.model()).is_some(),
            "detail: {name} left the detail"
        );
    }

    // `Esc` in the detail returns to the overview.
    let (handled, _) = key(&mut o, &esc());
    o.poll();
    assert_eq!(handled, Handled::Consumed);
    assert!(o.visible(), "Esc in the detail must not close the overlay");
    assert_eq!(
        h.model().view,
        View::Overview,
        "Esc in the detail returns to the overview"
    );

    // Every other key is swallowed in the overview.
    for (name, k) in [("a", plain_a()), ("Enter", enter()), ("Cmd+T", cmd_t())] {
        let (handled, requests) = key(&mut o, &k);
        assert_eq!(
            handled,
            Handled::Consumed,
            "overview: {name} was not swallowed"
        );
        assert!(requests.is_empty(), "overview: {name} queued {requests:?}");
        assert!(o.visible(), "overview: {name} closed the overlay");
    }

    // From the detail again, `Cmd+Shift+F` closes the overlay.
    enter_detail(&mut o, &h, 4);
    let (handled, _) = key(&mut o, &cmd_shift_f());
    assert_eq!(handled, Handled::Consumed);
    assert!(!o.visible(), "Cmd+Shift+F in the detail closes the overlay");

    // In the overview, `Esc` closes it.
    open(&mut o, &h, &world, 4);
    let (handled, _) = key(&mut o, &esc());
    assert_eq!(handled, Handled::Consumed);
    assert!(!o.visible(), "Esc in the overview closes the overlay");
}

// ---- R7.4 --------------------------------------------------------------------

#[test]
fn r7_4_back_returns_to_the_overview_with_filter_expanded_set_and_scroll() {
    let dir = TempDir::new("r7-4");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    pump(&mut o, &h, "the row carries its subagent", |m| {
        m.rows
            .iter()
            .any(|r| r.tab == 4 && r.pane == 0 && !r.subagents.is_empty())
    });

    // A non-default filter, the row expanded, the table scrolled.
    publish_row_hits(&h, 4);
    press(&mut o, centre(FILTER_WORKING));
    publish_row_hits(&h, 4);
    press(&mut o, centre(EXPAND));
    pump(&mut o, &h, "filter Working and (4, 0) expanded", |m| {
        m.filter == Filter::Working
            && m.rows
                .iter()
                .any(|r| r.tab == 4 && r.pane == 0 && r.expanded)
    });
    publish_row_hits(&h, 4);
    o.on_wheel(WheelDelta::Lines(-3.0), CELL_H);
    let scrolled = h.scroll();
    assert!(scrolled > 0, "precondition: the table is scrolled");

    // Into the detail, then `‹ All sessions`.
    enter_detail(&mut o, &h, 4);
    publish_detail_hits(&h, 4);
    assert_eq!(h.hit_at(centre(BACK)), Some(OverviewHit::Back));
    let (handled, requests) = press(&mut o, centre(BACK));
    o.poll();

    assert_eq!(handled, Handled::Consumed);
    assert!(
        activations(&requests).is_empty(),
        "Back requested {requests:?}"
    );
    assert!(o.visible(), "Back keeps the overlay open");
    let model = h.model();
    assert_eq!(model.view, View::Overview, "Back returns to the overview");
    assert_eq!(model.filter, Filter::Working, "the prior filter is kept");
    assert!(
        model
            .rows
            .iter()
            .any(|r| r.tab == 4 && r.pane == 0 && r.expanded),
        "the expanded set is kept: {:?}",
        model.rows
    );
    assert_eq!(h.scroll(), scrolled, "the table's scroll is kept");
}

// ---- R7.5 --------------------------------------------------------------------

#[test]
fn r7_5_a_vanished_session_returns_the_view_to_the_overview() {
    let dir = TempDir::new("r7-5");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    enter_detail(&mut o, &h, 4);

    // Tab 4 closes.
    World { tabs: Vec::new() }.stand_in(&mut o);
    o.poll();

    assert_eq!(h.model().view, View::Overview);
}

// ---- R7.6 --------------------------------------------------------------------

#[test]
fn r7_6_a_press_on_a_closed_tab_does_nothing() {
    let dir = TempDir::new("r7-6");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    publish_row_hits(&h, 4);

    // Tab 4 closes before a rebuild: no poll, and the published hits still name it.
    World { tabs: Vec::new() }.stand_in(&mut o);
    assert_eq!(
        h.hit_at(centre(OPEN)),
        Some(OverviewHit::Open(4, 0)),
        "precondition: the stale hit is still published"
    );

    let (handled, requests) = press(&mut o, centre(OPEN));

    assert_eq!(handled, Handled::Consumed);
    assert!(
        activations(&requests).is_empty(),
        "a stale press requested a tab switch: {requests:?}"
    );
    assert!(o.visible(), "a stale press leaves the overlay open");
}

// ---- R7.7 --------------------------------------------------------------------

#[test]
fn r7_7_reopen_resets_the_view_and_every_scroll() {
    let dir = TempDir::new("r7-7");
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);

    // The table scrolled, then the detail's lanes scrolled.
    publish_row_hits(&h, 4);
    o.on_wheel(WheelDelta::Lines(-3.0), CELL_H);
    assert!(h.scroll() > 0, "precondition: the table is scrolled");
    enter_detail(&mut o, &h, 4);
    publish_detail_hits(&h, 4);
    o.on_wheel(WheelDelta::Lines(-3.0), CELL_H);
    assert!(h.scroll() > 0, "precondition: the lanes are scrolled");

    // Close and reopen.
    let (handled, _) = key(&mut o, &cmd_shift_f());
    assert_eq!(handled, Handled::Consumed);
    assert!(!o.visible(), "precondition: the chord closed the overlay");
    let (handled, _) = key(&mut o, &cmd_shift_f());
    assert_eq!(handled, Handled::Consumed);
    assert!(o.visible(), "precondition: the chord reopened the overlay");
    world.stand_in(&mut o);
    pump(&mut o, &h, "the reopened overview shows the row", |m| {
        has_row(m, 4, 0)
    });

    assert_eq!(
        h.model().view,
        View::Overview,
        "a reopen shows the overview"
    );
    assert_eq!(h.scroll(), 0, "a reopen puts the table at the top");

    // The lanes, and the table the detail returns to, start at the top too.
    enter_detail(&mut o, &h, 4);
    assert_eq!(h.scroll(), 0, "a reopen puts the lanes at the top");
    publish_detail_hits(&h, 4);
    press(&mut o, centre(BACK));
    o.poll();
    assert_eq!(h.model().view, View::Overview, "precondition: Back");
    assert_eq!(
        h.scroll(),
        0,
        "the table scroll saved before the close does not survive it"
    );
}

// ---- R7.8 --------------------------------------------------------------------

const VIEWPORT: [f32; 2] = [1440.0, 900.0];

#[test]
fn r7_8_copy_resume_puts_the_claude_command_on_the_clipboard() {
    let dir = TempDir::new("r7-8");

    // A Claude session with session_id `abc-123`.
    let world = claude_world(&dir);
    let (mut o, h) = overlay();
    open(&mut o, &h, &world, 4);
    enter_detail(&mut o, &h, 4);
    pump(&mut o, &h, "the detail knows the session id", |m| {
        detail(m).is_some_and(|d| d.resume_cmd.is_some())
    });
    let model = h.model();
    let d = detail(&model).expect("the Claude detail");
    let layout = layout_detail(d, VIEWPORT, 1.0, 0);
    let copy = layout
        .hits
        .iter()
        .find(|(hit, _)| *hit == OverviewHit::CopyResume)
        .map(|(_, r)| *r)
        .expect("the Claude detail publishes a CopyResume hit");
    h.publish(layout.hits.clone(), layout.max_scroll());

    let (handled, requests) = press(&mut o, centre(copy));

    assert_eq!(handled, Handled::Consumed);
    assert!(
        requests.contains(&AppRequest::CopyToClipboard(
            "claude --resume abc-123".to_string()
        )),
        "CopyResume pushed {requests:?}"
    );

    // A Codex session: its detail publishes no CopyResume hit.
    let codex = World {
        tabs: vec![(5, meta(AgentKind::Codex, None))],
    };
    let (mut o, h) = overlay();
    open(&mut o, &h, &codex, 5);
    enter_detail(&mut o, &h, 5);
    let model = h.model();
    let d = detail(&model).expect("the Codex detail");
    let layout = layout_detail(d, VIEWPORT, 1.0, 0);
    assert!(
        !layout
            .hits
            .iter()
            .any(|(hit, _)| *hit == OverviewHit::CopyResume),
        "the Codex detail published a CopyResume hit: {:?}",
        layout.hits
    );
}

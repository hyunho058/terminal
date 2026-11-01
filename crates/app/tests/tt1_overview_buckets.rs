//! TT1 — acceptance tests for `specs/session-overview-ui` R1 (D2, D14): status buckets,
//! counts, order and which rows exist.
//!
//! Every fixture is hand-built data — `SidebarRow`s and `TabMeta`s the way the graph's own
//! acceptance tests build them (`tt5_graph_shape.rs`, `tt6_graph_status.rs`): no tmux, no
//! process table, no worker. The resolved `CardStatus` of a session is fixed through its
//! inputs (`resolve_status`: `Busy` → `Working`, `Idle` → `Ready`, `Waiting` →
//! `WaitingForInput`, `Unknown` → `Hidden`; a blocking dialog forces `WaitingForInput`).
//!
//! R1.6 (the worker publishing `graph_asks_user` / `graph_agent_cwd` on graph passes only) is
//! not here: nothing reachable from `tests/` can drive the worker over an Antigravity tab
//! (see the TT1 report).
//!
//! The pill labels (R1.1) and the header's `5 sessions` (R1.3) are drawn text, so those two
//! tests read them from `renderer::overview_layout::layout_overview` over the model
//! `build_overview` returns; every other test reads the model alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::SessionTelemetry;
use app::overview::{build_overview, ExpandSet};
use app::{resolve_status, Agent, AgentKind, AgentState, PaneMeta, SessionDigest, TabId, TabMeta};
use renderer::overview_layout::{layout_overview, OverviewLayout};
use renderer::overview_model::{Bucket, Counts, Filter, OverviewHit, OverviewModel};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;

/// 2026-10-31T10:30:00Z.
const NOW_MS: u64 = 1_793_404_800_000 + 10 * 3_600_000 + 30 * 60_000;

/// A large viewport at scale 1, so no D15 narrow-window rule drops anything.
const VIEWPORT: [f32; 2] = [1600.0, 1400.0];

fn agent(kind: AgentKind, state: AgentState) -> Agent {
    Agent { kind, state }
}

fn row(tab: u64, blocking_dialog: bool) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        blocking_dialog,
        ..Default::default()
    }
}

/// An unsplit tab's meta with `a` as its agent, on a graph pass (the graph's own copy set too).
fn meta(a: Option<Agent>, asks_user: bool) -> TabMeta {
    TabMeta {
        agent: a,
        graph_agent: a,
        graph_asks_user: asks_user,
        ..Default::default()
    }
}

fn pane(pid: u32, active: bool, a: Option<Agent>, asks_user: bool) -> PaneMeta {
    PaneMeta {
        graph_transcript: None,
        pane_pid: pid,
        active,
        agent: a,
        log: None,
        cwd: None,
        branch: None,
        repo: None,
        graph_asks_user: asks_user,
        graph_agent_cwd: None,
    }
}

/// `build_overview` over `rows` and their metas: no home, filter `All`, nothing expanded,
/// the first row active.
fn overview(
    rows: &[SidebarRow],
    metas: Vec<(u64, TabMeta)>,
    digests: &HashMap<PathBuf, SessionDigest>,
) -> OverviewModel {
    let metas: HashMap<TabId, TabMeta> = metas.into_iter().map(|(t, m)| (TabId(t), m)).collect();
    let (model, _) = build_overview(
        rows,
        &metas,
        None,
        digests,
        &ExpandSet::new(),
        Filter::All,
        0,
        NOW_MS,
    );
    model
}

fn bucket_of(model: &OverviewModel, tab: u64, pane: u32) -> Option<Bucket> {
    model
        .rows
        .iter()
        .find(|r| r.tab == tab && r.pane == pane)
        .map(|r| r.bucket)
}

/// R1.2's five sessions, in strip order: Idle (tab 1), NeedsYou (tab 2), Working (tab 3),
/// Working (tab 4), Agent (tab 5).
fn five_sessions() -> (Vec<SidebarRow>, Vec<(u64, TabMeta)>) {
    let claude = |s| Some(agent(AgentKind::Claude, s));
    let rows = vec![
        row(1, false),
        row(2, true),
        row(3, false),
        row(4, false),
        row(5, false),
    ];
    let metas = vec![
        (1, meta(claude(AgentState::Idle), false)),
        // A permission dialog on screen: `WaitingForInput` + `blocking_dialog`.
        (2, meta(claude(AgentState::Waiting), false)),
        (3, meta(claude(AgentState::Busy), false)),
        (4, meta(claude(AgentState::Busy), false)),
        (5, meta(claude(AgentState::Unknown), false)),
    ];
    (rows, metas)
}

/// The status pill's label drawn inside the row `(tab, pane)`: the leftmost text in the row's
/// `OverviewHit::Row` rect that is one of the four pill labels.
fn pill_label(layout: &OverviewLayout, tab: u64, pane: u32) -> Option<String> {
    const LABELS: [&str; 4] = ["Needs you", "Working", "Idle", "Agent"];
    let [rx, ry, rw, rh] = layout.hits.iter().find_map(|(hit, rect)| match hit {
        OverviewHit::Row(t, p) if *t == tab && *p == pane => Some(*rect),
        _ => None,
    })?;
    layout
        .texts
        .iter()
        .filter(|t| LABELS.contains(&t.text.as_str()))
        .filter(|t| {
            let cx = t.rect[0] + t.rect[2] / 2.0;
            let cy = t.rect[1] + t.rect[3] / 2.0;
            cx >= rx && cx < rx + rw && cy >= ry && cy < ry + rh
        })
        .min_by(|a, b| a.rect[0].total_cmp(&b.rect[0]))
        .map(|t| t.text.clone())
}

/// R1.1's six sessions, one per tab 1..=6: (a) `WaitingForInput` + `blocking_dialog`,
/// (b) `WaitingForInput` + `graph_asks_user`, (c) `WaitingForInput` with neither,
/// (d) `Working`, (e) `Ready`, (f) `Hidden`.
fn six_sessions() -> (Vec<SidebarRow>, Vec<(u64, TabMeta)>) {
    let claude = |s| Some(agent(AgentKind::Claude, s));
    let agy = |s| Some(agent(AgentKind::Antigravity, s));
    let rows = vec![
        row(1, true),
        row(2, false),
        row(3, false),
        row(4, false),
        row(5, false),
        row(6, false),
    ];
    let metas = vec![
        (1, meta(claude(AgentState::Waiting), false)),
        (2, meta(agy(AgentState::Waiting), true)),
        (3, meta(claude(AgentState::Waiting), false)),
        (4, meta(claude(AgentState::Busy), false)),
        (5, meta(claude(AgentState::Idle), false)),
        (6, meta(claude(AgentState::Unknown), false)),
    ];
    // The fixture really is R1.1's six resolved statuses.
    let expected = [
        CardStatus::WaitingForInput,
        CardStatus::WaitingForInput,
        CardStatus::WaitingForInput,
        CardStatus::Working,
        CardStatus::Ready,
        CardStatus::Hidden,
    ];
    for ((r, (_, m)), want) in rows.iter().zip(&metas).zip(expected) {
        assert_eq!(
            resolve_status(m.agent.as_ref(), r.blocking_dialog, false, None),
            want,
            "fixture for tab {}",
            r.tab_id
        );
    }
    (rows, metas)
}

// R1.1
#[test]
fn r1_1_card_status_and_flags_map_to_buckets() {
    let (rows, metas) = six_sessions();
    let model = overview(&rows, metas, &HashMap::new());
    let got: Vec<Option<Bucket>> = (1..=6).map(|t| bucket_of(&model, t, 0)).collect();
    assert_eq!(
        got,
        vec![
            Some(Bucket::NeedsYou),
            Some(Bucket::NeedsYou),
            Some(Bucket::Idle),
            Some(Bucket::Working),
            Some(Bucket::Idle),
            Some(Bucket::Agent),
        ],
        "buckets of (a) dialog, (b) asks-user, (c) neither, (d) Working, (e) Ready, (f) Hidden"
    );
}

// R1.1
#[test]
fn r1_1_pill_labels_name_each_bucket() {
    let (rows, metas) = six_sessions();
    let model = overview(&rows, metas, &HashMap::new());
    let layout = layout_overview(&model, VIEWPORT, 1.0, 0);
    let got: Vec<Option<String>> = (1..=6).map(|t| pill_label(&layout, t, 0)).collect();
    let want: Vec<Option<String>> = ["Needs you", "Needs you", "Idle", "Working", "Idle", "Agent"]
        .iter()
        .map(|s| Some(s.to_string()))
        .collect();
    assert_eq!(got, want, "pill labels of tabs 1..=6");
}

// R1.5
#[test]
fn r1_5_finished_claude_turn_is_idle_not_needs_you() {
    // The session log: a human prompt, then an assistant `end_turn`.
    let log = PathBuf::from("/tmp/tt1-overview/finished-turn.jsonl");
    let lines = [
        r#"{"type":"user","timestamp":"2026-10-31T10:20:00.000Z","sessionId":"s","origin":{"kind":"human"},"message":{"role":"user","content":"fix the build"}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-31T10:21:00.000Z","sessionId":"s","message":{"id":"msg_end","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}"#,
    ];
    let mut telemetry = SessionTelemetry::new().with_digest();
    for l in lines {
        telemetry.apply(l);
    }
    let mut digests = HashMap::new();
    digests.insert(
        log.clone(),
        SessionDigest::from_snapshot(telemetry.snapshot()),
    );

    // `end_turn` is what the worker publishes as `Waiting`; no dialog on screen.
    let r = row(1, false);
    let m = TabMeta {
        log: Some(log),
        ..meta(Some(agent(AgentKind::Claude, AgentState::Waiting)), false)
    };
    assert_eq!(
        resolve_status(m.agent.as_ref(), false, false, None),
        CardStatus::WaitingForInput,
        "the sidebar card reads Waiting for input"
    );
    let model = overview(std::slice::from_ref(&r), vec![(1, m)], &digests);

    assert_eq!(model.rows.len(), 1, "one row for the Claude tab");
    assert_eq!(model.rows[0].bucket, Bucket::Idle);
    assert_eq!(model.counts.needs_you, 0);
    assert_eq!(model.banner, None);
}

// R1.7
#[test]
fn r1_7_non_active_pane_asking_the_user_is_needs_you() {
    let claude_busy = Some(agent(AgentKind::Claude, AgentState::Busy));
    let agy_waiting = Some(agent(AgentKind::Antigravity, AgentState::Waiting));
    let r = row(1, false);
    let m = TabMeta {
        panes: vec![
            pane(10, true, claude_busy, false),
            pane(11, false, agy_waiting, true),
            pane(12, false, agy_waiting, false),
        ],
        ..meta(claude_busy, false)
    };
    let model = overview(std::slice::from_ref(&r), vec![(1, m)], &HashMap::new());
    assert_eq!(
        bucket_of(&model, 1, 11),
        Some(Bucket::NeedsYou),
        "non-active pane 11: Waiting with graph_asks_user"
    );
    assert_eq!(
        bucket_of(&model, 1, 12),
        Some(Bucket::Idle),
        "non-active pane 12: Waiting without the flag"
    );
}

// R1.2
#[test]
fn r1_2_rows_sort_by_bucket_then_session_order() {
    let (rows, metas) = five_sessions();
    let model = overview(&rows, metas, &HashMap::new());
    let order: Vec<u64> = model.rows.iter().map(|r| r.tab).collect();
    assert_eq!(order, vec![2, 3, 4, 1, 5]);
}

// R1.3
#[test]
fn r1_3_counts_over_the_five_sessions() {
    let (rows, metas) = five_sessions();
    let model = overview(&rows, metas, &HashMap::new());
    assert_eq!(
        model.counts,
        Counts {
            all: 5,
            needs_you: 1,
            working: 2,
            idle: 1,
        }
    );
    let agent_rows = model
        .rows
        .iter()
        .filter(|r| r.bucket == Bucket::Agent)
        .count();
    assert_eq!(
        model.counts.needs_you + model.counts.working + model.counts.idle + agent_rows,
        5,
        "needs-you + working + idle + agent-row count"
    );
}

// R1.3
#[test]
fn r1_3_header_reads_5_sessions() {
    let (rows, metas) = five_sessions();
    let model = overview(&rows, metas, &HashMap::new());
    let layout = layout_overview(&model, VIEWPORT, 1.0, 0);
    assert!(
        layout.texts.iter().any(|t| t.text.contains("5 sessions")),
        "no drawn text reads `5 sessions`; texts: {:?}",
        layout.texts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
}

// R1.4
#[test]
fn r1_4_only_agent_sessions_are_rows() {
    let claude_busy = agent(AgentKind::Claude, AgentState::Busy);
    let codex_busy = agent(AgentKind::Codex, AgentState::Busy);
    // The split tab shows a dialog on its active pane's screen: the non-active pane must not
    // take it.
    let rows = vec![row(1, false), row(2, false), row(3, true)];
    let zsh = TabMeta {
        cwd: Some(Path::new("/tmp/tt1-overview/zsh").to_path_buf()),
        ..meta(None, false)
    };
    let split = TabMeta {
        panes: vec![
            pane(30, true, Some(claude_busy), false),
            pane(31, false, Some(codex_busy), false),
        ],
        ..meta(Some(claude_busy), false)
    };
    let metas = vec![(1, zsh), (2, meta(Some(claude_busy), false)), (3, split)];
    let model = overview(&rows, metas, &HashMap::new());

    assert_eq!(model.rows.len(), 3, "Claude tab + two pane sessions");
    assert!(
        model.rows.iter().all(|r| r.tab != 1),
        "the plain zsh tab has no row"
    );
    let mut keys: Vec<(u64, u32)> = model.rows.iter().map(|r| (r.tab, r.pane)).collect();
    keys.sort();
    assert_eq!(keys, vec![(2, 0), (3, 30), (3, 31)]);

    // The non-active pane's status is resolve_status(pane.agent, false, false, None)'s bucket.
    assert_eq!(
        resolve_status(Some(&codex_busy), false, false, None),
        CardStatus::Working
    );
    assert_eq!(bucket_of(&model, 3, 31), Some(Bucket::Working));
}

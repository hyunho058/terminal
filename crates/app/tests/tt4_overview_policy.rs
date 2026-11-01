//! TT4 — acceptance tests for `specs/session-overview-ui` R4.1–R4.3 and R4.6 (D5, D6, D8):
//! the `Waiting on you` banner, the filter tabs and the subagents tile, as the app's pure
//! policy (`app::overview`) builds them.
//!
//! Written from the spec's Thens by the checker side; the implementation (T5) makes them
//! pass without editing them. Every fixture is hand-built data: no tmux, no logs, no
//! process table. R4.3's filter labels are rendered text, so that one assertion goes through
//! the renderer's `overview_layout::layout_overview` (the label is `{name} {count}`, D6).
//! R4.4 lives in `crates/renderer/tests/tt4_overview_empty_filter.rs`, R4.5 (which drives the
//! overlay) in `tt4_overview_overlay.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::overview::{build_overview, pick_banner, ExpandSet};
use app::{Agent, AgentKind, AgentState, SessionDigest, TabId, TabMeta};
use renderer::overview_layout::{layout_overview, OverviewLayout};
use renderer::overview_model::{
    AgentTag, Bucket, DialogSubject, Filter, OverviewHit, OverviewModel, SessionRowData,
    SubagentTotals,
};
use renderer::sidebar_row::SidebarRow;

/// Any fixed wall clock; nothing in R4 reads elapsed text.
const NOW_MS: u64 = 1_790_000_000_000;

fn home() -> PathBuf {
    PathBuf::from("/Users/tester")
}

fn repo(name: &str) -> app::RepoId {
    app::RepoId {
        common: PathBuf::from(format!("/Users/tester/IdeaProjects/{name}/.git")),
        main_root: PathBuf::from(format!("/Users/tester/IdeaProjects/{name}")),
        linked: false,
    }
}

fn claude(state: AgentState) -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state,
    }
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// A row whose screen shows a permission dialog for `cmd` — with a `Waiting` agent, the
/// card reads `WaitingForInput` and the dialog makes it Needs you (D2).
fn dialog_row(tab: u64, cmd: &str) -> SidebarRow {
    SidebarRow {
        blocking_dialog: true,
        blocking_subject: Some(DialogSubject::Command(cmd.to_string())),
        ..row(tab)
    }
}

/// An unsplit Claude tab in repository `name`, in `state`, optionally reading `log`.
fn meta(state: AgentState, name: &str, log: Option<&Path>) -> TabMeta {
    let cwd = PathBuf::from(format!("/Users/tester/IdeaProjects/{name}"));
    TabMeta {
        agent: Some(claude(state)),
        graph_agent: Some(claude(state)),
        cwd: Some(cwd.clone()),
        graph_agent_cwd: Some(cwd),
        branch: Some("main".to_string()),
        log: log.map(Path::to_path_buf),
        repo: Some(repo(name)),
        ..TabMeta::default()
    }
}

fn build(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    digests: &HashMap<PathBuf, SessionDigest>,
    filter: Filter,
) -> OverviewModel {
    let home = home();
    let (model, _) = build_overview(
        rows,
        metas,
        Some(home.as_path()),
        digests,
        &ExpandSet::new(),
        filter,
        0,
        NOW_MS,
    );
    model
}

fn tabs_of(model: &OverviewModel) -> Vec<u64> {
    model.rows.iter().map(|r| r.tab).collect()
}

// ---------------------------------------------------------------------------------------
// R4.1

fn needs_you_row(
    tab: u64,
    project: &str,
    agent_label: &str,
    agent: AgentTag,
    doing: &str,
) -> SessionRowData {
    SessionRowData {
        tab,
        pane: 0,
        bucket: Bucket::NeedsYou,
        project: project.to_string(),
        agent_label: agent_label.to_string(),
        agent,
        now_doing: doing.to_string(),
        ..SessionRowData::default()
    }
}

fn plain_row(tab: u64, bucket: Bucket, project: &str) -> SessionRowData {
    SessionRowData {
        tab,
        pane: 0,
        bucket,
        project: project.to_string(),
        agent_label: "Claude Code".to_string(),
        agent: AgentTag::Claude,
        now_doing: "Turn finished · ready for next prompt".to_string(),
        ..SessionRowData::default()
    }
}

// R4.1
#[test]
fn r4_1_banner_picks_first_waiting_row() {
    // Given: sorted rows with two Needs-you sessions, Claude `receipt-tracker` first.
    let rows = vec![
        needs_you_row(
            4,
            "receipt-tracker",
            "Claude Code",
            AgentTag::Claude,
            "Approval needed · ./gradlew test",
        ),
        needs_you_row(
            2,
            "terminal",
            "Antigravity",
            AgentTag::Antigravity,
            "Waiting for your input",
        ),
        plain_row(1, Bucket::Working, "infra"),
        plain_row(3, Bucket::Idle, "docs"),
    ];

    let banner = pick_banner(&rows).expect("two Needs-you rows must produce a banner");

    assert_eq!(
        (banner.tab, banner.pane),
        (4, 0),
        "target is the Claude row"
    );
    assert_eq!(
        banner.text,
        "Claude Code · receipt-tracker — Approval needed · ./gradlew test"
    );
    // The ` (+1 more)` suffix: one other row waits.
    assert_eq!(banner.more, 1, "suffix ` (+1 more)`");
}

// R4.1
#[test]
fn r4_1_no_waiting_row_means_no_banner() {
    let rows = vec![
        plain_row(1, Bucket::Working, "infra"),
        plain_row(3, Bucket::Idle, "docs"),
        plain_row(5, Bucket::Agent, "misc"),
    ];
    assert_eq!(pick_banner(&rows), None);
}

// ---------------------------------------------------------------------------------------
// R4.2

// R4.2
#[test]
fn r4_2_banner_ignores_the_filter() {
    // Given: filter Idle, one Needs-you session (tab 2) and one Idle session (tab 1).
    let rows = vec![row(1), dialog_row(2, "./gradlew test")];
    let mut metas = HashMap::new();
    metas.insert(TabId(1), meta(AgentState::Idle, "docs", None));
    metas.insert(TabId(2), meta(AgentState::Waiting, "receipt-tracker", None));

    let model = build(&rows, &metas, &HashMap::new(), Filter::Idle);

    let banner = model
        .banner
        .as_ref()
        .expect("the banner is present under the Idle filter");
    assert_eq!(banner.tab, 2, "the banner names the Needs-you session");
    assert_eq!(
        tabs_of(&model),
        vec![1],
        "the table holds only the Idle row"
    );
    assert!(
        model.rows.iter().all(|r| r.bucket == Bucket::Idle),
        "every table row is Idle: {:?}",
        model.rows.iter().map(|r| r.bucket).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------------------
// R4.3

/// R1.2's five sessions, in strip order: Idle(tab 1), NeedsYou(tab 2), Working(tab 3),
/// Working(tab 4), Agent(tab 5).
fn five_sessions() -> (Vec<SidebarRow>, HashMap<TabId, TabMeta>) {
    let rows = vec![
        row(1),
        dialog_row(2, "./gradlew test"),
        row(3),
        row(4),
        row(5),
    ];
    let mut metas = HashMap::new();
    metas.insert(TabId(1), meta(AgentState::Idle, "one", None));
    metas.insert(TabId(2), meta(AgentState::Waiting, "two", None));
    metas.insert(TabId(3), meta(AgentState::Busy, "three", None));
    metas.insert(TabId(4), meta(AgentState::Busy, "four", None));
    metas.insert(TabId(5), meta(AgentState::Unknown, "five", None));
    (rows, metas)
}

/// The label drawn inside filter tab `f`'s hit box: every text run whose centre lies in
/// that box, left to right, trimmed and joined by one space — so `All 5` reads the same
/// whether it was laid out as one run or as a name and a count.
fn filter_label(layout: &OverviewLayout, f: Filter) -> String {
    let Some((_, [x, y, w, h])) = layout
        .hits
        .iter()
        .find(|(hit, _)| *hit == OverviewHit::Filter(f))
        .copied()
    else {
        return format!("<no Filter({f:?}) hit>");
    };
    let mut runs: Vec<(f32, String)> = layout
        .texts
        .iter()
        .filter(|t| {
            let cx = t.rect[0] + t.rect[2] / 2.0;
            let cy = t.rect[1] + t.rect[3] / 2.0;
            cx >= x && cx < x + w && cy >= y && cy < y + h
        })
        .map(|t| (t.rect[0], t.text.trim().to_string()))
        .filter(|(_, s)| !s.is_empty())
        .collect();
    runs.sort_by(|a, b| a.0.total_cmp(&b.0));
    runs.into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join(" ")
}

// R4.3
#[test]
fn r4_3_filter_restricts_rows_not_tiles() {
    let (rows, metas) = five_sessions();

    let model = build(&rows, &metas, &HashMap::new(), Filter::Working);

    assert_eq!(tabs_of(&model), vec![3, 4], "rows are tab 3 and tab 4");
    // Tile counts are still 1/2/1.
    assert_eq!(model.counts.needs_you, 1, "Needs you tile");
    assert_eq!(model.counts.working, 2, "Working tile");
    assert_eq!(model.counts.idle, 1, "Idle tile");

    // Filter labels read `All 5`, `Needs you 1`, `Working 2`, `Idle 1`.
    let layout = layout_overview(&model, [1600.0, 1000.0], 1.0, 0);
    assert_eq!(filter_label(&layout, Filter::All), "All 5");
    assert_eq!(filter_label(&layout, Filter::NeedsYou), "Needs you 1");
    assert_eq!(filter_label(&layout, Filter::Working), "Working 2");
    assert_eq!(filter_label(&layout, Filter::Idle), "Idle 1");
}

// ---------------------------------------------------------------------------------------
// R4.6

fn subagent(id: &str, completed: bool, ended_ok: bool) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: None,
        description: None,
        model: None,
        start_ms: None,
        end_ms: None,
        completed,
        agent_id: None,
        ended_ok,
        report_preview: None,
    }
}

fn digest(subagents: Vec<Subagent>) -> SessionDigest {
    SessionDigest {
        subagents,
        ..SessionDigest::default()
    }
}

fn log_path(name: &str) -> PathBuf {
    PathBuf::from(format!("/nonexistent/tt4/{name}.jsonl"))
}

// R4.6
#[test]
fn r4_6_subagent_tile_totals() {
    // Given: sessions with subagents: 2 running + 1 done; 7 done; 1 failed.
    let (a, b, c) = (log_path("a"), log_path("b"), log_path("c"));
    let rows = vec![row(1), row(2), row(3)];
    let mut metas = HashMap::new();
    metas.insert(TabId(1), meta(AgentState::Busy, "one", Some(&a)));
    metas.insert(TabId(2), meta(AgentState::Busy, "two", Some(&b)));
    metas.insert(TabId(3), meta(AgentState::Busy, "three", Some(&c)));
    let mut digests = HashMap::new();
    digests.insert(
        a.clone(),
        digest(vec![
            subagent("a1", false, true),
            subagent("a2", true, true),
            subagent("a3", false, true),
        ]),
    );
    digests.insert(
        b.clone(),
        digest(
            (0..7)
                .map(|i| subagent(&format!("b{i}"), true, true))
                .collect(),
        ),
    );
    digests.insert(c.clone(), digest(vec![subagent("c1", true, false)]));

    let model = build(&rows, &metas, &digests, Filter::All);
    let t = model.total_subagents;

    assert_eq!(
        t,
        SubagentTotals {
            running: 2,
            done: 8,
            failed: 1,
        },
        "three disjoint counts: running, done ok, failed"
    );
    // The tile reads running 2, done 9 (failed counted in done).
    assert_eq!(t.running, 2, "tile: running");
    assert_eq!(t.done + t.failed, 9, "tile: done, failed counted in");
    // Bar fractions: running 2/11, done 8/11, failed 1/11.
    let total = t.running + t.done + t.failed;
    assert_eq!(total, 11, "bar denominator");
    assert_eq!((t.running, total), (2, 11), "running fraction");
    assert_eq!((t.done, total), (8, 11), "done fraction");
    assert_eq!((t.failed, total), (1, 11), "failed fraction");
}

//! TT5 — acceptance tests for `specs/session-overview-ui` R5 (D8): subagent squares, the
//! summary, the tree's durations and fallbacks, and the expanded set's pruning — the policy
//! half, through `app::overview`'s pure functions.
//!
//! R5.2 (layout `+k`) lives in `crates/renderer/tests/tt5_overview_squares.rs`; R5.3 and the
//! reopen half of R5.5 drive `FlowOverlay` in `tt5_overview_overlay.rs`.
//!
//! Every fixture is hand-built data: no worker, no tmux, no log file (log paths are lookup
//! keys into the digest map and are never opened).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::overview::{build_overview, subagent_summary, ExpandSet};
use app::{Agent, AgentKind, AgentState, SessionDigest, TabId, TabMeta};
use renderer::overview_model::{
    Filter, OverviewModel, SessionRowData, SubagentCell, SubagentStatus,
};
use renderer::sidebar_row::SidebarRow;

/// A fixed "now": 2026-10-31T00:00:00Z.
const NOW_MS: u64 = 1_793_404_800_000;

fn home() -> PathBuf {
    PathBuf::from("/Users/tester")
}

fn log_path(name: &str) -> PathBuf {
    PathBuf::from(format!("/nonexistent/tt5-overview/{name}.jsonl"))
}

fn sub(
    id: &str,
    ty: Option<&str>,
    desc: Option<&str>,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    completed: bool,
    ended_ok: bool,
) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: ty.map(str::to_string),
        description: desc.map(str::to_string),
        model: None,
        start_ms,
        end_ms,
        completed,
        agent_id: None,
        ended_ok,
        report_preview: None,
    }
}

fn running(id: &str) -> Subagent {
    sub(
        id,
        Some("Explore"),
        Some("find x"),
        Some(NOW_MS - 60_000),
        None,
        false,
        true,
    )
}

fn done(id: &str) -> Subagent {
    sub(
        id,
        Some("Explore"),
        Some("find y"),
        Some(NOW_MS - 120_000),
        Some(NOW_MS - 60_000),
        true,
        true,
    )
}

fn failed(id: &str) -> Subagent {
    sub(
        id,
        Some("Plan"),
        Some("plan z"),
        Some(NOW_MS - 120_000),
        Some(NOW_MS - 60_000),
        true,
        false,
    )
}

fn claude_busy() -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state: AgentState::Busy,
    }
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: "terminal".to_string(),
        ..SidebarRow::default()
    }
}

/// An unsplit Claude tab reading `log`, as a graph pass publishes it.
fn claude_meta(log: &Path) -> TabMeta {
    let cwd = PathBuf::from("/Users/tester/IdeaProjects/terminal");
    TabMeta {
        agent: Some(claude_busy()),
        graph_agent: Some(claude_busy()),
        cwd: Some(cwd.clone()),
        graph_agent_cwd: Some(cwd),
        branch: Some("main".to_string()),
        log: Some(log.to_path_buf()),
        ..TabMeta::default()
    }
}

fn digest_of(subagents: Vec<Subagent>) -> SessionDigest {
    SessionDigest {
        subagents,
        ..SessionDigest::default()
    }
}

fn build(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    digests: &HashMap<PathBuf, SessionDigest>,
    expanded: &ExpandSet,
) -> (OverviewModel, ExpandSet) {
    let home = home();
    build_overview(
        rows,
        metas,
        Some(home.as_path()),
        digests,
        expanded,
        Filter::All,
        0,
        NOW_MS,
    )
}

fn row_of(model: &OverviewModel, tab: u64, pane: u32) -> &SessionRowData {
    model
        .rows
        .iter()
        .find(|r| r.tab == tab && r.pane == pane)
        .unwrap_or_else(|| panic!("no row ({tab}, {pane}) in {:#?}", model.rows))
}

// --- R5.1 -------------------------------------------------------------------------------

#[test]
fn r5_1_squares_follow_log_order_and_summary_counts_failed_as_done() {
    let (cells, summary) = subagent_summary(&[running("a"), done("b"), failed("c")], NOW_MS);
    assert_eq!(
        cells,
        vec![
            SubagentCell::Square(SubagentStatus::Running),
            SubagentCell::Square(SubagentStatus::Done),
            SubagentCell::Square(SubagentStatus::Failed),
        ],
        "one square per subagent in log order: running, done, failed"
    );
    assert_eq!(summary, "1 running · 2 done");
}

#[test]
fn r5_1_summary_without_a_running_subagent_omits_the_running_part() {
    let (_, summary) = subagent_summary(&[done("b"), failed("c")], NOW_MS);
    assert_eq!(summary, "2 done");
}

#[test]
fn r5_1_no_subagents_is_a_dash() {
    let (cells, summary) = subagent_summary(&[], NOW_MS);
    assert!(cells.is_empty(), "no squares with no subagents: {cells:?}");
    assert_eq!(summary, "—");
}

// --- R5.4 -------------------------------------------------------------------------------

#[test]
fn r5_4_tree_durations_and_fallbacks() {
    const TAB: u64 = 3;
    let log = log_path("r5-4");
    let subagents = vec![
        // Finished, 171 s long.
        sub(
            "fin",
            Some("Explore"),
            Some("find x"),
            Some(NOW_MS - 600_000),
            Some(NOW_MS - 600_000 + 171_000),
            true,
            true,
        ),
        // Running, started 4 min 10 s ago.
        sub(
            "run",
            Some("Plan"),
            Some("plan it"),
            Some(NOW_MS - 250_000),
            None,
            false,
            true,
        ),
        // No start_ms, no type, no description.
        sub("unk", None, None, None, None, false, true),
    ];
    let rows = vec![row(TAB)];
    let metas = HashMap::from([(TabId(TAB), claude_meta(&log))]);
    let digests = HashMap::from([(log.clone(), digest_of(subagents))]);
    let expanded: ExpandSet = HashSet::from([(TabId(TAB), 0)]);

    let (model, _) = build(&rows, &metas, &digests, &expanded);

    let r = row_of(&model, TAB, 0);
    assert_eq!(r.tree.len(), 3, "one tree line per subagent: {:#?}", r.tree);
    let durations: Vec<&str> = r.tree.iter().map(|l| l.duration.as_str()).collect();
    assert_eq!(durations, ["2m 51s", "4m", "—"]);
    assert_eq!(
        r.tree[2].subagent_type, "agent",
        "type falls back to `agent`"
    );
    assert_eq!(r.tree[2].description, "—", "description falls back to `—`");
}

// --- R5.5 (pruning half) ----------------------------------------------------------------

#[test]
fn r5_5_expanded_set_is_kept_while_the_session_exists_and_pruned_once_it_is_gone() {
    const TAB: u64 = 3;
    const OTHER: u64 = 4;
    let log3 = log_path("r5-5-tab3");
    let log4 = log_path("r5-5-tab4");
    let metas = HashMap::from([
        (TabId(TAB), claude_meta(&log3)),
        (TabId(OTHER), claude_meta(&log4)),
    ]);
    let digests = HashMap::from([
        (
            log3.clone(),
            digest_of(vec![running("a"), done("b"), failed("c")]),
        ),
        (log4.clone(), digest_of(vec![running("d")])),
    ]);
    let expanded: ExpandSet = HashSet::from([(TabId(TAB), 0)]);

    // Tab 3 still open: the rebuild keeps (3, 0).
    let both = vec![row(TAB), row(OTHER)];
    let (model, kept) = build(&both, &metas, &digests, &expanded);
    assert!(
        kept.contains(&(TabId(TAB), 0)),
        "(3, 0) survives a rebuild while tab 3 exists: {kept:?}"
    );
    assert!(row_of(&model, TAB, 0).expanded, "and its row is expanded");

    // Tab 3 closed: the rebuild drops it.
    let only_other = vec![row(OTHER)];
    let mut metas_after = metas.clone();
    metas_after.remove(&TabId(TAB));
    let (_, pruned) = build(&only_other, &metas_after, &digests, &kept);
    assert!(
        !pruned.contains(&(TabId(TAB), 0)),
        "after tab 3 closes the set no longer holds (3, 0): {pruned:?}"
    );
}

//! TT11 — acceptance tests for `specs/session-overview-ui` R11.5 and R11.6 (D24): an
//! Antigravity session whose transcript the worker published as `TabMeta.graph_transcript`
//! gets its digest looked up under that path, so its row shows squares, the summary and
//! elapsed, and its detail has a timeline — exactly as a Claude row does — while
//! `Copy resume cmd` stays Claude-only.
//!
//! Every fixture is hand-built data: no worker, no tmux, no transcript on disk (the path is
//! a lookup key into the digest map and is never opened).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::overview::{build_overview, ExpandSet};
use app::overview_detail::build_detail;
use app::{Agent, AgentKind, AgentState, SessionDigest, TabId, TabMeta};
use renderer::overview_model::{
    AgentTag, Filter, OverviewModel, SessionRowData, SubagentCell, SubagentStatus,
};
use renderer::sidebar_row::SidebarRow;

/// A fixed "now": 2026-10-31T00:00:00Z.
const NOW_MS: u64 = 1_793_404_800_000;
const AGY_TAB: u64 = 7;
const CODEX_TAB: u64 = 8;

fn home() -> PathBuf {
    PathBuf::from("/Users/tester")
}

fn transcript() -> PathBuf {
    PathBuf::from(
        "/nonexistent/tt11/.gemini/antigravity/brain/c5fdd0df-a369-4025-a618-7905f77cbd04/.system_generated/logs/transcript.jsonl",
    )
}

fn sub(id: &str, start_ms: u64, end_ms: Option<u64>, completed: bool) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: Some("research".to_string()),
        description: Some(format!("review part {id}")),
        model: None,
        start_ms: Some(start_ms),
        end_ms,
        completed,
        agent_id: None,
        ended_ok: true,
        report_preview: None,
    }
}

/// Two subagents (newest first, the ring's order): one running, one done; the latest
/// human prompt 3 minutes before now. A session id is present so the resume command's
/// Claude-only rule is actually exercised.
fn agy_digest() -> SessionDigest {
    SessionDigest {
        subagents: vec![
            sub("run", NOW_MS - 120_000, None, false),
            sub("fin", NOW_MS - 170_000, Some(NOW_MS - 130_000), true),
        ],
        last_prompt_ms: Some(NOW_MS - 180_000),
        first_event_ms: Some(NOW_MS - 180_000),
        session_id: Some("c5fdd0df-a369-4025-a618-7905f77cbd04".to_string()),
        ..SessionDigest::default()
    }
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: "webapp".to_string(),
        ..SidebarRow::default()
    }
}

fn agent_meta(kind: AgentKind, transcript: Option<&Path>) -> TabMeta {
    let a = Agent {
        kind,
        state: AgentState::Busy,
    };
    let cwd = PathBuf::from("/Users/tester/IdeaProjects/webapp");
    TabMeta {
        agent: Some(a),
        graph_agent: Some(a),
        cwd: Some(cwd.clone()),
        graph_agent_cwd: Some(cwd),
        branch: Some("main".to_string()),
        graph_transcript: transcript.map(Path::to_path_buf),
        ..TabMeta::default()
    }
}

fn build() -> OverviewModel {
    let path = transcript();
    let rows = vec![row(AGY_TAB), row(CODEX_TAB)];
    let metas = HashMap::from([
        (
            TabId(AGY_TAB),
            agent_meta(AgentKind::Antigravity, Some(&path)),
        ),
        (TabId(CODEX_TAB), agent_meta(AgentKind::Codex, None)),
    ]);
    let digests = HashMap::from([(path, agy_digest())]);
    let expanded: ExpandSet = HashSet::new();
    let home = home();
    let (model, _) = build_overview(
        &rows,
        &metas,
        Some(home.as_path()),
        &digests,
        &expanded,
        Filter::All,
        0,
        NOW_MS,
    );
    model
}

fn row_of(model: &OverviewModel, tab: u64) -> &SessionRowData {
    model
        .rows
        .iter()
        .find(|r| r.tab == tab)
        .unwrap_or_else(|| panic!("no row for tab {tab} in {:#?}", model.rows))
}

// --- R11.5 ------------------------------------------------------------------------------

#[test]
fn r11_5_antigravity_row_shows_subagents_and_elapsed() {
    let model = build();
    let r = row_of(&model, AGY_TAB);
    assert_eq!(r.agent, AgentTag::Antigravity);
    assert_eq!(
        r.subagents.len(),
        2,
        "one square per subagent: {:?}",
        r.subagents
    );
    assert!(
        r.subagents
            .iter()
            .all(|c| matches!(c, SubagentCell::Square(_))),
        "two squares, no `+k`: {:?}",
        r.subagents
    );
    assert!(r
        .subagents
        .contains(&SubagentCell::Square(SubagentStatus::Running)));
    assert!(r
        .subagents
        .contains(&SubagentCell::Square(SubagentStatus::Done)));
    assert_eq!(r.subagent_summary, "1 running · 1 done");
    assert_eq!(r.elapsed, "3m");
}

// --- R11.6 ------------------------------------------------------------------------------

#[test]
fn r11_6_antigravity_detail_has_a_timeline_and_no_resume_cmd() {
    let model = build();
    let digest = agy_digest();
    let d = build_detail(row_of(&model, AGY_TAB), Some(&digest), None, NOW_MS, 0);
    assert!(d.has_timeline, "an Antigravity digest gives a timeline");
    assert_eq!(d.lanes.len(), 2, "one lane per subagent: {:#?}", d.lanes);
    assert_eq!(d.resume_cmd, None, "`Copy resume cmd` stays Claude-only");
}

#[test]
fn r11_6_codex_row_without_a_digest_has_no_timeline() {
    let model = build();
    let d = build_detail(row_of(&model, CODEX_TAB), None, None, NOW_MS, 0);
    assert!(
        !d.has_timeline,
        "a Codex row with no digest has no timeline"
    );
}

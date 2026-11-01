//! TT14 — acceptance tests for `specs/session-flow-graph` R14 (amendment 2026-10-21), carried
//! by `specs/session-overview-ui` (R14 is kept): every agent the sidebar detects — Claude,
//! Antigravity, Codex, Gemini, Aider — is a session row of the Agent Monitor overview, while
//! subagents stay Claude-only (D8).
//!
//! Everything is hand-built data handed to `app::overview::build_overview`; nothing reads the
//! live machine, and the log paths are never opened.
//!
//! R14.5 (the worker resolves `repo` for any agent kind) is not here: the worker's per-tab
//! derive path is not public — `graph_pass`, `publish_graph_state` and `repo_of` are the only
//! worker-side exports, and none takes a detected agent — so it is left to T5's in-crate test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::overview::{bucket_of, build_overview, ExpandSet};
use app::{
    resolve_status, row_meta, Agent, AgentKind, AgentState, PaneMeta, RepoId, SessionDigest, TabId,
    TabMeta,
};
use renderer::overview_model::{AgentTag, Bucket, Counts, Filter, OverviewModel, SessionRowData};
use renderer::sidebar_row::SidebarRow;

// ---- fixtures ---------------------------------------------------------------

const HOME: &str = "/Users/u";
const REPO: &str = "IdeaProjects/terminal";

fn home() -> PathBuf {
    PathBuf::from(HOME)
}

/// Repository R, seen from its main worktree.
fn repo() -> RepoId {
    let main_root = home().join(REPO);
    RepoId {
        common: main_root.join(".git"),
        main_root,
        linked: false,
    }
}

/// R seen from one of its linked worktrees.
fn linked() -> RepoId {
    RepoId {
        linked: true,
        ..repo()
    }
}

fn agent(kind: AgentKind, state: AgentState) -> Option<Agent> {
    Some(Agent { kind, state })
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// An unsplit tab in R's main worktree running `agent`.
fn unsplit(agent: Option<Agent>, log: Option<&Path>) -> TabMeta {
    TabMeta {
        agent,
        cwd: Some(home().join(REPO)),
        branch: Some("main".to_string()),
        log: log.map(Path::to_path_buf),
        repo: agent.map(|_| repo()),
        ..TabMeta::default()
    }
}

fn metas(entries: Vec<(u64, TabMeta)>) -> HashMap<TabId, TabMeta> {
    entries.into_iter().map(|(t, m)| (TabId(t), m)).collect()
}

fn build(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    digests: &HashMap<PathBuf, SessionDigest>,
) -> OverviewModel {
    let h = home();
    let (model, _expanded) = build_overview(
        rows,
        metas,
        Some(h.as_path()),
        digests,
        &ExpandSet::new(),
        Filter::All,
        0,
        1_793_404_800_000,
    );
    model
}

fn row_of(m: &OverviewModel, tab: u64) -> &SessionRowData {
    m.rows
        .iter()
        .find(|r| r.tab == tab)
        .unwrap_or_else(|| panic!("a session row for tab {tab}: {:#?}", m.rows))
}

fn subagent(id: &str, completed: bool) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: None,
        description: None,
        model: None,
        start_ms: None,
        end_ms: None,
        completed,
        agent_id: None,
        ended_ok: true,
        report_preview: None,
    }
}

fn digest(subagents: Vec<Subagent>) -> SessionDigest {
    SessionDigest {
        subagents,
        ..SessionDigest::default()
    }
}

// ---- R14.1 ------------------------------------------------------------------

/// R14.1: Antigravity, Codex, Gemini and Aider tabs are four session rows of repository R, in
/// tab order; the agentless tab 5 has no row; each row's state is its card's `row_meta`.
#[test]
fn r14_1_non_claude_agents_are_session_rows_of_their_repository() {
    let kinds = [
        (AgentKind::Antigravity, "Antigravity", AgentTag::Antigravity),
        (AgentKind::Codex, "Codex", AgentTag::Codex),
        (AgentKind::Gemini, "Gemini", AgentTag::Gemini),
        (AgentKind::Aider, "Aider", AgentTag::Aider),
    ];
    let rows: Vec<SidebarRow> = (1..=5).map(row).collect();
    let mut entries: Vec<(u64, TabMeta)> = kinds
        .iter()
        .zip(1u64..)
        .map(|(&(k, _, _), t)| (t, unsplit(agent(k, AgentState::Busy), None)))
        .collect();
    entries.push((5, unsplit(None, None)));
    let metas = metas(entries);

    let m = build(&rows, &metas, &HashMap::new());

    let tabs: Vec<(u64, u32)> = m.rows.iter().map(|r| (r.tab, r.pane)).collect();
    assert_eq!(
        tabs,
        vec![(1, 0), (2, 0), (3, 0), (4, 0)],
        "tabs 1–4 in tab order; no row for tab 5: {:#?}",
        m.rows
    );
    for (t, &(_, label, tag)) in (1..=4u64).zip(kinds.iter()) {
        let r = row_of(&m, t);
        let expected = row_meta(&row(t), metas.get(&TabId(t)), Some(Path::new(HOME)));
        assert_eq!(r.project, "terminal", "tab {t}: the project is R");
        assert_eq!(
            Some(r.branch_line.clone()),
            expected.branch,
            "tab {t} branch"
        );
        assert_eq!(
            (r.agent_label.as_str(), r.agent),
            (label, tag),
            "tab {t} agent"
        );
        assert_eq!(
            r.bucket,
            bucket_of(expected.status, false, false),
            "tab {t} status"
        );
    }
}

// ---- R14.2 ------------------------------------------------------------------

/// R14.2: a split tab with a Claude active pane (main worktree) and a Codex non-active pane
/// (linked worktree `w`) is two session rows of R, and the Codex row's state is
/// `resolve_status(pane.agent, false, false, None)`.
#[test]
fn r14_2_mixed_agents_in_one_split_tab() {
    let claude = agent(AgentKind::Claude, AgentState::Busy);
    // Waiting, so the pane's own status differs from the tab's (Claude, Busy → Working).
    let codex = agent(AgentKind::Codex, AgentState::Waiting);
    let codex_pane = PaneMeta {
        graph_transcript: None,
        graph_agent_cwd: None,
        graph_asks_user: false,
        pane_pid: 21,
        active: false,
        agent: codex,
        log: None,
        cwd: Some(home().join("wt/w")),
        branch: Some("feat/w".to_string()),
        repo: Some(linked()),
    };
    let tab = TabMeta {
        agent: claude,
        cwd: Some(home().join(REPO)),
        branch: Some("main".to_string()),
        repo: Some(repo()),
        panes: vec![
            codex_pane.clone(),
            PaneMeta {
                graph_transcript: None,
                graph_agent_cwd: None,
                graph_asks_user: false,
                pane_pid: 20,
                active: true,
                agent: claude,
                log: None,
                cwd: Some(home().join(REPO)),
                branch: Some("main".to_string()),
                repo: Some(repo()),
            },
        ],
        ..TabMeta::default()
    };
    let rows = vec![row(7)];
    let metas = metas(vec![(7, tab)]);

    let m = build(&rows, &metas, &HashMap::new());

    assert_eq!(m.rows.len(), 2, "two session rows: {:#?}", m.rows);
    assert!(
        m.rows.iter().all(|r| r.project == "terminal"),
        "both are R's"
    );
    let claude_row = m
        .rows
        .iter()
        .find(|r| r.pane == 20)
        .expect("the Claude pane's row");
    let codex_row = m
        .rows
        .iter()
        .find(|r| r.pane == 21)
        .expect("the Codex pane's row");
    assert_eq!(claude_row.agent, AgentTag::Claude);
    assert_eq!(claude_row.branch_line, "main");
    assert_eq!(codex_row.agent, AgentTag::Codex);
    assert_eq!(
        codex_row.branch_line, "feat/w · w",
        "the linked worktree's pane shows its own branch and worktree"
    );
    assert_eq!(
        codex_row.bucket,
        bucket_of(
            resolve_status(codex.as_ref(), false, false, None),
            false,
            false
        )
    );
}

// ---- R14.3 ------------------------------------------------------------------

/// R14.3: with both logs in the digest map, the Claude session shows its two subagents; the
/// Codex session shows none.
#[test]
fn r14_3_subagents_only_under_claude() {
    let codex_log = PathBuf::from("/nonexistent/tt14/codex.jsonl");
    let claude_log = PathBuf::from("/nonexistent/tt14/claude.jsonl");
    let rows = vec![row(1), row(2)];
    let metas = metas(vec![
        (
            1,
            unsplit(agent(AgentKind::Codex, AgentState::Busy), Some(&codex_log)),
        ),
        (
            2,
            unsplit(
                agent(AgentKind::Claude, AgentState::Busy),
                Some(&claude_log),
            ),
        ),
    ]);
    let digests = HashMap::from([
        (
            codex_log.clone(),
            digest(vec![subagent("c1", false), subagent("c2", true)]),
        ),
        (
            claude_log.clone(),
            digest(vec![subagent("a1", false), subagent("a2", true)]),
        ),
    ]);

    let m = build(&rows, &metas, &digests);

    let claude = row_of(&m, 2);
    assert_eq!(claude.subagents.len(), 2, "{:#?}", claude);
    assert_eq!(claude.tree.len(), 2);
    assert_eq!(claude.subagent_summary, "1 running · 1 done");
    let codex = row_of(&m, 1);
    assert!(
        codex.subagents.is_empty() && codex.tree.is_empty(),
        "the Codex session has no subagents: {codex:#?}"
    );
    assert_eq!(codex.subagent_summary, "—");
}

// ---- R14.4 ------------------------------------------------------------------

/// R14.4: Antigravity (Working), Codex (Waiting for input, no dialog) and Claude (Ready)
/// sessions are all counted — one working, two idle, three in all.
#[test]
fn r14_4_counts_every_agent_session() {
    let rows = vec![row(1), row(2), row(3)];
    let metas = metas(vec![
        (
            1,
            unsplit(agent(AgentKind::Antigravity, AgentState::Busy), None),
        ),
        (
            2,
            unsplit(agent(AgentKind::Codex, AgentState::Waiting), None),
        ),
        (3, unsplit(agent(AgentKind::Claude, AgentState::Idle), None)),
    ]);

    let m = build(&rows, &metas, &HashMap::new());

    assert_eq!(
        m.counts,
        Counts {
            all: 3,
            needs_you: 0,
            working: 1,
            idle: 2,
        }
    );
    assert_eq!(row_of(&m, 1).bucket, Bucket::Working);
}
